//! O7 实时变更流（SSE）：进程内广播本体定义/发布变更，供工具/前端实时感知。
//!
//! 轻量：`tokio::sync::broadcast` 单例通道；写路径（publish 等）调 `emit` 投递事件，
//! `/events` 端点把订阅转成 SSE。按 `tenant` 过滤（多租户隔离）。无持久化（掉线丢事件，
//! 需可靠投递走 O4 Outbox / webhook）。

use axum::extract::Query;
use axum::response::sse::{Event, Sse};
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::OnceLock;
use tokio::sync::broadcast;

/// 一条变更事件。
#[derive(Debug, Clone)]
pub struct ChangeEvent {
    pub tenant: String,
    /// 事件类型：published / object-type-changed / policy-changed …
    pub kind: String,
    pub payload: Value,
}

fn channel() -> &'static broadcast::Sender<ChangeEvent> {
    static CH: OnceLock<broadcast::Sender<ChangeEvent>> = OnceLock::new();
    CH.get_or_init(|| broadcast::channel(256).0)
}

/// 投递一条变更事件（写路径调用；无订阅者时静默丢弃）。
pub fn emit(tenant: &str, kind: &str, payload: Value) {
    let _ = channel().send(ChangeEvent {
        tenant: tenant.to_string(),
        kind: kind.to_string(),
        payload,
    });
}

/// SSE 订阅参数：?tenant= 过滤（缺省全租户）。
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct EventsQuery {
    pub tenant: Option<String>,
}

/// GET /onto/v1/events —— SSE 变更流（免认证，挂文档层；按 tenant 过滤）。
///
/// spawn 专用任务持 broadcast Receiver 全程，转发到 mpsc（rx 生命周期独立于 HTTP 流轮询，
/// 避免流首帧后被 drop 导致 receiver_count=0 收不到后续事件）。
pub async fn events(Query(q): Query<EventsQuery>) -> impl IntoResponse {
    let mut rx = channel().subscribe();
    let want = q.tenant;
    let (tx, mut mrx) = tokio::sync::mpsc::channel::<Event>(64);
    // 首帧 connected
    let _ = tx.try_send(Event::default().event("connected").data("{\"ok\":true}"));
    // 专用任务：持 rx 直到连接关闭（tx 关闭）
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if let Some(t) = &want {
                        if &ev.tenant != t { continue; }
                    }
                    let data = json!({ "tenant": ev.tenant, "kind": ev.kind, "payload": ev.payload });
                    if tx.send(Event::default().event(ev.kind).data(data.to_string())).await.is_err() {
                        break; // 客户端断开
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    let stream = async_stream::stream! {
        while let Some(ev) = mrx.recv().await {
            yield Ok::<Event, Infallible>(ev);
        }
    };
    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}
