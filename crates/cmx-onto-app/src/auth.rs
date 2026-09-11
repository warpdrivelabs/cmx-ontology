//! 认证中间件——复用 `cmx-engine-kit::auth::jwt`（唯一真源）。
//!
//! 本仓包装器注入多租户懒备库就绪钩子（[`crate::tenancy::ensure_current_ready`]，在租户 scope 内、
//! handler 前执行）。off 模式吃 `X-Tenant` / `X-User` 头；jwt 模式验 Bearer JWT。签名不变。

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

use cmx_engine_kit::auth::jwt::{self, JwtSpec};

/// 本仓专属参数：无 SSE 票据路径（空表）——`/events` 走标准 Bearer 鉴权，前端以
/// fetch 流式读取（自带 Authorization header）消费 SSE，不用 EventSource。
static SPEC: JwtSpec = JwtSpec::new("onto", &[], None);

/// 认证中间件（建租户 scope + 确保租户库就绪后放行；签名不变）。
pub async fn auth(req: Request, next: Next) -> Response {
    jwt::auth_mw_with_ready(req, next, &SPEC, || async {
        // multi 模式懒备库（幂等去重；内部失败仅 warn，维持既有非致命语义）。
        crate::tenancy::ensure_current_ready().await;
    })
    .await
}
