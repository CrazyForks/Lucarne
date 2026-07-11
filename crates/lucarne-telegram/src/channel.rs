//! Telegram channel: [`lucarne_channel::Channel`] implementation backed by
//! frankenstein (Bot API 10.1+).
//!
//! This module owns the translation between generic channel events and
//! Telegram-specific types (Updates, forum topics, inline keyboards).
//!
//! Outbound markdown is sent as **Rich Messages** (`sendRichMessage` with
//! Rich Markdown). Plain text and control-style short messages still use
//! `sendMessage`. Callback buttons stay on Telegram's standard
//! `InlineKeyboardMarkup` + `callback_data` so the existing bot button
//! routing is unchanged.

use async_trait::async_trait;
use frankenstein::client_reqwest::Bot;
use frankenstein::inline_mode::{
    InlineQueryResult, InlineQueryResultArticle, InputMessageContent, InputTextMessageContent,
};
use frankenstein::input_file::{FileUpload as TgFileUpload, InputFile};
use frankenstein::methods::{
    AnswerInlineQueryParams, CreateForumTopicParams, DeleteForumTopicParams, DeleteMessageParams,
    EditForumTopicParams, EditMessageTextParams, GetFileParams, GetUpdatesParams,
    SendChatActionParams, SendDocumentParams, SendMessageParams, SendPhotoParams,
    SendRichMessageParams, SendVideoParams, SetMyCommandsParams,
};
use frankenstein::rich_message::InputRichMessage;
use frankenstein::types::{
    AllowedUpdate, BotCommand, ChatAction, ChatId as TgChatId, InlineKeyboardButton,
    InlineKeyboardMarkup, MaybeInaccessibleMessage, PhotoSize, ReplyMarkup, ReplyParameters,
};
use frankenstein::updates::{Update, UpdateContent};
use frankenstein::{AsyncTelegramApi, Error as TgError};
use futures::stream::{BoxStream, StreamExt};
use lucarne_channel::{
    markdown::render_telegram_markdown_v2,
    splitter::split_for_channel,
    types::{
        Attachment, ChannelError, ChannelEvent, ChatId, CommandQuery, CommandQueryResult,
        FileUpload, IncomingAttachment, IncomingMessage, MessageId, OutgoingButton,
        OutgoingMessage, Result, WorkspaceHandle, WorkspaceId,
    },
    Channel, TextFormat,
};
use frankenstein::ParseMode;
use std::{
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;
use tracing::{debug, info, instrument, trace, warn};

const EVENT_QUEUE: usize = 128;
const TELEGRAM_MESSAGE_LIMIT: usize = 4000;
const GET_UPDATES_TIMEOUT_SECS: u32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TelegramAttachmentKind {
    Photo,
    Video,
    Document,
}

fn telegram_attachment_kind(media_type: &str) -> TelegramAttachmentKind {
    let media_type = media_type.trim().to_ascii_lowercase();
    if media_type.starts_with("image/") {
        TelegramAttachmentKind::Photo
    } else if media_type.starts_with("video/") {
        TelegramAttachmentKind::Video
    } else {
        TelegramAttachmentKind::Document
    }
}

/// Configuration for [`TelegramChannel`].
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    /// Bot token (from `@BotFather`).
    pub token: String,
    /// Numeric chat id of the super-group with forum topics enabled
    /// that hosts the fixed entry + working workspaces.
    pub entry_chat_id: i64,
    /// Optional allow-list of user ids. Empty = everyone who can reach
    /// the bot is allowed.
    pub authorized_user_ids: Vec<i64>,
}

pub struct TelegramChannel {
    bot: Bot,
    events_rx: Mutex<Option<mpsc::Receiver<ChannelEvent>>>,
    _poll_task: tokio::task::JoinHandle<()>,
    cfg: Arc<TelegramConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelegramBotCommand {
    pub command: &'static str,
    pub description: &'static str,
}

impl Drop for TelegramChannel {
    fn drop(&mut self) {
        self._poll_task.abort();
    }
}

fn bot_api_url(token: &str) -> String {
    format!("{}{token}", frankenstein::BASE_API_URL)
}

impl TelegramChannel {
    pub fn start(cfg: TelegramConfig) -> Arc<Self> {
        let bot = Bot::new(&cfg.token);
        Self::start_with_bot(cfg, bot)
    }

    /// Construct the channel using a shared daemon HTTP client when possible.
    ///
    /// frankenstein 0.50 depends on reqwest 0.13 while the workspace (and
    /// wechat-ilink) stay on reqwest 0.12, so the client handle cannot be
    /// shared by type. We still accept the daemon client for API stability and
    /// build an equivalent frankenstein client (timeouts + system proxy).
    pub fn start_with_client(cfg: TelegramConfig, http_client: reqwest::Client) -> Arc<Self> {
        let _daemon_client = http_client;
        let frankenstein_client = frankenstein::reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(500))
            .build()
            .unwrap_or_else(|_| frankenstein::reqwest::Client::new());
        let bot = Bot::builder()
            .api_url(bot_api_url(&cfg.token))
            .client(frankenstein_client)
            .build();
        Self::start_with_bot(cfg, bot)
    }

    fn start_with_bot(cfg: TelegramConfig, bot: Bot) -> Arc<Self> {
        lucarne::memory_profile_snapshot!("lucarne_telegram.channel.start.start");
        let cfg = Arc::new(cfg);
        info!(
            target: "lucarne_telegram",
            entry_chat_id = cfg.entry_chat_id,
            allow_list = cfg.authorized_user_ids.len(),
            "starting TelegramChannel"
        );
        lucarne::memory_profile_snapshot!("lucarne_telegram.channel.start.after_bot_new");
        let (tx, rx) = mpsc::channel(EVENT_QUEUE);

        let poll_bot = bot.clone();
        let poll_cfg = Arc::clone(&cfg);
        let handle = tokio::spawn(async move {
            if let Err(e) = poll_updates(poll_bot, tx, poll_cfg).await {
                warn!(target: "lucarne_telegram", "update loop ended: {e}");
            }
        });
        lucarne::memory_profile_snapshot!("lucarne_telegram.channel.start.after_poll_spawn");

        Arc::new(Self {
            bot,
            events_rx: Mutex::new(Some(rx)),
            _poll_task: handle,
            cfg,
        })
    }

    /// Test connection by sending a silent message. Returns Ok on success,
    /// or an error if the bot token or chat id is invalid.
    #[instrument(skip(self), fields(chat = self.cfg.entry_chat_id))]
    pub async fn test_connection(&self) -> Result<()> {
        debug!("sending connection-test message");
        let params = SendMessageParams {
            business_connection_id: None,
            chat_id: TgChatId::Integer(self.cfg.entry_chat_id),
            message_thread_id: None,
            direct_messages_topic_id: None,
            text: "✓ lucarne online".into(),
            parse_mode: None,
            entities: None,
            link_preview_options: None,
            disable_notification: Some(true),
            protect_content: None,
            allow_paid_broadcast: None,
            message_effect_id: None,
            suggested_post_parameters: None,
            reply_parameters: None,
            reply_markup: None,
        };
        self.bot.send_message(&params).await.map_err(|e| {
            warn!(error = %e, "connection test failed");
            ChannelError::Transport(format!("connection test failed: {e}"))
        })?;
        info!("connection test ok");
        Ok(())
    }

    pub fn entry_chat(&self) -> ChatId {
        ChatId::new(self.cfg.entry_chat_id.to_string())
    }

    /// The chat-root workspace handle used for the entry panel.
    pub fn entry_handle(&self) -> WorkspaceHandle {
        WorkspaceHandle::new(self.entry_chat(), WorkspaceId::new(""))
    }

    pub async fn sync_commands(&self, commands: &[TelegramBotCommand]) -> Result<()> {
        let commands: Vec<_> = commands
            .iter()
            .map(|cmd| BotCommand {
                command: cmd.command.to_string(),
                description: cmd.description.to_string(),
            })
            .collect();
        let count = commands.len();
        let params = SetMyCommandsParams {
            commands,
            scope: None,
            language_code: None,
        };
        self.bot.set_my_commands(&params).await.map_err(map_err)?;
        info!(
            target: "lucarne_telegram",
            count,
            "telegram bot commands synced"
        );
        Ok(())
    }
}

fn parse_tg_chat_id(id: &ChatId) -> std::result::Result<TgChatId, ChannelError> {
    i64::from_str(id.as_str())
        .map(TgChatId::Integer)
        .map_err(|_| ChannelError::Transport(format!("invalid chat id: {}", id.as_str())))
}

fn parse_tg_thread(ws: &WorkspaceId) -> Option<i32> {
    if ws.as_str().is_empty() {
        return None;
    }
    i32::from_str(ws.as_str()).ok()
}

fn parse_tg_message_id(id: &MessageId) -> std::result::Result<i32, ChannelError> {
    i32::from_str(id.as_str())
        .map_err(|_| ChannelError::Transport(format!("invalid message id {}", id.as_str())))
}

/// Build inline keyboard from channel buttons.
///
/// Preserves the exact `Vec<Vec<…>>` grid from callers (panel nav, approvals,
/// interventions, history pagination, etc.). Uses `callback_data` only so the
/// existing bot callback routing is unchanged.
fn build_keyboard(rows: &[Vec<OutgoingButton>]) -> Option<InlineKeyboardMarkup> {
    if rows.is_empty() {
        return None;
    }
    let keyboard = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|b| InlineKeyboardButton {
                    text: b.label.clone(),
                    icon_custom_emoji_id: None,
                    url: None,
                    login_url: None,
                    callback_data: Some(b.data.clone()),
                    web_app: None,
                    switch_inline_query: None,
                    switch_inline_query_current_chat: None,
                    switch_inline_query_chosen_chat: None,
                    copy_text: None,
                    callback_game: None,
                    pay: None,
                    style: None,
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    Some(InlineKeyboardMarkup {
        inline_keyboard: keyboard,
    })
}

fn map_err(e: TgError) -> ChannelError {
    match &e {
        TgError::Api(api) => {
            let text = api.description.clone();
            let lower = text.to_ascii_lowercase();
            if lower.contains("message thread not found")
                || lower.contains("topic not found")
                || lower.contains("topic_id_invalid")
            {
                return ChannelError::WorkspaceNotFound(text);
            }
            if lower.contains("message is not modified") {
                // Treat as success at call sites that ignore this via map_err paths.
                return ChannelError::Transport(text);
            }
            if lower.contains("message to delete not found")
                || lower.contains("message can't be deleted")
            {
                return ChannelError::Transport(text);
            }
            if lower.contains("parse entities")
                || lower.contains("parse markdown")
                || lower.contains("can't parse")
                || lower.contains("can't find end of the entity")
                || lower.contains("unsupported start tag")
                || lower.contains("message is too long")
            {
                return if lower.contains("too long") {
                    ChannelError::PayloadTooLarge
                } else {
                    ChannelError::FormatRejected(text)
                };
            }
            ChannelError::Transport(text)
        }
        other => ChannelError::Transport(other.to_string()),
    }
}

fn is_benign_edit_error(e: &TgError) -> bool {
    match e {
        TgError::Api(api) => {
            let lower = api.description.to_ascii_lowercase();
            lower.contains("message is not modified")
        }
        _ => false,
    }
}

fn is_benign_delete_error(e: &TgError) -> bool {
    match e {
        TgError::Api(api) => {
            let lower = api.description.to_ascii_lowercase();
            lower.contains("message to delete not found")
                || lower.contains("message can't be deleted")
                || lower.contains("message identifier is not specified")
        }
        _ => false,
    }
}

/// Write in-memory bytes to a temp file for frankenstein's path-based upload API.
async fn materialize_upload(filename: &str, bytes: &[u8]) -> Result<(PathBuf, tempfile::TempDir)> {
    let dir = tempfile::tempdir()
        .map_err(|e| ChannelError::Transport(format!("tempdir for upload: {e}")))?;
    // Keep a simple basename so Telegram displays a clean name.
    let safe_name = filename
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("file.bin");
    let path = dir.path().join(safe_name);
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|e| ChannelError::Transport(format!("write upload temp: {e}")))?;
    Ok((path, dir))
}

fn reply_params_from(reply_to: Option<&MessageId>) -> Result<Option<ReplyParameters>> {
    match reply_to {
        None => Ok(None),
        Some(id) => {
            let message_id = parse_tg_message_id(id)?;
            Ok(Some(ReplyParameters {
                message_id,
                chat_id: None,
                allow_sending_without_reply: Some(true),
                quote: None,
                quote_parse_mode: None,
                quote_entities: None,
                quote_position: None,
                checklist_task_id: None,
                poll_option_id: None,
            }))
        }
    }
}

fn silent_flag(silent: bool) -> Option<bool> {
    silent.then_some(true)
}

fn rich_markdown_body(text: impl Into<String>) -> InputRichMessage {
    InputRichMessage {
        html: None,
        markdown: Some(text.into()),
        is_rtl: None,
        skip_entity_detection: None,
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    fn name(&self) -> &'static str {
        "telegram"
    }

    fn message_char_limit(&self) -> usize {
        TELEGRAM_MESSAGE_LIMIT
    }

    async fn send(&self, target: &WorkspaceHandle, msg: OutgoingMessage) -> Result<MessageId> {
        self.send_all(target, msg)
            .await?
            .into_iter()
            .last()
            .ok_or_else(|| ChannelError::Transport("no chunks sent".into()))
    }

    async fn send_all(
        &self,
        target: &WorkspaceHandle,
        msg: OutgoingMessage,
    ) -> Result<Vec<MessageId>> {
        let chat = parse_tg_chat_id(&target.chat)?;
        let thread = parse_tg_thread(&target.workspace);
        // Only agent bodies use Rich Messages; panels/help/status keep MarkdownV2.
        let use_rich = matches!(msg.format, TextFormat::Rich);
        let (wire_body, parse_mode) = match msg.format {
            TextFormat::Rich => (msg.body.clone(), None),
            TextFormat::Markdown => (
                render_telegram_markdown_v2(&msg.body),
                Some(ParseMode::MarkdownV2),
            ),
            TextFormat::Plain => (msg.body.clone(), None),
        };
        let chunks = split_for_channel(&wire_body, TELEGRAM_MESSAGE_LIMIT);
        let kb = build_keyboard(&msg.buttons);
        debug!(
            target: "lucarne_telegram",
            chat = ?chat,
            thread = ?thread,
            chunks = chunks.len(),
            bytes = wire_body.len(),
            format = ?msg.format,
            rich = use_rich,
            buttons = msg.buttons.len(),
            "sending message"
        );

        let mut message_ids = Vec::with_capacity(chunks.len());
        for (idx, chunk) in chunks.iter().enumerate() {
            let is_last = idx + 1 == chunks.len();
            // Preserve full button grid (rows × columns) on the final chunk only.
            let reply = if idx == 0 {
                reply_params_from(msg.reply_to.as_ref())?
            } else {
                None
            };
            let markup = if is_last {
                kb.clone().map(ReplyMarkup::InlineKeyboardMarkup)
            } else {
                None
            };
            let silent = silent_flag(msg.notification.is_silent());

            let message_id = if use_rich {
                let params = SendRichMessageParams {
                    business_connection_id: None,
                    chat_id: chat.clone(),
                    message_thread_id: thread,
                    direct_messages_topic_id: None,
                    // CommonMark-ish agent body; Telegram parses as Rich Markdown.
                    rich_message: rich_markdown_body(chunk.clone()),
                    disable_notification: silent,
                    protect_content: None,
                    allow_paid_broadcast: None,
                    message_effect_id: None,
                    suggested_post_parameters: None,
                    reply_parameters: reply,
                    reply_markup: markup,
                };
                let sent = self.bot.send_rich_message(&params).await.map_err(|e| {
                    let mapped = map_err(e);
                    warn!(
                        target: "lucarne_telegram",
                        chunk_idx = idx, error = %mapped,
                        "send rich chunk failed"
                    );
                    mapped
                })?;
                sent.result.message_id
            } else {
                let params = SendMessageParams {
                    business_connection_id: None,
                    chat_id: chat.clone(),
                    message_thread_id: thread,
                    direct_messages_topic_id: None,
                    text: chunk.clone(),
                    parse_mode,
                    entities: None,
                    link_preview_options: None,
                    disable_notification: silent,
                    protect_content: None,
                    allow_paid_broadcast: None,
                    message_effect_id: None,
                    suggested_post_parameters: None,
                    reply_parameters: reply,
                    reply_markup: markup,
                };
                let sent = self.bot.send_message(&params).await.map_err(|e| {
                    let mapped = map_err(e);
                    warn!(
                        target: "lucarne_telegram",
                        chunk_idx = idx, error = %mapped,
                        "send chunk failed"
                    );
                    mapped
                })?;
                sent.result.message_id
            };

            trace!(
                target: "lucarne_telegram",
                chunk_idx = idx,
                tg_message_id = message_id,
                "chunk sent"
            );
            message_ids.push(MessageId::new(message_id.to_string()));
        }
        if message_ids.is_empty() {
            return Err(ChannelError::Transport("no chunks sent".into()));
        }
        Ok(message_ids)
    }

    async fn edit(
        &self,
        target: &WorkspaceHandle,
        id: &MessageId,
        msg: OutgoingMessage,
    ) -> Result<()> {
        let chat = parse_tg_chat_id(&target.chat)?;
        let msg_id = parse_tg_message_id(id)?;
        let use_rich = matches!(msg.format, TextFormat::Rich);
        let (wire_body, parse_mode) = match msg.format {
            TextFormat::Rich => (msg.body.clone(), None),
            TextFormat::Markdown => (
                render_telegram_markdown_v2(&msg.body),
                Some(ParseMode::MarkdownV2),
            ),
            TextFormat::Plain => (msg.body.clone(), None),
        };
        let kb = build_keyboard(&msg.buttons);

        let truncated = if wire_body.chars().count() > TELEGRAM_MESSAGE_LIMIT {
            let mut s: String = wire_body.chars().take(TELEGRAM_MESSAGE_LIMIT - 16).collect();
            s.push_str(" …(truncated)");
            s
        } else {
            wire_body
        };

        let params = EditMessageTextParams {
            business_connection_id: None,
            chat_id: Some(chat),
            message_id: Some(msg_id),
            inline_message_id: None,
            text: if use_rich {
                None
            } else {
                Some(truncated.clone())
            },
            parse_mode,
            entities: None,
            link_preview_options: None,
            rich_message: use_rich.then(|| rich_markdown_body(truncated)),
            reply_markup: kb,
        };
        match self.bot.edit_message_text(&params).await {
            Ok(_) => Ok(()),
            Err(e) if is_benign_edit_error(&e) => Ok(()),
            Err(e) => Err(map_err(e)),
        }
    }

    async fn delete(&self, target: &WorkspaceHandle, id: &MessageId) -> Result<()> {
        let chat = parse_tg_chat_id(&target.chat)?;
        let msg_id = parse_tg_message_id(id)?;
        let params = DeleteMessageParams {
            chat_id: chat,
            message_id: msg_id,
        };
        match self.bot.delete_message(&params).await {
            Ok(_) => Ok(()),
            Err(e) if is_benign_delete_error(&e) => Ok(()),
            Err(e) => Err(map_err(e)),
        }
    }

    async fn create_workspace(&self, parent: &ChatId, title: &str) -> Result<WorkspaceHandle> {
        let chat = parse_tg_chat_id(parent)?;
        info!(target: "lucarne_telegram", chat = ?chat, title, "creating forum topic");
        let params = CreateForumTopicParams {
            chat_id: chat,
            name: title.to_string(),
            icon_color: None,
            icon_custom_emoji_id: None,
        };
        let topic = self.bot.create_forum_topic(&params).await.map_err(|e| {
            let m = map_err(e);
            warn!(target: "lucarne_telegram", error = %m, "create_forum_topic failed");
            m
        })?;
        let thread_id = topic.result.message_thread_id;
        info!(
            target: "lucarne_telegram",
            thread_id,
            "forum topic created"
        );
        Ok(WorkspaceHandle::new(
            parent.clone(),
            WorkspaceId::new(thread_id.to_string()),
        ))
    }

    async fn probe_workspace(&self, handle: &WorkspaceHandle) -> Result<()> {
        let chat = parse_tg_chat_id(&handle.chat)?;
        let params = SendChatActionParams {
            business_connection_id: None,
            chat_id: chat,
            message_thread_id: parse_tg_thread(&handle.workspace),
            action: ChatAction::Typing,
        };
        self.bot
            .send_chat_action(&params)
            .await
            .map_err(map_err)?;
        Ok(())
    }

    async fn rename_workspace(&self, handle: &WorkspaceHandle, title: &str) -> Result<()> {
        let chat = parse_tg_chat_id(&handle.chat)?;
        let thread = parse_tg_thread(&handle.workspace)
            .ok_or_else(|| ChannelError::Unsupported("rename requires a topic".into()))?;
        let params = EditForumTopicParams {
            chat_id: chat,
            message_thread_id: thread,
            name: Some(title.to_string()),
            icon_custom_emoji_id: None,
        };
        self.bot
            .edit_forum_topic(&params)
            .await
            .map_err(map_err)?;
        Ok(())
    }

    async fn delete_workspace(&self, handle: &WorkspaceHandle) -> Result<()> {
        let chat = parse_tg_chat_id(&handle.chat)?;
        let thread = parse_tg_thread(&handle.workspace)
            .ok_or_else(|| ChannelError::Unsupported("delete requires a topic".into()))?;
        let params = DeleteForumTopicParams {
            chat_id: chat,
            message_thread_id: thread,
        };
        self.bot
            .delete_forum_topic(&params)
            .await
            .map_err(map_err)?;
        Ok(())
    }

    fn subscribe(&self) -> BoxStream<'static, ChannelEvent> {
        let mut guard = self
            .events_rx
            .lock()
            .expect("Channel::subscribe called more than once");
        let rx = guard
            .take()
            .expect("Channel::subscribe can only be called once per Channel");
        futures::stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|ev| (ev, rx)) },
        )
        .boxed()
    }

    async fn download_attachment(&self, att: &IncomingAttachment) -> Result<Vec<u8>> {
        let params = GetFileParams {
            file_id: att.file_ref.clone(),
        };
        let file = self.bot.get_file(&params).await.map_err(map_err)?;
        let path = file
            .result
            .file_path
            .ok_or_else(|| ChannelError::Transport("getFile missing file_path".into()))?;
        // Official file CDN URL (token stays in path by Telegram design).
        let url = format!(
            "https://api.telegram.org/file/bot{}/{}",
            self.cfg.token, path
        );
        let bytes = self
            .bot
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| ChannelError::Transport(format!("download: {e}")))?
            .bytes()
            .await
            .map_err(|e| ChannelError::Transport(format!("download body: {e}")))?;
        Ok(bytes.to_vec())
    }

    async fn acknowledge(&self, target: &WorkspaceHandle) -> Result<()> {
        self.probe_workspace(target).await
    }

    async fn send_file(&self, target: &WorkspaceHandle, file: FileUpload) -> Result<MessageId> {
        let chat = parse_tg_chat_id(&target.chat)?;
        let thread = parse_tg_thread(&target.workspace);
        info!(
            target: "lucarne_telegram",
            chat = ?chat,
            thread = ?thread,
            filename = %file.filename,
            bytes = file.bytes.len(),
            "uploading file"
        );
        let (path, _tmp) = materialize_upload(&file.filename, &file.bytes).await?;
        let params = SendDocumentParams {
            business_connection_id: None,
            chat_id: chat,
            message_thread_id: thread,
            direct_messages_topic_id: None,
            document: TgFileUpload::InputFile(InputFile { path }),
            thumbnail: None,
            caption: file.caption,
            parse_mode: None,
            caption_entities: None,
            disable_content_type_detection: None,
            disable_notification: silent_flag(file.notification.is_silent()),
            protect_content: None,
            allow_paid_broadcast: None,
            message_effect_id: None,
            suggested_post_parameters: None,
            reply_parameters: reply_params_from(file.reply_to.as_ref())?,
            reply_markup: None,
        };
        let m = self.bot.send_document(&params).await.map_err(|e| {
            let m = map_err(e);
            warn!(target: "lucarne_telegram", error = %m, "send_document failed");
            m
        })?;
        Ok(MessageId::new(m.result.message_id.to_string()))
    }

    async fn send_attachment(
        &self,
        target: &WorkspaceHandle,
        attachment: Attachment,
    ) -> Result<MessageId> {
        let chat = parse_tg_chat_id(&target.chat)?;
        let thread = parse_tg_thread(&target.workspace);
        let kind = telegram_attachment_kind(&attachment.media_type);
        info!(
            target: "lucarne_telegram",
            chat = ?chat,
            thread = ?thread,
            filename = %attachment.filename,
            media_type = %attachment.media_type,
            bytes = attachment.bytes.len(),
            kind = ?kind,
            "uploading attachment"
        );
        let Attachment {
            filename,
            media_type: _,
            bytes,
            caption,
            reply_to,
            notification,
        } = attachment;
        let (path, _tmp) = materialize_upload(&filename, &bytes).await?;
        let file = TgFileUpload::InputFile(InputFile { path });
        let reply = reply_params_from(reply_to.as_ref())?;
        let silent = silent_flag(notification.is_silent());

        let message_id = match kind {
            TelegramAttachmentKind::Photo => {
                let params = SendPhotoParams {
                    business_connection_id: None,
                    chat_id: chat,
                    message_thread_id: thread,
                    direct_messages_topic_id: None,
                    photo: file,
                    caption,
                    parse_mode: None,
                    caption_entities: None,
                    show_caption_above_media: None,
                    has_spoiler: None,
                    disable_notification: silent,
                    protect_content: None,
                    allow_paid_broadcast: None,
                    message_effect_id: None,
                    suggested_post_parameters: None,
                    reply_parameters: reply,
                    reply_markup: None,
                };
                self.bot
                    .send_photo(&params)
                    .await
                    .map_err(|e| {
                        let m = map_err(e);
                        warn!(target: "lucarne_telegram", error = %m, "send_photo failed");
                        m
                    })?
                    .result
                    .message_id
            }
            TelegramAttachmentKind::Video => {
                let params = SendVideoParams {
                    business_connection_id: None,
                    chat_id: chat,
                    message_thread_id: thread,
                    direct_messages_topic_id: None,
                    video: file,
                    duration: None,
                    width: None,
                    height: None,
                    thumbnail: None,
                    cover: None,
                    start_timestamp: None,
                    caption,
                    parse_mode: None,
                    caption_entities: None,
                    show_caption_above_media: None,
                    has_spoiler: None,
                    supports_streaming: None,
                    disable_notification: silent,
                    protect_content: None,
                    allow_paid_broadcast: None,
                    message_effect_id: None,
                    suggested_post_parameters: None,
                    reply_parameters: reply,
                    reply_markup: None,
                };
                self.bot
                    .send_video(&params)
                    .await
                    .map_err(|e| {
                        let m = map_err(e);
                        warn!(target: "lucarne_telegram", error = %m, "send_video failed");
                        m
                    })?
                    .result
                    .message_id
            }
            TelegramAttachmentKind::Document => {
                let params = SendDocumentParams {
                    business_connection_id: None,
                    chat_id: chat,
                    message_thread_id: thread,
                    direct_messages_topic_id: None,
                    document: file,
                    thumbnail: None,
                    caption,
                    parse_mode: None,
                    caption_entities: None,
                    disable_content_type_detection: None,
                    disable_notification: silent,
                    protect_content: None,
                    allow_paid_broadcast: None,
                    message_effect_id: None,
                    suggested_post_parameters: None,
                    reply_parameters: reply,
                    reply_markup: None,
                };
                self.bot
                    .send_document(&params)
                    .await
                    .map_err(|e| {
                        let m = map_err(e);
                        warn!(target: "lucarne_telegram", error = %m, "send_document failed");
                        m
                    })?
                    .result
                    .message_id
            }
        };
        Ok(MessageId::new(message_id.to_string()))
    }

    async fn answer_command_query(
        &self,
        query: &CommandQuery,
        results: Vec<CommandQueryResult>,
    ) -> Result<()> {
        let tg_results = results
            .into_iter()
            .take(50)
            .map(|result| {
                let article = InlineQueryResultArticle {
                    id: result.id,
                    title: result.title,
                    input_message_content: InputMessageContent::Text(InputTextMessageContent {
                        message_text: result.message_text,
                        parse_mode: None,
                        entities: None,
                        link_preview_options: None,
                    }),
                    reply_markup: None,
                    url: None,
                    #[allow(deprecated)]
                    hide_url: None,
                    description: Some(result.description.unwrap_or_default()),
                    thumbnail_url: None,
                    thumbnail_width: None,
                    thumbnail_height: None,
                };
                InlineQueryResult::from(article)
            })
            .collect::<Vec<_>>();

        let params = AnswerInlineQueryParams {
            inline_query_id: query.id.clone(),
            results: tg_results,
            cache_time: Some(0),
            is_personal: Some(true),
            next_offset: None,
            button: None,
        };
        self.bot.answer_inline_query(&params).await.map_err(|e| {
            let m = map_err(e);
            warn!(target: "lucarne_telegram", error = %m, "answer_inline_query failed");
            m
        })?;
        Ok(())
    }
}

#[instrument(skip(bot, tx, cfg), fields(chat = cfg.entry_chat_id))]
async fn poll_updates(
    bot: Bot,
    tx: mpsc::Sender<ChannelEvent>,
    cfg: Arc<TelegramConfig>,
) -> Result<()> {
    lucarne::memory_profile_snapshot!("lucarne_telegram.channel.poll_updates.start");
    info!(target: "lucarne_telegram", "long-poll loop starting");
    let allowed = vec![
        AllowedUpdate::Message,
        AllowedUpdate::CallbackQuery,
        AllowedUpdate::ChannelPost,
        AllowedUpdate::InlineQuery,
    ];
    let mut offset: i64 = 0;
    loop {
        let params = GetUpdatesParams {
            offset: Some(offset),
            limit: None,
            timeout: Some(GET_UPDATES_TIMEOUT_SECS),
            allowed_updates: Some(allowed.clone()),
        };
        let res = bot.get_updates(&params).await;
        let updates = match res {
            Ok(resp) => resp.result,
            Err(e) => {
                warn!(target: "lucarne_telegram", error = %e, "getUpdates error, sleeping 3s");
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
        };
        if !updates.is_empty() {
            debug!(target: "lucarne_telegram", count = updates.len(), "received updates");
        }
        for u in updates {
            offset = i64::from(u.update_id) + 1;
            if let Some(ev) = translate_update(u, &cfg) {
                if tx.send(ev).await.is_err() {
                    warn!(target: "lucarne_telegram", "event channel closed, exiting poll loop");
                    return Ok(());
                }
            }
        }
    }
}

fn translate_update(u: Update, cfg: &TelegramConfig) -> Option<ChannelEvent> {
    match u.content {
        UpdateContent::Message(m) | UpdateContent::ChannelPost(m) => {
            if !authorized(cfg, m.from.as_ref().map(|u| u.id as i64)) {
                debug!(target: "lucarne_telegram", "ignoring unauthorized message");
                return None;
            }
            let chat = ChatId::new(m.chat.id.to_string());
            let workspace = m
                .message_thread_id
                .map(|t| WorkspaceId::new(t.to_string()));
            let user = m
                .from
                .as_ref()
                .map(|u| {
                    u.username
                        .clone()
                        .unwrap_or_else(|| format!("id:{}", u.id))
                })
                .unwrap_or_else(|| "unknown".into());
            let text = m.text.clone().or_else(|| m.caption.clone());
            let reply_to = m
                .reply_to_message
                .as_ref()
                .map(|reply| MessageId::new(reply.message_id.to_string()));
            let mut attachments = Vec::new();
            if let Some(photo) = m.photo.as_deref().and_then(largest_photo) {
                attachments.push(IncomingAttachment {
                    file_ref: photo.file_id.clone(),
                    filename: Some("photo.jpg".into()),
                    mime_type: Some("image/jpeg".into()),
                    size: photo.file_size,
                });
            }
            if let Some(doc) = m.document.as_ref() {
                attachments.push(IncomingAttachment {
                    file_ref: doc.file_id.clone(),
                    filename: doc.file_name.clone(),
                    mime_type: doc.mime_type.clone(),
                    size: doc.file_size,
                });
            }
            Some(ChannelEvent::Message(IncomingMessage {
                message_id: MessageId::new(m.message_id.to_string()),
                chat,
                workspace,
                reply_to,
                user,
                text,
                attachments,
            }))
        }
        UpdateContent::InlineQuery(q) => {
            if !authorized(cfg, Some(q.from.id as i64)) {
                debug!(target: "lucarne_telegram", "ignoring unauthorized inline query");
                return None;
            }
            let user = q
                .from
                .username
                .clone()
                .unwrap_or_else(|| format!("id:{}", q.from.id));
            Some(ChannelEvent::CommandQuery(CommandQuery {
                id: q.id,
                user,
                query: q.query,
                chat_type: q.chat_type,
            }))
        }
        UpdateContent::CallbackQuery(q) => {
            if !authorized(cfg, Some(q.from.id as i64)) {
                return None;
            }
            let data = q.data?;
            let msg = q.message?;
            let (chat_id, workspace, message_id) = match msg {
                MaybeInaccessibleMessage::Message(m) => (
                    m.chat.id,
                    m.message_thread_id
                        .map(|t| WorkspaceId::new(t.to_string())),
                    m.message_id,
                ),
                MaybeInaccessibleMessage::InaccessibleMessage(m) => {
                    (m.chat.id, None, m.message_id)
                }
            };
            let chat = ChatId::new(chat_id.to_string());
            let source_message = MessageId::new(message_id.to_string());
            let user = q
                .from
                .username
                .clone()
                .unwrap_or_else(|| format!("id:{}", q.from.id));
            Some(ChannelEvent::Button {
                chat,
                workspace,
                user,
                data,
                source_message,
            })
        }
        _ => None,
    }
}

fn largest_photo(photos: &[PhotoSize]) -> Option<&PhotoSize> {
    photos
        .iter()
        .max_by_key(|p| u64::from(p.width) * u64::from(p.height))
}

fn authorized(cfg: &TelegramConfig, user_id: Option<i64>) -> bool {
    if cfg.authorized_user_ids.is_empty() {
        return true;
    }
    user_id
        .map(|id| cfg.authorized_user_ids.contains(&id))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn test_config() -> TelegramConfig {
        TelegramConfig {
            token: "test".into(),
            entry_chat_id: 100,
            authorized_user_ids: Vec::new(),
        }
    }

    #[test]
    fn telegram_attachment_kind_uses_native_media_for_images_and_videos() {
        assert_eq!(
            telegram_attachment_kind("image/png"),
            TelegramAttachmentKind::Photo
        );
        assert_eq!(
            telegram_attachment_kind(" IMAGE/JPEG "),
            TelegramAttachmentKind::Photo
        );
        assert_eq!(
            telegram_attachment_kind("video/mp4"),
            TelegramAttachmentKind::Video
        );
        assert_eq!(
            telegram_attachment_kind("application/pdf"),
            TelegramAttachmentKind::Document
        );
    }

    #[tokio::test]
    async fn dropping_channel_aborts_poll_task() {
        struct NotifyOnDrop(Option<tokio::sync::oneshot::Sender<()>>);

        impl Drop for NotifyOnDrop {
            fn drop(&mut self) {
                if let Some(done) = self.0.take() {
                    let _ = done.send(());
                }
            }
        }

        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let poll_task = tokio::spawn(async move {
            let _notify = NotifyOnDrop(Some(done_tx));
            std::future::pending::<()>().await;
        });
        let (_tx, rx) = tokio::sync::mpsc::channel(EVENT_QUEUE);
        let channel = TelegramChannel {
            bot: Bot::new("test"),
            events_rx: Mutex::new(Some(rx)),
            _poll_task: poll_task,
            cfg: Arc::new(test_config()),
        };
        tokio::task::yield_now().await;

        drop(channel);

        tokio::time::timeout(Duration::from_secs(1), done_rx)
            .await
            .expect("poll task should be aborted when channel is dropped")
            .expect("poll task drop notifier should fire");
    }

    #[test]
    fn telegram_channel_exposes_shared_reqwest_client_constructor() {
        let source = include_str!("channel.rs")
            .split("\n#[cfg(test)]")
            .next()
            .expect("production source");
        assert!(source.contains("pub fn start_with_client"));
        assert!(source.contains("http_client: reqwest::Client"));
        // Daemon still injects its client for API stability; frankenstein uses
        // reqwest 0.13 so we rebuild an equivalent client rather than sharing
        // the 0.12 handle with wechat-ilink.
        assert!(
            source.contains("frankenstein::reqwest::Client::builder()"),
            "Telegram bot must construct a frankenstein-compatible HTTP client"
        );
        assert!(
            !source.contains("teloxide"),
            "teloxide must be fully removed from channel production code"
        );
    }

    #[test]
    fn build_keyboard_preserves_row_and_column_layout_and_callback_data() {
        let rows = vec![
            vec![
                OutgoingButton {
                    label: "A".into(),
                    data: "a:1".into(),
                },
                OutgoingButton {
                    label: "B".into(),
                    data: "b:2".into(),
                },
            ],
            vec![OutgoingButton {
                label: "C".into(),
                data: "c:3".into(),
            }],
        ];
        let kb = build_keyboard(&rows).expect("keyboard");
        assert_eq!(kb.inline_keyboard.len(), 2, "two rows");
        assert_eq!(kb.inline_keyboard[0].len(), 2, "first row has 2 buttons");
        assert_eq!(kb.inline_keyboard[1].len(), 1, "second row has 1 button");
        assert_eq!(kb.inline_keyboard[0][0].text, "A");
        assert_eq!(
            kb.inline_keyboard[0][0].callback_data.as_deref(),
            Some("a:1")
        );
        assert_eq!(
            kb.inline_keyboard[0][1].callback_data.as_deref(),
            Some("b:2")
        );
        assert_eq!(
            kb.inline_keyboard[1][0].callback_data.as_deref(),
            Some("c:3")
        );
        // No accidental URL/pay buttons — callback-only contract for bot routing.
        assert!(kb.inline_keyboard[0][0].url.is_none());
        assert!(kb.inline_keyboard[0][0].pay.is_none());
    }

    #[test]
    fn memory_profile_snapshots_mark_telegram_channel_startup() {
        let source = include_str!("channel.rs");

        for label in [
            "lucarne_telegram.channel.start.start",
            "lucarne_telegram.channel.start.after_bot_new",
            "lucarne_telegram.channel.start.after_poll_spawn",
            "lucarne_telegram.channel.poll_updates.start",
        ] {
            let needle = format!("lucarne::memory_profile_snapshot!(\"{label}\")");
            assert!(source.contains(&needle), "missing snapshot {label}");
        }
    }

    #[test]
    fn agent_rich_and_ui_markdown_v2_are_split() {
        let source = include_str!("channel.rs")
            .split("\n#[cfg(test)]")
            .next()
            .expect("production source");
        assert!(source.contains("send_rich_message"));
        assert!(source.contains("InputRichMessage"));
        assert!(
            source.contains("render_telegram_markdown_v2"),
            "UI chrome must keep MarkdownV2 path"
        );
        assert!(
            source.contains("TextFormat::Rich"),
            "only TextFormat::Rich may use rich messages"
        );
        assert!(
            source.contains("callback_data"),
            "inline buttons must keep callback_data for bot routing"
        );
    }

    #[test]
    fn photo_message_becomes_image_attachment_with_caption_text() {
        let update: Update = serde_json::from_str(
            r#"{
                "update_id": 1,
                "message": {
                    "message_id": 10,
                    "from": {
                        "id": 123,
                        "is_bot": false,
                        "first_name": "era",
                        "username": "era"
                    },
                    "chat": {
                        "id": 100,
                        "type": "supergroup",
                        "title": "lucarne"
                    },
                    "message_thread_id": 2,
                    "date": 1,
                    "caption": "这是个啥",
                    "photo": [
                        {
                            "file_id": "small-photo",
                            "file_unique_id": "small-unique",
                            "file_size": 100,
                            "width": 90,
                            "height": 90
                        },
                        {
                            "file_id": "large-photo",
                            "file_unique_id": "large-unique",
                            "file_size": 12345,
                            "width": 800,
                            "height": 600
                        }
                    ]
                }
            }"#,
        )
        .unwrap();

        let ev = translate_update(update, &test_config()).expect("channel event");
        let ChannelEvent::Message(msg) = ev else {
            panic!("expected message");
        };

        assert_eq!(msg.text.as_deref(), Some("这是个啥"));
        assert_eq!(msg.workspace.as_ref().map(|w| w.as_str()), Some("2"));
        assert_eq!(msg.attachments.len(), 1);
        assert_eq!(msg.attachments[0].file_ref, "large-photo");
        assert_eq!(msg.attachments[0].filename.as_deref(), Some("photo.jpg"));
        assert_eq!(msg.attachments[0].mime_type.as_deref(), Some("image/jpeg"));
        assert_eq!(msg.attachments[0].size, Some(12345));
    }

    #[test]
    fn callback_query_keeps_callback_data_and_source_message() {
        let update: Update = serde_json::from_str(
            r#"{
                "update_id": 2,
                "callback_query": {
                    "id": "cb1",
                    "from": {
                        "id": 123,
                        "is_bot": false,
                        "first_name": "era",
                        "username": "era"
                    },
                    "message": {
                        "message_id": 42,
                        "date": 1,
                        "chat": { "id": 100, "type": "supergroup", "title": "lucarne" },
                        "message_thread_id": 7,
                        "text": "pick"
                    },
                    "chat_instance": "x",
                    "data": "agentcmd:c:t1"
                }
            }"#,
        )
        .unwrap();
        let ev = translate_update(update, &test_config()).expect("button event");
        let ChannelEvent::Button {
            data,
            source_message,
            workspace,
            ..
        } = ev
        else {
            panic!("expected button");
        };
        assert_eq!(data, "agentcmd:c:t1");
        assert_eq!(source_message.as_str(), "42");
        assert_eq!(workspace.as_ref().map(|w| w.as_str()), Some("7"));
    }

    fn real_bot_api_channel_from_env() -> Option<Arc<TelegramChannel>> {
        let _ = dotenvy::dotenv();
        if std::env::var("LUCARNE_TELEGRAM_REAL_E2E").ok().as_deref() != Some("1") {
            return None;
        }
        let token = std::env::var("TELEGRAM_BOT_TOKEN").ok()?;
        let entry_chat_id = std::env::var("TELEGRAM_CHAT_ID")
            .or_else(|_| std::env::var("LUCARNE_ENTRY_CHAT_ID"))
            .ok()?
            .parse()
            .ok()?;
        let cfg = Arc::new(TelegramConfig {
            token: token.clone(),
            entry_chat_id,
            authorized_user_ids: Vec::new(),
        });
        let (_tx, rx) = tokio::sync::mpsc::channel(EVENT_QUEUE);
        Some(Arc::new(TelegramChannel {
            bot: Bot::new(&token),
            events_rx: Mutex::new(Some(rx)),
            _poll_task: tokio::spawn(async { std::future::pending::<()>().await }),
            cfg,
        }))
    }

    #[tokio::test]
    async fn real_bot_api_forum_topic_keyboard_reply_round_trip() {
        let Some(channel) = real_bot_api_channel_from_env() else {
            return;
        };
        channel
            .test_connection()
            .await
            .expect("telegram connection");
        let parent = channel.entry_chat();
        let topic_title = format!(
            "lucarne-real-e2e-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        );
        let topic = channel
            .create_workspace(&parent, &topic_title)
            .await
            .expect("create forum topic");

        let first = channel
            .send(
                &topic,
                OutgoingMessage::plain("lucarne real Telegram Bot API e2e")
                    .with_buttons(vec![vec![OutgoingButton {
                        label: "status".into(),
                        data: "agentcmd:c:t1".into(),
                    }]])
                    .silent(),
            )
            .await
            .expect("send keyboard message");
        let reply = channel
            .send(
                &topic,
                OutgoingMessage::plain("reply check")
                    .reply_to(first.clone())
                    .silent(),
            )
            .await
            .expect("send reply");
        channel
            .edit(
                &topic,
                &first,
                OutgoingMessage::plain("lucarne real Telegram Bot API e2e edited").with_buttons(
                    vec![vec![OutgoingButton {
                        label: "refresh".into(),
                        data: "refresh:1".into(),
                    }]],
                ),
            )
            .await
            .expect("edit message");
        channel.probe_workspace(&topic).await.expect("probe topic");
        channel.delete(&topic, &reply).await.expect("delete reply");
        channel
            .delete_workspace(&topic)
            .await
            .expect("delete forum topic");
    }
}
