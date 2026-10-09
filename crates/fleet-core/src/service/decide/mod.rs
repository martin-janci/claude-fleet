//! The decision envelope (Jev evaluation, decisions D31 and D35–D37): the one
//! path by which fleet may ask a decision model — TypeSafe AI's Jev — a
//! closed-set question. The guide is `docs/decisions.md`; the decisions are
//! in `docs/superpowers/specs/2026-09-27-jev-language-census-design.md`.
//!
//! **Off by default, twice over.** With the default settings no call can
//! happen: [`gate`] refuses with [`Fallback::FlagOff`] (and every feature's
//! mode is `off`, every org's consent is off, no key is set).
//!
//! What the envelope does, for every use case alike (D35); a use case is a
//! thin adapter that builds a [`JevRequest`] and reads the [`DecisionOutcome`]:
//!
//! 1. **Gate** ([`gate`]): only the process that owns the fleet (a hub or a
//!    standalone desktop — never a desktop paired to a hub); the kill switch
//!    `decide.jev.enabled`; the feature's mode `decide.jev.<feature>`
//!    (`off | shadow | assist`); the org's consent (`orgs.jev_allowed`, or
//!    `decide.jev.unassigned` for a subject with no org); a configured key;
//!    the circuit breaker; the daily token budget.
//! 2. **Redact** ([`redact_state`] on every text value of the state and the
//!    question) — what is fingerprinted and what is sent.
//! 3. **Call** once, bounded by `decide.jev.timeout_ms`, never retried on
//!    the caller's path; the answer is checked against the question
//!    ([`jev::validate_answer`]) and, if the adapter asked, against its
//!    confidence floor.
//! 4. **Record** one `decision_runs` row in EVERY case, fallbacks included,
//!    so a shadow comparison sees coverage. Ids, vocabulary words, numbers
//!    and an HMAC fingerprint only — never raw text.
//!
//! A model answer never grants a permission and never runs a risky action:
//! in `shadow` the adapter only records; in `assist` it may *propose*
//! (pre-select, suggest) and a person confirms. `auto` is not offered.
//!
//! No `.await` holds the store lock: the gate and the record each take it
//! briefly; the call runs without it.

#[cfg(feature = "nl-detect")]
pub mod bench;
pub mod control_route;
#[cfg(test)]
mod control_route_tests;
pub mod duplicate;
#[cfg(test)]
mod duplicate_tests;
pub mod haiku;
pub mod host_placement;
#[cfg(test)]
mod host_placement_tests;
pub mod jev;
pub mod lost_target;
#[cfg(test)]
mod lost_target_tests;
pub mod main_ticket;
#[cfg(test)]
mod main_ticket_tests;
pub mod mission_triage;
#[cfg(test)]
mod mission_triage_tests;
pub mod pr_triage;
#[cfg(test)]
mod pr_triage_tests;
pub mod quick_answer;
#[cfg(test)]
mod quick_answer_tests;
pub mod related_session;
#[cfg(test)]
mod related_session_tests;
pub mod routine_run_outcome;
#[cfg(test)]
mod routine_run_outcome_tests;
pub mod sibling_repos;
#[cfg(test)]
mod sibling_repos_tests;
pub mod start_project;
#[cfg(test)]
mod start_project_tests;
pub mod status_map;
pub mod summary_check;
#[cfg(test)]
mod summary_check_tests;
#[cfg(test)]
mod tests;
pub mod tracker_duplicate;
#[cfg(test)]
mod tracker_duplicate_tests;
pub mod turn_outcome;
#[cfg(test)]
mod turn_outcome_tests;
pub mod work_link;
#[cfg(test)]
mod work_link_tests;
pub mod work_placement;
#[cfg(test)]
mod work_placement_tests;

pub use jev::{
    Answer, BackendError, DecisionBackend, JevBackend, JevRequest, JevResponse, NoulCriteria,
    Question, Usage, ValidAnswer, JEV_HOST, JEV_URL, PROVIDER_JEV,
};

use crate::ipc_error::{lock, IpcError};
use crate::service::settings;
use crate::store::{
    is_decision_word, DecisionKeyStatus, DecisionStatRow, NewDecisionRun, RunScope, Secret, Store,
    DECISION_BENCH_SUBJECT,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

/// The setting a desktop paired to a hub carries (`src-tauri`'s
/// `backend::REMOTE_URL_KEY` is this constant): such a process is a window
/// onto someone else's fleet and never calls out (D35).
pub const HUB_REMOTE_URL_KEY: &str = "hub.remote_url";

/// Rows the retention sweep deletes per lock, and per sweep.
pub const RETENTION_BATCH: usize = 500;
pub const RETENTION_TICK_CAP: usize = 5_000;

/// A use case of the envelope. Each has its own mode setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    /// Proposing a status category for an Asana section.
    StatusMap,
    /// Choosing a work item for a session no rule could link.
    WorkLink,
    /// Pre-selecting the repository of a task's first start (K1).
    StartProject,
    /// Pre-ticking the sibling repository a ticket start also needs (N3).
    SiblingRepos,
    /// Pre-selecting the host of a project's new session when no rule or
    /// number decides (N5, redesign step 4.11).
    HostPlacement,
    /// The likely option first in an agent's question or a form (J5).
    QuickAnswer,
    /// Prefilling the project of a pane fleet did not start (N4, Adopt).
    AdoptTarget,
    /// Prefilling the project of a found conversation (J10, Restore).
    RestoreTarget,
    /// Flagging a proposed task that may duplicate an existing one (K4).
    Duplicate,
    /// Proposing a Work-view group for a task nobody placed (K5).
    WorkPlacement,
    /// Noticing another session of the same person on the same work (N1).
    RelatedSession,
    /// Where a message typed in Control goes: a mission, a session, or
    /// Control itself (K2, redesign step 9.9).
    ControlRoute,
    /// Checking a watcher's "Since 13:20" summary against its transcript
    /// before it shows (J9, redesign step 11.11).
    SummaryCheck,
    /// What a turn came to when hooks said nothing, from the pane tail (J2).
    TurnOutcome,
    /// A stuck mission's outcome and next step (K3, redesign 9.10).
    MissionTriage,
    /// What a finished routine run came to when its exit and the rules say
    /// nothing, from the pane tail (N6, redesign step 8.10).
    RoutineRunOutcome,
    /// What a PR shepherd episode most likely needs (PR shepherd step 4).
    PrTriage,
    /// The main ticket among several keys a session's prompt names (J6,
    /// redesign step 6.8).
    MainTicket,
    /// A local task that may be the same work as a tracker ticket (J7,
    /// redesign step 6.8).
    TrackerDuplicate,
}

impl Feature {
    pub const ALL: &[Feature] = &[
        Feature::StatusMap,
        Feature::WorkLink,
        Feature::StartProject,
        Feature::SiblingRepos,
        Feature::HostPlacement,
        Feature::QuickAnswer,
        Feature::AdoptTarget,
        Feature::RestoreTarget,
        Feature::Duplicate,
        Feature::WorkPlacement,
        Feature::RelatedSession,
        Feature::ControlRoute,
        Feature::SummaryCheck,
        Feature::TurnOutcome,
        Feature::MissionTriage,
        Feature::RoutineRunOutcome,
        Feature::PrTriage,
        Feature::MainTicket,
        Feature::TrackerDuplicate,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Feature::StatusMap => "status_map",
            Feature::WorkLink => "work_link",
            Feature::StartProject => "start_project",
            Feature::SiblingRepos => "sibling_repos",
            Feature::HostPlacement => "host_placement",
            Feature::QuickAnswer => "quick_answer",
            Feature::AdoptTarget => "adopt_target",
            Feature::RestoreTarget => "restore_target",
            Feature::Duplicate => "duplicate",
            Feature::WorkPlacement => "work_placement",
            Feature::RelatedSession => "related_session",
            Feature::ControlRoute => "control_route",
            Feature::SummaryCheck => "summary_check",
            Feature::TurnOutcome => "turn_outcome",
            Feature::MissionTriage => "mission_triage",
            Feature::RoutineRunOutcome => "routine_run_outcome",
            Feature::PrTriage => "pr_triage",
            Feature::MainTicket => "main_ticket",
            Feature::TrackerDuplicate => "tracker_duplicate",
        }
    }

    pub fn parse(s: &str) -> Option<Feature> {
        Feature::ALL.iter().copied().find(|f| f.as_str() == s)
    }

    /// `decide.jev.<feature>`.
    pub fn setting_key(self) -> &'static str {
        match self {
            Feature::StatusMap => settings::DECIDE_JEV_STATUS_MAP,
            Feature::WorkLink => settings::DECIDE_JEV_WORK_LINK,
            Feature::StartProject => settings::DECIDE_JEV_START_PROJECT,
            Feature::SiblingRepos => settings::DECIDE_JEV_SIBLING_REPOS,
            Feature::HostPlacement => settings::DECIDE_JEV_HOST_PLACEMENT,
            Feature::QuickAnswer => settings::DECIDE_JEV_QUICK_ANSWER,
            Feature::AdoptTarget => settings::DECIDE_JEV_ADOPT_TARGET,
            Feature::RestoreTarget => settings::DECIDE_JEV_RESTORE_TARGET,
            Feature::Duplicate => settings::DECIDE_JEV_DUPLICATE,
            Feature::WorkPlacement => settings::DECIDE_JEV_WORK_PLACEMENT,
            Feature::RelatedSession => settings::DECIDE_JEV_RELATED_SESSION,
            Feature::ControlRoute => settings::DECIDE_JEV_CONTROL_ROUTE,
            Feature::SummaryCheck => settings::DECIDE_JEV_SUMMARY_CHECK,
            Feature::TurnOutcome => settings::DECIDE_JEV_TURN_OUTCOME,
            Feature::MissionTriage => settings::DECIDE_JEV_MISSION_TRIAGE,
            Feature::RoutineRunOutcome => settings::DECIDE_JEV_ROUTINE_RUN_OUTCOME,
            Feature::PrTriage => settings::DECIDE_JEV_PR_TRIAGE,
            Feature::MainTicket => settings::DECIDE_JEV_MAIN_TICKET,
            Feature::TrackerDuplicate => settings::DECIDE_JEV_TRACKER_DUPLICATE,
        }
    }

    /// Whether this feature sends Claude's reply text (a pane tail): then
    /// the org's SECOND consent (D48, the org's `decide.jev.reply_consent` row, or
    /// `decide.jev.unassigned_reply` for no org) is required on top of
    /// D31's. J2 and N6, which read the same screen, and J9, which sends
    /// the transcript's agent turns a summary is checked against.
    pub fn sends_reply_text(self) -> bool {
        matches!(
            self,
            Feature::TurnOutcome | Feature::RoutineRunOutcome | Feature::SummaryCheck
        )
    }
}

/// A feature's configured mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureMode {
    Off,
    Shadow,
    Assist,
}

impl FeatureMode {
    pub fn as_str(self) -> &'static str {
        match self {
            FeatureMode::Off => "off",
            FeatureMode::Shadow => "shadow",
            FeatureMode::Assist => "assist",
        }
    }

    /// Anything but `shadow` / `assist` is `off`.
    pub fn parse(s: &str) -> FeatureMode {
        match s.trim() {
            "shadow" => FeatureMode::Shadow,
            "assist" => FeatureMode::Assist,
            _ => FeatureMode::Off,
        }
    }

    pub fn of(s: &Store, feature: Feature) -> FeatureMode {
        FeatureMode::parse(&settings::get_string(s, feature.setting_key()))
    }
}

/// The mode a call is made in (what [`gate`] lets through).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Ask and record; the adapter acts on the rule's answer only.
    Shadow,
    /// Ask and record; the adapter may PROPOSE the answer to a person.
    Assist,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Shadow => "shadow",
            Mode::Assist => "assist",
        }
    }
}

/// Why a decision is not (or not only) the model's. The closed vocabulary
/// of `decision_runs.fallback` ([`crate::store::DECISION_FALLBACKS`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fallback {
    /// This process is a window onto a hub, not the fleet's owner.
    NotOwner,
    /// `decide.jev.enabled` is off.
    FlagOff,
    /// The subject's org has not consented (or it has no org and
    /// `decide.jev.unassigned` is off).
    OrgOff,
    /// The feature's mode is `off`.
    ModeOff,
    /// No API key is configured (or its reference cannot be read).
    NoKey,
    /// The circuit breaker is open.
    BreakerOpen,
    /// Today's input-token budget is spent.
    Budget,
    /// The call outlived `decide.jev.timeout_ms`.
    Timeout,
    /// The API refused (401, 422, 5xx), the request failed a local check the
    /// API would refuse, or the connection failed.
    HttpError,
    /// 429 or 529.
    RateLimited,
    /// An answer that does not fit the question (an option not offered,
    /// probabilities that are not a distribution, an unreadable body).
    InvalidAnswer,
    /// A valid answer below the adapter's confidence floor (recorded; not
    /// to be acted on).
    LowConfidence,
}

impl Fallback {
    pub const ALL: [Fallback; 12] = [
        Fallback::NotOwner,
        Fallback::FlagOff,
        Fallback::OrgOff,
        Fallback::ModeOff,
        Fallback::NoKey,
        Fallback::BreakerOpen,
        Fallback::Budget,
        Fallback::Timeout,
        Fallback::HttpError,
        Fallback::RateLimited,
        Fallback::InvalidAnswer,
        Fallback::LowConfidence,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Fallback::NotOwner => "not_owner",
            Fallback::FlagOff => "flag_off",
            Fallback::OrgOff => "org_off",
            Fallback::ModeOff => "mode_off",
            Fallback::NoKey => "no_key",
            Fallback::BreakerOpen => "breaker_open",
            Fallback::Budget => "budget",
            Fallback::Timeout => "timeout",
            Fallback::HttpError => "http_error",
            Fallback::RateLimited => "rate_limited",
            Fallback::InvalidAnswer => "invalid_answer",
            Fallback::LowConfidence => "low_confidence",
        }
    }

    pub fn parse(s: &str) -> Option<Fallback> {
        Fallback::ALL.into_iter().find(|f| f.as_str() == s)
    }
}

/// The settings one call reads, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSettings {
    pub timeout: Duration,
    pub breaker_failures: u32,
    pub breaker_open_secs: i64,
    pub daily_token_budget: i64,
    pub model: String,
}

fn int_setting(s: &Store, key: &str) -> i64 {
    settings::get_string(s, key).parse().unwrap_or_default()
}

impl CallSettings {
    pub fn read(s: &Store) -> CallSettings {
        CallSettings {
            timeout: Duration::from_millis(
                int_setting(s, settings::DECIDE_JEV_TIMEOUT_MS).max(1) as u64
            ),
            breaker_failures: int_setting(s, settings::DECIDE_JEV_BREAKER_FAILURES).max(1) as u32,
            breaker_open_secs: int_setting(s, settings::DECIDE_JEV_BREAKER_OPEN_SECS),
            daily_token_budget: int_setting(s, settings::DECIDE_JEV_DAILY_TOKEN_BUDGET),
            model: settings::get_string(s, settings::DECIDE_JEV_MODEL),
        }
    }
}

/// Whether this process owns the fleet: its store names no hub it is a
/// window onto.
pub fn owns_the_fleet(s: &Store) -> bool {
    s.get_setting(HUB_REMOTE_URL_KEY)
        .ok()
        .flatten()
        .is_none_or(|v| v.trim().is_empty())
}

/// PURE: the start of `now`'s UTC day (the budget's window).
pub fn day_start(now: i64) -> i64 {
    now - now.rem_euclid(86_400)
}

/// The circuit breaker, as the record shows it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreakerState {
    pub open: bool,
    /// Failed calls in a row (up to the threshold).
    pub consecutive_failures: u32,
    /// While open: when it lets a call through again.
    pub open_until: Option<i64>,
}

/// The breaker of `provider` at `now`, over the runs of `scope`: open once
/// `failures` calls in a row failed, for `open_secs` after the newest of
/// them. After that one call goes through (half-open); another failure
/// opens it again. The live breaker ([`RunScope::Live`]) never counts an
/// offline benchmark's calls.
pub fn breaker_state(
    s: &Store,
    provider: &str,
    failures: u32,
    open_secs: i64,
    now: i64,
    scope: RunScope,
) -> Result<BreakerState, IpcError> {
    let (streak, newest) = s.decision_failure_streak(provider, failures, scope)?;
    let until = newest.map(|at| at + open_secs);
    let open = streak >= failures && until.is_some_and(|u| now < u);
    Ok(BreakerState {
        open,
        consecutive_failures: streak,
        open_until: until.filter(|_| open),
    })
}

/// Who is asking: a live adapter, or the offline benchmark (`fleet-hub
/// decide bench`, subject kind [`DECISION_BENCH_SUBJECT`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Caller {
    Live,
    /// The mode check is waived — measuring a feature must not need its
    /// live mode on (for `status_map` that would start the daily live
    /// runs) — and the call counts as `shadow`: it never proposes. The
    /// owner, the flag, the org's consent, the key, the breaker and the
    /// budget still apply, the breaker counted over every run.
    Bench,
}

impl Caller {
    /// The runs the BREAKER counts: a live feature's breaker never opens on
    /// a benchmark's failures. The budget is not scoped: see [`clear`].
    fn scope(self) -> RunScope {
        match self {
            Caller::Live => RunScope::Live,
            Caller::Bench => RunScope::All,
        }
    }
}

/// What the gate hands [`decide`] when a call may be made.
struct Cleared {
    mode: Mode,
    key: Secret,
    cfg: CallSettings,
}

/// The gate, at `now`, with the key it cleared. Order: owner, flag, mode
/// (not for the benchmark), org, key, breaker, budget — the first refusal
/// wins.
fn clear(
    s: &Store,
    feature: Feature,
    org_id: Option<i64>,
    provider: &str,
    now: i64,
    caller: Caller,
) -> Result<Cleared, Fallback> {
    if !owns_the_fleet(s) {
        return Err(Fallback::NotOwner);
    }
    if !settings::get_bool(s, settings::DECIDE_JEV_ENABLED) {
        return Err(Fallback::FlagOff);
    }
    let mode = match (caller, FeatureMode::of(s, feature)) {
        (Caller::Bench, _) => Mode::Shadow,
        (Caller::Live, FeatureMode::Off) => return Err(Fallback::ModeOff),
        (Caller::Live, FeatureMode::Shadow) => Mode::Shadow,
        (Caller::Live, FeatureMode::Assist) => Mode::Assist,
    };
    if !consents(s, feature, org_id) {
        return Err(Fallback::OrgOff);
    }
    let key = match s.resolve_decision_credential() {
        Ok(Some(k)) => k,
        _ => return Err(Fallback::NoKey),
    };
    let cfg = CallSettings::read(s);
    // A record that cannot be read is treated as the conservative answer:
    // no call.
    match breaker_state(
        s,
        provider,
        cfg.breaker_failures,
        cfg.breaker_open_secs,
        now,
        caller.scope(),
    ) {
        Ok(b) if !b.open => {}
        _ => return Err(Fallback::BreakerOpen),
    }
    // ONE budget: `daily_token_budget` caps the day's spend, live and
    // benchmark together, for every caller. Scoping it like the breaker let
    // a benchmark and the live features each spend the whole budget, twice
    // the setting a day.
    match s.decision_usage_since(provider, day_start(now), RunScope::All) {
        Ok((used, _)) if used < cfg.daily_token_budget => {}
        _ => return Err(Fallback::Budget),
    }
    Ok(Cleared { mode, key, cfg })
}

/// The org's consent to reply text (D48), for a feature that sends some
/// only now and then. A store that cannot be read consents to nothing.
pub fn reply_text_allowed(s: &Store, org_id: Option<i64>) -> bool {
    match org_id {
        Some(id) => s.org_jev_reply_allowed(id).unwrap_or(false),
        None => settings::get_bool(s, settings::DECIDE_JEV_UNASSIGNED_REPLY),
    }
}

/// The org's consent to `feature` (D31), and to reply text on top of it
/// when the feature sends some (D48). A store that cannot be read
/// consents to nothing.
pub fn consents(s: &Store, feature: Feature, org_id: Option<i64>) -> bool {
    let (base, reply) = match org_id {
        Some(id) => (
            s.org_jev_allowed(id).unwrap_or(false),
            s.org_jev_reply_allowed(id).unwrap_or(false),
        ),
        None => (
            settings::get_bool(s, settings::DECIDE_JEV_UNASSIGNED),
            settings::get_bool(s, settings::DECIDE_JEV_UNASSIGNED_REPLY),
        ),
    };
    base && (reply || !feature.sends_reply_text())
}

/// May `feature` ask the decision model about a subject of `org_id` now?
/// `Ok(mode)` or the fallback a run would record. Sync and short: the
/// caller holds the store lock only for it.
pub fn gate(s: &Store, feature: Feature, org_id: Option<i64>) -> Result<Mode, Fallback> {
    gate_at(s, feature, org_id, crate::store::now_unix())
}

/// [`gate`] at a given time (tests, the CLI).
pub fn gate_at(
    s: &Store,
    feature: Feature,
    org_id: Option<i64>,
    now: i64,
) -> Result<Mode, Fallback> {
    clear(s, feature, org_id, PROVIDER_JEV, now, Caller::Live).map(|c| c.mode)
}

/// [`gate_at`] for the offline benchmark: the feature's mode is not
/// checked (measuring `status_map` must not need `decide.jev.status_map`
/// on, which would start its daily live runs); everything else is, the
/// breaker and the budget over every run. `Ok` is always [`Mode::Shadow`].
/// A benchmark's [`DecideRequest`] has subject kind
/// [`DECISION_BENCH_SUBJECT`], which [`decide`] gates the same way.
pub fn gate_bench_at(
    s: &Store,
    feature: Feature,
    org_id: Option<i64>,
    now: i64,
) -> Result<Mode, Fallback> {
    clear(s, feature, org_id, PROVIDER_JEV, now, Caller::Bench).map(|c| c.mode)
}

static URL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(?:https?|ftp|ssh|git|wss?)://[^\s<>()\[\]{}'"`]+"#)
        .expect("URL pattern compiles")
});
static EMAIL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[a-z0-9._%+\-]+@[a-z0-9\-]+(?:\.[a-z0-9\-]+)+")
        .expect("email pattern compiles")
});

/// The redaction hook: URLs become `[url]`, email addresses `[email]`, and
/// whatever [`crate::logging::redact`] masks (tokens, keys) `[REDACTED]`.
/// The envelope runs every text value of a request through it before the
/// request is fingerprinted or sent; an adapter may redact more first.
pub fn redact_state(text: &str) -> String {
    let t = URL_RE.replace_all(text, "[url]");
    let t = EMAIL_RE.replace_all(&t, "[email]");
    crate::logging::redact(&t).into_owned()
}

/// PURE: `s` with every fenced code block (```` ``` ```` … ```` ``` ````, an
/// unclosed one to the end) replaced by `[code: <lang>, N lines]` — the
/// fence's language word (lower case) or `unknown`, and the block's line
/// count. Inline code is kept (decision D42's placeholder A/B; J2 sends
/// every pane tail through it).
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

/// PURE: `v` as JSON with every object's keys sorted, whatever map the
/// build's `serde_json` uses — the fingerprint's canonical form.
pub fn canonical_json(v: &serde_json::Value) -> String {
    fn sorted(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(o) => {
                let m: BTreeMap<&String, serde_json::Value> =
                    o.iter().map(|(k, x)| (k, sorted(x))).collect();
                let mut out = serde_json::Map::new();
                for (k, x) in m {
                    out.insert(k.clone(), x);
                }
                serde_json::Value::Object(out)
            }
            serde_json::Value::Array(a) => serde_json::Value::Array(a.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    // With `preserve_order` the map keeps insertion order, which `sorted`
    // made sorted; without it the map sorts. Either way: sorted.
    sorted(v).to_string()
}

/// PURE: lower-case hex HMAC-SHA256 of `msg` under `key` (any key length).
/// The decision record's fingerprints ([`fingerprint`],
/// `status_map::section_id`) are keyed hashes on purpose: without the local
/// key, a guessed input cannot be confirmed from the record.
pub(crate) fn hmac_sha256_hex(key: &[u8], msg: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    let mut mac =
        Hmac::<sha2::Sha256>::new_from_slice(key).expect("HMAC-SHA256 accepts any key length");
    mac.update(msg);
    hex::encode(mac.finalize().into_bytes())
}

/// PURE: the run's `input_fp` — HMAC-SHA256 under the local fingerprint key
/// of the canonical redacted request (state and question). Not a plain
/// hash: without the key, a guessed prompt cannot be confirmed from it.
pub fn fingerprint(fp_key: &Secret, redacted: &JevRequest) -> String {
    let v = serde_json::json!({ "state": redacted.state, "question": redacted.question });
    hmac_sha256_hex(fp_key.expose().as_bytes(), canonical_json(&v).as_bytes())
}

/// What an adapter asks the envelope.
#[derive(Debug, Clone, PartialEq)]
pub struct DecideRequest {
    pub feature: Feature,
    /// What is decided about: `session`, `tracker`, `section`, … (a word).
    /// [`DECISION_BENCH_SUBJECT`] marks the offline benchmark's call: gated
    /// as [`gate_bench_at`], never counted by the live breaker or stats (the
    /// daily budget counts every run).
    pub subject_kind: String,
    /// Its id (a word).
    pub subject_id: String,
    /// The subject's org (its consent is checked); `None` when it has none.
    pub org_id: Option<i64>,
    pub request: JevRequest,
    /// What the current rule decided (an id or word), for the shadow
    /// comparison; recorded as is.
    pub baseline: Option<String>,
    /// The adapter's question version, a constant bumped when its
    /// question changes.
    pub question_version: String,
    /// Below this confidence a valid answer is recorded with
    /// [`Fallback::LowConfidence`]. `None`: no floor.
    pub min_confidence: Option<f64>,
}

/// The model's answer, checked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionAnswer {
    /// A choice's option, or a noul / score value (`0.95`).
    pub value: String,
    pub probabilities: Option<BTreeMap<String, f64>>,
    pub confidence: Option<f64>,
    /// The model version that answered.
    pub model_version: Option<String>,
}

/// What [`decide`] returns. A run is recorded in every case; `run_id` is
/// `None` only when the store refused the record (logged).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionOutcome {
    pub run_id: Option<i64>,
    /// The mode the call was made in; `None` when the gate refused.
    pub mode: Option<Mode>,
    /// The checked answer — also with [`Fallback::LowConfidence`], for the
    /// record; act on it only through [`Self::usable`].
    pub answer: Option<DecisionAnswer>,
    pub fallback: Option<Fallback>,
    /// The call's round trip, as recorded; `None` when nothing was sent.
    #[serde(default)]
    pub latency_ms: Option<i64>,
    /// The call's input tokens and cost, as recorded (`0` when nothing was
    /// sent) — also when the record itself failed.
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub cost_microusd: i64,
}

impl DecideRequest {
    fn caller(&self) -> Caller {
        if self.subject_kind == DECISION_BENCH_SUBJECT {
            Caller::Bench
        } else {
            Caller::Live
        }
    }
}

impl DecisionOutcome {
    /// The answer an adapter may act on: a valid answer with no fallback.
    /// In `shadow` it may only be compared, in `assist` proposed.
    pub fn usable(&self) -> Option<&DecisionAnswer> {
        self.answer.as_ref().filter(|_| self.fallback.is_none())
    }

    /// [`Self::usable`] in `assist` mode only: what an adapter may propose.
    pub fn proposal(&self) -> Option<&DecisionAnswer> {
        self.usable().filter(|_| self.mode == Some(Mode::Assist))
    }
}

/// The envelope's context: the store, the backend, the clock.
#[derive(Clone)]
pub struct DecideCtx {
    pub store: Arc<Mutex<Store>>,
    pub backend: Arc<dyn DecisionBackend>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl DecideCtx {
    pub fn new(store: Arc<Mutex<Store>>, backend: Arc<dyn DecisionBackend>) -> Self {
        DecideCtx {
            store,
            backend,
            clock: Arc::new(crate::store::now_unix),
        }
    }

    /// The real thing: Jev over HTTPS from this process.
    pub fn jev(store: Arc<Mutex<Store>>) -> Self {
        DecideCtx::new(store, Arc::new(JevBackend::direct()))
    }

    /// A fixed or scripted clock (tests).
    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }
}

/// Ask. Gate, redact, call once, check, and record — see the module docs.
/// Never errors: every failure is a [`Fallback`] on the outcome and on the
/// recorded run.
pub async fn decide(ctx: &DecideCtx, req: DecideRequest) -> DecisionOutcome {
    let now = ctx.now();
    let provider = ctx.backend.provider();
    let redacted = req.request.redacted();
    let candidates: Vec<String> = match req.request.question.check() {
        Ok(()) => req.request.question.candidates(),
        Err(_) => Vec::new(),
    };
    let baseline = req.baseline.clone().filter(|b| {
        let ok = is_decision_word(b);
        if !ok {
            tracing::warn!(
                feature = req.feature.as_str(),
                "[decide] a baseline that is not an id or word was not recorded"
            );
        }
        ok
    });
    let mut run = NewDecisionRun {
        at: now,
        feature: req.feature.as_str().into(),
        org_id: req.org_id,
        subject_kind: req.subject_kind.clone(),
        subject_id: req.subject_id.clone(),
        mode: FeatureMode::Off.as_str().into(),
        provider: provider.into(),
        model_version: None,
        question_version: req.question_version.clone(),
        input_fp: None,
        candidates,
        answer: None,
        probabilities: None,
        confidence: None,
        fallback: None,
        baseline_answer: baseline,
        called: false,
        latency_ms: None,
        input_tokens: 0,
        cost_microusd: 0,
    };

    // 0. A run that cannot be recorded is never sent: the budget and the
    //    breaker read `decision_runs`, so an unrecorded call escapes both.
    if let Err(e) = run.validate() {
        tracing::warn!(
            feature = req.feature.as_str(),
            "[decide] request not sent, its run would not record: {}",
            e.message
        );
        return DecisionOutcome {
            run_id: None,
            mode: None,
            answer: None,
            fallback: Some(Fallback::HttpError),
            latency_ms: None,
            input_tokens: 0,
            cost_microusd: 0,
        };
    }

    // 1. The gate, the fingerprint and the configured mode: one short lock.
    let gated = match lock(&ctx.store) {
        Ok(s) => {
            // A benchmark's call is a shadow call whatever the live mode.
            run.mode = match req.caller() {
                Caller::Live => FeatureMode::of(&s, req.feature).as_str().into(),
                Caller::Bench => Mode::Shadow.as_str().into(),
            };
            run.input_fp = s.decision_fp_key().ok().map(|k| fingerprint(&k, &redacted));
            clear(&s, req.feature, req.org_id, provider, now, req.caller())
        }
        Err(_) => Err(Fallback::FlagOff),
    };
    let cleared = match gated {
        Ok(c) => c,
        Err(f) => return record(ctx, run, None, None, Some(f)),
    };

    // 2. The checks the API would refuse, before anything is sent.
    if let Err(why) = redacted.check(&cleared.cfg.model) {
        tracing::warn!(
            feature = req.feature.as_str(),
            "[decide] request not sent: {why}"
        );
        return record(
            ctx,
            run,
            Some(cleared.mode),
            None,
            Some(Fallback::HttpError),
        );
    }

    // 3. One call, no lock held, bounded by the timeout (also around the
    //    backend, so a fake or a stuck transport cannot outlive it).
    run.called = true;
    // tokio's clock: the same as std's in production, and it moves with a
    // paused test clock.
    let t0 = tokio::time::Instant::now();
    let answered = tokio::time::timeout(
        cleared.cfg.timeout,
        ctx.backend.ask(
            &cleared.key,
            &cleared.cfg.model,
            &redacted,
            cleared.cfg.timeout,
        ),
    )
    .await;
    run.latency_ms = Some(t0.elapsed().as_millis().min(i64::MAX as u128) as i64);
    let resp = match answered {
        Err(_) => Err(BackendError::Timeout),
        Ok(r) => r,
    };
    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            match &e {
                BackendError::RateLimited { retry_after } => tracing::warn!(
                    feature = req.feature.as_str(),
                    retry_after = ?retry_after,
                    "[decide] rate limited"
                ),
                BackendError::Http { status } => tracing::warn!(
                    feature = req.feature.as_str(),
                    status,
                    "[decide] the decision model refused"
                ),
                other => tracing::warn!(
                    feature = req.feature.as_str(),
                    "[decide] no answer: {}",
                    crate::logging::redact_secrets(
                        &format!("{other:?}"),
                        &[cleared.key.expose().to_string()]
                    )
                ),
            }
            let f = e.fallback();
            return record(ctx, run, Some(cleared.mode), None, Some(f));
        }
    };

    // 4. Check the answer.
    run.input_tokens = resp.usage.input_tokens.min(i64::MAX as u64) as i64;
    run.cost_microusd = jev::cost_microusd(run.input_tokens);
    run.model_version = Some(resp.model.clone()).filter(|m| is_decision_word(m));
    let checked = resp
        .answers
        .get(jev::QUESTION_ID)
        .ok_or_else(|| "no answer to the question".to_string())
        .and_then(|a| jev::validate_answer(&redacted.question, a));
    let valid = match checked {
        Ok(v) => v,
        Err(why) => {
            tracing::warn!(
                feature = req.feature.as_str(),
                "[decide] invalid answer: {why}"
            );
            return record(
                ctx,
                run,
                Some(cleared.mode),
                None,
                Some(Fallback::InvalidAnswer),
            );
        }
    };
    let answer = DecisionAnswer {
        value: valid.value,
        probabilities: valid.probabilities,
        confidence: valid.confidence,
        model_version: run.model_version.clone(),
    };
    let low = match (req.min_confidence, answer.confidence) {
        (Some(floor), Some(c)) => c < floor,
        (Some(_), None) => true,
        (None, _) => false,
    };
    let fallback = low.then_some(Fallback::LowConfidence);
    record(ctx, run, Some(cleared.mode), Some(answer), fallback)
}

/// Write the run (with its answer and fallback) and build the outcome.
fn record(
    ctx: &DecideCtx,
    mut run: NewDecisionRun,
    mode: Option<Mode>,
    answer: Option<DecisionAnswer>,
    fallback: Option<Fallback>,
) -> DecisionOutcome {
    if let Some(a) = &answer {
        run.answer = Some(a.value.clone());
        run.probabilities = a.probabilities.clone();
        run.confidence = a.confidence;
    }
    run.fallback = fallback.map(|f| f.as_str().to_string());
    let run_id = match lock(&ctx.store).and_then(|s| s.insert_decision_run(&run)) {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!(
                feature = %run.feature,
                "[decide] the decision was not recorded: {}",
                e.message
            );
            None
        }
    };
    DecisionOutcome {
        run_id,
        mode,
        answer,
        fallback,
        latency_ms: run.latency_ms,
        input_tokens: run.input_tokens,
        cost_microusd: run.cost_microusd,
    }
}

/// Delete runs older than `decide.retention_days` (`0` keeps them), in
/// batches, at most [`RETENTION_TICK_CAP`] per call. A run a person
/// confirmed, corrected or rejected is kept (D37: a rejection holds). The GC tick runs it;
/// ungated like the work retention sweep. Returns the rows deleted.
pub fn sweep_runs(store: &Mutex<Store>, now: i64) -> usize {
    let days = match store.lock() {
        Ok(s) => int_setting(&s, settings::DECIDE_RETENTION_DAYS),
        Err(_) => return 0,
    };
    if days <= 0 {
        return 0;
    }
    let cutoff = now - days * 86_400;
    crate::service::work::retention::sweep_batches(
        store,
        RETENTION_BATCH,
        RETENTION_TICK_CAP,
        "decision_runs",
        |s, want| s.sweep_decision_runs(cutoff, want),
    )
}

// --- health (test map §7) -------------------------------------------------------------

/// The window `fleet_health.decide` judges the live calls over.
pub const HEALTH_WINDOW_SECS: i64 = 3_600;
/// Fewer attempts in the window are not enough to call the envelope
/// degraded by its failure rate (the breaker still can).
pub const HEALTH_MIN_ATTEMPTS: u32 = 5;
/// Above this share of failed attempts in the window the envelope is
/// degraded (test map §7: "fallback rate > 20% for an hour").
pub const HEALTH_FAILURE_RATE: f64 = 0.20;
/// Fallbacks that are a configuration's refusal, not an attempt: nothing
/// was meant to be sent.
pub const HEALTH_NOT_ATTEMPTS: &[&str] = &[
    "not_owner",
    "flag_off",
    "mode_off",
    "org_off",
    "no_key",
    "budget",
];
/// Fallbacks that are the service failing (or the breaker refusing because
/// it did): what the failure rate counts.
pub const HEALTH_FAILURES: &[&str] = &["timeout", "http_error", "rate_limited", "breaker_open"];

/// `fleet_health.decide`: whether the live decision calls are working —
/// the test map's "degraded" (§7). Counts only, over the live adapters'
/// runs of the last [`HEALTH_WINDOW_SECS`]; a benchmark's runs never count.
/// Every field defaults (an older hub omits the whole block).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DecideHealth {
    /// `decide.jev.enabled`.
    #[serde(default)]
    pub enabled: bool,
    /// Every feature whose mode is not `off`: feature → `shadow | assist`.
    #[serde(default)]
    pub modes: BTreeMap<String, String>,
    #[serde(default)]
    pub window_secs: i64,
    /// Live runs in the window that meant to ask (a configuration's refusal
    /// — [`HEALTH_NOT_ATTEMPTS`] — is not one).
    #[serde(default)]
    pub attempts: u32,
    /// Of them, the service failing ([`HEALTH_FAILURES`]).
    #[serde(default)]
    pub failures: u32,
    #[serde(default)]
    pub failure_rate: Option<f64>,
    #[serde(default)]
    pub breaker_open: bool,
    /// Today's input-token budget is spent: every call falls back until the
    /// UTC day ends (planned, so not `degraded`).
    #[serde(default)]
    pub budget_spent: bool,
    /// The envelope is on and its calls are failing: the breaker is open,
    /// or more than [`HEALTH_FAILURE_RATE`] of at least
    /// [`HEALTH_MIN_ATTEMPTS`] attempts failed. Answers fall back to what
    /// fleet does today by themselves; this is for a person to look.
    #[serde(default)]
    pub degraded: bool,
    /// `breaker_open` or `failure_rate` when degraded.
    #[serde(default)]
    pub reason: Option<String>,
}

/// PURE: the judgement over the live `stats` of the window (rows of
/// [`Store::decision_stats`]; benchmark rows are skipped).
pub fn health_of(
    enabled: bool,
    modes: BTreeMap<String, String>,
    stats: &[DecisionStatRow],
    breaker_open: bool,
    budget_spent: bool,
) -> DecideHealth {
    let (mut attempts, mut failures) = (0u32, 0u32);
    for r in stats.iter().filter(|r| !r.bench) {
        let f = r.fallback.as_deref();
        if f.is_some_and(|f| HEALTH_NOT_ATTEMPTS.contains(&f)) {
            continue;
        }
        let n = u32::try_from(r.runs).unwrap_or(u32::MAX);
        attempts = attempts.saturating_add(n);
        if f.is_some_and(|f| HEALTH_FAILURES.contains(&f)) {
            failures = failures.saturating_add(n);
        }
    }
    let rate = (attempts > 0)
        .then(|| (f64::from(failures) / f64::from(attempts) * 1000.0).round() / 1000.0);
    let on = enabled && !modes.is_empty();
    let reason = if !on {
        None
    } else if breaker_open {
        Some("breaker_open")
    } else if attempts >= HEALTH_MIN_ATTEMPTS && rate.is_some_and(|r| r > HEALTH_FAILURE_RATE) {
        Some("failure_rate")
    } else {
        None
    };
    DecideHealth {
        enabled,
        modes,
        window_secs: HEALTH_WINDOW_SECS,
        attempts,
        failures,
        failure_rate: rate,
        breaker_open,
        budget_spent,
        degraded: reason.is_some(),
        reason: reason.map(str::to_string),
    }
}

/// `fleet_health.decide` from the store at `now`: `None` in a process that
/// does not own the fleet (a window onto a hub never calls out), or when
/// the envelope is off and no feature is on (nothing to report). Read
/// errors report nothing rather than a healthy envelope.
pub fn health(s: &Store, now: i64) -> Option<DecideHealth> {
    if !owns_the_fleet(s) {
        return None;
    }
    let enabled = settings::get_bool(s, settings::DECIDE_JEV_ENABLED);
    let modes: BTreeMap<String, String> = Feature::ALL
        .iter()
        .copied()
        .map(|f| (f, FeatureMode::of(s, f)))
        .filter(|(_, m)| *m != FeatureMode::Off)
        .map(|(f, m)| (f.as_str().to_string(), m.as_str().to_string()))
        .collect();
    if !enabled && modes.is_empty() {
        return None;
    }
    let cfg = CallSettings::read(s);
    let stats = s.decision_stats(now - HEALTH_WINDOW_SECS).ok()?;
    let breaker = breaker_state(
        s,
        PROVIDER_JEV,
        cfg.breaker_failures,
        cfg.breaker_open_secs,
        now,
        RunScope::Live,
    )
    .ok()?;
    let (used, _) = s
        .decision_usage_since(PROVIDER_JEV, day_start(now), RunScope::Live)
        .ok()?;
    let budget_spent = cfg.daily_token_budget > 0 && used >= cfg.daily_token_budget;
    Some(health_of(
        enabled,
        modes,
        &stats,
        breaker.open,
        budget_spent,
    ))
}

impl DecideHealth {
    /// The CLI's line.
    pub fn line(&self) -> String {
        format!(
            "health (last {} min, live): {} attempt(s), {} failed{}{}{} → {}",
            self.window_secs / 60,
            self.attempts,
            self.failures,
            self.failure_rate
                .map(|r| format!(" ({:.0}%)", r * 100.0))
                .unwrap_or_default(),
            if self.breaker_open {
                ", breaker open"
            } else {
                ""
            },
            if self.budget_spent {
                ", today's budget spent"
            } else {
                ""
            },
            match self.reason.as_deref() {
                Some(r) => format!("DEGRADED ({r}); answers fall back to today's rules"),
                None if self.enabled && !self.modes.is_empty() => "ok".to_string(),
                None => "no live feature on".to_string(),
            }
        )
    }
}

/// Today's spend.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodayUsage {
    /// The UTC day's start.
    pub since: i64,
    /// The live adapters' spend; the budget counts it together with the
    /// benchmark's below.
    pub input_tokens: i64,
    pub cost_microusd: i64,
    pub budget: i64,
    /// The offline benchmark's spend today, apart (every call, live or
    /// benchmark, is gated on the live and the benchmark spend together).
    #[serde(default)]
    pub bench_input_tokens: i64,
    #[serde(default)]
    pub bench_cost_microusd: i64,
}

/// An org that consented.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgConsent {
    pub id: i64,
    pub name: String,
}

/// Everything `fleet-hub decide status` shows — never the key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecideStatus {
    pub enabled: bool,
    pub owns_the_fleet: bool,
    /// feature → `off | shadow | assist`.
    pub modes: BTreeMap<String, String>,
    pub unassigned: bool,
    pub model: String,
    pub key: DecisionKeyStatus,
    pub orgs_allowed: Vec<OrgConsent>,
    /// D48: the orgs that ALSO consented to reply text (J2), and whether
    /// rows with no org may send it. Absent from an older hub.
    #[serde(default)]
    pub orgs_reply_allowed: Vec<OrgConsent>,
    #[serde(default)]
    pub unassigned_reply: bool,
    pub breaker: BreakerState,
    pub today: TodayUsage,
    pub retention_days: i64,
    pub runs_kept: i64,
    /// The window of `stats`, in days.
    pub window_days: i64,
    pub stats: Vec<DecisionStatRow>,
    /// What `fleet_health.decide` says (`None` when nothing is on).
    #[serde(default)]
    pub health: Option<DecideHealth>,
}

/// Read-only: the flag, modes, consent, key (configured or not), the live
/// breaker, today's spend (live, and the benchmark's apart) and the runs of
/// the last `days` days per feature, live or benchmark, provider, fallback
/// and org.
pub fn status(s: &Store, now: i64, days: i64) -> Result<DecideStatus, IpcError> {
    let cfg = CallSettings::read(s);
    let (input_tokens, cost_microusd) =
        s.decision_usage_since(PROVIDER_JEV, day_start(now), RunScope::Live)?;
    let (all_tokens, all_cost) =
        s.decision_usage_since(PROVIDER_JEV, day_start(now), RunScope::All)?;
    Ok(DecideStatus {
        enabled: settings::get_bool(s, settings::DECIDE_JEV_ENABLED),
        owns_the_fleet: owns_the_fleet(s),
        modes: Feature::ALL
            .iter()
            .copied()
            .map(|f| {
                (
                    f.as_str().to_string(),
                    FeatureMode::of(s, f).as_str().to_string(),
                )
            })
            .collect(),
        unassigned: settings::get_bool(s, settings::DECIDE_JEV_UNASSIGNED),
        model: cfg.model.clone(),
        key: s.decision_credential_status()?,
        orgs_allowed: s
            .list_orgs()?
            .into_iter()
            .filter(|o| o.jev_allowed)
            .map(|o| OrgConsent {
                id: o.id,
                name: o.name,
            })
            .collect(),
        orgs_reply_allowed: s
            .list_orgs()?
            .into_iter()
            .filter(|o| o.jev_allowed && o.jev_reply_allowed)
            .map(|o| OrgConsent {
                id: o.id,
                name: o.name,
            })
            .collect(),
        unassigned_reply: settings::get_bool(s, settings::DECIDE_JEV_UNASSIGNED_REPLY),
        breaker: breaker_state(
            s,
            PROVIDER_JEV,
            cfg.breaker_failures,
            cfg.breaker_open_secs,
            now,
            RunScope::Live,
        )?,
        today: TodayUsage {
            since: day_start(now),
            input_tokens,
            cost_microusd,
            budget: cfg.daily_token_budget,
            bench_input_tokens: all_tokens - input_tokens,
            bench_cost_microusd: all_cost - cost_microusd,
        },
        retention_days: int_setting(s, settings::DECIDE_RETENTION_DAYS),
        runs_kept: s.decision_run_count()?,
        window_days: days,
        stats: s.decision_stats(now - days.max(0) * 86_400)?,
        health: health(s, now),
    })
}

/// PURE: micro-USD as dollars (`$0.000021`).
pub fn fmt_usd(micro: i64) -> String {
    format!("${}.{:06}", micro / 1_000_000, micro % 1_000_000)
}

impl DecideStatus {
    /// The CLI's lines.
    pub fn lines(&self) -> Vec<String> {
        let on = |b: bool| if b { "on" } else { "off" };
        let mut out = vec![
            format!(
                "decisions (Jev): {}{}",
                on(self.enabled),
                if self.owns_the_fleet {
                    ""
                } else {
                    "  [this store is a window onto a hub: it never calls out]"
                }
            ),
            format!(
                "modes: {}",
                self.modes
                    .iter()
                    .map(|(f, m)| format!("{f}={m}"))
                    .collect::<Vec<_>>()
                    .join("  ")
            ),
            format!(
                "orgs that consented: {}   rows with no org: {}",
                if self.orgs_allowed.is_empty() {
                    "none".to_string()
                } else {
                    self.orgs_allowed
                        .iter()
                        .map(|o| format!("{} (#{})", o.name, o.id))
                        .collect::<Vec<_>>()
                        .join(", ")
                },
                on(self.unassigned)
            ),
            format!(
                "reply text (D48): {}   rows with no org: {}",
                if self.orgs_reply_allowed.is_empty() {
                    "none".to_string()
                } else {
                    self.orgs_reply_allowed
                        .iter()
                        .map(|o| format!("{} (#{})", o.name, o.id))
                        .collect::<Vec<_>>()
                        .join(", ")
                },
                on(self.unassigned && self.unassigned_reply)
            ),
            format!(
                "key: {}   model: {}",
                match (self.key.configured, self.key.by_reference) {
                    (false, _) => "not configured".to_string(),
                    (true, true) => "configured (reference)".to_string(),
                    (true, false) => "configured".to_string(),
                },
                self.model
            ),
            format!(
                "breaker: {}",
                if self.breaker.open {
                    format!(
                        "OPEN after {} failed calls, until {}",
                        self.breaker.consecutive_failures,
                        self.breaker.open_until.unwrap_or_default()
                    )
                } else {
                    format!(
                        "closed ({} failed call(s) in a row)",
                        self.breaker.consecutive_failures
                    )
                }
            ),
            format!(
                "today (UTC): {} of {} input tokens, {}{}",
                self.today.input_tokens + self.today.bench_input_tokens,
                self.today.budget,
                fmt_usd(self.today.cost_microusd + self.today.bench_cost_microusd),
                if self.today.bench_input_tokens > 0 || self.today.bench_cost_microusd > 0 {
                    format!(
                        "   of which benchmark: {} input tokens, {}",
                        self.today.bench_input_tokens,
                        fmt_usd(self.today.bench_cost_microusd)
                    )
                } else {
                    String::new()
                }
            ),
            format!(
                "runs kept: {}   retention: {}",
                self.runs_kept,
                if self.retention_days == 0 {
                    "forever".to_string()
                } else {
                    format!("{} days", self.retention_days)
                }
            ),
        ];
        if let Some(h) = &self.health {
            out.push(h.line());
        }
        if self.stats.is_empty() {
            out.push(format!("no runs in the last {} days", self.window_days));
        } else {
            out.push(format!("runs in the last {} days:", self.window_days));
            for r in &self.stats {
                out.push(format!(
                    "  {:<11} {:<5} {:<6} {:<15} org {:<5} runs {:>6}  calls {:>6}  tokens {:>9}  {}  agreed {}/{}  followups c{} r{} x{} i{}",
                    r.feature,
                    if r.bench { "bench" } else { "live" },
                    r.provider,
                    r.fallback.as_deref().unwrap_or("answered"),
                    r.org_id.map(|o| o.to_string()).unwrap_or_else(|| "-".into()),
                    r.runs,
                    r.called,
                    r.input_tokens,
                    fmt_usd(r.cost_microusd),
                    r.agreed,
                    r.compared,
                    r.confirmed,
                    r.rejected,
                    r.corrected,
                    r.ignored,
                ));
            }
        }
        out
    }
}
