//! Runtime — composes Launcher + Framer + Dialect into a running
//! [`Session`]. No vendor-specific logic here; adding an agent family
//! means plugging in a different [`crate::dialect::Dialect`].
//!
//! Backpressure property: emission into `events` is bounded; when it
//! fills, frame pumping blocks, the OS pipe fills, and the agent's
//! `write(2)` blocks. No drops.

use crate::{
    agent_runtime::{AgentCommandCatalog, AgentCommandInvocation, AgentCommandSource},
    dialect::{command_result_events, CommandDispatch, Dialect, Input, OutFrame, SessionParams},
    error::{LucarneError, Result},
    event::{now_rfc3339, Decision, Event, LogLine, Payload, PermissionResponse},
    framer::Framer,
    launcher::{ExitInfo, LaunchSpec, Launcher, Process},
};
use std::{
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, Mutex as AsyncMutex},
    task::JoinHandle,
};
use tracing::{debug, info, trace, warn};

const DEFAULT_BUFFER: usize = 1024;
const DEFAULT_GRACE: Duration = Duration::from_secs(3);

pub struct Config {
    pub launcher: Arc<dyn Launcher>,
    pub spec: LaunchSpec,
    pub framer: Framer,
    pub dialect: Box<dyn Dialect>,
    pub session_params: SessionParams,
    pub buffer_size: usize,
    pub interrupt_grace: Duration,
}

impl Config {
    pub fn new(
        launcher: Arc<dyn Launcher>,
        spec: LaunchSpec,
        framer: Framer,
        dialect: Box<dyn Dialect>,
        session_params: SessionParams,
    ) -> Self {
        Self {
            launcher,
            spec,
            framer,
            dialect,
            session_params,
            buffer_size: DEFAULT_BUFFER,
            interrupt_grace: DEFAULT_GRACE,
        }
    }
}

/// The per-invocation session handle.
pub struct Session {
    id: String,
    epoch: String,
    proc: Option<Arc<Process>>,
    stdin: Arc<AsyncMutex<Option<tokio::process::ChildStdin>>>,
    dialect: Arc<AsyncMutex<Box<dyn Dialect>>>,
    events_rx: StdMutex<Option<mpsc::Receiver<Event>>>,
    events_tx: Arc<StdMutex<Option<mpsc::Sender<Event>>>>,
    grace: Duration,
    tasks: StdMutex<Vec<JoinHandle<()>>>,

    cancel: tokio_util_cancel::CancelFlag,
}

impl Session {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    pub fn process_id(&self) -> Option<i32> {
        self.proc.as_ref().map(|proc| proc.pid())
    }

    /// Take ownership of the event receiver. Only the first caller
    /// succeeds; subsequent callers see `None`.
    pub async fn events(&self) -> Option<mpsc::Receiver<Event>> {
        self.events_rx.lock().expect("runtime event rx lock").take()
    }

    pub fn new_synthetic(
        events_rx: mpsc::Receiver<Event>,
        _events_tx: mpsc::Sender<Event>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            epoch: uuid::Uuid::new_v4().to_string(),
            proc: None,
            stdin: Arc::new(AsyncMutex::new(None)),
            dialect: Arc::new(AsyncMutex::new(Box::new(NoopDialect))),
            events_rx: StdMutex::new(Some(events_rx)),
            events_tx: Arc::new(StdMutex::new(None)),
            grace: DEFAULT_GRACE,
            tasks: StdMutex::new(Vec::new()),
            cancel: tokio_util_cancel::CancelFlag::new(),
        }
    }

    pub async fn send(&self, input: Input) -> Result<()> {
        if self.proc.is_none() {
            return Err(LucarneError::runtime(
                "synthetic session does not support send",
            ));
        }
        debug!(
            target: "lucarne::runtime",
            session_id = self.id(),
            epoch = self.epoch(),
            text_bytes = input.text.len(),
            images = input.images.len(),
            "encoding user input"
        );
        let frames = {
            let mut d = self.dialect.lock().await;
            d.encode_user_message(&input)?
        };
        self.write_all(&frames).await
    }

    pub async fn command_catalog(&self) -> AgentCommandCatalog {
        self.dialect.lock().await.command_catalog()
    }

    pub async fn invoke_command(&self, command: AgentCommandInvocation) -> Result<()> {
        if self.proc.is_none() {
            return Err(LucarneError::runtime(
                "synthetic session does not support command invocation",
            ));
        }
        debug!(
            target: "lucarne::runtime",
            session_id = self.id(),
            epoch = self.epoch(),
            command = %command.name,
            "encoding command invocation"
        );
        let dispatch = {
            let mut d = self.dialect.lock().await;
            match command.source {
                AgentCommandSource::AdapterMapped => d.handle_system_command(&command)?,
                AgentCommandSource::ProviderNative => d.handle_native_command(&command)?,
            }
        };
        match dispatch {
            CommandDispatch::Ready(result) => {
                self.emit_command_events(command_result_events("adapter-command", result))
                    .await
            }
            CommandDispatch::Deferred(frames) => self.write_all(&frames).await,
        }
    }

    pub async fn resolve(&self, req_id: &str, decision: Decision) -> Result<()> {
        self.resolve_with_response(req_id, &PermissionResponse::from_decision(decision))
            .await
    }

    pub async fn resolve_with_response(
        &self,
        req_id: &str,
        resp: &PermissionResponse,
    ) -> Result<()> {
        debug!(
            target: "lucarne::runtime",
            session_id = self.id(),
            epoch = self.epoch(),
            req_id,
            answers = resp.answers.len(),
            "encoding permission response"
        );
        let frames = {
            let mut d = self.dialect.lock().await;
            d.encode_permission_response(req_id, resp)?
        };
        self.write_all(&frames).await
    }

    pub async fn interrupt(&self) -> Result<()> {
        info!(
            target: "lucarne::runtime",
            session_id = self.id(),
            epoch = self.epoch(),
            "interrupt requested"
        );
        let frames = {
            let mut d = self.dialect.lock().await;
            d.encode_interrupt()?
        };
        self.write_all(&frames).await?;
        let has_signal_frame = frames
            .iter()
            .any(|frame| matches!(frame, OutFrame::Signal(_)));
        if has_signal_frame {
            if let Some(proc) = &self.proc {
                let proc = Arc::clone(proc);
                let grace = self.grace;
                tokio::spawn(async move {
                    tokio::select! {
                        _ = proc.wait() => {}
                        _ = tokio::time::sleep(grace) => {
                            let _ = proc.signal("SIGTERM");
                        }
                    }
                });
            }
        }
        Ok(())
    }

    pub async fn close(&self) {
        info!(
            target: "lucarne::runtime",
            session_id = self.id(),
            epoch = self.epoch(),
            "closing runtime session"
        );
        self.cancel.set();
        self.events_tx.lock().expect("runtime event tx lock").take();
        if let Some(proc) = &self.proc {
            proc.close().await;
        }
        let tasks = {
            let mut tasks = self.tasks.lock().expect("runtime task list lock");
            tasks.drain(..).collect::<Vec<_>>()
        };
        for t in tasks {
            let _ = t.await;
        }
    }

    async fn write_all(&self, frames: &[OutFrame]) -> Result<()> {
        trace!(
            target: "lucarne::runtime",
            session_id = self.id(),
            epoch = self.epoch(),
            frames = frames.len(),
            "writing outbound frames"
        );
        for f in frames {
            self.write_out(f).await?;
        }
        Ok(())
    }

    async fn emit_command_events(&self, mut events: Vec<Event>) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let tx = self
            .events_tx
            .lock()
            .expect("runtime event tx lock")
            .as_ref()
            .cloned()
            .ok_or_else(|| LucarneError::runtime("event stream closed"))?;
        for ev in &mut events {
            stamp(ev, &self.epoch);
        }
        for ev in events {
            tx.send(ev)
                .await
                .map_err(|_| LucarneError::runtime("event stream closed"))?;
        }
        Ok(())
    }

    async fn write_out(&self, f: &OutFrame) -> Result<()> {
        match f {
            OutFrame::Stdin(b) => {
                trace!(
                    target: "lucarne::runtime",
                    session_id = self.id(),
                    epoch = self.epoch(),
                    frame = out_frame_name(f),
                    bytes = b.len(),
                    "writing frame to stdin"
                );
                let mut guard = self.stdin.lock().await;
                let stdin = guard
                    .as_mut()
                    .ok_or_else(|| LucarneError::runtime("stdin closed"))?;
                stdin.write_all(b).await?;
                stdin.flush().await?;
                Ok(())
            }
            OutFrame::CloseStdin => {
                // sentinel: close stdin
                debug!(
                    target: "lucarne::runtime",
                    session_id = self.id(),
                    epoch = self.epoch(),
                    "closing stdin"
                );
                self.stdin.lock().await.take();
                Ok(())
            }
            OutFrame::Signal(sig) => {
                if let Some(proc) = &self.proc {
                    debug!(
                        target: "lucarne::runtime",
                        session_id = self.id(),
                        epoch = self.epoch(),
                        signal = sig,
                        "sending reactive signal"
                    );
                    proc.signal(sig)?;
                }
                Ok(())
            }
        }
    }
}

/// Start a runtime session.
pub async fn start(cfg: Config) -> Result<Session> {
    let Config {
        launcher,
        spec,
        framer,
        mut dialect,
        session_params,
        buffer_size,
        interrupt_grace,
    } = cfg;

    let buffer = if buffer_size == 0 {
        DEFAULT_BUFFER
    } else {
        buffer_size
    };
    info!(
        target: "lucarne::runtime",
        dialect = dialect.name(),
        buffer,
        interrupt_grace_ms = interrupt_grace.as_millis() as u64,
        bin = spec.bin.as_str(),
        cwd = spec.cwd.as_str(),
        args = spec.args.len(),
        "starting runtime session"
    );

    let proc = launcher.launch(&spec).await?;
    let proc = Arc::new(proc);

    let stdin = proc.take_stdin().await;
    let stdout = proc.take_stdout().await;
    let stderr = proc.take_stderr().await;

    let stdin_arc = Arc::new(AsyncMutex::new(stdin));

    let (tx, rx) = mpsc::channel::<Event>(buffer);
    let session_events_tx = Arc::new(StdMutex::new(Some(tx.clone())));
    let epoch = uuid::Uuid::new_v4().to_string();
    info!(
        target: "lucarne::runtime",
        pid = proc.pid(),
        epoch = epoch.as_str(),
        "runtime process launched"
    );

    // Write Init frames before starting pumps.
    let init_frames = dialect.init(&session_params);
    if !init_frames.is_empty() {
        debug!(
            target: "lucarne::runtime",
            epoch = epoch.as_str(),
            init_frames = init_frames.len(),
            "writing init frames"
        );
        for f in &init_frames {
            match f {
                OutFrame::Stdin(b) => {
                    let mut g = stdin_arc.lock().await;
                    if let Some(s) = g.as_mut() {
                        s.write_all(b).await?;
                        s.flush().await?;
                    }
                }
                OutFrame::CloseStdin => {
                    stdin_arc.lock().await.take();
                }
                OutFrame::Signal(name) => {
                    proc.signal(name)?;
                }
            }
        }
    }

    let dialect_arc: Arc<AsyncMutex<Box<dyn Dialect>>> = Arc::new(AsyncMutex::new(dialect));
    let cancel = tokio_util_cancel::CancelFlag::new();

    let mut io_tasks = Vec::new();

    // pumpFrames
    if let Some(stdout) = stdout {
        let frames_rx = framer.run(stdout, 256);
        let tx_p = tx.clone();
        let dialect_p = Arc::clone(&dialect_arc);
        let stdin_p = Arc::clone(&stdin_arc);
        let proc_p = Arc::clone(&proc);
        let epoch_p = epoch.clone();
        let cancel_p = cancel.clone();
        io_tasks.push(tokio::spawn(async move {
            pump_frames(
                frames_rx, tx_p, dialect_p, stdin_p, proc_p, epoch_p, cancel_p,
            )
            .await;
        }));
    }

    // pumpStderr
    if let Some(stderr) = stderr {
        let tx_s = tx.clone();
        let epoch_s = epoch.clone();
        let cancel_s = cancel.clone();
        io_tasks.push(tokio::spawn(async move {
            pump_stderr(stderr, tx_s, epoch_s, cancel_s).await;
        }));
    }

    let mut tasks = Vec::new();

    // pumpExit: await process exit, then flush OnExit and drop tx.
    {
        let proc_e = Arc::clone(&proc);
        let dialect_e = Arc::clone(&dialect_arc);
        let tx_e = tx.clone();
        let session_events_tx_e = Arc::clone(&session_events_tx);
        let epoch_e = epoch.clone();
        tasks.push(tokio::spawn(async move {
            let info = proc_e.wait().await;
            for task in io_tasks {
                let _ = task.await;
            }
            // Match Go: `exit.Err` is the OS-level error (signal, I/O
            // failure), NOT a synthesized "exit code N" string. Normal
            // non-zero exits pass `err = nil` and the dialect decides
            // what message to compose from the code.
            let exit_err = if info.signalled {
                Some("signal: process killed".to_string())
            } else {
                None
            };
            let evs = {
                let mut d = dialect_e.lock().await;
                d.on_exit(info.code, exit_err)
            };
            for mut ev in evs {
                stamp(&mut ev, &epoch_e);
                let _ = tx_e.send(ev).await;
            }
            session_events_tx_e
                .lock()
                .expect("runtime event tx lock")
                .take();
            drop(tx_e);
        }));
    }

    // Release the original sender so the channel closes when all pumps drop.
    drop(tx);

    Ok(Session {
        id: uuid::Uuid::new_v4().to_string(),
        epoch,
        proc: Some(Arc::clone(&proc)),
        stdin: stdin_arc,
        dialect: dialect_arc,
        events_rx: StdMutex::new(Some(rx)),
        events_tx: session_events_tx,
        grace: interrupt_grace,
        tasks: StdMutex::new(tasks),
        cancel,
    })
}

async fn pump_frames(
    mut frames_rx: mpsc::Receiver<crate::framer::Frame>,
    tx: mpsc::Sender<Event>,
    dialect: Arc<AsyncMutex<Box<dyn Dialect>>>,
    stdin: Arc<AsyncMutex<Option<tokio::process::ChildStdin>>>,
    _proc: Arc<Process>,
    epoch: String,
    cancel: tokio_util_cancel::CancelFlag,
) {
    debug!(
        target: "lucarne::runtime",
        pid = _proc.pid(),
        epoch = epoch.as_str(),
        "stdout frame pump started"
    );
    loop {
        let frame = tokio::select! {
            biased;
            _ = cancel.wait() => return,
            f = frames_rx.recv() => f,
        };
        let Some(frame) = frame else { return };
        let bytes = match frame {
            Ok(b) => b,
            Err(e) => {
                warn!(
                    target: "lucarne::runtime",
                    pid = _proc.pid(),
                    epoch = epoch.as_str(),
                    error = %e,
                    "framer error"
                );
                let mut ev = Event::new(Payload::Log(LogLine {
                    level: "error".into(),
                    stream: "stdout".into(),
                    text: format!("framer: {}", e),
                }));
                stamp(&mut ev, &epoch);
                let _ = tx.send(ev).await;
                return;
            }
        };
        let (events, out_frames) = {
            let mut d = dialect.lock().await;
            let evs = d.translate(&bytes);
            let outs = d.drain_out_frames();
            (evs, outs)
        };
        trace!(
            target: "lucarne::runtime",
            pid = _proc.pid(),
            epoch = epoch.as_str(),
            frame_bytes = bytes.len(),
            events = events.len(),
            out_frames = out_frames.len(),
            "translated frame"
        );
        for mut ev in events {
            stamp(&mut ev, &epoch);
            trace!(
                target: "lucarne::runtime",
                pid = _proc.pid(),
                epoch = epoch.as_str(),
                kind = ?ev.payload.kind(),
                "emitting translated event"
            );
            if tx.send(ev).await.is_err() {
                return;
            }
        }
        for of in out_frames {
            match of {
                OutFrame::Stdin(b) => {
                    let mut g = stdin.lock().await;
                    if let Some(s) = g.as_mut() {
                        if let Err(e) = s.write_all(&b).await {
                            let mut ev = Event::new(Payload::Log(LogLine {
                                level: "error".into(),
                                stream: "stdout".into(),
                                text: format!("runtime: reactive write failed: {}", e),
                            }));
                            stamp(&mut ev, &epoch);
                            let _ = tx.send(ev).await;
                            return;
                        }
                        let _ = s.flush().await;
                    }
                }
                OutFrame::CloseStdin => {
                    stdin.lock().await.take();
                }
                OutFrame::Signal(_name) => {
                    // Reactive signal; rare, just ignore errors.
                    // no-op: we don't currently need to escalate here
                }
            }
        }
    }
}

async fn pump_stderr(
    mut r: Box<dyn tokio::io::AsyncRead + Unpin + Send>,
    tx: mpsc::Sender<Event>,
    epoch: String,
    cancel: tokio_util_cancel::CancelFlag,
) {
    debug!(target: "lucarne::runtime", epoch = epoch.as_str(), "stderr pump started");
    let mut buf = [0u8; 4096];
    loop {
        tokio::select! {
            biased;
            _ = cancel.wait() => return,
            res = r.read(&mut buf) => {
                match res {
                    Ok(0) => return,
                    Ok(n) => {
                        trace!(
                            target: "lucarne::runtime",
                            epoch = epoch.as_str(),
                            bytes = n,
                            "forwarding stderr chunk"
                        );
                        let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                        let mut ev = Event::new(Payload::Log(LogLine {
                            level: "info".into(),
                            stream: "stderr".into(),
                            text,
                        }));
                        stamp(&mut ev, &epoch);
                        if tx.send(ev).await.is_err() { return; }
                    }
                    Err(_) => return,
                }
            }
        }
    }
}

fn stamp(ev: &mut Event, epoch: &str) {
    if ev.ts.is_empty() {
        ev.ts = now_rfc3339();
    }
    // Go always overwrites epoch (ev.Epoch = s.epoch) — match that.
    ev.epoch = epoch.to_string();
}

// No-op dialect for synthetic sessions.
struct NoopDialect;
impl Dialect for NoopDialect {
    fn name(&self) -> &'static str {
        "noop"
    }
    fn translate(&mut self, _b: &[u8]) -> Vec<Event> {
        Vec::new()
    }
    fn encode_user_message(&mut self, _i: &Input) -> Result<Vec<OutFrame>> {
        Err(LucarneError::runtime("synthetic session: no dialect"))
    }
    fn encode_permission_response(
        &mut self,
        _req_id: &str,
        _r: &PermissionResponse,
    ) -> Result<Vec<OutFrame>> {
        Err(LucarneError::runtime("synthetic session: no dialect"))
    }
    fn encode_interrupt(&mut self) -> Result<Vec<OutFrame>> {
        Err(LucarneError::runtime("synthetic session: no dialect"))
    }
}

mod tokio_util_cancel {
    use tokio::sync::Notify;

    #[derive(Clone)]
    pub struct CancelFlag {
        notify: std::sync::Arc<Notify>,
        flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl CancelFlag {
        pub fn new() -> Self {
            Self {
                notify: std::sync::Arc::new(Notify::new()),
                flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }
        }
        pub fn set(&self) {
            self.flag.store(true, std::sync::atomic::Ordering::SeqCst);
            self.notify.notify_waiters();
        }
        pub async fn wait(&self) {
            loop {
                if self.flag.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                self.notify.notified().await;
            }
        }
    }
}

/// For dialect tests that just want to instantiate a Session with a
/// synthetic event channel (e.g., the capture adapter from the Go
/// harness).
pub fn synthetic_session() -> (Session, mpsc::Sender<Event>) {
    let (tx, rx) = mpsc::channel(16);
    let tx_clone = tx.clone();
    (Session::new_synthetic(rx, tx), tx_clone)
}

// Suppress warning about ExitInfo unused import when other modules
// drop it later.

fn _keep_exit_info(_: ExitInfo) {}

fn out_frame_name(frame: &OutFrame) -> &'static str {
    match frame {
        OutFrame::Stdin(_) => "stdin",
        OutFrame::CloseStdin => "close_stdin",
        OutFrame::Signal(_) => "signal",
    }
}
