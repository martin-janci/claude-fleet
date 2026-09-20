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
//!    with a message naming what to do instead. See
//!    [`FleetBackend::refuse_local_only`].
//! 3. **Same in both modes** — it is about *this process*, so the local answer
//!    is the right answer either way: `collect_diagnostics`, `open_log_folder`,
//!    `cancel_command`. These carry their reason with them, so that "I forgot"
//!    and "I decided" cannot look alike.
//!
//! Which one each command gets is written down once, in
//! [`VERDICTS`](super::verdicts::VERDICTS), and every other place that needs
//! to know — the refusal sentences, the tests, the frontend and the docs —
//! reads it from there.
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
//! `new_session` set the rule. `NewSessionArgs` used to carry `kind`,
//! `start_command` and `friendly_name` that `NewSessionParams` carried none
//! of, and a shell session was a different tool (`new_shell_session`):
//! routing it would not have failed — it would have **SUCCEEDED**, created a
//! session, and silently dropped the label the user typed. Task 1 (#146)
//! closed that gap by adding the three fields to the tool's params (optional
//! — absent means today's MCP behaviour), so `new_session` now routes
//! unconditionally. `repair_session` is still partly refused, and for the
//! same shape of reason: the tool always runs the EXPLICIT repair, and the
//! desktop's automatic pre-attach check (`explicit: false`) has no
//! counterpart, so routing it would quietly mean something else — only
//! `explicit: true` (the Repair workspace button) routes.
//!
//! So: do not "fix" a refusal like this by wiring a lossy mapping. If the
//! tool grows the missing parameters, route it then — and check the rest of
//! the struct again while you are there. The rule is also in `docs/hub.md`
//! (*Parity or refusal*), because the refusal is user-visible.

use super::remote::{HubBackend, HubTransport};
use super::verdicts::{self, Verdict};
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
            // NOT `local()`: that is the standalone arm of every command,
            // which would manage the hub's fleet from here. See
            // `HubBackend::unavailable`.
            Backend::Unavailable(hub) => Self {
                hub: Some(HubBackend::unavailable(hub.clone())),
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

    /// Refuse a command that has no hub counterpart, naming what to do
    /// instead — with the sentence its own row in
    /// [`VERDICTS`](super::verdicts::VERDICTS) carries.
    ///
    /// The sentence is a fragment completing "…; " — it must tell the user
    /// where the operation *does* work, because the honest answer is never
    /// "you cannot do this", it is "not from here". A no-op in standalone
    /// mode, so the guard costs a paired app one branch and costs a
    /// standalone app nothing.
    ///
    /// **It fails closed.** A `command` with no row, or with a row that has
    /// no sentence, is a bug — [`every_refusal_names_a_command_the_table_can_refuse`]
    /// makes shipping one impossible — but if one ever got out, the user must
    /// still be refused rather than silently allowed to mutate a fleet the
    /// hub also manages. So the miss is a `debug_assert!` (loud in a debug
    /// build and in every test) and, in release, an `E_LOCAL_ONLY` whose
    /// sentence says the build is at fault. Never a panic on a user path.
    ///
    /// [`every_refusal_names_a_command_the_table_can_refuse`]: super::tests_routing
    pub fn refuse_local_only(&self, command: &str) -> Result<(), IpcError> {
        let instead = verdicts::verdict(command).and_then(Verdict::instead);
        debug_assert!(
            instead.is_some(),
            "{command} refuses with E_LOCAL_ONLY but VERDICTS has no sentence for it"
        );
        self.local_only(command, instead.unwrap_or(NO_SENTENCE))
    }

    /// The formatting half, and the one place the "configured but unavailable"
    /// precedence lives. Private on purpose: a command refuses by name,
    /// through [`Self::refuse_local_only`], so that the sentence it refuses
    /// with is the one in [`VERDICTS`](super::verdicts::VERDICTS) and nowhere
    /// else. Pasting a sentence at a call site is now a compile error rather
    /// than a habit.
    fn local_only(&self, what: &str, instead: &str) -> Result<(), IpcError> {
        match self.hub() {
            None => Ok(()),
            // A configured hub this launch cannot use: "do it on the hub" is
            // not the problem, the hub is, so say that instead.
            Some(hub) => Err(hub.unavailable_error(what).unwrap_or_else(|| {
                IpcError::new(
                    codes::E_LOCAL_ONLY,
                    format!(
                        "{what} is not available while this desktop is a window onto \
                         {}; {instead}",
                        hub.config().base_url
                    ),
                )
            })),
        }
    }
}

/// The fail-closed sentence of [`FleetBackend::refuse_local_only`]. Nobody
/// should ever read it; if somebody does, it says whose fault that is.
const NO_SENTENCE: &str = "this build has no reason recorded for the refusal, which is a bug in \
     the app; do it on the hub, or from a standalone app";

#[cfg(test)]
#[path = "tests_routing.rs"]
mod tests;
