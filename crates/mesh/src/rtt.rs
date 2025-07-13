use std::time::Duration;

/// Round-trip time (RTT) statistics for network performance monitoring.
///
/// This struct tracks various RTT metrics for a peer connection, including
/// current, average, minimum, and maximum round-trip times. It uses an
/// exponential moving average to smooth out RTT fluctuations while remaining
/// responsive to network condition changes.
#[derive(Debug, Clone)]
pub struct RttStats {
    /// The most recent RTT measurement
    pub current_rtt: Option<Duration>,
    /// Exponential moving average of RTT measurements
    pub avg_rtt: Duration,
    /// Minimum RTT observed across all measurements
    pub min_rtt: Duration,
    /// Maximum RTT observed across all measurements
    pub max_rtt: Duration,
    /// Total number of RTT samples collected
    pub sample_count: u32,
}

impl RttStats {
    /// Updates the RTT statistics with a new measurement.
    ///
    /// This method incorporates a new RTT measurement into the statistics,
    /// updating the current RTT, min/max values, and the exponential moving
    /// average. The moving average uses an alpha value of 0.125 (1/8) which
    /// provides a good balance between responsiveness and smoothing.
    ///
    /// # Arguments
    /// * `rtt` - The new round-trip time measurement to incorporate
    ///
    /// # Algorithm
    /// The exponential moving average is calculated as:
    /// `new_avg = old_avg * (1 - α) + new_sample * α`
    /// where α = 0.125
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
        let old = self.avg_rtt.as_secs_f64();
        let new = rtt.as_secs_f64();
        self.avg_rtt = Duration::from_secs_f64(old * (1.0 - ALPHA) + new * ALPHA);
    }
}

impl Default for RttStats {
    /// Creates a new RttStats instance with default values.
    ///
    /// The default instance has no current RTT, zero average RTT,
    /// maximum possible minimum RTT (so any real measurement will be smaller),
    /// zero maximum RTT, and zero sample count.
    ///
    /// # Returns
    /// A new RttStats instance ready to receive measurements
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
