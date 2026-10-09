//! `pr_triage` (PR shepherd step 4): what a shepherd episode most likely
//! needs, asked once per episode as a closed-set question, so the shadow
//! comparison can show whether a model would pick a better prompt than the
//! shepherd's fixed one.
//!
//! * **When.** Right after the shepherd records a new episode
//!   (`service/pr_shepherd`), once per `(session, head, condition)`, off the
//!   shepherd's own path (the episode never waits for it).
//! * **What is sent.** The PR's reading only: the condition, GitHub's merge
//!   state, the failing checks' names and counts, the review decision and
//!   whether the branch has unpushed commits. No log, no diff, no code. The
//!   names go through the envelope's redaction.
//! * **Baseline.** What the shepherd itself does for that condition
//!   ([`rule_choice`]): a conflict or a stale branch gets `merge_base`, red
//!   CI gets `fix_in_pr`.
//! * **Off** asks nothing. **Shadow** records the answer beside the
//!   baseline. **Assist** is recorded the same way for now: it would only
//!   choose which prompt the shepherd sends, and never sends, merges, skips
//!   or re-runs anything itself. That wiring is a later step.

use super::{decide, DecideCtx, DecideRequest, Feature, FeatureMode, JevRequest, Question};
use crate::service::outcome::PrEvidence;
use crate::service::pr_shepherd::Condition;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "pr_triage.v1";
/// What a run is about: the session that opened the PR (its id as text).
pub const SUBJECT_KIND: &str = "session";
/// Most failing check names sent.
pub const CHECK_NAMES_MAX: usize = 10;

/// The instruction, read literally.
pub const INSTRUCTIONS: &str = "state describes a GitHub pull request that a coding agent \
    opened and that is not mergeable yet: state.condition is why (conflict, behind or ci_red), \
    state.merge_state is GitHub's mergeStateStatus, state.failing_checks names the failing CI \
    checks. Choose the one next step most likely to make it mergeable.";

/// The options, in a stable order: each key is recorded as is.
pub const OPTIONS: [(&str, &str); 6] = [
    (
        "fix_in_pr",
        "The pull request's own change is wrong: read the failure, fix the code, push.",
    ),
    (
        "regenerate",
        "A generated or derived file (docs, counts, lockfile, snapshot) is out of date: \
         regenerate it with the repository's tooling and push.",
    ),
    (
        "merge_base",
        "The branch conflicts with or lags its base: merge the base branch in, resolve, push.",
    ),
    (
        "flaky_rerun",
        "A timing-dependent or infrastructure failure unrelated to the change: re-run the check once.",
    ),
    (
        "not_this_pr",
        "The base branch is broken the same way: wait for or port the base branch's fix.",
    ),
    (
        "needs_person",
        "Needs a person's decision: a design question, a review, a secret or a permission.",
    ),
];

/// PURE: whether `word` is one of the options.
pub fn is_option(word: &str) -> bool {
    OPTIONS.iter().any(|(k, _)| *k == word)
}

/// PURE: what the shepherd does for `condition` without a model.
pub fn rule_choice(condition: Condition) -> &'static str {
    match condition {
        Condition::Conflict | Condition::Behind => "merge_base",
        Condition::CiRed => "fix_in_pr",
    }
}

/// PURE: the question.
pub fn question() -> Question {
    Question::Choice {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria: OPTIONS
            .iter()
            .map(|(k, d)| (k.to_string(), Some(Value::String(d.to_string()))))
            .collect::<BTreeMap<_, _>>(),
    }
}

/// PURE: the request for one episode.
pub fn request(condition: Condition, ev: &PrEvidence) -> JevRequest {
    let names: Vec<&str> = ev
        .checks
        .failing
        .iter()
        .take(CHECK_NAMES_MAX)
        .map(|c| c.name.as_str())
        .collect();
    JevRequest {
        state: json!({
            "condition": condition.as_str(),
            "merge_state": ev.merge_state.as_deref().unwrap_or("UNKNOWN"),
            "failing_checks": names,
            "failing_total": ev.checks.failing_total,
            "checks_total": ev.checks.total,
            "checks_pending": ev.checks.pending,
            "review_decision": ev.review_decision.as_deref().unwrap_or(""),
            "draft": ev.draft,
            "unpushed_commits": ev.ahead.unwrap_or(0),
        }),
        question: question(),
    }
}

/// Ask about one episode. Asks nothing when the feature is off; never
/// errors. Returns the model's option when there is a usable one (recorded
/// either way; nothing acts on it yet).
pub async fn triage(
    ctx: &DecideCtx,
    session_id: i64,
    org_id: Option<i64>,
    condition: Condition,
    evidence: &PrEvidence,
) -> Option<String> {
    let mode = match ctx.store.lock() {
        Ok(s) => FeatureMode::of(&s, Feature::PrTriage),
        Err(_) => return None,
    };
    if mode == FeatureMode::Off {
        return None;
    }
    let outcome = decide(
        ctx,
        DecideRequest {
            feature: Feature::PrTriage,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: session_id.to_string(),
            org_id,
            request: request(condition, evidence),
            baseline: Some(rule_choice(condition).to_string()),
            question_version: QUESTION_VERSION.into(),
            min_confidence: None,
        },
    )
    .await;
    outcome
        .usable()
        .map(|a| a.value.clone())
        .filter(|v| is_option(v))
}
