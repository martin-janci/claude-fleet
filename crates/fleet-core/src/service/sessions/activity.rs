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

/// PURE: the probe for a captured pane tail.
pub fn probe_from_tail(tail: &str) -> ActivityProbe {
    let intel = pane_intel::analyze(tail);
    ActivityProbe {
        claude_status: intel.derived_status.map(|s| s.as_str().to_string()),
        current_activity: intel.activity,
        stuck_kind: intel.stuck.map(|k| k.as_str().to_string()),
        waiting_for: intel.waiting_for.map(|w| w.as_str().to_string()),
        spinner: pane_intel::spinner_line(tail),
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
    Ok(probe_from_tail(&tail))
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
