use crate::io;

const CHUNK_SIZE: usize = 2048;

/// Read bytes from a source.
pub trait AsyncRead {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    async fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        // total bytes written to buf
        let mut n = 0;

        loop {
            // grow buf if empty
            if buf.len() == n {
                buf.resize(n + CHUNK_SIZE, 0u8);
            }

            let len = self.read(&mut buf[n..]).await?;
            if len == 0 {
                buf.truncate(n);
                return Ok(n);
            }

            n += len;
        }
    }

    // If the `AsyncRead` implementation is an unbuffered wrapper around an
    // `AsyncInputStream`, some I/O operations can be more efficient.
    #[inline]
    fn as_async_input_stream(&self) -> Option<&io::AsyncInputStream> {
        None
    }
}

impl<R: AsyncRead + ?Sized> AsyncRead for &mut R {
    #[inline]
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read(buf).await
    }

    #[inline]
    async fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        (**self).read_to_end(buf).await
    }

    #[inline]
    fn as_async_input_stream(&self) -> Option<&io::AsyncInputStream> {
        (**self).as_async_input_stream()
    }
}
