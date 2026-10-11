//! Per-feature confidence floors fitted from the record.
//!
//! Every adapter carries a built-in floor (`MIN_CONFIDENCE`, 0.5). A fitted
//! floor raises it for one feature, from evidence: the feature's runs a
//! person later confirmed, corrected or rejected (D37's follow-ups). The
//! fit splits those runs by time — the older half chooses the lowest floor
//! whose answers reach the target precision, the newer half must reach it
//! too at that floor — and only a fit that holds on both halves is stored.
//!
//! The floors live in `decide.jev.floors`, a setting only `fleet-hub decide
//! floor` writes. Each is tied to the question version and the model it was
//! fitted against: when an adapter bumps its question version or the model
//! setting changes, the fitted floor stops applying and the built-in one
//! stands until a new fit. A fitted floor only ever raises the built-in one
//! (D41: automatic only toward the conservative side).

use super::Feature;
use crate::service::settings;
use crate::store::{is_decision_word, DecisionRunRow, Store, DECISION_PERSON_FOLLOWUPS};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One feature's fitted floor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FittedFloor {
    /// The adapter's question version the runs answered.
    pub question_version: String,
    /// The model setting (`decide.jev.model`) the runs were asked under.
    pub model: String,
    /// The floor, in 0..=1.
    pub floor: f64,
    /// The precision it was fitted for.
    pub target: f64,
    /// Labelled runs the fit read (both halves).
    pub labelled: u64,
    /// When it was fitted (unix seconds).
    pub fitted_at: i64,
}

/// Every fitted floor, by feature (`Feature::as_str`).
pub type Floors = BTreeMap<String, FittedFloor>;

/// PURE: `v` (the setting's stored text) as floors; `""` is none. Refuses
/// an unknown feature, a floor outside 0..=1, and a version or model that
/// is not a decision word.
pub fn parse(v: &str) -> Result<Floors, String> {
    let v = v.trim();
    if v.is_empty() {
        return Ok(Floors::new());
    }
    let floors: Floors =
        serde_json::from_str(v).map_err(|e| format!("not a floors object: {e}"))?;
    for (feature, f) in &floors {
        if Feature::parse(feature).is_none() {
            return Err(format!("{feature:?} is not a feature"));
        }
        if !(f.floor.is_finite() && (0.0..=1.0).contains(&f.floor))
            || !(f.target.is_finite() && (0.0..=1.0).contains(&f.target))
        {
            return Err(format!("{feature}: a floor and a target are in 0..1"));
        }
        if !is_decision_word(&f.question_version) || !is_decision_word(&f.model) {
            return Err(format!("{feature}: a version and a model are words"));
        }
    }
    Ok(floors)
}

/// PURE: the setting's text for `floors` (compact JSON, `""` for none).
pub fn render(floors: &Floors) -> String {
    if floors.is_empty() {
        return String::new();
    }
    serde_json::to_string(floors).expect("floors always serialize")
}

/// The stored floors. A value that does not parse counts as none (logged):
/// the built-in floors stand.
pub fn read(s: &Store) -> Floors {
    let v = settings::get_string(s, settings::DECIDE_JEV_FLOORS);
    parse(&v).unwrap_or_else(|why| {
        tracing::warn!("[decide] {} ignored: {why}", settings::DECIDE_JEV_FLOORS);
        Floors::new()
    })
}

/// PURE: the floor a call is held to: the adapter's own, raised to the
/// feature's fitted floor when that was fitted against this question
/// version and model. `None` (no floor at all) only when the adapter set
/// none and nothing was fitted.
pub fn effective(
    adapter: Option<f64>,
    floors: &Floors,
    feature: Feature,
    question_version: &str,
    model: &str,
) -> Option<f64> {
    let fitted = floors
        .get(feature.as_str())
        .filter(|f| f.question_version == question_version && f.model == model)
        .map(|f| f.floor);
    match (adapter, fitted) {
        (Some(a), Some(f)) => Some(a.max(f)),
        (a, f) => a.or(f),
    }
}

// --- the fit --------------------------------------------------------------------

/// The floors a fit tries, lowest first.
pub const FIT_FLOORS: &[f64] = &[
    0.5, 0.55, 0.6, 0.65, 0.7, 0.75, 0.8, 0.85, 0.9, 0.92, 0.94, 0.95, 0.96, 0.97, 0.98, 0.99,
    0.995, 0.999,
];

/// One labelled run: how sure the answer was, and whether the person's
/// follow-up says it was right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Labelled {
    pub at: i64,
    pub certainty: Option<f64>,
    pub correct: bool,
}

/// PURE: the runs a fit reads: answered (not `unsure`) and followed up by
/// a person — `confirmed` is right, `corrected` and `rejected` are wrong —
/// for this question version and model, oldest first. Benchmark runs are
/// not the live distribution and are skipped.
pub fn labelled(rows: &[DecisionRunRow], question_version: &str, model: &str) -> Vec<Labelled> {
    let mut out: Vec<Labelled> = rows
        .iter()
        .filter(|r| {
            r.question_version == question_version
                && r.model_version.as_deref() == Some(model)
                && r.subject_kind != crate::store::DECISION_BENCH_SUBJECT
                && r.answer.as_deref().is_some_and(|a| a != "unsure")
        })
        .filter_map(|r| {
            let f = r.followup.as_deref()?;
            DECISION_PERSON_FOLLOWUPS.contains(&f).then(|| Labelled {
                at: r.at,
                certainty: r.confidence,
                correct: f == "confirmed",
            })
        })
        .collect();
    out.sort_by_key(|l| l.at);
    out
}

/// One side's numbers at a floor.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Side {
    pub runs: u64,
    /// Runs at or above the floor (what would still be proposed).
    pub answered: u64,
    pub correct: u64,
}

impl Side {
    fn at(rows: &[Labelled], floor: f64) -> Side {
        let mut s = Side {
            runs: rows.len() as u64,
            answered: 0,
            correct: 0,
        };
        for l in rows {
            if l.certainty.is_some_and(|c| c + 1e-9 >= floor) {
                s.answered += 1;
                s.correct += u64::from(l.correct);
            }
        }
        s
    }

    /// Correct over answered; `None` when nothing was answered.
    pub fn precision(&self) -> Option<f64> {
        (self.answered > 0).then(|| self.correct as f64 / self.answered as f64)
    }

    /// Answered over runs.
    pub fn coverage(&self) -> Option<f64> {
        (self.runs > 0).then(|| self.answered as f64 / self.runs as f64)
    }

    fn meets(&self, target: f64, min_answered: u64) -> bool {
        self.answered >= min_answered && self.precision().is_some_and(|p| p + 1e-9 >= target)
    }
}

/// What a fit found.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Fit {
    pub target: f64,
    pub min_answered: u64,
    pub labelled: u64,
    /// The lowest floor the older half meets the target at.
    pub floor: Option<f64>,
    /// The older half at that floor.
    pub fit: Option<Side>,
    /// The newer half at that floor.
    pub test: Option<Side>,
    /// The newer half meets the target too: the floor may be stored.
    pub holds: bool,
}

/// PURE: fit a floor on `rows` (oldest first) for `target` precision, each
/// half answering at least `min_answered` runs at it.
pub fn fit(rows: &[Labelled], target: f64, min_answered: u64) -> Fit {
    let (older, newer) = rows.split_at(rows.len() / 2);
    let chosen = FIT_FLOORS
        .iter()
        .copied()
        .find(|&f| Side::at(older, f).meets(target, min_answered));
    let fit = chosen.map(|f| Side::at(older, f));
    let test = chosen.map(|f| Side::at(newer, f));
    Fit {
        target,
        min_answered,
        labelled: rows.len() as u64,
        floor: chosen,
        holds: test.is_some_and(|t| t.meets(target, min_answered)),
        fit,
        test,
    }
}

impl Fit {
    /// The report, one line each.
    pub fn lines(&self, feature: Feature) -> Vec<String> {
        let side = |name: &str, s: &Side| {
            format!(
                "  {name}: {} of {} runs answered ({}), {} right ({})",
                s.answered,
                s.runs,
                pct(s.coverage()),
                s.correct,
                pct(s.precision()),
            )
        };
        let mut out = vec![format!(
            "{}: {} labelled runs, target precision {}, at least {} answered per half",
            feature.as_str(),
            self.labelled,
            pct(Some(self.target)),
            self.min_answered
        )];
        match (self.floor, &self.fit, &self.test) {
            (Some(f), Some(a), Some(b)) => {
                out.push(format!("  floor {f}"));
                out.push(side("older half (fit)", a));
                out.push(side("newer half (test)", b));
                out.push(if self.holds {
                    "  holds on the newer half".into()
                } else {
                    "  does NOT hold on the newer half: not stored".into()
                });
            }
            _ => out.push(
                "  no floor reaches the target on the older half: not stored (label more runs, \
                 or lower --precision)"
                    .into(),
            ),
        }
        out
    }
}

fn pct(x: Option<f64>) -> String {
    match x {
        Some(x) => format!("{:.1}%", x * 100.0),
        None => "-".into(),
    }
}

#[cfg(test)]
#[path = "floor_tests.rs"]
mod tests;
