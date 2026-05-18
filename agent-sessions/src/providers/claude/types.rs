use smol_str::SmolStr;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Role {
    User,
    Assistant,
    System,
    Other(SmolStr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextBlock {
    pub text: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThinkingBlock {
    pub text: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImageBlock {
    pub source_type: Option<SmolStr>,
    pub media_type: Option<SmolStr>,
    pub data: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolUseBlock {
    pub id: Option<SmolStr>,
    pub name: SmolStr,
    pub input_json: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolResultBlock {
    pub tool_use_id: Option<SmolStr>,
    pub content: SmolStr,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawBlock {
    pub kind: SmolStr,
    pub raw_json: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContentBlock {
    Text(TextBlock),
    Thinking(ThinkingBlock),
    Image(ImageBlock),
    ToolUse(ToolUseBlock),
    ToolResult(ToolResultBlock),
    Raw(RawBlock),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Version {
    V1,
    V2,
}

#[derive(Debug)]
pub(crate) struct Body {
    pub entries: Box<[Entry]>,
}

#[derive(Debug)]
pub(crate) enum Entry {
    Message(MessageEntry),
    InputSnapshot(InputSnapshotEntry),
    ToolUse(ToolUseEntry),
    ToolResult(ToolResultEntry),
    Attachment(AttachmentEntry),
    PermissionMode(PermissionModeEntry),
    FileHistorySnapshot(FileHistorySnapshotEntry),
    LastPrompt(LastPromptEntry),
    Progress(ProgressEntry),
    QueueOperation(QueueOperationEntry),
    System(SystemEntry),
    Unknown(UnknownEntry),
}

#[derive(Debug)]
pub(crate) struct MessageEntry {
    pub message_id: Option<SmolStr>,
    pub role: Role,
    pub session_id: Option<SmolStr>,
    pub cwd: Option<SmolStr>,
    pub model: Option<SmolStr>,
    pub usage: Option<Usage>,
    pub timestamp: Option<SmolStr>,
    pub stop_reason: Option<SmolStr>,
    pub blocks: Box<[ContentBlock]>,
}

impl MessageEntry {
    #[cfg(feature = "watch")]
    pub fn text(&self) -> Option<SmolStr> {
        let parts: Vec<&str> = self
            .blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(block) => Some(block.text.as_str()),
                _ => None,
            })
            .filter(|text| !text.is_empty())
            .collect();

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" ").into())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub web_search_requests: u64,
    pub speed: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct InputSnapshotEntry {
    pub display: Option<SmolStr>,
    pub pasted_contents_json: Option<SmolStr>,
    pub project: Option<SmolStr>,
    pub session_id: Option<SmolStr>,
    pub timestamp_millis: Option<i64>,
}

#[derive(Debug)]
pub(crate) struct ToolUseEntry {
    pub tool_name: Option<SmolStr>,
    pub tool_input_json: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct ToolResultEntry {
    pub tool_name: Option<SmolStr>,
    pub tool_input_json: Option<SmolStr>,
    pub tool_output_json: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct AttachmentEntry {
    pub attachment_type: Option<SmolStr>,
    pub name: Option<SmolStr>,
    pub species: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct PermissionModeEntry {
    pub permission_mode: Option<SmolStr>,
    pub session_id: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct FileHistorySnapshotEntry {
    pub message_id: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct LastPromptEntry {
    pub session_id: Option<SmolStr>,
    pub last_prompt: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) enum ProgressEntry {
    HookProgress {
        session_id: Option<SmolStr>,
        cwd: Option<SmolStr>,
        timestamp: Option<SmolStr>,
        parent_tool_use_id: Option<SmolStr>,
        tool_use_id: Option<SmolStr>,
        hook_event: Option<SmolStr>,
        hook_name: Option<SmolStr>,
        command: Option<SmolStr>,
    },
    BashProgress {
        session_id: Option<SmolStr>,
        cwd: Option<SmolStr>,
        timestamp: Option<SmolStr>,
        parent_tool_use_id: Option<SmolStr>,
        tool_use_id: Option<SmolStr>,
        output: Option<SmolStr>,
        full_output: Option<SmolStr>,
        elapsed_time_seconds: Option<u64>,
        total_lines: Option<u64>,
    },
    AgentProgress {
        session_id: Option<SmolStr>,
        cwd: Option<SmolStr>,
        timestamp: Option<SmolStr>,
        parent_tool_use_id: Option<SmolStr>,
        tool_use_id: Option<SmolStr>,
        prompt: Option<SmolStr>,
        agent_id: Option<SmolStr>,
        message_json: Option<SmolStr>,
    },
    QueryUpdate {
        session_id: Option<SmolStr>,
        cwd: Option<SmolStr>,
        timestamp: Option<SmolStr>,
        parent_tool_use_id: Option<SmolStr>,
        tool_use_id: Option<SmolStr>,
        query: Option<SmolStr>,
    },
    SearchResultsReceived {
        session_id: Option<SmolStr>,
        cwd: Option<SmolStr>,
        timestamp: Option<SmolStr>,
        parent_tool_use_id: Option<SmolStr>,
        tool_use_id: Option<SmolStr>,
        query: Option<SmolStr>,
        result_count: Option<u64>,
    },
    McpProgress {
        session_id: Option<SmolStr>,
        cwd: Option<SmolStr>,
        timestamp: Option<SmolStr>,
        parent_tool_use_id: Option<SmolStr>,
        tool_use_id: Option<SmolStr>,
        status: Option<SmolStr>,
        server_name: Option<SmolStr>,
        tool_name: Option<SmolStr>,
    },
    WaitingForTask {
        session_id: Option<SmolStr>,
        cwd: Option<SmolStr>,
        timestamp: Option<SmolStr>,
        parent_tool_use_id: Option<SmolStr>,
        tool_use_id: Option<SmolStr>,
        task_description: Option<SmolStr>,
        task_type: Option<SmolStr>,
    },
    Other {
        kind: Option<SmolStr>,
        session_id: Option<SmolStr>,
        cwd: Option<SmolStr>,
        timestamp: Option<SmolStr>,
        parent_tool_use_id: Option<SmolStr>,
        tool_use_id: Option<SmolStr>,
    },
}

#[derive(Debug)]
pub(crate) struct QueueOperationEntry {
    pub session_id: Option<SmolStr>,
    pub operation: Option<SmolStr>,
    pub content: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct SystemEntry {
    pub subtype: Option<SmolStr>,
    pub level: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct UnknownEntry {
    pub kind: SmolStr,
    pub raw_json: SmolStr,
    pub timestamp: Option<SmolStr>,
}
