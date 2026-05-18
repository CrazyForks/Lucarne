use super::state::{ControlPlaneError, ControlPlaneState};
use super::types::{
    ChannelBinding, LiveInstanceRecord, ProviderSessionRecord, ReconcileOutcome, WorkspaceBinding,
    WorkspaceId,
};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationCheck {
    ProviderSessionProbeRequired,
    ProviderSessionMissing,
    LiveInstanceReady,
    LiveInstanceStale,
    ChannelBindingMissing,
    ChannelTopicMissing,
    ChannelTopicProbeRequired,
    TurnOrphaned,
    PendingPermission,
    PermissionOrphaned,
    ManualAttentionRequired,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivationRequest {
    pub workspace_id: WorkspaceId,
    pub channel: SmolStr,
    pub chat_id: SmolStr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivationPlan {
    pub workspace: WorkspaceBinding,
    pub provider_session: Option<ProviderSessionRecord>,
    pub live_instance: Option<LiveInstanceRecord>,
    pub channel_binding: Option<ChannelBinding>,
    pub reconcile_outcome: ReconcileOutcome,
    pub checks: Vec<ActivationCheck>,
}

impl ActivationPlan {
    pub fn has_check(&self, check: ActivationCheck) -> bool {
        self.checks.contains(&check)
    }

    pub fn requires_topic_creation(&self) -> bool {
        self.has_check(ActivationCheck::ChannelBindingMissing)
            || self.has_check(ActivationCheck::ChannelTopicMissing)
    }

    pub fn requires_topic_probe(&self) -> bool {
        self.has_check(ActivationCheck::ChannelTopicProbeRequired)
    }

    pub fn requires_provider_probe(&self) -> bool {
        self.has_check(ActivationCheck::ProviderSessionProbeRequired)
    }
}

impl ControlPlaneState {
    pub fn plan_activation(
        &self,
        request: ActivationRequest,
    ) -> Result<ActivationPlan, ControlPlaneError> {
        let workspace = self
            .workspaces
            .get(&request.workspace_id)
            .cloned()
            .ok_or_else(|| ControlPlaneError::MissingWorkspace(request.workspace_id.clone()))?;
        let provider_session = workspace
            .active_provider_session_id
            .as_ref()
            .and_then(|id| self.provider_sessions.get(id))
            .cloned();
        let live_instance = workspace
            .active_live_instance_id
            .as_ref()
            .and_then(|id| self.live_instances.get(id))
            .cloned();
        let existing_binding = self
            .channel_bindings
            .values()
            .find(|binding| {
                binding.workspace_id == request.workspace_id
                    && binding.channel == request.channel
                    && binding.chat_id == request.chat_id
            })
            .cloned();

        let mut checks = Vec::new();
        if provider_session
            .as_ref()
            .is_some_and(|session| !session.native_resume_ref.is_empty())
        {
            checks.push(ActivationCheck::ProviderSessionProbeRequired);
        } else if workspace.active_provider_session_id.is_some() {
            checks.push(ActivationCheck::ProviderSessionMissing);
        }
        if let Some(live) = live_instance.as_ref() {
            if matches!(
                live.state,
                super::types::LiveInstanceState::Closed
                    | super::types::LiveInstanceState::Failed
                    | super::types::LiveInstanceState::Stale
            ) {
                checks.push(ActivationCheck::LiveInstanceStale);
            } else if live.state == super::types::LiveInstanceState::WaitingPermission {
                let has_active_turn = live.active_turn_id.as_ref().is_some_and(|turn_id| {
                    self.turns.get(turn_id).is_some_and(|turn| {
                        turn.workspace_id == request.workspace_id
                            && matches!(turn.state, super::types::TurnState::Running)
                    })
                });
                if has_active_turn {
                    checks.push(ActivationCheck::PendingPermission);
                } else {
                    checks.push(ActivationCheck::PermissionOrphaned);
                }
            } else {
                checks.push(ActivationCheck::LiveInstanceReady);
            }
        } else if workspace.active_live_instance_id.is_some() {
            checks.push(ActivationCheck::LiveInstanceStale);
        }

        match existing_binding.as_ref() {
            Some(binding) if binding.topic_id.is_some() => {
                checks.push(ActivationCheck::ChannelTopicProbeRequired);
            }
            Some(_) => checks.push(ActivationCheck::ChannelTopicMissing),
            None => checks.push(ActivationCheck::ChannelBindingMissing),
        }

        if self.turns.values().any(|turn| {
            turn.workspace_id == request.workspace_id
                && matches!(turn.state, super::types::TurnState::Orphaned)
        }) {
            checks.push(ActivationCheck::TurnOrphaned);
        }

        let reconcile_outcome = if checks.iter().any(|check| {
            matches!(
                check,
                ActivationCheck::ChannelBindingMissing | ActivationCheck::ChannelTopicMissing
            )
        }) {
            ReconcileOutcome::TopicMissing
        } else if checks.contains(&ActivationCheck::PermissionOrphaned) {
            ReconcileOutcome::PermissionOrphaned
        } else if checks.contains(&ActivationCheck::TurnOrphaned) {
            ReconcileOutcome::TurnOrphaned
        } else if checks.contains(&ActivationCheck::ProviderSessionMissing) {
            ReconcileOutcome::ProviderSessionStale
        } else if checks.contains(&ActivationCheck::LiveInstanceStale) {
            ReconcileOutcome::LiveInstanceStale
        } else if checks.contains(&ActivationCheck::ProviderSessionProbeRequired) {
            ReconcileOutcome::ProviderSessionProbeRequired
        } else {
            ReconcileOutcome::Ok
        };

        Ok(ActivationPlan {
            workspace,
            provider_session,
            live_instance,
            channel_binding: existing_binding,
            reconcile_outcome,
            checks,
        })
    }
}
