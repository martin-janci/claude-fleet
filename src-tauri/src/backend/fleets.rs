//! Extra fleets: windows onto further hubs, beside the one this app resolved
//! to at launch (`docs/superpowers/specs/2026-10-17-desktop-multi-hub-design.md`).
//!
//! # Two kinds of fleet
//!
//! The **primary** is everything [`super::Backend`] already is: standalone,
//! a hub client, or a configured hub this launch cannot use. It is decided
//! once, at startup, and it is the only fleet this process may ever *own*
//! (design D1). Its pairing is read where it has always lived — `hub.remote_url`
//! and the keychain item `hub-client-token` — so an install from before this
//! module is "hub #1" with no migration at all.
//!
//! An **extra fleet** is another hub this window also shows. It is never this
//! process's to own, so it is hot (D3): added, removed and retried at runtime.
//! Each has its own client stack — [`HubBackend`], [`HubConnectionStatus`]
//! and an event bridge — so a hub that is down, or on a wire contract this
//! build does not read, refuses only its own calls (D4). Its events reach the
//! window tagged with its slot id ([`TaggedSink`], D5), never in the primary's
//! stores, where its ids would collide.
//!
//! # Routing by owner
//!
//! A fleet-scoped command (one in [`FLEET_SCOPED`]) takes an optional `fleet`
//! argument and asks [`Fleets::select`] for the backend to use, then calls the
//! very `routed::` function it always called (D6). An extra fleet's
//! [`FleetBackend`] always holds a hub, so the standalone arm — this machine's
//! store and SSH — is unreachable from it, and a `LocalOnly` command refuses
//! with that fleet's URL like any hub's.

use super::connection::{ConnectionView, HubConnection, HubConnectionStatus};
use super::events::RemoteEventSink;
use super::pairing::{self, PairTransport};
use super::remote::HubBackend;
use super::routing::FleetBackend;
use super::token_store::TokenStore;
use super::{normalise_base_url, plaintext_risk, RemoteConfig};
use fleet_core::ipc_error::{codes, lock, IpcError};
use fleet_core::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};
use tokio_util::sync::CancellationToken;

/// The settings value holding the extra fleets, a JSON list of
/// [`ExtraFleet`]. Internal state, never registered in
/// `service::settings::SPECS`: the rows carry no secret, but they are this
/// window's own wiring, not an operator setting (design D2).
pub const FLEETS_KEY: &str = "hub.fleets";

/// The slot id of the fleet this app resolved to at launch.
pub const PRIMARY: &str = "primary";

/// The frontend event an extra fleet's events travel on: `{fleet, name,
/// payload}`. The primary's events keep their own names and channel.
pub const FLEET_EVENT: &str = "fleet:event";

/// The commands that take a `fleet` argument and route through
/// [`Fleets::select`]. Grown one reviewed command at a time (design D7);
/// `tests_fleets::every_fleet_scoped_command_selects_and_no_other_takes_a_fleet`
/// holds the command bodies to this list.
pub const FLEET_SCOPED: &[&str] = &[
    "list_sessions",
    "new_session",
    "kill_session",
    "send_prompt",
    "set_session_friendly_name",
    "pin_session",
    "list_hosts",
    "list_projects",
    // Phase 2: an extra fleet's session opens in its own view.
    "session_conversation",
    "session_tool_detail",
    "capture_session",
    "touch_session_viewed",
    // Phase 2: the PRs page per fleet.
    "list_pull_requests",
    // Settings › Environment per fleet: a person's own variables on each hub,
    // and the two reads that say what this device may change there.
    "env_vars",
    "env_vars_list",
    "env_var_set",
    "env_var_unset",
    "my_grants",
    "setting_proposals",
    // Settings › Company per fleet: the org_admin / realm_admin pages
    // (Organisations, Devices, People, Groups) on each hub, judged by that
    // hub, and the lists their option selects read.
    "list_orgs",
    "add_org",
    "update_org",
    "remove_org",
    "add_org_rule",
    "remove_org_rule",
    "org_rule_preview",
    "set_org_setting",
    "add_org_project",
    "remove_org_project",
    "assign_host_org",
    "assign_tracker_org",
    "set_org_member",
    "remove_org_member",
    "org_member_grants",
    "revoke_org_member_grants",
    "revoke_org_share",
    "narrow_org_share",
    "list_devices",
    "pair_device",
    "update_device",
    "revoke_device",
    "bind_device_org",
    "grant_device_catalog",
    "list_people",
    "add_person",
    "rename_person",
    "disable_person",
    "list_groups",
    "list_trackers",
    "catalog_list_catalogs",
    // Phase 3b: who this device is on each fleet.
    "my_profile",
    // Phase 3: an extra fleet's session in the full conversation panel.
    "session_conversations",
    "session_activity",
    "restart_session",
];

/// Colours offered to a new fleet, in order; the first one no fleet uses is
/// the default. The primary's is the first.
pub const PALETTE: &[&str] = &[
    "#5b8def", "#e8833a", "#3fb27f", "#c95fd1", "#d4b13c", "#e05a6d",
];

/// Longest slot id. Short, because it is a keychain account suffix, a file
/// name and a key in every frontend map.
const SLOT_MAX: usize = 24;
/// Longest label a person may give a fleet.
const LABEL_MAX: usize = 40;

/// One extra fleet, as stored in [`FLEETS_KEY`]. Carries no token.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ExtraFleet {
    /// The slot id: `[a-z0-9-]`, at most [`SLOT_MAX`], never [`PRIMARY`].
    pub id: String,
    /// What the window calls it ("company").
    pub label: String,
    /// The hub's base URL, normalised.
    pub url: String,
    /// The name this desktop was paired under on that hub. Cosmetic.
    #[serde(default)]
    pub client_name: String,
    /// The per-fleet plaintext opt-in (design D9).
    #[serde(default)]
    pub allow_plaintext: bool,
    /// A `#rrggbb` badge colour, or `None` for the palette's choice.
    #[serde(default)]
    pub color: Option<String>,
}

/// Whether `id` may name an extra fleet.
pub fn valid_slot(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= SLOT_MAX
        && id != PRIMARY
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A slot id for `label` that no fleet in `taken` uses: the label in
/// lower-case with runs of anything else turned into one `-`, then `-2`,
/// `-3`… until it is free. Never [`PRIMARY`].
pub fn slot_for(label: &str, taken: &[&str]) -> String {
    let mut base = String::new();
    for c in label.trim().chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            base.push(c);
        } else if !base.ends_with('-') && !base.is_empty() {
            base.push('-');
        }
    }
    let mut base: String = base.trim_matches('-').chars().take(SLOT_MAX - 3).collect();
    base = base.trim_end_matches('-').to_string();
    if base.is_empty() || base == PRIMARY {
        base = "fleet".to_string();
    }
    let free = |s: &str| s != PRIMARY && !taken.contains(&s);
    if free(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|s| free(s))
        .expect("an unbounded range finds a free slot")
}

/// A `#rrggbb` colour, or `None`.
fn valid_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|h| h.is_ascii_hexdigit())
}

/// The extra fleets as stored. A row whose id is not a valid slot is dropped
/// with a warning rather than failing the rest: its token account would not be
/// one this module minted.
pub fn read_records(store: &Mutex<Store>) -> Result<Vec<ExtraFleet>, IpcError> {
    let raw = lock(store)?.get_setting(FLEETS_KEY)?.unwrap_or_default();
    parse_records(&raw)
}

/// [`read_records`] without the store, so it is a unit test.
pub fn parse_records(raw: &str) -> Result<Vec<ExtraFleet>, IpcError> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<ExtraFleet> = serde_json::from_str(raw).map_err(|e| {
        IpcError::new(
            codes::E_PARSE,
            format!("{FLEETS_KEY} is not a list of fleets ({e})"),
        )
    })?;
    let mut seen: Vec<String> = Vec::new();
    Ok(rows
        .into_iter()
        .filter(|r| {
            let ok = valid_slot(&r.id) && !seen.contains(&r.id);
            if !ok {
                tracing::warn!(fleet = %r.id, "[fleets] dropped a stored fleet with an unusable or repeated id");
            }
            seen.push(r.id.clone());
            ok
        })
        .collect())
}

fn write_records(store: &Mutex<Store>, rows: &[ExtraFleet]) -> Result<(), IpcError> {
    let raw = serde_json::to_string(rows).map_err(|e| {
        IpcError::new(
            codes::E_SERIALIZE,
            format!("the fleets could not be saved: {e}"),
        )
    })?;
    lock(store)?.set_setting(FLEETS_KEY, &raw)?;
    Ok(())
}

/// One extra fleet's events, tagged with its slot id on the way to the
/// window (design D5). The name inside is still one of the `&'static str`
/// names [`super::events::known_event_name`] resolved, or the bridge's own
/// connection / resync events; nothing off the wire can invent one.
pub struct TaggedSink {
    fleet: String,
    inner: Arc<dyn RemoteEventSink>,
}

impl TaggedSink {
    pub fn new(fleet: &str, inner: Arc<dyn RemoteEventSink>) -> Self {
        Self {
            fleet: fleet.to_string(),
            inner,
        }
    }
}

impl RemoteEventSink for TaggedSink {
    fn emit_remote(&self, name: &'static str, payload: Value) {
        self.inner.emit_remote(
            FLEET_EVENT,
            json!({ "fleet": self.fleet, "name": name, "payload": payload }),
        );
    }
}

/// Where an extra fleet's token lives. A trait so the tests hand out
/// in-memory stores; the app hands out [`super::OsTokenStore::for_fleet`].
pub trait FleetTokens: Send + Sync {
    fn for_fleet(&self, fleet: &str) -> Arc<dyn TokenStore>;
}

/// Following an extra fleet's `GET /events`. The real one builds an
/// [`super::events::EventBridge`] in `bootstrap::tasks`, the one file allowed
/// to spawn background work; a test records the call instead.
pub trait FleetLinks: Send + Sync {
    fn follow(
        &self,
        fleet: &str,
        cfg: RemoteConfig,
        hub: Arc<HubBackend>,
        sink: Arc<dyn RemoteEventSink>,
        status: Arc<HubConnectionStatus>,
        cancel: CancellationToken,
    );
}

/// One extra fleet, live.
struct FleetHandle {
    record: ExtraFleet,
    /// `None` while the stored row cannot be used; `unavailable` says why.
    backend: Option<Arc<FleetBackend>>,
    status: Option<Arc<HubConnectionStatus>>,
    unavailable: Option<String>,
    cancel: CancellationToken,
}

/// What the window shows for one fleet. Carries no token, ever: it is an IPC
/// return value.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct FleetInfo {
    /// The slot id; [`PRIMARY`] for the launch-time fleet.
    pub id: String,
    pub label: String,
    /// The hub's base URL; `None` for a standalone primary.
    pub url: Option<String>,
    pub color: String,
    pub primary: bool,
    /// Whether this fleet is this app's own (a standalone primary).
    pub local: bool,
    pub client_name: Option<String>,
    /// The live link, `None` for a fleet that is not connected at all.
    pub connection: Option<HubConnection>,
    /// Why a configured fleet is not in use.
    pub unavailable: Option<String>,
    /// The fleet id the hub's `ready` frame named (design D8), once it has
    /// said one; for a standalone primary, this machine's own.
    pub fleet_id: Option<String>,
    /// The label of the fleet this one is the same hub as, when its `ready`
    /// frame named an id another fleet in this window already has. Its rows
    /// are not shown and its calls are refused: they would be the other's
    /// twice over.
    pub duplicate_of: Option<String>,
}

/// Adding a fleet: what the dialog sends.
#[derive(Deserialize, Debug, Clone)]
pub struct FleetAddArgs {
    pub url: String,
    pub code: String,
    /// Defaults to the hub's host name.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub allow_plaintext: bool,
}

/// The primary as [`Fleets::list`] needs it, built by the command from the
/// launch-time [`super::Backend`] (kept out of this type so the tests need no
/// `Backend`).
#[derive(Debug, Clone)]
pub struct PrimaryView {
    pub url: Option<String>,
    pub client_name: Option<String>,
    pub local: bool,
    pub connection: Option<HubConnection>,
    pub unavailable: Option<String>,
    /// `hub.remote_url` as stored and normalised, which a re-pairing awaiting
    /// a restart can make differ from `url`. Both count for duplicates.
    pub configured_url: Option<String>,
    /// The primary's fleet id: what its hub's `ready` frame named, or this
    /// machine's own when standalone. `None` while unknown.
    pub fleet_id: Option<String>,
}

/// The primary's fleet id, read when it is needed rather than copied once:
/// a hub client learns it from the first `ready` frame, after launch.
pub type PrimaryIdentity = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// Every extra fleet this window shows.
pub struct Fleets {
    store: Arc<Mutex<Store>>,
    tokens: Arc<dyn FleetTokens>,
    sink: Arc<dyn RemoteEventSink>,
    links: Arc<dyn FleetLinks>,
    shutdown: CancellationToken,
    handles: RwLock<BTreeMap<String, FleetHandle>>,
    /// Serialises every read-modify-write of the stored rows, so an add
    /// whose pairing was in flight cannot write back a stale list over a
    /// concurrent add or remove. Never held across an `.await`.
    records_lock: Mutex<()>,
    /// See [`PrimaryIdentity`]; `None` in tests that are not about it.
    primary_identity: Option<PrimaryIdentity>,
}

impl Fleets {
    pub fn new(
        store: Arc<Mutex<Store>>,
        tokens: Arc<dyn FleetTokens>,
        sink: Arc<dyn RemoteEventSink>,
        links: Arc<dyn FleetLinks>,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            store,
            tokens,
            sink,
            links,
            shutdown,
            handles: RwLock::new(BTreeMap::new()),
            records_lock: Mutex::new(()),
            primary_identity: None,
        }
    }

    /// How [`Self::select`] learns the primary's fleet id, to refuse an extra
    /// fleet that turned out to be the same hub (design D8).
    pub fn with_primary_identity(mut self, identity: PrimaryIdentity) -> Self {
        self.primary_identity = Some(identity);
        self
    }

    /// Which fleet `id` is the same hub as, by the fleet ids the hubs named:
    /// the primary, or an extra fleet earlier in slot order. `None` while
    /// either side has not named one.
    fn duplicate_label(
        handles: &BTreeMap<String, FleetHandle>,
        id: &str,
        primary_fleet_id: Option<&str>,
    ) -> Option<String> {
        let mine = handles.get(id)?.status.as_ref()?.fleet_id()?;
        if primary_fleet_id == Some(mine.as_str()) {
            return Some("the primary fleet".to_string());
        }
        handles
            .iter()
            .take_while(|(slot, _)| slot.as_str() != id)
            .find(|(_, h)| {
                h.status
                    .as_ref()
                    .and_then(|s| s.fleet_id())
                    .is_some_and(|other| other == mine)
            })
            .map(|(_, h)| format!("{:?}", h.record.label))
    }

    /// Connect every stored extra fleet. With none stored this reads one
    /// setting and touches no token store (design D11).
    pub fn load(&self) {
        let rows = match read_records(&self.store) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!("[fleets] {}", e.message);
                return;
            }
        };
        for row in rows {
            let handle = self.connect(row);
            self.write().insert(handle.record.id.clone(), handle);
        }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<String, FleetHandle>> {
        self.handles
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, BTreeMap<String, FleetHandle>> {
        self.handles
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Build one fleet's client stack, or say why it cannot be built. Reads
    /// that fleet's token; never anyone else's.
    fn connect(&self, record: ExtraFleet) -> FleetHandle {
        let cancel = self.shutdown.child_token();
        let unusable = |reason: String| FleetHandle {
            record: record.clone(),
            backend: None,
            status: None,
            unavailable: Some(reason),
            cancel: cancel.clone(),
        };
        if let Some(risk) = plaintext_risk(&record.url) {
            if !record.allow_plaintext {
                return unusable(format!(
                    "{risk}. Remove it and add it again with https://, or allow plain http when adding it"
                ));
            }
        }
        let token = match self.tokens.for_fleet(&record.id).get() {
            Ok(Some(t)) => t,
            Ok(None) => return unusable(format!("no client token is stored for {}", record.url)),
            Err(e) => {
                return unusable(format!(
                    "cannot read the client token for {} ({e})",
                    record.url
                ))
            }
        };
        let cfg = RemoteConfig {
            base_url: record.url.clone(),
            token: token.clone(),
            client_name: if record.client_name.is_empty() {
                "desktop".to_string()
            } else {
                record.client_name.clone()
            },
        };
        let sink: Arc<dyn RemoteEventSink> =
            Arc::new(TaggedSink::new(&record.id, Arc::clone(&self.sink)));
        let status = Arc::new(HubConnectionStatus::remote(Arc::clone(&sink), &token));
        // Watched by its OWN status: this hub's contract verdict gates this
        // hub's calls and nobody else's (design D4).
        let hub =
            HubBackend::new(cfg.clone()).watching(Arc::clone(&status) as Arc<dyn ConnectionView>);
        let backend = Arc::new(FleetBackend::over(hub));
        // The bridge's re-list gets a client of its own, watched the same way,
        // as the primary's does in `bootstrap::tasks`.
        let resync_hub = Arc::new(
            HubBackend::new(cfg.clone()).watching(Arc::clone(&status) as Arc<dyn ConnectionView>),
        );
        self.links.follow(
            &record.id,
            cfg,
            resync_hub,
            sink,
            Arc::clone(&status),
            cancel.clone(),
        );
        FleetHandle {
            record,
            backend: Some(backend),
            status: Some(status),
            unavailable: None,
            cancel,
        }
    }

    /// The backend a fleet-scoped command runs against. `None` and
    /// [`PRIMARY`] are the launch-time backend, exactly as before this module
    /// existed; an extra fleet's id is its hub; anything else is refused
    /// rather than guessed.
    pub fn select(
        &self,
        primary: &Arc<FleetBackend>,
        fleet: Option<&str>,
    ) -> Result<Arc<FleetBackend>, IpcError> {
        let id = match fleet.map(str::trim) {
            None | Some("") | Some(PRIMARY) => return Ok(Arc::clone(primary)),
            Some(id) => id,
        };
        let handles = self.read();
        let handle = handles.get(id).ok_or_else(|| {
            IpcError::new(
                codes::E_NOTFOUND,
                format!(
                    "no fleet {id:?} is connected to this window; add it in Settings → Hub & sync"
                ),
            )
        })?;
        let primary_id = self.primary_identity.as_ref().and_then(|f| f());
        if let Some(dup) = Self::duplicate_label(&handles, id, primary_id.as_deref()) {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!(
                    "{} ({}) is the same hub as {dup}, so this window already shows it; \
                     remove one of the two in Settings → Hub & sync",
                    handle.record.label, handle.record.url
                ),
            ));
        }
        match (&handle.backend, &handle.unavailable) {
            (Some(b), _) => Ok(Arc::clone(b)),
            (None, why) => Err(IpcError::new(
                codes::E_HUB_UNAVAILABLE,
                format!(
                    "{} ({}) cannot be used: {}; fix it in Settings → Hub & sync",
                    handle.record.label,
                    handle.record.url,
                    why.as_deref().unwrap_or("it is not connected")
                ),
            )),
        }
    }

    /// The primary, then every extra fleet in slot order.
    pub fn list(&self, primary: &PrimaryView) -> Vec<FleetInfo> {
        let handles = self.read();
        let mut out = vec![primary_info(primary)];
        out.extend(handles.values().map(|h| {
            let fleet_id = h.status.as_ref().and_then(|s| s.fleet_id());
            let duplicate_of =
                Self::duplicate_label(&handles, &h.record.id, primary.fleet_id.as_deref());
            FleetInfo {
                id: h.record.id.clone(),
                label: h.record.label.clone(),
                url: Some(h.record.url.clone()),
                color: h
                    .record
                    .color
                    .clone()
                    .unwrap_or_else(|| PALETTE[1].to_string()),
                primary: false,
                local: false,
                client_name: Some(h.record.client_name.clone()).filter(|n| !n.is_empty()),
                connection: h.status.as_ref().map(|s| s.current()),
                unavailable: h.unavailable.clone(),
                fleet_id,
                duplicate_of,
            }
        }));
        out
    }

    /// Pair with another hub and start showing it.
    ///
    /// Everything that can be refused without spending the code is refused
    /// first, as `hub_pair` does. Then the order is the primary's pairing
    /// order, per slot: forget any token under the new slot, write the row,
    /// store the token. A crash between the last two leaves a row with no
    /// token, which [`Self::connect`] lists as unusable — never a token
    /// beside a URL that did not issue it.
    pub async fn add(
        &self,
        transport: &dyn PairTransport,
        primary: &PrimaryView,
        args: FleetAddArgs,
    ) -> Result<FleetInfo, IpcError> {
        let url = normalise_base_url(args.url.trim()).map_err(|why| {
            IpcError::new(
                codes::E_INVALID,
                format!(
                    "{:?} is not a usable hub address ({why}) — it needs a scheme and a host, \
                     e.g. https://fleet.example.com",
                    args.url.trim()
                ),
            )
        })?;
        let code = args.code.trim().to_string();
        if code.is_empty() {
            return Err(IpcError::new(
                codes::E_INVALID,
                "enter the pairing code from `fleet-hub pair --name <this machine>` on that hub",
            ));
        }
        let records = read_records(&self.store)?;
        // Design D8: one window per fleet.
        if let Some(dup) = duplicate_of(&url, primary, &records) {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("{url} is already shown in this window as {dup}"),
            ));
        }
        if let Some(risk) = plaintext_risk(&url) {
            if !args.allow_plaintext {
                return Err(IpcError::new(
                    codes::E_HUB_PLAINTEXT,
                    format!(
                        "{risk}. Use https://, or add it again with \"send it in the clear \
                         anyway\" if this hop really is already private"
                    ),
                ));
            }
        }
        let label = args
            .label
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                let primary_label = primary_info(primary).label;
                let mut taken: Vec<&str> = records.iter().map(|r| r.label.as_str()).collect();
                taken.push(&primary_label);
                default_label_among(&url, &taken)
            });
        if label.chars().count() > LABEL_MAX {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("a fleet's label is at most {LABEL_MAX} characters"),
            ));
        }
        let chosen_color = match args
            .color
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            Some(c) if valid_color(c) => Some(c.to_ascii_lowercase()),
            Some(c) => {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("{c:?} is not a colour of the form #rrggbb"),
                ))
            }
            None => None,
        };

        let client = pairing::redeem(transport, &url, &code).await?;
        // From here the code is spent. The rows are read again, under the
        // lock: another add or a remove may have written them while the
        // pairing was in flight, and writing back the earlier read would
        // undo it. The slot and the colour are chosen from this read.
        let rows_guard = self.records_lock.lock().unwrap_or_else(|p| p.into_inner());
        let records = read_records(&self.store)?;
        if let Some(dup) = duplicate_of(&url, primary, &records) {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!(
                    "{url} was added as {dup} while this pairing was in flight — the code is \
                     spent, and the fleet is already shown"
                ),
            ));
        }
        let color = chosen_color.unwrap_or_else(|| next_color(&records));
        let taken: Vec<&str> = records.iter().map(|r| r.id.as_str()).collect();
        let id = slot_for(&label, &taken);
        let tokens = self.tokens.for_fleet(&id);
        tokens.clear().map_err(|e| {
            IpcError::new(
                codes::E_IO,
                format!(
                    "paired with {url}, but this machine's secure storage would not clear the \
                     slot for it ({e}), so nothing was kept — the code is spent, so mint a \
                     fresh one on that hub and try again"
                ),
            )
        })?;
        let record = ExtraFleet {
            id: id.clone(),
            label,
            url: url.clone(),
            client_name: client.name.clone(),
            allow_plaintext: args.allow_plaintext,
            color: Some(color),
        };
        let mut next = records.clone();
        next.push(record.clone());
        write_records(&self.store, &next).map_err(|e| {
            IpcError::new(
                codes::E_IO,
                format!(
                    "paired with {url}, but the fleet could not be saved ({}) — no token was \
                     kept; the code is spent, so mint a fresh one on that hub and try again",
                    e.message
                ),
            )
        })?;
        if let Err(e) = tokens.set(&client.token) {
            // Still under the lock, so this read is the list as it stands
            // without this call's row, not a stale snapshot.
            let _ = write_records(&self.store, &records);
            return Err(IpcError::new(
                codes::E_IO,
                format!(
                    "paired with {url}, but this machine's secure storage refused the token \
                     ({e}), so the fleet was not added — the code is spent, so mint a fresh \
                     one on that hub and try again"
                ),
            ));
        }
        drop(rows_guard);
        let handle = self.connect(record);
        self.write().insert(id.clone(), handle);
        self.list(primary)
            .into_iter()
            .find(|f| f.id == id)
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "the fleet vanished as it was added"))
    }

    /// Stop showing an extra fleet and forget its pairing on this machine.
    /// Revokes nothing on its hub (design D10). The bridge stops first, then
    /// the token goes, then the row. A failure part-way (the keychain refuses
    /// to forget the token, the row cannot be written) keeps the fleet
    /// listed as unusable, saying why, so Remove can be tried again rather
    /// than the fleet vanishing until the next launch brings it back
    /// (review w01-11).
    pub fn remove(&self, id: &str) -> Result<(), IpcError> {
        if id == PRIMARY {
            return Err(IpcError::new(
                codes::E_INVALID,
                "the primary fleet is this app's own pairing: Unpair it above instead",
            ));
        }
        let removed = self.write().remove(id);
        if let Some(handle) = &removed {
            handle.cancel.cancel();
        }
        let result = self.forget(id);
        if let (Err(e), Some(handle)) = (&result, removed) {
            if e.code != codes::E_NOTFOUND {
                let unusable = FleetHandle {
                    record: handle.record,
                    backend: None,
                    status: None,
                    unavailable: Some(format!("remove did not finish: {}", e.message)),
                    cancel: self.shutdown.child_token(),
                };
                self.write().insert(id.to_string(), unusable);
            }
        }
        result
    }

    /// [`Self::remove`]'s stored half: the token, then the row.
    fn forget(&self, id: &str) -> Result<(), IpcError> {
        let _rows = self.records_lock.lock().unwrap_or_else(|p| p.into_inner());
        let records = read_records(&self.store)?;
        if !records.iter().any(|r| r.id == id) {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("no fleet {id:?} is configured"),
            ));
        }
        if valid_slot(id) {
            self.tokens.for_fleet(id).clear().map_err(|e| {
                IpcError::new(
                    codes::E_IO,
                    format!("this machine's secure storage refused to forget the token: {e}"),
                )
            })?;
        }
        let rest: Vec<ExtraFleet> = records.into_iter().filter(|r| r.id != id).collect();
        write_records(&self.store, &rest)
    }

    /// The fleet header's Retry: cut the bridge's backoff short, or — for a
    /// fleet that could not be used (a keychain locked at launch) — try to
    /// build it again.
    pub fn retry(&self, id: &str) -> Result<(), IpcError> {
        let record = {
            let handles = self.read();
            let handle = handles.get(id).ok_or_else(|| {
                IpcError::new(codes::E_NOTFOUND, format!("no fleet {id:?} is configured"))
            })?;
            if let Some(status) = &handle.status {
                status.retry_now();
                return Ok(());
            }
            handle.record.clone()
        };
        let handle = self.connect(record);
        if let Some(old) = self.write().insert(id.to_string(), handle) {
            old.cancel.cancel();
        }
        Ok(())
    }

    /// Seed the stored rows, for a test that builds its own `Fleets`.
    #[cfg(test)]
    pub(crate) fn store_records_for_test(&self, rows: &[ExtraFleet]) {
        write_records(&self.store, rows).unwrap();
    }

    /// Every extra fleet's connection reporter, for a test that drives one.
    #[cfg(test)]
    pub(crate) fn status_of(&self, id: &str) -> Option<Arc<HubConnectionStatus>> {
        self.read().get(id).and_then(|h| h.status.clone())
    }

    /// Whether `id`'s bridge has been told to stop.
    #[cfg(test)]
    pub(crate) fn cancelled(&self, id: &str) -> Option<bool> {
        self.read().get(id).map(|h| h.cancel.is_cancelled())
    }
}

/// The primary's row in [`Fleets::list`].
fn primary_info(p: &PrimaryView) -> FleetInfo {
    FleetInfo {
        id: PRIMARY.to_string(),
        label: if p.local {
            "This machine".to_string()
        } else {
            p.url
                .as_deref()
                .or(p.configured_url.as_deref())
                .map(default_label)
                .unwrap_or_else(|| "Primary hub".to_string())
        },
        url: p.url.clone().or_else(|| p.configured_url.clone()),
        color: PALETTE[0].to_string(),
        primary: true,
        local: p.local,
        client_name: p.client_name.clone(),
        connection: p.connection.clone(),
        unavailable: p.unavailable.clone(),
        fleet_id: p.fleet_id.clone(),
        duplicate_of: None,
    }
}

/// The first host label of a URL's host, as a fleet's default label:
/// `https://fleet.papayapos.dev` → `papayapos`, `https://hub.example.com` →
/// `example`, `http://10.0.0.5:8787` → `10.0.0.5`.
pub fn default_label(url: &str) -> String {
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string());
    if host.parse::<std::net::IpAddr>().is_ok() || host.trim_start_matches('[').contains(':') {
        return host;
    }
    let parts: Vec<&str> = host.split('.').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [] => host,
        [one] => one.to_string(),
        // `fleet.rlt.sk` → `rlt`: the registrable name, not the service label.
        [.., name, _tld] => name.to_string(),
    }
}

/// The fleet `url` would duplicate, by label, if any (design D8).
fn duplicate_of(url: &str, primary: &PrimaryView, records: &[ExtraFleet]) -> Option<String> {
    let same = |other: &str| normalise_base_url(other).is_ok_and(|o| o.eq_ignore_ascii_case(url));
    if primary.url.as_deref().is_some_and(same)
        || primary.configured_url.as_deref().is_some_and(same)
    {
        return Some("the primary fleet".to_string());
    }
    records
        .iter()
        .find(|r| same(&r.url))
        .map(|r| format!("{:?}", r.label))
}

/// The first palette colour no extra fleet uses (the primary keeps the first).
/// [`default_label`], unless a fleet this window already shows reads that
/// way (`https://fleet.rlt.sk` and `https://company.rlt.sk` are both `rlt`,
/// review w01-10): then the first host label (`company`), then the whole host.
pub fn default_label_among(url: &str, taken: &[&str]) -> String {
    let base = default_label(url);
    let clash = |l: &str| taken.iter().any(|t| t.trim().eq_ignore_ascii_case(l));
    if !clash(&base) {
        return base;
    }
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string());
    if base == host {
        // An address or a one-label host: nothing shorter tells them apart.
        return base;
    }
    let first = host.split('.').find(|p| !p.is_empty()).unwrap_or(&host);
    if !clash(first) {
        return first.to_string();
    }
    if host.chars().count() <= LABEL_MAX {
        host
    } else {
        base
    }
}

fn next_color(records: &[ExtraFleet]) -> String {
    PALETTE[1..]
        .iter()
        .find(|c| !records.iter().any(|r| r.color.as_deref() == Some(**c)))
        .unwrap_or(&PALETTE[1])
        .to_string()
}

#[cfg(test)]
#[path = "tests_fleets.rs"]
mod tests;
