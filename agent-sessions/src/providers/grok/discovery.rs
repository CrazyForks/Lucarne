use smol_str::SmolStr;
use std::path::{Path, PathBuf};

use crate::agent::{AgentProviderSourceEntry, CandidateRole, DiscoverableProvider};
use crate::providers::AgentProviderSource;
use crate::{Error, Result};
use tracing::trace;

use super::{Grok, is_subagent_path, is_updates_jsonl, sessions_root};

impl Grok {
    pub fn default_roots() -> Vec<PathBuf> {
        sessions_root().into_iter().collect()
    }

    pub(crate) fn discover_in<I, P>(
        roots: I,
        emit: &mut dyn FnMut(AgentProviderSource),
    ) -> Result<()>
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        let mut include_all = |_path: &Path| true;
        Self::discover_recent_in(roots, &mut include_all, emit)
    }

    pub(crate) fn discover_recent_in<I, P>(
        roots: I,
        is_recent: &mut dyn FnMut(&Path) -> bool,
        emit: &mut dyn FnMut(AgentProviderSource),
    ) -> Result<()>
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        for root in roots {
            let root = root.into();
            if !root.is_dir() {
                continue;
            }
            trace!(
                target: "agent_sessions::discovery",
                agent = Grok::name(),
                root = %root.display(),
                "discovering Grok Build sessions"
            );

            let walker = walkdir::WalkDir::new(&root)
                .follow_links(false)
                .max_depth(6)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file() && is_updates_jsonl(e.path()));

            for entry in walker {
                let path = entry.into_path();
                if !is_recent(&path)
                    || !<Self as DiscoverableProvider>::includes_candidate_in_history(&root, &path)
                {
                    continue;
                }
                let mut entries = vec![AgentProviderSourceEntry::new(path.clone())];
                if let Some(dir) = path.parent() {
                    let summary = dir.join("summary.json");
                    if summary.is_file() {
                        entries.push(
                            AgentProviderSourceEntry::new(summary)
                                .named(SmolStr::new("summary.json")),
                        );
                    }
                }
                emit(AgentProviderSource::new(path, entries));
            }
        }
        Ok(())
    }
}

impl DiscoverableProvider for Grok {
    fn name() -> &'static str {
        Grok::name()
    }

    fn default_roots() -> Vec<PathBuf> {
        Self::default_roots()
    }

    fn candidate_role(_root: &Path, path: &Path) -> CandidateRole {
        if is_subagent_path(path) {
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
        Self::discover_in(roots, emit)
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
        Self::discover_recent_in(roots, is_recent, emit)
    }

    #[cfg(feature = "agent_session")]
    fn parse_candidate_entries_agent_session_meta(
        entries: &[AgentProviderSourceEntry],
    ) -> Result<crate::agent_session::SessionMeta> {
        // Prefer explicit summary.json entry, else sibling of updates.jsonl.
        for entry in entries {
            let name = entry
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if name == "summary.json"
                && let Some(meta) = super::read_summary_meta(&entry.path)?
            {
                return Ok(meta);
            }
        }
        for entry in entries {
            if is_updates_jsonl(&entry.path)
                && let Some(dir) = entry.path.parent()
            {
                let summary = dir.join("summary.json");
                if let Some(meta) = super::read_summary_meta(&summary)? {
                    return Ok(meta);
                }
            }
        }
        // Fallback: probe first lines of updates for session id only.
        for entry in entries {
            if !is_updates_jsonl(&entry.path) {
                continue;
            }
            let file = std::fs::File::open(&entry.path).map_err(|err| {
                Error::Message(format!("failed to open {}: {err}", entry.path.display()).into())
            })?;
            let body = super::parse_grok_body_reader(
                std::io::BufReader::new(file),
                crate::ParseSelection::meta_only(),
                None,
            )?;
            if let Some(session_id) = body.session_id {
                return Ok(crate::agent_session::SessionMeta {
                    session_id: Some(session_id),
                    source_kind: Some("grok-v1".into()),
                    ..Default::default()
                });
            }
        }
        Ok(crate::agent_session::SessionMeta {
            source_kind: Some("grok-v1".into()),
            ..Default::default()
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
        grok_user_offsets(bytes, base_offset)
    }

    #[cfg(feature = "agent_session")]
    fn is_transcript_user_text_visible(text: &str) -> bool {
        !text.trim().is_empty()
    }
}

#[cfg(feature = "agent_session")]
fn grok_user_offsets(bytes: &[u8], base_offset: u64) -> Vec<u64> {
    let mut offsets = Vec::new();
    let mut local = 0usize;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        let start = base_offset.saturating_add(local as u64);
        local += line.len();
        let line = trim_ascii(line);
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        let kind = v
            .pointer("/params/update/sessionUpdate")
            .and_then(|x| x.as_str());
        if kind == Some("user_message_chunk") {
            offsets.push(start);
        }
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
