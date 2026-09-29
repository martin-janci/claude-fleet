//! The `work_link` benchmark's robustness pass (dataset C, `work-link
//! --perturb`): each asked case's first prompt folded (no diacritics), with
//! a typo, or with its fenced code blocks replaced by a placeholder
//! (decision D42's A/B), asked of the same providers and compared with the
//! original at each provider's own operating point — its raw pick, before
//! any threshold chosen on dev (a threshold would be chosen again on the
//! variants, and the pair would compare two thresholds). A "none of these"
//! case is right when the provider abstains. A diagnostic: no card
//! registers a threshold on it. Holds counts and rates only.

use super::perturb::{self, compare, PairObs, PairedCompare, Perturbation};
use super::work_link::{
    BenchCase, BenchOptions, Dataset, Loaded, Outcome, Outcomes, Provider, BOOTSTRAP_RESAMPLES,
    BOOTSTRAP_SEED, SHOW_MIN_CASES,
};
use crate::service::nl::census::Shown;
use serde::Serialize;

/// PURE: `c` with its state (the redacted first prompt) under `p`, when
/// that changes it. The id is `<id>.<perturbation>`; the candidates, the
/// truth, the org and the language cells stay the original's.
pub fn variant(c: &BenchCase, p: Perturbation) -> Option<BenchCase> {
    let state = match p {
        Perturbation::Fold => perturb::fold_diacritics(&c.state),
        Perturbation::Typo => perturb::typo(&c.state, perturb::seed_of(&[p.as_str(), &c.id])),
        Perturbation::Code => perturb::code_placeholder(&c.state),
        Perturbation::Emoji | Perturbation::NoBoard | Perturbation::ShuffleBoard => return None,
    };
    (state != c.state).then(|| BenchCase {
        id: p.case_id(&c.id),
        state,
        ..c.clone()
    })
}

/// PURE: the cases the model providers would be asked in `opts` (the
/// reported side of A, and H), each one's variant under `p`, as a
/// [`Loaded`] the providers run on unchanged.
pub fn variants(loaded: &Loaded, opts: &BenchOptions, p: Perturbation) -> Loaded {
    Loaded {
        cases: loaded
            .cases
            .iter()
            .filter(|c| c.dataset == Dataset::H || opts.split.keeps(c.dev))
            .filter_map(|c| variant(c, p))
            .collect(),
        ..loaded.clone()
    }
}

/// One perturbation's comparison.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WlRobustness {
    pub perturbation: &'static str,
    /// The cases the providers were asked, and how many the perturbation
    /// changed (under 5 shows as `<5`).
    pub cases: Shown,
    pub changed: Shown,
    /// Providers with at least [`super::work_link::SHOW_MIN_CASES`] pairs
    /// (fewer are left out, like every J1 count under 5).
    pub providers: Vec<PairedCompare>,
}

fn obs(c: &BenchCase, o: &Outcome, v: &Outcome) -> PairObs {
    let right = |x: &Outcome| x.pick == c.truth;
    PairObs {
        ref_answered: o.pick.is_some(),
        ref_right: right(o),
        var_answered: v.pick.is_some(),
        var_right: right(v),
        same: o.pick == v.pick,
    }
}

/// PURE: the variants of `p` (`vloaded`, their outcomes `vouts`) against
/// their originals (`loaded`, `outs`), per provider but `none`, over the
/// pairs whose outcomes are both usable.
pub fn robustness(
    p: Perturbation,
    loaded: &Loaded,
    opts: &BenchOptions,
    outs: &Outcomes,
    vloaded: &Loaded,
    vouts: &Outcomes,
) -> WlRobustness {
    let asked = loaded
        .cases
        .iter()
        .filter(|c| c.dataset == Dataset::H || opts.split.keeps(c.dev))
        .count() as u64;
    let providers = outs
        .keys()
        .copied()
        .filter(|p| *p != Provider::None && vouts.contains_key(p))
        .map(|prov| {
            let (o, vo) = (&outs[&prov], &vouts[&prov]);
            let pairs: Vec<PairObs> = vloaded
                .cases
                .iter()
                .filter_map(|v| {
                    let id = v.id.strip_suffix(&format!(".{}", p.as_str()))?;
                    let (a, b) = (o.get(id)?, vo.get(&v.id)?);
                    (a.usable() && b.usable()).then(|| obs(v, a, b))
                })
                .collect();
            compare(prov.as_str(), &pairs, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED)
        })
        .filter(|c| c.pairs >= SHOW_MIN_CASES)
        .collect();
    WlRobustness {
        perturbation: p.as_str(),
        cases: Shown(asked),
        changed: Shown(vloaded.cases.len() as u64),
        providers,
    }
}

/// The robustness section's lines.
pub fn robustness_lines(rs: &[WlRobustness]) -> Vec<String> {
    if rs.is_empty() {
        return Vec::new();
    }
    let mut v = vec![format!(
        "robustness (dataset C; each variant against its original at the provider's raw pick, before any dev threshold; the verdict is McNemar on who was right at p < {}, not judged under {} pairs; a diagnostic):",
        perturb::ALPHA,
        perturb::PAIRED_MIN
    )];
    for r in rs {
        v.push(format!(
            "  {} — changed {} of {} cases asked",
            r.perturbation, r.changed, r.cases
        ));
        for p in &r.providers {
            v.push(format!("    {}", p.line()));
        }
    }
    v
}
