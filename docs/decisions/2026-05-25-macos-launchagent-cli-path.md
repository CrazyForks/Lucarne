# Autostart CLI PATH

Lucarne resumes quoted WeChat sessions inside `lucarned`. When
`lucarned autostart` starts the daemon through a service manager, the daemon
does not necessarily inherit the user's interactive shell PATH. Agent CLIs can
therefore be visible to commands such as `where claude` in a terminal but
unavailable when the daemon later resumes a quoted session.

Decision: autostart files persist only the `PATH` visible to
`lucarned autostart install`. If `PATH` is unavailable, Lucarne adds the minimal
POSIX system PATH:

`/usr/bin:/bin:/usr/sbin:/sbin`

Lucarne makes the CLI lookup path explicit instead of relying on inheritance
from a login shell. It does not synthesize tool directories of its own, does not
derive `PATH` from other environment variables, and does not persist unrelated
environment variables into the service file.

This belongs to daemon startup ownership. Provider adapters still receive
opaque binary names or configured paths and resolve them through the merged
process environment at launch time.

Rejected alternatives:

- Teach the Claude adapter about one host's concrete `claude` path. That fixes
  one provider while leaking host startup policy into provider-specific code.
- Tell users to export `LUCARNE_CLAUDE_BIN` in a shell. LaunchAgent jobs do not
  inherit those shell exports, and the same lookup failure can affect other
  agent CLIs.
