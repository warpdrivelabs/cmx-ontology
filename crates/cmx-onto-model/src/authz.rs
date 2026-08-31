//! O6 动态安全 · 内核（纯逻辑、零 IO、可单测）。
//!
//! 决策/执行解耦的**执行侧**：把主体（用户/角色）适用策略的**残差行约束**合并进对象集查询
//! （追加一层 `Filter`，复用对象集代数编译器），并对命中 marking 的列做**脱敏**。
//! 不依赖 cmx-dataauth（独立微服务纪律）；策略来源与主体匹配在壳层，此处只做纯合并/脱敏。

use crate::objectset::{ObjectRecord, ObjectSet, Predicate};
use serde_json::Value;
use std::collections::HashMap;

/// 把若干行级残差谓词合并进对象集：包一层 `Filter`（空则原样返回）。
///
/// 编译器对 `Filter` 在终端类型表上加 `props->>` 谓词，故残差约束天然生效、无需改编译器。
pub fn residual_set(set: ObjectSet, filters: Vec<Predicate>) -> ObjectSet {
    let mut filters: Vec<Predicate> = filters.into_iter().collect();
    match filters.len() {
        0 => set,
        1 => ObjectSet::Filter {
            source: Box::new(set),
            predicate: filters.pop().expect("len==1"),
        },
        _ => ObjectSet::Filter {
            source: Box::new(set),
            predicate: Predicate::And { predicates: filters },
        },
    }
}

/// 列级脱敏：把属性 marking ∈ `deny_markings` 的值替换为 `***`。
///
/// `marking_by_prop`：属性 apiName → 其 marking（来自对象类型定义）。就地改 rows。
pub fn redact_rows(
    rows: &mut [ObjectRecord],
    deny_markings: &[String],
    marking_by_prop: &HashMap<String, String>,
) {
    if deny_markings.is_empty() || marking_by_prop.is_empty() {
        return;
    }
    for r in rows.iter_mut() {
        if let Value::Object(m) = &mut r.properties {
            for (k, v) in m.iter_mut() {
                if let Some(mk) = marking_by_prop.get(k) {
                    if deny_markings.iter().any(|d| d == mk) {
                        *v = Value::String("***".to_string());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base(t: &str) -> ObjectSet {
        ObjectSet::Base { object_type: t.into() }
    }

    #[test]
    fn residual_none_is_identity() {
        let s = residual_set(base("Order"), vec![]);
        assert!(matches!(s, ObjectSet::Base { .. }));
    }

    #[test]
    fn residual_one_wraps_filter() {
        let p = Predicate::Eq { property: "region".into(), value: json!("east") };
        let s = residual_set(base("Order"), vec![p]);
        match s {
            ObjectSet::Filter { predicate, .. } => assert!(matches!(predicate, Predicate::Eq { .. })),
            _ => panic!("expected Filter"),
        }
    }

    #[test]
    fn residual_many_wraps_and() {
        let s = residual_set(
            base("Order"),
            vec![
                Predicate::Eq { property: "region".into(), value: json!("east") },
                Predicate::Gt { property: "amount".into(), value: json!(0) },
            ],
        );
        match s {
            ObjectSet::Filter { predicate: Predicate::And { predicates }, .. } => assert_eq!(predicates.len(), 2),
            _ => panic!("expected Filter(And)"),
        }
    }

    #[test]
    fn redact_masks_marked_columns() {
        let mut rows = vec![ObjectRecord {
            pk: "1".into(),
            title: "x".into(),
            properties: json!({ "name": "Ada", "ssn": "123-45", "region": "east" }),
        }];
        let mut mk = HashMap::new();
        mk.insert("ssn".to_string(), "pii".to_string());
        redact_rows(&mut rows, &["pii".to_string()], &mk);
        assert_eq!(rows[0].properties["ssn"], json!("***"));
        assert_eq!(rows[0].properties["name"], json!("Ada")); // 未标记不脱敏
    }
}
