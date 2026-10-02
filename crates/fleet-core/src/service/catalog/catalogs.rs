//! The set of catalogs (Assets M3): add, list and remove them, and which
//! hosts with no org admit an org catalog (`host_catalogs`). Shared by the
//! hub's `catalog_admin` and `fleet-hub catalog …`. Removing is config only:
//! the checkout on disk is never touched (spec, Hub CLI and MCP).

use super::{registry, repo};
use crate::ipc_error::{codes, lock, IpcError};
use crate::store::{CatalogConfigRow, CatalogRemoval, CatalogRow, Store};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

pub const PERSONAL: &str = "personal";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddCatalogArgs {
    pub name: String,
    pub repo_path: String,
    #[serde(default)]
    pub remote_url: Option<String>,
    /// The owning org by name; required for every catalog but `personal`.
    #[serde(default)]
    pub org: Option<String>,
}

/// One catalog as `list_catalogs` / `catalog list` show it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogStatus {
    pub id: i64,
    pub name: String,
    pub org_id: Option<i64>,
    #[serde(default)]
    pub org: Option<String>,
    pub repo_path: String,
    pub remote_url: Option<String>,
    pub head_commit: Option<String>,
    pub last_loaded_at: Option<i64>,
    /// `loaded` | `problem` | `not_loaded`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    pub asset_count: usize,
    /// Hosts with no org that admit this catalog.
    #[serde(default)]
    pub admitted: Vec<String>,
    /// Live paired clients granted this catalog (R19).
    #[serde(default)]
    pub granted: Vec<String>,
}

pub fn catalog_named(name: &str, store: &Mutex<Store>) -> Result<CatalogRow, IpcError> {
    lock(store)?.get_catalog_by_name(name)?.ok_or_else(|| {
        IpcError::new(
            codes::E_NOTFOUND,
            format!("no catalog named {name}; list them with list_catalogs"),
        )
    })
}

pub fn config_row(row: &CatalogRow) -> CatalogConfigRow {
    CatalogConfigRow {
        repo_path: row.repo_path.clone(),
        remote_url: row.remote_url.clone(),
        head_commit: row.head_commit.clone(),
        last_loaded_at: row.last_loaded_at,
    }
}

/// Every catalog with its load state, personal first. Store rows are read
/// under one guard, released, then the registry is read (store → registry
/// is never allowed).
pub fn list_catalogs(store: &Mutex<Store>) -> Result<Vec<CatalogStatus>, IpcError> {
    let mut rows = Vec::new();
    {
        let s = lock(store)?;
        let orgs = s.list_orgs()?;
        for r in s.list_catalogs()? {
            let org = r
                .org_id
                .and_then(|id| orgs.iter().find(|o| o.id == id).map(|o| o.name.clone()));
            let admitted = s.catalog_admissions(r.id)?;
            let granted = s.catalog_grantees(r.id)?;
            rows.push((r, org, admitted, granted));
        }
    }
    registry::with_catalogs(|m| {
        Ok(rows
            .into_iter()
            .map(|(r, org, admitted, granted)| {
                let (state, problem, asset_count) = match registry::entry_for(m, &r) {
                    None => ("not_loaded", None, 0),
                    Some(c) => match &c.load_error {
                        Some(e) => ("problem", Some(e.clone()), 0),
                        None => ("loaded", None, c.assets.len()),
                    },
                };
                CatalogStatus {
                    id: r.id,
                    name: r.name,
                    org_id: r.org_id,
                    org,
                    repo_path: r.repo_path,
                    remote_url: r.remote_url,
                    head_commit: r.head_commit,
                    last_loaded_at: r.last_loaded_at,
                    state: state.to_string(),
                    problem,
                    asset_count,
                    admitted,
                    granted,
                }
            })
            .collect())
    })
}

/// The org named by `catalog add --org` (case-insensitively) as its id;
/// `None` when none is named. Who may own which catalog is
/// `Store::check_catalog_owner`'s.
fn org_id_named(org: Option<&str>, s: &Store) -> Result<Option<i64>, IpcError> {
    let Some(org) = org else { return Ok(None) };
    s.list_orgs()?
        .into_iter()
        .find(|o| o.name.eq_ignore_ascii_case(org))
        .map(|o| Some(o.id))
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no org named {org}")))
}

/// Add a catalog (or re-point one, R12), then load it. Every precondition
/// `upsert_catalog` would refuse on — the org exists, and
/// `Store::check_catalog_owner` — is checked first (PF18), so a refused
/// call has no side effect on disk; then the checkout must be there or clonable from
/// `remote_url` before anything is recorded. An org catalog that then fails
/// to parse is kept as a problem entry (R5) and reported as `state:
/// problem`; `personal` failing is an error, as `catalog set` has always
/// been.
pub fn add_catalog(args: AddCatalogArgs, store: &Mutex<Store>) -> Result<CatalogStatus, IpcError> {
    super::validate::check_name(&args.name)?;
    let path = super::expand_home(args.repo_path.trim());
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
    let org = args.org.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let org_id = {
        let s = lock(store)?;
        let org_id = org_id_named(org, &s)?;
        s.check_catalog_owner(&args.name, org_id)?;
        org_id
    };
    repo::ensure_repo(std::path::Path::new(&path), remote)?;
    let row = lock(store)?.upsert_catalog(&args.name, &path, remote, org_id)?;
    match super::load_catalog(row.id, false, store) {
        Ok(_) => {}
        Err(e) if row.org_id.is_none() || !super::is_load_failure(&e) => return Err(e),
        Err(e) => registry::install(super::problem_entry(&row, &e))?,
    }
    list_catalogs(store)?
        .into_iter()
        .find(|c| c.id == row.id)
        .ok_or_else(|| IpcError::new(codes::E_INTERNAL, format!("catalog {} vanished", row.name)))
}

/// Remove an org catalog (R13): config only, the checkout stays.
pub fn remove_catalog(name: &str, store: &Mutex<Store>) -> Result<CatalogRemoval, IpcError> {
    let removal = lock(store)?.remove_catalog(name)?;
    registry::remove(removal.id)?;
    Ok(removal)
}

/// The preamble `admit` and `unadmit` share (PF14): a valid alias of a
/// registered host, and the catalog it names.
fn host_and_catalog(s: &Store, host_alias: &str, catalog: &str) -> Result<CatalogRow, IpcError> {
    crate::validate::host_alias(host_alias)?;
    super::require_host(s, host_alias)?;
    s.get_catalog_by_name(catalog)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no catalog named {catalog}")))
}

fn admitted_names(s: &Store, host_alias: &str) -> Result<Vec<String>, IpcError> {
    let ids = s.host_admissions(host_alias)?;
    Ok(s.list_catalogs()?
        .into_iter()
        .filter(|c| ids.contains(&c.id))
        .map(|c| c.name)
        .collect())
}

/// Admit an org catalog on a host with no org (spec, Which catalogs a host
/// accepts; R4). Answers the host's admitted catalogs by name.
pub fn admit(
    host_alias: &str,
    catalog: &str,
    store: &Mutex<Store>,
) -> Result<Vec<String>, IpcError> {
    let s = lock(store)?;
    let row = host_and_catalog(&s, host_alias, catalog)?;
    if row.org_id.is_none() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "every host takes the personal catalog already (all of it with no org, its shared \
             assets with one); only an org catalog is admitted",
        ));
    }
    if let Some(org_id) = s.host_org(host_alias)? {
        let org = s
            .get_org(org_id)?
            .map_or_else(|| org_id.to_string(), |o| o.name);
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "{host_alias} is in org {org}: it takes its own org's catalog and never another's; \
                 admission is for hosts with no org"
            ),
        ));
    }
    s.admit_host_catalog(host_alias, row.id)?;
    admitted_names(&s, host_alias)
}

/// Take an admission back. Works on any host (a leftover from before an org
/// change, R4).
pub fn unadmit(
    host_alias: &str,
    catalog: &str,
    store: &Mutex<Store>,
) -> Result<Vec<String>, IpcError> {
    unadmit_reporting(host_alias, catalog, store).map(|(_, names)| names)
}

/// [`unadmit`], also saying whether the host admitted that catalog at all
/// (`fleet-hub catalog unadmit` says so when it did not).
pub fn unadmit_reporting(
    host_alias: &str,
    catalog: &str,
    store: &Mutex<Store>,
) -> Result<(bool, Vec<String>), IpcError> {
    let s = lock(store)?;
    let row = host_and_catalog(&s, host_alias, catalog)?;
    let was_admitted = s.unadmit_host_catalog(host_alias, row.id)?;
    Ok((was_admitted, admitted_names(&s, host_alias)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc_error::codes;
    use crate::service::catalog::lock_registry_for_test;

    /// A committed checkout with `catalog.yaml` at `schema` and skills `s…`.
    fn git_repo(tag: &str, schema: u32, skills: &[&str]) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("fleet-catalogs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("catalog.yaml"),
            format!("schema_version: {schema}\n"),
        )
        .unwrap();
        for s in skills {
            std::fs::create_dir_all(root.join(format!("skills/{s}"))).unwrap();
            std::fs::write(
                root.join(format!("skills/{s}/asset.yaml")),
                format!("kind: skill\nname: {s}\ndescription: d\n"),
            )
            .unwrap();
            std::fs::write(root.join(format!("skills/{s}/body.md")), "b\n").unwrap();
        }
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.email", "t@t"],
            &["config", "user.name", "t"],
            &["add", "."],
            &["commit", "-q", "-m", "init"],
        ] {
            let o = crate::proc::std_command("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        }
        root
    }

    fn store_with_org() -> Mutex<Store> {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        s.add_org("acme", None, false).unwrap();
        Mutex::new(s)
    }

    fn add(name: &str, path: &std::path::Path, org: Option<&str>) -> AddCatalogArgs {
        AddCatalogArgs {
            name: name.into(),
            repo_path: path.to_string_lossy().into(),
            remote_url: None,
            org: org.map(String::from),
        }
    }

    #[test]
    fn add_list_admit_and_remove_an_org_catalog() {
        let _g = lock_registry_for_test();
        let store = store_with_org();
        add_catalog(add("personal", &git_repo("p", 1, &["a"]), None), &store).unwrap();
        let acme = add_catalog(
            add("acme", &git_repo("acme", 1, &["c", "d"]), Some("ACME")),
            &store,
        )
        .unwrap();
        assert_eq!(
            (acme.state.as_str(), acme.asset_count, acme.org.as_deref()),
            ("loaded", 2, Some("acme")),
            "the org is matched case-insensitively"
        );

        assert_eq!(
            admit("h", "acme", &store).unwrap(),
            vec!["acme".to_string()]
        );
        store
            .lock()
            .unwrap()
            .insert_client_token("ops", "aa11", "full")
            .unwrap();
        store
            .lock()
            .unwrap()
            .set_client_catalog_grant("ops", acme.id, true)
            .unwrap();
        let listed = list_catalogs(&store).unwrap();
        assert_eq!(
            listed.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["personal", "acme"]
        );
        assert_eq!(listed[1].admitted, vec!["h".to_string()]);
        assert_eq!(listed[1].granted, vec!["ops".to_string()]);
        assert!(listed[0].admitted.is_empty());

        assert!(unadmit("h", "acme", &store).unwrap().is_empty());
        let gone = remove_catalog("acme", &store).unwrap();
        assert_eq!((gone.id, gone.grants), (acme.id, 1));
        assert!(
            registry::get(acme.id).unwrap().is_none(),
            "evicted from the registry"
        );
        assert_eq!(list_catalogs(&store).unwrap().len(), 1);
        registry::clear().unwrap();
    }

    #[test]
    fn a_checkout_that_does_not_parse_is_added_as_a_problem() {
        let _g = lock_registry_for_test();
        let store = store_with_org();
        let st = add_catalog(
            add("acme", &git_repo("bad-schema", 99, &[]), Some("acme")),
            &store,
        )
        .unwrap();
        assert_eq!(st.state, "problem");
        assert!(
            st.problem.as_deref().unwrap().contains("schema_version 99"),
            "{:?}",
            st.problem
        );
        registry::clear().unwrap();
    }

    #[test]
    fn add_admit_and_remove_refuse_what_cannot_be() {
        let _g = lock_registry_for_test();
        let store = store_with_org();
        let repo = git_repo("refuse", 1, &[]);
        let code = |r: Result<CatalogStatus, IpcError>| r.unwrap_err().code;
        assert_eq!(
            code(add_catalog(add("../x", &repo, Some("acme")), &store)),
            codes::E_INVALID
        );
        assert_eq!(
            code(add_catalog(add("acme", &repo, None), &store)),
            codes::E_INVALID
        );
        assert_eq!(
            code(add_catalog(add("acme", &repo, Some("nope")), &store)),
            codes::E_NOTFOUND
        );
        assert_eq!(
            code(add_catalog(add("personal", &repo, Some("acme")), &store)),
            codes::E_INVALID
        );
        assert_eq!(
            code(add_catalog(
                add(
                    "acme",
                    std::path::Path::new("/nonexistent/fleet-m3"),
                    Some("acme")
                ),
                &store
            )),
            codes::E_CATALOG_GIT,
            "a checkout that is not there is refused before anything is recorded"
        );
        assert!(store
            .lock()
            .unwrap()
            .get_catalog_by_name("acme")
            .unwrap()
            .is_none());

        add_catalog(add("acme", &repo, Some("acme")), &store).unwrap();
        store
            .lock()
            .unwrap()
            .set_catalog_config(&git_repo("refuse-p", 1, &[]).to_string_lossy(), None)
            .unwrap();
        assert_eq!(
            admit("h", "personal", &store).unwrap_err().code,
            codes::E_INVALID
        );
        assert_eq!(
            admit("nohost", "acme", &store).unwrap_err().code,
            codes::E_NOTFOUND
        );
        assert_eq!(
            admit("h", "nocat", &store).unwrap_err().code,
            codes::E_NOTFOUND
        );
        let org = store.lock().unwrap().list_orgs().unwrap()[0].id;
        store.lock().unwrap().set_host_org("h", Some(org)).unwrap();
        let err = admit("h", "acme", &store).unwrap_err();
        assert_eq!(err.code, codes::E_INVALID, "R4: no org only");
        assert!(
            err.message.contains("is in org acme:"),
            "the refusal names the org: {}",
            err.message
        );
        assert_eq!(
            remove_catalog("personal", &store).unwrap_err().code,
            codes::E_INVALID
        );
        registry::clear().unwrap();
    }

    /// PF18: a re-point that would move a catalog to another org is refused
    /// BEFORE `ensure_repo` runs — a refused call must not clone anything.
    #[test]
    fn a_refused_org_move_leaves_no_checkout() {
        let _g = lock_registry_for_test();
        let store = store_with_org();
        store.lock().unwrap().add_org("other", None, false).unwrap();
        let origin = git_repo("move-origin", 1, &["a"]);
        add_catalog(add("acme", &origin, Some("acme")), &store).unwrap();

        let target =
            std::env::temp_dir().join(format!("fleet-catalogs-move-target-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&target);
        let err = add_catalog(
            AddCatalogArgs {
                name: "acme".into(),
                repo_path: target.to_string_lossy().into(),
                remote_url: Some(origin.to_string_lossy().into()),
                org: Some("other".into()),
            },
            &store,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID, "{}", err.message);
        assert!(
            !target.exists(),
            "a refused org move must not clone {}",
            target.display()
        );
        let row = store
            .lock()
            .unwrap()
            .get_catalog_by_name("acme")
            .unwrap()
            .unwrap();
        assert_eq!(
            row.repo_path,
            origin.to_string_lossy(),
            "still where it was"
        );
        registry::clear().unwrap();
    }

    /// Re-pointing an org catalog through `add_catalog` with its own org is
    /// allowed (R12): the row keeps its id and moves to the new checkout,
    /// which is what gets loaded.
    #[test]
    fn re_pointing_an_org_catalog_in_its_own_org_moves_it() {
        let _g = lock_registry_for_test();
        let store = store_with_org();
        let first = add_catalog(
            add("acme", &git_repo("repoint-a", 1, &["a"]), Some("acme")),
            &store,
        )
        .unwrap();
        let second_repo = git_repo("repoint-b", 1, &["b", "c"]);
        let second = add_catalog(add("acme", &second_repo, Some("acme")), &store).unwrap();
        assert_eq!(second.id, first.id, "the same catalog, re-pointed");
        assert_eq!(second.repo_path, second_repo.to_string_lossy());
        assert_eq!((second.state.as_str(), second.asset_count), ("loaded", 2));
        registry::clear().unwrap();
    }

    /// Only a checkout failure becomes a problem entry (R5, as in
    /// `ensure_fresh`): a store failure while recording the load — here a
    /// trigger refusing the HEAD write that follows a clean parse —
    /// propagates, and nothing is installed in the registry. A trigger rather
    /// than `PRAGMA query_only` because the upsert before the load must
    /// still succeed for the load path to be reached at all.
    #[test]
    fn a_store_failure_while_loading_propagates_without_a_problem_entry() {
        let _g = lock_registry_for_test();
        let store = store_with_org();
        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute_batch(
                "CREATE TRIGGER refuse_head BEFORE UPDATE OF head_commit ON catalogs \
                 WHEN NEW.head_commit IS NOT NULL BEGIN SELECT RAISE(ABORT, 'no head'); END;",
            )
            .unwrap();
        let err = add_catalog(
            add("acme", &git_repo("store-fail", 1, &["a"]), Some("acme")),
            &store,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_SQLITE, "{}", err.message);
        let id = store
            .lock()
            .unwrap()
            .get_catalog_by_name("acme")
            .unwrap()
            .unwrap()
            .id;
        assert!(
            registry::get(id).unwrap().is_none(),
            "a store failure must not install a problem entry"
        );
        registry::clear().unwrap();
    }
}
