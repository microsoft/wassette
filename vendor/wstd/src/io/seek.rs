/// The `Seek` trait provides a cursor which can be moved within a stream of
/// bytes.
pub trait AsyncSeek {
    /// Seek to an offset, in bytes, in a stream.
    async fn seek(&mut self, pos: SeekFrom) -> super::Result<u64>;

    /// Rewind to the beginning of a stream.
    async fn rewind(&mut self) -> super::Result<()> {
        self.seek(SeekFrom::Start(0)).await?;
        Ok(())
    }

    /// Returns the length of this stream (in bytes).
    async fn stream_len(&mut self) -> super::Result<u64> {
        let old_pos = self.stream_position().await?;
        let len = self.seek(SeekFrom::End(0)).await?;

        // Avoid seeking a third time when we were already at the end of the
        // stream. The branch is usually way cheaper than a seek operation.
        if old_pos != len {
            self.seek(SeekFrom::Start(old_pos)).await?;
        }

        Ok(len)
    }

    /// Returns the current seek position from the start of the stream.
    async fn stream_position(&mut self) -> super::Result<u64> {
        self.seek(SeekFrom::Current(0)).await
    }

    /// Seeks relative to the current position.
    async fn seek_relative(&mut self, offset: i64) -> super::Result<()> {
        self.seek(SeekFrom::Current(offset)).await?;
        Ok(())
    }
}

/// Enumeration of possible methods to seek within an I/O object.
///
/// It is used by the [`AsyncSeek`] trait.
#[derive(Copy, PartialEq, Eq, Clone, Debug)]
pub enum SeekFrom {
    /// Sets the offset to the provided number of bytes.
    Start(u64),

    /// Sets the offset to the size of this object plus the specified number of
    /// bytes.
    End(i64),

    /// Sets the offset to the current position plus the specified number of
    /// bytes.
    Current(i64),
}
