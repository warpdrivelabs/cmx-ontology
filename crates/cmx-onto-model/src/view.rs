//! 场景视图（om_view）——本体工作室 P1（方案 §2.1 / §七）。
//!
//! 场景是过滤器/透镜，不是容器：全局底座 om_* 唯一定义，om_view 只存成员引用 + 布局。
//! `source` 两态：
//! - `auto`：读时按 DAM 域惰性派生（不物化 members——落行的 auto 视图仅承载布局/元数据，
//!   成员随域现状现算；域消失 → 虚拟条目自然消失）；
//! - `manual`：成员物化（`members`），快照语义，不随 DAM 漂移。
//!
//! JSON 一律 camelCase，与 def.rs 同纪律。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::def::DamRef;

/// 视图来源（camelCase 序列化）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ViewSource {
    /// 域默认视图：成员读时按 DAM 现算，不物化。
    #[default]
    Auto,
    /// 手动场景：成员物化（快照语义）。
    Manual,
}

/// 视图成员引用（仅 manual 物化；auto 恒空）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ViewMembers {
    #[serde(default)]
    pub objects: Vec<String>,
    #[serde(default)]
    pub interfaces: Vec<String>,
}

/// 场景视图定义（om_view 行）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SceneViewDef {
    /// 稳定 API 名。auto 视图固定 `auto:<domain>` 前缀形态；manual 由维护者命名。
    pub api_name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    /// DAM 归属（视图自身的展示分组，可空）。
    #[serde(default)]
    pub dam: DamRef,
    #[serde(default)]
    pub members: ViewMembers,
    #[serde(default)]
    pub source: ViewSource,
    /// 画布布局（组件 `_layout` 形状：`{ "<nodeId>": {x, y} }`）；LWW 直写列，不占乐观锁。
    #[serde(default)]
    pub layout: Value,
    /// 乐观锁（B0 同款：0 = 新建/盲写，>0 = 条件更新）。
    #[serde(default)]
    pub version: u32,
}

impl SceneViewDef {
    /// 结构校验（apiName 形态 + auto 前缀纪律 + auto 不物化成员）。
    pub fn validate(&self) -> crate::Result<()> {
        if self.api_name.is_empty() || self.api_name.len() > 128 {
            return Err(crate::Error::Definition(
                "视图 apiName 不能为空且 ≤128 字符".into(),
            ));
        }
        match self.source {
            ViewSource::Auto => {
                if !self.api_name.starts_with("auto:") {
                    return Err(crate::Error::Definition(
                        "auto 视图 apiName 须以「auto:」开头（域默认视图命名纪律）".into(),
                    ));
                }
                if !self.members.objects.is_empty() || !self.members.interfaces.is_empty() {
                    return Err(crate::Error::Definition(
                        "auto 视图不物化成员（成员读时按 DAM 现算）".into(),
                    ));
                }
            }
            ViewSource::Manual => {
                // 允许 auto: 前缀：域默认视图「转为手动场景」保留原名（固化语义）；
                // 该名占用后 list_views 不再为该域派生虚拟条目。fresh manual 建议不用前缀。
            }
        }
        Ok(())
    }
}

/// 视图清单项（列表用，不含 layout）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneViewMeta {
    pub api_name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub dam: DamRef,
    pub source: ViewSource,
    /// 对象成员数（manual = 物化数；auto = 读时现算数）。
    pub object_count: u32,
    /// 接口成员数（manual = 物化数；auto 恒 0——接口不随域播种）。
    pub interface_count: u32,
    /// 是否读时派生的虚拟条目（om_view 无行，仅 manifest 现算；不可直接删除/重命名）。
    #[serde(rename = "virtual")]
    pub virtual_view: bool,
    pub version: u32,
    /// 成员引用（仅 manual 有值；auto 恒空——前端成员编辑以此取全集，避免把既有成员误当空集覆盖）。
    #[serde(default)]
    pub members: ViewMembers,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}
