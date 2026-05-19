# WeChat `/new` Command

## Decision

WeChat `/new` is scoped to a quoted Lucarne notification, matching the existing quoted `/status` routing model. It starts a fresh provider session for the resolved workspace, replies with an acknowledgement, and binds that acknowledgement to the new provider session so the next WeChat reply continues the new session.

## Rationale

WeChat has no persistent per-workspace topic like Telegram, so an unquoted `/new` has no reliable workspace target. Requiring a quoted notification keeps the command deterministic and avoids adding a separate global workspace creation flow.

Opening a fresh workspace session through `LucarneCore` keeps provider selection, project path, state persistence, and live-session cleanup in the core service. The WeChat layer only resolves the quoted workspace, sends the acknowledgement, and records the message binding.

## Alternatives

- Invoke the provider-native `/new` command on the existing live session. That preserves Telegram's lifecycle-command path, but it depends on provider-specific command support and does not by itself give WeChat a new message binding for subsequent replies.
- Add a global unquoted `/new`. That would require new WeChat UX for choosing provider and workspace, which is outside this change.
