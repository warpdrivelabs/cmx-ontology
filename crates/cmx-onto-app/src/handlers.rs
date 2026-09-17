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
    allowed_link_status, ActionTypeDef, FunctionDef, InterfaceDef, LinkBacking, LinkEnd,
    LinkTypeDef, ObjectTypeDef, OntologyStore, SharedPropertyTypeDef, StoreError, TypeStatus,
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
/// save 请求体（raw Value：检测 status / deprecation 旁路携带 → 结构化 warning）。
/// 状态剥离纪律（方案 §5.2）：七类 save 一律忽略 status / 弃用元数据，变更唯一入口
/// 是 POST /lifecycle/transition。
fn stripped_warnings(body: &Value) -> Vec<String> {
    let mut w = Vec::new();
    if body.get("status").is_some() {
        w.push("status 已忽略：状态只能经 POST /lifecycle/transition 变更".to_string());
    }
    if body.get("deprecation").is_some() {
        w.push("deprecation 已忽略：弃用元数据只能经 POST /lifecycle/transition 维护".to_string());
    }
    w
}

pub async fn save_object_type(Json(body): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let warnings = stripped_warnings(&body);
    let def: ObjectTypeDef = serde_json::from_value(body)
        .map_err(|e| OntoError::bad_request(format!("对象类型请求体非法: {e}")))?;
    let tenant = current_tenant();
    let version = save_object_core(&tenant, def, None).await?;
    Ok(Json(ApiResp::ok(json!({
        "saved": true, "version": version, "warnings": warnings,
    }))))
}

/// 保存对象类型 core（HTTP handler 与 revert 共用；含校验 / active 保护 / 修订管道 / SSE）。
pub(crate) async fn save_object_core(
    tenant: &str,
    def: ObjectTypeDef,
    change_note: Option<&str>,
) -> Result<u32> {
    def.validate()
        .map_err(|e| OntoError::business_error(format!("对象类型非法: {e}")))?;
    // 接口强校验（#4）：仅当声明了 implements 才逐个装载接口 + 其要求的共享属性定义。
    if !def.implements.is_empty() {
        validate_object_implements(tenant, &def).await?;
    }
    // active 保护（§5.3）：active 资源不可改主键（改 apiName 等价新建不受限）。
    if def.version > 0
        && let Some(existing) = store()
            .get_object_type(tenant, &def.api_name)
            .await
            .map_err(|e| OntoError::internal_error(format!("装载对象类型失败: {e}")))?
        && existing.status == TypeStatus::Active
        && !existing.primary_key.is_empty()
        && def.primary_key != existing.primary_key
    {
        return Err(OntoError::conflict(
            "active 资源不可修改主键属性，请先降级到 experimental / deprecated",
        ));
    }
    let version = store()
        .save_object_with_revision(&def, &changed_by(), change_note)
        .await
        .map_err(|e| match e {
            StoreError::Conflict(m) => OntoError::conflict(m),
            StoreError::NotFound(m) => OntoError::not_found(m),
            other => OntoError::internal_error(format!("保存对象类型失败: {other}")),
        })?;
    emit_resource_changed(tenant, "object", &def.api_name, "save");
    Ok(version)
}

/// 变更人（修订 changed_by；无上下文时 anonymous）。
pub(crate) fn changed_by() -> String {
    current_display_user().unwrap_or_else(|| "anonymous".into())
}

/// SSE resource-changed（方案 §6.5：保存/流转/revert/恢复成功后广播）。
pub(crate) fn emit_resource_changed(tenant: &str, kind: &str, api_name: &str, action: &str) {
    crate::events::emit(
        tenant,
        "resource-changed",
        json!({ "kind": kind, "apiName": api_name, "action": action, "by": changed_by() }),
    );
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
    let tenant = current_tenant();
    let existing = store()
        .get_object_type(&tenant, &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载对象类型失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("对象类型 {api_name} 不存在")))?;
    // active 保护（§5.3）：active 资源不可删除。
    if existing.status == TypeStatus::Active {
        return Err(OntoError::conflict(
            "active 资源不可删除，请先降级到 experimental / deprecated",
        ));
    }
    // 结构依赖（D10）：底座内部引用（关系两端 / 动作编辑）409 硬拒；场景引用级联清理。
    let refs = store()
        .object_type_references(&api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("引用检查失败: {e}")))?;
    let structural: Vec<_> = refs.iter().filter(|(k, _)| k != "view").collect();
    if !structural.is_empty() {
        let list = structural
            .iter()
            .map(|(k, n)| format!("{k}:{n}"))
            .collect::<Vec<_>>()
            .join("、");
        return Err(OntoError::conflict(format!(
            "对象类型 {api_name} 仍被引用（{list}），请先清理引用或改用「废弃」"
        )));
    }
    let scene_refs = store()
        .view_refs_of_object(&api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("场景引用检查失败: {e}")))?;
    // 删除前自动存档（顺序钉死：引用检查通过后才存档，避免被拒删除留噪音快照）。
    auto_snapshot_before("删除对象类型", &api_name).await;
    // 场景级联清理（单事务；每受影响场景一条修订）。
    let affected_scenes = store()
        .cascade_cleanup_scene_refs(Some(&api_name), None, &changed_by())
        .await
        .map_err(|e| OntoError::internal_error(format!("级联清理场景引用失败: {e}")))?;
    let _ = &scene_refs;
    let n = store()
        .delete_with_revision("object", &api_name, &changed_by(), Some("删除对象类型"))
        .await
        .map_err(|e| OntoError::internal_error(format!("删除对象类型失败: {e}")))?;
    emit_resource_changed(&tenant, "object", &api_name, "delete");
    Ok(Json(ApiResp::ok(json!({
        "apiName": api_name,
        "deleted": n > 0,
        "sceneRefs": scene_refs.iter().map(|(a, d)| json!({"apiName": a, "displayName": d})).collect::<Vec<_>>(),
        "affectedScenes": affected_scenes,
    }))))
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

/// B2 留痕：backing.fk 外键映射（`{"fk":{"sourceProperty","side","targetProperty"?}}`——与前端
/// `LINK_BACKING_FK` 常量互指，唯一口径）缺属性名时记服务端日志；直写旧 tagged
/// `{"kind":...}` 口径同样留痕告警，静默变有痕。
fn log_backing_fk_gaps(api_name: &str, backing: &Value) {
    if backing.get("kind").is_some() {
        tracing::warn!(link = %api_name, "backing 含已废除的 tagged {{\"kind\":...}} 口径（解析按 Edge 兜底），请改写为页面口径 {{\"fk\":{{...}}}}");
        return;
    }
    let Some(fk) = backing.get("fk") else { return };
    for key in ["sourceProperty"] {
        let v = fk.get(key).and_then(|x| x.as_str()).map(str::trim).unwrap_or("");
        if v.is_empty() {
            tracing::warn!(link = %api_name, field = key, "backing.fk 缺 {key}（映射不完整，速建气泡/关系 Inspector 应补齐）");
        }
    }
}

pub async fn save_link_type(Json(body): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let warnings = stripped_warnings(&body);
    let def: LinkTypeDef = serde_json::from_value(body)
        .map_err(|e| OntoError::bad_request(format!("关系类型请求体非法: {e}")))?;
    let tenant = current_tenant();
    let version = save_link_core(&tenant, def, None).await?;
    Ok(Json(ApiResp::ok(json!({ "saved": true, "version": version, "warnings": warnings }))))
}

/// 保存关系类型 core（handler 与 revert 共用；矩阵校验作用点 1，方案 §5.4）。
pub(crate) async fn save_link_core(
    tenant: &str,
    def: LinkTypeDef,
    change_note: Option<&str>,
) -> Result<u32> {
    def.validate()
        .map_err(|e| OntoError::business_error(format!("关系类型非法: {e}")))?;
    log_backing_fk_gaps(&def.api_name, &def.backing);
    ensure_backing_references(tenant, &def).await?;
    // 兼容矩阵（作用点 1）：新建时任一端对象 deprecated → 409（默认 experimental 违反矩阵
    // 且 transition 前资源须先存在，无法事后补救——N14）；修改两端 → 校验 live 状态仍合规。
    let sa = store()
        .get_status("object", &def.object_type_a)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载对象状态失败: {e}")))?;
    let sb = store()
        .get_status("object", &def.object_type_b)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载对象状态失败: {e}")))?;
    let is_new = store()
        .get_status("link", &def.api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载关系状态失败: {e}")))?
        .is_none();
    if is_new && (sa == Some(TypeStatus::Deprecated) || sb == Some(TypeStatus::Deprecated)) {
        return Err(OntoError::conflict(
            "对象已废弃，不可新建关系（兼容矩阵：deprecated 对象端仅允许 deprecated 关系）",
        ));
    }
    if !is_new {
        let cur = store()
            .get_status("link", &def.api_name)
            .await
            .map_err(|e| OntoError::internal_error(format!("装载关系状态失败: {e}")))?
            .unwrap_or_default();
        let allowed = allowed_link_status(sa.unwrap_or_default(), sb.unwrap_or_default());
        if !allowed.contains(&cur) {
            return Err(OntoError::conflict(format!(
                "兼容矩阵不允许关系当前状态 {cur:?}（两端对象状态 {sa:?}/{sb:?} 仅允许 {allowed:?}）——请先调整对象状态或废弃该关系"
            )));
        }
    }
    let version = store()
        .save_link_with_revision(&def, &changed_by(), change_note)
        .await
        .map_err(|e| match e {
            StoreError::Conflict(m) => OntoError::conflict(m),
            StoreError::NotFound(m) => OntoError::not_found(m),
            other => OntoError::internal_error(format!("保存关系类型失败: {other}")),
        })?;
    emit_resource_changed(tenant, "link", &def.api_name, "save");
    Ok(version)
}

/// save 跨端校验（定义层 validate 拿不到属性注册表，这里 async 补齐）：
/// 非 Edge backing 的两端对象类型须已注册；FK 的 sourceProperty 须存在于持键端
/// 属性定义中；Intermediary 的中间对象类型须已注册。JoinTable 连接表本体由
/// store 层 upsert_link_type 的索引维护把关（缺表明确报错）。
async fn ensure_backing_references(tenant: &str, def: &LinkTypeDef) -> Result<()> {
    if matches!(def.backing_parsed(), LinkBacking::Edge) {
        return Ok(());
    }
    ensure_object_registered(tenant, &def.object_type_a, "A 端").await?;
    ensure_object_registered(tenant, &def.object_type_b, "B 端").await?;
    match def.backing_parsed() {
        LinkBacking::ForeignKey { property, side, target_property } => {
            let holder = if side == LinkEnd::A { &def.object_type_a } else { &def.object_type_b };
            let other = if side == LinkEnd::A { &def.object_type_b } else { &def.object_type_a };
            let meta = store()
                .get_object_type(tenant, holder)
                .await
                .map_err(|e| OntoError::internal_error(format!("装载对象类型失败: {e}")))?
                .ok_or_else(|| OntoError::business_error(format!("对象类型 {holder} 未注册")))?;
            let known = meta.properties.iter().any(|p| p.api_name == property);
            if !known {
                return Err(OntoError::business_error(format!(
                    "对象类型 {holder} 无属性「{property}」：ForeignKey 的 sourceProperty 必须是持键端已注册属性"
                )));
            }
            if let Some(tp) = &target_property {
                let other_meta = store()
                    .get_object_type(tenant, other)
                    .await
                    .map_err(|e| OntoError::internal_error(format!("装载对象类型失败: {e}")))?
                    .ok_or_else(|| OntoError::business_error(format!("对象类型 {other} 未注册")))?;
                let tp_known = other_meta.properties.iter().any(|p| p.api_name == *tp);
                if !tp_known {
                    return Err(OntoError::business_error(format!(
                        "对象类型 {other} 无属性「{tp}」：ForeignKey 的 targetProperty 必须是对端已注册属性"
                    )));
                }
            }
        }
        LinkBacking::Intermediary { object_type, .. } => {
            ensure_object_registered(tenant, &object_type, "中间对象").await?;
        }
        _ => {}
    }
    Ok(())
}

async fn ensure_object_registered(tenant: &str, api_name: &str, label: &str) -> Result<()> {
    let known = store()
        .get_object_type(tenant, api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载对象类型失败: {e}")))?
        .is_some();
    if !known {
        return Err(OntoError::business_error(format!(
            "{label}对象类型 {api_name} 未注册，请先保存对象类型"
        )));
    }
    Ok(())
}

pub async fn delete_link_type(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let existing = store()
        .get_status("link", &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载关系类型失败: {e}")))?;
    let Some(status) = existing else {
        return Err(OntoError::not_found(format!("关系类型 {api_name} 不存在")));
    };
    if status == TypeStatus::Active {
        return Err(OntoError::conflict(
            "active 资源不可删除，请先降级到 experimental / deprecated",
        ));
    }
    let scene_refs = store()
        .view_refs_of_link(&api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("场景引用检查失败: {e}")))?;
    auto_snapshot_before("删除关系类型", &api_name).await;
    let affected_scenes = store()
        .cascade_cleanup_scene_refs(None, Some(&api_name), &changed_by())
        .await
        .map_err(|e| OntoError::internal_error(format!("级联清理场景引用失败: {e}")))?;
    let n = store()
        .delete_with_revision("link", &api_name, &changed_by(), Some("删除关系类型"))
        .await
        .map_err(|e| OntoError::internal_error(format!("删除关系类型失败: {e}")))?;
    emit_resource_changed(&tenant, "link", &api_name, "delete");
    Ok(Json(ApiResp::ok(json!({
        "apiName": api_name,
        "deleted": n > 0,
        "sceneRefs": scene_refs.iter().map(|(a, d)| json!({"apiName": a, "displayName": d})).collect::<Vec<_>>(),
        "affectedScenes": affected_scenes,
    }))))
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

pub async fn save_interface(Json(body): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let warnings = stripped_warnings(&body);
    let def: InterfaceDef = serde_json::from_value(body)
        .map_err(|e| OntoError::bad_request(format!("接口请求体非法: {e}")))?;
    let tenant = current_tenant();
    let version = save_interface_core(&tenant, def, None).await?;
    Ok(Json(ApiResp::ok(json!({ "saved": true, "version": version, "warnings": warnings }))))
}

pub(crate) async fn save_interface_core(
    tenant: &str,
    def: InterfaceDef,
    change_note: Option<&str>,
) -> Result<u32> {
    def.validate()
        .map_err(|e| OntoError::business_error(format!("接口非法: {e}")))?;
    let version = store()
        .save_interface_with_revision(&def, &changed_by(), change_note)
        .await
        .map_err(|e| match e {
            StoreError::Conflict(m) => OntoError::conflict(m),
            StoreError::NotFound(m) => OntoError::not_found(m),
            other => OntoError::internal_error(format!("保存接口失败: {other}")),
        })?;
    emit_resource_changed(tenant, "interface", &def.api_name, "save");
    Ok(version)
}

pub async fn delete_interface(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    ensure_deletable("interface", &api_name).await?;
    auto_snapshot_before("删除接口", &api_name).await;
    let n = store()
        .delete_interface(&tenant, &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("删除接口失败: {e}")))?;
    if n > 0 {
        store()
            .delete_with_revision("interface", &api_name, &changed_by(), Some("删除接口"))
            .await
            .map_err(|e| OntoError::internal_error(format!("接口墓碑修订失败: {e}")))?;
        emit_resource_changed(&tenant, "interface", &api_name, "delete");
    }
    Ok(Json(ApiResp::ok(json!({ "apiName": api_name, "deleted": n > 0 }))))
}

/// active 保护（§5.3）：active 资源不可删除（七类通用；对象/关系删除走带场景级联的重载流程）。
async fn ensure_deletable(kind: &str, api_name: &str) -> Result<()> {
    let status = store()
        .get_status(kind, api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载资源状态失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("{kind} {api_name} 不存在")))?;
    if status == TypeStatus::Active {
        return Err(OntoError::conflict(
            "active 资源不可删除，请先降级到 experimental / deprecated",
        ));
    }
    Ok(())
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

pub async fn save_shared_property(Json(body): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let warnings = stripped_warnings(&body);
    let def: SharedPropertyTypeDef = serde_json::from_value(body)
        .map_err(|e| OntoError::bad_request(format!("共享属性请求体非法: {e}")))?;
    let tenant = current_tenant();
    let version = save_shared_core(&tenant, def, None).await?;
    Ok(Json(ApiResp::ok(json!({ "saved": true, "version": version, "warnings": warnings }))))
}

pub(crate) async fn save_shared_core(
    tenant: &str,
    def: SharedPropertyTypeDef,
    change_note: Option<&str>,
) -> Result<u32> {
    def.validate()
        .map_err(|e| OntoError::business_error(format!("共享属性非法: {e}")))?;
    let version = store()
        .save_shared_with_revision(&def, &changed_by(), change_note)
        .await
        .map_err(|e| match e {
            StoreError::Conflict(m) => OntoError::conflict(m),
            StoreError::NotFound(m) => OntoError::not_found(m),
            other => OntoError::internal_error(format!("保存共享属性失败: {other}")),
        })?;
    emit_resource_changed(tenant, "shared_property", &def.api_name, "save");
    Ok(version)
}

/// DELETE /shared-properties/{apiName} —— 删除共享属性。
/// 安全网：①被引用（对象属性/接口契约）→ 409 出引用清单；②删除前自动存档（可撤销）。
pub async fn delete_shared_property(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    ensure_deletable("shared_property", &api_name).await?;
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
    if n > 0 {
        store()
            .delete_with_revision("shared_property", &api_name, &changed_by(), Some("删除共享属性"))
            .await
            .map_err(|e| OntoError::internal_error(format!("共享属性墓碑修订失败: {e}")))?;
        emit_resource_changed(&tenant, "shared_property", &api_name, "delete");
    }
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

pub async fn save_action_type(Json(body): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let warnings = stripped_warnings(&body);
    let mut def: ActionTypeDef = serde_json::from_value(body)
        .map_err(|e| OntoError::bad_request(format!("动作类型请求体非法: {e}")))?;
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
    // P2-0：派生作用对象类型并物化落列（语义真源仍是 parameters/logic；清单查询用）。
    let targets = cmx_onto_model::derive_target_object_types(&def.parameters, &def.logic);
    let version = save_action_core(&tenant, def, &targets, None).await?;
    Ok(Json(ApiResp::ok(json!({
        "saved": true,
        "version": version,
        "derivedParams": derived,
        "targetObjectTypes": targets,
        "warnings": warnings,
    }))))
}

pub(crate) async fn save_action_core(
    tenant: &str,
    def: ActionTypeDef,
    targets: &[String],
    change_note: Option<&str>,
) -> Result<u32> {
    let version = store()
        .save_action_with_revision(&def, targets, &changed_by(), change_note)
        .await
        .map_err(|e| match e {
            StoreError::Conflict(m) => OntoError::conflict(m),
            StoreError::NotFound(m) => OntoError::not_found(m),
            other => OntoError::internal_error(format!("保存动作类型失败: {other}")),
        })?;
    emit_resource_changed(tenant, "action", &def.api_name, "save");
    Ok(version)
}

pub async fn delete_action_type(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    ensure_deletable("action", &api_name).await?;
    auto_snapshot_before("删除动作类型", &api_name).await;
    let n = store()
        .delete_with_revision("action", &api_name, &changed_by(), Some("删除动作类型"))
        .await
        .map_err(|e| OntoError::internal_error(format!("删除动作类型失败: {e}")))?;
    if n > 0 {
        emit_resource_changed(&tenant, "action", &api_name, "delete");
    }
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

pub async fn save_function(Json(body): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let warnings = stripped_warnings(&body);
    let def: FunctionDef = serde_json::from_value(body)
        .map_err(|e| OntoError::bad_request(format!("函数请求体非法: {e}")))?;
    let tenant = current_tenant();
    let version = save_function_core(&tenant, def, None).await?;
    Ok(Json(ApiResp::ok(json!({ "saved": true, "version": version, "warnings": warnings }))))
}

pub(crate) async fn save_function_core(
    tenant: &str,
    def: FunctionDef,
    change_note: Option<&str>,
) -> Result<u32> {
    def.validate()
        .map_err(|e| OntoError::business_error(format!("函数非法: {e}")))?;
    let version = store()
        .save_function_with_revision(&def, &changed_by(), change_note)
        .await
        .map_err(|e| match e {
            StoreError::Conflict(m) => OntoError::conflict(m),
            StoreError::NotFound(m) => OntoError::not_found(m),
            other => OntoError::internal_error(format!("保存函数失败: {other}")),
        })?;
    emit_resource_changed(tenant, "function", &def.api_name, "save");
    Ok(version)
}

pub async fn delete_function(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    ensure_deletable("function", &api_name).await?;
    auto_snapshot_before("删除函数", &api_name).await;
    let n = store()
        .delete_with_revision("function", &api_name, &changed_by(), Some("删除函数"))
        .await
        .map_err(|e| OntoError::internal_error(format!("删除函数失败: {e}")))?;
    if n > 0 {
        emit_resource_changed(&tenant, "function", &api_name, "delete");
    }
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
    /// 状态分层过滤（D9，§5.5）：默认仅 active；逗号分隔 experimental/deprecated 或 all。
    pub include: Option<String>,
    /// 场景上下文（§7.3 六段口径）：成员按场景过滤；类型非成员 → 从清单消失。
    pub view: Option<String>,
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

/// GET /manifest —— 本体全量清单；`?types=` 按类型子集；`?include=` 状态分层（D9）；
/// `?view=` 场景六段口径（§7.3：成员类型 / 场景内关系 / 派生动作函数 / 派生共享属性）。
pub async fn manifest(Query(q): Query<ManifestQuery>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let filter = crate::filter::StatusFilter::parse(q.include.as_deref())?;
    let scope = crate::filter::SceneScope::resolve(&tenant, q.view.as_deref()).await?;
    let mut m: Value = match &q.types {
        None => serde_json::to_value(
            store()
                .manifest(&tenant)
                .await
                .map_err(|e| OntoError::internal_error(format!("装载清单失败: {e}")))?,
        )
        .unwrap_or(Value::Null),
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
            store()
                .manifest_filtered(&tenant, &kinds)
                .await
                .map_err(|e| OntoError::internal_error(format!("装载清单失败: {e}")))?
        }
    };
    filter_manifest(&mut m, &filter, scope.as_ref());
    Ok(Json(ApiResp::ok(m)))
}

/// 清单状态分层 + 场景六段口径过滤（§5.5 / §7.3；handler 层实现不下沉 store）。
fn filter_manifest(m: &mut Value, filter: &crate::filter::StatusFilter, scope: Option<&crate::filter::SceneScope>) {
    let Some(obj) = m.as_object_mut() else { return };
    // 1) 状态过滤（六段统一）。
    if let Some(arr) = obj.get_mut("objectTypes").and_then(|v| v.as_array_mut()) {
        arr.retain(|t| {
            t.get("status").and_then(|s| s.as_str()).map(|s| filter.allow_str(s)).unwrap_or(true)
        });
    }
    if let Some(arr) = obj.get_mut("linkTypes").and_then(|v| v.as_array_mut()) {
        arr.retain(|t| {
            t.get("status").and_then(|s| s.as_str()).map(|s| filter.allow_str(s)).unwrap_or(true)
        });
    }
    for seg in ["interfaces", "sharedProperties", "functions"] {
        if let Some(arr) = obj.get_mut(seg).and_then(|v| v.as_array_mut()) {
            arr.retain(|t| {
                t.get("status").and_then(|s| s.as_str()).map(|s| filter.allow_str(s)).unwrap_or(true)
            });
        }
    }
    if let Some(arr) = obj.get_mut("actionTypes").and_then(|v| v.as_array_mut()) {
        arr.retain(|t| {
            t.get("status").and_then(|s| s.as_str()).map(|s| filter.allow_str(s)).unwrap_or(true)
        });
    }
    // 2) 场景六段口径（scope 在状态过滤后取交集——场景不能豁免状态口径）。
    let Some(scope) = scope else { return };
    if let Some(arr) = obj.get_mut("objectTypes").and_then(|v| v.as_array_mut()) {
        arr.retain(|t| {
            t.get("apiName").and_then(|n| n.as_str()).map(|n| scope.objects.contains(n)).unwrap_or(false)
        });
    }
    if let Some(arr) = obj.get_mut("linkTypes").and_then(|v| v.as_array_mut()) {
        arr.retain(|t| {
            let name = t.get("apiName").and_then(|n| n.as_str()).unwrap_or("");
            match &scope.links {
                Some(links) => links.contains(name),
                None => {
                    // auto 视图：links 现算 = 两端在场（等价现状推导）。
                    let a = t.get("objectTypeA").and_then(|n| n.as_str()).unwrap_or("");
                    let b = t.get("objectTypeB").and_then(|n| n.as_str()).unwrap_or("");
                    scope.objects.contains(a) && scope.objects.contains(b)
                }
            }
        });
    }
    // interfaces = 成员对象 implements 并集 ∪（graph 对齐口径；members.interfaces 前端已知）。
    let implemented: std::collections::BTreeSet<String> = obj
        .get("objectTypes")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .flat_map(|t| {
                    t.get("implements").and_then(|x| x.as_array()).cloned().unwrap_or_default().into_iter()
                })
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if let Some(arr) = obj.get_mut("interfaces").and_then(|v| v.as_array_mut()) {
        arr.retain(|t| {
            t.get("apiName").and_then(|n| n.as_str()).map(|n| implemented.contains(n)).unwrap_or(false)
        });
    }
    // sharedProperties = 成员对象属性引用派生。
    let referenced: std::collections::BTreeSet<String> = obj
        .get("objectTypes")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .flat_map(|t| {
                    t.get("properties").and_then(|x| x.as_array()).cloned().unwrap_or_default().into_iter()
                })
                .filter_map(|p| p.get("sharedProperty").and_then(|s| s.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if let Some(arr) = obj.get_mut("sharedProperties").and_then(|v| v.as_array_mut()) {
        arr.retain(|t| {
            t.get("apiName").and_then(|n| n.as_str()).map(|n| referenced.contains(n)).unwrap_or(false)
        });
    }
    // actionTypes = 成员对象派生（targetObjectTypes ∩ 成员非空）。
    if let Some(arr) = obj.get_mut("actionTypes").and_then(|v| v.as_array_mut()) {
        arr.retain(|t| {
            t.get("targetObjectTypes")
                .and_then(|x| x.as_array())
                .map(|ts| {
                    ts.iter().filter_map(|x| x.as_str()).any(|x| scope.objects.contains(x))
                })
                .unwrap_or(false)
        });
    }
    // functions：无对象派生关系 → 空数组（§7.3 口径）。
    if let Some(arr) = obj.get_mut("functions").and_then(|v| v.as_array_mut()) {
        arr.clear();
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
