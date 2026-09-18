//! O2+ 对象数据后端统一契约（方案 20260918 §5.2）：内置物化 / PG 直连虚拟直查 / REST API / 连接器
//! 四类实现共用一个 trait——**读路径来源**的统一抽象（方案 D1/D2/D3）。
//!
//! 契约分层（A-P1-3）：backend 只对自己声明的算子子集负责——
//! - `Materialized` 吃**完整代数树**（整树单 SQL 快路径，收编 compile.rs 零重写）；
//! - `PgDirect`（及 M2 `RestApi`）只吃 `Base/Filter/Static` 子树；
//! - **SearchAround 与集合并交差留在分派器**（app 层）：桥接 = virtual 子树先解析 pk 集合
//!   （上限 [`PK_BRIDGE_MAX`]），替换为 `Static` 子集下推（`= IN (VALUES …)` 形态，与编译器
//!   Static 编译一致）。**不做 join 下推的跨源联邦**（pk 集合组合是受限组合）。
//!
//! 能力矩阵 [`BackendCaps`]：分派器据此预校验，不支持即整查询拒绝（fail-closed，绝不静默全量拉取）。
//! 本 crate 零 IO：trait 实现放 store-pg（Materialized/PgDirect）与 app（M2 RestApi）。

use crate::funnel::SourceMapping;
use crate::def::ObjectTypeDef;
use crate::objectset::{Aggregation, ObjectPage, ObjectSet, Page};
use crate::{StoreResult, DEFAULT_TENANT};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// pk 桥接集合上限（方案 Q1 定值）：`= IN (VALUES …)` 逐参绑定，PG 参数上限充裕。
pub const PK_BRIDGE_MAX: usize = 2000;

/// 后端类型（读路径来源）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum BackendKind {
    /// 内置物化表 `oo_<Type>`（默认；未绑定对象类型的兜底）。
    #[default]
    Materialized,
    /// PG 直连虚拟直查（查询下推源库；仅 PG，方案 D2）。
    PgDirect,
    /// REST API 结构化查询协议（业务系统转 SQL；M2 落地）。
    RestApi,
    /// 连接器扩展点（es/file/mq；仅预留不实装）。
    Connector,
}

/// 支持的对象集算子面。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AlgebraCaps {
    /// 完整代数树（Base/Filter/SearchAround/集合运算/Static）。
    #[default]
    Full,
    /// 仅 Base/Filter/Static 子树（SearchAround 与集合运算由分派器桥接）。
    BaseFilterStatic,
}

/// total 计数三档（方案 B-P2-6：大表 count 代价治理；虚拟类型默认不拉 total）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TotalMode {
    /// 精确 count。
    #[default]
    Exact,
    /// 估算（reltuples 类）。
    Estimated,
    /// 不提供。
    None,
}

/// 聚合算子。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AggKind {
    Count,
    GroupCount,
    GroupSum,
}

/// 谓词算子（从 objectset.rs 真实谓词集出发；无 Like——Contains 对应 LIKE 由实现自行翻译）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PredicateKind {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    In,
    Contains,
    IsNull,
    And,
    Or,
    Not,
}

/// 后端能力矩阵：分派器据此预校验，不支持即整查询拒绝（fail-closed）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendCaps {
    /// 支持的对象集算子面。
    pub algebra: AlgebraCaps,
    /// 支持的过滤谓词算子。
    pub filter_ops: Vec<PredicateKind>,
    /// 是否支持任意属性排序。M1 恒 false（E4：固定 title,pk 排序口径跨 backend 对齐）。
    pub sortable: bool,
    /// 单页行数上限（对齐现状 clamp 1000）。
    pub page_max: u32,
    /// total 计数档位。
    pub total_mode: TotalMode,
    /// 支持的聚合算子。
    pub aggregates: Vec<AggKind>,
}

impl BackendCaps {
    /// 物化（默认）能力：全代数 + 全谓词 + Exact total + 三聚合。
    pub fn materialized() -> Self {
        Self {
            algebra: AlgebraCaps::Full,
            filter_ops: vec![
                PredicateKind::Eq, PredicateKind::Ne, PredicateKind::Gt, PredicateKind::Ge,
                PredicateKind::Lt, PredicateKind::Le, PredicateKind::In, PredicateKind::Contains,
                PredicateKind::IsNull, PredicateKind::And, PredicateKind::Or, PredicateKind::Not,
            ],
            sortable: false,
            page_max: 1000,
            total_mode: TotalMode::Exact,
            aggregates: vec![AggKind::Count, AggKind::GroupCount, AggKind::GroupSum],
        }
    }

    /// PG 直连虚拟直查能力：Base/Filter/Static 子树 + 全谓词（物理列编译）。
    pub fn pg_direct() -> Self {
        Self { algebra: AlgebraCaps::BaseFilterStatic, ..Self::materialized() }
    }

    /// 该谓词算子是否受支持。
    pub fn supports(&self, kind: PredicateKind) -> bool {
        self.filter_ops.contains(&kind)
    }
}

/// 分派上下文：一次虚拟/物化装载的权威输入（绑定行 = om_source_mapping，E1 唯一真源）。
#[derive(Debug, Clone)]
pub struct BackendCtx {
    /// 租户（db-per-tenant 寻址；single 恒 default）。
    pub tenant: String,
    /// 本体库 db_id（物化路径用；虚拟路径写 oo_/隔离区也走它）。
    pub onto_db_id: String,
    /// 对象类型定义。
    pub def: ObjectTypeDef,
    /// 绑定行（mode=virtual 时 resource/source 映射是下推依据；materialized 时忽略）。
    pub mapping: SourceMapping,
}

impl Default for BackendCtx {
    fn default() -> Self {
        Self {
            tenant: DEFAULT_TENANT.to_string(),
            onto_db_id: String::new(),
            def: ObjectTypeDef::default(),
            mapping: SourceMapping::default(),
        }
    }
}

/// 探测报告（连通性 + 源结构基线；probe 结果落 om_data_source.probe_report / 映射报告）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProbeReport {
    /// 连通是否成功。
    pub reachable: bool,
    /// 摘要信息（版本/库 Ident 等；不含任何凭证）。
    pub detail: String,
    /// 源结构基线（resource → 列清单 [name/baseType]；PgDirect 反射 information_schema）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<Value>,
}

/// 对象数据后端统一契约（读路径）。实现必须无进程内业务缓存（集群无状态红线，AGENTS §五）。
#[async_trait]
pub trait ObjectDataBackend: Send + Sync {
    /// 后端类型。
    fn kind(&self) -> BackendKind;

    /// 能力矩阵（分派器预校验依据）。
    fn caps(&self) -> BackendCaps;

    /// 加载一页。`set` 必须落在 [`Self::caps`] 声明的算子面内（分派器保证；
    /// 实现仍应二次校验——防御纵深）。固定排序口径 `title, pk`（E4）。
    async fn load(&self, ctx: &BackendCtx, set: &ObjectSet, page: &Page)
        -> StoreResult<ObjectPage>;

    /// 对对象集执行聚合（Count/GroupCount/GroupSum）。
    async fn aggregate(&self, ctx: &BackendCtx, set: &ObjectSet, agg: &Aggregation)
        -> StoreResult<Value>;

    /// 解析对象集的 pk 集合（桥接专用，上限 `max`；超出即整查询拒绝——fail-closed 不截断）。
    async fn resolve_pks(&self, ctx: &BackendCtx, set: &ObjectSet, max: u32)
        -> StoreResult<Vec<String>>;

    /// 源探测：连通性 + 结构基线（M1 手动触发 + 查询失败自动触发一次；B-P1-5）。
    async fn probe(&self, source_db_id: &str, resource: Option<&str>) -> StoreResult<ProbeReport>;
}
