//! Asset catalog: harness-neutral skills / agents / hooks / MCP servers /
//! plugin refs kept in a git repo, rendered per harness, and compared with
//! what each host actually has installed.
//! Spec: docs/superpowers/specs/2026-09-14-asset-catalog-design.md

pub mod admin;
pub mod author;
pub mod author_session;
pub mod catalogs;
pub mod changesets;
pub mod effective;
pub mod harness;
pub mod harness_set;
pub mod identity;
pub mod import;
pub mod inventory;
pub mod layer;
pub mod model;
pub mod propose;
pub mod registry;
pub mod repo;
pub mod resolve;
pub mod scan_tick;
pub mod sync;
pub mod validate;

// The catalog's `IpcError::code` values live with every other code in
// `ipc_error::codes`; re-exported here so the catalog modules can keep
// saying `catalog::E_CATALOG_GIT`.
pub use crate::ipc_error::codes::{
    E_ASSET_EXISTS, E_ASSET_NOT_FOUND, E_ASSET_UNSUPPORTED, E_CATALOG_GIT,
    E_CATALOG_NOT_CONFIGURED, E_CATALOG_PARSE, E_LINT,
};

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The `registry` module and `HOME` are process-global, so tests that write
/// either must serialise on this lock.
#[cfg(test)]
pub static CATALOG_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take [`CATALOG_TEST_LOCK`] and leave the registry empty. Every test that
/// goes on to install a catalog (directly, or through `configure` + `load`)
/// takes this instead of the raw lock: a registry entry a test installs by
/// hand keeps whatever `id` its author gave it — 0 by convention when
/// nobody sets one — and nothing clears it when the test ends, so without
/// this, an id left behind by one test can outrank (or be outranked by) the
/// real, store-issued catalog a *later* test installs, since both live in
/// the same process-global map. Clearing on the way in, under the same lock
/// that serialises every other write, guarantees each test starts from an
/// empty registry regardless of what ran before it.
#[cfg(test)]
pub(crate) fn lock_registry_for_test() -> std::sync::MutexGuard<'static, ()> {
    let guard = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    registry::clear().expect("registry lock poisoned");
    guard
}

use crate::events::CatalogSummary;
use crate::ipc_error::lock;
use crate::ipc_error::{codes, IpcError};
use crate::ssh::SshClient;
use crate::store::{AssetInventoryRow, CatalogConfigRow, CatalogRow, Store};
use harness::RenderPlan;
use model::{Asset, Kind, Problem};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigureArgs {
    pub repo_path: String,
    pub remote_url: Option<String>,
}

pub(crate) fn expand_home(p: &str) -> String {
    // `~\` too on Windows, where that is how a user types it.
    let rest = p
        .strip_prefix("~/")
        .or_else(|| p.strip_prefix("~\\").filter(|_| cfg!(windows)));
    match (rest, crate::home::home_dir()) {
        (Some(rest), Some(home)) => format!("{}/{rest}", home.display()),
        _ => p.to_string(),
    }
}

pub fn config(store: &Mutex<Store>) -> Result<Option<CatalogConfigRow>, IpcError> {
    Ok(lock(store)?.get_catalog_config()?)
}

pub(crate) fn require_config(store: &Mutex<Store>) -> Result<CatalogConfigRow, IpcError> {
    config(store)?
        .ok_or_else(|| IpcError::new(E_CATALOG_NOT_CONFIGURED, "configure the catalog repo first"))
}

/// Which catalog an authoring call reads and writes (Assets M4, Rulings
/// R21). `Personal` keeps every pre-M4 path exactly — `require_config` for
/// the checkout, `registry::with_personal` for the loaded catalog, `load`
/// to reload — so the desktop's commands behave as before; `Row` is a
/// catalog the caller resolved (`catalog_admin`'s gate, R22).
#[derive(Debug, Clone, Copy)]
pub enum CatalogTarget<'a> {
    Personal,
    Row(&'a CatalogRow),
}

impl CatalogTarget<'_> {
    /// The checkout this target writes.
    pub(crate) fn root(self, store: &Mutex<Store>) -> Result<std::path::PathBuf, IpcError> {
        match self {
            CatalogTarget::Personal => {
                Ok(std::path::PathBuf::from(require_config(store)?.repo_path))
            }
            CatalogTarget::Row(r) => Ok(std::path::PathBuf::from(&r.repo_path)),
        }
    }

    /// Borrow its loaded catalog. Same lock rule as `registry::with_*`: `f`
    /// must never take the store.
    pub(crate) fn with<T>(
        self,
        f: impl FnOnce(&repo::Catalog) -> Result<T, IpcError>,
    ) -> Result<T, IpcError> {
        match self {
            CatalogTarget::Personal => registry::with_personal(f),
            CatalogTarget::Row(r) => registry::with_catalog_row(r, f),
        }
    }

    /// Reload it after a write.
    pub(crate) fn reload(self, store: &Mutex<Store>) -> Result<CatalogSummary, IpcError> {
        match self {
            CatalogTarget::Personal => load(false, store),
            CatalogTarget::Row(r) => load_catalog(r.id, false, store),
        }
    }
}

/// Persist the repo location and clone it if needed. Does not load.
pub fn configure(args: ConfigureArgs, store: &Mutex<Store>) -> Result<CatalogConfigRow, IpcError> {
    let path = expand_home(args.repo_path.trim());
    if path.is_empty() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "repo_path must not be empty",
        ));
    }
    let remote = args
        .remote_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    repo::ensure_repo(std::path::Path::new(&path), remote)?;
    Ok(lock(store)?.set_catalog_config(&path, remote)?)
}

/// (Optionally pull, then) parse catalog `id`'s checkout into the registry
/// and record its HEAD. Always tries, whatever the registry holds.
pub fn load_catalog(id: i64, pull: bool, store: &Mutex<Store>) -> Result<CatalogSummary, IpcError> {
    let row = lock(store)?
        .get_catalog(id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("catalog {id} not found")))?;
    let root = std::path::PathBuf::from(&row.repo_path);
    repo::ensure_repo(&root, row.remote_url.as_deref())?;
    if pull {
        repo::pull(&root)?;
    }
    let mut cat = repo::load_dir(&root)?;
    cat.head = repo::head(&root)?;
    let summary = CatalogSummary {
        head: cat.head.clone(),
        loaded_at: cat.loaded_at,
        asset_count: cat.assets.len(),
        problem_count: cat.problems.len(),
    };
    cat.id = row.id;
    cat.name = row.name.clone();
    cat.org_id = row.org_id;
    // The store guard and the registry lock are never held at the same
    // time: `resolve_preview` (via `with_catalog` → `sync::layers::
    // resolve_for_host`) takes the registry read lock and, from inside
    // that closure, the store lock — the opposite order from what this
    // function used to do (store guard held across `registry::install`).
    // Two callers on opposite lock orders can deadlock each other, so each
    // store/registry access below is its own, non-overlapping critical
    // section. See `registry.rs`'s module doc.
    //
    // Fix round 1, item 3: the store write happens BEFORE the registry
    // install, not after. A failure here (`E_SQLITE` from `rusqlite`, or
    // `E_LOCK` from a poisoned mutex) must propagate without the registry
    // having changed at all — `ensure_fresh` tells this kind of failure
    // apart from a genuine load failure (`is_load_failure`) and would
    // otherwise clobber whatever the registry already held (a good load
    // from a previous pass, say) with a problem entry over what is really
    // just a bookkeeping failure.
    {
        let s = lock(store)?;
        s.set_catalog_head_for(row.id, &summary.head, summary.loaded_at)?;
        s.bus_catalog_loaded(&summary);
    }
    if row.org_id.is_none() {
        registry::install_personal(cat)?;
    } else {
        registry::install(cat)?;
    }
    Ok(summary)
}

/// Load the personal catalog — the desktop's and `catalog set`'s load.
pub fn load(pull: bool, store: &Mutex<Store>) -> Result<CatalogSummary, IpcError> {
    let id = lock(store)?
        .personal_catalog()?
        .ok_or_else(|| IpcError::new(E_CATALOG_NOT_CONFIGURED, "configure the catalog repo first"))?
        .id;
    load_catalog(id, pull, store)
}

/// `catalog_load` with no catalog named (Rulings R18): personal, then every
/// other catalog brought up to date — an org catalog that cannot load stays
/// a problem entry. Answers personal's summary.
pub fn load_all(pull: bool, store: &Mutex<Store>) -> Result<CatalogSummary, IpcError> {
    let summary = load(pull, store)?;
    ensure_fresh(store)?;
    Ok(summary)
}

/// A registry entry for an org catalog that failed to load (Rulings R5):
/// stamped with the store's record as it stood, so [`is_current`] holds —
/// and nothing retries — until that record changes.
pub(crate) fn problem_entry(row: &CatalogRow, err: &IpcError) -> repo::Catalog {
    repo::Catalog {
        id: row.id,
        name: row.name.clone(),
        org_id: row.org_id,
        head: row.head_commit.as_deref().unwrap_or("").to_string(),
        loaded_at: row.last_loaded_at.unwrap_or(0),
        problems: vec![model::Problem {
            path: row.repo_path.clone(),
            message: format!("catalog {} could not be loaded: {}", row.name, err.message),
        }],
        load_error: Some(err.message.clone()),
        load_error_stamp: Some(problem_stamp(row)),
        ..Default::default()
    }
}

/// `repo_path`+`remote_url`, joined so a change in either is detectable —
/// see [`repo::Catalog::load_error_stamp`] (PF13).
fn problem_stamp(row: &CatalogRow) -> String {
    format!(
        "{}\u{0}{}",
        row.repo_path,
        row.remote_url.as_deref().unwrap_or("")
    )
}

/// Whether the registry's `entry` is the load the store last recorded.
fn is_current(entry: &repo::Catalog, row: &CatalogRow) -> bool {
    if entry.load_error.is_some() {
        entry.head == row.head_commit.as_deref().unwrap_or("")
            && entry.loaded_at == row.last_loaded_at.unwrap_or(0)
            && entry.load_error_stamp.as_deref() == Some(problem_stamp(row).as_str())
    } else {
        row.last_loaded_at == Some(entry.loaded_at)
            && row.head_commit.as_deref() == Some(entry.head.as_str())
    }
}

/// Fix round 1, item 3: whether `e` came from reading/parsing the catalog's
/// checkout itself (`ensure_repo`/`pull`/`head`/`load_dir` — `E_CATALOG_GIT`,
/// `E_CATALOG_PARSE`, or `E_IO` for an unreadable checkout) rather than from
/// the store or a lock (`E_SQLITE`, `E_LOCK`) or a row that vanished
/// (`E_NOTFOUND`, a store race between `ensure_fresh`'s own `list_catalogs`
/// and `load_catalog`'s `get_catalog` — `refresh_row` evicts that one, see
/// there). Only a load failure becomes a
/// problem entry in `ensure_fresh`: a store/lock failure must propagate
/// instead, because by the time it can happen (after `load_catalog`'s own
/// repo/parse steps already succeeded) the catalog may be fully loaded and
/// simply not yet installed — see `load_catalog`'s store-before-registry
/// ordering — so treating it as a load failure would clobber a good load,
/// or whatever the registry already held, with a problem entry over what is
/// really just a bookkeeping failure.
pub fn is_load_failure(e: &IpcError) -> bool {
    matches!(
        e.code.as_str(),
        E_CATALOG_GIT | E_CATALOG_PARSE | codes::E_IO
    )
}

/// Bring every configured catalog's registry entry up to the store's record
/// (spec, Runtime). The registry is one process's memory; the configuration
/// and each catalog's last load are in the store, written by `fleet-hub
/// catalog …` in another process — comparing the two is how a running hub
/// notices. An org catalog that cannot load becomes a problem entry and the
/// others still load; a personal failure is returned, as before (R5), after
/// the org catalogs were attempted. Org entries the store no longer has are
/// evicted. A store or lock failure (`is_load_failure` false) propagates
/// immediately instead of becoming a problem entry — see its doc.
pub fn ensure_fresh(store: &Mutex<Store>) -> Result<(), IpcError> {
    let rows = lock(store)?.list_catalogs()?;
    let configured: BTreeSet<i64> = rows.iter().map(|r| r.id).collect();
    registry::evict_org_catalogs_not_in(&configured)?;
    let mut personal_err = None;
    for row in &rows {
        if let Err(e) = refresh_row(row, store)? {
            personal_err = Some(e);
        }
    }
    personal_err.map_or(Ok(()), Err)
}

/// One row of [`ensure_fresh`]'s pass: `Ok(Err(e))` is personal's load
/// failure, kept for the end of the pass (R5); an outer `Err` stops it.
///
/// Final review M-b: an org row listed a moment ago that `load_catalog`
/// no longer finds (`E_NOTFOUND`) was removed meanwhile (`catalog remove`
/// in another process) — evicted, like the next pass would, instead of
/// failing an unrelated asset call.
fn refresh_row(row: &CatalogRow, store: &Mutex<Store>) -> Result<Result<(), IpcError>, IpcError> {
    let current = registry::with_catalogs(|m| {
        Ok(registry::entry_for(m, row).is_some_and(|c| is_current(c, row)))
    })
    .unwrap_or(false);
    if current {
        return Ok(Ok(()));
    }
    match load_catalog(row.id, false, store) {
        Ok(_) => {}
        Err(e) if row.org_id.is_none() => return Ok(Err(e)),
        Err(e) if e.code == codes::E_NOTFOUND => {
            // Only when the row really is gone, not some other not-found.
            if lock(store)?.get_catalog(row.id)?.is_some() {
                return Err(e);
            }
            registry::remove(row.id)?;
        }
        Err(e) if is_load_failure(&e) => {
            tracing::warn!(catalog = %row.name, error = %e.message, "catalog could not be loaded; kept as a problem");
            registry::install(problem_entry(row, &e))?;
        }
        Err(e) => return Err(e),
    }
    Ok(Ok(()))
}

fn with_catalog<T>(f: impl FnOnce(&repo::Catalog) -> Result<T, IpcError>) -> Result<T, IpcError> {
    registry::with_personal(f)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostState {
    pub host_alias: String,
    pub harness: String,
    pub state: String,
    /// Assets M5 (R4): `host` | `catalog` on a drifted managed copy, so a
    /// client that cannot read the inventory (an ungranted hub client) still
    /// sees which side moved. Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift_side: Option<String>,
}

/// `Deserialize` because a hub-client desktop reads this back from the hub's
/// `list_assets` (see `catalog_list_assets`'s verdict).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetSummary {
    pub kind: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub tags: Vec<String>,
    pub hosts: Vec<HostState>,
    /// The identifier this asset installs under when it differs from
    /// `name`; absent on the wire otherwise, so a listing reads the same as
    /// before for every asset that has no `install_as`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_as: Option<String>,
    /// Assets S1b: who may receive this asset. `#[serde(default)]` so an
    /// older hub that predates this field still parses on the desktop side.
    #[serde(default)]
    pub scope: model::Scope,
    /// Assets M5 (R11): the catalog this asset is in — `personal`, or an org
    /// catalog's name. A hub before M5 listed the personal catalog only, so
    /// an absent key reads as `personal`; and `personal` is never written
    /// (fix round 1), so the default, personal-only listing is byte for byte
    /// what it was before M5.
    #[serde(default = "personal_label", skip_serializing_if = "is_personal")]
    pub catalog: String,
}

fn personal_label() -> String {
    catalogs::PERSONAL.to_string()
}

fn is_personal(catalog: &str) -> bool {
    catalog == catalogs::PERSONAL
}

/// Assets M5 (R11): which catalogs a listing spans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingScope {
    /// Every loaded catalog: the desktop, the master, an unbound full
    /// person device (the audience of `list_catalogs`, M3 PF15).
    Every,
    /// The personal catalog only, as before M5: per-host tokens, org-bound
    /// and readonly clients.
    Personal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetListing {
    pub head: String,
    pub loaded_at: i64,
    pub assets: Vec<AssetSummary>,
    pub unmanaged: Vec<AssetInventoryRow>,
    pub problems: Vec<Problem>,
    /// Assets S1a: `unmanaged` rows grouped per (kind, name) and classified.
    /// `None` (and thus absent on the wire) for an old hub whose reply has
    /// no `identities` key — never `Some(vec![])`, which would look like a
    /// hub that ran the grouping and found nothing. The frontend's
    /// `identitiesOf` falls back to client-side grouping only when the key
    /// is truly missing; a `#[serde(default)]` `Vec` would silently collapse
    /// that distinction into an empty-but-present list, which is the bug
    /// this type is guarding against (a hub-client desktop losing the
    /// "on hosts, not in catalog" section for anyone talking to an old hub).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identities: Option<Vec<identity::AssetIdentity>>,
}

/// Which hosts hold `cat`'s asset `kind/name`, and in what state. `unmanaged`
/// and `orphan` rows are excluded by name: neither describes a catalog asset
/// (they are what `AssetListing::unmanaged` lists instead), and an orphan
/// shares a `(kind, name)` with nothing in the catalog anyway. Carry 3
/// (Assets M5, R12): a row is `cat`'s when the scan stamped `cat`'s id on
/// it, or — a managed row scanned before Assets M2 stamped any — when it has
/// none and `cat` is the personal catalog. So `personal/x` and `acme/x`
/// never show each other's hosts.
fn host_states(
    rows: &[AssetInventoryRow],
    cat: &repo::Catalog,
    kind: Kind,
    name: &str,
) -> Vec<HostState> {
    rows.iter()
        .filter(|r| {
            r.kind == kind.as_str()
                && r.name == name
                && r.state != "unmanaged"
                && r.state != "orphan"
        })
        .filter(|r| match r.catalog_id {
            Some(id) => id == cat.id,
            None => cat.org_id.is_none(),
        })
        .map(|r| HostState {
            host_alias: r.host_alias.clone(),
            harness: r.harness.clone(),
            state: r.state.clone(),
            drift_side: r.drift_side.clone(),
        })
        .collect()
}

pub fn inventory(store: &Mutex<Store>) -> Result<Vec<AssetInventoryRow>, IpcError> {
    Ok(lock(store)?.list_inventory()?)
}

/// The personal catalog's assets with their per-host state from the last
/// scan, plus the unmanaged and orphan rows and its problems — the listing
/// as it was before Assets M5. Every catalog's is opt-in (fix round 1):
/// [`list_assets_in`] with [`ListingScope::Every`], which the M5 desktop
/// asks for and the MCP tool grants by caller
/// (`mcp::tools::assets::listing_scope`).
pub fn list_assets(store: &Mutex<Store>) -> Result<AssetListing, IpcError> {
    list_assets_in(store, ListingScope::Personal)
}

/// [`list_assets`] over `scope`'s catalogs, personal first
/// ([`registry::in_order`]). Store first (the inventory), then the
/// registry, never the other way round; nothing inside the registry closure
/// takes the store or the registry again. `head`, `loaded_at` and
/// `problems` are the personal catalog's; an org catalog that failed to
/// load (a problem entry) lists nothing.
pub fn list_assets_in(store: &Mutex<Store>, scope: ListingScope) -> Result<AssetListing, IpcError> {
    require_config(store)?;
    let rows = inventory(store)?;
    let identities = identity::group_identities(&rows);
    registry::with_catalogs(|m| {
        let personal = m.values().find(|c| c.org_id.is_none()).ok_or_else(|| {
            IpcError::new(
                E_CATALOG_NOT_CONFIGURED,
                "catalog not loaded; call catalog_load",
            )
        })?;
        let mut assets = Vec::new();
        for cat in registry::in_order(m) {
            if cat.load_error.is_some() || (scope == ListingScope::Personal && cat.org_id.is_some())
            {
                continue;
            }
            let label = effective::label_of(cat.org_id, &cat.name);
            assets.extend(cat.assets.iter().map(|a| AssetSummary {
                kind: a.kind().as_str().to_string(),
                name: a.header.name.clone(),
                version: a.header.version.clone(),
                description: a.header.description.clone(),
                tags: a.header.tags.clone(),
                hosts: host_states(&rows, cat, a.kind(), &a.header.name),
                install_as: a.header.install_as.clone(),
                scope: a.header.scope,
                catalog: label.clone(),
            }));
        }
        Ok(AssetListing {
            head: personal.head.clone(),
            loaded_at: personal.loaded_at,
            assets,
            // `unmanaged` is the wire name for "installed on a host but not
            // a catalog asset". An `orphan` — a past sync's manifest entry
            // whose asset the catalog has dropped — belongs in the same
            // list, or it would be invisible in the UI: it is not a catalog
            // asset any more, so it has no `AssetSummary` row to hang a
            // `HostState` off. The frontend groups the list by `state`.
            unmanaged: rows
                .iter()
                .filter(|r| r.state == "unmanaged" || r.state == "orphan")
                .cloned()
                .collect(),
            problems: personal.problems.clone(),
            identities: Some(identities),
        })
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preview {
    pub harness: String,
    pub plan: Option<RenderPlan>,
    pub unsupported: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetDetail {
    pub asset: Asset,
    pub previews: Vec<Preview>,
    pub hosts: Vec<HostState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "ImportAssetsParams")]
pub struct ImportArgs {
    /// Any host alias; `local` reads this machine's config.
    pub host_alias: String,
    /// Report, write nothing.
    #[serde(default)]
    pub dry_run: bool,
    /// Only these `<kind>:<name>` assets; empty imports everything.
    #[serde(default)]
    pub only: Vec<String>,
}

/// `host_alias` must be a registered, non-hidden host before `import_host`
/// dials it over SSH. Security fix round 1, IMPORTANT 2: without this, any
/// string a caller sends reaches `run_host_script`/`ssh` unchecked, so a
/// caller who may drive `import_assets` at all (the master, or a client
/// granted the catalog) could make the hub dial an arbitrary
/// hostname/address that was never added to the fleet. Looked up before any
/// `.await` in the caller, so the store guard is never held across one.
fn require_dialable_host(store: &Mutex<Store>, host_alias: &str) -> Result<(), IpcError> {
    let known = lock(store)?.get_host_row(host_alias)?;
    match known {
        Some(h) if !h.hidden => Ok(()),
        _ => Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("host {host_alias} not found"),
        )),
    }
}

/// Import from a host's Claude config. `local` reads this machine's files;
/// any other host is copied over SSH into a temporary directory first, with
/// fleet's own hook, MCP server and skills stripped out before anything is
/// read (`import::scrub_fleet_entries` — see its doc comment for why that
/// matters: each host's fleet token differs from the controller's, so the
/// usual `scrub_token` redaction cannot catch it).
pub async fn import_host(
    args: ImportArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    fleet_token: Option<&str>,
) -> Result<import::ImportReport, IpcError> {
    import_host_into(CatalogTarget::Personal, args, store, ssh, fleet_token).await
}

/// [`import_host`] into `target`'s checkout (Assets M4: `catalog_admin
/// import_host` with a `catalog`, and a card's apply).
pub async fn import_host_into(
    target: CatalogTarget<'_>,
    args: ImportArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    fleet_token: Option<&str>,
) -> Result<import::ImportReport, IpcError> {
    let repo = target.root(store)?;
    if args.host_alias == "local" {
        crate::service::hub::ensure_local_allowed(&args.host_alias)?;
        let src = import::ImportSources::for_local()?;
        return import::import_claude_only(
            &src,
            &repo,
            "local",
            fleet_token,
            args.dry_run,
            &args.only,
        );
    }
    require_dialable_host(store, &args.host_alias)?;
    let script = import::REMOTE_SOURCES_SCRIPT;
    let out = inventory::run_host_script(ssh, &args.host_alias, script).await?;
    let tmp = tempfile::tempdir()
        .map_err(|e| IpcError::new(codes::E_IO, format!("remote import: {e}")))?;
    let src = import::parse_remote_dump(&out, tmp.path())?;
    import::scrub_fleet_entries(&src)?;
    import::import_claude_only(
        &src,
        &repo,
        &args.host_alias,
        fleet_token,
        args.dry_run,
        &args.only,
    )
}

pub fn get_asset(kind: Kind, name: &str, store: &Mutex<Store>) -> Result<AssetDetail, IpcError> {
    get_asset_in(CatalogTarget::Personal, kind, name, store)
}

/// [`get_asset`] out of `target`'s loaded catalog (Assets M4).
pub fn get_asset_in(
    target: CatalogTarget<'_>,
    kind: Kind,
    name: &str,
    store: &Mutex<Store>,
) -> Result<AssetDetail, IpcError> {
    let rows = inventory(store)?;
    target.with(|cat| {
        let asset = cat.find(kind, name).ok_or_else(|| {
            IpcError::new(
                E_ASSET_NOT_FOUND,
                format!("{} {name} is not in the catalog", kind.as_str()),
            )
        })?;
        let previews = harness::all()
            .iter()
            .map(|h| match h.render(asset) {
                Ok(plan) => Preview {
                    harness: h.id().into(),
                    plan: Some(plan),
                    unsupported: None,
                },
                Err(u) => Preview {
                    harness: h.id().into(),
                    plan: None,
                    unsupported: Some(u.into_ipc().message),
                },
            })
            .collect();
        Ok(AssetDetail {
            asset: asset.clone(),
            previews,
            hosts: host_states(&rows, cat, kind, name),
        })
    })
}

/// The catalog's layer definitions plus every host's stored assignment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerListing {
    pub layers: Vec<layer::Layer>,
    pub hosts: Vec<crate::store::HostLayerRow>,
}

/// One catalog's layer definitions plus every host's active assignment IN
/// THAT CATALOG (M2 carry 7: the rows of other catalogs no longer appear
/// next to definitions they do not belong to). Only active rows are part of
/// an assignment — the same rule `get_host_layers` (and therefore
/// resolution) applies. The store call stays "all rows" because callers
/// such as `delete_host`'s checks need exactly that.
pub fn list_layers_for(row: &CatalogRow, store: &Mutex<Store>) -> Result<LayerListing, IpcError> {
    let hosts: Vec<_> = lock(store)?
        .list_all_host_layers()?
        .into_iter()
        .filter(|r| r.active && r.catalog_id == row.id)
        .collect();
    registry::with_catalog_row(row, |cat| {
        Ok(LayerListing {
            layers: cat.layers.iter().cloned().collect(),
            hosts,
        })
    })
}

/// The personal catalog's layers and assignments.
pub fn list_layers(store: &Mutex<Store>) -> Result<LayerListing, IpcError> {
    let personal = lock(store)?.personal_catalog()?;
    match personal {
        Some(row) => list_layers_for(&row, store),
        None => with_catalog(|cat| {
            Ok(LayerListing {
                layers: cat.layers.iter().cloned().collect(),
                hosts: Vec::new(),
            })
        }),
    }
}

/// `host_alias` must be a registered host. Without this check, a typo'd
/// alias either trips the `host_layers` foreign key (`set_host_layers`) or —
/// worse — silently resolves to the WHOLE catalog, since `resolve_for_host`
/// treats "no assignment rows" as "no layering" for backward compatibility
/// and cannot tell a nonexistent host from an unassigned one.
fn require_host_exists(store: &Mutex<Store>, host_alias: &str) -> Result<(), IpcError> {
    require_host(&*lock(store)?, host_alias)
}

/// [`require_host_exists`] under a guard the caller already holds
/// (`catalogs::admit`/`unadmit`).
pub(crate) fn require_host(s: &Store, host_alias: &str) -> Result<(), IpcError> {
    match s.get_host_row(host_alias)? {
        Some(_) => Ok(()),
        None => Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("host {host_alias} not found"),
        )),
    }
}

/// Compute the effective asset set for `host_alias`, with provenance.
/// Nothing is written.
///
/// Assets M2: built from [`effective::effective_for_host`] rather than the
/// single-catalog `sync::layers::resolve_for_host`, so the preview reflects
/// every catalog the host accepts (its org catalog, and the shared slice of
/// personal), not personal alone — `refused` surfaces a scope boundary or a
/// cross-catalog collision the same way `plan_sync` does, and `excluded`
/// (merged across every accepted catalog by `effective::compose`) is exactly
/// what `sync::layers::resolve_for_host` would answer on the single-catalog
/// path. `Resolution`'s shape is unchanged otherwise, so a personal-only
/// host previews exactly as before.
pub fn resolve_preview(
    host_alias: &str,
    store: &Mutex<Store>,
) -> Result<resolve::Resolution, IpcError> {
    crate::validate::host_alias(host_alias)?;
    require_host_exists(store, host_alias)?;
    let eff = effective::effective_for_host(store, host_alias)?;
    Ok(resolve::Resolution {
        catalog: eff.catalog,
        provenance: eff.provenance,
        excluded: eff.excluded,
        layered: eff.layered,
        refused: eff.refused,
        withheld: eff.withheld,
        held_back: eff.held_back,
    })
}

/// A layer named by `set_host_layers` must exist in the loaded catalog, and
/// on the axis the caller is assigning it to (role vs. context). Without
/// this, a bad name reaches `Store::set_host_layers` and surfaces as a raw
/// SQLite constraint failure instead of a clear error; a name that exists
/// but on the wrong axis would otherwise let the store and the catalog
/// silently disagree about what a layer is.
fn check_layer_axis(cat: &repo::Catalog, name: &str, want: layer::Axis) -> Result<(), IpcError> {
    match cat.layers.get(name) {
        None => Err(IpcError::new(
            codes::E_INVALID,
            format!("layer '{name}' is not defined in the loaded catalog"),
        )),
        Some(l) if l.axis != want => Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "layer '{name}' is a {} layer, not a {}",
                l.axis.as_str(),
                want.as_str()
            ),
        )),
        Some(_) => Ok(()),
    }
}

/// `host_layers`'s primary key is `(host_alias, layer_name)`, so two rows
/// for the same host can never share a layer name — a duplicate context, or
/// a context that repeats the role, both hit that constraint. Catching it
/// here gives the caller a clear `E_INVALID` naming the offending layer
/// instead of a raw SQLite constraint violation from `Store::set_host_layers`.
fn check_no_name_collision(role: Option<&str>, contexts: &[&str]) -> Result<(), IpcError> {
    let mut seen: Vec<&str> = Vec::new();
    for c in contexts {
        if Some(*c) == role {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("layer '{c}' is assigned as both the role and a context"),
            ));
        }
        if seen.contains(c) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("layer '{c}' is assigned as a context more than once"),
            ));
        }
        seen.push(c);
    }
    Ok(())
}

/// Replace a host's assignment in one catalog: one optional role plus
/// contexts in application order, checked against THAT catalog's layer
/// definitions. Edits fleet state only; catalog files are never written.
/// Answers the host's rows in that catalog only (M2 carry 7).
pub fn set_host_layers_for(
    host_alias: &str,
    row: &CatalogRow,
    role: Option<&str>,
    contexts: &[&str],
    store: &Mutex<Store>,
) -> Result<Vec<crate::store::HostLayerRow>, IpcError> {
    crate::validate::host_alias(host_alias)?;
    require_host_exists(store, host_alias)?;
    check_no_name_collision(role, contexts)?;
    registry::with_catalog_row(row, |cat| {
        if let Some(r) = role {
            check_layer_axis(cat, r, layer::Axis::Role)?;
        }
        for c in contexts {
            check_layer_axis(cat, c, layer::Axis::Context)?;
        }
        Ok(())
    })?;
    let s = lock(store)?;
    s.set_host_layers_for(host_alias, row.id, role, contexts)?;
    Ok(s.get_host_layers_for(host_alias, row.id)?)
}

/// [`set_host_layers_for`] in the personal catalog — the desktop's command.
/// The host is validated once, by [`set_host_layers_for`] (PF14).
pub fn set_host_layers(
    host_alias: &str,
    role: Option<&str>,
    contexts: &[&str],
    store: &Mutex<Store>,
) -> Result<Vec<crate::store::HostLayerRow>, IpcError> {
    let row = lock(store)?.personal_catalog()?.ok_or_else(|| {
        IpcError::new(
            E_CATALOG_NOT_CONFIGURED,
            "catalog not loaded; call catalog_load",
        )
    })?;
    set_host_layers_for(host_alias, &row, role, contexts, store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::sync::Mutex;

    fn repo_with_one_skill(tag: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("fleet-catalog-svc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("skills/s")).unwrap();
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        std::fs::write(
            root.join("skills/s/asset.yaml"),
            "kind: skill\nname: s\ndescription: d\n",
        )
        .unwrap();
        std::fs::write(root.join("skills/s/body.md"), "b\n").unwrap();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        root
    }

    /// Two skills: `s` has no `scope` key (defaults to private), `shared-s`
    /// is explicitly `scope: shared`.
    fn repo_with_scoped_assets(tag: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("fleet-catalog-svc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("skills/s")).unwrap();
        std::fs::create_dir_all(root.join("skills/shared-s")).unwrap();
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        std::fs::write(
            root.join("skills/s/asset.yaml"),
            "kind: skill\nname: s\ndescription: d\n",
        )
        .unwrap();
        std::fs::write(root.join("skills/s/body.md"), "b\n").unwrap();
        std::fs::write(
            root.join("skills/shared-s/asset.yaml"),
            "kind: skill\nname: shared-s\ndescription: d\nscope: shared\n",
        )
        .unwrap();
        std::fs::write(root.join("skills/shared-s/body.md"), "b\n").unwrap();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        root
    }

    /// Like `repo_with_one_skill`, plus a `core` role layer (member: `s`
    /// only — NOT `other`), an `extra` context layer (no members), and a
    /// second skill `other` that no layer ever names. `other` is what makes
    /// the happy-path resolve test able to fail: with only one asset in the
    /// catalog, a resolved count of 1 holds whether layering ran, was
    /// ignored, or fell back to the whole catalog.
    fn repo_with_layers(tag: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("fleet-catalog-svc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("skills/s")).unwrap();
        std::fs::create_dir_all(root.join("skills/other")).unwrap();
        std::fs::create_dir_all(root.join("layers")).unwrap();
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        std::fs::write(
            root.join("skills/s/asset.yaml"),
            "kind: skill\nname: s\ndescription: d\n",
        )
        .unwrap();
        std::fs::write(root.join("skills/s/body.md"), "b\n").unwrap();
        std::fs::write(
            root.join("skills/other/asset.yaml"),
            "kind: skill\nname: other\ndescription: d\n",
        )
        .unwrap();
        std::fs::write(root.join("skills/other/body.md"), "b\n").unwrap();
        std::fs::write(
            root.join("layers/core.yaml"),
            "kind: layer\nname: core\naxis: role\nmembers:\n  - skill/s\n",
        )
        .unwrap();
        std::fs::write(
            root.join("layers/extra.yaml"),
            "kind: layer\nname: extra\naxis: context\n",
        )
        .unwrap();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        root
    }

    fn configured_store_with_layers(tag: &str) -> Mutex<Store> {
        let root = repo_with_layers(tag);
        let store = Mutex::new(Store::open_in_memory().unwrap());
        store.lock().unwrap().upsert_host("local").unwrap();
        configure(
            ConfigureArgs {
                repo_path: root.to_string_lossy().into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        load(false, &store).unwrap();
        store
    }

    /// A hub never calls `load` itself: `fleet-hub catalog set|reload` do,
    /// in another process. `ensure_fresh` is how the running hub catches up
    /// — loading when it has nothing, reloading after a re-point, and
    /// leaving a copy that matches the store's record alone.
    #[test]
    fn ensure_fresh_follows_the_stores_record() {
        let _g = lock_registry_for_test();
        let store = Mutex::new(Store::open_in_memory().unwrap());
        // Nothing configured: a no-op, and the listing still says why.
        ensure_fresh(&store).unwrap();
        assert_eq!(
            list_assets(&store).unwrap_err().code,
            E_CATALOG_NOT_CONFIGURED
        );

        // Configured and loaded by another process, never in this one.
        let one = repo_with_one_skill("fresh1");
        configure(
            ConfigureArgs {
                repo_path: one.to_string_lossy().into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        load(false, &store).unwrap();
        registry::clear().unwrap();
        ensure_fresh(&store).unwrap();
        assert_eq!(list_assets(&store).unwrap().assets.len(), 1);

        // Matches the record: left alone (the emptied copy stays empty).
        let mut cat = registry::personal().unwrap().unwrap();
        cat.assets.clear();
        registry::install(cat).unwrap();
        ensure_fresh(&store).unwrap();
        assert!(list_assets(&store).unwrap().assets.is_empty());

        // Re-pointed elsewhere: reloaded from the new path.
        let two = repo_with_layers("fresh2");
        configure(
            ConfigureArgs {
                repo_path: two.to_string_lossy().into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        ensure_fresh(&store).unwrap();
        assert_eq!(list_assets(&store).unwrap().assets.len(), 2);
        registry::clear().unwrap();
    }

    fn acme_org(store: &Mutex<Store>) -> i64 {
        store
            .lock()
            .unwrap()
            .add_org("acme", None, false)
            .unwrap()
            .id
    }

    /// Assets M3: `ensure_fresh` loads every configured catalog; an org
    /// catalog whose checkout cannot be read is kept as a problem entry while
    /// the others load (spec, Runtime), and is not retried until the store's
    /// record changes (Rulings R5); an explicit `load_catalog` always tries;
    /// a catalog removed from the store leaves the registry.
    #[test]
    fn ensure_fresh_loads_every_catalog_and_keeps_a_broken_one_as_a_problem() {
        let _g = lock_registry_for_test();
        let store = Mutex::new(Store::open_in_memory().unwrap());
        configure(
            ConfigureArgs {
                repo_path: repo_with_one_skill("m3-personal").to_string_lossy().into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        let org = acme_org(&store);
        let acme_repo = repo_with_one_skill("m3-acme");
        let broken_path = std::env::temp_dir().join(format!(
            "fleet-catalog-svc-m3-broken-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&broken_path);
        let (acme, broken) = {
            let s = store.lock().unwrap();
            (
                s.upsert_catalog("acme", &acme_repo.to_string_lossy(), None, Some(org))
                    .unwrap(),
                s.upsert_catalog("broken", &broken_path.to_string_lossy(), None, Some(org))
                    .unwrap(),
            )
        };

        ensure_fresh(&store).unwrap();
        assert_eq!(registry::personal().unwrap().unwrap().assets.len(), 1);
        let a = registry::get(acme.id).unwrap().expect("acme loaded");
        assert_eq!(
            (
                a.name.as_str(),
                a.org_id,
                a.assets.len(),
                a.load_error.is_none()
            ),
            ("acme", Some(org), 1, true)
        );
        let b = registry::get(broken.id)
            .unwrap()
            .expect("a problem entry, not a gap");
        assert!(
            b.load_error
                .as_deref()
                .unwrap_or_default()
                .contains("not a git repository"),
            "{:?}",
            b.load_error
        );
        assert!(b.assets.is_empty());
        assert_eq!(b.problems.len(), 1);

        // A checkout appearing at the path is not picked up by the catch-up
        // while the store's record stands…
        std::fs::rename(repo_with_one_skill("m3-broken-fixed"), &broken_path).unwrap();
        ensure_fresh(&store).unwrap();
        assert!(registry::get(broken.id)
            .unwrap()
            .unwrap()
            .load_error
            .is_some());
        // …but an explicit load always tries.
        load_catalog(broken.id, false, &store).unwrap();
        assert!(registry::get(broken.id)
            .unwrap()
            .unwrap()
            .load_error
            .is_none());

        store.lock().unwrap().remove_catalog("acme").unwrap();
        ensure_fresh(&store).unwrap();
        assert!(
            registry::get(acme.id).unwrap().is_none(),
            "removed elsewhere: evicted"
        );
        registry::clear().unwrap();
    }

    /// Final review M-b: a row `ensure_fresh` listed, removed before its
    /// load (another process's `catalog remove`), is evicted — not an error
    /// for the unrelated call that triggered the pass.
    #[test]
    fn a_catalog_removed_between_list_and_load_is_evicted_not_an_error() {
        let _g = lock_registry_for_test();
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let org = acme_org(&store);
        let acme = store
            .lock()
            .unwrap()
            .upsert_catalog(
                "acme",
                &repo_with_one_skill("m3-removed-meanwhile").to_string_lossy(),
                None,
                Some(org),
            )
            .unwrap();
        // A stale entry for it, so the pass tries to load it.
        registry::install_for_test(repo::Catalog {
            id: acme.id,
            name: "acme".into(),
            org_id: Some(org),
            head: "stale".into(),
            ..Default::default()
        })
        .unwrap();
        let row = store
            .lock()
            .unwrap()
            .list_catalogs()
            .unwrap()
            .into_iter()
            .find(|r| r.id == acme.id)
            .unwrap();
        store.lock().unwrap().remove_catalog("acme").unwrap();
        assert!(refresh_row(&row, &store).unwrap().is_ok());
        assert!(
            registry::get(acme.id).unwrap().is_none(),
            "removed meanwhile: evicted"
        );
        registry::clear().unwrap();
    }

    /// Parity (R5): personal never becomes a problem entry — its failure is
    /// still `ensure_fresh`'s error — but the org catalogs are attempted too.
    #[test]
    fn a_personal_load_failure_is_still_an_error_but_the_org_catalogs_load() {
        let _g = lock_registry_for_test();
        let store = Mutex::new(Store::open_in_memory().unwrap());
        store
            .lock()
            .unwrap()
            .set_catalog_config("/nonexistent/fleet-m3-personal", None)
            .unwrap();
        let org = acme_org(&store);
        let acme = store
            .lock()
            .unwrap()
            .upsert_catalog(
                "acme",
                &repo_with_one_skill("m3-org-ok").to_string_lossy(),
                None,
                Some(org),
            )
            .unwrap();
        assert_eq!(ensure_fresh(&store).unwrap_err().code, E_CATALOG_GIT);
        assert!(registry::personal().unwrap().is_none());
        assert!(registry::get(acme.id)
            .unwrap()
            .is_some_and(|c| c.load_error.is_none()));
        registry::clear().unwrap();
    }

    /// R18: `load_all` loads personal and catches every other catalog up.
    #[test]
    fn load_all_loads_personal_and_catches_up_the_rest() {
        let _g = lock_registry_for_test();
        let store = Mutex::new(Store::open_in_memory().unwrap());
        configure(
            ConfigureArgs {
                repo_path: repo_with_one_skill("m3-all-p").to_string_lossy().into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        let org = acme_org(&store);
        let acme = store
            .lock()
            .unwrap()
            .upsert_catalog(
                "acme",
                &repo_with_one_skill("m3-all-a").to_string_lossy(),
                None,
                Some(org),
            )
            .unwrap();
        let summary = load_all(false, &store).unwrap();
        assert_eq!(summary.asset_count, 1, "the personal catalog's summary");
        assert!(registry::get(acme.id).unwrap().is_some());
        registry::clear().unwrap();
    }

    /// PF13: the freshness stamp on a problem entry also tracks `repo_path`
    /// / `remote_url`, not only `head_commit`/`last_loaded_at` — both of
    /// which `upsert_catalog` clears to `NULL` on *every* re-point, so a
    /// catalog that has never once loaded looks identical before and after
    /// unless the path itself is compared too. Without that, a `catalog
    /// add` re-point of a broken catalog would never be retried by a
    /// running hub.
    #[test]
    fn ensure_fresh_retries_a_broken_catalog_once_its_repo_path_changes() {
        let _g = lock_registry_for_test();
        let store = Mutex::new(Store::open_in_memory().unwrap());
        configure(
            ConfigureArgs {
                repo_path: repo_with_one_skill("pf13-personal")
                    .to_string_lossy()
                    .into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        let org = acme_org(&store);
        let broken_path = std::env::temp_dir().join(format!(
            "fleet-catalog-svc-pf13-broken-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&broken_path);
        let broken = store
            .lock()
            .unwrap()
            .upsert_catalog("broken", &broken_path.to_string_lossy(), None, Some(org))
            .unwrap();

        ensure_fresh(&store).unwrap();
        assert!(registry::get(broken.id)
            .unwrap()
            .unwrap()
            .load_error
            .is_some());

        // Re-point to a real checkout: `head_commit`/`last_loaded_at` stay
        // `NULL` (as they already were), but `repo_path` itself changed.
        let fixed = repo_with_one_skill("pf13-broken-fixed");
        store
            .lock()
            .unwrap()
            .upsert_catalog("broken", &fixed.to_string_lossy(), None, Some(org))
            .unwrap();

        ensure_fresh(&store).unwrap();
        let after = registry::get(broken.id)
            .unwrap()
            .expect("still in the registry");
        assert!(after.load_error.is_none(), "{:?}", after.load_error);
        assert_eq!(after.assets.len(), 1);
        registry::clear().unwrap();
    }

    /// Fix round 1, item 6: the freshness stamp also catches a
    /// `remote_url`-only change — `repo_path` stays put, but a broken
    /// catalog that is given a remote it can finally clone from must still
    /// be retried.
    #[test]
    fn ensure_fresh_retries_a_broken_catalog_once_its_remote_url_changes() {
        let _g = lock_registry_for_test();
        let store = Mutex::new(Store::open_in_memory().unwrap());
        configure(
            ConfigureArgs {
                repo_path: repo_with_one_skill("pf13r-personal")
                    .to_string_lossy()
                    .into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        let org = acme_org(&store);
        let broken_path = std::env::temp_dir().join(format!(
            "fleet-catalog-svc-pf13r-broken-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&broken_path);
        let broken = store
            .lock()
            .unwrap()
            .upsert_catalog("broken", &broken_path.to_string_lossy(), None, Some(org))
            .unwrap();

        ensure_fresh(&store).unwrap();
        assert!(registry::get(broken.id)
            .unwrap()
            .unwrap()
            .load_error
            .is_some());

        // `repo_path` is untouched; only `remote_url` changes, from `None`
        // to a clonable local source.
        let remote = repo_with_one_skill("pf13r-remote");
        store
            .lock()
            .unwrap()
            .upsert_catalog(
                "broken",
                &broken_path.to_string_lossy(),
                Some(&remote.to_string_lossy()),
                Some(org),
            )
            .unwrap();

        ensure_fresh(&store).unwrap();
        let after = registry::get(broken.id)
            .unwrap()
            .expect("still in the registry");
        assert!(after.load_error.is_none(), "{:?}", after.load_error);
        assert_eq!(after.assets.len(), 1);
        registry::clear().unwrap();
    }

    /// Fix round 1, item 3: `is_load_failure` is what decides whether
    /// `ensure_fresh` turns a `load_catalog` failure into a problem entry.
    /// Only the catalog-checkout codes qualify; a store write failure, a
    /// poisoned lock, or a row that vanished between `list_catalogs` and
    /// `get_catalog` must all propagate instead.
    #[test]
    fn is_load_failure_only_matches_checkout_codes() {
        let checkout = |code: &str| is_load_failure(&IpcError::new(code, "x"));
        assert!(checkout(E_CATALOG_GIT));
        assert!(checkout(E_CATALOG_PARSE));
        assert!(checkout(codes::E_IO));
        assert!(!checkout(codes::E_SQLITE));
        assert!(!checkout(codes::E_LOCK));
        assert!(!checkout(codes::E_NOTFOUND));
    }

    /// Fix round 1, item 3: a failure writing the store's record
    /// (`set_catalog_head_for`, after the checkout has already parsed fine)
    /// must propagate out of `ensure_fresh` as-is — never as a problem entry
    /// — and must leave the registry exactly as it was (here: never
    /// installed at all, since `load_catalog` now writes the store before
    /// touching the registry). `PRAGMA query_only` fails every write on the
    /// store's connection while reads (the row lookup, `list_catalogs`)
    /// keep working, which reproduces "the checkout loaded fine, but the
    /// store write failed" without needing to race a second thread.
    #[test]
    fn ensure_fresh_propagates_a_store_write_failure_without_a_problem_entry() {
        let _g = lock_registry_for_test();
        let store = Mutex::new(Store::open_in_memory().unwrap());
        configure(
            ConfigureArgs {
                repo_path: repo_with_one_skill("pf3-personal").to_string_lossy().into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        load(false, &store).unwrap();
        let org = acme_org(&store);
        let acme = store
            .lock()
            .unwrap()
            .upsert_catalog(
                "acme",
                &repo_with_one_skill("pf3-acme").to_string_lossy(),
                None,
                Some(org),
            )
            .unwrap();

        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute_batch("PRAGMA query_only = ON;")
            .unwrap();

        let err = ensure_fresh(&store).unwrap_err();
        assert_eq!(err.code, codes::E_SQLITE, "{}", err.message);
        assert!(
            registry::get(acme.id).unwrap().is_none(),
            "a store-write failure must not install a problem entry"
        );
        registry::clear().unwrap();
    }

    /// Task 2: `load` sets the loaded catalog's identity from the store's
    /// personal row, not just its content — `registry::personal()` must
    /// agree with `store.personal_catalog()` on `id`/`name`/`org_id`.
    #[test]
    fn load_sets_the_catalogs_identity_from_the_stores_personal_row() {
        let _g = lock_registry_for_test();
        let root = repo_with_one_skill("identity");
        let store = Mutex::new(Store::open_in_memory().unwrap());
        configure(
            ConfigureArgs {
                repo_path: root.to_string_lossy().into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        load(false, &store).unwrap();
        let personal_row = store
            .lock()
            .unwrap()
            .personal_catalog()
            .unwrap()
            .expect("personal row");
        let loaded = registry::personal().unwrap().expect("loaded catalog");
        assert_eq!(loaded.name, "personal");
        assert_eq!(loaded.org_id, None);
        assert_eq!(loaded.id, personal_row.id);
        registry::clear().unwrap();
    }

    /// Fix round 1, item 2: `load` must not just install the current
    /// personal catalog — it must evict any OTHER `org_id: None` entry the
    /// registry happens to be holding (a stale one, under a different id,
    /// however it got there), so the registry can never disagree with
    /// itself about which catalog is "the" personal one.
    #[test]
    fn load_evicts_a_stale_personal_entry_with_a_different_id() {
        let _g = lock_registry_for_test();
        let root = repo_with_one_skill("evict-stale");
        let store = Mutex::new(Store::open_in_memory().unwrap());
        configure(
            ConfigureArgs {
                repo_path: root.to_string_lossy().into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        let real_id = store
            .lock()
            .unwrap()
            .personal_catalog()
            .unwrap()
            .unwrap()
            .id;
        // A stale personal entry under an id that is NOT this store's own
        // — simulates whatever left one behind (a hand-built test catalog,
        // or a previous process's load under a since-changed id).
        let stale_id = real_id + 1000;
        registry::install(repo::Catalog {
            id: stale_id,
            name: "personal".into(),
            org_id: None,
            ..Default::default()
        })
        .unwrap();

        load(false, &store).unwrap();

        let loaded = registry::personal().unwrap().expect("loaded catalog");
        assert_eq!(loaded.id, real_id, "load's own catalog must win");
        assert!(
            registry::get(stale_id).unwrap().is_none(),
            "the stale entry must be evicted, not left to coexist"
        );
        registry::clear().unwrap();
    }

    #[test]
    fn load_requires_configuration() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let err = load(false, &store).unwrap_err();
        assert_eq!(err.code, E_CATALOG_NOT_CONFIGURED);
        assert_eq!(
            list_assets(&store).unwrap_err().code,
            E_CATALOG_NOT_CONFIGURED
        );
    }

    #[test]
    fn configure_load_list_get() {
        let _g = lock_registry_for_test();
        let root = repo_with_one_skill("cll");
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let cfg = configure(
            ConfigureArgs {
                repo_path: root.to_string_lossy().into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        assert_eq!(cfg.repo_path, root.to_string_lossy());
        let summary = load(false, &store).unwrap();
        assert_eq!(summary.asset_count, 1);
        assert_eq!(summary.head.len(), 40);
        assert_eq!(
            store
                .lock()
                .unwrap()
                .get_catalog_config()
                .unwrap()
                .unwrap()
                .head_commit,
            Some(summary.head.clone())
        );

        // Seed inventory rows as a scan would.
        store
            .lock()
            .unwrap()
            .replace_host_inventory(
                "local",
                "claude",
                &[
                    crate::store::AssetInventoryRow {
                        host_alias: "local".into(),
                        harness: "claude".into(),
                        kind: "skill".into(),
                        name: "s".into(),
                        state: "drifted".into(),
                        catalog_hash: None,
                        host_hash: None,
                        scanned_at: 1,
                        managed: false,
                        ..Default::default()
                    },
                    crate::store::AssetInventoryRow {
                        host_alias: "local".into(),
                        harness: "claude".into(),
                        kind: "skill".into(),
                        name: "extra".into(),
                        state: "unmanaged".into(),
                        catalog_hash: None,
                        host_hash: None,
                        scanned_at: 1,
                        managed: false,
                        ..Default::default()
                    },
                ],
            )
            .unwrap();

        let listing = list_assets(&store).unwrap();
        assert_eq!(listing.assets.len(), 1);
        assert_eq!(listing.assets[0].name, "s");
        assert_eq!(
            listing.assets[0].hosts,
            vec![HostState {
                host_alias: "local".into(),
                harness: "claude".into(),
                state: "drifted".into(),
                drift_side: None,
            }]
        );
        assert_eq!(listing.unmanaged.len(), 1);
        assert_eq!(listing.unmanaged[0].name, "extra");
        assert!(listing.assets[0].install_as.is_none());

        let detail = get_asset(model::Kind::Skill, "s", &store).unwrap();
        assert_eq!(detail.asset.body, "b\n");
        assert_eq!(detail.previews.len(), 2);
        let claude = detail
            .previews
            .iter()
            .find(|p| p.harness == "claude")
            .unwrap();
        assert_eq!(
            claude.plan.as_ref().unwrap().files[0].path,
            "~/.claude/skills/s/SKILL.md"
        );
        assert_eq!(
            get_asset(model::Kind::Agent, "nope", &store)
                .unwrap_err()
                .code,
            E_ASSET_NOT_FOUND
        );
        let hook = model::Asset::from_yaml(None, "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n").unwrap();
        repo::write_asset(&root, &hook, false).unwrap();
        load(false, &store).unwrap();
        let detail = get_asset(model::Kind::Hook, "h", &store).unwrap();
        let codex = detail
            .previews
            .iter()
            .find(|p| p.harness == "codex")
            .unwrap();
        assert!(codex.plan.is_none());
        assert!(codex.unsupported.as_deref().unwrap().contains("codex"));
    }

    #[test]
    fn asset_summary_carries_install_as_only_when_set() {
        let base = AssetSummary {
            kind: "skill".into(),
            name: "foo-bar".into(),
            version: "1".into(),
            description: "d".into(),
            tags: Vec::new(),
            hosts: Vec::new(),
            install_as: None,
            scope: model::Scope::Private,
            catalog: catalogs::PERSONAL.into(),
        };
        let json = serde_json::to_value(&base).unwrap();
        assert!(
            !json.as_object().unwrap().contains_key("install_as"),
            "{json}"
        );
        let with = AssetSummary {
            install_as: Some("foo_bar".into()),
            ..base
        };
        assert_eq!(
            serde_json::to_value(&with).unwrap()["install_as"],
            serde_json::json!("foo_bar")
        );
    }

    /// An old hub's reply has no `identities` key at all; deserializing that
    /// into `AssetListing` must land on `None`, and re-serializing `None`
    /// must omit the key again — never round-trip it into a present-but-empty
    /// `[]`, which is indistinguishable on the frontend from "the hub ran
    /// grouping and found nothing" and makes `identitiesOf` skip its
    /// client-side fallback (the S1a review finding this test pins).
    #[test]
    fn asset_listing_identities_round_trips_absent_vs_empty() {
        let without_key = serde_json::json!({
            "head": "abc",
            "loaded_at": 1,
            "assets": [],
            "unmanaged": [],
            "problems": [],
        });
        let listing: AssetListing = serde_json::from_value(without_key).unwrap();
        assert!(listing.identities.is_none());
        let reserialized = serde_json::to_value(&listing).unwrap();
        assert!(
            !reserialized.as_object().unwrap().contains_key("identities"),
            "{reserialized}"
        );

        let with_empty = AssetListing {
            identities: Some(Vec::new()),
            ..listing
        };
        let reserialized = serde_json::to_value(&with_empty).unwrap();
        assert_eq!(reserialized["identities"], serde_json::json!([]));
    }

    /// Assets S1b: `AssetSummary.scope` mirrors each asset's own
    /// `Header.scope`, not a catalog-wide default.
    #[test]
    fn list_assets_carries_scope_per_asset() {
        let _g = lock_registry_for_test();
        let root = repo_with_scoped_assets("scope");
        let store = Mutex::new(Store::open_in_memory().unwrap());
        configure(
            ConfigureArgs {
                repo_path: root.to_string_lossy().into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        load(false, &store).unwrap();

        let listing = list_assets(&store).unwrap();
        let scope_of = |name: &str| {
            listing
                .assets
                .iter()
                .find(|a| a.name == name)
                .unwrap_or_else(|| panic!("no asset named {name}"))
                .scope
        };
        assert_eq!(scope_of("s"), model::Scope::Private);
        assert_eq!(scope_of("shared-s"), model::Scope::Shared);
    }

    /// Assets M5 (R11, R12): the listing spans every loaded catalog, each
    /// asset naming its catalog and showing only its own catalog's host
    /// states; the personal-only listing is as before; `get_asset` reads
    /// its own catalog's hosts.
    #[test]
    fn list_assets_spans_catalogs_and_keeps_each_catalogs_host_states() {
        let _g = lock_registry_for_test();
        let root = repo_with_one_skill("m5-list");
        let store = Mutex::new(Store::open_in_memory().unwrap());
        configure(
            ConfigureArgs {
                repo_path: root.to_string_lossy().into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        load(false, &store).unwrap();
        let org = store.lock().unwrap().add_org("acme", None, false).unwrap();
        let acme = store
            .lock()
            .unwrap()
            .upsert_catalog(
                "acme",
                &repo_with_one_skill("m5-list-acme").to_string_lossy(),
                None,
                Some(org.id),
            )
            .unwrap();
        load_catalog(acme.id, false, &store).unwrap();
        let personal = store
            .lock()
            .unwrap()
            .personal_catalog()
            .unwrap()
            .unwrap()
            .id;
        let row = |host: &str, state: &str, catalog_id: Option<i64>| AssetInventoryRow {
            host_alias: host.into(),
            harness: "claude".into(),
            kind: "skill".into(),
            name: "s".into(),
            state: state.into(),
            scanned_at: 1,
            managed: true,
            catalog_id,
            drift_side: (state == "drifted").then(|| "host".to_string()),
            ..Default::default()
        };
        {
            let s = store.lock().unwrap();
            for h in ["local", "mefistos", "oci"] {
                s.upsert_host(h).unwrap();
            }
            s.replace_host_inventory("local", "claude", &[row("local", "in_sync", Some(acme.id))])
                .unwrap();
            s.replace_host_inventory(
                "mefistos",
                "claude",
                &[row("mefistos", "drifted", Some(personal))],
            )
            .unwrap();
            s.replace_host_inventory("oci", "claude", &[row("oci", "missing", None)])
                .unwrap();
        }

        // Fix round 1: the default listing is personal's, exactly as before
        // M5 — not even a `catalog` key — so an older desktop keyed by name
        // never sees a second `s`. The wide listing is opt-in.
        let default = list_assets(&store).unwrap();
        assert_eq!(default.assets.len(), 1);
        assert_eq!(default.assets[0].catalog, "personal");
        let wire = serde_json::to_value(&default).unwrap();
        assert!(
            !wire["assets"][0]
                .as_object()
                .unwrap()
                .contains_key("catalog"),
            "{wire}"
        );
        let every = list_assets_in(&store, ListingScope::Every).unwrap();
        let hosts_of = |catalog: &str| -> Vec<(String, String, Option<String>)> {
            every
                .assets
                .iter()
                .find(|a| a.catalog == catalog)
                .unwrap_or_else(|| panic!("no asset in {catalog}"))
                .hosts
                .iter()
                .map(|h| (h.host_alias.clone(), h.state.clone(), h.drift_side.clone()))
                .collect()
        };
        assert_eq!(every.assets.len(), 2, "one `s` per catalog");
        assert_eq!(
            every
                .assets
                .iter()
                .map(|a| a.catalog.as_str())
                .collect::<Vec<_>>(),
            ["personal", "acme"],
            "personal first (registry::in_order)"
        );
        assert_eq!(
            hosts_of("personal"),
            [
                (
                    "mefistos".to_string(),
                    "drifted".to_string(),
                    Some("host".to_string())
                ),
                ("oci".to_string(), "missing".to_string(), None),
            ],
            "an unstamped row (scanned before M2) is personal's"
        );
        assert_eq!(
            hosts_of("acme"),
            [("local".to_string(), "in_sync".to_string(), None)]
        );

        let personal_only = list_assets_in(&store, ListingScope::Personal).unwrap();
        assert_eq!(
            personal_only
                .assets
                .iter()
                .map(|a| a.catalog.as_str())
                .collect::<Vec<_>>(),
            ["personal"]
        );
        let detail = get_asset(Kind::Skill, "s", &store).unwrap();
        assert_eq!(
            detail
                .hosts
                .iter()
                .map(|h| h.host_alias.as_str())
                .collect::<Vec<_>>(),
            ["mefistos", "oci"]
        );
        let acme_row = store.lock().unwrap().get_catalog(acme.id).unwrap().unwrap();
        let acme_detail =
            get_asset_in(CatalogTarget::Row(&acme_row), Kind::Skill, "s", &store).unwrap();
        assert_eq!(
            acme_detail
                .hosts
                .iter()
                .map(|h| h.host_alias.as_str())
                .collect::<Vec<_>>(),
            ["local"],
            "carry 3: acme/s shows acme's hosts only"
        );
        // An older hub's summary has no `catalog`: it reads as personal.
        let old: AssetSummary = serde_json::from_str(
            r#"{"kind":"skill","name":"s","version":"1","description":"d","tags":[],"hosts":[]}"#,
        )
        .unwrap();
        assert_eq!(old.catalog, "personal");
        registry::clear().unwrap();
    }

    /// `orphan` rows — the host still holds something a past sync wrote but
    /// the catalog has dropped — belong in the same "not in the catalog"
    /// list the UI shows, alongside `unmanaged`.
    #[test]
    fn list_assets_lists_orphan_rows_as_unmanaged() {
        let _g = lock_registry_for_test();
        let root = repo_with_one_skill("orphan");
        let store = Mutex::new(Store::open_in_memory().unwrap());
        configure(
            ConfigureArgs {
                repo_path: root.to_string_lossy().into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        load(false, &store).unwrap();
        let row = |name: &str, state: &str, managed: bool| crate::store::AssetInventoryRow {
            host_alias: "local".into(),
            harness: "claude".into(),
            kind: "skill".into(),
            name: name.into(),
            state: state.into(),
            catalog_hash: None,
            host_hash: None,
            scanned_at: 1,
            managed,
            ..Default::default()
        };
        store
            .lock()
            .unwrap()
            .replace_host_inventory(
                "local",
                "claude",
                &[
                    row("extra", "unmanaged", false),
                    row("gone", "orphan", true),
                    row("s", "in_sync", true),
                ],
            )
            .unwrap();

        let listing = list_assets(&store).unwrap();
        let mut names: Vec<&str> = listing.unmanaged.iter().map(|r| r.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["extra", "gone"], "{:?}", listing.unmanaged);
        assert_eq!(
            listing.assets[0].hosts,
            vec![HostState {
                host_alias: "local".into(),
                harness: "claude".into(),
                state: "in_sync".into(),
                drift_side: None,
            }],
            "an orphan is not a host state of a catalog asset"
        );
    }

    /// Assets M5 (R4): a catalog asset's host states carry which side moved,
    /// so a client that reads only `list_assets` sees it too.
    #[test]
    fn host_states_carry_which_side_moved() {
        let row = |host: &str, side: Option<&str>| AssetInventoryRow {
            host_alias: host.into(),
            harness: "claude".into(),
            kind: "skill".into(),
            name: "s".into(),
            state: "drifted".into(),
            managed: true,
            drift_side: side.map(String::from),
            ..Default::default()
        };
        let got: Vec<(String, Option<String>)> = host_states(
            &[
                row("oci", Some("catalog")),
                row("trn", Some("host")),
                row("htz", None),
            ],
            &repo::Catalog::default(),
            Kind::Skill,
            "s",
        )
        .into_iter()
        .map(|h| (h.host_alias, h.drift_side))
        .collect();
        assert_eq!(
            got,
            [
                ("oci".to_string(), Some("catalog".to_string())),
                ("trn".to_string(), Some("host".to_string())),
                ("htz".to_string(), None),
            ]
        );
        let wire = serde_json::to_value(HostState {
            host_alias: "oci".into(),
            harness: "claude".into(),
            state: "in_sync".into(),
            drift_side: None,
        })
        .unwrap();
        assert!(
            wire.get("drift_side").is_none(),
            "absent when unknown: {wire}"
        );
        let old: HostState =
            serde_json::from_str(r#"{"host_alias":"oci","harness":"claude","state":"drifted"}"#)
                .unwrap();
        assert_eq!(old.drift_side, None, "an older hub's reply still parses");
    }

    /// A typo'd `host_alias` must fail clearly, not as a raw SQLite
    /// foreign-key violation: `host_layers.host_alias` references
    /// `hosts(alias)` with `PRAGMA foreign_keys = ON`.
    #[test]
    fn set_host_layers_rejects_an_unknown_host() {
        let _g = lock_registry_for_test();
        let store = configured_store_with_layers("shl-unknown-host");

        let err = set_host_layers("mefistso", Some("core"), &[], &store).unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        assert!(err.message.contains("mefistso"), "{}", err.message);
    }

    /// The mirror problem on the read side: `resolve_for_host` treats "no
    /// assignment rows" as "no layering" for backward compatibility, so
    /// without a host-existence check a typo'd host would silently resolve
    /// to the WHOLE catalog instead of erroring — indistinguishable from an
    /// unassigned but real host.
    #[test]
    fn resolve_preview_rejects_an_unknown_host() {
        let _g = lock_registry_for_test();
        let store = configured_store_with_layers("rp-unknown-host");

        let err = resolve_preview("mefistso", &store).unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        assert!(err.message.contains("mefistso"), "{}", err.message);
    }

    /// A carry-forward from Task 4's review: `host_layers`'s primary key is
    /// `(host_alias, layer_name)` with no `axis` column, which is safe only
    /// because `set_host_layers` refuses a name the catalog does not define
    /// before it ever reaches the store — otherwise a typo becomes a raw
    /// SQLite error instead of a clear one.
    #[test]
    fn set_host_layers_rejects_a_layer_name_the_catalog_does_not_define() {
        let _g = lock_registry_for_test();
        let store = configured_store_with_layers("shl-unknown");

        let err = set_host_layers("local", Some("ghost"), &[], &store).unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(err.message.contains("ghost"), "{}", err.message);

        let err = set_host_layers("local", None, &["ghost"], &store).unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(err.message.contains("ghost"), "{}", err.message);

        // Neither rejected call wrote anything.
        assert!(store
            .lock()
            .unwrap()
            .get_host_layers("local")
            .unwrap()
            .is_empty());
    }

    /// The other half of the Task 4 carry-forward: a name that exists but on
    /// the wrong axis (a context passed as the role, or vice versa) must
    /// also be refused with a diagnostic — the store has no `axis` column of
    /// its own to catch this, so the catalog and the store could otherwise
    /// silently disagree about what a layer is.
    #[test]
    fn set_host_layers_rejects_a_layer_on_the_wrong_axis() {
        let _g = lock_registry_for_test();
        let store = configured_store_with_layers("shl-axis");

        // "extra" is a context layer; naming it as the role must fail.
        let err = set_host_layers("local", Some("extra"), &[], &store).unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(err.message.contains("extra"), "{}", err.message);

        // "core" is a role layer; naming it as a context must fail too.
        let err = set_host_layers("local", None, &["core"], &store).unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(err.message.contains("core"), "{}", err.message);

        assert!(store
            .lock()
            .unwrap()
            .get_host_layers("local")
            .unwrap()
            .is_empty());
    }

    /// A carry-forward from my own Task 8 review: `host_layers`'s primary
    /// key is `(host_alias, layer_name)`, so a context repeated twice would
    /// otherwise hit that constraint on the second insert and surface as a
    /// raw SQLite error instead of a clear one.
    #[test]
    fn set_host_layers_rejects_a_duplicate_context_name() {
        let _g = lock_registry_for_test();
        let store = configured_store_with_layers("shl-dup-ctx");

        let err = set_host_layers("local", None, &["extra", "extra"], &store).unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(err.message.contains("extra"), "{}", err.message);

        assert!(store
            .lock()
            .unwrap()
            .get_host_layers("local")
            .unwrap()
            .is_empty());
    }

    /// Same primary-key collision, the other way round: a context that
    /// names the same layer as the role.
    #[test]
    fn set_host_layers_rejects_a_context_that_repeats_the_role() {
        let _g = lock_registry_for_test();
        let store = configured_store_with_layers("shl-role-ctx-clash");

        let err = set_host_layers("local", Some("core"), &["core"], &store).unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(err.message.contains("core"), "{}", err.message);

        assert!(store
            .lock()
            .unwrap()
            .get_host_layers("local")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn list_layers_reports_only_active_assignments() {
        // The tool describes "each host's role + active contexts"; an
        // inactive row is not part of the assignment and must not be shown
        // as if it were.
        let _g = lock_registry_for_test();
        let store = configured_store_with_layers("ll-active");
        set_host_layers("local", Some("core"), &[], &store).unwrap();
        let personal = store
            .lock()
            .unwrap()
            .personal_catalog()
            .unwrap()
            .unwrap()
            .id;
        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute(
                "INSERT INTO host_layers (host_alias, catalog_id, layer_name, axis, position, active) \
                 VALUES ('local', ?1, 'extra', 'context', 0, 0)",
                [personal],
            )
            .unwrap();

        let listing = list_layers(&store).unwrap();
        let names: Vec<&str> = listing
            .hosts
            .iter()
            .map(|r| r.layer_name.as_str())
            .collect();
        assert_eq!(names, vec!["core"]);
    }

    #[test]
    fn set_host_layers_accepts_a_valid_assignment_and_list_layers_reflects_it() {
        let _g = lock_registry_for_test();
        let store = configured_store_with_layers("shl-ok");

        let rows = set_host_layers("local", Some("core"), &["extra"], &store).unwrap();
        assert_eq!(rows.len(), 2);

        let listing = list_layers(&store).unwrap();
        assert_eq!(listing.layers.len(), 2);
        assert_eq!(listing.hosts.len(), 2);

        let resolved = resolve_preview("local", &store).unwrap();
        // `other` is in the catalog but a member of no layer: its absence
        // is what distinguishes "layering actually ran" from "layering was
        // ignored" or "fell back to the whole catalog" — either of those
        // would leave `other` in the resolved set.
        let names: Vec<&str> = resolved
            .catalog
            .assets
            .iter()
            .map(|a| a.header.name.as_str())
            .collect();
        assert_eq!(names, vec!["s"]);
        assert!(
            !names.contains(&"other"),
            "'other' is not a member of any assigned layer"
        );
        assert_eq!(resolved.provenance["skill/s"].introduced_by, "core");
    }

    /// Assets M3 (M2 carry 7): layers are listed and assigned per catalog —
    /// personal's listing no longer shows another catalog's rows, and an
    /// assignment answers with the rows of the catalog it was made in.
    #[test]
    fn layers_are_listed_and_assigned_per_catalog() {
        let _g = lock_registry_for_test();
        let store = configured_store_with_layers("m3-layers");
        let org = store.lock().unwrap().add_org("acme", None, false).unwrap();
        let acme = store
            .lock()
            .unwrap()
            .upsert_catalog(
                "acme",
                &repo_with_layers("m3-layers-acme").to_string_lossy(),
                None,
                Some(org.id),
            )
            .unwrap();
        load_catalog(acme.id, false, &store).unwrap();

        let personal_rows = set_host_layers("local", Some("core"), &[], &store).unwrap();
        assert_eq!(personal_rows.len(), 1);
        let rows = set_host_layers_for("local", &acme, Some("core"), &["extra"], &store).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.catalog_id == acme.id), "{rows:?}");

        assert_eq!(list_layers(&store).unwrap().hosts.len(), 1);
        let listing = list_layers_for(&acme, &store).unwrap();
        assert_eq!((listing.hosts.len(), listing.layers.len()), (2, 2));
        assert_eq!(
            set_host_layers_for("local", &acme, Some("extra"), &[], &store)
                .unwrap_err()
                .code,
            codes::E_INVALID,
            "the axis is checked against THAT catalog's layers"
        );
        registry::clear().unwrap();
    }

    #[test]
    fn an_unloaded_org_catalog_has_no_layers_to_list() {
        let _g = lock_registry_for_test();
        let store = configured_store_with_layers("m3-unloaded");
        let org = store.lock().unwrap().add_org("acme", None, false).unwrap();
        let acme = store
            .lock()
            .unwrap()
            .upsert_catalog("acme", "/nowhere", None, Some(org.id))
            .unwrap();
        assert_eq!(
            list_layers_for(&acme, &store).unwrap_err().code,
            E_CATALOG_NOT_CONFIGURED
        );
        registry::clear().unwrap();
    }

    /// Fix round 1, item 3 (controller ruling): `resolve_preview` is built
    /// from `effective::effective_for_host`, whose `compose` must keep
    /// `Resolution::excluded` ("why is this gone") alive rather than
    /// dropping it — on the single-catalog (personal-only) path it must
    /// equal exactly what `sync::layers::resolve_for_host` answers.
    #[test]
    fn resolve_preview_excluded_matches_resolve_for_host() {
        let _g = lock_registry_for_test();
        let root = std::env::temp_dir().join(format!(
            "fleet-catalog-svc-rp-excluded-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("skills/s")).unwrap();
        std::fs::create_dir_all(root.join("skills/other")).unwrap();
        std::fs::create_dir_all(root.join("layers")).unwrap();
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        std::fs::write(
            root.join("skills/s/asset.yaml"),
            "kind: skill\nname: s\ndescription: d\n",
        )
        .unwrap();
        std::fs::write(root.join("skills/s/body.md"), "b\n").unwrap();
        std::fs::write(
            root.join("skills/other/asset.yaml"),
            "kind: skill\nname: other\ndescription: d\n",
        )
        .unwrap();
        std::fs::write(root.join("skills/other/body.md"), "b\n").unwrap();
        // A role that both names a member and excludes a DIFFERENT asset —
        // `resolve()` records an `excluded` entry for a key regardless of
        // whether it was ever a member, so "other" need not be included
        // anywhere first.
        std::fs::write(
            root.join("layers/core.yaml"),
            "kind: layer\nname: core\naxis: role\nmembers:\n  - skill/s\n\
             exclude:\n  - skill/other\n",
        )
        .unwrap();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);

        let store = Mutex::new(Store::open_in_memory().unwrap());
        store.lock().unwrap().upsert_host("local").unwrap();
        configure(
            ConfigureArgs {
                repo_path: root.to_string_lossy().into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        load(false, &store).unwrap();
        set_host_layers("local", Some("core"), &[], &store).unwrap();

        let preview = resolve_preview("local", &store).unwrap();
        let personal = registry::personal().unwrap().unwrap();
        let direct = sync::layers::resolve_for_host(&store, &personal, "local").unwrap();
        assert_eq!(preview.excluded, direct.excluded);
        assert_eq!(
            preview.excluded.get("skill/other").map(String::as_str),
            Some("core")
        );
    }

    /// `import_host` is `async` now (Task 6: a remote alias needs to SSH in
    /// before it can import), but it must still check the catalog is
    /// configured before it does anything else — for `local` exactly as
    /// before, and for any other alias before it ever dials out.
    #[tokio::test]
    async fn import_host_requires_catalog_config_first() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
        for host_alias in ["local", "oci"] {
            let err = import_host(
                ImportArgs {
                    host_alias: host_alias.into(),
                    dry_run: true,
                    only: vec![],
                },
                &store,
                &ssh,
                None,
            )
            .await
            .unwrap_err();
            assert_eq!(err.code, E_CATALOG_NOT_CONFIGURED, "{host_alias}");
        }
    }

    /// Security fix round 1, IMPORTANT 2: `import_host` must not dial an
    /// arbitrary string over SSH — only a registered, non-hidden host. Both
    /// cases must refuse `E_NOTFOUND` before ever reaching `run_host_script`
    /// (a real ssh subprocess would hang/fail slowly against "ghost" or a
    /// hidden host with no real address, which this test never triggers).
    #[tokio::test]
    async fn import_host_refuses_an_unregistered_or_hidden_alias_before_dialing_out() {
        let root = repo_with_one_skill("import-host-dial");
        let store = Mutex::new(Store::open_in_memory().unwrap());
        configure(
            ConfigureArgs {
                repo_path: root.to_string_lossy().into(),
                remote_url: None,
            },
            &store,
        )
        .unwrap();
        store
            .lock()
            .unwrap()
            .insert_host("hidden-host", None)
            .unwrap();
        store
            .lock()
            .unwrap()
            .set_host_hidden("hidden-host", true)
            .unwrap();
        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());

        for (alias, why) in [("ghost", "never registered"), ("hidden-host", "hidden")] {
            let err = import_host(
                ImportArgs {
                    host_alias: alias.into(),
                    dry_run: true,
                    only: vec![],
                },
                &store,
                &ssh,
                None,
            )
            .await
            .unwrap_err();
            assert_eq!(err.code, codes::E_NOTFOUND, "{why}: {}", err.message);
            assert!(err.message.contains(alias), "{why}: {}", err.message);
        }
    }
}
