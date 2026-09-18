//! OpenAPI 契约（O7 headless）：handler `#[utoipa::path]` 注解派生 + 模块切片聚合。
//!
//! **注解范式**（对齐 cmx-mdm / 平台侧既有写法）：每个 handler 自带完整注解
//!（method / path / tag / summary / params / request_body / responses），本文件以
//! `#[derive(OpenApi)]` 收册成切片 [`OntoV1ApiDoc`]——paths 引用未注解 handler 会在
//! 编译期报 `__path_` 符号缺失，天然防漏；「已注册未注解 / 注解未注册 / 路径拼写漂移」
//! 由底部 [`api_contract`] 源码扫描单测常驻守护。
//!
//! 旧前缀 `/onto/*` 是 v1 的兼容镜像（同 inner 表双前缀二重挂载），**不入册**——
//! utoipa `merge` 按 path 浅比较静默丢弃后者，双份入册必丢一份；正式契约只在 v1。
//!
//! 挂载：bin 组合根以 `ModuleSet::merged_openapi(openapi_seed())` 聚合 →
//! `GET /api/onto/v1/openapi.json`（规范）+ `GET /api/onto/v1/docs`（vendored Swagger UI，
//! 免认证、离线可用）。

use utoipa::OpenApi;
use utoipa::openapi::{InfoBuilder, OpenApi as OpenApiDoc, OpenApiBuilder};

/// 本体平台 v1 正式契约切片（`/onto/v1/*` 全部端点）。
#[derive(OpenApi)]
#[openapi(
    paths(
        crate::view_handlers::list_object_types,
        crate::handlers::save_object_type,
        crate::handlers::validate_object_type,
        crate::handlers::get_object_types_batch,
        crate::handlers::get_object_type,
        crate::handlers::delete_object_type,
        crate::handlers::list_link_types,
        crate::handlers::save_link_type,
        crate::handlers::get_link_type,
        crate::handlers::delete_link_type,
        crate::handlers::list_interfaces,
        crate::handlers::save_interface,
        crate::handlers::get_interface,
        crate::handlers::delete_interface,
        crate::handlers::list_shared_properties,
        crate::handlers::save_shared_property,
        crate::handlers::get_shared_property,
        crate::handlers::delete_shared_property,
        crate::view_handlers::get_shared_properties_batch,
        crate::view_handlers::list_views,
        crate::view_handlers::save_view,
        crate::view_handlers::remove_view,
        crate::view_handlers::save_view_layout,
        crate::view_handlers::graph,
        crate::handlers::list_action_types,
        crate::handlers::save_action_type,
        crate::handlers::get_action_type,
        crate::handlers::delete_action_type,
        crate::handlers::list_functions,
        crate::handlers::save_function,
        crate::handlers::get_function,
        crate::handlers::delete_function,
        crate::lifecycle::transition,
        crate::revision_handlers::list_revisions,
        crate::revision_handlers::revision_detail,
        crate::revision_handlers::revert,
        crate::archive_handlers::create_release,
        crate::archive_handlers::remove_release,
        crate::handlers::manifest,
        crate::archive_handlers::create_snapshot,
        crate::handlers::list_versions,
        crate::handlers::get_version,
        crate::archive_handlers::versions_diff,
        crate::archive_handlers::versions_restore,
        crate::archive_handlers::me_roles,
        crate::events::events,
        crate::object_handlers::put_object,
        crate::object_handlers::put_objects_batch,
        crate::object_handlers::delete_object,
        crate::object_handlers::modify_object,
        crate::object_handlers::search_around,
        crate::object_handlers::put_link,
        crate::object_handlers::delete_link,
        crate::object_handlers::load_object_set,
        crate::object_handlers::aggregate_object_set,
        crate::action_handlers::execute_action,
        crate::action_handlers::dry_run_action,
        crate::action_handlers::execute_batch,
        crate::action_handlers::check_permission,
        crate::action_handlers::list_action_logs,
        crate::action_handlers::list_action_outbox,
        crate::action_handlers::outbox_config,
        crate::action_handlers::flow_definitions,
        crate::action_handlers::report_definitions,
        crate::action_handlers::action_templates,
        crate::action_handlers::dispatch_outbox,
        crate::action_handlers::mark_outbox_dispatched,
        crate::function_handlers::evaluate_fn,
        crate::policy_handlers::list_policies,
        crate::policy_handlers::upsert_policy,
        crate::policy_handlers::delete_policy,
        crate::policy_handlers::secure_load,
        crate::funnel_handlers::list_mappings,
        crate::funnel_handlers::upsert_mapping,
        crate::funnel_handlers::delete_mapping,
        crate::funnel_handlers::run_sync,
        crate::funnel_handlers::list_quarantine,
        crate::funnel_handlers::pipeline_status,
        crate::funnel_handlers::funnel_push,
        crate::import_handlers::import_doc,
        crate::import_handlers::import_dct,
        crate::flow_callback_handlers::receive,
        crate::osdk_handlers::typescript_sdk,
        crate::stats::stats,
        // —— 方案 20260918：对象数据源绑定 + 注册表 ——
        crate::source_handlers::get_datasource,
        crate::source_handlers::bind_datasource,
        crate::source_handlers::unbind_datasource,
        crate::source_handlers::list_data_sources,
        crate::source_handlers::create_data_source,
        crate::source_handlers::update_data_source,
        crate::source_handlers::delete_data_source,
        crate::source_handlers::probe_data_source,
        crate::source_handlers::source_schema,
    )
)]
pub struct OntoV1ApiDoc;

/// 组合根聚合种子（info + tags 元数据）。
///
/// **刻意不带 servers**：注解 path 写全路径（含 `/api` 前缀），Swagger UI 以页面
/// origin 直发请求即命中；带 servers 会与之叠加成双前缀（Try-it-out 必 404）。
pub fn openapi_seed() -> OpenApiDoc {
    let tag = |name: &str, desc: &str| {
        utoipa::openapi::tag::TagBuilder::new()
            .name(name)
            .description(Some(desc))
            .build()
    };
    OpenApiBuilder::new()
        .info(
            InfoBuilder::new()
                .title("cmx-ontology · 本体平台 API")
                .version("1.0")
                .description(Some(
                    "Palantir 式企业本体平台对外契约：O1 建模（直改 live 架构，定义表即已发布真源）· \
                     O2 对象存储 · O4 动作引擎 · O5 函数计算 · O6 动态安全 · O7 headless。\
                     响应统一 {code,msg,data} 信封；建模资源七类（object/link/interface/shared_property/\
                     action/function/view）状态流转唯一入口 /lifecycle/transition。\
                     认证：Authorization: Bearer <JWT>；服务间回调走 X-API-Key。\
                     交互文档：GET /api/onto/v1/docs（Swagger UI，免认证）。",
                ))
                .build(),
        )
        .tags(Some(vec![
            tag("建模", "元模型六类元素 CRUD（object/link/interface/shared_property/action/function）"),
            tag("场景", "场景视图（om_view：auto 域默认 / manual 手动物化）+ 本体图"),
            tag("治理", "生命周期流转 / 修订历史 / 存档快照 / 版本与命名发布标记"),
            tag("对象存储", "O2 对象/关系写入 + 对象集加载/聚合（Object Set Service 对等）"),
            tag("动作", "O4 动作执行/试算/批量/审计/Outbox"),
            tag("函数", "O5 函数求值"),
            tag("安全", "O6 动态安全：策略 CRUD + 带安全的对象集加载"),
            tag("集成", "O3 数据集成：源映射 / 全量同步 / 隔离区 / 管道状态 + MDM 事件推送"),
            tag("数据源", "对象数据源绑定（bind/unbind 唯一写入口）+ 数据源注册表（pg/api；probe/结构反射）"),
            tag("实时", "O7 SSE 变更流"),
            tag("工具", "OSDK 代码生成 / 建模台监控数据源"),
        ]))
        .build()
}

// ============================================================================
// 路由 ↔ 文档契约守护（双向防漂移）
// ============================================================================

#[cfg(test)]
mod api_contract {
    //! 守护「路由表 ↔ OpenAPI 文档」双向一致。
    //!
    //! 源码扫描法：[`crate`] 路由表（lib.rs）的 `.route(...)` 字面量（空白归一化后
    //! 解析，兼容多行 `.route(` 与 `get(a).post(b)` 链式），拼 `/api/onto/v1` 前缀后
    //! 与切片文档的 (method, path) 有序对集合做**双向相等**断言。旧前缀 `/onto` 为
    //! 兼容镜像不在文档口径内（见模块注释）；`/events` SSE 双前缀都有，文档按 v1 收册。
    //!
    //! 注意：`/api` 前缀由 server 组合根 `nest("/api", …)` 加，app crate 内测试只能
    //! 硬编码拼接——挂载层级变更时须同步本常量（与 bin 侧 route_contract 探测互补）。

    use utoipa::openapi::PathItem;
    use utoipa::OpenApi as _;

    use super::OntoV1ApiDoc;

    /// v1 对外全路径前缀（server nest "/api" + 本 crate nest "/onto/v1"）。
    const V1_PREFIX: &str = "/api/onto/v1";

    /// 扫描路由注册源码 → [(路径, [METHOD…])]。
    fn scan_routes(src: &str) -> Vec<(String, Vec<String>)> {
        let no_comments: String = src
            .lines()
            .map(|l| match l.find("//") {
                Some(i) => &l[..i],
                None => l,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let norm: String = no_comments.chars().filter(|c| !c.is_whitespace()).collect();
        let b = norm.as_bytes();
        let mut out = Vec::new();
        let mut from = 0usize;
        while let Some(rel) = norm[from..].find(".route(") {
            let open = from + rel + ".route(".len() - 1;
            let q1 = norm[open + 1..].find('"').expect("route 缺路径字面量") + open + 1;
            let q2 = norm[q1 + 1..].find('"').expect("route 路径未闭合") + q1 + 1;
            let path = norm[q1 + 1..q2].to_string();
            let mut depth: i32 = 1;
            let mut i = open + 1;
            let mut methods = Vec::new();
            while i < b.len() && depth > 0 {
                match b[i] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                if depth > 0 && norm.is_char_boundary(i) {
                    for name in ["get", "post", "delete", "put", "patch", "head", "options"] {
                        let ident_before =
                            i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
                        if !ident_before
                            && norm[i..].starts_with(name)
                            && norm[i + name.len()..].starts_with('(')
                        {
                            let m = name.to_ascii_uppercase();
                            if !methods.contains(&m) {
                                methods.push(m);
                            }
                        }
                    }
                }
                i += 1;
            }
            assert_eq!(depth, 0, "route 参数段括号未配对: {path}");
            out.push((path, methods));
            from = i;
        }
        out
    }

    /// 已注册路由 → (METHOD, /api/onto/v1…) 集合。
    fn registered() -> std::collections::BTreeSet<(String, String)> {
        let src = include_str!("lib.rs");
        scan_routes(src)
            .into_iter()
            .flat_map(|(p, ms)| {
                let full = format!("{V1_PREFIX}{p}");
                ms.into_iter().map(move |m| (m, full.clone()))
            })
            .collect()
    }

    /// 已入册文档 → (METHOD, path) 集合。
    fn documented() -> std::collections::BTreeSet<(String, String)> {
        let doc = OntoV1ApiDoc::openapi();
        let mut set = std::collections::BTreeSet::new();
        for (path, item) in doc.paths.paths.iter() {
            for (m, op) in ops(item) {
                if op.is_some() {
                    set.insert((m.to_string(), path.clone()));
                }
            }
        }
        set
    }

    fn ops(item: &PathItem) -> [(&'static str, &Option<utoipa::openapi::path::Operation>); 5] {
        [
            ("GET", &item.get),
            ("POST", &item.post),
            ("DELETE", &item.delete),
            ("PUT", &item.put),
            ("PATCH", &item.patch),
        ]
    }

    /// 扫描基线：73 条路由、17 对双方法 = 93 操作（方案 20260918 +9：绑定三端点 + 注册表六端点）。
    #[test]
    fn scan_count_matches_baseline() {
        let reg = registered();
        assert_eq!(reg.len(), 93, "路由操作扫描数偏离基线 93（73 路径）——路由表变更后须同步 #[utoipa::path] 注解与文档入册");
    }

    /// 双向相等：已注册未注解与已注解未注册都算漂移。
    #[test]
    fn registered_routes_and_docs_are_consistent() {
        let reg = registered();
        let doc = documented();
        let missing: Vec<_> = reg.difference(&doc).collect();
        assert!(
            missing.is_empty(),
            "已注册但未入文档（补 #[utoipa::path] 注解并列入 OntoV1ApiDoc paths）: {missing:?}"
        );
        let extra: Vec<_> = doc.difference(&reg).collect();
        assert!(
            extra.is_empty(),
            "已入文档但未注册（paths 清单含幽灵条目或路径拼写漂移）: {extra:?}"
        );
    }

    /// 文档质量门：每个操作必须有 tag / summary / 200 响应（防语义降档）。
    #[test]
    fn every_operation_has_tag_summary_and_ok() {
        let doc = OntoV1ApiDoc::openapi();
        for (path, item) in doc.paths.paths.iter() {
            for (m, op) in ops(item) {
                // 方法存在性已由 registered_routes_and_docs_are_consistent 校验，此处只查已登记操作的质量。
                let Some(op) = op else { continue };
                assert!(
                    op.tags.as_ref().is_some_and(|t| !t.is_empty()),
                    "{m} {path} 缺 tag"
                );
                assert!(
                    op.summary.as_deref().is_some_and(|s| !s.trim().is_empty()),
                    "{m} {path} 缺 summary"
                );
                assert!(
                    op.responses.responses.contains_key("200"),
                    "{m} {path} 缺 200 响应"
                );
            }
        }
    }
}
