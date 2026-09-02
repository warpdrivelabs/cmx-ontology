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
const DEFAULT_REPORT_BASE: &str = "http://127.0.0.1:8092";

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

/// 调 flowengine 的服务间 API Key（`ONTO_FLOW_API_KEY` / `onto.flow_api_key`）。
/// 设了则起实例请求带 `X-API-Key`（portal→flow 同款服务身份，命中 flow `[auth].api_keys` 短路 JWT）；
/// 未设则仅带 `X-Tenant`（适配 flow off 模式）。
pub fn flow_api_key() -> Option<String> {
    let v = cfg("ONTO_FLOW_API_KEY", "onto.flow_api_key", "");
    if v.is_empty() { None } else { Some(v) }
}

/// 出站配置快照（诊断端点 `GET /action-outbox/config` 用；**不含任何密钥**，仅暴露是否已配）。
pub fn config_snapshot() -> Value {
    json!({
        "outboundEnabled": outbound_enabled(),
        "flowUrl": flow_base(),
        "flowInstancesPath": FLOW_INSTANCES_PATH,
        "flowApiKeySet": flow_api_key().is_some(),
        "reportUrl": report_base(),
        "reportApiKeySet": report_api_key().is_some(),
        "webhookAllow": webhook_allow(),
    })
}

/// 报表平台基址（去尾斜杠；默认 `http://127.0.0.1:8092`）。
pub fn report_base() -> String {
    cfg("ONTO_REPORT_URL", "onto.report_url", DEFAULT_REPORT_BASE)
        .trim_end_matches('/')
        .to_string()
}

/// 调 cmx-report 的服务间 API Key（`ONTO_REPORT_API_KEY` / `onto.report_api_key`）。
pub fn report_api_key() -> Option<String> {
    let v = cfg("ONTO_REPORT_API_KEY", "onto.report_api_key", "");
    if v.is_empty() { None } else { Some(v) }
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
    let mut body = json!({ "definitionKey": def_key, "variables": payload });
    // 副作用 payload 若带 businessKey，则透传为流程业务键（便于回查/幂等/单据关联）。
    if let Some(bk) = payload.get("businessKey").and_then(|v| v.as_str()) {
        body["businessKey"] = json!(bk);
    }
    let mut rb = client()
        .post(&url)
        .header("X-Tenant", tenant)
        .header("X-Onto-Source", "cmx-ontology");
    if let Some(key) = flow_api_key() {
        rb = rb.header("X-API-Key", key); // 服务身份，命中 flow [auth].api_keys 短路 JWT
    }
    let resp = rb
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

/// 列 flowengine 已发布流程定义（设计台「触发流程」副作用的可视化选择器数据源）。
/// 返回 flow `GET /api/flow/v1/definitions` 的 data（通常为 `[{key,name,...}]`）。flow 不可达即 Err。
pub async fn list_flow_definitions(tenant: &str) -> Result<Value, String> {
    let url = format!("{}/api/flow/v1/definitions", flow_base());
    let mut rb = client()
        .get(&url)
        .header("X-Tenant", tenant)
        .header("X-Onto-Source", "cmx-ontology");
    if let Some(key) = flow_api_key() {
        rb = rb.header("X-API-Key", key);
    }
    let resp = rb.send().await.map_err(|e| format!("列流程定义失败（{url}）: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("列流程定义非 2xx（{status}）"));
    }
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok(v.get("data").cloned().unwrap_or(v))
}

/// 触发报表计算：POST `{report_base}/api/report-design/reports/{code}/compute`（body 透传 payload，
/// cmx-report 读 `orgCode`/`periodCode`/`version?` 真算落 cr_cell_data）。返回 compute 结果 data。
pub async fn compute_report(tenant: &str, code: &str, payload: &Value) -> Result<Value, String> {
    let url = format!("{}/api/report-design/reports/{}/compute", report_base(), code);
    let mut rb = client()
        .post(&url)
        .header("X-Tenant", tenant)
        .header("X-Onto-Source", "cmx-ontology");
    if let Some(key) = report_api_key() {
        rb = rb.header("X-API-Key", key);
    }
    let resp = rb.json(payload).send().await.map_err(|e| format!("生成报表请求失败（{url}）: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "生成报表非 2xx（{status}）: {}",
            text.chars().take(200).collect::<String>()
        ));
    }
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if let Some(c) = v.get("code").and_then(|c| c.as_i64()) {
        if c != 0 {
            let msg = v.get("msg").and_then(|m| m.as_str()).unwrap_or("");
            return Err(format!("生成报表返回 code={c} {msg}"));
        }
    }
    Ok(v.get("data").cloned().unwrap_or(v))
}

/// 列 cmx-report 报表（设计台「生成报表」副作用的可视化选择器数据源）。
pub async fn list_reports(tenant: &str) -> Result<Value, String> {
    let url = format!("{}/api/report-design/reports", report_base());
    let mut rb = client()
        .get(&url)
        .header("X-Tenant", tenant)
        .header("X-Onto-Source", "cmx-ontology");
    if let Some(key) = report_api_key() {
        rb = rb.header("X-API-Key", key);
    }
    let resp = rb.send().await.map_err(|e| format!("列报表失败（{url}）: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("列报表非 2xx（{status}）"));
    }
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok(v.get("data").cloned().unwrap_or(v))
}
