use thiserror::Error;

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("memory not found: {0}")]
    MemoryNotFound(String),

    #[error("project not found: {0}")]
    ProjectNotFound(String),

    #[error("invalid id: {0}")]
    InvalidId(String),

    #[error("invalid memory type: {0}")]
    InvalidMemoryType(String),

    #[error("invalid memory status: {0}")]
    InvalidMemoryStatus(String),

    #[error("invalid representation depth: {0}")]
    InvalidDepth(String),

    #[error("validation: {0}")]
    Validation(String),

    #[error("storage: {0}")]
    Storage(String),
}
