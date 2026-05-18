# Config YAML Sync and WeChat Help Design

## Context

`/config` currently mutates `LucarneCore` system settings. Those settings persist in `state.sqlite3` via control-plane `system_settings`, but they do not appear in `lucarned.yaml`. The daemon YAML file only configures startup concerns such as agents, state path, logging, health, turn/session timeouts, and channel credentials.

WeChat also has `/config`, `/status`, and `/kill`, but no `/help`. When a user sends a normal message without quoting an agent notification, WeChat replies only with `Reply to an agent notification to continue that session.`, which gives too little guidance.

## Goals

- Make global `/config` changes visible in `lucarned.yaml`.
- Sync only the global `bypass` and `notifications` settings.
- Keep workspace and session overrides in SQLite only.
- Let manual YAML edits override saved global settings on daemon restart.
- Add WeChat `/help` and append help text to the no-quote guidance reply.
- Keep YAML ownership in the daemon/adapter boundary, not in `LucarneCore`.

## Non-goals

- Do not serialize workspace path overrides into YAML.
- Do not serialize provider session overrides into YAML.
- Do not teach `lucarne` core about config file paths or YAML mutation.
- Do not add provider-specific behavior.
- Do not guarantee comment-preserving YAML rewrites.

## YAML schema

Add an optional runtime config section:

```yaml
config:
  global:
    bypass: false
    notifications: true
```

Meaning:

- `config.global.bypass` maps to `SystemSettings.session.force_bypass_permissions`.
- `config.global.notifications` maps to `SystemSettings.notifications.enabled`.
- Missing fields mean "do not override existing SQLite value".
- New default/onboarding YAML files should include both fields with current defaults.

## Startup behavior

`lucarned` reads `lucarned.yaml` before opening adapters. After opening `LucarneCore`, it applies explicitly configured `config.global` values to core:

1. Load SQLite-backed control-plane state as today.
2. If `config.global.bypass` exists, set core global bypass to that value.
3. If `config.global.notifications` exists, set core global notifications to that value.
4. Persist those applied values back to SQLite through existing core setters.

This makes YAML the source of truth only when the YAML field is present. Existing installs without the new section keep their SQLite settings unchanged until a user edits YAML or runs `/config global ...`.

## Command write behavior

Global config commands in Telegram and WeChat should use a shared adapter-layer persistence hook:

- `/config global bypass on|off`
- `/config global notifications on|off`

Flow under `lucarned`:

1. Read current global settings from core.
2. Build the next global config with the requested field changed and the other field filled from current core state.
3. Write `config.global.bypass` and `config.global.notifications` to `lucarned.yaml`.
4. Apply the requested change to core/SQLite with the existing setter.
5. Render the updated effective config response.

If YAML write fails, return an error and do not mutate core. If the later core mutation fails, return an error; the YAML already contains the intended value and will apply on restart.

When adapters run outside `lucarned` and no YAML persistence hook is configured, `/config global ...` keeps the current behavior and only mutates core/SQLite.

## Component boundaries

`LucarneCore` remains file-agnostic. It exposes and mutates typed settings only.

`lucarne-adapter` should expose a small optional shared contract, for example:

```rust
pub struct GlobalConfigUpdate {
    pub bypass: bool,
    pub notifications: bool,
}

pub trait GlobalConfigPersistence: Send + Sync {
    fn persist_global_config(&self, update: GlobalConfigUpdate) -> AdapterResult<()>;
}
```

`AdapterContext` carries `Option<Arc<dyn GlobalConfigPersistence>>`. `lucarned` provides a YAML-backed implementation when it has a config path. Telegram and WeChat command handlers receive the optional hook through their adapter startup path.

The YAML-backed implementation belongs in `lucarned` because it owns config path resolution and daemon config file semantics. It should:

- Read current YAML as `serde_yaml::Value`.
- Create missing `config` / `global` mappings.
- Set `bypass` and `notifications` scalars.
- Write atomically with backup, reusing the existing onboarding backup/write pattern or equivalent shared helper.
- Preserve unknown keys and channel credentials as values, while accepting that comments/formatting may not survive reserialization.

## WeChat help behavior

Add `/help` to WeChat slash commands.

Help text should cover:

```text
commands
/help — show this help
/config — show global config
/config global bypass on|off — toggle global permission bypass
/config global notifications on|off — toggle global notifications
/status — show global status, or quoted workspace status
/kill all|<session_id:pid> — kill agent processes

Reply to an agent notification to continue that session.
```

When a WeChat user sends a non-command message without quoting a notification, reply with:

```text
Reply to an agent notification to continue that session.

commands
...
```

Stale quote errors can stay concise as `That notification is no longer routable.`.

## Testing

- Unit-test YAML default rendering includes `config.global.bypass: false` and `config.global.notifications: true`.
- Unit-test config parsing extracts optional global fields.
- Unit-test startup application: YAML explicit values override SQLite-loaded globals; missing values do not.
- Unit-test YAML persistence inserts or updates `config.global` while preserving unrelated YAML values.
- Telegram integration test: `/config global bypass on` updates core and writes YAML.
- WeChat unit test: `/config global notifications off` updates core and writes YAML when a persistence hook is present.
- WeChat unit test: `/help` replies with command help.
- WeChat unit test: no-quote normal message includes the quote guidance plus help text.

## Acceptance criteria

- After `Telegram /config global bypass on` under `lucarned`, `lucarned.yaml` contains `config.global.bypass: true`.
- After `WeChat /config global notifications off` under `lucarned`, `lucarned.yaml` contains `config.global.notifications: false`.
- Workspace/session `/config` changes never write YAML.
- Existing YAML files without `config.global` keep existing SQLite globals on restart.
- Manual YAML global values apply on restart.
- WeChat `/help` and no-quote guidance make valid commands discoverable.
