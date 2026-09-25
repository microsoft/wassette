// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

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

const MAX_PENDING_LINE_BYTES: usize = 64 * 1024;

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

    fn emit(&self, bytes: &[u8], continued: bool) {
        let text = String::from_utf8_lossy(bytes);
        if continued {
            tracing::info!(target: "wasm_stderr", "{}: {} [line continues]", self.target, text);
        } else {
            tracing::info!(target: "wasm_stderr", "{}: {}", self.target, text);
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
        for part in bytes.split_inclusive(|&b| b == b'\n') {
            let (mut content, complete) = match part.strip_suffix(b"\n") {
                Some(content) => (content, true),
                None => (part, false),
            };
            while !content.is_empty() {
                if buf.len() == MAX_PENDING_LINE_BYTES {
                    self.emit(&buf, true);
                    buf.clear();
                }
                let count = content.len().min(MAX_PENDING_LINE_BYTES - buf.len());
                buf.extend_from_slice(&content[..count]);
                content = &content[count..];
            }
            if complete {
                self.emit(&buf, false);
                buf.clear();
            }
        }
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut buf = self.buf.lock().unwrap();
        if !buf.is_empty() {
            self.emit(&buf, false);
            buf.clear();
        }
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_unterminated_lines_are_emitted_in_bounded_chunks() {
        let log = tempfile::NamedTempFile::new().unwrap();
        let writer = log.reopen().unwrap();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.try_clone().unwrap())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let mut stream = TracingStream::new("stdout");
        let input = vec![b'x'; MAX_PENDING_LINE_BYTES * 3 + 5];
        let cx = &mut Context::from_waker(std::task::Waker::noop());
        assert!(matches!(
            Pin::new(&mut stream).poll_write(cx, &input),
            Poll::Ready(Ok(n)) if n == input.len()
        ));
        assert_eq!(stream.buf.lock().unwrap().len(), 5);
        Pin::new(&mut stream).poll_shutdown(cx);
        let output = std::fs::read_to_string(log.path()).unwrap();
        let lines: Vec<_> = output.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(
            lines[..3]
                .iter()
                .all(|line| line.contains("[line continues]"))
        );
        assert!(lines[3].ends_with("xxxxx"));
        assert!(
            lines
                .iter()
                .all(|line| line.len() < MAX_PENDING_LINE_BYTES + 256)
        );
    }
}
