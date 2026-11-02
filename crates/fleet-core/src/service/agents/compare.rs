//! The built-in agents next to their restated definitions
//! (`docs/fleet-agents-rollout.md`, *Compare*): the jobs both ran on the
//! same work, side by side, and whether they agree.
//!
//! * A review: the built-in reviewer's job and a fleet instance's
//!   `github_prs` job on the same pull request at the same head
//!   (`gh:<owner>/<repo>#<n>@<sha>`); they agree on the same verdict.
//! * A support investigation: the built-in support agent's job and a fleet
//!   instance's tracker job on the same ticket comment (`<KEY>#c<id>`);
//!   they agree on the same severity and confidence.
//!
//! [`compare`] is PURE over the jobs; `admin` reads them.

use crate::store::AgentJobRow;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Jobs `compare` reads, newest first, at most.
pub const COMPARE_JOBS: usize = 1000;
/// Pairs it shows, newest first, at most (the counts cover all of them).
pub const COMPARE_PAIRS: usize = 50;

/// One side of a pair: what that agent's job came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Side {
    pub job_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<i64>,
    pub state: String,
    pub cost_micros: i64,
    /// What it said, in a few words ("REQUEST_CHANGES, 3 findings",
    /// "major, high confidence, BUG-9").
    pub said: String,
    /// What the agreement is judged on: the verdict, or severity and
    /// confidence. `None` when the job has no answer (skipped, failed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
}

/// The same work, done by both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pair {
    /// `review` | `support`.
    pub kind: String,
    /// The work both are about: `gh:o/r#4@<sha>` or `L2S-7#c100`.
    pub key: String,
    pub builtin: Side,
    pub fleet: Side,
    /// Both answered, and the same; `None` while either has no answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agree: Option<bool>,
}

/// `agents { action: compare }`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comparison {
    /// Newest first, at most [`COMPARE_PAIRS`].
    pub pairs: Vec<Pair>,
    pub agree: u32,
    pub differ: u32,
    /// Work only one of them did, among the jobs read: the other has no
    /// rule there, had not run yet, or keys the work differently.
    pub builtin_only: u32,
    pub fleet_only: u32,
}

fn result_of(j: &AgentJobRow) -> Option<Value> {
    serde_json::from_str(j.result.as_deref()?).ok()
}

fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or_default()
}

/// PURE: the head a review job's key names: after its last `@`, up to a
/// `#` (a forced review's reason) or a `:` (a fleet reverify's).
fn head_of(key: &str) -> Option<&str> {
    let (_, rest) = key.rsplit_once('@')?;
    let head = rest.split(['#', ':']).next().unwrap_or(rest);
    (!head.is_empty()).then_some(head)
}

/// PURE: the ticket comment a support job's key names: `<KEY>#<why>`.
fn ticket_of(key: &str) -> Option<String> {
    let (left, why) = key.rsplit_once('#')?;
    let ticket = left.rsplit(':').next()?;
    (!ticket.is_empty() && !why.is_empty()).then(|| format!("{ticket}#{why}"))
}

/// PURE: which work a job is about, and as which side: `(kind, key)`.
fn work_of(j: &AgentJobRow) -> Option<(&'static str, String)> {
    match j.agent.as_str() {
        "review" => Some((
            "review",
            format!("{}@{}", j.subject, head_of(&j.dedup_key)?),
        )),
        "support" if j.subject.starts_with("jira:") => Some(("support", ticket_of(&j.dedup_key)?)),
        "fleet" if j.subject.starts_with("gh:") && !j.subject.contains("@t") => Some((
            "review",
            format!("{}@{}", j.subject, head_of(&j.dedup_key)?),
        )),
        "fleet" if j.subject.starts_with("tracker:") => Some(("support", ticket_of(&j.dedup_key)?)),
        _ => None,
    }
}

/// PURE: a job's side of the work.
fn side(kind: &str, j: &AgentJobRow) -> Side {
    let r = result_of(j);
    // A fleet job's answer is under `answer`; the built-in ones' is the result.
    let answer = r
        .as_ref()
        .map(|r| {
            if j.agent == "fleet" {
                r["answer"].clone()
            } else {
                r.clone()
            }
        })
        .filter(Value::is_object);
    let (said, verdict) = match (kind, &answer) {
        ("review", Some(a)) => {
            let verdict = text(a, "verdict").to_ascii_uppercase();
            let n = if j.agent == "fleet" {
                a.get("findings")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len) as u64
            } else {
                a.get("inline").and_then(Value::as_u64).unwrap_or(0)
                    + a.get("orphans").and_then(Value::as_u64).unwrap_or(0)
            };
            let s = if n == 1 { "" } else { "s" };
            (
                format!("{verdict}, {n} finding{s}"),
                Some(verdict).filter(|v| !v.is_empty()),
            )
        }
        ("support", Some(a)) => {
            let severity = text(a, "severity").to_ascii_lowercase();
            let confidence = text(a, "confidence").to_ascii_lowercase();
            let bug = ["knownBug", "similarBug"]
                .iter()
                .map(|k| text(a, k))
                .find(|b| !b.is_empty());
            let mut said = format!("{severity}, {confidence} confidence");
            if let Some(b) = bug {
                said.push_str(&format!(", {b}"));
            }
            let judged = format!("{severity}/{confidence}");
            (said, (judged != "/").then_some(judged))
        }
        _ => (j.error.clone().unwrap_or_else(|| j.state.clone()), None),
    };
    Side {
        job_id: j.id,
        rule_id: j.rule_id,
        state: j.state.clone(),
        cost_micros: j.cost_micros,
        said,
        verdict,
    }
}

/// PURE: the jobs (newest first) as pairs of the same work: for each work,
/// the newest job of each side.
pub fn compare(jobs: &[AgentJobRow]) -> Comparison {
    let mut builtin: BTreeMap<(&str, String), &AgentJobRow> = BTreeMap::new();
    let mut fleet: BTreeMap<(&str, String), &AgentJobRow> = BTreeMap::new();
    let mut order: Vec<(&str, String)> = Vec::new();
    for j in jobs {
        let Some(w) = work_of(j) else { continue };
        let map = if j.agent == "fleet" {
            &mut fleet
        } else {
            &mut builtin
        };
        // Newest first: the first job of a side is the one kept.
        if let std::collections::btree_map::Entry::Vacant(e) = map.entry(w.clone()) {
            if !order.contains(&w) {
                order.push(w);
            }
            e.insert(j);
        }
    }
    let mut out = Comparison::default();
    for w in &order {
        match (builtin.get(w), fleet.get(w)) {
            (Some(b), Some(f)) => {
                let (b, f) = (side(w.0, b), side(w.0, f));
                let agree = match (&b.verdict, &f.verdict) {
                    (Some(x), Some(y)) => Some(x == y),
                    _ => None,
                };
                match agree {
                    Some(true) => out.agree += 1,
                    Some(false) => out.differ += 1,
                    None => {}
                }
                if out.pairs.len() < COMPARE_PAIRS {
                    out.pairs.push(Pair {
                        kind: w.0.to_string(),
                        key: w.1.clone(),
                        builtin: b,
                        fleet: f,
                        agree,
                    });
                }
            }
            (Some(_), None) => out.builtin_only += 1,
            (None, Some(_)) => out.fleet_only += 1,
            (None, None) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn job(id: i64, agent: &str, subject: &str, key: &str, result: Value) -> AgentJobRow {
        AgentJobRow {
            id,
            rule_id: Some(if agent == "fleet" { 2 } else { 1 }),
            agent: agent.into(),
            subject: subject.into(),
            trigger: String::new(),
            dedup_key: key.into(),
            state: "done".into(),
            attempts: 1,
            next_attempt_at: 0,
            lease_until: None,
            error_kind: None,
            error: None,
            result: (!result.is_null()).then(|| result.to_string()),
            cost_micros: 1000 * id,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn a_review_pairs_by_head_and_agrees_on_the_verdict() {
        let jobs = [
            // Newest first: the fleet instance's, then the built-in's.
            job(
                4,
                "fleet",
                "gh:o/r#4",
                "r2:gh:o/r#4@abc",
                json!({ "answer": { "verdict": "approve", "findings": [{}, {}] } }),
            ),
            job(
                3,
                "review",
                "gh:o/r#4",
                "gh:o/r#4@abc",
                json!({ "verdict": "APPROVE", "inline": 1, "orphans": 1 }),
            ),
            job(
                2,
                "fleet",
                "gh:o/r#5",
                "r2:gh:o/r#5@def:r77",
                json!({ "answer": { "verdict": "COMMENT", "findings": [] } }),
            ),
            job(
                1,
                "review",
                "gh:o/r#5",
                "gh:o/r#5@def#c9",
                json!({ "verdict": "REQUEST_CHANGES", "inline": 1, "orphans": 0 }),
            ),
            // A debate and an older head of #4: neither pairs.
            job(
                0,
                "fleet",
                "gh:o/r#4@t7",
                "r2:gh:o/r#4@t7:c9",
                json!({ "answer": {} }),
            ),
            job(
                -1,
                "review",
                "gh:o/r#4",
                "gh:o/r#4@old",
                json!({ "verdict": "COMMENT" }),
            ),
        ];
        let c = compare(&jobs);
        assert_eq!(
            (c.agree, c.differ, c.builtin_only, c.fleet_only),
            (1, 1, 1, 0)
        );
        assert_eq!(c.pairs[0].key, "gh:o/r#4@abc");
        assert_eq!(c.pairs[0].builtin.said, "APPROVE, 2 findings");
        assert_eq!(c.pairs[0].fleet.said, "APPROVE, 2 findings");
        assert_eq!(c.pairs[0].agree, Some(true));
        assert_eq!(c.pairs[1].key, "gh:o/r#5@def");
        assert_eq!(c.pairs[1].agree, Some(false));
    }

    #[test]
    fn an_investigation_pairs_by_comment_and_agrees_on_severity_and_confidence() {
        let jobs = [
            job(
                2,
                "fleet",
                "tracker:3:L2S-7",
                "r2:tracker:3:L2S-7#c100",
                json!({ "answer": { "severity": "major", "confidence": "high", "knownBug": "BUG-9" } }),
            ),
            job(
                1,
                "support",
                "jira:L2S-7",
                "jira:L2S-7#c100",
                json!({ "severity": "Major", "confidence": "High" }),
            ),
        ];
        let c = compare(&jobs);
        assert_eq!(c.pairs.len(), 1);
        let p = &c.pairs[0];
        assert_eq!((p.kind.as_str(), p.key.as_str()), ("support", "L2S-7#c100"));
        assert_eq!(p.fleet.said, "major, high confidence, BUG-9");
        assert_eq!(p.agree, Some(true));

        // A job with no answer pairs, but is not judged.
        let mut skipped = jobs[1].clone();
        skipped.result = None;
        skipped.state = "skipped".into();
        skipped.error = Some("cooldown".into());
        let c = compare(&[jobs[0].clone(), skipped]);
        assert_eq!(
            (c.pairs[0].agree, c.pairs[0].builtin.said.as_str()),
            (None, "cooldown")
        );
        assert_eq!((c.agree, c.differ), (0, 0));
    }
}
