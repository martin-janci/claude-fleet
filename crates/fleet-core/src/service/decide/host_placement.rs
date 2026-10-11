//! N5 `host_placement`: which host a project's new session runs on, when
//! neither a rule nor the numbers decide (Orbit Fleet redesign step 4.11,
//! the plan's "Default host when no rule decides").
//!
//! Host choice goes by rules and numbers first (the never-decides list's
//! `host_by_numbers`): the New session dialog keeps a host it remembers for
//! the project (that is the project's rule), and the numbers take every host
//! that is offline, hidden, outside the project's org, or whose own account
//! is past `accounts.pause_at` (step 4.4) out of the running. Only when two
//! or more hosts are left is the decision model asked one Choice over them:
//!
//! * **Shadow / assist only.** In `shadow` the question is asked off the
//!   dialog's path and only recorded, with the first candidate (the order
//!   the dialog's default host follows: `local` first) as the baseline. In
//!   `assist` the dialog waits for the one bounded call and a usable answer
//!   pre-selects the host with the shared ProposedBy chip; a person still
//!   presses Create. A host over its limit or offline is never a candidate,
//!   so it is never proposed.
//! * **What is sent.** The project's `owner/repo` and, per candidate host,
//!   bucketed numbers: live sessions, this project's starts there in the
//!   last [`RECENT_DAYS`] days, whether it is checked out there, free disk,
//!   latency and how much of its account's limit is used. Host aliases are
//!   option words; nothing from the hosts' files.
//! * **What is recorded.** A run about subject `project_start`
//!   `project:<id>`; the options are `h:<alias>` and `unsure`.
//! * **Asked once per input.** A decided run on the same subject, input
//!   fingerprint, question version, mode and pinned model in the last
//!   [`REASK_DAYS`] days is reused. The numbers are bucketed so a session
//!   more or less on a busy host does not ask again.
//! * **Follow-up.** When a person's start of the project lands on a host
//!   ([`record_start`]), the latest assist proposal nobody decided is marked
//!   `confirmed` or `corrected`. A shadow answer is never marked.

use std::collections::BTreeMap;
use std::sync::Mutex;

use super::{
    consents, decide, fingerprint, gate_at, DecideCtx, DecideRequest, Feature, JevRequest, Mode,
    Question,
};
use crate::ipc_error::{lock, IpcError};
use crate::service::account_limits;
use crate::service::account_usage::{AccountUsageSnapshot, UsageCache};
use crate::service::settings;
use crate::store::{
    is_decision_word, org_of_session, DecisionRunRow, HostRow, SessionOrgFacts, Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "host_placement.v1";
/// What a run is about: one project's next start.
pub const SUBJECT_KIND: &str = "project_start";
/// Below this confidence an answer is recorded, not suggested.
pub const MIN_CONFIDENCE: f64 = 0.5;
/// The option that suggests nothing.
pub const UNSURE: &str = "unsure";
/// A run decided this recently, on the same input, is reused.
pub const REASK_DAYS: i64 = 7;
/// The window "starts of this project on the host" counts.
pub const RECENT_DAYS: i64 = 30;
/// The host alias that is this machine.
const LOCAL: &str = "local";

/// The instruction, read literally: the exact condition and what to use.
pub const INSTRUCTIONS: &str = "state.project is one git repository, named owner/repo, that a \
     person is about to start a coding session in. Each option except unsure is a machine (a \
     host) that can run the session; state.hosts gives, per option, how many of this \
     project's sessions were started there in the last 30 days, whether the repository is \
     already checked out there, how many sessions run there now, its free disk in percent, \
     its network latency in milliseconds and how much of its account's usage limit is used in \
     percent. Decide which host the person would pick for this project. Choose unsure when no \
     host is clearly the one.";

/// One candidate host and the numbers the question carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub alias: String,
    /// Sessions this project started there in the last [`RECENT_DAYS`] days.
    pub recent_starts: i64,
    /// Whether the project has a checkout (a worktree row) there.
    pub checked_out: bool,
    /// Live sessions on the host now.
    pub live_sessions: i64,
    /// Free home disk, in percent rounded down to a tenth.
    pub disk_free_pct: Option<u8>,
    /// Latency, rounded to 50 ms.
    pub latency_ms: Option<i64>,
    /// The host's own account, percent of its tighter window used, rounded
    /// down to a tenth.
    pub account_used_pct: Option<u8>,
}

/// What the adapter asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementInput {
    pub project_id: i64,
    pub owner: String,
    pub repo: String,
    /// The project's org (its consent is checked); `None` when it has none.
    pub org_id: Option<i64>,
    /// The hosts the numbers leave, `local` first, then by alias.
    pub candidates: Vec<Candidate>,
}

/// The host the model proposes, for the dialog to pre-select.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestedHost {
    pub host_alias: String,
    /// The model's confidence, in whole percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_pct: Option<u8>,
    /// The recorded run (what a follow-up marks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<i64>,
}

/// PURE: a confidence in whole percent.
fn pct(c: Option<f64>) -> Option<u8> {
    c.map(|c| (c.clamp(0.0, 1.0) * 100.0).round() as u8)
}

/// PURE: a host's option word.
pub fn option_of(alias: &str) -> String {
    format!("h:{alias}")
}

/// PURE: the host an option names; `None` for `unsure` or anything else.
pub fn host_of(option: &str) -> Option<&str> {
    option.strip_prefix("h:").filter(|a| !a.is_empty())
}

/// PURE: down to a tenth of a percent scale (`37.9` → `30`).
fn tenth(v: f64) -> u8 {
    ((v.clamp(0.0, 100.0) / 10.0).floor() * 10.0) as u8
}

/// PURE: whether `h` is up: visible, and reachable unless it is `local`
/// (the dialog's `isPickableHost`).
pub fn online(h: &HostRow) -> bool {
    !h.hidden && (h.reachable || h.alias == LOCAL)
}

/// The project's org: the most specific rule on its owner, repo or path
/// alone (a host-only rule names a host, not the project).
pub fn project_org(
    s: &Store,
    owner: &str,
    repo: &str,
    path: &str,
) -> Result<Option<i64>, IpcError> {
    let facts = SessionOrgFacts {
        host_alias: "",
        owner: Some(owner),
        repo: Some(repo),
        path: Some(path),
    };
    let rules: Vec<_> = s
        .list_org_rules()?
        .into_iter()
        .filter(|r| r.host_alias.is_none())
        .collect();
    Ok(org_of_session(&facts, &rules, None))
}

/// PURE: the hosts the numbers leave, in the dialog's order (`local`
/// first, then by alias): online, an option word, inside the project's org
/// (`host_org` answers the org a session there would have; with no project
/// org every host is in), and not past `pause_at_pct` on its own account.
#[allow(clippy::too_many_arguments)]
pub fn candidates(
    hosts: &[HostRow],
    usage: &[AccountUsageSnapshot],
    pause_at_pct: f64,
    counts: &BTreeMap<String, (i64, i64, bool)>,
    project_org: Option<i64>,
    host_org: impl Fn(&str) -> Option<i64>,
    now: i64,
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = hosts
        .iter()
        .filter(|h| online(h) && is_decision_word(&option_of(&h.alias)))
        .filter(|h| project_org.is_none() || host_org(&h.alias) == project_org)
        .filter_map(|h| {
            let own = account_limits::headroom(h, None, usage, pause_at_pct, now);
            if own.over {
                return None;
            }
            let (live, recent, checked_out) = counts.get(&h.alias).copied().unwrap_or_default();
            let disk_free_pct = match (h.disk_home_free_kb, h.disk_home_total_kb) {
                (Some(free), Some(total)) if total > 0 => {
                    Some(tenth(free as f64 * 100.0 / total as f64))
                }
                _ => None,
            };
            Some(Candidate {
                alias: h.alias.clone(),
                recent_starts: recent,
                checked_out,
                live_sessions: live,
                disk_free_pct,
                latency_ms: h.latency_ms.map(|l| (l.max(0) + 25) / 50 * 50),
                account_used_pct: own.chosen.and_then(|c| c.used_pct).map(tenth),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        (b.alias == LOCAL)
            .cmp(&(a.alias == LOCAL))
            .then(a.alias.cmp(&b.alias))
    });
    out
}

/// PURE: whether `c`'s home disk is at or past `health.disk_low_pct` used,
/// the rule that raises `disk_low`: such a host is the numbers' answer
/// (not here), never Jev's to weigh (review r15).
pub fn past_disk_rule(c: &Candidate, disk_low_pct: u8) -> bool {
    c.disk_free_pct
        .is_some_and(|free| 100u16.saturating_sub(u16::from(free)) >= u16::from(disk_low_pct))
}

/// PURE: the question over `candidates`.
pub fn question(candidates: &[Candidate]) -> Question {
    let mut criteria: Vec<(String, Option<Value>)> = candidates
        .iter()
        .map(|c| {
            (
                option_of(&c.alias),
                Some(Value::String(format!(
                    "The session runs on the host {}.",
                    c.alias
                ))),
            )
        })
        .collect();
    criteria.push((
        UNSURE.to_string(),
        Some(Value::String(
            "No host is clearly the one for this project.".into(),
        )),
    ));
    Question::Choice {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria: criteria.into_iter().collect(),
    }
}

/// PURE: the state: the project's name and each candidate's numbers.
pub fn state(input: &PlacementInput) -> Value {
    let hosts: serde_json::Map<String, Value> = input
        .candidates
        .iter()
        .map(|c| {
            (
                option_of(&c.alias),
                json!({
                    "recent_starts": c.recent_starts,
                    "checked_out": c.checked_out,
                    "live_sessions": c.live_sessions,
                    "disk_free_pct": c.disk_free_pct,
                    "latency_ms": c.latency_ms,
                    "account_used_pct": c.account_used_pct,
                }),
            )
        })
        .collect();
    json!({
        "project": format!("{}/{}", input.owner, input.repo),
        "hosts": hosts,
    })
}

/// PURE: the request for `input`.
pub fn question_for(input: &PlacementInput) -> JevRequest {
    JevRequest {
        state: state(input),
        question: question(&input.candidates),
    }
}

/// PURE: the run's subject id.
pub fn subject_id(project_id: i64) -> String {
    format!("project:{project_id}")
}

/// A run that sent a request and got an answer the envelope checked (or
/// found wanting): asking again on the same input would repeat it.
fn decided(r: &DecisionRunRow) -> bool {
    r.called
        && matches!(
            r.fallback.as_deref(),
            None | Some("low_confidence") | Some("invalid_answer")
        )
}

/// PURE: what a run proposes: an answered run naming one of `candidates`.
fn proposed(r: &DecisionRunRow, candidates: &[Candidate]) -> Option<SuggestedHost> {
    if r.fallback.is_some() || r.followup.as_deref() == Some("rejected") {
        return None;
    }
    let alias = host_of(r.answer.as_deref()?)?;
    candidates
        .iter()
        .any(|c| c.alias == alias)
        .then(|| SuggestedHost {
            host_alias: alias.to_string(),
            confidence_pct: pct(r.confidence),
            run_id: Some(r.id),
        })
}

enum Plan {
    Skip,
    Reuse(Option<SuggestedHost>),
    Ask {
        subject: String,
        request: JevRequest,
    },
}

fn plan(s: &Store, input: &PlacementInput, now: i64) -> Result<Plan, IpcError> {
    // One host or none: the numbers decided.
    if input.candidates.len() < 2 {
        return Ok(Plan::Skip);
    }
    let Ok(mode) = gate_at(s, Feature::HostPlacement, input.org_id, now) else {
        return Ok(Plan::Skip);
    };
    let fp_key = s.decision_fp_key()?;
    let subject = subject_id(input.project_id);
    let request = question_for(input);
    let fp = fingerprint(&fp_key, &request.redacted());
    let model = settings::get_string(s, settings::DECIDE_JEV_MODEL);
    let since = now - REASK_DAYS * 86_400;
    let runs = s.decision_runs_for_subjects(
        Feature::HostPlacement.as_str(),
        SUBJECT_KIND,
        &subject,
        Some(since),
    )?;
    let recent = runs.iter().find(|r| {
        r.subject_id == subject
            && r.input_fp.as_deref() == Some(fp.as_str())
            && r.question_version == QUESTION_VERSION
            && r.mode == mode.as_str()
            && decided(r)
            && (model == "jev-latest" || r.model_version.as_deref() == Some(model.as_str()))
    });
    if let Some(r) = recent {
        let reuse = (mode == Mode::Assist)
            .then(|| proposed(r, &input.candidates))
            .flatten();
        return Ok(Plan::Reuse(reuse));
    }
    Ok(Plan::Ask { subject, request })
}

/// Ask (or reuse) the decision model's host for `input`. `Some` only in
/// `assist`, with a usable answer naming a candidate. Every failure is a
/// recorded fallback or nothing at all, never an error: a start never
/// depends on this. Holds the store lock only for reads, never across the
/// call.
pub async fn ask(ctx: &DecideCtx, input: &PlacementInput) -> Option<SuggestedHost> {
    let now = ctx.now();
    let planned = {
        let s = lock(&ctx.store).ok()?;
        match plan(&s, input, now) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("[decide] host_placement not asked: {}", e.message);
                return None;
            }
        }
    };
    let (subject, request) = match planned {
        Plan::Skip => return None,
        Plan::Reuse(s) => return s,
        Plan::Ask { subject, request } => (subject, request),
    };
    let out = decide(
        ctx,
        DecideRequest {
            feature: Feature::HostPlacement,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: subject.clone(),
            org_id: input.org_id,
            request,
            baseline: input.candidates.first().map(|c| option_of(&c.alias)),
            question_version: QUESTION_VERSION.into(),
            min_confidence: Some(MIN_CONFIDENCE),
        },
    )
    .await;
    let answer = out.proposal()?;
    let alias = host_of(&answer.value)?;
    if !input.candidates.iter().any(|c| c.alias == alias) {
        return None;
    }
    // A newer proposal takes the place of an older one nobody decided.
    if let (Some(id), Ok(s)) = (out.run_id, lock(&ctx.store)) {
        if let Err(e) = s.supersede_decision_runs(
            Feature::HostPlacement.as_str(),
            SUBJECT_KIND,
            &subject,
            Mode::Assist.as_str(),
            id,
            now,
        ) {
            tracing::warn!("[decide] ignored follow-up not recorded: {}", e.message);
        }
    }
    Some(SuggestedHost {
        host_alias: alias.to_string(),
        confidence_pct: pct(answer.confidence),
        run_id: out.run_id,
    })
}

/// The mode a dialog would ask `input` in, under one short lock: `None`
/// when the gate refuses or the numbers decided (nothing to ask).
pub fn mode_for(ctx: &DecideCtx, input: &PlacementInput) -> Option<Mode> {
    if input.candidates.len() < 2 {
        return None;
    }
    let s = lock(&ctx.store).ok()?;
    gate_at(&s, Feature::HostPlacement, input.org_id, ctx.now()).ok()
}

/// The input for `project_id` from the store and the usage cache. `None`
/// for an unknown project.
pub fn input_for(
    store: &Mutex<Store>,
    cache: &Mutex<UsageCache>,
    project_id: i64,
    now: i64,
) -> Result<Option<PlacementInput>, IpcError> {
    input_with(store, project_id, now, |accounts| {
        let c = cache.lock().unwrap_or_else(|e| e.into_inner());
        accounts.iter().map(|a| c.snapshot(&a.uuid)).collect()
    })
}

/// [`input_for`] from the usage the hub's bus followed (what its
/// `account_usage` tool serves), for the hub's own `propose_host_placement`
/// tool: an account the bus has no answer for reads as never fetched.
pub fn served_input_for(
    store: &Mutex<Store>,
    project_id: i64,
    now: i64,
) -> Result<Option<PlacementInput>, IpcError> {
    let served = lock(store)?.bus_account_usage();
    input_with(store, project_id, now, |accounts| {
        accounts
            .iter()
            .map(|a| {
                served
                    .iter()
                    .find(|u| u.account_uuid == a.uuid)
                    .cloned()
                    .unwrap_or_else(|| AccountUsageSnapshot::never_fetched(&a.uuid))
            })
            .collect()
    })
}

fn input_with(
    store: &Mutex<Store>,
    project_id: i64,
    now: i64,
    usage_of: impl FnOnce(&[crate::store::AccountRow]) -> Vec<AccountUsageSnapshot>,
) -> Result<Option<PlacementInput>, IpcError> {
    let (project, hosts, pause_at, accounts, counts, org, host_orgs, disk_low_pct, fenced) = {
        let s = lock(store)?;
        let Some(project) = s.get_project(project_id)? else {
            return Ok(None);
        };
        let hosts = s.list_hosts()?;
        let org = project_org(&s, &project.owner, &project.repo, &project.base_path)?;
        let mut host_orgs = BTreeMap::new();
        // Review r15: with no project org every host is a candidate, but a
        // host bound to an org that did not consent (D31) is never named
        // to the model; the person can still pick it.
        let mut fenced = std::collections::BTreeSet::new();
        for h in &hosts {
            let host_org = s.org_for_new_session(&h.alias, project_id)?;
            if org.is_none()
                && host_org.is_some()
                && !consents(&s, Feature::HostPlacement, host_org)
            {
                fenced.insert(h.alias.clone());
            }
            if org.is_some() {
                host_orgs.insert(h.alias.clone(), host_org);
            }
        }
        (
            project,
            hosts,
            account_limits::pause_at_pct(&s),
            s.list_accounts()?,
            s.host_placement_counts(project_id, now - RECENT_DAYS * 86_400)?,
            org,
            host_orgs,
            crate::service::health::host_thresholds(&s).disk_low_pct,
            fenced,
        )
    };
    let usage: Vec<AccountUsageSnapshot> = usage_of(&accounts);
    let mut candidates = candidates(
        &hosts,
        &usage,
        pause_at,
        &counts,
        org,
        |alias| host_orgs.get(alias).copied().flatten(),
        now,
    );
    candidates.retain(|c| !fenced.contains(&c.alias) && !past_disk_rule(c, disk_low_pct));
    Ok(Some(PlacementInput {
        project_id,
        candidates,
        owner: project.owner,
        repo: project.repo,
        org_id: org,
    }))
}

#[derive(Debug, Clone, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "ProposeHostPlacementParams")]
pub struct ProposeHostArgs {
    /// The project the new session starts in.
    pub project_id: i64,
}

/// `propose_host_placement`: the decision model's host for a new session
/// of the project, in `assist`; in `shadow` asked off this path and only
/// recorded. `None` with the defaults, for one candidate or none, and
/// whenever the model is unsure or under the floor.
pub async fn propose(
    ctx: &DecideCtx,
    cache: &Mutex<UsageCache>,
    args: &ProposeHostArgs,
) -> Result<Option<SuggestedHost>, IpcError> {
    let input = input_for(&ctx.store, cache, args.project_id, ctx.now())?;
    propose_from(ctx, input).await
}

/// [`propose`] on the hub, from the usage its bus followed: the
/// `propose_host_placement` tool a phone calls (gap plan G7.3).
pub async fn propose_served(
    ctx: &DecideCtx,
    args: &ProposeHostArgs,
) -> Result<Option<SuggestedHost>, IpcError> {
    let input = served_input_for(&ctx.store, args.project_id, ctx.now())?;
    propose_from(ctx, input).await
}

async fn propose_from(
    ctx: &DecideCtx,
    input: Option<PlacementInput>,
) -> Result<Option<SuggestedHost>, IpcError> {
    let Some(input) = input else {
        return Ok(None);
    };
    Ok(match mode_for(ctx, &input) {
        Some(Mode::Assist) => ask(ctx, &input).await,
        Some(Mode::Shadow) => {
            let ctx = ctx.clone();
            tokio::spawn(async move {
                ask(&ctx, &input).await;
            });
            None
        }
        None => None,
    })
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecordHostStartArgs {
    pub project_id: i64,
    pub host_alias: String,
}

/// After a PERSON's start of `project_id` landed on `host_alias`: mark the
/// latest assist proposal nobody decided `confirmed` (the same host) or
/// `corrected` (to theirs). Returns whether a run was marked. Never marks a
/// shadow answer, and never a run that already has a follow-up.
pub fn record_start(
    s: &Store,
    project_id: i64,
    host_alias: &str,
    now: i64,
) -> Result<bool, IpcError> {
    crate::validate::host_alias(host_alias)?;
    let subject = subject_id(project_id);
    let runs = s.decision_runs_for_subjects(
        Feature::HostPlacement.as_str(),
        SUBJECT_KIND,
        &subject,
        None,
    )?;
    let Some(r) = runs.iter().find(|r| {
        r.subject_id == subject
            && r.mode == Mode::Assist.as_str()
            && r.fallback.is_none()
            && r.answer.as_deref().and_then(host_of).is_some()
    }) else {
        return Ok(false);
    };
    if r.followup.is_some() {
        return Ok(false);
    }
    let theirs = option_of(host_alias);
    if !is_decision_word(&theirs) {
        return Ok(false);
    }
    if r.answer.as_deref() == Some(theirs.as_str()) {
        s.set_decision_followup(r.id, "confirmed", None, now)
    } else {
        s.set_decision_followup(r.id, "corrected", Some(&theirs), now)
    }
}
