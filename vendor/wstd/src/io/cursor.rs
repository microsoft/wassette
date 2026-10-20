use crate::io::{self, AsyncRead, AsyncSeek, AsyncWrite};

use super::SeekFrom;

/// A `Cursor` wraps an in-memory buffer and provides it with a
/// [`AsyncSeek`] implementation.
#[derive(Clone, Debug, Default)]
pub struct Cursor<T> {
    inner: std::io::Cursor<T>,
}

impl<T> Cursor<T> {
    /// Creates a new cursor wrapping the provided underlying in-memory buffer.
    pub fn new(inner: T) -> Cursor<T> {
        Cursor {
            inner: std::io::Cursor::new(inner),
        }
    }

    /// Consumes this cursor, returning the underlying value.
    pub fn into_inner(self) -> T {
        self.inner.into_inner()
    }

    /// Gets a reference to the underlying value in this cursor.
    pub fn get_ref(&self) -> &T {
        self.inner.get_ref()
    }

    /// Gets a mutable reference to the underlying value in this cursor.
    pub fn get_mut(&mut self) -> &mut T {
        self.inner.get_mut()
    }

    /// Returns the current position of this cursor.
    pub fn position(&self) -> u64 {
        self.inner.position()
    }

    /// Sets the position of this cursor.
    pub fn set_position(&mut self, pos: u64) {
        self.inner.set_position(pos)
    }
}

impl<T> AsyncSeek for Cursor<T>
where
    T: AsRef<[u8]>,
{
    async fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let pos = match pos {
            SeekFrom::Start(pos) => std::io::SeekFrom::Start(pos),
            SeekFrom::End(pos) => std::io::SeekFrom::End(pos),
            SeekFrom::Current(pos) => std::io::SeekFrom::Current(pos),
        };
        std::io::Seek::seek(&mut self.inner, pos)
    }
}

impl<T> AsyncRead for Cursor<T>
where
    T: AsRef<[u8]>,
{
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        std::io::Read::read(&mut self.inner, buf)
    }
}

impl AsyncWrite for Cursor<&mut [u8]> {
    async fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        std::io::Write::write(&mut self.inner, buf)
    }
    async fn flush(&mut self) -> io::Result<()> {
        std::io::Write::flush(&mut self.inner)
    }
}

impl AsyncWrite for Cursor<&mut Vec<u8>> {
    async fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        std::io::Write::write(&mut self.inner, buf)
    }
    async fn flush(&mut self) -> io::Result<()> {
        std::io::Write::flush(&mut self.inner)
    }
}

impl AsyncWrite for Cursor<Vec<u8>> {
    async fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        std::io::Write::write(&mut self.inner, buf)
    }
    async fn flush(&mut self) -> io::Result<()> {
        std::io::Write::flush(&mut self.inner)
    }
}
