//! Framer: turns a byte stream from a process into a sequence of
//! logical frames that dialects can parse.
//!
//! Framing is the boundary at which OS-level backpressure starts to
//! flow upstream: when the downstream event channel is full, frame
//! production blocks, the OS pipe buffer fills, and the agent's
//! `write(2)` calls block. No drops, no surprises.
//!
//! Two frame modes:
//!
//! * [`FramerKind::NewlineJson`] — one JSON object per LF-terminated line.
//!   Used by Claude `stream-json`, Copilot, Pi, Gemini (in some modes).
//! * [`FramerKind::JsonRpc`] — auto-detects LF-delimited JSON-RPC (used
//!   by Codex `app-server --listen stdio`) vs. LSP-style `Content-Length:`
//!   header framing.

use crate::error::{LucarneError, Result};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

pub type Frame = Result<Vec<u8>>;

const LINE_BUFFER_CAPACITY: usize = 4096;

#[derive(Debug, Clone, Copy)]
pub enum FramerKind {
    NewlineJson,
    JsonRpc,
}

#[derive(Debug, Clone, Copy)]
pub struct Framer {
    pub kind: FramerKind,
    pub max_line: usize,
}

impl Framer {
    pub fn newline_json() -> Self {
        Self {
            kind: FramerKind::NewlineJson,
            max_line: 8 << 20,
        }
    }
    pub fn jsonrpc() -> Self {
        Self {
            kind: FramerKind::JsonRpc,
            max_line: 16 << 20,
        }
    }

    /// Consume the reader on a dedicated task; each frame (or terminal
    /// error) is forwarded through the returned channel. The task exits
    /// silently on EOF; on read error it sends one final `Err` frame
    /// and then closes the channel.
    pub fn run<R>(self, mut reader: R, buf: usize) -> mpsc::Receiver<Frame>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let (tx, rx) = mpsc::channel::<Frame>(buf.max(1));
        debug!(
            target: "lucarne::framer",
            kind = ?self.kind,
            max_line = self.max_line,
            buffer = buf.max(1),
            "starting framer task"
        );
        tokio::spawn(async move {
            let res = match self.kind {
                FramerKind::NewlineJson => run_newline(&mut reader, &tx, self.max_line).await,
                FramerKind::JsonRpc => run_jsonrpc(&mut reader, &tx, self.max_line).await,
            };
            if let Err(e) = res {
                warn!(target: "lucarne::framer", error = %e, "framer task failed");
                let _ = tx.send(Err(e)).await;
            }
        });
        rx
    }
}

async fn run_newline<R>(reader: &mut R, tx: &mpsc::Sender<Frame>, max_line: usize) -> Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut br = BufReader::with_capacity(64 * 1024, reader);
    let mut buf = Vec::with_capacity(LINE_BUFFER_CAPACITY);
    loop {
        buf.clear();
        let n = read_until_capped(&mut br, b'\n', &mut buf, max_line).await?;
        if n == 0 {
            return Ok(());
        }
        // Strip trailing \n (and optional \r).
        if buf.last().copied() == Some(b'\n') {
            buf.pop();
            if buf.last().copied() == Some(b'\r') {
                buf.pop();
            }
        }
        if buf.iter().all(|b| b.is_ascii_whitespace()) {
            continue;
        }
        trace!(
            target: "lucarne::framer",
            mode = "newline_json",
            bytes = buf.len(),
            "framed newline-delimited payload"
        );
        let frame = std::mem::replace(&mut buf, Vec::with_capacity(LINE_BUFFER_CAPACITY));
        if tx.send(Ok(frame)).await.is_err() {
            return Ok(()); // consumer gone
        }
    }
}

async fn run_jsonrpc<R>(reader: &mut R, tx: &mpsc::Sender<Frame>, max_msg: usize) -> Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut br = BufReader::with_capacity(64 * 1024, reader);
    // Peek up to 16 bytes to decide mode.
    let peek = br.fill_buf().await?;
    if peek.is_empty() {
        return Ok(());
    }
    let looks_like_header = {
        let trimmed = peek
            .iter()
            .position(|b| !matches!(*b, b' ' | b'\t' | b'\r' | b'\n'))
            .map(|i| &peek[i..])
            .unwrap_or(peek);
        trimmed.starts_with(b"Content-Length:")
    };
    if looks_like_header {
        debug!(target: "lucarne::framer", mode = "jsonrpc_header", "detected Content-Length framing");
        run_jsonrpc_header(&mut br, tx, max_msg).await
    } else {
        debug!(target: "lucarne::framer", mode = "jsonrpc_newline", "detected newline JSON-RPC framing");
        run_newline_inner(&mut br, tx, max_msg).await
    }
}

async fn run_newline_inner<R>(
    br: &mut BufReader<R>,
    tx: &mpsc::Sender<Frame>,
    max_line: usize,
) -> Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut buf = Vec::with_capacity(LINE_BUFFER_CAPACITY);
    loop {
        buf.clear();
        let n = read_until_capped(br, b'\n', &mut buf, max_line).await?;
        if n == 0 {
            return Ok(());
        }
        if buf.last().copied() == Some(b'\n') {
            buf.pop();
            if buf.last().copied() == Some(b'\r') {
                buf.pop();
            }
        }
        if buf.iter().all(|b| b.is_ascii_whitespace()) {
            continue;
        }
        trace!(
            target: "lucarne::framer",
            mode = "jsonrpc_newline",
            bytes = buf.len(),
            "framed newline JSON-RPC payload"
        );
        let frame = std::mem::replace(&mut buf, Vec::with_capacity(LINE_BUFFER_CAPACITY));
        if tx.send(Ok(frame)).await.is_err() {
            return Ok(());
        }
    }
}

async fn run_jsonrpc_header<R>(
    br: &mut BufReader<R>,
    tx: &mpsc::Sender<Frame>,
    max_msg: usize,
) -> Result<()>
where
    R: AsyncRead + Unpin,
{
    loop {
        let mut length: i64 = -1;
        let mut header_line = Vec::with_capacity(64);
        loop {
            header_line.clear();
            let n = br.read_until(b'\n', &mut header_line).await?;
            if n == 0 {
                return Ok(());
            }
            // strip trailing \r\n
            while matches!(header_line.last(), Some(b'\n') | Some(b'\r')) {
                header_line.pop();
            }
            if header_line.is_empty() {
                break;
            }
            // "Content-Length: 123"
            if let Some(colon) = header_line.iter().position(|b| *b == b':') {
                let (k, v) = header_line.split_at(colon);
                if k.eq_ignore_ascii_case(b"Content-Length") {
                    let tail = std::str::from_utf8(&v[1..])
                        .map_err(|e| LucarneError::protocol(format!("jsonrpc header: {}", e)))?
                        .trim();
                    length = tail.parse().map_err(|e: std::num::ParseIntError| {
                        LucarneError::protocol(format!("jsonrpc header: {}", e))
                    })?;
                }
            }
        }
        if length < 0 {
            return Err(LucarneError::protocol(
                "jsonrpc framer: missing Content-Length",
            ));
        }
        if (length as usize) > max_msg {
            return Err(LucarneError::protocol(
                "jsonrpc framer: message exceeds MaxMessage",
            ));
        }
        let mut body = vec![0u8; length as usize];
        br.read_exact(&mut body).await?;
        trace!(
            target: "lucarne::framer",
            mode = "jsonrpc_header",
            bytes = body.len(),
            "framed header-based JSON-RPC payload"
        );
        if tx.send(Ok(body)).await.is_err() {
            return Ok(());
        }
    }
}

async fn read_until_capped<R>(
    br: &mut BufReader<R>,
    delim: u8,
    out: &mut Vec<u8>,
    max: usize,
) -> Result<usize>
where
    R: AsyncRead + Unpin,
{
    let start = out.len();
    loop {
        let available = br.fill_buf().await?;
        if available.is_empty() {
            return Ok(out.len() - start);
        }
        if let Some(pos) = available.iter().position(|b| *b == delim) {
            out.extend_from_slice(&available[..=pos]);
            if out.len() - start > max {
                return Err(LucarneError::protocol("framer: line exceeds max size"));
            }
            let consumed = pos + 1;
            br.consume(consumed);
            return Ok(out.len() - start);
        }
        out.extend_from_slice(available);
        if out.len() - start > max {
            return Err(LucarneError::protocol("framer: line exceeds max size"));
        }
        let consumed = available.len();
        br.consume(consumed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tokio::io::BufReader as TokioBufReader;

    async fn collect(rx: &mut mpsc::Receiver<Frame>) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while let Some(f) = rx.recv().await {
            match f {
                Ok(b) => out.push(b),
                Err(_) => break,
            }
        }
        out
    }

    #[tokio::test]
    async fn newline_json_basic() {
        let input = b"{\"a\":1}\n\n  \n{\"b\":2}\n".to_vec();
        let reader = TokioBufReader::new(Cursor::new(input));
        let mut rx = Framer::newline_json().run(reader, 8);
        let frames = collect(&mut rx).await;
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], b"{\"a\":1}");
        assert_eq!(frames[1], b"{\"b\":2}");
    }

    #[test]
    fn newline_framers_move_completed_buffers_without_cloning_payloads() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/framer.rs"),
        )
        .expect("read framer source");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("production source");

        assert!(
            !production.contains("Ok(buf.clone())"),
            "newline framers should move completed payload buffers instead of cloning provider output"
        );
    }

    #[tokio::test]
    async fn jsonrpc_line_mode() {
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1}\n".to_vec();
        let reader = TokioBufReader::new(Cursor::new(input));
        let mut rx = Framer::jsonrpc().run(reader, 8);
        let frames = collect(&mut rx).await;
        assert_eq!(frames.len(), 1);
    }

    #[tokio::test]
    async fn jsonrpc_header_mode() {
        let body = br#"{"jsonrpc":"2.0","id":1}"#;
        let mut input = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        input.extend_from_slice(body);
        let reader = TokioBufReader::new(Cursor::new(input));
        let mut rx = Framer::jsonrpc().run(reader, 8);
        let frames = collect(&mut rx).await;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], body);
    }
}
