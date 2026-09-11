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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource: Option<Value>,
    /// 若由 cmx-model DOC/DCT 生成，回指来源。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmx_origin: Option<Value>,
    #[serde(default)]
    pub version: u32,
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

/// 关系基数。
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

/// 关系落存储方式（强类型；对标 Palantir link datasource backing）。
///
/// 序列化按 `kind` 标签分派（camelCase）：
/// - `{"kind":"edge"}` —— 原生 `ol_edge` 单表（默认；写入 put_link/delete_link 走此）。
/// - `{"kind":"foreignKey","property":"customerId","side":"b"}` —— 外键列落在 `side` 端对象表的
///   `props->>'property'`，指向对端 pk（本轮 SearchAround 读侧按此编译 FK JOIN；写侧仍兜底 ol_edge）。
/// - `{"kind":"joinTable",...}` / `{"kind":"intermediary",...}` —— 声明占位，编译暂回退 ol_edge（下一轮接）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LinkBacking {
    /// 原生边表（默认）。
    #[default]
    Edge,
    /// 外键列 backing：`side` 端对象表的 `property` 列存对端 pk。
    ForeignKey { property: String, #[serde(default)] side: LinkEnd },
    /// 连接表 backing（多对多；两列外键）——声明占位，编译暂回退 Edge。
    JoinTable {
        #[serde(default)] table: String,
        #[serde(default)] left_column: String,
        #[serde(default)] right_column: String,
    },
    /// 中间对象类型 backing——声明占位，编译暂回退 Edge。
    Intermediary {
        #[serde(default)] object_type: String,
        #[serde(default)] left_property: String,
        #[serde(default)] right_property: String,
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
}

impl LinkTypeDef {
    /// 解析 `backing`（裸 JSON）为强类型 [`LinkBacking`]；空/非法/无法识别 → `Edge` 兜底（向后兼容）。
    pub fn backing_parsed(&self) -> LinkBacking {
        if self.backing.is_null() {
            return LinkBacking::Edge;
        }
        serde_json::from_value(self.backing.clone()).unwrap_or(LinkBacking::Edge)
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
        // backing 若指定，须能解析且约束合法：ForeignKey 的 property 非空且为合法标识符。
        if let LinkBacking::ForeignKey { property, .. } = self.backing_parsed() {
            if property.trim().is_empty() {
                return Err(crate::Error::Definition(
                    "ForeignKey backing 的 property（外键属性名）不能为空".into(),
                ));
            }
            if !is_valid_api_name(&property) {
                return Err(crate::Error::Definition(format!(
                    "ForeignKey backing 的 property「{property}」非法（须字母/下划线开头，仅字母数字下划线）"
                )));
            }
        }
        Ok(())
    }
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

/// 对象类型清单项（列表用，不含完整属性体）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectTypeMeta {
    pub api_name: String,
    pub display_name: String,
    pub status: TypeStatus,
    pub primary_key: String,
    pub property_count: u32,
    /// DAM 三级分类（清单富化：前端 explorer 免拉全定义即可按 DAM 分组）。
    #[serde(default)]
    pub dam: DamRef,
    /// 业务单据类型（清单富化：对象浏览器按此在模块下再分一层）。
    #[serde(default)]
    pub doc_type: DocTypeRef,
    pub version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
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
}

/// 通用类型清单项（接口/共享属性/动作/函数）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimpleTypeMeta {
    pub api_name: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    /// 实现者清单（清单富化 A3，仅接口填充；其余四类恒 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implements_by: Option<Vec<String>>,
    /// 继承的父接口链（清单富化 A3，仅接口填充；其余四类恒 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<Vec<String>>,
}

/// 本体全量清单（建模台/OSDK 生成的输入）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OntologyManifest {
    pub object_types: Vec<ObjectTypeMeta>,
    pub link_types: Vec<LinkTypeMeta>,
    pub interfaces: Vec<SimpleTypeMeta>,
    pub shared_properties: Vec<SimpleTypeMeta>,
    pub action_types: Vec<SimpleTypeMeta>,
    pub functions: Vec<SimpleTypeMeta>,
}

/// 发布版本元数据（不可变快照）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OntologyVersionMeta {
    pub version: u32,
    pub rev: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_by: Option<String>,
    pub published_at: DateTime<Utc>,
}
