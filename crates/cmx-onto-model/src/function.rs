//! O5 函数计算引擎 · 内核（纯逻辑、零 IO、可单测）。
//!
//! 一个 [`FunctionDef`](crate::FunctionDef) 把「输入参数（对象/对象集/标量）+ 函数体」求值成结果。
//! M1 只做 FEEL runtime（默认、最常用）：把已绑定的输入作为 FEEL 上下文求值 `body`。
//! `object`/`objectSet` 类型的输入由**壳层**先从存储加载好、以 JSON 注入 ctx，本模块只管求值（保持零 IO）。
//!
//! 支持用途（kind）：Query / DerivedProperty / Validation —— 皆表达式求值；
//! Aggregation 走存储层聚合（壳层分派，不在此）；Rhai/Wasm/NativeRust 运行时 M1 未实现。

use crate::def::{FunctionDef, FunctionRuntime};
use crate::feel::eval_expression;
use serde_json::{Map, Value};

/// 函数求值错误。
#[derive(Debug, thiserror::Error)]
pub enum FunctionError {
    #[error("缺输入参数「{0}」")]
    MissingInput(String),
    #[error("运行时 {0} 尚未支持（M1 仅 FEEL）")]
    UnsupportedRuntime(String),
    #[error("函数体为空")]
    EmptyBody,
    #[error("求值失败: {0}")]
    Eval(String),
}
type Result<T> = std::result::Result<T, FunctionError>;

/// 输入参数声明（从 `inputs` JSON 解析）。
#[derive(Debug, Clone)]
pub struct InputSpec {
    pub name: String,
    /// 类型：base 标量 / `object` / `objectSet`。
    pub ty: String,
}

/// 解析函数的 `inputs`（`[{name,type}]`）为声明列表。
pub fn input_specs(func: &FunctionDef) -> Vec<InputSpec> {
    let mut out = Vec::new();
    if let Some(arr) = func.inputs.as_array() {
        for it in arr {
            let name = it.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            let ty = it.get("type").and_then(|v| v.as_str()).unwrap_or("string").to_string();
            out.push(InputSpec { name, ty });
        }
    }
    out
}

/// 校验必填输入齐备（声明的每个 input 须在 `bound` 中存在，缺失即错）。
pub fn check_inputs(func: &FunctionDef, bound: &Value) -> Result<()> {
    for spec in input_specs(func) {
        let missing = bound.get(&spec.name).map(|v| v.is_null()).unwrap_or(true);
        if missing {
            return Err(FunctionError::MissingInput(spec.name));
        }
    }
    Ok(())
}

/// 用已绑定输入 `bound`（含标量 + 壳层注入的 object/objectSet JSON）求值函数体。
///
/// M1：仅 FEEL runtime。`bound` 的字段即 FEEL 顶层变量（`amount`、`order.total`、`orders[...]`）。
pub fn evaluate(func: &FunctionDef, bound: &Value) -> Result<Value> {
    match func.runtime {
        FunctionRuntime::Feel => {
            let body = func.body.trim();
            if body.is_empty() {
                return Err(FunctionError::EmptyBody);
            }
            check_inputs(func, bound)?;
            let ctx = ensure_object(bound);
            eval_expression(body, &ctx).map_err(|e| FunctionError::Eval(e.to_string()))
        }
        FunctionRuntime::Rhai => Err(FunctionError::UnsupportedRuntime("Rhai".into())),
        FunctionRuntime::Wasm => Err(FunctionError::UnsupportedRuntime("Wasm".into())),
        FunctionRuntime::NativeRust => Err(FunctionError::UnsupportedRuntime("NativeRust".into())),
    }
}

/// 非对象的 bound 包一层空对象（FEEL ctx 须对象）。
fn ensure_object(v: &Value) -> Value {
    match v {
        Value::Object(_) => v.clone(),
        _ => Value::Object(Map::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::FunctionKind;
    use serde_json::json;

    fn feel_fn(inputs: Value, body: &str) -> FunctionDef {
        FunctionDef {
            api_name: "discountRate".into(),
            runtime: FunctionRuntime::Feel,
            kind: FunctionKind::Query,
            inputs,
            body: body.into(),
            ..Default::default()
        }
    }

    #[test]
    fn evaluates_scalar_query() {
        let f = feel_fn(json!([{ "name": "amount", "type": "double" }]), "if amount > 1000 then 0.8 else 0.2");
        assert_eq!(evaluate(&f, &json!({ "amount": 1500 })).unwrap(), json!(0.8));
        assert_eq!(evaluate(&f, &json!({ "amount": 500 })).unwrap(), json!(0.2));
    }

    #[test]
    fn derived_property_over_object() {
        // object 类型输入（壳层注入的对象 JSON），读其属性
        let f = feel_fn(json!([{ "name": "order", "type": "object" }]), "order.qty * order.price");
        let ctx = json!({ "order": { "qty": 3, "price": 10 } });
        assert_eq!(evaluate(&f, &ctx).unwrap(), json!(30.0));
    }

    #[test]
    fn object_set_aggregate_via_feel_builtin() {
        // objectSet 输入（壳层注入的行数组），用 FEEL sum/count
        let f = feel_fn(json!([{ "name": "items", "type": "objectSet" }]), "sum(items)");
        assert_eq!(evaluate(&f, &json!({ "items": [1, 2, 3, 4] })).unwrap(), json!(10.0));
    }

    #[test]
    fn missing_input_errors() {
        let f = feel_fn(json!([{ "name": "amount", "type": "double" }]), "amount > 0");
        assert!(matches!(evaluate(&f, &json!({})), Err(FunctionError::MissingInput(_))));
    }

    #[test]
    fn empty_body_and_unsupported_runtime() {
        let f = feel_fn(json!([]), "   ");
        assert!(matches!(evaluate(&f, &json!({})), Err(FunctionError::EmptyBody)));
        let mut r = feel_fn(json!([]), "1");
        r.runtime = FunctionRuntime::Rhai;
        assert!(matches!(evaluate(&r, &json!({})), Err(FunctionError::UnsupportedRuntime(_))));
    }
}
