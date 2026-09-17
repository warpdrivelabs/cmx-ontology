//! 存档/回滚 + 权限守卫 handler（直改 live 架构）。
//!
//! 端点（POST 语义对齐工作区 API 硬规范——无 PUT/可变路径段）：
//! - `POST /snapshots`：存档——当前 live 全量快照 → om_version 检查点（rev 去重 + 并发安全）。
//! - `GET  /versions/diff?a=&b=`：版本对 / live 对比（元素级 diff；只读仅参 GET）。
//! - `POST /versions/restore`：回滚 = 历史快照整体恢复回 live（校验 + 护栏 + 留痕存档）。
//! - `GET  /me/roles`：当前用户角色/权限码（页内按角色渲染 + 门户权限注入取数）。
//!
//! 授权：写端点过 [`require_maintainer`]——om_maintainer 白名单**空表 = 开放**（全员维护
//! 等效；有行 = 仅命中者可写）。六类直改端点（POST/DELETE om_*）按用户裁决不加守卫，
//! 与旧设计器行为对齐（见方案 §4.5）。

use crate::engine::store;
use crate::resp::{ApiResp, OntoError, Result};
use crate::tenant::{current_display_user, current_roles, current_tenant, current_user};
use axum::extract::Query;
use axum::Json;
use cmx_onto_model::{
    derive_deletions, diff_snapshots, element_total, validate_snapshot, IssueSeverity,
    OntologyStore,
};
use serde::Deserialize;
use serde_json::{json, Value};

// ───────────────────────────── 授权 ─────────────────────────────

/// 维护角色守卫：om_maintainer 空表 = 开放（全员维护等效；有行时按 用户 id / 展示名 / 角色
/// 三路匹配，任一命中放行，否则 403（前端统一降级隐藏写入口）。
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
            "当前账号无本体维护权限（评审门：存档/回滚/场景编辑仅维护角色可用）",
        ))
    }
}

/// 当前账号是否维护角色（不抛错——`/me/roles` 与页内渲染判定用）。
async fn is_maintainer() -> Result<bool> {
    let rows = store()
        .list_maintainers()
        .await
        .map_err(|e| OntoError::internal_error(format!("装载维护白名单失败: {e}")))?;
    if rows.is_empty() {
        return Ok(true);
    }
    let user = current_user().unwrap_or_default();
    let display = current_display_user().unwrap_or_default();
    let roles = current_roles();
    Ok(rows.iter().any(|(subject, kind)| match kind.as_str() {
        "role" => roles.iter().any(|r| r == subject),
        _ => subject == &user || subject == &display,
    }))
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

// ───────────────────────────── 存档 ─────────────────────────────

/// POST /snapshots 请求体。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SnapshotReq {
    pub summary: String,
}

/// POST /snapshots —— 存档：当前 live 全量快照 → om_version 检查点。
/// rev 与最新版本相同 → 去重不插行（响应 `deduped: true`）；并发安全（事务内撞号重试）。
pub async fn create_snapshot(Json(req): Json<SnapshotReq>) -> Result<Json<ApiResp<Value>>> {
    require_maintainer().await?;
    let tenant = current_tenant();
    let summary = if req.summary.trim().is_empty() { "（无摘要）" } else { req.summary.trim() };
    let outcome = store()
        .archive_snapshot(&tenant, summary, current_display_user())
        .await
        .map_err(|e| OntoError::internal_error(format!("存档失败: {e}")))?;
    // O7 实时：广播检查点事件（§6.5 命名清理：存档≠发布；发布走 /releases + release-created）。
    crate::events::emit(
        &tenant,
        "checkpoint-created",
        json!({
            "version": outcome.version,
            "rev": outcome.rev,
            "deduped": outcome.deduped,
            "summary": outcome.summary,
            "archivedBy": current_display_user(),
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
    let counts = json!({
        "added": diff.iter().filter(|i| matches!(i.action, cmx_onto_model::DiffAction::Added)).count(),
        "modified": diff.iter().filter(|i| matches!(i.action, cmx_onto_model::DiffAction::Modified)).count(),
        "removed": diff.iter().filter(|i| matches!(i.action, cmx_onto_model::DiffAction::Removed)).count(),
    });
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

/// POST /versions/restore 请求体。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct RestoreReq {
    pub version: u32,
    /// 大规模删除护栏：派生删除集（live − 目标）超阈值时必须显式 true。
    pub confirm_mass_delete: bool,
}

/// POST /versions/restore —— 回滚 = 历史快照整体恢复回 live（无草稿中转）：
/// 校验（结构/引用，Error 阻断）→ 大规模删除护栏 → 单事务应用（六类 upsert + views +
/// 派生删除 + 级联）→ 回滚留痕存档（「回滚到 v{n}」，与 live 全等则去重不插）→ SSE 广播。
pub async fn versions_restore(Json(req): Json<RestoreReq>) -> Result<Json<ApiResp<Value>>> {
    require_maintainer().await?;
    let tenant = current_tenant();
    let snap = store()
        .get_version(req.version)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载版本失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("版本 {} 不存在", req.version)))?;
    let live = store()
        .snapshot_full(&tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("组装 live 快照失败: {e}")))?;

    // 派生删除集 + 引用校验（目标快照为 target、live 为基线；Error 级阻断）。
    let deletions = derive_deletions(&live, &snap);
    let issues = validate_snapshot(&snap, &deletions, &live);
    let errors: Vec<&cmx_onto_model::ValidationIssue> = issues
        .iter()
        .filter(|i| i.severity == IssueSeverity::Error)
        .collect();
    if !errors.is_empty() {
        let heads: Vec<String> = errors.iter().take(5).map(|i| i.message.clone()).collect();
        return Err(OntoError::business_error(format!(
            "回滚校验未通过（{} 项阻断）：{}",
            errors.len(),
            heads.join("；")
        )));
    }
    // 护栏预检（事务内 store 层还有同口径兜底）。
    let threshold = std::cmp::max(50, element_total(&live) / 5);
    if deletions.len() > threshold && !req.confirm_mass_delete {
        return Err(OntoError::conflict(format!(
            "本次回滚将删除 {} 个元素（live 共 {}，超过护栏阈值 {threshold}）。\
             确属批量回滚请在请求带 confirmMassDelete=true；\
             若与预期不符，请先「对比 live」核对差异",
            deletions.len(),
            element_total(&live)
        )));
    }

    let outcome = store()
        .restore_snapshot_to_live(&tenant, req.version, &snap, current_display_user(), req.confirm_mass_delete)
        .await
        .map_err(|e| match e {
            cmx_onto_model::StoreError::Conflict(m) => OntoError::conflict(m),
            other => OntoError::internal_error(format!("回滚失败: {other}")),
        })?;
    // O7 实时：回滚完成广播（检查点事件 + 资源变更事件——修订管道已逐资源留痕）。
    crate::events::emit(
        &tenant,
        "checkpoint-created",
        json!({
            "version": outcome.archive_version,
            "restoredFrom": outcome.restored_from,
            "deduped": outcome.archive_deduped,
            "summary": format!("回滚到 v{}", req.version),
            "archivedBy": current_display_user(),
        }),
    );
    crate::events::emit(
        &tenant,
        "resource-changed",
        json!({
            "kind": "batch",
            "action": "restore",
            "restoredFrom": req.version,
            "by": current_display_user(),
        }),
    );
    Ok(Json(ApiResp::ok(serde_json::to_value(&outcome).unwrap_or(Value::Null))))
}

// ───────────────────────── 命名发布标记（§6.4） ─────────────────────────

/// POST /releases 请求体。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ReleaseReq {
    /// 发布标记名（如 v1.2；非空 ≤64、`^[A-Za-z0-9][A-Za-z0-9._-]*$`、不得纯数字）。
    pub tag: String,
    #[serde(default)]
    pub note: Option<String>,
    /// 发布门禁：live 含 experimental / deprecated 资源时必须显式确认（对齐 confirmMassDelete 风格）。
    #[serde(default)]
    pub acknowledge_warnings: bool,
}

/// tag 格式校验（§6.4）。
fn validate_tag(tag: &str) -> Result<()> {
    if tag.is_empty() || tag.len() > 64 {
        return Err(OntoError::bad_request("tag 非空且 ≤64 字符"));
    }
    let ok = tag.chars().next().map(|c| c.is_ascii_alphanumeric()).unwrap_or(false)
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-');
    if !ok {
        return Err(OntoError::bad_request(
            "tag 须匹配 ^[A-Za-z0-9][A-Za-z0-9._-]*$",
        ));
    }
    if tag.chars().all(|c| c.is_ascii_digit()) {
        return Err(OntoError::bad_request("tag 不得为纯数字（避免与存档版本号混淆）"));
    }
    Ok(())
}

/// POST /releases —— 发布 = 给检查点起名：跑发布门禁（experimental / deprecated 警告清单）→
/// 打全量检查点并置 tag。软中带硬：警告可 acknowledge 放行（治理动作，D2 自洽）。
pub async fn create_release(Json(req): Json<ReleaseReq>) -> Result<Json<ApiResp<Value>>> {
    require_maintainer().await?;
    let tenant = current_tenant();
    validate_tag(req.tag.trim())?;
    let tag = req.tag.trim().to_string();
    let m = store()
        .manifest(&tenant)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载清单失败: {e}")))?;
    let mut warnings: Vec<Value> = Vec::new();
    for t in &m.object_types {
        if t.status != cmx_onto_model::TypeStatus::Active {
            warnings.push(json!({ "kind": "object", "apiName": t.api_name, "status": t.status.as_str(), "deprecation": t.deprecation }));
        }
    }
    for l in &m.link_types {
        if l.status != cmx_onto_model::TypeStatus::Active {
            warnings.push(json!({ "kind": "link", "apiName": l.api_name, "status": l.status.as_str(), "deprecation": l.deprecation }));
        }
    }
    for list in [&m.interfaces, &m.shared_properties, &m.functions] {
        for t in list {
            if t.status.as_deref() == Some("experimental") || t.status.as_deref() == Some("deprecated") {
                warnings.push(json!({ "kind": "simple", "apiName": t.api_name, "status": t.status }));
            }
        }
    }
    for t in &m.action_types {
        if t.status != cmx_onto_model::TypeStatus::Active {
            warnings.push(json!({ "kind": "action", "apiName": t.api_name, "status": t.status.as_str(), "deprecation": t.deprecation }));
        }
    }
    if !warnings.is_empty() && !req.acknowledge_warnings {
        let preview = warnings
            .iter()
            .take(5)
            .filter_map(|w| w.get("apiName").and_then(|n| n.as_str()))
            .collect::<Vec<_>>()
            .join("、");
        return Err(OntoError::conflict(format!(
            "发布门禁：live 含 {} 个 experimental / deprecated 资源（如 {preview}）。\
             确认发布请带 acknowledgeWarnings=true 重发",
            warnings.len()
        )));
    }
    let outcome = store()
        .create_release(&tag, req.note.as_deref().unwrap_or(""), current_display_user())
        .await
        .map_err(|e| OntoError::internal_error(format!("发布失败: {e}")))?;
    crate::events::emit(
        &tenant,
        "release-created",
        json!({
            "tag": outcome.tag,
            "version": outcome.version,
            "rev": outcome.rev,
            "reused": outcome.reused,
            "by": current_display_user(),
        }),
    );
    Ok(Json(ApiResp::ok(json!({
        "tag": outcome.tag,
        "version": outcome.version,
        "rev": outcome.rev,
        "reused": outcome.reused,
        "warnings": warnings,
    }))))
}

/// POST /releases/remove 请求体。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseRemoveReq {
    pub tag: String,
}

/// POST /releases/remove —— 解除发布标记（只清 tag，不删检查点行）。
pub async fn remove_release(Json(req): Json<ReleaseRemoveReq>) -> Result<Json<ApiResp<Value>>> {
    require_maintainer().await?;
    let n = store()
        .remove_release(&req.tag)
        .await
        .map_err(|e| OntoError::internal_error(format!("解除发布标记失败: {e}")))?;
    if n == 0 {
        return Err(OntoError::not_found(format!("发布标记 {} 不存在", req.tag)));
    }
    Ok(Json(ApiResp::ok(json!({ "tag": req.tag, "removed": true }))))
}
