//! Capped exponential backoff with jitter, for every loop in the fleet that
//! dials something that may be down.
//!
//! There were three of these — the agent's reconnect, the desktop's event
//! stream, the hub's supervised `ssh -R` tunnels — and only the agent's had
//! jitter, so the other two made a fleet restarted together retry in
//! lockstep. The curve is shared here and the numbers stay with the caller:
//! [`Backoff::new`] takes the base and the cap.
//!
//! Pure: no clock, no sleep, no tokio. The caller supplies the jitter draw
//! (usually [`jitter`]) and does its own waiting, which is also what lets a
//! test drive the sequence without one.

use std::time::Duration;

/// Capped exponential backoff: `base`, `base * 2`, `base * 4`, … up to `cap`,
/// each step jittered down into its own upper half.
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    cap: Duration,
    attempt: u32,
}

impl Backoff {
    /// The first delay is `base` (jittered) and no delay exceeds `cap`.
    pub fn new(base: Duration, cap: Duration) -> Self {
        Self {
            base,
            cap,
            attempt: 0,
        }
    }

    /// The next delay. `jitter` in `[0, 1)` picks a point in the upper half of
    /// the current step, so dials never bunch at zero and a fleet of clients
    /// restarted together does not dial in lockstep. A draw outside that range
    /// is clamped, and one that is not a number is read as zero —
    /// `Duration::mul_f64` panics on a NaN, and a backoff is the last place
    /// that should be able to take a process down.
    pub fn next(&mut self, jitter: f64) -> Duration {
        let step = self.step();
        self.attempt = self.attempt.saturating_add(1);
        let draw = if jitter.is_finite() {
            jitter.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let half = step / 2;
        half + half.mul_f64(draw)
    }

    /// The step [`Backoff::next`] would jitter within — the nominal cadence,
    /// for a message that tells someone how long the wait will be.
    pub fn step(&self) -> Duration {
        self.base
            .checked_mul(1u32 << self.attempt.min(SHIFT_CEILING))
            .unwrap_or(self.cap)
            .min(self.cap)
    }

    /// Start over — what a caller does after a connection that actually
    /// worked.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// Jump straight to the cap — for a refusal where retrying sooner cannot
    /// help (the agent's version mismatch: only an upgrade fixes it, and the
    /// agent should stay reachable for that without hammering a peer that has
    /// already said no). Every following delay stays at the cap too, since
    /// `attempt` only grows, until [`Backoff::reset`].
    pub fn force_max(&mut self) {
        // `next` clamps with `.min(cap)`, so this only has to be big enough:
        // the shift ceiling is as far as the curve ever goes.
        self.attempt = self.attempt.max(SHIFT_CEILING);
    }
}

/// How far `1 << attempt` is allowed to go. Past this the step is beyond any
/// cap a caller would set, and the shift itself would overflow.
const SHIFT_CEILING: u32 = 16;

/// A number in `[0, 1)` that differs between processes and between calls:
/// std's per-process random hash keys, fed a counter. No crate needed for a
/// reconnect delay — and `fleet-proto` takes no dependency for one.
pub fn jitter() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static CALLS: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(CALLS.fetch_add(1, Ordering::Relaxed));
    (h.finish() >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn it_doubles_to_its_cap_and_stays_in_the_upper_half_of_each_step() {
        let mut b = Backoff::new(secs(1), secs(60));
        // Jitter 0.0 is the floor of each step, ~1.0 its ceiling.
        for step in [1u64, 2, 4, 8, 16, 32, 60, 60, 60] {
            let mut probe = b.clone();
            assert_eq!(probe.next(0.0), secs(step) / 2, "floor of step {step}");
            let mut probe = b.clone();
            assert!(probe.next(1.0) <= secs(step), "ceiling of step {step}");
            assert_eq!(b.step(), secs(step));
            let drawn = b.next(0.5);
            assert!(
                drawn >= secs(step) / 2 && drawn <= secs(step),
                "{drawn:?} outside step {step}"
            );
        }
    }

    /// The other two callers' curve: the same shape with a 30 s ceiling.
    #[test]
    fn the_cap_is_the_callers_own() {
        let mut b = Backoff::new(secs(1), secs(30));
        let steps: Vec<Duration> = (0..10)
            .map(|_| {
                let s = b.step();
                b.next(0.0);
                s
            })
            .collect();
        assert_eq!(
            steps[..6],
            [secs(1), secs(2), secs(4), secs(8), secs(16), secs(30)]
        );
        assert!(steps.iter().all(|s| *s <= secs(30)));
    }

    #[test]
    fn a_connection_that_worked_starts_the_curve_over() {
        let mut b = Backoff::new(secs(1), secs(30));
        for _ in 0..5 {
            b.next(0.0);
        }
        assert!(b.step() > secs(1));
        b.reset();
        assert_eq!(b.step(), secs(1));
        assert_eq!(b.next(0.0), secs(1) / 2);
    }

    #[test]
    fn force_max_goes_straight_to_the_cap_and_stays_there() {
        let mut b = Backoff::new(secs(1), secs(60));
        b.force_max();
        for _ in 0..3 {
            assert_eq!(b.step(), secs(60));
            assert_eq!(b.next(0.0), secs(30));
        }
        b.reset();
        assert_eq!(b.step(), secs(1));
    }

    /// A sub-second base (what a test of a restart loop uses) still halves
    /// and doubles cleanly.
    #[test]
    fn a_small_base_behaves_the_same() {
        let mut b = Backoff::new(Duration::from_millis(20), secs(30));
        assert_eq!(b.next(0.0), Duration::from_millis(10));
        assert_eq!(b.next(0.0), Duration::from_millis(20));
        assert_eq!(b.next(1.0), Duration::from_millis(80));
    }

    #[test]
    fn jitter_is_in_range_and_varies() {
        let draws: Vec<f64> = (0..64).map(|_| jitter()).collect();
        assert!(draws.iter().all(|d| (0.0..1.0).contains(d)), "{draws:?}");
        assert!(draws.windows(2).any(|w| w[0] != w[1]), "{draws:?}");
    }

    /// Jitter outside `[0, 1]` cannot push a delay past its step.
    #[test]
    fn a_wild_jitter_draw_is_clamped() {
        let mut b = Backoff::new(secs(4), secs(60));
        assert_eq!(b.clone().next(f64::NAN), secs(2), "NaN clamps to the floor");
        assert_eq!(b.clone().next(-5.0), secs(2));
        assert_eq!(b.next(5.0), secs(4));
    }
}
