//! O3 数据集成存储：源→对象映射持久化（om_source_mapping）+ 全量同步执行 + 隔离区（oo_quarantine）。
//!
//! 全量同步：执行 source_query 读源行 → 逐行经内核 [`map_row`] 映射/校验 → 合格者批量 upsert 进
//! `oo_<type>`（复用 PgObjectStore 事务批写）、违规者入 oo_quarantine。返回 [`SyncReport`]。

use cmx_core::model::cell::DataValue;
use cmx_core::model::data::dataset::{DataSet, Row, Schema};
use cmx_database_pg::{execute_sql_with_params, query_sql_with_params, SqlParams};
use cmx_onto_model::objectset::ObjectRecord;
use cmx_onto_model::{map_row, ObjectStore, SourceMapping, StoreError, StoreResult, SyncReport};
use serde_json::{json, Map, Value};

use crate::object_store::PgObjectStore;

/// Funnel 存储（借用 db_id）。
pub struct FunnelStore {
    db_id: String,
}

impl FunnelStore {
    pub fn new(db_id: impl Into<String>) -> Self {
        Self { db_id: db_id.into() }
    }

    /// upsert 一条映射。
    pub async fn upsert_mapping(&self, m: &Value) -> StoreResult<String> {
        let object_type = m.get("objectType").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if object_type.is_empty() {
            return Err(StoreError::Backend("映射缺 objectType".into()));
        }
        let source_query = m.get("sourceQuery").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if source_query.trim().is_empty() {
            return Err(StoreError::Backend("映射缺 sourceQuery".into()));
        }
        let jarr = |k: &str| {
            let v = m.get(k).cloned().unwrap_or(json!([]));
            if v.is_array() { v.to_string() } else { "[]".into() }
        };
        let title = m.get("titleColumn").and_then(|v| v.as_str());
        execute_sql_with_params(
            &self.db_id,
            None,
            "INSERT INTO om_source_mapping (object_type, source_query, key_columns, title_column, property_map, required, created_at) \
             VALUES ($1,$2,$3,$4,$5,$6, now()) \
             ON CONFLICT (object_type) DO UPDATE SET source_query=EXCLUDED.source_query, key_columns=EXCLUDED.key_columns, \
             title_column=EXCLUDED.title_column, property_map=EXCLUDED.property_map, required=EXCLUDED.required",
            SqlParams::DataValues(vec![
                DataValue::String(object_type.clone()),
                DataValue::String(source_query),
                DataValue::Json(jarr("keyColumns")),
                match title { Some(t) if !t.is_empty() => DataValue::String(t.to_string()), _ => DataValue::Null },
                DataValue::Json(jarr("propertyMap")),
                DataValue::Json(jarr("required")),
            ]),
        )
        .await
        .map_err(|e| StoreError::Backend(format!("写映射失败: {e}")))?;
        Ok(object_type)
    }

    /// 列出映射（原样 JSON）。
    pub async fn list_mappings(&self) -> StoreResult<Value> {
        let ds = self
            .query("SELECT object_type, source_query, key_columns, title_column, property_map, required, last_sync_at, last_report \
                    FROM om_source_mapping ORDER BY object_type")
            .await?;
        let schema = ds.schema.as_ref();
        let mut out = Vec::new();
        for r in ds.iter() {
            let g = |c: &str| crate::object_store::row_text(r, schema, c);
            let opt = |c: &str| { let s = g(c); if s.is_empty() || s == "Null" { Value::Null } else { Value::String(s) } };
            out.push(json!({
                "objectType": g("object_type"),
                "sourceQuery": g("source_query"),
                "keyColumns": jparse(&g("key_columns")),
                "titleColumn": opt("title_column"),
                "propertyMap": jparse(&g("property_map")),
                "required": jparse(&g("required")),
                "lastSyncAt": opt("last_sync_at"),
                "lastReport": jparse(&g("last_report")),
            }));
        }
        Ok(Value::Array(out))
    }

    /// 删除映射。
    pub async fn delete_mapping(&self, object_type: &str) -> StoreResult<u64> {
        execute_sql_with_params(
            &self.db_id,
            None,
            "DELETE FROM om_source_mapping WHERE object_type = $1",
            SqlParams::DataValues(vec![DataValue::String(object_type.to_string())]),
        )
        .await
        .map_err(|e| StoreError::Backend(format!("删映射失败: {e}")))
    }

    /// 读取一条映射为内核 [`SourceMapping`]。
    async fn load_mapping(&self, object_type: &str) -> StoreResult<Option<SourceMapping>> {
        let ds = self
            .query(&format!(
                "SELECT object_type, source_query, key_columns, title_column, property_map, required \
                 FROM om_source_mapping WHERE object_type = '{}'",
                object_type.replace('\'', "''")
            ))
            .await?;
        let schema = ds.schema.as_ref();
        for r in ds.iter() {
            let g = |c: &str| crate::object_store::row_text(r, schema, c);
            let key_columns: Vec<String> = serde_json::from_str(&g("key_columns")).unwrap_or_default();
            let pm_raw: Vec<Value> = serde_json::from_str(&g("property_map")).unwrap_or_default();
            let property_map: Vec<(String, String)> = pm_raw
                .iter()
                .filter_map(|p| {
                    let s = p.get("source").or_else(|| p.get(0)).and_then(|v| v.as_str())?;
                    let t = p.get("property").or_else(|| p.get(1)).and_then(|v| v.as_str())?;
                    Some((s.to_string(), t.to_string()))
                })
                .collect();
            let required: Vec<String> = serde_json::from_str(&g("required")).unwrap_or_default();
            let tc = g("title_column");
            return Ok(Some(SourceMapping {
                object_type: g("object_type"),
                source_query: g("source_query"),
                key_columns,
                title_column: if tc.is_empty() || tc == "Null" { None } else { Some(tc) },
                property_map,
                required,
            }));
        }
        Ok(None)
    }

    /// 全量同步：读源 → 映射 → 合格批量 upsert，违规入隔离区。返回报告。
    pub async fn run_full_sync(&self, tenant: &str, object_type: &str) -> StoreResult<SyncReport> {
        let mapping = self
            .load_mapping(object_type)
            .await?
            .ok_or_else(|| StoreError::Backend(format!("对象类型 {object_type} 无源映射")))?;

        // Full 模式：先清该类型旧隔离区（全量同步=替换语义，不累积）。
        let _ = execute_sql_with_params(
            &self.db_id,
            None,
            "DELETE FROM oo_quarantine WHERE object_type = $1",
            SqlParams::DataValues(vec![DataValue::String(object_type.to_string())]),
        )
        .await;

        // 1) 读源行 → JSON 对象数组
        let ds = self.query(&mapping.source_query).await?;
        let schema = ds.schema.as_ref();
        let mut rows_json = Vec::new();
        for r in ds.iter() {
            rows_json.push(row_to_json(r, schema));
        }

        // 2) 逐行映射
        let mut good: Vec<ObjectRecord> = Vec::new();
        let mut report = SyncReport { read: rows_json.len(), written: 0, quarantined: 0 };
        for row in &rows_json {
            match map_row(&mapping, row) {
                Ok(o) => good.push(ObjectRecord { pk: o.pk, title: o.title, properties: o.properties }),
                Err(violations) => {
                    self.quarantine(object_type, row, &violations).await?;
                    report.quarantined += 1;
                }
            }
        }

        // 3) 批量 upsert 合格对象（复用 PgObjectStore 事务批写）
        if !good.is_empty() {
            let os = PgObjectStore::new(self.db_id.clone());
            os.ensure_object_table(tenant, object_type)
                .await
                .map_err(|e| StoreError::Backend(format!("建对象表失败: {e}")))?;
            report.written = os
                .put_objects(tenant, object_type, &good)
                .await
                .map_err(|e| StoreError::Backend(format!("批量写对象失败: {e}")))? as usize;
        }

        // 4) 记同步元数据
        let report_json = json!({ "read": report.read, "written": report.written, "quarantined": report.quarantined });
        let _ = execute_sql_with_params(
            &self.db_id,
            None,
            "UPDATE om_source_mapping SET last_sync_at = now(), last_report = $1 WHERE object_type = $2",
            SqlParams::DataValues(vec![DataValue::Json(report_json.to_string()), DataValue::String(object_type.to_string())]),
        )
        .await;

        Ok(report)
    }

    /// 隔离区列表。
    pub async fn list_quarantine(&self, object_type: Option<&str>, limit: i64) -> StoreResult<Value> {
        let sql = match object_type {
            Some(t) => format!(
                "SELECT id, object_type, raw, violations, source, created_at FROM oo_quarantine \
                 WHERE object_type = '{}' ORDER BY id DESC LIMIT {}",
                t.replace('\'', "''"), limit
            ),
            None => format!(
                "SELECT id, object_type, raw, violations, source, created_at FROM oo_quarantine ORDER BY id DESC LIMIT {limit}"
            ),
        };
        let ds = self.query(&sql).await?;
        let schema = ds.schema.as_ref();
        let mut out = Vec::new();
        for r in ds.iter() {
            let g = |c: &str| crate::object_store::row_text(r, schema, c);
            out.push(json!({
                "id": g("id").parse::<i64>().unwrap_or(0),
                "objectType": g("object_type"),
                "raw": jparse(&g("raw")),
                "violations": jparse(&g("violations")),
                "source": g("source"),
                "createdAt": g("created_at"),
            }));
        }
        Ok(Value::Array(out))
    }

    /// 管道状态（抽取/映射/索引三段 + 计数）。
    pub async fn pipeline_status(&self, object_type: &str) -> StoreResult<Value> {
        let mapping = self.load_mapping(object_type).await?;
        let has_mapping = mapping.is_some();
        let qcount = self.count(&format!(
            "SELECT count(*) AS n FROM oo_quarantine WHERE object_type = '{}'",
            object_type.replace('\'', "''")
        )).await.unwrap_or(0);
        let ocount = self.count(&format!("SELECT count(*) AS n FROM oo_{}", safe_table(object_type))).await.unwrap_or(0);
        Ok(json!({
            "objectType": object_type,
            "stages": [
                { "stage": "extract", "status": if has_mapping { "ready" } else { "unconfigured" } },
                { "stage": "map", "status": if has_mapping { "ready" } else { "unconfigured" } },
                { "stage": "index", "status": if ocount > 0 { "ready" } else { "empty" }, "objects": ocount }
            ],
            "quarantined": qcount,
            "hasMapping": has_mapping
        }))
    }

    // —— 内部 ——
    async fn quarantine(&self, object_type: &str, raw: &Value, violations: &[cmx_onto_model::Violation]) -> StoreResult<()> {
        let v_json = Value::Array(violations.iter().map(|v| json!({ "field": v.field, "reason": v.reason })).collect());
        execute_sql_with_params(
            &self.db_id,
            None,
            "INSERT INTO oo_quarantine (object_type, raw, violations, source, created_at) VALUES ($1,$2,$3,'funnel', now())",
            SqlParams::DataValues(vec![
                DataValue::String(object_type.to_string()),
                DataValue::Json(raw.to_string()),
                DataValue::Json(v_json.to_string()),
            ]),
        )
        .await
        .map(|_| ())
        .map_err(|e| StoreError::Backend(format!("写隔离区失败: {e}")))
    }

    async fn query(&self, sql: &str) -> StoreResult<DataSet> {
        query_sql_with_params(&self.db_id, None, sql, SqlParams::DataValues(vec![]), "funnel_q")
            .await
            .map_err(|e| StoreError::Backend(format!("查询失败: {e}")))
    }
    async fn count(&self, sql: &str) -> StoreResult<i64> {
        let ds = self.query(sql).await?;
        let schema = ds.schema.as_ref();
        for r in ds.iter() {
            return Ok(crate::object_store::row_text(r, schema, "n").parse::<i64>().unwrap_or(0));
        }
        Ok(0)
    }
}

/// 一行 DataSet → JSON 对象（列名→值）。
fn row_to_json(row: &Row, schema: &Schema) -> Value {
    let mut m = Map::new();
    for f in &schema.fields {
        let v = row.get_by_name(schema, &f.name).map(datavalue_to_json).unwrap_or(Value::Null);
        m.insert(f.name.clone(), v);
    }
    Value::Object(m)
}

fn datavalue_to_json(v: &DataValue) -> Value {
    match v {
        DataValue::Null | DataValue::NullTyped(_) => Value::Null,
        DataValue::Bool(b) => json!(b),
        DataValue::Int(n) => json!(n),
        DataValue::Float(f) => json!(f),
        DataValue::String(s) => Value::String(s.clone()),
        DataValue::ShortStr(s) | DataValue::LongStr(s) => Value::String(s.to_string()),
        DataValue::Decimal(d) => json!(d.to_string()),
        DataValue::DateTime(t) => Value::String(t.to_rfc3339()),
        DataValue::Date(d) => Value::String(d.to_string()),
        DataValue::Json(s) => serde_json::from_str(s).unwrap_or(Value::String(s.clone())),
        DataValue::Uuid(u) => Value::String(u.to_string()),
        other => Value::String(format!("{other:?}")),
    }
}

fn jparse(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or(Value::Null)
}
fn safe_table(t: &str) -> String {
    t.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect()
}
