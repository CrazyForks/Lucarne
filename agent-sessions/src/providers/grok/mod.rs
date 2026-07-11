use smol_str::SmolStr;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::trace;

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
        trace!(
            target: "agent_sessions::parse",
            agent = Grok::name(),
            session_id = ?meta.session_id,
            title = ?meta.title,
            "probed Grok session meta"
        );
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
            "tool_call"
                if selection.includes_operations()
                    || (selection.includes_messages() && is_ask_user_tool_update(update)) =>
            {
                let id = update.tool_call_id.clone();
                let name = update
                    .title
                    .clone()
                    .unwrap_or_else(|| "tool".into());
                let input_json = update
                    .raw_input
                    .as_ref()
                    .map(|v| v.to_string().into());
                // Operations path keeps the normal tool timeline (flush + ToolCall).
                // Message-only ask_user notify must NOT flush assistant buffers —
                // flushing mid-turn would split preambles ("I'll ask…") into separate
                // channel notifications and fragment the real final reply.
                if selection.includes_operations() {
                    flush_message_buffers(
                        &mut entries,
                        &mut user_buf,
                        &mut assistant_buf,
                        &mut thought_buf,
                        ts.clone(),
                    );
                    if let Some(id) = &id {
                        pending_tools
                            .insert(id.clone(), (name.clone().into(), input_json.clone()));
                    }
                    entries.push(Entry::ToolCall {
                        id: id.map(Into::into),
                        name: name.clone().into(),
                        input_json: input_json.clone(),
                        timestamp: ts.clone(),
                    });
                }
                // Notify-only: surface multi-option questions as assistant text so
                // message-only history watch can deliver them without enabling the
                // dense tool stream (and without an answer reverse-RPC path).
                if selection.includes_messages() && is_ask_user_tool_name(&name) {
                    if let Some(text) = format_ask_user_question_notify(input_json.as_deref()) {
                        entries.push(Entry::AssistantMessage {
                            text: text.into(),
                            timestamp: ts,
                            model: None,
                        });
                    }
                }
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
                // Capture final assistant body before flush so watch can attach it
                // to TurnCompleted (Grok wire has no last_agent_message field).
                let last_agent_message = (!assistant_buf.is_empty())
                    .then(|| assistant_buf.as_str().into())
                    .or_else(|| {
                        entries.iter().rev().find_map(|e| match e {
                            Entry::AssistantMessage { text, .. } => Some(text.clone()),
                            _ => None,
                        })
                    });
                flush_message_buffers(
                    &mut entries,
                    &mut user_buf,
                    &mut assistant_buf,
                    &mut thought_buf,
                    ts.clone(),
                );
                entries.push(Entry::TurnCompleted {
                    stop_reason: update.stop_reason.clone().map(Into::into),
                    last_agent_message,
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
        Some(serde_json::Value::String(s)) => {
            let s = s.trim();
            if s.is_empty() {
                return None;
            }
            // Already RFC3339-ish (used by synthesize duration).
            if s.contains('T') {
                return Some(s.into());
            }
            // Numeric string (seconds or millis).
            if let Ok(n) = s.parse::<f64>() {
                return unix_number_to_rfc3339(n).map(Into::into);
            }
            Some(s.into())
        }
        Some(serde_json::Value::Number(n)) => n
            .as_f64()
            .and_then(unix_number_to_rfc3339)
            .map(Into::into),
        _ => None,
    }
}

/// Grok `updates.jsonl` top-level `timestamp` is unix seconds (sometimes ms).
/// Watch `synthesize_task_complete` requires RFC3339 `…Z` for duration.
fn unix_number_to_rfc3339(n: f64) -> Option<String> {
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    // Heuristic: ≥ 1e12 → milliseconds since epoch; else seconds (frac → ms).
    let (secs, millis) = if n >= 1_000_000_000_000.0 {
        let ms = n as i64;
        (ms.div_euclid(1000), ms.rem_euclid(1000) as u32)
    } else {
        let secs = n.floor() as i64;
        let millis = ((n - secs as f64) * 1000.0).round() as u32;
        (secs, millis.min(999))
    };
    civil_from_unix_secs(secs).map(|(y, m, d, hh, mm, ss)| {
        format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{millis:03}Z")
    })
}

/// Inverse of days_from_civil used by watch duration parsing (Howard Hinnant).
fn civil_from_unix_secs(secs: i64) -> Option<(i32, u32, u32, u32, u32, u32)> {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400) as u32;
    let hh = tod / 3600;
    let mm = (tod % 3600) / 60;
    let ss = tod % 60;

    // civil_from_days: days is days since 1970-01-01
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    Some((y as i32, m, d, hh, mm, ss))
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

/// True when this path belongs to a Grok subagent session that must not enter
/// history/watch notifications.
///
/// Grok writes subagents twice:
/// 1. Nested under `parent/subagents/<id>/` (caught by [`is_subagent_path`])
/// 2. As a full top-level session dir with `summary.json` `session_kind` of
///    `subagent` or `subagent_fork` (harness roles like Goal Plan Writer /
///    Adversarial Verifier that emit "Done" / "Not Refuted")
///
/// Path-only exclusion misses (2) and floods WeChat/Telegram.
pub(crate) fn is_subagent_session(path: &Path) -> bool {
    if is_subagent_path(path) {
        return true;
    }
    let Some(summary) = summary_path_for_session_path(path) else {
        return false;
    };
    summary_marks_subagent(&summary)
}

fn summary_path_for_session_path(path: &Path) -> Option<PathBuf> {
    if path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "summary.json")
    {
        return Some(path.to_path_buf());
    }
    if is_updates_jsonl(path) {
        return path.parent().map(|dir| dir.join("summary.json"));
    }
    // Session uuid directory (watch/discovery expand session roots).
    if path.is_dir() {
        return Some(path.join("summary.json"));
    }
    // Sidecar file inside a session dir (resources_state.json, etc.).
    path.parent().map(|dir| dir.join("summary.json"))
}

fn summary_marks_subagent(summary: &Path) -> bool {
    let Ok(bytes) = std::fs::read(summary) else {
        return false;
    };
    let Ok(raw) = serde_json::from_slice::<RawSummaryKindProbe>(&bytes) else {
        return false;
    };
    raw.session_kind
        .as_deref()
        .is_some_and(is_subagent_session_kind)
}

fn is_subagent_session_kind(kind: &str) -> bool {
    let kind = kind.trim();
    kind == "subagent" || kind == "subagent_fork" || kind.starts_with("subagent")
}

#[derive(Debug, Deserialize)]
struct RawSummaryKindProbe {
    #[serde(default)]
    session_kind: Option<String>,
}

pub(crate) fn is_updates_jsonl(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "updates.jsonl")
}

fn is_ask_user_tool_name(name: &str) -> bool {
    let trimmed = name.trim();
    trimmed.eq_ignore_ascii_case("ask_user_question")
        || trimmed.eq_ignore_ascii_case("AskUserQuestion")
}

pub(super) fn is_ask_user_notify_text(text: &str) -> bool {
    text.trim_start().starts_with("Grok is asking:")
}

fn is_ask_user_tool_update(update: &RawSessionUpdate) -> bool {
    if update
        .title
        .as_deref()
        .is_some_and(is_ask_user_tool_name)
    {
        return true;
    }
    update
        .raw_input
        .as_ref()
        .and_then(|v| v.get("questions"))
        .and_then(|q| q.as_array())
        .is_some_and(|a| !a.is_empty())
        && update
            .title
            .as_deref()
            .map(|t| t.starts_with("Ask:") || is_ask_user_tool_name(t))
            .unwrap_or(false)
}

/// Format `ask_user_question` rawInput into a channel-facing notify body.
/// Notify-only: does not create an intervention or answer path.
pub(super) fn format_ask_user_question_notify(input_json: Option<&str>) -> Option<String> {
    let raw = input_json?.trim();
    if raw.is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let questions = value.get("questions")?.as_array()?;
    if questions.is_empty() {
        return None;
    }

    let mut body = String::from("Grok is asking:");
    let multi = questions.len() > 1;
    let mut wrote_question = false;
    for (idx, question) in questions.iter().enumerate() {
        let text = question
            .get("question")
            .or_else(|| question.get("prompt"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if text.is_empty() {
            continue;
        }
        wrote_question = true;
        if multi {
            body.push_str(&format!("\n\n{}. {text}", idx + 1));
        } else {
            body.push_str(&format!("\n\n{text}"));
        }
        let Some(options) = question.get("options").and_then(|v| v.as_array()) else {
            continue;
        };
        for option in options {
            let label = option
                .get("label")
                .or_else(|| option.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if label.is_empty() {
                continue;
            }
            let description = option
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if description.is_empty() {
                body.push_str(&format!("\n• {label}"));
            } else {
                body.push_str(&format!("\n• {label} — {description}"));
            }
        }
    }
    wrote_question.then_some(body)
}

#[cfg(test)]
mod subagent_session_tests {
    use super::*;

    #[test]
    fn session_kind_probe_matches_harness_roles() {
        assert!(is_subagent_session_kind("subagent"));
        assert!(is_subagent_session_kind("subagent_fork"));
        assert!(is_subagent_session_kind("subagent_something_new"));
        assert!(!is_subagent_session_kind("primary"));
        assert!(!is_subagent_session_kind(""));
    }

    #[test]
    fn top_level_summary_kind_excludes_without_subagents_path_segment() {
        let temp = tempfile::tempdir().expect("temp");
        let session = temp.path().join("enc").join("child");
        std::fs::create_dir_all(&session).expect("dir");
        std::fs::write(
            session.join("summary.json"),
            r#"{"session_kind":"subagent_fork","parent_session_id":"parent"}"#,
        )
        .expect("summary");
        let updates = session.join("updates.jsonl");
        std::fs::write(&updates, "").expect("updates");
        assert!(is_subagent_session(&updates));
        assert!(is_subagent_session(&session));
        assert!(!is_subagent_path(&updates));
    }
}

#[cfg(test)]
mod timestamp_tests {
    use super::*;

    #[test]
    fn unix_seconds_timestamp_becomes_rfc3339_z() {
        // 2026-07-11T09:46:50Z ≈ 1783763210 (session sample order of magnitude)
        let s = timestamp_to_smol(Some(&serde_json::json!(1783763210))).expect("ts");
        assert!(
            s.ends_with('Z') && s.contains('T'),
            "expected RFC3339 Z, got {s}"
        );
        // synthesize duration parser requires fixed layout
        assert_eq!(s.len(), "2026-07-11T09:46:50.000Z".len());
    }

    #[test]
    fn unix_millis_timestamp_becomes_rfc3339_z() {
        let s = timestamp_to_smol(Some(&serde_json::json!(1783763210228_i64))).expect("ts");
        assert!(s.ends_with('Z') && s.contains('T'), "got {s}");
    }

    #[test]
    fn rfc3339_string_timestamp_preserved() {
        let s = timestamp_to_smol(Some(&serde_json::json!("2026-07-11T09:46:50.228Z")))
            .expect("ts");
        assert_eq!(s.as_str(), "2026-07-11T09:46:50.228Z");
    }
}

#[cfg(test)]
mod ask_user_notify_tests {
    use super::*;

    #[test]
    fn format_ask_user_question_notify_renders_options() {
        let input = r#"{"questions":[{"question":"Which style?","options":[{"label":"brief","description":"Short"},{"label":"detailed","description":"Long"}]}]}"#;
        let text = format_ask_user_question_notify(Some(input)).expect("formatted");
        assert!(text.contains("Grok is asking:"), "{text}");
        assert!(text.contains("Which style?"), "{text}");
        assert!(text.contains("• brief — Short"), "{text}");
        assert!(text.contains("• detailed — Long"), "{text}");
    }

    #[test]
    fn format_ask_user_question_notify_rejects_empty() {
        assert!(format_ask_user_question_notify(None).is_none());
        assert!(format_ask_user_question_notify(Some("{}")).is_none());
        assert!(format_ask_user_question_notify(Some(r#"{"questions":[]}"#)).is_none());
    }
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

    #[test]
    fn message_selection_projects_ask_user_question_as_assistant_notify() {
        let updates = r#"{"method":"session/update","params":{"sessionId":"sid","update":{"sessionUpdate":"tool_call","toolCallId":"call-1","title":"ask_user_question","rawInput":{"questions":[{"question":"Which style?","options":[{"label":"brief","description":"Short"},{"label":"detailed","description":"Long"}]}]}}}}
"#;
        let body = parse_grok_body_reader(
            Cursor::new(updates.as_bytes()),
            ParseSelection::empty().with_messages(),
            None,
        )
        .expect("parse");
        let msgs: Vec<_> = body
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::AssistantMessage { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(msgs.len(), 1, "entries={:?}", body.entries.len());
        assert!(msgs[0].contains("Which style?"), "{}", msgs[0]);
        assert!(msgs[0].contains("brief"), "{}", msgs[0]);
        assert!(
            !body.entries.iter().any(|e| matches!(e, Entry::ToolCall { .. })),
            "message-only selection must not emit ToolCall entries"
        );
    }

    #[test]
    fn message_selection_turn_completed_carries_last_agent_message() {
        let msg = serde_json::json!({
            "timestamp": 1783763210_u64,
            "method": "session/update",
            "params": {
                "sessionId": "sid",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": "final answer body" }
                }
            }
        });
        let done = serde_json::json!({
            "timestamp": 1783763210_u64,
            "method": "session/update",
            "params": {
                "sessionId": "sid",
                "update": {
                    "sessionUpdate": "turn_completed",
                    "stop_reason": "end_turn"
                }
            }
        });
        let updates = format!("{msg}\n{done}\n");
        let body = parse_grok_body_reader(
            Cursor::new(updates.as_bytes()),
            ParseSelection::empty().with_messages(),
            None,
        )
        .expect("parse");
        let last = body.entries.iter().find_map(|e| match e {
            Entry::TurnCompleted {
                last_agent_message,
                ..
            } => last_agent_message.as_deref(),
            _ => None,
        });
        assert_eq!(last, Some("final answer body"));
        // timestamps on entries must be RFC3339 for watch synthesize
        let ts = body.entries.iter().find_map(|e| match e {
            Entry::AssistantMessage { timestamp, .. } => timestamp.as_deref(),
            _ => None,
        });
        let ts = ts.expect("assistant ts");
        assert!(ts.contains('T') && ts.ends_with('Z'), "got {ts}");
    }

    #[test]
    fn message_selection_ask_user_does_not_split_assistant_preamble() {
        // Real Grok turns often emit a short assistant preamble, then ask_user_question,
        // then the final answer. Message-only watch must keep the preamble in the
        // buffer so it does not become its own channel notification.
        let updates = r#"{"method":"session/update","params":{"sessionId":"sid","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"I'll ask one question with only options A and B."}}}}
{"method":"session/update","params":{"sessionId":"sid","update":{"sessionUpdate":"tool_call","toolCallId":"call-1","title":"ask_user_question","rawInput":{"questions":[{"question":"Which?","options":[{"label":"A"},{"label":"B"}]}]}}}}
{"method":"session/update","params":{"sessionId":"sid","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":" Final answer."}}}}
{"method":"session/update","params":{"sessionId":"sid","update":{"sessionUpdate":"turn_completed","stop_reason":"end_turn"}}}
"#;
        let body = parse_grok_body_reader(
            Cursor::new(updates.as_bytes()),
            ParseSelection::empty().with_messages(),
            None,
        )
        .expect("parse");
        let msgs: Vec<String> = body
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::AssistantMessage { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect();
        // One notify for the question + one aggregated final assistant body.
        assert_eq!(msgs.len(), 2, "msgs={msgs:?}");
        assert!(msgs[0].contains("Grok is asking:"), "notify={}", msgs[0]);
        assert!(
            msgs[1].contains("I'll ask one question") && msgs[1].contains("Final answer"),
            "final must keep preamble+tail, got {}",
            msgs[1]
        );
        assert!(
            !msgs.iter().any(|m| m == "I'll ask one question with only options A and B."),
            "preamble must not be a standalone notify: {msgs:?}"
        );
    }
}
