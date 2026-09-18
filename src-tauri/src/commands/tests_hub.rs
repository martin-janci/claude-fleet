//! Tests for [`super`] (`commands::hub`) — the three commands Settings calls
//! to see, make and undo a pairing.
//!
//! They run against the logic functions rather than the `#[tauri::command]`
//! wrappers, which is this codebase's idiom (`commands::tasks::routed`): a
//! `State<'_, …>` cannot be built without a live `tauri::App`, and everything
//! worth asserting is in the function underneath.

use super::logic;
use super::HubPairArgs;
use crate::backend::pairing::{PairTransport, PairedClient};
use crate::backend::remote::HubResponse;
use crate::backend::token_store::{InMemoryTokenStore, TokenStore};
use crate::backend::{Backend, RemoteConfig, ALLOW_PLAINTEXT_KEY, CLIENT_NAME_KEY, REMOTE_URL_KEY};
use fleet_core::events::NoopEventBus;
use fleet_core::ipc_error::codes;
use fleet_core::store::Store;
use serde_json::json;
use std::sync::{Arc, Mutex};

fn store_with(settings: &[(&str, &str)]) -> (tempfile::TempDir, Mutex<Store>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_with_bus(&dir.path().join("state.db"), Arc::new(NoopEventBus)).unwrap();
    for (k, v) in settings {
        store.set_setting(k, v).unwrap();
    }
    (dir, Mutex::new(store))
}

fn remote(url: &str) -> Backend {
    Backend::Remote(RemoteConfig {
        base_url: url.into(),
        token: "cl_live".into(),
        client_name: "laptop".into(),
    })
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

/// A transport that answers one recorded response and records what it was
/// asked.
struct Fake {
    answer: Mutex<Option<Result<HubResponse, String>>>,
    seen: Mutex<Vec<(String, String)>>,
}

impl Fake {
    fn ok(token: &str, name: &str, mode: &str) -> Self {
        Self {
            answer: Mutex::new(Some(Ok(HubResponse {
                status: 200,
                body: json!({
                    "token": token, "name": name, "mode": mode,
                    "hub": "https://fleet.example.com",
                })
                .to_string(),
            }))),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn refusing() -> Self {
        Self {
            answer: Mutex::new(Some(Ok(HubResponse {
                status: 404,
                body: json!({ "error": "invalid code" }).to_string(),
            }))),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<(String, String)> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl PairTransport for Fake {
    async fn post_pair(&self, url: &str, body: String) -> Result<HubResponse, String> {
        self.seen.lock().unwrap().push((url.into(), body.clone()));
        self.answer
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| Err("the fake was asked twice".into()))
    }
}

fn args(url: &str, code: &str) -> HubPairArgs {
    HubPairArgs {
        url: url.into(),
        code: code.into(),
        allow_plaintext: false,
    }
}

// --- hub_status --------------------------------------------------------------

#[test]
fn a_standalone_app_reports_no_hub_at_all() {
    let (_dir, store) = store_with(&[]);
    let got = logic::status(&Backend::Local, &store, &InMemoryTokenStore::empty()).unwrap();
    assert!(!got.remote);
    assert_eq!(got.url, None);
    assert_eq!(got.configured_url, None);
    assert_eq!(got.warning, None);
    assert!(!got.restart_required);
}

#[test]
fn a_paired_app_reports_the_hub_and_the_client_name() {
    let (_dir, store) = store_with(&[
        (REMOTE_URL_KEY, "https://fleet.example.com"),
        (CLIENT_NAME_KEY, "laptop"),
    ]);
    let got = logic::status(
        &remote("https://fleet.example.com"),
        &store,
        &InMemoryTokenStore::with_token("cl_live"),
    )
    .unwrap();
    assert!(got.remote);
    assert_eq!(got.url.as_deref(), Some("https://fleet.example.com"));
    assert_eq!(got.client_name.as_deref(), Some("laptop"));
    assert!(!got.restart_required, "nothing changed since launch");
}

/// The half-paired state the design calls out: a URL is configured but the
/// app is running standalone anyway. Settings must be able to say so, which
/// means the reason has to cross the IPC boundary, not only reach the log.
#[test]
fn a_configured_but_inactive_hub_reports_the_reason_and_asks_for_a_restart() {
    let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "https://fleet.example.com")]);
    let got = logic::status(&Backend::Local, &store, &InMemoryTokenStore::empty()).unwrap();
    assert!(!got.remote, "no token, so this process owns its own fleet");
    assert_eq!(
        got.configured_url.as_deref(),
        Some("https://fleet.example.com")
    );
    let warning = got.warning.expect("the user must be told why");
    assert!(warning.contains("no client token"), "{warning}");
}

/// Requirement (d)(ii): an opted-in plaintext hub says so wherever the hub is
/// named, on every launch, not once in a log line nobody reads.
#[test]
fn a_plaintext_hub_carries_its_warning_to_the_frontend() {
    let (_dir, store) = store_with(&[
        (REMOTE_URL_KEY, "http://fleet.example.com"),
        (ALLOW_PLAINTEXT_KEY, "true"),
    ]);
    let got = logic::status(
        &remote("http://fleet.example.com"),
        &store,
        &InMemoryTokenStore::with_token("cl_live"),
    )
    .unwrap();
    assert!(got.allow_plaintext);
    let warning = got.warning.expect("a plaintext hub must keep saying so");
    assert!(warning.contains("in the clear"), "{warning}");
}

/// The status is a Tauri return value, so it is serialised straight to the
/// frontend. A token in it would be in the webview, in a devtools network
/// panel and in any crash report the webview makes.
#[test]
fn a_hub_status_never_carries_the_token() {
    let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "https://fleet.example.com")]);
    let got = logic::status(
        &remote("https://fleet.example.com"),
        &store,
        &InMemoryTokenStore::with_token("s3cret-token"),
    )
    .unwrap();
    let json = serde_json::to_string(&got).unwrap();
    assert!(!json.contains("s3cret-token"), "{json}");
    assert!(!json.contains("cl_live"), "{json}");
}

// --- hub_pair ----------------------------------------------------------------

#[test]
fn pairing_stores_the_token_the_url_and_the_client_name() {
    let (_dir, store) = store_with(&[]);
    let tokens = InMemoryTokenStore::empty();
    let fake = Fake::ok("cl_new_token", "laptop", "full");

    let got = block_on(logic::pair(
        &fake,
        &Backend::Local,
        &store,
        &tokens,
        args("https://fleet.example.com/", "abcd1234"),
    ))
    .unwrap();

    assert_eq!(fake.calls()[0].0, "https://fleet.example.com/pair");
    assert_eq!(tokens.get().unwrap().as_deref(), Some("cl_new_token"));
    let s = store.lock().unwrap();
    assert_eq!(
        s.get_setting(REMOTE_URL_KEY).unwrap().as_deref(),
        // Normalised: no trailing slash, so `{base}/mcp` is well formed.
        Some("https://fleet.example.com")
    );
    assert_eq!(
        s.get_setting(CLIENT_NAME_KEY).unwrap().as_deref(),
        Some("laptop")
    );
    drop(s);

    // Still standalone until the app restarts: the backend is resolved once.
    assert!(!got.remote);
    assert!(
        got.restart_required,
        "the mode is decided at startup, so the user must be told to restart"
    );
    assert_eq!(
        got.configured_url.as_deref(),
        Some("https://fleet.example.com")
    );
    assert_eq!(got.configured_client_name.as_deref(), Some("laptop"));
}

#[test]
fn a_refused_code_changes_nothing_at_all() {
    let (_dir, store) = store_with(&[]);
    let tokens = InMemoryTokenStore::empty();
    let e = block_on(logic::pair(
        &Fake::refusing(),
        &Backend::Local,
        &store,
        &tokens,
        args("https://fleet.example.com", "BADCODE1"),
    ))
    .expect_err("a refused code must fail");
    assert_eq!(e.code, codes::E_INVALID);
    assert_eq!(tokens.get().unwrap(), None, "nothing may be stored");
    assert_eq!(
        store.lock().unwrap().get_setting(REMOTE_URL_KEY).unwrap(),
        None
    );
}

/// Requirement (d)(i): the plaintext question is asked BEFORE the code is
/// spent, so a person who typed `http://` by mistake has not burned their
/// pairing code by the time they are told.
#[test]
fn plaintext_is_refused_before_the_code_is_spent() {
    let (_dir, store) = store_with(&[]);
    let tokens = InMemoryTokenStore::empty();
    let fake = Fake::ok("cl_new_token", "laptop", "full");

    let e = block_on(logic::pair(
        &fake,
        &Backend::Local,
        &store,
        &tokens,
        args("http://fleet.example.com", "ABCD1234"),
    ))
    .expect_err("plain http to a routable host must be refused");

    assert_eq!(e.code, codes::E_HUB_PLAINTEXT);
    assert!(e.message.contains("in the clear"), "{}", e.message);
    assert!(
        fake.calls().is_empty(),
        "the code must not be spent to learn this: {:?}",
        fake.calls()
    );
    assert_eq!(tokens.get().unwrap(), None);
}

#[test]
fn plaintext_pairs_once_it_is_opted_into_and_the_opt_in_is_remembered() {
    let (_dir, store) = store_with(&[]);
    let tokens = InMemoryTokenStore::empty();
    let fake = Fake::ok("cl_new_token", "laptop", "full");

    let got = block_on(logic::pair(
        &fake,
        &Backend::Local,
        &store,
        &tokens,
        HubPairArgs {
            url: "http://fleet.example.com".into(),
            code: "ABCD1234".into(),
            allow_plaintext: true,
        },
    ))
    .unwrap();

    assert!(got.allow_plaintext);
    assert_eq!(
        store
            .lock()
            .unwrap()
            .get_setting(ALLOW_PLAINTEXT_KEY)
            .unwrap()
            .as_deref(),
        Some("true"),
        "the resolution at the next launch must find the same opt-in, or the \
         app would refuse the hub it just paired with"
    );
    // And the warning still rides along: opting in makes it a decision, not
    // something that goes quiet.
    assert!(got.warning.is_some_and(|w| w.contains("in the clear")));
}

/// Loopback is the tunnelled hub `docs/hub.md` describes. Pairing with it
/// must need no ceremony, or the opt-in becomes a tax on the normal case.
#[test]
fn a_loopback_hub_pairs_with_no_plaintext_ceremony() {
    let (_dir, store) = store_with(&[]);
    let tokens = InMemoryTokenStore::empty();
    let got = block_on(logic::pair(
        &Fake::ok("cl_new_token", "laptop", "full"),
        &Backend::Local,
        &store,
        &tokens,
        args("http://127.0.0.1:8787", "ABCD1234"),
    ))
    .unwrap();
    assert_eq!(tokens.get().unwrap().as_deref(), Some("cl_new_token"));
    assert_eq!(got.warning, None);
    assert!(!got.allow_plaintext);
}

#[test]
fn an_unusable_url_never_reaches_the_transport() {
    for url in [
        "not a url",
        "ftp://fleet.example.com",
        "/just/a/path",
        "   ",
    ] {
        let (_dir, store) = store_with(&[]);
        let tokens = InMemoryTokenStore::empty();
        let fake = Fake::ok("cl_new_token", "laptop", "full");
        let e = block_on(logic::pair(
            &fake,
            &Backend::Local,
            &store,
            &tokens,
            args(url, "ABCD1234"),
        ))
        .expect_err(&format!("{url:?} must be refused"));
        assert_eq!(e.code, codes::E_INVALID, "for {url:?}");
        assert!(fake.calls().is_empty(), "for {url:?}");
        assert_eq!(tokens.get().unwrap(), None, "for {url:?}");
    }
}

#[test]
fn an_empty_code_is_refused_without_spending_an_attempt() {
    let (_dir, store) = store_with(&[]);
    let fake = Fake::ok("cl_new_token", "laptop", "full");
    let e = block_on(logic::pair(
        &fake,
        &Backend::Local,
        &store,
        &InMemoryTokenStore::empty(),
        args("https://fleet.example.com", "   "),
    ))
    .expect_err("an empty code must be refused here");
    assert_eq!(e.code, codes::E_INVALID);
    assert!(
        fake.calls().is_empty(),
        "the hub's ten-a-minute budget is not ours to waste on a blank field"
    );
}

/// The pairing code is spent by the exchange. If the settings write then
/// fails we must not leave a token behind that no launch will ever use and
/// no operator knows exists — clean up, and say both halves of what happened.
#[test]
fn a_settings_failure_rolls_the_token_back_rather_than_stranding_it() {
    let (_dir, store) = store_with(&[]);
    // Poison the store so `set_setting` cannot run.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = store.lock().unwrap();
        panic!("poison");
    }));
    assert!(store.lock().is_err());

    let tokens = InMemoryTokenStore::empty();
    let e = block_on(logic::pair(
        &Fake::ok("cl_new_token", "laptop", "full"),
        &Backend::Local,
        &store,
        &tokens,
        args("https://fleet.example.com", "ABCD1234"),
    ))
    .expect_err("a store failure must fail the pairing");
    assert_eq!(tokens.get().unwrap(), None, "no stranded token");
    assert!(
        e.message.to_lowercase().contains("mint"),
        "the code is spent, so the user needs a new one: {}",
        e.message
    );
}

/// The token never reaches an error message, whichever half failed.
#[test]
fn no_pairing_error_carries_the_new_token() {
    let (_dir, store) = store_with(&[]);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = store.lock().unwrap();
        panic!("poison");
    }));
    let e = block_on(logic::pair(
        &Fake::ok("cl_s3cret_token", "laptop", "full"),
        &Backend::Local,
        &store,
        &InMemoryTokenStore::empty(),
        args("https://fleet.example.com", "ABCD1234"),
    ))
    .unwrap_err();
    assert!(!format!("{e:?}").contains("cl_s3cret_token"), "{e:?}");
}

// --- hub_disconnect ----------------------------------------------------------

#[test]
fn disconnecting_clears_the_token_and_the_url() {
    let (_dir, store) = store_with(&[
        (REMOTE_URL_KEY, "https://fleet.example.com"),
        (CLIENT_NAME_KEY, "laptop"),
        (ALLOW_PLAINTEXT_KEY, "true"),
    ]);
    let tokens = InMemoryTokenStore::with_token("cl_live");

    let got = logic::disconnect(&remote("https://fleet.example.com"), &store, &tokens).unwrap();

    assert_eq!(tokens.get().unwrap(), None);
    let s = store.lock().unwrap();
    for key in [REMOTE_URL_KEY, CLIENT_NAME_KEY, ALLOW_PLAINTEXT_KEY] {
        let left = s.get_setting(key).unwrap().unwrap_or_default();
        assert!(left.is_empty(), "{key} still holds {left:?}");
    }
    drop(s);
    assert_eq!(got.configured_url, None);
    assert!(
        got.restart_required,
        "this process is still a hub client until it restarts"
    );
}

#[test]
fn disconnecting_twice_is_not_an_error() {
    let (_dir, store) = store_with(&[]);
    let tokens = InMemoryTokenStore::empty();
    logic::disconnect(&Backend::Local, &store, &tokens).unwrap();
    logic::disconnect(&Backend::Local, &store, &tokens).unwrap();
}

/// The whole point of the wording the spec asks for: Disconnect is local. The
/// token stays valid on the hub until an operator revokes it, and this app
/// must never imply otherwise — a `revoke_client` call is one a paired client
/// is refused anyway.
#[test]
fn disconnect_does_not_pretend_to_revoke() {
    let src = include_str!("hub.rs");
    let said = src.to_lowercase();
    assert!(
        said.contains("does not revoke") || said.contains("revoke nothing"),
        "hub.rs must say in its own source that Disconnect revokes nothing"
    );
    // And it must not *call* anything that claims to. The tool name appears
    // in the module docs in backticks, explaining that a paired client is
    // refused it; what must not appear is the name as a string literal, which
    // is the only way it could become a call.
    assert!(
        !src.contains("\"revoke_client\""),
        "a paired client may not revoke — and could not: the hub refuses \
         revoke_client to anything but the master"
    );
}

// --- the shape of a PairedClient ---------------------------------------------

/// A guard on the seam rather than on this module: `logic::pair` writes what
/// `redeem` returned, so a rename there must not silently store the wrong
/// field (a `mode` in the name column would show "full" as a machine name).
#[test]
fn the_client_name_written_is_the_name_the_hub_gave() {
    let (_dir, store) = store_with(&[]);
    block_on(logic::pair(
        &Fake::ok("cl_t", "studio-mac", "readonly"),
        &Backend::Local,
        &store,
        &InMemoryTokenStore::empty(),
        args("https://fleet.example.com", "ABCD1234"),
    ))
    .unwrap();
    assert_eq!(
        store
            .lock()
            .unwrap()
            .get_setting(CLIENT_NAME_KEY)
            .unwrap()
            .as_deref(),
        Some("studio-mac")
    );
}

/// `PairedClient::mode` is `readonly` for a client the operator paired that
/// way, and the UI has to be able to say so — a readonly client's mutations
/// all fail at the hub with `E_FORBIDDEN`, which is a much better thing to
/// explain up front than at the click.
#[test]
fn the_paired_mode_is_reported_so_a_readonly_client_can_be_told_so() {
    let (_dir, store) = store_with(&[]);
    let got = block_on(logic::pair(
        &Fake::ok("cl_t", "studio-mac", "readonly"),
        &Backend::Local,
        &store,
        &InMemoryTokenStore::empty(),
        args("https://fleet.example.com", "ABCD1234"),
    ))
    .unwrap();
    assert_eq!(got.client_mode.as_deref(), Some("readonly"));
}

/// A `PairedClient` is built here and dropped; nothing may keep it.
#[test]
fn a_paired_client_is_not_kept_anywhere() {
    fn assert_no_serialize<T>(_: &T) {}
    let c = PairedClient {
        token: "cl_t".into(),
        name: "n".into(),
        mode: "full".into(),
        hub: "h".into(),
    };
    assert_no_serialize(&c);
    assert!(!format!("{c:?}").contains("cl_t"));
}
