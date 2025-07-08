use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RttStats {
    pub current_rtt: Option<Duration>,
    pub avg_rtt: Duration,
    pub min_rtt: Duration,
    pub max_rtt: Duration,
    pub sample_count: u32,
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
