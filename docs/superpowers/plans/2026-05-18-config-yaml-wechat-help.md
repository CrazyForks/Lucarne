# Config YAML Sync and WeChat Help Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Sync global `/config` bypass/notifications to `lucarned.yaml` and add discoverable WeChat help.

**Architecture:** Keep YAML file ownership in daemon/adapter layer. Add optional adapter persistence hook, wire it through Telegram/WeChat adapters, and let command handlers persist global config before mutating core. Add daemon YAML parsing/rendering for `config.global` and startup application.

**Tech Stack:** Rust nightly, serde_yaml, lucarne-adapter traits, lucarned config loader, Telegram/WeChat command handlers, cargo +nightly test -Zbuild-dir-new-layout.

---

### Task 1: Add shared global config persistence hook

**Files:**
- Modify: `crates/lucarne-adapter/src/lib.rs`

- [ ] Add `GlobalConfigUpdate`, `GlobalConfigPersistence`, and `AdapterContext.global_config_persistence`.
- [ ] Update tests/constructors that instantiate `AdapterContext` to set `None`.
- [ ] Run: `cargo +nightly test -Zbuild-dir-new-layout -p lucarne-adapter`.

### Task 2: Add lucarned YAML config read/write/apply

**Files:**
- Modify: `crates/lucarned/src/main.rs`
- Modify: `crates/lucarned/src/onboarding/config.rs`
- Modify: `examples/lucarned.yaml`

- [ ] Add `config.global.bypass` and `config.global.notifications` to default/onboarding/example YAML.
- [ ] Parse optional `LucarnedFileConfig.config.global` fields.
- [ ] Add YAML-backed `GlobalConfigPersistence` implementation that upserts `config.global` and uses backup+atomic write.
- [ ] After opening core, apply explicit YAML globals to core setters.
- [ ] Pass persistence hook into `AdapterContext`.
- [ ] Unit-test parsing, default rendering, YAML upsert, and startup apply.
- [ ] Run: `cargo +nightly test -Zbuild-dir-new-layout -p lucarned`.

### Task 3: Wire Telegram global config writes

**Files:**
- Modify: `crates/lucarne-telegram/src/adapter.rs`
- Modify: `crates/lucarne-telegram/src/bot.rs`
- Modify: `crates/lucarne-telegram/tests/topic_routing_integration.rs`

- [ ] Add optional persistence hook to Telegram runtime/bot construction.
- [ ] For global bypass/notifications updates, persist YAML update before core setter when hook exists.
- [ ] Keep workspace/session updates SQLite-only.
- [ ] Add integration/unit coverage proving global update invokes persistence.
- [ ] Run: `cargo +nightly test -Zbuild-dir-new-layout -p lucarne-telegram config`.

### Task 4: Wire WeChat global config writes and help

**Files:**
- Modify: `crates/lucarne-wechat/src/adapter.rs`
- Modify: `crates/lucarne-wechat/src/service.rs`

- [ ] Add optional persistence hook to WeChat service options.
- [ ] For global config updates, persist YAML update before core setter when hook exists.
- [ ] Add `/help` slash command.
- [ ] Append help text to no-quote default reply.
- [ ] Test `/help`, no-quote help, and persistence hook on `/config global notifications off`.
- [ ] Run: `cargo +nightly test -Zbuild-dir-new-layout -p lucarne-wechat config help`.

### Task 5: Full verification

**Files:**
- All changed files

- [ ] Run: `cargo +nightly test -Zbuild-dir-new-layout -p lucarned -p lucarne-adapter -p lucarne-telegram -p lucarne-wechat`.
- [ ] Run targeted `cargo +nightly check -Zbuild-dir-new-layout` if needed.
- [ ] Update docs if command text changed.
- [ ] Commit implementation.
