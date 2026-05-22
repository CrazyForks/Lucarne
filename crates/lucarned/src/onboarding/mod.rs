pub(crate) mod config;
pub(crate) mod session;
pub(crate) mod terminal;

use std::path::PathBuf;

use async_trait::async_trait;

use self::{
    config::{write_config_with_backup, ExistingConfigDefaults, WechatDraft},
    session::{OnboardingBackend, TelegramBotInfo, TelegramChatCandidate, WechatCredentialResult},
    terminal::{OnboardingTerminal, StdioTerminal},
};

pub(crate) async fn run_interactive_init() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    let mut terminal = StdioTerminal::new();
    ensure_interactive(terminal.is_interactive())?;

    let config_path = resolve_init_config_path()
        .ok_or("could not resolve lucarned config path; set LUCARNED_CONFIG")?;
    let config_path_label = config_path.display().to_string();

    let defaults = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|raw| ExistingConfigDefaults::from_yaml(&raw).ok())
        .unwrap_or_default();

    let http_client = lucarne_adapter::default_http_client()?;
    let backend = ProductionOnboardingBackend::new(http_client.clone());
    let available_agent_ids = available_agent_ids();

    let Some(mut outcome) = session::run_onboarding_session_with_defaults(
        &mut terminal,
        &backend,
        &config_path_label,
        &available_agent_ids,
        &defaults,
    )
    .await?
    else {
        return Ok(());
    };

    if let WechatDraft::Enabled {
        credential_path,
        reused_existing_credentials,
    } = &mut outcome.draft.wechat
    {
        let result = backend
            .ensure_wechat_credentials(credential_path, *reused_existing_credentials)
            .await?;
        *reused_existing_credentials = result.reused_existing_credentials;
    }

    let yaml = outcome.draft.render_yaml()?;
    write_config_with_backup(&config_path, &yaml)?;
    terminal.println("Configuration written.")?;
    terminal.println("Next steps:")?;
    terminal.println("  lucarned")?;
    terminal.println("  tail -f ~/.lucarned/logs/lucarned.log")?;

    Ok(())
}

pub(crate) fn resolve_init_config_path() -> Option<PathBuf> {
    crate::explicit_config_path().or_else(|| {
        lucarne::default_lucarned_home_dir().map(|home| crate::default_config_path_in(&home))
    })
}

pub(crate) fn ensure_interactive(interactive: bool) -> Result<(), Box<dyn std::error::Error>> {
    if interactive {
        Ok(())
    } else {
        Err("run `lucarned init` in an interactive terminal".into())
    }
}

pub(crate) fn available_agent_ids() -> Vec<String> {
    lucarne::adapters::default_adapter_provider_ids()
        .into_iter()
        .map(ToString::to_string)
        .collect()
}

struct ProductionOnboardingBackend {
    telegram: lucarne_telegram::onboarding::TelegramOnboardingClient,
    http_client: reqwest::Client,
}

impl ProductionOnboardingBackend {
    fn new(http_client: reqwest::Client) -> Self {
        Self {
            telegram: lucarne_telegram::onboarding::TelegramOnboardingClient::new(
                http_client.clone(),
            ),
            http_client,
        }
    }
}

#[async_trait]
impl OnboardingBackend for ProductionOnboardingBackend {
    async fn validate_telegram_token(
        &self,
        token: &str,
    ) -> Result<TelegramBotInfo, Box<dyn std::error::Error>> {
        let telegram = self.telegram.clone();
        let token = token.to_string();
        let bot = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| err.to_string())?;
            runtime
                .block_on(telegram.validate_token(&token))
                .map_err(|err| err.to_string())
        })
        .await??;

        Ok(TelegramBotInfo {
            username: bot.username,
        })
    }

    async fn discover_telegram_chats(
        &self,
        token: &str,
    ) -> Result<Vec<TelegramChatCandidate>, Box<dyn std::error::Error>> {
        let telegram = self.telegram.clone();
        let token = token.to_string();
        let chats = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| err.to_string())?;
            runtime
                .block_on(telegram.discover_chats(&token))
                .map_err(|err| err.to_string())
        })
        .await??;

        Ok(chats
            .into_iter()
            .map(|chat| TelegramChatCandidate {
                id: chat.id,
                label: chat.label,
            })
            .collect())
    }

    async fn ensure_wechat_credentials(
        &self,
        credential_path: &str,
        reuse_existing: bool,
    ) -> Result<WechatCredentialResult, Box<dyn std::error::Error>> {
        let result = lucarne_wechat::onboarding::ensure_wechat_onboarding_credentials(
            crate::expand_home_path(credential_path),
            reuse_existing,
            self.http_client.clone(),
        )
        .await
        .map_err(|err| -> Box<dyn std::error::Error> { err.to_string().into() })?;

        Ok(WechatCredentialResult {
            reused_existing_credentials: result.reused_existing_credentials,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvGuard {
        home: Option<std::ffi::OsString>,
        #[cfg(windows)]
        local_app_data: Option<std::ffi::OsString>,
        lucarne_config: Option<std::ffi::OsString>,
        lucarned_config: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set_home_without_config(home: &std::path::Path) -> Self {
            let guard = Self {
                home: std::env::var_os("HOME"),
                #[cfg(windows)]
                local_app_data: std::env::var_os("LOCALAPPDATA"),
                lucarne_config: std::env::var_os("LUCARNE_CONFIG"),
                lucarned_config: std::env::var_os("LUCARNED_CONFIG"),
            };
            std::env::set_var("HOME", home);
            #[cfg(windows)]
            std::env::set_var("LOCALAPPDATA", home);
            std::env::remove_var("LUCARNE_CONFIG");
            std::env::remove_var("LUCARNED_CONFIG");
            guard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            restore_env_var("HOME", self.home.take());
            #[cfg(windows)]
            restore_env_var("LOCALAPPDATA", self.local_app_data.take());
            restore_env_var("LUCARNE_CONFIG", self.lucarne_config.take());
            restore_env_var("LUCARNED_CONFIG", self.lucarned_config.take());
        }
    }

    fn restore_env_var(key: &str, value: Option<std::ffi::OsString>) {
        if let Some(value) = value {
            std::env::set_var(key, value);
        } else {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn init_requires_interactive_terminal() {
        let err = ensure_interactive(false).expect_err("non-tty fails");
        assert!(err
            .to_string()
            .contains("run `lucarned init` in an interactive terminal"));
    }

    #[test]
    fn init_config_path_resolution_does_not_create_default_file() {
        let _lock = env_lock().lock().expect("env lock");
        let temp = tempfile::tempdir().expect("temp home");
        let _env_guard = EnvGuard::set_home_without_config(temp.path());
        #[cfg(windows)]
        let expected_home = temp.path().join("lucarned");
        #[cfg(not(windows))]
        let expected_home = temp.path().join(".lucarned");
        let expected_path = expected_home.join("lucarned.yaml");

        let config_path = resolve_init_config_path().expect("resolve default init config path");

        assert_eq!(config_path, expected_path);
        assert!(
            !expected_path.exists(),
            "resolver must not create config file"
        );
        assert!(
            !expected_home.exists(),
            "resolver must not create config dir"
        );
    }

    #[test]
    fn available_agent_ids_come_from_lucarne_descriptors() {
        let ids = available_agent_ids();
        assert!(ids.contains(&"codex".to_string()));
        assert!(ids.contains(&"pi".to_string()));
    }
}
