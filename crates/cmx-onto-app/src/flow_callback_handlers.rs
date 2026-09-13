//! 流程审批结果回调：接收 cmx-flowengine 生命周期事件 webhook（`instance.completed` 等），
//! 按 `businessKey`（=对象 pk）回写本体对象状态——闭环「本体 action 发起流程 → 审批 → 回写」。
//!
//! 鉴权：依赖请求头 `X-API-Key`（flowengine 走 `[service_rpc.services]` 目录模式出站时由
//! service-rpc 基座自动注入，命中 `[auth].api_keys` 短路 JWT）；本端点不新增白名单。
//! 幂等：非 completed 事件 / businessKey 找不到对象 / 对象已同状态 → 一律 200 `{skipped:…}`，
//! 避免投递重试把事件打进死信（DEAD）污染画面。
//!
//! 目标对象类型可配：`ONTO_FLOW_CALLBACK_OBJECT_TYPE` / `onto.flow_callback_object_type`（默认 `Supplier`）；
//! 回写字段固定为 `reviewStatus`（= "已通过"）+ `lastReviewAt`（RFC3339），仅当事件 `event=instance.completed`。

use crate::object_engine::object_store;
use crate::resp::{ApiResp, OntoError, Result};
use axum::Json;
use serde_json::{json, Value};

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

/// POST /flow-callback —— flowengine 事件回调（审批完成 → 回写对象状态）。
pub async fn receive(Json(event): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let kind = event.get("event").and_then(|v| v.as_str()).unwrap_or("");
    if kind != "instance.completed" {
        // terminated/rejected 等暂不回写（评审否决走 terminate；需要时按 lastDecision 扩展）
        return Ok(Json(ApiResp::ok(json!({ "skipped": true, "reason": format!("event={kind} 不处理") }))));
    }
    let object_type = cfg(
        "ONTO_FLOW_CALLBACK_OBJECT_TYPE",
        "onto.flow_callback_object_type",
        "Supplier",
    );
    let pk = event
        .get("businessKey")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if pk.is_empty() {
        return Ok(Json(ApiResp::ok(json!({ "skipped": true, "reason": "businessKey 为空" }))));
    }
    let set = json!({ "reviewStatus": "已通过", "lastReviewAt": chrono::Utc::now().to_rfc3339() });
    match object_store().modify_with_optlock(&object_type, &pk, &set, None).await {
        Ok((status, updated_at, _)) => Ok(Json(ApiResp::ok(json!({
            "skipped": false,
            "objectType": object_type,
            "pk": pk,
            "writeStatus": status, // applied / conflict / notFound
            "updatedAt": updated_at,
        })))),
        Err(e) => {
            // 对象表不存在（类型从未物化）等业务性缺失 → skipped；真故障（连接等）→ 500 让 flow 重投
            let msg = format!("{e}");
            if msg.contains("不存在") || msg.contains("notFound") || msg.contains("Table") {
                Ok(Json(ApiResp::ok(json!({ "skipped": true, "reason": msg }))))
            } else {
                Err(OntoError::internal_error(format!("回写对象失败: {e}")))
            }
        }
    }
}
