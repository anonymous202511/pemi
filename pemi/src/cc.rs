use log::{debug, trace};

use crate::minmax::Minmax;
use std::{collections::VecDeque, time};

const MIN_RTT_WINDOW: time::Duration = time::Duration::from_secs(10);
const MIN_STANDING_WINDOW: time::Duration = time::Duration::from_millis(10);
const V_MAX: f64 = 32.0; // Maximum velocity

pub struct Copa {
    rtt_min_filter: Minmax<time::Duration>, // Minimum RTT seen in 10 seconds
    rtt_standing_filter: Minmax<time::Duration>, // Minimum RTT seen in smoothedRTT/2
    delta_reciprocal: f64,                  // 1/δ. δ=0.5 on default
    cwnd: f64,                              // Congestion window. In bytes
    cwnd_change: time::Instant,             // Time of the last cwnd change
    v: f64,                                 // velocity parameter
    direction: Direction,                   // Direction of the velocity
    direction_change: time::Instant,        // Time of the last direction change
    cwnd_last_direction_change: f64,        // cwnd at the last direction change
    slow_start: bool,                       // slow start
    cwnd_used: UsedWindow,                  // Used window
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Direction {
    Up,
    Down,
}

struct UsedWindow {
    packet_record: VecDeque<time::Instant>,
}

impl UsedWindow {
    fn new() -> Self {
        UsedWindow {
            packet_record: VecDeque::new(),
        }
    }

    /// Record the time of the packet sent.
    /// Remove the packets that are sent before the window.
    /// Return the number of packets sent in the window.
    fn on_data_send(&mut self, win: time::Duration, now: time::Instant) -> usize {
        self.packet_record.push_back(now);
        while *self.packet_record.front().unwrap() < now - win {
            self.packet_record.pop_front();
        }
        return self.packet_record.len();
    }
}

impl Copa {
    pub fn new(now: time::Instant) -> Self {
        Copa {
            rtt_min_filter: Minmax::new(time::Duration::MAX),
            rtt_standing_filter: Minmax::new(time::Duration::MAX),
            delta_reciprocal: 2.0,
            cwnd: 10.0,
            cwnd_change: now,
            v: 1.0,
            direction: Direction::Up,
            direction_change: now,
            cwnd_last_direction_change: 10.0,
            slow_start: true,
            cwnd_used: UsedWindow::new(),
        }
    }

    // Compute the recent sending rate and compare it with the target rate
    // Return: overspeed or not
    pub fn on_data_send(&mut self, now: time::Instant, client_rtt: time::Duration) -> bool {
        let recent_sent = self.cwnd_used.on_data_send(client_rtt, now);
        let rtt_min = self
            .rtt_min_filter
            .running_min(MIN_RTT_WINDOW, now, client_rtt);
        let rate_target = self.cwnd / rtt_min.as_secs_f64();
        let rate_recent = recent_sent as f64 / client_rtt.as_secs_f64();
        if rate_recent > rate_target {
            debug!(
                "Copa:need down the speed, rate_target: {}, rate_recent: {}",
                rate_target, rate_recent
            );
            true
        } else {
            false
        }
    }

    // Update cwnd, v, direction
    pub fn on_ack_send(&mut self, client_rtt: time::Duration, now: time::Instant) {
        // Minimum RTT seen in 10 seconds
        let rtt_min = self
            .rtt_min_filter
            .running_min(MIN_RTT_WINDOW, now, client_rtt);
        // Minimum RTT seen in smoothedRTT/2
        let mut standing_window = client_rtt / 2;
        if standing_window < MIN_STANDING_WINDOW {
            standing_window = MIN_STANDING_WINDOW;
        }
        let rtt_standing = self
            .rtt_standing_filter
            .running_min(standing_window, now, client_rtt);
        let dq = rtt_standing - rtt_min; // dq = RTTstanding - RTTmin
        let lambda_t = self.delta_reciprocal as f64 / dq.as_secs_f64(); // λt = 1/(δ*dq)
        let lambda = self.cwnd / rtt_standing.as_secs_f64(); // λ = cwnd/RTTstanding
        trace!("Copa:rtt_min: {:?}", rtt_min);
        trace!("Copa:rtt_standing: {:?}", rtt_standing);
        trace!("Copa:dq: {:?}", dq);
        trace!("Copa:λ_t: {}", lambda_t);
        trace!("Copa:λ: {}", lambda);

        // update cwnd
        if self.slow_start {
            self.cwnd_update_slow_start(lambda <= lambda_t, now, client_rtt);
        } else {
            self.cwnd_update(lambda <= lambda_t, now, client_rtt);
        }
        trace!("Copa:cwnd: {}", self.cwnd);

        // update v
        let delta_time = now - self.direction_change;
        if delta_time > client_rtt {
            let last_direction = self.direction;
            if self.cwnd >= self.cwnd_last_direction_change {
                self.direction = Direction::Up;
            } else {
                self.direction = Direction::Down;
            }
            if self.direction == last_direction {
                self.v = (self.v * 2.0).min(V_MAX);
            } else {
                self.v = 1.0; // reset v
                debug!(
                    "Copa:direction change detected, new direction: {:?}",
                    self.direction
                );
            }
            self.cwnd_last_direction_change = self.cwnd;
            self.direction_change = now;
        }
    }

    // Update cwnd in congestion avoidance phase
    fn cwnd_update(&mut self, up: bool, now: time::Instant, client_rtt: time::Duration) {
        let t_delta = now - self.cwnd_change; // time since last cwnd change
        self.cwnd_change = now;
        let cwnd_delta =
            self.v * self.delta_reciprocal * t_delta.as_secs_f64() / client_rtt.as_secs_f64();
        // default: self.v * 2.0 * t_delta/srtt
        trace!("Copa:v: {}", self.v);
        trace!("Copa:delta time: {}", t_delta.as_secs_f64());
        if up {
            trace!("Copa:add cwnd by {} ", cwnd_delta);
            // If λ <= λt, increase cwnd: cwnd = cwnd +  t_delta/srtt*v/δ
            self.cwnd += cwnd_delta;
        } else {
            trace!("Copa:sub cwnd by {} ", cwnd_delta);
            // If λ > λt, decrease cwnd: cwnd = cwnd -  t_delta/srtt*v/δ
            self.cwnd -= cwnd_delta;
            self.cwnd = self.cwnd.max(10.0); // cwnd = max(10, cwnd)
        }
    }

    // Update cwnd in slow start phase
    fn cwnd_update_slow_start(&mut self, up: bool, now: time::Instant, client_rtt: time::Duration) {
        if up {
            // If λ <= λt, double cwnd: cwnd = cwnd * 2
            let t_delta = now - self.cwnd_change;
            debug!(
                "Copa:time delta for slow start MI: {}",
                t_delta.as_secs_f64() / client_rtt.as_secs_f64()
            );
            self.cwnd *= 1.0 + (t_delta.as_secs_f64() / client_rtt.as_secs_f64()).min(1.0); // cwnd = cwnd * (1 + min(1, Δt/RTT))
            self.cwnd_change = now;
        } else {
            // If λ > λt, half cwnd: cwnd = cwnd / 2
            self.cwnd /= 2.0;
            self.slow_start = false;
            debug!("Copa:slow start end");
        }
    }

    // when calibration_rtt_sample >> measured RTT, reset rtt_min and rtt_standing to avoid the min RTT being erroneously small
    pub fn reset_rtt_filters(&mut self) {
        self.rtt_min_filter = Minmax::new(time::Duration::MAX);
        self.rtt_standing_filter = Minmax::new(time::Duration::MAX);
    }
}
