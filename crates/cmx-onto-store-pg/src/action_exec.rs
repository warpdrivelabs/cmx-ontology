//! O4 动作引擎 · 执行器（PG 落地）：把 [`ObjectEdit`] 列表**原子**写回 + `oe_action_log` 审计。
//!
//! 与 [`crate::object_store`] 分离：那里是 O2 单条读写；这里是 O4「一次动作 = 一串编辑，全成或全败」。
//! 事务纪律对齐 object_store::put_objects（begin→逐条→commit/rollback）。dry-run 只解析预演、不落库。

use chrono::Utc;
use cmx_core::model::cell::DataValue;
use cmx_database_pg::{
    execute_sql_with_params, get_default_pg_db_manager, query_sql_with_params, SqlParams,
};
use cmx_onto_model::{ObjectEdit, SideEffect, StoreError, StoreResult};
use serde_json::{json, Map, Value};

use crate::compile::{object_table, safe_ident};

/// 动作执行结果（供 handler 返回 + 审计）。
#[derive(Debug, Clone)]
pub struct ApplyOutcome {
    /// 实际影响的编辑条数。
    pub applied: usize,
    /// 审计日志行 id（dry-run 也记，status=dryRun）。
    pub log_id: i64,
    /// 入 Outbox 的副作用条数（dry-run=0）。
    pub effects: usize,
}

/// O4 执行器（借用 db_id；与 PgObjectStore 同源）。
pub struct ActionExecutor {
    db_id: String,
}

impl ActionExecutor {
    pub fn new(db_id: impl Into<String>) -> Self {
        Self { db_id: db_id.into() }
    }

    /// 原子执行一串编辑 + 副作用入 Outbox（全成或全败）。dry_run=true 则只校验/预演、不写库，但仍落审计。
    ///
    /// 事务内顺序：逐条编辑 → 审计日志（committed）→ 副作用入 oe_outbox（log_id 关联）→ commit。
    /// 任一步失败即回滚并落 status=failed 审计（事务外）后返回 Err。
    pub async fn apply(
        &self,
        action: &str,
        params: &Value,
        edits: &[ObjectEdit],
        side_effects: &[SideEffect],
        dry_run: bool,
        actor: Option<&str>,
    ) -> StoreResult<ApplyOutcome> {
        if dry_run {
            // 预演：校验标识合法性（表名/关系名），不落业务库、不入 Outbox。
            for e in edits {
                Self::precheck(e)?;
            }
            let log_id = self
                .write_log(None, action, params, edits, dry_run, "dryRun", None, actor)
                .await?;
            return Ok(ApplyOutcome { applied: edits.len(), log_id, effects: 0 });
        }

        let manager = get_default_pg_db_manager();
        let txn_ctx = manager.get_transaction_context();
        let txn_id = txn_ctx
            .begin(&self.db_id)
            .await
            .map_err(|e| StoreError::Backend(format!("开启事务失败: {e}")))?;

        // 失败即回滚 + 事务外落 failed 审计。
        macro_rules! abort {
            ($err:expr) => {{
                let err = $err;
                let _ = txn_ctx.rollback(&txn_id).await;
                let _ = self
                    .write_log(None, action, params, edits, dry_run, "failed", Some(&err.to_string()), actor)
                    .await;
                return Err(err);
            }};
        }

        for e in edits {
            if let Err(err) = self.apply_one(&txn_id, e).await {
                abort!(err);
            }
        }
        // 审计日志（事务内，拿 log_id 关联 Outbox）
        let log_id = match self
            .write_log(Some(&txn_id), action, params, edits, dry_run, "committed", None, actor)
            .await
        {
            Ok(id) => id,
            Err(err) => abort!(err),
        };
        // 副作用事务性 Outbox（与编辑同事务；提交后 dispatcher 抽取投递）
        for fx in side_effects {
            if let Err(err) = self.insert_outbox(&txn_id, action, log_id, fx).await {
                abort!(err);
            }
        }

        txn_ctx
            .commit(&txn_id)
            .await
            .map_err(|e| StoreError::Backend(format!("提交事务失败: {e}")))?;
        Ok(ApplyOutcome { applied: edits.len(), log_id, effects: side_effects.len() })
    }

    /// dry-run 预检：仅校验标识安全，不触库。
    fn precheck(e: &ObjectEdit) -> StoreResult<()> {
        match e {
            ObjectEdit::CreateObject { object_type, .. }
            | ObjectEdit::ModifyObject { object_type, .. }
            | ObjectEdit::DeleteObject { object_type, .. } => {
                object_table(object_type)?;
            }
            ObjectEdit::AddLink { link, .. } | ObjectEdit::RemoveLink { link, .. } => {
                safe_ident(link)?;
            }
        }
        Ok(())
    }

    /// 事务内执行单条编辑。
    async fn apply_one(&self, txn_id: &str, e: &ObjectEdit) -> StoreResult<()> {
        match e {
            ObjectEdit::CreateObject { object_type, pk, title, properties } => {
                let t = object_table(object_type)?;
                let props = obj_or_empty(properties);
                let sql = format!(
                    "INSERT INTO {t} (pk, title, props, created_at, updated_at) \
                     VALUES ($1, $2, $3, $4, $4) \
                     ON CONFLICT (pk) DO UPDATE SET title = EXCLUDED.title, props = EXCLUDED.props, \
                     updated_at = EXCLUDED.updated_at"
                );
                let now = Utc::now();
                self.txn_exec(
                    txn_id,
                    &sql,
                    vec![
                        DataValue::String(pk.clone()),
                        DataValue::String(title.clone()),
                        DataValue::Json(props.to_string()),
                        DataValue::DateTime(now),
                    ],
                )
                .await
            }
            ObjectEdit::ModifyObject { object_type, pk, set } => {
                let t = object_table(object_type)?;
                // 读改写：读现有 props（事务内），浅合并 set，写回。对象不存在则报错（modify 语义）。
                let existing = self.read_props_in_txn(txn_id, &t, pk).await?;
                let Some(mut cur) = existing else {
                    return Err(StoreError::Backend(format!(
                        "modifyObject：对象 {object_type}#{pk} 不存在"
                    )));
                };
                if let Some(patch) = set.as_object() {
                    for (k, v) in patch {
                        cur.insert(k.clone(), v.clone());
                    }
                }
                let merged = Value::Object(cur);
                let title = title_from(&merged).unwrap_or_else(|| pk.clone());
                let sql = format!("UPDATE {t} SET props = $1, title = $2, updated_at = $3 WHERE pk = $4");
                self.txn_exec(
                    txn_id,
                    &sql,
                    vec![
                        DataValue::Json(merged.to_string()),
                        DataValue::String(title),
                        DataValue::DateTime(Utc::now()),
                        DataValue::String(pk.clone()),
                    ],
                )
                .await
            }
            ObjectEdit::DeleteObject { object_type, pk } => {
                let t = object_table(object_type)?;
                self.txn_exec(
                    txn_id,
                    "DELETE FROM ol_edge WHERE a_pk = $1 OR b_pk = $1",
                    vec![DataValue::String(pk.clone())],
                )
                .await?;
                self.txn_exec(
                    txn_id,
                    &format!("DELETE FROM {t} WHERE pk = $1"),
                    vec![DataValue::String(pk.clone())],
                )
                .await
            }
            ObjectEdit::AddLink { link, a_pk, b_pk, properties } => {
                safe_ident(link)?;
                let props = obj_or_empty(properties);
                self.txn_exec(
                    txn_id,
                    "INSERT INTO ol_edge (link, a_pk, b_pk, props, created_at) \
                     VALUES ($1, $2, $3, $4, $5) \
                     ON CONFLICT (link, a_pk, b_pk) DO UPDATE SET props = EXCLUDED.props",
                    vec![
                        DataValue::String(link.clone()),
                        DataValue::String(a_pk.clone()),
                        DataValue::String(b_pk.clone()),
                        DataValue::Json(props.to_string()),
                        DataValue::DateTime(Utc::now()),
                    ],
                )
                .await
            }
            ObjectEdit::RemoveLink { link, a_pk, b_pk } => {
                safe_ident(link)?;
                self.txn_exec(
                    txn_id,
                    "DELETE FROM ol_edge WHERE link = $1 AND a_pk = $2 AND b_pk = $3",
                    vec![
                        DataValue::String(link.clone()),
                        DataValue::String(a_pk.clone()),
                        DataValue::String(b_pk.clone()),
                    ],
                )
                .await
            }
        }
    }

    /// 事务内读某对象的 props（jsonb → Map）；不存在返回 None。
    async fn read_props_in_txn(
        &self,
        txn_id: &str,
        table: &str,
        pk: &str,
    ) -> StoreResult<Option<Map<String, Value>>> {
        let sql = format!("SELECT props FROM {table} WHERE pk = $1");
        let ds = query_sql_with_params(
            &self.db_id,
            Some(txn_id),
            &sql,
            SqlParams::DataValues(vec![DataValue::String(pk.to_string())]),
            "oe_modify_read",
        )
        .await
        .map_err(|e| StoreError::Backend(format!("读对象失败: {e}")))?;
        let schema = ds.schema.as_ref();
        for r in ds.iter() {
            let raw = crate::object_store::row_text(r, schema, "props");
            let v: Value = serde_json::from_str(&raw).unwrap_or(Value::Object(Map::new()));
            return Ok(Some(match v {
                Value::Object(m) => m,
                _ => Map::new(),
            }));
        }
        Ok(None)
    }

    async fn txn_exec(&self, txn_id: &str, sql: &str, params: Vec<DataValue>) -> StoreResult<()> {
        execute_sql_with_params(&self.db_id, Some(txn_id), sql, SqlParams::DataValues(params))
            .await
            .map(|_| ())
            .map_err(|e| StoreError::Backend(format!("事务内编辑失败: {e}")))
    }

    /// 事务内把一条副作用写入 oe_outbox（status=pending；与编辑同事务，提交后可投递）。
    async fn insert_outbox(&self, txn_id: &str, action: &str, log_id: i64, fx: &SideEffect) -> StoreResult<()> {
        self.txn_exec(
            txn_id,
            "INSERT INTO oe_outbox (action, log_id, kind, target, payload, status, attempts, created_at) \
             VALUES ($1, $2, $3, $4, $5, 'pending', 0, $6)",
            vec![
                DataValue::String(action.to_string()),
                DataValue::Int(log_id),
                DataValue::String(fx.kind.clone()),
                DataValue::String(fx.target.clone()),
                DataValue::Json(obj_or_empty(&fx.payload).to_string()),
                DataValue::DateTime(Utc::now()),
            ],
        )
        .await
    }

    /// 原子领取 pending 的 Outbox 作业（UPDATE→processing RETURNING；SKIP LOCKED 防多 dispatcher 双取，
    /// 对齐 flow P1）。返回 (id, kind, target, payload)。领取后由调用方 dispatch → mark_status 终态。
    pub async fn fetch_pending(&self, limit: i64) -> StoreResult<Vec<(i64, String, String, Value)>> {
        let ds = query_sql_with_params(
            &self.db_id,
            None,
            "UPDATE oe_outbox SET status = 'processing' WHERE id IN ( \
               SELECT id FROM oe_outbox WHERE status = 'pending' ORDER BY id ASC LIMIT $1 FOR UPDATE SKIP LOCKED \
             ) RETURNING id, kind, target, payload",
            SqlParams::DataValues(vec![DataValue::Int(limit)]),
            "oe_outbox_claim",
        )
        .await
        .map_err(|e| StoreError::Backend(format!("领取 Outbox 失败: {e}")))?;
        let schema = ds.schema.as_ref();
        let mut out = Vec::new();
        for r in ds.iter() {
            let g = |c: &str| crate::object_store::row_text(r, schema, c);
            let payload = serde_json::from_str::<Value>(&g("payload")).unwrap_or(Value::Null);
            out.push((g("id").parse::<i64>().unwrap_or(0), g("kind"), g("target"), payload));
        }
        Ok(out)
    }

    /// 标记 Outbox 为某终态（dispatched/deferred/failed）+ attempts+1 + 记 error。
    pub async fn mark_status(&self, id: i64, status: &str, error: Option<&str>) -> StoreResult<u64> {
        // dispatched_at 是 timestamptz：非 dispatched 时须 NullTyped(Timestamp)（裸 Null 绑不定类型→UPDATE失败）。
        let dispatched_at = if status == "dispatched" {
            DataValue::DateTime(Utc::now())
        } else {
            DataValue::NullTyped(cmx_core::model::cell::SqlTypeMarker::Timestamp)
        };
        execute_sql_with_params(
            &self.db_id,
            None,
            "UPDATE oe_outbox SET status = $1, attempts = attempts + 1, last_error = $2, dispatched_at = $3 WHERE id = $4",
            SqlParams::DataValues(vec![
                DataValue::String(status.to_string()),
                match error { Some(e) => DataValue::String(e.to_string()), None => DataValue::Null },
                dispatched_at,
                DataValue::Int(id),
            ]),
        )
        .await
        .map_err(|e| StoreError::Backend(format!("标记 Outbox 失败: {e}")))
    }

    /// 查 Outbox（最新在前；可选 ?status= 过滤）。dispatcher / 运维用。
    pub async fn list_outbox(&self, status: Option<&str>, limit: i64) -> StoreResult<Value> {
        let (sql, params) = match status {
            Some(s) => (
                "SELECT id, action, log_id, kind, target, payload, status, attempts, last_error, created_at, dispatched_at \
                 FROM oe_outbox WHERE status = $1 ORDER BY id DESC LIMIT $2".to_string(),
                vec![DataValue::String(s.to_string()), DataValue::Int(limit)],
            ),
            None => (
                "SELECT id, action, log_id, kind, target, payload, status, attempts, last_error, created_at, dispatched_at \
                 FROM oe_outbox ORDER BY id DESC LIMIT $1".to_string(),
                vec![DataValue::Int(limit)],
            ),
        };
        let ds = query_sql_with_params(&self.db_id, None, &sql, SqlParams::DataValues(params), "oe_outbox_list")
            .await
            .map_err(|e| StoreError::Backend(format!("查 Outbox 失败: {e}")))?;
        let schema = ds.schema.as_ref();
        let mut out = Vec::new();
        for r in ds.iter() {
            let g = |c: &str| crate::object_store::row_text(r, schema, c);
            let opt = |c: &str| { let s = g(c); if s.is_empty() || s == "Null" { Value::Null } else { Value::String(s) } };
            out.push(json!({
                "id": g("id").parse::<i64>().unwrap_or(0),
                "action": g("action"),
                "logId": g("log_id").parse::<i64>().unwrap_or(0),
                "kind": g("kind"),
                "target": g("target"),
                "payload": serde_json::from_str::<Value>(&g("payload")).unwrap_or(Value::Null),
                "status": g("status"),
                "attempts": g("attempts").parse::<i64>().unwrap_or(0),
                "lastError": opt("last_error"),
                "createdAt": g("created_at"),
                "dispatchedAt": opt("dispatched_at"),
            }));
        }
        Ok(Value::Array(out))
    }

    /// 标记一条 Outbox 已投递（dispatcher 投递成功后调；status=dispatched + attempts+1）。
    pub async fn mark_dispatched(&self, id: i64, ok: bool, error: Option<&str>) -> StoreResult<u64> {
        let (status, dispatched_at, err_val) = if ok {
            ("dispatched", DataValue::DateTime(Utc::now()), DataValue::Null)
        } else {
            (
                "failed",
                DataValue::NullTyped(cmx_core::model::cell::SqlTypeMarker::Timestamp),
                match error { Some(e) => DataValue::String(e.to_string()), None => DataValue::Null },
            )
        };
        execute_sql_with_params(
            &self.db_id,
            None,
            "UPDATE oe_outbox SET status = $1, attempts = attempts + 1, last_error = $2, dispatched_at = $3 WHERE id = $4",
            SqlParams::DataValues(vec![
                DataValue::String(status.to_string()),
                err_val,
                dispatched_at,
                DataValue::Int(id),
            ]),
        )
        .await
        .map_err(|e| StoreError::Backend(format!("标记 Outbox 失败: {e}")))
    }

    /// 落一行审计（RETURNING id）。`txn` 为 Some 时在该事务内写（committed 路径，供 Outbox 关联 log_id）；
    /// None 时独立写（dryRun / failed 路径）。
    #[allow(clippy::too_many_arguments)]
    async fn write_log(
        &self,
        txn: Option<&str>,
        action: &str,
        params: &Value,
        edits: &[ObjectEdit],
        dry_run: bool,
        status: &str,
        error: Option<&str>,
        actor: Option<&str>,
    ) -> StoreResult<i64> {
        let edits_json = edits_to_json(edits);
        let sql = "INSERT INTO oe_action_log \
             (action, params, edits, edit_count, dry_run, status, error, actor, executed_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id";
        let ds = query_sql_with_params(
            &self.db_id,
            txn,
            sql,
            SqlParams::DataValues(vec![
                DataValue::String(action.to_string()),
                DataValue::Json(obj_or_empty(params).to_string()),
                DataValue::Json(edits_json.to_string()),
                DataValue::Int(edits.len() as i64),
                DataValue::Bool(dry_run),
                DataValue::String(status.to_string()),
                match error {
                    Some(e) => DataValue::String(e.to_string()),
                    None => DataValue::Null,
                },
                match actor {
                    Some(a) => DataValue::String(a.to_string()),
                    None => DataValue::Null,
                },
                DataValue::DateTime(Utc::now()),
            ]),
            "oe_log_insert",
        )
        .await
        .map_err(|e| StoreError::Backend(format!("写审计失败: {e}")))?;
        let schema = ds.schema.as_ref();
        for r in ds.iter() {
            let id = crate::object_store::row_text(r, schema, "id");
            return Ok(id.parse::<i64>().unwrap_or(0));
        }
        Ok(0)
    }

    /// 查审计日志（最新在前；可选按 action 过滤）。
    pub async fn list_logs(&self, action: Option<&str>, limit: i64) -> StoreResult<Value> {
        let (sql, params) = match action {
            Some(a) => (
                "SELECT id, action, params, edits, edit_count, dry_run, status, error, actor, executed_at \
                 FROM oe_action_log WHERE action = $1 ORDER BY id DESC LIMIT $2"
                    .to_string(),
                vec![DataValue::String(a.to_string()), DataValue::Int(limit)],
            ),
            None => (
                "SELECT id, action, params, edits, edit_count, dry_run, status, error, actor, executed_at \
                 FROM oe_action_log ORDER BY id DESC LIMIT $1"
                    .to_string(),
                vec![DataValue::Int(limit)],
            ),
        };
        let ds = query_sql_with_params(&self.db_id, None, &sql, SqlParams::DataValues(params), "oe_logs")
            .await
            .map_err(|e| StoreError::Backend(format!("查审计失败: {e}")))?;
        let schema = ds.schema.as_ref();
        let mut out = Vec::new();
        for r in ds.iter() {
            let g = |c: &str| crate::object_store::row_text(r, schema, c);
            let opt = |c: &str| {
                let s = g(c);
                if s.is_empty() || s == "Null" {
                    Value::Null
                } else {
                    Value::String(s)
                }
            };
            out.push(json!({
                "id": g("id").parse::<i64>().unwrap_or(0),
                "action": g("action"),
                "params": serde_json::from_str::<Value>(&g("params")).unwrap_or(Value::Null),
                "edits": serde_json::from_str::<Value>(&g("edits")).unwrap_or(Value::Null),
                "editCount": g("edit_count").parse::<i64>().unwrap_or(0),
                "dryRun": g("dry_run") == "true" || g("dry_run") == "t",
                "status": g("status"),
                "error": opt("error"),
                "actor": opt("actor"),
                "executedAt": g("executed_at"),
            }));
        }
        Ok(Value::Array(out))
    }
}

/// ObjectEdit 列表 → 可审计 JSON。
pub fn edits_to_json(edits: &[ObjectEdit]) -> Value {
    Value::Array(
        edits
            .iter()
            .map(|e| match e {
                ObjectEdit::CreateObject { object_type, pk, title, properties } => json!({
                    "op": "createObject", "objectType": object_type, "pk": pk, "title": title, "properties": properties
                }),
                ObjectEdit::ModifyObject { object_type, pk, set } => json!({
                    "op": "modifyObject", "objectType": object_type, "pk": pk, "set": set
                }),
                ObjectEdit::DeleteObject { object_type, pk } => json!({
                    "op": "deleteObject", "objectType": object_type, "pk": pk
                }),
                ObjectEdit::AddLink { link, a_pk, b_pk, properties } => json!({
                    "op": "addLink", "link": link, "aPk": a_pk, "bPk": b_pk, "properties": properties
                }),
                ObjectEdit::RemoveLink { link, a_pk, b_pk } => json!({
                    "op": "removeLink", "link": link, "aPk": a_pk, "bPk": b_pk
                }),
            })
            .collect(),
    )
}

fn obj_or_empty(v: &Value) -> Value {
    if v.is_null() {
        Value::Object(Map::new())
    } else {
        v.clone()
    }
}
fn title_from(props: &Value) -> Option<String> {
    props.get("title").and_then(|v| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}
