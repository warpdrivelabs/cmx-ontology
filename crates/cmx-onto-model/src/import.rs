//! DOC/DCT 反向导入 · 内核（纯逻辑、零 IO、可单测）。
//!
//! 把既有 cmx-model 定义映射为本体元素（§6.1.4）：
//! - **DOC**（主从实体图）→ 每实体一个对象类型 + 父子关系一条组合关系（LinkType）。
//! - **DCT**（字典）→ 一个参照对象类型（code 主键 + name 标题）+ 字典项作为种子对象。
//!
//! 输入为**归一化 JSON**（调用方从 cmx-model DocMetaView/DctQuery 适配而来），保持 onto 与 cmx-model 解耦。

use crate::def::{LinkCardinality, LinkTypeDef, ObjectTypeDef, PropertyBaseType, PropertyTypeDef, TypeStatus};
use crate::objectset::ObjectRecord;
use serde_json::{json, Map, Value};

/// DOC 导入结果。
pub struct DocImport {
    pub object_types: Vec<ObjectTypeDef>,
    pub link_types: Vec<LinkTypeDef>,
}

/// DCT 导入结果（参照对象类型 + 字典项种子）。
pub struct DctImport {
    pub object_type: ObjectTypeDef,
    pub items: Vec<ObjectRecord>,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// 解析一条属性 JSON `{apiName, baseType?, required?, ...}` → PropertyTypeDef。
fn to_property(p: &Value) -> Option<PropertyTypeDef> {
    let api_name = s(p, "apiName");
    if api_name.is_empty() {
        return None;
    }
    let base_type = match s(p, "baseType").as_str() {
        "integer" => PropertyBaseType::Integer,
        "long" => PropertyBaseType::Long,
        "double" => PropertyBaseType::Double,
        "decimal" => PropertyBaseType::Decimal,
        "boolean" => PropertyBaseType::Boolean,
        "date" => PropertyBaseType::Date,
        "timestamp" => PropertyBaseType::Timestamp,
        _ => PropertyBaseType::String,
    };
    Some(PropertyTypeDef {
        api_name,
        base_type,
        required: p.get("required").and_then(|v| v.as_bool()).unwrap_or(false),
        ..Default::default()
    })
}

/// 映射 DOC 归一化 JSON → 对象类型 + 组合关系。
///
/// 输入：`{ apiName, displayName?, entities:[{apiName, displayName?, primaryKey, titleProperty?,
/// properties:[{apiName, baseType?, required?}]}], relations:[{from, to, cardinality?, role?}] }`。
pub fn map_doc(doc: &Value) -> Result<DocImport, String> {
    let entities = doc.get("entities").and_then(|v| v.as_array()).ok_or("DOC 缺 entities 数组")?;
    if entities.is_empty() {
        return Err("DOC entities 为空".into());
    }
    let mut object_types = Vec::new();
    for e in entities {
        let api_name = s(e, "apiName");
        if api_name.is_empty() {
            return Err("实体缺 apiName".into());
        }
        let props: Vec<PropertyTypeDef> = e
            .get("properties")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(to_property).collect())
            .unwrap_or_default();
        let primary_key = {
            let pk = s(e, "primaryKey");
            if pk.is_empty() {
                props.first().map(|p| p.api_name.clone()).unwrap_or_default()
            } else {
                pk
            }
        };
        if primary_key.is_empty() {
            return Err(format!("实体 {api_name} 无主键且无属性可推断"));
        }
        let title_property = {
            let t = s(e, "titleProperty");
            if t.is_empty() { primary_key.clone() } else { t }
        };
        object_types.push(ObjectTypeDef {
            api_name: api_name.clone(),
            display_name: s(e, "displayName"),
            primary_key,
            title_property,
            status: TypeStatus::Active, // 导入自既有定义 → 直接可用
            properties: props,
            ..Default::default()
        });
    }

    let mut link_types = Vec::new();
    if let Some(rels) = doc.get("relations").and_then(|v| v.as_array()) {
        for r in rels {
            let from = s(r, "from");
            let to = s(r, "to");
            if from.is_empty() || to.is_empty() {
                continue;
            }
            let cardinality = match s(r, "cardinality").as_str() {
                "oneToOne" => LinkCardinality::OneToOne,
                "manyToMany" => LinkCardinality::ManyToMany,
                _ => LinkCardinality::OneToMany, // 主从默认一对多
            };
            let role = {
                let ro = s(r, "role");
                if ro.is_empty() { format!("has{to}") } else { ro }
            };
            link_types.push(LinkTypeDef {
                api_name: format!("{from}_{role}_{to}"),
                display_name: s(r, "displayName"),
                cardinality,
                object_type_a: from,
                object_type_b: to,
                role_a: role,
                role_b: String::new(),
                backing: json!({ "kind": "composition", "source": "doc-import" }),
                status: TypeStatus::Active,
            });
        }
    }
    Ok(DocImport { object_types, link_types })
}

/// 映射 DCT 归一化 JSON → 参照对象类型 + 字典项种子对象。
///
/// 输入：`{ apiName, displayName?, codeProperty?(默认 code), nameProperty?(默认 name),
/// items:[{code, name}] }`。
pub fn map_dct(dct: &Value) -> Result<DctImport, String> {
    let api_name = s(dct, "apiName");
    if api_name.is_empty() {
        return Err("DCT 缺 apiName".into());
    }
    let code_prop = { let c = s(dct, "codeProperty"); if c.is_empty() { "code".into() } else { c } };
    let name_prop = { let n = s(dct, "nameProperty"); if n.is_empty() { "name".into() } else { n } };

    let object_type = ObjectTypeDef {
        api_name: api_name.clone(),
        display_name: s(dct, "displayName"),
        primary_key: code_prop.clone(),
        title_property: name_prop.clone(),
        status: TypeStatus::Active,
        properties: vec![
            PropertyTypeDef { api_name: code_prop.clone(), base_type: PropertyBaseType::String, required: true, ..Default::default() },
            PropertyTypeDef { api_name: name_prop.clone(), base_type: PropertyBaseType::String, ..Default::default() },
        ],
        ..Default::default()
    };

    let mut items = Vec::new();
    if let Some(arr) = dct.get("items").and_then(|v| v.as_array()) {
        for it in arr {
            let code = s(it, &code_prop);
            let name = s(it, &name_prop);
            if code.is_empty() {
                continue;
            }
            let mut props = Map::new();
            props.insert(code_prop.clone(), Value::String(code.clone()));
            props.insert(name_prop.clone(), Value::String(name.clone()));
            items.push(ObjectRecord {
                pk: code.clone(),
                title: if name.is_empty() { code } else { name },
                properties: Value::Object(props),
            });
        }
    }
    Ok(DctImport { object_type, items })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_maps_entities_and_relations() {
        let doc = json!({
            "apiName": "SalesOrder", "displayName": "销售订单",
            "entities": [
                { "apiName": "SoHead", "displayName": "订单头", "primaryKey": "id", "titleProperty": "orderNo",
                  "properties": [{"apiName":"id","baseType":"string"},{"apiName":"orderNo","baseType":"string"},{"apiName":"amount","baseType":"decimal"}] },
                { "apiName": "SoLine", "displayName": "订单行", "primaryKey": "lineId",
                  "properties": [{"apiName":"lineId","baseType":"string"},{"apiName":"qty","baseType":"long"}] }
            ],
            "relations": [ { "from": "SoHead", "to": "SoLine", "cardinality": "oneToMany", "role": "lines" } ]
        });
        let r = map_doc(&doc).unwrap();
        assert_eq!(r.object_types.len(), 2);
        assert_eq!(r.object_types[0].api_name, "SoHead");
        assert_eq!(r.object_types[0].primary_key, "id");
        assert_eq!(r.object_types[0].title_property, "orderNo");
        assert_eq!(r.object_types[0].properties.len(), 3);
        assert!(matches!(r.object_types[0].status, TypeStatus::Active));
        assert_eq!(r.link_types.len(), 1);
        assert_eq!(r.link_types[0].api_name, "SoHead_lines_SoLine");
        assert_eq!(r.link_types[0].object_type_a, "SoHead");
        assert_eq!(r.link_types[0].object_type_b, "SoLine");
        assert_eq!(r.link_types[0].role_a, "lines");
    }

    #[test]
    fn doc_infers_pk_and_default_role() {
        let doc = json!({ "apiName": "X", "entities": [
            { "apiName": "A", "properties": [{"apiName":"aid"}] },
            { "apiName": "B", "properties": [{"apiName":"bid"}] }
        ], "relations": [ { "from": "A", "to": "B" } ] });
        let r = map_doc(&doc).unwrap();
        assert_eq!(r.object_types[0].primary_key, "aid"); // 推断首属性
        assert_eq!(r.link_types[0].api_name, "A_hasB_B"); // 默认 role
    }

    #[test]
    fn dct_maps_reference_type_and_items() {
        let dct = json!({ "apiName": "Currency", "displayName": "币种",
            "items": [ {"code":"USD","name":"美元"}, {"code":"CNY","name":"人民币"} ] });
        let r = map_dct(&dct).unwrap();
        assert_eq!(r.object_type.api_name, "Currency");
        assert_eq!(r.object_type.primary_key, "code");
        assert_eq!(r.object_type.title_property, "name");
        assert_eq!(r.object_type.properties.len(), 2);
        assert_eq!(r.items.len(), 2);
        assert_eq!(r.items[0].pk, "USD");
        assert_eq!(r.items[0].title, "美元");
        assert_eq!(r.items[0].properties["code"], json!("USD"));
    }

    #[test]
    fn empty_doc_errors() {
        assert!(map_doc(&json!({ "apiName": "X", "entities": [] })).is_err());
        assert!(map_dct(&json!({ "displayName": "no apiName" })).is_err());
    }
}
