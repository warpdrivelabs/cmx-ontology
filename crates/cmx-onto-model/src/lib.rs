//! cmx-onto-model —— 本体平台的语义中立内核。
//!
//! 元模型（对象/属性/关系/接口/共享属性/动作/函数类型）+ 清单/版本 DTO + 驱动无关的
//! [`OntologyStore`] 持久化契约 + 错误类型。零 DB / 零 cmx-* infra 依赖。

pub mod def;
pub mod draft;
pub mod error;
pub mod object_store;
pub mod objectset;
pub mod store;
pub mod view;

pub mod action;
pub mod feel;
pub mod rhai_engine;
pub mod function;
pub mod authz;
pub mod funnel;
pub mod import;
pub mod osdk;

pub use def::*;
pub use draft::{
    derive_deletions, diff_snapshots, snapshot_fingerprint, validate_draft, DeletionRef,
    DiffAction, DiffItem, DraftContent, DraftRow, IssueSeverity, ValidationIssue, ELEMENT_KINDS,
    KIND_ACTION, KIND_FUNCTION, KIND_INTERFACE, KIND_LINK, KIND_OBJECT, KIND_SHARED, KIND_VIEW,
};
pub use error::{Error, Result, StoreError, StoreResult};
pub use object_store::{LinkEnds, LinkResolver, ObjectStore};
pub use objectset::*;
pub use store::OntologyStore;
pub use view::{SceneViewDef, SceneViewMeta, ViewMembers, ViewSource};
pub use action::{resolve_edits, resolve_side_effects, validate_params, run_validations, SideEffect, ValidationFailure, ObjectEdit};
pub use feel::{eval_expression, eval_predicate, FeelError};
pub use function::{evaluate as evaluate_function, input_specs, check_inputs, InputSpec, FunctionError};
pub use authz::{residual_set, redact_rows};
pub use funnel::{map_row, MappedObject, SourceMapping, SyncReport, Violation};
pub use import::{map_doc, map_dct, DocImport, DctImport};
pub use osdk::generate_typescript;

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

    // ───────── backing 强类型（#5） ─────────

    #[test]
    fn backing_parsed_defaults_to_edge() {
        // 空 backing（未指定）→ Edge。
        let lt = LinkTypeDef {
            api_name: "l".into(),
            object_type_a: "A".into(),
            object_type_b: "B".into(),
            ..Default::default()
        };
        assert_eq!(lt.backing_parsed(), LinkBacking::Edge);
    }

    #[test]
    fn backing_parsed_foreign_key() {
        let lt: LinkTypeDef = serde_json::from_value(json!({
            "apiName": "orderByCustomer",
            "objectTypeA": "Customer",
            "objectTypeB": "Order",
            "backing": { "kind": "foreignKey", "property": "customerId", "side": "b" }
        }))
        .unwrap();
        assert_eq!(
            lt.backing_parsed(),
            LinkBacking::ForeignKey { property: "customerId".into(), side: LinkEnd::B }
        );
        assert!(lt.validate().is_ok());
    }

    #[test]
    fn backing_foreign_key_side_defaults_to_a() {
        // side 缺省 → A。
        let lt: LinkTypeDef = serde_json::from_value(json!({
            "apiName": "l", "objectTypeA": "A", "objectTypeB": "B",
            "backing": { "kind": "foreignKey", "property": "ref" }
        }))
        .unwrap();
        assert_eq!(
            lt.backing_parsed(),
            LinkBacking::ForeignKey { property: "ref".into(), side: LinkEnd::A }
        );
    }

    #[test]
    fn backing_foreign_key_empty_property_rejected() {
        let lt: LinkTypeDef = serde_json::from_value(json!({
            "apiName": "l", "objectTypeA": "A", "objectTypeB": "B",
            "backing": { "kind": "foreignKey", "property": "" }
        }))
        .unwrap();
        let e = lt.validate().unwrap_err().to_string();
        assert!(e.contains("property"), "应报 FK property 不能为空: {e}");
    }

    #[test]
    fn backing_foreign_key_bad_property_rejected() {
        let lt: LinkTypeDef = serde_json::from_value(json!({
            "apiName": "l", "objectTypeA": "A", "objectTypeB": "B",
            "backing": { "kind": "foreignKey", "property": "2bad; DROP" }
        }))
        .unwrap();
        assert!(lt.validate().is_err());
    }

    #[test]
    fn backing_garbage_falls_back_to_edge() {
        // 非法/无法识别的 backing JSON → Edge 兜底（不崩、向后兼容）。
        let lt: LinkTypeDef = serde_json::from_value(json!({
            "apiName": "l", "objectTypeA": "A", "objectTypeB": "B",
            "backing": { "kind": "bogusKind" }
        }))
        .unwrap();
        assert_eq!(lt.backing_parsed(), LinkBacking::Edge);
        // Edge 兜底下 validate 不因 backing 报错。
        assert!(lt.validate().is_ok());
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

    // ───────── 接口强校验 validate_implements（#4） ─────────

    fn shared_prop(name: &str, bt: PropertyBaseType) -> SharedPropertyTypeDef {
        SharedPropertyTypeDef { api_name: name.into(), base_type: bt, ..Default::default() }
    }

    fn prop_ref(name: &str, spt: &str, bt: PropertyBaseType) -> PropertyTypeDef {
        PropertyTypeDef {
            api_name: name.into(),
            base_type: bt,
            shared_property: Some(spt.into()),
            ..Default::default()
        }
    }

    #[test]
    fn implements_ok_when_shared_property_present_and_typed() {
        let iface = InterfaceDef {
            api_name: "Locatable".into(),
            properties: vec!["geohash".into()],
            ..Default::default()
        };
        let spt = shared_prop("geohash", PropertyBaseType::Geohash);
        let ot = ObjectTypeDef {
            api_name: "Store".into(),
            implements: vec!["Locatable".into()],
            properties: vec![prop_ref("loc", "geohash", PropertyBaseType::Geohash)],
            ..Default::default()
        };
        assert!(validate_implements(&ot, &[iface], &[spt]).is_ok());
    }

    #[test]
    fn implements_no_declarations_is_ok() {
        let ot = ObjectTypeDef { api_name: "Plain".into(), ..Default::default() };
        assert!(validate_implements(&ot, &[], &[]).is_ok());
    }

    #[test]
    fn implements_missing_shared_property_rejected() {
        let iface = InterfaceDef {
            api_name: "Locatable".into(),
            properties: vec!["geohash".into()],
            ..Default::default()
        };
        let spt = shared_prop("geohash", PropertyBaseType::Geohash);
        // 对象类型没有引用 geohash 的属性。
        let ot = ObjectTypeDef {
            api_name: "Store".into(),
            implements: vec!["Locatable".into()],
            properties: vec![prop("id")],
            ..Default::default()
        };
        let e = validate_implements(&ot, &[iface], &[spt]).unwrap_err().to_string();
        assert!(e.contains("未满足") && e.contains("geohash"), "应报缺少共享属性: {e}");
    }

    #[test]
    fn implements_type_mismatch_rejected() {
        let iface = InterfaceDef {
            api_name: "Locatable".into(),
            properties: vec!["geohash".into()],
            ..Default::default()
        };
        let spt = shared_prop("geohash", PropertyBaseType::Geohash);
        // 引用了共享属性但 baseType 不符（String ≠ Geohash）。
        let ot = ObjectTypeDef {
            api_name: "Store".into(),
            implements: vec!["Locatable".into()],
            properties: vec![prop_ref("loc", "geohash", PropertyBaseType::String)],
            ..Default::default()
        };
        assert!(validate_implements(&ot, &[iface], &[spt]).is_err());
    }

    #[test]
    fn implements_unknown_interface_rejected() {
        let ot = ObjectTypeDef {
            api_name: "Store".into(),
            implements: vec!["Ghost".into()],
            ..Default::default()
        };
        let e = validate_implements(&ot, &[], &[]).unwrap_err().to_string();
        assert!(e.contains("Ghost") && e.contains("未定义"), "应报接口未定义: {e}");
    }

    #[test]
    fn implements_unknown_shared_property_rejected() {
        // 接口要求某共享属性，但该共享属性定义缺失。
        let iface = InterfaceDef {
            api_name: "Locatable".into(),
            properties: vec!["geohash".into()],
            ..Default::default()
        };
        let ot = ObjectTypeDef {
            api_name: "Store".into(),
            implements: vec!["Locatable".into()],
            properties: vec![prop_ref("loc", "geohash", PropertyBaseType::Geohash)],
            ..Default::default()
        };
        let e = validate_implements(&ot, &[iface], &[]).unwrap_err().to_string();
        assert!(e.contains("geohash") && e.contains("未定义"), "应报共享属性未定义: {e}");
    }

    // ───────── 序列化契约（camelCase + 枚举 round-trip） ─────────

    #[test]
    fn enum_serializes_camel_case() {
        assert_eq!(serde_json::to_value(TypeStatus::Experimental).unwrap(), json!("experimental"));
        assert_eq!(serde_json::to_value(LinkCardinality::OneToMany).unwrap(), json!("oneToMany"));
        assert_eq!(serde_json::to_value(LinkCardinality::ManyToOne).unwrap(), json!("manyToOne"));
        assert_eq!(
            serde_json::from_value::<LinkCardinality>(json!("manyToOne")).unwrap(),
            LinkCardinality::ManyToOne
        );
        assert_eq!(serde_json::to_value(PropertyBaseType::MediaReference).unwrap(), json!("mediaReference"));
        assert_eq!(serde_json::to_value(FunctionRuntime::Feel).unwrap(), json!("feel"));
        assert_eq!(serde_json::to_value(FunctionKind::DerivedProperty).unwrap(), json!("derivedProperty"));
    }

    // ───────── 场景视图（om_view）校验 ─────────

    #[test]
    fn view_auto_requires_prefix_and_no_members() {
        let v = SceneViewDef {
            api_name: "auto:采购域".into(),
            source: ViewSource::Auto,
            members: ViewMembers { objects: vec!["CxOrder".into()], ..Default::default() },
            ..Default::default()
        };
        let e = v.validate().unwrap_err().to_string();
        assert!(e.contains("不物化"), "auto 视图不得物化成员: {e}");
        let v2 = SceneViewDef { api_name: "noPrefix".into(), source: ViewSource::Auto, ..Default::default() };
        assert!(v2.validate().is_err());
    }

    #[test]
    fn view_manual_allows_auto_prefix_for_conversion() {
        // auto→manual 转换保留原名（域默认视图固化语义）：manual 允许 auto: 前缀。
        let v = SceneViewDef {
            api_name: "auto:hr".into(),
            source: ViewSource::Manual,
            members: ViewMembers { objects: vec!["DgCand1".into()], ..Default::default() },
            ..Default::default()
        };
        assert!(v.validate().is_ok());
        let ok = SceneViewDef {
            api_name: "procure_scene".into(),
            source: ViewSource::Manual,
            members: ViewMembers { objects: vec!["A".into()], interfaces: vec!["I".into()] },
            ..Default::default()
        };
        assert!(ok.validate().is_ok());
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
