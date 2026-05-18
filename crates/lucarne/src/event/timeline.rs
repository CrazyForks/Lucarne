//! Timeline sub-domain: the `Timeline` payload and its `TimelineItem`
//! variants (user / assistant / reasoning / tool-call / tool-result /
//! todo / plan / compaction) plus the small constructor helpers used
//! by dialects to build items.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::tool::ToolCall;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Timeline {
    pub item: TimelineItem,
}

/// The visible "card" in the conversation — user message, assistant
/// message, tool call, etc. Corresponds to Go's `TimelineItem`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TimelineItem {
    #[serde(rename = "type")]
    pub ty: TimelineType,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_message: Option<UserMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_message: Option<AssistantMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<ToolCallItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolResultItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo: Option<TodoBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionBlock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TimelineType {
    #[default]
    UserMessage,
    AssistantMessage,
    Reasoning,
    ToolCall,
    ToolResult,
    Todo,
    Plan,
    Compaction,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserMessage {
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub text: String,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub streaming: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Reasoning {
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolCallItem {
    pub call: ToolCall,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolResultItem {
    pub call_id: String,
    pub result: ToolResult,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TodoBlock {
    pub items: Vec<Todo>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Todo {
    pub id: String,
    pub text: String,
    pub status: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanBlock {
    pub steps: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompactionBlock {
    pub summary: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolResult {
    #[serde(default, skip_serializing_if = "super::is_zero_i32")]
    pub exit_code: i32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub typed_output: BTreeMap<String, serde_json::Value>,
    /// True when this result is a partial (streaming) update, not the final result.
    #[serde(default)]
    pub partial: bool,
}

// ——— Constructors ———

pub fn new_timeline_assistant(id: &str, text: &str, streaming: bool) -> TimelineItem {
    TimelineItem {
        ty: TimelineType::AssistantMessage,
        id: id.to_string(),
        assistant_message: Some(AssistantMessage {
            text: text.to_string(),
            streaming,
        }),
        ..Default::default()
    }
}

pub fn new_timeline_reasoning(id: &str, text: &str) -> TimelineItem {
    TimelineItem {
        ty: TimelineType::Reasoning,
        id: id.to_string(),
        reasoning: Some(Reasoning {
            text: text.to_string(),
        }),
        ..Default::default()
    }
}

pub fn new_timeline_tool_call(id: &str, call: ToolCall) -> TimelineItem {
    TimelineItem {
        ty: TimelineType::ToolCall,
        id: id.to_string(),
        tool_call: Some(ToolCallItem { call }),
        ..Default::default()
    }
}

pub fn new_timeline_tool_result(id: &str, call_id: &str, r: ToolResult) -> TimelineItem {
    TimelineItem {
        ty: TimelineType::ToolResult,
        id: id.to_string(),
        tool_result: Some(ToolResultItem {
            call_id: call_id.to_string(),
            result: r,
        }),
        ..Default::default()
    }
}

pub fn new_timeline_user(id: &str, text: &str) -> TimelineItem {
    TimelineItem {
        ty: TimelineType::UserMessage,
        id: id.to_string(),
        user_message: Some(UserMessage {
            text: text.to_string(),
        }),
        ..Default::default()
    }
}
