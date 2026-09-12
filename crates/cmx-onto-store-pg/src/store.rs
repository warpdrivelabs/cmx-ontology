//! [`OntologyStore`] 的 tokio-postgres 实现 + 发布/版本快照（inherent 方法）。
//!
//! 严格类型纪律（对齐 flow/rules store-pg，tokio-postgres 类型敏感）：
//! - jsonb 列写入用 `DataValue::Json(String)`，读回也是 `DataValue::Json(String)`；可空 jsonb 用 `DataValue::Null`。
//! - TIMESTAMPTZ 用 `DataValue::DateTime`；文本用 `DataValue::String`；整数用 `DataValue::Int`；布尔用 `DataValue::Bool`。
//! - 参数绑定统一走 `SqlParams::DataValues`（顺序对应 `$1..$n`）。
//! - 枚举列（status/cardinality/baseType/runtime/kind）以 camelCase 文本存取，经 serde round-trip 转换。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cmx_core::model::cell::{DataValue, SqlTypeMarker};
use cmx_core::model::data::dataset::{DataSet, Row, Schema};
use cmx_database_pg::{
    execute_sql, execute_sql_with_params, get_default_pg_db_manager, query_sql_with_params,
    SqlParams,
};
use cmx_onto_model::{
    ActionTypeDef, FunctionDef, InterfaceDef, LinkTypeDef, LinkTypeMeta, ObjectTypeDef,
    ObjectTypeMeta, OntologyManifest, OntologyStore, OntologyVersionMeta, PropertyTypeDef,
    SharedPropertyTypeDef, SimpleTypeMeta, StoreError, StoreResult, TypeStatus,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

/// PG 本体存储。`db_id` 指向已注册的数据源（多租户下按租户派生）。
#[derive(Clone)]
pub struct PgOntologyStore {
    pub(crate) db_id: String,
}

impl PgOntologyStore {
    /// 用数据源 id 构造。
    pub fn new(db_id: impl Into<String>) -> Self {
        Self { db_id: db_id.into() }
    }

    /// 幂等建表（启动钩子调用）。
    pub async fn ensure_schema(&self) -> StoreResult<()> {
        for stmt in crate::ddl::DDL_STATEMENTS {
            execute_sql(&self.db_id, None, stmt)
                .await
                .map_err(|e| StoreError::Backend(format!("建表失败: {e}")))?;
        }
        // 表/列注释重放（COMMENT ON 幂等覆盖；语义见 ddl.rs 模块注释）。
        for stmt in crate::ddl::DDL_COMMENTS {
            execute_sql(&self.db_id, None, stmt)
                .await
                .map_err(|e| StoreError::Backend(format!("写表注释失败: {e}")))?;
        }
        // 一次性清理重放（已废弃表 DROP，幂等；见 ddl.rs DDL_CLEANUPS）。
        for stmt in crate::ddl::DDL_CLEANUPS {
            execute_sql(&self.db_id, None, stmt)
                .await
                .map_err(|e| StoreError::Backend(format!("清理废弃表失败: {e}")))?;
        }
        Ok(())
    }

    pub(crate) async fn exec(&self, sql: &str, params: Vec<DataValue>) -> StoreResult<u64> {
        execute_sql_with_params(&self.db_id, None, sql, SqlParams::DataValues(params))
            .await
            .map_err(|e| StoreError::Backend(format!("执行失败: {e}")))
    }

    pub(crate) async fn query(&self, sql: &str, params: Vec<DataValue>, ds_id: &str) -> StoreResult<DataSet> {
        query_sql_with_params(&self.db_id, None, sql, SqlParams::DataValues(params), ds_id)
            .await
            .map_err(|e| StoreError::Backend(format!("查询失败: {e}")))
    }

    // ─────────────────── D15 批量详情（inherent 方法） ───────────────────

    /// 按 apiName 列表批量取对象类型完整定义（单 SQL `= ANY($1)`）。
    ///
    /// 设计器首屏装载要对清单里每个对象类型各拉一次详情（44 类型 = 44 请求 × 远端库
    /// ~190ms 往返 ≈ 8.7s 的主体）；本方法把 N 次往返折叠为 1 次。返回的 Vec 不保证
    /// 与入参同序（按库内存储序），调用方按 api_name 自行对位；不存在的 apiName 静默
    /// 跳过（与既有 404 语义不同——清单驱动下的批量装载容忍并发删除）。
    pub async fn get_object_types_batch(
        &self,
        _tenant: &str,
        api_names: &[String],
    ) -> StoreResult<Vec<ObjectTypeDef>> {
        if api_names.is_empty() {
            return Ok(Vec::new());
        }
        let ds = self
            .query(
                "SELECT api_name, display_name, description, icon, color, primary_key, title_property, \
                 status, properties, implements, dam, doc_type, datasource, cmx_origin, version \
                 FROM om_object_type WHERE api_name = ANY($1)",
                vec![DataValue::Array(
                    api_names.iter().map(|n| DataValue::String(n.clone())).collect(),
                )],
                "om_object_type_batch",
            )
            .await?;
        let s = ds.schema.as_ref();
        ds.iter()
            .map(|row| object_def_from_row(row, s))
            .collect::<StoreResult<Vec<_>>>()
    }

    // ─────────────────── B0 乐观锁保存（原子） ───────────────────

    /// 带乐观锁的对象类型 upsert（B0，单语句原子，无 TOCTOU）：
    /// - `def.version == 0`（新建语义 / 无版本调用方如 quickCreate、import）：盲写 upsert，
    ///   服务端定版本——新建置 1、覆盖既有行时 `version = 旧 + 1`；
    /// - `def.version > 0`（designer GET→POST 通路）：条件 UPDATE `WHERE api_name=$1 AND version=$n`，
    ///   0 行 = 版本不匹配（他人已改，409 Conflict）或行不存在（已被删除，404 NotFound）；
    ///   命中则 `version = 旧 + 1`。
    ///
    /// 返回落库后的新版本号（前端以响应刷新基线）。行为变更声明见方案
    /// `documents/plans/20260908_cmx-ontology_本体设计器功能补全与交互样式优化方案.md` §3.3 B0。
    pub async fn upsert_object_type_locked(
        &self,
        _tenant: &str,
        def: &ObjectTypeDef,
    ) -> StoreResult<u32> {
        let now = Utc::now();
        if def.version == 0 {
            // 盲写路径：INSERT 定版本 1；冲突覆盖时在旧版本上 +1（服务端单一真相，不吃客户端值）。
            let ds = self
                .query(
                    "INSERT INTO om_object_type \
                     (api_name, display_name, description, icon, color, primary_key, title_property, status, \
                      properties, implements, dam, doc_type, datasource, cmx_origin, version, created_at, updated_at) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,1,$15,$15) \
                     ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, \
                      description=EXCLUDED.description, icon=EXCLUDED.icon, color=EXCLUDED.color, \
                      primary_key=EXCLUDED.primary_key, title_property=EXCLUDED.title_property, \
                      status=EXCLUDED.status, properties=EXCLUDED.properties, implements=EXCLUDED.implements, \
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
                        DataValue::String(enum_to_str(&def.status)),
                        json_arr(&def.properties),
                        json_arr(&def.implements),
                        DataValue::Json(serde_json::to_string(&def.dam).unwrap_or_else(|_| "{}".to_string())),
                        DataValue::Json(serde_json::to_string(&def.doc_type).unwrap_or_else(|_| "{}".to_string())),
                        opt_json(&def.datasource),
                        opt_json(&def.cmx_origin),
                        DataValue::DateTime(now),
                    ],
                    "om_object_type_upsert_v0",
                )
                .await?;
            return ds
                .iter()
                .next()
                .map(|row| get_i64(row, ds.schema.as_ref(), "version") as u32)
                .ok_or_else(|| StoreError::Backend("upsert 未返回新版本号".into()));
        }
        // 条件更新路径：命中即原子递增；0 行区分「版本冲突」与「行不存在」。
        let ds = self
            .query(
                "UPDATE om_object_type SET display_name=$2, description=$3, icon=$4, color=$5, \
                 primary_key=$6, title_property=$7, status=$8, properties=$9, implements=$10, \
                 dam=$11, doc_type=$12, datasource=$13, cmx_origin=$14, version=version + 1, updated_at=$15 \
                 WHERE api_name=$1 AND version=$16 \
                 RETURNING version",
                vec![
                    DataValue::String(def.api_name.clone()),
                    DataValue::String(def.display_name.clone()),
                    DataValue::String(def.description.clone()),
                    DataValue::String(def.icon.clone()),
                    DataValue::String(def.color.clone()),
                    DataValue::String(def.primary_key.clone()),
                    DataValue::String(def.title_property.clone()),
                    DataValue::String(enum_to_str(&def.status)),
                    json_arr(&def.properties),
                    json_arr(&def.implements),
                    DataValue::Json(serde_json::to_string(&def.dam).unwrap_or_else(|_| "{}".to_string())),
                    DataValue::Json(serde_json::to_string(&def.doc_type).unwrap_or_else(|_| "{}".to_string())),
                    opt_json(&def.datasource),
                    opt_json(&def.cmx_origin),
                    DataValue::DateTime(now),
                    DataValue::Int(def.version as i64),
                ],
                "om_object_type_update_locked",
            )
            .await?;
        if let Some(row) = ds.iter().next() {
            return Ok(get_i64(row, ds.schema.as_ref(), "version") as u32);
        }
        // 0 行：行不存在 → 404 语义；存在但版本不符 → 409 语义。
        let exists = self
            .query(
                "SELECT 1 AS one FROM om_object_type WHERE api_name = $1",
                vec![DataValue::String(def.api_name.clone())],
                "om_object_type_exists",
            )
            .await?;
        if exists.iter().next().is_some() {
            Err(StoreError::Conflict(format!(
                "对象类型 {} 已被他人修改（基线版本 {} 已过期），请刷新后重试",
                def.api_name, def.version
            )))
        } else {
            Err(StoreError::NotFound(format!(
                "对象类型 {} 不存在（可能已被删除），请刷新",
                def.api_name
            )))
        }
    }

    // ─────────────────── 版本快照（存档栈读取；写入走 snapshot_store） ───────────────────

    /// 列出全部存档版本（版本降序）。
    pub async fn list_versions(&self) -> StoreResult<Vec<OntologyVersionMeta>> {
        let ds = self
            .query(
                "SELECT version, rev, summary, published_by, published_at FROM om_version \
                 ORDER BY version DESC",
                vec![],
                "om_versions",
            )
            .await?;
        let schema = ds.schema.as_ref();
        let mut out = Vec::new();
        for row in ds.iter() {
            out.push(OntologyVersionMeta {
                version: get_i64(row, schema, "version") as u32,
                rev: get_opt_string(row, schema, "rev").unwrap_or_default(),
                summary: get_opt_string(row, schema, "summary").unwrap_or_default(),
                published_by: get_opt_string(row, schema, "published_by"),
                published_at: get_opt_ts(row, schema, "published_at").unwrap_or_else(Utc::now),
            });
        }
        Ok(out)
    }

    /// 取某版本存档快照（全量定义 jsonb）。
    pub async fn get_version(&self, version: u32) -> StoreResult<Option<Value>> {
        let ds = self
            .query(
                "SELECT snapshot FROM om_version WHERE version = $1",
                vec![DataValue::Int(version as i64)],
                "om_version_one",
            )
            .await?;
        match ds.iter().next() {
            Some(row) => Ok(Some(get_json(row, ds.schema.as_ref(), "snapshot")?)),
            None => Ok(None),
        }
    }

    /// 维护白名单（subject, subject_kind）——**空表 = 开放**（全员维护等效；
    /// 有行 = 仅命中者可写，写权限守卫不可被直连 API 绕过）。
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

    /// 清单过滤装载（inherent；`OntologyStore::manifest` 的按需子集版）：
    /// `kinds` 为空 = 全量六类（调用方走 trait `manifest`）；非空 = 仅装载指定键
    /// （objectTypes/linkTypes/interfaces/sharedProperties/actionTypes/functions），
    /// 返回仅含请求键的对象——供轻量消费方按需取数。
    pub async fn manifest_filtered(&self, tenant: &str, kinds: &[String]) -> StoreResult<Value> {
        let mut out = serde_json::Map::new();
        for k in kinds {
            match k.as_str() {
                "objectTypes" => {
                    let v = self.list_object_types(tenant).await?;
                    out.insert(k.clone(), serde_json::to_value(&v).unwrap_or(Value::Null));
                }
                "linkTypes" => {
                    let v = self.list_link_types(tenant).await?;
                    out.insert(k.clone(), serde_json::to_value(&v).unwrap_or(Value::Null));
                }
                "interfaces" => {
                    let v = self.list_interfaces(tenant).await?;
                    out.insert(k.clone(), serde_json::to_value(&v).unwrap_or(Value::Null));
                }
                "sharedProperties" => {
                    let v = self.list_shared_properties(tenant).await?;
                    out.insert(k.clone(), serde_json::to_value(&v).unwrap_or(Value::Null));
                }
                "actionTypes" => {
                    let v = self.list_action_types(tenant).await?;
                    out.insert(k.clone(), serde_json::to_value(&v).unwrap_or(Value::Null));
                }
                "functions" => {
                    let v = self.list_functions(tenant).await?;
                    out.insert(k.clone(), serde_json::to_value(&v).unwrap_or(Value::Null));
                }
                _ => return Err(StoreError::Backend(format!("未知清单类型 {k:?}"))),
            }
        }
        Ok(Value::Object(out))
    }
}

#[async_trait]
impl OntologyStore for PgOntologyStore {
    // ─────────────────────────── 对象类型 ───────────────────────────

    async fn upsert_object_type(&self, _tenant: &str, def: &ObjectTypeDef) -> StoreResult<()> {
        let now = Utc::now();
        self.exec(
            "INSERT INTO om_object_type \
             (api_name, display_name, description, icon, color, primary_key, title_property, status, \
              properties, implements, dam, doc_type, datasource, cmx_origin, version, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$16) \
             ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, \
              description=EXCLUDED.description, icon=EXCLUDED.icon, color=EXCLUDED.color, \
              primary_key=EXCLUDED.primary_key, title_property=EXCLUDED.title_property, \
              status=EXCLUDED.status, properties=EXCLUDED.properties, implements=EXCLUDED.implements, \
              dam=EXCLUDED.dam, doc_type=EXCLUDED.doc_type, datasource=EXCLUDED.datasource, cmx_origin=EXCLUDED.cmx_origin, version=EXCLUDED.version, \
              updated_at=EXCLUDED.updated_at",
            vec![
                DataValue::String(def.api_name.clone()),
                DataValue::String(def.display_name.clone()),
                DataValue::String(def.description.clone()),
                DataValue::String(def.icon.clone()),
                DataValue::String(def.color.clone()),
                DataValue::String(def.primary_key.clone()),
                DataValue::String(def.title_property.clone()),
                DataValue::String(enum_to_str(&def.status)),
                json_arr(&def.properties),
                json_arr(&def.implements),
                DataValue::Json(serde_json::to_string(&def.dam).unwrap_or_else(|_| "{}".to_string())),
                DataValue::Json(serde_json::to_string(&def.doc_type).unwrap_or_else(|_| "{}".to_string())),
                opt_json(&def.datasource),
                opt_json(&def.cmx_origin),
                DataValue::Int(def.version as i64),
                DataValue::DateTime(now),
            ],
        )
        .await?;
        Ok(())
    }

    async fn get_object_type(
        &self,
        _tenant: &str,
        api_name: &str,
    ) -> StoreResult<Option<ObjectTypeDef>> {
        let ds = self
            .query(
                "SELECT api_name, display_name, description, icon, color, primary_key, title_property, \
                 status, properties, implements, dam, doc_type, datasource, cmx_origin, version \
                 FROM om_object_type WHERE api_name = $1",
                vec![DataValue::String(api_name.to_string())],
                "om_object_type_one",
            )
            .await?;
        let Some(row) = ds.iter().next() else {
            return Ok(None);
        };
        Ok(Some(object_def_from_row(row, ds.schema.as_ref())?))
    }

    async fn list_object_types(&self, _tenant: &str) -> StoreResult<Vec<ObjectTypeMeta>> {
        // 清单富化：properties 全量随行（manifest 即全量形状，消费方免单独拉详情）。
        let ds = self
            .query(
                "SELECT api_name, display_name, status, primary_key, \
                 jsonb_array_length(properties) AS pc, properties, dam, doc_type, version, updated_at \
                 FROM om_object_type ORDER BY updated_at DESC",
                vec![],
                "om_object_type_list",
            )
            .await?;
        let s = ds.schema.as_ref();
        let mut out = Vec::new();
        for row in ds.iter() {
            out.push(ObjectTypeMeta {
                api_name: get_string(row, s, "api_name")?,
                display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
                status: parse_status(row, s),
                primary_key: get_opt_string(row, s, "primary_key").unwrap_or_default(),
                property_count: get_i64(row, s, "pc") as u32,
                properties: get_json(row, s, "properties")
                    .ok()
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default(),
                dam: get_opt_json(row, s, "dam").and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default(),
                doc_type: get_opt_json(row, s, "doc_type").and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default(),
                version: get_i64(row, s, "version") as u32,
                updated_at: get_opt_ts(row, s, "updated_at"),
            });
        }
        Ok(out)
    }

    async fn delete_object_type(&self, _tenant: &str, api_name: &str) -> StoreResult<u64> {
        self.exec(
            "DELETE FROM om_object_type WHERE api_name = $1",
            vec![DataValue::String(api_name.to_string())],
        )
        .await
    }

    // ─────────────────────────── 关系类型 ───────────────────────────

    async fn upsert_link_type(&self, _tenant: &str, def: &LinkTypeDef) -> StoreResult<()> {
        let now = Utc::now();
        self.exec(
            "INSERT INTO om_link_type \
             (api_name, display_name, cardinality, object_type_a, object_type_b, role_a, role_b, \
              backing, status, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$10) \
             ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, \
              cardinality=EXCLUDED.cardinality, object_type_a=EXCLUDED.object_type_a, \
              object_type_b=EXCLUDED.object_type_b, role_a=EXCLUDED.role_a, role_b=EXCLUDED.role_b, \
              backing=EXCLUDED.backing, status=EXCLUDED.status, updated_at=EXCLUDED.updated_at",
            vec![
                DataValue::String(def.api_name.clone()),
                DataValue::String(def.display_name.clone()),
                DataValue::String(enum_to_str(&def.cardinality)),
                DataValue::String(def.object_type_a.clone()),
                DataValue::String(def.object_type_b.clone()),
                DataValue::String(def.role_a.clone()),
                DataValue::String(def.role_b.clone()),
                DataValue::Json(def.backing.to_string()),
                DataValue::String(enum_to_str(&def.status)),
                DataValue::DateTime(now),
            ],
        )
        .await?;
        Ok(())
    }

    async fn get_link_type(&self, _tenant: &str, api_name: &str) -> StoreResult<Option<LinkTypeDef>> {
        let ds = self
            .query(
                "SELECT api_name, display_name, cardinality, object_type_a, object_type_b, \
                 role_a, role_b, backing, status FROM om_link_type WHERE api_name = $1",
                vec![DataValue::String(api_name.to_string())],
                "om_link_type_one",
            )
            .await?;
        let Some(row) = ds.iter().next() else {
            return Ok(None);
        };
        let s = ds.schema.as_ref();
        Ok(Some(LinkTypeDef {
            api_name: get_string(row, s, "api_name")?,
            display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
            cardinality: str_to_enum(&get_opt_string(row, s, "cardinality").unwrap_or_default()),
            object_type_a: get_opt_string(row, s, "object_type_a").unwrap_or_default(),
            object_type_b: get_opt_string(row, s, "object_type_b").unwrap_or_default(),
            role_a: get_opt_string(row, s, "role_a").unwrap_or_default(),
            role_b: get_opt_string(row, s, "role_b").unwrap_or_default(),
            backing: get_json(row, s, "backing").unwrap_or(Value::Null),
            status: parse_status(row, s),
        }))
    }

    async fn list_link_types(&self, _tenant: &str) -> StoreResult<Vec<LinkTypeMeta>> {
        // A3 清单富化：LEFT JOIN 两端对象类型的 DAM（跨域关系治理；对端被删 → None 跳过序列化）。
        let ds = self
            .query(
                "SELECT l.api_name, l.display_name, l.cardinality, l.object_type_a, l.object_type_b, \
                 l.status, l.updated_at, da.dam AS dam_a, db.dam AS dam_b \
                 FROM om_link_type l \
                 LEFT JOIN om_object_type da ON da.api_name = l.object_type_a \
                 LEFT JOIN om_object_type db ON db.api_name = l.object_type_b \
                 ORDER BY l.updated_at DESC",
                vec![],
                "om_link_type_list",
            )
            .await?;
        let s = ds.schema.as_ref();
        let mut out = Vec::new();
        for row in ds.iter() {
            out.push(LinkTypeMeta {
                api_name: get_string(row, s, "api_name")?,
                display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
                cardinality: str_to_enum(&get_opt_string(row, s, "cardinality").unwrap_or_default()),
                object_type_a: get_opt_string(row, s, "object_type_a").unwrap_or_default(),
                object_type_b: get_opt_string(row, s, "object_type_b").unwrap_or_default(),
                status: parse_status(row, s),
                updated_at: get_opt_ts(row, s, "updated_at"),
                dam_a: get_opt_json(row, s, "dam_a").and_then(|v| serde_json::from_value(v).ok()),
                dam_b: get_opt_json(row, s, "dam_b").and_then(|v| serde_json::from_value(v).ok()),
            });
        }
        Ok(out)
    }

    async fn delete_link_type(&self, _tenant: &str, api_name: &str) -> StoreResult<u64> {
        self.exec(
            "DELETE FROM om_link_type WHERE api_name = $1",
            vec![DataValue::String(api_name.to_string())],
        )
        .await
    }

    // ─────────────────────────── 接口 ───────────────────────────

    async fn upsert_interface(&self, _tenant: &str, def: &InterfaceDef) -> StoreResult<()> {
        let now = Utc::now();
        self.exec(
            "INSERT INTO om_interface (api_name, display_name, properties, extends, status, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$6) \
             ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, \
              properties=EXCLUDED.properties, extends=EXCLUDED.extends, status=EXCLUDED.status, \
              updated_at=EXCLUDED.updated_at",
            vec![
                DataValue::String(def.api_name.clone()),
                DataValue::String(def.display_name.clone()),
                json_arr(&def.properties),
                json_arr(&def.extends),
                DataValue::String(enum_to_str(&def.status)),
                DataValue::DateTime(now),
            ],
        )
        .await?;
        Ok(())
    }

    async fn get_interface(&self, _tenant: &str, api_name: &str) -> StoreResult<Option<InterfaceDef>> {
        let ds = self
            .query(
                "SELECT api_name, display_name, properties, extends, status FROM om_interface WHERE api_name = $1",
                vec![DataValue::String(api_name.to_string())],
                "om_interface_one",
            )
            .await?;
        let Some(row) = ds.iter().next() else {
            return Ok(None);
        };
        let s = ds.schema.as_ref();
        Ok(Some(InterfaceDef {
            api_name: get_string(row, s, "api_name")?,
            display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
            properties: get_json(row, s, "properties").ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default(),
            extends: get_json(row, s, "extends").ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default(),
            status: parse_status(row, s),
        }))
    }

    async fn list_interfaces(&self, _tenant: &str) -> StoreResult<Vec<SimpleTypeMeta>> {
        // A3 清单富化：附继承链（extends 本表列）与实现者清单（om_object_type.implements 数组
        // 包含性子查询：implements @> "apiName" 标量）；其余四类 simple 清单不填（None）。
        let ds = self
            .query(
                "SELECT i.api_name, i.display_name, i.updated_at, i.extends,                  (SELECT json_agg(o.api_name) FROM om_object_type o WHERE o.implements @> to_jsonb(i.api_name::text)) AS implements_by \
                 FROM om_interface i ORDER BY i.updated_at DESC",
                vec![],
                "om_interface_list",
            )
            .await?;
        let s = ds.schema.as_ref();
        let mut out = Vec::new();
        for row in ds.iter() {
            out.push(SimpleTypeMeta {
                api_name: get_opt_string(row, s, "api_name").unwrap_or_default(),
                display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
                updated_at: get_opt_ts(row, s, "updated_at"),
                implements_by: get_opt_json(row, s, "implements_by")
                    .and_then(|v| serde_json::from_value(v).ok()),
                extends: get_opt_json(row, s, "extends")
                    .and_then(|v| serde_json::from_value(v).ok()),
            });
        }
        Ok(out)
    }

    /// B1：删接口**同事务**级联清各对象类型的 implements 引用（消灭悬空引用；方案唯一行为级豁免）。
    /// 版本快照表（om_version）不动。`jsonb - text` 对数组删匹配字符串元素，须显式 `::text` 重载。
    async fn delete_interface(&self, _tenant: &str, api_name: &str) -> StoreResult<u64> {
        let manager = get_default_pg_db_manager();
        let txn_ctx = manager.get_transaction_context();
        let txn_id = txn_ctx
            .begin(&self.db_id)
            .await
            .map_err(|e| StoreError::Backend(format!("开启事务失败: {e}")))?;
        let n = match execute_sql_with_params(
            &self.db_id,
            Some(&txn_id),
            "DELETE FROM om_interface WHERE api_name = $1",
            SqlParams::DataValues(vec![DataValue::String(api_name.to_string())]),
        )
        .await
        {
            Ok(n) => n,
            Err(e) => {
                let _ = txn_ctx.rollback(&txn_id).await;
                return Err(StoreError::Backend(format!("删除接口失败: {e}")));
            }
        };
        if let Err(e) = execute_sql_with_params(
            &self.db_id,
            Some(&txn_id),
            "UPDATE om_object_type SET implements = implements - $1::text WHERE implements ? $1",
            SqlParams::DataValues(vec![DataValue::String(api_name.to_string())]),
        )
        .await
        {
            let _ = txn_ctx.rollback(&txn_id).await;
            return Err(StoreError::Backend(format!("级联清 implements 失败: {e}")));
        }
        txn_ctx
            .commit(&txn_id)
            .await
            .map_err(|e| StoreError::Backend(format!("提交事务失败: {e}")))?;
        Ok(n)
    }

    // ─────────────────────── 共享属性类型 ───────────────────────

    async fn upsert_shared_property(
        &self,
        _tenant: &str,
        def: &SharedPropertyTypeDef,
    ) -> StoreResult<()> {
        let now = Utc::now();
        self.exec(
            "INSERT INTO om_shared_property (api_name, display_name, base_type, semantic_type, description, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$6) \
             ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, \
              base_type=EXCLUDED.base_type, semantic_type=EXCLUDED.semantic_type, \
              description=EXCLUDED.description, updated_at=EXCLUDED.updated_at",
            vec![
                DataValue::String(def.api_name.clone()),
                DataValue::String(def.display_name.clone()),
                DataValue::String(enum_to_str(&def.base_type)),
                opt_str(&def.semantic_type),
                DataValue::String(def.description.clone()),
                DataValue::DateTime(now),
            ],
        )
        .await?;
        Ok(())
    }

    async fn get_shared_property(
        &self,
        _tenant: &str,
        api_name: &str,
    ) -> StoreResult<Option<SharedPropertyTypeDef>> {
        let ds = self
            .query(
                "SELECT api_name, display_name, base_type, semantic_type, description \
                 FROM om_shared_property WHERE api_name = $1",
                vec![DataValue::String(api_name.to_string())],
                "om_shared_property_one",
            )
            .await?;
        let Some(row) = ds.iter().next() else {
            return Ok(None);
        };
        let s = ds.schema.as_ref();
        Ok(Some(SharedPropertyTypeDef {
            api_name: get_string(row, s, "api_name")?,
            display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
            base_type: str_to_enum(&get_opt_string(row, s, "base_type").unwrap_or_default()),
            semantic_type: get_opt_string(row, s, "semantic_type"),
            description: get_opt_string(row, s, "description").unwrap_or_default(),
        }))
    }

    async fn list_shared_properties(&self, _tenant: &str) -> StoreResult<Vec<SimpleTypeMeta>> {
        let ds = self
            .query(
                "SELECT api_name, display_name, updated_at FROM om_shared_property ORDER BY updated_at DESC",
                vec![],
                "om_shared_property_list",
            )
            .await?;
        Ok(simple_metas(&ds))
    }

    async fn delete_shared_property(&self, _tenant: &str, api_name: &str) -> StoreResult<u64> {
        self.exec(
            "DELETE FROM om_shared_property WHERE api_name = $1",
            vec![DataValue::String(api_name.to_string())],
        )
        .await
    }

    // ─────────────────────────── 动作类型 ───────────────────────────

    async fn upsert_action_type(&self, _tenant: &str, def: &ActionTypeDef) -> StoreResult<()> {
        let now = Utc::now();
        self.exec(
            "INSERT INTO om_action_type \
             (api_name, display_name, description, parameters, logic, validations, side_effects, \
              function_backing, status, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$10) \
             ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, \
              description=EXCLUDED.description, parameters=EXCLUDED.parameters, logic=EXCLUDED.logic, \
              validations=EXCLUDED.validations, side_effects=EXCLUDED.side_effects, \
              function_backing=EXCLUDED.function_backing, status=EXCLUDED.status, updated_at=EXCLUDED.updated_at",
            vec![
                DataValue::String(def.api_name.clone()),
                DataValue::String(def.display_name.clone()),
                DataValue::String(def.description.clone()),
                json_or_default(&def.parameters, "[]"),
                json_or_default(&def.logic, "[]"),
                json_or_default(&def.validations, "[]"),
                json_or_default(&def.side_effects, "[]"),
                opt_str(&def.function_backing),
                DataValue::String(enum_to_str(&def.status)),
                DataValue::DateTime(now),
            ],
        )
        .await?;
        Ok(())
    }

    async fn get_action_type(
        &self,
        _tenant: &str,
        api_name: &str,
    ) -> StoreResult<Option<ActionTypeDef>> {
        let ds = self
            .query(
                "SELECT api_name, display_name, description, parameters, logic, validations, \
                 side_effects, function_backing, status FROM om_action_type WHERE api_name = $1",
                vec![DataValue::String(api_name.to_string())],
                "om_action_type_one",
            )
            .await?;
        let Some(row) = ds.iter().next() else {
            return Ok(None);
        };
        let s = ds.schema.as_ref();
        Ok(Some(ActionTypeDef {
            api_name: get_string(row, s, "api_name")?,
            display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
            description: get_opt_string(row, s, "description").unwrap_or_default(),
            parameters: get_json(row, s, "parameters").unwrap_or(Value::Null),
            logic: get_json(row, s, "logic").unwrap_or(Value::Null),
            validations: get_json(row, s, "validations").unwrap_or(Value::Null),
            side_effects: get_json(row, s, "side_effects").unwrap_or(Value::Null),
            function_backing: get_opt_string(row, s, "function_backing"),
            status: parse_status(row, s),
        }))
    }

    async fn list_action_types(&self, _tenant: &str) -> StoreResult<Vec<SimpleTypeMeta>> {
        let ds = self
            .query(
                "SELECT api_name, display_name, updated_at FROM om_action_type ORDER BY updated_at DESC",
                vec![],
                "om_action_type_list",
            )
            .await?;
        Ok(simple_metas(&ds))
    }

    async fn delete_action_type(&self, _tenant: &str, api_name: &str) -> StoreResult<u64> {
        self.exec(
            "DELETE FROM om_action_type WHERE api_name = $1",
            vec![DataValue::String(api_name.to_string())],
        )
        .await
    }

    // ─────────────────────────── 函数 ───────────────────────────

    async fn upsert_function(&self, _tenant: &str, def: &FunctionDef) -> StoreResult<()> {
        let now = Utc::now();
        self.exec(
            "INSERT INTO om_function \
             (api_name, display_name, runtime, kind, inputs, output, body, description, status, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$10) \
             ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, \
              runtime=EXCLUDED.runtime, kind=EXCLUDED.kind, inputs=EXCLUDED.inputs, output=EXCLUDED.output, \
              body=EXCLUDED.body, description=EXCLUDED.description, status=EXCLUDED.status, updated_at=EXCLUDED.updated_at",
            vec![
                DataValue::String(def.api_name.clone()),
                DataValue::String(def.display_name.clone()),
                DataValue::String(enum_to_str(&def.runtime)),
                DataValue::String(enum_to_str(&def.kind)),
                json_or_default(&def.inputs, "[]"),
                json_or_default(&def.output, "{}"),
                DataValue::String(def.body.clone()),
                DataValue::String(def.description.clone()),
                DataValue::String(enum_to_str(&def.status)),
                DataValue::DateTime(now),
            ],
        )
        .await?;
        Ok(())
    }

    async fn get_function(&self, _tenant: &str, api_name: &str) -> StoreResult<Option<FunctionDef>> {
        let ds = self
            .query(
                "SELECT api_name, display_name, runtime, kind, inputs, output, body, description, status \
                 FROM om_function WHERE api_name = $1",
                vec![DataValue::String(api_name.to_string())],
                "om_function_one",
            )
            .await?;
        let Some(row) = ds.iter().next() else {
            return Ok(None);
        };
        let s = ds.schema.as_ref();
        Ok(Some(FunctionDef {
            api_name: get_string(row, s, "api_name")?,
            display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
            runtime: str_to_enum(&get_opt_string(row, s, "runtime").unwrap_or_default()),
            kind: str_to_enum(&get_opt_string(row, s, "kind").unwrap_or_default()),
            inputs: get_json(row, s, "inputs").unwrap_or(Value::Null),
            output: get_json(row, s, "output").unwrap_or(Value::Null),
            body: get_opt_string(row, s, "body").unwrap_or_default(),
            description: get_opt_string(row, s, "description").unwrap_or_default(),
            status: parse_status(row, s),
        }))
    }

    async fn list_functions(&self, _tenant: &str) -> StoreResult<Vec<SimpleTypeMeta>> {
        let ds = self
            .query(
                "SELECT api_name, display_name, updated_at FROM om_function ORDER BY updated_at DESC",
                vec![],
                "om_function_list",
            )
            .await?;
        Ok(simple_metas(&ds))
    }

    async fn delete_function(&self, _tenant: &str, api_name: &str) -> StoreResult<u64> {
        self.exec(
            "DELETE FROM om_function WHERE api_name = $1",
            vec![DataValue::String(api_name.to_string())],
        )
        .await
    }

    // ─────────────────────────── 清单 ───────────────────────────

    async fn manifest(&self, tenant: &str) -> StoreResult<OntologyManifest> {
        // 六清单并行（try_join!）：远端库每往返 ~190ms，顺序执行 6×RTT ≈ 1.6s（实测首屏
        // 装载的最大单点）；并行后墙钟 ≈ 单查询耗时。连接池默认多连接，六查询互不争用。
        let (object_types, link_types, interfaces, shared_properties, action_types, functions) =
            tokio::try_join!(
                self.list_object_types(tenant),
                self.list_link_types(tenant),
                self.list_interfaces(tenant),
                self.list_shared_properties(tenant),
                self.list_action_types(tenant),
                self.list_functions(tenant),
            )?;
        Ok(OntologyManifest {
            object_types,
            link_types,
            interfaces,
            shared_properties,
            action_types,
            functions,
        })
    }
}

// ————————————————————————— 取值 / 转换助手 —————————————————————————

/// om_object_type 详情行 → `ObjectTypeDef`（单查 / 批量共用；列清单见
/// `get_object_type` / `get_object_types_batch` 的同款 SELECT）。
pub(crate) fn object_def_from_row(row: &Row, s: &Schema) -> StoreResult<ObjectTypeDef> {
    let properties: Vec<PropertyTypeDef> = get_json(row, s, "properties")
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let implements: Vec<String> = get_json(row, s, "implements")
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    Ok(ObjectTypeDef {
        api_name: get_string(row, s, "api_name")?,
        display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
        description: get_opt_string(row, s, "description").unwrap_or_default(),
        icon: get_opt_string(row, s, "icon").unwrap_or_default(),
        color: get_opt_string(row, s, "color").unwrap_or_default(),
        primary_key: get_opt_string(row, s, "primary_key").unwrap_or_default(),
        title_property: get_opt_string(row, s, "title_property").unwrap_or_default(),
        status: parse_status(row, s),
        properties,
        implements,
        dam: get_opt_json(row, s, "dam").and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default(),
        doc_type: get_opt_json(row, s, "doc_type").and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default(),
        datasource: get_opt_json(row, s, "datasource"),
        cmx_origin: get_opt_json(row, s, "cmx_origin"),
        version: get_i64(row, s, "version") as u32,
    })
}

/// 由 `SELECT api_name, display_name, updated_at` 的 DataSet 还原通用清单项。
fn simple_metas(ds: &DataSet) -> Vec<SimpleTypeMeta> {
    let s = ds.schema.as_ref();
    let mut out = Vec::new();
    for row in ds.iter() {
        out.push(SimpleTypeMeta {
            api_name: get_opt_string(row, s, "api_name").unwrap_or_default(),
            display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
            updated_at: get_opt_ts(row, s, "updated_at"),
            // A3 富化仅接口填充（见 list_interfaces）；共享属性/动作/函数恒 None。
            implements_by: None,
            extends: None,
        });
    }
    out
}

/// 枚举 → camelCase 文本（经 serde round-trip）。
fn enum_to_str<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|x| x.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// camelCase 文本 → 枚举（未知/空 → Default）。
pub(crate) fn str_to_enum<T: DeserializeOwned + Default>(s: &str) -> T {
    serde_json::from_value(Value::String(s.to_string())).unwrap_or_default()
}

/// status 列还原（VARCHAR → TypeStatus）。
pub(crate) fn parse_status(row: &Row, schema: &Schema) -> TypeStatus {
    str_to_enum(&get_opt_string(row, schema, "status").unwrap_or_default())
}

/// 可序列化对象 → jsonb DataValue（数组/对象通用）。
fn json_arr<T: Serialize>(v: &T) -> DataValue {
    DataValue::Json(serde_json::to_string(v).unwrap_or_else(|_| "[]".into()))
}

/// serde_json::Value → jsonb DataValue；Null 用 default 兜底（列 NOT NULL）。
pub(crate) fn json_or_default(v: &Value, default: &str) -> DataValue {
    if v.is_null() {
        DataValue::Json(default.to_string())
    } else {
        DataValue::Json(v.to_string())
    }
}

/// Option<Value> → jsonb DataValue（None → **带类型** jsonb NULL）。
/// 可空 jsonb 列的 None 必须用 `NullTyped(Json)`：裸 `DataValue::Null` 会被绑定层当
/// `Option<String>` 序列化，与 Postgres jsonb 类型不兼容而报 500（教训见 memory 记录）。
fn opt_json(v: &Option<Value>) -> DataValue {
    match v {
        Some(x) if !x.is_null() => DataValue::Json(x.to_string()),
        _ => DataValue::NullTyped(SqlTypeMarker::Json),
    }
}

pub(crate) fn opt_str(v: &Option<String>) -> DataValue {
    match v {
        Some(s) => DataValue::String(s.clone()),
        None => DataValue::Null,
    }
}

pub(crate) fn get_string(row: &Row, schema: &Schema, col: &str) -> StoreResult<String> {
    match row.get_by_name(schema, col) {
        Some(DataValue::String(s)) => Ok(s.clone()),
        Some(DataValue::ShortStr(s)) | Some(DataValue::LongStr(s)) => Ok(s.to_string()),
        other => Err(StoreError::Backend(format!("列 {col} 期望文本，实际 {other:?}"))),
    }
}

pub(crate) fn get_opt_string(row: &Row, schema: &Schema, col: &str) -> Option<String> {
    match row.get_by_name(schema, col) {
        Some(DataValue::String(s)) => Some(s.clone()),
        Some(DataValue::ShortStr(s)) | Some(DataValue::LongStr(s)) => Some(s.to_string()),
        _ => None,
    }
}

pub(crate) fn get_i64(row: &Row, schema: &Schema, col: &str) -> i64 {
    match row.get_by_name(schema, col) {
        Some(DataValue::Int(v)) => *v,
        _ => 0,
    }
}

pub(crate) fn get_opt_ts(row: &Row, schema: &Schema, col: &str) -> Option<DateTime<Utc>> {
    match row.get_by_name(schema, col) {
        Some(DataValue::DateTime(dt)) => Some(*dt),
        _ => None,
    }
}

pub(crate) fn get_json(row: &Row, schema: &Schema, col: &str) -> StoreResult<Value> {
    match row.get_by_name(schema, col) {
        Some(DataValue::Json(s)) => {
            serde_json::from_str(s).map_err(|e| StoreError::Backend(format!("解析 {col} jsonb 失败: {e}")))
        }
        Some(DataValue::String(s)) => serde_json::from_str(s)
            .map_err(|e| StoreError::Backend(format!("解析 {col} 字符串为 json 失败: {e}"))),
        other => Err(StoreError::Backend(format!("列 {col} 期望 jsonb，实际 {other:?}"))),
    }
}

pub(crate) fn get_opt_json(row: &Row, schema: &Schema, col: &str) -> Option<Value> {
    match row.get_by_name(schema, col) {
        Some(DataValue::Json(s)) | Some(DataValue::String(s)) => serde_json::from_str(s).ok(),
        _ => None,
    }
}
