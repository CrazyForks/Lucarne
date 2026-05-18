//! Copilot fixture scenarios — drive the Copilot adapter against each
//! tests/data/copilot fixture via a raw `cat`-the-fixture shell wrapper
//! (fixtures are plain JSONL, not fakeagent scripts).

pub mod common;

use common::{
    collect_timelines, find_timeline, fixture_path, kinds, run_scenario, write_cat_script, Scenario,
};
use lucarne::adapters::copilot;
use lucarne::event::{Kind, Payload, TimelineType};
use serde_json::Value;
use std::time::Duration;

fn adapter_for(fixture_name: &str) -> std::sync::Arc<lucarne::adapter::ProtocolAdapter> {
    let fixture = fixture_path("copilot", fixture_name);
    let bin = write_cat_script(&fixture);
    copilot::new(copilot::Options {
        binary: bin.to_string_lossy().into_owned(),
    })
}

#[tokio::test]
async fn basic_jsonl_flow() {
    let adapter = adapter_for("basic.jsonl");
    let mut sc = Scenario::new(adapter, fixture_path("copilot", "basic.jsonl"));
    sc.first_prompt = "write a haiku".into();
    sc.model = "gpt-4o".into();
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(
        r.closed,
        "expected SessionClosed; kinds = {:?}",
        kinds(&r.events)
    );

    // Session started with sess-1 / claude-sonnet-4
    let started = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::SessionStarted(s) => Some(s.clone()),
            _ => None,
        })
        .expect("SessionStarted");
    assert_eq!(started.session_id, "sess-1");
    assert_eq!(started.model, "claude-sonnet-4");
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnStarted));

    // Assistant message "pong"
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    assert!(
        msgs.iter()
            .any(|m| m.assistant_message.as_ref().unwrap().text == "pong"),
        "expected assistant 'pong' in {:?}",
        msgs
    );

    // Reasoning
    let reasoning = find_timeline(&r.events, TimelineType::Reasoning).expect("reasoning");
    assert_eq!(reasoning.reasoning.unwrap().text, "thinking step");

    let log_line = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::Log(log) => Some(log.clone()),
            _ => None,
        })
        .expect("Log");
    assert_eq!(log_line.level, "warn");
    assert_eq!(log_line.text, "approaching rate limit");

    // Tool call (bash ls -1)
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    let shell = &calls[0].tool_call.as_ref().unwrap().call;
    assert_eq!(shell.name.as_str(), "bash");
    assert_eq!(
        shell.input.get("command").and_then(Value::as_str),
        Some("ls -1")
    );

    // Tool result
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].tool_result.as_ref().unwrap().result.output,
        "AGENTS.md\n"
    );

    let delta = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::UsageDelta(delta) => Some(delta.clone()),
            _ => None,
        })
        .expect("UsageDelta");
    assert_eq!(delta.delta.output_tokens, 7);

    // Session closed with resume sess-1
    let closed = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::SessionClosed(c) => Some(c.clone()),
            _ => None,
        })
        .expect("SessionClosed");
    let resume = closed.resume.expect("resume");
    assert_eq!(
        resume.data.get("session_id").and_then(|v| v.as_str()),
        Some("sess-1")
    );
}

#[tokio::test]
async fn failure_surfaces_turn_failed() {
    let adapter = adapter_for("failure.jsonl");
    let mut sc = Scenario::new(adapter, fixture_path("copilot", "failure.jsonl"));
    sc.first_prompt = "fail".into();
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed);
    let mut failures: Vec<_> = r
        .events
        .iter()
        .filter_map(|e| match &e.payload {
            Payload::TurnFailed(t) => Some(t.error.clone()),
            _ => None,
        })
        .collect();
    failures.sort();
    assert_eq!(
        failures,
        vec![
            String::from("Rate limit exceeded"),
            String::from("copilot exited with code 1"),
        ]
    );
}

#[tokio::test]
async fn noise_tool_error_keeps_error_text() {
    let adapter = adapter_for("noise_tool_error.jsonl");
    let mut sc = Scenario::new(adapter, fixture_path("copilot", "noise_tool_error.jsonl"));
    sc.first_prompt = "noise".into();
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed);
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    let tr = results[0].tool_result.as_ref().unwrap();
    assert!(tr.result.error.contains("foobar"), "got {:?}", tr.result);
}

#[tokio::test]
async fn partial_reasoning_includes_reasoning() {
    let adapter = adapter_for("partial_reasoning.jsonl");
    let mut sc = Scenario::new(adapter, fixture_path("copilot", "partial_reasoning.jsonl"));
    sc.first_prompt = "partial".into();
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed);
    let reasoning = collect_timelines(&r.events, TimelineType::Reasoning);
    assert_eq!(reasoning.len(), 2);
    assert_eq!(
        reasoning[0].reasoning.as_ref().unwrap().text,
        "Let me think about this..."
    );
    assert_eq!(
        reasoning[1].reasoning.as_ref().unwrap().text,
        "thinking step"
    );
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].assistant_message.as_ref().unwrap().text, "hello ");
    assert_eq!(msgs[1].assistant_message.as_ref().unwrap().text, "world");
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnCompleted));
}
