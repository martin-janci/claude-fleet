//! The sync engine: applies rendered assets to hosts, tracking what it
//! wrote in a per-harness managed manifest so a later sync can update or
//! remove exactly what an earlier one added, and substituting `${NAME}`
//! secret placeholders into rendered plans before anything is written.
//!
//! `manifest` owns the on-host manifest file's shape and diffing against the
//! catalog; `secrets` resolves `${NAME}` values (host override > global >
//! built-ins) and substitutes them into a `RenderPlan`; `plan` consumes both
//! to decide, per asset and per host, what a sync would actually do, and
//! parks the result in a short-lived registry for the applier; `apply` then
//! executes one host's plan — guarded writes, secret uploads, config merges,
//! plugin CLI calls and the manifest rewrite.
//!
//! This module itself is the orchestration on top: `plan_sync` scans every
//! host, computes and registers a fleet-wide plan (refreshing the inventory
//! rows on the way), `apply_sync` applies one by id with progress events,
//! cancellation and a `sync_runs` history entry, and `last_sync` reads the
//! most recent entry back. `catalog_plan_sync` / `catalog_apply_sync` /
//! `catalog_last_sync` (Tauri commands) and `plan_sync` / `apply_sync` (MCP
//! tools) wire these in.

pub mod apply;
pub mod layers;
pub mod manifest;
pub mod plan;
pub mod secrets;

use super::effective;
use super::harness_set::{self, gated_catalog, harness_gate, HarnessFacts, HarnessGate};
use crate::cancel::{CancelGuard, CancellationRegistry};
use crate::events::SyncProgress;
use crate::ipc_error::lock;
use crate::ipc_error::{codes, IpcError};
use crate::ssh::SshClient;
use crate::store::Store;
use apply::{ActionResult, ApplyCtx, HostSyncResult};
use manifest::Manifest;
use plan::{ActionOp, HostPlan, PlanFilter, SyncPlan};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// Which hosts / assets a plan covers. All three narrow; `None` everywhere
/// means the whole fleet and the whole catalog.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanArgs {
    pub host_alias: Option<String>,
    pub kind: Option<super::model::Kind>,
    pub name: Option<String>,
    /// Plan a remote host that has no layers assigned. Without it such a
    /// host is skipped: with no layers it would receive the whole catalog.
    #[serde(default)]
    pub allow_unlayered: bool,
}

/// Detail on every `HostPlan` `refuse_unlayered` produces. Shared by the
/// engine (what it stamps on a skipped plan) and the MCP tool description
/// (what the caller should expect), so the two never drift apart.
pub const UNLAYERED_DETAIL: &str =
    "no layers assigned: syncing would install the whole catalog here. \
    Assign a role first (set_host_layers), or plan with allow_unlayered.";

/// A remote host with no layers would receive the whole catalog. `local` is
/// exempt: a single-machine user syncs its own catalog back to itself.
pub(crate) fn refuse_unlayered(
    alias: &str,
    layered: bool,
    allow: bool,
    catalog_empty: bool,
) -> bool {
    alias != "local" && !layered && !allow && !catalog_empty
}

/// Apply a plan `plan_sync` computed and parked in the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyArgs {
    pub plan_id: String,
    /// Apply everything that is not blocked on a missing `${NAME}` secret
    /// instead of refusing the whole run with `E_SECRET_MISSING`.
    #[serde(default)]
    pub force_partial: bool,
    /// Cancellation handle, like `NewSessionArgs::call_id`: bound into the
    /// registry so `cancel_task` can stop a sync between (and during) its
    /// per-host scripts.
    pub call_id: Option<u64>,
}

/// One completed `apply_sync`, returned to the caller and stored verbatim
/// as the `sync_runs` row's `summary_json` (hence `Deserialize` too — it is
/// what `last_sync` reads back).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRunSummary {
    pub plan_id: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub hosts: Vec<HostSyncResult>,
    /// Assets M4, final review I3: SB6's automatic additive sync ran this,
    /// not a person (nor a card a person applied). The scan tick's
    /// "everything changed" key skips such runs. Absent (false) on every
    /// other run, so their stored JSON is unchanged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto: bool,
}

/// Every catalog in `snapshot`, merged (`registry::union_of`). This is the
/// FULL catalog a scan diffs against — never the host-resolved one
/// (`effective::effective_for_host_in`'s `eff.catalog`), which is what a
/// host is supposed to have, not what its drift is measured against.
fn union_catalog(
    snapshot: &BTreeMap<i64, super::repo::Catalog>,
) -> Result<super::repo::Catalog, IpcError> {
    super::registry::union_of(snapshot).ok_or_else(|| {
        IpcError::new(
            super::E_CATALOG_NOT_CONFIGURED,
            "catalog not loaded; call catalog_load",
        )
    })
}

/// A `HostPlan` with nothing to do and a reason: an unreachable host, or a
/// harness whose scan failed. One per (host, harness) pair either way, so
/// the applier's progress accounting and its "skipped" results line up with
/// the pairs a successful plan produces.
fn skipped_plan(host_alias: &str, harness: &str, detail: &str) -> HostPlan {
    HostPlan {
        host_alias: host_alias.to_string(),
        harness: harness.to_string(),
        status: "skipped".into(),
        detail: Some(detail.to_string()),
        actions: Vec::new(),
        snapshot: Default::default(),
        manifest: Manifest::default(),
    }
}

/// Scan one (host, harness), persist the inventory rows that scan implies
/// (so the asset matrix refreshes through the row events
/// `replace_host_inventory` already emits) and hand the snapshot, its
/// managed manifest and the harness's gate back to the caller. `secrets` is
/// resolved by the caller BEFORE this await — `secrets::resolve` takes the
/// store lock internally. `configured` is the host's `harnesses` column
/// (`None` = auto): the rows follow `harness_set::harness_gate` — none for
/// `Off` (persisting the empty list clears rows an earlier scan left), only
/// fleet's own installs (as orphans) for `Retiring`. Persisting is
/// best-effort: a scan is still usable if the rows could not be written.
async fn scan_and_persist(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    catalog: &super::repo::Catalog,
    harness: &dyn super::harness::Harness,
    host_alias: &str,
    secrets: &BTreeMap<String, String>,
    configured: Option<&[String]>,
) -> Result<(super::harness::HostSnapshot, Manifest, HarnessGate), IpcError> {
    let scanned_at = super::now_secs();
    let snap = super::inventory::scan_host_harness(ssh, host_alias, harness).await?;
    let manifest = Manifest::from_snapshot(&snap, harness.manifest_path());
    let gate = harness_gate(harness.id(), configured, HarnessFacts::of(&snap, &manifest));
    let rows = gated_catalog(gate, catalog).map_or_else(Vec::new, |c| {
        super::inventory::compute_states(
            c, harness, host_alias, &snap, &manifest, secrets, scanned_at,
        )
    });
    match store.lock() {
        Ok(s) => {
            if let Err(e) = s.replace_host_inventory(host_alias, harness.id(), &rows) {
                tracing::warn!(
                    host = host_alias,
                    harness = harness.id(),
                    error = %e,
                    "could not persist host inventory"
                );
            }
        }
        Err(_) => tracing::warn!(
            host = host_alias,
            harness = harness.id(),
            "store mutex poisoned while persisting inventory"
        ),
    }
    Ok((snap, manifest, gate))
}

/// Compute what a sync would do across the fleet, park it in the plan
/// registry and hand the caller the plan (with `counts` tallied) plus its
/// id. Every host is scanned fresh, and the rows that scan implies are
/// persisted exactly as `inventory::scan_hosts` would — planning doubles as
/// a refresh, so the asset matrix is never staler than the plan the user is
/// looking at.
///
/// Per-host failures never abort the fleet: a hidden host is skipped
/// silently, an unreachable non-`local` host and a harness whose scan failed
/// each get a `skipped` `HostPlan` carrying the reason.
pub async fn plan_sync(
    args: PlanArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SyncPlan, IpcError> {
    // One registry view for the whole plan (Rulings R9, M2 carry 5): the
    // scan union and every host's effective set are built from the same
    // snapshot, cloned out of the registry so no lock is held across an
    // await.
    let snapshot = super::registry::snapshot()?;
    let catalog = union_catalog(&snapshot)?;
    let hosts = {
        let s = lock(store)?;
        s.list_hosts()?
    };
    // An unknown alias must fail loudly rather than silently plan nothing: a
    // typo'd `host_alias` would otherwise come back as an empty-but-valid
    // plan, indistinguishable from "the fleet has nothing to sync here". A
    // hidden host still counts as existing (it is filtered out below, same
    // as today) — only an alias no row has at all is rejected.
    if let Some(alias) = args.host_alias.as_deref() {
        if !hosts.iter().any(|h| h.alias == alias) {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("host {alias} not found"),
            ));
        }
    }
    let filter = PlanFilter {
        host_alias: args.host_alias.clone(),
        kind: args.kind,
        name: args.name.clone(),
        // Set per host below, once its layer assignment is known.
        layered: false,
    };
    // Only harnesses that can scan a host can be planned for: without a
    // snapshot there is nothing to diff the catalog against.
    let harnesses = super::harness::all();
    let scanning: Vec<&dyn super::harness::Harness> = harnesses
        .iter()
        .filter(|hn| hn.scan_script().is_some())
        .map(|hn| hn.as_ref())
        .collect();

    let mut host_plans: Vec<HostPlan> = Vec::new();
    for h in hosts {
        if h.hidden || filter.host_alias.as_deref().is_some_and(|o| o != h.alias) {
            continue;
        }
        // Multi-harness F3a: the host's own harness choice (`None` = auto).
        // Before a scan nothing is detected, so a host skipped before
        // scanning is reported under Claude and the harnesses it lists only.
        let configured = h.harnesses.as_deref();
        let listed: Vec<&dyn super::harness::Harness> = scanning
            .iter()
            .copied()
            .filter(|hn| {
                harness_gate(hn.id(), configured, HarnessFacts::UNKNOWN) != HarnessGate::Off
            })
            .collect();
        if h.alias != "local" && !h.reachable {
            for hn in &listed {
                host_plans.push(skipped_plan(&h.alias, hn.id(), "unreachable"));
            }
            continue;
        }
        // Before any await: `resolve` takes the store lock internally.
        let secrets = match secrets::resolve(store, &h.alias) {
            Ok(v) => v,
            Err(e) => {
                for hn in &listed {
                    host_plans.push(skipped_plan(&h.alias, hn.id(), &e.message));
                }
                continue;
            }
        };
        // Resolve the host's EFFECTIVE catalog ONCE per host, before its
        // harnesses are planned: every catalog it accepts (its admissions
        // included, Assets M3), resolved for it, the scope boundary applied
        // and cross-catalog collisions refused (Assets M2) — against the
        // plan's one snapshot (R9). It takes the store lock itself, so this
        // is called with no store guard held. The scan below still uses the
        // FULL (union) catalog: inventory is about the whole catalog's
        // drift, while the PLAN is about what this host is supposed to have.
        let eff = match effective::effective_for_host_in(store, &h.alias, &snapshot) {
            Ok(r) => r,
            Err(e) => {
                for harness in &listed {
                    host_plans.push(skipped_plan(&h.alias, harness.id(), &e.message));
                }
                continue;
            }
        };
        // A remote host with no layers would otherwise receive the whole
        // catalog — the spec's first critical finding. Refuse it up front,
        // before the scan: every listed harness gets the same skipped
        // plan and the host is never touched over SSH.
        //
        // The emptiness check is against the WHOLE loaded (union) catalog,
        // never `eff.catalog`: an org-bound host's effective catalog can be
        // empty purely because the scope boundary dropped every (private)
        // asset an unlayered personal catalog would otherwise hand it — that
        // is exactly the case this guard exists to catch, not a reason to
        // let it through. Treating that as "nothing to refuse" would plan
        // this host against an empty catalog, and every entry its manifest
        // already names would read as an orphan and get removed.
        if refuse_unlayered(
            &h.alias,
            eff.layered,
            args.allow_unlayered,
            catalog.assets.is_empty(),
        ) {
            for harness in &listed {
                host_plans.push(skipped_plan(&h.alias, harness.id(), UNLAYERED_DETAIL));
            }
            continue;
        }
        // A host with no `host_layers` row must plan a dropped `plugin_ref`
        // exactly as it did before layers existed (`Remove`), not the
        // "reported, not removed" `Noop` that only makes sense once a host
        // opts into layers. `eff.layered` answers that from the same read
        // `effective_for_host` already made — a second read would be a
        // second failure path, and one that used to abort the whole plan
        // instead of skipping just this host.
        let host_filter = PlanFilter {
            layered: eff.layered,
            ..filter.clone()
        };
        // Every asset the effective catalog refused (a scope boundary or a
        // cross-catalog collision) that this plan's `kind`/`name` filter
        // still covers, resolved to its `Kind` and deduplicated by
        // `(kind, name)` — a collision refuses EVERY colliding copy
        // (`effective::compose`), so a two-catalog clash on the same name
        // produces two `Refusal` entries sharing one key and the same
        // reason text; collecting into a map rather than a `Vec` keeps
        // exactly one. Reused both for the `Blocked` action below and to
        // keep `compute_host_plan` from treating the SAME asset as a
        // manifest orphan (it is refused, not dropped from the catalog: the
        // host's existing copy must be left exactly as it is, not removed).
        let refused_matched: BTreeMap<(super::model::Kind, String), (String, Option<String>)> = eff
            .refused
            .iter()
            .filter_map(|r| {
                let kind = super::model::Kind::ALL
                    .iter()
                    .copied()
                    .find(|k| k.as_str() == r.kind)?;
                host_filter.matches(kind, &r.name).then(|| {
                    (
                        (kind, r.name.clone()),
                        (r.reason.clone(), r.catalog.clone()),
                    )
                })
            })
            .collect();
        // Once per harness this host lists — the refusal is a catalog-level
        // decision, independent of which harness renders it, so every
        // harness's plan reports it.
        let blocked_for_refusals: Vec<plan::Action> = refused_matched
            .iter()
            .map(|((kind, name), (reason, catalog))| {
                plan::blocked_action(*kind, name, reason.clone(), catalog.clone())
            })
            .collect();
        // Every private asset the scope boundary dropped SILENTLY (an
        // unlayered org host's personal catalog — `effective::compose`
        // records no `Refusal` for this, since keeping only the shared
        // slice is the default, not a mistake) that this plan's filter
        // still covers. "Silent" must not mean "destructive": a host that
        // already has one of these synced (from before it had an org, or
        // before Assets M2) must keep it exactly as it is, same as a
        // `refused` asset.
        let withheld_keys: BTreeSet<(super::model::Kind, String)> = eff
            .withheld
            .iter()
            .filter_map(|(kind_str, name)| {
                let kind = super::model::Kind::ALL
                    .iter()
                    .copied()
                    .find(|k| k.as_str() == kind_str)?;
                host_filter
                    .matches(kind, name)
                    .then_some((kind, name.clone()))
            })
            .collect();
        // Both `refused` and `withheld` protect the host's existing copy
        // from `compute_host_plan`'s manifest-orphan `Remove` the same way:
        // merged into one set so a single lookup there covers either reason.
        let protected_keys: BTreeSet<(super::model::Kind, String)> = refused_matched
            .keys()
            .cloned()
            .chain(withheld_keys.iter().cloned())
            .collect();
        // Every scanning harness is scanned, even one this host may not
        // serve: the scan is what detects it and reads its manifest.
        let first_plan = host_plans.len();
        for harness in &scanning {
            let harness = *harness;
            match scan_and_persist(
                store, ssh, &catalog, harness, &h.alias, &secrets, configured,
            )
            .await
            {
                Ok((snap, manifest, gate)) => {
                    // `Off` plans nothing; `Retiring` plans against an empty
                    // catalog, so only removals of fleet's own installs remain.
                    let Some(planned) = gated_catalog(gate, &eff.catalog) else {
                        continue;
                    };
                    // Assets M3 (R6): only a catalog that speaks for this
                    // host may have its dropped entries removed; the rest
                    // are kept with a `Noop` and why. A "not accepted"
                    // reason (stale admissions included) is added only for
                    // catalogs this harness's own manifest names (PF10).
                    let keep = plan::KeepRules {
                        protected: protected_keys.clone(),
                        speaks_for: Some(eff.speaks_for.clone()),
                        held_back: eff
                            .held_back_for(manifest.assets.values().map(|e| e.catalog.as_str())),
                        problem_held: eff.problem_held.clone(),
                    };
                    let mut hp = plan::compute_host_plan(
                        planned,
                        harness,
                        &h.alias,
                        &snap,
                        &manifest,
                        &secrets,
                        &host_filter,
                        &keep,
                    );
                    if gate == HarnessGate::Retiring {
                        hp.detail = Some(harness_set::retiring_detail(harness.id()));
                    } else {
                        hp.actions.extend(blocked_for_refusals.iter().cloned());
                        // Report, don't just silently keep: a withheld asset
                        // this harness's manifest already names gets a
                        // `Noop` saying why it's staying, same spirit as the
                        // layered-host "plugins are not removed
                        // automatically" report just below in
                        // `compute_host_plan`'s own orphans pass.
                        for (kind, name) in &withheld_keys {
                            // M2 carry 4: only when no other catalog supplies
                            // the same name — then that catalog's own action
                            // already speaks for it.
                            if manifest.assets.contains_key(&Manifest::key(*kind, name))
                                && eff.catalog.find(*kind, name).is_none()
                            {
                                hp.actions.push(plan::withheld_noop(*kind, name));
                            }
                        }
                    }
                    host_plans.push(hp);
                }
                Err(e) => host_plans.push(skipped_plan(&h.alias, harness.id(), &e.message)),
            }
        }
        // F3c: no two harnesses on this host may manage one file.
        plan::block_cross_harness_collisions(&mut host_plans[first_plan..]);
    }

    let mut computed = SyncPlan::new(host_plans);
    tracing::info!(
        hosts = computed.hosts.len(),
        actions = computed
            .hosts
            .iter()
            .map(|h| h.actions.len())
            .sum::<usize>(),
        "computed a catalog sync plan"
    );
    computed.id = plan::registry_put(computed.clone());
    Ok(computed)
}

/// Every `${NAME}` a blocked action is waiting on, deduplicated and sorted.
/// NAMES ONLY — a secret value must never reach an error message.
fn missing_secret_names(plan: &SyncPlan) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for host in &plan.hosts {
        for action in &host.actions {
            if action.op == ActionOp::Blocked {
                names.extend(action.missing_secrets.iter().cloned());
            }
        }
    }
    names
}

/// Emit one `sync:progress`. Best-effort: a poisoned store mutex must not
/// abort a run whose host writes have already landed, and a progress event
/// nobody sees costs the caller nothing.
fn emit_progress(store: &Mutex<Store>, p: &SyncProgress) {
    match store.lock() {
        Ok(s) => s.bus_sync_progress(p),
        Err(_) => tracing::warn!(
            host = %p.host_alias,
            harness = %p.harness,
            "store mutex poisoned; sync progress not emitted"
        ),
    }
}

fn cancelled_result(host: &HostPlan) -> HostSyncResult {
    HostSyncResult {
        host_alias: host.host_alias.clone(),
        harness: host.harness.clone(),
        status: "skipped".into(),
        detail: Some("cancelled".into()),
        restart_required: false,
        actions: host
            .actions
            .iter()
            .map(|a| ActionResult {
                kind: a.kind.clone(),
                name: a.name.clone(),
                op: a.op,
                outcome: "skipped".into(),
                detail: Some("cancelled".into()),
            })
            .collect(),
    }
}

/// Re-scan one (host, harness) after applying and rewrite its inventory
/// rows, so the asset matrix reflects what is now on the host (through the
/// row events `replace_host_inventory` already emits) without the caller
/// having to trigger a separate scan. Failures are logged, never fatal: the
/// sync itself already happened.
async fn rescan_after_apply(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    catalog: &super::repo::Catalog,
    harness: &dyn super::harness::Harness,
    host_alias: &str,
) {
    // Before the await, as everywhere else: `resolve` takes the store lock.
    let secrets = match secrets::resolve(store, host_alias) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                host = host_alias,
                harness = harness.id(),
                error = %e.message,
                "could not resolve secrets for the post-sync re-scan"
            );
            return;
        }
    };
    // F3a: the host's harness choice decides which rows the re-scan keeps.
    // Read before the await, as everywhere else.
    // A failed read falls back to auto, and says so: the re-scan then keeps
    // whatever auto would, which can differ from an explicit choice.
    let configured = match lock(store) {
        Ok(s) => match s.get_host_row(host_alias) {
            Ok(row) => row.and_then(|r| r.harnesses),
            Err(e) => {
                tracing::warn!(
                    host = host_alias,
                    harness = harness.id(),
                    error = %e,
                    "could not read the host's harness choice for the post-sync re-scan; using auto"
                );
                None
            }
        },
        Err(e) => {
            tracing::warn!(
                host = host_alias,
                harness = harness.id(),
                error = %e.message,
                "could not lock the store for the post-sync re-scan's harness choice; using auto"
            );
            None
        }
    };
    if let Err(e) = scan_and_persist(
        store,
        ssh,
        catalog,
        harness,
        host_alias,
        &secrets,
        configured.as_deref(),
    )
    .await
    {
        tracing::warn!(
            host = host_alias,
            harness = harness.id(),
            error = %e.message,
            "post-sync re-scan failed; the asset matrix is stale for this host"
        );
    }
}

/// Apply the plan parked under `args.plan_id`, host by host.
///
/// The plan is taken from the registry, so a plan is *applied* at most
/// once; an unknown or expired id is `E_SYNC_PLAN_STALE` and the caller
/// must re-plan. Actions blocked on an unresolved `${NAME}` refuse the whole run
/// with `E_SECRET_MISSING` (listing the names, never a value) unless
/// `force_partial` says to apply the rest anyway. That refusal is not an
/// application: the plan goes back into the registry under the same id, so
/// the caller can set the secret and apply the very same plan again.
///
/// Per (host, harness) pair, in plan order: emit `sync:progress`, apply,
/// then re-scan and rewrite that pair's inventory rows so the asset matrix
/// follows along. `done` counts pairs already finished, so the event
/// announces the pair about to be applied (`0/n` first); one terminal
/// `n/n` naming no pair follows a run that completed. Cancelling stops
/// between pairs — every pair not reached is reported `skipped` /
/// `cancelled`, and no terminal event is emitted — and the run is recorded
/// in `sync_runs` either way, so a cancelled sync still leaves a history
/// entry saying how far it got.
pub async fn apply_sync(
    args: ApplyArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    reg: &Arc<CancellationRegistry>,
) -> Result<SyncRunSummary, IpcError> {
    // Mint and bind first, exactly like `add_project`: `bind` REPLACES
    // whatever sits under the id, so the token a run is cancelled through
    // is always the one this call made. `CancelGuard` releases the slot on
    // every exit path, early returns and panics included.
    let (cancel_id, token) = match args.call_id {
        Some(id) => {
            let token = CancellationToken::new();
            reg.bind(id, token.clone());
            (id, token)
        }
        None => reg.register_anonymous(),
    };
    let _guard = CancelGuard::new(Arc::clone(reg), cancel_id);
    apply_sync_with(args, store, ssh, token).await
}

/// What applying an unknown or expired plan id answers — also the
/// `catalog_admin` gate's answer when it cannot see the plan (Assets M3).
pub(crate) fn stale_plan() -> IpcError {
    IpcError::new(
        codes::E_SYNC_PLAN_STALE,
        "that sync plan is unknown or has expired; compute a new one",
    )
}

/// [`apply_sync`] with the cancellation token supplied directly, so a test
/// can prove what an already-cancelled run does without racing the
/// registry.
pub async fn apply_sync_with(
    args: ApplyArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    token: CancellationToken,
) -> Result<SyncRunSummary, IpcError> {
    apply_sync_run(args, false, store, ssh, token).await
}

/// [`apply_sync_with`] for SB6's automatic additive sync: its `sync_runs`
/// row is marked `auto` (final review I3).
pub(crate) async fn apply_sync_auto(
    args: ApplyArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    token: CancellationToken,
) -> Result<SyncRunSummary, IpcError> {
    apply_sync_run(args, true, store, ssh, token).await
}

async fn apply_sync_run(
    args: ApplyArgs,
    auto: bool,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    token: CancellationToken,
) -> Result<SyncRunSummary, IpcError> {
    let (expires_at, computed) =
        plan::registry_take_with_expiry(&args.plan_id).ok_or_else(stale_plan)?;

    if !args.force_partial {
        let missing = missing_secret_names(&computed);
        if !missing.is_empty() {
            // Refusing is not applying: the plan goes straight back into
            // the registry under the id the caller already holds (and its
            // ORIGINAL deadline — the host was never touched), so setting
            // the secret (or deciding to force) is a second `sync_apply`
            // with the same id rather than a re-plan.
            let names = missing.into_iter().collect::<Vec<_>>().join(", ");
            plan::registry_put_existing(&args.plan_id, expires_at, computed);
            return Err(IpcError::new(
                codes::E_SECRET_MISSING,
                format!(
                    "no value for {names}; set them (catalog_set_secret) or apply again with force_partial"
                ),
            ));
        }
    }

    // Only the post-apply re-scan needs the catalog. A catalog unloaded
    // mid-flight must not abort a sync that is already under way — the
    // writes still happen, only the matrix refresh is skipped.
    let catalog = super::registry::union_all().ok().flatten();
    let harnesses = super::harness::all();
    let started_at = super::now_secs();
    let total = computed.hosts.len();
    let mut results: Vec<HostSyncResult> = Vec::with_capacity(total);

    for (done, host) in computed.hosts.iter().enumerate() {
        if token.is_cancelled() {
            results.push(cancelled_result(host));
            continue;
        }
        emit_progress(
            store,
            &SyncProgress {
                plan_id: computed.id.clone(),
                host_alias: host.host_alias.clone(),
                harness: host.harness.clone(),
                done,
                total,
            },
        );
        let Some(harness) = harnesses.iter().find(|h| h.id() == host.harness) else {
            results.push(HostSyncResult {
                host_alias: host.host_alias.clone(),
                harness: host.harness.clone(),
                status: "failed".into(),
                detail: Some(format!("unknown harness {}", host.harness)),
                restart_required: false,
                actions: Vec::new(),
            });
            continue;
        };
        let ctx = ApplyCtx {
            ssh,
            token: token.clone(),
            now: super::now_secs(),
        };
        results.push(apply::apply_host(&ctx, harness.as_ref(), host).await);
        if host.status == "planned" {
            if let Some(catalog) = &catalog {
                rescan_after_apply(store, ssh, catalog, harness.as_ref(), &host.host_alias).await;
            }
        }
    }

    // One terminal event so a progress bar can reach 100%. It names no
    // pair (both strings empty) because no pair is about to be applied —
    // and a cancelled run never gets one, since it never completed.
    if !token.is_cancelled() {
        emit_progress(
            store,
            &SyncProgress {
                plan_id: computed.id.clone(),
                host_alias: String::new(),
                harness: String::new(),
                done: total,
                total,
            },
        );
    }

    let finished_at = super::now_secs();
    let summary = SyncRunSummary {
        plan_id: computed.id.clone(),
        started_at,
        finished_at,
        hosts: results,
        auto,
    };
    // The history entry is best-effort: the host writes have already
    // landed, so losing the `sync_runs` row must not turn a completed sync
    // into an error and throw the summary away with it.
    match serde_json::to_string(&summary) {
        Ok(json) => match store.lock() {
            Ok(s) => {
                if let Err(e) = s.record_sync_run(started_at, finished_at, &json) {
                    tracing::warn!(error = %e, "could not record the sync run");
                }
            }
            Err(_) => tracing::warn!("store mutex poisoned; the sync run was not recorded"),
        },
        Err(e) => tracing::warn!(error = %e, "could not serialise the sync summary"),
    }
    tracing::info!(
        plan_id = %summary.plan_id,
        hosts = summary.hosts.len(),
        applied = summary.hosts.iter().filter(|h| h.status == "applied").count(),
        partial = summary.hosts.iter().filter(|h| h.status == "partial").count(),
        failed = summary.hosts.iter().filter(|h| h.status == "failed").count(),
        skipped = summary.hosts.iter().filter(|h| h.status == "skipped").count(),
        "applied a catalog sync plan"
    );
    Ok(summary)
}

/// The most recent completed sync, deserialised from its `sync_runs` row.
pub fn last_sync(store: &Mutex<Store>) -> Result<Option<SyncRunSummary>, IpcError> {
    let row = {
        let s = lock(store)?;
        s.last_sync_run()?
    };
    match row {
        Some(row) => Ok(Some(serde_json::from_str(&row.summary_json).map_err(
            |e| IpcError::new(codes::E_PARSE, format!("stored sync summary: {e}")),
        )?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancel::CancellationRegistry;
    use crate::events::RecordingEventBus;
    #[cfg(unix)]
    use crate::service::catalog::model::{Asset, Kind};
    use crate::service::catalog::repo;
    use crate::ssh::SshClient;
    use crate::store::Store;
    use std::sync::{Arc, Mutex};

    /// Restores `HOME` when the test (or a panic) ends: it is process-wide,
    /// and leaking a temp dir into it breaks every other test that spawns a
    /// child. Copied from `apply.rs`'s test module, which cannot export it.
    struct HomeGuard(Option<String>);
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(home) => std::env::set_var("HOME", home),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    fn load_catalog(root: &std::path::Path, files: &[(&str, &str)]) {
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        for (rel, body) in files {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let mut cat = repo::load_dir(root).unwrap();
        // `repo::load_dir` alone leaves `name` at its `Default` (`""`): real
        // catalogs get their name from the store's `catalogs` row (`load`),
        // which these tests bypass. Naming it "personal" here is what every
        // test in this module has implicitly meant by "the catalog" all
        // along, and it is now observable: `Action::catalog` (Assets M2)
        // reports it on every action.
        cat.name = "personal".into();
        super::super::registry::install(cat).unwrap();
    }

    fn one_skill(body: &str) -> Vec<(&'static str, String)> {
        vec![
            (
                "skills/s/asset.yaml",
                "kind: skill\nname: s\ndescription: d\n".to_string(),
            ),
            ("skills/s/body.md", body.to_string()),
        ]
    }

    /// Two skills ("s" and "t") plus a `core` role layer that names only
    /// "s" as a member — "t" exists in the catalog but is not in any layer
    /// the role chain reaches. Used to prove a resolved plan actually
    /// restricts to the role while the scan/inventory still covers both.
    #[cfg(unix)]
    fn two_skills_and_a_role_layer() -> Vec<(&'static str, String)> {
        vec![
            (
                "skills/s/asset.yaml",
                "kind: skill\nname: s\ndescription: d\n".to_string(),
            ),
            ("skills/s/body.md", "b\n".to_string()),
            (
                "skills/t/asset.yaml",
                "kind: skill\nname: t\ndescription: d\n".to_string(),
            ),
            ("skills/t/body.md", "b\n".to_string()),
            (
                "layers/core.yaml",
                "kind: layer\nname: core\naxis: role\nmembers:\n  - skill/s\n".to_string(),
            ),
        ]
    }

    /// A `planned` host plan carrying one `Create`. Deliberately not a
    /// `skipped_plan`: `apply_host` returns `skipped` for those all by
    /// itself, which would hide whether the cancellation branch ran.
    fn plan_with_one_create(host: &str, harness: &str) -> HostPlan {
        let mut hp = skipped_plan(host, harness, "");
        hp.status = "planned".into();
        hp.detail = None;
        hp.actions.push(plan::Action {
            kind: "skill".into(),
            name: "s".into(),
            op: ActionOp::Create,
            catalog: Some("personal".into()),
            reason: None,
            files: vec!["~/.claude/skills/s/SKILL.md".into()],
            merges: Vec::new(),
            backup: false,
            secrets: Vec::new(),
            missing_secrets: Vec::new(),
            plan: None,
            expected: Default::default(),
            secret_files: Default::default(),
            remove_entry: None,
            plugin: None,
            host_copy: None,
        });
        hp
    }

    fn store_with_local(bus: Arc<RecordingEventBus>) -> Mutex<Store> {
        let dyn_bus: Arc<dyn crate::events::EventBus> = bus;
        let store = Mutex::new(Store::open_with_bus_in_memory(dyn_bus).unwrap());
        {
            let s = store.lock().unwrap();
            s.insert_host("local", None).unwrap();
            // Multi-harness F3a: pin Codex on, so these tests plan it whether
            // or not the machine running them has the codex CLI (auto would
            // follow detection).
            s.set_host_harnesses(
                "local",
                Some(&["claude".to_string(), "codex".to_string()][..]),
            )
            .unwrap();
            // `set_host_layers` now targets the personal catalog.
            s.set_catalog_config("/p", None).unwrap();
            // Every caller here calls `load_catalog` BEFORE this function,
            // when no store (and so no real catalog id) exists yet —
            // `load_catalog` installs it under the registry's "id 0 stands
            // for personal" convention instead. That convention is fine for
            // resolving `host_layers` rows, but `asset_inventory.catalog_id`
            // (Assets M2) is a real foreign key into `catalogs(id)`, and
            // `compute_states` now stamps it from the registry catalog's own
            // id: a scan would fail its (silently swallowed) insert with no
            // `catalogs` row `0` to reference. Production's `load` always
            // stamps the store's real personal id before installing
            // (`cat.id = personal.id`); reconcile the same way here, now
            // that the store row exists to read it from.
            if let Ok(Some(cat)) = super::super::registry::personal() {
                if cat.id == 0 {
                    let real_id = s.personal_catalog().unwrap().unwrap().id;
                    super::super::registry::install_personal(super::super::repo::Catalog {
                        id: real_id,
                        ..cat
                    })
                    .unwrap();
                }
            }
        }
        store
    }

    fn harness_ids(plan: &SyncPlan) -> Vec<&str> {
        plan.hosts.iter().map(|h| h.harness.as_str()).collect()
    }

    fn load_one_skill() -> tempfile::TempDir {
        let repo_dir = tempfile::tempdir().unwrap();
        let files = one_skill("b\n");
        load_catalog(
            repo_dir.path(),
            &files
                .iter()
                .map(|(a, b)| (*a, b.as_str()))
                .collect::<Vec<_>>(),
        );
        repo_dir
    }

    /// `plan_sync` against `local` with a temp HOME: one `HostPlan` per
    /// scanning harness, fresh inventory rows persisted, and the plan parked
    /// in the registry under the id it returned.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_sync_plans_every_scanning_harness_and_registers_the_plan() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        let repo_dir = tempfile::tempdir().unwrap();
        let files = one_skill("b\n");
        load_catalog(
            repo_dir.path(),
            &files
                .iter()
                .map(|(a, b)| (*a, b.as_str()))
                .collect::<Vec<_>>(),
        );
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(PlanArgs::default(), &store, &ssh).await.unwrap();
        assert_eq!(plan.hosts.len(), 2, "{:?}", plan.hosts);
        assert!(
            plan.hosts.iter().all(|h| h.status == "planned"),
            "{:?}",
            plan.hosts
        );
        assert_eq!(plan.counts.get("create"), Some(&2), "{:?}", plan.counts);
        assert!(!plan.id.is_empty());

        let rows = store.lock().unwrap().list_inventory().unwrap();
        assert!(
            rows.iter()
                .any(|r| r.name == "s" && r.harness == "claude" && r.state == "missing"),
            "{rows:?}"
        );

        assert!(plan::registry_take(&plan.id).is_some());
        assert!(
            plan::registry_take(&plan.id).is_none(),
            "a plan is applied at most once"
        );
    }

    /// Assets M2: every action `compute_host_plan` produces for a catalog
    /// asset says which catalog it came from. With only the personal
    /// catalog loaded and no layer assignment, that is `"personal"` for
    /// everything the plan touches — the personal-only equivalence the
    /// controller notes require, plus the one new field.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_records_each_actions_catalog() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        let _repo = load_one_skill();
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(PlanArgs::default(), &store, &ssh).await.unwrap();
        let create = plan
            .hosts
            .iter()
            .flat_map(|h| &h.actions)
            .find(|a| a.name == "s" && a.op == ActionOp::Create)
            .expect("a create action for s");
        assert_eq!(create.catalog.as_deref(), Some("personal"));
    }

    /// One skill named `s`, scoped `shared` — as `skill_asset` returns it
    /// via `Asset::from_yaml` rather than `one_skill`'s on-disk YAML: the
    /// refusal path never renders the asset (it never reaches the resolved
    /// catalog), so no body/harness rendering is needed here.
    #[cfg(unix)]
    fn skill_asset(name: &str, scope: &str) -> Asset {
        Asset::from_yaml(
            Some(Kind::Skill),
            &format!("kind: skill\nname: {name}\ndescription: d\nscope: {scope}\n"),
        )
        .unwrap()
    }

    /// A `plugin_ref` with no `scope:` key — defaults to private, same as a
    /// skill with none.
    #[cfg(unix)]
    fn plugin_ref_asset(name: &str) -> Asset {
        Asset::from_yaml(
            Some(Kind::PluginRef),
            &format!(
                "kind: plugin_ref\nname: {name}\ndescription: d\nharness: claude\n\
                 marketplace: {{ name: mk, source: github, repo: o/r }}\n\
                 plugin: {name}\nversion: \"latest\"\n"
            ),
        )
        .unwrap()
    }

    /// Assets M2: a cross-catalog collision (personal's shared `s` vs.
    /// acme's `s`, on a host bound to org acme) refuses both copies, and
    /// `plan_sync` turns the refusal into a `Blocked` action carrying
    /// `effective_for_host`'s own reason — proving the refusal actually
    /// reaches the plan, not just `EffectiveSet` in isolation. Unix-only like
    /// its siblings: it plans against the real `local` host, whose harness
    /// probe finds nothing on the Windows runner.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_refused_asset_is_a_blocked_action_with_its_reason() {
        let _lock = super::super::lock_registry_for_test();
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let (personal_id, acme_id): (i64, i64) = {
            let s = store.lock().unwrap();
            let personal_id = s.personal_catalog().unwrap().unwrap().id;
            s.conn_ref()
                .execute(
                    "INSERT INTO orgs (id, name, created_at) VALUES (10, 'acme', 0)",
                    [],
                )
                .unwrap();
            s.conn_ref()
                .execute(
                    "INSERT INTO catalogs (name, repo_path, org_id, created_at) \
                     VALUES ('acme', '/a', 10, 0)",
                    [],
                )
                .unwrap();
            let acme_id: i64 = s
                .conn_ref()
                .query_row("SELECT id FROM catalogs WHERE name='acme'", [], |r| {
                    r.get(0)
                })
                .unwrap();
            // Org-bound: `local`'s only acceptable catalogs are acme (its
            // own org) and the SHARED slice of personal.
            s.set_host_org("local", Some(10)).unwrap();
            (personal_id, acme_id)
        };
        // Real store-issued ids, not the registry's "id 0 stands for
        // personal" convention: `plan_sync` also scans the FULL (union)
        // catalog for inventory, and `compute_states` now stamps
        // `catalog_id` — a real foreign key into `catalogs(id)` — from
        // whichever id the registry catalog carries.
        super::super::registry::install_personal(repo::Catalog {
            id: personal_id,
            name: "personal".into(),
            org_id: None,
            assets: vec![skill_asset("s", "shared")],
            ..Default::default()
        })
        .unwrap();
        super::super::registry::install_for_test(repo::Catalog {
            id: acme_id,
            name: "acme".into(),
            org_id: Some(10),
            assets: vec![skill_asset("s", "private")],
            ..Default::default()
        })
        .unwrap();
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(
            PlanArgs {
                host_alias: Some("local".into()),
                ..PlanArgs::default()
            },
            &store,
            &ssh,
        )
        .await
        .unwrap();

        let blocked = plan
            .hosts
            .iter()
            .flat_map(|h| &h.actions)
            .find(|a| a.name == "s" && a.op == ActionOp::Blocked)
            .expect("a blocked action for the refused asset");
        assert!(
            blocked
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("conflict: personal/s vs acme/s"),
            "{:?}",
            blocked.reason
        );
    }

    /// Fix round 1, item 2: a refused asset is absent from `eff.catalog`, so
    /// without a fix it reads to `compute_host_plan` exactly like one the
    /// catalog dropped — `manifest.orphans` schedules its `Remove` right
    /// alongside the `Blocked` action the refusal itself produces, and
    /// `apply_sync` would delete the host's existing (perfectly fine) copy.
    /// Pre-seeds `local`'s manifest with `skill/s` (as an earlier sync would
    /// have left it) before `s` collides across catalogs: the plan must
    /// carry exactly the one `Blocked` action, never a `Remove`.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_refused_asset_already_synced_is_blocked_not_removed() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::fs::write(
            home.path().join(".claude/.fleet-assets.json"),
            r#"{"version":1,"updated_at":0,"assets":{"skill/s":
                {"hash":"h","files":["~/.claude/skills/s/SKILL.md"],"merges":[],"synced_at":0}}}"#,
        )
        .unwrap();
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let (personal_id, acme_id): (i64, i64) = {
            let s = store.lock().unwrap();
            let personal_id = s.personal_catalog().unwrap().unwrap().id;
            s.conn_ref()
                .execute(
                    "INSERT INTO orgs (id, name, created_at) VALUES (10, 'acme', 0)",
                    [],
                )
                .unwrap();
            s.conn_ref()
                .execute(
                    "INSERT INTO catalogs (name, repo_path, org_id, created_at) \
                     VALUES ('acme', '/a', 10, 0)",
                    [],
                )
                .unwrap();
            let acme_id: i64 = s
                .conn_ref()
                .query_row("SELECT id FROM catalogs WHERE name='acme'", [], |r| {
                    r.get(0)
                })
                .unwrap();
            s.set_host_org("local", Some(10)).unwrap();
            (personal_id, acme_id)
        };
        super::super::registry::install_personal(repo::Catalog {
            id: personal_id,
            name: "personal".into(),
            org_id: None,
            assets: vec![skill_asset("s", "shared")],
            ..Default::default()
        })
        .unwrap();
        super::super::registry::install_for_test(repo::Catalog {
            id: acme_id,
            name: "acme".into(),
            org_id: Some(10),
            assets: vec![skill_asset("s", "private")],
            ..Default::default()
        })
        .unwrap();
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(
            PlanArgs {
                host_alias: Some("local".into()),
                ..PlanArgs::default()
            },
            &store,
            &ssh,
        )
        .await
        .unwrap();

        let claude_actions_for_s: Vec<&plan::Action> = plan
            .hosts
            .iter()
            .filter(|h| h.harness == "claude")
            .flat_map(|h| &h.actions)
            .filter(|a| a.name == "s")
            .collect();
        assert_eq!(
            claude_actions_for_s.len(),
            1,
            "exactly one action for the refused, already-synced asset: {:?}",
            claude_actions_for_s
        );
        assert_eq!(claude_actions_for_s[0].op, ActionOp::Blocked);
    }

    /// Fix round 2, item (a): `local` bound to an org, unlayered, with a
    /// private personal skill already synced (from before it had an org).
    /// `effective::compose` withholds this asset SILENTLY (no `Refusal`,
    /// per the controller ruling) — but silent must never mean destructive:
    /// the plan must not remove it, and must report a `Noop` saying why it
    /// stays.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn an_org_bound_unlayered_local_host_keeps_a_withheld_skill_already_synced() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::fs::write(
            home.path().join(".claude/.fleet-assets.json"),
            r#"{"version":1,"updated_at":0,"assets":{"skill/s":
                {"hash":"h","files":["~/.claude/skills/s/SKILL.md"],"merges":[],"synced_at":0}}}"#,
        )
        .unwrap();
        let repo_dir = tempfile::tempdir().unwrap();
        // `one_skill` writes no `scope:` key, so "s" defaults to private.
        let files = one_skill("b\n");
        load_catalog(
            repo_dir.path(),
            &files
                .iter()
                .map(|(a, b)| (*a, b.as_str()))
                .collect::<Vec<_>>(),
        );
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        {
            let s = store.lock().unwrap();
            s.conn_ref()
                .execute(
                    "INSERT INTO orgs (id, name, created_at) VALUES (10, 'acme', 0)",
                    [],
                )
                .unwrap();
            s.set_host_org("local", Some(10)).unwrap();
        }
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(
            PlanArgs {
                host_alias: Some("local".into()),
                ..PlanArgs::default()
            },
            &store,
            &ssh,
        )
        .await
        .unwrap();

        assert!(
            plan.hosts
                .iter()
                .all(|h| h.actions.iter().all(|a| a.op != ActionOp::Remove)),
            "a withheld asset must never be removed: {:?}",
            plan.hosts
        );
        let noop = plan
            .hosts
            .iter()
            .flat_map(|h| &h.actions)
            .find(|a| a.name == "s" && a.op == ActionOp::Noop)
            .expect("a Noop reporting the withheld asset");
        assert_eq!(
            noop.reason.as_deref(),
            Some("private; withheld from org host, not removed")
        );
    }

    // ---- Assets M3: admissions, held catalogs, one snapshot -------------

    /// Org 10 (`acme`) and its catalog row. Returns `(personal_id, acme_id)`.
    #[cfg(unix)]
    fn with_acme(store: &Mutex<Store>) -> (i64, i64) {
        let s = store.lock().unwrap();
        let personal_id = s.personal_catalog().unwrap().unwrap().id;
        s.conn_ref()
            .execute(
                "INSERT INTO orgs (id, name, created_at) VALUES (10, 'acme', 0)",
                [],
            )
            .unwrap();
        let acme = s.upsert_catalog("acme", "/a", None, Some(10)).unwrap();
        (personal_id, acme.id)
    }

    /// A skill with a body, so the Claude harness renders it.
    #[cfg(unix)]
    fn body_skill(name: &str, scope: &str) -> Asset {
        let mut a = skill_asset(name, scope);
        a.body = "b\n".into();
        a
    }

    /// Install personal (with `personal`'s assets) and `acme` (org 10, the
    /// rest of its fields from `acme`) under their real store ids.
    #[cfg(unix)]
    fn install(personal_id: i64, personal: Vec<Asset>, acme_id: i64, acme: repo::Catalog) {
        super::super::registry::install_personal(repo::Catalog {
            id: personal_id,
            name: "personal".into(),
            org_id: None,
            assets: personal,
            ..Default::default()
        })
        .unwrap();
        super::super::registry::install_for_test(repo::Catalog {
            id: acme_id,
            name: "acme".into(),
            org_id: Some(10),
            ..acme
        })
        .unwrap();
    }

    /// `~/.claude/.fleet-assets.json` naming each `(key, catalog)` as synced.
    #[cfg(unix)]
    fn seed_manifest(home: &std::path::Path, entries: &[(&str, &str)]) {
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        let assets: serde_json::Map<String, serde_json::Value> = entries
            .iter()
            .map(|(key, catalog)| {
                let name = key.split('/').nth(1).unwrap();
                (
                    (*key).to_string(),
                    serde_json::json!({
                        "hash": "h",
                        "files": [format!("~/.claude/skills/{name}/SKILL.md")],
                        "merges": [],
                        "synced_at": 0,
                        "catalog": catalog,
                    }),
                )
            })
            .collect();
        std::fs::write(
            home.join(".claude/.fleet-assets.json"),
            serde_json::json!({ "version": 1, "updated_at": 0, "assets": assets }).to_string(),
        )
        .unwrap();
    }

    /// The Claude harness's actions for `name` on `host`.
    #[cfg(unix)]
    fn claude_actions<'a>(plan: &'a SyncPlan, host: &str, name: &str) -> Vec<&'a plan::Action> {
        plan.hosts
            .iter()
            .filter(|h| h.harness == "claude" && h.host_alias == host)
            .flat_map(|h| &h.actions)
            .filter(|a| a.name == name)
            .collect()
    }

    #[cfg(unix)]
    fn only(host: &str) -> PlanArgs {
        PlanArgs {
            host_alias: Some(host.into()),
            ..PlanArgs::default()
        }
    }

    #[cfg(unix)]
    fn no_remove(plan: &SyncPlan) -> bool {
        plan.hosts
            .iter()
            .all(|h| h.actions.iter().all(|a| a.op != ActionOp::Remove))
    }

    /// Spec, Testing (planning): a no-org host receives an org asset only
    /// when admitted.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_no_org_host_receives_an_org_asset_only_when_admitted() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let (personal_id, acme_id) = with_acme(&store);
        install(
            personal_id,
            vec![body_skill("p", "private")],
            acme_id,
            repo::Catalog {
                assets: vec![body_skill("c", "private")],
                ..Default::default()
            },
        );
        let ssh = Arc::new(SshClient::new());

        let before = plan_sync(only("local"), &store, &ssh).await.unwrap();
        assert!(
            claude_actions(&before, "local", "c").is_empty(),
            "{:?}",
            before.hosts
        );
        assert_eq!(
            claude_actions(&before, "local", "p")[0].op,
            ActionOp::Create
        );

        store
            .lock()
            .unwrap()
            .admit_host_catalog("local", acme_id)
            .unwrap();
        let after = plan_sync(only("local"), &store, &ssh).await.unwrap();
        let c = claude_actions(&after, "local", "c");
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(c[0].op, ActionOp::Create);
        assert_eq!(c[0].catalog.as_deref(), Some("acme"));
    }

    /// M2 carry 2: losing acceptance — an unadmit, or an org change — keeps
    /// what the catalog installed, with a `Noop` saying why; never a `Remove`.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn losing_acceptance_keeps_the_org_assets_with_a_noop() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        seed_manifest(home.path(), &[("skill/c", "acme")]);
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let (personal_id, acme_id) = with_acme(&store);
        install(
            personal_id,
            vec![],
            acme_id,
            repo::Catalog {
                assets: vec![body_skill("c", "private")],
                ..Default::default()
            },
        );
        let ssh = Arc::new(SshClient::new());

        // Not admitted (an unadmit after the sync that installed it).
        let plan = plan_sync(only("local"), &store, &ssh).await.unwrap();
        assert!(no_remove(&plan), "{:?}", plan.hosts);
        let c = claude_actions(&plan, "local", "c");
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(c[0].op, ActionOp::Noop);
        assert_eq!(c[0].catalog.as_deref(), Some("acme"));
        assert!(
            c[0].reason
                .as_deref()
                .unwrap()
                .contains("not accepted by this host"),
            "{:?}",
            c[0].reason
        );

        // An org change (local joins org 11): the same.
        {
            let s = store.lock().unwrap();
            s.conn_ref()
                .execute(
                    "INSERT INTO orgs (id, name, created_at) VALUES (11, 'other', 0)",
                    [],
                )
                .unwrap();
            s.set_host_org("local", Some(11)).unwrap();
        }
        let plan = plan_sync(only("local"), &store, &ssh).await.unwrap();
        assert!(no_remove(&plan), "{:?}", plan.hosts);
        let c = claude_actions(&plan, "local", "c");
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(c[0].op, ActionOp::Noop);
    }

    /// M2 carry 1: an org catalog that failed to load (a problem entry)
    /// never makes its installed assets read as orphans.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn an_org_catalog_that_failed_to_load_never_plans_a_remove() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        seed_manifest(home.path(), &[("skill/c", "acme")]);
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let (personal_id, acme_id) = with_acme(&store);
        store
            .lock()
            .unwrap()
            .set_host_org("local", Some(10))
            .unwrap();
        install(
            personal_id,
            vec![],
            acme_id,
            repo::Catalog {
                load_error: Some("boom".into()),
                ..Default::default()
            },
        );
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(only("local"), &store, &ssh).await.unwrap();
        assert!(no_remove(&plan), "{:?}", plan.hosts);
        let c = claude_actions(&plan, "local", "c");
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(c[0].op, ActionOp::Noop);
        assert!(
            c[0].reason.as_deref().unwrap().contains("failed to load"),
            "{:?}",
            c[0].reason
        );
    }

    /// M2 carry 4: the withheld `Noop` is only for an asset nothing else
    /// supplies — here acme supplies the same name, so acme's action stands
    /// alone.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn the_withheld_noop_is_skipped_when_another_catalog_supplies_the_name() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        seed_manifest(home.path(), &[("skill/s", "personal")]);
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let (personal_id, acme_id) = with_acme(&store);
        store
            .lock()
            .unwrap()
            .set_host_org("local", Some(10))
            .unwrap();
        install(
            personal_id,
            vec![body_skill("s", "private")],
            acme_id,
            repo::Catalog {
                assets: vec![body_skill("s", "private")],
                ..Default::default()
            },
        );
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(only("local"), &store, &ssh).await.unwrap();
        let s = claude_actions(&plan, "local", "s");
        assert_eq!(s.len(), 1, "{s:?}");
        assert_eq!(s[0].catalog.as_deref(), Some("acme"));
        assert_ne!(
            s[0].reason.as_deref(),
            Some("private; withheld from org host, not removed")
        );
    }

    /// A fake `ssh` that runs the remote command locally: everything up to
    /// `-- <host>` is dropped and the rest goes to `sh -c`, which re-parses it
    /// exactly as the remote login shell would (`bash -lc '<script>'`).
    #[cfg(unix)]
    fn ssh_running_locally(dir: &std::path::Path) -> Arc<SshClient> {
        use crate::tmux::fake_exec::{write_exec, PROBE_GUARD};
        let bin = write_exec(
            dir,
            "ssh",
            &format!(
                "#!/bin/sh\n{PROBE_GUARD}\
                 case \"$*\" in *'-O check'*|*'-O exit'*) exit 0;; esac\n\
                 while [ \"$#\" -gt 0 ] && [ \"$1\" != \"--\" ]; do shift; done\n\
                 shift 2\n\
                 exec sh -c \"$*\"\n"
            ),
        );
        Arc::new(SshClient::with_ssh_binary(bin))
    }

    /// Fix round 2, item (b) — no longer vacuous (Rulings R20): a REMOTE
    /// org-bound unlayered host planned with `allow_unlayered` is really
    /// scanned (fake `ssh`, temp `HOME`), and its already-synced private
    /// skill is kept with the withheld `Noop`, never removed.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn an_org_bound_unlayered_remote_host_with_allow_unlayered_keeps_withheld_assets() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        seed_manifest(home.path(), &[("skill/s", "personal")]);
        let repo_dir = tempfile::tempdir().unwrap();
        let files = one_skill("b\n");
        load_catalog(
            repo_dir.path(),
            &files
                .iter()
                .map(|(a, b)| (*a, b.as_str()))
                .collect::<Vec<_>>(),
        );
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        {
            let s = store.lock().unwrap();
            s.insert_host("oci", Some("oci")).unwrap();
            s.update_host_probe("oci", true, None, None, 1).unwrap();
            s.conn_ref()
                .execute(
                    "INSERT INTO orgs (id, name, created_at) VALUES (10, 'acme', 0)",
                    [],
                )
                .unwrap();
            s.set_host_org("oci", Some(10)).unwrap();
        }
        let bin_dir = tempfile::tempdir().unwrap();
        let ssh = ssh_running_locally(bin_dir.path());
        let plan = plan_sync(
            PlanArgs {
                host_alias: Some("oci".into()),
                allow_unlayered: true,
                ..PlanArgs::default()
            },
            &store,
            &ssh,
        )
        .await
        .unwrap();
        let claude: Vec<&HostPlan> = plan
            .hosts
            .iter()
            .filter(|h| h.harness == "claude")
            .collect();
        assert_eq!(claude.len(), 1, "{:?}", plan.hosts);
        assert_eq!(
            claude[0].status, "planned",
            "really scanned: {:?}",
            claude[0].detail
        );
        assert!(no_remove(&plan), "{:?}", plan.hosts);
        let s = claude_actions(&plan, "oci", "s");
        assert_eq!(s.len(), 1, "{s:?}");
        assert_eq!(
            s[0].reason.as_deref(),
            Some("private; withheld from org host, not removed")
        );
    }

    /// Fix round 2, item (c): the ordering fix must apply to a `plugin_ref`
    /// too — without it, an unlayered host's dropped `plugin_ref` orphan
    /// always planned `Remove` regardless of refusal/withholding (the
    /// pre-layers backward-compat path), since that check used to run
    /// before any protection could intervene. A withheld `plugin_ref`
    /// already in the manifest must be left alone exactly like a withheld
    /// skill.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn an_org_bound_unlayered_local_host_keeps_a_withheld_plugin_ref_already_synced() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::fs::write(
            home.path().join(".claude/.fleet-assets.json"),
            r#"{"version":1,"updated_at":0,"assets":{"plugin_ref/sp":
                {"hash":"h","files":[],"merges":[],"synced_at":0}}}"#,
        )
        .unwrap();
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let personal_id = {
            let s = store.lock().unwrap();
            s.conn_ref()
                .execute(
                    "INSERT INTO orgs (id, name, created_at) VALUES (10, 'acme', 0)",
                    [],
                )
                .unwrap();
            s.set_host_org("local", Some(10)).unwrap();
            s.personal_catalog().unwrap().unwrap().id
        };
        super::super::registry::install_personal(repo::Catalog {
            id: personal_id,
            name: "personal".into(),
            org_id: None,
            // No `scope:` key — defaults to private, same as a skill.
            assets: vec![plugin_ref_asset("sp")],
            ..Default::default()
        })
        .unwrap();
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(
            PlanArgs {
                host_alias: Some("local".into()),
                ..PlanArgs::default()
            },
            &store,
            &ssh,
        )
        .await
        .unwrap();

        assert!(
            plan.hosts
                .iter()
                .all(|h| h.actions.iter().all(|a| a.op != ActionOp::Remove)),
            "a withheld plugin_ref must never be removed: {:?}",
            plan.hosts
        );
    }

    /// A `host_alias` no host row has must fail loudly (`E_NOTFOUND`) instead
    /// of silently returning an empty plan.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_sync_rejects_an_unknown_host_alias() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        let repo_dir = tempfile::tempdir().unwrap();
        let files = one_skill("b\n");
        load_catalog(
            repo_dir.path(),
            &files
                .iter()
                .map(|(a, b)| (*a, b.as_str()))
                .collect::<Vec<_>>(),
        );
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let ssh = Arc::new(SshClient::new());

        let err = plan_sync(
            PlanArgs {
                host_alias: Some("bogus".into()),
                ..Default::default()
            },
            &store,
            &ssh,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, crate::ipc_error::codes::E_NOTFOUND);
        assert!(err.message.contains("bogus"), "{}", err.message);
    }

    /// A `host_alias` that DOES name a host still plans normally — the
    /// existence check must not disturb the happy path.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_sync_with_a_known_host_alias_is_unchanged() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        let repo_dir = tempfile::tempdir().unwrap();
        let files = one_skill("b\n");
        load_catalog(
            repo_dir.path(),
            &files
                .iter()
                .map(|(a, b)| (*a, b.as_str()))
                .collect::<Vec<_>>(),
        );
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(
            PlanArgs {
                host_alias: Some("local".into()),
                ..Default::default()
            },
            &store,
            &ssh,
        )
        .await
        .unwrap();
        assert_eq!(plan.hosts.len(), 2, "{:?}", plan.hosts);
        assert!(
            plan.hosts.iter().all(|h| h.host_alias == "local"),
            "{:?}",
            plan.hosts
        );
    }

    /// The decision is a pure function: it needs no SSH, no store, no
    /// catalog. Only a remote (non-`local`), unlayered host with a
    /// non-empty resolved catalog and `allow_unlayered` off gets refused.
    #[test]
    fn refuse_unlayered_only_for_remote_unlayered_non_empty_unless_allowed() {
        assert!(refuse_unlayered("oci", false, false, false));
        assert!(!refuse_unlayered("oci", false, true, false), "allowed");
        assert!(!refuse_unlayered("oci", true, false, false), "layered");
        assert!(
            !refuse_unlayered("oci", false, false, true),
            "empty catalog"
        );
        assert!(
            !refuse_unlayered("local", false, false, false),
            "local is exempt"
        );
    }

    /// `plan_sync` skips an unlayered remote host entirely — every scanning
    /// harness comes back `skipped` with `UNLAYERED_DETAIL`, and the host is
    /// never scanned (it never reaches SSH: `SshClient::new()` here has no
    /// transport wired up, so a real scan attempt would fail loudly instead
    /// of quietly succeeding).
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_sync_skips_an_unlayered_remote_host() {
        let _lock = super::super::lock_registry_for_test();
        let repo_dir = tempfile::tempdir().unwrap();
        let files = one_skill("b\n");
        load_catalog(
            repo_dir.path(),
            &files
                .iter()
                .map(|(a, b)| (*a, b.as_str()))
                .collect::<Vec<_>>(),
        );
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        {
            let s = store.lock().unwrap();
            s.insert_host("oci", Some("oci")).unwrap();
            s.update_host_probe("oci", true, None, None, 1).unwrap();
        }
        let ssh = Arc::new(SshClient::new());
        let plan = plan_sync(
            PlanArgs {
                host_alias: Some("oci".into()),
                ..PlanArgs::default()
            },
            &store,
            &ssh,
        )
        .await
        .unwrap();
        assert!(!plan.hosts.is_empty());
        assert!(
            plan.hosts
                .iter()
                .all(|h| h.status == "skipped" && h.detail.as_deref() == Some(UNLAYERED_DETAIL)),
            "{:?}",
            plan.hosts
        );
    }

    /// Fix round 1, item 1: the unlayered guard's emptiness check must use
    /// the WHOLE loaded (union) catalog, never `eff.catalog`. An org-bound
    /// remote host with no layers and an all-private personal catalog is
    /// exactly the case `effective::compose`'s scope boundary empties
    /// `eff.catalog` for (SharedOnly acceptance, nothing shared to keep) —
    /// but the catalog itself is very much non-empty, and this is exactly
    /// the hazard the guard exists to catch: without this fix the host would
    /// be planned against an empty catalog and every manifest entry it
    /// already has would read as an orphan and get removed.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_sync_skips_an_org_bound_unlayered_host_even_when_scope_empties_it() {
        let _lock = super::super::lock_registry_for_test();
        let repo_dir = tempfile::tempdir().unwrap();
        // `one_skill` writes no `scope:` key, so "s" defaults to private.
        let files = one_skill("b\n");
        load_catalog(
            repo_dir.path(),
            &files
                .iter()
                .map(|(a, b)| (*a, b.as_str()))
                .collect::<Vec<_>>(),
        );
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        {
            let s = store.lock().unwrap();
            s.insert_host("oci", Some("oci")).unwrap();
            s.update_host_probe("oci", true, None, None, 1).unwrap();
            s.conn_ref()
                .execute(
                    "INSERT INTO orgs (id, name, created_at) VALUES (10, 'acme', 0)",
                    [],
                )
                .unwrap();
            s.set_host_org("oci", Some(10)).unwrap();
        }
        let ssh = Arc::new(SshClient::new());
        let plan = plan_sync(
            PlanArgs {
                host_alias: Some("oci".into()),
                ..PlanArgs::default()
            },
            &store,
            &ssh,
        )
        .await
        .unwrap();
        assert!(!plan.hosts.is_empty());
        assert!(
            plan.hosts
                .iter()
                .all(|h| h.status == "skipped" && h.detail.as_deref() == Some(UNLAYERED_DETAIL)),
            "{:?}",
            plan.hosts
        );
        assert!(
            plan.hosts
                .iter()
                .all(|h| h.actions.iter().all(|a| a.op != ActionOp::Remove)),
            "a skipped host carries no actions at all, Remove included: {:?}",
            plan.hosts
        );
    }

    /// The integration this whole task exists for: a host assigned a role
    /// layer gets a PLAN restricted to that role's assets, while the
    /// INVENTORY rows scan and persist stay against the FULL catalog.
    ///
    /// Neither half is provable from `layers::resolve_for_host`'s own unit
    /// tests — those only ever see the pure `Resolution`, never the
    /// `plan_sync` call site that decides which catalog goes to
    /// `compute_host_plan` and which goes to `scan_and_persist`. This test
    /// fails if `sync/mod.rs` ever reverts `&resolved.catalog` back to
    /// `&catalog` at the `compute_host_plan` call (the feature silently
    /// stops working — "t" would be planned again) and it fails just as
    /// hard if the two arguments are ever "tidied" into the same resolved
    /// catalog (inventory would stop reporting drift for the asset the
    /// layer excludes).
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_sync_plans_the_resolved_catalog_but_scans_the_full_one() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        let repo_dir = tempfile::tempdir().unwrap();
        let files = two_skills_and_a_role_layer();
        load_catalog(
            repo_dir.path(),
            &files
                .iter()
                .map(|(a, b)| (*a, b.as_str()))
                .collect::<Vec<_>>(),
        );
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        store
            .lock()
            .unwrap()
            .set_host_layers("local", Some("core"), &[])
            .unwrap();
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(PlanArgs::default(), &store, &ssh).await.unwrap();

        // (a) The PLAN only concerns "core"'s member: "t" is excluded from
        // every harness's actions, while "s" (the role's own member) is
        // still planned normally.
        for host in &plan.hosts {
            assert!(
                host.actions.iter().all(|a| a.name != "t"),
                "the resolved plan must not act on an asset the role \
                 excludes: {:?}",
                host.actions
            );
            assert!(
                host.actions.iter().any(|a| a.name == "s"),
                "the role's own member must still be planned: {:?}",
                host.actions
            );
        }

        // (b) The persisted INVENTORY still covers the FULL catalog,
        // "t" included — inventory is about the whole catalog's drift, not
        // what this host is supposed to have.
        let rows = store.lock().unwrap().list_inventory().unwrap();
        assert!(
            rows.iter().any(|r| r.name == "t" && r.harness == "claude"),
            "inventory must still report drift for an asset the layer \
             excludes: {rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.name == "s" && r.harness == "claude"),
            "{rows:?}"
        );
    }

    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_sync_removes_a_dropped_plugin_only_on_an_unlayered_host() {
        // The `layered` flag must reach the planner per host: an unassigned
        // host keeps the pre-layers `Remove` for a plugin the catalog no
        // longer has, while a host with an assignment reports it as a `Noop`.
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::fs::write(
            home.path().join(".claude/.fleet-assets.json"),
            r#"{"version":1,"updated_at":0,"assets":{"plugin_ref/gone":
                {"hash":"h","files":[],"merges":[],"synced_at":0}}}"#,
        )
        .unwrap();
        let repo_dir = tempfile::tempdir().unwrap();
        let files = two_skills_and_a_role_layer();
        load_catalog(
            repo_dir.path(),
            &files
                .iter()
                .map(|(a, b)| (*a, b.as_str()))
                .collect::<Vec<_>>(),
        );
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let ssh = Arc::new(SshClient::new());

        let gone_op = |plan: &SyncPlan| {
            plan.hosts
                .iter()
                .filter(|h| h.harness == "claude")
                .flat_map(|h| h.actions.iter())
                .find(|a| a.name == "gone")
                .map(|a| a.op)
        };

        let unlayered = plan_sync(PlanArgs::default(), &store, &ssh).await.unwrap();
        assert_eq!(gone_op(&unlayered), Some(ActionOp::Remove));

        store
            .lock()
            .unwrap()
            .set_host_layers("local", Some("core"), &[])
            .unwrap();
        let layered = plan_sync(PlanArgs::default(), &store, &ssh).await.unwrap();
        assert_eq!(gone_op(&layered), Some(ActionOp::Noop));
    }

    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn assigning_a_layer_that_drops_an_installed_asset_plans_its_removal() {
        // The destructive half of layering: a host that fleet already synced
        // `skill/t` to, and whose new role no longer includes it, must plan
        // `Remove` — through resolve → Manifest::orphans, not special code.
        // Without the assignment, `t` is still in the effective catalog and
        // must NOT be removed.
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::fs::write(
            home.path().join(".claude/.fleet-assets.json"),
            r#"{"version":1,"updated_at":0,"assets":{"skill/t":
                {"hash":"h","files":[],"merges":[],"synced_at":0}}}"#,
        )
        .unwrap();
        let repo_dir = tempfile::tempdir().unwrap();
        let files = two_skills_and_a_role_layer();
        load_catalog(
            repo_dir.path(),
            &files
                .iter()
                .map(|(a, b)| (*a, b.as_str()))
                .collect::<Vec<_>>(),
        );
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let ssh = Arc::new(SshClient::new());

        let t_op = |plan: &SyncPlan| {
            plan.hosts
                .iter()
                .filter(|h| h.harness == "claude")
                .flat_map(|h| h.actions.iter())
                .find(|a| a.name == "t")
                .map(|a| a.op)
        };

        let before = plan_sync(PlanArgs::default(), &store, &ssh).await.unwrap();
        assert_ne!(t_op(&before), Some(ActionOp::Remove));

        store
            .lock()
            .unwrap()
            .set_host_layers("local", Some("core"), &[])
            .unwrap();
        let after = plan_sync(PlanArgs::default(), &store, &ssh).await.unwrap();
        assert_eq!(t_op(&after), Some(ActionOp::Remove));
    }

    #[tokio::test]
    async fn apply_sync_rejects_an_unknown_plan_id() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let ssh = Arc::new(SshClient::new());
        let reg = CancellationRegistry::new();
        let err = apply_sync(
            ApplyArgs {
                plan_id: "no-such-plan".into(),
                force_partial: false,
                call_id: None,
            },
            &store,
            &ssh,
            &reg,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, crate::ipc_error::codes::E_SYNC_PLAN_STALE);
    }

    /// A plan whose actions are blocked on an unresolved `${NAME}` is
    /// refused by name (never by value) unless `force_partial` says to apply
    /// the rest anyway.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn apply_sync_refuses_a_plan_blocked_on_missing_secrets() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        let repo_dir = tempfile::tempdir().unwrap();
        let files = one_skill("token ${MISSING_SECRET}\n");
        load_catalog(
            repo_dir.path(),
            &files
                .iter()
                .map(|(a, b)| (*a, b.as_str()))
                .collect::<Vec<_>>(),
        );
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        let ssh = Arc::new(SshClient::new());
        let reg = CancellationRegistry::new();

        let plan = plan_sync(PlanArgs::default(), &store, &ssh).await.unwrap();
        assert_eq!(plan.counts.get("blocked"), Some(&2), "{:?}", plan.counts);
        let err = apply_sync(
            ApplyArgs {
                plan_id: plan.id.clone(),
                force_partial: false,
                call_id: None,
            },
            &store,
            &ssh,
            &reg,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, crate::ipc_error::codes::E_SECRET_MISSING);
        assert!(err.message.contains("MISSING_SECRET"), "{}", err.message);

        // Refusing did not consume the plan: the SAME id still applies,
        // with `force_partial`, and the blocked actions are reported as
        // such.
        let summary = apply_sync(
            ApplyArgs {
                plan_id: plan.id.clone(),
                force_partial: true,
                call_id: None,
            },
            &store,
            &ssh,
            &reg,
        )
        .await
        .expect("the refused plan is still in the registry under its id");
        assert!(summary
            .hosts
            .iter()
            .all(|h| h.actions.iter().all(|a| a.outcome == "blocked")));
    }

    /// End to end on `local` with a temp HOME: plan, apply, and re-plan.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_apply_and_replan_locally() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        let repo_dir = tempfile::tempdir().unwrap();
        let files = one_skill("b\n");
        load_catalog(
            repo_dir.path(),
            &files
                .iter()
                .map(|(a, b)| (*a, b.as_str()))
                .collect::<Vec<_>>(),
        );
        let bus = Arc::new(RecordingEventBus::new());
        let store = store_with_local(bus.clone());
        let ssh = Arc::new(SshClient::new());
        let reg = CancellationRegistry::new();

        let plan = plan_sync(PlanArgs::default(), &store, &ssh).await.unwrap();
        bus.take();
        let summary = apply_sync(
            ApplyArgs {
                plan_id: plan.id.clone(),
                force_partial: false,
                call_id: None,
            },
            &store,
            &ssh,
            &reg,
        )
        .await
        .unwrap();
        assert_eq!(summary.plan_id, plan.id);
        assert_eq!(summary.hosts.len(), 2);
        assert!(
            summary.hosts.iter().all(|h| h.status == "applied"),
            "{:?}",
            summary.hosts
        );
        assert!(home.path().join(".claude/skills/s/SKILL.md").exists());

        let events = bus.take();
        assert!(
            events.iter().any(|e| e == "sync:progress:local:claude:0/2"),
            "the first pair announces itself at 0/2: {events:?}"
        );
        assert!(
            events.iter().any(|e| e == "sync:progress:::2/2"),
            "a completed run ends at 2/2, naming no pair: {events:?}"
        );

        // The run is in `sync_runs` and round-trips through `last_sync`.
        let last = last_sync(&store).unwrap().expect("a run was recorded");
        assert_eq!(last.plan_id, plan.id);
        assert_eq!(last.hosts.len(), 2);
        assert!(last.finished_at >= last.started_at);

        // The post-apply re-scan left the matrix `in_sync`, and a second
        // plan has nothing left to do.
        let rows = store.lock().unwrap().list_inventory().unwrap();
        assert!(
            rows.iter()
                .any(|r| r.name == "s" && r.harness == "claude" && r.state == "in_sync"),
            "{rows:?}"
        );
        let again = plan_sync(PlanArgs::default(), &store, &ssh).await.unwrap();
        assert_eq!(again.counts.get("noop"), Some(&2), "{:?}", again.counts);
    }

    /// A sync cancelled before it starts touches nothing: every pair is
    /// reported `skipped` / `cancelled`, no progress is emitted (there is
    /// no pair about to be applied, and the run never completes), and the
    /// run is still recorded so the history says how far it got.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn apply_sync_skips_every_pair_when_the_token_is_already_cancelled() {
        let _lock = super::super::lock_registry_for_test();
        let bus = Arc::new(RecordingEventBus::new());
        let store = store_with_local(bus.clone());
        let ssh = Arc::new(SshClient::new());

        let id = plan::registry_put(SyncPlan::new(vec![
            plan_with_one_create("local", "claude"),
            plan_with_one_create("local", "codex"),
        ]));
        // Cancelled up front. The token goes in directly rather than
        // through `reg.bind`: `apply_sync` mints and binds its OWN token
        // (`bind` replaces), so a pre-bound one would simply be dropped —
        // hence the `_with` seam.
        let token = CancellationToken::new();
        token.cancel();
        bus.take();

        let summary = apply_sync_with(
            ApplyArgs {
                plan_id: id.clone(),
                force_partial: false,
                call_id: None,
            },
            &store,
            &ssh,
            token,
        )
        .await
        .unwrap();

        assert_eq!(summary.hosts.len(), 2);
        assert!(
            summary
                .hosts
                .iter()
                .all(|h| h.status == "skipped" && h.detail.as_deref() == Some("cancelled")),
            "{:?}",
            summary.hosts
        );
        assert!(
            summary
                .hosts
                .iter()
                .flat_map(|h| &h.actions)
                .all(|a| { a.outcome == "skipped" && a.detail.as_deref() == Some("cancelled") }),
            "{:?}",
            summary.hosts
        );
        let events = bus.take();
        assert!(
            !events.iter().any(|e| e.starts_with("sync:progress")),
            "a cancelled run announces nothing: {events:?}"
        );
        let last = last_sync(&store)
            .unwrap()
            .expect("the run is in the history");
        assert_eq!(last.plan_id, id);
    }

    /// F3a: a host that turned Codex off (and has no Codex manifest) gets no
    /// Codex plan, and a Codex inventory row an earlier scan left is gone.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_sync_leaves_codex_out_on_a_host_that_turned_it_off() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        let _repo = load_one_skill();
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        {
            let s = store.lock().unwrap();
            s.set_host_harnesses("local", Some(&["claude".to_string()][..]))
                .unwrap();
            s.replace_host_inventory(
                "local",
                "codex",
                &[crate::store::AssetInventoryRow {
                    host_alias: "local".into(),
                    harness: "codex".into(),
                    kind: "skill".into(),
                    name: "s".into(),
                    state: "missing".into(),
                    ..Default::default()
                }],
            )
            .unwrap();
        }
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(PlanArgs::default(), &store, &ssh).await.unwrap();
        assert_eq!(harness_ids(&plan), vec!["claude"], "{:?}", plan.hosts);
        let rows = store.lock().unwrap().list_inventory().unwrap();
        assert!(
            rows.iter().any(|r| r.name == "s" && r.harness == "claude"),
            "{rows:?}"
        );
        assert!(rows.iter().all(|r| r.harness != "codex"), "{rows:?}");
    }

    /// F3a: on auto, a host with Codex's own session logs
    /// (`~/.codex/sessions`) is planned and inventoried for Codex
    /// (deterministic whatever PATH holds: the directory alone is
    /// detection).
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_sync_plans_codex_where_auto_finds_it() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        std::fs::create_dir_all(home.path().join(".codex/sessions")).unwrap();
        let _repo = load_one_skill();
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        store
            .lock()
            .unwrap()
            .set_host_harnesses("local", None)
            .unwrap();
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(PlanArgs::default(), &store, &ssh).await.unwrap();
        let codex = plan
            .hosts
            .iter()
            .find(|h| h.harness == "codex")
            .expect("codex is planned");
        assert_eq!(codex.status, "planned");
        assert_eq!(codex.detail, None);
        assert_eq!(
            codex.actions.iter().find(|a| a.name == "s").map(|a| a.op),
            Some(ActionOp::Create)
        );
        let rows = store.lock().unwrap().list_inventory().unwrap();
        assert!(
            rows.iter()
                .any(|r| r.harness == "codex" && r.name == "s" && r.state == "missing"),
            "{rows:?}"
        );
    }

    /// F3a: turning Codex off on a host fleet already synced Codex assets to
    /// retires it — the plan is removals only, and says why — while Claude
    /// is planned as before.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_sync_retires_codex_when_turned_off_but_still_managed() {
        let _lock = super::super::lock_registry_for_test();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard(std::env::var("HOME").ok());
        std::env::set_var("HOME", home.path());
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        std::fs::write(
            home.path().join(".codex/.fleet-assets.json"),
            r#"{"version":1,"updated_at":0,"assets":{"skill/s":
                {"hash":"h","files":["~/.codex/skills/s/SKILL.md"],"merges":[],"synced_at":0}}}"#,
        )
        .unwrap();
        let _repo = load_one_skill();
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        store
            .lock()
            .unwrap()
            .set_host_harnesses("local", Some(&["claude".to_string()][..]))
            .unwrap();
        let ssh = Arc::new(SshClient::new());

        let plan = plan_sync(PlanArgs::default(), &store, &ssh).await.unwrap();
        let codex = plan.hosts.iter().find(|h| h.harness == "codex").unwrap();
        assert_eq!(codex.status, "planned");
        assert_eq!(
            codex.detail.as_deref(),
            Some(harness_set::retiring_detail("codex").as_str())
        );
        assert_eq!(
            codex
                .actions
                .iter()
                .map(|a| (a.name.as_str(), a.op))
                .collect::<Vec<_>>(),
            vec![("s", ActionOp::Remove)]
        );
        let claude = plan.hosts.iter().find(|h| h.harness == "claude").unwrap();
        assert_eq!(
            claude.actions.iter().find(|a| a.name == "s").map(|a| a.op),
            Some(ActionOp::Create)
        );
        let rows = store.lock().unwrap().list_inventory().unwrap();
        assert!(
            rows.iter()
                .any(|r| r.harness == "codex" && r.name == "s" && r.state == "orphan"),
            "{rows:?}"
        );
    }

    /// F3a: a host skipped before its scan (here: unlayered and remote) is
    /// reported under Claude and the harnesses it lists — nothing is known
    /// about the others until a scan runs.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_host_skipped_before_its_scan_reports_claude_and_its_listed_harnesses() {
        let _lock = super::super::lock_registry_for_test();
        let _repo = load_one_skill();
        let store = store_with_local(Arc::new(RecordingEventBus::new()));
        {
            let s = store.lock().unwrap();
            s.insert_host("oci", Some("oci")).unwrap();
            s.update_host_probe("oci", true, None, None, 1).unwrap();
        }
        let ssh = Arc::new(SshClient::new());
        let args = || PlanArgs {
            host_alias: Some("oci".into()),
            ..PlanArgs::default()
        };

        let auto = plan_sync(args(), &store, &ssh).await.unwrap();
        assert_eq!(harness_ids(&auto), vec!["claude"], "{:?}", auto.hosts);

        store
            .lock()
            .unwrap()
            .set_host_harnesses(
                "oci",
                Some(&["claude".to_string(), "codex".to_string()][..]),
            )
            .unwrap();
        let listed = plan_sync(args(), &store, &ssh).await.unwrap();
        assert_eq!(harness_ids(&listed), vec!["claude", "codex"]);
        assert!(listed
            .hosts
            .iter()
            .all(|h| h.status == "skipped" && h.detail.as_deref() == Some(UNLAYERED_DETAIL)));
    }
}
