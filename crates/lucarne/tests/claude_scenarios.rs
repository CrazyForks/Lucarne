//! Claude fixture scenarios — drive the Claude adapter against fakeagent.

pub mod common;

use common::{
    collect_timelines, fakeagent_bin, fixture_path, kinds, run_scenario, EventHandler, Scenario,
};
use lucarne::adapters::claude;
use lucarne::event::{Decision, Kind, Payload, PermissionResponse, TimelineType};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

fn adapter() -> std::sync::Arc<lucarne::adapter::ProtocolAdapter> {
    claude::new(claude::Options {
        binary: fakeagent_bin().to_string_lossy().into_owned(),
    })
}

fn assistant_transcript(msgs: &[lucarne::event::TimelineItem]) -> String {
    let mut out = String::new();
    for msg in msgs {
        if let Some(assistant_message) = &msg.assistant_message {
            out.push_str(&assistant_message.text);
        }
    }
    out
}

fn argv_recording_claude_wrapper(log_path: &std::path::Path) -> std::path::PathBuf {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("claude-wrapper.sh");
    let fakeagent = fakeagent_bin();
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\nprintf '%s\\n' 'claude 2.1.119'\nexit 0\nfi\n: > {}\nfor arg in \"$@\"; do\nprintf '%s\\n' \"$arg\" >> {}\ndone\nexec {} \"$@\"\n",
        shell_quote(&log_path.to_string_lossy()),
        shell_quote(&log_path.to_string_lossy()),
        shell_quote(&fakeagent.to_string_lossy()),
    );
    fs::write(&path, script).expect("write claude wrapper");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod claude wrapper");
    std::mem::forget(dir);
    path
}

fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

fn close_on_turn_end() -> EventHandler {
    Arc::new(|sess, ev| {
        Box::pin(async move {
            if matches!(
                ev.payload,
                Payload::TurnCompleted(_) | Payload::TurnFailed(_)
            ) {
                sess.close().await;
            }
            Ok(())
        })
    })
}

#[tokio::test]
async fn basic() {
    let mut sc = Scenario::new(adapter(), fixture_path("claude", "basic.fixture"));
    sc.first_prompt = "Hello Claude".into();
    sc.drive_send = true;
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
    assert_eq!(started.session_id, "sess-basic");
    assert_eq!(started.model, "claude-sonnet-4");
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        msgs[0].assistant_message.as_ref().unwrap().text,
        "Hi! How can I help?"
    );
    let completed = r.events.iter().any(|e| e.kind() == Kind::TurnCompleted);
    assert!(completed);
}

#[tokio::test]
async fn launches_claude_as_long_lived_stream_json_without_print() {
    let dir = tempfile::tempdir().expect("tempdir");
    let argv_log = dir.path().join("argv.log");
    let wrapper = argv_recording_claude_wrapper(&argv_log);
    let adapter = claude::new(claude::Options {
        binary: wrapper.to_string_lossy().into_owned(),
    });
    let mut sc = Scenario::new(adapter, fixture_path("claude", "basic.fixture"));
    sc.first_prompt = "Hello Claude".into();
    sc.drive_send = true;
    sc.timeout = Duration::from_secs(5);

    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let argv = fs::read_to_string(&argv_log).expect("read argv log");
    let args = argv.lines().collect::<Vec<_>>();

    assert!(
        !args.contains(&"--print"),
        "Claude adapter must not use the old --print transport; argv={args:?}"
    );
    assert_eq!(
        args,
        vec![
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--replay-user-messages",
            "--model",
            "test-model",
        ]
    );
}

#[tokio::test]
async fn tool_use_flow() {
    let mut sc = Scenario::new(adapter(), fixture_path("claude", "tool_use.fixture"));
    sc.first_prompt = "list files".into();
    sc.drive_send = true;
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed);
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    let shell = calls[0].tool_call.as_ref().unwrap().call.clone();
    assert_eq!(shell.name.as_str(), "Bash");
    assert_eq!(
        shell.input.get("command").and_then(Value::as_str),
        Some("ls")
    );
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    assert!(results[0]
        .tool_result
        .as_ref()
        .unwrap()
        .result
        .output
        .contains("README.md"));
}

#[tokio::test]
async fn subagent_task_tool_call_preserves_child_identity_metadata() {
    let mut sc = Scenario::new(adapter(), fixture_path("claude", "subagent_task.fixture"));
    sc.first_prompt = "delegate investigation".into();
    sc.drive_send = true;
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));

    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    let subagent = calls[0].tool_call.as_ref().expect("tool call").call.clone();

    assert_eq!(subagent.name.as_str(), "Task");
    assert_eq!(
        subagent.input.get("prompt").and_then(Value::as_str),
        Some("Inspect the parser")
    );
    assert_eq!(
        subagent.input.get("model").and_then(Value::as_str),
        Some("opus")
    );
    assert_eq!(
        subagent.input.get("thread_id").and_then(Value::as_str),
        Some("child-thread-1")
    );
    assert_eq!(
        subagent.input.get("session_ref").and_then(Value::as_str),
        Some("child-session-1")
    );
    assert_eq!(
        subagent.input.get("agent_id").and_then(Value::as_str),
        Some("agent-1")
    );
    assert_eq!(
        subagent.input.get("nickname").and_then(Value::as_str),
        Some("Parser")
    );
    assert_eq!(
        subagent.input.get("role").and_then(Value::as_str),
        Some("explorer")
    );
    assert_eq!(
        subagent.input.get("status").and_then(Value::as_str),
        Some("running")
    );
    assert_eq!(
        subagent.input.get("message").and_then(Value::as_str),
        Some("child started")
    );
    assert_eq!(subagent.input["subagent_type"], "explorer");
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnCompleted));
}

#[tokio::test]
async fn thinking_flow() {
    let mut sc = Scenario::new(adapter(), fixture_path("claude", "thinking.fixture"));
    sc.first_prompt = "explain".into();
    sc.drive_send = true;
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed);
    let reasoning = collect_timelines(&r.events, TimelineType::Reasoning);
    assert_eq!(reasoning.len(), 1);
    assert_eq!(
        reasoning[0].reasoning.as_ref().unwrap().text,
        "Let me consider the options."
    );
}

#[tokio::test]
async fn error_fixture_emits_turn_failed() {
    let mut sc = Scenario::new(adapter(), fixture_path("claude", "error.fixture"));
    sc.first_prompt = "please fail".into();
    sc.drive_send = true;
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnFailed));
}

#[tokio::test]
async fn max_turns_emits_turn_failed() {
    let mut sc = Scenario::new(adapter(), fixture_path("claude", "max_turns.fixture"));
    sc.first_prompt = "keep going".into();
    sc.drive_send = true;
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnFailed));
}

#[tokio::test]
async fn permission_allow_flow() {
    let mut sc = Scenario::new(
        adapter(),
        fixture_path("claude", "permission_allow.fixture"),
    );
    sc.first_prompt = "delete logs".into();
    sc.drive_send = true;
    sc.on_permission = Some(std::sync::Arc::new(|_req| {
        PermissionResponse::from_decision(Decision::Allow)
    }));
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    assert!(r.events.iter().any(|e| e.kind() == Kind::PermissionRequest));
}

#[tokio::test]
async fn permission_deny_flow() {
    let mut sc = Scenario::new(adapter(), fixture_path("claude", "permission_deny.fixture"));
    sc.first_prompt = "delete files".into();
    sc.drive_send = true;
    sc.on_permission = Some(std::sync::Arc::new(|_req| {
        PermissionResponse::from_decision(Decision::Deny)
    }));
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    assert!(r.events.iter().any(|e| e.kind() == Kind::PermissionRequest));
}

#[tokio::test]
async fn permission_conversational_fallback_flow() {
    let mut sc = Scenario::new(
        adapter(),
        fixture_path("claude", "permission_conversational.fixture"),
    );
    sc.first_prompt = "write the file".into();
    sc.drive_send = true;
    sc.on_event = Some(close_on_turn_end());
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;

    assert!(!r.events.iter().any(|e| e.kind() == Kind::PermissionRequest));

    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].tool_call.as_ref().unwrap().call.name.as_str(),
        "Write"
    );
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    assert!(results[0]
        .tool_result
        .as_ref()
        .unwrap()
        .result
        .error
        .contains("haven't granted"));

    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    let transcript = assistant_transcript(&msgs);
    assert!(transcript.contains("I need permission to write ./danger.txt."));
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnCompleted));
}

#[tokio::test]
async fn permission_conversational_real_stream_json_shape() {
    let mut sc = Scenario::new(
        adapter(),
        fixture_path("claude", "permission_conversational_real.fixture"),
    );
    sc.first_prompt = "write the file".into();
    sc.drive_send = true;
    sc.on_event = Some(close_on_turn_end());
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;

    assert!(!r.events.iter().any(|e| e.kind() == Kind::PermissionRequest));
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].tool_call.as_ref().unwrap().call.name.as_str(),
        "Write"
    );
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    assert!(results[0]
        .tool_result
        .as_ref()
        .unwrap()
        .result
        .error
        .contains("haven't granted"));
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    let transcript = assistant_transcript(&msgs);
    assert!(transcript.contains("Permission approval is required"));
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnCompleted));
}

#[tokio::test]
async fn permission_conversational_real_reapproval_loop() {
    let mut sc = Scenario::new(
        adapter(),
        fixture_path(
            "claude",
            "permission_conversational_real_reapproval.fixture",
        ),
    );
    sc.first_prompt = "write the file".into();
    sc.drive_send = true;
    sc.on_event = Some(close_on_turn_end());
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;

    assert!(!r.events.iter().any(|e| e.kind() == Kind::PermissionRequest));
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].tool_call.as_ref().unwrap().call.name.as_str(),
        "Write"
    );
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    assert!(results[0]
        .tool_result
        .as_ref()
        .unwrap()
        .result
        .error
        .contains("haven't granted"));
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    let transcript = assistant_transcript(&msgs);
    assert!(transcript.contains("I need permission to write the file."));
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnCompleted));
}

#[tokio::test]
async fn permission_conversational_reapproval_loop() {
    let mut sc = Scenario::new(
        adapter(),
        fixture_path("claude", "permission_conversational_reapproval.fixture"),
    );
    sc.first_prompt = "write the file".into();
    sc.drive_send = true;
    sc.on_event = Some(close_on_turn_end());
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;

    assert!(!r.events.iter().any(|e| e.kind() == Kind::PermissionRequest));
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].tool_call.as_ref().unwrap().call.name.as_str(),
        "Write"
    );
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    let transcript = assistant_transcript(&msgs);
    assert!(transcript.contains("I need your permission to write the file."));
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnCompleted));
}

#[tokio::test]
async fn ask_user_question_real_stream_json_shape() {
    let answers = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let answers_capture = std::sync::Arc::clone(&answers);
    let mut sc = Scenario::new(
        adapter(),
        fixture_path("claude", "question_conversational_real.fixture"),
    );
    sc.first_prompt = "pick a theme".into();
    sc.drive_send = true;
    sc.on_permission = Some(std::sync::Arc::new(move |req| {
        answers_capture.lock().unwrap().push(req.clone());
        let mut answers = std::collections::BTreeMap::new();
        answers.insert(
            "layout_style".into(),
            lucarne::event::PermissionAnswer {
                answers: vec!["大字错落交替排版".into()],
                ..Default::default()
            },
        );
        PermissionResponse {
            decision: Decision::Allow,
            answers,
        }
    }));
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;

    let req = answers
        .lock()
        .unwrap()
        .first()
        .cloned()
        .expect("PermissionRequest");
    assert_eq!(req.tool, "AskUserQuestion");
    assert_eq!(req.questions.len(), 1);
    assert_eq!(req.questions[0].header, "布局风格");
    assert!(!req.questions[0].question.is_empty());
    assert_eq!(req.questions[0].options.len(), 3);
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    let call = calls[0].tool_call.as_ref().unwrap().call.clone();
    assert_eq!(call.name.as_str(), "AskUserQuestion");
    let raw = &call.input;
    assert_eq!(raw["questions"][0]["header"], "布局风格");
    assert!(raw["questions"][0]["question"].as_str().is_some());
    assert_eq!(raw["questions"][0]["options"].as_array().unwrap().len(), 3);
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnCompleted));
}

#[tokio::test]
async fn ask_user_question_real_multiselect_stream_json_shape() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let requests_capture = std::sync::Arc::clone(&requests);
    let mut sc = Scenario::new(
        adapter(),
        fixture_path("claude", "question_conversational_real_multiselect.fixture"),
    );
    sc.first_prompt = "pick preferences".into();
    sc.drive_send = true;
    sc.on_permission = Some(std::sync::Arc::new(move |req| {
        requests_capture.lock().unwrap().push(req.clone());
        let mut answers = std::collections::BTreeMap::new();
        answers.insert(
            "Preference".into(),
            lucarne::event::PermissionAnswer {
                answers: vec!["Red".into(), "Green".into()],
                ..Default::default()
            },
        );
        PermissionResponse {
            decision: Decision::Allow,
            answers,
        }
    }));
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;

    let req = requests
        .lock()
        .unwrap()
        .first()
        .cloned()
        .expect("PermissionRequest");
    assert_eq!(req.tool, "AskUserQuestion");
    assert_eq!(req.questions.len(), 1);
    let structured = &req.questions[0];
    assert!(structured.multi_select);
    assert_eq!(structured.header, "Preference");
    assert!(!structured.question.is_empty());
    assert_eq!(structured.options.len(), 3);
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    let call = calls[0].tool_call.as_ref().unwrap().call.clone();
    assert_eq!(call.name.as_str(), "AskUserQuestion");
    let raw = &call.input;
    let q = &raw["questions"][0];
    assert_eq!(q["header"], "Preference");
    assert_eq!(q["multiSelect"], true);
    assert_eq!(q["options"].as_array().unwrap().len(), 3);
    assert_eq!(q["options"][0]["label"], "Red");
    assert_eq!(q["options"][1]["label"], "Blue");
    assert_eq!(q["options"][2]["label"], "Green");
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnCompleted));
}

#[tokio::test]
async fn ask_user_question_real_single_option_rejected() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let requests_capture = std::sync::Arc::clone(&requests);
    let mut sc = Scenario::new(
        adapter(),
        fixture_path(
            "claude",
            "question_conversational_real_single_option_rejected.fixture",
        ),
    );
    sc.first_prompt = "ask for a default-only answer".into();
    sc.drive_send = true;
    sc.on_permission = Some(std::sync::Arc::new(move |req| {
        requests_capture.lock().unwrap().push(req.clone());
        PermissionResponse::from_decision(Decision::Allow)
    }));
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;

    let req = requests
        .lock()
        .unwrap()
        .first()
        .cloned()
        .expect("PermissionRequest");
    assert_eq!(req.tool, "AskUserQuestion");
    assert_eq!(req.questions.len(), 1);
    let structured = &req.questions[0];
    assert_eq!(structured.header, "Task");
    assert_eq!(structured.options.len(), 1);
    assert_eq!(structured.options[0].label, "Continue");
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    let call = calls[0].tool_call.as_ref().unwrap().call.clone();
    assert_eq!(call.name.as_str(), "AskUserQuestion");
    let raw = &call.input;
    let q = &raw["questions"][0];
    assert_eq!(q["header"], "Task");
    assert_eq!(q["options"].as_array().unwrap().len(), 1);
    assert_eq!(q["options"][0]["label"], "Continue");
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    let transcript = assistant_transcript(&msgs);
    assert!(transcript.contains("I need at least 2 options"));
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnCompleted));
}

#[tokio::test]
async fn ask_user_question_conversational_flow() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let requests_capture = std::sync::Arc::clone(&requests);
    let mut sc = Scenario::new(
        adapter(),
        fixture_path("claude", "question_conversational.fixture"),
    );
    sc.first_prompt = "pick a theme".into();
    sc.drive_send = true;
    sc.on_permission = Some(std::sync::Arc::new(move |req| {
        requests_capture.lock().unwrap().push(req.clone());
        let mut answers = std::collections::BTreeMap::new();
        answers.insert(
            "theme".into(),
            lucarne::event::PermissionAnswer {
                answers: vec!["Blue".into()],
                ..Default::default()
            },
        );
        PermissionResponse {
            decision: Decision::Allow,
            answers,
        }
    }));
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;

    let req = requests
        .lock()
        .unwrap()
        .first()
        .cloned()
        .expect("PermissionRequest");
    assert_eq!(req.tool, "AskUserQuestion");
    assert_eq!(req.questions.len(), 1);
    assert_eq!(req.questions[0].header, "Theme");
    assert_eq!(req.questions[0].question, "Which color should I use?");
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    let call = calls[0].tool_call.as_ref().unwrap().call.clone();
    assert_eq!(call.name.as_str(), "AskUserQuestion");
    let raw = &call.input;
    assert_eq!(raw["questions"][0]["header"], "Theme");
    assert_eq!(raw["questions"][0]["question"], "Which color should I use?");
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    let transcript = assistant_transcript(&msgs);
    assert!(transcript.contains("Using blue."));
}

#[tokio::test]
async fn ask_user_question_real_freeform_only_still_emits_options() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let requests_capture = std::sync::Arc::clone(&requests);
    let mut sc = Scenario::new(
        adapter(),
        fixture_path(
            "claude",
            "question_conversational_real_freeform_impossible.fixture",
        ),
    );
    sc.first_prompt = "ask for a freeform answer".into();
    sc.drive_send = true;
    sc.on_permission = Some(std::sync::Arc::new(move |req| {
        requests_capture.lock().unwrap().push(req.clone());
        PermissionResponse::from_decision(Decision::Allow)
    }));
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;

    let req = requests
        .lock()
        .unwrap()
        .first()
        .cloned()
        .expect("PermissionRequest");
    assert_eq!(req.tool, "AskUserQuestion");
    assert_eq!(req.questions.len(), 1);
    let structured = &req.questions[0];
    assert_eq!(structured.header, "Task");
    assert_eq!(structured.options.len(), 3);
    assert_eq!(structured.options[0].label, "Web Development");
    assert_eq!(structured.options[1].label, "Software Engineering");
    assert_eq!(structured.options[2].label, "Other");
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    let call = calls[0].tool_call.as_ref().unwrap().call.clone();
    assert_eq!(call.name.as_str(), "AskUserQuestion");
    let raw = &call.input;
    let q = &raw["questions"][0];
    assert_eq!(q["header"], "Task");
    assert_eq!(q["options"].as_array().unwrap().len(), 3);
    assert_eq!(q["options"][0]["label"], "Web Development");
    assert_eq!(q["options"][1]["label"], "Software Engineering");
    assert_eq!(q["options"][2]["label"], "Other");
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    let transcript = assistant_transcript(&msgs);
    assert!(transcript.contains("What would you like me to help you with today?"));
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnCompleted));
}

#[tokio::test]
async fn blocked_delete_real_stream_keeps_blocked_tool_result() {
    let mut sc = Scenario::new(
        adapter(),
        fixture_path("claude", "blocked_delete_real.fixture"),
    );
    sc.first_prompt = "delete the file".into();
    sc.drive_send = true;
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;

    assert!(!r.events.iter().any(|e| e.kind() == Kind::PermissionRequest));
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].tool_call.as_ref().unwrap().call.name.as_str(),
        "Bash"
    );
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    assert!(results[0]
        .tool_result
        .as_ref()
        .unwrap()
        .result
        .error
        .contains("blocked"));
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    let transcript = assistant_transcript(&msgs);
    assert!(transcript.contains("DELETE_OK"));
    assert!(r.events.iter().any(|e| e.kind() == Kind::TurnCompleted));
}

#[tokio::test]
async fn resume_propagates_session_id() {
    let mut sc = Scenario::new(adapter(), fixture_path("claude", "basic.fixture"));
    sc.first_prompt = "Hello Claude".into();
    sc.drive_send = true;
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;
    let closed = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::SessionClosed(c) => Some(c.clone()),
            _ => None,
        })
        .expect("SessionClosed");
    let resume = closed.resume.expect("resume handle");
    assert_eq!(
        resume.data.get("session_id").and_then(|v| v.as_str()),
        Some("sess-basic")
    );
}

#[tokio::test]
async fn error_uses_errors_array_fallback() {
    let mut sc = Scenario::new(
        adapter(),
        fixture_path("claude", "error_errors_array.fixture"),
    );
    sc.first_prompt = "fail from errors array".into();
    sc.drive_send = true;
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;

    let tf = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnFailed(tf) => Some(tf.clone()),
            _ => None,
        })
        .expect("TurnFailed");
    assert_eq!(tf.code, "error_during_execution");
    assert_eq!(tf.error, "no conversation found with session id sess-dead");
    assert!(!r.events.iter().any(|e| e.kind() == Kind::TurnCompleted));
}

#[tokio::test]
async fn assistant_usage_aliases_surface_as_usage_delta() {
    let mut sc = Scenario::new(adapter(), fixture_path("claude", "assistant_usage.fixture"));
    sc.first_prompt = "show usage".into();
    sc.drive_send = true;
    sc.timeout = Duration::from_secs(5);
    let r = run_scenario(sc).await;

    let usage_delta = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::UsageDelta(delta) => Some(delta.clone()),
            _ => None,
        })
        .expect("UsageDelta");
    assert_eq!(usage_delta.delta.input_tokens, 12);
    assert_eq!(usage_delta.delta.output_tokens, 5);
    assert_eq!(usage_delta.delta.cache_read_tokens, 4);
    assert_eq!(usage_delta.delta.cache_write_tokens, 2);

    let done = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::TurnCompleted(done) => Some(done.clone()),
            _ => None,
        })
        .expect("TurnCompleted");
    let usage = done.usage.expect("completed usage");
    assert_eq!(usage.cost_usd, 0.42);
    assert_eq!(usage.cache_read_tokens, 4);
    assert_eq!(usage.cache_write_tokens, 2);
}
