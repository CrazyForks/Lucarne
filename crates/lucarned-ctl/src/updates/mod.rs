mod github;
mod render;
mod state;
mod version;

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub use github::{fetch_latest_release, parse_release_json, GithubRelease};
pub use render::{
    render_doctor_message, render_update_cli, render_update_notification, truncate_release_body,
    INSTALL_HINT,
};
pub use state::{should_notify, UpdateState, UpdateStateStore};
pub use version::{is_newer_version, normalize_tag, parse_version};

#[derive(Debug)]
pub enum UpdateError {
    InvalidRepository(String),
    Http(reqwest::Error),
    HttpStatus(reqwest::StatusCode),
    ResponseTooLarge { limit: usize, actual: usize },
    Json(serde_json::Error),
    Version(semver::Error),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRepository(repository) => {
                write!(f, "invalid GitHub repository: {repository}")
            }
            Self::Http(err) => write!(f, "GitHub request failed: {err}"),
            Self::HttpStatus(status) => write!(f, "GitHub request returned HTTP {status}"),
            Self::ResponseTooLarge { limit, actual } => {
                write!(f, "GitHub response exceeded {limit} bytes: {actual}")
            }
            Self::Json(err) => write!(f, "GitHub response JSON was invalid: {err}"),
            Self::Version(err) => write!(f, "release version was invalid: {err}"),
        }
    }
}

impl std::error::Error for UpdateError {}

impl From<reqwest::Error> for UpdateError {
    fn from(err: reqwest::Error) -> Self {
        Self::Http(err)
    }
}

impl From<serde_json::Error> for UpdateError {
    fn from(err: serde_json::Error) -> Self {
        Self::Json(err)
    }
}

impl From<semver::Error> for UpdateError {
    fn from(err: semver::Error) -> Self {
        Self::Version(err)
    }
}

#[derive(Debug, Clone)]
pub struct UpdateConfig {
    pub enabled: bool,
    pub notify: bool,
    pub check_interval: Duration,
    pub remind_interval: Duration,
    pub repository: String,
    pub startup_delay: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateStatus {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub release_name: Option<String>,
    pub release_url: Option<String>,
    pub published_at: Option<String>,
    pub release_body: Option<String>,
    pub is_newer: bool,
    pub automatic_checks_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateNotice {
    pub current_version: String,
    pub latest_version: String,
    pub release_name: String,
    pub release_url: String,
    pub published_at: Option<String>,
    pub body_markdown: String,
    pub install_hint: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateRuntimeTick {
    pub notice: Option<UpdateNotice>,
    pub warnings: Vec<String>,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            notify: true,
            check_interval: Duration::from_secs(24 * 60 * 60),
            remind_interval: Duration::from_secs(24 * 60 * 60),
            repository: "tuchg/Lucarne".to_string(),
            startup_delay: Duration::from_secs(60),
        }
    }
}

pub async fn check_now(
    client: &reqwest::Client,
    config: &UpdateConfig,
    current_version: &str,
) -> Result<UpdateStatus, UpdateError> {
    if !config.enabled {
        return Ok(UpdateStatus {
            current_version: current_version.to_string(),
            latest_version: None,
            release_name: None,
            release_url: None,
            published_at: None,
            release_body: None,
            is_newer: false,
            automatic_checks_enabled: false,
        });
    }

    let user_agent = format!("lucarned/{current_version}");
    let Some(release) = fetch_latest_release(client, &config.repository, &user_agent).await? else {
        return Ok(UpdateStatus {
            current_version: current_version.to_string(),
            latest_version: None,
            release_name: None,
            release_url: None,
            published_at: None,
            release_body: None,
            is_newer: false,
            automatic_checks_enabled: true,
        });
    };

    let latest_version = normalize_tag(&release.tag_name).to_string();
    let is_newer = is_newer_version(current_version, &release.tag_name)?;
    let release_name = if release.name.trim().is_empty() {
        latest_version.clone()
    } else {
        release.name
    };

    Ok(UpdateStatus {
        current_version: current_version.to_string(),
        latest_version: Some(latest_version),
        release_name: Some(release_name),
        release_url: Some(release.html_url),
        published_at: release.published_at,
        release_body: Some(release.body),
        is_newer,
        automatic_checks_enabled: true,
    })
}

pub async fn run_update_command(
    client: &reqwest::Client,
    update_config: UpdateConfig,
) -> Result<(), String> {
    let automatic_checks_enabled = update_config.enabled;
    let mut manual_config = update_config;
    manual_config.enabled = true;
    let mut status = check_now(client, &manual_config, env!("CARGO_PKG_VERSION"))
        .await
        .map_err(|err| err.to_string())?;
    status.automatic_checks_enabled = automatic_checks_enabled;
    print!("{}", render_update_cli(&status));
    Ok(())
}

pub struct UpdateRuntime {
    config: UpdateConfig,
    state_store: UpdateStateStore,
    next_due: tokio::time::Instant,
}

impl UpdateRuntime {
    pub fn new(config: UpdateConfig, state_store: UpdateStateStore) -> Self {
        let next_due = tokio::time::Instant::now() + config.startup_delay;
        Self {
            config,
            state_store,
            next_due,
        }
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    pub async fn next_notice(
        &mut self,
        client: &reqwest::Client,
        current_version: &str,
    ) -> Option<UpdateNotice> {
        self.next_tick(client, current_version).await.notice
    }

    pub async fn next_tick(
        &mut self,
        client: &reqwest::Client,
        current_version: &str,
    ) -> UpdateRuntimeTick {
        if !self.enabled() {
            return UpdateRuntimeTick::default();
        }

        tokio::time::sleep_until(self.next_due).await;
        self.next_due = tokio::time::Instant::now() + self.config.check_interval;

        let status = match tokio::time::timeout(
            Duration::from_secs(10),
            check_now(client, &self.config, current_version),
        )
        .await
        {
            Ok(Ok(status)) => status,
            Ok(Err(err)) => {
                return UpdateRuntimeTick {
                    notice: None,
                    warnings: vec![format!("update check failed: {err}")],
                };
            }
            Err(_) => {
                return UpdateRuntimeTick {
                    notice: None,
                    warnings: vec!["update check failed: timed out after 10s".to_string()],
                };
            }
        };

        update_state_and_build_notice(&self.config, &self.state_store, &status, now_unix_secs())
    }

    pub fn record_notice_delivered(&self, notice: &UpdateNotice) {
        let _ = record_notice_delivered(&self.state_store, &notice.latest_version, now_unix_secs());
    }
}

fn update_state_and_build_notice(
    config: &UpdateConfig,
    state_store: &UpdateStateStore,
    status: &UpdateStatus,
    now_unix_secs: u64,
) -> UpdateRuntimeTick {
    let mut warnings = Vec::new();
    let mut state = match state_store.load() {
        Ok(state) => state,
        Err(err) => {
            warnings.push(format!(
                "update state {} could not be read; recreating: {err}",
                state_store.path().display()
            ));
            UpdateState::default()
        }
    };

    state.last_checked_at_unix_secs = Some(now_unix_secs);
    state.last_latest_version = status.latest_version.clone();
    state.last_latest_url = status.release_url.clone();

    let mut notice = None;
    if status.is_newer && config.notify {
        if let (Some(latest_version), Some(release_url)) =
            (status.latest_version.clone(), status.release_url.clone())
        {
            if should_notify(
                &state,
                &latest_version,
                now_unix_secs,
                config.remind_interval.as_secs(),
            ) {
                let release_name = status
                    .release_name
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| latest_version.clone());
                notice = Some(UpdateNotice {
                    current_version: status.current_version.clone(),
                    latest_version,
                    release_name,
                    release_url,
                    published_at: status.published_at.clone(),
                    body_markdown: render_update_notification(status),
                    install_hint: INSTALL_HINT.to_string(),
                });
            }
        }
    }

    if let Err(err) = state_store.save(&state) {
        warnings.push(format!(
            "update state {} could not be saved; reminder throttling may not persist: {err}",
            state_store.path().display()
        ));
    }

    UpdateRuntimeTick { notice, warnings }
}

fn record_notice_delivered(
    state_store: &UpdateStateStore,
    latest_version: &str,
    now_unix_secs: u64,
) -> Result<(), std::io::Error> {
    let mut state = state_store.load()?;
    state.last_notified_version = Some(latest_version.to_string());
    state.last_notified_at_unix_secs = Some(now_unix_secs);
    state_store.save(&state)
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_state_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "lucarned-ctl-update-runtime-{name}-{}-{unique}.json",
            std::process::id()
        ))
    }

    fn newer_status() -> UpdateStatus {
        UpdateStatus {
            current_version: "0.1.0".to_string(),
            latest_version: Some("0.2.0".to_string()),
            release_name: Some("Lucarne 0.2.0".to_string()),
            release_url: Some("https://example.invalid/release".to_string()),
            published_at: Some("2026-05-21T00:00:00Z".to_string()),
            release_body: Some("Changes".to_string()),
            is_newer: true,
            automatic_checks_enabled: true,
        }
    }

    #[test]
    fn config_default_matches_design() {
        let config = UpdateConfig::default();
        assert!(config.enabled);
        assert!(config.notify);
        assert_eq!(config.check_interval, Duration::from_secs(24 * 60 * 60));
        assert_eq!(config.remind_interval, Duration::from_secs(24 * 60 * 60));
        assert_eq!(config.repository, "tuchg/Lucarne");
        assert_eq!(config.startup_delay, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn disabled_check_does_not_report_latest() {
        let client = reqwest::Client::new();
        let config = UpdateConfig {
            enabled: false,
            ..UpdateConfig::default()
        };
        let status = check_now(&client, &config, "0.1.0").await.unwrap();
        assert_eq!(status.current_version, "0.1.0");
        assert_eq!(status.latest_version, None);
        assert!(!status.is_newer);
        assert!(!status.automatic_checks_enabled);
    }

    #[tokio::test]
    async fn disabled_runtime_returns_no_notice() {
        let client = reqwest::Client::new();
        let config = UpdateConfig {
            enabled: false,
            startup_delay: Duration::from_secs(0),
            ..UpdateConfig::default()
        };
        let store = UpdateStateStore::new(PathBuf::from("/tmp/lucarned-ctl-disabled-runtime.json"));
        let mut runtime = UpdateRuntime::new(config, store);
        assert!(!runtime.enabled());
        assert_eq!(runtime.next_notice(&client, "0.1.0").await, None);
    }

    #[test]
    fn newer_status_builds_notice_and_updates_state() {
        let path = temp_state_path("notice");
        let store = UpdateStateStore::new(path.clone());
        let config = UpdateConfig {
            remind_interval: Duration::from_secs(100),
            ..UpdateConfig::default()
        };

        let tick = update_state_and_build_notice(&config, &store, &newer_status(), 200);
        assert!(tick.warnings.is_empty());
        let notice = tick.notice.unwrap();

        assert_eq!(notice.latest_version, "0.2.0");
        assert_eq!(notice.release_name, "Lucarne 0.2.0");
        assert!(notice.body_markdown.contains("Lucarne update available"));
        assert!(notice.body_markdown.contains("Current: 0.1.0"));
        assert!(notice.body_markdown.contains("Latest: 0.2.0"));
        assert!(notice.body_markdown.contains("Changes"));
        assert!(notice.body_markdown.contains("curl -fsSL"));
        assert_eq!(notice.install_hint, INSTALL_HINT);
        assert_eq!(
            store.load().unwrap(),
            UpdateState {
                last_checked_at_unix_secs: Some(200),
                last_latest_version: Some("0.2.0".to_string()),
                last_latest_url: Some("https://example.invalid/release".to_string()),
                last_notified_version: None,
                last_notified_at_unix_secs: None,
            }
        );
        record_notice_delivered(&store, &notice.latest_version, 201).unwrap();
        let state = store.load().unwrap();
        assert_eq!(state.last_notified_version.as_deref(), Some("0.2.0"));
        assert_eq!(state.last_notified_at_unix_secs, Some(201));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn notify_false_updates_state_without_notice() {
        let path = temp_state_path("notify-false");
        let store = UpdateStateStore::new(path.clone());
        let config = UpdateConfig {
            notify: false,
            ..UpdateConfig::default()
        };

        let tick = update_state_and_build_notice(&config, &store, &newer_status(), 200);
        assert!(tick.warnings.is_empty());
        assert_eq!(tick.notice, None);
        let state = store.load().unwrap();
        assert_eq!(state.last_checked_at_unix_secs, Some(200));
        assert_eq!(state.last_latest_version.as_deref(), Some("0.2.0"));
        assert_eq!(state.last_notified_version, None);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn throttled_version_updates_check_without_notice() {
        let path = temp_state_path("throttled");
        let store = UpdateStateStore::new(path.clone());
        store
            .save(&UpdateState {
                last_notified_version: Some("0.2.0".to_string()),
                last_notified_at_unix_secs: Some(150),
                ..UpdateState::default()
            })
            .unwrap();
        let config = UpdateConfig {
            remind_interval: Duration::from_secs(100),
            ..UpdateConfig::default()
        };

        let tick = update_state_and_build_notice(&config, &store, &newer_status(), 200);
        assert!(tick.warnings.is_empty());
        assert_eq!(tick.notice, None);
        let state = store.load().unwrap();
        assert_eq!(state.last_checked_at_unix_secs, Some(200));
        assert_eq!(state.last_latest_version.as_deref(), Some("0.2.0"));
        assert_eq!(state.last_notified_at_unix_secs, Some(150));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn malformed_state_warns_and_still_builds_notice() {
        let path = temp_state_path("malformed-runtime");
        std::fs::write(&path, "not json").unwrap();
        let store = UpdateStateStore::new(path.clone());

        let tick =
            update_state_and_build_notice(&UpdateConfig::default(), &store, &newer_status(), 200);

        assert!(tick.notice.is_some());
        assert_eq!(tick.warnings.len(), 1);
        assert!(tick.warnings[0].contains("could not be read"));
        let state = store.load().unwrap();
        assert_eq!(state.last_latest_version.as_deref(), Some("0.2.0"));
        let _ = std::fs::remove_file(path);
    }
}
