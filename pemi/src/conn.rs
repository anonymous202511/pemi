use std::cmp::max;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time;

use crate::cc;
use crate::common::Error;
use crate::pemi_io;
use crate::queue;
use crate::quic_parse;
use crate::retrans;
use crate::IDLE_TIMEOUT;

use log::{debug, info};

/// Smoothing factor for RTT measurement.
const RTT_SMOOTHING_FACTOR: f64 = 1.0 / 8.0;

// Paras for ACK delay
#[allow(non_upper_case_globals)]
const DELAY_kGranularity: time::Duration = time::Duration::from_millis(1);
#[allow(non_upper_case_globals)]
const DELAY_kTimeThreshold: f64 = 1.125; // 9/8 RTT
#[allow(non_upper_case_globals)]
const DELAY_kPacketThreshold: usize = 3; // 3 packets

/// Connection ID for PEMI connection management.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnId {
    addr1: SocketAddr,
    addr2: SocketAddr,
}

impl ConnId {
    /// Create a new connection ID from two socket addresses.
    /// the order of the two addresses is not important.
    pub fn new(addr1: SocketAddr, addr2: SocketAddr) -> Self {
        // return the id with the smaller address first
        if addr1 < addr2 {
            ConnId { addr1, addr2 }
        } else {
            ConnId {
                addr1: addr2,
                addr2: addr1,
            }
        }
    }
}

impl std::fmt::Display for ConnId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} <-> {}", self.addr1, self.addr2)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnState {
    /// After the client has sent the Initial packet
    Initialed,

    /// After the server has sent the Handshake packet
    Handshaked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DominantDirection {
    ToClient,
    ToServer,
    None,
}

// Delayed ACK structure to hold information about ACKs that are delayed for reordering
struct DelayedACK {
    forward_ts: time::Instant,
    payload: Vec<u8>,
    e2e_rtt: time::Duration,
}

/// Metadata for a QUIC connection.
/// It stores the connection state and the last access time.
pub struct Conn {
    state: ConnState,
    last_access: time::Instant,

    /// Begin time of the connection.
    begin_time: time::Instant,

    /// When a connection is created, add the client and server address.
    /// The src who send the first Initial packet is the client. The dst is the server.
    client_addr: pemi_io::Addr,
    server_addr: pemi_io::Addr,

    /// RTT from the client to PEMI.
    /// Unit: milliseconds.
    /// Init to 0, means has not been measured.
    client_rtt: time::Duration,

    /// RTT from the server to PEMI.
    /// Unit: milliseconds.
    /// Init to 0, means has not been measured.
    server_rtt: time::Duration,

    /// Queue of packets from the client to the server.
    to_server_queue: queue::PacketQueue,

    /// Since we now not push all packets to server into the queue, record the packet number here
    to_server_pkt_num: u64,

    /// Queue of packets from the server to the client.
    to_client_queue: queue::PacketQueue,

    /// Dominant Direction measurement
    min_pkt_size: usize, // minimum packet size of this connection
    dominant_direction: DominantDirection, // current dominant direction
    last_dominant_check: time::Instant,    // last time check dominant direction
    server_bytes: usize,                   // bytes from server in this RTT period
    client_bytes: usize,                   // bytes from client in this RTT period

    /// Congestion control
    cc: cc::Copa,
    overspeed: bool,                         // whether the sending rate is overspeed
    overspeed_begin: Option<time::Instant>,  // when the overspeed begins
    delayed_ack_queue: VecDeque<DelayedACK>, // queue of delayed acks for reordering

    /// Last RTT calibration time
    last_rtt_calibration: time::Instant,
}

impl Conn {
    pub fn new(now: time::Instant, src: SocketAddr, dst: SocketAddr) -> Self {
        Conn {
            state: ConnState::Initialed,
            last_access: now,
            begin_time: now,
            client_addr: pemi_io::Addr::from_std_addr(src), // for the first packet, the src is the client
            server_addr: pemi_io::Addr::from_std_addr(dst), // and the dst is the server
            client_rtt: time::Duration::from_secs(0),
            server_rtt: time::Duration::from_secs(0),
            to_server_queue: queue::PacketQueue::new(),
            to_server_pkt_num: 0,
            to_client_queue: queue::PacketQueue::new(),
            min_pkt_size: usize::MAX,
            dominant_direction: DominantDirection::None,
            last_dominant_check: now,
            server_bytes: 0,
            client_bytes: 0,
            cc: cc::Copa::new(now),
            overspeed: false,
            overspeed_begin: None,
            delayed_ack_queue: VecDeque::new(),
            last_rtt_calibration: now,
        }
    }

    pub fn set_factors(&mut self, flowlet_interval_factor: f64, flowlet_end_factor: f64) {
        self.to_server_queue
            .set_factors(flowlet_interval_factor, flowlet_end_factor);
        self.to_client_queue
            .set_factors(flowlet_interval_factor, flowlet_end_factor);
    }

    /// Return elapsed time since the connection is created.
    fn elapsed(&self, now: time::Instant) -> time::Duration {
        now.duration_since(self.begin_time)
    }

    /// Call when first packet of the connection comes.
    pub fn first_quic_packet(
        now: &time::Instant,
        src: &SocketAddr,
        dst: &SocketAddr,
        buf: &[u8],
    ) -> Result<(Self, usize), Error> {
        let mut b = octets::Octets::with_slice(buf);
        // expect a long header packet: Initial
        let hdr = quic_parse::Header::from_bytes(&mut b, 0)?;
        debug!("parsed pkt header(first): {:?}", hdr);
        if hdr.ty != quic_parse::Type::Initial {
            return Err(Error::MayNotQUIC);
        }

        // skip the payload
        b.skip(hdr.length)?;

        // check if the left is UDP padding
        let read = if b.cap() > 0 && quic_parse::Header::is_udp_padding(&mut b)? {
            debug!("UDP padding");
            buf.len()
        } else {
            b.off()
        };
        Ok((Conn::new(*now, *src, *dst), read))
    }

    /// For processing coalesced QUIC packets.
    /// Call when a following packet comes.
    pub fn process_quic_packet(
        &mut self,
        now: &time::Instant,
        buf: &[u8],
        src: &SocketAddr,
    ) -> Result<usize, Error> {
        let mut b = octets::Octets::with_slice(buf);
        self.update_access_time(*now);
        let read = if !self.is_handshaked() {
            // wait for handshake packet
            let hdr = quic_parse::Header::from_bytes(&mut b, 0)?;
            debug!("parsed pkt header(following): {:?}", hdr);

            if hdr.ty == quic_parse::Type::Handshake {
                assert!(!self.is_from_client(src));
                self.set_handshaked();
                info!("conn handshaked");
            }

            // skip the payload
            b.skip(hdr.length)?;

            // check if the left is UDP padding
            if b.cap() > 0 && quic_parse::Header::is_udp_padding(&mut b)? {
                debug!("UDP padding");
                buf.len()
            } else {
                b.off()
            }
        } else {
            // parse the packet (may be the short header packet)
            // The connection is handshaked. No need to parse the packet.
            // if want to parse short packets, need dcid_len from the connection
            buf.len()
        };

        Ok(read)
    }

    pub fn measure_dominant_direction(
        &mut self,
        recv_ts: time::Instant,
        from: &str,
        pkt_size: usize,
    ) {
        // update data every packet
        if pkt_size < self.min_pkt_size {
            self.min_pkt_size = pkt_size;
            debug!("new min pkt size: {}", self.min_pkt_size);
        } else {
            if from == "client" {
                self.client_bytes += pkt_size - self.min_pkt_size;
                debug!(
                    "client pkt size: {}, client_bytes: {}",
                    pkt_size, self.client_bytes
                );
            } else {
                self.server_bytes += pkt_size - self.min_pkt_size;
                debug!(
                    "server pkt size: {}, server_bytes: {}",
                    pkt_size, self.server_bytes
                );
            }
        }

        // check every RTT
        if recv_ts.duration_since(self.last_dominant_check) >= self.client_rtt + self.server_rtt {
            debug!(
                "check dominant direction, interval: {:?}",
                recv_ts.duration_since(self.last_dominant_check)
            );
            if self.client_bytes * 2 < self.server_bytes {
                // to client
                self.dominant_direction = DominantDirection::ToClient;
            } else if self.server_bytes * 2 < self.client_bytes {
                // to server
                self.dominant_direction = DominantDirection::ToServer;
            }
            debug!(
                "dominant direction: {:?}, client_bytes: {}, server_bytes: {}",
                self.dominant_direction, self.client_bytes, self.server_bytes
            );
            // reset
            self.last_dominant_check = recv_ts;
            self.server_bytes = 0;
            self.client_bytes = 0;
        }
    }

    pub fn is_from_client(&self, src: &SocketAddr) -> bool {
        *src == self.client_addr.std_addr
    }

    // smoothed_rtt = 7/8 * smoothed_rtt + 1/8 * sample_rtt
    fn update_client_rtt(&mut self, value: time::Duration, now: time::Instant) {
        // assert!(value >= RTT_GRANULARITY);
        if self.client_rtt.is_zero() {
            // the first client RTT
            self.client_rtt = value;
            debug!("initial client RTT: {:?}", self.client_rtt);
        } else {
            self.client_rtt = self.client_rtt.mul_f64(1.0 - RTT_SMOOTHING_FACTOR)
                + value.mul_f64(RTT_SMOOTHING_FACTOR);
            debug!("updated client RTT: {:?}", self.client_rtt);
        }
        self.cc.on_ack_send(self.client_rtt, now);
    }

    // smoothed_rtt = 7/8 * smoothed_rtt + 1/8 * sample_rtt
    // TODO: change as same as update_client_rtt
    fn update_server_rtt(&mut self, value: time::Duration) {
        self.server_rtt = value;
        debug!("server RTT: {:?}", self.server_rtt);
    }

    /// This used for case where we disable the PEMI enhancement, only forwarding the UDP packet.
    pub fn process_udp_packet_no_pemi(
        &mut self,
        recv_ts: time::Instant,
        src: &pemi_io::Addr,
        buf: &Vec<u8>,
    ) {
        let from;
        // push the UDP packet to the queue
        if self.is_from_client(&src.std_addr) {
            from = "client";
        } else {
            from = "server";
        }
        // output time and packet id
        info!(
            "process pkt({}) {:?} {} {}B",
            from,
            self.elapsed(recv_ts),
            queue::PacketQueue::packet_id(&buf),
            buf.len()
        );
    }

    /// Process the UDP packet.
    /// Return: whether the new protected flowlet(to client data) is created.
    pub fn process_udp_packet(
        &mut self,
        recv_ts: time::Instant,
        src: &pemi_io::Addr,
        _dst: &pemi_io::Addr,
        buf: Vec<u8>,
    ) -> bool {
        let from;
        let mut new_flowlet = false;
        // measure initial RTT of both sides
        if self.is_from_client(&src.std_addr) {
            from = "client";
            // process client packet
            if self.client_rtt.is_zero() && !self.server_rtt.is_zero() {
                // measure the first client RTT(PEMI<->client RTT)
                self.update_client_rtt(
                    recv_ts.duration_since(self.to_client_queue.oldest_ts().unwrap()), // when server_rtt > 0(measured), the server queue must have packets(the 1st is the Handshake/Retry packet)
                    recv_ts,
                );
            }
        } else {
            from = "server";
            // process server packet
            if self.server_rtt.is_zero() {
                // measure the first server RTT(PEMI<->server RTT)
                self.update_server_rtt(
                    recv_ts.duration_since(self.to_server_queue.oldest_ts().unwrap()), // when received the first server packet, the client queue must have packets(the 1st is the Initial packet)
                );
            }
        }
        self.measure_dominant_direction(recv_ts, from, buf.len());

        // output time and packet id
        debug!(
            "process pkt({}) {:?} {} {}B",
            from,
            self.elapsed(recv_ts),
            queue::PacketQueue::packet_id(&buf),
            buf.len()
        );
        // push the UDP packet to the queue
        if self.is_from_client(&src.std_addr) {
            // from client
            if self.server_rtt.is_zero() {
                // add new packet to the queue (only for the initial rtt measurement)
                self.to_server_queue
                    .add(recv_ts, Some(buf), self.server_rtt, false);
                debug!("to server queue: {}", self.to_server_queue);
            }

            // protect pkts to client: pkts to client expect reply from client
            self.to_server_pkt_num += 1; // for recording replies' pkt number in flowlet
            let rtt_samples =
                self.to_client_queue
                    .check_reply(recv_ts, self.to_server_pkt_num, self.client_rtt);
            if let Some(samples) = rtt_samples {
                for rtt_sample in samples {
                    self.update_client_rtt(rtt_sample, recv_ts);
                }
            }
            debug!("process client reply: {}", self.to_server_pkt_num);
        } else {
            // from server
            // CC: on data send
            if !self.client_rtt.is_zero() {
                self.overspeed = self.cc.on_data_send(recv_ts, self.client_rtt);
                if self.overspeed {
                    if self.overspeed_begin.is_none() {
                        self.overspeed_begin = Some(recv_ts);
                    }
                } else if self.overspeed_begin.is_some() {
                    self.overspeed_begin = None;
                }
            }
            // add new packet to the to_client queue
            let (pkt_num, new_fl) =
                self.to_client_queue
                    .add(recv_ts, Some(buf), self.client_rtt, true);
            new_flowlet = new_fl;
            debug!("process server data: {}", pkt_num);
            debug!("to client queue: {}", self.to_client_queue);
        }
        self.check_delayed_acks(recv_ts);
        new_flowlet
    }

    pub fn rtt_calibration(&mut self, calibration_rtt_sample: time::Duration) {
        let now_ts = time::Instant::now();
        if now_ts - self.last_rtt_calibration >= calibration_rtt_sample + self.server_rtt {
            // A new mimic spin-bit sample arrival time has come (exceeds 1 end-to-end RTT)
            self.last_rtt_calibration = now_ts;

            debug!(
                "RTT calibration at ts {:?}; calibration RTT: {:?}.",
                now_ts, calibration_rtt_sample
            );

            // If the difference is large, reset PEMI: delete all flowlets that have found a reply; only focus on flowlets that have no reply yet
            // calculate the RTT error
            let rtt_error = if calibration_rtt_sample >= self.client_rtt {
                calibration_rtt_sample - self.client_rtt
            } else {
                self.client_rtt - calibration_rtt_sample
            };

            let allowable_error = self
                .to_client_queue
                .flowlet_timeout(&calibration_rtt_sample)
                .mul_f64(self.to_client_queue.flowlet_end_factor); // small error is recoverable, so no need to reset

            debug!("RTT Error: {:?}, allowed: {:?}", rtt_error, allowable_error);

            if rtt_error > allowable_error {
                info!(
                    "Large RTT deviation detected: calibration RTT {:?} vs current client RTT {:?}. Resetting PEMI.",
                    calibration_rtt_sample,
                    self.client_rtt
                );
                self.to_client_queue.reset_due_to_rtt_deviation();
                if self.client_rtt < calibration_rtt_sample {
                    // reset rtt filters, since the min RTT may be erroneously small
                    self.cc.reset_rtt_filters();
                }
                self.client_rtt = calibration_rtt_sample;
            }
        }
    }

    /// Record the retransmission packet in the queue.
    pub fn record_retrans_packet(&mut self, forward_ts: time::Instant, src: SocketAddr) {
        // output time and packet id
        debug!("process retrans pkt {:?}", self.elapsed(forward_ts));
        // push the UDP packet to the queue
        if self.is_from_client(&src) {
            // TODO: retrans data pkts to server
            todo!("retrans data pkts must not be from-client");
        } else {
            // add new packet to the queue
            self.to_client_queue
                .add(forward_ts, None, self.client_rtt, true);
            debug!("to client queue: {}", self.to_client_queue);
        }
    }

    fn is_handshaked(&self) -> bool {
        self.state == ConnState::Handshaked
    }

    fn set_handshaked(&mut self) {
        self.state = ConnState::Handshaked;
    }

    fn update_access_time(&mut self, now: time::Instant) {
        self.last_access = now;
    }

    /// Get the timeout of the connection: the time for the loss detection. This now setted by the to client queue.
    pub fn timeout(&self, now: time::Instant) -> Option<time::Duration> {
        if self.client_rtt.is_zero() {
            // the client RTT has not been measured, can't set the timeout
            return None;
        } else {
            return self.to_client_queue.timeout(now, self.client_rtt);
        }

        // TODO: timeout for pkts to server
    }

    /// Call when the timeout of the connection is reached.
    pub fn on_timeout(&mut self, now: time::Instant) {
        debug!("on_timeout, {:?}", self.elapsed(now));
        let rtt_samples = self.to_client_queue.on_timeout(now, self.client_rtt);
        for rtt_sample in rtt_samples {
            self.update_client_rtt(rtt_sample, now);
        }
        // TODO: protect pkts to server
    }

    // Return true if need reorder this ack to influcence the sender's sending rate
    pub fn need_reorder_ack(&self, src: &pemi_io::Addr) -> bool {
        self.overspeed && self.is_from_client(&src.std_addr) && self.is_handshaked()
    }

    // Push an ack which waits for later reordering
    pub fn add_delayed_ack(&mut self, forward_ts: time::Instant, payload: &Vec<u8>) {
        let e2e_rtt = self.client_rtt + self.server_rtt;
        self.delayed_ack_queue.push_back(DelayedACK {
            forward_ts,
            payload: payload.clone(),
            e2e_rtt,
        });
        self.check_delayed_acks(forward_ts);
    }

    // Check the delayed acks and send if the delay time or packet number threshold is met
    pub fn check_delayed_acks(&mut self, now: time::Instant) {
        if self.delayed_ack_queue.is_empty() {
            return;
        }
        if self.overspeed == false {
            // no need to reorder acks
            while let Some(ack) = self.delayed_ack_queue.pop_front() {
                // send the packet transparently
                pemi_io::send_transparently(
                    &self.client_addr.nix_addr,
                    &self.server_addr.nix_addr,
                    &ack.payload,
                );
            }
            return;
        }
        // need at least 2 acks to reorder
        if self.delayed_ack_queue.len() < 2 {
            return;
        }

        let front_ack = self.delayed_ack_queue.front().unwrap();
        let pkt_thresh;
        let time_thresh;
        if now - self.overspeed_begin.unwrap() > front_ack.e2e_rtt {
            // if overspeed lasts more than 1 e2e RTT, use more aggressive thresholds
            pkt_thresh = DELAY_kPacketThreshold * 2;
            time_thresh = 1.0 + (DELAY_kTimeThreshold - 1.0) * 2.0;
        } else {
            pkt_thresh = DELAY_kPacketThreshold;
            time_thresh = DELAY_kTimeThreshold;
        }
        if self.delayed_ack_queue.len() > pkt_thresh
            || now - front_ack.forward_ts
                > max(front_ack.e2e_rtt.mul_f64(time_thresh), DELAY_kGranularity)
        {
            // 1. send the tail ack first
            let tail_ack = self.delayed_ack_queue.pop_back().unwrap();
            pemi_io::send_transparently(
                &self.client_addr.nix_addr,
                &self.server_addr.nix_addr,
                &tail_ack.payload,
            );
            // 2. send other acks in order
            while let Some(ack) = self.delayed_ack_queue.pop_front() {
                pemi_io::send_transparently(
                    &self.client_addr.nix_addr,
                    &self.server_addr.nix_addr,
                    &ack.payload,
                );
            }
        }
    }

    /// Get the retransmission task for the client.
    /// To client, so the dst is the client and the src is the server.
    pub fn to_client_retrans_task(&mut self) -> Option<retrans::Task> {
        retrans::Task::from_queue(
            &mut self.to_client_queue,
            self.server_addr.std_addr,
            self.client_addr.std_addr,
            self.dominant_direction == DominantDirection::ToClient,
            self.overspeed, // avoid fast retrans when overspeed, this is the primary purpose of CC in pemi
        )
    }

    pub fn is_idle(&self, now: time::Instant) -> bool {
        now.duration_since(self.last_access) >= IDLE_TIMEOUT
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    #[test]
    fn test_conn_id() {
        let addr1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 443);
        let addr2 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2)), 443);

        let c_id1 = ConnId::new(addr1, addr2);
        let c_id2 = ConnId::new(addr2, addr1);

        assert_eq!(c_id1, c_id2);
    }

    #[test]
    fn test_conn() {
        let now = time::Instant::now();
        let src = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 1111); // client
        let dst = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 443); // server
        let mut conn = Conn::new(now, src, dst);

        assert_eq!(conn.is_handshaked(), false);

        conn.set_handshaked();
        assert_eq!(conn.is_handshaked(), true);

        let new_now = time::Instant::now();
        conn.update_access_time(new_now);

        assert_eq!(conn.is_idle(new_now), false);
    }
}
