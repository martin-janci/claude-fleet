//! On-demand pane probe for the Conversation tab's live indicator: one
//! `capture-pane` of the session's tail, analysed the same way the reconcile
//! tick does it, returned without touching the store. Cheap enough to poll
//! every couple of seconds while a session is working; the 20 s tick stays
//! the authority for what is persisted.

use super::*;
use crate::ipc_error::codes;
use crate::ipc_error::lock;
use crate::service::pane_intel;
use serde::{Deserialize, Serialize};

/// Lines of pane tail the probe reads: enough for the spinner, a queued
/// prompt line and the mode footer; more than the tick's 8 so a dialog's
/// question still fits above its options.
pub const ACTIVITY_TAIL_LINES: u32 = 12;

/// What the pane looks like right now. Strings use the same vocabularies as
/// the session row (`claude_status`, `stuck_kind`), so the frontend can lay
/// the probe over the row.
/// `Deserialize` is for the hub client, which reads this back off the wire
/// (`backend::remote::session_activity`). No `serde(default)` anywhere: a
/// hub that does not send a field is a hub that cannot answer this probe,
/// and a silently all-`None` reading would look exactly like an idle pane.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ActivityProbe {
    pub claude_status: Option<String>,
    pub current_activity: Option<String>,
    pub stuck_kind: Option<String>,
    /// `permission` | `input` when a dialog is what blocks the pane.
    pub waiting_for: Option<String>,
    /// The live spinner line without its glyph (`Cooking… (3s · …)`), only
    /// while the REPL is generating.
    pub spinner: Option<String>,
    /// The permission / question dialog on screen right now, options and
    /// all — the same value the tick stores on `sessions.pending_input`,
    /// but seconds old instead of up to a tick old. A client draws the
    /// answer buttons from this and re-reads it to check, immediately
    /// before sending, that the dialog it is answering is still the dialog
    /// on screen.
    ///
    /// The one `serde(default)` in this struct, deliberately: a hub too old
    /// to know this field would otherwise fail the whole probe. The rule
    /// above guards against a silently all-`None` probe reading as an idle
    /// pane — a missing dialog cannot lie that way, it only falls back to
    /// the row and to "open the terminal", which is what every client did
    /// before the field existed.
    #[serde(default)]
    pub pending_input: Option<pane_intel::PendingInput>,
}

/// PURE: the probe for a captured Claude Code pane tail.
pub fn probe_from_tail(tail: &str) -> ActivityProbe {
    probe_with(crate::agent_adapter::claude(), tail)
}

/// PURE: the probe for a pane tail, read by the session's agent.
pub fn probe_with(agent: &dyn crate::agent_adapter::AgentAdapter, tail: &str) -> ActivityProbe {
    let intel = agent.analyze_pane(tail);
    ActivityProbe {
        claude_status: intel.derived_status.map(|s| s.as_str().to_string()),
        current_activity: intel.activity,
        stuck_kind: intel.stuck.map(|k| k.as_str().to_string()),
        waiting_for: intel.waiting_for.map(|w| w.as_str().to_string()),
        spinner: agent.spinner_line(tail),
        pending_input: intel.pending_input,
    }
}

/// Probe one session's pane. Errors: `E_NOTFOUND`, `E_INVALID_STATE` for a
/// row that runs outside tmux (bg / external: nothing to capture), transport
/// codes. An empty capture (pane just vanished) yields an all-`None` probe
/// rather than an error, like the tick.
pub async fn session_activity(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    session_id: i64,
) -> Result<ActivityProbe, IpcError> {
    let row = {
        let s = lock(store)?;
        s.get_session_by_id(session_id)?.ok_or_else(|| {
            IpcError::new(codes::E_NOTFOUND, format!("session {session_id} not found"))
        })?
    };
    if crate::store::has_no_pane(&row.kind) {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            "session runs outside tmux; there is no pane to probe",
        ));
    }
    crate::validate::tmux_name_addressable(&row.tmux_name)?;
    let tmux = super::reconcile::exec_for(&row.host_alias, ssh);
    let tail = tmux
        .capture_pane_scrollback(&row.tmux_name, ACTIVITY_TAIL_LINES)
        .await?;
    let agent = crate::agent_adapter::for_session(&row.kind, Some(&row.agent))
        .unwrap_or_else(crate::agent_adapter::claude);
    Ok(probe_with(agent, &tail))
}

/// Probe one session's pane and write what it shows about a dialog through
/// to the row ([`Store::record_dialog_probe`]): the fast path that keeps a
/// small, urgent fact — "a dialog is up, with these options" / "it was
/// answered" — from waiting for the 20 s reconcile tick and its whole-host
/// probe. One `capture-pane`, one SSH round trip.
pub async fn sync_dialog(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    session_id: i64,
) -> Result<ActivityProbe, IpcError> {
    let probe = session_activity(store, ssh, session_id).await?;
    if probe.stuck_kind.is_none() {
        let status = probe
            .claude_status
            .as_deref()
            .and_then(|s| s.parse::<pane_intel::ClaudeStatus>().ok());
        let s = lock(store)?;
        s.record_dialog_probe(session_id, status, probe.pending_input.as_ref())?;
    }
    Ok(probe)
}

/// When a dialog fast-path read is made, in ms after the event that asked
/// for it. Claude Code redraws within a few hundred ms of a key or of
/// raising a dialog; the later reads catch a slow host, or the NEXT dialog
/// a tool call raises right after an approval.
pub const DIALOG_FOLLOWUP_MS: [u64; 3] = [300, 1_200, 3_500];

/// Spawn the dialog fast path for `session_id`: re-read its pane at
/// [`DIALOG_FOLLOWUP_MS`] and write the dialog through. Called when the
/// Notification hook says a dialog went up, and after a key was pressed
/// into a session. Stops early once a read finds a parsed dialog after a
/// [`DialogFollowup::Raised`] (the buttons are up). Best-effort: a failed
/// read is logged and the tick catches up.
pub fn spawn_dialog_followup(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    session_id: i64,
    why: DialogFollowup,
) {
    let _ = crate::rt::try_spawn(async move {
        let mut waited = 0;
        for at in DIALOG_FOLLOWUP_MS {
            tokio::time::sleep(std::time::Duration::from_millis(at - waited)).await;
            waited = at;
            match sync_dialog(&store, &ssh, session_id).await {
                Ok(p) if why == DialogFollowup::Raised && p.pending_input.is_some() => return,
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!(session_id, error = %e.message, "[dialog] fast-path read failed");
                    return;
                }
            }
        }
    });
}

/// Why [`spawn_dialog_followup`] runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogFollowup {
    /// The Notification hook: a dialog went up; read until it is parsed.
    Raised,
    /// A key went into the session: read until the pane settles, so the
    /// answered dialog leaves and a following one comes up.
    Answered,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_pane_yields_working_status_and_the_spinner() {
        let p = probe_from_tail(
            "⏺ Bash(cargo test)\n  ⎿ Running…\n✶ Cooking… (3s · esc to interrupt)\n",
        );
        assert_eq!(p.claude_status.as_deref(), Some("working"));
        assert_eq!(
            p.spinner.as_deref(),
            Some("Cooking… (3s · esc to interrupt)")
        );
        assert_eq!(p.stuck_kind, None);
    }

    #[test]
    fn idle_pane_yields_idle_and_no_spinner() {
        let p = probe_from_tail("❯ \n  ⏸ manual mode on · ? for shortcuts");
        assert_eq!(p.claude_status.as_deref(), Some("idle"));
        assert_eq!(p.spinner, None);
    }

    /// The probe reads 12 lines precisely so a dialog's question still fits
    /// above its options — but the options were being thrown away, leaving
    /// the 20 s tick's row as the only source of the one thing a client
    /// needs to draw an answer button. The dialog rides the probe.
    #[test]
    fn a_dialog_pane_yields_its_question_and_options() {
        let p = probe_from_tail(include_str!("../testdata/pane_intel/permission_bash.txt"));
        assert_eq!(p.claude_status.as_deref(), Some("blocked"));
        assert_eq!(p.waiting_for.as_deref(), Some("permission"));
        let dialog = p.pending_input.expect("the dialog rides the probe");
        assert_eq!(dialog.kind, "permission");
        assert_eq!(dialog.question.as_deref(), Some("Do you want to proceed?"));
        assert_eq!(dialog.options.len(), 4);
        assert_eq!(dialog.options[0].n, 1);
    }

    #[test]
    fn a_pane_without_a_dialog_carries_no_pending_input() {
        assert_eq!(
            probe_from_tail("❯ \n  ⏸ manual mode on · ? for shortcuts").pending_input,
            None
        );
    }

    /// The other fields deliberately have no `serde(default)`: a hub that
    /// cannot answer the probe must fail loudly rather than read as an idle
    /// pane. `pending_input` is the exception — a hub too old to know about
    /// it degrades to "no buttons, open the terminal", which is honest and
    /// is exactly what every client did before this field existed.
    #[test]
    fn a_probe_from_a_hub_too_old_to_know_the_dialog_still_reads() {
        let older: ActivityProbe = serde_json::from_str(
            r#"{"claude_status":"blocked","current_activity":"waiting for permission: x",
                "stuck_kind":null,"waiting_for":"permission","spinner":null}"#,
        )
        .expect("an older hub's probe must still deserialize");
        assert_eq!(older.waiting_for.as_deref(), Some("permission"));
        assert_eq!(older.pending_input, None);
    }

    #[test]
    fn empty_capture_is_an_all_none_probe() {
        assert_eq!(
            probe_from_tail(""),
            ActivityProbe {
                claude_status: None,
                current_activity: None,
                stuck_kind: None,
                waiting_for: None,
                spinner: None,
                pending_input: None,
            }
        );
    }
}
