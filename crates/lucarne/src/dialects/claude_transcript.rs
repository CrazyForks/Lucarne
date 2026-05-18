//! Claude transcript → stream-json reconstructor.
//!
//! Port of `lucarne/pkg/dialect/claude/transcript.go`.
//!
//! Claude Code persists every interactive session as JSONL under
//! `~/.claude/projects/<project>/<session>.jsonl`. Those transcript
//! records are a *superset* of the stream-json wire format — they
//! include queue operations, plain user approval text, attachments, etc.
//!
//! [`extract_stream_json_frames_from_transcript`] conservatively
//! projects a transcript back into the subset of stream-json frames
//! that [`super::claude::Claude`] can replay losslessly:
//!
//! * Assistant messages containing `text` / `thinking` / `tool_use`
//!   blocks. Consecutive entries sharing the same `message.id` are
//!   merged to reconstruct the original streamed assistant message.
//! * User messages containing `tool_result` blocks.
//!
//! Everything else — user prompts, queue ops, attachments, permission
//! approval text — is skipped. This is what lets the resume UI show a
//! coherent timeline without double-emitting the user's "Yes, I
//! approve" replies.

use serde_json::{Map, Value};
use std::io::{BufRead, BufReader, Read};

/// Projects `reader` (Claude project-transcript JSONL) onto the subset
/// of stream-json stdout frames that survive a round-trip.
///
/// Returns one byte buffer per emitted frame. Each buffer is a single
/// JSON object without a trailing newline; callers that need a wire
/// stream should append `\n` between frames.
pub fn extract_stream_json_frames_from_transcript<R: Read>(
    reader: R,
) -> Result<Vec<Vec<u8>>, String> {
    // Match Go's 4 MiB line cap for oversized attachments.
    let br = BufReader::with_capacity(64 * 1024, reader);
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut pending: Option<Map<String, Value>> = None;
    let mut pending_id = String::new();

    let flush_assistant = |pending: &mut Option<Map<String, Value>>,
                           pending_id: &mut String,
                           frames: &mut Vec<Vec<u8>>|
     -> Result<(), String> {
        if let Some(msg) = pending.take() {
            let frame = Value::Object({
                let mut m = Map::new();
                m.insert("type".into(), Value::String("assistant".into()));
                m.insert("message".into(), Value::Object(msg));
                m
            });
            let bytes = serde_json::to_vec(&frame)
                .map_err(|e| format!("marshal assistant frame: {}", e))?;
            frames.push(bytes);
            pending_id.clear();
        }
        Ok(())
    };

    for (i, line) in br.lines().enumerate() {
        let line = line.map_err(|e| format!("read transcript line {}: {}", i + 1, e))?;
        if line.trim().is_empty() {
            continue;
        }
        let record: Map<String, Value> = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Object(m)) => m,
            Ok(_) => continue,
            Err(e) => return Err(format!("unmarshal transcript line: {}", e)),
        };
        let ty = record.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match ty {
            "assistant" => {
                let message = filter_transcript_assistant_message(record.get("message"));
                let Some(message) = message else { continue };
                let id = message
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if pending.is_some() && !id.is_empty() && id == pending_id {
                    if let Some(ref mut dst) = pending {
                        merge_transcript_assistant_message(dst, message);
                    }
                    continue;
                }
                flush_assistant(&mut pending, &mut pending_id, &mut frames)?;
                pending = Some(message);
                pending_id = id;
            }
            "user" => {
                flush_assistant(&mut pending, &mut pending_id, &mut frames)?;
                let Some(message) = filter_transcript_user_tool_result(record.get("message"))
                else {
                    continue;
                };
                let frame = Value::Object({
                    let mut m = Map::new();
                    m.insert("type".into(), Value::String("user".into()));
                    m.insert("message".into(), Value::Object(message));
                    m
                });
                let bytes =
                    serde_json::to_vec(&frame).map_err(|e| format!("marshal user frame: {}", e))?;
                frames.push(bytes);
            }
            _ => continue,
        }
    }
    flush_assistant(&mut pending, &mut pending_id, &mut frames)?;
    Ok(frames)
}

fn filter_transcript_assistant_message(raw: Option<&Value>) -> Option<Map<String, Value>> {
    let msg = raw?.as_object()?;
    // Allow missing role (some vendors omit it); reject only explicit
    // non-assistant roles.
    if let Some(Value::String(r)) = msg.get("role") {
        if !r.is_empty() && r != "assistant" {
            return None;
        }
    }
    let allowed: &[&str] = &["text", "thinking", "tool_use"];
    let content = filter_transcript_content(msg.get("content"), allowed);
    if content.is_empty() {
        return None;
    }
    let mut out = Map::new();
    out.insert("content".into(), Value::Array(content));
    for key in [
        "id",
        "type",
        "role",
        "model",
        "stop_reason",
        "stop_sequence",
        "stop_details",
        "usage",
    ] {
        if let Some(v) = msg.get(key) {
            out.insert(key.to_string(), v.clone());
        }
    }
    Some(out)
}

fn filter_transcript_user_tool_result(raw: Option<&Value>) -> Option<Map<String, Value>> {
    let msg = raw?.as_object()?;
    if let Some(Value::String(r)) = msg.get("role") {
        if !r.is_empty() && r != "user" {
            return None;
        }
    }
    let content = filter_transcript_content(msg.get("content"), &["tool_result"]);
    if content.is_empty() {
        return None;
    }
    let mut out = Map::new();
    out.insert("content".into(), Value::Array(content));
    if let Some(role) = msg.get("role") {
        out.insert("role".into(), role.clone());
    }
    Some(out)
}

fn filter_transcript_content(raw: Option<&Value>, allowed: &[&str]) -> Vec<Value> {
    let Some(Value::Array(items)) = raw else {
        return Vec::new();
    };
    let mut out: Vec<Value> = Vec::with_capacity(items.len());
    for item in items {
        let Some(block) = item.as_object() else {
            continue;
        };
        let ty = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if !allowed.contains(&ty) {
            continue;
        }
        // Clone the block — serde_json::Map already shares strings, so
        // this is the same shallow copy Go's `copied := make(map...)` does.
        out.push(Value::Object(block.clone()));
    }
    out
}

fn merge_transcript_assistant_message(dst: &mut Map<String, Value>, src: Map<String, Value>) {
    // Concatenate content blocks.
    let mut dst_content: Vec<Value> = match dst.remove("content") {
        Some(Value::Array(a)) => a,
        _ => Vec::new(),
    };
    if let Some(Value::Array(src_content)) = src.get("content") {
        if !src_content.is_empty() {
            dst_content.extend(src_content.iter().cloned());
        }
    }
    dst.insert("content".into(), Value::Array(dst_content));
    // Overlay every other top-level key.
    for (k, v) in src {
        if k == "content" {
            continue;
        }
        dst.insert(k, v);
    }
}
