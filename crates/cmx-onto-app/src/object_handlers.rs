//! O2 对象层 handler：对象/关系写入 + 对象集加载（Search-Around）+ 聚合。
//!
//! 写入前经定义层校验：对象类型须已定义；主键值从 properties 按 primaryKey 抽取；title 从
//! titleProperty 抽取。对象集 load/aggregate 把代数编译为一条 SQL 执行（见 store-pg::compile）。

use crate::engine::store;
use crate::object_engine::{link_resolver, object_store};
use crate::resp::{ApiResp, OntoError, Result};
use crate::tenant::current_tenant;
use axum::extract::Path;
use axum::Json;
use cmx_onto_model::objectset::{Aggregation, LinkEdge, ObjectRecord, ObjectSet, Page};
use cmx_onto_model::{ObjectStore, OntologyStore};
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
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkReq {
    pub link: String,
    pub a_pk: String,
    pub b_pk: String,
    #[serde(default)]
    pub properties: Value,
}

/// POST /links —— 建立一条关系边（校验关系类型已定义）。
pub async fn put_link(Json(req): Json<LinkReq>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    store()
        .get_link_type(&tenant, &req.link)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载关系类型失败: {e}")))?
        .ok_or_else(|| OntoError::business_error(format!("关系类型 {} 未定义", req.link)))?;
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
pub async fn delete_link(Json(req): Json<LinkReq>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
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

/// 对象集加载请求体：{ objectSet: <代数>, limit?, offset? }。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadReq {
    pub object_set: ObjectSet,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
}

/// POST /object-sets/load —— 编译对象集代数为一条 SQL 并加载（分页）。
pub async fn load_object_set(Json(req): Json<LoadReq>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let page = Page {
        limit: req.limit.unwrap_or(100),
        offset: req.offset.unwrap_or(0),
    };
    let lr = link_resolver();
    let page_out = object_store()
        .load(&tenant, &req.object_set, &page, &lr)
        .await
        .map_err(|e| OntoError::internal_error(format!("加载对象集失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!(page_out))))
}

/// 对象集聚合请求体：{ objectSet, aggregation }。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregateReq {
    pub object_set: ObjectSet,
    pub aggregation: Aggregation,
}

/// POST /object-sets/aggregate —— 对象集聚合（Count/GroupCount/GroupSum，seeds cmx-agg）。
pub async fn aggregate_object_set(Json(req): Json<AggregateReq>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let lr = link_resolver();
    let out = object_store()
        .aggregate(&tenant, &req.object_set, &req.aggregation, &lr)
        .await
        .map_err(|e| OntoError::internal_error(format!("聚合失败: {e}")))?;
    Ok(Json(ApiResp::ok(out)))
}

/// GET /objects/{type}/{pk}/links/{link} —— 便捷 Search-Around（Forward）：取该对象经 link 的相关对象。
/// 等价于 load(SearchAround(Static([pk]), link, Forward))。
pub async fn search_around(
    Path((object_type, pk, link)): Path<(String, String, String)>,
) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let set = ObjectSet::SearchAround {
        source: Box::new(ObjectSet::Static {
            object_type: object_type.clone(),
            primary_keys: vec![pk.clone()],
        }),
        link: link.clone(),
        direction: cmx_onto_model::objectset::LinkDirection::Forward,
    };
    let lr = link_resolver();
    let page_out = object_store()
        .load(&tenant, &set, &Page::default(), &lr)
        .await
        .map_err(|e| OntoError::internal_error(format!("Search-Around 失败: {e}")))?;
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
