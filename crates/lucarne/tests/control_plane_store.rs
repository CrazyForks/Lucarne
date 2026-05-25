use lucarne::control_plane::{
    ChannelBinding, ChannelBindingId, ControlPlaneSqliteStore, ControlPlaneState,
    InterventionCallbackRecord, InterventionCallbackToken, LiveInstanceId, LiveInstanceRecord,
    MessageSessionBinding, PanelRenderId, PanelRenderRecord, ProviderSessionId,
    ProviderSessionRecord, Revision, SubAgentActionId, SubAgentLinkId, SubAgentLinkRecord,
    TimelineItem, TimelineItemKind, TurnId, TurnSource, WorkspaceBinding, WorkspaceId,
};
use rusqlite::Connection;
use std::time::SystemTime;

#[test]
fn sqlite_store_round_trips_control_plane_entities() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite3");
    let store = ControlPlaneSqliteStore::open(&db).unwrap();

    assert!(store.load_control_plane().unwrap().is_none());

    let mut state = ControlPlaneState::default();
    let workspace = state.upsert_workspace(WorkspaceBinding::new(
        WorkspaceId::new("ws-1"),
        "Workspace One",
        "codex",
        "/tmp/project",
    ));

    store
        .upsert_entities(ControlPlaneState::persistence_entities_for_workspace(
            &workspace,
        ))
        .unwrap();
    store
        .replace_non_timeline_entities(state.persistence_entities_without_timeline())
        .unwrap();

    let loaded = store.load_control_plane().unwrap().unwrap();
    assert!(loaded.get_workspace(&WorkspaceId::new("ws-1")).is_none());
    assert_eq!(
        store
            .workspace(&WorkspaceId::new("ws-1"))
            .unwrap()
            .unwrap()
            .title
            .as_str(),
        "Workspace One"
    );
}

#[test]
fn replace_non_timeline_entities_does_not_upsert_stale_cold_projection() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite3");
    let store = ControlPlaneSqliteStore::open(&db).unwrap();

    let mut state = ControlPlaneState::default();
    let workspace_one = state.upsert_workspace(WorkspaceBinding::new(
        WorkspaceId::new("ws-1"),
        "Workspace One",
        "codex",
        "/tmp/project-one",
    ));
    let workspace_two = state.upsert_workspace(WorkspaceBinding::new(
        WorkspaceId::new("ws-2"),
        "Workspace Two",
        "codex",
        "/tmp/project-two",
    ));
    store
        .upsert_entities(
            [
                ControlPlaneState::persistence_entities_for_workspace(&workspace_one),
                ControlPlaneState::persistence_entities_for_workspace(&workspace_two),
            ]
            .concat(),
        )
        .unwrap();

    install_entity_write_log(&db);

    state
        .rename_workspace(&WorkspaceId::new("ws-1"), "Workspace One Renamed")
        .unwrap();
    store
        .replace_non_timeline_entities(state.persistence_entities_without_timeline())
        .unwrap();

    let loaded = store.load_control_plane().unwrap().unwrap();
    assert!(loaded.get_workspace(&WorkspaceId::new("ws-1")).is_none());
    assert_eq!(
        store
            .workspace(&WorkspaceId::new("ws-1"))
            .unwrap()
            .unwrap()
            .title
            .as_str(),
        "Workspace One"
    );
    assert!(
        entity_write_log(&db, "workspace", "ws-1").is_empty(),
        "snapshot persistence must not upsert stale lazy workspace projections"
    );
    assert!(
        entity_write_log(&db, "workspace", "ws-2").is_empty(),
        "snapshot persistence must not touch unchanged lazy workspace projections"
    );
}

#[test]
fn replace_non_timeline_entities_preserves_db_only_workspaces() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite3");
    let store = ControlPlaneSqliteStore::open(&db).unwrap();

    let mut state = ControlPlaneState::default();
    let workspace_one = state.upsert_workspace(WorkspaceBinding::new(
        WorkspaceId::new("ws-1"),
        "Workspace One",
        "codex",
        "/tmp/project-one",
    ));
    let workspace_two = state.upsert_workspace(WorkspaceBinding::new(
        WorkspaceId::new("ws-2"),
        "Workspace Two",
        "codex",
        "/tmp/project-two",
    ));
    store
        .upsert_entities(
            [
                ControlPlaneState::persistence_entities_for_workspace(&workspace_one),
                ControlPlaneState::persistence_entities_for_workspace(&workspace_two),
            ]
            .concat(),
        )
        .unwrap();

    let mut hot_state = ControlPlaneState::default();
    hot_state.upsert_provider_session(ProviderSessionRecord::new(
        ProviderSessionId::new("session-a"),
        "codex",
        "native-a",
    ));
    store
        .replace_non_timeline_entities(hot_state.persistence_entities_without_timeline())
        .unwrap();

    assert!(store
        .workspace(&WorkspaceId::new("ws-1"))
        .unwrap()
        .is_some());
    assert!(store
        .workspace(&WorkspaceId::new("ws-2"))
        .unwrap()
        .is_some());
}

#[test]
fn sqlite_store_reads_workspace_rows_on_demand() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite3");
    let store = ControlPlaneSqliteStore::open(&db).unwrap();
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&store, &mut state);

    let loaded = store.load_control_plane().unwrap().unwrap();
    assert!(loaded
        .get_workspace(&WorkspaceId::new("workspace-a"))
        .is_none());
    store
        .replace_non_timeline_entities(loaded.persistence_entities_without_timeline())
        .unwrap();
    assert_eq!(
        store
            .workspace_id_for_live_instance(&LiveInstanceId::new("live-a"))
            .unwrap(),
        Some(WorkspaceId::new("workspace-a"))
    );

    assert_eq!(
        store
            .workspace(&WorkspaceId::new("workspace-a"))
            .unwrap()
            .expect("workspace row")
            .title
            .as_str(),
        "Workspace A"
    );
    assert_eq!(store.workspace_bindings().unwrap().len(), 1);
    assert_eq!(
        store
            .workspace_for_provider_session(&ProviderSessionId::new("session-a"))
            .unwrap()
            .expect("workspace for provider session")
            .workspace_id,
        WorkspaceId::new("workspace-a")
    );
}

#[test]
fn sqlite_store_defers_channel_bindings_and_preserves_them_in_snapshots() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite3");
    let store = ControlPlaneSqliteStore::open(&db).unwrap();
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&store, &mut state);
    let binding = state.upsert_channel_binding(ChannelBinding::new(
        ChannelBindingId::new("telegram:100:9"),
        WorkspaceId::new("workspace-a"),
        "telegram",
        "100",
        Some("9"),
    ));
    store
        .upsert_entities(ControlPlaneState::persistence_entities_for_channel_binding(
            &binding,
        ))
        .unwrap();

    let loaded = store.load_control_plane().unwrap().unwrap();
    assert!(loaded
        .get_channel_binding(&ChannelBindingId::new("telegram:100:9"))
        .is_none());
    assert_eq!(
        store
            .channel_binding(&ChannelBindingId::new("telegram:100:9"))
            .unwrap()
            .expect("lazy channel binding")
            .workspace_id,
        WorkspaceId::new("workspace-a")
    );
    assert_eq!(
        store
            .channel_bindings_for_workspace(&WorkspaceId::new("workspace-a"))
            .unwrap()
            .len(),
        1
    );

    install_entity_write_log(&db);
    state.remove_channel_binding(&binding.channel_binding_id);
    state
        .rename_workspace(&WorkspaceId::new("workspace-a"), "Workspace A Renamed")
        .unwrap();
    store
        .replace_non_timeline_entities(state.persistence_entities_without_timeline())
        .unwrap();

    assert!(store
        .channel_binding(&ChannelBindingId::new("telegram:100:9"))
        .unwrap()
        .is_some());
    assert!(
        entity_write_log(&db, "channel_binding", "telegram:100:9").is_empty(),
        "snapshot persistence must not touch lazy channel bindings"
    );
    assert!(store
        .delete_entity("channel_binding", "telegram:100:9")
        .unwrap());
    assert!(store
        .channel_binding(&ChannelBindingId::new("telegram:100:9"))
        .unwrap()
        .is_none());
}

#[test]
fn sqlite_store_maintains_table_lookup_indexes_without_json_extract() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite3");
    let store = ControlPlaneSqliteStore::open(&db).unwrap();
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&store, &mut state);
    let binding = MessageSessionBinding::new(
        "telegram",
        "100",
        "200",
        ProviderSessionId::new("session-a"),
    );
    store
        .upsert_entities(
            ControlPlaneState::persistence_entities_for_message_session_binding(&binding),
        )
        .unwrap();
    let link = SubAgentLinkRecord::new_non_openable(
        SubAgentLinkId::new("link-a"),
        WorkspaceId::new("workspace-a"),
        SubAgentActionId::new("action-a"),
        TurnId::new("turn-a"),
        ProviderSessionId::new("session-a"),
        "child",
    );
    store
        .upsert_entity_state(
            "subagent_link",
            link.link_id.as_str(),
            Some(link.workspace_id.as_str()),
            &link,
        )
        .unwrap();

    {
        let conn = store.clone_connection();
        let conn = conn.lock().unwrap();
        let schema: String = conn
            .query_row(
                "SELECT COALESCE(group_concat(sql, '\n'), '')
                 FROM sqlite_master
                 WHERE name LIKE 'control_plane_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!schema.contains("json_extract"));
        assert_eq!(
            conn.query_row(
                "SELECT workspace_id
                 FROM control_plane_workspace_provider_session_index
                 WHERE provider_session_id = 'session-a'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "workspace-a"
        );
        assert_eq!(
            conn.query_row(
                "SELECT binding_id
                 FROM control_plane_message_provider_session_index
                 WHERE provider_session_id = 'session-a'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            binding.binding_id.as_str()
        );
        assert_eq!(
            conn.query_row(
                "SELECT link_id
                 FROM control_plane_subagent_parent_turn_index
                 WHERE turn_id = 'turn-a'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "link-a"
        );
    }

    assert_eq!(
        store
            .workspace_for_provider_session(&ProviderSessionId::new("session-a"))
            .unwrap()
            .expect("workspace lookup")
            .workspace_id,
        WorkspaceId::new("workspace-a")
    );
    assert_eq!(
        store
            .message_session_bindings_for_provider_session(&ProviderSessionId::new("session-a"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .subagent_links_for_turn(&TurnId::new("turn-a"))
            .unwrap()
            .len(),
        1
    );

    {
        let conn = store.clone_connection();
        let conn = conn.lock().unwrap();
        conn.execute(
            "DELETE FROM control_plane_workspace_provider_session_index",
            [],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM control_plane_message_provider_session_index",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM control_plane_subagent_parent_turn_index", [])
            .unwrap();
        conn.execute(
            "DELETE FROM control_plane_store_meta WHERE key = 'lookup_index_version'",
            [],
        )
        .unwrap();
    }
    drop(store);
    let reopened = ControlPlaneSqliteStore::open(&db).unwrap();
    assert!(reopened
        .workspace_for_provider_session(&ProviderSessionId::new("session-a"))
        .unwrap()
        .is_some());
    assert_eq!(
        reopened
            .message_session_bindings_for_provider_session(&ProviderSessionId::new("session-a"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        reopened
            .subagent_links_for_turn(&TurnId::new("turn-a"))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn sqlite_store_does_not_rebuild_current_lookup_indexes_on_open() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite3");
    let store = ControlPlaneSqliteStore::open(&db).unwrap();
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&store, &mut state);

    {
        let conn = store.clone_connection();
        let conn = conn.lock().unwrap();
        conn.execute(
            "INSERT INTO control_plane_workspace_provider_session_index
                (workspace_id, provider_session_id)
             VALUES ('bogus-workspace', 'bogus-session')",
            [],
        )
        .unwrap();
    }
    drop(store);

    let reopened = ControlPlaneSqliteStore::open(&db).unwrap();
    let conn = reopened.clone_connection();
    let conn = conn.lock().unwrap();
    let retained = conn
        .query_row(
            "SELECT COUNT(*)
             FROM control_plane_workspace_provider_session_index
             WHERE workspace_id = 'bogus-workspace'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(
        retained, 1,
        "opening a current store must not clear and rebuild lookup tables"
    );
}

#[test]
fn sqlite_store_defers_message_session_bindings_and_preserves_them_in_snapshots() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite3");
    let store = ControlPlaneSqliteStore::open(&db).unwrap();
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&store, &mut state);
    let binding = state
        .upsert_message_session_binding(MessageSessionBinding::new(
            "telegram",
            "100",
            "200",
            ProviderSessionId::new("session-a"),
        ))
        .unwrap();
    store
        .upsert_entities(
            ControlPlaneState::persistence_entities_for_message_session_binding(&binding),
        )
        .unwrap();

    let loaded = store.load_control_plane().unwrap().unwrap();
    assert!(loaded
        .message_session_binding("telegram", "100", "200")
        .is_none());
    assert_eq!(
        store
            .message_session_binding("telegram", "100", "200")
            .unwrap()
            .expect("lazy binding")
            .provider_session_id,
        ProviderSessionId::new("session-a")
    );

    install_entity_write_log(&db);
    state
        .rename_workspace(&WorkspaceId::new("workspace-a"), "Workspace A Renamed")
        .unwrap();
    store
        .replace_non_timeline_entities(state.persistence_entities_without_timeline())
        .unwrap();

    assert!(store
        .message_session_binding("telegram", "100", "200")
        .unwrap()
        .is_some());
    assert!(
        entity_write_log(&db, "message_session_binding", "telegram:100:200").is_empty(),
        "snapshot persistence must not touch lazy message bindings"
    );
}

#[test]
fn sqlite_store_deletes_intervention_callbacks_by_exact_structured_fields() {
    let store = ControlPlaneSqliteStore::open_in_memory().unwrap();
    let callback = |token: &str, live: &str, req: &str| InterventionCallbackRecord {
        token: InterventionCallbackToken::new(token),
        workspace_id: WorkspaceId::new(format!("workspace-{live}")),
        workspace_revision: Revision::new(1),
        provider_session_id: None,
        live_instance_id: LiveInstanceId::new(live),
        req_id: req.into(),
        action: serde_json::Value::Null,
        created_at: SystemTime::now(),
    };
    for record in [
        callback("token-a", "live-a", "req%_exact"),
        callback("token-b", "live-b", "req%_exact"),
        callback("token-c", "live-a", "prefix-req%_exact"),
    ] {
        store
            .upsert_entity_state(
                "intervention_callback",
                record.token.as_str(),
                Some(record.workspace_id.as_str()),
                &record,
            )
            .unwrap();
    }

    store
        .delete_intervention_callbacks_for_request(&LiveInstanceId::new("live-a"), "req%_exact")
        .unwrap();

    assert!(store
        .intervention_callback(&InterventionCallbackToken::new("token-a"))
        .unwrap()
        .is_none());
    assert!(store
        .intervention_callback(&InterventionCallbackToken::new("token-b"))
        .unwrap()
        .is_some());
    assert!(store
        .intervention_callback(&InterventionCallbackToken::new("token-c"))
        .unwrap()
        .is_some());

    store
        .delete_intervention_callbacks_for_live_instances(&[LiveInstanceId::new("live-b")])
        .unwrap();
    assert!(store
        .intervention_callback(&InterventionCallbackToken::new("token-b"))
        .unwrap()
        .is_none());
    assert!(store
        .intervention_callback(&InterventionCallbackToken::new("token-c"))
        .unwrap()
        .is_some());
}

#[test]
fn sqlite_store_computes_max_panel_revision_from_rows() {
    let store = ControlPlaneSqliteStore::open_in_memory().unwrap();
    let low_lexically_high_id = PanelRenderRecord::new(
        PanelRenderId::new("z-panel"),
        "telegram",
        "100",
        None::<&str>,
        Revision::new(1),
    );
    let high_lexically_low_id = PanelRenderRecord::new(
        PanelRenderId::new("a-panel"),
        "telegram",
        "200",
        None::<&str>,
        Revision::new(9),
    );
    for panel in [&low_lexically_high_id, &high_lexically_low_id] {
        store
            .upsert_entity_state("panel_render", panel.panel_id.as_str(), None, panel)
            .unwrap();
    }

    assert_eq!(
        store.max_panel_render_revision().unwrap(),
        Some(Revision::new(9))
    );
}

#[test]
fn sqlite_store_loads_timeline_index_without_payload_and_fetches_payload_on_demand() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite3");
    let store = ControlPlaneSqliteStore::open(&db).unwrap();
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&store, &mut state);
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
    let payload = serde_json::json!({ "text": "x".repeat(1024) });
    let item = state
        .append_timeline(TimelineItem::new(
            WorkspaceId::new("workspace-a"),
            turn.turn_id,
            TimelineItemKind::Assistant,
            payload.clone(),
        ))
        .unwrap();
    store
        .upsert_entities(state.persistence_entities_for_timeline_item(&item))
        .unwrap();

    let mut loaded = store.load_control_plane().unwrap().unwrap();
    loaded
        .ensure_timeline_loaded(&WorkspaceId::new("workspace-a"))
        .unwrap();
    let indexed = loaded.timeline_for_workspace(&WorkspaceId::new("workspace-a"));
    assert_eq!(indexed.len(), 1);
    assert_eq!(indexed[0].payload, serde_json::Value::Null);

    let stored = store
        .timeline_item(&WorkspaceId::new("workspace-a"), item.seq)
        .unwrap()
        .expect("timeline item");
    assert_eq!(stored.payload, payload);
}

fn install_entity_write_log(db: &std::path::Path) {
    let conn = Connection::open(db).unwrap();
    conn.execute_batch(
        "CREATE TABLE entity_write_log (
            action TEXT NOT NULL,
            kind TEXT NOT NULL,
            entity_id TEXT NOT NULL
        );
        CREATE TRIGGER log_control_plane_delete
        AFTER DELETE ON control_plane_entities
        BEGIN
            INSERT INTO entity_write_log(action, kind, entity_id)
            VALUES ('delete', OLD.kind, OLD.entity_id);
        END;
        CREATE TRIGGER log_control_plane_insert
        AFTER INSERT ON control_plane_entities
        BEGIN
            INSERT INTO entity_write_log(action, kind, entity_id)
            VALUES ('insert', NEW.kind, NEW.entity_id);
        END;
        CREATE TRIGGER log_control_plane_update
        AFTER UPDATE ON control_plane_entities
        BEGIN
            INSERT INTO entity_write_log(action, kind, entity_id)
            VALUES ('update', NEW.kind, NEW.entity_id);
        END;",
    )
    .unwrap();
}

fn seed_workspace_session_and_live(store: &ControlPlaneSqliteStore, state: &mut ControlPlaneState) {
    let workspace_id = WorkspaceId::new("workspace-a");
    state.upsert_workspace(WorkspaceBinding::new(
        workspace_id.clone(),
        "Workspace A",
        "codex",
        "/tmp/project-a",
    ));
    let provider_session = state.upsert_provider_session(ProviderSessionRecord::new(
        ProviderSessionId::new("session-a"),
        "codex",
        "native-session-a",
    ));
    let live = state
        .attach_live_instance(
            workspace_id.clone(),
            LiveInstanceRecord::new(
                LiveInstanceId::new("live-a"),
                "codex",
                ProviderSessionId::new("session-a"),
                Some("pid-1"),
            ),
        )
        .unwrap();
    let workspace = state
        .get_workspace(&workspace_id)
        .expect("workspace after live attach")
        .clone();
    store
        .upsert_entities(
            [
                ControlPlaneState::persistence_entities_for_workspace(&workspace),
                ControlPlaneState::persistence_entities_for_provider_session(&provider_session),
                ControlPlaneState::persistence_entities_for_live_instance(
                    &live,
                    Some(&workspace.workspace_id),
                ),
            ]
            .concat(),
        )
        .unwrap();
    store
        .replace_non_timeline_entities(state.persistence_entities_without_timeline())
        .unwrap();
}

fn entity_write_log(db: &std::path::Path, kind: &str, entity_id: &str) -> Vec<String> {
    let conn = Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT action FROM entity_write_log
             WHERE kind = ?1 AND entity_id = ?2
             ORDER BY rowid",
        )
        .unwrap();
    stmt.query_map((kind, entity_id), |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}
