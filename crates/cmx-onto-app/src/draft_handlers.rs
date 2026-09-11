//! 草稿/发布双轨 handler（本体工作室 P2，方案 §2.4/§2.5/§六.4/§七）。
//!
//! 端点组（全部**新增**，既有端点零改动；POST 语义对齐工作区 API 硬规范——无 PUT/可变路径段）：
//! - `GET /draft`：惰性 fork（无行 → live 全量快照 + om_view 拷贝落行，base_rev = live 指纹）。
//! - `POST /draft/save`：整体保存草稿（行级乐观锁；他人保存过 → 409，与 base_rev 409 两源可区分）。
//! - `POST /releases/preview`：发布预览 dry-run（校验 + 元素级 diff + 基线检查 + rev 去重预判）。
//! - `POST /releases/publish`：发布门（base_rev 比对 409 → 校验 → 原子应用 + 打版本 + SSE 广播）。
//! - `GET /versions/diff?a=&b=`：版本对 / live 对比（元素级 diff；只读仅参 GET）。
//! - `POST /versions/restore`：回滚 = 写草稿（减法 deletions = live − 快照；过发布门，无绕过评审后门）。
//! - `GET /me/roles`：当前用户角色/权限码（页内按角色渲染 + 门户 `__PORTAL_PERMISSION_IDS` 注入取数）。
//!
//! 授权：写端点组（draft/releases/views 写/restore）过 [`require_maintainer`]——om_maintainer
//! 白名单**空表 = 开放**（P1 全员维护等效；有行 = 仅命中者可写，评审门不可被 curl 绕过）。

use crate::engine::store;
use crate::resp::{ApiResp, OntoError, Result};
use crate::tenant::{current_display_user, current_roles, current_tenant, current_user};
use axum::extract::Query;
use axum::Json;
use cmx_onto_model::{
    diff_snapshots, snapshot_fingerprint, validate_draft, DraftContent, DraftRow, DeletionRef,
    IssueSeverity, KIND_ACTION, KIND_FUNCTION, KIND_INTERFACE, KIND_LINK, KIND_OBJECT,
    KIND_SHARED,
};
use serde::Deserialize;
use serde_json::{json, Value};

// ───────────────────────────── 授权 ─────────────────────────────

/// 维护角色守卫：om_maintainer 空表 = 开放（P1 等效语义，生产录入行即收敛白名单模式）；
/// 有行时按 用户 id / 展示名 / 角色 三路匹配，任一命中放行，否则 403（前端统一降级隐藏写入口）。
pub async fn require_maintainer() -> Result<()> {
    let rows = store()
        .list_maintainers()
        .await
        .map_err(|e| OntoError::internal_error(format!("装载维护白名单失败: {e}")))?;
    if rows.is_empty() {
        return Ok(());
    }
    let user = current_user().unwrap_or_default();
    let display = current_display_user().unwrap_or_default();
    let roles = current_roles();
    let hit = rows.iter().any(|(subject, kind)| match kind.as_str() {
        "role" => roles.iter().any(|r| r == subject),
        _ => subject == &user || subject == &display,
    });
    if hit {
        Ok(())
    } else {
        Err(OntoError::forbidden(
            "当前账号无本体维护权限（评审门：草稿/发布/场景编辑仅维护角色可用）",
        ))
    }
}

/// 当前账号是否维护角色（不抛错——`/me/roles` 与页内渲染判定用）。
async fn is_maintainer() -> Result<bool> {
    Ok(store()
        .list_maintainers()
        .await
        .map_err(|e| OntoError::internal_error(format!("装载维护白名单失败: {e}")))?
        .is_empty()
        || {
            let user = current_user().unwrap_or_default();
            let display = current_display_user().unwrap_or_default();
            let roles = current_roles();
            store()
                .list_maintainers()
                .await
                .map_err(|e| OntoError::internal_error(format!("装载维护白名单失败: {e}")))?
                .iter()
                .any(|(subject, kind)| match kind.as_str() {
                    "role" => roles.iter().any(|r| r == subject),
                    _ => subject == &user || subject == &display,
                })
        })
}

/// GET /me/roles —— 当前用户角色/权限码（无守卫：人人可查自己的角色）。
pub async fn me_roles() -> Result<Json<ApiResp<Value>>> {
    let maintainer = is_maintainer().await?;
    Ok(Json(ApiResp::ok(json!({
        "user": current_user(),
        "username": current_display_user(),
        "roles": current_roles(),
        "maintainer": maintainer,
        // 权限码：维护角色持「本体工作室」菜单码；消费角色空集（菜单层不可见 + 页内降级）。
        "permissionCodes": if maintainer { vec!["portal.onto.studio"] } else { vec![] },
    }))))
}

// ───────────────────────────── 草稿 ─────────────────────────────

/// GET /draft —— 读草稿；无行则惰性 fork（七类 = live 全量、views = live om_view 拷贝、
/// base_rev = live 指纹）。**不组装 live 全量快照**：dirty/liveRev 从 base_rev 推出
/// （fork 后 live 侧不变则两者恒等；live 被他人发布推进的场景由发布门 `releases_publish`
/// 的 base_rev 实时比对 409 兜底）——高频读省去七表全量拉取与两轮全量指纹。
pub async fn get_draft() -> Result<Json<ApiResp<Value>>> {
    require_maintainer().await?;
    let tenant = current_tenant();
    let s = store();
    let row = match s.get_draft_row().await.map_err(draft_store_err("读草稿失败"))? {
        Some(r) => r,
        None => {
            let (row, _) = ensure_draft_forked(&tenant).await?;
            row
        }
    };
    Ok(Json(ApiResp::ok(draft_payload(&row))))
}

/// 惰性 fork：读行；无行 → live 快照落行（ON CONFLICT DO NOTHING 幂等）→ 重读。
/// 返回（草稿行, live 快照）——fork 路径复用快照免二次组装。
pub(crate) async fn ensure_draft_forked(tenant: &str) -> Result<(DraftRow, Value)> {
    let s = store();
    let live = s
        .snapshot_full(tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("组装 live 快照失败: {e}")))?;
    if let Some(row) = s.get_draft_row().await.map_err(draft_store_err("读草稿失败"))? {
        return Ok((row, live));
    }
    let content: DraftContent = serde_json::from_value(live.clone()).map_err(|e| {
        OntoError::internal_error(format!("live 快照转草稿内容失败（拒绝静默空草稿，禁止 fork）: {e}"))
    })?;
    s.insert_draft_row(&content, &snapshot_fingerprint(&live), current_display_user())
        .await
        .map_err(draft_store_err("fork 落行失败"))?;
    let row = s
        .get_draft_row()
        .await
        .map_err(draft_store_err("读草稿失败"))?
        .ok_or_else(|| OntoError::internal_error("fork 落行后读回失败"))?;
    Ok((row, live))
}

/// 草稿行 → 响应 Value。dirty/liveRev 从 base_rev 推出（base_rev = fork 时刻 live 指纹，
/// 即「草稿相对基线有无未发布改动」的权威口径；live 被他人推进由发布门实时比对兜底）。
fn draft_payload(row: &DraftRow) -> Value {
    let draft_rev = snapshot_fingerprint(&row.content.to_snapshot_value());
    json!({
        "version": row.version,
        "baseRev": row.base_rev,
        "updatedBy": row.updated_by,
        "updatedAt": row.updated_at,
        "liveRev": row.base_rev,
        "dirty": draft_rev != row.base_rev,
        "content": serde_json::to_value(&row.content).unwrap_or(Value::Null),
    })
}

/// POST /draft/save 请求体。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftSaveReq {
    pub content: DraftContent,
    /// 行级乐观锁基线（GET /draft 的 version）。
    pub base_version: u32,
}

/// POST /draft/save —— 整体保存草稿（编辑流：保存 = 整包写草稿，不再逐元素直写 om_*）。
pub async fn save_draft(Json(req): Json<DraftSaveReq>) -> Result<Json<ApiResp<Value>>> {
    require_maintainer().await?;
    req.content
        .validate_shape()
        .map_err(|e| OntoError::bad_request(format!("草稿形状非法: {e}")))?;
    let tenant = current_tenant();
    // base_rev 不随编辑保存推进（仅 fork/restore/发布重置改写——store 层 COALESCE(None) 保全）。
    let version = store()
        .save_draft_row(&req.content, None, req.base_version, current_display_user())
        .await
        .map_err(draft_store_err("保存草稿失败"))?;
    crate::events::emit(&tenant, "draft-changed", json!({ "by": current_display_user() }));
    Ok(Json(ApiResp::ok(json!({ "saved": true, "version": version }))))
}

/// POST /draft/discard 请求体。baseVersion = 草稿行乐观锁（可选；传了则防误丢他人编辑）。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DiscardReq {
    pub base_version: Option<u32>,
}

/// POST /draft/discard —— 丢弃当前草稿（未发布变更全部放弃；下次 GET /draft 惰性 fork 重建）。
/// 用途：发布中心「基线过期」时的 rebase 第一动作（live 被旧设计器直改，本地草稿无法干净重放）。
pub async fn discard_draft(Json(req): Json<DiscardReq>) -> Result<Json<ApiResp<Value>>> {
    require_maintainer().await?;
    let tenant = current_tenant();
    let discarded = store()
        .discard_draft_row(req.base_version)
        .await
        .map_err(draft_store_err("丢弃草稿失败"))?;
    if discarded {
        crate::events::emit(&tenant, "draft-changed", json!({ "by": current_display_user(), "reason": "discard" }));
    }
    Ok(Json(ApiResp::ok(json!({ "discarded": discarded }))))
}

/// StoreError → HTTP 错误映射（草稿链路：Conflict→409 / NotFound→404 / 其余→500）。
pub(crate) fn draft_store_err(ctx: &'static str) -> impl Fn(cmx_onto_model::StoreError) -> OntoError {
    move |e| match e {
        cmx_onto_model::StoreError::Conflict(m) => OntoError::conflict(m),
        cmx_onto_model::StoreError::NotFound(m) => OntoError::not_found(m),
        other => OntoError::internal_error(format!("{ctx}: {other}")),
    }
}

// ───────────────────────────── 发布 ─────────────────────────────

/// POST /releases/preview —— 发布预览 dry-run（不应用、不落版本）：
/// 校验清单 + 元素级 diff（live → 草稿，含视图维度，基线 = live om_view）+ 基线/去重预判。
pub async fn releases_preview() -> Result<Json<ApiResp<Value>>> {
    require_maintainer().await?;
    let tenant = current_tenant();
    let s = store();
    let Some(row) = s.get_draft_row().await.map_err(draft_store_err("读草稿失败"))? else {
        return Ok(Json(ApiResp::ok(json!({
            "hasDraft": false, "diff": [], "issues": [], "counts": {},
        }))));
    };
    let live = s
        .snapshot_full(&tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("组装 live 快照失败: {e}")))?;
    let draft_snap = row.content.to_snapshot_value();
    let diff = diff_snapshots(&live, &draft_snap);
    let issues = validate_draft(&row.content, &live);
    let counts = count_diff(&diff);
    let latest = s.latest_rev().await.map_err(draft_store_err("读版本失败"))?;
    let draft_rev = snapshot_fingerprint(&draft_snap);
    // live 六类元素总数（前端大规模删除护栏阈值同口径：max(50, liveTotal/5)）。
    let live_total: usize = cmx_onto_model::ELEMENT_KINDS
        .iter()
        .filter_map(|k| cmx_onto_model::kind_key(k))
        .map(|key| live.get(key).and_then(|v| v.as_array()).map_or(0, |a| a.len()))
        .sum();
    Ok(Json(ApiResp::ok(json!({
        "hasDraft": true,
        "baseRev": row.base_rev,
        "liveRev": snapshot_fingerprint(&live),
        "baseRevOk": snapshot_fingerprint(&live) == row.base_rev,
        "draftRev": draft_rev,
        "willCreateVersion": latest.as_ref().is_none_or(|(_, r)| r != &draft_rev),
        "latestVersion": latest.map(|(v, _)| v),
        "liveTotal": live_total,
        "massDelete": counts.get("removed").and_then(|v| v.as_u64()).unwrap_or(0)
            > std::cmp::max(50, live_total / 5) as u64,
        "updatedBy": row.updated_by,
        "updatedAt": row.updated_at,
        "diff": diff,
        "issues": issues,
        "counts": counts,
    }))))
}

fn count_diff(diff: &[cmx_onto_model::DiffItem]) -> Value {
    let mut added = 0usize;
    let mut modified = 0usize;
    let mut removed = 0usize;
    for i in diff {
        match i.action {
            cmx_onto_model::DiffAction::Added => added += 1,
            cmx_onto_model::DiffAction::Modified => modified += 1,
            cmx_onto_model::DiffAction::Removed => removed += 1,
        }
    }
    json!({ "added": added, "modified": modified, "removed": removed })
}

/// POST /releases/publish 请求体。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PublishReq {
    pub summary: String,
    /// 大规模删除护栏（H1）：派生删除集（live − 草稿）超过阈值时必须显式 true——
    /// 防草稿意外清空（前端整包空保存/结构漂移）被无声发布、把 live 全量删除。
    pub confirm_mass_delete: bool,
}

/// POST /releases/publish —— 发布门：base_rev 防覆盖（live 被直改 → 409 要求 rebase）→
/// 发布校验（Error 级阻断）→ 原子应用 + rev 去重打版本 + 草稿重置 → SSE 广播。
/// **发布 = 版本诞生点**；旧 `POST /publish` 原语义保留给旧设计器（空发布仍涨版本属既有债务）。
pub async fn releases_publish(Json(req): Json<PublishReq>) -> Result<Json<ApiResp<Value>>> {
    require_maintainer().await?;
    let tenant = current_tenant();
    let s = store();
    let row = s
        .get_draft_row()
        .await
        .map_err(draft_store_err("读草稿失败"))?
        .ok_or_else(|| OntoError::business_error("当前没有草稿（无可发布变更）"))?;
    // 基线防覆盖：比对当前 live 快照指纹（非版本表 rev——live 被旧设计器直改时两者不等）。
    let live = s
        .snapshot_full(&tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("组装 live 快照失败: {e}")))?;
    let live_rev = snapshot_fingerprint(&live);
    if live_rev != row.base_rev {
        return Err(OntoError::conflict(format!(
            "草稿基线已过期（base_rev 不一致）：live 已被直改或他人已发布。请先「刷新草稿比对」\
             （rebase）确认两边变更后再发布（草稿指纹 {}/live 指纹 {live_rev}）",
            row.base_rev
        )));
    }
    // 发布校验：Error 级阻断（Warning 放行——前端预览已展示）。
    let issues = validate_draft(&row.content, &live);
    let errors: Vec<&cmx_onto_model::ValidationIssue> = issues
        .iter()
        .filter(|i| i.severity == IssueSeverity::Error)
        .collect();
    if !errors.is_empty() {
        let heads: Vec<String> = errors.iter().take(5).map(|i| i.message.clone()).collect();
        return Err(OntoError::business_error(format!(
            "发布校验未通过（{} 项阻断）：{}",
            errors.len(),
            heads.join("；")
        )));
    }
    // 大规模删除护栏（H1）：派生删除集 = live − 草稿，与发布应用事务内同一口径。
    // 草稿意外变空时此集 = live 全量——超阈值必须显式确认，不给「无声清库」留通路。
    let draft_snap = row.content.to_snapshot_value();
    let deletions = cmx_onto_model::derive_deletions(&live, &draft_snap);
    let live_total: usize = cmx_onto_model::ELEMENT_KINDS
        .iter()
        .filter_map(|k| cmx_onto_model::kind_key(k))
        .map(|key| live.get(key).and_then(|v| v.as_array()).map_or(0, |a| a.len()))
        .sum();
    let threshold = std::cmp::max(50, live_total / 5);
    if deletions.len() > threshold && !req.confirm_mass_delete {
        return Err(OntoError::conflict(format!(
            "本次发布将删除 {} 个元素（live 共 {live_total}，超过护栏阈值 {threshold}）。\
             确属批量删除请在发布请求带 confirmMassDelete=true；\
             若草稿是被意外清空的，请先「丢弃草稿」重新 fork 再核对变更",
            deletions.len()
        )));
    }
    let summary = if req.summary.trim().is_empty() { "（无摘要）" } else { req.summary.trim() };
    let outcome = s
        .publish_draft(&tenant, &row.content, summary, current_display_user())
        .await
        .map_err(draft_store_err("发布失败"))?;
    // O7 实时：发布广播（订阅者——含消费端场景页签——经 /events SSE 感知刷新）。
    crate::events::emit(
        &tenant,
        "published",
        json!({
            "version": outcome.version,
            "rev": outcome.rev,
            "deduped": outcome.deduped,
            "summary": summary,
            "publishedBy": current_display_user(),
        }),
    );
    Ok(Json(ApiResp::ok(serde_json::to_value(&outcome).unwrap_or(Value::Null))))
}

// ───────────────────────────── 版本对比 / 回滚 ─────────────────────────────

/// GET /versions/diff 查询参数（a/b = 版本号或 "live"；只读仅参 GET，禁可变路径段）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionsDiffQuery {
    pub a: String,
    pub b: String,
}

/// GET /versions/diff?a=&b= —— 服务端元素级 diff（含视图维度）。
pub async fn versions_diff(Query(q): Query<VersionsDiffQuery>) -> Result<Json<ApiResp<Value>>> {
    let tenant = current_tenant();
    let va = resolve_snapshot(&tenant, &q.a).await?;
    let vb = resolve_snapshot(&tenant, &q.b).await?;
    let diff = diff_snapshots(&va, &vb);
    let counts = count_diff(&diff);
    Ok(Json(ApiResp::ok(json!({
        "a": q.a, "b": q.b, "diff": diff, "counts": counts,
    }))))
}

/// 解析 diff 侧：版本号 → om_version 快照；"live" → 现算全量快照。
async fn resolve_snapshot(tenant: &str, side: &str) -> Result<Value> {
    if side.eq_ignore_ascii_case("live") {
        return store()
            .snapshot_full(tenant)
            .await
            .map_err(|e| OntoError::internal_error(format!("组装 live 快照失败: {e}")));
    }
    let v: u32 = side
        .parse()
        .map_err(|_| OntoError::bad_request(format!("diff 侧须为版本号或 live，收到 {side:?}")))?;
    store()
        .get_version(v)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载版本失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("版本 {v} 不存在")))
}

/// POST /versions/restore 请求体。baseVersion = 草稿行乐观锁（可选——传了则防覆盖他人未发布编辑）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReq {
    pub version: u32,
    pub base_version: Option<u32>,
}

/// POST /versions/restore —— 回滚 = 写**草稿**（回滚也过校验+发布门，无绕过评审的后门）：
/// ①内容 = 快照七类元素；②deletions = live − 快照（**减法非并集**，live 多出者进删除清单）；
/// ③views 段：快照有 views → 用快照；无（P2 前旧版本）→ **保留当前 live 场景不动**（确认弹层
/// 前端明示）。base_rev = 当前 live 指纹（发布时重比）。
pub async fn versions_restore(Json(req): Json<RestoreReq>) -> Result<Json<ApiResp<Value>>> {
    require_maintainer().await?;
    let tenant = current_tenant();
    let s = store();
    let snap = s
        .get_version(req.version)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载版本失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("版本 {} 不存在", req.version)))?;
    let live = s
        .snapshot_full(&tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("组装 live 快照失败: {e}")))?;

    // fail-loud：快照反序列化失败必须报错（静默 Default 会让回滚草稿变空，发布时清空 live）。
    let mut content: DraftContent = serde_json::from_value(snap.clone()).map_err(|e| {
        OntoError::internal_error(format!("版本 {} 快照反序列化失败: {e}", req.version))
    })?;
    content.deletions = derive_deletions_payload(&live, &snap);
    // 快照有 views 段（本轮次起的新版本）→ 随滚；旧版本无段 → 场景保持现状。
    let has_views = snap.get("views").and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty());
    if has_views {
        content.views = serde_json::from_value(snap.get("views").cloned().unwrap_or(json!([])))
            .map_err(|e| OntoError::internal_error(format!("版本快照 views 段反序列化失败: {e}")))?;
    } else {
        content.views = s
            .list_view_defs(&tenant)
            .await
            .map_err(|e| OntoError::internal_error(format!("装载场景清单失败: {e}")))?;
    }
    let draft_version = s
        .restore_draft_row(
            &content,
            &snapshot_fingerprint(&live),
            req.base_version,
            current_display_user(),
        )
        .await
        .map_err(draft_store_err("回滚写草稿失败"))?;
    crate::events::emit(
        &tenant,
        "draft-changed",
        json!({ "by": current_display_user(), "reason": format!("restore-v{}", req.version) }),
    );
    Ok(Json(ApiResp::ok(json!({
        "restoredFrom": req.version,
        "draftVersion": draft_version,
        "deletions": content.deletions.len(),
        "viewsFromSnapshot": has_views,
    }))))
}

/// live − 快照 的减法删除清单（六类元素；复用模型层纯函数口径）。
fn derive_deletions_payload(live: &Value, snap: &Value) -> Vec<DeletionRef> {
    let live_names = |key: &str| -> std::collections::BTreeSet<String> {
        live.get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| e.get("apiName").and_then(|n| n.as_str()).map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    };
    let snap_names = |key: &str| -> std::collections::BTreeSet<String> {
        snap.get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| e.get("apiName").and_then(|n| n.as_str()).map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    };
    let pairs = [
        (KIND_OBJECT, "objectTypes"),
        (KIND_LINK, "linkTypes"),
        (KIND_INTERFACE, "interfaces"),
        (KIND_SHARED, "sharedProperties"),
        (KIND_ACTION, "actionTypes"),
        (KIND_FUNCTION, "functions"),
    ];
    let mut out = Vec::new();
    for (kind, key) in pairs {
        for name in live_names(key) {
            if !snap_names(key).contains(&name) {
                out.push(DeletionRef { kind: kind.to_string(), api_name: name });
            }
        }
    }
    out
}
