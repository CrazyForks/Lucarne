# Watch recent session files directly on macOS

## Context

On macOS the watcher registered provider roots with a recursive FSEvents stream
and registered only hot session files with a non-recursive file watch. A resumed
or still-active session can be recent but older than the hot-file window when
`lucarned` starts. In that case the file is baselined, but later appends depend
entirely on the recursive root watcher.

The live daemon showed that this dependency can miss an existing Codex JSONL
append: the final `task_complete` record was written, but no debounced watch
path was processed and no observed session was updated.

## Decision

For baselined session files under recursive roots, register a direct
non-recursive file watch when the file is recent or hot. Stale files stay
unwatched unless they are configured as explicit file roots.

Add trace-level diagnostics at the watch boundaries before relying on the fix:
initial baseline discovery, initial watch target selection, macOS recursive
FSEvents callbacks, JSONL delta lookback, and core watch-update projection.

Do not expand this to direct-watch stale files. A stale session that becomes
active should enter through the recursive root watch, be parsed from its latest
tail, and then be promoted to a direct file watch. That keeps inactive history
out of baseline memory and avoids registering watch targets for old sessions.

## Rationale

The root bug was ownership of change detection, not provider parsing or channel
notification. A session file that is recent enough to baseline is recent enough
to require append detection after startup. Direct file watches make the
contract explicit without scanning whole transcript files, without provider
special cases, and without relying on text-level notification de-duplication.

Stale-session reactivation is covered by the recursive root contract instead of
the recent-file direct-watch contract. This preserves the intended hot-path
ownership transition without increasing idle memory for old session history.
