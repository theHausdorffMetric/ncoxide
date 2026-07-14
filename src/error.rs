#[derive(thiserror::Error, Debug)]
pub enum NcError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("File operation failed: {0}")]
    FileOperation(String),
}

pub type Result<T> = std::result::Result<T, NcError>;
