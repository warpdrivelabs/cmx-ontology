//! 全部 axum handler（对任意 state 泛型 S 成立——不绑 State 提取器）。
//!
//! O1 端点：六类元模型元素（对象/关系/接口/共享属性/动作/函数类型）的 CRUD + 结构校验 +
//! 全量清单 + 发布/版本快照。所有写操作先结构校验（返回结构化错误），再 upsert。

use crate::engine::store;
use crate::resp::{ApiResp, OntoError, Result};
use crate::tenant::{current_display_user, current_tenant};
use axum::extract::Path;
use axum::Json;
use cmx_onto_model::{
    ActionTypeDef, FunctionDef, InterfaceDef, LinkTypeDef, ObjectTypeDef, OntologyStore,
    SharedPropertyTypeDef,
};
use serde::Deserialize;
use serde_json::{json, Value};

// ───────────────────────────── 对象类型 ─────────────────────────────

/// GET /object-types —— 对象类型清单。
pub async fn list_object_types() -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let metas = store()
        .list_object_types(&tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("列出对象类型失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!(metas))))
}

/// GET /object-types/{apiName} —— 对象类型详情（含完整属性）。
pub async fn get_object_type(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let def = store()
        .get_object_type(&tenant, &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载对象类型失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("对象类型 {api_name} 不存在")))?;
    Ok(Json(ApiResp::ok(json!(def))))
}

/// POST /object-types —— upsert 对象类型（结构校验后落库）。
pub async fn save_object_type(Json(def): Json<ObjectTypeDef>) -> Result<Json<ApiResp<Value>>> {
    def.validate()
        .map_err(|e| OntoError::business_error(format!("对象类型非法: {e}")))?;
    let tenant = current_tenant();
    store()
        .upsert_object_type(&tenant, &def)
        .await
        .map_err(|e| OntoError::internal_error(format!("保存对象类型失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "apiName": def.api_name, "saved": true }))))
}

/// POST /object-types/validate —— 仅结构校验（不落库）。
pub async fn validate_object_type(Json(def): Json<ObjectTypeDef>) -> Result<Json<ApiResp<Value>>> {
    match def.validate() {
        Ok(()) => Ok(Json(ApiResp::ok(json!({ "valid": true })))),
        Err(e) => Ok(Json(ApiResp::ok(
            json!({ "valid": false, "error": e.to_string() }),
        ))),
    }
}

/// DELETE /object-types/{apiName} —— 删除对象类型。
pub async fn delete_object_type(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let n = store()
        .delete_object_type(&tenant, &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("删除对象类型失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "apiName": api_name, "deleted": n > 0 }))))
}

// ───────────────────────────── 关系类型 ─────────────────────────────

pub async fn list_link_types() -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let metas = store()
        .list_link_types(&tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("列出关系类型失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!(metas))))
}

pub async fn get_link_type(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let def = store()
        .get_link_type(&tenant, &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载关系类型失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("关系类型 {api_name} 不存在")))?;
    Ok(Json(ApiResp::ok(json!(def))))
}

pub async fn save_link_type(Json(def): Json<LinkTypeDef>) -> Result<Json<ApiResp<Value>>> {
    def.validate()
        .map_err(|e| OntoError::business_error(format!("关系类型非法: {e}")))?;
    let tenant = current_tenant();
    store()
        .upsert_link_type(&tenant, &def)
        .await
        .map_err(|e| OntoError::internal_error(format!("保存关系类型失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "apiName": def.api_name, "saved": true }))))
}

pub async fn delete_link_type(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let n = store()
        .delete_link_type(&tenant, &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("删除关系类型失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "apiName": api_name, "deleted": n > 0 }))))
}

// ───────────────────────────── 接口 ─────────────────────────────

pub async fn list_interfaces() -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let metas = store()
        .list_interfaces(&tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("列出接口失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!(metas))))
}

pub async fn get_interface(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let def = store()
        .get_interface(&tenant, &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载接口失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("接口 {api_name} 不存在")))?;
    Ok(Json(ApiResp::ok(json!(def))))
}

pub async fn save_interface(Json(def): Json<InterfaceDef>) -> Result<Json<ApiResp<Value>>> {
    def.validate()
        .map_err(|e| OntoError::business_error(format!("接口非法: {e}")))?;
    let tenant = current_tenant();
    store()
        .upsert_interface(&tenant, &def)
        .await
        .map_err(|e| OntoError::internal_error(format!("保存接口失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "apiName": def.api_name, "saved": true }))))
}

pub async fn delete_interface(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let n = store()
        .delete_interface(&tenant, &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("删除接口失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "apiName": api_name, "deleted": n > 0 }))))
}

// ─────────────────────── 共享属性类型 ───────────────────────

pub async fn list_shared_properties() -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let metas = store()
        .list_shared_properties(&tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("列出共享属性失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!(metas))))
}

pub async fn get_shared_property(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let def = store()
        .get_shared_property(&tenant, &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载共享属性失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("共享属性 {api_name} 不存在")))?;
    Ok(Json(ApiResp::ok(json!(def))))
}

pub async fn save_shared_property(
    Json(def): Json<SharedPropertyTypeDef>,
) -> Result<Json<ApiResp<Value>>> {
    def.validate()
        .map_err(|e| OntoError::business_error(format!("共享属性非法: {e}")))?;
    let tenant = current_tenant();
    store()
        .upsert_shared_property(&tenant, &def)
        .await
        .map_err(|e| OntoError::internal_error(format!("保存共享属性失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "apiName": def.api_name, "saved": true }))))
}

pub async fn delete_shared_property(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let n = store()
        .delete_shared_property(&tenant, &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("删除共享属性失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "apiName": api_name, "deleted": n > 0 }))))
}

// ───────────────────────────── 动作类型 ─────────────────────────────

pub async fn list_action_types() -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let metas = store()
        .list_action_types(&tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("列出动作类型失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!(metas))))
}

pub async fn get_action_type(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let def = store()
        .get_action_type(&tenant, &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载动作类型失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("动作类型 {api_name} 不存在")))?;
    Ok(Json(ApiResp::ok(json!(def))))
}

pub async fn save_action_type(Json(def): Json<ActionTypeDef>) -> Result<Json<ApiResp<Value>>> {
    def.validate()
        .map_err(|e| OntoError::business_error(format!("动作类型非法: {e}")))?;
    let tenant = current_tenant();
    store()
        .upsert_action_type(&tenant, &def)
        .await
        .map_err(|e| OntoError::internal_error(format!("保存动作类型失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "apiName": def.api_name, "saved": true }))))
}

pub async fn delete_action_type(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let n = store()
        .delete_action_type(&tenant, &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("删除动作类型失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "apiName": api_name, "deleted": n > 0 }))))
}

// ───────────────────────────── 函数 ─────────────────────────────

pub async fn list_functions() -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let metas = store()
        .list_functions(&tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("列出函数失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!(metas))))
}

pub async fn get_function(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let def = store()
        .get_function(&tenant, &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载函数失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("函数 {api_name} 不存在")))?;
    Ok(Json(ApiResp::ok(json!(def))))
}

pub async fn save_function(Json(def): Json<FunctionDef>) -> Result<Json<ApiResp<Value>>> {
    def.validate()
        .map_err(|e| OntoError::business_error(format!("函数非法: {e}")))?;
    let tenant = current_tenant();
    store()
        .upsert_function(&tenant, &def)
        .await
        .map_err(|e| OntoError::internal_error(format!("保存函数失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "apiName": def.api_name, "saved": true }))))
}

pub async fn delete_function(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let n = store()
        .delete_function(&tenant, &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("删除函数失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "apiName": api_name, "deleted": n > 0 }))))
}

// ─────────────────────── 清单 / 发布 / 版本 ───────────────────────

/// GET /manifest —— 本体全量清单（六类元素的列表）。
pub async fn manifest() -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let m = store()
        .manifest(&tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载清单失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!(m))))
}

/// 发布请求体。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PublishReq {
    pub summary: String,
}

/// POST /publish —— 发布当前本体为不可变版本快照。
pub async fn publish(Json(req): Json<PublishReq>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let meta = store()
        .publish(&tenant, &req.summary, current_display_user())
        .await
        .map_err(|e| OntoError::internal_error(format!("发布失败: {e}")))?;
    // O7 实时：广播发布事件（订阅者经 /events SSE 感知）。
    crate::events::emit(&tenant, "published", json!(meta));
    Ok(Json(ApiResp::ok(json!(meta))))
}

/// GET /versions —— 发布版本列表（降序）。
pub async fn list_versions() -> Result<Json<ApiResp<Value>>> {
    let versions = store()
        .list_versions()
        .await
        .map_err(|e| OntoError::internal_error(format!("列出版本失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!(versions))))
}

/// GET /versions/{version} —— 某版本发布快照（全量定义）。
pub async fn get_version(Path(version): Path<u32>) -> Result<Json<ApiResp<Value>>> {
    let snap = store()
        .get_version(version)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载版本失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("版本 {version} 不存在")))?;
    Ok(Json(ApiResp::ok(snap)))
}
