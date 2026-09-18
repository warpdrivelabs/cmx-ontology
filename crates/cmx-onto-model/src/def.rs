//! 本体元模型定义类型（语义中立内核，零 DB/infra 依赖）。
//!
//! 对标 Palantir Foundry Ontology 的语义元素（Object/Property/Link/Interface/SharedProperty Type）
//! 与动能元素（Action/Function Type）。JSON 一律 camelCase（前端 JS 友好，字段 rename_all 显式声明，
//! 规避「漏 rename_all → 键名不匹配」类坑）。可空/可缺字段带 `#[serde(default)]`，容忍前端偏序 JSON。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

// ───────────────────────────── 通用 ─────────────────────────────

/// 类型生命周期（Experimental → Active 固化 → Deprecated 废弃）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TypeStatus {
    /// 试验态（对象存储可走 JSONB 过渡，见方案 §6.2）。
    #[default]
    Experimental,
    /// 激活态（可被高效查询；对象存储固化为 per-type 物理表）。
    Active,
    /// 废弃态（保留定义，新数据不再写入）。
    Deprecated,
}

impl TypeStatus {
    /// camelCase 文本（与列存一致；SQL 拼接/比对用）。
    pub fn as_str(&self) -> &'static str {
        match self {
            TypeStatus::Experimental => "experimental",
            TypeStatus::Active => "active",
            TypeStatus::Deprecated => "deprecated",
        }
    }
}

/// 弃用元数据（方案 20260917 §5.3；七类资源表四列的回读载体）。
///
/// **写入路径唯一**：仅 `POST /lifecycle/transition` 写入；普通 save 剥离（防旁路篡改）；
/// 离开 deprecated 即整体置 NULL（弃用史留在修订历史）。挂在定义上的这份是**回读展示**字段，
/// 保存 round-trip 时被剥离、不落库。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeprecationMeta {
    /// 弃用原因（transition → deprecated 必填）。
    #[serde(default)]
    pub reason: String,
    /// 预期下线期限（YYYY-MM-DD；transition 必填）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sunset_at: Option<String>,
    /// 替代资源 apiName（可选，展示用引用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_api_name: Option<String>,
    /// 废弃动作时间。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated_at: Option<DateTime<Utc>>,
}

/// 背书数据源展示指针（方案 20260918 §5.6）：`{sourceId, mode, resource}`。
///
/// **非权威**（E1）：绑定权威真源是 `om_source_mapping` 行；本指针仅 manifest/Inspector 快速展示，
/// bind/unbind 时同步维护。普通 save 剥离（E2，学 deprecation 先例）。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DataSourceBinding {
    /// 数据源 id（M1a = toml `[[databases]]` db_id；M1b 起 = om_data_source.id）。
    #[serde(default)]
    pub source_id: String,
    /// 绑定模式（"materialized" | "virtual"；与 om_source_mapping.mode 同值域）。
    #[serde(default)]
    pub mode: String,
    /// 源资源名（PG `schema.table` 或 API 资源名；materialized 且走手写 SQL 时可空）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}

/// datasource 容错反序列化：非标形状（历史占位/外来写入）→ None，不让指针脏数据阻断定义装载。
fn deserialize_binding_tolerant<'de, D>(d: D) -> Result<Option<DataSourceBinding>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = Option::<Value>::deserialize(d)?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok()))
}

/// 展示指针 → 落库 jsonb（store 层列写入口共用；None → 列 NULL）。
pub fn datasource_to_json(ds: &Option<DataSourceBinding>) -> Option<Value> {
    ds.as_ref()
        .map(|b| serde_json::to_value(b).unwrap_or(Value::Null))
}

/// 兼容矩阵判定（方案 20260917 §5.4；对齐 Palantir
/// ConflictBetweenLinkTypeStatusAndObjectTypeStatus，按"任一端"穷尽 9 组合）。
///
/// 返回该关系在两端对象当前状态下**允许的状态集**：
/// 任一端 deprecated → 仅 deprecated；否则任一端 experimental → 仅 experimental；
/// 两端均 active → 三态皆可（单独废弃一条关系而两端对象继续 active 是合法下线）。
pub fn allowed_link_status(a: TypeStatus, b: TypeStatus) -> &'static [TypeStatus] {
    use TypeStatus::{Active, Deprecated, Experimental};
    if a == Deprecated || b == Deprecated {
        &[Deprecated]
    } else if a == Experimental || b == Experimental {
        &[Experimental]
    } else {
        &[Active, Experimental, Deprecated]
    }
}

/// 元模型类别（用于清单/版本/通用列表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MetaKind {
    ObjectType,
    LinkType,
    Interface,
    SharedProperty,
    ActionType,
    Function,
}

/// 校验 apiName：字母/下划线开头，仅字母数字下划线（跨版本稳定锚，见方案 §5.2）。
pub fn is_valid_api_name(s: &str) -> bool {
    let mut it = s.chars();
    match it.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ───────────────────────── 对象 / 属性类型 ─────────────────────────

/// 属性基础类型（对齐 OSv2 属性类型；地理/向量首版占位，索引后置）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PropertyBaseType {
    #[default]
    String,
    Integer,
    Long,
    Double,
    Decimal,
    Boolean,
    Date,
    Timestamp,
    Array,
    Struct,
    Attachment,
    MediaReference,
    Marking,
    Geohash,
    GeoShape,
    Vector,
}

/// 属性类型定义（对象类型的字段）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PropertyTypeDef {
    /// 稳定 API 名。
    pub api_name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub base_type: PropertyBaseType,
    #[serde(default)]
    pub required: bool,
    /// 是否建搜索索引（O2 对象存储据此建 PG 索引）。
    #[serde(default)]
    pub is_indexed: bool,
    /// 语义类型（复用 cmx-meta-data semanticType：金额/百分比/邮箱…）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_type: Option<String>,
    /// 引用的共享属性类型（标准化字段）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_property: Option<String>,
    /// 列级安全标记（marking；O6 接 cmx-dataauth 脱敏）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marking: Option<String>,
    /// 取值约束（O4/O5 落规则引擎 FEEL；此处保留原始 JSON）。
    #[serde(default)]
    pub constraints: Value,
    #[serde(default)]
    pub description: String,
}

/// DAM 三级分类（Domain · Application · Module；复用系统既有分类，驱动本体图分域折叠）。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DamRef {
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub application: String,
    #[serde(default)]
    pub module: String,
}

/// 业务单据类型（Business Document Type；对象类型归属的单据，如"销售订单"；一张单据常含多个对象类型如头/行）。
/// 由 cmx-model DOC 反向导入回填，或设计器 Inspector 手填；对象浏览器按此在模块下再分一层。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DocTypeRef {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub name: String,
}

/// 对象类型定义（真实世界实体的 schema；名词）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ObjectTypeDef {
    /// 稳定 API 名（如 "Customer"，跨版本不变）。
    pub api_name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: String,
    /// 图谱着色。
    #[serde(default)]
    pub color: String,
    /// DAM 三级分类（域/应用/模块）——本体图按此分域折叠，缓解大图性能。
    #[serde(default)]
    pub dam: DamRef,
    /// 业务单据类型（对象浏览器在模块下按此再分一层；DOC 导入回填，Inspector 可手填）。
    #[serde(default)]
    pub doc_type: DocTypeRef,
    /// 主键属性 apiName。
    #[serde(default)]
    pub primary_key: String,
    /// 展示标题属性 apiName（对象卡片用哪个字段当"名字"）。
    #[serde(default)]
    pub title_property: String,
    #[serde(default)]
    pub status: TypeStatus,
    #[serde(default)]
    pub properties: Vec<PropertyTypeDef>,
    /// 实现的接口 apiName（多态）。
    #[serde(default)]
    pub implements: Vec<String>,
    /// 背书数据源（O3 Funnel 从哪里灌；此处保留原始 JSON）。
    /// 方案 20260918 §5.6：降为**展示指针** `{sourceId, mode, resource}`——非权威（派发只读
    /// om_source_mapping 行）；普通 save 剥离防旁路（E2），唯一写入口 = bind/unbind API。
    /// 反序列化容错：历史/外来非标形状 → None（不因指针脏数据阻断整个定义装载）。
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_binding_tolerant"
    )]
    pub datasource: Option<DataSourceBinding>,
    /// 若由 cmx-model DOC/DCT 生成，回指来源。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmx_origin: Option<Value>,
    #[serde(default)]
    pub version: u32,
    /// 弃用元数据回读（仅 transition 写入；save round-trip 剥离不落库）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<DeprecationMeta>,
}

impl ObjectTypeDef {
    /// 结构校验（不落库前调用）。
    pub fn validate(&self) -> crate::Result<()> {
        if !is_valid_api_name(&self.api_name) {
            return Err(crate::Error::Definition(format!(
                "对象类型 apiName「{}」非法（须字母/下划线开头，仅字母数字下划线）",
                self.api_name
            )));
        }
        let mut seen = HashSet::new();
        for p in &self.properties {
            if !is_valid_api_name(&p.api_name) {
                return Err(crate::Error::Definition(format!(
                    "属性 apiName「{}」非法",
                    p.api_name
                )));
            }
            if !seen.insert(p.api_name.as_str()) {
                return Err(crate::Error::Definition(format!(
                    "属性 apiName「{}」重复",
                    p.api_name
                )));
            }
        }
        if !self.primary_key.is_empty()
            && !self.properties.iter().any(|p| p.api_name == self.primary_key)
        {
            return Err(crate::Error::Definition(format!(
                "主键属性「{}」不在属性列表中",
                self.primary_key
            )));
        }
        if !self.title_property.is_empty()
            && !self.properties.iter().any(|p| p.api_name == self.title_property)
        {
            return Err(crate::Error::Definition(format!(
                "标题属性「{}」不在属性列表中",
                self.title_property
            )));
        }
        Ok(())
    }
}

/// 校验对象类型是否满足其声明接口的共享属性契约。
///
/// 规则：`def.implements` 中每个接口的 `properties`（共享属性 apiName 列表）在
/// `def.properties` 中都必须存在一个 `p.shared_property == Some(spt_api_name)`
/// 且 `p.base_type == spt.base_type` 的属性；否则返回 `Err(Definition(...))`。
///
/// 调用方（`save_object_type` handler）负责预加载接口和共享属性定义，保证本函数零 IO。
pub fn validate_implements(
    def: &ObjectTypeDef,
    ifaces: &[InterfaceDef],
    shared: &[SharedPropertyTypeDef],
) -> crate::Result<()> {
    for iface_name in &def.implements {
        let iface = ifaces
            .iter()
            .find(|i| &i.api_name == iface_name)
            .ok_or_else(|| {
                crate::Error::Definition(format!(
                    "对象类型「{}」声明实现了接口「{iface_name}」，但该接口未定义",
                    def.api_name
                ))
            })?;
        for spt_name in &iface.properties {
            let spt = shared.iter().find(|s| &s.api_name == spt_name).ok_or_else(|| {
                crate::Error::Definition(format!(
                    "接口「{iface_name}」要求共享属性「{spt_name}」，但该共享属性未定义"
                ))
            })?;
            // 对象类型必须有 shared_property == spt_name 且 base_type 完全匹配的属性。
            let satisfied = def
                .properties
                .iter()
                .any(|p| p.shared_property.as_deref() == Some(spt_name.as_str()) && p.base_type == spt.base_type);
            if !satisfied {
                return Err(crate::Error::Definition(format!(
                    "对象类型「{}」未满足接口「{iface_name}」的契约：缺少引用共享属性「{spt_name}」（baseType={:?}）的属性",
                    def.api_name, spt.base_type
                )));
            }
        }
    }
    Ok(())
}

// ───────────────────────────── 关系类型 ─────────────────────────────

/// 关系基数（有向：oneToMany = 源 1 : 靶 N）。manyToOne 已废除——与 oneToMany 调换两端
/// 同义（N:1(A,B) ≡ 1:N(B,A)），建模统一选 1:N 并把多端放 B；反序列化遇 "manyToOne" 拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum LinkCardinality {
    OneToOne,
    #[default]
    OneToMany,
    ManyToMany,
}

/// 关系的一端（A / B）。用于 ForeignKey backing 标记"外键列落在哪一端的对象表"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum LinkEnd {
    #[default]
    A,
    B,
}

/// 关系落存储方式（强类型；对标 Palantir Foundry link datasource backing 三模式）。
///
/// `LinkTypeDef.backing` 原样落库、原样回读（前端 round-trip 不失真）；本枚举只是解析后的
/// 引擎视图。**对外唯一口径 = 页面形状**，仅认三种顶层键（`fk` / `joinTable` / `intermediary`）：
///
/// - `{"fk":{"sourceProperty":"settleCurrencyId","side":"b","targetProperty"?:"code"}}`
///   —— `side` 端对象表 `props->>'sourceProperty'` 存对端匹配值；`targetProperty` 缺省 =
///   对端主键（`oo_<type>.pk` 列，Palantir Key 严格语义）；显式指定 = 与对端
///   `props->>'targetProperty'` 属性对属性相等（受控扩展，喂"外键存 code 等自然键"场景；
///   注意对象 pk 列与 props 的 id 属性不保证同值）。`side` 缺省按 cardinality 推导
///   （oneToMany→b、oneToOne→a，推导只在本解析层做一次）。
/// - `{"joinTable":{"table","leftColumn","rightColumn"}}`：连接表 backing（manyToMany）；
///   `leftColumn`↔A 端主键、`rightColumn`↔B 端主键，两列类型须与 `oo_*.pk` 同型（text）。
/// - `{"intermediary":{"objectType","leftProperty","rightProperty"}}`：中间对象类型 backing
///   （manyToMany）；中间对象两属性各存两端对象主键。
/// - 旧 tagged `{"kind":...}` 口径已废除：无法识别一律 `Edge` 兜底（保存路径留痕告警）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LinkBacking {
    /// 原生边表（默认）。
    #[default]
    Edge,
    /// 外键 backing：`side` 端 `property` 属性值 = 对端匹配值（target_property 缺省 = 对端
    /// 主键 pk 列；非空 = 对端 `props->>'target_property'`，属性对属性匹配）。
    ForeignKey {
        property: String,
        side: LinkEnd,
        /// 对端匹配属性 apiName；None = 对端主键 pk 列（严格 Palantir 语义）。
        target_property: Option<String>,
    },
    /// 连接表 backing（多对多；两列外键，left↔A 端主键、right↔B 端主键）。
    JoinTable {
        table: String,
        left_column: String,
        right_column: String,
    },
    /// 中间对象类型 backing（多对多；中间对象两属性各存两端主键）。
    Intermediary {
        object_type: String,
        left_property: String,
        right_property: String,
    },
}

/// 关系类型定义（对象类型间的关系；Search-Around 的路径）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LinkTypeDef {
    pub api_name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub cardinality: LinkCardinality,
    /// A 端对象类型 apiName。
    #[serde(default)]
    pub object_type_a: String,
    /// B 端对象类型 apiName。
    #[serde(default)]
    pub object_type_b: String,
    /// A→B 角色名（如 "places"）。
    #[serde(default)]
    pub role_a: String,
    /// B→A 角色名（如 "placedBy"）。
    #[serde(default)]
    pub role_b: String,
    /// 关系落存储方式（ForeignKey/JoinTable/Intermediary；此处保留原始 JSON，O2 消费）。
    #[serde(default)]
    pub backing: Value,
    #[serde(default)]
    pub status: TypeStatus,
    /// 乐观锁（0 = 新建/盲写；>0 = 条件更新，20260917 补齐五类表）。
    #[serde(default)]
    pub version: u32,
    /// 弃用元数据回读（仅 transition 写入；save round-trip 剥离不落库）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<DeprecationMeta>,
}

impl LinkTypeDef {
    /// 解析 `backing`（裸 JSON）为强类型 [`LinkBacking`]；空/非法/无法识别 → `Edge` 兜底。
    ///
    /// **唯一口径 = 页面形状**（`fk` / `joinTable` / `intermediary` 三种顶层键）；旧 tagged
    /// `{"kind":...}` 已废除（落 Edge，保存路径留痕告警）。FK `side` 缺省时按 cardinality
    /// 在此推导（oneToMany→B、oneToOne→A）——推导全工程仅此一处，编译与校验
    /// 均消费本结果，显式传值与基数的一致性由 [`Self::validate`] 把关。
    pub fn backing_parsed(&self) -> LinkBacking {
        let v = &self.backing;
        if let Some(fk) = v.get("fk") {
            let side = match fk.get("side").and_then(|x| x.as_str()) {
                Some("b") | Some("B") => LinkEnd::B,
                Some("a") | Some("A") => LinkEnd::A,
                // 缺省/非法值：按基数推导（manyToMany + FK 由 validate 拒绝，此处按 many 端推导）。
                _ => match self.cardinality {
                    LinkCardinality::OneToOne => LinkEnd::A,
                    _ => LinkEnd::B,
                },
            };
            let tp = json_str(fk, "targetProperty");
            return LinkBacking::ForeignKey {
                property: json_str(fk, "sourceProperty"),
                side,
                target_property: (!tp.is_empty()).then_some(tp),
            };
        }
        if let Some(jt) = v.get("joinTable") {
            return LinkBacking::JoinTable {
                table: json_str(jt, "table"),
                left_column: json_str(jt, "leftColumn"),
                right_column: json_str(jt, "rightColumn"),
            };
        }
        if let Some(im) = v.get("intermediary") {
            return LinkBacking::Intermediary {
                object_type: json_str(im, "objectType"),
                left_property: json_str(im, "leftProperty"),
                right_property: json_str(im, "rightProperty"),
            };
        }
        LinkBacking::Edge
    }

    pub fn validate(&self) -> crate::Result<()> {
        if !is_valid_api_name(&self.api_name) {
            return Err(crate::Error::Definition(format!(
                "关系类型 apiName「{}」非法",
                self.api_name
            )));
        }
        if self.object_type_a.is_empty() || self.object_type_b.is_empty() {
            return Err(crate::Error::Definition(
                "关系类型两端对象类型（objectTypeA / objectTypeB）不能为空".into(),
            ));
        }
        // backing 若指定，须能解析且约束合法（Palantir 编辑约束轻量版：key 恒映射对端主键）。
        match self.backing_parsed() {
            LinkBacking::Edge => {}
            LinkBacking::ForeignKey { property, side, target_property } => {
                // 白名单键：fk 仅允许 sourceProperty / side / targetProperty。
                if let Some(keys) = self.backing.get("fk").and_then(|f| f.as_object()) {
                    for key in keys.keys() {
                        if key != "sourceProperty" && key != "side" && key != "targetProperty" {
                            return Err(crate::Error::Definition(format!(
                                "ForeignKey backing 不支持字段「{key}」（仅允许 sourceProperty / side / targetProperty）"
                            )));
                        }
                    }
                    if let Some(s) = keys.get("side").and_then(|x| x.as_str())
                        && !matches!(s, "a" | "A" | "b" | "B") {
                            return Err(crate::Error::Definition(format!(
                                "ForeignKey backing 的 side「{s}」非法（仅 a / b）"
                            )));
                        }
                }
                if property.trim().is_empty() {
                    return Err(crate::Error::Definition(
                        "ForeignKey backing 的 sourceProperty（持键端外键属性名）不能为空".into(),
                    ));
                }
                if !is_valid_api_name(&property) {
                    return Err(crate::Error::Definition(format!(
                        "ForeignKey backing 的 sourceProperty「{property}」非法（须字母/下划线开头，仅字母数字下划线）"
                    )));
                }
                if let Some(tp) = &target_property
                    && !is_valid_api_name(tp) {
                        return Err(crate::Error::Definition(format!(
                            "ForeignKey backing 的 targetProperty「{tp}」非法（须字母/下划线开头，仅字母数字下划线）"
                        )));
                    }
                // 显式 side 与基数的一致性（外键恒在 many 端；缺省 side 已在解析层按基数推导）。
                let expect = match self.cardinality {
                    LinkCardinality::OneToMany => Some(LinkEnd::B),
                    LinkCardinality::OneToOne => None,
                    LinkCardinality::ManyToMany => {
                        return Err(crate::Error::Definition(
                            "ForeignKey backing 不支持 manyToMany（多对多请用连接表或中间对象 backing）".into(),
                        ));
                    }
                };
                if let Some(want) = expect
                    && want != side {
                        let want_s = if want == LinkEnd::A { "a" } else { "b" };
                        let got_s = if side == LinkEnd::A { "a" } else { "b" };
                        return Err(crate::Error::Definition(format!(
                            "ForeignKey backing 的 side「{got_s}」与 cardinality「{:?}」矛盾：外键须在 many 端「{want_s}」",
                            self.cardinality
                        )));
                    }
            }
            LinkBacking::JoinTable { table, left_column, right_column } => {
                if self.cardinality != LinkCardinality::ManyToMany {
                    return Err(crate::Error::Definition(
                        "JoinTable backing 仅用于 manyToMany 关系".into(),
                    ));
                }
                for (label, val) in [
                    ("table", table.as_str()),
                    ("leftColumn", left_column.as_str()),
                    ("rightColumn", right_column.as_str()),
                ] {
                    if val.trim().is_empty() {
                        return Err(crate::Error::Definition(format!(
                            "JoinTable backing 的 {label} 不能为空"
                        )));
                    }
                    if !is_valid_api_name(val) {
                        return Err(crate::Error::Definition(format!(
                            "JoinTable backing 的 {label}「{val}」非法（须字母/下划线开头，仅字母数字下划线）"
                        )));
                    }
                }
            }
            LinkBacking::Intermediary { object_type, left_property, right_property } => {
                if self.cardinality != LinkCardinality::ManyToMany {
                    return Err(crate::Error::Definition(
                        "Intermediary backing 仅用于 manyToMany 关系".into(),
                    ));
                }
                for (label, val) in [
                    ("objectType", object_type.as_str()),
                    ("leftProperty", left_property.as_str()),
                    ("rightProperty", right_property.as_str()),
                ] {
                    if val.trim().is_empty() {
                        return Err(crate::Error::Definition(format!(
                            "Intermediary backing 的 {label} 不能为空"
                        )));
                    }
                    if !is_valid_api_name(val) {
                        return Err(crate::Error::Definition(format!(
                            "Intermediary backing 的 {label}「{val}」非法（须字母/下划线开头，仅字母数字下划线）"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

/// 取 JSON 对象的字符串字段（trim；非字符串/缺失 → 空串）。
fn json_str(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

// ─────────────────────── 接口 / 共享属性类型 ───────────────────────

/// 接口（对象类型的形状契约，提供多态）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct InterfaceDef {
    pub api_name: String,
    #[serde(default)]
    pub display_name: String,
    /// 要求实现者具备的共享属性 apiName。
    #[serde(default)]
    pub properties: Vec<String>,
    /// 接口继承。
    #[serde(default)]
    pub extends: Vec<String>,
    #[serde(default)]
    pub status: TypeStatus,
    /// 乐观锁（20260917 补齐五类表）。
    #[serde(default)]
    pub version: u32,
    /// 弃用元数据回读（仅 transition 写入；save round-trip 剥离不落库）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<DeprecationMeta>,
}

impl InterfaceDef {
    pub fn validate(&self) -> crate::Result<()> {
        if !is_valid_api_name(&self.api_name) {
            return Err(crate::Error::Definition(format!(
                "接口 apiName「{}」非法",
                self.api_name
            )));
        }
        Ok(())
    }
}

/// 共享属性类型（全局标准属性，一处定义处处引用）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SharedPropertyTypeDef {
    pub api_name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub base_type: PropertyBaseType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_type: Option<String>,
    #[serde(default)]
    pub description: String,
    /// 生命周期（20260917 起状态覆盖七类资源；此前共享属性无状态）。
    #[serde(default)]
    pub status: TypeStatus,
    /// 乐观锁（20260917 补齐五类表）。
    #[serde(default)]
    pub version: u32,
    /// 弃用元数据回读（仅 transition 写入；save round-trip 剥离不落库）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<DeprecationMeta>,
}

impl SharedPropertyTypeDef {
    pub fn validate(&self) -> crate::Result<()> {
        if !is_valid_api_name(&self.api_name) {
            return Err(crate::Error::Definition(format!(
                "共享属性 apiName「{}」非法",
                self.api_name
            )));
        }
        Ok(())
    }
}

// ───────────────────────────── 动作类型 ─────────────────────────────

/// 动作类型定义（一组受治理的编辑 + 校验 + 副作用；动词）。O1 仅建模，执行引擎见 O4。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActionTypeDef {
    pub api_name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    /// 表单参数（可绑对象/对象集/标量）。
    #[serde(default)]
    pub parameters: Value,
    /// 编辑规则（Create/Modify/Delete Object，Add/Remove Link）。
    #[serde(default)]
    pub logic: Value,
    /// 提交校验（O4 落规则引擎 FEEL）。
    #[serde(default)]
    pub validations: Value,
    /// 副作用（通知/webhook/函数/流程/事件）。
    #[serde(default)]
    pub side_effects: Value,
    /// 函数背书动作（复杂逻辑走函数）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_backing: Option<String>,
    #[serde(default)]
    pub status: TypeStatus,
    /// 乐观锁（20260917 补齐五类表）。
    #[serde(default)]
    pub version: u32,
    /// 弃用元数据回读（仅 transition 写入；save round-trip 剥离不落库）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<DeprecationMeta>,
}

impl ActionTypeDef {
    pub fn validate(&self) -> crate::Result<()> {
        if !is_valid_api_name(&self.api_name) {
            return Err(crate::Error::Definition(format!(
                "动作类型 apiName「{}」非法",
                self.api_name
            )));
        }
        Ok(())
    }
}

// ───────────────────────────── 函数类型 ─────────────────────────────

/// 函数运行时（一个接缝多种载体；判定永远走 FEEL 保 gap/overlap）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum FunctionRuntime {
    #[default]
    Feel,
    Rhai,
    Wasm,
    NativeRust,
}

/// 函数用途。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum FunctionKind {
    #[default]
    Query,
    DerivedProperty,
    Validation,
    ActionLogic,
    Aggregation,
}

/// 函数定义（原生吃对象/对象集的计算逻辑）。O1 仅建模，执行引擎见 O5。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FunctionDef {
    pub api_name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub runtime: FunctionRuntime,
    #[serde(default)]
    pub kind: FunctionKind,
    /// 输入参数（可吃 对象 / 对象集 / 标量）。
    #[serde(default)]
    pub inputs: Value,
    /// 返回类型。
    #[serde(default)]
    pub output: Value,
    /// 函数体（源码或引用）。
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub status: TypeStatus,
    /// 乐观锁（20260917 补齐五类表）。
    #[serde(default)]
    pub version: u32,
    /// 弃用元数据回读（仅 transition 写入；save round-trip 剥离不落库）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<DeprecationMeta>,
}

impl FunctionDef {
    pub fn validate(&self) -> crate::Result<()> {
        if !is_valid_api_name(&self.api_name) {
            return Err(crate::Error::Definition(format!(
                "函数 apiName「{}」非法",
                self.api_name
            )));
        }
        Ok(())
    }
}

// ─────────────────────── 清单 / 元数据 / 版本 ───────────────────────

/// 对象类型清单项（含完整属性体——manifest 即全量形状，消费方免单独拉详情）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectTypeMeta {
    pub api_name: String,
    pub display_name: String,
    pub status: TypeStatus,
    pub primary_key: String,
    pub property_count: u32,
    /// 完整属性定义（清单富化：选中/展示免二次请求 GET /object-types/{name}）。
    #[serde(default)]
    pub properties: Vec<PropertyTypeDef>,
    /// DAM 三级分类（清单富化：前端 explorer 免拉全定义即可按 DAM 分组）。
    #[serde(default)]
    pub dam: DamRef,
    /// 业务单据类型（清单富化：对象浏览器按此在模块下再分一层）。
    #[serde(default)]
    pub doc_type: DocTypeRef,
    /// 实现的接口 apiName 清单（清单富化：画布底座总览据此派生「对象—接口」实现连线）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub implements: Vec<String>,
    pub version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    /// 弃用元数据（explorer 悬停 / Inspector 查看 / 发布门禁警告清单摘录；非 deprecated 为 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<DeprecationMeta>,
    /// 数据源展示指针（清单富化，方案 20260918 §5.6）：explorer「直查」徽章 / studio 目录角标
    /// 据此判定虚拟类型，免逐类型二次请求。权威真源在 om_source_mapping（E1），此处仅展示。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource: Option<DataSourceBinding>,
}

/// 关系类型清单项。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkTypeMeta {
    pub api_name: String,
    pub display_name: String,
    pub cardinality: LinkCardinality,
    pub object_type_a: String,
    pub object_type_b: String,
    pub status: TypeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    /// A 端对象类型的 DAM（清单富化 A3：跨域关系治理；对象被删/不存在时为 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dam_a: Option<DamRef>,
    /// B 端对象类型的 DAM（同上）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dam_b: Option<DamRef>,
    /// 关系 backing 原始 JSON（fk/joinTable/intermediary 页面形状；空对象 = Edge 兜底）。
    /// 清单必须带锚点：Inspector 关系编辑表单靠它回显当前锚点，缺失会被误判「未登记锚点」且保存时把锚点洗掉。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backing: Option<Value>,
    /// 弃用元数据（非 deprecated 为 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<DeprecationMeta>,
}

/// 通用类型清单项（接口/共享属性/函数；动作用下方富化的 [`ActionTypeMeta`]）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimpleTypeMeta {
    pub api_name: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    /// 实现者清单（清单富化 A3，仅接口填充；其余三类恒 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implements_by: Option<Vec<String>>,
    /// 继承的父接口链（清单富化 A3，仅接口填充；其余三类恒 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<Vec<String>>,
    /// 运行时（清单富化 20260913，仅函数填充；接口/共享属性恒 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    /// 用途（清单富化 20260913，仅函数填充；接口/共享属性恒 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// 状态（清单富化 20260913，函数/接口填充；共享属性恒 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// 弃用元数据（20260917；非 deprecated 为 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<DeprecationMeta>,
}

/// 动作类型清单项（P2-0 清单富化：含参数与作用对象类型，前端据此按对象类型过滤动作）。
///
/// `target_object_types` 是**保存期派生的物化字段**（语义真源仍是 `parameters`/`logic`，
/// 对齐 Palantir"作用对象由参数类型声明"——本字段只为清单查询效率而存在，boot 时回填存量）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionTypeMeta {
    pub api_name: String,
    pub display_name: String,
    pub status: TypeStatus,
    /// 表单参数（原样；前端渲染表单/按类型过滤用）。
    #[serde(default)]
    pub parameters: Value,
    /// 作用对象类型（保存期从 parameters + logic 派生去重；GIN 索引支持按类型查动作）。
    #[serde(default)]
    pub target_object_types: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    /// 弃用元数据（20260917；非 deprecated 为 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<DeprecationMeta>,
}

/// 本体全量清单（建模台/OSDK 生成的输入）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OntologyManifest {
    pub object_types: Vec<ObjectTypeMeta>,
    pub link_types: Vec<LinkTypeMeta>,
    pub interfaces: Vec<SimpleTypeMeta>,
    pub shared_properties: Vec<SimpleTypeMeta>,
    pub action_types: Vec<ActionTypeMeta>,
    pub functions: Vec<SimpleTypeMeta>,
}

/// 存档检查点元数据（不可变快照；tag 非空 = 命名发布标记）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OntologyVersionMeta {
    pub version: u32,
    pub rev: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_by: Option<String>,
    pub archived_at: DateTime<Utc>,
    /// 发布标记名（如 v1.2；NULL = 匿名检查点）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// 发布说明（随 tag 写入）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_note: Option<String>,
}
