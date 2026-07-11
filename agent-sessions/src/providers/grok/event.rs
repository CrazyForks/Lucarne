use crate::agent_session::*;
use smol_str::SmolStr;

use super::types::{Body as GrokBody, Entry as GrokEntry};

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

fn agent_session_from_grok_body(body: &GrokBody) -> Session {
    let mut events: Vec<Event> = Vec::with_capacity(body.entries.len());
    let mut models: Vec<SessionModelMeta> = Vec::new();
    let mut title = None;
    let mut cwd = None;
    let mut created_at = None;
    let mut updated_at = None;
    let mut session_id = body.session_id.clone();

    for entry in body.entries.iter() {
        match entry {
            GrokEntry::SessionInfo {
                session_id: sid,
                cwd: c,
                title: t,
                model,
                created_at: ca,
                updated_at: ua,
            } => {
                session_id = Some(sid.clone());
                cwd = c.clone();
                title = t.clone();
                created_at = ca.clone();
                updated_at = ua.clone();
                if let Some(model) = model {
                    models.push(SessionModelMeta::zero(model.clone()));
                }
            }
            GrokEntry::UserMessage { text, timestamp } => {
                events.push(event(
                    Actor::User,
                    Body::Prompt(Prompt {
                        text: if text.is_empty() {
                            None
                        } else {
                            Some(text.clone())
                        },
                        blocks: Box::new([]),
                    }),
                    timestamp.clone(),
                ));
            }
            GrokEntry::AssistantMessage {
                text,
                timestamp,
                model,
            } => {
                events.push(event(
                    Actor::Assistant,
                    Body::Response(Response {
                        model: smol_opt(model.clone()),
                        phase: None,
                        text: if text.is_empty() {
                            None
                        } else {
                            Some(text.clone())
                        },
                        blocks: Box::new([]),
                    }),
                    timestamp.clone(),
                ));
            }
            GrokEntry::Thinking { text, timestamp } => {
                events.push(event(
                    Actor::Assistant,
                    Body::Response(Response {
                        model: None,
                        phase: Some("thinking".into()),
                        text: None,
                        blocks: Box::new([ContentBlock::Thinking(ThinkingBlock {
                            text: text.clone(),
                        })]),
                    }),
                    timestamp.clone(),
                ));
            }
            GrokEntry::ToolCall {
                id,
                name,
                input_json,
                timestamp,
            } => {
                let parsed = projection::parsed_tool_input(name.as_ref(), input_json.as_deref());
                events.push(Event {
                    id: id.clone(),
                    timestamp: timestamp.clone(),
                    actor: Actor::Assistant,
                    turn_id: None,
                    op_id: id.clone(),
                    parent_op_id: None,
                    body: Body::Operation(Operation {
                        kind: OperationKind::Custom(name.clone()),
                        phase: OperationPhase::Started,
                        name: name.clone(),
                        input_json: input_json.clone(),
                        output_json: None,
                        command: parsed.command,
                        file_path: parsed.file_path,
                        lines_added: parsed.lines_added,
                        lines_removed: parsed.lines_removed,
                        is_error: false,
                        duration_seconds: None,
                        extra_json: None,
                    }),
                });
            }
            GrokEntry::ToolResult {
                id,
                name,
                text,
                is_error,
                timestamp,
            } => {
                let tool_name = name.clone().unwrap_or_else(|| "tool".into());
                events.push(Event {
                    id: id.clone(),
                    timestamp: timestamp.clone(),
                    actor: Actor::Tool,
                    turn_id: None,
                    op_id: id.clone(),
                    parent_op_id: None,
                    body: Body::Operation(Operation {
                        kind: OperationKind::Custom(tool_name.clone()),
                        phase: if *is_error {
                            OperationPhase::Failed
                        } else {
                            OperationPhase::Completed
                        },
                        name: tool_name,
                        input_json: None,
                        output_json: if text.is_empty() {
                            None
                        } else {
                            Some(text.clone())
                        },
                        command: None,
                        file_path: None,
                        lines_added: 0,
                        lines_removed: 0,
                        is_error: *is_error,
                        duration_seconds: None,
                        extra_json: None,
                    }),
                });
            }
            GrokEntry::TurnCompleted {
                stop_reason,
                last_agent_message: _,
                timestamp,
            } => {
                if let Some(reason) = stop_reason {
                    events.push(Event {
                        id: None,
                        timestamp: timestamp.clone(),
                        actor: Actor::System,
                        turn_id: None,
                        op_id: None,
                        parent_op_id: None,
                        body: Body::State(State {
                            kind: "turn_completed".into(),
                            value_json: Some(reason.clone()),
                        }),
                    });
                }
            }
        }
    }

    Session {
        agent: AgentKind::new("grok"),
        version: VersionKind::new("grok-v1"),
        meta: SessionMeta {
            session_id: smol_opt(session_id),
            cwd,
            title,
            created_at: smol_opt(created_at),
            updated_at: smol_opt(updated_at),
            source_kind: Some("grok-v1".into()),
            models: models.into_boxed_slice(),
            ..Default::default()
        },
        events: events.into_boxed_slice(),
    }
}

fn event(actor: Actor, body: Body, timestamp: Option<SmolStr>) -> Event {
    Event {
        id: None,
        timestamp,
        actor,
        turn_id: None,
        op_id: None,
        parent_op_id: None,
        body,
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
    // Byte parse has no path; meta (title/cwd) comes from summary via discovery.
    let body = super::parse_grok_body_reader(reader, selection, None)?;
    Ok(crate::agent_session::filter_selection(
        agent_session_from_grok_body(&body),
        selection,
    ))
}

fn seed_meta_from_path(path: &std::path::Path) -> Option<GrokEntry> {
    let dir = if super::is_updates_jsonl(path) {
        path.parent()?
    } else {
        path
    };
    let summary = dir.join("summary.json");
    let meta = super::read_summary_meta(&summary).ok().flatten()?;
    super::session_info_entry_from_meta(&meta)
}

#[cfg(feature = "watch")]
fn watch_events_from_grok_entries(
    entries: &[GrokEntry],
    selection: crate::ParseSelection,
) -> Box<[crate::watch::WatchEvent]> {
    let mut events = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry {
            GrokEntry::SessionInfo { title, .. } => {
                if selection.includes_state()
                    && let Some(title) = title
                {
                    events.push(crate::watch::WatchEvent::State(crate::watch::WatchState {
                        meta: crate::watch::WatchEventMeta::default(),
                        kind: "session_title".into(),
                        value_json: Some(title.clone().into()),
                    }));
                }
            }
            GrokEntry::UserMessage { text, timestamp } => {
                events.push(crate::watch::WatchEvent::UserMessage(
                    crate::watch::WatchMessage {
                        meta: watch_meta(timestamp.clone()),
                        text: watch_text(text),
                    },
                ));
            }
            GrokEntry::AssistantMessage {
                text, timestamp, ..
            } => {
                events.push(crate::watch::WatchEvent::AssistantMessage(
                    crate::watch::WatchAssistantMessage {
                        meta: watch_meta(timestamp.clone()),
                        model: None,
                        phase: None,
                        text: watch_text(text),
                    },
                ));
            }
            GrokEntry::Thinking { text, timestamp } => {
                events.push(crate::watch::WatchEvent::AssistantMessage(
                    crate::watch::WatchAssistantMessage {
                        meta: watch_meta(timestamp.clone()),
                        model: None,
                        phase: Some("thinking".into()),
                        text: watch_text(text),
                    },
                ));
            }
            GrokEntry::ToolCall {
                id,
                name,
                input_json,
                timestamp,
            } => {
                let parsed = projection::parsed_tool_input(name.as_ref(), input_json.as_deref());
                events.push(crate::watch::WatchEvent::ToolCall(
                    crate::watch::WatchToolCall {
                        meta: watch_meta(timestamp.clone()),
                        kind: OperationKind::Custom(name.clone()),
                        phase: OperationPhase::Started,
                        call_id: id.clone().map(Into::into),
                        name: name.clone().into(),
                        input_json: input_json.clone().map(Into::into),
                        command: parsed.command.map(Into::into),
                        file_path: parsed.file_path.map(Into::into),
                        lines_added: parsed.lines_added,
                        lines_removed: parsed.lines_removed,
                    },
                ));
            }
            GrokEntry::ToolResult {
                id,
                name,
                text,
                is_error,
                timestamp,
            } => {
                let tool_name = name.clone().unwrap_or_else(|| "tool".into());
                events.push(crate::watch::WatchEvent::ToolResult(
                    crate::watch::WatchToolResult {
                        meta: watch_meta(timestamp.clone()),
                        kind: OperationKind::Custom(tool_name.clone()),
                        phase: if *is_error {
                            OperationPhase::Failed
                        } else {
                            OperationPhase::Completed
                        },
                        call_id: id.clone().map(Into::into),
                        name: tool_name.into(),
                        output_json: if text.is_empty() {
                            None
                        } else {
                            Some(text.clone().into())
                        },
                        is_error: *is_error,
                        duration_seconds: None,
                    },
                ));
            }
            GrokEntry::TurnCompleted {
                stop_reason,
                last_agent_message,
                timestamp,
            } => {
                events.push(crate::watch::WatchEvent::TurnCompleted(
                    crate::watch::WatchTurnCompleted {
                        meta: watch_meta(timestamp.clone()),
                        last_agent_message: last_agent_message.clone(),
                        duration_ms: None,
                        value_json: stop_reason.clone().map(Into::into),
                    },
                ));
            }
        }
    }
    events.into_boxed_slice()
}

#[cfg(feature = "watch")]
impl crate::watch::provider::ProviderWatchEvents for super::Grok {
    fn parse_watch_reader<R>(
        path: &std::path::Path,
        reader: R,
        selection: crate::ParseSelection,
    ) -> crate::Result<crate::watch::provider::ParsedWatchSession>
    where
        R: std::io::BufRead,
    {
        let seed = seed_meta_from_path(path);
        // Always keep seed identity even when selection omits meta entries —
        // delta windows still need session_id/cwd for core history projection.
        let seed_identity = match &seed {
            Some(GrokEntry::SessionInfo {
                session_id,
                cwd,
                title,
                ..
            }) => (
                Some(session_id.clone()),
                cwd.clone(),
                title.clone(),
            ),
            _ => (None, None, None),
        };
        let body = super::parse_grok_body_reader(reader, selection, seed)?;
        let from_entries = body.entries.iter().find_map(|e| match e {
            GrokEntry::SessionInfo {
                session_id,
                cwd,
                title,
                ..
            } => Some((Some(session_id.clone()), cwd.clone(), title.clone())),
            _ => None,
        });
        let (session_id, cwd, title) = match from_entries {
            Some((sid, cwd, title)) => (
                sid.or(seed_identity.0).or(body.session_id),
                cwd.or(seed_identity.1),
                title.or(seed_identity.2),
            ),
            None => (
                seed_identity.0.or(body.session_id),
                seed_identity.1,
                seed_identity.2,
            ),
        };
        Ok(crate::watch::provider::ParsedWatchSession {
            session_id,
            cwd,
            title,
            events: watch_events_from_grok_entries(&body.entries, selection),
        })
    }

    fn probe_watch_session_meta<R>(
        path: &std::path::Path,
        _reader: R,
    ) -> crate::Result<crate::agent_session::SessionMeta>
    where
        R: std::io::BufRead,
    {
        if let Some(GrokEntry::SessionInfo {
            session_id,
            cwd,
            title,
            model,
            created_at,
            updated_at,
        }) = seed_meta_from_path(path)
        {
            return Ok(crate::agent_session::SessionMeta {
                session_id: Some(session_id),
                cwd,
                title,
                created_at,
                updated_at,
                models: model
                    .map(|m| vec![SessionModelMeta::zero(m)].into_boxed_slice())
                    .unwrap_or_default(),
                source_kind: Some("grok-v1".into()),
                ..Default::default()
            });
        }
        Ok(crate::agent_session::SessionMeta {
            source_kind: Some("grok-v1".into()),
            ..Default::default()
        })
    }

    fn needs_watch_state_seed() -> bool {
        true
    }

    fn initial_watch_directory_depth() -> Option<usize> {
        // sessions/<encoded-cwd>/<uuid>/
        Some(3)
    }

    fn changed_watch_directory_depth() -> Option<usize> {
        Some(3)
    }

    fn includes_watch_directory(
        root: &std::path::Path,
        path: &std::path::Path,
        is_recent: bool,
    ) -> bool {
        if super::is_subagent_path(path) {
            return false;
        }
        // Session dirs hold noise trees (terminal/compaction/goal/…). Only watch
        // the layout roots and the session uuid directory itself.
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if matches!(
                name,
                "subagents"
                    | "terminal"
                    | "compaction"
                    | "compaction_checkpoints"
                    | "compaction_requests"
                    | "goal"
            ) {
                return false;
            }
        }
        let depth = path
            .strip_prefix(root)
            .map(|relative| relative.components().count())
            .unwrap_or(0);
        // 0=sessions root, 1=encoded-cwd, 2=uuid
        depth <= 2 || is_recent
    }

    fn normalize_watch_events(
        events: Box<[crate::watch::WatchEvent]>,
        state: &mut crate::watch::state::ProviderWatchState,
    ) -> Box<[crate::watch::WatchEvent]> {
        // Grok wire has no assistant "phase". Every mid-turn status chunk would
        // otherwise look like a terminal reply: synthesize promoted each one and
        // channels filled with "I'll audit…", "Writing the verdict.", "Not Refuted".
        // Mark ordinary assistant text as partial so only TurnCompleted (with
        // last_agent_message) and explicit ask_user notifies are channel-facing.
        let mut events = events.into_vec();
        for event in &mut events {
            match event {
                crate::watch::WatchEvent::UserMessage(_) => {
                    state.pending_assistant_text = None;
                }
                crate::watch::WatchEvent::AssistantMessage(message) => {
                    if message.phase.is_none()
                        && !message
                            .text
                            .as_deref()
                            .is_some_and(super::is_ask_user_notify_text)
                    {
                        message.phase = Some("partial".into());
                        if let Some(text) = message.text.clone() {
                            state.pending_assistant_text = Some(text);
                        }
                    }
                }
                crate::watch::WatchEvent::TurnCompleted(completed) => {
                    if completed.last_agent_message.is_none() {
                        completed.last_agent_message = state.pending_assistant_text.take();
                    } else {
                        state.pending_assistant_text = None;
                    }
                }
                _ => {}
            }
        }

        let events = crate::watch::provider::synthesize_task_complete_from_terminal_responses(
            events.into_boxed_slice(),
            state,
            |response| response.phase.is_none(),
        );

        // History watch: user prompts + turn completions (+ ask_user notify).
        // Drop partial/final_answer assistant rows — core already skips phase != None,
        // but filtering here keeps the bus small.
        let mut out = Vec::new();
        for event in events.into_vec() {
            match event {
                crate::watch::WatchEvent::ToolCall(call)
                    if super::is_ask_user_tool_name(call.name.as_str()) =>
                {
                    if let Some(text) =
                        super::format_ask_user_question_notify(call.input_json.as_deref())
                    {
                        out.push(crate::watch::WatchEvent::AssistantMessage(
                            crate::watch::WatchAssistantMessage {
                                meta: call.meta,
                                model: None,
                                phase: None,
                                text: Some(text.into()),
                            },
                        ));
                    }
                }
                crate::watch::WatchEvent::AssistantMessage(message)
                    if message.phase.is_none() =>
                {
                    out.push(crate::watch::WatchEvent::AssistantMessage(message));
                }
                crate::watch::WatchEvent::TurnCompleted(completed)
                    if completed.last_agent_message.is_some()
                        || completed.value_json.is_some() =>
                {
                    // Prefer body-bearing completions; empty stop_reason-only rows
                    // are useless for channels and used to ride along noise.
                    if completed.last_agent_message.is_some() {
                        out.push(crate::watch::WatchEvent::TurnCompleted(completed));
                    }
                }
                crate::watch::WatchEvent::UserMessage(_)
                | crate::watch::WatchEvent::TurnFailed(_)
                | crate::watch::WatchEvent::Attachment(_)
                | crate::watch::WatchEvent::State(_) => out.push(event),
                _ => {}
            }
        }
        out.into_boxed_slice()
    }
}
