# Windows Complete Substrate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Windows host substrate so Lucarne behaves the same as macOS/Unix for launch, interrupt, timeout escalation, close/drop cleanup, resource status, file-user lookup, paths, watch, build, and tests.

**Architecture:** Add crate-private `crate::host` modules for OS behavior. Keep `tokio::process` and `notify` as shared primitives; use platform modules only to implement the same Lucarne-level functions with OS-native mechanisms. Keep provider discovery and transcript behavior inside `agent-sessions` providers.

**Tech Stack:** Rust nightly, Tokio process, notify watcher, `nix` on Unix only, `windows-sys` on Windows only, Win32 Job Objects, Console Control Events, Toolhelp snapshots, ProcessStatus, Restart Manager.

---

## Functional Parity Contract

Implementation tasks must satisfy these user-visible outcomes, regardless of underlying OS mechanism:

| Function | Required outcome on Windows |
| --- | --- |
| Launch agent | Same cwd/env/stdio/root PID behavior exposed to runtime |
| Interrupt turn | Same cancel-current-turn intent; session remains usable when delivered |
| Interrupt timeout | Same escalation after grace; unresponsive session ends and descendants are cleaned |
| Close session | Same final result: live session ends and agent process tree is gone |
| Drop process handle | Same no-intentional-orphan guarantee, enforced on Windows by Job close kill |
| Resource status | Same public fields: process count, CPU percent, memory bytes, identity |
| Observed writer PID | Same best-effort PID result for session file owner/writer |
| History watch | Same append/rename updates through existing `SessionWatcher` |
| Default paths | Platform-native path defaults, not Unix strings on Windows |

Signal names in code are Lucarne control intents. Windows must map them to native primitives that preserve outcomes; do not expose POSIX mechanics as the contract.

## macOS Preservation Guard

All implementation tasks must preserve existing macOS behavior. Windows support is additive unless code is mechanically moved behind a platform boundary. If a task touches shared code, it must prove macOS behavior is unchanged with existing or added tests.

Preserve these macOS contracts:

- launcher: Tokio spawn, env/cwd/stdio, `process_group(0)`, root-PID reactive signal, group close escalation
- paths: `HOME`-based user home and `~/.lucarned` state path
- provider discovery: provider-owned roots and parsing rules unchanged
- resource status: `/bin/ps` parser and process-group/descendant aggregation unchanged
- writer PID: `/usr/sbin/lsof -t -- <path>` best-effort behavior unchanged
- watch: FSEvents recursive roots and hot/recent direct file watches unchanged
- tests: Unix/macOS executable-bit and symlink tests remain Unix/macOS-only, not rewritten for Windows

Cfg rule: add `#[cfg(windows)]` branches. Do not replace macOS-specific branches with broad non-Windows or cross-platform behavior unless the macOS code path remains byte-for-byte equivalent in behavior.

## File Structure

Create and own these files:

- `crates/lucarne/src/host/mod.rs` — crate-private host module root.
- `crates/lucarne/src/host/paths.rs` — platform home/state path defaults.
- `crates/lucarne/src/host/process/mod.rs` — shared process-control facade.
- `crates/lucarne/src/host/process/unix.rs` — Unix process groups, signals, pid liveness.
- `crates/lucarne/src/host/process/windows.rs` — Windows Job Object, console break, pid liveness.
- `crates/lucarne/src/host/file_users/mod.rs` — shared file-user facade.
- `crates/lucarne/src/host/file_users/unix.rs` — `lsof` lookup and parser.
- `crates/lucarne/src/host/file_users/windows.rs` — Restart Manager lookup.
- `crates/lucarne/src/host/process_table/mod.rs` — shared process sample/aggregate types.
- `crates/lucarne/src/host/process_table/unix.rs` — `/bin/ps` snapshot and parser.
- `crates/lucarne/src/host/process_table/windows.rs` — Toolhelp/ProcessStatus/GetProcessTimes snapshot.
- `crates/lucarne-fakeagent/src/signals.rs` — fakeagent signal abstraction.
- `crates/lucarne/src/launcher.rs` — integrate host process lifecycle.
- `crates/lucarne/src/adapters/claude.rs` — remove direct production `HOME` read from Claude binary lookup.
- `crates/lucarned/src/main.rs` — remove direct production `HOME` read from config `~` expansion.
- `crates/lucarne-adapter/src/lib.rs` — remove direct production `HOME` read from adapter config path expansion.
- `agent-sessions/src/paths.rs` — harden shared provider home resolution for Windows and empty env values.
- `agent-sessions/src/providers/pi/discovery.rs` — use provider path helper instead of direct `HOME`.
- `agent-sessions/src/watch/mod.rs` — enable Windows native recursive root watch while keeping hot/recent file watches.
- `crates/lucarne/src/core_service/mod.rs` — delegate default paths.
- `crates/lucarne/src/core_service/service.rs` — delegate resource/process/file-user helpers.
- `crates/lucarne/src/testing/live/providers.rs` — use host lifecycle for preflight children.
- `crates/lucarne/tests/common/mod.rs` — Windows fakeagent path and script wrappers.
- `crates/lucarne/tests/common/agent_runtime.rs` — Windows version wrapper.
- `crates/lucarne/tests/claude_scenarios.rs` — cfg Unix shell wrapper test.
- `agent-sessions/tests/watch_live.rs` — add rename and nested-create smoke coverage using existing notify watcher.

Do not move provider discovery into `crate::host`. Provider Windows paths, if added in another change, go under `agent-sessions/src/providers/<provider>/discovery.rs`.

---

## Task 1: Add host module and platform path defaults

**Files:**
- Modify: `crates/lucarne/Cargo.toml`
- Modify: `crates/lucarne/src/lib.rs`
- Create: `crates/lucarne/src/host/mod.rs`
- Create: `crates/lucarne/src/host/paths.rs`
- Modify: `crates/lucarne/src/core_service/mod.rs`

- [ ] **Step 1: Move `nix` to Unix target deps and add Windows APIs**

Edit `crates/lucarne/Cargo.toml` so `[dependencies]` no longer contains `nix = { workspace = true }`. Add these target sections below `[dependencies]`:

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

- [ ] **Step 2: Register host module**

In `crates/lucarne/src/lib.rs`, add this private module beside other crate modules:

```rust
pub(crate) mod host;
```

- [ ] **Step 3: Create host module root**

Create `crates/lucarne/src/host/mod.rs`:

```rust
pub(crate) mod paths;
```

- [ ] **Step 4: Create path defaults with unit tests**

Create `crates/lucarne/src/host/paths.rs`:

```rust
use std::path::PathBuf;

pub(crate) fn default_lucarned_home_dir() -> Option<PathBuf> {
    default_lucarned_home_dir_from_env(EnvReader)
}

#[cfg(unix)]
fn default_lucarned_home_dir_from_env(env: impl Env) -> Option<PathBuf> {
    env.var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(".lucarned"))
}

#[cfg(windows)]
fn default_lucarned_home_dir_from_env(env: impl Env) -> Option<PathBuf> {
    env.var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|base| base.join("lucarned"))
        .or_else(|| {
            home_dir_from_env(env).map(|home| home.join(".lucarned"))
        })
}

#[cfg(windows)]
fn home_dir_from_env(env: impl Env) -> Option<PathBuf> {
    env.var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env.var_os("USERPROFILE")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| {
            let drive = env.var_os("HOMEDRIVE").filter(|value| !value.is_empty())?;
            let path = env.var_os("HOMEPATH").filter(|value| !value.is_empty())?;
            Some(PathBuf::from(format!(
                "{}{}",
                drive.to_string_lossy(),
                path.to_string_lossy()
            )))
        })
}

pub(crate) fn default_state_db_path() -> Option<PathBuf> {
    default_lucarned_home_dir().map(|home| home.join("state.sqlite3"))
}

trait Env: Copy {
    fn var_os(self, name: &str) -> Option<std::ffi::OsString>;
}

#[derive(Clone, Copy)]
struct EnvReader;

impl Env for EnvReader {
    fn var_os(self, name: &str) -> Option<std::ffi::OsString> {
        std::env::var_os(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::ffi::OsString;

    #[derive(Clone, Copy)]
    struct MapEnv<'a>(&'a BTreeMap<&'a str, &'a str>);

    impl Env for MapEnv<'_> {
        fn var_os(self, name: &str) -> Option<OsString> {
            self.0.get(name).map(OsString::from)
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_default_home_uses_home_dot_lucarned() {
        let env = BTreeMap::from([("HOME", "/home/alice")]);
        let path = default_lucarned_home_dir_from_env(MapEnv(&env)).expect("path");
        assert_eq!(path, PathBuf::from("/home/alice/.lucarned"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_default_home_prefers_local_app_data() {
        let env = BTreeMap::from([
            ("LOCALAPPDATA", r"C:\Users\alice\AppData\Local"),
            ("USERPROFILE", r"C:\Users\alice"),
        ]);
        let path = default_lucarned_home_dir_from_env(MapEnv(&env)).expect("path");
        assert_eq!(path, PathBuf::from(r"C:\Users\alice\AppData\Local\lucarned"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_default_home_falls_back_to_user_profile() {
        let env = BTreeMap::from([("USERPROFILE", r"C:\Users\alice")]);
        let path = default_lucarned_home_dir_from_env(MapEnv(&env)).expect("path");
        assert_eq!(path, PathBuf::from(r"C:\Users\alice\.lucarned"));
    }
}
```

- [ ] **Step 5: Delegate core path exports**

Replace `default_lucarned_home_dir` and `default_state_db_path` in `crates/lucarne/src/core_service/mod.rs` with:

```rust
pub fn default_lucarned_home_dir() -> Option<PathBuf> {
    crate::host::paths::default_lucarned_home_dir()
}

pub fn default_state_db_path() -> Option<PathBuf> {
    crate::host::paths::default_state_db_path()
}
```

Replace its path test with:

```rust
#[cfg(test)]
mod tests {
    use super::default_state_db_path;

    #[test]
    fn default_state_db_path_ends_with_state_sqlite3() {
        let path = default_state_db_path().expect("state db path");
        assert_eq!(path.file_name().unwrap(), "state.sqlite3");
    }
}
```

- [ ] **Step 6: Verify Task 1**

Run:

```bash
cargo +nightly check -Zbuild-dir-new-layout -p lucarne
cargo +nightly test -Zbuild-dir-new-layout -p lucarne host::paths core_service::tests::default_state_db_path_ends_with_state_sqlite3
```

Expected: both commands pass on macOS. On Windows this task can still fail at existing `nix`/Unix call sites; those are removed in later tasks.

- [ ] **Step 7: Commit Task 1**

```bash
git add crates/lucarne/Cargo.toml crates/lucarne/src/lib.rs crates/lucarne/src/host/mod.rs crates/lucarne/src/host/paths.rs crates/lucarne/src/core_service/mod.rs
git commit -m "feat: add host path defaults"
```

---

## Task 1.5: Remove direct production HOME assumptions

**Files:**
- Modify: `agent-sessions/src/paths.rs`
- Modify: `agent-sessions/src/providers/pi/discovery.rs`
- Modify: `crates/lucarne/src/host/paths.rs`
- Modify: `crates/lucarne/src/adapters/claude.rs`
- Modify: `crates/lucarned/src/main.rs`
- Modify: `crates/lucarne-adapter/src/lib.rs`

- [ ] **Step 1: Harden agent-sessions home helper**

In `agent-sessions/src/paths.rs`, make `home_dir()` filter empty values and keep Windows fallback centralized:

```rust
pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(windows_home_dir)
}

#[cfg(windows)]
fn windows_home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            let drive = std::env::var_os("HOMEDRIVE").filter(|value| !value.is_empty())?;
            let path = std::env::var_os("HOMEPATH").filter(|value| !value.is_empty())?;
            Some(PathBuf::from(format!(
                "{}{}",
                drive.to_string_lossy(),
                path.to_string_lossy()
            )))
        })
}
```

Keep `#[cfg(not(windows))] fn windows_home_dir() -> Option<PathBuf> { None }`.

- [ ] **Step 2: Move Pi discovery to provider home helper**

Replace `pi_sessions_root` in `agent-sessions/src/providers/pi/discovery.rs` with:

```rust
fn pi_sessions_root() -> Option<PathBuf> {
    let root = crate::paths::home_dir()?
        .join(".pi")
        .join("agent")
        .join("sessions");
    if root.is_dir() { Some(root) } else { None }
}
```

- [ ] **Step 3: Expose Lucarne host user home helper inside lucarne crate**

In `crates/lucarne/src/host/paths.rs`, add a user-home helper beside default path helpers:

```rust
pub(crate) fn home_dir() -> Option<PathBuf> {
    home_dir_from_env(EnvReader)
}

#[cfg(unix)]
fn home_dir_from_env(env: impl Env) -> Option<PathBuf> {
    env.var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
```

Keep the Windows `home_dir_from_env` from Task 1 and call it from `default_lucarned_home_dir_from_env` instead of duplicating Windows home logic.

- [ ] **Step 4: Remove direct HOME read from Claude adapter**

In `crates/lucarne/src/adapters/claude.rs`, replace:

```rust
let home = std::env::var("HOME").ok();
default_claude_binary_from(override_bin.as_deref(), home.as_deref(), |path| {
```

with:

```rust
let home = crate::host::paths::home_dir()
    .map(|path| path.to_string_lossy().into_owned());
default_claude_binary_from(override_bin.as_deref(), home.as_deref(), |path| {
```

- [ ] **Step 5: Remove direct HOME read from lucarne-adapter path expansion**

In `crates/lucarne-adapter/src/lib.rs`, replace `home_dir()` with Windows-aware fallback:

```rust
fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .filter(|value| !value.is_empty())
                .map(std::path::PathBuf::from)
        })
        .or_else(|| {
            let drive = std::env::var_os("HOMEDRIVE").filter(|value| !value.is_empty())?;
            let path = std::env::var_os("HOMEPATH").filter(|value| !value.is_empty())?;
            Some(std::path::PathBuf::from(format!(
                "{}{}",
                drive.to_string_lossy(),
                path.to_string_lossy()
            )))
        })
}
```

- [ ] **Step 6: Remove direct HOME read from lucarned config expansion**

In `crates/lucarned/src/main.rs`, add helper near `expand_home_path`:

```rust
fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| {
            let drive = std::env::var_os("HOMEDRIVE").filter(|value| !value.is_empty())?;
            let path = std::env::var_os("HOMEPATH").filter(|value| !value.is_empty())?;
            Some(PathBuf::from(format!(
                "{}{}",
                drive.to_string_lossy(),
                path.to_string_lossy()
            )))
        })
}
```

Replace both `std::env::var_os("HOME")` blocks inside `expand_home_path` with `user_home_dir()`.

- [ ] **Step 7: Verify Task 1.5**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions paths
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions providers::pi::discovery
cargo +nightly test -Zbuild-dir-new-layout -p lucarne host::paths
cargo +nightly test -Zbuild-dir-new-layout -p lucarne adapters::claude
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-adapter expand_home_path
cargo +nightly test -Zbuild-dir-new-layout -p lucarned expand_home_path
```

Expected: tests pass on macOS. Windows VM should pass with `HOME` absent and `USERPROFILE` set.

- [ ] **Step 8: Commit Task 1.5**

```bash
git add agent-sessions/src/paths.rs agent-sessions/src/providers/pi/discovery.rs crates/lucarne/src/host/paths.rs crates/lucarne/src/adapters/claude.rs crates/lucarne-adapter/src/lib.rs crates/lucarned/src/main.rs
git commit -m "fix: use platform home resolution"
```

---

## Task 2: Add host process lifecycle and integrate launcher

**Files:**
- Modify: `crates/lucarne/src/host/mod.rs`
- Create: `crates/lucarne/src/host/process/mod.rs`
- Create: `crates/lucarne/src/host/process/unix.rs`
- Create: `crates/lucarne/src/host/process/windows.rs`
- Modify: `crates/lucarne/src/launcher.rs`

- [ ] **Step 1: Expose process host module**

Edit `crates/lucarne/src/host/mod.rs`:

```rust
pub(crate) mod paths;
pub(crate) mod process;
```

- [ ] **Step 2: Create shared process facade**

Create `crates/lucarne/src/host/process/mod.rs`:

```rust
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(crate) use unix::{configure_command, pid_is_alive, ManagedProcess};
#[cfg(windows)]
pub(crate) use windows::{configure_command, pid_is_alive, ManagedProcess};
```

- [ ] **Step 3: Add Unix process implementation**

Create `crates/lucarne/src/host/process/unix.rs`:

```rust
use crate::error::{LucarneError, Result};
use nix::errno::Errno;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use tokio::process::{Child, Command};

#[derive(Debug, Default)]
pub(crate) struct ManagedProcess;

pub(crate) fn configure_command(command: &mut Command) {
    command.process_group(0);
}

impl ManagedProcess {
    pub(crate) fn attach(_child: &Child) -> Result<Self> {
        Ok(Self)
    }

    pub(crate) fn signal(&self, pid: i32, name: &str) -> Result<()> {
        let signal = signal_from_name(name)?;
        signal::kill(Pid::from_raw(pid), signal)
            .map_err(|err| LucarneError::runtime(format!("kill {name}: {err}")))
    }

    pub(crate) fn terminate_graceful(&self, pid: i32) -> Result<()> {
        signal::kill(Pid::from_raw(-pid), Signal::SIGTERM)
            .map_err(|err| LucarneError::runtime(format!("kill SIGTERM: {err}")))
    }

    pub(crate) fn terminate_force(&self, pid: i32) -> Result<()> {
        signal::kill(Pid::from_raw(-pid), Signal::SIGKILL)
            .map_err(|err| LucarneError::runtime(format!("kill SIGKILL: {err}")))
    }
}

fn signal_from_name(name: &str) -> Result<Signal> {
    match name {
        "SIGINT" => Ok(Signal::SIGINT),
        "SIGTERM" => Ok(Signal::SIGTERM),
        "SIGKILL" => Ok(Signal::SIGKILL),
        "SIGHUP" => Ok(Signal::SIGHUP),
        other => Err(LucarneError::runtime(format!("unknown signal {other}"))),
    }
}

pub(crate) fn pid_is_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    match signal::kill(Pid::from_raw(pid), None) {
        Ok(()) => true,
        Err(Errno::EPERM) => true,
        Err(_) => false,
    }
}
```

- [ ] **Step 4: Add Windows process implementation**

Create `crates/lucarne/src/host/process/windows.rs`:

```rust
use crate::error::{LucarneError, Result};
use std::io;
use std::mem;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::ptr;
use tokio::process::{Child, Command};
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_ACCESS_DENIED, FALSE, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE,
};
use windows_sys::Win32::System::Console::{GenerateConsoleCtrlEvent, CTRL_BREAK_EVENT};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, THREADENTRY32, TH32CS_SNAPTHREAD,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, OpenThread, ResumeThread, CREATE_NEW_PROCESS_GROUP,
    CREATE_SUSPENDED, PROCESS_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
};

#[derive(Debug)]
pub(crate) struct ManagedProcess {
    job: OwnedHandle,
}

unsafe impl Send for ManagedProcess {}
unsafe impl Sync for ManagedProcess {}

pub(crate) fn configure_command(command: &mut Command) {
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_SUSPENDED);
}

impl ManagedProcess {
    pub(crate) fn attach(child: &Child) -> Result<Self> {
        let pid = child
            .id()
            .ok_or_else(|| LucarneError::launcher("windows child has no pid"))?;
        let process = child
            .raw_handle()
            .ok_or_else(|| LucarneError::launcher("windows child has no process handle"))?
            as HANDLE;
        let job = create_job_object()?;
        let managed = Self { job };
        bool_result(
            unsafe { AssignProcessToJobObject(managed.job_handle(), process) },
            "AssignProcessToJobObject",
        )?;
        resume_child_threads(pid)?;
        Ok(managed)
    }

    pub(crate) fn signal(&self, pid: i32, name: &str) -> Result<()> {
        match name {
            "SIGINT" => self.console_break(pid),
            "SIGTERM" | "SIGKILL" => self.terminate_tree(1),
            "SIGHUP" => Err(LucarneError::runtime(
                "signal SIGHUP is unsupported on Windows".to_string(),
            )),
            other => Err(LucarneError::runtime(format!("unknown signal {other}"))),
        }
    }

    pub(crate) fn terminate_graceful(&self, pid: i32) -> Result<()> {
        self.console_break(pid)
    }

    pub(crate) fn terminate_force(&self, _pid: i32) -> Result<()> {
        self.terminate_tree(1)
    }

    fn terminate_tree(&self, exit_code: u32) -> Result<()> {
        bool_result(
            unsafe { TerminateJobObject(self.job_handle(), exit_code) },
            "TerminateJobObject",
        )
    }

    fn console_break(&self, pid: i32) -> Result<()> {
        if pid <= 0 {
            return Err(LucarneError::runtime(format!(
                "invalid Windows process group id {pid}"
            )));
        }
        bool_result(
            unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid as u32) },
            "GenerateConsoleCtrlEvent CTRL_BREAK_EVENT",
        )
    }

    fn job_handle(&self) -> HANDLE {
        self.job.as_raw_handle() as HANDLE
    }
}

fn create_job_object() -> Result<OwnedHandle> {
    let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
    if job.is_null() {
        return Err(last_os_error("CreateJobObjectW"));
    }
    let job = unsafe { OwnedHandle::from_raw_handle(job as RawHandle) };
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    bool_result(
        unsafe {
            SetInformationJobObject(
                job.as_raw_handle() as HANDLE,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        },
        "SetInformationJobObject JobObjectExtendedLimitInformation",
    )?;
    Ok(job)
}

fn resume_child_threads(pid: u32) -> Result<()> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(last_os_error("CreateToolhelp32Snapshot TH32CS_SNAPTHREAD"));
    }
    let _snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot as RawHandle) };
    let mut entry = THREADENTRY32 {
        dwSize: mem::size_of::<THREADENTRY32>() as u32,
        ..THREADENTRY32::default()
    };
    let mut has_entry = unsafe { Thread32First(snapshot, &mut entry) } != FALSE;
    while has_entry {
        if entry.th32OwnerProcessID == pid {
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, FALSE, entry.th32ThreadID) };
            if !thread.is_null() {
                let thread = unsafe { OwnedHandle::from_raw_handle(thread as RawHandle) };
                let resume_result = unsafe { ResumeThread(thread.as_raw_handle() as HANDLE) };
                if resume_result == u32::MAX {
                    return Err(last_os_error("ResumeThread"));
                }
            }
        }
        has_entry = unsafe { Thread32Next(snapshot, &mut entry) } != FALSE;
    }
    Ok(())
}

pub(crate) fn pid_is_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid as u32) };
    if handle.is_null() {
        return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle as RawHandle) };
    let mut exit_code = 0u32;
    if unsafe { GetExitCodeProcess(handle.as_raw_handle() as HANDLE, &mut exit_code) } == FALSE {
        return false;
    }
    exit_code == STILL_ACTIVE as u32
}

fn bool_result(value: i32, context: &str) -> Result<()> {
    if value == FALSE {
        Err(last_os_error(context))
    } else {
        Ok(())
    }
}

fn last_os_error(context: &str) -> LucarneError {
    LucarneError::runtime(format!("{context}: {}", io::Error::last_os_error()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn suspended_child_is_assigned_to_job_and_resumed() {
        let mut command = Command::new("cmd.exe");
        command.args(["/C", "echo lucarne-job-ok"]);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        configure_command(&mut command);
        let mut child = command.spawn().expect("spawn cmd");
        let _managed = ManagedProcess::attach(&child).expect("attach job");
        let mut stdout = String::new();
        child
            .stdout
            .take()
            .expect("stdout")
            .read_to_string(&mut stdout)
            .await
            .expect("read stdout");
        let status = child.wait().await.expect("wait child");
        assert!(status.success());
        assert!(stdout.contains("lucarne-job-ok"));
    }

    #[tokio::test]
    async fn terminate_job_kills_grandchild() {
        let temp = tempdir().expect("tempdir");
        let pid_file = temp.path().join("grandchild.pid");
        let script = format!(
            "$p = Start-Process -PassThru powershell -ArgumentList '-NoProfile','-Command','Start-Sleep -Seconds 30'; Set-Content -LiteralPath '{}' -Value $p.Id; Start-Sleep -Seconds 30",
            pid_file.display()
        );
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-Command", &script]);
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        configure_command(&mut command);
        let mut child = command.spawn().expect("spawn powershell");
        let managed = ManagedProcess::attach(&child).expect("attach job");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !pid_file.exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let grandchild_pid = fs::read_to_string(&pid_file)
            .expect("read pid")
            .trim()
            .parse::<i32>()
            .expect("parse pid");
        assert!(pid_is_alive(grandchild_pid));
        managed.terminate_tree(1).expect("terminate job");
        let _ = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .expect("wait timeout")
            .expect("wait child");
        let deadline = Instant::now() + Duration::from_secs(5);
        while pid_is_alive(grandchild_pid) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(!pid_is_alive(grandchild_pid));
    }
}
```

- [ ] **Step 5: Integrate launcher process ownership**

In `crates/lucarne/src/launcher.rs`, add field to `ProcessInner`:

```rust
host_process: crate::host::process::ManagedProcess,
```

Replace `Process::signal` body after `pid <= 0` guard with:

```rust
debug!(
    target: "lucarne::launcher",
    pid = self.inner.pid,
    signal = name,
    "sending signal to managed process"
);
self.inner.host_process.signal(self.inner.pid, name)
```

Replace the first Unix `SIGTERM` block in `close` with:

```rust
if self.inner.pid > 0 {
    let _ = self.inner.host_process.terminate_graceful(self.inner.pid);
}
```

Replace the later Unix `SIGKILL` block in `close` with:

```rust
if self.inner.pid > 0 {
    warn!(
        target: "lucarne::launcher",
        pid = self.inner.pid,
        "process did not exit during grace period; forcing termination"
    );
    let _ = self.inner.host_process.terminate_force(self.inner.pid);
}
```

Remove this Unix-only block before spawn:

```rust
{
    use std::os::unix::process::CommandExt;
    cmd.as_std_mut().process_group(0);
}
```

Add host configure before spawn:

```rust
crate::host::process::configure_command(&mut cmd);
```

Add host attach after pid log and before taking stdio:

```rust
let host_process = crate::host::process::ManagedProcess::attach(&child).map_err(|e| {
    for p in &cleanup_paths {
        let _ = std::fs::remove_file(p);
    }
    e
})?;
```

Add field when building `ProcessInner`:

```rust
host_process,
```

- [ ] **Step 6: Verify Task 2 on macOS**

Run:

```bash
cargo +nightly check -Zbuild-dir-new-layout -p lucarne
cargo +nightly test -Zbuild-dir-new-layout -p lucarne launcher::tests host::process
```

Expected: pass on macOS. If no `launcher::tests` target exists, command still runs matching unit tests and reports pass.

- [ ] **Step 7: Verify Task 2 on Windows**

Run in Windows VM:

```powershell
$env:HTTP_PROXY='http://10.211.55.2:6153'
$env:HTTPS_PROXY='http://10.211.55.2:6153'
$env:PATH = $env:USERPROFILE + '\.cargo\bin;' + $env:PATH
$env:CARGO_TARGET_DIR='C:\lucarne-target'
Set-Location 'Z:\Volumes\Data\opensource\conductor\lucarne\.worktrees\windows-platform-substrate'
cargo +nightly test -Zbuild-dir-new-layout -p lucarne host::process --lib
```

Expected: Windows process tests pass, proving Job Object attach/resume and descendant termination.

- [ ] **Step 8: Commit Task 2**

```bash
git add crates/lucarne/src/host/mod.rs crates/lucarne/src/host/process crates/lucarne/src/launcher.rs
git commit -m "feat: manage processes through host lifecycle"
```

---

## Task 3: Move pid liveness and file-user lookup behind host boundary

**Files:**
- Modify: `crates/lucarne/src/host/mod.rs`
- Create: `crates/lucarne/src/host/file_users/mod.rs`
- Create: `crates/lucarne/src/host/file_users/unix.rs`
- Create: `crates/lucarne/src/host/file_users/windows.rs`
- Modify: `crates/lucarne/src/core_service/service.rs`

- [ ] **Step 1: Expose file-user module**

Edit `crates/lucarne/src/host/mod.rs`:

```rust
pub(crate) mod file_users;
pub(crate) mod paths;
pub(crate) mod process;
```

- [ ] **Step 2: Create shared file-user facade**

Create `crates/lucarne/src/host/file_users/mod.rs`:

```rust
use std::path::Path;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

pub(crate) fn observed_session_writer_pid(path: &Path) -> Option<i32> {
    platform_observed_session_writer_pid(path)
}

#[cfg(unix)]
fn platform_observed_session_writer_pid(path: &Path) -> Option<i32> {
    unix::observed_session_writer_pid(path)
}

#[cfg(windows)]
fn platform_observed_session_writer_pid(path: &Path) -> Option<i32> {
    windows::observed_session_writer_pid(path)
}
```

- [ ] **Step 3: Add Unix `lsof` lookup**

Create `crates/lucarne/src/host/file_users/unix.rs`:

```rust
use std::path::Path;

pub(crate) fn observed_session_writer_pid(path: &Path) -> Option<i32> {
    let output = std::process::Command::new("/usr/sbin/lsof")
        .args(["-t", "--"])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_lsof_pid_output(&output.stdout)
}

fn parse_lsof_pid_output(stdout: &[u8]) -> Option<i32> {
    let current_pid = std::process::id() as i32;
    String::from_utf8_lossy(stdout).lines().find_map(|line| {
        let pid = line.trim().parse::<i32>().ok()?;
        (pid > 0 && pid != current_pid).then_some(pid)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lsof_pid_output_skips_current_process() {
        let current = std::process::id();
        let raw = format!("{current}\n4242\n");
        assert_eq!(parse_lsof_pid_output(raw.as_bytes()), Some(4242));
    }
}
```

- [ ] **Step 4: Add Windows Restart Manager lookup**

Create `crates/lucarne/src/host/file_users/windows.rs`:

```rust
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use tracing::trace;
use windows_sys::Win32::Foundation::ERROR_MORE_DATA;
use windows_sys::Win32::System::RestartManager::{
    RmEndSession, RmGetList, RmRegisterResources, RmStartSession, RM_PROCESS_INFO,
    CCH_RM_SESSION_KEY,
};

pub(crate) fn observed_session_writer_pid(path: &Path) -> Option<i32> {
    restart_manager_file_users(path).ok().and_then(|pids| {
        let current = std::process::id() as i32;
        pids.into_iter().find(|pid| *pid > 0 && *pid != current)
    })
}

fn restart_manager_file_users(path: &Path) -> Result<Vec<i32>, u32> {
    let mut handle = 0u32;
    let mut key = vec![0u16; CCH_RM_SESSION_KEY as usize + 1];
    let result = unsafe { RmStartSession(&mut handle, 0, key.as_mut_ptr()) };
    if result != 0 {
        trace!(target: "lucarne::host::file_users", error = result, "RmStartSession failed");
        return Err(result);
    }
    let session = RestartManagerSession(handle);
    let path_wide = wide_null(path.as_os_str());
    let files = [path_wide.as_ptr()];
    let result = unsafe {
        RmRegisterResources(
            session.0,
            1,
            files.as_ptr(),
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
        )
    };
    if result != 0 {
        trace!(target: "lucarne::host::file_users", error = result, "RmRegisterResources failed");
        return Err(result);
    }

    let mut needed = 0u32;
    let mut count = 0u32;
    let mut reboot_reasons = 0u32;
    let result = unsafe {
        RmGetList(
            session.0,
            &mut needed,
            &mut count,
            std::ptr::null_mut(),
            &mut reboot_reasons,
        )
    };
    if result != ERROR_MORE_DATA && result != 0 {
        trace!(target: "lucarne::host::file_users", error = result, "RmGetList sizing failed");
        return Err(result);
    }
    if needed == 0 {
        return Ok(Vec::new());
    }

    let mut processes = vec![RM_PROCESS_INFO::default(); needed as usize];
    count = needed;
    let result = unsafe {
        RmGetList(
            session.0,
            &mut needed,
            &mut count,
            processes.as_mut_ptr(),
            &mut reboot_reasons,
        )
    };
    if result != 0 {
        trace!(target: "lucarne::host::file_users", error = result, "RmGetList failed");
        return Err(result);
    }
    processes.truncate(count as usize);
    Ok(processes
        .into_iter()
        .map(|info| info.Process.dwProcessId as i32)
        .collect())
}

struct RestartManagerSession(u32);

impl Drop for RestartManagerSession {
    fn drop(&mut self) {
        unsafe {
            let _ = RmEndSession(self.0);
        }
    }
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}
```

- [ ] **Step 5: Integrate core service**

In `crates/lucarne/src/core_service/service.rs`, replace `process_id_is_alive` with:

```rust
fn process_id_is_alive(pid: i32) -> bool {
    crate::host::process::pid_is_alive(pid)
}
```

Replace `observed_session_writer_pid` with:

```rust
fn observed_session_writer_pid(path: &Path) -> Option<i32> {
    crate::host::file_users::observed_session_writer_pid(path)
}
```

Remove `parse_lsof_pid_output` from `service.rs`; its unit test now lives in host Unix module.

- [ ] **Step 6: Verify Task 3**

Run:

```bash
cargo +nightly check -Zbuild-dir-new-layout -p lucarne
cargo +nightly test -Zbuild-dir-new-layout -p lucarne host::file_users
```

Windows VM:

```powershell
cargo +nightly check -Zbuild-dir-new-layout -p lucarne
cargo +nightly test -Zbuild-dir-new-layout -p lucarne host::file_users --lib
```

Expected: macOS and Windows pass. Windows Restart Manager returns `None` on inaccessible resources instead of panicking.

- [ ] **Step 7: Commit Task 3**

```bash
git add crates/lucarne/src/host/mod.rs crates/lucarne/src/host/file_users crates/lucarne/src/core_service/service.rs
git commit -m "feat: isolate file user lookup by host"
```

---

## Task 4: Move process table snapshot behind host boundary

**Files:**
- Modify: `crates/lucarne/src/host/mod.rs`
- Create: `crates/lucarne/src/host/process_table/mod.rs`
- Create: `crates/lucarne/src/host/process_table/unix.rs`
- Create: `crates/lucarne/src/host/process_table/windows.rs`
- Modify: `crates/lucarne/src/core_service/service.rs`

- [ ] **Step 1: Expose process table module**

Edit `crates/lucarne/src/host/mod.rs`:

```rust
pub(crate) mod file_users;
pub(crate) mod paths;
pub(crate) mod process;
pub(crate) mod process_table;
```

- [ ] **Step 2: Create shared process table types and aggregation**

Create `crates/lucarne/src/host/process_table/mod.rs`:

```rust
use std::collections::{HashMap, HashSet};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ProcessSample {
    pub(crate) pid: i32,
    pub(crate) parent_pid: Option<i32>,
    pub(crate) group_id: Option<i32>,
    pub(crate) rss_bytes: u64,
    pub(crate) cpu_percent: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ProcessAggregate {
    pub(crate) process_count: usize,
    pub(crate) memory_bytes: u64,
    pub(crate) cpu_percent: f32,
}

pub(crate) async fn snapshot() -> Result<Vec<ProcessSample>, String> {
    platform_snapshot().await
}

#[cfg(unix)]
async fn platform_snapshot() -> Result<Vec<ProcessSample>, String> {
    unix::snapshot().await
}

#[cfg(windows)]
async fn platform_snapshot() -> Result<Vec<ProcessSample>, String> {
    windows::snapshot().await
}

pub(crate) fn aggregate_for_root(root_pid: i32, samples: &[ProcessSample]) -> ProcessAggregate {
    let children_by_parent = children_by_parent(samples);
    let mut pids = descendants_of(root_pid, &children_by_parent);
    pids.insert(root_pid);
    for sample in samples {
        if sample.group_id == Some(root_pid) {
            pids.insert(sample.pid);
        }
    }

    let mut aggregate = ProcessAggregate {
        process_count: 0,
        memory_bytes: 0,
        cpu_percent: 0.0,
    };
    for sample in samples {
        if pids.contains(&sample.pid) {
            aggregate.process_count += 1;
            aggregate.memory_bytes = aggregate.memory_bytes.saturating_add(sample.rss_bytes);
            aggregate.cpu_percent += sample.cpu_percent;
        }
    }
    aggregate
}

fn children_by_parent(samples: &[ProcessSample]) -> HashMap<i32, Vec<i32>> {
    let mut children = HashMap::<i32, Vec<i32>>::new();
    for sample in samples {
        if let Some(parent_pid) = sample.parent_pid {
            children.entry(parent_pid).or_default().push(sample.pid);
        }
    }
    children
}

fn descendants_of(root_pid: i32, children_by_parent: &HashMap<i32, Vec<i32>>) -> HashSet<i32> {
    let mut descendants = HashSet::new();
    let mut stack = children_by_parent
        .get(&root_pid)
        .cloned()
        .unwrap_or_default();
    while let Some(pid) = stack.pop() {
        if !descendants.insert(pid) {
            continue;
        }
        if let Some(children) = children_by_parent.get(&pid) {
            stack.extend(children.iter().copied());
        }
    }
    descendants
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregation_counts_unix_group_and_descendants() {
        let samples = vec![
            ProcessSample { pid: 10, parent_pid: Some(1), group_id: Some(10), rss_bytes: 1024, cpu_percent: 1.0 },
            ProcessSample { pid: 11, parent_pid: Some(10), group_id: Some(10), rss_bytes: 2048, cpu_percent: 2.5 },
            ProcessSample { pid: 12, parent_pid: Some(1), group_id: Some(10), rss_bytes: 4096, cpu_percent: 0.5 },
            ProcessSample { pid: 20, parent_pid: Some(1), group_id: Some(20), rss_bytes: 8192, cpu_percent: 9.0 },
        ];
        let aggregate = aggregate_for_root(10, &samples);
        assert_eq!(aggregate.process_count, 3);
        assert_eq!(aggregate.memory_bytes, 7168);
        assert_eq!(aggregate.cpu_percent, 4.0);
    }

    #[test]
    fn aggregation_counts_windows_descendants_without_group_id() {
        let samples = vec![
            ProcessSample { pid: 10, parent_pid: Some(1), group_id: None, rss_bytes: 1024, cpu_percent: 1.0 },
            ProcessSample { pid: 11, parent_pid: Some(10), group_id: None, rss_bytes: 2048, cpu_percent: 2.0 },
            ProcessSample { pid: 12, parent_pid: Some(11), group_id: None, rss_bytes: 4096, cpu_percent: 3.0 },
            ProcessSample { pid: 99, parent_pid: Some(1), group_id: None, rss_bytes: 8192, cpu_percent: 9.0 },
        ];
        let aggregate = aggregate_for_root(10, &samples);
        assert_eq!(aggregate.process_count, 3);
        assert_eq!(aggregate.memory_bytes, 7168);
        assert_eq!(aggregate.cpu_percent, 6.0);
    }
}
```

- [ ] **Step 3: Add Unix `/bin/ps` snapshot**

Create `crates/lucarne/src/host/process_table/unix.rs`:

```rust
use super::ProcessSample;
use tokio::process::Command;

pub(crate) async fn snapshot() -> Result<Vec<ProcessSample>, String> {
    let output = Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,pgid=,rss=,%cpu="])
        .output()
        .await
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(parse_process_table(&output.stdout))
}

fn parse_process_table(stdout: &[u8]) -> Vec<ProcessSample> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(parse_process_sample)
        .collect()
}

fn parse_process_sample(line: &str) -> Option<ProcessSample> {
    let mut parts = line.split_whitespace();
    let pid = parts.next()?.parse().ok()?;
    let parent_pid = parts.next()?.parse().ok()?;
    let group_id = parts.next()?.parse().ok()?;
    let rss_kib = parts.next()?.parse::<u64>().ok()?;
    let cpu_percent = parts.next()?.parse().ok()?;
    Some(ProcessSample {
        pid,
        parent_pid: Some(parent_pid),
        group_id: Some(group_id),
        rss_bytes: rss_kib.saturating_mul(1024),
        cpu_percent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_process_sample_reads_ps_columns() {
        let sample = parse_process_sample("  10   1  10  42  3.5").expect("sample");
        assert_eq!(sample.pid, 10);
        assert_eq!(sample.parent_pid, Some(1));
        assert_eq!(sample.group_id, Some(10));
        assert_eq!(sample.rss_bytes, 42 * 1024);
        assert_eq!(sample.cpu_percent, 3.5);
    }
}
```

- [ ] **Step 4: Add Windows Toolhelp snapshot**

Create `crates/lucarne/src/host/process_table/windows.rs`:

```rust
use super::ProcessSample;
use std::collections::HashMap;
use std::mem;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use windows_sys::Win32::Foundation::{FALSE, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_VM_READ,
};

pub(crate) async fn snapshot() -> Result<Vec<ProcessSample>, String> {
    tokio::task::spawn_blocking(|| sampler().lock().expect("process sampler lock").sample())
        .await
        .map_err(|err| err.to_string())?
}

fn sampler() -> &'static Mutex<ProcessSampler> {
    static SAMPLER: OnceLock<Mutex<ProcessSampler>> = OnceLock::new();
    SAMPLER.get_or_init(|| Mutex::new(ProcessSampler::default()))
}

#[derive(Default)]
struct ProcessSampler {
    previous: HashMap<i32, ProcessTiming>,
}

#[derive(Clone, Copy)]
struct ProcessTiming {
    ticks_100ns: u64,
    observed_at: Instant,
}

impl ProcessSampler {
    fn sample(&mut self) -> Result<Vec<ProcessSample>, String> {
        let now = Instant::now();
        let entries = process_entries()?;
        let mut next_previous = HashMap::new();
        let mut samples = Vec::with_capacity(entries.len());
        for entry in entries {
            let pid = entry.th32ProcessID as i32;
            let parent_pid = (entry.th32ParentProcessID != 0).then_some(entry.th32ParentProcessID as i32);
            let metrics = query_process_metrics(pid).unwrap_or(ProcessMetrics { rss_bytes: 0, ticks_100ns: None });
            let cpu_percent = metrics
                .ticks_100ns
                .and_then(|ticks| {
                    let previous = self.previous.get(&pid)?;
                    let elapsed = now.duration_since(previous.observed_at).as_secs_f64();
                    if elapsed <= 0.0 || ticks < previous.ticks_100ns {
                        return Some(0.0);
                    }
                    let cpu_seconds = (ticks - previous.ticks_100ns) as f64 / 10_000_000.0;
                    Some((cpu_seconds / elapsed * 100.0) as f32)
                })
                .unwrap_or(0.0);
            if let Some(ticks_100ns) = metrics.ticks_100ns {
                next_previous.insert(pid, ProcessTiming { ticks_100ns, observed_at: now });
            }
            samples.push(ProcessSample {
                pid,
                parent_pid,
                group_id: None,
                rss_bytes: metrics.rss_bytes,
                cpu_percent,
            });
        }
        self.previous = next_previous;
        Ok(samples)
    }
}

struct ProcessMetrics {
    rss_bytes: u64,
    ticks_100ns: Option<u64>,
}

fn process_entries() -> Result<Vec<PROCESSENTRY32W>, String> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let _snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot as RawHandle) };
    let mut entry = PROCESSENTRY32W {
        dwSize: mem::size_of::<PROCESSENTRY32W>() as u32,
        ..PROCESSENTRY32W::default()
    };
    let mut entries = Vec::new();
    let mut has_entry = unsafe { Process32FirstW(snapshot, &mut entry) } != FALSE;
    while has_entry {
        entries.push(entry);
        has_entry = unsafe { Process32NextW(snapshot, &mut entry) } != FALSE;
    }
    Ok(entries)
}

fn query_process_metrics(pid: i32) -> Option<ProcessMetrics> {
    let rights = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_QUERY_INFORMATION | PROCESS_VM_READ;
    let handle = unsafe { OpenProcess(rights, FALSE, pid as u32) };
    if handle.is_null() {
        return None;
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle as RawHandle) };
    let raw = handle.as_raw_handle() as HANDLE;
    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    let rss_bytes = if unsafe {
        GetProcessMemoryInfo(
            raw,
            &mut counters,
            mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    } != FALSE
    {
        counters.WorkingSetSize as u64
    } else {
        0
    };
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let ticks_100ns = if unsafe {
        GetProcessTimes(raw, &mut creation, &mut exit, &mut kernel, &mut user)
    } != FALSE
    {
        Some(filetime_to_u64(kernel).saturating_add(filetime_to_u64(user)))
    } else {
        None
    };
    Some(ProcessMetrics { rss_bytes, ticks_100ns })
}

fn filetime_to_u64(value: FILETIME) -> u64 {
    ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_to_u64_combines_high_and_low_bits() {
        let value = FILETIME { dwLowDateTime: 0x89AB_CDEF, dwHighDateTime: 0x0123_4567 };
        assert_eq!(filetime_to_u64(value), 0x0123_4567_89AB_CDEF);
    }
}
```

- [ ] **Step 5: Integrate core service resource snapshot**

In `crates/lucarne/src/core_service/service.rs`, remove local `ProcessSample`, `ProcessAggregate`, `process_table_snapshot`, `parse_process_table`, `parse_process_sample`, `children_by_parent`, `aggregate_process_group`, and `descendants_of`.

Add imports near existing crate imports:

```rust
use crate::host::process_table::{self, ProcessAggregate, ProcessSample};
```

Replace process table call:

```rust
let samples = process_table::snapshot()
    .await
    .map_err(CoreError::ProcessSnapshot)?;
```

Replace aggregation in `build_agent_resource_snapshot` with:

```rust
let aggregate = target
    .pid
    .map(|pid| process_table::aggregate_for_root(pid, samples))
    .unwrap_or(ProcessAggregate {
        process_count: 0,
        memory_bytes: 0,
        cpu_percent: 0.0,
    });
```

Delete the existing unit test `agent_resource_aggregation_counts_process_group_and_descendants`; it now lives as `host::process_table::tests::aggregation_counts_unix_group_and_descendants`.

- [ ] **Step 6: Verify Task 4**

Run:

```bash
cargo +nightly check -Zbuild-dir-new-layout -p lucarne
cargo +nightly test -Zbuild-dir-new-layout -p lucarne host::process_table
cargo +nightly test -Zbuild-dir-new-layout -p lucarne opened_workspace_uses_runtime_process_id_for_resource_status
```

Windows VM:

```powershell
cargo +nightly check -Zbuild-dir-new-layout -p lucarne
cargo +nightly test -Zbuild-dir-new-layout -p lucarne host::process_table --lib
```

Expected: pass. First Windows process sample may report `cpu_percent == 0.0`; second sample can compute delta.

- [ ] **Step 7: Commit Task 4**

```bash
git add crates/lucarne/src/host/mod.rs crates/lucarne/src/host/process_table crates/lucarne/src/core_service/service.rs
git commit -m "feat: isolate process snapshots by host"
```

---

## Task 5: Make fakeagent and test harness compile on Windows

**Files:**
- Modify: `crates/lucarne-fakeagent/Cargo.toml`
- Create: `crates/lucarne-fakeagent/src/signals.rs`
- Modify: `crates/lucarne-fakeagent/src/main.rs`
- Modify: `crates/lucarne/src/testing/live/providers.rs`
- Modify: `crates/lucarne/tests/common/mod.rs`
- Modify: `crates/lucarne/tests/common/agent_runtime.rs`
- Modify: `crates/lucarne/tests/claude_scenarios.rs`
- Modify: `crates/lucarne/tests/live_unit.rs`
- Modify: `crates/lucarne/src/testing/live/common.rs`
- Modify: `crates/lucarne/src/testing/live/recording.rs`

- [ ] **Step 1: Split fakeagent target dependencies**

Replace `crates/lucarne-fakeagent/Cargo.toml` dependencies with:

```toml
[dependencies]

[target.'cfg(unix)'.dependencies]
nix = { workspace = true }

[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.61", features = [
  "Win32_Foundation",
  "Win32_System_Console",
] }
```

- [ ] **Step 2: Add fakeagent signal module**

Create `crates/lucarne-fakeagent/src/signals.rs`:

```rust
use std::sync::atomic::{AtomicUsize, Ordering};

static SIGINT_COUNT: AtomicUsize = AtomicUsize::new(0);
static SIGTERM_COUNT: AtomicUsize = AtomicUsize::new(0);
static SIGHUP_COUNT: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn normalize_signal_name(raw: &str) -> Option<&'static str> {
    match raw.trim() {
        "SIGINT" | "INT" => Some("SIGINT"),
        "SIGTERM" | "TERM" => Some("SIGTERM"),
        "SIGHUP" | "HUP" => Some("SIGHUP"),
        _ => None,
    }
}

pub(crate) fn signal_counter(name: &'static str) -> &'static AtomicUsize {
    match name {
        "SIGINT" => &SIGINT_COUNT,
        "SIGTERM" => &SIGTERM_COUNT,
        "SIGHUP" => &SIGHUP_COUNT,
        other => panic!("missing counter for {other}"),
    }
}

#[cfg(unix)]
pub(crate) fn install_signal_handlers() {
    use nix::sys::signal::{self, SigHandler, Signal};
    for signal in [Signal::SIGINT, Signal::SIGTERM, Signal::SIGHUP] {
        unsafe {
            signal::signal(signal, SigHandler::Handler(handle_signal))
                .unwrap_or_else(|err| panic!("install signal handler for {signal:?}: {err}"));
        }
    }
}

#[cfg(unix)]
extern "C" fn handle_signal(sig: i32) {
    use nix::sys::signal::Signal;
    match sig {
        x if x == Signal::SIGINT as i32 => {
            SIGINT_COUNT.fetch_add(1, Ordering::SeqCst);
        }
        x if x == Signal::SIGTERM as i32 => {
            SIGTERM_COUNT.fetch_add(1, Ordering::SeqCst);
        }
        x if x == Signal::SIGHUP as i32 => {
            SIGHUP_COUNT.fetch_add(1, Ordering::SeqCst);
        }
        _ => {}
    }
}

#[cfg(windows)]
pub(crate) fn install_signal_handlers() {
    use windows_sys::Win32::Foundation::FALSE;
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    let ok = unsafe { SetConsoleCtrlHandler(Some(handle_console_control), 1) };
    if ok == FALSE {
        panic!("install Windows console control handler: {}", std::io::Error::last_os_error());
    }
}

#[cfg(windows)]
unsafe extern "system" fn handle_console_control(ctrl_type: u32) -> i32 {
    use windows_sys::Win32::Foundation::{FALSE, TRUE};
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT,
    };
    match ctrl_type {
        CTRL_C_EVENT | CTRL_BREAK_EVENT => {
            SIGINT_COUNT.fetch_add(1, Ordering::SeqCst);
            TRUE
        }
        CTRL_CLOSE_EVENT => {
            SIGTERM_COUNT.fetch_add(1, Ordering::SeqCst);
            FALSE
        }
        _ => FALSE,
    }
}
```

- [ ] **Step 3: Wire fakeagent main to signal module**

In `crates/lucarne-fakeagent/src/main.rs`:

Add module at top:

```rust
mod signals;
```

Replace current `nix` import and atomic statics with:

```rust
use signals::{install_signal_handlers, normalize_signal_name, signal_counter};
use std::{
    collections::BTreeMap,
    env, fmt,
    fs::File,
    io::{self, BufRead, BufReader, Read, Write},
    process::exit,
    sync::{Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};
```

Delete old `install_signal_handlers`, `handle_signal`, `normalize_signal_name`, and `signal_counter` functions from `main.rs`.

In tests at bottom, replace import line with:

```rust
use super::{contains_bytes, find_bytes, version_output, StdinState};
use crate::signals::normalize_signal_name;
```

- [ ] **Step 4: Use host lifecycle for live preflight children**

In `crates/lucarne/src/testing/live/providers.rs`, remove imports:

```rust
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
```

In both preflight functions, replace Unix process-group block with:

```rust
crate::host::process::configure_command(&mut command);
```

After each `spawn()`, add:

```rust
let managed_child = crate::host::process::ManagedProcess::attach(&child)
    .map_err(|err| format!("preflight process setup: {err}"))?;
```

Replace calls to `stop_preflight_child(&mut child).await.err()` with:

```rust
let stop_error = stop_preflight_child(&mut child, &managed_child).await.err();
```

Replace `stop_preflight_child` with:

```rust
#[cfg(any(feature = "codex", feature = "gemini"))]
async fn stop_preflight_child(
    child: &mut tokio::process::Child,
    managed_child: &crate::host::process::ManagedProcess,
) -> Result<(), String> {
    let pid = child
        .id()
        .map(|pid| pid as i32)
        .ok_or_else(|| "missing preflight pid".to_string())?;
    let _ = managed_child.terminate_graceful(pid);
    if tokio::time::timeout(Duration::from_millis(250), child.wait())
        .await
        .is_ok()
    {
        return Ok(());
    }
    let _ = managed_child.terminate_force(pid);
    tokio::time::timeout(Duration::from_millis(250), child.wait())
        .await
        .map_err(|_| "timed out waiting for preflight process tree to exit".to_string())?
        .map_err(|err| format!("wait preflight child: {err}"))?;
    Ok(())
}
```

- [ ] **Step 5: Make test helpers produce Windows executables**

In `crates/lucarne/tests/common/mod.rs`, change fakeagent target suffix:

```rust
target.push(if cfg!(windows) {
    "lucarne-fakeagent.exe"
} else {
    "lucarne-fakeagent"
});
```

Replace `write_cat_script` with:

```rust
pub fn write_cat_script(fixture: &std::path::Path) -> PathBuf {
    let dir = tempfile::tempdir().expect("tempdir");
    #[cfg(unix)]
    let path = {
        let path = dir.path().join("agent.sh");
        let script = format!(
            "#!/bin/sh\nexec cat {}\n",
            shell_quote(&fixture.to_string_lossy())
        );
        std::fs::write(&path, script).expect("write script");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    };
    #[cfg(windows)]
    let path = {
        let path = dir.path().join("agent.cmd");
        let script = format!("@echo off\r\ntype \"{}\"\r\n", fixture.display());
        std::fs::write(&path, script).expect("write script");
        path
    };
    std::mem::forget(dir);
    path
}
```

Replace `write_pi_cat_script` with:

```rust
pub fn write_pi_cat_script(fixture: &std::path::Path) -> PathBuf {
    let dir = tempfile::tempdir().expect("tempdir");
    #[cfg(unix)]
    let path = {
        let path = dir.path().join("agent.sh");
        let script = format!(
            r#"#!/bin/sh
if [ "$1" = "--list-models" ]; then
printf '%s\n' 'provider  model'
printf '%s\n' 'xai  grok-4'
exit 0
fi
exec cat {}
"#,
            shell_quote(&fixture.to_string_lossy())
        );
        std::fs::write(&path, script).expect("write script");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    };
    #[cfg(windows)]
    let path = {
        let path = dir.path().join("agent.cmd");
        let script = format!(
            "@echo off\r\nif \"%1\"==\"--list-models\" (\r\necho provider  model\r\necho xai  grok-4\r\nexit /b 0\r\n)\r\ntype \"{}\"\r\n",
            fixture.display()
        );
        std::fs::write(&path, script).expect("write script");
        path
    };
    std::mem::forget(dir);
    path
}
```

Keep `shell_quote` and mark it Unix-only:

```rust
#[cfg(unix)]
fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}
```

- [ ] **Step 6: Replace Unix symlink fakeagent wrapper**

In `crates/lucarne/tests/common/agent_runtime.rs`, replace `write_version_wrapper` with:

```rust
fn write_version_wrapper(provider_id: &'static str) -> PathBuf {
    let dir = tempfile::tempdir().expect("tempdir");
    let fakeagent = fakeagent_bin();
    #[cfg(unix)]
    let path = {
        let path = dir.path().join(format!("{provider_id}-fakeagent"));
        std::os::unix::fs::symlink(&fakeagent, &path).expect("symlink fakeagent");
        path
    };
    #[cfg(windows)]
    let path = {
        let path = dir.path().join(format!("{provider_id}-fakeagent.exe"));
        std::fs::copy(&fakeagent, &path).expect("copy fakeagent");
        path
    };
    std::mem::forget(dir);
    path
}
```

- [ ] **Step 7: Cfg Unix-only shell wrapper tests**

In `crates/lucarne/tests/claude_scenarios.rs`:

- Add `#[cfg(unix)]` to `use std::os::unix::fs::PermissionsExt;`.
- Add `#[cfg(unix)]` to `argv_recording_claude_wrapper`.
- Add `#[cfg(unix)]` to `shell_quote`.
- Add `#[cfg(unix)]` to test `launches_claude_as_long_lived_stream_json_without_print`.

In `crates/lucarne/tests/live_unit.rs`, add `#[cfg(unix)]` to tests that import `PermissionsExt`. Keep tests that do not rely on executable mode cross-platform.

In `crates/lucarne/src/testing/live/common.rs` and `crates/lucarne/src/testing/live/recording.rs`, add `#[cfg(unix)]` around helper blocks that import `PermissionsExt` and around tests that assert Unix executable bits.

- [ ] **Step 8: Verify Task 5**

Run:

```bash
cargo +nightly check -Zbuild-dir-new-layout -p lucarne-fakeagent
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-fakeagent
cargo +nightly test -Zbuild-dir-new-layout -p lucarne --test claude_scenarios basic
cargo +nightly test -Zbuild-dir-new-layout -p lucarne --test codex_scenarios -- --list
```

Windows VM:

```powershell
cargo +nightly check -Zbuild-dir-new-layout -p lucarne-fakeagent
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-fakeagent
cargo +nightly test -Zbuild-dir-new-layout -p lucarne --tests --no-run
```

Expected: fakeagent builds on both platforms; lucarne integration tests compile on Windows.

- [ ] **Step 9: Commit Task 5**

```bash
git add crates/lucarne-fakeagent crates/lucarne/src/testing/live/providers.rs crates/lucarne/src/testing/live/common.rs crates/lucarne/src/testing/live/recording.rs crates/lucarne/tests/common crates/lucarne/tests/claude_scenarios.rs crates/lucarne/tests/live_unit.rs
git commit -m "test: make harness portable to windows"
```

---

## Task 6: Add Windows two-layer watch parity and smoke coverage

**Files:**
- Modify: `agent-sessions/src/watch/mod.rs`
- Modify: `agent-sessions/src/watch/tests.rs`
- Modify: `agent-sessions/tests/watch_live.rs`

- [ ] **Step 1: Enable native recursive roots on Windows**

In `agent-sessions/src/watch/mod.rs`, replace the root-mode cfg split with:

```rust
#[cfg(any(target_os = "macos", windows))]
fn directory_root_watch_mode(_provider: WatchProvider) -> RecursiveMode {
    RecursiveMode::Recursive
}

#[cfg(not(any(target_os = "macos", windows)))]
fn directory_root_watch_mode(_provider: WatchProvider) -> RecursiveMode {
    RecursiveMode::NonRecursive
}
```

- [ ] **Step 2: Route only Windows recursive watch through notify**

Keep macOS on the existing `MacRecursiveWatcher` function body. Do not edit that body except if rustfmt requires whitespace. Add a Windows-only branch that calls `RecommendedWatcher` with recursive mode, and keep Linux/other platforms on the existing non-recursive fallback:

```rust
#[cfg(target_os = "macos")]
fn watch_recursive_path(&mut self, path: &Path) -> std::result::Result<(), WatchError> {
    let Some(watcher) = self._recursive_watcher.as_mut() else {
        return Err(WatchError::Notify(notify::Error::generic(
            "recursive watcher is not initialized",
        )));
    };
    if let Err(error) = watcher.watch(path) {
        self.watched_paths.remove(path);
        return Err(error);
    }
    Ok(())
}

#[cfg(windows)]
fn watch_recursive_path(&mut self, path: &Path) -> std::result::Result<(), WatchError> {
    let Some(watcher) = self._watcher.as_mut() else {
        return Err(WatchError::Notify(notify::Error::generic(
            "recursive watcher is not initialized",
        )));
    };
    if let Err(error) = watcher.watch(path, RecursiveMode::Recursive) {
        self.watched_paths.remove(path);
        return Err(error.into());
    }
    Ok(())
}

#[cfg(all(not(target_os = "macos"), not(windows)))]
fn watch_recursive_path(&mut self, path: &Path) -> std::result::Result<(), WatchError> {
    self.watch_non_recursive_path(path)
}
```

This does not add provider-name branches. Provider filtering stays in existing descriptor/watch contracts. Scope guard: macOS logic and non-Windows non-macOS logic must be behavior-identical after this change.

- [ ] **Step 3: Keep hot/recent direct file watches**

Do not remove this logic in `initial_watch_targets`:

```rust
if !self.has_recursive_root_for_path(session_path)
    || self.should_watch_session_file_target(session_path)
{
    push_watch_target(&mut paths, WatchTarget::non_recursive(session_path.clone()));
}
```

Reason: `docs/decisions/2026-05-19-watch-recent-session-file-targets.md` records a real missed-append bug when macOS relied only on recursive root events.

- [ ] **Step 4: Add Windows target-selection parity unit test**

In `agent-sessions/src/watch/tests.rs`, change both recursive target-selection tests from macOS-only to macOS-or-Windows:

```rust
#[cfg(all(feature = "codex", any(target_os = "macos", windows)))]
async fn recursive_codex_root_uses_recursive_watch_and_recent_session_file_targets()
```

and:

```rust
#[cfg(all(feature = "claude", any(target_os = "macos", windows)))]
async fn recursive_claude_root_uses_recursive_watch_and_recent_session_file_targets()
```

Only rename the existing `macos_*` test functions and cfg attributes; leave their assertions unchanged. Those assertions already verify recursive root plus hot/recent direct file targets. Keep non-recursive directory enumeration tests under `not(any(target_os = "macos", windows))`.

- [ ] **Step 5: Add atomic rename helper**

Add helper near `append_line` in `agent-sessions/tests/watch_live.rs`:

```rust
fn rename_into_place(path: &Path, content: &str) {
    let parent = path.parent().unwrap();
    fs::create_dir_all(parent).unwrap();
    let temp = parent.join(format!(
        ".{}.tmp",
        path.file_name().unwrap().to_string_lossy()
    ));
    fs::write(&temp, content).unwrap();
    fs::rename(&temp, path).unwrap();
}
```

- [ ] **Step 6: Add Codex rename smoke**

Add test below Codex append smoke:

```rust
#[cfg(feature = "codex")]
#[tokio::test]
async fn live_codex_watch_emits_renamed_assistant_response() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rollout-rename.jsonl");
    write_parent(
        &path,
        concat!(
            r#"{"timestamp":"2026-05-03T00:00:00.000Z","type":"session_meta","payload":{"session_id":"sess-rename","cwd":"/tmp/project","model":"gpt-5"}}"#,
            "\n",
            r#"{"timestamp":"2026-05-03T00:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"ping"}]}}"#,
            "\n",
        ),
    );
    let mut watcher = start_file_watcher(watch_provider("codex"), &path).await;

    rename_into_place(
        &path,
        concat!(
            r#"{"timestamp":"2026-05-03T00:00:00.000Z","type":"session_meta","payload":{"session_id":"sess-rename","cwd":"/tmp/project","model":"gpt-5"}}"#,
            "\n",
            r#"{"timestamp":"2026-05-03T00:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"ping"}]}}"#,
            "\n",
            r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"codex renamed pong"}]}}"#,
            "\n",
        ),
    );

    let update = wait_for_assistant_response(&mut watcher, "codex renamed pong").await;
    assert_eq!(update.provider, watch_provider("codex"));
}
```

- [ ] **Step 7: Add nested-create recursive smoke**

Add this test below the rename smoke:

```rust
#[cfg(feature = "codex")]
#[tokio::test]
async fn live_codex_watch_emits_nested_created_session_response() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("codex-root");
    fs::create_dir_all(&root).unwrap();
    let mut watcher = start_file_watcher(watch_provider("codex"), &root).await;
    let path = root.join("2026").join("05").join("20").join("rollout-nested.jsonl");

    write_parent(
        &path,
        concat!(
            r#"{"timestamp":"2026-05-03T00:00:00.000Z","type":"session_meta","payload":{"session_id":"sess-nested","cwd":"/tmp/project","model":"gpt-5"}}"#,
            "\n",
            r#"{"timestamp":"2026-05-03T00:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"ping"}]}}"#,
            "\n",
            r#"{"timestamp":"2026-05-03T00:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"codex nested pong"}]}}"#,
            "\n",
        ),
    );

    let update = wait_for_assistant_response(&mut watcher, "codex nested pong").await;
    assert_eq!(update.provider, watch_provider("codex"));
}
```

- [ ] **Step 8: Verify Task 6**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions --features 'discovery agent_session watch codex' recursive_codex_root_uses_recursive_watch_and_recent_session_file_targets
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions --features 'discovery agent_session watch claude' recursive_claude_root_uses_recursive_watch_and_recent_session_file_targets
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions --features 'discovery agent_session watch codex' live_codex_watch_emits_renamed_assistant_response
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions --features 'discovery agent_session watch codex' live_codex_watch_emits_nested_created_session_response
# Guard unchanged macOS behavior by confirming macOS FSEvents code remains present:
git diff -- agent-sessions/src/watch/mod.rs | grep -E 'MacRecursiveWatcher|target_os = "macos"' || true
```

Windows VM:

```powershell
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions --features 'discovery agent_session watch codex' recursive_codex_root_uses_recursive_watch_and_recent_session_file_targets
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions --features 'discovery agent_session watch claude' recursive_claude_root_uses_recursive_watch_and_recent_session_file_targets
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions --features 'discovery agent_session watch codex' live_codex_watch_emits_renamed_assistant_response
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions --features 'discovery agent_session watch codex' live_codex_watch_emits_nested_created_session_response
```

Expected: pass on macOS and Windows. macOS behavior must remain unchanged: recursive roots still use `MacRecursiveWatcher`, and hot/recent direct file target assertions remain unchanged. Linux/other non-macOS/non-Windows behavior must remain unchanged: recursive path requests still degrade to non-recursive fallback. If Windows rename coalesces events, existing watcher debounce should still emit update after parse. If nested create fails on Windows, do not fall back to manual whole-tree watching; first inspect raw `ReadDirectoryChangesW` paths and provider path filtering.

- [ ] **Step 9: Commit Task 6**

```bash
git add agent-sessions/src/watch/mod.rs agent-sessions/src/watch/tests.rs agent-sessions/tests/watch_live.rs
git commit -m "feat: add windows recursive watch parity"
```

---

## Task 7: Full Windows build and smoke verification

**Files:**
- No source changes unless verification finds compile errors from previous tasks.

- [ ] **Step 1: macOS full verification**

Run:

```bash
cargo +nightly check -Zbuild-dir-new-layout -p lucarned
cargo +nightly test -Zbuild-dir-new-layout -p lucarne
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-fakeagent
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions --features 'discovery agent_session watch codex claude gemini copilot pi'
```

Expected: all pass. Also review the diff before proceeding: macOS-specific logic (`target_os = "macos"`, FSEvents, Unix signal/process group behavior, `/bin/ps`, `/usr/sbin/lsof`, `HOME` defaults) must be unchanged except for mechanical relocation behind equivalent Unix/macOS helpers.

- [ ] **Step 2: Windows check/build verification**

Run in Windows VM:

```powershell
$env:HTTP_PROXY='http://10.211.55.2:6153'
$env:HTTPS_PROXY='http://10.211.55.2:6153'
$env:PATH = $env:USERPROFILE + '\.cargo\bin;' + $env:PATH
$env:CARGO_TARGET_DIR='C:\lucarne-target'
Set-Location 'Z:\Volumes\Data\opensource\conductor\lucarne\.worktrees\windows-platform-substrate'
cargo +nightly check -Zbuild-dir-new-layout -p lucarned
cargo +nightly build -Zbuild-dir-new-layout -p lucarned
```

Expected: both pass. `cargo build` is required because `rusqlite` link must be proven, not only checked.

- [ ] **Step 3: Windows tests**

Run:

```powershell
cargo +nightly test -Zbuild-dir-new-layout -p lucarne --lib
cargo +nightly test -Zbuild-dir-new-layout -p lucarne-fakeagent
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions --features 'discovery agent_session watch codex claude gemini copilot pi'
```

Expected: pass. These tests must validate functional parity, not only compilation: interrupt delivery keeps a handled fakeagent session alive, timeout/close paths end the session, process descendants are cleaned, resource snapshot fields are populated, and watch append/rename/nested-create content matches macOS/Unix. If a third-party CLI live test needs actual provider credentials, keep it outside this command set.

- [ ] **Step 4: Windows binary smoke**

Run:

```powershell
C:\lucarne-target\debug\lucarned.exe --help
```

If target layout places binary elsewhere, locate it with:

```powershell
Get-ChildItem C:\lucarne-target -Recurse -Filter lucarned.exe | Select-Object -First 5 -ExpandProperty FullName
```

Expected: help text prints and exit code is zero.

- [ ] **Step 5: Check for forbidden common-layer platform leaks**

Run:

```bash
grep -R "nix::\|std::os::unix" -n crates/lucarne/src crates/lucarne-fakeagent/src agent-sessions/src | grep -v "cfg(unix)" || true
grep -R "var_os(\"HOME\")\|var(\"HOME\")" -n crates/lucarne/src crates/lucarned/src agent-sessions/src | grep -v "host/paths.rs" | grep -v "agent-sessions/src/paths.rs" || true
grep -R "Codex\|Claude\|Gemini\|Copilot\|Pi" -n crates/lucarne/src/host agent-sessions/src/watch || true
```

Expected: first command prints only cfg-gated Unix files or nothing. Second command prints nothing from `crate::host` and no provider-name branching from `agent-sessions/src/watch`.

- [ ] **Step 6: Commit verification notes if commands required doc update**

If only code changed in previous tasks, no verification commit is needed. If documentation or scripts were corrected, commit:

```bash
git add docs scripts crates agent-sessions
git commit -m "docs: record windows verification commands"
```

---

## Self-Review Checklist

- Spec coverage: Windows user-visible behavior matches macOS/Unix for launch, interrupt, interrupt timeout, close/drop cleanup, resource status, writer PID, two-layer watch, paths, build, and tests.
- Boundary check: no provider IDs in `crate::host`; no Windows watch provider table in common history; no public provider enum.
- macOS preservation check: existing macOS process, path, resource, writer PID, watch, provider, and live-test behavior unchanged except for behavior-preserving mechanical moves.
- Dependency check: `nix` only under `cfg(unix)` dependencies; `windows-sys` only under `cfg(windows)` dependencies.
- Semantics check: Windows `SIGINT` maps to cancel-current-turn, not kill; failed interrupt returns error; timeout/close/force paths end the session and clean descendants; implementation mechanisms do not leak above host boundary.
- Verification check: macOS `check/test`, Windows `check/build/test`, and functional smoke tests must pass before claiming complete.
