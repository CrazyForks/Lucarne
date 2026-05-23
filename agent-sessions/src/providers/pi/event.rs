use crate::agent_session::*;

use super::types::{Block, Entry as PiEntry};
use smol_str::SmolStr;

#[cfg(feature = "watch")]
fn watch_meta(timestamp: Option<SmolStr>) -> crate::watch::WatchEventMeta {
    crate::watch::WatchEventMeta {
        timestamp: timestamp.map(Into::into),
        ..crate::watch::WatchEventMeta::default()
    }
}

#[cfg(feature = "watch")]
fn watch_text(text: &str) -> Option<smol_str::SmolStr> {
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.into())
    }
}

#[cfg(feature = "watch")]
fn watch_state(kind: SmolStr, value_json: Option<SmolStr>) -> crate::watch::WatchEvent {
    crate::watch::WatchEvent::State(crate::watch::WatchState {
        meta: crate::watch::WatchEventMeta::default(),
        kind: kind.into(),
        value_json: crate::watch::watch_smol_opt(value_json),
    })
}

#[cfg(feature = "watch")]
fn pi_watch_meta(entries: &[PiEntry]) -> (Option<SmolStr>, Option<SmolStr>) {
    entries
        .iter()
        .find_map(|entry| match entry {
            PiEntry::SessionInfo {
                session_id, cwd, ..
            } => Some((Some(session_id.clone()), cwd.clone())),
            _ => None,
        })
        .unwrap_or((None, None))
}

#[cfg(feature = "watch")]
fn watch_events_from_pi_entries(
    entries: &[PiEntry],
    selection: crate::ParseSelection,
) -> Box<[crate::watch::WatchEvent]> {
    let mut events = Vec::with_capacity(entries.len());

    for entry in entries {
        match entry {
            PiEntry::SessionInfo { name, .. } => {
                if selection.includes_state()
                    && let Some(name) = name
                {
                    events.push(watch_state("session_info".into(), Some(name.clone())));
                }
            }
            PiEntry::UserMessage {
                text, timestamp, ..
            } => {
                events.push(crate::watch::WatchEvent::UserMessage(
                    crate::watch::WatchMessage {
                        meta: watch_meta(timestamp.clone()),
                        text: watch_text(text),
                    },
                ));
            }
            PiEntry::AssistantMessage {
                text,
                timestamp,
                model,
                stop_reason,
                error_message,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                total_tokens,
                ..
            } => {
                events.push(crate::watch::WatchEvent::AssistantMessage(
                    crate::watch::WatchAssistantMessage {
                        meta: watch_meta(timestamp.clone()),
                        model: crate::watch::watch_smol_opt(model.clone()),
                        phase: crate::watch::watch_smol_opt(assistant_phase(
                            stop_reason.as_deref(),
                            error_message.as_deref(),
                        )),
                        text: watch_text(text),
                    },
                ));

                if selection.includes_usage() && *total_tokens > 0 {
                    events.push(crate::watch::WatchEvent::Usage(crate::watch::WatchUsage {
                        meta: watch_meta(timestamp.clone()),
                        model: crate::watch::watch_smol_opt(model.clone()),
                        input_tokens: *input_tokens,
                        output_tokens: *output_tokens,
                        cache_creation_tokens: *cache_write_tokens,
                        cache_read_tokens: *cache_read_tokens,
                        cached_tokens: 0,
                        reasoning_tokens: 0,
                        tool_tokens: 0,
                        total_tokens: *total_tokens,
                        web_search_requests: 0,
                        speed: None,
                    }));
                }
            }
            PiEntry::ToolResult {
                tool_name,
                text,
                is_error,
            } => {
                events.push(crate::watch::WatchEvent::ToolResult(
                    crate::watch::WatchToolResult {
                        meta: crate::watch::WatchEventMeta::default(),
                        kind: OperationKind::Shell,
                        phase: if *is_error {
                            OperationPhase::Failed
                        } else {
                            OperationPhase::Completed
                        },
                        call_id: None,
                        name: tool_name.clone().into(),
                        output_json: Some(text.clone().into()),
                        is_error: *is_error,
                        duration_seconds: None,
                    },
                ));
            }
            PiEntry::ModelChange {
                provider: _,
                model_id,
            } => {
                if selection.includes_state() {
                    events.push(watch_state("model_change".into(), Some(model_id.clone())));
                }
            }
            PiEntry::ThinkingLevelChange { level } => {
                if selection.includes_state() {
                    events.push(watch_state(
                        "thinking_level_change".into(),
                        Some(level.clone()),
                    ));
                }
            }
            PiEntry::Compaction {
                summary,
                first_kept_entry_id,
                tokens_before,
            } => {
                if selection.includes_state() {
                    let value = serde_json::json!({
                        "summary": summary.as_str(),
                        "firstKeptEntryId": first_kept_entry_id.as_deref(),
                        "tokensBefore": tokens_before.unwrap_or(0),
                    });
                    events.push(watch_state(
                        "compaction".into(),
                        Some(value.to_string().into()),
                    ));
                }
            }
            PiEntry::BranchSummary { summary, from_id } => {
                if selection.includes_state() {
                    let value = serde_json::json!({
                        "summary": summary.as_str(),
                        "fromId": from_id.as_deref(),
                    });
                    events.push(watch_state(
                        "branch_summary".into(),
                        Some(value.to_string().into()),
                    ));
                }
            }
            PiEntry::Custom {
                custom_type,
                data_json,
            } => {
                if selection.includes_state() {
                    events.push(watch_state(
                        format!("custom:{}", custom_type.as_str()).into(),
                        data_json.clone(),
                    ));
                }
            }
            PiEntry::CustomMessage {
                custom_type,
                text,
                display: _,
            } => {
                if selection.includes_state()
                    && let Some(text) = text
                {
                    events.push(watch_state(
                        format!("custom_message:{}", custom_type.as_str()).into(),
                        Some(text.clone()),
                    ));
                }
            }
            PiEntry::Label { target_id, label } => {
                if selection.includes_state() {
                    let value = serde_json::json!({
                        "targetId": target_id.as_deref(),
                        "label": label.as_deref(),
                    });
                    events.push(watch_state("label".into(), Some(value.to_string().into())));
                }
            }
        }
    }

    events.into_boxed_slice()
}

fn agent_session_from_pi_body(body: &super::Body) -> Session {
    let mut events: Vec<Event> = Vec::with_capacity(body.entries.len());
    let mut models: Vec<SessionModelMeta> = Vec::new();

    for entry in body.entries.iter() {
        match entry {
            PiEntry::SessionInfo { name, .. } => {
                // Session metadata — skip event unless it has a name (rename).
                if let Some(name) = name {
                    events.push(Event {
                        actor: Actor::System,
                        body: Body::State(State {
                            kind: "session_info".into(),
                            value_json: Some(name.clone()),
                        }),
                        ..event_empty()
                    });
                }
            }
            PiEntry::UserMessage {
                text,
                blocks,
                timestamp,
            } => {
                events.push(event(
                    Actor::User,
                    Body::Prompt(Prompt {
                        text: if text.is_empty() {
                            None
                        } else {
                            Some(text.clone())
                        },
                        blocks: map_blocks(blocks),
                    }),
                    timestamp.clone(),
                ));
            }
            PiEntry::AssistantMessage {
                text,
                blocks,
                timestamp,
                model,
                stop_reason,
                error_message,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                total_tokens,
            } => {
                let phase = assistant_phase(stop_reason.as_deref(), error_message.as_deref());
                events.push(event(
                    Actor::Assistant,
                    Body::Response(Response {
                        model: smol_opt(model.clone()),
                        phase: smol_opt(phase),
                        text: if text.is_empty() {
                            None
                        } else {
                            Some(text.clone())
                        },
                        blocks: map_blocks(blocks),
                    }),
                    timestamp.clone(),
                ));

                if let Some(model_name) = model
                    && *total_tokens > 0
                {
                    models.push(SessionModelMeta {
                        model: model_name.clone().into(),
                        input_tokens: *input_tokens,
                        output_tokens: *output_tokens,
                        cache_creation_tokens: *cache_write_tokens,
                        cache_read_tokens: *cache_read_tokens,
                        cached_tokens: 0,
                        reasoning_tokens: 0,
                        tool_tokens: 0,
                        total_tokens: *total_tokens,
                        web_search_requests: 0,
                    });
                }

                if *total_tokens > 0 {
                    events.push(event(
                        Actor::System,
                        Body::Usage(Usage {
                            model: smol_opt(model.clone()),
                            input_tokens: *input_tokens,
                            output_tokens: *output_tokens,
                            cache_creation_tokens: *cache_write_tokens,
                            cache_read_tokens: *cache_read_tokens,
                            cached_tokens: 0,
                            reasoning_tokens: 0,
                            tool_tokens: 0,
                            total_tokens: *total_tokens,
                            web_search_requests: 0,
                            speed: None,
                        }),
                        timestamp.clone(),
                    ));
                }
            }
            PiEntry::ToolResult {
                tool_name,
                text,
                is_error,
            } => {
                events.push(Event {
                    actor: Actor::Tool,
                    body: Body::Operation(Operation {
                        kind: OperationKind::Shell,
                        phase: if *is_error {
                            OperationPhase::Failed
                        } else {
                            OperationPhase::Completed
                        },
                        name: tool_name.clone().into(),
                        input_json: None,
                        output_json: Some(text.clone()),
                        command: None,
                        file_path: None,
                        lines_added: 0,
                        lines_removed: 0,
                        is_error: *is_error,
                        duration_seconds: None,
                        extra_json: None,
                    }),
                    ..event_empty()
                });
            }
            PiEntry::ModelChange {
                provider: _p,
                model_id,
            } => {
                events.push(Event {
                    actor: Actor::System,
                    body: Body::State(State {
                        kind: "model_change".into(),
                        value_json: Some(model_id.clone()),
                    }),
                    ..event_empty()
                });
            }
            PiEntry::ThinkingLevelChange { level } => {
                events.push(Event {
                    actor: Actor::System,
                    body: Body::State(State {
                        kind: "thinking_level_change".into(),
                        value_json: Some(level.clone()),
                    }),
                    ..event_empty()
                });
            }
            PiEntry::Compaction {
                summary,
                first_kept_entry_id,
                tokens_before,
            } => {
                let value = serde_json::json!({
                    "summary": summary.as_str(),
                    "firstKeptEntryId": first_kept_entry_id.as_deref(),
                    "tokensBefore": tokens_before.unwrap_or(0),
                });
                events.push(Event {
                    actor: Actor::System,
                    body: Body::State(State {
                        kind: "compaction".into(),
                        value_json: Some(value.to_string().into()),
                    }),
                    ..event_empty()
                });
            }
            PiEntry::BranchSummary { summary, from_id } => {
                let value = serde_json::json!({
                    "summary": summary.as_str(),
                    "fromId": from_id.as_deref(),
                });
                events.push(Event {
                    actor: Actor::System,
                    body: Body::State(State {
                        kind: "branch_summary".into(),
                        value_json: Some(value.to_string().into()),
                    }),
                    ..event_empty()
                });
            }
            PiEntry::Custom {
                custom_type,
                data_json,
            } => {
                events.push(Event {
                    actor: Actor::System,
                    body: Body::State(State {
                        kind: format!("custom:{}", custom_type.as_str()).into(),
                        value_json: data_json.clone(),
                    }),
                    ..event_empty()
                });
            }
            PiEntry::CustomMessage {
                custom_type,
                text,
                display: _display,
            } => {
                if let Some(t) = text {
                    events.push(Event {
                        actor: Actor::System,
                        body: Body::State(State {
                            kind: format!("custom_message:{}", custom_type.as_str()).into(),
                            value_json: Some(t.clone()),
                        }),
                        ..event_empty()
                    });
                }
            }
            PiEntry::Label { target_id, label } => {
                let value = serde_json::json!({
                    "targetId": target_id.as_deref(),
                    "label": label.as_deref(),
                });
                events.push(Event {
                    actor: Actor::System,
                    body: Body::State(State {
                        kind: "label".into(),
                        value_json: Some(value.to_string().into()),
                    }),
                    ..event_empty()
                });
            }
        }
    }

    // Extract session metadata from the first SessionInfo entry.
    let session_meta = body.entries.iter().find_map(|entry| {
        if let PiEntry::SessionInfo {
            session_id,
            cwd,
            timestamp,
            name: _,
        } = entry
        {
            Some((Some(session_id.clone()), cwd.clone(), timestamp.clone()))
        } else {
            None
        }
    });
    let (session_id, cwd, created_at) = session_meta.unwrap_or((None, None, None));

    Session {
        agent: AgentKind::new("pi"),
        version: VersionKind::new("pi-v1"),
        meta: SessionMeta {
            session_id: smol_opt(session_id),
            cwd,
            created_at: smol_opt(created_at),
            source_kind: Some("pi-v1".into()),
            models: models.into_boxed_slice(),
            ..Default::default()
        },
        events: events.into_boxed_slice(),
    }
}

pub(super) fn parse_direct_agent_session_reader_selected<R>(
    reader: R,
    _metadata: crate::InputMetadata<'_>,
    selection: crate::ParseSelection,
) -> crate::Result<Session>
where
    R: std::io::BufRead,
{
    let body = super::parse_pi_body_reader(reader, selection)?;
    Ok(crate::agent_session::filter_selection(
        agent_session_from_pi_body(&body),
        selection,
    ))
}

// ── Block mapping ─────────────────────────────────────────────────

fn map_blocks(blocks: &[Block]) -> Box<[ContentBlock]> {
    blocks
        .iter()
        .map(|b| match b {
            Block::Text { text } => ContentBlock::Text(TextBlock { text: text.clone() }),
            Block::Thinking { text } => {
                ContentBlock::Thinking(ThinkingBlock { text: text.clone() })
            }
            Block::ToolCall {
                id,
                name,
                input_json,
            } => {
                let parsed = projection::parsed_tool_input(name.as_ref(), input_json.as_deref());
                ContentBlock::ToolUse(ToolUseBlock {
                    id: smol_opt(id.clone()),
                    name: name.clone().into(),
                    input_json: input_json.clone(),
                    command: parsed.command,
                    file_path: parsed.file_path,
                    lines_added: parsed.lines_added,
                    lines_removed: parsed.lines_removed,
                })
            }
            Block::ToolResult {
                tool_use_id,
                text,
                is_error,
            } => ContentBlock::ToolResult(ToolResultBlock {
                tool_use_id: smol_opt(tool_use_id.clone()),
                content: text.clone(),
                is_error: *is_error,
            }),
            Block::Image {
                data,
                mime_type: _mime_type,
            } => ContentBlock::Image(ImageBlock {
                image_url: data.clone().unwrap_or_default(),
            }),
            Block::Other { block_type } => ContentBlock::Raw(RawBlock {
                kind: block_type.clone().into(),
                raw_json: "{}".into(),
            }),
        })
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

// ── Helpers ───────────────────────────────────────────────────────

fn assistant_phase(stop_reason: Option<&str>, error_message: Option<&str>) -> Option<SmolStr> {
    match stop_reason {
        Some("stop") | Some("end_turn") | Some("stop_sequence") => None,
        Some("toolUse" | "tool_use") => Some("tool_use".into()),
        Some("error") | Some("aborted") => {
            if let Some(err) = error_message {
                Some(format!("error: {}", err).into())
            } else {
                Some("error".into())
            }
        }
        Some(other) => Some(other.into()),
        None => None,
    }
}

fn event_empty() -> Event {
    Event {
        id: None,
        timestamp: None,
        actor: Actor::System,
        turn_id: None,
        op_id: None,
        parent_op_id: None,
        body: Body::Unknown(Unknown {
            kind: "placeholder".into(),
            raw_json: "{}".into(),
        }),
    }
}

#[cfg(feature = "watch")]
impl crate::watch::provider::ProviderWatchEvents for super::Pi {
    fn parse_watch_reader<R>(
        _path: &std::path::Path,
        reader: R,
        selection: crate::ParseSelection,
    ) -> crate::Result<crate::watch::provider::ParsedWatchSession>
    where
        R: std::io::BufRead,
    {
        let body = super::parse_pi_reader(reader, selection)?;
        let (session_id, cwd) = pi_watch_meta(&body.entries);
        Ok(crate::watch::provider::ParsedWatchSession {
            session_id,
            cwd,
            title: None,
            events: watch_events_from_pi_entries(&body.entries, selection),
        })
    }

    fn probe_watch_session_meta<R>(
        _path: &std::path::Path,
        reader: R,
    ) -> crate::Result<crate::agent_session::SessionMeta>
    where
        R: std::io::BufRead,
    {
        Ok(super::Pi::probe_agent_session_meta_with_title(reader)?.unwrap_or_default())
    }

    fn needs_watch_state_seed() -> bool {
        true
    }

    fn initial_watch_directory_depth() -> Option<usize> {
        Some(3)
    }

    fn changed_watch_directory_depth() -> Option<usize> {
        Some(3)
    }

    fn normalize_watch_events(
        events: Box<[crate::watch::WatchEvent]>,
        state: &mut crate::watch::state::ProviderWatchState,
    ) -> Box<[crate::watch::WatchEvent]> {
        crate::watch::provider::synthesize_task_complete_from_terminal_responses(
            events,
            state,
            |response| response.phase.is_none(),
        )
    }
}
