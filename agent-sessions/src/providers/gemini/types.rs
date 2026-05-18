use smol_str::SmolStr;
use std::borrow::Cow;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Version {
    V1,
}

#[derive(Debug)]
pub(crate) struct Body {
    pub session_id: SmolStr,
    pub cwd: Option<SmolStr>,
    pub start_time: Option<SmolStr>,
    pub last_updated: Option<SmolStr>,
    pub kind: Option<SmolStr>,
    pub summary: Option<SmolStr>,
    pub directories_json: Option<SmolStr>,
    pub entries: Box<[Entry]>,
}

#[derive(Debug)]
pub(crate) enum Entry {
    User(UserMessage),
    Gemini(GeminiMessage),
    Info(InfoMessage),
    Unknown(UnknownEntry),
}

#[derive(Debug)]
pub(crate) struct UserMessage {
    pub id: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
    pub content: Box<[UserContentPart]>,
}

#[derive(Debug)]
pub(crate) enum UserContentPart {
    Text(TextPart),
    Raw(RawPart),
}

#[derive(Debug)]
pub(crate) struct TextPart {
    pub text: SmolStr,
}

#[derive(Debug)]
pub(crate) struct GeminiMessage {
    pub id: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
    pub content: Option<SmolStr>,
    pub model: Option<SmolStr>,
    pub thoughts: Box<[Thought]>,
    pub tokens: Option<TokenUsage>,
    pub tool_calls: Box<[ToolCall]>,
}

#[derive(Debug)]
pub(crate) struct InfoMessage {
    pub id: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
    pub content: SmolStr,
}

#[derive(Debug)]
pub(crate) struct Thought {
    pub description: SmolStr,
}

#[derive(Debug)]
pub(crate) struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub thoughts: u64,
    pub tool: u64,
    pub total: u64,
}

#[derive(Debug)]
pub(crate) struct ToolCall {
    pub id: Option<SmolStr>,
    pub name: SmolStr,
    pub args_json: Option<SmolStr>,
    pub status: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
    pub result_display_json: Option<SmolStr>,
    pub responses: Box<[ToolResponse]>,
}

impl ToolCall {
    pub fn is_shell(&self) -> bool {
        matches!(self.name.as_ref(), "run_shell_command" | "shell_command")
    }

    pub fn shell_command(&self) -> Option<Cow<'_, str>> {
        let args_json = self.args_json.as_deref()?;
        serde_json::from_str::<ShellArgs<'_>>(args_json)
            .ok()?
            .command
    }
}

#[derive(Debug)]
pub(crate) enum ToolResponse {
    Output(OutputResponse),
    Error(ErrorResponse),
    Raw(RawPart),
}

#[derive(Debug)]
pub(crate) struct OutputResponse {
    pub id: Option<SmolStr>,
    pub name: Option<SmolStr>,
    pub output: SmolStr,
}

#[derive(Debug)]
pub(crate) struct ErrorResponse {
    pub id: Option<SmolStr>,
    pub name: Option<SmolStr>,
    pub error: SmolStr,
}

#[derive(Debug)]
pub(crate) struct RawPart {
    pub raw_json: SmolStr,
}

#[derive(Debug)]
pub(crate) struct UnknownEntry {
    pub kind: SmolStr,
    pub raw_json: SmolStr,
    pub timestamp: Option<SmolStr>,
}

#[derive(Deserialize)]
struct ShellArgs<'a> {
    #[serde(borrow)]
    command: Option<Cow<'a, str>>,
}
