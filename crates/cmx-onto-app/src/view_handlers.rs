//! 场景视图 + 成员级装载 handler（直改 live 架构：视图写轨直写 live om_view）。
//!
//! - `GET /views`：行集 + **auto 虚拟条目读时派生**（manifest 按 DAM 域聚合现算，不落行；
//!   未分组伪域不播种——防"一锅炖收编全部无 DAM 类型"）。
//! - `POST /views` / `POST /views/remove`：**直写 live**（`upsert_view_locked` B0 乐观锁 /
//!   `delete_view`）；删除对 auto 行豁免（域派生物化产物，与回滚派生删除同口径）。
//! - `POST /views/layout`：LWW 直写 live——布局是物化产物，不进版本语义（rev 指纹排除
//!   layout；回滚应用 DO UPDATE 不含 layout 列，互不回吞）。
//! - `GET /graph?view=X`：成员级装载一条请求到位（对象全量定义+相关边+实现接口+共享属性+
//!   跨场景关系角标数据 → 组件 spec，消 N+1）。
//! - `POST /shared-properties/batch`：A1 批量详情（ids≤500，`{items,errors}`）。
//! - `GET /object-types` 扩展 `q/dam/page/size`（A2，仅目录表格使用；不传参 = 既有全量语义零变化）。

use crate::engine::store;
use crate::resp::{ApiResp, OntoError, Result};
use crate::tenant::{current_display_user, current_tenant};
use axum::extract::Query;
use axum::Json;
use cmx_onto_model::{
    ObjectTypeDef, PropertyBaseType, PropertyTypeDef, SceneViewDef, SceneViewMeta, ViewSource,
    OntologyStore,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// StoreError → HTTP 错误映射（Conflict→409 / NotFound→404 / 其余→500）。
fn store_err(ctx: &'static str) -> impl Fn(cmx_onto_model::StoreError) -> OntoError {
    move |e| match e {
        cmx_onto_model::StoreError::Conflict(m) => OntoError::conflict(m),
        cmx_onto_model::StoreError::NotFound(m) => OntoError::not_found(m),
        other => OntoError::internal_error(format!("{ctx}: {other}")),
    }
}

// ───────────────────────────── 视图 CRUD ─────────────────────────────

/// GET /views —— 场景视图清单（行集 + auto 域默认视图读时派生合并）。
pub async fn list_views() -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let rows = store()
        .list_views(&tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("列出场景视图失败: {e}")))?;
    let metas = store()
        .list_object_types(&tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载对象清单失败: {e}")))?;

    // DAM 域聚合现算（auto 播种口径：manifest 读时派生；未分组 domain 为空 → 不播种）。
    let mut domain_counts: BTreeMap<String, u32> = BTreeMap::new();
    for m in &metas {
        let d = m.dam.domain.trim();
        if d.is_empty() {
            continue;
        }
        *domain_counts.entry(d.to_string()).or_insert(0) += 1;
    }

    let mut out: Vec<Value> = Vec::new();
    // 全部已落行名占用集合：manual 固化行占用 auto:<domain> 名后，该域不再派生虚拟条目。
    let mut seen_names: BTreeSet<String> = BTreeSet::new();
    for m in rows {
        let virtual_view = false;
        let api = m.api_name.clone();
        seen_names.insert(api.clone());
        if m.source == ViewSource::Auto {
            // auto 行（仅元数据+布局落行）的成员数也按域现算（members 恒空）。
            let key = api.strip_prefix("auto:").unwrap_or(&api).to_string();
            let mut v = meta_to_value(&m, virtual_view);
            v["objectCount"] = json!(domain_counts.get(&key).copied().unwrap_or(0));
            out.push(v);
        } else {
            out.push(meta_to_value(&m, virtual_view));
        }
    }
    // 域有类型但无行 → 虚拟条目（域消失 → 自然消失；布局保存/转手动时才落行）。
    for (domain, count) in domain_counts {
        let api = format!("auto:{domain}");
        if seen_names.contains(&api) {
            continue;
        }
        out.push(json!({
            "apiName": api,
            "displayName": format!("{domain}（域默认）"),
            "description": "按 DAM 域读时派生的默认场景（成员随域自动跟随；拖动布局或转为手动场景后固化）".to_string(),
            "dam": { "domain": domain },
            "source": "auto",
            "objectCount": count,
            "interfaceCount": 0,
            "linkCount": 0,
            "status": "experimental",
            "virtual": true,
            "version": 0,
            "members": { "objects": [], "interfaces": [], "links": [] },
        }));
    }
    Ok(Json(ApiResp::ok(json!(out))))
}

fn meta_to_value(m: &SceneViewMeta, _virtual_view: bool) -> Value {
    // virtual_view 经 serde rename = "virtual" 输出，与虚拟条目手写 json! 的键一致。
    serde_json::to_value(m).unwrap_or(Value::Null)
}

/// POST /views 请求体（偏序容忍）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SaveViewReq {
    pub api_name: String,
    pub display_name: String,
    pub description: String,
    pub dam: cmx_onto_model::DamRef,
    pub members: cmx_onto_model::ViewMembers,
    pub source: ViewSource,
    pub layout: Value,
    pub version: u32,
}
impl Default for SaveViewReq {
    fn default() -> Self {
        Self {
            api_name: String::new(),
            display_name: String::new(),
            description: String::new(),
            dam: Default::default(),
            members: Default::default(),
            source: ViewSource::Manual,
            layout: Value::Null,
            version: 0,
        }
    }
}

/// POST /views —— **直写 live om_view**（`upsert_view_locked` B0 乐观锁：`version`=0 宽松
/// 新建/覆盖，非 0 严格行锁——他人保存过 → 409）。同名单覆盖 meta/成员/来源；请求未带布局
/// （null/空对象）时**保留已有布局**——成员编辑不吞布局。
pub async fn save_view(Json(req): Json<SaveViewReq>) -> Result<Json<ApiResp<Value>>> {
    crate::archive_handlers::require_maintainer().await?;
    // 请求未带布局（null/空对象）→ 保留 live 已有布局：成员编辑不吞布局。
    let keep_layout =
        req.layout.is_null() || req.layout.as_object().is_some_and(|m| m.is_empty());
    let mut def = SceneViewDef {
        api_name: req.api_name.clone(),
        display_name: req.display_name,
        description: req.description,
        dam: req.dam,
        members: req.members,
        source: req.source,
        layout: if keep_layout { json!({}) } else { req.layout },
        version: req.version,
        // status / 弃用元数据剥离（七类纪律：只能经 /lifecycle/transition 变更）。
        status: cmx_onto_model::TypeStatus::default(),
        deprecation: None,
    };
    def.validate()
        .map_err(|e| OntoError::business_error(format!("场景视图非法: {e}")))?;
    let tenant = current_tenant();
    // links 白名单清洗（方案 §7.2 成员联动）：仅保留真实存在且两端都在成员集内的关系——
    // 移除对象后其边自动出清；写路径单点收口，前端无需先算后传。
    sanitize_view_links(&tenant, &mut def.members).await?;
    if keep_layout {
        if let Some(existing) = store()
            .get_view(&tenant, &def.api_name)
            .await
            .map_err(|e| OntoError::internal_error(format!("装载场景视图失败: {e}")))?
        {
            def.layout = existing.layout;
        }
    }
    let version = save_view_core(&tenant, def, &crate::handlers::changed_by(), None).await?;
    Ok(Json(ApiResp::ok(
        json!({ "apiName": req.api_name, "saved": true, "version": version }),
    )))
}

/// 保存场景 core（handler 与 revert 共用：落库 + 修订 + SSE）。
pub(crate) async fn save_view_core(
    tenant: &str,
    def: SceneViewDef,
    changed_by: &str,
    change_note: Option<&str>,
) -> Result<u32> {
    let version = store()
        .save_view_with_revision(&def, changed_by, change_note)
        .await
        .map_err(store_err("保存场景视图失败"))?;
    crate::events::emit(
        tenant,
        "view-changed",
        json!({
            "by": changed_by,
            "reason": format!("view-members:{}", def.api_name),
            "changeType": "members",
            "view": def.api_name,
        }),
    );
    Ok(version)
}

/// links 白名单清洗：drop 不存在的关系类型与两端不在成员集内的关系（manual 专用；auto 成员恒空不触达）。
async fn sanitize_view_links(tenant: &str, members: &mut cmx_onto_model::ViewMembers) -> Result<()> {
    if members.links.is_empty() {
        return Ok(());
    }
    let all_links = store()
        .list_link_types(tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载关系清单失败: {e}")))?;
    let objects: std::collections::BTreeSet<&str> =
        members.objects.iter().map(|s| s.as_str()).collect();
    let known: std::collections::BTreeSet<&str> =
        all_links.iter().map(|l| l.api_name.as_str()).collect();
    members.links.retain(|lk| {
        known.contains(lk.as_str())
            && all_links
                .iter()
                .find(|l| &l.api_name == lk)
                .map(|l| objects.contains(l.object_type_a.as_str()) && objects.contains(l.object_type_b.as_str()))
                .unwrap_or(false)
    });
    Ok(())
}

/// POST /views/remove 请求体。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveViewReq {
    pub api_name: String,
}

/// POST /views/remove —— **直删 live om_view 行**；auto 行豁免（域派生物化产物，成员随域
/// 自动跟随，删除无意义）。行不存在 → 幂等成功（removed=false）。二次确认由前端承担。
pub async fn remove_view(Json(req): Json<RemoveViewReq>) -> Result<Json<ApiResp<Value>>> {
    crate::archive_handlers::require_maintainer().await?;
    let tenant = current_tenant();
    let live_row = store()
        .get_view(&tenant, &req.api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载场景视图失败: {e}")))?;
    let Some(row) = live_row else {
        return Ok(Json(ApiResp::ok(json!({ "apiName": req.api_name, "removed": false }))));
    };
    if row.source == ViewSource::Auto {
        return Ok(Json(ApiResp::ok(json!(
            { "apiName": req.api_name, "removed": false, "reason": "auto 豁免" }
        ))));
    }
    let n = store()
        .delete_view(&tenant, &req.api_name)
        .await
        .map_err(store_err("删除场景视图失败"))?;
    crate::events::emit(
        &tenant,
        "view-changed",
        json!({
            "by": current_display_user(),
            "reason": format!("view-remove:{}", req.api_name),
            "changeType": "members",
            "view": req.api_name,
        }),
    );
    Ok(Json(ApiResp::ok(json!({ "apiName": req.api_name, "removed": n > 0 }))))
}

/// POST /views/layout 请求体。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveViewLayoutReq {
    pub api_name: String,
    pub layout: Value,
}

/// POST /views/layout —— 布局单列 LWW 直写（不做版本检查）；**P2 草稿轨豁免端点**：
/// 布局是物化产物不进版本语义（rev 指纹排除 layout；发布应用 DO UPDATE 不含 layout 列），
/// 保存即生效、无需过发布门。写路径授权仍收口（views 写端点组，方案 §六.4）。
/// auto 视图行不存在时按需落行（meta+layout+source=auto，永不落 members——§2.1 生命周期①）。
pub async fn save_view_layout(
    Json(req): Json<SaveViewLayoutReq>,
) -> Result<Json<ApiResp<Value>>> {
    crate::archive_handlers::require_maintainer().await?;
    if !req.api_name.starts_with("auto:") && req.api_name.is_empty() {
        return Err(OntoError::bad_request("apiName 不能为空"));
    }
    let tenant = current_tenant();
    let ok = store()
        .update_view_layout(&tenant, &req.api_name, &req.layout)
        .await
        .map_err(|e| OntoError::internal_error(format!("保存画布布局失败: {e}")))?;
    if !ok {
        // 按需落行：auto 语义（成员仍读时现算）；manual 场景被并发删除则如实 404。
        if let Some(domain) = req.api_name.strip_prefix("auto:") {
            let def = SceneViewDef {
                api_name: req.api_name.clone(),
                display_name: format!("{domain}（域默认）"),
                description: "按 DAM 域读时派生的默认场景".into(),
                dam: cmx_onto_model::DamRef {
                    domain: domain.to_string(),
                    ..Default::default()
                },
                members: Default::default(),
                source: ViewSource::Auto,
                layout: req.layout,
                version: 0,
                status: cmx_onto_model::TypeStatus::default(),
                deprecation: None,
            };
            store()
                .upsert_view_locked(&tenant, &def)
                .await
                .map_err(|e| OntoError::internal_error(format!("落行场景视图失败: {e}")))?;
            return Ok(Json(ApiResp::ok(
                json!({ "apiName": req.api_name, "saved": true, "materialized": true }),
            )));
        }
        return Err(OntoError::not_found(format!(
            "场景视图 {} 不存在（可能已被删除）",
            req.api_name
        )));
    }
    crate::events::emit(
        &tenant,
        "view-changed",
        json!({
            "by": current_display_user(),
            "reason": format!("view-layout:{}", req.api_name),
            "changeType": "layout",
            "view": req.api_name,
        }),
    );
    Ok(Json(ApiResp::ok(json!({ "apiName": req.api_name, "saved": true }))))
}

// ───────────────────────── 成员级装载（GET /graph） ─────────────────────────

/// GET /graph 查询参数。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct GraphQuery {
    pub view: Option<String>,
    /// 状态分层（D9）：studio 建模口径默认全量；其他消费方可显式收窄。
    pub include: Option<String>,
}

/// GET /graph?view=X —— 服务端组装成员级 spec（一条请求到位）。
///
/// 响应：`{ view: {...meta}, spec: {name, nodes, edges}, sharedProperties: [...] }`。
/// 节点附跨场景关系角标数据（externalCount/externalPeers：单端在场的边 → 补引入口）。
pub async fn graph(Query(q): Query<GraphQuery>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let view_name = q
        .view
        .clone()
        .ok_or_else(|| OntoError::bad_request("缺少 view 参数（场景视图 apiName）"))?;
    let filter = crate::filter::StatusFilter::parse(q.include.as_deref())?;

    let s = store();
    // 1. 解析视图 → 成员集合 + links 白名单（manual 物化 / auto 按域现算 links 不设限）。
    let (view_meta, member_objects, member_interfaces, links_whitelist, layout) =
        resolve_view(&tenant, &view_name).await?;

    // 2. 成员对象全量定义（D15 批量；清单里被并发删除的静默跳过）。
    let defs = s
        .get_object_types_batch(&tenant, &member_objects)
        .await
        .map_err(|e| OntoError::internal_error(format!("批量装载成员对象失败: {e}")))?;
    let def_by_name: BTreeMap<String, &ObjectTypeDef> =
        defs.iter().map(|d| (d.api_name.clone(), d)).collect();

    // 悬空防御（§7.5）：成员引用了已不存在的对象/接口/关系 → Warning 清单（不阻断）。
    let mut warnings: Vec<String> = Vec::new();
    for name in &member_objects {
        if !def_by_name.contains_key(name) {
            warnings.push(format!("场景引用了已不存在的对象类型「{name}」"));
        }
    }

    // 3. 接口集合 = 成员对象 implements 并集 ∪ members.interfaces。
    let mut iface_names: BTreeSet<String> = member_interfaces.iter().cloned().collect();
    for d in &defs {
        for i in &d.implements {
            iface_names.insert(i.clone());
        }
    }
    let mut live_ifaces: BTreeMap<String, String> = BTreeMap::new();
    for name in &iface_names {
        let iface = s
            .get_interface(&tenant, name)
            .await
            .map_err(|e| OntoError::internal_error(format!("装载接口失败: {e}")))?;
        match iface {
            Some(i) => {
                live_ifaces.insert(i.api_name.clone(), i.display_name.clone());
            }
            None => warnings.push(format!("场景引用了已不存在的接口「{name}」")),
        }
    }

    // 4. 关系边（方案 §7.1 D3）：边 =（link ∈ links 白名单）∧（两端在场）；
    //    auto 视图 links 现算（不设限，等价现状）。单端在场 / 未入白名单 → 角标与可加入清单。
    let all_links = s
        .list_link_types(&tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载关系清单失败: {e}")))?;
    let member_set: BTreeSet<&str> = def_by_name.keys().map(|k| k.as_str()).collect();
    let mut edges: Vec<Value> = Vec::new();
    let mut external: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut available_links: Vec<Value> = Vec::new();
    let known_links: BTreeSet<&str> = all_links.iter().map(|l| l.api_name.as_str()).collect();
    for name in links_whitelist.iter().flatten() {
        if !known_links.contains(name.as_str()) {
            warnings.push(format!("场景引用了已不存在的关系「{name}」"));
        }
    }
    for lt in &all_links {
        let a_in = member_set.contains(lt.object_type_a.as_str());
        let b_in = member_set.contains(lt.object_type_b.as_str());
        let in_whitelist = match &links_whitelist {
            Some(w) => w.contains(&lt.api_name),
            None => true,
        };
        let both = a_in && b_in;
        if both && in_whitelist {
            edges.push(json!({
                "apiName": lt.api_name,
                "source": lt.object_type_a,
                "target": lt.object_type_b,
                "displayName": lt.display_name,
                "cardinality": serde_json::to_value(lt.cardinality).unwrap_or(json!("oneToMany")),
                "status": serde_json::to_value(lt.status).unwrap_or(json!("experimental")),
            }));
        } else if both && !in_whitelist {
            // 两端在场但未入场景 links 白名单 → 实时可加入清单（审阅 3 增强裁决 §7.4.6）。
            available_links.push(json!({
                "apiName": lt.api_name,
                "displayName": lt.display_name,
                "source": lt.object_type_a,
                "target": lt.object_type_b,
                "sourceDisplayName": def_by_name.get(&lt.object_type_a).map(|d| d.display_name.clone()).unwrap_or_default(),
                "targetDisplayName": def_by_name.get(&lt.object_type_b).map(|d| d.display_name.clone()).unwrap_or_default(),
            }));
        }
        if a_in && !(b_in && in_whitelist) {
            external.entry(lt.object_type_a.clone()).or_default().push(json!({
                "apiName": lt.api_name,
                "displayName": lt.display_name,
                "peer": lt.object_type_b,
                "inScene": in_whitelist,
            }));
        }
        if b_in && !(a_in && in_whitelist) {
            external.entry(lt.object_type_b.clone()).or_default().push(json!({
                "apiName": lt.api_name,
                "displayName": lt.display_name,
                "peer": lt.object_type_a,
                "inScene": in_whitelist,
            }));
        }
    }

    // 5. 组装节点（对象全卡 + 接口胶囊；属性投影与 designer 同口径；include 过滤默认全量）。
    let mut nodes: Vec<Value> = Vec::new();
    let mut shared_refs: BTreeSet<String> = BTreeSet::new();
    for d in &defs {
        if !filter.allows(d.status) {
            continue;
        }
        let mut props: Vec<Value> = Vec::new();
        for p in &d.properties {
            if let Some(sp) = &p.shared_property {
                shared_refs.insert(sp.clone());
            }
            props.push(project_property(p, &d.primary_key, &d.title_property));
        }
        let group_path: Vec<String> = [&d.dam.domain, &d.dam.application, &d.dam.module]
            .iter()
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect();
        let peers = external.get(&d.api_name).cloned().unwrap_or_default();
        nodes.push(json!({
            "id": d.api_name,
            "kind": "object",
            "displayName": d.display_name,
            "status": serde_json::to_value(d.status).unwrap_or(json!("experimental")),
            "color": d.color,
            "icon": d.icon,
            "groupPath": group_path,
            "properties": props,
            "implements": d.implements,
            "externalCount": peers.len(),
            "externalPeers": peers,
        }));
    }
    for (name, display) in &live_ifaces {
        nodes.push(json!({
            "id": name,
            "kind": "interface",
            "displayName": display,
            "status": "active",
        }));
    }

    // 6. 共享属性详情批量（Inspector 语义类型展示用）。
    let shared_names: Vec<String> = shared_refs.into_iter().collect();
    let shared_items = s
        .get_shared_properties_batch(&tenant, &shared_names)
        .await
        .map_err(|e| OntoError::internal_error(format!("批量装载共享属性失败: {e}")))?;
    let shared_values: Vec<Value> = shared_items
        .iter()
        .map(|sp| serde_json::to_value(sp).unwrap_or(Value::Null))
        .collect();

    Ok(Json(ApiResp::ok(json!({
        "view": view_meta,
        "spec": { "name": view_name, "nodes": nodes, "edges": edges },
        "layout": layout,
        "sharedProperties": shared_values,
        "availableLinks": available_links,
        "warnings": warnings,
    }))))
}

/// 解析视图 →（meta、成员对象、成员接口、links 白名单、layout）。
/// manual 返回 members.links 白名单；auto（含虚拟条目）links 读时现算（None = 不设限）。
#[allow(clippy::type_complexity)]
async fn resolve_view(
    tenant: &str,
    view_name: &str,
) -> Result<(Value, Vec<String>, Vec<String>, Option<Vec<String>>, Value)> {
    let s = store();
    let row = s
        .get_view(tenant, view_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载场景视图失败: {e}")))?;
    if let Some(def) = row {
        let (objects, ifaces, links) = match def.source {
            ViewSource::Manual => (
                def.members.objects.clone(),
                def.members.interfaces.clone(),
                Some(def.members.links.clone()),
            ),
            ViewSource::Auto => {
                let key = def.api_name.strip_prefix("auto:").unwrap_or(&def.api_name);
                (domain_objects(tenant, key).await?, Vec::new(), None)
            }
        };
        let meta = json!({
            "apiName": def.api_name,
            "displayName": def.display_name,
            "description": def.description,
            "dam": serde_json::to_value(&def.dam).unwrap_or(json!({})),
            "source": serde_json::to_value(def.source).unwrap_or(json!("manual")),
            "objectCount": objects.len(),
            "interfaceCount": ifaces.len(),
            "linkCount": links.as_ref().map(|l| l.len()).unwrap_or(0),
            "status": serde_json::to_value(def.status).unwrap_or(json!("experimental")),
            "virtual": false,
            "version": def.version,
        });
        return Ok((meta, objects, ifaces, links, def.layout));
    }
    // 无行：仅 auto: 虚拟形态可现算；其余 404。
    let Some(domain) = view_name.strip_prefix("auto:") else {
        return Err(OntoError::not_found(format!("场景视图 {view_name} 不存在")));
    };
    let objects = domain_objects(tenant, domain).await?;
    let meta = json!({
        "apiName": view_name,
        "displayName": format!("{domain}（域默认）"),
        "description": "按 DAM 域读时派生的默认场景".to_string(),
        "dam": { "domain": domain },
        "source": "auto",
        "objectCount": objects.len(),
        "interfaceCount": 0,
        "linkCount": 0,
        "status": "experimental",
        "virtual": true,
        "version": 0,
    });
    Ok((meta, objects, Vec::new(), None, Value::Null))
}

/// 域内对象清单（auto 成员现算；域消失 → 空集 → 上层自然渲染空场景）。
pub(crate) async fn domain_objects(tenant: &str, domain: &str) -> Result<Vec<String>> {
    let metas = store()
        .list_object_types(tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载对象清单失败: {e}")))?;
    Ok(metas
        .into_iter()
        .filter(|m| m.dam.domain.trim() == domain.trim())
        .map(|m| m.api_name)
        .collect())
}

// ─────────────────── A1：共享属性批量 / A2：目录分页 ───────────────────

/// POST /shared-properties/batch 请求体。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedPropertiesBatchReq {
    pub api_names: Vec<String>,
}

/// POST /shared-properties/batch —— `{items, errors}`（缺失项按清单比对进 errors，响应形状与
/// object-types/batch 的纯数组不同：A1 新端点自带缺失表达，新页面按清单比对免二次请求）。
pub async fn get_shared_properties_batch(
    Json(req): Json<SharedPropertiesBatchReq>,
) -> Result<Json<ApiResp<Value>>> {
    if req.api_names.len() > 2000 {
        return Err(OntoError::bad_request("apiNames 数量超限（≤2000）"));
    }
    let tenant = current_tenant();
    let items = store()
        .get_shared_properties_batch(&tenant, &req.api_names)
        .await
        .map_err(|e| OntoError::internal_error(format!("批量装载共享属性失败: {e}")))?;
    let found: BTreeSet<&str> = items.iter().map(|i| i.api_name.as_str()).collect();
    let errors: Vec<String> = req
        .api_names
        .iter()
        .filter(|n| !found.contains(n.as_str()))
        .cloned()
        .collect();
    let item_values: Vec<Value> = items
        .iter()
        .map(|sp| serde_json::to_value(sp).unwrap_or(Value::Null))
        .collect();
    Ok(Json(ApiResp::ok(json!({ "items": item_values, "errors": errors }))))
}

/// GET /object-types 查询参数（A2；全缺省 = 既有全量数组语义，旧设计器零感知）。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ObjectTypesListQuery {
    pub q: Option<String>,
    pub dam: Option<String>,
    pub page: Option<u32>,
    pub size: Option<u32>,
}

/// GET /object-types —— 清单：不传参返回全量数组（既有语义）；传 q/dam/page/size 任一
/// 返回分页信封 `{rows, total, page, size}`（仅新页面目录表格使用）。
pub async fn list_object_types(
    Query(qp): Query<ObjectTypesListQuery>,
) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let paged =
        qp.q.is_some() || qp.dam.is_some() || qp.page.is_some() || qp.size.is_some();
    if !paged {
        let metas = store()
            .list_object_types(&tenant)
            .await
            .map_err(|e| OntoError::internal_error(format!("列出对象类型失败: {e}")))?;
        return Ok(Json(ApiResp::ok(json!(metas))));
    }
    let page = qp.page.unwrap_or(1);
    let size = qp.size.unwrap_or(50);
    let (rows, total) = store()
        .list_object_types_paged(&tenant, qp.q.as_deref().unwrap_or(""), qp.dam.as_deref().unwrap_or(""), page, size)
        .await
        .map_err(|e| OntoError::internal_error(format!("查询对象目录失败: {e}")))?;
    let row_values: Vec<Value> = rows
        .iter()
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .collect();
    Ok(Json(ApiResp::ok(json!({
        "rows": row_values,
        "total": total,
        "page": page,
        "size": size,
    }))))
}

// ───────────────────────── spec 属性投影 ─────────────────────────

/// 属性定义 → 组件 spec 属性行（与 designer.js projectProp 同口径；层块递归）。
fn project_property(p: &PropertyTypeDef, primary_key: &str, title_property: &str) -> Value {
    let mut obj = json!({
        "apiName": p.api_name,
        "displayName": p.display_name,
        "baseType": serde_json::to_value(p.base_type).unwrap_or(json!("string")),
        "isPrimaryKey": p.api_name == primary_key,
        "isTitle": p.api_name == title_property,
        "required": p.required,
        "isIndexed": p.is_indexed,
    });
    if let Some(st) = &p.semantic_type {
        obj["semanticType"] = json!(st);
    }
    if matches!(p.base_type, PropertyBaseType::Array | PropertyBaseType::Struct) {
        let is_level = p
            .constraints
            .get("level")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if is_level {
            obj["isLevel"] = json!(true);
            if let Some(e) = p.constraints.get("entity").and_then(|v| v.as_str()) {
                obj["entityName"] = json!(e);
            }
            let children: Vec<Value> = p
                .constraints
                .get("children")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|c| serde_json::from_value::<PropertyTypeDef>(c.clone()).ok())
                        .map(|cp| project_property(&cp, primary_key, title_property))
                        .collect()
                })
                .unwrap_or_default();
            obj["children"] = json!(children);
        }
    }
    obj
}

