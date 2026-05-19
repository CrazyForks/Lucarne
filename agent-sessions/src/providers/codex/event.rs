use smol_str::SmolStr;
use std::collections::HashMap;
#[cfg(feature = "watch")]
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use serde::Deserialize;

use crate::agent_session::{projection::parsed_tool_input, *};

fn codex_projection_selection(selection: crate::ParseSelection) -> crate::ParseSelection {
    selection
}

#[derive(Clone)]
struct OpTemplate {
    kind: OperationKind,
    name: smol_str::SmolStr,
    command: Option<SmolStr>,
    file_path: Option<SmolStr>,
    lines_added: u64,
    lines_removed: u64,
}

fn map_actor(role: &super::Role) -> Actor {
    match role {
        super::Role::User => Actor::User,
        super::Role::Assistant => Actor::Assistant,
        super::Role::System | super::Role::Developer => Actor::System,
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
            super::ContentBlock::Image(block) => ContentBlock::Image(ImageBlock {
                image_url: block.image_url.clone(),
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
        super::Version::V1 => VersionKind::new("codex-v1"),
        super::Version::V2 => VersionKind::new("codex-v2"),
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
        "exec_command" | "shell_command" | "write_stdin" | "run_shell_command" => {
            OperationKind::Shell
        }
        "apply_patch" | "patch_apply" => OperationKind::Edit,
        "web_search" | "web_search_call" => OperationKind::Web,
        "mcp_tool_call" => OperationKind::Mcp,
        "spawn_agent" | "send_input" | "wait_agent" | "close_agent" | "resume_agent" => {
            OperationKind::Subagent
        }
        other if other.contains("search") => OperationKind::Search,
        other if other.contains("read") => OperationKind::Read,
        other if other.contains("write") => OperationKind::Write,
        other if other.contains("edit") || other.contains("patch") => OperationKind::Edit,
        other if other.contains("mcp") => OperationKind::Mcp,
        other if other.contains("agent") => OperationKind::Subagent,
        other => OperationKind::Custom(other.into()),
    }
}

fn command_from_json(raw: Option<&str>) -> Option<SmolStr> {
    let raw = raw?;
    if let Ok(parsed) = serde_json::from_str::<String>(raw) {
        return Some(parsed.into());
    }

    #[derive(Deserialize)]
    struct CommandArgs {
        #[serde(default)]
        cmd: Option<SmolStr>,
        #[serde(default)]
        command: Option<SmolStr>,
    }

    serde_json::from_str::<CommandArgs>(raw)
        .ok()
        .and_then(|parsed| parsed.cmd.or(parsed.command))
}

fn duration_from_json(raw: Option<&str>) -> Option<f64> {
    let raw = raw?;
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    match value {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::Object(map) => map
            .get("duration_seconds")
            .or_else(|| map.get("seconds"))
            .or_else(|| map.get("secs"))
            .and_then(|value| value.as_f64()),
        _ => None,
    }
}

fn event_op_id(data: &super::EventMsgData) -> Option<SmolStr> {
    match data {
        super::EventMsgData::ExecCommandEnd(data) => data.call_id.clone(),
        super::EventMsgData::PatchApplyEnd(data) => data.call_id.clone(),
        super::EventMsgData::WebSearchEnd(data) => data.call_id.clone(),
        super::EventMsgData::CollabWaitingEnd(data) => data.call_id.clone(),
        super::EventMsgData::McpToolCallEnd(data) => data.call_id.clone(),
        super::EventMsgData::DynamicToolCallRequest(data) => data.call_id.clone(),
        super::EventMsgData::DynamicToolCallResponse(data) => data.call_id.clone(),
        super::EventMsgData::CollabAgentSpawnEnd(data) => data.call_id.clone(),
        super::EventMsgData::CollabCloseEnd(data) => data.call_id.clone(),
        super::EventMsgData::CollabAgentInteractionEnd(data) => data.call_id.clone(),
        _ => None,
    }
}

fn build_session_meta(
    body: &super::Body,
    events: &[Event],
    version: super::Version,
) -> SessionMeta {
    let session_meta = body.entries.iter().find_map(|entry| match entry {
        super::Entry::SessionMeta(meta) => Some(meta),
        _ => None,
    });
    let turn_context = body.entries.iter().find_map(|entry| match entry {
        super::Entry::TurnContext(context) => Some(context),
        _ => None,
    });

    let initial_model = session_meta
        .and_then(|meta| meta.model.clone())
        .or_else(|| turn_context.and_then(|context| context.model.clone()));
    let mut models = summarize_models(events).into_vec();
    if let Some(model) = initial_model.clone()
        && !models
            .iter()
            .any(|summary| summary.model.as_str() == model.as_str())
    {
        models.push(SessionModelMeta::zero(model));
    }

    SessionMeta {
        session_id: smol_opt(session_meta.and_then(|meta| meta.session_id.clone())),
        cwd: session_meta
            .and_then(|meta| meta.cwd.clone())
            .or_else(|| turn_context.and_then(|context| context.cwd.clone())),
        models: models.into_boxed_slice(),
        created_at: smol_opt(
            session_meta
                .and_then(|meta| meta.timestamp.clone())
                .or_else(|| earliest_timestamp(events)),
        ),
        updated_at: smol_opt(latest_timestamp(events)),
        source_kind: Some(source_kind_str(version).into()),
        extra_json: session_meta.map(|meta| {
            serde_json::json!({
                "originator": meta.originator,
                "model": meta.model,
                "cli_version": meta.cli_version,
            })
            .to_string()
            .into()
        }),
        ..SessionMeta::default()
    }
}

fn op_template(
    name: SmolStr,
    input_json: Option<&str>,
    command_override: Option<SmolStr>,
) -> OpTemplate {
    let parsed = parsed_tool_input(name.as_ref(), input_json);
    OpTemplate {
        kind: operation_kind(name.as_ref()),
        name: name.into(),
        command: command_override.or(parsed.command),
        file_path: parsed.file_path,
        lines_added: parsed.lines_added,
        lines_removed: parsed.lines_removed,
    }
}

fn response_item_message_text(entry: &super::Entry) -> Option<String> {
    let super::Entry::Message(message) = entry else {
        return None;
    };
    if !matches!(message.role, super::Role::Assistant) {
        return None;
    }
    let blocks = map_blocks(&message.blocks);
    crate::agent_session::text_from_blocks(&blocks).map(|text| text.trim().to_string())
}

fn is_mirrored_agent_message(entries: &[super::Entry], index: usize, message: &str) -> bool {
    let message = message.trim();
    if message.is_empty() {
        return false;
    }

    let matches_message = |entry: &super::Entry| {
        response_item_message_text(entry).is_some_and(|text| text == message)
    };

    index
        .checked_sub(1)
        .and_then(|prev| entries.get(prev))
        .is_some_and(matches_message)
        || entries.get(index + 1).is_some_and(matches_message)
}

#[cfg(feature = "watch")]
fn codex_watch_meta(
    id: Option<SmolStr>,
    timestamp: Option<SmolStr>,
    turn_id: Option<SmolStr>,
    op_id: Option<SmolStr>,
) -> crate::watch::WatchEventMeta {
    crate::watch::WatchEventMeta {
        id: id.map(Into::into),
        timestamp: timestamp.map(Into::into),
        turn_id: turn_id.map(Into::into),
        op_id: op_id.map(Into::into),
        parent_op_id: None,
    }
}

#[cfg(feature = "watch")]
fn codex_watch_text(text: Option<SmolStr>) -> Option<smol_str::SmolStr> {
    text.as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(crate::watch::watch_smol)
}

#[cfg(feature = "watch")]
fn codex_watch_image_generation(
    meta: crate::watch::WatchEventMeta,
    data: &super::ImageGenerationEventMsg,
) -> Option<crate::watch::WatchEvent> {
    let result = data.result_base64.as_deref()?.trim();
    if result.is_empty() {
        return None;
    }
    let id = data.id.clone().or_else(|| meta.id.clone());
    let short_id = id.as_deref().unwrap_or("image").to_string();
    let caption = data
        .revised_prompt
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty() && text.chars().count() <= 256)
        .map(crate::watch::watch_smol);
    Some(crate::watch::WatchEvent::Attachment(
        crate::watch::WatchAttachment {
            meta,
            id,
            filename: crate::watch::watch_smol(format!("codex-image-{short_id}.png")),
            media_type: crate::watch::watch_smol("image/png"),
            data_base64: crate::watch::watch_smol(result.to_string()),
            caption,
        },
    ))
}

#[cfg(feature = "watch")]
fn codex_watch_meta_fields(entries: &[super::Entry]) -> (Option<SmolStr>, Option<SmolStr>) {
    let session_meta = entries.iter().find_map(|entry| match entry {
        super::Entry::SessionMeta(meta) => Some(meta),
        _ => None,
    });
    let turn_context = entries.iter().find_map(|entry| match entry {
        super::Entry::TurnContext(context) => Some(context),
        _ => None,
    });
    (
        session_meta.and_then(|meta| meta.session_id.clone()),
        session_meta
            .and_then(|meta| meta.cwd.clone())
            .or_else(|| turn_context.and_then(|context| context.cwd.clone())),
    )
}

#[cfg(feature = "watch")]
fn codex_watch_state(
    meta: crate::watch::WatchEventMeta,
    kind: impl Into<smol_str::SmolStr>,
    value_json: Option<SmolStr>,
) -> crate::watch::WatchEvent {
    crate::watch::WatchEvent::State(crate::watch::WatchState {
        meta,
        kind: kind.into(),
        value_json: crate::watch::watch_smol_opt(value_json),
    })
}

#[cfg(feature = "watch")]
fn codex_watch_tool_result(
    meta: crate::watch::WatchEventMeta,
    kind: OperationKind,
    phase: OperationPhase,
    name: smol_str::SmolStr,
    output_json: Option<SmolStr>,
    is_error: bool,
    duration_seconds: Option<f64>,
) -> crate::watch::WatchEvent {
    let call_id = meta.parent_op_id.as_deref().map(Into::into);
    crate::watch::WatchEvent::ToolResult(crate::watch::WatchToolResult {
        meta,
        kind,
        phase,
        call_id,
        name,
        output_json: crate::watch::watch_smol_opt(output_json),
        is_error,
        duration_seconds,
    })
}

#[cfg(feature = "watch")]
fn codex_watch_task_complete(
    meta: crate::watch::WatchEventMeta,
    value_json: Option<SmolStr>,
) -> crate::watch::WatchEvent {
    let value = value_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<CodexTaskCompleteValue<'_>>(json).ok());
    crate::watch::WatchEvent::TurnCompleted(crate::watch::WatchTurnCompleted {
        meta,
        last_agent_message: value
            .as_ref()
            .and_then(|value| value.last_agent_message.as_deref())
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .map(crate::watch::watch_smol),
        duration_ms: value.as_ref().and_then(|value| value.duration_ms),
        value_json: crate::watch::watch_smol_opt(value_json),
    })
}

#[cfg(feature = "watch")]
fn codex_watch_turn_failed(
    meta: crate::watch::WatchEventMeta,
    value_json: Option<SmolStr>,
) -> crate::watch::WatchEvent {
    let value = value_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<CodexTurnAbortedValue<'_>>(json).ok());
    crate::watch::WatchEvent::TurnFailed(crate::watch::WatchTurnFailed {
        meta,
        reason: value
            .as_ref()
            .and_then(|value| value.reason.as_deref())
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .map(crate::watch::watch_smol),
        duration_ms: None,
        value_json: crate::watch::watch_smol_opt(value_json),
    })
}

#[cfg(feature = "watch")]
#[derive(Deserialize)]
struct CodexTaskCompleteValue<'a> {
    #[serde(default, borrow)]
    last_agent_message: Option<std::borrow::Cow<'a, str>>,
    #[serde(default)]
    duration_ms: Option<u64>,
}

#[cfg(feature = "watch")]
#[derive(Deserialize)]
struct CodexTurnAbortedValue<'a> {
    #[serde(default, borrow)]
    reason: Option<std::borrow::Cow<'a, str>>,
}

#[cfg(feature = "watch")]
fn codex_watch_usage(meta: crate::watch::WatchEventMeta, usage: Usage) -> crate::watch::WatchEvent {
    crate::watch::WatchEvent::Usage(crate::watch::WatchUsage {
        meta,
        model: crate::watch::watch_smol_opt(usage.model),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cached_tokens: usage.cached_tokens,
        reasoning_tokens: usage.reasoning_tokens,
        tool_tokens: usage.tool_tokens,
        total_tokens: usage.total_tokens,
        web_search_requests: usage.web_search_requests,
        speed: crate::watch::watch_smol_opt(usage.speed),
    })
}

#[cfg(feature = "watch")]
fn codex_watch_from_actor_body(
    meta: crate::watch::WatchEventMeta,
    actor: Actor,
    body: Body,
) -> crate::watch::WatchEvent {
    match (actor, body) {
        (Actor::User, Body::Prompt(prompt)) => {
            crate::watch::WatchEvent::UserMessage(crate::watch::WatchMessage {
                meta,
                text: codex_watch_text(
                    prompt
                        .text
                        .or_else(|| crate::agent_session::text_from_blocks(&prompt.blocks)),
                ),
            })
        }
        (Actor::Assistant, Body::Response(response)) => {
            crate::watch::WatchEvent::AssistantMessage(crate::watch::WatchAssistantMessage {
                meta,
                model: crate::watch::watch_smol_opt(response.model),
                phase: crate::watch::watch_smol_opt(response.phase),
                text: codex_watch_text(
                    response
                        .text
                        .or_else(|| crate::agent_session::text_from_blocks(&response.blocks)),
                ),
            })
        }
        (Actor::Assistant, Body::Operation(operation)) => {
            crate::watch::WatchEvent::ToolCall(crate::watch::WatchToolCall {
                call_id: meta.op_id.as_deref().map(Into::into),
                meta,
                kind: operation.kind,
                phase: operation.phase,
                name: operation.name,
                input_json: crate::watch::watch_smol_opt(operation.input_json),
                command: crate::watch::watch_smol_opt(operation.command),
                file_path: crate::watch::watch_smol_opt(operation.file_path),
                lines_added: operation.lines_added,
                lines_removed: operation.lines_removed,
            })
        }
        (Actor::Tool, Body::Operation(operation)) => codex_watch_tool_result(
            meta,
            operation.kind,
            operation.phase,
            operation.name,
            operation.output_json,
            operation.is_error,
            operation.duration_seconds,
        ),
        (Actor::System, Body::Usage(usage)) => codex_watch_usage(meta, usage),
        (Actor::System, Body::State(state)) => match state.kind.as_ref() {
            "task_complete" => codex_watch_task_complete(meta, state.value_json),
            "turn_aborted" => codex_watch_turn_failed(meta, state.value_json),
            _ => codex_watch_state(meta, state.kind, state.value_json),
        },
        (actor, Body::Snapshot(snapshot)) => {
            crate::watch::WatchEvent::Snapshot(crate::watch::WatchSnapshot {
                meta,
                actor,
                kind: snapshot.kind,
                value_json: snapshot.value_json.into(),
            })
        }
        (actor, Body::Unknown(unknown)) => {
            crate::watch::WatchEvent::Unknown(crate::watch::WatchUnknown {
                meta,
                actor,
                kind: unknown.kind,
                raw_json: unknown.raw_json.into(),
            })
        }
        (actor, body) => {
            crate::watch::WatchEvent::Other(crate::watch::WatchOther { meta, actor, body })
        }
    }
}

#[cfg(feature = "watch")]
fn watch_events_from_codex_entries(
    entries: &[super::Entry],
    selection: crate::ParseSelection,
) -> Box<[crate::watch::WatchEvent]> {
    let mut events = Vec::new();
    let mut current_turn_id = None;
    let mut ops = HashMap::<String, OpTemplate>::new();
    let include_lifecycle = selection.includes_messages() || selection.includes_state();

    for (entry_index, entry) in entries.iter().enumerate() {
        match entry {
            super::Entry::SessionMeta(_) => {}
            super::Entry::TurnContext(context) => {
                current_turn_id = context.turn_id.clone();
                if selection.includes_state() {
                    events.push(codex_watch_state(
                        codex_watch_meta(None, None, context.turn_id.clone(), None),
                        "turn_context",
                        Some(
                            serde_json::json!({
                                "cwd": context.cwd,
                                "current_date": context.current_date,
                                "timezone": context.timezone,
                                "model": context.model,
                            })
                            .to_string()
                            .into(),
                        ),
                    ));
                }
            }
            super::Entry::Message(message) => {
                let blocks = map_blocks(&message.blocks);
                let actor = map_actor(&message.role);
                let body = match message.role {
                    super::Role::Assistant => Body::Response(Response {
                        model: smol_opt(message.model.clone()),
                        phase: smol_opt(message.phase.clone()),
                        text: text_from_blocks(&blocks),
                        blocks,
                    }),
                    _ => Body::Prompt(Prompt {
                        text: text_from_blocks(&blocks),
                        blocks,
                    }),
                };
                events.push(codex_watch_from_actor_body(
                    codex_watch_meta(
                        None,
                        message.timestamp.clone(),
                        current_turn_id.clone(),
                        None,
                    ),
                    actor,
                    body,
                ));
            }
            super::Entry::FunctionCall(call) => {
                let template = op_template(
                    call.name.clone(),
                    call.arguments_json.as_deref(),
                    call.shell_command().map(crate::util::cow_to_box),
                );
                if let Some(call_id) = call.call_id.as_ref().or(call.id.as_ref()) {
                    ops.insert(call_id.to_string(), template.clone());
                }
                events.push(codex_watch_from_actor_body(
                    codex_watch_meta(
                        call.id.clone(),
                        call.timestamp.clone(),
                        current_turn_id.clone(),
                        call.call_id.clone().or_else(|| call.id.clone()),
                    ),
                    Actor::Tool,
                    Body::Operation(Operation {
                        kind: template.kind,
                        phase: OperationPhase::Requested,
                        name: template.name.clone(),
                        input_json: call.arguments_json.clone(),
                        output_json: None,
                        command: template.command.clone(),
                        file_path: template.file_path.clone(),
                        lines_added: template.lines_added,
                        lines_removed: template.lines_removed,
                        is_error: false,
                        duration_seconds: None,
                        extra_json: None,
                    }),
                ));
            }
            super::Entry::FunctionCallOutput(output) => {
                let template = ops
                    .get(output.call_id.as_str())
                    .cloned()
                    .unwrap_or_else(|| op_template("function_call".into(), None, None));
                events.push(codex_watch_from_actor_body(
                    codex_watch_meta(
                        None,
                        output.timestamp.clone(),
                        current_turn_id.clone(),
                        Some(output.call_id.clone()),
                    ),
                    Actor::Tool,
                    Body::Operation(Operation {
                        kind: template.kind,
                        phase: OperationPhase::Completed,
                        name: template.name,
                        input_json: None,
                        output_json: Some(output.output.clone()),
                        command: template.command,
                        file_path: template.file_path,
                        lines_added: template.lines_added,
                        lines_removed: template.lines_removed,
                        is_error: false,
                        duration_seconds: Some(output.shell_duration_seconds())
                            .filter(|seconds| *seconds > 0.0),
                        extra_json: None,
                    }),
                ));
            }
            super::Entry::CustomToolCall(call) => {
                let template = op_template(
                    call.name.clone(),
                    call.input.as_deref(),
                    command_from_json(call.input.as_deref()),
                );
                if let Some(call_id) = &call.call_id {
                    ops.insert(call_id.to_string(), template.clone());
                }
                events.push(codex_watch_from_actor_body(
                    codex_watch_meta(
                        None,
                        call.timestamp.clone(),
                        current_turn_id.clone(),
                        call.call_id.clone(),
                    ),
                    Actor::Tool,
                    Body::Operation(Operation {
                        kind: template.kind,
                        phase: OperationPhase::Requested,
                        name: template.name.clone(),
                        input_json: call.input.clone(),
                        output_json: None,
                        command: template.command.clone(),
                        file_path: template.file_path.clone(),
                        lines_added: template.lines_added,
                        lines_removed: template.lines_removed,
                        is_error: false,
                        duration_seconds: None,
                        extra_json: call.status.clone(),
                    }),
                ));
            }
            super::Entry::CustomToolCallOutput(output) => {
                let template = output
                    .call_id
                    .as_deref()
                    .and_then(|call_id| ops.get(call_id))
                    .cloned()
                    .unwrap_or_else(|| op_template("custom_tool".into(), None, None));
                events.push(codex_watch_from_actor_body(
                    codex_watch_meta(
                        None,
                        output.timestamp.clone(),
                        current_turn_id.clone(),
                        output.call_id.clone(),
                    ),
                    Actor::Tool,
                    Body::Operation(Operation {
                        kind: template.kind,
                        phase: OperationPhase::Completed,
                        name: template.name,
                        input_json: None,
                        output_json: Some(output.output.clone()),
                        command: template.command,
                        file_path: template.file_path,
                        lines_added: template.lines_added,
                        lines_removed: template.lines_removed,
                        is_error: false,
                        duration_seconds: None,
                        extra_json: None,
                    }),
                ));
            }
            super::Entry::WebSearchCall(search) => {
                events.push(codex_watch_from_actor_body(
                    codex_watch_meta(
                        None,
                        search.timestamp.clone(),
                        current_turn_id.clone(),
                        None,
                    ),
                    Actor::Tool,
                    Body::Operation(Operation {
                        kind: OperationKind::Web,
                        phase: OperationPhase::Requested,
                        name: "web_search".into(),
                        input_json: Some(
                            serde_json::json!({
                                "status": search.status,
                                "action_type": search.action_type,
                                "query": search.query,
                                "queries": search.queries,
                            })
                            .to_string()
                            .into(),
                        ),
                        output_json: None,
                        command: None,
                        file_path: None,
                        lines_added: 0,
                        lines_removed: 0,
                        is_error: false,
                        duration_seconds: None,
                        extra_json: None,
                    }),
                ));
            }
            super::Entry::GhostSnapshot(snapshot) => {
                events.push(codex_watch_from_actor_body(
                    codex_watch_meta(
                        None,
                        snapshot.timestamp.clone(),
                        current_turn_id.clone(),
                        None,
                    ),
                    Actor::System,
                    Body::Snapshot(Snapshot {
                        kind: "ghost_snapshot".into(),
                        value_json: serde_json::json!({
                            "commit_id": snapshot.commit_id,
                            "parent_id": snapshot.parent_id,
                            "preexisting_untracked_files": snapshot.preexisting_untracked_files,
                            "preexisting_untracked_dirs": snapshot.preexisting_untracked_dirs,
                        })
                        .to_string()
                        .into(),
                    }),
                ));
            }
            super::Entry::Compacted(compacted) => {
                if selection.includes_state() {
                    events.push(codex_watch_state(
                        codex_watch_meta(
                            None,
                            compacted.timestamp.clone(),
                            current_turn_id.clone(),
                            None,
                        ),
                        "compacted",
                        compacted.message.clone().map(|message| {
                            serde_json::json!({ "message": message }).to_string().into()
                        }),
                    ));
                }
            }
            super::Entry::Reasoning(reasoning) => {
                events.push(crate::watch::WatchEvent::AssistantMessage(
                    crate::watch::WatchAssistantMessage {
                        meta: codex_watch_meta(
                            None,
                            reasoning.timestamp.clone(),
                            current_turn_id.clone(),
                            None,
                        ),
                        model: None,
                        phase: None,
                        text: None,
                    },
                ));
            }
            super::Entry::EventMsg(message) => {
                let event_turn_id = message.turn_id.clone().or_else(|| current_turn_id.clone());
                let event_meta = |op_id| {
                    codex_watch_meta(
                        message.last_agent_message.clone(),
                        message.timestamp.clone(),
                        event_turn_id.clone(),
                        op_id,
                    )
                };
                match &message.data {
                    super::EventMsgData::TaskStarted(data) => {
                        if selection.includes_state() {
                            events.push(codex_watch_state(
                                event_meta(None),
                                "task_started",
                                Some(
                                    serde_json::json!({
                                        "model_context_window": data.model_context_window,
                                        "collaboration_mode_kind": data.collaboration_mode_kind,
                                    })
                                    .to_string()
                                    .into(),
                                ),
                            ));
                        }
                    }
                    super::EventMsgData::TaskComplete(data) => {
                        if include_lifecycle {
                            let value_json = Some(
                                serde_json::json!({
                                    "last_agent_message": message.last_agent_message,
                                    "completed_at": data.completed_at,
                                    "duration_ms": data.duration_ms,
                                    "time_to_first_token_ms": data.time_to_first_token_ms,
                                })
                                .to_string()
                                .into(),
                            );
                            events.push(codex_watch_task_complete(event_meta(None), value_json));
                        }
                    }
                    super::EventMsgData::AgentMessage(data) => {
                        if data.message.as_deref().is_some_and(|text| {
                            is_mirrored_agent_message(entries, entry_index, text)
                        }) {
                            continue;
                        }
                        events.push(crate::watch::WatchEvent::AssistantMessage(
                            crate::watch::WatchAssistantMessage {
                                meta: event_meta(None),
                                model: None,
                                phase: crate::watch::watch_smol_opt(data.phase.clone()),
                                text: codex_watch_text(data.message.clone()),
                            },
                        ));
                    }
                    super::EventMsgData::AgentReasoning(data) => {
                        let _ = data;
                        events.push(crate::watch::WatchEvent::AssistantMessage(
                            crate::watch::WatchAssistantMessage {
                                meta: event_meta(None),
                                model: None,
                                phase: None,
                                text: None,
                            },
                        ));
                    }
                    super::EventMsgData::ImageGeneration(data) => {
                        if let Some(event) = codex_watch_image_generation(
                            codex_watch_meta(
                                data.id.clone(),
                                message.timestamp.clone(),
                                event_turn_id.clone(),
                                None,
                            ),
                            data,
                        ) {
                            events.push(event);
                        }
                    }
                    super::EventMsgData::UserMessage(data) => {
                        events.push(crate::watch::WatchEvent::UserMessage(
                            crate::watch::WatchMessage {
                                meta: event_meta(None),
                                text: codex_watch_text(data.message.clone()),
                            },
                        ));
                    }
                    super::EventMsgData::TokenCount(data) => {
                        let info = data.info();
                        let usage = info.as_ref().and_then(|info| {
                            info.last_token_usage
                                .as_ref()
                                .or(info.total_token_usage.as_ref())
                        });
                        events.push(codex_watch_usage(
                            event_meta(None),
                            Usage {
                                model: smol_opt(info.as_ref().and_then(|info| {
                                    info.model.clone().or(info.model_name.clone())
                                })),
                                input_tokens: usage.map_or(0, |usage| usage.input_tokens),
                                output_tokens: usage.map_or(0, |usage| usage.output_tokens),
                                cache_creation_tokens: 0,
                                cache_read_tokens: usage
                                    .map_or(0, |usage| usage.cached_input_tokens),
                                cached_tokens: usage.map_or(0, |usage| usage.cached_input_tokens),
                                reasoning_tokens: usage
                                    .map_or(0, |usage| usage.reasoning_output_tokens),
                                tool_tokens: 0,
                                total_tokens: usage.map_or(0, |usage| usage.total_tokens),
                                web_search_requests: 0,
                                speed: None,
                            },
                        ));
                    }
                    super::EventMsgData::ExecCommandEnd(data) => {
                        let template = data
                            .call_id
                            .as_deref()
                            .and_then(|call_id| ops.get(call_id))
                            .cloned()
                            .unwrap_or_else(|| {
                                op_template(
                                    "exec_command".into(),
                                    data.parsed_cmd_json
                                        .as_deref()
                                        .or(data.command_json.as_deref()),
                                    None,
                                )
                            });
                        events.push(codex_watch_from_actor_body(
                            event_meta(data.call_id.clone()),
                            Actor::Tool,
                            Body::Operation(Operation {
                                kind: OperationKind::Shell,
                                phase: if data.exit_code.unwrap_or(0) == 0 {
                                    OperationPhase::Completed
                                } else {
                                    OperationPhase::Failed
                                },
                                name: template.name,
                                input_json: data
                                    .parsed_cmd_json
                                    .clone()
                                    .or_else(|| data.command_json.clone()),
                                output_json: data
                                    .aggregated_output
                                    .clone()
                                    .or_else(|| data.formatted_output.clone())
                                    .or_else(|| {
                                        Some(
                                            serde_json::json!({
                                                "stdout": data.stdout,
                                                "stderr": data.stderr,
                                                "exit_code": data.exit_code,
                                            })
                                            .to_string()
                                            .into(),
                                        )
                                    }),
                                command: template
                                    .command
                                    .or_else(|| command_from_json(data.parsed_cmd_json.as_deref()))
                                    .or_else(|| command_from_json(data.command_json.as_deref())),
                                file_path: template.file_path,
                                lines_added: template.lines_added,
                                lines_removed: template.lines_removed,
                                is_error: data.exit_code.unwrap_or(0) != 0,
                                duration_seconds: duration_from_json(data.duration_json.as_deref()),
                                extra_json: data.status.clone(),
                            }),
                        ));
                    }
                    super::EventMsgData::PatchApplyEnd(data) => {
                        events.push(codex_watch_from_actor_body(
                            event_meta(data.call_id.clone()),
                            Actor::Tool,
                            Body::Operation(Operation {
                                kind: OperationKind::Edit,
                                phase: if data.success.unwrap_or(false) {
                                    OperationPhase::Completed
                                } else {
                                    OperationPhase::Failed
                                },
                                name: "apply_patch".into(),
                                input_json: None,
                                output_json: data.changes_json.clone().or_else(|| {
                                    Some(
                                        serde_json::json!({
                                            "stdout": data.stdout,
                                            "stderr": data.stderr,
                                        })
                                        .to_string()
                                        .into(),
                                    )
                                }),
                                command: None,
                                file_path: None,
                                lines_added: 0,
                                lines_removed: 0,
                                is_error: !data.success.unwrap_or(false),
                                duration_seconds: None,
                                extra_json: data.status.clone(),
                            }),
                        ));
                    }
                    super::EventMsgData::TurnAborted(data) => {
                        if include_lifecycle {
                            let value_json = data.reason.clone().map(|reason| {
                                serde_json::json!({ "reason": reason }).to_string().into()
                            });
                            events.push(codex_watch_turn_failed(event_meta(None), value_json));
                        }
                    }
                    super::EventMsgData::ContextCompacted => {
                        if selection.includes_state() {
                            events.push(codex_watch_state(
                                event_meta(None),
                                "context_compacted",
                                None,
                            ));
                        }
                    }
                    super::EventMsgData::WebSearchEnd(data) => {
                        events.push(codex_watch_from_actor_body(
                            event_meta(data.call_id.clone()),
                            Actor::Tool,
                            Body::Operation(Operation {
                                kind: OperationKind::Web,
                                phase: OperationPhase::Completed,
                                name: "web_search".into(),
                                input_json: data.query.clone().map(|query| {
                                    serde_json::json!({ "query": query }).to_string().into()
                                }),
                                output_json: data.action_json.clone(),
                                command: None,
                                file_path: None,
                                lines_added: 0,
                                lines_removed: 0,
                                is_error: false,
                                duration_seconds: None,
                                extra_json: None,
                            }),
                        ));
                    }
                    super::EventMsgData::ThreadRolledBack(data) => {
                        if selection.includes_state() {
                            events.push(codex_watch_state(
                                event_meta(None),
                                "thread_rolled_back",
                                Some(
                                    serde_json::json!({ "num_turns": data.num_turns })
                                        .to_string()
                                        .into(),
                                ),
                            ));
                        }
                    }
                    super::EventMsgData::CollabWaitingEnd(data) => {
                        events.push(codex_watch_from_actor_body(
                            event_meta(data.call_id.clone()),
                            Actor::Subagent,
                            Body::Operation(Operation {
                                kind: OperationKind::Subagent,
                                phase: OperationPhase::Completed,
                                name: "wait_agent".into(),
                                input_json: None,
                                output_json: data.statuses_json.clone(),
                                command: None,
                                file_path: None,
                                lines_added: 0,
                                lines_removed: 0,
                                is_error: false,
                                duration_seconds: None,
                                extra_json: data.agent_statuses_json.clone(),
                            }),
                        ));
                    }
                    super::EventMsgData::McpToolCallEnd(data) => {
                        events.push(codex_watch_from_actor_body(
                            event_meta(data.call_id.clone()),
                            Actor::Tool,
                            Body::Operation(Operation {
                                kind: OperationKind::Mcp,
                                phase: OperationPhase::Completed,
                                name: "mcp_tool_call".into(),
                                input_json: data.invocation_json.clone(),
                                output_json: data.result_json.clone(),
                                command: None,
                                file_path: None,
                                lines_added: 0,
                                lines_removed: 0,
                                is_error: false,
                                duration_seconds: duration_from_json(data.duration_json.as_deref()),
                                extra_json: None,
                            }),
                        ));
                    }
                    super::EventMsgData::DynamicToolCallRequest(data) => {
                        let name = data.tool.clone().unwrap_or_else(|| "dynamic_tool".into());
                        let template = op_template(
                            name.clone(),
                            data.arguments_json.as_deref(),
                            command_from_json(data.arguments_json.as_deref()),
                        );
                        events.push(codex_watch_from_actor_body(
                            event_meta(data.call_id.clone()),
                            Actor::Tool,
                            Body::Operation(Operation {
                                kind: template.kind,
                                phase: OperationPhase::Requested,
                                name: template.name,
                                input_json: data.arguments_json.clone(),
                                output_json: None,
                                command: template.command,
                                file_path: template.file_path,
                                lines_added: template.lines_added,
                                lines_removed: template.lines_removed,
                                is_error: false,
                                duration_seconds: None,
                                extra_json: None,
                            }),
                        ));
                    }
                    super::EventMsgData::DynamicToolCallResponse(data) => {
                        let name = data.tool.clone().unwrap_or_else(|| "dynamic_tool".into());
                        let template = op_template(
                            name.clone(),
                            data.arguments_json.as_deref(),
                            command_from_json(data.arguments_json.as_deref()),
                        );
                        events.push(codex_watch_from_actor_body(
                            event_meta(data.call_id.clone()),
                            Actor::Tool,
                            Body::Operation(Operation {
                                kind: template.kind,
                                phase: if data.success.unwrap_or(true) {
                                    OperationPhase::Completed
                                } else {
                                    OperationPhase::Failed
                                },
                                name: template.name,
                                input_json: data.arguments_json.clone(),
                                output_json: data
                                    .content_items_json
                                    .clone()
                                    .or_else(|| data.error_json.clone()),
                                command: template.command,
                                file_path: template.file_path,
                                lines_added: template.lines_added,
                                lines_removed: template.lines_removed,
                                is_error: !data.success.unwrap_or(true),
                                duration_seconds: duration_from_json(data.duration_json.as_deref()),
                                extra_json: None,
                            }),
                        ));
                    }
                    super::EventMsgData::CollabAgentSpawnEnd(data) => {
                        events.push(codex_watch_from_actor_body(
                            event_meta(data.call_id.clone()),
                            Actor::Subagent,
                            Body::Operation(Operation {
                                kind: OperationKind::Subagent,
                                phase: OperationPhase::Completed,
                                name: "spawn_agent".into(),
                                input_json: data.prompt.clone().map(|prompt| {
                                    serde_json::json!({ "prompt": prompt }).to_string().into()
                                }),
                                output_json: data.status_json.clone(),
                                command: None,
                                file_path: None,
                                lines_added: 0,
                                lines_removed: 0,
                                is_error: false,
                                duration_seconds: None,
                                extra_json: Some(
                                    serde_json::json!({
                                        "sender_thread_id": data.sender_thread_id,
                                        "new_thread_id": data.new_thread_id,
                                        "new_agent_nickname": data.new_agent_nickname,
                                        "new_agent_role": data.new_agent_role,
                                        "model": data.model,
                                        "reasoning_effort": data.reasoning_effort,
                                    })
                                    .to_string()
                                    .into(),
                                ),
                            }),
                        ));
                    }
                    super::EventMsgData::CollabCloseEnd(data) => {
                        events.push(codex_watch_from_actor_body(
                            event_meta(data.call_id.clone()),
                            Actor::Subagent,
                            Body::Operation(Operation {
                                kind: OperationKind::Subagent,
                                phase: OperationPhase::Completed,
                                name: "close_agent".into(),
                                input_json: None,
                                output_json: data.status_json.clone(),
                                command: None,
                                file_path: None,
                                lines_added: 0,
                                lines_removed: 0,
                                is_error: false,
                                duration_seconds: None,
                                extra_json: Some(
                                    serde_json::json!({
                                        "sender_thread_id": data.sender_thread_id,
                                        "receiver_thread_id": data.receiver_thread_id,
                                        "receiver_agent_nickname": data.receiver_agent_nickname,
                                        "receiver_agent_role": data.receiver_agent_role,
                                    })
                                    .to_string()
                                    .into(),
                                ),
                            }),
                        ));
                    }
                    super::EventMsgData::CollabAgentInteractionEnd(data) => {
                        events.push(codex_watch_from_actor_body(
                            event_meta(data.call_id.clone()),
                            Actor::Subagent,
                            Body::Operation(Operation {
                                kind: OperationKind::Subagent,
                                phase: OperationPhase::Completed,
                                name: "agent_interaction".into(),
                                input_json: data.prompt.clone().map(|prompt| {
                                    serde_json::json!({ "prompt": prompt }).to_string().into()
                                }),
                                output_json: data.status_json.clone(),
                                command: None,
                                file_path: None,
                                lines_added: 0,
                                lines_removed: 0,
                                is_error: false,
                                duration_seconds: None,
                                extra_json: Some(
                                    serde_json::json!({
                                        "sender_thread_id": data.sender_thread_id,
                                        "receiver_thread_id": data.receiver_thread_id,
                                        "receiver_agent_nickname": data.receiver_agent_nickname,
                                        "receiver_agent_role": data.receiver_agent_role,
                                    })
                                    .to_string()
                                    .into(),
                                ),
                            }),
                        ));
                    }
                    super::EventMsgData::Error(data) => {
                        if selection.includes_state() {
                            events.push(codex_watch_state(
                                event_meta(None),
                                "error",
                                Some(
                                    serde_json::json!({
                                        "message": data.message,
                                        "codex_error_info": data.codex_error_info,
                                    })
                                    .to_string()
                                    .into(),
                                ),
                            ));
                        }
                    }
                    super::EventMsgData::EnteredReviewMode(data) => {
                        if selection.includes_state() {
                            events.push(codex_watch_state(
                                event_meta(None),
                                "entered_review_mode",
                                Some(
                                    serde_json::json!({
                                        "target_json": data.target_json,
                                        "user_facing_hint": data.user_facing_hint,
                                    })
                                    .to_string()
                                    .into(),
                                ),
                            ));
                        }
                    }
                    super::EventMsgData::ExitedReviewMode(data) => {
                        if selection.includes_state() {
                            events.push(codex_watch_state(
                                event_meta(None),
                                "exited_review_mode",
                                data.review_output_json.clone(),
                            ));
                        }
                    }
                    super::EventMsgData::Unknown(data) => {
                        events.push(crate::watch::WatchEvent::Unknown(
                            crate::watch::WatchUnknown {
                                meta: event_meta(None),
                                actor: Actor::System,
                                kind: message.kind.clone().into(),
                                raw_json: data.raw_json.clone().into(),
                            },
                        ));
                    }
                }
            }
            super::Entry::Unknown(unknown) => {
                events.push(crate::watch::WatchEvent::Unknown(
                    crate::watch::WatchUnknown {
                        meta: codex_watch_meta(
                            None,
                            unknown.timestamp.clone(),
                            current_turn_id.clone(),
                            None,
                        ),
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

fn agent_session_from_codex_body(body: &super::Body, version: super::Version) -> Session {
    let mut events = Vec::new();
    let mut current_turn_id = None::<smol_str::SmolStr>;
    let mut ops = HashMap::<String, OpTemplate>::new();

    let entries = &body.entries;
    for (entry_index, entry) in entries.iter().enumerate() {
        match entry {
            super::Entry::SessionMeta(_) => {}
            super::Entry::TurnContext(context) => {
                current_turn_id = smol_opt(context.turn_id.clone());
                let mut ev = event(
                    Actor::System,
                    Body::State(State {
                        kind: "turn_context".into(),
                        value_json: Some(
                            serde_json::json!({
                                "cwd": context.cwd,
                                "current_date": context.current_date,
                                "timezone": context.timezone,
                                "model": context.model,
                            })
                            .to_string()
                            .into(),
                        ),
                    }),
                    None,
                );
                ev.turn_id = smol_opt(context.turn_id.clone());
                events.push(ev);
            }
            super::Entry::Message(message) => {
                let blocks = map_blocks(&message.blocks);
                let actor = map_actor(&message.role);
                let body = match message.role {
                    super::Role::Assistant => Body::Response(Response {
                        model: smol_opt(message.model.clone()),
                        phase: smol_opt(message.phase.clone()),
                        text: text_from_blocks(&blocks),
                        blocks,
                    }),
                    _ => Body::Prompt(Prompt {
                        text: text_from_blocks(&blocks),
                        blocks,
                    }),
                };
                let mut ev = event(actor, body, message.timestamp.clone());
                ev.turn_id = current_turn_id.clone();
                events.push(ev);
            }
            super::Entry::FunctionCall(call) => {
                let template = op_template(
                    call.name.clone(),
                    call.arguments_json.as_deref(),
                    call.shell_command().map(crate::util::cow_to_box),
                );
                if let Some(call_id) = call.call_id.as_ref().or(call.id.as_ref()) {
                    ops.insert(call_id.to_string(), template.clone());
                }
                let mut ev = event(
                    Actor::Tool,
                    Body::Operation(Operation {
                        kind: template.kind,
                        phase: OperationPhase::Requested,
                        name: template.name.clone(),
                        input_json: call.arguments_json.clone(),
                        output_json: None,
                        command: template.command.clone(),
                        file_path: template.file_path.clone(),
                        lines_added: template.lines_added,
                        lines_removed: template.lines_removed,
                        is_error: false,
                        duration_seconds: None,
                        extra_json: None,
                    }),
                    call.timestamp.clone(),
                );
                ev.turn_id = current_turn_id.clone();
                ev.op_id = smol_opt(call.call_id.clone().or_else(|| call.id.clone()));
                ev.id = smol_opt(call.id.clone());
                events.push(ev);
            }
            super::Entry::FunctionCallOutput(output) => {
                let template = ops
                    .get(output.call_id.as_str())
                    .cloned()
                    .unwrap_or_else(|| op_template("function_call".into(), None, None));
                let mut ev = event(
                    Actor::Tool,
                    Body::Operation(Operation {
                        kind: template.kind,
                        phase: OperationPhase::Completed,
                        name: template.name,
                        input_json: None,
                        output_json: Some(output.output.clone()),
                        command: template.command,
                        file_path: template.file_path,
                        lines_added: template.lines_added,
                        lines_removed: template.lines_removed,
                        is_error: false,
                        duration_seconds: Some(output.shell_duration_seconds())
                            .filter(|seconds| *seconds > 0.0),
                        extra_json: None,
                    }),
                    output.timestamp.clone(),
                );
                ev.turn_id = current_turn_id.clone();
                ev.op_id = Some(output.call_id.clone().into());
                events.push(ev);
            }
            super::Entry::CustomToolCall(call) => {
                let template = op_template(
                    call.name.clone(),
                    call.input.as_deref(),
                    command_from_json(call.input.as_deref()),
                );
                if let Some(call_id) = &call.call_id {
                    ops.insert(call_id.to_string(), template.clone());
                }
                let mut ev = event(
                    Actor::Tool,
                    Body::Operation(Operation {
                        kind: template.kind,
                        phase: OperationPhase::Requested,
                        name: template.name.clone(),
                        input_json: call.input.clone(),
                        output_json: None,
                        command: template.command.clone(),
                        file_path: template.file_path.clone(),
                        lines_added: template.lines_added,
                        lines_removed: template.lines_removed,
                        is_error: false,
                        duration_seconds: None,
                        extra_json: call.status.clone(),
                    }),
                    call.timestamp.clone(),
                );
                ev.turn_id = current_turn_id.clone();
                ev.op_id = smol_opt(call.call_id.clone());
                events.push(ev);
            }
            super::Entry::CustomToolCallOutput(output) => {
                let template = output
                    .call_id
                    .as_deref()
                    .and_then(|call_id| ops.get(call_id))
                    .cloned()
                    .unwrap_or_else(|| op_template("custom_tool".into(), None, None));
                let mut ev = event(
                    Actor::Tool,
                    Body::Operation(Operation {
                        kind: template.kind,
                        phase: OperationPhase::Completed,
                        name: template.name,
                        input_json: None,
                        output_json: Some(output.output.clone()),
                        command: template.command,
                        file_path: template.file_path,
                        lines_added: template.lines_added,
                        lines_removed: template.lines_removed,
                        is_error: false,
                        duration_seconds: None,
                        extra_json: None,
                    }),
                    output.timestamp.clone(),
                );
                ev.turn_id = current_turn_id.clone();
                ev.op_id = smol_opt(output.call_id.clone());
                events.push(ev);
            }
            super::Entry::WebSearchCall(search) => {
                let mut ev = event(
                    Actor::Tool,
                    Body::Operation(Operation {
                        kind: OperationKind::Web,
                        phase: OperationPhase::Requested,
                        name: "web_search".into(),
                        input_json: Some(
                            serde_json::json!({
                                "status": search.status,
                                "action_type": search.action_type,
                                "query": search.query,
                                "queries": search.queries,
                            })
                            .to_string()
                            .into(),
                        ),
                        output_json: None,
                        command: None,
                        file_path: None,
                        lines_added: 0,
                        lines_removed: 0,
                        is_error: false,
                        duration_seconds: None,
                        extra_json: None,
                    }),
                    search.timestamp.clone(),
                );
                ev.turn_id = current_turn_id.clone();
                events.push(ev);
            }
            super::Entry::GhostSnapshot(snapshot) => {
                let mut ev = event(
                    Actor::System,
                    Body::Snapshot(Snapshot {
                        kind: "ghost_snapshot".into(),
                        value_json: serde_json::json!({
                            "commit_id": snapshot.commit_id,
                            "parent_id": snapshot.parent_id,
                            "preexisting_untracked_files": snapshot.preexisting_untracked_files,
                            "preexisting_untracked_dirs": snapshot.preexisting_untracked_dirs,
                        })
                        .to_string()
                        .into(),
                    }),
                    snapshot.timestamp.clone(),
                );
                ev.turn_id = current_turn_id.clone();
                events.push(ev);
            }
            super::Entry::Compacted(compacted) => {
                let mut ev = event(
                    Actor::System,
                    Body::State(State {
                        kind: "compacted".into(),
                        value_json: compacted.message.clone().map(|message| {
                            serde_json::json!({ "message": message }).to_string().into()
                        }),
                    }),
                    compacted.timestamp.clone(),
                );
                ev.turn_id = current_turn_id.clone();
                events.push(ev);
            }
            super::Entry::Reasoning(reasoning) => {
                let blocks = reasoning
                    .summary
                    .iter()
                    .map(|text| ContentBlock::Thinking(ThinkingBlock { text: text.clone() }))
                    .collect::<Vec<_>>()
                    .into_boxed_slice();
                let mut ev = event(
                    Actor::Assistant,
                    Body::Response(Response {
                        model: None,
                        phase: None,
                        text: None,
                        blocks,
                    }),
                    reasoning.timestamp.clone(),
                );
                ev.turn_id = current_turn_id.clone();
                events.push(ev);
            }
            super::Entry::EventMsg(message) => {
                let mut ev = match &message.data {
                    super::EventMsgData::TaskStarted(data) => event(
                        Actor::System,
                        Body::State(State {
                            kind: "task_started".into(),
                            value_json: Some(
                                serde_json::json!({
                                    "model_context_window": data.model_context_window,
                                    "collaboration_mode_kind": data.collaboration_mode_kind,
                                })
                                .to_string()
                                .into(),
                            ),
                        }),
                        message.timestamp.clone(),
                    ),
                    super::EventMsgData::TaskComplete(data) => event(
                        Actor::System,
                        Body::State(State {
                            kind: "task_complete".into(),
                            value_json: Some(
                                serde_json::json!({
                                    "last_agent_message": message.last_agent_message,
                                    "completed_at": data.completed_at,
                                    "duration_ms": data.duration_ms,
                                    "time_to_first_token_ms": data.time_to_first_token_ms,
                                })
                                .to_string()
                                .into(),
                            ),
                        }),
                        message.timestamp.clone(),
                    ),
                    super::EventMsgData::AgentMessage(data) => {
                        if data.message.as_deref().is_some_and(|text| {
                            is_mirrored_agent_message(entries, entry_index, text)
                        }) {
                            continue;
                        }
                        event(
                            Actor::Assistant,
                            Body::Response(Response {
                                model: None,
                                phase: smol_opt(data.phase.clone()),
                                text: data.message.clone(),
                                blocks: data
                                    .message
                                    .as_ref()
                                    .map(|text| {
                                        vec![ContentBlock::Text(TextBlock { text: text.clone() })]
                                            .into_boxed_slice()
                                    })
                                    .unwrap_or_default(),
                            }),
                            message.timestamp.clone(),
                        )
                    }
                    super::EventMsgData::AgentReasoning(data) => event(
                        Actor::Assistant,
                        Body::Response(Response {
                            model: None,
                            phase: None,
                            text: None,
                            blocks: data
                                .text
                                .as_ref()
                                .map(|text| {
                                    vec![ContentBlock::Thinking(ThinkingBlock {
                                        text: text.clone(),
                                    })]
                                    .into_boxed_slice()
                                })
                                .unwrap_or_default(),
                        }),
                        message.timestamp.clone(),
                    ),
                    super::EventMsgData::ImageGeneration(data) => event(
                        Actor::Assistant,
                        Body::Response(Response {
                            model: None,
                            phase: None,
                            text: None,
                            blocks: vec![ContentBlock::Raw(RawBlock {
                                kind: "image_generation".into(),
                                raw_json: serde_json::json!({
                                    "id": data.id,
                                    "status": data.status,
                                    "revised_prompt": data.revised_prompt,
                                    "has_result": data
                                        .result_base64
                                        .as_deref()
                                        .is_some_and(|result| !result.trim().is_empty()),
                                })
                                .to_string()
                                .into(),
                            })]
                            .into_boxed_slice(),
                        }),
                        message.timestamp.clone(),
                    ),
                    super::EventMsgData::UserMessage(data) => {
                        let mut blocks = Vec::new();
                        if let Some(text) = data.message.as_ref() {
                            blocks.push(ContentBlock::Text(TextBlock { text: text.clone() }));
                        }
                        if let Some(raw) = data.images_json.as_ref() {
                            blocks.push(ContentBlock::Raw(RawBlock {
                                kind: "images".into(),
                                raw_json: raw.clone(),
                            }));
                        }
                        if let Some(raw) = data.local_images_json.as_ref() {
                            blocks.push(ContentBlock::Raw(RawBlock {
                                kind: "local_images".into(),
                                raw_json: raw.clone(),
                            }));
                        }
                        if let Some(raw) = data.text_elements_json.as_ref() {
                            blocks.push(ContentBlock::Raw(RawBlock {
                                kind: "text_elements".into(),
                                raw_json: raw.clone(),
                            }));
                        }
                        event(
                            Actor::User,
                            Body::Prompt(Prompt {
                                text: data.message.clone(),
                                blocks: blocks.into_boxed_slice(),
                            }),
                            message.timestamp.clone(),
                        )
                    }
                    super::EventMsgData::TokenCount(data) => {
                        let info = data.info();
                        let usage = info.as_ref().and_then(|info| {
                            info.last_token_usage
                                .as_ref()
                                .or(info.total_token_usage.as_ref())
                        });
                        event(
                            Actor::System,
                            Body::Usage(Usage {
                                model: smol_opt(info.as_ref().and_then(|info| {
                                    info.model.clone().or(info.model_name.clone())
                                })),
                                input_tokens: usage.map_or(0, |usage| usage.input_tokens),
                                output_tokens: usage.map_or(0, |usage| usage.output_tokens),
                                cache_creation_tokens: 0,
                                cache_read_tokens: usage
                                    .map_or(0, |usage| usage.cached_input_tokens),
                                cached_tokens: usage.map_or(0, |usage| usage.cached_input_tokens),
                                reasoning_tokens: usage
                                    .map_or(0, |usage| usage.reasoning_output_tokens),
                                tool_tokens: 0,
                                total_tokens: usage.map_or(0, |usage| usage.total_tokens),
                                web_search_requests: 0,
                                speed: None,
                            }),
                            message.timestamp.clone(),
                        )
                    }
                    super::EventMsgData::ExecCommandEnd(data) => {
                        let template = data
                            .call_id
                            .as_deref()
                            .and_then(|call_id| ops.get(call_id))
                            .cloned()
                            .unwrap_or_else(|| {
                                op_template(
                                    "exec_command".into(),
                                    data.parsed_cmd_json
                                        .as_deref()
                                        .or(data.command_json.as_deref()),
                                    None,
                                )
                            });
                        let mut ev = event(
                            Actor::Tool,
                            Body::Operation(Operation {
                                kind: OperationKind::Shell,
                                phase: if data.exit_code.unwrap_or(0) == 0 {
                                    OperationPhase::Completed
                                } else {
                                    OperationPhase::Failed
                                },
                                name: template.name,
                                input_json: data
                                    .parsed_cmd_json
                                    .clone()
                                    .or_else(|| data.command_json.clone()),
                                output_json: data
                                    .aggregated_output
                                    .clone()
                                    .or_else(|| data.formatted_output.clone())
                                    .or_else(|| {
                                        Some(
                                            serde_json::json!({
                                                "stdout": data.stdout,
                                                "stderr": data.stderr,
                                                "exit_code": data.exit_code,
                                            })
                                            .to_string()
                                            .into(),
                                        )
                                    }),
                                command: template
                                    .command
                                    .or_else(|| command_from_json(data.parsed_cmd_json.as_deref()))
                                    .or_else(|| command_from_json(data.command_json.as_deref())),
                                file_path: template.file_path,
                                lines_added: template.lines_added,
                                lines_removed: template.lines_removed,
                                is_error: data.exit_code.unwrap_or(0) != 0,
                                duration_seconds: duration_from_json(data.duration_json.as_deref()),
                                extra_json: data.status.clone(),
                            }),
                            message.timestamp.clone(),
                        );
                        ev.op_id = smol_opt(data.call_id.clone());
                        ev
                    }
                    super::EventMsgData::PatchApplyEnd(data) => {
                        let mut ev = event(
                            Actor::Tool,
                            Body::Operation(Operation {
                                kind: OperationKind::Edit,
                                phase: if data.success.unwrap_or(false) {
                                    OperationPhase::Completed
                                } else {
                                    OperationPhase::Failed
                                },
                                name: "apply_patch".into(),
                                input_json: None,
                                output_json: data.changes_json.clone().or_else(|| {
                                    Some(
                                        serde_json::json!({
                                            "stdout": data.stdout,
                                            "stderr": data.stderr,
                                        })
                                        .to_string()
                                        .into(),
                                    )
                                }),
                                command: None,
                                file_path: None,
                                lines_added: 0,
                                lines_removed: 0,
                                is_error: !data.success.unwrap_or(false),
                                duration_seconds: None,
                                extra_json: data.status.clone(),
                            }),
                            message.timestamp.clone(),
                        );
                        ev.op_id = smol_opt(data.call_id.clone());
                        ev
                    }
                    super::EventMsgData::TurnAborted(data) => event(
                        Actor::System,
                        Body::State(State {
                            kind: "turn_aborted".into(),
                            value_json: data.reason.clone().map(|reason| {
                                serde_json::json!({ "reason": reason }).to_string().into()
                            }),
                        }),
                        message.timestamp.clone(),
                    ),
                    super::EventMsgData::ContextCompacted => event(
                        Actor::System,
                        Body::State(State {
                            kind: "context_compacted".into(),
                            value_json: None,
                        }),
                        message.timestamp.clone(),
                    ),
                    super::EventMsgData::WebSearchEnd(data) => {
                        let mut ev = event(
                            Actor::Tool,
                            Body::Operation(Operation {
                                kind: OperationKind::Web,
                                phase: OperationPhase::Completed,
                                name: "web_search".into(),
                                input_json: data.query.clone().map(|query| {
                                    serde_json::json!({ "query": query }).to_string().into()
                                }),
                                output_json: data.action_json.clone(),
                                command: None,
                                file_path: None,
                                lines_added: 0,
                                lines_removed: 0,
                                is_error: false,
                                duration_seconds: None,
                                extra_json: None,
                            }),
                            message.timestamp.clone(),
                        );
                        ev.op_id = smol_opt(data.call_id.clone());
                        ev
                    }
                    super::EventMsgData::ThreadRolledBack(data) => event(
                        Actor::System,
                        Body::State(State {
                            kind: "thread_rolled_back".into(),
                            value_json: Some(
                                serde_json::json!({ "num_turns": data.num_turns })
                                    .to_string()
                                    .into(),
                            ),
                        }),
                        message.timestamp.clone(),
                    ),
                    super::EventMsgData::CollabWaitingEnd(data) => {
                        let mut ev = event(
                            Actor::Subagent,
                            Body::Operation(Operation {
                                kind: OperationKind::Subagent,
                                phase: OperationPhase::Completed,
                                name: "wait_agent".into(),
                                input_json: None,
                                output_json: data.statuses_json.clone(),
                                command: None,
                                file_path: None,
                                lines_added: 0,
                                lines_removed: 0,
                                is_error: false,
                                duration_seconds: None,
                                extra_json: data.agent_statuses_json.clone(),
                            }),
                            message.timestamp.clone(),
                        );
                        ev.op_id = smol_opt(data.call_id.clone());
                        ev
                    }
                    super::EventMsgData::McpToolCallEnd(data) => {
                        let mut ev = event(
                            Actor::Tool,
                            Body::Operation(Operation {
                                kind: OperationKind::Mcp,
                                phase: OperationPhase::Completed,
                                name: "mcp_tool_call".into(),
                                input_json: data.invocation_json.clone(),
                                output_json: data.result_json.clone(),
                                command: None,
                                file_path: None,
                                lines_added: 0,
                                lines_removed: 0,
                                is_error: false,
                                duration_seconds: duration_from_json(data.duration_json.as_deref()),
                                extra_json: None,
                            }),
                            message.timestamp.clone(),
                        );
                        ev.op_id = smol_opt(data.call_id.clone());
                        ev
                    }
                    super::EventMsgData::DynamicToolCallRequest(data) => {
                        let name = data.tool.clone().unwrap_or_else(|| "dynamic_tool".into());
                        let template = op_template(
                            name.clone(),
                            data.arguments_json.as_deref(),
                            command_from_json(data.arguments_json.as_deref()),
                        );
                        let mut ev = event(
                            Actor::Tool,
                            Body::Operation(Operation {
                                kind: template.kind,
                                phase: OperationPhase::Requested,
                                name: template.name,
                                input_json: data.arguments_json.clone(),
                                output_json: None,
                                command: template.command,
                                file_path: template.file_path,
                                lines_added: template.lines_added,
                                lines_removed: template.lines_removed,
                                is_error: false,
                                duration_seconds: None,
                                extra_json: None,
                            }),
                            message.timestamp.clone(),
                        );
                        ev.op_id = smol_opt(data.call_id.clone());
                        ev
                    }
                    super::EventMsgData::DynamicToolCallResponse(data) => {
                        let name = data.tool.clone().unwrap_or_else(|| "dynamic_tool".into());
                        let template = op_template(
                            name.clone(),
                            data.arguments_json.as_deref(),
                            command_from_json(data.arguments_json.as_deref()),
                        );
                        let mut ev = event(
                            Actor::Tool,
                            Body::Operation(Operation {
                                kind: template.kind,
                                phase: if data.success.unwrap_or(true) {
                                    OperationPhase::Completed
                                } else {
                                    OperationPhase::Failed
                                },
                                name: template.name,
                                input_json: data.arguments_json.clone(),
                                output_json: data
                                    .content_items_json
                                    .clone()
                                    .or_else(|| data.error_json.clone()),
                                command: template.command,
                                file_path: template.file_path,
                                lines_added: template.lines_added,
                                lines_removed: template.lines_removed,
                                is_error: !data.success.unwrap_or(true),
                                duration_seconds: duration_from_json(data.duration_json.as_deref()),
                                extra_json: None,
                            }),
                            message.timestamp.clone(),
                        );
                        ev.op_id = smol_opt(data.call_id.clone());
                        ev
                    }
                    super::EventMsgData::CollabAgentSpawnEnd(data) => {
                        let mut ev = event(
                            Actor::Subagent,
                            Body::Operation(Operation {
                                kind: OperationKind::Subagent,
                                phase: OperationPhase::Completed,
                                name: "spawn_agent".into(),
                                input_json: data.prompt.clone().map(|prompt| {
                                    serde_json::json!({ "prompt": prompt }).to_string().into()
                                }),
                                output_json: data.status_json.clone(),
                                command: None,
                                file_path: None,
                                lines_added: 0,
                                lines_removed: 0,
                                is_error: false,
                                duration_seconds: None,
                                extra_json: Some(
                                    serde_json::json!({
                                        "sender_thread_id": data.sender_thread_id,
                                        "new_thread_id": data.new_thread_id,
                                        "new_agent_nickname": data.new_agent_nickname,
                                        "new_agent_role": data.new_agent_role,
                                        "model": data.model,
                                        "reasoning_effort": data.reasoning_effort,
                                    })
                                    .to_string()
                                    .into(),
                                ),
                            }),
                            message.timestamp.clone(),
                        );
                        ev.op_id = smol_opt(data.call_id.clone());
                        ev
                    }
                    super::EventMsgData::CollabCloseEnd(data) => {
                        let mut ev = event(
                            Actor::Subagent,
                            Body::Operation(Operation {
                                kind: OperationKind::Subagent,
                                phase: OperationPhase::Completed,
                                name: "close_agent".into(),
                                input_json: None,
                                output_json: data.status_json.clone(),
                                command: None,
                                file_path: None,
                                lines_added: 0,
                                lines_removed: 0,
                                is_error: false,
                                duration_seconds: None,
                                extra_json: Some(
                                    serde_json::json!({
                                        "sender_thread_id": data.sender_thread_id,
                                        "receiver_thread_id": data.receiver_thread_id,
                                        "receiver_agent_nickname": data.receiver_agent_nickname,
                                        "receiver_agent_role": data.receiver_agent_role,
                                    })
                                    .to_string()
                                    .into(),
                                ),
                            }),
                            message.timestamp.clone(),
                        );
                        ev.op_id = smol_opt(data.call_id.clone());
                        ev
                    }
                    super::EventMsgData::CollabAgentInteractionEnd(data) => {
                        let mut ev = event(
                            Actor::Subagent,
                            Body::Operation(Operation {
                                kind: OperationKind::Subagent,
                                phase: OperationPhase::Completed,
                                name: "agent_interaction".into(),
                                input_json: data.prompt.clone().map(|prompt| {
                                    serde_json::json!({ "prompt": prompt }).to_string().into()
                                }),
                                output_json: data.status_json.clone(),
                                command: None,
                                file_path: None,
                                lines_added: 0,
                                lines_removed: 0,
                                is_error: false,
                                duration_seconds: None,
                                extra_json: Some(
                                    serde_json::json!({
                                        "sender_thread_id": data.sender_thread_id,
                                        "receiver_thread_id": data.receiver_thread_id,
                                        "receiver_agent_nickname": data.receiver_agent_nickname,
                                        "receiver_agent_role": data.receiver_agent_role,
                                    })
                                    .to_string()
                                    .into(),
                                ),
                            }),
                            message.timestamp.clone(),
                        );
                        ev.op_id = smol_opt(data.call_id.clone());
                        ev
                    }
                    super::EventMsgData::Error(data) => event(
                        Actor::System,
                        Body::State(State {
                            kind: "error".into(),
                            value_json: Some(
                                serde_json::json!({
                                    "message": data.message,
                                    "codex_error_info": data.codex_error_info,
                                })
                                .to_string()
                                .into(),
                            ),
                        }),
                        message.timestamp.clone(),
                    ),
                    super::EventMsgData::EnteredReviewMode(data) => event(
                        Actor::System,
                        Body::State(State {
                            kind: "entered_review_mode".into(),
                            value_json: Some(
                                serde_json::json!({
                                    "target_json": data.target_json,
                                    "user_facing_hint": data.user_facing_hint,
                                })
                                .to_string()
                                .into(),
                            ),
                        }),
                        message.timestamp.clone(),
                    ),
                    super::EventMsgData::ExitedReviewMode(data) => event(
                        Actor::System,
                        Body::State(State {
                            kind: "exited_review_mode".into(),
                            value_json: data.review_output_json.clone(),
                        }),
                        message.timestamp.clone(),
                    ),
                    super::EventMsgData::Unknown(data) => event(
                        Actor::System,
                        Body::Unknown(Unknown {
                            kind: message.kind.clone().into(),
                            raw_json: data.raw_json.clone(),
                        }),
                        message.timestamp.clone(),
                    ),
                };
                ev.turn_id = smol_opt(message.turn_id.clone()).or_else(|| current_turn_id.clone());
                ev.id = smol_opt(message.last_agent_message.clone());
                ev.op_id = smol_opt(event_op_id(&message.data));
                events.push(ev);
            }
            super::Entry::Unknown(unknown) => {
                let mut ev = event(
                    Actor::System,
                    Body::Unknown(Unknown {
                        kind: unknown.kind.clone().into(),
                        raw_json: unknown.raw_json.clone(),
                    }),
                    unknown.timestamp.clone(),
                );
                ev.turn_id = current_turn_id.clone();
                events.push(ev);
            }
        }
    }

    let events = events.into_boxed_slice();
    Session {
        agent: AgentKind::new("codex"),
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
    let (version, body) = super::parse_codex_reader(reader, codex_projection_selection(selection))?;
    Ok(crate::agent_session::filter_selection(
        agent_session_from_codex_body(&body, version),
        selection,
    ))
}

#[cfg(feature = "watch")]
impl crate::watch::provider::ProviderWatchEvents for crate::Codex {
    fn parse_watch_reader<R>(
        _path: &std::path::Path,
        reader: R,
        selection: crate::ParseSelection,
    ) -> crate::Result<crate::watch::provider::ParsedWatchSession>
    where
        R: std::io::BufRead,
    {
        let (_version, body) =
            super::parse_codex_reader(reader, codex_projection_selection(selection))?;
        let (session_id, cwd) = codex_watch_meta_fields(&body.entries);
        Ok(crate::watch::provider::ParsedWatchSession {
            session_id,
            cwd,
            events: watch_events_from_codex_entries(&body.entries, selection),
        })
    }

    fn parse_watch_metadata_reader<R>(
        _path: &std::path::Path,
        reader: R,
    ) -> crate::Result<crate::watch::provider::ParsedWatchSession>
    where
        R: std::io::BufRead,
    {
        let Some(meta) = crate::Codex::probe_session_meta(reader)? else {
            return Err(crate::Error::Detection { agent: "codex" });
        };
        Ok(crate::watch::provider::parsed_watch_metadata(
            meta.session_id,
            meta.cwd,
        ))
    }

    fn needs_watch_state_seed() -> bool {
        true
    }

    fn seed_watch_state(
        events: &[crate::watch::WatchEvent],
    ) -> crate::watch::state::ProviderWatchState {
        codex_seed_response_fingerprint_watch_state(events)
    }

    fn dedupe_watch_events(
        events: Box<[crate::watch::WatchEvent]>,
        state: &mut crate::watch::state::ProviderWatchState,
    ) -> Box<[crate::watch::WatchEvent]> {
        codex_dedupe_response_fingerprint_events(events, state)
    }

    fn initial_watch_directory_depth() -> Option<usize> {
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
        let depth = path
            .strip_prefix(root)
            .map(|relative| relative.components().count())
            .unwrap_or(0);
        depth <= 2 || is_recent
    }
}

#[cfg(feature = "watch")]
fn codex_seed_response_fingerprint_watch_state(
    events: &[crate::watch::WatchEvent],
) -> crate::watch::state::ProviderWatchState {
    let mut state = crate::watch::state::ProviderWatchState::with_last_prompt_timestamp(
        crate::watch::provider::latest_prompt_timestamp(events),
    );
    codex_seed_response_fingerprint_events(events, &mut state);
    state
}

#[cfg(feature = "watch")]
fn codex_seed_response_fingerprint_events(
    events: &[crate::watch::WatchEvent],
    state: &mut crate::watch::state::ProviderWatchState,
) {
    let mut prompt_timestamp = state.last_prompt_timestamp.clone();
    for event in events {
        if event.user_text().is_some() {
            prompt_timestamp = event.cloned_timestamp();
        }
        if let Some(fingerprint) = codex_event_fingerprint(event, prompt_timestamp.as_deref()) {
            state.insert_event_fingerprint(fingerprint);
        }
    }
}

#[cfg(feature = "watch")]
fn codex_dedupe_response_fingerprint_events(
    events: Box<[crate::watch::WatchEvent]>,
    state: &mut crate::watch::state::ProviderWatchState,
) -> Box<[crate::watch::WatchEvent]> {
    let mut out = Vec::with_capacity(events.len());
    let mut prompt_timestamp = state.last_prompt_timestamp.clone();
    for event in events {
        if event.user_text().is_some() {
            prompt_timestamp = event.cloned_timestamp();
        }
        let is_new = codex_event_fingerprint(&event, prompt_timestamp.as_deref())
            .is_none_or(|fingerprint| state.insert_event_fingerprint(fingerprint));
        if is_new {
            out.push(event);
        }
    }
    out.into_boxed_slice()
}

#[cfg(feature = "watch")]
fn codex_event_fingerprint(
    event: &crate::watch::WatchEvent,
    prompt_timestamp: Option<&str>,
) -> Option<u64> {
    let mut hasher = DefaultHasher::new();
    match event {
        crate::watch::WatchEvent::AssistantMessage(response) => {
            let text = response.text.as_deref()?;
            "assistant-response".hash(&mut hasher);
            prompt_timestamp.hash(&mut hasher);
            response.phase.as_deref().hash(&mut hasher);
            text.hash(&mut hasher);
        }
        crate::watch::WatchEvent::Attachment(attachment) => {
            "attachment".hash(&mut hasher);
            prompt_timestamp.hash(&mut hasher);
            attachment.id.as_deref().hash(&mut hasher);
            attachment.filename.hash(&mut hasher);
            attachment.media_type.hash(&mut hasher);
            attachment.data_base64.hash(&mut hasher);
        }
        _ => return None,
    }
    Some(hasher.finish())
}

#[cfg(all(test, feature = "watch"))]
mod watch_attachment_tests {
    #[test]
    fn codex_watch_image_generation_emits_attachment() {
        let png = "iVBORw0KGgpib2R5".to_string();
        let raw = format!(
            r#"{{"timestamp":"2026-05-19T03:14:51.660Z","type":"event_msg","payload":{{"type":"imageGeneration","id":"ig_watch","status":"generating","revisedPrompt":"watch caption","result":"{png}"}}}}"#
        );
        let (_version, body) =
            super::super::parse_codex_reader(raw.as_bytes(), crate::ParseSelection::full())
                .expect("parse codex session");
        let events =
            super::watch_events_from_codex_entries(&body.entries, crate::ParseSelection::full());

        let attachment = events
            .iter()
            .find_map(|event| match event {
                crate::watch::WatchEvent::Attachment(attachment) => Some(attachment),
                _ => None,
            })
            .expect("watch attachment");

        assert_eq!(attachment.id.as_deref(), Some("ig_watch"));
        assert_eq!(attachment.filename.as_str(), "codex-image-ig_watch.png");
        assert_eq!(attachment.media_type.as_str(), "image/png");
        assert_eq!(attachment.data_base64.as_str(), png);
        assert_eq!(attachment.caption.as_deref(), Some("watch caption"));
    }
}
