use lucarne::agent_runtime::{
    AgentCommand, AgentCommandCatalog, AgentCommandCompletion, AgentCommandInput,
    AgentCommandSource, AgentContextUsage, AgentStatus, AgentTokenUsage,
};
use lucarne::control_plane::{
    plan_command_invocation, ActivationCheck, ActivationRequest, ChannelBinding, ChannelBindingId,
    CommandCompletionPolicy, CommandPlanError, CommandState, CommandWorkflow, ControlPlaneError,
    ControlPlaneState, LiveInstanceId, LiveInstanceRecord, LiveInstanceState, ProviderSessionId,
    ProviderSessionRecord, ReconcileOutcome, Revision, SubAgentActionRecord, SubAgentLinkId,
    SubAgentLinkRecord, SubAgentState, TimelineItem, TimelineItemKind, TurnId, TurnSource,
    TurnState, WorkspaceBinding, WorkspaceId,
};
use smol_str::SmolStr;

#[test]
fn identity_types_are_not_interchangeable() {
    fn accepts_workspace_id(_: WorkspaceId) {}
    fn accepts_channel_binding_id(_: ChannelBindingId) {}

    let workspace_id = WorkspaceId::new("workspace-a");
    let channel_binding_id = ChannelBindingId::new("telegram-topic-a");

    accepts_workspace_id(workspace_id);
    accepts_channel_binding_id(channel_binding_id);
}

#[test]
fn subagent_action_preserves_parent_turn_and_child_identity() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    let parent_turn_id = TurnId::new("turn-1");
    let parent_session_id = ProviderSessionId::new("parent-session");
    let child_session_id = ProviderSessionId::new("child-session");

    let mut action = SubAgentActionRecord::new(
        WorkspaceId::new("ws-parent"),
        parent_turn_id.clone(),
        parent_session_id.clone(),
        "Task",
    );
    action.provider_item_id = Some("toolu_123".into());
    action.prompt = Some("Inspect parser".into());
    action.requested_model = Some("opus".into());
    action.summary = Some("child started".into());
    action.state = SubAgentState::Running;
    let action = state.record_subagent_action(action);
    let mut link = SubAgentLinkRecord::new_openable(
        SubAgentLinkId::new("link-1"),
        WorkspaceId::new("ws-parent"),
        action.action_id.clone(),
        parent_turn_id.clone(),
        parent_session_id.clone(),
        child_session_id.clone(),
        "native-child-ref",
    );
    link.agent_id = Some("agent-1".into());
    link.nickname = Some("Parser".into());
    link.role = Some("explorer".into());
    link.model = Some("opus".into());
    link.prompt = Some("Inspect parser".into());
    link.last_message = Some("child started".into());
    link.state = SubAgentState::Running;
    let link = state.upsert_subagent_link(link);

    let stored_action = state
        .subagent_action(&action.action_id)
        .expect("stored subagent action");
    assert_eq!(stored_action.provider_item_id.as_deref(), Some("toolu_123"));
    assert_eq!(stored_action.prompt.as_deref(), Some("Inspect parser"));
    assert_eq!(stored_action.requested_model.as_deref(), Some("opus"));
    assert_eq!(stored_action.summary.as_deref(), Some("child started"));
    assert_eq!(stored_action.state, SubAgentState::Running);
    assert_eq!(link.workspace_id, WorkspaceId::new("ws-parent"));
    assert_eq!(link.parent_turn_id, parent_turn_id);
    assert_eq!(link.parent_provider_session_id, parent_session_id);
    assert_eq!(link.child_provider_session_id, Some(child_session_id));
    assert_eq!(link.child_native_ref.as_deref(), Some("native-child-ref"));
    assert!(link.openable);
    assert_eq!(link.agent_id.as_deref(), Some("agent-1"));
    assert_eq!(link.nickname.as_deref(), Some("Parser"));
    assert_eq!(link.role.as_deref(), Some("explorer"));
    assert_eq!(link.model.as_deref(), Some("opus"));
    assert_eq!(link.prompt.as_deref(), Some("Inspect parser"));
    assert_eq!(link.last_message.as_deref(), Some("child started"));
    assert_eq!(link.state, SubAgentState::Running);
}

#[test]
fn non_openable_subagent_action_does_not_create_child_workspace() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    let parent_turn_id = TurnId::new("turn-1");
    let parent_session_id = ProviderSessionId::new("parent-session");
    let action = state.record_subagent_action(SubAgentActionRecord::new(
        WorkspaceId::new("ws-parent"),
        parent_turn_id.clone(),
        parent_session_id.clone(),
        "Task",
    ));

    state.upsert_subagent_link(SubAgentLinkRecord::new_non_openable(
        SubAgentLinkId::new("link-1"),
        WorkspaceId::new("ws-parent"),
        action.action_id,
        parent_turn_id.clone(),
        parent_session_id,
        "native task did not expose child session",
    ));

    let links = state.subagent_links_for_turn(&parent_turn_id);
    assert_eq!(links.len(), 1);
    assert!(!links[0].openable);
    assert_eq!(links[0].child_provider_session_id, None);
    assert_eq!(links[0].state, SubAgentState::Unsupported);
    assert!(state.get_workspace(&WorkspaceId::new("child")).is_none());
}

#[test]
fn subagent_child_workspace_attach_preserves_link_revision_for_existing_buttons() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    let parent_turn_id = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::UserMessage,
            "inspect",
            None,
        )
        .unwrap()
        .turn_id;
    let action = state.record_subagent_action(SubAgentActionRecord::new(
        WorkspaceId::new("workspace-a"),
        parent_turn_id.clone(),
        ProviderSessionId::new("session-a"),
        "Task",
    ));
    let link = state.upsert_subagent_link(SubAgentLinkRecord::new_openable(
        SubAgentLinkId::new("link-1"),
        WorkspaceId::new("workspace-a"),
        action.action_id,
        parent_turn_id,
        ProviderSessionId::new("session-a"),
        ProviderSessionId::new("child-session"),
        "child-session",
    ));
    let callback = state
        .register_subagent_callback(WorkspaceId::new("workspace-a"), link.link_id.clone())
        .unwrap();

    let updated = state
        .attach_subagent_child_workspace(&link.link_id, WorkspaceId::new("child-workspace"))
        .unwrap();
    let resolved = state.resolve_subagent_callback(&callback.token).unwrap();

    assert_eq!(updated.revision, callback.link_revision);
    assert_eq!(resolved.link_revision, callback.link_revision);
    assert_eq!(
        state
            .openable_subagent_link(&link.link_id)
            .unwrap()
            .child_workspace_id,
        Some(WorkspaceId::new("child-workspace"))
    );
}

#[test]
fn status_snapshot_is_constructed_from_control_plane_read_model() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    let status = AgentStatus {
        version: Some(SmolStr::new("codex-1.2.3")),
        directory: Some("/tmp/project-a".into()),
        model: Some(SmolStr::new("gpt-5")),
        model_detail: Some(SmolStr::new("gpt-5-2026-04-01")),
        reasoning: Some(SmolStr::new("high")),
        permissions: Some("auto-review".into()),
        account: Some("user@example.com (Plus)".into()),
        base_url: Some("https://api.example.test".into()),
        proxy: Some("http://127.0.0.1:6152".into()),
        setting_sources: Some("User settings".into()),
        agents_md: Some("/tmp/AGENTS.md".into()),
        tokens: Some(AgentTokenUsage {
            input_tokens: Some(10),
            output_tokens: Some(20),
            total_tokens: Some(30),
        }),
        context: Some(AgentContextUsage {
            used_tokens: Some(100),
            max_tokens: Some(200),
            percent_used: Some(50),
        }),
        compactions: Some(2),
        ..AgentStatus::default()
    };
    state
        .update_provider_status(&ProviderSessionId::new("session-a"), &status)
        .unwrap();
    state
        .record_reconcile_outcome(
            WorkspaceId::new("workspace-a"),
            ReconcileOutcome::TopicMissingRecreated,
        )
        .unwrap();

    let snapshot = state
        .status_snapshot(&WorkspaceId::new("workspace-a"))
        .unwrap();

    assert_eq!(snapshot.workspace_id, Some(WorkspaceId::new("workspace-a")));
    assert_eq!(snapshot.provider_id, Some(SmolStr::new("codex")));
    assert_eq!(snapshot.provider_version, Some(SmolStr::new("codex-1.2.3")));
    assert_eq!(
        snapshot.provider_session_id,
        Some(ProviderSessionId::new("session-a"))
    );
    assert_eq!(
        snapshot.live_instance_id,
        Some(LiveInstanceId::new("live-a"))
    );
    assert_eq!(snapshot.live_instance_state, Some(LiveInstanceState::Idle));
    assert_eq!(
        snapshot.channel_binding_id,
        Some(ChannelBindingId::new("binding-a"))
    );
    assert_eq!(snapshot.channel, Some(SmolStr::new("telegram")));
    assert_eq!(snapshot.chat_id, Some(SmolStr::new("chat-a")));
    assert_eq!(snapshot.topic_id, Some(SmolStr::new("topic-a")));
    assert_eq!(snapshot.directory.as_deref(), Some("/tmp/project-a"));
    assert_eq!(
        snapshot.channel_binding_state.as_deref(),
        Some("telegram:chat-a:topic-a")
    );
    assert_eq!(
        snapshot.last_reconcile_outcome,
        Some(ReconcileOutcome::TopicMissingRecreated)
    );
    assert_eq!(snapshot.model, Some(SmolStr::new("gpt-5")));
    assert_eq!(
        snapshot.model_detail,
        Some(SmolStr::new("gpt-5-2026-04-01"))
    );
    assert_eq!(snapshot.reasoning, Some(SmolStr::new("high")));
    assert_eq!(snapshot.permission_mode, Some(SmolStr::new("auto-review")));
    assert_eq!(snapshot.account.as_deref(), Some("user@example.com (Plus)"));
    assert_eq!(
        snapshot.base_url.as_deref(),
        Some("https://api.example.test")
    );
    assert_eq!(snapshot.proxy.as_deref(), Some("http://127.0.0.1:6152"));
    assert_eq!(snapshot.setting_sources.as_deref(), Some("User settings"));
    assert_eq!(snapshot.agents_md.as_deref(), Some("/tmp/AGENTS.md"));
    assert_eq!(snapshot.token_usage, status.tokens.clone());
    assert_eq!(snapshot.context_usage, status.context.clone());
    assert_eq!(snapshot.compactions, Some(2));
    assert_eq!(
        snapshot.usage_snapshot,
        Some(serde_json::to_value(status.tokens.clone()).unwrap())
    );
    assert_eq!(
        snapshot.context_snapshot,
        Some(serde_json::to_value(status.context.clone()).unwrap())
    );
}

#[test]
fn command_callback_token_resolves_revisioned_action_without_raw_args() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    let long_args = "fork-target=".to_owned() + &"target-".repeat(20);
    let values = serde_json::json!({
        "target_id": long_args,
        "mode": "detached",
    });

    let record = state
        .register_command_callback(
            WorkspaceId::new("workspace-a"),
            Revision::new(1),
            "fork",
            Some(long_args.into()),
            values.clone(),
        )
        .unwrap();
    let payload = record.callback_payload();
    let resolved = state.resolve_command_callback(&record.token).unwrap();

    assert!(payload.len() < 64, "{payload}");
    assert_eq!(resolved.workspace_id, WorkspaceId::new("workspace-a"));
    assert_eq!(resolved.workspace_revision, Revision::new(2));
    assert_eq!(
        resolved.provider_session_id,
        Some(ProviderSessionId::new("session-a"))
    );
    assert_eq!(resolved.catalog_revision, Revision::new(1));
    assert_eq!(resolved.command_name, SmolStr::new("fork"));
    assert_eq!(resolved.args.as_deref(), record.args.as_deref());
    assert_eq!(resolved.values, values);
}

#[test]
fn command_invocation_plan_uses_provider_catalog_semantics() {
    let catalog = AgentCommandCatalog {
        commands: vec![
            AgentCommand {
                name: "report".into(),
                description: None,
                aliases: vec!["cost".into()],
                source: AgentCommandSource::ProviderNative,
                input: AgentCommandInput::Text {
                    label: "target".into(),
                    required: true,
                },
                completion: AgentCommandCompletion::ProviderIdle,
            },
            AgentCommand {
                name: "status".into(),
                description: None,
                aliases: Vec::new(),
                source: AgentCommandSource::AdapterMapped,
                input: AgentCommandInput::None,
                completion: AgentCommandCompletion::CommandResult,
            },
        ],
        complete: true,
        revision: 7,
    };

    let missing =
        plan_command_invocation(&catalog, "cost", "", serde_json::Value::Null).unwrap_err();
    assert_eq!(
        missing,
        CommandPlanError::MissingRequiredArgs {
            name: "cost".into(),
            label: "target".into(),
        }
    );

    let report = plan_command_invocation(
        &catalog,
        "cost",
        "today",
        serde_json::json!({"scope": "workspace"}),
    )
    .unwrap();
    assert_eq!(report.name.as_str(), "cost");
    assert_eq!(report.args.as_deref(), Some("today"));
    assert_eq!(report.catalog_revision, Revision::new(7));
    assert_eq!(
        report.completion_policy,
        CommandCompletionPolicy::ProviderIdle
    );
    assert_eq!(
        report.invocation().values,
        serde_json::json!({"scope": "workspace"})
    );

    let unsupported =
        plan_command_invocation(&catalog, "fork", "", serde_json::Value::Null).unwrap_err();
    assert_eq!(
        unsupported,
        CommandPlanError::Unsupported {
            name: "fork".into(),
        }
    );
}

#[test]
fn control_plane_persistence_entities_round_trip_indexed_records() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    let turn = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::UserMessage,
            "hello",
            Some(42),
        )
        .unwrap();
    let timeline_item = state
        .append_timeline(TimelineItem::new(
            WorkspaceId::new("workspace-a"),
            turn.turn_id.clone(),
            TimelineItemKind::User,
            "hello",
        ))
        .unwrap();
    state.complete_turn(turn.turn_id).unwrap();
    state
        .record_reconcile_outcome(WorkspaceId::new("workspace-a"), ReconcileOutcome::Ok)
        .unwrap();

    let entities = state.persistence_entities();

    assert!(entities.iter().any(|entity| {
        entity.kind == "workspace"
            && entity.workspace_id.as_deref() == Some("workspace-a")
            && entity.entity_id == "workspace-a"
    }));
    assert!(entities.iter().any(|entity| {
        entity.kind == "channel_binding"
            && entity.workspace_id.as_deref() == Some("workspace-a")
            && entity.entity_id == "binding-a"
    }));
    assert!(!entities.iter().any(|entity| entity.kind == "timeline"));

    let mut restored_entities = entities;
    restored_entities.extend(state.persistence_entities_for_timeline_item(&timeline_item));
    let restored = ControlPlaneState::from_persistence_entities(restored_entities).unwrap();
    let snapshot = restored
        .status_snapshot(&WorkspaceId::new("workspace-a"))
        .unwrap();
    assert_eq!(
        snapshot.channel_binding_id,
        Some(ChannelBindingId::new("binding-a"))
    );
    assert_eq!(
        snapshot.live_instance_id,
        Some(LiveInstanceId::new("live-a"))
    );
    assert_eq!(snapshot.last_reconcile_outcome, Some(ReconcileOutcome::Ok));
    assert_eq!(
        restored
            .timeline_for_workspace(&WorkspaceId::new("workspace-a"))
            .len(),
        1
    );
}

#[test]
fn canonical_activation_recreates_missing_channel_binding_without_changing_workspace_id() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    state.remove_channel_binding(&ChannelBindingId::new("binding-a"));

    let plan = state
        .plan_activation(ActivationRequest {
            workspace_id: WorkspaceId::new("workspace-a"),
            channel: SmolStr::new("telegram"),
            chat_id: SmolStr::new("chat-a"),
        })
        .unwrap();

    assert_eq!(plan.workspace.workspace_id, WorkspaceId::new("workspace-a"));
    assert!(plan.channel_binding.is_none());
    assert!(plan.has_check(ActivationCheck::ChannelBindingMissing));
    assert!(plan.requires_topic_creation());
    assert!(!plan.requires_topic_probe());
    assert_eq!(plan.reconcile_outcome, ReconcileOutcome::TopicMissing);
    assert!(state
        .status_snapshot(&WorkspaceId::new("workspace-a"))
        .unwrap()
        .channel_binding_id
        .is_none());
}

#[test]
fn fork_workspace_session_creates_child_without_rebinding_source() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);

    let fork = state
        .fork_workspace_session(lucarne::control_plane::ForkWorkspaceSession {
            source_workspace_id: WorkspaceId::new("workspace-a"),
            fork_workspace_id: WorkspaceId::new("workspace-fork"),
            title: "Workspace A fork".into(),
            provider_session_id: Some(ProviderSessionId::new("session-fork")),
            native_resume_ref: Some("native-session-fork".into()),
            live_instance_id: Some(LiveInstanceId::new("live-fork")),
            pid_or_handle: Some("pid-fork".into()),
        })
        .unwrap();

    let source = state
        .get_workspace(&WorkspaceId::new("workspace-a"))
        .expect("source workspace");
    assert_eq!(
        source.active_provider_session_id,
        Some(ProviderSessionId::new("session-a"))
    );
    assert_eq!(
        source.active_live_instance_id,
        Some(LiveInstanceId::new("live-a"))
    );
    assert_eq!(
        fork.source_workspace.workspace_id,
        WorkspaceId::new("workspace-a")
    );
    assert_eq!(
        fork.fork_workspace.workspace_id,
        WorkspaceId::new("workspace-fork")
    );
    assert_eq!(fork.fork_workspace.provider_id.as_str(), "codex");
    assert_eq!(
        fork.fork_workspace.project_path.as_path(),
        std::path::Path::new("/tmp/project-a")
    );
    assert_eq!(
        fork.fork_workspace.active_provider_session_id,
        Some(ProviderSessionId::new("session-fork"))
    );
    assert_eq!(
        fork.fork_workspace.active_live_instance_id,
        Some(LiveInstanceId::new("live-fork"))
    );
    assert_eq!(
        state
            .get_provider_session(&ProviderSessionId::new("session-fork"))
            .expect("fork provider session")
            .native_resume_ref
            .as_str(),
        "native-session-fork"
    );
    assert_eq!(
        state
            .get_live_instance(&LiveInstanceId::new("live-fork"))
            .expect("fork live instance")
            .provider_session_id,
        ProviderSessionId::new("session-fork")
    );
    assert!(state
        .channel_bindings_for_workspace(&WorkspaceId::new("workspace-fork"))
        .is_empty());
}

#[test]
fn fork_workspace_session_rejects_non_resumable_child_without_creating_workspace() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);

    let err = state
        .fork_workspace_session(lucarne::control_plane::ForkWorkspaceSession {
            source_workspace_id: WorkspaceId::new("workspace-a"),
            fork_workspace_id: WorkspaceId::new("workspace-fork"),
            title: "Workspace A fork".into(),
            provider_session_id: None,
            native_resume_ref: None,
            live_instance_id: Some(LiveInstanceId::new("live-fork")),
            pid_or_handle: Some("pid-fork".into()),
        })
        .unwrap_err();

    assert_eq!(
        err,
        ControlPlaneError::NonResumableFork {
            fork_workspace_id: WorkspaceId::new("workspace-fork"),
        }
    );
    assert!(
        state
            .get_workspace(&WorkspaceId::new("workspace-fork"))
            .is_none(),
        "non-resumable forks must not leave a workspace that later opens as a fresh session"
    );
}

#[test]
fn activation_plan_marks_missing_provider_session_stale() {
    let mut state = ControlPlaneState::default();
    let mut workspace = WorkspaceBinding::new(
        WorkspaceId::new("workspace-a"),
        "Workspace A",
        "codex",
        "/tmp/project-a",
    );
    workspace.active_provider_session_id = Some(ProviderSessionId::new("missing-session"));
    state.upsert_workspace(workspace);
    state.upsert_channel_binding(ChannelBinding::new(
        ChannelBindingId::new("binding-a"),
        WorkspaceId::new("workspace-a"),
        "telegram",
        "chat-a",
        Some("topic-a"),
    ));

    let plan = state
        .plan_activation(ActivationRequest {
            workspace_id: WorkspaceId::new("workspace-a"),
            channel: SmolStr::new("telegram"),
            chat_id: SmolStr::new("chat-a"),
        })
        .unwrap();

    assert!(plan.has_check(ActivationCheck::ProviderSessionMissing));
    assert!(plan.requires_topic_probe());
    assert_eq!(
        plan.reconcile_outcome,
        ReconcileOutcome::ProviderSessionStale
    );
}

#[test]
fn activation_plan_marks_missing_live_instance_stale() {
    let mut state = ControlPlaneState::default();
    let mut workspace = WorkspaceBinding::new(
        WorkspaceId::new("workspace-a"),
        "Workspace A",
        "codex",
        "/tmp/project-a",
    );
    workspace.active_provider_session_id = Some(ProviderSessionId::new("session-a"));
    workspace.active_live_instance_id = Some(LiveInstanceId::new("missing-live"));
    state.upsert_workspace(workspace);
    state.upsert_provider_session(ProviderSessionRecord::new(
        ProviderSessionId::new("session-a"),
        "codex",
        "native-session-a",
    ));
    state.upsert_channel_binding(ChannelBinding::new(
        ChannelBindingId::new("binding-a"),
        WorkspaceId::new("workspace-a"),
        "telegram",
        "chat-a",
        Some("topic-a"),
    ));

    let plan = state
        .plan_activation(ActivationRequest {
            workspace_id: WorkspaceId::new("workspace-a"),
            channel: SmolStr::new("telegram"),
            chat_id: SmolStr::new("chat-a"),
        })
        .unwrap();

    assert!(plan.has_check(ActivationCheck::ProviderSessionProbeRequired));
    assert!(plan.has_check(ActivationCheck::LiveInstanceStale));
    assert!(plan.requires_topic_probe());
    assert_eq!(plan.reconcile_outcome, ReconcileOutcome::LiveInstanceStale);
}

#[test]
fn activation_plan_requires_provider_probe_before_claiming_resume_ready() {
    let mut state = ControlPlaneState::default();
    let mut workspace = WorkspaceBinding::new(
        WorkspaceId::new("workspace-a"),
        "Workspace A",
        "codex",
        "/tmp/project-a",
    );
    workspace.active_provider_session_id = Some(ProviderSessionId::new("session-a"));
    state.upsert_workspace(workspace);
    state.upsert_provider_session(ProviderSessionRecord::new(
        ProviderSessionId::new("session-a"),
        "codex",
        "native-session-a",
    ));
    state.upsert_channel_binding(ChannelBinding::new(
        ChannelBindingId::new("binding-a"),
        WorkspaceId::new("workspace-a"),
        "telegram",
        "chat-a",
        Some("topic-a"),
    ));

    let plan = state
        .plan_activation(ActivationRequest {
            workspace_id: WorkspaceId::new("workspace-a"),
            channel: SmolStr::new("telegram"),
            chat_id: SmolStr::new("chat-a"),
        })
        .unwrap();

    assert!(plan.has_check(ActivationCheck::ProviderSessionProbeRequired));
    assert!(plan.requires_topic_probe());
    assert_eq!(
        plan.reconcile_outcome,
        ReconcileOutcome::ProviderSessionProbeRequired
    );
}

#[test]
fn activation_plan_surfaces_orphan_turns_separately_from_stale_live() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    let turn = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::UserMessage,
            "hello",
            None,
        )
        .unwrap();
    state
        .orphan_turn(turn.turn_id, "no completion")
        .expect("orphan turn");

    let plan = state
        .plan_activation(ActivationRequest {
            workspace_id: WorkspaceId::new("workspace-a"),
            channel: SmolStr::new("telegram"),
            chat_id: SmolStr::new("chat-a"),
        })
        .unwrap();

    assert!(plan.has_check(ActivationCheck::TurnOrphaned));
    assert!(plan.has_check(ActivationCheck::LiveInstanceStale));
    assert_eq!(plan.reconcile_outcome, ReconcileOutcome::TurnOrphaned);
}

#[test]
fn stale_revision_is_rejected() {
    let mut state = ControlPlaneState::default();
    let workspace = WorkspaceBinding::new(
        WorkspaceId::new("workspace-a"),
        "Workspace A",
        "codex",
        "/tmp/project-a",
    );

    state.upsert_workspace(workspace);

    assert_eq!(
        state.reject_stale_revision(&WorkspaceId::new("workspace-a"), Revision::new(0)),
        Err(ControlPlaneError::StaleRevision {
            current: Revision::new(1),
            observed: Revision::new(0),
        })
    );
    assert_eq!(
        state.reject_stale_revision(&WorkspaceId::new("workspace-a"), Revision::new(1)),
        Ok(())
    );
    assert_eq!(
        state.reject_stale_revision(&WorkspaceId::new("workspace-a"), Revision::new(2)),
        Err(ControlPlaneError::StaleRevision {
            current: Revision::new(1),
            observed: Revision::new(2),
        })
    );
}

#[test]
fn rename_workspace_preserves_active_provider_session() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    let turn = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::UserMessage,
            "hello",
            None,
        )
        .unwrap();
    state.complete_turn(turn.turn_id).unwrap();

    state
        .rename_workspace(&WorkspaceId::new("workspace-a"), "Renamed")
        .unwrap();

    let workspace = state
        .get_workspace(&WorkspaceId::new("workspace-a"))
        .unwrap();
    assert_eq!(workspace.title.as_str(), "Renamed");
    assert_eq!(
        workspace.active_provider_session_id,
        Some(ProviderSessionId::new("session-a"))
    );
    assert_eq!(
        workspace.active_live_instance_id,
        Some(LiveInstanceId::new("live-a"))
    );
}

#[test]
fn provider_session_can_be_active_without_live_instance() {
    let mut state = ControlPlaneState::default();
    state.upsert_workspace(WorkspaceBinding::new(
        WorkspaceId::new("workspace-a"),
        "Workspace A",
        "codex",
        "/tmp/project-a",
    ));
    state.upsert_provider_session(ProviderSessionRecord::new(
        ProviderSessionId::new("session-a"),
        "codex",
        "native-session-a",
    ));

    state
        .activate_provider_session(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
        )
        .unwrap();

    let snapshot = state
        .status_snapshot(&WorkspaceId::new("workspace-a"))
        .unwrap();
    assert_eq!(
        snapshot.provider_session_id,
        Some(ProviderSessionId::new("session-a"))
    );
    assert_eq!(snapshot.live_instance_state, None);
}

#[test]
fn clear_workspace_activation_removes_provider_and_live_refs() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);

    state
        .clear_workspace_activation(&WorkspaceId::new("workspace-a"), "new thread")
        .unwrap();

    let workspace = state
        .get_workspace(&WorkspaceId::new("workspace-a"))
        .unwrap();
    assert_eq!(workspace.active_provider_session_id, None);
    assert_eq!(workspace.active_live_instance_id, None);
    assert_eq!(
        state
            .get_live_instance(&LiveInstanceId::new("live-a"))
            .unwrap()
            .state,
        LiveInstanceState::Closed
    );
}

#[test]
fn remove_workspace_removes_workspace_scoped_records() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    let turn = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::UserMessage,
            "hello",
            None,
        )
        .unwrap();
    state
        .append_timeline(TimelineItem::new(
            WorkspaceId::new("workspace-a"),
            turn.turn_id,
            TimelineItemKind::User,
            "hello",
        ))
        .unwrap();

    let removed = state.remove_workspace(&WorkspaceId::new("workspace-a"));

    assert!(removed.is_some());
    assert!(state
        .get_workspace(&WorkspaceId::new("workspace-a"))
        .is_none());
    assert!(state
        .get_channel_binding(&ChannelBindingId::new("binding-a"))
        .is_none());
    assert!(state
        .timeline_for_workspace(&WorkspaceId::new("workspace-a"))
        .is_empty());
}

#[test]
fn command_no_output_ack_can_complete() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);

    let turn = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::Command,
            "/status",
            None,
        )
        .unwrap();
    let workflow = CommandWorkflow::new(
        WorkspaceId::new("workspace-a"),
        turn.turn_id.clone(),
        "status",
        None,
        serde_json::Value::Null,
        Revision::new(2),
        CommandCompletionPolicy::NoOutputAck,
    );
    let command = state.start_command(workflow).unwrap();

    state
        .complete_command_for_policy(
            command.command_id.clone(),
            CommandCompletionPolicy::NoOutputAck,
            serde_json::Value::Null,
        )
        .unwrap();

    let stored = state.get_command(&command.command_id).unwrap();
    assert_eq!(stored.state, CommandState::Completed);
    assert_eq!(
        stored.completion_policy,
        CommandCompletionPolicy::NoOutputAck
    );
}

#[test]
fn command_catalog_revision_is_provider_catalog_revision() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);

    let turn = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::Command,
            "/cost",
            None,
        )
        .unwrap();
    let workflow = CommandWorkflow::new(
        WorkspaceId::new("workspace-a"),
        turn.turn_id,
        "cost",
        None,
        serde_json::Value::Null,
        Revision::new(42),
        CommandCompletionPolicy::ProviderIdle,
    );

    let command = state.start_command(workflow).unwrap();

    assert_eq!(command.catalog_revision, Revision::new(42));
}

#[test]
fn command_completion_requires_matching_policy() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);

    let turn = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::Command,
            "/status",
            None,
        )
        .unwrap();
    let workflow = CommandWorkflow::new(
        WorkspaceId::new("workspace-a"),
        turn.turn_id,
        "status",
        None,
        serde_json::Value::Null,
        Revision::new(2),
        CommandCompletionPolicy::CommandResult,
    );
    let command = state.start_command(workflow).unwrap();

    let err = state
        .complete_command_for_policy(
            command.command_id.clone(),
            CommandCompletionPolicy::NoOutputAck,
            serde_json::Value::Null,
        )
        .unwrap_err();

    assert_eq!(
        err,
        ControlPlaneError::CommandCompletionPolicyMismatch {
            policy: CommandCompletionPolicy::CommandResult,
        }
    );
    assert_eq!(
        state.get_command(&command.command_id).unwrap().state,
        CommandState::Running
    );
}

#[test]
fn timeline_sequence_is_monotonic_per_workspace() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    let turn = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::UserMessage,
            "hello",
            Some(42),
        )
        .unwrap();

    let first = state
        .append_timeline(TimelineItem::new(
            WorkspaceId::new("workspace-a"),
            turn.turn_id.clone(),
            TimelineItemKind::User,
            "hello",
        ))
        .unwrap();
    let second = state
        .append_timeline(TimelineItem::new(
            WorkspaceId::new("workspace-a"),
            turn.turn_id.clone(),
            TimelineItemKind::Assistant,
            "hi",
        ))
        .unwrap();

    assert_eq!(first.seq.get(), 1);
    assert_eq!(second.seq.get(), 2);
    assert!(second.seq > first.seq);
}

#[test]
fn timeline_index_does_not_retain_payload() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    let turn = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::UserMessage,
            "hello",
            Some(42),
        )
        .unwrap();
    let payload = serde_json::json!({ "text": "x".repeat(1024) });

    let item = state
        .append_timeline(TimelineItem::new(
            WorkspaceId::new("workspace-a"),
            turn.turn_id,
            TimelineItemKind::Assistant,
            payload.clone(),
        ))
        .unwrap();

    assert_eq!(item.payload, payload);
    let indexed = state.timeline_for_workspace(&WorkspaceId::new("workspace-a"));
    assert_eq!(indexed.len(), 1);
    assert_eq!(indexed[0].payload, serde_json::Value::Null);

    let timeline_entity = state
        .persistence_entities_for_timeline_item(&item)
        .into_iter()
        .find(|entity| entity.kind == "timeline")
        .expect("timeline entity");
    assert_eq!(timeline_entity.state["payload"], payload);
}

#[test]
fn stale_live_instance_does_not_overwrite_provider_session_ref() {
    let mut state = ControlPlaneState::default();
    state.upsert_workspace(WorkspaceBinding::new(
        WorkspaceId::new("workspace-a"),
        "Workspace A",
        "codex",
        "/tmp/project-a",
    ));
    state.upsert_provider_session(ProviderSessionRecord::new(
        ProviderSessionId::new("session-a"),
        "codex",
        "native-session-a",
    ));
    state.upsert_provider_session(ProviderSessionRecord::new(
        ProviderSessionId::new("session-b"),
        "codex",
        "native-session-b",
    ));
    state
        .attach_live_instance(
            WorkspaceId::new("workspace-a"),
            LiveInstanceRecord::new(
                LiveInstanceId::new("live-old"),
                "codex",
                ProviderSessionId::new("session-a"),
                Some("pid-1"),
            ),
        )
        .unwrap();
    state
        .attach_live_instance(
            WorkspaceId::new("workspace-a"),
            LiveInstanceRecord::new(
                LiveInstanceId::new("live-new"),
                "codex",
                ProviderSessionId::new("session-b"),
                Some("pid-2"),
            ),
        )
        .unwrap();

    let stale = LiveInstanceRecord {
        state: LiveInstanceState::Stale,
        ..LiveInstanceRecord::new(
            LiveInstanceId::new("live-old"),
            "codex",
            ProviderSessionId::new("session-a"),
            Some("pid-1"),
        )
    };
    state
        .attach_live_instance(WorkspaceId::new("workspace-a"), stale)
        .unwrap();

    let workspace = state
        .get_workspace(&WorkspaceId::new("workspace-a"))
        .unwrap();
    assert_eq!(
        workspace.active_provider_session_id,
        Some(ProviderSessionId::new("session-b"))
    );
    assert_eq!(
        workspace.active_live_instance_id,
        Some(LiveInstanceId::new("live-new"))
    );
}

#[test]
fn live_instance_requires_existing_provider_session() {
    let mut state = ControlPlaneState::default();
    state.upsert_workspace(WorkspaceBinding::new(
        WorkspaceId::new("workspace-a"),
        "Workspace A",
        "codex",
        "/tmp/project-a",
    ));

    let err = state
        .attach_live_instance(
            WorkspaceId::new("workspace-a"),
            LiveInstanceRecord::new(
                LiveInstanceId::new("live-a"),
                "codex",
                ProviderSessionId::new("missing-session"),
                Some("pid-1"),
            ),
        )
        .unwrap_err();

    assert_eq!(
        err,
        ControlPlaneError::MissingProviderSession(ProviderSessionId::new("missing-session"))
    );
}

#[test]
fn live_instance_provider_must_match_workspace_provider() {
    let mut state = ControlPlaneState::default();
    state.upsert_workspace(WorkspaceBinding::new(
        WorkspaceId::new("workspace-a"),
        "Workspace A",
        "codex",
        "/tmp/project-a",
    ));
    state.upsert_provider_session(ProviderSessionRecord::new(
        ProviderSessionId::new("session-a"),
        "claude",
        "native-session-a",
    ));

    let err = state
        .attach_live_instance(
            WorkspaceId::new("workspace-a"),
            LiveInstanceRecord::new(
                LiveInstanceId::new("live-a"),
                "claude",
                ProviderSessionId::new("session-a"),
                Some("pid-1"),
            ),
        )
        .unwrap_err();

    assert_eq!(
        err,
        ControlPlaneError::ProviderMismatch {
            expected: SmolStr::new("codex"),
            actual: SmolStr::new("claude"),
        }
    );
}

#[test]
fn turn_requires_live_instance_for_same_provider_session() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    state.upsert_provider_session(ProviderSessionRecord::new(
        ProviderSessionId::new("session-b"),
        "codex",
        "native-session-b",
    ));

    let err = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-b"),
            LiveInstanceId::new("live-a"),
            TurnSource::UserMessage,
            "hello",
            None,
        )
        .unwrap_err();

    assert_eq!(
        err,
        ControlPlaneError::LiveProviderSessionMismatch {
            live_instance_id: LiveInstanceId::new("live-a"),
            expected: ProviderSessionId::new("session-b"),
            actual: ProviderSessionId::new("session-a"),
        }
    );
}

#[test]
fn turn_requires_workspace_active_live_binding() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    state.upsert_provider_session(ProviderSessionRecord::new(
        ProviderSessionId::new("session-b"),
        "codex",
        "native-session-b",
    ));
    state
        .attach_live_instance(
            WorkspaceId::new("workspace-a"),
            LiveInstanceRecord::new(
                LiveInstanceId::new("live-b"),
                "codex",
                ProviderSessionId::new("session-b"),
                Some("pid-2"),
            ),
        )
        .unwrap();

    let err = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::UserMessage,
            "hello",
            None,
        )
        .unwrap_err();

    assert_eq!(
        err,
        ControlPlaneError::WorkspaceActiveBindingMismatch {
            workspace_id: WorkspaceId::new("workspace-a"),
            live_instance_id: LiveInstanceId::new("live-a"),
        }
    );
}

#[test]
fn turn_rejects_closed_live_instance() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    state
        .detach_live_instance(
            WorkspaceId::new("workspace-a"),
            &LiveInstanceId::new("live-a"),
            "closed",
        )
        .unwrap();

    let err = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::UserMessage,
            "hello",
            None,
        )
        .unwrap_err();

    assert_eq!(
        err,
        ControlPlaneError::WorkspaceActiveBindingMismatch {
            workspace_id: WorkspaceId::new("workspace-a"),
            live_instance_id: LiveInstanceId::new("live-a"),
        }
    );
}

#[test]
fn turn_rejects_live_instance_with_active_turn() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    let turn = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::UserMessage,
            "hello",
            None,
        )
        .unwrap();

    let err = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::UserMessage,
            "again",
            None,
        )
        .unwrap_err();

    assert_eq!(
        err,
        ControlPlaneError::LiveInstanceAlreadyRunning {
            live_instance_id: LiveInstanceId::new("live-a"),
            active_turn_id: turn.turn_id,
        }
    );
}

#[test]
fn command_turn_must_match_command_workspace() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    state.upsert_workspace(WorkspaceBinding::new(
        WorkspaceId::new("workspace-b"),
        "Workspace B",
        "codex",
        "/tmp/project-b",
    ));
    let turn = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::Command,
            "/status",
            None,
        )
        .unwrap();
    let workflow = CommandWorkflow::new(
        WorkspaceId::new("workspace-b"),
        turn.turn_id.clone(),
        "status",
        None,
        serde_json::Value::Null,
        Revision::new(1),
        CommandCompletionPolicy::NoOutputAck,
    );

    let err = state.start_command(workflow).unwrap_err();

    assert_eq!(
        err,
        ControlPlaneError::TurnWorkspaceMismatch {
            turn_id: turn.turn_id,
            expected: WorkspaceId::new("workspace-a"),
            actual: WorkspaceId::new("workspace-b"),
        }
    );
}

#[test]
fn timeline_item_must_match_turn_workspace() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    state.upsert_workspace(WorkspaceBinding::new(
        WorkspaceId::new("workspace-b"),
        "Workspace B",
        "codex",
        "/tmp/project-b",
    ));
    let turn = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::UserMessage,
            "hello",
            None,
        )
        .unwrap();

    let err = state
        .append_timeline(TimelineItem::new(
            WorkspaceId::new("workspace-b"),
            turn.turn_id.clone(),
            TimelineItemKind::Assistant,
            "wrong workspace",
        ))
        .unwrap_err();

    assert_eq!(
        err,
        ControlPlaneError::TurnWorkspaceMismatch {
            turn_id: turn.turn_id,
            expected: WorkspaceId::new("workspace-a"),
            actual: WorkspaceId::new("workspace-b"),
        }
    );
}

#[test]
fn turn_reply_to_channel_message_id_is_persisted() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);

    let turn = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::UserMessage,
            "hello",
            Some(12345),
        )
        .unwrap();

    state.complete_turn(turn.turn_id.clone()).unwrap();

    let stored = state.get_turn(&turn.turn_id).unwrap();
    assert_eq!(stored.reply_to_channel_message_id, Some(12345));
    assert_eq!(stored.state, TurnState::Completed);
}

#[test]
fn permission_wait_moves_live_instance_through_control_plane_state() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);

    let turn = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::UserMessage,
            "run risky command",
            None,
        )
        .unwrap();

    let waiting = state.mark_turn_waiting_permission(&turn.turn_id).unwrap();
    assert_eq!(waiting.state, LiveInstanceState::WaitingPermission);
    let callback = state
        .register_intervention_callback(
            WorkspaceId::new("workspace-a"),
            LiveInstanceId::new("live-a"),
            "req-1",
            serde_json::json!({ "approve": true }),
        )
        .unwrap();

    let snapshot = state
        .status_snapshot(&WorkspaceId::new("workspace-a"))
        .unwrap();
    assert_eq!(
        snapshot.live_instance_state,
        Some(LiveInstanceState::WaitingPermission)
    );
    let restored = ControlPlaneState::from_persistence_entities(state.persistence_entities())
        .expect("intervention callback persists");
    let restored_callback = restored
        .resolve_intervention_callback(&callback.token)
        .expect("restored intervention callback");
    assert_eq!(restored_callback.req_id.as_str(), "req-1");
    assert_eq!(
        restored_callback.live_instance_id,
        LiveInstanceId::new("live-a")
    );

    let running = state
        .mark_live_instance_running(&LiveInstanceId::new("live-a"))
        .unwrap();
    assert_eq!(running.state, LiveInstanceState::Running);

    state.complete_turn(turn.turn_id.clone()).unwrap();
    let live = state
        .get_live_instance(&LiveInstanceId::new("live-a"))
        .unwrap();
    assert_eq!(live.state, LiveInstanceState::Idle);
    assert_eq!(live.active_turn_id, None);
}

#[test]
fn failed_permission_turn_removes_pending_intervention_callbacks() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);

    let turn = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::UserMessage,
            "run risky command",
            None,
        )
        .unwrap();
    state.mark_turn_waiting_permission(&turn.turn_id).unwrap();
    let callback = state
        .register_intervention_callback(
            WorkspaceId::new("workspace-a"),
            LiveInstanceId::new("live-a"),
            "req-1",
            serde_json::json!({ "approve": true }),
        )
        .unwrap();

    state
        .fail_turn(turn.turn_id.clone(), "permission timed out")
        .unwrap();

    assert!(
        state
            .resolve_intervention_callback(&callback.token)
            .is_none(),
        "permission callbacks must not outlive a failed turn"
    );
}

#[test]
fn orphan_turn_marks_live_instance_stale() {
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);

    let turn = state
        .start_turn(
            WorkspaceId::new("workspace-a"),
            ProviderSessionId::new("session-a"),
            LiveInstanceId::new("live-a"),
            TurnSource::Command,
            "/native-no-output",
            None,
        )
        .unwrap();

    let orphaned = state
        .orphan_turn(turn.turn_id.clone(), "command produced no completion")
        .unwrap();

    assert_eq!(orphaned.state, TurnState::Orphaned);
    let live = state
        .get_live_instance(&LiveInstanceId::new("live-a"))
        .unwrap();
    assert_eq!(live.state, LiveInstanceState::Stale);
    assert_eq!(live.active_turn_id, None);
    assert_eq!(
        live.close_reason.as_deref(),
        Some("command produced no completion")
    );
}

fn seed_workspace_session_and_live(state: &mut ControlPlaneState) {
    state.upsert_workspace(WorkspaceBinding::new(
        WorkspaceId::new("workspace-a"),
        "Workspace A",
        "codex",
        "/tmp/project-a",
    ));
    state.upsert_channel_binding(ChannelBinding::new(
        ChannelBindingId::new("binding-a"),
        WorkspaceId::new("workspace-a"),
        "telegram",
        "chat-a",
        Some("topic-a"),
    ));
    state.upsert_provider_session(ProviderSessionRecord::new(
        ProviderSessionId::new("session-a"),
        "codex",
        "native-session-a",
    ));
    state
        .attach_live_instance(
            WorkspaceId::new("workspace-a"),
            LiveInstanceRecord::new(
                LiveInstanceId::new("live-a"),
                "codex",
                ProviderSessionId::new("session-a"),
                Some("pid-1"),
            ),
        )
        .unwrap();
}
