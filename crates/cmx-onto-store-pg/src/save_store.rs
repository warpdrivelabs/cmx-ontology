//! 保存 + 修订管道（同事务）—— [`PgOntologyStore`] 的 inherent 方法块（方案 20260917 §8）。
//!
//! 一条修订管道贯穿：七类资源 save / delete 全部走「事务内 upsert（save SQL **剥离 status**
//! 与弃用四列——状态只能经 /lifecycle/transition 变更）→ 追加 om_revision → 提交」；
//! 修订失败即保存失败（不留半态）。修订号 per-resource max+1，UNIQUE 撞号整个事务重试（≤3）。
//! view 的 payload 剥离 layout（layout 变更不产生修订）；删除写墓碑（deleted=true）。

use cmx_core::model::cell::DataValue;
use cmx_database_pg::{execute_sql_with_params, get_default_pg_db_manager, SqlParams};
use cmx_onto_model::{
    ActionTypeDef, FunctionDef, InterfaceDef, LinkTypeDef, ObjectTypeDef, SceneViewDef,
    SharedPropertyTypeDef, StoreError, StoreResult,
};
use serde_json::Value;

use crate::revision_store::revision_table;
use crate::store::PgOntologyStore;

/// 修订唯一约束名片段（并发撞号重试的判据）。
const REVISION_UQ: &str = "om_revision_resource_kind_api_name_revision";
/// 撞号重试上限。
const RETRY_LIMIT: usize = 3;

/// 修订撞号判据：错误串含唯一约束名。
fn is_revision_race(e: &StoreError) -> bool {
    matches!(e, StoreError::Backend(m) if m.contains(REVISION_UQ))
}

impl PgOntologyStore {
    // ─────────────────── 对象类型 ───────────────────

    /// 保存对象类型（乐观锁 + 同事务修订）。
    pub async fn save_object_with_revision(
        &self,
        def: &ObjectTypeDef,
        changed_by: &str,
        change_note: Option<&str>,
    ) -> StoreResult<u32> {
        let payload = serde_json::to_value(def).unwrap_or(Value::Null);
        for _ in 0..RETRY_LIMIT {
            let (txn, txn_ctx) = self.begin().await?;
            let r = self.upsert_object_locked_tx(&txn, def).await;
            match r {
                Ok(v) => {
                    if let Err(e) = self
                        .append_revision_tx(&txn, "object", &def.api_name, &payload, change_note, changed_by, false)
                        .await
                    {
                        let _ = txn_ctx.rollback(&txn).await;
                        if is_revision_race(&e) { continue; }
                        return Err(e);
                    }
                    self.commit(txn, &txn_ctx).await?;
                    return Ok(v);
                }
                Err(e) => {
                    let _ = txn_ctx.rollback(&txn).await;
                    if is_revision_race(&e) { continue; }
                    return Err(e);
                }
            }
        }
        Err(StoreError::Backend("修订号并发冲突（重试 3 次未成功），请重试".into()))
    }

    async fn upsert_object_locked_tx(&self, txn: &str, def: &ObjectTypeDef) -> StoreResult<u32> {
        let now = chrono::Utc::now();
        // status 剥离：INSERT 缺省落列默认 experimental；DO UPDATE 不含 status（保留 live 现值）。
        if def.version == 0 {
            let ds = self
                .query_tx(
                    txn,
                    "INSERT INTO om_object_type \
                     (api_name, display_name, description, icon, color, primary_key, title_property, \
                      properties, implements, dam, doc_type, datasource, cmx_origin, version, created_at, updated_at) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,1,$14,$14) \
                     ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, \
                      description=EXCLUDED.description, icon=EXCLUDED.icon, color=EXCLUDED.color, \
                      primary_key=EXCLUDED.primary_key, title_property=EXCLUDED.title_property, \
                      properties=EXCLUDED.properties, implements=EXCLUDED.implements, \
                      dam=EXCLUDED.dam, doc_type=EXCLUDED.doc_type, datasource=EXCLUDED.datasource, cmx_origin=EXCLUDED.cmx_origin, \
                      version=om_object_type.version + 1, updated_at=EXCLUDED.updated_at \
                     RETURNING version",
                    vec![
                        DataValue::String(def.api_name.clone()),
                        DataValue::String(def.display_name.clone()),
                        DataValue::String(def.description.clone()),
                        DataValue::String(def.icon.clone()),
                        DataValue::String(def.color.clone()),
                        DataValue::String(def.primary_key.clone()),
                        DataValue::String(def.title_property.clone()),
                        crate::store::json_arr_pub(&def.properties),
                        crate::store::json_arr_pub(&def.implements),
                        DataValue::Json(serde_json::to_string(&def.dam).unwrap_or_else(|_| "{}".to_string())),
                        DataValue::Json(serde_json::to_string(&def.doc_type).unwrap_or_else(|_| "{}".to_string())),
                        crate::store::opt_json_pub(&def.datasource),
                        crate::store::opt_json_pub(&def.cmx_origin),
                        DataValue::DateTime(now),
                    ],
                    "save_object_v0",
                )
                .await?;
            return ds
                .iter()
                .next()
                .map(|row| crate::store::get_i64(row, ds.schema.as_ref(), "version") as u32)
                .ok_or_else(|| StoreError::Backend("保存对象类型未返回版本号".into()));
        }
        let ds = self
            .query_tx(
                txn,
                "UPDATE om_object_type SET display_name=$2, description=$3, icon=$4, color=$5, \
                 primary_key=$6, title_property=$7, properties=$8, implements=$9, \
                 dam=$10, doc_type=$11, datasource=$12, cmx_origin=$13, version=version + 1, updated_at=$14 \
                 WHERE api_name=$1 AND version=$15 \
                 RETURNING version",
                vec![
                    DataValue::String(def.api_name.clone()),
                    DataValue::String(def.display_name.clone()),
                    DataValue::String(def.description.clone()),
                    DataValue::String(def.icon.clone()),
                    DataValue::String(def.color.clone()),
                    DataValue::String(def.primary_key.clone()),
                    DataValue::String(def.title_property.clone()),
                    crate::store::json_arr_pub(&def.properties),
                    crate::store::json_arr_pub(&def.implements),
                    DataValue::Json(serde_json::to_string(&def.dam).unwrap_or_else(|_| "{}".to_string())),
                    DataValue::Json(serde_json::to_string(&def.doc_type).unwrap_or_else(|_| "{}".to_string())),
                    crate::store::opt_json_pub(&def.datasource),
                    crate::store::opt_json_pub(&def.cmx_origin),
                    DataValue::DateTime(now),
                    DataValue::Int(def.version as i64),
                ],
                "save_object_locked",
            )
            .await?;
        if let Some(row) = ds.iter().next() {
            return Ok(crate::store::get_i64(row, ds.schema.as_ref(), "version") as u32);
        }
        self.conflict_or_not_found(txn, "om_object_type", &def.api_name, def.version).await
    }

    // ─────────────────── 关系类型 ───────────────────

    pub async fn save_link_with_revision(
        &self,
        def: &LinkTypeDef,
        changed_by: &str,
        change_note: Option<&str>,
    ) -> StoreResult<u32> {
        // 索引维护前置（对齐既有 save 语义：JoinTable 缺表在落库前硬失败）。
        self.ensure_backing_indexes(def).await?;
        let payload = serde_json::to_value(def).unwrap_or(Value::Null);
        self.save_simple_with_revision(
            "link",
            &def.api_name,
            LINK_UPSERT_SQL,
            LINK_UPDATE_SQL,
            link_params(def),
            &payload,
            changed_by,
            change_note,
        )
        .await
    }

    // ─────────────────── 接口 / 共享属性 / 动作 / 函数 ───────────────────

    pub async fn save_interface_with_revision(
        &self,
        def: &InterfaceDef,
        changed_by: &str,
        change_note: Option<&str>,
    ) -> StoreResult<u32> {
        let payload = serde_json::to_value(def).unwrap_or(Value::Null);
        self.save_simple_with_revision("interface", &def.api_name, INTERFACE_UPSERT_SQL, INTERFACE_UPDATE_SQL, interface_params(def), &payload, changed_by, change_note).await
    }

    pub async fn save_shared_with_revision(
        &self,
        def: &SharedPropertyTypeDef,
        changed_by: &str,
        change_note: Option<&str>,
    ) -> StoreResult<u32> {
        let payload = serde_json::to_value(def).unwrap_or(Value::Null);
        self.save_simple_with_revision("shared_property", &def.api_name, SHARED_UPSERT_SQL, SHARED_UPDATE_SQL, shared_params(def), &payload, changed_by, change_note).await
    }

    pub async fn save_action_with_revision(
        &self,
        def: &ActionTypeDef,
        targets: &[String],
        changed_by: &str,
        change_note: Option<&str>,
    ) -> StoreResult<u32> {
        let payload = serde_json::to_value(def).unwrap_or(Value::Null);
        self.save_simple_with_revision("action", &def.api_name, ACTION_UPSERT_SQL, ACTION_UPDATE_SQL, action_params(def, targets), &payload, changed_by, change_note).await
    }

    pub async fn save_function_with_revision(
        &self,
        def: &FunctionDef,
        changed_by: &str,
        change_note: Option<&str>,
    ) -> StoreResult<u32> {
        let payload = serde_json::to_value(def).unwrap_or(Value::Null);
        self.save_simple_with_revision("function", &def.api_name, FUNCTION_UPSERT_SQL, FUNCTION_UPDATE_SQL, function_params(def), &payload, changed_by, change_note).await
    }

    /// 五类同构保存：事务内 upsert（version=0 盲写 / >0 条件更新）→ 追加修订 → 提交。
    #[allow(clippy::too_many_arguments)]
    async fn save_simple_with_revision(
        &self,
        kind: &str,
        api_name: &str,
        upsert_sql: &str,
        update_sql: &str,
        params: Vec<DataValue>,
        payload: &Value,
        changed_by: &str,
        change_note: Option<&str>,
    ) -> StoreResult<u32> {
        for _ in 0..RETRY_LIMIT {
            let (txn, txn_ctx) = self.begin().await?;
            let r = self
                .upsert_simple_locked_tx(&txn, kind, upsert_sql, update_sql, params.clone())
                .await;
            match r {
                Ok(v) => {
                    if let Err(e) = self.append_revision_tx(&txn, kind, api_name, payload, change_note, changed_by, false).await {
                        let _ = txn_ctx.rollback(&txn).await;
                        if is_revision_race(&e) { continue; }
                        return Err(e);
                    }
                    self.commit(txn, &txn_ctx).await?;
                    return Ok(v);
                }
                Err(e) => {
                    let _ = txn_ctx.rollback(&txn).await;
                    if is_revision_race(&e) { continue; }
                    return Err(e);
                }
            }
        }
        Err(StoreError::Backend("修订号并发冲突（重试 3 次未成功），请重试".into()))
    }

    /// 五类同构 upsert（事务内；返回落库后版本号）。
    /// params 约定：`[业务列..., now, version]`——尾位为乐观锁版本号（0 = 盲写新建/覆盖）。
    async fn upsert_simple_locked_tx(
        &self,
        txn: &str,
        kind: &str,
        upsert_sql: &str,
        update_sql: &str,
        mut params: Vec<DataValue>,
    ) -> StoreResult<u32> {
        let last = params.pop().ok_or_else(|| StoreError::Backend("保存参数为空".into()))?;
        let DataValue::Int(expect) = last else {
            return Err(StoreError::Backend("保存参数尾元素须为乐观锁版本号".into()));
        };
        if expect == 0 {
            let ds = self.query_tx(txn, upsert_sql, params, "save_simple_v0").await?;
            return ds
                .iter()
                .next()
                .map(|row| crate::store::get_i64(row, ds.schema.as_ref(), "version") as u32)
                .ok_or_else(|| StoreError::Backend("保存未返回版本号".into()));
        }
        // 条件更新（乐观锁）：0 行区分 409 版本冲突 / 404 行不存在。
        let mut upd = params;
        upd.push(DataValue::Int(expect));
        let n_affected = self
            .exec_tx(txn, update_sql, upd.clone(), "save_simple_upd")
            .await?;
        if n_affected > 0 {
            let table = revision_table(kind).ok_or_else(|| StoreError::Backend("未知类别".into()))?;
            let ds = self
                .query_tx(
                    txn,
                    &format!("SELECT version FROM {table} WHERE api_name = $1"),
                    vec![upd_first(&upd)],
                    "save_simple_readver",
                )
                .await?;
            return Ok(ds
                .iter()
                .next()
                .map(|row| crate::store::get_i64(row, ds.schema.as_ref(), "version") as u32)
                .unwrap_or(0));
        }
        let table = revision_table(kind).ok_or_else(|| StoreError::Backend("未知类别".into()))?;
        let ds = self
            .query_tx(
                txn,
                &format!("SELECT 1 AS one FROM {table} WHERE api_name = $1"),
                vec![upd_first(&upd)],
                "save_simple_exists",
            )
            .await?;
        let api_name = match &upd[0] {
            DataValue::String(s) => s.clone(),
            _ => String::new(),
        };
        if ds.iter().next().is_some() {
            Err(StoreError::Conflict(format!(
                "资源 {api_name} 已被他人修改（基线版本 {expect} 已过期），请刷新后重试"
            )))
        } else {
            Err(StoreError::NotFound(format!(
                "资源 {api_name} 不存在（可能已被删除），请刷新"
            )))
        }
    }

    // ─────────────────── 场景视图 ───────────────────

    /// 保存场景（乐观锁 + 同事务修订；payload 剥离 layout——布局 LWW 不进修订）。
    pub async fn save_view_with_revision(
        &self,
        def: &SceneViewDef,
        changed_by: &str,
        change_note: Option<&str>,
    ) -> StoreResult<u32> {
        let mut payload = serde_json::to_value(def).unwrap_or(Value::Null);
        if let Some(obj) = payload.as_object_mut() {
            obj.remove("layout");
            obj.remove("deprecation");
        }
        for _ in 0..RETRY_LIMIT {
            let (txn, txn_ctx) = self.begin().await?;
            let r = self.upsert_view_locked_tx(&txn, def).await;
            match r {
                Ok(v) => {
                    if let Err(e) = self
                        .append_revision_tx(&txn, "view", &def.api_name, &payload, change_note, changed_by, false)
                        .await
                    {
                        let _ = txn_ctx.rollback(&txn).await;
                        if is_revision_race(&e) { continue; }
                        return Err(e);
                    }
                    self.commit(txn, &txn_ctx).await?;
                    return Ok(v);
                }
                Err(e) => {
                    let _ = txn_ctx.rollback(&txn).await;
                    if is_revision_race(&e) { continue; }
                    return Err(e);
                }
            }
        }
        Err(StoreError::Backend("修订号并发冲突（重试 3 次未成功），请重试".into()))
    }

    async fn upsert_view_locked_tx(&self, txn: &str, def: &SceneViewDef) -> StoreResult<u32> {
        let now = chrono::Utc::now();
        // status 剥离（视图同七类纪律）：INSERT 不带 status；DO UPDATE 不含 status。
        if def.version == 0 {
            let ds = self
                .query_tx(
                    txn,
                    "INSERT INTO om_view \
                     (api_name, display_name, description, dam, members, source, layout, version, created_at, updated_at) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7,1,$8,$8) \
                     ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, \
                      description=EXCLUDED.description, dam=EXCLUDED.dam, members=EXCLUDED.members, \
                      source=EXCLUDED.source, layout=EXCLUDED.layout, version=om_view.version + 1, \
                      updated_at=EXCLUDED.updated_at \
                     RETURNING version",
                    vec![
                        DataValue::String(def.api_name.clone()),
                        DataValue::String(def.display_name.clone()),
                        DataValue::String(def.description.clone()),
                        DataValue::Json(serde_json::to_string(&def.dam).unwrap_or_else(|_| "{}".to_string())),
                        crate::store::json_or_default_pub(serde_json::to_value(&def.members).unwrap_or(Value::Null), r#"{"objects":[],"interfaces":[],"links":[]}"#),
                        DataValue::String(crate::view_store::view_source_str_pub(&def.source).to_string()),
                        crate::store::json_or_default_pub(def.layout.clone(), "{}"),
                        DataValue::DateTime(now),
                    ],
                    "save_view_v0",
                )
                .await?;
            return ds
                .iter()
                .next()
                .map(|row| crate::store::get_i64(row, ds.schema.as_ref(), "version") as u32)
                .ok_or_else(|| StoreError::Backend("保存场景未返回版本号".into()));
        }
        let ds = self
            .query_tx(
                txn,
                "UPDATE om_view SET display_name=$2, description=$3, dam=$4, members=$5, source=$6, \
                 layout=$7, version=version + 1, updated_at=$8 \
                 WHERE api_name=$1 AND version=$9 \
                 RETURNING version",
                vec![
                    DataValue::String(def.api_name.clone()),
                    DataValue::String(def.display_name.clone()),
                    DataValue::String(def.description.clone()),
                    DataValue::Json(serde_json::to_string(&def.dam).unwrap_or_else(|_| "{}".to_string())),
                    crate::store::json_or_default_pub(serde_json::to_value(&def.members).unwrap_or(Value::Null), r#"{"objects":[],"interfaces":[],"links":[]}"#),
                    DataValue::String(crate::view_store::view_source_str_pub(&def.source).to_string()),
                    crate::store::json_or_default_pub(def.layout.clone(), "{}"),
                    DataValue::DateTime(now),
                    DataValue::Int(def.version as i64),
                ],
                "save_view_locked",
            )
            .await?;
        if let Some(row) = ds.iter().next() {
            return Ok(crate::store::get_i64(row, ds.schema.as_ref(), "version") as u32);
        }
        self.conflict_or_not_found(txn, "om_view", &def.api_name, def.version).await
    }

    // ─────────────────── 删除（墓碑修订） ───────────────────

    /// 删除资源 + 墓碑修订（同事务）：payload = 删除前 live 定义（查不到时退 {}）。
    pub async fn delete_with_revision(
        &self,
        kind: &str,
        api_name: &str,
        changed_by: &str,
        change_note: Option<&str>,
    ) -> StoreResult<u64> {
        let payload = self.current_def_json(kind, api_name).await?;
        for _ in 0..RETRY_LIMIT {
            let (txn, txn_ctx) = self.begin().await?;
            let table = revision_table(kind)
                .ok_or_else(|| StoreError::Backend(format!("未知资源类别 {kind}")))?;
            let n = execute_sql_with_params(
                &self.db_id,
                Some(&txn),
                &format!("DELETE FROM {table} WHERE api_name = $1"),
                SqlParams::DataValues(vec![DataValue::String(api_name.to_string())]),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("删除失败: {e}")))?;
            if n > 0
                && let Err(e) = self
                    .append_revision_tx(
                        &txn,
                        kind,
                        api_name,
                        &payload.clone().unwrap_or_else(|| Value::Object(Default::default())),
                        change_note,
                        changed_by,
                        true,
                    )
                    .await
                {
                    let _ = txn_ctx.rollback(&txn).await;
                    if is_revision_race(&e) { continue; }
                    return Err(e);
                }
            self.commit(txn, &txn_ctx).await?;
            return Ok(n);
        }
        Err(StoreError::Backend("修订号并发冲突（重试 3 次未成功），请重试".into()))
    }

    /// 当前 live 定义 JSON（墓碑 payload 用；camelCase serde 形状——revert 按类型化定义
    /// 解析，须与保存路径 payload 同形状；view 剥离 layout）。
    async fn current_def_json(&self, kind: &str, api_name: &str) -> StoreResult<Option<Value>> {
        use cmx_onto_model::OntologyStore;
        let mut v: Value = match kind {
            "object" => serde_json::to_value(self.get_object_type("", api_name).await.ok().flatten()),
            "link" => serde_json::to_value(self.get_link_type("", api_name).await.ok().flatten()),
            "interface" => serde_json::to_value(self.get_interface("", api_name).await.ok().flatten()),
            "shared_property" => serde_json::to_value(self.get_shared_property("", api_name).await.ok().flatten()),
            "action" => serde_json::to_value(self.get_action_type("", api_name).await.ok().flatten()),
            "function" => serde_json::to_value(self.get_function("", api_name).await.ok().flatten()),
            "view" => {
                let view = self.get_view("", api_name).await.ok().flatten();
                let mut vv = serde_json::to_value(view)
                    .map_err(|e| StoreError::Backend(format!("序列化场景定义失败: {e}")))?;
                if let Some(o) = vv.as_object_mut() {
                    o.remove("layout");
                }
                Ok(vv)
            }
            other => return Err(StoreError::Backend(format!("未知资源类别 {other}"))),
        }
        .unwrap_or(Value::Null);
        if v.is_null() {
            return Ok(None);
        }
        Ok(Some(v))
    }

    // ─────────────────── 事务薄封装 ───────────────────

    async fn begin(&self) -> StoreResult<(String, cmx_database_pg::manager::TransactionContext)> {
        let manager = get_default_pg_db_manager();
        let txn_ctx = manager.get_transaction_context();
        let txn = txn_ctx
            .begin(&self.db_id)
            .await
            .map_err(|e| StoreError::Backend(format!("开启事务失败: {e}")))?;
        Ok((txn, txn_ctx))
    }

    async fn commit(
        &self,
        txn: String,
        txn_ctx: &cmx_database_pg::manager::TransactionContext,
    ) -> StoreResult<()> {
        txn_ctx
            .commit(&txn)
            .await
            .map_err(|e| StoreError::Backend(format!("提交事务失败: {e}")))
    }

    /// 条件更新 0 行后区分 409（版本冲突）与 404（行不存在）。
    async fn conflict_or_not_found(
        &self,
        txn: &str,
        table: &str,
        api_name: &str,
        version: u32,
    ) -> StoreResult<u32> {
        let ds = self
            .query_tx(
                txn,
                &format!("SELECT 1 AS one FROM {table} WHERE api_name = $1"),
                vec![DataValue::String(api_name.to_string())],
                "save_exists_check",
            )
            .await?;
        if ds.iter().next().is_some() {
            Err(StoreError::Conflict(format!(
                "资源 {api_name} 已被他人修改（基线版本 {version} 已过期），请刷新后重试"
            )))
        } else {
            Err(StoreError::NotFound(format!(
                "资源 {api_name} 不存在（可能已被删除），请刷新"
            )))
        }
    }
}

fn upd_first(v: &[DataValue]) -> DataValue {
    v.first().cloned().unwrap_or(DataValue::Null)
}

// ————————————————————————— 五类 upsert / update SQL（save 口径：剥离 status 与弃用列） —————————————————————————

const LINK_UPSERT_SQL: &str = "INSERT INTO om_link_type \
    (api_name, display_name, cardinality, object_type_a, object_type_b, role_a, role_b, backing, created_at, updated_at, version) \
    VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$9,1) \
    ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, \
     cardinality=EXCLUDED.cardinality, object_type_a=EXCLUDED.object_type_a, \
     object_type_b=EXCLUDED.object_type_b, role_a=EXCLUDED.role_a, role_b=EXCLUDED.role_b, \
     backing=EXCLUDED.backing, version=om_link_type.version + 1, updated_at=EXCLUDED.updated_at \
    RETURNING version";

const LINK_UPDATE_SQL: &str = "UPDATE om_link_type SET display_name=$2, cardinality=$3, object_type_a=$4, \
    object_type_b=$5, role_a=$6, role_b=$7, backing=$8, version=version + 1, updated_at=$9 \
    WHERE api_name=$1 AND version=$10";

const INTERFACE_UPSERT_SQL: &str = "INSERT INTO om_interface \
    (api_name, display_name, properties, extends, created_at, updated_at, version) \
    VALUES ($1,$2,$3,$4,$5,$5,1) \
    ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, \
     properties=EXCLUDED.properties, extends=EXCLUDED.extends, version=om_interface.version + 1, \
     updated_at=EXCLUDED.updated_at \
    RETURNING version";

const INTERFACE_UPDATE_SQL: &str = "UPDATE om_interface SET display_name=$2, properties=$3, extends=$4, \
    version=version + 1, updated_at=$5 WHERE api_name=$1 AND version=$6";

const SHARED_UPSERT_SQL: &str = "INSERT INTO om_shared_property \
    (api_name, display_name, base_type, semantic_type, description, created_at, updated_at, version) \
    VALUES ($1,$2,$3,$4,$5,$6,$6,1) \
    ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, \
     base_type=EXCLUDED.base_type, semantic_type=EXCLUDED.semantic_type, description=EXCLUDED.description, \
     version=om_shared_property.version + 1, updated_at=EXCLUDED.updated_at \
    RETURNING version";

const SHARED_UPDATE_SQL: &str = "UPDATE om_shared_property SET display_name=$2, base_type=$3, semantic_type=$4, \
    description=$5, version=version + 1, updated_at=$6 WHERE api_name=$1 AND version=$7";

const ACTION_UPSERT_SQL: &str = "INSERT INTO om_action_type \
    (api_name, display_name, description, parameters, logic, validations, side_effects, function_backing, \
     target_object_types, created_at, updated_at, version) \
    VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$10,1) \
    ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, \
     description=EXCLUDED.description, parameters=EXCLUDED.parameters, logic=EXCLUDED.logic, \
     validations=EXCLUDED.validations, side_effects=EXCLUDED.side_effects, \
     function_backing=EXCLUDED.function_backing, target_object_types=EXCLUDED.target_object_types, \
     version=om_action_type.version + 1, updated_at=EXCLUDED.updated_at \
    RETURNING version";

const ACTION_UPDATE_SQL: &str = "UPDATE om_action_type SET display_name=$2, description=$3, parameters=$4, \
    logic=$5, validations=$6, side_effects=$7, function_backing=$8, target_object_types=$9, \
    version=version + 1, updated_at=$10 WHERE api_name=$1 AND version=$11";

const FUNCTION_UPSERT_SQL: &str = "INSERT INTO om_function \
    (api_name, display_name, runtime, kind, inputs, output, body, description, created_at, updated_at, version) \
    VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$9,1) \
    ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, \
     runtime=EXCLUDED.runtime, kind=EXCLUDED.kind, inputs=EXCLUDED.inputs, output=EXCLUDED.output, \
     body=EXCLUDED.body, description=EXCLUDED.description, version=om_function.version + 1, \
     updated_at=EXCLUDED.updated_at \
    RETURNING version";

const FUNCTION_UPDATE_SQL: &str = "UPDATE om_function SET display_name=$2, runtime=$3, kind=$4, inputs=$5, \
    output=$6, body=$7, description=$8, version=version + 1, updated_at=$9 WHERE api_name=$1 AND version=$10";

// ————————————————————————— 参数构造（约定：[业务列..., now, version]） —————————————————————————

fn link_params(def: &LinkTypeDef) -> Vec<DataValue> {
    vec![
        DataValue::String(def.api_name.clone()),
        DataValue::String(def.display_name.clone()),
        DataValue::String(serde_json::to_value(def.cardinality).unwrap_or(Value::Null).as_str().unwrap_or("oneToMany").to_string()),
        DataValue::String(def.object_type_a.clone()),
        DataValue::String(def.object_type_b.clone()),
        DataValue::String(def.role_a.clone()),
        DataValue::String(def.role_b.clone()),
        DataValue::Json(def.backing.to_string()),
        DataValue::DateTime(chrono::Utc::now()),
        DataValue::Int(def.version as i64),
    ]
}

fn interface_params(def: &InterfaceDef) -> Vec<DataValue> {
    vec![
        DataValue::String(def.api_name.clone()),
        DataValue::String(def.display_name.clone()),
        crate::store::json_arr_pub(&def.properties),
        crate::store::json_arr_pub(&def.extends),
        DataValue::DateTime(chrono::Utc::now()),
        DataValue::Int(def.version as i64),
    ]
}

fn shared_params(def: &SharedPropertyTypeDef) -> Vec<DataValue> {
    vec![
        DataValue::String(def.api_name.clone()),
        DataValue::String(def.display_name.clone()),
        DataValue::String(serde_json::to_value(def.base_type).unwrap_or(Value::Null).as_str().unwrap_or("string").to_string()),
        crate::store::opt_str_pub(&def.semantic_type),
        DataValue::String(def.description.clone()),
        DataValue::DateTime(chrono::Utc::now()),
        DataValue::Int(def.version as i64),
    ]
}

fn action_params(def: &ActionTypeDef, targets: &[String]) -> Vec<DataValue> {
    vec![
        DataValue::String(def.api_name.clone()),
        DataValue::String(def.display_name.clone()),
        DataValue::String(def.description.clone()),
        crate::store::json_or_default_pub(def.parameters.clone(), "[]"),
        crate::store::json_or_default_pub(def.logic.clone(), "[]"),
        crate::store::json_or_default_pub(def.validations.clone(), "[]"),
        crate::store::json_or_default_pub(def.side_effects.clone(), "[]"),
        crate::store::opt_str_pub(&def.function_backing),
        crate::store::json_arr_pub(&targets.to_vec()),
        DataValue::DateTime(chrono::Utc::now()),
        DataValue::Int(def.version as i64),
    ]
}

fn function_params(def: &FunctionDef) -> Vec<DataValue> {
    vec![
        DataValue::String(def.api_name.clone()),
        DataValue::String(def.display_name.clone()),
        DataValue::String(serde_json::to_value(def.runtime).unwrap_or(Value::Null).as_str().unwrap_or("feel").to_string()),
        DataValue::String(serde_json::to_value(def.kind).unwrap_or(Value::Null).as_str().unwrap_or("query").to_string()),
        crate::store::json_or_default_pub(def.inputs.clone(), "[]"),
        crate::store::json_or_default_pub(def.output.clone(), "{}"),
        DataValue::String(def.body.clone()),
        DataValue::String(def.description.clone()),
        DataValue::DateTime(chrono::Utc::now()),
        DataValue::Int(def.version as i64),
    ]
}
