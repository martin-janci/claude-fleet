//! Card J2's offline benchmark (`fleet-hub decide bench turn-outcome`):
//! captured pane tails, each labeled with what the turn came to, asked of
//! the providers the live adapter compares (`service::decide::turn_outcome`):
//!
//! * `rule` — the pane rules' reading ([`turn_outcome::rule_outcome`]), the
//!   live baseline; it abstains where J8 would warn.
//! * `qmark` — the test map's "ends with ?" heuristic
//!   ([`turn_outcome::ends_with_question`]).
//! * `jev` — the adapter's own request ([`turn_outcome::prepare_tail`],
//!   [`turn_outcome::question_for`]) through [`decide`] (subject `bench`,
//!   question `turn_outcome.bench.v1`, the adapter's floor). Its gate needs
//!   both consents (D31 and D48); a fixture row has no org, so
//!   `decide.jev.unassigned` and `decide.jev.unassigned_reply`.
//!
//! The acceptance (test map J2): `asked` precision ≥ 0.9 and recall ≥ 0.8
//! — a false `finished` hides a waiting session. Judged from
//! [`JUDGE_MIN_ASKED`] labeled `asked` cases; the built-in fixture is far
//! smaller, so on it the verdict is NOT JUDGED. No pane text is printed.

use super::{gate_or_skip, pct, Verdict};
use crate::service::decide::turn_outcome::{
    self, ends_with_question, prepare_tail, question_for, rule_outcome, MIN_CONFIDENCE, UNSURE,
};
use crate::service::decide::{decide, DecideCtx, DecideRequest, Fallback, Feature};
use crate::store::{DECISION_BENCH_SUBJECT, DECISION_NO_BASELINE, TURN_OUTCOMES};
use serde::{Deserialize, Serialize};

/// What the benchmark's Jev runs are recorded under.
pub const QUESTION_VERSION: &str = "turn_outcome.bench.v1";
/// The built-in synthetic set: 24 hand-written tails (LLM-written, D43; not
/// a measurement of real sessions).
pub const FIXTURE: &str = include_str!("../../testdata/decide/turn_outcome_tails.jsonl");
pub const FIXTURE_PATH: &str =
    "crates/fleet-core/src/service/testdata/decide/turn_outcome_tails.jsonl";
/// The acceptance lines.
pub const ACCEPT_ASKED_PRECISION: f64 = 0.9;
pub const ACCEPT_ASKED_RECALL: f64 = 0.8;
/// Labeled `asked` cases needed to judge them.
pub const JUDGE_MIN_ASKED: u64 = 50;
pub const DEFAULT_MAX_CALLS: usize = 500;

/// One labeled tail (one JSON line).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TailLabel {
    #[serde(default)]
    pub id: Option<String>,
    pub pane_tail: String,
    /// `finished | asked | stuck | working`.
    pub label: String,
}

/// PURE: the labeled tails of a JSONL file; blank lines skipped, a label
/// outside the outcomes refused with its line number.
pub fn parse_labels(jsonl: &str) -> Result<Vec<TailLabel>, String> {
    let mut out = Vec::new();
    for (i, line) in jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let l: TailLabel =
            serde_json::from_str(line).map_err(|e| format!("line {}: {e}", i + 1))?;
        if l.label == UNSURE || !TURN_OUTCOMES.contains(&l.label.as_str()) {
            return Err(format!(
                "line {}: label is one of finished, asked, stuck, working, not {:?}",
                i + 1,
                l.label
            ));
        }
        out.push(l);
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Rule,
    Qmark,
    Jev,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Rule => "rule",
            Provider::Qmark => "qmark",
            Provider::Jev => "jev",
        }
    }
    pub fn parse(s: &str) -> Option<Provider> {
        [Provider::Rule, Provider::Qmark, Provider::Jev]
            .into_iter()
            .find(|p| p.as_str() == s)
    }
}

/// One provider's answer to one case: `None` is an abstention; `skipped`
/// names why nothing was asked (a gate fallback, `max_calls`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Answer {
    pub answer: Option<String>,
    pub skipped: Option<String>,
}

/// PURE: an offline provider's answer.
pub fn offline(p: Provider, tail: &str) -> Answer {
    let a = match p {
        Provider::Rule => rule_outcome(tail),
        Provider::Qmark => ends_with_question(tail),
        Provider::Jev => None,
    };
    Answer {
        answer: a.map(str::to_string),
        skipped: None,
    }
}

/// Ask Jev about each case through the envelope (recorded as a benchmark
/// run); at most `max_calls` calls.
pub async fn run_jev(ctx: &DecideCtx, cases: &[TailLabel], max_calls: usize) -> Vec<Answer> {
    let mut out = Vec::with_capacity(cases.len());
    let mut calls = 0usize;
    for (i, c) in cases.iter().enumerate() {
        if let Err(r) = gate_or_skip(ctx, Feature::TurnOutcome, None, calls, max_calls) {
            out.push(Answer {
                answer: None,
                skipped: Some(r.to_string()),
            });
            continue;
        }
        calls += 1;
        let res = decide(
            ctx,
            DecideRequest {
                feature: Feature::TurnOutcome,
                subject_kind: DECISION_BENCH_SUBJECT.into(),
                subject_id: format!("bench:{i}"),
                org_id: None,
                request: question_for(&prepare_tail(&c.pane_tail)),
                baseline: Some(
                    rule_outcome(&c.pane_tail)
                        .unwrap_or(DECISION_NO_BASELINE)
                        .to_string(),
                ),
                question_version: QUESTION_VERSION.into(),
                min_confidence: Some(MIN_CONFIDENCE),
            },
        )
        .await;
        let skipped = res
            .fallback
            .filter(|f| *f != Fallback::LowConfidence)
            .map(|f| f.as_str().to_string());
        let answer = res
            .usable()
            .map(|a| a.value.clone())
            .filter(|v| v != turn_outcome::UNSURE);
        out.push(Answer { answer, skipped });
    }
    out
}

/// One provider's numbers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Metrics {
    pub provider: Provider,
    pub cases: u64,
    pub skipped: u64,
    pub answered: u64,
    pub correct: u64,
    pub coverage: Option<f64>,
    pub accuracy_on_answered: Option<f64>,
    /// Labeled `asked`, answered `asked`, and both.
    pub asked_labeled: u64,
    pub asked_answered: u64,
    pub asked_right: u64,
    pub asked_precision: Option<f64>,
    /// Over every labeled `asked` case asked (an abstention misses it).
    pub asked_recall: Option<f64>,
    pub precision_verdict: Verdict,
    pub recall_verdict: Verdict,
}

/// PURE: the metrics of `answers` against `cases` (same order).
pub fn metrics(p: Provider, cases: &[TailLabel], answers: &[Answer]) -> Metrics {
    let mut m = Metrics {
        provider: p,
        cases: 0,
        skipped: 0,
        answered: 0,
        correct: 0,
        coverage: None,
        accuracy_on_answered: None,
        asked_labeled: 0,
        asked_answered: 0,
        asked_right: 0,
        asked_precision: None,
        asked_recall: None,
        precision_verdict: Verdict::NotJudged,
        recall_verdict: Verdict::NotJudged,
    };
    for (c, a) in cases.iter().zip(answers) {
        if a.skipped.is_some() {
            m.skipped += 1;
            continue;
        }
        m.cases += 1;
        let asked = c.label == "asked";
        m.asked_labeled += u64::from(asked);
        let Some(ans) = a.answer.as_deref() else {
            continue;
        };
        m.answered += 1;
        m.correct += u64::from(ans == c.label);
        if ans == "asked" {
            m.asked_answered += 1;
            m.asked_right += u64::from(asked);
        }
    }
    m.coverage = pct(m.answered, m.cases);
    m.accuracy_on_answered = pct(m.correct, m.answered);
    m.asked_precision = pct(m.asked_right, m.asked_answered);
    m.asked_recall = pct(m.asked_right, m.asked_labeled);
    let judged = m.asked_labeled >= JUDGE_MIN_ASKED;
    m.precision_verdict = Verdict::at_least(m.asked_precision, ACCEPT_ASKED_PRECISION, judged);
    m.recall_verdict = Verdict::at_least(m.asked_recall, ACCEPT_ASKED_RECALL, judged);
    m
}

fn f3(v: Option<f64>) -> String {
    v.map_or_else(|| "-".to_string(), |v| format!("{v:.3}"))
}

/// The report's lines: one per provider, then the acceptance.
pub fn lines(all: &[Metrics]) -> Vec<String> {
    let mut out = vec![format!(
        "J2 turn_outcome: asked precision >= {ACCEPT_ASKED_PRECISION} and recall >= \
         {ACCEPT_ASKED_RECALL}, judged from {JUDGE_MIN_ASKED} labeled asked cases"
    )];
    for m in all {
        out.push(format!(
            "{:<6} cases {}  skipped {}  coverage {}  accuracy {}  asked: precision {} ({}/{})  \
             recall {} ({}/{})  -> precision {}, recall {}",
            m.provider.as_str(),
            m.cases,
            m.skipped,
            f3(m.coverage),
            f3(m.accuracy_on_answered),
            f3(m.asked_precision),
            m.asked_right,
            m.asked_answered,
            f3(m.asked_recall),
            m.asked_right,
            m.asked_labeled,
            m.precision_verdict.as_str(),
            m.recall_verdict.as_str(),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<TailLabel> {
        parse_labels(FIXTURE).unwrap()
    }

    fn run(p: Provider) -> Metrics {
        let cases = fixture();
        let answers: Vec<Answer> = cases.iter().map(|c| offline(p, &c.pane_tail)).collect();
        metrics(p, &cases, &answers)
    }

    #[test]
    fn the_fixture_parses_and_a_bad_label_is_refused() {
        let cases = fixture();
        assert_eq!(cases.len(), 24);
        assert_eq!(cases.iter().filter(|c| c.label == "asked").count(), 9);
        assert!(parse_labels("{\"pane_tail\":\"x\",\"label\":\"done\"}").is_err());
        assert!(parse_labels("{\"pane_tail\":\"x\",\"label\":\"unsure\"}").is_err());
        assert!(parse_labels("\n").unwrap().is_empty());
    }

    /// The baselines' numbers on the fixture are pinned: the rules read an
    /// idle REPL as finished (so they miss every prose question), "ends
    /// with ?" misses a polite request, a dialog and an unknown input line,
    /// and neither is judged on 9 cases.
    #[test]
    fn the_baselines_numbers_on_the_fixture() {
        let rule = run(Provider::Rule);
        assert_eq!(
            (rule.cases, rule.answered),
            (24, 21),
            "J8: three screens the rules cannot read"
        );
        assert_eq!(
            (rule.asked_right, rule.asked_answered),
            (1, 1),
            "only the dialog"
        );
        assert_eq!(rule.asked_recall, Some(0.111));
        let q = run(Provider::Qmark);
        assert_eq!(q.answered, 24);
        assert_eq!((q.asked_right, q.asked_answered), (6, 6));
        assert_eq!(q.asked_precision, Some(1.0));
        assert_eq!(q.asked_recall, Some(0.667));
        for m in [&rule, &q] {
            assert_eq!(m.precision_verdict, Verdict::NotJudged);
            assert_eq!(m.recall_verdict, Verdict::NotJudged);
        }
        let l = lines(&[rule, q]);
        assert_eq!(l.len(), 3);
        assert!(
            l.iter().all(|x| !x.contains("migration guide")),
            "no pane text"
        );
    }

    #[test]
    fn the_acceptance_is_judged_from_enough_asked_cases() {
        let cases: Vec<TailLabel> = (0..60)
            .map(|i| TailLabel {
                id: None,
                pane_tail: String::new(),
                label: if i < 50 { "asked" } else { "finished" }.into(),
            })
            .collect();
        let answers: Vec<Answer> = (0..60)
            .map(|i| Answer {
                answer: Some(
                    if !(45..58).contains(&i) {
                        "asked"
                    } else {
                        "finished"
                    }
                    .into(),
                ),
                skipped: None,
            })
            .collect();
        let m = metrics(Provider::Jev, &cases, &answers);
        assert_eq!(
            (m.asked_right, m.asked_answered, m.asked_labeled),
            (45, 47, 50)
        );
        assert_eq!(m.precision_verdict, Verdict::Pass, "0.957");
        assert_eq!(m.recall_verdict, Verdict::Pass, "0.9");
        let worse: Vec<Answer> = (0..60)
            .map(|i| Answer {
                answer: Some(if i < 30 { "asked" } else { "finished" }.into()),
                skipped: None,
            })
            .collect();
        assert_eq!(
            metrics(Provider::Jev, &cases, &worse).recall_verdict,
            Verdict::Fail
        );
    }
}
