//! Gemini fixture scenarios.

pub mod common;

use common::{collect_timelines, fakeagent_bin, fixture_path, kinds, run_scenario, Scenario};
use lucarne::adapters::gemini;
use lucarne::event::ResumeHandle;
use lucarne::event::{Decision, Kind, Payload, PermissionResponse, TimelineType};
use std::collections::BTreeMap;
use std::time::Duration;

fn adapter() -> std::sync::Arc<lucarne::adapter::ProtocolAdapter> {
    gemini::new(gemini::Options {
        binary: fakeagent_bin().to_string_lossy().into_owned(),
    })
}

#[tokio::test]
async fn basic() {
    let mut sc = Scenario::new(adapter(), fixture_path("gemini", "basic.fixture"));
    sc.first_prompt = "Hello Gemini".into();
    sc.drive_send = true;
    sc.model = String::new();
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let started = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::SessionStarted(s) => Some(s.clone()),
            _ => None,
        })
        .expect("SessionStarted");
    assert_eq!(started.session_id, "gemini-sess-basic");
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].assistant_message.as_ref().unwrap().text,
        "Hello! How can I help you today?"
    );
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnCompleted));
    let done = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnCompleted(t) => Some(t.clone()),
            _ => None,
        })
        .expect("TurnCompleted");
    assert_eq!(done.usage.as_ref().unwrap().input_tokens, 5);
    assert_eq!(done.usage.as_ref().unwrap().output_tokens, 9);
}

#[tokio::test]
async fn tool_success_with_output() {
    let mut sc = Scenario::new(
        adapter(),
        fixture_path("gemini", "tool_success_output.fixture"),
    );
    sc.first_prompt = "run a command".into();
    sc.drive_send = true;
    sc.model = String::new();
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].tool_result.as_ref().unwrap().result.output,
        "hello"
    );
    let done = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnCompleted(t) => Some(t.clone()),
            _ => None,
        })
        .expect("TurnCompleted");
    assert_eq!(done.usage.as_ref().unwrap().input_tokens, 6);
    assert_eq!(done.usage.as_ref().unwrap().output_tokens, 2);
}

#[tokio::test]
async fn tool_success_empty_output() {
    let mut sc = Scenario::new(
        adapter(),
        fixture_path("gemini", "tool_success_empty_output.fixture"),
    );
    sc.first_prompt = "run a quiet command".into();
    sc.drive_send = true;
    sc.model = String::new();
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed);
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].tool_result.as_ref().unwrap().result.output, "");
    assert_eq!(results[0].tool_result.as_ref().unwrap().result.error, "");
    let done = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnCompleted(t) => Some(t.clone()),
            _ => None,
        })
        .expect("TurnCompleted");
    assert_eq!(done.usage.as_ref().unwrap().input_tokens, 5);
    assert_eq!(done.usage.as_ref().unwrap().output_tokens, 1);
}

#[tokio::test]
async fn tool_failure_becomes_error() {
    let mut sc = Scenario::new(adapter(), fixture_path("gemini", "tool_failure.fixture"));
    sc.first_prompt = "run a failing command".into();
    sc.drive_send = true;
    sc.model = String::new();
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed);
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].tool_result.as_ref().unwrap().result.error,
        "permission denied"
    );
}

#[tokio::test]
async fn error_response_emits_turn_failed() {
    let mut sc = Scenario::new(adapter(), fixture_path("gemini", "error.fixture"));
    sc.first_prompt = "bad request".into();
    sc.drive_send = true;
    sc.model = String::new();
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed);
    let fail = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnFailed(t) => Some(t.clone()),
            _ => None,
        })
        .expect("TurnFailed");
    assert!(fail.error.contains("quota"), "{}", fail.error);
}

#[tokio::test]
async fn permission_allow_flow() {
    let mut sc = Scenario::new(adapter(), fixture_path("gemini", "permission.fixture"));
    sc.first_prompt = "Use tools".into();
    sc.drive_send = true;
    sc.model = String::new();
    sc.on_permission = Some(std::sync::Arc::new(|_req| {
        PermissionResponse::from_decision(Decision::Allow)
    }));
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let req = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::PermissionRequest(req) => Some(req.clone()),
            _ => None,
        })
        .expect("PermissionRequest");
    assert_eq!(req.tool, "Write live-output.txt");
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert!(calls.len() >= 2, "expected at least two tool calls");
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].tool_result.as_ref().unwrap().call_id, "tool-2");
    assert_eq!(results[0].tool_result.as_ref().unwrap().result.error, "");
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    assert_eq!(
        msgs.last()
            .and_then(|m| m.assistant_message.as_ref())
            .map(|m| m.text.as_str()),
        Some("TOOL_OK")
    );
    let done = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnCompleted(t) => Some(t.clone()),
            _ => None,
        })
        .expect("TurnCompleted");
    assert_eq!(done.usage.as_ref().unwrap().input_tokens, 7);
    assert_eq!(done.usage.as_ref().unwrap().output_tokens, 4);
}

#[tokio::test]
async fn permission_read_flow() {
    let mut sc = Scenario::new(adapter(), fixture_path("gemini", "permission_read.fixture"));
    sc.first_prompt = "Read the README".into();
    sc.drive_send = true;
    sc.model = String::new();
    sc.on_permission = Some(std::sync::Arc::new(|req| {
        assert_eq!(req.tool, "Read README.md");
        PermissionResponse::from_decision(Decision::Allow)
    }));
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let req = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::PermissionRequest(req) => Some(req.clone()),
            _ => None,
        })
        .expect("PermissionRequest");
    assert_eq!(req.tool, "Read README.md");
    let input = req.input.expect("projected tool input");
    assert_eq!(input["tool_call_id"], "tool-read-1");
    assert_eq!(input["kind"], "read");
    assert_eq!(input["title"], "Read README.md");
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    let call = calls[0].tool_call.as_ref().expect("ToolCall");
    assert_eq!(call.call.name.as_str(), "read");
    assert_eq!(
        call.call
            .input
            .get("locations")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.get("path"))
            .and_then(|v| v.as_str()),
        Some("README.md")
    );
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    let result = results[0].tool_result.as_ref().expect("ToolResult");
    assert_eq!(result.call_id, "tool-read-1");
    assert_eq!(result.result.output, "# README\nlucarne-live-read");
    assert_eq!(result.result.error, "");
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    assert_eq!(
        msgs.last()
            .and_then(|m| m.assistant_message.as_ref())
            .map(|m| m.text.as_str()),
        Some("READ_OK")
    );
    let done = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnCompleted(t) => Some(t.clone()),
            _ => None,
        })
        .expect("TurnCompleted");
    assert_eq!(done.usage.as_ref().unwrap().input_tokens, 6);
    assert_eq!(done.usage.as_ref().unwrap().output_tokens, 3);
    assert_eq!(done.usage.as_ref().unwrap().cache_read_tokens, 1);
}

#[tokio::test]
async fn permission_deny_flow() {
    let mut sc = Scenario::new(adapter(), fixture_path("gemini", "permission_deny.fixture"));
    sc.first_prompt = "Use tools, do not simulate. Use a shell or terminal command to run `sleep 30` in the current working directory. Do not recover or switch tools.".into();
    sc.drive_send = true;
    sc.model = String::new();
    sc.on_permission = Some(std::sync::Arc::new(|req| {
        assert_eq!(req.tool, "sleep 30");
        PermissionResponse::from_decision(Decision::Deny)
    }));
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let req = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::PermissionRequest(req) => Some(req.clone()),
            _ => None,
        })
        .expect("PermissionRequest");
    assert_eq!(req.tool, "sleep 30");
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    let transcript = msgs
        .iter()
        .filter_map(|m| m.assistant_message.as_ref().map(|m| m.text.as_str()))
        .collect::<String>();
    assert!(
        transcript.contains("canceled by the user"),
        "unexpected assistant reply after denial: {transcript}"
    );
    let done = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnCompleted(t) => Some(t.clone()),
            _ => None,
        })
        .expect("TurnCompleted");
    assert_eq!(done.usage.as_ref().unwrap().input_tokens, 24848);
    assert_eq!(done.usage.as_ref().unwrap().output_tokens, 52);
}

#[tokio::test]
async fn usage_delta_emitted() {
    let mut sc = Scenario::new(adapter(), fixture_path("gemini", "usage_delta.fixture"));
    sc.first_prompt = "measure usage".into();
    sc.drive_send = true;
    sc.model = String::new();
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed);
    let delta = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::UsageDelta(delta) => Some(delta.clone()),
            _ => None,
        })
        .expect("UsageDelta");
    assert_eq!(delta.delta.cost_usd, 0.25);
    let done = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnCompleted(t) => Some(t.clone()),
            _ => None,
        })
        .expect("TurnCompleted");
    assert_eq!(done.usage.as_ref().unwrap().input_tokens, 8);
    assert_eq!(done.usage.as_ref().unwrap().output_tokens, 6);
}

#[tokio::test]
async fn question_flow() {
    let mut sc = Scenario::new(adapter(), fixture_path("gemini", "question.fixture"));
    sc.first_prompt = "Use tools, do not simulate. Ask me one clarifying question using the provider's native question flow. After it is answered, reply with exactly QUESTION_OK.".into();
    sc.drive_send = true;
    sc.model = String::new();
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    assert!(
        !r.events
            .iter()
            .any(|e| matches!(e.payload, Payload::PermissionRequest(_))),
        "unexpected synthetic permission request: {:?}",
        r.events
    );
    let reasoning = collect_timelines(&r.events, TimelineType::Reasoning);
    assert!(!reasoning.is_empty(), "expected streamed reasoning chunks");
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    assert_eq!(msgs.len(), 1, "unexpected assistant messages: {:?}", msgs);
    assert_eq!(
        msgs[0].assistant_message.as_ref().unwrap().text,
        "Which `.pen` file would you like to open or work with?"
    );
    let done = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnCompleted(t) => Some(t.clone()),
            _ => None,
        })
        .expect("TurnCompleted");
    assert_eq!(done.usage.as_ref().unwrap().input_tokens, 12362);
    assert_eq!(done.usage.as_ref().unwrap().output_tokens, 14);
}

#[tokio::test]
async fn interrupt_cancelled_flow() {
    let mut sc = Scenario::new(adapter(), fixture_path("gemini", "interrupt.fixture"));
    sc.first_prompt = "Use tools, do not simulate. Use a shell or terminal command to run `sleep 30` in the current working directory. Do not recover or switch tools.".into();
    sc.drive_send = true;
    sc.model = String::new();
    sc.on_permission = Some(std::sync::Arc::new(|_req| {
        PermissionResponse::from_decision(Decision::Allow)
    }));
    sc.on_event = Some(std::sync::Arc::new(|sess, ev| {
        Box::pin(async move {
            if matches!(ev.payload, Payload::PermissionRequest(_)) {
                sess.interrupt().await.map_err(|err| err.to_string())?;
            }
            Ok(())
        })
    }));
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let fail = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnFailed(t) => Some(t.clone()),
            _ => None,
        })
        .expect("TurnFailed");
    assert_eq!(fail.error, "cancelled");
    assert_eq!(fail.code, "cancelled");
}

#[tokio::test]
async fn resume_uses_session_load() {
    let mut sc = Scenario::new(adapter(), fixture_path("gemini", "resume.fixture"));
    sc.first_prompt = "resume check".into();
    sc.drive_send = true;
    sc.model = String::new();
    let mut data = BTreeMap::new();
    data.insert(
        "session_id".into(),
        serde_json::Value::String("resume-123".into()),
    );
    sc.resume = Some(ResumeHandle { version: 1, data });
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let started = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::SessionStarted(s) => Some(s.clone()),
            _ => None,
        })
        .expect("SessionStarted");
    assert_eq!(started.session_id, "resume-123");
}
