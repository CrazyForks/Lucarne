# Telegram command sync is hash-gated

## Context

Telegram startup performs `setMyCommands` after constructing the channel. Memory
profiles showed that this request can initialize extra HTTP/TLS/Security state
during daemon startup, even though the command menu rarely changes.

The command menu is Telegram-provider behavior. The cache must not add Telegram
knowledge to common control-plane state.

## Decision

Telegram computes a stable hash over its provider-owned command menu. The
adapter stores the last successfully synced hash in a Telegram-owned SQLite
table inside the existing lucarne state database. On startup, `setMyCommands`
runs only when the stored hash differs from the current menu hash.

The cache key is scoped by Telegram bot id parsed from the token prefix. The
secret token suffix is not stored.

## Consequences

- First startup for a bot still calls `setMyCommands`.
- Later startups skip `setMyCommands` until the command list or descriptions
  change.
- Failed command syncs do not update the cache, so a later startup retries.
- The table is provider-owned and does not expand common control-plane schema or
  load paths.
