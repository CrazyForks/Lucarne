# WeChat Rate Window Config Design

## Context

Lucarne uses `wechat-ilink` for WeChat outbound delivery. Today `lucarne-wechat` exposes only `channels.wechat.rate_limit.interaction_prompt`. The actual rate-window policy lives in the WeChat provider boundary:

- `wechat-ilink` defaults to a 300 second outbound interaction window and threshold 6.
- Successful proactive outbound sends are counted per account in a rolling window.
- When count reaches the threshold exactly, `wechat-ilink` emits `UserInteractionRequested::OutboundRateLimitApproaching`.
- An incoming WeChat message resets the account send window and rate-limit backoff.
- `wechat-ilink` defaults `retry_after` to 90 seconds and retry attempts to 5.
- Lucarne currently overrides max retries to 3 with `WECHAT_RATE_LIMIT_MAX_RETRIES`.

Users need these WeChat rate-limit knobs exposed in daemon config so deployments can tune how often Lucarne sends proactive messages before asking users to refresh the WeChat window.

## Goals

- Expose WeChat rate-limit retry and interaction-window settings in YAML and env.
- Keep all WeChat-specific config ownership inside `lucarne-wechat` adapter/provider boundary.
- Preserve current effective defaults: retry after 90s, max retries 3, interaction window 300s, interaction threshold 6, existing prompt text.
- Sync documented defaults into `examples/lucarned.yaml` and generated/init config templates.
- Add tests for parsing, defaults, and builder wiring.

## Non-goals

- Do not change public/common/core APIs.
- Do not move WeChat provider rules into common layers.
- Do not change `wechat-ilink` behavior or rate-count semantics.
- Do not add runtime mutation commands for these settings.
- Do not persist these settings through `/config`; they remain startup/channel config.

## YAML schema

Extend `channels.wechat.rate_limit`:

```yaml
channels:
  wechat:
    rate_limit:
      retry_after_secs: 90
      max_retries: 3
      interaction_window_secs: 300
      interaction_threshold: 6
      interaction_prompt: "微信主动通知快到发送限制了，请回复任意消息以刷新会话。"
```

Field meanings:

- `retry_after_secs`: delay after WeChat `ret=-2` / `errcode=-2` before retry/backoff.
- `max_retries`: retry attempts after initial rate-limited send.
- `interaction_window_secs`: rolling account window used to count proactive sends.
- `interaction_threshold`: send count in the rolling window that triggers the refresh prompt event. `0` disables the SDK interaction event.
- `interaction_prompt`: Lucarne prompt sent to known users when the SDK requests interaction.

Missing or invalid numeric duration fields keep defaults. Duration fields must be positive. `interaction_threshold` may be zero because the SDK treats zero as disabled.

## Environment variables

Add env aliases under existing WeChat naming:

```text
LUCARNE_WECHAT_RATE_LIMIT_RETRY_AFTER_SECS
LUCARNE_WECHAT_RATE_LIMIT_MAX_RETRIES
LUCARNE_WECHAT_RATE_LIMIT_INTERACTION_WINDOW_SECS
LUCARNE_WECHAT_RATE_LIMIT_INTERACTION_THRESHOLD
LUCARNE_WECHAT_RATE_LIMIT_INTERACTION_PROMPT
```

Env values override YAML via existing `AdapterConfig::channel_value` behavior.

## Adapter design

Add a `WechatRateLimitConfig` in `crates/lucarne-wechat/src/adapter.rs`:

```rust
pub struct WechatRateLimitConfig {
    pub retry_after: Duration,
    pub max_retries: usize,
    pub interaction_window: Duration,
    pub interaction_threshold: usize,
    pub interaction_prompt: Option<String>,
}
```

`WechatConfig` should replace `rate_limit_interaction_prompt: Option<String>` with `rate_limit: WechatRateLimitConfig`.

Defaults should live in `lucarne-wechat`:

- `retry_after`: 90 seconds
- `max_retries`: 3
- `interaction_window`: 300 seconds
- `interaction_threshold`: 6
- `interaction_prompt`: existing default prompt

The existing `WECHAT_RATE_LIMIT_MAX_RETRIES` constant may remain as the default source or be folded into the new config default, but builder wiring must use the parsed config value.

## Builder wiring

`configure_wechat_client_builder()` should pass parsed options to `wechat-ilink`:

```rust
builder = builder
    .markdown_filter(config.markdown_filter)
    .rate_limit_retry_after(config.rate_limit.retry_after)
    .rate_limit_max_retries(config.rate_limit.max_retries)
    .rate_limit_interaction_window(config.rate_limit.interaction_window)
    .rate_limit_interaction_threshold(config.rate_limit.interaction_threshold);
```

Context TTL/reminder wiring remains separate, but still in `configure_wechat_client_builder()`.

`run_wechat_adapter_with_transport()` should pass `config.rate_limit.interaction_prompt.clone()` into `WechatServiceOptions.rate_limit_interaction_prompt`.

## Config template sync

Update every shipped/default config example that currently renders WeChat rate-limit prompt:

- `examples/lucarned.yaml`
- `crates/lucarned/src/main.rs` default config string
- `crates/lucarned/src/onboarding/config.rs` generated/init config model and rendering

Each should include all five `rate_limit` fields with defaults.

## Error handling

- Invalid duration env/YAML values are ignored and defaults apply, matching existing parser style.
- Invalid `max_retries` and `interaction_threshold` values are ignored and defaults apply.
- Empty `interaction_prompt` falls back to default prompt, preserving current behavior.
- Zero duration values are ignored and defaults apply.
- `interaction_threshold: 0` is accepted to disable interaction threshold events.

## Testing

Add/update tests in `crates/lucarne-wechat/src/adapter.rs`:

- Defaults include retry after 90s, max retries 3, interaction window 300s, threshold 6, and default prompt.
- YAML parses all new fields under `channels.wechat.rate_limit`.
- Env parses all new fields and overrides YAML.
- Partial overrides keep defaults for missing fields.
- Builder wiring source contains all four SDK builder methods, or a testable builder helper proves values propagate.

Update adapter config coverage if needed so nested `rate_limit.*` keys remain visible through `AdapterConfig::channel_value`.

Update daemon/onboarding tests to prove generated templates contain:

- `retry_after_secs: 90`
- `max_retries: 3`
- `interaction_window_secs: 300`
- `interaction_threshold: 6`
- existing `interaction_prompt`

## Acceptance criteria

- `channels.wechat.rate_limit.retry_after_secs`, `max_retries`, `interaction_window_secs`, `interaction_threshold`, and `interaction_prompt` work from YAML.
- Equivalent env variables override YAML.
- Existing configs with only `interaction_prompt` still work and receive defaults for new fields.
- Default effective behavior is unchanged.
- Example config and init-generated config expose all new fields.
- Tests pass with `cargo +nightly test -Zbuild-dir-new-layout -p lucarne-wechat` and affected daemon/onboarding tests.
