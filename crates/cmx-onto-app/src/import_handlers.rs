//! DOC/DCT 反向导入 · app 层：归一化定义 JSON → 建对象类型 + 组合关系（DOC）/ 参照类型 + 种子项（DCT）。
//!
//! 保持 onto 与 cmx-model 解耦：接受归一化 JSON（调用方从 cmx-model 适配），本层只映射+持久化。

use crate::engine::store;
use crate::object_engine::object_store;
use crate::resp::{ApiResp, OntoError, Result};
use crate::tenant::current_tenant;
use axum::Json;
use cmx_onto_model::{map_dct, map_doc, ObjectStore, OntologyStore};
use serde_json::{json, Value};

/// POST /import/doc —— DOC（主从实体图）导入为对象类型 + 组合关系。
pub async fn import_doc(Json(body): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let imp = map_doc(&body).map_err(OntoError::business_error)?;

    let mut types = Vec::new();
    for ot in &imp.object_types {
        store()
            .upsert_object_type(&tenant, ot)
            .await
            .map_err(|e| OntoError::internal_error(format!("建对象类型 {} 失败: {e}", ot.api_name)))?;
        types.push(ot.api_name.clone());
    }
    let mut links = Vec::new();
    for lt in &imp.link_types {
        store()
            .upsert_link_type(&tenant, lt)
            .await
            .map_err(|e| OntoError::internal_error(format!("建关系 {} 失败: {e}", lt.api_name)))?;
        links.push(lt.api_name.clone());
    }
    Ok(Json(ApiResp::ok(json!({
        "source": "doc", "objectTypes": types, "linkTypes": links,
        "createdTypes": types.len(), "createdLinks": links.len()
    }))))
}

/// POST /import/dct —— DCT（字典）导入为参照对象类型 + 字典项种子对象。
pub async fn import_dct(Json(body): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let imp = map_dct(&body).map_err(OntoError::business_error)?;
    let ot = &imp.object_type;

    // 1) 建参照对象类型
    store()
        .upsert_object_type(&tenant, ot)
        .await
        .map_err(|e| OntoError::internal_error(format!("建参照类型 {} 失败: {e}", ot.api_name)))?;

    // 2) 种字典项为对象（批量事务 upsert）
    let os = object_store();
    os.ensure_object_table(&tenant, &ot.api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("建对象表失败: {e}")))?;
    let seeded = if imp.items.is_empty() {
        0
    } else {
        os.put_objects(&tenant, &ot.api_name, &imp.items)
            .await
            .map_err(|e| OntoError::internal_error(format!("种字典项失败: {e}")))?
    };

    Ok(Json(ApiResp::ok(json!({
        "source": "dct", "objectType": ot.api_name, "seededItems": seeded
    }))))
}
