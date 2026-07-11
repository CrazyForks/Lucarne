use smol_str::SmolStr;
use std::path::{Path, PathBuf};

use crate::agent::{AgentProviderSourceEntry, CandidateRole, DiscoverableProvider};
use crate::providers::AgentProviderSource;
use crate::{Error, Result};
use tracing::trace;

use super::{Grok, is_subagent_session, is_updates_jsonl, sessions_root};

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
        if is_subagent_session(path) {
            CandidateRole::Subagent
        } else {
            CandidateRole::Primary
        }
    }

    fn includes_candidate_in_history(_root: &Path, path: &Path) -> bool {
        // Grok sessions are multi-file: summary.json, events.jsonl,
        // resources_state.json, signals.json, rewind_points.jsonl, … live next
        // to updates.jsonl. Common watch uses a coarse is_session_like_path
        // (any *.json / *.jsonl); without this gate those sidecars are queued
        // as session files and can thrash/kill the notify kqueue loop.
        // Also drop top-level subagent/subagent_fork sessions (summary.session_kind).
        if is_subagent_session(path) {
            return false;
        }
        if path.is_dir() {
            // Allow directory expansion to discover updates.jsonl.
            return true;
        }
        is_updates_jsonl(path)
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
        // Peer pattern (Codex/Pi): header meta + bounded transcript title probe.
        // Grok header is summary.json; transcript is updates.jsonl.
        let mut summary_meta = None;
        let mut updates_path = None;

        for entry in entries {
            let name = entry
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if name == "summary.json" {
                if let Some(meta) = super::read_summary_meta(&entry.path)? {
                    summary_meta = Some(meta);
                }
            } else if is_updates_jsonl(&entry.path) {
                updates_path = Some(entry.path.clone());
                if summary_meta.is_none()
                    && let Some(dir) = entry.path.parent()
                {
                    let summary = dir.join("summary.json");
                    if let Some(meta) = super::read_summary_meta(&summary)? {
                        summary_meta = Some(meta);
                    }
                }
            }
        }

        if let Some(path) = updates_path {
            let file = std::fs::File::open(&path).map_err(|err| {
                Error::Message(format!("failed to open {}: {err}", path.display()).into())
            })?;
            let mut meta = Grok::probe_agent_session_meta_with_title(
                summary_meta,
                std::io::BufReader::new(file),
            )?;
            // If summary lacked session id, recover from updates (meta_only).
            if meta.session_id.is_none() {
                let file = std::fs::File::open(&path).map_err(|err| {
                    Error::Message(format!("failed to open {}: {err}", path.display()).into())
                })?;
                if let Ok(body) = super::parse_grok_body_reader(
                    std::io::BufReader::new(file),
                    crate::ParseSelection::meta_only(),
                    None,
                ) {
                    if let Some(session_id) = body.session_id {
                        meta.session_id = Some(session_id);
                    }
                }
            }
            return Ok(meta);
        }

        if let Some(mut meta) = summary_meta {
            // No transcript entry — still clear sidecar-skewed updated_at.
            meta.updated_at = None;
            if meta
                .title
                .as_deref()
                .is_some_and(super::is_grok_instruction_preamble)
            {
                // Keep junk title only as last resort when there is no updates file.
            } else if let Some(title) = meta.title.take() {
                meta.title = Some(super::first_line_snippet(title.as_str(), 80).into());
            }
            return Ok(meta);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn history_includes_updates_jsonl_and_session_dirs_only() {
        let root = PathBuf::from("/tmp/grok-home/sessions");
        let session_dir = root.join("enc-cwd").join("uuid-1");
        let updates = session_dir.join("updates.jsonl");
        let summary = session_dir.join("summary.json");
        let resources = session_dir.join("resources_state.json");
        let signals = session_dir.join("signals.json");
        let rewind = session_dir.join("rewind_points.jsonl");
        let events = session_dir.join("events.jsonl");
        let subagent_updates = session_dir
            .join("subagents")
            .join("child")
            .join("updates.jsonl");

        assert!(Grok::includes_candidate_in_history(&root, &updates));
        // Directory existence is path-based; is_dir() needs real dirs. For pure
        // path policy, non-updates files must be excluded regardless.
        assert!(!Grok::includes_candidate_in_history(&root, &summary));
        assert!(!Grok::includes_candidate_in_history(&root, &resources));
        assert!(!Grok::includes_candidate_in_history(&root, &signals));
        assert!(!Grok::includes_candidate_in_history(&root, &rewind));
        assert!(!Grok::includes_candidate_in_history(&root, &events));
        assert!(!Grok::includes_candidate_in_history(&root, &subagent_updates));
    }

    #[test]
    fn history_includes_existing_session_directory_for_expansion() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("sessions");
        let session_dir = root.join("enc").join("uuid");
        std::fs::create_dir_all(&session_dir).expect("session dir");
        assert!(Grok::includes_candidate_in_history(&root, &session_dir));
        assert!(!Grok::includes_candidate_in_history(
            &root,
            &session_dir.join("resources_state.json")
        ));
        assert!(Grok::includes_candidate_in_history(
            &root,
            &session_dir.join("updates.jsonl")
        ));
    }

    /// Grok materializes subagents as *top-level* session dirs (full
    /// updates.jsonl + summary.json) *and* under parent/subagents/. Path-only
    /// `subagents/` exclusion misses the top-level copy, so history watch
    /// notifies channels with harness noise ("Done", "Not Refuted").
    #[test]
    fn history_excludes_top_level_subagent_session_marked_in_summary() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("sessions");
        let primary = root.join("enc").join("primary-uuid");
        let subagent = root.join("enc").join("subagent-uuid");
        let subagent_fork = root.join("enc").join("fork-uuid");
        for dir in [&primary, &subagent, &subagent_fork] {
            std::fs::create_dir_all(dir).expect("session dir");
            std::fs::write(dir.join("updates.jsonl"), "{}\n").expect("updates");
        }
        std::fs::write(
            primary.join("summary.json"),
            r#"{"info":{"id":"primary-uuid","cwd":"/tmp"},"generated_title":"real work"}"#,
        )
        .expect("primary summary");
        std::fs::write(
            subagent.join("summary.json"),
            r#"{"info":{"id":"subagent-uuid","cwd":"/tmp"},"session_kind":"subagent","generated_title":"Adversarial Verifier"}"#,
        )
        .expect("subagent summary");
        std::fs::write(
            subagent_fork.join("summary.json"),
            r#"{"info":{"id":"fork-uuid","cwd":"/tmp"},"session_kind":"subagent_fork","parent_session_id":"primary-uuid","generated_title":"Goal Plan Writer"}"#,
        )
        .expect("fork summary");

        assert!(
            Grok::includes_candidate_in_history(&root, &primary.join("updates.jsonl")),
            "primary session must stay in history"
        );
        assert!(
            Grok::includes_candidate_in_history(&root, &primary),
            "primary session dir must stay expandable"
        );
        assert!(
            !Grok::includes_candidate_in_history(&root, &subagent.join("updates.jsonl")),
            "top-level session_kind=subagent must not enter history/watch"
        );
        assert!(
            !Grok::includes_candidate_in_history(&root, &subagent),
            "top-level subagent session dir must not expand into history"
        );
        assert!(
            !Grok::includes_candidate_in_history(&root, &subagent_fork.join("updates.jsonl")),
            "top-level session_kind=subagent_fork must not enter history/watch"
        );
        assert_eq!(
            Grok::candidate_role(&root, &subagent.join("updates.jsonl")),
            CandidateRole::Subagent
        );
        assert_eq!(
            Grok::candidate_role(&root, &subagent_fork.join("updates.jsonl")),
            CandidateRole::Subagent
        );
        assert_eq!(
            Grok::candidate_role(&root, &primary.join("updates.jsonl")),
            CandidateRole::Primary
        );
    }
}
