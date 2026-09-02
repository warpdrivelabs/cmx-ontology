//! O4 动作引擎 · 内核（纯逻辑、零 IO、可单测）。
//!
//! 一个 [`ActionTypeDef`](crate::ActionTypeDef) 的 `logic` 是一串**编辑操作**（对象/关系的增删改）。
//! 执行动作 = 用调用方传入的参数 `params` 解析 `logic` → [`ObjectEdit`] 列表 → 交存储层原子写回。
//!
//! 参数替换：`logic` 里任意等于 `"$name"` 的字符串替换为 `params.name`（递归 properties/set 等）。
//! 判定/校验（接规则引擎 FEEL）与副作用（接流程）属 O4-M2/M3，此处仅结构化校验必填参数。

use crate::def::ActionTypeDef;
use crate::feel::eval_expression;
use serde_json::{Map, Value};

/// 单条编辑操作（动作写回的原子单元）。
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectEdit {
    /// upsert 一个对象（pk 冲突则覆盖）。
    CreateObject {
        object_type: String,
        pk: String,
        title: String,
        properties: Value,
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

/// 标量 JSON → 字符串（pk/aPk/bPk 用）。
fn scalar_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// 递归参数替换：字符串 `"$name"` → `params.name`；数组/对象递归。
fn subst(v: &Value, params: &Value) -> Value {
    match v {
        Value::String(s) => {
            if let Some(name) = s.strip_prefix('$') {
                if let Some(pv) = params.get(name) {
                    return pv.clone();
                }
            }
            v.clone()
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| subst(x, params)).collect()),
        Value::Object(m) => {
            let mut out = Map::new();
            for (k, x) in m {
                out.insert(k.clone(), subst(x, params));
            }
            Value::Object(out)
        }
        _ => v.clone(),
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

/// 结构化校验必填参数（`parameters` 里 `required=true` 的须在 `params` 中非空）。
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
        }
    }
    Ok(())
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
/// `ctx` = 参数 + 便捷别名（如 `{"params": {...}, ...展开的参数}`）。fail-closed：表达式非布尔真
/// 或求值出错都视为不通过，返回全部失败项（供前端一次展示）。空 validations → 通过。
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

/// 解析动作 `logic` + 参数 → 编辑操作列表（纯逻辑）。
pub fn resolve_edits(action: &ActionTypeDef, params: &Value) -> Result<Vec<ObjectEdit>, String> {
    let ops = match action.logic.as_array() {
        Some(a) => a,
        None => return Ok(vec![]),
    };
    let mut edits = Vec::with_capacity(ops.len());
    for raw in ops {
        let op = subst(raw, params);
        let kind = op.get("op").and_then(|v| v.as_str()).unwrap_or("");
        let edit = match kind {
            "createObject" => ObjectEdit::CreateObject {
                object_type: str_field(&op, "objectType")?,
                pk: scalar_field(&op, "pk")?,
                title: op
                    .get("title")
                    .and_then(scalar_to_string)
                    .unwrap_or_else(|| scalar_field(&op, "pk").unwrap_or_default()),
                properties: op.get("properties").cloned().unwrap_or(Value::Object(Map::new())),
            },
            "modifyObject" => ObjectEdit::ModifyObject {
                object_type: str_field(&op, "objectType")?,
                pk: scalar_field(&op, "pk")?,
                set: op.get("set").cloned().unwrap_or(Value::Object(Map::new())),
            },
            "deleteObject" => ObjectEdit::DeleteObject {
                object_type: str_field(&op, "objectType")?,
                pk: scalar_field(&op, "pk")?,
            },
            "addLink" => ObjectEdit::AddLink {
                link: str_field(&op, "link")?,
                a_pk: scalar_field(&op, "aPk")?,
                b_pk: scalar_field(&op, "bPk")?,
                properties: op.get("properties").cloned().unwrap_or(Value::Object(Map::new())),
            },
            "removeLink" => ObjectEdit::RemoveLink {
                link: str_field(&op, "link")?,
                a_pk: scalar_field(&op, "aPk")?,
                b_pk: scalar_field(&op, "bPk")?,
            },
            "" => return Err("编辑操作缺 op 字段".to_string()),
            other => return Err(format!("未知编辑操作「{other}」")),
        };
        edits.push(edit);
    }
    Ok(edits)
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
        let edits = resolve_edits(&a, &params).unwrap();
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
        let edits = resolve_edits(&a, &json!({ "id": "O-7" })).unwrap();
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
        let edits = resolve_edits(&a, &json!({ "id": 42 })).unwrap();
        assert_eq!(edits[0], ObjectEdit::DeleteObject { object_type: "Order".into(), pk: "42".into() });
    }

    #[test]
    fn validate_params_requires_required() {
        let a = action(json!([]), json!([{ "name": "orderId", "required": true }]));
        assert!(validate_params(&a, &json!({})).is_err());
        assert!(validate_params(&a, &json!({ "orderId": "X" })).is_ok());
    }

    #[test]
    fn unknown_op_errors() {
        let a = action(json!([{ "op": "frobnicate" }]), json!([]));
        assert!(resolve_edits(&a, &json!({})).is_err());
    }

    #[test]
    fn validations_pass_and_fail() {
        let mut a = action(json!([]), json!([]));
        a.validations = json!([
            { "expression": "amount > 0", "message": "金额须为正" },
            { "expression": "status in ['open','pending']", "message": "状态非法" }
        ]);
        // 全通过
        assert!(run_validations(&a, &json!({ "amount": 10, "status": "open" })).is_empty());
        // amount 违规 → 1 条失败，带 message
        let f = run_validations(&a, &json!({ "amount": -1, "status": "open" }));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].message, "金额须为正");
        // 两条都违规
        assert_eq!(run_validations(&a, &json!({ "amount": 0, "status": "closed" })).len(), 2);
    }

    #[test]
    fn validation_non_boolean_fails_closed() {
        let mut a = action(json!([]), json!([]));
        a.validations = json!([{ "expression": "amount", "message": "must be bool" }]);
        assert_eq!(run_validations(&a, &json!({ "amount": 5 })).len(), 1); // 非布尔 → 不通过
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
}
