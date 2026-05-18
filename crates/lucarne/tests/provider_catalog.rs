use lucarne::{core_service::HistoryWatchState, LucarneCore};
use tempfile::TempDir;

#[tokio::test]
async fn empty_provider_filter_disables_history_watch_providers() {
    let tmp = TempDir::new().unwrap();
    let enabled = Vec::<String>::new();
    let core =
        LucarneCore::open_sqlite_with_provider_filter(tmp.path().join("state.sqlite3"), &enabled)
            .expect("open empty-filtered core");
    let _events = core.watch_events();

    core.start_history_session_watch()
        .expect("start empty-filtered watch loop");

    let status = core.history_watch_status();
    assert_eq!(status.state, HistoryWatchState::Degraded);
    assert_eq!(
        status.last_error.as_deref(),
        Some("provider roots unavailable")
    );
}

#[tokio::test]
async fn filtered_provider_catalog_registers_only_enabled_agents_and_skips_unknown() {
    let tmp = TempDir::new().unwrap();
    let enabled = vec![
        "codex".to_string(),
        "unknown-agent".to_string(),
        "pi".to_string(),
    ];
    let core =
        LucarneCore::open_sqlite_with_provider_filter(tmp.path().join("state.sqlite3"), &enabled)
            .expect("open filtered core");

    assert_eq!(core.provider_ids(), &["codex", "pi"]);
    assert_eq!(
        core.provider_catalog()
            .iter()
            .map(|entry| entry.provider_id)
            .collect::<Vec<_>>(),
        vec!["codex", "pi"]
    );
    assert_eq!(core.history_provider_ids(), &["codex", "pi"]);
    assert_eq!(
        core.history_provider_catalog()
            .iter()
            .map(|entry| entry.provider_id)
            .collect::<Vec<_>>(),
        vec!["codex", "pi"]
    );
}

#[tokio::test]
async fn empty_provider_filter_disables_all_agents_and_history() {
    let tmp = TempDir::new().unwrap();
    let enabled = Vec::<String>::new();
    let core =
        LucarneCore::open_sqlite_with_provider_filter(tmp.path().join("state.sqlite3"), &enabled)
            .expect("open empty-filtered core");

    assert!(core.provider_ids().is_empty());
    assert!(core.provider_catalog().is_empty());
    assert!(core.history_provider_ids().is_empty());
    assert!(core.history_provider_catalog().is_empty());
}

#[tokio::test]
async fn provider_catalog_is_reported_by_core_not_telegram_path_scans() {
    let tmp = TempDir::new().unwrap();
    let core = LucarneCore::open_sqlite(tmp.path().join("state.sqlite3")).expect("open core");
    let catalog = core.provider_catalog();

    assert_eq!(
        catalog
            .iter()
            .map(|entry| entry.provider_id)
            .collect::<Vec<_>>(),
        core.provider_ids()
    );
}
