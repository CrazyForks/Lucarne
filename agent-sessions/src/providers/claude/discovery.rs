use smol_str::SmolStr;
use std::{
    fs::File,
    io::{BufReader, Cursor},
    path::{Path, PathBuf},
};

use crate::agent::{AgentProviderSourceEntry, CandidateRole, DiscoverableProvider};
#[cfg(feature = "agent_session")]
use crate::agent_session::SessionMeta;
use crate::providers::AgentProviderSource;
use crate::{Error, Result, claude::Claude};
use tracing::trace;

impl DiscoverableProvider for Claude {
    fn name() -> &'static str {
        Claude::name()
    }

    fn default_roots() -> Vec<PathBuf> {
        let base = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| crate::paths::home_dir().map(|home| home.join(".claude")))
            .unwrap_or_else(|| PathBuf::from(".claude"));

        let desktop = if cfg!(target_os = "macos") {
            crate::paths::home_dir()
                .map(|home| {
                    home.join("Library/Application Support/Claude/local-agent-mode-sessions")
                })
                .unwrap_or_else(|| PathBuf::from("."))
        } else if cfg!(target_os = "windows") {
            crate::paths::home_dir()
                .map(|home| home.join("AppData/Roaming/Claude/local-agent-mode-sessions"))
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            crate::paths::home_dir()
                .map(|home| home.join(".config/Claude/local-agent-mode-sessions"))
                .unwrap_or_else(|| PathBuf::from("."))
        };

        vec![base.join("projects"), desktop]
    }

    fn candidate_role(_root: &std::path::Path, path: &std::path::Path) -> CandidateRole {
        if path
            .components()
            .any(|component| component.as_os_str() == "subagents")
        {
            CandidateRole::Subagent
        } else {
            CandidateRole::Primary
        }
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
            agent = Claude::name(),
            roots = roots.len(),
            "discovering Claude sessions"
        );
        let mut candidates = 0usize;

        for root in roots {
            if !root.is_dir() {
                continue;
            }

            for entry in walkdir::WalkDir::new(&root)
                .min_depth(1)
                .into_iter()
                .filter_map(std::result::Result::ok)
            {
                let path = entry.into_path();
                if !path.is_file()
                    || !is_recent(&path)
                    || path.extension().is_none_or(|ext| ext != "jsonl")
                    || !Self::includes_candidate_in_history(&root, &path)
                {
                    continue;
                }

                let file_name: SmolStr = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.to_owned().into())
                    .unwrap_or_else(|| "session.jsonl".into());
                let entry = AgentProviderSourceEntry::new(path.clone())
                    .named(file_name)
                    .with_media_type("application/jsonl");
                candidates += 1;
                emit(AgentProviderSource::new(path, vec![entry]));
            }
        }

        trace!(
            target: "agent_sessions::discovery",
            agent = Claude::name(),
            candidates,
            "discovered Claude sessions"
        );
        Ok(())
    }

    #[cfg(feature = "agent_session")]
    fn parse_candidate_entries_agent_session_meta(
        entries: &[AgentProviderSourceEntry],
    ) -> Result<SessionMeta> {
        let entry = entries.first().ok_or(Error::EmptyInput)?;
        let probed = match entry.inline_data.as_ref() {
            Some(data) => Claude::probe_session_meta_with_title(Cursor::new(data.as_ref()))?,
            None => {
                Claude::probe_session_meta_with_title(BufReader::new(File::open(&entry.path)?))?
            }
        };
        let (mut meta, title) = probed.ok_or(Error::Detection {
            agent: Claude::name(),
        })?;
        meta.title = title;
        Ok(meta)
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
        let Some(text) = claude_user_line_text(&value) else {
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
fn claude_user_line_text(value: &serde_json::Value) -> Option<String> {
    (value.get("type")?.as_str()? == "user").then_some(())?;
    let message = value.get("message")?;
    text_from_json_content(message.get("content")?)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_ignores_claude_subagent_transcripts() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("projects");
        let session_dir = root.join("-tmp-project").join("session-id");
        std::fs::create_dir_all(session_dir.join("subagents")).unwrap();
        std::fs::write(session_dir.join("main.jsonl"), "{}\n").unwrap();
        std::fs::write(session_dir.join("subagents").join("agent-a.jsonl"), "{}\n").unwrap();

        let mut sources = Vec::new();
        Claude::discover_in([root], &mut |source| sources.push(source)).unwrap();
        let paths = sources
            .into_iter()
            .map(|source| source.path().to_path_buf())
            .collect::<Vec<_>>();

        assert_eq!(paths.len(), 1);
        assert_eq!(
            paths[0].file_name().and_then(|name| name.to_str()),
            Some("main.jsonl")
        );
    }
}
