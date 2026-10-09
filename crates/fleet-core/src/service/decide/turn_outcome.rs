//! J2 `turn_outcome`: what a turn came to when hooks said nothing (the
//! test map's card J2; Orbit Fleet redesign step 5.11). The Stop hook says
//! a turn ended, never HOW: a finished task and a prose question ("Should I
//! also update the docs?") both leave an idle pane, and the attention model
//! reads both as done. A Notification (a permission or a question dialog)
//! or a StopFailure does say what happened; when none came, this adapter
//! asks the decision model one Choice over the end of the screen —
//! `finished`, `asked`, `stuck`, `working` or `unsure` — and nothing more:
//!
//! * **Hooks always win.** Every hook write clears `sessions.turn_outcome`
//!   (`store/sessions.rs`), and an answer lands only while no hook has
//!   spoken since the turn's Stop ([`Store::set_jev_turn_outcome`]): a hook
//!   before the answer keeps it out, a hook after it takes it back.
//! * **Shadow / assist only.** In `shadow` the answer is only recorded, with
//!   the pane rules' reading ([`rule_outcome`]) as the baseline. In
//!   `assist` a usable answer sets `sessions.turn_outcome`, and with it the
//!   Inbox state: `asked` reads as waiting, `stuck` as stuck
//!   (`service::attention`). Nothing is typed, killed or sent to the
//!   session; a person still reads the screen.
//! * **What is sent.** The ANSI-stripped visible pane tail — at most
//!   [`TAIL_LINES`] lines and [`TAIL_CHARS`] characters, the REPL's chrome
//!   (rules, the input line, the footer) left out and every fenced code
//!   block replaced by `[code: <lang>, N lines]` (D42) — through the
//!   envelope's redaction. Claude's reply text: the org must ALSO consent to
//!   reply text (D48, the org's `decide.jev.reply_consent` row, or
//!   `decide.jev.unassigned_reply` for a session with no org), on top of
//!   D31's consent.
//! * **What is recorded.** One run per turn: subject `session_turn`
//!   `<session id>:<turn_seq>`; a turn already decided is never asked
//!   again. The options are the five words of `sessions.turn_outcome`.
//! * **J8, the drift alarm.** When the pane rules read nothing on the
//!   screen ([`rule_outcome`] is `None`: Claude Code's UI may have changed),
//!   the run's baseline is `none` and the session's timeline gets one
//!   `pane_unreadable` event ([`PANE_UNREADABLE`]), which the UI can show.
//!   Local only: nothing about it is sent.
//! * **Follow-up.** A hook that speaks later about the same turn marks the
//!   assist answer `confirmed` (it said the same) or `corrected` (to what
//!   it said: a dialog is `asked`, a stuck screen `stuck`); a person's
//!   prompt within [`FOLLOWUP_SECS`] of an `asked` answer confirms it. A
//!   shadow answer nobody saw is never marked (D34, D37).

use super::start_project::decided;
use super::{decide, gate_at, DecideCtx, DecideRequest, Feature, JevRequest, Mode, Question};
use crate::ipc_error::{lock, IpcError};
use crate::service::pane_intel::{self, ClaudeStatus, StuckKind};
use crate::ssh::SshClient;
use crate::store::{DecisionRunRow, SessionRow, Store, DECISION_NO_BASELINE, TURN_OUTCOMES};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "turn_outcome.v1";
/// What a run is about: one turn of one session.
pub const SUBJECT_KIND: &str = "session_turn";
/// Below this confidence an answer is recorded, not applied.
pub const MIN_CONFIDENCE: f64 = 0.5;
/// The option that says nothing.
pub const UNSURE: &str = "unsure";
/// Lines of the screen sent, at most (the newest).
pub const TAIL_LINES: usize = 40;
/// Characters of the screen sent, at most (the newest).
pub const TAIL_CHARS: usize = 2_000;
/// A person's prompt this soon after an `asked` answer confirms it.
pub const FOLLOWUP_SECS: i64 = 30 * 60;
/// The timeline event J8 records when the pane rules read nothing.
pub const PANE_UNREADABLE: &str = "pane_unreadable";

/// The instruction, read literally: the exact condition and what to use.
pub const INSTRUCTIONS: &str = "state.screen is the end of a terminal screen of Claude Code, an \
     AI coding assistant, captured right after it ended a turn and went quiet: its last reply \
     and what is below it. Code is replaced by [code: ...] placeholders. Decide what the turn \
     came to, using only the screen. Choose asked when the reply ends by asking the person a \
     question or for a decision before it can go on; finished when it reports the work done or \
     answered and asks nothing; stuck when it shows an error, a crash or a screen it cannot \
     leave by itself; working when it is still doing the work (a spinner, a running tool). \
     Choose unsure when the screen does not show which.";

/// PURE: the question: one Choice over the five outcomes.
pub fn question() -> Question {
    let criteria: [(&str, &str); 5] = [
        (
            "finished",
            "The reply reports the work done or the question answered, and asks the person \
             nothing.",
        ),
        (
            "asked",
            "The reply asks the person a question or for a decision, and waits for the answer.",
        ),
        (
            "stuck",
            "The screen shows an error, a crash or a prompt Claude cannot get past by itself.",
        ),
        (
            "working",
            "Claude is still doing the work: a spinner or a running tool is on the screen.",
        ),
        (
            UNSURE,
            "The screen does not show which of these the turn came to.",
        ),
    ];
    Question::Choice {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria: criteria
            .into_iter()
            .map(|(k, v)| (k.to_string(), Some(Value::String(v.into()))))
            .collect(),
    }
}

/// PURE: a line of the REPL's chrome, never part of the reply: a rule or
/// box border, the input line, the mode / shortcut footer.
fn is_chrome(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return true;
    }
    if t.chars().all(|c| {
        matches!(
            c,
            '─' | '━' | '│' | '╭' | '╮' | '╰' | '╯' | '┃' | '═' | '-' | ' '
        )
    }) {
        return true;
    }
    let lower = t.to_lowercase();
    t.starts_with('❯')
        || t.starts_with('⏵')
        || t.starts_with('⏸')
        || lower.contains("? for shortcuts")
        || lower.contains("% used")
        || lower.contains("bypass permissions")
        || lower.contains("shift+tab to cycle")
        || lower.contains("esc to interrupt")
        || lower.contains("auto-accept edits")
}

/// PURE: what is sent of a captured pane tail: ANSI-stripped, fenced code
/// replaced by placeholders, the chrome and blank runs left out, the
/// newest [`TAIL_LINES`] lines and [`TAIL_CHARS`] characters. Redaction
/// (URLs, emails, tokens) is the envelope's, on top.
pub fn prepare_tail(raw: &str) -> String {
    let stripped = pane_intel::strip_ansi(raw);
    let coded = super::code_placeholder(&stripped);
    let lines: Vec<&str> = coded
        .lines()
        .map(str::trim_end)
        .filter(|l| !is_chrome(l))
        .collect();
    let start = lines.len().saturating_sub(TAIL_LINES);
    let text = lines[start..].join("\n");
    let n = text.chars().count();
    if n <= TAIL_CHARS {
        text
    } else {
        text.chars().skip(n - TAIL_CHARS).collect()
    }
}

/// PURE: the outcome the pane rules read (`service::pane_intel::analyze`),
/// the baseline: a stuck state is `stuck`, a dialog `asked`, a spinner
/// `working`, the idle input chrome `finished`. `None` when the rules read
/// nothing on a screen — J8's drift signal.
pub fn rule_outcome(raw: &str) -> Option<&'static str> {
    let intel = pane_intel::analyze(raw);
    if intel.stuck.is_some() {
        return Some("stuck");
    }
    match intel.derived_status? {
        ClaudeStatus::Blocked => Some("asked"),
        ClaudeStatus::Working => Some("working"),
        ClaudeStatus::Idle | ClaudeStatus::Completed | ClaudeStatus::Stopped => Some("finished"),
        ClaudeStatus::Failed => Some("stuck"),
    }
}

/// PURE: the test map's "ends with ?" baseline: `asked` when the last line
/// of the reply (the chrome left out) ends with a question mark, else
/// `finished`; `None` on an empty screen.
pub fn ends_with_question(raw: &str) -> Option<&'static str> {
    let stripped = pane_intel::strip_ansi(raw);
    let last = stripped.lines().rev().find(|l| !is_chrome(l))?;
    Some(if last.trim_end().ends_with('?') {
        "asked"
    } else {
        "finished"
    })
}

/// PURE: the run's subject id: one session's one turn.
pub fn subject_id(session_id: i64, turn_seq: i64) -> String {
    format!("{session_id}:{turn_seq}")
}

/// PURE: the request for a pane tail (already [`prepare_tail`]d).
pub fn question_for(screen: &str) -> JevRequest {
    JevRequest {
        state: json!({ "screen": screen }),
        question: question(),
    }
}

/// What the adapter asks about: one session's one ended turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnInput {
    pub session_id: i64,
    pub turn_seq: i64,
    /// The session's org (both its consents are checked).
    pub org_id: Option<i64>,
    /// The captured pane tail, raw (ANSI and all).
    pub tail: String,
}

/// What [`ask`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum Asked {
    /// Not asked: the gate refused, the turn was decided already, or the
    /// screen was empty. Nothing recorded.
    Skipped,
    /// Asked and recorded; nothing applied (shadow, a fallback, `unsure`,
    /// under the floor, or a hook spoke first).
    Recorded,
    /// Assist: the answer is the row's `turn_outcome` now.
    Applied {
        outcome: String,
        run_id: Option<i64>,
    },
}

/// PURE: the outcome a hook's effect says (a Notification's mapped status
/// and stuck kind): a stuck screen is `stuck`, a dialog `asked`, a resume
/// `working`. `None` for an effect that says nothing about the turn.
pub fn hook_word(status: ClaudeStatus, stuck: Option<Option<StuckKind>>) -> Option<&'static str> {
    match (status, stuck) {
        (_, Some(Some(_))) => Some("stuck"),
        (ClaudeStatus::Blocked, _) => Some("asked"),
        (ClaudeStatus::Working, _) => Some("working"),
        _ => None,
    }
}

/// The input for `session_id`'s ended turn, when it is a turn J2 reads: a
/// pane-backed Claude session, `idle` after its Stop, no stuck state, no
/// form and no answer yet. (That no hook spoke since the Stop is checked
/// where the answer is written, [`Store::set_jev_turn_outcome`].)
pub fn input_for(s: &Store, session_id: i64) -> Result<Option<TurnInput>, IpcError> {
    let Some(row) = s.get_session_by_id(session_id)? else {
        return Ok(None);
    };
    Ok(eligible(&row).then(|| TurnInput {
        session_id: row.id,
        turn_seq: row.turn_seq,
        org_id: row.org_id,
        tail: String::new(),
    }))
}

fn eligible(row: &SessionRow) -> bool {
    row.agent == crate::store::AGENT_CLAUDE
        && row.kind != "shell"
        && !crate::store::has_no_pane(&row.kind)
        && row.status == "running"
        && row.claude_status.as_deref() == Some("idle")
        && row.stuck_kind.is_none()
        && row.pending_form.is_none()
        && row.turn_outcome.is_none()
        && row.last_stop_at.is_some()
}

/// Ask the decision model what `input`'s turn came to, and in `assist`
/// apply a usable answer. Every failure is a recorded fallback or nothing
/// at all, never an error: the session never depends on this. Holds the
/// store lock only briefly, never across the call.
pub async fn ask(ctx: &DecideCtx, input: &TurnInput) -> Asked {
    let now = ctx.now();
    let subject = subject_id(input.session_id, input.turn_seq);
    let screen = prepare_tail(&input.tail);
    if screen.trim().is_empty() {
        return Asked::Skipped;
    }
    let rule = rule_outcome(&input.tail);
    {
        let Ok(s) = lock(&ctx.store) else {
            return Asked::Skipped;
        };
        if gate_at(&s, Feature::TurnOutcome, input.org_id, now).is_err() {
            return Asked::Skipped;
        }
        // One decision per turn.
        match s.decision_runs_for_subjects(
            Feature::TurnOutcome.as_str(),
            SUBJECT_KIND,
            &subject,
            None,
        ) {
            Ok(runs) if runs.iter().any(|r| r.subject_id == subject && decided(r)) => {
                return Asked::Skipped;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("[decide] turn_outcome not asked: {}", e.message);
                return Asked::Skipped;
            }
        }
        // J8: the rules read nothing on this screen. Local: a timeline
        // entry, once per turn (a decided turn is never back here).
        if rule.is_none() {
            tracing::warn!(
                session_id = input.session_id,
                "[decide] J8: no pane rule read the screen at a turn's end"
            );
            if let Err(e) = s.insert_session_event(
                input.session_id,
                PANE_UNREADABLE,
                Some("no pane rule read the screen at the turn's end (Claude Code's UI may have changed)"),
            ) {
                tracing::debug!(error = %e.message, "[decide] J8 event not recorded");
            }
        }
    }
    let out = decide(
        ctx,
        DecideRequest {
            feature: Feature::TurnOutcome,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: subject.clone(),
            org_id: input.org_id,
            request: question_for(&screen),
            baseline: Some(rule.unwrap_or(DECISION_NO_BASELINE).to_string()),
            question_version: QUESTION_VERSION.into(),
            min_confidence: Some(MIN_CONFIDENCE),
        },
    )
    .await;
    let Some(answer) = out.proposal() else {
        return Asked::Recorded;
    };
    if answer.value == UNSURE || !TURN_OUTCOMES.contains(&answer.value.as_str()) {
        return Asked::Recorded;
    }
    let Ok(s) = lock(&ctx.store) else {
        return Asked::Recorded;
    };
    match s.set_jev_turn_outcome(input.session_id, input.turn_seq, &answer.value) {
        Ok(true) => Asked::Applied {
            outcome: answer.value.clone(),
            run_id: out.run_id,
        },
        Ok(false) => {
            // A hook spoke first (or the turn moved on): it wins. What it
            // said is the follow-up.
            if let (Some(id), Ok(Some(row))) = (out.run_id, s.get_session_by_id(input.session_id)) {
                let said = match row.claude_status.as_deref() {
                    _ if row.turn_seq != input.turn_seq => None,
                    _ if row.stuck_kind.is_some() => Some("stuck"),
                    Some("blocked") => Some("asked"),
                    _ => None,
                };
                if let Some(said) = said {
                    let _ = mark(&s, id, &answer.value, said, now);
                }
            }
            Asked::Recorded
        }
        Err(e) => {
            tracing::warn!("[decide] turn_outcome not applied: {}", e.message);
            Asked::Recorded
        }
    }
}

/// Mark run `id` (answered `answer`) `confirmed` when `said` agrees, else
/// `corrected` to `said`.
fn mark(s: &Store, id: i64, answer: &str, said: &str, now: i64) -> Result<bool, IpcError> {
    if answer == said {
        s.set_decision_followup(id, "confirmed", None, now)
    } else {
        s.set_decision_followup(id, "corrected", Some(said), now)
    }
}

/// The latest assist answer about `session_id`'s turn `turn_seq` that
/// nobody decided yet.
fn open_answer(
    s: &Store,
    session_id: i64,
    turn_seq: i64,
) -> Result<Option<DecisionRunRow>, IpcError> {
    let subject = subject_id(session_id, turn_seq);
    let runs =
        s.decision_runs_for_subjects(Feature::TurnOutcome.as_str(), SUBJECT_KIND, &subject, None)?;
    Ok(runs
        .into_iter()
        .find(|r| {
            r.subject_id == subject
                && r.mode == Mode::Assist.as_str()
                && r.fallback.is_none()
                && r.answer.as_deref().is_some_and(|a| a != UNSURE)
        })
        .filter(|r| r.followup.is_none()))
}

/// A hook spoke about `session_id`'s current turn `turn_seq` and `said`
/// (see [`hook_word`]): mark the turn's open assist answer `confirmed` or
/// `corrected`. Returns whether a run was marked. Never a shadow answer.
pub fn record_hook(
    s: &Store,
    session_id: i64,
    turn_seq: i64,
    said: &str,
    now: i64,
) -> Result<bool, IpcError> {
    let Some(r) = open_answer(s, session_id, turn_seq)? else {
        return Ok(false);
    };
    mark(s, r.id, r.answer.as_deref().unwrap_or_default(), said, now)
}

/// A PERSON prompted `session_id` while it was on turn `turn_seq`: an
/// `asked` answer of that turn decided less than [`FOLLOWUP_SECS`] ago is
/// `confirmed` (they answered it). Anything else says nothing. Returns
/// whether a run was marked.
pub fn record_prompt(
    s: &Store,
    session_id: i64,
    turn_seq: i64,
    now: i64,
) -> Result<bool, IpcError> {
    let Some(r) = open_answer(s, session_id, turn_seq)? else {
        return Ok(false);
    };
    if r.answer.as_deref() == Some("asked") && now - r.at <= FOLLOWUP_SECS {
        s.set_decision_followup(r.id, "confirmed", None, now)
    } else {
        Ok(false)
    }
}

/// The mode J2 would ask `session_id`'s ended turn in, under one short
/// lock, with its input (tail still empty); `None` when it is not a turn J2
/// reads or the gate refuses — then the Stop pays nothing more.
pub fn plan_for(
    store: &Arc<Mutex<Store>>,
    session_id: i64,
) -> Option<(Mode, TurnInput, SessionRow)> {
    let s = lock(store).ok()?;
    let row = s.get_session_by_id(session_id).ok()??;
    if !eligible(&row) {
        return None;
    }
    let mode = gate_at(
        &s,
        Feature::TurnOutcome,
        row.org_id,
        crate::store::now_unix(),
    )
    .ok()?;
    let input = TurnInput {
        session_id: row.id,
        turn_seq: row.turn_seq,
        org_id: row.org_id,
        tail: String::new(),
    };
    Some((mode, input, row))
}

/// After a Stop: when J2 may read the turn at all, capture the session's
/// visible pane and [`ask`] — off the hook's path (spawned). With the
/// defaults (the feature `off`) it costs one short lock and spawns
/// nothing.
pub fn spawn_after_stop(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>, session_id: i64) {
    let Some((_, mut input, row)) = plan_for(store, session_id) else {
        return;
    };
    let ctx = DecideCtx::jev(Arc::clone(store));
    let ssh = Arc::clone(ssh);
    let _ = crate::rt::try_spawn(async move {
        let tmux: Box<dyn crate::tmux::TmuxExec> = if row.host_alias == "local" {
            Box::new(crate::tmux::LocalTmux)
        } else {
            Box::new(crate::tmux::RemoteTmux {
                client: ssh,
                host: row.host_alias.clone(),
            })
        };
        match tmux.capture_pane(&row.tmux_name).await {
            Ok(tail) => {
                input.tail = tail;
                ask(&ctx, &input).await;
            }
            Err(e) => tracing::debug!(
                session_id,
                error = %e.message,
                "[decide] turn_outcome: pane not captured"
            ),
        }
    });
}
