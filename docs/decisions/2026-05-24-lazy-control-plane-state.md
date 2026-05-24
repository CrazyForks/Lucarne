# Control-plane cold records are store-owned

## Context

`lucarned` startup memory grew with the durable control-plane table. Timeline
rows were already lazy, but startup still deserialized other long-lived records
such as workspaces, channel bindings, message bindings, provider sessions, live
instances, turns, callbacks, scheduled tasks, panels, and history replay state.

The target is a resident daemon that still starts channels and history watch at
startup, but keeps the control-plane boot set bounded. SQLite remains the durable
source of truth; mmap settings and adapter startup behavior are intentionally
unchanged.

## Decision

Startup loads only hot process metadata required before any user/channel action:

- `meta`
- `system_settings`

All other control-plane rows are cold and are read from `ControlPlaneSqliteStore`
on demand. Cold records must not be treated as cache-fill data in
`ControlPlaneState` read paths.

## Ownership rules

1. Store is source of truth for cold rows.
2. Read paths for cold rows query the store directly and must not fall back to
   stale in-memory maps.
3. Cold writes use scoped upsert/delete helpers for the exact rows they own.
4. Hot snapshot persistence may replace only hot rows. It must not upsert cold
   projections that happen to be present in memory for validation.
5. Temporary projections into `ControlPlaneState` are allowed only to reuse
   domain validation/state transitions during a single operation. They are not a
   cache and must not become snapshot-owned.
6. Restart cleanup must be store-backed and must reconcile workspace pointers to
   terminal or missing live instances, because workspaces are no longer eagerly
   loaded.
7. Snapshot persistence must defend the boundary at both layers: state snapshot
   construction emits only hot entities, and store replacement filters to hot
   entity kinds even if a caller passes stale cold projections.
8. ID/token allocation counters are hot metadata. Operations that allocate cold
   command/callback/subagent/history identifiers persist the hot meta snapshot
   separately from the exact cold row they create.
9. Batch reconcile writes validate every workspace from the store before writing
   any outcome, so a missing cold workspace cannot produce a partial batch.
10. Workspace removal may delete provider-session and message-session rows only
    when no remaining workspace references that provider session. Shared provider
    sessions and their message bindings survive until the last workspace is
    removed.
11. Cold live-instance operations must not reactivate or reassign workspace
    bindings as a side effect. They validate that the live row is owned by the
    requested workspace before starting turns or registering intervention
    callbacks.
12. Workspace removal also removes runtime-only live session handles, generation
    state, workspace event senders, and submitted-turn bookkeeping for that
    workspace. Durable deletion and runtime deletion must not diverge.
13. Rebinding a workspace or attaching a live instance must hydrate any existing
    provider-session row first, so status/model/usage fields are preserved rather
    than overwritten by a fresh resume-ref shell.

## Rationale

A partial lazy load is unsafe if full snapshot persistence can later write stale
cold projections back to SQLite. That would make memory and store compete for
ownership. Restricting snapshot ownership to hot rows keeps the boundary simple:
state may orchestrate, but durable cold data changes only through scoped store
operations.

This keeps startup RSS bounded by hot metadata while preserving existing daemon
startup behavior and SQLite configuration.

## Consequences

- New cold read APIs should be added to `ControlPlaneSqliteStore`, not by
  re-expanding `load_control_plane`.
- New cold mutation paths must persist exact changed entities or exact deletes.
- Tests for restart/reload behavior should assert that cold rows remain absent
  from `ControlPlaneState` but still hydrate through store-backed core APIs.
