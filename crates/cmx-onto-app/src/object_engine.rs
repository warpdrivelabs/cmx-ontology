//! O2 对象存储访问 + 启动预热（对象层，对标 [`crate::engine`] 定义层）。
//!
//! [`object_store`] / [`link_resolver`] 按当前租户 db_id 派生（构造廉价）。boot 时建 `ol_edge` 单表；
//! per-type 表 `oo_<type>` 在对象类型首次写入时惰性 ensure（对象类型可动态增删，不预建）。

use cmx_onto_store_pg::{PgLinkResolver, PgObjectStore};

/// 取当前请求租户的对象存储。
pub fn object_store() -> PgObjectStore {
    PgObjectStore::new(crate::tenancy::current_db_id())
}

/// 取当前请求租户的关系解析器（编译 SearchAround 用）。
pub fn link_resolver() -> PgLinkResolver {
    PgLinkResolver::new(crate::tenancy::current_db_id())
}

/// 启动钩子：建 `ol_edge` 关系边表（默认库；租户库由 tenancy 懒建时一并 ensure）。
pub async fn warm_object_store() -> Result<(), String> {
    PgObjectStore::new(crate::engine::ONTO_DB_ID)
        .ensure_edge_table()
        .await
        .map_err(|e| format!("建 ol_edge 失败: {e}"))?;
    tracing::info!(db = crate::engine::ONTO_DB_ID, "✅ 对象存储 ol_edge 就绪（O2）");
    Ok(())
}
