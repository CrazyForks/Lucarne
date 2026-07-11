use smol_str::SmolStr;
use std::io::BufRead;
use std::path::Path;

#[cfg(feature = "copilot")]
use crate::InputMetadata;
use crate::{ParseSelection, Result, agent_session::SessionMeta};

#[cfg(any(
    feature = "claude",
    feature = "copilot",
    feature = "cursor",
    feature = "gemini",
    feature = "grok",
    feature = "pi"
))]
use super::{WatchAssistantMessage, WatchEventMeta, WatchTurnCompleted};
use super::{WatchEvent, WatchProvider, state::ProviderWatchState};

#[derive(Debug)]
pub(crate) struct ParsedWatchSession {
    pub(crate) session_id: Option<SmolStr>,
    pub(crate) cwd: Option<SmolStr>,
    pub(crate) title: Option<SmolStr>,
    pub(crate) events: Box<[WatchEvent]>,
}

pub(crate) trait ProviderWatchEvents: Sized {
    /// Parse an appended watch delta into semantic watch events.
    ///
    /// Implementations must apply `selection` before materializing provider
    /// fields outside the selected projection. Watch callers pass small byte
    /// windows, but those windows can still contain large tool payloads.
    fn parse_watch_reader<R>(
        path: &Path,
        reader: R,
        selection: ParseSelection,
    ) -> Result<ParsedWatchSession>
    where
        R: BufRead;

    fn probe_watch_session_meta<R>(path: &Path, reader: R) -> Result<SessionMeta>
    where
        R: BufRead;

    fn parse_watch_metadata_reader<R>(path: &Path, reader: R) -> Result<ParsedWatchSession>
    where
        R: BufRead,
    {
        let meta = Self::probe_watch_session_meta(path, reader)?;
        Ok(parsed_watch_metadata(meta.session_id, meta.cwd, meta.title))
    }

    fn supports_incremental_watch_events() -> bool {
        true
    }

    fn needs_watch_state_seed() -> bool {
        false
    }

    fn seed_watch_state(events: &[WatchEvent]) -> ProviderWatchState {
        ProviderWatchState::with_last_prompt_timestamp(latest_prompt_timestamp(events))
    }

    fn normalize_watch_events(
        events: Box<[WatchEvent]>,
        state: &mut ProviderWatchState,
    ) -> Box<[WatchEvent]> {
        state.last_prompt_timestamp =
            latest_prompt_timestamp(&events).or_else(|| state.last_prompt_timestamp.clone());
        events
    }

    fn dedupe_watch_events(
        events: Box<[WatchEvent]>,
        _state: &mut ProviderWatchState,
    ) -> Box<[WatchEvent]> {
        events
    }

    fn initial_watch_directory_depth() -> Option<usize> {
        None
    }

    fn changed_watch_directory_depth() -> Option<usize> {
        Some(0)
    }

    fn includes_watch_directory(_root: &Path, _path: &Path, _is_recent: bool) -> bool {
        true
    }
}

pub(super) fn provider_supports_incremental_watch_events(provider: WatchProvider) -> bool {
    provider.supports_incremental_watch_events()
}

pub(super) fn provider_needs_watch_state_seed(provider: WatchProvider) -> bool {
    provider.needs_watch_state_seed()
}

pub(super) fn seed_provider_watch_state(
    provider: WatchProvider,
    events: &[WatchEvent],
) -> ProviderWatchState {
    provider.seed_watch_state(events)
}

pub(super) fn dedupe_provider_watch_events(
    provider: WatchProvider,
    events: Box<[WatchEvent]>,
    state: &mut ProviderWatchState,
) -> Box<[WatchEvent]> {
    provider.dedupe_watch_events(events, state)
}

pub(super) fn normalize_provider_watch_events(
    provider: WatchProvider,
    events: Box<[WatchEvent]>,
    state: &mut ProviderWatchState,
) -> Box<[WatchEvent]> {
    provider.normalize_watch_events(events, state)
}

#[cfg(any(
    feature = "claude",
    feature = "copilot",
    feature = "cursor",
    feature = "gemini",
    feature = "grok",
    feature = "pi"
))]
pub(crate) fn synthesize_task_complete_from_terminal_responses(
    events: Box<[WatchEvent]>,
    state: &mut ProviderWatchState,
    is_terminal_response: impl Fn(&WatchAssistantMessage) -> bool,
) -> Box<[WatchEvent]> {
    let mut out = Vec::with_capacity(events.len());
    for mut event in events.into_vec() {
        let timestamp = event.cloned_timestamp();
        let mut completion = None;
        if event.user_text().is_some()
            && let Some(timestamp) = timestamp.clone()
        {
            state.last_prompt_timestamp = Some(timestamp);
        } else if let Some(response) = event.assistant_message_mut()
            && is_terminal_response(response)
            && let (Some(text), Some(start), Some(end)) = (
                response.text.clone(),
                state.last_prompt_timestamp.as_deref(),
                timestamp.as_deref(),
            )
            && let Some(duration_ms) = timestamp_duration_ms(start, end)
        {
            response.phase = Some("final_answer".into());
            let value = serde_json::json!({
                "last_agent_message": text,
                "duration_ms": duration_ms,
            });
            completion = Some(WatchEvent::TurnCompleted(WatchTurnCompleted {
                meta: WatchEventMeta {
                    timestamp: timestamp.clone(),
                    ..WatchEventMeta::default()
                },
                last_agent_message: Some(text),
                duration_ms: Some(duration_ms),
                value_json: Some(value.to_string().into()),
            }));
        }
        out.push(event);
        if let Some(completion) = completion {
            out.push(completion);
        }
    }
    out.into_boxed_slice()
}

/// Metadata bytes wrapper for parser tests and existing byte-backed callers.
#[cfg(test)]
pub(super) fn parse_provider_metadata_bytes(
    provider: WatchProvider,
    path: &Path,
    bytes: Vec<u8>,
) -> Result<ParsedWatchSession> {
    let mut reader = std::io::Cursor::new(bytes);
    provider.parse_watch_metadata_reader(path, &mut reader)
}

pub(super) fn parse_provider_metadata_reader(
    provider: WatchProvider,
    path: &Path,
    reader: &mut dyn BufRead,
) -> Result<ParsedWatchSession> {
    provider.parse_watch_metadata_reader(path, reader)
}

pub(crate) fn parsed_watch_metadata(
    session_id: Option<SmolStr>,
    cwd: Option<SmolStr>,
    title: Option<SmolStr>,
) -> ParsedWatchSession {
    ParsedWatchSession {
        session_id,
        cwd,
        title,
        events: Vec::new().into_boxed_slice(),
    }
}

#[cfg(feature = "copilot")]
pub(crate) fn input_metadata_for_path(path: &Path) -> InputMetadata<'_> {
    let mut metadata = InputMetadata::default();
    if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
        metadata = metadata.name(name);
    }
    metadata.media_type(media_type_for_path(path))
}

#[cfg(feature = "copilot")]
fn media_type_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("json") => "application/json",
        Some("yaml") | Some("yml") => "application/yaml",
        _ => "application/jsonl",
    }
}

pub(super) fn is_session_like_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext, "jsonl" | "json"))
}

pub(super) fn includes_provider_candidate_in_history(
    provider: WatchProvider,
    root: &Path,
    path: &Path,
) -> bool {
    provider.includes_candidate_in_history(root, path)
}

pub(super) fn discover_provider_session_files_into(
    provider: WatchProvider,
    root: &Path,
    is_recent: &mut dyn FnMut(&Path) -> bool,
    emit: &mut dyn FnMut(std::path::PathBuf),
) {
    provider.discover_session_files_into(root, is_recent, emit);
}

pub(crate) fn latest_prompt_timestamp(events: &[WatchEvent]) -> Option<smol_str::SmolStr> {
    events.iter().rev().find_map(|event| {
        if event.user_text().is_some() {
            event.cloned_timestamp()
        } else {
            None
        }
    })
}

#[cfg(any(
    feature = "claude",
    feature = "copilot",
    feature = "cursor",
    feature = "gemini",
    feature = "grok",
    feature = "pi"
))]
fn timestamp_duration_ms(start: &str, end: &str) -> Option<u64> {
    let start = parse_utc_rfc3339_millis(start)?;
    let end = parse_utc_rfc3339_millis(end)?;
    let duration = end.checked_sub(start)?;
    u64::try_from(duration).ok()
}

#[cfg(any(
    feature = "claude",
    feature = "copilot",
    feature = "cursor",
    feature = "gemini",
    feature = "grok",
    feature = "pi"
))]
fn parse_utc_rfc3339_millis(timestamp: &str) -> Option<i64> {
    let bytes = timestamp.as_bytes();
    if bytes.len() < 20
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return None;
    }

    let year = parse_digits(bytes, 0, 4)? as i32;
    let month = parse_digits(bytes, 5, 2)?;
    let day = parse_digits(bytes, 8, 2)?;
    let hour = parse_digits(bytes, 11, 2)?;
    let minute = parse_digits(bytes, 14, 2)?;
    let second = parse_digits(bytes, 17, 2)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    let mut idx = 19;
    let mut millis = 0_u32;
    if bytes.get(idx) == Some(&b'.') {
        idx += 1;
        let mut factor = 100_u32;
        let mut digits = 0_u32;
        while let Some(byte) = bytes.get(idx).copied().filter(u8::is_ascii_digit) {
            if factor > 0 {
                millis += u32::from(byte - b'0') * factor;
                factor /= 10;
            }
            digits += 1;
            idx += 1;
        }
        if digits == 0 {
            return None;
        }
    }
    if bytes.get(idx) != Some(&b'Z') || idx + 1 != bytes.len() {
        return None;
    }

    let days = days_from_civil(year, month, day);
    Some(
        days * 86_400_000
            + i64::from(hour) * 3_600_000
            + i64::from(minute) * 60_000
            + i64::from(second) * 1_000
            + i64::from(millis),
    )
}

#[cfg(any(
    feature = "claude",
    feature = "copilot",
    feature = "cursor",
    feature = "gemini",
    feature = "grok",
    feature = "pi"
))]
fn parse_digits(bytes: &[u8], start: usize, len: usize) -> Option<u32> {
    let mut value = 0_u32;
    for byte in bytes.get(start..start + len)? {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u32::from(*byte - b'0');
    }
    Some(value)
}

#[cfg(any(
    feature = "claude",
    feature = "copilot",
    feature = "cursor",
    feature = "gemini",
    feature = "grok",
    feature = "pi"
))]
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era) * 146_097 + i64::from(doe) - 719_468
}

#[cfg(test)]
mod tests {
    #[cfg(any(
        feature = "codex",
        feature = "claude",
        feature = "copilot",
        feature = "cursor",
        feature = "grok",
    feature = "pi"
    ))]
    use std::path::Path;

    #[cfg(any(
        feature = "codex",
        feature = "claude",
        feature = "copilot",
        feature = "cursor",
        feature = "grok",
    feature = "pi"
    ))]
    use crate::ParseSelection;
    #[cfg(any(
        feature = "codex",
        feature = "claude",
        feature = "copilot",
        feature = "cursor",
        feature = "grok",
    feature = "pi"
    ))]
    use crate::watch::WatchProvider;

    #[cfg(any(
        feature = "codex",
        feature = "claude",
        feature = "copilot",
        feature = "cursor",
        feature = "grok",
    feature = "pi"
    ))]
    fn watch_provider(id: &str) -> WatchProvider {
        crate::agent_provider(id).expect("watch provider")
    }

    fn parse_provider_reader(
        provider: WatchProvider,
        path: &Path,
        bytes: Vec<u8>,
        selection: ParseSelection,
    ) -> crate::Result<super::ParsedWatchSession> {
        let mut reader = std::io::Cursor::new(bytes);
        provider.parse_watch_reader(path, &mut reader, selection)
    }

    #[cfg(feature = "cursor")]
    #[test]
    fn parse_provider_reader_preserves_cursor_session_id_from_path() {
        let bytes = r#"{"timestamp":"2026-05-03T00:00:02.000Z","role":"assistant","message":{"content":"cursor pong"}}"#;

        let parsed = parse_provider_reader(
            watch_provider("cursor"),
            Path::new("/tmp/cursor-reader-session.jsonl"),
            bytes.as_bytes().to_vec(),
            ParseSelection::full(),
        )
        .unwrap();

        assert_eq!(parsed.session_id.as_deref(), Some("cursor-reader-session"));
        assert_eq!(parsed.events.len(), 1);
    }

    #[cfg(feature = "grok")]
    #[test]
    fn parse_provider_reader_grok_delta_only_window_emits_assistant() {
        // Watch hot path feeds only the appended byte window, not the full file.
        let path = Path::new("/tmp/grok-delta/updates.jsonl");
        let delta = br#"{"timestamp":"2026-05-03T00:02:06.000Z","method":"session/update","params":{"sessionId":"sid-delta","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"delta only"}}}}
"#;
        let parsed = parse_provider_reader(
            watch_provider("grok"),
            path,
            delta.to_vec(),
            ParseSelection::empty().with_messages(),
        )
        .unwrap();
        assert!(
            parsed.events.iter().any(|event| matches!(
                event,
                crate::watch::WatchEvent::AssistantMessage(message)
                    if message.text.as_deref() == Some("delta only")
            )),
            "delta window must parse without full-session context: {:?}",
            parsed.events
        );
    }

    #[cfg(feature = "grok")]
    #[test]
    fn grok_supports_incremental_watch_events() {
        assert!(watch_provider("grok").supports_incremental_watch_events());
        assert!(watch_provider("grok").needs_watch_state_seed());
        assert_eq!(
            watch_provider("grok").initial_watch_directory_depth(),
            Some(3)
        );
    }

    #[cfg(feature = "grok")]
    #[test]
    fn parse_provider_reader_grok_projects_user_assistant_thought_and_tools() {
        // F2.05: real parse_watch_reader path (descriptor → ProviderWatchEvents).
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/grok/updates.jsonl");
        let bytes = std::fs::read(&fixture).expect("grok fixture");
        // Sibling summary for title/cwd seed (same layout as real sessions).
        let session_dir = tempfile::tempdir().unwrap();
        let updates = session_dir.path().join("updates.jsonl");
        let summary = session_dir.path().join("summary.json");
        std::fs::write(&updates, &bytes).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/grok/summary.json"),
            &summary,
        )
        .unwrap();

        let parsed = parse_provider_reader(
            watch_provider("grok"),
            &updates,
            bytes,
            ParseSelection::full(),
        )
        .unwrap();

        assert_eq!(
            parsed.session_id.as_deref(),
            Some("019f4f1c-8ae8-7632-adb2-6133aee3adf3")
        );
        assert_eq!(parsed.cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(parsed.title.as_deref(), Some("Hello Grok fixture"));

        assert!(
            parsed.events.iter().any(|event| matches!(
                event,
                crate::watch::WatchEvent::UserMessage(message)
                    if message.text.as_deref() == Some("hello grok")
            )),
            "expected user message watch event, got {:?}",
            parsed.events
        );
        assert!(
            parsed.events.iter().any(|event| matches!(
                event,
                crate::watch::WatchEvent::AssistantMessage(message)
                    if message.phase.as_deref() == Some("thinking")
                        && message.text.as_deref() == Some("thinking about reply")
            )),
            "expected thought/reasoning watch event, got {:?}",
            parsed.events
        );
        assert!(
            parsed.events.iter().any(|event| matches!(
                event,
                crate::watch::WatchEvent::AssistantMessage(message)
                    if message.phase.is_none()
                        && message.text.as_deref() == Some("Hello from Grok")
            )),
            "expected assistant text watch event, got {:?}",
            parsed.events
        );
        assert!(
            parsed.events.iter().any(|event| matches!(
                event,
                crate::watch::WatchEvent::ToolCall(call)
                    if call.name.as_str() == "read_file"
                        && call.call_id.as_deref() == Some("call-1")
            )),
            "expected tool_call watch event, got {:?}",
            parsed.events
        );
        assert!(
            parsed.events.iter().any(|event| matches!(
                event,
                crate::watch::WatchEvent::ToolResult(result)
                    if result.call_id.as_deref() == Some("call-1")
                        && !result.is_error
                        && result.output_json.as_deref() == Some("file contents")
            )),
            "expected tool_result watch event, got {:?}",
            parsed.events
        );
        assert!(
            parsed
                .events
                .iter()
                .any(|event| matches!(event, crate::watch::WatchEvent::TurnCompleted(_))),
            "expected turn_completed watch event, got {:?}",
            parsed.events
        );
    }

    #[cfg(feature = "cursor")]
    #[test]
    fn parse_provider_reader_preserves_cursor_timestamp_and_trims_text() {
        let bytes = concat!(
            r#"{"timestamp":"2026-05-03T00:00:01.000Z","role":"user","message":{"content":"  cursor ping  "}}"#,
            "\n",
            r#"{"timestamp":"2026-05-03T00:00:02.000Z","role":"assistant","message":{"content":"  cursor pong  "}}"#,
        );

        let parsed = parse_provider_reader(
            watch_provider("cursor"),
            Path::new("/tmp/cursor-trim-session.jsonl"),
            bytes.as_bytes().to_vec(),
            ParseSelection::full(),
        )
        .unwrap();

        assert_eq!(parsed.session_id.as_deref(), Some("cursor-trim-session"));
        assert_eq!(parsed.events.len(), 2);

        let crate::watch::WatchEvent::UserMessage(user) = &parsed.events[0] else {
            panic!("expected cursor user message");
        };
        assert_eq!(
            user.meta.timestamp.as_deref(),
            Some("2026-05-03T00:00:01.000Z")
        );
        assert_eq!(user.text.as_deref(), Some("cursor ping"));

        let crate::watch::WatchEvent::AssistantMessage(assistant) = &parsed.events[1] else {
            panic!("expected cursor assistant message");
        };
        assert_eq!(
            assistant.meta.timestamp.as_deref(),
            Some("2026-05-03T00:00:02.000Z")
        );
        assert_eq!(assistant.text.as_deref(), Some("cursor pong"));
    }

    #[cfg(feature = "pi")]
    #[test]
    fn parse_provider_reader_pi_respects_state_selection_for_session_info_name() {
        let bytes = concat!(
            r#"{"type":"session","version":3,"id":"pi-named","timestamp":"2026-05-03T00:00:00.000Z","cwd":"/tmp/project","name":"renamed session"}"#,
            "\n",
            r#"{"type":"message","id":"u1","timestamp":"2026-05-03T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"ping"}]}}"#,
            "\n",
        );

        let parsed = parse_provider_reader(
            watch_provider("pi"),
            Path::new("/tmp/pi.jsonl"),
            bytes.as_bytes().to_vec(),
            ParseSelection::empty().with_meta().with_messages(),
        )
        .unwrap();

        assert_eq!(parsed.session_id.as_deref(), Some("pi-named"));
        assert_eq!(parsed.cwd.as_deref(), Some("/tmp/project"));
        assert!(
            parsed
                .events
                .iter()
                .all(|event| !matches!(event, crate::watch::WatchEvent::State(_))),
            "Pi session metadata should not leak state events when state is not selected"
        );
        assert!(
            parsed.events.iter().any(|event| matches!(
                event,
                crate::watch::WatchEvent::UserMessage(message)
                    if message.text.as_deref() == Some("ping")
            )),
            "Pi selected messages should still be emitted"
        );
    }

    #[cfg(feature = "pi")]
    #[test]
    fn parse_provider_reader_pi_message_selection_ignores_unused_usage_shape() {
        let bytes = concat!(
            r#"{"type":"session","version":3,"id":"pi-watch","timestamp":"2026-05-03T00:00:00.000Z","cwd":"/tmp/project"}"#,
            "\n",
            r#"{"type":"message","id":"a1","timestamp":"2026-05-03T00:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"pong"}],"usage":"message-watch-must-not-parse-usage"}}"#,
            "\n",
        );

        let parsed = parse_provider_reader(
            watch_provider("pi"),
            Path::new("/tmp/pi.jsonl"),
            bytes.as_bytes().to_vec(),
            ParseSelection::empty().with_meta().with_messages(),
        )
        .unwrap();

        assert!(parsed.events.iter().any(|event| matches!(
            event,
            crate::watch::WatchEvent::AssistantMessage(message)
                if message.text.as_deref() == Some("pong")
        )));
    }

    #[cfg(feature = "codex")]
    #[test]
    fn parse_provider_reader_meta_only_stops_before_later_malformed_line() {
        let bytes = concat!(
            r#"{"timestamp":"2026-04-16T00:00:00.000Z","type":"session_meta","payload":{"session_id":"sess-watch-reader","cwd":"/tmp/project","originator":"codex-cli","model":"gpt-5.4"}}"#,
            "\n",
            "not-json",
        );

        let parsed = parse_provider_reader(
            watch_provider("codex"),
            Path::new("/tmp/rollout.jsonl"),
            bytes.as_bytes().to_vec(),
            ParseSelection::meta_only(),
        )
        .unwrap();

        assert_eq!(parsed.session_id.as_deref(), Some("sess-watch-reader"));
        assert_eq!(parsed.cwd.as_deref(), Some("/tmp/project"));
        assert!(parsed.events.is_empty());
    }

    #[cfg(feature = "codex")]
    #[test]
    fn parse_provider_reader_codex_message_selection_ignores_unused_summary_shape() {
        let bytes = r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"pong"}],"summary":"message-watch-must-not-parse-summary"}}"#;

        let parsed = parse_provider_reader(
            watch_provider("codex"),
            Path::new("/tmp/rollout.jsonl"),
            bytes.as_bytes().to_vec(),
            ParseSelection::empty().with_meta().with_messages(),
        )
        .unwrap();

        assert!(parsed.events.iter().any(|event| matches!(
            event,
            crate::watch::WatchEvent::AssistantMessage(message)
                if message.text.as_deref() == Some("pong")
        )));
    }

    #[cfg(feature = "codex")]
    #[test]
    fn parse_provider_reader_codex_message_selection_skips_unselected_operation_payload_shape() {
        let bytes = r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","call_id":"call-1","summary":"message-watch-must-not-parse-operation-summary"}}"#;

        let parsed = parse_provider_reader(
            watch_provider("codex"),
            Path::new("/tmp/rollout.jsonl"),
            bytes.as_bytes().to_vec(),
            ParseSelection::empty().with_meta().with_messages(),
        )
        .unwrap();

        assert!(parsed.events.is_empty());
    }

    #[cfg(feature = "codex")]
    #[test]
    fn parse_provider_reader_codex_operation_selection_keeps_tool_result_shape() {
        let bytes = r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","call_id":"call-1","arguments":"{\"cmd\":\"echo hi\"}"}}"#;

        let parsed = parse_provider_reader(
            watch_provider("codex"),
            Path::new("/tmp/rollout.jsonl"),
            bytes.as_bytes().to_vec(),
            ParseSelection::empty().with_operations(),
        )
        .unwrap();

        assert_eq!(parsed.events.len(), 1);
        let crate::watch::WatchEvent::ToolResult(tool) = &parsed.events[0] else {
            panic!("Codex operation watch parity currently exposes tool requests as ToolResult");
        };
        assert_eq!(tool.name.as_str(), "exec_command");
        assert_eq!(tool.kind, crate::agent_session::OperationKind::Shell);
        assert_eq!(tool.phase, crate::agent_session::OperationPhase::Requested);
        assert!(tool.output_json.is_none());
        assert!(!tool.is_error);
        assert!(tool.call_id.is_none());
    }

    #[cfg(feature = "codex")]
    #[test]
    fn parse_provider_reader_codex_raw_unknown_selection_emits_unknown() {
        let bytes =
            r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"mystery","payload":{"value":1}}"#;

        let parsed = parse_provider_reader(
            watch_provider("codex"),
            Path::new("/tmp/rollout.jsonl"),
            bytes.as_bytes().to_vec(),
            ParseSelection::empty().with_raw_unknown(),
        )
        .unwrap();

        assert_eq!(parsed.events.len(), 1);
        let crate::watch::WatchEvent::Unknown(unknown) = &parsed.events[0] else {
            panic!("Codex raw-unknown watch selection should emit Unknown");
        };
        assert_eq!(unknown.kind.as_str(), "mystery");
        assert_eq!(
            unknown.meta.timestamp.as_deref(),
            Some("2026-05-03T00:00:02.000Z")
        );
    }

    #[cfg(feature = "claude")]
    #[test]
    fn parse_provider_reader_claude_message_selection_ignores_unused_usage_shape() {
        let bytes = r#"{"type":"assistant","sessionId":"claude-watch","cwd":"/tmp/project","timestamp":"2026-05-03T00:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"pong"}],"usage":"message-watch-must-not-parse-usage"}}"#;

        let parsed = parse_provider_reader(
            watch_provider("claude"),
            Path::new("/tmp/claude.jsonl"),
            bytes.as_bytes().to_vec(),
            ParseSelection::empty().with_meta().with_messages(),
        )
        .unwrap();

        assert_eq!(parsed.session_id.as_deref(), Some("claude-watch"));
        assert_eq!(parsed.cwd.as_deref(), Some("/tmp/project"));
        assert!(parsed.events.iter().any(|event| matches!(
            event,
            crate::watch::WatchEvent::AssistantMessage(message)
                if message.text.as_deref() == Some("pong")
        )));
    }

    #[cfg(feature = "copilot")]
    #[test]
    fn parse_provider_reader_copilot_operation_selection_keeps_tool_result_shape() {
        let bytes = r#"{"type":"assistant.message","timestamp":"2026-04-15T10:00:10Z","data":{"messageId":"msg-1","content":"done","toolRequests":[{"name":"bash","toolCallId":"call-bash-1","arguments":{"command":"git status"}}]}}"#;

        let parsed = parse_provider_reader(
            watch_provider("copilot"),
            Path::new("/tmp/events.jsonl"),
            bytes.as_bytes().to_vec(),
            ParseSelection::empty().with_operations(),
        )
        .unwrap();

        assert_eq!(parsed.events.len(), 1);
        let crate::watch::WatchEvent::ToolResult(tool) = &parsed.events[0] else {
            panic!("Copilot operation watch parity currently exposes tool requests as ToolResult");
        };
        assert_eq!(tool.name.as_str(), "bash");
        assert_eq!(tool.kind, crate::agent_session::OperationKind::Shell);
        assert_eq!(tool.phase, crate::agent_session::OperationPhase::Requested);
        assert!(tool.output_json.is_none());
        assert!(!tool.is_error);
        assert!(tool.call_id.is_none());
    }

    #[cfg(feature = "copilot")]
    #[test]
    fn parse_provider_reader_copilot_message_selection_ignores_unused_tool_request_shape() {
        let bytes = r#"{"type":"assistant.message","timestamp":"2026-04-15T10:00:10Z","data":{"messageId":"msg-1","content":"done","toolRequests":[{"name":"bash","toolCallId":"call-bash-1","arguments":7}]}}"#;

        let parsed = parse_provider_reader(
            watch_provider("copilot"),
            Path::new("/tmp/events.jsonl"),
            bytes.as_bytes().to_vec(),
            ParseSelection::empty().with_meta().with_messages(),
        )
        .unwrap();

        assert!(parsed.events.iter().any(|event| matches!(
            event,
            crate::watch::WatchEvent::AssistantMessage(message)
                if message.text.as_deref() == Some("done")
        )));
    }

    #[cfg(feature = "copilot")]
    #[test]
    fn parse_provider_reader_copilot_message_selection_skips_unselected_tool_execution_shape() {
        let bytes = r#"{"type":"tool.execution_start","timestamp":"2026-04-15T10:00:10Z","data":{"toolName":"bash","toolCallId":"call-bash-1","arguments":7}}"#;

        let parsed = parse_provider_reader(
            watch_provider("copilot"),
            Path::new("/tmp/events.jsonl"),
            bytes.as_bytes().to_vec(),
            ParseSelection::empty().with_meta().with_messages(),
        )
        .unwrap();

        assert!(parsed.events.is_empty());
    }

    #[cfg(feature = "codex")]
    #[test]
    fn parse_provider_metadata_reader_stops_before_later_malformed_line() {
        let bytes = concat!(
            r#"{"timestamp":"2026-04-16T00:00:00.000Z","type":"session_meta","payload":{"session_id":"sess-watch-meta-reader","cwd":"/tmp/project","originator":"codex-cli","model":"gpt-5.4"}}"#,
            "\n",
            "not-json",
        );

        let parsed = super::parse_provider_metadata_bytes(
            watch_provider("codex"),
            Path::new("/tmp/rollout.jsonl"),
            bytes.as_bytes().to_vec(),
        )
        .unwrap();

        assert_eq!(parsed.session_id.as_deref(), Some("sess-watch-meta-reader"));
        assert_eq!(parsed.cwd.as_deref(), Some("/tmp/project"));
        assert!(parsed.events.is_empty());
    }

    #[cfg(feature = "claude")]
    #[test]
    fn parse_provider_metadata_reader_keeps_reading_claude_until_cwd() {
        let bytes = concat!(
            r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-05-01T11:09:46.280Z","sessionId":"sess-claude-watch"}"#,
            "\n",
            r#"{"type":"user","sessionId":"sess-claude-watch","cwd":"/work/project","timestamp":"2026-05-01T11:09:46.305Z"}"#,
            "\n",
        );

        let parsed = super::parse_provider_metadata_bytes(
            watch_provider("claude"),
            Path::new("/tmp/claude.jsonl"),
            bytes.as_bytes().to_vec(),
        )
        .unwrap();

        assert_eq!(parsed.session_id.as_deref(), Some("sess-claude-watch"));
        assert_eq!(parsed.cwd.as_deref(), Some("/work/project"));
        assert!(parsed.events.is_empty());
    }
}
