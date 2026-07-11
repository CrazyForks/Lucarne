use lucarne::history::{
    entry_at_for_providers, history_transcript_for_entry, list_page_for_providers, HistoryCursor,
    HistoryEntry,
};

#[test]
fn history_discovery_moves_default_roots_without_cloning_root_vectors() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../agent-sessions/src/providers/descriptor.rs"),
    )
    .expect("read provider descriptor source");

    assert!(
        !source.contains("discover_in(roots.clone())"),
        "history discovery should log root metadata before moving default_roots into discovery"
    );
}

#[test]
fn history_transcript_hot_path_uses_bounded_byte_cursor() {
    let lucarne_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/history/transcript.rs"),
    )
    .expect("read history transcript source");
    let agent_sessions_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../agent-sessions/src/agent.rs"),
    )
    .expect("read agent-sessions agent trait source");

    assert!(
        !lucarne_source.contains("read_to_string(&entry.session_path)"),
        "history transcript replay must not read the full session file on the hot path"
    );
    for forbidden in ["File::open", "SeekFrom", "read_exact"] {
        assert!(
            !lucarne_source.contains(forbidden),
            "history transcript hot path must use SessionReader instead of direct file IO: {forbidden}"
        );
    }
    assert!(
        lucarne_source.contains("reader::SessionReader"),
        "history transcript hot path should use agent-sessions SessionReader boundary"
    );
    assert!(
        lucarne_source.contains("HISTORY_TAIL_CURSOR_PREFIX"),
        "history cursor/window semantics belong in lucarne, not agent-sessions"
    );
    assert!(
        !agent_sessions_source.contains("transcript_cursor_prefix"),
        "agent-sessions must not own history cursor semantics"
    );
}

#[test]
fn history_transcript_rejects_legacy_before_cursor() {
    let tmp = tempfile::TempDir::new().expect("temp history");
    let path = tmp.path().join("rollout-test.jsonl");
    std::fs::write(
        &path,
        [
            r#"{"timestamp":"2026-05-15T00:00:00.000Z","type":"session_meta","payload":{"session_id":"sess-byte","cwd":"/tmp/project","originator":"codex-cli","model":"gpt-5.5"}}"#,
            r#"{"timestamp":"2026-05-15T00:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"first"}]}}"#,
            r#"{"timestamp":"2026-05-15T00:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"one"}]}}"#,
            r#"{"timestamp":"2026-05-15T00:00:03.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"second"}]}}"#,
            r#"{"timestamp":"2026-05-15T00:00:04.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"two"}]}}"#,
        ]
        .join("\n"),
    )
    .expect("write history");
    let entry = HistoryEntry {
        provider_id: "codex",
        session_id: "sess-byte".into(),
        session_path: path,
        cwd: Some("/tmp/project".into()),
        summary: "first".into(),
        last_active_unix: 0,
        last_active_display: String::new(),
    };

    let transcript = history_transcript_for_entry(&entry, 1, None).expect("transcript");
    assert_eq!(transcript.turns.len(), 1);
    assert_eq!(transcript.turns[0].user.text, "second");
    let cursor = transcript
        .older_cursor
        .as_ref()
        .expect("older cursor")
        .as_str();
    assert!(
        cursor.starts_with("history-before-byte:"),
        "older cursor must be a byte cursor, got {cursor:?}"
    );

    let err = history_transcript_for_entry(&entry, 1, Some(&HistoryCursor::new("before:1")))
        .expect_err("legacy cursor must be rejected");
    assert!(
        err.to_string()
            .contains("missing history-before-byte: prefix"),
        "unexpected cursor error: {err}"
    );
}

#[test]
fn journey_58_linkme_history_descriptor_discovers_and_replays_new_provider() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let history_source =
        std::fs::read_to_string(manifest.join("src/history/mod.rs")).expect("read history source");
    let provider_source = std::fs::read_to_string(manifest.join("src/history/provider.rs"))
        .expect("read history provider source");
    let production_history = history_source
        .split("#[cfg(test)]")
        .next()
        .expect("production history source");

    assert!(
        provider_source.contains("pub struct HistoryProviderDescriptor"),
        "history provider descriptor belongs in lucarne"
    );
    assert!(
        production_history.contains("mod provider;"),
        "history provider descriptor should live in a cohesive submodule"
    );
    assert!(
        production_history.contains("mod transcript;"),
        "bounded transcript replay should live in a cohesive submodule"
    );
    assert!(
        !production_history.contains("match entry.provider_id"),
        "history hot path must route through descriptors, not provider-id dispatch"
    );
    assert!(
        !provider_source.contains("agent_sessions::Gemini")
            && !provider_source.contains("agent_sessions::Codex")
            && !provider_source.contains("gemini_transcript_unavailable"),
        "lucarne history must not special-case concrete providers"
    );

    let ids = lucarne::history::history_providers()
        .into_iter()
        .map(|provider| provider.id())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"codex"));
    assert!(ids.contains(&"pi"));
    assert!(ids.contains(&"grok"));
}

#[test]
fn history_api_is_available_from_core_and_filters_unsupported_providers() {
    let (page, total) = list_page_for_providers(&["not-a-provider"], 0, 10);

    assert!(page.is_empty());
    assert_eq!(total, 0);
    assert!(entry_at_for_providers(&["not-a-provider"], 0).is_none());
}
