# WeChat Rate Window Config Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose WeChat retry and outbound interaction-window rate-limit knobs through YAML/env and default config templates.

**Architecture:** Keep WeChat-specific behavior in `lucarne-wechat::adapter`. Add a provider-owned `WechatRateLimitConfig`, parse nested `channels.wechat.rate_limit.*` values through existing `AdapterConfig::channel_value`, and wire values into `wechat-ilink` builder. Update daemon/example/onboarding config templates so generated configs show all rate-limit knobs.

**Tech Stack:** Rust nightly, Cargo `+nightly -Zbuild-dir-new-layout`, `lucarne-wechat`, `lucarned`, `serde_yaml`, `wechat-ilink` builder APIs.

---

## File map

- Modify `crates/lucarne-wechat/src/adapter.rs`: add `WechatRateLimitConfig`, parse new YAML/env keys, wire builder options, update adapter tests.
- Modify `crates/lucarned/src/main.rs`: include new defaults in `DEFAULT_LUCARNED_CONFIG`; add/adjust test if needed.
- Modify `crates/lucarned/src/onboarding/config.rs`: include new defaults in generated init YAML and tests.
- Modify `examples/lucarned.yaml`: include new defaults in sample config.
- Plan verification commands run from worktree root: `cargo +nightly test -Zbuild-dir-new-layout -p lucarne-wechat` and targeted `cargo +nightly test -Zbuild-dir-new-layout -p lucarned wechat_rate_limit`.

---

### Task 1: Add failing adapter config tests

**Files:**
- Modify: `crates/lucarne-wechat/src/adapter.rs`

- [ ] **Step 1: Replace default prompt test with full default rate-limit test**

Replace `config_defaults_rate_limit_interaction_prompt` with:

```rust
#[test]
fn config_defaults_rate_limit_options() {
    let config = wechat_config_from_adapter_config(&AdapterConfig::default());

    assert_eq!(config.rate_limit.retry_after, Duration::from_secs(90));
    assert_eq!(config.rate_limit.max_retries, WECHAT_RATE_LIMIT_MAX_RETRIES);
    assert_eq!(config.rate_limit.interaction_window, Duration::from_secs(300));
    assert_eq!(config.rate_limit.interaction_threshold, 6);
    assert_eq!(
        config.rate_limit.interaction_prompt.as_deref(),
        Some(DEFAULT_RATE_LIMIT_INTERACTION_PROMPT)
    );
}
```

- [ ] **Step 2: Update YAML parse test expectations**

In `config_parses_context_and_rate_limit_options_from_yaml`, change YAML rate-limit block to:

```yaml
    rate_limit:
      retry_after_secs: 45
      max_retries: 2
      interaction_window_secs: 120
      interaction_threshold: 4
      interaction_prompt: "请回复任意消息"
```

After existing prompt assertion, add:

```rust
assert_eq!(config.rate_limit.retry_after, Duration::from_secs(45));
assert_eq!(config.rate_limit.max_retries, 2);
assert_eq!(config.rate_limit.interaction_window, Duration::from_secs(120));
assert_eq!(config.rate_limit.interaction_threshold, 4);
```

- [ ] **Step 3: Add env override test**

Add this test near adapter config tests:

```rust
#[test]
fn config_parses_rate_limit_options_from_env() {
    let config = AdapterConfig::from_env([
        ("LUCARNE_WECHAT_RATE_LIMIT_RETRY_AFTER_SECS", "55"),
        ("LUCARNE_WECHAT_RATE_LIMIT_MAX_RETRIES", "1"),
        ("LUCARNE_WECHAT_RATE_LIMIT_INTERACTION_WINDOW_SECS", "180"),
        ("LUCARNE_WECHAT_RATE_LIMIT_INTERACTION_THRESHOLD", "0"),
        (
            "LUCARNE_WECHAT_RATE_LIMIT_INTERACTION_PROMPT",
            "env prompt",
        ),
    ]);

    let config = wechat_config_from_adapter_config(&config);

    assert_eq!(config.rate_limit.retry_after, Duration::from_secs(55));
    assert_eq!(config.rate_limit.max_retries, 1);
    assert_eq!(config.rate_limit.interaction_window, Duration::from_secs(180));
    assert_eq!(config.rate_limit.interaction_threshold, 0);
    assert_eq!(
        config.rate_limit.interaction_prompt.as_deref(),
        Some("env prompt")
    );
}
```

- [ ] **Step 4: Add partial override test**

Add:

```rust
#[test]
fn config_accepts_partial_rate_limit_overrides() {
    let retry_only = AdapterConfig::from_env([(
        "LUCARNE_WECHAT_RATE_LIMIT_RETRY_AFTER_SECS",
        "44",
    )]);
    let threshold_only = AdapterConfig::from_env([(
        "LUCARNE_WECHAT_RATE_LIMIT_INTERACTION_THRESHOLD",
        "0",
    )]);

    let retry_only = wechat_config_from_adapter_config(&retry_only).rate_limit;
    assert_eq!(retry_only.retry_after, Duration::from_secs(44));
    assert_eq!(retry_only.max_retries, WECHAT_RATE_LIMIT_MAX_RETRIES);
    assert_eq!(retry_only.interaction_window, Duration::from_secs(300));
    assert_eq!(retry_only.interaction_threshold, 6);

    let threshold_only = wechat_config_from_adapter_config(&threshold_only).rate_limit;
    assert_eq!(threshold_only.retry_after, Duration::from_secs(90));
    assert_eq!(threshold_only.max_retries, WECHAT_RATE_LIMIT_MAX_RETRIES);
    assert_eq!(threshold_only.interaction_window, Duration::from_secs(300));
    assert_eq!(threshold_only.interaction_threshold, 0);
}
```

- [ ] **Step 5: Run red test**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-wechat config_defaults_rate_limit_options
```

Expected: fail with missing `rate_limit` field or missing `WechatRateLimitConfig` fields.

---

### Task 2: Implement WeChat adapter rate-limit config

**Files:**
- Modify: `crates/lucarne-wechat/src/adapter.rs`

- [ ] **Step 1: Add provider-owned config type and defaults**

Add after `WechatContextExpiryReminderConfig`:

```rust
#[derive(Debug, Clone)]
pub struct WechatRateLimitConfig {
    pub retry_after: Duration,
    pub max_retries: usize,
    pub interaction_window: Duration,
    pub interaction_threshold: usize,
    pub interaction_prompt: Option<String>,
}

impl Default for WechatRateLimitConfig {
    fn default() -> Self {
        Self {
            retry_after: Duration::from_secs(90),
            max_retries: WECHAT_RATE_LIMIT_MAX_RETRIES,
            interaction_window: Duration::from_secs(300),
            interaction_threshold: 6,
            interaction_prompt: Some(DEFAULT_RATE_LIMIT_INTERACTION_PROMPT.to_string()),
        }
    }
}
```

- [ ] **Step 2: Replace `WechatConfig` prompt field**

Change `WechatConfig` field:

```rust
pub rate_limit_interaction_prompt: Option<String>,
```

to:

```rust
pub rate_limit: WechatRateLimitConfig,
```

In `Default for WechatConfig`, replace:

```rust
rate_limit_interaction_prompt: None,
```

with:

```rust
rate_limit: WechatRateLimitConfig::default(),
```

- [ ] **Step 3: Parse rate-limit config**

Add near `wechat_context_expiry_reminder_from_adapter_config`:

```rust
fn wechat_rate_limit_from_adapter_config(config: &AdapterConfig) -> WechatRateLimitConfig {
    let default = WechatRateLimitConfig::default();
    let retry_after = config
        .channel_value(
            "LUCARNE_WECHAT_RATE_LIMIT_RETRY_AFTER_SECS",
            "wechat",
            "rate_limit.retry_after_secs",
        )
        .and_then(parse_duration_secs)
        .unwrap_or(default.retry_after);
    let max_retries = config
        .channel_value(
            "LUCARNE_WECHAT_RATE_LIMIT_MAX_RETRIES",
            "wechat",
            "rate_limit.max_retries",
        )
        .and_then(parse_usize)
        .unwrap_or(default.max_retries);
    let interaction_window = config
        .channel_value(
            "LUCARNE_WECHAT_RATE_LIMIT_INTERACTION_WINDOW_SECS",
            "wechat",
            "rate_limit.interaction_window_secs",
        )
        .and_then(parse_duration_secs)
        .unwrap_or(default.interaction_window);
    let interaction_threshold = config
        .channel_value(
            "LUCARNE_WECHAT_RATE_LIMIT_INTERACTION_THRESHOLD",
            "wechat",
            "rate_limit.interaction_threshold",
        )
        .and_then(parse_usize_allow_zero)
        .unwrap_or(default.interaction_threshold);
    let interaction_prompt = Some(
        config
            .channel_value(
                "LUCARNE_WECHAT_RATE_LIMIT_INTERACTION_PROMPT",
                "wechat",
                "rate_limit.interaction_prompt",
            )
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(default.interaction_prompt.as_deref().unwrap_or(DEFAULT_RATE_LIMIT_INTERACTION_PROMPT))
            .to_string(),
    );

    WechatRateLimitConfig {
        retry_after,
        max_retries,
        interaction_window,
        interaction_threshold,
        interaction_prompt,
    }
}

fn parse_usize(value: &str) -> Option<usize> {
    let value = value.trim().parse::<usize>().ok()?;
    (value > 0).then_some(value)
}

fn parse_usize_allow_zero(value: &str) -> Option<usize> {
    value.trim().parse::<usize>().ok()
}
```

- [ ] **Step 4: Use parser in `wechat_config_from_adapter_config`**

Replace existing `rate_limit_interaction_prompt: Some(...)` expression with:

```rust
rate_limit: wechat_rate_limit_from_adapter_config(config),
```

- [ ] **Step 5: Wire builder**

In `configure_wechat_client_builder`, replace current chain:

```rust
builder = builder
    .markdown_filter(config.markdown_filter)
    .rate_limit_max_retries(WECHAT_RATE_LIMIT_MAX_RETRIES);
```

with:

```rust
builder = builder
    .markdown_filter(config.markdown_filter)
    .rate_limit_retry_after(config.rate_limit.retry_after)
    .rate_limit_max_retries(config.rate_limit.max_retries)
    .rate_limit_interaction_window(config.rate_limit.interaction_window)
    .rate_limit_interaction_threshold(config.rate_limit.interaction_threshold);
```

- [ ] **Step 6: Pass prompt to service**

In `run_wechat_adapter_with_transport`, replace:

```rust
rate_limit_interaction_prompt: config.rate_limit_interaction_prompt.clone(),
```

with:

```rust
rate_limit_interaction_prompt: config.rate_limit.interaction_prompt.clone(),
```

- [ ] **Step 7: Update spawn log field**

Replace:

```rust
rate_limit_interaction_prompt_enabled = config.rate_limit_interaction_prompt.is_some(),
```

with:

```rust
rate_limit_interaction_prompt_enabled = config.rate_limit.interaction_prompt.is_some(),
```

- [ ] **Step 8: Run green adapter tests**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-wechat rate_limit
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-wechat config_parses_context_and_rate_limit_options_from_yaml
```

Expected: all selected tests pass.

- [ ] **Step 9: Commit adapter changes**

Run:

```bash
git add crates/lucarne-wechat/src/adapter.rs
git commit -m "feat: expose wechat rate limit config"
```

---

### Task 3: Add failing daemon/onboarding template tests

**Files:**
- Modify: `crates/lucarned/src/main.rs`
- Modify: `crates/lucarned/src/onboarding/config.rs`

- [ ] **Step 1: Add `DEFAULT_LUCARNED_CONFIG` test in main**

In `#[cfg(test)] mod tests` in `crates/lucarned/src/main.rs`, add:

```rust
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
```

- [ ] **Step 2: Add onboarding render assertions**

In `render_omits_agents_for_all_agents`, after `assert!(yaml.contains("enabled: false"));`, add:

```rust
assert!(yaml.contains("retry_after_secs: 90"));
assert!(yaml.contains("max_retries: 3"));
assert!(yaml.contains("interaction_window_secs: 300"));
assert!(yaml.contains("interaction_threshold: 6"));
assert!(yaml.contains(
    "interaction_prompt: 微信主动通知快到发送限制了，请回复任意消息以刷新会话。"
));
```

- [ ] **Step 3: Run red template tests**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarned wechat_rate_limit
```

Expected: fail because default/onboarding templates do not yet include new fields.

---

### Task 4: Update config templates and examples

**Files:**
- Modify: `crates/lucarned/src/main.rs`
- Modify: `crates/lucarned/src/onboarding/config.rs`
- Modify: `examples/lucarned.yaml`

- [ ] **Step 1: Update `DEFAULT_LUCARNED_CONFIG`**

Change WeChat `rate_limit` block to:

```yaml
    rate_limit:
      retry_after_secs: 90
      max_retries: 3
      interaction_window_secs: 300
      interaction_threshold: 6
      interaction_prompt: "微信主动通知快到发送限制了，请回复任意消息以刷新会话。"
```

- [ ] **Step 2: Update onboarding serialization structs**

Change `WechatRateLimitYaml` to:

```rust
#[derive(Serialize)]
struct WechatRateLimitYaml {
    retry_after_secs: u16,
    max_retries: u8,
    interaction_window_secs: u16,
    interaction_threshold: u8,
    interaction_prompt: &'static str,
}
```

Change `WechatYaml::from` rate-limit construction to:

```rust
rate_limit: WechatRateLimitYaml {
    retry_after_secs: 90,
    max_retries: 3,
    interaction_window_secs: 300,
    interaction_threshold: 6,
    interaction_prompt: DEFAULT_RATE_LIMIT_INTERACTION_PROMPT,
},
```

- [ ] **Step 3: Update example config**

Change `examples/lucarned.yaml` WeChat `rate_limit` block to:

```yaml
    rate_limit:
      retry_after_secs: 90
      max_retries: 3
      interaction_window_secs: 300
      interaction_threshold: 6
      interaction_prompt: 微信主动通知快到发送限制了，请回复任意消息以刷新会话。
```

- [ ] **Step 4: Run green template tests**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarned default_config_exposes_wechat_rate_limit_knobs
cargo +nightly test -Zbuild-dir-new-layout -p lucarned render_omits_agents_for_all_agents
```

Expected: both pass.

- [ ] **Step 5: Commit template changes**

Run:

```bash
git add crates/lucarned/src/main.rs crates/lucarned/src/onboarding/config.rs examples/lucarned.yaml
git commit -m "docs: show wechat rate limit defaults in config"
```

---

### Task 5: Full verification

**Files:**
- All changed files

- [ ] **Step 1: Run WeChat tests**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-wechat
```

Expected: all tests pass.

- [ ] **Step 2: Run lucarned tests**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarned wechat_rate_limit
cargo +nightly test -Zbuild-dir-new-layout -p lucarned render_omits_agents_for_all_agents
```

Expected: selected tests pass.

- [ ] **Step 3: Run check for touched packages**

Run:

```bash
cargo +nightly check -Zbuild-dir-new-layout -p lucarne-wechat -p lucarned
```

Expected: check completes without errors.

- [ ] **Step 4: Inspect diff**

Run:

```bash
git diff --stat HEAD~2..HEAD
git status --short
```

Expected: only intended files changed; status clean except ignored worktree/global untracked files outside branch.

- [ ] **Step 5: Final commit if needed**

If verification fixes were necessary, commit them:

```bash
git add crates/lucarne-wechat/src/adapter.rs crates/lucarned/src/main.rs crates/lucarned/src/onboarding/config.rs examples/lucarned.yaml
git commit -m "test: verify wechat rate limit config"
```
