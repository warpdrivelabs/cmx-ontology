//! DOC/DCT 反向导入 · 内核（纯逻辑、零 IO、可单测）。
//!
//! 把既有 cmx-model 定义映射为本体元素（§6.1.4）：
//! - **DOC**（主从实体图）→ 每实体一个对象类型 + 父子关系一条组合关系（LinkType）。
//! - **DCT**（字典）→ 一个参照对象类型（code 主键 + name 标题）+ 字典项作为种子对象。
//!
//! 输入为**归一化 JSON**（调用方从 cmx-model DocMetaView/DctQuery 适配而来），保持 onto 与 cmx-model 解耦。

use crate::def::{DamRef, DocTypeRef, LinkCardinality, LinkTypeDef, ObjectTypeDef, PropertyBaseType, PropertyTypeDef, TypeStatus};
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

/// 读归一化 JSON 的可选 DAM（{domain,application,module}）——DOC/DCT 若带模块上下文则回填。
fn dam_of(v: &Value) -> DamRef {
    match v.get("dam") {
        Some(d) => DamRef { domain: s(d, "domain"), application: s(d, "application"), module: s(d, "module") },
        None => DamRef::default(),
    }
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

/// 解析实体的标量字段（不含子层）→ PropertyTypeDef 列表。
fn entity_scalar_props(e: &Value) -> Vec<PropertyTypeDef> {
    e.get("properties")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(to_property).collect())
        .unwrap_or_default()
}

/// 把一个（子）实体递归映射为**层块属性**：base_type=array(1:N)|struct(1:1)，
/// `constraints = { entity, displayName, level:true, children:[标量字段 + 更深层块] }`。
/// role = 该层在父上的属性名（如 lines）。
fn entity_to_level_prop(
    role: &str,
    card: LinkCardinality,
    child: &Value,
    entities: &std::collections::HashMap<String, Value>,
    rels: &[Value],
) -> PropertyTypeDef {
    let child_api = s(child, "apiName");
    // 该层字段 = 自身标量属性（转 JSON 形态，便于放进 constraints.children）+ 更深子层。
    let mut children: Vec<Value> = Vec::new();
    for p in entity_scalar_props(child) {
        children.push(prop_to_json(&p));
    }
    for (crole, ccard, cto) in child_relations(&child_api, rels) {
        if let Some(grand) = entities.get(&cto) {
            let deep = entity_to_level_prop(&crole, ccard, grand, entities, rels);
            children.push(prop_to_json(&deep));
        }
    }
    let base_type = match card {
        LinkCardinality::OneToOne => PropertyBaseType::Struct,
        _ => PropertyBaseType::Array,
    };
    let api = if role.is_empty() { child_api.clone() } else { role.to_string() };
    PropertyTypeDef {
        api_name: api,
        display_name: s(child, "displayName"),
        base_type,
        constraints: json!({
            "level": true,
            "entity": child_api,
            "displayName": s(child, "displayName"),
            "children": children,
        }),
        ..Default::default()
    }
}

/// PropertyTypeDef → 归一化 JSON（放进 constraints.children；保留 apiName/baseType/required/子层 constraints）。
fn prop_to_json(p: &PropertyTypeDef) -> Value {
    let base = serde_json::to_value(&p.base_type).unwrap_or(Value::String("string".into()));
    let mut o = json!({ "apiName": p.api_name, "baseType": base, "required": p.required });
    if !p.display_name.is_empty() {
        o["displayName"] = Value::String(p.display_name.clone());
    }
    if !p.constraints.is_null() {
        o["constraints"] = p.constraints.clone();
    }
    o
}

/// 找某实体的直接子层：relations 中 from==parent 的 (role, cardinality, to)。
fn child_relations(parent: &str, rels: &[Value]) -> Vec<(String, LinkCardinality, String)> {
    let mut out = Vec::new();
    for r in rels {
        if s(r, "from") != parent {
            continue;
        }
        let to = s(r, "to");
        if to.is_empty() {
            continue;
        }
        let card = match s(r, "cardinality").as_str() {
            "oneToOne" => LinkCardinality::OneToOne,
            "manyToMany" => LinkCardinality::ManyToMany,
            _ => LinkCardinality::OneToMany,
        };
        let role = { let ro = s(r, "role"); if ro.is_empty() { to.clone() } else { ro } };
        out.push((role, card, to));
    }
    out
}

/// 映射 DOC 归一化 JSON → **单一对象类型（业务单据整体）**，其子层为嵌套复合属性（层块）。
///
/// 业务单据是一个整体：头/行/明细是同一张单据的不同层次，每层有自己的属性。根实体 = 无入边的实体
/// （不作为任何 relation.to）；每个子实体折叠为父的一个 array(1:N)/struct(1:1) 层块属性，
/// 其 `constraints.children` 承载该层字段 + 更深层（递归）。不再拆成多对象、不产组合关系。
///
/// 输入：`{ apiName, displayName?, dam?, entities:[{apiName, displayName?, primaryKey, titleProperty?,
/// properties:[{apiName, baseType?, required?}]}], relations:[{from, to, cardinality?, role?}] }`。
pub fn map_doc(doc: &Value) -> Result<DocImport, String> {
    let entities_arr = doc.get("entities").and_then(|v| v.as_array()).ok_or("DOC 缺 entities 数组")?;
    if entities_arr.is_empty() {
        return Err("DOC entities 为空".into());
    }
    // 单据级：DAM + 业务单据类型（该 DOC 的 apiName/displayName）+ 存证来源，回填到根对象。
    let doc_api = s(doc, "apiName");
    let doc_display = { let d = s(doc, "displayName"); if d.is_empty() { doc_api.clone() } else { d } };
    let dam = dam_of(doc);
    let doc_type = if doc_api.is_empty() {
        DocTypeRef::default()
    } else {
        DocTypeRef { code: doc_api.clone(), name: doc_display.clone() }
    };
    let origin = json!({ "source": "doc", "docApiName": doc_api, "docDisplayName": doc_display });

    // 索引实体 + 关系；根 = 不作为任何 relation.to 的实体（无入边）。
    let mut entities: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    for e in entities_arr {
        let a = s(e, "apiName");
        if a.is_empty() {
            return Err("实体缺 apiName".into());
        }
        entities.insert(a, e.clone());
    }
    let rels: Vec<Value> = doc.get("relations").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let has_incoming: std::collections::HashSet<String> =
        rels.iter().map(|r| s(r, "to")).filter(|t| !t.is_empty()).collect();

    let mut roots: Vec<&Value> = entities_arr.iter().filter(|e| !has_incoming.contains(&s(e, "apiName"))).collect();
    if roots.is_empty() {
        roots.push(&entities_arr[0]); // 全成环兜底：取首实体为根
    }

    // 每个根 = 一张业务单据 = 一个对象类型；其子层折叠为嵌套层块属性。
    let mut object_types = Vec::new();
    for root in roots {
        let api_name = s(root, "apiName");
        let mut props = entity_scalar_props(root);
        for (role, card, to) in child_relations(&api_name, &rels) {
            if let Some(child) = entities.get(&to) {
                props.push(entity_to_level_prop(&role, card, child, &entities, &rels));
            }
        }
        let primary_key = {
            let pk = s(root, "primaryKey");
            if pk.is_empty() {
                // 首个标量属性（跳过层块）作主键。
                props.iter().find(|p| !matches!(p.base_type, PropertyBaseType::Array | PropertyBaseType::Struct))
                    .map(|p| p.api_name.clone()).unwrap_or_default()
            } else {
                pk
            }
        };
        if primary_key.is_empty() {
            return Err(format!("实体 {api_name} 无主键且无标量属性可推断"));
        }
        let title_property = { let t = s(root, "titleProperty"); if t.is_empty() { primary_key.clone() } else { t } };
        object_types.push(ObjectTypeDef {
            api_name: api_name.clone(),
            display_name: { let d = s(root, "displayName"); if d.is_empty() { doc_display.clone() } else { d } },
            dam: dam.clone(),
            doc_type: doc_type.clone(),
            primary_key,
            title_property,
            status: TypeStatus::Active, // 导入自既有定义 → 直接可用
            properties: props,
            cmx_origin: Some(origin.clone()),
            ..Default::default()
        });
    }

    // 业务单据内部层级 = 嵌套属性，非对象间关系 → 不产 link_types。
    Ok(DocImport { object_types, link_types: Vec::new() })
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
        dam: dam_of(dct),
        primary_key: code_prop.clone(),
        title_property: name_prop.clone(),
        status: TypeStatus::Active,
        properties: vec![
            PropertyTypeDef { api_name: code_prop.clone(), base_type: PropertyBaseType::String, required: true, ..Default::default() },
            PropertyTypeDef { api_name: name_prop.clone(), base_type: PropertyBaseType::String, ..Default::default() },
        ],
        // 存证来源（DCT=字典/参照，非业务单据 → doc_type 留空，对象浏览器归「未归类单据」桶）。
        cmx_origin: Some(json!({ "source": "dct", "dctApiName": api_name })),
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
    fn doc_maps_to_single_object_with_nested_levels() {
        let doc = json!({
            "apiName": "SalesOrder", "displayName": "销售订单",
            "dam": { "domain": "fi", "application": "cmxfico", "module": "sd" },
            "entities": [
                { "apiName": "SoHead", "displayName": "订单头", "primaryKey": "id", "titleProperty": "orderNo",
                  "properties": [{"apiName":"id","baseType":"string"},{"apiName":"orderNo","baseType":"string"},{"apiName":"amount","baseType":"decimal"}] },
                { "apiName": "SoLine", "displayName": "订单行", "primaryKey": "lineId",
                  "properties": [{"apiName":"lineId","baseType":"string"},{"apiName":"qty","baseType":"long"}] }
            ],
            "relations": [ { "from": "SoHead", "to": "SoLine", "cardinality": "oneToMany", "role": "lines" } ]
        });
        let r = map_doc(&doc).unwrap();
        // 业务单据 = 一个整体 → 单一对象类型（根实体 SoHead），子层为嵌套层块属性。
        assert_eq!(r.object_types.len(), 1, "DOC 应映射为单一对象");
        assert!(r.link_types.is_empty(), "单据内部层级不产对象间关系");
        let ot = &r.object_types[0];
        assert_eq!(ot.api_name, "SoHead");
        assert_eq!(ot.primary_key, "id");
        assert_eq!(ot.title_property, "orderNo");
        // 头级标量属性(id/orderNo/amount) + 一个「行」层块属性 lines。
        assert_eq!(ot.properties.len(), 4);
        assert_eq!(ot.doc_type.code, "SalesOrder");
        assert_eq!(ot.dam.module, "sd");
        assert!(ot.cmx_origin.is_some());
        // 层块 lines = array，constraints.children 含行字段 qty。
        let lines = ot.properties.iter().find(|p| p.api_name == "lines").expect("有 lines 层块");
        assert!(matches!(lines.base_type, PropertyBaseType::Array), "1:N 子层 → array");
        assert_eq!(lines.constraints["level"], json!(true));
        assert_eq!(lines.constraints["entity"], json!("SoLine"));
        let kids = lines.constraints["children"].as_array().expect("children 数组");
        assert!(kids.iter().any(|c| c["apiName"] == json!("qty")), "行层含字段 qty");
        assert!(kids.iter().any(|c| c["apiName"] == json!("lineId")));
    }

    #[test]
    fn doc_nests_three_levels() {
        // 头 → 行 → 明细：三层递归嵌套（层中层）。
        let doc = json!({ "apiName": "PO", "entities": [
            { "apiName": "Head", "properties": [{"apiName":"hid"}] },
            { "apiName": "Line", "properties": [{"apiName":"lid"}] },
            { "apiName": "Detail", "properties": [{"apiName":"did"}] }
        ], "relations": [
            { "from": "Head", "to": "Line", "role": "lines" },
            { "from": "Line", "to": "Detail", "role": "details" }
        ] });
        let r = map_doc(&doc).unwrap();
        assert_eq!(r.object_types.len(), 1);
        assert_eq!(r.object_types[0].api_name, "Head"); // 根 = 无入边
        assert_eq!(r.object_types[0].primary_key, "hid"); // 推断首标量
        let lines = r.object_types[0].properties.iter().find(|p| p.api_name == "lines").unwrap();
        let lkids = lines.constraints["children"].as_array().unwrap();
        // 行层里应嵌一个 details 层块（层中层）。
        let details = lkids.iter().find(|c| c["apiName"] == json!("details")).expect("行层嵌明细层");
        assert_eq!(details["constraints"]["entity"], json!("Detail"));
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
