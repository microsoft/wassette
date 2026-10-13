use crate::io;
use crate::iter::AsyncIterator;
use std::cell::RefCell;
use std::net::SocketAddr;

use super::{TcpStream, to_io_err};
use wasip3::sockets::types::{IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket};
use wasip3::wit_bindgen::rt::async_support::StreamReader;

/// A TCP socket server, listening for connections.
///
/// The p3 accept stream requires `&mut` access to make progress, so it is
/// wrapped in a `RefCell`. This lets `TcpListener` mirror the p2 API surface,
/// where `incoming` is available through a shared `&TcpListener` reference.
pub struct TcpListener {
    accept_stream: RefCell<StreamReader<TcpSocket>>,
    socket: TcpSocket,
}

impl std::fmt::Debug for TcpListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpListener").finish()
    }
}

impl TcpListener {
    /// Creates a new TcpListener which will be bound to the specified address.
    ///
    /// The returned listener is ready for accepting connections.
    pub async fn bind(addr: &str) -> io::Result<Self> {
        let addr: SocketAddr = addr
            .parse()
            .map_err(|_| io::Error::other("failed to parse string to socket addr"))?;
        let family = match addr {
            SocketAddr::V4(_) => IpAddressFamily::Ipv4,
            SocketAddr::V6(_) => IpAddressFamily::Ipv6,
        };
        let socket = TcpSocket::create(family).map_err(to_io_err)?;
        let local_address = sockaddr_to_wasi(addr);
        socket.bind(local_address).map_err(to_io_err)?;
        let accept_stream = socket.listen().map_err(to_io_err)?;
        Ok(Self {
            accept_stream: RefCell::new(accept_stream),
            socket,
        })
    }

    /// Returns the local socket address of this listener.
    pub fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        self.socket
            .get_local_address()
            .map_err(to_io_err)
            .map(sockaddr_from_wasi)
    }

    /// Returns an iterator over the connections being received on this listener.
    pub fn incoming(&self) -> Incoming<'_> {
        Incoming { listener: self }
    }
}

/// An iterator that infinitely accepts connections on a TcpListener.
pub struct Incoming<'a> {
    listener: &'a TcpListener,
}

impl<'a> std::fmt::Debug for Incoming<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Incoming").finish()
    }
}

// The accept stream is consumed by a single `incoming` iterator at a time, so
// holding the `RefCell` borrow across `.await` here is sound.
#[allow(clippy::await_holding_refcell_ref)]
impl<'a> AsyncIterator for Incoming<'a> {
    type Item = io::Result<TcpStream>;

    async fn next(&mut self) -> Option<Self::Item> {
        self.listener
            .accept_stream
            .borrow_mut()
            .next()
            .await
            .map(TcpStream::from_connected_socket)
    }
}

pub(crate) fn sockaddr_from_wasi(addr: IpSocketAddress) -> std::net::SocketAddr {
    use wasip3::sockets::types::Ipv6SocketAddress;
    match addr {
        IpSocketAddress::Ipv4(Ipv4SocketAddress { address, port }) => {
            std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
                std::net::Ipv4Addr::new(address.0, address.1, address.2, address.3),
                port,
            ))
        }
        IpSocketAddress::Ipv6(Ipv6SocketAddress {
            address,
            port,
            flow_info,
            scope_id,
        }) => std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
            std::net::Ipv6Addr::new(
                address.0, address.1, address.2, address.3, address.4, address.5, address.6,
                address.7,
            ),
            port,
            flow_info,
            scope_id,
        )),
    }
}

fn sockaddr_to_wasi(addr: std::net::SocketAddr) -> IpSocketAddress {
    use wasip3::sockets::types::Ipv6SocketAddress;
    match addr {
        std::net::SocketAddr::V4(addr) => {
            let ip = addr.ip().octets();
            IpSocketAddress::Ipv4(Ipv4SocketAddress {
                address: (ip[0], ip[1], ip[2], ip[3]),
                port: addr.port(),
            })
        }
        std::net::SocketAddr::V6(addr) => {
            let ip = addr.ip().segments();
            IpSocketAddress::Ipv6(Ipv6SocketAddress {
                address: (ip[0], ip[1], ip[2], ip[3], ip[4], ip[5], ip[6], ip[7]),
                port: addr.port(),
                flow_info: addr.flowinfo(),
                scope_id: addr.scope_id(),
            })
        }
    }
}
