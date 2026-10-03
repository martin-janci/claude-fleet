//! `fleet-hub catalog …` — point the hub at its asset catalog (a git
//! checkout of skills, agents, hooks, MCP servers and plugin refs on this
//! machine) and reload it. The desktop does this in its Assets tab; a paired
//! client cannot, because the checkout has to be on the hub's machine.
//!
//! Each subcommand works on `state.db` and the checkout directly, so it does
//! not need a running hub. A running hub notices at its next catalog call
//! (`catalog::ensure_fresh` compares the load recorded here with its own);
//! on a paired client that is the Assets tab's Refresh.
//!
//! `add`/`list`/`remove`/`admit`/`unadmit` manage org catalogs (Assets M3);
//! `set` stays the personal one's, and `reload --catalog NAME` reloads any.

use crate::config::{self, HubOptions};
use crate::out;
use crate::serve;
use clap::Subcommand;
use fleet_core::ipc_error::codes;
use fleet_core::service::catalog::catalogs::{self, AddCatalogArgs};
use fleet_core::service::catalog::{self, ConfigureArgs};
use fleet_core::store::Store;
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::Mutex;

#[derive(Subcommand, Debug)]
pub enum CatalogCmd {
    /// Print where the catalog is and what was last loaded.
    Show,
    /// Use the git checkout at PATH as the catalog, then load it.
    ///
    /// When PATH has no checkout and --remote is given, the remote is cloned
    /// into it (with this machine's git credentials — for an SSH remote, the
    /// key `fleet-hub ssh-key` prints must be allowed to read it).
    Set {
        /// The checkout, on this machine. `~/` is this user's home.
        path: String,
        /// Clone from this URL when PATH is not a checkout yet.
        #[arg(long)]
        remote: Option<String>,
    },
    /// Re-read a checkout, after editing it or pulling by hand.
    Reload {
        /// `git pull --ff-only` first.
        #[arg(long)]
        pull: bool,
        /// The catalog to reload; the personal one by default.
        #[arg(long)]
        catalog: Option<String>,
    },
    /// Add an org's catalog — a git checkout on this machine — and load it.
    /// On an existing name, re-point it. `add personal <path>` is `set`.
    Add {
        /// The catalog's name (`[a-z0-9][a-z0-9-]*`).
        name: String,
        /// The checkout, on this machine. `~/` is this user's home.
        path: String,
        /// Clone from this URL when PATH is not a checkout yet.
        #[arg(long)]
        remote: Option<String>,
        /// The org that owns it (by name). Required for every catalog but `personal`.
        #[arg(long)]
        org: Option<String>,
    },
    /// Load or refresh every catalog (as a running hub would), then print
    /// each: owner, load state, HEAD, path, admissions, grants.
    List,
    /// Forget an org catalog (config only: the checkout is never deleted).
    /// Its layer assignments, admissions and grants go; hosts keep what it installed.
    Remove { name: String },
    /// Let a host with no org take an org catalog.
    Admit { host: String, catalog: String },
    /// Take an admission back. Nothing is removed from the host.
    Unadmit { host: String, catalog: String },
}

pub fn run(
    cmd: CatalogCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    serve::existing_db(&config::resolve_data_dir(opts, env))?;
    let store = Mutex::new(serve::open_store(opts, env)?);
    match cmd {
        CatalogCmd::Show => show(&store),
        CatalogCmd::Set { path, remote } => set_personal(&store, path, remote),
        CatalogCmd::Reload {
            pull,
            catalog: None,
        } => load(&store, pull),
        CatalogCmd::Reload {
            pull,
            catalog: Some(name),
        } if name == catalogs::PERSONAL => load(&store, pull),
        CatalogCmd::Reload {
            pull,
            catalog: Some(name),
        } => {
            let row = catalogs::catalog_named(&name, &store).map_err(|e| {
                if e.code == codes::E_NOTFOUND {
                    format!("no catalog named {name}; see `fleet-hub catalog list`")
                } else {
                    e.message
                }
            })?;
            let s = catalog::load_catalog(row.id, pull, &store).map_err(|e| e.message)?;
            print_loaded(&s);
            Ok(ExitCode::SUCCESS)
        }
        // R12: `add personal <path>` is `set <path>`.
        CatalogCmd::Add {
            name,
            path,
            remote,
            org: None,
        } if name == catalogs::PERSONAL => set_personal(&store, path, remote),
        CatalogCmd::Add {
            name,
            path,
            remote,
            org,
        } => add(&store, name, path, remote, org),
        CatalogCmd::List => list(&store),
        CatalogCmd::Remove { name } => {
            let r = catalogs::remove_catalog(&name, &store).map_err(|e| e.message)?;
            out::line(&format!(
                "removed catalog {} (config only; the checkout is untouched): dropped {} layer \
                 assignment(s), {} admission(s), {} grant(s); hosts keep what it installed",
                r.name, r.layer_rows, r.admissions, r.grants
            ));
            Ok(ExitCode::SUCCESS)
        }
        CatalogCmd::Admit { host, catalog } => {
            let names = catalogs::admit(&host, &catalog, &store).map_err(|e| e.message)?;
            out::line(&format!("{host} admits: {}", names.join(", ")));
            Ok(ExitCode::SUCCESS)
        }
        CatalogCmd::Unadmit { host, catalog } => {
            let (was_admitted, names) =
                catalogs::unadmit_reporting(&host, &catalog, &store).map_err(|e| e.message)?;
            out::line(&unadmit_line(&host, &catalog, was_admitted, &names));
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// What `catalog unadmit` prints: whether anything was taken back, then
/// what the host still admits.
fn unadmit_line(host: &str, catalog: &str, was_admitted: bool, names: &[String]) -> String {
    let rest = if names.is_empty() {
        format!("{host} admits no org catalog")
    } else {
        format!("{host} admits: {}", names.join(", "))
    };
    if was_admitted {
        rest
    } else {
        format!("{host} did not admit {catalog}; nothing changed. {rest}")
    }
}

/// `catalog list`'s rows after `ensure_fresh`. A checkout failure it
/// returns can only be the personal catalog's (an org catalog's is kept as a
/// problem entry), so it lands on the personal row: `problem`, with the
/// error, whatever this process's registry held before. Any other error is
/// handed back to be printed on its own line.
fn list_rows(
    store: &Mutex<Store>,
) -> Result<(Vec<catalogs::CatalogStatus>, Option<String>), String> {
    let refresh_err = catalog::ensure_fresh(store).err();
    let mut all = catalogs::list_catalogs(store).map_err(|e| e.message)?;
    let Some(e) = refresh_err else {
        return Ok((all, None));
    };
    let personal = all.iter_mut().find(|c| c.org_id.is_none());
    match personal {
        Some(p) if catalog::is_load_failure(&e) => {
            p.state = "problem".to_string();
            p.problem = Some(e.message);
            p.asset_count = 0;
            Ok((all, None))
        }
        _ => Ok((all, Some(e.message))),
    }
}

fn set_personal(
    store: &Mutex<Store>,
    path: String,
    remote: Option<String>,
) -> Result<ExitCode, String> {
    catalog::configure(
        ConfigureArgs {
            repo_path: path,
            remote_url: remote,
        },
        store,
    )
    .map_err(|e| e.message)?;
    load(store, false)
}

fn add(
    store: &Mutex<Store>,
    name: String,
    path: String,
    remote: Option<String>,
    org: Option<String>,
) -> Result<ExitCode, String> {
    // Who may own which catalog is the service's (`Store::check_catalog_owner`);
    // the CLI only adds how to say it here.
    let no_org = org.is_none();
    let existed = fleet_core::ipc_error::lock(store)
        .and_then(|s| Ok(s.get_catalog_by_name(&name)?.is_some()))
        .map_err(|e| e.message)?;
    let st = catalogs::add_catalog(
        AddCatalogArgs {
            name: name.clone(),
            repo_path: path,
            remote_url: remote,
            org,
        },
        store,
    )
    .map_err(|e| {
        // Only the owner rule's "needs an org" refusal — not a bad name or
        // an empty path, which `--org` would not fix.
        if no_org
            && e.code == codes::E_INVALID
            && e.message == Store::catalog_needs_an_org_message(&name)
        {
            format!("{}; pass --org NAME", e.message)
        } else {
            e.message
        }
    })?;
    let verb = if existed { "re-pointed" } else { "added" };
    if let Some(problem) = st.problem {
        return Err(format!(
            "{verb} catalog {name}, but it could not be loaded: {problem}; fix the checkout and \
             run `fleet-hub catalog reload --catalog {name}`"
        ));
    }
    out::line(&format!(
        "{verb} catalog {} ({} asset(s)); a running hub picks it up at its next catalog call",
        st.name, st.asset_count
    ));
    Ok(ExitCode::SUCCESS)
}

fn show(store: &Mutex<Store>) -> Result<ExitCode, String> {
    match catalog::config(store).map_err(|e| e.message)? {
        None => out::line(
            "no asset catalog; set one with: fleet-hub catalog set <path> [--remote <url>]",
        ),
        Some(cfg) => {
            out::line(&format!("path    {}", cfg.repo_path));
            out::line(&format!(
                "remote  {}",
                cfg.remote_url.as_deref().unwrap_or("—")
            ));
            out::line(&format!(
                "head    {}",
                cfg.head_commit
                    .as_deref()
                    .filter(|h| !h.is_empty())
                    .map_or("— (not loaded yet)", |h| &h[..h.len().min(12)])
            ));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn print_loaded(s: &fleet_core::events::CatalogSummary) {
    out::line(&format!(
        "loaded {} asset(s) at {}{}; a running hub picks it up at its next catalog call (Refresh on a client)",
        s.asset_count,
        &s.head[..s.head.len().min(12)],
        if s.problem_count > 0 {
            format!(", {} problem(s)", s.problem_count)
        } else {
            String::new()
        }
    ));
}

fn load(store: &Mutex<Store>, pull: bool) -> Result<ExitCode, String> {
    let s = catalog::load(pull, store).map_err(|e| e.message)?;
    print_loaded(&s);
    Ok(ExitCode::SUCCESS)
}

/// `catalog list`: loads or refreshes every catalog first (`ensure_fresh`:
/// this process's registry starts empty, so the state column would say
/// nothing otherwise), then prints one line per catalog (NAME, OWNER, STATE,
/// HEAD, PATH) and its admissions and grants (R19) on their own lines. A
/// broken org catalog shows as `problem`; so does a personal one that failed
/// to load, with the error `ensure_fresh` returned for it.
fn list(store: &Mutex<Store>) -> Result<ExitCode, String> {
    let (all, unplaced) = list_rows(store)?;
    if let Some(e) = unplaced {
        out::line(&format!("refreshing the catalogs failed: {e}"));
    }
    if all.is_empty() {
        out::line(
            "no catalogs; set the personal one with: fleet-hub catalog set <path> [--remote <url>]",
        );
        return Ok(ExitCode::SUCCESS);
    }
    out::line(&format!(
        "{:<14} {:<12} {:<10} {:<12} PATH",
        "NAME", "OWNER", "STATE", "HEAD"
    ));
    for c in all {
        out::line(&format!(
            "{:<14} {:<12} {:<10} {:<12} {}",
            c.name,
            c.org.as_deref().unwrap_or("personal"),
            c.state,
            c.head_commit
                .as_deref()
                .filter(|h| !h.is_empty())
                .map_or("—", |h| &h[..h.len().min(12)]),
            c.repo_path
        ));
        if !c.admitted.is_empty() {
            out::line(&format!("    admitted: {}", c.admitted.join(", ")));
        }
        if !c.granted.is_empty() {
            out::line(&format!("    granted:  {}", c.granted.join(", ")));
        }
        if let Some(p) = c.problem {
            out::line(&format!("    problem:  {p}"));
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// fleet-core's catalog registry is process-global and its
    /// `lock_registry_for_test` is `cfg(test)` in that crate, so the tests
    /// here that load catalogs serialize on their own lock (PF15).
    static REGISTRY: Mutex<()> = Mutex::new(());

    fn git_repo(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let root = dir.join(name);
        std::fs::create_dir_all(root.join("skills/s")).unwrap();
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        std::fs::write(
            root.join("skills/s/asset.yaml"),
            "kind: skill\nname: s\ndescription: d\n",
        )
        .unwrap();
        std::fs::write(root.join("skills/s/body.md"), "b\n").unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.email", "t@t"],
            &["config", "user.name", "t"],
            &["add", "."],
            &["commit", "-q", "-m", "init"],
        ] {
            let o = fleet_core::proc::std_command("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        }
        root
    }

    #[test]
    fn add_admit_list_reload_and_remove_an_org_catalog() {
        let _registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let opts = HubOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ..HubOptions::default()
        };
        let env = HashMap::new();
        let org_id = {
            let s = Store::open_with_bus(
                &dir.path().join("state.db"),
                Arc::new(fleet_core::events::NoopEventBus),
            )
            .unwrap();
            s.upsert_host("h").unwrap();
            s.add_org("acme", None, false).unwrap().id
        };
        // A personal catalog, so `remove personal` reaches its own guard (PF7).
        let personal = git_repo(dir.path(), "personal-assets");
        run(
            CatalogCmd::Set {
                path: personal.to_string_lossy().into(),
                remote: None,
            },
            &opts,
            &env,
        )
        .unwrap();
        // R12: `add personal <path>` is `set <path>` (no --org needed).
        let add_personal = CatalogCmd::Add {
            name: "personal".into(),
            path: personal.to_string_lossy().into(),
            remote: None,
            org: None,
        };
        assert_eq!(run(add_personal, &opts, &env).unwrap(), ExitCode::SUCCESS);
        let refused = run(
            CatalogCmd::Add {
                name: "personal".into(),
                path: personal.to_string_lossy().into(),
                remote: None,
                org: Some("acme".into()),
            },
            &opts,
            &env,
        )
        .unwrap_err();
        assert!(refused.contains("belongs to no org"), "{refused}");
        run(
            CatalogCmd::Reload {
                pull: false,
                catalog: Some("personal".into()),
            },
            &opts,
            &env,
        )
        .unwrap();
        let unknown = run(
            CatalogCmd::Reload {
                pull: false,
                catalog: Some("nope".into()),
            },
            &opts,
            &env,
        )
        .unwrap_err();
        assert_eq!(
            unknown,
            "no catalog named nope; see `fleet-hub catalog list`"
        );
        let repo = git_repo(dir.path(), "acme-assets");
        let add = |org: Option<&str>| CatalogCmd::Add {
            name: "acme".into(),
            path: repo.to_string_lossy().into(),
            remote: None,
            org: org.map(String::from),
        };
        assert!(run(add(None), &opts, &env).unwrap_err().contains("--org"));
        assert_eq!(
            run(add(Some("acme")), &opts, &env).unwrap(),
            ExitCode::SUCCESS
        );
        // Re-pointing (R12): with its own --org it works; without, the
        // service's owner rule refuses and the CLI says how to pass it.
        let moved = git_repo(dir.path(), "acme-assets-2");
        let repoint = |org: Option<&str>| CatalogCmd::Add {
            name: "acme".into(),
            path: moved.to_string_lossy().into(),
            remote: None,
            org: org.map(String::from),
        };
        let refused = run(repoint(None), &opts, &env).unwrap_err();
        assert!(
            refused.contains("needs an org") && refused.ends_with("; pass --org NAME"),
            "{refused}"
        );
        // Any other refusal with no --org gets no hint: --org would not fix
        // an empty path.
        let empty_path = CatalogCmd::Add {
            name: "acme".into(),
            path: "  ".into(),
            remote: None,
            org: None,
        };
        let refused = run(empty_path, &opts, &env).unwrap_err();
        assert!(
            refused.contains("repo_path must not be empty") && !refused.contains("--org"),
            "{refused}"
        );
        assert_eq!(
            run(repoint(Some("acme")), &opts, &env).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            serve::open_store(&opts, &env)
                .unwrap()
                .get_catalog_by_name("acme")
                .unwrap()
                .unwrap()
                .repo_path,
            moved.to_string_lossy()
        );
        run(
            CatalogCmd::Admit {
                host: "h".into(),
                catalog: "acme".into(),
            },
            &opts,
            &env,
        )
        .unwrap();
        assert_eq!(
            run(CatalogCmd::List, &opts, &env).unwrap(),
            ExitCode::SUCCESS
        );
        run(
            CatalogCmd::Reload {
                pull: false,
                catalog: Some("acme".into()),
            },
            &opts,
            &env,
        )
        .unwrap();

        let s = serve::open_store(&opts, &env).unwrap();
        let acme = s.get_catalog_by_name("acme").unwrap().expect("added");
        assert_eq!(acme.org_id, Some(org_id));
        assert!(acme.head_commit.is_some(), "loaded and recorded");
        assert_eq!(s.host_admissions("h").unwrap(), vec![acme.id]);
        drop(s);

        run(
            CatalogCmd::Unadmit {
                host: "h".into(),
                catalog: "acme".into(),
            },
            &opts,
            &env,
        )
        .unwrap();
        // A second unadmit changes nothing, and the store says so.
        let again = catalogs::unadmit_reporting(
            "h",
            "acme",
            &Mutex::new(serve::open_store(&opts, &env).unwrap()),
        )
        .unwrap();
        assert_eq!(again, (false, Vec::<String>::new()));
        run(
            CatalogCmd::Remove {
                name: "acme".into(),
            },
            &opts,
            &env,
        )
        .unwrap();
        let s = serve::open_store(&opts, &env).unwrap();
        assert!(s.get_catalog_by_name("acme").unwrap().is_none());
        assert!(
            repo.join(".git").is_dir(),
            "remove is config only: the checkout stays"
        );
        drop(s);
        let refused = run(
            CatalogCmd::Remove {
                name: "personal".into(),
            },
            &opts,
            &env,
        )
        .unwrap_err();
        assert!(
            refused.contains("personal catalog cannot be removed"),
            "{refused}"
        );
    }

    /// A personal catalog that fails to load shows on its own row as
    /// `problem` with the error, not as a bare `not_loaded`.
    #[test]
    fn list_puts_a_personal_load_error_on_the_personal_row() {
        let _registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let opts = HubOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ..HubOptions::default()
        };
        let env = HashMap::new();
        drop(
            Store::open_with_bus(
                &dir.path().join("state.db"),
                Arc::new(fleet_core::events::NoopEventBus),
            )
            .unwrap(),
        );
        let personal = git_repo(dir.path(), "personal-assets");
        std::fs::write(personal.join("catalog.yaml"), "schema_version: 99\n").unwrap();
        let o = fleet_core::proc::std_command("git")
            .args(["commit", "-q", "-am", "unsupported schema"])
            .current_dir(&personal)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let set = CatalogCmd::Set {
            path: personal.to_string_lossy().into(),
            remote: None,
        };
        assert!(
            run(set, &opts, &env).is_err(),
            "the checkout does not parse"
        );

        let store = Mutex::new(serve::open_store(&opts, &env).unwrap());
        let (rows, unplaced) = list_rows(&store).unwrap();
        assert_eq!(unplaced, None);
        let row = rows
            .iter()
            .find(|c| c.name == "personal")
            .expect("configured");
        assert_eq!(row.state, "problem");
        assert!(row.problem.as_deref().is_some_and(|p| !p.is_empty()));
    }

    #[test]
    fn unadmit_says_when_nothing_was_admitted() {
        let none: Vec<String> = Vec::new();
        assert_eq!(
            unadmit_line("h", "acme", false, &none),
            "h did not admit acme; nothing changed. h admits no org catalog"
        );
        assert_eq!(
            unadmit_line("h", "acme", true, &["beta".to_string()]),
            "h admits: beta"
        );
    }
}
