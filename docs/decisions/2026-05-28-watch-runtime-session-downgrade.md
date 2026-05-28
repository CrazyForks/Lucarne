# Runtime watch session downgrade releases leaf targets

## Context

History watch had hot/recent classification only during startup and discovery.
Once a session file was baselined or direct-watched, it stayed in the running
watcher until the whole watcher restarted or the file was deleted. Long-lived
daemons could therefore keep session-specific baselines and direct file watch
targets after those files had aged out of the active window.

## Decision

Use `WatchConfig::scan_interval` as a runtime retention tick. On each tick,
release leaf watch targets whose files are no longer eligible for direct session
watching. Keep the session baseline, so a later append observed through the
parent/root watch can continue from the last known offset instead of treating
the session as newly discovered history.

On recursive-root platforms such as macOS and Windows, downgrade removes the
per-session file watch and leaves the recursive root watch in place. On
non-recursive platforms such as Linux, downgrade may remove a recent-session
directory watch when that directory is no longer needed by another retained
session; the always-watched parent directory targets remain in place.

The release boundary is the existing `should_watch_session_file_target`
contract: by default, a session is retained while it is within the recent
window. This intentionally does not use only the shorter hot-file window,
because recent-but-not-hot files are still direct-watched to avoid missing
appends from resumed sessions.

Skip downgrade for pending paths, explicit file roots, and baselines with a
pending partial JSONL record. Pending paths must first flow through the debounced
update/delete path, explicit roots are caller-owned watch targets, and partial
records need their buffered bytes to parse a later completion correctly.

## Rationale

Downgrade is a watch-target ownership concern, not provider parsing logic. Keeping
the decision in the common watcher preserves provider boundaries while letting
provider descriptors continue to decide which paths are session-like and which
directory roots cover future reactivation.

The tick performs only bounded state maintenance over already-baselined paths.
It does not introduce whole-history rescans or move provider-specific discovery
rules into the common layer.
