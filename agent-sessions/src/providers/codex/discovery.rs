use smol_str::SmolStr;
use std::{
    fs::File,
    io::{BufReader, Cursor},
    path::{Path, PathBuf},
};

use crate::agent::{AgentProviderSourceEntry, DiscoverableProvider};
#[cfg(feature = "agent_session")]
use crate::agent_session::{SessionMeta, SessionModelMeta};
use crate::providers::AgentProviderSource;
use crate::{Error, Result, codex::Codex};
use tracing::trace;

impl DiscoverableProvider for Codex {
    fn name() -> &'static str {
        Codex::name()
    }

    fn default_roots() -> Vec<PathBuf> {
        let root = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| crate::paths::home_dir().map(|home| home.join(".codex")))
            .unwrap_or_else(|| PathBuf::from(".codex"));
        vec![root.join("sessions")]
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
            agent = Codex::name(),
            roots = roots.len(),
            "discovering Codex sessions"
        );
        let mut candidates = 0usize;

        for root in roots {
            if !root.is_dir() {
                continue;
            }

            for entry in walkdir::WalkDir::new(&root)
                .min_depth(4)
                .max_depth(4)
                .into_iter()
                .filter_map(std::result::Result::ok)
            {
                let path = entry.into_path();
                if !path.is_file() || !is_recent(&path) {
                    continue;
                }
                let Some(file_name) = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| SmolStr::from(name))
                else {
                    continue;
                };
                if !file_name.starts_with("rollout-") || !file_name.ends_with(".jsonl") {
                    continue;
                }

                let entry = AgentProviderSourceEntry::new(path.clone())
                    .named(file_name)
                    .with_media_type("application/jsonl");
                candidates += 1;
                emit(AgentProviderSource::new(path, vec![entry]));
            }
        }

        trace!(
            target: "agent_sessions::discovery",
            agent = Codex::name(),
            candidates,
            "discovered Codex sessions"
        );
        Ok(())
    }

    #[cfg(feature = "agent_session")]
    fn parse_candidate_entries_agent_session_meta(
        entries: &[AgentProviderSourceEntry],
    ) -> Result<SessionMeta> {
        let entry = entries.first().ok_or(Error::EmptyInput)?;
        let probed = match entry.inline_data.as_ref() {
            Some(data) => Codex::probe_session_meta_with_title(Cursor::new(data.as_ref()))?,
            None => Codex::probe_session_meta_with_title(BufReader::new(File::open(&entry.path)?))?,
        }
        .ok_or(Error::Detection {
            agent: Codex::name(),
        })?;
        let (meta, title) = probed;
        let models = meta
            .model
            .as_deref()
            .map(SessionModelMeta::zero)
            .into_iter()
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Ok(SessionMeta {
            session_id: crate::agent_session::smol_opt(meta.session_id),
            cwd: meta.cwd,
            title,
            models,
            created_at: crate::agent_session::smol_opt(meta.timestamp),
            source_kind: Some("v1".into()),
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
        jsonl_user_offsets(bytes, base_offset, codex_user_line_text)
    }

    #[cfg(feature = "agent_session")]
    fn is_transcript_user_text_visible(text: &str) -> bool {
        !is_agent_instruction_preamble(text) && !is_turn_aborted_control_marker(text)
    }
}

#[cfg(feature = "agent_session")]
fn jsonl_user_offsets(
    bytes: &[u8],
    base_offset: u64,
    line_text: fn(&serde_json::Value) -> Option<String>,
) -> Vec<u64> {
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
        let Some(text) = line_text(&value) else {
            continue;
        };
        if is_agent_instruction_preamble(&text) || is_turn_aborted_control_marker(&text) {
            continue;
        }
        offsets.push(line_start);
    }
    offsets
}

#[cfg(feature = "agent_session")]
fn codex_user_line_text(value: &serde_json::Value) -> Option<String> {
    (value.get("type")?.as_str()? == "response_item").then_some(())?;
    let payload = value.get("payload")?;
    (payload.get("type")?.as_str()? == "message").then_some(())?;
    (payload.get("role")?.as_str()? == "user").then_some(())?;
    text_from_json_content(payload.get("content")?)
}

#[cfg(feature = "agent_session")]
fn text_from_json_content(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.to_string()),
        serde_json::Value::Array(items) => items.iter().find_map(text_from_json_content),
        serde_json::Value::Object(map) => map
            .get("text")
            .or_else(|| map.get("content"))
            .or_else(|| map.get("value"))
            .and_then(text_from_json_content),
        _ => None,
    }
    .filter(|text| !text.trim().is_empty())
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
