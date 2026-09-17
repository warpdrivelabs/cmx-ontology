//! 生命周期状态流转（方案 20260917 §五 —— Palantir 式软治理）。
//!
//! `POST /lifecycle/transition`：**状态变更的唯一入口**（七类资源 save 端点一律剥离 status）。
//! - 兼容矩阵判定（§5.4，`allowed_link_status`）+ 机械级联（级联目标 = 矩阵判定结果，
//!   系统不产生违规态）；
//! - → deprecated 必填弃用元数据（reason / sunsetAt）；离开 deprecated 四列清空（store 层）；
//! - `dryRun: true` 返回级联影响预览（含滞留清单），不落库；
//! - 权限：`require_maintainer`（治理动作，与存档/回滚同级）；
//! - 单事务落库（`apply_transition`）+ 每个变更资源一条修订 + SSE `resource-changed`。

use crate::engine::store;
use crate::resp::{ApiResp, OntoError, Result};
use crate::tenant::{current_display_user, current_tenant};
use axum::Json;
use cmx_onto_model::{allowed_link_status, DeprecationMeta, OntologyStore, TypeStatus};
use serde::Deserialize;
use serde_json::{json, Value};

use cmx_onto_store_pg::revision_store::CascadeWrite;

/// transition 请求体（偏序容忍）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionReq {
    pub kind: String,
    pub api_name: String,
    pub target: TypeStatus,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub deprecation: Option<DeprecationReq>,
    #[serde(default)]
    pub change_note: Option<String>,
}

/// 弃用元数据入参（→ deprecated 时 reason / sunsetAt 必填）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeprecationReq {
    pub reason: Option<String>,
    pub sunset_at: Option<String>,
    pub replacement_api_name: Option<String>,
}

/// 级联影响项（dryRun 预览 / 落库响应共用）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CascadeItem {
    pub kind: String,
    pub api_name: String,
    pub from: String,
    pub to: String,
    /// 机械级联依据（矩阵判定说明）。
    pub reason: String,
}

/// 流转结果。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionOutcome {
    pub kind: String,
    pub api_name: String,
    pub from: String,
    pub to: String,
    pub dry_run: bool,
    /// 级联变更清单（含"保持不变的对齐项"不列，只列实际变更）。
    pub cascade: Vec<CascadeItem>,
    /// 警告清单（如对象 → active 后两端关系滞留 deprecated 的滞留提示）。
    pub warnings: Vec<String>,
}

/// POST /lifecycle/transition —— 七类资源状态流转。
pub async fn transition(Json(req): Json<TransitionReq>) -> Result<Json<ApiResp<Value>>> {
    crate::archive_handlers::require_maintainer().await?;
    let tenant = current_tenant();
    let kind = req.kind.trim();
    if !cmx_onto_store_pg::revision_store::REVISION_KINDS.contains(&kind) {
        return Err(OntoError::bad_request(format!(
            "kind 非法：{kind:?}（可用：object/link/interface/shared_property/action/function/view）"
        )));
    }
    let s = store();
    let from = s
        .get_status(kind, &req.api_name)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载资源状态失败: {e}")))?
        .ok_or_else(|| OntoError::not_found(format!("{kind} {} 不存在", req.api_name)))?;

    // → deprecated：弃用元数据必填（reason / sunsetAt；replacementApiName 可选）。
    let dep: Option<DeprecationMeta> = if req.target == TypeStatus::Deprecated {
        let d = req.deprecation.clone().unwrap_or_default();
        let reason = d.reason.unwrap_or_default().trim().to_string();
        let sunset = d.sunset_at.unwrap_or_default().trim().to_string();
        if reason.is_empty() || sunset.is_empty() {
            return Err(OntoError::bad_request(
                "转 deprecated 必填弃用元数据：deprecation.reason 与 deprecation.sunsetAt（YYYY-MM-DD）",
            ));
        }
        if chrono::NaiveDate::parse_from_str(&sunset, "%Y-%m-%d").is_err() {
            return Err(OntoError::bad_request(format!(
                "deprecation.sunsetAt 格式非法：{sunset}（须 YYYY-MM-DD）"
            )));
        }
        Some(DeprecationMeta {
            reason,
            sunset_at: Some(sunset),
            replacement_api_name: d
                .replacement_api_name
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty()),
            deprecated_at: Some(chrono::Utc::now()),
        })
    } else {
        None
    };

    // 兼容矩阵：关系类型自身流转前必须过矩阵（矩阵校验作用点 2，方案 §5.4）。
    if kind == "link" {
        let lt = s
            .get_link_type(&tenant, &req.api_name)
            .await
            .map_err(|e| OntoError::internal_error(format!("装载关系类型失败: {e}")))?
            .ok_or_else(|| OntoError::not_found(format!("关系类型 {} 不存在", req.api_name)))?;
        let sa = s
            .get_status("object", &lt.object_type_a)
            .await
            .map_err(|e| OntoError::internal_error(format!("装载对象状态失败: {e}")))?
            .unwrap_or_default();
        let sb = s
            .get_status("object", &lt.object_type_b)
            .await
            .map_err(|e| OntoError::internal_error(format!("装载对象状态失败: {e}")))?
            .unwrap_or_default();
        let allowed = allowed_link_status(sa, sb);
        if !allowed.contains(&req.target) {
            return Err(OntoError::conflict(format!(
                "兼容矩阵不允许该流转：关系两端对象状态为 {sa:?}/{sb:?}，关系仅允许 {:?}（当前 {from:?}）",
                allowed
            )));
        }
    }

    // 机械级联（对象类型流转联动关系；级联目标 = 矩阵判定结果，Palantir 语义 §5.4）。
    let mut cascade: Vec<CascadeItem> = Vec::new();
    let mut writes: Vec<CascadeWrite> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    if kind == "object" {
        let all_links = s
            .list_link_types(&tenant)
            .await
            .map_err(|e| OntoError::internal_error(format!("装载关系清单失败: {e}")))?;
        let related: Vec<_> = all_links
            .iter()
            .filter(|l| l.object_type_a == req.api_name || l.object_type_b == req.api_name)
            .collect();
        for lt in related {
            let other = if lt.object_type_a == req.api_name {
                &lt.object_type_b
            } else {
                &lt.object_type_a
            };
            let other_status = s
                .get_status("object", other)
                .await
                .map_err(|e| OntoError::internal_error(format!("装载对象状态失败: {e}")))?
                .unwrap_or_default();
            let allowed = allowed_link_status(req.target, other_status);
            let note = format!(
                "级联：对象 {} → {:?}，对端 {}（{:?}）→ 矩阵仅允许 {:?}",
                req.api_name, req.target, other, other_status, allowed
            );
            if allowed.len() == 1 && allowed[0] != lt.status {
                // 唯一允许值 ≠ 当前 → 机械对齐（含"覆盖单独废弃态"的强制 experimental）。
                let link_target = allowed[0];
                let link_dep = if link_target == TypeStatus::Deprecated {
                    Some(cascade_deprecation(&req.api_name, req.target, &dep))
                } else {
                    // 离开 deprecated（如 deprecated → experimental 强制对齐）→ 四列清空。
                    None
                };
                cascade.push(CascadeItem {
                    kind: "link".into(),
                    api_name: lt.api_name.clone(),
                    from: lt.status.as_str().into(),
                    to: link_target.as_str().into(),
                    reason: note,
                });
                let payload = link_payload_with_status(&tenant, lt, link_target, &link_dep).await?;
                writes.push(CascadeWrite {
                    api_name: lt.api_name.clone(),
                    target: link_target,
                    deprecation: link_dep,
                    payload,
                });
            } else if allowed.len() == 3 && lt.status == TypeStatus::Deprecated {
                // 两端 active 后关系滞留 deprecated：合法态不自动升级 → 滞留清单（dryRun 可见）。
                warnings.push(format!(
                    "关系 {} 滞留 deprecated（两端对象均 active 后可经 transition(kind=link) 显式激活）",
                    lt.api_name
                ));
            }
        }
        // 对象 → deprecated 的级联降级元数据继承 sunset_at（§5.4）。
        let _ = &dep;
    }

    if req.dry_run {
        return Ok(Json(ApiResp::ok(serde_json::to_value(TransitionOutcome {
            kind: kind.into(),
            api_name: req.api_name.clone(),
            from: from.as_str().into(),
            to: req.target.as_str().into(),
            dry_run: true,
            cascade,
            warnings,
        })
        .unwrap_or(Value::Null))));
    }

    // 主资源修订 payload：当前定义 + 新 status + 新弃用元数据（view 剥离 layout）。
    let main_payload = def_payload_with_status(&tenant, kind, &req.api_name, req.target, &dep).await?;
    store()
        .apply_transition(
            kind,
            &req.api_name,
            &main_payload,
            req.target,
            dep.as_ref(),
            &writes,
            &current_display_user().unwrap_or_else(|| "anonymous".into()),
            req.change_note.as_deref().or(Some("lifecycle transition")),
        )
        .await
        .map_err(|e| OntoError::internal_error(format!("状态流转失败: {e}")))?;

    crate::events::emit(
        &tenant,
        "resource-changed",
        json!({
            "kind": kind,
            "apiName": req.api_name,
            "action": "transition",
            "from": from.as_str(),
            "to": req.target.as_str(),
            "by": current_display_user(),
        }),
    );

    Ok(Json(ApiResp::ok(serde_json::to_value(TransitionOutcome {
        kind: kind.into(),
        api_name: req.api_name.clone(),
        from: from.as_str().into(),
        to: req.target.as_str().into(),
        dry_run: false,
        cascade,
        warnings,
    })
    .unwrap_or(Value::Null))))
}

/// 级联降级的弃用元数据自动填充（§5.4：reason 固定话术、sunset_at 继承触发对象、deprecated_at=now）。
fn cascade_deprecation(obj: &str, target: TypeStatus, dep: &Option<DeprecationMeta>) -> DeprecationMeta {
    DeprecationMeta {
        reason: format!("级联降级：对象类型 {obj} 转为 {}", target.as_str()),
        sunset_at: dep.as_ref().and_then(|d| d.sunset_at.clone()),
        replacement_api_name: None,
        deprecated_at: Some(chrono::Utc::now()),
    }
}

/// 装载七类资源当前定义并替换 status / 弃用元数据（修订 payload；camelCase serde 形状）。
/// 前置：transition 入口已验证资源存在（from 状态已装载）。
async fn def_payload_with_status(
    tenant: &str,
    kind: &str,
    api_name: &str,
    target: TypeStatus,
    dep: &Option<DeprecationMeta>,
) -> Result<Value> {
    use cmx_onto_model::OntologyStore;
    let s = store();
    let mut v: Value = match kind {
        "object" => serde_json::to_value(
            s.get_object_type(tenant, api_name)
                .await
                .map_err(internal("装载对象类型失败"))?
                .ok_or_else(|| OntoError::not_found(format!("object {api_name} 不存在")))?,
        )
        .unwrap_or(Value::Null),
        "link" => serde_json::to_value(
            s.get_link_type(tenant, api_name)
                .await
                .map_err(internal("装载关系类型失败"))?
                .ok_or_else(|| OntoError::not_found(format!("link {api_name} 不存在")))?,
        )
        .unwrap_or(Value::Null),
        "interface" => serde_json::to_value(
            s.get_interface(tenant, api_name)
                .await
                .map_err(internal("装载接口失败"))?
                .ok_or_else(|| OntoError::not_found(format!("interface {api_name} 不存在")))?,
        )
        .unwrap_or(Value::Null),
        "shared_property" => serde_json::to_value(
            s.get_shared_property(tenant, api_name)
                .await
                .map_err(internal("装载共享属性失败"))?
                .ok_or_else(|| OntoError::not_found(format!("shared_property {api_name} 不存在")))?,
        )
        .unwrap_or(Value::Null),
        "action" => serde_json::to_value(
            s.get_action_type(tenant, api_name)
                .await
                .map_err(internal("装载动作类型失败"))?
                .ok_or_else(|| OntoError::not_found(format!("action {api_name} 不存在")))?,
        )
        .unwrap_or(Value::Null),
        "function" => serde_json::to_value(
            s.get_function(tenant, api_name)
                .await
                .map_err(internal("装载函数失败"))?
                .ok_or_else(|| OntoError::not_found(format!("function {api_name} 不存在")))?,
        )
        .unwrap_or(Value::Null),
        "view" => {
            let view = s
                .get_view(tenant, api_name)
                .await
                .map_err(internal("装载场景失败"))?
                .ok_or_else(|| OntoError::not_found(format!("view {api_name} 不存在")))?;
            let mut view_v = serde_json::to_value(view).unwrap_or(Value::Null);
            if let Some(o) = view_v.as_object_mut() {
                o.remove("layout");
            }
            view_v
        }
        other => return Err(OntoError::bad_request(format!("未知资源类别 {other}"))),
    };
    if let Some(o) = v.as_object_mut() {
        o.insert("status".into(), json!(target.as_str()));
        match dep {
            Some(d) => {
                o.insert("deprecation".into(), serde_json::to_value(d).unwrap_or(Value::Null));
            }
            None => {
                o.insert("deprecation".into(), Value::Null);
            }
        };
    }
    Ok(v)
}

/// 级联关系类型的修订 payload（清单行 + 新 status；清单行已含两端/backing 全量字段）。
async fn link_payload_with_status(
    _tenant: &str,
    lt: &cmx_onto_model::LinkTypeMeta,
    target: TypeStatus,
    dep: &Option<DeprecationMeta>,
) -> Result<Value> {
    let mut v = serde_json::to_value(lt).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.insert("status".into(), json!(target.as_str()));
        match dep {
            Some(d) => o.insert("deprecation".into(), serde_json::to_value(d).unwrap_or(Value::Null)),
            None => o.insert("deprecation".into(), Value::Null),
        };
    }
    Ok(v)
}

fn internal(ctx: &'static str) -> impl Fn(cmx_onto_model::StoreError) -> OntoError {
    move |e| OntoError::internal_error(format!("{ctx}: {e}"))
}
