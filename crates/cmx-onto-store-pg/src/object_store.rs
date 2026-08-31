//! [`ObjectStore`] 的 tokio-postgres 实现（O2）：per-type 物化表 `oo_<type>` + 统一关系边 `ol_edge`。
//!
//! 存储策略（方案 §6.2 A/B 权衡的落地）：per-type 物理表 + `props JSONB` 列承载全部属性。
//! 这是「物理表骨架 + JSONB 属性」的折中——表随类型而分（查询定位、索引、Search-Around 清晰），
//! 属性走 JSONB（类型演进零 DDL）。O2.1 再把 Active 类型的 isIndexed 属性「固化」为生成列/物理列。
//!
//! 写入原子性：put_objects 走单事务（begin→逐条→commit，对齐 flow store-pg exec_in_txn）。
//! 读取：对象集代数经 [`crate::compile::Compiler`] 编译为一条 SQL，一次往返，无 N+1。

use async_trait::async_trait;
use chrono::Utc;
use cmx_core::model::cell::DataValue;
use cmx_core::model::data::dataset::{DataSet, Row, Schema};
use cmx_database_pg::{
    execute_sql, execute_sql_with_params, get_default_pg_db_manager, query_sql_with_params,
    SqlParams,
};
use cmx_onto_model::objectset::*;
use cmx_onto_model::{LinkResolver, ObjectStore, StoreError, StoreResult};
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::compile::{object_table, safe_ident, Compiler};

/// PG 对象存储。`db_id` 指向已注册数据源（多租户下按租户派生）。
#[derive(Clone)]
pub struct PgObjectStore {
    db_id: String,
}

impl PgObjectStore {
    pub fn new(db_id: impl Into<String>) -> Self {
        Self { db_id: db_id.into() }
    }

    /// boot 时建统一关系边表 `ol_edge`（单表承载所有关系；对齐方案 JoinTable 落法）。
    pub async fn ensure_edge_table(&self) -> StoreResult<()> {
        for stmt in EDGE_DDL {
            execute_sql(&self.db_id, None, stmt)
                .await
                .map_err(|e| StoreError::Backend(format!("建 ol_edge 失败: {e}")))?;
        }
        Ok(())
    }

    async fn exec(&self, sql: &str, params: Vec<DataValue>) -> StoreResult<u64> {
        execute_sql_with_params(&self.db_id, None, sql, SqlParams::DataValues(params))
            .await
            .map_err(|e| StoreError::Backend(format!("执行失败: {e}")))
    }

    async fn query(&self, sql: &str, params: Vec<DataValue>, ds_id: &str) -> StoreResult<DataSet> {
        query_sql_with_params(&self.db_id, None, sql, SqlParams::DataValues(params), ds_id)
            .await
            .map_err(|e| StoreError::Backend(format!("查询失败: {e}")))
    }

    /// O2 乐观锁修改：读改写，条件 `updated_at == expected`。
    ///
    /// 返回 `(status, updated_at, props)`：status ∈ ok/conflict/notFound。expected 为 None 时不校验版本
    /// （盲写）。冲突时回带当前 updated_at + props 供前端刷新（对齐 flow 协同 M1 乐观锁范式）。
    pub async fn modify_with_optlock(
        &self,
        object_type: &str,
        pk: &str,
        set: &Value,
        expected_updated_at: Option<&str>,
    ) -> StoreResult<(String, Option<String>, Option<Value>)> {
        let t = object_table(object_type)?;
        // 读当前
        let ds = self
            .query(
                &format!("SELECT props, updated_at FROM {t} WHERE pk = $1"),
                vec![DataValue::String(pk.to_string())],
                "oo_optlock_read",
            )
            .await?;
        let schema = ds.schema.as_ref();
        let mut cur_props: Option<serde_json::Map<String, Value>> = None;
        let mut cur_uat: Option<String> = None;
        for r in ds.iter() {
            let raw = row_text(r, schema, "props");
            let v: Value = serde_json::from_str(&raw).unwrap_or(Value::Object(serde_json::Map::new()));
            cur_props = Some(match v {
                Value::Object(m) => m,
                _ => serde_json::Map::new(),
            });
            cur_uat = Some(row_text(r, schema, "updated_at"));
        }
        let Some(mut props) = cur_props else {
            return Ok(("notFound".into(), None, None));
        };
        // 版本校验
        if let Some(exp) = expected_updated_at {
            if cur_uat.as_deref() != Some(exp) {
                return Ok(("conflict".into(), cur_uat, Some(Value::Object(props))));
            }
        }
        // 合并 set
        if let Some(patch) = set.as_object() {
            for (k, v) in patch {
                props.insert(k.clone(), v.clone());
            }
        }
        let merged = Value::Object(props);
        // 条件写回（WHERE updated_at = expected，防并发；expected 为空则无条件）
        // 截到微秒：PG timestamptz 精度微秒，避免 Rust 纳秒值报回后与 DB 存值 rfc3339 不一致（假冲突）。
        let now = {
            use chrono::SubsecRound;
            chrono::Utc::now().trunc_subsecs(6)
        };
        let (sql, params) = match expected_updated_at {
            Some(exp) => {
                // timestamptz 参数须绑 DateTime（不能绑 String）；解析回时刻。
                let exp_dt = chrono::DateTime::parse_from_rfc3339(exp)
                    .map(|d| d.with_timezone(&chrono::Utc))
                    .map_err(|e| StoreError::Backend(format!("expectedUpdatedAt 非法时刻: {e}")))?;
                (
                    format!("UPDATE {t} SET props = $1, updated_at = $2 WHERE pk = $3 AND updated_at = $4"),
                    vec![
                        DataValue::Json(merged.to_string()),
                        DataValue::DateTime(now),
                        DataValue::String(pk.to_string()),
                        DataValue::DateTime(exp_dt),
                    ],
                )
            }
            None => (
                format!("UPDATE {t} SET props = $1, updated_at = $2 WHERE pk = $3"),
                vec![
                    DataValue::Json(merged.to_string()),
                    DataValue::DateTime(now),
                    DataValue::String(pk.to_string()),
                ],
            ),
        };
        let n = self.exec(&sql, params).await?;
        if n == 0 {
            // 竞态：expected 匹配读时但写时被他人改 → 冲突
            return Ok(("conflict".into(), cur_uat, Some(merged)));
        }
        Ok(("ok".into(), Some(now.to_rfc3339()), Some(merged)))
    }

    /// [调试] dump 某关系的边（link, a_pk, b_pk）。
    pub async fn dump_edges(&self, link: &str, limit: i64) -> StoreResult<Vec<(String, String, String)>> {
        let ds = self
            .query(
                "SELECT link, a_pk, b_pk FROM ol_edge WHERE link = $1 ORDER BY a_pk, b_pk LIMIT $2",
                vec![DataValue::String(link.to_string()), DataValue::Int(limit)],
                "dbg_edges",
            )
            .await?;
        let schema = ds.schema.as_ref();
        let mut out = Vec::new();
        for r in ds.iter() {
            out.push((
                get_text(r, schema, "link"),
                get_text(r, schema, "a_pk"),
                get_text(r, schema, "b_pk"),
            ));
        }
        Ok(out)
    }
}

/// 关系边表 DDL（幂等）。单表 ol_edge：link + a_pk + b_pk 唯一，props 承载边属性。
const EDGE_DDL: &[&str] = &[
    r#"CREATE TABLE IF NOT EXISTS ol_edge (
        link        VARCHAR(128) NOT NULL,
        a_pk        TEXT         NOT NULL,
        b_pk        TEXT         NOT NULL,
        props       JSONB        NOT NULL DEFAULT '{}',
        created_at  TIMESTAMPTZ  NOT NULL,
        PRIMARY KEY (link, a_pk, b_pk)
    )"#,
    "CREATE INDEX IF NOT EXISTS idx_ol_edge_fwd ON ol_edge (link, a_pk)",
    "CREATE INDEX IF NOT EXISTS idx_ol_edge_rev ON ol_edge (link, b_pk)",
];

#[async_trait]
impl ObjectStore for PgObjectStore {
    async fn ensure_object_table(&self, _tenant: &str, object_type: &str) -> StoreResult<()> {
        let t = object_table(object_type)?;
        // per-type 表：pk 主键 + title + props jsonb + 审计。
        let ddl = format!(
            "CREATE TABLE IF NOT EXISTS {t} (\
             pk TEXT PRIMARY KEY, \
             title TEXT NOT NULL DEFAULT '', \
             props JSONB NOT NULL DEFAULT '{{}}', \
             created_at TIMESTAMPTZ NOT NULL, \
             updated_at TIMESTAMPTZ NOT NULL)"
        );
        execute_sql(&self.db_id, None, &ddl)
            .await
            .map_err(|e| StoreError::Backend(format!("建对象表 {t} 失败: {e}")))?;
        // title 索引（对象浏览器按标题搜）。
        let idx = format!(
            "CREATE INDEX IF NOT EXISTS idx_{}_title ON {t} (title)",
            safe_ident(object_type)?
        );
        let _ = execute_sql(&self.db_id, None, &idx).await;
        Ok(())
    }

    async fn put_object(
        &self,
        tenant: &str,
        object_type: &str,
        pk: &str,
        title: &str,
        properties: &Value,
    ) -> StoreResult<()> {
        let rec = ObjectRecord {
            pk: pk.to_string(),
            title: title.to_string(),
            properties: properties.clone(),
        };
        self.put_objects(tenant, object_type, std::slice::from_ref(&rec)).await?;
        Ok(())
    }

    async fn put_objects(
        &self,
        _tenant: &str,
        object_type: &str,
        rows: &[ObjectRecord],
    ) -> StoreResult<u64> {
        if rows.is_empty() {
            return Ok(0);
        }
        let t = object_table(object_type)?;
        let sql = format!(
            "INSERT INTO {t} (pk, title, props, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $4) \
             ON CONFLICT (pk) DO UPDATE SET title = EXCLUDED.title, props = EXCLUDED.props, \
             updated_at = EXCLUDED.updated_at"
        );
        // 单事务批量 upsert（原子）。
        let manager = get_default_pg_db_manager();
        let txn_ctx = manager.get_transaction_context();
        let txn_id = txn_ctx
            .begin(&self.db_id)
            .await
            .map_err(|e| StoreError::Backend(format!("开启事务失败: {e}")))?;
        let now = Utc::now();
        for r in rows {
            let props = if r.properties.is_null() {
                "{}".to_string()
            } else {
                r.properties.to_string()
            };
            let params = SqlParams::DataValues(vec![
                DataValue::String(r.pk.clone()),
                DataValue::String(r.title.clone()),
                DataValue::Json(props),
                DataValue::DateTime(now),
            ]);
            if let Err(e) = execute_sql_with_params(&self.db_id, Some(&txn_id), &sql, params).await {
                let _ = txn_ctx.rollback(&txn_id).await;
                return Err(StoreError::Backend(format!("事务内写对象失败: {e}")));
            }
        }
        txn_ctx
            .commit(&txn_id)
            .await
            .map_err(|e| StoreError::Backend(format!("提交事务失败: {e}")))?;
        Ok(rows.len() as u64)
    }

    async fn delete_object(&self, _tenant: &str, object_type: &str, pk: &str) -> StoreResult<u64> {
        let t = object_table(object_type)?;
        // 连带清理该对象参与的关系边（作为 A 或 B 端）。
        let _ = self
            .exec(
                "DELETE FROM ol_edge WHERE a_pk = $1 OR b_pk = $1",
                vec![DataValue::String(pk.to_string())],
            )
            .await;
        self.exec(
            &format!("DELETE FROM {t} WHERE pk = $1"),
            vec![DataValue::String(pk.to_string())],
        )
        .await
    }

    async fn put_link(&self, _tenant: &str, edge: &LinkEdge) -> StoreResult<()> {
        safe_ident(&edge.link)?;
        let props = if edge.properties.is_null() {
            "{}".to_string()
        } else {
            edge.properties.to_string()
        };
        self.exec(
            "INSERT INTO ol_edge (link, a_pk, b_pk, props, created_at) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (link, a_pk, b_pk) DO UPDATE SET props = EXCLUDED.props",
            vec![
                DataValue::String(edge.link.clone()),
                DataValue::String(edge.a_pk.clone()),
                DataValue::String(edge.b_pk.clone()),
                DataValue::Json(props),
                DataValue::DateTime(Utc::now()),
            ],
        )
        .await?;
        Ok(())
    }

    async fn delete_link(&self, _tenant: &str, edge: &LinkEdge) -> StoreResult<u64> {
        self.exec(
            "DELETE FROM ol_edge WHERE link = $1 AND a_pk = $2 AND b_pk = $3",
            vec![
                DataValue::String(edge.link.clone()),
                DataValue::String(edge.a_pk.clone()),
                DataValue::String(edge.b_pk.clone()),
            ],
        )
        .await
    }

    async fn load(
        &self,
        tenant: &str,
        set: &ObjectSet,
        page: &Page,
        links: &dyn LinkResolver,
    ) -> StoreResult<ObjectPage> {
        let link_ends = resolve_links(tenant, set, links).await?;
        let mut compiler = Compiler::new(&link_ends);
        let compiled = compiler.compile(set)?;
        let t = object_table(&compiled.terminal_type)?;
        // 外层：从终端表取完整行，pk ∈ 编译出的 pk 集合；分页。
        // 分页参数追加在编译参数之后（$n+1, $n+2）。
        let base_n = compiled.params.len();
        let limit = page.limit.clamp(1, 1000);
        let sql = format!(
            "SELECT o.pk, o.title, o.props FROM {t} o \
             WHERE o.pk IN ({}) ORDER BY o.title, o.pk LIMIT ${} OFFSET ${}",
            compiled.pk_sql,
            base_n + 1,
            base_n + 2,
        );
        let mut params = compiled.params;
        params.push(DataValue::Int(limit as i64));
        params.push(DataValue::Int(page.offset as i64));
        let ds = self.query(&sql, params, "onto_load").await?;
        let schema = ds.schema.as_ref();
        let mut out = Vec::new();
        for row in ds.iter() {
            out.push(ObjectRecord {
                pk: get_text(row, schema, "pk"),
                title: get_text(row, schema, "title"),
                properties: get_json_val(row, schema, "props"),
            });
        }
        let has_more = out.len() as u32 == limit;
        Ok(ObjectPage {
            object_type: compiled.terminal_type,
            rows: out,
            limit,
            offset: page.offset,
            has_more,
        })
    }

    async fn aggregate(
        &self,
        tenant: &str,
        set: &ObjectSet,
        agg: &Aggregation,
        links: &dyn LinkResolver,
    ) -> StoreResult<Value> {
        let link_ends = resolve_links(tenant, set, links).await?;
        let mut compiler = Compiler::new(&link_ends);
        let compiled = compiler.compile(set)?;
        let t = object_table(&compiled.terminal_type)?;
        let inner = &compiled.pk_sql;
        match agg {
            Aggregation::Count => {
                let sql = format!("SELECT COUNT(*) AS n FROM {t} o WHERE o.pk IN ({inner})");
                let ds = self.query(&sql, compiled.params, "onto_agg_count").await?;
                let n = ds
                    .iter()
                    .next()
                    .map(|r| get_i64(r, ds.schema.as_ref(), "n"))
                    .unwrap_or(0);
                Ok(json!({ "count": n }))
            }
            Aggregation::GroupCount { property } => {
                let name = safe_ident(property)?;
                let sql = format!(
                    "SELECT (o.props ->> '{name}') AS g, COUNT(*) AS n FROM {t} o \
                     WHERE o.pk IN ({inner}) GROUP BY g ORDER BY n DESC"
                );
                let ds = self.query(&sql, compiled.params, "onto_agg_group").await?;
                let schema = ds.schema.as_ref();
                let mut buckets = Vec::new();
                for row in ds.iter() {
                    buckets.push(json!({
                        "group": get_opt_text(row, schema, "g"),
                        "count": get_i64(row, schema, "n"),
                    }));
                }
                Ok(json!({ "groups": buckets }))
            }
            Aggregation::GroupSum { group_by, sum } => {
                let g = safe_ident(group_by)?;
                let s = safe_ident(sum)?;
                let sql = format!(
                    "SELECT (o.props ->> '{g}') AS g, \
                     COALESCE(SUM((o.props ->> '{s}')::numeric), 0) AS s FROM {t} o \
                     WHERE o.pk IN ({inner}) GROUP BY g ORDER BY s DESC"
                );
                let ds = self.query(&sql, compiled.params, "onto_agg_sum").await?;
                let schema = ds.schema.as_ref();
                let mut buckets = Vec::new();
                for row in ds.iter() {
                    buckets.push(json!({
                        "group": get_opt_text(row, schema, "g"),
                        "sum": get_numeric_str(row, schema, "s"),
                    }));
                }
                Ok(json!({ "groups": buckets }))
            }
        }
    }
}

/// 预解析对象集里出现的所有关系类型两端（编译 SearchAround 需要）。
async fn resolve_links(
    tenant: &str,
    set: &ObjectSet,
    links: &dyn LinkResolver,
) -> StoreResult<HashMap<String, cmx_onto_model::LinkEnds>> {
    let mut names = Vec::new();
    collect_links(set, &mut names);
    let mut map = HashMap::new();
    for l in names {
        if let Some(ends) = links.ends(tenant, &l).await? {
            map.insert(l, ends);
        }
    }
    Ok(map)
}

fn collect_links(set: &ObjectSet, out: &mut Vec<String>) {
    match set {
        ObjectSet::SearchAround { source, link, .. } => {
            out.push(link.clone());
            collect_links(source, out);
        }
        ObjectSet::Filter { source, .. } => collect_links(source, out),
        ObjectSet::Union { left, right }
        | ObjectSet::Intersect { left, right }
        | ObjectSet::Subtract { left, right } => {
            collect_links(left, out);
            collect_links(right, out);
        }
        ObjectSet::Base { .. } | ObjectSet::Static { .. } => {}
    }
}

// ————————————————————————— 取值助手 —————————————————————————

fn get_text(row: &Row, schema: &Schema, col: &str) -> String {
    get_opt_text(row, schema, col).unwrap_or_default()
}

/// 取列文本（O4 执行器复用）：覆盖字符串/jsonb/整数/布尔/时间等常见列类型。
pub(crate) fn row_text(row: &Row, schema: &Schema, col: &str) -> String {
    match row.get_by_name(schema, col) {
        Some(DataValue::String(s)) => s.clone(),
        Some(DataValue::ShortStr(s)) | Some(DataValue::LongStr(s)) => s.to_string(),
        Some(DataValue::Json(s)) => s.clone(),
        Some(DataValue::Int(n)) => n.to_string(),
        Some(DataValue::Bool(b)) => b.to_string(),
        Some(DataValue::DateTime(t)) => t.to_rfc3339(),
        Some(other) => format!("{other:?}"),
        None => String::new(),
    }
}

fn get_opt_text(row: &Row, schema: &Schema, col: &str) -> Option<String> {
    match row.get_by_name(schema, col) {
        Some(DataValue::String(s)) => Some(s.clone()),
        Some(DataValue::ShortStr(s)) | Some(DataValue::LongStr(s)) => Some(s.to_string()),
        _ => None,
    }
}

fn get_i64(row: &Row, schema: &Schema, col: &str) -> i64 {
    match row.get_by_name(schema, col) {
        Some(DataValue::Int(v)) => *v,
        _ => 0,
    }
}

/// SUM/numeric 列可能回 Decimal 或文本，统一转字符串（前端解析）。
fn get_numeric_str(row: &Row, schema: &Schema, col: &str) -> String {
    match row.get_by_name(schema, col) {
        Some(DataValue::Int(v)) => v.to_string(),
        Some(DataValue::Float(v)) => v.to_string(),
        Some(DataValue::Decimal(d)) => d.to_string(),
        Some(DataValue::String(s)) => s.clone(),
        _ => "0".to_string(),
    }
}

fn get_json_val(row: &Row, schema: &Schema, col: &str) -> Value {
    match row.get_by_name(schema, col) {
        Some(DataValue::Json(s)) | Some(DataValue::String(s)) => {
            serde_json::from_str(s).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    }
}
