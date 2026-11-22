//! Socket helpers: privileged-port allocation and reusable UDP binding.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::ops::RangeInclusive;

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::{TcpListener, UdpSocket};

pub const UDP_PORT: u16 = 1716;
pub const TCP_PORT_MIN: u16 = 1716;
pub const TCP_PORT_MAX: u16 = 1764;
/// Upstream `LanLinkProvider.PAYLOAD_TRANSFER_MIN_PORT`.
pub const PAYLOAD_PORT_MIN: u16 = 1739;

/// Find the first free TCP port in `range` and return the bound listener.
///
/// Mirrors upstream `openServerSocketOnFreePort`: try each candidate in order
/// and keep the first one that binds.
pub fn find_free_tcp_port(range: RangeInclusive<u16>) -> io::Result<(StdTcpListener, u16)> {
    let mut last_error = None;
    for port in range.clone() {
        match StdTcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))) {
            Ok(listener) => return Ok((listener, port)),
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("no free TCP port in {range:?}"),
        )
    }))
}

/// Tokio variant of [`find_free_tcp_port`].
pub async fn bind_tcp_listener(range: RangeInclusive<u16>) -> io::Result<(TcpListener, u16)> {
    let (listener, port) = find_free_tcp_port(range)?;
    listener.set_nonblocking(true)?;
    Ok((TcpListener::from_std(listener)?, port))
}

/// Bind a UDP socket with `SO_REUSEADDR` and `SO_BROADCAST`, like upstream's
/// `DatagramSocket(null)` followed by `reuseAddress = true; broadcast = true`.
pub fn bind_udp_reuse(addr: SocketAddr) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.set_broadcast(true)?;
    socket.bind(&addr.into())?;
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(socket.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_port_allocation_starts_at_1739_and_stays_in_range() {
        let (listener, port) = find_free_tcp_port(PAYLOAD_PORT_MIN..=TCP_PORT_MAX).unwrap();
        assert!((PAYLOAD_PORT_MIN..=TCP_PORT_MAX).contains(&port));
        assert_eq!(listener.local_addr().unwrap().port(), port);
    }

    #[test]
    fn payload_port_allocation_skips_an_occupied_port() {
        let (first, port) = find_free_tcp_port(PAYLOAD_PORT_MIN..=TCP_PORT_MAX).unwrap();
        // The next search must never hand out the socket we are still holding.
        let (second, next) = find_free_tcp_port(PAYLOAD_PORT_MIN..=TCP_PORT_MAX).unwrap();
        assert_ne!(port, next);
        drop(first);
        drop(second);
    }

    #[test]
    fn main_port_search_stays_within_the_kdeconnect_range() {
        let (listener, port) = find_free_tcp_port(TCP_PORT_MIN..=TCP_PORT_MAX).unwrap();
        assert!((TCP_PORT_MIN..=TCP_PORT_MAX).contains(&port));
        drop(listener);
    }
}
