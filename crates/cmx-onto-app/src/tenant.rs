//! 请求级租户上下文——复用 `cmx-engine-kit::tenant`（唯一真源）。
//!
//! 本模块为 re-export shim：handlers / auth 用 `crate::tenant::*`（current_tenant /
//! current_user / current_display_user / identity_snapshot 等）。

pub use cmx_engine_kit::tenant::*;
