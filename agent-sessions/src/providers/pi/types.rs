//! Clean parsed types for Pi sessions.

use smol_str::SmolStr;
// ── Body ───────────────────────────────────────────────────────────

#[derive(Debug)]
pub(crate) struct Body {
    pub(crate) entries: Box<[Entry]>,
}

// ── Entry ──────────────────────────────────────────────────────────

#[derive(Debug)]
pub(crate) enum Entry {
    SessionInfo {
        session_id: SmolStr,
        cwd: Option<SmolStr>,
        timestamp: Option<SmolStr>,
        name: Option<SmolStr>,
    },
    UserMessage {
        text: SmolStr,
        blocks: Box<[Block]>,
        timestamp: Option<SmolStr>,
    },
    AssistantMessage {
        text: SmolStr,
        blocks: Box<[Block]>,
        timestamp: Option<SmolStr>,
        model: Option<SmolStr>,
        stop_reason: Option<SmolStr>,
        error_message: Option<SmolStr>,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
        total_tokens: u64,
    },
    ToolResult {
        tool_name: SmolStr,
        text: SmolStr,
        is_error: bool,
    },
    ModelChange {
        provider: Option<SmolStr>,
        model_id: SmolStr,
    },
    ThinkingLevelChange {
        level: SmolStr,
    },
    Compaction {
        summary: SmolStr,
        first_kept_entry_id: Option<SmolStr>,
        tokens_before: Option<u64>,
    },
    BranchSummary {
        summary: SmolStr,
        from_id: Option<SmolStr>,
    },
    Custom {
        custom_type: SmolStr,
        data_json: Option<SmolStr>,
    },
    CustomMessage {
        custom_type: SmolStr,
        text: Option<SmolStr>,
        display: bool,
    },
    Label {
        target_id: Option<SmolStr>,
        label: Option<SmolStr>,
    },
}

// ── Content blocks ─────────────────────────────────────────────────

/// Provider-specific block type. Mapped to `agent_session::ContentBlock`
/// in `event.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Block {
    Text {
        text: SmolStr,
    },
    Thinking {
        text: SmolStr,
    },
    ToolCall {
        id: Option<SmolStr>,
        name: SmolStr,
        input_json: Option<SmolStr>,
    },
    ToolResult {
        tool_use_id: Option<SmolStr>,
        text: SmolStr,
        is_error: bool,
    },
    Image {
        data: Option<SmolStr>,
        mime_type: Option<SmolStr>,
    },
    Other {
        block_type: SmolStr,
    },
}
