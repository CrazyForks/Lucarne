use std::process::Command;

fn manifest_dir() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn production_source(path: impl AsRef<std::path::Path>) -> String {
    let text = std::fs::read_to_string(path).expect("read source");
    let mut production = String::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line.trim() != "#[cfg(test)]" {
            production.push_str(line);
            production.push('\n');
            continue;
        }

        let Some(first_item_line) = lines.next() else {
            break;
        };
        let trimmed = first_item_line.trim();
        if trimmed.starts_with("mod tests") {
            break;
        }
        if trimmed.starts_with("use ") || trimmed.ends_with(',') {
            continue;
        }

        let mut brace_depth = brace_delta(first_item_line);
        let mut saw_block = first_item_line.contains('{');
        if saw_block && brace_depth == 0 {
            continue;
        }
        for skipped in lines.by_ref() {
            brace_depth += brace_delta(skipped);
            saw_block |= skipped.contains('{');
            if saw_block && brace_depth == 0 {
                break;
            }
        }
    }
    production
}

fn brace_delta(line: &str) -> isize {
    line.chars().fold(0, |depth, ch| match ch {
        '{' => depth + 1,
        '}' => depth - 1,
        _ => depth,
    })
}

#[test]
fn telegram_production_code_does_not_open_provider_sessions_or_state_db_directly() {
    let src_dir = manifest_dir().join("src");
    let files = Command::new("rg")
        .args([
            "--files",
            src_dir.to_str().expect("utf-8 path"),
            "-g",
            "*.rs",
            "-g",
            "!bin/*",
        ])
        .output()
        .expect("list telegram source files");
    let files = String::from_utf8(files.stdout).unwrap();
    let forbidden = [
        "AgentRuntime::new",
        "self.runtime",
        "ControlPlaneSqliteStore::open",
        "take_events(",
        ".open(",
        ".resume(",
        ".interrupt(",
        ".resolve(&req_id",
        "invoke_command(",
        "AgentEventStream",
    ];
    let mut production_hits = Vec::new();
    for path in files.lines() {
        let text = production_source(path);
        for (line_idx, line) in text.lines().enumerate() {
            if forbidden.iter().any(|pattern| line.contains(pattern)) {
                production_hits.push(format!("{path}:{}:{line}", line_idx + 1));
            }
        }
    }

    assert!(
        production_hits.is_empty(),
        "Telegram production code must call daemon API functions, not runtime/store directly:\n{}",
        production_hits.join("\n")
    );
}

#[test]
fn telegram_state_keeps_only_presentation_state() {
    let production = production_source(manifest_dir().join("src/state.rs"));
    for forbidden in [
        "ControlPlaneState",
        "control_plane: ",
        ".control_plane",
        "sync_control_plane",
        "persist_control_plane",
        "rebuild_sessions_from_control_plane",
        "ControlPlaneSqliteStore",
        "HistoryIndex",
        "DEFAULT_AGENT_PROVIDERS",
    ] {
        assert!(
            !production.contains(forbidden),
            "Telegram state must not own durable daemon state: {forbidden}"
        );
    }
}

#[test]
fn telegram_does_not_own_direct_notification_suppression_state() {
    let state = production_source(manifest_dir().join("src/state.rs"));
    for forbidden in [
        "direct_delivery_suppressed",
        "begin_direct_notification_suppression",
        "end_direct_notification_suppression",
    ] {
        assert!(
            !state.contains(forbidden),
            "Telegram state must not own direct notification suppression: {forbidden}"
        );
    }

    let bot = production_source(manifest_dir().join("src/bot.rs"));
    for forbidden in [
        "state.begin_direct_notification_suppression",
        "state.end_direct_notification_suppression",
        "state.direct_delivery_suppressed",
    ] {
        assert!(
            !bot.contains(forbidden),
            "Telegram bot must rely on core suppression state: {forbidden}"
        );
    }
}

#[test]
fn telegram_does_not_own_provider_discovery() {
    let production = production_source(manifest_dir().join("src/agents.rs"));
    for forbidden in [
        "DEFAULT_AGENT_PROVIDERS",
        "scan_path_agents",
        "find_in_path",
    ] {
        assert!(
            !production.contains(forbidden),
            "Telegram must render daemon provider catalog, not discover providers itself: {forbidden}"
        );
    }
}

#[test]
fn telegram_does_not_scan_history_files() {
    let src_dir = manifest_dir().join("src");
    let output = Command::new("rg")
        .args([
            "-n",
            "list_all_for_providers|list_page_for_providers|HistoryIndex",
            src_dir.to_str().expect("utf-8 path"),
            "-g",
            "*.rs",
        ])
        .output()
        .expect("scan telegram source");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.trim().is_empty(),
        "Telegram must query daemon history, not scan history files:\n{stdout}"
    );
}

#[test]
fn telegram_channel_subscription_receiver_uses_sync_lock() {
    let production = production_source(manifest_dir().join("src/channel.rs"));
    assert!(
        production.contains("events_rx: Mutex<Option<mpsc::Receiver<ChannelEvent>>>"),
        "Telegram channel subscribe() is synchronous, so its one-shot receiver lock should be sync"
    );
    assert!(
        !production.contains("events_rx: tokio::sync::Mutex<Option<mpsc::Receiver<ChannelEvent>>>"),
        "Telegram channel should not use async mutex for the one-shot subscription receiver"
    );
}

#[test]
fn telegram_presentation_state_emits_structured_tracing() {
    let production = production_source(manifest_dir().join("src/state.rs"));
    for needle in [
        "lucarne_telegram::state",
        "telegram state initialized",
        "telegram session upserted",
        "telegram live session bound",
        "telegram live session marked dead",
    ] {
        assert!(
            production.contains(needle),
            "Telegram presentation state tracing must cover adapter state boundary: {needle}"
        );
    }
}

#[test]
fn telegram_adapter_modules_emit_structured_tracing() {
    for (file, needles) in [(
        "src/adapter.rs",
        &[
            "lucarne_telegram::adapter",
            "telegram adapter spawning",
            "telegram adapter started",
        ][..],
    )] {
        let production = production_source(manifest_dir().join(file));
        for needle in needles {
            assert!(
                production.contains(needle),
                "{file} tracing must cover Telegram adapter boundary: {needle}"
            );
        }
    }
}

#[test]
fn telegram_state_does_not_sync_core_while_inner_state_locked() {
    let production = production_source(manifest_dir().join("src/state.rs"));
    let mut inner_locked = false;
    let mut hits = Vec::new();
    for (idx, line) in production.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("pub ") || trimmed.starts_with("fn ") {
            inner_locked = false;
        }
        if line.contains("let mut g = self.inner.lock().unwrap();")
            || line.contains("let mut g = self.inner.write().unwrap();")
        {
            inner_locked = true;
        }
        for needle in [
            "self.sync_core_session",
            "remove_workspace_projection",
            "clear_workspace_activation",
        ] {
            if inner_locked && line.contains(needle) {
                hits.push(format!("{}:{line}", idx + 1));
            }
        }
        if inner_locked && line.contains("drop(g);") {
            inner_locked = false;
        }
    }

    assert!(
        hits.is_empty(),
        "Telegram UI state lock must be released before core/session persistence sync:\n{}",
        hits.join("\n")
    );
}

#[test]
fn telegram_state_presentation_cache_is_read_optimized() {
    let production = production_source(manifest_dir().join("src/state.rs"));

    assert!(
        production.contains("inner: RwLock<Inner>"),
        "Telegram presentation cache should use RwLock so read-only UI paths do not serialize"
    );
    assert!(
        !production.contains("inner: Mutex<Inner>"),
        "Telegram presentation cache should not use a single Mutex for read-heavy state"
    );
}

#[test]
fn telegram_provider_status_update_borrows_status_snapshot() {
    let production = production_source(manifest_dir().join("src/state.rs"));
    assert!(
        !production
            .contains(".record_provider_status(&control_workspace_id(&workspace), status.clone())"),
        "Telegram should not clone the full provider status snapshot before calling core"
    );
    assert!(
        !production.contains("let workspace = g.workspace_by_topic.get(ws).unwrap_or(ws).clone();"),
        "Telegram should not clone the channel workspace id just to record provider status"
    );

    let crates_dir = manifest_dir().parent().expect("crates dir");
    let core_service = production_source(crates_dir.join("lucarne/src/core_service/service.rs"));
    assert!(
        core_service.contains("status: &AgentStatus"),
        "LucarneCore::record_provider_status should borrow AgentStatus"
    );

    let control_state = production_source(crates_dir.join("lucarne/src/control_plane/state.rs"));
    assert!(
        control_state.contains("status: &AgentStatus"),
        "ControlPlaneState::update_provider_status should borrow AgentStatus"
    );
}

#[test]
fn telegram_core_only_paths_do_not_clone_channel_workspace_ids() {
    let production = production_source(manifest_dir().join("src/state.rs"));
    assert!(
        production.contains(
            "pub fn workspace_for_topic(&self, topic: &WorkspaceId) -> Option<WorkspaceId>"
        ),
        "Telegram state should expose explicit topic->workspace resolution"
    );

    assert!(
        !production.contains("workspace_by_topic.get(ws).unwrap_or(ws)")
            && !production.contains(".unwrap_or_else(|| ws.clone())"),
        "Telegram state must not fall back from topic ids to same-text workspace ids"
    );
}
