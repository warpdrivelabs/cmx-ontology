//! 多租户：db-per-tenant 物理隔离（镜像 flow S2 / rules R3）。
//!
//! 模式由配置 `auth.tenancy` 决定（toml `[auth]` 段 ← env `AUTH__TENANCY` 覆盖）：
//! `single`（默认，单库，零回归）| `multi`（每租户一库 `onto_<tenant>`）。
//! multi 下按 [`current_tenant`](crate::tenant::current_tenant) 派生 db_id（小写），并在该租户首次
//! 访问时懒注册数据源（URL 由 env `ONTO_TENANT_DB_URL_TEMPLATE` 的 `{tenant}` 占位派生）+ 建表。

use cmx_database_pg::{DbConfig, DbType};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// 默认租户库 db_id（single 模式 / 无租户 scope）。
pub const ONTO_DB_ID: &str = "onto_pg";

/// 租户模式（配置 `auth.tenancy`，ConfigManager 直读；默认 single）。
fn mode() -> String {
    cmx_utils::ConfigManager::try_global()
        .and_then(|cm| cm.get_string("auth.tenancy").ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "single".to_string())
}

/// 是否多租户模式。
pub fn is_multi() -> bool {
    mode() == "multi"
}

/// 当前请求应使用的 db_id。single → [`ONTO_DB_ID`]；multi → `onto_<tenant>`（小写）。
pub fn current_db_id() -> String {
    if is_multi() {
        format!("onto_{}", crate::tenant::current_tenant().to_lowercase())
    } else {
        ONTO_DB_ID.to_string()
    }
}

static READY: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
fn ready_set() -> &'static Mutex<HashSet<String>> {
    READY.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 确保当前租户库就绪（懒注册数据源 + 建表；每 db_id 一次）。single 模式：默认库已在 boot 注册，跳过。
/// 由认证中间件在建立租户 scope 后、handler 前调用。非致命：失败只 warn，端点后续返错便于诊断。
pub async fn ensure_current_ready() {
    if !is_multi() {
        return;
    }
    let db_id = current_db_id();
    {
        let set = ready_set().lock().unwrap();
        if set.contains(&db_id) {
            return;
        }
    }
    let tenant = crate::tenant::current_tenant().to_lowercase();
    let template = std::env::var("ONTO_TENANT_DB_URL_TEMPLATE")
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:5432/onto_{tenant}".to_string());
    let url = template.replace("{tenant}", &tenant);
    let cfg = DbConfig {
        db_type: DbType::Postgres,
        db_url: url,
        db_id: db_id.clone(),
        db_name: None,
        db_schema: Some("public".to_string()),
        default: false,
        pool_config: Default::default(),
        health_check_interval: 60,
        health_check_timeout: 5,
        domain_code: None,
        application_code: None,
        module_code: None,
        source_type: Some("default".to_string()),
    };
    if let Err(e) = cmx_service_base::register_pg_datasources(&[cfg]).await {
        tracing::warn!(db_id = %db_id, error = %e, "租户数据源注册失败");
        return;
    }
    let store = cmx_onto_store_pg::PgOntologyStore::new(db_id.clone());
    if let Err(e) = store.ensure_schema().await {
        tracing::warn!(db_id = %db_id, error = %e, "租户建表失败");
        return;
    }
    // O2 对象层：建 ol_edge 关系边表（per-type 表按需惰性建）。
    let obj = cmx_onto_store_pg::PgObjectStore::new(db_id.clone());
    if let Err(e) = obj.ensure_edge_table().await {
        tracing::warn!(db_id = %db_id, error = %e, "租户建 ol_edge 失败");
        return;
    }
    ready_set().lock().unwrap().insert(db_id.clone());
    tracing::info!(db_id = %db_id, "✅ 租户库就绪（数据源 + schema）");
}
