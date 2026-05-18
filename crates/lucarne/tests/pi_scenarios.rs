//! Pi RPC fixture scenarios — drive the Pi RPC adapter against fakeagent.

pub mod common;

use common::{collect_timelines, fakeagent_bin, fixture_path, kinds, run_scenario, Scenario};
use lucarne::adapters::pi;
use lucarne::event::{
    CommandResultData, Decision, Kind, Payload, PermissionAnswer, PermissionResponse, ResumeHandle,
    TimelineType,
};
use lucarne::AgentCommandSource;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

fn adapter() -> std::sync::Arc<lucarne::adapter::ProtocolAdapter> {
    pi::new(pi::Options {
        binary: fakeagent_bin().to_string_lossy().into_owned(),
    })
}

fn base_scenario(fixture: &str) -> Scenario {
    let mut sc = Scenario::new(adapter(), fixture_path("pi", fixture));
    sc.first_prompt = "hi".into();
    sc.model = "xai/grok-4".into();
    sc.drive_send = true;
    sc.timeout = Duration::from_secs(10);
    sc
}

#[tokio::test]
async fn basic() {
    let r = run_scenario(base_scenario("basic.fixture")).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    // SessionStarted should carry the session file path from get_state response
    let started = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::SessionStarted(s) => Some(s.clone()),
            _ => None,
        })
        .expect("SessionStarted");
    assert!(
        !started.session_id.is_empty(),
        "session_id should be session file path"
    );
    // Reasoning delta
    let reasoning = collect_timelines(&r.events, TimelineType::Reasoning);
    assert_eq!(reasoning.len(), 1);
    assert_eq!(reasoning[0].reasoning.as_ref().unwrap().text, "Inspecting");
    // Assistant text deltas + turn_end concatenation
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].assistant_message.as_ref().unwrap().text, "Hello ");
    assert_eq!(
        msgs[1].assistant_message.as_ref().unwrap().text,
        "Hello from Pi"
    );
    // Tool call
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    let read = &calls[0].tool_call.as_ref().unwrap().call;
    assert_eq!(read.name.as_str(), "read");
    assert_eq!(
        read.input.get("path").and_then(Value::as_str),
        Some("/tmp/test.txt")
    );
    // Tool result
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].tool_result.as_ref().unwrap().result.output,
        "file contents"
    );
    // Usage in TurnCompleted
    let done = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnCompleted(t) => Some(t.clone()),
            _ => None,
        })
        .expect("TurnCompleted");
    let usage = done.usage.as_ref().expect("usage");
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 5);
    assert_eq!(usage.cache_read_tokens, 2);
    assert_eq!(usage.cost_usd, 0.001);
}

#[tokio::test]
async fn turn_end_tool_results() {
    let r = run_scenario(base_scenario("tool_results.fixture")).await;
    assert!(r.closed);
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 2);
    assert_eq!(
        results[0].tool_result.as_ref().unwrap().result.output,
        "file contents"
    );
    assert_eq!(results[0].tool_result.as_ref().unwrap().call_id, "tool_1");
    assert_eq!(
        results[1].tool_result.as_ref().unwrap().result.error,
        "File not found"
    );
    assert_eq!(results[1].tool_result.as_ref().unwrap().call_id, "tool_2");
}

#[tokio::test]
async fn error_emits_turn_failed() {
    let r = run_scenario(base_scenario("error.fixture")).await;
    assert!(r.closed);
    let fail = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnFailed(t) => Some(t.clone()),
            _ => None,
        })
        .expect("TurnFailed");
    assert_eq!(fail.error, "Connection to model provider lost");
}

#[tokio::test]
async fn retry_exhausted_emits_turn_failed() {
    let r = run_scenario(base_scenario("retry_exhausted.fixture")).await;
    assert!(r.closed);
    let fail = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnFailed(t) => Some(t.clone()),
            _ => None,
        })
        .expect("TurnFailed");
    assert!(fail.error.contains("RESOURCE_EXHAUSTED"));
}

#[tokio::test]
async fn auto_retry_success_does_not_fail() {
    let r = run_scenario(base_scenario("auto_retry_success.fixture")).await;
    assert!(r.closed);
    // No TurnFailed; auto_retry_end with success=true is a no-op
    let has_fail = r
        .events
        .iter()
        .any(|e| matches!(&e.payload, Payload::TurnFailed(_)));
    assert!(
        !has_fail,
        "auto_retry_end success should not emit TurnFailed"
    );
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnCompleted));
}

#[tokio::test]
async fn multi_turn_flow() {
    let sc = base_scenario("multi_turn.fixture");
    let r = run_scenario(sc).await;
    // Two turn completions expected from the fixture
    let turns: Vec<_> = r
        .events
        .iter()
        .filter(|e| e.kind() == Kind::TurnCompleted)
        .collect();
    assert_eq!(
        turns.len(),
        2,
        "expected 2 turn completions, got {:?}",
        kinds(&r.events)
    );
}

#[tokio::test]
async fn resume_propagated_via_session_closed() {
    use serde_json::Value;

    // Build a resume handle manually and pass it in
    let mut data = BTreeMap::new();
    data.insert(
        "session_path".into(),
        Value::String("/tmp/pi-resume-session.jsonl".into()),
    );
    let resume = Some(ResumeHandle { version: 1, data });

    let mut sc = base_scenario("resume.fixture");
    sc.resume = resume;
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));

    // The SessionClosed should carry the session_file as resume token
    let closed = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::SessionClosed(sc) => Some(sc.clone()),
            _ => None,
        })
        .expect("SessionClosed");
    let rh = closed.resume.as_ref().expect("resume handle");
    assert_eq!(
        rh.data.get("session_path").and_then(|v| v.as_str()),
        Some("/tmp/pi-resume-session.jsonl")
    );
}

#[tokio::test]
async fn tool_flow() {
    let r = run_scenario(base_scenario("tool_flow.fixture")).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));

    // tool_execution_start emits ToolCall timeline
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    let call = &calls[0].tool_call.as_ref().unwrap().call;
    assert_eq!(call.name.as_str(), "bash");
    assert_eq!(
        call.input.get("command").and_then(Value::as_str),
        Some("sleep 1\necho part1\necho part2")
    );

    // tool_execution_update emits partial ToolResult timelines
    // tool_execution_end emits final ToolResult timeline
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 3, "expected 2 partial updates + 1 final");

    // First two results are partial
    assert!(results[0].tool_result.as_ref().unwrap().result.partial);
    assert_eq!(
        results[0].tool_result.as_ref().unwrap().result.output,
        "partial output line 1\n"
    );
    assert!(results[1].tool_result.as_ref().unwrap().result.partial);
    assert_eq!(
        results[1].tool_result.as_ref().unwrap().result.output,
        "partial output line 2\n"
    );

    // Final result is not partial
    assert!(!results[2].tool_result.as_ref().unwrap().result.partial);
    assert_eq!(
        results[2].tool_result.as_ref().unwrap().result.output,
        "partial output line 1\npartial output line 2\n"
    );
}

#[tokio::test]
async fn permission_select_flow() {
    use lucarne::event::PermissionRequest;

    let mut sc = base_scenario("permission_select.fixture");
    sc.on_permission = Some(Arc::new(|_req: &PermissionRequest| {
        let mut answers = BTreeMap::new();
        answers.insert(
            "q".into(),
            PermissionAnswer {
                answers: vec!["Option A".into()],
                ..Default::default()
            },
        );
        PermissionResponse {
            decision: Decision::Allow,
            answers,
        }
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));

    // Should have emitted a PermissionRequest for the select
    let perm = r.events.iter().find_map(|e| match &e.payload {
        Payload::PermissionRequest(pr) => Some(pr.clone()),
        _ => None,
    });
    assert!(perm.is_some(), "expected PermissionRequest event");
    let perm = perm.unwrap();
    assert_eq!(perm.req_id, "ext-1");
    // Should have at least one question with options
    assert!(!perm.questions.is_empty());
    assert!(perm.questions[0].question.contains("Pick an option"));
    assert_eq!(perm.questions[0].options.len(), 2);
}

#[tokio::test]
async fn interrupt_flow() {
    let mut sc = base_scenario("interrupt.fixture");
    sc.on_event = Some(Arc::new(|sess, ev| {
        Box::pin(async move {
            if ev.kind() == Kind::TurnStarted {
                sess.interrupt().await.map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
}

#[tokio::test]
async fn model_catalog_via_rpc() {
    use lucarne::AgentCommandInvocation;

    let mut sc = base_scenario("model_catalog.fixture");
    sc.on_event = Some(Arc::new(|sess, ev| {
        Box::pin(async move {
            if ev.kind() == Kind::SessionStarted {
                sess.invoke_command(AgentCommandInvocation {
                    name: "list_models".into(),
                    args: None,
                    values: serde_json::Value::Null,
                    source: AgentCommandSource::AdapterMapped,
                })
                .await
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
}

fn system_command_values(extra: Value) -> Value {
    let obj = match extra {
        Value::Object(obj) => obj,
        _ => serde_json::Map::new(),
    };
    Value::Object(obj)
}

fn command_result_payloads(r: &common::ScenarioResult) -> Vec<CommandResultData> {
    r.events
        .iter()
        .filter_map(|e| match &e.payload {
            Payload::CommandResult(result) => Some(result.result.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn fork_flow() {
    use lucarne::AgentCommandInvocation;
    use std::sync::atomic::{AtomicBool, Ordering};

    std::fs::write(
        "/tmp/pi-forked-existing.jsonl",
        r#"{"type":"session","version":3,"id":"forked-1","timestamp":"2026-05-12T00:00:00.000Z","cwd":"/tmp"}"#,
    )
    .expect("write persisted Pi fork fixture");
    let mut sc = base_scenario("fork_flow.fixture");
    let invoked = Arc::new(AtomicBool::new(false));
    sc.on_event = Some(Arc::new(move |sess, ev| {
        let invoked = Arc::clone(&invoked);
        Box::pin(async move {
            if ev.kind() == Kind::SessionStarted && !invoked.swap(true, Ordering::SeqCst) {
                sess.invoke_command(AgentCommandInvocation {
                    name: "fork".into(),
                    args: Some("entry-1".into()),
                    values: system_command_values(json!({ "target_id": "entry-1" })),
                    source: AgentCommandSource::AdapterMapped,
                })
                .await
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let fork = command_result_payloads(&r)
        .into_iter()
        .find_map(|data| match data {
            CommandResultData::Forked(result) => Some(result),
            _ => None,
        })
        .expect("fork result");
    assert_eq!(
        fork.session_ref.as_ref().map(|r| r.0.as_str()),
        Some("forked-1")
    );
    assert_eq!(
        fork.source_session_ref.as_ref().map(|r| r.0.as_str()),
        Some("s1")
    );
}

#[tokio::test]
async fn fork_unpersisted_flow_is_live_only_until_pi_writes_session_file() {
    use lucarne::AgentCommandInvocation;
    use std::sync::atomic::{AtomicBool, Ordering};

    let mut sc = base_scenario("fork_unpersisted_flow.fixture");
    let invoked = Arc::new(AtomicBool::new(false));
    sc.on_event = Some(Arc::new(move |sess, ev| {
        let invoked = Arc::clone(&invoked);
        Box::pin(async move {
            if ev.kind() == Kind::SessionStarted && !invoked.swap(true, Ordering::SeqCst) {
                sess.invoke_command(AgentCommandInvocation {
                    name: "fork".into(),
                    args: Some("entry-1".into()),
                    values: system_command_values(json!({ "target_id": "entry-1" })),
                    source: AgentCommandSource::AdapterMapped,
                })
                .await
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let started: Vec<_> = r
        .events
        .iter()
        .filter_map(|e| match &e.payload {
            Payload::SessionStarted(started) => Some(started.session_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(started, vec!["s1"]);
    let fork = command_result_payloads(&r)
        .into_iter()
        .find_map(|data| match data {
            CommandResultData::Forked(result) => Some(result),
            _ => None,
        })
        .expect("fork result");
    assert!(
        fork.session_ref.is_none(),
        "unpersisted Pi forks must not be advertised as resumable"
    );
    assert_eq!(
        fork.source_session_ref.as_ref().map(|r| r.0.as_str()),
        Some("s1")
    );
}

#[tokio::test]
async fn new_session_flow() {
    use lucarne::AgentCommandInvocation;
    use std::sync::atomic::{AtomicBool, Ordering};

    let mut sc = base_scenario("new_session_flow.fixture");
    let invoked = Arc::new(AtomicBool::new(false));
    sc.on_event = Some(Arc::new(move |sess, ev| {
        let invoked = Arc::clone(&invoked);
        Box::pin(async move {
            if ev.kind() == Kind::SessionStarted && !invoked.swap(true, Ordering::SeqCst) {
                sess.invoke_command(AgentCommandInvocation {
                    name: "new".into(),
                    args: None,
                    values: system_command_values(Value::Null),
                    source: AgentCommandSource::AdapterMapped,
                })
                .await
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let messages: Vec<_> = collect_timelines(&r.events, TimelineType::AssistantMessage)
        .into_iter()
        .filter_map(|item| item.assistant_message.map(|msg| msg.text))
        .collect();
    assert!(
        messages
            .iter()
            .any(|text| text.contains("Started new thread")),
        "messages = {messages:?}"
    );
}

#[tokio::test]
async fn status_flow() {
    use lucarne::AgentCommandInvocation;

    let mut sc = base_scenario("status_flow.fixture");
    sc.on_event = Some(Arc::new(|sess, ev| {
        Box::pin(async move {
            if ev.kind() == Kind::SessionStarted {
                sess.invoke_command(AgentCommandInvocation {
                    name: "status".into(),
                    args: None,
                    values: system_command_values(Value::Null),
                    source: AgentCommandSource::AdapterMapped,
                })
                .await
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let status = command_result_payloads(&r)
        .into_iter()
        .find_map(|data| match data {
            CommandResultData::Status(status) => Some(status),
            _ => None,
        })
        .expect("status result");
    assert_eq!(status.session_id.as_deref(), Some("/tmp/pi-status.jsonl"));
    assert_eq!(status.tokens.as_ref().and_then(|t| t.total_tokens), Some(9));
}

#[tokio::test]
async fn set_model_flow() {
    use lucarne::AgentCommandInvocation;

    let mut sc = base_scenario("set_model_flow.fixture");
    sc.on_event = Some(Arc::new(|sess, ev| {
        Box::pin(async move {
            if ev.kind() == Kind::SessionStarted {
                sess.invoke_command(AgentCommandInvocation {
                    name: "model".into(),
                    args: Some("openai/gpt-5 high".into()),
                    values: system_command_values(
                        json!({ "model": "openai/gpt-5", "reasoning": "high" }),
                    ),
                    source: AgentCommandSource::AdapterMapped,
                })
                .await
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let changed = command_result_payloads(&r)
        .into_iter()
        .find_map(|data| match data {
            CommandResultData::ModelChanged(selection) => Some(selection),
            _ => None,
        })
        .expect("model changed result");
    assert_eq!(changed.model.as_str(), "openai/gpt-5");
    assert_eq!(changed.reasoning.as_deref(), Some("high"));
}

#[tokio::test]
async fn confirm_deny_flow() {
    use lucarne::event::PermissionRequest;

    let mut sc = base_scenario("confirm_deny_flow.fixture");
    sc.on_permission = Some(Arc::new(|_req: &PermissionRequest| {
        PermissionResponse::from_decision(Decision::Deny)
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let perm = r.events.iter().find_map(|e| match &e.payload {
        Payload::PermissionRequest(pr) => Some(pr.clone()),
        _ => None,
    });
    assert!(perm.is_some(), "expected PermissionRequest event");
}

#[tokio::test]
async fn get_commands_flow() {
    use lucarne::AgentCommandInvocation;

    let mut sc = base_scenario("get_commands_flow.fixture");
    sc.on_event = Some(Arc::new(|sess, ev| {
        Box::pin(async move {
            if ev.kind() == Kind::SessionStarted {
                sess.invoke_command(AgentCommandInvocation {
                    name: "list_commands".into(),
                    args: None,
                    values: system_command_values(Value::Null),
                    source: AgentCommandSource::AdapterMapped,
                })
                .await
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let commands = command_result_payloads(&r)
        .into_iter()
        .find_map(|data| match data {
            CommandResultData::Commands(catalog) => Some(catalog),
            _ => None,
        })
        .expect("commands result");
    let names: Vec<_> = commands
        .commands
        .iter()
        .map(|cmd| cmd.name.as_str())
        .collect();
    assert_eq!(names, vec!["status"]);
}
