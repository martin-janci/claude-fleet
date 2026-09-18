//! Is the absence of the three background tasks actually observable?
//!
//! The Task 2 review's finding 4, in test form. See [`super`] for why the
//! previous guard was a tautology.

use super::*;
use crate::backend::RemoteConfig;
use std::sync::Mutex;

/// Records what it was asked to start, in order.
#[derive(Default)]
struct Recorder(Mutex<Vec<&'static str>>);

impl Recorder {
    fn started(&self) -> Vec<&'static str> {
        self.0.lock().unwrap().clone()
    }
}

impl FleetTasks for Recorder {
    fn start_control_api(&self) {
        self.0.lock().unwrap().push("control_api");
    }
    fn start_reconcile_tick(&self) {
        self.0.lock().unwrap().push("reconcile_tick");
    }
    fn start_account_usage_tick(&self) {
        self.0.lock().unwrap().push("account_usage_tick");
    }
    fn start_event_bridge(&self) {
        self.0.lock().unwrap().push("event_bridge");
    }
}

fn remote() -> Backend {
    Backend::Remote(RemoteConfig {
        base_url: "https://fleet.example.com".into(),
        token: "cl_s3cret-token".into(),
        client_name: "laptop".into(),
    })
}

/// **The safety property of the whole remote mode**, asserted against a call
/// that was not made rather than against a predicate's own definition.
///
/// If this ever goes green with a non-empty list, two processes are
/// reconciling one fleet: two sets of hooks fighting over which URL a host
/// reports to, and two databases drifting apart.
#[test]
fn a_hub_client_starts_none_of_the_three() {
    let recorder = Recorder::default();
    start_background_tasks(&remote(), &recorder);
    assert_eq!(
        recorder.started(),
        vec!["event_bridge"],
        "a desktop pointed at a hub started a fleet-owning background task. \
         Two processes reconciling one fleet is the failure this whole mode \
         exists to prevent — and unlike most bugs it is silent, because both \
         halves appear to work. The event bridge is the one task a client DOES \
         run: it only reads the hub's stream."
    );
}

/// The other half: standalone behaviour must not change. A guard that
/// stopped a standalone app ticking would pass the test above and break the
/// app, so both directions are pinned.
#[test]
fn a_standalone_app_starts_all_three() {
    let recorder = Recorder::default();
    start_background_tasks(&Backend::Local, &recorder);
    assert_eq!(
        recorder.started(),
        vec!["control_api", "reconcile_tick", "account_usage_tick"],
        "standalone must keep its control API, its reconcile tick and its \
         usage poll — and must NOT start the hub event bridge, because there \
         is no hub and its own event bus already drives the stores"
    );
}

/// The reviewer's refactor, caught.
///
/// [`a_hub_client_starts_none_of_the_three`] covers a call hoisted above the
/// guard *inside* `start_background_tasks`. It cannot cover what the reviewer
/// actually did, which was to call `spawn_reconcile_tick` straight from
/// `lib.rs` — code this module never sees.
///
/// So: `lib.rs` may not name any of the three. The real calls live in
/// `bootstrap::tasks`, reached only through [`start_background_tasks`]. This
/// is a string match rather than a type-level guarantee, which would mean
/// threading a witness token through `fleet-core`'s public API and
/// `fleet-hub` with it — a bigger change than this fix round, and noted in
/// the report as the stronger option still on the table.
#[test]
fn lib_rs_cannot_start_a_background_task_behind_this_modules_back() {
    let lib = include_str!("../lib.rs");
    for forbidden in [
        "spawn_reconcile_tick(",
        "spawn_account_usage_tick(",
        "maybe_start_mcp(",
        "spawn_event_bridge(",
    ] {
        assert!(
            !lib.contains(forbidden),
            "lib.rs calls `{forbidden}` directly. That bypasses \
             `backend::startup::start_background_tasks`, which is the only \
             place allowed to decide whether this process owns its fleet — \
             and a call in `lib.rs`'s setup closure is unreachable from every \
             test, because it needs a live tauri::App. Move it into \
             `bootstrap::tasks::RealFleetTasks`."
        );
    }
    // The dispatcher really is called, so the check above cannot pass
    // vacuously by nothing starting anything at all.
    assert!(
        lib.contains("start_background_tasks("),
        "lib.rs no longer calls start_background_tasks, so nothing starts the \
         reconcile tick at all"
    );
}

/// And the real implementation holds each of them exactly once — so a second
/// reconcile tick cannot be added beside the first inside the one file that is
/// allowed to spawn them.
#[test]
fn the_real_tasks_module_spawns_each_of_the_three_exactly_once() {
    let tasks = include_str!("../bootstrap/tasks.rs");
    for (call, what) in [
        ("spawn_reconcile_tick(", "the reconcile tick"),
        ("spawn_account_usage_tick(", "the account-usage poll"),
        ("maybe_start_mcp(", "the embedded control API"),
        ("spawn_event_bridge(", "the hub event bridge"),
    ] {
        assert_eq!(
            tasks.matches(call).count(),
            1,
            "{what} is started {} time(s) in bootstrap/tasks.rs; it must be \
             exactly once",
            tasks.matches(call).count()
        );
    }
}
