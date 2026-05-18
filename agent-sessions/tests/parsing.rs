#![cfg(all(
    feature = "agent_session",
    feature = "discovery",
    any(
        feature = "codex",
        feature = "claude",
        feature = "copilot",
        feature = "cursor",
        feature = "gemini",
        feature = "pi"
    )
))]

use agent_sessions::{ParseSelection, agent_session::Body};

#[cfg(any(
    feature = "codex",
    feature = "claude",
    feature = "copilot",
    feature = "cursor",
    feature = "gemini"
))]
fn fixture(path: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(path)
}

fn parse_provider(
    provider_id: &str,
    bytes: Vec<u8>,
    selection: ParseSelection,
) -> agent_sessions::agent_session::Session {
    agent_sessions::agent_provider(provider_id)
        .expect("provider descriptor")
        .parse_agent_session_bytes(bytes, selection)
        .unwrap()
}

#[cfg(feature = "codex")]
#[test]
fn parses_codex_fixture_to_semantic_session() {
    let bytes = std::fs::read(fixture("codex/codex_current_sample.jsonl")).unwrap();
    let session = parse_provider("codex", bytes, ParseSelection::full());

    assert_eq!(session.agent.as_str(), "codex");
    assert_eq!(session.version.as_str(), "codex-v2");
    assert!(session.meta.session_id.is_some());
    assert!(
        session
            .events
            .iter()
            .any(|event| matches!(event.body, Body::Prompt(_)))
    );
    assert!(
        session
            .events
            .iter()
            .any(|event| matches!(event.body, Body::Operation(_)))
    );
}

#[cfg(feature = "claude")]
#[test]
fn parses_claude_fixture_to_semantic_session() {
    let bytes = std::fs::read(fixture("claude/claude_current_sample.jsonl")).unwrap();
    let session = parse_provider("claude", bytes, ParseSelection::full());

    assert_eq!(session.agent.as_str(), "claude");
    assert!(session.meta.cwd.is_some());
    assert!(session.events.iter().any(|event| matches!(
        event.actor,
        agent_sessions::agent_session::Actor::User
    ) && matches!(event.body, Body::Prompt(_))));
    assert!(
        session
            .events
            .iter()
            .any(|event| matches!(event.actor, agent_sessions::agent_session::Actor::Assistant))
    );
}

#[cfg(feature = "copilot")]
#[test]
fn parses_copilot_cli_fixture_to_semantic_session() {
    let bytes = std::fs::read(fixture("copilot/events.jsonl")).unwrap();
    let session = parse_provider("copilot", bytes, ParseSelection::full());

    assert_eq!(session.agent.as_str(), "copilot");
    assert!(
        session
            .events
            .iter()
            .any(|event| matches!(event.body, Body::Prompt(_)))
    );
    assert!(
        session
            .events
            .iter()
            .any(|event| matches!(event.body, Body::Response(_)))
    );
}

#[cfg(feature = "cursor")]
#[test]
fn parses_cursor_fixture_to_semantic_session() {
    let bytes = std::fs::read(fixture("cursor/transcript.jsonl")).unwrap();
    let session = parse_provider("cursor", bytes, ParseSelection::full());

    assert_eq!(session.agent.as_str(), "cursor");
    assert!(
        session
            .events
            .iter()
            .any(|event| matches!(event.body, Body::Prompt(_)))
    );
    assert!(session.events.iter().any(|event| {
        matches!(
            &event.body,
            Body::Response(response)
                if response.blocks.iter().any(|block| {
                    matches!(
                        block,
                        agent_sessions::agent_session::ContentBlock::ToolUse(_)
                    )
                })
        )
    }));
}

#[cfg(feature = "gemini")]
#[test]
fn parses_gemini_fixture_to_semantic_session() {
    let bytes = std::fs::read(fixture("gemini/session-sample.json")).unwrap();
    let session = parse_provider("gemini", bytes, ParseSelection::full());

    assert_eq!(session.agent.as_str(), "gemini");
    assert!(session.meta.session_id.is_some());
    assert!(
        session
            .events
            .iter()
            .any(|event| matches!(event.body, Body::Prompt(_)))
    );
    assert!(
        session
            .events
            .iter()
            .any(|event| matches!(event.body, Body::Response(_)))
    );
}

#[cfg(feature = "pi")]
#[test]
fn parses_pi_jsonl_to_semantic_session() {
    let bytes = [
        r#"{"type":"session","version":3,"id":"pi-session","timestamp":"2026-05-03T00:00:00.000Z","cwd":"/tmp/project"}"#,
        r#"{"type":"message","id":"u1","timestamp":"2026-05-03T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}"#,
        r#"{"type":"message","id":"a1","timestamp":"2026-05-03T00:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}"#,
    ]
    .join("\n")
    .into_bytes();
    let session = parse_provider("pi", bytes, ParseSelection::full());

    assert_eq!(session.agent.as_str(), "pi");
    assert_eq!(session.meta.session_id.as_deref(), Some("pi-session"));
    assert_eq!(session.meta.cwd.as_deref(), Some("/tmp/project"));
    assert!(session.events.iter().any(|event| matches!(
        event.actor,
        agent_sessions::agent_session::Actor::User
    ) && matches!(event.body, Body::Prompt(_))));
}

#[cfg(feature = "codex")]
#[test]
fn codex_probe_session_meta_stops_after_first_session_meta() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rollout.jsonl");
    let bytes = [
        r#"{"timestamp":"2026-04-16T00:00:00.000Z","type":"session_meta","payload":{"session_id":"sess-probe","cwd":"/tmp/project","originator":"Codex Desktop","model":"gpt-5.4","cli_version":"0.120.0"}}"#,
        "not-json",
    ]
    .join("\n");
    std::fs::write(&path, bytes).unwrap();

    let provider = agent_sessions::agent_provider("codex").expect("codex provider");
    let meta = provider.parse_file_meta(path).unwrap();

    assert_eq!(meta.session_id.as_deref(), Some("sess-probe"));
    assert_eq!(meta.cwd.as_deref(), Some("/tmp/project"));
    assert_eq!(
        meta.models.first().map(|model| model.model.as_str()),
        Some("gpt-5.4")
    );
    assert_eq!(meta.source_kind.as_deref(), Some("v1"));
}

#[cfg(feature = "codex")]
#[test]
fn codex_probe_session_meta_with_title_skips_instruction_preamble() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rollout.jsonl");
    let bytes = [
        r##"{"timestamp":"2026-05-08T01:13:41.757Z","type":"session_meta","payload":{"id":"019e0525","cwd":"/tmp/project","originator":"codex-tui","cli_version":"0.128.0"}}"##,
        r##"{"timestamp":"2026-05-08T01:15:10.683Z","type":"event_msg","payload":{"type":"task_started"}}"##,
        r##"{"timestamp":"2026-05-08T01:15:10.686Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<permissions instructions>\nfoo"}]}}"##,
        r##"{"timestamp":"2026-05-08T01:15:10.686Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions for /tmp/project\n\n<INSTRUCTIONS>\nfoo"}]}}"##,
        r##"{"timestamp":"2026-05-08T01:15:10.687Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"investigate the production bug"}]}}"##,
    ]
    .join("\n");
    std::fs::write(&path, bytes).unwrap();

    let provider = agent_sessions::agent_provider("codex").expect("codex provider");
    let meta = provider.parse_file_meta(path).unwrap();

    assert_eq!(meta.session_id.as_deref(), Some("019e0525"));
    assert_eq!(meta.cwd.as_deref(), Some("/tmp/project"));
    assert_eq!(
        meta.title.as_deref(),
        Some("investigate the production bug")
    );
}

#[cfg(feature = "codex")]
#[test]
fn codex_extended_response_items_project_to_semantic_operations_and_blocks() {
    let bytes = [
        r#"{"timestamp":"2026-03-04T11:20:59.764Z","type":"response_item","payload":{"type":"custom_tool_call","status":"completed","call_id":"call_1","name":"apply_patch","input":"*** Begin Patch"}}"#,
        r#"{"timestamp":"2026-03-04T11:21:00.000Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call_1","output":"{\"output\":\"ok\"}"}}"#,
        r#"{"timestamp":"2026-03-04T11:21:01.000Z","type":"response_item","payload":{"type":"web_search_call","status":"completed","action":{"type":"search","query":"rust serde","queries":["rust serde","serde rawvalue"]}}}"#,
        r#"{"timestamp":"2026-03-04T11:21:04.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,abc"}]}}"#,
    ]
    .join("\n")
    .into_bytes();

    let session = parse_provider("codex", bytes, ParseSelection::full());

    assert!(session.events.iter().any(|event| matches!(
        &event.body,
        Body::Operation(operation)
            if operation.kind == agent_sessions::agent_session::OperationKind::Edit
                && operation.name.as_str() == "apply_patch"
    )));
    assert!(session.events.iter().any(|event| matches!(
        &event.body,
        Body::Operation(operation)
            if operation.kind == agent_sessions::agent_session::OperationKind::Web
    )));
    assert!(session.events.iter().any(|event| matches!(
        &event.body,
        Body::Prompt(prompt)
            if prompt.blocks.iter().any(|block| matches!(
                block,
                agent_sessions::agent_session::ContentBlock::Image(image)
                    if image.image_url.as_str() == "data:image/png;base64,abc"
            ))
    )));
}

#[cfg(feature = "codex")]
#[test]
fn selection_filters_semantic_events_without_raw_projection_surface() {
    let bytes = std::fs::read(fixture("codex/codex_current_sample.jsonl")).unwrap();
    let messages = parse_provider(
        "codex",
        bytes,
        ParseSelection::empty().with_meta().with_messages(),
    );

    assert!(messages.meta.session_id.is_some());
    assert!(
        messages
            .events
            .iter()
            .all(|event| matches!(event.body, Body::Prompt(_) | Body::Response(_)))
    );
}
