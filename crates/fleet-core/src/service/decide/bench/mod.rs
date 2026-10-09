//! Offline benchmarks of the decision envelope's use cases (the Jev
//! evaluation's phase 0, `docs/superpowers/specs/2026-09-27-jev-test-map.md`
//! §2): each reads a frozen dataset from the store, asks its providers, and
//! reports the test map's metrics (§4). Nothing here runs by itself or
//! changes what fleet does; the one network path is an explicit
//! `--provider jev` run through [`super::decide`], so its gate (flag, org
//! consent, key, breaker, budget) applies to every case. The feature's live
//! mode is not needed ([`super::gate_bench_at`]): measuring `status_map`
//! must not start its daily live runs. A benchmark's runs (subject kind
//! [`crate::store::DECISION_BENCH_SUBJECT`]) never count toward the live
//! breaker, budget or stats.
//!
//! - [`work_link`]: card J1, choosing a work item for a session
//!   (`fleet-hub decide bench work-link`).
//! - [`status_map`]: card J3, an Asana section's status category
//!   (`fleet-hub decide bench status-map`).
//! - [`turn_outcome`]: card J2, what a silent turn's end came to, from
//!   labeled pane tails (`fleet-hub decide bench turn-outcome`).
//! - [`choice`]: one labelled set per closed-choice use case — K2
//!   control_route, K4 duplicate, N1 related_session, K5 work_placement,
//!   N5 host_placement, N6 routine_run_outcome, N4 adopt_target, J10
//!   restore_target, J6 main_ticket, J7 tracker_duplicate
//!   (`fleet-hub decide bench <use-case>`). The built-in sets are synthetic.
//!
//! - [`perturb`]: perturbed variants (dataset C) and the paired comparison
//!   datasets B and C are judged by; [`status_map_robust`] applies them to
//!   J3, with its floor sweep, and [`work_link_robust`] to J1.
//!
//! Shared here: the bootstrap on differences, percentiles, calibration (ECE
//! and Brier, test map §4) and the acceptance verdicts both benches print.

pub mod bm25;
pub mod choice;
#[cfg(test)]
mod choice_tests;
pub mod perturb;
pub mod status_map;
pub mod status_map_robust;
#[cfg(test)]
mod status_map_robust_tests;
#[cfg(test)]
mod status_map_tests;
pub mod turn_outcome;
pub mod work_link;
pub mod work_link_robust;
#[cfg(test)]
mod work_link_tests;

use crate::service::nl::census::{Shown, SUPPRESS_BELOW};
use serde::{Deserialize, Serialize};

/// Which cases a benchmark reports: its `dev` cases, its `test` cases or
/// `all`. Each bench says what makes a case dev (J1: the oldest share by
/// decision time; J3: its board's hash).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    Dev,
    Test,
    All,
}

impl Split {
    pub fn parse(s: &str) -> Option<Split> {
        match s {
            "dev" => Some(Split::Dev),
            "test" => Some(Split::Test),
            "all" => Some(Split::All),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Split::Dev => "dev",
            Split::Test => "test",
            Split::All => "all",
        }
    }

    /// Whether a case in the dev split (`dev`) or not is reported.
    pub fn keeps(self, dev: bool) -> bool {
        match self {
            Split::Dev => dev,
            Split::Test => !dev,
            Split::All => true,
        }
    }
}

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

// --- calibration (test map §4) ---------------------------------------------------

/// Equal-width bins of the expected calibration error.
pub const ECE_BINS: usize = 10;

fn unit_clamp(c: f64) -> f64 {
    if c.is_finite() {
        c.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// PURE: the expected calibration error of `obs` — each a stated
/// confidence in 0..=1 and whether the answer was right — over `bins`
/// equal-width bins of the confidence (`[0, 0.1)`, …, `[0.9, 1]`): the sum
/// over bins of (the bin's share of answers) × |its accuracy − its mean
/// confidence|. `None` without observations (or bins). A confidence
/// outside 0..=1 is clamped.
pub fn ece(obs: &[(f64, bool)], bins: usize) -> Option<f64> {
    if obs.is_empty() || bins == 0 {
        return None;
    }
    let mut n = vec![0u64; bins];
    let mut conf = vec![0f64; bins];
    let mut right = vec![0u64; bins];
    for &(c, ok) in obs {
        let c = unit_clamp(c);
        let b = ((c * bins as f64).floor() as usize).min(bins - 1);
        n[b] += 1;
        conf[b] += c;
        right[b] += u64::from(ok);
    }
    let total = obs.len() as f64;
    Some(
        (0..bins)
            .filter(|&b| n[b] > 0)
            .map(|b| {
                let k = n[b] as f64;
                (k / total) * (right[b] as f64 / k - conf[b] / k).abs()
            })
            .sum(),
    )
}

/// PURE: the Brier score of `obs` (the confidence of the given answer
/// against whether it was right): the mean of (confidence − 1 if right,
/// else 0)². 0 is perfect; always saying 0.5 scores 0.25. `None` without
/// observations.
pub fn brier(obs: &[(f64, bool)]) -> Option<f64> {
    if obs.is_empty() {
        return None;
    }
    let sum: f64 = obs
        .iter()
        .map(|&(c, ok)| {
            let y = if ok { 1.0 } else { 0.0 };
            (unit_clamp(c) - y).powi(2)
        })
        .sum();
    Some(sum / obs.len() as f64)
}

/// Calibration of the answers that carried a confidence.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Calibration {
    /// Answers with a confidence.
    pub n: Shown,
    /// Expected calibration error over [`ECE_BINS`] equal-width bins.
    pub ece: Option<f64>,
    pub brier: Option<f64>,
}

impl Calibration {
    /// PURE: [`ece`] and [`brier`] of `obs`, rounded to 3 places. Under
    /// [`SUPPRESS_BELOW`] observations neither is shown (the count shows
    /// as `<5`).
    pub fn of(obs: &[(f64, bool)]) -> Calibration {
        let n = obs.len() as u64;
        let shown = n >= SUPPRESS_BELOW;
        Calibration {
            n: Shown(n),
            ece: ece(obs, ECE_BINS).filter(|_| shown).map(round3),
            brier: brier(obs).filter(|_| shown).map(round3),
        }
    }

    /// `ECE 0.041 Brier 0.120 (n 212)`, or `-` without answers.
    pub fn line(&self) -> String {
        if self.n.0 == 0 {
            return "-".into();
        }
        let f = |x: Option<f64>| x.map(|v| format!("{v:.3}")).unwrap_or_else(|| "-".into());
        format!("ECE {} Brier {} (n {})", f(self.ece), f(self.brier), self.n)
    }
}

// --- acceptance ------------------------------------------------------------------

/// A registered threshold, judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail,
    /// Too few cases (test map §3), or what it compares against is not
    /// built or was not run.
    NotJudged,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::NotJudged => "NOT JUDGED",
        }
    }

    /// PURE: `value ≥ threshold`, or not judged when `value` is unknown or
    /// `judged` is false.
    pub fn at_least(value: Option<f64>, threshold: f64, judged: bool) -> Verdict {
        match value {
            Some(v) if judged => {
                if v + 1e-9 >= threshold {
                    Verdict::Pass
                } else {
                    Verdict::Fail
                }
            }
            _ => Verdict::NotJudged,
        }
    }

    /// PURE: every one of `vs` passes → pass; any fails → fail; else not
    /// judged.
    pub fn all(vs: &[Verdict]) -> Verdict {
        if vs.contains(&Verdict::Fail) {
            Verdict::Fail
        } else if !vs.is_empty() && vs.iter().all(|v| *v == Verdict::Pass) {
            Verdict::Pass
        } else {
            Verdict::NotJudged
        }
    }
}

/// One line of a card's acceptance: the registered criterion, what was
/// measured, and the verdict.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Criterion {
    pub criterion: String,
    pub measured: String,
    pub verdict: Verdict,
}

impl Criterion {
    pub fn new(
        criterion: impl Into<String>,
        measured: impl Into<String>,
        verdict: Verdict,
    ) -> Self {
        Criterion {
            criterion: criterion.into(),
            measured: measured.into(),
            verdict,
        }
    }

    pub fn line(&self) -> String {
        format!(
            "  {:<10} {} — {}",
            self.verdict.as_str(),
            self.criterion,
            self.measured
        )
    }
}

/// PURE: `x` to three places, `-` for none.
pub fn f3(x: Option<f64>) -> String {
    x.map(|v| format!("{v:.3}")).unwrap_or_else(|| "-".into())
}

/// PURE: `x` to two places, `-` for none.
pub(crate) fn f2(x: Option<f64>) -> String {
    x.map(|v| format!("{v:.2}")).unwrap_or_else(|| "-".into())
}

/// PURE: `x` rounded to three places.
pub(crate) fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// PURE: `k / n` to three places, none when `n` is 0.
pub(crate) fn pct(k: u64, n: u64) -> Option<f64> {
    (n > 0).then(|| round3(k as f64 / n as f64))
}

/// The skip reason of a case past a run's `max_calls`.
pub(crate) const MAX_CALLS: &str = "max_calls";

/// Before a bench call: the gate for `org_id` (its fallback's name when it
/// refuses — nothing is sent or recorded), then the `max_calls` budget.
/// `Ok` means the call may be made; the caller counts it.
pub(crate) fn gate_or_skip(
    ctx: &crate::service::decide::DecideCtx,
    feature: crate::service::decide::Feature,
    org_id: Option<i64>,
    calls: usize,
    max_calls: usize,
) -> Result<(), &'static str> {
    let gated = match crate::ipc_error::lock(&ctx.store) {
        Ok(s) => crate::service::decide::gate_bench_at(&s, feature, org_id, ctx.now()),
        Err(_) => Err(crate::service::decide::Fallback::FlagOff),
    };
    gated.map_err(|f| f.as_str())?;
    if calls >= max_calls {
        return Err(MAX_CALLS);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_metric_and_format_helpers() {
        assert_eq!(pct(1, 3), Some(0.333));
        assert_eq!(pct(2, 3), Some(0.667));
        assert_eq!(pct(0, 0), None);
        assert_eq!(pct(0, 4), Some(0.0));
        assert_eq!(round3(0.12345), 0.123);
        assert_eq!(round3(-0.0006), -0.001);
        assert_eq!(f2(None), "-");
        assert_eq!(f2(Some(0.126)), "0.13");
        assert_eq!(f2(Some(1.0)), "1.00");
        assert_eq!(f3(None), "-");
        assert_eq!(f3(Some(1.0 / 3.0)), "0.333");
    }

    #[test]
    fn ece_of_a_calibrated_model_is_zero_and_of_an_overconfident_one_is_the_gap() {
        // 10 answers at 0.8, 8 right: calibrated.
        let cal: Vec<(f64, bool)> = (0..10).map(|i| (0.8, i < 8)).collect();
        assert!(ece(&cal, 10).unwrap().abs() < 1e-12);
        // 10 answers at 0.9, 6 right: |0.6 − 0.9| = 0.3.
        let over: Vec<(f64, bool)> = (0..10).map(|i| (0.9, i < 6)).collect();
        assert!((ece(&over, 10).unwrap() - 0.3).abs() < 1e-12);
        // Two bins weighted by their share: 4 at 0.25 all wrong (gap 0.25),
        // 4 at 0.95 all right (gap 0.05) → 0.5·0.25 + 0.5·0.05 = 0.15.
        let two: Vec<(f64, bool)> = (0..8)
            .map(|i| if i < 4 { (0.25, false) } else { (0.95, true) })
            .collect();
        assert!((ece(&two, 10).unwrap() - 0.15).abs() < 1e-12);
        // 1.0 falls in the last bin, 0.0 in the first.
        assert!(ece(&[(1.0, true), (0.0, false)], 10).unwrap().abs() < 1e-12);
        assert_eq!(ece(&[], 10), None);
        assert_eq!(ece(&[(0.5, true)], 0), None);
    }

    #[test]
    fn brier_is_the_mean_squared_gap() {
        assert!(brier(&[(1.0, true), (0.0, false)]).unwrap().abs() < 1e-12);
        assert!((brier(&[(0.5, true), (0.5, false)]).unwrap() - 0.25).abs() < 1e-12);
        // (0.8 − 1)² = 0.04, (0.6 − 0)² = 0.36 → 0.2.
        assert!((brier(&[(0.8, true), (0.6, false)]).unwrap() - 0.2).abs() < 1e-12);
        assert_eq!(brier(&[]), None);
    }

    #[test]
    fn calibration_hides_its_numbers_under_five_answers() {
        let few = Calibration::of(&[(0.9, true), (0.9, false)]);
        assert_eq!(few.n.0, 2);
        assert_eq!((few.ece, few.brier), (None, None));
        assert_eq!(few.line(), "ECE - Brier - (n <5)");
        let many: Vec<(f64, bool)> = (0..10).map(|i| (0.9, i < 6)).collect();
        let c = Calibration::of(&many);
        assert_eq!(c.ece, Some(0.3));
        // 6 × 0.01 + 4 × 0.81 = 3.3 over 10.
        assert_eq!(c.brier, Some(0.33));
        assert_eq!(Calibration::of(&[]).line(), "-");
    }

    #[test]
    fn verdicts() {
        assert_eq!(Verdict::at_least(Some(0.97), 0.97, true), Verdict::Pass);
        assert_eq!(Verdict::at_least(Some(0.96), 0.97, true), Verdict::Fail);
        assert_eq!(
            Verdict::at_least(Some(0.99), 0.97, false),
            Verdict::NotJudged
        );
        assert_eq!(Verdict::at_least(None, 0.97, true), Verdict::NotJudged);
        assert_eq!(Verdict::all(&[Verdict::Pass, Verdict::Pass]), Verdict::Pass);
        assert_eq!(
            Verdict::all(&[Verdict::Pass, Verdict::NotJudged]),
            Verdict::NotJudged
        );
        assert_eq!(
            Verdict::all(&[Verdict::NotJudged, Verdict::Fail]),
            Verdict::Fail
        );
        assert_eq!(Verdict::all(&[]), Verdict::NotJudged);
        let c = Criterion::new("done precision ≥ 0.97", "0.980 over 50", Verdict::Pass);
        assert!(c.line().contains("PASS") && c.line().contains("0.980"));
    }

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
