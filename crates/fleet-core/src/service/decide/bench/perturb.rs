//! Perturbed variants of a benchmark's cases (the test map's dataset C) and
//! the paired comparison both dataset B (the same case in another language)
//! and dataset C are judged by.
//!
//! A perturbation is **deterministic**: its choices come from a
//! [`SplitMix64`] seeded by the SHA-256 of the text it changes, so a case's
//! variant is the same on every run and machine, and a rerun asks the model
//! the same thing. A variant exists only when the perturbation changed what
//! a model would be sent: folding an English name changes nothing, and the
//! pair would only measure the model against itself.
//!
//! The comparison ([`compare`]) pairs each variant with its reference — the
//! original case (C) or the English case of the same pair (B) — over the
//! cases where both outcomes are usable, and reports each side's accuracy
//! on answered and coverage, how often the answer changed, McNemar's exact
//! test on who was right ([`mcnemar_exact`]; the verdict, since a lost
//! answer is as real a loss as a wrong one) and the bootstrap interval of
//! the accuracy-on-answered difference (beside it). Under [`PAIRED_MIN`] pairs it is not
//! judged (test map §3: a paired design needs about a third of the 200
//! unpaired cases). These are diagnostics: no card registers a threshold on
//! them, so they never change an acceptance verdict.

use super::{bootstrap_acc_diff, Paired, SplitMix64};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Fewer usable pairs than this are not judged (test map §3).
pub const PAIRED_MIN: u64 = 60;
/// McNemar's p-value under which a paired difference is called.
pub const ALPHA: f64 = 0.05;

/// How a case is perturbed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Perturbation {
    /// Diacritics removed (`hotové` → `hotove`, `ß` → `ss`): a keyboard
    /// without them. Case kept.
    Fold,
    /// One typo in one word of four letters or more: two inner letters
    /// swapped, one dropped, or one doubled.
    Typo,
    /// A neutral emoji before every section name of the board (J3).
    Emoji,
    /// The board left out: the section's name alone (J3). Measures how
    /// much the answer leans on the position.
    NoBoard,
    /// The board's sections in another order (J3). Position is a strong
    /// signal; this shows what is left without it.
    ShuffleBoard,
    /// Every fenced code block replaced by `[code: <lang>, N lines]` (J1,
    /// decision D42's A/B).
    Code,
}

impl Perturbation {
    pub const ALL: [Perturbation; 6] = [
        Perturbation::Fold,
        Perturbation::Typo,
        Perturbation::Emoji,
        Perturbation::NoBoard,
        Perturbation::ShuffleBoard,
        Perturbation::Code,
    ];
    /// What `status-map --perturb` takes.
    pub const STATUS_MAP: [Perturbation; 5] = [
        Perturbation::Fold,
        Perturbation::Typo,
        Perturbation::Emoji,
        Perturbation::NoBoard,
        Perturbation::ShuffleBoard,
    ];
    /// What `work-link --perturb` takes.
    pub const WORK_LINK: [Perturbation; 3] =
        [Perturbation::Fold, Perturbation::Typo, Perturbation::Code];

    pub fn as_str(self) -> &'static str {
        match self {
            Perturbation::Fold => "fold",
            Perturbation::Typo => "typo",
            Perturbation::Emoji => "emoji",
            Perturbation::NoBoard => "no-board",
            Perturbation::ShuffleBoard => "shuffle-board",
            Perturbation::Code => "code",
        }
    }

    pub fn parse(s: &str) -> Option<Perturbation> {
        Perturbation::ALL.into_iter().find(|p| p.as_str() == s)
    }

    /// PURE: the variant's case id: `<id>.<perturbation>` (a
    /// `decision_runs` word).
    pub fn case_id(self, id: &str) -> String {
        format!("{id}.{}", self.as_str())
    }
}

/// PURE: a seed from `parts` (the first 8 bytes of the SHA-256 of them,
/// joined with `\n`), the same on every machine and Rust version.
pub fn seed_of(parts: &[&str]) -> u64 {
    let d = Sha256::digest(parts.join("\n").as_bytes());
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[..8]);
    u64::from_be_bytes(b)
}

/// PURE: `s` without its diacritics, in its own case (`Hotové` →
/// `Hotove`, `Straße` → `Strasse`): the letters [`crate::service::nl::fold_char`]
/// folds; everything else unchanged.
pub fn fold_diacritics(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let mut f = String::new();
        crate::service::nl::fold_char(c, &mut f);
        if f.chars().eq(c.to_lowercase()) {
            out.push(c);
        } else if c.is_uppercase() {
            out.push_str(&f.to_uppercase());
        } else {
            out.push_str(&f);
        }
    }
    out
}

/// PURE: one typo in one word of `s` (a word, split on single spaces, of at
/// least four letters and nothing else): two inner letters swapped, an
/// inner letter dropped, or a letter doubled — the first letter never
/// moves, so the word stays recognisable. `s` unchanged when no word
/// qualifies.
pub fn typo(s: &str, seed: u64) -> String {
    let mut words: Vec<String> = s.split(' ').map(str::to_string).collect();
    let eligible: Vec<usize> = words
        .iter()
        .enumerate()
        .filter(|(_, w)| w.chars().count() >= 4 && w.chars().all(char::is_alphabetic))
        .map(|(i, _)| i)
        .collect();
    if eligible.is_empty() {
        return s.to_string();
    }
    let mut rng = SplitMix64::new(seed);
    let at = eligible[rng.below(eligible.len())];
    let mut chars: Vec<char> = words[at].chars().collect();
    let n = chars.len();
    match rng.below(3) {
        0 => {
            // Swap two neighbours after the first letter (a different pair
            // when they are the same letter, else a drop).
            let i = 1 + rng.below(n - 2);
            if chars[i] != chars[i + 1] {
                chars.swap(i, i + 1);
            } else {
                chars.remove(i);
            }
        }
        1 => {
            chars.remove(1 + rng.below(n - 2));
        }
        _ => {
            let i = 1 + rng.below(n - 1);
            chars.insert(i, chars[i]);
        }
    }
    words[at] = chars.into_iter().collect();
    words.join(" ")
}

/// Emoji a board may carry before its names, chosen for meaning nothing
/// about a status (no ✅, 🚧 or ❌).
pub const NEUTRAL_EMOJI: [&str; 6] = ["📌", "🔹", "⭐", "🟣", "📁", "🌀"];

/// PURE: `s` with a neutral emoji and a space before it, the emoji chosen
/// by `seed`.
pub fn emoji_prefix(s: &str, seed: u64) -> String {
    let mut rng = SplitMix64::new(seed);
    format!("{} {s}", NEUTRAL_EMOJI[rng.below(NEUTRAL_EMOJI.len())])
}

/// PURE: `v` in another order (Fisher–Yates under `seed`); never the same
/// order when `v` has two or more different elements (rotated by one when
/// the shuffle happened to keep it).
pub fn shuffle<T: Clone + PartialEq>(v: &[T], seed: u64) -> Vec<T> {
    let mut out = v.to_vec();
    let mut rng = SplitMix64::new(seed);
    for i in (1..out.len()).rev() {
        let j = rng.below(i + 1);
        out.swap(i, j);
    }
    if out == v && v.len() > 1 {
        out.rotate_left(1);
    }
    out
}

/// PURE: `s` with every fenced code block (```` ``` ```` … ```` ``` ````, an
/// unclosed one to the end) replaced by `[code: <lang>, N lines]` — the
/// fence's language word (lower case) or `unknown`, and the block's line
/// count. Inline code is kept (decision D42's placeholder A/B).
pub fn code_placeholder(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find("```") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 3..];
        let (info, body_start) = match after.find('\n') {
            Some(nl) => (&after[..nl], nl + 1),
            None => (after, after.len()),
        };
        let lang = info
            .split_whitespace()
            .next()
            .map(str::to_lowercase)
            .filter(|w| {
                w.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '#' | '-' | '.'))
            })
            .unwrap_or_else(|| "unknown".to_string());
        let body_and_rest = &after[body_start..];
        let (body, next) = match body_and_rest.find("```") {
            Some(close) => (&body_and_rest[..close], &body_and_rest[close + 3..]),
            None => (body_and_rest, ""),
        };
        let lines = body.lines().filter(|l| !l.trim().is_empty()).count();
        out.push_str(&format!("[code: {lang}, {lines} lines]"));
        rest = next;
    }
    out.push_str(rest);
    out
}

// --- the paired comparison ---------------------------------------------------------

/// McNemar's exact test on who was right: `only_ref_right` pairs the
/// reference got right and the variant not, `only_var_right` the other way.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct McNemar {
    pub only_ref_right: u64,
    pub only_var_right: u64,
    /// Two-sided exact binomial p-value over the discordant pairs (1 with
    /// none).
    pub p: f64,
}

/// PURE: the two-sided exact McNemar p-value for `b` and `c` discordant
/// pairs: `min(1, 2 · P(X ≤ min(b, c)))` for `X ~ Binomial(b + c, ½)`,
/// summed in log space so thousands of pairs do not overflow.
pub fn mcnemar_exact(b: u64, c: u64) -> f64 {
    let n = b + c;
    if n == 0 {
        return 1.0;
    }
    let k = b.min(c);
    let ln2 = std::f64::consts::LN_2;
    let mut ln_choose = 0.0f64;
    let mut sum = 0.0f64;
    for i in 0..=k {
        if i > 0 {
            ln_choose += ((n - i + 1) as f64).ln() - (i as f64).ln();
        }
        sum += (ln_choose - n as f64 * ln2).exp();
    }
    (2.0 * sum).min(1.0)
}

/// One pair: the reference's and the variant's outcome on the same case.
/// `right` is the right OUTCOME — for a "none of these" case (J1) an
/// abstention is right; `answered` says whether an answer was given (the
/// denominator of accuracy on answered).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairObs {
    pub ref_answered: bool,
    pub ref_right: bool,
    pub var_answered: bool,
    pub var_right: bool,
    /// The two gave the same answer (both abstaining counts as the same).
    pub same: bool,
}

/// One provider, the variant against its reference, over the usable pairs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PairedCompare {
    pub provider: &'static str,
    pub pairs: u64,
    pub ref_accuracy_on_answered: Option<f64>,
    pub ref_coverage: Option<f64>,
    pub accuracy_on_answered: Option<f64>,
    pub coverage: Option<f64>,
    /// Pairs whose answer differs (an answer against an abstention too).
    pub changed: u64,
    pub change_rate: Option<f64>,
    pub mcnemar: McNemar,
    /// Accuracy on answered, variant minus reference, and its bootstrap
    /// 95% interval.
    pub diff: Option<f64>,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
    /// On who was right (McNemar, so a lost answer counts as much as a
    /// wrong one): `worse` or `better` when its p-value is under [`ALPHA`],
    /// else `no difference`; `not judged` under [`PAIRED_MIN`] pairs. The
    /// accuracy-on-answered interval is reported beside it.
    pub verdict: &'static str,
}

fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn ratio(k: u64, n: u64) -> Option<f64> {
    (n > 0).then(|| r3(k as f64 / n as f64))
}

/// PURE: [`PairedCompare`] of `obs`, the bootstrap with `resamples` and
/// `seed`.
pub fn compare(
    provider: &'static str,
    obs: &[PairObs],
    resamples: usize,
    seed: u64,
) -> PairedCompare {
    let n = obs.len() as u64;
    let count = |f: &dyn Fn(&PairObs) -> bool| obs.iter().filter(|o| f(o)).count() as u64;
    let ref_ans = count(&|o| o.ref_answered);
    let var_ans = count(&|o| o.var_answered);
    let ref_ok = count(&|o| o.ref_answered && o.ref_right);
    let var_ok = count(&|o| o.var_answered && o.var_right);
    let changed = count(&|o| !o.same);
    let b = count(&|o| o.ref_right && !o.var_right);
    let c = count(&|o| !o.ref_right && o.var_right);
    let paired: Vec<Paired> = obs
        .iter()
        .map(|o| Paired {
            a_answered: o.var_answered,
            a_correct: o.var_right,
            b_answered: o.ref_answered,
            b_correct: o.ref_right,
        })
        .collect();
    let ci = bootstrap_acc_diff(&paired, resamples, seed);
    let p = mcnemar_exact(b, c);
    let verdict = match () {
        _ if n < PAIRED_MIN => "not judged",
        _ if p < ALPHA && b > c => "worse",
        _ if p < ALPHA && c > b => "better",
        _ => "no difference",
    };
    PairedCompare {
        provider,
        pairs: n,
        ref_accuracy_on_answered: ratio(ref_ok, ref_ans),
        ref_coverage: ratio(ref_ans, n),
        accuracy_on_answered: ratio(var_ok, var_ans),
        coverage: ratio(var_ans, n),
        changed,
        change_rate: ratio(changed, n),
        mcnemar: McNemar {
            only_ref_right: b,
            only_var_right: c,
            p: r3(p),
        },
        diff: ci.map(|x| r3(x.0)),
        lo: ci.map(|x| r3(x.1)),
        hi: ci.map(|x| r3(x.2)),
        verdict,
    }
}

impl PairedCompare {
    /// One line: `jev  n 212  acc@ans 0.910 → 0.884 (−0.026 [−0.05, −0.004])
    /// cov 0.71 → 0.69  changed 0.09  McNemar 14/6 p 0.115 → worse`.
    pub fn line(&self) -> String {
        let f = |x: Option<f64>| x.map(|v| format!("{v:.3}")).unwrap_or_else(|| "-".into());
        let f2 = |x: Option<f64>| x.map(|v| format!("{v:.2}")).unwrap_or_else(|| "-".into());
        format!(
            "{:<5} n {:>4}  acc@ans {} → {} ({} [{}, {}])  cov {} → {}  changed {}  McNemar {}/{} p {:.3} → {}{}",
            self.provider,
            self.pairs,
            f(self.ref_accuracy_on_answered),
            f(self.accuracy_on_answered),
            f(self.diff),
            f(self.lo),
            f(self.hi),
            f2(self.ref_coverage),
            f2(self.coverage),
            f2(self.change_rate),
            self.mcnemar.only_ref_right,
            self.mcnemar.only_var_right,
            self.mcnemar.p,
            self.verdict,
            if self.pairs < PAIRED_MIN {
                format!(" (n < {PAIRED_MIN})")
            } else {
                String::new()
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_ids_are_decision_words() {
        for p in Perturbation::ALL {
            assert_eq!(Perturbation::parse(p.as_str()), Some(p));
            assert!(crate::store::is_decision_word(&p.case_id("s12")));
        }
        assert_eq!(Perturbation::parse("upper"), None);
        assert_eq!(Perturbation::NoBoard.case_id("s3"), "s3.no-board");
    }

    #[test]
    fn folding_keeps_case_and_everything_but_diacritics() {
        assert_eq!(fold_diacritics("Hotové úlohy"), "Hotove ulohy");
        assert_eq!(fold_diacritics("ČAKÁ NA SCHVÁLENIE"), "CAKA NA SCHVALENIE");
        assert_eq!(fold_diacritics("Straße"), "Strasse");
        assert_eq!(fold_diacritics("in progress 🚀"), "in progress 🚀");
        assert_eq!(fold_diacritics("Vyřízeno"), "Vyrizeno");
    }

    #[test]
    fn a_typo_changes_one_word_by_one_edit_and_is_deterministic() {
        for seed in 0..200u64 {
            let s = "waiting for review";
            let t = typo(s, seed);
            assert_ne!(t, s, "seed {seed}");
            assert_eq!(t, typo(s, seed));
            let (a, b): (Vec<&str>, Vec<&str>) = (s.split(' ').collect(), t.split(' ').collect());
            assert_eq!(a.len(), b.len());
            let diff: Vec<(&&str, &&str)> = a.iter().zip(&b).filter(|(x, y)| x != y).collect();
            assert_eq!(diff.len(), 1, "{t}");
            let (x, y) = diff[0];
            assert_eq!(
                x.chars().next(),
                y.chars().next(),
                "the first letter stays: {t}"
            );
            let (lx, ly) = (x.chars().count() as i64, y.chars().count() as i64);
            assert!((lx - ly).abs() <= 1, "{t}");
        }
        // Nothing to misspell.
        assert_eq!(typo("qa", 1), "qa");
        assert_eq!(typo("to do", 1), "to do");
        // Only a word of letters alone is misspelled.
        let t = typo("v3.2 done", 5);
        assert!(t.starts_with("v3.2 ") && t != "v3.2 done", "{t}");
    }

    #[test]
    fn an_emoji_is_neutral_and_shuffles_never_keep_the_order() {
        let e = emoji_prefix("backlog", 3);
        assert!(e.ends_with(" backlog"));
        assert!(NEUTRAL_EMOJI.iter().any(|x| e.starts_with(x)));
        assert_eq!(e, emoji_prefix("backlog", 3));
        let v: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        for seed in 0..100 {
            let s = shuffle(&v, seed);
            assert_ne!(s, v);
            let mut sorted = s.clone();
            sorted.sort();
            assert_eq!(sorted, v);
        }
        assert_eq!(shuffle(&["x".to_string()], 1), vec!["x".to_string()]);
    }

    #[test]
    fn code_blocks_become_a_placeholder_and_inline_code_stays() {
        let s = "fix the parser\n```rust\nfn a() {}\n\nfn b() {}\n```\nthen run `cargo test`";
        assert_eq!(
            code_placeholder(s),
            "fix the parser\n[code: rust, 2 lines]\nthen run `cargo test`"
        );
        // Unclosed (a prompt cut at 200 characters), no language.
        assert_eq!(
            code_placeholder("see:\n```\nline one\nline two"),
            "see:\n[code: unknown, 2 lines]"
        );
        assert_eq!(code_placeholder("no code here"), "no code here");
    }

    #[test]
    fn mcnemar_matches_the_binomial() {
        assert_eq!(mcnemar_exact(0, 0), 1.0);
        // b = 0, c = 6: 2 · 0.5⁶ = 0.03125.
        assert!((mcnemar_exact(0, 6) - 0.03125).abs() < 1e-12);
        // b = 1, c = 5: 2 · (1 + 6) / 64 = 0.21875.
        assert!((mcnemar_exact(5, 1) - 0.21875).abs() < 1e-12);
        assert_eq!(mcnemar_exact(10, 10), 1.0);
        // Large n does not overflow or turn NaN.
        let p = mcnemar_exact(900, 1100);
        assert!(p.is_finite() && p > 0.0 && p < 0.001, "{p}");
    }

    fn obs(n: usize, var_right: impl Fn(usize) -> bool) -> Vec<PairObs> {
        (0..n)
            .map(|i| PairObs {
                ref_answered: true,
                ref_right: true,
                var_answered: true,
                var_right: var_right(i),
                same: var_right(i),
            })
            .collect()
    }

    #[test]
    fn a_clearly_worse_variant_is_worse_and_a_small_set_is_not_judged() {
        let c = compare("jev", &obs(100, |i| i % 4 != 0), 1000, 7);
        assert_eq!(c.pairs, 100);
        assert_eq!(c.ref_accuracy_on_answered, Some(1.0));
        assert_eq!(c.accuracy_on_answered, Some(0.75));
        assert_eq!(c.changed, 25);
        assert_eq!(
            (c.mcnemar.only_ref_right, c.mcnemar.only_var_right),
            (25, 0)
        );
        assert_eq!(c.verdict, "worse");
        assert!(c.line().contains("→ worse"), "{}", c.line());

        let same = compare("rule", &obs(100, |_| true), 1000, 7);
        assert_eq!(same.verdict, "no difference");
        assert_eq!(same.mcnemar.p, 1.0);

        let few = compare("jev", &obs(20, |i| i % 2 == 0), 1000, 7);
        assert_eq!(few.verdict, "not judged");
        assert!(few.line().contains("(n < 60)"));
    }

    #[test]
    fn an_abstention_that_is_right_counts_for_mcnemar_not_for_accuracy() {
        // A "none of these" case: the reference abstains (right), the
        // variant answers (wrong).
        let o = vec![PairObs {
            ref_answered: false,
            ref_right: true,
            var_answered: true,
            var_right: false,
            same: false,
        }];
        let c = compare("jev", &o, 100, 1);
        assert_eq!(c.ref_accuracy_on_answered, None);
        assert_eq!(c.accuracy_on_answered, Some(0.0));
        assert_eq!(c.mcnemar.only_ref_right, 1);
    }
}
