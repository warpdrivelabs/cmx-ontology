/*
 * cmx-onto 独立本体平台微服务 HTTP 服务器。
 *
 * 采用通用骨架 cmx-web-chassis：main 只填 ServiceSpec——onto 路由 + 三个启动钩子（注册数据源、
 * 建表预热、拉起 Outbox 定时投递）+ onto 专属 banner/配色，交 chassis::run 装配。零 cmx-api 依赖。
 * 唯一长驻任务 = Outbox 定时投递（动作副作用出站自动挡）；本体建模本身纯请求驱动。
 *
 * 配置（onto-server.toml，路径由 CONFIG_FILE 指定；[server] 框架键 env 覆盖 SERVER__*）：
 *   [server] host/port/log_dir/log_level/graceful_timeout_secs（默认 0.0.0.0:8097）
 *   [[databases]] 标准数据源段（db_id = ONTO_DB_ID = "onto_pg"，default=true；缺段启动失败）
 *   [auth] 段 → cmx-onto-app 认证中间件 ConfigManager 直读（env 覆盖 AUTH__*）
 *
 * 用法：
 *   CONFIG_FILE=onto-server-dev.toml cargo run -p cmx-onto-server   # 真机开发库
 *   浏览器打开 http://127.0.0.1:8097/  进入本体建模控制台
 */

use cmx_form::serve::{FormPagesModule, PageServeConfig};
use cmx_onto_app::{
    ModuleSet, OntoCoreModule, OntoError, OntoV1Module, openapi_json, warm_object_store,
    warm_store, ONTO_DB_ID,
};
use cmx_web_chassis::{run, BannerSpec, ChassisConfig, ServiceSpec};

/// onto 专属字符画。
const ONTO_ART: &str = r#"
████████╗██████╗ ██╗   ██╗███████╗     ██████╗ ███╗   ██╗████████╗ ██████╗ 
╚══██╔══╝██╔══██╗██║   ██║██╔════╝    ██╔═══██╗████╗  ██║╚══██╔══╝██╔═══██╗
   ██║   ██████╔╝██║   ██║█████╗      ██║   ██║██╔██╗ ██║   ██║   ██║   ██║
   ██║   ██╔══██╗██║   ██║██╔══╝      ██║   ██║██║╚██╗██║   ██║   ██║   ██║
   ██║   ██║  ██║╚██████╔╝███████╗    ╚██████╔╝██║ ╚████║   ██║   ╚██████╔╝
   ╚═╝   ╚═╝  ╚═╝ ╚═════╝ ╚══════╝     ╚═════╝ ╚═╝  ╚═══╝   ╚═╝    ╚═════╝ 
"#;

#[tokio::main]
async fn main() -> cmx_web_chassis::Result<()> {
    dotenvy::dotenv().ok();
    // 基础设施装配（三源 ConfigManager + 注册中心客户端；开关默认全关，走 Mock 纯本地 toml+env）。
    cmx_service_base::init_infra()
        .await
        .map_err(|e| cmx_web_chassis::ChassisError::Config(format!("基础设施初始化失败: {e}")))?;

    let mut cfg = ChassisConfig::load("onto", "onto-server.toml");
    if std::env::var("SERVER__PORT").is_err() && cfg.port == 8080 {
        cfg.port = 8097; // 本体平台默认端口（承 meta-data 8096 之后）。
    }

    let banner = BannerSpec::defaults("onto")
        .art(ONTO_ART)
        .tagline("  ONTOLOGY · Palantir 式企业本体平台 · cmx-web-chassis ")
        .stops(vec![(34, 211, 238), (99, 102, 241), (168, 85, 247)]);

    // 路由：模块化装配（authed / open 双切片 + api_router 级公开文档）见 [`build_app_router`]。
    let app_router = build_app_router();

    // 技术监控（/_mon）：注入身份读取器 + 拓扑（onto 自身即引擎，内嵌）。
    cmx_web_monitor::set_service_name("cmx-ontology 本体平台");
    cmx_web_monitor::set_identity_provider(cmx_onto_app::identity_snapshot);
    cmx_web_monitor::set_topology_provider(|| {
        vec![cmx_web_monitor::ServiceDep {
            key: "onto".into(),
            label: "本体平台".into(),
            mode: "embedded".into(),
            target: None,
            proxiable: true,
        }]
    });

    let spec = ServiceSpec::<()>::new("onto", cfg)
        .banner(banner)
        .nest_api(false) // 已自行 nest /api，让根控制台 / 逃出 /api。
        .router(app_router)
        .state(())
        // 钩子① 注册数据源（标准 [[databases]] 段；要求 db_id = ONTO_DB_ID，缺段/缺 db_id/库不可达 fail-fast）。
        .init("datasources", |_meta| {
            Box::pin(async {
                let base = cmx_service_base::BaseConfig::from_config_manager()
                    .map_err(|e| anyhow::anyhow!("读取 [[databases]] 配置失败: {e}"))?;
                cmx_service_base::validate_databases(
                    &base.databases,
                    &cmx_service_base::DatasourceRules {
                        required_db_ids: &[ONTO_DB_ID],
                        ..Default::default()
                    },
                )
                .map_err(|e| {
                    anyhow::anyhow!("数据源校验失败（需 db_id=\"{ONTO_DB_ID}\"，本体 store 按该 db_id 寻址）: {e}")
                })?;
                let ids: Vec<&str> = base.databases.iter().map(|d| d.db_id.as_str()).collect();
                cmx_service_base::register_pg_datasources(&base.databases)
                    .await
                    .map_err(|e| anyhow::anyhow!("注册数据源失败: {e}"))?;
                tracing::info!(databases = ?ids, "✅ 本体平台 tokio-pg 数据源已注册（[[databases]] 配置驱动）");
                Ok(())
            })
        })
        // 钩子② 建表预热。DB 不可达已在钩子① 探活 fail-fast；此处失败同样终止启动。
        .init("store", |_meta| {
            Box::pin(async {
                warm_store()
                    .await
                    .map_err(|e| anyhow::anyhow!("本体存储初始化失败: {e}"))?;
                // O2 对象层：建 ol_edge 关系边表（per-type oo_* 表按需惰性建）。
                warm_object_store()
                    .await
                    .map_err(|e| anyhow::anyhow!("对象存储初始化失败: {e}"))?;
                Ok(())
            })
        })
        // 钩子③ Outbox 定时投递：动作副作用出站自动挡（与手动 POST /action-outbox/dispatch
        // 共用一条投递路径）。间隔 onto.outbox_dispatch_secs（缺省 10s，0=关）；
        // ONTO_OUTBOUND=off 整轮跳过；SKIP LOCKED 认领，多实例部署安全。
        .init("outbox-dispatcher", |_meta| {
            Box::pin(async {
                cmx_onto_app::action_handlers::spawn_outbox_dispatcher();
                Ok(())
            })
        });

    let result = run(spec).await;
    // serve 结束：注销注册中心实例后再返回（不用 `?` 提前返回，否则 Err 路径会跳过注销）。
    cmx_service_base::shutdown_infra().await;
    result
}

// ============================================================================
// bin 组合根装配（模块化）
// ============================================================================

/// authed 切片：本体业务路由（v1 正式契约 + 旧前缀，同 inner 表双前缀）。
///
/// 返回**未加层**的路由器——main 按现状序「observe（内）→ auth（外）」加层；契约测试
/// 直接探测本函数（auth 中间件对无凭证请求统一 401，会掩盖 405/404 区分）。
fn build_authed_router() -> axum::Router {
    ModuleSet::<()>::new(vec![])
        // v1 在前、旧前缀在后：与改造前 `onto_routes_v1().merge(onto_routes())` 顺序一致。
        .with(Box::new(OntoV1Module))
        .with(Box::new(OntoCoreModule))
        .fold()
}

/// open 切片：前端页只读投递（native；门户 F3 反代 portal.onto.* 取页请求到此，免认证）。
///
/// 挂认证之外、与 openapi 同层；**现状无任何中间件层，保持**。本体平台仅 native 页
/// （HtmlLayout::Disabled）：四区本体设计工作台 + `<cmx-ontology-graph>` 组件 vendor；
/// 错误体经 OntoError 保持历史语义。
fn build_open_router() -> axum::Router {
    ModuleSet::<()>::new(vec![]).with(Box::new(FormPagesModule::<OntoError>::new(
        PageServeConfig {
            html: cmx_form::serve::HtmlLayout::Disabled,
            ..PageServeConfig::from_assets()
        },
    ))).fold()
}

/// 全量装配：根级控制台 + `/api`（authed 切片 + open 切片 + 公开文档）。
///
/// openapi.json 与 Swagger UI（O7）挂在 api_router 上、authed 子树之外（URL 含 `/api`
/// 前缀，免认证公开文档），**不得挪到 app_router 根**（会丢 `/api` 前缀致消费方 404）。
fn build_app_router() -> axum::Router {
    let authed = build_authed_router()
        .layer(axum::middleware::from_fn(cmx_web_monitor::observe))
        .layer(axum::middleware::from_fn(cmx_onto_app::auth_middleware));
    let api_router = axum::Router::new()
        .merge(authed)
        .merge(build_open_router())
        .route("/onto/v1/openapi.json", axum::routing::get(openapi_json))
        // O7 headless：Swagger UI 免认证层。SSE（/events）P2 起注册进鉴权路由
        //（onto_routes_v1 内；标准 Bearer 鉴权，前端以 fetch 流式读取消费 SSE）——
        // 原免认证挂载移除（方案 §七）。
        .route("/onto/v1/docs", axum::routing::get(cmx_onto_app::swagger_ui));
    axum::Router::new()
        // 根 → 本体建模控制台（免认证，前端 fetch /api/onto/v1/*）。
        .route("/", axum::routing::get(cmx_onto_app::dashboard::dashboard))
        .nest("/api", api_router)
}

// ============================================================================
// 路由契约守护（bin 装配级）
// ============================================================================

#[cfg(test)]
mod route_contract {
    //! 静态清单以改造前 main.rs 逐条抄录（改造后不变即零回归）。
    //!
    //! 探测法：以 **OPTIONS** 探测——命中已有路径返回 405（方法不符），未命中 404；
    //! 不触发任何 handler。authed 切片在加层前探测（原因见 [`super::build_authed_router`]）。

    use super::{build_app_router, build_authed_router, build_open_router};
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// authed 切片（改造前 main.rs 挂载清单抽样：v1 SSE 端点，标准 Bearer 鉴权）。
    const AUTHED: &[&str] = &["/onto/v1/events"];

    /// open / 根级（改造前 main.rs 挂载清单：控制台、公开文档、页面端点）。
    const OPEN_OR_ROOT: &[&str] = &[
        "/",
        "/api/onto/v1/openapi.json",
        "/api/onto/v1/docs",
        "/api/native-pages",
        "/api/native-pages/probe-id",
    ];

    async fn probe(router: Router, method: &str, path: &str) -> StatusCode {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        router.oneshot(req).await.unwrap().status()
    }

    #[tokio::test]
    async fn authed_paths_mounted() {
        let router = build_authed_router();
        for path in AUTHED {
            let status = probe(router.clone(), "OPTIONS", path).await;
            assert_ne!(status, StatusCode::NOT_FOUND, "authed 路径丢失: {path}");
        }
    }

    #[tokio::test]
    async fn open_and_root_paths_mounted() {
        let router = build_app_router();
        for path in OPEN_OR_ROOT {
            let status = probe(router.clone(), "OPTIONS", path).await;
            assert_ne!(status, StatusCode::NOT_FOUND, "open/根级路径丢失: {path}");
        }
    }

    #[tokio::test]
    async fn open_slice_mounts_without_auth_layers() {
        // 防误把页面投递挂进 authed（免认证面被收窄属行为回归）。
        let status = probe(build_open_router(), "OPTIONS", "/native-pages").await;
        assert_ne!(status, StatusCode::NOT_FOUND, "open 切片路径丢失");
    }

    #[tokio::test]
    async fn unknown_path_is_404() {
        let router = build_app_router();
        for path in ["/api/__definitely_absent__", "/__definitely_absent__"] {
            let status = probe(router.clone(), "OPTIONS", path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "未注册路径竟命中: {path}");
        }
    }
}
