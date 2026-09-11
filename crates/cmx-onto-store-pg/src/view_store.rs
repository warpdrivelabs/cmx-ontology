//! 场景视图（om_view）+ 目录查询扩展 + 共享属性批量 —— [`PgOntologyStore`] 的 inherent 方法块。
//!
//! 本体工作室 P1（方案 §2.1/§5.1/§七）：视图走 B0 同款乐观锁（`version=0` 盲写定版本、
//! `>0` 条件更新 409）；`update_view_layout` 单列 LWW 直写不做版本检查（拖拽专用，
//! 避免拖拽 409 风暴、不用陈旧布局覆盖他人 members 编辑）。auto 视图成员不物化——
//! 现算在 app 层（[`crate::store`] 的 manifest + DAM 聚合）完成。

use chrono::Utc;
use cmx_core::model::cell::DataValue;
use cmx_core::model::data::dataset::{Row, Schema};
use cmx_onto_model::{
    ObjectTypeMeta, SceneViewDef, SceneViewMeta, SharedPropertyTypeDef, StoreError, StoreResult,
    ViewMembers, ViewSource,
};
use serde_json::Value;

use crate::store::{
    get_i64, get_opt_json, get_opt_string, get_opt_ts, get_string, json_or_default, parse_status,
    PgOntologyStore,
};

impl PgOntologyStore {
    // ─────────────────── 场景视图（om_view） ───────────────────

    /// 视图清单（行集，不含读时派生的 auto 虚拟条目——app 层合并）。
    pub async fn list_views(&self, _tenant: &str) -> StoreResult<Vec<SceneViewMeta>> {
        let ds = self
            .query(
                "SELECT api_name, display_name, description, dam, members, source, version, updated_at \
                 FROM om_view ORDER BY updated_at DESC",
                vec![],
                "om_view_list",
            )
            .await?;
        let s = ds.schema.as_ref();
        let mut out = Vec::new();
        for row in ds.iter() {
            let members = view_members_from_row(row, s);
            out.push(SceneViewMeta {
                api_name: get_string(row, s, "api_name")?,
                display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
                description: get_opt_string(row, s, "description").unwrap_or_default(),
                dam: get_opt_json(row, s, "dam")
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default(),
                object_count: members.objects.len() as u32,
                interface_count: members.interfaces.len() as u32,
                source: str_to_view_source(&get_opt_string(row, s, "source").unwrap_or_default()),
                virtual_view: false,
                version: get_i64(row, s, "version") as u32,
                members,
                updated_at: get_opt_ts(row, s, "updated_at"),
            });
        }
        Ok(out)
    }

    /// 单视图定义（含 layout）。
    pub async fn get_view(&self, _tenant: &str, api_name: &str) -> StoreResult<Option<SceneViewDef>> {
        let ds = self
            .query(
                "SELECT api_name, display_name, description, dam, members, source, layout, version \
                 FROM om_view WHERE api_name = $1",
                vec![DataValue::String(api_name.to_string())],
                "om_view_one",
            )
            .await?;
        let Some(row) = ds.iter().next() else {
            return Ok(None);
        };
        let s = ds.schema.as_ref();
        Ok(Some(SceneViewDef {
            api_name: get_string(row, s, "api_name")?,
            display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
            description: get_opt_string(row, s, "description").unwrap_or_default(),
            dam: get_opt_json(row, s, "dam")
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default(),
            members: view_members_from_row(row, s),
            source: str_to_view_source(&get_opt_string(row, s, "source").unwrap_or_default()),
            layout: get_opt_json(row, s, "layout").unwrap_or(Value::Null),
            version: get_i64(row, s, "version") as u32,
        }))
    }

    /// 视图 upsert（B0 同款乐观锁；返回落库后版本号；409/404 语义与对象类型一致）。
    pub async fn upsert_view_locked(&self, _tenant: &str, def: &SceneViewDef) -> StoreResult<u32> {
        let now = Utc::now();
        if def.version == 0 {
            let ds = self
                .query(
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
                        DataValue::Json(
                            serde_json::to_string(&def.dam).unwrap_or_else(|_| "{}".to_string()),
                        ),
                        json_or_default(&members_json(def), r#"{"objects":[],"interfaces":[]}"#),
                        DataValue::String(view_source_str(&def.source).to_string()),
                        json_or_default(&def.layout, "{}"),
                        DataValue::DateTime(now),
                    ],
                    "om_view_upsert_v0",
                )
                .await?;
            return ds
                .iter()
                .next()
                .map(|row| get_i64(row, ds.schema.as_ref(), "version") as u32)
                .ok_or_else(|| StoreError::Backend("视图 upsert 未返回新版本号".into()));
        }
        let ds = self
            .query(
                "UPDATE om_view SET display_name=$2, description=$3, dam=$4, members=$5, source=$6, \
                 layout=$7, version=version + 1, updated_at=$8 \
                 WHERE api_name=$1 AND version=$9 \
                 RETURNING version",
                vec![
                    DataValue::String(def.api_name.clone()),
                    DataValue::String(def.display_name.clone()),
                    DataValue::String(def.description.clone()),
                    DataValue::Json(
                        serde_json::to_string(&def.dam).unwrap_or_else(|_| "{}".to_string()),
                    ),
                    json_or_default(&members_json(def), r#"{"objects":[],"interfaces":[]}"#),
                    DataValue::String(view_source_str(&def.source).to_string()),
                    json_or_default(&def.layout, "{}"),
                    DataValue::DateTime(now),
                    DataValue::Int(def.version as i64),
                ],
                "om_view_update_locked",
            )
            .await?;
        if let Some(row) = ds.iter().next() {
            return Ok(get_i64(row, ds.schema.as_ref(), "version") as u32);
        }
        let exists = self
            .query(
                "SELECT 1 AS one FROM om_view WHERE api_name = $1",
                vec![DataValue::String(def.api_name.clone())],
                "om_view_exists",
            )
            .await?;
        if exists.iter().next().is_some() {
            Err(StoreError::Conflict(format!(
                "视图 {} 已被他人修改（基线版本 {} 已过期），请刷新后重试",
                def.api_name, def.version
            )))
        } else {
            Err(StoreError::NotFound(format!("视图 {} 不存在（可能已被删除），请刷新", def.api_name)))
        }
    }

    /// 删除视图（manual 场景；auto 行（仅布局落行）同此入口，域消失后自然消亡）。
    pub async fn delete_view(&self, _tenant: &str, api_name: &str) -> StoreResult<u64> {
        self.exec(
            "DELETE FROM om_view WHERE api_name = $1",
            vec![DataValue::String(api_name.to_string())],
        )
        .await
    }

    /// 布局单列 LWW 直写（不做版本检查、不递增 version）；返回行是否存在。
    pub async fn update_view_layout(
        &self,
        _tenant: &str,
        api_name: &str,
        layout: &Value,
    ) -> StoreResult<bool> {
        let n = self
            .exec(
                "UPDATE om_view SET layout=$2, updated_at=$3 WHERE api_name=$1",
                vec![
                    DataValue::String(api_name.to_string()),
                    json_or_default(layout, "{}"),
                    DataValue::DateTime(Utc::now()),
                ],
            )
            .await?;
        Ok(n > 0)
    }

    // ─────────────────── A2：对象目录服务端过滤分页 ───────────────────

    /// 对象目录分页查询（q 模糊匹配 apiName/displayName、dam 过滤顶级域；page 从 1 起）。
    /// 返回（行, 总数）。仅新页面目录表格使用——旧设计器继续走全量 `list_object_types`。
    pub async fn list_object_types_paged(
        &self,
        _tenant: &str,
        q: &str,
        dam: &str,
        page: u32,
        size: u32,
    ) -> StoreResult<(Vec<ObjectTypeMeta>, i64)> {
        let mut where_parts: Vec<String> = Vec::new();
        let mut params: Vec<DataValue> = Vec::new();
        if !q.trim().is_empty() {
            params.push(DataValue::String(format!("%{}%", q.trim())));
            where_parts.push(format!(
                "(api_name ILIKE ${} OR display_name ILIKE ${})",
                params.len(),
                params.len()
            ));
        }
        if !dam.trim().is_empty() {
            params.push(DataValue::String(dam.trim().to_string()));
            // dam 为 jsonb {domain,...}：取 ->>'domain' 精确匹配顶级域。
            where_parts.push(format!("dam->>'domain' = ${}", params.len()));
        }
        let where_sql = if where_parts.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", where_parts.join(" AND "))
        };
        let total_ds = self
            .query(
                &format!("SELECT COUNT(*) AS tc FROM om_object_type{where_sql}"),
                params.clone(),
                "om_object_type_paged_count",
            )
            .await?;
        let total = total_ds
            .iter()
            .next()
            .map(|r| get_i64(r, total_ds.schema.as_ref(), "tc"))
            .unwrap_or(0);
        let size = size.clamp(1, 500);
        let page = page.max(1);
        params.push(DataValue::Int(size as i64));
        params.push(DataValue::Int(((page - 1) * size) as i64));
        let ds = self
            .query(
                &format!(
                    "SELECT api_name, display_name, status, primary_key, \
                     jsonb_array_length(properties) AS pc, dam, doc_type, version, updated_at \
                     FROM om_object_type{where_sql} \
                     ORDER BY updated_at DESC LIMIT ${} OFFSET ${}",
                    params.len() - 1,
                    params.len()
                ),
                params,
                "om_object_type_paged",
            )
            .await?;
        let s = ds.schema.as_ref();
        let mut out = Vec::new();
        for row in ds.iter() {
            out.push(object_meta_from_row(row, s)?);
        }
        Ok((out, total))
    }

    // ─────────────────── A1：共享属性批量详情 ───────────────────

    /// 按 apiName 列表批量取共享属性定义（单 SQL `= ANY($1)`；不存在的静默跳过，
    /// 与 object-types/batch 语义对齐——响应 {items, errors} 由 handler 层按清单比对补 errors）。
    pub async fn get_shared_properties_batch(
        &self,
        _tenant: &str,
        api_names: &[String],
    ) -> StoreResult<Vec<SharedPropertyTypeDef>> {
        if api_names.is_empty() {
            return Ok(Vec::new());
        }
        let ds = self
            .query(
                "SELECT api_name, display_name, base_type, semantic_type, description \
                 FROM om_shared_property WHERE api_name = ANY($1)",
                vec![DataValue::Array(
                    api_names.iter().map(|n| DataValue::String(n.clone())).collect(),
                )],
                "om_shared_property_batch",
            )
            .await?;
        let s = ds.schema.as_ref();
        let mut out = Vec::new();
        for row in ds.iter() {
            out.push(SharedPropertyTypeDef {
                api_name: get_string(row, s, "api_name")?,
                display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
                base_type: serde_json::from_value(Value::String(
                    get_opt_string(row, s, "base_type").unwrap_or_else(|| "string".into()),
                ))
                .unwrap_or_default(),
                semantic_type: get_opt_string(row, s, "semantic_type"),
                description: get_opt_string(row, s, "description").unwrap_or_default(),
            });
        }
        Ok(out)
    }
}

// ————————————————————————— 视图行助手 —————————————————————————

/// members 行值还原（缺键/形状漂移容忍为空成员）。
fn view_members_from_row(row: &Row, s: &Schema) -> ViewMembers {
    get_opt_json(row, s, "members")
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn view_source_str(v: &ViewSource) -> &'static str {
    match v {
        ViewSource::Auto => "auto",
        ViewSource::Manual => "manual",
    }
}

fn str_to_view_source(s: &str) -> ViewSource {
    match s {
        "auto" => ViewSource::Auto,
        _ => ViewSource::Manual,
    }
}

fn members_json(def: &SceneViewDef) -> Value {
    serde_json::to_value(&def.members).unwrap_or_else(|_| serde_json::json!({}))
}

/// 对象清单行 → ObjectTypeMeta（与 store.rs `list_object_types` 同列清单；分页查询复用）。
fn object_meta_from_row(row: &Row, s: &Schema) -> StoreResult<ObjectTypeMeta> {
    Ok(ObjectTypeMeta {
        api_name: get_string(row, s, "api_name")?,
        display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
        status: parse_status(row, s),
        primary_key: get_opt_string(row, s, "primary_key").unwrap_or_default(),
        property_count: get_i64(row, s, "pc") as u32,
        dam: get_opt_json(row, s, "dam")
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default(),
        doc_type: get_opt_json(row, s, "doc_type")
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default(),
        version: get_i64(row, s, "version") as u32,
        updated_at: get_opt_ts(row, s, "updated_at"),
    })
}
