//! The operator feed: Control's own session hears what changed in the fleet
//! without having to ask.
//!
//! Before this, the operator learned that a session it started had
//! finished, stalled or died only when the person asked it to look, so
//! "start three sessions and tell me when they are done" could not be kept:
//! nothing ever told it. This tick reads what changed since its cursor and,
//! when there is something, types ONE compact `[fleet events]` digest into
//! the operator at its next idle moment (`sessions::deferred`, so it never
//! lands mid-turn or on top of the person's typing).
//!
//! **What it covers.**
//! * Sessions Control answers for — those it started (`origin =
//!   'operator'`) and those it handed work to (a `control_handoffs`
//!   receipt names them): a turn finished, a dialog or a stall waiting on
//!   someone, a failure, the session lost, brought back by
//!   `service::auto_restore`, or past bringing back. Read off the session
//!   timeline by id ([`Store::operator_feed_events`]), so nothing is read
//!   twice and nothing is skipped.
//! * Missions Control handed off that now wait on a person
//!   (`attention::mission_waiting`: a question, a grant to sign, cards to
//!   confirm).
//! * Hosts that went unreachable or run low on disk or memory, as
//!   `fleet_health` judges them.
//!
//! The last two are states, not events: they are reported when they are
//! first seen (`FeedState::raised`) and again only after they cleared and
//! came back.
//!
//! **What it never does.** It never reports Control's own session (its row
//! is excluded in SQL), never runs while automation is paused (the
//! `operator_feed` loop gate, and [`run_with`] asks again), and never quotes
//! a session's words: a turn's last message is another agent's text, and a
//! digest the operator reads as fleet's must not carry instructions from
//! one session into the agent that drives all of them. It names the
//! session; `session_conversation` is one call away.
//!
//! **How often.** At most one digest per `control.feed_min_interval_secs`
//! (15 minutes by default) and [`MAX_DIGESTS_PER_HOUR`] an hour; while one
//! is still queued (the operator busy), nothing new is queued, so changes
//! pile up into the next one instead of a stack of notes.
//!
//! **Only for real events** (Control review F11). A digest goes out only
//! when something in it needs Control ([`What::needs_control`]: a session
//! waiting or stuck, a failure, failing checks, a lost session, a host
//! down, a mission waiting on a person); a finished turn, a merge or an
//! automatic restore is kept in [`FeedState::backlog`] and rides along
//! with the next digest that goes. Events older than
//! [`EVENT_MAX_AGE_SECS`] when finally sent are not read out one by one:
//! they become ONE "while you were away" line of counts
//! ([`away_line`]), so an operator that was down for a day hears that
//! something happened without yesterday read to it.
//!
//! **Only what Control still follows** (Control review F10,
//! `service::control_follow`): a session done, killed, merged, quiet for
//! `control.follow_quiet_hours` or muted, and a muted mission, are left
//! out.
//!
//! **Whose words.** The digest opens with `[fleet events]`
//! (`prompt_origin::FLEET_PROMPT_MARKS`), so the typing step records it as
//! fleet's (`prompt::Origin::Fleet`) and nothing reads it as the person.
//!
//! The cursor and the raised set live in one setting, [`FEED_STATE_KEY`]:
//! no migration, and a hub restart picks up where it stopped.

use crate::ipc_error::IpcError;
use crate::service::settings;
use crate::ssh::SshClient;
use crate::store::{FeedEventRow, SessionRow, Store};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The setting holding [`FeedState`] as JSON. Internal bookkeeping, not a
/// person's setting: no `SPECS` row, never shown.
pub const FEED_STATE_KEY: &str = "control.feed_state";
/// The mark a digest opens with (one of `prompt_origin::FLEET_PROMPT_MARKS`).
pub const DIGEST_MARK: &str = "[fleet events]";
/// Digests one hour may carry, whatever the interval setting says: a fleet
/// in trouble produces events every tick, and the operator's context is
/// worth more than a running commentary.
pub const MAX_DIGESTS_PER_HOUR: usize = 6;
/// Lines one digest shows; the rest are counted ("and 4 more").
pub const MAX_LINES: usize = 12;
/// Events older than this when a digest goes out are dropped.
pub const EVENT_MAX_AGE_SECS: i64 = 3600;
/// Events read per pass. The cursor advances only past what was read, so a
/// longer backlog goes out over the next passes.
pub const READ_LIMIT: i64 = 500;

/// The timeline kinds the feed reads. Each maps to one [`What`] in
/// [`classify`]; anything else on the timeline is not the operator's
/// business.
pub const KINDS: [&str; 13] = [
    crate::store::PR_EVENT_CI_FAILED,
    crate::store::PR_EVENT_MERGED,
    "turn_done",
    "task_done",
    "notification",
    "stuck",
    "stop_failure",
    "task_failed",
    "session_restore_failed",
    "lost",
    crate::service::auto_restore::EVENT_RESTORED,
    crate::service::auto_restore::EVENT_FAILED,
    crate::service::auto_restore::EVENT_SKIPPED,
];

/// The `notification` types that mean a session waits on someone
/// (`hooks::notification_effect`'s blocked ones). `quota_auto_resume_fired`
/// is the session going again, not a wait.
const BLOCKING_NOTIFICATIONS: [&str; 5] = [
    "permission_prompt",
    "elicitation_dialog",
    "elicitation_url_dialog",
    "quota_auto_resume_stale",
    "quota_auto_resume_disabled",
];

/// What happened to a session, as the digest words it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum What {
    Finished,
    /// Waiting on an answer (a dialog) or stuck on a prompt.
    Blocked,
    /// A turn or a task failed; the word is the failure's kind when the
    /// detail starts with one (`rate_limit`, …).
    Failed(Option<String>),
    /// Lost, with its `lost_reason`.
    Lost(String),
    AutoRestored,
    /// An automatic restore failed or the budget is spent: a person's
    /// Restore is what is left.
    NotRestorable,
    /// Its pull request's checks turned failing.
    CiFailing,
    /// Its pull request merged (Control stops following it, F10).
    Merged,
}

impl What {
    /// One digest line per session and kind of change: two finished turns
    /// are one "finished its turn".
    fn slot(&self) -> &'static str {
        match self {
            What::Finished => "finished",
            What::Blocked => "blocked",
            What::Failed(_) => "failed",
            What::Lost(_) => "lost",
            What::AutoRestored => "restored",
            What::NotRestorable => "not_restorable",
            What::CiFailing => "ci_failing",
            What::Merged => "merged",
        }
    }

    /// PURE: whether this needs Control (F11): a digest goes out for it.
    /// A finished turn, a merge and an automatic restore are routine: they
    /// wait for the next digest something else sends.
    pub fn needs_control(&self) -> bool {
        match self {
            What::Finished | What::AutoRestored | What::Merged => false,
            What::Blocked
            | What::Failed(_)
            | What::Lost(_)
            | What::NotRestorable
            | What::CiFailing => true,
        }
    }

    fn words(&self) -> String {
        match self {
            What::Finished => "finished its turn".into(),
            What::Blocked => "is waiting on an answer or stuck".into(),
            What::Failed(Some(w)) => format!("failed ({w})"),
            What::Failed(None) => "failed".into(),
            What::Lost(r) => format!(
                "was lost: {}",
                crate::service::auto_restore::reason_words(r)
            ),
            What::AutoRestored => "was brought back automatically".into(),
            What::NotRestorable => {
                "could not be brought back automatically; it needs a Restore".into()
            }
            What::CiFailing => "has failing checks on its pull request".into(),
            What::Merged => "had its pull request merged".into(),
        }
    }
}

/// PURE: a vocabulary word at the head of `detail` (`rate_limit: …` →
/// `rate_limit`), or nothing. Only `[a-z0-9_]`, so free text never reaches
/// the digest.
fn head_word(detail: Option<&str>) -> Option<String> {
    let w = detail?.split([':', ' ']).next()?.trim();
    (!w.is_empty()
        && w.len() <= 40
        && w.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'))
    .then(|| w.to_string())
}

/// PURE: what a timeline event means for the digest, `None` for one it
/// does not report.
pub fn classify(kind: &str, detail: Option<&str>) -> Option<What> {
    use crate::service::auto_restore::{EVENT_FAILED, EVENT_RESTORED, EVENT_SKIPPED};
    Some(match kind {
        "turn_done" | "task_done" => What::Finished,
        "notification" if detail.is_some_and(|d| BLOCKING_NOTIFICATIONS.contains(&d)) => {
            What::Blocked
        }
        "stuck" => What::Blocked,
        "stop_failure" | "task_failed" | "session_restore_failed" => {
            What::Failed(head_word(detail))
        }
        "lost" => What::Lost(detail.unwrap_or("missing").to_string()),
        k if k == EVENT_RESTORED => What::AutoRestored,
        k if k == EVENT_FAILED || k == EVENT_SKIPPED => What::NotRestorable,
        crate::store::PR_EVENT_CI_FAILED => What::CiFailing,
        crate::store::PR_EVENT_MERGED => What::Merged,
        _ => return None,
    })
}

/// One line of a digest, with the key it is deduplicated on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedLine {
    pub key: String,
    pub text: String,
}

/// PURE: one event as a digest line (its key and words) and what it was,
/// `None` for one the digest does not report.
fn line_of(e: &FeedEventRow) -> Option<(FeedLine, What)> {
    let what = classify(&e.kind, e.detail.as_deref())?;
    let key = format!("s{}:{}", e.session_id, what.slot());
    let who = if e.started_by_control {
        ", you started it"
    } else {
        ""
    };
    let text = format!(
        "session {} on {} (#{}{who}) {}",
        e.tmux_name,
        e.host_alias,
        e.session_id,
        what.words()
    );
    Some((FeedLine { key, text }, what))
}

/// PURE: the session lines of a batch of events, one per session and kind
/// of change, in the order each first happened, worded by its latest event.
/// Events older than `now - EVENT_MAX_AGE_SECS` are left out.
pub fn session_lines(events: &[FeedEventRow], now: i64) -> Vec<FeedLine> {
    let mut order: Vec<String> = Vec::new();
    let mut latest: BTreeMap<String, String> = BTreeMap::new();
    for e in events.iter().filter(|e| e.at >= now - EVENT_MAX_AGE_SECS) {
        let Some((FeedLine { key, text }, _)) = line_of(e) else {
            continue;
        };
        if !latest.contains_key(&key) {
            order.push(key.clone());
        }
        latest.insert(key, text);
    }
    order
        .into_iter()
        .map(|key| {
            let text = latest.remove(&key).unwrap_or_default();
            FeedLine { key, text }
        })
        .collect()
}

/// A session line waiting for the next digest (F11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    pub key: String,
    pub text: String,
    /// Its latest event, unix seconds.
    pub at: i64,
    /// [`What::needs_control`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub urgent: bool,
}

/// Lines a backlog keeps; the oldest past it are counted in [`Away`].
pub const BACKLOG_MAX: usize = 100;

/// What happened more than [`EVENT_MAX_AGE_SECS`] before it could be told
/// (F11): counted, not read out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Away {
    /// Events per kind of change ([`What::slot`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub counts: BTreeMap<String, u32>,
    /// The sessions they were on, by name.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub sessions: BTreeSet<String>,
    /// One of them needs Control.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub urgent: bool,
}

impl Away {
    fn add(&mut self, slot: &str, session: &str, urgent: bool) {
        *self.counts.entry(slot.to_string()).or_default() += 1;
        self.sessions.insert(session.to_string());
        self.urgent |= urgent;
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }
}

/// PURE: the slot a held line's key names (`s4:failed` → `failed`).
fn slot_of(key: &str) -> &str {
    key.rsplit(':').next().unwrap_or(key)
}

/// PURE: the "while you were away" line for `a`, `None` when empty.
pub fn away_line(a: &Away) -> Option<String> {
    if a.is_empty() {
        return None;
    }
    let words = |slot: &str| match slot {
        "finished" => "finished turns",
        "blocked" => "waits on an answer",
        "failed" => "failures",
        "lost" => "lost sessions",
        "restored" => "automatic restores",
        "not_restorable" => "sessions left needing a Restore",
        "ci_failing" => "pull requests with failing checks",
        "merged" => "merged pull requests",
        _ => "other changes",
    };
    let what = a
        .counts
        .iter()
        .map(|(slot, n)| format!("{n} {}", words(slot)))
        .collect::<Vec<_>>()
        .join(", ");
    const NAMES: usize = 4;
    let mut names = a
        .sessions
        .iter()
        .take(NAMES)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if a.sessions.len() > NAMES {
        names.push_str(&format!(" and {} more", a.sessions.len() - NAMES));
    }
    Some(format!(
        "while you were away (over an hour ago): {what}, on {names}; `list_sessions` has where they are now"
    ))
}

/// PURE: fold a batch of events into `state`'s backlog (F11): a recent
/// event replaces its session-and-kind line (worded by the latest), an
/// event older than `now - EVENT_MAX_AGE_SECS` (and a held line that grew
/// that old) is counted in `state.away`; past [`BACKLOG_MAX`] lines the
/// oldest are counted too.
pub fn fold(state: &mut FeedState, events: &[FeedEventRow], now: i64) {
    let cutoff = now - EVENT_MAX_AGE_SECS;
    for e in events {
        let Some((line, what)) = line_of(e) else {
            continue;
        };
        let urgent = what.needs_control();
        if e.at < cutoff {
            state.away.add(what.slot(), &e.tmux_name, urgent);
            continue;
        }
        match state.backlog.iter_mut().find(|h| h.key == line.key) {
            Some(h) => {
                h.text = line.text;
                h.at = h.at.max(e.at);
            }
            None => state.backlog.push(Held {
                key: line.key,
                text: line.text,
                at: e.at,
                urgent,
            }),
        }
    }
    let (old, keep): (Vec<Held>, Vec<Held>) = std::mem::take(&mut state.backlog)
        .into_iter()
        .partition(|h| h.at < cutoff);
    state.backlog = keep;
    let over = state.backlog.len().saturating_sub(BACKLOG_MAX);
    let dropped: Vec<Held> = state.backlog.drain(..over).collect();
    for h in old.iter().chain(&dropped) {
        // The line names its session first: "session NAME on …".
        let name = h
            .text
            .strip_prefix("session ")
            .and_then(|t| t.split(' ').next())
            .unwrap_or("?");
        state.away.add(slot_of(&h.key), name, h.urgent);
    }
}

/// PURE: whether `state` (with `fresh` conditions raised this pass) has
/// something that needs Control, so a digest goes out (F11).
pub fn needs_control(state: &FeedState, fresh_conditions: usize) -> bool {
    fresh_conditions > 0 || state.away.urgent || state.backlog.iter().any(|h| h.urgent)
}

/// PURE: the digest lines of `state` and this pass's fresh conditions:
/// what needs Control first, then the routine lines, then the away line.
pub fn digest_lines(state: &FeedState, fresh: &[String]) -> Vec<String> {
    let urgent = state.backlog.iter().filter(|h| h.urgent);
    let routine = state.backlog.iter().filter(|h| !h.urgent);
    urgent
        .map(|h| h.text.clone())
        .chain(fresh.iter().cloned())
        .chain(routine.map(|h| h.text.clone()))
        .chain(away_line(&state.away))
        .collect()
}

/// A standing condition (a host down, a mission waiting): reported when
/// first seen, again only after it cleared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Condition {
    pub key: String,
    pub text: String,
}

/// PURE: a host's conditions now. `down` outranks the others: an
/// unreachable host's disk reading says nothing about now.
pub fn host_conditions(
    alias: &str,
    reachable: bool,
    disk_low: Option<u8>,
    mem_low: Option<f64>,
) -> Vec<Condition> {
    if !reachable {
        return vec![Condition {
            key: format!("h:{alias}:down"),
            text: format!("host {alias} is unreachable"),
        }];
    }
    let mut out = Vec::new();
    if let Some(p) = disk_low {
        out.push(Condition {
            key: format!("h:{alias}:disk"),
            text: format!("host {alias} is low on disk ({p}% used)"),
        });
    }
    if let Some(p) = mem_low {
        out.push(Condition {
            key: format!("h:{alias}:mem"),
            text: format!("host {alias} is low on memory ({p:.0}% available)"),
        });
    }
    out
}

/// PURE: the condition of a handed-off mission waiting on a person.
pub fn mission_condition(id: i64, name: &str, reason: &str) -> Condition {
    let why = match reason {
        "question" => "its planner asked a question only a person answers",
        "sign_grant" => "its autonomy grant needs signing",
        _ => "commands wait for a person's confirmation",
    };
    Condition {
        key: format!("m{id}:{reason}"),
        text: format!("mission #{id} \"{name}\" waits on a person: {why}"),
    }
}

/// PURE: the digest for `lines`, `None` when there is nothing to say. At
/// most [`MAX_LINES`] lines; the rest are counted.
pub fn build_digest(lines: &[String]) -> Option<String> {
    if lines.is_empty() {
        return None;
    }
    let mut out = format!(
        "{DIGEST_MARK} What changed since the last note. Nothing here asks for a reply: act \
         only if one of these is yours to follow up, and say so to the person in a line. \
         `list_sessions` and `session_conversation` have the details.\n"
    );
    for l in lines.iter().take(MAX_LINES) {
        out.push_str("- ");
        out.push_str(l);
        out.push('\n');
    }
    if lines.len() > MAX_LINES {
        out.push_str(&format!("- … and {} more.\n", lines.len() - MAX_LINES));
    }
    Some(out.trim_end().to_string())
}

/// What the feed remembers between passes ([`FEED_STATE_KEY`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedState {
    /// The newest timeline id already read. `None` until the first pass,
    /// which starts at the newest id so turning the feed on replays nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<i64>,
    /// When the digests of the last hour went out (the hourly cap).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sent: Vec<i64>,
    /// The standing conditions already reported and still true.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub raised: BTreeSet<String>,
    /// Session lines read but not told yet: nothing needed Control (F11).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backlog: Vec<Held>,
    /// What grew too old to read out, counted (F11).
    #[serde(default, skip_serializing_if = "Away::is_empty")]
    pub away: Away,
}

impl FeedState {
    fn load(s: &Store) -> Self {
        s.get_setting(FEED_STATE_KEY)
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }

    fn save(&self, s: &Store) {
        let j = serde_json::to_string(self).unwrap_or_default();
        if let Err(e) = s.set_setting(FEED_STATE_KEY, &j) {
            tracing::debug!(error = %e, "[operator-feed] state not saved");
        }
    }
}

/// The settings a pass reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedConfig {
    pub enabled: bool,
    pub min_interval_secs: i64,
}

impl FeedConfig {
    pub fn from_store(s: &Store) -> Self {
        Self {
            enabled: settings::get_bool(s, settings::CONTROL_REACT_TO_EVENTS),
            min_interval_secs: settings::get_secs(s, settings::CONTROL_FEED_MIN_INTERVAL_SECS)
                as i64,
        }
    }
}

/// What a pass may do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Read, and send a digest when there is something to say.
    Send,
    /// Not now: leave the cursor where it is so the changes wait.
    Hold(&'static str),
    /// Nobody to tell (off, or no operator): forget the cursor, so turning
    /// it on later starts from then, not from a backlog.
    Reset,
}

/// PURE: whether a pass may send, given the operator's state and the
/// feed's history.
///
/// `operator` is `None` when no operator was ever made (or its row is
/// gone), else whether its session can take a prompt at all (running, an
/// agent pane). `pending` is a digest still queued, undelivered.
pub fn gate(
    cfg: &FeedConfig,
    state: &FeedState,
    operator: Option<bool>,
    pending: bool,
    now: i64,
) -> Gate {
    if !cfg.enabled {
        return Gate::Reset;
    }
    let Some(ready) = operator else {
        return Gate::Reset;
    };
    if !ready {
        return Gate::Hold("operator_not_running");
    }
    if pending {
        return Gate::Hold("digest_pending");
    }
    if state
        .sent
        .last()
        .is_some_and(|&t| now - t < cfg.min_interval_secs)
    {
        return Gate::Hold("min_interval");
    }
    if state.sent.iter().filter(|&&t| now - t < 3600).count() >= MAX_DIGESTS_PER_HOUR {
        return Gate::Hold("hourly_cap");
    }
    Gate::Send
}

/// Side effects of a pass, injected for tests.
#[async_trait]
pub trait FeedExec: Send + Sync {
    /// Queue `body` for the session's next idle moment.
    async fn queue(&self, session_id: i64, body: String) -> Result<(), IpcError>;
}

/// Production executor over the deferred queue.
pub struct RealFeedExec {
    pub store: Arc<Mutex<Store>>,
    pub ssh: Arc<SshClient>,
}

#[async_trait]
impl FeedExec for RealFeedExec {
    async fn queue(&self, session_id: i64, body: String) -> Result<(), IpcError> {
        crate::service::sessions::deferred::queue_prompt(
            crate::service::sessions::deferred::QueuePromptArgs {
                session_id,
                prompt: body,
                ..Default::default()
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map(|_| ())
    }
}

/// What a pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeedReport {
    /// A digest was queued (or typed at once).
    pub sent: bool,
    /// Its lines.
    pub lines: usize,
    /// Why nothing went out, when a gate held it.
    pub held: Option<&'static str>,
}

/// The operator's row, when one was ever made and still has one.
fn operator_row(s: &Store) -> Option<SessionRow> {
    let r = crate::service::operator::operator_ref(s)?;
    s.find_sessions_by_tmux_name(&r.tmux_name, Some(&r.host_alias))
        .ok()?
        .into_iter()
        .next()
}

/// Whether a digest is still waiting in the operator's queue.
fn digest_pending(s: &Store, session_id: i64) -> bool {
    s.list_deferred_prompts(Some(session_id))
        .unwrap_or_default()
        .iter()
        .any(|p| {
            p.delivered_at.is_none()
                && p.failed_at.is_none()
                && p.cancelled_at.is_none()
                && p.skipped_at.is_none()
                && p.body.starts_with(DIGEST_MARK)
        })
}

/// Every standing condition now: hosts as `fleet_health` judges them (the
/// hidden and disabled left out), and the handed-off missions waiting on a
/// person.
fn conditions(s: &Store, now: i64) -> Vec<Condition> {
    use crate::service::health;
    let local = crate::service::hub::local_host_enabled();
    let hosts = crate::service::hosts::active_hosts(s.list_hosts().unwrap_or_default(), local);
    let t = health::host_thresholds(s);
    let mut out = Vec::new();
    for h in &hosts {
        let rows = health::hosts_health(std::slice::from_ref(h), &[], &t, &[], "", now);
        let disk = rows
            .first()
            .filter(|r| r.disk_low)
            .and_then(|r| r.disk_home_pct);
        let mem = health::mem_avail_pct(h, now).filter(|_| health::mem_low(h, t.mem_low_pct, now));
        out.extend(host_conditions(&h.alias, h.reachable, disk, mem));
    }
    for id in s.handed_off_mission_ids().unwrap_or_default() {
        // F10: a mission the person muted is not told.
        if crate::service::control_follow::mission_muted(s, id) {
            continue;
        }
        let Ok(Some(mut m)) = s.get_mission(id) else {
            continue;
        };
        if m.state != "active" {
            continue;
        }
        if crate::service::work::orchestrate::fill_spend(s, &mut m, now).is_err() {
            continue;
        }
        if let Some(w) = m.waiting_on {
            out.push(mission_condition(m.id, &m.name, w.reason.as_str()));
        }
    }
    out
}

/// One pass: read what changed, and queue a digest for the operator when
/// the gates allow and there is something to say.
pub async fn run_with(
    store: &Mutex<Store>,
    exec: &dyn FeedExec,
    cfg: &FeedConfig,
    now: i64,
) -> FeedReport {
    let (op_id, body, mut next) = {
        let Ok(s) = store.lock() else {
            return FeedReport::default();
        };
        // Pause all (redesign 8.1): the tick's gate records it; this stops
        // any other caller too.
        if crate::service::loops::paused(&s) {
            return FeedReport {
                held: Some("paused"),
                ..Default::default()
            };
        }
        let mut state = FeedState::load(&s);
        let op = operator_row(&s);
        let ready = op.as_ref().map(|r| {
            r.status == "running" && !crate::store::has_no_pane(&r.kind) && r.kind != "shell"
        });
        let pending = op.as_ref().is_some_and(|r| digest_pending(&s, r.id));
        match gate(cfg, &state, ready, pending, now) {
            Gate::Reset => {
                if state != FeedState::default() {
                    FeedState::default().save(&s);
                }
                return FeedReport::default();
            }
            Gate::Hold(why) => {
                return FeedReport {
                    held: Some(why),
                    ..Default::default()
                }
            }
            Gate::Send => {}
        }
        let op = op.expect("Send has an operator");
        let Some(cursor) = state.cursor else {
            // First pass: start from now, not from the fleet's history.
            state.cursor = Some(s.newest_session_event_id().unwrap_or(0));
            state.save(&s);
            return FeedReport::default();
        };
        let mut events = s
            .operator_feed_events(cursor, Some(op.id), &KINDS, READ_LIMIT)
            .unwrap_or_default();
        state.cursor = Some(events.last().map_or(cursor, |e| e.id));
        // F10: only the sessions Control still follows. A merge is told
        // once (it is what stops the following), unless muted.
        let follows = crate::service::control_follow::follows(&s, Some(op.id), now);
        let followed = |id: i64, kind: &str| {
            follows.iter().any(|f| {
                f.kind == "session"
                    && f.id == id
                    && (f.state.following() || (kind == crate::store::PR_EVENT_MERGED && !f.muted))
            })
        };
        events.retain(|e| followed(e.session_id, &e.kind));
        fold(&mut state, &events, now);
        let now_raised = conditions(&s, now);
        let fresh: Vec<String> = now_raised
            .iter()
            .filter(|c| !state.raised.contains(&c.key))
            .map(|c| c.text.clone())
            .collect();
        state.raised = now_raised.into_iter().map(|c| c.key).collect();
        state.sent.retain(|&t| now - t < 3600);
        if !needs_control(&state, fresh.len()) {
            // Nothing needs Control: move past what was read, keep the
            // routine lines for the next digest and what is raised.
            if state != FeedState::load(&s) {
                state.save(&s);
            }
            let held =
                (!state.backlog.is_empty() || !state.away.is_empty()).then_some("nothing_urgent");
            return FeedReport {
                held,
                ..Default::default()
            };
        }
        let body = build_digest(&digest_lines(&state, &fresh));
        state.backlog.clear();
        state.away = Away::default();
        (op.id, body, state)
    };
    let body = body.expect("checked above");
    let lines = body.lines().count() - 1;
    match exec.queue(op_id, body).await {
        Ok(()) => {
            next.sent.push(now);
            if let Ok(s) = store.lock() {
                next.save(&s);
            }
            FeedReport {
                sent: true,
                lines,
                held: None,
            }
        }
        Err(e) => {
            // The cursor stays: the same changes go out on a later pass.
            tracing::debug!(error = %e.message, "[operator-feed] digest not queued");
            FeedReport {
                held: Some("queue_failed"),
                ..Default::default()
            }
        }
    }
}

static RUNNING: AtomicBool = AtomicBool::new(false);

/// The tick's entry: gated (`operator_feed`, stops while paused), cheap
/// when off, detached and single-flight.
pub fn maybe_run(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) {
    let cfg = {
        let Ok(s) = store.lock() else {
            return;
        };
        if !crate::service::loops::gate_in("operator_feed", &s, None) {
            return;
        }
        FeedConfig::from_store(&s)
    };
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let store = Arc::clone(store);
    let ssh = Arc::clone(ssh);
    crate::rt::spawn(async move {
        let _flight = crate::rt::ClearOnDrop::of_static(&RUNNING);
        let exec = RealFeedExec {
            store: Arc::clone(&store),
            ssh,
        };
        let r = run_with(&store, &exec, &cfg, crate::store::now_unix()).await;
        crate::service::loops::report("operator_feed", Ok::<_, String>(()), None);
        if r.sent {
            tracing::info!("[operator-feed] digest of {} line(s) queued", r.lines);
        }
    });
}

#[cfg(test)]
#[path = "operator_feed_tests.rs"]
mod tests;
