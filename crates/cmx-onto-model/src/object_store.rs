//! O2 对象存储契约（驱动无关）——物化对象读写 + 对象集编译执行 + 关系边 + 聚合。
//!
//! 与 [`OntologyStore`](crate::OntologyStore)（定义层）分离：本契约管**对象运行时**（oo_*/ol_*）。
//! PG 实现见 cmx-onto-store-pg::object_store。

use crate::objectset::*;
use crate::StoreResult;
use async_trait::async_trait;
use serde_json::Value;

/// 关系类型解析回调：给定关系 apiName，返回 (A端对象类型, B端对象类型)。
/// 编译 SearchAround 时需据此定目标表；由定义层 [`OntologyStore`](crate::OntologyStore) 提供。
pub type LinkEnds = (String, String);

/// 对象存储契约。
#[async_trait]
pub trait ObjectStore: Send + Sync {
    /// 确保某对象类型的物化表 `oo_<type>` 就绪（幂等建表 + 按 isIndexed 建索引）。
    /// 关系统一落 `ol_edge`（单表，boot 时建）。
    async fn ensure_object_table(&self, tenant: &str, object_type: &str) -> StoreResult<()>;

    /// upsert 一个对象（按主键；properties 为完整属性 JSON）。
    async fn put_object(
        &self,
        tenant: &str,
        object_type: &str,
        pk: &str,
        title: &str,
        properties: &Value,
    ) -> StoreResult<()>;

    /// 批量 upsert（同一事务，原子）。返回写入行数。
    async fn put_objects(
        &self,
        tenant: &str,
        object_type: &str,
        rows: &[ObjectRecord],
    ) -> StoreResult<u64>;

    /// 删除一个对象（连带清理其关系边）。
    async fn delete_object(&self, tenant: &str, object_type: &str, pk: &str) -> StoreResult<u64>;

    /// 建立一条关系边（幂等）。
    async fn put_link(&self, tenant: &str, edge: &LinkEdge) -> StoreResult<()>;

    /// 删除一条关系边。
    async fn delete_link(&self, tenant: &str, edge: &LinkEdge) -> StoreResult<u64>;

    /// 加载一个对象集（编译代数为一条 SQL 执行；分页）。
    /// `resolve_link`：关系 apiName → (A类型, B类型)，供 SearchAround 定目标表。
    async fn load(
        &self,
        tenant: &str,
        set: &ObjectSet,
        page: &Page,
        links: &dyn LinkResolver,
    ) -> StoreResult<ObjectPage>;

    /// 对一个对象集执行聚合（编译为 SQL 聚合）。
    async fn aggregate(
        &self,
        tenant: &str,
        set: &ObjectSet,
        agg: &Aggregation,
        links: &dyn LinkResolver,
    ) -> StoreResult<Value>;
}

/// 关系两端解析器（编译 SearchAround 用）。同步、廉价——实现通常查一次定义缓存/表。
#[async_trait]
pub trait LinkResolver: Send + Sync {
    /// 关系 apiName → (A端对象类型, B端对象类型)；不存在则 None。
    async fn ends(&self, tenant: &str, link: &str) -> StoreResult<Option<LinkEnds>>;
}
