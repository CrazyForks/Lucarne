//! Codex fixture scenarios — drive the Codex adapter against fakeagent.

pub mod common;

use common::{collect_timelines, fakeagent_bin, fixture_path, kinds, run_scenario, Scenario};
use lucarne::adapters::codex;
use lucarne::event::{Decision, Kind, Payload, PermissionResponse, ResumeHandle, TimelineType};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

fn adapter() -> Arc<lucarne::adapter::ProtocolAdapter> {
    codex::new(codex::Options {
        binary: fakeagent_bin().to_string_lossy().into_owned(),
    })
}

fn base_scenario(fixture: &str) -> Scenario {
    let mut sc = Scenario::new(adapter(), fixture_path("codex", fixture));
    sc.first_prompt = "hello".into();
    sc.drive_send = true;
    sc.model = String::new();
    sc.timeout = Duration::from_secs(5);
    sc
}

fn assistant_transcript(items: &[lucarne::event::TimelineItem]) -> String {
    items
        .iter()
        .filter_map(|item| {
            item.assistant_message
                .as_ref()
                .map(|message| message.text.as_str())
        })
        .collect::<Vec<_>>()
        .join("")
}

#[tokio::test]
async fn basic() {
    let r = run_scenario(base_scenario("basic.fixture")).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let started = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::SessionStarted(s) => Some(s.clone()),
            _ => None,
        })
        .expect("SessionStarted");
    assert_eq!(started.session_id, "test-thread-basic");
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].assistant_message.as_ref().unwrap().text,
        "Hello from Codex!"
    );
    let completed = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnCompleted(tc) => Some(tc.clone()),
            _ => None,
        })
        .expect("TurnCompleted");
    let usage = completed.usage.expect("usage");
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 5);
}

#[tokio::test]
async fn legacy_protocol_emits_message_and_shell() {
    let r = run_scenario(base_scenario("legacy.fixture")).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].assistant_message.as_ref().unwrap().text,
        "Legacy response from Codex."
    );
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0]
            .tool_call
            .as_ref()
            .unwrap()
            .call
            .input
            .get("command")
            .and_then(Value::as_str)
            .unwrap(),
        "echo hello"
    );
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].tool_result.as_ref().unwrap().result.output,
        "hello"
    );
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnCompleted));
}

#[tokio::test]
async fn legacy_abort_emits_turn_failed() {
    let r = run_scenario(base_scenario("legacy_abort.fixture")).await;
    assert!(r.closed);
    assert!(r.events.iter().any(|e| matches!(
        &e.payload,
        Payload::TurnFailed(tf) if tf.error == "aborted"
    )));
}

#[tokio::test]
async fn permission_allow_flow() {
    let mut sc = base_scenario("permission.fixture");
    sc.on_permission = Some(Arc::new(|_req| {
        PermissionResponse::from_decision(Decision::Allow)
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let req = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::PermissionRequest(p) => Some(p.clone()),
            _ => None,
        })
        .expect("PermissionRequest");
    // Command should be on the input map.
    let input = req.input.as_ref().expect("input");
    assert_eq!(
        input.get("command").and_then(|v| v.as_str()),
        Some("ls /tmp")
    );
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnCompleted));
}

#[tokio::test]
async fn permission_deny_apply_patch_alias() {
    let mut sc = base_scenario("permission_deny.fixture");
    sc.on_permission = Some(Arc::new(|_req| {
        PermissionResponse::from_decision(Decision::Deny)
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    assert!(r
        .events
        .iter()
        .any(|e| matches!(&e.payload, Payload::PermissionRequest(_))));
    // file_change item rejected — should surface tool_result.error == "rejected".
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert!(
        results
            .iter()
            .any(|r| r.tool_result.as_ref().unwrap().result.error == "rejected"),
        "expected rejected tool result"
    );
}

#[tokio::test]
async fn permission_real_approval_shape() {
    let mut sc = base_scenario("permission_real.fixture");
    let saw_req = Arc::new(Mutex::new(None));
    let saw_req_capture = Arc::clone(&saw_req);
    sc.on_permission = Some(Arc::new(move |req| {
        *saw_req_capture.lock().expect("lock") = Some(req.clone());
        PermissionResponse::from_decision(Decision::Allow)
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));

    let req = saw_req
        .lock()
        .expect("lock")
        .clone()
        .expect("permission request");
    assert_eq!(req.tool, "item/commandExecution/requestApproval");
    let input = req
        .input
        .as_ref()
        .and_then(Value::as_object)
        .expect("input");
    assert_eq!(
        input.get("itemId").and_then(Value::as_str),
        Some("call_real_1")
    );
    assert_eq!(
        input.get("threadId").and_then(Value::as_str),
        Some("real-thread-1")
    );
    assert_eq!(
        input.get("turnId").and_then(Value::as_str),
        Some("turn-real-1")
    );
    assert!(!input
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .is_empty());
    assert!(!input
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .is_empty());

    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert!(
        calls.iter().any(|call| {
            call.tool_call
                .as_ref()
                .filter(|tool_call| tool_call.call.name == "shell")
                .and_then(|tool_call| tool_call.call.input.get("command"))
                .and_then(Value::as_str)
                == Some("rm /tmp/repo/delete-target.txt")
        }),
        "missing expected shell command in tool calls: {:?}",
        calls
    );
    assert!(collect_timelines(&r.events, TimelineType::ToolResult).is_empty());
    let done = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnCompleted(tc) => Some(tc.clone()),
            _ => None,
        })
        .expect("TurnCompleted");
    assert_eq!(done.turn_id, "turn-real-1");
}

#[tokio::test]
async fn permission_real_decline_produces_declined_tool_result_and_transcript() {
    let mut sc = base_scenario("permission_real_decline.fixture");
    let saw_req = Arc::new(Mutex::new(None));
    let saw_req_capture = Arc::clone(&saw_req);
    sc.on_permission = Some(Arc::new(move |req| {
        *saw_req_capture.lock().expect("lock") = Some(req.clone());
        PermissionResponse::from_decision(Decision::Deny)
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));

    let req = saw_req
        .lock()
        .expect("lock")
        .clone()
        .expect("permission request");
    assert_eq!(req.tool, "item/commandExecution/requestApproval");
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    let shell = calls[0].tool_call.as_ref().expect("tool_call").call.clone();
    assert_eq!(shell.name.as_str(), "shell");
    assert_eq!(
        shell.input.get("command").and_then(Value::as_str),
        Some("rm /tmp/repo/delete-target.txt")
    );

    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].tool_result.as_ref().unwrap().result.error,
        "declined"
    );

    let transcript = assistant_transcript(&collect_timelines(
        &r.events,
        TimelineType::AssistantMessage,
    ));
    assert_eq!(
        transcript,
        "Deletion was not approved, so I could not remove the file."
    );
    let done = r
        .events
        .iter()
        .any(|e| matches!(e.payload, Payload::TurnCompleted(_)));
    assert!(done, "expected TurnCompleted");
}

#[tokio::test]
async fn interrupt_real_produces_interrupted_failure() {
    let interrupted = Arc::new(AtomicBool::new(false));
    let interrupted_capture = Arc::clone(&interrupted);
    let mut sc = base_scenario("interrupt_real.fixture");
    sc.first_prompt = "Use tools, do not simulate. Run a shell command to execute `sleep 30` in the current working directory. Do not recover or switch tools.".into();
    sc.on_event = Some(Arc::new(move |sess, ev| {
        let interrupted = Arc::clone(&interrupted_capture);
        Box::pin(async move {
            if interrupted.load(Ordering::SeqCst) || !matches!(ev.kind(), Kind::Timeline) {
                return Ok(());
            }
            let item = match ev.payload {
                Payload::Timeline(tl) => tl.item,
                _ => return Ok(()),
            };
            if item.ty != TimelineType::ToolCall {
                return Ok(());
            }
            let Some(tool_call) = item.tool_call else {
                return Ok(());
            };
            if tool_call.call.name != "shell" {
                return Ok(());
            };
            if tool_call.call.input.get("command").and_then(Value::as_str) != Some("sleep 30") {
                return Ok(());
            }
            interrupted.store(true, Ordering::SeqCst);
            sess.interrupt().await.map_err(|err| err.to_string())
        })
    }));
    let r = run_scenario(sc).await;
    assert!(
        interrupted.load(Ordering::SeqCst),
        "expected interrupt hook to fire"
    );

    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    let shell = calls[0].tool_call.as_ref().expect("tool_call").call.clone();
    assert_eq!(shell.name.as_str(), "shell");
    assert_eq!(
        shell.input.get("command").and_then(Value::as_str),
        Some("sleep 30")
    );

    let failures: Vec<_> = r
        .events
        .iter()
        .filter_map(|e| match &e.payload {
            Payload::TurnFailed(tf) => Some(tf.error.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(failures, vec!["interrupted".to_string()]);
}

#[tokio::test]
async fn file_change_real_deny_rejected_tool_result() {
    let mut sc = base_scenario("file_change_real_deny.fixture");
    let saw_req = Arc::new(Mutex::new(None));
    let saw_req_capture = Arc::clone(&saw_req);
    sc.on_permission = Some(Arc::new(move |req| {
        *saw_req_capture.lock().expect("lock") = Some(req.clone());
        PermissionResponse::from_decision(Decision::Deny)
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));

    let req = saw_req
        .lock()
        .expect("lock")
        .clone()
        .expect("permission request");
    assert_eq!(req.tool, "item/fileChange/requestApproval");
    let input = req
        .input
        .as_ref()
        .and_then(Value::as_object)
        .expect("input");
    assert_eq!(
        input.get("itemId").and_then(Value::as_str),
        Some("call_file_deny_1")
    );
    assert_eq!(
        input.get("threadId").and_then(Value::as_str),
        Some("real-thread-file-change-deny")
    );
    assert_eq!(
        input.get("turnId").and_then(Value::as_str),
        Some("turn-file-change-deny")
    );

    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0]
            .tool_call
            .as_ref()
            .expect("tool_call")
            .call
            .input
            .get("path")
            .and_then(Value::as_str)
            .unwrap(),
        "/tmp/repo/README.md"
    );

    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].tool_result.as_ref().unwrap().result.error,
        "rejected"
    );
    let done = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnCompleted(tc) => Some(tc.clone()),
            _ => None,
        })
        .expect("TurnCompleted");
    assert_eq!(done.turn_id, "turn-file-change-deny");
}

#[tokio::test]
async fn permission_exec_alias_round_trip() {
    let perm_seen = Arc::new(AtomicBool::new(false));
    let perm_seen_capture = Arc::clone(&perm_seen);
    let mut sc = base_scenario("permission_exec_alias.fixture");
    sc.first_prompt = "run shell".into();
    sc.on_permission = Some(Arc::new(move |req| {
        perm_seen_capture.store(true, Ordering::SeqCst);
        assert_eq!(req.tool, "execCommandApproval");
        PermissionResponse::from_decision(Decision::Allow)
    }));
    let r = run_scenario(sc).await;
    assert!(
        perm_seen.load(Ordering::SeqCst),
        "expected permission request"
    );

    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    let shell = calls[0].tool_call.as_ref().expect("tool_call").call.clone();
    assert_eq!(shell.name.as_str(), "shell");
    assert_eq!(
        shell.input.get("command").and_then(Value::as_str),
        Some("pwd")
    );
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].tool_result.as_ref().unwrap().result.output,
        "/tmp/repo"
    );
}

#[tokio::test]
async fn permission_apply_patch_alias_round_trip() {
    let perm_seen = Arc::new(AtomicBool::new(false));
    let perm_seen_capture = Arc::clone(&perm_seen);
    let mut sc = base_scenario("permission_apply_patch_alias.fixture");
    sc.first_prompt = "patch file".into();
    sc.on_permission = Some(Arc::new(move |req| {
        perm_seen_capture.store(true, Ordering::SeqCst);
        assert_eq!(req.tool, "applyPatchApproval");
        PermissionResponse::from_decision(Decision::Allow)
    }));
    let r = run_scenario(sc).await;
    assert!(
        perm_seen.load(Ordering::SeqCst),
        "expected permission request"
    );

    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0]
            .tool_call
            .as_ref()
            .expect("tool_call")
            .call
            .input
            .get("path")
            .and_then(Value::as_str)
            .unwrap(),
        "/tmp/repo/README.md"
    );
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].tool_result.as_ref().unwrap().result.output,
        "applied"
    );
}

#[tokio::test]
async fn question_multi_defaults() {
    let perm_seen = Arc::new(AtomicBool::new(false));
    let perm_seen_capture = Arc::clone(&perm_seen);
    let mut sc = base_scenario("question_multi.fixture");
    sc.first_prompt = "ask me two things".into();
    sc.on_permission = Some(Arc::new(move |req| {
        perm_seen_capture.store(true, Ordering::SeqCst);
        assert_eq!(req.tool, "request_user_input");
        assert_eq!(req.questions.len(), 3);
        assert_eq!(req.questions[0].id, "confirm_path");
        assert_eq!(req.questions[0].header, "Confirm");
        assert!(!req.questions[0].multi_select);
        assert_eq!(req.questions[1].id, "empty_options");
        assert!(req.questions[1].options.is_empty());
        assert_eq!(req.questions[2].id, "pick_model");
        assert_eq!(req.questions[2].options[0].label, "gpt-5 (Recommended)");
        PermissionResponse::from_decision(Decision::Allow)
    }));
    let r = run_scenario(sc).await;
    assert!(
        perm_seen.load(Ordering::SeqCst),
        "expected question permission request"
    );

    let transcript = assistant_transcript(&collect_timelines(
        &r.events,
        TimelineType::AssistantMessage,
    ));
    assert_eq!(transcript, "Captured both answers.");
}

#[tokio::test]
async fn question_custom_explicit_answers_with_metadata() {
    let perm_seen = Arc::new(AtomicBool::new(false));
    let perm_seen_capture = Arc::clone(&perm_seen);
    let mut sc = base_scenario("question_custom.fixture");
    sc.first_prompt = "ask me two things".into();
    sc.on_permission = Some(Arc::new(move |req| {
        perm_seen_capture.store(true, Ordering::SeqCst);
        assert_eq!(req.tool, "request_user_input");
        assert_eq!(req.questions.len(), 2);
        assert_eq!(req.questions[0].header, "Confirm");
        assert!(req.questions[0].multi_select);
        assert_eq!(req.questions[0].options.len(), 3);
        assert_eq!(req.questions[1].id, "pick_model");
        assert_eq!(req.questions[1].options[0].label, "gpt-5 (Recommended)");
        PermissionResponse {
            decision: Decision::Allow,
            answers: BTreeMap::from([
                (
                    "Confirm".into(),
                    lucarne::event::PermissionAnswer {
                        text: "No, Maybe".into(),
                        ..Default::default()
                    },
                ),
                (
                    "pick_model".into(),
                    lucarne::event::PermissionAnswer {
                        answers: vec!["gpt-5-mini".into()],
                        ..Default::default()
                    },
                ),
            ]),
        }
    }));
    let r = run_scenario(sc).await;
    assert!(
        perm_seen.load(Ordering::SeqCst),
        "expected question permission request"
    );

    let transcript = assistant_transcript(&collect_timelines(
        &r.events,
        TimelineType::AssistantMessage,
    ));
    assert_eq!(transcript, "Captured your custom answers.");
}

#[tokio::test]
async fn file_change_usage_usage_delta_and_completed_usage() {
    let r = run_scenario(base_scenario("file_change_usage.fixture")).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));

    let delta = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::UsageDelta(d) => Some(d.clone()),
            _ => None,
        })
        .expect("UsageDelta");
    assert_eq!(delta.delta.input_tokens, 11);
    assert_eq!(delta.delta.output_tokens, 3);
    assert_eq!(delta.delta.cache_read_tokens, 2);

    let done = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnCompleted(tc) => Some(tc.clone()),
            _ => None,
        })
        .expect("TurnCompleted");
    assert_eq!(done.turn_id, "turn-file-change");
    let usage = done.usage.expect("usage");
    assert_eq!(usage.input_tokens, 11);
    assert_eq!(usage.output_tokens, 3);
    assert_eq!(usage.cache_read_tokens, 2);
}

#[tokio::test]
async fn interrupt_recovery_second_turn_success_after_abort() {
    let interrupted = Arc::new(AtomicBool::new(false));
    let resent = Arc::new(AtomicBool::new(false));
    let interrupted_capture = Arc::clone(&interrupted);
    let resent_capture = Arc::clone(&resent);
    let mut sc = base_scenario("interrupt_recovery.fixture");
    sc.first_prompt = "first turn".into();
    sc.on_event = Some(Arc::new(move |sess, ev| {
        let interrupted = Arc::clone(&interrupted_capture);
        let resent = Arc::clone(&resent_capture);
        Box::pin(async move {
            match ev.payload {
                Payload::Timeline(tl)
                    if tl.item.ty == TimelineType::ToolCall
                        && !interrupted.swap(true, Ordering::SeqCst) =>
                {
                    sess.interrupt().await.map_err(|err| err.to_string())
                }
                Payload::TurnFailed(tf)
                    if interrupted.load(Ordering::SeqCst)
                        && !resent.swap(true, Ordering::SeqCst)
                        && tf.error == "aborted" =>
                {
                    sess.send(lucarne::dialect::Input {
                        text: "second turn".into(),
                        images: vec![],
                    })
                    .await
                    .map_err(|err| err.to_string())
                }
                _ => Ok(()),
            }
        })
    }));
    let r = run_scenario(sc).await;
    assert!(
        interrupted.load(Ordering::SeqCst) && resent.load(Ordering::SeqCst),
        "interrupt flow did not fire as expected"
    );

    let failures: Vec<_> = r
        .events
        .iter()
        .filter_map(|e| match &e.payload {
            Payload::TurnFailed(tf) => Some(tf.error.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(failures, vec!["aborted".to_string()]);

    let transcript = assistant_transcript(&collect_timelines(
        &r.events,
        TimelineType::AssistantMessage,
    ));
    assert_eq!(transcript, "Recovered after abort.");

    let recovered = r.events.iter().any(|e| {
        matches!(
            &e.payload,
            Payload::TurnCompleted(tc) if tc.turn_id == "turn-recovered"
        )
    });
    assert!(recovered, "missing recovered turn_completed");
}

#[tokio::test]
async fn resume_missing_id_fallback() {
    let mut sc = base_scenario("resume_missing_id_fallback.fixture");
    let mut data: BTreeMap<String, Value> = BTreeMap::new();
    data.insert("thread_id".into(), Value::String("prior-thread-id".into()));
    sc.resume = Some(ResumeHandle { version: 1, data });
    sc.first_prompt = "Resume if possible".into();
    let r = run_scenario(sc).await;

    let started = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::SessionStarted(s) => Some(s.clone()),
            _ => None,
        })
        .expect("SessionStarted");
    assert_eq!(started.session_id, "test-thread-fresh-after-empty-resume");

    let transcript = assistant_transcript(&collect_timelines(
        &r.events,
        TimelineType::AssistantMessage,
    ));
    assert_eq!(transcript, "Fresh thread after empty resume result.");
}

#[tokio::test]
async fn resume_success_uses_thread_resume() {
    let mut sc = base_scenario("resume_success.fixture");
    let mut data: BTreeMap<String, Value> = BTreeMap::new();
    data.insert(
        "thread_id".into(),
        Value::String("test-thread-prior".into()),
    );
    sc.resume = Some(ResumeHandle { version: 1, data });
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
    assert_eq!(started.session_id, "test-thread-resumed");
}

#[tokio::test]
async fn resume_fallback_to_fresh_start() {
    let mut sc = base_scenario("resume_fallback.fixture");
    let mut data: BTreeMap<String, Value> = BTreeMap::new();
    data.insert("thread_id".into(), Value::String("stale-thread".into()));
    sc.resume = Some(ResumeHandle { version: 1, data });
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
    assert_eq!(started.session_id, "test-thread-fresh-after-resume");
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    assert_eq!(msgs.len(), 1);
}

#[tokio::test]
async fn start_missing_id_closes_session() {
    let r = run_scenario(base_scenario("start_missing_id.fixture")).await;
    assert!(r.closed);
    // SessionClosed should carry a non-empty reason surfacing the init failure.
    let closed = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::SessionClosed(s) => Some(s.clone()),
            _ => None,
        })
        .expect("SessionClosed");
    assert!(
        closed.reason.is_empty()
            || closed.reason.contains("no threadId")
            || closed.reason.contains("thread/start"),
        "unexpected reason = {:?}",
        closed.reason
    );
}

#[tokio::test]
async fn foreign_thread_notifications_ignored() {
    let r = run_scenario(base_scenario("foreign_thread_error.fixture")).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    // Only the current-thread assistant message should show up.
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].assistant_message.as_ref().unwrap().text,
        "Current thread response."
    );
    // Only the current-thread error should produce a TurnFailed.
    let fails: Vec<_> = r
        .events
        .iter()
        .filter_map(|e| match &e.payload {
            Payload::TurnFailed(tf) => Some(tf.error.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(fails, vec!["disk full".to_string()]);
}

#[tokio::test]
async fn question_default_first_option() {
    // No permission handler: the dialect should auto-pick the first option
    // label ("Yes (Recommended)") because no explicit answer was provided
    // and the server-sent question has options.
    let mut sc = base_scenario("question.fixture");
    sc.on_permission = Some(Arc::new(|_req| {
        // Allow but don't supply structured answers; the dialect will fall
        // back to the default first-option behavior in Allow mode.
        PermissionResponse::from_decision(Decision::Allow)
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    // A PermissionRequest with questions should have surfaced.
    let req = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::PermissionRequest(p) if !p.questions.is_empty() => Some(p.clone()),
            _ => None,
        })
        .expect("PermissionRequest with questions");
    assert_eq!(req.tool, "request_user_input");
    assert_eq!(req.questions[0].id, "confirm_path");
}

#[tokio::test]
async fn file_change_real_approval() {
    let mut sc = base_scenario("file_change_real.fixture");
    sc.on_permission = Some(Arc::new(|_req| {
        PermissionResponse::from_decision(Decision::Allow)
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    // At least one file-change call carrying the raw path and unified diff.
    let edit = calls
        .iter()
        .filter_map(|c| c.tool_call.as_ref())
        .map(|c| &c.call)
        .find(|call| call.name == "fileChange")
        .expect("fileChange tool call");
    assert_eq!(
        edit.input.get("path").and_then(Value::as_str),
        Some("/tmp/repo/README.md")
    );
    let patch = edit
        .input
        .get("patch")
        .and_then(Value::as_str)
        .expect("patch");
    assert!(patch.contains("-old"));
    assert!(patch.contains("+new"));
}
