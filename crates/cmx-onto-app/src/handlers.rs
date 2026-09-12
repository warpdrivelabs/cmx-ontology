//! 全部 axum handler（对任意 state 泛型 S 成立——不绑 State 提取器）。
//!
//! O1 端点：六类元模型元素（对象/关系/接口/共享属性/动作/函数类型）的 CRUD + 结构校验 +
//! 全量清单 + 发布/版本快照。所有写操作先结构校验（返回结构化错误），再 upsert。

use crate::engine::store;
use crate::resp::{ApiResp, OntoError, Result};
use crate::tenant::{current_display_user, current_tenant};
use axum::extract::{Path, Query};
use axum::Json;
use cmx_onto_model::{
    ActionTypeDef, FunctionDef, InterfaceDef, LinkTypeDef, ObjectTypeDef, OntologyStore,
    SharedPropertyTypeDef, StoreError,
};
use serde::Deserialize;
use serde_json::{json, Value};

// ───────────────────────────── 对象类型 ─────────────────────────────

// GET /object-types（清单 + A2 q/dam/page/size 扩展）已迁至 view_handlers.rs——
// 旧设计器不传参仍得全量数组（零破坏），新页面目录表格走分页信封。

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

/// POST /object-types/batch —— 按 apiName 列表批量取完整定义（设计器首屏装载用）。
///
/// 单 SQL `= ANY($1)` 把 N 次远端往返折叠为 1 次（远端库每往返 ~190ms，44 类型逐个拉
/// ≈ 8.7s 的主体）。不存在的 apiName 静默跳过（清单驱动下的批量装载容忍并发删除）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectTypesBatchReq {
    pub api_names: Vec<String>,
}
pub async fn get_object_types_batch(
    Json(req): Json<ObjectTypesBatchReq>,
) -> Result<Json<ApiResp<Value>>> {
    if req.api_names.len() > 2000 {
        return Err(OntoError::bad_request("apiNames 数量超限（≤2000）"));
    }
    let tenant = current_tenant();
    let defs = store()
        .get_object_types_batch(&tenant, &req.api_names)
        .await
        .map_err(|e| OntoError::internal_error(format!("批量装载对象类型失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!(defs))))
}

/// POST /object-types —— upsert 对象类型（结构校验 + 接口契约校验后落库）。
///
/// B0 乐观锁：`version > 0` 走原子条件更新（跨标签页/久置缓冲的过期保存得 409）；
/// `version = 0`（新建 / quickCreate / import）保持既有盲写语义。响应带服务端递增后的
/// `version`，前端以响应刷新基线。
pub async fn save_object_type(Json(def): Json<ObjectTypeDef>) -> Result<Json<ApiResp<Value>>> {
    def.validate()
        .map_err(|e| OntoError::business_error(format!("对象类型非法: {e}")))?;
    let tenant = current_tenant();
    // 接口强校验（#4）：仅当声明了 implements 才逐个装载接口 + 其要求的共享属性定义，
    // 交内核纯函数 validate_implements 校验"实现者具备接口要求的共享属性且类型匹配"。
    if !def.implements.is_empty() {
        validate_object_implements(&tenant, &def).await?;
    }
    let version = store()
        .upsert_object_type_locked(&tenant, &def)
        .await
        .map_err(|e| match e {
            StoreError::Conflict(m) => OntoError::conflict(m),
            StoreError::NotFound(m) => OntoError::not_found(m),
            other => OntoError::internal_error(format!("保存对象类型失败: {other}")),
        })?;
    Ok(Json(ApiResp::ok(
        json!({ "apiName": def.api_name, "saved": true, "version": version }),
    )))
}

/// 装载 `def.implements` 涉及的接口与共享属性定义，调用内核 [`validate_implements`]。
/// 缺失的接口/共享属性不入切片——由 `validate_implements` 报"未定义"，语义一致。
async fn validate_object_implements(tenant: &str, def: &ObjectTypeDef) -> Result<()> {
    let mut ifaces = Vec::new();
    let mut shared: Vec<SharedPropertyTypeDef> = Vec::new();
    for iface_name in &def.implements {
        if let Some(iface) = store()
            .get_interface(tenant, iface_name)
            .await
            .map_err(|e| OntoError::internal_error(format!("装载接口失败: {e}")))?
        {
            for spt_name in &iface.properties {
                if shared.iter().any(|s: &SharedPropertyTypeDef| &s.api_name == spt_name) {
                    continue;
                }
                if let Some(spt) = store()
                    .get_shared_property(tenant, spt_name)
                    .await
                    .map_err(|e| OntoError::internal_error(format!("装载共享属性失败: {e}")))?
                {
                    shared.push(spt);
                }
            }
            ifaces.push(iface);
        }
    }
    cmx_onto_model::validate_implements(def, &ifaces, &shared)
        .map_err(|e| OntoError::business_error(format!("接口契约校验未通过: {e}")))
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
/// 安全网：①被引用（关系/动作编辑/场景成员）→ 409 出引用清单；②删除前自动存档（可撤销）。
pub async fn delete_object_type(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let refs = store()
        .object_type_references(&api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("引用检查失败: {e}")))?;
    if !refs.is_empty() {
        let list = refs
            .iter()
            .map(|(k, n)| format!("{k}:{n}"))
            .collect::<Vec<_>>()
            .join("、");
        return Err(OntoError::conflict(format!(
            "对象类型 {api_name} 仍被引用（{list}），请先清理引用或改用「废弃」"
        )));
    }
    // 删除前自动存档（顺序钉死：引用检查通过后才存档，避免被拒删除留噪音快照）。
    auto_snapshot_before("删除对象类型", &api_name).await;
    let tenant = current_tenant();
    let n = store()
        .delete_object_type(&tenant, &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("删除对象类型失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "apiName": api_name, "deleted": n > 0 }))))
}

/// 高危删除前置自动存档：失败仅记日志不阻断删除（存档是兜底而非门槛；live 未变，不发事件）。
async fn auto_snapshot_before(op: &str, name: &str) {
    let tenant = current_tenant();
    let summary = format!("{op} {name} 前");
    if let Err(e) = store().archive_snapshot(&tenant, &summary, current_display_user()).await {
        tracing::warn!(op = %op, name = %name, "删除前自动存档失败（删除继续）: {e}");
    }
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

/// B2 留痕：backing.fk 外键映射（`{"fk":{"sourceProperty","targetProperty"}}`——与前端
/// `LINK_BACKING_FK` 常量互指，唯一口径）缺属性名时记服务端日志，不拒绝、不新增响应通道。
fn log_backing_fk_gaps(api_name: &str, backing: &Value) {
    let Some(fk) = backing.get("fk") else { return };
    for key in ["sourceProperty", "targetProperty"] {
        let v = fk.get(key).and_then(|x| x.as_str()).map(str::trim).unwrap_or("");
        if v.is_empty() {
            tracing::warn!(link = %api_name, field = key, "backing.fk 缺 {key}（映射不完整，速建气泡/关系 Inspector 应补齐）");
        }
    }
}

pub async fn save_link_type(Json(def): Json<LinkTypeDef>) -> Result<Json<ApiResp<Value>>> {
    def.validate()
        .map_err(|e| OntoError::business_error(format!("关系类型非法: {e}")))?;
    log_backing_fk_gaps(&def.api_name, &def.backing);
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

/// GET /interfaces 查询参数（本体工作室实现挂接/继承添加的帮助弹框；全缺省 = 既有全量数组语义）。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct InterfacesListQuery {
    pub q: Option<String>,
    pub page: Option<u32>,
    pub size: Option<u32>,
}

/// GET /interfaces —— 清单：不传参返回全量数组（既有语义）；传 q/page/size 任一
/// 返回分页信封 `{rows, total, page, size}`。
pub async fn list_interfaces(Query(qp): Query<InterfacesListQuery>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let paged = qp.q.is_some() || qp.page.is_some() || qp.size.is_some();
    if !paged {
        let metas = store()
            .list_interfaces(&tenant)
            .await
            .map_err(|e| OntoError::internal_error(format!("列出接口失败: {e}")))?;
        return Ok(Json(ApiResp::ok(json!(metas))));
    }
    let page = qp.page.unwrap_or(1);
    let size = qp.size.unwrap_or(50);
    let (rows, total) = store()
        .list_interfaces_paged(&tenant, qp.q.as_deref().unwrap_or(""), page, size)
        .await
        .map_err(|e| OntoError::internal_error(format!("查询接口目录失败: {e}")))?;
    let row_values: Vec<Value> = rows
        .iter()
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .collect();
    Ok(Json(ApiResp::ok(json!({
        "rows": row_values,
        "total": total,
        "page": page,
        "size": size,
    }))))
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

/// GET /shared-properties 查询参数（本体工作室帮助弹框；全缺省 = 既有全量数组语义，旧调用方零感知）。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SharedPropertiesListQuery {
    pub q: Option<String>,
    pub page: Option<u32>,
    pub size: Option<u32>,
}

/// GET /shared-properties —— 清单：不传参返回全量轻量 meta 数组（既有语义）；
/// 传 q/page/size 任一返回分页信封 `{rows, total, page, size}`（rows 为完整定义，
/// 含 baseType/semanticType——选择器免逐个 GET 详情）。
pub async fn list_shared_properties(
    Query(qp): Query<SharedPropertiesListQuery>,
) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let paged = qp.q.is_some() || qp.page.is_some() || qp.size.is_some();
    if !paged {
        let metas = store()
            .list_shared_properties(&tenant)
            .await
            .map_err(|e| OntoError::internal_error(format!("列出共享属性失败: {e}")))?;
        return Ok(Json(ApiResp::ok(json!(metas))));
    }
    let page = qp.page.unwrap_or(1);
    let size = qp.size.unwrap_or(50);
    let (rows, total) = store()
        .list_shared_properties_paged(&tenant, qp.q.as_deref().unwrap_or(""), page, size)
        .await
        .map_err(|e| OntoError::internal_error(format!("查询共享属性目录失败: {e}")))?;
    let row_values: Vec<Value> = rows
        .iter()
        .map(|sp| serde_json::to_value(sp).unwrap_or(Value::Null))
        .collect();
    Ok(Json(ApiResp::ok(json!({
        "rows": row_values,
        "total": total,
        "page": page,
        "size": size,
    }))))
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

/// DELETE /shared-properties/{apiName} —— 删除共享属性。
/// 安全网：①被引用（对象属性/接口契约）→ 409 出引用清单；②删除前自动存档（可撤销）。
pub async fn delete_shared_property(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let refs = store()
        .shared_property_references(&api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("引用检查失败: {e}")))?;
    if !refs.is_empty() {
        let list = refs
            .iter()
            .map(|(k, n)| format!("{k}:{n}"))
            .collect::<Vec<_>>()
            .join("、");
        return Err(OntoError::conflict(format!(
            "共享属性 {api_name} 仍被引用（{list}），请先清理引用"
        )));
    }
    auto_snapshot_before("删除共享属性", &api_name).await;
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

pub async fn save_action_type(Json(mut def): Json<ActionTypeDef>) -> Result<Json<ApiResp<Value>>> {
    def.validate()
        .map_err(|e| OntoError::business_error(format!("动作类型非法: {e}")))?;
    // 保存期校验（P0）：函数背书与 logic/side_effects 互斥；defaultValue 满足 multipleChoice。
    cmx_onto_model::save_validate_action(&def)
        .map_err(OntoError::business_error)?;
    // 保存期组合序列校验（P0-4 四规则）：logic 可静态解析时校验（$参数留原样、对象状态为空）；
    // 解析失败（如 src:param 引用值缺失）则跳过——运行期/试算期仍会拦截（fail-closed 双保险）。
    if def.function_backing.as_deref().map(|s| s.trim().is_empty()).unwrap_or(true)
        && let Ok(edits) = cmx_onto_model::resolve_edits(&def, &json!({}), &json!({}), None, None)
    {
        cmx_onto_model::validate_edit_sequence(&edits)
            .map_err(OntoError::business_error)?;
    }
    // 自动派生缺失参数（P1-2，只增不删）：① logic $name ② src 显式引用 ③ side_effects 引用
    // ④ 函数背书时从 FunctionDef.inputs 派生（object/objectSet 型入参同样成参数）。
    let mut derived = cmx_onto_model::derive_missing_params(&mut def);
    let tenant = current_tenant();
    if def.function_backing.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false) {
        let fname = def.function_backing.as_deref().unwrap_or("").trim();
        let func = store()
            .get_function(&tenant, fname)
            .await
            .map_err(|e| OntoError::business_error(format!("装载函数 {fname} 失败: {e}")))?
            .ok_or_else(|| OntoError::business_error(format!("函数 {fname} 未定义")))?;
        let mut ps = def.parameters.as_array().cloned().unwrap_or_default();
        for spec in cmx_onto_model::input_specs(&func) {
            let exists = ps.iter().any(|p| p.get("name").and_then(|v| v.as_str()) == Some(spec.name.as_str()));
            if !exists {
                ps.push(json!({ "name": spec.name, "required": true, "type": spec.ty }));
                if !derived.iter().any(|d| d == &spec.name) {
                    derived.push(spec.name.clone());
                }
            }
        }
        def.parameters = json!(ps);
    }
    store()
        .upsert_action_type(&tenant, &def)
        .await
        .map_err(|e| OntoError::internal_error(format!("保存动作类型失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({
        "apiName": def.api_name,
        "saved": true,
        "derivedParams": derived,
    }))))
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

// ─────────────────────── 清单 / 版本 ───────────────────────

/// GET /manifest 查询参数。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ManifestQuery {
    /// 可选：逗号分隔的类型键（objectTypes/linkTypes/interfaces/sharedProperties/
    /// actionTypes/functions，大小写不敏感，亦接受 object/link/... 简写）。
    /// 不传 = 全量六类；传了 = 仅装载指定类型（轻量消费方按需取数）。
    pub types: Option<String>,
}

/// kind 简写 → 清单键名归一。
fn normalize_manifest_type(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "objecttypes" | "object" | "objects" => Some("objectTypes"),
        "linktypes" | "link" | "links" => Some("linkTypes"),
        "interfaces" | "interface" => Some("interfaces"),
        "sharedproperties" | "shared" | "sharedproperty" => Some("sharedProperties"),
        "actiontypes" | "action" | "actions" => Some("actionTypes"),
        "functions" | "function" | "fn" => Some("functions"),
        _ => None,
    }
}

/// GET /manifest —— 本体全量清单（六类元素的列表）；`?types=` 支持按类型子集装载。
pub async fn manifest(Query(q): Query<ManifestQuery>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    match &q.types {
        None => {
            let m = store()
                .manifest(&tenant)
                .await
                .map_err(|e| OntoError::internal_error(format!("装载清单失败: {e}")))?;
            Ok(Json(ApiResp::ok(json!(m))))
        }
        Some(types) => {
            let mut kinds = Vec::new();
            for raw in types.split(',') {
                let Some(k) = normalize_manifest_type(raw) else {
                    return Err(OntoError::bad_request(format!(
                        "未知清单类型 {raw:?}（可用：objectTypes/linkTypes/interfaces/sharedProperties/actionTypes/functions）"
                    )));
                };
                if !kinds.contains(&k.to_string()) {
                    kinds.push(k.to_string());
                }
            }
            if kinds.is_empty() {
                return Err(OntoError::bad_request("types 参数为空"));
            }
            let m = store()
                .manifest_filtered(&tenant, &kinds)
                .await
                .map_err(|e| OntoError::internal_error(format!("装载清单失败: {e}")))?;
            Ok(Json(ApiResp::ok(json!(m))))
        }
    }
}

/// GET /versions —— 存档版本列表（降序）。
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
