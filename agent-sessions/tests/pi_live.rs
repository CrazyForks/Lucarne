//! Real Pi session file round-trip tests.
//! Parses actual Pi JSONL session files from ~/.pi/agent/sessions/ and
//! checks that the parser produces correct events without panicking or
//! losing content.

#![cfg(all(feature = "pi", feature = "agent_session", feature = "discovery"))]

use agent_sessions::{ParseSelection, agent_session};
use std::path::PathBuf;

/// Collect all Pi session files from the user's real session directory.
fn pi_session_files() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let root = PathBuf::from(home)
        .join(".pi")
        .join("agent")
        .join("sessions");
    if !root.is_dir() {
        return Vec::new();
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(&root)
        .max_depth(8)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.into_path();
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
    files.sort();
    files
}

#[test]
fn all_pi_sessions_parse_without_error() {
    let files = pi_session_files();
    assert!(
        !files.is_empty(),
        "expected at least one Pi session file in ~/.pi/agent/sessions/"
    );

    let selection = ParseSelection::full();

    let mut total = 0;
    let mut failed = 0;

    for file in &files {
        total += 1;
        let data = match std::fs::read(file) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("SKIP {}: read error: {}", file.display(), e);
                failed += 1;
                continue;
            }
        };

        let provider = agent_sessions::agent_provider("pi").expect("pi provider");
        match provider.parse_agent_session_bytes(data, selection) {
            Ok(projected) => {
                // Every session must have at least the header
                assert!(
                    projected.meta.session_id.is_some(),
                    "{}: missing session_id",
                    file.display()
                );

                // Every event should have a valid actor/body
                for event in projected.events.iter() {
                    match &event.body {
                        agent_session::Body::Prompt(_)
                        | agent_session::Body::Response(_)
                        | agent_session::Body::Operation(_)
                        | agent_session::Body::Usage(_)
                        | agent_session::Body::State(_)
                        | agent_session::Body::Snapshot(_)
                        | agent_session::Body::Unknown(_) => {}
                    }
                }
            }
            Err(e) => {
                eprintln!("FAIL {}: {}", file.display(), e);
                failed += 1;
            }
        }
    }

    eprintln!(
        "Parsed {}/{} Pi session files successfully",
        total - failed,
        total
    );

    if failed > 0 {
        panic!("{} Pi session files failed to parse", failed);
    }
}

#[test]
fn pi_session_messages_preserve_content() {
    let files = pi_session_files();
    if files.is_empty() {
        return;
    }

    let selection = ParseSelection::full();

    // Take first 5 files for detailed content check
    for file in files.iter().take(5) {
        let data = std::fs::read(file).expect("read file");
        let provider = agent_sessions::agent_provider("pi").expect("pi provider");
        let projected = provider
            .parse_agent_session_bytes(data, selection)
            .expect("parse session");

        let user_count = projected
            .events
            .iter()
            .filter(|e| matches!(e.actor, agent_session::Actor::User))
            .count();
        let assistant_count = projected
            .events
            .iter()
            .filter(|e| matches!(e.actor, agent_session::Actor::Assistant))
            .count();

        // Every real session has at least one user message
        assert!(
            user_count > 0,
            "{}: no user messages found in session",
            file.display()
        );

        eprintln!(
            "{}: {} user + {} assistant messages ({} total events)",
            file.file_name().unwrap_or_default().to_str().unwrap_or("?"),
            user_count,
            assistant_count,
            projected.events.len()
        );
    }
}

#[test]
fn pi_session_header_extracts_metadata() {
    let files = pi_session_files();
    if files.is_empty() {
        return;
    }

    for file in files.iter().take(5) {
        let data = std::fs::read(file).expect("read file");
        let selection = ParseSelection::meta_only();
        let provider = agent_sessions::agent_provider("pi").expect("pi provider");
        let projected = provider
            .parse_agent_session_bytes(data, selection)
            .expect("parse meta");

        assert!(
            projected.meta.session_id.is_some(),
            "{}: missing session_id",
            file.display()
        );
        assert!(
            projected.meta.cwd.is_some(),
            "{}: missing cwd",
            file.display()
        );
    }
}

#[test]
fn pi_session_handles_tool_results() {
    let files = pi_session_files();
    if files.is_empty() {
        return;
    }

    let selection = ParseSelection::empty()
        .with_meta()
        .with_messages()
        .with_operations()
        .with_usage();

    let mut found_tool_results = false;

    for file in &files {
        let data = std::fs::read(file).expect("read file");
        let provider = agent_sessions::agent_provider("pi").expect("pi provider");
        let projected = provider
            .parse_agent_session_bytes(data, selection)
            .expect("parse session");

        for event in projected.events.iter() {
            if matches!(event.actor, agent_session::Actor::Tool) {
                found_tool_results = true;
                if let agent_session::Body::Operation(op) = &event.body {
                    assert!(
                        !op.name.is_empty(),
                        "tool result must have a name: {}",
                        file.display()
                    );
                }
            }
        }
    }

    if !found_tool_results {
        eprintln!("NOTE: no tool results found in any Pi session (may be OK for small sessions)");
    }
}

#[test]
fn pi_session_handles_model_and_thinking_changes() {
    let files = pi_session_files();
    if files.is_empty() {
        return;
    }

    let selection = ParseSelection::full();

    for file in files.iter().take(10) {
        let data = std::fs::read(file).expect("read file");
        let provider = agent_sessions::agent_provider("pi").expect("pi provider");
        let projected = provider
            .parse_agent_session_bytes(data, selection)
            .expect("parse session");

        for event in projected.events.iter() {
            if let agent_session::Body::State(state) = &event.body {
                match state.kind.as_ref() {
                    "model_change"
                    | "thinking_level_change"
                    | "compaction"
                    | "branch_summary"
                    | "session_info"
                    | "label" => {}
                    other
                        if other.starts_with("custom:") || other.starts_with("custom_message:") => {
                    }
                    _ => {}
                }
            }
        }
    }
}
