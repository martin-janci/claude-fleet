//! Context help at a prompt line (`fleet_core::service::context_help`): a
//! question about what the person is typing in a session's shell terminal
//! or composer, answered by Haiku (`work.help_model`) on the session's host.
//!
//! `context_help` (the shell strip) is the same in both modes, like the
//! terminal it helps with: the run is this machine's own ssh to the alias
//! passed in (`pty_open`'s path), and the context is the shell's own
//! scrollback, read over that same ssh. Only a standalone desktop books its
//! cost: a paired one's state.db is not the fleet's.
//!
//! `session_context_help` (the composer) is routed: a paired desktop asks
//! the hub, so a session shared at `answer` or `drive` has it too.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::context_help::{self, ClaudeOnHost, HelpAnswer, HelpRequest, Surface};
use fleet_core::service::settings;
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use std::sync::{Arc, Mutex};
use tauri::State;

/// `context_help`'s arguments.
#[derive(serde::Deserialize)]
pub struct ContextHelpArgs {
    pub surface: Surface,
    /// The session's host: where the model runs, and the shell's history is
    /// read.
    pub host_alias: String,
    /// The session's tmux name, for a shell's history.
    pub session_name: String,
    /// The shell terminal asked from (`Surface::Shell`); its scrollback is
    /// the history. Absent, or unreadable: `request.history` stands in.
    #[serde(default)]
    pub terminal: Option<u32>,
    /// The session's credential profile (`claude_profile`).
    #[serde(default)]
    pub profile: Option<String>,
    /// `work.help_model`, as the window read it; Haiku when absent.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(flatten)]
    pub request: HelpRequest,
}

/// Ask about the prompt line, with its history as context. Nothing is run
/// on the person's behalf: the answer's `command` is for the prompt line.
///
/// Errors: `E_INVALID` (nothing to ask about, a bad host, model or
/// profile), `E_CLAUDE_CLI` (no `claude`, a failed or signed-out run),
/// `E_TIMEOUT`, transport codes.
#[tauri::command]
pub async fn context_help(
    args: ContextHelpArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<HelpAnswer, IpcError> {
    // Writing help's "Context help" toggle. A paired desktop's state.db is
    // not the fleet's, so there the window's own check (the hub's setting)
    // is the gate.
    if backend.hub().is_none() {
        let s = fleet_core::ipc_error::lock(&store)?;
        settings::require_writing_help(&s, settings::WORK_CONTEXT_HELP)?;
    }
    let exec: &dyn fleet_core::ssh::SshExec = ssh.inner().as_ref();
    let scrollback = match (args.surface, args.terminal) {
        (Surface::Shell, Some(n)) => {
            match context_help::shell_history(exec, &args.host_alias, &args.session_name, n).await {
                Ok(text) => text,
                Err(e) => {
                    tracing::warn!(error = %e.message, "[context_help] shell history unread");
                    None
                }
            }
        }
        _ => None,
    };
    let model = ClaudeOnHost {
        exec,
        host: args.host_alias.clone(),
        profile: args.profile.filter(|p| !p.is_empty()),
        model: args.model.unwrap_or_else(|| "haiku".into()),
    };
    let (answer, usage) =
        context_help::ask(&model, args.surface, &args.request, scrollback.as_deref()).await?;
    if backend.hub().is_none() {
        context_help::book(&store, &answer, usage.as_ref(), None);
    }
    Ok(answer)
}

/// `session_context_help`'s arguments: `SessionContextHelpParams` field
/// for field.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SessionContextHelpArgs {
    pub session_id: i64,
    #[serde(flatten)]
    pub request: HelpRequest,
}

/// Context help at a session's composer, owned or shared at `answer` /
/// `drive`: on the session's host under the profile it was launched with,
/// on `work.help_model`. A paired desktop asks the hub
/// (`session_context_help`), which checks the grant, the setting and books
/// the run; a standalone one owns every row and runs it here.
///
/// Errors: `E_INVALID_STATE` (Context help off), `E_FORBIDDEN` (a watch
/// grant), `E_NOTFOUND`, `E_INVALID`, `E_CLAUDE_CLI`, `E_TIMEOUT`, transport codes.
#[tauri::command]
pub async fn session_context_help(
    args: SessionContextHelpArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<HelpAnswer, IpcError> {
    routed::session_context_help(&backend, args, &store, &ssh).await
}

pub mod routed {
    use super::*;
    use fleet_core::ipc_error::codes;

    pub async fn session_context_help(
        backend: &FleetBackend,
        args: SessionContextHelpArgs,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
    ) -> Result<HelpAnswer, IpcError> {
        if let Some(hub) = backend.hub() {
            return hub.route("session_context_help", &args).await;
        }
        let row = {
            let s = fleet_core::ipc_error::lock(store)?;
            s.get_session_by_id(args.session_id)?.ok_or_else(|| {
                IpcError::new(
                    codes::E_NOTFOUND,
                    format!("session {} not found", args.session_id),
                )
            })?
        };
        context_help::ask_for_session(store, ssh.as_ref(), &row, &args.request).await
    }
}
