use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use futures::{stream::BoxStream, StreamExt};
use lucarne::{default_lucarned_home_dir, LucarneCore};
use lucarne_adapter::{
    AdapterConfig, AdapterContext, AdapterError, AdapterPlugin, AdapterResult, AdapterTask,
};
use qrcode::{types::Color, EcLevel, QrCode};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};
use wechat_ilink::{
    Credentials, LoginQrEvent, SendReceipt, WechatContext, WechatEvent, WechatIlinkClient,
    WechatIlinkClientBuilder, WechatIlinkError,
};

use crate::context_store::WechatContextStore;
use crate::service::{
    WechatError, WechatIncoming, WechatNotificationService, WechatSendReceipt,
    WechatServiceOptions, WechatTransport, WechatUserInteractionRequest,
};

/// Adapter-owned context expiry reminder configuration.
#[derive(Debug, Clone)]
pub struct WechatContextExpiryReminderConfig {
    pub expires_after: Duration,
    pub remind_before: Duration,
    pub prompt_template: String,
}

impl Default for WechatContextExpiryReminderConfig {
    fn default() -> Self {
        Self {
            expires_after: Duration::from_secs(7200),
            remind_before: Duration::from_secs(300),
            prompt_template: DEFAULT_CONTEXT_EXPIRY_REMINDER_TEMPLATE.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WechatConfig {
    pub base_url: Option<String>,
    pub cred_path: Option<String>,
    pub bot_agent: Option<String>,
    pub ilink_app_id: Option<String>,
    pub route_tag: Option<String>,
    pub markdown_filter: bool,
    pub context_expiry_reminder: Option<WechatContextExpiryReminderConfig>,
    pub rate_limit_interaction_prompt: Option<String>,
    pub force_login: bool,
    pub notify_user_ids: Vec<String>,
}

impl Default for WechatConfig {
    fn default() -> Self {
        Self {
            base_url: None,
            cred_path: None,
            bot_agent: None,
            ilink_app_id: None,
            route_tag: None,
            markdown_filter: true,
            context_expiry_reminder: None,
            rate_limit_interaction_prompt: None,
            force_login: false,
            notify_user_ids: Vec::new(),
        }
    }
}

pub struct WechatAdapterPlugin;

pub fn wechat_plugin() -> WechatAdapterPlugin {
    WechatAdapterPlugin
}

#[async_trait]
impl AdapterPlugin for WechatAdapterPlugin {
    fn id(&self) -> &'static str {
        "wechat"
    }

    fn name(&self) -> &'static str {
        "WeChat"
    }

    fn startup_priority(&self) -> i32 {
        -100
    }

    fn enabled(&self, config: &AdapterConfig) -> bool {
        let configured = config.channel_enabled(self.id());
        let auto_enabled = configured
            .is_none()
            .then(|| wechat_auto_enabled(config))
            .unwrap_or(false);
        let enabled = configured.unwrap_or(auto_enabled);
        debug!(
            target: "lucarne_wechat::adapter",
            enabled,
            configured,
            auto_enabled,
            "wechat adapter enablement checked"
        );
        enabled
    }

    async fn spawn(&self, ctx: AdapterContext) -> AdapterResult<AdapterTask> {
        lucarne::memory_profile_snapshot!("lucarne_wechat.adapter.spawn.start");
        let config = wechat_config_from_adapter_config(&*ctx.config);
        let core = Arc::clone(&ctx.core);
        let shutdown = ctx.shutdown.clone();
        lucarne::memory_profile_snapshot!("lucarne_wechat.adapter.spawn.before_transport_new");
        let transport = Arc::new(
            WechatIlinkTransport::new_with_client(
                &config,
                ctx.http_client.clone(),
                core.sqlite_connection(),
            )
            .await,
        );
        lucarne::memory_profile_snapshot!("lucarne_wechat.adapter.spawn.after_transport_new");
        transport
            .login(config.force_login)
            .await
            .map_err(|err| AdapterError::message(err.to_string()))?;
        lucarne::memory_profile_snapshot!("lucarne_wechat.adapter.spawn.after_login");
        let restored_users = transport.known_user_ids().await.len();
        lucarne::memory_profile_snapshot!("lucarne_wechat.adapter.spawn.after_known_user_ids");
        info!(
            target: "lucarne_wechat::adapter",
            configured_users = config.notify_user_ids.len(),
            restored_users,
            force_login = config.force_login,
            context_expiry_reminder_enabled = config.context_expiry_reminder.is_some(),
            rate_limit_interaction_prompt_enabled = config.rate_limit_interaction_prompt.is_some(),
            "wechat adapter spawning"
        );

        lucarne::memory_profile_snapshot!("lucarne_wechat.adapter.spawn.before_task_spawn");
        Ok(AdapterTask::spawn(self.id(), async move {
            run_wechat_adapter_with_transport(core, config, shutdown, transport)
                .await
                .map_err(|err| AdapterError::message(err.to_string()))
        }))
    }
}

fn wechat_config_from_adapter_config(config: &AdapterConfig) -> WechatConfig {
    WechatConfig {
        base_url: config
            .get("LUCARNE_WECHAT_BASE_URL")
            .map(ToString::to_string),
        cred_path: config
            .channel_value("LUCARNE_WECHAT_CRED_PATH", "wechat", "credential_path")
            .map(ToString::to_string),
        bot_agent: config
            .get("LUCARNE_WECHAT_BOT_AGENT")
            .map(ToString::to_string),
        ilink_app_id: config
            .get("LUCARNE_WECHAT_ILINK_APP_ID")
            .map(ToString::to_string),
        route_tag: config
            .get("LUCARNE_WECHAT_ROUTE_TAG")
            .map(ToString::to_string),
        markdown_filter: config
            .get("LUCARNE_WECHAT_MARKDOWN_FILTER")
            .and_then(parse_bool)
            .unwrap_or(true),
        context_expiry_reminder: wechat_context_expiry_reminder_from_adapter_config(config),
        rate_limit_interaction_prompt: Some(
            config
                .channel_value(
                    "LUCARNE_WECHAT_RATE_LIMIT_INTERACTION_PROMPT",
                    "wechat",
                    "rate_limit.interaction_prompt",
                )
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(DEFAULT_RATE_LIMIT_INTERACTION_PROMPT)
                .to_string(),
        ),
        force_login: config
            .channel_value("LUCARNE_WECHAT_FORCE_LOGIN", "wechat", "force_login")
            .and_then(parse_bool)
            .unwrap_or(false),
        notify_user_ids: config
            .get("LUCARNE_WECHAT_NOTIFY_USER_IDS")
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect(),
    }
}

fn wechat_auto_enabled(config: &AdapterConfig) -> bool {
    let config = wechat_config_from_adapter_config(config);
    config.force_login
        || !config.notify_user_ids.is_empty()
        || wechat_credential_path(&config).is_file()
}

const DEFAULT_CONTEXT_EXPIRY_REMINDER_TEMPLATE: &str =
    "会话将在 {remaining_minutes} 分钟后到期，请回复以保持会话可用。";
const DEFAULT_RATE_LIMIT_INTERACTION_PROMPT: &str =
    "微信主动通知快到发送限制了，请回复任意消息以刷新会话。";

fn wechat_context_expiry_reminder_from_adapter_config(
    config: &AdapterConfig,
) -> Option<WechatContextExpiryReminderConfig> {
    let default = WechatContextExpiryReminderConfig::default();
    let expires_after = config
        .channel_value(
            "LUCARNE_WECHAT_CONTEXT_TTL_SECS",
            "wechat",
            "context.ttl_secs",
        )
        .and_then(parse_duration_secs)
        .unwrap_or(default.expires_after);
    let remind_before = config
        .channel_value(
            "LUCARNE_WECHAT_CONTEXT_EXPIRY_REMIND_BEFORE_SECS",
            "wechat",
            "context.expiry_remind_before_secs",
        )
        .and_then(parse_duration_secs)
        .unwrap_or(default.remind_before);
    let prompt_template = config
        .channel_value(
            "LUCARNE_WECHAT_CONTEXT_EXPIRY_REMINDER_TEMPLATE",
            "wechat",
            "context.expiry_reminder_template",
        )
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default.prompt_template.as_str());
    Some(WechatContextExpiryReminderConfig {
        expires_after,
        remind_before,
        prompt_template: prompt_template.to_string(),
    })
}

fn parse_duration_secs(value: &str) -> Option<Duration> {
    let secs = value.trim().parse::<u64>().ok()?;
    (secs > 0).then(|| Duration::from_secs(secs))
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

pub async fn run_wechat_adapter(
    core: Arc<LucarneCore>,
    config: WechatConfig,
    shutdown: watch::Receiver<bool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let transport = Arc::new(WechatIlinkTransport::new(&config, core.sqlite_connection()).await);
    transport.login(config.force_login).await?;
    run_wechat_adapter_with_transport(core, config, shutdown, transport).await
}

async fn run_wechat_adapter_with_transport(
    core: Arc<LucarneCore>,
    config: WechatConfig,
    shutdown: watch::Receiver<bool>,
    transport: Arc<WechatIlinkTransport>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let poll_transport = Arc::clone(&transport);
    let poll_task = tokio::spawn(async move { poll_transport.run().await });
    let restored_user_ids = transport.known_user_ids().await;
    let restored_user_count = restored_user_ids.len();
    let initial_user_ids = startup_notification_user_ids(config.notify_user_ids, restored_user_ids);
    debug!(
        target: "lucarne_wechat::adapter",
        configured_users = initial_user_ids.len(),
        restored_users = restored_user_count,
        "wechat startup notification recipients prepared"
    );

    let mut service = WechatNotificationService::new(
        core,
        transport,
        WechatServiceOptions {
            initial_user_ids,
            rate_limit_interaction_prompt: config.rate_limit_interaction_prompt.clone(),
            ..WechatServiceOptions::default()
        },
    );
    if let Some(reminder_config) = config.context_expiry_reminder.clone() {
        service = service.with_context_expiry_reminder(reminder_config);
    }
    let result = service.run_until_shutdown(shutdown).await;
    service.transport().stop().await?;

    match poll_task.await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => warn!(
            target: "lucarne_wechat::adapter",
            error = %err,
            "wechat polling stopped with error"
        ),
        Err(err) => warn!(
            target: "lucarne_wechat::adapter",
            error = %err,
            "wechat polling task failed"
        ),
    }

    result?;
    Ok(())
}

fn startup_notification_user_ids(
    mut configured_user_ids: Vec<String>,
    restored_user_ids: Vec<String>,
) -> Vec<String> {
    configured_user_ids.extend(restored_user_ids);
    configured_user_ids.sort();
    configured_user_ids.dedup();
    configured_user_ids
}

pub struct WechatIlinkTransport {
    bot: Arc<WechatIlinkClient>,
    store: WechatContextStore,
    credential_path: PathBuf,
    account_key: Mutex<Option<String>>,
    incoming_tx: mpsc::Sender<WechatIncoming>,
    events_rx: Mutex<Option<mpsc::Receiver<WechatIncoming>>>,
    interaction_tx: mpsc::Sender<WechatUserInteractionRequest>,
    interaction_rx: Mutex<Option<mpsc::Receiver<WechatUserInteractionRequest>>>,
}

fn account_key(creds: &wechat_ilink::Credentials) -> String {
    if !creds.account_id.is_empty() {
        creds.account_id.clone()
    } else {
        creds.user_id.clone()
    }
}

fn wechat_credential_path(config: &WechatConfig) -> PathBuf {
    config
        .cred_path
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| default_lucarned_home_dir().map(|home| home.join("wechat-credentials.json")))
        .unwrap_or_else(|| PathBuf::from("wechat-credentials.json"))
}

async fn load_wechat_credentials(path: &Path) -> Result<Option<Credentials>, WechatError> {
    match tokio::fs::read_to_string(path).await {
        Ok(data) => serde_json::from_str(&data)
            .map(Some)
            .map_err(|err| WechatError::Transport(err.to_string())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(WechatError::Transport(err.to_string())),
    }
}

async fn save_wechat_credentials(path: &Path, creds: &Credentials) -> Result<(), WechatError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|err| WechatError::Transport(err.to_string()))?;
    }
    let data = serde_json::to_string_pretty(creds)
        .map_err(|err| WechatError::Transport(err.to_string()))?;
    tokio::fs::write(path, format!("{data}\n"))
        .await
        .map_err(|err| WechatError::Transport(err.to_string()))
}

fn configure_wechat_client_builder(
    mut builder: WechatIlinkClientBuilder,
    config: &WechatConfig,
) -> WechatIlinkClientBuilder {
    builder = builder.markdown_filter(config.markdown_filter);
    if let Some(base_url) = config.base_url.clone() {
        builder = builder.base_url(base_url);
    }
    if let Some(bot_agent) = config.bot_agent.clone() {
        builder = builder.bot_agent(bot_agent);
    }
    if let Some(ilink_app_id) = config.ilink_app_id.clone() {
        builder = builder.ilink_app_id(ilink_app_id);
    }
    if let Some(route_tag) = config.route_tag.clone() {
        builder = builder.route_tag(route_tag);
    }
    if let Some(reminder) = config.context_expiry_reminder.clone() {
        builder = builder
            .context_ttl(reminder.expires_after)
            .context_expiry_remind_before(reminder.remind_before);
    }
    builder
}

impl WechatIlinkTransport {
    pub async fn new(
        config: &WechatConfig,
        sqlite: Arc<std::sync::Mutex<rusqlite::Connection>>,
    ) -> Self {
        Self::new_with_builder(config, WechatIlinkClient::builder(), sqlite).await
    }

    pub async fn new_with_client(
        config: &WechatConfig,
        http_client: reqwest::Client,
        sqlite: Arc<std::sync::Mutex<rusqlite::Connection>>,
    ) -> Self {
        Self::new_with_builder(
            config,
            WechatIlinkClient::builder().http_client(http_client),
            sqlite,
        )
        .await
    }

    async fn new_with_builder(
        config: &WechatConfig,
        builder: WechatIlinkClientBuilder,
        sqlite: Arc<std::sync::Mutex<rusqlite::Connection>>,
    ) -> Self {
        let store = WechatContextStore::open(sqlite).expect("wechat context store");

        let bot = Arc::new(configure_wechat_client_builder(builder, config).build());

        let (incoming_tx, rx) = mpsc::channel(128);
        let (interaction_tx, interaction_rx) = mpsc::channel(32);

        Self {
            bot,
            store,
            credential_path: wechat_credential_path(config),
            account_key: Mutex::new(None),
            incoming_tx,
            events_rx: Mutex::new(Some(rx)),
            interaction_tx,
            interaction_rx: Mutex::new(Some(interaction_rx)),
        }
    }

    async fn login(&self, force: bool) -> Result<(), WechatError> {
        let creds = if !force {
            match load_wechat_credentials(&self.credential_path).await? {
                Some(creds) => {
                    self.bot.set_credentials(creds.clone()).await;
                    creds
                }
                None => {
                    let creds = self.login_qr().await?;
                    save_wechat_credentials(&self.credential_path, &creds).await?;
                    creds
                }
            }
        } else {
            let creds = self.login_qr().await?;
            save_wechat_credentials(&self.credential_path, &creds).await?;
            creds
        };
        *self.account_key.lock().expect("wechat account key lock") = Some(account_key(&creds));
        Ok(())
    }

    async fn run(&self) -> Result<(), WechatError> {
        let cursor = {
            let account_key = self
                .account_key
                .lock()
                .expect("wechat account key lock")
                .clone();
            match account_key {
                Some(account_key) => self
                    .store
                    .cursor(&account_key)
                    .await
                    .map_err(|err| WechatError::Transport(err.to_string()))?,
                None => None,
            }
        };
        let mut events = Arc::clone(&self.bot).events_from_cursor(cursor);
        while let Some(event) = events.next().await {
            self.handle_event(event.map_err(map_ilink_error)?).await;
        }
        Ok(())
    }

    async fn login_qr(&self) -> Result<Credentials, WechatError> {
        let mut login = self.bot.login_qr();
        while let Some(event) = login.next().await {
            match event.map_err(map_ilink_error)? {
                LoginQrEvent::QrCode { content } => show_login_qr(&content),
                LoginQrEvent::StatusChanged { status } => {
                    debug!(
                        target: "lucarne_wechat::adapter",
                        status = %status,
                        "wechat login status changed"
                    );
                }
                LoginQrEvent::NeedVerifyCode { prompt, responder } => {
                    let _ = responder.cancel();
                    return Err(WechatError::Transport(prompt));
                }
                LoginQrEvent::Confirmed { credentials } => return Ok(credentials),
            }
        }
        Err(WechatError::Transport(
            "wechat QR login stream ended before confirmation".into(),
        ))
    }

    async fn handle_event(&self, event: WechatEvent) {
        match event {
            WechatEvent::ContextObserved(ctx) => {
                if let Err(err) = self.store.upsert_context(ctx).await {
                    warn!(
                        target: "lucarne_wechat::adapter",
                        error = %err,
                        "context store upsert failed"
                    );
                }
            }
            WechatEvent::CursorAdvanced {
                account_key,
                cursor,
            } => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                if let Err(err) = self.store.save_cursor(&account_key, cursor, now).await {
                    warn!(
                        target: "lucarne_wechat::adapter",
                        error = %err,
                        "cursor save failed"
                    );
                }
            }
            WechatEvent::Message(msg) => {
                if let Err(err) = self.incoming_tx.try_send(WechatIncoming::from_sdk(msg)) {
                    warn!(
                        target: "lucarne_wechat::adapter",
                        error = %err,
                        "wechat incoming queue full"
                    );
                }
            }
            WechatEvent::AuthSessionExpired { account_key } => {
                if let Err(err) = self.store.disable_account(&account_key).await {
                    warn!(
                        target: "lucarne_wechat::adapter",
                        error = %err,
                        "disable account on auth session expired failed"
                    );
                }
            }
            WechatEvent::UserInteractionRequested {
                account_key,
                user_id,
                reason,
            } => {
                let request = WechatUserInteractionRequest {
                    account_key,
                    user_id,
                    reason,
                };
                if let Err(err) = self.interaction_tx.try_send(request) {
                    warn!(
                        target: "lucarne_wechat::adapter",
                        error = %err,
                        "wechat interaction queue full"
                    );
                }
            }
        }
    }

    async fn known_user_ids(&self) -> Vec<String> {
        let all = self.store.all_contexts().await.unwrap_or_default();
        let mut users: Vec<String> = all
            .into_iter()
            .filter(|ctx| !ctx.disabled)
            .map(|ctx| ctx.context.user_id)
            .collect();
        users.sort();
        users.dedup();
        users
    }
}

#[async_trait]
impl WechatTransport for WechatIlinkTransport {
    fn subscribe(&self) -> BoxStream<'static, WechatIncoming> {
        let rx = self
            .events_rx
            .lock()
            .expect("wechat event receiver lock")
            .take()
            .expect("WechatTransport::subscribe called more than once");
        futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|message| (message, rx))
        })
        .boxed()
    }

    fn subscribe_user_interactions(&self) -> BoxStream<'static, WechatUserInteractionRequest> {
        let rx = self
            .interaction_rx
            .lock()
            .expect("wechat interaction receiver lock")
            .take()
            .expect("WechatTransport::subscribe_user_interactions called more than once");
        futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|request| (request, rx))
        })
        .boxed()
    }

    async fn context_for_user(&self, user_id: &str) -> Result<Option<WechatContext>, WechatError> {
        self.store
            .context_by_user(user_id)
            .await
            .map_err(|err| WechatError::Transport(err.to_string()))
    }

    async fn send(
        &self,
        context: &WechatContext,
        text: &str,
    ) -> Result<WechatSendReceipt, WechatError> {
        self.bot
            .send_text_with_context(context, text)
            .await
            .map(send_receipt)
            .map_err(map_ilink_error)
    }

    async fn reply(
        &self,
        message: &WechatIncoming,
        text: &str,
    ) -> Result<WechatSendReceipt, WechatError> {
        let Some(raw) = message.sdk_message.as_ref() else {
            return Err(WechatError::MissingMessageContext(
                message.message_id.clone(),
            ));
        };
        self.bot
            .reply(raw, text)
            .await
            .map(send_receipt)
            .map_err(map_ilink_error)
    }

    async fn send_typing(&self, context: &WechatContext) -> Result<(), WechatError> {
        self.bot
            .send_typing_with_context(context)
            .await
            .map_err(map_ilink_error)
    }

    async fn stop(&self) -> Result<(), WechatError> {
        self.bot.stop().await;
        Ok(())
    }
}

fn map_ilink_error(err: WechatIlinkError) -> WechatError {
    let message = err.to_string();
    match err {
        WechatIlinkError::RateLimited { retry_after, .. } => WechatError::RateLimited {
            retry_after,
            message,
        },
        _ => WechatError::Transport(message),
    }
}

fn send_receipt(receipt: SendReceipt) -> WechatSendReceipt {
    WechatSendReceipt {
        message_ids: receipt.message_ids,
        visible_texts: receipt.visible_texts,
    }
}

fn show_login_qr(content: &str) {
    match render_terminal_qr(content) {
        Ok(qr) => {
            info!(target: "lucarne_wechat::adapter", "wechat login QR generated");
            eprintln!("\n[lucarne-wechat] WeChat login required.\n{qr}");
        }
        Err(err) => {
            warn!(
                target: "lucarne_wechat::adapter",
                error = %err,
                "failed to render wechat login QR"
            );
            eprintln!(
                "\n[lucarne-wechat] WeChat login required.\n[lucarne-wechat] QR render failed: {err}\n[lucarne-wechat] QR content:\n{content}\n"
            );
        }
    }
}

pub(crate) fn render_terminal_qr(content: &str) -> Result<String, qrcode::types::QrError> {
    let code = QrCode::with_error_correction_level(content.trim().as_bytes(), EcLevel::L)?;
    Ok(render_small_qr(code.width(), &code.to_colors()))
}

fn render_small_qr(module_count: usize, modules: &[Color]) -> String {
    let odd_row = module_count % 2 == 1;
    let output_rows = (module_count + 1) / 2;
    let mut output = String::new();

    output.push_str(&"▄".repeat(module_count + 2));
    output.push('\n');

    for row in 0..output_rows {
        output.push('█');
        for col in 0..module_count {
            let top = color_at(modules, module_count, row * 2, col);
            let bottom = if row * 2 + 1 < module_count {
                color_at(modules, module_count, row * 2 + 1, col)
            } else {
                Color::Light
            };
            output.push(match (top, bottom) {
                (Color::Light, Color::Light) => '█',
                (Color::Light, Color::Dark) => '▀',
                (Color::Dark, Color::Light) => '▄',
                (Color::Dark, Color::Dark) => ' ',
            });
        }
        output.push('█');
        output.push('\n');
    }

    if !odd_row {
        output.push_str(&"▀".repeat(module_count + 2));
        output.push('\n');
    }

    output
}

fn color_at(modules: &[Color], width: usize, row: usize, col: usize) -> Color {
    modules[row * width + col]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wechat_adapter_passes_shared_http_client_to_ilink_transport() {
        let source = include_str!("adapter.rs")
            .split("\n#[cfg(test)]")
            .next()
            .expect("production source");
        assert!(source.contains("WechatIlinkTransport::new_with_client"));
        assert!(source.contains("ctx.http_client.clone()"));
        assert!(source.contains("pub async fn new_with_client"));
        assert!(source.contains("http_client: reqwest::Client"));
        assert!(source.contains(".http_client(http_client)"));
    }

    #[test]
    fn memory_profile_snapshots_mark_wechat_spawn_phases() {
        let source = include_str!("adapter.rs");

        for label in [
            "lucarne_wechat.adapter.spawn.start",
            "lucarne_wechat.adapter.spawn.before_transport_new",
            "lucarne_wechat.adapter.spawn.after_transport_new",
            "lucarne_wechat.adapter.spawn.after_login",
            "lucarne_wechat.adapter.spawn.after_known_user_ids",
            "lucarne_wechat.adapter.spawn.before_task_spawn",
        ] {
            let needle = format!("lucarne::memory_profile_snapshot!(\"{label}\")");
            assert!(source.contains(&needle), "missing snapshot {label}");
        }
    }

    #[test]
    fn terminal_qr_renders_small_unicode_qr() {
        let rendered = render_terminal_qr("hello").expect("render qr");
        assert!(rendered.contains('█'));
        assert!(rendered.contains('▀') || rendered.contains('▄'));
        assert!(rendered.lines().count() > 2);
    }

    #[test]
    fn terminal_qr_uses_qrcode_terminal_small_border_shape() {
        let rendered = render_terminal_qr("hello").expect("render qr");
        let lines = rendered.lines().collect::<Vec<_>>();
        assert_eq!(
            lines.first().map(|line| line.chars().next()),
            Some(Some('▄'))
        );
        assert!(lines.iter().any(|line| {
            line.chars().next() == Some('█') && line.chars().last() == Some('█')
        }));
    }

    #[test]
    fn startup_notification_users_include_restored_context_users() {
        let users = startup_notification_user_ids(
            vec!["configured-user".to_string()],
            vec!["restored-user".to_string(), "configured-user".to_string()],
        );

        assert_eq!(
            users,
            vec!["configured-user".to_string(), "restored-user".to_string()]
        );
    }

    #[test]
    fn config_defaults_wechat_credentials_under_lucarned_home() {
        let config = wechat_config_from_adapter_config(&AdapterConfig::default());

        assert!(wechat_credential_path(&config).ends_with(".lucarned/wechat-credentials.json"));
    }

    #[test]
    fn config_defaults_rate_limit_interaction_prompt() {
        let config = wechat_config_from_adapter_config(&AdapterConfig::default());

        assert_eq!(
            config.rate_limit_interaction_prompt.as_deref(),
            Some(DEFAULT_RATE_LIMIT_INTERACTION_PROMPT)
        );
    }

    #[test]
    fn config_defaults_context_expiry_reminder() {
        let config = wechat_config_from_adapter_config(&AdapterConfig::default());
        let reminder = config
            .context_expiry_reminder
            .expect("context expiry reminder should default on");

        assert_eq!(reminder.expires_after, Duration::from_secs(7200));
        assert_eq!(reminder.remind_before, Duration::from_secs(300));
        assert_eq!(
            reminder.prompt_template,
            DEFAULT_CONTEXT_EXPIRY_REMINDER_TEMPLATE
        );
    }

    #[test]
    fn adapter_auto_enables_when_credentials_file_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cred_path = dir.path().join("wechat-credentials.json");
        std::fs::write(&cred_path, "{}").expect("write credentials marker");
        let config = AdapterConfig::from_env([(
            "LUCARNE_WECHAT_CRED_PATH",
            cred_path.to_str().expect("credential path"),
        )]);

        assert!(WechatAdapterPlugin.enabled(&config));
    }

    #[test]
    fn adapter_explicit_disable_overrides_credentials_auto_enable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cred_path = dir.path().join("wechat-credentials.json");
        std::fs::write(&cred_path, "{}").expect("write credentials marker");
        let config = AdapterConfig::from_env([
            ("LUCARNE_WECHAT_ENABLED", "off"),
            (
                "LUCARNE_WECHAT_CRED_PATH",
                cred_path.to_str().expect("credential path"),
            ),
        ]);

        assert!(!WechatAdapterPlugin.enabled(&config));
    }

    #[test]
    fn config_parses_protocol_builder_options() {
        let config = AdapterConfig::from_env([
            ("LUCARNE_WECHAT_ILINK_APP_ID", "custom-app"),
            ("LUCARNE_WECHAT_MARKDOWN_FILTER", "false"),
            (
                "LUCARNE_WECHAT_RATE_LIMIT_INTERACTION_PROMPT",
                "请回复任意消息刷新微信窗口。",
            ),
        ]);

        let config = wechat_config_from_adapter_config(&config);
        assert_eq!(config.ilink_app_id.as_deref(), Some("custom-app"));
        assert!(!config.markdown_filter);
        assert_eq!(
            config.rate_limit_interaction_prompt.as_deref(),
            Some("请回复任意消息刷新微信窗口。")
        );
    }

    #[test]
    fn config_parses_context_and_rate_limit_options_from_yaml() {
        let path = std::env::temp_dir().join(format!(
            "lucarne-wechat-runtime-config-{}.yaml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"
channels:
  wechat:
    context:
      ttl_secs: 3600
      expiry_remind_before_secs: 120
      expiry_reminder_template: "还有 {remaining_secs} 秒"
    rate_limit:
      interaction_prompt: "请回复任意消息"
"#,
        )
        .expect("write config");
        let config = AdapterConfig::from_env_and_file(Vec::<(String, String)>::new(), Some(&path))
            .expect("load config");

        let config = wechat_config_from_adapter_config(&config);
        let reminder = config
            .context_expiry_reminder
            .expect("context expiry reminder");
        assert_eq!(reminder.expires_after, Duration::from_secs(3600));
        assert_eq!(reminder.remind_before, Duration::from_secs(120));
        assert_eq!(reminder.prompt_template, "还有 {remaining_secs} 秒");
        assert_eq!(
            config.rate_limit_interaction_prompt.as_deref(),
            Some("请回复任意消息")
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn config_parses_context_expiry_reminder_options() {
        let config = AdapterConfig::from_env([
            ("LUCARNE_WECHAT_CONTEXT_TTL_SECS", "7200"),
            ("LUCARNE_WECHAT_CONTEXT_EXPIRY_REMIND_BEFORE_SECS", "300"),
            (
                "LUCARNE_WECHAT_CONTEXT_EXPIRY_REMINDER_TEMPLATE",
                "还有 {remaining_secs} 秒，{user_id} 回复确认保持会话可用",
            ),
        ]);

        let config = wechat_config_from_adapter_config(&config);
        let reminder = config
            .context_expiry_reminder
            .expect("context expiry reminder");
        assert_eq!(reminder.expires_after, Duration::from_secs(7200));
        assert_eq!(reminder.remind_before, Duration::from_secs(300));
        assert_eq!(
            reminder.prompt_template,
            "还有 {remaining_secs} 秒，{user_id} 回复确认保持会话可用"
        );
    }

    #[test]
    fn config_accepts_partial_context_expiry_reminder_overrides() {
        let ttl_only = AdapterConfig::from_env([("LUCARNE_WECHAT_CONTEXT_TTL_SECS", "3600")]);
        let before_only =
            AdapterConfig::from_env([("LUCARNE_WECHAT_CONTEXT_EXPIRY_REMIND_BEFORE_SECS", "60")]);

        let ttl_only = wechat_config_from_adapter_config(&ttl_only)
            .context_expiry_reminder
            .expect("ttl override should keep default reminder");
        assert_eq!(ttl_only.expires_after, Duration::from_secs(3600));
        assert_eq!(ttl_only.remind_before, Duration::from_secs(300));

        let before_only = wechat_config_from_adapter_config(&before_only)
            .context_expiry_reminder
            .expect("window override should keep default reminder");
        assert_eq!(before_only.expires_after, Duration::from_secs(7200));
        assert_eq!(before_only.remind_before, Duration::from_secs(60));
    }
}
