mod cc;
pub mod common;
pub mod conn;
mod minmax;
pub mod pemi_io;
mod queue;
pub mod quic_parse;
pub mod retrans;
mod rtt_det;

use common::Error;

use log::{debug, info, trace};

use std::time;

use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::net::SocketAddr;

#[cfg(any(feature = "cycles"))]
use std::arch::x86_64::_rdtsc;

pub const RETRANS_HELP: bool = true;

/// The idle timeout for a connection.
const IDLE_TIMEOUT: time::Duration = time::Duration::from_secs(120); // 2 minutes

/// [0]: all, [1]: recv_io, [2]: forward_io, [3]: pemi computing, [4]: recv wait time due to is_readable but recv failed
/// [0] - others = time to wait is_readable and all other cycles
#[cfg(any(feature = "cycles"))]
pub static mut CYCLES: [u64; 5] = [0; 5];
#[cfg(any(feature = "cycles"))]
static mut CYCLES_COUNT: u64 = 0;
#[cfg(any(feature = "cycles"))]
const WARMUP: u64 = 10;

/// Count the cycles for index `idx`.
#[cfg(any(feature = "cycles"))]
pub fn count_cycles(idx: usize, start: u64) {
    unsafe {
        CYCLES[idx] += _rdtsc() - start;
    }
}

#[cfg(any(feature = "cycles"))]
pub fn print_cycles_count_summary(recv_pkts: u64) {
    unsafe {
        if CYCLES_COUNT == WARMUP {
            // println!("warmup finished");
            CYCLES[0] = 0;
            CYCLES[1] = 0;
            CYCLES[2] = 0;
            CYCLES[3] = 0;
            CYCLES[4] = 0;
            println!(
                "cycles: All, Recv IO (read), Forward IO, PEMI Computing(includes Forward IO), Recv IO (EAGAIN), Other (block wait, etc)"
            );
        }
        CYCLES_COUNT += 1;
        if CYCLES_COUNT % 100 == 0 {
            // println!("{:?}", [CYCLES]);
            let cycles_all = CYCLES[0] as f64;
            // get the percentage of each cycle type
            let cycles_norm = CYCLES
                .clone()
                .into_iter()
                .map(|cycles| cycles as f64 / cycles_all * 100.0)
                .collect::<Vec<_>>();
            println!(
                "cycle%: {:?}",
                [
                    cycles_norm[0],                                           // all
                    cycles_norm[1],                                           // recv_io
                    cycles_norm[2],                                           // forward_io
                    cycles_norm[3],                                           // pemi computing
                    cycles_norm[4], // extra recv wait time
                    cycles_norm[0] - cycles_norm.iter().skip(1).sum::<f64>(), // other
                ],
            );
            // for debug: print the cycle counts
            let million = 1_000_000;
            println!(
                "cycles(Million): {:?}",
                [
                    CYCLES[0] / million,                                                   // all
                    CYCLES[1] / million, // recv_io
                    CYCLES[2] / million, // forward_io
                    CYCLES[3] / million, // pemi computing
                    CYCLES[4] / million, // extra recv wait time
                    (CYCLES[0] - CYCLES[1] - CYCLES[2] - CYCLES[3] - CYCLES[4]) / million, // other
                ],
            );
            let kilo = 1_000;
            println!(
                "cycles per pkt(K): {:?}",
                [
                    CYCLES[0] / recv_pkts / kilo, // all
                    CYCLES[1] / recv_pkts / kilo, // recv_io
                    CYCLES[2] / recv_pkts / kilo, // forward_io
                    CYCLES[3] / recv_pkts / kilo, // pemi computing
                    CYCLES[4] / recv_pkts / kilo, // extra recv wait time
                    (CYCLES[0] - CYCLES[1] - CYCLES[2] - CYCLES[3] - CYCLES[4]) / recv_pkts / kilo, // other
                ],
            )
        }
    }
}

/// To manage connections, we need to store the connection state and the access time.
/// Enable min-heap to find the oldest connection.
#[derive(Debug, Eq, PartialEq)]
struct AccessTime(time::Instant, conn::ConnId);

impl Ord for AccessTime {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.0.cmp(&self.0) // reverse the cmp order, so the oldest time is at the top
    }
}

impl PartialOrd for AccessTime {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Statistics of the PEMI.
struct Stats {
    /// Processed UDP packets.
    pkts: u64,

    /// Retransmission packets.
    retrans_pkts: u64,
}

impl Stats {
    fn new() -> Self {
        Stats {
            pkts: 0,
            retrans_pkts: 0,
        }
    }

    // Increment the number of processed packets.
    fn new_pkt(&mut self) {
        self.pkts += 1;
    }

    // Increment the number of retransmission packets.
    fn new_retrans_pkt(&mut self) {
        self.retrans_pkts += 1;
    }
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "pkts: {}", self.pkts)
    }
}

pub struct PEMI {
    /// Connections.
    conns: HashMap<conn::ConnId, conn::Conn>,

    /// Active RTT detection
    pub rtt_detector: rtt_det::RttDetector,

    /// A priority queue to store the access time of connections.
    /// The connection ID is used to find the connection in the `conns` map.
    ///
    /// The connection with the earliest access time is at the top.
    /// When the top connection is considered idle, check the conns map: update the access time or remove the connection.
    access_times: BinaryHeap<AccessTime>,

    /// Statistics.
    stats: Stats,

    /// Retransmission tasks.
    retrans_tasks: Vec<retrans::Task>,

    /// Factors for the PEMI.
    flowlet_interval_factor: f64,
    flowlet_end_factor: f64,

    /// Retransmission rate limit.
    retrans_rate_limit: f64,

    /// Is set as True, only transparent forwarding. (not enable PEMI)
    proxy_only: bool,
}

impl PEMI {
    pub fn new() -> Self {
        PEMI {
            conns: HashMap::new(),
            rtt_detector: rtt_det::RttDetector::new(),
            access_times: BinaryHeap::new(),
            stats: Stats::new(),
            retrans_tasks: Vec::new(),
            flowlet_interval_factor: 2.0,
            flowlet_end_factor: 2.0,
            retrans_rate_limit: 0.1, // 1.0 means no limit, we now said 10% in the paper
            proxy_only: false,
        }
    }

    pub fn set_proxy_only(&mut self, proxy_only: bool) {
        self.proxy_only = proxy_only;
    }

    /// Set the factors for the PEMI.
    /// flowlet_interval_factor
    /// flowlet_end_factor
    pub fn set_factors(&mut self, flowlet_interval_factor: f64, flowlet_end_factor: f64) {
        self.flowlet_interval_factor = flowlet_interval_factor;
        self.flowlet_end_factor = flowlet_end_factor;
    }

    /// Process a UDP packet.
    pub fn process_packet(
        &mut self,
        buf: Vec<u8>,
        recv_ts: time::Instant,
        srcaddr: pemi_io::Addr,
        dstaddr: pemi_io::Addr,
    ) -> Result<(), Error> {
        self.stats.new_pkt();
        trace!("-----\nstats: {}", self.stats);

        let conn_id = conn::ConnId::new(srcaddr.std_addr, dstaddr.std_addr);
        trace!("pkt Conn ID: {conn_id}");

        let res = self.quic_conn_process(
            &buf,
            &conn_id,
            &recv_ts,
            &srcaddr.std_addr,
            &dstaddr.std_addr,
        );

        if res == Err(Error::InvalidState) || res == Err(Error::MayNotQUIC) {
            // not a QUIC packet
            // send the packet transparently
            pemi_io::send_transparently(&srcaddr.nix_addr, &dstaddr.nix_addr, &buf);
            trace!(
                "{} not a QUIC packet, sent transparently. parse res: {}",
                queue::PacketQueue::packet_id(&buf),
                res.unwrap_err()
            );
            return Ok(());
        } else if res.is_err() {
            return res;
        }

        // is QUIC packet, do following processing

        // Process the UDP packet by the connection.
        let conn = self
            .conns
            .get_mut(&conn_id)
            .expect("the connection must exist"); // quic_conn_process will add the connection if not exist
        if self.proxy_only {
            pemi_io::send_transparently(&srcaddr.nix_addr, &dstaddr.nix_addr, &buf);
            conn.process_udp_packet_no_pemi(recv_ts, &srcaddr, &buf);
        } else {
            if conn.need_reorder_ack(&srcaddr) {
                conn.add_delayed_ack(recv_ts, &buf);
            } else {
                // send the packet transparently
                pemi_io::send_transparently(&srcaddr.nix_addr, &dstaddr.nix_addr, &buf);
            }
            let new_flowlet = conn.process_udp_packet(recv_ts, &srcaddr, &dstaddr, buf);
            if new_flowlet {
                // send ICMP RTT request for the new flowlet for debug purpose. calibration used it every E2E RTT
                self.rtt_detector.send_request(dstaddr.std_addr);
                debug!(
                    "recv a packet to addr: {:?}, ICMP request sent",
                    dstaddr.std_addr.ip()
                );
            }
        }

        if let Some(task) = conn.to_client_retrans_task() {
            self.retrans_tasks.push(task);
        }

        // remove idle connections
        self.remove_idle_conns(recv_ts);

        Ok(())
    }

    /// Calibrate the RTT based on the sample from RTT detector.
    /// TODO:
    /// Recognize the connection with the same receiver address, and only calibrate the RTT for those connections.
    pub fn rtt_calibration(&mut self, calibration_rtt_sample: time::Duration) {
        for (_, conn) in self.conns.iter_mut() {
            conn.rtt_calibration(calibration_rtt_sample);
        }
    }

    /// Process a retransmission packet in the queue.
    fn record_retrans_packet(
        &mut self,
        srcaddr: SocketAddr,
        dstaddr: SocketAddr,
    ) -> Result<(), Error> {
        let conn_id = conn::ConnId::new(srcaddr, dstaddr);
        debug!("pkt Conn ID: {conn_id}");

        let now: time::Instant = time::Instant::now();

        // Process the UDP packet by the connection.
        let conn = self
            .conns
            .get_mut(&conn_id)
            .expect("the connection must exist"); // quic_conn_process will add the connection if not exist
        conn.record_retrans_packet(now, srcaddr);

        Ok(())
    }

    /// Process a retransmission task.
    pub fn process_retrans_task(&mut self, task: &mut retrans::Task) -> Result<(), Error> {
        let src = pemi_io::to_nix_addr(task.src());
        let dst = pemi_io::to_nix_addr(task.dst());
        while let Some(pkt) = task.pop_front() {
            let buf = pkt.payload();
            if self.match_retrans_limit() && self.pkts() > 100 {
                // avoid too early limit. To support initial retransmissions.
                debug!("retransmission rate limit, skip a retransmission packet");
                // If used for multiple connections, this need to be checked in the connection level.
                continue;
            }
            pemi_io::send_transparently(&src, &dst, buf);
            self.stats.new_retrans_pkt(); // Increment retransmission counter
            self.record_retrans_packet(*task.src(), *task.dst())?;
        }
        Ok(())
    }

    /// Process coalesced QUIC packets. Now only for the handshake tracking.
    /// If needed, create a new connection.
    /// For new connection, if is not a QUIC Initial packet, now will return error.
    /// For now, this only used for checking if the connection is QUIC. Though the handshake is tracked, and all the long and short header can be parsed, we only use the Initial packet.
    pub fn quic_conn_process(
        &mut self,
        buf: &[u8],
        conn_id: &conn::ConnId,
        now: &time::Instant,
        src: &SocketAddr,
        dst: &SocketAddr,
    ) -> Result<(), Error> {
        let len: usize = buf.len();
        let mut left = len;
        while left > 0 {
            // Process a single QUIC packet. A UDP packet may contain multiple QUIC packets.
            // On success the number of bytes processed from the input buffer is returned.
            let read = match self.conns.get_mut(&conn_id) {
                None => {
                    // new connection
                    let (conn, read) =
                        conn::Conn::first_quic_packet(now, src, dst, &buf[len - left..len])?;
                    self.new_conn(*conn_id, conn, *now);
                    self.rtt_detector.fresh_begin_time(*now); // make sure the rtt detector ts is synced with the connection(only useful in debug with only one connection)
                    info!("conn new added: {conn_id}");
                    read
                }
                Some(c) => {
                    // existing connection
                    // process a QUIC packet by the connection
                    c.process_quic_packet(now, &buf[len - left..len], src)?
                }
            };
            left -= read;
            trace!("processed {read} bytes, {left} bytes left");
        }
        Ok(())
    }

    /// Add a new connection.
    pub fn new_conn(&mut self, conn_id: conn::ConnId, mut conn: conn::Conn, now: time::Instant) {
        conn.set_factors(self.flowlet_interval_factor, self.flowlet_end_factor);
        self.conns.insert(conn_id, conn);
        self.access_times.push(AccessTime(now, conn_id));
    }

    fn remove_idle_conns(&mut self, now: time::Instant) {
        debug!("check idle conn");
        loop {
            let (oldest_time, oldest_conn_id) = match self.access_times.peek() {
                None => {
                    debug!("no conn in the heap");
                    break;
                }
                Some(AccessTime(t, c)) => (*t, *c),
            };

            if now.duration_since(oldest_time) >= IDLE_TIMEOUT {
                // the top connection may be idle
                // check the connection
                let c = self
                    .conns
                    .get(&oldest_conn_id)
                    .expect("the connection must exist");
                if c.is_idle(now) {
                    // remove the connection
                    self.conns.remove(&oldest_conn_id); // from the map
                    self.access_times.pop(); // from the heap
                    info!(
                        "conn removed: {}, {} conns left",
                        oldest_conn_id,
                        self.conns.len(),
                    );
                } else {
                    // the connection is not idle
                    // update the access time and reinsert to the heap
                    debug!("conn updated: {oldest_conn_id}");
                    self.access_times.pop();
                    self.access_times.push(AccessTime(now, oldest_conn_id));
                }
            } else {
                // even the top connection is not idle
                // no need to check the rest
                debug!("no more idle conns");
                break;
            }
        }
    }

    pub fn timeout(&mut self) -> Option<time::Duration> {
        let now = time::Instant::now();

        // connection access time timeout
        let idle_timer = match self.access_times.peek() {
            None => return None, // no connection now, PEMI should wait for the first packet
            Some(AccessTime(t, _)) => IDLE_TIMEOUT.saturating_sub(now.duration_since(*t)),
        };

        // timeout for recv the reply
        let reply_timeout = match self.conns.values().filter_map(|c| c.timeout(now)).min() {
            None => time::Duration::MAX, // no connection has the timeout
            Some(t) => t,
        };

        // return the minimum timeout
        let timers = [idle_timer, reply_timeout];
        let timeout = timers.iter().min().cloned();
        timeout
    }

    pub fn process_timeout(&mut self) -> Result<(), Error> {
        debug!("-----\ntimeout before recv an UDP packet");

        let now = time::Instant::now();

        // remove idle connections
        self.remove_idle_conns(now);

        // timeout on the connections
        for (_, conn) in self.conns.iter_mut() {
            if let Some(timeout) = conn.timeout(now) {
                if timeout.is_zero() {
                    // timeout
                    conn.on_timeout(now);
                }
            }
            if let Some(task) = conn.to_client_retrans_task() {
                self.retrans_tasks.push(task);
            }
        }

        Ok(())
    }

    pub fn has_retrans_task(&self) -> bool {
        !self.retrans_tasks.is_empty()
    }

    pub fn pop_retrans_task(&mut self) -> Option<retrans::Task> {
        self.retrans_tasks.pop()
    }

    /// Get processed packets.
    pub fn pkts(&self) -> u64 {
        self.stats.pkts
    }

    /// Print the statistics.
    pub fn print_stats(&self) {
        // now used information: 1. processed packets, 2. retransmission rate
        assert!(self.stats.pkts > 0);
        debug!(
            "-----stats: processed pkts: {}, retrans rate: {}",
            self.stats.pkts,
            self.stats.retrans_pkts as f64 / self.stats.pkts as f64
        );
    }

    /// Check if the retransmission rate is limited.
    fn match_retrans_limit(&self) -> bool {
        if self.stats.retrans_pkts as f64 / self.stats.pkts as f64 > self.retrans_rate_limit {
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn genarate_addrs(seed: u16) -> (SocketAddr, SocketAddr) {
        // use the seed to generate the different addresses
        let addr1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 443 + seed);
        let addr2 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2)), 443 + seed);
        (addr1, addr2)
    }

    #[tokio::test]
    async fn test_timeout() {
        let mut pemi = PEMI::new();
        // 1. no connection, no timeout
        assert_eq!(pemi.timeout(), None);

        let (addr1, addr2) = genarate_addrs(0);
        let conn_id = conn::ConnId::new(addr1, addr2);

        // add a connection
        let now = time::Instant::now();
        let conn = conn::Conn::new(now, addr1, addr2);
        pemi.new_conn(conn_id, conn, now);

        // 2. the connection has no measured rtt, timeout is the idle timeout
        let time_passed = IDLE_TIMEOUT - pemi.timeout().unwrap();
        assert!(
            time::Duration::ZERO < time_passed && time_passed < time::Duration::from_micros(100)
        ); // the timeout should be close to the idle timeout (idle time - first packet time)

        // 3. the connection has measured rtt, timeout is the measured rtt
        // TODO: this test need to construct more packets to measure the rtt; or make the rtt in conn public(now private)
    }

    #[tokio::test]
    async fn test_new_conn() {
        let mut pemi = PEMI::new();
        assert_eq!(pemi.conns.len(), 0);
        assert_eq!(pemi.access_times.len(), 0);
        let (addr1, addr2) = genarate_addrs(0);
        let conn_id = conn::ConnId::new(addr1, addr2);
        let now = time::Instant::now();
        let conn = conn::Conn::new(now, addr1, addr2);
        pemi.new_conn(conn_id, conn, now);
        assert_eq!(pemi.conns.len(), 1);
        assert_eq!(pemi.access_times.len(), 1);
    }

    #[tokio::test]
    async fn test_remove_idle_conns() {
        let mut pemi = PEMI::new();

        // first connection
        let (addr1, addr2) = genarate_addrs(0);
        let conn_id = conn::ConnId::new(addr1, addr2);
        let begin_time = time::Instant::now();
        let conn = conn::Conn::new(begin_time, addr1, addr2);
        pemi.new_conn(conn_id, conn, begin_time);
        assert_eq!(pemi.conns.len(), 1);
        assert_eq!(pemi.access_times.len(), 1);

        // the connection is not idle
        pemi.remove_idle_conns(begin_time);
        assert_eq!(pemi.conns.len(), 1);
        assert_eq!(pemi.access_times.len(), 1);

        // another connection
        let (addr1, addr2) = genarate_addrs(1);
        let conn_id = conn::ConnId::new(addr1, addr2);
        let conn_time = begin_time + time::Duration::from_secs(60);
        let conn = conn::Conn::new(conn_time, addr1, addr2);
        pemi.new_conn(conn_id, conn, conn_time);
        assert_eq!(pemi.conns.len(), 2);
        assert_eq!(pemi.access_times.len(), 2);

        // the first connection is idle
        let now = begin_time + IDLE_TIMEOUT;
        pemi.remove_idle_conns(now);
        assert_eq!(pemi.conns.len(), 1);
        assert_eq!(pemi.access_times.len(), 1);
    }
}
