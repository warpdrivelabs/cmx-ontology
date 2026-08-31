/*
 * cmx-onto 独立本体平台微服务 HTTP 服务器。
 *
 * 采用通用骨架 cmx-web-chassis：main 只填 ServiceSpec——onto 路由 + 两个启动钩子（注册数据源、
 * 建表预热）+ onto 专属 banner/配色，交 chassis::run 装配。零 cmx-api 依赖。**无 poller**（本体
 * 建模无长驻实例/定时器）——钩子②只建表预热，纯请求驱动。
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

use cmx_onto_app::{
    onto_routes, onto_routes_v1, openapi_json, warm_object_store, warm_store, ONTO_DB_ID,
};
use cmx_web_chassis::{run, BannerSpec, ChassisConfig, ServiceSpec};

/// onto 专属字符画。
const ONTO_ART: &str = r#"
 ██████╗ ███╗   ██╗████████╗ ██████╗ ██╗      ██████╗  ██████╗ ██╗   ██╗
██╔═══██╗████╗  ██║╚══██╔══╝██╔═══██╗██║     ██╔═══██╗██╔════╝ ╚██╗ ██╔╝
██║   ██║██╔██╗ ██║   ██║   ██║   ██║██║     ██║   ██║██║  ███╗ ╚████╔╝
██║   ██║██║╚██╗██║   ██║   ██║   ██║██║     ██║   ██║██║   ██║  ╚██╔╝
╚██████╔╝██║ ╚████║   ██║   ╚██████╔╝███████╗╚██████╔╝╚██████╔╝   ██║
 ╚═════╝ ╚═╝  ╚═══╝   ╚═╝    ╚═════╝ ╚══════╝ ╚═════╝  ╚═════╝    ╚═╝
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

    // 路由：v1 正式契约 + 旧前缀（内嵌壳兼容），经监控遥测 + 认证中间件。
    let authed = onto_routes_v1::<()>()
        .merge(onto_routes::<()>())
        .layer(axum::middleware::from_fn(cmx_web_monitor::observe))
        .layer(axum::middleware::from_fn(cmx_onto_app::auth_middleware));
    // 公开文档（免认证，挂认证之外）。
    let api_router = axum::Router::new()
        .merge(authed)
        // 前端页只读投递（native；门户 F3 反代 portal.onto.* 取页请求到此，免认证——静态内容 +
        // 门户反代注入服务身份）。挂认证之外、与 openapi 同层，得 /api/native-pages/*。
        // 本体平台仅 native 页（HtmlLayout::Disabled）：四区本体设计工作台 + <cmx-ontology-graph> 组件 vendor。
        .merge(cmx_form::serve::frontend_pages_routes::<(), cmx_onto_app::OntoError>(
            cmx_form::serve::PageServeConfig {
                html: cmx_form::serve::HtmlLayout::Disabled,
                ..cmx_form::serve::PageServeConfig::from_assets()
            },
        ))
        .route("/onto/v1/openapi.json", axum::routing::get(openapi_json))
        // O7 headless：Swagger UI（/api/onto/v1/docs）+ SSE 变更流（/api/onto/v1/events）——免认证层。
        .route("/onto/v1/docs", axum::routing::get(cmx_onto_app::swagger_ui))
        .route("/onto/v1/events", axum::routing::get(cmx_onto_app::sse_events));
    let app_router = axum::Router::new()
        // 根 → 本体建模控制台（免认证，前端 fetch /api/onto/v1/*）。
        .route("/", axum::routing::get(cmx_onto_app::dashboard::dashboard))
        .nest("/api", api_router);

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
        // 钩子② 建表预热（**无 poller**）。DB 不可达已在钩子① 探活 fail-fast；此处失败同样终止启动。
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
        });

    let result = run(spec).await;
    // serve 结束：注销注册中心实例后再返回（不用 `?` 提前返回，否则 Err 路径会跳过注销）。
    cmx_service_base::shutdown_infra().await;
    result
}
