//! OSDK 代码生成 · 内核（纯逻辑、零 IO、可单测）。
//!
//! 读本体定义 → 生成**强类型 TypeScript 客户端**（对齐 Palantir OSDK §7.3）：
//! - 每对象类型一个 `interface`（属性→TS 类型）；
//! - `OntologyClient`：`objects.<Type>.all()/.filter()`（映射对象集代数）、
//!   `actions.<Action>(params)`（→ /action-types/{}/execute）、`functions.<Fn>(args)`（→ /functions/{}/evaluate）、
//!   `searchAround(type, pk, link)`（→ Search-Around）。
//! 前端/二开零手写 REST，类型随本体演进。

use crate::def::{LinkTypeDef, ObjectTypeDef, PropertyBaseType};

/// TS 保留字/非法标识 → 安全键（加引号）。
fn key(name: &str) -> String {
    if name.chars().all(|c| c.is_alphanumeric() || c == '_') && !name.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(true) {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\\\""))
    }
}

/// PropertyBaseType → TS 类型。
fn ts_type(t: &PropertyBaseType) -> &'static str {
    match t {
        PropertyBaseType::Integer | PropertyBaseType::Long | PropertyBaseType::Double | PropertyBaseType::Decimal => "number",
        PropertyBaseType::Boolean => "boolean",
        PropertyBaseType::Array => "unknown[]",
        PropertyBaseType::Struct => "Record<string, unknown>",
        _ => "string", // String/Date/Timestamp/其它 → string
    }
}

/// 生成 TypeScript OSDK。`types` 为完整对象类型定义，`links`/`actions`/`functions` 为 apiName 列表。
pub fn generate_typescript(
    types: &[ObjectTypeDef],
    links: &[LinkTypeDef],
    actions: &[String],
    functions: &[String],
) -> String {
    let mut out = String::new();
    out.push_str("// ═══ cmx-ontology OSDK (TypeScript) — 自动生成，勿手改 ═══\n");
    out.push_str(&format!("// 对象类型 {} · 关系 {} · 动作 {} · 函数 {}\n\n", types.len(), links.len(), actions.len(), functions.len()));

    // 基类 + 对象集代数类型
    out.push_str("export interface OntObject { pk: string; title: string; }\n");
    out.push_str("export type Predicate =\n");
    out.push_str("  | { kind: 'eq' | 'ne' | 'gt' | 'ge' | 'lt' | 'le'; property: string; value: unknown }\n");
    out.push_str("  | { kind: 'in'; property: string; values: unknown[] }\n");
    out.push_str("  | { kind: 'contains'; property: string; value: string }\n");
    out.push_str("  | { kind: 'isNull'; property: string }\n");
    out.push_str("  | { kind: 'and' | 'or'; predicates: Predicate[] }\n");
    out.push_str("  | { kind: 'not'; predicate: Predicate };\n");
    out.push_str("export type ObjectSet =\n");
    out.push_str("  | { op: 'base'; objectType: string }\n");
    out.push_str("  | { op: 'static'; objectType: string; primaryKeys: string[] }\n");
    out.push_str("  | { op: 'filter'; source: ObjectSet; predicate: Predicate }\n");
    out.push_str("  | { op: 'searchAround'; source: ObjectSet; link: string; direction: 'forward' | 'reverse' }\n");
    out.push_str("  | { op: 'union' | 'intersect' | 'subtract'; left: ObjectSet; right: ObjectSet };\n");
    out.push_str("export interface ObjectPage<T> { objectType: string; rows: T[]; limit: number; offset: number; hasMore: boolean; }\n\n");

    // 对象类型接口
    for t in types {
        out.push_str(&format!("/** {} */\n", if t.display_name.is_empty() { &t.api_name } else { &t.display_name }));
        out.push_str(&format!("export interface {} extends OntObject {{\n", safe_ident(&t.api_name)));
        for p in &t.properties {
            let opt = if p.required { "" } else { "?" };
            out.push_str(&format!("  {}{}: {};\n", key(&p.api_name), opt, ts_type(&p.base_type)));
        }
        out.push_str("}\n\n");
    }

    // 客户端
    out.push_str("export class OntologyClient {\n");
    out.push_str("  constructor(private base = '/api/onto/v1', private apiKey?: string) {}\n");
    out.push_str("  private async post<T>(path: string, body: unknown): Promise<T> {\n");
    out.push_str("    const h: Record<string,string> = { 'Content-Type': 'application/json' };\n");
    out.push_str("    if (this.apiKey) h['X-API-Key'] = this.apiKey;\n");
    out.push_str("    const r = await fetch(this.base + path, { method: 'POST', headers: h, body: JSON.stringify(body) });\n");
    out.push_str("    const j = await r.json();\n");
    out.push_str("    if (j && typeof j.code === 'number' && j.code !== 0) throw new Error(j.msg || 'error');\n");
    out.push_str("    return (j && 'data' in j ? j.data : j) as T;\n");
    out.push_str("  }\n");
    out.push_str("  private async get<T>(path: string): Promise<T> {\n");
    out.push_str("    const h: Record<string,string> = {}; if (this.apiKey) h['X-API-Key'] = this.apiKey;\n");
    out.push_str("    const r = await fetch(this.base + path, { headers: h }); const j = await r.json();\n");
    out.push_str("    return (j && 'data' in j ? j.data : j) as T;\n");
    out.push_str("  }\n");
    out.push_str("  load<T extends OntObject>(objectSet: ObjectSet, limit = 100): Promise<ObjectPage<T>> {\n");
    out.push_str("    return this.post<ObjectPage<T>>('/object-sets/load', { objectSet, limit });\n");
    out.push_str("  }\n");
    out.push_str("  searchAround<T extends OntObject>(objectType: string, pk: string, link: string): Promise<ObjectPage<T>> {\n");
    out.push_str("    return this.get<ObjectPage<T>>(`/objects/${objectType}/${pk}/links/${link}`);\n");
    out.push_str("  }\n");

    // objects.<Type>.all()/.filter()
    out.push_str("  objects = {\n");
    for t in types {
        let id = safe_ident(&t.api_name);
        out.push_str(&format!("    {}: {{\n", key(&t.api_name)));
        out.push_str(&format!("      all: (limit?: number) => this.load<{id}>({{ op: 'base', objectType: '{}' }}, limit),\n", t.api_name));
        out.push_str(&format!("      filter: (predicate: Predicate, limit?: number) => this.load<{id}>({{ op: 'filter', source: {{ op: 'base', objectType: '{}' }}, predicate }}, limit),\n", t.api_name));
        out.push_str("    },\n");
    }
    out.push_str("  };\n");

    // actions.<name>(params)
    out.push_str("  actions = {\n");
    for a in actions {
        out.push_str(&format!("    {}: (params: Record<string, unknown>, opts?: {{ dryRun?: boolean; subjects?: string[] }}) => this.post<{{ applied: number; status: string; logId: number }}>('/action-types/{}/execute', {{ params, ...(opts || {{}}) }}),\n", key(a), a));
    }
    out.push_str("  };\n");

    // functions.<name>(args)
    out.push_str("  functions = {\n");
    for f in functions {
        out.push_str(&format!("    {}: (args: Record<string, unknown> = {{}}) => this.post<{{ result: unknown }}>('/functions/{}/evaluate', {{ args }}),\n", key(f), f));
    }
    out.push_str("  };\n");
    out.push_str("}\n");
    out
}

/// apiName → 合法 TS 标识（非法字符转 _）。
fn safe_ident(name: &str) -> String {
    let mut s: String = name.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if s.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(true) {
        s.insert(0, '_');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::{PropertyTypeDef, TypeStatus};

    fn obj(api: &str, props: Vec<(&str, PropertyBaseType, bool)>) -> ObjectTypeDef {
        ObjectTypeDef {
            api_name: api.into(),
            display_name: api.into(),
            primary_key: "id".into(),
            status: TypeStatus::Active,
            properties: props.into_iter().map(|(n, t, req)| PropertyTypeDef { api_name: n.into(), base_type: t, required: req, ..Default::default() }).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn generates_interfaces_and_client() {
        let types = vec![
            obj("Customer", vec![("id", PropertyBaseType::String, true), ("amount", PropertyBaseType::Decimal, false), ("active", PropertyBaseType::Boolean, false)]),
        ];
        let ts = generate_typescript(&types, &[], &["reassignOrder".into()], &["discountRate".into()]);
        assert!(ts.contains("export interface Customer extends OntObject"));
        assert!(ts.contains("id: string;"));         // required → 无 ?
        assert!(ts.contains("amount?: number;"));     // decimal → number, 可选
        assert!(ts.contains("active?: boolean;"));
        assert!(ts.contains("Customer: {"));          // objects.Customer
        assert!(ts.contains("all: (limit"));
        assert!(ts.contains("reassignOrder: (params"));
        assert!(ts.contains("discountRate: (args"));
        assert!(ts.contains("searchAround<T"));
        assert!(ts.contains("class OntologyClient"));
    }

    #[test]
    fn illegal_property_names_quoted() {
        let types = vec![obj("T", vec![("weird-name", PropertyBaseType::String, false)])];
        let ts = generate_typescript(&types, &[], &[], &[]);
        assert!(ts.contains("\"weird-name\"?: string;"));
    }

    #[test]
    fn illegal_type_apiname_sanitized() {
        let types = vec![obj("qo_Ord", vec![("id", PropertyBaseType::String, true)])];
        let ts = generate_typescript(&types, &[], &[], &[]);
        assert!(ts.contains("export interface qo_Ord extends OntObject"));
    }
}
