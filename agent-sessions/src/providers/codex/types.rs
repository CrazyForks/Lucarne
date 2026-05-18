use smol_str::SmolStr;
use std::borrow::Cow;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Role {
    User,
    Assistant,
    System,
    Developer,
    Other(SmolStr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextBlock {
    pub text: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImageBlock {
    pub image_url: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawBlock {
    pub kind: SmolStr,
    pub raw_json: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContentBlock {
    Text(TextBlock),
    Image(ImageBlock),
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
    SessionMeta(SessionMeta),
    TurnContext(TurnContext),
    Message(Message),
    FunctionCall(FunctionCall),
    FunctionCallOutput(FunctionCallOutput),
    CustomToolCall(CustomToolCall),
    CustomToolCallOutput(CustomToolCallOutput),
    WebSearchCall(WebSearchCall),
    GhostSnapshot(GhostSnapshot),
    Compacted(Compacted),
    Reasoning(Reasoning),
    EventMsg(EventMsg),
    Unknown(UnknownRecord),
}

#[derive(Debug)]
pub(crate) struct SessionMeta {
    pub session_id: Option<SmolStr>,
    pub cwd: Option<SmolStr>,
    pub originator: Option<SmolStr>,
    pub model: Option<SmolStr>,
    pub cli_version: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct TurnContext {
    pub turn_id: Option<SmolStr>,
    pub cwd: Option<SmolStr>,
    pub current_date: Option<SmolStr>,
    pub timezone: Option<SmolStr>,
    pub model: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct Message {
    pub role: Role,
    pub model: Option<SmolStr>,
    pub phase: Option<SmolStr>,
    pub blocks: Box<[ContentBlock]>,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct FunctionCall {
    pub id: Option<SmolStr>,
    pub call_id: Option<SmolStr>,
    pub name: SmolStr,
    pub arguments_json: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
}

impl FunctionCall {
    pub fn shell_command(&self) -> Option<Cow<'_, str>> {
        let args_json = self.arguments_json.as_deref()?;
        serde_json::from_str::<ShellArgs<'_>>(args_json)
            .ok()
            .and_then(|args| args.cmd.or(args.command))
    }
}

#[derive(Debug)]
pub(crate) struct FunctionCallOutput {
    pub call_id: SmolStr,
    pub output: SmolStr,
    pub timestamp: Option<SmolStr>,
}

impl FunctionCallOutput {
    pub fn shell_duration_seconds(&self) -> f64 {
        let trimmed = self.output.trim();
        if trimmed.is_empty() {
            return 0.0;
        }

        if let Ok(parsed) = serde_json::from_str::<ShellOutputEnvelope<'_>>(trimmed) {
            if let Some(seconds) = parsed
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.duration_seconds)
            {
                return seconds.max(0.0);
            }
            if let Some(output) = parsed.output
                && let Some(seconds) = parse_wall_time_seconds(output.as_ref())
            {
                return seconds.max(0.0);
            }
        }

        parse_wall_time_seconds(trimmed).unwrap_or(0.0)
    }
}

#[derive(Debug)]
pub(crate) struct CustomToolCall {
    pub call_id: Option<SmolStr>,
    pub name: SmolStr,
    pub input: Option<SmolStr>,
    pub status: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct CustomToolCallOutput {
    pub call_id: Option<SmolStr>,
    pub output: SmolStr,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct WebSearchCall {
    pub status: Option<SmolStr>,
    pub action_type: Option<SmolStr>,
    pub query: Option<SmolStr>,
    pub queries: Box<[SmolStr]>,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct GhostSnapshot {
    pub commit_id: Option<SmolStr>,
    pub parent_id: Option<SmolStr>,
    pub preexisting_untracked_files: Box<[SmolStr]>,
    pub preexisting_untracked_dirs: Box<[SmolStr]>,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct Compacted {
    pub message: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct Reasoning {
    pub summary: Box<[SmolStr]>,
    pub timestamp: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct EventMsg {
    pub kind: SmolStr,
    pub turn_id: Option<SmolStr>,
    pub last_agent_message: Option<SmolStr>,
    pub timestamp: Option<SmolStr>,
    pub data: EventMsgData,
}

#[derive(Debug)]
pub(crate) enum EventMsgData {
    TaskStarted(TaskStartedEventMsg),
    TaskComplete(TaskCompleteEventMsg),
    AgentMessage(AgentMessageEventMsg),
    AgentReasoning(AgentReasoningEventMsg),
    UserMessage(UserMessageEventMsg),
    TokenCount(TokenCountEventMsg),
    ExecCommandEnd(ExecCommandEndEventMsg),
    PatchApplyEnd(PatchApplyEndEventMsg),
    TurnAborted(TurnAbortedEventMsg),
    ContextCompacted,
    WebSearchEnd(WebSearchEndEventMsg),
    ThreadRolledBack(ThreadRolledBackEventMsg),
    CollabWaitingEnd(CollabWaitingEndEventMsg),
    McpToolCallEnd(McpToolCallEndEventMsg),
    DynamicToolCallRequest(DynamicToolCallRequestEventMsg),
    DynamicToolCallResponse(DynamicToolCallResponseEventMsg),
    CollabAgentSpawnEnd(CollabAgentSpawnEndEventMsg),
    CollabCloseEnd(CollabCloseEndEventMsg),
    CollabAgentInteractionEnd(CollabAgentInteractionEndEventMsg),
    Error(ErrorEventMsg),
    EnteredReviewMode(EnteredReviewModeEventMsg),
    ExitedReviewMode(ExitedReviewModeEventMsg),
    Unknown(UnknownEventMsg),
}

#[derive(Debug)]
pub(crate) struct TaskStartedEventMsg {
    pub model_context_window: Option<i64>,
    pub collaboration_mode_kind: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct TaskCompleteEventMsg {
    pub completed_at: Option<i64>,
    pub duration_ms: Option<u64>,
    pub time_to_first_token_ms: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct AgentMessageEventMsg {
    pub message: Option<SmolStr>,
    pub phase: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct AgentReasoningEventMsg {
    pub text: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct UserMessageEventMsg {
    pub message: Option<SmolStr>,
    pub images_json: Option<SmolStr>,
    pub local_images_json: Option<SmolStr>,
    pub text_elements_json: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct TokenCountEventMsg {
    pub info_json: Option<SmolStr>,
}

impl TokenCountEventMsg {
    pub fn info(&self) -> Option<TokenCountInfo> {
        let raw = self.info_json.as_deref()?;
        serde_json::from_str(raw).ok()
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub(crate) struct TokenCountInfo {
    #[serde(default)]
    pub model: Option<SmolStr>,
    #[serde(default, alias = "model_name")]
    pub model_name: Option<SmolStr>,
    #[serde(default)]
    pub last_token_usage: Option<TokenUsageInfo>,
    #[serde(default)]
    pub total_token_usage: Option<TokenUsageInfo>,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub(crate) struct TokenUsageInfo {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub reasoning_output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

#[derive(Debug)]
pub(crate) struct ExecCommandEndEventMsg {
    pub call_id: Option<SmolStr>,
    pub command_json: Option<SmolStr>,
    pub parsed_cmd_json: Option<SmolStr>,
    pub stdout: Option<SmolStr>,
    pub stderr: Option<SmolStr>,
    pub aggregated_output: Option<SmolStr>,
    pub exit_code: Option<i64>,
    pub duration_json: Option<SmolStr>,
    pub formatted_output: Option<SmolStr>,
    pub status: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct PatchApplyEndEventMsg {
    pub call_id: Option<SmolStr>,
    pub stdout: Option<SmolStr>,
    pub stderr: Option<SmolStr>,
    pub success: Option<bool>,
    pub changes_json: Option<SmolStr>,
    pub status: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct TurnAbortedEventMsg {
    pub reason: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct WebSearchEndEventMsg {
    pub call_id: Option<SmolStr>,
    pub query: Option<SmolStr>,
    pub action_json: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct ThreadRolledBackEventMsg {
    pub num_turns: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct CollabWaitingEndEventMsg {
    pub call_id: Option<SmolStr>,
    pub agent_statuses_json: Option<SmolStr>,
    pub statuses_json: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct McpToolCallEndEventMsg {
    pub call_id: Option<SmolStr>,
    pub invocation_json: Option<SmolStr>,
    pub duration_json: Option<SmolStr>,
    pub result_json: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct DynamicToolCallRequestEventMsg {
    pub call_id: Option<SmolStr>,
    pub tool: Option<SmolStr>,
    pub arguments_json: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct DynamicToolCallResponseEventMsg {
    pub call_id: Option<SmolStr>,
    pub tool: Option<SmolStr>,
    pub arguments_json: Option<SmolStr>,
    pub content_items_json: Option<SmolStr>,
    pub success: Option<bool>,
    pub error_json: Option<SmolStr>,
    pub duration_json: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct CollabAgentSpawnEndEventMsg {
    pub call_id: Option<SmolStr>,
    pub sender_thread_id: Option<SmolStr>,
    pub new_thread_id: Option<SmolStr>,
    pub new_agent_nickname: Option<SmolStr>,
    pub new_agent_role: Option<SmolStr>,
    pub prompt: Option<SmolStr>,
    pub model: Option<SmolStr>,
    pub reasoning_effort: Option<SmolStr>,
    pub status_json: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct CollabCloseEndEventMsg {
    pub call_id: Option<SmolStr>,
    pub sender_thread_id: Option<SmolStr>,
    pub receiver_thread_id: Option<SmolStr>,
    pub receiver_agent_nickname: Option<SmolStr>,
    pub receiver_agent_role: Option<SmolStr>,
    pub status_json: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct CollabAgentInteractionEndEventMsg {
    pub call_id: Option<SmolStr>,
    pub sender_thread_id: Option<SmolStr>,
    pub receiver_thread_id: Option<SmolStr>,
    pub receiver_agent_nickname: Option<SmolStr>,
    pub receiver_agent_role: Option<SmolStr>,
    pub prompt: Option<SmolStr>,
    pub status_json: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct ErrorEventMsg {
    pub message: Option<SmolStr>,
    pub codex_error_info: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct EnteredReviewModeEventMsg {
    pub target_json: Option<SmolStr>,
    pub user_facing_hint: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct ExitedReviewModeEventMsg {
    pub review_output_json: Option<SmolStr>,
}

#[derive(Debug)]
pub(crate) struct UnknownEventMsg {
    pub raw_json: SmolStr,
}

#[derive(Debug)]
pub(crate) struct UnknownRecord {
    pub kind: SmolStr,
    pub raw_json: SmolStr,
    pub timestamp: Option<SmolStr>,
}

#[derive(Deserialize)]
struct ShellArgs<'a> {
    #[serde(borrow)]
    cmd: Option<Cow<'a, str>>,
    #[serde(borrow)]
    command: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct ShellOutputEnvelope<'a> {
    #[serde(default)]
    metadata: Option<ShellOutputMetadata>,
    #[serde(default, borrow)]
    output: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct ShellOutputMetadata {
    #[serde(default)]
    duration_seconds: Option<f64>,
}

fn parse_wall_time_seconds(text: &str) -> Option<f64> {
    let marker = "Wall time:";
    let start = text.find(marker)? + marker.len();
    let tail = text[start..].trim_start();
    let mut parts = tail.split_whitespace();
    let value = parts.next()?.parse::<f64>().ok()?;
    let unit = parts
        .next()?
        .trim_matches(|ch: char| ch == ',' || ch == ')' || ch == '.');

    Some(match unit.to_ascii_lowercase().as_str() {
        "millisecond" | "milliseconds" | "ms" => value / 1000.0,
        "second" | "seconds" | "sec" | "secs" | "s" => value,
        _ => return None,
    })
}
