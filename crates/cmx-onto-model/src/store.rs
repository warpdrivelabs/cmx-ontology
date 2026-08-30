//! 驱动无关的持久化契约（内核不认识任何数据库；PG 实现见 cmx-onto-store-pg）。
//!
//! 对标 cmx-rule-model 的 `DecisionStore` / cmx-flow-model 的 `RuntimeStore`。O1 覆盖六类元模型
//! 元素的定义读写 + 全量清单。发布/版本为 store 实现的 inherent 方法（非本 trait），因其涉及
//! 跨表快照，随后端而异。所有方法按租户隔离（单租户传 "default"；multi 下库即租户边界）。

use crate::def::*;
use crate::StoreResult;
use async_trait::async_trait;

/// 本体定义的存储契约。
#[async_trait]
pub trait OntologyStore: Send + Sync {
    // ── 对象类型 ──
    async fn upsert_object_type(&self, tenant: &str, def: &ObjectTypeDef) -> StoreResult<()>;
    async fn get_object_type(&self, tenant: &str, api_name: &str)
        -> StoreResult<Option<ObjectTypeDef>>;
    async fn list_object_types(&self, tenant: &str) -> StoreResult<Vec<ObjectTypeMeta>>;
    async fn delete_object_type(&self, tenant: &str, api_name: &str) -> StoreResult<u64>;

    // ── 关系类型 ──
    async fn upsert_link_type(&self, tenant: &str, def: &LinkTypeDef) -> StoreResult<()>;
    async fn get_link_type(&self, tenant: &str, api_name: &str) -> StoreResult<Option<LinkTypeDef>>;
    async fn list_link_types(&self, tenant: &str) -> StoreResult<Vec<LinkTypeMeta>>;
    async fn delete_link_type(&self, tenant: &str, api_name: &str) -> StoreResult<u64>;

    // ── 接口 ──
    async fn upsert_interface(&self, tenant: &str, def: &InterfaceDef) -> StoreResult<()>;
    async fn get_interface(&self, tenant: &str, api_name: &str) -> StoreResult<Option<InterfaceDef>>;
    async fn list_interfaces(&self, tenant: &str) -> StoreResult<Vec<SimpleTypeMeta>>;
    async fn delete_interface(&self, tenant: &str, api_name: &str) -> StoreResult<u64>;

    // ── 共享属性类型 ──
    async fn upsert_shared_property(&self, tenant: &str, def: &SharedPropertyTypeDef)
        -> StoreResult<()>;
    async fn get_shared_property(&self, tenant: &str, api_name: &str)
        -> StoreResult<Option<SharedPropertyTypeDef>>;
    async fn list_shared_properties(&self, tenant: &str) -> StoreResult<Vec<SimpleTypeMeta>>;
    async fn delete_shared_property(&self, tenant: &str, api_name: &str) -> StoreResult<u64>;

    // ── 动作类型 ──
    async fn upsert_action_type(&self, tenant: &str, def: &ActionTypeDef) -> StoreResult<()>;
    async fn get_action_type(&self, tenant: &str, api_name: &str)
        -> StoreResult<Option<ActionTypeDef>>;
    async fn list_action_types(&self, tenant: &str) -> StoreResult<Vec<SimpleTypeMeta>>;
    async fn delete_action_type(&self, tenant: &str, api_name: &str) -> StoreResult<u64>;

    // ── 函数 ──
    async fn upsert_function(&self, tenant: &str, def: &FunctionDef) -> StoreResult<()>;
    async fn get_function(&self, tenant: &str, api_name: &str) -> StoreResult<Option<FunctionDef>>;
    async fn list_functions(&self, tenant: &str) -> StoreResult<Vec<SimpleTypeMeta>>;
    async fn delete_function(&self, tenant: &str, api_name: &str) -> StoreResult<u64>;

    // ── 全量清单 ──
    async fn manifest(&self, tenant: &str) -> StoreResult<OntologyManifest>;
}
