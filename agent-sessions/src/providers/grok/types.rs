//! Clean parsed types for Grok Build sessions (`updates.jsonl` + `summary.json`).

use smol_str::SmolStr;

#[derive(Debug)]
pub(crate) struct Body {
    pub(crate) session_id: Option<SmolStr>,
    pub(crate) entries: Box<[Entry]>,
}

#[derive(Debug)]
pub(crate) enum Entry {
    SessionInfo {
        session_id: SmolStr,
        cwd: Option<SmolStr>,
        title: Option<SmolStr>,
        model: Option<SmolStr>,
        created_at: Option<SmolStr>,
        updated_at: Option<SmolStr>,
    },
    UserMessage {
        text: SmolStr,
        timestamp: Option<SmolStr>,
    },
    AssistantMessage {
        text: SmolStr,
        timestamp: Option<SmolStr>,
        model: Option<SmolStr>,
    },
    Thinking {
        text: SmolStr,
        timestamp: Option<SmolStr>,
    },
    ToolCall {
        id: Option<SmolStr>,
        name: SmolStr,
        input_json: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
    ToolResult {
        id: Option<SmolStr>,
        name: Option<SmolStr>,
        text: SmolStr,
        is_error: bool,
        timestamp: Option<SmolStr>,
    },
    TurnCompleted {
        stop_reason: Option<SmolStr>,
        /// Final assistant text from the same parse window (if any).
        /// Used by watch so channel notify still works when synthesize cannot
        /// rebuild duration from timestamps.
        last_agent_message: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
}
