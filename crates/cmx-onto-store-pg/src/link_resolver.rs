//! [`LinkResolver`] 的 PG 实现——查 `om_link_type` 得关系两端（编译 SearchAround 用）。
//!
//! 复用定义层存储 [`PgOntologyStore`](crate::PgOntologyStore)：关系 apiName → (A端类型, B端类型)。

use async_trait::async_trait;
use cmx_onto_model::{LinkEnds, LinkResolver, OntologyStore, StoreResult};

use crate::PgOntologyStore;

/// 经定义层查关系两端的解析器。构造廉价（裹 db_id）。
#[derive(Clone)]
pub struct PgLinkResolver {
    store: PgOntologyStore,
}

impl PgLinkResolver {
    pub fn new(db_id: impl Into<String>) -> Self {
        Self { store: PgOntologyStore::new(db_id) }
    }
}

#[async_trait]
impl LinkResolver for PgLinkResolver {
    async fn ends(&self, tenant: &str, link: &str) -> StoreResult<Option<LinkEnds>> {
        Ok(self
            .store
            .get_link_type(tenant, link)
            .await?
            .map(|lt| (lt.object_type_a, lt.object_type_b)))
    }
}
