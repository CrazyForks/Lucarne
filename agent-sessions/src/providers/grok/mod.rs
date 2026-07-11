use smol_str::SmolStr;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Error, ParseSelection, Result};

mod types;
pub(crate) use types::*;

#[cfg(feature = "discovery")]
mod discovery;
#[cfg(feature = "agent_session")]
mod event;

pub struct Grok;

impl Grok {
    pub(crate) fn name() -> &'static str {
        "grok"
    }
}

/// Resolve Grok home: `GROK_HOME` then `~/.grok`.
pub(crate) fn grok_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("GROK_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(home));
    }
    crate::paths::home_dir().map(|home| home.join(".grok"))
}

pub(crate) fn sessions_root() -> Option<PathBuf> {
    grok_home().map(|home| home.join("sessions"))
}

/// Primary transcript path for a session directory.
#[allow(dead_code)]
pub(crate) fn updates_path(session_dir: &Path) -> PathBuf {
    session_dir.join("updates.jsonl")
}

#[allow(dead_code)]
pub(crate) fn summary_path(session_dir: &Path) -> PathBuf {
    session_dir.join("summary.json")
}

/// Session dir for an `updates.jsonl` path (parent directory).
#[allow(dead_code)]
pub(crate) fn session_dir_for_updates(updates: &Path) -> Option<&Path> {
    updates.parent()
}

#[derive(Debug, Deserialize)]
struct RawSummary {
    #[serde(default)]
    info: Option<RawSummaryInfo>,
    #[serde(default)]
    session_summary: Option<String>,
    #[serde(default)]
    generated_title: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    current_model_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawSummaryInfo {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawUpdateLine {
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<RawParams>,
    #[serde(default)]
    timestamp: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RawParams {
    #[serde(default, rename = "sessionId")]
    session_id: Option<String>,
    #[serde(default)]
    update: Option<RawSessionUpdate>,
}

#[derive(Debug, Deserialize)]
struct RawSessionUpdate {
    #[serde(default, rename = "sessionUpdate")]
    session_update: Option<String>,
    #[serde(default)]
    content: Option<serde_json::Value>,
    #[serde(default, rename = "toolCallId")]
    tool_call_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default, rename = "rawInput")]
    raw_input: Option<serde_json::Value>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "stop_reason")]
    stop_reason: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    kind: Option<String>,
}

#[cfg(feature = "agent_session")]
pub(crate) fn read_summary_meta(path: &Path) -> Result<Option<crate::agent_session::SessionMeta>> {
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(path).map_err(|err| {
        Error::Message(format!("failed to read {}: {err}", path.display()).into())
    })?;
    parse_summary_bytes(&bytes)
}

#[cfg(feature = "agent_session")]
fn parse_summary_bytes(bytes: &[u8]) -> Result<Option<crate::agent_session::SessionMeta>> {
    let raw: RawSummary = serde_json::from_slice(bytes)
        .map_err(|err| Error::Message(format!("grok summary.json: {err}").into()))?;
    let info = raw.info.as_ref();
    let session_id = info.and_then(|i| i.id.as_deref()).map(SmolStr::new);
    let cwd = info.and_then(|i| i.cwd.as_deref()).map(SmolStr::new);
    let title = raw
        .generated_title
        .or(raw.session_summary)
        .map(SmolStr::from);
    let model = raw.current_model_id.as_deref().map(SmolStr::new);
    Ok(Some(crate::agent_session::SessionMeta {
        session_id,
        cwd,
        title,
        created_at: raw.created_at.map(SmolStr::from),
        updated_at: raw.updated_at.map(SmolStr::from),
        models: model
            .map(|m| {
                vec![crate::agent_session::SessionModelMeta::zero(m)].into_boxed_slice()
            })
            .unwrap_or_default(),
        source_kind: Some("grok-v1".into()),
        ..Default::default()
    }))
}

#[cfg(any(test, feature = "watch", feature = "agent_session"))]
pub(super) fn parse_grok_body_reader<R>(
    reader: R,
    selection: ParseSelection,
    seed_meta: Option<Entry>,
) -> Result<Body>
where
    R: BufRead,
{
    let mut session_id = None;
    let mut entries = Vec::new();
    if let Some(Entry::SessionInfo {
        session_id: sid, ..
    }) = &seed_meta
    {
        session_id = Some(sid.clone());
    }
    if selection.includes_meta()
        && let Some(meta) = seed_meta
    {
        entries.push(meta);
    }

    let mut line = Vec::new();
    let mut line_num = 0usize;
    let mut reader = reader;
    // Aggregate streaming chunks into complete messages for history.
    let mut user_buf = String::new();
    let mut assistant_buf = String::new();
    let mut thought_buf = String::new();
    let mut pending_tools: std::collections::HashMap<String, (SmolStr, Option<SmolStr>)> =
        std::collections::HashMap::new();

    loop {
        line.clear();
        let n = reader
            .read_until(b'\n', &mut line)
            .map_err(|err| Error::Message(err.to_string().into()))?;
        if n == 0 {
            break;
        }
        line_num += 1;
        if line.iter().all(|b| b.is_ascii_whitespace()) {
            continue;
        }
        let text = std::str::from_utf8(&line)
            .map_err(|_| Error::Message("grok updates.jsonl is not valid UTF-8".into()))?
            .trim();
        let raw: RawUpdateLine = serde_json::from_str(text)
            .map_err(|err| Error::Message(format!("grok line {line_num}: {err}").into()))?;

        let method = raw.method.as_deref().unwrap_or("");
        if method != "session/update" && method != "_x.ai/session/update" {
            continue;
        }
        let Some(params) = raw.params.as_ref() else {
            continue;
        };
        if let Some(sid) = params.session_id.as_deref() {
            if session_id.is_none() {
                session_id = Some(sid.into());
            }
        }
        // Meta-only: keep scanning until session id is known, then stop.
        // Does not materialize message/tool entries (bounded for discovery).
        if selection.is_meta_only() {
            if session_id.is_some() {
                break;
            }
            continue;
        }
        let Some(update) = params.update.as_ref() else {
            continue;
        };
        let kind = update.session_update.as_deref().unwrap_or("");
        let ts = timestamp_to_smol(raw.timestamp.as_ref());

        match kind {
            "user_message_chunk" if selection.includes_messages() => {
                if let Some(chunk) = content_text(update.content.as_ref()) {
                    user_buf.push_str(&chunk);
                }
            }
            "agent_message_chunk" if selection.includes_messages() => {
                // flush thought before assistant text appears after tools mid-turn
                if !thought_buf.is_empty() {
                    entries.push(Entry::Thinking {
                        text: std::mem::take(&mut thought_buf).into(),
                        timestamp: ts.clone(),
                    });
                }
                if let Some(chunk) = content_text(update.content.as_ref()) {
                    assistant_buf.push_str(&chunk);
                }
            }
            "agent_thought_chunk" if selection.includes_messages() => {
                if let Some(chunk) = content_text(update.content.as_ref()) {
                    thought_buf.push_str(&chunk);
                }
            }
            "tool_call" if selection.includes_operations() => {
                flush_message_buffers(
                    &mut entries,
                    &mut user_buf,
                    &mut assistant_buf,
                    &mut thought_buf,
                    ts.clone(),
                );
                let id = update.tool_call_id.clone();
                let name = update
                    .title
                    .clone()
                    .unwrap_or_else(|| "tool".into());
                let input_json = update
                    .raw_input
                    .as_ref()
                    .map(|v| v.to_string().into());
                if let Some(id) = &id {
                    pending_tools.insert(id.clone(), (name.clone().into(), input_json.clone()));
                }
                entries.push(Entry::ToolCall {
                    id: id.map(Into::into),
                    name: name.into(),
                    input_json,
                    timestamp: ts,
                });
            }
            "tool_call_update" if selection.includes_operations() => {
                let id = update.tool_call_id.clone();
                let status = update.status.as_deref().unwrap_or("");
                if matches!(status, "completed" | "failed" | "error" | "cancelled") {
                    let is_error = matches!(status, "failed" | "error" | "cancelled");
                    let text = tool_result_text(update.content.as_ref()).unwrap_or_default();
                    let name = id
                        .as_ref()
                        .and_then(|i| pending_tools.get(i))
                        .map(|(n, _)| n.clone());
                    entries.push(Entry::ToolResult {
                        id: id.map(Into::into),
                        name,
                        text: text.into(),
                        is_error,
                        timestamp: ts,
                    });
                }
            }
            "turn_completed" if selection.includes_messages() || selection.includes_state() => {
                flush_message_buffers(
                    &mut entries,
                    &mut user_buf,
                    &mut assistant_buf,
                    &mut thought_buf,
                    ts.clone(),
                );
                entries.push(Entry::TurnCompleted {
                    stop_reason: update.stop_reason.clone().map(Into::into),
                    timestamp: ts,
                });
            }
            _ => {}
        }
    }

    flush_message_buffers(
        &mut entries,
        &mut user_buf,
        &mut assistant_buf,
        &mut thought_buf,
        None,
    );

    Ok(Body {
        session_id,
        entries: entries.into_boxed_slice(),
    })
}

fn flush_message_buffers(
    entries: &mut Vec<Entry>,
    user_buf: &mut String,
    assistant_buf: &mut String,
    thought_buf: &mut String,
    timestamp: Option<SmolStr>,
) {
    if !user_buf.is_empty() {
        entries.push(Entry::UserMessage {
            text: std::mem::take(user_buf).into(),
            timestamp: timestamp.clone(),
        });
    }
    if !thought_buf.is_empty() {
        entries.push(Entry::Thinking {
            text: std::mem::take(thought_buf).into(),
            timestamp: timestamp.clone(),
        });
    }
    if !assistant_buf.is_empty() {
        entries.push(Entry::AssistantMessage {
            text: std::mem::take(assistant_buf).into(),
            timestamp,
            model: None,
        });
    }
}

fn content_text(content: Option<&serde_json::Value>) -> Option<String> {
    let content = content?;
    if let Some(text) = content.get("text").and_then(|v| v.as_str()) {
        return Some(text.to_string());
    }
    if let Some(arr) = content.as_array() {
        let mut out = String::new();
        for item in arr {
            if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                out.push_str(t);
            } else if let Some(t) = item
                .get("content")
                .and_then(|c| c.get("text"))
                .and_then(|v| v.as_str())
            {
                out.push_str(t);
            }
        }
        if !out.is_empty() {
            return Some(out);
        }
    }
    None
}

fn tool_result_text(content: Option<&serde_json::Value>) -> Option<String> {
    let content = content?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    if let Some(arr) = content.as_array() {
        let mut out = String::new();
        for item in arr {
            if let Some(t) = item
                .pointer("/content/text")
                .and_then(|v| v.as_str())
                .or_else(|| item.get("text").and_then(|v| v.as_str()))
            {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(t);
            }
        }
        if !out.is_empty() {
            return Some(out);
        }
    }
    content_text(Some(content))
}

fn timestamp_to_smol(value: Option<&serde_json::Value>) -> Option<SmolStr> {
    match value {
        Some(serde_json::Value::String(s)) => Some(s.as_str().into()),
        Some(serde_json::Value::Number(n)) => Some(n.to_string().into()),
        _ => None,
    }
}

#[cfg(feature = "agent_session")]
pub(crate) fn session_info_entry_from_meta(
    meta: &crate::agent_session::SessionMeta,
) -> Option<Entry> {
    let session_id = meta.session_id.clone()?;
    Some(Entry::SessionInfo {
        session_id,
        cwd: meta.cwd.clone(),
        title: meta.title.clone(),
        model: meta.models.first().map(|m| m.model.clone()),
        created_at: meta.created_at.clone(),
        updated_at: meta.updated_at.clone(),
    })
}

/// True when path is under a `subagents` directory (exclude from history).
pub(crate) fn is_subagent_path(path: &Path) -> bool {
    path.components()
        .any(|c| c.as_os_str() == "subagents")
}

pub(crate) fn is_updates_jsonl(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "updates.jsonl")
}
