# Linux Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Linux a verified first-class Lucarne target in the existing single install/autostart PR.

**Architecture:** Keep platform-specific behavior behind existing cfg/platform modules. Linux uses `notify` recursive inotify for watch parity, PATH-aware host tool lookup for `ps`/`lsof`, and systemd user services for autostart. Common layers continue to orchestrate through existing typed contracts without concrete provider branching.

**Tech Stack:** Rust nightly, Cargo `-Zbuild-dir-new-layout`, `notify`, `tokio::process`, systemd user services, cargo-dist `.tar.xz`, Linux `ps`/`lsof`.

---

## File Map

- Modify `agent-sessions/src/watch/mod.rs`: route Linux recursive watch roots to `notify::RecursiveMode::Recursive`.
- Modify `agent-sessions/tests/watch_live.rs`: broaden recursive watch live tests from macOS/Windows to Linux where appropriate.
- Modify `crates/lucarne/src/host/process_table/unix.rs`: resolve `ps` from PATH/fallbacks instead of hardcoding `/bin/ps`.
- Modify `crates/lucarne/src/host/file_users/unix.rs`: resolve `lsof` from PATH/fallbacks instead of hardcoding `/usr/sbin/lsof`.
- Modify `crates/lucarned-ctl/src/autostart/linux.rs`: harden systemd unit and improve command errors without changing public command surface.
- Modify `crates/lucarned-ctl/src/doctor.rs`: add Linux-specific checks and package hints.
- Modify `README.md`: document Linux install/autostart and runtime prerequisites.

---

## Task 1: Linux Recursive Watch Parity

**Files:**
- Modify: `agent-sessions/src/watch/mod.rs`
- Modify/Test: `agent-sessions/tests/watch_live.rs`

- [ ] **Step 1: Inspect current recursive cfgs**

Run:

```bash
grep -RIn "watch_recursive_path\|target_os = \"macos\"\|windows" agent-sessions/src/watch agent-sessions/tests/watch_live.rs
```

Expected: Linux uses the non-macOS/non-Windows fallback in `watch_recursive_path`.

- [ ] **Step 2: Write or broaden failing Linux watch coverage**

In `agent-sessions/tests/watch_live.rs`, adjust cfg attributes so recursive nested-create and rename tests include Linux:

```rust
#[cfg(any(target_os = "macos", windows, target_os = "linux"))]
```

Keep existing assertions unchanged unless Linux event ordering requires waiting/debounce already supported by helpers.

- [ ] **Step 3: Run Linux watch test and observe failure before implementation**

Run on Linux:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions --features watch,codex,claude,gemini,pi,copilot,agent_session,discovery --test watch_live --quiet
```

Expected before implementation: recursive nested/rename watch coverage fails or skips incorrectly on Linux.

- [ ] **Step 4: Implement Linux recursive watch**

In `agent-sessions/src/watch/mod.rs`, change cfgs to keep macOS on `MacRecursiveWatcher` and route Windows/Linux through `notify` recursive:

```rust
#[cfg(any(windows, target_os = "linux"))]
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

#[cfg(all(not(target_os = "macos"), not(windows), not(target_os = "linux")))]
fn watch_recursive_path(&mut self, path: &Path) -> std::result::Result<(), WatchError> {
    self.watch_non_recursive_path(path)
}
```

- [ ] **Step 5: Verify Linux watch tests pass**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions --features watch,codex,claude,gemini,pi,copilot,agent_session,discovery --test watch_live --quiet
```

Expected: pass.

- [ ] **Step 6: Commit watch parity**

```bash
git add agent-sessions/src/watch/mod.rs agent-sessions/tests/watch_live.rs
git commit -m "fix: enable linux recursive session watch"
```

---

## Task 2: Linux Host Tool Resolution

**Files:**
- Modify/Test: `crates/lucarne/src/host/process_table/unix.rs`
- Modify/Test: `crates/lucarne/src/host/file_users/unix.rs`

- [ ] **Step 1: Add tests for Unix command resolution**

Add unit tests that verify resolution prefers PATH and falls back to known absolute paths:

```rust
#[test]
fn resolve_unix_tool_prefers_path_candidate() {
    let path = std::env::join_paths([std::path::PathBuf::from("/custom/bin")]).unwrap();
    let resolved = resolve_unix_tool("ps", Some(path), &["/bin/ps", "/usr/bin/ps"]);
    assert_eq!(resolved, std::path::PathBuf::from("/custom/bin/ps"));
}

#[test]
fn resolve_unix_tool_uses_first_fallback_without_path() {
    let resolved = resolve_unix_tool("lsof", None, &["/usr/bin/lsof", "/usr/sbin/lsof"]);
    assert_eq!(resolved, std::path::PathBuf::from("/usr/bin/lsof"));
}
```

- [ ] **Step 2: Run tests to verify helper missing**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne host::process_table::unix --quiet
cargo +nightly test -Zbuild-dir-new-layout -p lucarne host::file_users::unix --quiet
```

Expected before implementation: compile failure for missing `resolve_unix_tool` or test failure.

- [ ] **Step 3: Implement resolver in each Unix module or extract local helper**

Use this helper shape in both files, or a small private shared helper if existing module layout makes that cleaner:

```rust
fn resolve_unix_tool(name: &str, path: Option<std::ffi::OsString>, fallbacks: &[&str]) -> std::path::PathBuf {
    if let Some(path) = path {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    fallbacks
        .first()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(name))
}
```

Use it as:

```rust
let program = resolve_unix_tool("ps", std::env::var_os("PATH"), &["/usr/bin/ps", "/bin/ps"]);
Command::new(program)
```

and:

```rust
let program = resolve_unix_tool("lsof", std::env::var_os("PATH"), &["/usr/bin/lsof", "/usr/sbin/lsof"]);
Command::new(program)
```

- [ ] **Step 4: Verify lucarne host tests**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarne host::process_table::unix --quiet
cargo +nightly test -Zbuild-dir-new-layout -p lucarne host::file_users::unix --quiet
```

Expected: pass.

- [ ] **Step 5: Commit host tool resolution**

```bash
git add crates/lucarne/src/host/process_table/unix.rs crates/lucarne/src/host/file_users/unix.rs
git commit -m "fix: resolve linux host tools from path"
```

---

## Task 3: Harden Linux systemd User Autostart

**Files:**
- Modify/Test: `crates/lucarned-ctl/src/autostart/linux.rs`

- [ ] **Step 1: Extend unit rendering test**

Update `unit_quotes_exec_start_with_spaces` to assert restart policy:

```rust
assert!(unit.contains("Restart=on-failure"));
assert!(unit.contains("RestartSec=5"));
```

- [ ] **Step 2: Run test to verify failure**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarned-ctl autostart::linux --quiet
```

Expected before implementation: test fails because rendered unit still has `Restart=no`.

- [ ] **Step 3: Update unit template**

In `render_unit`, change service section to:

```ini
[Service]
Type=simple
ExecStart=<quoted lucarned>
Restart=on-failure
RestartSec=5
```

- [ ] **Step 4: Verify lucarned-ctl tests**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarned-ctl --quiet
```

Expected: pass.

- [ ] **Step 5: Commit systemd unit hardening**

```bash
git add crates/lucarned-ctl/src/autostart/linux.rs
git commit -m "fix: harden linux autostart unit"
```

---

## Task 4: Linux Doctor Diagnostics

**Files:**
- Modify/Test: `crates/lucarned-ctl/src/doctor.rs`

- [ ] **Step 1: Inspect doctor output structure**

Run:

```bash
sed -n '1,260p' crates/lucarned-ctl/src/doctor.rs
```

Expected: current doctor has generic command/path checks and optional CLI checks.

- [ ] **Step 2: Add Linux-specific expected output tests**

Add tests under `#[cfg(target_os = "linux")]` or platform-neutral helper tests verifying Linux hints contain:

```text
systemctl --user
XDG_RUNTIME_DIR
loginctl enable-linger
ps
lsof
Debian/Ubuntu: procps lsof
Fedora: procps-ng lsof
Arch: procps-ng lsof
```

- [ ] **Step 3: Run test to verify missing hints**

Run on Linux:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarned-ctl doctor --quiet
```

Expected before implementation: missing Linux hint assertions fail.

- [ ] **Step 4: Implement Linux doctor section**

Add a Linux-only function called from doctor output:

```rust
#[cfg(target_os = "linux")]
fn linux_diagnostics(lines: &mut Vec<String>) {
    lines.push("linux:".to_string());
    lines.push(format_tool_status("systemctl --user", command_exists("systemctl")));
    lines.push(format_env_status("XDG_RUNTIME_DIR", std::env::var_os("XDG_RUNTIME_DIR").is_some()));
    lines.push(format_tool_status("loginctl", command_exists("loginctl")));
    lines.push("hint: headless autostart may need `loginctl enable-linger $USER`".to_string());
    lines.push(format_tool_status("ps", command_exists("ps")));
    lines.push(format_tool_status("lsof", command_exists("lsof")));
    lines.push("packages: Debian/Ubuntu: procps lsof; Fedora: procps-ng lsof; Arch: procps-ng lsof".to_string());
}
```

Use existing formatting style if `doctor.rs` already has equivalent helpers.

- [ ] **Step 5: Verify lucarned-ctl tests**

Run:

```bash
cargo +nightly test -Zbuild-dir-new-layout -p lucarned-ctl --quiet
```

Expected: pass.

- [ ] **Step 6: Commit Linux doctor**

```bash
git add crates/lucarned-ctl/src/doctor.rs
git commit -m "feat: add linux doctor diagnostics"
```

---

## Task 5: Linux README and Release Smoke Notes

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add Linux prerequisites and autostart notes**

Add a Linux section with this content:

- Lucarne ships GNU Linux archives for x86_64 and aarch64.
- Autostart uses systemd user services.
- Show these commands:
  - `lucarned autostart install --start`
  - `lucarned autostart status`
- On headless servers, tell users to run `sudo loginctl enable-linger "$USER"` if user services must survive logout.
- Required host tools for full diagnostics/resource attribution:
  - Debian/Ubuntu: `sudo apt install procps lsof`
  - Fedora: `sudo dnf install procps-ng lsof`
  - Arch: `sudo pacman -S procps-ng lsof`

- [ ] **Step 2: Verify README formatting**

Run:

```bash
git diff --check HEAD -- README.md
```

Expected: no whitespace errors.

- [ ] **Step 3: Commit docs**

```bash
git add README.md
git commit -m "docs: document linux autostart requirements"
```

---

## Task 6: Cross-Platform Verification

**Files:**
- No source changes expected.

- [ ] **Step 1: macOS verification**

Run on macOS:

```bash
cargo +nightly check -Zbuild-dir-new-layout -p lucarned --quiet
cargo +nightly test -Zbuild-dir-new-layout -p lucarned-ctl --quiet
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions --features watch,codex,claude,gemini,pi,copilot,agent_session,discovery --quiet
dist plan --tag v0.1.0
dist build --target aarch64-apple-darwin --artifacts local --tag v0.1.0
git diff --check HEAD
```

Expected: all commands exit 0.

- [ ] **Step 2: Linux verification**

Run on Linux:

```bash
cargo +nightly check -Zbuild-dir-new-layout -p lucarned --quiet
cargo +nightly test -Zbuild-dir-new-layout -p lucarned-ctl --quiet
cargo +nightly test -Zbuild-dir-new-layout -p lucarned --quiet
cargo +nightly test -Zbuild-dir-new-layout -p agent-sessions --features watch,codex,claude,gemini,pi,copilot,agent_session,discovery --quiet
dist build --target x86_64-unknown-linux-gnu --artifacts local --tag v0.1.0
```

Extract archive and run:

```bash
tar -xJf target/distrib/lucarned-x86_64-unknown-linux-gnu.tar.xz -C /tmp/lucarne-linux-smoke
/tmp/lucarne-linux-smoke/lucarned-x86_64-unknown-linux-gnu/lucarned --version
/tmp/lucarne-linux-smoke/lucarned-x86_64-unknown-linux-gnu/lucarned paths
/tmp/lucarne-linux-smoke/lucarned-x86_64-unknown-linux-gnu/lucarned doctor
```

Expected: all commands exit 0.

- [ ] **Step 3: Linux systemd-user smoke**

On a systemd user session, run:

```bash
lucarned autostart uninstall --stop || true
lucarned autostart install --start
lucarned autostart status
lucarned autostart stop
lucarned autostart uninstall --stop
```

Expected: install/start/status/stop/uninstall succeed, or headless failure reports linger/user-bus guidance.

- [ ] **Step 4: Windows verification**

Run on Windows VM:

```powershell
cargo +nightly check -Zbuild-dir-new-layout -p lucarned --quiet
cargo +nightly test -Zbuild-dir-new-layout -p lucarned-ctl --quiet
cargo +nightly test -Zbuild-dir-new-layout -p lucarned --quiet
dist build --target x86_64-pc-windows-msvc --artifacts local --tag v0.1.0
```

Expected: all commands exit 0 and Windows archive remains `.tar.xz`.

- [ ] **Step 5: Final commit or amend verification notes if needed**

If only verification ran, no commit is required. If docs/scripts were adjusted, commit with:

```bash
git add <changed-files>
git commit -m "test: document linux verification"
```

---

## Self-Review

- Spec coverage: watch parity, host tools, systemd user autostart, doctor, release smoke, and cross-platform constraints all map to tasks.
- Placeholder scan: no placeholder markers remain.
- Type consistency: file paths and command names match current crate/module layout.
