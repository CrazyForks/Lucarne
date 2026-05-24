use lucarne::control_plane::{
    ControlPlaneSqliteStore, ControlPlaneState, LiveInstanceId, LiveInstanceRecord,
    MessageSessionBinding, ProviderSessionId, ProviderSessionRecord, TimelineItem,
    TimelineItemKind, TurnSource, WorkspaceBinding, WorkspaceId,
};
use rusqlite::Connection;

#[test]
fn sqlite_store_round_trips_control_plane_entities() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite3");
    let store = ControlPlaneSqliteStore::open(&db).unwrap();

    assert!(store.load_control_plane().unwrap().is_none());

    let mut state = ControlPlaneState::default();
    state.upsert_workspace(WorkspaceBinding::new(
        WorkspaceId::new("ws-1"),
        "Workspace One",
        "codex",
        "/tmp/project",
    ));

    store
        .replace_non_timeline_entities(state.persistence_entities_without_timeline())
        .unwrap();

    let loaded = store.load_control_plane().unwrap().unwrap();
    assert_eq!(
        loaded
            .get_workspace(&WorkspaceId::new("ws-1"))
            .unwrap()
            .title
            .as_str(),
        "Workspace One"
    );
}

#[test]
fn replace_non_timeline_entities_preserves_unchanged_rows() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite3");
    let store = ControlPlaneSqliteStore::open(&db).unwrap();

    let mut state = ControlPlaneState::default();
    state.upsert_workspace(WorkspaceBinding::new(
        WorkspaceId::new("ws-1"),
        "Workspace One",
        "codex",
        "/tmp/project-one",
    ));
    state.upsert_workspace(WorkspaceBinding::new(
        WorkspaceId::new("ws-2"),
        "Workspace Two",
        "codex",
        "/tmp/project-two",
    ));
    store
        .replace_non_timeline_entities(state.persistence_entities_without_timeline())
        .unwrap();

    install_entity_write_log(&db);

    state
        .rename_workspace(&WorkspaceId::new("ws-1"), "Workspace One Renamed")
        .unwrap();
    store
        .replace_non_timeline_entities(state.persistence_entities_without_timeline())
        .unwrap();

    let loaded = store.load_control_plane().unwrap().unwrap();
    assert_eq!(
        loaded
            .get_workspace(&WorkspaceId::new("ws-1"))
            .unwrap()
            .title
            .as_str(),
        "Workspace One Renamed"
    );
    assert!(
        entity_write_log(&db, "workspace", "ws-2").is_empty(),
        "unchanged entities should not be deleted and reinserted during snapshot persistence"
    );
}

#[test]
fn replace_non_timeline_entities_deletes_removed_rows() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite3");
    let store = ControlPlaneSqliteStore::open(&db).unwrap();

    let mut state = ControlPlaneState::default();
    state.upsert_workspace(WorkspaceBinding::new(
        WorkspaceId::new("ws-1"),
        "Workspace One",
        "codex",
        "/tmp/project-one",
    ));
    state.upsert_workspace(WorkspaceBinding::new(
        WorkspaceId::new("ws-2"),
        "Workspace Two",
        "codex",
        "/tmp/project-two",
    ));
    store
        .replace_non_timeline_entities(state.persistence_entities_without_timeline())
        .unwrap();

    state.remove_workspace(&WorkspaceId::new("ws-2"));
    store
        .replace_non_timeline_entities(state.persistence_entities_without_timeline())
        .unwrap();

    let loaded = store.load_control_plane().unwrap().unwrap();
    assert!(loaded.get_workspace(&WorkspaceId::new("ws-1")).is_some());
    assert!(loaded.get_workspace(&WorkspaceId::new("ws-2")).is_none());
}

#[test]
fn sqlite_store_defers_message_session_bindings_and_preserves_them_in_snapshots() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite3");
    let store = ControlPlaneSqliteStore::open(&db).unwrap();
    let mut state = ControlPlaneState::default();
    seed_workspace_session_and_live(&mut state);
    store
        .replace_non_timeline_entities(state.persistence_entities_without_timeline())
        .unwrap();
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
fn sqlite_store_loads_timeline_index_without_payload_and_fetches_payload_on_demand() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite3");
    let store = ControlPlaneSqliteStore::open(&db).unwrap();
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
        .upsert_entities(state.persistence_entities_without_timeline())
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

fn seed_workspace_session_and_live(state: &mut ControlPlaneState) {
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
