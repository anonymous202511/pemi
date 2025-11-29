/* A queue of packets. Used in PEMI packets and flowlets management. */

use log::{debug, error, info, trace};
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::time;

/// if two packets' interval < close_threshold, they are considered may be replyed in a single packet.
/// now set as 0.1ms
const CLOSE_THRESHOLD: time::Duration = time::Duration::from_micros(100);

/// The maximum number of packets in a flowlet.
/// If the number of packets in a flowlet exceeds this value, the flowlet is considered as complete.
/// In paper: when the number of packets in a flowlet exceeds 𝑁, PEMI abandons the protection for it. We set 𝑁 as 100 in our experiments in Section 5.
const FLOWLET_MAX_PKTS: usize = 100;

/// eliciting_threshold setting
const DEFAULT_ELICITING_THRESHOLD: u8 = 1;
const WHEN_MEASURE_ELICITING_THRESHOLD: u64 = 50; // measure the eliciting threshold for every WHEN_MEASURE_ELICITING_THRESHOLD packets
const THRESHOLD_FOR_1_ELICITING_THRESHOLD: f64 = 0.6; // if reply ratio > this value, set eliciting threshold as 1; else set as 2

/// Duration ratio threshold to decide whether the lost pkts are not edge pkts; this decide the used RTT for sent-reply matching, and whether we use the RTT samples from a flowlet.
const DURATION_RATIO_THRESHOLD: f64 = 0.8;

/// Each flowlet corresponds to a sub-queue of packets.
/// This struct manages the flowlet metadata and packet nums.
struct Flowlet {
    /// The packets in the flowlet.
    /// Each packet number is as the same as the packet number in the `PacketQueue.packets UdpPacket.number`.
    pkt_nums: VecDeque<u64>,

    /// The packet number that: packet.time-pre_packet.time < CLOSE_THRESHOLD
    close_count: usize,

    /// Reply packet numbers for the packets in the flowlet.
    reply_pkt_times: Vec<time::Instant>,
    /// The packet numbers of the reply packets.
    reply_pkt_nums: Vec<u64>,

    /// The time when the first packet of the flowlet comes.
    begin_time: time::Instant,

    /// The time when the last packet of the flowlet comes.
    end_time: time::Instant,

    /// If the flowlet is complete.
    complete: bool,
}

impl Flowlet {
    /// Create a new flowlet with the first packet number.
    fn new(pkt_num: u64, begin_time: time::Instant) -> Self {
        Flowlet {
            pkt_nums: VecDeque::from(vec![pkt_num]),
            close_count: 0,
            reply_pkt_times: Vec::new(),
            reply_pkt_nums: Vec::new(),
            begin_time,
            end_time: begin_time,
            complete: false,
        }
    }

    /// Add a packet to the flowlet.
    fn add(&mut self, packet_num: u64, forward_ts: time::Instant) {
        // check if the packet is close to the last packet
        if !self.pkt_nums.is_empty() {
            if forward_ts - self.end_time < CLOSE_THRESHOLD {
                self.close_count += 1;
                trace!(
                    "flowlet add close packet, close_count: {}",
                    self.close_count
                );
            }
        }
        // add the packet
        self.pkt_nums.push_back(packet_num);
        self.end_time = forward_ts;
    }

    /// Label the flowlet as complete.
    fn set_as_complete(&mut self) {
        self.complete = true;
    }

    /// Check if the flowlet is complete.
    fn is_complete(&self) -> bool {
        self.complete
    }

    /// Check if all packets in the flowlet have been replied exactly.
    fn exactly_replyed(&self) -> bool {
        self.reply_pkt_times.len() == self.pkt_nums.len()
    }

    /// Add a reply.
    fn add_reply(&mut self, come_time: time::Instant, pkt_num: u64) {
        self.reply_pkt_times.push(come_time);
        self.reply_pkt_nums.push(pkt_num);
        if self.reply_pkt_times.len() > self.pkt_nums.len() {
            debug!("reply num > data num");
        }
    }

    /// Establish mapping between sent and reply packets when only part of packets are replied.
    /// Output: sent_to_reply_map. A indexs list(len=len of sent_pkts): sent_to_reply_map[i] is the sent_pkts[i] matched reply_pkts index
    /// This function uses a DP algorithm to find the best mapping.
    fn match_sent_part_reply(
        &self,
        sent_pkt_times: &Vec<time::Instant>,
        reply_rtt: time::Duration,
    ) -> Vec<usize> {
        // select used_rtt by reply_duration/sent_duration
        let sent_duration = self.end_time - self.begin_time;
        let reply_duration = {
            let first = self.reply_pkt_times.first().unwrap();
            let last = self.reply_pkt_times.last().unwrap();
            last.duration_since(*first)
        };

        let used_rtt = if reply_duration < sent_duration.mul_f64(DURATION_RATIO_THRESHOLD) {
            reply_rtt
        } else {
            // use end sample
            self.reply_pkt_times
                .last()
                .unwrap()
                .duration_since(self.end_time)
        };

        debug!("Used RTT for matching: {:?}", used_rtt);

        let base = sent_pkt_times[0]; // only used in this function for time to f64 conversion

        // convert Instant to f64 seconds since base
        assert_ne!(sent_pkt_times.len(), 0); // a flowlet should have at least one sent packet
        assert_ne!(self.reply_pkt_times.len(), 0); // a flowlet without reply should not call this function
        let sent: Vec<f64> = sent_pkt_times
            .iter()
            .map(|t| t.duration_since(base).as_secs_f64())
            .collect();

        let reply: Vec<f64> = self
            .reply_pkt_times
            .iter()
            .map(|t| t.duration_since(base).as_secs_f64())
            .collect();

        self.match_sent_reply_dp(sent, reply, used_rtt.as_secs_f64())
    }

    // Match sent packets to reply packets using a DP algorithm.
    // The DP algorithm minimizes ∑ |(reply[j] - sent[i]) - rtt|,
    // while preserving temporal order (monotonic matching).
    // Returns the index mapping from sent to reply; unmatched entries are set to usize::MAX.
    fn match_sent_reply_dp(&self, sent: Vec<f64>, reply: Vec<f64>, used_rtt: f64) -> Vec<usize> {
        let n = sent.len();
        let m = reply.len();

        let inf = f64::INFINITY;

        // dp[i][j]: minimal error for first i sent and first j reply
        let mut dp = vec![vec![inf; m + 1]; n + 1];
        // prev[i][j]: (prev_i, prev_j, match), match = reply index or -1
        let mut prev: Vec<Vec<Option<(usize, usize, isize)>>> = vec![vec![None; m + 1]; n + 1];

        dp[0][0] = 0.0;
        for i in 1..=n {
            dp[i][0] = 0.0;
            // skip sent[i-1]
            prev[i][0] = Some((i - 1, 0, -1));
        }

        for i in 1..=n {
            // the already matched reply num should not > sent num
            let upto = std::cmp::min(i, m);
            for j in 1..=upto {
                // option A: match sent[i-1] with reply[j-1]
                let cost = ((reply[j - 1] - sent[i - 1]) - used_rtt).abs();

                let mut best = dp[i - 1][j - 1] + cost;
                let mut best_prev = (i - 1, j - 1, (j - 1) as isize);

                // option B: skip sent[i-1]
                if dp[i - 1][j] < best {
                    best = dp[i - 1][j];
                    best_prev = (i - 1, j, -1);
                }

                dp[i][j] = best;
                prev[i][j] = Some(best_prev);
            }
        }

        // Backtrack to construct the mapping
        let mut i = n;
        let mut j = m;
        let mut mapping = vec![usize::MAX; n];

        while i > 0 {
            let (pi, pj, matched) = prev[i][j].expect("prev[i][j] should be set");
            if matched != -1 {
                mapping[i - 1] = matched as usize;
            }
            i = pi;
            j = pj;
        }

        mapping
    }

    // find partly lossed packets considering the close packets and eliciting_threshold
    // based on the sent→reply mapping to infer the reply packets.
    // 1. All packets adjacent to a mapped reply packet are considered replied.
    // 2. All packets within eliciting_threshold before and after a mapped reply packet are considered replied.
    // return the lost packet index list
    fn extract_part_loss(
        &self,
        sent_pkt_times: &Vec<time::Instant>,
        sent_to_reply_map: &Vec<usize>,
        eliciting_threshold: u8,
    ) -> BTreeSet<u64> {
        let n = sent_pkt_times.len();

        // inferred_reply[i] = reply index, or usize::MAX if no inferred reply
        let mut inferred_reply = vec![usize::MAX; n];

        for i in 0..n {
            let reply = sent_to_reply_map[i];
            if reply == usize::MAX {
                continue;
            }
            inferred_reply[i] = reply;

            // find close packets before
            let mut j = i as isize - 1;
            while j >= 0
            && inferred_reply[j as usize] == usize::MAX // stop when meet pkt already has inferred reply
            && sent_pkt_times[j as usize + 1] - sent_pkt_times[j as usize] < CLOSE_THRESHOLD
            {
                inferred_reply[j as usize] = reply;
                j -= 1;
            }

            // find close packets after
            let mut j = i + 1;
            while j < n
                && inferred_reply[j] == usize::MAX
                && sent_pkt_times[j] - sent_pkt_times[j - 1] < CLOSE_THRESHOLD
            {
                inferred_reply[j] = reply;
                j += 1;
            }
        }

        // considering eliciting_threshold: find packets within eliciting_threshold before every reply by two-pointer algorithm
        if eliciting_threshold >= 2 {
            let mut left = 0;
            while left < inferred_reply.len() {
                if inferred_reply[left] != usize::MAX {
                    left += 1;
                    continue;
                }

                // Starting from left, expand to the right to find a continuous range of usize::MAX
                let mut right = left;
                while right < inferred_reply.len() && inferred_reply[right] == usize::MAX {
                    right += 1;
                }

                // skip the end unreplied area(hope has reply for the end packets)
                if right == inferred_reply.len() {
                    break;
                }

                let len = right - left;

                if len < eliciting_threshold as usize {
                    assert_eq!(len, 1); // now only support eliciting_threshold as 2 and 1 in PEMI
                    for i in left..right {
                        inferred_reply[i] = 0; // mark as replied
                    }
                }

                left = right;
            }
        }

        // lost packets are those without inferred reply
        let mut lossed_pkts = BTreeSet::new();
        for i in 0..n {
            if inferred_reply[i] == usize::MAX {
                lossed_pkts.insert(self.pkt_nums[i]);
            }
        }
        lossed_pkts
    }

    fn extract_rtt_samples(
        &self,
        sent_to_reply_map: &Vec<usize>,
        sent_pkt_times: &Vec<time::Instant>,
    ) -> Vec<time::Duration> {
        // extract RTT samples
        let mut rtt_samples = Vec::new();
        for (i, &reply_index) in sent_to_reply_map.iter().enumerate() {
            if reply_index != usize::MAX {
                let reply_ts = self.reply_pkt_times[reply_index];
                let sent_ts = sent_pkt_times[i];

                let rtt_sample = reply_ts.duration_since(sent_ts);
                rtt_samples.push(rtt_sample);
            }
        }
        rtt_samples
    }
}

impl std::fmt::Debug for Flowlet {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.pkt_nums.len() {
            0 => return write!(f, "Flowlet [empty]"),
            1 => {
                write!(
                    f,
                    "Flowlet: len 1, [{}], reply num: {}, reply_pkts: {:?}",
                    self.pkt_nums.front().unwrap(),
                    self.reply_pkt_times.len(),
                    self.reply_pkt_nums
                )
            }
            len => {
                write!(
                    f,
                    "Flowlet: len {}, [{}~{}], reply num: {}, reply_pkts: {:?}",
                    len,
                    self.pkt_nums.front().unwrap(),
                    self.pkt_nums.back().unwrap(),
                    self.reply_pkt_times.len(),
                    self.reply_pkt_nums
                )
            }
        }
    }
}

/// A raw UDP packet.
pub struct RawUdpPacket {
    /// Packet number. (Recorded by the PEMI, no end-to-end meaning. For example, this is not the serial number of TCP, nor the packet number of QUIC.)
    /// The first packet is 1.
    number: u64,

    /// Timestamp of this packet.
    timestamp: time::Instant,

    /// Payload of this packet. Used in retranmission.
    payload: Vec<u8>,
}

impl RawUdpPacket {
    /// Return the payload of the packet.
    pub fn payload(&self) -> &Vec<u8> {
        &self.payload
    }

    /// Return the packet number.
    pub fn pkt_num(&self) -> u64 {
        self.number
    }
}

impl std::fmt::Debug for RawUdpPacket {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "UdpPacket {{ number: {}, timestamp: {:?}, payload len: {} }}",
            self.number,
            self.timestamp,
            self.payload.len()
        )
    }
}

impl std::fmt::Display for RawUdpPacket {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.number)
    }
}

/// A retransmitted UDP packet.
/// same as UdpPacket, but without payload(no need to retrans again).
#[derive(Debug)]
pub struct RetransUdpPacket {
    /// Packet number. (Recorded by the PEMI, no end-to-end meaning. For example, this is not the serial number of TCP, nor the packet number of QUIC.)
    /// The first packet is 1.
    number: u64,

    /// Timestamp of this packet.
    timestamp: time::Instant,
}

/// A UDP packet.
#[derive(Debug)]
enum Packet {
    Raw(RawUdpPacket),         // raw packet
    Retrans(RetransUdpPacket), // retransmit packet
}

impl Packet {
    /// Return the packet number.
    pub fn number(&self) -> u64 {
        match self {
            Packet::Raw(pkt) => pkt.number,
            Packet::Retrans(pkt) => pkt.number,
        }
    }

    /// Return the timestamp of the packet.
    pub fn timestamp(&self) -> time::Instant {
        match self {
            Packet::Raw(pkt) => pkt.timestamp,
            Packet::Retrans(pkt) => pkt.timestamp,
        }
    }
}

/// A queue of packets.
/// Each connection has two queues: one for the packets-from-client and one for the packets-from-server.
#[derive(Debug)]
pub struct PacketQueue {
    /// The packets in the queue.
    packets: VecDeque<Packet>,

    /// To be retranmitted packets.
    detected_loss: VecDeque<RawUdpPacket>,

    /// Flowlets of the connection.
    flowlets: VecDeque<Flowlet>,

    /// Processed packets.
    /// Used to provide the next packet number.
    /// Init to 0, means no packet has been processed.
    processed: u64,

    /// timestamp of the last packet
    last_packet_time: time::Instant,

    /// smoothed packet interval
    smoothed_interval: time::Duration,

    /// Measured eliciting_threshold
    eliciting_threshold: u8,
    /// Processed reply packets. Now only used to measure eliciting threshold.
    reply_nums: u64,

    /// The factors of the flowlet timeout.
    flowlet_interval_factor: f64,
    pub flowlet_end_factor: f64,
}

impl PacketQueue {
    pub fn new() -> Self {
        PacketQueue {
            packets: VecDeque::new(),
            detected_loss: VecDeque::new(),
            flowlets: VecDeque::new(),
            processed: 0,
            reply_nums: 0,
            last_packet_time: time::Instant::now(), // init as now
            smoothed_interval: time::Duration::from_millis(1), // init as 1ms
            eliciting_threshold: DEFAULT_ELICITING_THRESHOLD,
            flowlet_interval_factor: 2.0,
            flowlet_end_factor: 2.0,
        }
    }

    pub fn set_factors(&mut self, flowlet_interval_factor: f64, flowlet_end_factor: f64) {
        self.flowlet_interval_factor = flowlet_interval_factor;
        self.flowlet_end_factor = flowlet_end_factor;
    }

    /// Return the timeout of the queue.
    /// The timeout is setted by the first flowlet.
    /// timeout = time to the last packet of flowlet1 + flowlet timeout
    /// Return Some(Duration to timeout). Some(0) means timeout already happened
    pub fn timeout(&self, now: time::Instant, reply_rtt: time::Duration) -> Option<time::Duration> {
        // the timeout is setted by the first flowlet: should recv the reply of the end packet
        let fl_timeout = match self.flowlets.front() {
            None => None, //no flowlet in the queue
            Some(fl) => {
                let flowlet_timeout = self.flowlet_end_timeout(&reply_rtt);
                Some(fl.end_time + flowlet_timeout - now) // the value will be >= 0, 0 means timeout already happened
            }
        };
        return fl_timeout;
    }

    /// Record the packet interval.
    /// The interval is used to determine the new flowlet.
    /// The interval is smoothed by the last value.
    fn record_packet_interval(&mut self, forward_ts: time::Instant) {
        let interval = forward_ts - self.last_packet_time;
        self.last_packet_time = forward_ts;
        self.smoothed_interval = self.smoothed_interval.mul_f64(0.875) + interval.mul_f64(0.125); // 1/8
        trace!("smoothed interval: {:?}", self.smoothed_interval);
    }

    /// Return the flowlet timeout. Which is the time gap to decide whether to create a new flowlet.
    pub fn flowlet_timeout(&self, _side_rtt: &time::Duration) -> time::Duration {
        self.smoothed_interval.mul_f64(self.flowlet_interval_factor)
    }

    fn flowlet_end_timeout(&self, reply_rtt: &time::Duration) -> time::Duration {
        *reply_rtt
            + self
                .flowlet_timeout(reply_rtt)
                .mul_f64(self.flowlet_end_factor)
    }

    // If RTT deviation is detected, reset.
    // Delete all flowlets that have found a reply; only leave flowlets that have no reply yet.
    pub fn reset_due_to_rtt_deviation(&mut self) {
        while let Some(front) = self.flowlets.front() {
            if front.reply_pkt_times.is_empty() {
                break; // stop when encounter a flowlet without reply
            }
            let fl = self.flowlets.pop_front().unwrap(); // otherwise pop the front flowlet (which already has reply)
            info!("reset due to RTT deviation, remove flowlet: {:?}", fl);
            for pkt_num in fl.pkt_nums {
                debug!("reset due to RTT deviation, remove pkt num: {}", pkt_num);
                // remove the packets in the flowlet from the packet queue
                let pkt = self.packets.pop_front().unwrap();
                assert_eq!(pkt.number(), pkt_num);
            }
        }
    }

    // Measure the eliciting threshold.
    // Called every WHEN_MEASURE_ELICITING_THRESHOLD packets.
    // Continuous measurement is necessary because:
    // 1.Even in one-directional transfering applications, there can still be bursts of bidirectional packets in certain phases (e.g., the initial request phase).
    // 2.When the latency is large, the arrival of reply packets can be significantly delayed.
    fn measure_eliciting_threshold(&mut self) {
        if self.reply_nums < (self.processed as f64 * THRESHOLD_FOR_1_ELICITING_THRESHOLD) as u64 {
            self.eliciting_threshold = 2;
        } else {
            self.eliciting_threshold = 1;
        }

        debug!(
            "🎯 Eliciting threshold measured: {} (client pkts num: {}, ratio: {})",
            self.eliciting_threshold,
            self.reply_nums,
            (self.reply_nums) as f64 / self.processed as f64
        );
    }

    /// Add a packet to the queue.
    /// If the packet is a retransmit packet, the payload MUST be None.
    /// If the packet is a raw packet, the payload MUST be Some(payload).
    /// Return the packet number and whether new flowlet is created.
    pub fn add(
        &mut self,
        forward_ts: time::Instant,
        payload: Option<Vec<u8>>, // None for retransmit packet
        side_rtt: time::Duration,
        _client_queue: bool, // only print the client queue operation
    ) -> (u64, bool) {
        // record the packet interval
        if payload.is_some() {
            self.record_packet_interval(forward_ts);
        }

        // new packet number
        self.processed += 1;
        if self.processed % WHEN_MEASURE_ELICITING_THRESHOLD == 0 {
            self.measure_eliciting_threshold();
        }
        // the packets in the queue should not be too many
        if self.packets.len() > 1000 {
            error!("packet queue: {:?}", self);
            panic!("too many packets in the queue");
        }

        let mut new_flowlet = false;
        // add to flowlet
        match self.newest_ts() {
            None => {
                // no any flowlet(thus no any packet), create the first flowlet
                let fl = Flowlet::new(self.processed, forward_ts);
                new_flowlet = true;
                self.flowlets.push_back(fl);
                if _client_queue {
                    // output the flowlet add operation
                    debug!("new flowlet: {:?}", self.flowlets.back().unwrap());
                }
            }
            Some(newest_time) => {
                if forward_ts.duration_since(newest_time) <= self.flowlet_timeout(&side_rtt) {
                    // packet of the existing flowlet
                    let fl = self.flowlets.back_mut().unwrap();
                    fl.add(self.processed, forward_ts); // add to the last flowlet
                    if _client_queue {
                        // output the flowlet add operation
                        debug!("add to flowlet: {:?}", fl);
                    }
                } else {
                    // new flowlet come
                    let fl = Flowlet::new(self.processed, forward_ts);
                    new_flowlet = true;
                    self.flowlets.push_back(fl);
                    if _client_queue {
                        // output the flowlet add operation
                        debug!("new flowlet: {:?}", self.flowlets.back().unwrap());
                    }
                }
            }
        }

        // save the packet
        match payload {
            // retransmit packet
            None => {
                self.packets.push_back(Packet::Retrans(RetransUdpPacket {
                    number: self.processed,
                    timestamp: forward_ts,
                }));
            }
            // raw packet
            Some(payload) => {
                self.packets.push_back(Packet::Raw(RawUdpPacket {
                    number: self.processed,
                    timestamp: forward_ts,
                    payload,
                }));
            }
        }
        (self.processed, new_flowlet)
    }

    /// Complete flowlets[0]. But based on the new algorithms: DP based mapping; considering the eliciting threshold.
    /// Must be called after the flowlet is checked as complete.
    /// Return: rtt samples from the completed flowlet.
    fn complete_one_flowlet(&mut self, reply_rtt: time::Duration) -> Vec<time::Duration> {
        assert!(self.flowlets[0].is_complete());
        let fl = &self.flowlets[0];

        // check if all packets in the flowlet have been replied
        let mut rtt_samples;
        let lossed_pkts: BTreeSet<u64>;

        if fl.pkt_nums.len() < fl.reply_pkt_times.len() {
            // more replies than sent packets: no loss, but not use rtt samples
            debug!("more replies than sent packets in flowlet: {:?}", fl);
            lossed_pkts = BTreeSet::new();
            rtt_samples = Vec::new();
        } else if fl.exactly_replyed() {
            // no loss
            debug!("no flowlet pkt lossed");
            lossed_pkts = BTreeSet::new();

            // all sent packets are replied, get rtt samples
            rtt_samples = Vec::new();
            for i in 0..fl.pkt_nums.len() {
                let reply_time = fl.reply_pkt_times[i];
                let pkt_num = fl.pkt_nums[i];
                let sample = reply_time - self.get_packet(pkt_num).timestamp();
                rtt_samples.push(sample);
            }
        } else if fl.reply_pkt_times.is_empty() {
            // no reply. all loss
            lossed_pkts = fl.pkt_nums.iter().copied().collect();
            info!("all flowlet lossed, packets: {:?}", lossed_pkts);
            self.print_lossed_pkts(&lossed_pkts);
            rtt_samples = Vec::new(); // no rtt samples
        } else {
            // process partly replied flowlet
            let sent_pkt_times: Vec<time::Instant> = fl
                .pkt_nums
                .iter()
                .map(|pkt_num| self.get_packet(*pkt_num).timestamp())
                .collect();
            let sent_to_reply_map = fl.match_sent_part_reply(&sent_pkt_times, reply_rtt);
            assert_eq!(fl.pkt_nums.len(), sent_to_reply_map.len());
            lossed_pkts = fl.extract_part_loss(
                &sent_pkt_times,
                &sent_to_reply_map,
                self.eliciting_threshold,
            );
            rtt_samples = fl.extract_rtt_samples(&sent_to_reply_map, &sent_pkt_times);
        }

        self.remove_a_complete_flowlet(lossed_pkts);
        rtt_samples
    }

    /// Find reply packet for flowlet.
    /// now is the time when the reply packet comes.
    /// pkt_num is the packet number(number in peer queue, not this queue) of the reply packet.
    /// If return None, means the reply packet is not inserted into the flowlet.
    pub fn check_reply(
        &mut self,
        now: time::Instant,
        pkt_num: u64,
        reply_rtt: time::Duration,
    ) -> Option<Vec<time::Duration>> {
        // 1.< begin_time1 - 1/2 flow_let timeout: error
        // 2.else if only 1 flowlet, push to flowlet1
        // 3.else: find the most suitable flowlet, push to it, and label all the previous flowlets as complete
        self.reply_nums += 1;
        let flowlet_timeout_addition = self.flowlet_end_timeout(&reply_rtt) - reply_rtt;
        let flowlets_len = self.flowlets.len();
        trace!("check reply, pkt: {}", pkt_num);
        if flowlets_len == 0 {
            trace!("no flowlet for reply check");
            return None;
        } else if now < self.flowlets[0].begin_time + reply_rtt - flowlet_timeout_addition
            && self.flowlets[0].reply_pkt_nums.is_empty()
        // if not, the earlier reply packet be seen as not too early, so we not see this one as too early
        {
            trace!("reply packet too early");
            return None;
        } else if now
            > self.flowlets[flowlets_len - 1].end_time + reply_rtt + flowlet_timeout_addition
        {
            trace!("reply packet too late");
            return None;
        } else if flowlets_len == 1 {
            // push to flowlet1
            self.flowlets[0].add_reply(now, pkt_num);
            // mark the flowlet as complete if there are too many packets
            if self.flowlets[0].pkt_nums.len() > FLOWLET_MAX_PKTS {
                self.flowlets[0].set_as_complete(); // avoid protect flowlet longer than FLOWLET_MAX_PKTS
            }
            // output the flowlet reply operation
            trace!("reply to flowlet1: {:?}", self.flowlets[0]);
        } else {
            // more than 1 flowlet: search the suitable flowlet, push to it, and label all the previous flowlets as complete
            let mut replyed_flowlet = 0;
            let mut min_match_error = time::Duration::MAX;
            for i in 0..flowlets_len {
                let fl = &self.flowlets[i];
                // find exact match
                if fl.begin_time + reply_rtt <= now && now <= fl.end_time + reply_rtt {
                    replyed_flowlet = i;
                    break;
                }

                // find the flowlet with the smallest match error
                let match_error = if now > fl.end_time + reply_rtt {
                    now - fl.end_time - reply_rtt
                } else {
                    fl.begin_time + reply_rtt - now
                };
                if match_error < min_match_error {
                    min_match_error = match_error;
                    replyed_flowlet = i;
                }
            }
            // push to the replyed flowlet
            self.flowlets[replyed_flowlet].add_reply(now, pkt_num);
            // output the flowlet reply operation
            trace!(
                "reply to flowlet{}: {:?}",
                replyed_flowlet + 1,
                self.flowlets[replyed_flowlet]
            );
            // label all the previous flowlets as complete
            for i in 0..replyed_flowlet {
                self.flowlets[i].set_as_complete();
            }
        }

        // if some flowlets are complete, remove it and return the RTT sample
        let mut rtt_samples = Vec::new();
        while !self.flowlets.is_empty() && self.flowlets[0].is_complete() {
            rtt_samples.append(&mut self.complete_one_flowlet(reply_rtt));
        }
        Some(rtt_samples)
    }

    pub fn on_timeout(
        &mut self,
        now: time::Instant,
        reply_rtt: time::Duration,
    ) -> Vec<time::Duration> {
        debug!("PacketQueue: check timeout at {:?}", now);
        // check if any flowlets are timeout
        let flowlet_timeout = self.flowlet_end_timeout(&reply_rtt);
        let mut rtt_samples = Vec::new();
        // complete the timeout flowlets
        loop {
            let end_time1 = match self.flowlets.len() {
                0 => {
                    debug!("no flowlet for timeout set");
                    break;
                }
                _ => self.flowlets[0].end_time,
            };
            if now > end_time1 + flowlet_timeout {
                // timeout
                self.flowlets[0].set_as_complete();
            } else {
                break;
            }

            // check if there is any packet loss
            rtt_samples.append(&mut self.complete_one_flowlet(reply_rtt));
        }
        rtt_samples
    }

    /// use first 8 bytes + last 8 bytes of UDP payload as packet id
    /// return hex string
    pub fn packet_id(pkt: &Vec<u8>) -> String {
        let mut id = String::new();
        for i in 0..8 {
            id.push_str(&format!("{:02x}", pkt[i]));
        }
        for i in 0..8 {
            id.push_str(&format!("{:02x}", pkt[pkt.len() - 8 + i]));
        }
        id
    }

    fn print_lossed_pkts(&self, lossed_pkts: &BTreeSet<u64>) {
        if log::log_enabled!(log::Level::Debug) {
            for pkt_num in lossed_pkts {
                let pkt = self.get_packet(*pkt_num);
                match pkt {
                    Packet::Raw(p) => {
                        debug!("lossed pkt: {}", Self::packet_id(&p.payload()));
                    }
                    Packet::Retrans(_) => {
                        debug!("loss a retransmit pkt");
                    }
                }
            }
        }
    }

    /// Remove the complete flowlet.
    /// Calling this function will remove the complete flowlet from the queue.
    /// This must be called after the flowlet is unuseful: after the loss check, RTT measurement.
    fn remove_a_complete_flowlet(&mut self, lossed_pkts: BTreeSet<u64>) {
        assert!(self.flowlets[0].is_complete());
        // Remove the completed flowlet1 and its pkts from the queue.
        debug!("remove flowlet: {:?}", self.flowlets[0]);
        if log::log_enabled!(log::Level::Info) {
            // print id of all packets in the flowlet
            let mut pkt_ids = String::new();
            for pkt_num in &self.flowlets[0].pkt_nums {
                let pkt = self.get_packet(*pkt_num);
                match pkt {
                    Packet::Raw(p) => {
                        pkt_ids.push_str(&Self::packet_id(&p.payload()));
                    }
                    Packet::Retrans(_) => {
                        pkt_ids.push_str("retransmit");
                    }
                }
                pkt_ids.push_str(", ");
            }
            debug!("flowlet pkts: {}", pkt_ids);
        }
        let fl = self.flowlets.pop_front().unwrap();
        let mut reply_index = 0;
        for pkt_num in fl.pkt_nums {
            // remove the packet from the queue
            // the packet number in flowlet must be the same as the packet number in the `PacketQueue.packets UdpPacket.number`.
            // if the packet is lossed, push it to the detected loss queue
            let pkt = self.packets.pop_front().unwrap();
            assert_eq!(pkt.number(), pkt_num);
            if lossed_pkts.contains(&pkt_num) {
                match pkt {
                    Packet::Raw(pkt) => {
                        self.detected_loss.push_back(pkt);
                    }
                    Packet::Retrans(_) => {
                        debug!("retransmit packet need not to be retransmitted again");
                    }
                }
            } else {
                // an unlossed packet, push its reply to the replys.
                // (the reply containing mechanism has been deprecated)
                if reply_index >= fl.reply_pkt_nums.len() {
                    debug!("there are interval close/eliciting batch processed packets");
                } else {
                    reply_index += 1;
                }
            }
        }
    }

    pub fn pop_retransmit_front(&mut self) -> Option<RawUdpPacket> {
        self.detected_loss.pop_front()
    }

    pub fn have_retransmit(&self) -> bool {
        !self.detected_loss.is_empty()
    }

    /// Return the timestamp of the oldest packet.
    pub fn oldest_ts(&self) -> Option<time::Instant> {
        self.packets.front().map(|p| p.timestamp())
    }

    /// Return the timestamp of the newest packet in the queue.
    fn newest_ts(&self) -> Option<time::Instant> {
        self.packets.back().map(|p| p.timestamp())
    }

    /// Return the packet of packet number `num`.
    fn get_packet(&self, num: u64) -> &Packet {
        // if not exist, panic
        if num < self.packets.front().unwrap().number()
            || num > self.packets.back().unwrap().number()
        {
            panic!("packet number {} not exist in the queue", num);
        }

        let off = num - self.packets.front().unwrap().number();

        &self.packets[off as usize]
    }

    /// Return ref of payload of the packet number `num`.
    pub fn get_packet_payload(&self, num: u64) -> &Vec<u8> {
        match self.get_packet(num) {
            Packet::Raw(p) => p.payload(),
            Packet::Retrans(_) => panic!("retransmit packet has no payload"),
        }
    }
}

impl std::fmt::Display for PacketQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "PacketQueue {{ packets: {}, flowlets: {}, processed: {} }}, flowlets: {:?}",
            self.packets.len(),
            self.flowlets.len(),
            self.processed,
            self.flowlets,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_flowlet() {
        let now = time::Instant::now();
        let mut fl = Flowlet::new(1, now);
        assert_eq!(fl.pkt_nums.len(), 1);
        assert_eq!(fl.pkt_nums[0], 1);
        assert_eq!(fl.reply_pkt_times.len(), 0);
        assert_eq!(fl.begin_time, now);
        assert_eq!(fl.end_time, now);
        assert_eq!(fl.is_complete(), false);

        fl.add(2, now);
        assert_eq!(fl.pkt_nums.len(), 2);
        assert_eq!(fl.pkt_nums[1], 2);
        assert_eq!(fl.reply_pkt_times.len(), 0);
        assert_eq!(fl.begin_time, now);
        assert_eq!(fl.end_time, now);

        fl.add_reply(now, 1);
        assert_eq!(fl.reply_pkt_times.len(), 1);
        assert_eq!(fl.exactly_replyed(), false);

        fl.add_reply(now, 2);
        assert_eq!(fl.reply_pkt_times.len(), 2);
        assert_eq!(fl.exactly_replyed(), true);
    }

    // exmpale from real test when handshake lost
    #[test]
    fn reply_to_right_flowlet() {
        // add packets
        let mut pq = PacketQueue::new();

        // RTT = 949.855501ms, this is too big rtt is because the packet lost
        let rtt = Duration::from_micros(949_856);

        // packets:
        // 1. data: 50.246223ms f00000000114fb6d58d157197c287b00
        // 2. reply: 1.000101724s ca000000011046eb0000000000000000
        // 3. data: 1.050441748s f00000000114fb6d58d157197c287b00
        // 4. reply: 1.052968357s cb000000011487820000000000000000

        let start_time = time::Instant::now();
        let pkt1_time = start_time + Duration::from_secs_f64(50.246223e-3);
        let reply1_time = start_time + Duration::from_secs_f64(1.000101724);
        let pkt2_time = start_time + Duration::from_secs_f64(1.050441748);
        let reply2_time = start_time + Duration::from_secs_f64(1.052968357);

        let pkt1_payload = hex::decode("f00000000114fb6d58d157197c287b00").unwrap();
        let pkt2_payload = hex::decode("f00000000114fb6d58d157197c287b00").unwrap();

        pq.add(pkt1_time, Some(pkt1_payload), rtt, true);
        assert_eq!(pq.flowlets.len(), 1);
        assert_eq!(pq.flowlets[0].pkt_nums, vec![1]);

        pq.check_reply(reply1_time, 1, rtt);
        assert_eq!(pq.flowlets.len(), 1);
        assert_eq!(pq.flowlets[0].reply_pkt_times.len(), 1);
        assert_eq!(pq.flowlets[0].is_complete(), false);

        pq.add(pkt2_time, Some(pkt2_payload), rtt, true);
        assert_eq!(pq.flowlets.len(), 2);
        assert_eq!(pq.flowlets[1].pkt_nums, vec![2]);

        pq.check_reply(reply2_time, 2, rtt);
    }

    #[test]
    fn test_match_sent_reply_dp_all_examples() {
        // create a dummy instance of your struct
        let fl = Flowlet::new(1, time::Instant::now());

        // Helper to call the method
        let run = |sent: &[f64], reply: &[f64], rtt: f64| {
            fl.match_sent_reply_dp(sent.to_vec(), reply.to_vec(), rtt)
        };

        // ===== Example 1 =====
        let sent1 = [0.0, 50.0, 100.0, 150.0, 200.0, 250.0];
        let reply1 = [30.0, 98.0, 200.0, 298.0, 352.0];
        let rtt1 = 100.0;
        let expected1 = vec![0, 1, 2, usize::MAX, 3, 4];
        let map1 = run(&sent1, &reply1, rtt1);
        assert_eq!(map1, expected1, "DP Example 1 failed");

        // ===== Example 2 =====
        let sent2 = [0.0, 40.0, 80.0, 120.0, 160.0, 200.0, 240.0];
        let reply2 = [18.0, 121.0, 205.0, 330.0, 400.0];
        let rtt2 = 120.0;
        let expected2 = vec![0, 1, 2, usize::MAX, usize::MAX, 3, 4];
        let map2 = run(&sent2, &reply2, rtt2);
        assert_eq!(map2, expected2, "DP Example 2 failed");

        // ===== Example 3 =====
        let sent3 = [0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0];
        let reply3 = [20.0, 30.0, 51.0, 71.0];
        let rtt3 = 20.0;
        let expected3 = vec![0, 1, usize::MAX, 2, usize::MAX, 3, usize::MAX];
        let map3 = run(&sent3, &reply3, rtt3);
        assert_eq!(map3, expected3, "DP Example 3 failed");

        // ===== Example 4 (also labeled example 3 in Python) =====
        let sent4 = [1.0, 2.0, 3.0, 4.0];
        let reply4 = [2.0, 4.0];
        let rtt4 = 0.0;
        let expected4 = vec![usize::MAX, 0, usize::MAX, 1];
        let map4 = run(&sent4, &reply4, rtt4);
        assert_eq!(map4, expected4, "DP Example 4 failed");
    }
}
