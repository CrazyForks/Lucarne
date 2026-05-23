use crate::agent_session::{projection::parsed_tool_input, *};
#[cfg(feature = "watch")]
use smol_str::SmolStr;

#[cfg(feature = "watch")]
fn watch_meta(timestamp: Option<SmolStr>) -> crate::watch::WatchEventMeta {
    crate::watch::WatchEventMeta {
        timestamp: timestamp.map(Into::into),
        ..crate::watch::WatchEventMeta::default()
    }
}

#[cfg(feature = "watch")]
fn watch_text_from_blocks(blocks: &[super::ContentBlock]) -> Option<smol_str::SmolStr> {
    let parts = blocks
        .iter()
        .filter_map(|block| match block {
            super::ContentBlock::Text(block) => Some(block.text.as_str()),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();

    if parts.is_empty() {
        None
    } else {
        let joined = parts.join(" ");
        let trimmed = joined.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.into())
        }
    }
}

#[cfg(feature = "watch")]
fn watch_events_from_cursor_entries(
    entries: Box<[super::Entry]>,
) -> Box<[crate::watch::WatchEvent]> {
    entries
        .into_vec()
        .into_iter()
        .map(|entry| {
            let meta = watch_meta(entry.timestamp);
            let text = watch_text_from_blocks(&entry.blocks);
            match entry.role {
                super::Role::User => {
                    crate::watch::WatchEvent::UserMessage(crate::watch::WatchMessage { meta, text })
                }
                super::Role::Assistant => crate::watch::WatchEvent::AssistantMessage(
                    crate::watch::WatchAssistantMessage {
                        meta,
                        model: None,
                        phase: None,
                        text,
                    },
                ),
                super::Role::Other(value) => {
                    let blocks = map_blocks(&entry.blocks);
                    crate::watch::WatchEvent::Other(crate::watch::WatchOther {
                        meta,
                        actor: Actor::Other(value.into()),
                        body: Body::Response(Response {
                            model: None,
                            phase: None,
                            text: text.as_ref().map(|text| text.as_str().into()),
                            blocks,
                        }),
                    })
                }
            }
        })
        .collect()
}

fn map_actor(role: &super::Role) -> Actor {
    match role {
        super::Role::User => Actor::User,
        super::Role::Assistant => Actor::Assistant,
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

fn agent_session_from_cursor_body(body: &super::Body) -> Session {
    let events = body
        .entries
        .iter()
        .map(|entry| {
            let blocks = map_blocks(&entry.blocks);
            let actor = map_actor(&entry.role);
            let text = text_from_blocks(&blocks);
            let body = match entry.role {
                super::Role::User => Body::Prompt(Prompt { text, blocks }),
                _ => Body::Response(Response {
                    model: None,
                    phase: None,
                    text,
                    blocks,
                }),
            };
            event(actor, body, entry.timestamp.clone())
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();

    Session {
        agent: AgentKind::new("cursor"),
        version: VersionKind::new("cursor-v1"),
        meta: SessionMeta {
            session_id: smol_opt(body.session_id.clone()),
            models: summarize_models(&events),
            source_kind: Some("agent_transcript_jsonl".into()),
            ..SessionMeta::default()
        },
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
    let body = super::parse_cursor_body_reader(
        reader,
        super::cursor_session_id_from_name(metadata.name),
        selection,
    )?;
    Ok(crate::agent_session::filter_selection(
        agent_session_from_cursor_body(&body),
        selection,
    ))
}

#[cfg(feature = "watch")]
impl crate::watch::provider::ProviderWatchEvents for crate::Cursor {
    fn parse_watch_reader<R>(
        path: &std::path::Path,
        reader: R,
        selection: crate::ParseSelection,
    ) -> crate::Result<crate::watch::provider::ParsedWatchSession>
    where
        R: std::io::BufRead,
    {
        let body = super::parse_cursor_reader(
            reader,
            super::cursor_session_id_from_name(path.file_name().and_then(|name| name.to_str())),
            selection,
        )?;
        Ok(crate::watch::provider::ParsedWatchSession {
            session_id: body.session_id,
            cwd: None,
            title: None,
            events: watch_events_from_cursor_entries(body.entries),
        })
    }

    fn probe_watch_session_meta<R>(
        path: &std::path::Path,
        reader: R,
    ) -> crate::Result<crate::agent_session::SessionMeta>
    where
        R: std::io::BufRead,
    {
        let body = super::parse_cursor_reader(
            reader,
            super::cursor_session_id_from_name(path.file_name().and_then(|name| name.to_str())),
            crate::ParseSelection::meta_only(),
        )?;
        Ok(crate::agent_session::SessionMeta {
            session_id: body.session_id,
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
