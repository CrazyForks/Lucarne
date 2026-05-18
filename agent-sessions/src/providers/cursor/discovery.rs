use smol_str::SmolStr;
#[cfg(feature = "agent_session")]
use std::fs::File;
#[cfg(feature = "agent_session")]
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::agent::{AgentProviderSourceEntry, CandidateRole, DiscoverableProvider};
#[cfg(feature = "agent_session")]
use crate::agent_session::SessionMeta;
use crate::providers::AgentProviderSource;
use crate::{Error, Result, cursor::Cursor};
#[cfg(feature = "agent_session")]
use serde::Deserialize;
#[cfg(feature = "agent_session")]
use serde_json::value::RawValue;
use tracing::trace;

impl DiscoverableProvider for Cursor {
    fn name() -> &'static str {
        Cursor::name()
    }

    fn default_roots() -> Vec<PathBuf> {
        let root = crate::paths::home_dir()
            .map(|home| home.join(".cursor").join("projects"))
            .unwrap_or_else(|| PathBuf::from(".cursor/projects"));
        vec![root]
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
            agent = Cursor::name(),
            roots = roots.len(),
            "discovering Cursor sessions"
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
                if !path.is_file()
                    || !is_recent(&path)
                    || path.extension().is_none_or(|ext| ext != "jsonl")
                    || !Self::includes_candidate_in_history(&root, &path)
                {
                    continue;
                }

                let is_transcript = path
                    .parent()
                    .and_then(|parent| parent.parent())
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name == "agent-transcripts");
                if !is_transcript {
                    continue;
                }

                let file_name: SmolStr = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.to_owned().into())
                    .unwrap_or_else(|| "transcript.jsonl".into());
                let entry = AgentProviderSourceEntry::new(path.clone())
                    .named(file_name)
                    .with_media_type("application/jsonl");
                candidates += 1;
                emit(AgentProviderSource::new(path, vec![entry]));
            }
        }

        trace!(
            target: "agent_sessions::discovery",
            agent = Cursor::name(),
            candidates,
            "discovered Cursor sessions"
        );
        Ok(())
    }

    #[cfg(feature = "agent_session")]
    fn parse_candidate_entries_agent_session_meta(
        entries: &[AgentProviderSourceEntry],
    ) -> Result<SessionMeta> {
        let entry = entries.first().ok_or(Error::EmptyInput)?;
        let session_id: Option<SmolStr> = entry.name.as_deref().and_then(|name| {
            std::path::Path::new(name)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(Into::into)
        });
        let title = read_cursor_title(entry)?;
        Ok(SessionMeta {
            session_id: crate::agent_session::smol_opt(session_id),
            title,
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
}

#[cfg(feature = "agent_session")]
fn read_cursor_title(entry: &AgentProviderSourceEntry) -> Result<Option<SmolStr>> {
    match entry.inline_data.as_ref() {
        Some(data) => probe_cursor_title(std::io::Cursor::new(data.as_ref())),
        None => {
            let Ok(file) = File::open(&entry.path) else {
                return Ok(None);
            };
            probe_cursor_title(BufReader::new(file))
        }
    }
}

#[cfg(feature = "agent_session")]
fn probe_cursor_title<R>(mut reader: R) -> Result<Option<SmolStr>>
where
    R: BufRead,
{
    let mut line = Vec::new();
    loop {
        line.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut line)
            .map_err(|err| Error::Message(err.to_string().into()))?;
        if bytes_read == 0 {
            return Ok(None);
        }
        if line.iter().all(|byte| byte.is_ascii_whitespace()) {
            continue;
        }

        let raw: RawCursorTitleEntry<'_> = match serde_json::from_slice(&line) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        if raw.role != Some("user") {
            continue;
        }
        let Some(text) = raw
            .message
            .and_then(|message| cursor_title_text(message.content))
        else {
            continue;
        };
        let title = first_line_snippet(&text, 80);
        if title.is_empty() || is_cursor_instruction_preamble(&text) {
            continue;
        }
        return Ok(Some(title.into()));
    }
}

#[cfg(feature = "agent_session")]
#[derive(Deserialize)]
struct RawCursorTitleEntry<'a> {
    #[serde(default)]
    role: Option<&'a str>,
    #[serde(default)]
    message: Option<RawCursorTitleMessage<'a>>,
}

#[cfg(feature = "agent_session")]
#[derive(Deserialize)]
struct RawCursorTitleMessage<'a> {
    #[serde(borrow)]
    content: &'a RawValue,
}

#[cfg(feature = "agent_session")]
fn cursor_title_text(content: &RawValue) -> Option<String> {
    let blocks = super::map_cursor_content(content).ok()?;
    blocks.into_iter().find_map(|block| match block {
        super::ContentBlock::Text(text) => Some(text.text.into()),
        _ => None,
    })
}

#[cfg(feature = "agent_session")]
fn first_line_snippet(text: &str, max: usize) -> String {
    let first = text.lines().next().unwrap_or(text).trim();
    if first.chars().count() > max {
        let mut iter = first.chars();
        let truncated: String = iter.by_ref().take(max).collect();
        format!("{truncated}…")
    } else {
        first.to_string()
    }
}

#[cfg(feature = "agent_session")]
fn is_cursor_instruction_preamble(text: &str) -> bool {
    let trimmed = text.trim_start();
    let first = trimmed.lines().next().unwrap_or("").trim();
    first.starts_with("# AGENTS.md instructions")
        || trimmed.starts_with("<INSTRUCTIONS>")
        || trimmed.contains("\n<INSTRUCTIONS>")
        || trimmed.starts_with("<permissions instructions>")
}

#[cfg(all(test, feature = "agent_session"))]
mod tests {
    use std::path::PathBuf;

    use crate::agent::{AgentProviderSourceEntry, DiscoverableProvider};
    use crate::cursor::Cursor;

    #[test]
    fn parse_candidate_meta_keeps_delegated_user_text_title() {
        let entry = AgentProviderSourceEntry::new(PathBuf::from("cursor-session.jsonl"))
            .named("cursor-session.jsonl")
            .with_media_type("application/jsonl")
            .with_inline_data(
                "{\"role\":\"user\",\"timestamp\":\"2026-05-11T00:49:51.117Z\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Task: You are a delegated subagent running from a fork of the parent session. Treat the inherited conversation as reference-only context, not a live thread to continue.\\n\\nTask:\\nImplement the Cursor title parser.\"}]}}\n",
            );

        let meta =
            <Cursor as DiscoverableProvider>::parse_candidate_entries_agent_session_meta(&[entry])
                .unwrap();

        assert_eq!(meta.session_id.as_deref(), Some("cursor-session"));
        assert_eq!(
            meta.title.as_deref(),
            Some(
                "Task: You are a delegated subagent running from a fork of the parent session. Tr…"
            )
        );
    }

    #[test]
    fn parse_candidate_meta_scans_to_late_user_title() {
        let mut lines = (0..70)
            .map(|idx| {
                serde_json::json!({
                    "role": "assistant",
                    "timestamp": format!("2026-05-11T00:{idx:02}:00.000Z"),
                    "message": {"content": [{"type": "text", "text": "noise"}]}
                })
                .to_string()
            })
            .collect::<Vec<_>>();
        lines.push(
            r#"{"role":"user","timestamp":"2026-05-11T00:49:51.117Z","message":{"content":[{"type":"text","text":"late Cursor title"}]}}"#
                .to_string(),
        );
        let entry = AgentProviderSourceEntry::new(PathBuf::from("cursor-session.jsonl"))
            .named("cursor-session.jsonl")
            .with_media_type("application/jsonl")
            .with_inline_data(lines.join("\n"));

        let meta =
            <Cursor as DiscoverableProvider>::parse_candidate_entries_agent_session_meta(&[entry])
                .unwrap();

        assert_eq!(meta.session_id.as_deref(), Some("cursor-session"));
        assert_eq!(meta.title.as_deref(), Some("late Cursor title"));
    }

    #[test]
    fn discovery_ignores_cursor_subagent_transcripts() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("projects");
        let transcript_dir = root.join("project").join("agent-transcripts");
        let primary = transcript_dir.join("main").join("primary.jsonl");
        let subagent = transcript_dir.join("subagents").join("child.jsonl");
        std::fs::create_dir_all(primary.parent().unwrap()).unwrap();
        std::fs::create_dir_all(subagent.parent().unwrap()).unwrap();
        std::fs::write(&primary, "{}\n").unwrap();
        std::fs::write(&subagent, "{}\n").unwrap();

        let mut sources = Vec::new();
        <Cursor as DiscoverableProvider>::discover_in([root], &mut |source| sources.push(source))
            .unwrap();
        let paths = sources
            .into_iter()
            .map(|source| source.path().to_path_buf())
            .collect::<Vec<_>>();

        assert_eq!(paths, vec![primary]);
    }
}
