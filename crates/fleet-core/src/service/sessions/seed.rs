//! Seeding a freshly spawned session: type its first prompt once Claude's
//! REPL is really up, never into a dialog, and make sure it was submitted.
//!
//! Every create path that hands a new session a prompt goes through here: a
//! task/ticket start or a brief resume ([`spawn_seed`], in the background,
//! waiting out a trust dialog the person answers), a review or an asset
//! authoring session ([`seed_now`], before the call returns), and a
//! dispatched worker ([`wait_for_repl`], which then delivers the task
//! itself). Typing before the REPL reads input loses the prompt, and an
//! Enter typed into the trust dialog answers it — its default is "No, exit".

use super::prompt::{send_prompt_inner, Origin};
use crate::ipc_error::IpcError;
use crate::service::pane_intel::{ClaudeStatus, StuckKind};
use crate::ssh::SshClient;
use crate::store::{PromptAckState, SessionRow, Store};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What the pane says about typing into it now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptGate {
    /// The REPL is up and waiting: type.
    Ready,
    /// "Do you trust the files in this folder?": NEVER type (Enter would
    /// answer it).
    TrustDialog,
    /// Another dialog or stuck state: do not type.
    Blocked,
    /// Still starting.
    NotYet,
}

/// PURE: decide from a captured pane.
pub fn prompt_gate(pane: &str) -> PromptGate {
    let intel = crate::agent_adapter::claude().analyze_pane(pane);
    match intel.stuck {
        Some(StuckKind::TrustPrompt) => return PromptGate::TrustDialog,
        Some(_) => return PromptGate::Blocked,
        None => {}
    }
    if intel.waiting_for.is_some() {
        return PromptGate::Blocked;
    }
    if crate::service::tasks::pane_shows_repl(pane) {
        PromptGate::Ready
    } else {
        PromptGate::NotYet
    }
}

/// How long each phase of a seed may take.
#[derive(Debug, Clone, Copy)]
pub struct SeedTimings {
    /// For the REPL to come up.
    pub wait: Duration,
    /// How long a dialog the person has to answer (the trust prompt, a
    /// login menu) holds the prompt back; `None` gives up on the first
    /// sight of one, for a caller that cannot wait for a person.
    pub dialog_wait: Option<Duration>,
    /// For the hook to acknowledge the prompt (`prompt_submit_seq`).
    pub ack: Duration,
    /// Unacknowledged this long, the pane is looked at again
    /// ([`unacked_step`]).
    pub nudge: Duration,
    /// At most this many Enters / retypes after the first send.
    pub retries: u32,
}

/// A seed nobody waits for (it runs after the call returned). The ack
/// window is long because a slow SessionStart hook holds a submitted prompt
/// queued until it finishes; the dialog wait because the person may only
/// see the trust prompt once they open the session.
pub const BACKGROUND: SeedTimings = SeedTimings {
    wait: Duration::from_secs(90),
    dialog_wait: Some(Duration::from_secs(15 * 60)),
    ack: Duration::from_secs(60),
    nudge: Duration::from_secs(5),
    retries: 2,
};

/// A seed the call waits for.
pub const FOREGROUND: SeedTimings = SeedTimings {
    wait: Duration::from_secs(30),
    dialog_wait: None,
    ack: Duration::from_secs(8),
    nudge: Duration::from_secs(3),
    retries: 1,
};

const POLL: Duration = Duration::from_millis(1000);
/// Ready polls in a row before typing: a pane that shows the REPL's chrome
/// for one capture (a `cl --resume` that is about to fail over to
/// `--session-id`, a redraw) is not yet the REPL that will read the prompt.
const SETTLE: u32 = 2;

/// The timeline kinds a seed writes, or none (its outcome is only logged).
#[derive(Debug, Clone, Copy)]
pub struct SeedEvents {
    pub waiting: &'static str,
    pub started: &'static str,
    /// Written once, when the REPL has settled and the prompt is about to
    /// be typed; `None` writes nothing.
    pub ready: Option<&'static str>,
}

/// The work graph's start prompt (`work::resume`, `trackers::tickets`).
/// `repl_ready` and `handover_started` are two of the start's progress
/// steps (task → session P-5): the desktop's progress strip reads them, and
/// `handover_started` is the spec's `brief_sent` — the hook took the prompt
/// that delivers the brief.
pub const HANDOVER: SeedEvents = SeedEvents {
    waiting: "handover_waiting",
    started: "handover_started",
    ready: Some(REPL_READY),
};

/// The timeline kind for a start whose REPL is up (task → session P-5).
pub const REPL_READY: &str = "repl_ready";

/// How [`wait_for_repl`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplWait {
    /// Settled: type now.
    Ready,
    /// A dialog the person has to answer is up (`trust_prompt` | `dialog`),
    /// and the caller's timings do not wait for it.
    Dialog(&'static str),
    /// Not up within the wait (or a dialog was not answered in time).
    NotReady,
}

/// What a seed needs from the session: its pane, the two ways of typing
/// into it, the hook's acknowledgement and the timeline. A trait so the
/// loop is tested against a scripted pane.
#[async_trait::async_trait]
pub(crate) trait SeedPane: Send + Sync {
    /// The visible pane; `None` when it could not be captured.
    async fn capture(&self) -> Option<String>;
    /// Type the prompt and press Enter.
    async fn type_prompt(&self, prompt: &str) -> Result<(), IpcError>;
    /// Press Enter alone.
    async fn press_enter(&self) -> Result<(), IpcError>;
    /// `None` when the row cannot be read.
    fn ack_state(&self) -> Option<PromptAckState>;
    /// A `waiting` (`started == false`) or `started` outcome.
    fn event(&self, started: bool, detail: Option<&str>);
    /// The REPL has settled and the prompt is about to be typed.
    fn ready(&self) {}
}

/// The production [`SeedPane`]: the session's tmux pane and its row.
pub(super) struct LivePane<'a> {
    store: &'a Mutex<Store>,
    ssh: &'a Arc<SshClient>,
    session_id: i64,
    host: String,
    tmux_name: String,
    origin: Origin,
    events: Option<SeedEvents>,
    /// Handover rows the typed prompt itself carries: stamped delivered
    /// once the REPL is ready, so the UserPromptSubmit hook the prompt
    /// fires does not hand them to Claude a second time.
    carries: Vec<i64>,
    tmux: Box<dyn crate::tmux::TmuxExec>,
}

impl<'a> LivePane<'a> {
    pub(super) fn new(
        store: &'a Mutex<Store>,
        ssh: &'a Arc<SshClient>,
        row: &SessionRow,
        origin: Origin,
        events: Option<SeedEvents>,
    ) -> Self {
        let tmux: Box<dyn crate::tmux::TmuxExec> = if row.host_alias == "local" {
            Box::new(crate::tmux::LocalTmux)
        } else {
            Box::new(crate::tmux::RemoteTmux {
                client: Arc::clone(ssh),
                host: row.host_alias.clone(),
            })
        };
        LivePane {
            store,
            ssh,
            session_id: row.id,
            host: row.host_alias.clone(),
            tmux_name: row.tmux_name.clone(),
            origin,
            events,
            carries: Vec::new(),
            tmux,
        }
    }

    async fn send(&self, prompt: &str) -> Result<(), IpcError> {
        send_prompt_inner(
            self.store,
            self.ssh,
            &self.host,
            &self.tmux_name,
            prompt,
            true,
            self.origin,
        )
        .await
    }
}

#[async_trait::async_trait]
impl SeedPane for LivePane<'_> {
    async fn capture(&self) -> Option<String> {
        self.tmux.capture_pane(&self.tmux_name).await.ok()
    }
    async fn type_prompt(&self, prompt: &str) -> Result<(), IpcError> {
        self.send(prompt).await
    }
    async fn press_enter(&self) -> Result<(), IpcError> {
        // An empty body with `submit` is a bare Enter, recorded as no prompt.
        self.send("").await
    }
    fn ack_state(&self) -> Option<PromptAckState> {
        let s = self.store.lock().ok()?;
        s.prompt_ack_state(self.session_id).ok().flatten()
    }
    fn event(&self, started: bool, detail: Option<&str>) {
        let Some(ev) = self.events else {
            tracing::info!(
                session = %self.tmux_name,
                host = %self.host,
                started,
                detail = detail.unwrap_or(""),
                "[seed] first prompt"
            );
            return;
        };
        let kind = if started { ev.started } else { ev.waiting };
        if let Ok(s) = self.store.lock() {
            let _ = s.insert_session_event(self.session_id, kind, detail);
        }
    }
    fn ready(&self) {
        let kind = self.events.and_then(|e| e.ready);
        if kind.is_none() && self.carries.is_empty() {
            return;
        }
        let Ok(s) = self.store.lock() else {
            return;
        };
        if !self.carries.is_empty() {
            let current = s
                .get_session_by_id(self.session_id)
                .ok()
                .flatten()
                .and_then(|r| r.claude_session_id);
            if let Err(e) = s.mark_handovers_delivered(&self.carries, current.as_deref()) {
                tracing::warn!(error = %e.message, "[seed] stamping the typed handover failed");
            }
        }
        if let Some(kind) = kind {
            let _ = s.insert_session_event(self.session_id, kind, None);
        }
    }
}

/// Seed `row` with `prompt` in the background ([`BACKGROUND`]), the
/// outcome written to its timeline as `events`.
pub fn spawn_seed(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    row: &SessionRow,
    prompt: String,
    events: SeedEvents,
) {
    let row = row.clone();
    crate::rt::spawn(async move {
        let pane = LivePane::new(&store, &ssh, &row, Origin::Person, Some(events));
        seed(&pane, &prompt, &BACKGROUND).await;
    });
}

/// Seed `row` with `prompt` before returning ([`FOREGROUND`]), as a
/// person's prompt. `false` when the prompt was not typed (the REPL did not
/// come up, a dialog is up, the send failed); the session is live either way
/// and the person can type it.
pub async fn seed_now(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    row: &SessionRow,
    prompt: &str,
) -> bool {
    seed_now_as(store, ssh, row, prompt, Origin::Person).await
}

/// [`seed_now`] with the prompt's [`Origin`].
pub(super) async fn seed_now_as(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    row: &SessionRow,
    prompt: &str,
    origin: Origin,
) -> bool {
    let pane = LivePane::new(store, ssh, row, origin, None);
    seed(&pane, prompt, &FOREGROUND).await
}

/// Seed a routine run's session with its whole prompt ([`BACKGROUND`]), as
/// fleet's own words. The prompt is typed, not left for a hook to carry:
/// a fresh session takes no turn until something is typed into it, and
/// the typed prompt does not depend on the hook's delivery. `handover` is
/// the row the run queued with the same text, stamped delivered when the
/// REPL is ready so the hook does not repeat it. `Err` says why nothing
/// was typed; the session is live either way.
pub async fn seed_routine(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    row: &SessionRow,
    prompt: &str,
    handover: i64,
) -> Result<(), String> {
    let mut pane = LivePane::new(store, ssh, row, Origin::Fleet, Some(HANDOVER));
    pane.carries = vec![handover];
    seed_outcome(&pane, prompt, &BACKGROUND).await
}

/// Wait ([`FOREGROUND`]) for `row`'s REPL before the caller types into it
/// itself.
pub async fn wait_for_repl(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    row: &SessionRow,
) -> ReplWait {
    let pane = LivePane::new(store, ssh, row, Origin::Person, None);
    await_repl(&pane, &FOREGROUND).await
}

/// Wait for a settled REPL, type, then make sure the prompt was submitted.
/// `true` when it was typed.
pub(crate) async fn seed(pane: &dyn SeedPane, prompt: &str, t: &SeedTimings) -> bool {
    seed_outcome(pane, prompt, t).await.is_ok()
}

/// [`seed`], with why the prompt was not typed.
pub(crate) async fn seed_outcome(
    pane: &dyn SeedPane,
    prompt: &str,
    t: &SeedTimings,
) -> Result<(), String> {
    match await_repl(pane, t).await {
        ReplWait::Ready => {}
        ReplWait::Dialog(why) => return Err(format!("a dialog is up in its session ({why})")),
        ReplWait::NotReady => {
            return Err("Claude's prompt did not come up in its session".to_string())
        }
    }
    pane.ready();
    let before = pane.ack_state();
    if let Err(e) = pane.type_prompt(prompt).await {
        pane.event(false, Some(&format!("send failed: {}", e.code)));
        return Err(format!("typing it failed: {}", e.message));
    }
    await_ack(pane, prompt, before, t).await;
    Ok(())
}

/// Poll until the REPL has shown ready [`SETTLE`] times in a row. A dialog
/// is reported once (`waiting`) and waited out for `t.dialog_wait`: the
/// person answers it, and the prompt goes in then. Any end but `Ready` is
/// reported too.
pub(crate) async fn await_repl(pane: &dyn SeedPane, t: &SeedTimings) -> ReplWait {
    let mut deadline = tokio::time::Instant::now() + t.wait;
    let mut streak = 0;
    let mut reported: Option<&'static str> = None;
    loop {
        let gate = match pane.capture().await {
            Some(text) => prompt_gate(&text),
            None => PromptGate::NotYet,
        };
        match gate {
            PromptGate::Ready => {
                streak += 1;
                if streak >= SETTLE {
                    return ReplWait::Ready;
                }
            }
            PromptGate::TrustDialog | PromptGate::Blocked => {
                streak = 0;
                let why = if gate == PromptGate::TrustDialog {
                    "trust_prompt"
                } else {
                    "dialog"
                };
                if reported != Some(why) {
                    pane.event(false, Some(why));
                    reported = Some(why);
                    match t.dialog_wait {
                        Some(w) => deadline = deadline.max(tokio::time::Instant::now() + w),
                        None => return ReplWait::Dialog(why),
                    }
                }
            }
            PromptGate::NotYet => streak = 0,
        }
        if tokio::time::Instant::now() >= deadline {
            let why = match reported {
                Some(_) => "dialog_not_answered",
                None => "repl_not_ready",
            };
            pane.event(false, Some(why));
            return ReplWait::NotReady;
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Wait for the hook to acknowledge the prompt. Unacknowledged, the pane is
/// looked at every `t.nudge`: a prompt still sitting in the input box gets
/// its Enter again, one that vanished is typed again ([`unacked_step`]), at
/// most `t.retries` times.
async fn await_ack(
    pane: &dyn SeedPane,
    prompt: &str,
    before: Option<PromptAckState>,
    t: &SeedTimings,
) {
    let start = tokio::time::Instant::now();
    let deadline = start + t.ack;
    let mut next_look = start + t.nudge;
    let mut retries = 0;
    let may_retype = before.as_ref().is_some_and(|b| b.hooks_seen);
    loop {
        let seq = pane.ack_state().map(|st| st.prompt_submit_seq);
        if let (Some(b), Some(now)) = (before.as_ref(), seq) {
            if now > b.prompt_submit_seq {
                pane.event(true, None);
                return;
            }
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            pane.event(false, Some("start prompt not acknowledged"));
            return;
        }
        if now >= next_look && retries < t.retries {
            next_look = now + t.nudge;
            let step = match pane.capture().await {
                Some(text) => unacked_step(&text, prompt, may_retype),
                None => Nudge::Wait,
            };
            let sent = match step {
                Nudge::Wait => None,
                Nudge::Submit => Some(pane.press_enter().await),
                Nudge::Retype => Some(pane.type_prompt(prompt).await),
            };
            if let Some(sent) = sent {
                retries += 1;
                if let Err(e) = sent {
                    tracing::warn!(error = %e.message, "[seed] retrying the first prompt failed");
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// What to do about a prompt the hook has not acknowledged yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Nudge {
    /// Nothing: Claude is busy (a queued prompt waits for a SessionStart
    /// hook), a dialog is up, or the prompt was plainly submitted.
    Wait,
    /// The prompt sits in the input box, its Enter lost: press Enter.
    Submit,
    /// The REPL is idle with an empty input and the prompt is nowhere on
    /// screen: it was swallowed while Claude was starting. Type it again.
    Retype,
}

/// PURE: decide from the pane what an unacknowledged prompt needs.
/// `may_retype` is whether the session's hooks report at all: without them
/// there is never an acknowledgement, and a retype would risk a second copy.
fn unacked_step(pane: &str, prompt: &str, may_retype: bool) -> Nudge {
    if prompt_gate(pane) != PromptGate::Ready {
        return Nudge::Wait;
    }
    let intel = crate::agent_adapter::claude().analyze_pane(pane);
    if intel.derived_status == Some(ClaudeStatus::Working) {
        return Nudge::Wait;
    }
    let head = prompt_head(prompt);
    let Some(input) = input_box(pane) else {
        return Nudge::Wait;
    };
    if !head.is_empty() && squash(&input).starts_with(&head) {
        return Nudge::Submit;
    }
    let empty = input.is_empty() || input.starts_with("Try \"");
    if may_retype && empty && !squash(pane).contains(&head) {
        return Nudge::Retype;
    }
    Nudge::Wait
}

/// The first words of a prompt, whitespace-squashed: what the pane is
/// searched for (a wrapped line holds at least this much).
fn prompt_head(prompt: &str) -> String {
    squash(prompt).chars().take(24).collect()
}

/// `text` with every run of whitespace made one space.
fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// PURE: the text on the first line of the REPL's input box — the
/// bottom-most `❯` / `>` prompt line, so a queued or submitted prompt above
/// it in the transcript is not mistaken for it. `""` for an empty box; the
/// placeholder (`Try "…"`) is returned as it is. `None` when there is no
/// input box on screen.
fn input_box(pane: &str) -> Option<String> {
    // `capture-pane` without `-e` carries no escape codes.
    pane.lines().rev().find_map(|line| {
        let l = line.trim_start().trim_start_matches('│').trim();
        let rest = l.strip_prefix('❯').or_else(|| l.strip_prefix('>'))?;
        if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            return None;
        }
        Some(rest.trim().trim_end_matches('│').trim().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_start_prompt_is_never_typed_into_a_dialog() {
        assert_eq!(
            prompt_gate("Do you trust the files in this folder?\n❯ 1. Yes, proceed\n  2. No, exit"),
            PromptGate::TrustDialog
        );
        assert_eq!(prompt_gate("Loading…"), PromptGate::NotYet);
        assert_eq!(
            prompt_gate("╭────╮\n│ >  │\n╰────╯\n  ? for shortcuts"),
            PromptGate::Ready
        );
    }

    const PROMPT: &str = "Start on ABC-1: the ticket's context is in your fleet brief. Read it, then plan before you edit.";
    const READY: &str =
        "────\n❯ Try \"fix lint errors\"\n────\n  ⏵⏵ bypass permissions on (shift+tab to cycle)";
    const TRUST: &str = " Quick safety check: Is this a project you created or one you trust?\n ❯ No, exit\n   Yes, I trust this folder\n Enter to confirm · Esc to cancel";
    /// The prompt typed, its Enter lost.
    const IN_INPUT: &str = "────\n❯ Start on ABC-1: the ticket's context is in your fleet brief.\n  Read it, then plan before you edit.\n────\n  ⏵⏵ bypass permissions on (shift+tab to cycle)";
    /// Submitted, queued behind a slow SessionStart hook.
    const QUEUED: &str = "❯ Start on ABC-1: the ticket's context is in your fleet brief.\n✶ Moonwalking… (running SessionStart hook · 3s)\n────\n❯ \n────\n  ⏵⏵ bypass permissions on (shift+tab to cycle) · esc to interrupt";

    /// A pane that plays `before` until the first send (the last frame
    /// repeats), then `after[n]` after the n+1-th send; the sends listed in
    /// `acks` are acknowledged by the hook.
    struct Scripted {
        before: Mutex<std::collections::VecDeque<&'static str>>,
        after: Vec<&'static str>,
        acks: Vec<usize>,
        hooks_seen: bool,
        sends: Mutex<Vec<&'static str>>,
        captures: Mutex<usize>,
        captures_at_type: Mutex<Option<usize>>,
        seq: Mutex<i64>,
        events: Mutex<Vec<String>>,
        /// The number of sends made when each `ready` was reported.
        readies: Mutex<Vec<usize>>,
    }

    impl Scripted {
        fn new(before: &[&'static str], after: &[&'static str], acks: &[usize]) -> Self {
            Scripted {
                before: Mutex::new(before.iter().copied().collect()),
                after: after.to_vec(),
                acks: acks.to_vec(),
                hooks_seen: true,
                sends: Mutex::new(Vec::new()),
                captures: Mutex::new(0),
                captures_at_type: Mutex::new(None),
                seq: Mutex::new(0),
                events: Mutex::new(Vec::new()),
                readies: Mutex::new(Vec::new()),
            }
        }
        fn send(&self, what: &'static str) {
            let mut sends = self.sends.lock().unwrap();
            if self.acks.contains(&sends.len()) {
                *self.seq.lock().unwrap() += 1;
            }
            sends.push(what);
        }
        fn sends(&self) -> Vec<&'static str> {
            self.sends.lock().unwrap().clone()
        }
        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl SeedPane for Scripted {
        async fn capture(&self) -> Option<String> {
            *self.captures.lock().unwrap() += 1;
            let n = self.sends.lock().unwrap().len();
            if n == 0 {
                let mut b = self.before.lock().unwrap();
                let frame = if b.len() > 1 {
                    b.pop_front()
                } else {
                    b.front().copied()
                };
                return frame.map(str::to_string);
            }
            Some(self.after[(n - 1).min(self.after.len() - 1)].to_string())
        }
        async fn type_prompt(&self, _: &str) -> Result<(), IpcError> {
            self.captures_at_type
                .lock()
                .unwrap()
                .get_or_insert(*self.captures.lock().unwrap());
            self.send("type");
            Ok(())
        }
        async fn press_enter(&self) -> Result<(), IpcError> {
            self.send("enter");
            Ok(())
        }
        fn ack_state(&self) -> Option<PromptAckState> {
            Some(PromptAckState {
                prompt_submit_seq: *self.seq.lock().unwrap(),
                hooks_seen: self.hooks_seen,
            })
        }
        fn event(&self, started: bool, detail: Option<&str>) {
            let kind = if started {
                HANDOVER.started
            } else {
                HANDOVER.waiting
            };
            let e = match detail {
                Some(d) => format!("{kind}: {d}"),
                None => kind.to_string(),
            };
            self.events.lock().unwrap().push(e);
        }
        fn ready(&self) {
            let sent = self.sends.lock().unwrap().len();
            self.readies.lock().unwrap().push(sent);
        }
    }

    /// Task → session P-5: the start's `repl_ready` step is reported once,
    /// after the trust dialog and before the prompt is typed; a REPL that
    /// never comes up reports none.
    #[tokio::test(start_paused = true)]
    async fn the_repl_is_reported_ready_once_before_the_prompt_is_typed() {
        let pane = Scripted::new(&["", TRUST, READY], &[READY], &[0]);
        seed(&pane, PROMPT, &BACKGROUND).await;
        assert_eq!(*pane.readies.lock().unwrap(), [0]);

        let stuck = Scripted::new(&[TRUST], &[READY], &[0]);
        seed(&stuck, PROMPT, &FOREGROUND).await;
        assert!(stuck.readies.lock().unwrap().is_empty());
        assert_eq!(HANDOVER.ready, Some(REPL_READY));
    }

    /// The trust dialog holds the prompt back, never drops it: once the
    /// person answers it, the prompt goes in.
    #[tokio::test(start_paused = true)]
    async fn the_start_prompt_goes_in_once_the_trust_dialog_is_answered() {
        let pane = Scripted::new(&["", TRUST, TRUST, TRUST, READY], &[READY], &[0]);
        seed(&pane, PROMPT, &BACKGROUND).await;
        assert_eq!(pane.sends(), ["type"]);
        assert_eq!(
            pane.events(),
            ["handover_waiting: trust_prompt", "handover_started"]
        );
    }

    /// A REPL seen for one capture is not yet the one that reads the prompt
    /// (a `cl --resume` about to fail over, a redraw).
    #[tokio::test(start_paused = true)]
    async fn the_start_prompt_waits_for_a_settled_repl() {
        let pane = Scripted::new(&["", READY, "", READY, READY], &[READY], &[0]);
        seed(&pane, PROMPT, &BACKGROUND).await;
        assert_eq!(pane.sends(), ["type"]);
        assert_eq!(*pane.captures_at_type.lock().unwrap(), Some(5));
    }

    /// The prompt sits in the input box, its Enter lost: Enter again.
    #[tokio::test(start_paused = true)]
    async fn a_start_prompt_left_in_the_input_box_is_submitted() {
        let pane = Scripted::new(&[READY], &[IN_INPUT, READY], &[1]);
        seed(&pane, PROMPT, &BACKGROUND).await;
        assert_eq!(pane.sends(), ["type", "enter"]);
        assert_eq!(pane.events(), ["handover_started"]);
    }

    /// Swallowed while Claude was starting: idle, empty input, nowhere on
    /// screen. Typed again — but only where the hooks report, since without
    /// them nothing is ever acknowledged.
    #[tokio::test(start_paused = true)]
    async fn a_swallowed_start_prompt_is_typed_again_only_where_hooks_report() {
        let pane = Scripted::new(&[READY], &[READY], &[1]);
        seed(&pane, PROMPT, &BACKGROUND).await;
        assert_eq!(pane.sends(), ["type", "type"]);
        assert_eq!(pane.events(), ["handover_started"]);

        let mut quiet = Scripted::new(&[READY], &[READY], &[]);
        quiet.hooks_seen = false;
        seed(&quiet, PROMPT, &BACKGROUND).await;
        assert_eq!(quiet.sends(), ["type"]);
        assert_eq!(
            quiet.events(),
            ["handover_waiting: start prompt not acknowledged"]
        );
    }

    /// A prompt queued behind a slow SessionStart hook is left alone.
    #[tokio::test(start_paused = true)]
    async fn a_queued_start_prompt_is_not_sent_twice() {
        let pane = Scripted::new(&[READY], &[QUEUED], &[]);
        seed(&pane, PROMPT, &BACKGROUND).await;
        assert_eq!(pane.sends(), ["type"]);
    }

    /// A dialog nobody answers ends the wait, typed into never.
    #[tokio::test(start_paused = true)]
    async fn an_unanswered_dialog_never_gets_the_start_prompt() {
        let pane = Scripted::new(&[TRUST], &[READY], &[0]);
        seed(&pane, PROMPT, &BACKGROUND).await;
        assert!(pane.sends().is_empty());
        assert_eq!(
            pane.events(),
            [
                "handover_waiting: trust_prompt",
                "handover_waiting: dialog_not_answered"
            ]
        );
    }

    #[test]
    fn the_input_box_is_the_bottom_most_prompt_line() {
        assert_eq!(input_box(QUEUED).as_deref(), Some(""));
        assert_eq!(
            input_box(IN_INPUT).as_deref(),
            Some("Start on ABC-1: the ticket's context is in your fleet brief.")
        );
        assert_eq!(
            input_box("╭───╮\n│ > hi there │\n╰───╯").as_deref(),
            Some("hi there")
        );
        assert_eq!(input_box("Loading…"), None);
        assert_eq!(unacked_step(QUEUED, PROMPT, true), Nudge::Wait);
        assert_eq!(unacked_step(IN_INPUT, PROMPT, false), Nudge::Submit);
        assert_eq!(unacked_step(READY, PROMPT, true), Nudge::Retype);
        assert_eq!(unacked_step(TRUST, PROMPT, true), Nudge::Wait);
    }

    /// A caller that cannot wait for a person stops at the dialog — and
    /// never presses Enter into it (the trust prompt's default is "No,
    /// exit").
    #[tokio::test(start_paused = true)]
    async fn a_foreground_seed_never_types_into_a_dialog() {
        let pane = Scripted::new(&["", TRUST], &[READY], &[0]);
        assert!(!seed(&pane, PROMPT, &FOREGROUND).await);
        assert!(pane.sends().is_empty());
        assert_eq!(pane.events(), ["handover_waiting: trust_prompt"]);
        assert_eq!(
            await_repl(&Scripted::new(&[TRUST], &[], &[]), &FOREGROUND).await,
            ReplWait::Dialog("trust_prompt")
        );
    }

    /// A routine run fails with why its prompt was not typed (the
    /// scheduler writes it as the run's reason).
    #[tokio::test(start_paused = true)]
    async fn an_untyped_seed_says_why() {
        let dialog = Scripted::new(&[TRUST], &[], &[]);
        assert_eq!(
            seed_outcome(&dialog, PROMPT, &FOREGROUND).await,
            Err("a dialog is up in its session (trust_prompt)".to_string())
        );
        let never = Scripted::new(&[""], &[], &[]);
        assert_eq!(
            seed_outcome(&never, PROMPT, &FOREGROUND).await,
            Err("Claude's prompt did not come up in its session".to_string())
        );
        let pane = Scripted::new(&[READY], &[IN_INPUT, READY], &[1]);
        assert_eq!(seed_outcome(&pane, PROMPT, &FOREGROUND).await, Ok(()));
    }

    /// The pre-2.x readiness check (`>` or `│` anywhere) never matched a
    /// 2.x REPL and typed blind after its timeout; the gate does match it.
    #[tokio::test(start_paused = true)]
    async fn a_foreground_seed_types_into_a_2x_repl_and_submits_it() {
        let pane = Scripted::new(&["", "", READY], &[IN_INPUT, READY], &[1]);
        assert!(seed(&pane, PROMPT, &FOREGROUND).await);
        assert_eq!(pane.sends(), ["type", "enter"]);
        assert_eq!(pane.events(), ["handover_started"]);
    }
}
