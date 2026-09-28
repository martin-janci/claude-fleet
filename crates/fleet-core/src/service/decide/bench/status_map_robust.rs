//! The `status_map` benchmark's paired diagnostics and its floor sweep —
//! evidence for decisions, never an acceptance verdict (no card registers a
//! threshold on them):
//!
//! - **Robustness** (dataset C, `--perturb`): each reported case's
//!   [`variant`] under a [`Perturbation`] — diacritics folded, a typo, a
//!   neutral emoji before every name, the board left out or shuffled — asked
//!   of the same providers, and compared with the original case
//!   ([`robustness`]).
//! - **Languages** (dataset B): rows of a label file that carry the same
//!   `pair` id are one section written in several languages, each with its
//!   board translated. [`language_pairs`] compares every language with the
//!   English case of the same pair. The built-in paired set
//!   ([`PAIRED_FIXTURE`], `--paired-fixture`) is LLM-written (D43), like the
//!   main one.
//! - **Floor sweep** (`--floor-sweep`): a model's answers at every
//!   confidence floor of [`SWEEP_FLOORS`] — coverage, accuracy and `done`
//!   precision — and, with `--split all`, the lowest floor the dev boards
//!   would choose under card J3's precision lines, reported on test
//!   ([`floor_sweep`]). The adapter's floor
//!   ([`crate::service::decide::status_map::MIN_CONFIDENCE`]) changes only
//!   with a code change and a new decision row; this is the evidence for one.
//!
//! Like the rest of the report, these hold counts and rates only.

use super::perturb::{self, compare, PairObs, PairedCompare, Perturbation};
use super::status_map::{
    board_key, is_dev_board, Outcome, Outcomes, Provider, SectionCase, ACCEPT_ACCURACY,
    ACCEPT_DONE_PRECISION, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED, CATEGORIES,
};
use crate::service::decide::status_map::{self as sm, MIN_CONFIDENCE};
use crate::service::trackers::asana::{infer_section, section_key};
use serde::Serialize;
use std::collections::BTreeMap;

/// The built-in paired set (dataset B): the same boards in en, sk, cs and de.
pub const PAIRED_FIXTURE: &str = include_str!("../../testdata/decide/status_map_paired.jsonl");
/// Where it lives in the repository.
pub const PAIRED_FIXTURE_PATH: &str =
    "crates/fleet-core/src/service/testdata/decide/status_map_paired.jsonl";
/// The language every other language of a pair is compared with.
pub const REFERENCE_LANG: &str = "en";

/// Where the built-in question files live in the repository.
pub const QUESTION_SET_DIR: &str = "crates/fleet-core/src/service/testdata/decide/questions";

/// The built-in rewordings of the adapter's question (`--question-set`, or
/// one of them by path with `--question`): drafts for the dev boards, never
/// adopted by the adapter until one passes on test and a code change makes
/// it `status_map.v2`. File name → contents.
pub const QUESTION_SET: [(&str, &str); 3] = [
    (
        "status_map.v2-position.json",
        include_str!("../../testdata/decide/questions/status_map.v2-position.json"),
    ),
    (
        "status_map.v2-multilingual.json",
        include_str!("../../testdata/decide/questions/status_map.v2-multilingual.json"),
    ),
    (
        "status_map.v2-careful-done.json",
        include_str!("../../testdata/decide/questions/status_map.v2-careful-done.json"),
    ),
];

/// One question's run, for the comparison `--question-set` prints: the
/// model providers' numbers where the rule abstains.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuestionRow {
    /// `adapter` or the file's version.
    pub question: String,
    pub provider: &'static str,
    pub cases: u64,
    pub rule_abstained_coverage: Option<f64>,
    pub rule_abstained_accuracy: Option<f64>,
    pub done_precision: Option<f64>,
    pub done_answers: u64,
    pub ece: Option<f64>,
    pub calls: u64,
    pub input_tokens: i64,
}

/// PURE: the comparison rows of one report (its model providers only).
pub fn question_rows(r: &super::status_map::Report) -> Vec<QuestionRow> {
    let question = if r.question.starts_with("file ") {
        r.question.trim_start_matches("file ").to_string()
    } else {
        "adapter".to_string()
    };
    r.metrics
        .iter()
        .filter(|m| Provider::parse(m.provider).is_some_and(Provider::is_model))
        .map(|m| QuestionRow {
            question: question.clone(),
            provider: m.provider,
            cases: m.cases,
            rule_abstained_coverage: m.rule_abstained.coverage,
            rule_abstained_accuracy: m.rule_abstained.accuracy_on_answered,
            done_precision: m.done_precision,
            done_answers: m.done_answers,
            ece: m.calibration.ece,
            calls: m.calls,
            input_tokens: m.input_tokens,
        })
        .collect()
}

/// The comparison's lines.
pub fn question_lines(rows: &[QuestionRow]) -> Vec<String> {
    let mut v = vec![
        "questions compared (where the rule abstains; iterate on dev, judge the chosen one ONCE on test):".to_string(),
        format!(
            "  {:<18} {:<5} {:>5} {:>9} {:>8} {:>14} {:>6} {:>6} {:>9}",
            "question", "", "cases", "coverage", "acc@ans", "done prec (n)", "ECE", "calls", "tokens"
        ),
    ];
    for r in rows {
        v.push(format!(
            "  {:<18} {:<5} {:>5} {:>9} {:>8} {:>14} {:>6} {:>6} {:>9}",
            r.question,
            r.provider,
            r.cases,
            f2(r.rule_abstained_coverage),
            f3(r.rule_abstained_accuracy),
            format!("{} ({})", f3(r.done_precision), r.done_answers),
            f3(r.ece),
            r.calls,
            r.input_tokens
        ));
    }
    v
}

// --- robustness (dataset C) ---------------------------------------------------------

/// PURE: `c` under `p` — its name and board changed as a person or a board
/// might have written them — when that changes what a model is sent (else
/// `None`, as when a name the probe would not keep comes out). The id is
/// `<id>.<perturbation>`, the label, language and org stay, and the keyword
/// rule is read again on the new name (what it would decide in real life).
pub fn variant(c: &SectionCase, p: Perturbation) -> Option<SectionCase> {
    let seed = |part: &str| perturb::seed_of(&[p.as_str(), part]);
    let (key, board): (String, Vec<String>) = match p {
        Perturbation::Fold => (
            perturb::fold_diacritics(&c.key),
            c.board
                .iter()
                .map(|n| perturb::fold_diacritics(n))
                .collect(),
        ),
        Perturbation::Typo => {
            let k = perturb::typo(&c.key, seed(&c.key));
            let board = c
                .board
                .iter()
                .map(|n| if *n == c.key { k.clone() } else { n.clone() })
                .collect();
            (k, board)
        }
        Perturbation::Emoji => {
            let e = |n: &str| perturb::emoji_prefix(n, seed(n));
            (e(&c.key), c.board.iter().map(|n| e(n)).collect())
        }
        Perturbation::NoBoard => (c.key.clone(), Vec::new()),
        Perturbation::ShuffleBoard => (
            c.key.clone(),
            perturb::shuffle(&c.board, seed(&board_key(c))),
        ),
        Perturbation::Code => return None,
    };
    let key = section_key(&key)?;
    let mut kept: Vec<String> = Vec::new();
    for k in board.iter().filter_map(|n| section_key(n)) {
        if !kept.contains(&k) {
            kept.push(k);
        }
    }
    if key == c.key && kept == c.board {
        return None;
    }
    Some(SectionCase {
        id: p.case_id(&c.id),
        rule: infer_section(&key),
        key,
        board: kept,
        ..c.clone()
    })
}

/// PURE: the variants of `cases` under `p`, each with the index of its
/// original.
pub fn variants(cases: &[SectionCase], p: Perturbation) -> Vec<(usize, SectionCase)> {
    cases
        .iter()
        .enumerate()
        .filter_map(|(i, c)| variant(c, p).map(|v| (i, v)))
        .collect()
}

/// One provider over the pairs, with `done` precision on both sides.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SmPaired {
    #[serde(flatten)]
    pub compare: PairedCompare,
    /// Of the reference's answers that apply as done, the share labeled so.
    pub ref_done_precision: Option<f64>,
    pub done_precision: Option<f64>,
}

/// One perturbation's comparison.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Robustness {
    pub perturbation: &'static str,
    /// The reported cases, and how many of them the perturbation changed
    /// (only those are asked and compared).
    pub cases: u64,
    pub changed: u64,
    /// Of the changed cases, where the rule's answer changed too.
    pub rule_changed: u64,
    pub providers: Vec<SmPaired>,
}

fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn done_precision(pairs: &[(&SectionCase, &Outcome)]) -> Option<f64> {
    let (mut n, mut k) = (0u64, 0u64);
    for (c, o) in pairs {
        if o.answer.as_deref().and_then(sm::applied_category) == Some("done") {
            n += 1;
            k += u64::from(sm::applied_category(c.expect) == Some("done"));
        }
    }
    (n > 0).then(|| r3(k as f64 / n as f64))
}

/// PURE: one provider's comparison of `var` against `reference` over the
/// pairs `(reference index, variant index)` both answered usably.
fn paired(p: Provider, pairs: &[(&SectionCase, &Outcome, &SectionCase, &Outcome)]) -> SmPaired {
    let usable: Vec<&(&SectionCase, &Outcome, &SectionCase, &Outcome)> = pairs
        .iter()
        .filter(|(_, a, _, b)| a.usable() && b.usable())
        .collect();
    let obs: Vec<PairObs> = usable
        .iter()
        .map(|(rc, ro, vc, vo)| PairObs {
            ref_answered: ro.answer.is_some(),
            ref_right: ro.answer.as_deref() == Some(rc.expect),
            var_answered: vo.answer.is_some(),
            var_right: vo.answer.as_deref() == Some(vc.expect),
            same: ro.answer == vo.answer,
        })
        .collect();
    let refs: Vec<(&SectionCase, &Outcome)> = usable.iter().map(|x| (x.0, x.1)).collect();
    let vars: Vec<(&SectionCase, &Outcome)> = usable.iter().map(|x| (x.2, x.3)).collect();
    SmPaired {
        compare: compare(p.as_str(), &obs, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED),
        ref_done_precision: done_precision(&refs),
        done_precision: done_precision(&vars),
    }
}

/// PURE: the comparison of the variants under `p` (`variants`, with their
/// originals' indices, and the providers' outcomes on them, `vouts`) with
/// the original cases (`cases`, `outs`), per provider but `none`.
pub fn robustness(
    p: Perturbation,
    cases: &[SectionCase],
    outs: &Outcomes,
    variants: &[(usize, SectionCase)],
    vouts: &Outcomes,
) -> Robustness {
    let providers = outs
        .keys()
        .copied()
        .filter(|p| *p != Provider::None && vouts.contains_key(p))
        .map(|prov| {
            let pairs: Vec<(&SectionCase, &Outcome, &SectionCase, &Outcome)> = variants
                .iter()
                .enumerate()
                .map(|(j, (i, v))| (&cases[*i], &outs[&prov][*i], v, &vouts[&prov][j]))
                .collect();
            paired(prov, &pairs)
        })
        .collect();
    Robustness {
        perturbation: p.as_str(),
        cases: cases.len() as u64,
        changed: variants.len() as u64,
        rule_changed: variants
            .iter()
            .filter(|(i, v)| cases[*i].rule != v.rule)
            .count() as u64,
        providers,
    }
}

// --- languages (dataset B) ----------------------------------------------------------

/// One language against the reference.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LanguagePair {
    pub lang: String,
    /// Pairs that have this language and the reference.
    pub pairs: u64,
    pub providers: Vec<SmPaired>,
}

/// Every language of a paired set against [`REFERENCE_LANG`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LanguagePairs {
    pub reference: &'static str,
    /// Distinct pair ids among the cases.
    pub pairs: u64,
    pub langs: Vec<LanguagePair>,
}

/// PURE: the language comparison of `cases` whose rows carry a `pair` id:
/// for each language but the reference, over the pairs that hold both, the
/// language's outcome against the reference's. `None` when no case has a
/// pair (or no pair has the reference). A pair with two rows of one
/// language uses the first.
pub fn language_pairs(cases: &[SectionCase], outs: &Outcomes) -> Option<LanguagePairs> {
    let mut by_pair: BTreeMap<&str, BTreeMap<&str, usize>> = BTreeMap::new();
    for (i, c) in cases.iter().enumerate() {
        if let Some(p) = c.pair.as_deref() {
            by_pair
                .entry(p)
                .or_default()
                .entry(c.lang.as_str())
                .or_insert(i);
        }
    }
    if by_pair.is_empty()
        || !by_pair
            .values()
            .any(|langs| langs.contains_key(REFERENCE_LANG))
    {
        return None;
    }
    let mut langs: Vec<&str> = by_pair
        .values()
        .flat_map(|l| l.keys().copied())
        .filter(|l| *l != REFERENCE_LANG)
        .collect();
    langs.sort_unstable();
    langs.dedup();
    let providers: Vec<Provider> = outs
        .keys()
        .copied()
        .filter(|p| *p != Provider::None)
        .collect();
    let rows = langs
        .into_iter()
        .map(|lang| {
            let idx: Vec<(usize, usize)> = by_pair
                .values()
                .filter_map(|l| Some((*l.get(REFERENCE_LANG)?, *l.get(lang)?)))
                .collect();
            LanguagePair {
                lang: lang.to_string(),
                pairs: idx.len() as u64,
                providers: providers
                    .iter()
                    .map(|&p| {
                        let o = &outs[&p];
                        let pairs: Vec<(&SectionCase, &Outcome, &SectionCase, &Outcome)> = idx
                            .iter()
                            .map(|&(r, v)| (&cases[r], &o[r], &cases[v], &o[v]))
                            .collect();
                        paired(p, &pairs)
                    })
                    .collect(),
            }
        })
        .collect();
    Some(LanguagePairs {
        reference: REFERENCE_LANG,
        pairs: by_pair.len() as u64,
        langs: rows,
    })
}

// --- the floor sweep ------------------------------------------------------------------

/// The confidence floors the sweep reports.
pub const SWEEP_FLOORS: [f64; 15] = [
    0.0, 0.30, 0.35, 0.40, 0.45, 0.50, 0.55, 0.60, 0.65, 0.70, 0.75, 0.80, 0.85, 0.90, 0.95,
];
/// A floor dev chooses must leave at least this many answers where the rule
/// abstains: fewer is noise, not a precision.
pub const SWEEP_MIN_ANSWERED: u64 = 20;

/// One floor's numbers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FloorRow {
    pub floor: f64,
    pub answered: u64,
    pub coverage: Option<f64>,
    pub accuracy_on_answered: Option<f64>,
    /// The same where the keyword rule abstains (what assist asks about).
    pub rule_abstained_answered: u64,
    pub rule_abstained_coverage: Option<f64>,
    pub rule_abstained_accuracy: Option<f64>,
    pub done_answers: u64,
    pub done_precision: Option<f64>,
    /// The adapter's floor today.
    pub current: bool,
}

/// What the dev boards would choose, and how it does on test.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DevChoice {
    pub dev_cases: u64,
    pub test_cases: u64,
    /// The lowest floor whose dev numbers meet card J3's `done` precision and
    /// accuracy lines with at least [`SWEEP_MIN_ANSWERED`] rule-abstained
    /// answers; `None` when none does.
    pub floor: Option<f64>,
    /// That floor on the test boards.
    pub test: Option<FloorRow>,
}

/// One model provider's sweep.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FloorSweep {
    pub provider: &'static str,
    /// Usable outcomes over the reported cases.
    pub cases: u64,
    /// Answers that carried no confidence (they stand at every floor).
    pub without_confidence: u64,
    pub rows: Vec<FloorRow>,
    /// With both sides reported (`--split all`).
    pub dev_choice: Option<DevChoice>,
}

/// PURE: the category `o` answers at `floor`: the model's category when its
/// confidence reaches the floor (or it gave none); `unsure`, anything else
/// and a failed call abstain.
pub fn answer_at(o: &Outcome, floor: f64) -> Option<&str> {
    if !o.usable() {
        return None;
    }
    let m = o.model.as_deref().filter(|m| CATEGORIES.contains(m))?;
    o.confidence.is_none_or(|c| c + 1e-9 >= floor).then_some(m)
}

fn ratio(k: u64, n: u64) -> Option<f64> {
    (n > 0).then(|| r3(k as f64 / n as f64))
}

/// PURE: one floor over `rows` (usable outcomes only).
pub fn floor_row(rows: &[(&SectionCase, &Outcome)], floor: f64) -> FloorRow {
    let (mut n, mut ans, mut ok) = (0u64, 0u64, 0u64);
    let (mut ra_n, mut ra_ans, mut ra_ok) = (0u64, 0u64, 0u64);
    let (mut done_n, mut done_k) = (0u64, 0u64);
    for (c, o) in rows.iter().filter(|(_, o)| o.usable()) {
        n += 1;
        let abst = c.rule.is_none();
        ra_n += u64::from(abst);
        let Some(a) = answer_at(o, floor) else {
            continue;
        };
        ans += 1;
        ok += u64::from(a == c.expect);
        if abst {
            ra_ans += 1;
            ra_ok += u64::from(a == c.expect);
        }
        if sm::applied_category(a) == Some("done") {
            done_n += 1;
            done_k += u64::from(sm::applied_category(c.expect) == Some("done"));
        }
    }
    FloorRow {
        floor,
        answered: ans,
        coverage: ratio(ans, n),
        accuracy_on_answered: ratio(ok, ans),
        rule_abstained_answered: ra_ans,
        rule_abstained_coverage: ratio(ra_ans, ra_n),
        rule_abstained_accuracy: ratio(ra_ok, ra_ans),
        done_answers: done_n,
        done_precision: ratio(done_k, done_n),
        current: (floor - MIN_CONFIDENCE).abs() < 1e-9,
    }
}

/// PURE: whether a floor's numbers meet card J3's precision lines (`done`
/// precision — when anything applies as done — and accuracy where the rule
/// abstains), with enough answers to mean it.
pub fn meets_card(r: &FloorRow) -> bool {
    r.rule_abstained_answered >= SWEEP_MIN_ANSWERED
        && r.rule_abstained_accuracy
            .is_some_and(|a| a + 1e-9 >= ACCEPT_ACCURACY)
        && r.done_precision
            .is_none_or(|d| d + 1e-9 >= ACCEPT_DONE_PRECISION)
}

/// PURE: the sweep of every model provider in `outs` over `cases`. With
/// `both_sides` (the report holds dev and test boards), the lowest floor
/// the dev boards meet the card at ([`meets_card`]) and its numbers on
/// test.
pub fn floor_sweep(cases: &[SectionCase], outs: &Outcomes, both_sides: bool) -> Vec<FloorSweep> {
    outs.iter()
        .filter(|(p, _)| p.is_model())
        .map(|(&p, o)| {
            let rows: Vec<(&SectionCase, &Outcome)> = cases.iter().zip(o.iter()).collect();
            let usable = rows.iter().filter(|(_, o)| o.usable()).count() as u64;
            let without = rows
                .iter()
                .filter(|(_, o)| o.usable() && o.model.is_some() && o.confidence.is_none())
                .count() as u64;
            let dev_choice = both_sides.then(|| {
                let (dev, test): (Vec<_>, Vec<_>) = rows
                    .iter()
                    .copied()
                    .partition(|(c, _)| is_dev_board(&board_key(c)));
                let floor = SWEEP_FLOORS
                    .iter()
                    .copied()
                    .find(|&f| meets_card(&floor_row(&dev, f)));
                DevChoice {
                    dev_cases: dev.len() as u64,
                    test_cases: test.len() as u64,
                    floor,
                    test: floor.map(|f| floor_row(&test, f)),
                }
            });
            FloorSweep {
                provider: p.as_str(),
                cases: usable,
                without_confidence: without,
                rows: SWEEP_FLOORS.iter().map(|&f| floor_row(&rows, f)).collect(),
                dev_choice,
            }
        })
        .collect()
}

// --- lines ----------------------------------------------------------------------------

fn f2(x: Option<f64>) -> String {
    x.map(|v| format!("{v:.2}")).unwrap_or_else(|| "-".into())
}

fn f3(x: Option<f64>) -> String {
    x.map(|v| format!("{v:.3}")).unwrap_or_else(|| "-".into())
}

fn paired_line(p: &SmPaired) -> String {
    format!(
        "{}  done prec {} → {}",
        p.compare.line(),
        f3(p.ref_done_precision),
        f3(p.done_precision)
    )
}

/// The robustness section's lines.
pub fn robustness_lines(rs: &[Robustness]) -> Vec<String> {
    if rs.is_empty() {
        return Vec::new();
    }
    let mut v = vec![format!(
        "robustness (dataset C; each variant against its original over the pairs both answered usably; the verdict is McNemar on who was right at p < {}, not judged under {} pairs; a diagnostic, no acceptance line):",
        perturb::ALPHA,
        perturb::PAIRED_MIN
    )];
    for r in rs {
        v.push(format!(
            "  {} — changed {} of {} cases (the rule's answer changed on {})",
            r.perturbation, r.changed, r.cases, r.rule_changed
        ));
        for p in &r.providers {
            v.push(format!("    {}", paired_line(p)));
        }
    }
    v
}

/// The language section's lines.
pub fn language_lines(l: &LanguagePairs) -> Vec<String> {
    let mut v = vec![format!(
        "languages (dataset B; {} pairs; each language against {} on the same pairs; the verdict is McNemar on who was right at p < {}, not judged under {} pairs; a diagnostic):",
        l.pairs,
        l.reference,
        perturb::ALPHA,
        perturb::PAIRED_MIN
    )];
    for lang in &l.langs {
        v.push(format!("  {} — {} pairs", lang.lang, lang.pairs));
        for p in &lang.providers {
            v.push(format!("    {}", paired_line(p)));
        }
    }
    v
}

/// The floor sweep's lines.
pub fn floor_sweep_lines(sweeps: &[FloorSweep]) -> Vec<String> {
    let mut v = Vec::new();
    for s in sweeps {
        v.push(format!(
            "floor sweep {} over {} usable cases{} (* the adapter's floor, {MIN_CONFIDENCE}):",
            s.provider,
            s.cases,
            if s.without_confidence > 0 {
                format!(
                    "; {} answers had no confidence and stand at every floor",
                    s.without_confidence
                )
            } else {
                String::new()
            }
        ));
        v.push(format!(
            "    {:>5} {:>8} {:>8} {:>8} {:>15} {:>14}",
            "floor", "answered", "coverage", "acc@ans", "rule-abst cov", "done prec (n)"
        ));
        for r in &s.rows {
            v.push(format!(
                "  {}{:>5.2} {:>8} {:>8} {:>8} {:>15} {:>14}",
                if r.current { "*" } else { " " },
                r.floor,
                r.answered,
                f2(r.coverage),
                f3(r.accuracy_on_answered),
                format!(
                    "{} ({})",
                    f2(r.rule_abstained_coverage),
                    f3(r.rule_abstained_accuracy)
                ),
                format!("{} ({})", f3(r.done_precision), r.done_answers),
            ));
        }
        match &s.dev_choice {
            None => v.push(
                "  (the dev boards' choice needs both sides: run with --split all)".into(),
            ),
            Some(d) => match (d.floor, &d.test) {
                (Some(f), Some(t)) => v.push(format!(
                    "  dev ({} cases) chooses floor {f:.2}; on test ({} cases): rule-abst cov {} acc@ans {}, done prec {} ({}){}",
                    d.dev_cases,
                    d.test_cases,
                    f2(t.rule_abstained_coverage),
                    f3(t.rule_abstained_accuracy),
                    f3(t.done_precision),
                    t.done_answers,
                    if meets_card(t) { " — meets the card's precision lines" } else { " — does NOT meet them on test" }
                )),
                _ => v.push(format!(
                    "  dev ({} cases): no floor meets done precision ≥ {ACCEPT_DONE_PRECISION} and accuracy ≥ {ACCEPT_ACCURACY} where the rule abstains with ≥ {SWEEP_MIN_ANSWERED} answers",
                    d.dev_cases
                )),
            },
        }
    }
    if !sweeps.is_empty() {
        v.push(
            "  (the adapter's floor changes only with a code change and a new decision row; this sweep is its evidence)"
                .into(),
        );
    }
    v
}
