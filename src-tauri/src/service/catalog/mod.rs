//! Asset catalog: harness-neutral skills / agents / hooks / MCP servers /
//! plugin refs kept in a git repo, rendered per harness, and compared with
//! what each host actually has installed.
//! Spec: docs/superpowers/specs/2026-09-14-asset-catalog-design.md

pub mod harness;
pub mod import;
pub mod inventory;
pub mod model;
pub mod repo;

// The catalog's `IpcError::code` values live with every other code in
// `ipc_error::codes`; re-exported here so the catalog modules can keep
// saying `catalog::E_CATALOG_GIT`.
pub use crate::ipc_error::codes::{
    E_ASSET_EXISTS, E_ASSET_NOT_FOUND, E_ASSET_UNSUPPORTED, E_CATALOG_GIT,
    E_CATALOG_NOT_CONFIGURED, E_CATALOG_PARSE,
};

/// The loaded catalog, process-wide. `None` until `load` succeeds. Both the
/// Tauri commands and the MCP tools read it; only `load` writes it.
pub static CATALOG: once_cell::sync::Lazy<std::sync::RwLock<Option<repo::Catalog>>> =
    once_cell::sync::Lazy::new(|| std::sync::RwLock::new(None));

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `CATALOG` is process-global, so tests that write it must serialise.
#[cfg(test)]
pub static CATALOG_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

use crate::events::CatalogSummary;
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

fn lock(store: &Mutex<Store>) -> Result<std::sync::MutexGuard<'_, Store>, IpcError> {
    store.lock().map_err(|_| IpcError::lock())
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

fn require_config(store: &Mutex<Store>) -> Result<CatalogConfigRow, IpcError> {
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

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HostState {
    pub host_alias: String,
    pub harness: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetSummary {
    pub kind: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub tags: Vec<String>,
    pub hosts: Vec<HostState>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetListing {
    pub head: String,
    pub loaded_at: i64,
    pub assets: Vec<AssetSummary>,
    pub unmanaged: Vec<AssetInventoryRow>,
    pub problems: Vec<Problem>,
}

fn host_states(rows: &[AssetInventoryRow], kind: Kind, name: &str) -> Vec<HostState> {
    rows.iter()
        .filter(|r| r.kind == kind.as_str() && r.name == name && r.state != "unmanaged")
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
                })
                .collect(),
            unmanaged: rows
                .iter()
                .filter(|r| r.state == "unmanaged")
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

#[derive(Debug, Clone, Deserialize)]
pub struct ImportArgs {
    pub host_alias: String,
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
}
