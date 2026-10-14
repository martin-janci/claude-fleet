//! `limit_switch`: which login an out-of-credit session resumes under when
//! `accounts.auto_switch` switches it (`service::account_switch`).
//!
//! * **When.** Once per automatic switch with two or more candidate logins,
//!   right before the restart. One candidate decides itself and asks
//!   nothing.
//! * **What is sent.** Per candidate login: how much of its account's
//!   tighter window is used, bucketed to a tenth, and whether it is the
//!   host's own login. Option words are `own` and `p:<profile>`; no account
//!   name, email or uuid is sent.
//! * **Baseline.** The rule's pick: the login with the most headroom
//!   ([`super::super::account_switch::pick`]).
//! * **Off** asks nothing. **Shadow** records the answer beside the
//!   baseline. **Assist** is recorded the same way for now: the switch is
//!   automatic, and a model answer never runs an action on its own (D35),
//!   so the rule's pick is used until the comparison earns Jev the choice.

use super::{decide, DecideCtx, DecideRequest, Feature, FeatureMode, JevRequest, Question};
use crate::service::account_limits::HostLogin;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "limit_switch.v1";
/// What a run is about: the session being switched (its id as text).
pub const SUBJECT_KIND: &str = "session";
/// The host's own login's option word.
pub const OWN: &str = "own";

/// The instruction, read literally.
pub const INSTRUCTIONS: &str = "A coding session's Claude account ran out of usage, and \
     the session will be resumed under another login on the same machine. Each option is \
     one such login; state.logins gives, per option, the percent of its account's usage \
     limit already used and whether it is the machine's own login. Choose the login the \
     session should continue under so that it runs the longest without hitting a limit \
     again.";

/// PURE: a login's option word.
pub fn option_of(login: &HostLogin) -> String {
    match &login.profile {
        Some(p) => format!("p:{p}"),
        None => OWN.to_string(),
    }
}

/// PURE: down to a tenth of a percent scale (`37.9` → `30`).
fn tenth(v: f64) -> u8 {
    ((v.clamp(0.0, 100.0) / 10.0).floor() * 10.0) as u8
}

/// PURE: the question over `candidates`.
pub fn question(candidates: &[HostLogin]) -> Question {
    Question::Choice {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria: candidates
            .iter()
            .map(|l| (option_of(l), None))
            .collect::<BTreeMap<_, _>>(),
    }
}

/// PURE: the request for one switch.
pub fn request(candidates: &[HostLogin]) -> JevRequest {
    let logins: BTreeMap<String, Value> = candidates
        .iter()
        .map(|l| {
            (
                option_of(l),
                json!({
                    "used_pct": l.used_pct.map(tenth),
                    "machine_login": l.profile.is_none(),
                }),
            )
        })
        .collect();
    JevRequest {
        state: json!({ "logins": logins }),
        question: question(candidates),
        order_salt: None,
    }
}

/// Ask about one switch. Asks nothing when the feature is off or there is
/// nothing to choose; never errors. Returns the model's option when there is
/// a usable one among `candidates` (recorded either way; nothing acts on it
/// yet).
pub async fn shadow(
    ctx: &DecideCtx,
    session_id: i64,
    org_id: Option<i64>,
    candidates: &[HostLogin],
    baseline: &HostLogin,
) -> Option<String> {
    if candidates.len() < 2 {
        return None;
    }
    let mode = match ctx.store.lock() {
        Ok(s) => FeatureMode::of(&s, Feature::LimitSwitch),
        Err(_) => return None,
    };
    if mode == FeatureMode::Off {
        return None;
    }
    let outcome = decide(
        ctx,
        DecideRequest {
            feature: Feature::LimitSwitch,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: session_id.to_string(),
            org_id,
            request: request(candidates),
            baseline: Some(option_of(baseline)),
            question_version: QUESTION_VERSION.into(),
            min_confidence: None,
        },
    )
    .await;
    let words: Vec<String> = candidates.iter().map(option_of).collect();
    outcome
        .usable()
        .map(|a| a.value.clone())
        .filter(|v| words.contains(v))
}
