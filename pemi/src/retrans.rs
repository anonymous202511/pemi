use crate::queue::PacketQueue;
use crate::queue::RawUdpPacket;
use std::collections::VecDeque;
use std::net::SocketAddr;

pub struct Task {
    src: SocketAddr,
    dst: SocketAddr,
    retrans_queue: VecDeque<RawUdpPacket>,
}

impl Task {
    pub fn from_queue(
        pkt_queue: &mut PacketQueue,
        src: SocketAddr,
        dst: SocketAddr,
        direction_protect: bool,
        overspeed: bool,
    ) -> Option<Self> {
        if !pkt_queue.have_retransmit() {
            return None;
        }
        let mut retrans_queue = VecDeque::new();
        while let Some(p) = pkt_queue.pop_retransmit_front() {
            retrans_queue.push_back(p);
        }
        // not help retrans if direction protect is off
        if !direction_protect || overspeed {
            None
        } else {
            Some(Task {
                src,
                dst,
                retrans_queue,
            })
        }
    }

    pub fn src(&self) -> &SocketAddr {
        &self.src
    }

    pub fn dst(&self) -> &SocketAddr {
        &self.dst
    }

    pub fn pop_front(&mut self) -> Option<RawUdpPacket> {
        self.retrans_queue.pop_front()
    }
}

impl std::fmt::Display for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut q = String::new();
        for p in &self.retrans_queue {
            q.push_str(&format!("{}, ", p));
        }
        write!(
            f,
            "Task: {{ src: {} -> dst: {}, pkts: [{}]}}",
            self.src, self.dst, q
        )
    }
}
