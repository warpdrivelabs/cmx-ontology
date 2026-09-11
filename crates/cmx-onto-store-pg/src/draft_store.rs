//! 草稿工作区（om_draft）+ 维护白名单（om_maintainer）+ 全量快照/发布事务 —— [`PgOntologyStore`]
//! 的 inherent 方法块（本体工作室 P2，方案 §2.4/§2.5/§七）。
//!
//! 关键语义（方案钉死，实现据此）：
//! - **fork**：首次 GET /draft 惰性 fork——七类元素 = live 全量、views 段 = live om_view 全量拷贝、
//!   base_rev = 当时 live 快照指纹；发布成功后**草稿行原子删除**（重置由下次 fork 重建，内容与
//!   live 全等），P1 存量视图自然带入，无「删除全部场景」假 diff。
//! - **发布应用**：单事务内 批量 upsert 六类（`jsonb_to_recordset` 单语句/类，千级对象一次往返）
//!   → views 应用（**已存在行不动 layout 列**——发布不回吞他人刚拖的布局；live manual 行草稿无
//!   → 删除，**auto 行豁免**——它只是布局物化产物）→ 派生删除集（live − 草稿，权威语义）批量
//!   删除 + 既有级联（删接口同事务清 implements）→ 读回快照 → rev 去重（最新 rev 相同则不插
//!   版本）→ 删草稿行 → 提交。
//! - **rev 去重**：指纹口径见 cmx-onto-model::draft（七类 + views 语义字段，排除 layout/审计）；
//!   旧 `POST /publish` 原语义保留不动，去重仅新端点 `/releases/publish`。

use chrono::Utc;
use cmx_core::model::cell::DataValue;
use cmx_core::model::data::dataset::DataSet;
use cmx_database_pg::{execute_sql_with_params, get_default_pg_db_manager, query_sql_with_params, SqlParams};
use cmx_onto_model::{
    snapshot_fingerprint, ActionTypeDef, DeletionRef, DraftContent, DraftRow, FunctionDef,
    InterfaceDef, LinkTypeDef, ObjectTypeDef, SceneViewDef, SharedPropertyTypeDef, StoreError,
    StoreResult, ELEMENT_KINDS, KIND_ACTION, KIND_FUNCTION, KIND_INTERFACE, KIND_LINK, KIND_OBJECT,
    KIND_SHARED,
};
use serde::Serialize;
use serde_json::Value;

use crate::store::{get_i64, get_opt_string, get_opt_ts, get_string, PgOntologyStore};

/// 发布结果（含去重标记与变更计数，供发布对话框/审计展示）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishOutcome {
    /// 版本号（去重命中时 = 最新既有版本）。
    pub version: u32,
    pub rev: String,
    /// true = 指纹与最新版本相同，未插新版本（「无实质变更」明示，方案 §2.5）。
    pub deduped: bool,
    pub counts: PublishCounts,
}

/// 发布变更计数（与 diff 预览同口径可对账）。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishCounts {
    pub objects_upserted: usize,
    pub links_upserted: usize,
    pub interfaces_upserted: usize,
    pub shared_upserted: usize,
    pub actions_upserted: usize,
    pub functions_upserted: usize,
    pub deletions: usize,
    pub views_upserted: usize,
    pub views_removed: usize,
}

impl PgOntologyStore {
    // ─────────────────── om_draft 行 CRUD ───────────────────

    /// 读草稿行（单工作区恒 id=1；无行 = 尚未 fork）。
    pub async fn get_draft_row(&self) -> StoreResult<Option<DraftRow>> {
        let ds = self
            .query(
                "SELECT content, base_rev, version, updated_by, updated_at FROM om_draft WHERE id = 1",
                vec![],
                "om_draft_one",
            )
            .await?;
        let Some(row) = ds.iter().next() else { return Ok(None) };
        let s = ds.schema.as_ref();
        let content: DraftContent = crate::store::get_json(row, s, "content")
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        Ok(Some(DraftRow {
            version: get_i64(row, s, "version") as u32,
            base_rev: get_opt_string(row, s, "base_rev").unwrap_or_default(),
            content,
            updated_by: get_opt_string(row, s, "updated_by"),
            updated_at: get_opt_ts(row, s, "updated_at"),
        }))
    }

    /// fork 落行（ON CONFLICT DO NOTHING——并发 fork 单语句幂等，落败方读胜方行即可）。
    pub async fn insert_draft_row(
        &self,
        content: &DraftContent,
        base_rev: &str,
        updated_by: Option<String>,
    ) -> StoreResult<()> {
        self.exec(
            "INSERT INTO om_draft (id, content, base_rev, version, updated_by, updated_at) \
             VALUES (1, $1, $2, 1, $3, $4) ON CONFLICT (id) DO NOTHING",
            vec![
                DataValue::Json(serde_json::to_string(content).map_err(|e| {
                    StoreError::Backend(format!("草稿内容序列化失败: {e}"))
                })?),
                DataValue::String(base_rev.to_string()),
                crate::store::opt_str(&updated_by),
                DataValue::DateTime(Utc::now()),
            ],
        )
        .await?;
        Ok(())
    }

    /// 保存草稿（行级乐观锁；返回新版本号）。
    /// `base_rev`：None = 不动（编辑保存——基线仅 fork/restore/发布重置推进）；Some = 覆盖（restore）。
    /// 0 行 → 行不存在（发布后已重置，需重新 fork，404 语义）或版本不符（他人已保存，409 语义）。
    pub async fn save_draft_row(
        &self,
        content: &DraftContent,
        base_rev: Option<&str>,
        expected_version: u32,
        updated_by: Option<String>,
    ) -> StoreResult<u32> {
        let ds = self
            .query(
                // COALESCE($2, base_rev)：None 保全现值（参数化，禁字符串拼 SQL）。
                "UPDATE om_draft SET content=$1, base_rev=COALESCE($2, base_rev), version=version+1, \
                 updated_by=$3, updated_at=$4 WHERE id=1 AND version=$5 RETURNING version",
                vec![
                    DataValue::Json(serde_json::to_string(content).map_err(|e| {
                        StoreError::Backend(format!("草稿内容序列化失败: {e}"))
                    })?),
                    base_rev.map(|s| DataValue::String(s.to_owned())).unwrap_or(DataValue::Null),
                    crate::store::opt_str(&updated_by),
                    DataValue::DateTime(Utc::now()),
                    DataValue::Int(expected_version as i64),
                ],
                "om_draft_save_locked",
            )
            .await?;
        if let Some(row) = ds.iter().next() {
            return Ok(get_i64(row, ds.schema.as_ref(), "version") as u32);
        }
        let exists = self
            .query("SELECT 1 AS one FROM om_draft WHERE id = 1", vec![], "om_draft_exists")
            .await?;
        if exists.iter().next().is_some() {
            Err(StoreError::Conflict(format!(
                "草稿已被他人保存（基线版本 {expected_version} 已过期），请刷新草稿后重试"
            )))
        } else {
            Err(StoreError::NotFound("草稿不存在（已被发布重置或尚未 fork），请刷新拉取".into()))
        }
    }

    /// restore 覆盖草稿：有行 → 覆盖内容 version+1（expected_version 传 Some 时做条件更新，
    /// 防覆盖他人未发布的编辑）；无行 → fork 落行。返回新草稿版本号。
    pub async fn restore_draft_row(
        &self,
        content: &DraftContent,
        base_rev: &str,
        expected_version: Option<u32>,
        updated_by: Option<String>,
    ) -> StoreResult<u32> {
        if let Some(expected) = expected_version {
            return self.save_draft_row(content, Some(base_rev), expected, updated_by).await;
        }
        let ds = self
            .query(
                "UPDATE om_draft SET content=$1, base_rev=$2, version=version+1, updated_by=$3, updated_at=$4 \
                 WHERE id=1 RETURNING version",
                vec![
                    DataValue::Json(serde_json::to_string(content).map_err(|e| {
                        StoreError::Backend(format!("草稿内容序列化失败: {e}"))
                    })?),
                    DataValue::String(base_rev.to_string()),
                    crate::store::opt_str(&updated_by),
                    DataValue::DateTime(Utc::now()),
                ],
                "om_draft_restore_overwrite",
            )
            .await?;
        if let Some(row) = ds.iter().next() {
            return Ok(get_i64(row, ds.schema.as_ref(), "version") as u32);
        }
        self.insert_draft_row(content, base_rev, updated_by).await?;
        let row = self
            .get_draft_row()
            .await?
            .ok_or_else(|| StoreError::Backend("restore 落行后读回失败".into()))?;
        Ok(row.version)
    }

    /// 丢弃草稿（重开 = 下次 GET /draft 惰性 fork 重建）。expected_version 传 Some 时
    /// 条件删除（防误丢他人未发布的编辑）；返回是否真的删了行。
    pub async fn discard_draft_row(&self, expected_version: Option<u32>) -> StoreResult<bool> {
        let n = match expected_version {
            Some(v) => {
                self.exec(
                    "DELETE FROM om_draft WHERE id = 1 AND version = $1",
                    vec![DataValue::Int(v as i64)],
                )
                .await?
            }
            None => self.exec("DELETE FROM om_draft WHERE id = 1", vec![]).await?,
        };
        Ok(n > 0)
    }

    // ─────────────────── 全量快照（含 views；发布/fork/指纹共用） ───────────────────
    /// 组装全量定义快照（七路：六类元素全量定义按 api_name 序 + om_view 全量行定义含 layout）。
    ///
    /// 相比旧 `snapshot()`（逐类型 N+1），六类各一条全表 SELECT——千级对象一次往返；
    /// views 段为 P2 新增（历史快照无此段，回滚时按「场景保持现状」处理，方案 §2.4）。
    pub async fn snapshot_full(&self, tenant: &str) -> StoreResult<Value> {
        let (objects, links, interfaces, shared, actions, functions, views) = tokio::try_join!(
            self.list_object_defs(tenant),
            self.list_link_defs(tenant),
            self.list_interface_defs(tenant),
            self.list_shared_defs(tenant),
            self.list_action_defs(tenant),
            self.list_function_defs(tenant),
            self.list_view_defs(tenant),
        )?;
        Ok(Self::snapshot_value(objects, links, interfaces, shared, actions, functions, views))
    }

    /// 事务内读回快照（发布应用后组装「发布时刻全量」用；同 [`Self::snapshot_full`] 口径）。
    async fn snapshot_full_tx(&self, txn: &str) -> StoreResult<Value> {
        let objects = self.list_object_defs_tx(txn).await?;
        let links = self.list_link_defs_tx(txn).await?;
        let interfaces = self.list_interface_defs_tx(txn).await?;
        let shared = self.list_shared_defs_tx(txn).await?;
        let actions = self.list_action_defs_tx(txn).await?;
        let functions = self.list_function_defs_tx(txn).await?;
        let views = self.list_view_defs_tx(txn).await?;
        Ok(Self::snapshot_value(objects, links, interfaces, shared, actions, functions, views))
    }

    #[allow(clippy::too_many_arguments)]
    fn snapshot_value(
        objects: Vec<ObjectTypeDef>,
        links: Vec<LinkTypeDef>,
        interfaces: Vec<InterfaceDef>,
        shared: Vec<SharedPropertyTypeDef>,
        actions: Vec<ActionTypeDef>,
        functions: Vec<FunctionDef>,
        views: Vec<SceneViewDef>,
    ) -> Value {
        fn j<T: Serialize>(v: &T) -> Value { serde_json::to_value(v).unwrap_or(Value::Null) }
        serde_json::json!({
            "objectTypes": j(&objects),
            "linkTypes": j(&links),
            "interfaces": j(&interfaces),
            "sharedProperties": j(&shared),
            "actionTypes": j(&actions),
            "functions": j(&functions),
            "views": j(&views),
        })
    }

    // ─────────────────── 版本表 / 白名单 ───────────────────

    /// 最新版本（version, rev）；无任何版本 → None。
    pub async fn latest_rev(&self) -> StoreResult<Option<(u32, String)>> {
        let ds = self
            .query(
                "SELECT version, rev FROM om_version ORDER BY version DESC LIMIT 1",
                vec![],
                "om_ver_latest",
            )
            .await?;
        Ok(ds.iter().next().map(|row| {
            let s = ds.schema.as_ref();
            (
                get_i64(row, s, "version") as u32,
                get_opt_string(row, s, "rev").unwrap_or_default(),
            )
        }))
    }

    /// 维护白名单（subject, subject_kind）——**空表 = 开放**（P1 全员维护等效；
    /// 有行 = 仅命中者可写，评审门不可被直连 API 绕过）。
    pub async fn list_maintainers(&self) -> StoreResult<Vec<(String, String)>> {
        let ds = self
            .query(
                "SELECT subject, subject_kind FROM om_maintainer ORDER BY subject",
                vec![],
                "om_maintainer_list",
            )
            .await?;
        let s = ds.schema.as_ref();
        Ok(ds
            .iter()
            .map(|row| {
                (
                    get_string(row, s, "subject").unwrap_or_default(),
                    get_opt_string(row, s, "subject_kind").unwrap_or_else(|| "user".into()),
                )
            })
            .collect())
    }

    // ─────────────────── 发布事务（原子应用 + 去重 + 草稿重置） ───────────────────

    /// 发布草稿：单事务原子应用（六类批量 upsert → views 应用 → 派生删除+级联 → 读回快照
    /// → rev 去重打版本 → 删草稿行）。返回 [`PublishOutcome`]。
    pub async fn publish_draft(
        &self,
        _tenant: &str,
        draft: &DraftContent,
        summary: &str,
        published_by: Option<String>,
    ) -> StoreResult<PublishOutcome> {
        let manager = get_default_pg_db_manager();
        let txn_ctx = manager.get_transaction_context();
        let txn = txn_ctx
            .begin(&self.db_id)
            .await
            .map_err(|e| StoreError::Backend(format!("开启发布事务失败: {e}")))?;
        match self.publish_draft_tx(&txn, draft, summary, published_by).await {
            Ok(outcome) => {
                txn_ctx
                    .commit(&txn)
                    .await
                    .map_err(|e| StoreError::Backend(format!("提交发布事务失败: {e}")))?;
                Ok(outcome)
            }
            Err(e) => {
                let _ = txn_ctx.rollback(&txn).await;
                Err(e)
            }
        }
    }

    async fn publish_draft_tx(
        &self,
        txn: &str,
        draft: &DraftContent,
        summary: &str,
        published_by: Option<String>,
    ) -> StoreResult<PublishOutcome> {
        let mut counts = PublishCounts::default();

        // 1. 六类批量 upsert（每类一条 jsonb_to_recordset 语句——千级对象一次往返）。
        let batches: [(&str, &str, &Value); 6] = [
            ("om_object_type", UPSERT_OBJECTS, &serde_json::to_value(&draft.object_types).unwrap_or_default()),
            ("om_link_type", UPSERT_LINKS, &serde_json::to_value(&draft.link_types).unwrap_or_default()),
            ("om_interface", UPSERT_INTERFACES, &serde_json::to_value(&draft.interfaces).unwrap_or_default()),
            ("om_shared_property", UPSERT_SHARED, &serde_json::to_value(&draft.shared_properties).unwrap_or_default()),
            ("om_action_type", UPSERT_ACTIONS, &serde_json::to_value(&draft.action_types).unwrap_or_default()),
            ("om_function", UPSERT_FUNCTIONS, &serde_json::to_value(&draft.functions).unwrap_or_default()),
        ];
        for (table, sql, arr) in batches {
            if arr.as_array().is_none_or(|a| a.is_empty()) {
                continue;
            }
            self.exec_tx(txn, sql, vec![json_param(arr)], table).await?;
        }
        counts.objects_upserted = draft.object_types.len();
        counts.links_upserted = draft.link_types.len();
        counts.interfaces_upserted = draft.interfaces.len();
        counts.shared_upserted = draft.shared_properties.len();
        counts.actions_upserted = draft.action_types.len();
        counts.functions_upserted = draft.functions.len();

        // 2. views 应用：upsert 草稿全量（DO UPDATE **不含 layout**——已存在行保留 live 布局；
        //    新行用草稿布局作缺省）。
        if !draft.views.is_empty() {
            let arr = serde_json::to_value(&draft.views)
                .map_err(|e| StoreError::Backend(format!("views 序列化失败: {e}")))?;
            self.exec_tx(txn, UPSERT_VIEWS, vec![json_param(&arr)], "om_view_publish")
                .await?;
        }
        counts.views_upserted = draft.views.len();

        // 3. views 删除：live manual 行有、草稿无 → 删除（整体 diff 口径）；
        //    **auto 行豁免**——它只是布局物化产物，fork 后新落行的 auto 行不被发布回吞。
        let live_view_names = self.view_names_tx(txn).await?;
        let draft_view_names: std::collections::BTreeSet<String> =
            draft.views.iter().map(|v| v.api_name.clone()).collect();
        let stale_manual: Vec<String> = live_view_names
            .manual
            .into_iter()
            .filter(|n| !draft_view_names.contains(n))
            .collect();
        if !stale_manual.is_empty() {
            self.exec_tx(
                txn,
                "DELETE FROM om_view WHERE source='manual' AND api_name = ANY($1)",
                vec![names_param(&stale_manual)],
                "om_view_publish_delete",
            )
            .await?;
        }
        counts.views_removed = stale_manual.len();

        // 4. 六类派生删除集 = live − 草稿（权威语义，方案 §2.4 裁决 1：diff 预览 = 实际变更）。
        let deletions = self.derive_deletions_tx(txn, draft).await?;
        counts.deletions = deletions.len();
        self.apply_deletions_tx(txn, &deletions).await?;

        // 5. 读回「发布时刻全量」快照（txn 内最终态）。
        let snapshot = self.snapshot_full_tx(txn).await?;
        let rev = snapshot_fingerprint(&snapshot);

        // 6. rev 去重：与最新版本指纹相同 → 不插新版本（「无实质变更」明示，方案 §2.5）。
        let latest = self.latest_rev().await?;
        let (version, deduped) = match latest {
            Some((lv, lrev)) if lrev == rev => (lv, true),
            _ => {
                let next = latest.map_or(1, |(lv, _)| lv + 1);
                self.exec_tx(
                    txn,
                    "INSERT INTO om_version (version, rev, summary, snapshot, published_by, published_at) \
                     VALUES ($1, $2, $3, $4, $5, $6)",
                    vec![
                        DataValue::Int(next as i64),
                        DataValue::String(rev.clone()),
                        DataValue::String(summary.to_string()),
                        json_param(&snapshot),
                        crate::store::opt_str(&published_by),
                        DataValue::DateTime(Utc::now()),
                    ],
                    "om_version_publish",
                )
                .await?;
                (next, false)
            }
        };

        // 7. 草稿行原子删除（重置 = 下次 fork 从新 live 重建，内容全等、base_rev 推进）。
        self.exec_tx(txn, "DELETE FROM om_draft WHERE id = 1", vec![], "om_draft_reset")
            .await?;

        Ok(PublishOutcome { version, rev, deduped, counts })
    }

    /// 事务内派生删除集：live − 草稿（六类元素；restore 减法同口径）。
    async fn derive_deletions_tx(
        &self,
        txn: &str,
        draft: &DraftContent,
    ) -> StoreResult<Vec<DeletionRef>> {
        let live = self.snapshot_full_tx(txn).await?;
        let draft_snap = draft.to_snapshot_value();
        Ok(cmx_onto_model::derive_deletions(&live, &draft_snap))
    }

    /// 应用删除集（复用元素级删除既有级联语义：删接口同事务清各对象 implements 引用）。
    async fn apply_deletions_tx(&self, txn: &str, deletions: &[DeletionRef]) -> StoreResult<()> {
        let by_kind = |kind: &str| -> Vec<String> {
            deletions
                .iter()
                .filter(|d| d.kind == kind)
                .map(|d| d.api_name.clone())
                .collect()
        };
        let batch_delete = async |txn: &str, table: &str, names: &[String]| -> StoreResult<()> {
            if names.is_empty() {
                return Ok(());
            }
            let sql = format!("DELETE FROM {table} WHERE api_name = ANY($1)");
            self.exec_tx(txn, &sql, vec![names_param(names)], table).await?;
            Ok(())
        };
        batch_delete(txn, "om_object_type", &by_kind(KIND_OBJECT)).await?;
        batch_delete(txn, "om_link_type", &by_kind(KIND_LINK)).await?;
        // 接口：删行 + 同事务级联清 implements（对齐 delete_interface 的 B1 语义）。
        let ifaces = by_kind(KIND_INTERFACE);
        if !ifaces.is_empty() {
            batch_delete(txn, "om_interface", &ifaces).await?;
            for name in &ifaces {
                self.exec_tx(
                    txn,
                    "UPDATE om_object_type SET implements = implements - $1::text WHERE implements ? $1",
                    vec![DataValue::String(name.clone())],
                    "om_interface_cascade",
                )
                .await?;
            }
        }
        batch_delete(txn, "om_shared_property", &by_kind(KIND_SHARED)).await?;
        batch_delete(txn, "om_action_type", &by_kind(KIND_ACTION)).await?;
        batch_delete(txn, "om_function", &by_kind(KIND_FUNCTION)).await?;
        Ok(())
    }

    // ─────────────────── 事务内读助手 ───────────────────

    async fn view_names_tx(&self, txn: &str) -> StoreResult<ViewNameSets> {
        let ds = self
            .query_tx(
                txn,
                "SELECT api_name, source FROM om_view",
                vec![],
                "om_view_names",
            )
            .await?;
        let s = ds.schema.as_ref();
        let mut out = ViewNameSets::default();
        for row in ds.iter() {
            let name = get_string(row, s, "api_name")?;
            if get_opt_string(row, s, "source").unwrap_or_default() == "auto" {
                out.auto.push(name);
            } else {
                out.manual.push(name);
            }
        }
        Ok(out)
    }

    // 六类全量定义（免事务版并行 + 事务内串行两套；SELECT 同清单、仅执行通路不同）。

    pub async fn list_object_defs(&self, _tenant: &str) -> StoreResult<Vec<ObjectTypeDef>> {
        let ds = self
            .query(
                &format!("{OBJECT_DEF_SELECT} ORDER BY api_name"),
                vec![],
                "om_object_type_full",
            )
            .await?;
        rows_to_object_defs(&ds)
    }

    async fn list_object_defs_tx(&self, txn: &str) -> StoreResult<Vec<ObjectTypeDef>> {
        let ds = self
            .query_tx(txn, &format!("{OBJECT_DEF_SELECT} ORDER BY api_name"), vec![], "om_object_type_full_tx")
            .await?;
        rows_to_object_defs(&ds)
    }

    pub async fn list_link_defs(&self, _tenant: &str) -> StoreResult<Vec<LinkTypeDef>> {
        let ds = self
            .query(&format!("{LINK_DEF_SELECT} ORDER BY api_name"), vec![], "om_link_type_full")
            .await?;
        rows_to_link_defs(&ds)
    }

    async fn list_link_defs_tx(&self, txn: &str) -> StoreResult<Vec<LinkTypeDef>> {
        let ds = self
            .query_tx(txn, &format!("{LINK_DEF_SELECT} ORDER BY api_name"), vec![], "om_link_type_full_tx")
            .await?;
        rows_to_link_defs(&ds)
    }

    pub async fn list_interface_defs(&self, _tenant: &str) -> StoreResult<Vec<InterfaceDef>> {
        let ds = self
            .query(&format!("{INTERFACE_DEF_SELECT} ORDER BY api_name"), vec![], "om_interface_full")
            .await?;
        rows_to_interface_defs(&ds)
    }

    async fn list_interface_defs_tx(&self, txn: &str) -> StoreResult<Vec<InterfaceDef>> {
        let ds = self
            .query_tx(txn, &format!("{INTERFACE_DEF_SELECT} ORDER BY api_name"), vec![], "om_interface_full_tx")
            .await?;
        rows_to_interface_defs(&ds)
    }

    pub async fn list_shared_defs(&self, _tenant: &str) -> StoreResult<Vec<SharedPropertyTypeDef>> {
        let ds = self
            .query(&format!("{SHARED_DEF_SELECT} ORDER BY api_name"), vec![], "om_shared_full")
            .await?;
        rows_to_shared_defs(&ds)
    }

    async fn list_shared_defs_tx(&self, txn: &str) -> StoreResult<Vec<SharedPropertyTypeDef>> {
        let ds = self
            .query_tx(txn, &format!("{SHARED_DEF_SELECT} ORDER BY api_name"), vec![], "om_shared_full_tx")
            .await?;
        rows_to_shared_defs(&ds)
    }

    pub async fn list_action_defs(&self, _tenant: &str) -> StoreResult<Vec<ActionTypeDef>> {
        let ds = self
            .query(&format!("{ACTION_DEF_SELECT} ORDER BY api_name"), vec![], "om_action_full")
            .await?;
        rows_to_action_defs(&ds)
    }

    async fn list_action_defs_tx(&self, txn: &str) -> StoreResult<Vec<ActionTypeDef>> {
        let ds = self
            .query_tx(txn, &format!("{ACTION_DEF_SELECT} ORDER BY api_name"), vec![], "om_action_full_tx")
            .await?;
        rows_to_action_defs(&ds)
    }

    pub async fn list_function_defs(&self, _tenant: &str) -> StoreResult<Vec<FunctionDef>> {
        let ds = self
            .query(&format!("{FUNCTION_DEF_SELECT} ORDER BY api_name"), vec![], "om_function_full")
            .await?;
        rows_to_function_defs(&ds)
    }

    async fn list_function_defs_tx(&self, txn: &str) -> StoreResult<Vec<FunctionDef>> {
        let ds = self
            .query_tx(txn, &format!("{FUNCTION_DEF_SELECT} ORDER BY api_name"), vec![], "om_function_full_tx")
            .await?;
        rows_to_function_defs(&ds)
    }

    /// om_view 全量行定义（含 layout；fork 拷贝与发布快照用；不含读时派生虚拟条目）。
    pub async fn list_view_defs(&self, _tenant: &str) -> StoreResult<Vec<SceneViewDef>> {
        let ds = self
            .query(&format!("{VIEW_DEF_SELECT} ORDER BY api_name"), vec![], "om_view_full")
            .await?;
        rows_to_view_defs(&ds)
    }

    async fn list_view_defs_tx(&self, txn: &str) -> StoreResult<Vec<SceneViewDef>> {
        let ds = self
            .query_tx(txn, &format!("{VIEW_DEF_SELECT} ORDER BY api_name"), vec![], "om_view_full_tx")
            .await?;
        rows_to_view_defs(&ds)
    }

    // ─────────────────── 事务版 exec/query 薄封装 ───────────────────

    async fn exec_tx(&self, txn: &str, sql: &str, params: Vec<DataValue>, ds_id: &str) -> StoreResult<u64> {
        let _ = ds_id;
        execute_sql_with_params(&self.db_id, Some(txn), sql, SqlParams::DataValues(params))
            .await
            .map_err(|e| StoreError::Backend(format!("执行失败: {e}")))
    }

    async fn query_tx(
        &self,
        txn: &str,
        sql: &str,
        params: Vec<DataValue>,
        ds_id: &str,
    ) -> StoreResult<DataSet> {
        query_sql_with_params(&self.db_id, Some(txn), sql, SqlParams::DataValues(params), ds_id)
            .await
            .map_err(|e| StoreError::Backend(format!("查询失败: {e}")))
    }
}

// ————————————————————————— SELECT / UPSERT 语句常量 —————————————————————————

const OBJECT_DEF_SELECT: &str =
    "SELECT api_name, display_name, description, icon, color, primary_key, title_property, \
     status, properties, implements, dam, doc_type, datasource, cmx_origin, version FROM om_object_type";
const LINK_DEF_SELECT: &str =
    "SELECT api_name, display_name, cardinality, object_type_a, object_type_b, role_a, role_b, \
     backing, status FROM om_link_type";
const INTERFACE_DEF_SELECT: &str =
    "SELECT api_name, display_name, properties, extends, status FROM om_interface";
const SHARED_DEF_SELECT: &str =
    "SELECT api_name, display_name, base_type, semantic_type, description FROM om_shared_property";
const ACTION_DEF_SELECT: &str =
    "SELECT api_name, display_name, description, parameters, logic, validations, side_effects, \
     function_backing, status FROM om_action_type";
const FUNCTION_DEF_SELECT: &str =
    "SELECT api_name, display_name, runtime, kind, inputs, output, body, description, status FROM om_function";
const VIEW_DEF_SELECT: &str =
    "SELECT api_name, display_name, description, dam, members, source, layout, version FROM om_view";

/// 六类批量 upsert：`jsonb_to_recordset`（camelCase 引号别名对位 def 字段）单语句。
/// created_at 仅插入时定值；version 用草稿携带值（保持 B0 链路）。
const UPSERT_OBJECTS: &str = r#"INSERT INTO om_object_type
    (api_name, display_name, description, icon, color, primary_key, title_property, status,
     properties, implements, dam, doc_type, datasource, cmx_origin, version, created_at, updated_at)
SELECT "apiName", COALESCE("displayName",''), COALESCE("description",''), COALESCE("icon",''),
     COALESCE("color",''), COALESCE("primaryKey",''), COALESCE("titleProperty",''),
     COALESCE("status",'experimental'), COALESCE("properties",'[]'::jsonb), COALESCE("implements",'[]'::jsonb),
     COALESCE("dam",'{}'::jsonb), COALESCE("docType",'{}'::jsonb), "datasource", "cmxOrigin",
     COALESCE(NULLIF("version",0),1), now(), now()
FROM jsonb_to_recordset($1::jsonb) AS x("apiName" text,"displayName" text,"description" text,"icon" text,
     "color" text,"primaryKey" text,"titleProperty" text,"status" text,"properties" jsonb,"implements" jsonb,
     "dam" jsonb,"docType" jsonb,"datasource" jsonb,"cmxOrigin" jsonb,"version" integer)
ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, description=EXCLUDED.description,
     icon=EXCLUDED.icon, color=EXCLUDED.color, primary_key=EXCLUDED.primary_key, title_property=EXCLUDED.title_property,
     status=EXCLUDED.status, properties=EXCLUDED.properties, implements=EXCLUDED.implements, dam=EXCLUDED.dam,
     doc_type=EXCLUDED.doc_type, datasource=EXCLUDED.datasource, cmx_origin=EXCLUDED.cmx_origin,
     version=EXCLUDED.version, updated_at=EXCLUDED.updated_at"#;

const UPSERT_LINKS: &str = r#"INSERT INTO om_link_type
    (api_name, display_name, cardinality, object_type_a, object_type_b, role_a, role_b, backing, status, created_at, updated_at)
SELECT "apiName", COALESCE("displayName",''), COALESCE("cardinality",'oneToMany'), COALESCE("objectTypeA",''),
     COALESCE("objectTypeB",''), COALESCE("roleA",''), COALESCE("roleB",''), COALESCE("backing",'{}'::jsonb),
     COALESCE("status",'experimental'), now(), now()
FROM jsonb_to_recordset($1::jsonb) AS x("apiName" text,"displayName" text,"cardinality" text,"objectTypeA" text,
     "objectTypeB" text,"roleA" text,"roleB" text,"backing" jsonb,"status" text)
ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, cardinality=EXCLUDED.cardinality,
     object_type_a=EXCLUDED.object_type_a, object_type_b=EXCLUDED.object_type_b, role_a=EXCLUDED.role_a,
     role_b=EXCLUDED.role_b, backing=EXCLUDED.backing, status=EXCLUDED.status, updated_at=EXCLUDED.updated_at"#;

const UPSERT_INTERFACES: &str = r#"INSERT INTO om_interface
    (api_name, display_name, properties, extends, status, created_at, updated_at)
SELECT "apiName", COALESCE("displayName",''), COALESCE("properties",'[]'::jsonb), COALESCE("extends",'[]'::jsonb),
     COALESCE("status",'experimental'), now(), now()
FROM jsonb_to_recordset($1::jsonb) AS x("apiName" text,"displayName" text,"properties" jsonb,"extends" jsonb,"status" text)
ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, properties=EXCLUDED.properties,
     extends=EXCLUDED.extends, status=EXCLUDED.status, updated_at=EXCLUDED.updated_at"#;

const UPSERT_SHARED: &str = r#"INSERT INTO om_shared_property
    (api_name, display_name, base_type, semantic_type, description, created_at, updated_at)
SELECT "apiName", COALESCE("displayName",''), COALESCE("baseType",'string'), "semanticType",
     COALESCE("description",''), now(), now()
FROM jsonb_to_recordset($1::jsonb) AS x("apiName" text,"displayName" text,"baseType" text,"semanticType" text,"description" text)
ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, base_type=EXCLUDED.base_type,
     semantic_type=EXCLUDED.semantic_type, description=EXCLUDED.description, updated_at=EXCLUDED.updated_at"#;

const UPSERT_ACTIONS: &str = r#"INSERT INTO om_action_type
    (api_name, display_name, description, parameters, logic, validations, side_effects, function_backing, status, created_at, updated_at)
SELECT "apiName", COALESCE("displayName",''), COALESCE("description",''), COALESCE("parameters",'[]'::jsonb),
     COALESCE("logic",'[]'::jsonb), COALESCE("validations",'[]'::jsonb), COALESCE("sideEffects",'[]'::jsonb),
     "functionBacking", COALESCE("status",'experimental'), now(), now()
FROM jsonb_to_recordset($1::jsonb) AS x("apiName" text,"displayName" text,"description" text,"parameters" jsonb,
     "logic" jsonb,"validations" jsonb,"sideEffects" jsonb,"functionBacking" text,"status" text)
ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, description=EXCLUDED.description,
     parameters=EXCLUDED.parameters, logic=EXCLUDED.logic, validations=EXCLUDED.validations,
     side_effects=EXCLUDED.side_effects, function_backing=EXCLUDED.function_backing, status=EXCLUDED.status,
     updated_at=EXCLUDED.updated_at"#;

const UPSERT_FUNCTIONS: &str = r#"INSERT INTO om_function
    (api_name, display_name, runtime, kind, inputs, output, body, description, status, created_at, updated_at)
SELECT "apiName", COALESCE("displayName",''), COALESCE("runtime",'feel'), COALESCE("kind",'query'),
     COALESCE("inputs",'[]'::jsonb), COALESCE("output",'{}'::jsonb), COALESCE("body",''),
     COALESCE("description",''), COALESCE("status",'experimental'), now(), now()
FROM jsonb_to_recordset($1::jsonb) AS x("apiName" text,"displayName" text,"runtime" text,"kind" text,
     "inputs" jsonb,"output" jsonb,"body" text,"description" text,"status" text)
ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, runtime=EXCLUDED.runtime,
     kind=EXCLUDED.kind, inputs=EXCLUDED.inputs, output=EXCLUDED.output, body=EXCLUDED.body,
     description=EXCLUDED.description, status=EXCLUDED.status, updated_at=EXCLUDED.updated_at"#;

/// views 批量 upsert：**DO UPDATE 有意不含 layout 列**——发布保留 live 侧布局（方案 §2.4
/// layout 豁免双轨）；新建行用草稿 layout 作缺省；version 冲突时递增。
const UPSERT_VIEWS: &str = r#"INSERT INTO om_view
    (api_name, display_name, description, dam, members, source, layout, version, created_at, updated_at)
SELECT "apiName", COALESCE("displayName",''), COALESCE("description",''), COALESCE("dam",'{}'::jsonb),
     COALESCE("members",'{"objects":[],"interfaces":[]}'::jsonb), COALESCE("source",'manual'),
     COALESCE("layout",'{}'::jsonb), 1, now(), now()
FROM jsonb_to_recordset($1::jsonb) AS x("apiName" text,"displayName" text,"description" text,"dam" jsonb,
     "members" jsonb,"source" text,"layout" jsonb)
ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, description=EXCLUDED.description,
     dam=EXCLUDED.dam, members=EXCLUDED.members, source=EXCLUDED.source, version=om_view.version + 1,
     updated_at=now()"#;

// ————————————————————————— 行转换 / 参数助手 —————————————————————————

#[derive(Default)]
struct ViewNameSets {
    auto: Vec<String>,
    manual: Vec<String>,
}

fn json_param(v: &Value) -> DataValue {
    DataValue::Json(v.to_string())
}

/// apiName 列表 → `= ANY($1)` / `<> ALL($1)` 参数（与既有 batch 同形状）。
fn names_param(names: &[String]) -> DataValue {
    DataValue::Array(names.iter().map(|n| DataValue::String(n.clone())).collect())
}

fn rows_to_object_defs(ds: &DataSet) -> StoreResult<Vec<ObjectTypeDef>> {
    let s = ds.schema.as_ref();
    ds.iter().map(|row| crate::store::object_def_from_row(row, s)).collect()
}

fn rows_to_link_defs(ds: &DataSet) -> StoreResult<Vec<LinkTypeDef>> {
    let s = ds.schema.as_ref();
    ds.iter()
        .map(|row| {
            Ok(LinkTypeDef {
                api_name: get_string(row, s, "api_name")?,
                display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
                cardinality: crate::store::str_to_enum(&get_opt_string(row, s, "cardinality").unwrap_or_default()),
                object_type_a: get_opt_string(row, s, "object_type_a").unwrap_or_default(),
                object_type_b: get_opt_string(row, s, "object_type_b").unwrap_or_default(),
                role_a: get_opt_string(row, s, "role_a").unwrap_or_default(),
                role_b: get_opt_string(row, s, "role_b").unwrap_or_default(),
                backing: crate::store::get_json(row, s, "backing").unwrap_or(Value::Null),
                status: crate::store::parse_status(row, s),
            })
        })
        .collect()
}

fn rows_to_interface_defs(ds: &DataSet) -> StoreResult<Vec<InterfaceDef>> {
    let s = ds.schema.as_ref();
    ds.iter()
        .map(|row| {
            Ok(InterfaceDef {
                api_name: get_string(row, s, "api_name")?,
                display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
                properties: crate::store::get_json(row, s, "properties")
                    .ok()
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default(),
                extends: crate::store::get_json(row, s, "extends")
                    .ok()
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default(),
                status: crate::store::parse_status(row, s),
            })
        })
        .collect()
}

fn rows_to_shared_defs(ds: &DataSet) -> StoreResult<Vec<SharedPropertyTypeDef>> {
    let s = ds.schema.as_ref();
    ds.iter()
        .map(|row| {
            Ok(SharedPropertyTypeDef {
                api_name: get_string(row, s, "api_name")?,
                display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
                base_type: crate::store::str_to_enum(&get_opt_string(row, s, "base_type").unwrap_or_default()),
                semantic_type: get_opt_string(row, s, "semantic_type"),
                description: get_opt_string(row, s, "description").unwrap_or_default(),
            })
        })
        .collect()
}

fn rows_to_action_defs(ds: &DataSet) -> StoreResult<Vec<ActionTypeDef>> {
    let s = ds.schema.as_ref();
    ds.iter()
        .map(|row| {
            Ok(ActionTypeDef {
                api_name: get_string(row, s, "api_name")?,
                display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
                description: get_opt_string(row, s, "description").unwrap_or_default(),
                parameters: crate::store::get_json(row, s, "parameters").unwrap_or(Value::Null),
                logic: crate::store::get_json(row, s, "logic").unwrap_or(Value::Null),
                validations: crate::store::get_json(row, s, "validations").unwrap_or(Value::Null),
                side_effects: crate::store::get_json(row, s, "side_effects").unwrap_or(Value::Null),
                function_backing: get_opt_string(row, s, "function_backing"),
                status: crate::store::parse_status(row, s),
            })
        })
        .collect()
}

fn rows_to_function_defs(ds: &DataSet) -> StoreResult<Vec<FunctionDef>> {
    let s = ds.schema.as_ref();
    ds.iter()
        .map(|row| {
            Ok(FunctionDef {
                api_name: get_string(row, s, "api_name")?,
                display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
                runtime: crate::store::str_to_enum(&get_opt_string(row, s, "runtime").unwrap_or_default()),
                kind: crate::store::str_to_enum(&get_opt_string(row, s, "kind").unwrap_or_default()),
                inputs: crate::store::get_json(row, s, "inputs").unwrap_or(Value::Null),
                output: crate::store::get_json(row, s, "output").unwrap_or(Value::Null),
                body: get_opt_string(row, s, "body").unwrap_or_default(),
                description: get_opt_string(row, s, "description").unwrap_or_default(),
                status: crate::store::parse_status(row, s),
            })
        })
        .collect()
}

fn rows_to_view_defs(ds: &DataSet) -> StoreResult<Vec<SceneViewDef>> {
    let s = ds.schema.as_ref();
    ds.iter()
        .map(|row| {
            Ok(SceneViewDef {
                api_name: get_string(row, s, "api_name")?,
                display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
                description: get_opt_string(row, s, "description").unwrap_or_default(),
                dam: crate::store::get_json(row, s, "dam")
                    .ok()
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default(),
                members: crate::store::get_json(row, s, "members")
                    .ok()
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default(),
                source: crate::view_store::str_to_view_source(&get_opt_string(row, s, "source").unwrap_or_default()),
                layout: crate::store::get_json(row, s, "layout").unwrap_or(Value::Null),
                version: get_i64(row, s, "version") as u32,
            })
        })
        .collect()
}

/// 引用 ELEMENT_KINDS 确保常量被使用（derive_deletions_tx 借模型层同名函数；此处仅纪律性触达）。
const _: &[&str] = ELEMENT_KINDS;
