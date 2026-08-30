//! 本体存储访问 + 启动预热（对标 cmx-rule-app::engine；本体无长驻实例，**无 poller**）。
//!
//! 多租户：[`store`] 按当前请求租户派生 db_id（见 [`crate::tenancy`]）返回一个轻量
//! [`PgOntologyStore`]（仅裹 db_id，构造廉价）。single 模式恒用 [`ONTO_DB_ID`]。

use cmx_onto_store_pg::PgOntologyStore;

/// 默认（single）租户库数据源 id；boot 注册，DDL 建 om_* 表落此库。
pub const ONTO_DB_ID: &str = crate::tenancy::ONTO_DB_ID;

/// 取当前请求租户的本体存储（single → 默认库；multi → `onto_<tenant>`）。构造廉价（仅裹 db_id）。
pub fn store() -> PgOntologyStore {
    PgOntologyStore::new(crate::tenancy::current_db_id())
}

/// 启动钩子：建默认库表（single 的 ONTO_DB_ID；multi 的租户库由 tenancy::ensure_current_ready 懒建）。
/// **不起后台线程**（本体无 poller，纯请求驱动）。
pub async fn warm_store() -> Result<(), String> {
    PgOntologyStore::new(ONTO_DB_ID)
        .ensure_schema()
        .await
        .map_err(|e| format!("建表失败: {e}"))?;
    tracing::info!(db = ONTO_DB_ID, "✅ 本体存储 schema 就绪（om_* 七表）");
    Ok(())
}
