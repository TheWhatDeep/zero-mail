//! Retry timing (section 6, Retry and throttling): 2 s doubling to a 5 minute cap with
//! plus or minus 20% jitter, reset after a connection stays healthy for 60 s.

use std::time::Duration;

pub const START: Duration = Duration::from_secs(2);
pub const CAP: Duration = Duration::from_secs(5 * 60);
pub const JITTER: f64 = 0.2;
/// A connection healthy this long resets the backoff.
pub const HEALTHY: Duration = Duration::from_secs(60);
/// A throttle response pauses the account this long, fixed.
pub const THROTTLE_PAUSE: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self { base: START }
    }
}

impl Backoff {
    /// The next delay. `jitter` is in `[-1, 1]` and scales the 20% band; the base then
    /// doubles, up to the cap.
    pub fn next_delay(&mut self, jitter: f64) -> Duration {
        let j = jitter.clamp(-1.0, 1.0) * JITTER;
        let delay = self.base.mul_f64(1.0 + j);
        self.base = (self.base * 2).min(CAP);
        delay
    }

    pub fn reset(&mut self) {
        self.base = START;
    }

    /// The base the next delay is drawn around.
    pub fn base(&self) -> Duration {
        self.base
    }
}

/// A uniform value in `[-1, 1]`.
pub fn jitter() -> f64 {
    rand::random::<f64>() * 2.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_from_two_seconds_to_the_cap() {
        let mut b = Backoff::default();
        let bases: Vec<u64> = (0..10).map(|_| b.next_delay(0.0).as_secs()).collect();
        assert_eq!(bases, vec![2, 4, 8, 16, 32, 64, 128, 256, 300, 300]);
        b.reset();
        assert_eq!(b.next_delay(0.0), START);
    }

    #[test]
    fn jitter_stays_within_twenty_percent() {
        let mut b = Backoff::default();
        assert_eq!(b.next_delay(1.0), Duration::from_millis(2_400));
        assert_eq!(b.next_delay(-1.0), Duration::from_millis(3_200));
        assert_eq!(b.next_delay(5.0), Duration::from_millis(9_600), "clamped");
        for _ in 0..1_000 {
            let j = jitter();
            assert!((-1.0..=1.0).contains(&j));
        }
    }
}
