//! O5 函数计算引擎 · Rhai runtime（过程式逃生舱）。
//!
//! 当声明式的 FEEL 表达不动时（阶梯累进、多步过程、带状态迭代），函数作者用一段**受控、沙箱、
//! 可审计**的 Rhai 脚本兜底。移植自 cmx-rulesengine `cmx-rule-feel::script` 的求值内核（沙箱闸门 +
//! serde 桥 + f64 归一），**剔除**其脚本函数库特性（`ScriptFn`/`thread_local`，本体函数是单 body）。
//!
//! 设计取向（与 [`crate::feel`] 同纪律）：
//! - **沙箱**：`max_operations` 限操作数（防死循环 / CPU 耗尽）、`max_call_levels` 限递归深度、
//!   string/array/map 尺寸上限（防内存爆炸）。刻意用操作数而非 wall-clock 做 CPU 闸门——不引
//!   `Instant`，保 wasm / 确定性友好（与 model crate 零 IO 纪律一致）。
//! - **无 IO/OS**：Rhai 默认纯计算，不注册任何文件 / 网络 / 时间 / 随机函数。
//! - **Don't-Panic**：Rhai 保证脚本永不 panic 宿主，错误以 `Result` 返回（带行号）。
//! - **数值归一 f64**：与 FEEL 引擎算术输出一致，保证同一函数 FEEL / Rhai 输出的 JSON 相等。

use rhai::{Dynamic, Engine, EvalAltResult, Scope};
use serde_json::Value;

/// 操作数上限（防死循环 / CPU 耗尽）。~10 万步足够任何合理函数计算。
const MAX_OPERATIONS: u64 = 100_000;
/// 函数调用 / 递归深度上限（防爆栈）。
const MAX_CALL_LEVELS: usize = 32;
/// 字符串最大字节数（防内存爆炸）。
const MAX_STRING_SIZE: usize = 64 * 1024;
/// 数组最大元素数。
const MAX_ARRAY_SIZE: usize = 10_000;
/// 对象（map）最大键数。
const MAX_MAP_SIZE: usize = 10_000;

/// 构造沙箱化 Rhai 引擎（每次求值新建，脚本间不共享可变状态 → 无跨请求污染）。
fn sandboxed_engine() -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(MAX_OPERATIONS);
    engine.set_max_call_levels(MAX_CALL_LEVELS);
    engine.set_max_string_size(MAX_STRING_SIZE);
    engine.set_max_array_size(MAX_ARRAY_SIZE);
    engine.set_max_map_size(MAX_MAP_SIZE);
    engine
}

/// 求值一段 Rhai 脚本：以 `ctx` 对象的各字段为**顶层变量**，返回脚本结果值（数值归一 f64）。
///
/// - `ctx` 为对象时其每个键作为脚本可读变量（`income` / `order` / …）；非对象则脚本无自由变量。
/// - 返回值经 serde 桥回 `serde_json::Value`：Rhai `#{...}` map → JSON 对象、数值 → f64、
///   unit `()` → `null`。
/// - 出错（解析 / 运行 / 超沙箱闸门）→ `Err(String)`（附行号），由调用点包成 `FunctionError::Eval`。
///
/// 永不 panic（Rhai Don't-Panic 保证 + 本函数不 unwrap）。
pub fn eval_script(src: &str, ctx: &Value) -> Result<Value, String> {
    let s = src.trim();
    if s.is_empty() {
        return Ok(Value::Null);
    }
    let engine = sandboxed_engine();
    let mut scope = scope_from_ctx(ctx)?;
    // eval_with_scope::<Dynamic> 取脚本最后一个表达式的值（脚本习惯：末尾裸表达式即返回值）。
    match engine.eval_with_scope::<Dynamic>(&mut scope, s) {
        Ok(dynamic) => dynamic_to_value(dynamic),
        Err(e) => Err(format_eval_error(&e)),
    }
}

/// 把上下文对象的各字段推入 Rhai 作用域作为顶层变量。
fn scope_from_ctx(ctx: &Value) -> Result<Scope<'static>, String> {
    let mut scope = Scope::new();
    if let Value::Object(map) = ctx {
        for (k, v) in map {
            let dynamic = value_to_dynamic(v)?;
            scope.push_dynamic(k.clone(), dynamic);
        }
    }
    Ok(scope)
}

/// `serde_json::Value` → `rhai::Dynamic`（经 rhai::serde 桥）。
fn value_to_dynamic(v: &Value) -> Result<Dynamic, String> {
    rhai::serde::to_dynamic(v.clone()).map_err(|e| format!("上下文值无法转入脚本: {e}"))
}

/// `rhai::Dynamic` → `serde_json::Value`（经 rhai::serde 桥）。unit `()` → `null`；整数归一 f64。
///
/// **数值归一为 f64**：Rhai 有独立 i64/f64（`1+1`→整数 `2`），FEEL 引擎算术统一 f64（`2.0`）。
/// 若不归一，同一函数 FEEL 输出 `income*5`（40000.0）与 Rhai 输出（40000）会产出不等的 JSON
/// （serde_json `Number(i) != Number(f)`），破坏相等判定。故此处把返回值树中整数归一为 f64。
fn dynamic_to_value(d: Dynamic) -> Result<Value, String> {
    if d.is_unit() {
        return Ok(Value::Null);
    }
    let v: Value = rhai::serde::from_dynamic(&d).map_err(|e| format!("脚本返回值无法转出: {e}"))?;
    Ok(normalize_numbers(v))
}

/// 递归把 Value 树中的整数 Number 归一为 f64（与 FEEL 引擎算术输出一致）。
fn normalize_numbers(v: Value) -> Value {
    match v {
        Value::Number(n) => match n.as_f64() {
            Some(f) => Value::from(f),
            None => Value::Number(n),
        },
        Value::Array(a) => Value::Array(a.into_iter().map(normalize_numbers).collect()),
        Value::Object(m) => {
            Value::Object(m.into_iter().map(|(k, val)| (k, normalize_numbers(val))).collect())
        }
        other => other,
    }
}

/// 把 Rhai 运行期错误格式化成带行号的归因文案（Rhai 错误带位置，比 FEEL 更精确）。
fn format_eval_error(e: &EvalAltResult) -> String {
    let suffix = match e.position().line() {
        Some(line) => format!("（第 {line} 行）"),
        None => String::new(),
    };
    format!("脚本求值错误{suffix}: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn eval_basic_arithmetic() {
        // 末尾裸表达式即返回值；整数归一 f64。
        assert_eq!(eval_script("let x = 1; x + 1", &json!({})).unwrap(), json!(2.0));
        assert_eq!(eval_script("40 + 2", &json!({})).unwrap(), json!(42.0));
    }

    #[test]
    fn reads_context_variables() {
        let ctx = json!({ "income": 8000, "level": "gold" });
        assert_eq!(eval_script("income * 5", &ctx).unwrap(), json!(40000.0));
        assert_eq!(eval_script("\"tier-\" + level", &ctx).unwrap(), json!("tier-gold"));
    }

    #[test]
    fn returns_map_object() {
        let ctx = json!({ "income": 30000 });
        let out = eval_script("#{ tax: income * 0.1, net: income * 0.9 }", &ctx).unwrap();
        assert_eq!(out, json!({ "tax": 3000.0, "net": 27000.0 }));
    }

    #[test]
    fn multi_statement_with_loop() {
        // 阶梯累进：Rhai 逃生舱的招牌场景（FEEL 表达不动）。
        let ctx = json!({ "income": 30000 });
        let src = "let taxable = income - 5000;\n\
                   let t = 0.0;\n\
                   let brackets = [[25000.0, 0.25], [12000.0, 0.20], [3000.0, 0.10], [0.0, 0.03]];\n\
                   let b = taxable;\n\
                   for br in brackets { if b > br[0] { t += (b - br[0]) * br[1]; b = br[0]; } }\n\
                   #{ tax: t, taxable: taxable }";
        let out = eval_script(src, &ctx).unwrap();
        let obj = out.as_object().unwrap();
        assert_eq!(obj.get("taxable"), Some(&json!(25000.0)));
        assert!(obj.get("tax").unwrap().as_f64().unwrap() > 0.0);
    }

    #[test]
    fn sandbox_blocks_infinite_loop() {
        // 死循环 → 超操作数上限报错（不挂起、不 panic）。
        let r = eval_script("let i = 0; while true { i += 1; } i", &json!({}));
        assert!(r.is_err(), "死循环应被沙箱 kill");
    }

    #[test]
    fn panic_does_not_escape() {
        // 运行期错误（未定义变量）落 Err，不穿透。
        assert!(eval_script("undefined_var + nope", &json!({})).is_err());
    }

    #[test]
    fn error_carries_line_number() {
        // 第 2 行调用不存在的函数。
        let msg = eval_script("let a = 1;\nno_such_fn(a)", &json!({})).unwrap_err();
        assert!(msg.contains("行"), "错误应含行号: {msg}");
    }

    #[test]
    fn serde_roundtrip_nested() {
        // 嵌套结构双向桥接；len()→3 归一为 3.0。
        let ctx = json!({ "order": { "items": [1, 2, 3], "vip": true } });
        let out = eval_script("#{ n: order.items.len(), vip: order.vip }", &ctx).unwrap();
        assert_eq!(out, json!({ "n": 3.0, "vip": true }));
    }

    #[test]
    fn empty_script_is_null() {
        assert_eq!(eval_script("", &json!({})).unwrap(), json!(null));
        assert_eq!(eval_script("   ", &json!({})).unwrap(), json!(null));
    }
}
