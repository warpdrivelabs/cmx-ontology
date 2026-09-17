//! 资源级修订历史 + 生命周期状态流转 —— [`PgOntologyStore`] 的 inherent 方法块
//! （方案 20260917 §6.2 / §5.2-5.4）。
//!
//! - **修订管道**：七类资源每次保存 / transition / revert / restore 同事务追加一条
//!   `om_revision`（per-resource max+1，UNIQUE 兜底并发）；删除写墓碑（deleted=true）。
//! - **状态流转**：`set_status*` 是 transition 的唯一写入口（status + 弃用元数据四列同语句
//!   更新；离开 deprecated 即清空四列）；普通 save 路径不触达本模块。
//! - **场景引用**：sceneRefs 查询（删除确认框影响面）与级联清理（D10：场景是透镜不是容器）。

use cmx_core::model::cell::DataValue;
use cmx_core::model::data::dataset::DataSet;
use cmx_database_pg::get_default_pg_db_manager;
use cmx_onto_model::{DeprecationMeta, StoreError, StoreResult, TypeStatus};
use serde_json::Value;

use crate::store::{get_i64, get_opt_string, get_opt_ts, get_string, PgOntologyStore};

/// 级联流转写项（对象类型流转联动的关系类型；方案 §5.4 机械对齐矩阵）。
#[derive(Debug, Clone)]
pub struct CascadeWrite {
    pub api_name: String,
    pub target: TypeStatus,
    /// 级联降级时自动填充的弃用元数据（转出 deprecated 时为 None 即清空）。
    pub deprecation: Option<DeprecationMeta>,
    /// 级联后全量定义（修订 payload；app 层组装）。
    pub payload: Value,
}

/// 修订/流转的 resource_kind 值域（= 端点 kind 参数；与 KIND_* 清单键解耦）。
pub const REVISION_KINDS: &[&str] = &[
    "object",
    "link",
    "interface",
    "shared_property",
    "action",
    "function",
    "view",
];

/// kind → 定义表名（transition / 修订 payload 归属表）。
pub(crate) fn revision_table(kind: &str) -> Option<&'static str> {
    match kind {
        "object" => Some("om_object_type"),
        "link" => Some("om_link_type"),
        "interface" => Some("om_interface"),
        "shared_property" => Some("om_shared_property"),
        "action" => Some("om_action_type"),
        "function" => Some("om_function"),
        "view" => Some("om_view"),
        _ => None,
    }
}

/// 修订时间线条目（不含 payload——详情端点按 id 单取）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionMeta {
    pub id: i64,
    pub resource_kind: String,
    pub api_name: String,
    pub revision: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_note: Option<String>,
    pub changed_by: String,
    pub changed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub deleted: bool,
}

/// 已删除资源清单项（墓碑视图：每资源最新修订 deleted=true）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedResource {
    pub resource_kind: String,
    pub api_name: String,
    pub revision: i64,
    pub changed_by: String,
    pub changed_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl PgOntologyStore {
    // ─────────────────── 修订写入 ───────────────────

    /// 事务内追加一条修订（revision = per-resource max+1 单语句原子；UNIQUE 兜底并发——
    /// 撞号由调用方整个事务重试，见 save_store）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn append_revision_tx(
        &self,
        txn: &str,
        kind: &str,
        api_name: &str,
        payload: &Value,
        change_note: Option<&str>,
        changed_by: &str,
        deleted: bool,
    ) -> StoreResult<i64> {
        let ds = self
            .query_tx(
                txn,
                "INSERT INTO om_revision (resource_kind, api_name, revision, payload, change_note, changed_by, deleted) \
                 SELECT $1, $2, COALESCE(MAX(revision), 0) + 1, $3, $4, $5, $6 \
                 FROM om_revision WHERE resource_kind = $1 AND api_name = $2 \
                 RETURNING revision",
                vec![
                    DataValue::String(kind.to_string()),
                    DataValue::String(api_name.to_string()),
                    DataValue::Json(payload.to_string()),
                    match change_note {
                        Some(n) if !n.is_empty() => DataValue::String(n.to_string()),
                        _ => DataValue::Null,
                    },
                    DataValue::String(changed_by.to_string()),
                    DataValue::Bool(deleted),
                ],
                "om_revision_append",
            )
            .await?;
        Ok(ds
            .iter()
            .next()
            .map(|row| get_i64(row, ds.schema.as_ref(), "revision"))
            .unwrap_or(0))
    }

    /// 修订时间线（单资源；revision 降序；`deleted=Some(true/false)` 过滤墓碑）。
    pub async fn list_revisions(
        &self,
        kind: &str,
        api_name: &str,
        limit: u32,
        deleted: Option<bool>,
    ) -> StoreResult<Vec<RevisionMeta>> {
        let mut where_parts = vec!["resource_kind = $1".to_string(), "api_name = $2".to_string()];
        if let Some(d) = deleted {
            where_parts.push(format!("deleted = {}", if d { "TRUE" } else { "FALSE" }));
        }
        let sql = format!(
            "SELECT id, resource_kind, api_name, revision, change_note, changed_by, changed_at, deleted \
             FROM om_revision WHERE {} ORDER BY revision DESC LIMIT {}",
            where_parts.join(" AND "),
            limit.clamp(1, 500)
        );
        let ds = self
            .query(
                &sql,
                vec![
                    DataValue::String(kind.to_string()),
                    DataValue::String(api_name.to_string()),
                ],
                "om_revision_list",
            )
            .await?;
        Ok(revision_metas(&ds))
    }

    /// 已删除资源清单（每资源最新修订为墓碑的；全库扫描，kind 可选过滤）。
    pub async fn list_deleted_resources(&self, kind: Option<&str>) -> StoreResult<Vec<DeletedResource>> {
        let kind_filter = match kind {
            Some(k) => format!(" AND resource_kind = '{k}'"),
            None => String::new(),
        };
        let sql = format!(
            "SELECT resource_kind, api_name, revision, changed_by, changed_at FROM ( \
               SELECT DISTINCT ON (resource_kind, api_name) resource_kind, api_name, revision, changed_by, changed_at, deleted \
               FROM om_revision ORDER BY resource_kind, api_name, revision DESC \
             ) t WHERE deleted = TRUE{kind_filter} ORDER BY changed_at DESC LIMIT 500"
        );
        let ds = self.query(&sql, vec![], "om_revision_deleted").await?;
        let s = ds.schema.as_ref();
        Ok(ds
            .iter()
            .map(|row| DeletedResource {
                resource_kind: get_string(row, s, "resource_kind").unwrap_or_default(),
                api_name: get_string(row, s, "api_name").unwrap_or_default(),
                revision: get_i64(row, s, "revision"),
                changed_by: get_opt_string(row, s, "changed_by").unwrap_or_default(),
                changed_at: get_opt_ts(row, s, "changed_at"),
            })
            .collect())
    }

    /// 单条修订详情（含 payload）。
    pub async fn get_revision_detail(&self, id: i64) -> StoreResult<Option<Value>> {
        let ds = self
            .query(
                "SELECT id, resource_kind, api_name, revision, payload, change_note, changed_by, changed_at, deleted \
                 FROM om_revision WHERE id = $1",
                vec![DataValue::Int(id)],
                "om_revision_detail",
            )
            .await?;
        let Some(row) = ds.iter().next() else {
            return Ok(None);
        };
        let s = ds.schema.as_ref();
        let mut v = serde_json::to_value(RevisionMeta {
            id: get_i64(row, s, "id"),
            resource_kind: get_string(row, s, "resource_kind").unwrap_or_default(),
            api_name: get_string(row, s, "api_name").unwrap_or_default(),
            revision: get_i64(row, s, "revision"),
            change_note: get_opt_string(row, s, "change_note"),
            changed_by: get_opt_string(row, s, "changed_by").unwrap_or_default(),
            changed_at: get_opt_ts(row, s, "changed_at"),
            deleted: matches!(
                row.get_by_name(s, "deleted"),
                Some(DataValue::Bool(true))
            ),
        })
        .unwrap_or(Value::Null);
        if let Some(obj) = v.as_object_mut() {
            obj.insert(
                "payload".into(),
                crate::store::get_json(row, s, "payload").unwrap_or(Value::Null),
            );
        }
        Ok(Some(v))
    }

    /// 修订号接续检查：资源现存最大修订号（删除后重建同 apiName 时 UNIQUE 天然接续，无特判）。
    pub async fn latest_revision(&self, kind: &str, api_name: &str) -> StoreResult<i64> {
        let ds = self
            .query(
                "SELECT COALESCE(MAX(revision), 0) AS rev FROM om_revision WHERE resource_kind = $1 AND api_name = $2",
                vec![
                    DataValue::String(kind.to_string()),
                    DataValue::String(api_name.to_string()),
                ],
                "om_revision_max",
            )
            .await?;
        Ok(ds
            .iter()
            .next()
            .map(|row| get_i64(row, ds.schema.as_ref(), "rev"))
            .unwrap_or(0))
    }

    // ─────────────────── 状态流转（transition 唯一写入口） ───────────────────

    /// 单资源状态流转 + 弃用元数据落库（方案 §5.3：离开 deprecated 四列清空；
    /// sunset_at 空串 → NULL；deprecated_at 进入 deprecated 时取 now()）。
    pub(crate) async fn set_status_tx(
        &self,
        txn: &str,
        kind: &str,
        api_name: &str,
        target: TypeStatus,
        dep: Option<&DeprecationMeta>,
    ) -> StoreResult<u64> {
        let table = revision_table(kind)
            .ok_or_else(|| StoreError::Backend(format!("未知资源类别 {kind}")))?;
        let dep = dep.cloned().unwrap_or_default();
        let n = self
            .exec_tx(
                txn,
                &format!(
                    "UPDATE {table} SET status = $1, \
                     deprecation_reason = NULLIF($2::text, ''), \
                     sunset_at = NULLIF($3::text, '')::date, \
                     replacement_api_name = NULLIF($4::text, ''), \
                     deprecated_at = CASE WHEN $1 = 'deprecated' THEN now() ELSE NULL END, \
                     updated_at = now() \
                     WHERE api_name = $5"
                ),
                vec![
                    DataValue::String(target.as_str().to_string()),
                    DataValue::String(dep.reason),
                    DataValue::String(dep.sunset_at.unwrap_or_default()),
                    DataValue::String(dep.replacement_api_name.unwrap_or_default()),
                    DataValue::String(api_name.to_string()),
                ],
                "om_transition_set",
            )
            .await?;
        Ok(n)
    }

    /// 批量级联流转（关系类型两端对象联动；同 set_status 语义）。
    pub(crate) async fn set_links_status_tx(
        &self,
        txn: &str,
        api_names: &[String],
        target: TypeStatus,
        dep: Option<&DeprecationMeta>,
    ) -> StoreResult<u64> {
        if api_names.is_empty() {
            return Ok(0);
        }
        let dep = dep.cloned().unwrap_or_default();
        let n = self
            .exec_tx(
                txn,
                "UPDATE om_link_type SET status = $1, \
                 deprecation_reason = NULLIF($2::text, ''), \
                 sunset_at = NULLIF($3::text, '')::date, \
                 replacement_api_name = NULLIF($4::text, ''), \
                 deprecated_at = CASE WHEN $1 = 'deprecated' THEN now() ELSE NULL END, \
                 updated_at = now() \
                 WHERE api_name = ANY($5)",
                vec![
                    DataValue::String(target.as_str().to_string()),
                    DataValue::String(dep.reason),
                    DataValue::String(dep.sunset_at.unwrap_or_default()),
                    DataValue::String(dep.replacement_api_name.unwrap_or_default()),
                    DataValue::Array(
                        api_names
                            .iter()
                            .map(|n| DataValue::String(n.clone()))
                            .collect(),
                    ),
                ],
                "om_transition_cascade",
            )
            .await?;
        Ok(n)
    }

    /// 生命周期流转落库（transition 唯一事务写入口，方案 §5.2/§5.4）：
    /// 主资源 set_status + 级联关系批量 set_status + 每个变更资源追加修订——单事务。
    /// `main_payload` 为 app 层组装的新状态全量定义（camelCase；view 剥离 layout）。
    #[allow(clippy::too_many_arguments)]
    pub async fn apply_transition(
        &self,
        kind: &str,
        api_name: &str,
        main_payload: &Value,
        target: TypeStatus,
        dep: Option<&DeprecationMeta>,
        cascades: &[CascadeWrite],
        changed_by: &str,
        change_note: Option<&str>,
    ) -> StoreResult<()> {
        let manager = get_default_pg_db_manager();
        let txn_ctx = manager.get_transaction_context();
        let txn = txn_ctx
            .begin(&self.db_id)
            .await
            .map_err(|e| StoreError::Backend(format!("开启流转事务失败: {e}")))?;
        let r = async {
            self.set_status_tx(&txn, kind, api_name, target, dep).await?;
            self.append_revision_tx(&txn, kind, api_name, main_payload, change_note, changed_by, false)
                .await?;
            for c in cascades {
                self.set_links_status_tx(&txn, std::slice::from_ref(&c.api_name), c.target, c.deprecation.as_ref())
                    .await?;
                self.append_revision_tx(&txn, "link", &c.api_name, &c.payload, change_note, changed_by, false)
                    .await?;
            }
            Ok::<(), StoreError>(())
        }
        .await;
        match r {
            Ok(()) => txn_ctx
                .commit(&txn)
                .await
                .map_err(|e| StoreError::Backend(format!("提交流转事务失败: {e}"))),
            Err(e) => {
                let _ = txn_ctx.rollback(&txn).await;
                Err(e)
            }
        }
    }

    /// 资源当前状态（transition 预检用；行不存在 → None）。
    pub async fn get_status(&self, kind: &str, api_name: &str) -> StoreResult<Option<TypeStatus>> {
        let table = revision_table(kind)
            .ok_or_else(|| StoreError::Backend(format!("未知资源类别 {kind}")))?;
        let ds = self
            .query(
                &format!("SELECT status FROM {table} WHERE api_name = $1"),
                vec![DataValue::String(api_name.to_string())],
                "om_transition_status",
            )
            .await?;
        Ok(ds
            .iter()
            .next()
            .and_then(|row| get_opt_string(row, ds.schema.as_ref(), "status"))
            .map(|s| crate::store::str_to_enum(&s)))
    }

    // ─────────────────── 场景引用（D10：sceneRefs + 级联清理） ───────────────────

    /// 引用某对象类型的场景清单（apiName, displayName）——删除确认框影响面。
    pub async fn view_refs_of_object(&self, api_name: &str) -> StoreResult<Vec<(String, String)>> {
        let ds = self
            .query(
                "SELECT api_name, display_name FROM om_view \
                 WHERE members->'objects' @> to_jsonb($1::text) ORDER BY api_name",
                vec![DataValue::String(api_name.to_string())],
                "om_view_refs_object",
            )
            .await?;
        Ok(view_ref_rows(&ds))
    }

    /// 引用某关系类型（members.links 白名单）的场景清单。
    pub async fn view_refs_of_link(&self, api_name: &str) -> StoreResult<Vec<(String, String)>> {
        let ds = self
            .query(
                "SELECT api_name, display_name FROM om_view \
                 WHERE members->'links' @> to_jsonb($1::text) ORDER BY api_name",
                vec![DataValue::String(api_name.to_string())],
                "om_view_refs_link",
            )
            .await?;
        Ok(view_ref_rows(&ds))
    }

    /// 级联清理场景引用（单事务；每个受影响场景产生一条修订，方案 §7.5）。
    /// `object`：从 objects 移除该对象，并联动移除其所有边（两端含该对象的 links）；
    /// `link`：仅从 links 白名单移除该关系。changed_by 记入修订。
    pub async fn cascade_cleanup_scene_refs(
        &self,
        object: Option<&str>,
        link: Option<&str>,
        changed_by: &str,
    ) -> StoreResult<Vec<String>> {
        let manager = get_default_pg_db_manager();
        let txn_ctx = manager.get_transaction_context();
        let txn = txn_ctx
            .begin(&self.db_id)
            .await
            .map_err(|e| StoreError::Backend(format!("开启场景清理事务失败: {e}")))?;
        let out = self
            .cascade_cleanup_scene_refs_tx(&txn, object, link, changed_by)
            .await;
        match out {
            Ok(names) => {
                txn_ctx
                    .commit(&txn)
                    .await
                    .map_err(|e| StoreError::Backend(format!("提交场景清理事务失败: {e}")))?;
                Ok(names)
            }
            Err(e) => {
                let _ = txn_ctx.rollback(&txn).await;
                Err(e)
            }
        }
    }

    pub(crate) async fn cascade_cleanup_scene_refs_tx(
        &self,
        txn: &str,
        object: Option<&str>,
        link: Option<&str>,
        changed_by: &str,
    ) -> StoreResult<Vec<String>> {
        // 受影响场景 = objects 挂该对象 ∪ links 挂该关系（或挂该对象的边，Rust 侧判定）。
        let obj_links: Vec<String> = match object {
            Some(obj) => {
                let ds = self
                    .query_tx(
                        txn,
                        "SELECT api_name FROM om_link_type WHERE object_type_a = $1 OR object_type_b = $1",
                        vec![DataValue::String(obj.to_string())],
                        "om_cleanup_obj_links",
                    )
                    .await?;
                ds.iter()
                    .map(|row| get_string(row, ds.schema.as_ref(), "api_name").unwrap_or_default())
                    .collect()
            }
            None => Vec::new(),
        };
        let ds = self
            .query_tx(
                txn,
                "SELECT api_name, display_name, members, version FROM om_view WHERE source = 'manual'",
                vec![],
                "om_cleanup_views",
            )
            .await?;
        let s = ds.schema.as_ref();
        let mut affected: Vec<String> = Vec::new();
        for row in ds.iter() {
            let api = get_string(row, s, "api_name")?;
            let mut members: Value = crate::store::get_json(row, s, "members").unwrap_or(Value::Null);
            if !members.is_object() {
                continue;
            }
            let objects = members
                .get("objects")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let links = members
                .get("links")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let mut changed = false;
            let mut note_parts: Vec<String> = Vec::new();
            let arr_of = |v: &[Value]| -> Vec<String> {
                v.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            };
            if let Some(obj) = object {
                let mut objs = arr_of(&objects);
                if objs.iter().any(|x| x == obj) {
                    objs.retain(|x| x != obj);
                    members["objects"] = serde_json::to_value(&objs).unwrap_or(Value::Null);
                    changed = true;
                    note_parts.push(format!("移除对象 {obj}"));
                }
                let mut lks = arr_of(&links);
                let before = lks.len();
                lks.retain(|x| !obj_links.contains(x));
                if lks.len() != before {
                    members["links"] = serde_json::to_value(&lks).unwrap_or(Value::Null);
                    changed = true;
                }
            }
            if let Some(lk) = link {
                let mut lks = arr_of(&links);
                if lks.iter().any(|x| x == lk) {
                    lks.retain(|x| x != lk);
                    members["links"] = serde_json::to_value(&lks).unwrap_or(Value::Null);
                    changed = true;
                    note_parts.push(format!("移除关系 {lk}"));
                }
            }
            if !changed {
                continue;
            }
            // members 直写（LWW，不占乐观锁——清理不与成员编辑竞争）；同事务追加场景修订。
            self.exec_tx(
                txn,
                "UPDATE om_view SET members = $2, updated_at = now() WHERE api_name = $1",
                vec![
                    DataValue::String(api.clone()),
                    DataValue::Json(members.to_string()),
                ],
                "om_cleanup_write",
            )
            .await?;
            let note = if note_parts.is_empty() {
                "级联清理场景引用".to_string()
            } else {
                format!("级联清理：{}", note_parts.join("、"))
            };
            self.append_revision_tx(txn, "view", &api, &members, Some(&note), changed_by, false)
                .await?;
            affected.push(api);
        }
        Ok(affected)
    }

    /// 存量一致性：离开 deprecated 的行清空弃用元数据（restore 回滚后调用；幂等）。
    pub(crate) async fn clear_stale_deprecation_tx(&self, txn: &str) -> StoreResult<u64> {
        let mut total = 0u64;
        for kind in REVISION_KINDS {
            let table = revision_table(kind).expect("revision kind 表映射完整");
            total += self
                .exec_tx(
                    txn,
                    &format!(
                        "UPDATE {table} SET deprecation_reason = NULL, sunset_at = NULL, \
                         replacement_api_name = NULL, deprecated_at = NULL \
                         WHERE status <> 'deprecated' AND deprecation_reason IS NOT NULL"
                    ),
                    vec![],
                    "om_dep_cleanup",
                )
                .await?;
        }
        Ok(total)
    }

    /// 存量回填（boot 幂等）：manual 场景 members 缺 `links` 键的，按"两端在场"现算回填
    /// （方案 §7.1 存量迁移——视觉不变：回填值 = 现状动态推导集）。auto 行恒空成员，跳过。
    pub async fn backfill_view_links(&self) -> StoreResult<u64> {
        let ds = self
            .query(
                "SELECT api_name, members FROM om_view WHERE source = 'manual' AND NOT (members ? 'links')",
                vec![],
                "om_view_links_bf_scan",
            )
            .await?;
        let s = ds.schema.as_ref();
        let mut updated = 0u64;
        for row in ds.iter() {
            let api = get_string(row, s, "api_name")?;
            let mut members: Value =
                crate::store::get_json(row, s, "members").unwrap_or(Value::Null);
            let objects: Vec<String> = members
                .get("objects")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            if objects.is_empty() {
                // 无对象成员：补空 links 键即可（后续保存走 serde default 不再缺键）。
                if let Some(obj) = members.as_object_mut() {
                    obj.insert("links".into(), Value::Array(vec![]));
                }
            } else {
                let set: std::collections::BTreeSet<&str> =
                    objects.iter().map(|x| x.as_str()).collect();
                let links_ds = self
                    .query(
                        "SELECT api_name FROM om_link_type WHERE \
                         object_type_a = ANY($1) AND object_type_b = ANY($1)",
                        vec![DataValue::Array(
                            objects.iter().map(|o| DataValue::String(o.clone())).collect(),
                        )],
                        "om_view_links_bf_links",
                    )
                    .await?;
                let links: Vec<String> = links_ds
                    .iter()
                    .map(|r| get_string(r, links_ds.schema.as_ref(), "api_name").unwrap_or_default())
                    .filter(|n| set.contains(n.as_str()))
                    .collect();
                if let Some(obj) = members.as_object_mut() {
                    obj.insert("links".into(), serde_json::to_value(&links).unwrap_or(Value::Null));
                }
            }
            self.exec(
                "UPDATE om_view SET members = $2 WHERE api_name = $1",
                vec![
                    DataValue::String(api.clone()),
                    DataValue::Json(members.to_string()),
                ],
            )
            .await?;
            updated += 1;
        }
        Ok(updated)
    }
}

fn revision_metas(ds: &DataSet) -> Vec<RevisionMeta> {
    let s = ds.schema.as_ref();
    ds.iter()
        .map(|row| RevisionMeta {
            id: get_i64(row, s, "id"),
            resource_kind: get_string(row, s, "resource_kind").unwrap_or_default(),
            api_name: get_string(row, s, "api_name").unwrap_or_default(),
            revision: get_i64(row, s, "revision"),
            change_note: get_opt_string(row, s, "change_note"),
            changed_by: get_opt_string(row, s, "changed_by").unwrap_or_default(),
            changed_at: get_opt_ts(row, s, "changed_at"),
            deleted: matches!(row.get_by_name(s, "deleted"), Some(DataValue::Bool(true))),
        })
        .collect()
}

fn view_ref_rows(ds: &DataSet) -> Vec<(String, String)> {
    let s = ds.schema.as_ref();
    ds.iter()
        .map(|row| {
            (
                get_string(row, s, "api_name").unwrap_or_default(),
                get_opt_string(row, s, "display_name").unwrap_or_default(),
            )
        })
        .collect()
}

