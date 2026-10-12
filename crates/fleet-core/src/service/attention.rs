//! Which sessions need a person, decided once, here.
//!
//! # Why this is on the hub
//!
//! "What needs me?" is the question a phone is opened to answer, and until
//! now every client worked it out for itself from rows it had to download
//! first. The desktop has the rich version (`src/lib/attention.ts`,
//! `classify`), fleet-mobile has a two-condition predicate, and the hub's own
//! `fleet_health` roll-up has no notion of it at all — it counts ghosts,
//! stuck rows and context pressure, never "a person is needed here".
//!
//! So a phone downloaded 44 full rows — 51 968 B measured — to find the three
//! that wanted an answer. With the reason on the row and a filter beside it,
//! that is 1 668 B.
//!
//! The divergence between those two classifiers is latent rather than
//! absent: replayed over a 56-row capture they agree exactly, but only
//! because that capture holds no `failed` row, no ghost, no `lost_at` and no
//! `safe_kill_state`. The first one of those would have split them.
//!
//! # What is here and what is not
//!
//! Only the classification. Ranking, scoring and sort order stay with the
//! client: they are how a screen chooses to present the queue, not a fact
//! about the fleet.
//!
//! The desktop's "idle for too long" bucket is deliberately **not** decided
//! here either. It depends on an operator-configured idle threshold,
//! and a hub that guessed one would quietly disagree with the desktop that
//! set it. What this module answers is the states that need no knob to
//! recognise; a client is free to add its own idle rule on top.

use std::collections::{BTreeMap, BTreeSet};

use crate::service::account_usage::{AccountUsageSnapshot, UsageOutcomeKind};
use crate::store::{HostRow, SessionRow};

/// Why a session needs a person. The order is the urgency order, and it is
/// the same order the desktop's `TRIAGE_BUCKETS` uses, so the two cannot
/// drift into disagreeing about which of two reasons wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// Blocked on a dialog: a permission prompt, a question, an elicitation.
    /// The one state where an answer is all that is wanted.
    Waiting,
    /// Wedged in a way the REPL will not leave on its own — an auth menu, a
    /// reconnect, an OOM. `stuck_kind` says which.
    Stuck,
    /// The session's host does not answer (step 2.4). Nothing in the session
    /// can move until it does; the row's last status is stale meanwhile.
    HostDown,
    /// The session's account is at a usage limit and the session is not
    /// working: shown as "Paused · limit" (step 2.4), with Switch account and
    /// Wait (step 4.4).
    AccountLimit,
    /// The session's account has no usable login (no credentials file, an
    /// expired login or a rejected token) and the session is not working
    /// (step 2.4): sign in again.
    NoCredentials,
    /// The last turn ended in an API error (a `StopFailure`: rate limit,
    /// auth, …); the `stop_failure` timeline entry says which. Re-prompt.
    StopFailed,
    /// Claude reported a failed turn (`claude agents`, a pane-less agent).
    Failed,
    /// The context window is at or past `health.context_red_pct`: compact
    /// or hand over before the next turn does it for you.
    ContextFull,
    /// The tick demoted a `working` row nothing had moved for
    /// `reconcile.stale_working_secs`: look at what it was doing. Ends on
    /// the next hook, an attach, the row `working` / `blocked` again, or
    /// after `reconcile.stale_working_ttl_secs`.
    StaleWorking,
    /// Idle with a PR whose checks are failing.
    CiFailing,
    /// Jev read a silent turn's end as a question (J2, `turn_outcome`
    /// `asked`), on a row still idle after it: *probably* waiting (gap plan
    /// G1.6). A proposal, not a fact, so it is kept apart from Needs you and
    /// never raises the badge; the person confirms it by answering or sets
    /// it aside with "Not waiting". Hub contract 15.
    ProbablyWaiting,
    /// The session's lifecycle is broken: a safe kill that failed or is still
    /// pending, a ghost row, or a row the fleet has lost track of.
    Lifecycle,
}

impl Reason {
    /// The wire spelling, matching the desktop's bucket names.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Waiting => "waiting",
            Reason::Stuck => "stuck",
            Reason::HostDown => "host_down",
            Reason::AccountLimit => "account_limit",
            Reason::NoCredentials => "no_credentials",
            Reason::StopFailed => "stop_failed",
            Reason::Failed => "failed",
            Reason::ContextFull => "context_full",
            Reason::StaleWorking => "stale_working",
            Reason::CiFailing => "ci_failing",
            Reason::ProbablyWaiting => "probably_waiting",
            Reason::Lifecycle => "lifecycle",
        }
    }
}

impl Reason {
    /// The attention state this reason puts a session in.
    pub fn state(self) -> State {
        match self {
            Reason::Waiting | Reason::Stuck | Reason::ContextFull | Reason::StaleWorking => {
                State::ActionRequired
            }
            Reason::StopFailed | Reason::Failed | Reason::CiFailing => State::Failed,
            // Waiting on something outside the session (step 2.4).
            Reason::HostDown | Reason::AccountLimit | Reason::NoCredentials => State::Blocked,
            // Jev's reading, offered beside Needs you (G1.6).
            Reason::ProbablyWaiting => State::Proposed,
            // A ghost, a lost row or a pending safe kill: nobody can answer
            // it, so it leaves the badge (step 1.1 folds a mass loss into one
            // Restore row instead).
            Reason::Lifecycle => State::Paused,
        }
    }
}

/// The attention states of the Orbit Fleet redesign (step 0.4; the eighth,
/// `Proposed`, since gap plan G1.6), in urgency order. The twelve triage buckets fold into these, and the badge
/// counts only the first three. The table is shared with the desktop
/// through `src/lib/attention_states.json`, which both test suites check.
///
/// On the wire since hub contract 11 (step 2.6): [`Attention::state`] rides
/// beside the reason. `Blocked` has three reasons since step 2.4, decided
/// from [`Facts`] about the fleet rather than from the row alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// A person has to answer or act: shown as "Needs you".
    ActionRequired,
    Failed,
    /// Waiting on something outside the session (a host, credentials, a
    /// limit, another task); shown as "Needs you" with its reason line.
    Blocked,
    /// Jev proposes the session is waiting (G1.6, hub contract 15): shown
    /// as "+1 proposed" apart from Needs you, never counted by the badge.
    Proposed,
    Working,
    Paused,
    Done,
    Idle,
}

impl State {
    pub const ALL: [State; 8] = [
        State::ActionRequired,
        State::Failed,
        State::Blocked,
        State::Proposed,
        State::Working,
        State::Paused,
        State::Done,
        State::Idle,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            State::ActionRequired => "action_required",
            State::Failed => "failed",
            State::Blocked => "blocked",
            State::Proposed => "proposed",
            State::Working => "working",
            State::Paused => "paused",
            State::Done => "done",
            State::Idle => "idle",
        }
    }

    /// Whether a session in this state raises the Needs you badge.
    pub fn counts_toward_badge(self) -> bool {
        matches!(self, State::ActionRequired | State::Failed | State::Blocked)
    }
}

/// Every triage bucket, in the desktop's `TRIAGE_BUCKETS` order, and its
/// state. The hub decides the twelve [`Reason`]s; `done_unread`, `idle_long`,
/// `working` and `idle` are the desktop's own buckets, listed so the whole
/// map lives in one table.
pub const BUCKET_STATES: [(&str, State); 16] = [
    ("waiting", State::ActionRequired),
    ("stuck", State::ActionRequired),
    ("host_down", State::Blocked),
    ("account_limit", State::Blocked),
    ("no_credentials", State::Blocked),
    ("stop_failed", State::Failed),
    ("failed", State::Failed),
    ("context_full", State::ActionRequired),
    ("stale_working", State::ActionRequired),
    ("ci_failing", State::Failed),
    ("probably_waiting", State::Proposed),
    ("done_unread", State::Done),
    ("lifecycle", State::Paused),
    ("idle_long", State::Idle),
    ("working", State::Working),
    ("idle", State::Idle),
];

/// A session's attention state: its [`Reason`]'s state when it needs a
/// person, else `Working` or `Idle` from `claude_status`.
pub fn state_with(row: &SessionRow, context_red_pct: f64) -> State {
    state_in(row, context_red_pct, &Facts::default())
}

/// [`state_with`], with what is known about the fleet (step 2.4).
pub fn state_in(row: &SessionRow, context_red_pct: f64, facts: &Facts) -> State {
    match needs_attention_in(row, context_red_pct, facts) {
        Some(a) => a.reason.state(),
        // A shell has no agent in it, whatever its status column says.
        None if row.kind != "shell" && row.claude_status.as_deref() == Some("working") => {
            State::Working
        }
        None => State::Idle,
    }
}

/// The one context threshold, when no store is at hand to read
/// `health.context_red_pct`: `fleet_health.context_red`, `context_full`
/// here and the desktop's chip all count from the same number.
pub const DEFAULT_CONTEXT_RED_PCT: f64 = 85.0;

/// What the fleet knows beyond a session's own row (step 2.4): which hosts
/// are down, and which accounts are at a limit or have no usable login. The
/// three `Blocked` reasons come from here; with the default (nothing known)
/// the classification is exactly the row-only one.
///
/// Built by [`Facts::from_fleet`]; the shared fixture spells it in JSON.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Facts {
    /// Aliases of hosts that were pinged and did not answer.
    pub down_hosts: BTreeSet<String>,
    /// Accounts at a usage limit, by uuid.
    pub limited_accounts: BTreeMap<String, Limit>,
    /// Accounts whose login is gone, expired or rejected, by uuid.
    pub uncredentialed_accounts: BTreeSet<String>,
}

/// A usage window at its limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Limit {
    pub window: LimitWindow,
    /// When the window resets (unix seconds), when known.
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitWindow {
    FiveHour,
    Weekly,
}

impl Facts {
    /// The facts the host rows and the usage snapshots hold at `now`.
    ///
    /// A host is down once a ping said so (`reachable` false with a
    /// `last_pinged_at`); one never pinged is unknown, not down. An account
    /// is at its limit when its 5-hour or weekly window is fully used and
    /// has not reset yet (both at their limit: the one that frees last). A
    /// login counts as gone on `login_expired` and `token_rejected` only: an
    /// expired access token refreshes by itself, and `no_credentials` is the
    /// usage script finding no token file, which on a macOS host (the token
    /// lives in the Keychain) says nothing about the login.
    pub fn from_fleet(hosts: &[HostRow], usage: &[AccountUsageSnapshot], now: i64) -> Facts {
        let down_hosts = hosts
            .iter()
            .filter(|h| !h.reachable && h.last_pinged_at.is_some())
            .map(|h| h.alias.clone())
            .collect();
        let mut limited_accounts = BTreeMap::new();
        let mut uncredentialed_accounts = BTreeSet::new();
        for snap in usage {
            if matches!(
                snap.status,
                UsageOutcomeKind::LoginExpired | UsageOutcomeKind::TokenRejected
            ) {
                uncredentialed_accounts.insert(snap.account_uuid.clone());
            }
            let Some(u) = &snap.usage else { continue };
            use crate::service::account_usage::{FIVE_HOUR_SECS, WEEK_SECS};
            let at_limit = |w: &crate::service::account_usage::Window, len: i64| {
                w.utilization >= 100.0 && w.live_at(snap.fetched_at, len, now)
            };
            // Both windows at their limit: the one that frees last decides,
            // so the row does not unblock while the other still holds it
            // (no reset time holds longest). Weekly wins a tie.
            let weekly = u
                .seven_day
                .as_ref()
                .filter(|w| at_limit(w, WEEK_SECS))
                .map(|w| (LimitWindow::Weekly, w));
            let five = u
                .five_hour
                .as_ref()
                .filter(|w| at_limit(w, FIVE_HOUR_SECS))
                .map(|w| (LimitWindow::FiveHour, w));
            let frees = |w: &crate::service::account_usage::Window| w.resets_at.unwrap_or(i64::MAX);
            let limit = match (weekly, five) {
                (Some(wk), Some(fh)) if frees(fh.1) > frees(wk.1) => Some(fh),
                (Some(wk), _) => Some(wk),
                (None, fh) => fh,
            };
            if let Some((window, w)) = limit {
                limited_accounts.insert(
                    snap.account_uuid.clone(),
                    Limit {
                        window,
                        resets_at: w.resets_at,
                    },
                );
            }
        }
        Facts {
            down_hosts,
            limited_accounts,
            uncredentialed_accounts,
        }
    }
}

/// A session that needs a person, and since when.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Attention {
    pub reason: Reason,
    /// Best-effort unix second the session entered this state. Falls back to
    /// `last_activity_at`, which is always present, so a client can always
    /// draw an age.
    pub since: i64,
    /// The attention state [`Self::reason`] puts the session in (hub
    /// contract 11, step 2.6), so a client reads "Needs you · Blocked"
    /// without carrying the reason→state table itself.
    pub state: State,
}

/// [`needs_attention_with`] at [`DEFAULT_CONTEXT_RED_PCT`]. Callers with a
/// store read the setting (`service::health::context_red_pct`) instead.
pub fn needs_attention(row: &SessionRow) -> Option<Attention> {
    needs_attention_with(row, DEFAULT_CONTEXT_RED_PCT)
}

/// Whether this row needs a person, and why.
///
/// The order of the checks *is* the precedence: a session that is both
/// blocked and ghosted is reported as blocked, because that is the one a
/// person can do something about right now — and every reason a person can
/// act on comes before `Lifecycle`, which nobody can act on from a phone.
///
/// An `external` session — a Claude running outside fleet entirely — never
/// qualifies, whatever its fields say: it is read-only here, so reporting it
/// as needing a person offers an action that does not exist. Nor does a
/// `shell`: there is no Claude in it.
pub fn needs_attention_with(row: &SessionRow, context_red_pct: f64) -> Option<Attention> {
    needs_attention_in(row, context_red_pct, &Facts::default())
}

/// [`needs_attention_with`], with what is known about the fleet: a live
/// session on a down host, or one that is not working on an account at its
/// limit or without a login, is `Blocked` (step 2.4). A dead row (ghost or
/// lost) keeps `Lifecycle`: a host's mass loss is one Restore row, not a
/// badge per session.
pub fn needs_attention_in(
    row: &SessionRow,
    context_red_pct: f64,
    facts: &Facts,
) -> Option<Attention> {
    if row.kind == "external" || row.kind == "shell" {
        return None;
    }
    let failed = row.claude_status.as_deref() == Some("failed");
    // A dead row keeps its last context reading; only a live one can act on
    // it (or on a stale stamp) — a lost one reads `Lifecycle`.
    let live = row.status != "ghost" && row.lost_at.is_none();
    let working = row.claude_status.as_deref() == Some("working");
    let account = row.account_uuid.as_deref();
    let idle = row
        .claude_status
        .as_deref()
        .is_some_and(|s| crate::store::IDLE_STATUSES.contains(&s));
    // J2 (step 5.11): what Jev read a silent turn's end as, on a row that
    // is still idle after it. Every hook clears it, so it never outvotes
    // one.
    let jev = |o: &str| idle && row.turn_outcome.as_deref() == Some(o);
    let reason = if row.claude_status.as_deref() == Some("blocked") || row.pending_form.is_some() {
        Reason::Waiting
    } else if row.stuck_kind.is_some() || jev("stuck") {
        Reason::Stuck
    } else if live && facts.down_hosts.contains(&row.host_alias) {
        Reason::HostDown
    } else if live && !working && account.is_some_and(|a| facts.limited_accounts.contains_key(a)) {
        Reason::AccountLimit
    } else if live && !working && account.is_some_and(|a| facts.uncredentialed_accounts.contains(a))
    {
        Reason::NoCredentials
    } else if failed && !crate::store::has_no_pane(&row.kind) {
        Reason::StopFailed
    } else if failed {
        Reason::Failed
    } else if live && row.context_pct.is_some_and(|p| p >= context_red_pct) {
        Reason::ContextFull
    } else if live && row.stale_working_at.is_some() {
        Reason::StaleWorking
    } else if idle && row.ci_status.as_deref() == Some("failing") {
        Reason::CiFailing
    } else if live && jev("asked") {
        // After every reason a person must act on: a proposal never hides
        // one, and a dead row is `Lifecycle` whatever Jev read.
        Reason::ProbablyWaiting
    } else if is_lifecycle_broken(row) {
        Reason::Lifecycle
    } else {
        return None;
    };
    Some(Attention {
        reason,
        since: since_for(row, reason),
        state: reason.state(),
    })
}

/// Why a mission waits on a person (gap plan G1.6): the class of the one
/// attention model that is a mission, not a session. Inbox and Today list
/// it beside the sessions that need you, the rail badge counts it, and the
/// phone reads it off the mission row (`MissionRow::waiting_on`, hub
/// contract 15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionWaitReason {
    /// The planner asked a question only a person answers (an open `ask`
    /// card on the mission).
    Question,
    /// The mission asks for autonomy (level 1–3) and no live grant covers
    /// its current plan: sign the autonomy grant, or it runs nothing alone.
    SignGrant,
    /// Commands wait in its confirm queue (open cards) for a person.
    Confirm,
}

impl MissionWaitReason {
    pub fn as_str(self) -> &'static str {
        match self {
            MissionWaitReason::Question => "question",
            MissionWaitReason::SignGrant => "sign_grant",
            MissionWaitReason::Confirm => "confirm",
        }
    }
}

/// A mission waiting on a person, and since when.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MissionWait {
    pub reason: MissionWaitReason,
    /// Unix second it started waiting: the oldest open card, or the
    /// mission's last change for a grant to sign.
    pub since: i64,
    /// Open cards in its confirm queue.
    pub open_cards: u32,
}

/// Whether an active mission waits on a person, and why. Only an `active`
/// mission is asked: a draft is still being written, a paused one has its
/// brake reason, a finished one waits on nobody. A question outranks the
/// grant (only a person answers it, and signing does not), and the grant
/// outranks plain confirmations (signing clears most of them at once).
pub fn mission_waiting(
    mission: &crate::store::MissionRow,
    live_grant: bool,
    open_cards: &[crate::store::CardRow],
) -> Option<MissionWait> {
    if mission.state != "active" {
        return None;
    }
    let open: Vec<&crate::store::CardRow> =
        open_cards.iter().filter(|c| c.state == "open").collect();
    let oldest = |kind: Option<&str>| {
        open.iter()
            .filter(|c| kind.is_none_or(|k| c.kind == k))
            .map(|c| c.created_at)
            .min()
    };
    let n = u32::try_from(open.len()).unwrap_or(u32::MAX);
    let (reason, since) = if let Some(at) = oldest(Some("ask")) {
        (MissionWaitReason::Question, at)
    } else if mission.level > 0 && !live_grant {
        (MissionWaitReason::SignGrant, mission.updated_at)
    } else {
        (MissionWaitReason::Confirm, oldest(None)?)
    };
    Some(MissionWait {
        reason,
        since,
        open_cards: n,
    })
}

fn is_lifecycle_broken(row: &SessionRow) -> bool {
    matches!(
        row.safe_kill_state.as_deref(),
        Some("failed") | Some("requested")
    ) || row.status == "ghost"
        || row.lost_at.is_some()
}

fn since_for(row: &SessionRow, reason: Reason) -> i64 {
    match reason {
        Reason::Stuck => row.stuck_since.unwrap_or(row.last_activity_at),
        Reason::StopFailed => row.last_stop_at.unwrap_or(row.last_activity_at),
        Reason::ContextFull => row.context.context_at.unwrap_or(row.last_activity_at),
        Reason::StaleWorking => row.stale_working_at.unwrap_or(row.last_activity_at),
        Reason::CiFailing
        | Reason::AccountLimit
        | Reason::NoCredentials
        | Reason::ProbablyWaiting => row.idle_since.unwrap_or(row.last_activity_at),
        Reason::Lifecycle => row
            .lost_at
            .or(row.safe_kill_requested_at)
            .unwrap_or(row.last_activity_at),
        _ => row.last_activity_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain running session that needs nobody, to vary one field at a time.
    fn row() -> SessionRow {
        SessionRow {
            id: 1,
            row_version: 0,
            prompt_submit_seq: 0,
            tmux_name: "t".to_string(),
            host_alias: "alpha".to_string(),
            project_id: None,
            worktree_id: None,
            created_at: 0,
            last_activity_at: 100,
            status: "running".to_string(),
            notes: None,
            account_uuid: None,
            kind: "work".to_string(),
            reviews_session_id: None,
            worktree_key: None,
            lost_at: None,
            lost_reason: None,
            claude_session_id: None,
            claude_status: Some("working".to_string()),
            effort_level: None,
            pr_url: None,
            current_activity: None,
            context_pct: None,
            stuck_kind: None,
            friendly_name: None,
            safe_kill_state: None,
            safe_kill_nonce: None,
            safe_kill_detail: None,
            safe_kill_requested_at: None,
            idle_since: None,
            stuck_since: None,
            last_playbook_at: None,
            last_prompt: None,
            started_at: None,
            last_turn_at: None,
            ci_status: None,
            turn_seq: 0,
            last_stop_at: None,
            stale_working_at: None,
            stale_demoted_at: None,
            work_rev: 0,
            pr_evidence: None,
            pr_checked_at: None,
            owner_person_id: None,
            visibility: crate::store::VISIBILITY_UNCLAIMED.into(),
            claude_profile: None,
            agent: crate::store::AGENT_CLAUDE.into(),
            origin: None,
            origin_ref: None,
            last_viewed_at: None,
            turn_outcome: None,
            proposals: Vec::new(),
            pending_form: None,
            form_draft: None,
            parent_session_id: None,
            tags: Vec::new(),
            usage: Default::default(),
            context: Default::default(),
            pending_input: None,
            work: None,
            work_rejected: vec![],
            work_suggested: None,
            org_id: None,
        }
    }

    /// G1.6: what Jev reads as a question is a proposal: its own class,
    /// in the state the badge never counts, and it never outranks a reason
    /// a person must act on.
    #[test]
    fn jevs_question_is_probably_waiting_and_never_counted() {
        let mut r = row();
        r.claude_status = Some("idle".into());
        r.turn_outcome = Some("asked".into());
        r.idle_since = Some(60);
        let a = needs_attention(&r).unwrap();
        assert_eq!(
            a,
            Attention {
                reason: Reason::ProbablyWaiting,
                since: 60,
                state: State::Proposed,
            }
        );
        assert!(!a.state.counts_toward_badge());
        assert_eq!(
            serde_json::to_value(a).unwrap(),
            serde_json::json!({ "reason": "probably_waiting", "since": 60, "state": "proposed" })
        );
        // A real question on the row is Needs you, whatever Jev read.
        r.claude_status = Some("blocked".into());
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::Waiting);
    }

    fn mission(state: &str, level: i64) -> crate::store::MissionRow {
        serde_json::from_value(serde_json::json!({
            "id": 7, "name": "Hub federation v2", "goal": "g", "mode": "finite",
            "state": state, "level": level, "plan_version": 1, "created_at": 1,
            "updated_at": 40, "version": 1
        }))
        .unwrap()
    }

    fn card(kind: &str, state: &str, at: i64) -> crate::store::CardRow {
        crate::store::CardRow {
            id: at,
            mission_id: 7,
            decision_id: format!("d{at}"),
            source: "planner".into(),
            kind: kind.into(),
            work_item_id: None,
            payload: None,
            state: state.into(),
            note: None,
            created_at: at,
            decided_at: None,
            decided_by: None,
        }
    }

    /// G1.6: a mission waits on a person for a question, a grant to sign,
    /// or commands to confirm, in that order; an inactive one never does.
    #[test]
    fn a_mission_waits_on_a_question_a_grant_or_its_confirm_queue() {
        let cards = [
            card("run", "open", 50),
            card("ask", "open", 70),
            card("run", "applied", 10),
        ];
        assert_eq!(
            mission_waiting(&mission("active", 2), false, &cards),
            Some(MissionWait {
                reason: MissionWaitReason::Question,
                since: 70,
                open_cards: 2,
            })
        );
        assert_eq!(
            mission_waiting(&mission("active", 2), false, &cards[..1]).map(|w| (w.reason, w.since)),
            Some((MissionWaitReason::SignGrant, 40)),
            "autonomy asked, no grant: sign it"
        );
        assert_eq!(
            mission_waiting(&mission("active", 2), true, &cards[..1]).map(|w| (w.reason, w.since)),
            Some((MissionWaitReason::Confirm, 50)),
            "under a grant, an open card still waits"
        );
        assert_eq!(
            mission_waiting(&mission("active", 0), false, &cards[2..]),
            None,
            "level 0 asks for no grant, and a decided card waits on nobody"
        );
        for state in ["draft", "paused", "completed"] {
            assert_eq!(
                mission_waiting(&mission(state, 2), false, &cards),
                None,
                "{state}"
            );
        }
        assert_eq!(
            serde_json::to_value(mission_waiting(&mission("active", 1), false, &[]).unwrap())
                .unwrap(),
            serde_json::json!({ "reason": "sign_grant", "since": 40, "open_cards": 0 })
        );
    }

    #[test]
    fn a_blocked_session_is_waiting_and_an_ordinary_one_is_nothing() {
        let mut r = row();
        assert_eq!(needs_attention(&r), None, "a working session needs nobody");

        r.claude_status = Some("blocked".into());
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::Waiting);
    }

    #[test]
    fn a_session_waiting_on_a_form_needs_attention_while_working() {
        let mut r = row();
        assert_eq!(needs_attention(&r), None, "a working session needs nobody");
        r.pending_form = Some(crate::store::PendingForm {
            form_id: "f_x".into(),
            title: "T".into(),
        });
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::Waiting);
    }

    /// The order of the checks is the precedence, and this is the pair that
    /// makes it matter: a person can answer a blocked session now, and can do
    /// nothing about a ghost row until they are at a terminal.
    #[test]
    fn blocked_outranks_every_other_reason() {
        let mut r = row();
        r.claude_status = Some("blocked".into());
        r.stuck_kind = Some("auth_menu".into());
        r.status = "ghost".into();
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::Waiting);
    }

    #[test]
    fn stuck_failed_and_the_three_lifecycle_shapes_each_qualify() {
        let mut r = row();
        r.stuck_kind = Some("oom".into());
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::Stuck);

        // A tmux row's failed turn is a StopFailure (re-prompt); `Failed`
        // is the pane-less agent's verdict (see the F7 test below).
        let mut r = row();
        r.claude_status = Some("failed".into());
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::StopFailed);

        for broken in [
            |r: &mut SessionRow| r.safe_kill_state = Some("failed".into()),
            |r: &mut SessionRow| r.safe_kill_state = Some("requested".into()),
            |r: &mut SessionRow| r.status = "ghost".into(),
            |r: &mut SessionRow| r.lost_at = Some(99),
        ] {
            let mut r = row();
            broken(&mut r);
            assert_eq!(
                needs_attention(&r).map(|a| a.reason),
                Some(Reason::Lifecycle)
            );
        }
    }

    /// An external session is a Claude running outside fleet: nothing here can
    /// act on it, so reporting it would offer an action that does not exist.
    #[test]
    fn an_external_session_never_needs_a_person() {
        let mut r = row();
        r.kind = "external".into();
        r.claude_status = Some("blocked".into());
        r.stuck_kind = Some("oom".into());
        assert_eq!(needs_attention(&r), None);
    }

    /// A client draws an age from `since`, so it must always be a number.
    #[test]
    fn since_prefers_the_state_stamp_and_always_falls_back() {
        let mut r = row();
        r.last_activity_at = 100;
        r.stuck_kind = Some("oom".into());
        assert_eq!(
            needs_attention(&r).unwrap().since,
            100,
            "no stuck_since yet"
        );

        r.stuck_since = Some(140);
        assert_eq!(needs_attention(&r).unwrap().since, 140);

        let mut r = row();
        r.last_activity_at = 100;
        r.lost_at = Some(150);
        assert_eq!(needs_attention(&r).unwrap().since, 150);
    }

    /// F7: the model flagged three ghosts a person can do nothing about and
    /// missed five context-red rows, two stale `working` rows, a 429 and
    /// four idle sessions with failing CI.
    #[test]
    fn a_failed_turn_a_full_context_a_stale_row_and_failing_ci_each_qualify() {
        let mut r = row();
        r.claude_status = Some("failed".into());
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::StopFailed);
        let mut r = row();
        r.kind = "bg".into();
        r.claude_status = Some("failed".into());
        assert_eq!(
            needs_attention(&r).unwrap().reason,
            Reason::Failed,
            "a bg agent's exit is `claude agents`' verdict"
        );

        let mut r = row();
        r.context_pct = Some(85.0);
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::ContextFull);
        assert_eq!(
            needs_attention_with(&r, 90.0),
            None,
            "the threshold is the hub's setting"
        );

        let mut r = row();
        r.claude_status = Some("idle".into());
        r.stale_working_at = Some(50);
        assert_eq!(
            needs_attention(&r).unwrap(),
            Attention {
                reason: Reason::StaleWorking,
                since: 50,
                state: State::ActionRequired,
            }
        );

        let mut r = row();
        r.claude_status = Some("idle".into());
        r.ci_status = Some("failing".into());
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::CiFailing);
        let mut r = row();
        r.ci_status = Some("failing".into());
        assert_eq!(
            needs_attention(&r),
            None,
            "a working session may be fixing its CI"
        );

        let mut r = row();
        r.kind = "shell".into();
        r.context_pct = Some(99.0);
        assert_eq!(needs_attention(&r), None, "a shell has no Claude in it");
    }

    /// Every reason a person can act on outranks a broken lifecycle.
    #[test]
    fn every_actionable_reason_outranks_lifecycle() {
        for set in [
            (|r: &mut SessionRow| r.context_pct = Some(99.0)) as fn(&mut SessionRow),
            |r: &mut SessionRow| {
                r.claude_status = Some("idle".into());
                r.stale_working_at = Some(1);
            },
            |r: &mut SessionRow| {
                r.claude_status = Some("idle".into());
                r.ci_status = Some("failing".into());
            },
            |r: &mut SessionRow| r.claude_status = Some("failed".into()),
        ] {
            let mut r = row();
            set(&mut r);
            r.safe_kill_state = Some("failed".into());
            assert_ne!(needs_attention(&r).unwrap().reason, Reason::Lifecycle);
        }
    }

    /// A lost row keeps its last context reading and may keep a stale stamp;
    /// neither is anything a person can act on, so it reads `Lifecycle`.
    #[test]
    fn a_dead_rows_context_or_stale_stamp_reads_lifecycle() {
        for dead in [
            (|r: &mut SessionRow| r.status = "ghost".into()) as fn(&mut SessionRow),
            |r: &mut SessionRow| r.lost_at = Some(1),
        ] {
            let mut r = row();
            r.context_pct = Some(99.0);
            r.stale_working_at = Some(1);
            dead(&mut r);
            assert_eq!(needs_attention(&r).unwrap().reason, Reason::Lifecycle);
        }
    }

    /// The wire spellings are the desktop's bucket names; a rename on one
    /// side without the other would split the two classifiers silently.
    #[test]
    fn the_wire_spellings_match_the_desktop_buckets() {
        let ts = crate::repo_files::read("src/lib/attention.ts");
        for r in [
            Reason::Waiting,
            Reason::Stuck,
            Reason::HostDown,
            Reason::AccountLimit,
            Reason::NoCredentials,
            Reason::StopFailed,
            Reason::Failed,
            Reason::ContextFull,
            Reason::StaleWorking,
            Reason::CiFailing,
            Reason::Lifecycle,
        ] {
            assert!(
                ts.contains(&format!("'{}'", r.as_str())),
                "src/lib/attention.ts does not name the bucket {}",
                r.as_str()
            );
        }
    }

    /// The shared attention table (redesign step 0.4): the desktop reads
    /// `src/lib/attention_states.json` at runtime, so it must say exactly
    /// what this module does, and its rows must classify the same here as
    /// in `attention.test.ts`.
    #[test]
    fn the_shared_attention_table_matches_this_module() {
        let fixture: serde_json::Value =
            serde_json::from_str(&crate::repo_files::read("src/lib/attention_states.json"))
                .unwrap();

        let states: Vec<(String, bool)> = fixture["states"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["id"].as_str().unwrap().to_string(),
                    s["counted"].as_bool().unwrap(),
                )
            })
            .collect();
        let ours: Vec<(String, bool)> = State::ALL
            .iter()
            .map(|s| (s.as_str().to_string(), s.counts_toward_badge()))
            .collect();
        assert_eq!(states, ours, "states in attention_states.json");

        let buckets: Vec<(String, String)> = fixture["buckets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                (
                    b[0].as_str().unwrap().to_string(),
                    b[1].as_str().unwrap().to_string(),
                )
            })
            .collect();
        let ours: Vec<(String, String)> = BUCKET_STATES
            .iter()
            .map(|(b, s)| (b.to_string(), s.as_str().to_string()))
            .collect();
        assert_eq!(buckets, ours, "buckets in attention_states.json");

        // Each hub reason's state is its bucket's row in the table.
        let reasons = [
            Reason::Waiting,
            Reason::Stuck,
            Reason::HostDown,
            Reason::AccountLimit,
            Reason::NoCredentials,
            Reason::StopFailed,
            Reason::Failed,
            Reason::ContextFull,
            Reason::StaleWorking,
            Reason::CiFailing,
            Reason::ProbablyWaiting,
            Reason::Lifecycle,
        ];
        for r in reasons {
            let row = BUCKET_STATES.iter().find(|(b, _)| *b == r.as_str());
            assert_eq!(row.map(|(_, s)| *s), Some(r.state()), "{}", r.as_str());
        }

        let base = serde_json::to_value(row()).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let mut v = base.clone();
            for (k, val) in case["row"].as_object().unwrap() {
                v[k] = val.clone();
            }
            let r: SessionRow = serde_json::from_value(v).unwrap_or_else(|e| panic!("{name}: {e}"));
            let facts: Facts = case
                .get("facts")
                .map(|f| {
                    serde_json::from_value(f.clone()).unwrap_or_else(|e| panic!("{name}: {e}"))
                })
                .unwrap_or_default();
            let state = state_in(&r, DEFAULT_CONTEXT_RED_PCT, &facts);
            assert_eq!(state.as_str(), case["state"].as_str().unwrap(), "{name}");
            assert_eq!(
                state.counts_toward_badge(),
                case["counted"].as_bool().unwrap(),
                "{name}"
            );
            // Where the hub decides the bucket, it is the same one.
            let reason =
                needs_attention_in(&r, DEFAULT_CONTEXT_RED_PCT, &facts).map(|a| a.reason.as_str());
            let bucket = case["bucket"].as_str().unwrap();
            if reasons.iter().any(|r| r.as_str() == bucket) {
                assert_eq!(reason, Some(bucket), "{name}");
            } else {
                assert_eq!(reason, None, "{name}");
            }
        }
    }

    fn limited(uuid: &str) -> Facts {
        let mut f = Facts::default();
        f.limited_accounts.insert(
            uuid.into(),
            Limit {
                window: LimitWindow::Weekly,
                resets_at: Some(5_000),
            },
        );
        f
    }

    /// Step 2.4: the three Blocked reasons, each from a fact about the fleet.
    #[test]
    fn a_down_host_a_limited_account_and_a_lost_login_block_a_live_session() {
        let mut down = Facts::default();
        down.down_hosts.insert("alpha".into());
        let r = row();
        let a = needs_attention_in(&r, DEFAULT_CONTEXT_RED_PCT, &down).unwrap();
        assert_eq!(a.reason, Reason::HostDown);
        assert_eq!(a.reason.state(), State::Blocked);

        let mut r = row();
        r.account_uuid = Some("acc".into());
        r.claude_status = Some("idle".into());
        r.idle_since = Some(70);
        let a = needs_attention_in(&r, DEFAULT_CONTEXT_RED_PCT, &limited("acc")).unwrap();
        assert_eq!(
            a,
            Attention {
                reason: Reason::AccountLimit,
                since: 70,
                state: State::Blocked,
            }
        );
        r.claude_status = Some("working".into());
        assert_eq!(
            needs_attention_in(&r, DEFAULT_CONTEXT_RED_PCT, &limited("acc")),
            None,
            "a working session is not paused by its account's limit yet"
        );

        let mut gone = Facts::default();
        gone.uncredentialed_accounts.insert("acc".into());
        r.claude_status = Some("failed".into());
        assert_eq!(
            needs_attention_in(&r, DEFAULT_CONTEXT_RED_PCT, &gone)
                .unwrap()
                .reason,
            Reason::NoCredentials,
            "a turn that failed on a lost login reads as the login"
        );
    }

    #[test]
    fn a_dead_row_on_a_down_host_keeps_lifecycle_and_a_question_still_wins() {
        let mut down = Facts::default();
        down.down_hosts.insert("alpha".into());
        let mut r = row();
        r.lost_at = Some(10);
        assert_eq!(
            needs_attention_in(&r, DEFAULT_CONTEXT_RED_PCT, &down)
                .unwrap()
                .reason,
            Reason::Lifecycle
        );
        let mut r = row();
        r.claude_status = Some("blocked".into());
        assert_eq!(
            needs_attention_in(&r, DEFAULT_CONTEXT_RED_PCT, &down)
                .unwrap()
                .reason,
            Reason::Waiting
        );
    }

    /// Review r05 F7: a full window with no reset time blocks only while the
    /// reading is younger than the window; an 8-day-old weekly one does not.
    #[test]
    fn a_limit_with_no_reset_time_lapses_with_its_window() {
        use crate::service::account_usage::{AccountUsage, Window};
        let now = 1_000_000;
        let snap = |fetched_at: i64| AccountUsageSnapshot {
            account_uuid: "acc".into(),
            usage: Some(AccountUsage {
                five_hour: None,
                seven_day: Some(Window {
                    utilization: 100.0,
                    resets_at: None,
                }),
                seven_day_opus: None,
                seven_day_sonnet: None,
            }),
            subscription: None,
            fetched_at: Some(fetched_at),
            source_host: None,
            status: UsageOutcomeKind::Ok,
            detail: None,
            next_try_at: 0,
        };
        let fresh = Facts::from_fleet(&[], &[snap(now - 86_400)], now);
        assert_eq!(fresh.limited_accounts["acc"].window, LimitWindow::Weekly);
        let old = Facts::from_fleet(&[], &[snap(now - 8 * 86_400)], now);
        assert!(old.limited_accounts.is_empty(), "{old:?}");
    }

    #[test]
    fn facts_come_from_pinged_hosts_and_the_usage_snapshots() {
        use crate::service::account_usage::{AccountUsage, Window};
        let host = |alias: &str, reachable: bool, pinged: Option<i64>| HostRow {
            alias: alias.into(),
            ssh_alias: None,
            reachable,
            claude_version: None,
            tmux_version: None,
            hidden: false,
            last_pinged_at: pinged,
            account_uuid: None,
            provisioned: false,
            transport: "ssh".into(),
            org_id: None,
            claude_version_at: None,
            disk_home_free_kb: None,
            disk_home_total_kb: None,
            disk_tmp_free_kb: None,
            load_1m: None,
            mem_avail_kb: None,
            uptime_secs: None,
            health_at: None,
            last_hook_at: None,
            agent_version: None,
            provisioned_at: None,
            provision_stale: false,
            unclaimed_sessions: None,
            provision_warning: None,
            auth_overrides: None,
            claude_profiles: None,
            cpu_count: None,
            mem_total_kb: None,
            boot_at: None,
            latency_ms: None,
            worktree_kb: None,
            worktree_at: None,
            agents_on_path: None,
            last_reachable_at: None,
            last_probe_error_code: None,
            last_probe_error: None,
            harnesses: None,
        };
        let snap = |uuid: &str, status: UsageOutcomeKind, five: f64, week: f64, resets: i64| {
            AccountUsageSnapshot {
                account_uuid: uuid.into(),
                usage: Some(AccountUsage {
                    five_hour: Some(Window {
                        utilization: five,
                        resets_at: Some(resets),
                    }),
                    seven_day: Some(Window {
                        utilization: week,
                        resets_at: Some(resets + 100),
                    }),
                    seven_day_opus: None,
                    seven_day_sonnet: None,
                }),
                subscription: None,
                fetched_at: Some(900),
                source_host: None,
                status,
                detail: None,
                next_try_at: 0,
            }
        };
        let f = Facts::from_fleet(
            &[
                host("up", true, Some(1)),
                host("down", false, Some(1)),
                host("new", false, None),
            ],
            &[
                snap("five", UsageOutcomeKind::Ok, 100.0, 40.0, 2_000),
                snap("both", UsageOutcomeKind::Ok, 100.0, 100.0, 2_000),
                snap("reset", UsageOutcomeKind::Ok, 100.0, 10.0, 500),
                snap("fine", UsageOutcomeKind::Ok, 80.0, 99.0, 2_000),
                snap("gone", UsageOutcomeKind::LoginExpired, 0.0, 0.0, 2_000),
                snap("rejected", UsageOutcomeKind::TokenRejected, 0.0, 0.0, 2_000),
                // Review r05 F3: no token file to read (a macOS host keeps
                // it in the Keychain) is not a lost login.
                snap("keychain", UsageOutcomeKind::NoCredentials, 0.0, 0.0, 2_000),
                snap(
                    "refresh",
                    UsageOutcomeKind::AccessTokenExpired,
                    0.0,
                    0.0,
                    2_000,
                ),
            ],
            1_000,
        );
        assert_eq!(f.down_hosts.iter().collect::<Vec<_>>(), ["down"]);
        assert_eq!(
            f.limited_accounts.get("five"),
            Some(&Limit {
                window: LimitWindow::FiveHour,
                resets_at: Some(2_000)
            })
        );
        assert_eq!(f.limited_accounts["both"].window, LimitWindow::Weekly);
        assert!(
            !f.limited_accounts.contains_key("reset"),
            "a window already reset"
        );
        assert!(!f.limited_accounts.contains_key("fine"));
        assert_eq!(
            f.uncredentialed_accounts.iter().collect::<Vec<_>>(),
            ["gone", "rejected"]
        );
        // So an idle session on the Keychain host's account needs no one.
        let mut r = row();
        r.account_uuid = Some("keychain".into());
        r.claude_status = Some("idle".into());
        assert_eq!(needs_attention_in(&r, DEFAULT_CONTEXT_RED_PCT, &f), None);
        r.account_uuid = Some("gone".into());
        assert_eq!(
            needs_attention_in(&r, DEFAULT_CONTEXT_RED_PCT, &f)
                .unwrap()
                .reason,
            Reason::NoCredentials
        );
        // Both at their limit, the week freeing first: the 5-hour window
        // still holds the account after the weekly reset.
        let mut late = snap("late", UsageOutcomeKind::Ok, 100.0, 100.0, 3_000);
        late.usage
            .as_mut()
            .unwrap()
            .seven_day
            .as_mut()
            .unwrap()
            .resets_at = Some(1_500);
        let f = Facts::from_fleet(&[], &[late], 1_000);
        assert_eq!(
            f.limited_accounts.get("late"),
            Some(&Limit {
                window: LimitWindow::FiveHour,
                resets_at: Some(3_000)
            })
        );
    }
}
