//! 存档/回滚 + 全量快照（`snapshot_store`）—— [`PgOntologyStore`] 的 inherent 方法块
//! （直改 live 架构：编辑直写 om_*，存档 = live → om_version 检查点，回滚 = 快照 → live）。
//!
//! 关键语义（方案钉死，实现据此）：
//! - **存档**：单事务内 `snapshot_full`（六类 + om_view，views 剥离 layout）→ rev 指纹与
//!   最新版本比对（相同 = `deduped` 不插行）→ `ON CONFLICT (version) DO NOTHING` 插行，
//!   撞号重读重试（≤3 次）——并发存档安全。
//! - **回滚**：单事务内 目标快照六类批量 upsert（`jsonb_to_recordset` 单语句/类）→ views
//!   应用（**已存在行不动 layout 列**；live manual 行目标无 → 删除，**auto 行豁免**）→
//!   派生删除集（live − 目标，权威语义）批量删除 + 既有级联（删接口同事务清 implements）
//!   → 大规模删除护栏（超阈值须显式确认）→ 读回最终态快照 → **回滚留痕**（再插一条
//!   「回滚到 v{n}」存档，与最新 rev 相同则去重不插）。
//! - **引用检查**：高危删除（对象类型/共享属性）的被引用查询（关系/动作编辑/视图成员/
//!   对象属性/接口契约）。

use chrono::Utc;
use cmx_core::model::cell::DataValue;
use cmx_core::model::data::dataset::DataSet;
use cmx_database_pg::{execute_sql_with_params, get_default_pg_db_manager, query_sql_with_params, SqlParams};
use cmx_onto_model::{
    derive_deletions, element_total, snapshot_fingerprint, ActionTypeDef, DeletionRef,
    FunctionDef, InterfaceDef, LinkTypeDef, ObjectTypeDef, SceneViewDef, SharedPropertyTypeDef,
    StoreError, StoreResult, ELEMENT_KINDS, KIND_ACTION, KIND_FUNCTION, KIND_INTERFACE,
    KIND_LINK, KIND_OBJECT, KIND_SHARED,
};
use serde::Serialize;
use serde_json::Value;

use crate::store::{get_i64, get_opt_string, get_string, PgOntologyStore};

/// 存档结果（含去重标记，供前端 toast）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveOutcome {
    /// 存档版本号（去重命中时 = 最新既有版本）。
    pub version: u32,
    pub rev: String,
    /// true = 指纹与最新版本相同，未插新行（「无实质变更」明示）。
    pub deduped: bool,
    pub summary: String,
}

/// 回滚应用变更计数（与 diff 预览同口径可对账）。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyCounts {
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

/// 回滚结果（应用计数 + 留痕存档版本）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOutcome {
    pub restored_from: u32,
    /// 回滚留痕存档版本（与 live 全等去重时 = 最新版本且 deduped=true）。
    pub archive_version: u32,
    pub archive_deduped: bool,
    pub counts: ApplyCounts,
}

impl PgOntologyStore {
    // ─────────────────── 存档 / 回滚 ───────────────────

    /// 存档：单事务把当前 live 打成 om_version 检查点（rev 去重 + 并发安全）。
    pub async fn archive_snapshot(
        &self,
        _tenant: &str,
        summary: &str,
        archived_by: Option<String>,
    ) -> StoreResult<ArchiveOutcome> {
        let manager = get_default_pg_db_manager();
        let txn_ctx = manager.get_transaction_context();
        let txn = txn_ctx
            .begin(&self.db_id)
            .await
            .map_err(|e| StoreError::Backend(format!("开启存档事务失败: {e}")))?;
        let outcome = self.archive_snapshot_tx(&txn, summary, &archived_by).await;
        match outcome {
            Ok(out) => {
                txn_ctx
                    .commit(&txn)
                    .await
                    .map_err(|e| StoreError::Backend(format!("提交存档事务失败: {e}")))?;
                Ok(out)
            }
            Err(e) => {
                let _ = txn_ctx.rollback(&txn).await;
                Err(e)
            }
        }
    }

    async fn archive_snapshot_tx(
        &self,
        txn: &str,
        summary: &str,
        archived_by: &Option<String>,
    ) -> StoreResult<ArchiveOutcome> {
        let mut snapshot = self.snapshot_full_tx(txn).await?;
        strip_view_layout(&mut snapshot);
        let (version, rev, deduped) =
            self.insert_version_tx(txn, &snapshot, summary, archived_by).await?;
        Ok(ArchiveOutcome { version, rev, deduped, summary: summary.to_string() })
    }

    /// 回滚：单事务把历史快照整体恢复回 live（upsert → views → 派生删除 + 护栏 → 留痕存档）。
    pub async fn restore_snapshot_to_live(
        &self,
        _tenant: &str,
        restored_from: u32,
        target: &Value,
        archived_by: Option<String>,
        confirm_mass_delete: bool,
    ) -> StoreResult<RestoreOutcome> {
        let manager = get_default_pg_db_manager();
        let txn_ctx = manager.get_transaction_context();
        let txn = txn_ctx
            .begin(&self.db_id)
            .await
            .map_err(|e| StoreError::Backend(format!("开启回滚事务失败: {e}")))?;
        let outcome =
            self.restore_snapshot_to_live_tx(&txn, restored_from, target, archived_by, confirm_mass_delete).await;
        match outcome {
            Ok(out) => {
                txn_ctx
                    .commit(&txn)
                    .await
                    .map_err(|e| StoreError::Backend(format!("提交回滚事务失败: {e}")))?;
                Ok(out)
            }
            Err(e) => {
                let _ = txn_ctx.rollback(&txn).await;
                Err(e)
            }
        }
    }

    async fn restore_snapshot_to_live_tx(
        &self,
        txn: &str,
        restored_from: u32,
        target: &Value,
        archived_by: Option<String>,
        confirm_mass_delete: bool,
    ) -> StoreResult<RestoreOutcome> {
        let mut counts = ApplyCounts::default();

        // 1. 六类批量 upsert（target 数组直接进 jsonb_to_recordset，字段 camelCase 对位）。
        let batches: [(&str, &str, Option<&Value>); 6] = [
            ("om_object_type", UPSERT_OBJECTS, target.get("objectTypes")),
            ("om_link_type", UPSERT_LINKS, target.get("linkTypes")),
            ("om_interface", UPSERT_INTERFACES, target.get("interfaces")),
            ("om_shared_property", UPSERT_SHARED, target.get("sharedProperties")),
            ("om_action_type", UPSERT_ACTIONS, target.get("actionTypes")),
            ("om_function", UPSERT_FUNCTIONS, target.get("functions")),
        ];
        for (table, sql, arr) in batches {
            let Some(arr) = arr else { continue };
            if arr.as_array().is_none_or(|a| a.is_empty()) {
                continue;
            }
            self.exec_tx(txn, sql, vec![json_param(arr)], table).await?;
        }
        counts.objects_upserted = arr_len(target.get("objectTypes"));
        counts.links_upserted = arr_len(target.get("linkTypes"));
        counts.interfaces_upserted = arr_len(target.get("interfaces"));
        counts.shared_upserted = arr_len(target.get("sharedProperties"));
        counts.actions_upserted = arr_len(target.get("actionTypes"));
        counts.functions_upserted = arr_len(target.get("functions"));

        // 2. views 应用：upsert 目标全量（DO UPDATE **不含 layout 列**——保留 live 布局；
        //    新行 layout 缺省 '{}'——历史快照已剥离 layout）。
        if let Some(views) = target.get("views") {
            if let Some(arr) = views.as_array() {
                if !arr.is_empty() {
                    self.exec_tx(txn, UPSERT_VIEWS, vec![json_param(views)], "om_view_restore")
                        .await?;
                }
            }
        }
        counts.views_upserted = arr_len(target.get("views"));

        // 3. views 删除：live manual 行有、目标无 → 删除；**auto 行豁免**（布局物化产物）。
        let live_view_names = self.view_names_tx(txn).await?;
        let target_view_names: std::collections::BTreeSet<String> = target
            .get("views")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.get("apiName").and_then(|n| n.as_str()).map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let stale_manual: Vec<String> = live_view_names
            .manual
            .into_iter()
            .filter(|n| !target_view_names.contains(n))
            .collect();
        if !stale_manual.is_empty() {
            self.exec_tx(
                txn,
                "DELETE FROM om_view WHERE source='manual' AND api_name = ANY($1)",
                vec![names_param(&stale_manual)],
                "om_view_restore_delete",
            )
            .await?;
        }
        counts.views_removed = stale_manual.len();

        // 4. 派生删除集 = live − 目标（权威语义）+ 大规模删除护栏。
        let live = self.snapshot_full_tx(txn).await?;
        let deletions = derive_deletions(&live, target);
        counts.deletions = deletions.len();
        let threshold = std::cmp::max(50, element_total(&live) / 5);
        if deletions.len() > threshold && !confirm_mass_delete {
            return Err(StoreError::Conflict(format!(
                "本次回滚将删除 {} 个元素（live 共 {}，超过护栏阈值 {threshold}）。\
                 确属批量回滚请在请求带 confirmMassDelete=true 重发",
                deletions.len(),
                element_total(&live)
            )));
        }
        self.apply_deletions_tx(txn, &deletions).await?;

        // 5. 读回最终态快照 → 回滚留痕存档（「回滚到 v{n}」；与 live 全等则去重不插行）。
        let mut snapshot = self.snapshot_full_tx(txn).await?;
        strip_view_layout(&mut snapshot);
        let summary = format!("回滚到 v{restored_from}");
        let (archive_version, _rev, archive_deduped) =
            self.insert_version_tx(txn, &snapshot, &summary, &archived_by).await?;

        Ok(RestoreOutcome {
            restored_from,
            archive_version,
            archive_deduped,
            counts,
        })
    }

    /// 插入存档行（事务内；rev 去重 + `ON CONFLICT (version) DO NOTHING` 撞号重读重试）。
    /// 返回 (版本号, rev, 是否去重)。
    async fn insert_version_tx(
        &self,
        txn: &str,
        snapshot: &Value,
        summary: &str,
        archived_by: &Option<String>,
    ) -> StoreResult<(u32, String, bool)> {
        let rev = snapshot_fingerprint(snapshot);
        for _ in 0..3 {
            let latest = self.latest_rev_tx(txn).await?;
            if let Some((lv, lrev)) = &latest {
                if lrev == &rev {
                    return Ok((*lv, rev, true));
                }
            }
            let next = latest.as_ref().map_or(1u32, |(lv, _)| lv.saturating_add(1));
            let n = self
                .exec_tx(
                    txn,
                    "INSERT INTO om_version (version, rev, summary, snapshot, published_by, published_at) \
                     VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (version) DO NOTHING",
                    vec![
                        DataValue::Int(next as i64),
                        DataValue::String(rev.clone()),
                        DataValue::String(summary.to_string()),
                        json_param(snapshot),
                        crate::store::opt_str(archived_by),
                        DataValue::DateTime(Utc::now()),
                    ],
                    "om_version_archive",
                )
                .await?;
            if n > 0 {
                return Ok((next, rev, false));
            }
            // 撞号（并发存档/回滚）→ 重读最新重算重试。
        }
        Err(StoreError::Backend(
            "存档版本号并发冲突（重试 3 次未成功），请稍后重试".into(),
        ))
    }

    /// 最新存档（事务内；按版本号取最大）。
    async fn latest_rev_tx(&self, txn: &str) -> StoreResult<Option<(u32, String)>> {
        let ds = self
            .query_tx(
                txn,
                "SELECT version, rev FROM om_version ORDER BY version DESC LIMIT 1",
                vec![],
                "om_ver_latest_tx",
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

    // ─────────────────── 高危删除引用检查 ───────────────────

    /// 对象类型的被引用清单：(kind, apiName)。关系两端 / 动作编辑规则 / 场景成员。
    pub async fn object_type_references(&self, api_name: &str) -> StoreResult<Vec<(String, String)>> {
        let ds = self
            .query(
                "SELECT 'linkType' AS kind, api_name FROM om_link_type \
                  WHERE object_type_a = $1 OR object_type_b = $1 \
                 UNION ALL \
                 SELECT 'actionType', api_name FROM om_action_type \
                  WHERE EXISTS (SELECT 1 FROM jsonb_array_elements(COALESCE(logic, '[]'::jsonb)) e \
                                WHERE e->>'objectType' = $1) \
                 UNION ALL \
                 SELECT 'view', api_name FROM om_view \
                  WHERE members->'objects' @> to_jsonb($1::text)",
                vec![DataValue::String(api_name.to_string())],
                "om_object_type_refs",
            )
            .await?;
        let s = ds.schema.as_ref();
        Ok(ds
            .iter()
            .map(|row| {
                (
                    get_string(row, s, "kind").unwrap_or_default(),
                    get_string(row, s, "api_name").unwrap_or_default(),
                )
            })
            .collect())
    }

    /// 共享属性的被引用清单：(kind, apiName)。对象属性 / 接口契约。
    pub async fn shared_property_references(&self, api_name: &str) -> StoreResult<Vec<(String, String)>> {
        let ds = self
            .query(
                "SELECT 'objectType' AS kind, api_name FROM om_object_type \
                  WHERE EXISTS (SELECT 1 FROM jsonb_array_elements(COALESCE(properties, '[]'::jsonb)) p \
                                WHERE p->>'sharedProperty' = $1) \
                 UNION ALL \
                 SELECT 'interface', api_name FROM om_interface \
                  WHERE properties @> to_jsonb($1::text)",
                vec![DataValue::String(api_name.to_string())],
                "om_shared_property_refs",
            )
            .await?;
        let s = ds.schema.as_ref();
        Ok(ds
            .iter()
            .map(|row| {
                (
                    get_string(row, s, "kind").unwrap_or_default(),
                    get_string(row, s, "api_name").unwrap_or_default(),
                )
            })
            .collect())
    }

    // ─────────────────── 全量快照（含 views；存档/回滚/指纹共用） ───────────────────

    /// 组装全量定义快照（七路：六类元素全量定义按 api_name 序 + om_view 全量行定义含 layout）。
    pub async fn snapshot_full(&self, _tenant: &str) -> StoreResult<Value> {
        let (objects, links, interfaces, shared, actions, functions, views) = tokio::try_join!(
            self.list_object_defs(_tenant),
            self.list_link_defs(_tenant),
            self.list_interface_defs(_tenant),
            self.list_shared_defs(_tenant),
            self.list_action_defs(_tenant),
            self.list_function_defs(_tenant),
            self.list_view_defs(_tenant),
        )?;
        Ok(Self::snapshot_value(objects, links, interfaces, shared, actions, functions, views))
    }

    /// 事务内读回快照（存档/回滚后组装「应用时刻全量」用；同 [`Self::snapshot_full`] 口径）。
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

    // ─────────────────── 事务内派生删除 / 视图名集 ───────────────────

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
        let ds = self.query(&format!("{LINK_DEF_SELECT} ORDER BY api_name"), vec![], "om_link_type_full").await?;
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

    /// om_view 全量行定义（含 layout；快照用；不含读时派生虚拟条目）。
    pub async fn list_view_defs(&self, _tenant: &str) -> StoreResult<Vec<SceneViewDef>> {
        let ds = self.query(&format!("{VIEW_DEF_SELECT} ORDER BY api_name"), vec![], "om_view_full").await?;
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

/// 存档快照瘦身：剥离 views 的 layout（不进指纹、回滚不恢复，重复入库纯浪费）。
fn strip_view_layout(snapshot: &mut Value) {
    if let Some(views) = snapshot.get_mut("views").and_then(|v| v.as_array_mut()) {
        for v in views.iter_mut() {
            if let Some(o) = v.as_object_mut() {
                o.remove("layout");
            }
        }
    }
}

fn arr_len(v: Option<&Value>) -> usize {
    v.and_then(|v| v.as_array()).map_or(0, |a| a.len())
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
/// created_at 仅插入时定值；version 用快照携带值（保持 B0 链路）。
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

/// views 批量 upsert：**DO UPDATE 有意不含 layout 列**——回滚保留 live 侧布局（layout 豁免双轨）；
/// 新行 layout 缺省 '{}'（历史快照已剥离 layout）；version 冲突时递增。
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
    #[allow(dead_code)]
    auto: Vec<String>,
    manual: Vec<String>,
}

fn json_param(v: &Value) -> DataValue {
    DataValue::Json(v.to_string())
}

/// apiName 列表 → `= ANY($1)` 参数。
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

/// 引用 ELEMENT_KINDS 确保常量被使用（derive_deletions 借模型层同名函数；此处仅纪律性触达）。
const _: &[&str] = ELEMENT_KINDS;
