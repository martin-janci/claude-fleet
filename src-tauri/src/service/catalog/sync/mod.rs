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
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlanArgs {
    pub host_alias: Option<String>,
    pub kind: Option<super::model::Kind>,
    pub name: Option<String>,
}

/// Apply a plan `plan_sync` computed and parked in the registry.
#[derive(Debug, Clone, Deserialize)]
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
}

/// The loaded catalog, cloned out of the global so nothing holds the
/// `CATALOG` lock across an await.
fn catalog() -> Result<super::repo::Catalog, IpcError> {
    let g = super::CATALOG
        .read()
        .map_err(|_| IpcError::new(codes::E_LOCK, "catalog lock poisoned"))?;
    g.clone().ok_or_else(|| {
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
/// `replace_host_inventory` already emits) and hand the snapshot and its
/// managed manifest back to the caller. `secrets` is resolved by the caller
/// BEFORE this await — `secrets::resolve` takes the store lock internally.
/// Persisting is best-effort: a scan is still usable if the rows could not
/// be written.
async fn scan_and_persist(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    catalog: &super::repo::Catalog,
    harness: &dyn super::harness::Harness,
    host_alias: &str,
    secrets: &BTreeMap<String, String>,
) -> Result<(super::harness::HostSnapshot, Manifest), IpcError> {
    let scanned_at = super::now_secs();
    let snap = super::inventory::scan_host_harness(ssh, host_alias, harness).await?;
    let manifest = Manifest::from_snapshot(&snap, harness.manifest_path());
    let rows = super::inventory::compute_states(
        catalog, harness, host_alias, &snap, &manifest, secrets, scanned_at,
    );
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
    Ok((snap, manifest))
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
    let catalog = catalog()?;
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
        if h.alias != "local" && !h.reachable {
            for hn in &scanning {
                host_plans.push(skipped_plan(&h.alias, hn.id(), "unreachable"));
            }
            continue;
        }
        // Before any await: `resolve` takes the store lock internally.
        let secrets = match secrets::resolve(store, &h.alias) {
            Ok(v) => v,
            Err(e) => {
                for hn in &scanning {
                    host_plans.push(skipped_plan(&h.alias, hn.id(), &e.message));
                }
                continue;
            }
        };
        // Whether this host has ANY layer assignment at all, before
        // resolving it: a host with no `host_layers` row must plan a dropped
        // `plugin_ref` exactly as it did before layers existed (`Remove`),
        // not the "reported, not removed" `Noop` that only makes sense once
        // a host is actually opting into layered assignment.
        let layered = {
            let s = lock(store)?;
            !s.get_host_layers(&h.alias)?.is_empty()
        };
        // Resolve the host's layers ONCE per host, before its harnesses are
        // planned. The scan below still uses the FULL catalog: inventory is
        // about the whole catalog's drift, while the PLAN is about what this
        // host is supposed to have.
        let resolved = match layers::resolve_for_host(store, &catalog, &h.alias) {
            Ok(r) => r,
            Err(e) => {
                for harness in &scanning {
                    host_plans.push(skipped_plan(&h.alias, harness.id(), &e.message));
                }
                continue;
            }
        };
        let host_filter = PlanFilter {
            layered,
            ..filter.clone()
        };
        for harness in &scanning {
            let harness = *harness;
            match scan_and_persist(store, ssh, &catalog, harness, &h.alias, &secrets).await {
                Ok((snap, manifest)) => host_plans.push(plan::compute_host_plan(
                    &resolved.catalog,
                    harness,
                    &h.alias,
                    &snap,
                    &manifest,
                    &secrets,
                    &host_filter,
                )),
                Err(e) => host_plans.push(skipped_plan(&h.alias, harness.id(), &e.message)),
            }
        }
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
    if let Err(e) = scan_and_persist(store, ssh, catalog, harness, host_alias, &secrets).await {
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

/// [`apply_sync`] with the cancellation token supplied directly, so a test
/// can prove what an already-cancelled run does without racing the
/// registry.
pub async fn apply_sync_with(
    args: ApplyArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    token: CancellationToken,
) -> Result<SyncRunSummary, IpcError> {
    let (expires_at, computed) =
        plan::registry_take_with_expiry(&args.plan_id).ok_or_else(|| {
            IpcError::new(
                codes::E_SYNC_PLAN_STALE,
                "that sync plan is unknown or has expired; compute a new one",
            )
        })?;

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
    let catalog = catalog().ok();
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
        let cat = repo::load_dir(root).unwrap();
        *super::super::CATALOG.write().unwrap() = Some(cat);
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
        });
        hp
    }

    fn store_with_local(bus: Arc<RecordingEventBus>) -> Mutex<Store> {
        let dyn_bus: Arc<dyn crate::events::EventBus> = bus;
        let store = Mutex::new(Store::open_with_bus_in_memory(dyn_bus).unwrap());
        store.lock().unwrap().insert_host("local", None).unwrap();
        store
    }

    /// `plan_sync` against `local` with a temp HOME: one `HostPlan` per
    /// scanning harness, fresh inventory rows persisted, and the plan parked
    /// in the registry under the id it returned.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_sync_plans_every_scanning_harness_and_registers_the_plan() {
        let _lock = super::super::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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

    /// A `host_alias` no host row has must fail loudly (`E_NOTFOUND`) instead
    /// of silently returning an empty plan.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_sync_rejects_an_unknown_host_alias() {
        let _lock = super::super::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
        let _lock = super::super::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_sync_plans_the_resolved_catalog_but_scans_the_full_one() {
        let _lock = super::super::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn apply_sync_refuses_a_plan_blocked_on_missing_secrets() {
        let _lock = super::super::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn plan_apply_and_replan_locally() {
        let _lock = super::super::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
    #[tokio::test]
    async fn apply_sync_skips_every_pair_when_the_token_is_already_cancelled() {
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
}
