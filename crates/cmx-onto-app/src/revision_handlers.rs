//! 资源级修订历史（方案 20260917 §6.2/§6.3）——时间线 / 详情 / revert（git revert 式）。
//!
//! revert = 以旧 payload 执行一次**新保存**并产生新修订（历史只追加不改写）；走全部保存校验。
//! 对带墓碑的已删除资源，revert 自动升级为创建语义（等价恢复该资源）——US-B3 的实现载体。

use crate::engine::store;
use crate::resp::{ApiResp, OntoError, Result};
use crate::tenant::{current_display_user, current_tenant};
use axum::extract::Query;
use axum::Json;
use cmx_onto_model::{
    ActionTypeDef, FunctionDef, InterfaceDef, LinkTypeDef, ObjectTypeDef, SharedPropertyTypeDef,
    TypeStatus,
};
use serde::Deserialize;
use serde_json::{json, Value};

/// GET /revisions 查询参数。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct RevisionsQuery {
    pub kind: Option<String>,
    pub api_name: Option<String>,
    pub limit: Option<u32>,
    /// true = 列出带墓碑的已删除资源清单（kind 可省略，全库扫描）。
    pub deleted: Option<bool>,
}

/// GET /revisions —— 单资源修订时间线；`deleted=true` 时列出已删除资源。
pub async fn list_revisions(Query(q): Query<RevisionsQuery>) -> Result<Json<ApiResp<Value>>> {
    let s = store();
    if q.deleted.unwrap_or(false) {
        let rows = s
            .list_deleted_resources(q.kind.as_deref())
            .await
            .map_err(|e| OntoError::internal_error(format!("查询已删除资源失败: {e}")))?;
        return Ok(Json(ApiResp::ok(json!(rows))));
    }
    let (Some(kind), Some(api_name)) = (q.kind.as_deref(), q.api_name.as_deref()) else {
        return Err(OntoError::bad_request(
            "缺参数：kind 与 apiName 必填（或 deleted=true 列出已删除资源）",
        ));
    };
    let rows = s
        .list_revisions(kind, api_name, q.limit.unwrap_or(100), None)
        .await
        .map_err(|e| OntoError::internal_error(format!("查询修订历史失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!(rows))))
}

/// GET /revisions/detail 查询参数。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionDetailQuery {
    pub id: i64,
}

/// GET /revisions/detail?id= —— 单条修订详情（含 payload）。
pub async fn revision_detail(Query(q): Query<RevisionDetailQuery>) -> Result<Json<ApiResp<Value>>> {
    let detail = store()
        .get_revision_detail(q.id)
        .await
        .map_err(|e| OntoError::internal_error(format!("查询修订详情失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("修订 #{} 不存在", q.id)))?;
    Ok(Json(ApiResp::ok(detail)))
}

/// POST /revisions/revert 请求体。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevertReq {
    pub kind: String,
    pub api_name: String,
    pub revision: i64,
    #[serde(default)]
    pub change_note: Option<String>,
}

/// POST /revisions/revert —— 以旧定义执行一次新保存（历史只追加）；
/// 资源已删除（墓碑）时升级为创建语义（恢复该资源）。
pub async fn revert(Json(req): Json<RevertReq>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let kind = req.kind.trim();
    if !cmx_onto_store_pg::revision_store::REVISION_KINDS.contains(&kind) {
        return Err(OntoError::bad_request(format!("kind 非法：{kind:?}")));
    }
    let detail = store()
        .get_revision_detail(req.revision)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载修订失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("修订 #{} 不存在", req.revision)))?;
    // 校验修订归属（kind/apiName 与修订行一致，防错位恢复）。
    let d_kind = detail.get("resourceKind").and_then(|v| v.as_str()).unwrap_or("");
    let d_name = detail.get("apiName").and_then(|v| v.as_str()).unwrap_or("");
    let deleted = detail.get("deleted").and_then(|v| v.as_bool()).unwrap_or(false);
    if d_kind != kind || d_name != req.api_name {
        return Err(OntoError::bad_request(format!(
            "修订 #{} 属于 {d_kind} {d_name}，与请求 {kind} {} 不符",
            req.revision, req.api_name
        )));
    }
    let payload = detail.get("payload").cloned().unwrap_or(Value::Null);
    if payload.is_null() {
        return Err(OntoError::business_error("修订 payload 为空，无法回滚"));
    }
    let note = req
        .change_note
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| {
            if deleted {
                format!("恢复自修订 #{}", req.revision)
            } else {
                format!("回滚自修订 #{}", req.revision)
            }
        });

    // 资源现存性：墓碑 / 不存在 → 创建语义（仍走完整保存校验）。
    let exists = store()
        .get_status(kind, &req.api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载资源失败: {e}")))?
        .is_some();

    let version = revert_apply(&tenant, kind, &req.api_name, &payload, &note, exists).await?;
    crate::events::emit(
        &tenant,
        "resource-changed",
        json!({
            "kind": kind,
            "apiName": req.api_name,
            "action": if exists { "revert" } else { "restore" },
            "revision": req.revision,
            "by": current_display_user(),
        }),
    );
    Ok(Json(ApiResp::ok(json!({
        "kind": kind,
        "apiName": req.api_name,
        "revertedToRevision": req.revision,
        "restored": !exists,
        "version": version,
        "changeNote": note,
    }))))
}

/// payload → 类型化定义 → 走保存管道（全部校验 + 同事务修订）。
async fn revert_apply(
    tenant: &str,
    kind: &str,
    api_name: &str,
    payload: &Value,
    note: &str,
    exists: bool,
) -> Result<u32> {
    let s = store();
    let changed_by = current_display_user().unwrap_or_else(|| "anonymous".into());
    // 已存资源 revert：保留 live 的乐观锁版本号（条件更新链不断）；创建语义 version=0 盲写。
    let live_version = |has: bool, def_ver: u32| {
        if exists && has { def_ver } else { 0 }
    };
    match kind {
        "object" => {
            let mut def: ObjectTypeDef = serde_json::from_value(payload.clone())
                .map_err(|e| OntoError::business_error(format!("修订 payload 无法解析为对象类型: {e}")))?;
            if def.api_name != api_name {
                return Err(OntoError::bad_request("修订 payload apiName 与请求不符"));
            }
            def.version = live_version(exists, def.version);
            crate::handlers::save_object_core(tenant, def, Some(note)).await
        }
        "link" => {
            let mut def: LinkTypeDef = serde_json::from_value(payload.clone())
                .map_err(|e| OntoError::business_error(format!("修订 payload 无法解析为关系类型: {e}")))?;
            if def.api_name != api_name {
                return Err(OntoError::bad_request("修订 payload apiName 与请求不符"));
            }
            def.version = live_version(exists, def.version);
            crate::handlers::save_link_core(tenant, def, Some(note)).await
        }
        "interface" => {
            let mut def: InterfaceDef = serde_json::from_value(payload.clone())
                .map_err(|e| OntoError::business_error(format!("修订 payload 无法解析为接口: {e}")))?;
            def.api_name = api_name.to_string();
            def.version = live_version(exists, def.version);
            crate::handlers::save_interface_core(tenant, def, Some(note)).await
        }
        "shared_property" => {
            let mut def: SharedPropertyTypeDef = serde_json::from_value(payload.clone())
                .map_err(|e| OntoError::business_error(format!("修订 payload 无法解析为共享属性: {e}")))?;
            def.api_name = api_name.to_string();
            def.version = live_version(exists, def.version);
            crate::handlers::save_shared_core(tenant, def, Some(note)).await
        }
        "action" => {
            let mut def: ActionTypeDef = serde_json::from_value(payload.clone())
                .map_err(|e| OntoError::business_error(format!("修订 payload 无法解析为动作类型: {e}")))?;
            def.api_name = api_name.to_string();
            def.version = live_version(exists, def.version);
            {
            let targets = cmx_onto_model::derive_target_object_types(&def.parameters, &def.logic);
            crate::handlers::save_action_core(tenant, def, &targets, Some(note)).await
        }
        }
        "function" => {
            let mut def: FunctionDef = serde_json::from_value(payload.clone())
                .map_err(|e| OntoError::business_error(format!("修订 payload 无法解析为函数: {e}")))?;
            def.api_name = api_name.to_string();
            def.version = live_version(exists, def.version);
            crate::handlers::save_function_core(tenant, def, Some(note)).await
        }
        "view" => {
            let mut def: cmx_onto_model::SceneViewDef = serde_json::from_value(payload.clone())
                .map_err(|e| OntoError::business_error(format!("修订 payload 无法解析为场景: {e}")))?;
            def.api_name = api_name.to_string();
            def.version = if exists {
                // 修订 payload 剥离了 layout：恢复时保留 live 布局。
                let live = s
                    .get_view(tenant, api_name)
                    .await
                    .map_err(internal("装载场景失败"))?;
                match live {
                    Some(l) => {
                        def.layout = l.layout;
                        l.version
                    }
                    None => 0,
                }
            } else {
                0
            };
            crate::view_handlers::save_view_core(tenant, def, &changed_by, Some(note)).await
        }
        other => Err(OntoError::bad_request(format!("未知资源类别 {other}"))),
    }
}

fn internal(ctx: &'static str) -> impl Fn(cmx_onto_model::StoreError) -> OntoError {
    move |e| OntoError::internal_error(format!("{ctx}: {e}"))
}

/// 状态枚举的 re-export（保持模块自洽；供后续扩展校验）。
#[allow(dead_code)]
type _StatusUse = TypeStatus;
