//! Context help at a prompt line (`fleet_core::service::context_help`): a
//! question about what the person is typing in a session's shell terminal
//! or composer, answered by Haiku (`work.help_model`) on the session's host.
//!
//! The same in both modes, like the terminal it helps with: the run is this
//! machine's own ssh to the alias passed in (`pty_open`'s path), and the
//! context is the caller's or the shell's own scrollback, read over that
//! same ssh. Only a standalone desktop books the run's cost: a paired one's
//! state.db is not the fleet's.

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
        context_help::book(&store, &answer, usage.as_ref());
    }
    Ok(answer)
}
