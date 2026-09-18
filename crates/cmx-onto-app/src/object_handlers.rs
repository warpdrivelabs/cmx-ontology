//! O2 对象层 handler：对象/关系写入 + 对象集加载（Search-Around）+ 聚合。
//!
//! 写入前经定义层校验：对象类型须已定义；主键值从 properties 按 primaryKey 抽取；title 从
//! titleProperty 抽取。对象集 load/aggregate 把代数编译为一条 SQL 执行（见 store-pg::compile）。

use crate::engine::store;
use crate::object_engine::{link_resolver, object_store};
use crate::resp::{ApiResp, OntoError, Result};
use crate::tenant::current_tenant;
use axum::extract::{Path, Query};
use axum::Json;
use cmx_onto_model::objectset::{Aggregation, LinkEdge, ObjectRecord, ObjectSet, Page};
use cmx_onto_model::{LinkBacking, LinkResolver, LinkTypeDef, ObjectStore, OntologyStore};
use serde::Deserialize;
use serde_json::{json, Value};

/// 写对象请求体：{ properties: {...}, pk?, title? }。pk/title 缺省从定义的 primaryKey/titleProperty 抽取。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PutObjectReq {
    pub properties: Value,
    pub pk: Option<String>,
    pub title: Option<String>,
}

/// 乐观锁修改请求体：{ set: {...}, expectedUpdatedAt? }。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ModifyReq {
    pub set: Value,
    pub expected_updated_at: Option<String>,
}

/// POST /objects/{type}/{pk}/modify —— 乐观锁修改（读改写；expectedUpdatedAt 版本冲突→conflict）。
#[utoipa::path(
    post,
    path = "/api/onto/v1/objects/{object_type}/{pk}/modify",
    tag = "对象存储",
    summary = "乐观锁修改对象（读改写）",
    params(
        ("object_type" = String, Path, description = "对象类型 API 名"),
        ("pk" = String, Path, description = "对象主键"),
    ),
    request_body(content = Value, description = "set 字段 + expectedUpdatedAt"),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn modify_object(
    Path((object_type, pk)): Path<(String, String)>,
    Json(req): Json<ModifyReq>,
) -> Result<Json<ApiResp<Value>>> {
    let (status, updated_at, props) = object_store()
        .modify_with_optlock(&object_type, &pk, &req.set, req.expected_updated_at.as_deref())
        .await
        .map_err(|e| OntoError::internal_error(format!("修改对象失败: {e}")))?;
    // 冲突走 code=0 + data.conflict（对齐 flow 协同 M1 乐观锁；前端据此刷新重试）。
    Ok(Json(ApiResp::ok(json!({
        "objectType": object_type,
        "pk": pk,
        "status": status,
        "conflict": status == "conflict",
        "updatedAt": updated_at,
        "properties": props,
    }))))
}

/// POST /objects/{type} —— upsert 一个对象（按定义校验 + ensure 物化表）。
#[utoipa::path(
    post,
    path = "/api/onto/v1/objects/{object_type}",
    tag = "对象存储",
    summary = "写入/更新一个对象",
    params(
        ("object_type" = String, Path, description = "对象类型 API 名"),
    ),
    request_body(content = Value, description = "属性值（pk/title 缺省按定义抽取）"),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn put_object(
    Path(object_type): Path<String>,
    Json(req): Json<PutObjectReq>,
) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let def = store()
        .get_object_type(&tenant, &object_type)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载对象类型失败: {e}")))?
        .ok_or_else(|| OntoError::business_error(format!("对象类型 {object_type} 未定义，无法写入")))?;

    let (pk, title) = derive_pk_title(&def, &req)?;
    let os = object_store();
    os.ensure_object_table(&tenant, &object_type)
        .await
        .map_err(|e| OntoError::internal_error(format!("建对象表失败: {e}")))?;
    os.put_object(&tenant, &object_type, &pk, &title, &req.properties)
        .await
        .map_err(|e| OntoError::internal_error(format!("写对象失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "objectType": object_type, "pk": pk, "saved": true }))))
}

/// POST /objects/{type}/batch —— 批量 upsert（同一事务）。body: [{properties,pk?,title?}, ...]
#[utoipa::path(
    post,
    path = "/api/onto/v1/objects/{object_type}/batch",
    tag = "对象存储",
    summary = "批量 upsert 对象（同一事务，要么全成要么全败）",
    params(
        ("object_type" = String, Path, description = "对象类型 API 名"),
    ),
    request_body(content = Value, description = "每项 {properties, pk?, title?}；全部在同一事务提交"),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn put_objects_batch(
    Path(object_type): Path<String>,
    Json(items): Json<Vec<PutObjectReq>>,
) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let def = store()
        .get_object_type(&tenant, &object_type)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载对象类型失败: {e}")))?
        .ok_or_else(|| OntoError::business_error(format!("对象类型 {object_type} 未定义")))?;

    let mut rows = Vec::with_capacity(items.len());
    for req in &items {
        let (pk, title) = derive_pk_title(&def, req)?;
        rows.push(ObjectRecord { pk, title, properties: req.properties.clone() });
    }
    let os = object_store();
    os.ensure_object_table(&tenant, &object_type)
        .await
        .map_err(|e| OntoError::internal_error(format!("建对象表失败: {e}")))?;
    let n = os
        .put_objects(&tenant, &object_type, &rows)
        .await
        .map_err(|e| OntoError::internal_error(format!("批量写对象失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "objectType": object_type, "written": n }))))
}

/// DELETE /objects/{type}/{pk} —— 删除对象（连带清关系边）。
#[utoipa::path(
    delete,
    path = "/api/onto/v1/objects/{object_type}/{pk}",
    tag = "对象存储",
    summary = "删除对象（连带清关系边）",
    params(
        ("object_type" = String, Path, description = "对象类型 API 名"),
        ("pk" = String, Path, description = "对象主键"),
    ),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn delete_object(
    Path((object_type, pk)): Path<(String, String)>,
) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let n = object_store()
        .delete_object(&tenant, &object_type, &pk)
        .await
        .map_err(|e| OntoError::internal_error(format!("删对象失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "pk": pk, "deleted": n > 0 }))))
}

/// 关系边写入请求体。
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LinkReq {
    pub link: String,
    pub a_pk: String,
    pub b_pk: String,
    #[serde(default)]
    pub properties: Value,
}

/// 非 Edge backing 的关系不落 ol_edge（FK/连接表/中间对象均直查物理布局）：
/// 边写入显式拒绝，杜绝「写入返回 200 成功、查询永不生效」的静默假写。
fn ensure_edge_writable(link: &str, lt: &LinkTypeDef) -> Result<()> {
    if !matches!(lt.backing_parsed(), LinkBacking::Edge) {
        return Err(OntoError::business_error(format!(
            "关系 {link} 为 FK/连接表/中间对象背书：请直接维护外键属性值/连接表/中间对象数据，不支持 /links 边写入"
        )));
    }
    Ok(())
}

/// POST /links —— 建立一条关系边（校验关系类型已定义）。
#[utoipa::path(
    post,
    path = "/api/onto/v1/links",
    tag = "对象存储",
    summary = "建立一条关系边",
    request_body(content = Value, description = "关系边"),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn put_link(Json(req): Json<LinkReq>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let lt = store()
        .get_link_type(&tenant, &req.link)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载关系类型失败: {e}")))?
        .ok_or_else(|| OntoError::business_error(format!("关系类型 {} 未定义", req.link)))?;
    ensure_edge_writable(&req.link, &lt)?;
    let edge = LinkEdge {
        link: req.link.clone(),
        a_pk: req.a_pk,
        b_pk: req.b_pk,
        properties: req.properties,
    };
    object_store()
        .put_link(&tenant, &edge)
        .await
        .map_err(|e| OntoError::internal_error(format!("建关系边失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "link": req.link, "saved": true }))))
}

/// DELETE /links —— 删除一条关系边。body: {link,aPk,bPk}
#[utoipa::path(
    delete,
    path = "/api/onto/v1/links",
    tag = "对象存储",
    summary = "删除一条关系边",
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn delete_link(Json(req): Json<LinkReq>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let lt = store()
        .get_link_type(&tenant, &req.link)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载关系类型失败: {e}")))?
        .ok_or_else(|| OntoError::business_error(format!("关系类型 {} 未定义", req.link)))?;
    ensure_edge_writable(&req.link, &lt)?;
    let edge = LinkEdge {
        link: req.link.clone(),
        a_pk: req.a_pk,
        b_pk: req.b_pk,
        properties: Value::Null,
    };
    let n = object_store()
        .delete_link(&tenant, &edge)
        .await
        .map_err(|e| OntoError::internal_error(format!("删关系边失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "link": req.link, "deleted": n > 0 }))))
}

/// 对象集加载请求体：{ objectSet: <代数>, limit?, offset?, subjects?, view?, include? }。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadReq {
    pub object_set: ObjectSet,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    /// 主体覆盖（`["role:east","user:bob"]`；读侧硬门 PEP 用）。auth off/单租户下调用方声明；
    /// jwt 模式以令牌为准。空则回退上下文（role:tenant + user）。
    #[serde(default)]
    pub subjects: Vec<String>,
    /// 场景上下文（§7.3）：terminal 类型 ∈ 场景 objects，否则 409（校验先于 PEP）。
    #[serde(default)]
    pub view: Option<String>,
    /// 状态分层（D9，§5.5）：默认仅 active；非 active 且未 include → 404。
    #[serde(default)]
    pub include: Option<String>,
}

/// POST /object-sets/load —— 编译对象集代数为一条 SQL 并加载（读侧硬门 → 分页 → 列脱敏）。
#[utoipa::path(
    post,
    path = "/api/onto/v1/object-sets/load",
    tag = "对象存储",
    summary = "加载对象集（代数编译成一条 SQL，杜绝 N+1）",
    request_body(content = Value, description = "对象集代数 + 分页 + 主体/场景/状态参数"),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn load_object_set(Json(req): Json<LoadReq>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    // 场景 + 状态过滤（§7.3：两校验先于 PEP 权限检查；场景过滤是可见性组织不是权限）。
    let scope = crate::filter::SceneScope::resolve(&tenant, req.view.as_deref()).await?;
    let filter = crate::filter::StatusFilter::parse(req.include.as_deref())?;
    if let Some(scope) = &scope {
        scope.ensure_set_allowed(&req.object_set)?;
    }
    let terminal_for_status = req.object_set.terminal_object_type().unwrap_or("").to_string();
    if !terminal_for_status.is_empty() {
        crate::filter::ensure_object_queryable(&tenant, &terminal_for_status, &filter).await?;
    }
    let subjects = crate::pep::subjects_from(&req.subjects);
    // 读侧硬门：终端类型策略匹配 → 硬拒 / 行残差折入。
    let terminal = req.object_set.terminal_object_type().unwrap_or("").to_string();
    let (secured_set, mask_plan) =
        crate::pep::enforce_read(&tenant, &subjects, &terminal, req.object_set.clone()).await?;
    let page = Page {
        limit: req.limit.unwrap_or(100),
        offset: req.offset.unwrap_or(0),
    };
    let lr = link_resolver();
    let mut page_out = object_store()
        .load(&tenant, &secured_set, &page, &lr)
        .await
        .map_err(|e| OntoError::internal_error(format!("加载对象集失败: {e}")))?;
    mask_plan.apply(&mut page_out.rows);
    Ok(Json(ApiResp::ok(json!(page_out))))
}

/// 对象集聚合请求体：{ objectSet, aggregation, subjects?, view?, include? }。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregateReq {
    pub object_set: ObjectSet,
    pub aggregation: Aggregation,
    #[serde(default)]
    pub subjects: Vec<String>,
    #[serde(default)]
    pub view: Option<String>,
    #[serde(default)]
    pub include: Option<String>,
}

/// POST /object-sets/aggregate —— 对象集聚合（读侧硬门 → Count/GroupCount/GroupSum）。
/// 硬门把行残差折入后再聚合（受限行不计入统计）；deny / 受控无授权 → 403。
#[utoipa::path(
    post,
    path = "/api/onto/v1/object-sets/aggregate",
    tag = "对象存储",
    summary = "对象集聚合（受限行不计入统计）",
    request_body(content = Value, description = "对象集代数 + 聚合规格"),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn aggregate_object_set(Json(req): Json<AggregateReq>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let scope = crate::filter::SceneScope::resolve(&tenant, req.view.as_deref()).await?;
    let filter = crate::filter::StatusFilter::parse(req.include.as_deref())?;
    if let Some(scope) = &scope {
        scope.ensure_set_allowed(&req.object_set)?;
    }
    let terminal = req.object_set.terminal_object_type().unwrap_or("").to_string();
    if !terminal.is_empty() {
        crate::filter::ensure_object_queryable(&tenant, &terminal, &filter).await?;
    }
    let subjects = crate::pep::subjects_from(&req.subjects);
    let (secured_set, _mask) =
        crate::pep::enforce_read(&tenant, &subjects, &terminal, req.object_set.clone()).await?;
    let lr = link_resolver();
    let out = object_store()
        .aggregate(&tenant, &secured_set, &req.aggregation, &lr)
        .await
        .map_err(|e| OntoError::internal_error(format!("聚合失败: {e}")))?;
    Ok(Json(ApiResp::ok(out)))
}

/// GET /objects/{type}/{pk}/links/{link} —— 便捷 Search-Around：取该对象经 link 的相关对象。
/// 方向自判：源对象类型在 A 端 → Forward（终端 B），在 B 端 → Reverse（终端 A）；
/// 自关联（A==B）取 Forward；均不命中报 404。读侧硬门作用在解析出的终端类型上。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchAroundQuery {
    /// 场景上下文（§7.3）：link 必须 ∈ 场景 links，否则 409。
    pub view: Option<String>,
    /// 状态分层（D9）。
    pub include: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/onto/v1/objects/{object_type}/{pk}/links/{link}",
    tag = "对象存储",
    summary = "Search-Around：沿关系走到另一头的对象们",
    params(
        ("object_type" = String, Path, description = "对象类型 API 名"),
        ("pk" = String, Path, description = "对象主键"),
        ("link" = String, Path, description = "关系类型 API 名"),
    ),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn search_around(
    Path((object_type, pk, link)): Path<(String, String, String)>,
    Query(q): Query<SearchAroundQuery>,
) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let scope = crate::filter::SceneScope::resolve(&tenant, q.view.as_deref()).await?;
    let filter = crate::filter::StatusFilter::parse(q.include.as_deref())?;
    crate::filter::ensure_object_queryable(&tenant, &object_type, &filter).await?;
    crate::filter::ensure_link_queryable(&tenant, &link, &filter).await?;
    if let Some(scope) = &scope {
        scope.ensure_object_member(&object_type)?;
        scope.ensure_link_member(&link)?;
    }
    let lr = link_resolver();
    let (direction, terminal) = match lr
        .ends(&tenant, &link)
        .await
        .map_err(|e| OntoError::internal_error(format!("解析关系两端失败: {e}")))?
    {
        Some((a, b)) if a == object_type => (cmx_onto_model::objectset::LinkDirection::Forward, b),
        Some((a, b)) if b == object_type => (cmx_onto_model::objectset::LinkDirection::Reverse, a),
        _ => {
            return Err(OntoError::not_found(format!(
                "关系 {link} 不存在于对象类型 {object_type} 上"
            )))
        }
    };
    let set = ObjectSet::SearchAround {
        source: Box::new(ObjectSet::Static {
            object_type: object_type.clone(),
            primary_keys: vec![pk.clone()],
        }),
        link: link.clone(),
        direction,
    };
    let subjects = crate::pep::subjects_from(&[]);
    let (secured_set, mask_plan) =
        crate::pep::enforce_read(&tenant, &subjects, &terminal, set).await?;
    let mut page_out = object_store()
        .load(&tenant, &secured_set, &Page::default(), &lr)
        .await
        .map_err(|e| OntoError::internal_error(format!("Search-Around 失败: {e}")))?;
    mask_plan.apply(&mut page_out.rows);
    Ok(Json(ApiResp::ok(json!(page_out))))
}

// ————————————————————————— 助手 —————————————————————————

/// 从定义 + 请求抽取 (pk, title)：pk 优先请求显式值，否则按 primaryKey 从 properties 取；title 同理。
fn derive_pk_title(
    def: &cmx_onto_model::ObjectTypeDef,
    req: &PutObjectReq,
) -> Result<(String, String)> {
    let pk = match &req.pk {
        Some(p) if !p.is_empty() => p.clone(),
        _ => {
            if def.primary_key.is_empty() {
                return Err(OntoError::business_error(
                    "对象类型未定义主键，且请求未显式给 pk".to_string(),
                ));
            }
            json_scalar_to_string(req.properties.get(&def.primary_key)).ok_or_else(|| {
                OntoError::business_error(format!(
                    "properties 缺主键属性「{}」的值", def.primary_key
                ))
            })?
        }
    };
    let title = match &req.title {
        Some(t) if !t.is_empty() => t.clone(),
        _ => {
            if def.title_property.is_empty() {
                pk.clone()
            } else {
                json_scalar_to_string(req.properties.get(&def.title_property)).unwrap_or_else(|| pk.clone())
            }
        }
    };
    Ok((pk, title))
}

/// 标量 JSON → 字符串（pk/title 用；非标量返回 None）。
fn json_scalar_to_string(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    }
}
