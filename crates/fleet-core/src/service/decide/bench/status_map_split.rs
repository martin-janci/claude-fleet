//! The status_map benchmark's split question (`--provider jev-split`): the
//! adapter's one five-way Choice asked instead as four narrow Nouls — "are
//! the tasks in this section not started?", "… being worked on?", "…
//! finished?", "… not going to be done?" — and combined in code.
//!
//! Independent tests of jev-1.13 found one broad question far weaker than
//! the same task split into narrow ones and combined in code (phishing: 62.6%
//! as one question, 95.0% as five). This provider measures whether J3's
//! question is such a case before anyone rewrites the live adapter.
//!
//! Each Noul goes through the envelope like the `jev` provider's Choice:
//! the same state (the section and its board window, redacted as ever),
//! feature `status_map`, subject `bench:<case>.<category>`, question
//! version [`QUESTION_VERSION`]. A case costs four calls. **The combination**
//! ([`combine`]): the category whose Noul is highest is the answer when it
//! reaches the adapter's floor and no other category does (two "yes" is a
//! conflict, and abstains like `unsure`); its Noul is the confidence. A
//! case with any failed call is skipped with that call's fallback.

use super::gate_or_skip;
use super::status_map::{Outcome, SectionCase, CATEGORIES, SUBJECT_KIND};
use crate::service::decide::status_map::{self as sm, MIN_CONFIDENCE, NO_RULE};
use crate::service::decide::{decide, DecideCtx, DecideRequest, Feature, JevRequest, Question};
use serde_json::Value;

/// The question version every Noul of the split question records.
pub const QUESTION_VERSION: &str = "status_map.bench.split.v1";

/// What the Nouls share: the state's meaning and the evidence to use (the
/// adapter's instruction without its choice); the category's criterion
/// follows it.
pub const NOUL_PREFIX: &str = "state.section is the name of one section (a column) of an Asana \
     project board, in lower case. state.project_sections lists every section of that board in \
     board order, first to last. Using only the section's name and its position among \
     state.project_sections (sections placed after an in-progress section are usually later \
     stages, and the first sections are usually work not started), is the following true of the \
     tasks placed in state.section?";

/// PURE: the criterion of `category`, the adapter's own wording.
fn criterion(category: &str) -> &'static str {
    sm::OPTIONS
        .iter()
        .find(|(k, _)| *k == category)
        .map(|(_, d)| *d)
        .expect("every category has an option")
}

/// PURE: the Noul for `c` about `category`: the adapter's state, one
/// narrow question.
pub fn noul_request(c: &SectionCase, category: &str) -> JevRequest {
    let base = super::status_map::request_for(c, None);
    JevRequest {
        state: base.state,
        question: Question::Noul {
            instructions: Value::String(format!("{NOUL_PREFIX} {}", criterion(category))),
            criteria: None,
        },
        order_salt: None,
    }
}

/// PURE: the combined answer of four Nouls (`(category, p)`, in
/// [`CATEGORIES`] order): the highest category when it reaches the floor
/// and no other does, with its Noul as the confidence; else an abstention
/// (`None`), still naming the highest category and its Noul for
/// calibration.
pub fn combine(
    nouls: &[(&'static str, f64)],
) -> (Option<&'static str>, Option<(&'static str, f64)>) {
    let top = nouls
        .iter()
        .copied()
        .fold(None::<(&str, f64)>, |best, (k, p)| match best {
            Some((_, b)) if b >= p => best,
            _ => Some((k, p)),
        });
    let above = nouls
        .iter()
        .filter(|(_, p)| *p + 1e-9 >= MIN_CONFIDENCE)
        .count();
    let answer = top.filter(|_| above == 1).map(|(k, _)| k);
    (answer, top)
}

/// Ask every case the four Nouls through the envelope. A case is asked
/// only when its org passes the gate and all four calls fit under
/// `max_calls`; a case with a failed call is skipped with its fallback.
pub async fn run(ctx: &DecideCtx, cases: &[SectionCase], max_calls: usize) -> Vec<Outcome> {
    let n = CATEGORIES.len();
    let mut out = Vec::with_capacity(cases.len());
    let mut calls = 0usize;
    'case: for c in cases {
        // All four calls must fit: the gate refuses at `max_calls`.
        if let Err(r) = gate_or_skip(ctx, Feature::StatusMap, c.org_id, calls + n - 1, max_calls) {
            out.push(Outcome {
                reason: Some(r.to_string()),
                ..Default::default()
            });
            continue;
        }
        let mut o = Outcome {
            ran: true,
            ..Default::default()
        };
        let mut nouls = Vec::with_capacity(n);
        for cat in CATEGORIES {
            calls += 1;
            let res = decide(
                ctx,
                DecideRequest {
                    feature: Feature::StatusMap,
                    subject_kind: SUBJECT_KIND.into(),
                    subject_id: format!("{}.{cat}", c.id),
                    org_id: c.org_id,
                    request: noul_request(c, cat),
                    baseline: Some(c.rule.unwrap_or(NO_RULE).to_string()),
                    question_version: QUESTION_VERSION.into(),
                    min_confidence: None,
                },
            )
            .await;
            o.ran &= res.mode.is_some();
            o.latency_ms = Some(o.latency_ms.unwrap_or(0) + res.latency_ms.unwrap_or(0));
            o.input_tokens += res.input_tokens;
            o.cost_microusd += res.cost_microusd;
            let p = res.usable().and_then(|a| a.value.parse::<f64>().ok());
            match (res.fallback, p) {
                (None, Some(p)) => nouls.push((cat, p)),
                (f, _) => {
                    o.reason = Some(f.map_or("invalid_answer", |f| f.as_str()).to_string());
                    out.push(o);
                    continue 'case;
                }
            }
        }
        let (answer, top) = combine(&nouls);
        o.answer = answer.map(str::to_string);
        if let Some((k, p)) = top {
            o.model = Some(k.to_string());
            o.confidence = Some(p);
        }
        out.push(o);
    }
    out
}

#[cfg(test)]
#[path = "status_map_split_tests.rs"]
mod tests;
