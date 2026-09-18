//! PgDirectBackend —— PG 直连虚拟直查（方案 20260918 §5.3；D2 仅 PG）。
//!
//! 按 `om_source_mapping`（mode=virtual）把 `Base/Filter/Static` 子树编译为**源库 SQL** 下推：
//! `SELECT key AS pk, title AS title, … FROM <resource> WHERE <下推> ORDER BY title, pk LIMIT/OFFSET`
//! （固定排序口径对齐物化路径 E4：`ORDER BY title, pk`）。SearchAround/集合运算**不在本实现内**——
//! 由 app 层分派器桥接（pk 集合语义），本实现吃到的子树恒为 Base/Filter/Static。
//!
//! 复用边界（A-P2-1 纠正）：`safe_ident`/`safe_qualified_table` 白名单与 `$N` 参数绑定框架沿用
//! compile.rs 思路；**谓词到物理列的编译为本实现新写**——源表是物理类型列（int4/timestamptz/bool），
//! 照搬物化路径的 `props->>'x'::text::numeric` 双跳 cast 会 SQL 报错或语义漂移。
//!
//! 安全 / 正确性口径：
//! - fail-closed：谓词引用**未映射属性**或**非标量基型**、pk 超单列、算子不受支持 → 整查询拒绝，
//!   绝不静默全量拉取（R1）；
//! - 行级权限残差已由 PEP 折入对象集 Filter（residual_set）——随本编译自然下推（E7 同理），
//!   残差引用未映射属性 → 编译失败 → 整查询拒绝（R3）；
//! - 全部查询在 `READ ONLY` 事务 + `statement_timeout` 内执行（与漏斗 M0 同款执行期闸）；
//! - resource 过 `safe_qualified_table`（恰一段点号 + 标识符白名单，A-P2-2）；
//! - 每次查询落结构化日志（源/类型/SQL/行数/耗时），>2s 告警（§5.9 观测）。

use async_trait::async_trait;
use cmx_core::model::cell::DataValue;
use cmx_core::model::data::dataset::{DataSet, Row, Schema};
use cmx_database_pg::{execute_sql_with_params, get_default_pg_db_manager, query_sql_with_params, SqlParams};
use cmx_onto_model::backend::{BackendCaps, BackendCtx, BackendKind, ObjectDataBackend, PK_BRIDGE_MAX};
use cmx_onto_model::def::PropertyBaseType;
use cmx_onto_model::objectset::{Aggregation, ObjectPage, ObjectRecord, ObjectSet, Page, Predicate};
use cmx_onto_model::{ProbeReport, StoreError, StoreResult};
use serde_json::{json, Map, Value};

use crate::compile::safe_ident;
use crate::sql_guard::safe_qualified_table;

/// 虚拟查询语句超时秒数（env `ONTO_SRC_QUERY_TIMEOUT_SECS` 可覆盖；与漏斗同键同缺省）。
fn src_timeout_secs() -> u64 {
    std::env::var("ONTO_SRC_QUERY_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(30)
}

/// 虚拟直查后端（无状态：连接走 db_id 寻址的共享池；集群合规，无进程内业务缓存）。
#[derive(Default)]
pub struct PgDirectBackend;

impl PgDirectBackend {
    pub fn new() -> Self {
        Self
    }

    /// 映射主键源列（M1 虚拟直查限定单列：pk 桥接 `IN` 语义 + 固定排序都需要单列锚）。
    fn key_col(ctx: &BackendCtx) -> StoreResult<String> {
        if ctx.mapping.key_columns.len() != 1 {
            return Err(StoreError::Backend(format!(
                "虚拟直查要求单列主键映射（当前 {} 列）；请在绑定中仅保留一个 keyColumn",
                ctx.mapping.key_columns.len()
            )));
        }
        Ok(safe_ident(ctx.mapping.key_columns[0].trim())?.to_string())
    }

    /// 属性 → 源列映射（未映射即 Err：fail-closed，不静默丢谓词）。
    fn col_of(ctx: &BackendCtx, property: &str) -> StoreResult<String> {
        let (col, _) = ctx
            .mapping
            .property_map
            .iter()
            .find(|(_, p)| p == property)
            .ok_or_else(|| {
                StoreError::Backend(format!(
                    "属性「{property}」未映射到源列：虚拟直查仅支持映射列参与过滤（未映射属性请走物化模式）"
                ))
            })?;
        Ok(safe_ident(col.trim())?.to_string())
    }

    /// 属性基型（定义缺失视作 String——bind 强校验已保证映射属性在定义中）。
    fn base_type_of(ctx: &BackendCtx, property: &str) -> PropertyBaseType {
        ctx.def
            .properties
            .iter()
            .find(|p| p.api_name == property)
            .map(|p| p.base_type)
            .unwrap_or(PropertyBaseType::String)
    }

    /// 标题源列表达式（可空 → 兜底 pk，沿袭 funnel 惯例 B-P2-2）。
    fn title_expr(ctx: &BackendCtx, key_col: &str) -> StoreResult<String> {
        Ok(match &ctx.mapping.title_column {
            Some(t) if !t.trim().is_empty() => {
                format!("COALESCE({}::text, {key_col}::text)", safe_ident(t.trim())?)
            }
            _ => format!("{key_col}::text"),
        })
    }
}

/// 子树编译产物：FROM + WHERE（"true" 恒真兜底）+ 顺序参数。
struct PgSelect {
    from: String,
    where_sql: String,
    key_col: String,
    title_sql: String,
}

/// 数值家族基型判定（原生数值列谓词，无 ::text 中转）。
fn is_numeric(bt: PropertyBaseType) -> bool {
    matches!(
        bt,
        PropertyBaseType::Integer | PropertyBaseType::Long | PropertyBaseType::Double | PropertyBaseType::Decimal
    )
}

/// 可下推基型白名单（M1 仅标量；Array/Struct/Attachment 等复合类型参与过滤 → 整查询拒绝）。
fn pushdown_allowed(bt: PropertyBaseType) -> bool {
    matches!(
        bt,
        PropertyBaseType::String
            | PropertyBaseType::Integer
            | PropertyBaseType::Long
            | PropertyBaseType::Double
            | PropertyBaseType::Decimal
            | PropertyBaseType::Boolean
            | PropertyBaseType::Date
            | PropertyBaseType::Timestamp
            | PropertyBaseType::Geohash
    )
}

/// JSON 标量 → 按基型的绑定值（类型不符 → Err：fail-closed，不做隐式文本比较的语义漂移）。
fn bind_value(bt: PropertyBaseType, v: &Value) -> StoreResult<DataValue> {
    let scalar = |s: String| Ok(DataValue::String(s));
    match bt {
        PropertyBaseType::String | PropertyBaseType::Geohash => match v {
            Value::String(s) => scalar(s.clone()),
            Value::Number(n) => scalar(n.to_string()),
            Value::Bool(b) => scalar(b.to_string()),
            _ => Err(StoreError::Backend("过滤值须为文本标量".into())),
        },
        // 数值家族统一以文本绑定：源列类型谱系宽（int4/numeric/float8…），绑定层逐类型匹配
        // 易碎（f64 vs numeric）；与 compile.rs 相同口径——`$N::text::numeric` 显式转让参数稳定为 text。
        PropertyBaseType::Integer | PropertyBaseType::Long | PropertyBaseType::Double
        | PropertyBaseType::Decimal => {
            let ok = match v {
                Value::Number(_) => true,
                Value::String(s) => s.trim().parse::<f64>().is_ok(),
                _ => false,
            };
            if !ok {
                return Err(StoreError::Backend(format!("过滤值「{v}」须为数值（源列为数值型）")));
            }
            Ok(DataValue::String(match v {
                Value::Number(n) => n.to_string(),
                Value::String(s) => s.trim().to_string(),
                _ => unreachable!("已校验"),
            }))
        }
        PropertyBaseType::Boolean => match v {
            Value::Bool(b) => Ok(DataValue::Bool(*b)),
            Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "1" => Ok(DataValue::Bool(true)),
                "false" | "0" => Ok(DataValue::Bool(false)),
                _ => Err(StoreError::Backend("过滤值须为布尔".into())),
            },
            _ => Err(StoreError::Backend("过滤值须为布尔".into())),
        },
        PropertyBaseType::Date => {
            let s = v.as_str().unwrap_or_default().trim();
            let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map_err(|_| StoreError::Backend(format!("过滤值「{s}」须为 YYYY-MM-DD 日期")))?;
            Ok(DataValue::Date(d))
        }
        PropertyBaseType::Timestamp => {
            let s = v.as_str().unwrap_or_default().trim();
            let t = chrono::DateTime::parse_from_rfc3339(s)
                .map(|d| d.with_timezone(&chrono::Utc))
                .map_err(|_| StoreError::Backend(format!("过滤值「{s}」须为 RFC3339 时刻")))?;
            Ok(DataValue::DateTime(t))
        }
        _ => Err(StoreError::Backend("该属性基型不支持过滤".into())),
    }
}

/// 谓词编译器（物理列版；属性/列已校验，值全参数绑定）。
struct PushdownCompiler<'a> {
    ctx: &'a BackendCtx,
    params: &'a mut Vec<DataValue>,
}

impl<'a> PushdownCompiler<'a> {
    /// 属性的可比较 SQL 表达式：文本家族 `col::text`，数值/时间/布尔原生列。
    fn col_expr(&self, property: &str) -> StoreResult<String> {
        let col = PgDirectBackend::col_of(self.ctx, property)?;
        let bt = PgDirectBackend::base_type_of(self.ctx, property);
        if !pushdown_allowed(bt) {
            return Err(StoreError::Backend(format!(
                "属性「{property}」基型 {bt:?} 非可下推标量：虚拟直查仅支持标量基型参与过滤"
            )));
        }
        // 数值列统一 ::numeric（int4/int8/numeric/float8 均可安全 cast，与文本/时间绑定层解耦）。
        Ok(if is_numeric(bt) {
            format!("{col}::numeric")
        } else if matches!(bt, PropertyBaseType::Boolean | PropertyBaseType::Date | PropertyBaseType::Timestamp) {
            col
        } else {
            format!("{col}::text")
        })
    }

    fn bind(&mut self, v: DataValue) -> usize {
        self.params.push(v);
        self.params.len()
    }

    fn pred(&mut self, p: &Predicate) -> StoreResult<String> {
        Ok(match p {
            Predicate::Eq { property, value } => {
                let bt = PgDirectBackend::base_type_of(self.ctx, property);
                let idx = self.bind(bind_value(bt, value)?);
                if is_numeric(bt) {
                    format!("{} = (${idx})::text::numeric", self.col_expr(property)?)
                } else {
                    format!("{} = ${idx}", self.col_expr(property)?)
                }
            }
            Predicate::Ne { property, value } => {
                let bt = PgDirectBackend::base_type_of(self.ctx, property);
                let idx = self.bind(bind_value(bt, value)?);
                if is_numeric(bt) {
                    format!("{} <> (${idx})::text::numeric", self.col_expr(property)?)
                } else {
                    format!("{} <> ${idx}", self.col_expr(property)?)
                }
            }
            Predicate::Gt { property, value } => self.cmp_ord(property, value, ">")?,
            Predicate::Ge { property, value } => self.cmp_ord(property, value, ">=")?,
            Predicate::Lt { property, value } => self.cmp_ord(property, value, "<")?,
            Predicate::Le { property, value } => self.cmp_ord(property, value, "<=")?,
            Predicate::In { property, values } => {
                if values.is_empty() {
                    return Ok("false".into());
                }
                let bt = PgDirectBackend::base_type_of(self.ctx, property);
                let expr = self.col_expr(property)?;
                let mut ph = Vec::with_capacity(values.len());
                for v in values {
                    let i = self.bind(bind_value(bt, v)?);
                    ph.push(if is_numeric(bt) { format!("(${i})::text::numeric") } else { format!("${i}") });
                }
                format!("{expr} IN ({})", ph.join(", "))
            }
            Predicate::Contains { property, value } => {
                // 文本包含：恒走 ::text（数值列同样语义成立），转义 %/_ 后参数绑定。
                let col = PgDirectBackend::col_of(self.ctx, property)?;
                let pat = format!("%{}%", value.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
                let idx = self.bind(DataValue::String(pat));
                format!("{col}::text LIKE ${idx}")
            }
            Predicate::IsNull { property } => {
                let col = PgDirectBackend::col_of(self.ctx, property)?;
                format!("{col} IS NULL")
            }
            Predicate::And { predicates } => self.junction("AND", predicates)?,
            Predicate::Or { predicates } => self.junction("OR", predicates)?,
            Predicate::Not { predicate } => format!("NOT ({})", self.pred(predicate)?),
        })
    }

    fn cmp_ord(&mut self, property: &str, value: &Value, op: &str) -> StoreResult<String> {
        let bt = PgDirectBackend::base_type_of(self.ctx, property);
        let idx = self.bind(bind_value(bt, value)?);
        if is_numeric(bt) {
            // 值以文本绑定 → 显式转让 numeric（参数类型稳定为 text，compile.rs 同款防 500）。
            Ok(format!("{} {op} (${idx})::text::numeric", self.col_expr(property)?))
        } else {
            Ok(format!("{} {op} ${idx}", self.col_expr(property)?))
        }
    }

    fn junction(&mut self, op: &str, preds: &[Predicate]) -> StoreResult<String> {
        if preds.is_empty() {
            return Ok(if op == "AND" { "true".into() } else { "false".into() });
        }
        let parts: Result<Vec<String>, _> = preds.iter().map(|p| self.pred(p)).collect();
        Ok(format!("({})", parts?.join(&format!(" {op} "))))
    }
}

/// 编译 `Base/Filter/Static` 子树 → FROM + WHERE（fail-closed：越界算子 / 类型不一致 / 未映射属性拒绝）。
fn compile_subtree(ctx: &BackendCtx, set: &ObjectSet, params: &mut Vec<DataValue>) -> StoreResult<PgSelect> {
    let from = safe_qualified_table(
        ctx.mapping.resource.as_deref().ok_or_else(|| {
            StoreError::Backend("虚拟绑定缺 resource（源表名）；请重新绑定并补齐".into())
        })?,
    )?;
    let key_col = PgDirectBackend::key_col(ctx)?;
    let title_sql = PgDirectBackend::title_expr(ctx, &key_col)?;
    let where_sql = compile_where(ctx, set, params, &key_col)?;
    Ok(PgSelect { from, where_sql, key_col, title_sql })
}

fn compile_where(
    ctx: &BackendCtx,
    set: &ObjectSet,
    params: &mut Vec<DataValue>,
    key_col: &str,
) -> StoreResult<String> {
    match set {
        ObjectSet::Base { object_type } => {
            ensure_terminal(ctx, object_type)?;
            Ok("true".into())
        }
        ObjectSet::Static { object_type, primary_keys } => {
            ensure_terminal(ctx, object_type)?;
            if primary_keys.is_empty() {
                return Ok("false".into());
            }
            // pk 集以 VALUES 列表绑定（与物化编译器 Static 形态一致；PG 参数上限充裕）。
            let ph: Vec<String> = primary_keys
                .iter()
                .map(|pk| {
                    params.push(DataValue::String(pk.clone()));
                    format!("(${})", params.len())
                })
                .collect();
            Ok(format!("{key_col} IN (SELECT pk FROM (VALUES {}) AS v(pk))", ph.join(",")))
        }
        ObjectSet::Filter { source, predicate } => {
            let inner = compile_where(ctx, source, params, key_col)?;
            let mut c = PushdownCompiler { ctx, params };
            let p = c.pred(predicate)?;
            if inner == "true" {
                Ok(p)
            } else {
                Ok(format!("({inner}) AND ({p})"))
            }
        }
        other => Err(StoreError::Backend(format!(
            "虚拟直查仅支持 Base/Filter/Static 子树（收到 {:?}）；关系遍历/集合运算由平台桥接，不应直达后端",
            other
        ))),
    }
}

fn ensure_terminal(ctx: &BackendCtx, object_type: &str) -> StoreResult<()> {
    if object_type != ctx.def.api_name {
        return Err(StoreError::Backend(format!(
            "对象集终端类型 {object_type} 与绑定类型 {} 不一致",
            ctx.def.api_name
        )));
    }
    Ok(())
}

/// 只读事务 + 语句超时内执行源库查询（M0 执行期闸同款；fail-closed）。
async fn query_source_readonly(
    source_db: &str,
    sql: &str,
    params: Vec<DataValue>,
    ds_id: &str,
) -> StoreResult<DataSet> {
    let manager = get_default_pg_db_manager();
    let txn_ctx = manager.get_transaction_context();
    let txn = txn_ctx
        .begin(source_db)
        .await
        .map_err(|e| StoreError::Backend(format!("开启虚拟查询事务失败: {e}")))?;
    if let Err(e) =
        execute_sql_with_params(source_db, Some(&txn), "SET TRANSACTION READ ONLY", SqlParams::DataValues(vec![]))
            .await
    {
        let _ = txn_ctx.rollback(&txn).await;
        return Err(StoreError::Backend(format!("设置只读事务失败: {e}")));
    }
    if let Err(e) = execute_sql_with_params(
        source_db,
        Some(&txn),
        &format!("SET LOCAL statement_timeout = '{}s'", src_timeout_secs()),
        SqlParams::DataValues(vec![]),
    )
    .await
    {
        let _ = txn_ctx.rollback(&txn).await;
        return Err(StoreError::Backend(format!("设置语句超时失败: {e}")));
    }
    let started = std::time::Instant::now();
    let out = query_sql_with_params(source_db, Some(&txn), sql, SqlParams::DataValues(params), ds_id).await;
    if out.is_err() {
        let _ = txn_ctx.rollback(&txn).await;
    } else {
        let _ = txn_ctx.commit(&txn).await;
    }
    let elapsed = started.elapsed();
    let warn = elapsed.as_secs_f64() > 2.0;
    match out {
        Ok(ds) => {
            let n: usize = ds.iter().count();
            tracing::info!(
                source = %source_db, sql = %sql, rows = n,
                elapsed_ms = elapsed.as_millis() as u64,
                "虚拟直查下推完成"
            );
            if warn {
                tracing::warn!(source = %source_db, elapsed_ms = elapsed.as_millis() as u64, "虚拟直查慢查询（>2s）");
            }
            Ok(ds)
        }
        Err(e) => {
            tracing::warn!(source = %source_db, sql = %sql, error = %e, "虚拟直查下推失败");
            Err(StoreError::Backend(format!("虚拟查询执行失败: {e}")))
        }
    }
}

/// 行取值助手（列名直取文本 / JSON 值）。
fn col_json(row: &Row, schema: &Schema, col: &str) -> Value {
    match row.get_by_name(schema, col) {
        Some(v) => crate::funnel_store::datavalue_to_json(v),
        None => Value::Null,
    }
}

fn col_text(row: &Row, schema: &Schema, col: &str) -> String {
    crate::object_store::row_text(row, schema, col)
}

#[async_trait]
impl ObjectDataBackend for PgDirectBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::PgDirect
    }

    fn caps(&self) -> BackendCaps {
        BackendCaps::pg_direct()
    }

    /// 一页：`SELECT key AS pk, title AS title, <mapped cols> FROM <resource> WHERE <下推>
    /// ORDER BY title, pk LIMIT OFFSET`（E4 固定排序）。
    async fn load(
        &self,
        ctx: &BackendCtx,
        set: &ObjectSet,
        page: &Page,
    ) -> StoreResult<ObjectPage> {
        let limit = page.limit.clamp(1, 1000);
        let mut params: Vec<DataValue> = Vec::new();
        let sel = compile_subtree(ctx, set, &mut params)?;
        // 投影：pk + title + 全部映射属性（别名 = 属性 apiName；未映射属性不投影 = 恒缺失）。
        let mut proj = vec![
            format!("{} AS pk", sel.key_col),
            format!("{} AS title", sel.title_sql),
        ];
        for (src, prop) in &ctx.mapping.property_map {
            let col = safe_ident(src.trim())?;
            let alias = safe_ident(prop)?;
            proj.push(format!(r#"{col} AS "{alias}""#));
        }
        let sql = format!(
            "SELECT {} FROM {} WHERE {} ORDER BY title, pk LIMIT ${} OFFSET ${}",
            proj.join(", "),
            sel.from,
            sel.where_sql,
            params.len() + 1,
            params.len() + 2,
        );
        params.push(DataValue::Int(limit as i64));
        params.push(DataValue::Int(page.offset as i64));

        let source_db = ctx.mapping.effective_source().ok_or_else(|| {
            StoreError::Backend("虚拟绑定缺源寻址（sourceId/sourceDbId 均空）".into())
        })?;
        let ds = query_source_readonly(source_db, &sql, params, "onto_vload").await?;
        let schema = ds.schema.as_ref();
        let mut rows = Vec::new();
        for row in ds.iter() {
            let mut props = Map::new();
            for (_, prop) in &ctx.mapping.property_map {
                props.insert(prop.clone(), col_json(row, schema, prop));
            }
            rows.push(ObjectRecord {
                pk: col_text(row, schema, "pk"),
                title: col_text(row, schema, "title"),
                properties: Value::Object(props),
            });
        }
        let has_more = rows.len() as u32 == limit;
        Ok(ObjectPage {
            object_type: ctx.def.api_name.clone(),
            rows,
            limit,
            offset: page.offset,
            has_more,
        })
    }

    /// 聚合下推：Count / GroupCount / GroupSum（GROUP BY 下推；ORDER BY n/s DESC 对齐物化口径）。
    async fn aggregate(
        &self,
        ctx: &BackendCtx,
        set: &ObjectSet,
        agg: &Aggregation,
    ) -> StoreResult<Value> {
        let mut params: Vec<DataValue> = Vec::new();
        let sel = compile_subtree(ctx, set, &mut params)?;
        let sql = match agg {
            Aggregation::Count => {
                format!("SELECT COUNT(*) AS n FROM {} WHERE {}", sel.from, sel.where_sql)
            }
            Aggregation::GroupCount { property } => {
                let col = PgDirectBackend::col_of(ctx, property)?;
                format!(
                    "SELECT {col}::text AS g, COUNT(*) AS n FROM {} WHERE {} GROUP BY g ORDER BY n DESC",
                    sel.from, sel.where_sql
                )
            }
            Aggregation::GroupSum { group_by, sum } => {
                let gcol = PgDirectBackend::col_of(ctx, group_by)?;
                let scol = PgDirectBackend::col_of(ctx, sum)?;
                let sbt = PgDirectBackend::base_type_of(ctx, sum);
                if !is_numeric(sbt) {
                    return Err(StoreError::Backend(format!(
                        "GroupSum 求和属性「{sum}」基型 {sbt:?} 非数值：虚拟直查拒绝下推"
                    )));
                }
                format!(
                    "SELECT {gcol}::text AS g, COALESCE(SUM({scol}), 0) AS s FROM {} WHERE {} GROUP BY g ORDER BY s DESC",
                    sel.from, sel.where_sql
                )
            }
        };
        let source_db = ctx.mapping.effective_source().ok_or_else(|| {
            StoreError::Backend("虚拟绑定缺源寻址（sourceId/sourceDbId 均空）".into())
        })?;
        let ds = query_source_readonly(source_db, &sql, params, "onto_vagg").await?;
        let schema = ds.schema.as_ref();
        match agg {
            Aggregation::Count => {
                let n = ds
                    .iter()
                    .next()
                    .map(|r| col_text(r, schema, "n").parse::<i64>().unwrap_or(0))
                    .unwrap_or(0);
                Ok(json!({ "count": n }))
            }
            _ => {
                let mut buckets = Vec::new();
                let has_sum = ds.schema.fields.iter().any(|f| f.name == "s");
                for row in ds.iter() {
                    let g = match row.get_by_name(schema, "g") {
                        Some(v) => crate::funnel_store::datavalue_to_json(v),
                        None => Value::Null,
                    };
                    let bucket = if has_sum {
                        json!({ "group": g, "sum": col_text(row, schema, "s") })
                    } else {
                        json!({ "group": g, "count": col_text(row, schema, "n").parse::<i64>().unwrap_or(0) })
                    };
                    buckets.push(bucket);
                }
                Ok(json!({ "groups": buckets }))
            }
        }
    }

    /// 解析 pk 集合（分派器桥接用）：`SELECT key FROM … WHERE 下推 LIMIT max+1`——
    /// 超出 max 即整查询拒绝（fail-closed 不截断，防桥接语义残缺）。
    async fn resolve_pks(
        &self,
        ctx: &BackendCtx,
        set: &ObjectSet,
        max: u32,
    ) -> StoreResult<Vec<String>> {
        let mut params: Vec<DataValue> = Vec::new();
        let sel = compile_subtree(ctx, set, &mut params)?;
        let sql = format!(
            "SELECT {} AS pk FROM {} WHERE {} LIMIT ${}",
            sel.key_col, sel.from, sel.where_sql, params.len() + 1
        );
        params.push(DataValue::Int((max as i64) + 1));
        let source_db = ctx.mapping.effective_source().ok_or_else(|| {
            StoreError::Backend("虚拟绑定缺源寻址（sourceId/sourceDbId 均空）".into())
        })?;
        let ds = query_source_readonly(source_db, &sql, params, "onto_vpks").await?;
        let schema = ds.schema.as_ref();
        let mut out = Vec::new();
        for row in ds.iter() {
            out.push(col_text(row, schema, "pk"));
        }
        if out.len() > max as usize {
            return Err(StoreError::Backend(format!(
                "虚拟端 pk 集合超出桥接上限（>{max}）：请收紧过滤条件或改用物化模式"
            )));
        }
        Ok(out)
    }

    /// 源探测：连通性（SELECT version()）+ resource 列基线反射（schema 漂移比对依据，B-P2-5）。
    async fn probe(&self, source_db_id: &str, resource: Option<&str>) -> StoreResult<ProbeReport> {
        let version_ds = query_source_readonly(source_db_id, "SELECT version() AS v", vec![], "onto_vprobe").await?;
        let detail = version_ds
            .iter()
            .next()
            .map(|r| col_text(r, version_ds.schema.as_ref(), "v"))
            .unwrap_or_default();
        let columns = match resource {
            Some(r) => {
                let table = safe_qualified_table(r)?;
                let (schema_name, table_name) = match table.split_once('.') {
                    Some((s, t)) => (s.to_string(), t.to_string()),
                    None => ("public".to_string(), table.clone()),
                };
                let ds = query_source_readonly(
                    source_db_id,
                    "SELECT column_name, data_type, is_nullable FROM information_schema.columns \
                     WHERE table_schema = $1 AND table_name = $2 ORDER BY ordinal_position",
                    vec![
                        DataValue::String(schema_name),
                        DataValue::String(table_name),
                    ],
                    "onto_vprobe_cols",
                )
                .await?;
                let s = ds.schema.as_ref();
                let cols: Vec<Value> = ds
                    .iter()
                    .map(|row| {
                        json!({
                            "name": col_text(row, s, "column_name"),
                            "dataType": col_text(row, s, "data_type"),
                            "nullable": col_text(row, s, "is_nullable") == "YES",
                        })
                    })
                    .collect();
                Some(Value::Array(cols))
            }
            None => None,
        };
        Ok(ProbeReport { reachable: true, detail, columns })
    }
}

/// 桥接上限常量的引用点（防止 dead_code 警告并集中文档）：分派器统一使用 [`PK_BRIDGE_MAX`]。
pub fn bridge_max() -> u32 {
    PK_BRIDGE_MAX as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_onto_model::funnel::MappingMode;
    use serde_json::json;

    fn ctx(resource: &str, pm: Vec<(&str, &str)>) -> BackendCtx {
        let def: cmx_onto_model::ObjectTypeDef = serde_json::from_value(json!({
            "apiName": "VCust",
            "properties": [
                { "apiName": "id", "baseType": "string" },
                { "apiName": "name", "baseType": "string" },
                { "apiName": "amount", "baseType": "double" },
                { "apiName": "createdAt", "baseType": "timestamp" },
                { "apiName": "active", "baseType": "boolean" }
            ]
        }))
        .unwrap();
        BackendCtx {
            tenant: "default".into(),
            onto_db_id: "onto_pg".into(),
            def,
            mapping: cmx_onto_model::SourceMapping {
                object_type: "VCust".into(),
                mode: MappingMode::Virtual,
                resource: Some(resource.into()),
                key_columns: vec!["cust_id".into()],
                title_column: Some("cust_name".into()),
                property_map: pm.into_iter().map(|(a, b)| (a.into(), b.into())).collect(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn compile_base_where_is_true() {
        let cx = ctx("fico.src_cust", vec![("cust_id", "id"), ("cust_name", "name"), ("amt", "amount")]);
        let mut p = Vec::new();
        let sel = compile_subtree(&cx, &ObjectSet::Base { object_type: "VCust".into() }, &mut p).unwrap();
        assert_eq!(sel.from, "fico.src_cust");
        assert_eq!(sel.where_sql, "true");
        assert_eq!(sel.key_col, "cust_id");
        assert_eq!(sel.title_sql, "COALESCE(cust_name::text, cust_id::text)");
    }

    #[test]
    fn compile_filter_numeric_and_text() {
        let cx = ctx("src", vec![("amt", "amount"), ("nm", "name")]);
        let mut params = Vec::new();
        let sql = {
            let mut c = PushdownCompiler { ctx: &cx, params: &mut params };
            c.pred(&Predicate::And {
                predicates: vec![
                    Predicate::Ge { property: "amount".into(), value: json!(1000) },
                    Predicate::Eq { property: "name".into(), value: json!("Ada") },
                ],
            })
            .unwrap()
        };
        assert_eq!(sql, "(amt::numeric >= ($1)::text::numeric AND nm::text = $2)");
        assert_eq!(params.len(), 2);
        assert!(matches!(params[0], DataValue::String(_)));
        assert!(matches!(params[1], DataValue::String(_)));
    }

    #[test]
    fn unmapped_property_rejected() {
        let cx = ctx("src", vec![("nm", "name")]);
        let mut params = Vec::new();
        let e = {
            let mut c = PushdownCompiler { ctx: &cx, params: &mut params };
            c.pred(&Predicate::Eq { property: "amount".into(), value: json!(1) })
                .unwrap_err()
        };
        assert!(e.to_string().contains("未映射"), "应报未映射: {e}");
    }

    #[test]
    fn contains_uses_like_with_escape() {
        let cx = ctx("src", vec![("nm", "name")]);
        let mut params = Vec::new();
        let sql = {
            let mut c = PushdownCompiler { ctx: &cx, params: &mut params };
            c.pred(&Predicate::Contains { property: "name".into(), value: "a_b%c".into() })
                .unwrap()
        };
        assert_eq!(sql, "nm::text LIKE $1");
        assert_eq!(params.len(), 1);
        assert_eq!(
            match &params[0] { DataValue::String(s) => s.clone(), _ => String::new() },
            "%a\\_b\\%c%"
        );
    }

    #[test]
    fn static_pk_values_shape() {
        let cx = ctx("src", vec![]);
        let mut p = Vec::new();
        let sel = compile_subtree(
            &cx,
            &ObjectSet::Static { object_type: "VCust".into(), primary_keys: vec!["a".into(), "b".into()] },
            &mut p,
        )
        .unwrap();
        assert!(sel.where_sql.contains("cust_id IN (SELECT pk FROM (VALUES ($1),($2)) AS v(pk))"), "{}", sel.where_sql);
    }

    #[test]
    fn type_mismatch_and_bad_resource_rejected() {
        let cx = ctx("src", vec![("amt", "amount")]);
        let mut params = Vec::new();
        // 数值列收文本 → 拒绝（避免文本比较语义漂移）
        assert!(PushdownCompiler { ctx: &cx, params: &mut params }
            .pred(&Predicate::Eq { property: "amount".into(), value: json!("abc") }).is_err());
        // resource 注入拒绝
        let bad = ctx("src; DROP TABLE x", vec![]);
        let mut p = Vec::new();
        assert!(compile_subtree(&bad, &ObjectSet::Base { object_type: "VCust".into() }, &mut p).is_err());
        // 类型不一致拒绝
        let mut p2 = Vec::new();
        assert!(compile_subtree(&cx, &ObjectSet::Base { object_type: "Other".into() }, &mut p2).is_err());
    }
}
