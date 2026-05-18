use lucarne::LucarneDaemon;
use tempfile::TempDir;

#[tokio::test]
async fn daemon_opens_default_runtime_and_sqlite_control_plane_store() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite3");

    let daemon = LucarneDaemon::open_sqlite(&db).expect("open daemon");

    let runtime_provider_ids = daemon
        .runtime()
        .providers()
        .into_iter()
        .map(|provider| provider.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(runtime_provider_ids.as_slice(), daemon.provider_ids());

    let store = daemon
        .control_plane_store()
        .expect("sqlite daemon exposes control-plane store");
    assert!(store.load_control_plane().unwrap().is_none());
}
