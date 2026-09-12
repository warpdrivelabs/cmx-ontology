//! 存档/回滚（直改 live 架构）——版本快照指纹/diff/校验/派生删除纯函数（驱动无关）。
//!
//! 纪律：om_* 是「已发布真源」（消费方读取路径零改动）；编辑直写 live，存档 = 用户主动
//! 把 live 打成 om_version 检查点；回滚 = 把历史快照整体恢复回 live（同过校验+护栏）。
//! 本模块只放**驱动无关**的形状与纯函数（store 实现见 cmx-onto-store-pg/snapshot_store.rs）。
//!
//! 指纹口径：rev = xxh64，覆盖七类元素 + views **语义字段**（名称/描述/source/成员清单），
//! **排除 layout 与审计字段**（乐观锁 version）——纯布局拖动不产生新存档；数组顺序不敏感。

use crate::def::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ───────────────────────────── 元素类别常量 ─────────────────────────────

/// 七类元素（六类 om_* 元素 + 场景视图段）的 kind 标识（camelCase，前端同口径）。
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

/// kind → 快照里的数组键名。
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

/// 快照中六类元素总数（大规模删除护栏阈值口径：`max(50, total/5)`）。
pub fn element_total(snapshot: &Value) -> usize {
    ELEMENT_KINDS
        .iter()
        .filter_map(|k| kind_key(k))
        .map(|key| snapshot.get(key).and_then(|v| v.as_array()).map_or(0, |a| a.len()))
        .sum()
}

// ───────────────────────────── 形状 ─────────────────────────────

/// 显式删除引用（回滚派生时计算；校验阶段做一致性检查）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeletionRef {
    /// 元素 kind（ELEMENT_KINDS 之一）。
    pub kind: String,
    pub api_name: String,
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

/// 视图指纹规范化：剔除 layout 与 version（纯布局拖动/乐观锁漂移不进指纹）。
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

/// 快照指纹（rev）：七类元素 canonical 化后按 apiName 排序拼接，xxh64 十六进制。
/// 顺序不敏感 + 排除 layout/审计字段。
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

/// 元素级 diff：base → target。base 有 target 无 = Removed（回滚应用时按此派生删除集）；
/// 两均有但指纹不同 = Modified。视图 diff 排除 layout。
pub fn diff_snapshots(base: &Value, target: &Value) -> Vec<DiffItem> {
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
        let (li, di) = (index(base), index(target));
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
    // views 段：canonical（无 layout/version）对比。
    let vindex = |snap: &Value| -> std::collections::BTreeMap<String, Value> {
        arr_of(snap, "views")
            .iter()
            .filter_map(|e| {
                let name = e.get("apiName").and_then(|v| v.as_str())?.to_string();
                Some((name, canonical_view(e)))
            })
            .collect()
    };
    let (lv, dv) = (vindex(base), vindex(target));
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
                summary: "当前有、目标无 → 应用时删除该场景（manual 行）".into(),
            });
        }
    }
    out
}

/// 元素摘要：对象/视图给数量对比，其余给显示名对比。
fn diff_summary(kind: &str, live: Option<&Value>, target: Option<&Value>) -> String {
    let name = |v: Option<&Value>| {
        v.and_then(|x| x.get("displayName")).and_then(|d| d.as_str()).unwrap_or("").to_string()
    };
    match kind {
        KIND_OBJECT => {
            let pc = |v: Option<&Value>| {
                v.and_then(|x| x.get("properties")).and_then(|p| p.as_array()).map(|a| a.len()).unwrap_or(0)
            };
            format!("属性数 {} → {}", pc(live), pc(target))
        }
        KIND_VIEW => {
            let members = |v: Option<&Value>| {
                format!(
                    "{}对象/{}接口",
                    v.and_then(|x| x.get("members")).and_then(|m| m.get("objects")).and_then(|o| o.as_array()).map(|a| a.len()).unwrap_or(0),
                    v.and_then(|x| x.get("members")).and_then(|m| m.get("interfaces")).and_then(|o| o.as_array()).map(|a| a.len()).unwrap_or(0),
                )
            };
            format!("成员 {} → {}", members(live), members(target))
        }
        _ => format!("显示名 {} → {}", name(live), name(target)),
    }
}

/// 「base − target」派生删除集（六类元素；回滚应用与此口径共用）。
pub fn derive_deletions(base: &Value, target: &Value) -> Vec<DeletionRef> {
    let mut out = Vec::new();
    for kind in ELEMENT_KINDS {
        let key = kind_key(kind).unwrap_or_default();
        let names = |snap: &Value| -> std::collections::BTreeSet<String> {
            arr_of(snap, key)
                .iter()
                .filter_map(|e| e.get("apiName").and_then(|v| v.as_str()).map(|s| s.to_string()))
                .collect()
        };
        let (ln, tn) = (names(base), names(target));
        for name in ln {
            if !tn.contains(&name) {
                out.push(DeletionRef { kind: kind.to_string(), api_name: name });
            }
        }
    }
    out
}

// ───────────────────────────── 校验（引用/命名/删除一致性） ─────────────────────────────

/// 校验问题严重级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IssueSeverity {
    /// 阻断应用。
    Error,
    /// 提示但可应用。
    Warning,
}

/// 一条校验问题。
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

/// canonical 索引（apiName → 剥 version 的规范化形状）。
fn canon_index(snap: &Value, key: &str) -> std::collections::BTreeMap<String, Value> {
    arr_of(snap, key)
        .iter()
        .filter_map(|e| {
            let n = e.get("apiName")?.as_str()?.to_string();
            Some((n, canonical_element(e)))
        })
        .collect()
}

/// 快照应用校验（Value 基，直改架构版）：命名/铁律（逐元素反序列化 + def.validate）+
/// 引用检查（target ∪ live 解引用）+ 删除清单一致性。
///
/// `target` 为待应用快照（Value 形状），`live` 为当前快照；`deletions` = live − target
/// 派生删除集（调用方算好传入）。口径：应用后的世界必须自洽。
///
/// **存量债务降级**：target 与 live 同名同 canonical 的元素上的一切问题都是历史债务——
/// 降为 Warning 不阻断；改动过/新增的元素才 Error。否则存量脏数据会卡死任何回滚。
pub fn validate_snapshot(target: &Value, deletions: &[DeletionRef], live: &Value) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    // untouched(kind, apiName)：live 同名且 canonical 相等 = 原样带回未改动（存量债务判定）。
    let live_maps: std::collections::BTreeMap<&'static str, std::collections::BTreeMap<String, Value>> =
        ELEMENT_KINDS.iter().map(|k| (*k, canon_index(live, kind_key(k).unwrap_or_default()))).collect();
    let target_maps: std::collections::BTreeMap<&'static str, std::collections::BTreeMap<String, Value>> =
        ELEMENT_KINDS.iter().map(|k| (*k, canon_index(target, kind_key(k).unwrap_or_default()))).collect();
    let untouched = |kind: &str, name: &str| -> bool {
        match (target_maps.get(kind).and_then(|m| m.get(name)), live_maps.get(kind).and_then(|m| m.get(name))) {
            (Some(tc), Some(lc)) => tc == lc,
            _ => false,
        }
    };

    // 反序列化单个元素（失败 → 结构 Error；untouched 降 Warning——存量债务）。
    trait DefValidate {
        fn validate_def(&self) -> crate::Result<()>;
    }
    impl DefValidate for ObjectTypeDef {
        fn validate_def(&self) -> crate::Result<()> { self.validate() }
    }
    impl DefValidate for LinkTypeDef {
        fn validate_def(&self) -> crate::Result<()> { self.validate() }
    }
    impl DefValidate for InterfaceDef {
        fn validate_def(&self) -> crate::Result<()> { self.validate() }
    }
    impl DefValidate for SharedPropertyTypeDef {
        fn validate_def(&self) -> crate::Result<()> { self.validate() }
    }
    impl DefValidate for ActionTypeDef {
        fn validate_def(&self) -> crate::Result<()> { self.validate() }
    }
    impl DefValidate for FunctionDef {
        fn validate_def(&self) -> crate::Result<()> { self.validate() }
    }
    fn elem<T: serde::de::DeserializeOwned + DefValidate>(target: &Value, key: &str) -> NamedChecks {
        arr_of(target, key)
            .iter()
            .map(|e| {
                let name = e.get("apiName").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let r = serde_json::from_value::<T>(e.clone())
                    .map_err(|e| crate::Error::Definition(format!("结构反序列化失败: {e}")))
                    .and_then(|def| def.validate_def());
                (name, r)
            })
            .collect()
    }

    let struct_checks: Vec<(&str, NamedChecks)> = vec![
        (KIND_OBJECT, elem::<ObjectTypeDef>(target, "objectTypes")),
        (KIND_LINK, elem::<LinkTypeDef>(target, "linkTypes")),
        (KIND_INTERFACE, elem::<InterfaceDef>(target, "interfaces")),
        (KIND_SHARED, elem::<SharedPropertyTypeDef>(target, "sharedProperties")),
        (KIND_ACTION, elem::<ActionTypeDef>(target, "actionTypes")),
        (KIND_FUNCTION, elem::<FunctionDef>(target, "functions")),
    ];
    for (kind, items) in struct_checks {
        for (name, r) in items {
            if let Err(e) = r {
                let legacy = untouched(kind, &name);
                issues.push(ValidationIssue {
                    severity: if legacy { IssueSeverity::Warning } else { IssueSeverity::Error },
                    kind: kind.to_string(),
                    api_name: name,
                    message: if legacy { format!("{e}（live 存量债务：元素未改动，不阻断本次应用）") } else { e.to_string() },
                });
            }
        }
    }
    // target 内对象类型重复（同 apiName 出现两次）。
    {
        let mut seen = std::collections::HashSet::new();
        for e in arr_of(target, "objectTypes") {
            if let Some(n) = e.get("apiName").and_then(|v| v.as_str())
                && !seen.insert(n.to_string())
            {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    kind: KIND_OBJECT.into(),
                    api_name: n.to_string(),
                    message: format!("目标快照内对象类型 {n} 重复"),
                });
            }
        }
    }
    // 删除清单合法性。
    for d in deletions {
        if kind_key(&d.kind).is_none() || d.api_name.is_empty() {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Error,
                kind: d.kind.clone(),
                api_name: d.api_name.clone(),
                message: format!("删除清单项非法：kind={:?} apiName={:?}", d.kind, d.api_name),
            });
        }
    }

    // 引用解析集合：应用后最终态（live − 派生删除 + target 覆盖）。
    let applied = apply_preview(live, target);
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

    let ref_issue = |kind: &str, api_name: &str, message: String| -> ValidationIssue {
        let legacy = untouched(kind, api_name);
        ValidationIssue {
            severity: if legacy { IssueSeverity::Warning } else { IssueSeverity::Error },
            kind: kind.to_string(),
            api_name: api_name.to_string(),
            message: if legacy { format!("{message}（live 存量债务：引用者未改动，不阻断本次应用）") } else { message },
        }
    };

    // 关系两端引用（从 Value 读取，避免整体反序列化失败掩盖逐元素问题）。
    for e in arr_of(target, "linkTypes") {
        let (Some(name), Some(a), Some(b)) = (
            e.get("apiName").and_then(|v| v.as_str()),
            e.get("objectTypeA").and_then(|v| v.as_str()),
            e.get("objectTypeB").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        for (end, obj) in [("A", a), ("B", b)] {
            if !obj.is_empty() && !obj_names.contains(obj) {
                issues.push(ref_issue(KIND_LINK, name, format!("{end} 端对象类型 {obj} 在应用后不存在（被删或未建）")));
            }
        }
    }
    // 对象 implements / 共享属性引用（跳过 untouched 存量）。
    for e in arr_of(target, "objectTypes") {
        let Some(name) = e.get("apiName").and_then(|v| v.as_str()) else { continue };
        if untouched(KIND_OBJECT, name) {
            continue;
        }
        if let Some(implements) = e.get("implements").and_then(|v| v.as_array()) {
            for i in implements {
                if let Some(i) = i.as_str()
                    && !iface_names.contains(i)
                {
                    issues.push(ref_issue(KIND_OBJECT, name, format!("实现的接口 {i} 在应用后不存在")));
                }
            }
        }
        if let Some(props) = e.get("properties").and_then(|v| v.as_array()) {
            for p in props {
                if let Some(sp) = p.get("sharedProperty").and_then(|v| v.as_str())
                    && !shared_names.contains(sp)
                {
                    let papi = p.get("apiName").and_then(|v| v.as_str()).unwrap_or("?");
                    issues.push(ref_issue(KIND_OBJECT, name, format!("属性 {papi} 引用的共享属性 {sp} 在应用后不存在")));
                }
            }
        }
    }
    for e in arr_of(target, "interfaces") {
        let Some(name) = e.get("apiName").and_then(|v| v.as_str()) else { continue };
        if untouched(KIND_INTERFACE, name) {
            continue;
        }
        if let Some(props) = e.get("properties").and_then(|v| v.as_array()) {
            for sp in props {
                if let Some(sp) = sp.as_str()
                    && !shared_names.contains(sp)
                {
                    issues.push(ref_issue(KIND_INTERFACE, name, format!("契约要求的共享属性 {sp} 在应用后不存在")));
                }
            }
        }
        if let Some(extends) = e.get("extends").and_then(|v| v.as_array()) {
            for ext in extends {
                if let Some(ext) = ext.as_str()
                    && !iface_names.contains(ext)
                {
                    issues.push(ValidationIssue {
                        severity: IssueSeverity::Warning,
                        kind: KIND_INTERFACE.into(),
                        api_name: name.to_string(),
                        message: format!("继承的父接口 {ext} 在应用后不存在"),
                    });
                }
            }
        }
    }
    for e in arr_of(target, "actionTypes") {
        let Some(name) = e.get("apiName").and_then(|v| v.as_str()) else { continue };
        if untouched(KIND_ACTION, name) {
            continue;
        }
        if let Some(fb) = e.get("functionBacking").and_then(|v| v.as_str())
            && !fn_names.contains(fb)
        {
            issues.push(ref_issue(KIND_ACTION, name, format!("函数背书 {fb} 在应用后不存在")));
        }
    }
    for v in arr_of(target, "views") {
        let Some(name) = v.get("apiName").and_then(|v| v.as_str()) else { continue };
        if untouched(KIND_VIEW, name) {
            continue;
        }
        let members = v.get("members");
        for o in members.and_then(|m| m.get("objects")).and_then(|o| o.as_array()).into_iter().flatten() {
            if let Some(o) = o.as_str()
                && !obj_names.contains(o)
            {
                issues.push(ref_issue(KIND_VIEW, name, format!("成员对象 {o} 在应用后不存在")));
            }
        }
        for i in members.and_then(|m| m.get("interfaces")).and_then(|o| o.as_array()).into_iter().flatten() {
            if let Some(i) = i.as_str()
                && !iface_names.contains(i)
            {
                issues.push(ref_issue(KIND_VIEW, name, format!("成员接口 {i} 在应用后不存在")));
            }
        }
    }

    // 被删对象仍被幸存关系引用 / 被删共享属性仍被引用 → 阻断。
    let removed: std::collections::BTreeSet<(String, String)> =
        deletions.iter().map(|d| (d.kind.clone(), d.api_name.clone())).collect();
    for e in arr_of(target, "linkTypes") {
        let Some(lt) = e.get("apiName").and_then(|v| v.as_str()) else { continue };
        for end in ["objectTypeA", "objectTypeB"] {
            if let Some(end_name) = e.get(end).and_then(|v| v.as_str())
                && removed.contains(&(KIND_OBJECT.to_string(), end_name.to_string()))
            {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    kind: KIND_OBJECT.into(),
                    api_name: end_name.to_string(),
                    message: format!("对象类型仍被关系 {lt} 引用，不能删除（先删关系）"),
                });
            }
        }
    }
    for e in arr_of(target, "objectTypes") {
        let Some(ot) = e.get("apiName").and_then(|v| v.as_str()) else { continue };
        if let Some(props) = e.get("properties").and_then(|v| v.as_array()) {
            for p in props {
                if let Some(sp) = p.get("sharedProperty").and_then(|v| v.as_str())
                    && removed.contains(&(KIND_SHARED.to_string(), sp.to_string()))
                {
                    let papi = p.get("apiName").and_then(|v| v.as_str()).unwrap_or("?");
                    issues.push(ValidationIssue {
                        severity: IssueSeverity::Error,
                        kind: KIND_SHARED.into(),
                        api_name: sp.to_string(),
                        message: format!("共享属性仍被对象 {ot} 的属性 {papi} 引用，不能删除"),
                    });
                }
            }
        }
    }
    issues
}

/// 「把目标快照应用到 live」的最终集合预览（纯函数）：live 覆盖 target 键集。
/// 回滚应用与引用校验共用此口径，保证 diff 预览 = 实际变更。
pub fn apply_preview(live: &Value, target: &Value) -> Value {
    let mut applied = live.clone();
    let obj = applied.as_object_mut().expect("live 快照须为对象");
    for kind in ELEMENT_KINDS {
        let key = kind_key(kind).unwrap_or_default().to_string();
        if let Some(arr) = target.get(&key).cloned() {
            obj.insert(key, arr);
        }
    }
    if let Some(views) = target.get("views").cloned() {
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
        assert_eq!(snapshot_fingerprint(&a), snapshot_fingerprint(&b));
        let v1 = snap(json!([]), json!([{"apiName": "auto:财务", "layout": {"A": {"x": 1}}, "version": 3}]));
        let v2 = snap(json!([]), json!([{"apiName": "auto:财务", "layout": {"A": {"x": 999}}, "version": 4}]));
        assert_eq!(snapshot_fingerprint(&v1), snapshot_fingerprint(&v2));
        let c = snap(json!([obj("A", "甲改", 1), obj("B", "乙", 2)]), json!([]));
        assert_ne!(snapshot_fingerprint(&a), snapshot_fingerprint(&c));
    }

    #[test]
    fn view_member_change_changes_fingerprint() {
        let v1 = snap(json!([]), json!([{"apiName": "s1", "members": {"objects": ["A"], "interfaces": []}}]));
        let v2 = snap(json!([]), json!([{"apiName": "s1", "members": {"objects": ["A", "B"], "interfaces": []}}]));
        assert_ne!(snapshot_fingerprint(&v1), snapshot_fingerprint(&v2));
    }

    #[test]
    fn diff_added_modified_removed_and_views() {
        let live = snap(
            json!([obj("A", "甲", 1), obj("B", "乙", 1)]),
            json!([{"apiName": "keep", "members": {"objects": [], "interfaces": []}}]),
        );
        let target = snap(
            json!([obj("A", "甲改", 2), obj("C", "丙", 0)]),
            json!([
                {"apiName": "keep", "members": {"objects": [], "interfaces": []}},
                {"apiName": "new-s", "members": {"objects": ["C"], "interfaces": []}},
            ]),
        );
        let items = diff_snapshots(&live, &target);
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

    #[test]
    fn validate_blocks_dangling_links_and_shared_refs() {
        let live = snap(json!([obj("A", "甲", 1), obj("B", "乙", 1)]), json!([]));
        // 删 A、B，但幸存关系 l1 仍指向 A → 阻断。
        let target = snap(json!([]), json!([]));
        let mut link = json!({"apiName": "l1", "objectTypeA": "A", "objectTypeB": "B"});
        let deletions = derive_deletions(&live, &target);
        // 关系 l1 也在删除集中（target 无 linkTypes）→ 无幸存引用 → 不报。
        let issues = validate_snapshot(&target, &deletions, &live);
        assert!(!issues.iter().any(|i| i.api_name == "A" && i.message.contains("关系")), "{issues:?}");

        // 幸存关系（target 保留 l1）指向被删的 A → 阻断。
        let target2 = snap(json!([]), json!([]));
        let mut t2 = target2.clone();
        t2["linkTypes"] = json!([link.clone()]);
        let deletions2 = derive_deletions(&live, &t2);
        let issues2 = validate_snapshot(&t2, &deletions2, &live);
        assert!(issues2.iter().any(|i| i.severity == IssueSeverity::Error && i.api_name == "A"),
            "幸存关系指向被删对象应报错：{issues2:?}");
        let _ = &mut link;
    }

    #[test]
    fn validate_flags_missing_refs() {
        let live = snap(json!([obj("A", "甲", 1)]), json!([]));
        // target 新增对象 C 引用不存在的接口 → 错误。
        let target = json!({
            "objectTypes": [{"apiName": "C", "implements": ["ghost_iface"], "properties": []}],
            "linkTypes": [], "interfaces": [], "sharedProperties": [],
            "actionTypes": [], "functions": [], "views": [],
        });
        let issues = validate_snapshot(&target, &[], &live);
        assert!(issues.iter().any(|i| i.message.contains("ghost_iface")), "{issues:?}");
    }

    #[test]
    fn legacy_broken_refs_downgraded_to_warning() {
        // live 里本就存在的断链关系被原样带回（canonical 相等）→ 仅 Warning；
        // 同一条关系被改动（displayName 变化）→ Error 阻断。
        let live_link = json!({"apiName": "l1", "displayName": "l1", "cardinality": "oneToMany",
            "objectTypeA": "A", "objectTypeB": "Ghost", "roleA": "", "roleB": "", "backing": null,
            "status": "experimental"});
        let mut live = snap(json!([obj("A", "甲", 1)]), json!([]));
        live["linkTypes"] = json!([live_link.clone()]);

        let issues = validate_snapshot(&live, &[], &live);
        let l1 = issues.iter().find(|i| i.api_name == "l1");
        assert!(l1.is_some(), "存量断链应至少提示：{issues:?}");
        assert_eq!(l1.unwrap().severity, IssueSeverity::Warning, "存量断链应为 Warning：{issues:?}");

        let mut modified = live_link.clone();
        modified["displayName"] = json!("改名了");
        let mut target = live.clone();
        target["linkTypes"] = json!([modified]);
        let deletions = derive_deletions(&live, &target);
        let issues2 = validate_snapshot(&target, &deletions, &live);
        let l1m = issues2.iter().find(|i| i.api_name == "l1");
        assert_eq!(l1m.map(|i| i.severity), Some(IssueSeverity::Error), "改动过元素断链应为 Error：{issues2:?}");
    }
}
