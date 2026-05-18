use async_trait::async_trait;

use crate::onboarding::{
    config::{AgentSelection, ExistingConfigDefaults, InitConfigDraft, TelegramDraft, WechatDraft},
    terminal::OnboardingTerminal,
};

const DEFAULT_WECHAT_CREDENTIAL_PATH: &str = "~/.lucarned/wechat-credentials.json";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TelegramBotInfo {
    pub(crate) username: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TelegramChatCandidate {
    pub(crate) id: i64,
    pub(crate) label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WechatCredentialResult {
    pub(crate) reused_existing_credentials: bool,
}

impl Clone for AgentSelection {
    fn clone(&self) -> Self {
        match self {
            Self::All => Self::All,
            Self::None => Self::None,
            Self::Selected(agents) => Self::Selected(agents.clone()),
        }
    }
}

impl std::fmt::Debug for AgentSelection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::All => formatter.write_str("All"),
            Self::None => formatter.write_str("None"),
            Self::Selected(agents) => formatter.debug_tuple("Selected").field(agents).finish(),
        }
    }
}

impl PartialEq for AgentSelection {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::All, Self::All) | (Self::None, Self::None) => true,
            (Self::Selected(left), Self::Selected(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for AgentSelection {}

impl Clone for TelegramDraft {
    fn clone(&self) -> Self {
        match self {
            Self::Disabled => Self::Disabled,
            Self::Enabled {
                token,
                entry_chat_id,
                bot_username,
            } => Self::Enabled {
                token: token.clone(),
                entry_chat_id: *entry_chat_id,
                bot_username: bot_username.clone(),
            },
        }
    }
}

impl std::fmt::Debug for TelegramDraft {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => formatter.write_str("Disabled"),
            Self::Enabled {
                entry_chat_id,
                bot_username,
                ..
            } => formatter
                .debug_struct("Enabled")
                .field("token", &"<redacted>")
                .field("entry_chat_id", entry_chat_id)
                .field("bot_username", bot_username)
                .finish(),
        }
    }
}

impl PartialEq for TelegramDraft {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Disabled, Self::Disabled) => true,
            (
                Self::Enabled {
                    token: left_token,
                    entry_chat_id: left_chat,
                    bot_username: left_bot,
                },
                Self::Enabled {
                    token: right_token,
                    entry_chat_id: right_chat,
                    bot_username: right_bot,
                },
            ) => left_token == right_token && left_chat == right_chat && left_bot == right_bot,
            _ => false,
        }
    }
}

impl Eq for TelegramDraft {}

impl Clone for WechatDraft {
    fn clone(&self) -> Self {
        match self {
            Self::Disabled { credential_path } => Self::Disabled {
                credential_path: credential_path.clone(),
            },
            Self::Enabled {
                credential_path,
                reused_existing_credentials,
            } => Self::Enabled {
                credential_path: credential_path.clone(),
                reused_existing_credentials: *reused_existing_credentials,
            },
        }
    }
}

impl std::fmt::Debug for WechatDraft {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled { credential_path } => formatter
                .debug_struct("Disabled")
                .field("credential_path", credential_path)
                .finish(),
            Self::Enabled {
                credential_path,
                reused_existing_credentials,
            } => formatter
                .debug_struct("Enabled")
                .field("credential_path", credential_path)
                .field("reused_existing_credentials", reused_existing_credentials)
                .finish(),
        }
    }
}

impl PartialEq for WechatDraft {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Disabled {
                    credential_path: left,
                },
                Self::Disabled {
                    credential_path: right,
                },
            ) => left == right,
            (
                Self::Enabled {
                    credential_path: left_path,
                    reused_existing_credentials: left_reused,
                },
                Self::Enabled {
                    credential_path: right_path,
                    reused_existing_credentials: right_reused,
                },
            ) => left_path == right_path && left_reused == right_reused,
            _ => false,
        }
    }
}

impl Eq for WechatDraft {}

impl Clone for InitConfigDraft {
    fn clone(&self) -> Self {
        Self {
            agents: self.agents.clone(),
            telegram: self.telegram.clone(),
            wechat: self.wechat.clone(),
        }
    }
}

impl std::fmt::Debug for InitConfigDraft {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InitConfigDraft")
            .field("agents", &self.agents)
            .field("telegram", &self.telegram)
            .field("wechat", &self.wechat)
            .finish()
    }
}

impl PartialEq for InitConfigDraft {
    fn eq(&self, other: &Self) -> bool {
        self.agents == other.agents
            && self.telegram == other.telegram
            && self.wechat == other.wechat
    }
}

impl Eq for InitConfigDraft {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OnboardingOutcome {
    pub(crate) draft: InitConfigDraft,
    pub(crate) summary: String,
}

#[async_trait]
pub(crate) trait OnboardingBackend {
    async fn validate_telegram_token(
        &self,
        token: &str,
    ) -> Result<TelegramBotInfo, Box<dyn std::error::Error>>;

    async fn discover_telegram_chats(
        &self,
        token: &str,
    ) -> Result<Vec<TelegramChatCandidate>, Box<dyn std::error::Error>>;

    async fn ensure_wechat_credentials(
        &self,
        credential_path: &str,
        reuse_existing: bool,
    ) -> Result<WechatCredentialResult, Box<dyn std::error::Error>>;
}

#[cfg(test)]
pub(crate) async fn run_onboarding_session<T, B>(
    terminal: &mut T,
    backend: &B,
    config_path: &str,
    available_agent_ids: &[String],
) -> Result<Option<OnboardingOutcome>, Box<dyn std::error::Error>>
where
    T: OnboardingTerminal + Send,
    B: OnboardingBackend + Sync,
{
    run_onboarding_session_with_defaults(
        terminal,
        backend,
        config_path,
        available_agent_ids,
        &ExistingConfigDefaults::default(),
    )
    .await
}

pub(crate) async fn run_onboarding_session_with_defaults<T, B>(
    terminal: &mut T,
    backend: &B,
    config_path: &str,
    available_agent_ids: &[String],
    defaults: &ExistingConfigDefaults,
) -> Result<Option<OnboardingOutcome>, Box<dyn std::error::Error>>
where
    T: OnboardingTerminal + Send,
    B: OnboardingBackend + Sync,
{
    terminal.println("Welcome to lucarned init.")?;
    terminal.println(&format!("Config path: {config_path}"))?;
    terminal.println("Nothing will be written until final confirmation.")?;

    let agents = prompt_agents(terminal, available_agent_ids, &defaults.agents)?;
    let telegram = prompt_telegram(terminal, backend, defaults).await?;
    let wechat = prompt_wechat(terminal, defaults)?;

    let draft = InitConfigDraft {
        agents,
        telegram,
        wechat,
    };
    let summary = draft.redacted_summary(config_path);
    terminal.println("Configuration summary:")?;
    terminal.println(&summary)?;

    if !terminal.confirm("Write configuration?", true)? {
        terminal.println("No changes written.")?;
        return Ok(None);
    }

    Ok(Some(OnboardingOutcome { draft, summary }))
}

fn prompt_agents<T>(
    terminal: &mut T,
    available_agent_ids: &[String],
    defaults: &AgentSelection,
) -> Result<AgentSelection, Box<dyn std::error::Error>>
where
    T: OnboardingTerminal + Send,
{
    if available_agent_ids.is_empty() {
        terminal.println("Available agents: none")?;
    } else {
        terminal.println(&format!(
            "Available agents: {}",
            available_agent_ids.join(", ")
        ))?;
    }

    let default_mode = match defaults {
        AgentSelection::All => "a",
        AgentSelection::None => "n",
        AgentSelection::Selected(_) => "s",
    };

    loop {
        let mode = terminal.prompt("Agents ([a]ll, [n]one, [s]elect)", Some(default_mode))?;
        match mode.trim().to_ascii_lowercase().as_str() {
            "a" | "all" => return Ok(AgentSelection::All),
            "n" | "none" => return Ok(AgentSelection::None),
            "s" | "select" => {
                return prompt_selected_agents(terminal, available_agent_ids, defaults);
            }
            _ => terminal.println("Please choose a, n, or s.")?,
        }
    }
}

fn prompt_selected_agents<T>(
    terminal: &mut T,
    available_agent_ids: &[String],
    defaults: &AgentSelection,
) -> Result<AgentSelection, Box<dyn std::error::Error>>
where
    T: OnboardingTerminal + Send,
{
    let default_selected = match defaults {
        AgentSelection::Selected(agents) => Some(agents.join(",")),
        _ => None,
    };
    let selected = terminal.prompt(
        "Selected agent IDs (comma-separated)",
        default_selected.as_deref(),
    )?;
    let mut known = Vec::new();

    for id in selected
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        if available_agent_ids.iter().any(|available| available == id) {
            if !known.iter().any(|known_id| known_id == id) {
                known.push(id.to_string());
            }
        } else {
            terminal.println(&format!("Unknown agent skipped: {id}"))?;
        }
    }

    Ok(AgentSelection::Selected(known))
}

async fn prompt_telegram<T, B>(
    terminal: &mut T,
    backend: &B,
    defaults: &ExistingConfigDefaults,
) -> Result<TelegramDraft, Box<dyn std::error::Error>>
where
    T: OnboardingTerminal + Send,
    B: OnboardingBackend + Sync,
{
    if !terminal.confirm(
        "Enable Telegram?",
        defaults.telegram_enabled.unwrap_or(false),
    )? {
        return Ok(TelegramDraft::Disabled);
    }

    let token_answer = if defaults.telegram_token.is_some() {
        terminal.prompt("Telegram bot token (leave blank to keep existing)", None)?
    } else {
        terminal.prompt("Telegram bot token", None)?
    };
    let token_answer = token_answer.trim();
    let reused_existing_token = token_answer.is_empty() && defaults.telegram_token.is_some();
    let token = if reused_existing_token {
        defaults.telegram_token.clone().unwrap_or_default()
    } else {
        token_answer.to_string()
    };
    if token.is_empty() {
        return Err("Telegram bot token cannot be empty".into());
    }

    if reused_existing_token {
        if let Some(entry_chat_id) = defaults.telegram_entry_chat_id {
            return Ok(TelegramDraft::Enabled {
                token,
                entry_chat_id,
                bot_username: None,
            });
        }
    }

    let bot_info = backend
        .validate_telegram_token(&token)
        .await
        .map_err(|_| "Telegram token validation failed; check the bot token")?;
    let bot_label = bot_info.username.as_deref().unwrap_or("unknown bot");
    terminal.println(&format!("Validated Telegram bot: {bot_label}"))?;
    terminal.println("Send a message to the bot, then press Enter.")?;
    let _ = terminal.prompt("Press Enter after sending Telegram message", Some(""))?;

    let chats = backend
        .discover_telegram_chats(&token)
        .await
        .map_err(|_| "Telegram chat discovery failed; send the bot a message and try again")?;
    if chats.is_empty() {
        if let Some(entry_chat_id) = defaults.telegram_entry_chat_id {
            terminal.println(&format!(
                "No Telegram chats discovered; using existing chat {entry_chat_id}."
            ))?;
            return Ok(TelegramDraft::Enabled {
                token,
                entry_chat_id,
                bot_username: bot_info.username,
            });
        }
        return Err("no Telegram chats discovered".into());
    }

    terminal.println("Discovered Telegram chats:")?;
    for (index, chat) in chats.iter().enumerate() {
        terminal.println(&format!("{}. {} ({})", index + 1, chat.label, chat.id))?;
    }

    let chat = loop {
        let answer = terminal.prompt("Telegram chat number", Some("1"))?;
        match answer.trim().parse::<usize>() {
            Ok(number) if (1..=chats.len()).contains(&number) => break &chats[number - 1],
            _ => terminal.println("Please enter a valid chat number.")?,
        }
    };

    Ok(TelegramDraft::Enabled {
        token,
        entry_chat_id: chat.id,
        bot_username: bot_info.username,
    })
}

fn prompt_wechat<T>(
    terminal: &mut T,
    defaults: &ExistingConfigDefaults,
) -> Result<WechatDraft, Box<dyn std::error::Error>>
where
    T: OnboardingTerminal + Send,
{
    let credential_path_default = defaults
        .wechat_credential_path
        .as_deref()
        .unwrap_or(DEFAULT_WECHAT_CREDENTIAL_PATH);

    if !terminal.confirm("Enable WeChat?", defaults.wechat_enabled.unwrap_or(false))? {
        return Ok(WechatDraft::Disabled {
            credential_path: credential_path_default.to_string(),
        });
    }

    let credential_path =
        terminal.prompt("WeChat credential path", Some(credential_path_default))?;
    let reuse_existing = terminal.confirm("Reuse existing WeChat credentials?", true)?;

    Ok(WechatDraft::Enabled {
        credential_path,
        reused_existing_credentials: reuse_existing,
    })
}

#[cfg(test)]
mod tests {
    use std::{error::Error, sync::Mutex};

    use async_trait::async_trait;

    use super::*;
    use crate::onboarding::{
        config::{AgentSelection, TelegramDraft, WechatDraft},
        terminal::ScriptedTerminal,
    };

    struct MockBackend {
        telegram_tokens: Mutex<Vec<String>>,
        telegram_discovers: Mutex<Vec<String>>,
        wechat_calls: Mutex<Vec<(String, bool)>>,
        telegram_chats: Vec<TelegramChatCandidate>,
        telegram_validation_error: Option<String>,
        telegram_discovery_error: Option<String>,
    }

    impl Default for MockBackend {
        fn default() -> Self {
            Self {
                telegram_tokens: Mutex::new(Vec::new()),
                telegram_discovers: Mutex::new(Vec::new()),
                wechat_calls: Mutex::new(Vec::new()),
                telegram_chats: vec![TelegramChatCandidate {
                    id: 42,
                    label: "Ops Chat".to_string(),
                }],
                telegram_validation_error: None,
                telegram_discovery_error: None,
            }
        }
    }

    #[async_trait]
    impl OnboardingBackend for MockBackend {
        async fn validate_telegram_token(
            &self,
            token: &str,
        ) -> Result<TelegramBotInfo, Box<dyn Error>> {
            self.telegram_tokens.lock().unwrap().push(token.to_string());
            if let Some(error) = &self.telegram_validation_error {
                return Err(error.clone().into());
            }
            Ok(TelegramBotInfo {
                username: Some("lucarne_bot".to_string()),
            })
        }

        async fn discover_telegram_chats(
            &self,
            token: &str,
        ) -> Result<Vec<TelegramChatCandidate>, Box<dyn Error>> {
            self.telegram_discovers
                .lock()
                .unwrap()
                .push(token.to_string());
            if let Some(error) = &self.telegram_discovery_error {
                return Err(error.clone().into());
            }
            Ok(self.telegram_chats.clone())
        }

        async fn ensure_wechat_credentials(
            &self,
            credential_path: &str,
            reuse_existing: bool,
        ) -> Result<WechatCredentialResult, Box<dyn Error>> {
            self.wechat_calls
                .lock()
                .unwrap()
                .push((credential_path.to_string(), reuse_existing));
            Ok(WechatCredentialResult {
                reused_existing_credentials: reuse_existing,
            })
        }
    }

    #[tokio::test]
    async fn disabled_channels_and_selected_agents_returns_selected_config() {
        let mut terminal = ScriptedTerminal::new(["s", "codex, unknown, pi", "n", "n", "y"]);
        let backend = MockBackend::default();
        let agents = vec!["codex".to_string(), "pi".to_string()];

        let outcome = run_onboarding_session(&mut terminal, &backend, "lucarned.yaml", &agents)
            .await
            .expect("session")
            .expect("outcome");

        match outcome.draft.agents {
            AgentSelection::Selected(selected) => {
                assert_eq!(selected, vec!["codex".to_string(), "pi".to_string()]);
            }
            _ => panic!("expected selected agents"),
        }
        assert!(matches!(outcome.draft.telegram, TelegramDraft::Disabled));
        assert!(matches!(
            outcome.draft.wechat,
            WechatDraft::Disabled { ref credential_path }
                if credential_path == "~/.lucarned/wechat-credentials.json"
        ));
        assert!(terminal.output().contains("Unknown agent skipped: unknown"));
    }

    #[tokio::test]
    async fn enabled_channels_call_backend_and_redact_token_in_summary() {
        let mut terminal =
            ScriptedTerminal::new(["a", "y", "secret-token", "", "1", "y", "", "y", "y"]);
        let backend = MockBackend::default();
        let agents = vec!["codex".to_string(), "pi".to_string()];

        let outcome = run_onboarding_session(&mut terminal, &backend, "lucarned.yaml", &agents)
            .await
            .expect("session")
            .expect("outcome");

        assert!(matches!(outcome.draft.agents, AgentSelection::All));
        assert!(matches!(
            outcome.draft.telegram,
            TelegramDraft::Enabled {
                entry_chat_id: 42,
                ..
            }
        ));
        assert!(matches!(
            outcome.draft.wechat,
            WechatDraft::Enabled {
                reused_existing_credentials: true,
                ..
            }
        ));
        assert_eq!(
            backend.telegram_tokens.lock().unwrap().as_slice(),
            ["secret-token"]
        );
        assert_eq!(
            backend.telegram_discovers.lock().unwrap().as_slice(),
            ["secret-token"]
        );
        assert!(backend.wechat_calls.lock().unwrap().is_empty());
        assert!(outcome.summary.contains("<redacted>"));
        assert!(!outcome.summary.contains("secret-token"));
        assert!(!terminal.output().contains("secret-token"));
    }

    #[tokio::test]
    async fn existing_config_defaults_are_used_as_prompt_defaults() {
        let mut terminal = ScriptedTerminal::new(["", "", "", "", "", "", "", "y"]);
        let backend = MockBackend::default();
        let agents = vec!["codex".to_string(), "pi".to_string()];
        let defaults = crate::onboarding::config::ExistingConfigDefaults {
            agents: AgentSelection::Selected(vec!["pi".to_string()]),
            telegram_enabled: Some(true),
            telegram_token: Some("existing-token".to_string()),
            telegram_entry_chat_id: Some(42),
            wechat_enabled: Some(true),
            wechat_credential_path: Some("~/.lucarned/existing-wechat.json".to_string()),
        };

        let outcome = run_onboarding_session_with_defaults(
            &mut terminal,
            &backend,
            "lucarned.yaml",
            &agents,
            &defaults,
        )
        .await
        .expect("session")
        .expect("outcome");

        assert!(matches!(
            outcome.draft.agents,
            AgentSelection::Selected(ref selected) if selected == &["pi".to_string()]
        ));
        assert!(matches!(
            outcome.draft.telegram,
            TelegramDraft::Enabled {
                ref token,
                entry_chat_id: 42,
                ..
            } if token == "existing-token"
        ));
        assert!(matches!(
            outcome.draft.wechat,
            WechatDraft::Enabled {
                ref credential_path,
                reused_existing_credentials: true,
            } if credential_path == "~/.lucarned/existing-wechat.json"
        ));
        assert!(!terminal.output().contains("existing-token"));
        assert!(backend.telegram_tokens.lock().unwrap().is_empty());
        assert!(backend.telegram_discovers.lock().unwrap().is_empty());
        assert!(backend.wechat_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn reuses_existing_telegram_token_and_chat_without_backend_calls() {
        let mut terminal = ScriptedTerminal::new(["n", "", "", "n", "y"]);
        let backend = MockBackend::default();
        let defaults = crate::onboarding::config::ExistingConfigDefaults {
            telegram_enabled: Some(true),
            telegram_token: Some("existing-token".to_string()),
            telegram_entry_chat_id: Some(42),
            ..Default::default()
        };

        let outcome = run_onboarding_session_with_defaults(
            &mut terminal,
            &backend,
            "lucarned.yaml",
            &[],
            &defaults,
        )
        .await
        .expect("session")
        .expect("outcome");

        assert!(matches!(
            outcome.draft.telegram,
            TelegramDraft::Enabled {
                ref token,
                entry_chat_id: 42,
                bot_username: None,
            } if token == "existing-token"
        ));
        assert!(!terminal.output().contains("existing-token"));
        assert!(backend.telegram_tokens.lock().unwrap().is_empty());
        assert!(backend.telegram_discovers.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn uses_existing_telegram_entry_chat_id_when_discovery_returns_no_chats() {
        let mut terminal = ScriptedTerminal::new(["n", "", "new-token", "", "n", "y"]);
        let backend = MockBackend {
            telegram_chats: Vec::new(),
            ..MockBackend::default()
        };
        let defaults = crate::onboarding::config::ExistingConfigDefaults {
            telegram_enabled: Some(true),
            telegram_token: Some("existing-token".to_string()),
            telegram_entry_chat_id: Some(42),
            ..Default::default()
        };

        let outcome = run_onboarding_session_with_defaults(
            &mut terminal,
            &backend,
            "lucarned.yaml",
            &[],
            &defaults,
        )
        .await
        .expect("session")
        .expect("outcome");

        assert!(matches!(
            outcome.draft.telegram,
            TelegramDraft::Enabled {
                ref token,
                entry_chat_id: 42,
                ..
            } if token == "new-token"
        ));
        assert_eq!(
            backend.telegram_tokens.lock().unwrap().as_slice(),
            ["new-token"]
        );
        assert_eq!(
            backend.telegram_discovers.lock().unwrap().as_slice(),
            ["new-token"]
        );
    }

    #[tokio::test]
    async fn final_decline_returns_none_and_prints_no_changes_written() {
        let mut terminal = ScriptedTerminal::new(["n", "n", "y", "", "y", "n"]);
        let backend = MockBackend::default();

        let outcome = run_onboarding_session(&mut terminal, &backend, "lucarned.yaml", &[])
            .await
            .expect("session");

        assert_eq!(outcome, None);
        assert!(backend.wechat_calls.lock().unwrap().is_empty());
        assert!(terminal.output().contains("No changes written."));
    }

    #[tokio::test]
    async fn rejects_empty_telegram_token_before_backend() {
        let mut terminal = ScriptedTerminal::new(["a", "y", "   "]);
        let backend = MockBackend::default();

        let error = run_onboarding_session(&mut terminal, &backend, "lucarned.yaml", &[])
            .await
            .expect_err("empty token should fail");

        assert!(error
            .to_string()
            .contains("Telegram bot token cannot be empty"));
        assert!(backend.telegram_tokens.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn telegram_validation_error_does_not_include_token_string() {
        let token = "12345:secret-token";
        let mut terminal = ScriptedTerminal::new(["a", "y", token]);
        let backend = MockBackend {
            telegram_validation_error: Some(format!(
                "GET https://api.telegram.org/bot{token}/getMe failed"
            )),
            ..MockBackend::default()
        };

        let error = run_onboarding_session(&mut terminal, &backend, "lucarned.yaml", &[])
            .await
            .expect_err("validation should fail");
        let message = error.to_string();

        assert!(message.contains("Telegram token validation failed"));
        assert!(!message.contains(token));
    }

    #[tokio::test]
    async fn telegram_discovery_error_does_not_include_token_string() {
        let token = "123456:secret";
        let mut terminal = ScriptedTerminal::new(["a", "y", token, ""]);
        let backend = MockBackend {
            telegram_discovery_error: Some(format!(
                "https://api.telegram.org/bot{token}/getUpdates failed"
            )),
            ..MockBackend::default()
        };

        let error = run_onboarding_session(&mut terminal, &backend, "lucarned.yaml", &[])
            .await
            .expect_err("discovery should fail");
        let message = error.to_string();

        assert!(message.contains("Telegram chat discovery failed"));
        assert!(!message.contains(token));
    }
}
