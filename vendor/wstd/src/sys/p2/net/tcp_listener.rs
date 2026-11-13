use wasip2::sockets::network::Ipv4SocketAddress;
use wasip2::sockets::tcp::{IpAddressFamily, IpSocketAddress, TcpSocket};

use crate::io;
use crate::iter::AsyncIterator;
use std::net::SocketAddr;

use super::{TcpStream, to_io_err};
use crate::runtime::AsyncPollable;

/// A TCP socket server, listening for connections.
#[derive(Debug)]
pub struct TcpListener {
    // Field order matters: must drop this child before parent below
    pollable: AsyncPollable,
    socket: TcpSocket,
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
        let socket =
            wasip2::sockets::tcp_create_socket::create_tcp_socket(family).map_err(to_io_err)?;
        let network = wasip2::sockets::instance_network::instance_network();

        let local_address = sockaddr_to_wasi(addr);

        socket
            .start_bind(&network, local_address)
            .map_err(to_io_err)?;
        let pollable = AsyncPollable::new(socket.subscribe());
        pollable.wait_for().await;
        socket.finish_bind().map_err(to_io_err)?;

        socket.start_listen().map_err(to_io_err)?;
        pollable.wait_for().await;
        socket.finish_listen().map_err(to_io_err)?;
        Ok(Self { pollable, socket })
    }

    /// Returns the local socket address of this listener.
    pub fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        self.socket
            .local_address()
            .map_err(to_io_err)
            .map(sockaddr_from_wasi)
    }

    /// Returns an iterator over the connections being received on this listener.
    pub fn incoming(&self) -> Incoming<'_> {
        Incoming { listener: self }
    }
}

/// An iterator that infinitely accepts connections on a TcpListener.
#[derive(Debug)]
pub struct Incoming<'a> {
    listener: &'a TcpListener,
}

impl<'a> AsyncIterator for Incoming<'a> {
    type Item = io::Result<TcpStream>;

    async fn next(&mut self) -> Option<Self::Item> {
        self.listener.pollable.wait_for().await;
        let (socket, input, output) = match self.listener.socket.accept().map_err(to_io_err) {
            Ok(accepted) => accepted,
            Err(err) => return Some(Err(err)),
        };
        Some(Ok(TcpStream::new(input, output, socket)))
    }
}

fn sockaddr_from_wasi(addr: IpSocketAddress) -> std::net::SocketAddr {
    use wasip2::sockets::network::Ipv6SocketAddress;
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
    use wasip2::sockets::network::Ipv6SocketAddress;
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
