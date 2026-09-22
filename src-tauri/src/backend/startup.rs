//! The one place the three fleet-owning background tasks are started.
//!
//! # Why this module exists rather than an `if` in `lib.rs`
//!
//! A desktop pointed at a hub must start **none** of the reconcile tick, the
//! account-usage poll or the embedded control API. Two processes reconciling
//! one fleet means two sets of hooks fighting over which URL a host reports
//! to, and two databases drifting apart. It is the failure the whole remote
//! mode exists to prevent.
//!
//! Until this module, that rule lived as an `if backend.owns_the_fleet()` in
//! `lib.rs`'s setup closure, guarded by a test asserting
//! `matches!(self, Backend::Local)`. That guard is a tautology in disguise:
//! hoisting `spawn_reconcile_tick` out of the `else` so **both** modes
//! started it left all 91 tests passing. The test observed the predicate's
//! *definition* — not its *use*, which is the only thing that could go
//! wrong.
//!
//! So the decision moved out of `lib.rs`'s setup closure, which no test can
//! run (it needs a live `tauri::App`), into [`start_background_tasks`], which
//! is an ordinary function over a trait. Two tests then hold it down from
//! different directions:
//!
//! - [`tests::a_hub_client_starts_none_of_the_three`] drives this function
//!   with a recording double and asserts nothing was started in remote mode.
//!   Hoisting a call above the guard *inside* this function fails it. The
//!   `..._starts_nothing` tests do the same for a hub that is configured but
//!   cannot be used, one per way that can happen, and they go through
//!   `Backend::resolve` first — the final review found the door there, not
//!   here.
//! - [`tests::lib_rs_cannot_start_a_background_task_behind_this_modules_back`]
//!   asserts `lib.rs` contains no direct call to any of the three. Doing what
//!   the reviewer did — calling `spawn_reconcile_tick` in `lib.rs` — fails
//!   that one, which is the exact refactor nothing caught before.
//!
//! Neither is a tautology: both observe a call that was or was not made.

use super::Backend;

/// The three background tasks that make a process the brain of its fleet.
///
/// A trait so that [`start_background_tasks`] can be driven by a recorder in
/// a test. The real implementation is `bootstrap::tasks::RealFleetTasks`,
/// which is where the actual `spawn_*` calls live — deliberately not in
/// `lib.rs`, so that "did `lib.rs` start something itself?" is a question with
/// a mechanical answer.
pub trait FleetTasks {
    /// The embedded MCP control API, if the user enabled it.
    fn start_control_api(&self);
    /// The proactive reconcile tick.
    fn start_reconcile_tick(&self);
    /// The 60s account-usage poll.
    fn start_account_usage_tick(&self);
    /// The hub's event stream, re-emitted as the frontend's own row events.
    /// The mirror image of the three above: it is the ONLY background task a
    /// hub client runs, and a standalone app must not run it — there is no hub
    /// to subscribe to, and its own event bus already drives the stores.
    fn start_event_bridge(&self);
    /// The error-report flusher: a hub client's only other background task;
    /// off with `CLAUDE_FLEET_HUB_REPORTS=0`.
    fn start_report_flusher(&self);
}

/// `CLAUDE_FLEET_HUB_REPORTS`: unset or anything but `0`/`false` means on.
pub fn report_flusher_wanted(env: Option<&str>) -> bool {
    !env.is_some_and(|v| v == "0" || v.eq_ignore_ascii_case("false"))
}

/// Start exactly the background tasks this backend is entitled to run.
///
/// Standalone: the three fleet-owning ones, and not the event bridge — there
/// is no hub to subscribe to. Pointed at a hub: **only** the event bridge.
/// Pointed at a hub this launch cannot use: **nothing at all** — there is no
/// stream to follow, and the fleet is still the hub's.
///
/// A `match` rather than an `if`, so that a fourth kind of backend cannot be
/// added without someone deciding here what it may start.
pub fn start_background_tasks(backend: &Backend, tasks: &dyn FleetTasks) {
    match backend {
        Backend::Remote(cfg) => {
            // Deliberately no token in this line. `base_url` carries no
            // userinfo either — `normalise_base_url` strips it, because this
            // line is logged and `collect_diagnostics` ships the log tail to
            // support.
            tracing::info!(
                hub = %cfg.base_url,
                client = %cfg.client_name,
                "remote backend: skipping the reconcile tick, the account-usage \
                 poll and the embedded control API — the hub owns this fleet; \
                 following its event stream instead"
            );
            // The one thing a client DOES start. Without it a hub-client
            // desktop renders whatever it listed at startup and then never
            // changes: the local event bus has nothing to emit, because
            // nothing local mutates.
            tasks.start_event_bridge();
            if report_flusher_wanted(std::env::var("CLAUDE_FLEET_HUB_REPORTS").ok().as_deref()) {
                tasks.start_report_flusher();
            } else {
                tracing::info!("CLAUDE_FLEET_HUB_REPORTS is off: not reporting errors to the hub");
            }
        }
        Backend::Unavailable(hub) => {
            // The final review's F1. This used to be `Local`, which starts
            // all three below: an app the operator had pointed at a hub
            // became a second brain for it whenever the keychain was locked
            // at launch.
            tracing::warn!(
                "hub unavailable: starting no reconcile tick, no account-usage \
                 poll, no embedded control API and no event stream — {}",
                hub.explain()
            );
        }
        Backend::Local => {
            tracing::info!(
                "standalone backend: local database and SSH; starting the \
                 reconcile tick and the account-usage poll"
            );
            tasks.start_control_api();
            tasks.start_reconcile_tick();
            tasks.start_account_usage_tick();
        }
    }
}

#[cfg(test)]
#[path = "tests_startup.rs"]
mod tests;
