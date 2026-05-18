use smol_str::SmolStr;
use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("input bundle is empty")]
    EmptyInput,
    #[error("agent `{agent}` could not detect a supported session schema")]
    Detection { agent: &'static str },
    #[error("agent `{agent}` does not support this input: {details}")]
    UnsupportedInput {
        agent: &'static str,
        details: &'static str,
    },
    #[error("invalid session structure for `{agent}`: {details}")]
    InvalidStructure {
        agent: &'static str,
        details: &'static str,
    },
    #[error("json parse failed: {0}")]
    Json(#[from] serde_json::Error),
    #[cfg(feature = "copilot")]
    #[error("yaml parse failed: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("utf-8 decode failed: {0}")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("i/o failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Message(SmolStr),
}
