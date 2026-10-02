use crate::io;

/// Write bytes to a sink.
pub trait AsyncWrite {
    // Required methods
    async fn write(&mut self, buf: &[u8]) -> io::Result<usize>;
    async fn flush(&mut self) -> io::Result<()>;

    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        let mut to_write = &buf[0..];
        loop {
            let bytes_written = self.write(to_write).await?;
            to_write = &to_write[bytes_written..];
            if to_write.is_empty() {
                return Ok(());
            }
        }
    }

    // If the `AsyncWrite` implementation is an unbuffered wrapper around an
    // `AsyncOutputStream`, some I/O operations can be more efficient.
    #[inline]
    fn as_async_output_stream(&self) -> Option<&io::AsyncOutputStream> {
        None
    }
}

impl<W: AsyncWrite + ?Sized> AsyncWrite for &mut W {
    #[inline]
    async fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (**self).write(buf).await
    }

    #[inline]
    async fn flush(&mut self) -> io::Result<()> {
        (**self).flush().await
    }

    #[inline]
    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        (**self).write_all(buf).await
    }

    #[inline]
    fn as_async_output_stream(&self) -> Option<&io::AsyncOutputStream> {
        (**self).as_async_output_stream()
    }
}
