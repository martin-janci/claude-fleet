//! Transport-failure backoff for the dialer: 1 s doubling to 60 s, ±20 %
//! jitter, reset after one good exchange. Seeded, so tests are deterministic.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

const BASE_SECS: f64 = 1.0;
const CAP_SECS: f64 = 60.0;
const JITTER: f64 = 0.2;

pub struct Backoff {
    step: u32,
    rng: u64,
    /// The link whose wait [`retry_every`] reports (M15 step G7.14).
    link: Option<i64>,
}

/// Each dialer link's current wait between tries, in whole seconds before
/// jitter, while it is retrying (M15 step G7.14: the Federation page says
/// "retrying every 30 s"). In memory: a restart starts the backoff over.
static RETRY_EVERY: Mutex<BTreeMap<i64, u64>> = Mutex::new(BTreeMap::new());

/// How often `link_id` is being tried again, when its dialer is backing off.
pub fn retry_every(link_id: i64) -> Option<u64> {
    RETRY_EVERY.lock().ok()?.get(&link_id).copied()
}

fn note(link: Option<i64>, secs: Option<u64>) {
    let (Some(link), Ok(mut m)) = (link, RETRY_EVERY.lock()) else {
        return;
    };
    match secs {
        Some(s) => m.insert(link, s),
        None => m.remove(&link),
    };
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
            link: None,
        }
    }

    /// A backoff whose wait [`retry_every`] reports for `link_id`.
    pub fn for_link(link_id: i64) -> Self {
        Self {
            link: Some(link_id),
            ..Self::new()
        }
    }

    pub fn reset(&mut self) {
        self.step = 0;
        note(self.link, None);
    }

    // Named `next` per the task-5 brief's public interface (`Backoff::next`),
    // not `std::iter::Iterator` — it returns a bare `Duration`, not `Option`.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Duration {
        let base = (BASE_SECS * 2f64.powi(self.step.min(16) as i32)).min(CAP_SECS);
        self.step = self.step.saturating_add(1);
        note(self.link, Some(base.round() as u64));
        // xorshift64: no dependency for a jitter source.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        let unit = (self.rng % 10_000) as f64 / 10_000.0; // [0, 1)
        Duration::from_secs_f64(base * (1.0 - JITTER + 2.0 * JITTER * unit))
    }
}

impl Drop for Backoff {
    fn drop(&mut self) {
        note(self.link, None);
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
    fn a_links_backoff_says_how_often_it_retries_until_it_resets() {
        let link = -7_001;
        let mut b = Backoff::for_link(link);
        assert_eq!(retry_every(link), None, "not retrying yet");
        for _ in 0..5 {
            b.next();
        }
        assert_eq!(retry_every(link), Some(16), "the wait before jitter");
        for _ in 0..5 {
            b.next();
        }
        assert_eq!(retry_every(link), Some(60), "capped at a minute");
        b.reset();
        assert_eq!(retry_every(link), None, "a good exchange ends it");
        b.next();
        drop(b);
        assert_eq!(retry_every(link), None, "a stopped dialer says nothing");
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
