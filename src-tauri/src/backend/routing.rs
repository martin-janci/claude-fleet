//! What a Tauri command does with the resolved [`Backend`].
//!
//! [`Backend`] answers *which fleet this app is a window onto*; it is resolved
//! once at startup and is a plain value. [`FleetBackend`] is the live thing
//! the commands hold: the same decision, plus the [`HubBackend`] to call when
//! the answer is "someone else's fleet". It is managed state, so a command
//! reaches it the same way it reaches the `Store` — through a `State<'_, …>`
//! parameter, which is injected by Tauri and is invisible to the frontend.
//! No command's IPC contract changes.
//!
//! # The three verdicts
//!
//! Every command in `generate_handler!` is one of:
//!
//! 1. **Routed** — it has an MCP tool counterpart, so in remote mode it calls
//!    the hub and returns the deserialised value unchanged. `list_sessions`,
//!    the eight repo-browsing reads, `send_prompt`, `kill_session`, …
//! 2. **Local-only** — it has no counterpart and must *not* quietly run
//!    against this machine. It returns [`codes::E_LOCAL_ONLY`] in remote mode
//!    with a message naming what to do instead. See [`FleetBackend::local_only`].
//! 3. **Same in both modes** — it is about *this process*, so the local answer
//!    is the right answer either way: `collect_diagnostics`, `open_log_folder`,
//!    `cancel_command`. These are listed explicitly in
//!    [`tests_routing`](super::tests_routing) so that "I forgot" and "I decided"
//!    cannot look alike.
//!
//! The dangerous verdict is the one nobody makes. A mutation left on the local
//! path in remote mode does not fail — it SSHes into a host with this
//! machine's keys and changes a fleet the hub also manages, which is the exact
//! double-brain failure the whole mode exists to prevent. A test enumerates
//! the handler list and fails on any command that has no verdict.
//!
//! # PARITY OR REFUSAL
//!
//! **A mutation routes only where the desktop's arguments map one-to-one onto
//! the tool's parameters**, checked field by field against the source. Where
//! they do not, the command is local-only. This is a rule, not a backlog.
//!
//! `new_session` is why it exists. `NewSessionArgs` carries `kind`,
//! `start_command` and `friendly_name`; `NewSessionParams` carries none of the
//! three, and a shell session is a different tool (`new_shell_session`).
//! Routing it would not have failed — it would have **SUCCEEDED**, created a
//! session, and silently dropped the label the user typed. A refusal is
//! visible; a dropped field is not. `repair_session` is the same shape: the
//! tool always runs the EXPLICIT repair, and the desktop's automatic
//! pre-attach check has no counterpart, so a routed call would quietly mean
//! something else.
//!
//! So: do not "fix" one of these refusals by wiring a lossy mapping. If the
//! tool grows the missing parameters, route it then — and check the rest of
//! the struct again while you are there. The rule is also in `docs/hub.md`
//! (*Parity or refusal*), because the refusal is user-visible.

use super::remote::{HubBackend, HubTransport};
use super::{Backend, RemoteConfig};
use fleet_core::ipc_error::{codes, IpcError};
use std::sync::Arc;

/// The backend as the commands see it.
pub struct FleetBackend {
    /// `None` is standalone. `Some` is a window onto that hub.
    hub: Option<HubBackend>,
}

impl std::fmt::Debug for FleetBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `HubBackend`'s own `Debug` redacts the token.
        f.debug_struct("FleetBackend")
            .field("hub", &self.hub)
            .finish()
    }
}

impl FleetBackend {
    /// Standalone: this app owns its fleet.
    pub fn local() -> Self {
        Self { hub: None }
    }

    /// Build the live backend from the startup decision.
    pub fn from_resolved(backend: &Backend) -> Self {
        match backend {
            Backend::Local => Self::local(),
            Backend::Remote(cfg) => Self {
                hub: Some(HubBackend::new(cfg.clone())),
            },
        }
    }

    /// A hub client over a caller-supplied transport — the seam the routing
    /// tests use to answer with a recorded response instead of a network.
    pub fn remote_over(cfg: RemoteConfig, transport: Arc<dyn HubTransport>) -> Self {
        Self {
            hub: Some(HubBackend::with_transport(cfg, transport)),
        }
    }

    /// The hub to call, or `None` when this app runs its own fleet.
    ///
    /// The whole routing idiom is `match backend.hub() { Some(h) => …, None =>
    /// <today's service call> }`, so the standalone arm is literally the code
    /// that was there before.
    pub fn hub(&self) -> Option<&HubBackend> {
        self.hub.as_ref()
    }

    pub fn is_remote(&self) -> bool {
        self.hub.is_some()
    }

    /// Refuse a command that has no hub counterpart, naming what to do instead.
    ///
    /// `instead` is a sentence fragment completing "…; " — it must tell the
    /// user where the operation *does* work, because the honest answer is
    /// never "you cannot do this", it is "not from here". A no-op in
    /// standalone mode, so the guard costs a paired app one branch and costs
    /// a standalone app nothing.
    pub fn local_only(&self, what: &str, instead: &str) -> Result<(), IpcError> {
        match self.hub() {
            None => Ok(()),
            Some(hub) => Err(IpcError::new(
                codes::E_LOCAL_ONLY,
                format!(
                    "{what} is not available while this desktop is a window onto \
                     {}; {instead}",
                    hub.config().base_url
                ),
            )),
        }
    }
}

#[cfg(test)]
#[path = "tests_routing.rs"]
mod tests;
