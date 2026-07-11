#![cfg(all(
    feature = "watch",
    feature = "agent_session",
    any(
        feature = "codex",
        feature = "claude",
        feature = "copilot",
        feature = "cursor",
        feature = "gemini",
        feature = "grok",
        feature = "pi"
    )
))]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use agent_sessions::{
    ParseSelection, SessionWatcher, WatchConfig, WatchError, WatchEvent, WatchProvider, WatchUpdate,
};
use futures::StreamExt;

fn watch_provider(id: &str) -> WatchProvider {
    agent_sessions::agent_provider(id).expect("watch provider")
}

fn write_parent(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn append_line(path: &Path, line: &str) {
    let mut file = OpenOptions::new().append(true).open(path).unwrap();
    writeln!(file, "{line}").unwrap();
    file.sync_all().unwrap();
}

#[cfg(feature = "codex")]
fn rename_into_place(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let tmp_path = path.with_extension("tmp");
    {
        let mut file = fs::File::create(&tmp_path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        file.sync_all().unwrap();
    }
    fs::rename(tmp_path, path).unwrap();
}

async fn recv_timeout(
    watcher: &mut SessionWatcher,
    timeout: Duration,
) -> std::result::Result<Box<[WatchUpdate]>, WatchError> {
    match tokio::time::timeout(timeout, watcher.next()).await {
        Ok(Some(result)) => result,
        Ok(None) => Err(WatchError::Disconnected),
        Err(_) => Ok(Vec::new().into_boxed_slice()),
    }
}

async fn start_file_watcher(provider: WatchProvider, path: &Path) -> SessionWatcher {
    let mut watcher = SessionWatcher::start(
        WatchConfig::new()
            .providers([provider])
            .provider_roots(provider, [path.to_path_buf()])
            .selection(ParseSelection::empty().with_messages())
            .debounce(Duration::from_millis(25)),
    )
    .unwrap();
    assert!(
        recv_timeout(&mut watcher, Duration::from_millis(40))
            .await
            .unwrap()
            .is_empty()
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    watcher
}

fn event_carries_channel_body(event: &WatchEvent, expected: &str) -> bool {
    match event {
        // Peer providers may emit intermediate/final phases; match body text.
        WatchEvent::AssistantMessage(message) if message.text.as_deref() == Some(expected) => true,
        // Grok surfaces the final body on turn complete after mid-turn filtering.
        WatchEvent::TurnCompleted(completed)
            if completed.last_agent_message.as_deref() == Some(expected) =>
        {
            true
        }
        _ => false,
    }
}

async fn wait_for_assistant_response(watcher: &mut SessionWatcher, expected: &str) -> WatchUpdate {
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut seen_updates = Vec::new();
    while Instant::now() < deadline {
        let updates = recv_timeout(watcher, Duration::from_millis(250))
            .await
            .unwrap();
        for update in updates {
            if update
                .events
                .iter()
                .any(|event| event_carries_channel_body(event, expected))
            {
                return update;
            }
            seen_updates.push(update);
        }
    }
    panic!("missing assistant response {expected}; updates={seen_updates:?}");
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn live_codex_watch_emits_appended_assistant_response() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rollout-live.jsonl");
    write_parent(
        &path,
        concat!(
            r#"{"timestamp":"2026-05-03T00:00:00.000Z","type":"session_meta","payload":{"session_id":"sess-live","cwd":"/tmp/project","model":"gpt-5"}}"#,
            "\n",
            r#"{"timestamp":"2026-05-03T00:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"ping"}]}}"#,
            "\n",
        ),
    );
    let mut watcher = start_file_watcher(watch_provider("codex"), &path).await;

    append_line(
        &path,
        r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"codex pong"}]}}"#,
    );

    let update = wait_for_assistant_response(&mut watcher, "codex pong").await;
    assert_eq!(update.provider, watch_provider("codex"));
}

#[cfg(feature = "codex")]
#[tokio::test]
async fn live_codex_watch_emits_renamed_assistant_response() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("codex-live-root");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("rollout-renamed-live.jsonl");
    let mut watcher = start_file_watcher(watch_provider("codex"), &root).await;

    rename_into_place(
        &path,
        concat!(
            r#"{"timestamp":"2026-05-03T00:00:00.000Z","type":"session_meta","payload":{"session_id":"sess-live-renamed","cwd":"/tmp/project","model":"gpt-5"}}"#,
            "\n",
            r#"{"timestamp":"2026-05-03T00:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"ping"}]}}"#,
            "\n",
            r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"codex renamed pong"}]}}"#,
            "\n",
        ),
    );

    let update = wait_for_assistant_response(&mut watcher, "codex renamed pong").await;
    assert_eq!(update.provider, watch_provider("codex"));
}

#[cfg(all(
    feature = "codex",
    any(target_os = "macos", windows, target_os = "linux")
))]
#[tokio::test]
async fn live_codex_watch_emits_nested_created_session_response() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("codex-home").join("sessions");
    fs::create_dir_all(&root).unwrap();
    let path = root
        .join("2026")
        .join("05")
        .join("03")
        .join("rollout-nested-live.jsonl");
    let mut watcher = start_file_watcher(watch_provider("codex"), &root).await;

    rename_into_place(
        &path,
        concat!(
            r#"{"timestamp":"2026-05-03T00:00:00.000Z","type":"session_meta","payload":{"session_id":"sess-live-nested","cwd":"/tmp/project","model":"gpt-5"}}"#,
            "\n",
            r#"{"timestamp":"2026-05-03T00:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"ping"}]}}"#,
            "\n",
            r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"codex nested pong"}]}}"#,
            "\n",
        ),
    );

    let update = wait_for_assistant_response(&mut watcher, "codex nested pong").await;
    assert_eq!(update.provider, watch_provider("codex"));
}

#[cfg(feature = "claude")]
#[tokio::test]
async fn live_claude_watch_emits_appended_assistant_response() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("claude-live.jsonl");
    write_parent(
        &path,
        concat!(
            r#"{"type":"user","timestamp":"2026-05-03T00:00:01.000Z","message":{"id":"u1","content":[{"type":"text","text":"ping"}]}}"#,
            "\n",
        ),
    );
    let mut watcher = start_file_watcher(watch_provider("claude"), &path).await;

    append_line(
        &path,
        r#"{"type":"assistant","timestamp":"2026-05-03T00:00:02.000Z","message":{"id":"a1","model":"claude","content":[{"type":"text","text":"claude pong"}]}}"#,
    );

    let update = wait_for_assistant_response(&mut watcher, "claude pong").await;
    assert_eq!(update.provider, watch_provider("claude"));
}

#[cfg(feature = "copilot")]
#[tokio::test]
async fn live_copilot_watch_emits_appended_assistant_response() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    write_parent(
        &path,
        concat!(
            r#"{"type":"user.message","timestamp":"2026-05-03T00:00:01.000Z","data":{"messageId":"u1","content":"ping"}}"#,
            "\n",
        ),
    );
    let mut watcher = start_file_watcher(watch_provider("copilot"), &path).await;

    append_line(
        &path,
        r#"{"type":"assistant.message","timestamp":"2026-05-03T00:00:02.000Z","data":{"messageId":"a1","content":"copilot pong"}}"#,
    );

    let update = wait_for_assistant_response(&mut watcher, "copilot pong").await;
    assert_eq!(update.provider, watch_provider("copilot"));
}

#[cfg(feature = "cursor")]
#[tokio::test]
async fn live_cursor_watch_emits_appended_assistant_response() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cursor-live.jsonl");
    write_parent(
        &path,
        concat!(
            r#"{"timestamp":"2026-05-03T00:00:01.000Z","role":"user","message":{"content":"ping"}}"#,
            "\n",
        ),
    );
    let mut watcher = start_file_watcher(watch_provider("cursor"), &path).await;

    append_line(
        &path,
        r#"{"timestamp":"2026-05-03T00:00:02.000Z","role":"assistant","message":{"content":"cursor pong"}}"#,
    );

    let update = wait_for_assistant_response(&mut watcher, "cursor pong").await;
    assert_eq!(update.provider, watch_provider("cursor"));
}

#[cfg(feature = "gemini")]
#[tokio::test]
async fn live_gemini_watch_ignores_rewritten_assistant_response_without_incremental_target() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("gemini-live.json");
    write_parent(
        &path,
        r#"{"sessionId":"gemini-live","messages":[{"type":"user","id":"u1","timestamp":"2026-05-03T00:00:01.000Z","content":"ping"}]}"#,
    );
    let mut watcher = start_file_watcher(watch_provider("gemini"), &path).await;

    fs::write(
        &path,
        r#"{"sessionId":"gemini-live","messages":[{"type":"user","id":"u1","timestamp":"2026-05-03T00:00:01.000Z","content":"ping"},{"type":"gemini","id":"a1","timestamp":"2026-05-03T00:00:02.000Z","model":"gemini","content":"gemini pong"}]}"#,
    )
    .unwrap();

    let updates = recv_timeout(&mut watcher, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(
        updates.is_empty(),
        "Gemini rewritten JSON must not be full-reparsed by live watch; updates={updates:?}"
    );
}

#[cfg(feature = "grok")]
fn percent_encode_cwd(cwd: &str) -> String {
    let mut out = String::with_capacity(cwd.len() * 3);
    for b in cwd.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(feature = "grok")]
fn write_grok_session_layout(
    sessions_root: &Path,
    session_id: &str,
    cwd: &str,
    baseline_lines: &str,
) -> std::path::PathBuf {
    let session_dir = sessions_root
        .join(percent_encode_cwd(cwd))
        .join(session_id);
    fs::create_dir_all(&session_dir).unwrap();
    let updates = session_dir.join("updates.jsonl");
    let summary = session_dir.join("summary.json");
    fs::write(&updates, baseline_lines).unwrap();
    fs::write(
        &summary,
        format!(
            r#"{{"info":{{"id":"{session_id}","cwd":"{cwd}"}},"generated_title":"Grok live watch","current_model_id":"grok-4.5"}}"#
        ),
    )
    .unwrap();
    updates
}

#[cfg(feature = "grok")]
fn grok_user_line(session_id: &str, ts: &str, text: &str) -> String {
    serde_json::json!({
        "timestamp": ts,
        "method": "session/update",
        "params": {
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "user_message_chunk",
                "content": { "type": "text", "text": text }
            }
        }
    })
    .to_string()
}

#[cfg(feature = "grok")]
fn grok_assistant_line(session_id: &str, ts: &str, text: &str) -> String {
    serde_json::json!({
        "timestamp": ts,
        "method": "session/update",
        "params": {
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": text }
            }
        }
    })
    .to_string()
}

#[cfg(feature = "grok")]
fn grok_turn_completed_line(session_id: &str, ts: &str) -> String {
    serde_json::json!({
        "timestamp": ts,
        "method": "session/update",
        "params": {
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "turn_completed",
                "stop_reason": "end_turn"
            }
        }
    })
    .to_string()
}

/// Real Grok turns end with turn_completed; channel notify uses last_agent_message.
#[cfg(feature = "grok")]
fn grok_assistant_turn(session_id: &str, ts: &str, text: &str) -> String {
    format!(
        "{}\n{}",
        grok_assistant_line(session_id, ts, text),
        grok_turn_completed_line(session_id, ts)
    )
}

/// Direct file append: incremental tail only (Codex/Claude parity).
#[cfg(feature = "grok")]
#[tokio::test]
async fn live_grok_watch_emits_appended_assistant_response() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("updates.jsonl");
    write_parent(
        &path,
        &format!(
            "{}\n",
            grok_user_line(
                "019f4f1c-live-append-000000000001",
                "2026-05-03T00:00:01.000Z",
                "ping"
            )
        ),
    );
    let mut watcher = start_file_watcher(watch_provider("grok"), &path).await;

    for line in grok_assistant_turn(
        "019f4f1c-live-append-000000000001",
        "2026-05-03T00:02:06.000Z",
        "grok pong",
    )
    .lines()
    {
        append_line(&path, line);
    }

    let update = wait_for_assistant_response(&mut watcher, "grok pong").await;
    assert_eq!(update.provider, watch_provider("grok"));
}

/// Nested real layout under sessions/<encoded-cwd>/<uuid>/updates.jsonl.
#[cfg(all(
    feature = "grok",
    any(target_os = "macos", windows, target_os = "linux")
))]
#[tokio::test]
async fn live_grok_watch_emits_nested_session_append() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("grok-home").join("sessions");
    fs::create_dir_all(&root).unwrap();
    // Empty root first so start_file_watcher baseline drain stays quiet.
    let mut watcher = start_file_watcher(watch_provider("grok"), &root).await;

    let cwd = "/tmp/grok-live-project";
    let sid = "019f4f1c-live-nested-000000000002";
    let path = write_grok_session_layout(
        &root,
        sid,
        cwd,
        &format!("{}\n", grok_user_line(sid, "2026-05-03T00:00:01.000Z", "ping")),
    );
    // Drain Created/discovery noise before the assistant append under test.
    let _ = recv_timeout(&mut watcher, Duration::from_millis(400)).await;

    for line in grok_assistant_turn(sid, "2026-05-03T00:02:06.000Z", "grok nested pong").lines()
    {
        append_line(&path, line);
    }

    let update = wait_for_assistant_response(&mut watcher, "grok nested pong").await;
    assert_eq!(update.provider, watch_provider("grok"));
    assert_eq!(update.session_id.as_deref(), Some(sid));
}

/// New nested session created after watch starts (Created baseline + Updated append).
#[cfg(all(
    feature = "grok",
    any(target_os = "macos", windows, target_os = "linux")
))]
#[tokio::test]
async fn live_grok_watch_emits_nested_created_session_response() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("grok-home").join("sessions");
    fs::create_dir_all(&root).unwrap();
    let mut watcher = start_file_watcher(watch_provider("grok"), &root).await;

    let cwd = "/tmp/grok-created-project";
    let sid = "019f4f1c-live-created-000000000003";
    let _path = write_grok_session_layout(
        &root,
        sid,
        cwd,
        &format!(
            "{}\n{}\n",
            grok_user_line(sid, "2026-05-03T00:00:01.000Z", "ping"),
            grok_assistant_turn(sid, "2026-05-03T00:02:06.000Z", "grok created pong"),
        ),
    );

    let update = wait_for_assistant_response(&mut watcher, "grok created pong").await;
    assert_eq!(update.provider, watch_provider("grok"));
    assert_eq!(update.session_id.as_deref(), Some(sid));
}

/// Large baseline + small append: only the new assistant text is delivered (bounded delta).
#[cfg(feature = "grok")]
#[tokio::test]
async fn live_grok_watch_large_baseline_append_only_emits_delta() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("updates.jsonl");
    let sid = "019f4f1c-live-large-000000000004";
    let mut baseline = String::new();
    baseline.push_str(&grok_user_line(sid, "2026-05-03T00:00:01.000Z", "ping"));
    baseline.push('\n');
    // ~64KiB of noise chunks that must not be re-emitted on append.
    for i in 0..800 {
        baseline.push_str(&grok_assistant_line(
            sid,
            "2026-05-03T00:00:02.000Z",
            &format!("noise-{i:04}-pad-xxxxxxxxxxxxxxxx"),
        ));
        baseline.push('\n');
    }
    write_parent(&path, &baseline);
    let mut watcher = start_file_watcher(watch_provider("grok"), &path).await;

    for line in
        grok_assistant_turn(sid, "2026-05-03T00:02:06.000Z", "grok large-delta pong").lines()
    {
        append_line(&path, line);
    }

    let update = wait_for_assistant_response(&mut watcher, "grok large-delta pong").await;
    assert_eq!(update.provider, watch_provider("grok"));
    // Delta window must not replay earlier noise blobs as separate expected hits;
    // the wait already required exact text match. Also assert no full baseline reparse
    // by ensuring we did not get hundreds of channel-facing body events in this update.
    let body_count = update
        .events
        .iter()
        .filter(|e| {
            matches!(
                e,
                WatchEvent::AssistantMessage(_) | WatchEvent::TurnCompleted(_)
            )
        })
        .count();
    assert!(
        body_count <= 4,
        "expected bounded delta events, got {body_count} body events in {:?}",
        update.events
    );
}

#[cfg(feature = "pi")]
#[tokio::test]
async fn live_pi_watch_emits_appended_assistant_response() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pi-live.jsonl");
    write_parent(
        &path,
        concat!(
            r#"{"type":"session","id":"pi-live","cwd":"/tmp/project","timestamp":"2026-05-03T00:00:00.000Z"}"#,
            "\n",
            r#"{"type":"message","id":"u1","parentId":null,"timestamp":"2026-05-03T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"ping"}]}}"#,
            "\n",
        ),
    );
    let mut watcher = start_file_watcher(watch_provider("pi"), &path).await;

    append_line(
        &path,
        r#"{"type":"message","id":"a1","parentId":"u1","timestamp":"2026-05-03T00:02:06.000Z","message":{"role":"assistant","model":"pi","stopReason":"stop","content":[{"type":"text","text":"pi pong"}]}}"#,
    );

    let update = wait_for_assistant_response(&mut watcher, "pi pong").await;
    assert_eq!(update.provider, watch_provider("pi"));
}
