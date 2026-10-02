use std::cell::RefCell;
use std::io::ErrorKind;
use std::net::{SocketAddr, ToSocketAddrs};

use super::tcp_listener::sockaddr_from_wasi;
use super::to_io_err;
use crate::io::{self, AsyncInputStream, AsyncOutputStream};
use wasip3::sockets::types::{IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket};

/// A TCP stream between a local and a remote socket.
///
/// The underlying p3 stream reader and writer require `&mut` access, so they
/// are wrapped in `RefCell`s. This lets `TcpStream` mirror the p2 API surface,
/// where reading and writing are available through shared `&TcpStream`
/// references (e.g. `io::copy(&stream, &stream)`).
pub struct TcpStream {
    input: RefCell<AsyncInputStream>,
    output: RefCell<AsyncOutputStream>,
    socket: TcpSocket,
}

impl std::fmt::Debug for TcpStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpStream").finish()
    }
}

impl TcpStream {
    pub(crate) fn new(
        input: AsyncInputStream,
        output: AsyncOutputStream,
        socket: TcpSocket,
    ) -> Self {
        TcpStream {
            input: RefCell::new(input),
            output: RefCell::new(output),
            socket,
        }
    }

    /// Opens a TCP connection to a remote host.
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
        let family = match addr {
            SocketAddr::V4(_) => IpAddressFamily::Ipv4,
            SocketAddr::V6(_) => IpAddressFamily::Ipv6,
        };
        let socket = TcpSocket::create(family).map_err(to_io_err)?;

        let remote_address = match addr {
            SocketAddr::V4(addr) => {
                let ip = addr.ip().octets();
                let address = (ip[0], ip[1], ip[2], ip[3]);
                let port = addr.port();
                IpSocketAddress::Ipv4(Ipv4SocketAddress { port, address })
            }
            SocketAddr::V6(_) => todo!("IPv6 not yet supported in `wstd::net::TcpStream`"),
        };

        // p3 connect is async
        socket.connect(remote_address).await.map_err(to_io_err)?;

        Self::from_connected_socket(socket)
    }

    /// Create a TcpStream from an already-connected socket.
    pub(crate) fn from_connected_socket(socket: TcpSocket) -> io::Result<Self> {
        // Get receive stream
        let (recv_reader, _recv_completion) = socket.receive();
        let input = AsyncInputStream::new(recv_reader);

        // Create a send stream and wire it to the socket
        let (send_writer, send_reader) = wasip3::wit_stream::new::<u8>();
        let _send_completion = socket.send(send_reader);
        let output = AsyncOutputStream::new(send_writer);

        Ok(TcpStream::new(input, output, socket))
    }

    /// Returns the socket address of the remote peer of this TCP connection.
    pub fn peer_addr(&self) -> io::Result<String> {
        self.socket
            .get_remote_address()
            .map_err(to_io_err)
            .map(sockaddr_from_wasi)
            .map(|addr| addr.to_string())
    }

    pub fn split(&self) -> (ReadHalf<'_>, WriteHalf<'_>) {
        (ReadHalf(self), WriteHalf(self))
    }
}

impl io::AsyncRead for TcpStream {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.input.get_mut().read(buf).await
    }
}

// The `RefCell` borrows below are held across `.await`. This is sound for the
// supported usage (a single reader and a single writer operating on disjoint
// cells, e.g. `io::copy(&stream, &stream)`): the input and output cells are
// never borrowed concurrently. Concurrently issuing two reads (or two writes)
// on the same stream would panic, which mirrors the exclusive-access nature of
// the underlying p3 stream resources.
#[allow(clippy::await_holding_refcell_ref)]
impl io::AsyncRead for &TcpStream {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.input.borrow_mut().read(buf).await
    }
}

impl io::AsyncWrite for TcpStream {
    async fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.get_mut().write(buf).await
    }

    async fn flush(&mut self) -> io::Result<()> {
        self.output.get_mut().flush().await
    }
}

#[allow(clippy::await_holding_refcell_ref)]
impl io::AsyncWrite for &TcpStream {
    async fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.borrow_mut().write(buf).await
    }

    async fn flush(&mut self) -> io::Result<()> {
        self.output.borrow_mut().flush().await
    }
}

pub struct ReadHalf<'a>(&'a TcpStream);
#[allow(clippy::await_holding_refcell_ref)]
impl<'a> io::AsyncRead for ReadHalf<'a> {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.input.borrow_mut().read(buf).await
    }
}

pub struct WriteHalf<'a>(&'a TcpStream);
#[allow(clippy::await_holding_refcell_ref)]
impl<'a> io::AsyncWrite for WriteHalf<'a> {
    async fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.output.borrow_mut().write(buf).await
    }

    async fn flush(&mut self) -> io::Result<()> {
        self.0.output.borrow_mut().flush().await
    }
}
