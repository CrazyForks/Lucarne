# Linux Support Design

## Goal

Make Linux a first-class Lucarne target in the existing install/autostart PR. Linux support must cover Ubuntu/Debian desktop, Ubuntu/Debian headless server, Fedora, and Arch with one shipped `lucarned` binary archive per cargo-dist target.

## Scope

This work stays in one PR and keeps macOS/Windows behavior unchanged. Linux support includes:

- Linux recursive session watch parity through `notify`/inotify.
- Linux host tool discovery for process/resource and writer PID probes.
- Linux `lucarned autostart` hardening for systemd user services.
- Linux `lucarned doctor` diagnostics for runtime prerequisites and common headless failures.
- Linux cargo-dist archive smoke coverage.

Non-systemd Linux remains supported for manual `lucarned` execution only. `lucarned autostart` reports unsupported/diagnostic guidance when systemd user services are unavailable.

## Platform Contracts

### Watch

macOS keeps `MacRecursiveWatcher`. Windows keeps the existing `notify` recursive implementation. Linux changes from non-recursive fallback to `notify::RecursiveMode::Recursive` for recursive session roots.

Linux watch must handle:

- existing hot/recent session files,
- nested session file creation,
- rename into watched roots,
- append updates,
- provider-specific path filtering without Linux-specific provider branching in shared layers.

### Host tools

Linux must not assume macOS paths such as `/usr/sbin/lsof`. Linux resolves `ps` and `lsof` from `PATH` first, then uses distro-common fallbacks. Missing tools degrade gracefully where possible and are reported by `doctor` with package hints:

- Debian/Ubuntu: `procps`, `lsof`
- Fedora: `procps-ng`, `lsof`
- Arch: `procps-ng`, `lsof`

macOS keeps its current behavior unless shared Unix helper extraction is needed without behavior change.

### Autostart

Linux autostart uses systemd user services:

- unit path: `~/.config/systemd/user/lucarned.service`
- commands: `systemctl --user daemon-reload`, `enable`, `start`, `status`, `stop`, `disable`
- unit restart policy: `Restart=on-failure`, `RestartSec=5`

Headless failures should guide users toward:

```sh
loginctl enable-linger $USER
```

Lucarne must not run `sudo`, modify linger automatically, or create system services.

### Doctor

Linux `doctor` reports:

- config/state/log paths and writability,
- `systemctl --user` availability,
- user bus indicators (`XDG_RUNTIME_DIR`, `DBUS_SESSION_BUS_ADDRESS` where relevant),
- `loginctl` availability and linger hint,
- `ps` availability,
- `lsof` availability,
- optional agent CLI presence in `PATH`.

Doctor warnings should be actionable and must not fail solely because optional agent CLIs are absent.

## Release and Runtime Dependencies

Initial Linux release remains GNU dynamic (`unknown-linux-gnu`). This PR does not add musl/static builds and does not switch TLS/sqlite strategy. Runtime dependency issues are handled through release smoke and doctor/documentation.

cargo-dist continues to emit `.tar.xz` archives for Linux. Archive smoke must verify:

```sh
./lucarned --version
./lucarned paths
./lucarned doctor
```

On systemd hosts, smoke also verifies autostart install/start/status/stop/uninstall.

## Testing Strategy

- Unit tests for Linux path/tool resolution, systemd unit rendering, and doctor output.
- Linux watch tests for recursive nested creation and rename behavior.
- Existing macOS and Windows tests remain green.
- Linux verification uses nightly Cargo with `-Zbuild-dir-new-layout`.
- Real/VM smoke covers at least one Debian/Ubuntu systemd user environment. Fedora/Arch coverage may use containers for compile/doctor/path checks and a systemd-capable host when available.

## Acceptance Criteria

- `cargo +nightly check -Zbuild-dir-new-layout -p lucarned` passes on Linux, macOS, and Windows.
- `cargo +nightly test -Zbuild-dir-new-layout -p lucarned-ctl` passes on Linux, macOS, and Windows.
- `cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions --features watch,codex,claude,gemini,pi,copilot,agent_session,discovery` passes on Linux for watch/provider coverage.
- Linux cargo-dist `.tar.xz` archive builds and extracted `lucarned` runs `--version`, `paths`, and `doctor`.
- Linux systemd-user autostart works on a systemd user session and gives clear guidance on headless/missing-bus failures.
- No new concrete provider special-cases are added to common/history layers.
