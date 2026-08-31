//! O4 动作引擎 · app 层 handler：执行动作（校验参数 → 解析编辑 → 原子写回）+ dry-run + 审计查询。
//!
//! 执行链路：装载 ActionTypeDef（定义层）→ validate_params → resolve_edits（内核，纯逻辑）
//! → ActionExecutor.apply（存储层，一事务全成或全败）→ 落 oe_action_log。
//! 判定校验（接规则引擎 FEEL）与副作用（接流程 Outbox）为 O4-M2/M3，此处先做执行核 + 审计。

use crate::engine::store;
use crate::resp::{ApiResp, OntoError, Result};
use crate::tenant::current_tenant;
use axum::extract::{Path, Query};
use axum::Json;
use cmx_onto_model::{resolve_edits, resolve_side_effects, run_validations, validate_params, ObjectEdit, OntologyStore};
use cmx_onto_store_pg::{action_exec::edits_to_json, ActionExecutor, PolicyStore};
use serde::Deserialize;
use serde_json::{json, Value};

/// 取当前租户的动作执行器。
fn action_executor() -> ActionExecutor {
    ActionExecutor::new(crate::tenancy::current_db_id())
}

/// 执行动作请求体：{ params: {...}, dryRun?, actor?, subjects? }。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ExecuteReq {
    pub params: Value,
    pub dry_run: bool,
    pub actor: Option<String>,
    /// 主体覆盖（`["role:teller","user:bob"]`；写侧 PEP 用）。auth off/单租户下调用方声明；
    /// jwt 模式忽略、以令牌为准。
    pub subjects: Vec<String>,
}

/// POST /action-types/{api_name}/execute —— 执行动作（校验+编辑+原子写回；dryRun 只预演）。
pub async fn execute_action(
    Path(api_name): Path<String>,
    Json(req): Json<ExecuteReq>,
) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let action = store()
        .get_action_type(&tenant, &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载动作类型失败: {e}")))?
        .ok_or_else(|| OntoError::business_error(format!("动作类型 {api_name} 未定义")))?;

    // 1) 必填参数校验
    validate_params(&action, &req.params).map_err(OntoError::business_error)?;
    // 2) 提交校验（FEEL validations）：以「参数展开 + params 别名」为上下文，fail-closed。
    let ctx = validation_ctx(&req.params);
    let fails = run_validations(&action, &ctx);
    if !fails.is_empty() {
        let detail = fails.iter().map(|f| f.message.clone()).collect::<Vec<_>>().join("；");
        return Err(OntoError::business_error(format!(
            "动作校验未通过（{} 项）：{detail}",
            fails.len()
        )));
    }
    // 3) 解析编辑（参数替换 → ObjectEdit 列表）
    let edits = resolve_edits(&action, &req.params).map_err(OntoError::business_error)?;
    if edits.is_empty() {
        return Err(OntoError::business_error(format!(
            "动作 {api_name} 无编辑规则（logic 为空），无可执行内容"
        )));
    }
    // 4) 解析副作用（参数替换 → SideEffect 列表；随编辑同事务入 Outbox）
    let effects = resolve_side_effects(&action, &req.params);
    // 4.5) 写侧 PEP（O6 策略 deny_actions）：主体对该动作/目标对象类型是否被拒。
    let subjects = subjects_of(&req.subjects);
    if !subjects.is_empty() {
        let target_types = edit_object_types(&edits);
        if let Some(denier) = PolicyStore::new(crate::tenancy::current_db_id())
            .check_action_permission(&target_types, &api_name, &subjects)
            .await
            .map_err(|e| OntoError::internal_error(format!("权限检查失败: {e}")))?
        {
            return Err(OntoError::business_error(format!(
                "动作 {api_name} 被策略「{denier}」拒绝执行（写侧 PEP）"
            )));
        }
    }
    // 5) 原子执行（或 dry-run 预演），落审计 + Outbox
    let outcome = action_executor()
        .apply(&api_name, &req.params, &edits, &effects, req.dry_run, req.actor.as_deref())
        .await
        .map_err(|e| OntoError::internal_error(format!("执行动作失败: {e}")))?;

    Ok(Json(ApiResp::ok(json!({
        "action": api_name,
        "dryRun": req.dry_run,
        "applied": outcome.applied,
        "edits": edits_to_json(&edits),
        "effects": outcome.effects,
        "logId": outcome.log_id,
        "status": if req.dry_run { "dryRun" } else { "committed" },
    }))))
}

/// POST /action-types/{api_name}/dry-run —— 试算：解析编辑并预演，不落业务库（等价 execute + dryRun）。
pub async fn dry_run_action(
    Path(api_name): Path<String>,
    Json(mut req): Json<ExecuteReq>,
) -> Result<Json<ApiResp<Value>>> {
    req.dry_run = true;
    execute_action(Path(api_name), Json(req)).await
}

/// 审计查询参数：?action=&limit=
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct LogQuery {
    pub action: Option<String>,
    pub limit: Option<i64>,
}

/// GET /action-logs —— 动作执行审计（最新在前；可 ?action= 过滤）。
pub async fn list_action_logs(Query(q): Query<LogQuery>) -> Result<Json<ApiResp<Value>>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let out = action_executor()
        .list_logs(q.action.as_deref(), limit)
        .await
        .map_err(|e| OntoError::internal_error(format!("查审计失败: {e}")))?;
    Ok(Json(ApiResp::ok(out)))
}

/// Outbox 查询参数：?status=&limit=
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct OutboxQuery {
    pub status: Option<String>,
    pub limit: Option<i64>,
}

/// GET /action-outbox —— 副作用 Outbox（最新在前；可 ?status=pending 过滤）。下游 dispatcher / 运维用。
pub async fn list_action_outbox(Query(q): Query<OutboxQuery>) -> Result<Json<ApiResp<Value>>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let out = action_executor()
        .list_outbox(q.status.as_deref(), limit)
        .await
        .map_err(|e| OntoError::internal_error(format!("查 Outbox 失败: {e}")))?;
    Ok(Json(ApiResp::ok(out)))
}

/// 标记投递请求体：{ ok, error? }。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct MarkDispatchReq {
    pub ok: bool,
    pub error: Option<String>,
}

/// POST /action-outbox/{id}/dispatched —— dispatcher 投递后回标（ok=true→dispatched；false→failed）。
pub async fn mark_outbox_dispatched(
    Path(id): Path<i64>,
    Json(req): Json<MarkDispatchReq>,
) -> Result<Json<ApiResp<Value>>> {
    let n = action_executor()
        .mark_dispatched(id, req.ok, req.error.as_deref())
        .await
        .map_err(|e| OntoError::internal_error(format!("标记 Outbox 失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "id": id, "updated": n > 0 }))))
}

/// 派发参数：?limit=
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchQuery {
    pub limit: Option<i64>,
}

/// POST /action-outbox/dispatch —— 抽取 pending 副作用并**真投递**（O4-M3 dispatcher）。
///
/// 按 kind 分派：`emitEvent`→SSE 事件流（O7）；`callFunction`→O5 函数求值；`notification`→SSE 通知；
/// `webhook`/`startBusinessProcess`→deferred（外部投递需 URL/flow 配置，M1 未接则挂起）。
pub async fn dispatch_outbox(Query(q): Query<DispatchQuery>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let jobs = action_executor()
        .fetch_pending(limit)
        .await
        .map_err(|e| OntoError::internal_error(format!("领取 Outbox 失败: {e}")))?;
    let exec = action_executor();
    let mut dispatched = 0u32;
    let mut deferred = 0u32;
    let mut failed = 0u32;
    for (id, kind, target, payload) in jobs {
        let outcome = dispatch_one(&tenant, &kind, &target, &payload).await;
        match outcome {
            Ok(true) => { let _ = exec.mark_status(id, "dispatched", None).await; dispatched += 1; }
            Ok(false) => { let _ = exec.mark_status(id, "deferred", Some("外部投递未配置（webhook URL / flow）")).await; deferred += 1; }
            Err(e) => { let _ = exec.mark_status(id, "failed", Some(&e)).await; failed += 1; }
        }
    }
    Ok(Json(ApiResp::ok(json!({
        "dispatched": dispatched, "deferred": deferred, "failed": failed,
        "total": dispatched + deferred + failed
    }))))
}

/// 投递单条副作用。Ok(true)=已投递；Ok(false)=挂起（外部未配置）；Err=失败。
async fn dispatch_one(tenant: &str, kind: &str, target: &str, payload: &Value) -> std::result::Result<bool, String> {
    match kind {
        // 发事件 → O7 SSE 变更流（进程内真投递）
        "emitEvent" => {
            crate::events::emit(tenant, target, payload.clone());
            Ok(true)
        }
        // 通知 → SSE notification 事件（进程内）
        "notification" => {
            crate::events::emit(tenant, "notification", json!({ "template": target, "payload": payload }));
            Ok(true)
        }
        // 调函数 → O5 求值（进程内真投递）
        "callFunction" => {
            let func = crate::engine::store()
                .get_function(tenant, target)
                .await
                .map_err(|e| format!("装载函数失败: {e}"))?
                .ok_or_else(|| format!("函数 {target} 未定义"))?;
            // payload 的字段作为 FEEL 上下文；无输入则纯求值 body。
            cmx_onto_model::evaluate_function(&func, payload).map_err(|e| e.to_string())?;
            Ok(true)
        }
        // 外部投递：webhook / 触发流程 —— M1 未接（无 URL/flow 配置）→ 挂起。
        "webhook" | "startBusinessProcess" => Ok(false),
        other => Err(format!("未知副作用类型 {other}")),
    }
}

/// 校验上下文：既把参数平铺到顶层（`amount > 0`），也保留 `params.*` 前缀（`params.amount > 0`）。
fn validation_ctx(params: &Value) -> Value {
    let mut ctx = serde_json::Map::new();
    if let Some(m) = params.as_object() {
        for (k, v) in m {
            ctx.insert(k.clone(), v.clone());
        }
    }
    ctx.insert("params".to_string(), params.clone());
    Value::Object(ctx)
}

/// 请求 subjects（`["role:x","user:y"]`）→ (kind,subject) 列表；空则回退上下文（role:tenant + user）。
fn subjects_of(req_subjects: &[String]) -> Vec<(String, String)> {
    if !req_subjects.is_empty() {
        return req_subjects
            .iter()
            .filter_map(|s| s.split_once(':').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect();
    }
    let mut subs = vec![("role".to_string(), current_tenant())];
    if let Some(u) = crate::tenant::current_user() {
        subs.push(("user".to_string(), u));
    }
    subs
}

/// 从编辑列表提取涉及的对象类型（PEP 作用域）。
fn edit_object_types(edits: &[ObjectEdit]) -> Vec<String> {
    let mut out = Vec::new();
    for e in edits {
        let ot = match e {
            ObjectEdit::CreateObject { object_type, .. }
            | ObjectEdit::ModifyObject { object_type, .. }
            | ObjectEdit::DeleteObject { object_type, .. } => Some(object_type.clone()),
            _ => None,
        };
        if let Some(t) = ot {
            if !out.contains(&t) {
                out.push(t);
            }
        }
    }
    out
}
