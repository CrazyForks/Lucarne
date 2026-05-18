use super::state::ControlPlaneState;
use super::types::{ProviderSessionId, WorkspaceId};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::{path::PathBuf, time::SystemTime};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HistoryOlderCallbackToken(SmolStr);

impl HistoryOlderCallbackToken {
    pub fn new(value: impl Into<SmolStr>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryReplayRecord {
    pub workspace_id: WorkspaceId,
    #[serde(default)]
    pub channel: Option<SmolStr>,
    #[serde(default)]
    pub chat_id: Option<SmolStr>,
    #[serde(default)]
    pub topic_id: Option<SmolStr>,
    pub provider_id: SmolStr,
    pub session_id: SmolStr,
    pub session_path: PathBuf,
    pub replayed_turns: Vec<HistoryReplayTurnRecord>,
    pub older_cursor: Option<SmolStr>,
    #[serde(default)]
    pub older_channel_message_id: Option<SmolStr>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl HistoryReplayRecord {
    pub fn new(
        workspace_id: WorkspaceId,
        provider_id: impl Into<SmolStr>,
        session_id: impl Into<SmolStr>,
        session_path: PathBuf,
    ) -> Self {
        let now = SystemTime::now();
        Self {
            workspace_id,
            channel: None,
            chat_id: None,
            topic_id: None,
            provider_id: provider_id.into(),
            session_id: session_id.into(),
            session_path,
            replayed_turns: Vec::new(),
            older_cursor: None,
            older_channel_message_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn set_projection_target(
        &mut self,
        channel: impl Into<SmolStr>,
        chat_id: impl Into<SmolStr>,
        topic_id: impl Into<SmolStr>,
    ) {
        self.channel = Some(channel.into());
        self.chat_id = Some(chat_id.into());
        self.topic_id = Some(topic_id.into());
        self.updated_at = SystemTime::now();
    }

    pub fn matches_projection_target(&self, channel: &str, chat_id: &str, topic_id: &str) -> bool {
        self.channel.as_deref() == Some(channel)
            && self.chat_id.as_deref() == Some(chat_id)
            && self.topic_id.as_deref() == Some(topic_id)
    }

    pub fn mark_user_sent(
        &mut self,
        turn_id: impl Into<SmolStr>,
        user_channel_message_id: impl Into<SmolStr>,
    ) {
        let turn_id = turn_id.into();
        let user_channel_message_id = user_channel_message_id.into();
        match self
            .replayed_turns
            .iter_mut()
            .find(|turn| turn.turn_id == turn_id)
        {
            Some(turn) => {
                record_projected_message(turn, user_channel_message_id.clone());
                turn.user_channel_message_id = Some(user_channel_message_id);
            }
            None => self.replayed_turns.push(HistoryReplayTurnRecord {
                turn_id,
                user_channel_message_id: Some(user_channel_message_id.clone()),
                assistant_sent: false,
                projected_channel_message_ids: vec![user_channel_message_id],
                user_image_channel_message_ids: Vec::new(),
            }),
        }
        self.updated_at = SystemTime::now();
    }

    pub fn mark_assistant_sent(
        &mut self,
        turn_id: &str,
        assistant_channel_message_id: impl Into<SmolStr>,
    ) {
        let assistant_channel_message_id = assistant_channel_message_id.into();
        if let Some(turn) = self
            .replayed_turns
            .iter_mut()
            .find(|turn| turn.turn_id.as_str() == turn_id)
        {
            record_projected_message(turn, assistant_channel_message_id);
            turn.assistant_sent = true;
        } else {
            self.replayed_turns.push(HistoryReplayTurnRecord {
                turn_id: turn_id.into(),
                user_channel_message_id: None,
                assistant_sent: true,
                projected_channel_message_ids: vec![assistant_channel_message_id],
                user_image_channel_message_ids: Vec::new(),
            });
        }
        self.updated_at = SystemTime::now();
    }

    pub fn mark_user_image_sent(&mut self, turn_id: &str, channel_message_id: impl Into<SmolStr>) {
        let channel_message_id = channel_message_id.into();
        if let Some(turn) = self
            .replayed_turns
            .iter_mut()
            .find(|turn| turn.turn_id.as_str() == turn_id)
        {
            record_projected_message(turn, channel_message_id.clone());
            record_user_image_message(turn, channel_message_id);
        } else {
            self.replayed_turns.push(HistoryReplayTurnRecord {
                turn_id: turn_id.into(),
                user_channel_message_id: None,
                assistant_sent: false,
                projected_channel_message_ids: vec![channel_message_id.clone()],
                user_image_channel_message_ids: vec![channel_message_id],
            });
        }
        self.updated_at = SystemTime::now();
    }

    pub fn projected_channel_message_ids(&self) -> Vec<SmolStr> {
        self.replayed_turns
            .iter()
            .flat_map(|turn| {
                if turn.projected_channel_message_ids.is_empty() {
                    turn.user_channel_message_id.iter().cloned().collect()
                } else {
                    turn.projected_channel_message_ids.clone()
                }
            })
            .collect()
    }

    pub fn clear_replayed_turns(&mut self) {
        self.replayed_turns.clear();
        self.updated_at = SystemTime::now();
    }

    pub fn turn(&self, turn_id: &str) -> Option<&HistoryReplayTurnRecord> {
        self.replayed_turns
            .iter()
            .find(|turn| turn.turn_id.as_str() == turn_id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryReplayTurnRecord {
    pub turn_id: SmolStr,
    pub user_channel_message_id: Option<SmolStr>,
    pub assistant_sent: bool,
    #[serde(default)]
    pub projected_channel_message_ids: Vec<SmolStr>,
    #[serde(default)]
    pub user_image_channel_message_ids: Vec<SmolStr>,
}

fn record_projected_message(turn: &mut HistoryReplayTurnRecord, message_id: SmolStr) {
    if !turn
        .projected_channel_message_ids
        .iter()
        .any(|existing| existing == &message_id)
    {
        turn.projected_channel_message_ids.push(message_id);
    }
}

fn record_user_image_message(turn: &mut HistoryReplayTurnRecord, message_id: SmolStr) {
    if !turn
        .user_image_channel_message_ids
        .iter()
        .any(|existing| existing == &message_id)
    {
        turn.user_image_channel_message_ids.push(message_id);
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryOlderCallbackRecord {
    pub token: HistoryOlderCallbackToken,
    pub workspace_id: WorkspaceId,
    #[serde(default)]
    pub provider_session_id: Option<ProviderSessionId>,
    pub provider_id: SmolStr,
    pub session_id: SmolStr,
    pub session_path: PathBuf,
    pub cursor: SmolStr,
    pub created_at: SystemTime,
}

impl HistoryOlderCallbackRecord {
    pub fn callback_payload(&self) -> String {
        format!("historyolder:c:{}", self.token.as_str())
    }
}

impl ControlPlaneState {
    pub fn history_replay(&self, workspace_id: &WorkspaceId) -> Option<HistoryReplayRecord> {
        self.history_replays.get(workspace_id).cloned()
    }

    pub fn upsert_history_replay(
        &mut self,
        mut record: HistoryReplayRecord,
    ) -> HistoryReplayRecord {
        if let Some(existing) = self.history_replays.get(&record.workspace_id) {
            record.created_at = existing.created_at;
        }
        record.updated_at = SystemTime::now();
        self.history_replays
            .insert(record.workspace_id.clone(), record.clone());
        record
    }

    pub fn register_history_older_callback(
        &mut self,
        workspace_id: WorkspaceId,
        provider_id: impl Into<SmolStr>,
        session_id: impl Into<SmolStr>,
        session_path: PathBuf,
        cursor: impl Into<SmolStr>,
    ) -> HistoryOlderCallbackRecord {
        self.next_history_older_callback += 1;
        let provider_session_id = self
            .workspaces
            .get(&workspace_id)
            .and_then(|workspace| workspace.active_provider_session_id.clone());
        let record = HistoryOlderCallbackRecord {
            token: HistoryOlderCallbackToken::new(format!("h{}", self.next_history_older_callback)),
            workspace_id,
            provider_session_id,
            provider_id: provider_id.into(),
            session_id: session_id.into(),
            session_path,
            cursor: cursor.into(),
            created_at: SystemTime::now(),
        };
        self.history_older_callbacks
            .insert(record.token.clone(), record.clone());
        record
    }

    pub fn resolve_history_older_callback(
        &self,
        token: &HistoryOlderCallbackToken,
    ) -> Option<HistoryOlderCallbackRecord> {
        self.history_older_callbacks.get(token).cloned()
    }
}
