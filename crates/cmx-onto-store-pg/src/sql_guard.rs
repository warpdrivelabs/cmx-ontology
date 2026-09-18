//! M0 S2 安全止血：漏斗 `sourceQuery` 治理三件套的**静态校验件**（方案 20260918 §5.8.5）。
//!
//! sourceQuery 是全平台唯一非参数化查询面（任意 SQL 直连源库）。三件套 = ① 静态校验（本模块，
//! sqlparser 判定**单语句纯 SELECT**：拒绝多语句 / DML·DDL / CTE 写副作用 / 注释绕过——A-P2-8 /
//! B-P3-1）＋ ② 执行期 `BEGIN READ ONLY` + `SET LOCAL statement_timeout`（funnel_store 内，
//! 挡静态校验拦不住的 pg_sleep 类危险函数）＋ ③ 生成式默认路径（未手写 SQL 时由 resource+映射
//! 自动生成参数化 SELECT，本模块 [`generate_select`]）。
//!
//! 白名单语义：`SELECT`（含 CTE、子查询、UNION）放行；其余一律拒绝——fail-closed。

use cmx_onto_model::{SourceMapping, StoreError, StoreResult};
use sqlparser::ast::{Statement, TableFactor};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;

/// 校验一段 sourceQuery：必须解析成功、恰好一条语句、且为纯查询（SELECT / VALUES / 表函数）。
///
/// 拒绝（各自独立防线，注释绕过在解析层天然失效——注释不参与 AST）：
/// - 多语句（`;` 分隔的第二条语句）；
/// - 非 Query 语句（INSERT/UPDATE/DELETE/DDL/SET/CALL…）；
/// - CTE 内含写操作（`WITH w AS (INSERT …)` 的 data-modifying CTE）；
/// - 存储过程调用形态（CALL / SELECT func() 本身放行——只读视角由 ② READ ONLY 兜底）。
pub fn ensure_readonly_select(sql: &str) -> StoreResult<()> {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return Err(StoreError::Backend("sourceQuery 为空".into()));
    }
    let dialect = PostgreSqlDialect {};
    let statements = Parser::parse_sql(&dialect, trimmed)
        .map_err(|e| StoreError::Backend(format!("sourceQuery 解析失败（仅支持合法 SQL）: {e}")))?;
    if statements.len() != 1 {
        return Err(StoreError::Backend(format!(
            "sourceQuery 须为单条语句（检测到 {} 条；多语句拒绝）",
            statements.len()
        )));
    }
    match &statements[0] {
        Statement::Query(q) => {
            // data-modifying CTE：`WITH … AS (INSERT/UPDATE …)`——sqlparser 0.49 以
            // SetExpr::Insert/Update 承载（其余变体只可能是查询形态）。
            if let Some(w) = &q.with {
                for cte in &w.cte_tables {
                    if !matches!(
                        *cte.query.body,
                        sqlparser::ast::SetExpr::Select(_)
                            | sqlparser::ast::SetExpr::Query(_)
                            | sqlparser::ast::SetExpr::Values(_)
                    ) {
                        return Err(StoreError::Backend(
                            "sourceQuery 拒绝：CTE 内含写操作（data-modifying CTE）".into(),
                        ));
                    }
                }
            }
            Ok(())
        }
        other => Err(StoreError::Backend(format!(
            "sourceQuery 拒绝：仅支持只读 SELECT（收到非查询语句：{}）",
            statement_kind(other)
        ))),
    }
}

/// 语句类型名（报错可读性；只取首词，不回显完整语句防数据泄漏）。
fn statement_kind(s: &Statement) -> String {
    let rendered = s.to_string();
    rendered
        .split_whitespace()
        .next()
        .unwrap_or("非查询语句")
        .to_ascii_uppercase()
}

/// 校验 PG 资源名：恰一段点号（`schema.table`）或单段（`table`）；各段过标识符白名单（A-P2-2）。
///
/// 返回规范化形态（trim 后原样；供 SQL 拼接前最后一道闸）。
pub fn safe_qualified_table(resource: &str) -> StoreResult<String> {
    let r = resource.trim();
    if r.is_empty() {
        return Err(StoreError::Backend("resource 为空".into()));
    }
    let seg_ok = |s: &str| {
        let mut it = s.chars();
        matches!(it.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    let parts: Vec<&str> = r.split('.').collect();
    let ok = match parts.len() {
        1 => seg_ok(parts[0]),
        2 => seg_ok(parts[0]) && seg_ok(parts[1]),
        _ => false,
    };
    if !ok {
        return Err(StoreError::Backend(format!(
            "resource「{resource}」非法（须 table 或 schema.table，标识符白名单；防注入拒绝）"
        )));
    }
    Ok(r.to_string())
}

/// 生成式默认路径（M0 ③）：由 resource + 字段映射生成参数化 SELECT（B-P2-3）。
///
/// 形态：`SELECT key, [title,] mapped… FROM resource`——列名经标识符校验，值零拼接（无 WHERE，
/// 全量读源语义与手写 `SELECT *` 一致；漏斗映射/下推谓词在执行侧叠加）。手写 SQL 仍是高级模式
/// 覆盖项（sourceQuery 非空时优先，但须过 [`ensure_readonly_select`]）。
pub fn generate_select(m: &SourceMapping) -> StoreResult<String> {
    let table = safe_qualified_table(
        m.resource.as_deref().ok_or_else(|| {
            StoreError::Backend("生成式查询须提供 resource（源表名）".into())
        })?,
    )?;
    let mut cols: Vec<String> = Vec::new();
    let ident = |s: &str| -> StoreResult<String> {
        let t = s.trim();
        let ok = !t.is_empty()
            && t.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if ok {
            Ok(t.to_string())
        } else {
            Err(StoreError::Backend(format!("映射列名「{s}」非法（标识符白名单）")))
        }
    };
    for k in &m.key_columns {
        cols.push(ident(k)?);
    }
    if let Some(t) = &m.title_column {
        let t = ident(t)?;
        if !cols.contains(&t) {
            cols.push(t);
        }
    }
    for (src, _) in &m.property_map {
        let s = ident(src)?;
        if !cols.contains(&s) {
            cols.push(s);
        }
    }
    if cols.is_empty() {
        return Ok(format!("SELECT * FROM {table}"));
    }
    cols.dedup();
    Ok(format!("SELECT {} FROM {}", cols.join(", "), table))
}

/// 生效的读源 SQL：sourceQuery 非空 → 校验后使用（高级模式）；为空 → [`generate_select`]。
pub fn effective_source_sql(m: &SourceMapping) -> StoreResult<String> {
    let q = m.source_query.trim();
    if !q.is_empty() {
        ensure_readonly_select(q)?;
        return Ok(q.to_string());
    }
    generate_select(m)
}

/// 提取一条 SELECT 的全部被查表名（探测 / 运维诊断用；不参与安全判定）。
pub fn tables_of(sql: &str) -> Vec<String> {
    let dialect = PostgreSqlDialect {};
    let Ok(statements) = Parser::parse_sql(&dialect, sql) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for st in statements {
        if let Statement::Query(q) = st {
            collect_tables(&q.body, &mut out);
        }
    }
    out
}

fn collect_tables(body: &sqlparser::ast::SetExpr, out: &mut Vec<String>) {
    match body {
        sqlparser::ast::SetExpr::Select(sel) => {
            for from in &sel.from {
                collect_factor(&from.relation, out);
                for join in &from.joins {
                    collect_factor(&join.relation, out);
                }
            }
        }
        sqlparser::ast::SetExpr::Query(q) => collect_tables(&q.body, out),
        sqlparser::ast::SetExpr::SetOperation { left, right, .. } => {
            collect_tables(left, out);
            collect_tables(right, out);
        }
        _ => {}
    }
}

fn collect_factor(f: &TableFactor, out: &mut Vec<String>) {
    if let TableFactor::Table { name, .. } = f {
        out.push(name.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_select_ok() {
        assert!(ensure_readonly_select("SELECT a, b FROM t").is_ok());
        assert!(ensure_readonly_select("  select * from s.t where x = 1 ").is_ok());
        assert!(ensure_readonly_select("SELECT 1 UNION SELECT 2").is_ok());
        assert!(ensure_readonly_select("WITH w AS (SELECT 1) SELECT * FROM w").is_ok());
        // 行注释 / 块注释不参与判定（AST 层天然剥除，注释绕过无效）
        assert!(ensure_readonly_select("SELECT a -- drop table x\nFROM t").is_ok());
        assert!(ensure_readonly_select("VALUES (1),(2)").is_ok()); // 裸 VALUES 解析为只读行集，放行
    }

    #[test]
    fn dml_ddl_multi_statement_rejected() {
        assert!(ensure_readonly_select("DELETE FROM t").is_err());
        assert!(ensure_readonly_select("INSERT INTO t VALUES (1)").is_err());
        assert!(ensure_readonly_select("UPDATE t SET a=1").is_err());
        assert!(ensure_readonly_select("DROP TABLE t").is_err());
        assert!(ensure_readonly_select("TRUNCATE t").is_err());
        assert!(ensure_readonly_select("SELECT 1; DELETE FROM t").is_err()); // 多语句
        assert!(ensure_readonly_select("SELECT 1; SELECT 2").is_err());
        assert!(ensure_readonly_select("WITH w AS (INSERT INTO t VALUES(1) RETURNING *) SELECT * FROM w").is_err());
        assert!(ensure_readonly_select("").is_err());
        assert!(ensure_readonly_select("not even sql~~~").is_err());
    }

    #[test]
    fn qualified_table_guard() {
        assert_eq!(safe_qualified_table("src_cust").unwrap(), "src_cust");
        assert_eq!(safe_qualified_table(" fico.src_cust ").unwrap(), "fico.src_cust");
        assert!(safe_qualified_table("a.b.c").is_err());
        assert!(safe_qualified_table("t; DROP TABLE x").is_err());
        assert!(safe_qualified_table("2t").is_err());
        assert!(safe_qualified_table("").is_err());
    }

    fn m(resource: &str, keys: &[&str], title: Option<&str>, pm: &[(&str, &str)]) -> SourceMapping {
        SourceMapping {
            object_type: "T".into(),
            resource: Some(resource.into()),
            key_columns: keys.iter().map(|s| s.to_string()).collect(),
            title_column: title.map(|s| s.to_string()),
            property_map: pm.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn generated_select_shape() {
        let m1 = m("fico.src_cust", &["cust_id"], Some("cust_name"), &[("cust_name", "name"), ("region_code", "region")]);
        let sql = generate_select(&m1).unwrap();
        assert_eq!(sql, "SELECT cust_id, cust_name, region_code FROM fico.src_cust");
        // 坏列名拒绝
        let bad = m("t", &["k; drop"], None, &[]);
        assert!(generate_select(&bad).is_err());
        // 缺 resource 拒绝
        let mut n = m("t", &["k"], None, &[]);
        n.resource = None;
        assert!(generate_select(&n).is_err());
    }

    #[test]
    fn effective_sql_prefers_handwritten_but_guards() {
        let mut m = m("t", &["k"], None, &[]);
        // 空 → 生成式
        assert_eq!(effective_source_sql(&m).unwrap(), "SELECT k FROM t");
        // 手写合法 → 原样
        m.source_query = "SELECT * FROM t WHERE k > 10".into();
        assert_eq!(effective_source_sql(&m).unwrap(), "SELECT * FROM t WHERE k > 10");
        // 手写非法 → 拒绝
        m.source_query = "DELETE FROM t".into();
        assert!(effective_source_sql(&m).is_err());
    }

    #[test]
    fn tables_extracted() {
        assert_eq!(tables_of("SELECT * FROM a JOIN b ON a.x = b.x"), vec!["a", "b"]);
    }
}
