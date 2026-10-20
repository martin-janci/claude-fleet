//! Control goes on by itself, where the fleet runs (review 2026-10-17, F17).
//!
//! When Control's reply ends on a `choices` card (`docs/chat-blocks.md`)
//! with one option marked `recommended` and nobody answers for
//! `control.auto_continue_minutes`, fleet sends that option for the person.
//! This used to be a timer in each open desktop window
//! (`src/lib/AutoContinueBar.svelte`), so it stopped with the window, two
//! windows could both send, and the phone never saw it. Now:
//!
//! * the operator's Stop hook ([`on_stop`]) reads the reply
//!   (`last_assistant_message`) and records the pick in ONE setting,
//!   [`STATE_KEY`]: the newest turn's card replaces the last one;
//! * the `control_continue` loop ([`maybe_run`]) sends it when due, once:
//!   the state moves `waiting -> sent` under the store lock before the
//!   prompt is queued, so a second pass (or a second process reading the
//!   same store) finds nothing to send;
//! * the desktop and the phone read it (`control_auto_continue` action
//!   `status`) to draw the countdown, and settle it with `cancel` (Cancel,
//!   or the person typing) or `take` (Send now: the client sends the
//!   prompt as the person's own words).
//!
//! What keeps it from acting where a person must:
//! * `control.auto_continue` off, or the autonomy dial at "Ask first"
//!   (or Custom, which acts as it: `autonomy::Position::is_ask_first`): never;
//! * an option or question naming a push, a delete or another step hard to
//!   undo (`decide::quick_answer::risky`): never recorded;
//! * a newer turn (the person said something else), a busy or blocked
//!   agent, or a prompt already queued for it: not now, and a newer turn
//!   replaces the card;
//! * a card older than [`STALE_SECS`]: left alone;
//! * `automation.paused`: the loop's gate.
//!
//! The prompt opens with `[fleet]` (`prompt_origin::FLEET_PROMPT_MARKS`): it
//! is fleet's send on the person's standing setting, not their typing.

use crate::ipc_error::{codes, IpcError};
use crate::pages::chat_blocks;
use crate::service::autonomy::{self, Position};
use crate::service::decide::quick_answer::{risky, risky_question};
use crate::service::settings;
use crate::ssh::SshClient;
use crate::store::{SessionRow, Store};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The setting holding [`Pending`] as JSON. Internal: no `SPECS` row.
pub const STATE_KEY: &str = "control.auto_continue_state";
/// A card from a reply older than this is not answered for the person.
pub const STALE_SECS: i64 = 24 * 3600;
/// The longest prompt or label kept; a block caps them far lower.
const TEXT_MAX: usize = 4000;

/// Where a recorded pick stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickState {
    /// Counting down.
    Waiting,
    /// Fleet sent it.
    Sent,
    /// A person sent it themselves (Send now).
    Taken,
    /// A person stopped it (Cancel, typing).
    Cancelled,
    /// A newer turn, or too old: nobody needs it any more.
    Superseded,
    /// Queuing it failed; it is not retried.
    Failed,
}

/// The recommended option of Control's latest turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub session_id: i64,
    /// The operator's `turn_seq` after the turn the card ended: the card's
    /// identity.
    pub turn_seq: i64,
    pub label: String,
    pub prompt: String,
    /// When the turn ended (unix seconds); the clock counts from here.
    pub ended_at: i64,
    pub state: PickState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<i64>,
}

/// What a client draws: the pick, when it is due and whether it may run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutoContinueView {
    /// `control.auto_continue` is on and the dial is not at "Ask first".
    pub enabled: bool,
    pub minutes: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Pending>,
    /// `ended_at + minutes`, for the countdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_at: Option<i64>,
}

/// The option picked, as [`recommended_pick`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub label: String,
    pub prompt: String,
}

// ── Reading the reply ──────────────────────────────────────────────────────

/// PURE: an opening code fence (`^ {0,3}(```+|~~~+)info`): its marker and
/// lowercased language, as `markdown.ts::fenceOpen` reads it.
fn fence_open(line: &str) -> Option<(String, String)> {
    let lead = line.len() - line.trim_start_matches(' ').len();
    if lead > 3 {
        return None;
    }
    let rest = &line[lead..];
    let ch = rest.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = rest.chars().take_while(|c| *c == ch).count();
    if n < 3 {
        return None;
    }
    let info = &rest[n..];
    if ch == '`' && info.contains('`') {
        return None;
    }
    let lang = info.split_whitespace().next().unwrap_or("").to_lowercase();
    Some((ch.to_string().repeat(n), lang))
}

fn closes(line: &str, marker: &str) -> bool {
    let t = line.trim();
    let c = marker.chars().next().unwrap_or('`');
    t.len() >= marker.len() && t.chars().all(|x| x == c)
}

/// PURE: every `fleet.ui/1` block in `text`, as `rich_blocks.ts::splitRich`
/// finds them: a closed ```` ```fleet-ui ```` fence (or one tagged with the
/// spec's own name, `fleet.ui`), or a ```` ```json ```` or untagged one whose
/// object says `"spec": "fleet.ui/1"`, that passes the check.
pub fn ui_blocks(text: &str) -> Vec<Value> {
    blocks_in(text, true)
}

/// PURE: [`ui_blocks`], and with `untagged` false only the fences Control
/// tags as a card (```` ```fleet-ui ````, its aliases, or ```` ```json ````):
/// an untagged fence is how Control quotes pane output, logs or an issue
/// verbatim, so a card inside one is drawn but never sent by itself.
fn blocks_in(text: &str, untagged: bool) -> Vec<Value> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some((marker, lang)) = fence_open(lines[i]) else {
            i += 1;
            continue;
        };
        let Some(close) = (i + 1..lines.len()).find(|&k| closes(lines[k], &marker)) else {
            break;
        };
        let raw = lines[i + 1..close].join("\n");
        let raw = raw.trim();
        let tagged = matches!(
            lang.as_str(),
            "fleet-ui" | "fleet.ui" | "fleet_ui" | "fleetui"
        );
        let json_card = (matches!(lang.as_str(), "json" | "jsonc")
            || (untagged && lang.is_empty()))
            && raw.starts_with('{')
            && raw.contains(chat_blocks::UI_SPEC);
        if (tagged || json_card) && chat_blocks::check_text(raw).is_ok() {
            if let Ok(v) = serde_json::from_str::<Value>(raw) {
                if tagged || v.get("spec").and_then(Value::as_str) == Some(chat_blocks::UI_SPEC) {
                    out.push(v);
                }
            }
        }
        i = close + 1;
    }
    out
}

/// PURE: the recommended option of the LAST `choices` card in `text`, or
/// `None`, also when the option or the card's question or title names a
/// risky step. A card in an untagged fence does not count: that is quoted
/// text (a pane, a log, an issue body), not Control's own choice.
pub fn recommended_pick(text: &str) -> Option<Pick> {
    let last = blocks_in(text, false)
        .into_iter()
        .rfind(|v| v.get("kind").and_then(Value::as_str) == Some("choices"))?;
    let s = |k: &str| last.get(k).and_then(Value::as_str).unwrap_or("");
    if risky_question(s("question")) || risky_question(s("title")) {
        return None;
    }
    let opt = last
        .get("options")?
        .as_array()?
        .iter()
        .find(|o| o.get("recommended").and_then(Value::as_bool) == Some(true))?;
    let label = opt.get("label")?.as_str()?.to_string();
    let prompt = opt.get("prompt")?.as_str()?.to_string();
    if risky(&label) || risky(&prompt) || prompt.trim().is_empty() {
        return None;
    }
    Some(Pick {
        label: label.chars().take(TEXT_MAX).collect(),
        prompt: prompt.chars().take(TEXT_MAX).collect(),
    })
}

// ── State ──────────────────────────────────────────────────────────────────

pub fn load(s: &Store) -> Option<Pending> {
    s.get_setting(STATE_KEY)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
}

fn save(s: &Store, p: Option<&Pending>) -> Result<(), IpcError> {
    match p {
        Some(p) => s.set_setting(STATE_KEY, &serde_json::to_string(p).unwrap_or_default())?,
        None => s.set_setting(STATE_KEY, "")?,
    }
    Ok(())
}

/// PURE: may the dial and the settings let it run: never at Ask first,
/// nor at Custom, which acts as it.
pub fn enabled_by(auto_continue: bool, position: Position) -> bool {
    auto_continue && !position.is_ask_first()
}

fn enabled(s: &Store) -> bool {
    enabled_by(
        settings::get_bool(s, settings::CONTROL_AUTO_CONTINUE),
        autonomy::position(s),
    )
}

fn minutes(s: &Store) -> i64 {
    (settings::get_secs(s, settings::CONTROL_AUTO_CONTINUE_MINUTES) as i64).max(1)
}

/// The view clients draw.
pub fn view(s: &Store) -> AutoContinueView {
    let minutes = minutes(s);
    let pending = load(s);
    AutoContinueView {
        enabled: enabled(s),
        minutes,
        due_at: pending.as_ref().map(|p| p.ended_at + minutes * 60),
        pending,
    }
}

fn is_operator(s: &Store, row: &SessionRow) -> bool {
    crate::service::operator::operator_ref(s)
        .is_some_and(|r| r.host_alias == row.host_alias && r.tmux_name == row.tmux_name)
}

/// The Stop hook of any session: for Control's, record the newest turn's
/// recommended option (or forget the last one when the turn has none).
/// Runs inside the hook's transaction; it only reads rows and writes one
/// setting.
pub fn on_stop(s: &Store, session_id: i64, reply: Option<&str>, now: i64) -> Result<(), IpcError> {
    let Some(row) = s.get_session_by_id(session_id)? else {
        return Ok(());
    };
    if !is_operator(s, &row) {
        return Ok(());
    }
    let next = reply.and_then(recommended_pick).map(|p| Pending {
        session_id,
        turn_seq: row.turn_seq,
        label: p.label,
        prompt: p.prompt,
        ended_at: now,
        state: PickState::Waiting,
        settled_at: None,
    });
    if next.is_none() && load(s).is_none() {
        return Ok(());
    }
    save(s, next.as_ref())
}

// ── Deciding ───────────────────────────────────────────────────────────────

/// What a pass does with the recorded pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Nothing,
    /// Not yet, in words.
    Wait(&'static str),
    /// Settle it without sending (a newer turn, too old).
    Drop,
    Send,
}

/// What the pass knows about the operator's session now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperatorNow {
    pub id: i64,
    pub turn_seq: i64,
    /// Running, in a pane fleet can type into.
    pub live: bool,
    /// Mid-turn, blocked on a dialog, stuck, or a question pending.
    pub busy: bool,
    /// A prompt is queued for it and not yet delivered.
    pub queued: bool,
}

/// PURE: the step for `p` at `now`.
pub fn decide(
    p: Option<&Pending>,
    op: Option<OperatorNow>,
    enabled: bool,
    minutes: i64,
    now: i64,
) -> Step {
    let Some(p) = p.filter(|p| p.state == PickState::Waiting) else {
        return Step::Nothing;
    };
    if !enabled {
        return Step::Wait("off");
    }
    let Some(op) = op else {
        return Step::Wait("no_operator");
    };
    if op.id != p.session_id || op.turn_seq != p.turn_seq || now - p.ended_at > STALE_SECS {
        return Step::Drop;
    }
    if now < p.ended_at + minutes.max(1) * 60 {
        return Step::Wait("not_due");
    }
    if !op.live {
        return Step::Wait("operator_not_running");
    }
    if op.busy || op.queued {
        return Step::Wait("operator_busy");
    }
    Step::Send
}

fn operator_now(s: &Store) -> Option<OperatorNow> {
    let r = crate::service::operator::operator_ref(s)?;
    let row = s
        .find_sessions_by_tmux_name(&r.tmux_name, Some(&r.host_alias))
        .ok()?
        .into_iter()
        .next()?;
    let queued = s
        .list_deferred_prompts(Some(row.id))
        .unwrap_or_default()
        .iter()
        .any(|p| {
            p.delivered_at.is_none()
                && p.failed_at.is_none()
                && p.cancelled_at.is_none()
                && p.skipped_at.is_none()
        });
    Some(OperatorNow {
        id: row.id,
        turn_seq: row.turn_seq,
        live: row.status == "running" && !crate::store::has_no_pane(&row.kind),
        busy: matches!(
            row.claude_status.as_deref(),
            Some("working") | Some("blocked")
        ) || row.pending_input.is_some()
            || row.stuck_kind.is_some(),
        queued,
    })
}

/// PURE: the prompt fleet types.
pub fn prompt_for(p: &Pending, minutes: i64) -> String {
    format!(
        "[fleet] Nobody answered for {minutes} min, so fleet goes on with the option you \
         recommended, \"{}\", on the person's auto-continue setting:\n\n{}",
        p.label, p.prompt
    )
}

/// Settle the waiting pick of `turn_seq` (any when `None`) as `to`. The
/// pick as it was, or `None` when nothing was waiting: the compare-and-set
/// that makes a send happen once.
fn settle(
    s: &Store,
    turn_seq: Option<i64>,
    to: PickState,
    now: i64,
) -> Result<Option<Pending>, IpcError> {
    let Some(mut p) = load(s) else {
        return Ok(None);
    };
    if p.state != PickState::Waiting || turn_seq.is_some_and(|t| t != p.turn_seq) {
        return Ok(None);
    }
    let was = p.clone();
    p.state = to;
    p.settled_at = Some(now);
    save(s, Some(&p))?;
    Ok(Some(was))
}

/// Side effects of a pass, injected for tests.
#[async_trait::async_trait]
pub trait ContinueExec: Send + Sync {
    async fn queue(&self, session_id: i64, body: String) -> Result<(), IpcError>;
}

pub struct RealContinueExec {
    pub store: Arc<Mutex<Store>>,
    pub ssh: Arc<SshClient>,
}

#[async_trait::async_trait]
impl ContinueExec for RealContinueExec {
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

/// One pass. `true` when a prompt was queued.
pub async fn run_with(store: &Mutex<Store>, exec: &dyn ContinueExec, now: i64) -> bool {
    let (sid, seq, body) = {
        let Ok(s) = store.lock() else {
            return false;
        };
        if crate::service::loops::paused(&s) {
            return false;
        }
        let p = load(&s);
        let minutes = minutes(&s);
        match decide(p.as_ref(), operator_now(&s), enabled(&s), minutes, now) {
            Step::Nothing | Step::Wait(_) => return false,
            Step::Drop => {
                let _ = settle(&s, None, PickState::Superseded, now);
                return false;
            }
            Step::Send => {}
        }
        // Waiting -> Sent BEFORE the send: whatever happens next, this pick
        // is never sent again.
        let Ok(Some(p)) = settle(&s, p.map(|p| p.turn_seq), PickState::Sent, now) else {
            return false;
        };
        (p.session_id, p.turn_seq, prompt_for(&p, minutes))
    };
    match exec.queue(sid, body).await {
        Ok(()) => true,
        Err(e) => {
            tracing::debug!(error = %e.message, "[control-continue] not queued");
            if let Ok(s) = store.lock() {
                // Only the pick that was sent: a newer one saved while the
                // send was in flight (Control finished another turn) is not
                // this failure's.
                if let Some(mut p) = load(&s).filter(|p| {
                    p.state == PickState::Sent && p.turn_seq == seq && p.session_id == sid
                }) {
                    p.state = PickState::Failed;
                    let _ = save(&s, Some(&p));
                }
            }
            false
        }
    }
}

static RUNNING: AtomicBool = AtomicBool::new(false);

/// The tick's entry: gated (`control_continue`, stops while paused), cheap
/// when nothing waits, detached and single-flight.
pub fn maybe_run(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) {
    {
        let Ok(s) = store.lock() else {
            return;
        };
        if !crate::service::loops::gate_in("control_continue", &s, None) {
            return;
        }
        if load(&s).is_none_or(|p| p.state != PickState::Waiting) {
            crate::service::loops::report("control_continue", Ok::<_, String>(()), None);
            return;
        }
    }
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let store = Arc::clone(store);
    let ssh = Arc::clone(ssh);
    crate::rt::spawn(async move {
        let _flight = crate::rt::ClearOnDrop::of_static(&RUNNING);
        let exec = RealContinueExec {
            store: Arc::clone(&store),
            ssh,
        };
        if run_with(&store, &exec, crate::store::now_unix()).await {
            tracing::info!("[control-continue] recommended option queued for Control");
        }
        crate::service::loops::report("control_continue", Ok::<_, String>(()), None);
    });
}

// ── The clients' tool ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "ControlAutoContinueParams")]
pub struct AutoContinueArgs {
    /// status (the countdown), cancel (stop it: Cancel, or the person
    /// typing) or take (Send now: settles it and returns the prompt for the
    /// client to send as the person's own words).
    pub action: String,
    /// The card's `turn_seq` from `status`; cancel and take leave a newer
    /// card alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_seq: Option<i64>,
}

/// `control_auto_continue`. `take` and `cancel` answer the view after the
/// change; `take`'s `pending` carries the prompt to send, `null` when it was
/// already settled (sent by fleet, another window took it).
pub fn act(
    store: &Mutex<Store>,
    args: &AutoContinueArgs,
    now: i64,
) -> Result<AutoContinueView, IpcError> {
    let s = crate::ipc_error::lock(store)?;
    match args.action.as_str() {
        "status" => Ok(view(&s)),
        "cancel" => {
            settle(&s, args.turn_seq, PickState::Cancelled, now)?;
            Ok(view(&s))
        }
        "take" => {
            let took = settle(&s, args.turn_seq, PickState::Taken, now)?;
            let mut v = view(&s);
            v.pending = took;
            Ok(v)
        }
        other => Err(IpcError::new(
            codes::E_INVALID,
            format!("unknown action {other:?}: status, cancel or take"),
        )),
    }
}

#[cfg(test)]
#[path = "control_continue_tests.rs"]
mod tests;
