//! O4-M3 dispatcher 出站投递：`webhook`（真发 HTTP）+ `startBusinessProcess`（调 cmx-flowengine v1 起实例）。
//!
//! 与 flow/rules 一致用 reqwest（rustls）。目标地址来自配置/env（**不硬编码**）：
//!   - `ONTO_FLOW_URL`       流程引擎基址（默认 `http://127.0.0.1:8091`）；起实例 POST `{base}/api/flow/v1/instances`
//!   - `ONTO_WEBHOOK_ALLOW`  webhook 目标 host 白名单（逗号；默认 `127.0.0.1,localhost`）——SSRF 护栏
//!   - `ONTO_OUTBOUND=off`   全局熄火（两类外部投递回 deferred，便于离线/灰度）
//!
//! 跨微服务只经 HTTP（onto **不** path-dep flowengine），契合 headless「一芯多壳」。

use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

const DEFAULT_FLOW_BASE: &str = "http://127.0.0.1:8091";
const FLOW_INSTANCES_PATH: &str = "/api/flow/v1/instances";

/// 进程级共享 HTTP 客户端（连接复用 + 统一超时，避免慢下游拖垮 dispatcher）。
fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .user_agent("cmx-ontology-dispatcher")
            .build()
            .unwrap_or_default()
    })
}

/// 读配置项：先 env（大写全名），再 ConfigManager（`onto.<key>`），否则 default。
fn cfg(env_key: &str, cm_key: &str, default: &str) -> String {
    if let Ok(v) = std::env::var(env_key) {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return v;
        }
    }
    if let Some(cm) = cmx_utils::ConfigManager::try_global() {
        if let Ok(v) = cm.get_string(cm_key) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return v;
            }
        }
    }
    default.to_string()
}

/// 全局出站开关（`ONTO_OUTBOUND=off` → 关；此时 webhook/startBusinessProcess 回 deferred 挂起）。
pub fn outbound_enabled() -> bool {
    !std::env::var("ONTO_OUTBOUND")
        .map(|v| v.trim().eq_ignore_ascii_case("off"))
        .unwrap_or(false)
}

/// 流程引擎基址（去尾斜杠）。
pub fn flow_base() -> String {
    cfg("ONTO_FLOW_URL", "onto.flow_url", DEFAULT_FLOW_BASE)
        .trim_end_matches('/')
        .to_string()
}

/// webhook host 白名单（小写）。
pub fn webhook_allow() -> Vec<String> {
    cfg("ONTO_WEBHOOK_ALLOW", "onto.webhook_allow", "127.0.0.1,localhost")
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 出站配置快照（诊断端点 `GET /action-outbox/config` 用；**不含任何密钥**）。
pub fn config_snapshot() -> Value {
    json!({
        "outboundEnabled": outbound_enabled(),
        "flowUrl": flow_base(),
        "flowInstancesPath": FLOW_INSTANCES_PATH,
        "webhookAllow": webhook_allow(),
    })
}

/// host 是否在白名单（`*` 放行一切；否则 host 精确匹配，忽略大小写）。
fn host_allowed(url: &str, allow: &[String]) -> bool {
    if allow.iter().any(|a| a == "*") {
        return true;
    }
    match reqwest::Url::parse(url) {
        Ok(u) => u
            .host_str()
            .map(|h| allow.iter().any(|a| a == &h.to_ascii_lowercase()))
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// 投递 webhook：POST payload JSON 到 target（受白名单约束）。2xx→Ok；否则 Err。
pub async fn post_webhook(url: &str, payload: &Value) -> Result<Value, String> {
    let allow = webhook_allow();
    if !host_allowed(url, &allow) {
        return Err(format!(
            "webhook 目标 {url} 不在白名单 {allow:?}（SSRF 护栏；配 ONTO_WEBHOOK_ALLOW 放行）"
        ));
    }
    let resp = client()
        .post(url)
        .header("X-Onto-Source", "cmx-ontology")
        .json(payload)
        .send()
        .await
        .map_err(|e| format!("webhook 请求失败: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if status.is_success() {
        Ok(serde_json::from_str::<Value>(&body).unwrap_or_else(|_| json!({ "raw": body })))
    } else {
        Err(format!(
            "webhook 非 2xx（{status}）: {}",
            body.chars().take(200).collect::<String>()
        ))
    }
}

/// 触发流程：POST `{flow_base}/api/flow/v1/instances` `{definitionKey, variables}`（X-Tenant 隔离）。
/// 返回实例 id（信封 `{code,data}` 与裸对象都兼容）。
pub async fn start_business_process(tenant: &str, def_key: &str, payload: &Value) -> Result<String, String> {
    let url = format!("{}{}", flow_base(), FLOW_INSTANCES_PATH);
    let body = json!({ "definitionKey": def_key, "variables": payload });
    let resp = client()
        .post(&url)
        .header("X-Tenant", tenant)
        .header("X-Onto-Source", "cmx-ontology")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("起流程请求失败（{url}）: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "起流程非 2xx（{status}）: {}",
            text.chars().take(200).collect::<String>()
        ));
    }
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if let Some(code) = v.get("code").and_then(|c| c.as_i64()) {
        if code != 0 {
            let msg = v.get("msg").and_then(|m| m.as_str()).unwrap_or("");
            return Err(format!("起流程返回 code={code} {msg}"));
        }
    }
    let data = v.get("data").unwrap_or(&v);
    let id = data
        .get("id")
        .or_else(|| data.get("instanceId"))
        .and_then(|i| i.as_str())
        .unwrap_or("");
    Ok(id.to_string())
}
