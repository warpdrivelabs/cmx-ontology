//! cmx-onto-app —— cmx-ontology 的**平台中立应用层**（一芯）。
//!
//! **一芯多壳**：本 crate 是"芯"。handler 不绑 `State` 提取器，故 [`onto_routes::<S>()`] 对任意
//! state 泛型 `S` 成立：
//!   - 平台壳 `cmx-onto-api`（cmx-container 内，O8）：`onto_routes::<CmxAppState>()`；
//!   - 独立壳 `cmx-onto-server`（本 workspace）：`onto_routes::<()>()`。
//!
//! 两壳复用同一 handler + 同一路由表，零业务漂移。

// openapi.rs 的大 JSON 宏在 clippy 下超默认递归深度（128）——提高到 512。
#![recursion_limit = "512"]

pub mod auth;
pub mod dashboard;
pub mod engine;
pub mod handlers;
pub mod action_handlers;
pub mod action_templates;
pub mod archive_handlers;
pub mod function_handlers;
pub mod function_runtime;
pub mod policy_handlers;
pub mod funnel_handlers;
pub mod flow_callback_handlers;
pub mod import_handlers;
pub mod osdk_handlers;
pub mod events;
pub mod object_engine;
pub mod object_handlers;
pub mod module;
pub mod openapi;
pub mod outbound;
pub mod pep;
pub mod resp;
pub mod stats;
pub mod tenancy;
pub mod tenant;
pub mod view_handlers;

pub use auth::auth as auth_middleware;
pub use engine::{warm_store, ONTO_DB_ID};
pub use object_engine::warm_object_store;
pub use openapi::{openapi_json, swagger_ui};
pub use events::events as sse_events;
pub use resp::{ApiResp, OntoError, Result};
pub use tenant::{current_tenant, current_user, identity_snapshot};

// 路由装配契约与组合器（真源 cmx-engine-kit；bin 组合根经本 crate 引用，免加依赖）。
pub use cmx_engine_kit::routes::{ModuleRoutes, ModuleSet};
pub use module::{OntoCoreModule, OntoV1Module};

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
            get(view_handlers::list_object_types).post(handlers::save_object_type),
        )
        .route("/object-types/validate", post(handlers::validate_object_type))
        // D15：批量详情（设计器首屏装载；静态段 + POST + JSON body，符合新接口规范）。
        .route("/object-types/batch", post(handlers::get_object_types_batch))
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
        // A1：共享属性批量详情（本体工作室装载层，{items,errors}）。
        .route("/shared-properties/batch", post(view_handlers::get_shared_properties_batch))
        // —— 场景视图（本体工作室 P1）——
        .route(
            "/views",
            get(view_handlers::list_views).post(view_handlers::save_view),
        )
        .route("/views/remove", post(view_handlers::remove_view))
        .route("/views/layout", post(view_handlers::save_view_layout))
        .route("/graph", get(view_handlers::graph))
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
        // —— 清单 / 存档 / 版本（直改 live 架构：编辑直写 om_*，存档 = 检查点，回滚 = 恢复）——
        .route("/manifest", get(handlers::manifest))
        .route("/snapshots", post(archive_handlers::create_snapshot))
        .route("/versions", get(handlers::list_versions))
        .route("/versions/{version}", get(handlers::get_version))
        .route("/versions/diff", get(archive_handlers::versions_diff))
        .route("/versions/restore", post(archive_handlers::versions_restore))
        .route("/me/roles", get(archive_handlers::me_roles))
        // —— O7 实时（P2 注册进鉴权路由；标准 Bearer 鉴权，前端 fetch 流式读取消费 SSE）——
        .route("/events", get(sse_events))
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
        // —— O4 动作引擎：执行 / 试算 / 批量 / 审计 ——
        .route(
            "/action-types/{api_name}/execute",
            post(action_handlers::execute_action),
        )
        .route(
            "/action-types/{api_name}/dry-run",
            post(action_handlers::dry_run_action),
        )
        // 批量执行（P1-3；固定路径无路径参数，apiName 入 body——AGENTS §四.6）
        .route("/action-types/execute-batch", post(action_handlers::execute_batch))
        // 动作可见性 PEP 预检（P2-1；固定路径，动作列表入 body）
        .route("/action-types/check-permission", post(action_handlers::check_permission))
        .route("/action-logs", get(action_handlers::list_action_logs))
        .route("/action-outbox", get(action_handlers::list_action_outbox))
        .route("/action-outbox/config", get(action_handlers::outbox_config))
        .route("/flow/definitions", get(action_handlers::flow_definitions))
        .route("/report/definitions", get(action_handlers::report_definitions))
        .route("/action-templates", get(action_handlers::action_templates))
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
        // —— 主数据事件推送（MDM 分发引擎 webhook 订阅入口；鉴权走 X-API-Key 服务身份）——
        .route("/funnel/push", post(funnel_handlers::funnel_push))
        // —— DOC/DCT 反向导入 ——
        .route("/import/doc", post(import_handlers::import_doc))
        .route("/import/dct", post(import_handlers::import_dct))
        // —— 流程审批结果回调（flowengine webhook → 回写对象状态；鉴权走 X-API-Key 服务身份）——
        .route("/flow-callback", post(flow_callback_handlers::receive))
        // —— OSDK 代码生成 ——
        .route("/osdk/typescript", get(osdk_handlers::typescript_sdk))
        // —— 建模台 / 监控数据源 ——
        .route("/stats", get(stats::stats))
}
