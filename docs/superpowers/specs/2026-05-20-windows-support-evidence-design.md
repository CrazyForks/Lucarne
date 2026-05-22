# Windows Support Evidence-First Design

## Goal

Support Windows by fixing confirmed platform gaps only. Do not replace cross-platform crates or add platform abstraction around libraries that already support Windows.

## Correction From Prior Draft

Prior draft overreached. `tokio::process` and `notify` both support Windows. They must stay as shared dependencies. The Windows work is about Lucarne's Unix-only calls around those crates, not about replacing those crates.

Evidence:

- Tokio process docs say the module provides async process management and uses Unix signal handling and Windows system APIs: [`tokio/process/mod.rs` lines 1-7](https://docs.rs/crate/tokio/1.52.1/source/src/process/mod.rs#L1-L7).
- Tokio `Command::process_group` is explicitly Unix-only: [`tokio/process/mod.rs` lines 787-792](https://docs.rs/crate/tokio/1.52.1/source/src/process/mod.rs#L787-L792).
- Tokio `Child::start_kill` and `Child::kill` kill the child process handle and then wait; they are not documented as process-tree/group termination: [`tokio/process/mod.rs` lines 1240-1261](https://docs.rs/crate/tokio/1.52.1/source/src/process/mod.rs#L1240-L1261), [`tokio/process/mod.rs` lines 1326-1329](https://docs.rs/crate/tokio/1.52.1/source/src/process/mod.rs#L1326-L1329).
- Tokio exposes Windows process raw handles, so Windows-specific process management can build on Tokio children if needed: [`tokio/process/mod.rs` lines 1230-1237](https://docs.rs/crate/tokio/1.52.1/source/src/process/mod.rs#L1230-L1237).
- `notify` describes itself as cross-platform: [`notify/src/lib.rs` line 1](https://docs.rs/crate/notify/6.1.1/source/src/lib.rs#L1).
- `notify::RecommendedWatcher` maps to `ReadDirectoryChangesWatcher` on Windows: [`notify/src/lib.rs` lines 369-378](https://docs.rs/crate/notify/6.1.1/source/src/lib.rs#L369-L378).
- `notify` has a Windows dependency block for `windows-sys`: [`notify/Cargo.toml` lines 111-116](https://docs.rs/crate/notify/6.1.1/source/Cargo.toml#L111-L116).

## Verified Dependency Support Matrix

| Dependency / area | Windows support status | Evidence | Decision |
|---|---|---|---|
| `tokio::process` | Supported on Windows. | Tokio process docs state Windows system APIs are used for async process support ([source](https://docs.rs/crate/tokio/1.52.1/source/src/process/mod.rs#L1-L7)). | Keep. Do not wrap basic spawn/stdin/stdout/stderr/wait. |
| Tokio process group | Unix-only. | `Command::process_group` is under `#[cfg(unix)]` ([source](https://docs.rs/crate/tokio/1.52.1/source/src/process/mod.rs#L787-L792)). | Keep `process_group(0)` behind Unix cfg. Use Windows-specific strategy only for tree termination if needed. |
| Tokio child kill | Cross-platform child kill, not process tree. | `start_kill` calls child kill; `kill` then waits ([source](https://docs.rs/crate/tokio/1.52.1/source/src/process/mod.rs#L1240-L1261), [source](https://docs.rs/crate/tokio/1.52.1/source/src/process/mod.rs#L1326-L1329)). | Use for single process. For child tree, design explicit Windows Job Object/Toolhelp work. |
| `notify` | Supported on Windows. | Cross-platform docs and Windows `RecommendedWatcher` alias ([source](https://docs.rs/crate/notify/6.1.1/source/src/lib.rs#L1), [source](https://docs.rs/crate/notify/6.1.1/source/src/lib.rs#L369-L378)). | Keep current `notify::recommended_watcher`. Test behavior; do not replace. |
| `nix` | Unix-oriented; not Windows substrate. | Cargo metadata says `os::unix-apis` and README says bindings to *nix APIs ([Cargo.toml](https://docs.rs/crate/nix/0.29.0/source/Cargo.toml#L26-L30), [README](https://docs.rs/crate/nix/0.29.0/source/README.md#L9-L11)). | Move usages behind `#[cfg(unix)]` or Unix-only module. No Windows compile dependency. |
| `reqwest` + `native-tls` | Supported on Windows through system TLS. | Reqwest native-tls backend uses system TLS on Windows/Mac and OpenSSL on Linux ([source](https://docs.rs/crate/reqwest/0.12.28/source/src/tls.rs#L31-L39)); `native-tls` uses SChannel on Windows ([source](https://docs.rs/crate/native-tls/0.2.18/source/src/lib.rs#L9-L15)). | Keep. No platform wrapper. |
| `wechat-ilink` | No OS-specific source found; built on `reqwest(native-tls)` + `tokio`. | Crate depends on reqwest `json`, `native-tls` and tokio `macros/sync/time` ([Cargo.toml](https://docs.rs/crate/wechat-ilink/0.5.0/source/Cargo.toml#L93-L121)). Local source grep found no `std::os::*`, `cfg(windows)`, `cfg(unix)`, `/bin`, or `/usr` in `src/`. | Treat as likely portable; verify by Windows build/run smoke, not redesign. |
| `teloxide` | Rust/Tokio framework; no obvious OS-specific barrier for bot runtime. | Teloxide depends on Tokio and teloxide-core; default feature includes native-tls and ctrlc handler ([Cargo.toml](https://docs.rs/crate/teloxide/0.17.0/source/Cargo.toml#L78-L87), [Cargo.toml](https://docs.rs/crate/teloxide/0.17.0/source/Cargo.toml#L364-L383)). | Keep. Verify Windows build. Ctrl-C behavior may need smoke test. |
| `rusqlite` / `libsqlite3-sys` | Rust crate is cross-platform, but linking can require system SQLite/vcpkg unless bundled feature is enabled. | `libsqlite3-sys` default uses `pkg-config`/`vcpkg`; `bundled` and `bundled-windows` features compile SQLite via `cc` ([Cargo.toml](https://docs.rs/crate/libsqlite3-sys/0.28.0/source/Cargo.toml#L70-L100)). | Windows `cargo check` may pass, but release `cargo build` must be verified. Consider `rusqlite` bundled/bundled-windows only if build fails or packaging needs static simplicity. |
| `tracing-appender` | Uses `std::io::Write` and file appender abstractions; docs.rs builds Windows target page. | Docs describe file appender/non-blocking writer through standard `Write` ([docs](https://docs.rs/tracing-appender/latest/i686-pc-windows-msvc/tracing_appender/)). | Keep. Verify log path behavior. |

## Confirmed Current Windows Compile Blockers

After installing Rust nightly and Visual Studio Build Tools in the Parallels Windows ARM64 VM, `cargo +nightly check -Zbuild-dir-new-layout -p lucarned` reached Lucarne compile and failed on confirmed Unix-only code:

- `crates/lucarne/src/launcher.rs`
  - direct `nix::sys::signal` usage
  - direct `std::os::unix::process::CommandExt`
- `crates/lucarne/src/core_service/service.rs`
  - direct `nix::sys::signal::kill(pid, None)` for PID liveness
  - hardcoded `/bin/ps`
  - hardcoded `/usr/sbin/lsof`
- `crates/lucarne/src/testing/live/providers.rs`
  - direct `nix` signal imports
  - direct `std::os::unix::process::CommandExt`
- Tests and live helpers contain Unix-only `PermissionsExt`, shell scripts, and symlinks. These are test harness issues, not core runtime library support issues.

## Design Principles

1. **Do not abstract what is already cross-platform.** Keep Tokio process spawn, I/O, wait, kill, and notify watcher usage unless a verified API gap requires OS-specific code.
2. **Localize actual OS gaps.** Unix-only operations get small cfg boundaries near the usage or in focused helpers.
3. **Prefer Tokio APIs over std Unix extensions.** Use `tokio::process::Command::process_group(0)` under `#[cfg(unix)]` instead of `cmd.as_std_mut()` with `std::os::unix::process::CommandExt`.
4. **Do not add Windows behavior until verified.** Windows process-tree management needs a separate evidence pass: Job Objects, Toolhelp traversal, or explicit child-only semantics.
5. **Provider-specific Windows paths stay in provider modules.** History discovery paths for Claude/Codex/Gemini/Copilot/Pi remain provider-owned per repository instructions.
6. **Verification must include `cargo build`, not only `cargo check`, for `rusqlite` linking.**

## Revised Implementation Shape

### Minimal Platform Helpers

Do not create a broad `platform` layer for Tokio/notify. If helpers are useful, keep them narrow:

```rust
mod process_control;
```

Responsibilities:

- Unix signal mapping and process-group termination.
- Windows liveness and optional termination strategy only after Windows API choice is verified.
- `process_id_is_alive(pid)` behind cfg.
- Resource snapshot collection behind cfg (`ps` on Unix; Windows strategy pending).
- Writer PID lookup behind cfg (`lsof` on Unix; Windows strategy pending).

No wrapper around:

- `tokio::process::Command::new/spawn/output/status`
- `notify::recommended_watcher`
- `reqwest`
- `teloxide`
- `wechat-ilink`

### Launcher

Current runtime should continue using Tokio directly:

```rust
let mut cmd = tokio::process::Command::new(&spec.bin);
cmd.args(&spec.args);
cmd.stdin(Stdio::piped())
   .stdout(Stdio::piped())
   .stderr(Stdio::piped())
   .kill_on_drop(true);

#[cfg(unix)]
cmd.process_group(0);
```

Open question for Windows:

- If child-only termination is acceptable for first Windows milestone, use Tokio `Child::start_kill/kill` where handle is still owned.
- If full process-tree termination is required, implement and test Windows Job Objects or child traversal. Do not claim equivalence to POSIX process groups until proven.

### Watch

Keep `notify::recommended_watcher` as-is. Windows work is behavioral verification only:

- create file under watched root
- append JSONL line
- rename/write temp file pattern
- observe debounce behavior
- verify path casing/canonicalization

No design change unless these tests fail.

### Paths

Use a small helper for home/config expansion because project code currently assumes `$HOME` in multiple places. Windows resolution order:

1. `HOME` if set and non-empty
2. `USERPROFILE`
3. `HOMEDRIVE` + `HOMEPATH`

For user-facing defaults, decide later whether Windows should keep `~/.lucarned` for compatibility or use `%APPDATA%\lucarned`. This is product behavior, not dependency support.

### SQLite

Do not change `rusqlite` features based on assumption. Run Windows `cargo build -p lucarned`. If link fails, choose one:

- add `rusqlite` `bundled`/`bundled-windows` for release simplicity
- install/package SQLite via vcpkg/system dependency

Prefer bundled for single-binary user experience, but verify size/build impact first.

## Verification Plan

### Local macOS

```bash
cargo +nightly check -Zbuild-dir-new-layout -p lucarned
cargo +nightly test -Zbuild-dir-new-layout -p lucarne launcher
```

### Windows VM

Use local target dir; Parallels shared folder caused rlib temp archive errors under `target/`.

```powershell
$env:HTTP_PROXY='http://10.211.55.2:6153'
$env:HTTPS_PROXY='http://10.211.55.2:6153'
$env:PATH = $env:USERPROFILE + '\.cargo\bin;' + $env:PATH
$env:CARGO_TARGET_DIR='C:\lucarne-target'
Set-Location 'Z:\Volumes\Data\opensource\conductor\lucarne\.worktrees\windows-platform-substrate'
cargo +nightly check -Zbuild-dir-new-layout -p lucarned
cargo +nightly build -Zbuild-dir-new-layout -p lucarned
```

### Windows watch smoke

Add a small test binary or integration test only after compile succeeds:

1. start `SessionWatcher` with temp root
2. create provider-like file under root
3. append complete JSONL line
4. assert update emitted within timeout

## Acceptance Criteria For Next Implementation Plan

- No code changes before plan is rewritten from this evidence.
- No claim that a crate lacks Windows support unless docs/source confirm it.
- Tokio and notify remain shared cross-platform crates.
- Direct Unix-only imports in common runtime are removed or cfg-gated.
- Windows `cargo check` and `cargo build` both pass before declaring bottom layer done.
