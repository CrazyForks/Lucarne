//! Error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LucarneError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("launcher: {0}")]
    Launcher(String),
    #[error("runtime: {0}")]
    Runtime(String),
    #[error("adapter: {0}")]
    Adapter(String),
    #[error("dialect: {0}")]
    Dialect(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("timeout")]
    Timeout,
    #[error("closed")]
    Closed,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, LucarneError>;

impl LucarneError {
    pub fn runtime(msg: impl Into<String>) -> Self {
        Self::Runtime(msg.into())
    }
    pub fn launcher(msg: impl Into<String>) -> Self {
        Self::Launcher(msg.into())
    }
    pub fn adapter(msg: impl Into<String>) -> Self {
        Self::Adapter(msg.into())
    }
    pub fn dialect(msg: impl Into<String>) -> Self {
        Self::Dialect(msg.into())
    }
    pub fn protocol(msg: impl Into<String>) -> Self {
        Self::Protocol(msg.into())
    }
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}
