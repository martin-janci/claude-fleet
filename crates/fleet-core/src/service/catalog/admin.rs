//! Managing the asset catalog as one call type, for the hub's `catalog_admin`
//! tool.
//!
//! The desktop manages its own catalog through ~30 Tauri commands, each a
//! thin wrapper over a function in this module's siblings. A desktop paired
//! with a hub reaches the hub's catalog instead, and the rule there is
//! *parity or refusal* (`docs/hub.md`): a command routes only where its
//! arguments map one to one onto the tool's. [`AdminCall`] makes that true by
//! construction — each variant carries the command's own argument struct, the
//! desktop serialises exactly what it was given, and [`run`] answers with the
//! same type the local call returns, as JSON.
//!
//! Who may call it is the tool's business, not this module's: the master, or
//! a paired client the operator granted (`fleet-hub client grant <name>
//! assets`, [`crate::store::Store::client_is_assets_admin`]).

use super::author::{
    self, AddResourceBytesArgs, AssetRef, CommitPendingArgs, CreateArgs, RemoveResourceArgs,
    UpdateArgs,
};
use super::layer::{Axis, Layer};
use super::model::{is_valid_name, Kind};
use super::sync::{self, secrets::is_valid_secret_name, ApplyArgs, PlanArgs};
use super::ConfigureArgs;
use crate::cancel::CancellationRegistry;
use crate::ipc_error::{codes, lock, IpcError};
use crate::ssh::SshClient;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadArgs {
    #[serde(default)]
    pub pull: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAssetArgs {
    pub kind: Kind,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvePreviewArgs {
    pub host_alias: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetHostLayersArgs {
    pub host_alias: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub contexts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerTemplateArgs {
    pub name: String,
    pub axis: Axis,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteLayerArgs {
    pub layer: Layer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerRef {
    pub name: String,
}

/// No `Debug`: the value is a secret, and a derived `Debug` is one log line
/// away from printing it.
#[derive(Clone, Serialize, Deserialize)]
pub struct SetSecretArgs {
    pub name: String,
    #[serde(default)]
    pub host_alias: Option<String>,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteSecretArgs {
    pub name: String,
    #[serde(default)]
    pub host_alias: Option<String>,
}

/// One catalog operation. On the wire: `{"action": "<snake_case>", "args":
/// {…}}`, `args` absent for the operations that take none.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "action", content = "args", rename_all = "snake_case")]
pub enum AdminCall {
    Config,
    Configure(ConfigureArgs),
    Load(LoadArgs),
    GetAsset(GetAssetArgs),
    ListLayers,
    ResolvePreview(ResolvePreviewArgs),
    ProposeLayers,
    SetHostLayers(SetHostLayersArgs),
    LayerTemplate(LayerTemplateArgs),
    WriteLayer(WriteLayerArgs),
    DeleteLayer(LayerRef),
    Inventory,
    PlanSync(PlanArgs),
    ApplySync(ApplyArgs),
    LastSync,
    ListSecrets,
    SetSecret(SetSecretArgs),
    DeleteSecret(DeleteSecretArgs),
    CreateAsset(CreateArgs),
    /// Boxed: a whole asset (body and resources) is by far the largest
    /// variant, and every other call would pay for its size.
    UpdateAsset(Box<UpdateArgs>),
    DeleteAsset(AssetRef),
    /// `catalog_add_resource` with the file already read on the caller's
    /// side ([`author::read_resource_file`]): the path names a file on the
    /// desktop, which the hub cannot open.
    AddResourceBytes(AddResourceBytesArgs),
    RemoveResource(RemoveResourceArgs),
    LintAsset(AssetRef),
    LintAll,
    CommitPending(CommitPendingArgs),
    Push,
    RepoStatus,
    Template(AssetRef),
}

impl AdminCall {
    /// The wire name of this call's action, for audit lines and errors.
    pub fn action(&self) -> &'static str {
        match self {
            AdminCall::Config => "config",
            AdminCall::Configure(_) => "configure",
            AdminCall::Load(_) => "load",
            AdminCall::GetAsset(_) => "get_asset",
            AdminCall::ListLayers => "list_layers",
            AdminCall::ResolvePreview(_) => "resolve_preview",
            AdminCall::ProposeLayers => "propose_layers",
            AdminCall::SetHostLayers(_) => "set_host_layers",
            AdminCall::LayerTemplate(_) => "layer_template",
            AdminCall::WriteLayer(_) => "write_layer",
            AdminCall::DeleteLayer(_) => "delete_layer",
            AdminCall::Inventory => "inventory",
            AdminCall::PlanSync(_) => "plan_sync",
            AdminCall::ApplySync(_) => "apply_sync",
            AdminCall::LastSync => "last_sync",
            AdminCall::ListSecrets => "list_secrets",
            AdminCall::SetSecret(_) => "set_secret",
            AdminCall::DeleteSecret(_) => "delete_secret",
            AdminCall::CreateAsset(_) => "create_asset",
            AdminCall::UpdateAsset(_) => "update_asset",
            AdminCall::DeleteAsset(_) => "delete_asset",
            AdminCall::AddResourceBytes(_) => "add_resource_bytes",
            AdminCall::RemoveResource(_) => "remove_resource",
            AdminCall::LintAsset(_) => "lint_asset",
            AdminCall::LintAll => "lint_all",
            AdminCall::CommitPending(_) => "commit_pending",
            AdminCall::Push => "push",
            AdminCall::RepoStatus => "repo_status",
            AdminCall::Template(_) => "template",
        }
    }

    /// True for the calls that only read: nothing in the checkout, the
    /// store or on a host changes.
    pub fn is_read(&self) -> bool {
        matches!(
            self,
            AdminCall::Config
                | AdminCall::GetAsset(_)
                | AdminCall::ListLayers
                | AdminCall::ResolvePreview(_)
                | AdminCall::ProposeLayers
                | AdminCall::LayerTemplate(_)
                | AdminCall::Inventory
                | AdminCall::PlanSync(_)
                | AdminCall::LastSync
                | AdminCall::ListSecrets
                | AdminCall::LintAsset(_)
                | AdminCall::LintAll
                | AdminCall::RepoStatus
                | AdminCall::Template(_)
        )
    }
}

pub fn check_name(name: &str) -> Result<(), IpcError> {
    if is_valid_name(name) {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_INVALID,
            format!("invalid asset name '{name}'"),
        ))
    }
}

pub fn check_layer_name(name: &str) -> Result<(), IpcError> {
    if is_valid_name(name) {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_INVALID,
            format!("invalid layer name '{name}'"),
        ))
    }
}

pub fn check_resource_path(rel_path: &str) -> Result<(), IpcError> {
    if super::repo::valid_resource_rel_path(rel_path) {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_INVALID,
            format!("invalid resource path: {rel_path}"),
        ))
    }
}

pub fn check_secret_name(name: &str) -> Result<(), IpcError> {
    if is_valid_secret_name(name) {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_INVALID,
            format!("invalid secret name '{name}'; use [A-Z0-9_]+"),
        ))
    }
}

fn json<T: Serialize>(v: T) -> Result<serde_json::Value, IpcError> {
    serde_json::to_value(v)
        .map_err(|e| IpcError::new(codes::E_SERIALIZE, format!("encode the answer: {e}")))
}

/// Run one call against this process's catalog and answer the same value
/// the matching desktop command returns, as JSON. Every argument check the
/// desktop command makes is made here too: the arguments may have come over
/// the wire.
pub async fn run(
    call: AdminCall,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    reg: &Arc<CancellationRegistry>,
) -> Result<serde_json::Value, IpcError> {
    match call {
        AdminCall::Config => json(super::config(store)?),
        AdminCall::Configure(a) => json(super::configure(a, store)?),
        AdminCall::Load(a) => json(super::load(a.pull, store)?),
        AdminCall::GetAsset(a) => {
            check_name(&a.name)?;
            json(super::get_asset(a.kind, &a.name, store)?)
        }
        AdminCall::ListLayers => json(super::list_layers(store)?),
        AdminCall::ResolvePreview(a) => json(super::resolve_preview(&a.host_alias, store)?),
        AdminCall::ProposeLayers => json(super::propose::propose_layers(store)?),
        AdminCall::SetHostLayers(a) => json(super::set_host_layers(
            &a.host_alias,
            a.role.as_deref(),
            &a.contexts.iter().map(String::as_str).collect::<Vec<_>>(),
            store,
        )?),
        AdminCall::LayerTemplate(a) => {
            check_layer_name(&a.name)?;
            json(author::layer_template(&a.name, a.axis))
        }
        AdminCall::WriteLayer(a) => {
            check_layer_name(&a.layer.name)?;
            json(author::write_layer(&a.layer, store)?)
        }
        AdminCall::DeleteLayer(a) => {
            check_layer_name(&a.name)?;
            json(author::delete_layer(&a.name, store)?)
        }
        AdminCall::Inventory => json(super::inventory(store)?),
        AdminCall::PlanSync(a) => json(sync::plan_sync(a, store, ssh).await?),
        AdminCall::ApplySync(a) => json(sync::apply_sync(a, store, ssh, reg).await?),
        AdminCall::LastSync => json(sync::last_sync(store)?),
        AdminCall::ListSecrets => json(lock(store)?.list_secrets()?),
        AdminCall::SetSecret(a) => {
            check_secret_name(&a.name)?;
            lock(store)?.set_secret(&a.name, a.host_alias.as_deref(), &a.value)?;
            json(())
        }
        AdminCall::DeleteSecret(a) => {
            json(lock(store)?.delete_secret(&a.name, a.host_alias.as_deref())?)
        }
        AdminCall::CreateAsset(a) => {
            check_name(&a.name)?;
            if let Some(from) = a.duplicate_from.as_deref() {
                check_name(from)?;
            }
            json(author::create(a, store)?)
        }
        AdminCall::UpdateAsset(a) => {
            check_name(&a.asset.header.name)?;
            for r in &a.asset.resources {
                check_resource_path(&r.rel_path)?;
            }
            json(author::update(*a, store)?)
        }
        AdminCall::DeleteAsset(a) => {
            check_name(&a.name)?;
            json(author::delete_asset(a, store)?)
        }
        AdminCall::AddResourceBytes(a) => json(author::add_resource_bytes(a, store)?),
        AdminCall::RemoveResource(a) => {
            check_name(&a.name)?;
            check_resource_path(&a.rel_path)?;
            json(author::remove_resource(a, store)?)
        }
        AdminCall::LintAsset(a) => {
            check_name(&a.name)?;
            json(author::lint_asset(a, store)?)
        }
        AdminCall::LintAll => json(author::lint_everything(store)?),
        AdminCall::CommitPending(a) => json(author::commit_pending(a, store)?),
        AdminCall::Push => json(author::push(store)?),
        AdminCall::RepoStatus => json(author::repo_status(store)?),
        AdminCall::Template(a) => {
            check_name(&a.name)?;
            json(author::template(a.kind, &a.name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire shape the desktop sends and the tool reads: the action's
    /// snake_case name, and `args` only where the call takes some.
    #[test]
    fn a_call_is_an_action_and_its_args() {
        let v = serde_json::to_value(AdminCall::Push).unwrap();
        assert_eq!(v, serde_json::json!({ "action": "push" }));
        let v = serde_json::to_value(AdminCall::GetAsset(GetAssetArgs {
            kind: Kind::Skill,
            name: "s".into(),
        }))
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "action": "get_asset", "args": { "kind": "skill", "name": "s" } })
        );
        for call in [
            AdminCall::Config,
            AdminCall::Load(LoadArgs { pull: true }),
            AdminCall::SetSecret(SetSecretArgs {
                name: "TOKEN".into(),
                host_alias: None,
                value: "v".into(),
            }),
            AdminCall::AddResourceBytes(AddResourceBytesArgs {
                kind: Kind::Skill,
                name: "s".into(),
                rel_path: "resources/a.sh".into(),
                bytes: vec![0, 1, 2, 255],
            }),
        ] {
            let v = serde_json::to_value(&call).unwrap();
            assert_eq!(v["action"], call.action());
            let back: AdminCall = serde_json::from_value(v).unwrap();
            assert_eq!(back.action(), call.action());
        }
    }

    #[test]
    fn a_write_is_never_classed_as_a_read() {
        for call in [
            AdminCall::Configure(ConfigureArgs {
                repo_path: "/x".into(),
                remote_url: None,
            }),
            AdminCall::Load(LoadArgs { pull: false }),
            AdminCall::Push,
            AdminCall::CommitPending(CommitPendingArgs::default()),
            AdminCall::DeleteAsset(AssetRef {
                kind: Kind::Skill,
                name: "s".into(),
            }),
        ] {
            assert!(!call.is_read(), "{}", call.action());
        }
        assert!(AdminCall::RepoStatus.is_read());
    }
}
