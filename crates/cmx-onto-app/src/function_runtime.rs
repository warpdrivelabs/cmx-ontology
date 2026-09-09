//! O5 函数运行时 · 壳层分派。
//!
//! cmx-onto-model 只做 FEEL/Rhai 的**纯同步**求值；Wasm/NativeRust 属 IO/重型或应用级逻辑，按分层落壳层。
//! [`eval_function_any`] 是两个调用点（HTTP `/functions/{api}/evaluate`、动作 `callFunction` 副作用）
//! 共用的**异步**分派入口——异步是为 Wasm 的 async Extism 调用预留 await 边界：
//! - FEEL / Rhai → 交 model [`evaluate_function`]（同步纯求值）；
//! - NativeRust → 壳层编译期函数注册表 [`invoke_native`]（同步 Rust 逻辑）；
//! - Wasm → 经 Extism 异步调用已发布插件制品（待接入，见下）。

use cmx_onto_model::{check_inputs, evaluate_function, FunctionDef, FunctionError, FunctionRuntime};
use serde_json::{json, Value};

/// 两调用点共用的运行时分派（异步）。`bound` 为已绑定输入（壳层预注入的标量 / object / objectSet JSON）。
pub async fn eval_function_any(func: &FunctionDef, bound: &Value) -> Result<Value, FunctionError> {
    match func.runtime {
        FunctionRuntime::NativeRust => invoke_native(func, bound),
        FunctionRuntime::Wasm => Err(FunctionError::UnsupportedRuntime(
            "Wasm（待接入 Extism 插件制品）".into(),
        )),
        // FEEL / Rhai：交 model 纯求值（同步）
        _ => evaluate_function(func, bound),
    }
}

/// NativeRust 编译期函数注册表：`func.body` 作函数标识分派到内置 Rust 实现。
///
/// 新增原生函数 = 在此 `match` 加一分支：编译期强类型、可跑任意复杂 Rust 逻辑（FEEL/Rhai 都表达不动时的
/// 终极逃生舱，且零脚本解释开销）。前置校验复用 model 的 [`check_inputs`]。
fn invoke_native(func: &FunctionDef, bound: &Value) -> Result<Value, FunctionError> {
    let name = func.body.trim();
    if name.is_empty() {
        return Err(FunctionError::EmptyBody);
    }
    check_inputs(func, bound)?;
    match name {
        "nativeTierTax" => native_tier_tax(bound),
        other => Err(FunctionError::UnsupportedRuntime(format!(
            "未注册的 NativeRust 函数「{other}」"
        ))),
    }
}

/// 内置原生函数示例：阶梯累进个税（Rust 实现，与 Rhai 版对照——展示编译期任意逻辑）。
/// 1500 → (1500-1000)*0.2 + (1000-500)*0.1 + (500-0)*0.03 = 165。
fn native_tier_tax(bound: &Value) -> Result<Value, FunctionError> {
    let income = bound
        .get("income")
        .and_then(Value::as_f64)
        .ok_or_else(|| FunctionError::Eval("入参 income 缺失或非数值".into()))?;
    let brackets = [(1000.0_f64, 0.20_f64), (500.0, 0.10), (0.0, 0.03)];
    let (mut tax, mut base) = (0.0_f64, income);
    for (floor, rate) in brackets {
        if base > floor {
            tax += (base - floor) * rate;
            base = floor;
        }
    }
    Ok(json!(tax))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_onto_model::FunctionKind;

    fn native_fn(body: &str, inputs: Value) -> FunctionDef {
        FunctionDef {
            api_name: "n".into(),
            runtime: FunctionRuntime::NativeRust,
            kind: FunctionKind::Query,
            inputs,
            body: body.into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn native_tier_tax_evaluates() {
        let f = native_fn("nativeTierTax", json!([{ "name": "income", "type": "double" }]));
        let out = eval_function_any(&f, &json!({ "income": 1500 })).await.unwrap();
        assert_eq!(out, json!(165.0));
    }

    #[tokio::test]
    async fn native_missing_input_and_unknown_and_wasm() {
        // 缺输入
        let f = native_fn("nativeTierTax", json!([{ "name": "income", "type": "double" }]));
        assert!(matches!(
            eval_function_any(&f, &json!({})).await,
            Err(FunctionError::MissingInput(_))
        ));
        // 未注册原生函数
        let u = native_fn("noSuchFn", json!([]));
        assert!(matches!(
            eval_function_any(&u, &json!({})).await,
            Err(FunctionError::UnsupportedRuntime(_))
        ));
        // Wasm 尚未接入
        let mut w = native_fn("plugin", json!([]));
        w.runtime = FunctionRuntime::Wasm;
        assert!(matches!(
            eval_function_any(&w, &json!({})).await,
            Err(FunctionError::UnsupportedRuntime(_))
        ));
    }

    #[tokio::test]
    async fn feel_still_routes_to_model() {
        let mut f = native_fn("if amount > 1000 then 0.8 else 0.2", json!([{ "name": "amount", "type": "double" }]));
        f.runtime = FunctionRuntime::Feel;
        assert_eq!(eval_function_any(&f, &json!({ "amount": 1500 })).await.unwrap(), json!(0.8));
    }
}
