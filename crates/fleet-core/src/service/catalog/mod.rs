//! Asset catalog: harness-neutral skills / agents / hooks / MCP servers /
//! plugin refs kept in a git repo, rendered per harness, and compared with
//! what each host actually has installed.
//! Spec: docs/superpowers/specs/2026-09-14-asset-catalog-design.md

pub mod author;
pub mod author_session;
pub mod harness;
pub mod import;
pub mod inventory;
pub mod layer;
pub mod model;
pub mod propose;
pub mod repo;
pub mod resolve;
pub mod sync;

// The catalog's `IpcError::code` values live with every other code in
// `ipc_error::codes`; re-exported here so the catalog modules can keep
// saying `catalog::E_CATALOG_GIT`.
pub use crate::ipc_error::codes::{
    E_ASSET_EXISTS, E_ASSET_NOT_FOUND, E_ASSET_UNSUPPORTED, E_CATALOG_GIT,
    E_CATALOG_NOT_CONFIGURED, E_CATALOG_PARSE, E_LINT,
};

/// The loaded catalog, process-wide. `None` until `load` succeeds. Both the
/// Tauri commands and the MCP tools read it; only `load` writes it.
pub static CATALOG: std::sync::LazyLock<std::sync::RwLock<Option<repo::Catalog>>> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(None));

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `CATALOG` and `HOME` are process-global, so tests that write either must
/// serialise on this lock.
#[cfg(test)]
pub static CATALOG_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

use crate::events::CatalogSummary;
use crate::ipc_error::lock;
use crate::ipc_error::{codes, IpcError};
use crate::store::{AssetInventoryRow, CatalogConfigRow, Store};
use harness::RenderPlan;
use model::{Asset, Kind, Problem};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigureArgs {
    pub repo_path: String,
    pub remote_url: Option<String>,
}

fn expand_home(p: &str) -> String {
    match (p.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
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

/// (Optionally pull, then) parse the repo into `CATALOG` and record HEAD.
pub fn load(pull: bool, store: &Mutex<Store>) -> Result<CatalogSummary, IpcError> {
    let cfg = require_config(store)?;
    let root = std::path::PathBuf::from(&cfg.repo_path);
    repo::ensure_repo(&root, cfg.remote_url.as_deref())?;
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
    *CATALOG
        .write()
        .map_err(|_| IpcError::new(codes::E_LOCK, "catalog lock poisoned"))? = Some(cat);
    {
        let s = lock(store)?;
        s.set_catalog_head(&summary.head, summary.loaded_at)?;
        s.bus_catalog_loaded(&summary);
    }
    Ok(summary)
}

fn with_catalog<T>(f: impl FnOnce(&repo::Catalog) -> Result<T, IpcError>) -> Result<T, IpcError> {
    let guard = CATALOG
        .read()
        .map_err(|_| IpcError::new(codes::E_LOCK, "catalog lock poisoned"))?;
    match guard.as_ref() {
        Some(c) => f(c),
        None => Err(IpcError::new(
            E_CATALOG_NOT_CONFIGURED,
            "catalog not loaded; call catalog_load",
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostState {
    pub host_alias: String,
    pub harness: String,
    pub state: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetListing {
    pub head: String,
    pub loaded_at: i64,
    pub assets: Vec<AssetSummary>,
    pub unmanaged: Vec<AssetInventoryRow>,
    pub problems: Vec<Problem>,
}

/// Which hosts hold this catalog asset, and in what state. `unmanaged` and
/// `orphan` rows are excluded by name: neither describes a catalog asset
/// (they are what `AssetListing::unmanaged` lists instead), and an orphan
/// shares a `(kind, name)` with nothing in the catalog anyway.
fn host_states(rows: &[AssetInventoryRow], kind: Kind, name: &str) -> Vec<HostState> {
    rows.iter()
        .filter(|r| {
            r.kind == kind.as_str()
                && r.name == name
                && r.state != "unmanaged"
                && r.state != "orphan"
        })
        .map(|r| HostState {
            host_alias: r.host_alias.clone(),
            harness: r.harness.clone(),
            state: r.state.clone(),
        })
        .collect()
}

pub fn inventory(store: &Mutex<Store>) -> Result<Vec<AssetInventoryRow>, IpcError> {
    Ok(lock(store)?.list_inventory()?)
}

pub fn list_assets(store: &Mutex<Store>) -> Result<AssetListing, IpcError> {
    require_config(store)?;
    let rows = inventory(store)?;
    with_catalog(|cat| {
        Ok(AssetListing {
            head: cat.head.clone(),
            loaded_at: cat.loaded_at,
            assets: cat
                .assets
                .iter()
                .map(|a| AssetSummary {
                    kind: a.kind().as_str().to_string(),
                    name: a.header.name.clone(),
                    version: a.header.version.clone(),
                    description: a.header.description.clone(),
                    tags: a.header.tags.clone(),
                    hosts: host_states(&rows, a.kind(), &a.header.name),
                    install_as: a.header.install_as.clone(),
                })
                .collect(),
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
            problems: cat.problems.clone(),
        })
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    pub harness: String,
    pub plan: Option<RenderPlan>,
    pub unsupported: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetDetail {
    pub asset: Asset,
    pub previews: Vec<Preview>,
    pub hosts: Vec<HostState>,
}

#[derive(Debug, Clone, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "ImportAssetsParams")]
pub struct ImportArgs {
    /// Only `local` (the fleet controller).
    pub host_alias: String,
    /// Report, write nothing.
    #[serde(default)]
    pub dry_run: bool,
}

/// Import from a host's Claude config. v1 supports the controller (`local`)
/// only; other hosts return E_ASSET_UNSUPPORTED.
pub fn import_host(
    args: ImportArgs,
    store: &Mutex<Store>,
    fleet_token: Option<&str>,
) -> Result<import::ImportReport, IpcError> {
    let cfg = require_config(store)?;
    if args.host_alias != "local" {
        return Err(IpcError::new(
            E_ASSET_UNSUPPORTED,
            "importing from remote hosts is not supported yet; use local",
        ));
    }
    crate::service::hub::ensure_local_allowed(&args.host_alias)?;
    let src = import::ImportSources::for_local()?;
    import::import_claude(
        &src,
        std::path::Path::new(&cfg.repo_path),
        &args.host_alias,
        fleet_token,
        args.dry_run,
    )
}

pub fn get_asset(kind: Kind, name: &str, store: &Mutex<Store>) -> Result<AssetDetail, IpcError> {
    let rows = inventory(store)?;
    with_catalog(|cat| {
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
            hosts: host_states(&rows, kind, name),
        })
    })
}

/// The catalog's layer definitions plus every host's stored assignment.
#[derive(Debug, Clone, Serialize)]
pub struct LayerListing {
    pub layers: Vec<layer::Layer>,
    pub hosts: Vec<crate::store::HostLayerRow>,
}

pub fn list_layers(store: &Mutex<Store>) -> Result<LayerListing, IpcError> {
    // Only active rows are part of an assignment — the same rule
    // `get_host_layers` (and therefore resolution) applies. The store call
    // stays "all rows" because callers such as `delete_host`'s checks need
    // exactly that.
    let hosts: Vec<_> = lock(store)?
        .list_all_host_layers()?
        .into_iter()
        .filter(|r| r.active)
        .collect();
    with_catalog(|cat| {
        Ok(LayerListing {
            layers: cat.layers.iter().cloned().collect(),
            hosts,
        })
    })
}

/// `host_alias` must be a registered host. Without this check, a typo'd
/// alias either trips the `host_layers` foreign key (`set_host_layers`) or —
/// worse — silently resolves to the WHOLE catalog, since `resolve_for_host`
/// treats "no assignment rows" as "no layering" for backward compatibility
/// and cannot tell a nonexistent host from an unassigned one.
fn require_host_exists(store: &Mutex<Store>, host_alias: &str) -> Result<(), IpcError> {
    match lock(store)?.get_host_row(host_alias)? {
        Some(_) => Ok(()),
        None => Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("host {host_alias} not found"),
        )),
    }
}

/// Compute the effective asset set for `host_alias`, with provenance.
/// Nothing is written.
pub fn resolve_preview(
    host_alias: &str,
    store: &Mutex<Store>,
) -> Result<resolve::Resolution, IpcError> {
    crate::validate::host_alias(host_alias)?;
    require_host_exists(store, host_alias)?;
    with_catalog(|cat| sync::layers::resolve_for_host(store, cat, host_alias))
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

/// Replace a host's layer assignment wholesale: one optional role plus
/// contexts in application order. Edits fleet state only; catalog files are
/// never written. Returns the host's new assignment.
pub fn set_host_layers(
    host_alias: &str,
    role: Option<&str>,
    contexts: &[&str],
    store: &Mutex<Store>,
) -> Result<Vec<crate::store::HostLayerRow>, IpcError> {
    crate::validate::host_alias(host_alias)?;
    require_host_exists(store, host_alias)?;
    check_no_name_collision(role, contexts)?;
    with_catalog(|cat| {
        if let Some(r) = role {
            check_layer_axis(cat, r, layer::Axis::Role)?;
        }
        for c in contexts {
            check_layer_axis(cat, c, layer::Axis::Context)?;
        }
        Ok(())
    })?;
    let s = lock(store)?;
    s.set_host_layers(host_alias, role, contexts)?;
    Ok(s.get_host_layers(host_alias)?)
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
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
                state: "drifted".into()
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

    /// `orphan` rows — the host still holds something a past sync wrote but
    /// the catalog has dropped — belong in the same "not in the catalog"
    /// list the UI shows, alongside `unmanaged`.
    #[test]
    fn list_assets_lists_orphan_rows_as_unmanaged() {
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
                state: "in_sync".into()
            }],
            "an orphan is not a host state of a catalog asset"
        );
    }

    /// A typo'd `host_alias` must fail clearly, not as a raw SQLite
    /// foreign-key violation: `host_layers.host_alias` references
    /// `hosts(alias)` with `PRAGMA foreign_keys = ON`.
    #[test]
    fn set_host_layers_rejects_an_unknown_host() {
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let store = configured_store_with_layers("ll-active");
        set_host_layers("local", Some("core"), &[], &store).unwrap();
        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute(
                "INSERT INTO host_layers (host_alias, layer_name, axis, position, active) \
                 VALUES ('local', 'extra', 'context', 0, 0)",
                [],
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
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
}
