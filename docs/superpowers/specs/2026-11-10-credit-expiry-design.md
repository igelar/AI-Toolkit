# 积分包有效期查询设计

日期：2026-11-10
状态：待评审

## 目标

在「积分与代理」页的账户卡片上，增加一个「即将过期」区块，让用户能直接看到
自己各平台积分包（赠送额度包）的**使用到期时间**，避免积分过期才发现。

## 背景与现状

应用已实现多平台一键签到领取免费积分：

- 顶部「一键领取积分」批量领取所有启用账户（`src/components/credits/CreditsPanel.tsx:371`）
- 账户卡片内「领取积分」单账户领取（同文件 `:193`）

### 到期时间数据其实已经存在

`CreditPack.expires_at` 在后端类型（`src-tauri/src/credits/types.rs:71`）和前端类型
（`src/lib/api/credits.ts:21`）中都已定义，各平台适配器也一直在解析并随余额刷新入库：

| 平台 / 区域 | 到期字段来源 | 代码位置 |
| --- | --- | --- |
| WorkBuddy 国内 | `CycleEndTime` / `PackageEndTime` | `providers/workbuddy.rs:201` |
| WorkBuddy 国际 | `DeductionEndTime` / `ExpiredTime` / `CycleEndTime` | `providers/workbuddy.rs:315` |
| TRAE 国内 / 国际 | `entitlement_base_info.end_time` | `providers/traework.rs:556`、`:635` |
| Qoder | `expiresAt` | `providers/qoder.rs:365`、`:377` |

**但 UI 从不展示到期时间**，且积分包明细区块只在国际版账户下渲染
（`CreditsPanel.tsx:203` 的 `account.region === "intl"` 条件），国内版账户的
积分包数据完全被隐藏。

### MiniMax Code 是唯一例外

`providers/minimax.rs:282` 的 `balance_snapshot` 只解析会员汇总里的余额，
不产出任何 `CreditPack`，因此该平台没有任何积分包概念。

经本机只读探测（使用 `~/.minimax/auth/prod/cn/mcode-public/auth.json` 现有凭据，
只调用读取接口，未提交领取、未发起推理），确认官方客户端使用以下接口：

```
GET /minimax-cloud/api/v1/credit/details?page_number=1&page_size=20&include_expired=true
```

响应结构：

```json
{
  "details": [
    {
      "granted_at_ms": 1794273301938,
      "expire_at_ms": 1796832000000,
      "credit_type": 2,
      "granted_amount": "1000.00",
      "remaining_amount": "1000.00",
      "consumed_amount": "0.00"
    }
  ],
  "total_count": 17,
  "base_resp": { "status_code": 0, "status_msg": "ok" }
}
```

实测要点：

- `credit_type=2` 是每日签到赠送积分：`granted_at_ms` 与签到时刻吻合（北京时间上午），
  `expire_at_ms` 为发放后第 30 天北京时间 0 点（实测 30 天）
- `credit_type=1` 是购买积分：本次到期 2027-10-28 23:59:59，
  `remaining_amount`（2000）与会员汇总的 `purchased_remaining_amount`（2000）吻合
- `include_expired=false` 只返回未过期明细（实测 15 条），`total_count` 同步变化
- `page_size` 上限 100（实测 200 被上游拒绝，返回 code `1406011050`）

## 关键约束

**余额仍以会员汇总为准，明细只用于展示到期信息。**

实测明细剩余合计（9129.54）与会员汇总 `op_credit_summary.total_remaining_amount`
（9267.619）并不一致。因此 MiniMax Code 的余额继续读 `op_credit_summary`，
新接口的明细仅用于生成到期信息，不参与余额计算——沿用 WorkBuddy 国际版
已有的 "summary is authoritative" 做法（`providers/workbuddy.rs:330`）。

## 方案

### 展示位置

「即将过期」区块放在账户卡片内、**「可用模型」按钮的正上方**，与「积分领取 /
每日签到」区块的布局一致（`CreditsPanel.tsx:197-202` 的 `border-t` + `text-xs font-medium`
小标题 + 内容行）。

- **国内版账户**：显示「即将过期」区块
- **国际版账户**：保留原有「额度明细」区块（`CreditsPanel.tsx:203`），**不**显示
  「即将过期」区块

### 交互

点击「即将过期」区块的标题行，弹出浮层列表（复用 `ModelInfoPopover.tsx` 已导出的
`modelInfoPosition` 定位 helper 与相同的 pointer/Escape/滚轮关闭模式），
逐个积分包显示：积分包名称、剩余额度、到期时间、剩余天数。

临近到期（≤7 天）的行用琥珀色高亮；已过期的行标灰并注明「已过期」。

**最早到期只取剩余有额度的积分包。** 已用完（剩余为 0）的积分包没有可损失的额度，
不参与最早到期计算，也不出现在浮层列表中；剩余未知的积分包仍然保留，
与网关选择即将到期账户时的口径一致（`gateway.rs` 的
`remaining.is_none_or(|v| v > 0.0)`）。

### 数据来源

点击时**不发起任何网络请求**，全部读取已同步到本地的
`Account.snapshot.packs`。数据随每次余额刷新更新，刷新周期不变
（前台 5 分钟 / 后台 15 分钟）。

### 改动范围

**后端**（仅 MiniMax Code 适配器，WorkBuddy / TRAE / Qoder 零改动）：

- `providers/minimax.rs`：新增 `credit_details()` 读取 `credit/details`，
  解析成 `Vec<CreditPack>`，并入余额刷新路径
- MiniMax Code 的 `CreditPack` 名称按 `credit_type` 区分「购买积分」/「签到赠送积分」
- 余额仍取会员汇总，**不**用明细求和

**前端**：

- `src/components/credits/CreditExpiryPopover.tsx`（新增）：到期信息浮层与纯函数
  `expiryEntries` / `expirySummary` / `expiryCountdown`
- `src/components/credits/CreditsPanel.tsx`：账户卡片新增「即将过期」区块
- `src/lib/api/credits.ts`：类型无需改动（`CreditPack.expiresAt` 已存在）
- `src-tauri/src/credits/types.rs`：类型无需改动

**测试**：

- Rust：MiniMax Code 明细解析的单元测试（合成夹具，含 `credit_type`
  与到期时间映射、余额仍取汇总的断言）
- 前端：`scripts/credit-expiry.test.mjs`，断言到期排序、倒计时文案、
  国内版显示「即将过期」、国际版保留「额度明细」且不显示「即将过期」、
  浮层列出每个积分包

## 不做的事

- 不新增「实时查询」后端命令（用户选择用已同步快照）
- 不改动 WorkBuddy / TRAE / Qoder 的适配器（数据已具备）
- 不改动余额计算口径
- 不为 MiniMax Code 国际版做未经验证的接口适配（该区域接口行为未经实测，
  与本仓库既有做法一致：国际版保持实验性标记，未实测不开放）

## 验证

- `pnpm check`（TypeScript 类型检查 + oxlint + 既有前端测试）
- `cargo test --manifest-path src-tauri/Cargo.toml --locked`
- `pnpm build`
- 本机 MiniMax Code 只读冒烟：确认 `credit/details` 解析出的到期时间
  与官方客户端一致（不提交领取、不发起推理）

## 未验证 / 风险

- MiniMax Code 国际版的 `credit/details` 未实测，国际版账户可能只显示
  「暂无到期信息」
- 官方客户端日志显示 `credit/details` 的调用频率远低于余额查询
  （本次仅在 10:12:06 出现一次），并入余额刷新路径会增加该接口的调用量；
  若上游限流，余额刷新会失败——这是需要接受的权衡，因为到期信息没有
  独立的刷新通道
