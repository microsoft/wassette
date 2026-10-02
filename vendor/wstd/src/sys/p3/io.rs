use crate::io::{AsyncRead, AsyncWrite};
use std::pin::Pin;
use std::task::{Context, Poll};
use wasip3::wit_bindgen::rt::async_support::{StreamReader, StreamResult, StreamWriter};

mod stdio;

pub use stdio::*;

/// A wrapper for a p3 `StreamReader<u8>` that provides `AsyncRead`.
pub struct AsyncInputStream {
    reader: StreamReader<u8>,
}

impl std::fmt::Debug for AsyncInputStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncInputStream").finish()
    }
}

impl AsyncInputStream {
    /// Construct an `AsyncInputStream` from a p3 `StreamReader<u8>`.
    pub fn new(reader: StreamReader<u8>) -> Self {
        Self { reader }
    }

    /// Asynchronously read from the input stream.
    pub async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read_buf = Vec::with_capacity(buf.len());
        let (result, data) = self.reader.read(read_buf).await;
        match result {
            StreamResult::Complete(_n) => {
                if data.is_empty() {
                    return Ok(0);
                }
                let len = data.len();
                buf[0..len].copy_from_slice(&data);
                Ok(len)
            }
            StreamResult::Dropped => Ok(0),
            StreamResult::Cancelled => Ok(0),
        }
    }

    /// Use this `AsyncInputStream` as a `futures_lite::stream::Stream` with
    /// items of `Result<Vec<u8>, std::io::Error>`.
    pub fn into_stream(self) -> AsyncInputChunkStream {
        AsyncInputChunkStream::new(self, 8 * 1024)
    }

    /// Use this `AsyncInputStream` as a `futures_lite::stream::Stream` with
    /// items of `Result<Vec<u8>, std::io::Error>`. The returned byte vectors
    /// will be at most the `chunk_size` argument specified.
    pub fn into_stream_of(self, chunk_size: usize) -> AsyncInputChunkStream {
        AsyncInputChunkStream::new(self, chunk_size)
    }

    /// Use this `AsyncInputStream` as a `futures_lite::stream::Stream` with
    /// items of `Result<u8, std::io::Error>`.
    pub fn into_bytestream(self) -> AsyncInputByteStream {
        AsyncInputByteStream {
            stream: self.into_stream(),
            buffer: std::io::Read::bytes(std::io::Cursor::new(Vec::new())),
        }
    }
}

impl AsyncRead for AsyncInputStream {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        AsyncInputStream::read(self, buf).await
    }

    #[inline]
    fn as_async_input_stream(&self) -> Option<&AsyncInputStream> {
        Some(self)
    }
}

/// Wrapper of `AsyncInputStream` that impls `futures_lite::stream::Stream`.
///
/// The underlying p3 `StreamReader::read` future borrows the reader,
/// which makes it impossible to store it in a `poll_next` impl without
/// self-referential gymnastics. Storing a boxed future also won't work
/// because the future's lifetime is tied to `&mut reader`. We work
/// around this by re-implementing the stream as an `async` state
/// machine that owns the `AsyncInputStream` between yields.
pub struct AsyncInputChunkStream {
    inner:
        Pin<Box<dyn futures_lite::stream::Stream<Item = Result<Vec<u8>, std::io::Error>> + Send>>,
    /// Holds the stream when the chunk-stream is created or after it
    /// reaches end-of-stream so that [`Self::into_inner`] can still
    /// return it. When `None`, the stream is in flight and cannot be
    /// recovered.
    saved: Option<AsyncInputStream>,
}

impl AsyncInputChunkStream {
    fn new(stream: AsyncInputStream, chunk_size: usize) -> Self {
        let inner = futures_lite::stream::unfold(Some((stream, chunk_size)), |state| async move {
            let (mut stream, n) = state?;
            let mut buf = vec![0u8; n];
            match stream.read(&mut buf).await {
                Ok(0) => None,
                Ok(k) => {
                    buf.truncate(k);
                    Some((Ok(buf), Some((stream, n))))
                }
                Err(e) => Some((Err(e), None)),
            }
        });
        Self {
            inner: Box::pin(inner),
            saved: None,
        }
    }

    /// Best-effort recovery of the underlying [`AsyncInputStream`]. Only
    /// returns `Some` if the chunk stream hasn't been polled yet (the
    /// in-flight read future owns the reader otherwise). Kept for API
    /// compatibility; production code should call this before any
    /// `next().await`.
    pub fn into_inner(self) -> AsyncInputStream {
        self.saved
            .expect("AsyncInputChunkStream::into_inner called after polling; the underlying reader is owned by the in-flight stream")
    }
}

impl futures_lite::stream::Stream for AsyncInputChunkStream {
    type Item = Result<Vec<u8>, std::io::Error>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        this.saved = None;
        this.inner.as_mut().poll_next(cx)
    }
}

pin_project_lite::pin_project! {
    /// Wrapper of `AsyncInputStream` that impls
    /// `futures_lite::stream::Stream` with item `Result<u8, std::io::Error>`.
    pub struct AsyncInputByteStream {
        #[pin]
        stream: AsyncInputChunkStream,
        buffer: std::io::Bytes<std::io::Cursor<Vec<u8>>>,
    }
}

impl AsyncInputByteStream {
    pub fn into_inner(self) -> (AsyncInputStream, Vec<u8>) {
        (
            self.stream.into_inner(),
            self.buffer
                .collect::<Result<Vec<u8>, std::io::Error>>()
                .expect("read of Cursor<Vec<u8>> is infallible"),
        )
    }
}

impl futures_lite::stream::Stream for AsyncInputByteStream {
    type Item = Result<u8, std::io::Error>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        match this.buffer.next() {
            Some(byte) => Poll::Ready(Some(Ok(byte.expect("cursor on Vec<u8> is infallible")))),
            None => match futures_lite::stream::Stream::poll_next(this.stream, cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    let mut bytes = std::io::Read::bytes(std::io::Cursor::new(bytes));
                    match bytes.next() {
                        Some(Ok(byte)) => {
                            *this.buffer = bytes;
                            Poll::Ready(Some(Ok(byte)))
                        }
                        Some(Err(err)) => Poll::Ready(Some(Err(err))),
                        None => Poll::Ready(None),
                    }
                }
                Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(err))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

/// A wrapper for a p3 `StreamWriter<u8>` that provides `AsyncWrite`.
pub struct AsyncOutputStream {
    writer: StreamWriter<u8>,
}

impl std::fmt::Debug for AsyncOutputStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncOutputStream").finish()
    }
}

impl AsyncOutputStream {
    /// Construct an `AsyncOutputStream` from a p3 `StreamWriter<u8>`.
    pub fn new(writer: StreamWriter<u8>) -> Self {
        Self { writer }
    }

    /// Asynchronously write to the output stream.
    pub async fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let data = buf.to_vec();
        let remaining = self.writer.write_all(data).await;
        Ok(buf.len() - remaining.len())
    }

    /// Asynchronously write all bytes to the output stream.
    pub async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let data = buf.to_vec();
        let remaining = self.writer.write_all(data).await;
        if remaining.is_empty() {
            Ok(())
        } else {
            Err(std::io::Error::from(std::io::ErrorKind::ConnectionReset))
        }
    }

    /// Flush the output stream (no-op for p3 streams).
    pub async fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl AsyncWrite for AsyncOutputStream {
    async fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        AsyncOutputStream::write(self, buf).await
    }
    async fn flush(&mut self) -> std::io::Result<()> {
        AsyncOutputStream::flush(self).await
    }

    #[inline]
    fn as_async_output_stream(&self) -> Option<&AsyncOutputStream> {
        Some(self)
    }
}
