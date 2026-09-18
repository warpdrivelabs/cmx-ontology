//! OSDK 代码生成 · app 层：读当前租户本体 → 生成 TypeScript 客户端（text/typescript）。
//!
//! 载全部对象类型完整定义（manifest 只给 meta）+ 动作/函数 apiName 列表 + 关系 → 纯生成器出码。

use crate::engine::store;
use crate::resp::{OntoError, Result};
use crate::tenant::current_tenant;
use axum::extract::Query;
use axum::response::{IntoResponse, Response};
use cmx_onto_model::{generate_typescript, OntologyStore};

#[derive(Debug, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct OsdkQuery {
    /// 状态分层（D9，§5.5）：默认仅生成 active；deprecated 显式打开时下游可见 @Deprecated。
    pub include: Option<String>,
}

/// GET /osdk/typescript —— 生成 TypeScript OSDK（强类型对象接口 + 客户端；默认仅 active）。
#[utoipa::path(
    get,
    path = "/api/onto/v1/osdk/typescript",
    tag = "工具",
    summary = "按当前本体生成 TypeScript 客户端代码",
    responses(
        (status = 200, content_type = "text/typescript", description = "TypeScript OSDK 单文件源码"),
    )
)]
pub async fn typescript_sdk(Query(q): Query<OsdkQuery>) -> Result<Response> {
    let tenant = current_tenant();
    let s = store();
    let filter = crate::filter::StatusFilter::parse(q.include.as_deref())?;

    // 对象类型：manifest 只给 meta，逐一拉完整定义（含 properties）。
    let metas = s
        .list_object_types(&tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("列对象类型失败: {e}")))?;
    let metas: Vec<_> = metas.into_iter().filter(|m| filter.allows(m.status)).collect();
    let mut types = Vec::with_capacity(metas.len());
    for m in &metas {
        if let Ok(Some(def)) = s.get_object_type(&tenant, &m.api_name).await {
            types.push(def);
        }
    }
    // 关系完整定义（LinkTypeMeta→取 def；失败跳过）
    let link_metas = s
        .list_link_types(&tenant)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|m| filter.allows(m.status))
        .collect::<Vec<_>>();
    let mut links = Vec::with_capacity(link_metas.len());
    for m in &link_metas {
        if let Ok(Some(def)) = s.get_link_type(&tenant, &m.api_name).await {
            links.push(def);
        }
    }
    let actions: Vec<String> = s
        .list_action_types(&tenant)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|m| filter.allows(m.status))
        .map(|m| m.api_name)
        .collect();
    let functions: Vec<String> = s
        .list_functions(&tenant)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|m| m.api_name)
        .collect();

    let code = generate_typescript(&types, &links, &actions, &functions);
    // 以 TS 文本返回（免信封，供 curl/构建工具直取；附下载文件名）。
    Ok((
        [
            ("content-type", "text/typescript; charset=utf-8"),
            ("content-disposition", "inline; filename=\"ontology-sdk.ts\""),
        ],
        code,
    )
        .into_response())
}
