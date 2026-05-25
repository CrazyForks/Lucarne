use super::types::*;
use super::ControlPlaneStoreError;
use crate::agent_runtime::AgentStatus;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tracing::debug;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlPlaneError {
    MissingWorkspace(WorkspaceId),
    MissingProviderSession(ProviderSessionId),
    MissingLiveInstance(LiveInstanceId),
    MissingTurn(TurnId),
    MissingCommand(CommandId),
    ProviderMismatch {
        expected: SmolStr,
        actual: SmolStr,
    },
    LiveProviderSessionMismatch {
        live_instance_id: LiveInstanceId,
        expected: ProviderSessionId,
        actual: ProviderSessionId,
    },
    WorkspaceActiveBindingMismatch {
        workspace_id: WorkspaceId,
        live_instance_id: LiveInstanceId,
    },
    NonResumableFork {
        fork_workspace_id: WorkspaceId,
    },
    LiveInstanceUnavailable {
        live_instance_id: LiveInstanceId,
        state: LiveInstanceState,
    },
    LiveInstanceAlreadyRunning {
        live_instance_id: LiveInstanceId,
        active_turn_id: TurnId,
    },
    TurnWorkspaceMismatch {
        turn_id: TurnId,
        expected: WorkspaceId,
        actual: WorkspaceId,
    },
    StaleRevision {
        current: Revision,
        observed: Revision,
    },
    CommandCompletionPolicyMismatch {
        policy: CommandCompletionPolicy,
    },
}

#[derive(Debug, Clone)]
pub struct ControlPlaneState {
    pub(super) system_settings: SystemSettings,
    pub(super) workspaces: HashMap<WorkspaceId, WorkspaceBinding>,
    pub(super) channel_bindings: HashMap<ChannelBindingId, ChannelBinding>,
    pub(super) panel_renders: HashMap<PanelRenderId, PanelRenderRecord>,
    pub(super) message_session_bindings: HashMap<MessageSessionBindingId, MessageSessionBinding>,
    pub(super) provider_sessions: HashMap<ProviderSessionId, ProviderSessionRecord>,
    pub(super) live_instances: HashMap<LiveInstanceId, LiveInstanceRecord>,
    live_instance_workspaces: HashMap<LiveInstanceId, WorkspaceId>,
    pub(super) turns: HashMap<TurnId, TurnRecord>,
    commands: HashMap<CommandId, CommandWorkflow>,
    pub(super) command_callbacks:
        HashMap<super::commands::CommandCallbackToken, super::commands::CommandCallbackRecord>,
    pub(super) intervention_callbacks: HashMap<
        super::interventions::InterventionCallbackToken,
        super::interventions::InterventionCallbackRecord,
    >,
    pub(super) subagent_actions: HashMap<SubAgentActionId, super::subagents::SubAgentActionRecord>,
    pub(super) subagent_links: HashMap<SubAgentLinkId, super::subagents::SubAgentLinkRecord>,
    pub(super) subagent_callbacks:
        HashMap<super::subagents::SubAgentCallbackToken, super::subagents::SubAgentCallbackRecord>,
    pub(super) scheduled_tasks: HashMap<ScheduledTaskId, ScheduledTaskRecord>,
    pub(super) history_replays: HashMap<WorkspaceId, super::history_replay::HistoryReplayRecord>,
    pub(super) history_older_callbacks: HashMap<
        super::history_replay::HistoryOlderCallbackToken,
        super::history_replay::HistoryOlderCallbackRecord,
    >,
    last_reconcile_by_workspace: HashMap<WorkspaceId, ReconcileOutcome>,
    timeline: Vec<TimelineItem>,
    next_turn: u64,
    next_command: u64,
    pub(super) next_command_callback: u64,
    pub(super) next_intervention_callback: u64,
    pub(super) next_subagent_action: u64,
    pub(super) next_subagent_callback: u64,
    pub(super) next_history_older_callback: u64,
    next_timeline_by_workspace: HashMap<WorkspaceId, TimelineSeq>,

    // Lazy timeline loading: timeline items are loaded per-workspace on first access.
    #[allow(dead_code)]
    timeline_conn: Option<Arc<Mutex<rusqlite::Connection>>>,
    loaded_timeline_workspaces: HashSet<WorkspaceId>,
}

impl Default for ControlPlaneState {
    fn default() -> Self {
        Self {
            system_settings: SystemSettings::default(),
            workspaces: HashMap::default(),
            channel_bindings: HashMap::default(),
            panel_renders: HashMap::default(),
            message_session_bindings: HashMap::default(),
            provider_sessions: HashMap::default(),
            live_instances: HashMap::default(),
            live_instance_workspaces: HashMap::default(),
            turns: HashMap::default(),
            commands: HashMap::default(),
            command_callbacks: HashMap::default(),
            intervention_callbacks: HashMap::default(),
            subagent_actions: HashMap::default(),
            subagent_links: HashMap::default(),
            subagent_callbacks: HashMap::default(),
            scheduled_tasks: HashMap::default(),
            history_replays: HashMap::default(),
            history_older_callbacks: HashMap::default(),
            last_reconcile_by_workspace: HashMap::default(),
            timeline: Vec::default(),
            next_turn: 0,
            next_command: 0,
            next_command_callback: 0,
            next_intervention_callback: 0,
            next_subagent_action: 0,
            next_subagent_callback: 0,
            next_history_older_callback: 0,
            next_timeline_by_workspace: HashMap::default(),
            timeline_conn: None,
            loaded_timeline_workspaces: HashSet::default(),
        }
    }
}

const SYSTEM_SETTINGS_ENTITY_ID: &str = "system";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlPlanePersistenceEntity {
    pub kind: String,
    pub entity_id: String,
    pub workspace_id: Option<String>,
    pub state: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ControlPlanePersistenceMeta {
    next_turn: u64,
    next_command: u64,
    next_command_callback: u64,
    next_intervention_callback: u64,
    next_subagent_action: u64,
    next_subagent_callback: u64,
    #[serde(default)]
    next_history_older_callback: u64,
    next_timeline_by_workspace: HashMap<WorkspaceId, TimelineSeq>,
}

fn persistence_entity(
    kind: impl Into<String>,
    workspace_id: Option<&str>,
    entity_id: impl Into<String>,
    state: impl Serialize,
) -> ControlPlanePersistenceEntity {
    ControlPlanePersistenceEntity {
        kind: kind.into(),
        entity_id: entity_id.into(),
        workspace_id: workspace_id.map(str::to_string),
        state: serde_json::to_value(state)
            .expect("control-plane persistence entity must serialize"),
    }
}

fn timeline_index_item(item: &TimelineItem) -> TimelineItem {
    let mut item = item.clone();
    item.payload = serde_json::Value::Null;
    item
}

fn workspace_for_live<'a>(
    state: &'a ControlPlaneState,
    live_instance_id: &LiveInstanceId,
) -> Option<&'a WorkspaceId> {
    state
        .live_instance_workspaces
        .get(live_instance_id)
        .or_else(|| {
            state
                .workspaces
                .values()
                .find(|workspace| {
                    workspace.active_live_instance_id.as_ref() == Some(live_instance_id)
                })
                .map(|workspace| &workspace.workspace_id)
        })
}

impl ControlPlaneState {
    pub fn persistence_entities(&self) -> Vec<ControlPlanePersistenceEntity> {
        self.persistence_entities_without_timeline()
    }

    pub fn persistence_entities_without_timeline(&self) -> Vec<ControlPlanePersistenceEntity> {
        self.persistence_entities_for_snapshot()
    }

    pub fn persistence_entities_for_system_settings(&self) -> Vec<ControlPlanePersistenceEntity> {
        vec![persistence_entity(
            "system_settings",
            None,
            SYSTEM_SETTINGS_ENTITY_ID,
            &self.system_settings,
        )]
    }

    pub fn persistence_entities_for_message_session_binding(
        binding: &MessageSessionBinding,
    ) -> Vec<ControlPlanePersistenceEntity> {
        vec![persistence_entity(
            "message_session_binding",
            None,
            binding.binding_id.as_str(),
            binding,
        )]
    }

    pub fn persistence_entities_for_workspace(
        workspace: &WorkspaceBinding,
    ) -> Vec<ControlPlanePersistenceEntity> {
        vec![persistence_entity(
            "workspace",
            Some(workspace.workspace_id.as_str()),
            workspace.workspace_id.as_str(),
            workspace,
        )]
    }

    pub fn persistence_entities_for_channel_binding(
        binding: &ChannelBinding,
    ) -> Vec<ControlPlanePersistenceEntity> {
        vec![persistence_entity(
            "channel_binding",
            Some(binding.workspace_id.as_str()),
            binding.channel_binding_id.as_str(),
            binding,
        )]
    }

    pub fn persistence_entities_for_provider_session(
        session: &ProviderSessionRecord,
    ) -> Vec<ControlPlanePersistenceEntity> {
        vec![persistence_entity(
            "provider_session",
            None,
            session.provider_session_id.as_str(),
            session,
        )]
    }

    pub fn persistence_entities_for_live_instance(
        live: &LiveInstanceRecord,
        workspace_id: Option<&WorkspaceId>,
    ) -> Vec<ControlPlanePersistenceEntity> {
        vec![persistence_entity(
            "live_instance",
            workspace_id.map(WorkspaceId::as_str),
            live.live_instance_id.as_str(),
            live,
        )]
    }

    pub fn persistence_entities_for_turn_record(
        turn: &TurnRecord,
    ) -> Vec<ControlPlanePersistenceEntity> {
        vec![persistence_entity(
            "turn",
            Some(turn.workspace_id.as_str()),
            turn.turn_id.as_str(),
            turn,
        )]
    }

    pub fn persistence_entities_for_reconcile_outcome(
        workspace_id: &WorkspaceId,
        outcome: ReconcileOutcome,
    ) -> Vec<ControlPlanePersistenceEntity> {
        vec![persistence_entity(
            "reconcile_outcome",
            Some(workspace_id.as_str()),
            workspace_id.as_str(),
            outcome,
        )]
    }

    pub fn persistence_entities_for_panel_render(
        panel: &PanelRenderRecord,
    ) -> Vec<ControlPlanePersistenceEntity> {
        vec![persistence_entity(
            "panel_render",
            None,
            panel.panel_id.as_str(),
            panel,
        )]
    }

    fn persistence_meta_entity(&self) -> ControlPlanePersistenceEntity {
        persistence_entity(
            "meta",
            None,
            "control-plane",
            ControlPlanePersistenceMeta {
                next_turn: self.next_turn,
                next_command: self.next_command,
                next_command_callback: self.next_command_callback,
                next_intervention_callback: self.next_intervention_callback,
                next_subagent_action: self.next_subagent_action,
                next_subagent_callback: self.next_subagent_callback,
                next_history_older_callback: self.next_history_older_callback,
                next_timeline_by_workspace: self.next_timeline_by_workspace.clone(),
            },
        )
    }

    fn persistence_entities_for_snapshot(&self) -> Vec<ControlPlanePersistenceEntity> {
        vec![
            self.persistence_meta_entity(),
            persistence_entity(
                "system_settings",
                None,
                SYSTEM_SETTINGS_ENTITY_ID,
                &self.system_settings,
            ),
        ]
    }

    pub fn persistence_entities_for_timeline_item(
        &self,
        item: &TimelineItem,
    ) -> Vec<ControlPlanePersistenceEntity> {
        let mut entities = Vec::new();
        entities.push(self.persistence_meta_entity());
        if let Some(turn) = self.turns.get(&item.turn_id) {
            entities.push(persistence_entity(
                "turn",
                Some(turn.workspace_id.as_str()),
                turn.turn_id.as_str(),
                turn,
            ));
        }
        entities.push(persistence_entity(
            "timeline",
            Some(item.workspace_id.as_str()),
            format!("{}:{}", item.workspace_id.as_str(), item.seq.get()),
            item,
        ));
        entities
    }

    pub fn persistence_entities_for_turn_lifecycle(
        &self,
        turn_id: &TurnId,
    ) -> Vec<ControlPlanePersistenceEntity> {
        let mut entities = Vec::new();
        entities.push(self.persistence_meta_entity());
        if let Some(turn) = self.turns.get(turn_id) {
            entities.push(persistence_entity(
                "turn",
                Some(turn.workspace_id.as_str()),
                turn.turn_id.as_str(),
                turn,
            ));
            if let Some(live) = self.live_instances.get(&turn.live_instance_id) {
                entities.push(persistence_entity(
                    "live_instance",
                    workspace_for_live(self, &live.live_instance_id).map(WorkspaceId::as_str),
                    live.live_instance_id.as_str(),
                    live,
                ));
            }
        }
        entities
    }

    pub(super) fn apply_persistence_entity_json(
        &mut self,
        kind: &str,
        workspace_id: Option<&str>,
        state_json: &[u8],
    ) -> Result<(), serde_json::Error> {
        match kind {
            "meta" => {
                let meta = serde_json::from_slice::<ControlPlanePersistenceMeta>(state_json)?;
                self.apply_persistence_meta(meta);
            }
            "workspace" => {
                let record = serde_json::from_slice::<WorkspaceBinding>(state_json)?;
                self.workspaces.insert(record.workspace_id.clone(), record);
            }
            "system_settings" => {
                self.system_settings = serde_json::from_slice::<SystemSettings>(state_json)?;
            }
            "channel_binding" => {
                let record = serde_json::from_slice::<ChannelBinding>(state_json)?;
                self.channel_bindings
                    .insert(record.channel_binding_id.clone(), record);
            }
            "panel_render" => {
                let record = serde_json::from_slice::<PanelRenderRecord>(state_json)?;
                self.panel_renders.insert(record.panel_id.clone(), record);
            }
            "message_session_binding" => {
                let record = serde_json::from_slice::<MessageSessionBinding>(state_json)?;
                self.message_session_bindings
                    .insert(record.binding_id.clone(), record);
            }
            "provider_session" => {
                let record = serde_json::from_slice::<ProviderSessionRecord>(state_json)?;
                self.provider_sessions
                    .insert(record.provider_session_id.clone(), record);
            }
            "live_instance" => {
                let record = serde_json::from_slice::<LiveInstanceRecord>(state_json)?;
                if let Some(workspace_id) = workspace_id {
                    self.live_instance_workspaces.insert(
                        record.live_instance_id.clone(),
                        WorkspaceId::new(workspace_id),
                    );
                }
                self.live_instances
                    .insert(record.live_instance_id.clone(), record);
            }
            "turn" => {
                let record = serde_json::from_slice::<TurnRecord>(state_json)?;
                self.turns.insert(record.turn_id.clone(), record);
            }
            "command" => {
                let record = serde_json::from_slice::<CommandWorkflow>(state_json)?;
                self.commands.insert(record.command_id.clone(), record);
            }
            "command_callback" => {
                let record =
                    serde_json::from_slice::<super::commands::CommandCallbackRecord>(state_json)?;
                self.command_callbacks.insert(record.token.clone(), record);
            }
            "intervention_callback" => {
                let record = serde_json::from_slice::<
                    super::interventions::InterventionCallbackRecord,
                >(state_json)?;
                self.intervention_callbacks
                    .insert(record.token.clone(), record);
            }
            "subagent_action" => {
                let record =
                    serde_json::from_slice::<super::subagents::SubAgentActionRecord>(state_json)?;
                self.subagent_actions
                    .insert(record.action_id.clone(), record);
            }
            "subagent_link" => {
                let record =
                    serde_json::from_slice::<super::subagents::SubAgentLinkRecord>(state_json)?;
                self.subagent_links.insert(record.link_id.clone(), record);
            }
            "subagent_callback" => {
                let record =
                    serde_json::from_slice::<super::subagents::SubAgentCallbackRecord>(state_json)?;
                self.subagent_callbacks.insert(record.token.clone(), record);
            }
            "scheduled_task" => {
                let record = serde_json::from_slice::<ScheduledTaskRecord>(state_json)?;
                self.scheduled_tasks.insert(record.task_id.clone(), record);
            }
            "history_replay" => {
                let record = serde_json::from_slice::<super::history_replay::HistoryReplayRecord>(
                    state_json,
                )?;
                self.history_replays
                    .insert(record.workspace_id.clone(), record);
            }
            "history_older_callback" => {
                let record = serde_json::from_slice::<
                    super::history_replay::HistoryOlderCallbackRecord,
                >(state_json)?;
                self.history_older_callbacks
                    .insert(record.token.clone(), record);
            }
            "reconcile_outcome" => {
                if let Some(workspace_id) = workspace_id {
                    let outcome = serde_json::from_slice::<ReconcileOutcome>(state_json)?;
                    self.last_reconcile_by_workspace
                        .insert(WorkspaceId::new(workspace_id), outcome);
                }
            }
            "timeline" => {
                // Timeline items are loaded lazily per workspace.
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_persistence_meta(&mut self, meta: ControlPlanePersistenceMeta) {
        self.next_turn = meta.next_turn;
        self.next_command = meta.next_command;
        self.next_command_callback = meta.next_command_callback;
        self.next_intervention_callback = meta.next_intervention_callback;
        self.next_subagent_action = meta.next_subagent_action;
        self.next_subagent_callback = meta.next_subagent_callback;
        self.next_history_older_callback = meta.next_history_older_callback;
        self.next_timeline_by_workspace = meta.next_timeline_by_workspace;
    }

    pub fn from_persistence_entities(
        entities: Vec<ControlPlanePersistenceEntity>,
    ) -> Result<Self, serde_json::Error> {
        let mut state = Self::default();
        let mut meta = None;
        for entity in entities {
            match entity.kind.as_str() {
                "meta" => {
                    meta = Some(serde_json::from_value::<ControlPlanePersistenceMeta>(
                        entity.state,
                    )?);
                }
                "workspace" => {
                    let record = serde_json::from_value::<WorkspaceBinding>(entity.state)?;
                    state.workspaces.insert(record.workspace_id.clone(), record);
                }
                "system_settings" => {
                    state.system_settings = serde_json::from_value::<SystemSettings>(entity.state)?;
                }
                "channel_binding" => {
                    let record = serde_json::from_value::<ChannelBinding>(entity.state)?;
                    state
                        .channel_bindings
                        .insert(record.channel_binding_id.clone(), record);
                }
                "panel_render" => {
                    let record = serde_json::from_value::<PanelRenderRecord>(entity.state)?;
                    state.panel_renders.insert(record.panel_id.clone(), record);
                }
                "message_session_binding" => {
                    let record = serde_json::from_value::<MessageSessionBinding>(entity.state)?;
                    state
                        .message_session_bindings
                        .insert(record.binding_id.clone(), record);
                }
                "provider_session" => {
                    let record = serde_json::from_value::<ProviderSessionRecord>(entity.state)?;
                    state
                        .provider_sessions
                        .insert(record.provider_session_id.clone(), record);
                }
                "live_instance" => {
                    let record = serde_json::from_value::<LiveInstanceRecord>(entity.state)?;
                    if let Some(workspace_id) = entity.workspace_id {
                        state.live_instance_workspaces.insert(
                            record.live_instance_id.clone(),
                            WorkspaceId::new(workspace_id),
                        );
                    }
                    state
                        .live_instances
                        .insert(record.live_instance_id.clone(), record);
                }
                "turn" => {
                    let record = serde_json::from_value::<TurnRecord>(entity.state)?;
                    state.turns.insert(record.turn_id.clone(), record);
                }
                "command" => {
                    let record = serde_json::from_value::<CommandWorkflow>(entity.state)?;
                    state.commands.insert(record.command_id.clone(), record);
                }
                "command_callback" => {
                    let record = serde_json::from_value::<super::commands::CommandCallbackRecord>(
                        entity.state,
                    )?;
                    state.command_callbacks.insert(record.token.clone(), record);
                }
                "intervention_callback" => {
                    let record = serde_json::from_value::<
                        super::interventions::InterventionCallbackRecord,
                    >(entity.state)?;
                    state
                        .intervention_callbacks
                        .insert(record.token.clone(), record);
                }
                "subagent_action" => {
                    let record = serde_json::from_value::<super::subagents::SubAgentActionRecord>(
                        entity.state,
                    )?;
                    state
                        .subagent_actions
                        .insert(record.action_id.clone(), record);
                }
                "subagent_link" => {
                    let record = serde_json::from_value::<super::subagents::SubAgentLinkRecord>(
                        entity.state,
                    )?;
                    state.subagent_links.insert(record.link_id.clone(), record);
                }
                "subagent_callback" => {
                    let record = serde_json::from_value::<super::subagents::SubAgentCallbackRecord>(
                        entity.state,
                    )?;
                    state
                        .subagent_callbacks
                        .insert(record.token.clone(), record);
                }
                "scheduled_task" => {
                    let record = serde_json::from_value::<ScheduledTaskRecord>(entity.state)?;
                    state.scheduled_tasks.insert(record.task_id.clone(), record);
                }
                "history_replay" => {
                    let record = serde_json::from_value::<
                        super::history_replay::HistoryReplayRecord,
                    >(entity.state)?;
                    state
                        .history_replays
                        .insert(record.workspace_id.clone(), record);
                }
                "history_older_callback" => {
                    let record = serde_json::from_value::<
                        super::history_replay::HistoryOlderCallbackRecord,
                    >(entity.state)?;
                    state
                        .history_older_callbacks
                        .insert(record.token.clone(), record);
                }
                "reconcile_outcome" => {
                    if let Some(workspace_id) = entity.workspace_id {
                        let outcome = serde_json::from_value::<ReconcileOutcome>(entity.state)?;
                        state
                            .last_reconcile_by_workspace
                            .insert(WorkspaceId::new(workspace_id), outcome);
                    }
                }
                "timeline" => {
                    // Timeline items are loaded lazily per workspace.
                    // from_persistence_entities no longer processes them.
                }
                _ => {}
            }
        }
        if let Some(meta) = meta {
            state.next_turn = meta.next_turn;
            state.next_command = meta.next_command;
            state.next_command_callback = meta.next_command_callback;
            state.next_intervention_callback = meta.next_intervention_callback;
            state.next_subagent_action = meta.next_subagent_action;
            state.next_subagent_callback = meta.next_subagent_callback;
            state.next_history_older_callback = meta.next_history_older_callback;
            state.next_timeline_by_workspace = meta.next_timeline_by_workspace;
        }
        Ok(state)
    }

    #[doc(hidden)]
    pub fn set_timeline_store(&mut self, conn: Arc<Mutex<rusqlite::Connection>>) {
        self.timeline_conn = Some(conn);
    }

    /// Load timeline items for a workspace from the store. No-op if already loaded
    /// or if no store connection is available.
    pub fn ensure_timeline_loaded(
        &mut self,
        workspace_id: &WorkspaceId,
    ) -> Result<(), ControlPlaneStoreError> {
        if self.loaded_timeline_workspaces.contains(workspace_id) {
            return Ok(());
        }
        let Some(conn) = &self.timeline_conn else {
            return Ok(());
        };
        let conn = conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT state_json
             FROM control_plane_entities
             WHERE kind = 'timeline' AND workspace_id = ?1
             ORDER BY entity_id",
        )?;
        let rows = stmt.query_map(rusqlite::params![workspace_id.as_str()], |row| {
            let state_json: String = row.get(0)?;
            serde_json::from_str::<TimelineItem>(&state_json).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })
        })?;
        for row in rows {
            let item: TimelineItem = row?;
            self.timeline.push(timeline_index_item(&item));
        }
        self.loaded_timeline_workspaces.insert(workspace_id.clone());
        debug!(
            target: "lucarne::control_plane::state",
            workspace_id = workspace_id.as_str(),
            timeline_len = self.timeline.len(),
            "workspace timeline lazy-loaded"
        );
        Ok(())
    }

    pub fn system_settings(&self) -> SystemSettings {
        self.system_settings.clone()
    }

    pub fn set_system_settings(&mut self, settings: SystemSettings) -> SystemSettings {
        self.system_settings = settings;
        self.system_settings.clone()
    }

    pub fn set_force_bypass_permissions(&mut self, enabled: bool) -> SystemSettings {
        self.system_settings.session.force_bypass_permissions = enabled;
        self.system_settings.clone()
    }

    pub fn set_global_notifications_enabled(&mut self, enabled: bool) -> SystemSettings {
        self.system_settings.notifications.enabled = enabled;
        self.system_settings.clone()
    }

    pub fn set_workspace_notifications_enabled(
        &mut self,
        project_path: &Path,
        enabled: bool,
    ) -> SystemSettings {
        self.system_settings
            .workspace
            .entry(project_path.to_path_buf())
            .or_default()
            .notifications_enabled = Some(enabled);
        self.system_settings.clone()
    }

    pub fn set_session_notifications_enabled(
        &mut self,
        provider_session_id: &ProviderSessionId,
        enabled: bool,
    ) -> SystemSettings {
        self.system_settings
            .provider_session
            .entry(provider_session_id.clone())
            .or_default()
            .notifications_enabled = Some(enabled);
        self.system_settings.clone()
    }

    pub fn set_workspace_force_bypass_permissions(
        &mut self,
        project_path: &Path,
        enabled: bool,
    ) -> SystemSettings {
        self.system_settings
            .workspace
            .entry(project_path.to_path_buf())
            .or_default()
            .force_bypass_permissions = Some(enabled);
        self.system_settings.clone()
    }

    pub fn set_session_force_bypass_permissions(
        &mut self,
        provider_session_id: &ProviderSessionId,
        enabled: bool,
    ) -> SystemSettings {
        self.system_settings
            .provider_session
            .entry(provider_session_id.clone())
            .or_default()
            .force_bypass_permissions = Some(enabled);
        self.system_settings.clone()
    }

    pub fn effective_settings(
        &self,
        project_path: Option<&Path>,
        provider_session_id: Option<&ProviderSessionId>,
    ) -> EffectiveSettings {
        let mut settings = EffectiveSettings {
            session: self.system_settings.session.clone(),
            notifications: self.system_settings.notifications.clone(),
        };
        if let Some(project_path) = project_path {
            if let Some(override_settings) = self.system_settings.workspace.get(project_path) {
                override_settings.apply_to(&mut settings);
            }
        }
        if let Some(provider_session_id) = provider_session_id {
            if let Some(override_settings) = self
                .system_settings
                .provider_session
                .get(provider_session_id)
            {
                override_settings.apply_to(&mut settings);
            }
        }
        settings
    }

    pub fn upsert_workspace(&mut self, mut workspace: WorkspaceBinding) -> WorkspaceBinding {
        let now = SystemTime::now();
        if let Some(existing) = self.workspaces.get(&workspace.workspace_id) {
            workspace.created_at = existing.created_at;
            workspace.revision = existing.revision.next();
        } else {
            workspace.revision = Revision::new(1);
        }
        workspace.updated_at = now;
        if let Some(live_instance_id) = &workspace.active_live_instance_id {
            self.live_instance_workspaces
                .insert(live_instance_id.clone(), workspace.workspace_id.clone());
        }
        self.workspaces
            .insert(workspace.workspace_id.clone(), workspace.clone());
        debug!(
            target: "lucarne::control_plane::state",
            workspace_id = %workspace.workspace_id.as_str(),
            provider_id = %workspace.provider_id.as_str(),
            revision = workspace.revision.get(),
            "workspace upserted"
        );
        workspace
    }

    pub fn record_workspace_projection(&mut self, workspace: WorkspaceBinding) {
        if let Some(live_instance_id) = &workspace.active_live_instance_id {
            self.live_instance_workspaces
                .insert(live_instance_id.clone(), workspace.workspace_id.clone());
        }
        self.workspaces
            .insert(workspace.workspace_id.clone(), workspace);
    }

    pub fn get_workspace(&self, workspace_id: &WorkspaceId) -> Option<&WorkspaceBinding> {
        self.workspaces.get(workspace_id)
    }

    pub fn workspace_bindings(&self) -> Vec<WorkspaceBinding> {
        let mut workspaces = self.workspaces.values().cloned().collect::<Vec<_>>();
        workspaces.sort_by(|a, b| a.workspace_id.as_str().cmp(b.workspace_id.as_str()));
        workspaces
    }

    pub fn rename_workspace(
        &mut self,
        workspace_id: &WorkspaceId,
        title: impl Into<SmolStr>,
    ) -> Result<WorkspaceBinding, ControlPlaneError> {
        let workspace = self
            .workspaces
            .get_mut(workspace_id)
            .ok_or_else(|| ControlPlaneError::MissingWorkspace(workspace_id.clone()))?;
        workspace.title = title.into();
        workspace.updated_at = SystemTime::now();
        workspace.revision = workspace.revision.next();
        Ok(workspace.clone())
    }

    pub fn remove_workspace(&mut self, workspace_id: &WorkspaceId) -> Option<WorkspaceBinding> {
        self.channel_bindings
            .retain(|_, binding| &binding.workspace_id != workspace_id);
        self.timeline
            .retain(|item| &item.workspace_id != workspace_id);
        self.next_timeline_by_workspace.remove(workspace_id);
        self.last_reconcile_by_workspace.remove(workspace_id);
        self.commands
            .retain(|_, command| &command.workspace_id != workspace_id);
        self.turns
            .retain(|_, turn| &turn.workspace_id != workspace_id);
        self.scheduled_tasks
            .retain(|_, task| &task.workspace_id != workspace_id);
        if let Some(workspace) = self.workspaces.remove(workspace_id) {
            if let Some(live_instance_id) = &workspace.active_live_instance_id {
                self.live_instance_workspaces.remove(live_instance_id);
                if let Some(live) = self.live_instances.get_mut(live_instance_id) {
                    live.state = LiveInstanceState::Stale;
                    live.active_turn_id = None;
                    live.close_reason = Some("workspace removed".into());
                    live.last_seen_at = SystemTime::now();
                }
            }
            if let Some(provider_session_id) = &workspace.active_provider_session_id {
                let still_referenced = self.workspaces.values().any(|other| {
                    other.active_provider_session_id.as_ref() == Some(provider_session_id)
                });
                if !still_referenced {
                    self.provider_sessions.remove(provider_session_id);
                    self.message_session_bindings
                        .retain(|_, binding| &binding.provider_session_id != provider_session_id);
                }
            }
            Some(workspace)
        } else {
            None
        }
    }

    pub fn remove_history_replay(&mut self, workspace_id: &WorkspaceId) -> bool {
        self.history_replays.remove(workspace_id).is_some()
    }

    pub fn clear_workspace_records(&mut self) {
        let cleared_workspace_ids = self.workspaces.keys().cloned().collect::<HashSet<_>>();
        self.workspaces.clear();
        self.channel_bindings
            .retain(|_, binding| !cleared_workspace_ids.contains(&binding.workspace_id));
        self.message_session_bindings.clear();
        self.provider_sessions.clear();
        self.live_instances.clear();
        self.live_instance_workspaces.clear();
        self.turns.clear();
        self.commands.clear();
        self.command_callbacks.clear();
        self.intervention_callbacks.clear();
        self.subagent_actions.clear();
        self.subagent_links.clear();
        self.subagent_callbacks.clear();
        self.scheduled_tasks.clear();
        self.history_replays.clear();
        self.history_older_callbacks.clear();
        self.last_reconcile_by_workspace.clear();
        self.timeline.clear();
        self.loaded_timeline_workspaces.clear();
        self.next_timeline_by_workspace.clear();
    }

    pub fn upsert_scheduled_task(&mut self, mut task: ScheduledTaskRecord) -> ScheduledTaskRecord {
        let now = SystemTime::now();
        if let Some(existing) = self.scheduled_tasks.get(&task.task_id) {
            task.created_at = existing.created_at;
            task.last_run_unix_ms = existing.last_run_unix_ms;
        }
        task.updated_at = now;
        self.scheduled_tasks
            .insert(task.task_id.clone(), task.clone());
        task
    }

    pub fn scheduled_task(&self, task_id: &ScheduledTaskId) -> Option<ScheduledTaskRecord> {
        self.scheduled_tasks.get(task_id).cloned()
    }

    pub fn due_scheduled_tasks(&self, now_unix_ms: u64) -> Vec<ScheduledTaskRecord> {
        let mut tasks = self
            .scheduled_tasks
            .values()
            .filter(|task| task.enabled && task.next_run_unix_ms <= now_unix_ms)
            .cloned()
            .collect::<Vec<_>>();
        tasks.sort_by(|a, b| {
            a.next_run_unix_ms
                .cmp(&b.next_run_unix_ms)
                .then_with(|| a.task_id.cmp(&b.task_id))
        });
        tasks
    }

    pub fn mark_scheduled_task_triggered(
        &mut self,
        task_id: &ScheduledTaskId,
        now_unix_ms: u64,
    ) -> Option<ScheduledTaskRecord> {
        let task = self.scheduled_tasks.get_mut(task_id)?;
        task.enabled = false;
        task.last_run_unix_ms = Some(now_unix_ms);
        task.updated_at = SystemTime::now();
        Some(task.clone())
    }

    pub fn upsert_channel_binding(&mut self, mut binding: ChannelBinding) -> ChannelBinding {
        let now = SystemTime::now();
        if let Some(existing) = self.channel_bindings.get(&binding.channel_binding_id) {
            binding.created_at = existing.created_at;
        }
        binding.updated_at = now;
        self.channel_bindings
            .insert(binding.channel_binding_id.clone(), binding.clone());
        binding
    }

    pub fn get_channel_binding(
        &self,
        channel_binding_id: &ChannelBindingId,
    ) -> Option<&ChannelBinding> {
        self.channel_bindings.get(channel_binding_id)
    }

    pub fn channel_bindings_for_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Vec<ChannelBinding> {
        let mut bindings = self
            .channel_bindings
            .values()
            .filter(|binding| &binding.workspace_id == workspace_id)
            .cloned()
            .collect::<Vec<_>>();
        bindings.sort_by(|a, b| {
            a.channel_binding_id
                .as_str()
                .cmp(b.channel_binding_id.as_str())
        });
        bindings
    }

    pub fn remove_channel_binding(
        &mut self,
        channel_binding_id: &ChannelBindingId,
    ) -> Option<ChannelBinding> {
        self.channel_bindings.remove(channel_binding_id)
    }

    pub fn upsert_panel_render(&mut self, mut panel: PanelRenderRecord) -> PanelRenderRecord {
        let now = SystemTime::now();
        if let Some(existing) = self.panel_renders.get(&panel.panel_id) {
            panel.created_at = existing.created_at;
            if panel.last_observed_stale_revision.is_none() {
                panel.last_observed_stale_revision = existing.last_observed_stale_revision;
            }
        }
        panel.updated_at = now;
        self.panel_renders
            .insert(panel.panel_id.clone(), panel.clone());
        panel
    }

    pub fn get_panel_render(&self, panel_id: &PanelRenderId) -> Option<&PanelRenderRecord> {
        self.panel_renders.get(panel_id)
    }

    pub fn record_panel_stale_revision(
        &mut self,
        panel_id: PanelRenderId,
        channel: impl Into<SmolStr>,
        chat_id: impl Into<SmolStr>,
        observed: Revision,
        current: Revision,
    ) -> PanelRenderRecord {
        let mut record = self
            .panel_renders
            .get(&panel_id)
            .cloned()
            .unwrap_or_else(|| {
                PanelRenderRecord::new(panel_id, channel, chat_id, None::<&str>, current)
            });
        record.last_observed_stale_revision = Some(observed);
        record.last_reconcile_outcome = Some(ReconcileOutcome::StaleRevision);
        self.upsert_panel_render(record)
    }

    pub fn mark_panel_renders_stale_after_restart(&mut self) {
        let now = SystemTime::now();
        for panel in self.panel_renders.values_mut() {
            if panel.last_observed_stale_revision.is_none() {
                panel.last_observed_stale_revision = Some(panel.last_rendered_revision);
            }
            panel.last_reconcile_outcome = Some(ReconcileOutcome::StaleRevision);
            panel.updated_at = now;
        }
    }

    pub fn max_panel_render_revision(&self) -> Option<Revision> {
        self.panel_renders
            .values()
            .map(|panel| panel.last_rendered_revision)
            .max()
    }

    pub fn upsert_provider_session(
        &mut self,
        mut session: ProviderSessionRecord,
    ) -> ProviderSessionRecord {
        let now = SystemTime::now();
        if let Some(existing) = self.provider_sessions.get(&session.provider_session_id) {
            session.created_at = existing.created_at;
            if session.model.is_none() {
                session.model = existing.model.clone();
            }
            if session.reasoning.is_none() {
                session.reasoning = existing.reasoning.clone();
            }
            if session.permission_mode.is_none() {
                session.permission_mode = existing.permission_mode.clone();
            }
            if session.status_extra.is_null() {
                session.status_extra = existing.status_extra.clone();
            }
            if session.usage_snapshot.is_null() {
                session.usage_snapshot = existing.usage_snapshot.clone();
            }
            if session.context_snapshot.is_null() {
                session.context_snapshot = existing.context_snapshot.clone();
            }
        }
        session.updated_at = now;
        self.provider_sessions
            .insert(session.provider_session_id.clone(), session.clone());
        session
    }

    pub fn update_provider_status(
        &mut self,
        provider_session_id: &ProviderSessionId,
        status: &AgentStatus,
    ) -> Result<ProviderSessionRecord, ControlPlaneError> {
        let session = self
            .provider_sessions
            .get_mut(provider_session_id)
            .ok_or_else(|| {
                ControlPlaneError::MissingProviderSession(provider_session_id.clone())
            })?;
        session.model = status.model.clone();
        session.reasoning = status.reasoning.clone();
        session.permission_mode = status
            .permissions
            .as_deref()
            .map(|permission| SmolStr::new(permission.to_string()));
        session.usage_snapshot = status
            .tokens
            .as_ref()
            .map(|tokens| serde_json::to_value(tokens).expect("agent token usage must serialize"))
            .unwrap_or(serde_json::Value::Null);
        session.context_snapshot = status
            .context
            .as_ref()
            .map(|context| {
                serde_json::to_value(context).expect("agent context usage must serialize")
            })
            .unwrap_or(serde_json::Value::Null);
        session.status_extra =
            serde_json::to_value(&status).expect("agent status must serialize into status_extra");
        session.updated_at = SystemTime::now();
        Ok(session.clone())
    }

    pub fn get_provider_session(
        &self,
        provider_session_id: &ProviderSessionId,
    ) -> Option<&ProviderSessionRecord> {
        self.provider_sessions.get(provider_session_id)
    }

    pub fn upsert_message_session_binding(
        &mut self,
        mut binding: MessageSessionBinding,
    ) -> Result<MessageSessionBinding, ControlPlaneError> {
        if !self
            .provider_sessions
            .contains_key(&binding.provider_session_id)
        {
            return Err(ControlPlaneError::MissingProviderSession(
                binding.provider_session_id,
            ));
        }
        let now = SystemTime::now();
        if let Some(existing) = self.message_session_bindings.get(&binding.binding_id) {
            binding.created_at = existing.created_at;
        }
        binding.updated_at = now;
        self.message_session_bindings
            .insert(binding.binding_id.clone(), binding.clone());
        Ok(binding)
    }

    pub fn message_session_binding(
        &self,
        channel: &str,
        chat_id: &str,
        message_id: &str,
    ) -> Option<&MessageSessionBinding> {
        let binding_id = message_session_binding_id(channel, chat_id, message_id);
        self.message_session_bindings.get(&binding_id)
    }

    pub fn workspace_for_provider_session(
        &self,
        provider_session_id: &ProviderSessionId,
    ) -> Option<&WorkspaceBinding> {
        self.workspaces.values().find(|workspace| {
            workspace.active_provider_session_id.as_ref() == Some(provider_session_id)
        })
    }

    pub fn activate_provider_session(
        &mut self,
        workspace_id: WorkspaceId,
        provider_session_id: ProviderSessionId,
    ) -> Result<WorkspaceBinding, ControlPlaneError> {
        let workspace_provider_id = self
            .workspaces
            .get(&workspace_id)
            .ok_or_else(|| ControlPlaneError::MissingWorkspace(workspace_id.clone()))?
            .provider_id
            .clone();
        let provider_session = self
            .provider_sessions
            .get(&provider_session_id)
            .ok_or_else(|| {
                ControlPlaneError::MissingProviderSession(provider_session_id.clone())
            })?;
        if provider_session.provider_id != workspace_provider_id {
            return Err(ControlPlaneError::ProviderMismatch {
                expected: workspace_provider_id,
                actual: provider_session.provider_id.clone(),
            });
        }
        let workspace = self
            .workspaces
            .get_mut(&workspace_id)
            .ok_or_else(|| ControlPlaneError::MissingWorkspace(workspace_id.clone()))?;
        let changed = workspace.active_provider_session_id.as_ref() != Some(&provider_session_id)
            || workspace.active_live_instance_id.is_some();
        workspace.active_provider_session_id = Some(provider_session_id);
        workspace.active_live_instance_id = None;
        if changed {
            workspace.updated_at = SystemTime::now();
            workspace.revision = workspace.revision.next();
        }
        Ok(workspace.clone())
    }

    pub fn clear_workspace_activation(
        &mut self,
        workspace_id: &WorkspaceId,
        close_reason: impl Into<SmolStr>,
    ) -> Result<WorkspaceBinding, ControlPlaneError> {
        let close_reason = close_reason.into();
        let workspace = self
            .workspaces
            .get_mut(workspace_id)
            .ok_or_else(|| ControlPlaneError::MissingWorkspace(workspace_id.clone()))?;
        if let Some(live_instance_id) = workspace.active_live_instance_id.as_ref() {
            if let Some(live) = self.live_instances.get_mut(live_instance_id) {
                live.state = LiveInstanceState::Closed;
                live.active_turn_id = None;
                live.close_reason = Some(close_reason);
                live.last_seen_at = SystemTime::now();
            }
        }
        workspace.active_provider_session_id = None;
        workspace.active_live_instance_id = None;
        workspace.updated_at = SystemTime::now();
        workspace.revision = workspace.revision.next();
        Ok(workspace.clone())
    }

    pub fn attach_live_instance(
        &mut self,
        workspace_id: WorkspaceId,
        mut live: LiveInstanceRecord,
    ) -> Result<LiveInstanceRecord, ControlPlaneError> {
        let workspace_provider_id = self
            .workspaces
            .get(&workspace_id)
            .ok_or_else(|| ControlPlaneError::MissingWorkspace(workspace_id.clone()))?
            .provider_id
            .clone();
        let provider_session = self
            .provider_sessions
            .get(&live.provider_session_id)
            .ok_or_else(|| {
                ControlPlaneError::MissingProviderSession(live.provider_session_id.clone())
            })?;
        if provider_session.provider_id != workspace_provider_id {
            return Err(ControlPlaneError::ProviderMismatch {
                expected: workspace_provider_id,
                actual: provider_session.provider_id.clone(),
            });
        }
        if live.provider_id != workspace_provider_id {
            return Err(ControlPlaneError::ProviderMismatch {
                expected: workspace_provider_id,
                actual: live.provider_id.clone(),
            });
        }

        live.last_seen_at = SystemTime::now();
        self.live_instance_workspaces
            .insert(live.live_instance_id.clone(), workspace_id.clone());
        self.live_instances
            .insert(live.live_instance_id.clone(), live.clone());

        if live.state != LiveInstanceState::Stale {
            let workspace = self
                .workspaces
                .get_mut(&workspace_id)
                .ok_or_else(|| ControlPlaneError::MissingWorkspace(workspace_id.clone()))?;
            let changed = workspace.active_provider_session_id.as_ref()
                != Some(&live.provider_session_id)
                || workspace.active_live_instance_id.as_ref() != Some(&live.live_instance_id);
            workspace.active_provider_session_id = Some(live.provider_session_id.clone());
            workspace.active_live_instance_id = Some(live.live_instance_id.clone());
            if changed {
                workspace.updated_at = SystemTime::now();
                workspace.revision = workspace.revision.next();
            }
        }

        debug!(
            target: "lucarne::control_plane::state",
            workspace_id = %workspace_id.as_str(),
            live_instance_id = %live.live_instance_id.as_str(),
            provider_session_id = %live.provider_session_id.as_str(),
            state = ?live.state,
            "live instance attached"
        );
        Ok(live)
    }

    pub fn detach_live_instance(
        &mut self,
        workspace_id: WorkspaceId,
        live_instance_id: &LiveInstanceId,
        close_reason: impl Into<SmolStr>,
    ) -> Result<LiveInstanceRecord, ControlPlaneError> {
        let workspace = self
            .workspaces
            .get_mut(&workspace_id)
            .ok_or_else(|| ControlPlaneError::MissingWorkspace(workspace_id.clone()))?;
        let live = self
            .live_instances
            .get_mut(live_instance_id)
            .ok_or_else(|| ControlPlaneError::MissingLiveInstance(live_instance_id.clone()))?;

        live.state = LiveInstanceState::Closed;
        live.close_reason = Some(close_reason.into());
        live.last_seen_at = SystemTime::now();

        if workspace.active_live_instance_id.as_ref() == Some(live_instance_id) {
            workspace.active_live_instance_id = None;
            workspace.updated_at = SystemTime::now();
            workspace.revision = workspace.revision.next();
        }

        Ok(live.clone())
    }

    pub fn record_live_instance_projection(
        &mut self,
        live: LiveInstanceRecord,
        workspace_id: Option<WorkspaceId>,
    ) {
        if let Some(workspace_id) = workspace_id {
            self.live_instance_workspaces
                .insert(live.live_instance_id.clone(), workspace_id);
        }
        self.live_instances
            .insert(live.live_instance_id.clone(), live);
    }

    pub fn get_live_instance(
        &self,
        live_instance_id: &LiveInstanceId,
    ) -> Option<&LiveInstanceRecord> {
        self.live_instances.get(live_instance_id)
    }

    pub fn start_turn(
        &mut self,
        workspace_id: WorkspaceId,
        provider_session_id: ProviderSessionId,
        live_instance_id: LiveInstanceId,
        source: TurnSource,
        input: impl Into<SmolStr>,
        reply_to_channel_message_id: Option<i64>,
    ) -> Result<TurnRecord, ControlPlaneError> {
        let workspace = self
            .workspaces
            .get(&workspace_id)
            .ok_or_else(|| ControlPlaneError::MissingWorkspace(workspace_id.clone()))?;
        let provider_session = self
            .provider_sessions
            .get(&provider_session_id)
            .ok_or_else(|| {
                ControlPlaneError::MissingProviderSession(provider_session_id.clone())
            })?;
        if provider_session.provider_id != workspace.provider_id {
            return Err(ControlPlaneError::ProviderMismatch {
                expected: workspace.provider_id.clone(),
                actual: provider_session.provider_id.clone(),
            });
        }
        let live = self
            .live_instances
            .get(&live_instance_id)
            .ok_or_else(|| ControlPlaneError::MissingLiveInstance(live_instance_id.clone()))?;
        if live.provider_id != workspace.provider_id {
            return Err(ControlPlaneError::ProviderMismatch {
                expected: workspace.provider_id.clone(),
                actual: live.provider_id.clone(),
            });
        }
        if live.provider_session_id != provider_session_id {
            return Err(ControlPlaneError::LiveProviderSessionMismatch {
                live_instance_id: live_instance_id.clone(),
                expected: provider_session_id.clone(),
                actual: live.provider_session_id.clone(),
            });
        }
        if workspace.active_provider_session_id.as_ref() != Some(&provider_session_id)
            || workspace.active_live_instance_id.as_ref() != Some(&live_instance_id)
        {
            return Err(ControlPlaneError::WorkspaceActiveBindingMismatch {
                workspace_id: workspace_id.clone(),
                live_instance_id: live_instance_id.clone(),
            });
        }
        if let Some(active_turn_id) = live.active_turn_id.clone() {
            return Err(ControlPlaneError::LiveInstanceAlreadyRunning {
                live_instance_id: live_instance_id.clone(),
                active_turn_id,
            });
        }
        if live.state != LiveInstanceState::Idle {
            return Err(ControlPlaneError::LiveInstanceUnavailable {
                live_instance_id: live_instance_id.clone(),
                state: live.state,
            });
        }

        self.next_turn += 1;
        let turn = TurnRecord {
            turn_id: TurnId::new(format!("turn-{}", self.next_turn)),
            workspace_id,
            provider_session_id,
            live_instance_id: live_instance_id.clone(),
            source,
            input: input.into(),
            reply_to_channel_message_id,
            state: TurnState::Running,
            timeline_seq_start: None,
            timeline_seq_end: None,
            usage: serde_json::Value::Null,
            created_at: SystemTime::now(),
            completed_at: None,
        };

        if let Some(live) = self.live_instances.get_mut(&live_instance_id) {
            live.active_turn_id = Some(turn.turn_id.clone());
            live.state = LiveInstanceState::Running;
            live.last_seen_at = SystemTime::now();
        }

        self.turns.insert(turn.turn_id.clone(), turn.clone());
        debug!(
            target: "lucarne::control_plane::state",
            workspace_id = %turn.workspace_id.as_str(),
            turn_id = %turn.turn_id.as_str(),
            live_instance_id = %turn.live_instance_id.as_str(),
            source = ?turn.source,
            "turn started"
        );
        Ok(turn)
    }

    pub fn complete_turn(&mut self, turn_id: TurnId) -> Result<TurnRecord, ControlPlaneError> {
        self.complete_turn_with_usage(turn_id, None)
    }

    pub fn complete_turn_with_usage(
        &mut self,
        turn_id: TurnId,
        usage: Option<serde_json::Value>,
    ) -> Result<TurnRecord, ControlPlaneError> {
        let turn = self
            .turns
            .get_mut(&turn_id)
            .ok_or_else(|| ControlPlaneError::MissingTurn(turn_id.clone()))?;
        turn.state = TurnState::Completed;
        turn.completed_at = Some(SystemTime::now());
        if let Some(usage) = usage {
            turn.usage = usage;
        }
        let live_instance_id = turn.live_instance_id.clone();
        let completed = turn.clone();

        if let Some(live) = self.live_instances.get_mut(&live_instance_id) {
            if live.active_turn_id.as_ref() == Some(&turn_id) {
                live.active_turn_id = None;
                live.state = LiveInstanceState::Idle;
                live.last_seen_at = SystemTime::now();
            }
        }
        self.remove_intervention_callbacks_for_live_instance(&live_instance_id);

        debug!(
            target: "lucarne::control_plane::state",
            workspace_id = %completed.workspace_id.as_str(),
            turn_id = %completed.turn_id.as_str(),
            live_instance_id = %live_instance_id.as_str(),
            has_usage = !completed.usage.is_null(),
            "turn completed"
        );
        Ok(completed)
    }

    pub fn update_turn_usage(
        &mut self,
        turn_id: &TurnId,
        usage: serde_json::Value,
    ) -> Result<TurnRecord, ControlPlaneError> {
        let turn = self
            .turns
            .get_mut(turn_id)
            .ok_or_else(|| ControlPlaneError::MissingTurn(turn_id.clone()))?;
        turn.usage = usage;
        Ok(turn.clone())
    }

    pub fn mark_turn_waiting_permission(
        &mut self,
        turn_id: &TurnId,
    ) -> Result<LiveInstanceRecord, ControlPlaneError> {
        let turn = self
            .turns
            .get(turn_id)
            .ok_or_else(|| ControlPlaneError::MissingTurn(turn_id.clone()))?;
        let live = self
            .live_instances
            .get_mut(&turn.live_instance_id)
            .ok_or_else(|| ControlPlaneError::MissingLiveInstance(turn.live_instance_id.clone()))?;
        if live.active_turn_id.as_ref() != Some(turn_id) {
            return Err(ControlPlaneError::WorkspaceActiveBindingMismatch {
                workspace_id: turn.workspace_id.clone(),
                live_instance_id: turn.live_instance_id.clone(),
            });
        }
        live.state = LiveInstanceState::WaitingPermission;
        live.last_seen_at = SystemTime::now();
        Ok(live.clone())
    }

    pub fn mark_live_instance_running(
        &mut self,
        live_instance_id: &LiveInstanceId,
    ) -> Result<LiveInstanceRecord, ControlPlaneError> {
        let live = self
            .live_instances
            .get_mut(live_instance_id)
            .ok_or_else(|| ControlPlaneError::MissingLiveInstance(live_instance_id.clone()))?;
        if live.active_turn_id.is_some() {
            live.state = LiveInstanceState::Running;
            live.last_seen_at = SystemTime::now();
        }
        Ok(live.clone())
    }

    pub fn fail_turn(
        &mut self,
        turn_id: TurnId,
        error: impl Into<SmolStr>,
    ) -> Result<TurnRecord, ControlPlaneError> {
        let turn = self
            .turns
            .get_mut(&turn_id)
            .ok_or_else(|| ControlPlaneError::MissingTurn(turn_id.clone()))?;
        turn.state = TurnState::Failed;
        turn.completed_at = Some(SystemTime::now());
        let live_instance_id = turn.live_instance_id.clone();
        let failed = turn.clone();

        if let Some(live) = self.live_instances.get_mut(&live_instance_id) {
            if live.active_turn_id.as_ref() == Some(&turn_id) {
                live.active_turn_id = None;
                live.state = LiveInstanceState::Failed;
                live.close_reason = Some(error.into());
                live.last_seen_at = SystemTime::now();
            }
        }
        self.remove_intervention_callbacks_for_live_instance(&live_instance_id);

        Ok(failed)
    }

    pub fn orphan_turn(
        &mut self,
        turn_id: TurnId,
        reason: impl Into<SmolStr>,
    ) -> Result<TurnRecord, ControlPlaneError> {
        let reason = reason.into();
        let turn = self
            .turns
            .get_mut(&turn_id)
            .ok_or_else(|| ControlPlaneError::MissingTurn(turn_id.clone()))?;
        turn.state = TurnState::Orphaned;
        turn.completed_at = Some(SystemTime::now());
        let live_instance_id = turn.live_instance_id.clone();
        let orphaned = turn.clone();

        if let Some(live) = self.live_instances.get_mut(&live_instance_id) {
            if live.active_turn_id.as_ref() == Some(&turn_id) {
                live.active_turn_id = None;
                live.state = LiveInstanceState::Stale;
                live.close_reason = Some(reason);
                live.last_seen_at = SystemTime::now();
            }
        }
        self.remove_intervention_callbacks_for_live_instance(&live_instance_id);

        Ok(orphaned)
    }

    fn remove_intervention_callbacks_for_live_instance(
        &mut self,
        live_instance_id: &LiveInstanceId,
    ) {
        self.intervention_callbacks
            .retain(|_, callback| &callback.live_instance_id != live_instance_id);
    }

    pub fn turn_has_intervention_callbacks(
        &self,
        turn_id: &TurnId,
    ) -> Result<bool, ControlPlaneError> {
        let turn = self
            .turns
            .get(turn_id)
            .ok_or_else(|| ControlPlaneError::MissingTurn(turn_id.clone()))?;
        Ok(self
            .intervention_callbacks
            .values()
            .any(|callback| callback.live_instance_id == turn.live_instance_id))
    }

    pub fn record_turn_projection(&mut self, turn: TurnRecord) {
        self.turns.insert(turn.turn_id.clone(), turn);
    }

    pub fn get_turn(&self, turn_id: &TurnId) -> Option<&TurnRecord> {
        self.turns.get(turn_id)
    }

    pub fn start_command(
        &mut self,
        mut workflow: CommandWorkflow,
    ) -> Result<CommandWorkflow, ControlPlaneError> {
        if !self.workspaces.contains_key(&workflow.workspace_id) {
            return Err(ControlPlaneError::MissingWorkspace(workflow.workspace_id));
        }
        if !self.turns.contains_key(&workflow.turn_id) {
            return Err(ControlPlaneError::MissingTurn(workflow.turn_id));
        }
        let turn = self.turns.get(&workflow.turn_id).unwrap();
        if turn.workspace_id != workflow.workspace_id {
            return Err(ControlPlaneError::TurnWorkspaceMismatch {
                turn_id: workflow.turn_id,
                expected: turn.workspace_id.clone(),
                actual: workflow.workspace_id,
            });
        }
        if workflow.command_id.is_empty() {
            self.next_command += 1;
            workflow.command_id = CommandId::new(format!("command-{}", self.next_command));
        }
        workflow.state = CommandState::Running;
        self.commands
            .insert(workflow.command_id.clone(), workflow.clone());
        Ok(workflow)
    }

    fn complete_command_unchecked(
        &mut self,
        command_id: CommandId,
        result: serde_json::Value,
    ) -> Result<CommandWorkflow, ControlPlaneError> {
        let workflow = self
            .commands
            .get_mut(&command_id)
            .ok_or_else(|| ControlPlaneError::MissingCommand(command_id.clone()))?;
        workflow.state = CommandState::Completed;
        workflow.result = Some(result);
        workflow.completed_at = Some(SystemTime::now());
        Ok(workflow.clone())
    }

    pub fn complete_command_for_policy(
        &mut self,
        command_id: CommandId,
        policy: CommandCompletionPolicy,
        result: serde_json::Value,
    ) -> Result<CommandWorkflow, ControlPlaneError> {
        let workflow = self
            .commands
            .get(&command_id)
            .ok_or_else(|| ControlPlaneError::MissingCommand(command_id.clone()))?;
        if workflow.completion_policy != policy {
            return Err(ControlPlaneError::CommandCompletionPolicyMismatch {
                policy: workflow.completion_policy,
            });
        }
        self.complete_command_unchecked(command_id, result)
    }

    pub fn fail_command(
        &mut self,
        command_id: CommandId,
        error: impl Into<SmolStr>,
    ) -> Result<CommandWorkflow, ControlPlaneError> {
        let workflow = self
            .commands
            .get_mut(&command_id)
            .ok_or_else(|| ControlPlaneError::MissingCommand(command_id.clone()))?;
        workflow.state = CommandState::Failed;
        workflow.error = Some(error.into());
        workflow.completed_at = Some(SystemTime::now());
        Ok(workflow.clone())
    }

    pub fn record_command_projection(&mut self, command: CommandWorkflow) {
        self.commands.insert(command.command_id.clone(), command);
    }

    pub fn get_command(&self, command_id: &CommandId) -> Option<&CommandWorkflow> {
        self.commands.get(command_id)
    }

    pub fn command_workflows_for_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Vec<CommandWorkflow> {
        self.commands
            .values()
            .filter(|workflow| &workflow.workspace_id == workspace_id)
            .cloned()
            .collect()
    }

    pub fn append_timeline(
        &mut self,
        mut item: TimelineItem,
    ) -> Result<TimelineItem, ControlPlaneError> {
        if !self.workspaces.contains_key(&item.workspace_id) {
            return Err(ControlPlaneError::MissingWorkspace(item.workspace_id));
        }
        if !self.turns.contains_key(&item.turn_id) {
            return Err(ControlPlaneError::MissingTurn(item.turn_id));
        }
        let turn = self.turns.get(&item.turn_id).unwrap();
        if turn.workspace_id != item.workspace_id {
            return Err(ControlPlaneError::TurnWorkspaceMismatch {
                turn_id: item.turn_id,
                expected: turn.workspace_id.clone(),
                actual: item.workspace_id,
            });
        }

        let seq = self
            .next_timeline_by_workspace
            .entry(item.workspace_id.clone())
            .or_insert_with(TimelineSeq::default)
            .next();
        self.next_timeline_by_workspace
            .insert(item.workspace_id.clone(), seq);
        item.seq = seq;
        item.created_at = SystemTime::now();

        if let Some(turn) = self.turns.get_mut(&item.turn_id) {
            if turn.timeline_seq_start.is_none() {
                turn.timeline_seq_start = Some(seq);
            }
            turn.timeline_seq_end = Some(seq);
        }

        self.timeline.push(timeline_index_item(&item));
        debug!(
            target: "lucarne::control_plane::state",
            workspace_id = %item.workspace_id.as_str(),
            turn_id = %item.turn_id.as_str(),
            seq = item.seq.get(),
            kind = ?item.kind,
            "timeline item appended"
        );
        Ok(item)
    }

    pub fn timeline_for_workspace(&self, workspace_id: &WorkspaceId) -> Vec<&TimelineItem> {
        self.timeline
            .iter()
            .filter(|item| &item.workspace_id == workspace_id)
            .collect()
    }

    pub fn timeline_item(
        &self,
        workspace_id: &WorkspaceId,
        seq: TimelineSeq,
    ) -> Option<&TimelineItem> {
        self.timeline
            .iter()
            .find(|item| &item.workspace_id == workspace_id && item.seq == seq)
    }

    pub fn status_snapshot(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<StatusSnapshot, ControlPlaneError> {
        let workspace = self
            .workspaces
            .get(workspace_id)
            .ok_or_else(|| ControlPlaneError::MissingWorkspace(workspace_id.clone()))?;
        let provider_session = workspace
            .active_provider_session_id
            .as_ref()
            .and_then(|provider_session_id| self.provider_sessions.get(provider_session_id));
        let live = workspace
            .active_live_instance_id
            .as_ref()
            .and_then(|live_instance_id| self.live_instances.get(live_instance_id));
        let channel_binding = self
            .channel_bindings
            .values()
            .find(|binding| binding.workspace_id == workspace.workspace_id);
        let last_reconcile_outcome = self.last_reconcile_by_workspace.get(workspace_id).cloned();
        Ok(super::status::build_status_snapshot(
            Some(workspace),
            provider_session,
            live,
            channel_binding,
            last_reconcile_outcome,
        ))
    }

    pub fn record_reconcile_outcome(
        &mut self,
        workspace_id: WorkspaceId,
        outcome: ReconcileOutcome,
    ) -> Result<ReconcileOutcome, ControlPlaneError> {
        if !self.workspaces.contains_key(&workspace_id) {
            return Err(ControlPlaneError::MissingWorkspace(workspace_id));
        }
        self.last_reconcile_by_workspace
            .insert(workspace_id, outcome);
        Ok(outcome)
    }

    pub fn reject_stale_revision(
        &self,
        workspace_id: &WorkspaceId,
        observed: Revision,
    ) -> Result<(), ControlPlaneError> {
        let workspace = self
            .workspaces
            .get(workspace_id)
            .ok_or_else(|| ControlPlaneError::MissingWorkspace(workspace_id.clone()))?;
        if observed != workspace.revision {
            Err(ControlPlaneError::StaleRevision {
                current: workspace.revision,
                observed,
            })
        } else {
            Ok(())
        }
    }
}
