//! Grok Build ACP fixture scenarios — drive adapter against fakeagent.

pub mod common;

use common::{collect_timelines, fakeagent_bin, fixture_path, kinds, run_scenario, Scenario};
use lucarne::adapters::grok;
use lucarne::dialect::Input;
use lucarne::event::{
    Decision, Kind, Payload, PermissionRequest, PermissionResponse, ResumeHandle, TimelineType,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn adapter() -> std::sync::Arc<lucarne::adapter::ProtocolAdapter> {
    grok::new(grok::Options {
        binary: fakeagent_bin().to_string_lossy().into_owned(),
    })
}

fn base_scenario(fixture: &str) -> Scenario {
    let mut sc = Scenario::new(adapter(), fixture_path("grok", fixture));
    sc.first_prompt = "hi".into();
    sc.model = "grok-4.5".into();
    sc.drive_send = true;
    sc.timeout = Duration::from_secs(10);
    sc
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
    assert_eq!(started.session_id, "019f4f1c-8ae8-7632-adb2-6133aee3adf3");

    let reasoning = collect_timelines(&r.events, TimelineType::Reasoning);
    assert!(!reasoning.is_empty());
    assert!(reasoning[0]
        .reasoning
        .as_ref()
        .unwrap()
        .text
        .contains("Inspecting"));

    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    assert!(
        msgs.iter().any(|m| {
            m.assistant_message
                .as_ref()
                .is_some_and(|a| a.text.contains("Hello") || a.text.contains("from Grok"))
        }),
        "assistant texts missing: {:?}",
        msgs
    );

    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].tool_call.as_ref().unwrap().call.name.as_str(),
        "read_file"
    );

    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].tool_result.as_ref().unwrap().result.output,
        "file contents"
    );

    assert!(
        r.events
            .iter()
            .any(|e| matches!(e.payload, Payload::TurnCompleted(_)))
    );
}

#[tokio::test]
async fn multi_turn_flow() {
    let sent_second = Arc::new(AtomicBool::new(false));
    let sent_second_c = Arc::clone(&sent_second);
    let mut sc = base_scenario("multi_turn.fixture");
    sc.on_event = Some(Arc::new(move |sess, ev| {
        let sent_second = Arc::clone(&sent_second_c);
        Box::pin(async move {
            if matches!(ev.payload, Payload::TurnCompleted(_))
                && !sent_second.swap(true, Ordering::SeqCst)
            {
                sess.send(Input {
                    text: "again".into(),
                    images: vec![],
                })
                .await
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let turns: Vec<_> = r
        .events
        .iter()
        .filter(|e| e.kind() == Kind::TurnCompleted)
        .collect();
    assert!(
        turns.len() >= 2,
        "expected >=2 turn completions, got {:?}",
        kinds(&r.events)
    );
}

#[tokio::test]
async fn resume_propagates_session_id() {
    let mut sc = base_scenario("resume.fixture");
    let mut data = BTreeMap::new();
    data.insert(
        "session_id".into(),
        serde_json::Value::String("uuid-resume-1".into()),
    );
    sc.resume = Some(ResumeHandle {
        version: 1,
        data,
    });
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
    assert_eq!(started.session_id, "uuid-resume-1");
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
        Some("uuid-resume-1")
    );
}

#[tokio::test]
async fn permission_allow_flow() {
    let mut sc = base_scenario("permission.fixture");
    sc.on_permission = Some(Arc::new(|_req: &PermissionRequest| {
        PermissionResponse::from_decision(Decision::Allow)
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    assert!(
        r.events
            .iter()
            .any(|e| matches!(e.payload, Payload::PermissionRequest(_))),
        "expected PermissionRequest, kinds={:?}",
        kinds(&r.events)
    );
    assert!(
        r.events
            .iter()
            .any(|e| matches!(e.payload, Payload::TurnCompleted(_)))
    );
}

#[tokio::test]
async fn interrupt_flow() {
    let mut sc = base_scenario("interrupt.fixture");
    sc.on_event = Some(Arc::new(|sess, ev| {
        Box::pin(async move {
            if matches!(&ev.payload, Payload::Timeline(t) if t.item.assistant_message.is_some()) {
                sess.interrupt().await.map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    }));
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
}

/// Interrupt first turn, then recover with a second successful turn (Codex F1.09 role).
#[tokio::test]
async fn interrupt_recovery_second_turn_success_after_cancel() {
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
            match &ev.payload {
                Payload::Timeline(tl)
                    if tl.item.ty == TimelineType::ToolCall
                        && !interrupted.swap(true, Ordering::SeqCst) =>
                {
                    sess.interrupt().await.map_err(|err| err.to_string())
                }
                Payload::TurnFailed(tf)
                    if interrupted.load(Ordering::SeqCst)
                        && !resent.swap(true, Ordering::SeqCst)
                        && tf.error == "cancelled" =>
                {
                    sess.send(Input {
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
        "interrupt recovery flow did not fire; kinds={:?}",
        kinds(&r.events)
    );

    let failures: Vec<_> = r
        .events
        .iter()
        .filter_map(|e| match &e.payload {
            Payload::TurnFailed(tf) => Some(tf.error.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(failures, vec!["cancelled".to_string()]);

    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    assert!(
        msgs.iter().any(|m| {
            m.assistant_message
                .as_ref()
                .is_some_and(|a| a.text.contains("Recovered after cancel."))
        }),
        "missing recovery assistant text: {msgs:?}"
    );
    assert!(
        r.events
            .iter()
            .any(|e| matches!(e.payload, Payload::TurnCompleted(_))),
        "missing recovered TurnCompleted; kinds={:?}",
        kinds(&r.events)
    );
}

/// Empty session/new result closes cleanly (Codex F1.19 role).
#[tokio::test]
async fn start_missing_id_closes_session() {
    let r = run_scenario(base_scenario("start_missing_id.fixture")).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let closed = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::SessionClosed(s) => Some(s.clone()),
            _ => None,
        })
        .expect("SessionClosed");
    assert!(
        closed.reason.contains("missing sessionId") || closed.reason.contains("session"),
        "unexpected reason = {:?}",
        closed.reason
    );
    assert!(
        !r.events
            .iter()
            .any(|e| matches!(e.payload, Payload::SessionStarted(_))),
        "must not SessionStarted without sessionId"
    );
}

/// session/load RPC error closes session (Grok has no thread/start-style fallback).
#[tokio::test]
async fn resume_load_error_closes_session() {
    let mut sc = base_scenario("resume_load_error.fixture");
    let mut data = BTreeMap::new();
    data.insert(
        "session_id".into(),
        serde_json::Value::String("stale-uuid".into()),
    );
    sc.resume = Some(ResumeHandle {
        version: 1,
        data,
    });
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let closed = r
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::SessionClosed(s) => Some(s.clone()),
            _ => None,
        })
        .expect("SessionClosed");
    assert!(
        closed.reason.contains("unknown session"),
        "unexpected reason = {:?}",
        closed.reason
    );
    assert!(
        !r.events
            .iter()
            .any(|e| matches!(e.payload, Payload::SessionStarted(_))),
        "load error must not start session"
    );
}

/// Empty sessionId on successful session/load keeps the requested resume UUID.
#[tokio::test]
async fn resume_empty_session_id_keeps_requested_uuid() {
    let mut sc = base_scenario("resume_empty_session_id.fixture");
    let mut data = BTreeMap::new();
    data.insert(
        "session_id".into(),
        serde_json::Value::String("uuid-resume-empty".into()),
    );
    sc.resume = Some(ResumeHandle {
        version: 1,
        data,
    });
    sc.first_prompt = "Resume if possible".into();
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
    assert_eq!(started.session_id, "uuid-resume-empty");
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    assert!(
        msgs.iter().any(|m| {
            m.assistant_message
                .as_ref()
                .is_some_and(|a| a.text.contains("Loaded with requested UUID."))
        }),
        "assistant texts: {msgs:?}"
    );
}

#[tokio::test]
async fn error_emits_turn_failed() {
    let r = run_scenario(base_scenario("error.fixture")).await;
    assert!(
        r.events
            .iter()
            .any(|e| matches!(&e.payload, Payload::TurnFailed(f) if f.error.contains("boom"))),
        "kinds={:?}",
        kinds(&r.events)
    );
}

/// Catalog + AdapterMapped command surface (peer parity with Pi/Codex/Gemini).
#[tokio::test]
async fn commands_catalog_and_system_dispatch() {
    use lucarne::agent_runtime::{AgentCommandInvocation, AgentCommandSource};
    use lucarne::dialect::{CommandDispatch, CommandResult, Dialect};

    let mut sc = base_scenario("commands.fixture");
    sc.drive_send = false;
    let r = run_scenario(sc).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));

    // Drive dialect unit-style against the same session shapes the fixture validates.
    let mut d = lucarne::dialects::grok_acp::GrokAcp::new();
    d.init(&lucarne::dialect::SessionParams {
        cwd: "/tmp/project".into(),
        model: "grok-4.5".into(),
        ..Default::default()
    });
    let _ = d.translate(
        br#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"_meta":{"agentVersion":"0.2.93"}}}"#,
    );
    let _ = d.drain_out_frames();
    let _ = d.translate(
        br#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"019f4f1c-cmd-fixture-0000000001","models":{"currentModelId":"grok-4.5","availableModels":[{"modelId":"grok-4.5","name":"Grok 4.5"}]}}}"#,
    );
    let _ = d.translate(
        br#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"019f4f1c-cmd-fixture-0000000001","update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"compact","description":"Compress"},{"name":"status","description":"filtered"},{"name":"tdd","description":"skill","_meta":{"path":"/skills/tdd/SKILL.md"}}]}}}"#,
    );

    let catalog = d.command_catalog();
    assert!(
        catalog.commands.iter().any(|c| c.name == "compact"),
        "expected compact native: {catalog:?}"
    );
    assert!(
        !catalog.commands.iter().any(|c| c.name == "status"),
        "AdapterMapped status must not be in catalog: {catalog:?}"
    );

    for name in [
        "status",
        "model",
        "permissions",
        "skills",
        "list_commands",
        "fork",
        "quit",
    ] {
        let out = d
            .handle_system_command(&AgentCommandInvocation {
                name: name.into(),
                args: None,
                values: serde_json::Value::Null,
                source: AgentCommandSource::AdapterMapped,
            })
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        if name == "quit" {
            assert!(matches!(out, CommandDispatch::Ready(CommandResult::Quit)));
        } else {
            assert!(
                matches!(out, CommandDispatch::Ready(_)),
                "{name} => {out:?}"
            );
        }
    }

    let set_perm = d
        .handle_system_command(&AgentCommandInvocation {
            name: "permissions".into(),
            args: Some("always-approve".into()),
            values: serde_json::Value::Null,
            source: AgentCommandSource::AdapterMapped,
        })
        .expect("set permissions");
    assert!(
        matches!(set_perm, CommandDispatch::Deferred(_)),
        "permissions set must defer slash frames to agent"
    );

    let native = d
        .handle_native_command(&AgentCommandInvocation {
            name: "compact".into(),
            args: None,
            values: serde_json::Value::Null,
            source: AgentCommandSource::ProviderNative,
        })
        .expect("native compact");
    assert!(matches!(native, CommandDispatch::Deferred(_)));
}

/// Fixture smoke for ACP reverse `fs/*` client RPCs (live tool path depends on these).
#[tokio::test]
async fn tool_fs_reverse_rpc_flow() {
    let r = run_scenario(base_scenario("tool_fs.fixture")).await;
    assert!(r.closed, "kinds = {:?}", kinds(&r.events));
    let calls = collect_timelines(&r.events, TimelineType::ToolCall);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].tool_call.as_ref().unwrap().call.name.as_str(),
        "write"
    );
    let results = collect_timelines(&r.events, TimelineType::ToolResult);
    assert_eq!(results.len(), 1);
    let msgs = collect_timelines(&r.events, TimelineType::AssistantMessage);
    assert!(
        msgs.iter().any(|m| {
            m.assistant_message
                .as_ref()
                .is_some_and(|a| a.text.contains("TOOL_OK"))
        }),
        "assistant texts missing TOOL_OK: {:?}",
        msgs
    );
    assert!(
        r.events
            .iter()
            .any(|e| matches!(e.payload, Payload::TurnCompleted(_)))
    );
    // Side effect of reverse write RPC
    let written = std::fs::read_to_string("/tmp/lucarne-grok-acp-fs-test.txt")
        .expect("fs/write_text_file should have written the file");
    assert_eq!(written, "lucarne-fs-ok\n");
}

#[test]
fn history_lists_grok_provider_id() {
    let ids = lucarne::history::history_providers()
        .into_iter()
        .map(|p| p.id())
        .collect::<Vec<_>>();
    assert!(
        ids.contains(&"grok"),
        "history providers missing grok: {ids:?}"
    );
}

#[test]
fn adapter_descriptor_registers_grok() {
    let ids = lucarne::adapters::default_adapter_provider_ids();
    assert!(ids.contains(&"grok"), "adapter ids missing grok: {ids:?}");
}
