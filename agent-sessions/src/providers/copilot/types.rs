use smol_str::SmolStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Version {
    CliEventsV1,
    ChatSessionV1,
}

#[derive(Debug)]
pub(crate) enum Body {
    CliEvents(CliEventsBody),
    ChatSession(ChatSessionBody),
}

#[derive(Debug)]
pub(crate) struct CliEventsBody {
    pub workspace: Option<WorkspaceMetadata>,
    pub records: Box<[CliRecord]>,
}

#[derive(Debug)]
pub(crate) struct WorkspaceMetadata {
    pub id: Option<SmolStr>,
    pub cwd: Option<SmolStr>,
    pub summary: Option<SmolStr>,
    pub created_at: Option<SmolStr>,
    pub updated_at: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) enum CliRecord {
    Message(CliMessage),
    ToolExecution(ToolExecution),
    TaskComplete(TaskComplete),
    TurnBoundary(TurnBoundary),
    SystemNotification(SystemNotification),
    SessionEvent(SessionEvent),
    SubagentEvent(SubagentEvent),
    Abort(AbortRecord),
    Unknown(UnknownRecord),
}

#[derive(Debug)]
pub(crate) struct CliMessage {
    pub message_id: Option<SmolStr>,
    pub parent_tool_call_id: Option<SmolStr>,
    pub role: Role,
    pub content: SmolStr,
    pub output_tokens: Option<u64>,
    pub tool_names: Box<[SmolStr]>,
    pub tool_requests: Box<[ToolRequest]>,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct ToolRequest {
    pub name: Option<SmolStr>,
    pub tool_call_id: Option<SmolStr>,
    pub command: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) enum ToolExecution {
    Start {
        tool_name: Option<SmolStr>,
        tool_call_id: Option<SmolStr>,
        command: Option<SmolStr>,
        model: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
    Complete {
        tool_name: Option<SmolStr>,
        tool_call_id: Option<SmolStr>,
        success: Option<bool>,
        error_message: Option<SmolStr>,
        error_code: Option<SmolStr>,
        model: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
}

#[derive(Debug)]
pub(crate) struct TaskComplete {
    pub summary: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) enum TurnBoundary {
    Start {
        turn_id: Option<SmolStr>,
        interaction_id: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
    End {
        turn_id: Option<SmolStr>,
        interaction_id: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
}

#[derive(Debug)]
pub(crate) enum SystemNotification {
    ShellCompleted {
        content: Option<SmolStr>,
        shell_id: Option<SmolStr>,
        exit_code: Option<i64>,
        description: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
    Other {
        content: Option<SmolStr>,
        notification_type: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
}

#[derive(Debug)]
pub(crate) enum SessionEvent {
    Start {
        session_id: Option<SmolStr>,
        selected_model: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
    Resume {
        selected_model: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
    Shutdown {
        current_model: Option<SmolStr>,
        model_usages: Box<[ShutdownModelUsage]>,
        timestamp: Option<SmolStr>,
    },
    ModeChanged {
        previous_mode: Option<SmolStr>,
        new_mode: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
    ModelChange {
        previous_model: Option<SmolStr>,
        new_model: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
    PlanChanged {
        operation: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
    CompactionStart {
        timestamp: Option<SmolStr>,
    },
    CompactionComplete {
        success: Option<bool>,
        summary_content: Option<SmolStr>,
        error_message: Option<SmolStr>,
        error_code: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
    Truncation {
        timestamp: Option<SmolStr>,
    },
    Error {
        error_type: Option<SmolStr>,
        message: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ShutdownModelUsage {
    pub model: SmolStr,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
}

#[derive(Debug)]
pub(crate) enum SubagentEvent {
    Started {
        tool_call_id: Option<SmolStr>,
        agent_name: Option<SmolStr>,
        agent_display_name: Option<SmolStr>,
        agent_description: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
    Completed {
        tool_call_id: Option<SmolStr>,
        agent_name: Option<SmolStr>,
        agent_display_name: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
    Failed {
        tool_call_id: Option<SmolStr>,
        agent_name: Option<SmolStr>,
        agent_display_name: Option<SmolStr>,
        error: Option<SmolStr>,
        timestamp: Option<SmolStr>,
    },
    Deselected {
        timestamp: Option<SmolStr>,
    },
}

#[derive(Debug)]
pub(crate) struct AbortRecord {
    pub reason: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct UnknownRecord {
    pub kind: SmolStr,
    pub raw_json: SmolStr,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct ChatSessionBody {
    pub(crate) session_id: Option<SmolStr>,
    pub(crate) workspace_id: Option<SmolStr>,
    pub(crate) model: Option<SmolStr>,
    pub(crate) mode: Option<SmolStr>,
    pub(crate) requests: Box<[ChatRequest]>,
}

#[derive(Debug)]
pub(crate) struct ChatRequest {
    pub(crate) request_id: SmolStr,
    pub(crate) prompt: Option<SmolStr>,
    pub(crate) timestamp: Option<i64>,
    pub(crate) model_id: Option<SmolStr>,
    pub(crate) response: Box<[ChatResponsePart]>,
}

#[derive(Debug)]
pub(crate) struct ChatResponsePart {
    pub(crate) text: Option<SmolStr>,
}
