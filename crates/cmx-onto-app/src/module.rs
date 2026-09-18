//! onto 路由模块适配器——`ModuleRoutes<S>` 契约接入（bin 组合根消费）。
//!
//! [`crate::onto_routes`] / [`crate::onto_routes_v1`] 自由函数仍是路由真源（平台壳
//! `cmx-onto-api` 以 `S = CmxAppState` 直调），本模块仅以 unit struct 适配为契约模块，
//! 供独立壳 bin 以 [`crate::ModuleSet`] 组合（去重守卫 + 统一 fold）。v1 与旧前缀是同
//! inner 表的双前缀二重挂载，注册为两个模块（module_name 不同），去重守卫不误伤。

use axum::Router;

use cmx_engine_kit::routes::ModuleRoutes;
use utoipa::OpenApi as _;

use crate::{onto_routes, onto_routes_v1};

/// 旧前缀 `/onto/*`（内嵌壳兼容）。
///
/// `api_doc()` 保持默认 `None`：旧前缀是 v1 同 inner 表的兼容镜像，正式契约只在 v1
/// （`OntoV1Module`）；utoipa `merge` 按 path 浅比较静默丢弃后者，双份入册必丢一份。
pub struct OntoCoreModule;

impl<S: Clone + Send + Sync + 'static> ModuleRoutes<S> for OntoCoreModule {
    fn routes(&self) -> Router<S> {
        onto_routes::<S>()
    }

    fn prefix(&self) -> &'static str {
        "/onto"
    }

    fn module_name(&self) -> &'static str {
        "onto.core"
    }
}

/// v1 正式契约 `/onto/v1/*`（含 SSE `/events`，标准 Bearer 鉴权）。
pub struct OntoV1Module;

impl<S: Clone + Send + Sync + 'static> ModuleRoutes<S> for OntoV1Module {
    fn routes(&self) -> Router<S> {
        onto_routes_v1::<S>()
    }

    fn api_doc(&self) -> Option<utoipa::openapi::OpenApi> {
        // 文档与路由同体提供；组合根 merged_openapi() 聚合，漏挂模块 = 路由与文档一起丢。
        Some(crate::openapi::OntoV1ApiDoc::openapi())
    }

    fn prefix(&self) -> &'static str {
        "/onto/v1"
    }

    fn module_name(&self) -> &'static str {
        "onto.v1"
    }
}
