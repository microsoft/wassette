use crate::io::{AsyncRead, AsyncWrite};

/// Copy bytes from a reader to a writer.
pub async fn copy<R, W>(mut reader: R, mut writer: W) -> crate::io::Result<()>
where
    R: AsyncRead,
    W: AsyncWrite,
{
    // Optimized path when we have an `AsyncInputStream` and an
    // `AsyncOutputStream` (p2 only — p2 can use wasi splice).
    #[cfg(wstd_p2)]
    if let Some(reader) = reader.as_async_input_stream()
        && let Some(writer) = writer.as_async_output_stream()
    {
        reader.copy_to(writer).await?;
        return Ok(());
    }

    // Unoptimized case: read the input and then write it.
    let mut buf = [0; 1024];
    'read: loop {
        let bytes_read = reader.read(&mut buf).await?;
        if bytes_read == 0 {
            break 'read Ok(());
        }
        writer.write_all(&buf[0..bytes_read]).await?;
    }
}
