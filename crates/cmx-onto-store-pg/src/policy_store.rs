//! O6 动态安全策略存储（`om_policy`）：upsert / 列表 / 按 subject+objectType 匹配。
//!
//! 一条策略 = { 对某对象类型 objectType，对主体 subject(role/user)，追加行过滤 row_filter(残差谓词)
//! 并拒绝列 marking deny_markings }。查询执行时按 (objectType, 主体集) 取全部适用策略，合并残差。

use cmx_core::model::cell::DataValue;
use cmx_database_pg::{execute_sql_with_params, query_sql_with_params, SqlParams};
use cmx_onto_model::objectset::Predicate;
use cmx_onto_model::{StoreError, StoreResult};
use serde_json::{json, Value};

/// 一条适用策略（匹配后返回，供合并残差 + 脱敏）。
#[derive(Debug, Clone)]
pub struct AppliedPolicy {
    pub api_name: String,
    pub row_filter: Vec<Predicate>,
    pub deny_markings: Vec<String>,
    pub deny_actions: Vec<String>,
}

/// 策略存储（借用 db_id；与 PgObjectStore 同源）。
pub struct PolicyStore {
    db_id: String,
}

impl PolicyStore {
    pub fn new(db_id: impl Into<String>) -> Self {
        Self { db_id: db_id.into() }
    }

    /// upsert 一条策略。
    pub async fn upsert(&self, p: &Value) -> StoreResult<String> {
        let api_name = p.get("apiName").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if api_name.is_empty() {
            return Err(StoreError::Backend("策略缺 apiName".into()));
        }
        let g = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let jarr = |k: &str| {
            let v = p.get(k).cloned().unwrap_or(json!([]));
            if v.is_array() { v.to_string() } else { "[]".to_string() }
        };
        execute_sql_with_params(
            &self.db_id,
            None,
            "INSERT INTO om_policy (api_name, display_name, object_type, subject_kind, subject, row_filter, deny_markings, deny_actions, status, created_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9, now()) \
             ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, object_type=EXCLUDED.object_type, \
             subject_kind=EXCLUDED.subject_kind, subject=EXCLUDED.subject, row_filter=EXCLUDED.row_filter, \
             deny_markings=EXCLUDED.deny_markings, deny_actions=EXCLUDED.deny_actions, status=EXCLUDED.status",
            SqlParams::DataValues(vec![
                DataValue::String(api_name.clone()),
                DataValue::String(g("displayName")),
                {
                    let ot = g("objectType");
                    if ot.is_empty() { DataValue::Null } else { DataValue::String(ot) }
                },
                DataValue::String({ let s = g("subjectKind"); if s.is_empty() { "role".into() } else { s } }),
                DataValue::String(g("subject")),
                DataValue::Json(jarr("rowFilter")),
                DataValue::Json(jarr("denyMarkings")),
                DataValue::Json(jarr("denyActions")),
                DataValue::String({ let s = g("status"); if s.is_empty() { "active".into() } else { s } }),
            ]),
        )
        .await
        .map_err(|e| StoreError::Backend(format!("写策略失败: {e}")))?;
        Ok(api_name)
    }

    /// 列出全部策略（原样 JSON）。
    pub async fn list(&self) -> StoreResult<Value> {
        let ds = query_sql_with_params(
            &self.db_id,
            None,
            "SELECT api_name, display_name, object_type, subject_kind, subject, row_filter, deny_markings, deny_actions, status \
             FROM om_policy ORDER BY api_name",
            SqlParams::DataValues(vec![]),
            "om_policy_list",
        )
        .await
        .map_err(|e| StoreError::Backend(format!("查策略失败: {e}")))?;
        let schema = ds.schema.as_ref();
        let mut out = Vec::new();
        for r in ds.iter() {
            let g = |c: &str| crate::object_store::row_text(r, schema, c);
            let ot = g("object_type");
            let object_type = if ot.is_empty() || ot == "Null" { Value::Null } else { Value::String(ot) };
            out.push(json!({
                "apiName": g("api_name"),
                "displayName": g("display_name"),
                "objectType": object_type,
                "subjectKind": g("subject_kind"),
                "subject": g("subject"),
                "rowFilter": serde_json::from_str::<Value>(&g("row_filter")).unwrap_or(json!([])),
                "denyMarkings": serde_json::from_str::<Value>(&g("deny_markings")).unwrap_or(json!([])),
                "denyActions": serde_json::from_str::<Value>(&g("deny_actions")).unwrap_or(json!([])),
                "status": g("status"),
            }));
        }
        Ok(Value::Array(out))
    }

    /// 删除一条策略。
    pub async fn delete(&self, api_name: &str) -> StoreResult<u64> {
        execute_sql_with_params(
            &self.db_id,
            None,
            "DELETE FROM om_policy WHERE api_name = $1",
            SqlParams::DataValues(vec![DataValue::String(api_name.to_string())]),
        )
        .await
        .map_err(|e| StoreError::Backend(format!("删策略失败: {e}")))
    }

    /// 匹配适用于 (object_type, 主体集) 的 active 策略。subjects 含 user:X / role:Y 形式的键。
    /// object_type 为空的策略（全局）也匹配。
    pub async fn match_policies(
        &self,
        object_type: &str,
        subjects: &[(String, String)], // (subject_kind, subject)
    ) -> StoreResult<Vec<AppliedPolicy>> {
        if subjects.is_empty() {
            return Ok(vec![]);
        }
        let ds = query_sql_with_params(
            &self.db_id,
            None,
            "SELECT api_name, object_type, subject_kind, subject, row_filter, deny_markings, deny_actions \
             FROM om_policy WHERE status = 'active' AND (object_type IS NULL OR object_type = $1)",
            SqlParams::DataValues(vec![DataValue::String(object_type.to_string())]),
            "om_policy_match",
        )
        .await
        .map_err(|e| StoreError::Backend(format!("匹配策略失败: {e}")))?;
        let schema = ds.schema.as_ref();
        let mut out = Vec::new();
        for r in ds.iter() {
            let g = |c: &str| crate::object_store::row_text(r, schema, c);
            let sk = g("subject_kind");
            let sub = g("subject");
            let hit = subjects.iter().any(|(k, s)| *k == sk && *s == sub);
            if !hit {
                continue;
            }
            let filters: Vec<Predicate> =
                serde_json::from_str(&g("row_filter")).unwrap_or_default();
            let deny: Vec<String> = serde_json::from_str(&g("deny_markings")).unwrap_or_default();
            let deny_actions: Vec<String> = serde_json::from_str(&g("deny_actions")).unwrap_or_default();
            out.push(AppliedPolicy { api_name: g("api_name"), row_filter: filters, deny_markings: deny, deny_actions });
        }
        Ok(out)
    }

    /// 写侧 PEP：检查某动作是否被主体的某条策略拒绝。返回拒绝策略 apiName（None=放行）。
    /// 匹配全局策略（object_type IS NULL）或针对该动作目标对象类型的策略。
    pub async fn check_action_permission(
        &self,
        object_types: &[String],
        action: &str,
        subjects: &[(String, String)],
    ) -> StoreResult<Option<String>> {
        if subjects.is_empty() {
            return Ok(None);
        }
        // 取全局 + 相关对象类型的 active 策略，任一命中主体且 deny_actions 含该动作 → 拒。
        let ds = query_sql_with_params(
            &self.db_id,
            None,
            "SELECT api_name, object_type, subject_kind, subject, deny_actions \
             FROM om_policy WHERE status = 'active'",
            SqlParams::DataValues(vec![]),
            "om_policy_action_check",
        )
        .await
        .map_err(|e| StoreError::Backend(format!("检查动作权限失败: {e}")))?;
        let schema = ds.schema.as_ref();
        for r in ds.iter() {
            let g = |c: &str| crate::object_store::row_text(r, schema, c);
            let ot = g("object_type");
            let scoped = ot.is_empty() || ot == "Null" || object_types.iter().any(|t| t == &ot);
            if !scoped {
                continue;
            }
            let hit_subject = subjects.iter().any(|(k, s)| *k == g("subject_kind") && *s == g("subject"));
            if !hit_subject {
                continue;
            }
            let deny_actions: Vec<String> = serde_json::from_str(&g("deny_actions")).unwrap_or_default();
            if deny_actions.iter().any(|a| a == action) {
                return Ok(Some(g("api_name")));
            }
        }
        Ok(None)
    }
}
