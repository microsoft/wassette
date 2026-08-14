//! WASI stdout/stderr adapter that pipes wasm guest output into the host's
//! `tracing` system. The default `inherit_stderr()` writes to the host
//! process's stderr — fine when run from a terminal, but invisible when
//! launched by an editor (Zed) that captures stderr separately. Routing
//! through `tracing::info!` ensures guest `eprintln!`s land in the
//! `--log-file` alongside host events.

use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::io::AsyncWrite;
use wasmtime_wasi::cli::{IsTerminal, StdoutStream};

/// Adapter that exposes a tokio `AsyncWrite` (used by wstd / wasi stdio)
/// and emits each completed line as a `tracing::info!` event under the
/// given target.
pub struct TracingStream {
    target: &'static str,
    buf: Arc<Mutex<Vec<u8>>>,
}

impl TracingStream {
    pub fn new(target: &'static str) -> Self {
        Self {
            target,
            buf: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Clone for TracingStream {
    fn clone(&self) -> Self {
        Self {
            target: self.target,
            buf: self.buf.clone(),
        }
    }
}

impl IsTerminal for TracingStream {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl StdoutStream for TracingStream {
    fn async_stream(&self) -> Box<dyn AsyncWrite + Send + Sync> {
        Box::new(self.clone())
    }
}

impl AsyncWrite for TracingStream {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut buf = self.buf.lock().unwrap();
        buf.extend_from_slice(bytes);
        // Emit complete lines as we accumulate them.
        while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=nl).collect();
            let line = &line[..line.len().saturating_sub(1)];
            let text = String::from_utf8_lossy(line);
            tracing::info!(target: "wasm_stderr", "{}: {}", self.target, text);
        }
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut buf = self.buf.lock().unwrap();
        if !buf.is_empty() {
            let text = String::from_utf8_lossy(&buf);
            tracing::info!(target: "wasm_stderr", "{}: {}", self.target, text);
            buf.clear();
        }
        Poll::Ready(Ok(()))
    }
}
