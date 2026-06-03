# Managed resource status uses live state

## Context

`/status` shows two different resource views:

- managed agents owned by `lucarned`;
- observed provider sessions found through provider history files.

Codex can have multiple writers on the same session JSONL. A desktop Codex
conversation and a `lucarned`-managed Codex process may both append to the same
provider transcript. In that case the JSONL timestamp is durable provider
history activity, but it is not proof that the managed process handled a turn or
that `lucarned` saw live control-plane activity.

The failure was a managed Codex row whose live instance had last been seen at
15:41, while the shared Codex JSONL later received external writes at 16:20.
The resource snapshot copied the observed JSONL `last_active` onto the managed
row, so `/status` made the managed process look active at 16:20.

## Decision

Managed resource rows use only `lucarned` live/runtime state for their active
time. They must not overwrite or fill their `last_active` from observed provider
history sessions.

Observed provider sessions may continue to show JSONL/history-watch activity for
sessions that are not currently represented as managed targets. When an observed
session shares a provider session id with a managed target, the observed row may
be hidden to avoid duplicate status rows, but its timestamp must not fill or
overwrite the managed row. The renderer also keeps the sources separate: a
managed row renders the timestamp on the managed entry, and an observed row
renders the timestamp on the observed entry.

## Rationale

A provider transcript is shared durable history. It can indicate that some
writer appended to the provider session, but it cannot establish ownership by a
specific managed process.

Managed rows are control-plane state: process id, live instance, turn state, and
runtime heartbeat. Observed rows are history state. Keeping these timestamps
separate prevents cross-writer activity from being reported as managed-agent
activity while preserving the external activity signal in the observed section.
