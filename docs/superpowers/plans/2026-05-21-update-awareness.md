# Update Awareness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add update awareness that checks GitHub Releases, reports status in `doctor`/`update`, and notifies Telegram/WeChat users without self-updating.

**Architecture:** `lucarned-ctl` owns update config, release checking, version comparison, state throttling, and rendering behind an `updates` feature. `lucarned` creates the shared `lucarne_adapter::default_http_client()`, drives `UpdateRuntime` from the existing main wait loop, and forwards generic `SystemNotification` values to adapters. Telegram and WeChat consume notifications in their existing long-lived loops; no update-specific task is spawned.

**Tech Stack:** Rust, Tokio, reqwest, serde/serde_json/serde_yaml, semver, existing lucarne-adapter/telegram/wechat channel abstractions.

---

## File Structure

- `crates/lucarned-ctl/Cargo.toml` — add optional `updates` feature and optional dependencies.
- `crates/lucarned-ctl/src/updates/mod.rs` — public update facade: config, status, notices, checker entrypoints, runtime.
- `crates/lucarned-ctl/src/updates/github.rs` — GitHub latest release request and JSON parsing.
- `crates/lucarned-ctl/src/updates/version.rs` — `v` prefix normalization and semver comparison.
- `crates/lucarned-ctl/src/updates/state.rs` — `update-state.json` persistence and notification throttling.
- `crates/lucarned-ctl/src/updates/render.rs` — CLI, doctor, and notification text rendering.
- `crates/lucarned-ctl/src/args.rs` — add `update` command parser/usage/tests.
- `crates/lucarned-ctl/src/doctor.rs` — add async update-aware doctor helper behind feature.
- `crates/lucarned-ctl/src/lib.rs` — export update module behind feature and expose async run entrypoint.
- `crates/lucarne-adapter/src/lib.rs` — add channel-agnostic `SystemNotification` bus and context field.
- `crates/lucarned/Cargo.toml` — enable `lucarned-ctl/updates`.
- `crates/lucarned/src/main.rs` — parse update config, create shared client once, route `update`/async doctor, drive update runtime in wait loop.
- `crates/lucarned/src/onboarding/config.rs` — render default `updates` config.
- `crates/lucarne-telegram/src/adapter.rs` — subscribe to system notifications and pass receiver to bot.
- `crates/lucarne-telegram/src/bot.rs` — deliver update notices to entry chat.
- `crates/lucarne-wechat/src/adapter.rs` — subscribe to system notifications and pass receiver to service.
- `crates/lucarne-wechat/src/service.rs` — deliver update notices to configured/known users.
- `README.md` — document update checks, opt-out, manual update command.

---

### Task 1: Build `lucarned-ctl` update core

**Files:**
- Modify: `crates/lucarned-ctl/Cargo.toml`
- Modify: `crates/lucarned-ctl/src/lib.rs`
- Create: `crates/lucarned-ctl/src/updates/mod.rs`
- Create: `crates/lucarned-ctl/src/updates/github.rs`
- Create: `crates/lucarned-ctl/src/updates/version.rs`
- Create: `crates/lucarned-ctl/src/updates/state.rs`
- Create: `crates/lucarned-ctl/src/updates/render.rs`

- [ ] **Step 1: Add feature-gated dependencies**

Add to `crates/lucarned-ctl/Cargo.toml`:

```toml
[features]
default = []
updates = [
    "dep:reqwest",
    "dep:semver",
    "dep:serde",
    "dep:serde_json",
    "dep:tokio",
]

[dependencies]
reqwest = { workspace = true, optional = true }
semver = { version = "1", optional = true }
serde = { workspace = true, optional = true }
serde_json = { workspace = true, optional = true }
tokio = { workspace = true, optional = true }
```

Do not add non-optional dependencies to `lucarned-ctl`.

- [ ] **Step 2: Export updates module behind feature**

Add to `crates/lucarned-ctl/src/lib.rs` near other modules:

```rust
#[cfg(feature = "updates")]
pub mod updates;
```

- [ ] **Step 3: Implement version comparison tests first**

Create `crates/lucarned-ctl/src/updates/version.rs` with tests for:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_single_v_prefix_and_compares_versions() {
        assert_eq!(normalize_tag("v0.2.0"), "0.2.0");
        assert_eq!(normalize_tag("0.2.0"), "0.2.0");
        assert!(is_newer_version("0.1.0", "v0.2.0").unwrap());
        assert!(!is_newer_version("0.2.0", "v0.2.0").unwrap());
        assert!(!is_newer_version("0.3.0", "v0.2.0").unwrap());
    }

    #[test]
    fn invalid_latest_version_is_an_error() {
        assert!(is_newer_version("0.1.0", "nightly").is_err());
    }
}
```

Then implement:

```rust
use semver::Version;

pub fn normalize_tag(tag: &str) -> &str {
    tag.trim().strip_prefix('v').unwrap_or_else(|| tag.trim())
}

pub fn parse_version(value: &str) -> Result<Version, semver::Error> {
    Version::parse(normalize_tag(value))
}

pub fn is_newer_version(current: &str, latest_tag: &str) -> Result<bool, semver::Error> {
    Ok(parse_version(latest_tag)? > parse_version(current)?)
}
```

- [ ] **Step 4: Implement GitHub release parser and checker**

Create `crates/lucarned-ctl/src/updates/github.rs` with:

```rust
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubRelease {
    pub tag_name: String,
    pub name: String,
    pub html_url: String,
    pub body: String,
    pub published_at: Option<String>,
    pub draft: bool,
    pub prerelease: bool,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseJson {
    tag_name: String,
    #[serde(default)]
    name: String,
    html_url: String,
    #[serde(default)]
    body: String,
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}
```

Expose:

```rust
pub fn parse_release_json(raw: &str) -> Result<GithubRelease, serde_json::Error>;
pub async fn fetch_latest_release(
    client: &reqwest::Client,
    repository: &str,
    user_agent: &str,
) -> Result<Option<GithubRelease>, UpdateError>;
```

Rules:
- URL: `https://api.github.com/repos/{repository}/releases/latest`.
- Reject repository strings not shaped as `owner/repo`.
- Add headers `User-Agent` and `Accept: application/vnd.github+json`.
- Non-success HTTP is transient `UpdateError::HttpStatus(status)`.
- If `draft || prerelease`, return `Ok(None)`.
- Read response text through reqwest; if body length exceeds 128 KiB, return `UpdateError::ResponseTooLarge`.

Tests: use `parse_release_json` for normal release, prerelease, draft, missing optional fields.

- [ ] **Step 5: Implement state persistence and throttling**

Create `crates/lucarned-ctl/src/updates/state.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateState {
    pub last_checked_at_unix_secs: Option<u64>,
    pub last_latest_version: Option<String>,
    pub last_latest_url: Option<String>,
    pub last_notified_version: Option<String>,
    pub last_notified_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct UpdateStateStore {
    path: PathBuf,
}
```

Implement:

```rust
impl UpdateStateStore {
    pub fn new(path: PathBuf) -> Self;
    pub fn path(&self) -> &Path;
    pub fn load(&self) -> Result<UpdateState, std::io::Error>;
    pub fn save(&self, state: &UpdateState) -> Result<(), std::io::Error>;
}

pub fn should_notify(
    state: &UpdateState,
    latest_version: &str,
    now_unix_secs: u64,
    remind_interval_secs: u64,
) -> bool;
```

Save atomically: create parent, write `.<file>.tmp-<pid>`, rename.

Tests:
- missing file loads default,
- malformed file returns error from `load`,
- save then load round-trips,
- same version inside interval suppresses,
- same version after interval notifies,
- different version notifies immediately.

- [ ] **Step 6: Implement render helpers**

Create `crates/lucarned-ctl/src/updates/render.rs` with:

```rust
pub const INSTALL_HINT: &str = "macOS/Linux:\n  curl -fsSL https://github.com/tuchg/Lucarne/releases/latest/download/lucarned-installer.sh | sh\n\nWindows PowerShell:\n  irm https://github.com/tuchg/Lucarne/releases/latest/download/lucarned-installer.ps1 | iex\n\nIf installed through a package manager, upgrade with that package manager.";

pub fn truncate_release_body(body: &str, max_chars: usize) -> String;
pub fn render_update_cli(status: &UpdateStatus) -> String;
pub fn render_update_notification(status: &UpdateStatus) -> String;
pub fn render_doctor_message(status: &UpdateStatus) -> (crate::doctor::CheckLevel, String);
```

Tests:
- truncation appends `...(truncated)` when over limit,
- CLI output includes current/latest/release/install hints,
- current status says current,
- notification output includes current/latest/release and truncated changes.

- [ ] **Step 7: Implement public facade and runtime**

Create `crates/lucarned-ctl/src/updates/mod.rs`:

```rust
mod github;
mod render;
mod state;
mod version;

pub use render::{render_update_cli, render_update_notification, INSTALL_HINT};
pub use state::{UpdateState, UpdateStateStore};

#[derive(Debug, Clone)]
pub struct UpdateConfig {
    pub enabled: bool,
    pub notify: bool,
    pub check_interval: std::time::Duration,
    pub remind_interval: std::time::Duration,
    pub repository: String,
    pub startup_delay: std::time::Duration,
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
```

Implement:

```rust
impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            notify: true,
            check_interval: std::time::Duration::from_secs(24 * 60 * 60),
            remind_interval: std::time::Duration::from_secs(24 * 60 * 60),
            repository: "tuchg/Lucarne".to_string(),
            startup_delay: std::time::Duration::from_secs(60),
        }
    }
}

pub async fn check_now(
    client: &reqwest::Client,
    config: &UpdateConfig,
    current_version: &str,
) -> Result<UpdateStatus, UpdateError>;

pub struct UpdateRuntime { /* config, state_store, next_due */ }

impl UpdateRuntime {
    pub fn new(config: UpdateConfig, state_store: UpdateStateStore) -> Self;
    pub fn enabled(&self) -> bool;
    pub async fn next_notice(
        &mut self,
        client: &reqwest::Client,
        current_version: &str,
    ) -> Option<UpdateNotice>;
}
```

`next_notice` must:
- sleep until `next_due`,
- set next due to `now + check_interval`,
- wrap `check_now` in `tokio::time::timeout(Duration::from_secs(10), ...)`,
- load state, update `last_checked_*`,
- if newer and `notify` and `should_notify`, save `last_notified_*` and return notice,
- never panic or return errors; log via `eprintln!` is not acceptable in library. Return `None` on error and let callers log later only if API exposes error. Prefer tracing-free ctl by keeping runtime quiet and tested through state.

- [ ] **Step 8: Verify Task 1**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarned-ctl --features updates
cargo +nightly test -Zbuild-dir-new-layout -p lucarned-ctl --no-default-features
```

Expected: both pass.

- [ ] **Step 9: Commit Task 1**

```bash
git add crates/lucarned-ctl Cargo.lock
git commit -m "feat: add update check core"
```

---

### Task 2: Add `update` CLI and update-aware `doctor`

**Files:**
- Modify: `crates/lucarned-ctl/src/args.rs`
- Modify: `crates/lucarned-ctl/src/lib.rs`
- Modify: `crates/lucarned-ctl/src/doctor.rs`
- Modify: `crates/lucarned/src/main.rs`
- Modify: `crates/lucarned/Cargo.toml`
- Modify: `crates/lucarned/src/onboarding/config.rs`

- [ ] **Step 1: Add parser tests**

In `crates/lucarned-ctl/src/args.rs`, add to `parses_top_level_commands`:

```rust
assert_eq!(parse_words(&["lucarned", "update"]).unwrap(), Command::Update);
```

Add `Update` to `Command` enum and `usage()` after `doctor`.

- [ ] **Step 2: Enable updates feature in daemon**

Change `crates/lucarned/Cargo.toml`:

```toml
lucarned-ctl = { path = "../lucarned-ctl", features = ["updates"] }
```

- [ ] **Step 3: Add async ctl entrypoint**

In `crates/lucarned-ctl/src/lib.rs`, keep existing sync `run` for non-network commands. Add behind feature:

```rust
#[cfg(feature = "updates")]
pub async fn run_async(
    command: Command,
    client: &reqwest::Client,
    update_config: updates::UpdateConfig,
    update_state_path: std::path::PathBuf,
) -> Result<(), String> {
    match command {
        Command::Doctor => doctor::run_doctor_async(client, update_config).await,
        Command::Update => updates::run_update_command(client, update_config).await,
        other => run(other),
    }
}
```

Also make sync `run(Command::Update)` return a clear error when feature is unavailable or route only through async path when feature is enabled.

- [ ] **Step 4: Add doctor async helper**

In `crates/lucarned-ctl/src/doctor.rs`:

```rust
#[cfg(feature = "updates")]
pub async fn run_doctor_async(
    client: &reqwest::Client,
    update_config: crate::updates::UpdateConfig,
) -> Result<(), String> {
    let mut checks = collect_checks()?;
    checks.push(update_check(client, update_config).await);
    print_checks(&checks);
    if checks.iter().any(|check| check.level == CheckLevel::Fail) {
        Err("doctor found critical failures".to_string())
    } else {
        Ok(())
    }
}
```

Refactor current print loop into `fn print_checks(checks: &[Check])` so sync and async share it.

`update_check` behavior:
- if `!update_config.enabled`: `ok: update: checks disabled by config`,
- if current: `ok: update: current version ...`,
- if newer: `warn: update: <version> available: <url>`,
- if error: `warn: update: check failed: <err>`.

- [ ] **Step 5: Add config parsing in `lucarned main`**

In `crates/lucarned/src/main.rs`, add to `DEFAULT_LUCARNED_CONFIG`:

```yaml
updates:
  enabled: true
  notify: true
  check_interval_hours: 24
  remind_interval_hours: 24
  repository: tuchg/Lucarne
```

Add `updates: UpdateFileConfig` to `LucarnedFileConfig` and a helper:

```rust
#[derive(Clone, Debug, Default, Deserialize)]
struct UpdateFileConfig {
    enabled: Option<bool>,
    notify: Option<bool>,
    check_interval_hours: Option<u64>,
    remind_interval_hours: Option<u64>,
    repository: Option<String>,
}

#[cfg(feature = "updates")]
fn update_config_from_file(config: &LucarnedFileConfig) -> lucarned_ctl::updates::UpdateConfig {
    let defaults = lucarned_ctl::updates::UpdateConfig::default();
    lucarned_ctl::updates::UpdateConfig {
        enabled: env_bool("LUCARNED_UPDATES_ENABLED").or(config.updates.enabled).unwrap_or(defaults.enabled),
        notify: env_bool("LUCARNED_UPDATES_NOTIFY").or(config.updates.notify).unwrap_or(defaults.notify),
        check_interval: env_hours("LUCARNED_UPDATES_CHECK_INTERVAL_HOURS").or(config.updates.check_interval_hours).map(hours_to_duration).unwrap_or(defaults.check_interval),
        remind_interval: env_hours("LUCARNED_UPDATES_REMIND_INTERVAL_HOURS").or(config.updates.remind_interval_hours).map(hours_to_duration).unwrap_or(defaults.remind_interval),
        repository: std::env::var("LUCARNED_UPDATES_REPOSITORY").ok().or_else(|| config.updates.repository.clone()).unwrap_or(defaults.repository),
        startup_delay: defaults.startup_delay,
    }
}
```

Clamp hour values below 1 to 1 hour.

- [ ] **Step 6: Route CLI through shared client**

In `main()`, for `Command::Doctor` and `Command::Update`, load dotenv/config, create `default_http_client()?`, and call `lucarned_ctl::run_async(...)`. Other sync commands keep current path.

Do not create a client inside `lucarned-ctl`.

- [ ] **Step 7: Render update config from `lucarned init`**

In `crates/lucarned/src/onboarding/config.rs`, add an `updates: UpdatesYaml` field to `ConfigYaml` and render defaults:

```rust
#[derive(Serialize)]
struct UpdatesYaml {
    enabled: bool,
    notify: bool,
    check_interval_hours: u8,
    remind_interval_hours: u8,
    repository: &'static str,
}
```

Assert in existing render tests that YAML contains `updates:`, `check_interval_hours: 24`, and `repository: tuchg/Lucarne`.

- [ ] **Step 8: Verify Task 2**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarned-ctl --features updates
cargo +nightly test -Zbuild-dir-new-layout -p lucarned --quiet
```

Expected: pass.

- [ ] **Step 9: Commit Task 2**

```bash
git add crates/lucarned-ctl crates/lucarned Cargo.lock
git commit -m "feat: add update status commands"
```

---

### Task 3: Add generic system notification bus and daemon update driver

**Files:**
- Modify: `crates/lucarne-adapter/src/lib.rs`
- Modify: `crates/lucarned/src/main.rs`

- [ ] **Step 1: Add adapter bus types and tests**

In `crates/lucarne-adapter/src/lib.rs`, import broadcast:

```rust
use tokio::sync::{broadcast, mpsc, watch};
```

Add near `GlobalConfigUpdate`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemNotification {
    UpdateAvailable(SystemUpdateNotification),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemUpdateNotification {
    pub current_version: String,
    pub latest_version: String,
    pub release_name: String,
    pub release_url: String,
    pub published_at: Option<String>,
    pub body_markdown: String,
    pub install_hint: String,
}

#[derive(Clone)]
pub struct SystemNotificationBus {
    tx: broadcast::Sender<SystemNotification>,
}

pub struct SystemNotificationReceiver {
    rx: broadcast::Receiver<SystemNotification>,
}
```

Implement:

```rust
impl SystemNotificationBus {
    pub fn new(capacity: usize) -> Self;
    pub fn subscribe(&self) -> SystemNotificationReceiver;
    pub fn send(&self, notification: SystemNotification) -> usize;
}

impl SystemNotificationReceiver {
    pub async fn recv(&mut self) -> Result<SystemNotification, broadcast::error::RecvError>;
}
```

`send` returns `0` if there are no receivers or send fails.

Add field to `AdapterContext`:

```rust
pub system_notifications: SystemNotificationBus,
```

Update tests/context constructors with `SystemNotificationBus::new(16)`.

- [ ] **Step 2: Create and pass bus in daemon**

In `run_daemon()`, create bus before supervising adapters:

```rust
let system_notifications = lucarne_adapter::SystemNotificationBus::new(32);
```

Pass `system_notifications.clone()` into `AdapterContext`.

- [ ] **Step 3: Drive update runtime from main wait loop**

Create update runtime in `run_daemon()` after file config is loaded and shared client exists:

```rust
let update_config = update_config_from_file(&file_config);
let update_state_path = lucarned_ctl::paths::default_config_dir()?.join("update-state.json");
let mut update_runtime = lucarned_ctl::updates::UpdateRuntime::new(
    update_config,
    lucarned_ctl::updates::UpdateStateStore::new(update_state_path),
);
```

Move `default_http_client()` so one client is created once before both adapter and update usage.

Update `wait_for_shutdown_or_adapter_fatal` to loop and include:

```rust
notice = update_runtime.next_notice(&http_client, env!("CARGO_PKG_VERSION")), if update_runtime.enabled() => {
    if let Some(notice) = notice {
        let notification = lucarne_adapter::SystemNotification::UpdateAvailable(
            lucarne_adapter::SystemUpdateNotification {
                current_version: notice.current_version,
                latest_version: notice.latest_version,
                release_name: notice.release_name,
                release_url: notice.release_url,
                published_at: notice.published_at,
                body_markdown: notice.body_markdown,
                install_hint: notice.install_hint,
            },
        );
        let receivers = system_notifications.send(notification);
        tracing::info!(receivers, "sent update system notification");
    }
}
```

Do not use `tokio::spawn` for update checking.

- [ ] **Step 4: Verify Task 3**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-adapter
cargo +nightly test -Zbuild-dir-new-layout -p lucarned --quiet
```

Expected: pass.

- [ ] **Step 5: Commit Task 3**

```bash
git add crates/lucarne-adapter crates/lucarned Cargo.lock
git commit -m "feat: drive update notifications from daemon"
```

---

### Task 4: Deliver system notifications through Telegram and WeChat

**Files:**
- Modify: `crates/lucarne-telegram/src/adapter.rs`
- Modify: `crates/lucarne-telegram/src/bot.rs`
- Modify: `crates/lucarne-wechat/src/adapter.rs`
- Modify: `crates/lucarne-wechat/src/service.rs`

- [ ] **Step 1: Telegram subscribes in adapter spawn**

In `TelegramAdapterPlugin::spawn`, call:

```rust
let system_notifications = ctx.system_notifications.subscribe();
```

Pass receiver into `run_telegram_adapter_with_client_and_global_config_persistence`. Preserve existing public helper functions by creating a fresh bus and subscribing to it for callers that do not have daemon context.

- [ ] **Step 2: Telegram bot handles notices in existing loop**

Refactor `Bot::run` so it owns `mut system_notifications: SystemNotificationReceiver` and selects over channel events and system notifications in the same long-lived loop.

Add:

```rust
async fn handle_system_notification(
    &self,
    notification: lucarne_adapter::SystemNotification,
) -> Result<(), String> {
    match notification {
        lucarne_adapter::SystemNotification::UpdateAvailable(update) => {
            let msg = OutgoingMessage::markdown(update.body_markdown);
            send_with_fallback(&*self.channel, &self.entry, msg, "lucarned")
                .await
                .map(|_| ())
                .map_err(|err| err.to_string())
        }
    }
}
```

Use entry chat/root handle, not provider session binding.

- [ ] **Step 3: WeChat subscribes in adapter spawn**

In `WechatAdapterPlugin::spawn`, subscribe before spawning task and pass receiver into the service run path. Avoid adding receiver to `WechatServiceOptions` if it would break `Clone`; pass as a separate argument to `run_until_shutdown` or add a new method.

- [ ] **Step 4: WeChat service handles notices in existing loop**

In `WechatNotificationService::run_until_shutdown`, add a select branch for `system_notifications.recv()`.

Add:

```rust
async fn deliver_system_notification(
    &self,
    notification: lucarne_adapter::SystemNotification,
) -> Result<(), WechatError> {
    match notification {
        lucarne_adapter::SystemNotification::UpdateAvailable(update) => {
            self.deliver_update_notification(&update.body_markdown).await
        }
    }
}
```

`deliver_update_notification`:
- calls `self.notification_users()`;
- logs and returns `Ok(())` if empty;
- for each user, calls `transport.context_for_user(&user_id).await?`;
- sends `transport.send(&context, body).await`;
- on rate limit, records rate limit and skips/queues only if a small system pending queue is added;
- does not call `bind_receipt`, does not require `ProviderSessionId`, and does not use `WorkspaceId`.

Keep it simple: log per-user failures and continue; return `Ok(())` unless all sends fail due a shared fatal transport error already represented by `WechatError`.

- [ ] **Step 5: Add adapter delivery tests**

Telegram test: use fake/recording channel or existing bot test helper to send `SystemNotification::UpdateAvailable` and assert one outbound markdown message to entry chat.

WeChat test: use existing `FakeTransport`, initialize service with `initial_user_ids = vec!["user-1".into()]`, deliver a system notification, and assert visible sent text contains `Lucarne update available` and no provider/session binding was created.

- [ ] **Step 6: Verify Task 4**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-telegram --quiet
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-wechat --quiet
cargo +nightly test -Zbuild-dir-new-layout -p lucarned --quiet
```

Expected: pass.

- [ ] **Step 7: Commit Task 4**

```bash
git add crates/lucarne-telegram crates/lucarne-wechat crates/lucarne-adapter crates/lucarned Cargo.lock
git commit -m "feat: notify chats about updates"
```

---

### Task 5: Document and verify update awareness end-to-end

**Files:**
- Modify: `README.md`
- Modify tests as needed only for deterministic behavior.

- [ ] **Step 1: Document update behavior**

Add README section near install/doctor docs:

```markdown
### Update checks

`lucarned` checks GitHub Releases once every 24 hours by default. When a newer stable release is available, it sends one Telegram/WeChat notification per version per 24 hours. Lucarne does not auto-update or replace the running daemon.

Disable automatic checks:

```yaml
updates:
  enabled: false
```

Disable chat notifications while keeping checks:

```yaml
updates:
  notify: false
```

Check manually:

```sh
lucarned update
```

`lucarned update` prints the latest version, release notes, release URL, and installer/package-manager commands. It does not modify files.
```

- [ ] **Step 2: Full verification**

Run:

```bash
cargo +nightly check -Zbuild-dir-new-layout -p lucarned
cargo +nightly test -Zbuild-dir-new-layout -p lucarned-ctl --features updates
cargo +nightly test -Zbuild-dir-new-layout -p lucarned-ctl --no-default-features
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-adapter
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-telegram
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-wechat
git diff --check
```

Expected: all pass.

- [ ] **Step 3: Manual CLI smoke**

Run against live GitHub if network is available:

```bash
cargo +nightly run -Zbuild-dir-new-layout -p lucarned -- update
cargo +nightly run -Zbuild-dir-new-layout -p lucarned -- doctor
```

Expected:
- `update` prints current/latest/release/install hint.
- `doctor` includes `ok: update:` or `warn: update:` but does not fail solely due network.

- [ ] **Step 4: Commit Task 5**

```bash
git add README.md crates Cargo.lock
git commit -m "docs: document update awareness"
```

---

## Self-Review Checklist

- Spec coverage: update checks, opt-out, doctor, update command, notifications, 24h throttle, stable releases only, shared client, no update task, no self-update are covered.
- No provider/common/history layer changes are required.
- The only platform-specific installer text is rendered as user-facing update instructions; no OS runtime branching is added to common layers beyond existing CLI display.
- Plan tasks are sequential because later daemon/adapters depend on core types and parser changes.
