use crate::agent_session::{projection::parsed_tool_input, *};
use smol_str::SmolStr;

fn map_user_blocks(parts: &[super::UserContentPart]) -> Box<[ContentBlock]> {
    parts
        .iter()
        .map(|part| match part {
            super::UserContentPart::Text(part) => ContentBlock::Text(TextBlock {
                text: part.text.clone(),
            }),
            super::UserContentPart::Raw(part) => ContentBlock::Raw(RawBlock {
                kind: "raw".into(),
                raw_json: part.raw_json.clone(),
            }),
        })
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

fn map_response_blocks(message: &super::GeminiMessage) -> Box<[ContentBlock]> {
    let mut blocks = Vec::new();

    for thought in &message.thoughts {
        if !thought.description.is_empty() {
            blocks.push(ContentBlock::Thinking(ThinkingBlock {
                text: thought.description.clone(),
            }));
        }
    }

    if let Some(content) = &message.content
        && !content.is_empty()
    {
        blocks.push(ContentBlock::Text(TextBlock {
            text: content.clone(),
        }));
    }

    blocks.into_boxed_slice()
}

fn tool_kind(tool: &super::ToolCall) -> OperationKind {
    if tool.is_shell() {
        OperationKind::Shell
    } else if tool.name.as_str() == "google_search" {
        OperationKind::Web
    } else if tool.name.to_ascii_lowercase().contains("search") {
        OperationKind::Search
    } else {
        OperationKind::Custom(tool.name.clone().into())
    }
}

fn tool_phase(tool: &super::ToolCall) -> OperationPhase {
    if matches!(
        tool.status.as_deref(),
        Some("error" | "failed" | "cancelled")
    ) {
        OperationPhase::Failed
    } else if tool.is_shell() && tool.timestamp.is_some() {
        OperationPhase::Completed
    } else if tool.responses.is_empty() {
        OperationPhase::Requested
    } else {
        OperationPhase::Completed
    }
}

fn tool_output_json(responses: &[super::ToolResponse]) -> Option<SmolStr> {
    if responses.is_empty() {
        return None;
    }

    Some(
        serde_json::json!(
            responses
                .iter()
                .map(|response| match response {
                    super::ToolResponse::Output(output) => serde_json::json!({
                        "kind": "output",
                        "id": output.id,
                        "name": output.name,
                        "output": output.output,
                    }),
                    super::ToolResponse::Error(error) => serde_json::json!({
                        "kind": "error",
                        "id": error.id,
                        "name": error.name,
                        "error": error.error,
                    }),
                    super::ToolResponse::Raw(raw) => serde_json::json!({
                        "kind": "raw",
                        "raw_json": raw.raw_json,
                    }),
                })
                .collect::<Vec<_>>()
        )
        .to_string()
        .into(),
    )
}

fn usage_event(message: &super::GeminiMessage) -> Option<Event> {
    let tokens = message.tokens.as_ref()?;
    Some(event(
        Actor::Assistant,
        Body::Usage(Usage {
            model: smol_opt(message.model.clone()),
            input_tokens: tokens.input,
            output_tokens: tokens.output,
            cache_creation_tokens: 0,
            cache_read_tokens: tokens.cached,
            cached_tokens: tokens.cached,
            reasoning_tokens: tokens.thoughts,
            tool_tokens: tokens.tool,
            total_tokens: tokens.total,
            web_search_requests: 0,
            speed: None,
        }),
        message.timestamp.clone(),
    ))
}

#[cfg(feature = "watch")]
fn gemini_watch_meta(
    id: Option<SmolStr>,
    timestamp: Option<SmolStr>,
    op_id: Option<SmolStr>,
) -> crate::watch::WatchEventMeta {
    crate::watch::WatchEventMeta {
        id: id.map(Into::into),
        timestamp: timestamp.map(Into::into),
        op_id: op_id.map(Into::into),
        ..crate::watch::WatchEventMeta::default()
    }
}

#[cfg(feature = "watch")]
fn gemini_watch_text(text: Option<SmolStr>) -> Option<smol_str::SmolStr> {
    text.as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(Into::into)
}

#[cfg(feature = "watch")]
fn gemini_watch_usage(
    meta: crate::watch::WatchEventMeta,
    message: &super::GeminiMessage,
) -> Option<crate::watch::WatchEvent> {
    let tokens = message.tokens.as_ref()?;
    Some(crate::watch::WatchEvent::Usage(crate::watch::WatchUsage {
        meta,
        model: crate::watch::watch_smol_opt(message.model.clone()),
        input_tokens: tokens.input,
        output_tokens: tokens.output,
        cache_creation_tokens: 0,
        cache_read_tokens: tokens.cached,
        cached_tokens: tokens.cached,
        reasoning_tokens: tokens.thoughts,
        tool_tokens: tokens.tool,
        total_tokens: tokens.total,
        web_search_requests: 0,
        speed: None,
    }))
}

#[cfg(feature = "watch")]
fn gemini_watch_tool_result(
    meta: crate::watch::WatchEventMeta,
    tool: &super::ToolCall,
    phase: OperationPhase,
    timestamp_output: Option<SmolStr>,
) -> crate::watch::WatchEvent {
    crate::watch::WatchEvent::ToolResult(crate::watch::WatchToolResult {
        meta,
        kind: tool_kind(tool),
        phase,
        call_id: None,
        name: tool.name.clone().into(),
        output_json: crate::watch::watch_smol_opt(timestamp_output),
        is_error: matches!(tool.status.as_deref(), Some("error" | "failed")),
        duration_seconds: None,
    })
}

#[cfg(feature = "watch")]
fn watch_events_from_gemini_body(body: &super::Body) -> Box<[crate::watch::WatchEvent]> {
    let mut events = Vec::new();

    for entry in &body.entries {
        match entry {
            super::Entry::User(message) => {
                let blocks = map_user_blocks(&message.content);
                events.push(crate::watch::WatchEvent::UserMessage(
                    crate::watch::WatchMessage {
                        meta: gemini_watch_meta(
                            message.id.clone(),
                            message.timestamp.clone(),
                            None,
                        ),
                        text: gemini_watch_text(text_from_blocks(&blocks)),
                    },
                ));
            }
            super::Entry::Gemini(message) => {
                let blocks = map_response_blocks(message);
                events.push(crate::watch::WatchEvent::AssistantMessage(
                    crate::watch::WatchAssistantMessage {
                        meta: gemini_watch_meta(
                            message.id.clone(),
                            message.timestamp.clone(),
                            None,
                        ),
                        model: crate::watch::watch_smol_opt(message.model.clone()),
                        phase: None,
                        text: gemini_watch_text(text_from_blocks(&blocks)),
                    },
                ));

                if let Some(usage) = gemini_watch_usage(
                    gemini_watch_meta(message.id.clone(), message.timestamp.clone(), None),
                    message,
                ) {
                    events.push(usage);
                }

                for (index, tool) in message.tool_calls.iter().enumerate() {
                    let op_id = tool.id.clone().or_else(|| {
                        message
                            .id
                            .as_deref()
                            .map(|message_id| format!("{message_id}:tool:{index}").into())
                    });
                    if tool.is_shell()
                        && let (Some(start), Some(end)) =
                            (message.timestamp.clone(), tool.timestamp.clone())
                        && start != end
                    {
                        events.push(crate::watch::WatchEvent::ToolResult(
                            crate::watch::WatchToolResult {
                                meta: gemini_watch_meta(None, Some(start), op_id.clone()),
                                kind: tool_kind(tool),
                                phase: OperationPhase::Started,
                                call_id: None,
                                name: tool.name.clone().into(),
                                output_json: None,
                                is_error: false,
                                duration_seconds: None,
                            },
                        ));
                    }
                    events.push(gemini_watch_tool_result(
                        gemini_watch_meta(
                            None,
                            tool.timestamp.clone().or_else(|| message.timestamp.clone()),
                            op_id,
                        ),
                        tool,
                        tool_phase(tool),
                        tool_output_json(&tool.responses),
                    ));
                }
            }
            super::Entry::Info(info) => {
                events.push(crate::watch::WatchEvent::State(crate::watch::WatchState {
                    meta: gemini_watch_meta(info.id.clone(), info.timestamp.clone(), None),
                    kind: "info".into(),
                    value_json: Some(
                        serde_json::json!({ "content": info.content })
                            .to_string()
                            .into(),
                    ),
                }));
            }
            super::Entry::Unknown(unknown) => {
                events.push(crate::watch::WatchEvent::Unknown(
                    crate::watch::WatchUnknown {
                        meta: gemini_watch_meta(None, unknown.timestamp.clone(), None),
                        actor: Actor::System,
                        kind: unknown.kind.clone().into(),
                        raw_json: unknown.raw_json.clone().into(),
                    },
                ));
            }
        }
    }

    events.into_boxed_slice()
}

fn agent_session_from_gemini_body(body: &super::Body) -> Session {
    let mut events = Vec::new();

    for entry in &body.entries {
        match entry {
            super::Entry::User(message) => {
                let blocks = map_user_blocks(&message.content);
                let mut ev = event(
                    Actor::User,
                    Body::Prompt(Prompt {
                        text: text_from_blocks(&blocks),
                        blocks,
                    }),
                    message.timestamp.clone(),
                );
                ev.id = smol_opt(message.id.clone());
                events.push(ev);
            }
            super::Entry::Gemini(message) => {
                let blocks = map_response_blocks(message);
                let mut response = event(
                    Actor::Assistant,
                    Body::Response(Response {
                        model: smol_opt(message.model.clone()),
                        phase: None,
                        text: text_from_blocks(&blocks),
                        blocks,
                    }),
                    message.timestamp.clone(),
                );
                response.id = smol_opt(message.id.clone());
                events.push(response);

                if let Some(usage) = usage_event(message) {
                    events.push(usage);
                }

                for (index, tool) in message.tool_calls.iter().enumerate() {
                    let parsed = parsed_tool_input(tool.name.as_ref(), tool.args_json.as_deref());
                    let op_id = tool.id.clone().or_else(|| {
                        message
                            .id
                            .as_deref()
                            .map(|message_id| format!("{message_id}:tool:{index}").into())
                    });
                    if tool.is_shell()
                        && let (Some(start), Some(end)) =
                            (message.timestamp.clone(), tool.timestamp.clone())
                        && start != end
                    {
                        let mut start_event = event(
                            Actor::Tool,
                            Body::Operation(Operation {
                                kind: tool_kind(tool),
                                phase: OperationPhase::Started,
                                name: tool.name.clone().into(),
                                input_json: tool.args_json.clone(),
                                output_json: None,
                                command: tool.shell_command().map(crate::util::cow_to_box),
                                file_path: parsed.file_path.clone(),
                                lines_added: parsed.lines_added,
                                lines_removed: parsed.lines_removed,
                                is_error: false,
                                duration_seconds: None,
                                extra_json: tool.result_display_json.clone(),
                            }),
                            Some(start),
                        );
                        start_event.op_id = smol_opt(op_id.clone());
                        events.push(start_event);
                    }
                    let mut ev = event(
                        Actor::Tool,
                        Body::Operation(Operation {
                            kind: tool_kind(tool),
                            phase: tool_phase(tool),
                            name: tool.name.clone().into(),
                            input_json: tool.args_json.clone(),
                            output_json: tool_output_json(&tool.responses),
                            command: tool.shell_command().map(crate::util::cow_to_box),
                            file_path: parsed.file_path,
                            lines_added: parsed.lines_added,
                            lines_removed: parsed.lines_removed,
                            is_error: matches!(tool.status.as_deref(), Some("error" | "failed")),
                            duration_seconds: None,
                            extra_json: tool.result_display_json.clone(),
                        }),
                        tool.timestamp.clone().or_else(|| message.timestamp.clone()),
                    );
                    ev.op_id = smol_opt(op_id);
                    events.push(ev);
                }
            }
            super::Entry::Info(info) => {
                let mut ev = event(
                    Actor::System,
                    Body::State(State {
                        kind: "info".into(),
                        value_json: Some(
                            serde_json::json!({ "content": info.content })
                                .to_string()
                                .into(),
                        ),
                    }),
                    info.timestamp.clone(),
                );
                ev.id = smol_opt(info.id.clone());
                events.push(ev);
            }
            super::Entry::Unknown(unknown) => {
                events.push(event(
                    Actor::System,
                    Body::Unknown(Unknown {
                        kind: unknown.kind.clone().into(),
                        raw_json: unknown.raw_json.clone(),
                    }),
                    unknown.timestamp.clone(),
                ));
            }
        }
    }

    let events = events.into_boxed_slice();
    Session {
        agent: AgentKind::new("gemini"),
        version: VersionKind::new("gemini-v1"),
        meta: SessionMeta {
            session_id: Some(body.session_id.clone().into()),
            cwd: body.cwd.clone(),
            title: body.summary.clone(),
            models: summarize_models(&events),
            created_at: smol_opt(
                body.start_time
                    .clone()
                    .or_else(|| earliest_timestamp(&events)),
            ),
            updated_at: smol_opt(
                body.last_updated
                    .clone()
                    .or_else(|| latest_timestamp(&events)),
            ),
            source_kind: smol_opt(body.kind.clone()),
            extra_json: body.directories_json.clone(),
            ..SessionMeta::default()
        },
        events,
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
    let body = super::parse_gemini_body_reader(reader, None, selection)?;
    Ok(crate::agent_session::filter_selection(
        agent_session_from_gemini_body(&body),
        selection,
    ))
}

#[cfg(feature = "watch")]
impl crate::watch::provider::ProviderWatchEvents for crate::Gemini {
    fn parse_watch_reader<R>(
        _path: &std::path::Path,
        reader: R,
        selection: crate::ParseSelection,
    ) -> crate::Result<crate::watch::provider::ParsedWatchSession>
    where
        R: std::io::BufRead,
    {
        let body = super::parse_gemini_reader(reader, None, selection)?;
        Ok(crate::watch::provider::ParsedWatchSession {
            session_id: Some(body.session_id.clone()),
            cwd: body.cwd.clone(),
            title: None,
            events: watch_events_from_gemini_body(&body),
        })
    }

    fn probe_watch_session_meta<R>(
        _path: &std::path::Path,
        reader: R,
    ) -> crate::Result<crate::agent_session::SessionMeta>
    where
        R: std::io::BufRead,
    {
        let body = super::parse_gemini_reader(reader, None, crate::ParseSelection::meta_only())?;
        Ok(crate::agent_session::SessionMeta {
            session_id: Some(body.session_id),
            cwd: body.cwd,
            ..crate::agent_session::SessionMeta::default()
        })
    }

    fn supports_incremental_watch_events() -> bool {
        false
    }

    fn needs_watch_state_seed() -> bool {
        true
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
