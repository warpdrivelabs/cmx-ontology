//! O3 数据集成 · 内核（纯逻辑、零 IO、可单测）。
//!
//! 把「源行（JSON 对象，列名→值）」按 [`SourceMapping`] 映射为「对象（pk/title/properties）」，
//! 并做更严校验（主键非空、必填齐全、类型可转）——违规产出 [`Violation`] 交壳层入隔离区。
//! 连接器读取（IO）在壳层，本模块只做映射/校验（保持芯零 IO）。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 绑定模式 = **读路径来源**（方案 E1；与缓存正交，混合模式 M3 加独立 cache 段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum MappingMode {
    /// 本体平台内置物化表（默认；funnel 同步灌数）。
    #[default]
    Materialized,
    /// 虚拟直查（查询时下推源库；当前仅 PgDirect）。
    Virtual,
}

impl MappingMode {
    /// 解析存储列文本（兼容未知值 → Materialized，存量行语义）。
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "virtual" => Self::Virtual,
            _ => Self::Materialized,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Materialized => "materialized",
            Self::Virtual => "virtual",
        }
    }
}

/// 源 → 对象的映射规格（持久化为 om_source_mapping；升格后 = **绑定唯一权威行**，方案 E1）。
///
/// 同一行两种语义：mode=materialized 时是漏斗灌数映射；mode=virtual 时是查询下推映射（同构）。
#[derive(Debug, Clone, Default)]
pub struct SourceMapping {
    /// 目标对象类型 apiName。
    pub object_type: String,
    /// 绑定模式（权威值；漏斗建的映射恒 materialized，virtual 只能经 bind API 建立）。
    pub mode: MappingMode,
    /// 源资源名：PG 表名（`schema.table` 或 `table`）或 API 资源名。生成式查询与虚拟下推的取数对象。
    pub resource: Option<String>,
    /// 源查询（SELECT …；壳层执行，读出行）。**放宽为可空**（B-P2-3）：虚拟映射恒空（下推
    /// 由 resource+映射自动生成）；物化映射缺省时也由 resource+映射生成参数化 SELECT（M0 生成式默认路径）。
    pub source_query: String,
    /// 主键裁定：这些源列拼成对象 pk（多列以 '|' 连接）。virtual 模式仅支持单列（bind 强校验）。
    pub key_columns: Vec<String>,
    /// 标题源列（可空；缺省用 pk）。
    pub title_column: Option<String>,
    /// 属性映射：源列名 → 对象属性 apiName。
    pub property_map: Vec<(String, String)>,
    /// 必填对象属性 apiName（映射后为空即违规）。
    pub required: Vec<String>,
    /// 源数据源 db_id（可空 = 本体库 onto_pg）；读源在该库执行，写 oo_/隔离区仍走本体库。
    /// M1a 直指 toml `[[databases]]` 池；M1b 起与 source_id 双读（source_id 优先，Q6）。
    pub source_db_id: Option<String>,
    /// 数据源注册表 id（M1b；`om_data_source.id`，读取优先于 source_db_id）。
    pub source_id: Option<String>,
}

impl SourceMapping {
    /// 运行期连接寻址（数据库池 db_id）：恒取 source_db_id——bind 时已写入**生效 db_id**
    /// （ref 池名或 `ontosrc_<id>` 懒注册名）；source_id 仅作注册表溯源展示，不作连接寻址。
    pub fn effective_source(&self) -> Option<&str> {
        self.source_db_id.as_deref().filter(|s| !s.is_empty())
    }
}

/// 一条校验违规。
#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub field: String,
    pub reason: String,
}

/// 映射成功的对象。
#[derive(Debug, Clone, PartialEq)]
pub struct MappedObject {
    pub pk: String,
    pub title: String,
    pub properties: Value,
}

/// 标量 JSON → 字符串（pk/title 拼接用）。
fn scalar_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// 映射一行源数据 → 对象或违规列表。
///
/// `row` 为源行 JSON 对象（列名→值）。规则：
/// 1. 按 property_map 抽取属性（源列缺失 → 该属性不写）；
/// 2. pk = key_columns 各源列标量值以 '|' 连接，任一为空即违规；
/// 3. required 属性映射后为空即违规；
/// 4. title = title_column 值或 pk。
pub fn map_row(mapping: &SourceMapping, row: &Value) -> Result<MappedObject, Vec<Violation>> {
    let mut violations = Vec::new();
    let empty = Map::new();
    let obj = row.as_object().unwrap_or(&empty);

    // 属性
    let mut props = Map::new();
    for (src, prop) in &mapping.property_map {
        if let Some(v) = obj.get(src)
            && !v.is_null() {
                props.insert(prop.clone(), v.clone());
            }
    }

    // 主键裁定
    let mut key_parts = Vec::new();
    for kc in &mapping.key_columns {
        match obj.get(kc).and_then(scalar_str) {
            Some(s) if !s.is_empty() => key_parts.push(s),
            _ => violations.push(Violation {
                field: kc.clone(),
                reason: format!("主键源列「{kc}」为空或非标量"),
            }),
        }
    }
    if mapping.key_columns.is_empty() {
        violations.push(Violation { field: "__pk".into(), reason: "映射未指定主键列".into() });
    }

    // 必填校验
    for req in &mapping.required {
        let missing = props.get(req).map(|v| v.is_null()).unwrap_or(true);
        if missing {
            violations.push(Violation { field: req.clone(), reason: format!("必填属性「{req}」缺失") });
        }
    }

    if !violations.is_empty() {
        return Err(violations);
    }

    let pk = key_parts.join("|");
    let title = mapping
        .title_column
        .as_ref()
        .and_then(|tc| obj.get(tc))
        .and_then(scalar_str)
        .unwrap_or_else(|| pk.clone());

    Ok(MappedObject { pk, title, properties: Value::Object(props) })
}

/// 同步报告。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SyncReport {
    pub read: usize,
    pub written: usize,
    pub quarantined: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mapping() -> SourceMapping {
        SourceMapping {
            object_type: "Customer".into(),
            mode: MappingMode::Materialized,
            source_query: "SELECT * FROM src".into(),
            key_columns: vec!["cust_id".into()],
            title_column: Some("cust_name".into()),
            property_map: vec![
                ("cust_id".into(), "id".into()),
                ("cust_name".into(), "name".into()),
                ("region_code".into(), "region".into()),
            ],
            required: vec!["name".into()],
            source_db_id: None,
            resource: None,
            source_id: None,
        }
    }

    #[test]
    fn maps_row_to_object() {
        let m = mapping();
        let row = json!({ "cust_id": "C-1", "cust_name": "Ada", "region_code": "east" });
        let o = map_row(&m, &row).unwrap();
        assert_eq!(o.pk, "C-1");
        assert_eq!(o.title, "Ada");
        assert_eq!(o.properties, json!({ "id": "C-1", "name": "Ada", "region": "east" }));
    }

    #[test]
    fn numeric_pk_coerced() {
        let mut m = mapping();
        m.key_columns = vec!["cust_id".into()];
        let row = json!({ "cust_id": 42, "cust_name": "x" });
        assert_eq!(map_row(&m, &row).unwrap().pk, "42");
    }

    #[test]
    fn composite_key_joined() {
        let mut m = mapping();
        m.key_columns = vec!["cust_id".into(), "region_code".into()];
        let row = json!({ "cust_id": "C-1", "cust_name": "x", "region_code": "east" });
        assert_eq!(map_row(&m, &row).unwrap().pk, "C-1|east");
    }

    #[test]
    fn missing_pk_is_violation() {
        let m = mapping();
        let row = json!({ "cust_name": "Ada" }); // no cust_id
        let v = map_row(&m, &row).unwrap_err();
        assert!(v.iter().any(|x| x.field == "cust_id"));
    }

    #[test]
    fn missing_required_is_violation() {
        let m = mapping();
        let row = json!({ "cust_id": "C-1" }); // no name
        let v = map_row(&m, &row).unwrap_err();
        assert!(v.iter().any(|x| x.field == "name"));
    }

    // ───────── 绑定模式与源寻址（方案 E1 / Q6 双读） ─────────

    #[test]
    fn mode_parse_defaults_materialized() {
        assert_eq!(MappingMode::parse("virtual"), MappingMode::Virtual);
        assert_eq!(MappingMode::parse("MATERIALIZED"), MappingMode::Materialized);
        assert_eq!(MappingMode::parse(""), MappingMode::Materialized);
        assert_eq!(MappingMode::parse("bogus"), MappingMode::Materialized); // 存量行语义
    }

    #[test]
    fn effective_source_is_runtime_db_id() {
        // 运行期连接寻址恒取 source_db_id（生效 db_id）；source_id 仅注册表溯源，不参与寻址。
        let mut m = mapping();
        assert!(m.effective_source().is_none());
        m.source_id = Some("fico-registry".into());
        assert!(m.effective_source().is_none(), "仅注册表 id 时不可寻址");
        m.source_db_id = Some("ontosrc_fico-registry".into());
        assert_eq!(m.effective_source(), Some("ontosrc_fico-registry"));
    }
}
