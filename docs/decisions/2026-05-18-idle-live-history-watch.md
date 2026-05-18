# Idle live sessions must not hide history watch updates

## Context

`lucarne::core_service` skipped every history watch update for a workspace that
had a live session. That prevented duplicate notifications for core-submitted
turns, but it also swallowed external writes to the same provider session after
the submitted turn had already completed.

The observed failure was a Codex workspace with an idle live session. WeChat had
already sent the direct reply, then later Codex history updates for the same
session kept logging `history watch update skipped for core-owned live session`
and never reached WeChat's core event subscriber.

## Decision

Treat history watch updates as core-owned duplicates only while the workspace has
a current submitted turn. Idle live sessions fall through to normal history
watch event projection, so external updates can still produce notifications.

## Rationale

Live-session presence is not proof that the live process authored a history
append. The submitted-turn queue is the core-owned work boundary that WeChat and
other adapters already use for direct replies. Keeping suppression tied to that
boundary preserves duplicate protection for active core turns without hiding
external session updates.

See `2026-05-18-live-runtime-history-echo.md` for the follow-up decision that
keeps provider turn ownership after a live runtime terminal event until its
persisted history echo has been observed.
