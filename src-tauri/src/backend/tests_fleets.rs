//! Tests for [`super`] (`backend::fleets`): the extra-fleet registry, its
//! routing seam and its isolation guarantees. No network, no keychain.

use super::*;
use crate::backend::connection::{ConnectionReporter, HubConnection};
use crate::backend::remote::HubResponse;
use crate::backend::token_store::InMemoryTokenStore;
use fleet_core::events::NoopEventBus;
use std::sync::atomic::{AtomicUsize, Ordering};

// --- doubles -----------------------------------------------------------------

/// One in-memory token store per slot, created on first ask, and a count of
/// how many times anyone asked at all.
#[derive(Default)]
struct Tokens {
    by_fleet: Mutex<BTreeMap<String, Arc<InMemoryTokenStore>>>,
    asks: AtomicUsize,
    failing: Option<String>,
}

impl Tokens {
    fn with(entries: &[(&str, &str)]) -> Arc<Self> {
        let t = Self::default();
        for (fleet, token) in entries {
            t.by_fleet.lock().unwrap().insert(
                fleet.to_string(),
                Arc::new(InMemoryTokenStore::with_token(token)),
            );
        }
        Arc::new(t)
    }

    fn token_of(&self, fleet: &str) -> Option<String> {
        self.by_fleet
            .lock()
            .unwrap()
            .get(fleet)
            .and_then(|s| s.get().unwrap())
    }
}

impl FleetTokens for Tokens {
    fn for_fleet(&self, fleet: &str) -> Arc<dyn TokenStore> {
        self.asks.fetch_add(1, Ordering::SeqCst);
        if let Some(e) = &self.failing {
            return Arc::new(InMemoryTokenStore::failing(e));
        }
        let mut map = self.by_fleet.lock().unwrap();
        let store = map
            .entry(fleet.to_string())
            .or_insert_with(|| Arc::new(InMemoryTokenStore::empty()));
        Arc::clone(store) as Arc<dyn TokenStore>
    }
}

/// Records every `follow` instead of opening a stream.
#[derive(Default)]
struct Links {
    followed: Mutex<Vec<(String, String, String)>>,
}

impl FleetLinks for Links {
    fn follow(
        &self,
        fleet: &str,
        cfg: RemoteConfig,
        _hub: Arc<HubBackend>,
        _sink: Arc<dyn RemoteEventSink>,
        _status: Arc<HubConnectionStatus>,
        _cancel: CancellationToken,
    ) {
        self.followed
            .lock()
            .unwrap()
            .push((fleet.to_string(), cfg.base_url, cfg.token));
    }
}

/// Every `(name, payload)` the window would have been sent.
#[derive(Default)]
struct Sink {
    seen: Mutex<Vec<(&'static str, Value)>>,
}

impl RemoteEventSink for Sink {
    fn emit_remote(&self, name: &'static str, payload: Value) {
        self.seen.lock().unwrap().push((name, payload));
    }
}

/// A hub's `POST /pair`, answering with `token`, counting calls.
struct Pair {
    token: &'static str,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl PairTransport for Pair {
    async fn post_pair(&self, _url: &str, _body: String) -> Result<HubResponse, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(HubResponse {
            status: 200,
            body: json!({
                "token": self.token,
                "name": "laptop",
                "mode": "full",
                "hub": "https://fleet.example.com",
            })
            .to_string(),
        })
    }
}

fn pair(token: &'static str) -> Pair {
    Pair {
        token,
        calls: AtomicUsize::new(0),
    }
}

struct World {
    _dir: tempfile::TempDir,
    store: Arc<Mutex<Store>>,
    tokens: Arc<Tokens>,
    links: Arc<Links>,
    sink: Arc<Sink>,
    fleets: Fleets,
}

fn world(records: &[ExtraFleet], tokens: Arc<Tokens>) -> World {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_with_bus(&dir.path().join("state.db"), Arc::new(NoopEventBus)).unwrap();
    if !records.is_empty() {
        store
            .set_setting(FLEETS_KEY, &serde_json::to_string(records).unwrap())
            .unwrap();
    }
    let store = Arc::new(Mutex::new(store));
    let links = Arc::new(Links::default());
    let sink = Arc::new(Sink::default());
    let fleets = Fleets::new(
        Arc::clone(&store),
        Arc::clone(&tokens) as Arc<dyn FleetTokens>,
        Arc::clone(&sink) as Arc<dyn RemoteEventSink>,
        Arc::clone(&links) as Arc<dyn FleetLinks>,
        CancellationToken::new(),
    );
    fleets.load();
    World {
        _dir: dir,
        store,
        tokens,
        links,
        sink,
        fleets,
    }
}

fn record(id: &str, url: &str) -> ExtraFleet {
    ExtraFleet {
        id: id.into(),
        label: id.into(),
        url: url.into(),
        client_name: "laptop".into(),
        allow_plaintext: false,
        color: None,
    }
}

fn standalone() -> PrimaryView {
    PrimaryView {
        url: None,
        client_name: None,
        local: true,
        connection: None,
        unavailable: None,
        configured_url: None,
        fleet_id: None,
    }
}

fn paired_to(url: &str) -> PrimaryView {
    PrimaryView {
        url: Some(url.into()),
        client_name: Some("laptop".into()),
        local: false,
        connection: Some(HubConnection::Connected),
        unavailable: None,
        configured_url: Some(url.into()),
        fleet_id: None,
    }
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

fn add_args(url: &str) -> FleetAddArgs {
    FleetAddArgs {
        url: url.into(),
        code: "ABCD2345".into(),
        label: None,
        color: None,
        allow_plaintext: false,
    }
}

// --- the stored rows ---------------------------------------------------------

#[test]
fn no_stored_fleets_is_an_empty_list_and_garbage_is_an_error() {
    assert!(parse_records("").unwrap().is_empty());
    assert!(parse_records("  ").unwrap().is_empty());
    assert_eq!(parse_records("{not json").unwrap_err().code, codes::E_PARSE);
}

#[test]
fn a_stored_row_with_an_unusable_or_repeated_id_is_dropped_not_fatal() {
    let raw = serde_json::to_string(&[
        record("company", "https://fleet.papayapos.dev"),
        record("primary", "https://a.example.com"),
        record("../etc", "https://b.example.com"),
        record("company", "https://c.example.com"),
    ])
    .unwrap();
    let rows = parse_records(&raw).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].url, "https://fleet.papayapos.dev");
}

#[test]
fn slot_ids_are_safe_as_a_keychain_account_and_a_file_name() {
    for ok in ["company", "rlt", "a-1", "x"] {
        assert!(valid_slot(ok), "{ok}");
    }
    for bad in [
        "",
        "primary",
        "-x",
        "Company",
        "a/b",
        "a.b",
        "../x",
        "a b",
        "aaaaaaaaaaaaaaaaaaaaaaaaa",
    ] {
        assert!(!valid_slot(bad), "{bad:?}");
    }
}

#[test]
fn a_slot_is_derived_from_the_label_and_never_collides() {
    assert_eq!(slot_for("PapayaPOS company", &[]), "papayapos-company");
    assert_eq!(slot_for("  rlt  ", &[]), "rlt");
    assert_eq!(slot_for("rlt", &["rlt"]), "rlt-2");
    assert_eq!(slot_for("rlt", &["rlt", "rlt-2"]), "rlt-3");
    assert_eq!(slot_for("Primary", &[]), "fleet");
    assert_eq!(slot_for("✨", &[]), "fleet");
    for label in [
        "PapayaPOS company",
        "a very long label that goes on and on and on",
    ] {
        assert!(valid_slot(&slot_for(label, &[])), "{label}");
    }
}

#[test]
fn a_fleets_default_label_is_its_registrable_name() {
    assert_eq!(default_label("https://fleet.papayapos.dev"), "papayapos");
    assert_eq!(default_label("https://fleet.rlt.sk"), "rlt");
    assert_eq!(default_label("http://localhost:8787"), "localhost");
    assert_eq!(default_label("http://10.0.0.5:8787"), "10.0.0.5");
}

// --- loading -----------------------------------------------------------------

#[test]
fn with_no_extra_fleet_nothing_is_read_or_followed() {
    let w = world(&[], Tokens::with(&[]));
    assert_eq!(
        w.tokens.asks.load(Ordering::SeqCst),
        0,
        "an install with no extra fleet must not touch a token store (design D11)"
    );
    assert!(w.links.followed.lock().unwrap().is_empty());
    let list = w.fleets.list(&standalone());
    assert_eq!(list.len(), 1);
    assert!(list[0].primary && list[0].local);
    assert_eq!(list[0].label, "This machine");
}

#[test]
fn each_stored_fleet_is_followed_with_its_own_token() {
    let w = world(
        &[
            record("company", "https://fleet.papayapos.dev"),
            record("lab", "https://lab.example.com"),
        ],
        Tokens::with(&[("company", "cl_company"), ("lab", "cl_lab")]),
    );
    let followed = w.links.followed.lock().unwrap().clone();
    assert_eq!(
        followed,
        vec![
            (
                "company".into(),
                "https://fleet.papayapos.dev".into(),
                "cl_company".into()
            ),
            (
                "lab".into(),
                "https://lab.example.com".into(),
                "cl_lab".into()
            ),
        ]
    );
    let primary = Arc::new(FleetBackend::local());
    let company = w.fleets.select(&primary, Some("company")).unwrap();
    let hub = company.hub().expect("an extra fleet is always a hub");
    assert_eq!(hub.config().base_url, "https://fleet.papayapos.dev");
    assert_eq!(
        hub.config().token,
        "cl_company",
        "no token is ever shared between hubs"
    );
    assert_eq!(
        w.fleets
            .select(&primary, Some("lab"))
            .unwrap()
            .hub()
            .unwrap()
            .config()
            .token,
        "cl_lab"
    );
}

#[test]
fn a_stored_fleet_with_no_token_is_listed_with_the_reason_and_refuses() {
    let w = world(
        &[record("company", "https://fleet.papayapos.dev")],
        Tokens::with(&[]),
    );
    assert!(w.links.followed.lock().unwrap().is_empty());
    let info = &w.fleets.list(&standalone())[1];
    assert_eq!(info.id, "company");
    assert!(info.connection.is_none());
    assert!(
        info.unavailable
            .as_deref()
            .is_some_and(|r| r.contains("no client token")),
        "{info:?}"
    );
    let err = w
        .fleets
        .select(&Arc::new(FleetBackend::local()), Some("company"))
        .unwrap_err();
    assert_eq!(err.code, codes::E_HUB_UNAVAILABLE);
}

#[test]
fn a_plaintext_fleet_without_its_opt_in_is_unusable_before_its_token_is_read() {
    let mut row = record("lan", "http://10.0.0.5:8787");
    let w = world(
        std::slice::from_ref(&row),
        Tokens::with(&[("lan", "cl_lan")]),
    );
    assert_eq!(w.tokens.asks.load(Ordering::SeqCst), 0);
    assert!(w.fleets.list(&standalone())[1].unavailable.is_some());

    row.allow_plaintext = true;
    let w = world(&[row], Tokens::with(&[("lan", "cl_lan")]));
    assert_eq!(w.links.followed.lock().unwrap().len(), 1);
}

#[test]
fn a_locked_keychain_marks_only_that_fleet_unusable() {
    let tokens = Arc::new(Tokens {
        failing: Some("keychain locked".into()),
        ..Default::default()
    });
    let w = world(&[record("company", "https://fleet.papayapos.dev")], tokens);
    let info = &w.fleets.list(&standalone())[1];
    assert!(
        info.unavailable
            .as_deref()
            .is_some_and(|r| r.contains("keychain locked")),
        "{info:?}"
    );
}

// --- routing -----------------------------------------------------------------

#[test]
fn no_fleet_and_the_primary_slot_select_the_launch_time_backend() {
    let w = world(&[], Tokens::with(&[]));
    let primary = Arc::new(FleetBackend::local());
    for fleet in [None, Some(""), Some("primary")] {
        let got = w.fleets.select(&primary, fleet).unwrap();
        assert!(Arc::ptr_eq(&got, &primary), "{fleet:?}");
    }
}

#[test]
fn an_unknown_fleet_is_refused_not_guessed() {
    let w = world(&[], Tokens::with(&[]));
    let err = w
        .fleets
        .select(&Arc::new(FleetBackend::local()), Some("company"))
        .unwrap_err();
    assert_eq!(err.code, codes::E_NOTFOUND);
    assert!(err.message.contains("company"), "{}", err.message);
}

#[test]
fn a_local_only_command_addressed_to_an_extra_fleet_refuses_with_that_hubs_url() {
    let w = world(
        &[record("company", "https://fleet.papayapos.dev")],
        Tokens::with(&[("company", "cl_company")]),
    );
    let company = w
        .fleets
        .select(&Arc::new(FleetBackend::local()), Some("company"))
        .unwrap();
    let err = company.refuse_local_only("inspect_safe_kill").unwrap_err();
    assert_eq!(err.code, codes::E_LOCAL_ONLY);
    assert!(
        err.message.contains("https://fleet.papayapos.dev"),
        "{}",
        err.message
    );
}

#[test]
fn one_hubs_contract_skew_refuses_only_that_hubs_calls() {
    let w = world(
        &[
            record("company", "https://fleet.papayapos.dev"),
            record("lab", "https://lab.example.com"),
        ],
        Tokens::with(&[("company", "cl_company"), ("lab", "cl_lab")]),
    );
    w.fleets
        .status_of("company")
        .unwrap()
        .report(HubConnection::HubTooOld {
            hub_contract: 1,
            min_contract: 13,
        });
    let primary = Arc::new(FleetBackend::local());
    let company = w.fleets.select(&primary, Some("company")).unwrap();
    let err = block_on(company.hub().unwrap().list_sessions(false)).unwrap_err();
    assert_eq!(err.code, codes::E_HUB_CONTRACT);
    assert!(
        w.fleets
            .status_of("lab")
            .unwrap()
            .contract_verdict()
            .is_none(),
        "a skewed hub must not blind the others"
    );
}

#[test]
fn one_hub_added_twice_under_two_urls_is_spotted_by_its_fleet_id() {
    let w = world(
        &[
            record("company", "https://fleet.papayapos.dev"),
            record("company-2", "https://10.0.0.7"),
            record("lab", "https://lab.example.com"),
        ],
        Tokens::with(&[("company", "cl_a"), ("company-2", "cl_b"), ("lab", "cl_c")]),
    );
    // Nothing is a duplicate before the hubs have said who they are.
    let primary = Arc::new(FleetBackend::local());
    assert!(w.fleets.select(&primary, Some("company-2")).is_ok());
    for (slot, id) in [("company", "f-1"), ("company-2", "f-1"), ("lab", "f-2")] {
        w.fleets
            .status_of(slot)
            .unwrap()
            .hub_fleet_id(Some(id.to_string()));
    }
    let list = w.fleets.list(&standalone());
    let dup = |slot: &str| {
        list.iter()
            .find(|f| f.id == slot)
            .unwrap()
            .duplicate_of
            .clone()
    };
    // The first in slot order keeps working; the later one is the duplicate.
    assert_eq!(dup("company"), None);
    assert_eq!(dup("company-2").as_deref(), Some("\"company\""));
    assert_eq!(dup("lab"), None);
    assert_eq!(
        list.iter()
            .find(|f| f.id == "lab")
            .unwrap()
            .fleet_id
            .as_deref(),
        Some("f-2")
    );
    let err = w.fleets.select(&primary, Some("company-2")).unwrap_err();
    assert_eq!(err.code, codes::E_EXISTS);
    assert!(w.fleets.select(&primary, Some("company")).is_ok());
}

#[test]
fn an_extra_fleet_that_is_the_primarys_own_hub_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_with_bus(&dir.path().join("state.db"), Arc::new(NoopEventBus)).unwrap();
    let w = Fleets::new(
        Arc::new(Mutex::new(store)),
        Tokens::with(&[("home", "cl_home")]) as Arc<dyn FleetTokens>,
        Arc::new(Sink::default()) as Arc<dyn RemoteEventSink>,
        Arc::new(Links::default()) as Arc<dyn FleetLinks>,
        CancellationToken::new(),
    )
    .with_primary_identity(Arc::new(|| Some("f-home".to_string())));
    w.store_records_for_test(&[record("home", "https://10.0.0.5")]);
    w.load();
    w.status_of("home")
        .unwrap()
        .hub_fleet_id(Some("f-home".to_string()));
    let err = w
        .select(&Arc::new(FleetBackend::local()), Some("home"))
        .unwrap_err();
    assert_eq!(err.code, codes::E_EXISTS);
    assert!(err.message.contains("primary"), "{}", err.message);
    let mut primary = standalone();
    primary.fleet_id = Some("f-home".into());
    let list = w.list(&primary);
    assert_eq!(list[0].fleet_id.as_deref(), Some("f-home"));
    assert_eq!(list[1].duplicate_of.as_deref(), Some("the primary fleet"));
}

// --- events ------------------------------------------------------------------

#[test]
fn an_extra_fleets_events_reach_the_window_tagged_with_its_slot() {
    let sink = Arc::new(Sink::default());
    let tagged = TaggedSink::new("company", Arc::clone(&sink) as Arc<dyn RemoteEventSink>);
    tagged.emit_remote("session:updated", json!({ "id": 1 }));
    let seen = sink.seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![(
            FLEET_EVENT,
            json!({ "fleet": "company", "name": "session:updated", "payload": { "id": 1 } })
        )]
    );
}

#[test]
fn an_extra_fleets_connection_state_is_tagged_too() {
    let w = world(
        &[record("company", "https://fleet.papayapos.dev")],
        Tokens::with(&[("company", "cl_company")]),
    );
    w.fleets
        .status_of("company")
        .unwrap()
        .report(HubConnection::Connected);
    let seen = w.sink.seen.lock().unwrap().clone();
    assert!(
        seen.iter().any(|(name, p)| *name == FLEET_EVENT
            && p["fleet"] == "company"
            && p["name"] == "hub:connection"),
        "{seen:?}"
    );
    assert_eq!(
        w.fleets.list(&standalone())[1].connection,
        Some(HubConnection::Connected)
    );
}

// --- adding and removing -----------------------------------------------------

#[test]
fn adding_a_hub_pairs_stores_its_token_under_its_own_slot_and_follows_it() {
    let w = world(&[], Tokens::with(&[]));
    let transport = pair("cl_new");
    let info = block_on(w.fleets.add(
        &transport,
        &paired_to("https://fleet.rlt.sk"),
        FleetAddArgs {
            label: Some("Company".into()),
            ..add_args("https://fleet.papayapos.dev/")
        },
    ))
    .unwrap();
    assert_eq!(info.id, "company");
    assert_eq!(info.label, "Company");
    assert_eq!(info.url.as_deref(), Some("https://fleet.papayapos.dev"));
    assert!(!info.primary);
    assert_eq!(w.tokens.token_of("company").as_deref(), Some("cl_new"));
    let stored = read_records(&w.store).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].client_name, "laptop");
    assert!(
        !serde_json::to_string(&stored).unwrap().contains("cl_new"),
        "a token must never reach state.db"
    );
    assert_eq!(w.links.followed.lock().unwrap().len(), 1);
}

/// A hub's `POST /pair` that runs `during` before it answers: what another
/// add or a remove did while the pairing was in flight.
struct PairWhile<F: Fn() + Send + Sync> {
    inner: Pair,
    during: F,
}

#[async_trait::async_trait]
impl<F: Fn() + Send + Sync> PairTransport for PairWhile<F> {
    async fn post_pair(&self, url: &str, body: String) -> Result<HubResponse, String> {
        (self.during)();
        self.inner.post_pair(url, body).await
    }
}

#[test]
fn an_add_keeps_the_rows_written_while_its_pairing_was_in_flight() {
    let w = world(
        &[record("company", "https://fleet.papayapos.dev")],
        Tokens::with(&[("company", "cl_company")]),
    );
    // While the pairing is in flight: `company` is removed and another add
    // takes the slot `lab` would have had.
    let store = Arc::clone(&w.store);
    let transport = PairWhile {
        inner: pair("cl_lab"),
        during: move || {
            write_records(&store, &[record("lab", "https://other.example.com")]).unwrap();
        },
    };
    let info = block_on(w.fleets.add(
        &transport,
        &standalone(),
        FleetAddArgs {
            label: Some("lab".into()),
            ..add_args("https://lab.example.com")
        },
    ))
    .unwrap();
    assert_ne!(
        info.id, "lab",
        "the slot is chosen from the rows as they stand"
    );
    let stored: Vec<(String, String)> = read_records(&w.store)
        .unwrap()
        .into_iter()
        .map(|r| (r.id, r.url))
        .collect();
    assert_eq!(
        stored,
        vec![
            ("lab".to_string(), "https://other.example.com".to_string()),
            (info.id.clone(), "https://lab.example.com".to_string()),
        ],
        "the removed row must stay gone and the concurrent add must survive"
    );
    assert_eq!(w.tokens.token_of(&info.id).as_deref(), Some("cl_lab"));
}

#[test]
fn a_hub_added_while_its_pairing_was_in_flight_is_not_added_twice() {
    let w = world(&[], Tokens::with(&[]));
    let store = Arc::clone(&w.store);
    let transport = PairWhile {
        inner: pair("cl_lab"),
        during: move || {
            write_records(&store, &[record("lab", "https://lab.example.com")]).unwrap();
        },
    };
    let err = block_on(w.fleets.add(
        &transport,
        &standalone(),
        add_args("https://lab.example.com"),
    ))
    .unwrap_err();
    assert_eq!(err.code, codes::E_EXISTS, "{}", err.message);
    assert_eq!(read_records(&w.store).unwrap().len(), 1);
}

#[test]
fn a_hub_already_shown_is_refused_before_the_code_is_spent() {
    let w = world(
        &[record("company", "https://fleet.papayapos.dev")],
        Tokens::with(&[("company", "cl_company")]),
    );
    for (url, primary) in [
        ("https://fleet.rlt.sk/", paired_to("https://fleet.rlt.sk")),
        ("https://FLEET.papayapos.dev", standalone()),
    ] {
        let transport = pair("cl_dup");
        let err = block_on(w.fleets.add(&transport, &primary, add_args(url))).unwrap_err();
        assert_eq!(err.code, codes::E_EXISTS, "{url}: {}", err.message);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0, "{url}");
    }
}

#[test]
fn plain_http_to_a_routable_hub_is_refused_before_the_code_is_spent() {
    let w = world(&[], Tokens::with(&[]));
    let transport = pair("cl_x");
    let err = block_on(
        w.fleets
            .add(&transport, &standalone(), add_args("http://10.0.0.5:8787")),
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_HUB_PLAINTEXT);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_bad_colour_or_an_empty_code_is_refused_before_the_code_is_spent() {
    let w = world(&[], Tokens::with(&[]));
    let transport = pair("cl_x");
    let mut args = add_args("https://lab.example.com");
    args.color = Some("red".into());
    let err = block_on(w.fleets.add(&transport, &standalone(), args)).unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
    let mut args = add_args("https://lab.example.com");
    args.code = "  ".into();
    let err = block_on(w.fleets.add(&transport, &standalone(), args)).unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn removing_a_fleet_stops_it_and_forgets_its_token_and_row() {
    let w = world(
        &[
            record("company", "https://fleet.papayapos.dev"),
            record("lab", "https://lab.example.com"),
        ],
        Tokens::with(&[("company", "cl_company"), ("lab", "cl_lab")]),
    );
    w.fleets.remove("company").unwrap();
    assert_eq!(w.tokens.token_of("company"), None);
    assert_eq!(
        w.tokens.token_of("lab").as_deref(),
        Some("cl_lab"),
        "removing one fleet must not touch another's token"
    );
    let ids: Vec<String> = read_records(&w.store)
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, vec!["lab".to_string()]);
    assert!(w.fleets.cancelled("company").is_none());
    assert_eq!(w.fleets.cancelled("lab"), Some(false));
    let err = w
        .fleets
        .select(&Arc::new(FleetBackend::local()), Some("company"))
        .unwrap_err();
    assert_eq!(err.code, codes::E_NOTFOUND);
}

#[test]
fn the_primary_cannot_be_removed_here() {
    let w = world(&[], Tokens::with(&[]));
    assert_eq!(w.fleets.remove(PRIMARY).unwrap_err().code, codes::E_INVALID);
    assert_eq!(w.fleets.remove("nope").unwrap_err().code, codes::E_NOTFOUND);
}

#[test]
fn retry_rebuilds_a_fleet_whose_token_could_not_be_read() {
    let w = world(
        &[record("company", "https://fleet.papayapos.dev")],
        Tokens::with(&[]),
    );
    assert!(w.fleets.list(&standalone())[1].unavailable.is_some());
    w.tokens.for_fleet("company").set("cl_late").unwrap();
    w.fleets.retry("company").unwrap();
    let info = &w.fleets.list(&standalone())[1];
    assert!(info.unavailable.is_none(), "{info:?}");
    assert_eq!(w.links.followed.lock().unwrap().len(), 1);
}

// --- the source -------------------------------------------------------------

/// The body of `#[tauri::command] … fn name(`, up to the next command.
fn command_body<'a>(src: &'a str, name: &str) -> Option<&'a str> {
    let start = [format!("pub async fn {name}("), format!("pub fn {name}(")]
        .iter()
        .find_map(|sig| src.find(sig.as_str()))?;
    let rest = &src[start..];
    let end = rest[1..]
        .find("#[tauri::command")
        .map(|i| i + 1)
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Every command in [`FLEET_SCOPED`] takes `fleet` and routes through
/// `select`; no other command in those files takes a `fleet` it would
/// silently ignore (design D7).
#[test]
fn every_fleet_scoped_command_selects_and_no_other_takes_a_fleet() {
    let files = [
        (
            "commands/sessions.rs",
            include_str!("../commands/sessions.rs"),
        ),
        ("commands/hosts.rs", include_str!("../commands/hosts.rs")),
        (
            "commands/projects.rs",
            include_str!("../commands/projects.rs"),
        ),
        ("commands/prs.rs", include_str!("../commands/prs.rs")),
        (
            "commands/env_vars.rs",
            include_str!("../commands/env_vars.rs"),
        ),
        ("commands/pages.rs", include_str!("../commands/pages.rs")),
        // Settings › Company per fleet.
        ("commands/orgs.rs", include_str!("../commands/orgs.rs")),
        (
            "commands/org_devices.rs",
            include_str!("../commands/org_devices.rs"),
        ),
        (
            "commands/trackers.rs",
            include_str!("../commands/trackers.rs"),
        ),
        ("commands/assets.rs", include_str!("../commands/assets.rs")),
    ];
    for name in FLEET_SCOPED {
        let body = files
            .iter()
            .find_map(|(_, src)| command_body(src, name))
            .unwrap_or_else(|| {
                panic!(
                    "{name} is fleet-scoped but defined in none of {files:?}",
                    files = files.map(|f| f.0)
                )
            });
        assert!(
            body.contains("fleet: Option<String>"),
            "{name} takes no fleet"
        );
        assert!(
            body.contains("fleets.select(backend.inner(), fleet.as_deref())?"),
            "{name} takes a fleet but does not route by it"
        );
    }
    for (file, src) in files {
        for chunk in src.split("#[tauri::command").skip(1) {
            let Some(sig) = chunk.split_once("fn ").map(|(_, s)| s) else {
                continue;
            };
            let name = &sig[..sig.find('(').unwrap()];
            let head = &sig[..sig
                .find(") ->")
                .or_else(|| sig.find(")\n"))
                .unwrap_or(sig.len())];
            if head.contains("fleet: Option<String>") {
                assert!(
                    FLEET_SCOPED.contains(&name),
                    "{file}::{name} takes a fleet but is not in FLEET_SCOPED"
                );
            }
        }
    }
}

/// Settings › Company acts on whichever fleet its picker names: every
/// command its resources (Organisations, Devices, People, Groups) list, run
/// or preview through must route by `fleet`, or a write meant for an extra
/// fleet's hub would be refused by the frontend (`resources.ts::fleetArgs`)
/// rather than reach it — and a new action added to one of them later must
/// join `FLEET_SCOPED` with it.
#[test]
fn every_company_resource_command_is_fleet_scoped() {
    use fleet_core::pages::resources::resource;
    for id in ["org", "device", "person", "group"] {
        let r = resource(id).unwrap_or_else(|| panic!("no resource {id}"));
        let mut names = vec![r.list];
        for a in r.actions() {
            names.push(a.command);
            names.extend(a.preview);
        }
        for name in names {
            assert!(
                FLEET_SCOPED.contains(&name),
                "{id}: {name} is not fleet-scoped, so the Company page cannot send it to an extra fleet"
            );
        }
    }
}

/// Retry and remove reach the OS keychain, which can wait on an unlock
/// prompt: a sync command would run them on the macOS main thread.
#[test]
fn fleet_commands_that_touch_the_keychain_are_off_the_main_thread() {
    let src = include_str!("../commands/fleets.rs");
    for name in ["fleet_retry_now", "fleet_remove"] {
        let at = src
            .find(&format!("pub fn {name}("))
            .or_else(|| src.find(&format!("pub async fn {name}(")))
            .unwrap_or_else(|| panic!("{name} not found"));
        let head = &src[..at];
        let attr = &head[head.rfind("#[tauri::command").expect("an attribute")..];
        assert!(
            attr.starts_with("#[tauri::command(async)]") || src[at..].starts_with("pub async fn"),
            "{name} must be an async command"
        );
    }
}

#[test]
fn the_extra_fleet_hub_clients_are_watched() {
    let src = include_str!("fleets.rs");
    assert!(
        src.matches(".watching(").count() >= 2,
        "an extra fleet's hub clients must consult that fleet's own connection status"
    );
}
