//! O5 函数计算引擎 · app 层 handler：绑定输入（标量/对象/对象集）→ 求值函数体（FEEL）。
//!
//! 输入解析（`inputs:[{name,type}]`）：
//! - 标量：从请求 `args.<name>` 直取；
//! - `object`：请求给 `{objectType, pk}` → 从 `oo_<type>` 加载该对象的 properties 注入；
//! - `objectSet`：请求给对象集代数 → load 后把「行属性数组」注入（FEEL 可 sum/count/for）。
//! Aggregation 用途：请求给对象集 + 聚合规格 → 走存储层 aggregate（对齐 O2）。

use crate::engine::store;
use crate::object_engine::{link_resolver, object_store};
use crate::resp::{ApiResp, OntoError, Result};
use crate::tenant::current_tenant;
use axum::extract::Path;
use axum::Json;
use cmx_onto_model::objectset::{Aggregation, ObjectSet, Page};
use cmx_onto_model::{input_specs, FunctionKind, ObjectStore, OntologyStore};
use serde::Deserialize;
use serde_json::{json, Map, Value};

/// 求值函数请求体：
/// { args?: {标量参数}, objects?: {name:{objectType,pk}}, objectSets?: {name:<代数>}, aggregation?: <规格> }
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct EvalFnReq {
    /// 标量参数（直接注入 ctx）。
    pub args: Value,
    /// object 类型输入：参数名 → { objectType, pk }。
    pub objects: Value,
    /// objectSet 类型输入：参数名 → 对象集代数。
    pub object_sets: Value,
    /// Aggregation 用途的聚合规格（配合首个 objectSet 输入或 objectSet 字段）。
    pub aggregation: Option<Aggregation>,
    /// Aggregation 用途的对象集（当不走 objectSets 时）。
    pub object_set: Option<ObjectSet>,
}

/// POST /functions/{api_name}/evaluate —— 求值一个函数。
#[utoipa::path(
    post,
    path = "/api/onto/v1/functions/{api_name}/evaluate",
    tag = "函数",
    summary = "函数求值：绑定输入 → 执行函数体 → 返回结果",
    params(
        ("api_name" = String, Path, description = "资源 API 名"),
    ),
    request_body(content = Value, description = "args/objects/objectSets/aggregation/objectSet"),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn evaluate_fn(
    Path(api_name): Path<String>,
    Json(req): Json<EvalFnReq>,
) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let func = store()
        .get_function(&tenant, &api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载函数失败: {e}")))?
        .ok_or_else(|| OntoError::business_error(format!("函数 {api_name} 未定义")))?;

    // Aggregation 用途：走存储层聚合（对齐 O2 object-sets/aggregate）。
    if matches!(func.kind, FunctionKind::Aggregation) {
        let set = req
            .object_set
            .clone()
            .ok_or_else(|| OntoError::business_error("聚合函数须提供 objectSet".to_string()))?;
        let agg = req
            .aggregation
            .clone()
            .ok_or_else(|| OntoError::business_error("聚合函数须提供 aggregation".to_string()))?;
        // E3 内部读路径守卫：virtual 类型一律 4xx（防静默空集假结果）。
        crate::backend_dispatcher::ensure_set_internal_loadable(&tenant, &set).await?;
        let out = object_store()
            .aggregate(&tenant, &set, &agg, &link_resolver())
            .await
            .map_err(|e| OntoError::internal_error(format!("聚合失败: {e}")))?;
        return Ok(Json(ApiResp::ok(json!({
            "function": api_name, "kind": "aggregation", "result": out
        }))));
    }

    // 其余用途（Query/DerivedProperty/Validation/ActionLogic）：绑定输入 → FEEL 求值。
    let mut ctx = Map::new();
    // 标量参数
    if let Some(m) = req.args.as_object() {
        for (k, v) in m {
            ctx.insert(k.clone(), v.clone());
        }
    }
    // object 输入：加载对象 properties
    if let Some(m) = req.objects.as_object() {
        for (name, spec) in m {
            let object_type = spec.get("objectType").and_then(|v| v.as_str()).unwrap_or("");
            let pk = spec.get("pk").and_then(scalar_str).unwrap_or_default();
            if object_type.is_empty() || pk.is_empty() {
                return Err(OntoError::business_error(format!(
                    "object 输入「{name}」须给 objectType 与 pk"
                )));
            }
            let set = ObjectSet::Static {
                object_type: object_type.to_string(),
                primary_keys: vec![pk.clone()],
            };
            // E3：virtual 类型内部装载拒绝（单对象同样静默空集）。
            crate::backend_dispatcher::ensure_set_internal_loadable(&tenant, &set).await?;
            let page = object_store()
                .load(&tenant, &set, &Page { limit: 1, offset: 0 }, &link_resolver())
                .await
                .map_err(|e| OntoError::internal_error(format!("加载 object 输入失败: {e}")))?;
            let props = page
                .rows
                .into_iter()
                .next()
                .map(|r| r.properties)
                .unwrap_or(Value::Null);
            ctx.insert(name.clone(), props);
        }
    }
    // objectSet 输入：加载行、把 properties 数组注入（FEEL 可聚合/推导）
    if let Some(m) = req.object_sets.as_object() {
        for (name, raw_set) in m {
            let set: ObjectSet = serde_json::from_value(raw_set.clone())
                .map_err(|e| OntoError::business_error(format!("objectSet 输入「{name}」非法：{e}")))?;
            // E3：virtual 类型内部装载拒绝。
            crate::backend_dispatcher::ensure_set_internal_loadable(&tenant, &set).await?;
            // B-P1-9 截断 bug 修复：store 层 clamp(1,1000) 会把 limit=10000 静默截到 1000——
            // 改为分页循环拉取（上限 10000 或取尽即止），注入行数不再悄悄丢 90%。
            let os = object_store();
            let lr = link_resolver();
            let mut rows: Vec<Value> = Vec::new();
            let (mut offset, page_size) = (0u32, 1000u32);
            loop {
                let page = os
                    .load(&tenant, &set, &Page { limit: page_size, offset }, &lr)
                    .await
                    .map_err(|e| OntoError::internal_error(format!("加载 objectSet 输入失败: {e}")))?;
                let got = page.rows.len();
                rows.extend(page.rows.into_iter().map(|r| r.properties));
                let exhausted = got < page_size as usize || !page.has_more;
                if exhausted || rows.len() >= 10000 {
                    break;
                }
                offset += page_size;
            }
            ctx.insert(name.clone(), Value::Array(rows));
        }
    }

    // 校验输入齐备（声明的 input 都要绑上）
    let specs = input_specs(&func);
    for s in &specs {
        if !ctx.contains_key(&s.name) {
            return Err(OntoError::business_error(format!(
                "缺输入「{}」（type={}）", s.name, s.ty
            )));
        }
    }

    let bound = Value::Object(ctx);
    let result = crate::function_runtime::eval_function_any(&func, &bound)
        .await
        .map_err(|e| OntoError::business_error(format!("函数求值失败: {e}")))?;

    Ok(Json(ApiResp::ok(json!({
        "function": api_name,
        "kind": format!("{:?}", func.kind).to_lowercase(),
        "runtime": format!("{:?}", func.runtime).to_lowercase(),
        "result": result,
    }))))
}

/// 标量 JSON → 字符串（pk 用）。
fn scalar_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}
