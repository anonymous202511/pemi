use pemi::pemi_io;
use pemi::retrans;
use pemi::PEMI;
use pemi::RETRANS_HELP;

#[cfg(any(feature = "cycles"))]
use {
    pemi::{count_cycles, print_cycles_count_summary},
    std::arch::x86_64::_rdtsc,
};

use std::io::IoSliceMut;
use std::io::Write;
use std::net::UdpSocket;
use std::os::fd::AsRawFd;
use std::time;

use nix::sys::socket::sockopt::{IpTransparent, Ipv4OrigDstAddr, ReuseAddr};
use nix::sys::socket::{recvmsg, setsockopt, MsgFlags, RecvMsg, SockaddrIn};

use log::{debug, trace};

use clap::Parser;

const MAX_RECV_BUF: usize = 1500;

#[derive(Parser)]
struct Args {
    /// port number, default is 5000
    #[clap(short, long, default_value = "5000")]
    port: u16,

    /// FLOWLET_INTERVAL_FACTOR. The factor of the flowlet timeout(to decide whether to create new flowlet.
    #[clap(short, long, default_value = "2.0")]
    fl_inv_factor: f64,

    /// FLOWLET_END_FACTOR. The factor of the flowlet end timeout.
    #[clap(short, long, default_value = "0.5")]
    fl_end_factor: f64,

    /// Frequency to print the stats.(every N packets)
    #[clap(short, long, default_value = "1000")]
    print_interval: u64,

    /// Is set as True, only transparent forwarding. (not enable PEMI)
    #[clap(short, long)]
    proxy_only: bool,
}

#[tokio::main]
async fn main() -> Result<(), String> {
    env_logger::builder()
        .format(|buf, record| writeln!(buf, "[{}] {}", record.level(), record.args()))
        .init();
    let args: Args = Args::parse();

    let port = args.port;

    let socket = UdpSocket::bind(format!("0.0.0.0:{}", port))
        .map_err(|e| format!("error creating listening socket: {}", e))?;

    // set socket options: SO_REUSEADDR, IP_TRANSPARENT, IP_RECVORIGDSTADDR
    setsockopt(&socket, ReuseAddr, &true)
        .map_err(|e| format!("error setting SO_REUSEADDR: {}", e))?;
    setsockopt(&socket, IpTransparent, &true)
        .map_err(|e| format!("error setting IP_TRANSPARENT: {}", e))?;
    setsockopt(&socket, Ipv4OrigDstAddr, &true)
        .map_err(|e| format!("error setting IP_RECVORIGDSTADDR: {}", e))?;

    // transfer to tokio socket
    socket
        .set_nonblocking(true)
        .map_err(|e| format!("error setting non-blocking mode: {}", e))?;
    let socket = tokio::net::UdpSocket::from_std(socket)
        .map_err(|e| format!("error converting to tokio socket: {}", e))?;

    println!("RETRANS_HELP: {}", RETRANS_HELP);
    println!("listening on port {}", port);

    // init PEMI
    let mut pemi = PEMI::new();
    pemi.set_factors(args.fl_inv_factor, args.fl_end_factor);
    pemi.set_proxy_only(args.proxy_only);

    let mut last_print_stats = 0;
    loop {
        #[cfg(any(feature = "cycles"))]
        let start_0 = unsafe { _rdtsc() };

        let mut buf = [0u8; MAX_RECV_BUF].to_vec();

        let timeout = pemi.timeout();

        if timeout == Some(time::Duration::ZERO) {
            // already timeout
            #[cfg(any(feature = "cycles"))]
            let start_3 = unsafe { _rdtsc() }; // count cycles of pemi computing
            pemi.process_timeout()?;
            while let Some(task) = pemi.pop_retrans_task() {
                debug!("timeout, retrans task: {}", task);
                process_retrans_task(task, &mut pemi)?;
            }
            #[cfg(any(feature = "cycles"))]
            count_cycles(3, start_3);
            #[cfg(any(feature = "cycles"))]
            count_cycles(0, start_0);
            continue; // process timeout and continue
        }

        tokio::select! {
            _ = tokio::time::sleep(timeout.unwrap_or(time::Duration::MAX)) => {
                // timeout: go the beginning of the loop, where we process timeout
                #[cfg(any(feature = "cycles"))]
                count_cycles(0, start_0);
                continue;
            }
            r = pemi.rtt_detector.wait_readable() => {
                if r.is_ok() {
                    match pemi.rtt_detector.recv_response() {
                            Ok(calibration_rtt_sample) => {
                                pemi.rtt_calibration(calibration_rtt_sample);
                                #[cfg(any(feature = "cycles"))]
                                count_cycles(0, start_0);
                                continue; // process rtt response and continue
                            }
                            Err(e) => {
                                if e.kind() == std::io::ErrorKind::WouldBlock {
                                    #[cfg(any(feature = "cycles"))]
                                    count_cycles(0, start_0);
                                    continue;
                                }
                                panic!("Error receiving ICMP packet: {:?}", e);
                        }
                    }
                }
                else {
                    panic!("Error checking read: {:?}", r);
                }
            }
            _ = socket.readable() => {

                #[cfg(any(feature = "cycles"))]
                let start_1 = unsafe { _rdtsc() }; // count cycles of recv io

                #[cfg(any(feature = "cycles"))]
                let start_4 = unsafe { _rdtsc() }; // count cycles of extra wait due to false positive readable

                // create iov
                let mut iov = [IoSliceMut::new(&mut buf)];
                let mut cmsgspace = nix::cmsg_space!([u8; 64]); // control message space

                // recv message
                let rmg: RecvMsg<'_, '_, SockaddrIn> = match recvmsg(
                    socket.as_raw_fd(),
                    &mut iov,
                    Some(&mut cmsgspace),
                    MsgFlags::empty(),
                ) {
                    Ok(rmg) => rmg,
                    Err(e) => {
                        if e == nix::errno::Errno::EAGAIN {
                            // readable not mean necessarily recv will succeed
                            #[cfg(any(feature = "cycles"))]
                            count_cycles(4, start_4); // count into extra wait
                            #[cfg(any(feature = "cycles"))]
                            count_cycles(0, start_0);
                            continue;
                        } else {
                            return Err(format!("error receiving message: {}", e));
                        }
                    }
                };

                let recv_ts = time::Instant::now();

                // get the original destination address
                let dstaddr = rmg
                    .cmsgs()
                    .map_err(|e| format!("error getting control messages: {}", e))?
                    .find_map(|cmsg| match cmsg {
                        nix::sys::socket::ControlMessageOwned::Ipv4OrigDstAddr(addr) => Some(addr),
                        _ => None,
                    })
                    .ok_or("no original destination address in message")?;

                let srcaddr = rmg.address.ok_or("no source address in message")?;
                trace!(
                    "Recv {} bytes, src: {}; dst: {}",
                    rmg.bytes,
                    pemi_io::print_addr(&srcaddr.as_ref()),
                    pemi_io::print_addr(&dstaddr)
                );

                // parse the quiche packet and identify the connection. ref: RFC 9000 and 9312
                // long header: try to parse, if is a QUIC initial packet, add new connection
                // short header: find connection and process packet

                let dstaddr = SockaddrIn::from(dstaddr);

                let pkt_len = rmg.bytes;
                buf.truncate(pkt_len);

                #[cfg(any(feature = "cycles"))]
                count_cycles(1, start_1);

                #[cfg(any(feature = "cycles"))]
                let start_3 = unsafe { _rdtsc() }; // count cycles of pemi computing

                pemi.process_packet(
                    buf,
                    recv_ts,
                    pemi_io::Addr::from_nix_addr(srcaddr),
                    pemi_io::Addr::from_nix_addr(dstaddr),
                )?;
                while let Some(task) = pemi.pop_retrans_task() {
                    debug!("process packet, retrans task: {}", task);
                    process_retrans_task(task, &mut pemi)?;
                }

                #[cfg(any(feature = "cycles"))]
                count_cycles(3, start_3);
            }
        }

        #[cfg(any(feature = "cycles"))]
        {
            count_cycles(0, start_0);
            print_cycles_count_summary(pemi.pkts()); // count when finish processing a packet
        }

        if pemi.pkts() - last_print_stats >= args.print_interval {
            assert_eq!(pemi.pkts() - last_print_stats, args.print_interval);
            last_print_stats = pemi.pkts();
            pemi.print_stats();
        }
    }
}

fn process_retrans_task(mut task: retrans::Task, pemi: &mut PEMI) -> Result<(), String> {
    if !RETRANS_HELP {
        return Ok(());
    }
    pemi.process_retrans_task(&mut task)?;
    Ok(())
}
