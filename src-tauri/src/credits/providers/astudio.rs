//! AStudio (Astron Studio) provider adapter. Protocol contract and evidence: providers/NOTICE.md.
//! Dual-token architecture: SSO UUID (studio Cookie) + model JWT (volces Bearer).
use super::*;
use std::path::PathBuf;

const STUDIO_ORIGIN: &str = "https://agent.xfyun.cn/xingchen-studio/";
const VOLCES_ORIGIN: &str = "https://astronstudio-api-volces-prod.xf-yun.com";
const STUDIO_VERSION: &str = "3.6.2";

fn astudio_data_dir() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("ASTUDIO_DATA_DIR").filter(|p| !p.is_empty()) {
        let path = PathBuf::from(dir);
        return Ok(if path.is_absolute() {
            path
        } else {
            std::env::current_dir()
                .map_err(|_| "无法解析 AStudio 数据目录")?
                .join(path)
        });
    }
    let config = dirs::config_dir().ok_or("无法定位 AStudio 数据目录")?;
    Ok(config.join("AStudio"))
}

fn parse_session(value: &Value) -> Result<Credentials, String> {
    let sso = text(value, &["ssoSessionId", "token"]).ok_or("本机 AStudio 缺少会话凭据")?;
    let account_id = text(value, &["accountId"]).ok_or("本机 AStudio 缺少账号标识")?;
    let jwt = text(value, &["modelBearerToken"]).ok_or("本机 AStudio 缺少模型凭据")?;
    let uid = text(value, &["uid", "accountId"]).ok_or("本机 AStudio 缺少用户标识")?;
    let exp = jwt_claims(&jwt)["exp"].as_i64().map(|e| e * 1000);
    Ok(Credentials {
        region: CreditRegion::Cn,
        access_token: jwt,
        refresh_token: None,
        user_id: Some(uid),
        device_id: None,
        expires_at: exp,
        extra: json!({"localImport": true, "ssoSessionId": sso, "accountId": account_id}),
    })
}

fn parse_cst_datetime(v: &Value) -> Option<i64> {
    let s = v.as_str()?;
    let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok()?;
    let offset = chrono::FixedOffset::east_opt(8 * 3600)?;
    naive.and_local_timezone(offset).single().map(|d| d.timestamp_millis())
}

fn remap_session_error(e: ProviderError) -> ProviderError {
    if matches!(e.upstream_code.as_deref(), Some("1002" | "1005")) || matches!(e.status, 401 | 403) {
        ProviderError {
            status: 401,
            rejected: true,
            message: "AStudio 登录已失效，请在客户端重新登录".into(),
            ..e
        }
    } else {
        e
    }
}

async fn studio_read_at(
    client: &ProviderClient,
    c: &Credentials,
    origin: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, ProviderError> {
    let sso = c.extra["ssoSessionId"].as_str()
        .ok_or_else(|| ProviderError::new(401, "AStudio 凭据缺少会话标识"))?;
    let account = c.extra["accountId"].as_str()
        .ok_or_else(|| ProviderError::new(401, "AStudio 凭据缺少账号标识"))?;
    let cookie = format!("ssoSessionId={sso}; sso_sessionid={sso}; account_id={account}; token={sso}");
    let client_type = if cfg!(target_os = "macos") { "22" } else { "21" };
    let request = if let Some(body) = body {
        client.http.post(format!("{origin}{path}")).json(body)
    } else {
        client.http.get(format!("{origin}{path}"))
    };
    let request = request
        .header("Cookie", cookie)
        .header("clientType", client_type)
        .header("studioVersion", STUDIO_VERSION)
        .header("Accept", "application/json");
    match client.json(request).await {
        Ok(v) => {
            if v["flag"].as_bool() != Some(true) || v.get("data").is_none() {
                return Err(ProviderError::new(502, "AStudio 返回了无效的数据信封"));
            }
            Ok(v)
        }
        Err(e) => Err(remap_session_error(e)),
    }
}
async fn studio_read(
    client: &ProviderClient,
    c: &Credentials,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, ProviderError> {
    studio_read_at(client, c, STUDIO_ORIGIN, path, body).await
}

async fn volces_read_at(
    client: &ProviderClient,
    c: &Credentials,
    origin: &str,
    path: &str,
) -> Result<Value, ProviderError> {
    client.json(
        client.http.get(format!("{origin}{path}"))
            .bearer_auth(&c.access_token)
            .header("Accept", "application/json"),
    ).await
}

pub(super) async fn authenticate(
    client: &ProviderClient,
    c: &Credentials,
) -> Result<(String, String, Credentials), ProviderError> {
    authenticate_at(client, c, STUDIO_ORIGIN).await
}
async fn authenticate_at(
    client: &ProviderClient,
    c: &Credentials,
    origin: &str,
) -> Result<(String, String, Credentials), ProviderError> {
    let v = studio_read_at(client, c, origin, "membership/me", None).await?;
    let user = data(&v);
    let id = text(user, &["uid"]).ok_or_else(|| ProviderError::new(502, "AStudio 未返回账号 ID"))?;
    let label = text(user, &["planName"])
        .map(|p| format!("AStudio {p}"))
        .unwrap_or_else(|| "AStudio".into());
    let mut credentials = c.clone();
    credentials.user_id = Some(id.clone());
    Ok((id, label, credentials))
}

pub(super) async fn refresh(
    client: &ProviderClient,
    c: &Credentials,
) -> Result<Credentials, ProviderError> {
    let local = local_credentials_for_region(CreditRegion::Cn)
        .map_err(|m| ProviderError::new(401, m))?
        .ok_or_else(|| ProviderError::new(401, "未找到本机 AStudio 登录信息，请先在客户端登录"))?;
    if c.user_id.as_deref().is_some_and(|expected| expected != local.user_id.as_deref().unwrap_or("")) {
        return Err(ProviderError::new(401, "本机 AStudio 已切换账号，请重新导入"));
    }
    let v = studio_read(client, &local, "auth/bearer-token", Some(&json!({}))).await?;
    let jwt = text(data(&v), &["modelBearerToken"])
        .ok_or_else(|| ProviderError::new(502, "AStudio 未返回模型凭据"))?;
    let mut refreshed = local;
    refreshed.access_token = jwt;
    refreshed.expires_at = jwt_claims(&refreshed.access_token)["exp"].as_i64().map(|e| e * 1000);
    Ok(refreshed)
}

fn parse_balance(value: &Value) -> Result<(f64, Vec<CreditPack>), ProviderError> {
    let d = data(value);
    let total = num(d, &["totalBalance"])
        .filter(|n| n.is_finite() && *n >= 0.0)
        .ok_or_else(|| ProviderError::new(502, "AStudio 未返回可识别的积分余额"))?;
    let pack = |id: &str, name: &str, balance_key: &str, expire_key: &str, unit: &str| CreditPack {
        id: id.into(),
        name: name.into(),
        unit: unit.into(),
        remaining: num(d, &[balance_key]).map(|n| n.max(0.0)),
        expires_at: d.get(expire_key).and_then(parse_cst_datetime),
    };
    Ok((
        total,
        vec![
            pack("member", "套餐积分", "memberBalance", "memberNextExpireTime", "credits"),
            pack("buy", "增购积分", "buyBalance", "buyNextExpireTime", "credits"),
            pack("activity", "活动积分", "activityBalance", "activityNextExpireTime", "credits"),
            pack("spark", "星火积分", "sparkTotalBalance", "", "spark"),
        ],
    ))
}
async fn balance_at(
    client: &ProviderClient,
    c: &Credentials,
    origin: &str,
) -> Result<ProviderSnapshot, ProviderError> {
    let v = studio_read_at(client, c, origin, "points/balance", None).await?;
    let (total, packs) = parse_balance(&v)?;
    let mut result = snapshot(packs, vec![], vec![], None);
    // totalBalance is authoritative (server-side expiry filtering); packs describe buckets.
    result.balance = Some(total);
    result.unit = "credits".into();
    Ok(result)
}
pub(super) async fn balance(
    client: &ProviderClient,
    c: &Credentials,
) -> Result<ProviderSnapshot, ProviderError> {
    balance_at(client, c, STUDIO_ORIGIN).await
}

struct DailyPopup {
    popup_id: u64,
    instance_key: String,
}
fn parse_daily_popup(value: &Value) -> Result<Option<DailyPopup>, ProviderError> {
    let Some(popup) = data(value).as_array().into_iter().flatten()
        .find(|p| p["componentType"].as_str() == Some("DAILY_REWARD_DIALOG")) else {
        return Ok(None)
    };
    let popup_id = num(popup, &["popupId"])
        .filter(|n| *n > 0.0 && n.fract() == 0.0)
        .ok_or_else(|| ProviderError::new(502, "AStudio 弹窗 popupId 无效"))? as u64;
    let instance_key = text(popup, &["instanceKey"])
        .ok_or_else(|| ProviderError::new(502, "AStudio 弹窗缺少 instanceKey"))?;
    Ok(Some(DailyPopup { popup_id, instance_key }))
}
// The daily popup payload is a pure notification string with no points field
// (measured). The amount is resolved from membership/me planCode →
// membership/plan-config plans[].dailyLoginPoints.
async fn daily_points_at(
    client: &ProviderClient,
    c: &Credentials,
    origin: &str,
) -> Option<f64> {
    let (me, cfg) = tokio::join!(
        studio_read_at(client, c, origin, "membership/me", None),
        studio_read_at(client, c, origin, "membership/plan-config", None),
    );
    let plan_code = text(data(&me.ok()?), &["planCode"])?;
    data(&cfg.ok()?)["plans"].as_array()?
        .iter()
        .find(|p| p["planCode"].as_str() == Some(&plan_code))
        .and_then(|p| num(p, &["dailyLoginPoints"]))
}
fn daily_reward(popup: &DailyPopup, amount: Option<f64>) -> Reward {
    Reward {
        // instanceKey is the YYYYMMDD date string from the server; the stable daily id.
        id: format!("daily-login-{}", popup.instance_key),
        name: "每日登录积分".into(),
        kind: "credits".into(),
        status: "claimable".into(),
        amount,
        next_available_at: None,
        action_url: None,
    }
}
async fn rewards_at(
    client: &ProviderClient,
    c: &Credentials,
    origin: &str,
) -> Result<Vec<Reward>, ProviderError> {
    let v = studio_read_at(client, c, origin, "client-popups/pending", None).await?;
    let Some(popup) = parse_daily_popup(&v)? else { return Ok(vec![]) };
    let amount = daily_points_at(client, c, origin).await;
    Ok(vec![daily_reward(&popup, amount)])
}
pub(super) async fn rewards(
    client: &ProviderClient,
    c: &Credentials,
) -> Result<Vec<Reward>, ProviderError> {
    rewards_at(client, c, STUDIO_ORIGIN).await
}

async fn claim_at(
    client: &ProviderClient,
    account: &Account,
    c: &Credentials,
    origin: &str,
) -> Result<Vec<RewardOutcome>, ProviderError> {
    let Some(popup) = parse_daily_popup(&studio_read_at(client, c, origin, "client-popups/pending", None).await?)? else {
        let placeholder = Reward {
            id: format!("daily-login-{}", chrono::Utc::now().format("%Y%m%d")),
            name: "每日登录积分".into(),
            kind: "credits".into(),
            status: "unavailable".into(),
            amount: None,
            next_available_at: None,
            action_url: None,
        };
        return Ok(vec![outcome(
            account,
            &placeholder,
            "unavailable",
            "当前没有可领取的签到积分（可能已领取或尚未发放）",
            None,
        )]);
    };
    let amount = daily_points_at(client, c, origin).await;
    let reward = daily_reward(&popup, amount);
    let submitted = studio_read_at(
        client,
        c,
        origin,
        "client-popups/complete",
        Some(&json!({"popupId": popup.popup_id, "instanceKey": popup.instance_key, "completed": true})),
    ).await;
    match submitted {
        Ok(_) => Ok(vec![outcome(account, &reward, "claimed", "每日登录积分已领取", amount)]),
        Err(e) if e.rejected => Ok(vec![outcome(account, &reward, "failed", e.message, None)]),
        Err(_) => {
            // Ambiguous/timeout: read back pending once; popup gone = claimed, never resubmit.
            // A read-back failure is treated as unconfirmed → pending (not an error).
            let confirmed = match studio_read_at(client, c, origin, "client-popups/pending", None).await {
                Ok(v) => parse_daily_popup(&v)?.is_none(),
                Err(_) => false,
            };
            Ok(vec![outcome(
                account,
                &reward,
                if confirmed { "claimed" } else { "pending" },
                if confirmed { "回查确认弹窗已完成，积分已领取" } else { "领取结果尚未确认，已回查状态；不会自动重发领取请求" },
                if confirmed { amount } else { None },
            )])
        }
    }
}
pub(super) async fn claim(
    client: &ProviderClient,
    account: &Account,
    c: &Credentials,
) -> Result<Vec<RewardOutcome>, ProviderError> {
    claim_at(client, account, c, STUDIO_ORIGIN).await
}

fn parse_models(value: &Value) -> Result<Vec<ModelInfo>, ProviderError> {
    let entries = data(value)["models"].as_array()
        .ok_or_else(|| ProviderError::new(502, "AStudio 未返回模型目录"))?;
    let mut seen = std::collections::HashSet::new();
    let mut result = vec![];
    for entry in entries {
        // Slugs may contain "/" (zhipu/glm-5.3, moonshot/kimi-k2.8-preview).
        let Some(id) = text(entry, &["slug"]).filter(|id| id.len() <= 128
            && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b))) else { continue };
        if !seen.insert(id.clone()) { continue; }
        let name = text(entry, &["display_name"]).unwrap_or_else(|| id.clone());
        let mut m = model(id, name, entry.get("context_window").and_then(Value::as_u64));
        m.supports_images = entry.get("input_modalities").and_then(Value::as_array)
            .map(|mods| mods.iter().any(|x| x.as_str() == Some("image")));
        let efforts: Vec<String> = entry["supported_reasoning_levels"].as_array()
            .map(|levels| levels.iter().filter_map(|l| l["effort"].as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        m.supports_reasoning = Some(!efforts.is_empty());
        m.reasoning_effort_options = efforts;
        m.default_reasoning_effort = text(entry, &["default_reasoning_level"]);
        result.push(m);
    }
    Ok(result)
}
async fn models_at(
    client: &ProviderClient,
    c: &Credentials,
    origin: &str,
) -> Result<Vec<ModelInfo>, ProviderError> {
    let v = volces_read_at(client, c, origin, "/api/v1/model-manager/models").await?;
    parse_models(&v)
}
pub(super) async fn models(
    client: &ProviderClient,
    c: &Credentials,
) -> Result<Vec<ModelInfo>, ProviderError> {
    models_at(client, c, VOLCES_ORIGIN).await
}

pub(super) async fn inspect(
    client: &ProviderClient,
    c: &Credentials,
) -> Result<ProviderSnapshot, ProviderError> {
    let (balance, rewards, models) = tokio::join!(balance(client, c), rewards(client, c), models(client, c));
    let mut result = balance?;
    result.rewards = rewards?;
    result.models = models?;
    Ok(result)
}

pub(super) async fn chat(
    client: &ProviderClient,
    c: &Credentials,
    model: &ModelInfo,
    body: Value,
) -> Result<ProviderChat, ProviderError> {
    let converted = super::astudio_responses::request_body(&body, model)?;
    let request = client.http.post(format!("{VOLCES_ORIGIN}/model-gateway/v1/responses"))
        .bearer_auth(&c.access_token)
        .header("Accept", "text/event-stream")
        .json(&converted);
    super::stream::response(request, Platform::AStudio).await
}

pub(crate) fn local_credentials_for_region(
    region: CreditRegion,
) -> Result<Option<Credentials>, String> {
    if region == CreditRegion::Intl {
        return Ok(None);
    }
    let data_dir = astudio_data_dir()?;
    let path = data_dir.join("userdata").join("astron-session.json");
    let raw = match std::fs::read(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("无法读取本机 AStudio 登录信息".into()),
    };
    let value: Value = serde_json::from_slice(&raw).map_err(|_| "本机 AStudio 登录信息文件无效")?;
    parse_session(&value).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn credentials() -> Credentials {
        Credentials {
            region: CreditRegion::Cn,
            access_token: "synthetic-jwt".into(),
            refresh_token: None,
            user_id: Some("synthetic-uid".into()),
            device_id: None,
            expires_at: None,
            extra: json!({"ssoSessionId":"synthetic-sso","accountId":"synthetic-account"}),
        }
    }
    fn account() -> Account {
        Account {
            id: "fixture".into(),
            platform: Platform::AStudio,
            region: CreditRegion::Cn,
            user_id: "synthetic-uid".into(),
            label: "AStudio".into(),
            enabled: true,
            status: "ready".into(),
            last_error: None,
            snapshot: None,
        }
    }

    #[test]
    fn parse_session_extracts_dual_tokens_and_jwt_expiry() {
        // Build a minimal JWT with an exp claim.
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"alg":"HS256","typ":"JWT"}"#);
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"uid":"22093331744","iat":1700000000,"exp":1800000000}"#);
        let jwt = format!("{header}.{claims}.signature");
        let value = json!({"accountId":"22093331744","token":"uuid-token","ssoSessionId":"uuid-token",
            "modelBearerToken":jwt,"uid":"22093331744","loginMethod":"sms"});
        let c = parse_session(&value).unwrap();
        assert_eq!(c.access_token, jwt);
        assert_eq!(c.user_id.as_deref(), Some("22093331744"));
        assert_eq!(c.extra["ssoSessionId"], "uuid-token");
        assert_eq!(c.extra["accountId"], "22093331744");
        assert_eq!(c.extra["localImport"], true);
        assert_eq!(c.expires_at, Some(1_800_000_000_000i64));
        for missing in [json!({}), json!({"accountId":"x"}), json!({"accountId":"x","token":"y"})] {
            assert!(parse_session(&missing).is_err());
        }
    }

    #[test]
    fn parse_cst_datetime_converts_local_time_to_epoch_millis() {
        assert_eq!(parse_cst_datetime(&json!("2026-12-12T19:24:37")),
            Some(chrono::DateTime::parse_from_rfc3339("2026-12-12T11:24:37+00:00").unwrap().timestamp_millis()));
        assert_eq!(parse_cst_datetime(&json!("not-a-date")), None);
        assert_eq!(parse_cst_datetime(&Value::Null), None);
        assert_eq!(parse_cst_datetime(&json!(123)), None);
    }

    #[test]
    fn parse_balance_maps_four_buckets_and_spark_unit() {
        let value = json!({"flag":true,"code":0,"data":{
            "totalAmount":1200,"totalBalance":1200,
            "memberBalance":0,"buyBalance":0,
            "activityBalance":1200,"activityNextExpireTime":"2026-12-12T19:24:37",
            "sparkTotalBalance":5000
        }});
        let (total, packs) = parse_balance(&value).unwrap();
        assert_eq!(total, 1200.0);
        assert_eq!(packs.len(), 4);
        assert_eq!(packs[0].name, "套餐积分");
        assert_eq!(packs[2].expires_at, parse_cst_datetime(&json!("2026-12-12T19:24:37")));
        assert_eq!(packs[3].unit, "spark");
        assert_eq!(packs[3].expires_at, None);
        assert_eq!(packs[3].remaining, Some(5000.0));
        assert!(parse_balance(&json!({"flag":true,"code":0,"data":{}})).is_err());
    }

    #[test]
    fn parse_models_accepts_slash_slugs_and_reasoning_efforts() {
        let value = json!({"code":0,"data":{"models":[
            {"slug":"spark-x2.5","display_name":"Spark X2.5","context_window":1048576,
             "input_modalities":["text"],"supported_reasoning_levels":[{"effort":"none"},{"effort":"high"}],
             "default_reasoning_level":"high"},
            {"slug":"zhipu/glm-5.3","display_name":"GLM-5.3","context_window":1000000,
             "input_modalities":["text","image"]},
            {"slug":"moonshot/kimi-k2.8-preview","display_name":"Kimi-K2.8"},
            {"slug":"bad slug!","display_name":"Bad"}
        ]}});
        let items = parse_models(&value).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].id, "spark-x2.5");
        assert_eq!(items[0].supports_images, Some(false));
        assert_eq!(items[0].reasoning_effort_options, vec!["none","high"]);
        assert_eq!(items[0].default_reasoning_effort.as_deref(), Some("high"));
        assert_eq!(items[1].id, "zhipu/glm-5.3");
        assert_eq!(items[1].supports_images, Some(true));
        assert!(items[1].supports_reasoning == Some(false));
        assert_eq!(items[2].id, "moonshot/kimi-k2.8-preview");
        assert!(parse_models(&json!({"code":0,"data":{}})).is_err());
    }

    #[test]
    fn parse_daily_popup_finds_reward_dialog_and_ignores_others() {
        // Fixture mirrors the measured capture: payload is a pure notification
        // string with no points field.
        let value = json!({"flag":true,"code":0,"data":[
            {"popupId":12345,"componentType":"CLIENT_DOWNLOAD_REWARD_DIALOG","instanceKey":"a",
             "payload":{"confirmText":"知道了","title":"下载奖励","content":"请下载客户端。"}},
            {"popupId":"67890","componentType":"DAILY_REWARD_DIALOG","instanceKey":"20261113",
             "payload":{"confirmText":"知道了","title":"今日登录积分已到账","content":"每日登录积分已发放，可前往积分中心查看。"}}
        ]});
        let popup = parse_daily_popup(&value).unwrap().unwrap();
        assert_eq!(popup.popup_id, 67890);
        assert_eq!(popup.instance_key, "20261113");
        assert!(parse_daily_popup(&json!({"flag":true,"code":0,"data":[]})).unwrap().is_none());
        let missing = json!({"flag":true,"code":0,"data":[{"popupId":1,"componentType":"DAILY_REWARD_DIALOG","payload":{"confirmText":"x"}}]});
        assert!(parse_daily_popup(&missing).is_err());
    }

    #[tokio::test]
    async fn daily_points_at_resolves_amount_from_plan_config() {
        use axum::{routing::get, Json, Router};
        async fn me() -> Json<Value> {
            Json(json!({"flag":true,"code":0,"data":{"uid":1,"planCode":"TRIAL","planName":"体验版"}}))
        }
        async fn plan_config() -> Json<Value> {
            Json(json!({"flag":true,"code":0,"data":{
                "dailyLoginEnabled":true,"dailyLoginValidDays":31,
                "plans":[{"planCode":"TRIAL","dailyLoginPoints":100},{"planCode":"BASIC","dailyLoginPoints":200}]
            }}))
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new()
                .route("/membership/me", get(me))
                .route("/membership/plan-config", get(plan_config))).await.unwrap();
        });
        let mut client = ProviderClient::new();
        client.http = Client::builder().no_proxy().build().unwrap();
        assert_eq!(daily_points_at(&client, &credentials(), &origin).await, Some(100.0));
        server.abort();
    }

    #[test]
    fn remap_session_error_promotes_sso_invalid_codes() {
        let mut e = ProviderError::new(502, "平台业务请求被拒绝（代码 1002）");
        e.upstream_code = Some("1002".into());
        let mapped = remap_session_error(e);
        assert_eq!(mapped.status, 401);
        assert!(mapped.rejected);
        assert!(mapped.message.contains("登录已失效"));
        let http = remap_session_error(http_error(403));
        assert_eq!(http.status, 401);
        assert!(http.rejected);
        let other = remap_session_error(ProviderError::new(502, "other"));
        assert_eq!(other.status, 502);
        assert!(!other.rejected);
    }

    #[tokio::test]
    async fn studio_read_validates_flag_and_envelope() {
        use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let flag = Arc::new(AtomicBool::new(false));
        async fn ok(State(flag): State<Arc<AtomicBool>>) -> Json<Value> {
            if flag.load(Ordering::SeqCst) {
                Json(json!({"flag":false,"code":0,"data":{}}))
            } else {
                Json(json!({"flag":true,"code":0,"data":{"uid":1}}))
            }
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let router = Router::new().route("/membership/me", get(ok)).with_state(flag.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
        let mut client = ProviderClient::new();
        client.http = Client::builder().no_proxy().build().unwrap();
        assert!(studio_read_at(&client, &credentials(), &origin, "membership/me", None).await.is_ok());
        flag.store(true, Ordering::SeqCst);
        let err = studio_read_at(&client, &credentials(), &origin, "membership/me", None).await.unwrap_err();
        assert_eq!(err.status, 502);
        server.abort();
    }

    #[tokio::test]
    async fn balance_at_uses_total_balance_as_authoritative() {
        use axum::{routing::get, Json, Router};
        async fn handler() -> Json<Value> {
            Json(json!({"flag":true,"code":0,"data":{
                "totalBalance":1200,"memberBalance":0,"buyBalance":0,
                "activityBalance":1200,"activityNextExpireTime":"2026-12-12T19:24:37",
                "sparkTotalBalance":5000
            }}))
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/points/balance", get(handler))).await.unwrap();
        });
        let mut client = ProviderClient::new();
        client.http = Client::builder().no_proxy().build().unwrap();
        let snap = balance_at(&client, &credentials(), &origin).await.unwrap();
        assert_eq!(snap.balance, Some(1200.0));
        assert_eq!(snap.unit, "credits");
        assert_eq!(snap.packs.len(), 4);
        server.abort();
    }

    #[tokio::test]
    async fn claim_at_handles_claimable_unavailable_and_ambiguous() {
        use axum::{extract::State, http::HeaderMap, http::StatusCode, routing::{get, post}, Json, Router};
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Fixture { mode: u8, claims: AtomicUsize }
        async fn pending(State(f): State<Arc<Fixture>>) -> Json<Value> {
            let popup = f.mode != 3;
            let disappeared = f.mode == 2 && f.claims.load(Ordering::SeqCst) > 0;
            let data = if popup && !disappeared {
                json!([{"popupId":1,"componentType":"DAILY_REWARD_DIALOG","instanceKey":"20261113"}])
            } else { json!([]) };
            Json(json!({"flag":true,"code":0,"data":data}))
        }
        async fn complete(State(f): State<Arc<Fixture>>, _headers: HeaderMap, Json(_body): Json<Value>) -> (StatusCode, Json<Value>) {
            f.claims.fetch_add(1, Ordering::SeqCst);
            match f.mode {
                0 => (StatusCode::OK, Json(json!({"flag":true,"code":0,"data":null}))),
                1 | 2 => (StatusCode::BAD_GATEWAY, Json(json!({}))),
                _ => (StatusCode::OK, Json(json!({"flag":true,"code":0,"data":null}))),
            }
        }
        async fn membership_me() -> Json<Value> {
            Json(json!({"flag":true,"code":0,"data":{"uid":1,"planCode":"TRIAL"}}))
        }
        async fn plan_config() -> Json<Value> {
            Json(json!({"flag":true,"code":0,"data":{"plans":[{"planCode":"TRIAL","dailyLoginPoints":100}]}}))
        }
        for (mode, expected, amount) in [
            (0, "claimed", Some(100.0)),
            (1, "pending", None),
            (2, "claimed", Some(100.0)),
            (3, "unavailable", None),
        ] {
            let fixture = Arc::new(Fixture { mode, claims: AtomicUsize::new(0) });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}/", listener.local_addr().unwrap());
            let router = Router::new()
                .route("/client-popups/pending", get(pending))
                .route("/client-popups/complete", post(complete))
                .route("/membership/me", get(membership_me))
                .route("/membership/plan-config", get(plan_config))
                .with_state(fixture.clone());
            let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
            let mut client = ProviderClient::new();
            client.http = Client::builder().no_proxy().build().unwrap();
            let result = claim_at(&client, &account(), &credentials(), &origin).await.unwrap();
            assert_eq!(result.len(), 1);
            assert_eq!(result[0].status, expected, "mode {mode}");
            assert_eq!(result[0].amount, amount, "mode {mode}");
            if mode == 3 {
                assert_eq!(fixture.claims.load(Ordering::SeqCst), 0);
            }
            server.abort();
        }
    }

    #[tokio::test]
    async fn models_at_reads_volces_catalog() {
        use axum::{routing::get, Json, Router};
        async fn handler() -> Json<Value> {
            Json(json!({"code":0,"data":{"models":[{"slug":"spark-x2.5","display_name":"Spark X2.5","context_window":1048576}]}}))
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/api/v1/model-manager/models", get(handler))).await.unwrap();
        });
        let mut client = ProviderClient::new();
        client.http = Client::builder().no_proxy().build().unwrap();
        let models = models_at(&client, &credentials(), &origin).await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "spark-x2.5");
        server.abort();
    }

    #[test]
    fn local_import_only_supports_cn_and_reads_session_file() {
        let dir = std::env::temp_dir().join(format!("astudio-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("userdata")).unwrap();
        let jwt = "header.claims.sig";
        std::fs::write(dir.join("userdata/astron-session.json"), serde_json::to_string(&json!({
            "accountId":"acc","token":"tok","ssoSessionId":"tok",
            "modelBearerToken":jwt,"uid":"uid"
        })).unwrap()).unwrap();
        std::env::set_var("ASTUDIO_DATA_DIR", &dir);
        assert!(local_credentials_for_region(CreditRegion::Intl).unwrap().is_none());
        let c = local_credentials_for_region(CreditRegion::Cn).unwrap().unwrap();
        assert_eq!(c.access_token, jwt);
        assert_eq!(c.extra["ssoSessionId"], "tok");
        // Clean up
        let _ = std::fs::remove_dir_all(&dir);
        std::env::remove_var("ASTUDIO_DATA_DIR");
    }

    #[tokio::test]
    #[ignore = "explicit read-only diagnostic using the local official client session"]
    async fn astudio_local_read_only() {
        assert_eq!(std::env::var("AI_TOOLKIT_ASTUDIO_DIAGNOSTIC").as_deref(), Ok("read-only"));
        let c = local_credentials_for_region(CreditRegion::Cn).unwrap().unwrap();
        let client = ProviderClient::new();
        let (_, _, c) = authenticate(&client, &c).await.unwrap();
        let result = inspect(&client, &c).await.unwrap();
        assert!(result.balance.is_some());
        assert!(!result.models.is_empty());
        println!("AStudio identity, credits, daily signin and live model catalog verified; no claim or inference submitted.");
    }

    #[tokio::test]
    #[ignore = "explicit short inference diagnostic using an isolated in-memory gateway"]
    async fn astudio_local_gateway_inference() {
        use super::super::super::CreditManager;
        assert_eq!(std::env::var("AI_TOOLKIT_PROXY_DIAGNOSTIC").as_deref(), Ok("real-inference"));
        let c = local_credentials_for_region(CreditRegion::Cn).unwrap().unwrap();
        let client = ProviderClient::new();
        let (_, _, c) = authenticate(&client, &c).await.unwrap();
        let snapshot = inspect(&client, &c).await.unwrap();
        let model_id = snapshot.models[0].id.clone();
        let mut account = account(); account.user_id = c.user_id.clone().unwrap(); account.snapshot = Some(snapshot);
        let manager = CreditManager::test();
        manager.store.save_account(&account).unwrap();
        manager.save_credentials(&account.id, &c).unwrap();
        let gateway = manager.gateway.start(manager.clone(), 0).await.unwrap();
        let http = Client::builder().no_proxy().timeout(Duration::from_secs(90)).build().unwrap();
        let key = manager.gateway_key().unwrap();
        let catalog: Value = http.get(format!("{}/models", gateway.base_url)).bearer_auth(&key).send().await.unwrap().json().await.unwrap();
        assert!(catalog["data"].as_array().unwrap().iter().any(|model| model["id"].as_str().is_some_and(|id| id.starts_with("astudio/"))));
        let response = http.post(format!("{}/chat/completions", gateway.base_url)).bearer_auth(&key)
            .json(&json!({"model":format!("astudio/{model_id}"),"max_tokens":16,"stream":true,
                "messages":[{"role":"user","content":"Reply OK."}]}))
            .send().await.unwrap();
        assert!(response.status().is_success());
        let text = response.text().await.unwrap();
        assert!(text.contains("response.completed"), "missing stream completion");
        manager.gateway.stop().await.unwrap();
        println!("AStudio live gateway passed: catalog and Responses SSE streaming verified. No executable tool or credit claim was run; gateway keys/history stayed in memory.");
    }
}
