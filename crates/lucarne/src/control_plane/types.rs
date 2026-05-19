use crate::agent_runtime::{AgentContextUsage, AgentStatus, AgentTokenUsage};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default, PartialOrd, Ord,
        )]
        pub struct $name(SmolStr);

        impl $name {
            pub fn new(value: impl Into<SmolStr>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }
    };
}

id_type!(WorkspaceId);
id_type!(ChannelBindingId);
id_type!(PanelRenderId);
id_type!(ProviderSessionId);
id_type!(MessageSessionBindingId);
id_type!(LiveInstanceId);
id_type!(TurnId);
id_type!(CommandId);
id_type!(SubAgentActionId);
id_type!(SubAgentLinkId);
id_type!(ScheduledTaskId);

impl CommandId {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default, PartialOrd, Ord,
)]
pub struct TimelineSeq(u64);

impl TimelineSeq {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }

    pub(crate) fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default, PartialOrd, Ord,
)]
pub struct Revision(u64);

impl Revision {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }

    pub(crate) fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SystemSettings {
    #[serde(default)]
    pub session: SessionDefaultSettings,
    #[serde(default)]
    pub notifications: NotificationSettings,
    #[serde(default)]
    pub workspace: HashMap<PathBuf, ScopedSettingsOverride>,
    #[serde(default)]
    pub provider_session: HashMap<ProviderSessionId, ScopedSettingsOverride>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SessionDefaultSettings {
    #[serde(default)]
    pub force_bypass_permissions: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationSettings {
    #[serde(default = "default_notifications_enabled")]
    pub enabled: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            enabled: default_notifications_enabled(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ScopedSettingsOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_bypass_permissions: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications_enabled: Option<bool>,
}

impl ScopedSettingsOverride {
    pub fn apply_to(&self, settings: &mut EffectiveSettings) {
        if let Some(enabled) = self.force_bypass_permissions {
            settings.session.force_bypass_permissions = enabled;
        }
        if let Some(enabled) = self.notifications_enabled {
            settings.notifications.enabled = enabled;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EffectiveSettings {
    #[serde(default)]
    pub session: SessionDefaultSettings,
    #[serde(default)]
    pub notifications: NotificationSettings,
}

fn default_notifications_enabled() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceBinding {
    pub workspace_id: WorkspaceId,
    pub title: SmolStr,
    pub provider_id: SmolStr,
    pub project_path: PathBuf,
    pub worktree_ref: Option<SmolStr>,
    pub active_provider_session_id: Option<ProviderSessionId>,
    pub active_live_instance_id: Option<LiveInstanceId>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub revision: Revision,
}

impl WorkspaceBinding {
    pub fn new(
        workspace_id: WorkspaceId,
        title: impl Into<SmolStr>,
        provider_id: impl Into<SmolStr>,
        project_path: impl Into<PathBuf>,
    ) -> Self {
        let now = SystemTime::now();
        Self {
            workspace_id,
            title: title.into(),
            provider_id: provider_id.into(),
            project_path: project_path.into(),
            worktree_ref: None,
            active_provider_session_id: None,
            active_live_instance_id: None,
            created_at: now,
            updated_at: now,
            revision: Revision::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelBinding {
    pub channel_binding_id: ChannelBindingId,
    pub workspace_id: WorkspaceId,
    pub channel: SmolStr,
    pub chat_id: SmolStr,
    pub topic_id: Option<SmolStr>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl ChannelBinding {
    pub fn new(
        channel_binding_id: ChannelBindingId,
        workspace_id: WorkspaceId,
        channel: impl Into<SmolStr>,
        chat_id: impl Into<SmolStr>,
        topic_id: Option<impl Into<SmolStr>>,
    ) -> Self {
        let now = SystemTime::now();
        Self {
            channel_binding_id,
            workspace_id,
            channel: channel.into(),
            chat_id: chat_id.into(),
            topic_id: topic_id.map(Into::into),
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelRenderRecord {
    pub panel_id: PanelRenderId,
    pub channel: SmolStr,
    pub chat_id: SmolStr,
    pub message_id: Option<SmolStr>,
    pub last_rendered_revision: Revision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_observed_stale_revision: Option<Revision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reconcile_outcome: Option<ReconcileOutcome>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl PanelRenderRecord {
    pub fn new(
        panel_id: PanelRenderId,
        channel: impl Into<SmolStr>,
        chat_id: impl Into<SmolStr>,
        message_id: Option<impl Into<SmolStr>>,
        last_rendered_revision: Revision,
    ) -> Self {
        let now = SystemTime::now();
        Self {
            panel_id,
            channel: channel.into(),
            chat_id: chat_id.into(),
            message_id: message_id.map(Into::into),
            last_rendered_revision,
            last_observed_stale_revision: None,
            last_reconcile_outcome: Some(ReconcileOutcome::Ok),
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageSessionBinding {
    pub binding_id: MessageSessionBindingId,
    pub channel: SmolStr,
    pub chat_id: SmolStr,
    pub message_id: SmolStr,
    pub provider_session_id: ProviderSessionId,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl MessageSessionBinding {
    pub fn new(
        channel: impl Into<SmolStr>,
        chat_id: impl Into<SmolStr>,
        message_id: impl Into<SmolStr>,
        provider_session_id: ProviderSessionId,
    ) -> Self {
        let channel = channel.into();
        let chat_id = chat_id.into();
        let message_id = message_id.into();
        let now = SystemTime::now();
        Self {
            binding_id: message_session_binding_id(&channel, &chat_id, &message_id),
            channel,
            chat_id,
            message_id,
            provider_session_id,
            created_at: now,
            updated_at: now,
        }
    }
}

pub fn message_session_binding_id(
    channel: &str,
    chat_id: &str,
    message_id: &str,
) -> MessageSessionBindingId {
    MessageSessionBindingId::new(format!("{channel}:{chat_id}:{message_id}"))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderSessionRecord {
    pub provider_session_id: ProviderSessionId,
    pub provider_id: SmolStr,
    pub native_resume_ref: SmolStr,
    pub model: Option<SmolStr>,
    pub reasoning: Option<SmolStr>,
    pub permission_mode: Option<SmolStr>,
    pub status_extra: serde_json::Value,
    pub usage_snapshot: serde_json::Value,
    pub context_snapshot: serde_json::Value,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl ProviderSessionRecord {
    pub fn new(
        provider_session_id: ProviderSessionId,
        provider_id: impl Into<SmolStr>,
        native_resume_ref: impl Into<SmolStr>,
    ) -> Self {
        let now = SystemTime::now();
        Self {
            provider_session_id,
            provider_id: provider_id.into(),
            native_resume_ref: native_resume_ref.into(),
            model: None,
            reasoning: None,
            permission_mode: None,
            status_extra: serde_json::Value::Null,
            usage_snapshot: serde_json::Value::Null,
            context_snapshot: serde_json::Value::Null,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveInstanceState {
    Starting,
    Idle,
    Running,
    WaitingPermission,
    Closing,
    Closed,
    Failed,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveInstanceRecord {
    pub live_instance_id: LiveInstanceId,
    pub provider_id: SmolStr,
    pub provider_session_id: ProviderSessionId,
    pub pid_or_handle: Option<SmolStr>,
    pub state: LiveInstanceState,
    pub last_seen_at: SystemTime,
    pub active_turn_id: Option<TurnId>,
    pub close_reason: Option<SmolStr>,
}

impl LiveInstanceRecord {
    pub fn new(
        live_instance_id: LiveInstanceId,
        provider_id: impl Into<SmolStr>,
        provider_session_id: ProviderSessionId,
        pid_or_handle: Option<impl Into<SmolStr>>,
    ) -> Self {
        Self {
            live_instance_id,
            provider_id: provider_id.into(),
            provider_session_id,
            pid_or_handle: pid_or_handle.map(Into::into),
            state: LiveInstanceState::Idle,
            last_seen_at: SystemTime::now(),
            active_turn_id: None,
            close_reason: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnSource {
    UserMessage,
    Command,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    Queued,
    Submitted,
    Running,
    Completed,
    Failed,
    Canceled,
    Orphaned,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnRecord {
    pub turn_id: TurnId,
    pub workspace_id: WorkspaceId,
    pub provider_session_id: ProviderSessionId,
    pub live_instance_id: LiveInstanceId,
    pub source: TurnSource,
    pub input: SmolStr,
    pub reply_to_channel_message_id: Option<i64>,
    pub state: TurnState,
    pub timeline_seq_start: Option<TimelineSeq>,
    pub timeline_seq_end: Option<TimelineSeq>,
    pub usage: serde_json::Value,
    pub created_at: SystemTime,
    pub completed_at: Option<SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineItemKind {
    User,
    Assistant,
    Attachment,
    Reasoning,
    ToolCall,
    ToolResult,
    Permission,
    CommandResult,
    Usage,
    Status,
    Error,
    Compaction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineItem {
    pub workspace_id: WorkspaceId,
    pub turn_id: TurnId,
    pub seq: TimelineSeq,
    pub epoch: u64,
    pub provider_item_id: Option<SmolStr>,
    pub kind: TimelineItemKind,
    pub payload: serde_json::Value,
    pub created_at: SystemTime,
}

impl TimelineItem {
    pub fn new(
        workspace_id: WorkspaceId,
        turn_id: TurnId,
        kind: TimelineItemKind,
        payload: impl Into<serde_json::Value>,
    ) -> Self {
        Self {
            workspace_id,
            turn_id,
            seq: TimelineSeq::default(),
            epoch: 0,
            provider_item_id: None,
            kind,
            payload: payload.into(),
            created_at: SystemTime::now(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandCompletionPolicy {
    CommandResult,
    TurnCompleted,
    NoOutputAck,
    ProviderIdle,
    ManualError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandState {
    Pending,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandWorkflow {
    pub command_id: CommandId,
    pub workspace_id: WorkspaceId,
    pub turn_id: TurnId,
    pub name: SmolStr,
    pub args: Option<SmolStr>,
    pub values: serde_json::Value,
    pub catalog_revision: Revision,
    pub completion_policy: CommandCompletionPolicy,
    pub state: CommandState,
    pub result: Option<serde_json::Value>,
    pub error: Option<SmolStr>,
    pub created_at: SystemTime,
    pub completed_at: Option<SystemTime>,
}

impl CommandWorkflow {
    pub fn new(
        workspace_id: WorkspaceId,
        turn_id: TurnId,
        name: impl Into<SmolStr>,
        args: Option<SmolStr>,
        values: serde_json::Value,
        catalog_revision: Revision,
        completion_policy: CommandCompletionPolicy,
    ) -> Self {
        Self {
            command_id: CommandId::default(),
            workspace_id,
            turn_id,
            name: name.into(),
            args,
            values,
            catalog_revision,
            completion_policy,
            state: CommandState::Pending,
            result: None,
            error: None,
            created_at: SystemTime::now(),
            completed_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledTaskRecord {
    pub task_id: ScheduledTaskId,
    pub workspace_id: WorkspaceId,
    pub provider_id: SmolStr,
    pub project_path: PathBuf,
    pub title: SmolStr,
    pub prompt: SmolStr,
    pub next_run_unix_ms: u64,
    pub enabled: bool,
    pub last_run_unix_ms: Option<u64>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl ScheduledTaskRecord {
    pub fn new(
        task_id: ScheduledTaskId,
        workspace_id: WorkspaceId,
        provider_id: impl Into<SmolStr>,
        project_path: impl Into<PathBuf>,
        title: impl Into<SmolStr>,
        prompt: impl Into<SmolStr>,
        next_run_unix_ms: u64,
    ) -> Self {
        let now = SystemTime::now();
        Self {
            task_id,
            workspace_id,
            provider_id: provider_id.into(),
            project_path: project_path.into(),
            title: title.into(),
            prompt: prompt.into(),
            next_run_unix_ms,
            enabled: true,
            last_run_unix_ms: None,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StatusSnapshot {
    pub workspace_id: Option<WorkspaceId>,
    pub provider_id: Option<SmolStr>,
    pub provider_version: Option<SmolStr>,
    pub provider_session_id: Option<ProviderSessionId>,
    pub native_resume_ref: Option<SmolStr>,
    pub live_instance_id: Option<LiveInstanceId>,
    pub live_instance_state: Option<LiveInstanceState>,
    pub channel_binding_id: Option<ChannelBindingId>,
    pub channel: Option<SmolStr>,
    pub chat_id: Option<SmolStr>,
    pub topic_id: Option<SmolStr>,
    pub directory: Option<SmolStr>,
    pub project_path: Option<PathBuf>,
    pub worktree_ref: Option<SmolStr>,
    pub model: Option<SmolStr>,
    pub model_detail: Option<SmolStr>,
    pub reasoning: Option<SmolStr>,
    pub permission_mode: Option<SmolStr>,
    pub account: Option<SmolStr>,
    pub base_url: Option<SmolStr>,
    pub proxy: Option<SmolStr>,
    pub setting_sources: Option<SmolStr>,
    pub agents_md: Option<SmolStr>,
    pub token_usage: Option<AgentTokenUsage>,
    pub context_usage: Option<AgentContextUsage>,
    pub compactions: Option<u64>,
    pub usage_snapshot: Option<serde_json::Value>,
    pub context_snapshot: Option<serde_json::Value>,
    pub provider_status: Option<AgentStatus>,
    pub channel_binding_state: Option<SmolStr>,
    pub last_reconcile_outcome: Option<ReconcileOutcome>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconcileOutcome {
    Ok,
    StaleRevision,
    TopicMissing,
    TopicMissingRecreated,
    ProviderSessionProbeRequired,
    ProviderSessionStale,
    LiveInstanceStale,
    TurnOrphaned,
    PermissionOrphaned,
    ManualAttentionRequired,
}
