//! O3 数据集成 · app 层：源→对象映射 CRUD + 全量同步 + 隔离区 + 管道状态。
//!
//! 全量同步是**长任务雏形**（M1 同步执行；M2 接异步任务中心 SSE 进度/暂停/HA）。

use crate::resp::{ApiResp, OntoError, Result};
use crate::tenant::current_tenant;
use axum::extract::{Path, Query};
use axum::Json;
use cmx_onto_store_pg::FunnelStore;
use serde::Deserialize;
use serde_json::{json, Value};

fn funnel() -> FunnelStore {
    FunnelStore::new(crate::tenancy::current_db_id())
}

/// GET /funnel/mappings —— 列出源映射。
pub async fn list_mappings() -> Result<Json<ApiResp<Value>>> {
    let out = funnel()
        .list_mappings()
        .await
        .map_err(|e| OntoError::internal_error(format!("查映射失败: {e}")))?;
    Ok(Json(ApiResp::ok(out)))
}

/// POST /funnel/mappings —— upsert 源映射。
pub async fn upsert_mapping(Json(body): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let ot = funnel()
        .upsert_mapping(&body)
        .await
        .map_err(|e| OntoError::business_error(format!("写映射失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "objectType": ot, "saved": true }))))
}

/// DELETE /funnel/mappings/{object_type} —— 删除映射。
pub async fn delete_mapping(Path(object_type): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let n = funnel()
        .delete_mapping(&object_type)
        .await
        .map_err(|e| OntoError::internal_error(format!("删映射失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "objectType": object_type, "deleted": n > 0 }))))
}

/// POST /funnel/sync/{object_type} —— 全量同步（读源→映射→合格 upsert，违规入隔离区）。
pub async fn run_sync(Path(object_type): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let report = funnel()
        .run_full_sync(&tenant, &object_type)
        .await
        .map_err(|e| OntoError::business_error(format!("同步失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({
        "objectType": object_type,
        "read": report.read,
        "written": report.written,
        "quarantined": report.quarantined,
        "mode": "full"
    }))))
}

/// 隔离区查询参数。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct QuarantineQuery {
    pub object_type: Option<String>,
    pub limit: Option<i64>,
}

/// GET /funnel/quarantine —— 隔离区（校验不通过的源行 + violations）。
pub async fn list_quarantine(Query(q): Query<QuarantineQuery>) -> Result<Json<ApiResp<Value>>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let out = funnel()
        .list_quarantine(q.object_type.as_deref(), limit)
        .await
        .map_err(|e| OntoError::internal_error(format!("查隔离区失败: {e}")))?;
    Ok(Json(ApiResp::ok(out)))
}

/// GET /funnel/pipeline-status/{object_type} —— 管道图（抽取/映射/索引三段 + 计数）。
pub async fn pipeline_status(Path(object_type): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let out = funnel()
        .pipeline_status(&object_type)
        .await
        .map_err(|e| OntoError::internal_error(format!("查管道状态失败: {e}")))?;
    Ok(Json(ApiResp::ok(out)))
}
