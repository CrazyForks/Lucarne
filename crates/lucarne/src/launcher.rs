//! Launcher — spawns and tracks OS processes that host agent runtimes.
//!
//! Deliberately protocol-agnostic: framing, translation, and control
//! live in [`crate::framer`] and [`crate::dialect`].
//!
//! Implementations:
//!
//! * [`LocalLauncher`] — always cold-spawn a fresh process.

use crate::error::{LucarneError, Result};
use async_trait::async_trait;
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::Duration,
};
use tokio::{
    io::AsyncRead,
    process::{Child, ChildStdin, Command},
    sync::watch,
};
use tracing::{debug, info, warn};

/// Environment variables that leak from a parent Claude-session-like
/// environment. We strip them before spawning children so agents don't
/// pick up stale state.
const FILTERED_PARENT_SESSION_ENV: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_AGENT_SDK_VERSION",
    "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING",
];

#[derive(Debug, Clone, Default)]
pub struct LaunchSpec {
    pub bin: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: String,
    pub use_pty: bool,
    pub files: Vec<TempFile>,
}

#[derive(Debug, Clone)]
pub struct TempFile {
    pub purpose: String,
    pub content: Vec<u8>,
    pub suffix: String,
}

#[derive(Debug, Clone, Copy)]
pub struct ExitInfo {
    pub code: i32,
    pub signalled: bool,
}

pub struct Process {
    inner: Arc<ProcessInner>,
}

struct ProcessInner {
    pid: i32,
    stdin: StdMutex<Option<ChildStdin>>,
    stdout: StdMutex<Option<Box<dyn AsyncRead + Unpin + Send>>>,
    stderr: StdMutex<Option<Box<dyn AsyncRead + Unpin + Send>>>,
    exit_rx: watch::Receiver<Option<ExitInfo>>,
    closed: AtomicBool,
    host_process: crate::host::process::ManagedProcess,
    _cleanup: Vec<PathBuf>,
    grace: Duration,
}

impl Process {
    pub fn pid(&self) -> i32 {
        self.inner.pid
    }
    pub async fn take_stdin(&self) -> Option<ChildStdin> {
        self.inner.stdin.lock().expect("process stdin lock").take()
    }
    pub async fn take_stdout(&self) -> Option<Box<dyn AsyncRead + Unpin + Send>> {
        self.inner
            .stdout
            .lock()
            .expect("process stdout lock")
            .take()
    }
    pub async fn take_stderr(&self) -> Option<Box<dyn AsyncRead + Unpin + Send>> {
        self.inner
            .stderr
            .lock()
            .expect("process stderr lock")
            .take()
    }

    /// Awaits the process exit. Returns the last observed [`ExitInfo`].
    pub async fn wait(&self) -> ExitInfo {
        debug!(target: "lucarne::launcher", pid = self.inner.pid, "waiting for process exit");
        let mut rx = self.inner.exit_rx.clone();
        loop {
            if let Some(info) = *rx.borrow() {
                info!(
                    target: "lucarne::launcher",
                    pid = self.inner.pid,
                    exit_code = info.code,
                    signalled = info.signalled,
                    "process exited"
                );
                return info;
            }
            if rx.changed().await.is_err() {
                warn!(
                    target: "lucarne::launcher",
                    pid = self.inner.pid,
                    "process exit watcher closed unexpectedly"
                );
                return ExitInfo {
                    code: -1,
                    signalled: false,
                };
            }
        }
    }

    pub fn try_exit(&self) -> Option<ExitInfo> {
        *self.inner.exit_rx.borrow()
    }

    pub fn signal(&self, name: &str) -> Result<()> {
        if self.inner.pid <= 0 {
            debug!(
                target: "lucarne::launcher",
                pid = self.inner.pid,
                signal = name,
                "ignoring signal for virtual process"
            );
            return Ok(()); // virtual process (e.g. retry wrapper) — ignore signals
        }
        debug!(
            target: "lucarne::launcher",
            pid = self.inner.pid,
            signal = name,
            "sending signal to process group"
        );
        self.inner.host_process.signal(self.inner.pid, name)
    }

    /// SIGTERM the process group, wait up to grace, then SIGKILL.
    pub async fn close(&self) {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        info!(
            target: "lucarne::launcher",
            pid = self.inner.pid,
            grace_ms = self.inner.grace.as_millis() as u64,
            "closing process"
        );
        // pid <= 0 means there is no OS process group to signal.
        // In that case just wait for the background task to finish via the
        // exit watch channel.
        if self.inner.pid > 0 {
            let _ = self.inner.host_process.terminate_graceful(self.inner.pid);
        }
        let grace = self.inner.grace;
        let mut rx = self.inner.exit_rx.clone();
        let deadline = tokio::time::sleep(grace);
        tokio::select! {
            _ = async {
                while rx.borrow().is_none() {
                    if rx.changed().await.is_err() { break; }
                }
            } => return,
            _ = deadline => {}
        }
        if self.inner.pid > 0 {
            warn!(
                target: "lucarne::launcher",
                pid = self.inner.pid,
                "process did not exit during grace period; sending SIGKILL"
            );
            let _ = self.inner.host_process.terminate_force(self.inner.pid);
        }
        let _ = self.wait().await;
    }
}

#[async_trait]
pub trait Launcher: Send + Sync {
    async fn launch(&self, spec: &LaunchSpec) -> Result<Process>;
}

// ——— LocalLauncher ———

pub struct LocalLauncher {
    pub grace: Duration,
}

impl Default for LocalLauncher {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(2),
        }
    }
}

impl LocalLauncher {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Launcher for LocalLauncher {
    async fn launch(&self, spec: &LaunchSpec) -> Result<Process> {
        info!(
            target: "lucarne::launcher",
            bin = spec.bin.as_str(),
            args = spec.args.len(),
            cwd = spec.cwd.as_str(),
            env = spec.env.len(),
            files = spec.files.len(),
            use_pty = spec.use_pty,
            "launching local process"
        );
        let (cleanup_paths, _purpose_map) = materialize(&spec.files)?;
        let mut cmd = Command::new(&spec.bin);
        cmd.args(&spec.args);
        if !spec.cwd.is_empty() {
            cmd.current_dir(&spec.cwd);
        }
        apply_env(&mut cmd, &spec.env);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        crate::host::process::configure_command(&mut cmd);

        let mut child: Child = cmd.spawn().map_err(|e| {
            for p in &cleanup_paths {
                let _ = std::fs::remove_file(p);
            }
            LucarneError::launcher(format!("spawn {}: {}", spec.bin, e))
        })?;

        let pid = child.id().ok_or_else(|| LucarneError::launcher("no pid"))? as i32;
        info!(
            target: "lucarne::launcher",
            pid,
            bin = spec.bin.as_str(),
            "local process spawned"
        );
        let host_process = crate::host::process::ManagedProcess::attach(&child).map_err(|e| {
            for p in &cleanup_paths {
                let _ = std::fs::remove_file(p);
            }
            e
        })?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (tx, rx) = watch::channel(None);
        let cleanup_for_waiter: Vec<PathBuf> = cleanup_paths.clone();
        tokio::spawn(async move {
            let status = child.wait().await;
            let info = match status {
                Ok(s) => ExitInfo {
                    code: s.code().unwrap_or(-1),
                    signalled: s.code().is_none(),
                },
                Err(_) => ExitInfo {
                    code: -1,
                    signalled: false,
                },
            };
            debug!(
                target: "lucarne::launcher",
                pid,
                exit_code = info.code,
                signalled = info.signalled,
                "wait task observed process exit"
            );
            let _ = tx.send(Some(info));
            for p in cleanup_for_waiter {
                let _ = std::fs::remove_file(p);
            }
        });

        Ok(Process {
            inner: Arc::new(ProcessInner {
                pid,
                stdin: StdMutex::new(stdin),
                stdout: StdMutex::new(
                    stdout.map(|s| -> Box<dyn AsyncRead + Unpin + Send> { Box::new(s) }),
                ),
                stderr: StdMutex::new(
                    stderr.map(|s| -> Box<dyn AsyncRead + Unpin + Send> { Box::new(s) }),
                ),
                exit_rx: rx,
                closed: AtomicBool::new(false),
                host_process,
                _cleanup: cleanup_paths,
                grace: self.grace,
            }),
        })
    }
}

fn apply_env(cmd: &mut Command, extra: &BTreeMap<String, String>) {
    let mut base: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| !FILTERED_PARENT_SESSION_ENV.contains(&k.as_str()))
        .collect();
    // Overwrite / append
    for (k, v) in extra {
        if FILTERED_PARENT_SESSION_ENV.contains(&k.as_str()) {
            continue;
        }
        if let Some(pos) = base.iter().position(|(bk, _)| bk == k) {
            base[pos].1 = v.clone();
        } else {
            base.push((k.clone(), v.clone()));
        }
    }
    crate::host::proxy_env::apply_missing_proxy_env(&mut base);

    // Default LANG
    if !base.iter().any(|(k, _)| k == "LANG") {
        base.push(("LANG".into(), "en_US.UTF-8".into()));
    } else {
        for (k, v) in &mut base {
            if k == "LANG" && v.is_empty() {
                *v = "en_US.UTF-8".into();
            }
        }
    }
    // Clear and set
    cmd.env_clear();
    for (k, v) in base {
        cmd.env(k, v);
    }
}

fn materialize(files: &[TempFile]) -> Result<(Vec<PathBuf>, HashMap<String, PathBuf>)> {
    let mut paths: HashMap<String, PathBuf> = HashMap::new();
    let mut created = Vec::new();
    debug!(
        target: "lucarne::launcher",
        file_count = files.len(),
        "materializing temp files"
    );
    for f in files {
        let suffix = if f.suffix.is_empty() {
            "".to_string()
        } else {
            f.suffix.clone()
        };
        let name = format!("lucarne-{}{}", uuid::Uuid::new_v4().simple(), suffix);
        let mut path = std::env::temp_dir();
        path.push(name);
        std::fs::write(&path, &f.content)?;
        debug!(
            target: "lucarne::launcher",
            purpose = f.purpose.as_str(),
            path = %path.display(),
            bytes = f.content.len(),
            "materialized temp file"
        );
        paths.insert(f.purpose.clone(), path.clone());
        created.push(path);
    }
    Ok((created, paths))
}

// silence unused warnings in case the dialect traits haven't all been wired

fn _assert_process_send_sync() {
    fn check<T: Send + Sync>() {}
    check::<Process>();
}
