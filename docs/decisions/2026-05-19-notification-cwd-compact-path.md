# Notification cwd compact path

## Context

Telegram `/panel` already shortened cwd values with a tail-preserving
`…/segment/leaf` format. Agent notifications still rendered full cwd paths in
their footer, which exposed noisy absolute prefixes and made Telegram and
WeChat inconsistent.

## Decision

Move the panel path compaction helper into `lucarne-channel::agent_message` and
use it before putting cwd into notification footers. Telegram panel rendering,
Telegram notifications, and WeChat agent messages now share the same display
rule.

Keep runtime cwd values unchanged for resume, status, history lookup, and
provider calls. Only user-visible notification footer text is compacted.

## Consequences

Long absolute paths render as tail-preserving paths such as
`…/opensource/conductor/lucarnex`. Short two-segment paths such as
`/tmp/workspace-a` remain unchanged.
