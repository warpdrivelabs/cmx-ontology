//! 平台中立的响应信封 + 错误类型——复用 `cmx-engine-kit::resp`（经 cmx-api-types，唯一真源）。

pub use cmx_engine_kit::resp::{ApiResp, Result};

/// 过渡期别名：handlers 用 `OntoError::xxx` 构造错误（构造器名对齐 api-types）。
pub use cmx_engine_kit::resp::Error as OntoError;
