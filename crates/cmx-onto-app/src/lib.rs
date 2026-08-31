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
pub mod action_handlers;
pub mod function_handlers;
pub mod policy_handlers;
pub mod funnel_handlers;
pub mod import_handlers;
pub mod osdk_handlers;
pub mod events;
pub mod object_engine;
pub mod object_handlers;
pub mod openapi;
pub mod outbound;
pub mod resp;
pub mod stats;
pub mod tenancy;
pub mod tenant;

pub use auth::auth as auth_middleware;
pub use engine::{warm_store, ONTO_DB_ID};
pub use object_engine::warm_object_store;
pub use openapi::{openapi_json, swagger_ui};
pub use events::events as sse_events;
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
            "/objects/{object_type}/{pk}/modify",
            post(object_handlers::modify_object),
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
        // —— O4 动作引擎：执行 / 试算 / 审计 ——
        .route(
            "/action-types/{api_name}/execute",
            post(action_handlers::execute_action),
        )
        .route(
            "/action-types/{api_name}/dry-run",
            post(action_handlers::dry_run_action),
        )
        .route("/action-logs", get(action_handlers::list_action_logs))
        .route("/action-outbox", get(action_handlers::list_action_outbox))
        .route("/action-outbox/config", get(action_handlers::outbox_config))
        .route("/action-outbox/dispatch", post(action_handlers::dispatch_outbox))
        .route(
            "/action-outbox/{id}/dispatched",
            post(action_handlers::mark_outbox_dispatched),
        )
        // —— O5 函数计算引擎：求值 ——
        .route(
            "/functions/{api_name}/evaluate",
            post(function_handlers::evaluate_fn),
        )
        // —— O6 动态安全：策略 CRUD + 带安全的对象集加载 ——
        .route(
            "/policies",
            get(policy_handlers::list_policies).post(policy_handlers::upsert_policy),
        )
        .route(
            "/policies/{api_name}",
            axum::routing::delete(policy_handlers::delete_policy),
        )
        .route("/secure/object-sets/load", post(policy_handlers::secure_load))
        // —— O3 数据集成：源映射 CRUD + 全量同步 + 隔离区 + 管道状态 ——
        .route(
            "/funnel/mappings",
            get(funnel_handlers::list_mappings).post(funnel_handlers::upsert_mapping),
        )
        .route(
            "/funnel/mappings/{object_type}",
            axum::routing::delete(funnel_handlers::delete_mapping),
        )
        .route("/funnel/sync/{object_type}", post(funnel_handlers::run_sync))
        .route("/funnel/quarantine", get(funnel_handlers::list_quarantine))
        .route(
            "/funnel/pipeline-status/{object_type}",
            get(funnel_handlers::pipeline_status),
        )
        // —— DOC/DCT 反向导入 ——
        .route("/import/doc", post(import_handlers::import_doc))
        .route("/import/dct", post(import_handlers::import_dct))
        // —— OSDK 代码生成 ——
        .route("/osdk/typescript", get(osdk_handlers::typescript_sdk))
        // —— 建模台 / 监控数据源 ——
        .route("/stats", get(stats::stats))
}
