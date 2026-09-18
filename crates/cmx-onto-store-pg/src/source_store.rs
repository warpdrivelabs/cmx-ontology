//! 数据源注册表存储（M1b，方案 §5.4）：`om_data_source` CRUD + 探测报告落库。
//!
//! kind：`pg`（ref 引用 toml 池 / standalone 独立连接）| `api`（REST 协议端点，M2 启用后端）|
//! `connector`（仅预留，创建即拒绝）。**凭证只存环境变量引用名**（passwordEnv/tokenEnv），绝不落
//! 明文、不进 git（§5.8.4）；`config.headers` 禁认证类键（防绕过凭证不落库原则，B-P2-4）。
//!
//! 运行期寻址 / 独立源连接池懒注册在 app 层（source_handlers）——store 层保持纯表读写。

use cmx_core::model::cell::DataValue;
use cmx_database_pg::{execute_sql_with_params, query_sql_with_params, SqlParams};
use cmx_onto_model::backend::ObjectDataBackend;
use cmx_onto_model::{StoreError, StoreResult};
use serde_json::{json, Value};

/// 认证类 header 键（小写比较）：禁止出现在 api 源 config.headers（B-P2-4）。
const AUTH_HEADER_KEYS: &[&str] = &[
    "authorization", "cookie", "x-api-key", "api-key", "x-auth-token", "proxy-authorization",
];

pub struct SourceStore {
    db_id: String,
}

impl SourceStore {
    pub fn new(db_id: impl Into<String>) -> Self {
        Self { db_id: db_id.into() }
    }

    /// 列出全部数据源（config 原样透出——其中凭证仅为环境变量名，无明文）。
    pub async fn list(&self) -> StoreResult<Value> {
        let ds = self
            .query(
                "SELECT id, name, kind, config, caps, status, last_probe_at, probe_report \
                 FROM om_data_source ORDER BY id",
            )
            .await?;
        let s = ds.schema.as_ref();
        let mut out = Vec::new();
        for r in ds.iter() {
            out.push(self.row_to_json(r, s));
        }
        Ok(Value::Array(out))
    }

    /// 取一条（None = 未注册）。
    pub async fn get(&self, id: &str) -> StoreResult<Option<Value>> {
        let ds = self
            .query(&format!(
                "SELECT id, name, kind, config, caps, status, last_probe_at, probe_report \
                 FROM om_data_source WHERE id = '{}'",
                id.replace('\'', "''")
            ))
            .await?;
        let s = ds.schema.as_ref();
        Ok(ds.iter().next().map(|r| self.row_to_json(r, s)))
    }

    /// upsert（校验 kind / config 形态 / headers 认证键禁入；返回 id）。
    pub async fn upsert(&self, body: &Value) -> StoreResult<String> {
        let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let kind = body.get("kind").and_then(|v| v.as_str()).unwrap_or("").trim().to_ascii_lowercase();
        if id.is_empty() || name.is_empty() || kind.is_empty() {
            return Err(StoreError::Backend("数据源缺 id / name / kind".into()));
        }
        if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(StoreError::Backend("数据源 id 仅允许字母数字下划线连字符".into()));
        }
        match kind.as_str() {
            "pg" => validate_pg_config(body.get("config").unwrap_or(&Value::Null))?,
            "api" => validate_api_config(body.get("config").unwrap_or(&Value::Null))?,
            "connector" => {
                return Err(StoreError::Backend(
                    "connector 类型仅预留扩展点（es/file/mq），暂不可创建".into(),
                ))
            }
            other => return Err(StoreError::Backend(format!("未知数据源 kind「{other}」（pg | api）"))),
        }
        let caps = body.get("caps").cloned().unwrap_or(Value::Null);
        execute_sql_with_params(
            &self.db_id,
            None,
            "INSERT INTO om_data_source (id, name, kind, config, caps, status, updated_at) \
             VALUES ($1,$2,$3,$4,$5,'active', now()) \
             ON CONFLICT (id) DO UPDATE SET name=EXCLUDED.name, kind=EXCLUDED.kind, config=EXCLUDED.config, \
             caps=EXCLUDED.caps, status='active', updated_at=now()",
            SqlParams::DataValues(vec![
                DataValue::String(id.clone()),
                DataValue::String(name),
                DataValue::String(kind),
                DataValue::Json(body.get("config").cloned().unwrap_or(json!({})).to_string()),
                // jsonb 列参数恒以 Json 承载（DataValue::Null 绑定层无法推断目标类型，cell.rs 口径）
                DataValue::Json(if caps.is_null() { "null".to_string() } else { caps.to_string() }),
            ]),
        )
        .await
        .map_err(|e| StoreError::Backend(format!("写数据源失败: {e}")))?;
        Ok(id)
    }

    /// 删除（幂等；绑定它的映射不会级联——bind 侧校验会在查询时报源不存在）。
    pub async fn delete(&self, id: &str) -> StoreResult<u64> {
        execute_sql_with_params(
            &self.db_id,
            None,
            "DELETE FROM om_data_source WHERE id = $1",
            SqlParams::DataValues(vec![DataValue::String(id.to_string())]),
        )
        .await
        .map_err(|e| StoreError::Backend(format!("删数据源失败: {e}")))
    }

    /// 停用 / 启用（status ∈ active|disabled）。
    pub async fn set_status(&self, id: &str, status: &str) -> StoreResult<u64> {
        if status != "active" && status != "disabled" {
            return Err(StoreError::Backend("status 仅允许 active|disabled".into()));
        }
        execute_sql_with_params(
            &self.db_id,
            None,
            "UPDATE om_data_source SET status = $2, updated_at = now() WHERE id = $1",
            SqlParams::DataValues(vec![DataValue::String(id.to_string()), DataValue::String(status.to_string())]),
        )
        .await
        .map_err(|e| StoreError::Backend(format!("更新数据源状态失败: {e}")))
    }

    /// 探测报告落库（连通 + 列基线；schema 漂移比对依据，B-P2-5）。
    pub async fn save_probe_report(&self, id: &str, report: &Value) -> StoreResult<()> {
        execute_sql_with_params(
            &self.db_id,
            None,
            "UPDATE om_data_source SET last_probe_at = now(), probe_report = $2, updated_at = now() WHERE id = $1",
            SqlParams::DataValues(vec![
                DataValue::String(id.to_string()),
                DataValue::Json(report.to_string()),
            ]),
        )
        .await
        .map(|_| ())
        .map_err(|e| StoreError::Backend(format!("写探测报告失败: {e}")))
    }

    /// 源表结构反射（「从源导入字段」数据；A-P2-10 统一命名 schema 端点）。
    pub async fn reflect_schema(&self, id: &str, resource: &str) -> StoreResult<Value> {
        let Some(src) = self.get(id).await? else {
            return Err(StoreError::Backend(format!("数据源 {id} 不存在")));
        };
        let kind = src.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        if kind != "pg" {
            return Err(StoreError::Backend(format!(
                "数据源 {id} 类型 {kind} 暂不支持结构反射（仅 pg；api 源走 GET /onto-source/schema，M2）"
            )));
        }
        // 反射即探测：连通 + 列基线（错误透出给向导——先测后存的即时形态）。
        let (db_id, _cfg) = resolve_pg_source_db(&src)?;
        let backend = crate::backend_pg::PgDirectBackend::new();
        let probe = backend.probe(&db_id, Some(resource)).await?;
        let cols = probe.columns.clone().unwrap_or(Value::Array(vec![]));
        Ok(json!({ "sourceId": id, "resource": resource, "columns": cols }))
    }

    fn row_to_json(&self, r: &cmx_core::model::data::dataset::Row, s: &cmx_core::model::data::dataset::Schema) -> Value {
        let g = |c: &str| crate::object_store::row_text(r, s, c);
        let opt = |c: &str| {
            let v = g(c);
            if v.is_empty() || v == "Null" { Value::Null } else { Value::String(v) }
        };
        json!({
            "id": g("id"),
            "name": g("name"),
            "kind": g("kind"),
            "config": serde_json::from_str::<Value>(&g("config")).unwrap_or(json!({})),
            "caps": serde_json::from_str::<Value>(&g("caps")).unwrap_or(Value::Null),
            "status": g("status"),
            "lastProbeAt": opt("last_probe_at"),
            "probeReport": serde_json::from_str::<Value>(&g("probe_report")).unwrap_or(Value::Null),
        })
    }

    async fn query(&self, sql: &str) -> StoreResult<cmx_core::model::data::dataset::DataSet> {
        query_sql_with_params(&self.db_id, None, sql, SqlParams::DataValues(vec![]), "src_q")
            .await
            .map_err(|e| StoreError::Backend(format!("查询数据源注册表失败: {e}")))
    }
}

/// 解析 pg 源行的生效 db_id（ref → toml 池名；standalone → `ontosrc_<id>`，E5 前缀防撞名）。
/// standalone 的连接注册（懒注册）在 app 层；此处仅按 config 形态判定寻址名。
pub fn resolve_pg_source_db(src_row: &Value) -> StoreResult<(String, Value)> {
    let cfg = src_row.get("config").cloned().unwrap_or(json!({}));
    if let Some(r) = cfg.get("ref").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) {
        return Ok((r.trim().to_string(), cfg));
    }
    // standalone 形态字段存在性校验（连接由 app 层懒注册）。
    for k in ["host", "port", "db", "user", "passwordEnv"] {
        if cfg.get(k).is_none() {
            return Err(StoreError::Backend(format!(
                "pg 独立源 config 缺 {k}（形态：{{ref}} 或 {{host,port,db,user,passwordEnv,poolMax}}）"
            )));
        }
    }
    let id = src_row.get("id").and_then(|v| v.as_str()).unwrap_or("");
    Ok((format!("ontosrc_{id}"), cfg))
}

/// pg 源 config 校验：恰为 ref 或 standalone 完整形态。
fn validate_pg_config(cfg: &Value) -> StoreResult<()> {
    if cfg.get("ref").is_some() {
        return Ok(());
    }
    let mut missing = Vec::new();
    for k in ["host", "port", "db", "user", "passwordEnv"] {
        if cfg.get(k).is_none() {
            missing.push(k.to_string());
        }
    }
    if !missing.is_empty() {
        return Err(StoreError::Backend(format!(
            "pg 独立源 config 缺字段 {missing:?}（形态：{{ref}} 引用 toml 池，或独立连接全字段；凭证仅环境变量名）"
        )));
    }
    if !cfg.get("passwordEnv").and_then(|v| v.as_str()).is_some_and(|s| !s.trim().is_empty()) {
        return Err(StoreError::Backend(
            "passwordEnv 须为环境变量名（凭证绝不落明文；§5.8.4）".to_string(),
        ));
    }
    Ok(())
}

/// api 源 config 校验：baseUrl 必填；headers 禁认证类键；值支持 ${ENV} 插值由消费方解析。
fn validate_api_config(cfg: &Value) -> StoreResult<()> {
    if cfg.get("baseUrl").and_then(|v| v.as_str()).is_none_or(|s| !s.starts_with("http")) {
        return Err(StoreError::Backend("api 源 config 须含 baseUrl（http/https）".to_string()));
    }
    if let Some(Value::Object(headers)) = cfg.get("headers") {
        for k in headers.keys() {
            if AUTH_HEADER_KEYS.contains(&k.to_ascii_lowercase().as_str()) {
                return Err(StoreError::Backend(format!(
                    "headers 禁止认证类键「{k}」（认证走 config.auth 凭证环境变量引用；B-P2-4）"
                )));
            }
        }
    }
    Ok(())
}
