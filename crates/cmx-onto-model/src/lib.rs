//! cmx-onto-model —— 本体平台的语义中立内核。
//!
//! 元模型（对象/属性/关系/接口/共享属性/动作/函数类型）+ 清单/版本 DTO + 驱动无关的
//! [`OntologyStore`] 持久化契约 + 错误类型。零 DB / 零 cmx-* infra 依赖。

pub mod def;
pub mod error;
pub mod object_store;
pub mod objectset;
pub mod store;

pub mod action;
pub mod feel;
pub mod function;
pub mod authz;
pub mod funnel;

pub use def::*;
pub use error::{Error, Result, StoreError, StoreResult};
pub use object_store::{LinkEnds, LinkResolver, ObjectStore};
pub use objectset::*;
pub use store::OntologyStore;
pub use action::{resolve_edits, resolve_side_effects, validate_params, run_validations, SideEffect, ValidationFailure, ObjectEdit};
pub use feel::{eval_expression, eval_predicate, FeelError};
pub use function::{evaluate as evaluate_function, input_specs, check_inputs, InputSpec, FunctionError};
pub use authz::{residual_set, redact_rows};
pub use funnel::{map_row, MappedObject, SourceMapping, SyncReport, Violation};

/// 单租户 / 无租户 scope 的默认租户名。
pub const DEFAULT_TENANT: &str = "default";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ───────── apiName 校验 ─────────

    #[test]
    fn api_name_rules() {
        assert!(is_valid_api_name("Customer"));
        assert!(is_valid_api_name("_private"));
        assert!(is_valid_api_name("order_line_2"));
        assert!(!is_valid_api_name("")); // 空
        assert!(!is_valid_api_name("2fast")); // 数字开头
        assert!(!is_valid_api_name("has space"));
        assert!(!is_valid_api_name("dash-no")); // 连字符
        assert!(!is_valid_api_name("dot.no"));
    }

    // ───────── 对象类型校验 ─────────

    fn prop(name: &str) -> PropertyTypeDef {
        PropertyTypeDef { api_name: name.into(), ..Default::default() }
    }

    #[test]
    fn object_type_ok() {
        let ot = ObjectTypeDef {
            api_name: "Customer".into(),
            primary_key: "id".into(),
            title_property: "name".into(),
            properties: vec![prop("id"), prop("name"), prop("region")],
            ..Default::default()
        };
        assert!(ot.validate().is_ok());
    }

    #[test]
    fn object_type_bad_api_name() {
        let ot = ObjectTypeDef { api_name: "2Bad".into(), ..Default::default() };
        assert!(ot.validate().is_err());
    }

    #[test]
    fn object_type_duplicate_property() {
        let ot = ObjectTypeDef {
            api_name: "Customer".into(),
            properties: vec![prop("id"), prop("id")],
            ..Default::default()
        };
        let e = ot.validate().unwrap_err().to_string();
        assert!(e.contains("重复"), "应报属性重复: {e}");
    }

    #[test]
    fn object_type_primary_key_must_exist() {
        let ot = ObjectTypeDef {
            api_name: "Customer".into(),
            primary_key: "nope".into(),
            properties: vec![prop("id")],
            ..Default::default()
        };
        let e = ot.validate().unwrap_err().to_string();
        assert!(e.contains("主键"), "应报主键不存在: {e}");
    }

    #[test]
    fn object_type_title_property_must_exist() {
        let ot = ObjectTypeDef {
            api_name: "Customer".into(),
            title_property: "ghost".into(),
            properties: vec![prop("id")],
            ..Default::default()
        };
        let e = ot.validate().unwrap_err().to_string();
        assert!(e.contains("标题"), "应报标题属性不存在: {e}");
    }

    #[test]
    fn object_type_empty_pk_and_title_allowed() {
        // 主键/标题留空（未指定）不应报错——仅当指定了才校验存在性。
        let ot = ObjectTypeDef {
            api_name: "Draft".into(),
            properties: vec![prop("x")],
            ..Default::default()
        };
        assert!(ot.validate().is_ok());
    }

    // ───────── 关系类型校验 ─────────

    #[test]
    fn link_type_ok() {
        let lt = LinkTypeDef {
            api_name: "customerPlacesOrder".into(),
            object_type_a: "Customer".into(),
            object_type_b: "Order".into(),
            ..Default::default()
        };
        assert!(lt.validate().is_ok());
    }

    #[test]
    fn link_type_needs_both_ends() {
        let lt = LinkTypeDef {
            api_name: "dangling".into(),
            object_type_a: "Customer".into(),
            object_type_b: String::new(),
            ..Default::default()
        };
        let e = lt.validate().unwrap_err().to_string();
        assert!(e.contains("两端"), "应报两端不能为空: {e}");
    }

    // ───────── 其余四类校验 ─────────

    #[test]
    fn interface_shared_action_function_validate() {
        assert!(InterfaceDef { api_name: "Locatable".into(), ..Default::default() }.validate().is_ok());
        assert!(InterfaceDef { api_name: "2bad".into(), ..Default::default() }.validate().is_err());
        assert!(SharedPropertyTypeDef { api_name: "currencyCode".into(), ..Default::default() }.validate().is_ok());
        assert!(ActionTypeDef { api_name: "reassignOrder".into(), ..Default::default() }.validate().is_ok());
        assert!(FunctionDef { api_name: "delayRisk".into(), ..Default::default() }.validate().is_ok());
    }

    // ───────── 序列化契约（camelCase + 枚举 round-trip） ─────────

    #[test]
    fn enum_serializes_camel_case() {
        assert_eq!(serde_json::to_value(TypeStatus::Experimental).unwrap(), json!("experimental"));
        assert_eq!(serde_json::to_value(LinkCardinality::OneToMany).unwrap(), json!("oneToMany"));
        assert_eq!(serde_json::to_value(PropertyBaseType::MediaReference).unwrap(), json!("mediaReference"));
        assert_eq!(serde_json::to_value(FunctionRuntime::Feel).unwrap(), json!("feel"));
        assert_eq!(serde_json::to_value(FunctionKind::DerivedProperty).unwrap(), json!("derivedProperty"));
    }

    #[test]
    fn enum_round_trip_and_default_on_unknown() {
        // 已知值 round-trip。
        let s: TypeStatus = serde_json::from_value(json!("active")).unwrap();
        assert_eq!(s, TypeStatus::Active);
        // 未知值反序列化失败（store 层用 unwrap_or_default 兜底为 Experimental）。
        assert!(serde_json::from_value::<TypeStatus>(json!("bogus")).is_err());
    }

    #[test]
    fn object_type_partial_json_tolerated() {
        // 前端只传部分字段（缺 description/color/properties…）应能反序列化（#[serde(default)]）。
        let ot: ObjectTypeDef = serde_json::from_value(json!({
            "apiName": "Order",
            "primaryKey": "orderId",
            "properties": [{ "apiName": "orderId", "baseType": "long" }]
        }))
        .expect("偏序 JSON 应容忍");
        assert_eq!(ot.api_name, "Order");
        assert_eq!(ot.properties.len(), 1);
        assert_eq!(ot.properties[0].base_type, PropertyBaseType::Long);
        assert_eq!(ot.status, TypeStatus::Experimental); // 默认
    }
}
