use smol_str::SmolStr;

use std::path::Path;

use agent_sessions::{
    agent_session::{AgentKind, SessionMeta, VersionKind},
    reader::SessionReader,
    AgentProviderDescriptor, ParseSelection,
};

use super::HistoryTranscriptError;

pub(super) const HISTORY_TAIL_CURSOR_PREFIX: &str = "history-before-byte:";

pub(super) struct ProviderTranscript {
    session: agent_sessions::agent_session::Session,
    cursor_state: TranscriptCursorState,
}

impl ProviderTranscript {
    pub(super) fn session(&self) -> &agent_sessions::agent_session::Session {
        &self.session
    }

    pub(super) fn older_cursor_for_visible_start(&self, visible_start: usize) -> Option<SmolStr> {
        match &self.cursor_state {
            TranscriptCursorState::None => None,
            TranscriptCursorState::ByteWindow {
                window_start,
                has_older_bytes,
                visible_user_offsets,
            } => {
                if visible_start > 0 {
                    visible_user_offsets
                        .get(visible_start)
                        .map(|offset| format!("{HISTORY_TAIL_CURSOR_PREFIX}{offset}").into())
                } else if *has_older_bytes {
                    Some(format!("{HISTORY_TAIL_CURSOR_PREFIX}{window_start}").into())
                } else {
                    None
                }
            }
        }
    }
}

enum TranscriptCursorState {
    None,
    ByteWindow {
        window_start: u64,
        has_older_bytes: bool,
        visible_user_offsets: Vec<u64>,
    },
}

pub(super) fn parse_bounded_jsonl_transcript_window(
    provider: AgentProviderDescriptor,
    path: &Path,
    cursor: Option<&str>,
    selection: ParseSelection,
    visible_limit: usize,
) -> Result<ProviderTranscript, HistoryTranscriptError> {
    let window = read_jsonl_window(provider, path, cursor, visible_limit)?;
    if window.bytes.is_empty() {
        return Ok(ProviderTranscript {
            session: agent_sessions::agent_session::Session {
                agent: AgentKind::new(provider.id()),
                version: VersionKind::new("empty"),
                meta: SessionMeta::default(),
                events: Box::new([]),
            },
            cursor_state: TranscriptCursorState::None,
        });
    }

    let visible_user_offsets =
        provider.visible_transcript_user_offsets(&window.bytes, window.start);
    let session = provider
        .parse_agent_session_bytes(window.bytes, selection)
        .map_err(|err| HistoryTranscriptError::Parse {
            path: path.to_path_buf(),
            message: err.to_string(),
        })?;
    Ok(ProviderTranscript {
        session,
        cursor_state: TranscriptCursorState::ByteWindow {
            window_start: window.start,
            has_older_bytes: window.has_older_bytes,
            visible_user_offsets,
        },
    })
}

struct JsonlWindow {
    bytes: Vec<u8>,
    start: u64,
    has_older_bytes: bool,
}

fn read_jsonl_window(
    provider: AgentProviderDescriptor,
    path: &Path,
    cursor: Option<&str>,
    visible_limit: usize,
) -> Result<JsonlWindow, HistoryTranscriptError> {
    let end = cursor.map(decode_byte_cursor).transpose()?;
    let reader = SessionReader::open(path).map_err(|err| read_error(path, err))?;
    let mut lines = match end {
        Some(end) => reader
            .reverse_lines_before(end)
            .map_err(|err| read_error(path, err))?,
        None => reader
            .reverse_lines()
            .map_err(|err| read_error(path, err))?,
    };
    let target_visible_users = visible_limit.saturating_add(1).max(1);
    let mut visible_users = 0usize;
    let mut reversed_lines = Vec::new();
    while let Some(line) = lines
        .next_line_with_start()
        .map_err(|err| read_error(path, err))?
    {
        visible_users = visible_users.saturating_add(
            provider
                .visible_transcript_user_offsets(&line.bytes, line.start)
                .len(),
        );
        reversed_lines.push(line);
        if visible_users >= target_visible_users {
            break;
        }
    }

    let Some(start) = reversed_lines.last().map(|line| line.start) else {
        return Ok(JsonlWindow {
            bytes: Vec::new(),
            start: 0,
            has_older_bytes: false,
        });
    };
    let mut bytes = Vec::new();
    for line in reversed_lines.iter().rev() {
        bytes.extend_from_slice(&line.bytes);
        bytes.push(b'\n');
    }

    Ok(JsonlWindow {
        bytes,
        start,
        has_older_bytes: start > 0,
    })
}

fn decode_byte_cursor(cursor: &str) -> Result<u64, HistoryTranscriptError> {
    let Some(byte_offset) = cursor.strip_prefix(HISTORY_TAIL_CURSOR_PREFIX) else {
        return Err(HistoryTranscriptError::InvalidCursor {
            cursor: cursor.to_string(),
            message: format!("missing {HISTORY_TAIL_CURSOR_PREFIX} prefix"),
        });
    };
    byte_offset
        .parse::<u64>()
        .map_err(|err| HistoryTranscriptError::InvalidCursor {
            cursor: cursor.to_string(),
            message: err.to_string(),
        })
}

fn read_error(path: &Path, err: std::io::Error) -> HistoryTranscriptError {
    HistoryTranscriptError::Read {
        path: path.to_path_buf(),
        message: err.to_string(),
    }
}
