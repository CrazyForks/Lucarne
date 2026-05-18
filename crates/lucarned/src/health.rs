use std::{
    fmt,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use lucarne::{
    core_service::{HistoryWatchState, HistoryWatchStatus},
    LucarneCore,
};
use lucarne_adapter::{AdapterState, AdapterStatus, AdapterStatusReader};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{watch, Semaphore},
    time::timeout,
};
use tracing::{debug, warn};

const HEALTH_IO_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_HEALTH_CONNECTIONS: usize = 16;

#[derive(Debug)]
pub enum HealthError {
    InvalidAddress(String),
    NonLoopback(SocketAddr),
    Io(std::io::Error),
}

impl fmt::Display for HealthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAddress(value) => write!(f, "invalid health address: {value}"),
            Self::NonLoopback(addr) => {
                write!(f, "health address must be loopback-only: {addr}")
            }
            Self::Io(err) => write!(f, "health io: {err}"),
        }
    }
}

impl std::error::Error for HealthError {}

impl From<std::io::Error> for HealthError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Clone)]
pub struct HealthState {
    core: Arc<LucarneCore>,
    adapters: AdapterStatusReader,
    db_path: PathBuf,
    started_at_unix_ms: u64,
}

impl HealthState {
    pub fn new(core: Arc<LucarneCore>, adapters: AdapterStatusReader, db_path: PathBuf) -> Self {
        Self {
            core,
            adapters,
            db_path,
            started_at_unix_ms: unix_ms_now(),
        }
    }

    fn snapshot(&self) -> HealthSnapshot {
        HealthSnapshot {
            adapters: self.adapters.snapshot(),
            history_watch: self.core.history_watch_status(),
            db_path: self.db_path.display().to_string(),
            started_at_unix_ms: self.started_at_unix_ms,
        }
    }
}

#[derive(Debug, Clone)]
struct HealthSnapshot {
    adapters: Vec<AdapterStatus>,
    history_watch: HistoryWatchStatus,
    db_path: String,
    started_at_unix_ms: u64,
}

pub fn parse_health_addr(value: &str) -> Result<SocketAddr, HealthError> {
    let addr = value
        .parse::<SocketAddr>()
        .map_err(|_| HealthError::InvalidAddress(value.to_string()))?;
    if !addr.ip().is_loopback() {
        return Err(HealthError::NonLoopback(addr));
    }
    Ok(addr)
}

pub async fn bind_health_listener(addr: SocketAddr) -> Result<TcpListener, HealthError> {
    if !addr.ip().is_loopback() {
        return Err(HealthError::NonLoopback(addr));
    }
    Ok(TcpListener::bind(addr).await?)
}

pub async fn serve_health(
    listener: TcpListener,
    state: HealthState,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), HealthError> {
    let permits = Arc::new(Semaphore::new(MAX_HEALTH_CONNECTIONS));
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            accepted = listener.accept() => {
                let (stream, peer) = accepted?;
                if !peer.ip().is_loopback() {
                    debug!(peer = %peer, "rejecting non-loopback health client");
                    continue;
                }
                let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                    warn!(
                        max_connections = MAX_HEALTH_CONNECTIONS,
                        "health connection limit reached"
                    );
                    continue;
                };
                let state = state.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(err) = handle_health_connection(stream, state).await {
                        warn!(error = %err, "health connection failed");
                    }
                });
            }
        }
    }
}

async fn handle_health_connection(
    mut stream: TcpStream,
    state: HealthState,
) -> Result<(), std::io::Error> {
    let mut buf = [0_u8; 1024];
    let read = match timeout(HEALTH_IO_TIMEOUT, stream.read(&mut buf)).await {
        Ok(read) => read?,
        Err(_) => return Ok(()),
    };
    let request = String::from_utf8_lossy(&buf[..read]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let response = render_health_response(path, &state.snapshot());
    if let Ok(write) = timeout(HEALTH_IO_TIMEOUT, stream.write_all(response.as_bytes())).await {
        write?;
    }
    if let Ok(shutdown) = timeout(HEALTH_IO_TIMEOUT, stream.shutdown()).await {
        shutdown?;
    }
    Ok(())
}

fn render_health_response(path: &str, snapshot: &HealthSnapshot) -> String {
    match path {
        "/healthz" if healthy(snapshot) => http_response(200, "text/plain", "ok\n"),
        "/healthz" => http_response(503, "text/plain", "unhealthy\n"),
        "/readyz" if ready(snapshot) => http_response(200, "text/plain", "ready\n"),
        "/readyz" => http_response(503, "text/plain", "not ready\n"),
        "/status" => http_response(200, "application/json", &status_json(snapshot)),
        _ => http_response(404, "text/plain", "not found\n"),
    }
}

fn healthy(_snapshot: &HealthSnapshot) -> bool {
    true
}

fn ready(snapshot: &HealthSnapshot) -> bool {
    snapshot.adapters.iter().any(|status| {
        matches!(
            status.state,
            AdapterState::Starting | AdapterState::Running | AdapterState::Backoff
        )
    })
}

fn status_json(snapshot: &HealthSnapshot) -> String {
    let adapters = snapshot
        .adapters
        .iter()
        .map(|status| {
            json!({
                "id": status.id,
                "state": adapter_state(status.state),
                "restart_count": status.restart_count,
                "last_error": status.last_error,
                "last_started_at_unix_ms": status.last_started_at_unix_ms,
                "last_stopped_at_unix_ms": status.last_stopped_at_unix_ms,
                "next_retry_at_unix_ms": status.next_retry_at_unix_ms,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "uptime_ms": unix_ms_now().saturating_sub(snapshot.started_at_unix_ms),
        "started_at_unix_ms": snapshot.started_at_unix_ms,
        "db_path": snapshot.db_path,
        "adapters": adapters,
        "history_watch": {
            "state": history_watch_state(snapshot.history_watch.state),
            "running": snapshot.history_watch.running,
            "restart_count": snapshot.history_watch.restart_count,
            "last_error": snapshot.history_watch.last_error,
            "next_retry_at_unix_ms": snapshot.history_watch.next_retry_at_unix_ms,
        }
    })
    .to_string()
}

fn adapter_state(state: AdapterState) -> &'static str {
    match state {
        AdapterState::Starting => "starting",
        AdapterState::Running => "running",
        AdapterState::Backoff => "backoff",
        AdapterState::Degraded => "degraded",
        AdapterState::Stopped => "stopped",
    }
}

fn history_watch_state(state: HistoryWatchState) -> &'static str {
    match state {
        HistoryWatchState::Starting => "starting",
        HistoryWatchState::Running => "running",
        HistoryWatchState::Backoff => "backoff",
        HistoryWatchState::Degraded => "degraded",
        HistoryWatchState::Stopped => "stopped",
    }
}

fn http_response(status: u16, content_type: &str, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "OK",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_with_adapter_state(state: AdapterState) -> HealthSnapshot {
        HealthSnapshot {
            adapters: vec![AdapterStatus {
                id: "wechat",
                state,
                restart_count: 2,
                last_error: Some("network down".into()),
                last_started_at_unix_ms: Some(10),
                last_stopped_at_unix_ms: Some(20),
                next_retry_at_unix_ms: Some(30),
            }],
            history_watch: HistoryWatchStatus {
                state: HistoryWatchState::Running,
                running: true,
                restart_count: 0,
                last_error: None,
                next_retry_at_unix_ms: None,
            },
            db_path: "/tmp/lucarne.sqlite".into(),
            started_at_unix_ms: unix_ms_now(),
        }
    }

    #[test]
    fn health_addr_must_be_loopback() {
        assert!(parse_health_addr("127.0.0.1:17890").is_ok());
        assert!(parse_health_addr("[::1]:17890").is_ok());
        assert!(matches!(
            parse_health_addr("0.0.0.0:17890"),
            Err(HealthError::NonLoopback(_))
        ));
    }

    #[test]
    fn readyz_uses_adapter_snapshot_without_provider_calls() {
        let running = snapshot_with_adapter_state(AdapterState::Running);
        assert!(render_health_response("/readyz", &running).starts_with("HTTP/1.1 200"));

        let degraded = snapshot_with_adapter_state(AdapterState::Degraded);
        assert!(render_health_response("/readyz", &degraded).starts_with("HTTP/1.1 503"));
    }

    #[test]
    fn status_includes_adapter_and_history_watch_state() {
        let response = render_health_response(
            "/status",
            &snapshot_with_adapter_state(AdapterState::Backoff),
        );

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains(r#""id":"wechat""#));
        assert!(response.contains(r#""state":"backoff""#));
        assert!(response.contains(r#""history_watch""#));
        assert!(response.contains(r#""restart_count":0"#));
        assert!(response.contains(r#""db_path":"/tmp/lucarne.sqlite""#));
    }

    #[test]
    fn healthz_reports_daemon_alive_without_requiring_history_watcher() {
        let degraded = snapshot_with_adapter_state(AdapterState::Degraded);
        assert!(render_health_response("/healthz", &degraded).starts_with("HTTP/1.1 200"));

        let mut stopped_watcher = degraded;
        stopped_watcher.history_watch.running = false;
        stopped_watcher.history_watch.state = HistoryWatchState::Stopped;
        assert!(render_health_response("/healthz", &stopped_watcher).starts_with("HTTP/1.1 200"));
    }
}
