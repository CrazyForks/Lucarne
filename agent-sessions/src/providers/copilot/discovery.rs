use std::{
    borrow::Cow,
    fs::File,
    io::{BufRead, BufReader, Cursor},
    path::{Path, PathBuf},
};

use smol_str::SmolStr;

use super::{CLI_EVENTS_FILE, WORKSPACE_FILE};
use crate::agent::{AgentProviderSourceEntry, DiscoverableProvider};
#[cfg(feature = "agent_session")]
use crate::agent_session::{SessionMeta, SessionModelMeta};
use crate::providers::AgentProviderSource;
use crate::{Error, Result, copilot::Copilot};
use serde::Deserialize;
use tracing::trace;

const COPILOT_HOME_ENV: &str = "COPILOT_HOME";
const DEFAULT_COPILOT_HOME: &str = ".copilot";
const SESSION_STATE_DIR: &str = "session-state";
impl DiscoverableProvider for Copilot {
    fn name() -> &'static str {
        Copilot::name()
    }

    fn default_roots() -> Vec<PathBuf> {
        vec![copilot_session_state_dir(&copilot_home())]
    }

    fn discover_in<I, P>(roots: I, emit: &mut dyn FnMut(AgentProviderSource)) -> Result<()>
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        let mut include_all = |_path: &Path| true;
        Self::discover_recent_in(roots, &mut include_all, emit)
    }

    fn discover_recent_in<I, P>(
        roots: I,
        is_recent: &mut dyn FnMut(&Path) -> bool,
        emit: &mut dyn FnMut(AgentProviderSource),
    ) -> Result<()>
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        let roots = roots.into_iter().map(Into::into).collect::<Vec<_>>();
        trace!(
            target: "agent_sessions::discovery",
            agent = Copilot::name(),
            roots = roots.len(),
            "discovering Copilot sessions"
        );
        let mut candidates = 0usize;

        for root in roots {
            if !root.is_dir() {
                continue;
            }

            let read_dir = std::fs::read_dir(&root)?;

            for entry in read_dir.flatten() {
                let session_dir = entry.path();
                if !session_dir.is_dir() {
                    continue;
                }

                let events_path = copilot_events_path(&session_dir);
                if !events_path.is_file() || !is_recent(&events_path) {
                    continue;
                }

                let mut entries = vec![
                    AgentProviderSourceEntry::new(events_path.clone())
                        .named(CLI_EVENTS_FILE)
                        .with_media_type("application/jsonl"),
                ];

                let workspace = copilot_workspace_path(&session_dir);
                if workspace.is_file() {
                    entries.push(
                        AgentProviderSourceEntry::new(workspace)
                            .named(WORKSPACE_FILE)
                            .with_media_type("application/yaml"),
                    );
                }

                candidates += 1;
                emit(AgentProviderSource::new(events_path, entries));
            }
        }

        trace!(
            target: "agent_sessions::discovery",
            agent = Copilot::name(),
            candidates,
            "discovered Copilot sessions"
        );
        Ok(())
    }

    #[cfg(feature = "agent_session")]
    fn parse_candidate_entries_agent_session_meta(
        entries: &[AgentProviderSourceEntry],
    ) -> Result<SessionMeta> {
        let workspace = entries
            .iter()
            .find(|entry| is_workspace_entry(entry))
            .map(read_workspace_meta)
            .transpose()?;
        let events = entries
            .iter()
            .find(|entry| is_cli_events_entry(entry))
            .or_else(|| entries.iter().find(|entry| !is_workspace_entry(entry)))
            .ok_or(Error::EmptyInput)?;
        let needs_event_title = workspace
            .as_ref()
            .and_then(|workspace| workspace.summary.as_ref())
            .is_none();
        let event = match events.inline_data.as_ref() {
            Some(data) => probe_cli_meta(Cursor::new(data.as_ref()), needs_event_title)?,
            None => probe_cli_meta(BufReader::new(File::open(&events.path)?), needs_event_title)?,
        };
        let models = event
            .selected_model
            .as_deref()
            .map(SessionModelMeta::zero)
            .into_iter()
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Ok(SessionMeta {
            session_id: crate::agent_session::smol_opt(event.session_id.or_else(|| {
                workspace
                    .as_ref()
                    .and_then(|workspace| workspace.id.clone())
            })),
            cwd: workspace
                .as_ref()
                .and_then(|workspace| workspace.cwd.clone()),
            title: workspace
                .as_ref()
                .and_then(|workspace| workspace.summary.clone())
                .or(event.title),
            models,
            created_at: crate::agent_session::smol_opt(
                workspace
                    .as_ref()
                    .and_then(|workspace| workspace.created_at.clone()),
            ),
            updated_at: crate::agent_session::smol_opt(event.timestamp.or_else(|| {
                workspace
                    .as_ref()
                    .and_then(|workspace| workspace.updated_at.clone())
            })),
            source_kind: Some("cli-events-v1".into()),
            ..SessionMeta::default()
        })
    }

    #[cfg(feature = "agent_session")]
    fn parse_direct_agent_session_reader_selected<R>(
        reader: &mut R,
        metadata: crate::InputMetadata<'_>,
        selection: crate::ParseSelection,
    ) -> Result<Option<crate::agent_session::Session>>
    where
        R: std::io::BufRead,
    {
        super::event::parse_direct_agent_session_reader_selected(reader, metadata, selection)
            .map(Some)
    }

    #[cfg(feature = "agent_session")]
    fn visible_transcript_user_offsets(bytes: &[u8], base_offset: u64) -> Vec<u64> {
        jsonl_user_offsets(bytes, base_offset)
    }

    #[cfg(feature = "agent_session")]
    fn is_transcript_user_text_visible(text: &str) -> bool {
        !is_agent_instruction_preamble(text) && !is_turn_aborted_control_marker(text)
    }
}

#[cfg(feature = "agent_session")]
fn jsonl_user_offsets(bytes: &[u8], base_offset: u64) -> Vec<u64> {
    let mut offsets = Vec::new();
    let mut local_offset = 0usize;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        let line_start = base_offset.saturating_add(local_offset as u64);
        local_offset += line.len();
        let line = trim_ascii(line);
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|kind| kind.as_str()) != Some("user.message") {
            continue;
        }
        let Some(text) = value
            .get("data")
            .and_then(|data| data.get("content"))
            .and_then(|content| content.as_str())
            .filter(|text| !text.trim().is_empty())
        else {
            continue;
        };
        if is_agent_instruction_preamble(text) || is_turn_aborted_control_marker(text) {
            continue;
        }
        offsets.push(line_start);
    }
    offsets
}

#[cfg(feature = "agent_session")]
fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while let Some((first, rest)) = bytes.split_first() {
        if !first.is_ascii_whitespace() {
            break;
        }
        bytes = rest;
    }
    while let Some((last, rest)) = bytes.split_last() {
        if !last.is_ascii_whitespace() {
            break;
        }
        bytes = rest;
    }
    bytes
}

#[cfg(feature = "agent_session")]
fn is_agent_instruction_preamble(text: &str) -> bool {
    let trimmed = text.trim_start();
    let first = trimmed.lines().next().unwrap_or("").trim();
    first.starts_with("# AGENTS.md instructions")
        || trimmed.starts_with("<INSTRUCTIONS>")
        || trimmed.contains("\n<INSTRUCTIONS>")
}

#[cfg(feature = "agent_session")]
fn is_turn_aborted_control_marker(text: &str) -> bool {
    text.trim_start().starts_with("<turn_aborted>")
}

#[cfg(feature = "agent_session")]
fn is_workspace_entry(entry: &AgentProviderSourceEntry) -> bool {
    entry.kind.as_deref() == Some("workspace")
        || entry
            .name
            .as_deref()
            .is_some_and(|name| name.ends_with(WORKSPACE_FILE))
        || entry
            .path
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new(WORKSPACE_FILE))
}

#[cfg(feature = "agent_session")]
fn is_cli_events_entry(entry: &AgentProviderSourceEntry) -> bool {
    entry.kind.as_deref() == Some("events")
        || entry.media_type.as_deref() == Some("application/jsonl")
        || entry
            .name
            .as_deref()
            .is_some_and(|name| name.ends_with(CLI_EVENTS_FILE))
        || entry
            .path
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new(CLI_EVENTS_FILE))
}

#[cfg(feature = "agent_session")]
fn read_workspace_meta(entry: &AgentProviderSourceEntry) -> Result<RawWorkspaceMeta> {
    match entry.inline_data.as_ref() {
        Some(data) => Ok(serde_yaml::from_slice(data.as_ref())?),
        None => Ok(serde_yaml::from_reader(File::open(&entry.path)?)?),
    }
}

#[cfg(feature = "agent_session")]
fn probe_cli_meta<R>(mut reader: R, want_title: bool) -> Result<RawCliProbe>
where
    R: BufRead,
{
    let mut line = Vec::new();
    let mut probe = RawCliProbe::default();
    loop {
        line.clear();
        let bytes_read = reader.read_until(b'\n', &mut line)?;
        if bytes_read == 0 {
            return Ok(probe);
        }
        if line.iter().all(|byte| byte.is_ascii_whitespace()) {
            continue;
        }
        let event: RawCliEvent<'_> = match serde_json::from_slice(&line) {
            Ok(event) => event,
            Err(_err) if probe.session_id.is_some() => return Ok(probe),
            Err(err) => return Err(err.into()),
        };
        match event.kind {
            "session.start" => {
                probe.session_id = event.data.session_id.map(Cow::into_owned).map(Into::into);
                probe.selected_model = event
                    .data
                    .selected_model
                    .map(Cow::into_owned)
                    .map(Into::into);
                probe.timestamp = event.timestamp;
                if !want_title {
                    return Ok(probe);
                }
            }
            "user.message" if probe.session_id.is_some() && probe.title.is_none() => {
                if let Some(content) = event.data.content.as_deref() {
                    let title = first_line_snippet(content, 80);
                    if !title.is_empty() {
                        probe.title = Some(title.into());
                    }
                }
            }
            _ => {}
        }
        if !want_title || (probe.session_id.is_some() && probe.title.is_some()) {
            return Ok(probe);
        }
    }
}

#[cfg(feature = "agent_session")]
#[derive(Deserialize)]
struct RawWorkspaceMeta {
    #[serde(default)]
    id: Option<SmolStr>,
    #[serde(default)]
    cwd: Option<SmolStr>,
    #[serde(default)]
    summary: Option<SmolStr>,
    #[serde(default)]
    created_at: Option<SmolStr>,
    #[serde(default)]
    updated_at: Option<SmolStr>,
}

#[cfg(feature = "agent_session")]
#[derive(Deserialize)]
struct RawCliEvent<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(default)]
    timestamp: Option<SmolStr>,
    #[serde(default, borrow)]
    data: RawCliEventData<'a>,
}

#[cfg(feature = "agent_session")]
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RawCliEventData<'a> {
    #[serde(default)]
    session_id: Option<Cow<'a, str>>,
    #[serde(default)]
    selected_model: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    content: Option<Cow<'a, str>>,
}

#[cfg(feature = "agent_session")]
#[derive(Default)]
struct RawCliProbe {
    session_id: Option<SmolStr>,
    selected_model: Option<SmolStr>,
    timestamp: Option<SmolStr>,
    title: Option<SmolStr>,
}

#[cfg(feature = "agent_session")]
fn first_line_snippet(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or(text).trim();
    if line.chars().count() <= max {
        return line.to_string();
    }
    let mut snippet = line.chars().take(max.saturating_sub(1)).collect::<String>();
    snippet.push('…');
    snippet
}

fn copilot_home() -> PathBuf {
    std::env::var_os(COPILOT_HOME_ENV)
        .map(PathBuf::from)
        .or_else(|| crate::paths::home_dir().map(|home| home.join(DEFAULT_COPILOT_HOME)))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_COPILOT_HOME))
}

fn copilot_session_state_dir(home: &Path) -> PathBuf {
    home.join(SESSION_STATE_DIR)
}

fn copilot_events_path(session_dir: &Path) -> PathBuf {
    session_dir.join(CLI_EVENTS_FILE)
}

fn copilot_workspace_path(session_dir: &Path) -> PathBuf {
    session_dir.join(WORKSPACE_FILE)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    use super::*;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn restore_env(key: &str, value: Option<OsString>) {
        unsafe {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    fn default_roots_uses_copilot_home_with_default_session_state_dir() {
        let _guard = env_lock().lock().unwrap();
        let previous_home = std::env::var_os("COPILOT_HOME");
        let copilot_home = std::env::temp_dir().join("agent-sessions-copilot-home");

        unsafe {
            std::env::set_var("COPILOT_HOME", &copilot_home);
        }

        assert_eq!(
            Copilot::default_roots(),
            vec![copilot_home.join("session-state")]
        );

        restore_env("COPILOT_HOME", previous_home);
    }

    #[cfg(feature = "agent_session")]
    #[test]
    fn parse_candidate_meta_keeps_workspace_summary_title() {
        let events = AgentProviderSourceEntry::new(PathBuf::from("events.jsonl"))
            .named(CLI_EVENTS_FILE)
            .with_media_type("application/jsonl")
            .with_inline_data(
                "{\"type\":\"session.start\",\"timestamp\":\"2026-05-11T00:49:47.936Z\",\"data\":{\"sessionId\":\"copilot-fork\",\"selectedModel\":\"gpt-5\"}}\n",
            );
        let workspace = AgentProviderSourceEntry::new(PathBuf::from("workspace.yaml"))
            .named(WORKSPACE_FILE)
            .with_media_type("application/yaml")
            .with_inline_data(
                "cwd: /work/fork\nsummary: |\n  Task: You are a delegated subagent running from a fork of the parent session. Treat the inherited conversation as reference-only context, not a live thread to continue.\n  \n  Task:\n  Implement the Copilot title parser.\ncreated_at: 2026-05-11T00:49:47.936Z\nupdated_at: 2026-05-11T00:49:51.117Z\n",
            );

        let meta =
            <Copilot as DiscoverableProvider>::parse_candidate_entries_agent_session_meta(&[
                events, workspace,
            ])
            .unwrap();

        assert_eq!(meta.session_id.as_deref(), Some("copilot-fork"));
        assert_eq!(
            meta.title.as_deref(),
            Some(
                "Task: You are a delegated subagent running from a fork of the parent session. Treat the inherited conversation as reference-only context, not a live thread to continue.\n\nTask:\nImplement the Copilot title parser.\n"
            )
        );
    }

    #[cfg(feature = "agent_session")]
    #[test]
    fn parse_candidate_meta_uses_first_user_message_when_workspace_summary_missing() {
        let events = AgentProviderSourceEntry::new(PathBuf::from("events.jsonl"))
            .named(CLI_EVENTS_FILE)
            .with_media_type("application/jsonl")
            .with_inline_data(
                "{\"type\":\"session.start\",\"timestamp\":\"2026-05-11T00:49:47.936Z\",\"data\":{\"sessionId\":\"copilot-title\",\"selectedModel\":\"gpt-5\"}}\n\
                 {\"type\":\"user.message\",\"timestamp\":\"2026-05-11T00:49:48.936Z\",\"data\":{\"messageId\":\"u1\",\"content\":\"copilot title prompt\\nsecond line\"}}\n",
            );

        let meta =
            <Copilot as DiscoverableProvider>::parse_candidate_entries_agent_session_meta(&[
                events,
            ])
            .unwrap();

        assert_eq!(meta.session_id.as_deref(), Some("copilot-title"));
        assert_eq!(meta.title.as_deref(), Some("copilot title prompt"));
    }

    #[cfg(feature = "agent_session")]
    #[test]
    fn probe_cli_meta_scans_to_late_session_start_and_title() {
        let mut lines = Vec::new();
        lines.extend((0..300).map(|idx| {
            format!(
                "{{\"type\":\"user.message\",\"timestamp\":\"2026-05-11T00:{idx:02}:00.000Z\",\"data\":{{\"content\":\"pre-start title must not leak\"}}}}"
            )
        }));
        lines.push(
            "{\"type\":\"session.start\",\"timestamp\":\"2026-05-11T00:49:47.936Z\",\"data\":{\"sessionId\":\"copilot-late-start\",\"selectedModel\":\"gpt-5\"}}".to_string(),
        );
        lines.push(
            "{\"type\":\"user.message\",\"timestamp\":\"2026-05-11T00:49:48.936Z\",\"data\":{\"content\":\"late Copilot title\"}}".to_string(),
        );
        lines.push("not-json-after-title".into());

        let probe = super::probe_cli_meta(std::io::Cursor::new(lines.join("\n")), true)
            .expect("poison after title must not be read");

        assert_eq!(probe.session_id.as_deref(), Some("copilot-late-start"));
        assert_eq!(probe.title.as_deref(), Some("late Copilot title"));
    }
}
