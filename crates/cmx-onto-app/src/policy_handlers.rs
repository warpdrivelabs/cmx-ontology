//! O6 动态安全 · app 层：策略 CRUD + 「带安全的对象集加载」。
//!
//! 主体（subject）来自请求上下文：`role:<tenant>`（M1 用租户名当角色占位，jwt 模式可扩真实角色）
//! + `user:<current_user>`。匹配 (objectType, 主体集) 的 active 策略 → 合并行级残差 Filter（编译期生效）
//! + 收集 deny_markings → 对返回行按对象类型定义的属性 marking 脱敏。

use crate::engine::store;
use crate::resp::{ApiResp, OntoError, Result};
use crate::tenant::{current_tenant, current_user};
use axum::extract::Path;
use axum::Json;
use cmx_onto_model::objectset::{ObjectSet, Page};
use cmx_onto_model::{redact_rows, residual_set, OntologyStore};
use cmx_onto_store_pg::PolicyStore;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

fn policy_store() -> PolicyStore {
    PolicyStore::new(crate::tenancy::current_db_id())
}

/// 当前请求主体集：role:<tenant> + user:<user>（M1 占位；jwt 模式补真实角色列表）。
fn current_subjects() -> Vec<(String, String)> {
    let mut subs = vec![("role".to_string(), current_tenant())];
    if let Some(u) = current_user() {
        subs.push(("user".to_string(), u));
    }
    subs
}

// ————————————————————— 策略 CRUD —————————————————————

/// GET /policies —— 列出全部策略。
#[utoipa::path(
    get,
    path = "/api/onto/v1/policies",
    tag = "安全",
    summary = "列出全部策略",
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn list_policies() -> Result<Json<ApiResp<Value>>> {
    let out = policy_store()
        .list()
        .await
        .map_err(|e| OntoError::internal_error(format!("查策略失败: {e}")))?;
    Ok(Json(ApiResp::ok(out)))
}

/// POST /policies —— upsert 一条策略。
#[utoipa::path(
    post,
    path = "/api/onto/v1/policies",
    tag = "安全",
    summary = "新建/更新策略（行级过滤 + 列脱敏 + 动作拒绝）",
    request_body(content = Value, description = "策略定义"),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn upsert_policy(Json(body): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let api_name = policy_store()
        .upsert(&body)
        .await
        .map_err(|e| OntoError::business_error(format!("写策略失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "apiName": api_name, "saved": true }))))
}

/// DELETE /policies/{api_name} —— 删除策略。
#[utoipa::path(
    delete,
    path = "/api/onto/v1/policies/{api_name}",
    tag = "安全",
    summary = "删除策略",
    params(
        ("api_name" = String, Path, description = "资源 API 名"),
    ),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn delete_policy(Path(api_name): Path<String>) -> Result<Json<ApiResp<Value>>> {
    let n = policy_store()
        .delete(&api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("删策略失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "apiName": api_name, "deleted": n > 0 }))))
}

// ————————————————————— 带安全的加载 —————————————————————

/// 对象集加载请求体（复用 O2 结构）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecureLoadReq {
    pub object_set: ObjectSet,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    /// 主体覆盖（`["role:east","user:bob"]`）。auth off/单租户下由调用方声明主体；
    /// jwt/多租户模式忽略此字段、以令牌真实身份为准（防越权）。
    #[serde(default)]
    pub subjects: Vec<String>,
    /// 场景上下文（§7.3；与 /object-sets/load 同规则——统一口径不留旁路）。
    #[serde(default)]
    pub view: Option<String>,
    /// 状态分层（D9，§5.5；与 /load 同参数）。
    #[serde(default)]
    pub include: Option<String>,
}

/// POST /secure/object-sets/load —— 按当前主体的策略加载对象集（行级残差 + 列级脱敏）。
#[utoipa::path(
    post,
    path = "/api/onto/v1/secure/object-sets/load",
    tag = "安全",
    summary = "显式带安全加载（同 /object-sets/load，响应多 appliedPolicies/subjects）",
    request_body(content = Value, description = "对象集代数 + 分页 + 主体"),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn secure_load(Json(req): Json<SecureLoadReq>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    // 场景 + 状态过滤（先于 PEP；与 /object-sets/load 同规则同顺序）。
    let scope = crate::filter::SceneScope::resolve(&tenant, req.view.as_deref()).await?;
    let filter = crate::filter::StatusFilter::parse(req.include.as_deref())?;
    if let Some(scope) = &scope {
        scope.ensure_set_allowed(&req.object_set)?;
    }
    let terminal_pre = req.object_set.terminal_object_type().unwrap_or("").to_string();
    if !terminal_pre.is_empty() {
        crate::filter::ensure_object_queryable(&tenant, &terminal_pre, &filter).await?;
    }
    // 主体：优先请求显式声明（dev/off 模式）；否则从上下文推导（role:tenant + user:current_user）。
    let subjects = if req.subjects.is_empty() {
        current_subjects()
    } else {
        req.subjects
            .iter()
            .filter_map(|s| s.split_once(':').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect()
    };
    let terminal = req.object_set.terminal_object_type().unwrap_or("").to_string();

    // 匹配策略 → 合并行级残差 + 收集 deny markings
    let policies = policy_store()
        .match_policies(&terminal, &subjects)
        .await
        .map_err(|e| OntoError::internal_error(format!("匹配策略失败: {e}")))?;
    let mut residuals = Vec::new();
    let mut deny_markings: Vec<String> = Vec::new();
    let mut applied: Vec<String> = Vec::new();
    for p in &policies {
        residuals.extend(p.row_filter.clone());
        for m in &p.deny_markings {
            if !deny_markings.contains(m) {
                deny_markings.push(m.clone());
            }
        }
        applied.push(p.api_name.clone());
    }

    let secured_set = residual_set(req.object_set.clone(), residuals);
    let page = Page {
        limit: req.limit.unwrap_or(100),
        offset: req.offset.unwrap_or(0),
    };
    // 读路径分派：虚拟类型同样受策略门约束（残差折入后随下推编译生效，E7/R3）。
    let mut page_out = crate::backend_dispatcher::load(&tenant, &secured_set, &page)
        .await
        .map_err(|e| OntoError::internal_error(format!("加载对象集失败: {e}")))?;

    // 列级脱敏：取终端类型属性 marking 表
    if !deny_markings.is_empty() && !terminal.is_empty() {
        let marking_by_prop = marking_map(&tenant, &terminal).await;
        redact_rows(&mut page_out.rows, &deny_markings, &marking_by_prop);
    }

    let mut out = serde_json::to_value(&page_out).unwrap_or(json!({}));
    if let Value::Object(m) = &mut out {
        m.insert("appliedPolicies".to_string(), json!(applied));
        m.insert(
            "subjects".to_string(),
            json!(subjects.iter().map(|(k, s)| format!("{k}:{s}")).collect::<Vec<_>>()),
        );
    }
    Ok(Json(ApiResp::ok(out)))
}

/// 取对象类型的属性 marking 表（apiName → marking）。
async fn marking_map(tenant: &str, object_type: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Ok(Some(def)) = store().get_object_type(tenant, object_type).await {
        for p in &def.properties {
            if let Some(mk) = &p.marking
                && !mk.is_empty() {
                    map.insert(p.api_name.clone(), mk.clone());
                }
        }
    }
    map
}
