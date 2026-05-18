//! Port of `lucarne/pkg/dialect/claude/transcript_test.go`.

pub mod common;

use lucarne::dialect::{Dialect, SessionParams};
use lucarne::dialects::claude::Claude;
use lucarne::dialects::claude_transcript::extract_stream_json_frames_from_transcript;
use lucarne::event::{Event, Kind, Payload, TimelineType};
use serde_json::Value;
use std::fs::File;
use std::io::BufReader;

fn load(name: &str) -> Vec<Vec<u8>> {
    let mut p = common::repo_root();
    p.push("tests");
    p.push("data");
    p.push("claude");
    p.push(name);
    let f = File::open(&p).expect("open transcript fixture");
    extract_stream_json_frames_from_transcript(BufReader::new(f)).expect("extract frames")
}

fn translate(frames: &[Vec<u8>]) -> Vec<Event> {
    let mut d = Claude::new();
    d.init(&SessionParams {
        cwd: "/tmp/workdir".into(),
        ..Default::default()
    });
    let mut out: Vec<Event> = Vec::new();
    for f in frames {
        out.extend(d.translate(f));
    }
    out
}

fn permission_requests(events: &[Event]) -> Vec<&lucarne::event::PermissionRequest> {
    events
        .iter()
        .filter_map(|e| match (e.kind(), &e.payload) {
            (Kind::PermissionRequest, Payload::PermissionRequest(r)) => Some(r),
            _ => None,
        })
        .collect()
}

fn count_timeline(events: &[Event], ty: TimelineType) -> usize {
    events
        .iter()
        .filter(|e| {
            matches!(
                (e.kind(), &e.payload),
                (Kind::Timeline, Payload::Timeline(tl)) if tl.item.ty == ty
            )
        })
        .count()
}

fn assistant_transcript(events: &[Event]) -> String {
    let mut b = String::new();
    for e in events {
        if e.kind() != Kind::Timeline {
            continue;
        }
        if let Payload::Timeline(tl) = &e.payload {
            if tl.item.ty == TimelineType::AssistantMessage {
                if let Some(am) = &tl.item.assistant_message {
                    b.push_str(&am.text);
                }
            }
        }
    }
    b
}

#[test]
fn groups_only_protocol_frames() {
    let frames = load("transcript_write_permission.jsonl");
    assert_eq!(frames.len(), 3, "expected 3 frames, got {}", frames.len());

    let frame0: Value = serde_json::from_slice(&frames[0]).unwrap();
    assert_eq!(frame0.get("type"), Some(&Value::String("assistant".into())));
    let msg0 = frame0.get("message").and_then(|m| m.as_object()).unwrap();
    assert_eq!(
        msg0.get("id"),
        Some(&Value::String("msg_real_perm_1".into()))
    );
    let content0 = msg0.get("content").and_then(|c| c.as_array()).unwrap();
    assert_eq!(content0.len(), 2);
    assert_eq!(content0[0]["type"], Value::String("thinking".into()));
    assert_eq!(content0[1]["type"], Value::String("tool_use".into()));

    let frame1: Value = serde_json::from_slice(&frames[1]).unwrap();
    assert_eq!(frame1.get("type"), Some(&Value::String("user".into())));
    let content1 = frame1
        .pointer("/message/content")
        .and_then(|c| c.as_array())
        .unwrap();
    assert_eq!(content1.len(), 1);
    assert_eq!(content1[0]["type"], Value::String("tool_result".into()));

    let frame2: Value = serde_json::from_slice(&frames[2]).unwrap();
    assert_eq!(frame2.get("type"), Some(&Value::String("assistant".into())));
    let content2 = frame2
        .pointer("/message/content")
        .and_then(|c| c.as_array())
        .unwrap();
    assert_eq!(content2.len(), 1);
    assert_eq!(
        content2[0]["text"],
        Value::String("I need permission to write ./danger.txt.".into())
    );
}

#[test]
fn real_ask_user_question_preserves_question_shape() {
    let frames = load("transcript_ask_user_question.jsonl");
    assert_eq!(frames.len(), 1);
    let raw = std::str::from_utf8(&frames[0]).unwrap();
    assert!(!raw.contains("我选"), "user answer text must be dropped");
    let frame: Value = serde_json::from_slice(&frames[0]).unwrap();
    let content = frame
        .pointer("/message/content")
        .and_then(|c| c.as_array())
        .unwrap();
    let tool_use = &content[0];
    assert_eq!(tool_use["name"], Value::String("AskUserQuestion".into()));
    let questions = tool_use
        .pointer("/input/questions")
        .and_then(|q| q.as_array())
        .unwrap();
    let q = &questions[0];
    assert_eq!(q["header"], Value::String("布局风格".into()));

    let events = translate(&frames);
    let reqs = permission_requests(&events);
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].tool, "AskUserQuestion");
    assert_eq!(reqs[0].questions.len(), 1);
    assert_eq!(reqs[0].questions[0].header, "布局风格");
    assert_eq!(count_timeline(&events, TimelineType::ToolCall), 1);
}

#[test]
fn real_approval_loop_skips_user_approval_replies() {
    let frames = load("transcript_write_permission_loop.jsonl");
    assert_eq!(frames.len(), 6);
    for f in &frames {
        let raw = std::str::from_utf8(f).unwrap();
        assert!(
            !raw.contains("Yes, I approve"),
            "extractor must not replay transcript-only user approval text"
        );
    }

    let events = translate(&frames);
    let reqs = permission_requests(&events);
    assert_eq!(reqs.len(), 0);
    assert_eq!(count_timeline(&events, TimelineType::ToolCall), 2);
    let transcript = assistant_transcript(&events);
    assert!(
        transcript.contains("I need permission to write the file."),
        "missing approval ask: {}",
        transcript
    );
    assert!(
        transcript.contains("TOOL_OK"),
        "missing TOOL_OK: {}",
        transcript
    );
}

#[test]
fn real_blocked_delete_does_not_invent_permission() {
    let frames = load("transcript_blocked_delete.jsonl");
    assert_eq!(frames.len(), 3);
    let events = translate(&frames);
    assert_eq!(permission_requests(&events).len(), 0);
    assert_eq!(count_timeline(&events, TimelineType::ToolResult), 1);
    let transcript = assistant_transcript(&events);
    assert!(
        transcript.contains("DELETE_OK"),
        "missing DELETE_OK: {}",
        transcript
    );
}
