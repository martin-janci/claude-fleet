//! Tauri IPC wrappers for tmux session management. Logic lives in
//! `service::sessions`; this file only adapts `tauri::State` to plain
//! references, and chooses the backend.
//!
//! # What routes and what refuses
//!
//! A command routes when the desktop's arguments map **one to one** onto the
//! hub tool's parameters. Where they do not, it refuses with `E_LOCAL_ONLY`
//! and says why, rather than calling a tool that would quietly drop a field.
//! A silent argument mismatch on a session mutation is the worst thing this
//! layer can do: it would succeed, and do something other than what the user
//! asked.
//!
//! Three commands are refused for that reason rather than for want of a tool:
//!
//! - `new_session` — `NewSessionArgs` carries `kind`, `start_command` and
//!   `friendly_name`; `NewSessionParams` carries none of them, and a shell
//!   session is a different tool (`new_shell_session`) with a different shape.
//!   Routing it would drop the user's label and mis-handle shell sessions.
//! - `repair_session` — the tool always runs the **explicit** repair (which
//!   may unregister a stale worktree entry, adopt a moved checkout and
//!   recreate a branch). The desktop's automatic pre-attach check
//!   (`explicit: false`) has no counterpart, and it exists to serve the PTY
//!   attach, which is itself local-only in remote mode.
//! - `session_activity` — `peek_session` reads a pane, but it answers a
//!   different shape than `ActivityProbe`.

use crate::backend::FleetBackend;
use fleet_core::cancel::CancellationRegistry;
use fleet_core::ipc_error::lock;
use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::service::bg_sessions::{
    self, DismissAgentArgs, NewBgSessionArgs, PurgeProjectArgs,
};
use fleet_core::service::repair::{self, RepairReport};
use fleet_core::service::safe_kill::{
    self, DiscardKillSessionArgs, InspectSafeKillArgs, SafeKillInspection, SafeKillSessionArgs,
};
use fleet_core::service::sessions::{
    self, DiscoverLostSessionsArgs, DismissGhostSessionArgs, KillSessionArgs, LostCandidate,
    NewSessionArgs, RecreateSessionArgs, RelatedSessionsArgs, RenameSessionArgs,
    RestartSessionArgs, RestoreHostSessionsArgs, RestoreReport, SendPromptArgs,
    SetFriendlyNameArgs, SpawnReviewArgs,
};
use fleet_core::ssh::SshClient;
use fleet_core::store::{SessionRow, Store};
use std::sync::{Arc, Mutex};
use tauri::State;

/// `force: true` (the sidebar Refresh button) always runs a fleet reconcile
/// pass; the default serves stored rows while the last pass is within the
/// configured interval.
#[tauri::command]
pub async fn list_sessions(
    force: Option<bool>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<SessionRow>, IpcError> {
    routed::list_sessions(&backend, force, &store, &ssh).await
}

#[tauri::command]
pub async fn related_sessions(
    args: RelatedSessionsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<SessionRow>, IpcError> {
    routed::related_sessions(&backend, args, &store).await
}

#[tauri::command]
pub async fn new_session(
    args: NewSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<SessionRow, IpcError> {
    // Not "no tool" — a tool exists and takes fewer arguments than this
    // dialog sends. See the module header.
    backend.local_only(
        "new_session",
        "the hub's new_session tool cannot carry this dialog's session kind, \
         start command or label; start the session on the hub",
    )?;
    sessions::new_session(args, &store, &ssh, &reg).await
}

#[tauri::command]
pub async fn kill_session(
    args: KillSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<i64, IpcError> {
    routed::kill_session(&backend, args, &store, &ssh).await
}

#[tauri::command]
pub async fn safe_kill_session(
    args: SafeKillSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    routed::safe_kill_session(&backend, args, &store, &ssh).await
}

/// Pre-flight check used by the UI before showing the safe-remove dialog:
/// returns dirty files + pushed-state so we can either skip the Claude prompt
/// (clean+pushed) or warn the user about what would be lost.
#[tauri::command]
pub async fn inspect_safe_kill(
    args: InspectSafeKillArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SafeKillInspection, IpcError> {
    backend.local_only(
        "inspect_safe_kill",
        "it inspects the worktree over this machine's SSH connection and the \
         hub exposes no tool for it; retire the session from the hub",
    )?;
    safe_kill::inspect_safe_kill(args, &store, &ssh).await
}

/// Direct remove: skip the Claude prompt, drop the worktree, kill the
/// session. `force` is the discard-dirty toggle — pass `false` for the
/// clean+pushed fast path (any unexpected dirty state errors out), and `true`
/// when the user explicitly chose "discard & kill" from the dialog.
#[tauri::command]
pub async fn discard_kill_session(
    args: DiscardKillSessionArgs,
    force: bool,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<i64, IpcError> {
    backend.local_only(
        "discard_kill_session",
        "the hub exposes no tool that discards a worktree and kills in one \
         step; use safe_kill_session, or do it from the hub",
    )?;
    safe_kill::discard_kill_session(args, force, &store, &ssh).await
}

#[tauri::command]
pub async fn rename_session(
    args: RenameSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    routed::rename_session(&backend, args, &store, &ssh).await
}

#[tauri::command]
pub async fn set_session_friendly_name(
    args: SetFriendlyNameArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::set_session_friendly_name(&backend, args, &store).await
}

#[tauri::command]
pub async fn restart_session(
    args: RestartSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    routed::restart_session(&backend, args, &store, &ssh).await
}

#[tauri::command]
pub async fn send_prompt(
    args: SendPromptArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    routed::send_prompt(&backend, args, &store, &ssh).await
}

#[tauri::command]
pub async fn spawn_review(
    args: SpawnReviewArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    routed::spawn_review(&backend, args, &store, &ssh).await
}

#[tauri::command]
pub async fn recreate_session(
    args: RecreateSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    routed::recreate_session(&backend, args, &store, &ssh).await
}

/// Batch-restore a host's sessions lost to a reboot or a tmux server
/// restart, over `recreate_session`. Logic lives in `service::sessions::restore`.
#[tauri::command]
pub async fn restore_host_sessions(
    args: RestoreHostSessionsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<RestoreReport, IpcError> {
    routed::restore_host_sessions(&backend, args, &store, &ssh).await
}

/// Scan a host's Claude transcripts for lost sessions fleet has no row for
/// and rank/enrich them. Read-only. Logic lives in `service::sessions::discover`.
#[tauri::command]
pub async fn discover_lost_sessions(
    args: DiscoverLostSessionsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<LostCandidate>, IpcError> {
    routed::discover_lost_sessions(&backend, args, &store, &ssh).await
}

#[tauri::command]
pub async fn dismiss_ghost_session(
    args: DismissGhostSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    routed::dismiss_ghost_session(&backend, args, &store).await
}

/// Remove an inactive background agent (`kind='bg'`, not working) from the
/// list. Frontend-only; logic lives in `service::bg_sessions`.
#[tauri::command]
pub fn dismiss_agent_session(
    args: DismissAgentArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    backend.local_only(
        "dismiss_agent_session",
        "use Kill instead: the hub's kill_session removes an inactive \
         agent from the list exactly as this would. It is not routed here \
         because the two differ on a WORKING agent, which this refuses and \
         kill_session stops",
    )?;
    bg_sessions::dismiss_agent_session(args, &store)
}

/// Make the session's directory a healthy git worktree on its branch and its
/// tmux session run there (creating tmux when it is gone). A no-op on a
/// healthy session. Logic lives in `service::repair`.
#[tauri::command]
pub async fn repair_session(
    args: RepairSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<RepairReport, IpcError> {
    backend.local_only(
        "repair_session",
        "the hub's repair_session always runs the EXPLICIT repair, which may \
         unregister a stale worktree entry, adopt a moved checkout and \
         recreate a branch — this app will not turn an automatic pre-attach \
         check into that; repair from the hub",
    )?;
    repair::repair_session(args.session_id, args.explicit, &store, &ssh).await
}

#[derive(serde::Deserialize)]
pub struct RepairSessionArgs {
    pub session_id: i64,
    /// `true`: the Repair workspace button — an explicit repair that may
    /// unregister this worktree's stale entry, adopt a moved checkout,
    /// recreate the branch from its base, re-link, and respawn a live pane.
    /// `false` (default): the automatic pre-attach check, which only creates
    /// what is confirmed missing and reports the rest.
    #[serde(default)]
    pub explicit: bool,
}

/// Launch a Claude background session on the given host.
#[tauri::command]
pub async fn new_bg_session(
    args: NewBgSessionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<bg_sessions::NewBgSessionResult, IpcError> {
    routed::new_bg_session(&backend, args, &store, &ssh).await
}

/// Delete all Claude Code state for a project and remove it from the fleet database.
#[tauri::command]
pub async fn purge_project(
    args: PurgeProjectArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<bg_sessions::PurgeReport>, IpcError> {
    backend.local_only(
        "purge_project",
        "it deletes Claude Code state on every host over this machine's SSH \
         connections and the hub exposes no tool for it; purge from the hub",
    )?;
    bg_sessions::purge_project(args, &store, &ssh).await
}

// ── Operator settings (Wave 2 Track D) ──────────────────────────────────────
//
// Typed key/value settings behind the Settings dialog's automation toggles
// (playbooks, GC, reconcile cadence). The registry in `service::settings`
// owns the key list, defaults and validation; these wrappers only adapt
// `tauri::State`.
//
// Local-only in remote mode, and this is the one pair where returning the
// local answer would be actively misleading: these settings govern the
// reconcile tick, the GC sweeper and the playbooks, none of which this
// process runs when a hub owns the fleet. Showing this app's values would
// show settings that do nothing, and writing one would change nothing.

/// Every registered operator setting with its effective value.
#[tauri::command]
pub fn get_fleet_settings(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<std::collections::BTreeMap<String, String>, IpcError> {
    backend.local_only(
        "get_fleet_settings",
        "these settings drive the reconcile tick, the GC sweeper and the \
         playbooks, which the hub runs and this app does not; read and change \
         them on the hub",
    )?;
    let s = lock(&store)?;
    Ok(fleet_core::service::settings::read_all(&s))
}

/// Validate and persist one operator setting. `E_INVALID` for an unknown key
/// or a value of the wrong shape. Returns the full effective map so the
/// dialog can re-render from one source of truth.
#[tauri::command]
pub fn set_fleet_setting(
    key: String,
    value: String,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<std::collections::BTreeMap<String, String>, IpcError> {
    backend.local_only(
        "set_fleet_setting",
        "these settings drive the reconcile tick, the GC sweeper and the \
         playbooks, which the hub runs and this app does not; change them on \
         the hub",
    )?;
    let s = lock(&store)?;
    fleet_core::service::settings::set(&s, &key, &value)?;
    Ok(fleet_core::service::settings::read_all(&s))
}

// ── Session timeline (Q9) ───────────────────────────────────────────────────

/// Default and ceiling for `session_history`'s `limit`. The store caps the
/// timeline at 500 rows per session, so asking for more returns nothing extra.
const HISTORY_DEFAULT_LIMIT: i64 = 200;
const HISTORY_MAX_LIMIT: i64 = 500;

#[derive(serde::Deserialize)]
pub struct SessionHistoryArgs {
    pub session_id: i64,
    /// Newest-first cap; `None` or non-positive means the default.
    #[serde(default)]
    pub limit: Option<i64>,
}

/// Pure: clamp the requested timeline length into `1..=HISTORY_MAX_LIMIT`.
fn history_limit(requested: Option<i64>) -> i64 {
    match requested {
        Some(n) if n > 0 => n.min(HISTORY_MAX_LIMIT),
        _ => HISTORY_DEFAULT_LIMIT,
    }
}

/// The recorded event timeline for one session, newest first. Same data as
/// the MCP `session_history` tool; rendered by the details pane's Timeline.
#[tauri::command]
pub async fn session_history(
    args: SessionHistoryArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<fleet_core::store::SessionEvent>, IpcError> {
    routed::session_history(&backend, args, &store).await
}

// ── Conversation (structured transcript) ────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct SessionConversationArgs {
    pub session_id: i64,
    /// Most-recent turns to return; omitted = the default window. Clamped
    /// (see `transcript::conv_limits`).
    #[serde(default)]
    pub turns: Option<usize>,
}

/// The session's recent conversation — prompts, assistant text and one line
/// per tool call — read from its Claude Code transcript. Rendered by the
/// details pane's Conversation tab. Errors: `E_NOTFOUND`, `E_INVALID_STATE`
/// (no `claude_session_id` yet), `E_NO_TRANSCRIPT`, transport codes.
#[tauri::command]
pub async fn session_conversation(
    args: SessionConversationArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<fleet_core::service::transcript::Conversation, IpcError> {
    routed::session_conversation(&backend, args, &store, &ssh).await
}

// ── Activity probe (live indicator) ─────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct SessionActivityArgs {
    pub session_id: i64,
}

/// What the session's pane shows right now (status, spinner, stuck / dialog
/// state), read on demand for the Conversation tab's live indicator. Errors:
/// `E_NOTFOUND`, `E_INVALID_STATE` (runs outside tmux), transport codes.
#[tauri::command]
pub async fn session_activity(
    args: SessionActivityArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<sessions::ActivityProbe, IpcError> {
    backend.local_only(
        "session_activity",
        "it captures the session's pane over this machine's SSH connection; \
         the hub's peek_session answers a different shape, so the live \
         indicator is off in remote mode",
    )?;
    sessions::session_activity(&store, &ssh, args.session_id).await
}

/// The routing, away from `tauri::State` so the tests can drive it.
pub(crate) mod routed {
    use super::*;

    pub async fn list_sessions(
        backend: &FleetBackend,
        force: Option<bool>,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<Vec<SessionRow>, IpcError> {
        let force = force.unwrap_or(false);
        match backend.hub() {
            // `force` becomes the tool's `force`, so the sidebar's Refresh
            // button makes the HUB reconcile rather than this app.
            Some(hub) => hub.list_sessions(force).await,
            None if force => sessions::refresh_sessions(store, ssh).await,
            None => sessions::list_sessions(store, ssh).await,
        }
    }

    pub async fn related_sessions(
        backend: &FleetBackend,
        args: RelatedSessionsArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<SessionRow>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.related_sessions(args.session_id).await,
            None => sessions::related_sessions(args, store),
        }
    }

    pub async fn kill_session(
        backend: &FleetBackend,
        args: KillSessionArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<i64, IpcError> {
        match backend.hub() {
            Some(hub) => hub.kill_session(&args).await,
            None => sessions::kill_session(args, store, ssh).await,
        }
    }

    pub async fn safe_kill_session(
        backend: &FleetBackend,
        args: SafeKillSessionArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.safe_kill_session(&args).await,
            None => safe_kill::safe_kill_session(args, store, ssh).await,
        }
    }

    pub async fn rename_session(
        backend: &FleetBackend,
        args: RenameSessionArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.rename_session(&args).await,
            None => sessions::rename_session(args, store, ssh).await,
        }
    }

    pub async fn set_session_friendly_name(
        backend: &FleetBackend,
        args: SetFriendlyNameArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.set_session_friendly_name(&args).await,
            None => sessions::set_session_friendly_name(args, store),
        }
    }

    pub async fn restart_session(
        backend: &FleetBackend,
        args: RestartSessionArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.restart_session(&args).await,
            None => sessions::restart_session(args, store, ssh).await,
        }
    }

    pub async fn send_prompt(
        backend: &FleetBackend,
        args: SendPromptArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<(), IpcError> {
        match backend.hub() {
            Some(hub) => hub.send_prompt(&args).await,
            None => sessions::send_prompt(args, store, ssh).await,
        }
    }

    pub async fn spawn_review(
        backend: &FleetBackend,
        args: SpawnReviewArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.spawn_review(&args).await,
            None => sessions::spawn_review(args, store, ssh).await,
        }
    }

    pub async fn recreate_session(
        backend: &FleetBackend,
        args: RecreateSessionArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<SessionRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.recreate_session(&args).await,
            None => sessions::recreate_session(args, store, ssh).await,
        }
    }

    pub async fn restore_host_sessions(
        backend: &FleetBackend,
        args: RestoreHostSessionsArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<RestoreReport, IpcError> {
        match backend.hub() {
            Some(hub) => hub.restore_host_sessions(&args).await,
            None => sessions::restore_host_sessions(args, store, ssh).await,
        }
    }

    pub async fn discover_lost_sessions(
        backend: &FleetBackend,
        args: DiscoverLostSessionsArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<Vec<LostCandidate>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.discover_lost_sessions(&args).await,
            None => sessions::discover_lost_sessions(args, store, ssh).await,
        }
    }

    pub async fn dismiss_ghost_session(
        backend: &FleetBackend,
        args: DismissGhostSessionArgs,
        store: &Mutex<Store>,
    ) -> Result<(), IpcError> {
        match backend.hub() {
            Some(hub) => hub.dismiss_ghost_session(args.session_id).await,
            None => sessions::dismiss_ghost_session(args, store),
        }
    }

    pub async fn new_bg_session(
        backend: &FleetBackend,
        args: NewBgSessionArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<bg_sessions::NewBgSessionResult, IpcError> {
        match backend.hub() {
            Some(hub) => hub.new_bg_session(&args).await,
            None => bg_sessions::new_bg_session_tracked(args, store, ssh).await,
        }
    }

    /// The `limit` clamp applies to both backends, so the hub is asked for
    /// the same window the local store would have returned.
    pub async fn session_history(
        backend: &FleetBackend,
        args: SessionHistoryArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<fleet_core::store::SessionEvent>, IpcError> {
        let limit = history_limit(args.limit);
        match backend.hub() {
            Some(hub) => hub.session_history(args.session_id, Some(limit)).await,
            None => {
                let s = lock(store)?;
                s.list_session_events(args.session_id, limit)
            }
        }
    }

    pub async fn session_conversation(
        backend: &FleetBackend,
        args: SessionConversationArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<fleet_core::service::transcript::Conversation, IpcError> {
        use fleet_core::service::transcript;
        if let Some(hub) = backend.hub() {
            // The hub owns the transcript file and the clamp; `turns` goes
            // over unclamped so `conv_limits` runs once, there.
            return hub.session_conversation(args.session_id, args.turns).await;
        }
        let row = {
            let s = lock(store)?;
            s.get_session_by_id(args.session_id)?.ok_or_else(|| {
                IpcError::new(
                    codes::E_NOTFOUND,
                    format!("session {} not found", args.session_id),
                )
            })?
        };
        // `resolve_args` takes (and releases) the lock itself; nothing holds
        // it across the fetch.
        let (turns, max_chars) = transcript::conv_limits(args.turns);
        let targs = transcript::resolve_args(store, &row, turns, max_chars)?;
        transcript::fetch_conversation(targs, ssh).await
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn history_limit_defaults_and_clamps() {
        assert_eq!(history_limit(None), HISTORY_DEFAULT_LIMIT);
        assert_eq!(history_limit(Some(0)), HISTORY_DEFAULT_LIMIT);
        assert_eq!(history_limit(Some(-3)), HISTORY_DEFAULT_LIMIT);
        assert_eq!(history_limit(Some(25)), 25);
        assert_eq!(history_limit(Some(10_000)), HISTORY_MAX_LIMIT);
    }

    #[test]
    fn session_history_args_accept_a_missing_limit() {
        let a: SessionHistoryArgs = serde_json::from_str(r#"{"session_id":7}"#).unwrap();
        assert_eq!(a.session_id, 7);
        assert!(a.limit.is_none());
    }
}
