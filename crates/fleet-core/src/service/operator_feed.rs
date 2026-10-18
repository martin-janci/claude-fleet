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
//! * Autofix findings (`service::autofix`) that wait on a person, and the
//!   safe fixes fleet applied by itself ([`autofix_conditions`]).
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
//! and [`MAX_DIGESTS_PER_HOUR`] an hour; while one is still queued (the
//! operator busy), nothing new is queued, so changes pile up into the next
//! one instead of a stack of notes. Events older than
//! [`EVENT_MAX_AGE_SECS`] when finally sent are dropped: an operator that
//! was down for a day does not need yesterday read to it.
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
    crate::service::account_switch::EVENT_SWITCHED,
    crate::service::account_switch::EVENT_SWITCH_FAILED,
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
    /// Its login was switched (`true`) or the switch failed (control
    /// review F9, `account_switch`).
    AccountSwitch(bool),
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
            What::AccountSwitch(_) => "account_switch",
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
            What::AccountSwitch(true) => {
                "was switched to a login with headroom after its account ran out".into()
            }
            What::AccountSwitch(false) => {
                "could not be switched to another login; it waits at its limit".into()
            }
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
        k if k == crate::service::account_switch::EVENT_SWITCHED => What::AccountSwitch(true),
        k if k == crate::service::account_switch::EVENT_SWITCH_FAILED => What::AccountSwitch(false),
        _ => return None,
    })
}

/// One line of a digest, with the key it is deduplicated on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedLine {
    pub key: String,
    pub text: String,
}

/// PURE: the session lines of a batch of events, one per session and kind
/// of change, in the order each first happened, worded by its latest event.
/// Events older than `now - EVENT_MAX_AGE_SECS` are left out.
pub fn session_lines(events: &[FeedEventRow], now: i64) -> Vec<FeedLine> {
    let mut order: Vec<String> = Vec::new();
    let mut latest: BTreeMap<String, String> = BTreeMap::new();
    for e in events.iter().filter(|e| e.at >= now - EVENT_MAX_AGE_SECS) {
        let Some(what) = classify(&e.kind, e.detail.as_deref()) else {
            continue;
        };
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

/// PURE: the autofix lines. A live finding whose fix needs a person (or
/// that has no fix fleet can run, or whose automatic fix stopped working)
/// is a problem; a safe fix fleet applied by itself and that cleared within
/// [`EVENT_MAX_AGE_SECS`] is news. A safe fix still to run, or one a loop
/// owns (`auto: false`), says nothing yet. Titles are fleet's own words
/// (the detectors'), never a session's.
pub fn autofix_conditions(
    live: &[crate::service::autofix::api::FindingView],
    fixed: &[crate::service::autofix::api::FindingView],
    now: i64,
) -> Vec<Condition> {
    use crate::service::autofix::Safety;
    let mut out = Vec::new();
    for f in live {
        if !matches!(f.state.as_str(), "open" | "apply_failed") {
            continue;
        }
        let title = f.title.as_deref().unwrap_or(&f.kind);
        let subject = crate::service::autofix::api::subject_words(&f.subject);
        let tail = match &f.fix {
            Some(x) if x.safety == Safety::NeedsApproval => "fix needs your approval",
            Some(x) if x.auto && f.state == "apply_failed" => {
                "the automatic fix did not work; it needs you"
            }
            Some(_) => continue,
            None => "fleet has no fix for it; it needs you",
        };
        out.push(Condition {
            key: format!("af{}", f.id),
            text: format!(
                "problem: {title} ({subject}) — {tail} (autofix finding #{})",
                f.id
            ),
        });
    }
    for f in fixed {
        let automatic = f
            .fix
            .as_ref()
            .is_some_and(|x| x.safety == Safety::Safe && x.auto);
        if automatic && f.last_seen_at >= now - EVENT_MAX_AGE_SECS {
            out.push(Condition {
                key: format!("af{}:fixed", f.id),
                text: format!(
                    "fixed automatically: {}",
                    f.title.as_deref().unwrap_or(&f.kind)
                ),
            });
        }
    }
    out
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
    if let Ok(found) = crate::service::autofix::api::list(s, None, false, now) {
        out.extend(autofix_conditions(&found.findings, &found.fixed, now));
    }
    // Accounts at their limit and signed-out logins (control review F9).
    out.extend(crate::service::account_feed::conditions(s, &hosts, now));
    for id in s.handed_off_mission_ids().unwrap_or_default() {
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
    // F15: what Control handed out that went wrong on its own.
    out.extend(crate::service::control_sweep::conditions(
        s,
        operator_row(s).map(|r| r.id),
        now,
    ));
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
        let events = s
            .operator_feed_events(cursor, Some(op.id), &KINDS, READ_LIMIT)
            .unwrap_or_default();
        let mut lines: Vec<String> = session_lines(&events, now)
            .into_iter()
            .map(|l| l.text)
            .collect();
        let now_raised = conditions(&s, now);
        lines.extend(
            now_raised
                .iter()
                .filter(|c| !state.raised.contains(&c.key))
                .map(|c| c.text.clone()),
        );
        // A limit that reset, a login signed in again (control review F9).
        lines.extend(crate::service::account_feed::cleared_lines(
            &s,
            &state.raised,
            &now_raised,
        ));
        state.cursor = Some(events.last().map_or(cursor, |e| e.id));
        state.raised = now_raised.into_iter().map(|c| c.key).collect();
        state.sent.retain(|&t| now - t < 3600);
        let body = build_digest(&lines).map(|d| {
            crate::service::autonomy::with_header(&d, crate::service::autonomy::position(&s))
        });
        if body.is_none() {
            // Nothing to say: move past what was read, keep what is raised.
            if state != FeedState::load(&s) {
                state.save(&s);
            }
            return FeedReport::default();
        }
        (op.id, body, state)
    };
    let body = body.expect("checked above");
    // The digest's `- ` lines: not its mark nor the autonomy line (F3).
    let lines = body.lines().filter(|l| l.starts_with("- ")).count();
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
