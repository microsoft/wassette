use crate::io::{AsyncInputStream, AsyncOutputStream, AsyncRead, AsyncWrite, Result};
use std::cell::LazyCell;
use wasip2::cli::terminal_input::TerminalInput;
use wasip2::cli::terminal_output::TerminalOutput;

/// Use the program's stdin as an `AsyncInputStream`.
#[derive(Debug)]
pub struct Stdin {
    stream: AsyncInputStream,
    terminput: LazyCell<Option<TerminalInput>>,
}

/// Get the program's stdin for use as an `AsyncInputStream`.
pub fn stdin() -> Stdin {
    let stream = AsyncInputStream::new(wasip2::cli::stdin::get_stdin());
    Stdin {
        stream,
        terminput: LazyCell::new(wasip2::cli::terminal_stdin::get_terminal_stdin),
    }
}

impl Stdin {
    /// Check if stdin is a terminal.
    pub fn is_terminal(&self) -> bool {
        LazyCell::force(&self.terminput).is_some()
    }

    /// Get the `AsyncInputStream` used to implement `Stdin`
    pub fn into_inner(self) -> AsyncInputStream {
        self.stream
    }
}

impl AsyncRead for Stdin {
    #[inline]
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.stream.read(buf).await
    }

    #[inline]
    async fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        self.stream.read_to_end(buf).await
    }

    #[inline]
    fn as_async_input_stream(&self) -> Option<&AsyncInputStream> {
        Some(&self.stream)
    }
}

/// Use the program's stdout as an `AsyncOutputStream`.
#[derive(Debug)]
pub struct Stdout {
    stream: AsyncOutputStream,
    termoutput: LazyCell<Option<TerminalOutput>>,
}

/// Get the program's stdout for use as an `AsyncOutputStream`.
pub fn stdout() -> Stdout {
    let stream = AsyncOutputStream::new(wasip2::cli::stdout::get_stdout());
    Stdout {
        stream,
        termoutput: LazyCell::new(wasip2::cli::terminal_stdout::get_terminal_stdout),
    }
}

impl Stdout {
    /// Check if stdout is a terminal.
    pub fn is_terminal(&self) -> bool {
        LazyCell::force(&self.termoutput).is_some()
    }

    /// Get the `AsyncOutputStream` used to implement `Stdout`
    pub fn into_inner(self) -> AsyncOutputStream {
        self.stream
    }
}

impl AsyncWrite for Stdout {
    #[inline]
    async fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.stream.write(buf).await
    }

    #[inline]
    async fn flush(&mut self) -> Result<()> {
        self.stream.flush().await
    }

    #[inline]
    async fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.stream.write_all(buf).await
    }

    #[inline]
    fn as_async_output_stream(&self) -> Option<&AsyncOutputStream> {
        self.stream.as_async_output_stream()
    }
}

/// Use the program's stdout as an `AsyncOutputStream`.
#[derive(Debug)]
pub struct Stderr {
    stream: AsyncOutputStream,
    termoutput: LazyCell<Option<TerminalOutput>>,
}

/// Get the program's stdout for use as an `AsyncOutputStream`.
pub fn stderr() -> Stderr {
    let stream = AsyncOutputStream::new(wasip2::cli::stderr::get_stderr());
    Stderr {
        stream,
        termoutput: LazyCell::new(wasip2::cli::terminal_stderr::get_terminal_stderr),
    }
}

impl Stderr {
    /// Check if stderr is a terminal.
    pub fn is_terminal(&self) -> bool {
        LazyCell::force(&self.termoutput).is_some()
    }

    /// Get the `AsyncOutputStream` used to implement `Stderr`
    pub fn into_inner(self) -> AsyncOutputStream {
        self.stream
    }
}

impl AsyncWrite for Stderr {
    #[inline]
    async fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.stream.write(buf).await
    }

    #[inline]
    async fn flush(&mut self) -> Result<()> {
        self.stream.flush().await
    }

    #[inline]
    async fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.stream.write_all(buf).await
    }

    #[inline]
    fn as_async_output_stream(&self) -> Option<&AsyncOutputStream> {
        self.stream.as_async_output_stream()
    }
}

#[cfg(test)]
mod test {
    use crate::io::AsyncWrite;
    use crate::runtime::block_on;
    #[test]
    // No internal predicate. Run test with --nocapture and inspect output manually.
    fn stdout_println_hello_world() {
        block_on(async {
            let mut stdout = super::stdout();
            let term = if stdout.is_terminal() { "is" } else { "is not" };
            stdout
                .write_all(format!("hello, world! stdout {term} a terminal\n",).as_bytes())
                .await
                .unwrap();
        })
    }
    #[test]
    // No internal predicate. Run test with --nocapture and inspect output manually.
    fn stderr_println_hello_world() {
        block_on(async {
            let mut stdout = super::stdout();
            let term = if stdout.is_terminal() { "is" } else { "is not" };
            stdout
                .write_all(format!("hello, world! stderr {term} a terminal\n",).as_bytes())
                .await
                .unwrap();
        })
    }
}
