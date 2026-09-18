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

// --- a hub that is configured but cannot be used ------------------------------
//
// The final review's F1. Every test above hands `start_background_tasks` a
// `Backend` built by hand; these go through `Backend::resolve_detail`, which
// is what `lib.rs` really calls, because the door F1 found was in the
// resolution, not in the dispatcher: a configured hub that could not be used
// resolved to `Local`, and `Local` starts all three.
//
// While `hub.remote_url` is set the operator has said "the hub owns this
// fleet". Whatever went wrong between that setting and a usable client, this
// process must not become a second brain for the same fleet — it owns nothing
// until the hub is usable again or the operator disconnects.

use crate::backend::token_store::{InMemoryTokenStore, TokenStore};
use crate::backend::{ALLOW_PLAINTEXT_KEY, REMOTE_URL_KEY};
use fleet_core::events::NoopEventBus;
use fleet_core::store::Store;
use std::sync::Arc;

fn store_with(settings: &[(&str, &str)]) -> (tempfile::TempDir, Mutex<Store>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_with_bus(&dir.path().join("state.db"), Arc::new(NoopEventBus)).unwrap();
    for (k, v) in settings {
        store.set_setting(k, v).unwrap();
    }
    (dir, Mutex::new(store))
}

/// Resolve exactly as `lib.rs` does, then start exactly as `lib.rs` does.
fn started_after_resolving(store: &Mutex<Store>, tokens: &dyn TokenStore) -> Vec<&'static str> {
    let backend = Backend::resolve(store, tokens);
    let recorder = Recorder::default();
    start_background_tasks(&backend, &recorder);
    recorder.started()
}

const NOTHING: Vec<&'static str> = Vec::new();

fn two_brains(trigger: &str) -> String {
    format!(
        "{trigger}: hub.remote_url is set, so the operator pointed this app at a \
         hub — and it started a fleet-owning background task anyway. That makes \
         it a SECOND brain reconciling the hub's fleet, which is the failure the \
         whole remote mode exists to prevent. A hub that cannot be used must \
         leave this app owning nothing, with the reason on screen."
    )
}

#[test]
fn a_configured_hub_with_no_stored_token_starts_nothing() {
    let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "https://fleet.example.com")]);
    assert_eq!(
        started_after_resolving(&store, &InMemoryTokenStore::empty()),
        NOTHING,
        "{}",
        two_brains("no client token stored")
    );
}

/// The realistic trigger: a locked macOS keychain at launch, or a keychain
/// prompt the user denied.
#[test]
fn a_configured_hub_whose_token_cannot_be_read_starts_nothing() {
    let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "https://fleet.example.com")]);
    assert_eq!(
        started_after_resolving(
            &store,
            &InMemoryTokenStore::failing("the keychain is locked")
        ),
        NOTHING,
        "{}",
        two_brains("the token store could not be read")
    );
}

#[test]
fn a_plaintext_hub_without_the_opt_in_starts_nothing() {
    let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "http://fleet.example.com")]);
    assert_eq!(
        started_after_resolving(&store, &InMemoryTokenStore::with_token("cl_live")),
        NOTHING,
        "{}",
        two_brains("plain http without hub.allow_plaintext")
    );
    // An explicit "false" is the same refusal as an absent opt-in.
    let (_dir, store) = store_with(&[
        (REMOTE_URL_KEY, "http://fleet.example.com"),
        (ALLOW_PLAINTEXT_KEY, "false"),
    ]);
    assert_eq!(
        started_after_resolving(&store, &InMemoryTokenStore::with_token("cl_live")),
        NOTHING,
        "{}",
        two_brains("plain http with hub.allow_plaintext=false")
    );
}

#[test]
fn a_configured_hub_url_that_does_not_parse_starts_nothing() {
    for url in ["not a url", "ftp://fleet.example.com", "https://"] {
        let (_dir, store) = store_with(&[(REMOTE_URL_KEY, url)]);
        assert_eq!(
            started_after_resolving(&store, &InMemoryTokenStore::with_token("cl_live")),
            NOTHING,
            "{}",
            two_brains(&format!("unusable hub.remote_url {url:?}"))
        );
    }
}

/// The settings cannot be read, but a stored client token proves this app
/// was paired. Falling back to standalone here is the same two-brains
/// failure with a less likely trigger.
#[test]
fn a_poisoned_store_in_a_paired_app_starts_nothing() {
    let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "https://fleet.example.com")]);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = store.lock().unwrap();
        panic!("poison");
    }));
    assert_eq!(
        started_after_resolving(&store, &InMemoryTokenStore::with_token("cl_live")),
        NOTHING,
        "{}",
        two_brains("the settings store is poisoned and a client token is stored")
    );
}

/// And the direction the fix must not break: with no hub configured, the
/// same path — resolve, then start — still starts all three. Blank counts
/// as absent (`hub_disconnect` writes `""`), and a token left over from an
/// old pairing does not make an unconfigured app a client.
#[test]
fn with_no_hub_configured_the_resolved_app_still_starts_all_three() {
    for settings in [
        &[][..],
        &[(REMOTE_URL_KEY, "")][..],
        &[(REMOTE_URL_KEY, "   ")][..],
    ] {
        for tokens in [
            InMemoryTokenStore::empty(),
            InMemoryTokenStore::with_token("cl_left_over"),
            InMemoryTokenStore::failing("the keychain is locked"),
        ] {
            let (_dir, store) = store_with(settings);
            assert_eq!(
                started_after_resolving(&store, &tokens),
                vec!["control_api", "reconcile_tick", "account_usage_tick"],
                "standalone behaviour must not change: settings {settings:?}"
            );
        }
    }
}
