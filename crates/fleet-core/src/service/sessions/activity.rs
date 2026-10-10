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

// The dialog a client drew its answer from, sent with the key so the hub
// checks it against a fresh read of the pane and presses in the same step
// (`send_prompt { keys, expect }`). Without it the check was the client's:
// a read, a round trip back, then the press — and another client answering,
// or the dialog closing, inside that window put the key into whatever came
// next. Compared field by field as the clients' own fingerprints do: kind,
// question, each option's number and label, and the tool call (`detail`);
// `selected` (the option that must still be highlighted) only for Enter.
//
// Plain comments, not doc comments: these would become the tool schema's
// descriptions, which every MCP client pays for on every connect
// (`the_served_definition_budget_stays_bounded`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ExpectDialog {
    pub kind: String,
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub options: Vec<ExpectOption>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub selected: Option<u8>,
}

// One option of an `ExpectDialog`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ExpectOption {
    pub n: u8,
    pub label: String,
}

/// PURE: whether `on_screen` is still the dialog `expect` describes, for
/// pressing `key`. `E_CONFLICT` with a sentence a client can show as is.
pub fn dialog_still_matches(
    expect: &ExpectDialog,
    on_screen: Option<&pane_intel::PendingInput>,
    key: crate::tmux::NamedKey,
) -> Result<(), IpcError> {
    let Some(p) = on_screen else {
        return Err(IpcError::new(
            codes::E_CONFLICT,
            "That dialog is gone — nothing was sent.",
        ));
    };
    let same = p.kind == expect.kind
        && p.question == expect.question
        && p.detail == expect.detail
        && p.options.len() == expect.options.len()
        && p.options
            .iter()
            .zip(&expect.options)
            .all(|(a, b)| a.n == b.n && a.label == b.label);
    if !same {
        return Err(IpcError::new(
            codes::E_CONFLICT,
            "The dialog changed — nothing was sent.",
        ));
    }
    if key == crate::tmux::NamedKey::Enter {
        if let Some(want) = expect.selected {
            if p.options.iter().find(|o| o.selected).map(|o| o.n) != Some(want) {
                return Err(IpcError::new(
                    codes::E_CONFLICT,
                    "A different answer is highlighted now — nothing was sent.",
                ));
            }
        }
    }
    Ok(())
}

/// Read the pane of `session_id` and refuse unless it still shows the
/// dialog `expect` describes ([`dialog_still_matches`]); the caller presses
/// right after, with no client round trip in between.
pub async fn check_expected_dialog(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    session_id: i64,
    expect: &ExpectDialog,
    key: crate::tmux::NamedKey,
) -> Result<(), IpcError> {
    let probe = session_activity(store, ssh, session_id).await?;
    let on_screen = probe.pending_input.as_ref().filter(|_| {
        probe.stuck_kind.is_none() && probe.claude_status.as_deref() == Some("blocked")
    });
    dialog_still_matches(expect, on_screen, key)
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

    fn dialog() -> pane_intel::PendingInput {
        pane_intel::PendingInput {
            kind: "permission".into(),
            question: Some("Do you want to proceed?".into()),
            options: vec![
                pane_intel::PendingOption {
                    n: 1,
                    label: "Yes".into(),
                    selected: true,
                    checked: false,
                },
                pane_intel::PendingOption {
                    n: 2,
                    label: "No".into(),
                    selected: false,
                    checked: false,
                },
            ],
            multi: false,
            detail: Some("Bash(rm -rf build)".into()),
        }
    }

    fn expect_of(p: &pane_intel::PendingInput) -> ExpectDialog {
        ExpectDialog {
            kind: p.kind.clone(),
            question: p.question.clone(),
            options: p
                .options
                .iter()
                .map(|o| ExpectOption {
                    n: o.n,
                    label: o.label.clone(),
                })
                .collect(),
            detail: p.detail.clone(),
            selected: p.options.iter().find(|o| o.selected).map(|o| o.n),
        }
    }

    #[test]
    fn the_expected_dialog_is_pressed_only_while_it_is_on_screen() {
        use crate::tmux::NamedKey;
        let d = dialog();
        let expect = expect_of(&d);
        let one = NamedKey::parse("1").unwrap();
        assert!(dialog_still_matches(&expect, Some(&d), one).is_ok());
        // Gone.
        assert_eq!(
            dialog_still_matches(&expect, None, one).unwrap_err().code,
            codes::E_CONFLICT
        );
        // Same question and options, another command.
        let other = pane_intel::PendingInput {
            detail: Some("Bash(rm -rf ~)".into()),
            ..d.clone()
        };
        assert!(dialog_still_matches(&expect, Some(&other), one).is_err());
        // Enter with the highlight moved to "No".
        let mut moved = d.clone();
        for o in &mut moved.options {
            o.selected = o.n == 2;
        }
        assert!(dialog_still_matches(&expect, Some(&moved), NamedKey::Enter).is_err());
        assert!(
            dialog_still_matches(&expect, Some(&moved), one).is_ok(),
            "a digit names its option; the highlight does not matter"
        );
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
