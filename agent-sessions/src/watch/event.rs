use std::path::PathBuf;

use smol_str::SmolStr;

use crate::agent_session::{Actor, Body, OperationKind, OperationPhase};

use super::WatchProvider;

#[cfg(any(feature = "codex", feature = "claude"))]
pub(crate) fn watch_smol<T>(value: T) -> SmolStr
where
    T: Into<SmolStr>,
{
    value.into()
}

#[cfg(any(
    feature = "codex",
    feature = "claude",
    feature = "copilot",
    feature = "gemini",
    feature = "pi"
))]
pub(crate) fn watch_smol_opt<T>(value: Option<T>) -> Option<SmolStr>
where
    T: Into<SmolStr>,
{
    value.map(Into::into)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WatchChange {
    Created,
    Updated,
    Deleted,
    Truncated,
    ParseError,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WatchUpdate {
    pub provider: WatchProvider,
    pub path: PathBuf,
    pub session_id: Option<SmolStr>,
    pub cwd: Option<SmolStr>,
    pub change: WatchChange,
    pub events: Box<[WatchEvent]>,
    pub error: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum WatchEvent {
    UserMessage(WatchMessage),
    AssistantMessage(WatchAssistantMessage),
    Attachment(WatchAttachment),
    ToolCall(WatchToolCall),
    ToolResult(WatchToolResult),
    Usage(WatchUsage),
    TurnCompleted(WatchTurnCompleted),
    TurnFailed(WatchTurnFailed),
    State(WatchState),
    Snapshot(WatchSnapshot),
    Unknown(WatchUnknown),
    Other(WatchOther),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WatchEventMeta {
    pub id: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
    pub turn_id: Option<SmolStr>,
    pub op_id: Option<SmolStr>,
    pub parent_op_id: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchMessage {
    pub meta: WatchEventMeta,
    pub text: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchAssistantMessage {
    pub meta: WatchEventMeta,
    pub model: Option<SmolStr>,
    pub phase: Option<SmolStr>,
    pub text: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchAttachment {
    pub meta: WatchEventMeta,
    pub id: Option<SmolStr>,
    pub filename: SmolStr,
    pub media_type: SmolStr,
    pub data_base64: SmolStr,
    pub caption: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WatchToolCall {
    pub meta: WatchEventMeta,
    pub kind: OperationKind,
    pub phase: OperationPhase,
    pub call_id: Option<SmolStr>,
    pub name: SmolStr,
    pub input_json: Option<SmolStr>,
    pub command: Option<SmolStr>,
    pub file_path: Option<SmolStr>,
    pub lines_added: u64,
    pub lines_removed: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WatchToolResult {
    pub meta: WatchEventMeta,
    pub kind: OperationKind,
    pub phase: OperationPhase,
    pub call_id: Option<SmolStr>,
    pub name: SmolStr,
    pub output_json: Option<SmolStr>,
    pub is_error: bool,
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchUsage {
    pub meta: WatchEventMeta,
    pub model: Option<SmolStr>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub cached_tokens: u64,
    pub reasoning_tokens: u64,
    pub tool_tokens: u64,
    pub total_tokens: u64,
    pub web_search_requests: u64,
    pub speed: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchTurnCompleted {
    pub meta: WatchEventMeta,
    pub last_agent_message: Option<SmolStr>,
    pub duration_ms: Option<u64>,
    pub value_json: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchTurnFailed {
    pub meta: WatchEventMeta,
    pub reason: Option<SmolStr>,
    pub duration_ms: Option<u64>,
    pub value_json: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchState {
    pub meta: WatchEventMeta,
    pub kind: SmolStr,
    pub value_json: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchSnapshot {
    pub meta: WatchEventMeta,
    pub actor: Actor,
    pub kind: SmolStr,
    pub value_json: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchUnknown {
    pub meta: WatchEventMeta,
    pub actor: Actor,
    pub kind: SmolStr,
    pub raw_json: SmolStr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WatchOther {
    pub meta: WatchEventMeta,
    pub actor: Actor,
    pub body: Body,
}

impl WatchEvent {
    pub(crate) fn cloned_timestamp(&self) -> Option<SmolStr> {
        self.meta().timestamp.clone()
    }

    pub fn meta(&self) -> &WatchEventMeta {
        match self {
            Self::UserMessage(event) => &event.meta,
            Self::AssistantMessage(event) => &event.meta,
            Self::Attachment(event) => &event.meta,
            Self::ToolCall(event) => &event.meta,
            Self::ToolResult(event) => &event.meta,
            Self::Usage(event) => &event.meta,
            Self::TurnCompleted(event) => &event.meta,
            Self::TurnFailed(event) => &event.meta,
            Self::State(event) => &event.meta,
            Self::Snapshot(event) => &event.meta,
            Self::Unknown(event) => &event.meta,
            Self::Other(event) => &event.meta,
        }
    }

    pub fn timestamp(&self) -> Option<&str> {
        self.meta().timestamp.as_deref()
    }

    pub fn user_text(&self) -> Option<&str> {
        match self {
            Self::UserMessage(message) => message.text.as_deref(),
            _ => None,
        }
    }

    pub fn assistant_text(&self) -> Option<&str> {
        match self {
            Self::AssistantMessage(message) => message.text.as_deref(),
            _ => None,
        }
    }

    #[cfg(any(
        feature = "claude",
        feature = "copilot",
        feature = "cursor",
        feature = "gemini",
        feature = "pi"
    ))]
    pub(crate) fn assistant_message_mut(&mut self) -> Option<&mut WatchAssistantMessage> {
        match self {
            Self::AssistantMessage(message) => Some(message),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("watcher is not configured with any providers")]
    NoProviders,
    #[error("watcher has no existing roots to watch")]
    NoRoots,
    #[error("file watch failed: {0}")]
    Notify(#[from] notify::Error),
    #[error("watch event channel disconnected")]
    Disconnected,
}
