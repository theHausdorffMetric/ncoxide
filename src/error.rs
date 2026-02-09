use std::path::PathBuf;

#[derive(thiserror::Error, Debug)]
pub enum NcError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Operation cancelled")]
    Cancelled,

    #[error("File operation failed: {0}")]
    FileOperation(String),

    #[error("Permission denied: {}", .0.display())]
    PermissionDenied(PathBuf),

    #[error("Path not found: {}", .0.display())]
    NotFound(PathBuf),
}

pub type Result<T> = std::result::Result<T, NcError>;
