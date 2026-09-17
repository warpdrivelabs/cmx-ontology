//! 读取侧状态分层过滤（方案 20260917 §5.5 / D9 裁决）——统一口径助手。
//!
//! 规则：查询默认仅返回 active；experimental 默认不返回（`include=experimental` 打开）；
//! deprecated 默认忽略（`include=deprecated` 可显式查看存量）；`include=all` 全量。
//! 404 语义仅限读取端点（manifest / object-sets / Search-Around / OSDK——按"不存在"处理）；
//! **写入与执行端点一律豁免**（D2 软治理）。过滤在 handler 层传参实现，不下沉 store。

use crate::engine::store;
use crate::resp::{OntoError, Result};
use cmx_onto_model::{ObjectSet, OntologyStore, TypeStatus};

/// 状态可见性过滤器（默认仅 active）。
#[derive(Debug, Clone, Copy, Default)]
pub struct StatusFilter {
    pub experimental: bool,
    pub deprecated: bool,
}

impl StatusFilter {
    /// 解析 `include` 参数：逗号分隔（experimental / deprecated）或 `all`。
    /// 非法值报 400（消费方拼写错误不该被静默吞掉）。
    pub fn parse(raw: Option<&str>) -> Result<Self> {
        let Some(raw) = raw else { return Ok(Self::default()) };
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok(Self::default());
        }
        if raw.eq_ignore_ascii_case("all") {
            return Ok(Self { experimental: true, deprecated: true });
        }
        let mut f = Self::default();
        for part in raw.split(',') {
            match part.trim().to_ascii_lowercase().as_str() {
                "experimental" => f.experimental = true,
                "deprecated" => f.deprecated = true,
                "" => {}
                other => {
                    return Err(OntoError::bad_request(format!(
                        "include 参数非法：{other:?}（可用：experimental / deprecated / all）"
                    )))
                }
            }
        }
        Ok(f)
    }

    /// 字符串状态（manifest JSON 内联过滤）是否可见。
    pub fn allow_str(&self, status: &str) -> bool {
        match status {
            "active" => true,
            "experimental" => self.experimental,
            "deprecated" => self.deprecated,
            _ => true,
        }
    }

    /// 该状态是否在可见集内。
    pub fn allows(&self, status: TypeStatus) -> bool {
        match status {
            TypeStatus::Active => true,
            TypeStatus::Experimental => self.experimental,
            TypeStatus::Deprecated => self.deprecated,
        }
    }
}

/// 校验对象类型在当前过滤口径下可查询（读取端点：非 active 且未 include → 404"不存在"；
/// 类型未注册同样 404）。**写入 / 执行端点不得调用本函数**（D2 豁免清单见方案 §5.5）。
pub async fn ensure_object_queryable(tenant: &str, object_type: &str, f: &StatusFilter) -> Result<()> {
    let Some(def) = store()
        .get_object_type(tenant, object_type)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载对象类型失败: {e}")))?
    else {
        return Err(OntoError::not_found(format!("对象类型 {object_type} 不存在")));
    };
    if !f.allows(def.status) {
        return Err(OntoError::not_found(format!(
            "对象类型 {object_type} 不存在（状态 {:?} 默认不返回，可显式 include 打开）",
            def.status
        )));
    }
    Ok(())
}

/// 校验关系类型在当前过滤口径下可查询（Search-Around 钻取用）。
pub async fn ensure_link_queryable(tenant: &str, link: &str, f: &StatusFilter) -> Result<()> {
    let Some(lt) = store()
        .get_link_type(tenant, link)
        .await
        .map_err(|e| OntoError::internal_error(format!("装载关系类型失败: {e}")))?
    else {
        return Err(OntoError::not_found(format!("关系类型 {link} 不存在")));
    };
    if !f.allows(lt.status) {
        return Err(OntoError::not_found(format!(
            "关系类型 {link} 不存在（状态 {:?} 默认不返回，可显式 include 打开）",
            lt.status
        )));
    }
    Ok(())
}

/// 场景上下文（§7.3 数据链路过滤）：解析 + 成员校验助手。
/// 场景过滤是**可见性组织**不是权限（权限仍归 PEP）；与状态过滤取交集。
pub struct SceneScope {
    pub name: String,
    pub objects: std::collections::BTreeSet<String>,
    pub links: Option<std::collections::BTreeSet<String>>,
}

impl SceneScope {
    /// 解析 view 参数（None = 非场景模式）；视图不存在 → 404。
    pub async fn resolve(tenant: &str, view: Option<&str>) -> Result<Option<Self>> {
        let Some(name) = view else { return Ok(None) };
        let s = store();
        let row = s
            .get_view(tenant, name)
            .await
            .map_err(|e| OntoError::internal_error(format!("装载场景视图失败: {e}")))?;
        let Some(def) = row else {
            return Err(OntoError::not_found(format!("场景视图 {name} 不存在")));
        };
        match def.source {
            cmx_onto_model::ViewSource::Manual => {
                let objects: std::collections::BTreeSet<String> =
                    def.members.objects.into_iter().collect();
                let links: std::collections::BTreeSet<String> =
                    def.members.links.into_iter().collect();
                Ok(Some(Self { name: name.to_string(), objects, links: Some(links) }))
            }
            // auto 视图成员读时按 DAM 现算（域对象全集；links 不设限）。
            cmx_onto_model::ViewSource::Auto => {
                let objects = crate::view_handlers::domain_objects(tenant, name.trim_start_matches("auto:")).await?;
                Ok(Some(Self {
                    name: name.to_string(),
                    objects: objects.into_iter().collect(),
                    links: None,
                }))
            }
        }
    }

    /// 对象类型 ∈ 场景 objects，否则 409（"不在场景内"——无论状态）。
    pub fn ensure_object_member(&self, object_type: &str) -> Result<()> {
        if self.objects.contains(object_type) {
            return Ok(());
        }
        Err(OntoError::conflict(format!(
            "对象类型 {object_type} 不在场景 {} 内（场景模式下仅可查询场景成员）",
            self.name
        )))
    }

    /// 关系 ∈ 场景 links（auto 视图不设限），否则 409。
    pub fn ensure_link_member(&self, link: &str) -> Result<()> {
        match &self.links {
            None => Ok(()),
            Some(set) if set.contains(link) => Ok(()),
            _ => Err(OntoError::conflict(format!(
                "关系 {link} 不在场景 {} 的关系清单内（可在场景中显式加入）",
                self.name
            ))),
        }
    }

    /// 对象集代数的终端类型场景校验（object-sets/load·aggregate 编译前调用）。
    pub fn ensure_set_allowed(&self, set: &ObjectSet) -> Result<()> {
        let terminal = set.terminal_object_type().unwrap_or("");
        if terminal.is_empty() {
            return Ok(());
        }
        self.ensure_object_member(terminal)
    }
}
