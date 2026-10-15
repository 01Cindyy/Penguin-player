//! Caps on how often a plugin may message the app, so a plugin stuck in a loop can't
//! bury the user in notifications or flood the app with commands and errors.

use std::time::Instant;

/// A token bucket: allows bursts of up to `burst` messages, refilled at a steady
/// `per_second`. Messages beyond that are dropped, with one warning per burst.
#[derive(Debug)]
pub(crate) struct RateLimit {
    what: &'static str,
    burst: f64,
    per_second: f64,
    tokens: f64,
    updated: Instant,
    dropping: bool,
}

impl RateLimit {
    pub fn new(what: &'static str, burst: u32, per_second: u32) -> Self {
        Self {
            what,
            burst: f64::from(burst),
            per_second: f64::from(per_second),
            tokens: f64::from(burst),
            updated: Instant::now(),
            dropping: false,
        }
    }

    /// Whether one more message may be sent now. When not, a warning is logged to
    /// `log_target` the first time.
    pub fn allow(&mut self, log_target: &str) -> bool {
        self.allow_at(Instant::now(), log_target)
    }

    fn allow_at(&mut self, now: Instant, log_target: &str) -> bool {
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.per_second).min(self.burst);
        self.updated = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            self.dropping = false;
            return true;
        }
        if !self.dropping {
            self.dropping = true;
            log::warn!(
                target: log_target,
                "Too many {}: dropping them (at most {} at once, then {} per second)",
                self.what,
                self.burst,
                self.per_second
            );
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn allows_bursts_then_refills_steadily() {
        let mut limit = RateLimit::new("tests", 3, 2);
        let start = limit.updated;
        let allowed =
            |limit: &mut RateLimit, at| (0..10).filter(|_| limit.allow_at(at, "test")).count();
        assert_eq!(allowed(&mut limit, start), 3);
        assert_eq!(allowed(&mut limit, start + Duration::from_millis(250)), 0);
        assert_eq!(allowed(&mut limit, start + Duration::from_millis(500)), 1);
        assert_eq!(allowed(&mut limit, start + Duration::from_secs(60)), 3);
    }
}
