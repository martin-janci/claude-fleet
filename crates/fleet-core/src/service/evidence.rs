//! What stands between a session's PR and a merge, decided once, here.
//!
//! The PR probe (`service::outcome`) stores what it read as
//! `sessions.pr_evidence`, with `pr_checked_at` saying when. This module turns
//! one reading into a verdict and the reasons for it: the design's §5
//! (`docs/specs/2026-09-29-result-evidence-design.md`).
//!
//! # Principles it holds
//!
//! - **`Ready` needs a fresh reading.** A reading older than
//!   [`PR_EVIDENCE_STALE_SECS`] is `Unknown`, whatever it said: it describes
//!   the past. Its reasons are still listed, as the last thing known.
//! - **Absent is never fine.** No evidence (an older hub, an old `gh`) is
//!   `Unknown`; "no checks configured" is `Waiting`, not a pass.
//! - **It explains, it does not decide.** Nothing here gates `done`, blocks
//!   a merge or feeds Attention.
//!
//! # One rule, two places
//!
//! The desktop assesses the row it already holds (`src/lib/evidence.ts`), so
//! the card works the same against a hub and needs no round trip. The two are
//! held to each other by the shared fixture `testdata/evidence_cases.json`:
//! both test suites run every case in it, so a rule changed on one side
//! only fails until the other matches.
//!
//! GitHub's `mergeStateStatus: UNKNOWN` is deliberately no reason: GitHub
//! computes mergeability lazily and answers `UNKNOWN` to a first read as a
//! matter of course, so counting it would make most PRs "unknown".

use crate::service::outcome::{PrEvidence, PR_EVIDENCE_STALE_SECS};

/// The one-word answer. [`Verdict::rank`] orders the four an open PR can
/// have; `Merged` and `Closed` end the question instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Checks passed for the head commit and nothing known stands in the way.
    Ready,
    /// Something will clear with time or a person: checks running, a review
    /// to give, a draft, a branch behind its base.
    Waiting,
    /// Fleet cannot tell: no evidence, an old reading, or a worktree on
    /// another commit than the PR.
    Unknown,
    /// Something must be done first: failing checks, uncommitted or unpushed
    /// work, requested changes, merge conflicts.
    Blocked,
    Merged,
    Closed,
}

impl Verdict {
    /// Severity among the open-PR verdicts; the worst reason decides.
    fn rank(self) -> u8 {
        match self {
            Verdict::Ready => 0,
            Verdict::Waiting => 1,
            Verdict::Unknown => 2,
            Verdict::Blocked => 3,
            Verdict::Merged | Verdict::Closed => 4,
        }
    }
}

/// One reason, in the order the card lists them (most decisive first). The
/// wording is the client's: each code names one condition of design §5, and
/// the numbers and names behind it are in the evidence itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// The reading is older than [`PR_EVIDENCE_STALE_SECS`].
    Stale,
    /// A PR, but no evidence: never probed since 082, an older hub, or a
    /// host whose `gh` answers only the basic fields.
    NoEvidence,
    Merged,
    Closed,
    /// Tracked files differ from the worktree's HEAD.
    Dirty,
    /// Commits on HEAD not on the upstream: what CI checked is not all the
    /// work there is.
    Unpushed,
    /// The worktree's HEAD is not the PR's head, and not because it is ahead.
    HeadMismatch,
    ChecksFailing,
    ChecksPending,
    /// The PR has no checks at all.
    NoChecks,
    Draft,
    ChangesRequested,
    ReviewRequired,
    /// `mergeStateStatus: DIRTY`.
    MergeConflicts,
    /// `mergeStateStatus: BEHIND`.
    Behind,
    /// `mergeStateStatus: BLOCKED` with no other reason to explain it
    /// (branch rules fleet does not read).
    MergeBlocked,
    /// Nothing stands in the way: the only reason a `Ready` verdict has.
    ChecksPassed,
}

impl Reason {
    /// The verdict this reason alone would give.
    pub fn verdict(self) -> Verdict {
        match self {
            Reason::Stale | Reason::NoEvidence | Reason::HeadMismatch => Verdict::Unknown,
            Reason::Merged => Verdict::Merged,
            Reason::Closed => Verdict::Closed,
            Reason::Dirty
            | Reason::Unpushed
            | Reason::ChecksFailing
            | Reason::ChangesRequested
            | Reason::MergeConflicts => Verdict::Blocked,
            Reason::ChecksPending
            | Reason::NoChecks
            | Reason::Draft
            | Reason::ReviewRequired
            | Reason::Behind
            | Reason::MergeBlocked => Verdict::Waiting,
            Reason::ChecksPassed => Verdict::Ready,
        }
    }
}

/// The assessment of one session's PR.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Assessment {
    pub verdict: Verdict,
    /// Never empty.
    pub reasons: Vec<Reason>,
    /// The commit the verdict is about: the PR's head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<i64>,
}

/// Whether a reading stamped `checked_at` describes the past at `now`. A
/// reading with no stamp is not called stale: it is `NoEvidence` instead.
pub fn is_stale(checked_at: Option<i64>, now: i64) -> bool {
    checked_at.is_some_and(|t| now - t > PR_EVIDENCE_STALE_SECS)
}

/// Assess one session's PR. `None` when the session has no PR: there is
/// nothing to assess, which is not the same as "unknown".
pub fn assess(
    pr_url: Option<&str>,
    evidence: Option<&PrEvidence>,
    checked_at: Option<i64>,
    now: i64,
) -> Option<Assessment> {
    pr_url?;
    let Some(ev) = evidence else {
        return Some(Assessment {
            verdict: Verdict::Unknown,
            reasons: vec![Reason::NoEvidence],
            commit: None,
            checked_at,
        });
    };
    let mut reasons = Vec::new();
    let unpushed = ev.ahead.is_some_and(|n| n > 0);

    // A finished PR ends the question; what is left in the worktree is
    // still worth saying (work that is not in what was merged).
    let finished = match ev.state.as_deref() {
        Some("MERGED") => Some(Reason::Merged),
        Some("CLOSED") => Some(Reason::Closed),
        _ => None,
    };
    if let Some(end) = finished {
        reasons.push(end);
        if ev.dirty == Some(true) {
            reasons.push(Reason::Dirty);
        }
        if unpushed {
            reasons.push(Reason::Unpushed);
        }
        return Some(Assessment {
            verdict: end.verdict(),
            reasons,
            commit: ev.head_oid.clone(),
            checked_at,
        });
    }

    let stale = is_stale(checked_at, now);
    if stale {
        reasons.push(Reason::Stale);
    }
    if ev.dirty == Some(true) {
        reasons.push(Reason::Dirty);
    }
    if unpushed {
        reasons.push(Reason::Unpushed);
    } else if let (Some(local), Some(head)) = (ev.local_head.as_deref(), ev.head_oid.as_deref()) {
        if local != head {
            reasons.push(Reason::HeadMismatch);
        }
    }
    let checks = &ev.checks;
    if checks.failing_total > 0 {
        reasons.push(Reason::ChecksFailing);
    }
    if checks.pending > 0 {
        reasons.push(Reason::ChecksPending);
    }
    if checks.total == 0 {
        reasons.push(Reason::NoChecks);
    }
    if ev.draft {
        reasons.push(Reason::Draft);
    }
    match ev.review_decision.as_deref() {
        Some("CHANGES_REQUESTED") => reasons.push(Reason::ChangesRequested),
        Some("REVIEW_REQUIRED") => reasons.push(Reason::ReviewRequired),
        _ => {}
    }
    match ev.merge_state.as_deref() {
        Some("DIRTY") => reasons.push(Reason::MergeConflicts),
        Some("BEHIND") => reasons.push(Reason::Behind),
        // Only when nothing above already explains it.
        Some("BLOCKED") if reasons.iter().all(|r| *r == Reason::Stale) => {
            reasons.push(Reason::MergeBlocked)
        }
        _ => {}
    }
    if reasons.iter().all(|r| *r == Reason::Stale) {
        reasons.push(Reason::ChecksPassed);
    }

    let worst = reasons
        .iter()
        .map(|r| r.verdict())
        .max_by_key(|v| v.rank())
        .unwrap_or(Verdict::Unknown);
    // An old reading is the past: never Ready, never a confident Blocked.
    let verdict = if stale { Verdict::Unknown } else { worst };
    Some(Assessment {
        verdict,
        reasons,
        commit: ev.head_oid.clone(),
        checked_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Case {
        name: String,
        now: i64,
        pr_url: Option<String>,
        evidence: Option<PrEvidence>,
        checked_at: Option<i64>,
        want: Option<Want>,
    }

    #[derive(serde::Deserialize)]
    struct Want {
        verdict: Verdict,
        reasons: Vec<Reason>,
    }

    fn cases() -> Vec<Case> {
        serde_json::from_str(include_str!("testdata/evidence_cases.json"))
            .expect("evidence_cases.json parses")
    }

    /// The shared fixture: `src/lib/evidence.test.ts` runs the same cases
    /// against the desktop's mirror.
    #[test]
    fn every_shared_case_assesses_as_the_fixture_says() {
        let cases = cases();
        assert!(cases.len() >= 20, "the fixture covers design §5");
        for c in cases {
            let got = assess(
                c.pr_url.as_deref(),
                c.evidence.as_ref(),
                c.checked_at,
                c.now,
            );
            match (&got, &c.want) {
                (None, None) => {}
                (Some(a), Some(w)) => {
                    assert_eq!(a.verdict, w.verdict, "{}: verdict", c.name);
                    assert_eq!(a.reasons, w.reasons, "{}: reasons", c.name);
                    assert_eq!(a.checked_at, c.checked_at, "{}", c.name);
                    assert_eq!(
                        a.commit,
                        c.evidence.as_ref().and_then(|e| e.head_oid.clone()),
                        "{}: the commit is the PR's head",
                        c.name
                    );
                }
                _ => panic!("{}: got {got:?}", c.name),
            }
        }
    }

    /// Every reason code appears in some case, so neither side can carry a
    /// rule the other never had to match.
    #[test]
    fn the_fixture_exercises_every_reason() {
        let all = [
            Reason::Stale,
            Reason::NoEvidence,
            Reason::Merged,
            Reason::Closed,
            Reason::Dirty,
            Reason::Unpushed,
            Reason::HeadMismatch,
            Reason::ChecksFailing,
            Reason::ChecksPending,
            Reason::NoChecks,
            Reason::Draft,
            Reason::ChangesRequested,
            Reason::ReviewRequired,
            Reason::MergeConflicts,
            Reason::Behind,
            Reason::MergeBlocked,
            Reason::ChecksPassed,
        ];
        let seen: Vec<Reason> = cases()
            .into_iter()
            .filter_map(|c| c.want)
            .flat_map(|w| w.reasons)
            .collect();
        for r in all {
            assert!(seen.contains(&r), "no fixture case gives {r:?}");
        }
    }

    #[test]
    fn staleness_is_strictly_older_than_the_threshold() {
        assert!(!is_stale(None, 10_000));
        assert!(!is_stale(Some(10_000 - PR_EVIDENCE_STALE_SECS), 10_000));
        assert!(is_stale(Some(10_000 - PR_EVIDENCE_STALE_SECS - 1), 10_000));
    }

    #[test]
    fn the_wire_spelling_is_snake_case() {
        let a = Assessment {
            verdict: Verdict::Blocked,
            reasons: vec![Reason::ChecksFailing, Reason::HeadMismatch],
            commit: Some("abc1234".into()),
            checked_at: Some(1),
        };
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::json!({
                "verdict": "blocked",
                "reasons": ["checks_failing", "head_mismatch"],
                "commit": "abc1234",
                "checked_at": 1
            })
        );
    }
}
