use super::types::{
    ChannelBinding, LiveInstanceRecord, ProviderSessionRecord, ReconcileOutcome, StatusSnapshot,
    WorkspaceBinding,
};
use crate::agent_runtime::AgentStatus;

pub fn build_status_snapshot(
    workspace: Option<&WorkspaceBinding>,
    provider_session: Option<&ProviderSessionRecord>,
    live_instance: Option<&LiveInstanceRecord>,
    channel_binding: Option<&ChannelBinding>,
    last_reconcile_outcome: Option<ReconcileOutcome>,
) -> StatusSnapshot {
    let provider_status = provider_session
        .and_then(|session| serde_json::from_value(session.status_extra.clone()).ok());

    StatusSnapshot {
        workspace_id: workspace.map(|workspace| workspace.workspace_id.clone()),
        provider_id: workspace
            .map(|workspace| workspace.provider_id.clone())
            .or_else(|| provider_session.map(|session| session.provider_id.clone())),
        provider_version: provider_status
            .as_ref()
            .and_then(|status: &AgentStatus| status.version.clone()),
        provider_session_id: provider_session.map(|session| session.provider_session_id.clone()),
        native_resume_ref: provider_session.map(|session| session.native_resume_ref.clone()),
        live_instance_id: live_instance.map(|live| live.live_instance_id.clone()),
        live_instance_state: live_instance.map(|live| live.state),
        channel_binding_id: channel_binding.map(|binding| binding.channel_binding_id.clone()),
        channel: channel_binding.map(|binding| binding.channel.clone()),
        chat_id: channel_binding.map(|binding| binding.chat_id.clone()),
        topic_id: channel_binding.and_then(|binding| binding.topic_id.clone()),
        directory: provider_status
            .as_ref()
            .and_then(|status| status.directory.clone()),
        project_path: workspace.map(|workspace| workspace.project_path.clone()),
        worktree_ref: workspace.and_then(|workspace| workspace.worktree_ref.clone()),
        model: provider_session
            .and_then(|session| session.model.clone())
            .or_else(|| {
                provider_status
                    .as_ref()
                    .and_then(|status| status.model.clone())
            }),
        model_detail: provider_status
            .as_ref()
            .and_then(|status| status.model_detail.clone()),
        reasoning: provider_session
            .and_then(|session| session.reasoning.clone())
            .or_else(|| {
                provider_status
                    .as_ref()
                    .and_then(|status| status.reasoning.clone())
            }),
        permission_mode: provider_session.and_then(|session| session.permission_mode.clone()),
        account: provider_status
            .as_ref()
            .and_then(|status| status.account.clone()),
        base_url: provider_status
            .as_ref()
            .and_then(|status| status.base_url.clone()),
        proxy: provider_status
            .as_ref()
            .and_then(|status| status.proxy.clone()),
        setting_sources: provider_status
            .as_ref()
            .and_then(|status| status.setting_sources.clone()),
        agents_md: provider_status
            .as_ref()
            .and_then(|status| status.agents_md.clone()),
        token_usage: provider_status
            .as_ref()
            .and_then(|status| status.tokens.clone()),
        context_usage: provider_status
            .as_ref()
            .and_then(|status| status.context.clone()),
        compactions: provider_status
            .as_ref()
            .and_then(|status| status.compactions),
        usage_snapshot: provider_session.map(|session| session.usage_snapshot.clone()),
        context_snapshot: provider_session.map(|session| session.context_snapshot.clone()),
        provider_status,
        channel_binding_state: channel_binding.map(|binding| {
            if let Some(topic_id) = binding.topic_id.as_ref() {
                format!("{}:{}:{}", binding.channel, binding.chat_id, topic_id).into()
            } else {
                "missing_topic".into()
            }
        }),
        last_reconcile_outcome,
    }
}
