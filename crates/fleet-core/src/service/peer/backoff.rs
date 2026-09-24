//! Transport-failure backoff for the dialer: 1 s doubling to 60 s, ±20 %
//! jitter, reset after one good exchange. Seeded, so tests are deterministic.

use std::time::Duration;

const BASE_SECS: f64 = 1.0;
const CAP_SECS: f64 = 60.0;
const JITTER: f64 = 0.2;

pub struct Backoff {
    step: u32,
    rng: u64,
}

impl Backoff {
    pub fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        Self::with_seed(seed)
    }

    fn with_seed(seed: u64) -> Self {
        Self {
            step: 0,
            rng: seed | 1,
        }
    }

    pub fn reset(&mut self) {
        self.step = 0;
    }

    // Named `next` per the task-5 brief's public interface (`Backoff::next`),
    // not `std::iter::Iterator` — it returns a bare `Duration`, not `Option`.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Duration {
        let base = (BASE_SECS * 2f64.powi(self.step.min(16) as i32)).min(CAP_SECS);
        self.step = self.step.saturating_add(1);
        // xorshift64: no dependency for a jitter source.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        let unit = (self.rng % 10_000) as f64 / 10_000.0; // [0, 1)
        Duration::from_secs_f64(base * (1.0 - JITTER + 2.0 * JITTER * unit))
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

/// A refusal ends the link (`refused` / `incompatible`); anything else retries.
pub fn is_terminal(code: &str) -> bool {
    matches!(code, "E_FORBIDDEN" | "E_UNAUTHORIZED" | "E_UNSUPPORTED")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_to_a_cap_within_jitter_and_resets() {
        let mut b = Backoff::with_seed(7);
        let mut last = 0.0;
        for i in 0..10 {
            let d = b.next().as_secs_f64();
            let base = (1u64 << i).min(60) as f64;
            assert!(
                d >= base * 0.8 && d <= base * 1.2,
                "step {i}: {d} vs {base}"
            );
            last = d;
        }
        assert!(last <= 72.0);
        b.reset();
        assert!(b.next().as_secs_f64() <= 1.2);
    }

    #[test]
    fn refusals_are_terminal_and_everything_else_retries() {
        for c in ["E_FORBIDDEN", "E_UNAUTHORIZED", "E_UNSUPPORTED"] {
            assert!(is_terminal(c), "{c}");
        }
        for c in ["E_INTERNAL", "E_TIMEOUT", "E_HUB_UNREACHABLE", "E_VALIDATE"] {
            assert!(!is_terminal(c), "{c}");
        }
    }
}
