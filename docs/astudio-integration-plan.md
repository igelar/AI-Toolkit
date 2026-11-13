# 讯飞 AStudio（星辰Agent）积分领取平台接入技术方案

> 文档日期：2026-11-12（Phase 0 协议实测完成后重写）  
> 项目：AI-Toolkit（Tauri + React + TypeScript + Rust）  
> 目标：在现有 4 个 agent 积分平台（WorkBuddy、TRAE、Qoder、MiniMax Code）基础上，新增讯飞 AStudio 平台，支持本地凭据导入、积分余额查询、每日签到领取、模型目录发现与代理推理。  
> 状态：**Phase 0 协议实测已完成**，本文所有协议契约均有实测或客户端代码证据；剩余待验证项见 §6.1。

---

## 一、现有架构梳理

### 1.1 整体架构

AI-Toolkit 是一个 Tauri 桌面应用，credits（积分）模块采用 **Provider 适配模式**：每个 agent 平台是一个独立的 Rust adapter（`src-tauri/src/credits/providers/<platform>.rs`），统一实现 8 个函数，由 `ProviderClient` 分发。前端通过 Tauri command 调用后端，通过 React 组件渲染。

### 1.2 四个平台的接入模式对比

| 维度 | WorkBuddy | TRAE | Qoder | MiniMax Code |
|---|---|---|---|---|
| 登录方式 | OAuth（PKCE + 浏览器回调） | OAuth（IDE client + 本地回调端口） | OAuth（设备码轮询） | OAuth（设备码）+ 本机导入 |
| 本机导入 | ✗ | ✓（桌面/CLI 凭据） | ✗ | ✓（`~/.minimax/auth/` 明文 JSON） |
| 凭据刷新 | refresh_token | refresh_token | refresh_token | 本地 lease 重读 / refresh_token |
| 积分余额 | `/v3/config` 计费 | billing API | billing API | `/matrix/.../get_membership_info` |
| 签到机制 | 国际版自动 | UG 网关 checkin | campaign claim | signin status + claim |
| 模型代理协议 | Messages/OpenAI | Messages/OpenAI | OpenAI | Messages/OpenAI |
| 流解析 | 通用 | 通用（extra_info 特殊） | 信封解包（body 字段） | 专用（message_stop 校验） |

### 1.3 接入一个平台需要改动的位置（全链路清单）

**后端（Rust）：**

| # | 文件 | 改动内容 | 必要性 |
|---|---|---|---|
| 1 | `credits/types.rs` | `Platform` 枚举新增 `AStudio`；`key()` 新增分支 | 必须 |
| 2 | `credits/providers.rs` | `mod astudio;` 声明；`ProviderClient` 8 个 match 分支 | 必须 |
| 3 | `credits/providers/astudio.rs` | **新建** — 核心适配器 | 必须 |
| 4 | `credits/providers/astudio_responses.rs` | **新建** — chat/completions → Responses API 请求转换 | 必须 |
| 5 | `credits/providers/stream.rs` | 新增 `parse_astudio` 分支（Responses SSE 事件映射） | 必须 |
| 6 | `credits/oauth.rs` | `start_login` / `poll_login` 穷举 match 新增显式"不支持网页登录"分支 | 必须（否则编译失败） |
| 7 | `credits/mod.rs` | `add_account` 导入时预刷新特判（AStudio 始终重发模型 JWT） | 必须 |
| 8 | `commands/credits.rs` | `credits_import_local` 新增 AStudio 分支 | 必须 |
| 9 | `credits/gateway.rs` | 无需改逻辑（`model_matches` 全字符串比较，支持含 `/` 的 slug）；测试循环建议加入 AStudio | 可选 |

**前端（TypeScript/React）：**

| # | 文件 | 改动内容 |
|---|---|---|
| 10 | `src/lib/api/credits.ts` | `CreditPlatform` 类型新增 `"astudio"` |
| 11 | `src/components/credits/CreditsPanel.tsx` | `platforms` 数组；导入 UI；**区域选择器仅 Cn**；模型分组跳过 intl |
| 12 | `src/components/ui/AgentIcon.tsx` | 图标映射 |
| 13 | `src/assets/agents/` | 图标文件 + `README.md` 来源记录 |
| 14 | `src-tauri/src/credits/providers/NOTICE.md` | 协议参考文档新增 AStudio 章节 |

### 1.4 Provider 适配器的统一接口

每个平台适配器实现以下 8 个 `pub(super)` 函数（签名见 `providers.rs`）：

```rust
async fn authenticate(client, c) -> Result<(String, String, Credentials), ProviderError>
async fn refresh(client, c) -> Result<Credentials, ProviderError>
async fn inspect(client, c) -> Result<ProviderSnapshot, ProviderError>
async fn balance(client, c) -> Result<ProviderSnapshot, ProviderError>
async fn rewards(client, c) -> Result<Vec<Reward>, ProviderError>
async fn models(client, c) -> Result<Vec<ModelInfo>, ProviderError>
async fn claim(client, account, c) -> Result<Vec<RewardOutcome>, ProviderError>
async fn chat(client, c, model, body) -> Result<ProviderChat, ProviderError>
```

`balance` / `rewards` / `models` 由 `ProviderClient::inspect_parts` 按 `Scope` 并发调用后合并（刷新调度走这条路径）；`inspect` 是全量快照。

---

## 二、AStudio 协议契约（Phase 0 实测确认）

### 2.1 平台基本信息

| 属性 | 值 | 证据 |
|---|---|---|
| 产品名 | AStudio（Astron Studio，讯飞星辰Agent 桌面客户端） | 安装目录 |
| 客户端版本 | 3.6.2（内核 `@iflytek/astron-code-prod` 1.0.6-rc.11，Codex 分支，Rust 原生二进制） | `acode-runtime-manifest.json`、内核日志 |
| 官网 | `https://agent.xfyun.cn` | 实测 |
| 平台标识 | `astudio` | 建议 key |
| 区域 | 仅中国大陆（Cn） | 无国际版部署 |
| 账号体系 | 讯飞开放平台账号（手机号 SMS / 微信扫码 SSO） | 会话文件 `loginMethod`、bundle 登录接口 |
| 积分单位 | 通用积分 + 星火积分（独立桶），映射为 `credits` / `spark` | balance 实测 |

### 2.2 会话文件与双令牌架构

**路径（两种安装布局）：**

| 布局 | 会话文件路径 |
|---|---|
| 标准安装 | `%APPDATA%/AStudio/userdata/astron-session.json`（macOS：`~/Library/Application Support/AStudio/…`） |
| 便携版 | `<数据目录>/userdata/astron-session.json`（实测本机：`F:\AStudio Data\userdata\astron-session.json`） |

便携版数据目录不在默认位置，适配器必须支持 `ASTUDIO_DATA_DIR` 环境变量覆盖（参照 `MINIMAX_DATA_DIR` 先例）。

**实际文件结构（实测，明文 JSON）：**

```json
{
  "accountId": "22093331744",
  "uid": "22093331744",
  "token": "<UUID>",
  "ssoSessionId": "<UUID，与 token 同值>",
  "modelBearerToken": "<JWT>",
  "loginMethod": "sms",
  "loggedInAt": "2026-11-11T11:24:17.055Z",
  "mobile": "<明文手机号>",
  "isNew": true,
  "banned": false
}
```

**与原文档推断的差异：没有 `expiresAt`、没有 `refreshToken`、没有 `modelCredentials` map。**

**双令牌：**

| 令牌 | 形态 | 用途 | 鉴权方式 |
|---|---|---|---|
| `token` / `ssoSessionId` | UUID | studio 账号/积分/弹窗 API | Cookie（见 §2.3） |
| `modelBearerToken` | JWT（HS256，claims `{uid, iat, exp}`，有效期约 3 年） | volces 模型目录 + 推理 | `Authorization: Bearer` |

**JWT 重发机制（关键）**：`POST {studio}/auth/bearer-token`（Cookie 鉴权）→ `data.modelBearerToken` 返回新 JWT。桌面端每次启动内核及遇到 401 时调用（bundle `fetchAstronModelCredential` / `refreshAstronSessionCredentialAfterUnauthorized`）。**文件中的 JWT 可能已轮换失效——实测直接用文件 JWT 请求模型接口被拒，重发后即通过。** 适配器的 refresh 必须主动重发，不能只重读文件。

SSO 令牌有效期文件中无记录，失效判定靠响应：HTTP 401/403 或业务码 **1002 / 1005**（bundle `requestAstronAccountData` 确认）。

> 隐私注意：会话文件含明文手机号。错误消息与诊断不得回显文件内容；label 不使用手机号。

### 2.3 上游服务、鉴权与业务信封

**三个 base URL（全部实测确认）：**

| 服务 | Base URL | 鉴权 |
|---|---|---|
| studio（账号/积分/弹窗） | `https://agent.xfyun.cn/xingchen-studio/` | Cookie + 客户端标识头 |
| volces 模型目录 | `https://astronstudio-api-volces-prod.xf-yun.com/api/v1/model-manager/models` | Bearer JWT |
| volces 推理 | `https://astronstudio-api-volces-prod.xf-yun.com/model-gateway/v1/responses` | Bearer JWT |

> ⚠️ 内核二进制内嵌的 `https://maas-api.cn-huabei-1.xf-yun.com/v1` 是默认模板，运行时被桌面端覆盖为 volces `model-gateway/v1`（bundle `resolveAstronRuntimeProfile`：`maasBaseUrl = astronCodeApiUrl + "/model-gateway/v1"`）。**直连 maas-api 需要另一套 HMAC/apikey 鉴权，实测 Bearer JWT 被拒——不要使用。**

**studio Cookie 构造（bundle `buildAstronCookieHeader$1` 确认）：**

```
Cookie: ssoSessionId=<token>; sso_sessionid=<token>; account_id=<accountId>; token=<token>
clientType: 21        # win32；macOS 为 22
studioVersion: 3.6.2  # 客户端版本
Accept: application/json
```

**两种业务信封：**

| 服务 | 信封 | 成功条件 | 登录失效 |
|---|---|---|---|
| studio | `{flag, code, desc, count, data, traceId}` | `flag===true && code===0 && data!=null` | HTTP 401/403 或 code 1002/1005 |
| volces | `{code, message, traceId, data}` | `code===0` | HTTP 401 |

现有 `check_business` 识别 `code/biz_code/bizCode`，volces 信封可直接复用；studio 信封需适配器补充 `flag` 校验与 1002/1005 → 401 映射（见 §3.1.3）。

### 2.4 积分接口（实测响应）

**`GET points/balance`** — 四桶 + 到期时间：

```json
{"flag":true,"code":0,"desc":"成功","data":{
  "totalAmount":1200,"totalBalance":1200,
  "memberTotal":0,"memberBalance":0,
  "buyTotal":0,"buyBalance":0,
  "activityTotal":1200,"activityBalance":1200,
  "memberNextExpireTime":null,"buyNextExpireTime":null,
  "activityNextExpireTime":"2026-12-12T19:24:37",
  "sparkTotalAmount":5000,"sparkTotalBalance":5000,
  "sparkMemberTotal":0,"sparkMemberBalance":0,
  "sparkActivityTotal":5000,"sparkActivityBalance":5000
},"traceId":"…"}
```

- `totalBalance` 为通用积分合计（= member + buy + activity），**不含星火**；星火是独立桶
- `*NextExpireTime` 为无时区本地时间字符串（CST），实测 activity 到期 = 登录 + 31 天，与官方规则一致
- 桶语义：member=套餐、buy=增购、activity=活动（含每日登录与新用户赠送）

**`GET points/summary`** → `{totalBalance, purchaseTarget: "NONE"|"MEMBERSHIP_PLAN"|"POINT_ADD_ON", sparkTotalBalance}`（注意上游路径是 `points/summary`，不是 `client-popups/points-summary`）

**`GET membership/me`** → `{uid: <number>, active, planCode:"TRIAL", planName:"体验版", billingMode, autoRenew, startTime, endTime, nextRenewTime, source}`（authenticate 用；uid 是数字）

**`GET membership/plan-config`** → `{dailyLoginEnabled:true, dailyLoginValidDays:31, plans:[{planCode:"TRIAL",dailyLoginPoints:100},{BASIC:200},{PRO:400},{ENJOY:400}]}`

**积分规则（官方 + plan-config 确认）：** 每日登录体验版 100 / 标准版 200 / 高级版与畅享版 400，有效期 31 天；新用户赠送 1000（3 个月）。

### 2.5 签到弹窗（全链路实测确认，2026-11-13）

**接口（上游路径均相对 studio base）：**

| 功能 | 方法 | 路径 | body |
|---|---|---|---|
| 待领取弹窗 | GET | `client-popups/pending` | — |
| **每日积分领取** | POST | `client-popups/complete` | `{popupId, instanceKey, completed:true}` |
| 客户端下载奖励 | POST | `client-popups/claim` | 无 |
| 新手奖励 | POST | `client-popups/new-user-onboarding/claim` | 无 |

> ⚠️ **`claim`（无 body）是客户端下载奖励，不是每日签到。** 每日积分通过 `complete` 发放（bundle 每日弹窗 `onComplete` → `completeAstronClientPopup`，上游 `client-popups/complete`）。原文档混淆了二者。

**pending 响应**：`data` 直接是数组。每日弹窗真实结构（实测捕获，见 `docs/astudio-daily-claim-capture.txt`）：

```json
{"popupId":2,"popupCode":"DAILY_LOGIN_REWARD_V1","popupType":"BANNER",
 "componentType":"DAILY_REWARD_DIALOG","priority":100,
 "instanceKey":"20261113",
 "payload":{"confirmText":"知道了","title":"今日登录积分已到账",
            "content":"每日登录积分已发放，可前往积分中心查看。"}}
```

- `componentType` 枚举：`CLIENT_DOWNLOAD_REWARD_DIALOG` / `NEW_USER_DIALOG` / `NEW_USER_DIALOG_V2` / `DAILY_REWARD_DIALOG`，只处理最后一种
- `popupId` 是静态小整数（实测 2，number）；**每日唯一键是 `instanceKey` = YYYYMMDD 日期串**，reward id 直接用它
- **payload 没有 points 字段**（纯通知文案）——积分数额须按账号套餐解析：`membership/me` 的 `planCode` → `membership/plan-config` 的 `plans[].dailyLoginPoints`（实测 TRIAL=100）
- 弹窗唯一键：`${popupId}:${instanceKey}`

**complete 契约（实测）：**

- 请求 `{popupId:2, instanceKey:"20261113", completed:true}` → 响应 `{flag:true, code:0, data:null}`。**成功响应 data 为 null**，写路径不能要求 data 非空
- **重复提交幂等**：再次 complete 同样返回 200 成功——"已领取"无法从 complete 响应判定，只能靠 pending 是否存在弹窗
- **生命周期**：complete 后弹窗从 pending 消失（实测 pending 回查为 `[]`）
- **发放确认**：complete 触发发放，实测 activityBalance 1200 → 1300（+100，与 TRIAL 每日积分一致）

现有通用 `reconcile_claim` 要求回查出现 `status="claimed"` 的同 id reward，**不适用于弹窗消失模型**，需适配器本地回查逻辑（见 §3.1.3）。

### 2.6 模型目录（实测）

**`GET {volces}/api/v1/model-manager/models`**（Bearer JWT）→ `data.models[]`：

```json
{"slug":"xopglm52","display_name":"GLM-5.2","description":"GLM-5.2 文本模型。",
 "context_window":1048576,"input_modalities":["text"],"supported_in_api":true,
 "priority":6,"shell_type":"unified_exec","visibility":"list","service_tiers":[],
 "supported_reasoning_levels":[{"effort":"none","description":"关闭思考"},…],
 "default_reasoning_level":"high","base_instructions":"…"}
```

实测 10 个模型：`astronclaw-auto`（Auto）、`spark-x2.6`、`spark-x2.6-4b`（Lite）、`spark-x2.5`、`zhipu/glm-5.3`、`zhipu/glm-5.3-flash`、`xopglm52`（GLM-5.2）、`xopdeepseekv4pro0813`（DeepSeek-V4-Pro）、`xopdsv4flash0731in`（DeepSeek-V4-Flash）、`moonshot/kimi-k2.8-preview`（Kimi-K2.8）。

- **slug 含 `/`**（`zhipu/…`、`moonshot/…`）——ID 校验字符集必须放行 `/`；gateway `model_matches` 是全字符串比较（`route_key + "/" + model.id`），路由 `astudio/zhipu/glm-5.3` 无需特殊处理（已核对 gateway.rs:339）
- 推理档位：`none` / `high` / `max`（部分模型仅 none/high）
- 本机缓存可作离线兜底：`userdata/provider-model-catalog-v1.json`（UI 版）、`userdata/model-gateway/catalog-<hash>.json`（完整版）

### 2.7 推理协议（实测——与原文档假设完全不同）

**上游只支持 OpenAI Responses API**（内核配置 `wire_api="responses"`）：

- `POST {volces}/model-gateway/v1/responses` ✓（零成本探针确认鉴权通过）
- `POST …/v1/chat/completions` → **404 "API endpoint not found"，不支持**
- 原文档"标准 OpenAI Chat Completions + 通用 SSE、stream.rs 零改动"的假设**不成立**，必须实现协议转换层

**最小请求（实测成功，消耗 38 token）：**

```json
{"model":"spark-x2.5","input":"hi","stream":true,"max_output_tokens":16,"reasoning":{"effort":"none"}}
```

**SSE 事件序列（完整捕获见 `docs/astudio-responses-sse-capture.txt`）：**

```
response.created            # response 骨架，含 model 回显
response.in_progress
response.output_item.added  # item: {type:"message", role:"assistant", …}
response.content_part.added # part: {type:"output_text", text:""}
response.output_text.delta × N   # {delta:"…", content_index, item_id, output_index}
response.output_text.done
response.content_part.done
response.output_item.done
response.incomplete         # 或 response.completed；终止帧，携带 usage
```

**关键事实：**

1. **`model` 回显 = 请求 slug**（`"spark-x2.5"`）——`identity_matches` 现有大小写不敏感比较即可，无需特判
2. **没有 `data: [DONE]`**——流以 `response.completed` / `response.incomplete` / `response.failed` 事件结束
3. 截断终止帧为 `response.incomplete` + `incomplete_details.reason:"max_output_tokens"` → 映射 `finish_reason:"length"`
4. usage 在终止帧 `response.usage`：`{input_tokens, output_tokens, total_tokens, input_tokens_details:{cached_tokens}, output_tokens_details:{reasoning_tokens}}`。实测 input=22（"hi" 仅 1 token）——网关注入系统指令，记账以上游返回为准
5. `reasoning:{effort:"none"}` 被接受且 `reasoning_tokens:0`
6. 响应 ID 形如 `cht000d5896@dx1a…`（含 `@`）；响应头有 `x-request-id` / `x-gateway-request-id`，可接入现有诊断
7. SSE 帧带 `event:` 类型行（stream.rs 已解析 event 行，有 minimax 先例）
8. **思考事件（effort=high 实测）**：`response.reasoning_summary_part.added` → `response.reasoning_summary_text.delta` ×N（帧含 `delta`、`summary_index`、`item_id` 前缀 `rs_`）→ `…text.done` → `…part.done`；思考内容在 output item 的 `summary[].text`（type=summary_text）
9. **工具调用事件（带 tools 实测）**：`response.output_item.added`（item.type=function_call，含 `call_id`/`name`，形如 `call_26d9…`）→ `response.function_call_arguments.delta`（`delta` 为 arguments JSON 分片）→ `…arguments.done`（完整 arguments）→ `output_item.done` → `response.completed`。**终止帧不区分工具调用**，finish_reason 须从 output items 推导（含 function_call → tool_calls）

### 2.8 与现有平台的关键差异

| 维度 | AStudio 特殊之处 | 对应处理 |
|---|---|---|
| 凭据 | 双令牌（SSO UUID + 模型 JWT），无 expiresAt/refreshToken | access_token=JWT，SSO 存 extra；refresh 重读文件 + 重发 JWT |
| 鉴权 | studio 用 Cookie，volces 用 Bearer | 两套请求辅助函数 |
| 签到 | 弹窗 complete 发放，claim 是另一种奖励 | rewards/claim 走 pending/complete；确认=弹窗消失 |
| 推理协议 | **仅 Responses API** | 新建请求转换模块 + stream 专用分支 |
| 区域 | 仅 Cn | 前端隐藏 Intl；导入入口拒绝 Intl |
| 积分 | 多桶 + 到期时间 + 独立星火桶 | CreditPack 映射（含 expires_at） |

---

## 三、接入方案设计

### 3.1 后端改动详细设计

#### 3.1.1 `credits/types.rs` — Platform 枚举扩展

```rust
pub enum Platform {
    Workbuddy, Traework, Qoder, MinimaxCode,
    AStudio,   // serde rename_all="lowercase" 自动序列化为 "astudio"
}
// key() 新增 Self::AStudio => "astudio"
```

`route_key` 沿用通用逻辑（Intl 会追加 `-intl` 后缀）。**AStudio 仅 Cn 的约束在入口实现**：前端区域选择器禁用 Intl、`local_credentials_for_region` 对 Intl 返回 None。不在 `route_key` 特判。

#### 3.1.2 `credits/providers.rs` — 分发接线

`mod astudio;` + `ProviderClient` 的 authenticate / refresh_credentials / inspect / inspect_parts（balance、rewards、models 三处）/ claim / chat 共 8 个 match 分支，模式与 MinimaxCode 完全一致。

#### 3.1.3 `credits/providers/astudio.rs` — 核心适配器（新建）

**凭据映射：**

```rust
Credentials {
    region: CreditRegion::Cn,
    access_token: <modelBearerToken>,      // volces Bearer
    refresh_token: None,
    user_id: Some(<uid>),
    device_id: None,
    expires_at: <JWT exp × 1000>,          // jwt_claims() 现有辅助函数解码
    extra: json!({
        "localImport": true,
        "ssoSessionId": <token>,           // studio Cookie 用
        "accountId": <accountId>,
    }),
}
```

extra 随 Credentials 整体存入 OS credential store（`secrets::SecretStore`），不落明文盘。

**常量与路径定位：**

```rust
const STUDIO_ORIGIN: &str = "https://agent.xfyun.cn/xingchen-studio/";
const VOLCES_ORIGIN: &str = "https://astronstudio-api-volces-prod.xf-yun.com";
const STUDIO_VERSION: &str = "3.6.2";

fn astudio_data_dir() -> Result<PathBuf, String> {
    // 1. ASTUDIO_DATA_DIR 环境变量覆盖（便携版必需，参照 MINIMAX_DATA_DIR）
    // 2. Windows: dirs::config_dir()/AStudio（即 %APPDATA%/AStudio）
    //    macOS: ~/Library/Application Support/AStudio
    //    其他平台: 返回 Err（无 Linux 客户端）
}
// 会话文件 = data_dir/userdata/astron-session.json
```

**parse_session（实测字段）：**

```rust
fn parse_session(value: &Value) -> Result<Credentials, String> {
    let sso = text(value, &["ssoSessionId", "token"]).ok_or("本机 AStudio 缺少会话凭据")?;
    let account_id = text(value, &["accountId"]).ok_or("本机 AStudio 缺少账号标识")?;
    let jwt = text(value, &["modelBearerToken"]).ok_or("本机 AStudio 缺少模型凭据")?;
    let uid = text(value, &["uid", "accountId"]).ok_or("本机 AStudio 缺少用户标识")?;
    let exp = jwt_claims(&jwt)["exp"].as_i64().map(|e| e * 1000);
    Ok(Credentials {
        region: CreditRegion::Cn, access_token: jwt, refresh_token: None,
        user_id: Some(uid), device_id: None, expires_at: exp,
        extra: json!({"localImport": true, "ssoSessionId": sso, "accountId": account_id}),
    })
}
```

注意：`expires_at` 来自 JWT（约 3 年），**不代表 SSO 有效期**；SSO 失效靠 1002/1005 响应判定。文件无过期字段，不做导入时过期预检（与 MiniMax 强制 expiresAtMs 不同）。

**两套请求辅助：**

```rust
// studio：Cookie + 客户端标识头
fn studio_request(client, c, path, body: Option<&Value>) -> RequestBuilder {
    let sso = c.extra["ssoSessionId"].as_str()…;
    let account = c.extra["accountId"].as_str()…;
    let cookie = format!("ssoSessionId={sso}; sso_sessionid={sso}; account_id={account}; token={sso}");
    // GET 或 POST（body 存在时），headers: Cookie, clientType: "21"/"22", studioVersion, Accept
}

async fn studio_read(client, c, path, body) -> Result<Value, ProviderError> {
    match client.json(studio_request(…)).await {
        Ok(v) => {
            // check_business 已校验 code；补充 flag 校验
            if v["flag"].as_bool() != Some(true) || v.get("data").is_none() {
                return Err(ProviderError::new(502, "AStudio 返回了无效的数据信封"));
            }
            Ok(v)
        }
        Err(e) => Err(remap_session_error(e)),
    }
}

// studio 写操作（complete）：实测成功响应 data 为 null，只校验 flag/code，不要求 data
async fn studio_write(client, c, path, body) -> Result<(), ProviderError> {
    let v = client.json(studio_request(…)).await.map_err(remap_session_error)?;
    if v["flag"].as_bool() != Some(true) {
        return Err(ProviderError::new(502, "AStudio 返回了无效的数据信封"));
    }
    Ok(())
}

// SSO 失效映射：check_business 把 1002/1005 归为 502，这里提升为 401 rejected
fn remap_session_error(e: ProviderError) -> ProviderError {
    if matches!(e.upstream_code.as_deref(), Some("1002" | "1005")) || matches!(e.status, 401 | 403) {
        ProviderError { status: 401, rejected: true, message: "AStudio 登录已失效，请在客户端重新登录".into(), ..e }
    } else { e }
}

// volces：Bearer JWT，信封 {code, message, data}，check_business 直接适用
async fn volces_read(client, c, path) -> Result<Value, ProviderError> {
    client.json(client.http.get(format!("{VOLCES_ORIGIN}{path}"))
        .bearer_auth(&c.access_token).header("Accept", "application/json")).await
}
```

**authenticate：**

```rust
pub(super) async fn authenticate(client, c) -> Result<(String, String, Credentials), ProviderError> {
    let v = studio_read(client, c, "membership/me", None).await?;
    let user = data(&v);
    let id = text(user, &["uid"]).ok_or_else(|| ProviderError::new(502, "AStudio 未返回账号 ID"))?;
    let label = text(user, &["planName"]).map(|p| format!("AStudio {p}")).unwrap_or_else(|| "AStudio".into());
    let mut credentials = c.clone();
    credentials.user_id = Some(id.clone());
    Ok((id, label, credentials))
}
```

**refresh（重读文件 + 重发 JWT）：**

```rust
pub(super) async fn refresh(client, c) -> Result<Credentials, ProviderError> {
    // 1. 重读本机会话文件（客户端可能已轮换 SSO）
    let local = local_credentials_for_region(CreditRegion::Cn)
        .map_err(|m| ProviderError::new(401, m))?
        .ok_or_else(|| ProviderError::new(401, "未找到本机 AStudio 登录信息，请先在客户端登录"))?;
    // 2. 账号切换检测
    if c.user_id.as_ref().is_some_and(|expected| expected != local.user_id.as_ref().unwrap()) {
        return Err(ProviderError::new(401, "本机 AStudio 已切换账号，请重新导入"));
    }
    // 3. 用本机 SSO 重发模型 JWT（文件中的 JWT 可能已失效）
    let v = studio_read(client, &local, "auth/bearer-token", Some(&json!({}))).await?;
    let jwt = text(data(&v), &["modelBearerToken"])
        .ok_or_else(|| ProviderError::new(502, "AStudio 未返回模型凭据"))?;
    let mut refreshed = local;
    refreshed.access_token = jwt;
    refreshed.expires_at = jwt_claims(&refreshed.access_token)["exp"].as_i64().map(|e| e * 1000);
    Ok(refreshed)
}
```

SSO 失效时 `studio_read` 返回 401 rejected，文案提示在客户端重新登录（第一阶段无网页 OAuth，**不使用"或在此使用网页登录"措辞**）。

**balance（CreditPack + 到期时间）：**

```rust
pub(super) async fn balance(client, c) -> Result<ProviderSnapshot, ProviderError> {
    let v = studio_read(client, c, "points/balance", None).await?;
    let d = data(&v);
    let total = num(d, &["totalBalance"]).filter(|n| n.is_finite() && *n >= 0.0)
        .ok_or_else(|| ProviderError::new(502, "AStudio 未返回可识别的积分余额"))?;
    let pack = |id: &str, name: &str, balance_key: &str, expire_key: &str, unit: &str| CreditPack {
        id: id.into(), name: name.into(), unit: unit.into(),
        remaining: num(d, &[balance_key]).map(|n| n.max(0.0)),
        expires_at: d.get(expire_key).and_then(parse_cst_datetime),
    };
    let packs = vec![
        pack("member", "套餐积分", "memberBalance", "memberNextExpireTime", "credits"),
        pack("buy", "增购积分", "buyBalance", "buyNextExpireTime", "credits"),
        pack("activity", "活动积分", "activityBalance", "activityNextExpireTime", "credits"),
        pack("spark", "星火积分", "sparkTotalBalance", "", "spark"),  // 无到期字段
    ];
    let mut result = snapshot(packs, vec![], vec![], None);
    result.balance = Some(total);   // 服务端汇总权威（含到期过滤），不依赖 packs 求和
    result.unit = "credits".into();
    Ok(result)
}

// "2026-12-12T19:24:37" → CST(+8) 毫秒时间戳；现有 epoch() 不支持 T 分隔格式，适配器本地实现
fn parse_cst_datetime(v: &Value) -> Option<i64> { … chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") + FixedOffset::east(8*3600) … }
```

星火桶 unit 为 `spark`，与通用积分分离；`snapshot()` 混合 unit 时 balance 为 None，故显式覆写为 `totalBalance`（MiniMax `balance_snapshot` 同款模式）。

**rewards / claim（共享弹窗解析辅助）：**

```rust
struct DailyPopup { popup_id: u64, instance_key: String }  // instance_key = "20261113" 日期串

async fn pending_daily_popup(client, c) -> Result<Option<DailyPopup>, ProviderError> {
    let v = studio_read(client, c, "client-popups/pending", None).await?;
    let Some(popup) = data(&v).as_array().into_iter().flatten()
        .find(|p| p["componentType"].as_str() == Some("DAILY_REWARD_DIALOG")) else { return Ok(None) };
    // popupId 实测为 number（静态值 2），仍兼容字符串
    let popup_id = num(popup, &["popupId"]).filter(|n| *n > 0.0 && n.fract() == 0.0)
        .ok_or_else(|| ProviderError::new(502, "AStudio 弹窗 popupId 无效"))? as u64;
    let instance_key = text(popup, &["instanceKey"])
        .ok_or_else(|| ProviderError::new(502, "AStudio 弹窗缺少 instanceKey"))?;
    Ok(Some(DailyPopup { popup_id, instance_key }))
}

// payload 无 points（实测为纯通知文案）：按账号套餐解析每日积分额
async fn daily_points(client, c) -> Option<f64> {
    let (me, cfg) = tokio::join!(
        studio_read(client, c, "membership/me", None),
        studio_read(client, c, "membership/plan-config", None));
    let plan_code = text(data(&me.ok()?), &["planCode"])?;
    data(&cfg.ok()?)["plans"].as_array()?.iter()
        .find(|p| p["planCode"].as_str() == Some(&plan_code))
        .and_then(|p| num(p, &["dailyLoginPoints"]))
}

fn daily_reward(popup: &DailyPopup, amount: Option<f64>) -> Reward {
    Reward {
        // instanceKey 即 YYYYMMDD 日期串，服务端提供的稳定每日标识
        id: format!("daily-login-{}", popup.instance_key),
        name: "每日登录积分".into(), kind: "credits".into(),
        status: "claimable".into(), amount,
        next_available_at: None, action_url: None,
    }
}

pub(super) async fn rewards(client, c) -> Result<Vec<Reward>, ProviderError> {
    let Some(popup) = pending_daily_popup(client, c).await? else { return Ok(vec![]) };
    let amount = daily_points(client, c).await;
    Ok(vec![daily_reward(&popup, amount)])
}

pub(super) async fn claim(client, account, c) -> Result<Vec<RewardOutcome>, ProviderError> {
    let Some(popup) = pending_daily_popup(client, c).await? else {
        // 无待领取弹窗：返回 outcome 而非 Err（避免污染账号错误状态）
        let placeholder = Reward { id: …, status: "unavailable".into(), … };
        return Ok(vec![outcome(account, &placeholder, "unavailable",
            "当前没有可领取的签到积分（可能已领取或尚未发放）", None)]);
    };
    let amount = daily_points(client, c).await;
    let reward = daily_reward(&popup, amount);
    // complete 成功响应 data:null，走 studio_write；重复提交幂等（实测）
    let submitted = studio_write(client, c, "client-popups/complete",
        Some(&json!({"popupId": popup.popup_id, "instanceKey": popup.instance_key, "completed": true}))).await;
    match submitted {
        Ok(_) => Ok(vec![outcome(account, &reward, "claimed", "每日登录积分已领取", amount)]),
        Err(e) if e.rejected => Ok(vec![outcome(account, &reward, "failed", e.message, None)]),
        Err(_) => {
            // 模糊/超时：回查一次 pending，弹窗消失 = 已领取；不重发
            let after = pending_daily_popup(client, c).await;
            let confirmed = after.as_ref().is_ok_and(|p| p.is_none());
            Ok(vec![outcome(account, &reward,
                if confirmed { "claimed" } else { "pending" },
                if confirmed { "回查确认弹窗已完成，积分已领取" }
                else { "领取结果尚未确认，已回查状态；不会自动重发领取请求" },
                if confirmed { amount } else { None })])
        }
    }
}
```

> 不复用通用 `reconcile_claim`：它要求回查出现 `status="claimed"` 的同 id reward，而 AStudio 领取后弹窗直接消失，永远无法满足该条件。弹窗消失即确认是本适配器的专用语义。重复 complete 实测幂等（同样 200 成功），故"已领取"判定完全依赖 pending 状态；complete 成功响应 `data:null`，必须走 `studio_write`。

**models（volces 目录 + 能力字段）：**

```rust
pub(super) async fn models(client, c) -> Result<Vec<ModelInfo>, ProviderError> {
    let v = volces_read(client, c, "/api/v1/model-manager/models").await?;
    let entries = data(&v)["models"].as_array()
        .ok_or_else(|| ProviderError::new(502, "AStudio 未返回模型目录"))?;
    let mut seen = HashSet::new();
    let mut result = vec![];
    for entry in entries {
        // slug 允许 "/"（zhipu/glm-5.3、moonshot/kimi-k2.8-preview）
        let Some(id) = text(entry, &["slug"]).filter(|id| id.len() <= 128
            && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b))) else { continue };
        if !seen.insert(id.clone()) { continue; }
        let name = text(entry, &["display_name"]).unwrap_or_else(|| id.clone());
        let mut m = model(id, name, entry.get("context_window").and_then(Value::as_u64));
        m.supports_images = entry.get("input_modalities").and_then(Value::as_array)
            .map(|mods| mods.iter().any(|x| x.as_str() == Some("image")));
        let efforts: Vec<String> = entry["supported_reasoning_levels"].as_array()…map(|l| l["effort"])…;
        m.supports_reasoning = Some(!efforts.is_empty());
        m.reasoning_effort_options = efforts;
        m.default_reasoning_effort = text(entry, &["default_reasoning_level"]);
        result.push(m);
    }
    Ok(result)
}
```

`supports_tools` 沿用 `model()` 默认 true：工具调用协议已实测确认（§2.7），但目录未声明 per-model 工具支持，保留默认值，集成阶段逐模型复验。

**inspect / chat：**

```rust
pub(super) async fn inspect(client, c) -> Result<ProviderSnapshot, ProviderError> {
    let (balance, rewards, models) = tokio::join!(balance(client, c), rewards(client, c), models(client, c));
    let mut result = balance?;
    result.rewards = rewards?;
    result.models = models?;
    Ok(result)
}

pub(super) async fn chat(client, c, model, body) -> Result<ProviderChat, ProviderError> {
    let converted = super::astudio_responses::request_body(&body, model)?;
    let request = client.http.post(format!("{VOLCES_ORIGIN}/model-gateway/v1/responses"))
        .bearer_auth(&c.access_token)
        .header("Accept", "text/event-stream")
        .json(&converted);
    super::stream::response(request, Platform::AStudio).await
}
```

#### 3.1.4 `credits/providers/astudio_responses.rs` — 请求转换（新建）

chat/completions → Responses API，参照 `minimax_chat.rs` 的校验风格（不支持的参数显式报 400 validation）。

**字段映射：**

| chat/completions | Responses | 说明 |
|---|---|---|
| `model` | `model` | slug；有 `upstream_id` 时用之 |
| `messages[system/developer]` | `instructions` | 文本拼接 |
| `messages[user]` | `input[]` `{type:"message", role:"user", content:[{type:"input_text",text}]}` | 图片 → `{type:"input_image", image_url}` |
| `messages[assistant]` | `input[]` `{type:"message", role:"assistant", content:[{type:"output_text",text}]}` | |
| `assistant.tool_calls[]` | `input[]` `{type:"function_call", call_id, name, arguments}` | 独立 item |
| `messages[tool]` | `input[]` `{type:"function_call_output", call_id, output}` | |
| `tools[]` | `tools[]` `{type:"function", name, description, parameters}` | 扁平化（少一层 function 嵌套） |
| `tool_choice` | `tool_choice` | auto/none/required/指定函数 |
| `max_tokens` / `max_completion_tokens` | `max_output_tokens` | |
| `reasoning_effort` | `reasoning.effort` | disabled/none→none；目录支持 none/high/max |
| `temperature` / `top_p` | 同名透传 | |
| `stream` | `stream: true` | 网关恒流式 |
| `presence_penalty` / `frequency_penalty` / `seed` / `response_format` / `logprobs` / `stop` | — | 显式 400 拒绝（Responses 不支持） |

#### 3.1.5 `credits/providers/stream.rs` — `parse_astudio` 分支

`State::parse` 已解析 `event:` 行并按平台分发（minimax 先例）。新增 `Platform::AStudio => self.parse_astudio(event, &v)`，State 增加 `astudio_tools` / `astudio_tool_count` / `astudio_usage` / `astudio_finish` 字段（与 minimax_* 同构）。

**事件映射（基于实测捕获）：**

| Responses 事件 | 输出 chat.completion.chunk | 备注 |
|---|---|---|
| `response.created` / `in_progress` | `delta:{role:"assistant"}` | **提取 `response.model` 作为 identity**；identity 确认前的内容帧 → 502（minimax 同款守卫） |
| `response.output_text.delta` | `delta:{content: delta}` | |
| `response.reasoning_summary_text.delta` | `delta:{reasoning_content: delta}` | ✅ 实测确认；帧含 `summary_index`；reasoning 的 item/part added/done 均忽略 |
| `response.output_item.added`（type=function_call） | `delta:{tool_calls:[{index, id:call_id, type:"function", function:{name, arguments:""}}]}` | ✅ 实测确认；item 含 call_id/name |
| `response.function_call_arguments.delta` | `delta:{tool_calls:[{index, function:{arguments:delta}}]}` | ✅ 实测确认 |
| `response.function_call_arguments.done` | 忽略（arguments 已由 delta 累积） | 含完整 arguments，可作校验 |
| `response.completed` | output 含 function_call → `finish_reason:"tool_calls"`，否则 `"stop"`；+ usage | 终止帧；**finish_reason 从 output items 推导** |
| `response.incomplete` | `finish_reason:"length"`（reason=max_output_tokens）+ usage | 其他 reason 按需映射 |
| `response.failed` | 流错误（502，携带上游错误信息） | |
| 其余（content_part.*、output_item.done 等） | 忽略 | |

**usage 转换**：`{input_tokens, output_tokens, total_tokens}` → `{prompt_tokens, completion_tokens, total_tokens}`。

**终止语义**：无 `[DONE]`；终止帧置 `finished=true, ended=true`。流 EOF 而无终止帧 → 现有 "平台流在完成前中断" 502 逻辑自动生效。

#### 3.1.6 `commands/credits.rs` — 本地导入分支

```rust
Platform::AStudio => crate::credits::providers::astudio::local_credentials_for_region(region)?
    .ok_or_else(|| "本机未找到 AStudio 登录信息，请先登录 AStudio 桌面客户端（便携版需设置 ASTUDIO_DATA_DIR）")?,
```

`local_credentials_for_region` 对 `CreditRegion::Intl` 显式返回 `Ok(None)`。

#### 3.1.7 `credits/mod.rs` — 导入预刷新

`add_account` 现有 MinimaxCode 特判（mod.rs:195）扩展为：

```rust
let credentials = if platform == Platform::AStudio {
    // 文件中的 JWT 可能已轮换失效，导入时始终重发
    self.provider.refresh_credentials(platform, &credentials).await.map_err(|e| e.message)?
} else if platform == Platform::MinimaxCode && credentials.expires_at.is_some_and(…) {
    …现有逻辑…
} else { credentials };
```

#### 3.1.8 `credits/oauth.rs` — 显式不支持分支（编译必需）

`start_login` 与 `poll_login` 的 Platform match 是穷举的，新增变体必须加分支：

```rust
Platform::AStudio => return Err(ProviderError::new(400, "AStudio 暂不支持网页登录，请使用本机导入")),
```

### 3.2 前端改动详细设计

#### 3.2.1 `src/lib/api/credits.ts`

```typescript
export type CreditPlatform = "workbuddy" | "traework" | "qoder" | "minimax-code" | "astudio";
```

#### 3.2.2 `src/components/credits/CreditsPanel.tsx`

1. `platforms` 数组新增 `{ id: "astudio", name: "AStudio" }`
2. 导入 UI 与 minimax-code 同款（一键导入本机凭据，ShieldCheck 图标）
3. **区域选择器**：`platform === "astudio"` 时仅渲染"国内版"（或禁用 Intl 按钮）；`importLocal` 恒传 `"cn"`
4. **模型目录分组**（现对每个平台 flatMap cn+intl）：astudio 跳过 intl 分组，避免空的"AStudio · 国际版"
5. 平台说明文案："支持积分、每日登录签到与模型代理，模型和奖励资格以当前账号返回为准。"
6. `automaticReward` / `noDailyReward` 均不加入 astudio（有每日签到）

#### 3.2.3 `src/components/ui/AgentIcon.tsx` + `src/assets/agents/`

新增 `astudio.svg`（官网 favicon 提取）+ 映射 + README 来源记录。

### 3.3 NOTICE.md 协议文档

新增 AStudio 章节：双令牌架构、Cookie 构造、两种信封、积分桶契约、弹窗 complete 语义、Responses API 契约与事件映射、实测证据索引（含 `docs/astudio-responses-sse-capture.txt`）、与 MiniMax localImport 模式的差异（无 expiresAt、JWT 需重发）。

---

## 四、关键实现细节

### 4.1 本地凭据导入流程

```
用户点击"一键导入"（区域恒 cn）
  → credits_import_local(astudio, cn)
  → astudio::local_credentials_for_region(cn)
     → ASTUDIO_DATA_DIR 或默认 userData 目录
     → 读取 userdata/astron-session.json，parse_session（JWT exp → expires_at）
  → CreditManager.add_account()
     → 预刷新：refresh() 重读文件 + POST auth/bearer-token 重发 JWT
     → authenticate()：membership/me 验证 SSO，取 uid + planName
  → 存入 SecretStore + CreditStore
  → refresh_account() 拉取余额/奖励/模型
```

容错：文件不存在 → 提示登录客户端（便携版提示 ASTUDIO_DATA_DIR）；格式无效 → 提示重新登录；SSO 失效（1002/1005）→ 401 提示客户端重新登录；账号切换 → 拒绝并提示重新导入。

### 4.2 签到领取流程

```
claim_account()
  → pending_daily_popup()：GET client-popups/pending，找 DAILY_REWARD_DIALOG
  → 无弹窗 → Ok(outcome "unavailable")（不报错）
  → 有弹窗 → POST client-popups/complete {popupId:number, instanceKey, completed:true}
     → 成功 → outcome "claimed" + amount
     → rejected 错误 → outcome "failed"
     → 模糊/超时 → 回查 pending 一次：弹窗消失 → "claimed"；仍在 → "pending"（不重发）
```

要点：popupId 必须 number（实测为静态值 2，兼容字符串）；`instanceKey` 是 YYYYMMDD 日期串，直接做 reward id；**payload 无 points**，数额按 planCode 从 plan-config 解析；complete 成功响应 `data:null`，走 `studio_write`；重复提交幂等，"已领取"判定依赖 pending；只 complete 每日积分弹窗，不触碰其他 componentType。

### 4.3 余额与积分包

- `totalBalance` 权威（服务端已做到期过滤），packs 仅描述各桶余额与到期
- 到期时间解析：`%Y-%m-%dT%H:%M:%S` + CST(+8)（现有 `epoch()` 不支持 T 分隔，适配器本地实现）
- 星火积分独立 unit `spark`，不并入通用余额
- 余额字段缺失 → 502（协议异常）；`snapshot()` 的 None 语义仅用于 packs 不完整场景

### 4.4 凭据刷新策略

1. `extra.localImport = true`；access_token = 模型 JWT；SSO 存 extra
2. refresh = 重读文件（取最新 SSO）+ 账号校验 + `auth/bearer-token` 重发 JWT
3. SSO 失效（401/403/1002/1005）→ 401 rejected，提示客户端重新登录
4. 模型接口 401（JWT 被轮换）→ 现有刷新调度触发 refresh 重发 JWT
5. 导入时始终预刷新（§3.1.7），不依赖 expires_at 判断（JWT 有效期 3 年，但可能提前失效）

### 4.5 模型代理

- 上游：`POST {volces}/model-gateway/v1/responses`，Bearer JWT
- 请求转换：`astudio_responses::request_body`（§3.1.4）
- 流转换：`parse_astudio`（§3.1.5），identity 取 `response.created` 的 model 回显（实测 = slug）
- 路由键：`astudio/<slug>`，含 `/` 的 slug 由 gateway 全字符串比较天然支持
- 诊断：`x-request-id` / `x-gateway-request-id` 接入现有 `upstream_request_id` 机制

### 4.6 错误映射汇总

| 场景 | 判定 | ProviderError |
|---|---|---|
| studio SSO 失效 | HTTP 401/403 或 code 1002/1005 | 401 rejected=true |
| studio 其他业务错误 | flag!=true 或 code!=0 | 502（check_business + flag 校验） |
| volces JWT 失效 | HTTP 401 | 401 rejected（http_error 现有逻辑） |
| 限流 | HTTP 429 + Retry-After | 429 rejected（现有逻辑） |
| 流中断/无终止帧 | EOF without response.completed/incomplete | 502 stage=stream |

---

## 五、测试策略

### 5.1 单元测试（fixtures 驱动，axum 内存服务器，沿用现有模式）

**Fixture 来源（实测捕获，脱敏后入库）：** balance、membership/me、plan-config、points/summary、model-manager/models、Responses SSE 全序列（`docs/astudio-responses-sse-capture.txt`）；每日弹窗真实 pending、complete 响应、重复提交、领取前后余额（`docs/astudio-daily-claim-capture.txt`）；思考事件全序列（`docs/astudio-responses-reasoning-capture.txt`）；工具调用全序列（`docs/astudio-responses-tools-capture.txt`）。

| 测试场景 | 验证点 |
|---|---|
| `parse_session` | 实测字段提取；JWT exp → expires_at；缺字段报错 |
| `local_credentials_for_region` | 默认路径 + ASTUDIO_DATA_DIR 覆盖；Intl → None；文件不存在 → None |
| Cookie / 请求头构造 | 四段 Cookie；clientType 21/22；studioVersion |
| studio 信封 | flag/code 校验；1002/1005 → 401 rejected；其他码 → 502 |
| `balance` | 四桶 packs + 到期时间（CST 解析）；totalBalance 权威；spark 独立 unit；缺字段 502 |
| `rewards` | DAILY_REWARD_DIALOG 识别；popupId 字符串兼容；amount 按 planCode 从 plan-config 解析；无弹窗 → 空；其他 componentType 忽略 |
| `claim` | 成功 → claimed（complete data:null 接受）；无弹窗 → unavailable outcome（非 Err）；rejected → failed；超时回查弹窗消失 → claimed、仍在 → pending 且不重发；重复提交幂等 |
| `refresh` | 重读文件 + 重发 JWT；账号切换拒绝；SSO 失效 401 |
| `models` | slug 含 `/` 合法；去重；能力字段（context_window、reasoning efforts、images） |
| 请求转换 | messages/tools/tool 结果映射；reasoning_effort 映射；不支持参数 400 |
| 流解析 | delta→content；reasoning delta→reasoning_content；function_call→tool_calls；completed→stop/tool_calls+usage；incomplete→length；failed→错误；identity 守卫；无 [DONE] 终止；EOF 无终止帧 → 502 |
| `chat` 请求构造 | URL、Bearer、转换后 body |

### 5.2 集成测试（`#[ignore]`）

- `astudio_local_read_only`：本机真实凭据，identity + balance + rewards + models（只读）
- `astudio_local_gateway_inference`：隔离内存 gateway 验证 Responses 转换链路（参考 minimax 测试模式）

### 5.3 前端测试

平台列表渲染、导入按钮、区域选择器仅 Cn、模型分组无 intl、签到状态标签。

---

## 六、风险与注意事项

### 6.1 剩余待验证项（Phase 0 已销账大部分）

| 项 | 当前结论 | 确认方式 | 阻塞性 |
|---|---|---|---|
| 每日弹窗生命周期 | ✅ 已确认：complete 后弹窗从 pending 消失；complete 触发发放（activityBalance +100 实测） | 2026-11-13 全链路实测 | — |
| complete 响应 / 重复提交 | ✅ 已确认：成功 `{flag:true,code:0,data:null}`；重复提交幂等（同样 200）；payload 无 points，数额按套餐解析 | 2026-11-13 全链路实测 | — |
| 开启思考的 reasoning 事件 | ✅ 已确认：`reasoning_summary_part.added` → `reasoning_summary_text.delta`（含 summary_index）→ done | 2026-11-13 effort=high 探针实测 | — |
| 工具调用事件流 | ✅ 已确认：output_item.added(function_call) → arguments.delta → done → completed；finish_reason 从 output 推导 tool_calls | 2026-11-13 带 tools 探针实测 | — |
| SSO 令牌实际有效期 | 未知（文件无记录） | 长期观察 | 不阻塞 |
| 标准安装 userData 路径 | `%APPDATA%/AStudio`（推断，本机为便携版） | 标准安装环境验证 | 不阻塞（env 覆盖兜底） |

### 6.2 安全考虑

1. 凭据（含 extra 中的 SSO）整体存 OS credential store，不落明文盘
2. 会话文件含明文手机号——错误消息/诊断/label 不得回显文件内容；沿用 `safe_diagnostic_id` 机制
3. 本地文件只读，不修改不写入
4. 账号切换检测（uid 比对）防凭据串号
5. 测试 fixture 入库前脱敏（uid、traceId、手机号、token）

### 6.3 兼容性

1. `credit_accounts.platform` 为 TEXT，新增枚举值无需迁移
2. `ModelInfo` 的 `#[serde(default)]` 保证旧快照可反序列化
3. `CreditPlatform` 新增值不影响现有类型安全

### 6.4 与现有代码的边界

1. 不改动现有 4 个平台适配器逻辑，只新增文件与 match 分支
2. gateway 路由/身份校验零改动（已核对 `model_matches`、`matching_upstream`）
3. oauth.rs 仅加显式拒绝分支（编译必需），不实现登录流程
4. `check_business` 不改通用逻辑，studio 信封差异在适配器内处理

---

## 七、实施步骤与里程碑

### 阶段 0：协议实测 ✅ 已完成（2026-11-12）

会话文件结构、双令牌与 JWT 重发、三个上游 base URL、Cookie/信封契约、积分四桶+到期、弹窗 complete 语义、模型目录、Responses API 与 SSE 全序列——均已确认。证据：本文 §2 各节 + `docs/astudio-responses-sse-capture.txt`。

### 阶段一：账号 + 积分 + 签到 + 目录（预计 2-3 天）

| 步骤 | 任务 | 验证 |
|---|---|---|
| 1 | types.rs / providers.rs / oauth.rs / mod.rs 接线 | `cargo check` 通过 |
| 2 | astudio.rs：导入、refresh、authenticate、balance、rewards、claim、models + 单元测试 | `cargo test credits::providers::astudio` |
| 3 | commands/credits.rs 导入分支 | 前端可触发导入 |
| 4 | 前端类型 + CreditsPanel（含区域约束）+ 图标 | `npm run build`；UI 手动验证 |
| 5 | 真实弹窗 fixture 捕获与 claim 测试定稿 ✅ 已完成（2026-11-13 全链路实测） | `docs/astudio-daily-claim-capture.txt` |

### 阶段二：推理代理（预计 2-3 天）

| 步骤 | 任务 | 验证 |
|---|---|---|
| 6 | astudio_responses.rs 请求转换 + 测试 | 单元测试 |
| 7 | stream.rs parse_astudio + 测试（用实测 SSE fixture） | 单元测试 |
| 8 | chat 接线 + 隔离 gateway 推理验证 | `#[ignore]` 集成测试 |
| 9 | 工具调用 / 思考档位协议验证 ✅ 已完成（2026-11-13 探针）；实现后做集成复验 | 探针捕获 + 集成测试 |
| 10 | NOTICE.md 协议文档 | 文档审查 |

### 阶段三：可选增强（按需）

网页 OAuth（bundle 已有 SMS/QR 登录接口：`login/sms`、`qr-code`/`qr-poll`）、`tools.ts` skills 同步、`credits_verify_model` 实测、模型目录离线缓存兜底。

---

## 八、改动文件清单汇总

### 新建文件

| 文件 | 行数预估（含测试） |
|---|---|
| `src-tauri/src/credits/providers/astudio.rs` | ~600-800 |
| `src-tauri/src/credits/providers/astudio_responses.rs` | ~350-500 |
| `src/assets/agents/astudio.svg` | 矢量 |

### 修改文件

| 文件 | 改动量 | 内容 |
|---|---|---|
| `credits/types.rs` | +3 | 枚举 + key() |
| `credits/providers.rs` | +10 | mod + 8 分支 |
| `credits/providers/stream.rs` | +150-220 | parse_astudio + State 字段 + 测试 |
| `credits/oauth.rs` | +6 | 两处显式拒绝分支 |
| `credits/mod.rs` | +4 | 导入预刷新特判 |
| `commands/credits.rs` | +3 | import_local 分支 |
| `src/lib/api/credits.ts` | +1 | 类型联合 |
| `src/components/credits/CreditsPanel.tsx` | +25 | platforms + 导入 UI + 区域约束 + 分组过滤 |
| `src/components/ui/AgentIcon.tsx` | +2 | 图标映射 |
| `src/assets/agents/README.md` | +2 | 来源 |
| `credits/providers/NOTICE.md` | +60 | 协议参考 |

**总计约 1300-1700 行新增 + 60 行修改**（原文档估算 900-1000 行，因 Responses 转换层上调）。

---

## 九、总结

AStudio 接入复用项目成熟的 Provider 模式，但 Phase 0 实测推翻了原文档的多个核心假设，最终方案有四个独特之处：

1. **双令牌 + JWT 重发**：SSO UUID（Cookie）与模型 JWT（Bearer）分离；refresh 不是简单重读文件，而是重读 + `auth/bearer-token` 重发
2. **签到走弹窗 complete**：`claim` 是另一种奖励；领取确认 = 弹窗从 pending 消失，需专用回查逻辑（通用 reconcile_claim 不适用）
3. **仅 Responses API**：必须新建请求转换模块 + stream 专用事件映射分支，这是工作量上调的主因
4. **多桶积分 + 到期时间**：balance 四桶与星火桶映射 CreditPack，复用刚上线的积分包到期展示

协议契约已全部实测锁定（含 2026-11-13 签到全链路与思考/工具探针），剩余仅 SSO 有效期、标准安装路径等观察项，均不阻塞开发。建议按阶段一（账号积分签到目录）→ 阶段二（推理代理）推进。