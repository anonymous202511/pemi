use nix::sys::socket::sockopt::IpTransparent;
use nix::sys::socket::{bind, sendto, socket, AddressFamily, SockFlag, SockType};
use nix::sys::socket::{setsockopt, MsgFlags, SockaddrIn};
use std::net::SocketAddr;
use std::net::SocketAddrV4;
use std::os::fd::AsRawFd;

#[cfg(any(feature = "cycles"))]
use crate::count_cycles;
#[cfg(any(feature = "cycles"))]
use std::arch::x86_64::_rdtsc;

use nix::libc::sockaddr_in;

use log::trace;

/// transparently send the payload to the destination address
// TODO: avoid creating a new socket every time -- create a global socket pool by thread_local and RefCell. This not important now but may improve performance.
pub fn send_transparently(srcaddr: &SockaddrIn, dstaddr: &SockaddrIn, buf: &[u8]) {
    #[cfg(any(feature = "cycles"))]
    let start_2 = unsafe { _rdtsc() };
    let fd_send = socket(
        AddressFamily::Inet, // now only support IPv4
        SockType::Datagram,
        SockFlag::empty(),
        None,
    )
    .expect("error creating socket");

    setsockopt(&fd_send, IpTransparent, &true).expect("error setting transparency");

    // bind to source address
    bind(fd_send.as_raw_fd(), srcaddr).expect("error binding to source address");

    // send the payload to the destination address
    let ret = sendto(fd_send.as_raw_fd(), buf, dstaddr, MsgFlags::empty())
        .expect("error sending to destination");
    trace!("sent {} bytes to dst", ret);
    #[cfg(any(feature = "cycles"))]
    count_cycles(2, start_2);
}

fn to_std_addr(addr: &SockaddrIn) -> SocketAddr {
    let ip = addr.ip().into();
    let port = addr.port();
    SocketAddr::new(ip, port)
}

pub fn to_nix_addr(addr: &SocketAddr) -> SockaddrIn {
    let ip = addr.ip();
    let port = addr.port();
    // use std::net::Ipv4Addr as middle type
    let addr: SocketAddrV4 = match ip {
        std::net::IpAddr::V4(ip) => SocketAddrV4::new(ip, port),
        _ => panic!("only support IPv4 now"),
    };
    SockaddrIn::from(addr)
}

pub fn print_addr(addr: &sockaddr_in) -> String {
    let ip = std::net::Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr));
    let port = u16::from_be(addr.sin_port);
    format!("{}:{}", ip, port)
}

pub struct Addr {
    pub std_addr: SocketAddr,
    pub nix_addr: SockaddrIn,
}

impl Addr {
    pub fn from_std_addr(addr: SocketAddr) -> Self {
        let nix_addr = to_nix_addr(&addr);
        Addr {
            std_addr: addr,
            nix_addr,
        }
    }

    pub fn from_nix_addr(addr: SockaddrIn) -> Self {
        let std_addr = to_std_addr(&addr);
        Addr {
            std_addr,
            nix_addr: addr,
        }
    }
}
