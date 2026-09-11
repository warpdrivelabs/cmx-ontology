//! 草稿/发布双轨（P2，方案 §2.4/§2.5）——om_draft 单工作区内容 + 发布指纹/diff/校验纯函数。
//!
//! 双轨纪律：om_* 是「已发布真源」（消费方读取路径零改动）；编辑写 om_draft 草稿工作区，
//! 发布 = 校验 → diff 预览 → 原子应用 om_* + 打 om_version 快照。本模块只放**驱动无关**的
//! 形状与纯函数（store 实现见 cmx-onto-store-pg/draft_store.rs；handler 见 cmx-onto-app）。
//!
//! 指纹口径（方案 §2.5 钉死）：rev = xxh64，覆盖七类元素 + views **语义字段**（名称/描述/
//! source/成员清单），**排除 layout 与审计字段**（乐观锁 version）——纯布局拖动不涨版本，
//! 视图成员变更必涨版本并留快照。数组顺序不敏感（规范化后排序）。

use crate::def::*;
use crate::view::SceneViewDef;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ───────────────────────────── 元素类别常量 ─────────────────────────────

/// 七类元素（七类 = 六类 om_* 元素 + 场景视图段）的 kind 标识（camelCase，前端同口径）。
pub const KIND_OBJECT: &str = "objectType";
pub const KIND_LINK: &str = "linkType";
pub const KIND_INTERFACE: &str = "interface";
pub const KIND_SHARED: &str = "sharedProperty";
pub const KIND_ACTION: &str = "actionType";
pub const KIND_FUNCTION: &str = "function";
pub const KIND_VIEW: &str = "view";

/// 全部元素 kind（不含 view——view 走 views 段整体 diff，无显式删除清单）。
pub const ELEMENT_KINDS: &[&str] = &[
    KIND_OBJECT,
    KIND_LINK,
    KIND_INTERFACE,
    KIND_SHARED,
    KIND_ACTION,
    KIND_FUNCTION,
];

/// kind → 快照/草稿内容里的数组键名。
pub fn kind_key(kind: &str) -> Option<&'static str> {
    match kind {
        KIND_OBJECT => Some("objectTypes"),
        KIND_LINK => Some("linkTypes"),
        KIND_INTERFACE => Some("interfaces"),
        KIND_SHARED => Some("sharedProperties"),
        KIND_ACTION => Some("actionTypes"),
        KIND_FUNCTION => Some("functions"),
        KIND_VIEW => Some("views"),
        _ => None,
    }
}

// ───────────────────────────── 草稿形状 ─────────────────────────────

/// 显式删除引用（restore 计算 / 前端维护；发布应用时服务端以「live − 草稿」派生集为权威，
/// 与显式清单做一致性校验——见 [`validate_draft`]）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeletionRef {
    /// 元素 kind（ELEMENT_KINDS 之一）。
    pub kind: String,
    pub api_name: String,
}

/// 草稿内容：七类元素 + 显式删除清单 + 场景视图段。可从发布快照 Value 直接反序列化
/// （快照缺 deletions/views 键 → 默认空），fork 即「快照 → 草稿内容」。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DraftContent {
    #[serde(default)]
    pub object_types: Vec<ObjectTypeDef>,
    #[serde(default)]
    pub link_types: Vec<LinkTypeDef>,
    #[serde(default)]
    pub interfaces: Vec<InterfaceDef>,
    #[serde(default)]
    pub shared_properties: Vec<SharedPropertyTypeDef>,
    #[serde(default)]
    pub action_types: Vec<ActionTypeDef>,
    #[serde(default)]
    pub functions: Vec<FunctionDef>,
    #[serde(default)]
    pub deletions: Vec<DeletionRef>,
    /// 场景视图段（P2 起场景编辑走草稿，live om_view 仅由 publish 应用；方案 §2.4 视图写轨对齐）。
    #[serde(default)]
    pub views: Vec<SceneViewDef>,
}

impl DraftContent {
    /// 轻量形状校验（草稿保存期；发布门用 [`validate_draft`] 全量校验）。
    /// 只拦「必然写不进去」的垃圾：apiName 非法/重复、deletions kind 不认识。
    pub fn validate_shape(&self) -> crate::Result<()> {
        let mut seen: std::collections::HashSet<String> = Default::default();
        for d in &self.object_types {
            if !seen.insert(format!("{KIND_OBJECT}:{}", d.api_name)) {
                return Err(crate::Error::Definition(format!("草稿内对象类型 {} 重复", d.api_name)));
            }
        }
        for d in &self.link_types {
            if !is_valid_api_name(&d.api_name) {
                return Err(crate::Error::Definition(format!("草稿内关系类型 apiName「{}」非法", d.api_name)));
            }
        }
        for d in &self.deletions {
            if kind_key(&d.kind).is_none() || d.api_name.is_empty() {
                return Err(crate::Error::Definition(format!(
                    "删除清单项非法：kind={:?} apiName={:?}",
                    d.kind, d.api_name
                )));
            }
        }
        Ok(())
    }

    /// 草稿内容 → 快照形状 Value（六类元素 + views；供指纹与 diff 对齐口径）。
    pub fn to_snapshot_value(&self) -> Value {
        let mut obj = serde_json::Map::new();
        let arrs: [(&str, &dyn Fn() -> Value); 6] = [
            ("objectTypes", &|| serde_json::to_value(&self.object_types).unwrap_or(Value::Null)),
            ("linkTypes", &|| serde_json::to_value(&self.link_types).unwrap_or(Value::Null)),
            ("interfaces", &|| serde_json::to_value(&self.interfaces).unwrap_or(Value::Null)),
            ("sharedProperties", &|| serde_json::to_value(&self.shared_properties).unwrap_or(Value::Null)),
            ("actionTypes", &|| serde_json::to_value(&self.action_types).unwrap_or(Value::Null)),
            ("functions", &|| serde_json::to_value(&self.functions).unwrap_or(Value::Null)),
        ];
        for (k, f) in arrs {
            obj.insert(k.to_string(), f());
        }
        obj.insert("views".into(), serde_json::to_value(&self.views).unwrap_or(Value::Null));
        Value::Object(obj)
    }
}

/// om_draft 行（单工作区恒 id=1）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftRow {
    /// 行级乐观锁（他人保存过 → 基线过期 409，与 base_rev 409 是两个错误源，方案 §2.4）。
    pub version: u32,
    /// 基线 live 快照指纹（fork/最近同步时；发布比对的是当前 live 指纹而非版本表 rev）。
    pub base_rev: String,
    pub content: DraftContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}

// ───────────────────────────── diff ─────────────────────────────

/// 元素级 diff 动作（camelCase 序列化）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffAction {
    Added,
    Modified,
    Removed,
}

/// 一条元素级差异。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffItem {
    /// 元素 kind（ELEMENT_KINDS 或 "view"）。
    pub kind: String,
    pub api_name: String,
    pub action: DiffAction,
    /// 人类可读摘要（对象附属性数变化等；视图附成员数变化）。
    pub summary: String,
}

/// 元素指纹规范化：剔除审计字段（乐观锁 version——直改盲写也会 +1，非语义变更）。
fn canonical_element(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("version");
    }
    v
}

/// 视图指纹规范化：剔除 layout 与 version（纯布局拖动/乐观锁漂移不进指纹，方案 §2.5）。
fn canonical_view(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("layout");
        o.remove("version");
    }
    v
}

fn arr_of<'a>(snap: &'a Value, key: &str) -> &'a [Value] {
    snap.get(key).and_then(|v| v.as_array()).map(|v| v.as_slice()).unwrap_or(&[])
}

/// 发布快照指纹（rev）：七类元素 canonical 化后按 apiName 排序拼接，xxh64 十六进制。
/// 顺序不敏感 + 排除 layout/审计字段——见模块头口径声明。
pub fn snapshot_fingerprint(snapshot: &Value) -> String {
    let mut buf = String::new();
    for key in ["objectTypes", "linkTypes", "interfaces", "sharedProperties", "actionTypes", "functions"] {
        let mut items: Vec<String> = arr_of(snapshot, key)
            .iter()
            .map(|e| canonical_element(e).to_string())
            .collect();
        items.sort();
        buf.push_str(key);
        buf.push_str(&format!("[{}]", items.join(",")));
    }
    let mut views: Vec<String> = arr_of(snapshot, "views").iter().map(canonical_view).map(|v| v.to_string()).collect();
    views.sort();
    buf.push_str(&format!("views[{}]", views.join(",")));
    format!("{:016x}", xxhash_rust::xxh64::xxh64(buf.as_bytes(), 0))
}

/// 元素级 diff：live（已发布真源）→ draft（草稿）。live 有草稿无 = Removed（发布应用时按此
/// 派生删除集，权威语义）；两均有但指纹不同 = Modified。视图 diff 排除 layout。
pub fn diff_snapshots(live: &Value, draft: &Value) -> Vec<DiffItem> {
    let mut out = Vec::new();
    for kind in ELEMENT_KINDS {
        let key = kind_key(kind).unwrap_or_default();
        let index = |snap: &Value| -> std::collections::BTreeMap<String, Value> {
            arr_of(snap, key)
                .iter()
                .filter_map(|e| {
                    let name = e.get("apiName").and_then(|v| v.as_str())?.to_string();
                    Some((name, canonical_element(e)))
                })
                .collect()
        };
        let (li, di) = (index(live), index(draft));
        for (name, dv) in &di {
            match li.get(name) {
                None => out.push(DiffItem {
                    kind: kind.to_string(),
                    api_name: name.clone(),
                    action: DiffAction::Added,
                    summary: diff_summary(kind, None, Some(dv)),
                }),
                Some(lv) if lv != dv => out.push(DiffItem {
                    kind: kind.to_string(),
                    api_name: name.clone(),
                    action: DiffAction::Modified,
                    summary: diff_summary(kind, Some(lv), Some(dv)),
                }),
                _ => {}
            }
        }
        for (name, lv) in &li {
            if !di.contains_key(name) {
                out.push(DiffItem {
                    kind: kind.to_string(),
                    api_name: name.clone(),
                    action: DiffAction::Removed,
                    summary: diff_summary(kind, Some(lv), None),
                });
            }
        }
    }
    // views 段：canonical（无 layout/version）对比，基线 = live om_view 全量（方案 §2.4）。
    let vindex = |snap: &Value| -> std::collections::BTreeMap<String, Value> {
        arr_of(snap, "views")
            .iter()
            .filter_map(|e| {
                let name = e.get("apiName").and_then(|v| v.as_str())?.to_string();
                Some((name, canonical_view(e)))
            })
            .collect()
    };
    let (lv, dv) = (vindex(live), vindex(draft));
    for (name, v) in &dv {
        match lv.get(name) {
            None => out.push(DiffItem {
                kind: KIND_VIEW.to_string(),
                api_name: name.clone(),
                action: DiffAction::Added,
                summary: diff_summary(KIND_VIEW, None, Some(v)),
            }),
            Some(l) if l != v => {
                let members = |x: &Value| {
                    format!(
                        "{}对象/{}接口",
                        x.get("members").and_then(|m| m.get("objects")).and_then(|o| o.as_array()).map(|a| a.len()).unwrap_or(0),
                        x.get("members").and_then(|m| m.get("interfaces")).and_then(|o| o.as_array()).map(|a| a.len()).unwrap_or(0),
                    )
                };
                out.push(DiffItem {
                    kind: KIND_VIEW.to_string(),
                    api_name: name.clone(),
                    action: DiffAction::Modified,
                    summary: format!("成员 {} → {}（或元数据变更）", members(l), members(v)),
                });
            }
            _ => {}
        }
    }
    for name in lv.keys() {
        if !dv.contains_key(name) {
            out.push(DiffItem {
                kind: KIND_VIEW.to_string(),
                api_name: name.clone(),
                action: DiffAction::Removed,
                summary: "live 有、草稿无 → 发布时删除该场景（manual 行）".into(),
            });
        }
    }
    out
}

/// 元素摘要：对象/视图给数量对比，其余给显示名对比。
fn diff_summary(kind: &str, live: Option<&Value>, draft: Option<&Value>) -> String {
    let name = |v: Option<&Value>| {
        v.and_then(|x| x.get("displayName")).and_then(|d| d.as_str()).unwrap_or("").to_string()
    };
    match kind {
        KIND_OBJECT => {
            let pc = |v: Option<&Value>| {
                v.and_then(|x| x.get("properties")).and_then(|p| p.as_array()).map(|a| a.len()).unwrap_or(0)
            };
            format!(
                "属性数 {} → {}",
                pc(live),
                pc(draft)
            )
        }
        KIND_VIEW => {
            let members = |v: Option<&Value>| {
                format!(
                    "{}对象/{}接口",
                    v.and_then(|x| x.get("members")).and_then(|m| m.get("objects")).and_then(|o| o.as_array()).map(|a| a.len()).unwrap_or(0),
                    v.and_then(|x| x.get("members")).and_then(|m| m.get("interfaces")).and_then(|o| o.as_array()).map(|a| a.len()).unwrap_or(0),
                )
            };
            format!("成员 {} → {}", members(live), members(draft))
        }
        _ => format!("显示名 {} → {}", name(live), name(draft)),
    }
}

/// 「live − 草稿」派生删除集（六类元素；发布应用与 restore 减法共用此口径）。
pub fn derive_deletions(live: &Value, target: &Value) -> Vec<DeletionRef> {
    let mut out = Vec::new();
    for kind in ELEMENT_KINDS {
        let key = kind_key(kind).unwrap_or_default();
        let names = |snap: &Value| -> std::collections::BTreeSet<String> {
            arr_of(snap, key)
                .iter()
                .filter_map(|e| e.get("apiName").and_then(|v| v.as_str()).map(|s| s.to_string()))
                .collect()
        };
        let (ln, tn) = (names(live), names(target));
        for name in ln {
            if !tn.contains(&name) {
                out.push(DeletionRef { kind: kind.to_string(), api_name: name });
            }
        }
    }
    out
}

// ───────────────────────────── 发布校验（命名/铁律/引用检查） ─────────────────────────────

/// 校验问题严重级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IssueSeverity {
    /// 阻断发布。
    Error,
    /// 提示但可发布。
    Warning,
}

/// 一条发布校验问题。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationIssue {
    pub severity: IssueSeverity,
    pub kind: String,
    pub api_name: String,
    pub message: String,
}

/// 逐 def 结构校验的中间集（kind → [(apiName, 校验结果)]）。
type NamedChecks = Vec<(String, crate::Result<()>)>;

/// canonical 索引（apiName → 剥 version/layout 的规范化形状）。
fn canon_index(snap: &Value, key: &str) -> std::collections::BTreeMap<String, Value> {
    arr_of(snap, key)
        .iter()
        .filter_map(|e| {
            let n = e.get("apiName")?.as_str()?.to_string();
            Some((n, canonical_element(e)))
        })
        .collect()
}

/// 发布门校验：命名/铁律（各 def.validate）+ 引用检查（草稿∪live 解引用）+ 删除清单一致性。
///
/// `live` 为当前已发布快照形状 Value（引用解析允许「草稿删、live 仍有」的过渡态——只要求
/// 发布后的最终集合自洽：被引用者与引用者同批删除合法，留引用者删被引用者报错）。
///
/// **存量债务降级**：fork 把 live 全量带入草稿，未改动元素（同名同 canonical）上的一切问题
/// （结构铁律/引用断链）都是历史债务——降为 Warning 不阻断发布；改动过/新增的元素才 Error。
/// 否则 fork 即被存量脏数据卡死，任何发布都过不了门。
pub fn validate_draft(draft: &DraftContent, live: &Value) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let d = draft.to_snapshot_value();

    // untouched(kind, apiName)：live 同名且 canonical 相等 = fork 原样带回未改动（存量债务判定）。
    let live_maps: std::collections::BTreeMap<&'static str, std::collections::BTreeMap<String, Value>> =
        ELEMENT_KINDS.iter().map(|k| (*k, canon_index(live, kind_key(k).unwrap_or_default()))).collect();
    let draft_maps: std::collections::BTreeMap<&'static str, std::collections::BTreeMap<String, Value>> =
        ELEMENT_KINDS.iter().map(|k| (*k, canon_index(&d, kind_key(k).unwrap_or_default()))).collect();
    let untouched = |kind: &str, name: &str| -> bool {
        match (draft_maps.get(kind).and_then(|m| m.get(name)), live_maps.get(kind).and_then(|m| m.get(name))) {
            (Some(dc), Some(lc)) => dc == lc,
            _ => false, // live 无（新增）或草稿无 → 视为改动过，严格检查
        }
    };

    // 1. 结构/命名/铁律：逐 def 调既有 validate（未改动的存量元素降 Warning）。
    let struct_checks: Vec<(&str, NamedChecks)> = vec![
        (KIND_OBJECT, draft.object_types.iter().map(|x| (x.api_name.clone(), x.validate())).collect()),
        (KIND_LINK, draft.link_types.iter().map(|x| (x.api_name.clone(), x.validate())).collect()),
        (KIND_INTERFACE, draft.interfaces.iter().map(|x| (x.api_name.clone(), x.validate())).collect()),
        (KIND_SHARED, draft.shared_properties.iter().map(|x| (x.api_name.clone(), x.validate())).collect()),
        (KIND_ACTION, draft.action_types.iter().map(|x| (x.api_name.clone(), x.validate())).collect()),
        (KIND_FUNCTION, draft.functions.iter().map(|x| (x.api_name.clone(), x.validate())).collect()),
    ];
    for (kind, items) in struct_checks {
        for (name, r) in items {
            if let Err(e) = r {
                let legacy = untouched(kind, &name);
                issues.push(ValidationIssue {
                    severity: if legacy { IssueSeverity::Warning } else { IssueSeverity::Error },
                    kind: kind.to_string(),
                    api_name: name,
                    message: if legacy { format!("{e}（live 存量债务：元素未改动，不阻断本次发布）") } else { e.to_string() },
                });
            }
        }
    }
    if let Err(e) = draft.validate_shape() {
        issues.push(ValidationIssue {
            severity: IssueSeverity::Error,
            kind: "draft".into(),
            api_name: "*".into(),
            message: e.to_string(),
        });
    }

    // 2. 引用解析集合：最终集合 = live − 草稿删除派生 + 草稿新增/修改（即「应用后 live」）。
    //    检查口径：发布后的世界必须自洽。
    let applied = apply_preview(live, &d);
    let names_of = |snap: &Value, key: &str| -> std::collections::BTreeSet<String> {
        arr_of(snap, key)
            .iter()
            .filter_map(|e| e.get("apiName").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .collect()
    };
    let obj_names = names_of(&applied, "objectTypes");
    let iface_names = names_of(&applied, "interfaces");
    let shared_names = names_of(&applied, "sharedProperties");
    let fn_names = names_of(&applied, "functions");

    // 3. 引用检查（对草稿里每个「仍在最终集合」的元素做解引用）。
    //    untouched 引用者的断链 = live 存量债务 → Warning 不阻断；改动过/新增的 → Error。
    let ref_issue = |kind: &str, api_name: &str, message: String| -> ValidationIssue {
        let legacy = untouched(kind, api_name);
        ValidationIssue {
            severity: if legacy { IssueSeverity::Warning } else { IssueSeverity::Error },
            kind: kind.to_string(),
            api_name: api_name.to_string(),
            message: if legacy { format!("{message}（live 存量债务：引用者未改动，不阻断本次发布）") } else { message },
        }
    };
    for lt in &draft.link_types {
        for (end, name) in [("A", &lt.object_type_a), ("B", &lt.object_type_b)] {
            if !name.is_empty() && !obj_names.contains(name) {
                issues.push(ref_issue(KIND_LINK, &lt.api_name, format!("{end} 端对象类型 {name} 在发布后不存在（被删或未建）")));
            }
        }
    }
    for ot in &draft.object_types {
        if untouched(KIND_OBJECT, &ot.api_name) {
            continue;
        }
        for i in &ot.implements {
            if !iface_names.contains(i) {
                issues.push(ref_issue(KIND_OBJECT, &ot.api_name, format!("实现的接口 {i} 在发布后不存在")));
            }
        }
        for p in &ot.properties {
            if let Some(sp) = &p.shared_property
                && !shared_names.contains(sp)
            {
                issues.push(ref_issue(KIND_OBJECT, &ot.api_name, format!("属性 {} 引用的共享属性 {sp} 在发布后不存在", p.api_name)));
            }
        }
    }
    for i in &draft.interfaces {
        if untouched(KIND_INTERFACE, &i.api_name) {
            continue;
        }
        for sp in &i.properties {
            if !shared_names.contains(sp) {
                issues.push(ref_issue(KIND_INTERFACE, &i.api_name, format!("契约要求的共享属性 {sp} 在发布后不存在")));
            }
        }
        for e in &i.extends {
            if !iface_names.contains(e) {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Warning,
                    kind: KIND_INTERFACE.into(),
                    api_name: i.api_name.clone(),
                    message: format!("继承的父接口 {e} 在发布后不存在"),
                });
            }
        }
    }
    for a in &draft.action_types {
        if untouched(KIND_ACTION, &a.api_name) {
            continue;
        }
        if let Some(fb) = &a.function_backing
            && !fn_names.contains(fb)
        {
            issues.push(ref_issue(KIND_ACTION, &a.api_name, format!("函数背书 {fb} 在发布后不存在")));
        }
    }
    for v in &draft.views {
        if untouched(KIND_VIEW, &v.api_name) {
            continue;
        }
        for o in &v.members.objects {
            if !obj_names.contains(o) {
                issues.push(ref_issue(KIND_VIEW, &v.api_name, format!("成员对象 {o} 在发布后不存在")));
            }
        }
        for i in &v.members.interfaces {
            if !iface_names.contains(i) {
                issues.push(ref_issue(KIND_VIEW, &v.api_name, format!("成员接口 {i} 在发布后不存在")));
            }
        }
    }

    // 4. 删除清单一致性：目标既在草稿又在删除清单 = 矛盾；删除仍被引用者 = 阻断。
    let in_final = |kind: &str, name: &str| -> bool {
        draft_maps.get(kind).is_some_and(|m| m.contains_key(name))
    };
    for del in &draft.deletions {
        if in_final(&del.kind, &del.api_name) {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Error,
                kind: del.kind.clone(),
                api_name: del.api_name.clone(),
                message: "该元素已在草稿中修改/新增，却又出现在删除清单（矛盾，请二选一）".into(),
            });
        }
    }
    // 被删对象仍被（最终集合里的）关系引用 → 悬空边；被删共享属性仍被属性引用 → 悬空引用。
    let removed: std::collections::BTreeSet<(String, String)> = derive_deletions(live, &d)
        .into_iter()
        .map(|x| (x.kind, x.api_name))
        .collect();
    for lt in &draft.link_types {
        for end_name in [&lt.object_type_a, &lt.object_type_b] {
            if removed.contains(&(KIND_OBJECT.into(), end_name.clone())) {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    kind: KIND_OBJECT.into(),
                    api_name: end_name.clone(),
                    message: format!("对象类型仍被关系 {} 引用，不能删除（先删关系）", lt.api_name),
                });
            }
        }
    }
    for ot in &draft.object_types {
        for p in &ot.properties {
            if let Some(sp) = &p.shared_property
                && removed.contains(&(KIND_SHARED.into(), sp.clone()))
            {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    kind: KIND_SHARED.into(),
                    api_name: sp.clone(),
                    message: format!(
                        "共享属性仍被对象 {} 的属性 {} 引用，不能删除",
                        ot.api_name, p.api_name
                    ),
                });
            }
        }
    }
    issues
}

/// 「把草稿应用到 live」的最终集合预览（纯函数）：live − 派生删除 + 草稿全量覆盖。
/// 发布应用与引用校验共用此口径，保证 diff 预览 = 实际变更（方案 §2.4 裁决 1）。
pub fn apply_preview(live: &Value, draft_snap: &Value) -> Value {
    let mut applied = live.clone();
    let obj = applied.as_object_mut().expect("live 快照须为对象");
    for kind in ELEMENT_KINDS {
        let key = kind_key(kind).unwrap_or_default().to_string();
        // 草稿全量覆盖（含视图段之外的六类；views 单独处理）。
        if let Some(arr) = draft_snap.get(&key).cloned() {
            obj.insert(key, arr);
        }
    }
    // views：草稿全量即最终集合（live manual 缺失行由发布删除；auto 行布局豁免不入集合语义）。
    if let Some(views) = draft_snap.get("views").cloned() {
        obj.insert("views".to_string(), views);
    }
    applied
}

// ───────────────────────────── 单测 ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snap(objects: Value, views: Value) -> Value {
        json!({
            "objectTypes": objects,
            "linkTypes": [],
            "interfaces": [],
            "sharedProperties": [],
            "actionTypes": [],
            "functions": [],
            "views": views,
        })
    }

    fn obj(name: &str, display: &str, version: u32) -> Value {
        json!({"apiName": name, "displayName": display, "version": version, "properties": []})
    }

    #[test]
    fn fingerprint_ignores_order_version_layout() {
        let a = snap(json!([obj("A", "甲", 1), obj("B", "乙", 2)]), json!([]));
        let b = snap(json!([obj("B", "乙", 9), obj("A", "甲", 1)]), json!([]));
        // 顺序不同 + 乐观锁 version 漂移 → 同指纹（无实质变更）。
        assert_eq!(snapshot_fingerprint(&a), snapshot_fingerprint(&b));
        // 视图 layout/version 不进指纹。
        let v1 = snap(json!([]), json!([{"apiName": "auto:财务", "layout": {"A": {"x": 1}}, "version": 3}]));
        let v2 = snap(json!([]), json!([{"apiName": "auto:财务", "layout": {"A": {"x": 999}}, "version": 4}]));
        assert_eq!(snapshot_fingerprint(&v1), snapshot_fingerprint(&v2));
        // 语义变更（改显示名）→ 指纹变化。
        let c = snap(json!([obj("A", "甲改", 1), obj("B", "乙", 2)]), json!([]));
        assert_ne!(snapshot_fingerprint(&a), snapshot_fingerprint(&c));
    }

    #[test]
    fn view_member_change_changes_fingerprint() {
        // C1 裁决：视图成员变更必须涨版本（rev 含 views 语义字段）。
        let v1 = snap(
            json!([]),
            json!([{"apiName": "s1", "members": {"objects": ["A"], "interfaces": []}}]),
        );
        let v2 = snap(
            json!([]),
            json!([{"apiName": "s1", "members": {"objects": ["A", "B"], "interfaces": []}}]),
        );
        assert_ne!(snapshot_fingerprint(&v1), snapshot_fingerprint(&v2));
    }

    #[test]
    fn diff_added_modified_removed_and_views() {
        let live = snap(
            json!([obj("A", "甲", 1), obj("B", "乙", 1)]),
            json!([{"apiName": "keep", "members": {"objects": [], "interfaces": []}}]),
        );
        let draft = snap(
            json!([obj("A", "甲改", 2), obj("C", "丙", 0)]),
            json!([
                {"apiName": "keep", "members": {"objects": [], "interfaces": []}},
                {"apiName": "new-s", "members": {"objects": ["C"], "interfaces": []}},
            ]),
        );
        let items = diff_snapshots(&live, &draft);
        let find = |k: &str, n: &str| items.iter().find(|i| i.kind == k && i.api_name == n).map(|i| i.action);
        assert_eq!(find(KIND_OBJECT, "A"), Some(DiffAction::Modified));
        assert_eq!(find(KIND_OBJECT, "B"), Some(DiffAction::Removed));
        assert_eq!(find(KIND_OBJECT, "C"), Some(DiffAction::Added));
        assert_eq!(find(KIND_VIEW, "keep"), None, "未变视图不进 diff");
        assert_eq!(find(KIND_VIEW, "new-s"), Some(DiffAction::Added));
    }

    #[test]
    fn derive_deletions_is_live_minus_target() {
        let live = snap(json!([obj("A", "甲", 1), obj("B", "乙", 1)]), json!([]));
        let target = snap(json!([obj("A", "甲", 1)]), json!([]));
        let dels = derive_deletions(&live, &target);
        assert_eq!(dels.len(), 1);
        assert_eq!(dels[0].kind, KIND_OBJECT);
        assert_eq!(dels[0].api_name, "B");
    }

    fn draft_with(objects: Vec<Value>, deletions: Vec<DeletionRef>) -> DraftContent {
        DraftContent {
            object_types: objects
                .into_iter()
                .map(|o| serde_json::from_value(o).unwrap())
                .collect(),
            deletions,
            ..Default::default()
        }
    }

    #[test]
    fn validate_blocks_dangling_links_and_shared_refs() {
        let live = snap(
            json!([obj("A", "甲", 1), obj("B", "乙", 1)]),
            json!([]),
        );
        // 场景 1：删 A，但关系仍指向 A → 阻断。
        let mut draft = draft_with(vec![], vec![DeletionRef { kind: KIND_OBJECT.into(), api_name: "A".into() }]);
        draft.link_types.push(serde_json::from_value(json!({
            "apiName": "l1", "objectTypeA": "A", "objectTypeB": "B"
        })).unwrap());
        let issues = validate_draft(&draft, &live);
        assert!(issues.iter().any(|i| i.severity == IssueSeverity::Error && i.api_name == "A"),
            "被删对象仍被关系引用应报错：{issues:?}");

        // 场景 2：关系两端与删除同批（B 也删）→ 关系本身也不在草稿 → 悬空检查只看幸存关系。
        let mut draft2 = draft_with(
            vec![],
            vec![
                DeletionRef { kind: KIND_OBJECT.into(), api_name: "A".into() },
                DeletionRef { kind: KIND_OBJECT.into(), api_name: "B".into() },
            ],
        );
        draft2.link_types = vec![]; // 关系一并删除 → 无幸存引用 → 不报
        let issues2 = validate_draft(&draft2, &live);
        assert!(!issues2.iter().any(|i| i.api_name == "A" && i.message.contains("关系")), "{issues2:?}");
    }

    #[test]
    fn validate_flags_deletion_conflict_and_missing_refs() {
        let live = snap(json!([obj("A", "甲", 1)]), json!([]));
        // 既在草稿又在删除清单 → 矛盾错误。
        let mut d1 = draft_with(
            vec![obj("A", "甲改", 0)],
            vec![DeletionRef { kind: KIND_OBJECT.into(), api_name: "A".into() }],
        );
        d1.link_types = vec![];
        let issues = validate_draft(&d1, &live);
        assert!(issues.iter().any(|i| i.message.contains("矛盾")), "{issues:?}");

        // 引用不存在的接口 → 错误。
        let mut d2: DraftContent = DraftContent::default();
        d2.object_types.push(serde_json::from_value(json!({
            "apiName": "C", "implements": ["ghost_iface"]
        })).unwrap());
        let issues2 = validate_draft(&d2, &live);
        assert!(issues2.iter().any(|i| i.message.contains("ghost_iface")), "{issues2:?}");
    }

    #[test]
    fn legacy_broken_refs_downgraded_to_warning() {
        // live 里本就存在的断链（关系指向已删对象）被 fork 原样带回 → 存量债务 Warning 不阻断；
        // 同样的断链若出现在「本草稿修改过」的元素上 → Error 阻断。
        // 注意 live 侧须与生产同型（serde round-trip 后含默认字段），手搓偏序 JSON 会让
        // canonical 不等而误判「改动过」——生产 snapshot_value 两侧均为 to_value(def)。
        let link: crate::LinkTypeDef = serde_json::from_value(json!({
            "apiName": "l1", "objectTypeA": "A", "objectTypeB": "Ghost"
        })).unwrap();
        let live_link = serde_json::to_value(&link).unwrap();
        let mut live = snap(json!([obj("A", "甲", 1)]), json!([]));
        live["linkTypes"] = json!([live_link.clone()]);

        // 存量：断链关系与 live 完全一致（canonical 相等）→ 仅 Warning。
        let draft_forked: DraftContent = serde_json::from_value(live.clone()).unwrap();
        let issues = validate_draft(&draft_forked, &live);
        let l1 = issues.iter().find(|i| i.api_name == "l1");
        assert!(l1.is_some(), "存量断链应至少提示：{issues:?}");
        assert_eq!(l1.unwrap().severity, IssueSeverity::Warning, "存量断链应为 Warning：{issues:?}");

        // 改动过：同一条关系被草稿修改（displayName 变化）→ Error 阻断。
        let mut modified = link.clone();
        modified.display_name = "改名了".into();
        let mut draft_mod: DraftContent = serde_json::from_value(live.clone()).unwrap();
        draft_mod.link_types = vec![modified];
        let issues2 = validate_draft(&draft_mod, &live);
        let l1m = issues2.iter().find(|i| i.api_name == "l1");
        assert!(l1m.is_some(), "改过的断链关系应报：{issues2:?}");
        assert_eq!(l1m.unwrap().severity, IssueSeverity::Error, "改动过元素断链应为 Error：{issues2:?}");
    }

    #[test]
    fn draft_content_roundtrips_from_snapshot() {
        let s = snap(json!([obj("A", "甲", 1)]), json!([{"apiName": "s1"}]));
        let d: DraftContent = serde_json::from_value(s).unwrap();
        assert_eq!(d.object_types.len(), 1);
        assert_eq!(d.views.len(), 1);
        assert!(d.deletions.is_empty());
        // 往返幂等：fork→保存→再转指纹恒定（serde 会补默认字段如 status，首次往返即稳定；
        // 生产两侧 live/draft 均走同型序列化，不会产生假 diff）。
        let back = d.to_snapshot_value();
        let d2: DraftContent = serde_json::from_value(back).unwrap();
        let back2 = d2.to_snapshot_value();
        assert_eq!(snapshot_fingerprint(&back2), snapshot_fingerprint(&d2.to_snapshot_value()));
        let d3: DraftContent = serde_json::from_value(back2.clone()).unwrap();
        assert_eq!(
            snapshot_fingerprint(&d3.to_snapshot_value()),
            snapshot_fingerprint(&back2),
            "往返幂等（fork 不产生假差异）"
        );
    }
}
