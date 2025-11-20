use libc;
use log::debug;
use pnet::packet::icmp::echo_reply::EchoReplyPacket;
use pnet::packet::icmp::echo_request::{IcmpCodes, MutableEchoRequestPacket};
use pnet::packet::icmp::{IcmpPacket, IcmpTypes};
use pnet::packet::{util, MutablePacket, Packet};
use socket2::{Domain, Protocol, Socket, Type};
use std::os::unix::io::{AsRawFd, FromRawFd};
use tokio::net::UdpSocket;

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Instant;

/// Record requests for a single IP address.
struct IpRequests {
    seq: u16,
    requests: HashMap<u16, Instant>,
}

impl IpRequests {
    fn new() -> Self {
        IpRequests {
            seq: 1,
            requests: HashMap::new(),
        }
    }
    fn send_request(&mut self) {
        self.requests.insert(self.seq, Instant::now());
        self.seq += 1;
    }
    fn recv_response(&mut self, seq: u16) -> Option<Instant> {
        self.requests.remove(&seq)
    }
    fn seq(&self) -> u16 {
        self.seq
    }
}

/// A struct to detect RTT (Round Trip Time) using ICMP Echo Requests.
pub struct RttDetector {
    socket: socket2::Socket,
    tokio_socket: UdpSocket, // for async operations, point to the same socket as `socket`
    id: u16,                 // ICMP identifier
    send_buf: Vec<u8>,
    recv_buf: Vec<u8>,
    sent_requests: HashMap<IpAddr, IpRequests>,
    // TODO: del long time unreplied requests

    // For info print with ts (only used in debug, not needed in production)
    begin_time: Instant,
}

impl RttDetector {
    pub fn new() -> Self {
        // create the socket for ICMPv4
        let socket = Self::init_icmp_socket();
        let tokio_socket = unsafe {
            // Duplicate the underlying raw fd so that the original `socket`
            // and the `std::net::UdpSocket`/Tokio wrapper each own separate
            // file descriptors. This prevents a double-close when both are
            // dropped (which causes an IO safety abort on recent Rust).
            let fd = socket.as_raw_fd();
            let fd_dup = libc::dup(fd);
            if fd_dup == -1 {
                panic!(
                    "failed to duplicate fd: {}",
                    std::io::Error::last_os_error()
                );
            }
            let std_socket = std::net::UdpSocket::from_raw_fd(fd_dup);
            UdpSocket::from_std(std_socket)
                .expect("Failed to convert std socket to Tokio UdpSocket")
        };
        RttDetector {
            socket,
            tokio_socket,
            id: 2025,
            send_buf: vec![0; 64], // Use 64 bytes for ICMP echo request
            recv_buf: vec![0; 64], // Use 64 bytes for ICMP echo reply
            sent_requests: HashMap::new(),
            begin_time: Instant::now(),
        }
    }

    // (For debug)
    pub fn fresh_begin_time(&mut self, now: Instant) {
        self.begin_time = now;
    }

    // (For debug) Return elapsed time since the connection is created.
    fn elapsed(&self, now: Instant) -> std::time::Duration {
        now.duration_since(self.begin_time)
    }

    /// Create a new ICMPv4 socket and bind it to an unspecified address.
    /// The socket is transformed into a Tokio UdpSocket for asynchronous operations.
    fn init_icmp_socket() -> socket2::Socket {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::ICMPV4))
            .expect("Failed to create ICMPv4 socket");
        let src = std::net::SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);
        socket
            .bind(&src.into())
            .expect("Failed to bind ICMPv4 socket");
        socket
            .set_ttl(64)
            .expect("Failed to set TTL for ICMPv4 socket");
        socket
            .set_nonblocking(true)
            .expect("Failed to set ICMPv4 socket to non-blocking mode");
        socket
    }

    // TODO: count the overhead of the ICMP recv and recv(only count one for every RTT)
    /// Send an ICMP request to the specified destination address.
    /// In PEMI implementation, every send_to call must be successful(We now haven't process the resend logic for write failure).
    pub fn send_request(&mut self, dest: std::net::SocketAddr) {
        let ip_requests = match self.sent_requests.get_mut(&dest.ip()) {
            Some(requests) => requests,
            None => {
                let new_requests = IpRequests::new();
                self.sent_requests.insert(dest.ip(), new_requests);
                self.sent_requests.get_mut(&dest.ip()).unwrap()
            }
        };

        let mut icmp = MutableEchoRequestPacket::new(&mut self.send_buf[..])
            .expect("Failed to create ICMP Echo Request packet");
        icmp.set_icmp_type(IcmpTypes::EchoRequest);
        icmp.set_icmp_code(IcmpCodes::NoCode);
        icmp.set_identifier(self.id);
        icmp.set_sequence_number(ip_requests.seq());
        icmp.set_checksum(util::checksum(icmp.packet(), 1));

        ip_requests.send_request();
        self.socket
            .send_to(icmp.packet_mut(), &dest.into())
            .expect("Failed to send ICMP request"); // if write fails, panic with the error message
    }

    pub async fn wait_readable(&self) -> Result<(), std::io::Error> {
        // Wait for the socket to become readable
        self.tokio_socket.readable().await?;
        Ok(())
    }

    /// Call after the socket becomes ready to read
    pub fn recv_response(&mut self) -> Result<std::time::Duration, std::io::Error> {
        // Here you would implement the logic to receive the ICMP response
        // and calculate the RTT based on the sent request time.
        // handle recv
        let mut mem_buf = unsafe {
            &mut *(self.recv_buf.as_mut_slice() as *mut [u8] as *mut [std::mem::MaybeUninit<u8>])
        };
        let (size, server_addr) = self.socket.recv_from(&mut mem_buf)?;
        let server_addr = server_addr.as_socket().unwrap();

        let reply_packet = IcmpPacket::new(&self.recv_buf[..size]).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Failed to parse ICMP packet",
            )
        })?;
        match reply_packet.get_icmp_type() {
            IcmpTypes::EchoReply => {
                let reply = EchoReplyPacket::new(&self.recv_buf).ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Failed to parse ICMP reply",
                    )
                })?;
                let seq: u16 = reply.get_sequence_number();
                let ip_requests = self
                    .sent_requests
                    .get_mut(&server_addr.ip())
                    .unwrap_or_else(|| {
                        panic!(
                            "No requests found for addr: {:?}:{:?}",
                            server_addr.ip(),
                            server_addr.port()
                        )
                    });
                let now = Instant::now();
                let duration = now
                    .duration_since(ip_requests.recv_response(seq).expect("No response for seq"));
                let duration_ms = duration.as_micros() as f64 / 1000.0; // Convert to milliseconds

                debug!(
                    "{}B from {} {:?} icmp_seq={} id={} ttl={} time={:.2}ms",
                    size,
                    server_addr.ip(),
                    self.elapsed(now),
                    seq,
                    reply.get_identifier(),
                    self.socket.ttl()?,
                    duration_ms
                );
                Ok(duration)
            }
            other_type => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Received ICMP packet of type: {:?}", other_type),
                ));
            }
        }
    }
}
