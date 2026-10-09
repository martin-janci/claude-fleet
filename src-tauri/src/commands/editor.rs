//! Open in VS Code (Orbit Fleet redesign step 5.5): the thin handler over
//! `fleet_core::service::editor`.

use fleet_core::ipc_error::IpcError;
use fleet_core::ssh::SshClient;
use serde::Deserialize;
use std::sync::Arc;
use tauri::State;

#[derive(Debug, Deserialize)]
pub struct OpenSessionInEditorArgs {
    pub host_alias: String,
    pub tmux_name: String,
}

/// Open the session's worktree in VS Code on this machine: directly for a
/// `local` session, through Remote - SSH for any other host.
#[tauri::command]
pub async fn open_session_in_editor(
    args: OpenSessionInEditorArgs,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    // No remote-mode guard, for the same reason as `pty_open`: the editor and
    // the `ssh` that finds the folder are this machine's, addressed by the
    // alias and tmux name passed in; nothing here reads `state.db`.
    // An agent-transport host has no route for VS Code's Remote - SSH
    // (review r18): refused with the reason, not opened as a dead URI. In
    // hub-client mode this machine's `ssh` has no agent route, so such a
    // host fails the folder lookup instead, unless this machine can reach
    // it over SSH, when the editor works as `pty_open` does.
    let through_agent = ssh.routes_through_agent(&args.host_alias);
    fleet_core::service::editor::open_session_in_editor(
        ssh.inner().as_ref(),
        &args.host_alias,
        &args.tmux_name,
        through_agent,
    )
    .await
}
