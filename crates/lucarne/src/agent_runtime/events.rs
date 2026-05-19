use crate::event::CommandResultPayload;

use super::types::{CallId, InterventionRequest, RuntimeBusFilter};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageEvent {
    pub role: MessageRole,
    pub text: SmolStr,
    #[serde(default, skip_serializing_if = "is_false")]
    pub streaming: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: SmolStr,
    pub filename: SmolStr,
    pub media_type: SmolStr,
    pub data_base64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningEvent {
    pub text: SmolStr,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallEvent {
    pub call_id: CallId,
    pub name: SmolStr,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResultEvent {
    pub call_id: CallId,
    pub output: serde_json::Value,
    pub is_error: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageEvent {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandResultEvent {
    pub command: SmolStr,
    pub result: CommandResultPayload,
}

/// Emitted when the provider signals it has finished the current turn.
/// Downstream consumers should treat this as the authoritative
/// "you may send the next user message now" signal — inferring turn
/// boundaries from idle timeouts is fragile when the model pauses to
/// think for >10 seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnCompletedEvent {
    /// Provider turn id when available.
    pub turn_id: SmolStr,
    pub usage: Option<UsageEvent>,
}

/// Emitted when the provider reports a turn-level failure (e.g. API
/// error, token exhaustion). The session stays open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnFailedEvent {
    pub turn_id: SmolStr,
    pub error: SmolStr,
    pub code: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Message(MessageEvent),
    Attachment(Attachment),
    Reasoning(ReasoningEvent),
    ToolCall(ToolCallEvent),
    ToolResult(ToolResultEvent),
    Usage(UsageEvent),
    CommandResult(CommandResultEvent),
    InterventionRequest(InterventionRequest),
    TurnCompleted(TurnCompletedEvent),
    TurnFailed(TurnFailedEvent),
}

impl Event {
    pub(crate) fn matches_filter(&self, filter: &RuntimeBusFilter) -> bool {
        match self {
            Self::Message(MessageEvent {
                role: MessageRole::User,
                ..
            }) => filter.user_messages,
            Self::Message(MessageEvent {
                role: MessageRole::Assistant,
                ..
            }) => filter.assistant_messages,
            Self::Attachment(_) => filter.assistant_messages,
            Self::Reasoning(_) => filter.reasoning,
            Self::ToolCall(_) => filter.tool_calls,
            Self::ToolResult(_) => filter.tool_results,
            Self::Usage(_) => filter.usage,
            Self::CommandResult(_) => filter.assistant_messages,
            Self::InterventionRequest(_) => filter.intervention_requests,
            Self::TurnCompleted(_) | Self::TurnFailed(_) => filter.turn_lifecycle,
        }
    }
}
