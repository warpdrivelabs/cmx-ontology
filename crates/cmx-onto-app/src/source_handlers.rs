//! 数据源绑定与管理 API（方案 20260918 §5.4/§5.6；M1a bind/unbind + M1b 注册表管理）。
//!
//! 绑定唯一写入口（E2）：`POST /object-types/datasource/bind` —— 走 `save_object_with_revision`
//! 修订链维护 `om_object_type.datasource` 展示指针，权威行落 `om_source_mapping`（mode=virtual）；
//! 普通 save 剥离 datasource（handlers.rs），杜绝旁路。前置闸（E6/D5）：总闸 on + authz ≠ off。
//!
//! 数据源注册表（M1b）：om_data_source CRUD + probe（先测后存）+ 结构反射（「从源导入字段」）。
//! pg 独立源每节点**请求期懒注册**（E5：复刻 tenancy.rs 模式，`ontosrc_` 前缀防撞名，小池）；
//! M1b+ 编辑：`POST /data-sources/update` 改 name/config；配置变更即拆本节点旧池，下次访问按新配置懒注册
//! （绑定关系不受影响；多节点部署时其余节点待滚动后收敛——与 E5 单节点懒注册同口径）。

use crate::backend_dispatcher::{authz_mode, binding_summary, ensure_bind_gate, virtual_query_enabled};
use crate::engine::store;
use crate::resp::{ApiResp, OntoError, Result};
use crate::tenant::current_tenant;
use axum::extract::Query;
use axum::Json;
use cmx_onto_model::backend::ObjectDataBackend;
use cmx_onto_model::{MappingMode, OntologyStore, SourceMapping};
use cmx_onto_store_pg::sql_guard::safe_qualified_table;
use cmx_onto_store_pg::{PgDirectBackend, SourceStore};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

fn source_store() -> SourceStore {
    SourceStore::new(crate::tenancy::current_db_id())
}

// ————————————————————————— 对象类型绑定（M1a） —————————————————————————

/// POST /object-types/datasource/bind 请求体。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BindReq {
    pub object_type: String,
    /// 仅接受 "virtual"（解除虚拟绑定走 unbind；物化模式 = 无绑定缺省，无须 bind）。
    pub mode: String,
    /// M1a = toml `[[databases]]` db_id；M1b 起 = om_data_source.id（两阶段语义，P2-2）。
    pub source_id: String,
    /// 源表名（schema.table 或 table；虚拟绑定必填）。
    pub resource: String,
    /// 主键源列（虚拟绑定**恰 1 个**：pk 桥接 + 固定排序都需要单列锚）。
    pub key_columns: Vec<String>,
    pub title_column: Option<String>,
    /// [{source, property}]。
    pub property_map: Vec<Value>,
    pub required: Vec<String>,
}

impl Default for BindReq {
    fn default() -> Self {
        Self {
            object_type: String::new(),
            mode: "virtual".into(),
            source_id: String::new(),
            resource: String::new(),
            key_columns: vec![],
            title_column: None,
            property_map: vec![],
            required: vec![],
        }
    }
}

/// POST /object-types/datasource/bind —— 绑定虚拟数据源（唯一写入口；修订链 + 强校验）。
#[utoipa::path(
    post,
    path = "/api/onto/v1/object-types/datasource/bind",
    tag = "数据源",
    summary = "绑定对象类型数据源（mode=virtual：查询下推源库；走修订链）",
    request_body(content = Value, description = "objectType/mode/sourceId/resource/keyColumns/titleColumn/propertyMap/required"),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn bind_datasource(Json(req): Json<BindReq>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    if req.mode != "virtual" {
        return Err(OntoError::bad_request(
            "bind 仅支持 mode=virtual（本体物化为缺省模式，无须绑定；解除虚拟绑定走 unbind）",
        ));
    }
    // E6/D5 前置闸：总闸 + authz。
    ensure_bind_gate()?;
    let mapping = validate_and_resolve(&tenant, &req).await?;

    // 权威行落 om_source_mapping（mode=virtual）。
    save_virtual_mapping(&mapping).await?;

    // 展示指针：读 def → 设 datasource → 修订链保存（乐观锁 + om_revision + SSE）。
    let mut def = store()
        .get_object_type(&tenant, &req.object_type)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载对象类型失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("对象类型 {} 不存在", req.object_type)))?;
    def.datasource = Some(cmx_onto_model::DataSourceBinding {
        source_id: req.source_id.clone(),
        mode: "virtual".into(),
        resource: Some(req.resource.clone()),
    });
    let version = store()
        .save_object_with_revision(&def, &crate::handlers::changed_by(), Some("绑定虚拟数据源"))
        .await
        .map_err(|e| OntoError::internal_error(format!("保存绑定（修订链）失败: {e}")))?;
    crate::handlers::emit_resource_changed(&tenant, "object", &req.object_type, "save");

    let mut summary = binding_summary(&tenant, &req.object_type).await?;
    if let Value::Object(m) = &mut summary {
        m.insert("version".into(), json!(version));
    }
    Ok(Json(ApiResp::ok(summary)))
}

/// POST /object-types/datasource/unbind —— 解除绑定（删权威行 + 清展示指针，走修订链）。
#[utoipa::path(
    post,
    path = "/api/onto/v1/object-types/datasource/unbind",
    tag = "数据源",
    summary = "解除对象类型数据源绑定（回到本地物化缺省模式）",
    request_body(content = Value, description = "{ objectType }"),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn unbind_datasource(Json(body): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let object_type = body
        .get("objectType")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if object_type.is_empty() {
        return Err(OntoError::bad_request("缺 objectType"));
    }
    let n = source_store_is_funnel()
        .delete_mapping(&object_type)
        .await
        .map_err(|e| OntoError::internal_error(format!("删绑定行失败: {e}")))?;
    let mut def = store()
        .get_object_type(&tenant, &object_type)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载对象类型失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("对象类型 {object_type} 不存在")))?;
    def.datasource = None;
    store()
        .save_object_with_revision(&def, &crate::handlers::changed_by(), Some("解除数据源绑定"))
        .await
        .map_err(|e| OntoError::internal_error(format!("保存解绑（修订链）失败: {e}")))?;
    crate::handlers::emit_resource_changed(&tenant, "object", &object_type, "save");
    Ok(Json(ApiResp::ok(json!({
        "objectType": object_type, "unbound": n > 0,
    }))))
}

/// GET /object-types/datasource?objectType= —— 绑定详情（权威行 + 展示指针 + 闸门状态）。
#[utoipa::path(
    get,
    path = "/api/onto/v1/object-types/datasource",
    tag = "数据源",
    summary = "查询对象类型的数据源绑定（mode/source/resource/映射/闸门状态）",
    params(("object_type" = String, Query, description = "对象类型 API 名")),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn get_datasource(Query(q): Query<DataSourceQuery>) -> Result<Json<ApiResp<Value>>> {
    if q.object_type.is_empty() {
        return Err(OntoError::bad_request("缺 objectType"));
    }
    let tenant = current_tenant();
    Ok(Json(ApiResp::ok(binding_summary(&tenant, &q.object_type).await?)))
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DataSourceQuery {
    pub object_type: String,
}

/// bind 强校验 + 源解析（两阶段语义）：注册表行优先（M1b），未命中按 toml db_id（M1a）。
/// 返回构造好的虚拟映射（未落库）。
async fn validate_and_resolve(tenant: &str, req: &BindReq) -> Result<SourceMapping> {
    let def = store()
        .get_object_type(tenant, &req.object_type)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载对象类型失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("对象类型 {} 不存在", req.object_type)))?;

    let resource = safe_qualified_table(&req.resource)
        .map_err(|e| OntoError::bad_request(format!("{e}")))?;
    if req.key_columns.len() != 1 || req.key_columns[0].trim().is_empty() {
        return Err(OntoError::bad_request(
            "虚拟绑定要求恰 1 个 keyColumn（pk 桥接与固定排序需要单列锚）",
        ));
    }
    // 映射属性 ⊆ 类型属性且合法标识符（bind 强校验，R5）。
    let def_props: HashSet<&str> = def.properties.iter().map(|p| p.api_name.as_str()).collect();
    let mut property_map = Vec::with_capacity(req.property_map.len());
    for pm in &req.property_map {
        let src = pm.get("source").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let prop = pm.get("property").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if src.is_empty() || prop.is_empty() {
            return Err(OntoError::bad_request("propertyMap 项缺 source/property"));
        }
        if !def_props.contains(prop.as_str()) {
            return Err(OntoError::bad_request(format!(
                "映射属性「{prop}」不在类型 {} 的属性列表中",
                req.object_type
            )));
        }
        property_map.push((src, prop));
    }
    // 主键/标题属性必须被映射（读取需要 pk/title 列）。
    let mapped: HashSet<&str> = property_map.iter().map(|(_, p)| p.as_str()).collect();
    if !def.primary_key.is_empty() && !mapped.contains(def.primary_key.as_str()) {
        return Err(OntoError::bad_request(format!(
            "主键属性「{}」必须出现在 propertyMap 中（虚拟读取按其取 pk）",
            def.primary_key
        )));
    }
    // 标题属性不强制映射：PgDirect 的 title 兜底 pk（title_expr COALESCE，B-P2-2）。
    // 主键属性仍强制（pk 桥接锚）。

    // 源解析：注册表（M1b）→ ref 池 / 独立懒注册；未命中 → toml db_id（M1a 两阶段语义）。
    let registry = source_store().get(&req.source_id).await
        .map_err(|e| OntoError::internal_error(format!("查数据源注册表失败: {e}")))?;
    let (source_id_col, effective_db) = match registry {
        Some(row) => {
            let kind = row.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let status = row.get("status").and_then(|v| v.as_str()).unwrap_or("");
            if status != "active" {
                return Err(OntoError::business_error(format!(
                    "数据源 {} 状态为 {status}（非 active），禁止绑定",
                    req.source_id
                )));
            }
            if kind != "pg" {
                return Err(OntoError::business_error(format!(
                    "数据源 {} 类型 {kind} 的查询后端随 M2 交付，当前仅支持 pg 源",
                    req.source_id
                )));
            }
            let (db_id, cfg) = cmx_onto_store_pg::source_store::resolve_pg_source_db(&row)
                .map_err(|e| OntoError::bad_request(format!("{e}")))?;
            if db_id.starts_with("ontosrc_") {
                ensure_standalone_registered(&req.source_id, &cfg).await?;
            }
            (Some(req.source_id.clone()), db_id)
        }
        None => {
            // M1a：sourceId 即 toml db_id——运行期注册且可达才允许绑定（probe 验证）。
            PgDirectBackend::new()
                .probe(&req.source_id, None)
                .await
                .map_err(|e| {
                    OntoError::business_error(format!(
                        "源 {} 不可达或未在 [[databases]] 注册: {e}",
                        req.source_id
                    ))
                })?;
            (None, req.source_id.clone())
        }
    };

    // probe 源结构（列基线校验映射列存在；fail-fast 到 bind 时刻而非首查）。
    let probe = PgDirectBackend::new()
        .probe(&effective_db, Some(&resource))
        .await
        .map_err(|e| OntoError::business_error(format!("源表 {resource} 探测失败: {e}")))?;
    if let Some(Value::Array(cols)) = &probe.columns {
        let col_names: HashSet<&str> = cols
            .iter()
            .filter_map(|c| c.get("name").and_then(|v| v.as_str()))
            .collect();
        let mut check = Vec::new();
        check.extend(req.key_columns.iter().cloned());
        if let Some(t) = &req.title_column {
            check.push(t.clone());
        }
        for (s, _) in &property_map {
            check.push(s.clone());
        }
        let missing: Vec<String> = check
            .iter()
            .filter(|c| !col_names.contains(c.as_str()))
            .cloned()
            .collect();
        if !missing.is_empty() {
            return Err(OntoError::bad_request(format!(
                "映射列在源表 {resource} 中不存在：{missing:?}（源结构漂移？请核对列名）"
            )));
        }
    } else {
        return Err(OntoError::business_error(
            "源结构探测未返回列基线，绑定拒绝（fail-closed）",
        ));
    }

    Ok(SourceMapping {
        object_type: req.object_type.clone(),
        mode: MappingMode::Virtual,
        resource: Some(resource),
        source_query: String::new(),
        key_columns: req.key_columns.clone(),
        title_column: req.title_column.clone().filter(|t| !t.trim().is_empty()),
        property_map,
        required: req.required.clone(),
        source_db_id: Some(effective_db),
        source_id: source_id_col,
    })
}

/// 虚拟权威行落库（upsert om_source_mapping，mode=virtual）。
async fn save_virtual_mapping(m: &SourceMapping) -> Result<()> {
    let fs = funnel_store();
    fs.upsert_virtual(m)
        .await
        .map_err(|e| OntoError::internal_error(format!("写绑定权威行失败: {e}")))
}

fn funnel_store() -> cmx_onto_store_pg::FunnelStore {
    cmx_onto_store_pg::FunnelStore::new(crate::tenancy::current_db_id())
}

/// unbind 删行用（与 funnel 共用同一 store）。
fn source_store_is_funnel() -> cmx_onto_store_pg::FunnelStore {
    funnel_store()
}

// ————————————————————————— 数据源注册表管理（M1b） —————————————————————————

/// GET /data-sources —— 列出注册表。
#[utoipa::path(
    get,
    path = "/api/onto/v1/data-sources",
    tag = "数据源",
    summary = "列出数据源注册表（凭证仅环境变量名，不落明文）",
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn list_data_sources() -> Result<Json<ApiResp<Value>>> {
    let out = source_store()
        .list()
        .await
        .map_err(|e| OntoError::internal_error(format!("列数据源失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({
        "items": out,
        "virtualQueryEnabled": virtual_query_enabled(),
        "authzMode": authz_mode(),
    }))))
}

/// POST /data-sources —— 新建/覆盖一个数据源（pg 独立源 = 删源重建语义；M1 配置不可变）。
#[utoipa::path(
    post,
    path = "/api/onto/v1/data-sources",
    tag = "数据源",
    summary = "新建/覆盖数据源（pg 独立源改配置=删源重建；凭证只存环境变量引用名）",
    request_body(content = Value, description = "{id,name,kind,config,caps?}"),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn create_data_source(Json(body): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let id = source_store()
        .upsert(&body)
        .await
        .map_err(|e| OntoError::bad_request(format!("{e}")))?;
    Ok(Json(ApiResp::ok(json!({ "id": id, "saved": true }))))
}

/// POST /data-sources/update —— 更新（kind 与 id 不可变；name/config 缺省沿用现值）。
/// 配置变更时拆本节点旧池，下次访问按新配置懒注册（绑定关系不受影响）。
#[utoipa::path(
    post,
    path = "/api/onto/v1/data-sources/update",
    tag = "数据源",
    summary = "更新数据源（改 name/config；配置变更后连接池自动重建）",
    request_body(content = Value, description = "{id, name?, config?}"),
    responses(
        (status = 200, description = "统一信封 {code,msg,data:{id,saved,configChanged}}", body = ApiResp<Value>),
    )
)]
pub async fn update_data_source(Json(body): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if id.is_empty() {
        return Err(OntoError::bad_request("缺 id"));
    }
    let row = source_store()
        .get(&id)
        .await
        .map_err(|e| OntoError::internal_error(format!("查数据源失败: {e}")))?
        .ok_or_else(|| OntoError::bad_request(format!("数据源 {id} 不存在")))?;
    let kind = row.get("kind").and_then(|v| v.as_str()).unwrap_or("pg").to_string();
    let old_cfg = row.get("config").cloned().unwrap_or(json!({}));
    let mut merged = body.clone();
    merged["kind"] = json!(kind); // kind 不可变（pg/api 切换 = 删源重建）
    if merged.get("config").is_none() {
        merged["config"] = old_cfg.clone();
    }
    if merged.get("name").is_none() {
        merged["name"] = row.get("name").cloned().unwrap_or_else(|| json!(id));
    }
    source_store()
        .upsert(&merged)
        .await
        .map_err(|e| OntoError::bad_request(format!("{e}")))?;
    let new_cfg = merged["config"].clone();
    let config_changed = standalone_cfg_key(&new_cfg) != standalone_cfg_key(&old_cfg);
    if config_changed {
        let (db_id, _) = cmx_onto_store_pg::source_store::resolve_pg_source_db(&json!(
            { "id": id, "kind": kind, "config": new_cfg }
        ))
        .map_err(|e| OntoError::bad_request(format!("{e}")))?;
        if db_id.starts_with("ontosrc_") {
            teardown_standalone(&id).await;
        }
    }
    Ok(Json(ApiResp::ok(
        json!({ "id": id, "saved": true, "configChanged": config_changed }),
    )))
}

/// POST /data-sources/delete —— 删除（幂等）。
#[utoipa::path(
    post,
    path = "/api/onto/v1/data-sources/delete",
    tag = "数据源",
    summary = "删除数据源注册行（绑定它的映射将在查询时报源不存在）",
    request_body(content = Value, description = "{id}"),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn delete_data_source(Json(body): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if id.is_empty() {
        return Err(OntoError::bad_request("缺 id"));
    }
    let n = source_store()
        .delete(&id)
        .await
        .map_err(|e| OntoError::internal_error(format!("删数据源失败: {e}")))?;
    Ok(Json(ApiResp::ok(json!({ "id": id, "deleted": n > 0 }))))
}

/// POST /data-sources/probe —— 连通性 + 结构探测（**config 优先 = 草稿探测**：编辑场景对已存在
/// id 先测新配置；探测后还原原池，不留副作用、不断现网）。
#[utoipa::path(
    post,
    path = "/api/onto/v1/data-sources/probe",
    tag = "数据源",
    summary = "探测数据源（连通 + 列基线；带 config 按草稿测（id 可已存在），否则按 id 走注册行 / toml db_id）",
    request_body(content = Value, description = "{id} 或 {id?, kind:'pg', config:{...}, resource?}"),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn probe_data_source(Json(body): Json<Value>) -> Result<Json<ApiResp<Value>>> {
    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let resource = body.get("resource").and_then(|v| v.as_str()).map(str::trim);
    let backend = PgDirectBackend::new();
    // 草稿探测不得留池副作用：(draft_id, 探测前已注册的旧 cfg)——探测后按此还原。
    let (effective_db, from_registry, draft_restore) = if body.get("config").is_some() {
        // 草稿探测**优先**（config 在即按草稿测）：编辑场景对已存在的 id 先测新配置再保存，
        // 新建场景对未保存的 id 先测后存。探测经草稿配置建临时池，结束后还原原池状态。
        let draft_id = if id.is_empty() { "draft".to_string() } else { id.clone() };
        let cfg = body.get("config").cloned().unwrap_or(json!({}));
        let row = json!({ "id": draft_id, "kind": "pg", "config": cfg });
        let (db_id, cfg2) = cmx_onto_store_pg::source_store::resolve_pg_source_db(&row)
            .map_err(|e| OntoError::bad_request(format!("{e}")))?;
        let prev = src_ready().lock().unwrap().get(&db_id).cloned();
        if db_id.starts_with("ontosrc_") {
            ensure_standalone_registered(&draft_id, &cfg2).await?;
        }
        (db_id, false, Some((draft_id, prev)))
    } else if !id.is_empty() {
        match source_store().get(&id).await
            .map_err(|e| OntoError::internal_error(format!("查数据源失败: {e}")))? {
            Some(row) => {
                let (db_id, cfg) = cmx_onto_store_pg::source_store::resolve_pg_source_db(&row)
                    .map_err(|e| OntoError::bad_request(format!("{e}")))?;
                if db_id.starts_with("ontosrc_") {
                    ensure_standalone_registered(&id, &cfg).await?;
                }
                (db_id, true, None)
            }
            // M1a：toml db_id 直探（两阶段语义）。
            None => (id.clone(), false, None),
        }
    } else {
        return Err(OntoError::bad_request("probe 须传 id 或完整草稿 config"));
    };
    let probe_res = backend.probe(&effective_db, resource).await;
    // 草稿探测还原：拆草稿池；探测前有旧池则按旧配置重建（探错/取消编辑都不留脏池、不断现网）
    if let Some((draft_id, prev)) = draft_restore {
        teardown_standalone(&draft_id).await;
        if let Some(pc) = prev
            && let Err(e) = ensure_standalone_registered(&draft_id, &pc).await
        {
            tracing::warn!(db_id = %format!("ontosrc_{draft_id}"), error = %e,
                "草稿探测还原旧池失败（已清标记，下次访问懒注册重试）");
        }
    }    let probe = probe_res.map_err(|e| {
        OntoError::business_error(format!("探测失败（db={effective_db}）: {e}"))
    })?;
    if from_registry && !id.is_empty() {
        let _ = source_store()
            .save_probe_report(&id, &serde_json::to_value(&probe).unwrap_or(Value::Null))
            .await;
    }
    Ok(Json(ApiResp::ok(json!({
        "sourceDb": effective_db,
        "reachable": probe.reachable,
        "detail": probe.detail,
        "columns": probe.columns,
    }))))
}

/// GET /data-sources/schema?sourceId=&resource= —— 源表结构反射（「从源导入字段」数据源）。
#[utoipa::path(
    get,
    path = "/api/onto/v1/data-sources/schema",
    tag = "数据源",
    summary = "反射源表结构（列名/类型/可空；供创建向导导入字段）",
    params(("source_id" = String, Query, description = "数据源 id"), ("resource" = String, Query, description = "源表 schema.table")),
    responses(
        (status = 200, description = "统一信封 {code,msg,data}", body = ApiResp<Value>),
    )
)]
pub async fn source_schema(Query(q): Query<SchemaQuery>) -> Result<Json<ApiResp<Value>>> {
    if q.source_id.is_empty() || q.resource.is_empty() {
        return Err(OntoError::bad_request("缺 sourceId / resource"));
    }
    // 独立源先懒注册（E5），再反射（store 层不做连接管理）。
    if let Some(row) = source_store().get(&q.source_id).await
        .map_err(|e| OntoError::internal_error(format!("查数据源失败: {e}")))? {
        let (db_id, cfg) = cmx_onto_store_pg::source_store::resolve_pg_source_db(&row)
            .map_err(|e| OntoError::bad_request(format!("{e}")))?;
        if db_id.starts_with("ontosrc_") {
            ensure_standalone_registered(&q.source_id, &cfg).await?;
        }
    }
    let out = source_store()
        .reflect_schema(&q.source_id, &q.resource)
        .await
        .map_err(|e| OntoError::business_error(format!("{e}")))?;
    Ok(Json(ApiResp::ok(out)))
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SchemaQuery {
    pub source_id: String,
    pub resource: String,
}

// ————————————————————————— pg 独立源懒注册（E5；复刻 tenancy.rs 模式） —————————————————————————

static SRC_READY: OnceLock<Mutex<HashMap<String, Value>>> = OnceLock::new();
fn src_ready() -> &'static Mutex<HashMap<String, Value>> {
    SRC_READY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 独立源配置指纹（仅取影响连接的字段；json 值直比）。
fn standalone_cfg_key(cfg: &Value) -> Value {
    let g = |k: &str| cfg.get(k).cloned().unwrap_or(Value::Null);
    json!({"host": g("host"), "port": g("port"), "db": g("db"), "user": g("user"),
           "passwordEnv": g("passwordEnv"), "poolMax": g("poolMax"), "ref": g("ref")})
}

/// 每节点请求期懒注册独立连接池（`ontosrc_<id>` 前缀防与 toml 撞名；小池 poolMax 缺省 3）。
/// 配置感知：注册过的源若 cfg 指纹变化（编辑源），先拆旧池再按新配置重建，本节点即时生效。
async fn ensure_standalone_registered(id: &str, cfg: &Value) -> Result<()> {
    let db_id = format!("ontosrc_{id}");
    let key = standalone_cfg_key(cfg);
    let need_rebuild = {
        let mut map = src_ready().lock().unwrap();
        match map.get(&db_id) {
            Some(prev) if *prev == key => return Ok(()),
            Some(_) => {
                map.remove(&db_id);
                true
            }
            None => false,
        }
    };
    if need_rebuild {
        // 旧池拆掉（在途连接随池 drop 关闭）；注册中心无此 id 时 unregister 报错可忽略
        if let Err(e) = cmx_database_pg::get_default_pg_db_manager()
            .unregister_data_source(&db_id)
            .await
        {
            tracing::debug!(db_id = %db_id, error = %e, "拆旧独立源池（可能本就未注册）");
        }
    }
    let password_env = cfg.get("passwordEnv").and_then(|v| v.as_str()).unwrap_or("");
    let password = std::env::var(password_env).map_err(|_| {
        OntoError::business_error(format!(
            "环境变量 {password_env} 未设置（独立源凭证经环境变量注入，绝不落库）"
        ))
    })?;
    let host = cfg.get("host").and_then(|v| v.as_str()).unwrap_or("127.0.0.1");
    let port = cfg.get("port").and_then(|v| v.as_i64()).unwrap_or(5432);
    let db = cfg.get("db").and_then(|v| v.as_str()).unwrap_or("");
    let user = cfg.get("user").and_then(|v| v.as_str()).unwrap_or("");
    let pool_max = cfg.get("poolMax").and_then(|v| v.as_i64()).unwrap_or(3).clamp(1, 4) as u32;
    let url = format!(
        "postgres://{}:{}@{}:{}/{}",
        user,
        percent_encode(&password),
        host,
        port,
        db
    );
    let db_cfg = cmx_database_pg::DbConfig {
        db_type: cmx_database_pg::DbType::Postgres,
        db_url: url,
        db_id: db_id.clone(),
        db_name: None,
        db_schema: Some("public".to_string()),
        default: false,
        pool_config: Default::default(),
        health_check_interval: 60,
        health_check_timeout: 5,
        domain_code: None,
        application_code: None,
        module_code: None,
        source_type: Some("biz".to_string()),
    };
    let _ = pool_max; // 池上限由注册中心池配置承载（缺省小池）；此处保留显式上限语义供 M3 调优。
    cmx_service_base::register_pg_datasources(&[db_cfg])
        .await
        .map_err(|e| OntoError::business_error(format!("独立源 {db_id} 注册失败: {e}")))?;
    src_ready().lock().unwrap().insert(db_id.clone(), key);
    tracing::info!(db_id = %db_id, "✅ 独立数据源连接池已懒注册（E5）");
    Ok(())
}

/// 拆掉独立源本节点连接池并清除注册标记（编辑/草稿探测后的池复位；幂等）。
async fn teardown_standalone(id: &str) {
    let db_id = format!("ontosrc_{id}");
    src_ready().lock().unwrap().remove(&db_id);
    if let Err(e) = cmx_database_pg::get_default_pg_db_manager()
        .unregister_data_source(&db_id)
        .await
    {
        tracing::debug!(db_id = %db_id, error = %e, "拆独立源池（可能本就未注册）");
    }
}

/// URL 百分号编码（密码段；仅宽容 [A-Za-z0-9_.-]，其余 %XX）。
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' => out.push(b as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}
