use super::*;
use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(any(feature = "codex", all(feature = "claude", target_os = "macos")))]
use std::time::SystemTime;

fn watch_provider(id: &str) -> WatchProvider {
    crate::agent_provider(id).expect("watch provider")
}

#[cfg(feature = "codex")]
fn codex_root(base: &Path) -> PathBuf {
    base.join("codex-home").join("sessions")
}

#[cfg(feature = "codex")]
fn codex_session_path(root: &Path) -> PathBuf {
    root.join("2026")
        .join("05")
        .join("03")
        .join("rollout-test.jsonl")
}

#[cfg(feature = "codex")]
fn write_initial_codex_session(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        concat!(
            r#"{"timestamp":"2026-05-03T00:00:00.000Z","type":"session_meta","payload":{"session_id":"sess-watch","cwd":"/tmp/project","model":"gpt-5"}}"#,
            "\n",
            r#"{"timestamp":"2026-05-03T00:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"ping"}]}}"#,
            "\n",
        ),
    )
    .unwrap();
}

#[cfg(feature = "codex")]
fn write_large_codex_session_meta(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let base_instructions = "x".repeat(24 * 1024);
    let line = serde_json::json!({
        "timestamp": "2026-05-03T00:00:00.000Z",
        "type": "session_meta",
        "payload": {
            "id": "sess-large-meta",
            "timestamp": "2026-05-03T00:00:00.000Z",
            "cwd": "/tmp/project",
            "originator": "codex-tui",
            "cli_version": "0.130.0",
            "base_instructions": base_instructions,
        }
    })
    .to_string();
    assert!(line.len() > 2 * 1024);
    assert!(line.len() < (MAX_WATCH_METADATA_READ_BYTES * MAX_WATCH_METADATA_READ_CHUNKS) as usize);
    fs::write(path, format!("{line}\n")).unwrap();
}

fn append_line(path: &Path, line: &str) {
    let mut file = OpenOptions::new().append(true).open(path).unwrap();
    writeln!(file, "{line}").unwrap();
    file.sync_all().unwrap();
}

#[cfg(feature = "codex")]
fn test_watcher(
    root: &Path,
    debounce: Duration,
) -> (SessionWatcher, mpsc::UnboundedSender<RawWatchEvent>) {
    test_watcher_for(
        watch_provider("codex"),
        root,
        crate::ParseSelection::empty().with_messages(),
        debounce,
    )
}

fn test_watcher_for(
    provider: WatchProvider,
    root: &Path,
    selection: crate::ParseSelection,
    debounce: Duration,
) -> (SessionWatcher, mpsc::UnboundedSender<RawWatchEvent>) {
    let (tx, raw_rx) = mpsc::unbounded_channel();
    let root = fs::canonicalize(root).unwrap();
    let mut watcher = SessionWatcher {
        config: WatchConfig::new()
            .providers([provider])
            .provider_roots(provider, [root.clone()])
            .selection(selection)
            .debounce(debounce),
        raw_rx,
        _watcher: None,
        #[cfg(target_os = "macos")]
        _recursive_watcher: None,
        watched_paths: HashSet::new(),
        baselines: HashMap::new(),
        providers_by_path: vec![(root, provider)],
        pending_paths: HashSet::new(),
        pending_updates: std::collections::VecDeque::new(),
        quiet_until: None,
        quiet_sleep: None,
        disconnected: false,
    };
    watcher.initialize_baselines();
    (watcher, tx)
}

#[test]
fn watcher_start_debug_log_avoids_formatting_full_provider_descriptors() {
    let source = include_str!("mod.rs");
    let start_section = source
        .split("pub fn start(config: WatchConfig)")
        .nth(1)
        .expect("SessionWatcher::start should exist")
        .split("let (raw_tx, raw_rx)")
        .next()
        .expect("start logging should precede raw channel setup");

    assert!(start_section.contains("provider_ids = ?WatchProviderIds(&config.providers)"));
    assert!(!start_section.contains("providers = ?config.providers"));
    assert!(!start_section.contains("collect::<Vec<_>>()"));
    for label in [
        "agent_sessions.watch.start",
        "agent_sessions.watch.after_recommended_watcher",
        "agent_sessions.watch.after_roots",
        "agent_sessions.watch.after_initialize_baselines",
    ] {
        let needle = format!("crate::memory_profile_snapshot!(\"{label}\")");
        assert!(source.contains(&needle), "missing snapshot {label}");
    }
}

#[test]
fn watch_config_scan_interval_available_for_backward_compat() {
    // scan_interval is kept in the config for backward compat but no
    // longer drives a periodic scan loop.
    assert_eq!(WatchConfig::new().scan_interval, Duration::from_secs(60));
}

#[test]
fn watch_read_chunks_are_capped_at_two_kib() {
    assert_eq!(MAX_WATCH_METADATA_READ_BYTES, 2 * 1024);
    assert_eq!(MAX_WATCH_METADATA_READ_CHUNKS, 16);
    assert_eq!(MAX_WATCH_READ_BYTES, MAX_WATCH_METADATA_READ_BYTES);
}

#[test]
fn watch_source_avoids_full_file_reads() {
    let watch_source = include_str!("mod.rs");
    let provider_source = include_str!("provider.rs");
    for forbidden in [
        "read_full(",
        "process_reparsed_path",
        "read_provider_file",
        "read_provider_metadata(provider, path)",
        "std::fs::read(path)",
        ".with_inline_data(Bytes::from(bytes))",
        "CandidateEntry::new(path.to_path_buf())",
    ] {
        assert!(
            !watch_source.contains(forbidden) && !provider_source.contains(forbidden),
            "watch path must stay incremental/bounded; found {forbidden}"
        );
    }
    assert!(!watch_source.contains("64 * 1024"));
    assert!(watch_source.contains("MAX_WATCH_METADATA_READ_BYTES"));
    assert!(
        watch_source.contains("fn metadata_reader"),
        "watch metadata reads must stay bounded"
    );
    assert!(
        watch_source.contains("fn read_bounded_lookback"),
        "watch state seeding must stay bounded"
    );
    assert!(
        provider_source.contains("fn parse_provider_metadata_bytes"),
        "watch metadata must use bounded metadata bytes"
    );
}

#[test]
fn watch_discovery_filters_during_provider_iteration() {
    let watch_source = include_str!("mod.rs");
    let provider_source = include_str!("provider.rs");
    let agent_source = include_str!("../agent.rs");
    let descriptor_source = include_str!("../providers/descriptor.rs");

    assert!(
        agent_source.contains("fn discover_recent_in"),
        "provider discovery contract should accept watch recency filters before building source lists"
    );
    assert!(
        descriptor_source.contains("A::discover_recent_in"),
        "watch descriptor should dispatch to provider-filtered recent discovery"
    );
    assert!(
        provider_source.contains("fn discover_provider_session_files_into"),
        "watch provider boundary should expose callback-based session discovery"
    );
    assert!(
        watch_source.contains("discover_provider_session_files_into"),
        "watch startup should stream provider discoveries and filter before storing paths"
    );
    assert!(
        !watch_source.contains("discover_provider_session_files(*provider, root)")
            && !watch_source
                .contains(".extend(\n                        discover_provider_session_files"),
        "watch startup must not eagerly collect all provider session files before recency filtering"
    );
}

#[cfg(any(feature = "codex", all(feature = "claude", target_os = "macos")))]
fn set_modified(path: &Path, modified: SystemTime) {
    filetime::set_file_mtime(path, filetime::FileTime::from_system_time(modified)).unwrap();
}

#[cfg(any(
    feature = "codex",
    feature = "claude",
    feature = "copilot",
    feature = "cursor"
))]
fn assert_single_assistant_response(updates: &[WatchUpdate], expected: &str) {
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].change, WatchChange::Updated);
    assert!(
        updates[0].events.iter().any(|event| {
            matches!(
                event,
                WatchEvent::AssistantMessage(message)
                    if message.text.as_deref() == Some(expected)
            )
        }),
        "watch update should include assistant response {expected:?}"
    );
}

async fn recv_timeout(
    watcher: &mut SessionWatcher,
    timeout: Duration,
) -> std::result::Result<Box<[WatchUpdate]>, WatchError> {
    use futures::StreamExt;

    match tokio::time::timeout(timeout, watcher.next()).await {
        Ok(Some(result)) => result,
        Ok(None) => Err(WatchError::Disconnected),
        Err(_) => Ok(Vec::new().into_boxed_slice()),
    }
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn baselines_existing_codex_session_without_emitting() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let session_path = codex_session_path(&root);
    write_initial_codex_session(&session_path);

    let (mut watcher, _tx) = test_watcher(&root, Duration::from_millis(50));

    let updates = recv_timeout(&mut watcher, Duration::from_millis(60))
        .await
        .unwrap();
    assert!(
        updates.is_empty(),
        "existing messages must not be emitted on startup"
    );
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn discovery_filters_directory_roots_to_recent_session_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let recent_path = codex_session_path(&root);
    let stale_path = root
        .join("2026")
        .join("05")
        .join("02")
        .join("rollout-stale.jsonl");
    write_initial_codex_session(&recent_path);
    write_initial_codex_session(&stale_path);
    let stale_modified = SystemTime::now() - Duration::from_secs(3 * 24 * 60 * 60);
    set_modified(&stale_path, stale_modified);

    let (watcher, _tx) = test_watcher(&root, Duration::from_millis(50));
    let discovered = watcher.discover_session_files();
    let watch_paths = watcher.initial_watch_paths(&[fs::canonicalize(&root).unwrap()]);

    let recent_path = fs::canonicalize(&recent_path).unwrap();
    let stale_path = fs::canonicalize(&stale_path).unwrap();
    assert!(discovered.contains(&recent_path));
    assert!(!discovered.contains(&stale_path));
    assert!(watch_paths.contains(&recent_path));
    assert!(!watch_paths.contains(&stale_path));
}

#[cfg(all(feature = "codex", not(target_os = "macos")))]
#[tokio::test]
async fn initial_watch_paths_include_only_recent_codex_day_directories() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let day_dir = root.join("2026").join("05").join("08");
    let stale_day_dir = root.join("2026").join("05").join("01");
    fs::create_dir_all(&day_dir).unwrap();
    fs::create_dir_all(&stale_day_dir).unwrap();
    let stale_modified = SystemTime::now() - Duration::from_secs(3 * 24 * 60 * 60);
    set_modified(&stale_day_dir, stale_modified);

    let (watcher, _tx) = test_watcher(&root, Duration::from_millis(50));
    let watch_paths = watcher.initial_watch_paths(&[fs::canonicalize(&root).unwrap()]);

    assert!(watch_paths.contains(&fs::canonicalize(root.join("2026")).unwrap()));
    assert!(watch_paths.contains(&fs::canonicalize(root.join("2026/05")).unwrap()));
    assert!(watch_paths.contains(&fs::canonicalize(day_dir).unwrap()));
    assert!(!watch_paths.contains(&fs::canonicalize(stale_day_dir).unwrap()));
}

#[cfg(all(feature = "codex", target_os = "macos"))]
#[tokio::test]
async fn macos_codex_root_uses_recursive_watch_and_hot_session_file_targets() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let hot_path = codex_session_path(&root);
    let cold_recent_path = root
        .join("2026")
        .join("05")
        .join("02")
        .join("rollout-cold-recent.jsonl");
    let stale_path = root
        .join("2026")
        .join("05")
        .join("01")
        .join("rollout-stale.jsonl");
    write_initial_codex_session(&hot_path);
    write_initial_codex_session(&cold_recent_path);
    write_initial_codex_session(&stale_path);
    let cold_recent_modified = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    set_modified(&cold_recent_path, cold_recent_modified);
    let stale_modified = SystemTime::now() - Duration::from_secs(3 * 24 * 60 * 60);
    set_modified(&stale_path, stale_modified);

    let (watcher, _tx) = test_watcher(&root, Duration::from_millis(50));
    let targets = watcher.initial_watch_targets(&[fs::canonicalize(&root).unwrap()]);

    let root = fs::canonicalize(&root).unwrap();
    let hot_path = fs::canonicalize(&hot_path).unwrap();
    let cold_recent_path = fs::canonicalize(&cold_recent_path).unwrap();
    let stale_path = fs::canonicalize(&stale_path).unwrap();
    assert!(targets.contains(&WatchTarget {
        path: root.clone(),
        recursive_mode: RecursiveMode::Recursive,
    }));
    assert!(targets.contains(&WatchTarget {
        path: hot_path,
        recursive_mode: RecursiveMode::NonRecursive,
    }));
    assert!(!targets.iter().any(|target| target.path == cold_recent_path));
    assert!(!targets.iter().any(|target| target.path == stale_path));
    assert!(
        !targets
            .iter()
            .any(|target| target.path == root.join("2026"))
    );
    assert!(
        !targets
            .iter()
            .any(|target| target.path == root.join("2026").join("05"))
    );
}

#[cfg(all(feature = "claude", not(target_os = "macos")))]
#[tokio::test]
async fn initial_watch_paths_do_not_register_every_claude_project_dir() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("projects");
    let recent_project = root.join("-tmp-recent");
    let empty_project = root.join("-tmp-empty");
    fs::create_dir_all(&recent_project).unwrap();
    fs::create_dir_all(&empty_project).unwrap();
    fs::write(recent_project.join("recent.jsonl"), "{}\n").unwrap();

    let (watcher, _tx) = test_watcher_for(
        watch_provider("claude"),
        &root,
        crate::ParseSelection::empty().with_messages(),
        Duration::from_millis(50),
    );
    let watch_paths = watcher.initial_watch_paths(&[fs::canonicalize(&root).unwrap()]);

    assert!(watch_paths.contains(&fs::canonicalize(&root).unwrap()));
    assert!(watch_paths.contains(&fs::canonicalize(recent_project).unwrap()));
    assert!(!watch_paths.contains(&fs::canonicalize(empty_project).unwrap()));
}

#[cfg(all(feature = "claude", target_os = "macos"))]
#[tokio::test]
async fn macos_claude_root_uses_recursive_watch_and_hot_session_file_targets() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("projects");
    let hot_project = root.join("-tmp-hot");
    let cold_recent_project = root.join("-tmp-cold-recent");
    let empty_project = root.join("-tmp-empty");
    let hot_file = hot_project.join("hot.jsonl");
    let cold_recent_file = cold_recent_project.join("cold-recent.jsonl");
    fs::create_dir_all(&hot_project).unwrap();
    fs::create_dir_all(&cold_recent_project).unwrap();
    fs::create_dir_all(&empty_project).unwrap();
    fs::write(&hot_file, "{}\n").unwrap();
    fs::write(&cold_recent_file, "{}\n").unwrap();
    let cold_recent_modified = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    set_modified(&cold_recent_file, cold_recent_modified);

    let (watcher, _tx) = test_watcher_for(
        watch_provider("claude"),
        &root,
        crate::ParseSelection::empty().with_messages(),
        Duration::from_millis(50),
    );
    let targets = watcher.initial_watch_targets(&[fs::canonicalize(&root).unwrap()]);
    let root = fs::canonicalize(&root).unwrap();
    let hot_project = fs::canonicalize(hot_project).unwrap();
    let cold_recent_project = fs::canonicalize(cold_recent_project).unwrap();
    let cold_recent_file = fs::canonicalize(cold_recent_file).unwrap();
    let empty_project = fs::canonicalize(empty_project).unwrap();

    assert!(targets.contains(&WatchTarget {
        path: root.clone(),
        recursive_mode: RecursiveMode::Recursive,
    }));
    assert!(targets.contains(&WatchTarget {
        path: fs::canonicalize(hot_file).unwrap(),
        recursive_mode: RecursiveMode::NonRecursive,
    }));
    assert!(!targets.iter().any(|target| target.path == hot_project));
    assert!(
        !targets
            .iter()
            .any(|target| target.path == cold_recent_project)
    );
    assert!(!targets.iter().any(|target| target.path == cold_recent_file));
    assert!(!targets.iter().any(|target| target.path == empty_project));
}

#[cfg(feature = "claude")]
#[tokio::test]
async fn ignores_claude_subagent_watch_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("projects");
    let subagent = root
        .join("-tmp-project")
        .join("subagents")
        .join("agent-a.jsonl");
    fs::create_dir_all(subagent.parent().unwrap()).unwrap();
    fs::write(
        &subagent,
        r#"{"type":"assistant","timestamp":"2026-05-03T00:00:02.000Z","message":{"id":"a1","model":"claude","content":[{"type":"text","text":"subagent output"}]}}"#,
    )
    .unwrap();

    let canonical = fs::canonicalize(&subagent).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("claude"),
        &root,
        crate::ParseSelection::empty().with_messages(),
        Duration::from_millis(5),
    );
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_millis(50))
        .await
        .unwrap();
    assert!(updates.is_empty());
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn emits_appended_codex_assistant_response_after_debounce() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let session_path = codex_session_path(&root);
    write_initial_codex_session(&session_path);
    let canonical_session_path = fs::canonicalize(&session_path).unwrap();

    let (mut watcher, tx) = test_watcher(&root, Duration::from_millis(20));

    append_line(
        &session_path,
        r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"pong"}]}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical_session_path.clone()]))
        .unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].provider, watch_provider("codex"));
    assert_eq!(updates[0].path, canonical_session_path);
    assert_eq!(updates[0].session_id.as_deref(), Some("sess-watch"));
    assert_eq!(updates[0].cwd.as_deref(), Some("/tmp/project"));
    assert_eq!(updates[0].change, WatchChange::Updated);
    assert_eq!(updates[0].events.len(), 1);
    let event = &updates[0].events[0];
    assert!(matches!(
        event,
        WatchEvent::AssistantMessage(message) if message.text.as_deref() == Some("pong")
    ));
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn configured_unsubscribed_codex_path_skips_initial_state_seed_and_delta_parse() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let session_path = codex_session_path(&root);
    write_initial_codex_session(&session_path);
    let canonical_root = fs::canonicalize(&root).unwrap();
    let canonical_session_path = fs::canonicalize(&session_path).unwrap();
    let (tx, raw_rx) = mpsc::unbounded_channel();
    let mut watcher = SessionWatcher {
        config: WatchConfig::new()
            .providers([watch_provider("codex")])
            .provider_roots(watch_provider("codex"), [canonical_root.clone()])
            .selection(crate::ParseSelection::empty().with_messages())
            .debounce(Duration::from_millis(10))
            .scan_interval(Duration::from_secs(30))
            .subscribed_paths(std::iter::empty::<PathBuf>()),
        raw_rx,
        _watcher: None,
        #[cfg(target_os = "macos")]
        _recursive_watcher: None,
        watched_paths: HashSet::new(),
        baselines: HashMap::new(),
        providers_by_path: vec![(canonical_root, watch_provider("codex"))],
        pending_paths: HashSet::new(),
        pending_updates: std::collections::VecDeque::new(),
        quiet_until: None,
        quiet_sleep: None,
        disconnected: false,
    };
    watcher.initialize_baselines();
    assert!(
        !watcher
            .baselines
            .get(&canonical_session_path)
            .expect("initial codex baseline")
            .has_subscriber,
        "configured unsubscribed path should start without subscriber"
    );

    append_line(
        &session_path,
        r#"{"this is not valid codex session jsonl":"#,
    );
    let appended_len = fs::metadata(&session_path).unwrap().len();
    tx.send(RawWatchEvent::Paths(vec![canonical_session_path.clone()]))
        .unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(updates.is_empty());
    assert_eq!(
        watcher
            .baselines
            .get(&canonical_session_path)
            .expect("advanced baseline")
            .len,
        appended_len
    );
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn unsubscribed_codex_delta_advances_baseline_without_parsing() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let session_path = codex_session_path(&root);
    write_initial_codex_session(&session_path);
    let canonical_session_path = fs::canonicalize(&session_path).unwrap();

    let (mut watcher, tx) = test_watcher(&root, Duration::from_millis(10));
    watcher
        .baselines
        .get_mut(&canonical_session_path)
        .expect("initial codex baseline")
        .has_subscriber = false;

    append_line(
        &session_path,
        r#"{"this is not valid codex session jsonl":"#,
    );
    let appended_len = fs::metadata(&session_path).unwrap().len();
    tx.send(RawWatchEvent::Paths(vec![canonical_session_path.clone()]))
        .unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(
        updates.is_empty(),
        "unsubscribed delta should not be parsed or emitted: {updates:?}"
    );
    let baseline = watcher
        .baselines
        .get(&canonical_session_path)
        .expect("advanced codex baseline");
    assert_eq!(baseline.len, appended_len);
    assert_eq!(baseline.session_id.as_deref(), Some("sess-watch"));
    assert_eq!(baseline.cwd.as_deref(), Some("/tmp/project"));
    assert!(baseline.pending_partial.is_empty());
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn created_codex_jsonl_emits_latest_complete_record_without_replaying_history() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    fs::create_dir_all(&root).unwrap();
    let (mut watcher, tx) = test_watcher(&root, Duration::from_millis(10));

    let session_path = codex_session_path(&root);
    write_initial_codex_session(&session_path);
    append_line(
        &session_path,
        r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"historical pong"}]}}"#,
    );
    let canonical_session_path = fs::canonicalize(&session_path).unwrap();
    tx.send(RawWatchEvent::Paths(vec![canonical_session_path.clone()]))
        .unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].provider, watch_provider("codex"));
    assert_eq!(updates[0].change, WatchChange::Created);
    assert_eq!(updates[0].session_id.as_deref(), Some("sess-watch"));
    assert_eq!(updates[0].cwd.as_deref(), Some("/tmp/project"));
    assert!(
        updates[0].events.iter().any(|event| {
            matches!(
                event,
                WatchEvent::AssistantMessage(message)
                    if message.text.as_deref() == Some("historical pong")
            )
        }),
        "created JSONL tail should include latest assistant response"
    );
    assert!(
        updates[0].events.iter().all(|event| {
            !matches!(event, WatchEvent::UserMessage(message) if message.text.as_deref() == Some("ping"))
        }),
        "new JSONL tail reads must not replay older user prompt records"
    );

    append_line(
        &session_path,
        r#"{"timestamp":"2026-05-03T00:00:03.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"live pong"}]}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical_session_path]))
        .unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_single_assistant_response(&updates, "live pong");
    assert_eq!(updates[0].session_id.as_deref(), Some("sess-watch"));
    assert_eq!(updates[0].cwd.as_deref(), Some("/tmp/project"));
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn created_codex_jsonl_reads_large_session_meta() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    fs::create_dir_all(&root).unwrap();
    let (mut watcher, tx) = test_watcher(&root, Duration::from_millis(10));

    let session_path = codex_session_path(&root);
    write_large_codex_session_meta(&session_path);
    let canonical_session_path = fs::canonicalize(&session_path).unwrap();
    tx.send(RawWatchEvent::Paths(vec![canonical_session_path.clone()]))
        .unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].provider, watch_provider("codex"));
    assert_eq!(updates[0].change, WatchChange::Created);
    assert_eq!(updates[0].session_id.as_deref(), Some("sess-large-meta"));
    assert_eq!(updates[0].cwd.as_deref(), Some("/tmp/project"));
    assert!(updates[0].events.is_empty());
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn session_watcher_exposes_watch_updates_as_stream() {
    use futures::StreamExt;

    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let session_path = codex_session_path(&root);
    write_initial_codex_session(&session_path);
    let canonical_session_path = fs::canonicalize(&session_path).unwrap();

    let (mut watcher, tx) = test_watcher(&root, Duration::from_millis(20));

    append_line(
        &session_path,
        r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"stream pong"}]}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical_session_path]))
        .unwrap();

    let updates = tokio::time::timeout(Duration::from_secs(1), watcher.next())
        .await
        .expect("stream item")
        .expect("watch stream open")
        .expect("watch update");
    assert_single_assistant_response(&updates, "stream pong");
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn dedupes_codex_agent_message_mirrored_by_response_item() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let session_path = codex_session_path(&root);
    write_initial_codex_session(&session_path);
    let canonical_session_path = fs::canonicalize(&session_path).unwrap();

    let (mut watcher, tx) = test_watcher(&root, Duration::from_millis(20));

    append_line(
        &session_path,
        r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"event_msg","payload":{"type":"agent_message","message":"pong","phase":"final_answer","memory_citation":null}}"#,
    );
    append_line(
        &session_path,
        r#"{"timestamp":"2026-05-03T00:00:02.002Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"pong"}],"phase":"final_answer"}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical_session_path]))
        .unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_single_assistant_response(&updates, "pong");
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn dedupes_codex_mirrored_assistant_response_across_deltas() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let session_path = codex_session_path(&root);
    write_initial_codex_session(&session_path);
    let canonical_session_path = fs::canonicalize(&session_path).unwrap();

    let (mut watcher, tx) = test_watcher(&root, Duration::from_millis(20));

    append_line(
        &session_path,
        r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"event_msg","payload":{"type":"agent_message","message":"pong","phase":"final_answer","memory_citation":null}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical_session_path.clone()]))
        .unwrap();
    let first = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_single_assistant_response(&first, "pong");

    append_line(
        &session_path,
        r#"{"timestamp":"2026-05-03T00:00:02.002Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"pong"}],"phase":"final_answer"}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical_session_path]))
        .unwrap();
    let second = recv_timeout(&mut watcher, Duration::from_millis(100))
        .await
        .unwrap();

    assert!(
        second.is_empty(),
        "mirrored assistant response should not emit a second watch update: {second:?}"
    );
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn dedupes_codex_mirrored_assistant_response_from_created_baseline() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let session_path = codex_session_path(&root);
    write_initial_codex_session(&session_path);
    append_line(
        &session_path,
        r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"event_msg","payload":{"type":"agent_message","message":"baseline pong","phase":"final_answer","memory_citation":null}}"#,
    );
    let canonical_session_path = fs::canonicalize(&session_path).unwrap();

    let (mut watcher, tx) = test_watcher(&root, Duration::from_millis(20));

    append_line(
        &session_path,
        r#"{"timestamp":"2026-05-03T00:00:02.002Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"baseline pong"}],"phase":"final_answer"}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical_session_path]))
        .unwrap();
    let updates = recv_timeout(&mut watcher, Duration::from_millis(100))
        .await
        .unwrap();

    assert!(
        updates.is_empty(),
        "baseline mirrored assistant response should not emit a watch update: {updates:?}"
    );
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn debounces_multiple_appends_into_one_update() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let session_path = codex_session_path(&root);
    write_initial_codex_session(&session_path);
    let canonical_session_path = fs::canonicalize(&session_path).unwrap();

    let (mut watcher, tx) = test_watcher(&root, Duration::from_millis(30));

    append_line(
        &session_path,
        r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"one"}]}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical_session_path.clone()]))
        .unwrap();
    append_line(
        &session_path,
        r#"{"timestamp":"2026-05-03T00:00:03.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"two"}]}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical_session_path]))
        .unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].events.len(), 2);
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn buffers_partial_lines_without_advancing_semantic_events() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let session_path = codex_session_path(&root);
    write_initial_codex_session(&session_path);
    let canonical_session_path = fs::canonicalize(&session_path).unwrap();

    let (mut watcher, tx) = test_watcher(&root, Duration::from_millis(10));

    {
        let mut file = OpenOptions::new().append(true).open(&session_path).unwrap();
        file.write_all(
            r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"partial"}]}}"#
                .as_bytes(),
        )
        .unwrap();
        file.sync_all().unwrap();
    }
    tx.send(RawWatchEvent::Paths(vec![canonical_session_path.clone()]))
        .unwrap();
    assert!(
        recv_timeout(&mut watcher, Duration::from_millis(40))
            .await
            .unwrap()
            .is_empty()
    );

    {
        let mut file = OpenOptions::new().append(true).open(&session_path).unwrap();
        writeln!(file).unwrap();
        file.sync_all().unwrap();
    }
    tx.send(RawWatchEvent::Paths(vec![canonical_session_path]))
        .unwrap();
    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].events.len(), 1);
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn truncate_emits_no_historical_events() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let session_path = codex_session_path(&root);
    write_initial_codex_session(&session_path);
    let canonical_session_path = fs::canonicalize(&session_path).unwrap();

    let (mut watcher, tx) = test_watcher(&root, Duration::from_millis(10));

    fs::write(&session_path, "").unwrap();
    tx.send(RawWatchEvent::Paths(vec![canonical_session_path]))
        .unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].change, WatchChange::Truncated);
    assert!(updates[0].events.is_empty());
}

#[cfg(feature = "claude")]
#[tokio::test]
async fn emits_appended_claude_assistant_response() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("claude.jsonl");
    fs::write(
        &path,
        concat!(
            r#"{"type":"user","timestamp":"2026-05-03T00:00:01.000Z","message":{"id":"u1","content":[{"type":"text","text":"ping"}]}}"#,
            "\n",
        ),
    )
    .unwrap();
    let canonical = fs::canonicalize(&path).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("claude"),
        &path,
        crate::ParseSelection::empty().with_messages(),
        Duration::from_millis(10),
    );

    append_line(
        &path,
        r#"{"type":"assistant","timestamp":"2026-05-03T00:00:02.000Z","message":{"id":"a1","model":"claude","content":[{"type":"text","text":"claude pong"}]}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_single_assistant_response(&updates, "claude pong");
}

#[cfg(feature = "claude")]
#[tokio::test]
async fn emits_repeated_claude_assistant_response_across_deltas() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("claude.jsonl");
    fs::write(
        &path,
        concat!(
            r#"{"type":"user","timestamp":"2026-05-03T00:00:01.000Z","message":{"id":"u1","content":[{"type":"text","text":"ping"}]}}"#,
            "\n",
        ),
    )
    .unwrap();
    let canonical = fs::canonicalize(&path).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("claude"),
        &path,
        crate::ParseSelection::empty().with_messages(),
        Duration::from_millis(10),
    );

    append_line(
        &path,
        r#"{"type":"assistant","timestamp":"2026-05-03T00:00:02.000Z","message":{"id":"a1","model":"claude","content":[{"type":"text","text":"same"}]}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical.clone()]))
        .unwrap();
    let first = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_single_assistant_response(&first, "same");

    append_line(
        &path,
        r#"{"type":"assistant","timestamp":"2026-05-03T00:00:03.000Z","message":{"id":"a2","model":"claude","content":[{"type":"text","text":"same"}]}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();
    let second = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_single_assistant_response(&second, "same");
}

#[cfg(feature = "claude")]
#[tokio::test]
async fn claude_watch_task_complete_includes_elapsed_duration_across_deltas() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("claude.jsonl");
    fs::write(&path, "").unwrap();
    let canonical = fs::canonicalize(&path).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("claude"),
        &path,
        crate::ParseSelection::full(),
        Duration::from_millis(10),
    );

    append_line(
        &path,
        r#"{"type":"user","sessionId":"claude-duration","cwd":"/tmp/project","timestamp":"2026-05-03T00:00:01.000Z","message":{"id":"u1","content":[{"type":"text","text":"ping"}]}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical.clone()]))
        .unwrap();
    let user_updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(user_updates.len(), 1);

    append_line(
        &path,
        r#"{"type":"assistant","sessionId":"claude-duration","cwd":"/tmp/project","timestamp":"2026-05-03T00:00:16.000Z","message":{"id":"a1","model":"claude","stop_reason":"end_turn","content":[{"type":"text","text":"claude pong"}]}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();
    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(updates.len(), 1);

    let completion = updates[0]
        .events
        .iter()
        .find_map(|event| match event {
            WatchEvent::TurnCompleted(completion) => Some(completion),
            _ => None,
        })
        .expect("claude terminal message should emit task_complete");
    assert_eq!(
        completion.last_agent_message.as_deref(),
        Some("claude pong")
    );
    assert_eq!(completion.duration_ms, Some(15_000));

    assert!(
        updates[0].events.iter().any(|event| {
            matches!(
                event,
                WatchEvent::AssistantMessage(message)
                    if message.text.as_deref() == Some("claude pong")
                        && message.phase.as_deref() == Some("final_answer")
            )
        }),
        "claude response should stay in transcript but not be projected as a duplicate notification"
    );
}

#[cfg(feature = "claude")]
#[tokio::test]
async fn streams_claude_terminal_line_larger_than_single_read_window() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("claude.jsonl");
    fs::write(
        &path,
        concat!(
            r#"{"type":"user","sessionId":"claude-large-line","cwd":"/tmp/project","timestamp":"2026-05-03T00:00:01.000Z","message":{"id":"u1","content":[{"type":"text","text":"ping"}]}}"#,
            "\n",
        ),
    )
    .unwrap();
    let canonical = fs::canonicalize(&path).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("claude"),
        &path,
        crate::ParseSelection::full(),
        Duration::from_millis(10),
    );
    let text = "claude-done".repeat((MAX_WATCH_READ_BYTES as usize / 11) + 128);
    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": "claude-large-line",
        "cwd": "/tmp/project",
        "timestamp": "2026-05-03T00:00:16.000Z",
        "message": {
            "id": "a-large",
            "model": "claude",
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": text}],
        },
    })
    .to_string();
    assert!(line.len() > MAX_WATCH_READ_BYTES as usize);

    append_line(&path, &line);
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(updates.len(), 1);
    let completion = updates[0]
        .events
        .iter()
        .find_map(|event| match event {
            WatchEvent::TurnCompleted(completion) => Some(completion),
            _ => None,
        })
        .expect("large Claude terminal line should be streamed across read chunks");
    assert_eq!(
        completion.last_agent_message.as_deref(),
        Some(text.as_str())
    );
}

#[cfg(feature = "claude")]
#[tokio::test]
async fn message_selection_suppresses_appended_claude_usage() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("claude.jsonl");
    fs::write(
        &path,
        concat!(
            r#"{"type":"user","timestamp":"2026-05-03T00:00:01.000Z","message":{"id":"u1","content":[{"type":"text","text":"ping"}]}}"#,
            "\n",
        ),
    )
    .unwrap();
    let canonical = fs::canonicalize(&path).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("claude"),
        &path,
        crate::ParseSelection::empty().with_messages(),
        Duration::from_millis(10),
    );

    append_line(
        &path,
        r#"{"type":"assistant","timestamp":"2026-05-03T00:00:02.000Z","message":{"id":"a1","model":"claude","content":[{"type":"text","text":"claude pong"}],"usage":{"input_tokens":100,"output_tokens":30,"cache_creation_input_tokens":5,"cache_read_input_tokens":7,"server_tool_use":{"web_search_requests":2},"speed":"fast"}}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_single_assistant_response(&updates, "claude pong");
    assert!(
        updates[0]
            .events
            .iter()
            .all(|event| !matches!(event, WatchEvent::Usage(_))),
        "message-only Claude watch selection must not emit usage"
    );
}

#[cfg(feature = "claude")]
#[tokio::test]
async fn message_selection_suppresses_appended_claude_state() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("claude.jsonl");
    fs::write(
        &path,
        concat!(
            r#"{"type":"user","timestamp":"2026-05-03T00:00:01.000Z","message":{"id":"u1","content":[{"type":"text","text":"ping"}]}}"#,
            "\n",
        ),
    )
    .unwrap();
    let canonical = fs::canonicalize(&path).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("claude"),
        &path,
        crate::ParseSelection::empty().with_messages(),
        Duration::from_millis(10),
    );

    append_line(
        &path,
        r#"{"type":"system","timestamp":"2026-05-03T00:00:02.000Z","subtype":"permission_mode","level":"info"}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_millis(100))
        .await
        .unwrap();
    assert!(
        updates.is_empty(),
        "message-only Claude watch selection must not emit state"
    );
}

#[cfg(feature = "pi")]
#[tokio::test]
async fn pi_watch_task_complete_includes_elapsed_duration_across_deltas() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pi.jsonl");
    fs::write(
        &path,
        concat!(
            r#"{"type":"session","version":3,"id":"pi-duration","timestamp":"2026-05-03T00:00:00.000Z","cwd":"/tmp/project"}"#,
            "\n",
        ),
    )
    .unwrap();
    let canonical = fs::canonicalize(&path).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("pi"),
        &path,
        crate::ParseSelection::full(),
        Duration::from_millis(10),
    );

    append_line(
        &path,
        r#"{"type":"message","id":"u1","timestamp":"2026-05-03T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"ping"}]}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical.clone()]))
        .unwrap();
    let user_updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(user_updates.len(), 1);

    append_line(
        &path,
        r#"{"type":"message","id":"a1","parentId":"u1","timestamp":"2026-05-03T00:00:15.000Z","message":{"role":"assistant","model":"deepseek-v4-pro","stopReason":"stop","content":[{"type":"thinking","thinking":"internal"},{"type":"text","text":"pi pong"}]}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();
    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(updates.len(), 1);

    let completion = updates[0]
        .events
        .iter()
        .find_map(|event| match event {
            WatchEvent::TurnCompleted(completion) => Some(completion),
            _ => None,
        })
        .expect("pi terminal message should emit task_complete");
    assert_eq!(completion.last_agent_message.as_deref(), Some("pi pong"));
    assert_eq!(completion.duration_ms, Some(14_000));

    assert!(
        updates[0].events.iter().any(|event| {
            matches!(
                event,
                WatchEvent::AssistantMessage(message)
                    if message.text.as_deref() == Some("pi pong")
                        && message.phase.as_deref() == Some("final_answer")
            )
        }),
        "pi response should stay in transcript but not be projected as a duplicate notification"
    );
}

#[cfg(feature = "pi")]
#[tokio::test]
async fn streams_pi_terminal_line_larger_than_single_read_window() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pi.jsonl");
    fs::write(
        &path,
        concat!(
            r#"{"type":"session","version":3,"id":"pi-large-line","timestamp":"2026-05-03T00:00:00.000Z","cwd":"/tmp/project"}"#,
            "\n",
            r#"{"type":"message","id":"u1","timestamp":"2026-05-03T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"ping"}]}}"#,
            "\n",
        ),
    )
    .unwrap();
    let canonical = fs::canonicalize(&path).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("pi"),
        &path,
        crate::ParseSelection::full(),
        Duration::from_millis(10),
    );
    let text = "pi-done".repeat((MAX_WATCH_READ_BYTES as usize / 7) + 128);
    let line = serde_json::json!({
        "type": "message",
        "id": "a-large",
        "parentId": "u1",
        "timestamp": "2026-05-03T00:00:15.000Z",
        "message": {
            "role": "assistant",
            "model": "deepseek-v4-pro",
            "stopReason": "stop",
            "content": [{"type": "text", "text": text}],
        },
    })
    .to_string();
    assert!(line.len() > MAX_WATCH_READ_BYTES as usize);

    append_line(&path, &line);
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(updates.len(), 1);
    let completion = updates[0]
        .events
        .iter()
        .find_map(|event| match event {
            WatchEvent::TurnCompleted(completion) => Some(completion),
            _ => None,
        })
        .expect("large Pi terminal line should be streamed across read chunks");
    assert_eq!(
        completion.last_agent_message.as_deref(),
        Some(text.as_str())
    );
}

#[cfg(feature = "pi")]
#[tokio::test]
async fn pi_watch_task_complete_includes_elapsed_duration_when_prompt_preexists_created_baseline() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("pi.jsonl");
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("pi"),
        &root,
        crate::ParseSelection::full(),
        Duration::from_millis(10),
    );

    fs::write(
        &path,
        concat!(
            r#"{"type":"session","version":3,"id":"pi-created-duration","timestamp":"2026-05-03T00:00:00.000Z","cwd":"/tmp/project"}"#,
            "\n",
            r#"{"type":"message","id":"u1","timestamp":"2026-05-03T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"ping"}]}}"#,
            "\n",
        ),
    )
    .unwrap();
    let canonical = fs::canonicalize(&path).unwrap();
    tx.send(RawWatchEvent::Paths(vec![canonical.clone()]))
        .unwrap();
    let created_updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(created_updates.len(), 1);
    assert_eq!(created_updates[0].change, WatchChange::Created);
    assert!(
        created_updates[0].events.is_empty(),
        "newly discovered jsonl history should not replay preexisting transcript rows"
    );

    append_line(
        &path,
        r#"{"type":"message","id":"a1","parentId":"u1","timestamp":"2026-05-03T00:00:15.000Z","message":{"role":"assistant","model":"deepseek-v4-pro","stopReason":"stop","content":[{"type":"text","text":"pi pong"}]}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();
    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(updates.len(), 1);

    let completion = updates[0]
        .events
        .iter()
        .find_map(|event| match event {
            WatchEvent::TurnCompleted(completion) => Some(completion),
            _ => None,
        })
        .expect("preexisting pi prompt should still seed task_complete duration");
    assert_eq!(completion.last_agent_message.as_deref(), Some("pi pong"));
    assert_eq!(completion.duration_ms, Some(14_000));
}

#[cfg(feature = "claude")]
#[tokio::test]
async fn claude_watch_duration_when_prompt_preexists_created_baseline() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("projects");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("claude.jsonl");
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("claude"),
        &root,
        crate::ParseSelection::full(),
        Duration::from_millis(10),
    );

    fs::write(
        &path,
        concat!(
            r#"{"type":"user","sessionId":"claude-created-duration","cwd":"/tmp/project","timestamp":"2026-05-03T00:00:01.000Z","message":{"id":"u1","content":[{"type":"text","text":"ping"}]}}"#,
            "\n",
        ),
    )
    .unwrap();
    let canonical = fs::canonicalize(&path).unwrap();
    tx.send(RawWatchEvent::Paths(vec![canonical.clone()]))
        .unwrap();
    let created_updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(created_updates.len(), 1);
    assert_eq!(created_updates[0].change, WatchChange::Created);
    assert!(created_updates[0].events.is_empty());

    append_line(
        &path,
        r#"{"type":"assistant","sessionId":"claude-created-duration","cwd":"/tmp/project","timestamp":"2026-05-03T00:00:16.000Z","message":{"id":"a1","model":"claude","stop_reason":"end_turn","content":[{"type":"text","text":"claude pong"}]}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();
    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(updates.len(), 1);

    let completion = updates[0]
        .events
        .iter()
        .find_map(|event| match event {
            WatchEvent::TurnCompleted(completion) => Some(completion),
            _ => None,
        })
        .expect("provider watch adapter should synthesize claude task_complete");
    assert_eq!(
        completion.last_agent_message.as_deref(),
        Some("claude pong")
    );
    assert_eq!(completion.duration_ms, Some(15_000));
}

#[cfg(feature = "gemini")]
#[tokio::test]
async fn gemini_watch_does_not_synthesize_duration_from_rewritten_json() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("gemini.json");
    fs::write(
        &path,
        concat!(
            r#"{"sessionId":"gemini-duration","messages":["#,
            r#"{"type":"user","id":"u1","timestamp":"2026-05-03T00:00:01.000Z","content":"ping"}"#,
            r#"]}"#
        ),
    )
    .unwrap();
    let canonical = fs::canonicalize(&path).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("gemini"),
        &path,
        crate::ParseSelection::full(),
        Duration::from_millis(10),
    );

    fs::write(
        &path,
        concat!(
            r#"{"sessionId":"gemini-duration","messages":["#,
            r#"{"type":"user","id":"u1","timestamp":"2026-05-03T00:00:01.000Z","content":"ping"}"#,
            r#",{"type":"gemini","id":"a1","timestamp":"2026-05-03T00:00:12.000Z","content":"gemini pong","model":"gemini"}"#,
            r#"]}"#
        ),
    )
    .unwrap();
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();
    let updates = recv_timeout(&mut watcher, Duration::from_millis(50))
        .await
        .unwrap();
    assert!(
        updates.is_empty(),
        "watch must not full-reparse rewritten Gemini JSON"
    );
}

#[cfg(feature = "pi")]
#[tokio::test]
async fn ignores_pi_subagent_watch_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let project = root.join("--tmp-project--");
    let primary = project.join("2026-05-12T01-00-00-000Z_parent.jsonl");
    let subagent = project
        .join("2026-05-12T01-00-00-000Z_parent")
        .join("4ccc2ec1")
        .join("run-2")
        .join("session.jsonl");
    fs::create_dir_all(&project).unwrap();
    fs::write(&primary, "{}\n").unwrap();
    fs::create_dir_all(subagent.parent().unwrap()).unwrap();
    fs::write(
        &subagent,
        concat!(
            r#"{"type":"session","version":3,"id":"pi-subagent","timestamp":"2026-05-03T00:00:00.000Z","cwd":"/tmp/project"}"#,
            "\n",
        ),
    )
    .unwrap();

    let canonical = fs::canonicalize(&subagent).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("pi"),
        &root,
        crate::ParseSelection::empty().with_messages(),
        Duration::from_millis(10),
    );

    append_line(
        &subagent,
        r#"{"type":"message","id":"a1","timestamp":"2026-05-03T00:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"subagent output"}]}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_millis(50))
        .await
        .unwrap();
    assert!(updates.is_empty());
}

#[cfg(feature = "copilot")]
#[tokio::test]
async fn emits_appended_copilot_assistant_response() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    fs::write(
        &path,
        concat!(
            r#"{"type":"user.message","timestamp":"2026-05-03T00:00:01.000Z","data":{"messageId":"u1","content":"ping"}}"#,
            "\n",
        ),
    )
    .unwrap();
    let canonical = fs::canonicalize(&path).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("copilot"),
        &path,
        crate::ParseSelection::empty().with_messages(),
        Duration::from_millis(10),
    );

    append_line(
        &path,
        r#"{"type":"assistant.message","timestamp":"2026-05-03T00:00:02.000Z","data":{"messageId":"a1","content":"copilot pong"}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_single_assistant_response(&updates, "copilot pong");
}

#[cfg(feature = "cursor")]
#[tokio::test]
async fn emits_appended_cursor_assistant_response() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cursor.jsonl");
    fs::write(
        &path,
        concat!(
            r#"{"timestamp":"2026-05-03T00:00:01.000Z","role":"user","message":{"content":"ping"}}"#,
            "\n",
        ),
    )
    .unwrap();
    let canonical = fs::canonicalize(&path).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("cursor"),
        &path,
        crate::ParseSelection::empty().with_messages(),
        Duration::from_millis(10),
    );

    append_line(
        &path,
        r#"{"timestamp":"2026-05-03T00:00:02.000Z","role":"assistant","message":{"content":"cursor pong"}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_single_assistant_response(&updates, "cursor pong");
}

#[cfg(feature = "gemini")]
#[tokio::test]
async fn ignores_rewritten_gemini_assistant_response_without_incremental_target() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("gemini.json");
    fs::write(
        &path,
        r#"{"sessionId":"gemini-unit","messages":[{"type":"user","id":"u1","timestamp":"2026-05-03T00:00:01.000Z","content":"ping"}]}"#,
    )
    .unwrap();
    let canonical = fs::canonicalize(&path).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("gemini"),
        &path,
        crate::ParseSelection::empty().with_messages(),
        Duration::from_millis(10),
    );

    fs::write(
        &path,
        r#"{"sessionId":"gemini-unit","messages":[{"type":"user","id":"u1","timestamp":"2026-05-03T00:00:01.000Z","content":"ping"},{"type":"gemini","id":"a1","timestamp":"2026-05-03T00:00:02.000Z","model":"gemini","content":"gemini pong"}]}"#,
    )
    .unwrap();
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_millis(50))
        .await
        .unwrap();
    assert!(
        updates.is_empty(),
        "watch must not full-reparse rewritten Gemini JSON"
    );
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn message_selection_suppresses_appended_codex_operation() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let session_path = codex_session_path(&root);
    write_initial_codex_session(&session_path);
    let canonical = fs::canonicalize(&session_path).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("codex"),
        &root,
        crate::ParseSelection::empty().with_messages(),
        Duration::from_millis(10),
    );

    append_line(
        &session_path,
        r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","call_id":"call-1","arguments":{"cmd":"echo hi"}}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(updates.is_empty());
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn message_selection_keeps_codex_task_complete_without_operations() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let session_path = codex_session_path(&root);
    write_initial_codex_session(&session_path);
    let canonical = fs::canonicalize(&session_path).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("codex"),
        &root,
        crate::ParseSelection::empty().with_messages(),
        Duration::from_millis(10),
    );

    append_line(
        &session_path,
        r#"{"timestamp":"2026-05-03T00:00:20.000Z","type":"event_msg","payload":{"type":"task_complete","last_agent_message":"done","duration_ms":15000}}"#,
    );
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(updates.len(), 1);
    let completion = updates[0]
        .events
        .iter()
        .find_map(|event| match event {
            WatchEvent::TurnCompleted(completion) => Some(completion),
            _ => None,
        })
        .expect("message selection should keep Codex task_complete");
    assert_eq!(completion.last_agent_message.as_deref(), Some("done"));
    assert_eq!(completion.duration_ms, Some(15_000));
    assert!(
        updates[0]
            .events
            .iter()
            .all(|event| !matches!(event, WatchEvent::ToolCall(_) | WatchEvent::ToolResult(_))),
        "message selection must not emit operation payloads"
    );
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn streams_codex_task_complete_line_larger_than_single_read_window() {
    use futures::StreamExt;

    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let session_path = codex_session_path(&root);
    write_initial_codex_session(&session_path);
    let canonical = fs::canonicalize(&session_path).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("codex"),
        &root,
        crate::ParseSelection::empty().with_messages(),
        Duration::from_millis(10),
    );
    let text = "done".repeat((MAX_WATCH_READ_BYTES as usize / 4) + 128);
    let line = serde_json::json!({
        "timestamp": "2026-05-03T00:00:20.000Z",
        "type": "event_msg",
        "payload": {
            "type": "task_complete",
            "turn_id": "turn-large-line",
            "last_agent_message": text,
            "duration_ms": 15000,
        },
    })
    .to_string();
    assert!(line.len() > MAX_WATCH_READ_BYTES as usize);

    append_line(&session_path, &line);
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();

    let completion = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let updates = watcher
                .next()
                .await
                .expect("watch stream update")
                .expect("watch update");
            if let Some(completion) = updates.iter().find_map(|update| {
                update.events.iter().find_map(|event| match event {
                    WatchEvent::TurnCompleted(completion) => Some(completion.clone()),
                    _ => None,
                })
            }) {
                break completion;
            }
        }
    })
    .await
    .expect("large task_complete line should be streamed across read chunks");
    assert_eq!(completion.meta.turn_id.as_deref(), Some("turn-large-line"));
    assert_eq!(completion.last_agent_message.as_deref(), Some(text.trim()));
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn created_codex_jsonl_preserves_trailing_partial_for_next_delta() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    fs::create_dir_all(&root).unwrap();
    let (mut watcher, tx) = test_watcher_for(
        watch_provider("codex"),
        &root,
        crate::ParseSelection::empty().with_messages(),
        Duration::from_millis(10),
    );
    let session_path = codex_session_path(&root);
    fs::create_dir_all(session_path.parent().unwrap()).unwrap();
    let partial = serde_json::json!({
        "timestamp": "2026-05-03T00:00:20.000Z",
        "type": "event_msg",
        "payload": {
            "type": "task_complete",
            "turn_id": "turn-created-partial",
            "last_agent_message": "created partial done",
            "duration_ms": 15000,
        },
    })
    .to_string();
    fs::write(
        &session_path,
        format!(
            "{}\n{}\n{}",
            r#"{"timestamp":"2026-05-03T00:00:00.000Z","type":"session_meta","payload":{"session_id":"sess-watch","cwd":"/tmp/project","model":"gpt-5"}}"#,
            r#"{"timestamp":"2026-05-03T00:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"ping"}]}}"#,
            partial
        ),
    )
    .unwrap();
    let canonical = fs::canonicalize(&session_path).unwrap();
    tx.send(RawWatchEvent::Paths(vec![canonical.clone()]))
        .unwrap();

    let created = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].change, WatchChange::Created);
    assert!(created[0].events.is_empty());

    {
        let mut file = OpenOptions::new().append(true).open(&session_path).unwrap();
        writeln!(file).unwrap();
        file.sync_all().unwrap();
    }
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();
    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(updates.len(), 1);
    let completion = updates[0]
        .events
        .iter()
        .find_map(|event| match event {
            WatchEvent::TurnCompleted(completion) => Some(completion),
            _ => None,
        })
        .expect("created trailing partial should complete on the next delta");
    assert_eq!(
        completion.meta.turn_id.as_deref(),
        Some("turn-created-partial")
    );
    assert_eq!(
        completion.last_agent_message.as_deref(),
        Some("created partial done")
    );
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn initial_codex_jsonl_preserves_trailing_partial_for_next_delta() {
    let temp = tempfile::tempdir().unwrap();
    let root = codex_root(temp.path());
    let session_path = codex_session_path(&root);
    write_initial_codex_session(&session_path);
    let partial = serde_json::json!({
        "timestamp": "2026-05-03T00:00:20.000Z",
        "type": "event_msg",
        "payload": {
            "type": "task_complete",
            "turn_id": "turn-initial-partial",
            "last_agent_message": "initial partial done",
            "duration_ms": 15000,
        },
    })
    .to_string();
    {
        let mut file = OpenOptions::new().append(true).open(&session_path).unwrap();
        write!(file, "{partial}").unwrap();
        file.sync_all().unwrap();
    }
    let canonical = fs::canonicalize(&session_path).unwrap();

    let (mut watcher, tx) = test_watcher_for(
        watch_provider("codex"),
        &root,
        crate::ParseSelection::empty().with_messages(),
        Duration::from_millis(10),
    );
    assert!(
        !watcher
            .baselines
            .get(&canonical)
            .expect("initial codex baseline")
            .pending_partial
            .is_empty(),
        "startup baselines must remember an incomplete trailing JSONL record"
    );

    {
        let mut file = OpenOptions::new().append(true).open(&session_path).unwrap();
        writeln!(file).unwrap();
        file.sync_all().unwrap();
    }
    tx.send(RawWatchEvent::Paths(vec![canonical])).unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(updates.len(), 1);
    let completion = updates[0]
        .events
        .iter()
        .find_map(|event| match event {
            WatchEvent::TurnCompleted(completion) => Some(completion),
            _ => None,
        })
        .expect("initial trailing partial should complete on the next delta");
    assert_eq!(
        completion.meta.turn_id.as_deref(),
        Some("turn-initial-partial")
    );
    assert_eq!(
        completion.last_agent_message.as_deref(),
        Some("initial partial done")
    );
}
