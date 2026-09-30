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
        AdminCall, DeleteSecretArgs, GetAssetArgs, LayerRef, LayerTemplateArgs, LoadArgs,
        ResolvePreviewArgs, SetHostLayersArgs, SetSecretArgs, WriteLayerArgs,
    },
    author::{
        self, AddResourceArgs, AddResourceBytesArgs, AssetRef, CommitPendingArgs, CreateArgs,
        LintAll, LintReport, RemoveResourceArgs, UpdateArgs, WriteResult,
    },
    author_session::{self, SpawnAuthorArgs},
    import::ImportReport,
    inventory,
    model::Asset,
    repo::RepoStatus,
    sync::{self, plan::SyncPlan, ApplyArgs, PlanArgs, SyncRunSummary},
    validate::{check_layer_name, check_name, check_resource_path, check_secret_name},
    AssetDetail, AssetListing, ConfigureArgs, ImportArgs,
};
use fleet_core::ssh::SshClient;
use fleet_core::store::{
    AssetInventoryRow, CatalogConfigRow, HostLayerRow, SecretRow, SessionRow, Store,
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
        match backend.hub() {
            Some(hub) => hub.catalog_list_assets().await,
            None => catalog::list_assets(store),
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
            None => catalog::configure(args, store),
        }
    }

    pub async fn catalog_load(
        backend: &FleetBackend,
        args: LoadArgs,
        store: &Mutex<Store>,
    ) -> Result<fleet_core::events::CatalogSummary, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("catalog_load", &AdminCall::Load(args)).await,
            None => catalog::load(args.pull, store),
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
            None => catalog::set_host_layers(
                &args.host_alias,
                args.role.as_deref(),
                &args.contexts.iter().map(String::as_str).collect::<Vec<_>>(),
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
            None => author::write_layer(&args.layer, store),
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
            None => author::delete_layer(&args.name, store),
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
            None => author::create(args, store),
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
            None => author::update(args, store),
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
            None => author::delete_asset(args, store),
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
            None => author::add_resource(args, store),
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
            None => author::remove_resource(args, store),
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
            None => author::commit_pending(args, store),
        }
    }

    pub async fn catalog_push(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<RepoStatus, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("catalog_push", &AdminCall::Push).await,
            None => author::push(store),
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
