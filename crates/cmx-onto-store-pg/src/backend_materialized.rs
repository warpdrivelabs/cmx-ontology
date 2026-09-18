//! MaterializedBackend —— 内置物化表 `oo_<Type>` 读路径收编（方案 20260918 §5.3，默认 backend）。
//!
//! **只包不改**：现有 `PgObjectStore::load/aggregate`（整树单 SQL 快路径，compile.rs 编译）原样挂到
//! 统一 trait 后面——未绑定对象类型、演示造数（/objects/{type}/batch 直灌）、漏斗同步后的读取
//! 全部走它，行为零变化（存量 21 个 e2e 全绿为准出）。resolve_pks（桥接用）以分页循环拉取，
//! 不放宽单页 clamp 1000 的既有防护。
//!
//! 无状态：每请求按 `ctx.onto_db_id` 构造轻量 store（仅 String clone，构造廉价——engine.rs 同款）。

use async_trait::async_trait;
use cmx_onto_model::backend::{BackendCaps, BackendCtx, BackendKind, ObjectDataBackend, PK_BRIDGE_MAX};
use cmx_onto_model::objectset::{Aggregation, ObjectPage, ObjectSet, Page};
use cmx_onto_model::{ObjectStore, ProbeReport, StoreError, StoreResult};
use serde_json::Value;

use crate::{PgLinkResolver, PgObjectStore};

/// 内置物化读路径（默认；行为与改造前完全一致）。
#[derive(Default)]
pub struct MaterializedBackend;

impl MaterializedBackend {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ObjectDataBackend for MaterializedBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Materialized
    }

    fn caps(&self) -> BackendCaps {
        BackendCaps::materialized()
    }

    async fn load(
        &self,
        ctx: &BackendCtx,
        set: &ObjectSet,
        page: &Page,
    ) -> StoreResult<ObjectPage> {
        let os = PgObjectStore::new(ctx.onto_db_id.clone());
        let lr = PgLinkResolver::new(ctx.onto_db_id.clone());
        os.load(&ctx.tenant, set, page, &lr).await
    }

    async fn aggregate(
        &self,
        ctx: &BackendCtx,
        set: &ObjectSet,
        agg: &Aggregation,
    ) -> StoreResult<Value> {
        let os = PgObjectStore::new(ctx.onto_db_id.clone());
        let lr = PgLinkResolver::new(ctx.onto_db_id.clone());
        os.aggregate(&ctx.tenant, set, agg, &lr).await
    }

    /// 解析 pk 集合（分派器桥接用）：分页循环拉取到 `max` 或取尽；超 `max` 拒绝（fail-closed，
    /// 与 PgDirect 同口径）。复用既有 load（clamp 1000 防护不变，R4 行为零变化）。
    async fn resolve_pks(
        &self,
        ctx: &BackendCtx,
        set: &ObjectSet,
        max: u32,
    ) -> StoreResult<Vec<String>> {
        let os = PgObjectStore::new(ctx.onto_db_id.clone());
        let lr = PgLinkResolver::new(ctx.onto_db_id.clone());
        let mut out = Vec::new();
        let mut offset = 0u32;
        let page_size = 1000u32;
        loop {
            let page = os
                .load(&ctx.tenant, set, &Page { limit: page_size, offset }, &lr)
                .await?;
            let got = page.rows.len();
            out.extend(page.rows.into_iter().map(|r| r.pk));
            if out.len() > max as usize {
                return Err(StoreError::Backend(format!(
                    "对象集 pk 集合超出桥接上限（>{max}）：请收紧过滤条件（集合运算/关系遍历跨 backend 组合上限护栏）"
                )));
            }
            if got < page_size as usize || !page.has_more || out.len() >= max as usize {
                return Ok(out);
            }
            offset += page_size;
        }
    }

    /// 物化路径无外部源：探测 = 本体库连通 + 固定摘要。
    async fn probe(&self, source_db_id: &str, _resource: Option<&str>) -> StoreResult<ProbeReport> {
        let os = PgObjectStore::new(source_db_id.to_string());
        os.ensure_edge_table().await?;
        Ok(ProbeReport {
            reachable: true,
            detail: "内置物化表（oo_<Type> / ol_edge，本体库）".into(),
            columns: None,
        })
    }
}

/// 分派器桥接上限引用（文档集中）。
pub const BRIDGE_MAX: u32 = PK_BRIDGE_MAX as u32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_declare_full_algebra() {
        let b = MaterializedBackend::new();
        assert_eq!(b.kind(), BackendKind::Materialized);
        let caps = b.caps();
        assert!(matches!(caps.algebra, cmx_onto_model::backend::AlgebraCaps::Full));
        assert!(caps.supports(cmx_onto_model::backend::PredicateKind::Contains));
        assert!(!caps.sortable, "M1 恒固定排序（E4）");
        assert_eq!(BRIDGE_MAX, 2000);
    }
}
