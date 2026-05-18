#![cfg(all(feature = "agent_session", feature = "discovery"))]

#[cfg(any(
    feature = "codex",
    feature = "gemini",
    feature = "copilot",
    feature = "cursor"
))]
fn fixture(path: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(path)
}

#[test]
fn provider_catalog_exposes_descriptor_boundary_for_enabled_providers() {
    let providers = agent_sessions::agent_providers();
    let ids = providers
        .iter()
        .map(|provider| provider.id())
        .collect::<Vec<_>>();

    #[cfg(feature = "codex")]
    assert!(ids.contains(&"codex"));
    #[cfg(feature = "claude")]
    assert!(ids.contains(&"claude"));
    #[cfg(feature = "copilot")]
    assert!(ids.contains(&"copilot"));
    #[cfg(feature = "cursor")]
    assert!(ids.contains(&"cursor"));
    #[cfg(feature = "gemini")]
    assert!(ids.contains(&"gemini"));
    #[cfg(feature = "pi")]
    assert!(ids.contains(&"pi"));
}

#[test]
fn provider_lookup_rejects_unknown_ids_without_fallback() {
    assert!(agent_sessions::agent_provider("not-a-provider").is_none());
}

#[cfg(feature = "codex")]
#[test]
fn codex_descriptor_parses_metadata_from_candidate_entries() {
    let path = fixture("codex/codex_current_sample.jsonl");
    let provider = agent_sessions::agent_provider("codex").expect("codex provider");
    let meta = provider.parse_file_meta(path).unwrap();

    assert!(meta.session_id.is_some());
    assert!(meta.cwd.is_some());
}

#[cfg(feature = "gemini")]
#[test]
fn gemini_descriptor_parses_json_document_metadata_without_history_replay() {
    let path = fixture("gemini/session-sample.json");
    let provider = agent_sessions::agent_provider("gemini").expect("gemini provider");
    let meta = provider.parse_file_meta(path).unwrap();

    assert!(meta.session_id.is_some());
    assert!(meta.source_kind.is_some());
    assert_eq!(
        provider.session_file_format(),
        agent_sessions::SessionFileFormat::JsonDocument
    );
}

#[cfg(feature = "copilot")]
#[test]
fn copilot_descriptor_parses_single_file_metadata_without_public_sidecar_entries() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    std::fs::write(
        &path,
        concat!(
            r#"{"type":"session.start","timestamp":"2026-05-11T00:49:47.936Z","data":{"sessionId":"copilot-public-source","selectedModel":"gpt-5"}}"#,
            "\n",
            r#"{"type":"user.message","timestamp":"2026-05-11T00:49:48.936Z","data":{"messageId":"u1","content":"public source title"}}"#,
            "\n",
        ),
    )
    .unwrap();
    let provider = agent_sessions::agent_provider("copilot").expect("copilot provider");
    let meta = provider.parse_file_meta(path).unwrap();

    assert_eq!(meta.session_id.as_deref(), Some("copilot-public-source"));
    assert_eq!(meta.title.as_deref(), Some("public source title"));
}

#[cfg(feature = "cursor")]
#[test]
fn cursor_descriptor_parses_semantic_bytes_with_reader_metadata_defaults() {
    let bytes = std::fs::read(fixture("cursor/transcript.jsonl")).unwrap();
    let provider = agent_sessions::agent_provider("cursor").expect("cursor provider");
    let session = provider
        .parse_agent_session_bytes(bytes, agent_sessions::ParseSelection::full())
        .unwrap();

    assert_eq!(session.agent.as_str(), "cursor");
    assert!(
        session
            .events
            .iter()
            .any(|event| matches!(event.body, agent_sessions::agent_session::Body::Prompt(_)))
    );
}

#[cfg(feature = "pi")]
#[test]
fn pi_descriptor_parses_inline_semantic_bytes() {
    let bytes = [
        r#"{"type":"session","version":3,"id":"pi-inline","timestamp":"2026-05-03T00:00:00.000Z","cwd":"/tmp/project"}"#,
        r#"{"type":"message","id":"u1","timestamp":"2026-05-03T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}"#,
    ]
    .join("\n")
    .into_bytes();
    let provider = agent_sessions::agent_provider("pi").expect("pi provider");
    let session = provider
        .parse_agent_session_bytes(bytes, agent_sessions::ParseSelection::full())
        .unwrap();

    assert_eq!(session.meta.session_id.as_deref(), Some("pi-inline"));
    assert!(
        session
            .events
            .iter()
            .any(|event| matches!(event.actor, agent_sessions::agent_session::Actor::User))
    );
}
