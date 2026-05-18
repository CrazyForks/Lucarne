use crate::error::LucarneError;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::fmt;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentErrorKind {
    Unsupported,
    InvalidState,
    Internal,
}

impl fmt::Display for AgentErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "unsupported",
            Self::InvalidState => "invalid_state",
            Self::Internal => "internal",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
#[error("{kind}: {message}")]
pub struct AgentError {
    pub kind: AgentErrorKind,
    pub message: SmolStr,
}

impl From<LucarneError> for AgentErrorKind {
    fn from(err: LucarneError) -> Self {
        match err {
            LucarneError::Closed => Self::InvalidState,
            LucarneError::Adapter(msg) if msg.starts_with("unknown id ") => Self::Unsupported,
            LucarneError::Adapter(_)
            | LucarneError::Dialect(_)
            | LucarneError::Protocol(_)
            | LucarneError::Launcher(_)
            | LucarneError::Runtime(_)
            | LucarneError::Io(_)
            | LucarneError::Json(_)
            | LucarneError::Timeout
            | LucarneError::Other(_) => Self::Internal,
        }
    }
}
