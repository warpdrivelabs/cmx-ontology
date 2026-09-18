//! 建模台 / 监控大盘数据源：各类型计数 + 发布态。

use crate::engine::store;
use crate::resp::{ApiResp, OntoError, Result};
use crate::tenant::current_tenant;
use axum::Json;
use cmx_onto_model::OntologyStore;
use serde_json::{json, Value};

/// GET /stats —— 本体各类型计数 + 最新发布版本（建模台顶部统计块 / /_mon 消费）。
#[utoipa::path(
    get,
    path = "/api/onto/v1/stats",
    tag = "工具",
    summary = "各类元素计数 + 最新检查点版本",
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn stats() -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let m = store()
        .manifest(&tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("统计失败: {e}")))?;
    let versions = store().list_versions().await.unwrap_or_default();
    let latest = versions.first().map(|v| v.version).unwrap_or(0);
    Ok(Json(ApiResp::ok(json!({
        "objectTypes": m.object_types.len(),
        "linkTypes": m.link_types.len(),
        "interfaces": m.interfaces.len(),
        "sharedProperties": m.shared_properties.len(),
        "actionTypes": m.action_types.len(),
        "functions": m.functions.len(),
        "publishedVersion": latest,
        "versionCount": versions.len(),
    }))))
}
