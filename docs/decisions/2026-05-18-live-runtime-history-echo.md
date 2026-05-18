# Live runtime turns own their persisted history echoes

## Context

Lucarne receives provider output through two independent sources:

- the live runtime event stream, used for core-submitted turns and direct replies;
- the provider history watch, used for external/background transcript changes.

After a live runtime turn completed, the core removed the submitted turn from the
workspace queue immediately. A later provider file append for that same turn was
then seen by the history watch with no remaining submitted-turn context, so it
was emitted as a background notification. This produced duplicate WeChat
delivery: first the direct live reply, then the persisted transcript echo.

## Decision

When a live runtime terminal event completes a submitted turn, record the
provider session id and provider turn id as owned by the live runtime. While a
bounded claim exists, history watch events carrying that same provider turn id
are treated as the persisted form of the already-delivered live turn and are not
broadcast as background timeline events. The claim is independent of the live
session map so it still suppresses the echo if the provider process is killed or
detached before the file watcher observes the persisted terminal record.

Core ownership starts when the daemon records a live `start_turn`, not only when
callers use `submit_turn`. This covers adapters such as Telegram that submit
through the core-provided live session handle while still recording the turn in
the core control plane.

The decision is tied to provider turn identity, not message text and not a
channel-specific duplicate check. History updates with a different provider turn
id remain visible, so idle live sessions can still surface real external writes.

Codex resume references are known provider session ids, so the Codex adapter
marks resume refs as session-id hints. That keeps live runtime ownership and
history watch ownership on the same `codex:<thread_id>` key even if readiness
times out before Codex sends an explicit thread-started notification.

## Rationale

Provider transcript files are durable state, not a second user-visible source of
truth for core-submitted live turns. The root issue was source ownership being
lost between the live runtime stream and the asynchronous file watcher. Keeping
that ownership in the core service makes WeChat, Telegram, and future adapters
consume the same normalized event stream instead of each channel guessing which
messages to suppress.

Telegram previously had its own `BotState` direct-delivery suppression set in
addition to the core suppression state. That adapter-local state is removed so
the channel does not carry a second copy of this ownership decision.

The claim is bounded by time so an unobserved terminal echo cannot leave stale
runtime state behind.
