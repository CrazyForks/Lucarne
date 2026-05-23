use std::collections::HashMap;

use crate::agent_session::*;
use smol_str::SmolStr;

fn copilot_projection_selection(selection: crate::ParseSelection) -> crate::ParseSelection {
    selection
}

#[derive(Clone)]
struct OpTemplate {
    kind: OperationKind,
    name: smol_str::SmolStr,
    command: Option<SmolStr>,
}

#[derive(Clone, Copy, Default)]
struct ShutdownUsageDelta {
    input_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    reasoning_tokens: u64,
    total_tokens: u64,
}

#[derive(Clone)]
struct PendingAssistantMessage {
    record_index: usize,
    explicit_model: Option<SmolStr>,
    output_tokens: u64,
}

fn shutdown_total_tokens(usage: &super::ShutdownModelUsage) -> u64 {
    usage.input_tokens
        + usage.output_tokens
        + usage.cache_read_tokens
        + usage.cache_write_tokens
        + usage.reasoning_tokens
}

fn saturating_shutdown_delta(
    current: &super::ShutdownModelUsage,
    previous: Option<&super::ShutdownModelUsage>,
) -> ShutdownUsageDelta {
    ShutdownUsageDelta {
        input_tokens: current
            .input_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.input_tokens)),
        cache_read_tokens: current
            .cache_read_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.cache_read_tokens)),
        cache_write_tokens: current
            .cache_write_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.cache_write_tokens)),
        reasoning_tokens: current
            .reasoning_tokens
            .saturating_sub(previous.map_or(0, |usage| usage.reasoning_tokens)),
        total_tokens: shutdown_total_tokens(current)
            .saturating_sub(previous.map_or(0, shutdown_total_tokens)),
    }
}

fn sum_shutdown_deltas<'a>(
    deltas: impl Iterator<Item = &'a ShutdownUsageDelta>,
) -> ShutdownUsageDelta {
    let mut total = ShutdownUsageDelta::default();
    for delta in deltas {
        total.input_tokens += delta.input_tokens;
        total.cache_read_tokens += delta.cache_read_tokens;
        total.cache_write_tokens += delta.cache_write_tokens;
        total.reasoning_tokens += delta.reasoning_tokens;
        total.total_tokens += delta.total_tokens;
    }
    total
}

fn explicit_tool_call_models(body: &super::CliEventsBody) -> HashMap<String, SmolStr> {
    let mut models = HashMap::new();
    for record in body.records.iter() {
        match record {
            super::CliRecord::ToolExecution(super::ToolExecution::Start {
                tool_call_id: Some(tool_call_id),
                model: Some(model),
                ..
            })
            | super::CliRecord::ToolExecution(super::ToolExecution::Complete {
                tool_call_id: Some(tool_call_id),
                model: Some(model),
                ..
            }) => {
                models.insert(tool_call_id.to_string(), model.clone());
            }
            _ => {}
        }
    }
    models
}

fn explicit_message_model(
    message: &super::CliMessage,
    tool_call_models: &HashMap<String, SmolStr>,
) -> Option<SmolStr> {
    message
        .parent_tool_call_id
        .as_deref()
        .and_then(|call_id| tool_call_models.get(call_id))
        .cloned()
}

fn proportional_allocations(total: u64, weights: &[u64]) -> Vec<u64> {
    if weights.is_empty() {
        return Vec::new();
    }
    let weight_sum: u64 = weights.iter().sum();
    if weight_sum == 0 {
        let mut allocations = vec![total / weights.len() as u64; weights.len()];
        let mut remainder = total % weights.len() as u64;
        for allocation in &mut allocations {
            if remainder == 0 {
                break;
            }
            *allocation += 1;
            remainder -= 1;
        }
        return allocations;
    }

    let mut allocations = Vec::with_capacity(weights.len());
    let mut used = 0u64;
    for (index, weight) in weights.iter().enumerate() {
        if index + 1 == weights.len() {
            allocations.push(total.saturating_sub(used));
        } else {
            let share = total.saturating_mul(*weight) / weight_sum;
            allocations.push(share);
            used += share;
        }
    }
    allocations
}

fn shutdown_usage_allocations(body: &super::CliEventsBody) -> HashMap<usize, ShutdownUsageDelta> {
    let mut allocations = HashMap::new();
    let tool_call_models = explicit_tool_call_models(body);
    let mut pending_messages = Vec::<PendingAssistantMessage>::new();
    let mut last_usage_by_model = HashMap::<String, super::ShutdownModelUsage>::new();

    for (index, record) in body.records.iter().enumerate() {
        match record {
            super::CliRecord::Message(message)
                if matches!(message.role, super::Role::Assistant) =>
            {
                pending_messages.push(PendingAssistantMessage {
                    record_index: index,
                    explicit_model: explicit_message_model(message, &tool_call_models),
                    output_tokens: message.output_tokens.unwrap_or(0),
                });
            }
            super::CliRecord::SessionEvent(super::SessionEvent::Shutdown {
                model_usages, ..
            }) => {
                let mut remaining_deltas = HashMap::<String, ShutdownUsageDelta>::new();
                for usage in model_usages.iter().cloned() {
                    let model_key = usage.model.to_string();
                    let previous_usage = last_usage_by_model.get(&model_key);
                    remaining_deltas.insert(
                        model_key.clone(),
                        saturating_shutdown_delta(&usage, previous_usage),
                    );
                    last_usage_by_model.insert(model_key, usage);
                }

                let mut exactly_assigned = vec![false; pending_messages.len()];
                for (model_key, delta) in remaining_deltas.iter_mut() {
                    let matching_indices: Vec<usize> = pending_messages
                        .iter()
                        .enumerate()
                        .filter_map(|(pending_index, pending_message)| {
                            (pending_message.explicit_model.as_deref() == Some(model_key.as_str()))
                                .then_some(pending_index)
                        })
                        .collect();
                    if matching_indices.is_empty() {
                        continue;
                    }

                    let weights: Vec<u64> = matching_indices
                        .iter()
                        .map(|pending_index| pending_messages[*pending_index].output_tokens.max(1))
                        .collect();
                    let input_allocations = proportional_allocations(delta.input_tokens, &weights);
                    let cache_read_allocations =
                        proportional_allocations(delta.cache_read_tokens, &weights);
                    let cache_write_allocations =
                        proportional_allocations(delta.cache_write_tokens, &weights);
                    let reasoning_allocations =
                        proportional_allocations(delta.reasoning_tokens, &weights);
                    let total_allocations = proportional_allocations(delta.total_tokens, &weights);

                    for (
                        pending_index,
                        (
                            (
                                ((input_tokens, cache_read_tokens), cache_write_tokens),
                                reasoning_tokens,
                            ),
                            total_tokens,
                        ),
                    ) in matching_indices.into_iter().zip(
                        input_allocations
                            .into_iter()
                            .zip(cache_read_allocations)
                            .zip(cache_write_allocations)
                            .zip(reasoning_allocations)
                            .zip(total_allocations),
                    ) {
                        exactly_assigned[pending_index] = true;
                        let pending_message = &pending_messages[pending_index];
                        allocations.insert(
                            pending_message.record_index,
                            ShutdownUsageDelta {
                                input_tokens,
                                cache_read_tokens,
                                cache_write_tokens,
                                reasoning_tokens,
                                total_tokens,
                            },
                        );
                    }

                    *delta = ShutdownUsageDelta::default();
                }

                let fallback_indices: Vec<usize> = pending_messages
                    .iter()
                    .enumerate()
                    .filter_map(|(pending_index, _)| {
                        (!exactly_assigned[pending_index]).then_some(pending_index)
                    })
                    .collect();
                if !fallback_indices.is_empty() {
                    let fallback_delta = sum_shutdown_deltas(remaining_deltas.values());
                    let weights: Vec<u64> = fallback_indices
                        .iter()
                        .map(|pending_index| pending_messages[*pending_index].output_tokens.max(1))
                        .collect();
                    let input_allocations =
                        proportional_allocations(fallback_delta.input_tokens, &weights);
                    let cache_read_allocations =
                        proportional_allocations(fallback_delta.cache_read_tokens, &weights);
                    let cache_write_allocations =
                        proportional_allocations(fallback_delta.cache_write_tokens, &weights);
                    let reasoning_allocations =
                        proportional_allocations(fallback_delta.reasoning_tokens, &weights);
                    let total_allocations =
                        proportional_allocations(fallback_delta.total_tokens, &weights);

                    for (
                        pending_index,
                        (
                            (
                                ((input_tokens, cache_read_tokens), cache_write_tokens),
                                reasoning_tokens,
                            ),
                            total_tokens,
                        ),
                    ) in fallback_indices.into_iter().zip(
                        input_allocations
                            .into_iter()
                            .zip(cache_read_allocations)
                            .zip(cache_write_allocations)
                            .zip(reasoning_allocations)
                            .zip(total_allocations),
                    ) {
                        let pending_message = &pending_messages[pending_index];
                        allocations.insert(
                            pending_message.record_index,
                            ShutdownUsageDelta {
                                input_tokens,
                                cache_read_tokens,
                                cache_write_tokens,
                                reasoning_tokens,
                                total_tokens,
                            },
                        );
                    }
                }

                pending_messages.clear();
            }
            _ => {}
        }
    }

    allocations
}

fn event_version(version: super::Version) -> VersionKind {
    match version {
        super::Version::CliEventsV1 => VersionKind::new("copilot-cli-events-v1"),
        super::Version::ChatSessionV1 => VersionKind::new("copilot-chat-session-v1"),
    }
}

fn operation_kind(name: &str, command: Option<&str>) -> OperationKind {
    if matches!(name, "bash" | "shell_command" | "run_shell_command") || command.is_some() {
        OperationKind::Shell
    } else if name.contains("search") {
        OperationKind::Search
    } else if name.contains("read") {
        OperationKind::Read
    } else if name.contains("write") {
        OperationKind::Write
    } else if name.contains("edit") || name.contains("patch") {
        OperationKind::Edit
    } else if name.contains("agent") {
        OperationKind::Subagent
    } else {
        OperationKind::Custom(name.into())
    }
}

fn cli_actor(role: &super::Role) -> Actor {
    match role {
        super::Role::User => Actor::User,
        super::Role::Assistant => Actor::Assistant,
    }
}

fn chat_timestamp(timestamp: Option<i64>) -> Option<SmolStr> {
    timestamp.map(|value| value.to_string().into())
}

fn response_text(parts: &[super::ChatResponsePart]) -> Option<SmolStr> {
    let parts: Vec<&str> = parts
        .iter()
        .filter_map(|part| part.text.as_deref())
        .filter(|text| !text.is_empty())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" ").into())
    }
}

fn chat_events(body: &super::ChatSessionBody) -> Box<[Event]> {
    let mut events = Vec::new();

    for request in &body.requests {
        let timestamp = chat_timestamp(request.timestamp);
        let mut prompt = event(
            Actor::User,
            Body::Prompt(Prompt {
                text: request.prompt.clone(),
                blocks: request
                    .prompt
                    .as_ref()
                    .map(|text| {
                        vec![ContentBlock::Text(TextBlock { text: text.clone() })]
                            .into_boxed_slice()
                    })
                    .unwrap_or_default(),
            }),
            timestamp.clone(),
        );
        prompt.id = Some(request.request_id.clone().into());
        prompt.turn_id = Some(request.request_id.clone().into());
        events.push(prompt);

        let mut response = event(
            Actor::Assistant,
            Body::Response(Response {
                model: smol_opt(request.model_id.clone().or_else(|| body.model.clone())),
                phase: None,
                text: response_text(&request.response),
                blocks: response_text(&request.response)
                    .map(|text| vec![ContentBlock::Text(TextBlock { text })].into_boxed_slice())
                    .unwrap_or_default(),
            }),
            timestamp,
        );
        response.turn_id = Some(request.request_id.clone().into());
        events.push(response);
    }

    events.into_boxed_slice()
}

fn cli_events(body: &super::CliEventsBody) -> Box<[Event]> {
    let mut events = Vec::new();
    let mut current_turn_id = None::<smol_str::SmolStr>;
    let mut current_model = None::<SmolStr>;
    let tool_call_models = explicit_tool_call_models(body);
    let mut ops = HashMap::<String, OpTemplate>::new();
    let usage_allocations = shutdown_usage_allocations(body);

    for (record_index, record) in body.records.iter().enumerate() {
        match record {
            super::CliRecord::Message(message) => {
                let actor = cli_actor(&message.role);
                let usage_allocation = usage_allocations.get(&record_index).copied();
                let skip_empty_assistant = matches!(message.role, super::Role::Assistant)
                    && message.output_tokens == Some(0)
                    && message.content.is_empty()
                    && message.tool_requests.is_empty()
                    && usage_allocation.is_none();
                if skip_empty_assistant {
                    continue;
                }
                let blocks = if message.content.is_empty() {
                    Box::default()
                } else {
                    vec![ContentBlock::Text(TextBlock {
                        text: message.content.clone(),
                    })]
                    .into_boxed_slice()
                };
                let body = match message.role {
                    super::Role::User => Body::Prompt(Prompt {
                        text: Some(message.content.clone()),
                        blocks,
                    }),
                    super::Role::Assistant => Body::Response(Response {
                        model: smol_opt(
                            explicit_message_model(message, &tool_call_models)
                                .or_else(|| current_model.clone()),
                        ),
                        phase: None,
                        text: Some(message.content.clone()),
                        blocks,
                    }),
                };
                let mut ev = event(actor, body, message.timestamp.clone());
                ev.id = smol_opt(message.message_id.clone());
                ev.turn_id = current_turn_id.clone();
                events.push(ev);

                if matches!(message.role, super::Role::Assistant)
                    && (message.output_tokens.is_some() || usage_allocation.is_some())
                {
                    let output_tokens = message.output_tokens.unwrap_or(0);
                    let usage_input_tokens = usage_allocation.map_or(0, |usage| usage.input_tokens);
                    let cache_creation_tokens =
                        usage_allocation.map_or(0, |usage| usage.cache_write_tokens);
                    let cache_read_tokens =
                        usage_allocation.map_or(0, |usage| usage.cache_read_tokens);
                    let reasoning_tokens =
                        usage_allocation.map_or(0, |usage| usage.reasoning_tokens);
                    let total_tokens = usage_allocation.map_or(output_tokens, |usage| {
                        if usage.total_tokens > 0 {
                            usage.total_tokens
                        } else {
                            usage_input_tokens
                                + output_tokens
                                + cache_creation_tokens
                                + cache_read_tokens
                                + reasoning_tokens
                        }
                    });
                    let mut usage = event(
                        Actor::Assistant,
                        Body::Usage(Usage {
                            model: smol_opt(
                                explicit_message_model(message, &tool_call_models)
                                    .or_else(|| current_model.clone()),
                            ),
                            input_tokens: usage_input_tokens,
                            output_tokens,
                            cache_creation_tokens,
                            cache_read_tokens,
                            cached_tokens: cache_creation_tokens + cache_read_tokens,
                            reasoning_tokens,
                            tool_tokens: 0,
                            total_tokens,
                            web_search_requests: 0,
                            speed: None,
                        }),
                        message.timestamp.clone(),
                    );
                    usage.turn_id = current_turn_id.clone();
                    events.push(usage);
                }

                for tool in &message.tool_requests {
                    let name = tool.name.clone().unwrap_or_else(|| "tool".into());
                    let template = OpTemplate {
                        kind: operation_kind(name.as_ref(), tool.command.as_deref()),
                        name: name.clone().into(),
                        command: tool.command.clone(),
                    };
                    if let Some(call_id) = &tool.tool_call_id {
                        ops.insert(call_id.to_string(), template.clone());
                    }
                    let mut ev = event(
                        Actor::Tool,
                        Body::Operation(Operation {
                            kind: template.kind,
                            phase: OperationPhase::Requested,
                            name: template.name,
                            input_json: None,
                            output_json: None,
                            command: template.command,
                            file_path: None,
                            lines_added: 0,
                            lines_removed: 0,
                            is_error: false,
                            duration_seconds: None,
                            extra_json: None,
                        }),
                        message.timestamp.clone(),
                    );
                    ev.turn_id = current_turn_id.clone();
                    ev.op_id = smol_opt(tool.tool_call_id.clone());
                    events.push(ev);
                }
            }
            super::CliRecord::ToolExecution(execution) => match execution {
                super::ToolExecution::Start {
                    tool_name,
                    tool_call_id,
                    command,
                    model: _,
                    timestamp,
                } => {
                    let name = tool_name.clone().unwrap_or_else(|| "tool".into());
                    let template = OpTemplate {
                        kind: operation_kind(name.as_ref(), command.as_deref()),
                        name: name.clone().into(),
                        command: command.clone(),
                    };
                    if let Some(call_id) = tool_call_id {
                        ops.insert(call_id.to_string(), template.clone());
                    }
                    let mut ev = event(
                        Actor::Tool,
                        Body::Operation(Operation {
                            kind: template.kind,
                            phase: OperationPhase::Started,
                            name: template.name,
                            input_json: None,
                            output_json: None,
                            command: template.command,
                            file_path: None,
                            lines_added: 0,
                            lines_removed: 0,
                            is_error: false,
                            duration_seconds: None,
                            extra_json: None,
                        }),
                        timestamp.clone(),
                    );
                    ev.turn_id = current_turn_id.clone();
                    ev.op_id = smol_opt(tool_call_id.clone());
                    events.push(ev);
                }
                super::ToolExecution::Complete {
                    tool_name,
                    tool_call_id,
                    success,
                    error_message,
                    error_code,
                    model: _,
                    timestamp,
                } => {
                    let template = tool_call_id
                        .as_deref()
                        .and_then(|call_id| ops.get(call_id))
                        .cloned()
                        .unwrap_or_else(|| {
                            let name = tool_name.clone().unwrap_or_else(|| "tool".into());
                            OpTemplate {
                                kind: operation_kind(name.as_ref(), None),
                                name: name.into(),
                                command: None,
                            }
                        });
                    let mut ev = event(
                        Actor::Tool,
                        Body::Operation(Operation {
                            kind: template.kind,
                            phase: if *success == Some(false)
                                || error_message.is_some()
                                || error_code.is_some()
                            {
                                OperationPhase::Failed
                            } else {
                                OperationPhase::Completed
                            },
                            name: template.name,
                            input_json: None,
                            output_json: error_message.clone().map(|message| {
                                serde_json::json!({
                                    "error_message": message,
                                    "error_code": error_code,
                                })
                                .to_string()
                                .into()
                            }),
                            command: template.command,
                            file_path: None,
                            lines_added: 0,
                            lines_removed: 0,
                            is_error: *success == Some(false)
                                || error_message.is_some()
                                || error_code.is_some(),
                            duration_seconds: None,
                            extra_json: None,
                        }),
                        timestamp.clone(),
                    );
                    ev.turn_id = current_turn_id.clone();
                    ev.op_id = smol_opt(tool_call_id.clone());
                    events.push(ev);
                }
            },
            super::CliRecord::TaskComplete(task) => {
                let mut ev = event(
                    Actor::System,
                    Body::State(State {
                        kind: "task_complete".into(),
                        value_json: task.summary.clone().map(|summary| {
                            serde_json::json!({ "summary": summary }).to_string().into()
                        }),
                    }),
                    task.timestamp.clone(),
                );
                ev.turn_id = current_turn_id.clone();
                events.push(ev);
            }
            super::CliRecord::TurnBoundary(boundary) => match boundary {
                super::TurnBoundary::Start {
                    turn_id,
                    interaction_id,
                    timestamp,
                } => {
                    current_turn_id =
                        smol_opt(turn_id.clone()).or_else(|| smol_opt(interaction_id.clone()));
                    let mut ev = event(
                        Actor::System,
                        Body::State(State {
                            kind: "turn_start".into(),
                            value_json: interaction_id.clone().map(|interaction_id| {
                                serde_json::json!({ "interaction_id": interaction_id })
                                    .to_string()
                                    .into()
                            }),
                        }),
                        timestamp.clone(),
                    );
                    ev.turn_id = current_turn_id.clone();
                    events.push(ev);
                }
                super::TurnBoundary::End {
                    turn_id,
                    interaction_id,
                    timestamp,
                } => {
                    let turn_ref = smol_opt(turn_id.clone())
                        .or_else(|| smol_opt(interaction_id.clone()))
                        .or_else(|| current_turn_id.clone());
                    let mut ev = event(
                        Actor::System,
                        Body::State(State {
                            kind: "turn_end".into(),
                            value_json: interaction_id.clone().map(|interaction_id| {
                                serde_json::json!({ "interaction_id": interaction_id })
                                    .to_string()
                                    .into()
                            }),
                        }),
                        timestamp.clone(),
                    );
                    ev.turn_id = turn_ref;
                    current_turn_id = None;
                    events.push(ev);
                }
            },
            super::CliRecord::SystemNotification(notification) => match notification {
                super::SystemNotification::ShellCompleted {
                    content,
                    shell_id,
                    exit_code,
                    description,
                    timestamp,
                } => {
                    let template = shell_id
                        .as_deref()
                        .and_then(|shell_id| ops.get(shell_id))
                        .cloned()
                        .unwrap_or(OpTemplate {
                            kind: OperationKind::Shell,
                            name: description.clone().unwrap_or_else(|| "shell".into()).into(),
                            command: None,
                        });
                    let mut ev = event(
                        Actor::Tool,
                        Body::Operation(Operation {
                            kind: OperationKind::Shell,
                            phase: if exit_code.unwrap_or(0) == 0 {
                                OperationPhase::Completed
                            } else {
                                OperationPhase::Failed
                            },
                            name: template.name,
                            input_json: None,
                            output_json: content.clone(),
                            command: template.command,
                            file_path: None,
                            lines_added: 0,
                            lines_removed: 0,
                            is_error: exit_code.unwrap_or(0) != 0,
                            duration_seconds: None,
                            extra_json: description.clone().map(|description| {
                                serde_json::json!({
                                    "description": description,
                                    "exit_code": exit_code,
                                })
                                .to_string()
                                .into()
                            }),
                        }),
                        timestamp.clone(),
                    );
                    ev.turn_id = current_turn_id.clone();
                    ev.op_id = smol_opt(shell_id.clone());
                    events.push(ev);
                }
                super::SystemNotification::Other {
                    content,
                    notification_type,
                    timestamp,
                } => {
                    let mut ev = event(
                        Actor::System,
                        Body::State(State {
                            kind: notification_type
                                .clone()
                                .unwrap_or_else(|| "notification".into())
                                .into(),
                            value_json: content.clone().map(|content| {
                                serde_json::json!({ "content": content }).to_string().into()
                            }),
                        }),
                        timestamp.clone(),
                    );
                    ev.turn_id = current_turn_id.clone();
                    events.push(ev);
                }
            },
            super::CliRecord::SessionEvent(event_record) => {
                let (kind, value_json, timestamp) = match event_record {
                    super::SessionEvent::Start {
                        session_id,
                        selected_model,
                        timestamp,
                    } => {
                        current_model = selected_model.clone();
                        (
                            "session_start".into(),
                            Some(
                                serde_json::json!({
                                    "session_id": session_id,
                                    "selected_model": selected_model,
                                })
                                .to_string()
                                .into(),
                            ),
                            timestamp.clone(),
                        )
                    }
                    super::SessionEvent::Resume {
                        selected_model,
                        timestamp,
                    } => {
                        current_model = selected_model.clone();
                        (
                            "session_resume".into(),
                            Some(
                                serde_json::json!({ "selected_model": selected_model })
                                    .to_string()
                                    .into(),
                            ),
                            timestamp.clone(),
                        )
                    }
                    super::SessionEvent::Shutdown {
                        current_model,
                        model_usages,
                        timestamp,
                    } => (
                        "session_shutdown".into(),
                        Some(
                            serde_json::json!({
                                "current_model": current_model,
                                "model_usages": model_usages
                                    .iter()
                                    .map(|usage| serde_json::json!({
                                        "model": usage.model,
                                        "input_tokens": usage.input_tokens,
                                        "output_tokens": usage.output_tokens,
                                        "cache_read_tokens": usage.cache_read_tokens,
                                        "cache_write_tokens": usage.cache_write_tokens,
                                        "reasoning_tokens": usage.reasoning_tokens,
                                    }))
                                    .collect::<Vec<_>>(),
                            })
                            .to_string()
                            .into(),
                        ),
                        timestamp.clone(),
                    ),
                    super::SessionEvent::ModeChanged {
                        previous_mode,
                        new_mode,
                        timestamp,
                    } => (
                        "session_mode_changed".into(),
                        Some(
                            serde_json::json!({
                                "previous_mode": previous_mode,
                                "new_mode": new_mode,
                            })
                            .to_string()
                            .into(),
                        ),
                        timestamp.clone(),
                    ),
                    super::SessionEvent::ModelChange {
                        previous_model,
                        new_model,
                        timestamp,
                    } => {
                        current_model = new_model.clone();
                        (
                            "session_model_change".into(),
                            Some(
                                serde_json::json!({
                                    "previous_model": previous_model,
                                    "new_model": new_model,
                                })
                                .to_string()
                                .into(),
                            ),
                            timestamp.clone(),
                        )
                    }
                    super::SessionEvent::PlanChanged {
                        operation,
                        timestamp,
                    } => (
                        "session_plan_changed".into(),
                        Some(
                            serde_json::json!({ "operation": operation })
                                .to_string()
                                .into(),
                        ),
                        timestamp.clone(),
                    ),
                    super::SessionEvent::CompactionStart { timestamp } => {
                        ("session_compaction_start".into(), None, timestamp.clone())
                    }
                    super::SessionEvent::CompactionComplete {
                        success,
                        summary_content,
                        error_message,
                        error_code,
                        timestamp,
                    } => (
                        "session_compaction_complete".into(),
                        Some(
                            serde_json::json!({
                                "success": success,
                                "summary_content": summary_content,
                                "error_message": error_message,
                                "error_code": error_code,
                            })
                            .to_string()
                            .into(),
                        ),
                        timestamp.clone(),
                    ),
                    super::SessionEvent::Truncation { timestamp } => {
                        ("session_truncation".into(), None, timestamp.clone())
                    }
                    super::SessionEvent::Error {
                        error_type,
                        message,
                        timestamp,
                    } => (
                        "session_error".into(),
                        Some(
                            serde_json::json!({
                                "error_type": error_type,
                                "message": message,
                            })
                            .to_string()
                            .into(),
                        ),
                        timestamp.clone(),
                    ),
                };
                let mut ev = event(
                    Actor::System,
                    Body::State(State { kind, value_json }),
                    timestamp,
                );
                ev.turn_id = current_turn_id.clone();
                events.push(ev);
            }
            super::CliRecord::SubagentEvent(subagent) => {
                let (phase, op_id, name, extra_json, timestamp) = match subagent {
                    super::SubagentEvent::Started {
                        tool_call_id,
                        agent_name,
                        agent_display_name,
                        agent_description,
                        timestamp,
                    } => (
                        OperationPhase::Started,
                        tool_call_id.clone(),
                        agent_name
                            .clone()
                            .or_else(|| agent_display_name.clone())
                            .unwrap_or_else(|| "subagent".into()),
                        Some(
                            serde_json::json!({
                                "agent_display_name": agent_display_name,
                                "agent_description": agent_description,
                            })
                            .to_string()
                            .into(),
                        ),
                        timestamp.clone(),
                    ),
                    super::SubagentEvent::Completed {
                        tool_call_id,
                        agent_name,
                        agent_display_name,
                        timestamp,
                    } => (
                        OperationPhase::Completed,
                        tool_call_id.clone(),
                        agent_name
                            .clone()
                            .or_else(|| agent_display_name.clone())
                            .unwrap_or_else(|| "subagent".into()),
                        Some(
                            serde_json::json!({ "agent_display_name": agent_display_name })
                                .to_string()
                                .into(),
                        ),
                        timestamp.clone(),
                    ),
                    super::SubagentEvent::Failed {
                        tool_call_id,
                        agent_name,
                        agent_display_name,
                        error,
                        timestamp,
                    } => (
                        OperationPhase::Failed,
                        tool_call_id.clone(),
                        agent_name
                            .clone()
                            .or_else(|| agent_display_name.clone())
                            .unwrap_or_else(|| "subagent".into()),
                        Some(
                            serde_json::json!({
                                "agent_display_name": agent_display_name,
                                "error": error,
                            })
                            .to_string()
                            .into(),
                        ),
                        timestamp.clone(),
                    ),
                    super::SubagentEvent::Deselected { timestamp } => (
                        OperationPhase::Cancelled,
                        None,
                        "subagent".into(),
                        None,
                        timestamp.clone(),
                    ),
                };
                let mut ev = event(
                    Actor::Subagent,
                    Body::Operation(Operation {
                        kind: OperationKind::Subagent,
                        phase,
                        name: name.into(),
                        input_json: None,
                        output_json: None,
                        command: None,
                        file_path: None,
                        lines_added: 0,
                        lines_removed: 0,
                        is_error: phase == OperationPhase::Failed,
                        duration_seconds: None,
                        extra_json,
                    }),
                    timestamp,
                );
                ev.turn_id = current_turn_id.clone();
                ev.op_id = smol_opt(op_id);
                events.push(ev);
            }
            super::CliRecord::Abort(abort) => {
                let mut ev = event(
                    Actor::System,
                    Body::State(State {
                        kind: "abort".into(),
                        value_json: abort.reason.clone().map(|reason| {
                            serde_json::json!({ "reason": reason }).to_string().into()
                        }),
                    }),
                    abort.timestamp.clone(),
                );
                ev.turn_id = current_turn_id.clone();
                events.push(ev);
            }
            super::CliRecord::Unknown(unknown) => {
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

    events.into_boxed_slice()
}

fn cli_session_id(body: &super::CliEventsBody) -> Option<SmolStr> {
    body.records.iter().find_map(|record| match record {
        super::CliRecord::SessionEvent(super::SessionEvent::Start { session_id, .. }) => {
            session_id.clone()
        }
        _ => None,
    })
}

#[cfg(feature = "watch")]
fn copilot_watch_text(text: Option<SmolStr>) -> Option<smol_str::SmolStr> {
    text.as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(Into::into)
}

#[cfg(feature = "watch")]
fn copilot_watch_usage(
    meta: crate::watch::WatchEventMeta,
    usage: Usage,
) -> crate::watch::WatchEvent {
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
fn copilot_watch_event_from_agent_event(event: Event) -> crate::watch::WatchEvent {
    let Event {
        id,
        timestamp,
        actor,
        turn_id,
        op_id,
        parent_op_id,
        body,
    } = event;
    let meta = crate::watch::WatchEventMeta {
        id,
        timestamp,
        turn_id,
        op_id,
        parent_op_id,
    };

    match (actor, body) {
        (Actor::User, Body::Prompt(prompt)) => {
            crate::watch::WatchEvent::UserMessage(crate::watch::WatchMessage {
                meta,
                text: copilot_watch_text(
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
                text: copilot_watch_text(
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
        (Actor::Tool, Body::Operation(operation)) => {
            let call_id = meta.parent_op_id.as_deref().map(Into::into);
            crate::watch::WatchEvent::ToolResult(crate::watch::WatchToolResult {
                meta,
                kind: operation.kind,
                phase: operation.phase,
                call_id,
                name: operation.name,
                output_json: crate::watch::watch_smol_opt(operation.output_json),
                is_error: operation.is_error,
                duration_seconds: operation.duration_seconds,
            })
        }
        (_, Body::Usage(usage)) => copilot_watch_usage(meta, usage),
        (Actor::System, Body::State(state)) if state.kind.as_str() == "task_complete" => {
            crate::watch::WatchEvent::TurnCompleted(crate::watch::WatchTurnCompleted {
                meta,
                last_agent_message: None,
                duration_ms: None,
                value_json: crate::watch::watch_smol_opt(state.value_json),
            })
        }
        (Actor::System, Body::State(state)) => {
            crate::watch::WatchEvent::State(crate::watch::WatchState {
                meta,
                kind: state.kind,
                value_json: crate::watch::watch_smol_opt(state.value_json),
            })
        }
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
fn watch_events_from_copilot_agent_events(
    events: Box<[Event]>,
    selection: crate::ParseSelection,
) -> Box<[crate::watch::WatchEvent]> {
    events
        .into_vec()
        .into_iter()
        .filter(|event| selection.includes_body(&event.body))
        .map(copilot_watch_event_from_agent_event)
        .collect()
}

fn agent_session_from_copilot_body(body: &super::Body, version: super::Version) -> Session {
    let (meta, events) = match body {
        super::Body::CliEvents(body) => {
            let events = cli_events(body);
            let models = summarize_models(&events);
            (
                SessionMeta {
                    session_id: smol_opt(cli_session_id(body).or_else(|| {
                        body.workspace
                            .as_ref()
                            .and_then(|workspace| workspace.id.clone())
                    })),
                    cwd: body
                        .workspace
                        .as_ref()
                        .and_then(|workspace| workspace.cwd.clone()),
                    title: body
                        .workspace
                        .as_ref()
                        .and_then(|workspace| workspace.summary.clone()),
                    models,
                    created_at: smol_opt(
                        body.workspace
                            .as_ref()
                            .and_then(|workspace| workspace.created_at.clone())
                            .or_else(|| earliest_timestamp(&events)),
                    ),
                    updated_at: smol_opt(
                        body.workspace
                            .as_ref()
                            .and_then(|workspace| workspace.updated_at.clone())
                            .or_else(|| latest_timestamp(&events)),
                    ),
                    source_kind: Some("cli_events".into()),
                    ..SessionMeta::default()
                },
                events,
            )
        }
        super::Body::ChatSession(body) => {
            let events = chat_events(body);
            let mut models = summarize_models(&events).into_vec();
            if let Some(model) = body.model.clone()
                && !models
                    .iter()
                    .any(|summary| summary.model.as_str() == model.as_str())
            {
                models.push(SessionModelMeta::zero(model));
            }
            (
                SessionMeta {
                    session_id: smol_opt(body.session_id.clone()),
                    thread_id: smol_opt(body.workspace_id.clone()),
                    models: models.into_boxed_slice(),
                    created_at: smol_opt(earliest_timestamp(&events)),
                    updated_at: smol_opt(latest_timestamp(&events)),
                    source_kind: Some("chat_session".into()),
                    extra_json: Some(
                        serde_json::json!({
                            "model": body.model,
                            "mode": body.mode,
                        })
                        .to_string()
                        .into(),
                    ),
                    ..SessionMeta::default()
                },
                events,
            )
        }
    };

    Session {
        agent: AgentKind::new("copilot"),
        version: event_version(version),
        meta,
        events,
    }
}

pub(super) fn parse_direct_agent_session_reader_selected<R>(
    reader: R,
    metadata: crate::InputMetadata<'_>,
    selection: crate::ParseSelection,
) -> crate::Result<Session>
where
    R: std::io::BufRead,
{
    let (version, body) = super::parse_copilot_body_reader(
        reader,
        metadata,
        copilot_projection_selection(selection),
    )?;
    Ok(crate::agent_session::filter_selection(
        agent_session_from_copilot_body(&body, version),
        selection,
    ))
}

#[cfg(feature = "watch")]
impl crate::watch::provider::ProviderWatchEvents for crate::Copilot {
    fn parse_watch_reader<R>(
        path: &std::path::Path,
        reader: R,
        selection: crate::ParseSelection,
    ) -> crate::Result<crate::watch::provider::ParsedWatchSession>
    where
        R: std::io::BufRead,
    {
        let (_version, body) = super::parse_copilot_body_reader(
            reader,
            crate::watch::provider::input_metadata_for_path(path),
            copilot_projection_selection(selection),
        )?;
        let (session_id, cwd, events) = match body {
            super::Body::CliEvents(body) => {
                let session_id = cli_session_id(&body).or_else(|| {
                    body.workspace
                        .as_ref()
                        .and_then(|workspace| workspace.id.clone())
                });
                let cwd = body
                    .workspace
                    .as_ref()
                    .and_then(|workspace| workspace.cwd.clone());
                let events = watch_events_from_copilot_agent_events(cli_events(&body), selection);
                (session_id, cwd, events)
            }
            super::Body::ChatSession(body) => {
                let session_id = body.session_id.clone();
                let events = watch_events_from_copilot_agent_events(chat_events(&body), selection);
                (session_id, None, events)
            }
        };
        Ok(crate::watch::provider::ParsedWatchSession {
            session_id,
            cwd,
            title: None,
            events,
        })
    }

    fn probe_watch_session_meta<R>(
        path: &std::path::Path,
        reader: R,
    ) -> crate::Result<crate::agent_session::SessionMeta>
    where
        R: std::io::BufRead,
    {
        let (_version, body) = super::parse_copilot_body_reader(
            reader,
            crate::watch::provider::input_metadata_for_path(path),
            crate::ParseSelection::meta_only(),
        )?;
        let (session_id, cwd) = match body {
            super::Body::CliEvents(body) => (
                cli_session_id(&body).or_else(|| {
                    body.workspace
                        .as_ref()
                        .and_then(|workspace| workspace.id.clone())
                }),
                body.workspace
                    .as_ref()
                    .and_then(|workspace| workspace.cwd.clone()),
            ),
            super::Body::ChatSession(body) => (body.session_id, None),
        };
        Ok(crate::agent_session::SessionMeta {
            session_id,
            cwd,
            ..crate::agent_session::SessionMeta::default()
        })
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
