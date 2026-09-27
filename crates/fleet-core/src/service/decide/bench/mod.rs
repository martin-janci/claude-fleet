//! Offline benchmarks of the decision envelope's use cases (the Jev
//! evaluation's phase 0, `docs/superpowers/specs/2026-09-27-jev-test-map.md`
//! §2): each reads a frozen dataset from the store, asks its providers, and
//! reports the test map's metrics (§4). Nothing here runs by itself or
//! changes what fleet does; the one network path is an explicit
//! `--provider jev` run through [`super::decide`], so its gate (flag, mode,
//! org consent, key, breaker, budget) applies to every case.
//!
//! - [`work_link`]: card J1, choosing a work item for a session
//!   (`fleet-hub decide bench work-link`).

pub mod bm25;
pub mod work_link;
#[cfg(test)]
mod work_link_tests;

/// A deterministic generator (splitmix64) for the bootstrap: the same seed
/// gives the same intervals on every machine.
#[derive(Debug, Clone)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`n > 0`).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// PURE: a stable 64-bit mix of two ids (a case's candidate order).
pub fn mix(a: u64, b: u64) -> u64 {
    SplitMix64::new(a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b).next_u64()
}

/// PURE: the `p`-th percentile (0..=100, nearest rank) of `v`.
pub fn percentile(v: &[i64], p: f64) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_unstable();
    let rank = ((p / 100.0) * s.len() as f64).ceil() as usize;
    Some(s[rank.clamp(1, s.len()) - 1])
}

/// PURE: the `q` quantile (0..=1, linear) of sorted `v`.
fn quantile_sorted(v: &[f64], q: f64) -> f64 {
    let pos = q * (v.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    v[lo] + (v[hi] - v[lo]) * (pos - lo as f64)
}

/// One paired observation: did provider A / B answer, and were they right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paired {
    pub a_answered: bool,
    pub a_correct: bool,
    pub b_answered: bool,
    pub b_correct: bool,
}

fn acc(xs: impl Iterator<Item = (bool, bool)>) -> Option<f64> {
    let (mut n, mut k) = (0u64, 0u64);
    for (answered, correct) in xs {
        if answered {
            n += 1;
            if correct {
                k += 1;
            }
        }
    }
    (n > 0).then(|| k as f64 / n as f64)
}

/// PURE: accuracy on answered of A minus B over `obs`, and its bootstrap
/// 95% interval (`resamples` resamples of the cases, `seed`). A resample in
/// which either provider answered nothing is left out. `None` when the
/// point estimate itself is undefined.
pub fn bootstrap_acc_diff(obs: &[Paired], resamples: usize, seed: u64) -> Option<(f64, f64, f64)> {
    let a = acc(obs.iter().map(|o| (o.a_answered, o.a_correct)))?;
    let b = acc(obs.iter().map(|o| (o.b_answered, o.b_correct)))?;
    let mut rng = SplitMix64::new(seed);
    let mut diffs = Vec::with_capacity(resamples);
    for _ in 0..resamples {
        let (mut an, mut ak, mut bn, mut bk) = (0u64, 0u64, 0u64, 0u64);
        for _ in 0..obs.len() {
            let o = obs[rng.below(obs.len())];
            if o.a_answered {
                an += 1;
                ak += u64::from(o.a_correct);
            }
            if o.b_answered {
                bn += 1;
                bk += u64::from(o.b_correct);
            }
        }
        if an > 0 && bn > 0 {
            diffs.push(ak as f64 / an as f64 - bk as f64 / bn as f64);
        }
    }
    if diffs.is_empty() {
        return Some((a - b, a - b, a - b));
    }
    diffs.sort_by(|x, y| x.total_cmp(y));
    Some((
        a - b,
        quantile_sorted(&diffs, 0.025),
        quantile_sorted(&diffs, 0.975),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_is_deterministic() {
        let a: Vec<u64> = {
            let mut r = SplitMix64::new(7);
            (0..5).map(|_| r.next_u64()).collect()
        };
        let b: Vec<u64> = {
            let mut r = SplitMix64::new(7);
            (0..5).map(|_| r.next_u64()).collect()
        };
        assert_eq!(a, b);
        let mut r = SplitMix64::new(1);
        assert!((0..1000).all(|_| r.below(10) < 10));
    }

    #[test]
    fn percentiles_are_nearest_rank() {
        let v = [5, 1, 3, 2, 4, 10, 7, 6, 9, 8];
        assert_eq!(percentile(&v, 50.0), Some(5));
        assert_eq!(percentile(&v, 95.0), Some(10));
        assert_eq!(percentile(&[], 50.0), None);
        assert_eq!(percentile(&[3], 95.0), Some(3));
    }

    #[test]
    fn a_clear_gap_has_an_interval_that_excludes_zero() {
        // A right on 90 of 100, B on 50 of 100.
        let obs: Vec<Paired> = (0..100)
            .map(|i| Paired {
                a_answered: true,
                a_correct: i % 10 != 0,
                b_answered: true,
                b_correct: i % 2 == 0,
            })
            .collect();
        let (d, lo, hi) = bootstrap_acc_diff(&obs, 1000, 42).unwrap();
        assert!((d - 0.4).abs() < 1e-9);
        assert!(lo > 0.0 && hi > lo, "{lo} {hi}");
        // Same seed, same interval.
        assert_eq!(bootstrap_acc_diff(&obs, 1000, 42), Some((d, lo, hi)));
    }

    #[test]
    fn equal_providers_have_an_interval_around_zero() {
        let obs: Vec<Paired> = (0..60)
            .map(|i| Paired {
                a_answered: true,
                a_correct: i % 3 != 0,
                b_answered: true,
                b_correct: i % 3 != 1,
            })
            .collect();
        let (d, lo, hi) = bootstrap_acc_diff(&obs, 1000, 1).unwrap();
        assert!(d.abs() < 1e-9);
        assert!(lo < 0.0 && hi > 0.0, "{lo} {hi}");
    }

    #[test]
    fn no_answers_means_no_difference_to_estimate() {
        let obs = vec![Paired {
            a_answered: false,
            a_correct: false,
            b_answered: true,
            b_correct: true,
        }];
        assert!(bootstrap_acc_diff(&obs, 10, 1).is_none());
    }
}
