# Windows Complete Substrate Design

## Goal

Implement Windows as a first-class host platform with the same user-visible behavior as macOS/Unix. OS mechanisms are implementation details; the contract is functional parity for launch, interrupt, close, resource display, history watch, file-user detection, paths, and build/test workflows. Keep boundaries strict: cross-platform crates stay shared, OS-specific behavior stays in host/platform modules, provider-specific behavior stays in provider modules.

## Non-Negotiable Corrections

- Adding Windows support must not change macOS behavior. Existing macOS process, path, resource, watch, provider, and test semantics are the reference contract. Any macOS code movement must be a mechanical isolation with identical behavior and must be guarded by existing or added macOS tests.
- `tokio::process` supports Windows. Keep Tokio for spawn, stdio pipes, async wait, output, and child handles.
- `notify` supports Windows. Keep `notify::recommended_watcher`; verify behavior with Windows smoke tests.
- Windows does not use POSIX `nix`, `SIGTERM`, `SIGKILL`, or POSIX process groups. Implement the same Lucarne functions with Windows-native Job Objects and console control events.
- Do not introduce common-layer provider path catalogs or provider-specific branches.

## Functional Parity Contract

Lucarne callers must observe the same behavior on Windows and macOS/Unix:

| Function | macOS/Unix behavior | Windows behavior target |
| --- | --- | --- |
| Launch agent | Spawn CLI with same cwd/env/stdio and track root PID | Same |
| Interrupt turn | Deliver cancel signal; session stays alive if agent handles it | Same user result via console control event; explicit error if Windows cannot deliver |
| Interrupt timeout | Escalate unresponsive session after configured grace | Same: session ends and descendants are cleaned |
| Close session | End live session and clean descendants after grace | Same final result: live session ends and no descendants remain |
| Drop process handle | No intentional orphaning | Same, enforced by Job close kill limit |
| Resource status | Show process count, CPU, memory for live agent tree | Same fields and meaning for Windows process tree |
| Observed writer PID | Best-effort owning/writing process for session file | Same best-effort result through Restart Manager |
| History watch | Append/rename updates appear through `SessionWatcher` | Same through existing `notify` watcher |
| Default paths | Platform-native Lucarne state/config path | Platform-native Windows path |

Signal names are internal Lucarne control intents, not a requirement to emulate POSIX APIs. Windows mappings may use different primitives, but must preserve these outcomes.

## Evidence

- Tokio process docs state async process support uses signal handling on Unix and system APIs on Windows: [`tokio/process/mod.rs` lines 1-7](https://docs.rs/crate/tokio/1.52.1/source/src/process/mod.rs#L1-L7).
- Tokio `Command::process_group` is Unix-only: [`tokio/process/mod.rs` lines 787-792](https://docs.rs/crate/tokio/1.52.1/source/src/process/mod.rs#L787-L792).
- Tokio exposes Windows `creation_flags` and `Child::raw_handle`: [`tokio/process/mod.rs` lines 669-672](https://docs.rs/crate/tokio/1.52.1/source/src/process/mod.rs#L669-L672), [`tokio/process/mod.rs` lines 1230-1237](https://docs.rs/crate/tokio/1.52.1/source/src/process/mod.rs#L1230-L1237).
- Tokio `Child::start_kill` / `kill` operate on the child handle and then wait; they are not process-tree semantics: [`tokio/process/mod.rs` lines 1240-1261](https://docs.rs/crate/tokio/1.52.1/source/src/process/mod.rs#L1240-L1261), [`tokio/process/mod.rs` lines 1326-1329](https://docs.rs/crate/tokio/1.52.1/source/src/process/mod.rs#L1326-L1329).
- Windows Job Objects manage groups of processes as a unit; operations affect all associated processes; `TerminateJobObject` terminates all associated processes: Microsoft Job Objects docs and TerminateJobObject docs.
- By default, child processes created by a process associated with a job are also associated with that job, unless breakaway limits are set: Microsoft Job Objects docs.
- `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` terminates associated processes when the last job handle closes: Microsoft Job Objects docs.
- `GenerateConsoleCtrlEvent` sends a control signal to a console process group sharing the caller's console; `CREATE_NEW_PROCESS_GROUP` makes a process root of a new Windows process group: Microsoft Console and Process Creation Flags docs.
- `notify` is cross-platform and maps `RecommendedWatcher` to `ReadDirectoryChangesWatcher` on Windows: [`notify/src/lib.rs` lines 1](https://docs.rs/crate/notify/6.1.1/source/src/lib.rs#L1), [`notify/src/lib.rs` lines 369-378](https://docs.rs/crate/notify/6.1.1/source/src/lib.rs#L369-L378).
- `notify`'s Windows backend passes `RecursiveMode::Recursive` to `ReadDirectoryChangesW` as subtree monitoring, while macOS `RecommendedWatcher` would be kqueue in this workspace because `macos_kqueue` is enabled. Lucarne therefore already uses its own FSEvents watcher for macOS recursive roots.
- Lucarne decision `docs/decisions/2026-05-19-watch-recent-session-file-targets.md` records a real macOS missed-append bug when relying only on recursive FSEvents. Hot/recent direct file watches are required for behavior, not cosmetic optimization.
- `nix` is Unix API binding: [`nix/Cargo.toml` lines 26-30](https://docs.rs/crate/nix/0.29.0/source/Cargo.toml#L26-L30), [`nix/README.md` lines 9-11](https://docs.rs/crate/nix/0.29.0/source/README.md#L9-L11).
- Windows Restart Manager can list applications/services currently using registered file resources through `RmStartSession`, `RmRegisterResources`, and `RmGetList`: Microsoft Restart Manager docs.

## macOS Preservation Contract

Windows work is additive unless code must move behind a platform boundary. For every moved call site, macOS must keep the same inputs, outputs, side effects, and error behavior.

| Area | macOS behavior to preserve |
| --- | --- |
| Launch | Tokio spawn, cwd/env/stdin/stdout/stderr, `process_group(0)`, root-PID reactive signal, group close escalation |
| Process cleanup | `SIGTERM` process group, grace wait, `SIGKILL` process group |
| Paths | existing `HOME`-based defaults and `~/.lucarned` state path |
| Provider discovery | provider-owned discovery rules and macOS/default roots |
| Resource status | `/bin/ps` parsing and current process-group/descendant aggregation |
| Writer PID | `/usr/sbin/lsof -t -- <path>` best-effort behavior |
| Watch | FSEvents recursive roots plus hot/recent direct file watches |
| Tests/live harness | Unix executable-bit and symlink assumptions stay Unix/macOS-only; existing macOS fixtures unchanged |

Do not broaden cfgs in ways that sweep macOS into Windows changes. Prefer explicit `#[cfg(windows)]` branches plus existing `#[cfg(unix)]` or `#[cfg(target_os = "macos")]` branches.

## Boundary Model

### `crates/lucarne/src/host/`

New crate-private module. Owns OS behavior. Not public API.

```text
host/
  mod.rs
  paths.rs
  process/
    mod.rs
    unix.rs
    windows.rs
  process_table/
    mod.rs
    unix.rs
    windows.rs
  file_users/
    mod.rs
    unix.rs
    windows.rs
```

Responsibilities:

- `host::process`: process launch extras, process-tree/session control, pid liveness.
- `host::process_table`: bounded process/resource snapshots and aggregation inputs.
- `host::file_users`: map a session file path to a process using it.
- `host::paths`: platform defaults and `~` expansion.

Explicit non-responsibilities:

- No provider IDs.
- No provider session paths.
- No transcript parsing.
- No channel behavior.
- No wrapper around `tokio::process::Command::new/spawn/stdout/stderr/wait` except OS-specific launch attributes.
- No wrapper around `notify::recommended_watcher`.

### Provider boundary

Provider Windows paths stay in `agent-sessions/src/providers/<provider>/discovery.rs`. If Codex, Claude, Gemini, Copilot, or Pi need Windows-specific history roots, add them inside that provider's discovery implementation.

## Process Lifecycle Design

### Shared `Process` model

`launcher::ProcessInner` gains one crate-private guard:

```rust
host_process: host::process::ManagedProcess,
```

`ManagedProcess` is created immediately after `tokio::process::Command::spawn()` and before Lucarne exposes the process to runtime.

Shared flow stays Tokio-native:

1. Build `tokio::process::Command`.
2. Set stdin/stdout/stderr pipes and `kill_on_drop(true)`.
3. Apply `host::process::configure_command(&mut cmd)`.
4. `cmd.spawn()`.
5. Create `host::process::ManagedProcess::attach(&child)`.
6. Store `ManagedProcess` in `ProcessInner`.
7. Waiter task still awaits Tokio `Child::wait()`.

### Unix implementation

Use Tokio's Unix API, not `std::os::unix::process::CommandExt` directly:

```rust
#[cfg(unix)]
cmd.process_group(0);
```

Signals:

- `Process::signal(name)` keeps current macOS/Unix reactive-signal behavior: send `name` to the root process PID.
- `Process::close()` keeps current macOS/Unix tree cleanup behavior: send `SIGTERM` to the process group, wait grace, then send `SIGKILL` to the process group.

Dependency:

```toml
[target.'cfg(unix)'.dependencies]
nix = { workspace = true }
```

### Windows implementation

Windows uses Job Objects for the same no-orphan process-family contract and console control events for the same interrupt/close cooperation contract. These mechanisms are not exposed above `host::process`.

#### Launch flags

`host::process::configure_command` on Windows sets:

```rust
cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_SUSPENDED);
```

Reasons:

- `CREATE_NEW_PROCESS_GROUP`: child root can receive `CTRL_BREAK_EVENT` by PID group.
- `CREATE_SUSPENDED`: child cannot spawn grandchildren before Job Object assignment.

#### Attach after spawn

`ManagedProcess::attach(&child)` on Windows:

1. Get `child.raw_handle()` from Tokio.
2. `CreateJobObjectW(null, null)`.
3. `SetInformationJobObject(JobObjectExtendedLimitInformation)` with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
4. `AssignProcessToJobObject(job, child_raw_handle)`.
5. Resume all threads owned by the new process:
   - `CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)`
   - `Thread32First` / `Thread32Next`
   - filter `th32OwnerProcessID == child_pid`
   - `OpenThread(THREAD_SUSPEND_RESUME, ...)`
   - `ResumeThread`
6. Store job handle in `ManagedProcess`.

If any attach/resume step fails, launch fails and Lucarne cleans up temp files. Do not silently run a Windows agent outside the Job Object.

#### Close / kill semantics

- `Process::close()` calls `ManagedProcess::terminate_graceful(pid)`, waits the configured grace period, then calls `ManagedProcess::terminate_force(pid)` if the root process has not exited.
- Windows `terminate_graceful` sends `CTRL_BREAK_EVENT` to the Windows process group. This is the closest Windows console equivalent to a cooperative Unix `SIGTERM` close attempt.
- Windows `terminate_force` calls `TerminateJobObject(job, exit_code)`.
- `ManagedProcess::Drop` closes the job handle; because `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` is set, any remaining descendants are terminated.

This gives Windows the same close/drop operational contract Lucarne expects from Unix process groups: try cooperative shutdown, then prevent orphaned agent/tool subtrees.

#### Interrupt semantics

`Process::signal("SIGINT")` on Windows:

1. Call `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid_as_process_group_id)`.
2. If it succeeds, return `Ok(())`.
3. If it fails, return a runtime error that says Windows console interrupt failed.

Do not silently map failed `SIGINT` to `TerminateJobObject`. Functional parity requires interrupt to mean cancel current turn and keep the session usable when delivery succeeds; termination means end session. Mixing them would corrupt runtime semantics.

`Process::signal("SIGTERM")` and `Process::signal("SIGKILL")` on Windows:

- both call `TerminateJobObject`.

`Process::signal("SIGHUP")` on Windows:

- return unsupported signal error unless a provider requires a mapped semantic later.

#### Why not only Tokio `Child::kill()`?

Tokio kill is child-process level. It does not promise descendant cleanup. Lucarne agents spawn tools/shells; Windows needs Job Objects for reliable tree ownership.

## Process Table / Resource Snapshot Design

Keep resource snapshot code below `host::process_table`.

Shared type:

```rust
pub(crate) struct ProcessSample {
    pid: i32,
    parent_pid: Option<i32>,
    group_id: Option<i32>,
    rss_bytes: u64,
    cpu_percent: f32,
}
```

Aggregation moves out of `core_service/service.rs` into host-aware helper:

```rust
pub(crate) fn aggregate_for_root(root_pid: i32, samples: &[ProcessSample]) -> ProcessAggregate;
```

Unix aggregation:

- descendants by parent PID
- plus `group_id == root_pid` to preserve current process-group semantics

Windows aggregation:

- descendants by parent PID from Toolhelp process snapshot
- no fake process group
- managed Job Object controls cleanup, while resource snapshot reports descendant tree

Windows snapshot collection:

- enumerate processes with `CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS)` and `Process32FirstW/Process32NextW`
- get parent PID from `PROCESSENTRY32W.th32ParentProcessID`
- open each process with query rights
- memory: `GetProcessMemoryInfo` / `PROCESS_MEMORY_COUNTERS.WorkingSetSize`
- CPU: `GetProcessTimes`; keep previous `(kernel+user time, observed_at)` in `ProcessSampler` to compute percent between snapshots

No `sysinfo` dependency needed for first complete design. Use Windows APIs already exposed by `windows-sys`, keeping behavior explicit and reducing dependency surface.

## Writer PID / File User Design

`observed_session_writer_pid(path)` moves to `host::file_users`.

Unix:

- existing `/usr/sbin/lsof -t -- <path>` parser

Windows:

- `RmStartSession`
- `RmRegisterResources` with the absolute session file path
- `RmGetList` to retrieve `RM_PROCESS_INFO`
- choose the first PID that is positive and not current process
- `RmEndSession` in all paths

If Restart Manager returns access/privilege errors, return `None` and trace the error. Do not panic. This mirrors current Unix behavior where `lsof` failure returns absence.

## Path Design

`host::paths` owns Lucarne host paths.

Home resolution:

- Unix: `$HOME`
- Windows: `$HOME`, then `%USERPROFILE%`, then `%HOMEDRIVE%%HOMEPATH%`

Lucarne defaults:

- Unix keeps current `~/.lucarned` behavior.
- Windows uses `%LOCALAPPDATA%\lucarned` for state/logs and `%APPDATA%\lucarned` for config when available.
- If `%LOCALAPPDATA%` or `%APPDATA%` is absent, fall back to `%USERPROFILE%\.lucarned`.

`~` expansion uses `home_dir()` only. Config defaults shown to users should be rendered as actual platform paths, not Unix-only strings.

All production `HOME` reads must be audited. Direct `std::env::var("HOME")` / `var_os("HOME")` is allowed only inside platform home helpers or Unix-only tests. Known production call sites to move behind platform home helpers:

- `agent-sessions/src/providers/pi/discovery.rs`
- `crates/lucarne/src/adapters/claude.rs`
- `crates/lucarne/src/testing/live/providers.rs` when compiled for Windows live harnesses
- `crates/lucarne-adapter/src/lib.rs` adapter config path expansion
- `crates/lucarned/src/main.rs` config `~` expansion

## Watch Design

Keep `agent-sessions::SessionWatcher` using `notify::recommended_watcher`.

macOS currently has two functional watch layers:

1. recursive root watch through Lucarne's FSEvents wrapper
2. direct non-recursive watches for hot/recent session files

The second layer is required. It was added after a live missed-append bug where recursive FSEvents alone did not surface an existing Codex JSONL append.

Windows should use the same two-layer functional model:

1. recursive root watch through `notify::RecommendedWatcher` / `ReadDirectoryChangesW` with `RecursiveMode::Recursive`
2. direct non-recursive watches for hot/recent session files

Do not emulate macOS internals and do not change macOS behavior. Keep macOS on `MacRecursiveWatcher` for recursive roots and keep its hot/recent file target logic unchanged. Add a Windows-only recursive branch that calls `notify::RecommendedWatcher.watch(path, RecursiveMode::Recursive)`. Linux and other non-macOS/non-Windows platforms must keep the existing non-recursive directory discovery behavior.

Do preserve behavior: new nested session files are seen through root recursive watch on platforms with native recursive roots, while active/recent appends are protected by direct file watches. Windows does not have Linux-style per-file inotify watch pressure for a recursive root, but direct hot/recent file watches still consume handles, so the direct-watch set must remain bounded to hot/recent sessions rather than all history.

Windows smoke coverage:

- create watched root
- create provider-like JSONL file
- append complete line
- rename temp file into place
- create/update a session file in a nested directory that was not separately pre-watched
- assert updates arrive within timeout
- verify canonicalized path handling does not break on drive letters/case

Only if this fails should `agent-sessions/src/watch` get Windows-specific event normalization. That normalization must stay watch/provider-contract driven, not provider-id branching in common history code.

## Test Harness Design

The test harness must not assume Unix.

- Replace shell wrappers in cross-platform tests with `lucarne-fakeagent` binaries or platform-specific wrapper helpers.
- Unix executable-bit tests stay `#[cfg(unix)]`.
- Symlink tests are `#[cfg(unix)]` unless Windows symlink privileges are explicitly configured and tested.
- `lucarne-fakeagent` must remove `nix` from Windows builds or move its signal-handler code behind `#[cfg(unix)]`; Windows fakeagent should handle stdin-driven exit and process kill tests through Tokio/standard APIs.

## Cargo Design

`crates/lucarne`:

```toml
[target.'cfg(unix)'.dependencies]
nix = { workspace = true }

[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.61", features = [
  "Win32_Foundation",
  "Win32_Security",
  "Win32_System_Console",
  "Win32_System_Diagnostics_ToolHelp",
  "Win32_System_JobObjects",
  "Win32_System_ProcessStatus",
  "Win32_System_RestartManager",
  "Win32_System_Threading",
] }
```

`crates/lucarne-fakeagent`:

```toml
[target.'cfg(unix)'.dependencies]
nix = { workspace = true }
```

Do not move `notify`, `tokio`, `reqwest`, `teloxide`, or `wechat-ilink` behind Windows cfg. They are cross-platform dependencies.

## Verification Requirements

Implementation is not complete until all pass.

macOS:

```bash
cargo +nightly check -Zbuild-dir-new-layout -p lucarned
cargo +nightly test -Zbuild-dir-new-layout -p lucarne
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions --features 'discovery agent_session watch codex claude gemini copilot pi'
```

Windows VM:

```powershell
$env:HTTP_PROXY='http://10.211.55.2:6153'
$env:HTTPS_PROXY='http://10.211.55.2:6153'
$env:PATH = $env:USERPROFILE + '\.cargo\bin;' + $env:PATH
$env:CARGO_TARGET_DIR='C:\lucarne-target'
Set-Location 'Z:\Volumes\Data\opensource\conductor\lucarne\.worktrees\windows-platform-substrate'
cargo +nightly check -Zbuild-dir-new-layout -p lucarned
cargo +nightly build -Zbuild-dir-new-layout -p lucarned
cargo +nightly test -Zbuild-dir-new-layout -p lucarne --lib
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions --features 'discovery agent_session watch codex claude gemini copilot pi'
```

Windows smoke tests:

- launch fakeagent with same cwd/env/stdio behavior as macOS/Unix tests
- verify process assigned to job and suspended process resumes
- send interrupt to fakeagent, assert the fixture observes `SIGINT` intent and process remains available for next scripted output
- trigger interrupt timeout path, assert unresponsive session ends and descendants are cleaned
- spawn fakeagent child process, close workspace, assert child and grandchild die
- run resource snapshot and verify same public fields are populated: process count, CPU percent, memory bytes, identity
- query file user with Restart Manager on an open file and verify best-effort PID behavior
- run watch append/rename/nested-create smoke and verify update contents match macOS/Unix tests
- run `lucarned --help`
- run default config initialization and verify Windows path defaults

## Acceptance Criteria

- No direct `nix` imports compile on Windows.
- No direct `std::os::unix::*` imports compile on Windows.
- `launcher.rs` uses Tokio for spawn/I/O/wait and host module only for OS-specific process control.
- Windows processes are launched suspended, assigned to Job Object, then resumed.
- Windows launch/interrupt/timeout/close/drop behavior matches Lucarne's macOS/Unix functional contract.
- Closing/dropping a Windows `Process` leaves no agent descendants alive.
- Windows interrupt never silently kills a session when console control fails.
- Resource, file-user, watch, and path behavior expose the same Lucarne-level fields/outcomes as macOS/Unix.
- `notify` remains unchanged and has Windows smoke coverage.
- Provider-specific Windows discovery paths, if added, live only under `agent-sessions/src/providers/<provider>/`.
- Windows `cargo check`, `cargo build`, and required smoke tests pass before claiming Windows substrate done.
