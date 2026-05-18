use super::events::Event;
use crate::ProviderId;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct SessionId(pub SmolStr);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct InstanceId(pub SmolStr);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct CommandId(pub SmolStr);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct SessionRef(pub SmolStr);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct CallId(pub SmolStr);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AgentCapabilities {
    pub reasoning_stream: bool,
    pub tool_stream: bool,
    pub usage_reporting: bool,
    pub structured_intervention: bool,
    pub command_catalog: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProbeResult {
    pub provider_id: ProviderId,
    pub provider_version: Option<SmolStr>,
    pub capabilities: AgentCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AgentInput {
    pub text: SmolStr,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<AgentImageInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentImageInput {
    pub media_type: SmolStr,
    pub data_base64: SmolStr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandSource {
    ProviderNative,
    AdapterMapped,
}

impl Default for AgentCommandSource {
    fn default() -> Self {
        Self::ProviderNative
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandCompletion {
    CommandResult,
    #[default]
    TurnCompleted,
    NoOutputAck,
    ProviderIdle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentCommandInput {
    None,
    Text { label: SmolStr, required: bool },
    JsonSchema { schema: serde_json::Value },
}

impl Default for AgentCommandInput {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentCommand {
    pub name: SmolStr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<SmolStr>,
    pub source: AgentCommandSource,
    #[serde(default)]
    pub input: AgentCommandInput,
    #[serde(default)]
    pub completion: AgentCommandCompletion,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentCommandCatalog {
    pub commands: Vec<AgentCommand>,
    pub complete: bool,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentCommandInvocation {
    pub name: SmolStr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<SmolStr>,
    #[serde(default)]
    pub values: serde_json::Value,
    #[serde(default)]
    pub source: AgentCommandSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum AgentCommandResultData {
    Models(AgentModelCatalog),
    Permissions(AgentPermissionCatalog),
    Skills(AgentSkillCatalog),
    Commands(AgentCommandCatalog),
    ForkTargets(AgentForkTargetCatalog),
    Fork(AgentForkResult),
    Status(AgentStatus),
    Text { text: SmolStr },
    Json(serde_json::Value),
    Empty,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentCommandResult {
    pub name: SmolStr,
    pub source: AgentCommandSource,
    pub data: AgentCommandResultData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentModelSelection {
    pub model: SmolStr,
    pub reasoning: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPermissionSelection {
    pub mode: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentForkSelection {
    pub target_id: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentReasoningOption {
    pub value: SmolStr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentModelOption {
    pub id: SmolStr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_reasoning: Vec<AgentReasoningOption>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentModelCatalog {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_model: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_reasoning: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<AgentModelOption>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentPermissionOption {
    pub id: SmolStr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentPermissionCatalog {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_mode: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modes: Vec<AgentPermissionOption>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentTokenUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentContextUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent_used: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_detail: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting_sources: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents_md: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<AgentTokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<AgentContextUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compactions: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentSkillSummary {
    pub name: SmolStr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentSkillCatalog {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<AgentSkillSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentForkResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<SessionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_session_ref: Option<SessionRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentForkTarget {
    pub id: SmolStr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<SmolStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentForkTargetCatalog {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<AgentForkTarget>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenSession {
    pub model: Option<SmolStr>,
    pub cwd: Option<SmolStr>,
    pub initial_input: Option<AgentInput>,
    /// Runtime-level idle process recycle timeout. `None` uses the
    /// runtime default, while `Some(0)` disables automatic recycle for
    /// this session.
    #[serde(default)]
    pub idle_timeout_ms: Option<u64>,
    #[serde(default)]
    pub args: serde_json::Value,
}

impl Default for OpenSession {
    fn default() -> Self {
        Self {
            model: None,
            cwd: None,
            initial_input: None,
            idle_timeout_ms: None,
            args: serde_json::Value::Null,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResumeSession {
    pub session_ref: SessionRef,
    /// Runtime-level idle process recycle timeout. `None` uses the
    /// runtime default, while `Some(0)` disables automatic recycle for
    /// this session.
    #[serde(default)]
    pub idle_timeout_ms: Option<u64>,
    #[serde(default)]
    pub args: serde_json::Value,
}

impl Default for ResumeSession {
    fn default() -> Self {
        Self {
            session_ref: SessionRef::default(),
            idle_timeout_ms: None,
            args: serde_json::Value::Null,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub req_id: SmolStr,
    pub tool_name: SmolStr,
    pub message: Option<SmolStr>,
    pub input: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: SmolStr,
    pub description: Option<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub header: Option<SmolStr>,
    pub text: SmolStr,
    pub options: Vec<QuestionOption>,
    pub multi_select: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnswer {
    pub values: Vec<SmolStr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionResponse {
    pub answers: Vec<QuestionAnswer>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionRequest {
    pub req_id: SmolStr,
    pub questions: Vec<Question>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum InterventionRequest {
    Approval(ApprovalRequest),
    Question(QuestionRequest),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum InterventionResponse {
    Approval(ApprovalDecision),
    Answers(QuestionResponse),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RuntimeCommand {
    Open {
        command_id: CommandId,
        provider_id: ProviderId,
        req: OpenSession,
    },
    Resume {
        command_id: CommandId,
        provider_id: ProviderId,
        req: ResumeSession,
    },
    Submit {
        instance_id: InstanceId,
        input: AgentInput,
    },
    Interrupt {
        instance_id: InstanceId,
    },
    Resolve {
        instance_id: InstanceId,
        req_id: SmolStr,
        response: InterventionResponse,
    },
    Close {
        instance_id: InstanceId,
    },
    UpdateFilter {
        filter: RuntimeBusFilter,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeBusFilter {
    pub session_lifecycle: bool,
    pub user_messages: bool,
    pub assistant_messages: bool,
    pub reasoning: bool,
    pub tool_calls: bool,
    pub tool_results: bool,
    pub usage: bool,
    pub intervention_requests: bool,
    /// Emit [`Event::TurnCompleted`] / [`Event::TurnFailed`] signals.
    /// Defaults to true because these are control-plane markers
    /// consumers need to detect end-of-turn reliably.
    pub turn_lifecycle: bool,
}

impl Default for RuntimeBusFilter {
    fn default() -> Self {
        Self {
            session_lifecycle: true,
            user_messages: false,
            assistant_messages: false,
            reasoning: false,
            tool_calls: false,
            tool_results: false,
            usage: false,
            intervention_requests: false,
            turn_lifecycle: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuntimeBusEvent {
    pub instance_id: InstanceId,
    pub provider_id: ProviderId,
    pub session_id: SessionId,
    pub event: Event,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionOpenedEvent {
    pub command_id: CommandId,
    pub instance_id: InstanceId,
    pub provider_id: ProviderId,
    pub session_id: SessionId,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionClosedEvent {
    pub instance_id: InstanceId,
    pub provider_id: ProviderId,
    pub session_id: SessionId,
    pub reason: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommandRejectedEvent {
    pub command_id: Option<CommandId>,
    pub session_id: Option<SessionId>,
    pub instance_id: Option<InstanceId>,
    pub message: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RuntimeBusOutput {
    SessionOpened(SessionOpenedEvent),
    Event(RuntimeBusEvent),
    SessionClosed(SessionClosedEvent),
    CommandRejected(CommandRejectedEvent),
}
