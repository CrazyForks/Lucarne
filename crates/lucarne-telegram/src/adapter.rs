use std::sync::Arc;

use async_trait::async_trait;
use lucarne::LucarneCore;
use lucarne_adapter::{
    AdapterConfig, AdapterContext, AdapterError, AdapterPlugin, AdapterResult, AdapterTask,
};
use tracing::{debug, info};

use crate::{
    bot::{telegram_menu_commands, Bot},
    channel::{TelegramChannel, TelegramConfig},
};

pub struct TelegramAdapterPlugin;

pub fn telegram_plugin() -> TelegramAdapterPlugin {
    TelegramAdapterPlugin
}

#[async_trait]
impl AdapterPlugin for TelegramAdapterPlugin {
    fn id(&self) -> &'static str {
        "telegram"
    }

    fn name(&self) -> &'static str {
        "Telegram"
    }

    fn enabled(&self, config: &AdapterConfig) -> bool {
        let configured = config.channel_enabled(self.id());
        let token_present = config.get("TELEGRAM_BOT_TOKEN").is_some();
        let enabled = configured.unwrap_or(false);
        debug!(
            target: "lucarne_telegram::adapter",
            enabled,
            configured,
            token_present,
            "telegram adapter enablement checked"
        );
        enabled
    }

    async fn spawn(&self, ctx: AdapterContext) -> AdapterResult<AdapterTask> {
        lucarne::memory_profile_snapshot!("lucarne_telegram.adapter.spawn.start");
        let config = telegram_config_from_adapter_config(&*ctx.config)?;
        let core = Arc::clone(&ctx.core);
        let http_client = ctx.http_client.clone();
        let global_config_persistence = ctx.global_config_persistence.clone();
        info!(
            target: "lucarne_telegram::adapter",
            entry_chat_id = config.entry_chat_id,
            authorized_user_count = config.authorized_user_ids.len(),
            "telegram adapter spawning"
        );
        lucarne::memory_profile_snapshot!("lucarne_telegram.adapter.spawn.before_task_spawn");
        Ok(AdapterTask::spawn(self.id(), async move {
            run_telegram_adapter_with_client_and_global_config_persistence(
                core,
                config,
                http_client,
                global_config_persistence,
            )
            .await
            .map_err(|err| AdapterError::message(err.to_string()))
        }))
    }
}

fn telegram_config_from_adapter_config(config: &AdapterConfig) -> AdapterResult<TelegramConfig> {
    let token = config
        .channel_value("TELEGRAM_BOT_TOKEN", "telegram", "token")
        .ok_or_else(|| AdapterError::permanent("TELEGRAM_BOT_TOKEN is required"))?
        .to_string();
    let entry_chat_id = config
        .channel_value("TELEGRAM_CHAT_ID", "telegram", "entry_chat_id")
        .or_else(|| config.get("LUCARNE_ENTRY_CHAT_ID"))
        .ok_or_else(|| {
            AdapterError::permanent("TELEGRAM_CHAT_ID or LUCARNE_ENTRY_CHAT_ID is required")
        })?
        .parse::<i64>()
        .map_err(|err| AdapterError::permanent(format!("invalid Telegram chat id: {err}")))?;
    let authorized_user_ids = config
        .get("LUCARNE_AUTHORIZED_USER_IDS")
        .unwrap_or("")
        .split(',')
        .filter_map(|text| text.trim().parse::<i64>().ok())
        .collect();
    Ok(TelegramConfig {
        token,
        entry_chat_id,
        authorized_user_ids,
    })
}

pub async fn run_telegram_adapter(
    core: Arc<LucarneCore>,
    config: TelegramConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let http_client = lucarne_adapter::default_http_client()?;
    run_telegram_adapter_with_client(core, config, http_client).await
}

pub async fn run_telegram_adapter_with_client(
    core: Arc<LucarneCore>,
    config: TelegramConfig,
    http_client: reqwest::Client,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    run_telegram_adapter_with_client_and_global_config_persistence(core, config, http_client, None)
        .await
}

pub async fn run_telegram_adapter_with_client_and_global_config_persistence(
    core: Arc<LucarneCore>,
    config: TelegramConfig,
    http_client: reqwest::Client,
    global_config_persistence: Option<Arc<dyn lucarne_adapter::GlobalConfigPersistence>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    lucarne::memory_profile_snapshot!("lucarne_telegram.adapter.run.start");
    info!(
        target: "lucarne_telegram::adapter",
        entry_chat_id = config.entry_chat_id,
        "telegram adapter starting"
    );
    let channel = TelegramChannel::start_with_client(config, http_client);
    lucarne::memory_profile_snapshot!("lucarne_telegram.adapter.run.after_channel_start");
    channel.sync_commands(telegram_menu_commands()).await?;
    lucarne::memory_profile_snapshot!("lucarne_telegram.adapter.run.after_sync_commands");
    let entry = channel.entry_handle();
    let state = crate::state::BotState::new_with_core(Arc::clone(&core));
    lucarne::memory_profile_snapshot!("lucarne_telegram.adapter.run.after_state_new");
    let bot = Arc::new(Bot::new_with_state_and_global_config_persistence(
        channel.clone(),
        core,
        entry,
        state,
        global_config_persistence,
    ));
    lucarne::memory_profile_snapshot!("lucarne_telegram.adapter.run.after_bot_new");
    info!(target: "lucarne_telegram::adapter", "telegram adapter started");
    lucarne::memory_profile_snapshot!("lucarne_telegram.adapter.run.before_bot_run");
    bot.run().await;
    info!(target: "lucarne_telegram::adapter", "telegram adapter stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_adapter_disabled_by_default_even_with_token() {
        let config = lucarne_adapter::AdapterConfig::from_env([
            ("TELEGRAM_BOT_TOKEN", "000000:test"),
            ("TELEGRAM_CHAT_ID", "1"),
        ]);

        assert!(!telegram_plugin().enabled(&config));
    }

    #[test]
    fn telegram_adapter_enabled_when_explicitly_configured() {
        let config = lucarne_adapter::AdapterConfig::from_env([
            ("LUCARNE_TELEGRAM_ENABLED", "true"),
            ("TELEGRAM_BOT_TOKEN", "000000:test"),
            ("TELEGRAM_CHAT_ID", "1"),
        ]);

        assert!(telegram_plugin().enabled(&config));
    }

    #[test]
    fn telegram_authorized_users_remain_env_only() {
        let path = std::env::temp_dir().join(format!(
            "lucarne-telegram-authorized-users-{}.yaml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"
channels:
  telegram:
    token: test-token
    entry_chat_id: 1
    authorized_user_ids: [111, 222]
"#,
        )
        .expect("write config");
        let config = AdapterConfig::from_env_and_file(Vec::<(String, String)>::new(), Some(&path))
            .expect("load config");

        let telegram = telegram_config_from_adapter_config(&config).expect("telegram config");

        assert!(telegram.authorized_user_ids.is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn telegram_adapter_does_not_load_manual_agents_config() {
        let source = include_str!("adapter.rs")
            .split("\n#[cfg(test)]")
            .next()
            .expect("production source");

        assert!(!source.contains("LUCARNE_TELEGRAM_CONFIG"));
        assert!(!source.contains("config_path"));
    }

    #[test]
    fn telegram_adapter_passes_shared_http_client_to_channel() {
        let source = include_str!("adapter.rs")
            .split("\n#[cfg(test)]")
            .next()
            .expect("production source");
        assert!(
            source.contains("let http_client = ctx.http_client.clone();"),
            "adapter spawn should clone the shared reqwest client from AdapterContext"
        );
        assert!(source.contains("run_telegram_adapter_with_client_and_global_config_persistence"));
        assert!(source.contains("TelegramChannel::start_with_client(config, http_client)"));
    }

    #[test]
    fn memory_profile_snapshots_mark_telegram_async_startup() {
        let source = include_str!("adapter.rs");

        for label in [
            "lucarne_telegram.adapter.spawn.start",
            "lucarne_telegram.adapter.spawn.before_task_spawn",
            "lucarne_telegram.adapter.run.start",
            "lucarne_telegram.adapter.run.after_channel_start",
            "lucarne_telegram.adapter.run.after_sync_commands",
            "lucarne_telegram.adapter.run.after_state_new",
            "lucarne_telegram.adapter.run.after_bot_new",
            "lucarne_telegram.adapter.run.before_bot_run",
        ] {
            let needle = format!("lucarne::memory_profile_snapshot!(\"{label}\")");
            assert!(source.contains(&needle), "missing snapshot {label}");
        }
    }
}
