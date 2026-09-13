//! O4 动作引擎 · 内核（纯逻辑、零 IO、零时钟、可单测）。
//!
//! 一个 [`ActionTypeDef`](crate::ActionTypeDef) 的 `logic` 是一串**编辑操作**（对象/关系的增删改）。
//! 执行动作 = 用调用方传入的参数 `params` + 对象状态 `objects`（壳层预装载）解析 `logic`
//! → [`ObjectEdit`] 列表 → 交存储层原子写回。
//!
//! 参数替换：`logic` 里任意等于 `"$name"` 的字符串替换为 `params.name`（递归 properties/set 等）。
//! 值映射来源（对标 Palantir Rules: Values and parameters，显式 `src` 保留字约定）：
//! `param` / `static` / `currentUser` / `currentTime` / `paramProperty`——对象字面量含 `"src"` 键
//! 即被解析为映射来源（递归作用于 set/properties 的嵌套对象与数组）；确需写入形如 `{"src":..}`
//! 的静态对象必须显式 `{"src":"static","value":{...}}` 包裹。
//!
//! 提交校验（`validations`，FEEL，fail-closed）、副作用（`side_effects`，随编辑同事务入 Outbox）
//! 与函数背书（`function_backing`，Run function rule 最小闭环：函数返回编辑 JSON）见各函数文档。

use crate::def::ActionTypeDef;
use crate::feel::eval_expression;
use serde_json::{Map, Value};

/// 单条编辑操作（动作写回的原子单元）。
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectEdit {
    /// upsert 一个对象（pk 冲突则整行覆盖 title+props）。
    CreateObject {
        object_type: String,
        pk: String,
        title: String,
        properties: Value,
    },
    /// 显式 upsert（createOrModifyObject）：存在 → 等同 ModifyObject（set 合并）；不存在 → 全量落。
    UpsertObject {
        object_type: String,
        pk: String,
        title: String,
        set: Value,
    },
    /// 合并修改某对象的部分属性（读改写；对象须存在）。
    ModifyObject {
        object_type: String,
        pk: String,
        set: Value,
    },
    /// 删除对象（连带清其关系边）。
    DeleteObject { object_type: String, pk: String },
    /// 建一条关系边（幂等）。
    AddLink {
        link: String,
        a_pk: String,
        b_pk: String,
        properties: Value,
    },
    /// 删一条关系边。
    RemoveLink {
        link: String,
        a_pk: String,
        b_pk: String,
    },
}

impl ObjectEdit {
    /// 该编辑的主键定位（对象类编辑返回 (objectType, pk)；链接类 None）——组合序列校验用。
    fn object_key(&self) -> Option<(&str, &str)> {
        match self {
            ObjectEdit::CreateObject { object_type, pk, .. }
            | ObjectEdit::UpsertObject { object_type, pk, .. }
            | ObjectEdit::ModifyObject { object_type, pk, .. }
            | ObjectEdit::DeleteObject { object_type, pk } => Some((object_type, pk)),
            _ => None,
        }
    }
}

/// 标量 JSON → 字符串（pk/aPk/bPk 用）。
fn scalar_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// 递归参数替换：字符串 `"$name"` → `params.name`；数组/对象递归（不触碰映射来源对象——
/// 含 `"src"` 键的对象由 [`resolve_value`] 先行解析，此处仅作兜底递归）。
fn subst(v: &Value, params: &Value) -> Value {
    match v {
        Value::String(s) => {
            if let Some(name) = s.strip_prefix('$')
                && let Some(pv) = params.get(name) {
                    return pv.clone();
                }
            v.clone()
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| subst(x, params)).collect()),
        Value::Object(m) => {
            if m.contains_key("src") {
                return v.clone(); // 保留字：映射来源对象原样交给 resolve_value
            }
            let mut out = Map::new();
            for (k, x) in m {
                out.insert(k.clone(), subst(x, params));
            }
            Value::Object(out)
        }
        _ => v.clone(),
    }
}

/// 值解析：五种显式映射来源 + `$name` 替换（递归对象/数组）。
///
/// - `{"src":"param","name":"x"}` → params.x（缺失即错）
/// - `{"src":"static","value":..}` → value（原样）
/// - `{"src":"currentUser"}` → actor（缺 actor 即错）
/// - `{"src":"currentTime"}` → now（缺 now 即错）
/// - `{"src":"paramProperty","param":"p","property":"f"}` → objects.p.f（对象未装载/属性缺失即错）
fn resolve_value(
    v: &Value,
    params: &Value,
    objects: &Value,
    actor: Option<&str>,
    now: Option<&str>,
) -> Result<Value, String> {
    if let Value::Object(m) = v {
        if let Some(src) = m.get("src").and_then(Value::as_str) {
            return match src {
                "param" => {
                    let name = m.get("name").and_then(Value::as_str).unwrap_or("");
                    if name.is_empty() {
                        return Err("映射来源 param 缺 name 字段".into());
                    }
                    params
                        .get(name)
                        .cloned()
                        .ok_or_else(|| format!("映射来源 param「{name}」在参数中不存在"))
                }
                "static" => m
                    .get("value")
                    .cloned()
                    .ok_or_else(|| "映射来源 static 缺 value 字段".to_string()),
                "currentUser" => actor
                    .map(|a| Value::String(a.to_string()))
                    .ok_or_else(|| "映射来源 currentUser：无当前用户上下文".to_string()),
                "currentTime" => now
                    .map(|t| Value::String(t.to_string()))
                    .ok_or_else(|| "映射来源 currentTime：无时间上下文".to_string()),
                "paramProperty" => {
                    let pname = m.get("param").and_then(Value::as_str).unwrap_or("");
                    let prop = m.get("property").and_then(Value::as_str).unwrap_or("");
                    if pname.is_empty() || prop.is_empty() {
                        return Err("映射来源 paramProperty 缺 param/property 字段".into());
                    }
                    let obj = objects.get(pname).ok_or_else(|| {
                        format!("映射来源 paramProperty「{pname}.{prop}」：参数对象未装载")
                    })?;
                    obj.get(prop).cloned().ok_or_else(|| {
                        format!("映射来源 paramProperty「{pname}.{prop}」：对象属性不存在")
                    })
                }
                other => Err(format!("未知映射来源「{other}」（支持 param/static/currentUser/currentTime/paramProperty）")),
            };
        }
        // 普通对象：递归各字段
        let mut out = Map::new();
        for (k, x) in m {
            out.insert(k.clone(), resolve_value(x, params, objects, actor, now)?);
        }
        return Ok(Value::Object(out));
    }
    if let Value::Array(a) = v {
        let mut out = Vec::with_capacity(a.len());
        for x in a {
            out.push(resolve_value(x, params, objects, actor, now)?);
        }
        return Ok(Value::Array(out));
    }
    // 标量：先整串 `$name` 替换（语法糖），再对含 `$ident` 的拼接串做内插
    // （如 `prio-$priority` → `prio-P2`；两步都无法解析的 `$x` 原样保留，向后兼容）。
    match subst(v, params) {
        Value::String(s) if s.contains('$') => {
            let out = interpolate(&s, params);
            Ok(Value::String(if out == s { s } else { out }))
        }
        done => Ok(done),
    }
}

/// 取 op 内某字符串字段（缺失/空 → Err）。
fn str_field(op: &Value, key: &str) -> Result<String, String> {
    op.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("编辑操作缺字段「{key}」"))
}

/// 取 op 内某标量字段并转字符串（pk/aPk/bPk；解析参数后可能是数字）。
fn scalar_field(op: &Value, key: &str) -> Result<String, String> {
    op.get(key)
        .and_then(scalar_to_string)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("编辑操作缺字段「{key}」（或非标量）"))
}

/// 解析单条编辑操作 JSON（已代入值）→ [`ObjectEdit`]。
/// `resolve_edits`（替换/映射后）与 `parse_edits_json`（函数返回值）共用。
fn parse_op(op: &Value) -> Result<ObjectEdit, String> {
    let kind = op.get("op").and_then(|v| v.as_str()).unwrap_or("");
    let edit = match kind {
        "createObject" => ObjectEdit::CreateObject {
            object_type: str_field(op, "objectType")?,
            pk: scalar_field(op, "pk")?,
            title: op
                .get("title")
                .and_then(scalar_to_string)
                .unwrap_or_else(|| scalar_field(op, "pk").unwrap_or_default()),
            properties: op.get("properties").cloned().unwrap_or(Value::Object(Map::new())),
        },
        "createOrModifyObject" => ObjectEdit::UpsertObject {
            object_type: str_field(op, "objectType")?,
            pk: scalar_field(op, "pk")?,
            title: op.get("title").and_then(scalar_to_string).unwrap_or_default(),
            set: op.get("set").cloned().unwrap_or(Value::Object(Map::new())),
        },
        "modifyObject" => ObjectEdit::ModifyObject {
            object_type: str_field(op, "objectType")?,
            pk: scalar_field(op, "pk")?,
            set: op.get("set").cloned().unwrap_or(Value::Object(Map::new())),
        },
        "deleteObject" => ObjectEdit::DeleteObject {
            object_type: str_field(op, "objectType")?,
            pk: scalar_field(op, "pk")?,
        },
        "addLink" => ObjectEdit::AddLink {
            link: str_field(op, "link")?,
            a_pk: scalar_field(op, "aPk")?,
            b_pk: scalar_field(op, "bPk")?,
            properties: op.get("properties").cloned().unwrap_or(Value::Object(Map::new())),
        },
        "removeLink" => ObjectEdit::RemoveLink {
            link: str_field(op, "link")?,
            a_pk: scalar_field(op, "aPk")?,
            b_pk: scalar_field(op, "bPk")?,
        },
        "" => return Err("编辑操作缺 op 字段".to_string()),
        other => return Err(format!("未知编辑操作「{other}」")),
    };
    Ok(edit)
}

/// 结构化校验必填参数（`parameters` 里 `required=true` 的须在 `params` 中非空）+ 约束校验。
///
/// 约束（P1-2）：`constraints.kind == "multipleChoice"` 时取值须在 `options` 内（缺省值已由
/// [`apply_param_defaults`] 补齐后再进来）。非布尔真式校验不在本层。
pub fn validate_params(action: &ActionTypeDef, params: &Value) -> Result<(), String> {
    if let Some(ps) = action.parameters.as_array() {
        for p in ps {
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            let required = p.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
            let missing = params.get(name).map(|v| v.is_null()).unwrap_or(true);
            if required && missing {
                return Err(format!("缺必填参数「{name}」"));
            }
            // multipleChoice 约束：提供了值才校验（required=false 且未提供时跳过）
            if let Some(opts) = p
                .get("constraints")
                .filter(|c| c.get("kind").and_then(|v| v.as_str()) == Some("multipleChoice"))
                .and_then(|c| c.get("options"))
                .and_then(Value::as_array)
                && let Some(v) = params.get(name).filter(|v| !v.is_null()) {
                    let sv = v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string());
                    let hit = opts.iter().any(|o| {
                        o.as_str().map(|s| s == sv).unwrap_or_else(|| *o == sv)
                    });
                    if !hit {
                        let list: Vec<String> =
                            opts.iter().map(|o| o.as_str().unwrap_or("").to_string()).collect();
                        return Err(format!(
                            "参数「{name}」取值 {sv} 不在可选范围 [{}]",
                            list.join(", ")
                        ));
                    }
                }
        }
    }
    Ok(())
}

/// 参数默认值补齐（P1-2）：`defaultValue` 存在且 params 缺该参数时补入（再跑必填校验）。
pub fn apply_param_defaults(action: &ActionTypeDef, params: &mut Value) {
    let Some(ps) = action.parameters.as_array() else { return };
    let map = match params.as_object_mut() {
        Some(m) => m,
        None => return,
    };
    for p in ps {
        let name = match p.get("name").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() => n,
            _ => continue,
        };
        let missing = map.get(name).map(|v| v.is_null()).unwrap_or(true);
        if missing
            && let Some(dv) = p.get("defaultValue").filter(|v| !v.is_null()) {
                map.insert(name.to_string(), dv.clone());
            }
    }
}

/// 提交校验上下文（壳层组装的纯合并）：参数平铺 + `params` 别名 + `objects`（参数对象状态）。
/// objects 由壳层按参数声明装载（object → 单对象 JSON；objectSet → 对象 JSON 数组）。
/// FEEL 表达式因此可写 `objects.orderId.status == 'open'`（链式成员访问）。
pub fn build_validation_ctx(params: &Value, objects: &Value) -> Value {
    let mut ctx = serde_json::Map::new();
    if let Some(m) = params.as_object() {
        for (k, v) in m {
            ctx.insert(k.clone(), v.clone());
        }
    }
    ctx.insert("params".to_string(), params.clone());
    let objs = match objects {
        Value::Object(_) => objects.clone(),
        _ => Value::Object(Map::new()),
    };
    ctx.insert("objects".to_string(), objs);
    Value::Object(ctx)
}

/// 一条提交校验失败（O4-M2）。
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationFailure {
    /// 失败的 FEEL 表达式。
    pub expression: String,
    /// 面向用户的错误提示（缺省回退表达式）。
    pub message: String,
}

/// 跑动作的提交校验（`validations` 每条 `{expression, message}`）：对 `ctx` 求值 FEEL 谓词。
///
/// `ctx` = 参数平铺 + `params` 别名 + `objects.*`（壳层装载的对象状态，见 [`build_validation_ctx`]）。
/// fail-closed：表达式非布尔真或求值出错都视为不通过，返回全部失败项（供前端一次展示）。
pub fn run_validations(action: &ActionTypeDef, ctx: &Value) -> Vec<ValidationFailure> {
    let mut fails = Vec::new();
    let Some(vs) = action.validations.as_array() else {
        return fails;
    };
    for v in vs {
        let expr = v.get("expression").and_then(|x| x.as_str()).unwrap_or("").trim();
        if expr.is_empty() {
            continue;
        }
        let message = v
            .get("message")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(expr)
            .to_string();
        let passed = matches!(eval_expression(expr, ctx), Ok(Value::Bool(true)));
        if !passed {
            fails.push(ValidationFailure { expression: expr.to_string(), message });
        }
    }
    fails
}

/// 一条已解析的副作用（O4-M3）：动作提交后经 Outbox 投递（触发流程/webhook/通知/事件/函数）。
#[derive(Debug, Clone, PartialEq)]
pub struct SideEffect {
    /// 类型：notification / webhook / callFunction / startBusinessProcess / emitEvent。
    pub kind: String,
    /// 目标引用（流程键 / 函数 apiName / URL / 事件主题 / 模板），已做参数替换。
    pub target: String,
    /// 载荷（参数替换后的完整对象；投递时透传）。
    pub payload: Value,
}

/// 副作用 kind → 目标字段名（前端保存时按 kind 存不同键）。
fn side_effect_target_key(kind: &str) -> &'static str {
    match kind {
        "startBusinessProcess" => "flowDefKey",
        "computeReport" => "reportCode",
        "callFunction" => "function",
        "webhook" => "url",
        "emitEvent" => "topic",
        _ => "template", // notification 及未知
    }
}

/// 字符串内 `$name` 插值（副作用 target 用；如 `approve_$orderId` → `approve_O-1`）。
/// 与 `subst`（整串 `"$name"` 替换，用于 logic）互补：target 常需拼接故支持内插。
fn interpolate(s: &str, params: &Value) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() && (chars[i + 1].is_alphabetic() || chars[i + 1] == '_') {
            let start = i + 1;
            let mut j = start;
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            let name: String = chars[start..j].iter().collect();
            match params.get(&name).and_then(scalar_to_string) {
                Some(v) => out.push_str(&v),
                None => {
                    out.push('$');
                    out.push_str(&name);
                }
            }
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// 解析动作 `side_effects` + 参数 → 副作用列表（参数替换；纯逻辑）。
///
/// 每条形如 `{kind, <targetKey>: "...", ...}`；target 支持 `$name` 内插（拼接场景）；
/// payload 走整串 `subst`。未知/空 target 的项跳过（宽容，避免脏配置阻断执行）。
pub fn resolve_side_effects(action: &ActionTypeDef, params: &Value) -> Vec<SideEffect> {
    let mut out = Vec::new();
    let Some(arr) = action.side_effects.as_array() else {
        return out;
    };
    for raw in arr {
        let sub = subst(raw, params);
        let kind = sub.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if kind.is_empty() {
            continue;
        }
        let key = side_effect_target_key(&kind);
        let raw_target = raw
            .get(key)
            .and_then(|v| v.as_str())
            .or_else(|| raw.get("target").and_then(|v| v.as_str()))
            .unwrap_or("");
        let target = interpolate(raw_target, params);
        if target.is_empty() {
            continue;
        }
        out.push(SideEffect { kind, target, payload: sub });
    }
    out
}

/// 解析动作 `logic` + 参数/对象状态/执行上下文 → 编辑操作列表（纯逻辑）。
///
/// - `params`：表单参数（`$name` 替换与 `src:"param"` 来源）。
/// - `objects`：参数对象状态（`{"<paramName>": 对象JSON 或 对象JSON数组}`；壳层装载，
///   `src:"paramProperty"` 取属性用）。
/// - `actor` / `now`：`currentUser` / `currentTime` 来源的取值（壳层传入，内核零时钟）。
pub fn resolve_edits(
    action: &ActionTypeDef,
    params: &Value,
    objects: &Value,
    actor: Option<&str>,
    now: Option<&str>,
) -> Result<Vec<ObjectEdit>, String> {
    let ops = match action.logic.as_array() {
        Some(a) => a,
        None => return Ok(vec![]),
    };
    let mut edits = Vec::with_capacity(ops.len());
    for raw in ops {
        // 先做整串 $name 替换（pk/对象标识常用），再递归解析映射来源。
        let sub = subst(raw, params);
        let op = resolve_value(&sub, params, objects, actor, now)?;
        let mut edit = parse_op(&op)?;
        // createOrModifyObject：title 缺省回退 pk（与 createObject 一致的展示语义）
        if let ObjectEdit::UpsertObject { pk, title, .. } = &mut edit
            && title.is_empty() {
                *title = pk.clone();
            }
        edits.push(edit);
    }
    Ok(edits)
}

/// 函数背书返回值解析（Run function rule 最小闭环）：`{"edits":[...], "sideEffects":[...]}`；
/// 顶层返回**数组**视为 edits（兼容简写）。edits 条目与 logic 同构（op/objectType/pk/set…）。
pub fn parse_function_result(v: &Value) -> Result<(Vec<ObjectEdit>, Vec<SideEffect>), String> {
    let (edits_v, fx_v) = match v {
        Value::Array(_) => (v, &Value::Null),
        Value::Object(m) => (m.get("edits").unwrap_or(&Value::Null), m.get("sideEffects").unwrap_or(&Value::Null)),
        _ => (&Value::Null, &Value::Null),
    };
    let mut edits = Vec::new();
    if let Some(arr) = edits_v.as_array() {
        for op in arr {
            edits.push(parse_op(op)?);
        }
    } else if !edits_v.is_null() {
        return Err("函数返回 edits 须为数组".into());
    }
    let mut effects = Vec::new();
    if let Some(arr) = fx_v.as_array() {
        for raw in arr {
            let kind = raw.get("kind").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if kind.is_empty() {
                continue;
            }
            let key = side_effect_target_key(&kind);
            let target = raw
                .get(key)
                .and_then(|x| x.as_str())
                .or_else(|| raw.get("target").and_then(|x| x.as_str()))
                .unwrap_or("")
                .to_string();
            if target.is_empty() {
                continue;
            }
            effects.push(SideEffect { kind, target, payload: raw.clone() });
        }
    }
    Ok((edits, effects))
}

/// 组合序列静态校验（对标 Palantir invalid combinations；upsert 归一化：序列中**首个**
/// upsert 视为 create、后续 upsert 视为 modify——静态可判定，不依赖运行期存在性）。
///
/// 四条规则（同一 (objectType, pk)）：
/// 1. delete 不得先于 create / modify / upsert；
/// 2. modify 不得先于 create / upsert；
/// 3. create 不得出现两次；upsert 之后不得再 create；
/// 4. upsert 之后不得 delete（保守：合法性依赖运行期存在性，静态不可判定）。
pub fn validate_edit_sequence(edits: &[ObjectEdit]) -> Result<(), String> {
    use std::collections::HashMap;
    #[derive(Clone, Copy, PartialEq)]
    enum St {
        Created,
        Upserted,
        Modified,
        Deleted,
    }
    let mut state: HashMap<(&str, &str), St> = HashMap::new();
    for (i, e) in edits.iter().enumerate() {
        let Some((t, pk)) = e.object_key() else { continue };
        let cur = state.get(&(t, pk)).copied();
        let next = match (e, cur) {
            // 链接类编辑不参与对象状态机（前面已 continue 跳过；此处兜底不可达）
            (ObjectEdit::AddLink { .. }, _) | (ObjectEdit::RemoveLink { .. }, _) => None,
            // —— None ——
            (ObjectEdit::CreateObject { .. }, None) => Some(St::Created),
            (ObjectEdit::UpsertObject { .. }, None) => Some(St::Upserted),
            (ObjectEdit::ModifyObject { .. }, None) => Some(St::Modified),
            (ObjectEdit::DeleteObject { .. }, None) => Some(St::Deleted),
            // —— from Created ——
            (ObjectEdit::CreateObject { .. }, Some(St::Created)) => {
                return Err(format!("第 {} 条规则：对象 {t}#{pk} 在本次提交中被创建两次", i + 1))
            }
            (ObjectEdit::UpsertObject { .. }, Some(St::Created)) => {
                return Err(format!(
                    "第 {} 条规则：对象 {t}#{pk} 创建后不能再 upsert（同 create 两次）",
                    i + 1
                ))
            }
            (ObjectEdit::ModifyObject { .. }, Some(St::Created)) => Some(St::Modified),
            (ObjectEdit::DeleteObject { .. }, Some(St::Created)) => Some(St::Deleted),
            // —— from Upserted ——
            (ObjectEdit::CreateObject { .. }, Some(St::Upserted)) => {
                return Err(format!("第 {} 条规则：对象 {t}#{pk} upsert 后不能再 create", i + 1))
            }
            (ObjectEdit::UpsertObject { .. }, Some(St::Upserted)) => Some(St::Upserted), // 归一为 modify
            (ObjectEdit::ModifyObject { .. }, Some(St::Upserted)) => Some(St::Upserted),
            (ObjectEdit::DeleteObject { .. }, Some(St::Upserted)) => {
                return Err(format!(
                    "第 {} 条规则：对象 {t}#{pk} upsert 后不得 delete（存在性依赖运行期，保守拒绝）",
                    i + 1
                ))
            }
            // —— from Modified ——
            (ObjectEdit::CreateObject { .. }, Some(St::Modified)) | (ObjectEdit::UpsertObject { .. }, Some(St::Modified)) => {
                return Err(format!("第 {} 条规则：对象 {t}#{pk} 不得在 modify 之后 create/upsert", i + 1))
            }
            (ObjectEdit::ModifyObject { .. }, Some(St::Modified)) => Some(St::Modified),
            (ObjectEdit::DeleteObject { .. }, Some(St::Modified)) => Some(St::Deleted),
            // —— from Deleted ——
            (ObjectEdit::DeleteObject { .. }, Some(St::Deleted)) => Some(St::Deleted),
            (_, Some(St::Deleted)) => {
                return Err(format!("第 {} 条规则：对象 {t}#{pk} 不得在 delete 之后 create/modify/upsert", i + 1))
            }
        };
        if let Some(s) = next {
            state.insert((t, pk), s);
        }
    }
    Ok(())
}

/// 从编辑列表提取涉及的对象类型（PEP 作用域；内核导出——与执行期/预检同源口径）。
/// 含 UpsertObject；链接类编辑不入作用域（既有行为，向后兼容）。
pub fn edit_object_types(edits: &[ObjectEdit]) -> Vec<String> {
    let mut out = Vec::new();
    for e in edits {
        let ot = match e {
            ObjectEdit::CreateObject { object_type, .. }
            | ObjectEdit::UpsertObject { object_type, .. }
            | ObjectEdit::ModifyObject { object_type, .. }
            | ObjectEdit::DeleteObject { object_type, .. } => Some(object_type.clone()),
            _ => None,
        };
        if let Some(t) = ot
            && !out.contains(&t) {
                out.push(t);
            }
    }
    out
}

/// 扫描 JSON 树中全部 `$ident` 参数引用（logic/side_effects 派生用；整串与内插两种形态都算）。
fn collect_param_refs(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            let b: Vec<char> = s.chars().collect();
            let mut i = 0;
            while i < b.len() {
                if b[i] == '$' && i + 1 < b.len() && (b[i + 1].is_alphabetic() || b[i + 1] == '_') {
                    let start = i + 1;
                    let mut j = start;
                    while j < b.len() && (b[j].is_alphanumeric() || b[j] == '_') {
                        j += 1;
                    }
                    let name: String = b[start..j].iter().collect();
                    if !out.contains(&name) {
                        out.push(name);
                    }
                    i = j;
                } else {
                    i += 1;
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_param_refs(x, out)),
        Value::Object(m) => {
            for (k, x) in m {
                if k == "src" {
                    continue; // 映射来源键不是参数引用
                }
                collect_param_refs(x, out);
            }
        }
        _ => {}
    }
}

/// 派生动作的**作用对象类型**（P2-0 物化列 `target_object_types` 的计算真源；对齐
/// Palantir"作用对象由参数类型声明"——本派生只为清单查询效率，不引入新语义）：
/// ① `parameters` 中 `type == object/objectSet` 且 `objectType` 非空的声明；
/// ② `logic` 中对象类编辑（create/createOrModify/modify/delete）的 `objectType`。
/// 去重保序；链接类编辑不产生对象类型。函数背书动作以其显式声明的 parameters 为准。
pub fn derive_target_object_types(parameters: &Value, logic: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |s: &str, out: &mut Vec<String>| {
        if !s.is_empty() && !out.iter().any(|x| x == s) {
            out.push(s.to_string());
        }
    };
    if let Some(ps) = parameters.as_array() {
        for p in ps {
            let ty = p.get("type").and_then(Value::as_str).unwrap_or("");
            if ty != "object" && ty != "objectSet" {
                continue;
            }
            if let Some(ot) = p.get("objectType").and_then(Value::as_str) {
                push(ot, &mut out);
            }
        }
    }
    if let Some(ops) = logic.as_array() {
        for op in ops {
            let kind = op.get("op").and_then(Value::as_str).unwrap_or("");
            if matches!(
                kind,
                "createObject" | "createOrModifyObject" | "modifyObject" | "deleteObject"
            ) {
                if let Some(ot) = op.get("objectType").and_then(Value::as_str) {
                    push(ot, &mut out);
                }
            }
        }
    }
    out
}

/// 保存期派生缺失参数（P1-2，四类来源并集；只增不删）：① logic 的 `$name` 引用；
/// ② logic 的 `src:"param"/"paramProperty"` 显式引用；③ side_effects 的 `$name` 引用。
/// （④ 函数入参派生需 FunctionDef，由壳层 handler 补充。）
/// 返回新增的参数名列表。
pub fn derive_missing_params(def: &mut ActionTypeDef) -> Vec<String> {
    let mut refs: Vec<String> = Vec::new();
    if def.logic.is_object() || def.logic.is_array() {
        collect_param_refs(&def.logic, &mut refs);
    }
    // 显式 src 引用
    if let Some(ops) = def.logic.as_array() {
        for op in ops {
            if let Some(set) = op.get("set").or_else(|| op.get("properties"))
                && let Some(fields) = set.as_object() {
                    for v in fields.values() {
                        if let Some(src) = v.get("src").and_then(Value::as_str) {
                            if src == "param"
                                && let Some(n) = v.get("name").and_then(Value::as_str)
                                    && !refs.iter().any(|r| r == n) {
                                        refs.push(n.to_string());
                                    }
                            if src == "paramProperty"
                                && let Some(n) = v.get("param").and_then(Value::as_str)
                                    && !refs.iter().any(|r| r == n) {
                                        refs.push(n.to_string());
                                    }
                        }
                    }
                }
        }
    }
    if def.side_effects.is_object() || def.side_effects.is_array() {
        collect_param_refs(&def.side_effects, &mut refs);
    }
    let mut added = Vec::new();
    if !refs.is_empty() {
        let mut ps = def.parameters.as_array().cloned().unwrap_or_default();
        for name in refs {
            let exists = ps
                .iter()
                .any(|p| p.get("name").and_then(Value::as_str) == Some(name.as_str()));
            if !exists {
                ps.push(serde_json::json!({ "name": name, "required": false, "type": "string" }));
                added.push(name);
            }
        }
        if !added.is_empty() {
            def.parameters = Value::Array(ps);
        }
    }
    added
}

/// 保存期结构校验（shell handler 调用；在 `def.validate()` 之后）：
/// - function_backing 非空 ⇒ logic、side_effects 必须为空（对齐 Palantir Run function
///   rule 不能与其他规则组合），validations 允许保留（提交校验与规则正交）；
/// - defaultValue 必须满足 multipleChoice 约束。
pub fn save_validate_action(def: &ActionTypeDef) -> Result<(), String> {
    if def.function_backing.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false) {
        let has_logic = def.logic.as_array().map(|a| !a.is_empty()).unwrap_or(false);
        let has_fx = def.side_effects.as_array().map(|a| !a.is_empty()).unwrap_or(false);
        if has_logic || has_fx {
            return Err(
                "函数背书动作不能同时配置编辑规则或副作用（Run function 规则不与其他规则组合；复杂逻辑请写入函数体）"
                    .into(),
            );
        }
    }
    if let Some(ps) = def.parameters.as_array() {
        for p in ps {
            let name = p.get("name").and_then(Value::as_str).unwrap_or("");
            let Some(dv) = p.get("defaultValue").filter(|v| !v.is_null()) else { continue };
            if let Some(opts) = p
                .get("constraints")
                .filter(|c| c.get("kind").and_then(|v| v.as_str()) == Some("multipleChoice"))
                .and_then(|c| c.get("options"))
                .and_then(Value::as_array)
            {
                let sv = dv.as_str().map(|s| s.to_string()).unwrap_or_else(|| dv.to_string());
                let hit = opts
                    .iter()
                    .any(|o| o.as_str().map(|s| s == sv).unwrap_or_else(|| *o == sv));
                if !hit {
                    return Err(format!("参数「{name}」的 defaultValue 不在 multipleChoice 可选范围内"));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn action(logic: Value, parameters: Value) -> ActionTypeDef {
        ActionTypeDef {
            api_name: "reassignOrder".into(),
            logic,
            parameters,
            ..Default::default()
        }
    }

    const OBJS: &str = r#"{"orderId":{"pk":"O-1","status":"open","customerId":"C-7"}}"#;

    #[test]
    fn resolve_substitutes_params_and_builds_edits() {
        let a = action(
            json!([
                { "op": "modifyObject", "objectType": "Order", "pk": "$orderId", "set": { "owner": "$newOwner" } },
                { "op": "addLink", "link": "handledBy", "aPk": "$orderId", "bPk": "$newOwner" }
            ]),
            json!([{ "name": "orderId", "required": true }, { "name": "newOwner", "required": true }]),
        );
        let params = json!({ "orderId": "O-1", "newOwner": "U-9" });
        let edits = resolve_edits(&a, &params, &json!({}), Some("u1"), None).unwrap();
        assert_eq!(edits.len(), 2);
        assert_eq!(
            edits[0],
            ObjectEdit::ModifyObject {
                object_type: "Order".into(),
                pk: "O-1".into(),
                set: json!({ "owner": "U-9" }),
            }
        );
        assert_eq!(
            edits[1],
            ObjectEdit::AddLink { link: "handledBy".into(), a_pk: "O-1".into(), b_pk: "U-9".into(), properties: json!({}) }
        );
    }

    #[test]
    fn create_object_defaults_title_to_pk() {
        let a = action(
            json!([{ "op": "createObject", "objectType": "Order", "pk": "$id", "properties": { "id": "$id" } }]),
            json!([]),
        );
        let edits = resolve_edits(&a, &json!({ "id": "O-7" }), &json!({}), None, None).unwrap();
        match &edits[0] {
            ObjectEdit::CreateObject { pk, title, properties, .. } => {
                assert_eq!(pk, "O-7");
                assert_eq!(title, "O-7");
                assert_eq!(properties, &json!({ "id": "O-7" }));
            }
            _ => panic!("expected CreateObject"),
        }
    }

    #[test]
    fn numeric_param_coerced_to_pk_string() {
        let a = action(json!([{ "op": "deleteObject", "objectType": "Order", "pk": "$id" }]), json!([]));
        let edits = resolve_edits(&a, &json!({ "id": 42 }), &json!({}), None, None).unwrap();
        assert_eq!(edits[0], ObjectEdit::DeleteObject { object_type: "Order".into(), pk: "42".into() });
    }

    #[test]
    fn validate_params_requires_required() {
        let a = action(json!([]), json!([{ "name": "orderId", "required": true }]));
        assert!(validate_params(&a, &json!({})).is_err());
        assert!(validate_params(&a, &json!({ "orderId": "X" })).is_ok());
    }

    #[test]
    fn validate_params_multiple_choice_constraint() {
        let a = action(
            json!([]),
            json!([{ "name": "priority", "constraints": { "kind": "multipleChoice", "options": ["P0", "P1"] } }]),
        );
        assert!(validate_params(&a, &json!({ "priority": "P1" })).is_ok());
        assert!(validate_params(&a, &json!({ "priority": "P9" })).is_err());
        // 未提供 → 跳过约束（required=false）
        assert!(validate_params(&a, &json!({})).is_ok());
    }

    #[test]
    fn apply_param_defaults_fills_missing() {
        let a = action(
            json!([]),
            json!([{ "name": "p", "defaultValue": "P1" }, { "name": "q", "defaultValue": "X" }]),
        );
        let mut params = json!({ "q": "given" });
        apply_param_defaults(&a, &mut params);
        assert_eq!(params, json!({ "p": "P1", "q": "given" }));
    }

    #[test]
    fn unknown_op_errors() {
        let a = action(json!([{ "op": "frobnicate" }]), json!([]));
        assert!(resolve_edits(&a, &json!({}), &json!({}), None, None).is_err());
    }

    #[test]
    fn validations_pass_and_fail() {
        let mut a = action(json!([]), json!([]));
        a.validations = json!([
            { "expression": "amount > 0", "message": "金额须为正" },
            { "expression": "status in ['open','pending']", "message": "状态非法" }
        ]);
        let ctx = build_validation_ctx(&json!({ "amount": 10, "status": "open" }), &json!({}));
        assert!(run_validations(&a, &ctx).is_empty());
        let f = run_validations(&a, &build_validation_ctx(&json!({ "amount": -1, "status": "open" }), &json!({})));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].message, "金额须为正");
        assert_eq!(
            run_validations(&a, &build_validation_ctx(&json!({ "amount": 0, "status": "closed" }), &json!({}))).len(),
            2
        );
    }

    #[test]
    fn validation_non_boolean_fails_closed() {
        let mut a = action(json!([]), json!([]));
        a.validations = json!([{ "expression": "amount", "message": "must be bool" }]);
        assert_eq!(
            run_validations(&a, &build_validation_ctx(&json!({ "amount": 5 }), &json!({}))).len(),
            1
        );
    }

    #[test]
    fn validation_reads_object_state() {
        // P0-2：校验表达式引用对象引用参数的当前属性（Palantir Parameter condition 对标）
        let mut a = action(json!([]), json!([]));
        a.validations = json!([{ "expression": "objects.orderId.status == 'open'", "message": "仅 open 可提交" }]);
        let objs = serde_json::from_str::<Value>(OBJS).unwrap();
        let ctx = build_validation_ctx(&json!({ "orderId": "O-1" }), &objs);
        assert!(run_validations(&a, &ctx).is_empty());
        let mut closed = objs.clone();
        closed["orderId"]["status"] = json!("closed");
        assert_eq!(run_validations(&a, &build_validation_ctx(&json!({ "orderId": "O-1" }), &closed)).len(), 1);
    }

    #[test]
    fn resolve_side_effects_substitutes_and_maps_target() {
        let mut a = action(json!([]), json!([]));
        a.side_effects = json!([
            { "kind": "startBusinessProcess", "flowDefKey": "approve_$orderId" },
            { "kind": "webhook", "url": "https://hook/$newOwner" },
            { "kind": "notification", "template": "reassigned" },
            { "kind": "emitEvent" }  // 无 target → 跳过
        ]);
        let fx = resolve_side_effects(&a, &json!({ "orderId": "O-1", "newOwner": "U-9" }));
        assert_eq!(fx.len(), 3);
        assert_eq!(fx[0].kind, "startBusinessProcess");
        assert_eq!(fx[0].target, "approve_O-1");
        assert_eq!(fx[1].kind, "webhook");
        assert_eq!(fx[1].target, "https://hook/U-9");
        assert_eq!(fx[2].kind, "notification");
        assert_eq!(fx[2].target, "reassigned");
    }

    // ───────────── P0-3 值映射五来源 ─────────────

    #[test]
    fn value_sources_five_kinds() {
        let objs = serde_json::from_str::<Value>(OBJS).unwrap();
        let a = action(
            json!([{ "op": "modifyObject", "objectType": "Order", "pk": "$orderId", "set": {
                "owner":    { "src": "param", "name": "newOwner" },
                "memo":     { "src": "static", "value": "年度结转" },
                "closedBy": { "src": "currentUser" },
                "closedAt": { "src": "currentTime" },
                "customer": { "src": "paramProperty", "param": "orderId", "property": "customerId" }
            } }]),
            json!([{ "name": "orderId", "required": true }, { "name": "newOwner", "required": true }]),
        );
        let params = json!({ "orderId": "O-1", "newOwner": "U-9" });
        let edits = resolve_edits(&a, &params, &objs, Some("boss"), Some("2026-09-12T00:00:00Z")).unwrap();
        assert_eq!(
            edits[0],
            ObjectEdit::ModifyObject {
                object_type: "Order".into(),
                pk: "O-1".into(),
                set: json!({
                    "owner": "U-9", "memo": "年度结转", "closedBy": "boss",
                    "closedAt": "2026-09-12T00:00:00Z", "customer": "C-7"
                }),
            }
        );
    }

    #[test]
    fn value_sources_nested_and_reserved_word() {
        // 嵌套对象/数组递归；确需写 {"src":..} 形态的静态对象须显式包裹
        let a = action(
            json!([{ "op": "modifyObject", "objectType": "Order", "pk": "P-1", "set": {
                "meta": { "tags": [{ "src": "param", "name": "t" }] },
                "body": { "src": "static", "value": { "src": "looks-like-source" } }
            } }]),
            json!([]),
        );
        let edits = resolve_edits(&a, &json!({ "t": "hot" }), &json!({}), None, None).unwrap();
        assert_eq!(
            edits[0],
            ObjectEdit::ModifyObject {
                object_type: "Order".into(),
                pk: "P-1".into(),
                set: json!({ "meta": { "tags": ["hot"] }, "body": { "src": "looks-like-source" } }),
            }
        );
    }

    #[test]
    fn value_source_errors_fail_closed() {
        let a = action(
            json!([{ "op": "modifyObject", "objectType": "Order", "pk": "P-1", "set": {
                "customer": { "src": "paramProperty", "param": "orderId", "property": "nope" } } }]),
            json!([]),
        );
        let objs = serde_json::from_str::<Value>(OBJS).unwrap();
        // 属性缺失 → 报错
        assert!(resolve_edits(&a, &json!({}), &objs, None, None).is_err());
        // 对象未装载 → 报错
        assert!(resolve_edits(&a, &json!({}), &json!({}), None, None).is_err());
        // currentUser 无上下文 → 报错
        let b = action(
            json!([{ "op": "modifyObject", "objectType": "Order", "pk": "P-1", "set": { "u": { "src": "currentUser" } } }]),
            json!([]),
        );
        assert!(resolve_edits(&b, &json!({}), &json!({}), None, None).is_err());
    }

    // ───────────── P0-5 upsert ─────────────

    #[test]
    fn upsert_parses_with_title_fallback() {
        let a = action(
            json!([{ "op": "createOrModifyObject", "objectType": "Order", "pk": "$id", "set": { "status": "closed" } }]),
            json!([]),
        );
        let edits = resolve_edits(&a, &json!({ "id": "O-9" }), &json!({}), None, None).unwrap();
        assert_eq!(
            edits[0],
            ObjectEdit::UpsertObject {
                object_type: "Order".into(),
                pk: "O-9".into(),
                title: "O-9".into(),
                set: json!({ "status": "closed" }),
            }
        );
    }

    #[test]
    fn edit_sequence_four_rules() {
        let mk = |op: &str| -> ObjectEdit {
            parse_op(&json!({ "op": op, "objectType": "Order", "pk": "K" })).unwrap()
        };
        // 规则 2：modify → upsert 拒
        assert!(validate_edit_sequence(&[mk("modifyObject"), mk("createOrModifyObject")]).is_err());
        // 规则 3：create → create 拒；upsert → create 拒
        assert!(validate_edit_sequence(&[mk("createObject"), mk("createObject")]).is_err());
        assert!(validate_edit_sequence(&[mk("createOrModifyObject"), mk("createObject")]).is_err());
        // 规则 4：upsert → delete 拒
        assert!(validate_edit_sequence(&[mk("createOrModifyObject"), mk("deleteObject")]).is_err());
        // 规则 1：delete → create/modify/upsert 拒
        assert!(validate_edit_sequence(&[mk("deleteObject"), mk("modifyObject")]).is_err());
        assert!(validate_edit_sequence(&[mk("deleteObject"), mk("createOrModifyObject")]).is_err());
        // 合法序列：create → modify → delete（Palantir 允许）；upsert → upsert；modify → modify
        assert!(validate_edit_sequence(&[mk("createObject"), mk("modifyObject"), mk("deleteObject")]).is_ok());
        assert!(validate_edit_sequence(&[mk("createOrModifyObject"), mk("createOrModifyObject")]).is_ok());
        assert!(validate_edit_sequence(&[mk("modifyObject"), mk("modifyObject")]).is_ok());
        // 不同对象互不影响；链接编辑不参与
        assert!(validate_edit_sequence(&[
            parse_op(&json!({ "op": "createObject", "objectType": "A", "pk": "1" })).unwrap(),
            parse_op(&json!({ "op": "createObject", "objectType": "B", "pk": "1" })).unwrap(),
            parse_op(&json!({ "op": "addLink", "link": "L", "aPk": "1", "bPk": "2" })).unwrap(),
        ])
        .is_ok());
    }

    #[test]
    fn edit_object_types_includes_upsert_and_dedups() {
        let edits = vec![
            parse_op(&json!({ "op": "createOrModifyObject", "objectType": "Order", "pk": "1" })).unwrap(),
            parse_op(&json!({ "op": "modifyObject", "objectType": "Order", "pk": "2" })).unwrap(),
            parse_op(&json!({ "op": "addLink", "link": "L", "aPk": "1", "bPk": "2" })).unwrap(),
        ];
        assert_eq!(edit_object_types(&edits), vec!["Order".to_string()]);
    }

    // ───────────── P0-1 函数返回解析 ─────────────

    #[test]
    fn parse_function_result_object_form() {
        let v = json!({
            "edits": [{ "op": "modifyObject", "objectType": "Order", "pk": "O-1", "set": { "status": "closed" } }],
            "sideEffects": [{ "kind": "notification", "template": "closed" }]
        });
        let (edits, fx) = parse_function_result(&v).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(fx.len(), 1);
        assert_eq!(fx[0].target, "closed");
    }

    #[test]
    fn parse_function_result_array_shorthand() {
        let (edits, fx) = parse_function_result(&json!([
            { "op": "createObject", "objectType": "Order", "pk": "O-2", "properties": { "id": "O-2" } }
        ]))
        .unwrap();
        assert_eq!(edits.len(), 1);
        assert!(fx.is_empty());
    }

    #[test]
    fn parse_function_result_rejects_bad_op() {
        assert!(parse_function_result(&json!([{ "op": "nope" }])).is_err());
        assert!(parse_function_result(&json!({ "edits": "not-array" })).is_err());
    }

    // ───────────── P1-2 派生与保存校验 ─────────────

    #[test]
    fn derive_missing_params_from_logic_and_side_effects() {
        let mut a = action(
            json!([
                { "op": "modifyObject", "objectType": "Order", "pk": "$orderId",
                  "set": { "owner": { "src": "param", "name": "newOwner" },
                           "customer": { "src": "paramProperty", "param": "orderId", "property": "cid" } } }
            ]),
            json!([]),
        );
        a.side_effects = json!([{ "kind": "startBusinessProcess", "flowDefKey": "approve_$orderId" }]);
        let added = derive_missing_params(&mut a);
        assert_eq!(added, vec!["orderId".to_string(), "newOwner".to_string()]);
        let names: Vec<&str> =
            a.parameters.as_array().unwrap().iter().map(|p| p.get("name").unwrap().as_str().unwrap()).collect();
        assert_eq!(names, vec!["orderId", "newOwner"]);
    }

    #[test]
    fn derive_no_duplicates_for_declared() {
        let mut a = action(
            json!([{ "op": "deleteObject", "objectType": "Order", "pk": "$id" }]),
            json!([{ "name": "id", "required": true }]),
        );
        assert!(derive_missing_params(&mut a).is_empty());
    }

    #[test]
    fn derive_target_object_types_from_params_and_logic() {
        // 参数声明 + logic 目标取并集去重；标量参数与链接编辑不计入
        let parameters = json!([
            { "name": "orderId", "type": "object", "objectType": "Order" },
            { "name": "ids", "type": "objectSet", "objectType": "Order" },
            { "name": "memo", "type": "string" },
            { "name": "noType", "type": "object" }
        ]);
        let logic = json!([
            { "op": "modifyObject", "objectType": "Order", "pk": "K" },
            { "op": "addLink", "link": "L", "aPk": "1", "bPk": "2" },
            { "op": "createOrModifyObject", "objectType": "Ticket", "pk": "K" }
        ]);
        assert_eq!(
            derive_target_object_types(&parameters, &logic),
            vec!["Order".to_string(), "Ticket".to_string()]
        );
        // 空定义 → 空
        assert!(derive_target_object_types(&json!([]), &json!([])).is_empty());
    }

    #[test]
    fn save_validate_function_backing_mutex() {
        let mut a = action(json!([{ "op": "modifyObject", "objectType": "Order", "pk": "K" }]), json!([]));
        a.function_backing = Some("someFn".into());
        assert!(save_validate_action(&a).is_err());
        a.logic = json!([]);
        a.side_effects = json!([{ "kind": "notification", "template": "t" }]);
        assert!(save_validate_action(&a).is_err());
        a.side_effects = json!([]);
        assert!(save_validate_action(&a).is_ok());
        // 无函数背书时 logic 正常
        let b = action(json!([{ "op": "modifyObject", "objectType": "Order", "pk": "K" }]), json!([]));
        assert!(save_validate_action(&b).is_ok());
    }

    #[test]
    fn save_validate_default_value_constraint() {
        let a = action(
            json!([]),
            json!([{ "name": "p", "defaultValue": "P9",
                     "constraints": { "kind": "multipleChoice", "options": ["P0", "P1"] } }]),
        );
        assert!(save_validate_action(&a).is_err());
        let b = action(
            json!([]),
            json!([{ "name": "p", "defaultValue": "P0",
                     "constraints": { "kind": "multipleChoice", "options": ["P0", "P1"] } }]),
        );
        assert!(save_validate_action(&b).is_ok());
    }
}
