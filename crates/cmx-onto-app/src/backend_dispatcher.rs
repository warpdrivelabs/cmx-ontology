//! 读路径分派器（方案 20260918 §5.1/§5.7）：按对象类型的绑定（om_source_mapping.mode）选择 backend，
//! 并把 SearchAround / 集合并交差的**跨源组合桥接为 pk 集合语义**。
//!
//! 契约分层落地：
//! - `Materialized`（默认）吃**完整代数树**——树中不含 virtual 节点时原样透传（行为零变化，快路径）；
//! - `PgDirect` 只吃 `Base/Filter/Static` 子树——terminal 为 virtual 且树本身 simple（无 SearchAround /
//!   集合运算/无跨源）时直接下推（分页语义完整）；
//! - 其余（树中含 virtual 节点但结构不 simple）→ **桥接**：自顶向下找到「产出类型为 virtual 的最大
//!   子树」，整树解析为 pk 集（谓词随之下推，上限 [`PK_BRIDGE_MAX`]，超出整查询拒绝），替换为
//!   `Static` 后继续分派。**不做 join 下推的跨源联邦**。
//!
//! 安全（E6/D5 三道保险）：`ONTO_VIRTUAL_QUERY` 总闸（默认 off——off 时 virtual 类型查询明确报
//! 「虚拟直查已停用」）；virtual 绑定要求 authz 模式 ≠ off（bind 时强制，见 source_handlers）。
//! 每请求查 def + mapping 元数据（量级小、走索引；红线禁进程内业务缓存，请求级去重除外）。

use crate::object_engine::link_resolver;
use crate::resp::{OntoError, Result};
use crate::tenancy;
use cmx_onto_model::backend::{BackendCtx, ObjectDataBackend, PK_BRIDGE_MAX};
use cmx_onto_model::objectset::{Aggregation, LinkDirection, ObjectPage, ObjectSet, Page};
use cmx_onto_model::{LinkResolver, MappingMode, ObjectTypeDef, OntologyStore, SourceMapping, StoreError};
use serde_json::Value;
use std::collections::HashMap;

// ————————————————————————— 总闸与 authz 三态（E6 / D5） —————————————————————————

/// `ONTO_VIRTUAL_QUERY` 全局总闸：默认 **off**（发布保险丝；回滚 = 设 off，全部回 Materialized）。
/// 显式 `on` / `true` / `1` 才放行虚拟查询。
pub fn virtual_query_enabled() -> bool {
    std::env::var("ONTO_VIRTUAL_QUERY")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "on" | "true" | "1"))
        .unwrap_or(false)
}

/// authz 三态开关最小实现（E6：env `ONTO_AUTHZ_MODE=off|local|dataauth`；S1 完整修复后替换）。
/// off = 自报身份模式（auth.rs 的 X-Tenant/X-User 直通）；virtual 绑定硬前置要求 ≠ off。
pub fn authz_mode() -> String {
    std::env::var("ONTO_AUTHZ_MODE")
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
        .or_else(|| {
            cmx_utils::ConfigManager::try_global()
                .and_then(|cm| cm.get_string("onto.authz_mode").ok())
                .map(|v| v.trim().to_ascii_lowercase())
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_else(|| "off".to_string())
}

/// 虚拟绑定前置闸（bind 时强制；E6）：总闸 on + authz ≠ off。
pub fn ensure_bind_gate() -> Result<()> {
    if !virtual_query_enabled() {
        return Err(OntoError::business_error(
            "虚拟直查总闸未开启（ONTO_VIRTUAL_QUERY=off）：设为 on 并满足 authz≠off 后方可绑定虚拟源",
        ));
    }
    if authz_mode() == "off" {
        return Err(OntoError::business_error(
            "authz 模式 = off（自报身份）禁止绑定虚拟源：虚拟直查把读取面扩大到业务库实库，\
             须先启用认证（ONTO_AUTHZ_MODE=local|dataauth）",
        ));
    }
    Ok(())
}

/// 虚拟查询总闸（查询路径；off 时明确报「虚拟直查已停用」——绑定类型退化为不可查而非静默回退）。
fn ensure_query_gate() -> Result<()> {
    if !virtual_query_enabled() {
        return Err(OntoError::business_error(
            "虚拟直查已停用（ONTO_VIRTUAL_QUERY=off）：该类型为虚拟绑定，数据留源系统未物化；请联系管理员开启总闸",
        ));
    }
    Ok(())
}

// ————————————————————————— 绑定装载（E1：mapping 行是唯一权威） —————————————————————————

fn funnel() -> cmx_onto_store_pg::FunnelStore {
    cmx_onto_store_pg::FunnelStore::new(tenancy::current_db_id())
}

fn store() -> cmx_onto_store_pg::PgOntologyStore {
    cmx_onto_store_pg::PgOntologyStore::new(tenancy::current_db_id())
}

/// 某对象类型的绑定行（None = 未绑定）。注意：mode 为权威值；`om_object_type.datasource` 仅展示指针。
pub async fn binding_of(tenant: &str, object_type: &str) -> Result<Option<SourceMapping>> {
    let _ = tenant; // om_source_mapping 落租户库，当前 db_id 已含租户边界；参数保留语义占位
    funnel()
        .load_mapping(object_type)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载绑定失败: {e}")))
}

/// 对象类型定义（virtual 分派需要属性基型）。
async fn def_of(tenant: &str, object_type: &str) -> Result<ObjectTypeDef> {
    store()
        .get_object_type(tenant, object_type)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载对象类型失败: {e}")))?
        .ok_or_else(|| OntoError::business_error(format!("对象类型 {object_type} 未定义")))
}

fn materialized_backend() -> cmx_onto_store_pg::MaterializedBackend {
    cmx_onto_store_pg::MaterializedBackend::new()
}

fn pg_direct_backend() -> cmx_onto_store_pg::PgDirectBackend {
    cmx_onto_store_pg::PgDirectBackend::new()
}

fn ctx_for(tenant: &str, def: ObjectTypeDef, mapping: SourceMapping) -> BackendCtx {
    BackendCtx {
        tenant: tenant.to_string(),
        onto_db_id: tenancy::current_db_id(),
        def,
        mapping,
    }
}

// ————————————————————————— 终端类型解析（SearchAround 需 link 两端） —————————————————————————

/// 解析对象集的产出类型（SearchAround 借 link 定义两端推得；解析不出即 Err）。
/// async 递归经 Box::pin（深度 = 树深，有限）。
fn resolve_terminal<'a>(tenant: &'a str, set: &'a ObjectSet) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>> {
    Box::pin(resolve_terminal_inner(tenant, set))
}

async fn resolve_terminal_inner(tenant: &str, set: &ObjectSet) -> Result<String> {
    match set {
        ObjectSet::Base { object_type } | ObjectSet::Static { object_type, .. } => {
            Ok(object_type.clone())
        }
        ObjectSet::Filter { source, .. } => resolve_terminal(tenant, source).await,
        ObjectSet::Union { left, .. } | ObjectSet::Intersect { left, .. } | ObjectSet::Subtract { left, .. } => {
            resolve_terminal(tenant, left).await
        }
        ObjectSet::SearchAround { source, link, direction } => {
            let src_ty = resolve_terminal(tenant, source).await?;
            let lr = link_resolver();
            let (a, b) = lr
                .ends(tenant, link)
                .await
                .map_err(|e| OntoError::internal_error(format!("解析关系两端失败: {e}")))?
                .ok_or_else(|| {
                    OntoError::business_error(format!("关系类型 {link} 未定义，无法 Search-Around"))
                })?;
            match direction {
                LinkDirection::Forward if a == src_ty => Ok(b),
                LinkDirection::Reverse if b == src_ty => Ok(a),
                _ => Err(OntoError::business_error(format!(
                    "关系 {link} 与源类型 {src_ty} 方向不匹配"
                ))),
            }
        }
    }
}

/// 树中出现的全部对象类型（去重）。
fn collect_types(set: &ObjectSet, out: &mut Vec<String>) {
    match set {
        ObjectSet::Base { object_type } | ObjectSet::Static { object_type, .. } => {
            if !out.contains(object_type) {
                out.push(object_type.clone());
            }
        }
        ObjectSet::Filter { source, .. } => collect_types(source, out),
        ObjectSet::SearchAround { source, .. } => collect_types(source, out),
        ObjectSet::Union { left, right }
        | ObjectSet::Intersect { left, right }
        | ObjectSet::Subtract { left, right } => {
            collect_types(left, out);
            collect_types(right, out);
        }
    }
}

// ————————————————————————— 对外主入口（四读入口共用） —————————————————————————

/// 加载一页（`/object-sets/load`、`/secure/object-sets/load`、links searchAround 共用）。
pub async fn load(tenant: &str, set: &ObjectSet, page: &Page) -> Result<ObjectPage> {
    let terminal = resolve_terminal(tenant, set).await?;
    let binding = binding_of(tenant, &terminal).await?;
    let is_virtual = matches!(binding.as_ref(), Some(m) if m.mode == MappingMode::Virtual);

    // terminal 虚拟：总闸预检 + 树形判定（simple 直接下推；否则桥接为 Static 再下推）。
    if is_virtual {
        ensure_query_gate()?;
        let def = def_of(tenant, &terminal).await?;
        let mapping = binding.expect("is_virtual");
        let backend = pg_direct_backend();
        if is_simple_subtree(set) {
            let ctx = ctx_for(tenant, def, mapping);
            return backend
                .load(&ctx, set, page)
                .await
                .map_err(backend_err("虚拟直查加载失败"));
        }
        let bridged = bridge_set(tenant, set).await?;
        let ctx = ctx_for(tenant, def, mapping);
        return backend
            .load(&ctx, &bridged, page)
            .await
            .map_err(backend_err("虚拟直查（桥接）加载失败"));
    }

    // terminal 物化（含未绑定）：树中含 virtual 节点 → 桥接替换；否则原样透传（快路径，零变化）。
    let bridged = bridge_set(tenant, set).await?;
    let mapping = binding.unwrap_or_default();
    let def = def_of(tenant, &terminal).await?;
    materialized_backend()
        .load(&ctx_for(tenant, def, mapping), &bridged, page)
        .await
        .map_err(backend_err("加载对象集失败"))
}

/// 聚合（`/object-sets/aggregate`）：分派口径同 load。
pub async fn aggregate(tenant: &str, set: &ObjectSet, agg: &Aggregation) -> Result<Value> {
    let terminal = resolve_terminal(tenant, set).await?;
    let binding = binding_of(tenant, &terminal).await?;
    let is_virtual = matches!(binding.as_ref(), Some(m) if m.mode == MappingMode::Virtual);

    if is_virtual {
        ensure_query_gate()?;
        let def = def_of(tenant, &terminal).await?;
        let mapping = binding.expect("is_virtual");
        let backend = pg_direct_backend();
        if is_simple_subtree(set) {
            let ctx = ctx_for(tenant, def, mapping);
            return backend
                .aggregate(&ctx, set, agg)
                .await
                .map_err(backend_err("虚拟直查聚合失败"));
        }
        let bridged = bridge_set(tenant, set).await?;
        let ctx = ctx_for(tenant, def, mapping);
        return backend
            .aggregate(&ctx, &bridged, agg)
            .await
            .map_err(backend_err("虚拟直查（桥接）聚合失败"));
    }

    let bridged = bridge_set(tenant, set).await?;
    let mapping = binding.unwrap_or_default();
    let def = def_of(tenant, &terminal).await?;
    materialized_backend()
        .aggregate(&ctx_for(tenant, def, mapping), &bridged, agg)
        .await
        .map_err(backend_err("聚合失败"))
}

// ————————————————————————— 桥接：virtual 子树 → pk 集合 Static —————————————————————————

/// 自顶向下遍历：产出类型为 virtual 的子树 → 整树解析 pk 集（谓词下推）→ `Static` 替换。
/// 树中不含 virtual 节点时原样返回（物化快路径零重排，R4 行为零变化）。
async fn bridge_set(tenant: &str, set: &ObjectSet) -> Result<ObjectSet> {
    // 请求级绑定缓存（每请求查一次/类型；非进程内跨请求缓存，集群红线合规）。
    let mut cache: HashMap<String, Option<SourceMapping>> = HashMap::new();
    let mut def_cache: HashMap<String, ObjectTypeDef> = HashMap::new();
    bridge_node(tenant, set, &mut cache, &mut def_cache).await
}

/// 请求级绑定缓存：无则查库插入（每请求一次/类型；非进程内跨请求缓存）。
async fn binding_cached(
    cache: &mut HashMap<String, Option<SourceMapping>>,
    tenant: &str,
    t: &str,
) -> Result<Option<SourceMapping>> {
    if !cache.contains_key(t) {
        let b = binding_of(tenant, t).await?;
        cache.insert(t.to_string(), b);
    }
    Ok(cache.get(t).cloned().flatten())
}

async fn def_cached(
    cache: &mut HashMap<String, ObjectTypeDef>,
    tenant: &str,
    t: &str,
) -> Result<ObjectTypeDef> {
    if let Some(d) = cache.get(t) {
        return Ok(d.clone());
    }
    let d = def_of(tenant, t).await?;
    cache.insert(t.to_string(), d.clone());
    Ok(d)
}

/// 递归桥接（async 递归经 Box::pin；深度 = 树中虚拟嵌套层数，有限）。
fn bridge_node<'a>(
    tenant: &'a str,
    set: &'a ObjectSet,
    cache: &'a mut HashMap<String, Option<SourceMapping>>,
    def_cache: &'a mut HashMap<String, ObjectTypeDef>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ObjectSet>> + Send + 'a>> {
    Box::pin(bridge_node_inner(tenant, set, cache, def_cache))
}

async fn bridge_node_inner(
    tenant: &str,
    set: &ObjectSet,
    cache: &mut HashMap<String, Option<SourceMapping>>,
    def_cache: &mut HashMap<String, ObjectTypeDef>,
) -> Result<ObjectSet> {
    let terminal = resolve_terminal(tenant, set).await?;
    let binding = binding_cached(cache, tenant, &terminal).await?;
    if matches!(binding.as_ref(), Some(m) if m.mode == MappingMode::Virtual) {
        // 产出类型为虚拟的最大子树：整树解析 pk 集（virtual 端谓词随下推），替换为 Static。
        let pks = resolve_virtual_pks(tenant, set, &terminal, cache, def_cache).await?;
        return Ok(ObjectSet::Static { object_type: terminal, primary_keys: pks });
    }
    Ok(match set {
        ObjectSet::Filter { source, predicate } => ObjectSet::Filter {
            source: Box::new(bridge_node(tenant, source, cache, def_cache).await?),
            predicate: predicate.clone(),
        },
        ObjectSet::SearchAround { source, link, direction } => ObjectSet::SearchAround {
            source: Box::new(bridge_node(tenant, source, cache, def_cache).await?),
            link: link.clone(),
            direction: *direction,
        },
        ObjectSet::Union { left, right } => ObjectSet::Union {
            left: Box::new(bridge_node(tenant, left, cache, def_cache).await?),
            right: Box::new(bridge_node(tenant, right, cache, def_cache).await?),
        },
        ObjectSet::Intersect { left, right } => ObjectSet::Intersect {
            left: Box::new(bridge_node(tenant, left, cache, def_cache).await?),
            right: Box::new(bridge_node(tenant, right, cache, def_cache).await?),
        },
        ObjectSet::Subtract { left, right } => ObjectSet::Subtract {
            left: Box::new(bridge_node(tenant, left, cache, def_cache).await?),
            right: Box::new(bridge_node(tenant, right, cache, def_cache).await?),
        },
        passthrough @ (ObjectSet::Base { .. } | ObjectSet::Static { .. }) => passthrough.clone(),
    })
}

/// 解析「产出类型为 virtual 的子树」的 pk 集合（fail-closed，上限 [`PK_BRIDGE_MAX`]）：
/// 产出端拆解为 `Base/Filter/Static` 可下推树（SearchAround 产物转 In 谓词；集合运算产物拒绝），
/// 源端先经 [`bridge_node`] 桥接（virtual 源端递归解析）。
fn resolve_virtual_pks<'a>(
    tenant: &'a str,
    set: &'a ObjectSet,
    terminal: &'a str,
    cache: &'a mut HashMap<String, Option<SourceMapping>>,
    def_cache: &'a mut HashMap<String, ObjectTypeDef>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<String>>> + Send + 'a>> {
    Box::pin(resolve_virtual_pks_inner(tenant, set, terminal, cache, def_cache))
}

async fn resolve_virtual_pks_inner(
    tenant: &str,
    set: &ObjectSet,
    terminal: &str,
    cache: &mut HashMap<String, Option<SourceMapping>>,
    def_cache: &mut HashMap<String, ObjectTypeDef>,
) -> Result<Vec<String>> {
    if !virtual_query_enabled() {
        return Err(OntoError::business_error(
            "虚拟直查已停用（ONTO_VIRTUAL_QUERY=off）：虚拟端无法参与组合查询",
        ));
    }
    let pushable = decompose_virtual(tenant, set, terminal, cache, def_cache).await?;
    let binding = binding_cached(cache, tenant, terminal)
        .await?
        .ok_or_else(|| OntoError::internal_error(format!("类型 {terminal} 绑定缺失")))?;
    let def = def_cached(def_cache, tenant, terminal).await?;
    let ctx = ctx_for(tenant, def, binding);
    pg_direct_backend()
        .resolve_pks(&ctx, &pushable, PK_BRIDGE_MAX as u32)
        .await
        .map_err(backend_err("虚拟端 pk 集合解析失败"))
}

/// 产出端拆解：把以 virtual 类型为产出的子树化简为 PgDirect 可吃的 `Base/Filter/Static`。
async fn decompose_virtual(
    tenant: &str,
    set: &ObjectSet,
    terminal: &str,
    cache: &mut HashMap<String, Option<SourceMapping>>,
    def_cache: &mut HashMap<String, ObjectTypeDef>,
) -> Result<ObjectSet> {
    match set {
        ObjectSet::Base { .. } | ObjectSet::Static { .. } => Ok(set.clone()),
        ObjectSet::Filter { source, predicate } => {
            // 谓词属于产出端（与 terminal 同类型）；源端先桥接（virtual 源端 → Static）。
            let bridged_src = bridge_node(tenant, source, cache, def_cache).await?;
            Ok(ObjectSet::Filter {
                source: Box::new(bridged_src),
                predicate: predicate.clone(),
            })
        }
        ObjectSet::SearchAround { source, link, .. } => {
            // 方向合法性已由 resolve_terminal 校验（Forward ⇔ 源在 A 端）。
            search_around_to_filter(tenant, source, link, terminal, cache, def_cache).await
        }
        ObjectSet::Union { .. } | ObjectSet::Intersect { .. } | ObjectSet::Subtract { .. } => Err(
            OntoError::business_error(
                "虚拟端不支持作为集合并/交/差的直接产物（分页语义破碎）；请把虚拟端包在过滤/关系遍历内，或改用物化模式",
            ),
        ),
    }
}

/// SearchAround 产物为 virtual 端：按关系 backing 生成对虚拟端的 In 谓词过滤（Q3：仅 FK 可桥接）。
///
/// - FK 落在虚拟端（side=T）：连接值 = 源端 pk 集 → `Filter{Base{T}, In{fkProperty, pks}}`；
/// - FK 落在物化源端（side=X）：连接值 = 源端 fk 属性值集（load 取 props）→
///   `Filter{Base{T}, In{primaryKey|targetProperty, values}}`；
/// - Edge / JoinTable / Intermediary backing：拒绝（virtual 端无 ol_edge/join 表，Q3 决策）。
async fn search_around_to_filter(
    tenant: &str,
    source: &ObjectSet,
    link: &str,
    terminal: &str,
    cache: &mut HashMap<String, Option<SourceMapping>>,
    def_cache: &mut HashMap<String, ObjectTypeDef>,
) -> Result<ObjectSet> {
    let src_ty = resolve_terminal(tenant, source).await?;
    let lr = link_resolver();
    let (a, b) = lr
        .ends(tenant, link)
        .await
        .map_err(|e| OntoError::internal_error(format!("解析关系两端失败: {e}")))?
        .ok_or_else(|| OntoError::business_error(format!("关系类型 {link} 未定义")))?;
    let src_is_a = a == src_ty;
    if !src_is_a && b != src_ty {
        return Err(OntoError::business_error(format!(
            "关系 {link} 与源类型 {src_ty} 方向不匹配"
        )));
    }
    let lt = store()
        .get_link_type(tenant, link)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载关系类型失败: {e}")))?
        .ok_or_else(|| OntoError::business_error(format!("关系类型 {link} 未定义")))?;
    let backing = lt.backing_parsed();
    let fk = match backing {
        cmx_onto_model::LinkBacking::ForeignKey { property, side, target_property } => {
            (property, side, target_property)
        }
        other => {
            return Err(OntoError::business_error(format!(
                "关系 {link} 的 backing（{other:?}）不支持桥接虚拟端：仅 FK 关系可跨虚拟/物化遍历（Q3）"
            )));
        }
    };
    let terminal_def = def_cached(def_cache, tenant, terminal).await?;
    let src_is_fk_side = matches!(
        (src_is_a, fk.1),
        (true, cmx_onto_model::LinkEnd::A) | (false, cmx_onto_model::LinkEnd::B)
    );
    // 连接值集合：FK 在虚拟端 → 源端 pk 集；FK 在源端 → 源端 fk 属性值集。
    let values: Vec<String> = if src_is_fk_side {
        // FK 属性在源端（物化）：load 分页取属性值集（上限 PK_BRIDGE_MAX；virtual 源端 M1 不支持）。
        let src_binding = binding_cached(cache, tenant, &src_ty).await?;
        if matches!(src_binding.as_ref(), Some(m) if m.mode == MappingMode::Virtual) {
            return Err(OntoError::business_error(
                "虚拟端作 FK 持有端的关系遍历暂不支持（M1）：请把 FK 放到虚拟端一侧，或该关系走物化端",
            ));
        }
        load_prop_values(&src_ty, source, &fk.0, tenant).await?
    } else {
        // FK 属性在虚拟端（terminal）：连接值 = 源端 pk 集（源端 virtual 已由 bridge 成 Static）。
        let bridged = bridge_node(tenant, source, cache, def_cache).await?;
        match bridged {
            ObjectSet::Static { primary_keys, .. } => primary_keys,
            other => {
                // 物化源端原树 → MaterializedBackend.resolve_pks。
                let m_def = def_cached(def_cache, tenant, &src_ty).await?;
                materialized_backend()
                    .resolve_pks(
                        &ctx_for(tenant, m_def, SourceMapping::default()),
                        &other,
                        PK_BRIDGE_MAX as u32,
                    )
                    .await
                    .map_err(backend_err("源端 pk 集合解析失败"))?
            }
        }
    };
    if values.is_empty() {
        // 空连接集 → 恒假过滤（合法空结果，非错误）。
        return Ok(ObjectSet::Static { object_type: terminal.to_string(), primary_keys: vec![] });
    }
    // In 谓词的属性：FK 在虚拟端 → fk 属性名；FK 在源端 → targetProperty 或 terminal 主键。
    let in_prop = if !src_is_fk_side {
        fk.0.clone()
    } else {
        fk.2.clone().unwrap_or_else(|| terminal_def.primary_key.clone())
    };
    if in_prop.is_empty() {
        return Err(OntoError::business_error(format!(
            "关系 {link} 缺可桥接属性（targetProperty/primaryKey 均未定义）"
        )));
    }
    let values_json: Vec<Value> = values.into_iter().map(Value::String).collect();
    Ok(ObjectSet::Filter {
        source: Box::new(ObjectSet::Base { object_type: terminal.to_string() }),
        predicate: cmx_onto_model::objectset::Predicate::In { property: in_prop, values: values_json },
    })
}

/// 物化端属性值集解析（load 分页取 props[prop]；上限 PK_BRIDGE_MAX，超出拒绝）。
async fn load_prop_values(
    object_type: &str,
    set: &ObjectSet,
    prop: &str,
    tenant: &str,
) -> Result<Vec<String>> {
    let def = def_of(tenant, object_type).await?;
    let backend = materialized_backend();
    let ctx = ctx_for(tenant, def, SourceMapping::default());
    let mut values = Vec::new();
    let mut offset = 0u32;
    loop {
        let page = backend
            .load(&ctx, set, &Page { limit: 1000, offset })
            .await
            .map_err(backend_err("源端属性值集解析失败"))?;
        for r in &page.rows {
            if let Some(v) = r.properties.get(prop)
                && let Some(s) = v.as_str() {
                    values.push(s.to_string());
                }
        }
        let got = page.rows.len();
        if values.len() > PK_BRIDGE_MAX {
            return Err(OntoError::business_error(format!(
                "源端属性值集超出桥接上限（>{PK_BRIDGE_MAX}）：请收紧过滤条件"
            )));
        }
        if got < 1000usize || !page.has_more {
            return Ok(values);
        }
        offset += 1000;
    }
}

/// 树是否纯 `Base/Filter/Static`（可直接下推，无 SearchAround/集合运算）。
fn is_simple_subtree(set: &ObjectSet) -> bool {
    match set {
        ObjectSet::Base { .. } | ObjectSet::Static { .. } => true,
        ObjectSet::Filter { source, .. } => is_simple_subtree(source),
        _ => false,
    }
}

fn backend_err(prefix: &str) -> impl Fn(StoreError) -> OntoError + '_ {
    move |e| OntoError::business_error(format!("{prefix}: {e}"))
}

// ————————————————————————— E3 守卫（供写路径与函数/动作读路径复用） —————————————————————————

/// 对象类型是否虚拟绑定（写保护矩阵 / 读端内部路径判定）。
pub async fn is_virtual_type(tenant: &str, object_type: &str) -> Result<bool> {
    Ok(matches!(
        binding_of(tenant, object_type).await?.as_ref(),
        Some(m) if m.mode == MappingMode::Virtual
    ))
}

/// E3 写保护：virtual 类型的一切对象写路径 4xx（设计期由前端隐藏入口，执行期在此硬拒）。
pub async fn ensure_writable(tenant: &str, object_type: &str) -> Result<()> {
    if is_virtual_type(tenant, object_type).await? {
        return Err(OntoError::business_error(format!(
            "对象类型 {object_type} 为虚拟直查绑定（mode=virtual）：数据留源系统只读；\
             写操作请走业务系统或动作副作用（side_effects），不从本体直写"
        )));
    }
    Ok(())
}

/// E3 读端内部路径守卫：函数 objectSet/object/Aggregation 装载、动作参数装载引用 virtual 类型
/// → 4xx（防「静默空集」假结果——内部装载不走 PEP/分派口径，fail-fast 显式拒绝，二轮 P1-2）。
pub async fn ensure_set_internal_loadable(tenant: &str, set: &ObjectSet) -> Result<()> {
    let mut types = Vec::new();
    collect_types(set, &mut types);
    for t in types {
        if is_virtual_type(tenant, &t).await? {
            return Err(OntoError::business_error(format!(
                "对象类型 {t} 为虚拟直查绑定：函数/动作的内部数据装载暂不支持虚拟类型（避免静默空集）；\
                 请改用物化类型或显式 /object-sets/load 查询"
            )));
        }
    }
    Ok(())
}

/// 供 handlers 展示用的绑定摘要（GET /object-types/datasource 等）。
pub async fn binding_summary(tenant: &str, object_type: &str) -> Result<Value> {
    let binding = binding_of(tenant, object_type).await?;
    let def = store().get_object_type(tenant, object_type).await.ok().flatten();
    let pointer = def.as_ref().and_then(|d| d.datasource.clone());
    let gate = virtual_query_enabled();
    Ok(serde_json::json!({
        "objectType": object_type,
        "bound": binding.is_some(),
        "mode": binding.as_ref().map(|m| m.mode.as_str()),
        "sourceId": binding.as_ref().and_then(|m| m.source_id.clone()),
        "sourceDbId": binding.as_ref().and_then(|m| m.source_db_id.clone()),
        "resource": binding.as_ref().and_then(|m| m.resource.clone()),
        "keyColumns": binding.as_ref().map(|m| m.key_columns.clone()),
        "titleColumn": binding.as_ref().and_then(|m| m.title_column.clone()),
        "propertyMap": binding.as_ref().map(|m| m.property_map.iter()
            .map(|(s, p)| serde_json::json!({"source": s, "property": p}))
            .collect::<Vec<_>>()),
        "required": binding.as_ref().map(|m| m.required.clone()),
        "virtualQueryEnabled": gate,
        "authzMode": authz_mode(),
        "pointer": pointer,
    }))
}
