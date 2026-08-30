//! 极简 OpenAPI 契约（O1；O7 headless 阶段换 utoipa 生成完整文档）。

use axum::Json;
use serde_json::{json, Value};

/// GET /onto/v1/openapi.json —— 手写最小契约（免认证，供 OSDK/工具发现端点）。
pub async fn openapi_json() -> Json<Value> {
    Json(json!({
        "openapi": "3.0.3",
        "info": {
            "title": "cmx-ontology · 本体平台 API",
            "version": "0.1.0",
            "description": "Palantir 式企业本体平台（O1 建模引擎）。元模型六类元素 CRUD + 发布/版本。"
        },
        "servers": [{ "url": "/api/onto/v1" }],
        "paths": {
            "/object-types": { "get": {}, "post": {} },
            "/object-types/{apiName}": { "get": {}, "delete": {} },
            "/link-types": { "get": {}, "post": {} },
            "/interfaces": { "get": {}, "post": {} },
            "/shared-properties": { "get": {}, "post": {} },
            "/action-types": { "get": {}, "post": {} },
            "/functions": { "get": {}, "post": {} },
            "/manifest": { "get": {} },
            "/publish": { "post": {} },
            "/versions": { "get": {} },
            "/versions/{version}": { "get": {} }
        }
    }))
}
