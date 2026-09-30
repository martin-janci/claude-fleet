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
use super::model::Kind;
use super::sync::{self, ApplyArgs, PlanArgs};
use super::validate::{check_layer_name, check_name, check_secret_name};
use super::{ConfigureArgs, ImportArgs};
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

/// Declares [`AdminCall`] with each variant's wire name written once: the
/// literal is both the serde tag (`#[serde(rename = ...)]`) and what
/// [`AdminCall::action`] answers, so the two cannot drift apart.
macro_rules! admin_calls {
    (@pat $variant:ident) => { AdminCall::$variant };
    (@pat $variant:ident $args:ty) => { AdminCall::$variant(_) };
    ($( $(#[$meta:meta])* $wire:literal => $variant:ident $(($args:ty))? ),* $(,)?) => {
        /// One catalog operation. On the wire: `{"action": "<snake_case>",
        /// "args": {…}}`, `args` absent for the operations that take none.
        #[derive(Clone, Serialize, Deserialize)]
        #[serde(tag = "action", content = "args")]
        pub enum AdminCall {
            $( $(#[$meta])* #[serde(rename = $wire)] $variant $(($args))?, )*
        }

        impl AdminCall {
            /// Every action's wire name, in declaration order.
            pub const ACTIONS: &'static [&'static str] = &[$($wire),*];

            /// The wire name of this call's action, for audit lines and errors.
            pub fn action(&self) -> &'static str {
                match self {
                    $( admin_calls!(@pat $variant $($args)?) => $wire, )*
                }
            }
        }
    };
}

admin_calls! {
    "config" => Config,
    "configure" => Configure(ConfigureArgs),
    "load" => Load(LoadArgs),
    "get_asset" => GetAsset(GetAssetArgs),
    "list_layers" => ListLayers,
    "resolve_preview" => ResolvePreview(ResolvePreviewArgs),
    "propose_layers" => ProposeLayers,
    "set_host_layers" => SetHostLayers(SetHostLayersArgs),
    "layer_template" => LayerTemplate(LayerTemplateArgs),
    "write_layer" => WriteLayer(WriteLayerArgs),
    "delete_layer" => DeleteLayer(LayerRef),
    "inventory" => Inventory,
    "import_host" => ImportHost(ImportArgs),
    "plan_sync" => PlanSync(PlanArgs),
    "apply_sync" => ApplySync(ApplyArgs),
    "last_sync" => LastSync,
    "list_secrets" => ListSecrets,
    "set_secret" => SetSecret(SetSecretArgs),
    "delete_secret" => DeleteSecret(DeleteSecretArgs),
    "create_asset" => CreateAsset(CreateArgs),
    /// Boxed: a whole asset (body and resources) is by far the largest
    /// variant, and every other call would pay for its size.
    "update_asset" => UpdateAsset(Box<UpdateArgs>),
    "delete_asset" => DeleteAsset(AssetRef),
    /// `catalog_add_resource` with the file already read on the caller's
    /// side ([`author::read_resource_file`]): the path names a file on the
    /// desktop, which the hub cannot open.
    "add_resource_bytes" => AddResourceBytes(AddResourceBytesArgs),
    "remove_resource" => RemoveResource(RemoveResourceArgs),
    "lint_asset" => LintAsset(AssetRef),
    "lint_all" => LintAll,
    "commit_pending" => CommitPending(CommitPendingArgs),
    "push" => Push,
    "repo_status" => RepoStatus,
    "template" => Template(AssetRef),
}

impl AdminCall {
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
        AdminCall::WriteLayer(a) => json(author::write_layer(&a.layer, store)?),
        AdminCall::DeleteLayer(a) => json(author::delete_layer(&a.name, store)?),
        AdminCall::Inventory => json(super::inventory(store)?),
        AdminCall::ImportHost(a) => {
            let token = lock(store)?.get_setting(crate::mcp::SETTING_TOKEN)?;
            json(super::import_host(a, store, ssh, token.as_deref()).await?)
        }
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
        // The authoring calls check their own names and paths, before they
        // read the config or touch the checkout.
        AdminCall::CreateAsset(a) => json(author::create(a, store)?),
        AdminCall::UpdateAsset(a) => json(author::update(*a, store)?),
        AdminCall::DeleteAsset(a) => json(author::delete_asset(a, store)?),
        AdminCall::AddResourceBytes(a) => json(author::add_resource_bytes(a, store)?),
        AdminCall::RemoveResource(a) => json(author::remove_resource(a, store)?),
        AdminCall::LintAsset(a) => json(author::lint_asset(a, store)?),
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
        let calls = every_call();
        assert_eq!(calls.len(), AdminCall::ACTIONS.len());
        let mut seen = std::collections::BTreeSet::new();
        for (call, wire) in calls.iter().zip(AdminCall::ACTIONS) {
            assert_eq!(call.action(), *wire);
            assert!(seen.insert(*wire), "{wire} twice");
            let v = serde_json::to_value(call).unwrap();
            assert_eq!(v["action"], *wire);
            let back: AdminCall = serde_json::from_value(v.clone()).unwrap();
            assert_eq!(back.action(), call.action());
            assert_eq!(serde_json::to_value(&back).unwrap(), v, "{wire}");
        }
        // The names are the snake_case of the variant, as the desktop and
        // `docs/hub.md` spell them.
        for wire in AdminCall::ACTIONS {
            assert!(
                wire.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{wire}"
            );
        }
    }

    /// One of every variant, in declaration order.
    fn every_call() -> Vec<AdminCall> {
        let skill = |name: &str| AssetRef {
            kind: Kind::Skill,
            name: name.into(),
        };
        vec![
            AdminCall::Config,
            AdminCall::Configure(ConfigureArgs {
                repo_path: "/x".into(),
                remote_url: Some("git@example.com:c.git".into()),
            }),
            AdminCall::Load(LoadArgs { pull: true }),
            AdminCall::GetAsset(GetAssetArgs {
                kind: Kind::Agent,
                name: "a".into(),
            }),
            AdminCall::ListLayers,
            AdminCall::ResolvePreview(ResolvePreviewArgs {
                host_alias: "h".into(),
            }),
            AdminCall::ProposeLayers,
            AdminCall::SetHostLayers(SetHostLayersArgs {
                host_alias: "h".into(),
                role: Some("r".into()),
                contexts: vec!["c".into()],
            }),
            AdminCall::LayerTemplate(LayerTemplateArgs {
                name: "l".into(),
                axis: Axis::Context,
            }),
            AdminCall::WriteLayer(WriteLayerArgs {
                layer: author::layer_template("l", Axis::Role),
            }),
            AdminCall::DeleteLayer(LayerRef { name: "l".into() }),
            AdminCall::Inventory,
            AdminCall::ImportHost(ImportArgs {
                host_alias: "oci".into(),
                dry_run: true,
                only: vec![],
            }),
            AdminCall::PlanSync(PlanArgs {
                host_alias: Some("h".into()),
                kind: Some(Kind::Hook),
                name: None,
            }),
            AdminCall::ApplySync(ApplyArgs {
                plan_id: "p".into(),
                force_partial: true,
                call_id: Some(7),
            }),
            AdminCall::LastSync,
            AdminCall::ListSecrets,
            AdminCall::SetSecret(SetSecretArgs {
                name: "TOKEN".into(),
                host_alias: None,
                value: "v".into(),
            }),
            AdminCall::DeleteSecret(DeleteSecretArgs {
                name: "TOKEN".into(),
                host_alias: Some("h".into()),
            }),
            AdminCall::CreateAsset(CreateArgs {
                kind: Kind::Skill,
                name: "s".into(),
                duplicate_from: Some("t".into()),
            }),
            AdminCall::UpdateAsset(Box::new(UpdateArgs {
                asset: author::template(Kind::Skill, "s"),
            })),
            AdminCall::DeleteAsset(skill("s")),
            AdminCall::AddResourceBytes(AddResourceBytesArgs {
                kind: Kind::Skill,
                name: "s".into(),
                rel_path: "resources/a.sh".into(),
                bytes: vec![0, 1, 2, 255],
            }),
            AdminCall::RemoveResource(RemoveResourceArgs {
                kind: Kind::Skill,
                name: "s".into(),
                rel_path: "resources/a.sh".into(),
            }),
            AdminCall::LintAsset(skill("s")),
            AdminCall::LintAll,
            AdminCall::CommitPending(CommitPendingArgs {
                message: Some("m".into()),
            }),
            AdminCall::Push,
            AdminCall::RepoStatus,
            AdminCall::Template(skill("s")),
        ]
    }

    /// Every call that names an asset, a layer, a resource or a secret
    /// refuses a hostile one with `E_INVALID` before it reads the catalog
    /// config (unset here, so any later step would answer
    /// `E_CATALOG_NOT_CONFIGURED`) or writes the store. `run` keeps a
    /// pre-check only where the callee has none; this pins that dropping
    /// the others lost nothing.
    #[tokio::test]
    async fn hostile_names_and_paths_are_refused_before_any_effect() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let ssh = Arc::new(SshClient::new());
        let reg = CancellationRegistry::new();
        let asset_ref = |name: &str| AssetRef {
            kind: Kind::Skill,
            name: name.into(),
        };
        let with_resource = |rel_path: &str| {
            let mut asset = author::template(Kind::Skill, "ok");
            asset.resources.push(super::super::model::Resource {
                rel_path: rel_path.into(),
                bytes: vec![1],
            });
            AdminCall::UpdateAsset(Box::new(UpdateArgs { asset }))
        };
        let mut calls: Vec<(String, AdminCall)> = Vec::new();
        for bad in ["../x", "a/b", "", ".hidden"] {
            let named = |a: AdminCall| (format!("{} {bad:?}", a.action()), a);
            calls.push(named(AdminCall::GetAsset(GetAssetArgs {
                kind: Kind::Skill,
                name: bad.into(),
            })));
            calls.push(named(AdminCall::DeleteAsset(asset_ref(bad))));
            calls.push(named(AdminCall::LintAsset(asset_ref(bad))));
            calls.push(named(AdminCall::Template(asset_ref(bad))));
            calls.push(named(AdminCall::CreateAsset(CreateArgs {
                kind: Kind::Skill,
                name: bad.into(),
                duplicate_from: None,
            })));
            calls.push(named(AdminCall::CreateAsset(CreateArgs {
                kind: Kind::Skill,
                name: "ok".into(),
                duplicate_from: Some(bad.into()),
            })));
            let mut asset = author::template(Kind::Skill, "ok");
            asset.header.name = bad.into();
            calls.push(named(AdminCall::UpdateAsset(Box::new(UpdateArgs {
                asset,
            }))));
            calls.push(named(AdminCall::RemoveResource(RemoveResourceArgs {
                kind: Kind::Skill,
                name: bad.into(),
                rel_path: "resources/a.sh".into(),
            })));
            calls.push(named(AdminCall::AddResourceBytes(AddResourceBytesArgs {
                kind: Kind::Skill,
                name: bad.into(),
                rel_path: "resources/a.sh".into(),
                bytes: vec![1],
            })));
            let mut layer = author::layer_template("ok", Axis::Role);
            layer.name = bad.into();
            calls.push(named(AdminCall::WriteLayer(WriteLayerArgs { layer })));
            calls.push(named(AdminCall::DeleteLayer(LayerRef { name: bad.into() })));
            calls.push(named(AdminCall::LayerTemplate(LayerTemplateArgs {
                name: bad.into(),
                axis: Axis::Role,
            })));
        }
        for bad in ["../x", "/abs", "resources/../../x"] {
            let named = |a: AdminCall| (format!("{} {bad:?}", a.action()), a);
            calls.push(named(AdminCall::RemoveResource(RemoveResourceArgs {
                kind: Kind::Skill,
                name: "ok".into(),
                rel_path: bad.into(),
            })));
            calls.push(named(AdminCall::AddResourceBytes(AddResourceBytesArgs {
                kind: Kind::Skill,
                name: "ok".into(),
                rel_path: bad.into(),
                bytes: vec![1],
            })));
            calls.push(named(with_resource(bad)));
        }
        calls.push((
            "set_secret \"bad-name\"".into(),
            AdminCall::SetSecret(SetSecretArgs {
                name: "bad-name".into(),
                host_alias: None,
                value: "v".into(),
            }),
        ));
        for (label, call) in calls {
            let err = match run(call, &store, &ssh, &reg).await {
                Ok(v) => panic!("{label}: accepted, answered {v}"),
                Err(e) => e,
            };
            assert_eq!(err.code, codes::E_INVALID, "{label}: {}", err.message);
        }
        assert!(store.lock().unwrap().list_secrets().unwrap().is_empty());
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
