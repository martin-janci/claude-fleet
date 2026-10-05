//! Tauri IPC wrappers for the asset catalog. Logic lives in
//! `service::catalog`; this file only adapts `tauri::State` to plain refs.
//!
//! On a desktop paired with a hub, every command here but one routes to the
//! hub's `catalog_admin` tool with its own arguments
//! (`service::catalog::admin::AdminCall`), so a client the operator granted
//! (`fleet-hub client grant <name> assets`) manages the hub's catalog from
//! this window; the hub refuses an ungranted one with `E_FORBIDDEN`, and the
//! panel falls back to its read-only overview. `catalog_spawn_author_session`
//! still refuses — see `backend::verdicts`.

use crate::backend::FleetBackend;
use fleet_core::cancel::CancellationRegistry;
use fleet_core::ipc_error::lock;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::catalog::{
    self,
    admin::{
        AdminCall, AdmitArgs, CatalogNameArgs, DeleteSecretArgs, GetAssetArgs, LayerRef,
        LayerTemplateArgs, LoadArgs, ResolvePreviewArgs, SetHostHarnessesArgs, SetHostLayersArgs,
        SetSecretArgs, WriteLayerArgs,
    },
    author::{
        self, AddResourceArgs, AddResourceBytesArgs, AssetRef, CommitPendingArgs, CreateArgs,
        LintAll, LintReport, RemoveResourceArgs, UpdateArgs, WriteResult,
    },
    author_session::{self, SpawnAuthorArgs},
    catalogs::{AddCatalogArgs, CatalogStatus},
    changesets::{self, ChangesetSummary, ChangesetView, LayerChange},
    drift_diff::{self, DriftDiff, DriftDiffArgs},
    import::ImportReport,
    inventory,
    model::{Asset, Kind},
    repo::{CommitEntry, RepoStatus},
    sync::{self, plan::SyncPlan, ApplyArgs, PlanArgs, SyncRunSummary},
    validate::{check_layer_name, check_name, check_resource_path, check_secret_name},
    AssetDetail, AssetListing, ConfigureArgs, ImportArgs,
};
use fleet_core::ssh::SshClient;
use fleet_core::store::{
    AssetInventoryRow, CatalogConfigRow, CatalogRemoval, HostLayerRow, HostRow, SecretRow,
    SessionRow, Store,
};
use std::sync::{Arc, Mutex};
use tauri::State;

#[tauri::command]
pub async fn catalog_config(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Option<CatalogConfigRow>, IpcError> {
    routed::catalog_config(&backend, &store).await
}

#[tauri::command]
pub async fn catalog_configure(
    backend: State<'_, Arc<FleetBackend>>,
    args: ConfigureArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<CatalogConfigRow, IpcError> {
    routed::catalog_configure(&backend, args, &store).await
}

#[tauri::command]
pub async fn catalog_load(
    backend: State<'_, Arc<FleetBackend>>,
    args: LoadArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<fleet_core::events::CatalogSummary, IpcError> {
    routed::catalog_load(&backend, args, &store).await
}

#[tauri::command]
pub async fn catalog_list_assets(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<AssetListing, IpcError> {
    routed::catalog_list_assets(&backend, &store).await
}

#[tauri::command]
pub async fn catalog_get_asset(
    backend: State<'_, Arc<FleetBackend>>,
    args: GetAssetArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<AssetDetail, IpcError> {
    routed::catalog_get_asset(&backend, args, &store).await
}

#[tauri::command]
pub async fn catalog_list_layers(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<catalog::LayerListing, IpcError> {
    routed::catalog_list_layers(&backend, &store).await
}

#[tauri::command]
pub async fn catalog_resolve_preview(
    backend: State<'_, Arc<FleetBackend>>,
    args: ResolvePreviewArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<catalog::resolve::Resolution, IpcError> {
    routed::catalog_resolve_preview(&backend, args, &store).await
}

#[tauri::command]
pub async fn catalog_propose_layers(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<catalog::propose::LayerProposal, IpcError> {
    routed::catalog_propose_layers(&backend, &store).await
}

#[tauri::command]
pub async fn catalog_set_host_layers(
    backend: State<'_, Arc<FleetBackend>>,
    args: SetHostLayersArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<HostLayerRow>, IpcError> {
    routed::catalog_set_host_layers(&backend, args, &store).await
}

#[tauri::command]
pub async fn catalog_set_host_harnesses(
    backend: State<'_, Arc<FleetBackend>>,
    args: SetHostHarnessesArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<HostRow, IpcError> {
    routed::catalog_set_host_harnesses(&backend, args, &store).await
}

#[tauri::command]
pub async fn catalog_layer_template(
    backend: State<'_, Arc<FleetBackend>>,
    args: LayerTemplateArgs,
) -> Result<catalog::layer::Layer, IpcError> {
    routed::catalog_layer_template(&backend, args).await
}

#[tauri::command]
pub async fn catalog_write_layer(
    backend: State<'_, Arc<FleetBackend>>,
    args: WriteLayerArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<String, IpcError> {
    routed::catalog_write_layer(&backend, args, &store).await
}

#[tauri::command]
pub async fn catalog_delete_layer(
    backend: State<'_, Arc<FleetBackend>>,
    args: LayerRef,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<String, IpcError> {
    routed::catalog_delete_layer(&backend, args, &store).await
}

#[tauri::command]
pub async fn catalog_import_host(
    backend: State<'_, Arc<FleetBackend>>,
    args: ImportArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<ImportReport, IpcError> {
    routed::catalog_import_host(&backend, args, &store, &ssh).await
}

#[tauri::command]
pub async fn assets_scan_hosts(
    backend: State<'_, Arc<FleetBackend>>,
    args: ScanArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<inventory::HostScanResult>, IpcError> {
    routed::assets_scan_hosts(&backend, &store, &ssh, args.host_alias).await
}

#[tauri::command]
pub async fn assets_inventory(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<AssetInventoryRow>, IpcError> {
    routed::assets_inventory(&backend, &store).await
}

#[tauri::command]
pub async fn catalog_plan_sync(
    backend: State<'_, Arc<FleetBackend>>,
    args: PlanArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SyncPlan, IpcError> {
    routed::catalog_plan_sync(&backend, args, &store, &ssh).await
}

#[tauri::command]
pub async fn catalog_apply_sync(
    backend: State<'_, Arc<FleetBackend>>,
    args: ApplyArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<SyncRunSummary, IpcError> {
    routed::catalog_apply_sync(&backend, args, &store, &ssh, &reg).await
}

#[tauri::command]
pub async fn catalog_last_sync(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Option<SyncRunSummary>, IpcError> {
    routed::catalog_last_sync(&backend, &store).await
}

#[tauri::command]
pub async fn catalog_list_secrets(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<SecretRow>, IpcError> {
    routed::catalog_list_secrets(&backend, &store).await
}

#[tauri::command]
pub async fn catalog_set_secret(
    backend: State<'_, Arc<FleetBackend>>,
    args: SetSecretArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    routed::catalog_set_secret(&backend, args, &store).await
}

#[tauri::command]
pub async fn catalog_delete_secret(
    backend: State<'_, Arc<FleetBackend>>,
    args: DeleteSecretArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<bool, IpcError> {
    routed::catalog_delete_secret(&backend, args, &store).await
}

// ------------------------------------------------------------- authoring

#[tauri::command]
pub async fn catalog_create_asset(
    backend: State<'_, Arc<FleetBackend>>,
    args: CreateArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WriteResult, IpcError> {
    routed::catalog_create_asset(&backend, args, &store).await
}

#[tauri::command]
pub async fn catalog_update_asset(
    backend: State<'_, Arc<FleetBackend>>,
    args: UpdateArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WriteResult, IpcError> {
    routed::catalog_update_asset(&backend, args, &store).await
}

#[tauri::command]
pub async fn catalog_delete_asset(
    backend: State<'_, Arc<FleetBackend>>,
    args: AssetRef,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<String, IpcError> {
    routed::catalog_delete_asset(&backend, args, &store).await
}

#[tauri::command]
pub async fn catalog_add_resource(
    backend: State<'_, Arc<FleetBackend>>,
    args: AddResourceArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WriteResult, IpcError> {
    routed::catalog_add_resource(&backend, args, &store).await
}

#[tauri::command]
pub async fn catalog_remove_resource(
    backend: State<'_, Arc<FleetBackend>>,
    args: RemoveResourceArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WriteResult, IpcError> {
    routed::catalog_remove_resource(&backend, args, &store).await
}

#[tauri::command]
pub async fn catalog_lint_asset(
    backend: State<'_, Arc<FleetBackend>>,
    args: AssetRef,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<LintReport, IpcError> {
    routed::catalog_lint_asset(&backend, args, &store).await
}

#[tauri::command]
pub async fn catalog_lint_all(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<LintAll, IpcError> {
    routed::catalog_lint_all(&backend, &store).await
}

#[tauri::command]
pub async fn catalog_commit_pending(
    backend: State<'_, Arc<FleetBackend>>,
    args: CommitPendingArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<String, IpcError> {
    routed::catalog_commit_pending(&backend, args, &store).await
}

#[tauri::command]
pub async fn catalog_push(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<RepoStatus, IpcError> {
    routed::catalog_push(&backend, &store).await
}

#[tauri::command]
pub async fn catalog_repo_status(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<RepoStatus, IpcError> {
    routed::catalog_repo_status(&backend, &store).await
}

/// Assets M5 (R13, R24): every catalog and its load state — the footer's
/// chips.
#[tauri::command]
pub async fn catalog_list_catalogs(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<CatalogStatus>, IpcError> {
    routed::catalog_list_catalogs(&backend, &store).await
}

/// Assets M5 (R13, R14): the changeset cards, read only — the Inbox's
/// proposed cards. Opening, applying, undoing and dismissing one, rejecting
/// its items and proposing new ones are the card-verb commands below
/// (`catalog_get_changeset` … `catalog_propose_changesets`).
#[tauri::command]
pub async fn catalog_list_changesets(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<ChangesetSummary>, IpcError> {
    routed::catalog_list_changesets(&backend, &store).await
}

/// `catalog_get_changeset` / `_undo_` / `_dismiss_`'s arguments.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChangesetIdArgs {
    pub id: i64,
}

/// `catalog_apply_changeset`'s arguments: the card and, optionally, the
/// items to run (default: every pending item but "needs a look"; a drift
/// card needs exactly one).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApplyChangesetArgs {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub positions: Option<Vec<i64>>,
}

/// `catalog_reject_changeset_items`' arguments.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RejectItemsArgs {
    pub id: i64,
    pub positions: Vec<i64>,
}

/// `catalog_propose_layer_change`'s arguments.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LayerChangeArgs {
    pub change: LayerChange,
}

/// Assets M6 (R8): one card in full — the Inspector's Items / Hosts / Diff.
#[tauri::command]
pub async fn catalog_get_changeset(
    backend: State<'_, Arc<FleetBackend>>,
    args: ChangesetIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<ChangesetView, IpcError> {
    routed::catalog_get_changeset(&backend, args, &store).await
}

/// Assets M6 (R8): apply a card (or the named items) — on a hub, its
/// `changesets { apply }`, which checks the caller's grant on every catalog
/// the items touch and, for a rollout or restore, the hub's confirm gate.
#[tauri::command]
pub async fn catalog_apply_changeset(
    backend: State<'_, Arc<FleetBackend>>,
    args: ApplyChangesetArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<ChangesetView, IpcError> {
    routed::catalog_apply_changeset(&backend, args, &store, &ssh).await
}

/// Assets M6 (R8): undo an applied card — its commits are reverted and the
/// host-layer snapshot restored; allowed only for the latest applied card in
/// each catalog it touched.
#[tauri::command]
pub async fn catalog_undo_changeset(
    backend: State<'_, Arc<FleetBackend>>,
    args: ChangesetIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<ChangesetView, IpcError> {
    routed::catalog_undo_changeset(&backend, args, &store).await
}

/// Assets M6 (R8): dismiss a proposed card; nothing is written.
#[tauri::command]
pub async fn catalog_dismiss_changeset(
    backend: State<'_, Arc<FleetBackend>>,
    args: ChangesetIdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<ChangesetView, IpcError> {
    routed::catalog_dismiss_changeset(&backend, args, &store).await
}

/// Assets M6 (R8): a person's no to some of a card's items; the card is
/// dismissed once nothing is pending.
#[tauri::command]
pub async fn catalog_reject_changeset_items(
    backend: State<'_, Arc<FleetBackend>>,
    args: RejectItemsArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<ChangesetView, IpcError> {
    routed::catalog_reject_changeset_items(&backend, args, &store).await
}

/// Assets M6 (R5): run the proposers now — the new cards, newest first.
#[tauri::command]
pub async fn catalog_propose_changesets(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<ChangesetSummary>, IpcError> {
    routed::catalog_propose_changesets(&backend, &store).await
}

/// Assets M6 (R5): a layer change (create, rename, move a member) as a
/// proposed card — never a direct commit.
#[tauri::command]
pub async fn catalog_propose_layer_change(
    backend: State<'_, Arc<FleetBackend>>,
    args: LayerChangeArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<ChangesetView, IpcError> {
    routed::catalog_propose_layer_change(&backend, args, &store).await
}

/// `catalog_drift_diff`'s arguments: the host and the asset, and the
/// asset's catalog (`None` or `personal` = the personal catalog).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DriftDiffCmdArgs {
    pub host_alias: String,
    pub kind: Kind,
    pub name: String,
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub catalog: Option<String>,
}

/// Assets M6 (R9): add an org catalog (name, checkout path, remote, org) —
/// on a hub, the master's alone.
#[tauri::command]
pub async fn catalog_add_catalog(
    backend: State<'_, Arc<FleetBackend>>,
    args: AddCatalogArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<CatalogStatus, IpcError> {
    routed::catalog_add_catalog(&backend, args, &store).await
}

/// Assets M6 (R9): remove an org catalog — config only, the checkout stays.
#[tauri::command]
pub async fn catalog_remove_catalog(
    backend: State<'_, Arc<FleetBackend>>,
    args: CatalogNameArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<CatalogRemoval, IpcError> {
    routed::catalog_remove_catalog(&backend, args, &store).await
}

/// Assets M6 (R9): a host with no org accepts an org catalog. Answers the
/// host's admitted catalogs.
#[tauri::command]
pub async fn catalog_admit_catalog(
    backend: State<'_, Arc<FleetBackend>>,
    args: AdmitArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<String>, IpcError> {
    routed::catalog_admit_catalog(&backend, args, &store).await
}

/// Assets M6 (R9): take an admission back. Answers the host's remaining
/// admitted catalogs.
#[tauri::command]
pub async fn catalog_unadmit_catalog(
    backend: State<'_, Arc<FleetBackend>>,
    args: AdmitArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<String>, IpcError> {
    routed::catalog_unadmit_catalog(&backend, args, &store).await
}

/// Assets M6 (R9): one catalog's layers and the hosts' assignments in it,
/// by name (`personal` = the personal catalog).
#[tauri::command]
pub async fn catalog_list_layers_in(
    backend: State<'_, Arc<FleetBackend>>,
    args: CatalogNameArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<catalog::LayerListing, IpcError> {
    routed::catalog_list_layers_in(&backend, args, &store).await
}

/// Assets M6 (R9): one host's effective set with provenance — the summary
/// view (no asset bodies), the same answer as the MCP `resolve_preview`.
#[tauri::command]
pub async fn catalog_host_provenance(
    backend: State<'_, Arc<FleetBackend>>,
    args: ResolvePreviewArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<catalog::resolve::ResolutionView, IpcError> {
    routed::catalog_host_provenance(&backend, args, &store).await
}

/// Assets M6 (R7, R9): the catalog's and the host's text of a drifted
/// asset's files, for the Drift card's DiffView.
#[tauri::command]
pub async fn catalog_drift_diff(
    backend: State<'_, Arc<FleetBackend>>,
    args: DriftDiffCmdArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<DriftDiff, IpcError> {
    routed::catalog_drift_diff(&backend, args, &store, &ssh).await
}

/// Assets M5 (R13, R24): one catalog's dirty / ahead / behind, by name.
#[tauri::command]
pub async fn catalog_repo_status_in(
    backend: State<'_, Arc<FleetBackend>>,
    args: CatalogNameArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<RepoStatus, IpcError> {
    routed::catalog_repo_status_in(&backend, args, &store).await
}

/// `catalog_asset_history`'s arguments: an asset, and its catalog (`None`
/// or `personal` = the personal catalog).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AssetHistoryArgs {
    pub kind: Kind,
    pub name: String,
    #[serde(default)]
    pub catalog: Option<String>,
}

/// Assets M5 (R13, R19): the commits that touched one asset — the
/// Inspector's History.
#[tauri::command]
pub async fn catalog_asset_history(
    backend: State<'_, Arc<FleetBackend>>,
    args: AssetHistoryArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<CommitEntry>, IpcError> {
    routed::catalog_asset_history(&backend, args, &store).await
}

#[tauri::command]
pub async fn catalog_template(
    backend: State<'_, Arc<FleetBackend>>,
    args: AssetRef,
) -> Result<Asset, IpcError> {
    routed::catalog_template(&backend, args).await
}

#[tauri::command]
pub async fn catalog_spawn_author_session(
    backend: State<'_, Arc<FleetBackend>>,
    args: SpawnAuthorArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<SessionRow, IpcError> {
    backend.refuse_local_only("catalog_spawn_author_session")?;
    if let Some(name) = args.name.as_deref() {
        check_name(name)?;
    }
    author_session::spawn_author_session(args, &store, &ssh, &reg).await
}

#[derive(serde::Deserialize)]
pub struct ScanArgs {
    pub host_alias: Option<String>,
}

/// Every catalog command that routes: on a hub, to `list_assets` /
/// `scan_assets` (open to any paired client) or `catalog_admin` (the master
/// or a granted client); standalone, to the local catalog. Everything the
/// local path checks, the hub's `catalog_admin` checks again on its side.
pub(crate) mod routed {
    use super::*;

    pub async fn catalog_list_assets(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<AssetListing, IpcError> {
        // Assets M5: this desktop lists every catalog (fix round 1: opt-in,
        // so an older desktop keeps the personal-only listing).
        match backend.hub() {
            Some(hub) => hub.catalog_list_assets().await,
            None => catalog::list_assets_in(store, catalog::ListingScope::Every),
        }
    }

    pub async fn assets_scan_hosts(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        host_alias: Option<String>,
    ) -> Result<Vec<inventory::HostScanResult>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.assets_scan_hosts(host_alias).await,
            None => inventory::scan_hosts(store, ssh, host_alias.as_deref()).await,
        }
    }

    pub async fn catalog_config(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Option<CatalogConfigRow>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("catalog_config", &AdminCall::Config).await,
            None => catalog::config(store),
        }
    }

    pub async fn catalog_configure(
        backend: &FleetBackend,
        args: ConfigureArgs,
        store: &Mutex<Store>,
    ) -> Result<CatalogConfigRow, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_configure", &AdminCall::Configure(args))
                    .await
            }
            None => {
                // PF7: waits for a changeset apply in flight.
                let _busy = catalog::changesets::authoring_lock().await;
                catalog::configure(args, store)
            }
        }
    }

    pub async fn catalog_load(
        backend: &FleetBackend,
        args: LoadArgs,
        store: &Mutex<Store>,
    ) -> Result<fleet_core::events::CatalogSummary, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("catalog_load", &AdminCall::Load(args)).await,
            None => {
                // PF7: waits for a changeset apply in flight.
                let _busy = catalog::changesets::authoring_lock().await;
                catalog::load_all(args.pull, store)
            }
        }
    }

    pub async fn catalog_get_asset(
        backend: &FleetBackend,
        args: GetAssetArgs,
        store: &Mutex<Store>,
    ) -> Result<AssetDetail, IpcError> {
        check_name(&args.name)?;
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_get_asset", &AdminCall::GetAsset(args))
                    .await
            }
            None => catalog::get_asset(args.kind, &args.name, store),
        }
    }

    pub async fn catalog_list_layers(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<catalog::LayerListing, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_list_layers", &AdminCall::ListLayers)
                    .await
            }
            None => catalog::list_layers(store),
        }
    }

    pub async fn catalog_resolve_preview(
        backend: &FleetBackend,
        args: ResolvePreviewArgs,
        store: &Mutex<Store>,
    ) -> Result<catalog::resolve::Resolution, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_resolve_preview", &AdminCall::ResolvePreview(args))
                    .await
            }
            None => catalog::resolve_preview(&args.host_alias, store),
        }
    }

    pub async fn catalog_propose_layers(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<catalog::propose::LayerProposal, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_propose_layers", &AdminCall::ProposeLayers)
                    .await
            }
            None => catalog::propose::propose_layers(store),
        }
    }

    pub async fn catalog_set_host_layers(
        backend: &FleetBackend,
        args: SetHostLayersArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<HostLayerRow>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_set_host_layers", &AdminCall::SetHostLayers(args))
                    .await
            }
            None => {
                // PF7: waits for a changeset apply in flight.
                let _busy = catalog::changesets::authoring_lock().await;
                catalog::set_host_layers(
                    &args.host_alias,
                    args.role.as_deref(),
                    &args.contexts.iter().map(String::as_str).collect::<Vec<_>>(),
                    store,
                )
            }
        }
    }

    pub async fn catalog_set_host_harnesses(
        backend: &FleetBackend,
        args: SetHostHarnessesArgs,
        store: &Mutex<Store>,
    ) -> Result<HostRow, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "catalog_set_host_harnesses",
                    &AdminCall::SetHostHarnesses(args),
                )
                .await
            }
            None => catalog::harness_set::set_host_harnesses(
                &args.host_alias,
                args.harnesses.as_deref(),
                store,
            ),
        }
    }

    pub async fn catalog_layer_template(
        backend: &FleetBackend,
        args: LayerTemplateArgs,
    ) -> Result<catalog::layer::Layer, IpcError> {
        check_layer_name(&args.name)?;
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_layer_template", &AdminCall::LayerTemplate(args))
                    .await
            }
            None => Ok(author::layer_template(&args.name, args.axis)),
        }
    }

    pub async fn catalog_write_layer(
        backend: &FleetBackend,
        args: WriteLayerArgs,
        store: &Mutex<Store>,
    ) -> Result<String, IpcError> {
        check_layer_name(&args.layer.name)?;
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_write_layer", &AdminCall::WriteLayer(args))
                    .await
            }
            None => {
                // PF7: waits for a changeset apply in flight.
                let _busy = catalog::changesets::authoring_lock().await;
                author::write_layer(&args.layer, store)
            }
        }
    }

    pub async fn catalog_delete_layer(
        backend: &FleetBackend,
        args: LayerRef,
        store: &Mutex<Store>,
    ) -> Result<String, IpcError> {
        check_layer_name(&args.name)?;
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_delete_layer", &AdminCall::DeleteLayer(args))
                    .await
            }
            None => {
                // PF7: waits for a changeset apply in flight.
                let _busy = catalog::changesets::authoring_lock().await;
                author::delete_layer(&args.name, store)
            }
        }
    }

    pub async fn assets_inventory(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<AssetInventoryRow>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("assets_inventory", &AdminCall::Inventory).await,
            None => catalog::inventory(store),
        }
    }

    pub async fn catalog_plan_sync(
        backend: &FleetBackend,
        args: PlanArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<SyncPlan, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_plan_sync", &AdminCall::PlanSync(args))
                    .await
            }
            None => sync::plan_sync(args, store, ssh).await,
        }
    }

    /// On a hub the plan id is the hub's (its `plan_sync` parked the plan),
    /// and `call_id` is dropped there: a cancellation id means something only
    /// in the process that minted it.
    pub async fn catalog_apply_sync(
        backend: &FleetBackend,
        args: ApplyArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
    ) -> Result<SyncRunSummary, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_apply_sync", &AdminCall::ApplySync(args))
                    .await
            }
            None => sync::apply_sync(args, store, ssh, reg).await,
        }
    }

    pub async fn catalog_last_sync(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Option<SyncRunSummary>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("catalog_last_sync", &AdminCall::LastSync).await,
            None => sync::last_sync(store),
        }
    }

    pub async fn catalog_list_secrets(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<SecretRow>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_list_secrets", &AdminCall::ListSecrets)
                    .await
            }
            None => Ok(lock(store)?.list_secrets()?),
        }
    }

    pub async fn catalog_set_secret(
        backend: &FleetBackend,
        args: SetSecretArgs,
        store: &Mutex<Store>,
    ) -> Result<(), IpcError> {
        check_secret_name(&args.name)?;
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_set_secret", &AdminCall::SetSecret(args))
                    .await
            }
            None => {
                Ok(lock(store)?.set_secret(&args.name, args.host_alias.as_deref(), &args.value)?)
            }
        }
    }

    pub async fn catalog_delete_secret(
        backend: &FleetBackend,
        args: DeleteSecretArgs,
        store: &Mutex<Store>,
    ) -> Result<bool, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_delete_secret", &AdminCall::DeleteSecret(args))
                    .await
            }
            None => Ok(lock(store)?.delete_secret(&args.name, args.host_alias.as_deref())?),
        }
    }

    pub async fn catalog_import_host(
        backend: &FleetBackend,
        args: ImportArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<ImportReport, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_import_host", &AdminCall::ImportHost(args))
                    .await
            }
            None => {
                let token = lock(store)?.get_setting(fleet_core::mcp::SETTING_TOKEN)?;
                // PF7: waits for a changeset apply in flight.
                let _busy = catalog::changesets::authoring_lock().await;
                catalog::import_host(args, store, ssh, token.as_deref()).await
            }
        }
    }

    pub async fn catalog_create_asset(
        backend: &FleetBackend,
        args: CreateArgs,
        store: &Mutex<Store>,
    ) -> Result<WriteResult, IpcError> {
        check_name(&args.name)?;
        if let Some(from) = args.duplicate_from.as_deref() {
            check_name(from)?;
        }
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_create_asset", &AdminCall::CreateAsset(args))
                    .await
            }
            None => {
                // PF7: waits for a changeset apply in flight.
                let _busy = catalog::changesets::authoring_lock().await;
                author::create(args, store)
            }
        }
    }

    pub async fn catalog_update_asset(
        backend: &FleetBackend,
        args: UpdateArgs,
        store: &Mutex<Store>,
    ) -> Result<WriteResult, IpcError> {
        check_name(&args.asset.header.name)?;
        for r in &args.asset.resources {
            check_resource_path(&r.rel_path)?;
        }
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "catalog_update_asset",
                    &AdminCall::UpdateAsset(Box::new(args)),
                )
                .await
            }
            None => {
                // PF7: waits for a changeset apply in flight.
                let _busy = catalog::changesets::authoring_lock().await;
                author::update(args, store)
            }
        }
    }

    pub async fn catalog_delete_asset(
        backend: &FleetBackend,
        args: AssetRef,
        store: &Mutex<Store>,
    ) -> Result<String, IpcError> {
        check_name(&args.name)?;
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_delete_asset", &AdminCall::DeleteAsset(args))
                    .await
            }
            None => {
                // PF7: waits for a changeset apply in flight.
                let _busy = catalog::changesets::authoring_lock().await;
                author::delete_asset(args, store)
            }
        }
    }

    /// The file is on THIS machine (it came from this window's file dialog),
    /// so on a hub it is read here — with the same checks and size limit as
    /// the local path — and its bytes are what cross the wire.
    pub async fn catalog_add_resource(
        backend: &FleetBackend,
        args: AddResourceArgs,
        store: &Mutex<Store>,
    ) -> Result<WriteResult, IpcError> {
        check_name(&args.name)?;
        if let Some(rel_path) = args.rel_path.as_deref() {
            check_resource_path(rel_path)?;
        }
        match backend.hub() {
            Some(hub) => {
                let (rel_path, bytes) =
                    author::read_resource_file(&args.local_path, args.rel_path)?;
                let call = AdminCall::AddResourceBytes(AddResourceBytesArgs {
                    kind: args.kind,
                    name: args.name,
                    rel_path,
                    bytes,
                });
                hub.route("catalog_add_resource", &call).await
            }
            None => {
                // PF7: waits for a changeset apply in flight.
                let _busy = catalog::changesets::authoring_lock().await;
                author::add_resource(args, store)
            }
        }
    }

    pub async fn catalog_remove_resource(
        backend: &FleetBackend,
        args: RemoveResourceArgs,
        store: &Mutex<Store>,
    ) -> Result<WriteResult, IpcError> {
        check_name(&args.name)?;
        check_resource_path(&args.rel_path)?;
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_remove_resource", &AdminCall::RemoveResource(args))
                    .await
            }
            None => {
                // PF7: waits for a changeset apply in flight.
                let _busy = catalog::changesets::authoring_lock().await;
                author::remove_resource(args, store)
            }
        }
    }

    pub async fn catalog_lint_asset(
        backend: &FleetBackend,
        args: AssetRef,
        store: &Mutex<Store>,
    ) -> Result<LintReport, IpcError> {
        check_name(&args.name)?;
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_lint_asset", &AdminCall::LintAsset(args))
                    .await
            }
            None => author::lint_asset(args, store),
        }
    }

    pub async fn catalog_lint_all(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<LintAll, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("catalog_lint_all", &AdminCall::LintAll).await,
            None => author::lint_everything(store),
        }
    }

    pub async fn catalog_commit_pending(
        backend: &FleetBackend,
        args: CommitPendingArgs,
        store: &Mutex<Store>,
    ) -> Result<String, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_commit_pending", &AdminCall::CommitPending(args))
                    .await
            }
            None => {
                // PF7: waits for a changeset apply in flight.
                let _busy = catalog::changesets::authoring_lock().await;
                author::commit_pending(args, store)
            }
        }
    }

    pub async fn catalog_push(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<RepoStatus, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("catalog_push", &AdminCall::Push).await,
            None => {
                // PF7: waits for a changeset apply in flight.
                let _busy = catalog::changesets::authoring_lock().await;
                author::push(store)
            }
        }
    }

    pub async fn catalog_repo_status(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<RepoStatus, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_repo_status", &AdminCall::RepoStatus)
                    .await
            }
            None => author::repo_status(store),
        }
    }

    /// `call` addressed to `catalog` through the tool's own top-level
    /// `catalog` parameter (Assets M5, R13) — never inside `args`, where a
    /// hub would ignore it and answer for personal.
    fn in_catalog(call: &AdminCall, catalog: &str) -> Result<serde_json::Value, IpcError> {
        let mut v = serde_json::to_value(call).map_err(|e| {
            IpcError::new(
                fleet_core::ipc_error::codes::E_INTERNAL,
                format!("{} could not be encoded for the hub: {e}", call.action()),
            )
        })?;
        v["catalog"] = serde_json::Value::String(catalog.to_string());
        Ok(v)
    }

    pub async fn catalog_list_catalogs(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<CatalogStatus>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_list_catalogs", &AdminCall::ListCatalogs)
                    .await
            }
            None => catalog::catalogs::list_catalogs(store),
        }
    }

    pub async fn catalog_list_changesets(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<ChangesetSummary>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "catalog_list_changesets",
                    &serde_json::json!({ "action": "list" }),
                )
                .await
            }
            None => catalog::changesets::list(store),
        }
    }

    /// Hub tool arguments for the card verbs that take only a card id.
    fn card_call(action: &str, id: i64) -> serde_json::Value {
        serde_json::json!({ "action": action, "id": id })
    }

    pub async fn catalog_get_changeset(
        backend: &FleetBackend,
        args: ChangesetIdArgs,
        store: &Mutex<Store>,
    ) -> Result<ChangesetView, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_get_changeset", &card_call("list", args.id))
                    .await
            }
            None => changesets::get(args.id, store),
        }
    }

    pub async fn catalog_apply_changeset(
        backend: &FleetBackend,
        args: ApplyChangesetArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<ChangesetView, IpcError> {
        match backend.hub() {
            Some(hub) => {
                let mut body = card_call("apply", args.id);
                if let Some(p) = &args.positions {
                    body["positions"] = serde_json::json!(p);
                }
                hub.route("catalog_apply_changeset", &body).await
            }
            None => {
                changesets::apply::apply(
                    changesets::apply::ApplyArgs {
                        id: args.id,
                        positions: args.positions,
                    },
                    store,
                    ssh,
                )
                .await
            }
        }
    }

    pub async fn catalog_undo_changeset(
        backend: &FleetBackend,
        args: ChangesetIdArgs,
        store: &Mutex<Store>,
    ) -> Result<ChangesetView, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_undo_changeset", &card_call("undo", args.id))
                    .await
            }
            None => changesets::undo::undo(args.id, store).await,
        }
    }

    pub async fn catalog_dismiss_changeset(
        backend: &FleetBackend,
        args: ChangesetIdArgs,
        store: &Mutex<Store>,
    ) -> Result<ChangesetView, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_dismiss_changeset", &card_call("dismiss", args.id))
                    .await
            }
            None => changesets::undo::dismiss(args.id, store).await,
        }
    }

    pub async fn catalog_reject_changeset_items(
        backend: &FleetBackend,
        args: RejectItemsArgs,
        store: &Mutex<Store>,
    ) -> Result<ChangesetView, IpcError> {
        match backend.hub() {
            Some(hub) => {
                let mut body = card_call("reject_item", args.id);
                body["positions"] = serde_json::json!(args.positions);
                hub.route("catalog_reject_changeset_items", &body).await
            }
            None => changesets::undo::reject_items(args.id, &args.positions, store).await,
        }
    }

    pub async fn catalog_propose_changesets(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<ChangesetSummary>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "catalog_propose_changesets",
                    &serde_json::json!({ "action": "propose" }),
                )
                .await
            }
            None => changesets::propose(store).await,
        }
    }

    pub async fn catalog_propose_layer_change(
        backend: &FleetBackend,
        args: LayerChangeArgs,
        store: &Mutex<Store>,
    ) -> Result<ChangesetView, IpcError> {
        match backend.hub() {
            Some(hub) => {
                let change = serde_json::to_value(&args.change).map_err(|e| {
                    IpcError::new(
                        fleet_core::ipc_error::codes::E_INTERNAL,
                        format!("the layer change could not be encoded for the hub: {e}"),
                    )
                })?;
                hub.route(
                    "catalog_propose_layer_change",
                    &serde_json::json!({ "action": "propose_layer", "change": change }),
                )
                .await
            }
            None => changesets::propose_layer(args.change, store).await,
        }
    }

    pub async fn catalog_add_catalog(
        backend: &FleetBackend,
        args: AddCatalogArgs,
        store: &Mutex<Store>,
    ) -> Result<CatalogStatus, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_add_catalog", &AdminCall::AddCatalog(args))
                    .await
            }
            None => {
                // PF7: waits for a changeset apply in flight.
                let _busy = catalog::changesets::authoring_lock().await;
                catalog::catalogs::add_catalog(args, store)
            }
        }
    }

    pub async fn catalog_remove_catalog(
        backend: &FleetBackend,
        args: CatalogNameArgs,
        store: &Mutex<Store>,
    ) -> Result<CatalogRemoval, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_remove_catalog", &AdminCall::RemoveCatalog(args))
                    .await
            }
            None => {
                // PF7: waits for a changeset apply in flight.
                let _busy = catalog::changesets::authoring_lock().await;
                catalog::catalogs::remove_catalog(&args.name, store)
            }
        }
    }

    pub async fn catalog_admit_catalog(
        backend: &FleetBackend,
        args: AdmitArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<String>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_admit_catalog", &AdminCall::AdmitCatalog(args))
                    .await
            }
            None => catalog::catalogs::admit(&args.host_alias, &args.catalog, store),
        }
    }

    pub async fn catalog_unadmit_catalog(
        backend: &FleetBackend,
        args: AdmitArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<String>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_unadmit_catalog", &AdminCall::UnadmitCatalog(args))
                    .await
            }
            None => catalog::catalogs::unadmit(&args.host_alias, &args.catalog, store),
        }
    }

    pub async fn catalog_list_layers_in(
        backend: &FleetBackend,
        args: CatalogNameArgs,
        store: &Mutex<Store>,
    ) -> Result<catalog::LayerListing, IpcError> {
        let personal = args.name == catalog::catalogs::PERSONAL;
        match backend.hub() {
            // Personal sends no `catalog`, exactly `catalog_list_layers`.
            Some(hub) if personal => {
                hub.route("catalog_list_layers_in", &AdminCall::ListLayers)
                    .await
            }
            Some(hub) => {
                hub.route(
                    "catalog_list_layers_in",
                    &in_catalog(&AdminCall::ListLayers, &args.name)?,
                )
                .await
            }
            None if personal => catalog::list_layers(store),
            None => {
                let row = catalog::catalogs::catalog_named(&args.name, store)?;
                catalog::list_layers_for(&row, store)
            }
        }
    }

    pub async fn catalog_host_provenance(
        backend: &FleetBackend,
        args: ResolvePreviewArgs,
        store: &Mutex<Store>,
    ) -> Result<catalog::resolve::ResolutionView, IpcError> {
        match backend.hub() {
            // The MCP `resolve_preview` tool, whose answer is this view.
            Some(hub) => {
                hub.route(
                    "catalog_host_provenance",
                    &serde_json::json!({ "host_alias": args.host_alias }),
                )
                .await
            }
            None => {
                let res = catalog::resolve_preview(&args.host_alias, store)?;
                Ok(catalog::resolve::ResolutionView::of(&res))
            }
        }
    }

    pub async fn catalog_drift_diff(
        backend: &FleetBackend,
        args: DriftDiffCmdArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
    ) -> Result<DriftDiff, IpcError> {
        check_name(&args.name)?;
        let named = args
            .catalog
            .filter(|c| c.as_str() != catalog::catalogs::PERSONAL);
        let call = DriftDiffArgs {
            host_alias: args.host_alias,
            kind: args.kind,
            name: args.name,
            harness: args.harness,
        };
        match (backend.hub(), named) {
            (Some(hub), Some(c)) => {
                hub.route(
                    "catalog_drift_diff",
                    &in_catalog(&AdminCall::DriftDiff(call), &c)?,
                )
                .await
            }
            (Some(hub), None) => {
                hub.route("catalog_drift_diff", &AdminCall::DriftDiff(call))
                    .await
            }
            (None, Some(c)) => {
                let row = catalog::catalogs::catalog_named(&c, store)?;
                drift_diff::drift_diff(catalog::CatalogTarget::Row(&row), call, store, ssh).await
            }
            (None, None) => {
                drift_diff::drift_diff(catalog::CatalogTarget::Personal, call, store, ssh).await
            }
        }
    }

    pub async fn catalog_repo_status_in(
        backend: &FleetBackend,
        args: CatalogNameArgs,
        store: &Mutex<Store>,
    ) -> Result<RepoStatus, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "catalog_repo_status_in",
                    &in_catalog(&AdminCall::RepoStatus, &args.name)?,
                )
                .await
            }
            None if args.name == catalog::catalogs::PERSONAL => author::repo_status(store),
            None => {
                let row = catalog::catalogs::catalog_named(&args.name, store)?;
                author::repo_status_in(catalog::CatalogTarget::Row(&row), store)
            }
        }
    }

    pub async fn catalog_asset_history(
        backend: &FleetBackend,
        args: AssetHistoryArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<CommitEntry>, IpcError> {
        check_name(&args.name)?;
        let asset = AssetRef {
            kind: args.kind,
            name: args.name,
        };
        let named = args
            .catalog
            .filter(|c| c.as_str() != catalog::catalogs::PERSONAL);
        match (backend.hub(), named) {
            (Some(hub), Some(c)) => {
                hub.route(
                    "catalog_asset_history",
                    &in_catalog(&AdminCall::AssetHistory(asset), &c)?,
                )
                .await
            }
            (Some(hub), None) => {
                hub.route("catalog_asset_history", &AdminCall::AssetHistory(asset))
                    .await
            }
            (None, Some(c)) => {
                let row = catalog::catalogs::catalog_named(&c, store)?;
                author::asset_history_in(catalog::CatalogTarget::Row(&row), asset, store)
            }
            (None, None) => {
                author::asset_history_in(catalog::CatalogTarget::Personal, asset, store)
            }
        }
    }

    pub async fn catalog_template(
        backend: &FleetBackend,
        args: AssetRef,
    ) -> Result<Asset, IpcError> {
        check_name(&args.name)?;
        match backend.hub() {
            Some(hub) => {
                hub.route("catalog_template", &AdminCall::Template(args))
                    .await
            }
            None => Ok(author::template(args.kind, &args.name)),
        }
    }
}
