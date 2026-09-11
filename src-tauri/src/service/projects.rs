use crate::ipc_error::IpcError;
use crate::projects::{list_worktrees, scan_projects, Layout};
use crate::service::settings;
use crate::store::{ProjectRow, Store, WorktreeRow};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Serialize)]
pub struct ProjectTreeRow {
    pub project: ProjectRow,
    pub worktrees: Vec<WorktreeRow>,
}

/// Legacy override for the LOCAL projects root. Still honoured, below the
/// `projects.base_path` setting.
pub const ENV_BASE: &str = "CLAUDE_FLEET_PROJECTS_BASE";

/// Alias of the machine running the app.
pub const LOCAL_HOST: &str = "local";

// ── projects-root resolution ────────────────────────────────────────────────
//
// The "projects root" is the directory whose children are the layout's
// top-level entries (`<root>/<owner>/<repo>` for `github`, `<root>/<repo>`
// for `flat`). Precedence, per host:
//   1. `projects.base_path` setting entry for that host alias
//   2. `$CLAUDE_FLEET_PROJECTS_BASE`: `local` only. It is an env var of the
//      app process and says nothing about a remote filesystem.
//   3. the layout's default root (`~/projects/github.com` | `~/projects`)
// With no setting stored, every path is identical to the pre-setting
// behaviour.

/// Pure precedence: setting, then env (local only), then the layout default.
/// Blank values count as unset. The result may still start with `~/`; see
/// [`expand_home`].
pub fn resolve_base(
    setting: Option<&str>,
    env: Option<&str>,
    layout: Layout,
    is_local: bool,
) -> String {
    fn non_blank(v: Option<&str>) -> Option<&str> {
        v.map(str::trim).filter(|v| !v.is_empty())
    }
    if let Some(v) = non_blank(setting) {
        return v.to_string();
    }
    if is_local {
        if let Some(v) = non_blank(env) {
            return v.to_string();
        }
    }
    layout.default_root().to_string()
}

/// Expand a leading `~` / `~/` against `home`. Other paths pass through.
pub fn expand_home(root: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    if root == "~" {
        return if home.is_empty() {
            "/".into()
        } else {
            home.into()
        };
    }
    match root.strip_prefix("~/") {
        Some(rest) => format!("{home}/{rest}"),
        None => root.to_string(),
    }
}

/// The configured `projects.layout`.
pub fn layout(s: &Store) -> Layout {
    Layout::parse(&settings::get_string(s, settings::PROJECTS_LAYOUT))
}

/// Projects root for `host_alias`, unexpanded (may start with `~/`; a remote
/// caller expands it against the remote `$HOME`). Single source of truth for
/// every place that derives a project path.
pub fn project_base_for(s: &Store, host_alias: &str) -> String {
    let is_local = host_alias == LOCAL_HOST;
    let setting = settings::base_path_map(s).remove(host_alias);
    let env = if is_local {
        std::env::var(ENV_BASE).ok()
    } else {
        None
    };
    resolve_base(setting.as_deref(), env.as_deref(), layout(s), is_local)
}

/// Absolute local projects root (the directory `refresh_projects` scans).
pub fn local_projects_root(s: &Store) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    PathBuf::from(expand_home(&project_base_for(s, LOCAL_HOST), &home))
}

/// Resolved root per known host, for the Settings preview: `local` absolute,
/// remote hosts in `~/` form (their `$HOME` is only known over SSH).
pub fn resolved_bases(s: &Store) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    out.insert(
        LOCAL_HOST.to_string(),
        local_projects_root(s).to_string_lossy().into_owned(),
    );
    for h in s.list_hosts().unwrap_or_default() {
        if h.alias != LOCAL_HOST {
            let base = project_base_for(s, &h.alias);
            out.insert(h.alias, base);
        }
    }
    out
}

pub fn list_projects(store: &Mutex<Store>) -> Result<Vec<ProjectTreeRow>, IpcError> {
    let s = store.lock().map_err(|_| IpcError::lock())?;
    s.list_projects_joined()
}

pub async fn refresh_projects(store: &Mutex<Store>) -> Result<Vec<ProjectTreeRow>, IpcError> {
    // 1. Resolve the scan root + layout and snapshot the current project list
    //    under a brief lock.
    let (base, layout, snapshot) = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        (
            local_projects_root(&s),
            layout(&s),
            s.list_projects_joined()?,
        )
    };

    // 2. Scan the filesystem for new/removed projects (IO-only, no lock needed).
    //    `read_dir` over the whole projects tree is blocking std::fs, so it
    //    runs on the blocking pool rather than stalling a tokio worker.
    let discovered = tokio::task::spawn_blocking(move || scan_projects(&base, layout))
        .await
        .map_err(|e| IpcError::new("E_IO", format!("project scan task failed: {e}")))??;

    // 3. Fan-out: run `git worktree list` for each discovered project, off-lock
    //    and in parallel using tokio tasks. `list_worktrees` is async
    //    (tokio::process) so N repos never pin N worker threads (BE-4).
    let mut set = tokio::task::JoinSet::new();
    for dp in discovered.iter().cloned() {
        let _ = &snapshot; // borrow-check: snapshot not moved into tasks
        set.spawn(async move {
            let result = list_worktrees(&dp.base_path).await;
            (dp, result)
        });
    }

    // Collect git results off-lock.
    let mut upserts: Vec<(
        crate::projects::DiscoveredProject,
        Vec<crate::projects::DiscoveredWorktree>,
    )> = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok((dp, Ok(worktrees))) = joined {
            upserts.push((dp, worktrees));
        }
        // If the join errored or `list_worktrees` failed, skip — same behaviour
        // as the original `Err(_) => continue`.
    }

    // 4. Apply all writes under a single brief lock.
    {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        for (dp, worktrees) in &upserts {
            let project_id =
                s.upsert_project(&dp.owner, &dp.repo, &dp.base_path.to_string_lossy())?;
            let mut keep_names = Vec::with_capacity(worktrees.len());
            for wt in worktrees {
                keep_names.push(wt.name.clone());
                s.upsert_worktree(
                    project_id,
                    &wt.name,
                    &wt.path.to_string_lossy(),
                    wt.branch.as_deref(),
                )?;
            }
            s.delete_worktrees_not_in(project_id, &keep_names)?;
        }
    }

    // 5. Return the fresh list under one final brief lock.
    let s = store.lock().map_err(|_| IpcError::lock())?;
    s.list_projects_joined()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/home/u";

    fn local_root(setting: Option<&str>, env: Option<&str>, layout: Layout) -> String {
        expand_home(&resolve_base(setting, env, layout, true), HOME)
    }

    #[test]
    fn precedence_setting_over_env_over_default() {
        // setting wins over env
        assert_eq!(
            local_root(Some("/srv/repos"), Some("/env/base"), Layout::Github),
            "/srv/repos"
        );
        // env wins over default
        assert_eq!(
            local_root(None, Some("/env/base"), Layout::Github),
            "/env/base"
        );
        // default last
        assert_eq!(
            local_root(None, None, Layout::Github),
            "/home/u/projects/github.com"
        );
        // blank values are "unset"
        assert_eq!(
            local_root(Some("  "), Some(""), Layout::Github),
            "/home/u/projects/github.com"
        );
    }

    #[test]
    fn env_var_applies_to_local_only() {
        assert_eq!(
            resolve_base(None, Some("/env/base"), Layout::Github, false),
            "~/projects/github.com"
        );
        assert_eq!(
            resolve_base(Some("~/code"), Some("/env/base"), Layout::Flat, false),
            "~/code"
        );
    }

    #[test]
    fn layout_default_root_and_tilde_expansion() {
        assert_eq!(local_root(None, None, Layout::Flat), "/home/u/projects");
        assert_eq!(
            local_root(Some("~/code"), None, Layout::Flat),
            "/home/u/code"
        );
        assert_eq!(expand_home("~", "/home/u/"), "/home/u");
        assert_eq!(expand_home("~/p", "/"), "/p");
        assert_eq!(expand_home("/abs/p", HOME), "/abs/p");
    }

    #[test]
    fn existing_config_resolves_exactly_as_before() {
        // No settings stored: identical to the old `projects_base()`,
        // `$HOME/projects/github.com` or the env var verbatim.
        let s = Store::open_in_memory().unwrap();
        assert_eq!(layout(&s), Layout::Github);
        assert_eq!(project_base_for(&s, "mefistos"), "~/projects/github.com");
        // The env var is process-global; tests never set it (set_var races
        // other tests). Assert the default only when the box has none.
        if std::env::var(ENV_BASE).is_err() {
            let old = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
                .join("projects")
                .join("github.com");
            assert_eq!(local_projects_root(&s), old);
        }
    }

    #[test]
    fn project_base_for_reads_the_per_host_setting() {
        let s = Store::open_in_memory().unwrap();
        settings::set(
            &s,
            settings::PROJECTS_BASE_PATH,
            r#"{"local":"/srv/local-repos","mefistos":"~/code"}"#,
        )
        .unwrap();
        assert_eq!(project_base_for(&s, "local"), "/srv/local-repos");
        assert_eq!(project_base_for(&s, "mefistos"), "~/code");
        // a host without an entry falls through to the default
        assert_eq!(project_base_for(&s, "other"), "~/projects/github.com");
        settings::set(&s, settings::PROJECTS_LAYOUT, "flat").unwrap();
        assert_eq!(layout(&s), Layout::Flat);
        assert_eq!(project_base_for(&s, "other"), "~/projects");
        assert_eq!(local_projects_root(&s), PathBuf::from("/srv/local-repos"));
    }

    #[test]
    fn resolved_bases_lists_local_and_every_host() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("mefistos").unwrap();
        settings::set(
            &s,
            settings::PROJECTS_BASE_PATH,
            r#"{"mefistos":"/data/git"}"#,
        )
        .unwrap();
        let r = resolved_bases(&s);
        assert_eq!(r["mefistos"], "/data/git");
        assert!(r.contains_key(LOCAL_HOST));
    }

    #[tokio::test]
    async fn refresh_projects_scans_the_flat_setting_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("solo");
        std::fs::create_dir_all(&repo).unwrap();
        let ok = std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&repo)
            .status()
            .map(|st| st.success())
            .unwrap_or(false);
        if !ok {
            return; // no git on this box; scan-level coverage lives in crate::projects
        }
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            let map = serde_json::json!({ "local": tmp.path().to_string_lossy() }).to_string();
            settings::set(&s, settings::PROJECTS_BASE_PATH, &map).unwrap();
            settings::set(&s, settings::PROJECTS_LAYOUT, "flat").unwrap();
        }
        let rows = refresh_projects(&store).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].project.repo, "solo");
        assert_eq!(rows[0].project.base_path, repo.to_string_lossy());
    }
}
