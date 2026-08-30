//! cmx-onto-app —— cmx-ontology 的**平台中立应用层**（一芯）。
//!
//! **一芯多壳**：本 crate 是"芯"。handler 不绑 `State` 提取器，故 [`onto_routes::<S>()`] 对任意
//! state 泛型 `S` 成立：
//!   - 平台壳 `cmx-onto-api`（cmx-container 内，O8）：`onto_routes::<CmxAppState>()`；
//!   - 独立壳 `cmx-onto-server`（本 workspace）：`onto_routes::<()>()`。
//!
//! 两壳复用同一 handler + 同一路由表，零业务漂移。

pub mod auth;
pub mod dashboard;
pub mod engine;
pub mod handlers;
pub mod object_engine;
pub mod object_handlers;
pub mod openapi;
pub mod resp;
pub mod stats;
pub mod tenancy;
pub mod tenant;

pub use auth::auth as auth_middleware;
pub use engine::{warm_store, ONTO_DB_ID};
pub use object_engine::warm_object_store;
pub use openapi::openapi_json;
pub use resp::{ApiResp, OntoError, Result};
pub use tenant::{current_tenant, current_user, identity_snapshot};

use axum::routing::{get, post};
use axum::Router;

/// 本体模块全部路由，**旧前缀 `/onto/*`**（内嵌壳兼容）。对任意 state 泛型 `S` 成立。
pub fn onto_routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new().nest("/onto", onto_routes_inner::<S>())
}

/// 本体模块全部路由，**v1 正式契约前缀 `/onto/v1/*`**。
pub fn onto_routes_v1<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new().nest("/onto/v1", onto_routes_inner::<S>())
}

/// 路由表本体（相对前缀）。
fn onto_routes_inner<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        // —— 对象类型 ——
        .route(
            "/object-types",
            get(handlers::list_object_types).post(handlers::save_object_type),
        )
        .route("/object-types/validate", post(handlers::validate_object_type))
        .route(
            "/object-types/{api_name}",
            get(handlers::get_object_type).delete(handlers::delete_object_type),
        )
        // —— 关系类型 ——
        .route(
            "/link-types",
            get(handlers::list_link_types).post(handlers::save_link_type),
        )
        .route(
            "/link-types/{api_name}",
            get(handlers::get_link_type).delete(handlers::delete_link_type),
        )
        // —— 接口 ——
        .route(
            "/interfaces",
            get(handlers::list_interfaces).post(handlers::save_interface),
        )
        .route(
            "/interfaces/{api_name}",
            get(handlers::get_interface).delete(handlers::delete_interface),
        )
        // —— 共享属性类型 ——
        .route(
            "/shared-properties",
            get(handlers::list_shared_properties).post(handlers::save_shared_property),
        )
        .route(
            "/shared-properties/{api_name}",
            get(handlers::get_shared_property).delete(handlers::delete_shared_property),
        )
        // —— 动作类型 ——
        .route(
            "/action-types",
            get(handlers::list_action_types).post(handlers::save_action_type),
        )
        .route(
            "/action-types/{api_name}",
            get(handlers::get_action_type).delete(handlers::delete_action_type),
        )
        // —— 函数 ——
        .route(
            "/functions",
            get(handlers::list_functions).post(handlers::save_function),
        )
        .route(
            "/functions/{api_name}",
            get(handlers::get_function).delete(handlers::delete_function),
        )
        // —— 清单 / 发布 / 版本 ——
        .route("/manifest", get(handlers::manifest))
        .route("/publish", post(handlers::publish))
        .route("/versions", get(handlers::list_versions))
        .route("/versions/{version}", get(handlers::get_version))
        // —— O2 对象层：对象写入 ——
        .route("/objects/{object_type}", post(object_handlers::put_object))
        .route(
            "/objects/{object_type}/batch",
            post(object_handlers::put_objects_batch),
        )
        .route(
            "/objects/{object_type}/{pk}",
            axum::routing::delete(object_handlers::delete_object),
        )
        .route(
            "/objects/{object_type}/{pk}/links/{link}",
            get(object_handlers::search_around),
        )
        // —— O2 对象层：关系边 ——
        .route(
            "/links",
            post(object_handlers::put_link).delete(object_handlers::delete_link),
        )
        // —— O2 对象层：对象集加载 / 聚合（Object Set Service 对等）——
        .route("/object-sets/load", post(object_handlers::load_object_set))
        .route(
            "/object-sets/aggregate",
            post(object_handlers::aggregate_object_set),
        )
        // —— 建模台 / 监控数据源 ——
        .route("/stats", get(stats::stats))
}
