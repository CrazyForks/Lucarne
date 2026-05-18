//! Tool-call sub-domain: generic tool-call payload plus small constructor
//! helpers used by tests and adapters.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use smol_str::SmolStr;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: SmolStr,
    #[serde(default)]
    pub input: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubAgentCall {
    pub tool_name: SmolStr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_model: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_thread_id: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_session_ref: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nickname: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<SmolStr>,
    #[serde(default)]
    pub raw: Value,
}

pub fn tool_call(name: impl Into<SmolStr>, input: impl Into<Value>) -> ToolCall {
    ToolCall {
        name: name.into(),
        input: input.into(),
    }
}

pub fn shell(cmd: impl Into<String>) -> ToolCall {
    tool_call("shell", serde_json::json!({ "command": cmd.into() }))
}

pub fn read_tool(path: impl Into<String>) -> ToolCall {
    tool_call("read", serde_json::json!({ "path": path.into() }))
}

pub fn write_tool(path: impl Into<String>, content: impl Into<String>) -> ToolCall {
    tool_call(
        "write",
        serde_json::json!({ "path": path.into(), "content": content.into() }),
    )
}

pub fn edit_tool(
    path: impl Into<String>,
    old: impl Into<String>,
    new: impl Into<String>,
) -> ToolCall {
    tool_call(
        "edit",
        serde_json::json!({
            "path": path.into(),
            "old_string": old.into(),
            "new_string": new.into(),
        }),
    )
}

pub fn unknown_tool(name: impl Into<String>, raw: Option<Value>) -> ToolCall {
    tool_call(name.into(), raw.unwrap_or(Value::Null))
}
