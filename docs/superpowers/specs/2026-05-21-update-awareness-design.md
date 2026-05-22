# Update Awareness Design

## Goal

Make Lucarne users aware when a newer GitHub Release is available, without self-updating or replacing a running `lucarned` process.

The feature should:

- check GitHub Releases periodically when the daemon is running,
- notify users through enabled Telegram and WeChat channels,
- show update status from `lucarned doctor`,
- add `lucarned update` as an explicit manual status/help command,
- let users disable automatic checks in config,
- remind at most once per version per 24 hours by default,
- reuse the existing shared HTTP client path,
- avoid spawning a dedicated update-check task.

This work should be implemented in a separate PR from the Linux/install support chain. The branch can start from `feature/linux-support` while PR #5 is still stacked, then retarget after the base PRs merge.

## Non-goals

This design does not implement automatic binary replacement.

Out of scope:

- downloading and swapping `lucarned`,
- restarting systemd/LaunchAgent/Task Scheduler services,
- package-manager mutation,
- background self-update helpers,
- GitHub asset signature verification for downloaded binaries,
- support for prerelease notification,
- external hosted notification services.

`lucarned update` is intentionally informational. It prints the latest version, changelog summary, release URL, and the installer/package-manager command the user can run manually.

## Default Behavior

Defaults are user-visible and conservative:

```yaml
updates:
  enabled: true
  notify: true
  check_interval_hours: 24
  remind_interval_hours: 24
  repository: tuchg/Lucarne
```

Semantics:

- `enabled: true` permits daemon automatic checks and `doctor` update checks.
- `enabled: false` disables daemon automatic checks and `doctor` network checks.
- `notify: false` allows automatic checks but suppresses chat notifications.
- `lucarned update` is explicit user intent and performs a manual check even if automatic checks are disabled. Its output should mention when automatic checks are disabled.
- Only stable GitHub Releases are considered. Drafts and prereleases are ignored.
- The daemon checks every 24 hours by default.
- The daemon waits 30-60 seconds after startup before the first automatic check so startup and adapter initialization stay fast.
- A newer version is notified at most once per version per `remind_interval_hours`, default 24 hours.

Environment overrides should mirror config for operational use:

- `LUCARNED_UPDATES_ENABLED`
- `LUCARNED_UPDATES_NOTIFY`
- `LUCARNED_UPDATES_CHECK_INTERVAL_HOURS`
- `LUCARNED_UPDATES_REMIND_INTERVAL_HOURS`
- `LUCARNED_UPDATES_REPOSITORY`

Invalid intervals are rejected or clamped to a safe minimum of 1 hour for daemon scheduling. Manual `lucarned update` is not interval-gated.

## Crate Boundaries

### `lucarned-ctl`

Update-checking logic belongs in `lucarned-ctl`, not in `lucarned` main, because `doctor`, `paths`, `autostart`, and the new `update` command are CLI/control-plane concerns already owned by this crate.

`lucarned-ctl` currently has no dependencies. That property should remain true by default. Update support should be feature-gated:

```toml
[features]
default = []
updates = [
  "dep:reqwest",
  "dep:serde",
  "dep:serde_json",
  "dep:serde_yaml",
  "dep:tokio",
  "dep:semver",
]
```

`lucarned` enables the feature:

```toml
lucarned-ctl = { path = "../lucarned-ctl", features = ["updates"] }
```

The update module layout should stay narrow and testable:

```text
crates/lucarned-ctl/src/updates/
  mod.rs          # public facade, config, runtime, status structs
  github.rs       # GitHub release JSON fetch/parse
  version.rs      # version normalization and semver comparison
  state.rs        # last check / last notification persistence
  render.rs       # doctor, update command, and notification text
```

`lucarned-ctl` must not depend on Telegram, WeChat, provider, history, or daemon internals.

### `lucarned`

`lucarned` remains the composition root:

- loads config,
- creates the shared HTTP client,
- creates the update runtime,
- drives update checks from the existing daemon main wait loop,
- forwards update notices to adapters through a generic system-notification bus.

`lucarned` must not own GitHub parsing, version comparison, changelog rendering, or notification throttling.

### `lucarne-adapter`

A generic notification bus belongs in `lucarne-adapter` because enabled adapters receive it through `AdapterContext`.

Proposed shape:

```rust
#[derive(Debug, Clone)]
pub enum SystemNotification {
    UpdateAvailable(SystemUpdateNotification),
}

#[derive(Debug, Clone)]
pub struct SystemUpdateNotification {
    pub current_version: String,
    pub latest_version: String,
    pub release_name: String,
    pub release_url: String,
    pub published_at: Option<String>,
    pub body_markdown: String,
    pub install_hint: String,
}
```

The exact type can be adjusted during implementation, but it should contain only channel-agnostic strings and metadata. Platform formatting stays in Telegram and WeChat crates.

`AdapterContext` should expose a cloneable notification source, likely a wrapper around `tokio::sync::broadcast::Sender<SystemNotification>` with a `subscribe()` method. Adapters subscribe when they spawn. The daemon sends notices through the same bus.

### Telegram and WeChat crates

Telegram and WeChat consume generic `SystemNotification` values and render platform-specific messages.

- Telegram sends update notifications to the configured entry chat.
- WeChat sends update notifications to configured and remembered notification users.
- Delivery failure is logged and does not fail the daemon.
- Channel-specific markdown escaping, splitting, rate limits, and recipient lookup stay inside each adapter crate.

Common update code must not know Telegram chat IDs, WeChat user IDs, channel-specific rate limits, or platform markdown rules.

## HTTP Client Ownership

No update-check code creates a new `reqwest::Client` in daemon mode.

The existing shared client helper remains the source of clients:

```rust
let http_client = lucarne_adapter::default_http_client()?;
```

Daemon mode:

- `lucarned` creates this client once.
- The adapter supervisor receives `http_client.clone()`.
- `UpdateRuntime` receives `http_client.clone()`.
- `lucarned-ctl::updates` APIs borrow or receive a clone passed by `lucarned`; they never call `reqwest::Client::new()` or `reqwest::Client::builder()`.

CLI mode:

- `lucarned main` creates the same default client for async `doctor` and `update` commands.
- `lucarned main` passes `&reqwest::Client` into `lucarned-ctl`.
- `lucarned-ctl` stays client-agnostic.

This keeps proxy/TLS/timeout behavior consistent with existing Telegram/WeChat network code.

## Daemon Runtime Without a Dedicated Task

The update checker should be an independent module, but it should not own a long-lived task.

`lucarned` should drive it from the existing daemon wait loop:

```rust
let mut updates = updates::UpdateRuntime::new(update_config, update_state, http_client.clone());

loop {
    tokio::select! {
        fatal = adapter_supervisor.next_fatal() => {
            // existing fatal handling
        }
        signal = tokio::signal::ctrl_c() => {
            // existing shutdown handling
        }
        notice = updates.next_notice(), if updates.enabled() => {
            if let Some(notice) = notice {
                let _ = system_notifications.send(notice.into_system_notification());
            }
        }
    }
}
```

`UpdateRuntime::next_notice()` owns scheduling and throttling:

- sleeps until the next due check,
- performs a timeout-bound GitHub check,
- compares current and latest versions,
- consults persisted notification state,
- records last check and last notification timestamps,
- returns `Some(UpdateNotice)` only when a chat notification should be sent.

No `tokio::spawn` should be introduced for update checking. The update future is polled by the main daemon loop and dropped naturally on shutdown or fatal error.

Network checks should be bounded so Ctrl-C and adapter-fatal handling are not delayed for long. The check itself should be wrapped in a short timeout, recommended 10 seconds, even though the shared client has a broader default request timeout.

## GitHub Release Check

The checker queries GitHub's latest release endpoint for the configured repository:

```text
GET https://api.github.com/repos/{owner}/{repo}/releases/latest
```

Request requirements:

- set a stable `User-Agent`, for example `lucarned/<version>`;
- set `Accept: application/vnd.github+json`;
- use the injected shared `reqwest::Client`;
- treat HTTP non-2xx as a transient warning;
- cap accepted response size to avoid unexpectedly large memory use.

The parsed fields needed are:

- `tag_name`, e.g. `v0.2.0`,
- `name`,
- `html_url`,
- `body`,
- `published_at`,
- `draft`,
- `prerelease`.

The checker ignores releases where `draft` or `prerelease` is true. The `/latest` endpoint should already do this for normal public releases, but the filter should remain explicit for correctness and tests.

Version comparison:

- current version comes from `env!("CARGO_PKG_VERSION")` in `lucarned-ctl` or is passed in by `lucarned`,
- latest version is parsed from `tag_name`, with one leading `v` stripped,
- compare using semver,
- if latest cannot be parsed, report a warning and do not notify,
- notify only when `latest > current`,
- equal or older releases render as current.

## State Persistence

Reminder throttling must survive daemon restart. Use a small state file under the existing lucarned config directory instead of adding a SQLite dependency to `lucarned-ctl`.

Recommended path:

```text
~/.lucarned/update-state.json
```

On Windows this follows `paths::default_config_dir()`:

```text
%LOCALAPPDATA%\lucarned\update-state.json
```

State shape:

```json
{
  "last_checked_at_unix_secs": 1780000000,
  "last_latest_version": "0.2.0",
  "last_latest_url": "https://github.com/tuchg/Lucarne/releases/tag/v0.2.0",
  "last_notified_version": "0.2.0",
  "last_notified_at_unix_secs": 1780000000
}
```

Persistence rules:

- create parent directory if needed,
- write atomically through a temporary file and rename,
- ignore unreadable or malformed state with a warning and recreate it,
- never store secrets,
- do not fail daemon startup because update state cannot be read or written,
- if state cannot be written, still allow a notification for the current run but log that throttling may not persist.

Throttling rule:

- If `last_notified_version == latest_version` and `now - last_notified_at < remind_interval`, do not notify.
- If the version is different, notify immediately after a successful check and persist the new version/time.
- If `notify: false`, do not write `last_notified_*`; still update `last_checked_*` and `last_latest_*`.

## CLI Behavior

### `lucarned update`

Add a new top-level command:

```text
lucarned update
```

Output when current:

```text
lucarned 0.1.0 is current
latest: 0.1.0
release: https://github.com/tuchg/Lucarne/releases/tag/v0.1.0
```

Output when new version exists:

```text
current: 0.1.0
latest: 0.2.0
release: https://github.com/tuchg/Lucarne/releases/tag/v0.2.0

Changes:
- ... truncated release notes ...

Update manually:
  macOS/Linux:
    curl -fsSL https://github.com/tuchg/Lucarne/releases/latest/download/lucarned-installer.sh | sh

  Windows PowerShell:
    irm https://github.com/tuchg/Lucarne/releases/latest/download/lucarned-installer.ps1 | iex

If installed through a package manager, use that package manager instead:
  Homebrew: brew upgrade lucarned
  AUR: yay -Syu lucarned-bin
  winget/scoop/choco: use the matching package manager upgrade command
```

`lucarned update` does not update files. It exits nonzero only for hard local errors such as invalid config path arguments. GitHub/network errors should print a clear error and exit nonzero for manual CLI use.

### `lucarned doctor`

`doctor` appends an update check when automatic checks are enabled.

Possible lines:

```text
ok: update: checks disabled by config
ok: update: current version 0.1.0
warn: update: 0.2.0 available: https://github.com/tuchg/Lucarne/releases/tag/v0.2.0
warn: update: check failed: GitHub returned 403 rate limited
```

Network failure in `doctor` should be a warning, not a fatal failure. Optional update-check failure must not make `doctor` exit with failure unless an existing critical check already fails.

If config is missing, default update settings apply and doctor may still check. The existing config-missing warning remains separate.

## Notification Content

Chat notification content should be concise, useful, and bounded.

Recommended generic markdown before channel rendering:

```text
Lucarne update available: 0.2.0

Current: 0.1.0
Release: https://github.com/tuchg/Lucarne/releases/tag/v0.2.0

Changes:
- first release note line
- second release note line

Manual update:
macOS/Linux: curl -fsSL https://github.com/tuchg/Lucarne/releases/latest/download/lucarned-installer.sh | sh
Windows PowerShell: irm https://github.com/tuchg/Lucarne/releases/latest/download/lucarned-installer.ps1 | iex

If installed through a package manager, upgrade with that package manager.
```

Rendering rules:

- truncate release notes to about 1200 characters before platform-specific escaping,
- preserve the release URL,
- include current and latest version,
- do not include secrets or environment details,
- avoid mentioning unsupported automatic self-update,
- set Telegram `silent` to false so users notice the update,
- let WeChat use its existing notification/rate-limit machinery.

## Adapter Delivery

### Telegram

The Telegram adapter already has a long-lived bot loop and a core-event watcher. Do not add a new update-specific task. Reuse existing loop structure by selecting over a system-notification subscription alongside the current event sources.

Delivery:

- send to the configured entry chat,
- use the existing `OutgoingMessage::markdown` path,
- use existing fallback/splitting helpers,
- log failures without stopping the adapter.

### WeChat

The WeChat service already has a long-lived select loop for incoming messages, core events, pending retries, and rate-limit interactions. Add a system-notification receiver branch to that loop rather than spawning a new task.

Delivery:

- send to `notify_user_ids` plus remembered users, matching existing notification recipient behavior,
- use existing transport/rate-limit handling,
- if no known users exist, log and skip,
- do not bind update notices to provider sessions,
- do not suppress update notices based on per-workspace direct conversation suppression, because updates are daemon-level notifications rather than agent-session notifications.

## Config Rendering and Init

`lucarned init` should render the `updates` block with defaults so users see the feature and can disable it.

Generated config should include:

```yaml
updates:
  enabled: true
  notify: true
  check_interval_hours: 24
  remind_interval_hours: 24
  repository: tuchg/Lucarne
```

Existing config parsing should default missing `updates` to enabled. Existing users therefore begin receiving update awareness after upgrading, unless they add:

```yaml
updates:
  enabled: false
```

## Error Handling

Daemon automatic check errors are non-fatal:

- DNS/connect/TLS failure: warn log, no notification.
- GitHub 403/429 rate limit: warn log, no notification.
- GitHub 404 repository missing: warn log, no notification.
- Invalid JSON: warn log, no notification.
- Invalid semver tag: warn log, no notification.
- State file read/write failure: warn log; continue.
- Notification delivery failure: channel-specific warn log; continue.

Manual `lucarned update` should report network and parse errors to stderr and exit nonzero because the user explicitly requested a check.

`doctor` should convert network and parse errors into `warn: update: ...` and keep the existing doctor success/failure policy.

## Privacy and Network Behavior

Automatic checks contact GitHub once per configured interval. The request reveals normal HTTP metadata such as IP address, User-Agent, and target repository. No local config, usernames, agent sessions, chat IDs, or tokens are sent.

Users can disable automatic checks:

```yaml
updates:
  enabled: false
```

Manual `lucarned update` still performs a check because running the command is explicit user intent. The command should mention when automatic checks are disabled to avoid confusion.

## Testing Strategy

### `lucarned-ctl` unit tests

- config defaults enable update checks and notifications,
- config can disable checks,
- env overrides config,
- invalid interval is rejected or clamped consistently,
- GitHub latest JSON parses required fields,
- draft and prerelease responses are ignored,
- semver comparison handles `v0.2.0`, `0.2.0`, equal, older, and invalid tags,
- changelog truncation keeps release URL and version fields,
- state file read/write is atomic and tolerant of malformed input,
- same version inside 24 hours does not notify,
- same version after 24 hours does notify,
- new version notifies immediately even if a different version was notified recently,
- `render_update_cli` prints installer commands without claiming to self-update.

### `lucarned` tests

- `lucarned update` parses as a top-level command,
- async CLI path passes an injected client to ctl update APIs,
- daemon constructs one shared client and clones it into adapters and updates,
- automatic update checks are not started when `updates.enabled=false`,
- update runtime is driven from the main wait loop without an update-specific spawn.

### Adapter tests

- Telegram receives `SystemNotification::UpdateAvailable` and sends one message to entry chat,
- Telegram uses existing markdown/fallback path,
- WeChat sends update notices to configured/known notification users,
- WeChat skips cleanly when no notification users exist,
- adapter notification failures do not become fatal adapter errors.

### Integration and verification

Run existing verification commands after implementation:

```sh
cargo +nightly check -Zbuild-dir-new-layout -p lucarned
cargo +nightly test -Zbuild-dir-new-layout -p lucarned-ctl
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-telegram
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-wechat
```

Release smoke should verify:

```sh
lucarned --version
lucarned update
lucarned doctor
```

Network-dependent tests should use mocked HTTP responses or local test servers. They should not depend on live GitHub availability in normal CI.

## Acceptance Criteria

- `lucarned update` reports latest release information and manual update commands.
- `lucarned doctor` reports update status without making network failure fatal.
- daemon automatic checks are enabled by default and configurable off.
- daemon checks stable GitHub Releases every 24 hours by default.
- daemon sends at most one notification per version per 24 hours by default.
- daemon does not spawn a dedicated update task.
- update checks use the existing `lucarne_adapter::default_http_client()` client supplied by `lucarned`.
- `lucarned-ctl` stays dependency-free without the `updates` feature.
- provider/common/history layers do not gain Telegram, WeChat, GitHub, or OS-specific update branching.
- Telegram and WeChat notification routing remains adapter-owned.
- no automatic binary replacement or service restart is introduced.
