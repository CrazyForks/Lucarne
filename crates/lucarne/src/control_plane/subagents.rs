use super::state::ControlPlaneState;
use super::types::{
    ProviderSessionId, Revision, SubAgentActionId, SubAgentLinkId, TurnId, WorkspaceId,
};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubAgentCallbackToken(SmolStr);

impl SubAgentCallbackToken {
    pub fn new(value: impl Into<SmolStr>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubAgentCallbackRecord {
    pub token: SubAgentCallbackToken,
    pub workspace_id: WorkspaceId,
    pub workspace_revision: Revision,
    pub link_id: SubAgentLinkId,
    pub link_revision: Revision,
    pub created_at: SystemTime,
}

impl SubAgentCallbackRecord {
    pub fn callback_payload(&self) -> String {
        format!("subagent:c:{}", self.token.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentState {
    Starting,
    Running,
    Waiting,
    Completed,
    Failed,
    Stopped,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubAgentActionRecord {
    pub action_id: SubAgentActionId,
    pub workspace_id: WorkspaceId,
    pub parent_turn_id: TurnId,
    pub parent_provider_session_id: ProviderSessionId,
    pub provider_item_id: Option<SmolStr>,
    pub tool_name: SmolStr,
    pub prompt: Option<SmolStr>,
    pub requested_model: Option<SmolStr>,
    pub child_provider_session_id: Option<ProviderSessionId>,
    pub child_native_ref: Option<SmolStr>,
    pub state: SubAgentState,
    pub summary: Option<SmolStr>,
    pub raw: serde_json::Value,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl SubAgentActionRecord {
    pub fn new(
        workspace_id: WorkspaceId,
        parent_turn_id: TurnId,
        parent_provider_session_id: ProviderSessionId,
        tool_name: impl Into<SmolStr>,
    ) -> Self {
        Self {
            action_id: SubAgentActionId::default(),
            workspace_id,
            parent_turn_id,
            parent_provider_session_id,
            provider_item_id: None,
            tool_name: tool_name.into(),
            prompt: None,
            requested_model: None,
            child_provider_session_id: None,
            child_native_ref: None,
            state: SubAgentState::Running,
            summary: None,
            raw: serde_json::Value::Null,
            created_at: SystemTime::now(),
            updated_at: SystemTime::now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubAgentLinkRecord {
    pub link_id: SubAgentLinkId,
    pub workspace_id: WorkspaceId,
    pub revision: Revision,
    pub action_id: SubAgentActionId,
    pub parent_turn_id: TurnId,
    pub parent_provider_session_id: ProviderSessionId,
    pub child_provider_session_id: Option<ProviderSessionId>,
    pub child_native_ref: Option<SmolStr>,
    pub child_workspace_id: Option<WorkspaceId>,
    pub label: Option<SmolStr>,
    pub agent_id: Option<SmolStr>,
    pub nickname: Option<SmolStr>,
    pub role: Option<SmolStr>,
    pub model: Option<SmolStr>,
    pub prompt: Option<SmolStr>,
    pub last_message: Option<SmolStr>,
    pub openable: bool,
    pub state: SubAgentState,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl SubAgentLinkRecord {
    pub fn new_openable_ref(
        link_id: SubAgentLinkId,
        workspace_id: WorkspaceId,
        action_id: SubAgentActionId,
        parent_turn_id: TurnId,
        parent_provider_session_id: ProviderSessionId,
        child_provider_session_id: Option<ProviderSessionId>,
        child_native_ref: Option<SmolStr>,
    ) -> Self {
        let now = SystemTime::now();
        let openable = child_provider_session_id.is_some() || child_native_ref.is_some();
        Self {
            link_id,
            workspace_id,
            revision: Revision::default(),
            action_id,
            parent_turn_id,
            parent_provider_session_id,
            child_provider_session_id,
            child_native_ref,
            child_workspace_id: None,
            label: None,
            agent_id: None,
            nickname: None,
            role: None,
            model: None,
            prompt: None,
            last_message: None,
            openable,
            state: if openable {
                SubAgentState::Running
            } else {
                SubAgentState::Unsupported
            },
            created_at: now,
            updated_at: now,
        }
    }

    pub fn new_openable(
        link_id: SubAgentLinkId,
        workspace_id: WorkspaceId,
        action_id: SubAgentActionId,
        parent_turn_id: TurnId,
        parent_provider_session_id: ProviderSessionId,
        child_provider_session_id: ProviderSessionId,
        child_native_ref: impl Into<SmolStr>,
    ) -> Self {
        Self::new_openable_ref(
            link_id,
            workspace_id,
            action_id,
            parent_turn_id,
            parent_provider_session_id,
            Some(child_provider_session_id),
            Some(child_native_ref.into()),
        )
    }

    pub fn new_non_openable(
        link_id: SubAgentLinkId,
        workspace_id: WorkspaceId,
        action_id: SubAgentActionId,
        parent_turn_id: TurnId,
        parent_provider_session_id: ProviderSessionId,
        label: impl Into<SmolStr>,
    ) -> Self {
        let now = SystemTime::now();
        Self {
            link_id,
            workspace_id,
            revision: Revision::default(),
            action_id,
            parent_turn_id,
            parent_provider_session_id,
            child_provider_session_id: None,
            child_native_ref: None,
            child_workspace_id: None,
            label: Some(label.into()),
            agent_id: None,
            nickname: None,
            role: None,
            model: None,
            prompt: None,
            last_message: None,
            openable: false,
            state: SubAgentState::Unsupported,
            created_at: now,
            updated_at: now,
        }
    }
}

impl ControlPlaneState {
    pub fn record_subagent_action(
        &mut self,
        mut action: SubAgentActionRecord,
    ) -> SubAgentActionRecord {
        if action.action_id.as_str().is_empty() {
            self.next_subagent_action += 1;
            action.action_id =
                SubAgentActionId::new(format!("subagent-action-{}", self.next_subagent_action));
        }
        let now = SystemTime::now();
        action.created_at = now;
        action.updated_at = now;
        self.subagent_actions
            .insert(action.action_id.clone(), action.clone());
        action
    }

    pub fn upsert_subagent_link(&mut self, mut link: SubAgentLinkRecord) -> SubAgentLinkRecord {
        if let Some(existing) = self.subagent_links.get(&link.link_id) {
            link.created_at = existing.created_at;
            link.revision = existing.revision.next();
        } else {
            link.revision = Revision::new(1);
        }
        link.updated_at = SystemTime::now();
        self.subagent_links
            .insert(link.link_id.clone(), link.clone());
        link
    }

    pub fn subagent_links_for_turn(&self, turn_id: &TurnId) -> Vec<SubAgentLinkRecord> {
        self.subagent_links
            .values()
            .filter(|link| &link.parent_turn_id == turn_id)
            .cloned()
            .collect()
    }

    pub fn subagent_action(&self, action_id: &SubAgentActionId) -> Option<SubAgentActionRecord> {
        self.subagent_actions.get(action_id).cloned()
    }

    pub fn subagent_links_for_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Vec<SubAgentLinkRecord> {
        self.subagent_links
            .values()
            .filter(|link| &link.workspace_id == workspace_id)
            .cloned()
            .collect()
    }

    pub fn openable_subagent_link(&self, link_id: &SubAgentLinkId) -> Option<SubAgentLinkRecord> {
        self.subagent_links
            .get(link_id)
            .filter(|link| link.openable)
            .cloned()
    }

    pub fn attach_subagent_child_workspace(
        &mut self,
        link_id: &SubAgentLinkId,
        child_workspace_id: WorkspaceId,
    ) -> Option<SubAgentLinkRecord> {
        let link = self.subagent_links.get_mut(link_id)?;
        if !link.openable {
            return None;
        }
        link.child_workspace_id = Some(child_workspace_id);
        link.updated_at = SystemTime::now();
        Some(link.clone())
    }

    pub fn register_subagent_callback(
        &mut self,
        workspace_id: WorkspaceId,
        link_id: SubAgentLinkId,
    ) -> Option<SubAgentCallbackRecord> {
        let workspace_revision = self.workspaces.get(&workspace_id)?.revision;
        let link_revision = self.subagent_links.get(&link_id)?.revision;
        self.next_subagent_callback += 1;
        let record = SubAgentCallbackRecord {
            token: SubAgentCallbackToken::new(format!("s{}", self.next_subagent_callback)),
            workspace_id,
            workspace_revision,
            link_id,
            link_revision,
            created_at: SystemTime::now(),
        };
        self.subagent_callbacks
            .insert(record.token.clone(), record.clone());
        Some(record)
    }

    pub fn resolve_subagent_callback(
        &self,
        token: &SubAgentCallbackToken,
    ) -> Option<SubAgentCallbackRecord> {
        self.subagent_callbacks.get(token).cloned()
    }
}
