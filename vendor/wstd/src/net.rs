//! Async network abstractions.
//!
//! The types here are the crate's public network surface. They are thin
//! facades that delegate to the selected backend's implementation under
//! [`crate::sys::net`]; the portable address-resolution loop in
//! [`TcpStream::connect`] is the one piece written once here rather than per
//! backend. A second backend only has to supply the `sys::net` primitives.

use crate::io::{self, AsyncInputStream, AsyncOutputStream, AsyncRead, AsyncWrite};
use crate::iter::AsyncIterator;
use std::io::ErrorKind;
use std::net::{SocketAddr, ToSocketAddrs};

/// A TCP stream between a local and a remote socket.
pub struct TcpStream {
    inner: crate::sys::net::TcpStream,
}

impl TcpStream {
    /// Opens a TCP connection to a remote host.
    ///
    /// `addr` is an address of the remote host. Anything which implements the
    /// [`ToSocketAddrs`] trait can be supplied as the address.  If `addr`
    /// yields multiple addresses, connect will be attempted with each of the
    /// addresses until a connection is successful. If none of the addresses
    /// result in a successful connection, the error returned from the last
    /// connection attempt (the last address) is returned.
    pub async fn connect(addr: impl ToSocketAddrs) -> io::Result<Self> {
        let addrs = addr.to_socket_addrs()?;
        let mut last_err = None;
        for addr in addrs {
            match TcpStream::connect_addr(addr).await {
                Ok(stream) => return Ok(stream),
                Err(e) => last_err = Some(e),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            io::Error::new(ErrorKind::InvalidInput, "could not resolve to any address")
        }))
    }

    /// Establishes a connection to the specified `addr`.
    pub async fn connect_addr(addr: SocketAddr) -> io::Result<Self> {
        Ok(Self {
            inner: crate::sys::net::TcpStream::connect_addr(addr).await?,
        })
    }

    /// Returns the socket address of the remote peer of this TCP connection.
    pub fn peer_addr(&self) -> io::Result<String> {
        self.inner.peer_addr()
    }

    /// Splits this stream into a read half and a write half, which can be used
    /// to read and write concurrently.
    pub fn split(&self) -> (ReadHalf<'_>, WriteHalf<'_>) {
        let (read, write) = self.inner.split();
        (ReadHalf(read), WriteHalf(write))
    }
}

impl AsyncRead for TcpStream {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf).await
    }

    fn as_async_input_stream(&self) -> Option<&AsyncInputStream> {
        self.inner.as_async_input_stream()
    }
}

impl AsyncRead for &TcpStream {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut inner = &self.inner;
        inner.read(buf).await
    }

    fn as_async_input_stream(&self) -> Option<&AsyncInputStream> {
        self.inner.as_async_input_stream()
    }
}

impl AsyncWrite for TcpStream {
    async fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf).await
    }

    async fn flush(&mut self) -> io::Result<()> {
        self.inner.flush().await
    }

    fn as_async_output_stream(&self) -> Option<&AsyncOutputStream> {
        self.inner.as_async_output_stream()
    }
}

impl AsyncWrite for &TcpStream {
    async fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut inner = &self.inner;
        inner.write(buf).await
    }

    async fn flush(&mut self) -> io::Result<()> {
        let mut inner = &self.inner;
        inner.flush().await
    }

    fn as_async_output_stream(&self) -> Option<&AsyncOutputStream> {
        self.inner.as_async_output_stream()
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        // The socket shutdown runs when the inner backend handle is dropped.
    }
}

/// The read half of a [`TcpStream`], created by [`TcpStream::split`].
pub struct ReadHalf<'a>(crate::sys::net::ReadHalf<'a>);

impl<'a> AsyncRead for ReadHalf<'a> {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf).await
    }

    fn as_async_input_stream(&self) -> Option<&AsyncInputStream> {
        self.0.as_async_input_stream()
    }
}

impl<'a> Drop for ReadHalf<'a> {
    fn drop(&mut self) {
        // The receive-side shutdown runs when the inner backend handle is dropped.
    }
}

/// The write half of a [`TcpStream`], created by [`TcpStream::split`].
pub struct WriteHalf<'a>(crate::sys::net::WriteHalf<'a>);

impl<'a> AsyncWrite for WriteHalf<'a> {
    async fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf).await
    }

    async fn flush(&mut self) -> io::Result<()> {
        self.0.flush().await
    }

    fn as_async_output_stream(&self) -> Option<&AsyncOutputStream> {
        self.0.as_async_output_stream()
    }
}

impl<'a> Drop for WriteHalf<'a> {
    fn drop(&mut self) {
        // The send-side shutdown runs when the inner backend handle is dropped.
    }
}

/// A TCP socket server, listening for connections.
#[derive(Debug)]
pub struct TcpListener {
    inner: crate::sys::net::TcpListener,
}

impl TcpListener {
    /// Creates a new TcpListener which will be bound to the specified address.
    ///
    /// The returned listener is ready for accepting connections.
    pub async fn bind(addr: &str) -> io::Result<Self> {
        Ok(Self {
            inner: crate::sys::net::TcpListener::bind(addr).await?,
        })
    }

    /// Returns the local socket address of this listener.
    pub fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        self.inner.local_addr()
    }

    /// Returns an iterator over the connections being received on this listener.
    pub fn incoming(&self) -> Incoming<'_> {
        Incoming {
            inner: self.inner.incoming(),
        }
    }
}

/// An iterator that infinitely accepts connections on a [`TcpListener`].
#[derive(Debug)]
pub struct Incoming<'a> {
    inner: crate::sys::net::Incoming<'a>,
}

impl<'a> AsyncIterator for Incoming<'a> {
    type Item = io::Result<TcpStream>;

    async fn next(&mut self) -> Option<Self::Item> {
        self.inner
            .next()
            .await
            .map(|result| result.map(|inner| TcpStream { inner }))
    }
}
