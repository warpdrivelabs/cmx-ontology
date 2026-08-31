//! OpenAPI 契约 + Swagger UI（O7 headless）。手写但覆盖全端点（离线无 utoipa 构建负担）。

use axum::response::Html;
use axum::Json;
use serde_json::{json, Value};

/// 路径条目：method 列表 → 简述。
fn p(summary: &str, methods: &[&str], tag: &str) -> Value {
    let mut m = serde_json::Map::new();
    for meth in methods {
        m.insert(
            meth.to_string(),
            json!({ "summary": summary, "tags": [tag], "responses": { "200": { "description": "OK" } } }),
        );
    }
    Value::Object(m)
}

/// GET /onto/v1/openapi.json —— 完整契约（免认证，供 OSDK/工具/Swagger 发现端点）。
pub async fn openapi_json() -> Json<Value> {
    Json(json!({
        "openapi": "3.0.3",
        "info": {
            "title": "cmx-ontology · 本体平台 API",
            "version": "0.6.0",
            "description": "Palantir 式企业本体平台。O1 建模 · O2 对象存储 · O4 动作引擎 · O5 函数计算 · O6 动态安全 · O7 headless。"
        },
        "servers": [{ "url": "/api/onto/v1" }],
        "tags": [
            { "name": "建模", "description": "元模型六类元素 CRUD + 发布/版本" },
            { "name": "对象存储", "description": "O2 对象/关系写入 + 对象集加载/聚合" },
            { "name": "动作", "description": "O4 动作执行/试算/审计/Outbox" },
            { "name": "函数", "description": "O5 函数求值" },
            { "name": "安全", "description": "O6 策略 + 带安全的加载" },
            { "name": "实时", "description": "O7 SSE 变更流" }
        ],
        "paths": {
            "/object-types": p("列表 / upsert 对象类型", &["get", "post"], "建模"),
            "/object-types/validate": p("仅校验对象类型", &["post"], "建模"),
            "/object-types/{apiName}": p("详情 / 删除对象类型", &["get", "delete"], "建模"),
            "/link-types": p("列表 / upsert 关系类型", &["get", "post"], "建模"),
            "/link-types/{apiName}": p("详情 / 删除关系类型", &["get", "delete"], "建模"),
            "/interfaces": p("列表 / upsert 接口", &["get", "post"], "建模"),
            "/interfaces/{apiName}": p("详情 / 删除接口", &["get", "delete"], "建模"),
            "/shared-properties": p("列表 / upsert 共享属性", &["get", "post"], "建模"),
            "/shared-properties/{apiName}": p("详情 / 删除共享属性", &["get", "delete"], "建模"),
            "/action-types": p("列表 / upsert 动作类型", &["get", "post"], "建模"),
            "/action-types/{apiName}": p("详情 / 删除动作类型", &["get", "delete"], "建模"),
            "/functions": p("列表 / upsert 函数", &["get", "post"], "建模"),
            "/functions/{apiName}": p("详情 / 删除函数", &["get", "delete"], "建模"),
            "/manifest": p("本体全量清单", &["get"], "建模"),
            "/publish": p("发布快照", &["post"], "建模"),
            "/versions": p("版本列表", &["get"], "建模"),
            "/versions/{version}": p("某版本快照", &["get"], "建模"),
            "/objects/{objectType}": p("upsert 对象", &["post"], "对象存储"),
            "/objects/{objectType}/batch": p("批量 upsert 对象", &["post"], "对象存储"),
            "/objects/{objectType}/{pk}": p("删除对象", &["delete"], "对象存储"),
            "/objects/{objectType}/{pk}/links/{link}": p("Search-Around", &["get"], "对象存储"),
            "/links": p("建 / 删关系边", &["post", "delete"], "对象存储"),
            "/object-sets/load": p("加载对象集（代数编译一条 SQL）", &["post"], "对象存储"),
            "/object-sets/aggregate": p("对象集聚合", &["post"], "对象存储"),
            "/action-types/{apiName}/execute": p("执行动作（校验→事务写回→Outbox）", &["post"], "动作"),
            "/action-types/{apiName}/dry-run": p("动作试算", &["post"], "动作"),
            "/action-logs": p("动作执行审计", &["get"], "动作"),
            "/action-outbox": p("副作用 Outbox", &["get"], "动作"),
            "/action-outbox/{id}/dispatched": p("回标 Outbox 投递", &["post"], "动作"),
            "/functions/{apiName}/evaluate": p("求值函数（FEEL）", &["post"], "函数"),
            "/policies": p("列表 / upsert 策略", &["get", "post"], "安全"),
            "/policies/{apiName}": p("删除策略", &["delete"], "安全"),
            "/secure/object-sets/load": p("带安全的对象集加载（残差+脱敏）", &["post"], "安全"),
            "/events": p("SSE 定义变更流（免认证；?tenant= 过滤）", &["get"], "实时"),
            "/stats": p("建模台计数", &["get"], "建模")
        }
    }))
}

/// GET /onto/v1/docs —— Swagger UI（CDN；免认证）。
pub async fn swagger_ui() -> Html<&'static str> {
    Html(SWAGGER_HTML)
}

const SWAGGER_HTML: &str = r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>cmx-ontology API · Swagger UI</title>
<link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css">
<style>body{margin:0}.topbar{display:none}</style></head>
<body><div id="swagger-ui"></div>
<script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
<script>
window.onload = () => { window.ui = SwaggerUIBundle({
  url: '/api/onto/v1/openapi.json', dom_id: '#swagger-ui',
  presets: [SwaggerUIBundle.presets.apis], layout: 'BaseLayout', deepLinking: true });
};
</script></body></html>"#;
