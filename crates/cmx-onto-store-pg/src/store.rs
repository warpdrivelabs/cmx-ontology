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
use cmx_database_pg::{execute_sql, execute_sql_with_params, query_sql_with_params, SqlParams};
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
    db_id: String,
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
        Ok(())
    }

    async fn exec(&self, sql: &str, params: Vec<DataValue>) -> StoreResult<u64> {
        execute_sql_with_params(&self.db_id, None, sql, SqlParams::DataValues(params))
            .await
            .map_err(|e| StoreError::Backend(format!("执行失败: {e}")))
    }

    async fn query(&self, sql: &str, params: Vec<DataValue>, ds_id: &str) -> StoreResult<DataSet> {
        query_sql_with_params(&self.db_id, None, sql, SqlParams::DataValues(params), ds_id)
            .await
            .map_err(|e| StoreError::Backend(format!("查询失败: {e}")))
    }

    // ─────────────────── 发布 / 版本（inherent 方法） ───────────────────

    /// 发布当前本体：全量定义快照 → 不可变 om_version（version+1、rev 内容哈希）。返回版本元数据。
    pub async fn publish(
        &self,
        tenant: &str,
        summary: &str,
        published_by: Option<String>,
    ) -> StoreResult<OntologyVersionMeta> {
        let snapshot = self.snapshot(tenant).await?;
        let snap_str = snapshot.to_string();
        let rev = format!("{:016x}", xxhash_rust::xxh64::xxh64(snap_str.as_bytes(), 0));
        let vds = self
            .query(
                "SELECT COALESCE(MAX(version), 0) AS mx FROM om_version",
                vec![],
                "om_ver_max",
            )
            .await?;
        let cur_max = vds
            .iter()
            .next()
            .map(|r| get_i64(r, vds.schema.as_ref(), "mx"))
            .unwrap_or(0);
        let next = (cur_max + 1) as u32;
        let now = Utc::now();
        self.exec(
            "INSERT INTO om_version (version, rev, summary, snapshot, published_by, published_at) \
             VALUES ($1, $2, $3, $4, $5, $6)",
            vec![
                DataValue::Int(next as i64),
                DataValue::String(rev.clone()),
                DataValue::String(summary.to_string()),
                DataValue::Json(snap_str),
                opt_str(&published_by),
                DataValue::DateTime(now),
            ],
        )
        .await?;
        Ok(OntologyVersionMeta {
            version: next,
            rev,
            summary: summary.to_string(),
            published_by,
            published_at: now,
        })
    }

    /// 列出全部发布版本（版本降序）。
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

    /// 取某版本发布快照（全量定义 jsonb）。
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

    /// 组装全量定义快照（发布用）：六类元素的完整定义。
    async fn snapshot(&self, tenant: &str) -> StoreResult<Value> {
        // 对象类型：按清单逐一取全量定义。
        let mut object_types = Vec::new();
        for m in self.list_object_types(tenant).await? {
            if let Some(d) = self.get_object_type(tenant, &m.api_name).await? {
                object_types.push(serde_json::to_value(d).unwrap_or(Value::Null));
            }
        }
        let mut link_types = Vec::new();
        for m in self.list_link_types(tenant).await? {
            if let Some(d) = self.get_link_type(tenant, &m.api_name).await? {
                link_types.push(serde_json::to_value(d).unwrap_or(Value::Null));
            }
        }
        let mut interfaces = Vec::new();
        for m in self.list_interfaces(tenant).await? {
            if let Some(d) = self.get_interface(tenant, &m.api_name).await? {
                interfaces.push(serde_json::to_value(d).unwrap_or(Value::Null));
            }
        }
        let mut shared_properties = Vec::new();
        for m in self.list_shared_properties(tenant).await? {
            if let Some(d) = self.get_shared_property(tenant, &m.api_name).await? {
                shared_properties.push(serde_json::to_value(d).unwrap_or(Value::Null));
            }
        }
        let mut action_types = Vec::new();
        for m in self.list_action_types(tenant).await? {
            if let Some(d) = self.get_action_type(tenant, &m.api_name).await? {
                action_types.push(serde_json::to_value(d).unwrap_or(Value::Null));
            }
        }
        let mut functions = Vec::new();
        for m in self.list_functions(tenant).await? {
            if let Some(d) = self.get_function(tenant, &m.api_name).await? {
                functions.push(serde_json::to_value(d).unwrap_or(Value::Null));
            }
        }
        Ok(serde_json::json!({
            "objectTypes": object_types,
            "linkTypes": link_types,
            "interfaces": interfaces,
            "sharedProperties": shared_properties,
            "actionTypes": action_types,
            "functions": functions,
        }))
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
              properties, implements, datasource, cmx_origin, version, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$14) \
             ON CONFLICT (api_name) DO UPDATE SET display_name=EXCLUDED.display_name, \
              description=EXCLUDED.description, icon=EXCLUDED.icon, color=EXCLUDED.color, \
              primary_key=EXCLUDED.primary_key, title_property=EXCLUDED.title_property, \
              status=EXCLUDED.status, properties=EXCLUDED.properties, implements=EXCLUDED.implements, \
              datasource=EXCLUDED.datasource, cmx_origin=EXCLUDED.cmx_origin, version=EXCLUDED.version, \
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
                 status, properties, implements, datasource, cmx_origin, version \
                 FROM om_object_type WHERE api_name = $1",
                vec![DataValue::String(api_name.to_string())],
                "om_object_type_one",
            )
            .await?;
        let Some(row) = ds.iter().next() else {
            return Ok(None);
        };
        let s = ds.schema.as_ref();
        let properties: Vec<PropertyTypeDef> = get_json(row, s, "properties")
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let implements: Vec<String> = get_json(row, s, "implements")
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        Ok(Some(ObjectTypeDef {
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
            datasource: get_opt_json(row, s, "datasource"),
            cmx_origin: get_opt_json(row, s, "cmx_origin"),
            version: get_i64(row, s, "version") as u32,
        }))
    }

    async fn list_object_types(&self, _tenant: &str) -> StoreResult<Vec<ObjectTypeMeta>> {
        let ds = self
            .query(
                "SELECT api_name, display_name, status, primary_key, \
                 jsonb_array_length(properties) AS pc, version, updated_at \
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
        let ds = self
            .query(
                "SELECT api_name, display_name, cardinality, object_type_a, object_type_b, status, updated_at \
                 FROM om_link_type ORDER BY updated_at DESC",
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
        let ds = self
            .query(
                "SELECT api_name, display_name, updated_at FROM om_interface ORDER BY updated_at DESC",
                vec![],
                "om_interface_list",
            )
            .await?;
        Ok(simple_metas(&ds))
    }

    async fn delete_interface(&self, _tenant: &str, api_name: &str) -> StoreResult<u64> {
        self.exec(
            "DELETE FROM om_interface WHERE api_name = $1",
            vec![DataValue::String(api_name.to_string())],
        )
        .await
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
        Ok(OntologyManifest {
            object_types: self.list_object_types(tenant).await?,
            link_types: self.list_link_types(tenant).await?,
            interfaces: self.list_interfaces(tenant).await?,
            shared_properties: self.list_shared_properties(tenant).await?,
            action_types: self.list_action_types(tenant).await?,
            functions: self.list_functions(tenant).await?,
        })
    }
}

// ————————————————————————— 取值 / 转换助手 —————————————————————————

/// 由 `SELECT api_name, display_name, updated_at` 的 DataSet 还原通用清单项。
fn simple_metas(ds: &DataSet) -> Vec<SimpleTypeMeta> {
    let s = ds.schema.as_ref();
    let mut out = Vec::new();
    for row in ds.iter() {
        out.push(SimpleTypeMeta {
            api_name: get_opt_string(row, s, "api_name").unwrap_or_default(),
            display_name: get_opt_string(row, s, "display_name").unwrap_or_default(),
            updated_at: get_opt_ts(row, s, "updated_at"),
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
fn str_to_enum<T: DeserializeOwned + Default>(s: &str) -> T {
    serde_json::from_value(Value::String(s.to_string())).unwrap_or_default()
}

/// status 列还原（VARCHAR → TypeStatus）。
fn parse_status(row: &Row, schema: &Schema) -> TypeStatus {
    str_to_enum(&get_opt_string(row, schema, "status").unwrap_or_default())
}

/// 可序列化对象 → jsonb DataValue（数组/对象通用）。
fn json_arr<T: Serialize>(v: &T) -> DataValue {
    DataValue::Json(serde_json::to_string(v).unwrap_or_else(|_| "[]".into()))
}

/// serde_json::Value → jsonb DataValue；Null 用 default 兜底（列 NOT NULL）。
fn json_or_default(v: &Value, default: &str) -> DataValue {
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

fn opt_str(v: &Option<String>) -> DataValue {
    match v {
        Some(s) => DataValue::String(s.clone()),
        None => DataValue::Null,
    }
}

fn get_string(row: &Row, schema: &Schema, col: &str) -> StoreResult<String> {
    match row.get_by_name(schema, col) {
        Some(DataValue::String(s)) => Ok(s.clone()),
        Some(DataValue::ShortStr(s)) | Some(DataValue::LongStr(s)) => Ok(s.to_string()),
        other => Err(StoreError::Backend(format!("列 {col} 期望文本，实际 {other:?}"))),
    }
}

fn get_opt_string(row: &Row, schema: &Schema, col: &str) -> Option<String> {
    match row.get_by_name(schema, col) {
        Some(DataValue::String(s)) => Some(s.clone()),
        Some(DataValue::ShortStr(s)) | Some(DataValue::LongStr(s)) => Some(s.to_string()),
        _ => None,
    }
}

fn get_i64(row: &Row, schema: &Schema, col: &str) -> i64 {
    match row.get_by_name(schema, col) {
        Some(DataValue::Int(v)) => *v,
        _ => 0,
    }
}

fn get_opt_ts(row: &Row, schema: &Schema, col: &str) -> Option<DateTime<Utc>> {
    match row.get_by_name(schema, col) {
        Some(DataValue::DateTime(dt)) => Some(*dt),
        _ => None,
    }
}

fn get_json(row: &Row, schema: &Schema, col: &str) -> StoreResult<Value> {
    match row.get_by_name(schema, col) {
        Some(DataValue::Json(s)) => {
            serde_json::from_str(s).map_err(|e| StoreError::Backend(format!("解析 {col} jsonb 失败: {e}")))
        }
        Some(DataValue::String(s)) => serde_json::from_str(s)
            .map_err(|e| StoreError::Backend(format!("解析 {col} 字符串为 json 失败: {e}"))),
        other => Err(StoreError::Backend(format!("列 {col} 期望 jsonb，实际 {other:?}"))),
    }
}

fn get_opt_json(row: &Row, schema: &Schema, col: &str) -> Option<Value> {
    match row.get_by_name(schema, col) {
        Some(DataValue::Json(s)) | Some(DataValue::String(s)) => serde_json::from_str(s).ok(),
        _ => None,
    }
}
