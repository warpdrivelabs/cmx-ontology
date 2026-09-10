//! 对象集代数 → SQL 编译器（O2 的核心）。
//!
//! 把递归的 [`ObjectSet`] 编译为**一条** SQL：产出终端对象类型的 `pk` 集合（子查询/JOIN/集合运算），
//! 外层再从 `oo_<终端类型>` 取 `pk, title, props`。Search-Around 经单表 `ol_edge` JOIN 遍历，**不 N+1**。
//!
//! 安全：
//! - 对象类型名先经 [`safe_ident`] 校验（字母/下划线开头、仅字母数字下划线）——与建模层 apiName 规则
//!   一致，故表名 `oo_<type>` 拼接安全（非用户任意串）。
//! - 属性名同样校验后，以 `props ->> 'name'` 访问（jsonb 文本抽取），值走**参数绑定**（$N），杜绝注入。
//!
//! 产出 `(sql, params, terminal_type)`。params 为 [`DataValue`] 顺序列表。

use cmx_core::model::cell::DataValue;
use cmx_onto_model::objectset::*;
use cmx_onto_model::{LinkBacking, LinkEnd, LinkEnds, StoreError, StoreResult};
use serde_json::Value;
use std::collections::HashMap;

/// 校验并返回安全 SQL 标识符（对象类型 / 属性名）。非法即 Err（防注入纵深）。
pub fn safe_ident(s: &str) -> StoreResult<&str> {
    let ok = {
        let mut it = s.chars();
        matches!(it.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    if ok {
        Ok(s)
    } else {
        Err(StoreError::Backend(format!("非法标识符（防注入拒绝）: {s}")))
    }
}

/// 物化表名：`oo_<objectType>`。
pub fn object_table(object_type: &str) -> StoreResult<String> {
    Ok(format!("oo_{}", safe_ident(object_type)?))
}

/// 编译结果。
pub struct Compiled {
    /// 产出 `pk`（TEXT）单列的 SQL 子查询表达式（不含外层 SELECT *）。
    pub pk_sql: String,
    /// 绑定参数（顺序对应 SQL 内 $1..$n）。
    pub params: Vec<DataValue>,
    /// 终端对象类型（外层据此选 oo_<type> 取完整行）。
    pub terminal_type: String,
}

/// SearchAround 需预解析各关系两端（异步），故编译器吃一张 link→ends 映射（调用方先查好）。
pub struct Compiler<'a> {
    /// 关系 apiName → (A端类型, B端类型)。
    pub link_ends: &'a HashMap<String, LinkEnds>,
    /// 关系 apiName → 落存储方式（强类型）。`None` 或表中缺项 → 视为 [`LinkBacking::Edge`]。
    pub link_backing: Option<&'a HashMap<String, LinkBacking>>,
    next_param: usize,
}

impl<'a> Compiler<'a> {
    pub fn new(link_ends: &'a HashMap<String, LinkEnds>) -> Self {
        Self { link_ends, link_backing: None, next_param: 1 }
    }

    /// 带 backing 分派的构造（读侧 SearchAround 按 backing 编译 FK JOIN / ol_edge）。
    pub fn with_backing(
        link_ends: &'a HashMap<String, LinkEnds>,
        link_backing: &'a HashMap<String, LinkBacking>,
    ) -> Self {
        Self { link_ends, link_backing: Some(link_backing), next_param: 1 }
    }

    /// 取关系的 backing（缺省 Edge）。
    fn backing_of(&self, link: &str) -> LinkBacking {
        self.link_backing
            .and_then(|m| m.get(link))
            .cloned()
            .unwrap_or(LinkBacking::Edge)
    }

    /// 编译一个对象集为「产出 pk 的子查询」。
    pub fn compile(&mut self, set: &ObjectSet) -> StoreResult<Compiled> {
        let mut params: Vec<DataValue> = Vec::new();
        let (pk_sql, terminal) = self.emit(set, &mut params)?;
        Ok(Compiled { pk_sql, params, terminal_type: terminal })
    }

    /// 递归产出 (pk 子查询 SQL, 终端对象类型)。
    fn emit(&mut self, set: &ObjectSet, params: &mut Vec<DataValue>) -> StoreResult<(String, String)> {
        match set {
            ObjectSet::Base { object_type } => {
                let t = object_table(object_type)?;
                Ok((format!("SELECT pk FROM {t}"), object_type.clone()))
            }
            ObjectSet::Static { object_type, primary_keys } => {
                // 主键列表以 VALUES 绑定（每个走参数）。空集特判。
                let t = safe_ident(object_type)?.to_string();
                if primary_keys.is_empty() {
                    return Ok(("SELECT NULL::text AS pk WHERE false".into(), t));
                }
                let mut placeholders = Vec::new();
                for pk in primary_keys {
                    placeholders.push(format!("(${})", self.bind(params, DataValue::String(pk.clone()))));
                }
                // VALUES (..),(..) AS v(pk)
                Ok((
                    format!("SELECT pk FROM (VALUES {}) AS v(pk)", placeholders.join(",")),
                    t,
                ))
            }
            ObjectSet::Filter { source, predicate } => {
                let (inner, ty) = self.emit(source, params)?;
                // 过滤须在终端表上按属性谓词 → 用终端表 self-join 到 pk 集合。
                let t = object_table(&ty)?;
                let where_sql = self.pred_sql(predicate, params)?;
                Ok((
                    format!(
                        "SELECT o.pk FROM {t} o WHERE o.pk IN ({inner}) AND ({where_sql})"
                    ),
                    ty,
                ))
            }
            ObjectSet::SearchAround { source, link, direction } => {
                let (inner, _src_ty) = self.emit(source, params)?;
                let ends = self.link_ends.get(link).ok_or_else(|| {
                    StoreError::Backend(format!("关系类型 {link} 未定义，无法 Search-Around"))
                })?;
                // Forward：源在 A 端 → 终端 B 端；Reverse：源在 B 端 → 终端 A 端。
                let (terminal, src_end) = match direction {
                    LinkDirection::Forward => (ends.1.clone(), LinkEnd::A),
                    LinkDirection::Reverse => (ends.0.clone(), LinkEnd::B),
                };
                let sql = match self.backing_of(link) {
                    // 外键 backing：走对象表 FK 列 JOIN（不碰 ol_edge）。
                    LinkBacking::ForeignKey { property, side } => {
                        self.fk_search_around(&inner, ends, src_end, &property, side)?
                    }
                    // Edge / 暂未接的 JoinTable·Intermediary → 回退 ol_edge（保今日语义）。
                    _ => {
                        let link_lit = self.bind(params, DataValue::String(link.clone()));
                        let (from_col, to_col) = match direction {
                            LinkDirection::Forward => ("a_pk", "b_pk"),
                            LinkDirection::Reverse => ("b_pk", "a_pk"),
                        };
                        format!(
                            "SELECT DISTINCT e.{to_col} AS pk FROM ol_edge e \
                             WHERE e.link = ${link_lit} AND e.{from_col} IN ({inner})"
                        )
                    }
                };
                Ok((sql, terminal))
            }
            ObjectSet::Union { left, right } => self.set_op("UNION", left, right, params),
            ObjectSet::Intersect { left, right } => self.set_op("INTERSECT", left, right, params),
            ObjectSet::Subtract { left, right } => self.set_op("EXCEPT", left, right, params),
        }
    }

    /// ForeignKey backing 的 SearchAround → 对象表 FK 列 JOIN（不碰 ol_edge）。
    ///
    /// `ends`=(A类型,B类型)；`src_end`=源所在端；`property`=外键属性名；`side`=外键列所在端。
    /// 两种物理形态：
    /// - **FK 列在源端表**（`side == src_end`）：源表的 `props->>'property'` 即对端 pk → 直接取列值。
    /// - **FK 列在终端表**（`side != src_end`）：终端表 `props->>'property'` 指回源 pk → 取终端 pk where FK ∈ 源。
    fn fk_search_around(
        &self,
        inner: &str,
        ends: &LinkEnds,
        src_end: LinkEnd,
        property: &str,
        side: LinkEnd,
    ) -> StoreResult<String> {
        let prop = safe_ident(property)?;
        let (a_ty, b_ty) = ends;
        let (src_ty, terminal_ty) = match src_end {
            LinkEnd::A => (a_ty, b_ty),
            LinkEnd::B => (b_ty, a_ty),
        };
        let src_tbl = object_table(src_ty)?;
        let terminal_tbl = object_table(terminal_ty)?;
        if side == src_end {
            // FK 列在源端表：源表 property 列存对端 pk。取出去重、非空。
            Ok(format!(
                "SELECT DISTINCT s.props ->> '{prop}' AS pk FROM {src_tbl} s \
                 WHERE s.pk IN ({inner}) AND s.props ->> '{prop}' IS NOT NULL"
            ))
        } else {
            // FK 列在终端表：终端表 property 列指回源 pk。取终端 pk where FK ∈ 源集。
            Ok(format!(
                "SELECT DISTINCT t.pk AS pk FROM {terminal_tbl} t \
                 WHERE t.props ->> '{prop}' IN ({inner})"
            ))
        }
    }

    fn set_op(
        &mut self,
        op: &str,
        left: &ObjectSet,
        right: &ObjectSet,
        params: &mut Vec<DataValue>,
    ) -> StoreResult<(String, String)> {
        let (l, lt) = self.emit(left, params)?;
        let (r, rt) = self.emit(right, params)?;
        if lt != rt {
            return Err(StoreError::Backend(format!(
                "集合运算两侧对象类型不一致：{lt} vs {rt}"
            )));
        }
        Ok((format!("({l}) {op} ({r})"), lt))
    }

    /// 谓词 → SQL（属性经 `props ->> 'name'` 抽取文本；值参数绑定）。
    fn pred_sql(&mut self, p: &Predicate, params: &mut Vec<DataValue>) -> StoreResult<String> {
        Ok(match p {
            Predicate::Eq { property, value } => self.cmp(property, "=", value, params)?,
            Predicate::Ne { property, value } => self.cmp(property, "<>", value, params)?,
            Predicate::Gt { property, value } => self.cmp_num(property, ">", value, params)?,
            Predicate::Ge { property, value } => self.cmp_num(property, ">=", value, params)?,
            Predicate::Lt { property, value } => self.cmp_num(property, "<", value, params)?,
            Predicate::Le { property, value } => self.cmp_num(property, "<=", value, params)?,
            Predicate::In { property, values } => {
                let col = self.prop_text(property)?;
                if values.is_empty() {
                    return Ok("false".into());
                }
                let ph: Vec<String> = values
                    .iter()
                    .map(|v| format!("${}", self.bind(params, scalar_text(v))))
                    .collect();
                format!("{col} IN ({})", ph.join(","))
            }
            Predicate::Contains { property, value } => {
                let col = self.prop_text(property)?;
                let pat = format!("%{}%", value.replace('%', "\\%").replace('_', "\\_"));
                format!("{col} LIKE ${}", self.bind(params, DataValue::String(pat)))
            }
            Predicate::IsNull { property } => {
                let col = self.prop_text(property)?;
                format!("{col} IS NULL")
            }
            Predicate::And { predicates } => self.junction("AND", predicates, params)?,
            Predicate::Or { predicates } => self.junction("OR", predicates, params)?,
            Predicate::Not { predicate } => format!("NOT ({})", self.pred_sql(predicate, params)?),
        })
    }

    fn junction(
        &mut self,
        op: &str,
        preds: &[Predicate],
        params: &mut Vec<DataValue>,
    ) -> StoreResult<String> {
        if preds.is_empty() {
            return Ok(if op == "AND" { "true".into() } else { "false".into() });
        }
        let parts: Result<Vec<String>, _> =
            preds.iter().map(|p| self.pred_sql(p, params)).collect();
        Ok(format!("({})", parts?.join(&format!(" {op} "))))
    }

    /// 文本比较（=, <>, IN）：以 jsonb 文本抽取比较。
    fn cmp(
        &mut self,
        property: &str,
        op: &str,
        value: &Value,
        params: &mut Vec<DataValue>,
    ) -> StoreResult<String> {
        let col = self.prop_text(property)?;
        let idx = self.bind(params, scalar_text(value));
        Ok(format!("{col} {op} ${idx}"))
    }

    /// 数值比较（>, >=, <, <=）：抽取后 cast numeric（数值属性）。
    fn cmp_num(
        &mut self,
        property: &str,
        op: &str,
        value: &Value,
        params: &mut Vec<DataValue>,
    ) -> StoreResult<String> {
        let name = safe_ident(property)?;
        let idx = self.bind(params, scalar_text(value));
        // 数值比较：两侧都 cast 到 numeric。值以**文本**绑定（scalar_text），故 `$N::text::numeric`——
        // 若写 `($N)::numeric`，PG 会据 cast 目标把 $N 的参数类型推断为 numeric，而绑定层送的是
        // String → "cannot convert String and numeric" 500。经 ::text 中转让参数类型稳定为 text。
        Ok(format!(
            "(props ->> '{name}')::numeric {op} (${idx})::text::numeric"
        ))
    }

    /// 属性文本抽取表达式 `props ->> 'name'`（name 经标识符校验）。
    fn prop_text(&self, property: &str) -> StoreResult<String> {
        let name = safe_ident(property)?;
        Ok(format!("(props ->> '{name}')"))
    }

    /// 绑定一个参数，返回其位置索引（$N 的 N）。
    fn bind(&mut self, params: &mut Vec<DataValue>, v: DataValue) -> usize {
        params.push(v);
        let i = self.next_param;
        self.next_param += 1;
        i
    }
}

/// 标量 JSON → 文本 DataValue（用于与 `props ->>` 的文本比较）。
fn scalar_text(v: &Value) -> DataValue {
    match v {
        Value::String(s) => DataValue::String(s.clone()),
        Value::Bool(b) => DataValue::String(b.to_string()),
        Value::Number(n) => DataValue::String(n.to_string()),
        Value::Null => DataValue::String(String::new()),
        other => DataValue::String(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ends() -> HashMap<String, LinkEnds> {
        let mut m = HashMap::new();
        m.insert("customerPlacesOrder".into(), ("Customer".to_string(), "Order".to_string()));
        m
    }

    #[test]
    fn safe_ident_guards_injection() {
        assert!(safe_ident("Customer").is_ok());
        assert!(safe_ident("order_line").is_ok());
        assert!(safe_ident("Customer; DROP TABLE x").is_err());
        assert!(safe_ident("a'b").is_err());
        assert!(safe_ident("2bad").is_err());
    }

    #[test]
    fn compile_base() {
        let m = ends();
        let mut c = Compiler::new(&m);
        let out = c.compile(&ObjectSet::Base { object_type: "Customer".into() }).unwrap();
        assert_eq!(out.pk_sql, "SELECT pk FROM oo_Customer");
        assert_eq!(out.terminal_type, "Customer");
        assert!(out.params.is_empty());
    }

    #[test]
    fn compile_filter_binds_value() {
        let m = ends();
        let mut c = Compiler::new(&m);
        let set = ObjectSet::Filter {
            source: Box::new(ObjectSet::Base { object_type: "Customer".into() }),
            predicate: Predicate::Eq { property: "region".into(), value: json!("north") },
        };
        let out = c.compile(&set).unwrap();
        assert!(out.pk_sql.contains("props ->> 'region'"), "sql: {}", out.pk_sql);
        assert!(out.pk_sql.contains("IN (SELECT pk FROM oo_Customer)"));
        assert_eq!(out.params.len(), 1);
    }

    #[test]
    fn compile_search_around_forward() {
        let m = ends();
        let mut c = Compiler::new(&m);
        let set = ObjectSet::SearchAround {
            source: Box::new(ObjectSet::Base { object_type: "Customer".into() }),
            link: "customerPlacesOrder".into(),
            direction: LinkDirection::Forward,
        };
        let out = c.compile(&set).unwrap();
        assert_eq!(out.terminal_type, "Order"); // Forward → B 端
        assert!(out.pk_sql.contains("e.b_pk AS pk"), "sql: {}", out.pk_sql);
        assert!(out.pk_sql.contains("e.a_pk IN"));
        assert!(out.pk_sql.contains("ol_edge"));
    }

    #[test]
    fn compile_search_around_reverse() {
        let m = ends();
        let mut c = Compiler::new(&m);
        let set = ObjectSet::SearchAround {
            source: Box::new(ObjectSet::Base { object_type: "Order".into() }),
            link: "customerPlacesOrder".into(),
            direction: LinkDirection::Reverse,
        };
        let out = c.compile(&set).unwrap();
        assert_eq!(out.terminal_type, "Customer"); // Reverse → A 端
        assert!(out.pk_sql.contains("e.a_pk AS pk"));
    }

    // ───────── FK backing 编译分派（#5） ─────────

    /// customerPlacesOrder：A=Customer, B=Order。FK 列 customerId 落在 Order（B 端）指回 Customer.pk。
    fn fk_backing_b() -> HashMap<String, LinkBacking> {
        let mut b = HashMap::new();
        b.insert(
            "customerPlacesOrder".to_string(),
            LinkBacking::ForeignKey { property: "customerId".into(), side: LinkEnd::B },
        );
        b
    }

    #[test]
    fn fk_forward_column_on_terminal_side() {
        // Forward：源 Customer(A) → 终端 Order(B)；FK 列在终端(B)。
        // 取终端表 pk where props->>'customerId' ∈ 源集，绝不碰 ol_edge。
        let m = ends();
        let bk = fk_backing_b();
        let mut c = Compiler::with_backing(&m, &bk);
        let set = ObjectSet::SearchAround {
            source: Box::new(ObjectSet::Base { object_type: "Customer".into() }),
            link: "customerPlacesOrder".into(),
            direction: LinkDirection::Forward,
        };
        let out = c.compile(&set).unwrap();
        assert_eq!(out.terminal_type, "Order");
        assert!(!out.pk_sql.contains("ol_edge"), "FK 分派不应碰 ol_edge: {}", out.pk_sql);
        assert!(out.pk_sql.contains("oo_Order"), "sql: {}", out.pk_sql);
        assert!(out.pk_sql.contains("props ->> 'customerId' IN"), "sql: {}", out.pk_sql);
    }

    #[test]
    fn fk_reverse_column_on_source_side() {
        // Reverse：源 Order(B) → 终端 Customer(A)；FK 列在源(B)。
        // 直接取源表 props->>'customerId' 作终端 pk。
        let m = ends();
        let bk = fk_backing_b();
        let mut c = Compiler::with_backing(&m, &bk);
        let set = ObjectSet::SearchAround {
            source: Box::new(ObjectSet::Base { object_type: "Order".into() }),
            link: "customerPlacesOrder".into(),
            direction: LinkDirection::Reverse,
        };
        let out = c.compile(&set).unwrap();
        assert_eq!(out.terminal_type, "Customer");
        assert!(!out.pk_sql.contains("ol_edge"), "sql: {}", out.pk_sql);
        assert!(out.pk_sql.contains("oo_Order"), "源表取列: {}", out.pk_sql);
        assert!(out.pk_sql.contains("s.props ->> 'customerId' AS pk"), "sql: {}", out.pk_sql);
    }

    #[test]
    fn edge_backing_still_uses_ol_edge() {
        // 显式 Edge backing（或缺省）→ 仍走 ol_edge（不回归）。
        let m = ends();
        let mut bk = HashMap::new();
        bk.insert("customerPlacesOrder".to_string(), LinkBacking::Edge);
        let mut c = Compiler::with_backing(&m, &bk);
        let set = ObjectSet::SearchAround {
            source: Box::new(ObjectSet::Base { object_type: "Customer".into() }),
            link: "customerPlacesOrder".into(),
            direction: LinkDirection::Forward,
        };
        let out = c.compile(&set).unwrap();
        assert!(out.pk_sql.contains("ol_edge"), "Edge 应走 ol_edge: {}", out.pk_sql);
    }

    #[test]
    fn fk_injection_guarded_on_property() {
        // FK property 经 safe_ident 校验：非法列名（注入企图）编译期拒。
        let m = ends();
        let mut bk = HashMap::new();
        bk.insert(
            "customerPlacesOrder".to_string(),
            LinkBacking::ForeignKey { property: "x; DROP TABLE oo_Order".into(), side: LinkEnd::B },
        );
        let mut c = Compiler::with_backing(&m, &bk);
        let set = ObjectSet::SearchAround {
            source: Box::new(ObjectSet::Base { object_type: "Customer".into() }),
            link: "customerPlacesOrder".into(),
            direction: LinkDirection::Forward,
        };
        assert!(c.compile(&set).is_err());
    }

    #[test]
    fn compile_set_ops_type_check() {
        let m = ends();
        let mut c = Compiler::new(&m);
        // 同类型 union ok
        let ok = ObjectSet::Union {
            left: Box::new(ObjectSet::Base { object_type: "Customer".into() }),
            right: Box::new(ObjectSet::Base { object_type: "Customer".into() }),
        };
        assert!(c.compile(&ok).is_ok());
        // 异类型 union 报错
        let mut c2 = Compiler::new(&m);
        let bad = ObjectSet::Union {
            left: Box::new(ObjectSet::Base { object_type: "Customer".into() }),
            right: Box::new(ObjectSet::Base { object_type: "Order".into() }),
        };
        assert!(c2.compile(&bad).is_err());
    }

    #[test]
    fn compile_three_hop_single_sql() {
        // Customer --places--> Order，再 filter：验证嵌套编译成单条（含子查询）。
        let m = ends();
        let mut c = Compiler::new(&m);
        let set = ObjectSet::Filter {
            source: Box::new(ObjectSet::SearchAround {
                source: Box::new(ObjectSet::Filter {
                    source: Box::new(ObjectSet::Base { object_type: "Customer".into() }),
                    predicate: Predicate::Eq { property: "region".into(), value: json!("north") },
                }),
                link: "customerPlacesOrder".into(),
                direction: LinkDirection::Forward,
            }),
            predicate: Predicate::Ge { property: "amount".into(), value: json!(1000) },
        };
        let out = c.compile(&set).unwrap();
        assert_eq!(out.terminal_type, "Order");
        // 一条 SQL：含两处 props 抽取 + edge join。参数 3 个：north、link 名、1000。
        assert_eq!(out.params.len(), 3);
        assert!(out.pk_sql.contains("ol_edge"));
        assert!(out.pk_sql.contains("(props ->> 'amount')::numeric"));
    }
}
