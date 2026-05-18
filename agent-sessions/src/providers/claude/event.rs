use crate::agent_session::{projection::parsed_tool_input, *};
use smol_str::SmolStr;

/// Map a Claude assistant message's `stop_reason` to a `Response.phase`.
///
/// The convention matches Codex: a `phase = None` response is the final
/// answer that downstream consumers project as a notification. Any other
/// value (including `Some("intermediate")` for in-progress / streaming /
/// `tool_use` turns) marks the message as a process step that should not
/// be surfaced as a final result.
///
/// Terminal `stop_reason` values per the Anthropic API:
/// `end_turn`, `stop_sequence`, `max_tokens`, `pause_turn`, `refusal`.
/// `tool_use` means the model is asking to call a tool — the turn is
/// not yet finished, so the message is a process step, not a conclusion.
/// A missing/`null` `stop_reason` typically means the message is still
/// being streamed; we also treat that as a process step.
fn assistant_message_phase(stop_reason: Option<&str>) -> Option<SmolStr> {
    match stop_reason {
        Some("end_turn" | "stop_sequence" | "max_tokens" | "pause_turn" | "refusal") => None,
        Some(other) => Some(other.into()),
        None => Some("intermediate".into()),
    }
}

fn map_actor(role: &super::Role) -> Actor {
    match role {
        super::Role::User => Actor::User,
        super::Role::Assistant => Actor::Assistant,
        super::Role::System => Actor::System,
        super::Role::Other(value) => Actor::Other(value.clone().into()),
    }
}

fn map_blocks(blocks: &[super::ContentBlock]) -> Box<[ContentBlock]> {
    blocks
        .iter()
        .map(|block| match block {
            super::ContentBlock::Text(block) => ContentBlock::Text(TextBlock {
                text: block.text.clone(),
            }),
            super::ContentBlock::Thinking(block) => ContentBlock::Thinking(ThinkingBlock {
                text: block.text.clone(),
            }),
            super::ContentBlock::Image(block) => ContentBlock::Image(ImageBlock {
                image_url: block.data.clone().unwrap_or_default(),
            }),
            super::ContentBlock::ToolUse(block) => {
                let parsed = parsed_tool_input(block.name.as_ref(), block.input_json.as_deref());
                ContentBlock::ToolUse(ToolUseBlock {
                    id: smol_opt(block.id.clone()),
                    name: block.name.clone().into(),
                    input_json: block.input_json.clone(),
                    command: parsed.command,
                    file_path: parsed.file_path,
                    lines_added: parsed.lines_added,
                    lines_removed: parsed.lines_removed,
                })
            }
            super::ContentBlock::ToolResult(block) => ContentBlock::ToolResult(ToolResultBlock {
                tool_use_id: smol_opt(block.tool_use_id.clone()),
                content: block.content.clone(),
                is_error: block.is_error,
            }),
            super::ContentBlock::Raw(block) => ContentBlock::Raw(RawBlock {
                kind: block.kind.clone().into(),
                raw_json: block.raw_json.clone(),
            }),
        })
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

fn event_version(version: super::Version) -> VersionKind {
    match version {
        super::Version::V1 => VersionKind::new("claude-v1"),
        super::Version::V2 => VersionKind::new("claude-v2"),
    }
}

fn source_kind_str(version: super::Version) -> SmolStr {
    match version {
        super::Version::V1 => "v1".into(),
        super::Version::V2 => "v2".into(),
    }
}

fn operation_kind(name: &str) -> OperationKind {
    match name {
        "Bash" | "BashTool" | "PowerShellTool" => OperationKind::Shell,
        "Edit" | "FileEditTool" | "NotebookEdit" | "cursor:edit" => OperationKind::Edit,
        "Write" | "FileWriteTool" => OperationKind::Write,
        other if other.contains("Search") || other.contains("search") => OperationKind::Search,
        other if other.contains("Read") || other.contains("read") => OperationKind::Read,
        other if other.contains("mcp") || other.contains("MCP") => OperationKind::Mcp,
        other if other.contains("agent") => OperationKind::Subagent,
        other => OperationKind::Custom(other.into()),
    }
}

fn usage_event(message: &super::MessageEntry) -> Option<Event> {
    let usage = message.usage.as_ref()?;
    Some(event(
        Actor::Assistant,
        Body::Usage(Usage {
            model: smol_opt(message.model.clone()),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_tokens: usage.cache_creation_input_tokens,
            cache_read_tokens: usage.cache_read_input_tokens,
            cached_tokens: usage.cache_creation_input_tokens + usage.cache_read_input_tokens,
            reasoning_tokens: 0,
            tool_tokens: 0,
            total_tokens: usage.input_tokens
                + usage.output_tokens
                + usage.cache_creation_input_tokens
                + usage.cache_read_input_tokens,
            web_search_requests: usage.web_search_requests,
            speed: smol_opt(usage.speed.clone()),
        }),
        message.timestamp.clone(),
    ))
}

#[cfg(feature = "watch")]
fn watch_meta(id: Option<SmolStr>, timestamp: Option<SmolStr>) -> crate::watch::WatchEventMeta {
    crate::watch::WatchEventMeta {
        id: id.map(Into::into),
        timestamp: timestamp.map(Into::into),
        ..crate::watch::WatchEventMeta::default()
    }
}

#[cfg(feature = "watch")]
fn watch_text(text: Option<SmolStr>) -> Option<smol_str::SmolStr> {
    text.as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(crate::watch::watch_smol)
}

#[cfg(feature = "watch")]
fn claude_watch_meta(entries: &[super::Entry]) -> (Option<SmolStr>, Option<SmolStr>) {
    let session_id = entries.iter().find_map(|entry| match entry {
        super::Entry::Message(message) => message.session_id.clone(),
        super::Entry::InputSnapshot(snapshot) => snapshot.session_id.clone(),
        super::Entry::PermissionMode(mode) => mode.session_id.clone(),
        super::Entry::LastPrompt(prompt) => prompt.session_id.clone(),
        super::Entry::Progress(progress) => match progress {
            super::ProgressEntry::HookProgress { session_id, .. }
            | super::ProgressEntry::BashProgress { session_id, .. }
            | super::ProgressEntry::AgentProgress { session_id, .. }
            | super::ProgressEntry::QueryUpdate { session_id, .. }
            | super::ProgressEntry::SearchResultsReceived { session_id, .. }
            | super::ProgressEntry::McpProgress { session_id, .. }
            | super::ProgressEntry::WaitingForTask { session_id, .. }
            | super::ProgressEntry::Other { session_id, .. } => session_id.clone(),
        },
        _ => None,
    });
    let cwd = entries.iter().find_map(|entry| match entry {
        super::Entry::Message(message) => message.cwd.clone(),
        super::Entry::Progress(progress) => match progress {
            super::ProgressEntry::HookProgress { cwd, .. }
            | super::ProgressEntry::BashProgress { cwd, .. }
            | super::ProgressEntry::AgentProgress { cwd, .. }
            | super::ProgressEntry::QueryUpdate { cwd, .. }
            | super::ProgressEntry::SearchResultsReceived { cwd, .. }
            | super::ProgressEntry::McpProgress { cwd, .. }
            | super::ProgressEntry::WaitingForTask { cwd, .. }
            | super::ProgressEntry::Other { cwd, .. } => cwd.clone(),
        },
        _ => None,
    });
    (session_id, cwd)
}

#[cfg(feature = "watch")]
fn watch_usage_event(message: &super::MessageEntry) -> Option<crate::watch::WatchEvent> {
    let usage = message.usage.as_ref()?;
    Some(crate::watch::WatchEvent::Usage(crate::watch::WatchUsage {
        meta: watch_meta(message.message_id.clone(), message.timestamp.clone()),
        model: crate::watch::watch_smol_opt(message.model.clone()),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_tokens: usage.cache_creation_input_tokens,
        cache_read_tokens: usage.cache_read_input_tokens,
        cached_tokens: usage.cache_creation_input_tokens + usage.cache_read_input_tokens,
        reasoning_tokens: 0,
        tool_tokens: 0,
        total_tokens: usage.input_tokens
            + usage.output_tokens
            + usage.cache_creation_input_tokens
            + usage.cache_read_input_tokens,
        web_search_requests: usage.web_search_requests,
        speed: crate::watch::watch_smol_opt(usage.speed.clone()),
    }))
}

#[cfg(feature = "watch")]
fn watch_state(
    timestamp: Option<SmolStr>,
    kind: SmolStr,
    value_json: Option<SmolStr>,
) -> crate::watch::WatchEvent {
    crate::watch::WatchEvent::State(crate::watch::WatchState {
        meta: watch_meta(None, timestamp),
        kind: kind.into(),
        value_json: crate::watch::watch_smol_opt(value_json),
    })
}

#[cfg(feature = "watch")]
fn watch_events_from_claude_entries(entries: &[super::Entry]) -> Box<[crate::watch::WatchEvent]> {
    let mut events = Vec::with_capacity(entries.len());

    for entry in entries {
        match entry {
            super::Entry::Message(message) => {
                let meta = watch_meta(message.message_id.clone(), message.timestamp.clone());
                match message.role {
                    super::Role::User => {
                        events.push(crate::watch::WatchEvent::UserMessage(
                            crate::watch::WatchMessage {
                                meta,
                                text: watch_text(message.text()),
                            },
                        ));
                    }
                    super::Role::Assistant => {
                        events.push(crate::watch::WatchEvent::AssistantMessage(
                            crate::watch::WatchAssistantMessage {
                                meta,
                                model: crate::watch::watch_smol_opt(message.model.clone()),
                                phase: crate::watch::watch_smol_opt(assistant_message_phase(
                                    message.stop_reason.as_deref(),
                                )),
                                text: watch_text(message.text()),
                            },
                        ));
                    }
                    _ => {
                        let blocks = map_blocks(&message.blocks);
                        events.push(crate::watch::WatchEvent::Other(crate::watch::WatchOther {
                            meta,
                            actor: map_actor(&message.role),
                            body: Body::Prompt(Prompt {
                                text: text_from_blocks(&blocks),
                                blocks,
                            }),
                        }));
                    }
                }

                if let Some(usage) = watch_usage_event(message) {
                    events.push(usage);
                }
            }
            super::Entry::InputSnapshot(snapshot) => {
                events.push(crate::watch::WatchEvent::Snapshot(
                    crate::watch::WatchSnapshot {
                        meta: watch_meta(
                            None,
                            snapshot
                                .timestamp_millis
                                .map(|value| value.to_string().into()),
                        ),
                        actor: Actor::System,
                        kind: "input_snapshot".into(),
                        value_json: crate::watch::watch_smol(
                            serde_json::json!({
                                "display": snapshot.display,
                                "pasted_contents_json": snapshot.pasted_contents_json,
                                "project": snapshot.project,
                                "session_id": snapshot.session_id,
                                "timestamp_millis": snapshot.timestamp_millis,
                            })
                            .to_string(),
                        ),
                    },
                ));
            }
            super::Entry::ToolUse(tool) => {
                let name = tool.tool_name.clone().unwrap_or_else(|| "unknown".into());
                let parsed = parsed_tool_input(name.as_ref(), tool.tool_input_json.as_deref());
                events.push(crate::watch::WatchEvent::ToolCall(
                    crate::watch::WatchToolCall {
                        meta: watch_meta(None, tool.timestamp.clone()),
                        kind: operation_kind(name.as_ref()),
                        phase: OperationPhase::Requested,
                        call_id: None,
                        name: name.into(),
                        input_json: crate::watch::watch_smol_opt(tool.tool_input_json.clone()),
                        command: crate::watch::watch_smol_opt(parsed.command),
                        file_path: crate::watch::watch_smol_opt(parsed.file_path),
                        lines_added: parsed.lines_added,
                        lines_removed: parsed.lines_removed,
                    },
                ));
            }
            super::Entry::ToolResult(tool) => {
                let name = tool.tool_name.clone().unwrap_or_else(|| "unknown".into());
                events.push(crate::watch::WatchEvent::ToolResult(
                    crate::watch::WatchToolResult {
                        meta: watch_meta(None, tool.timestamp.clone()),
                        kind: operation_kind(name.as_ref()),
                        phase: OperationPhase::Completed,
                        call_id: None,
                        name: name.into(),
                        output_json: crate::watch::watch_smol_opt(tool.tool_output_json.clone()),
                        is_error: false,
                        duration_seconds: None,
                    },
                ));
            }
            super::Entry::Attachment(attachment) => {
                events.push(crate::watch::WatchEvent::Snapshot(
                    crate::watch::WatchSnapshot {
                        meta: watch_meta(None, attachment.timestamp.clone()),
                        actor: Actor::System,
                        kind: "attachment".into(),
                        value_json: crate::watch::watch_smol(
                            serde_json::json!({
                                "attachment_type": attachment.attachment_type,
                                "name": attachment.name,
                                "species": attachment.species,
                            })
                            .to_string(),
                        ),
                    },
                ));
            }
            super::Entry::PermissionMode(mode) => {
                events.push(watch_state(
                    None,
                    "permission_mode".into(),
                    Some(
                        serde_json::json!({
                            "permission_mode": mode.permission_mode,
                            "session_id": mode.session_id,
                        })
                        .to_string()
                        .into(),
                    ),
                ));
            }
            super::Entry::FileHistorySnapshot(snapshot) => {
                events.push(crate::watch::WatchEvent::Snapshot(
                    crate::watch::WatchSnapshot {
                        meta: watch_meta(None, snapshot.timestamp.clone()),
                        actor: Actor::System,
                        kind: "file_history_snapshot".into(),
                        value_json: crate::watch::watch_smol(
                            serde_json::json!({
                                "message_id": snapshot.message_id,
                            })
                            .to_string(),
                        ),
                    },
                ));
            }
            super::Entry::LastPrompt(prompt) => {
                events.push(watch_state(
                    None,
                    "last_prompt".into(),
                    Some(
                        serde_json::json!({
                            "session_id": prompt.session_id,
                            "last_prompt": prompt.last_prompt,
                        })
                        .to_string()
                        .into(),
                    ),
                ));
            }
            super::Entry::Progress(progress) => {
                let (kind, timestamp, op_id, value_json) = progress_payload(progress);
                let mut meta = watch_meta(None, timestamp);
                meta.op_id = op_id.map(Into::into);
                events.push(crate::watch::WatchEvent::State(crate::watch::WatchState {
                    meta,
                    kind: kind.into(),
                    value_json: crate::watch::watch_smol_opt(value_json),
                }));
            }
            super::Entry::QueueOperation(queue) => {
                events.push(watch_state(
                    queue.timestamp.clone(),
                    "queue_operation".into(),
                    Some(
                        serde_json::json!({
                            "session_id": queue.session_id,
                            "operation": queue.operation,
                            "content": queue.content,
                        })
                        .to_string()
                        .into(),
                    ),
                ));
            }
            super::Entry::System(system) => {
                events.push(watch_state(
                    system.timestamp.clone(),
                    system.subtype.clone().unwrap_or_else(|| "system".into()),
                    Some(
                        serde_json::json!({ "level": system.level })
                            .to_string()
                            .into(),
                    ),
                ));
            }
            super::Entry::Unknown(unknown) => {
                events.push(crate::watch::WatchEvent::Unknown(
                    crate::watch::WatchUnknown {
                        meta: watch_meta(None, unknown.timestamp.clone()),
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

type ProgressPayload = (SmolStr, Option<SmolStr>, Option<SmolStr>, Option<SmolStr>);

fn progress_payload(progress: &super::ProgressEntry) -> ProgressPayload {
    match progress {
        super::ProgressEntry::HookProgress {
            timestamp,
            parent_tool_use_id,
            tool_use_id,
            hook_event,
            hook_name,
            command,
            ..
        } => (
            "hook_progress".into(),
            timestamp.clone(),
            tool_use_id.clone().or_else(|| parent_tool_use_id.clone()),
            Some(
                serde_json::json!({
                    "hook_event": hook_event,
                    "hook_name": hook_name,
                    "command": command,
                })
                .to_string()
                .into(),
            ),
        ),
        super::ProgressEntry::BashProgress {
            timestamp,
            parent_tool_use_id,
            tool_use_id,
            output,
            full_output,
            elapsed_time_seconds,
            total_lines,
            ..
        } => (
            "bash_progress".into(),
            timestamp.clone(),
            tool_use_id.clone().or_else(|| parent_tool_use_id.clone()),
            Some(
                serde_json::json!({
                    "output": output,
                    "full_output": full_output,
                    "elapsed_time_seconds": elapsed_time_seconds,
                    "total_lines": total_lines,
                })
                .to_string()
                .into(),
            ),
        ),
        super::ProgressEntry::AgentProgress {
            timestamp,
            parent_tool_use_id,
            tool_use_id,
            prompt,
            agent_id,
            message_json,
            ..
        } => (
            "agent_progress".into(),
            timestamp.clone(),
            tool_use_id.clone().or_else(|| parent_tool_use_id.clone()),
            Some(
                serde_json::json!({
                    "prompt": prompt,
                    "agent_id": agent_id,
                    "message_json": message_json,
                })
                .to_string()
                .into(),
            ),
        ),
        super::ProgressEntry::QueryUpdate {
            timestamp,
            parent_tool_use_id,
            tool_use_id,
            query,
            ..
        } => (
            "query_update".into(),
            timestamp.clone(),
            tool_use_id.clone().or_else(|| parent_tool_use_id.clone()),
            Some(serde_json::json!({ "query": query }).to_string().into()),
        ),
        super::ProgressEntry::SearchResultsReceived {
            timestamp,
            parent_tool_use_id,
            tool_use_id,
            query,
            result_count,
            ..
        } => (
            "search_results_received".into(),
            timestamp.clone(),
            tool_use_id.clone().or_else(|| parent_tool_use_id.clone()),
            Some(
                serde_json::json!({
                    "query": query,
                    "result_count": result_count,
                })
                .to_string()
                .into(),
            ),
        ),
        super::ProgressEntry::McpProgress {
            timestamp,
            parent_tool_use_id,
            tool_use_id,
            status,
            server_name,
            tool_name,
            ..
        } => (
            "mcp_progress".into(),
            timestamp.clone(),
            tool_use_id.clone().or_else(|| parent_tool_use_id.clone()),
            Some(
                serde_json::json!({
                    "status": status,
                    "server_name": server_name,
                    "tool_name": tool_name,
                })
                .to_string()
                .into(),
            ),
        ),
        super::ProgressEntry::WaitingForTask {
            timestamp,
            parent_tool_use_id,
            tool_use_id,
            task_description,
            task_type,
            ..
        } => (
            "waiting_for_task".into(),
            timestamp.clone(),
            tool_use_id.clone().or_else(|| parent_tool_use_id.clone()),
            Some(
                serde_json::json!({
                    "task_description": task_description,
                    "task_type": task_type,
                })
                .to_string()
                .into(),
            ),
        ),
        super::ProgressEntry::Other {
            kind,
            timestamp,
            parent_tool_use_id,
            tool_use_id,
            ..
        } => (
            kind.clone().unwrap_or_else(|| "progress".into()),
            timestamp.clone(),
            tool_use_id.clone().or_else(|| parent_tool_use_id.clone()),
            None,
        ),
    }
}

fn build_session_meta(
    body: &super::Body,
    events: &[Event],
    version: super::Version,
) -> SessionMeta {
    let session_id = body.entries.iter().find_map(|entry| match entry {
        super::Entry::Message(message) => message.session_id.clone(),
        super::Entry::InputSnapshot(snapshot) => snapshot.session_id.clone(),
        super::Entry::PermissionMode(mode) => mode.session_id.clone(),
        super::Entry::LastPrompt(prompt) => prompt.session_id.clone(),
        super::Entry::Progress(progress) => match progress {
            super::ProgressEntry::HookProgress { session_id, .. }
            | super::ProgressEntry::BashProgress { session_id, .. }
            | super::ProgressEntry::AgentProgress { session_id, .. }
            | super::ProgressEntry::QueryUpdate { session_id, .. }
            | super::ProgressEntry::SearchResultsReceived { session_id, .. }
            | super::ProgressEntry::McpProgress { session_id, .. }
            | super::ProgressEntry::WaitingForTask { session_id, .. }
            | super::ProgressEntry::Other { session_id, .. } => session_id.clone(),
        },
        _ => None,
    });
    let cwd = body.entries.iter().find_map(|entry| match entry {
        super::Entry::Message(message) => message.cwd.clone(),
        super::Entry::Progress(progress) => match progress {
            super::ProgressEntry::HookProgress { cwd, .. }
            | super::ProgressEntry::BashProgress { cwd, .. }
            | super::ProgressEntry::AgentProgress { cwd, .. }
            | super::ProgressEntry::QueryUpdate { cwd, .. }
            | super::ProgressEntry::SearchResultsReceived { cwd, .. }
            | super::ProgressEntry::McpProgress { cwd, .. }
            | super::ProgressEntry::WaitingForTask { cwd, .. }
            | super::ProgressEntry::Other { cwd, .. } => cwd.clone(),
        },
        _ => None,
    });

    SessionMeta {
        session_id: smol_opt(session_id),
        cwd,
        models: summarize_models(events),
        created_at: smol_opt(earliest_timestamp(events)),
        updated_at: smol_opt(latest_timestamp(events)),
        source_kind: Some(source_kind_str(version).into()),
        ..SessionMeta::default()
    }
}

fn agent_session_from_claude_body(body: &super::Body, version: super::Version) -> Session {
    let mut events = Vec::new();

    for entry in &body.entries {
        match entry {
            super::Entry::Message(message) => {
                let blocks = map_blocks(&message.blocks);
                let actor = map_actor(&message.role);
                let body = match message.role {
                    super::Role::Assistant => Body::Response(Response {
                        model: smol_opt(message.model.clone()),
                        phase: smol_opt(assistant_message_phase(message.stop_reason.as_deref())),
                        text: text_from_blocks(&blocks),
                        blocks,
                    }),
                    _ => Body::Prompt(Prompt {
                        text: text_from_blocks(&blocks),
                        blocks,
                    }),
                };
                let mut ev = event(actor, body, message.timestamp.clone());
                ev.id = smol_opt(message.message_id.clone());
                events.push(ev);

                if let Some(usage) = usage_event(message) {
                    events.push(usage);
                }
            }
            super::Entry::InputSnapshot(snapshot) => {
                events.push(event(
                    Actor::System,
                    Body::Snapshot(Snapshot {
                        kind: "input_snapshot".into(),
                        value_json: serde_json::json!({
                            "display": snapshot.display,
                            "pasted_contents_json": snapshot.pasted_contents_json,
                            "project": snapshot.project,
                            "session_id": snapshot.session_id,
                            "timestamp_millis": snapshot.timestamp_millis,
                        })
                        .to_string()
                        .into(),
                    }),
                    snapshot
                        .timestamp_millis
                        .map(|value| value.to_string().into()),
                ));
            }
            super::Entry::ToolUse(tool) => {
                let parsed = parsed_tool_input(
                    tool.tool_name.as_deref().unwrap_or("unknown"),
                    tool.tool_input_json.as_deref(),
                );
                events.push(event(
                    Actor::Tool,
                    Body::Operation(Operation {
                        kind: operation_kind(tool.tool_name.as_deref().unwrap_or("unknown")),
                        phase: OperationPhase::Requested,
                        name: tool
                            .tool_name
                            .clone()
                            .unwrap_or_else(|| "unknown".into())
                            .into(),
                        input_json: tool.tool_input_json.clone(),
                        output_json: None,
                        command: parsed.command,
                        file_path: parsed.file_path,
                        lines_added: parsed.lines_added,
                        lines_removed: parsed.lines_removed,
                        is_error: false,
                        duration_seconds: None,
                        extra_json: None,
                    }),
                    tool.timestamp.clone(),
                ));
            }
            super::Entry::ToolResult(tool) => {
                let parsed = parsed_tool_input(
                    tool.tool_name.as_deref().unwrap_or("unknown"),
                    tool.tool_input_json.as_deref(),
                );
                events.push(event(
                    Actor::Tool,
                    Body::Operation(Operation {
                        kind: operation_kind(tool.tool_name.as_deref().unwrap_or("unknown")),
                        phase: OperationPhase::Completed,
                        name: tool
                            .tool_name
                            .clone()
                            .unwrap_or_else(|| "unknown".into())
                            .into(),
                        input_json: tool.tool_input_json.clone(),
                        output_json: tool.tool_output_json.clone(),
                        command: parsed.command,
                        file_path: parsed.file_path,
                        lines_added: parsed.lines_added,
                        lines_removed: parsed.lines_removed,
                        is_error: false,
                        duration_seconds: None,
                        extra_json: None,
                    }),
                    tool.timestamp.clone(),
                ));
            }
            super::Entry::Attachment(attachment) => {
                events.push(event(
                    Actor::System,
                    Body::Snapshot(Snapshot {
                        kind: "attachment".into(),
                        value_json: serde_json::json!({
                            "attachment_type": attachment.attachment_type,
                            "name": attachment.name,
                            "species": attachment.species,
                        })
                        .to_string()
                        .into(),
                    }),
                    attachment.timestamp.clone(),
                ));
            }
            super::Entry::PermissionMode(mode) => {
                events.push(event(
                    Actor::System,
                    Body::State(State {
                        kind: "permission_mode".into(),
                        value_json: Some(
                            serde_json::json!({
                                "permission_mode": mode.permission_mode,
                                "session_id": mode.session_id,
                            })
                            .to_string()
                            .into(),
                        ),
                    }),
                    None,
                ));
            }
            super::Entry::FileHistorySnapshot(snapshot) => {
                events.push(event(
                    Actor::System,
                    Body::Snapshot(Snapshot {
                        kind: "file_history_snapshot".into(),
                        value_json: serde_json::json!({
                            "message_id": snapshot.message_id,
                        })
                        .to_string()
                        .into(),
                    }),
                    snapshot.timestamp.clone(),
                ));
            }
            super::Entry::LastPrompt(prompt) => {
                events.push(event(
                    Actor::System,
                    Body::State(State {
                        kind: "last_prompt".into(),
                        value_json: Some(
                            serde_json::json!({
                                "session_id": prompt.session_id,
                                "last_prompt": prompt.last_prompt,
                            })
                            .to_string()
                            .into(),
                        ),
                    }),
                    None,
                ));
            }
            super::Entry::Progress(progress) => {
                let (kind, timestamp, op_id, value_json) = progress_payload(progress);
                let mut ev = event(
                    Actor::Tool,
                    Body::State(State {
                        kind: kind.into(),
                        value_json,
                    }),
                    timestamp,
                );
                ev.op_id = smol_opt(op_id);
                events.push(ev);
            }
            super::Entry::QueueOperation(queue) => {
                events.push(event(
                    Actor::System,
                    Body::State(State {
                        kind: "queue_operation".into(),
                        value_json: Some(
                            serde_json::json!({
                                "session_id": queue.session_id,
                                "operation": queue.operation,
                                "content": queue.content,
                            })
                            .to_string()
                            .into(),
                        ),
                    }),
                    queue.timestamp.clone(),
                ));
            }
            super::Entry::System(system) => {
                events.push(event(
                    Actor::System,
                    Body::State(State {
                        kind: system
                            .subtype
                            .clone()
                            .unwrap_or_else(|| "system".into())
                            .into(),
                        value_json: Some(
                            serde_json::json!({ "level": system.level })
                                .to_string()
                                .into(),
                        ),
                    }),
                    system.timestamp.clone(),
                ));
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
        agent: AgentKind::new("claude"),
        version: event_version(version),
        meta: build_session_meta(body, &events, version),
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
    let (version, body) = super::parse_claude_body_reader(reader, selection)?;
    Ok(crate::agent_session::filter_selection(
        agent_session_from_claude_body(&body, version),
        selection,
    ))
}

#[cfg(feature = "watch")]
impl crate::watch::provider::ProviderWatchEvents for crate::Claude {
    fn parse_watch_reader<R>(
        _path: &std::path::Path,
        reader: R,
        selection: crate::ParseSelection,
    ) -> crate::Result<crate::watch::provider::ParsedWatchSession>
    where
        R: std::io::BufRead,
    {
        let (_version, body) = super::parse_claude_reader(reader, selection)?;
        let (session_id, cwd) = claude_watch_meta(&body.entries);
        Ok(crate::watch::provider::ParsedWatchSession {
            session_id,
            cwd,
            events: watch_events_from_claude_entries(&body.entries),
        })
    }

    fn parse_watch_metadata_reader<R>(
        _path: &std::path::Path,
        reader: R,
    ) -> crate::Result<crate::watch::provider::ParsedWatchSession>
    where
        R: std::io::BufRead,
    {
        let Some(meta) = crate::Claude::probe_session_meta(reader)? else {
            return Err(crate::Error::Detection { agent: "claude" });
        };
        Ok(crate::watch::provider::parsed_watch_metadata(
            meta.session_id
                .map(|session_id| session_id.to_string().into()),
            meta.cwd,
        ))
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
