use std::path::PathBuf;

use agent_sessions::agent_session::{Actor, Body, ContentBlock, Session as UniSession};

use super::{
    transcript::ProviderTranscript, HistoryCursor, HistoryImage, HistoryMessage,
    HistoryProviderDescriptor, HistoryTranscript, HistoryTranscriptError, HistoryTurn,
};

pub(super) fn transcript_from_session_window(
    provider: HistoryProviderDescriptor,
    provider_id: &'static str,
    session_id: &str,
    session_path: PathBuf,
    session: &UniSession,
    transcript: &ProviderTranscript,
    limit: usize,
) -> Result<HistoryTranscript, HistoryTranscriptError> {
    let turns = visible_turns(provider, session);
    let end = turns.len();
    let start = end.saturating_sub(limit);
    let page = if limit == 0 {
        Vec::new()
    } else {
        turns[start..end].to_vec()
    };
    let older_cursor = transcript
        .older_cursor_for_visible_start(start)
        .map(|cursor| HistoryCursor::new(cursor.to_string()));
    let has_older = older_cursor.is_some();
    Ok(HistoryTranscript {
        provider_id,
        session_id: session_id.to_string(),
        session_path,
        turns: page,
        has_older,
        older_cursor,
    })
}

pub(super) fn visible_turns(
    provider: HistoryProviderDescriptor,
    session: &UniSession,
) -> Vec<HistoryTurn> {
    let mut turns = Vec::new();
    let mut current: Option<HistoryTurn> = None;
    for (idx, ev) in session.events.iter().enumerate() {
        match (&ev.actor, &ev.body) {
            (Actor::User, Body::Prompt(prompt)) => {
                let Some(user) = history_message_from_prompt("user", idx, ev, prompt) else {
                    continue;
                };
                if !provider.is_transcript_user_text_visible(&user.text) {
                    continue;
                }
                if current.as_ref().is_some_and(|turn| {
                    turn.assistant.is_none()
                        && turn.user.text == user.text
                        && turn.user.images == user.images
                }) {
                    continue;
                }
                if let Some(turn) = current.take() {
                    turns.push(turn);
                }
                current = Some(HistoryTurn {
                    id: user.id.clone(),
                    user,
                    assistant: None,
                });
            }
            (Actor::Assistant, Body::Response(response)) => {
                let Some(message) = history_message_from_response("assistant", idx, ev, response)
                else {
                    continue;
                };
                if let Some(turn) = current.as_mut() {
                    turn.assistant = Some(message);
                }
            }
            (Actor::System, Body::Prompt(_)) => {
                if let Some(turn) = current.take() {
                    turns.push(turn);
                }
            }
            _ => {}
        }
    }
    if let Some(turn) = current {
        turns.push(turn);
    }
    turns
}

fn history_message_from_prompt(
    fallback_kind: &str,
    idx: usize,
    ev: &agent_sessions::agent_session::Event,
    prompt: &agent_sessions::agent_session::Prompt,
) -> Option<HistoryMessage> {
    let text = prompt_text(prompt).unwrap_or_default();
    let images = images_from_blocks(&prompt.blocks);
    if text.is_empty() && images.is_empty() {
        return None;
    }
    Some(history_message(fallback_kind, idx, ev, text, images))
}

fn history_message_from_response(
    fallback_kind: &str,
    idx: usize,
    ev: &agent_sessions::agent_session::Event,
    response: &agent_sessions::agent_session::Response,
) -> Option<HistoryMessage> {
    let text = response_text(response).unwrap_or_default();
    let images = images_from_blocks(&response.blocks);
    if text.is_empty() && images.is_empty() {
        return None;
    }
    Some(history_message(fallback_kind, idx, ev, text, images))
}

fn prompt_text(prompt: &agent_sessions::agent_session::Prompt) -> Option<String> {
    if let Some(text) = prompt
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        return Some(text.to_string());
    }
    agent_sessions::agent_session::text_from_blocks(&prompt.blocks).and_then(|text| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    })
}

fn response_text(response: &agent_sessions::agent_session::Response) -> Option<String> {
    if let Some(text) = response
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        return Some(text.to_string());
    }
    agent_sessions::agent_session::text_from_blocks(&response.blocks).and_then(|text| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    })
}

fn images_from_blocks(blocks: &[ContentBlock]) -> Vec<HistoryImage> {
    blocks
        .iter()
        .flat_map(|block| match block {
            ContentBlock::Image(image) => {
                let image_url = image.image_url.trim();
                (!image_url.is_empty())
                    .then(|| {
                        vec![HistoryImage {
                            image_url: image_url.to_string(),
                        }]
                    })
                    .unwrap_or_default()
            }
            ContentBlock::Raw(raw) if raw.kind.as_str() == "images" => {
                images_from_raw_json(&raw.raw_json)
            }
            ContentBlock::Raw(raw) if raw.kind.as_str() == "local_images" => {
                images_from_raw_json(&raw.raw_json)
            }
            _ => Vec::new(),
        })
        .collect()
}

fn images_from_raw_json(raw: &str) -> Vec<HistoryImage> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(image_url_from_json_value)
            .map(|image_url| HistoryImage { image_url })
            .collect(),
        other => image_url_from_json_value(&other)
            .map(|image_url| vec![HistoryImage { image_url }])
            .unwrap_or_default(),
    }
}

fn image_url_from_json_value(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        }
        serde_json::Value::Object(map) => [
            "image_url",
            "url",
            "path",
            "file_path",
            "local_path",
            "filename",
        ]
        .iter()
        .find_map(|key| map.get(*key).and_then(image_url_from_json_value)),
        _ => None,
    }
}

fn history_message(
    fallback_kind: &str,
    idx: usize,
    ev: &agent_sessions::agent_session::Event,
    text: String,
    images: Vec<HistoryImage>,
) -> HistoryMessage {
    let id = ev
        .id
        .as_deref()
        .map(str::to_string)
        .unwrap_or_else(|| format!("{fallback_kind}:{idx}"));
    HistoryMessage {
        id,
        text,
        timestamp: ev.timestamp.as_deref().map(str::to_string),
        images,
    }
}
