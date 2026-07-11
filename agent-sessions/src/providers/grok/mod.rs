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
        // Keep product updated_at for watch/seed consumers; history list probe
        // may clear it so ranking/display align on transcript mtime (peer path).
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

/// Probe history-list meta the same way Codex/Pi do:
/// session header (summary.json) + first usable user title from the transcript.
///
/// Title priority (peer-aligned):
/// 1. Non-preamble `generated_title` / `session_summary` (like Pi session name)
/// 2. First non-preamble `user_message_chunk` from `updates.jsonl` (like Codex/Pi)
/// 3. Preamble user text / junk generated title as last-resort fallback
///
/// Reading stops once a preferred title is resolved so large rollouts stay cheap.
#[cfg(feature = "agent_session")]
impl Grok {
    pub(crate) fn probe_agent_session_meta_with_title<R>(
        summary_meta: Option<crate::agent_session::SessionMeta>,
        updates: R,
    ) -> Result<crate::agent_session::SessionMeta>
    where
        R: BufRead,
    {
        let mut meta = summary_meta.unwrap_or_else(|| crate::agent_session::SessionMeta {
            source_kind: Some("grok-v1".into()),
            ..Default::default()
        });
        if meta.source_kind.is_none() {
            meta.source_kind = Some("grok-v1".into());
        }

        // Peer list rows rank/display on the primary session file mtime. Grok's
        // summary.updated_at is often refreshed by sidecars without transcript
        // growth — omit it so history falls through to updates.jsonl mtime.
        meta.updated_at = None;

        let header_title = meta.title.clone();
        let header_is_preferred = header_title
            .as_deref()
            .is_some_and(|t| !is_grok_instruction_preamble(t));
        if header_is_preferred {
            // Pi: session name/title wins when already set and usable.
            if let Some(title) = header_title {
                meta.title = Some(first_line_snippet(title.as_str(), 80).into());
            }
            return Ok(meta);
        }

        // Header title missing or junk — scan transcript like Codex/Pi.
        let (title_candidate, fallback_title) = Self::probe_updates_title(updates)?;
        meta.title = title_candidate
            .or(fallback_title)
            .or_else(|| {
                header_title
                    .map(|t| first_line_snippet(t.as_str(), 80).into())
            });
        Ok(meta)
    }

    /// Bounded scan of updates.jsonl for the first usable user title.
    fn probe_updates_title<R>(
        mut reader: R,
    ) -> Result<(Option<SmolStr>, Option<SmolStr>)>
    where
        R: BufRead,
    {
        let mut line = Vec::new();
        let mut title_candidate: Option<SmolStr> = None;
        let mut fallback_title: Option<SmolStr> = None;
        let mut lines_seen = 0usize;
        // Cap scan so multi-MB harness sessions stay list-cheap (Codex stops early).
        const MAX_PROBE_LINES: usize = 400;

        loop {
            line.clear();
            let n = reader
                .read_until(b'\n', &mut line)
                .map_err(|err| Error::Message(err.to_string().into()))?;
            if n == 0 {
                break;
            }
            lines_seen += 1;
            if lines_seen > MAX_PROBE_LINES {
                break;
            }
            if line.iter().all(|b| b.is_ascii_whitespace()) {
                continue;
            }
            let Ok(text) = std::str::from_utf8(&line) else {
                continue;
            };
            let Ok(raw) = serde_json::from_str::<RawUpdateLine>(text.trim()) else {
                // Best-effort past seed: do not fail the whole list entry.
                continue;
            };
            let method = raw.method.as_deref().unwrap_or("");
            if method != "session/update" && method != "_x.ai/session/update" {
                continue;
            }
            let Some(params) = raw.params.as_ref() else {
                continue;
            };
            let Some(update) = params.update.as_ref() else {
                continue;
            };
            if update.session_update.as_deref() != Some("user_message_chunk") {
                continue;
            }
            let Some(chunk) = content_text(update.content.as_ref()) else {
                continue;
            };
            let title = first_line_snippet(&chunk, 80);
            if title.is_empty() {
                continue;
            }
            if is_grok_instruction_preamble(&chunk) {
                if fallback_title.is_none() {
                    fallback_title = Some(title.into());
                }
                continue;
            }
            title_candidate = Some(title.into());
            break;
        }
        Ok((title_candidate, fallback_title))
    }
}

pub(crate) fn first_line_snippet(text: &str, max: usize) -> String {
    let first = text.lines().next().unwrap_or(text).trim();
    // Strip light markdown emphasis for list readability (peer UIs show plain text).
    let first = first.replace("**", "");
    if first.chars().count() > max {
        let mut iter = first.chars();
        let truncated: String = iter.by_ref().take(max).collect();
        format!("{truncated}…")
    } else {
        first
    }
}

/// Instruction / harness role dumps that must not win list titles (Codex/Pi preamble gate).
pub(crate) fn is_grok_instruction_preamble(text: &str) -> bool {
    let trimmed = text.trim_start();
    let first = trimmed.lines().next().unwrap_or("").trim();
    let first_plain = first.replace("**", "");
    first_plain.starts_with("You are an ")
        || first_plain.starts_with("You are the ")
        || first_plain.starts_with("You are a ")
        || first_plain.starts_with("You are an**")
        || first.starts_with("Task: You are a delegated subagent")
        || trimmed.starts_with("<INSTRUCTIONS>")
        || trimmed.contains("\n<INSTRUCTIONS>")
        || first.starts_with("# AGENTS.md instructions")
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

#[cfg(all(test, feature = "agent_session"))]
mod title_probe_tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn preferred_generated_title_wins_without_scanning_preamble_user() {
        let summary = crate::agent_session::SessionMeta {
            session_id: Some("sid".into()),
            cwd: Some("/tmp/p".into()),
            title: Some("Codex/Pi Full Alignment Goal Summarizer".into()),
            created_at: Some("2026-07-11T05:00:00Z".into()),
            updated_at: Some("2026-07-11T06:00:00Z".into()),
            source_kind: Some("grok-v1".into()),
            ..Default::default()
        };
        let updates = r#"{"method":"session/update","params":{"sessionId":"sid","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"You are the Goal Summarizer for the xAI Grok Build harness."}}}}
"#;
        let meta =
            Grok::probe_agent_session_meta_with_title(Some(summary), Cursor::new(updates.as_bytes()))
                .unwrap();
        assert_eq!(
            meta.title.as_deref(),
            Some("Codex/Pi Full Alignment Goal Summarizer")
        );
        assert!(meta.updated_at.is_none(), "list meta must not prefer sidecar updated_at");
        assert_eq!(meta.session_id.as_deref(), Some("sid"));
    }

    #[test]
    fn junk_generated_title_falls_back_to_first_real_user_message() {
        let summary = crate::agent_session::SessionMeta {
            session_id: Some("sid".into()),
            title: Some("You are an **adversarial verifier** for the xAI Grok Build".into()),
            source_kind: Some("grok-v1".into()),
            ..Default::default()
        };
        let updates = r#"{"method":"session/update","params":{"sessionId":"sid","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"You are an **adversarial verifier** for the harness."}}}}
{"method":"session/update","params":{"sessionId":"sid","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"/setup-matt-pocock-skills"}}}}
"#;
        let meta =
            Grok::probe_agent_session_meta_with_title(Some(summary), Cursor::new(updates.as_bytes()))
                .unwrap();
        assert_eq!(meta.title.as_deref(), Some("/setup-matt-pocock-skills"));
    }

    #[test]
    fn preamble_only_users_keep_fallback_snippet() {
        let updates = r#"{"method":"session/update","params":{"sessionId":"sid","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"You are an adversarial verifier for the harness. Long body."}}}}
"#;
        let meta =
            Grok::probe_agent_session_meta_with_title(None, Cursor::new(updates.as_bytes()))
                .unwrap();
        assert_eq!(
            meta.title.as_deref(),
            Some("You are an adversarial verifier for the harness. Long body.")
        );
    }
}
