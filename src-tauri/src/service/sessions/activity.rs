//! On-demand pane probe for the Conversation tab's live indicator: one
//! `capture-pane` of the session's tail, analysed the same way the reconcile
//! tick does it, returned without touching the store. Cheap enough to poll
//! every couple of seconds while a session is working; the 20 s tick stays
//! the authority for what is persisted.

use super::*;
use crate::ipc_error::codes;
use crate::ipc_error::lock;
use crate::service::pane_intel;
use serde::Serialize;

/// Lines of pane tail the probe reads: enough for the spinner, a queued
/// prompt line and the mode footer; more than the tick's 8 so a dialog's
/// question still fits above its options.
pub const ACTIVITY_TAIL_LINES: u32 = 12;

/// What the pane looks like right now. Strings use the same vocabularies as
/// the session row (`claude_status`, `stuck_kind`), so the frontend can lay
/// the probe over the row.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct ActivityProbe {
    pub claude_status: Option<String>,
    pub current_activity: Option<String>,
    pub stuck_kind: Option<String>,
    /// `permission` | `input` when a dialog is what blocks the pane.
    pub waiting_for: Option<String>,
    /// The live spinner line without its glyph (`Cooking… (3s · …)`), only
    /// while the REPL is generating.
    pub spinner: Option<String>,
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

    #[test]
    fn empty_capture_is_an_all_none_probe() {
        assert_eq!(
            probe_from_tail(""),
            ActivityProbe {
                claude_status: None,
                current_activity: None,
                stuck_kind: None,
                waiting_for: None,
                spinner: None
            }
        );
    }
}
