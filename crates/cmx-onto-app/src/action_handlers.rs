//! O4 动作引擎 · app 层 handler：执行动作（默认值 → 参数校验 → 对象装载 → 提交校验 →
//! [函数分支] → 编辑解析 → 组合校验 → PEP → 原子写回）+ dry-run + execute-batch + 审计查询。
//!
//! 执行链路（v2.1 方案定序）：装载定义 → 参数默认值 → 必填/约束校验 → 装载参数对象状态
//! → FEEL 提交校验（fail-closed）→（function_backing ? 函数求值产出编辑 : logic 解析编辑）
//! → 组合序列校验 → 写侧 PEP → 事务写回 + 审计 + 副作用 Outbox → dispatcher 投递。
//!
//! 双通道错误（P1-1）：`ActionErr` 拆 `msg`（面向用户）与 `adminDetail`（管理员调试），
//! 仅作用于动作执行端点；其余 handler 沿用 OntoError。

use crate::engine::store;
use crate::object_engine::{link_resolver, object_store};
use crate::resp::{ApiResp, OntoError, Result};
use crate::tenant::current_tenant;
use axum::extract::{Path, Query};
use axum::response::{IntoResponse, Response};
use axum::Json;
use cmx_onto_model::objectset::{ObjectSet, Page};
use cmx_onto_model::{
    apply_param_defaults, build_validation_ctx, edit_object_types, resolve_edits, resolve_side_effects,
    run_validations, validate_edit_sequence, validate_params, parse_function_result, ObjectEdit,
    ObjectStore, OntologyStore, SideEffect,
};
use cmx_onto_store_pg::{action_exec::edits_to_json, ActionExecutor, PolicyStore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use std::collections::BTreeMap;

/// 取当前租户的动作执行器。
fn action_executor() -> ActionExecutor {
    ActionExecutor::new(crate::tenancy::current_db_id())
}

// ───────────────────────── 双通道错误（P1-1） ─────────────────────────

/// 动作执行端点专用错误：`msg` 面向用户（validations message、缺参提示），
/// `adminDetail` 面向管理员（内部表达式、存储细节、执行日志）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionErr {
    status: u16,
    code: u16,
    msg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    admin_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_log: Option<Value>,
}

impl ActionErr {
    fn business(msg: impl Into<String>) -> Self {
        Self { status: 200, code: 1, msg: msg.into(), admin_detail: None, execution_log: None }
    }
    fn forbidden(msg: impl Into<String>) -> Self {
        Self { status: 403, code: 403, msg: msg.into(), admin_detail: None, execution_log: None }
    }
    fn internal(msg: impl Into<String>) -> Self {
        Self { status: 500, code: 500, msg: "动作执行失败，请联系管理员查看服务日志".into(), admin_detail: Some(msg.into()), execution_log: None }
    }
    fn admin(mut self, detail: impl Into<String>) -> Self {
        self.admin_detail = Some(detail.into());
        self
    }
    fn with_log(mut self, log: &[Value]) -> Self {
        self.execution_log = Some(Value::Array(log.to_vec()));
        self
    }
}

impl IntoResponse for ActionErr {
    fn into_response(self) -> Response {
        let status = axum::http::StatusCode::from_u16(self.status)
            .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(serde_json::to_value(&self).unwrap_or_default())).into_response()
    }
}

type ActionOutcome = std::result::Result<Response, ActionErr>;

// ───────────────────────── 参数对象装载器（P0-2） ─────────────────────────

/// 按参数声明装载 object / objectSet 参数对应的对象状态（壳层 IO；供校验上下文、
/// 值映射 paramProperty、函数 object 型入参三处共用）。
///
/// 返回 `{"<paramName>": 对象JSON 或 对象JSON数组}`；对象 JSON = `{pk, title, ...properties}`。
/// 参数缺 objectType / pk 为空 / 装载失败 → 该参数不注入（校验引用即失败，fail-closed）。
async fn load_param_objects(
    tenant: &str,
    action_params: &Value,
    params: &Value,
) -> Value {
    let Some(decls) = action_params.as_array() else { return json!({}) };
    let mut objects: Map<String, Value> = Map::new();
    for p in decls {
        let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let ty = p.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let ot = p.get("objectType").and_then(|v| v.as_str()).unwrap_or("");
        if name.is_empty() || ot.is_empty() || (ty != "object" && ty != "objectSet") {
            continue;
        }
        let raw = params.get(name);
        let pks: Vec<String> = match raw {
            None | Some(Value::Null) => continue,
            Some(Value::Array(a)) if ty == "objectSet" => a.iter().filter_map(scalar_pk).collect(),
            Some(v) => scalar_pk(v).map(|s| vec![s]).unwrap_or_default(),
        };
        if pks.is_empty() {
            continue;
        }
        let set = ObjectSet::Static { object_type: ot.to_string(), primary_keys: pks };
        let lr = link_resolver();
        let page = object_store()
            .load(tenant, &set, &Page { limit: 500, offset: 0 }, &lr)
            .await;
        let Ok(page) = page else { continue }; // 装载失败 → 不注入（fail-closed 由引用侧承担）
        let rows: Vec<Value> = page
            .rows
            .iter()
            .map(|r| {
                let mut o = Map::new();
                o.insert("pk".into(), json!(r.pk));
                o.insert("title".into(), json!(r.title));
                if let Some(props) = r.properties.as_object() {
                    for (k, v) in props {
                        o.insert(k.clone(), v.clone());
                    }
                }
                Value::Object(o)
            })
            .collect();
        match ty {
            "object" => {
                if let Some(first) = rows.into_iter().next() {
                    objects.insert(name.to_string(), first);
                }
            }
            _ => {
                objects.insert(name.to_string(), Value::Array(rows));
            }
        }
    }
    Value::Object(objects)
}

/// 标量 → pk 字符串。
fn scalar_pk(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// 装载编辑目标对象的当前值（P1-1 proposedChanges 的 from；delete 含完整快照）。
/// 返回 (objectType, pk) → 对象 JSON（未装载 = 不存在/新建）。
async fn load_edit_target_objects(
    tenant: &str,
    edits: &[ObjectEdit],
) -> BTreeMap<(String, String), Value> {
    // 按 (objectType) 聚合 pk 再装载，避免 N 次单查
    let mut wanted: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in edits {
        let (t, pk) = match e {
            ObjectEdit::ModifyObject { object_type, pk, .. }
            | ObjectEdit::UpsertObject { object_type, pk, .. }
            | ObjectEdit::DeleteObject { object_type, pk } => (object_type.clone(), pk.clone()),
            _ => continue,
        };
        let v = wanted.entry(t).or_default();
        if !v.contains(&pk) {
            v.push(pk);
        }
    }
    let mut out = BTreeMap::new();
    for (t, pks) in wanted {
        let set = ObjectSet::Static { object_type: t.clone(), primary_keys: pks };
        let lr = link_resolver();
        if let Ok(page) = object_store()
            .load(tenant, &set, &Page { limit: 500, offset: 0 }, &lr)
            .await
        {
            for r in page.rows {
                let mut o = Map::new();
                o.insert("pk".into(), json!(r.pk));
                o.insert("title".into(), json!(r.title));
                if let Some(props) = r.properties.as_object() {
                    for (k, v) in props {
                        o.insert(k.clone(), v.clone());
                    }
                }
                out.insert((t.clone(), r.pk), Value::Object(o));
            }
        }
    }
    out
}

/// 由编辑 + 当前值构建 proposedChanges（对象条目带 from→to diff；链接条目平铺）。
fn build_proposed_changes(
    edits: &[ObjectEdit],
    before: &BTreeMap<(String, String), Value>,
) -> Value {
    let arr: Vec<Value> = edits
        .iter()
        .map(|e| match e {
            ObjectEdit::CreateObject { object_type, pk, title, properties } => json!({
                "action": "create", "objectType": object_type, "pk": pk,
                "title": title, "properties": properties,
            }),
            ObjectEdit::UpsertObject { object_type, pk, set, .. } => {
                let cur = before.get(&(object_type.clone(), pk.clone()));
                json!({
                    "action": if cur.is_some() { "modify" } else { "create" },
                    "objectType": object_type, "pk": pk,
                    "diff": diff_props(cur, set),
                })
            }
            ObjectEdit::ModifyObject { object_type, pk, set } => {
                let cur = before.get(&(object_type.clone(), pk.clone()));
                json!({
                    "action": "modify", "objectType": object_type, "pk": pk,
                    "diff": diff_props(cur, set),
                })
            }
            ObjectEdit::DeleteObject { object_type, pk } => {
                let cur = before.get(&(object_type.clone(), pk.clone()));
                json!({
                    "action": "delete", "objectType": object_type, "pk": pk,
                    "title": cur.and_then(|c| c.get("title")).cloned().unwrap_or(Value::Null),
                    "snapshot": cur.cloned().unwrap_or(Value::Null),
                })
            }
            ObjectEdit::AddLink { link, a_pk, b_pk, properties } => json!({
                "action": "addLink", "link": link, "aPk": a_pk, "bPk": b_pk, "properties": properties,
            }),
            ObjectEdit::RemoveLink { link, a_pk, b_pk } => json!({
                "action": "removeLink", "link": link, "aPk": a_pk, "bPk": b_pk,
            }),
        })
        .collect();
    Value::Array(arr)
}

/// set 相对当前 props 的浅 diff：`{prop: {from, to}}`（仅列出有变化的属性）。
fn diff_props(cur: Option<&Value>, set: &Value) -> Value {
    let mut diff = Map::new();
    let Some(patch) = set.as_object() else { return Value::Object(diff) };
    for (k, to) in patch {
        let from = cur.and_then(|c| c.get(k)).cloned().unwrap_or(Value::Null);
        if from != *to {
            diff.insert(k.clone(), json!({ "from": from, "to": to }));
        }
    }
    Value::Object(diff)
}

// ───────────────────────── 执行核（execute / dry-run / batch 共用） ─────────────────────────

/// 执行请求体：{ params: {...}, dryRun?, actor?, subjects? }。
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

/// 一条执行链路解析结果（校验/解析全部通过后的中间产物）。
struct Resolved {
    edits: Vec<ObjectEdit>,
    effects: Vec<SideEffect>,
    log: Vec<Value>,
}

/// 执行链路解析段（不含 PEP / 写回）：默认值 → 参数校验 → 对象装载 → 提交校验 →
/// 函数分支或 logic 解析 → 组合序列校验。任一步失败返回 ActionErr（带执行日志）。
async fn resolve_action(
    tenant: &str,
    action: &cmx_onto_model::ActionTypeDef,
    req_params: &Value,
    actor: Option<&str>,
) -> std::result::Result<Resolved, ActionErr> {
    let mut log: Vec<Value> = vec![json!({ "stage": "loadDefinition", "ok": true })];

    // 1) 参数默认值补齐（P1-2）+ 必填/约束校验
    let mut params = req_params.clone();
    apply_param_defaults(action, &mut params);
    validate_params(action, &params)
        .map_err(|e| ActionErr::business(e).with_log(&log))?;
    log.push(json!({ "stage": "paramValidation", "ok": true }));

    // 2) 装载参数对象状态（object / objectSet 参数）
    let objects = load_param_objects(tenant, &action.parameters, &params).await;

    // 3) 提交校验（FEEL）：参数平铺 + params 别名 + objects.* 上下文；fail-closed
    let ctx = build_validation_ctx(&params, &objects);
    let fails = run_validations(action, &ctx);
    if !fails.is_empty() {
        let user = fails.iter().map(|f| f.message.clone()).collect::<Vec<_>>().join("；");
        let admin = fails
            .iter()
            .map(|f| format!("{} ⟶ {}", f.expression, f.message))
            .collect::<Vec<_>>()
            .join(" | ");
        return Err(ActionErr::business(format!("动作校验未通过（{} 项）：{user}", fails.len()))
            .admin(admin)
            .with_log(&log));
    }
    log.push(json!({ "stage": "submissionCriteria", "ok": true, "evaluated": action.validations.as_array().map(|a| a.len()).unwrap_or(0) }));

    // 4) 编辑解析：函数背书分支（Run function rule 最小闭环）或 logic 解析
    let (edits, effects) = if action.function_backing.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false) {
        let fname = action.function_backing.as_deref().unwrap_or("").trim();
        let func = store()
            .get_function(tenant, fname)
            .await
            .map_err(|e| ActionErr::internal(format!("装载函数 {fname} 失败: {e}")).with_log(&log))?
            .ok_or_else(|| ActionErr::business(format!("函数 {fname} 未定义")).with_log(&log))?;
        // 函数输入绑定：object/objectSet 型入参经装载器注入对象 JSON，其余取标量参数
        let mut bound = Map::new();
        for spec in cmx_onto_model::input_specs(&func) {
            let v = match spec.ty.as_str() {
                "object" | "objectSet" => objects.get(&spec.name).cloned(),
                _ => params.get(&spec.name).cloned(),
            };
            match v {
                Some(v) => {
                    bound.insert(spec.name, v);
                }
                None => {
                    return Err(ActionErr::business(format!(
                        "函数 {fname} 缺输入「{}」",
                        spec.name
                    ))
                    .with_log(&log))
                }
            }
        }
        let out = crate::function_runtime::eval_function_any(&func, &Value::Object(bound))
            .await
            .map_err(|e| {
                ActionErr::business(format!("函数 {fname} 求值失败：{e}"))
                    .admin(format!("runtime={:?}", func.runtime))
                    .with_log(&log)
            })?;
        let (edits, effects) = parse_function_result(&out).map_err(|e| {
            ActionErr::business(format!("函数 {fname} 返回不合契约：{e}"))
                .admin(format!("返回值: {out}"))
                .with_log(&log)
        })?;
        log.push(json!({ "stage": "functionEvaluation", "ok": true, "function": fname, "edits": edits.len(), "sideEffects": effects.len() }));
        (edits, effects)
    } else {
        let now = chrono::Utc::now().to_rfc3339();
        let edits = resolve_edits(action, &params, &objects, actor, Some(&now))
            .map_err(|e| ActionErr::business(format!("编辑规则解析失败：{e}")).with_log(&log))?;
        let effects = resolve_side_effects(action, &params);
        log.push(json!({ "stage": "editResolution", "ok": true, "edits": edits.len() }));
        (edits, effects)
    };

    // 5) 组合序列静态校验（P0-4 四规则）
    validate_edit_sequence(&edits).map_err(|e| ActionErr::business(format!("规则组合非法：{e}")).with_log(&log))?;
    log.push(json!({ "stage": "editSequenceCheck", "ok": true }));

    // 编辑与副作用皆空 → 空动作拒绝；仅副作用（如纯「起流程」/通知）也是合法可执行动作。
    if edits.is_empty() && effects.is_empty() {
        return Err(ActionErr::business(format!(
            "动作 {} 既无编辑规则也无副作用，无可执行内容",
            action.api_name
        ))
        .with_log(&log));
    }
    Ok(Resolved { edits, effects, log })
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

/// 写侧 PEP（O6 策略 deny_actions）：fail-closed 硬门。
async fn pep_check(
    target_types: &[String],
    api_name: &str,
    subjects: &[(String, String)],
    log: &[Value],
) -> std::result::Result<(), ActionErr> {
    if subjects.is_empty() {
        return Err(ActionErr::forbidden(format!(
            "动作 {api_name} 无法确定执行主体，拒绝执行（写侧硬门）"
        ))
        .with_log(log));
    }
    if let Some(denier) = PolicyStore::new(crate::tenancy::current_db_id())
        .check_action_permission(target_types, api_name, subjects)
        .await
        .map_err(|e| ActionErr::internal(format!("权限检查失败: {e}")).with_log(log))?
    {
        return Err(ActionErr::forbidden(format!(
            "动作 {api_name} 被策略「{denier}」拒绝执行（写侧 PEP）"
        ))
        .with_log(log));
    }
    Ok(())
}

fn ok_response(payload: Value) -> Response {
    (axum::http::StatusCode::OK, Json(ApiResp::ok(payload))).into_response()
}

/// POST /action-types/{api_name}/execute —— 执行动作（校验+编辑+原子写回；dryRun 只预演）。
pub async fn execute_action(
    Path(api_name): Path<String>,
    Json(req): Json<ExecuteReq>,
) -> ActionOutcome {
    let tenant = current_tenant();
    let action = store()
        .get_action_type(&tenant, &api_name)
        .await
        .map_err(|e| ActionErr::internal(format!("装载动作类型失败: {e}")))?
        .ok_or_else(|| ActionErr::business(format!("动作类型 {api_name} 未定义")))?;

    let actor = req.actor.clone().or_else(crate::tenant::current_user);
    let resolved = resolve_action(&tenant, &action, &req.params, actor.as_deref()).await?;
    let mut log = resolved.log;

    // PEP（作用域 = 编辑涉及的对象类型；内核同源口径，含 upsert）
    let subjects = subjects_of(&req.subjects);
    let targets = edit_object_types(&resolved.edits);
    pep_check(&targets, &api_name, &subjects, &log).await?;
    log.push(json!({ "stage": "pepCheck", "ok": true, "scopes": targets }));

    // 编辑目标当前值（proposedChanges 的 from；dry-run 预读、execute 亦预读供前端展示）
    let before = load_edit_target_objects(&tenant, &resolved.edits).await;
    let proposed = build_proposed_changes(&resolved.edits, &before);
    let preview: Vec<Value> = resolved
        .effects
        .iter()
        .map(|fx| json!({ "kind": fx.kind, "target": fx.target }))
        .collect();

    let outcome = action_executor()
        .apply(&api_name, &req.params, &resolved.edits, &resolved.effects, req.dry_run, actor.as_deref())
        .await
        .map_err(|e| ActionErr::internal(format!("执行动作失败: {e}")).with_log(&log))?;

    Ok(ok_response(json!({
        "action": api_name,
        "dryRun": req.dry_run,
        "applied": outcome.applied,
        "edits": edits_to_json(&resolved.edits),
        "effects": outcome.effects,
        "logId": outcome.log_id,
        "status": if req.dry_run { "dryRun" } else { "committed" },
        // —— P1-1 新增（与旧字段共存一个版本周期）——
        "proposedChanges": proposed,
        "executionLog": log,
        "sideEffectPreview": preview,
        // from 值为事务前预读快照（非事务提交快照；TOCTOU 窗口见方案 P1-1）
        "snapshotBasis": "preRead",
    })))
}

/// POST /action-types/{api_name}/dry-run —— 试算：完整校验链 + 预演，不落业务库（等价 execute + dryRun）。
pub async fn dry_run_action(
    Path(api_name): Path<String>,
    Json(mut req): Json<ExecuteReq>,
) -> ActionOutcome {
    req.dry_run = true;
    execute_action(Path(api_name), Json(req)).await
}

// ───────────────────────── 批量执行（P1-3） ─────────────────────────

/// 单批上限：env `ONTO_ACTION_BATCH_MAX` → ConfigManager `onto.action_batch_max_items` → 100。
pub fn batch_max_items() -> i64 {
    if let Ok(v) = std::env::var("ONTO_ACTION_BATCH_MAX")
        && let Ok(n) = v.trim().parse::<i64>()
            && n > 0 {
                return n;
            }
    if let Some(cm) = cmx_utils::ConfigManager::try_global()
        && let Ok(v) = cm.get_string("onto.action_batch_max_items")
            && let Ok(n) = v.trim().parse::<i64>()
                && n > 0 {
                    return n;
                }
    100
}

/// 批量执行请求体（固定路径 POST /action-types/execute-batch；apiName 入 body，无路径参数）。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct BatchExecuteReq {
    pub api_name: String,
    pub items: Vec<BatchItemReq>,
    pub dry_run: bool,
    pub actor: Option<String>,
    pub subjects: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct BatchItemReq {
    pub params: Value,
}

/// POST /action-types/execute-batch —— 同事务逐项提交，任一失败全回滚（失败批次单条 failed 审计）。
pub async fn execute_batch(Json(req): Json<BatchExecuteReq>) -> ActionOutcome {
    let tenant = current_tenant();
    let api_name = req.api_name.trim().to_string();
    if api_name.is_empty() {
        return Err(ActionErr::business("缺 apiName"));
    }
    if req.items.is_empty() {
        return Err(ActionErr::business("items 不能为空"));
    }
    if req.items.len() as i64 > batch_max_items() {
        return Err(ActionErr::business(format!(
            "批量项数 {} 超过单批上限 {}（ONTO_ACTION_BATCH_MAX / onto.action_batch_max_items），请分批提交",
            req.items.len(),
            batch_max_items()
        )));
    }
    let action = store()
        .get_action_type(&tenant, &api_name)
        .await
        .map_err(|e| ActionErr::internal(format!("装载动作类型失败: {e}")))?
        .ok_or_else(|| ActionErr::business(format!("动作类型 {api_name} 未定义")))?;
    let actor = req.actor.clone().or_else(crate::tenant::current_user);

    // 逐 item 走完整校验链；任一失败 → 整批拒绝（带 item 序号）
    let mut items: Vec<(Value, Vec<ObjectEdit>, Vec<SideEffect>)> = Vec::new();
    for (idx, it) in req.items.iter().enumerate() {
        let r = resolve_action(&tenant, &action, &it.params, actor.as_deref())
            .await
            .map_err(|e| {
                ActionErr::business(format!("批量第 {} 项被拒绝：{}", idx + 1, e.msg))
                    .admin(e.admin_detail.unwrap_or_default())
            })?;
        items.push((it.params.clone(), r.edits, r.effects));
    }

    // PEP：作用域 = 全部 item 编辑对象类型并集
    let subjects = subjects_of(&req.subjects);
    let all_edits: Vec<ObjectEdit> = items.iter().flat_map(|(_, e, _)| e.iter().cloned()).collect();
    let targets = edit_object_types(&all_edits);
    pep_check(&targets, &api_name, &subjects, &[]).await?;

    if req.dry_run {
        let before = load_edit_target_objects(&tenant, &all_edits).await;
        let proposed = build_proposed_changes(&all_edits, &before);
        return Ok(ok_response(json!({
            "action": api_name, "dryRun": true, "items": items.len(),
            "applied": all_edits.len(), "edits": edits_to_json(&all_edits),
            "effects": items.iter().map(|(_, _, fx)| fx.len()).sum::<usize>(),
            "status": "dryRun",
            "proposedChanges": proposed,
            "sideEffectPreview": items.iter().flat_map(|(_, _, fx)| fx.iter())
                .map(|f| json!({ "kind": f.kind, "target": f.target })).collect::<Vec<_>>(),
        })));
    }

    let outcome = action_executor()
        .apply_batch(&api_name, &items, actor.as_deref())
        .await
        .map_err(|e| ActionErr::business(format!("批量执行失败（已整体回滚）：{e}"))
            .admin(format!("apply_batch: {e}")))?;

    Ok(ok_response(json!({
        "action": api_name, "dryRun": false,
        "items": items.len(), "applied": outcome.applied,
        "edits": edits_to_json(&all_edits),
        "effects": outcome.effects, "logId": outcome.log_id,
        "status": "committed",
    })))
}

// ───────────────────────── 审计 / Outbox / 模板 / 投递（存量） ─────────────────────────

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

/// GET /action-outbox/config —— dispatcher 出站配置快照（运维/诊断；不含任何密钥）。
/// 返回 `{outboundEnabled, flowUrl, flowInstancesPath, webhookAllow}`。
pub async fn outbox_config() -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(crate::outbound::config_snapshot())))
}

/// GET /flow/definitions —— 代理 flowengine 已发布流程定义（设计台「触发流程」副作用选择器用）。
/// flow 不可达时**容错**返回空列表 + error（前端降级为自由输入 flowDefKey）。
pub async fn flow_definitions() -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    match crate::outbound::list_flow_definitions(&tenant).await {
        Ok(data) => {
            // flow 返回 data 可能是裸数组 [..] 或 {definitions:[..]}（信封已剥一层）——两者都归一。
            let arr = data
                .get("definitions")
                .and_then(|d| d.as_array())
                .or_else(|| data.as_array());
            let list: Vec<Value> = arr
                .map(|a| {
                    a.iter()
                        .map(|d| {
                            json!({
                                "key": d.get("key").and_then(|x| x.as_str()).unwrap_or(""),
                                "name": d.get("name").and_then(|x| x.as_str()).unwrap_or(""),
                            })
                        })
                        .filter(|d| !d["key"].as_str().unwrap_or("").is_empty())
                        .collect()
                })
                .unwrap_or_default();
            Ok(Json(ApiResp::ok(json!({ "definitions": list }))))
        }
        Err(e) => Ok(Json(ApiResp::ok(json!({ "definitions": [], "error": e })))),
    }
}

/// GET /report/definitions —— 代理 cmx-report 报表列表（设计台「生成报表」副作用选择器用）。
/// 容错：report 不可达返回空列表 + error（前端降级为自由输入 reportCode）。
pub async fn report_definitions() -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    match crate::outbound::list_reports(&tenant).await {
        Ok(data) => {
            // report 列表形如 {dbId, items:[..]}；兼容 reports/裸数组。item 字段 code/name。
            let arr = data
                .get("items")
                .and_then(|d| d.as_array())
                .or_else(|| data.get("reports").and_then(|d| d.as_array()))
                .or_else(|| data.as_array());
            let list: Vec<Value> = arr
                .map(|a| {
                    a.iter()
                        .map(|d| {
                            let code = d
                                .get("code")
                                .or_else(|| d.get("reportCode"))
                                .or_else(|| d.get("report_code"))
                                .and_then(|x| x.as_str())
                                .unwrap_or("");
                            let name = d
                                .get("name")
                                .or_else(|| d.get("reportName"))
                                .or_else(|| d.get("report_name"))
                                .and_then(|x| x.as_str())
                                .unwrap_or("");
                            json!({ "code": code, "name": name })
                        })
                        .filter(|d| !d["code"].as_str().unwrap_or("").is_empty())
                        .collect()
                })
                .unwrap_or_default();
            Ok(Json(ApiResp::ok(json!({ "reports": list }))))
        }
        Err(e) => Ok(Json(ApiResp::ok(json!({ "reports": [], "error": e })))),
    }
}

/// GET /action-templates —— 内置动作模板清单（前端「从模板新建动作」用；关账联动等预置组合）。
pub async fn action_templates() -> Result<Json<ApiResp<Value>>> {
    Ok(Json(ApiResp::ok(json!({ "templates": crate::action_templates::templates() }))))
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
/// `webhook`→真发 HTTP（受 host 白名单约束）；`startBusinessProcess`→调 cmx-flowengine v1 起实例。
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
            Ok(false) => { let _ = exec.mark_status(id, "deferred", Some("出站已熄火（ONTO_OUTBOUND=off）")).await; deferred += 1; }
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
            // payload 的字段作为求值上下文；无输入则纯求值 body。
            crate::function_runtime::eval_function_any(&func, payload)
                .await
                .map_err(|e| e.to_string())?;
            Ok(true)
        }
        // 外部投递（O4-M3 真投递）：webhook 真发 HTTP；startBusinessProcess 调 cmx-flowengine v1 起实例。
        // 跨微服务只经 HTTP（onto 不 path-dep flowengine）。全局熄火（ONTO_OUTBOUND=off）时回 deferred 挂起。
        "webhook" => {
            if !crate::outbound::outbound_enabled() {
                return Ok(false);
            }
            crate::outbound::post_webhook(target, payload).await.map(|_| true)
        }
        "startBusinessProcess" => {
            if !crate::outbound::outbound_enabled() {
                return Ok(false);
            }
            let iid = crate::outbound::start_business_process(tenant, target, payload).await?;
            tracing::info!(target = %target, instance = %iid, "startBusinessProcess 已投递");
            Ok(true)
        }
        // 触发报表计算 → 调 cmx-report compute（真算落 cr_cell_data）。
        "computeReport" => {
            if !crate::outbound::outbound_enabled() {
                return Ok(false);
            }
            crate::outbound::compute_report(tenant, target, payload).await?;
            tracing::info!(report = %target, "computeReport 已投递");
            Ok(true)
        }
        other => Err(format!("未知副作用类型 {other}")),
    }
}
