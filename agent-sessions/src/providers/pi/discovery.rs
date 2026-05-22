use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::agent::{AgentProviderSourceEntry, CandidateRole, DiscoverableProvider};
use crate::providers::AgentProviderSource;
use crate::{Error, Result};
use serde::Deserialize;
use smol_str::SmolStr;

fn pi_sessions_root() -> Option<PathBuf> {
    let root = crate::paths::home_dir()?
        .join(".pi")
        .join("agent")
        .join("sessions");
    if root.is_dir() { Some(root) } else { None }
}

impl super::Pi {
    pub fn default_roots() -> Vec<PathBuf> {
        pi_sessions_root().into_iter().collect()
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
            let walker = walkdir::WalkDir::new(&root)
                .follow_links(false)
                .max_depth(8)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_type().is_file()
                        && e.path()
                            .extension()
                            .and_then(|ext| ext.to_str())
                            .is_some_and(|ext| ext == "jsonl")
                });

            for entry in walker {
                if !is_recent(entry.path())
                    || !<Self as DiscoverableProvider>::includes_candidate_in_history(
                        &root,
                        entry.path(),
                    )
                {
                    continue;
                }
                let path = entry.into_path();
                emit(AgentProviderSource::new(
                    path.clone(),
                    vec![AgentProviderSourceEntry::new(path)],
                ));
            }
        }

        Ok(())
    }
}

impl DiscoverableProvider for super::Pi {
    fn name() -> &'static str {
        super::Pi::name()
    }

    fn default_roots() -> Vec<PathBuf> {
        Self::default_roots()
    }

    fn candidate_role(root: &std::path::Path, path: &std::path::Path) -> CandidateRole {
        if is_pi_subagent_path(root, path) {
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
        for entry in entries {
            let file = std::fs::File::open(&entry.path).map_err(|err| {
                Error::Message(format!("failed to open {}: {}", entry.path.display(), err).into())
            })?;
            let reader = std::io::BufReader::new(file);
            if let Some(meta) = Self::probe_session_meta(reader)? {
                return Ok(meta);
            }
        }
        Ok(crate::agent_session::SessionMeta::default())
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
        pi_user_offsets(bytes, base_offset)
    }

    #[cfg(feature = "agent_session")]
    fn is_transcript_user_text_visible(text: &str) -> bool {
        !is_agent_instruction_preamble(text) && !is_turn_aborted_control_marker(text)
    }
}

#[cfg(feature = "agent_session")]
fn pi_user_offsets(bytes: &[u8], base_offset: u64) -> Vec<u64> {
    let mut offsets = Vec::new();
    let mut local_offset = 0usize;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        let line_start = base_offset.saturating_add(local_offset as u64);
        local_offset += line.len();
        let line = trim_ascii(line);
        if line.is_empty() {
            continue;
        }
        let Ok(raw) = serde_json::from_slice::<PiSubagentProbeLine<'_>>(line) else {
            continue;
        };
        if raw.entry_type != Some("message") {
            continue;
        }
        let Some(message) = raw.message.as_ref() else {
            continue;
        };
        if message.role != Some("user") {
            continue;
        }
        let Some(text) = message.content.as_ref().and_then(super::extract_msg_text) else {
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

fn is_pi_subagent_path(root: &Path, path: &Path) -> bool {
    is_pi_subagent_artifact_path(root, path)
        || is_pi_nested_subagent_path(root, path)
        || (path.strip_prefix(root).is_ok() && is_pi_delegated_subagent_fork_file(path))
}

fn is_pi_subagent_artifact_path(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    relative
        .components()
        .any(|component| component.as_os_str() == "subagent-artifacts")
}

fn is_pi_nested_subagent_path(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let components = relative
        .components()
        .map(|component| component.as_os_str())
        .collect::<Vec<_>>();
    if components.len() < 3 {
        return false;
    }

    let Some(parent_session) = components.get(1).and_then(|name| name.to_str()) else {
        return false;
    };
    if !root
        .join(std::path::Path::new(components[0]))
        .join(format!("{parent_session}.jsonl"))
        .is_file()
    {
        return false;
    }

    let Some(run_id) = components.get(2).and_then(|name| name.to_str()) else {
        return false;
    };
    if run_id.len() != 8 || !run_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return false;
    }

    match components.as_slice() {
        [_, _, _] => true,
        [_, _, _, run_dir, rest @ ..] => {
            let Some(run_dir) = run_dir.to_str() else {
                return false;
            };
            let Some(run_index) = run_dir.strip_prefix("run-") else {
                return false;
            };
            !run_index.is_empty()
                && run_index.bytes().all(|byte| byte.is_ascii_digit())
                && (rest.is_empty()
                    || (rest.len() == 1 && rest[0] == std::ffi::OsStr::new("session.jsonl")))
        }
        _ => false,
    }
}

fn is_pi_delegated_subagent_fork_file(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut reader = std::io::BufReader::new(file);
    let mut line = Vec::new();
    let mut session_timestamp: Option<SmolStr> = None;
    let mut has_parent_session = false;
    loop {
        line.clear();
        let Ok(bytes_read) = reader.read_until(b'\n', &mut line) else {
            return false;
        };
        if bytes_read == 0 {
            return false;
        }
        if line.iter().all(|byte| byte.is_ascii_whitespace()) {
            continue;
        }

        let Ok(raw) = serde_json::from_slice::<PiSubagentProbeLine<'_>>(&line) else {
            continue;
        };

        if raw.entry_type == Some("session") {
            has_parent_session = raw.parent_session.is_some();
            if !has_parent_session {
                return false;
            }
            session_timestamp = raw.timestamp.map(Into::into);
            continue;
        }

        if !has_parent_session {
            continue;
        }
        if raw
            .timestamp
            .zip(session_timestamp.as_deref())
            .is_some_and(|(timestamp, session_timestamp)| timestamp < session_timestamp)
        {
            continue;
        }
        if raw.entry_type != Some("message") {
            continue;
        }

        let Some(message) = raw.message.as_ref() else {
            continue;
        };
        if message.role != Some("user") {
            continue;
        }
        let Some(text) = message.content.as_ref().and_then(super::extract_msg_text) else {
            continue;
        };
        return is_pi_delegated_subagent_user_text(&text);
    }
}

fn is_pi_delegated_subagent_user_text(text: &str) -> bool {
    text.trim_start().starts_with(
        "Task: You are a delegated subagent running from a fork of the parent session.",
    )
}

#[derive(Deserialize)]
struct PiSubagentProbeLine<'a> {
    #[serde(rename = "type", default)]
    entry_type: Option<&'a str>,
    #[serde(default)]
    timestamp: Option<&'a str>,
    #[serde(rename = "parentSession", default)]
    parent_session: Option<&'a str>,
    #[serde(default)]
    message: Option<PiSubagentProbeMessage<'a>>,
}

#[derive(Deserialize)]
struct PiSubagentProbeMessage<'a> {
    #[serde(default)]
    role: Option<&'a str>,
    #[serde(default)]
    content: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use crate::DiscoverableProvider;
    use crate::Pi;

    #[test]
    fn discovery_ignores_pi_subagent_child_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("sessions");
        let project = root.join("--tmp-project--");
        std::fs::create_dir_all(&project).unwrap();

        let primary = project.join("2026-05-12T01-00-00-000Z_parent.jsonl");
        std::fs::write(&primary, "{}\n").unwrap();

        let child = project
            .join("2026-05-12T01-00-00-000Z_parent")
            .join("4ccc2ec1")
            .join("run-2")
            .join("session.jsonl");
        std::fs::create_dir_all(child.parent().unwrap()).unwrap();
        std::fs::write(&child, "{}\n").unwrap();

        let mut sources = Vec::new();
        Pi::discover_in([root], &mut |source| sources.push(source)).unwrap();
        let paths = sources
            .into_iter()
            .map(|source| source.path().to_path_buf())
            .collect::<Vec<_>>();

        assert_eq!(paths, vec![primary]);
    }

    #[test]
    fn pi_subagent_artifact_metadata_is_not_history() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("sessions");
        let project = root.join("--tmp-project--");
        let artifacts = project.join("subagent-artifacts");
        std::fs::create_dir_all(&artifacts).unwrap();

        let meta = artifacts.join("a498a8e3_context-builder_1_meta.json");
        std::fs::write(&meta, "{\n  \"runId\": \"a498a8e3\"\n}\n").unwrap();

        assert!(!Pi::includes_candidate_in_history(&root, &meta));
    }

    #[test]
    fn discovery_keeps_pi_parent_session_forks_as_history() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("sessions");
        let project = root.join("--tmp-project--");
        std::fs::create_dir_all(&project).unwrap();

        let primary = project.join("2026-05-10T11-59-22-505Z_parent.jsonl");
        std::fs::write(&primary, "{}\n").unwrap();

        let fork = project.join("2026-05-11T00-49-47-936Z_child.jsonl");
        std::fs::write(
            &fork,
            r#"{"type":"session","version":3,"id":"child","timestamp":"2026-05-11T00:49:47.936Z","cwd":"/tmp/project","parentSession":"/tmp/parent.jsonl"}"#,
        )
        .unwrap();

        let mut sources = Vec::new();
        Pi::discover_in([root], &mut |source| sources.push(source)).unwrap();
        let paths = sources
            .into_iter()
            .map(|source| source.path().to_path_buf())
            .collect::<Vec<_>>();

        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&primary));
        assert!(paths.contains(&fork));
    }

    #[test]
    fn discovery_ignores_pi_delegated_subagent_fork_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("sessions");
        let project = root.join("--tmp-project--");
        std::fs::create_dir_all(&project).unwrap();

        let primary = project.join("2026-05-10T11-59-22-505Z_parent.jsonl");
        std::fs::write(&primary, "{}\n").unwrap();

        let subagent = project.join("2026-05-11T00-49-47-936Z_child.jsonl");
        std::fs::write(
            &subagent,
            [
                r#"{"type":"session","version":3,"id":"child","timestamp":"2026-05-11T00:49:47.936Z","cwd":"/tmp/project","parentSession":"/tmp/parent.jsonl"}"#,
                r#"{"type":"message","id":"old","timestamp":"2026-05-10T11:59:25.404Z","message":{"role":"user","content":[{"type":"text","text":"parent title should not leak"}]}}"#,
                r#"{"type":"message","id":"new","timestamp":"2026-05-11T00:49:51.117Z","message":{"role":"user","content":[{"type":"text","text":"Task: You are a delegated subagent running from a fork of the parent session. Treat the inherited conversation as reference-only context, not a live thread to continue.\n\nTask:\nImplement the child task."}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let mut sources = Vec::new();
        Pi::discover_in([root], &mut |source| sources.push(source)).unwrap();
        let paths = sources
            .into_iter()
            .map(|source| source.path().to_path_buf())
            .collect::<Vec<_>>();

        assert_eq!(paths, vec![primary]);
    }

    #[test]
    fn delegated_subagent_probe_scans_to_late_marker() {
        let temp = tempfile::tempdir().unwrap();
        let session = temp.path().join("late-marker.jsonl");
        let mut lines = vec![
            r#"{"type":"session","version":3,"id":"child","timestamp":"2026-05-11T00:49:47.936Z","cwd":"/tmp/project","parentSession":"/tmp/parent.jsonl"}"#.to_string(),
        ];
        lines.extend((0..300).map(|idx| {
            format!(
                r#"{{"type":"message","id":"filler-{idx}","timestamp":"2026-05-11T00:49:48.000Z","message":{{"role":"assistant","content":[{{"type":"text","text":"filler"}}]}}}}"#
            )
        }));
        lines.push(
            r#"{"type":"message","id":"late","timestamp":"2026-05-11T00:49:51.117Z","message":{"role":"user","content":[{"type":"text","text":"Task: You are a delegated subagent running from a fork of the parent session. Treat the inherited conversation as reference-only context, not a live thread to continue."}]}}"#.to_string(),
        );
        std::fs::write(&session, lines.join("\n")).unwrap();

        assert!(
            super::is_pi_delegated_subagent_fork_file(&session),
            "delegated marker after inherited filler must still classify the file as a subagent",
        );
    }
}
