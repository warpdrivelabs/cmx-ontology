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
#[utoipa::path(
    get,
    path = "/api/onto/v1/funnel/mappings",
    tag = "集成",
    summary = "列出源→对象映射",
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn list_mappings() -> Result<Json<ApiResp<Value>>> {
    let out = funnel()
        .list_mappings()
        .await
        .map_err(|e| OntoError::internal_error(format!("查映射失败: {e}")))?;
    Ok(Json(ApiResp::ok(out)))
}

/// POST /funnel/mappings —— upsert 源映射。
#[utoipa::path(
    post,
    path = "/api/onto/v1/funnel/mappings",
    tag = "集成",
    summary = "新建/更新映射",
    request_body(content = Value, description = "映射定义"),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn upsert_mapping(Json(body): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let ot = funnel()
        .upsert_mapping(&body)
        .await
        .map_err(|e| OntoError::business_error(format!("写映射失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "objectType": ot, "saved": true }))))
}

/// DELETE /funnel/mappings/{object_type} —— 删除映射。
#[utoipa::path(
    delete,
    path = "/api/onto/v1/funnel/mappings/{object_type}",
    tag = "集成",
    summary = "删除某对象类型的映射",
    params(
        ("object_type" = String, Path, description = "对象类型 API 名"),
    ),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn delete_mapping(Path(object_type): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let n = funnel()
        .delete_mapping(&object_type)
        .await
        .map_err(|e| OntoError::internal_error(format!("删映射失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "objectType": object_type, "deleted": n > 0 }))))
}

/// POST /funnel/sync/{object_type} —— 全量同步（读源→映射→合格 upsert，违规入隔离区）。
#[utoipa::path(
    post,
    path = "/api/onto/v1/funnel/sync/{object_type}",
    tag = "集成",
    summary = "全量同步：读源 → 映射转换 → 合格写入对象库，违规进隔离区",
    params(
        ("object_type" = String, Path, description = "对象类型 API 名"),
    ),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
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
#[utoipa::path(
    get,
    path = "/api/onto/v1/funnel/quarantine",
    tag = "集成",
    summary = "隔离区：违规源行 + violations 原因",
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn list_quarantine(Query(q): Query<QuarantineQuery>) -> Result<Json<ApiResp<Value>>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let out = funnel()
        .list_quarantine(q.object_type.as_deref(), limit)
        .await
        .map_err(|e| OntoError::internal_error(format!("查隔离区失败: {e}")))?;
    Ok(Json(ApiResp::ok(out)))
}

/// GET /funnel/pipeline-status/{object_type} —— 管道图（抽取/映射/索引三段 + 计数）。
#[utoipa::path(
    get,
    path = "/api/onto/v1/funnel/pipeline-status/{object_type}",
    tag = "集成",
    summary = "管道状态图数据（抽取/映射/索引三段计数）",
    params(
        ("object_type" = String, Path, description = "对象类型 API 名"),
    ),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn pipeline_status(Path(object_type): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let out = funnel()
        .pipeline_status(&object_type)
        .await
        .map_err(|e| OntoError::internal_error(format!("查管道状态失败: {e}")))?;
    Ok(Json(ApiResp::ok(out)))
}

/// 表名词边界匹配：`cm_supplier` 不得命中 `cm_supplier_xxx`（后者是非字母下划线继续）。
fn query_touches_table(source_query: &str, dict_code: &str) -> bool {
    let table = format!("cm_{dict_code}");
    let mut from = 0usize;
    while let Some(pos) = source_query[from..].find(&table) {
        let abs = from + pos + table.len();
        let next = source_query[abs..].chars().next();
        if !next.is_some_and(|c| c.is_ascii_alphabetic() || c == '_') {
            return true;
        }
        from = abs;
    }
    false
}

/// POST /funnel/push —— 主数据事件推送（MDM 分发引擎 webhook 订阅入口）。
///
/// 收到激活事件后按 `dictCode` 定位命中映射的漏斗（sourceQuery 含 `cm_{dict_code}`）并自动
/// 全量同步——主数据变更免手动 sync。body 宽容：只消费 `dictCode`/`dict_code`，其余透传忽略；
/// `dictCode` 缺省时全量同步所有映射。幂等可重入（sync 本身按 pk upsert）。
#[utoipa::path(
    post,
    path = "/api/onto/v1/funnel/push",
    tag = "集成",
    summary = "MDM 主数据事件推送入口（分发引擎 webhook 订阅）",
    request_body(content = Value, description = "MDM 事件负载（宽容消费：只读 dictCode/dict_code，其余透传忽略）"),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn funnel_push(Json(body): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let dict_code = body
        .get("dictCode")
        .or_else(|| body.get("dict_code"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let mappings = funnel()
        .list_mappings()
        .await
        .map_err(|e| OntoError::internal_error(format!("枚举映射失败: {e}")))?;
    let tenant = current_tenant();
    let mut synced = Vec::new();
    // list_mappings 透传 JSON 数组；命中判定用 camelCase 键。
    for m in mappings.as_array().unwrap_or(&Vec::new()).clone() {
        let object_type = m
            .get("objectType")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if object_type.is_empty() {
            continue;
        }
        let source_query = m
            .get("sourceQuery")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let hit = dict_code
            .as_deref()
            .is_none_or(|dc| query_touches_table(source_query, dc));
        if !hit {
            continue;
        }
        let report = funnel()
            .run_full_sync(&tenant, &object_type)
            .await
            .map_err(|e| OntoError::business_error(format!("推送同步 {object_type} 失败: {e}")))?;
        synced.push(json!({
            "objectType": object_type,
            "read": report.read,
            "written": report.written,
            "quarantined": report.quarantined,
        }));
    }
    tracing::info!(dict_code = ?dict_code, synced = synced.len(), "funnel push 已处理");
    Ok(Json(ApiResp::ok(json!({ "dictCode": dict_code, "synced": synced }))))
}
