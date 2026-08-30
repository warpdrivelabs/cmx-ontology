//! O2 对象存储与索引层的读写 DTO（语义中立，零 DB 依赖）。
//!
//! - **对象记录** [`ObjectRecord`] / **关系边** [`LinkEdge`]：物化对象与关系的运行时表示。
//! - **对象集代数** [`ObjectSet`]：读取的统一抽象——Base/Filter/SearchAround/集合运算/Static，
//!   由 store 层编译为**一条** SQL（含 JOIN/子查询，避免 N+1）。对标 Palantir Object Set Service。
//! - **谓词** [`Predicate`]：过滤的类型化表示（不接受裸 SQL，防注入）。
//! - **聚合** [`Aggregation`]：Count / GroupCount（跨对象汇总，seeds cmx-agg）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 物化对象记录（一个对象实例）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ObjectRecord {
    /// 主键（规范化为字符串，跨类型统一 join）。
    pub pk: String,
    /// 展示标题（对象卡片的"名字"；由 titleProperty 派生，可空）。
    #[serde(default)]
    pub title: String,
    /// 全部属性值（apiName → 值）。
    #[serde(default)]
    pub properties: Value,
}

/// 关系边（一条 A→B 的关系实例）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LinkEdge {
    /// 关系类型 apiName。
    pub link: String,
    /// A 端对象 pk。
    pub a_pk: String,
    /// B 端对象 pk。
    pub b_pk: String,
    /// 边上属性（Intermediary 关系携带属性；可空）。
    #[serde(default)]
    pub properties: Value,
}

/// Search-Around 方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum LinkDirection {
    /// 沿 A→B（源在 A 端，遍历到 B 端对象）。
    #[default]
    Forward,
    /// 沿 B→A（源在 B 端，遍历到 A 端对象）。
    Reverse,
}

/// 对象集代数——读取的统一抽象。递归组合，由 store 层编译为一条 SQL。
///
/// serde 注意：`#[serde(tag)]` + `rename_all` 只重命名**变体标签**（base/searchAround…），**不 cascade**
/// 到 struct-variant 内部字段——故多词字段须显式 `#[serde(rename)]` 才得 camelCase JSON 键。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "op")]
pub enum ObjectSet {
    /// 某对象类型的全量。
    Base {
        #[serde(rename = "objectType")]
        object_type: String,
    },
    /// 过滤（谓词树）。
    Filter {
        source: Box<ObjectSet>,
        predicate: Predicate,
    },
    /// ★关系遍历（本体灵魂）：沿关系类型从源对象集走到相关对象集。
    SearchAround {
        source: Box<ObjectSet>,
        link: String,
        #[serde(default)]
        direction: LinkDirection,
    },
    /// 并集（按 pk 去重）。
    Union {
        left: Box<ObjectSet>,
        right: Box<ObjectSet>,
    },
    /// 交集（按 pk）。
    Intersect {
        left: Box<ObjectSet>,
        right: Box<ObjectSet>,
    },
    /// 差集（左 - 右，按 pk）。
    Subtract {
        left: Box<ObjectSet>,
        right: Box<ObjectSet>,
    },
    /// 静态集（存主键列表；不随数据变）。
    Static {
        #[serde(rename = "objectType")]
        object_type: String,
        #[serde(rename = "primaryKeys")]
        primary_keys: Vec<String>,
    },
}

impl ObjectSet {
    /// 该对象集最终产出的对象类型（用于选目标表 / 前端展示）。
    /// SearchAround 的产出类型由关系两端 + 方向决定，需外部 link 解析——此处返回 None，
    /// 由编译器结合 [`LinkResolver`] 推断。
    pub fn terminal_object_type(&self) -> Option<&str> {
        match self {
            ObjectSet::Base { object_type } | ObjectSet::Static { object_type, .. } => {
                Some(object_type)
            }
            ObjectSet::Filter { source, .. } => source.terminal_object_type(),
            ObjectSet::Union { left, .. }
            | ObjectSet::Intersect { left, .. }
            | ObjectSet::Subtract { left, .. } => left.terminal_object_type(),
            ObjectSet::SearchAround { .. } => None,
        }
    }
}

/// 类型化过滤谓词（不接受裸 SQL；值为标量 JSON）。属性路径即对象属性 apiName。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Predicate {
    Eq { property: String, value: Value },
    Ne { property: String, value: Value },
    Gt { property: String, value: Value },
    Ge { property: String, value: Value },
    Lt { property: String, value: Value },
    Le { property: String, value: Value },
    /// 属性值 ∈ 列表（文本比较）。
    In { property: String, values: Vec<Value> },
    /// 文本包含子串（LIKE %sub%）。
    Contains { property: String, value: String },
    /// 属性为空（缺失或 null）。
    IsNull { property: String },
    And { predicates: Vec<Predicate> },
    Or { predicates: Vec<Predicate> },
    Not { predicate: Box<Predicate> },
}

/// 聚合类型（跨对象汇总；seeds cmx-agg）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Aggregation {
    /// 对象计数。
    Count,
    /// 按属性分组计数。
    GroupCount { property: String },
    /// 按属性分组求和（对另一数值属性）。
    GroupSum {
        #[serde(rename = "groupBy")]
        group_by: String,
        sum: String,
    },
}

/// 分页（load 用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
}
fn default_limit() -> u32 {
    100
}
impl Default for Page {
    fn default() -> Self {
        Self { limit: 100, offset: 0 }
    }
}

/// load 结果（一页对象 + 是否还有更多 + 该集合的对象类型提示）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ObjectPage {
    pub object_type: String,
    pub rows: Vec<ObjectRecord>,
    pub limit: u32,
    pub offset: u32,
    /// 本页是否满（满则可能还有下一页）。
    pub has_more: bool,
}
