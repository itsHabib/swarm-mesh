use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct RttStats {
    pub current_rtt: Option<Duration>,
    pub avg_rtt: Duration,
    pub min_rtt: Duration,
    pub max_rtt: Duration,
    pub sample_count: u32,
}

impl RttStats {
    pub fn update(&mut self, rtt: Duration) {
        self.current_rtt = Some(rtt);
        self.sample_count += 1;

        if rtt < self.min_rtt {
            self.min_rtt = rtt;
        }
        if rtt > self.max_rtt {
            self.max_rtt = rtt;
        }

        if self.sample_count == 1 {
            self.avg_rtt = rtt;
            return;
        }

        // update average RTT using a simple exponential moving average
        const ALPHA: f64 = 0.125;
        let old = self.avg_rtt.as_millis() as f64;
        let new = rtt.as_millis() as f64;
        self.avg_rtt = Duration::from_millis((old * (1.0 - ALPHA) + new * ALPHA) as u64);
    }
}

impl Default for RttStats {
    fn default() -> Self {
        Self {
            current_rtt: None,
            avg_rtt: Duration::ZERO,
            min_rtt: Duration::MAX,
            max_rtt: Duration::ZERO,
            sample_count: 0,
        }
    }
}
