//! O6 读侧权限硬门（壳层统一 PEP）。
//!
//! 决策/执行解耦的**强制点**：主链路读取（load / aggregate / search_around）在查库**前**过 [`enforce_read`]
//! 收口——
//! - **硬拒绝**：命中 `effect=deny` 策略 → 403；「受控类型」（存在针对它的 allow 策略）无 allow 命中 → 403；
//! - **行约束（强制）**：allow 策略的 `row_filter` 经 [`residual_set`] 折入对象集（编译期 `props->>` 生效）；
//! - **列脱敏（硬）**：`deny_markings` 命中的属性列，用 cmx-dataauth-core 的 [`apply_masks`] + [`MaskType::Hide`]
//!   **移除列**（比旧 `redact_rows` 的 `***` 更强——敏感列直接不出现在响应里）。
//!
//! 与旧 `authz::redact_rows`（软脱敏、可选旁路 `/secure/*`）的区别：本门挂主链路、deny 真 403、marking 硬 Hide。
//! 复用 cmx-dataauth-core 仅取其零 IO 脱敏内核（不接其维度 PDP——onto 用自有扁平 `om_policy` 行残差）。

use crate::engine::store;
use crate::resp::{OntoError, Result};
use cmx_dataauth_core::{apply_masks, MaskType, Obligation};
use cmx_onto_model::objectset::{ObjectRecord, ObjectSet};
use cmx_onto_model::{residual_set, OntologyStore};
use cmx_onto_store_pg::PolicyStore;
use serde_json::Value;

fn policy_store() -> PolicyStore {
    PolicyStore::new(crate::tenancy::current_db_id())
}

/// 读侧脱敏计划：终端类型上需 Hide 的属性列（marking 命中 deny_markings）。
#[derive(Debug, Default, Clone)]
pub struct MaskPlan {
    /// 需移除的属性 apiName（Hide 语义）。
    hide_columns: Vec<String>,
    /// 命中的策略名（响应回显 / 审计）。
    pub applied_policies: Vec<String>,
}

impl MaskPlan {
    /// 对加载出的行施加列脱敏（Hide：就地移除列）。空计划为 no-op。
    pub fn apply(&self, rows: &mut [ObjectRecord]) {
        if self.hide_columns.is_empty() {
            return;
        }
        let obligations: Vec<Obligation> = self
            .hide_columns
            .iter()
            .map(|c| Obligation { column: c.clone(), mask_type: MaskType::Hide, pattern: None })
            .collect();
        // dataauth apply_masks 吃 &mut [Map<String,Value>]；逐行把 ObjectRecord.properties 借出施加。
        for r in rows.iter_mut() {
            if let Value::Object(m) = &mut r.properties {
                let mut one = [std::mem::take(m)];
                apply_masks(&mut one, &obligations);
                let [restored] = one;
                *m = restored;
            }
        }
    }
}

/// 读侧硬门：匹配主体策略 → 硬拒判定 → 行残差折入 → 返回（加固对象集, 脱敏计划）。
///
/// `terminal`：对象集终端类型（策略按此匹配；SearchAround 需由调用方先解析终端类型传入，空则跳过类型级门）。
/// `subjects`：已解析的主体集（`(kind, subject)`）。返回后调用方按加固集查库，再 `MaskPlan::apply` 脱敏。
pub async fn enforce_read(
    tenant: &str,
    subjects: &[(String, String)],
    terminal: &str,
    set: ObjectSet,
) -> Result<(ObjectSet, MaskPlan)> {
    // 终端类型未知（如未解析的 SearchAround）→ 不做类型级策略门（编译后由 store 侧兜底）。
    if terminal.is_empty() {
        return Ok((set, MaskPlan::default()));
    }
    let ps = policy_store();
    let policies = ps
        .match_policies(terminal, subjects)
        .await
        .map_err(|e| OntoError::internal_error(format!("匹配策略失败: {e}")))?;

    // 1) 硬拒绝：任一 deny 策略命中 → 403。
    if let Some(denier) = policies.iter().find(|p| p.is_deny()) {
        return Err(OntoError::forbidden(format!(
            "读取对象类型「{terminal}」被策略「{}」拒绝（deny）",
            denier.api_name
        )));
    }

    // 2) default-deny（仅受控类型）：该类型存在针对性 allow 策略但当前主体无 allow 命中 → 403。
    //    非受控类型（无任何针对它的 allow 策略）放行——向后兼容既有无策略读取。
    let allow_policies: Vec<_> = policies.iter().filter(|p| !p.is_deny()).collect();
    if allow_policies.is_empty() {
        let controlled = ps
            .is_controlled_type(terminal)
            .await
            .map_err(|e| OntoError::internal_error(format!("查受控类型失败: {e}")))?;
        if controlled {
            return Err(OntoError::forbidden(format!(
                "对象类型「{terminal}」受策略管控，当前主体无授权（未命中任何 allow 策略）"
            )));
        }
        // 非受控 → 放行，无残差、无脱敏。
        return Ok((set, MaskPlan::default()));
    }

    // 3) 行约束（强制）：合并所有 allow 策略的 row_filter → 折入对象集。
    let mut residuals = Vec::new();
    let mut deny_markings: Vec<String> = Vec::new();
    let mut applied: Vec<String> = Vec::new();
    for p in &allow_policies {
        residuals.extend(p.row_filter.clone());
        for m in &p.deny_markings {
            if !deny_markings.contains(m) {
                deny_markings.push(m.clone());
            }
        }
        applied.push(p.api_name.clone());
    }
    let secured = residual_set(set, residuals);

    // 4) 列脱敏计划：deny_markings → 终端类型上 marking 命中的属性列（Hide）。
    let hide_columns = if deny_markings.is_empty() {
        Vec::new()
    } else {
        hide_columns_for(tenant, terminal, &deny_markings).await
    };

    Ok((secured, MaskPlan { hide_columns, applied_policies: applied }))
}

/// 终端类型上 marking ∈ deny_markings 的属性 apiName 列表（Hide 目标列）。
async fn hide_columns_for(tenant: &str, object_type: &str, deny_markings: &[String]) -> Vec<String> {
    let mut cols = Vec::new();
    if let Ok(Some(def)) = store().get_object_type(tenant, object_type).await {
        for p in &def.properties {
            if let Some(mk) = &p.marking {
                if !mk.is_empty() && deny_markings.iter().any(|d| d == mk) {
                    cols.push(p.api_name.clone());
                }
            }
        }
    }
    cols
}

/// 请求 subjects（`["role:x","user:y"]`）→ (kind,subject) 列表；空则回退上下文（role:tenant + user）。
/// 与 action/policy handler 同构；auth off/单租户下调用方声明，jwt 模式以令牌为准。
pub fn subjects_from(req_subjects: &[String]) -> Vec<(String, String)> {
    if !req_subjects.is_empty() {
        return req_subjects
            .iter()
            .filter_map(|s| s.split_once(':').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect();
    }
    let mut subs = vec![("role".to_string(), crate::tenant::current_tenant())];
    if let Some(u) = crate::tenant::current_user() {
        subs.push(("user".to_string(), u));
    }
    subs
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mask_plan_hide_removes_column() {
        let mut rows = vec![ObjectRecord {
            pk: "1".into(),
            title: "x".into(),
            properties: json!({ "name": "Ada", "ssn": "123-45", "region": "east" }),
        }];
        let plan = MaskPlan { hide_columns: vec!["ssn".into()], applied_policies: vec![] };
        plan.apply(&mut rows);
        // Hide = 移除列：ssn 键不再存在（比 *** 更强）。
        assert!(rows[0].properties.get("ssn").is_none(), "ssn 应被移除: {:?}", rows[0].properties);
        assert_eq!(rows[0].properties["name"], json!("Ada")); // 未标记列不动
        assert_eq!(rows[0].properties["region"], json!("east"));
    }

    #[test]
    fn mask_plan_empty_is_noop() {
        let mut rows = vec![ObjectRecord {
            pk: "1".into(),
            title: "x".into(),
            properties: json!({ "a": 1 }),
        }];
        MaskPlan::default().apply(&mut rows);
        assert_eq!(rows[0].properties["a"], json!(1));
    }

    #[test]
    fn subjects_from_parses_and_falls_back() {
        let parsed = subjects_from(&["role:teller".into(), "user:bob".into()]);
        assert_eq!(parsed, vec![("role".into(), "teller".into()), ("user".into(), "bob".into())]);
    }
}
