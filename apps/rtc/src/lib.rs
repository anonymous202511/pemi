use std::time;

use log::debug;

pub const HTTP_REQ_STREAM_ID: u64 = 0;
pub const MAX_DATAGRAM_SIZE: usize = 1350;

pub fn print_bytes(bytes: usize) -> String {
    let units = ["B", "KB", "MB", "GB", "TB", "PB", "EB", "ZB", "YB"];
    let mut unit = 0;
    let mut bytes = bytes as f64;
    while bytes >= 1000.0 {
        bytes /= 1000.0;
        unit += 1;
    }
    format!("{:.2} {}", bytes, units[unit])
}

pub struct ClientStats {
    /// Total number of QUIC payload bytes recved.
    bytes_recv: usize,

    /// Time when send the request.
    start_time: Option<time::Instant>,

    /// Interval to print the stats.
    print_interval: time::Duration,

    /// Interval bytes recved.
    interval_recv: usize,

    /// Past seconds.
    past_sec: u64,

    /// Last print time.
    last_print_time: time::Instant,
}
impl ClientStats {
    pub fn new() -> Self {
        let interval_as_sec = 1;
        let print_interval = time::Duration::from_secs(interval_as_sec);
        Self {
            bytes_recv: 0,
            start_time: None,
            print_interval,
            interval_recv: 0,
            past_sec: 0,
            last_print_time: time::Instant::now(),
        }
    }

    pub fn request_start(&mut self) {
        self.start_time = Some(time::Instant::now());
        // Print the header.
        debug!("Interval    Transfer   Bitrate");
    }

    pub fn bytes_recv(&mut self, bytes: usize) {
        self.bytes_recv += bytes;

        self.interval_recv += bytes;

        if self.last_print_time.elapsed() >= self.print_interval {
            self.past_sec += 1;
            debug!(
                "{}-{} sec   {} MB   {} Mbits/sec",
                self.past_sec - self.print_interval.as_secs(),
                self.past_sec,
                self.interval_recv as f64 / 1_000_000.0,
                self.interval_recv as f64 / 1_000_000.0 * 8.0
                    / self.print_interval.as_secs() as f64
            );

            //refresh for next sec
            self.last_print_time = time::Instant::now();
            self.interval_recv = 0;
        }
    }

    pub fn print_stats(&self) {
        let elapsed = self.start_time.unwrap().elapsed().as_secs_f64();

        // Goodput in Mbps.
        let goodput = self.bytes_recv as f64 / elapsed / 1000.0 / 1000.0 * 8.0;
        println!(
            "Recv {} bytes in {:.3} s, goodput: {:.2} Mbps",
            print_bytes(self.bytes_recv),
            elapsed,
            goodput
        );
    }
}

/// use first 8 bytes + last 8 bytes of UDP payload as packet id
/// return hex string
pub fn packet_id(pkt: &[u8]) -> String {
    let mut id = String::new();
    for i in 0..8 {
        id.push_str(&format!("{:02x}", pkt[i]));
    }
    for i in 0..8 {
        id.push_str(&format!("{:02x}", pkt[pkt.len() - 8 + i]));
    }
    id
}

pub struct Stats {
    /// Total number of QUIC payload bytes sent.
    bytes_sent: usize,

    /// Time when recv the client request.
    start_time: Option<time::Instant>,
}
impl Stats {
    pub fn new() -> Self {
        Self {
            bytes_sent: 0,
            start_time: None,
        }
    }

    pub fn request_recved(&mut self) {
        self.start_time = Some(time::Instant::now());
    }

    pub fn bytes_sent(&mut self, bytes: usize) {
        self.bytes_sent += bytes;
    }

    pub fn print_stats(&self) {
        let elapsed = self.start_time.unwrap().elapsed().as_secs_f64();

        // Goodput in Mbps.
        let goodput = self.bytes_sent as f64 / elapsed / 1000.0 / 1000.0 * 8.0;
        println!(
            "Sent {} in {:.3} seconds, goodput: {:.2} Mbps",
            print_bytes(self.bytes_sent),
            elapsed,
            goodput
        );
    }
}

#[derive(Debug, Clone)]
pub struct PeerTime {
    start_time: time::SystemTime,
}
impl PeerTime {
    pub fn new(start_time: &f64) -> Self {
        Self {
            start_time: time::SystemTime::UNIX_EPOCH
                + std::time::Duration::from_secs_f64(*start_time),
        }
    }

    pub fn elapsed(&self) -> time::Duration {
        let now = time::SystemTime::now();
        now.duration_since(self.start_time).unwrap()
    }
}
