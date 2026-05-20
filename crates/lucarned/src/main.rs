use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use clap::{Parser, Subcommand};
use lucarne::{default_lucarned_home_dir, default_state_db_path, CoreOptions, LucarneCore};
use lucarne_adapter::{
    default_http_client, AdapterConfig, AdapterContext, AdapterError, AdapterPlugin,
    AdapterRegistry, AdapterResult, AdapterStatusReader, AdapterSupervisorHandle,
    AdapterSupervisorOptions, GlobalConfigPersistence, GlobalConfigUpdate,
};
use lucarne_telegram::telegram_plugin;
use lucarne_wechat::wechat_plugin;
use serde::Deserialize;
use tokio::sync::watch;
use tracing::{info, warn};
use tracing_appender::{
    non_blocking::NonBlockingBuilder,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{
    filter::{LevelFilter, Targets},
    prelude::*,
};

mod health;
mod onboarding;

const DEFAULT_LOG_BUFFERED_LINES: usize = 1024;
const DEFAULT_LOG_MAX_FILES: usize = 16;
const DEFAULT_HEALTH_ADDR: &str = "127.0.0.1:7766";

#[derive(Debug, Parser)]
#[command(name = "lucarned", about = "lucarne daemon")]
struct Cli {
    #[command(subcommand)]
    command: Option<LucarnedCommand>,
}

#[derive(Debug, Subcommand)]
enum LucarnedCommand {
    /// Configure lucarned interactively in a terminal.
    Init,
}

const DEFAULT_LUCARNED_CONFIG: &str = r#"agents:
  - claude
  - codex
  - copilot
  - gemini
  - pi

state:
  db: ~/.lucarned/state.sqlite3

logging:
  filter: "info,lucarne=debug,lucarned=debug"
  dir: ~/.lucarned/logs
  file: null
  max_files: 16
  buffered_lines: 1024

health:
  enabled: false
  addr: 127.0.0.1:7766

turn:
  inactivity_secs: 1800
  deadline_secs: 3600

session:
  idle_timeout_secs: 7200

config:
  global:
    bypass: false
    notifications: true

channels:
  telegram:
    enabled: false
    token: ""
    entry_chat_id: null

  wechat:
    enabled: false
    credential_path: ~/.lucarned/wechat-credentials.json
    force_login: false
    context:
      ttl_secs: 7200
      expiry_remind_before_secs: 300
      expiry_reminder_template: "会话将在 {remaining_minutes} 分钟后到期，请回复以保持会话可用。"
    rate_limit:
      retry_after_secs: 90
      max_retries: 3
      interaction_window_secs: 300
      interaction_threshold: 6
      interaction_prompt: "微信主动通知快到发送限制了，请回复任意消息以刷新会话。"
"#;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Some(LucarnedCommand::Init) => onboarding::run_interactive_init().await,
        None => run_daemon().await,
    }
}

async fn run_daemon() -> Result<(), Box<dyn std::error::Error>> {
    lucarne::memory_profile_snapshot!("lucarned.main.start");
    dotenvy::dotenv().ok();
    lucarne::memory_profile_snapshot!("lucarned.main.after_dotenv");
    let config_path = resolve_config_path()?;
    let file_config = LucarnedFileConfig::from_path_opt(config_path.as_deref())?;
    init_tracing(&file_config)?;
    lucarne::memory_profile_snapshot!("lucarned.main.after_tracing");

    let config = AdapterConfig::from_env_and_file(std::env::vars(), config_path.as_deref())?;
    lucarne::memory_profile_snapshot!("lucarned.main.after_config_load");
    let health_addr = health_addr_from_config(&file_config)?;

    let config = Arc::new(config);
    let mut registry = AdapterRegistry::default();
    let enabled_adapter_count =
        usize::from(register_if_enabled(&mut registry, wechat_plugin(), &config))
            + usize::from(register_if_enabled(
                &mut registry,
                telegram_plugin(),
                &config,
            ));
    lucarne::memory_profile_snapshot!("lucarned.main.after_register_adapters");

    if enabled_adapter_count == 0 && health_addr.is_none() {
        lucarne::memory_profile_snapshot!("lucarned.main.no_enabled_adapters");
        info!(
            config_path = ?config_path,
            "no adapters enabled; edit lucarned config to enable a channel"
        );
        return Ok(());
    }

    let state_db_path = state_db_path_from_config(&file_config)?;
    lucarne::memory_profile_snapshot!("lucarned.main.before_open_sqlite");
    let core = open_core_from_config(&state_db_path, &file_config)?;
    apply_global_config_from_file(&core, &file_config)?;
    lucarne::memory_profile_snapshot!("lucarned.main.after_open_sqlite");
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let global_config_persistence = config_path.as_ref().map(|path| {
        Arc::new(YamlGlobalConfigPersistence::new(path.clone())) as Arc<dyn GlobalConfigPersistence>
    });

    let (mut adapter_supervisor, adapter_status_reader) = if enabled_adapter_count > 0 {
        let http_client = default_http_client()?;
        lucarne::memory_profile_snapshot!("lucarned.main.before_supervise_enabled");
        let supervisor = registry
            .supervise_enabled(
                AdapterContext {
                    core: Arc::clone(&core),
                    config: Arc::clone(&config),
                    shutdown: shutdown_rx,
                    http_client,
                    global_config_persistence: global_config_persistence.clone(),
                },
                AdapterSupervisorOptions::default(),
            )
            .await?;
        lucarne::memory_profile_snapshot!("lucarned.main.after_supervise_enabled");
        let status_reader = supervisor.status_reader();
        (Some(supervisor), status_reader)
    } else {
        (None, AdapterStatusReader::empty())
    };

    if let Some(addr) = health_addr {
        let listener = health::bind_health_listener(addr).await?;
        let addr = listener.local_addr()?;
        let health_state = health::HealthState::new(
            Arc::clone(&core),
            adapter_status_reader.clone(),
            state_db_path.clone(),
        );
        let health_shutdown = shutdown_tx.subscribe();
        tokio::spawn(async move {
            if let Err(err) = health::serve_health(listener, health_state, health_shutdown).await {
                warn!(error = %err, "lucarned health server stopped");
            }
        });
        info!(addr = %addr, "lucarned health server started");
    }
    info!(
        adapters = adapter_status_reader.snapshot().len(),
        config_path = ?config_path,
        "lucarned started supervised adapter tasks"
    );
    lucarne::memory_profile_snapshot!("lucarned.main.before_wait");

    let fatal_error = wait_for_shutdown_or_adapter_fatal(adapter_supervisor.as_mut()).await;
    if let Some(fatal) = fatal_error {
        let _ = shutdown_tx.send(true);
        return Err(format!("adapter {} fatal: {}", fatal.id, fatal.error).into());
    }
    let _ = shutdown_tx.send(true);
    info!(
        adapters = adapter_status_reader.snapshot().len(),
        "lucarned shutdown requested"
    );
    Ok(())
}

fn init_tracing(file_config: &LucarnedFileConfig) -> Result<(), Box<dyn std::error::Error>> {
    lucarne::memory_profile_snapshot!("lucarned.init_tracing.start");
    let filter_spec = log_filter_spec(file_config);
    let stderr_filter = parse_log_filter(&filter_spec);
    let file_filter = parse_log_filter(&filter_spec);
    lucarne::memory_profile_snapshot!("lucarned.init_tracing.after_filters");

    let config = log_file_config_from_config(file_config);
    let log_target = lucarne_log_file_target(file_config)?;
    lucarne::memory_profile_snapshot!("lucarned.init_tracing.before_file_appender");
    let file_appender = lucarne_file_appender(&log_target, config)?;
    lucarne::memory_profile_snapshot!("lucarned.init_tracing.after_file_appender");
    let (file_writer, guard) = NonBlockingBuilder::default()
        .buffered_lines_limit(config.buffered_lines)
        .thread_name("lucarned-log")
        .finish(file_appender);
    Box::leak(Box::new(guard));
    lucarne::memory_profile_snapshot!("lucarned.init_tracing.after_nonblocking");

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_writer(std::io::stderr)
        .with_filter(stderr_filter);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_writer(file_writer)
        .with_filter(file_filter);
    lucarne::memory_profile_snapshot!("lucarned.init_tracing.after_layers");

    let _ = tracing_subscriber::registry()
        .with(stderr_layer)
        .with(file_layer)
        .try_init();
    lucarne::memory_profile_snapshot!("lucarned.init_tracing.after_try_init");
    info!(
        target = %log_target.display(),
        max_files = config.max_files,
        buffered_lines = config.buffered_lines,
        "lucarned file logging enabled"
    );
    Ok(())
}

async fn wait_for_shutdown_or_adapter_fatal(
    adapter_supervisor: Option<&mut AdapterSupervisorHandle>,
) -> Option<lucarne_adapter::AdapterFatal> {
    if let Some(adapter_supervisor) = adapter_supervisor {
        tokio::select! {
            fatal = adapter_supervisor.next_fatal() => fatal,
            signal = tokio::signal::ctrl_c() => {
                if let Err(err) = signal {
                    warn!(error = %err, "failed to wait for ctrl-c signal; shutting down");
                }
                None
            }
        }
    } else {
        if let Err(err) = tokio::signal::ctrl_c().await {
            warn!(error = %err, "failed to wait for ctrl-c signal; shutting down");
        }
        None
    }
}

fn register_if_enabled<P>(registry: &mut AdapterRegistry, plugin: P, config: &AdapterConfig) -> bool
where
    P: AdapterPlugin + 'static,
{
    let enabled = plugin.enabled(config);
    if enabled {
        registry.register(plugin);
        true
    } else {
        tracing::debug!(
            target: "lucarned",
            adapter_id = plugin.id(),
            adapter_name = plugin.name(),
            "adapter plugin skipped disabled"
        );
        false
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct LucarnedFileConfig {
    #[serde(default)]
    agents: Option<Vec<String>>,
    #[serde(default)]
    state: StateFileConfig,
    #[serde(default)]
    logging: LoggingFileConfig,
    #[serde(default)]
    health: HealthFileConfig,
    #[serde(default)]
    turn: TurnFileConfig,
    #[serde(default)]
    session: SessionFileConfig,
    #[serde(default, rename = "config")]
    runtime_config: RuntimeFileConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct StateFileConfig {
    db: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct LoggingFileConfig {
    filter: Option<String>,
    dir: Option<String>,
    file: Option<String>,
    max_files: Option<usize>,
    buffered_lines: Option<usize>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct HealthFileConfig {
    enabled: Option<bool>,
    addr: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct TurnFileConfig {
    inactivity_secs: Option<u64>,
    deadline_secs: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SessionFileConfig {
    idle_timeout_secs: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct RuntimeFileConfig {
    #[serde(default)]
    global: GlobalRuntimeFileConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct GlobalRuntimeFileConfig {
    bypass: Option<bool>,
    notifications: Option<bool>,
}

impl LucarnedFileConfig {
    fn from_path_opt(path: Option<&Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let raw = std::fs::read_to_string(path)
            .map_err(|err| format!("failed to read lucarned config {}: {err}", path.display()))?;
        Self::from_yaml_str(&raw).map_err(|err| {
            format!("failed to parse lucarned config {}: {err}", path.display()).into()
        })
    }

    fn from_yaml_str(raw: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(raw)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LogFileConfig {
    buffered_lines: usize,
    max_files: usize,
}

impl Default for LogFileConfig {
    fn default() -> Self {
        Self {
            buffered_lines: DEFAULT_LOG_BUFFERED_LINES,
            max_files: DEFAULT_LOG_MAX_FILES,
        }
    }
}

fn open_core_from_config(
    path: impl AsRef<Path>,
    config: &LucarnedFileConfig,
) -> Result<Arc<LucarneCore>, Box<dyn std::error::Error>> {
    let options = core_options_from_config(config)?;
    match config.agents.as_ref() {
        Some(agents) => Ok(LucarneCore::open_sqlite_with_provider_filter_and_options(
            path, agents, options,
        )?),
        None => Ok(LucarneCore::open_sqlite_with_options(path, options)?),
    }
}

fn apply_global_config_from_file(
    core: &LucarneCore,
    config: &LucarnedFileConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(enabled) = config.runtime_config.global.bypass {
        core.set_force_bypass_permissions(enabled)?;
    }
    if let Some(enabled) = config.runtime_config.global.notifications {
        core.set_global_notifications_enabled(enabled)?;
    }
    Ok(())
}

fn core_options_from_config(
    config: &LucarnedFileConfig,
) -> Result<CoreOptions, Box<dyn std::error::Error>> {
    let defaults = CoreOptions::default();
    let options = CoreOptions {
        turn_inactivity: env_secs("LUCARNE_TURN_INACTIVITY_SECS")
            .or(config.turn.inactivity_secs)
            .map(Duration::from_secs)
            .unwrap_or(defaults.turn_inactivity),
        turn_deadline: env_secs("LUCARNE_TURN_DEADLINE_SECS")
            .or(config.turn.deadline_secs)
            .map(Duration::from_secs)
            .unwrap_or(defaults.turn_deadline),
        session_idle_timeout: env_secs("LUCARNE_SESSION_IDLE_TIMEOUT_SECS")
            .or(config.session.idle_timeout_secs)
            .map(Duration::from_secs)
            .unwrap_or(defaults.session_idle_timeout),
    };
    validate_core_options(&options)?;
    Ok(options)
}

fn env_secs(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.parse().ok()
}

fn validate_core_options(options: &CoreOptions) -> Result<(), Box<dyn std::error::Error>> {
    if options.turn_inactivity.is_zero() {
        return Err("turn.inactivity_secs must be greater than zero".into());
    }
    if options.turn_deadline.is_zero() {
        return Err("turn.deadline_secs must be greater than zero".into());
    }
    if options.session_idle_timeout.is_zero() {
        return Err("session.idle_timeout_secs must be greater than zero".into());
    }
    if options.turn_deadline < options.turn_inactivity {
        return Err(
            "turn.deadline_secs must be greater than or equal to turn.inactivity_secs".into(),
        );
    }
    Ok(())
}

fn state_db_path_from_config(
    config: &LucarnedFileConfig,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    std::env::var("LUCARNE_STATE_DB")
        .ok()
        .map(PathBuf::from)
        .or_else(|| config.state.db.as_deref().map(expand_home_path))
        .or_else(default_state_db_path)
        .ok_or_else(|| "LUCARNE_STATE_DB default path unavailable".into())
}

fn health_addr_from_config(
    config: &LucarnedFileConfig,
) -> Result<Option<std::net::SocketAddr>, Box<dyn std::error::Error>> {
    let enabled = std::env::var("LUCARNED_HEALTH_ENABLED")
        .ok()
        .as_deref()
        .and_then(parse_bool)
        .or(config.health.enabled)
        .unwrap_or(false);
    if !enabled {
        return Ok(None);
    }

    let addr = std::env::var("LUCARNED_HEALTH_ADDR")
        .ok()
        .or_else(|| config.health.addr.clone())
        .unwrap_or_else(|| DEFAULT_HEALTH_ADDR.to_string());
    health::parse_health_addr(&addr)
        .map(Some)
        .map_err(Into::into)
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn log_filter_spec(config: &LucarnedFileConfig) -> String {
    std::env::var("RUST_LOG")
        .ok()
        .or_else(|| config.logging.filter.clone())
        .unwrap_or_else(default_log_filter_spec)
}

fn default_log_filter_spec() -> String {
    "info,lucarne=debug,lucarne::core_service=debug,lucarne::control_plane=debug,lucarne_adapter=debug,lucarne_channel=debug,lucarne_telegram=debug,lucarne_wechat=debug,wechat_ilink=debug,lucarned=debug,agent_sessions=debug"
        .to_string()
}

fn log_file_config_from_config(config: &LucarnedFileConfig) -> LogFileConfig {
    LogFileConfig {
        buffered_lines: config
            .logging
            .buffered_lines
            .unwrap_or(DEFAULT_LOG_BUFFERED_LINES),
        max_files: config.logging.max_files.unwrap_or(DEFAULT_LOG_MAX_FILES),
    }
}

fn parse_log_filter(filter_spec: &str) -> Targets {
    filter_spec
        .parse::<Targets>()
        .unwrap_or_else(|_| Targets::new().with_default(LevelFilter::INFO))
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum LogFileTarget {
    File(PathBuf),
    Directory(PathBuf),
}

impl LogFileTarget {
    fn display(&self) -> String {
        match self {
            Self::File(path) => path.display().to_string(),
            Self::Directory(path) => format!("{}/lucarned.YYYY-MM-DD.log", path.display()),
        }
    }
}

fn lucarne_file_appender(
    target: &LogFileTarget,
    config: LogFileConfig,
) -> std::io::Result<RollingFileAppender> {
    match target {
        LogFileTarget::File(path) => explicit_file_appender(path),
        LogFileTarget::Directory(path) => daily_directory_appender(path, config),
    }
}

fn explicit_file_appender(path: &Path) -> std::io::Result<RollingFileAppender> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let filename = path
        .file_name()
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing log file name")
        })?
        .to_string_lossy()
        .into_owned();

    RollingFileAppender::builder()
        .rotation(Rotation::NEVER)
        .filename_prefix(filename)
        .build(directory)
        .map_err(log_init_error)
}

fn daily_directory_appender(
    directory: &Path,
    config: LogFileConfig,
) -> std::io::Result<RollingFileAppender> {
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("lucarned")
        .filename_suffix("log")
        .max_log_files(config.max_files)
        .build(directory)
        .map_err(log_init_error)
}

fn log_init_error(err: tracing_appender::rolling::InitError) -> std::io::Error {
    std::io::Error::other(err)
}

fn lucarne_log_file_target(
    config: &LucarnedFileConfig,
) -> Result<LogFileTarget, Box<dyn std::error::Error>> {
    if let Ok(path) = std::env::var("LUCARNE_LOG_FILE") {
        return Ok(LogFileTarget::File(PathBuf::from(path)));
    }
    if let Ok(path) = std::env::var("LUCARNE_LOG_DIR") {
        return Ok(LogFileTarget::Directory(PathBuf::from(path)));
    }
    let home = default_lucarned_home_dir().ok_or("LUCARNE_LOG_DIR default path unavailable")?;
    log_file_target_from_config(config, &home)
}

fn log_file_target_from_config(
    config: &LucarnedFileConfig,
    default_home: &Path,
) -> Result<LogFileTarget, Box<dyn std::error::Error>> {
    if let Some(path) = config.logging.file.as_deref() {
        return Ok(LogFileTarget::File(expand_home_path(path)));
    }
    if let Some(path) = config.logging.dir.as_deref() {
        return Ok(LogFileTarget::Directory(expand_home_path(path)));
    }
    Ok(default_log_file_target_in(default_home))
}

fn default_log_file_target_in(home: &Path) -> LogFileTarget {
    LogFileTarget::Directory(home.join("logs"))
}

fn resolve_config_path() -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    if let Some(path) = explicit_config_path() {
        return Ok(Some(path));
    }
    let Some(home) = default_lucarned_home_dir() else {
        return Ok(None);
    };
    let path = default_config_path_in(&home);
    ensure_default_config_file(&path)?;
    Ok(Some(path))
}

fn explicit_config_path() -> Option<PathBuf> {
    std::env::var("LUCARNE_CONFIG")
        .or_else(|_| std::env::var("LUCARNED_CONFIG"))
        .ok()
        .map(PathBuf::from)
}

fn ensure_default_config_file(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => file.write_all(DEFAULT_LUCARNED_CONFIG.as_bytes())?,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(err.into()),
    }
    Ok(())
}

fn default_config_path_in(home: &Path) -> PathBuf {
    home.join("lucarned.yaml")
}

fn expand_home_path(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
            return PathBuf::from(home).join(rest);
        }
    }
    if value == "~" {
        if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(value)
}

#[derive(Clone, Debug)]
struct YamlGlobalConfigPersistence {
    path: PathBuf,
}

impl YamlGlobalConfigPersistence {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl GlobalConfigPersistence for YamlGlobalConfigPersistence {
    fn persist_global_config(&self, update: GlobalConfigUpdate) -> AdapterResult<()> {
        persist_global_config_to_yaml(&self.path, update).map_err(|err| {
            AdapterError::permanent(format!(
                "failed to write lucarned config {}: {err}",
                self.path.display()
            ))
        })
    }
}

fn persist_global_config_to_yaml(
    path: &Path,
    update: GlobalConfigUpdate,
) -> Result<(), Box<dyn std::error::Error>> {
    let raw = std::fs::read_to_string(path)?;
    let yaml = update_global_config_yaml(&raw, update)?;
    onboarding::config::write_config_with_backup(path, &yaml)
}

fn update_global_config_yaml(
    raw: &str,
    update: GlobalConfigUpdate,
) -> Result<String, serde_yaml::Error> {
    let mut value = serde_yaml::from_str::<serde_yaml::Value>(raw)?;
    let root = ensure_yaml_mapping(&mut value);
    let config = ensure_yaml_child_mapping(root, "config");
    let global = ensure_yaml_child_mapping(config, "global");
    global.insert(
        serde_yaml::Value::String("bypass".to_string()),
        serde_yaml::Value::Bool(update.bypass),
    );
    global.insert(
        serde_yaml::Value::String("notifications".to_string()),
        serde_yaml::Value::Bool(update.notifications),
    );
    serde_yaml::to_string(&value)
}

fn ensure_yaml_mapping(value: &mut serde_yaml::Value) -> &mut serde_yaml::Mapping {
    if !matches!(value, serde_yaml::Value::Mapping(_)) {
        *value = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }
    match value {
        serde_yaml::Value::Mapping(mapping) => mapping,
        _ => unreachable!("value was forced to mapping"),
    }
}

fn ensure_yaml_child_mapping<'a>(
    mapping: &'a mut serde_yaml::Mapping,
    key: &str,
) -> &'a mut serde_yaml::Mapping {
    let yaml_key = serde_yaml::Value::String(key.to_string());
    if !matches!(mapping.get(&yaml_key), Some(serde_yaml::Value::Mapping(_))) {
        mapping.insert(
            yaml_key.clone(),
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
        );
    }
    match mapping.get_mut(&yaml_key) {
        Some(serde_yaml::Value::Mapping(child)) => child,
        _ => unreachable!("child was forced to mapping"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn cli_parses_init_subcommand() {
        let cli = Cli::try_parse_from(["lucarned", "init"]).expect("parse init cli");
        assert!(matches!(cli.command, Some(LucarnedCommand::Init)));
    }

    #[test]
    fn cli_defaults_to_daemon_without_subcommand() {
        let cli = Cli::try_parse_from(["lucarned"]).expect("parse daemon cli");
        assert!(cli.command.is_none());
    }

    #[test]
    fn cli_rejects_unknown_subcommand() {
        let err =
            Cli::try_parse_from(["lucarned", "configure"]).expect_err("unknown command fails");
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn default_log_file_config_bounds_memory_and_disk_growth() {
        let config = LogFileConfig::default();

        assert_eq!(config.buffered_lines, 1024);
        assert_eq!(config.max_files, 16);
    }

    #[test]
    fn tokio_runtime_uses_two_worker_threads() {
        let source = include_str!("main.rs");
        let compact_source = source
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();

        assert!(compact_source.contains("#[tokio::main(flavor=\"multi_thread\",worker_threads=2)]"));
    }

    #[test]
    fn memory_profile_snapshots_are_feature_gated() {
        let manifest = include_str!("../Cargo.toml");
        let source = include_str!("main.rs");
        let production_source = source.split("#[cfg(test)]").next().unwrap_or(source);

        let lucarne_lib = include_str!("../../lucarne/src/lib.rs");
        let lucarne_observability = include_str!("../../lucarne/src/observability.rs");

        assert!(manifest.contains("memory-profiling = [\"lucarne/memory-profiling\"]"));
        assert!(lucarne_lib.contains("macro_rules! memory_profile_snapshot"));
        assert!(lucarne_lib.contains("#[cfg(feature = \"memory-profiling\")]"));
        assert!(production_source
            .contains("lucarne::memory_profile_snapshot!(\"lucarned.main.start\")"));
        assert!(production_source.contains(
            "lucarne::memory_profile_snapshot!(\"lucarned.init_tracing.after_nonblocking\")"
        ));
        assert!(lucarne_observability.contains("LUCARNE_MEMORY_PROFILE_PAUSE_MS"));
    }

    #[test]
    fn default_config_exposes_wechat_rate_limit_knobs() {
        assert!(DEFAULT_LUCARNED_CONFIG.contains("retry_after_secs: 90"));
        assert!(DEFAULT_LUCARNED_CONFIG.contains("max_retries: 3"));
        assert!(DEFAULT_LUCARNED_CONFIG.contains("interaction_window_secs: 300"));
        assert!(DEFAULT_LUCARNED_CONFIG.contains("interaction_threshold: 6"));
        assert!(DEFAULT_LUCARNED_CONFIG.contains(
            "interaction_prompt: \"微信主动通知快到发送限制了，请回复任意消息以刷新会话。\""
        ));
    }

    #[test]
    fn default_log_target_uses_lucarned_home_logs_dir() {
        let temp_dir = tempfile::tempdir().expect("create temp home dir");

        assert_eq!(
            default_log_file_target_in(temp_dir.path()),
            LogFileTarget::Directory(temp_dir.path().join("logs"))
        );
    }

    #[test]
    fn default_adapter_config_path_uses_lucarned_yaml() {
        let temp_dir = tempfile::tempdir().expect("create temp config dir");

        assert_eq!(
            default_config_path_in(temp_dir.path()),
            temp_dir.path().join("lucarned.yaml")
        );
    }

    #[test]
    fn missing_default_config_file_is_bootstrapped() {
        let temp_dir = tempfile::tempdir().expect("create temp config dir");
        let config_path = default_config_path_in(temp_dir.path());

        ensure_default_config_file(&config_path).expect("bootstrap default config");

        let raw = std::fs::read_to_string(&config_path).expect("read bootstrapped config");
        assert!(raw.contains("enabled: false"));
        assert!(raw.contains("health:"));
        assert!(raw.contains("turn:"));
        assert!(raw.contains("inactivity_secs: 1800"));
        assert!(raw.contains("deadline_secs: 3600"));
        assert!(raw.contains("session:"));
        assert!(raw.contains("idle_timeout_secs: 7200"));
        assert!(raw.contains("config:"));
        assert!(raw.contains("global:"));
        assert!(raw.contains("bypass: false"));
        assert!(raw.contains("notifications: true"));
        assert!(raw.contains("agents:"));
        assert!(raw.contains("  - claude"));
        assert!(raw.contains("  - codex"));
        assert!(raw.contains("  - copilot"));
        assert!(raw.contains("  - gemini"));
        assert!(raw.contains("  - pi"));
        assert!(raw.contains("telegram:"));
        assert!(raw.contains("wechat:"));

        let daemon_config = LucarnedFileConfig::from_yaml_str(&raw).expect("parse daemon config");
        assert_eq!(daemon_config.health.enabled, Some(false));
        assert_eq!(daemon_config.runtime_config.global.bypass, Some(false));
        assert_eq!(
            daemon_config.runtime_config.global.notifications,
            Some(true)
        );

        let adapter_config =
            AdapterConfig::from_env_and_file(Vec::<(String, String)>::new(), Some(&config_path))
                .expect("parse adapter config");
        assert_eq!(adapter_config.channel_enabled("telegram"), Some(false));
        assert_eq!(adapter_config.channel_enabled("wechat"), Some(false));
    }

    #[test]
    fn existing_default_config_file_is_not_overwritten() {
        let temp_dir = tempfile::tempdir().expect("create temp config dir");
        let config_path = default_config_path_in(temp_dir.path());
        std::fs::write(&config_path, "channels: {}\n").expect("write existing config");

        ensure_default_config_file(&config_path).expect("bootstrap default config");

        assert_eq!(
            std::fs::read_to_string(&config_path).expect("read existing config"),
            "channels: {}\n"
        );
    }

    #[test]
    fn lucarned_file_config_parses_core_daemon_settings() {
        let config = LucarnedFileConfig::from_yaml_str(
            r#"
state:
  db: ~/.lucarned/custom.sqlite3
logging:
  filter: info,lucarned=debug
  dir: ~/.lucarned/custom-logs
  max_files: 7
  buffered_lines: 64
health:
  enabled: true
  addr: 127.0.0.1:7766
"#,
        )
        .expect("parse lucarned config");

        assert_eq!(
            config.state.db.as_deref(),
            Some("~/.lucarned/custom.sqlite3")
        );
        assert_eq!(
            config.logging.filter.as_deref(),
            Some("info,lucarned=debug")
        );
        assert_eq!(
            config.logging.dir.as_deref(),
            Some("~/.lucarned/custom-logs")
        );
        assert_eq!(config.logging.max_files, Some(7));
        assert_eq!(config.logging.buffered_lines, Some(64));
        assert_eq!(config.health.enabled, Some(true));
        assert_eq!(config.health.addr.as_deref(), Some("127.0.0.1:7766"));
    }

    #[tokio::test]
    async fn core_open_uses_all_agents_when_filter_missing() {
        let temp_dir = tempfile::tempdir().expect("create temp state dir");
        let config = LucarnedFileConfig::from_yaml_str(
            r#"
state:
  db: ~/.lucarned/state.sqlite3
"#,
        )
        .expect("parse unfiltered config");

        let core = open_core_from_config(temp_dir.path().join("state.sqlite3"), &config)
            .expect("open unfiltered core");
        let unfiltered = LucarneCore::open_sqlite(temp_dir.path().join("unfiltered.sqlite3"))
            .expect("open directly unfiltered core");

        assert_eq!(core.provider_ids(), unfiltered.provider_ids());
    }

    #[tokio::test]
    async fn core_open_uses_agent_filter_from_config() {
        let temp_dir = tempfile::tempdir().expect("create temp state dir");
        let config = LucarnedFileConfig::from_yaml_str(
            r#"
agents:
  - codex
  - missing-provider
  - pi
"#,
        )
        .expect("parse filtered config");

        let core = open_core_from_config(temp_dir.path().join("state.sqlite3"), &config)
            .expect("open filtered core");

        assert_eq!(core.provider_ids(), &["codex", "pi"]);
        assert_eq!(core.history_provider_ids(), &["codex", "pi"]);
    }

    #[test]
    fn lucarned_file_config_parses_agent_filter() {
        let missing = LucarnedFileConfig::from_yaml_str(
            r#"
state:
  db: ~/.lucarned/state.sqlite3
"#,
        )
        .expect("parse missing agents config");
        assert_eq!(missing.agents, None);

        let subset = LucarnedFileConfig::from_yaml_str(
            r#"
agents:
  - codex
  - pi
"#,
        )
        .expect("parse subset agents config");
        assert_eq!(subset.agents, Some(vec!["codex".into(), "pi".into()]));

        let empty =
            LucarnedFileConfig::from_yaml_str("agents: []\n").expect("parse empty agents config");
        assert_eq!(empty.agents, Some(Vec::new()));
    }

    #[test]
    fn lucarned_file_config_parses_core_timeouts() {
        let config = LucarnedFileConfig::from_yaml_str(
            r#"
turn:
  inactivity_secs: 1800
  deadline_secs: 3600
session:
  idle_timeout_secs: 7200
"#,
        )
        .expect("parse timeout config");

        assert_eq!(config.turn.inactivity_secs, Some(1800));
        assert_eq!(config.turn.deadline_secs, Some(3600));
        assert_eq!(config.session.idle_timeout_secs, Some(7200));
    }

    #[test]
    fn lucarned_file_config_parses_optional_global_runtime_config() {
        let config = LucarnedFileConfig::from_yaml_str(
            r#"
config:
  global:
    bypass: true
    notifications: false
"#,
        )
        .expect("parse runtime config");

        assert_eq!(config.runtime_config.global.bypass, Some(true));
        assert_eq!(config.runtime_config.global.notifications, Some(false));

        let missing = LucarnedFileConfig::from_yaml_str("channels: {}\n")
            .expect("parse missing runtime config");
        assert_eq!(missing.runtime_config.global.bypass, None);
        assert_eq!(missing.runtime_config.global.notifications, None);
    }

    #[tokio::test]
    async fn apply_global_config_from_file_overrides_only_explicit_values() {
        let temp_dir = tempfile::tempdir().expect("create temp state dir");
        let core =
            LucarneCore::open_sqlite(temp_dir.path().join("state.sqlite3")).expect("open core");
        core.set_force_bypass_permissions(true)
            .expect("set initial bypass");
        core.set_global_notifications_enabled(false)
            .expect("set initial notifications");

        let missing = LucarnedFileConfig::from_yaml_str("channels: {}\n")
            .expect("parse missing runtime config");
        apply_global_config_from_file(&core, &missing).expect("apply missing config");
        assert!(core.system_settings().session.force_bypass_permissions);
        assert!(!core.system_settings().notifications.enabled);

        let explicit = LucarnedFileConfig::from_yaml_str(
            r#"
config:
  global:
    bypass: false
    notifications: true
"#,
        )
        .expect("parse explicit runtime config");
        apply_global_config_from_file(&core, &explicit).expect("apply explicit config");
        assert!(!core.system_settings().session.force_bypass_permissions);
        assert!(core.system_settings().notifications.enabled);
    }

    #[test]
    fn update_global_config_yaml_upserts_global_values_and_preserves_channels() {
        let raw = r#"
channels:
  telegram:
    enabled: true
    token: keep-me
config:
  other: keep
"#;

        let updated = update_global_config_yaml(
            raw,
            GlobalConfigUpdate {
                bypass: true,
                notifications: false,
            },
        )
        .expect("update yaml");
        let value: serde_json::Value = serde_yaml::from_str(&updated).expect("parse updated yaml");

        assert_eq!(
            value
                .pointer("/channels/telegram/token")
                .and_then(serde_json::Value::as_str),
            Some("keep-me")
        );
        assert_eq!(
            value
                .pointer("/config/other")
                .and_then(serde_json::Value::as_str),
            Some("keep")
        );
        assert_eq!(
            value
                .pointer("/config/global/bypass")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            value
                .pointer("/config/global/notifications")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn yaml_global_config_persistence_writes_backup_and_config_file() {
        let temp_dir = tempfile::tempdir().expect("create temp config dir");
        let config_path = temp_dir.path().join("lucarned.yaml");
        std::fs::write(&config_path, "channels:\n  wechat:\n    enabled: true\n")
            .expect("write config");
        let persistence = YamlGlobalConfigPersistence::new(config_path.clone());

        persistence
            .persist_global_config(GlobalConfigUpdate {
                bypass: true,
                notifications: false,
            })
            .expect("persist global config");

        let updated = std::fs::read_to_string(&config_path).expect("read updated config");
        let value: serde_json::Value = serde_yaml::from_str(&updated).expect("parse updated yaml");
        assert_eq!(
            value
                .pointer("/config/global/bypass")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            value
                .pointer("/config/global/notifications")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert!(
            std::fs::read_dir(temp_dir.path())
                .expect("read temp dir")
                .any(|entry| entry
                    .expect("dir entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with("lucarned.yaml.bak-")),
            "persisting should create a backup for existing config"
        );
    }

    #[test]
    fn core_options_defaults_match_timeout_policy() {
        let options = CoreOptions::default();

        assert_eq!(options.turn_inactivity, Duration::from_secs(1800));
        assert_eq!(options.turn_deadline, Duration::from_secs(3600));
        assert_eq!(options.session_idle_timeout, Duration::from_secs(7200));
    }

    #[test]
    fn core_timeout_validation_rejects_deadline_before_inactivity() {
        let options = CoreOptions {
            turn_inactivity: Duration::from_secs(3600),
            turn_deadline: Duration::from_secs(1800),
            session_idle_timeout: Duration::from_secs(7200),
        };

        let err = validate_core_options(&options).expect_err("invalid timeout ordering");
        assert!(err.to_string().contains("deadline_secs"));
    }

    #[test]
    fn health_config_requires_enabled_gate() {
        let default_config = LucarnedFileConfig::default();
        assert_eq!(
            health_addr_from_config(&default_config).expect("default health config"),
            None
        );

        let addr_only = LucarnedFileConfig::from_yaml_str(
            r#"
health:
  addr: 127.0.0.1:7766
"#,
        )
        .expect("parse addr-only health config");
        assert_eq!(
            health_addr_from_config(&addr_only).expect("addr-only health config"),
            None
        );

        let enabled = LucarnedFileConfig::from_yaml_str(
            r#"
health:
  enabled: true
"#,
        )
        .expect("parse enabled health config");
        assert_eq!(
            health_addr_from_config(&enabled).expect("enabled health config"),
            Some("127.0.0.1:7766".parse().unwrap())
        );
    }

    #[test]
    fn log_config_uses_yaml_when_env_absent() {
        let temp_dir = tempfile::tempdir().expect("create temp home dir");
        let config = LucarnedFileConfig::from_yaml_str(
            r#"
logging:
  dir: logs/custom
  max_files: 3
  buffered_lines: 32
"#,
        )
        .expect("parse lucarned config");

        assert_eq!(
            log_file_target_from_config(&config, temp_dir.path()).expect("log target"),
            LogFileTarget::Directory(PathBuf::from("logs/custom"))
        );
        assert_eq!(log_file_config_from_config(&config).max_files, 3);
        assert_eq!(log_file_config_from_config(&config).buffered_lines, 32);
    }

    #[test]
    fn daemon_registers_adapter_plugins_only_after_enabled_check() {
        let source = include_str!("main.rs");
        let production_source = source.split("#[cfg(test)]").next().unwrap_or(source);

        let compact_source = production_source
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();

        assert!(compact_source.contains("register_if_enabled(&mutregistry,wechat_plugin(),&config"));
        assert!(
            compact_source.contains("register_if_enabled(&mutregistry,telegram_plugin(),&config")
        );
        assert!(production_source.contains("adapter plugin skipped disabled"));
        assert!(!production_source.contains("registry.register(wechat_plugin())"));
        assert!(!production_source.contains("registry.register(telegram_plugin())"));
    }

    #[test]
    fn daemon_exits_idle_before_opening_core_or_http_client() {
        let source = include_str!("main.rs");
        let production_source = source.split("#[cfg(test)]").next().unwrap_or(source);
        let idle_exit = production_source
            .find("no adapters enabled; edit lucarned config to enable a channel")
            .expect("idle exit guidance log");
        let open_sqlite = production_source
            .find("LucarneCore::open_sqlite")
            .expect("core open");
        let http_client = production_source
            .find("default_http_client()")
            .expect("http client creation");

        assert!(production_source.contains("enabled_adapter_count == 0 && health_addr.is_none()"));
        assert!(idle_exit < open_sqlite);
        assert!(idle_exit < http_client);
    }

    #[test]
    fn log_writer_uses_bounded_buffer_and_daily_file_rotation() {
        let source = include_str!("main.rs");
        let compact_source = source
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        let bounded_builder = concat!("NonBlockingBuilder", "::default()");
        let bounded_limit = concat!(".buffered_lines_limit", "(config.buffered_lines)");

        assert!(source.contains(bounded_builder));
        assert!(compact_source.contains(bounded_limit));
        assert!(source.contains("Rotation::DAILY"));
        assert!(source.contains(".filename_prefix(\"lucarned\")"));
        assert!(source.contains(".filename_suffix(\"log\")"));
        assert!(source.contains(".max_log_files(config.max_files)"));
    }

    #[test]
    fn daily_file_appender_writes_dated_log_in_directory() {
        let temp_dir = tempfile::tempdir().expect("create temp log dir");
        let config = LogFileConfig {
            buffered_lines: 8,
            max_files: 2,
        };
        let target = LogFileTarget::Directory(temp_dir.path().to_path_buf());
        let mut appender = lucarne_file_appender(&target, config).expect("create log appender");

        writeln!(appender, "first line").expect("write first log line");
        appender.flush().expect("flush first log line");

        let files = std::fs::read_dir(temp_dir.path())
            .expect("read log dir")
            .map(|entry| entry.expect("log file").file_name())
            .collect::<Vec<_>>();
        assert!(files.iter().any(|name| {
            let name = name.to_string_lossy();
            name.starts_with("lucarned.") && name.ends_with(".log")
        }));
    }

    #[test]
    fn explicit_log_file_target_writes_exact_file() {
        let temp_dir = tempfile::tempdir().expect("create temp log dir");
        let log_path = temp_dir.path().join("custom.log");
        let config = LogFileConfig {
            buffered_lines: 8,
            max_files: 2,
        };
        let target = LogFileTarget::File(log_path.clone());
        let mut appender = lucarne_file_appender(&target, config).expect("create log appender");

        writeln!(appender, "first line").expect("write first log line");
        appender.flush().expect("flush first log line");

        assert!(log_path.exists());
    }
}
