//! 内核错误类型（建模校验 / 存储）。

/// 本体内核错误。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 类型定义结构非法（apiName 非法、主键缺失、属性重复等）。
    #[error("本体定义错误: {0}")]
    Definition(String),
}

/// 内核结果别名。
pub type Result<T> = std::result::Result<T, Error>;

/// 存储契约错误（OntologyStore 实现产生）。
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// 后端错误（DB / 序列化）。
    #[error("存储后端错误: {0}")]
    Backend(String),
    /// 目标不存在。
    #[error("未找到: {0}")]
    NotFound(String),
    /// 乐观锁冲突（B0：版本条件更新 0 行）——调用方应以 409 透出，提示刷新后重试。
    #[error("并发冲突: {0}")]
    Conflict(String),
}

/// 存储结果别名。
pub type StoreResult<T> = std::result::Result<T, StoreError>;
