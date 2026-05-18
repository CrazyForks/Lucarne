use super::state::ControlPlaneState;
use super::types::{LiveInstanceId, ProviderSessionId, Revision, WorkspaceId};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InterventionCallbackToken(SmolStr);

impl InterventionCallbackToken {
    pub fn new(value: impl Into<SmolStr>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterventionCallbackRecord {
    pub token: InterventionCallbackToken,
    pub workspace_id: WorkspaceId,
    pub workspace_revision: Revision,
    pub provider_session_id: Option<ProviderSessionId>,
    pub live_instance_id: LiveInstanceId,
    pub req_id: SmolStr,
    pub action: serde_json::Value,
    pub created_at: SystemTime,
}

impl InterventionCallbackRecord {
    pub fn callback_payload(&self) -> String {
        format!("intv:c:{}", self.token.as_str())
    }
}

impl ControlPlaneState {
    pub fn register_intervention_callback(
        &mut self,
        workspace_id: WorkspaceId,
        live_instance_id: LiveInstanceId,
        req_id: impl Into<SmolStr>,
        action: serde_json::Value,
    ) -> Option<InterventionCallbackRecord> {
        let workspace = self.workspaces.get(&workspace_id)?;
        if workspace.active_live_instance_id.as_ref() != Some(&live_instance_id) {
            return None;
        }
        let live = self.live_instances.get(&live_instance_id)?;
        if workspace.active_provider_session_id.as_ref() != Some(&live.provider_session_id) {
            return None;
        }
        self.next_intervention_callback += 1;
        let record = InterventionCallbackRecord {
            token: InterventionCallbackToken::new(format!("i{}", self.next_intervention_callback)),
            workspace_id,
            workspace_revision: workspace.revision,
            provider_session_id: workspace.active_provider_session_id.clone(),
            live_instance_id,
            req_id: req_id.into(),
            action,
            created_at: SystemTime::now(),
        };
        self.intervention_callbacks
            .insert(record.token.clone(), record.clone());
        Some(record)
    }

    pub fn resolve_intervention_callback(
        &self,
        token: &InterventionCallbackToken,
    ) -> Option<InterventionCallbackRecord> {
        self.intervention_callbacks.get(token).cloned()
    }

    pub fn remove_intervention_callbacks_for_request(
        &mut self,
        live_instance_id: &LiveInstanceId,
        req_id: &str,
    ) -> usize {
        let before = self.intervention_callbacks.len();
        self.intervention_callbacks.retain(|_, callback| {
            callback.live_instance_id != *live_instance_id || callback.req_id.as_str() != req_id
        });
        before - self.intervention_callbacks.len()
    }
}
