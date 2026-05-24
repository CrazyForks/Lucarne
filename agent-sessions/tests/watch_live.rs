#![cfg(all(
    feature = "watch",
    feature = "agent_session",
    any(
        feature = "codex",
        feature = "claude",
        feature = "copilot",
        feature = "cursor",
        feature = "gemini"
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

async fn wait_for_assistant_response(watcher: &mut SessionWatcher, expected: &str) -> WatchUpdate {
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut seen_updates = Vec::new();
    while Instant::now() < deadline {
        let updates = recv_timeout(watcher, Duration::from_millis(250))
            .await
            .unwrap();
        for update in updates {
            if update.events.iter().any(|event| {
                matches!(
                    event,
                    WatchEvent::AssistantMessage(message)
                        if message.text.as_deref() == Some(expected)
                )
            }) {
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
