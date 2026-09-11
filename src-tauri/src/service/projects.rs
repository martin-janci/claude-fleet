use crate::ipc_error::IpcError;
use crate::projects::path_identity::{canonical, canonical_str};
use crate::projects::{
    git_common_dir, list_worktrees, scan_projects, DiscoveredProject, DiscoveredWorktree, Layout,
};
use crate::service::settings;
use crate::store::{ProjectRow, Store, WorktreeRow};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
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

/// `$CLAUDE_FLEET_PROJECTS_BASE`, trimmed; `None` when unset or blank.
pub fn local_env_base() -> Option<String> {
    std::env::var(ENV_BASE)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// `Some(rest)` when `path` is `root` itself (`""`) or lies below it, compared
/// by whole path components: root `/b/x` does not contain `/b/x-build`.
pub(crate) fn strip_root<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    let root = root.trim_end_matches('/');
    if root.is_empty() {
        return Some(path.trim_start_matches('/'));
    }
    if path == root {
        return Some("");
    }
    path.strip_prefix(root)?.strip_prefix('/')
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
    let env = if is_local { local_env_base() } else { None };
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

/// One scanned repository: its `git worktree list` (canonical paths, main
/// first) and its canonical `git rev-parse --git-common-dir`. `None` when git
/// could not answer; such a repo is never merged with another.
struct Scanned {
    dp: DiscoveredProject,
    worktrees: Vec<DiscoveredWorktree>,
    common_dir: Option<PathBuf>,
}

/// Keep ONE project per git repository. Two scanned directories sharing a
/// common dir are checkouts of the same repo, and each one's `git worktree
/// list` names every worktree, so keeping both stores each worktree twice
/// (the sidebar showed sales-twins-app's worktrees up to four times). The
/// directory that IS the main worktree wins, then the lowest (owner, repo)
/// for determinism.
fn dedupe_by_common_dir(mut scanned: Vec<Scanned>) -> Vec<Scanned> {
    fn is_main(sc: &Scanned) -> bool {
        sc.worktrees
            .first()
            .is_some_and(|w| w.path == sc.dp.base_path)
    }
    scanned.sort_by(|a, b| {
        (!is_main(a), &a.dp.owner, &a.dp.repo).cmp(&(!is_main(b), &b.dp.owner, &b.dp.repo))
    });
    let mut seen = HashSet::new();
    scanned.retain(|sc| match &sc.common_dir {
        Some(dir) => seen.insert(dir.clone()),
        None => true,
    });
    scanned
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

    // 2. Scan the filesystem (IO-only, no lock). `read_dir` and canonicalize
    //    are blocking std::fs, so they run on the blocking pool rather than
    //    stalling a tokio worker. The scan canonicalizes the root, so a
    //    symlinked root stores physical base_paths. The snapshot's paths are
    //    canonicalized here too, for the duplicate check in step 4.
    let root_raw = base.to_string_lossy().into_owned();
    let snap_paths: Vec<String> = snapshot
        .iter()
        .flat_map(|p| {
            std::iter::once(p.project.base_path.clone())
                .chain(p.worktrees.iter().map(|w| w.path.clone()))
        })
        .collect();
    // Every known worktree row, for its parent-fingerprint keys: resolved in
    // the blocking task below (off-lock), used by the deletes in step 4.
    let snap_rows: Vec<WorktreeRow> = snapshot
        .iter()
        .flat_map(|p| p.worktrees.iter().cloned())
        .collect();
    let (discovered, root_canon, canon_of, fp_keys) = tokio::task::spawn_blocking(move || {
        let root_canon = canonical(&base).to_string_lossy().into_owned();
        let discovered = scan_projects(&base, layout)?;
        let mut canon_of: HashMap<String, String> = HashMap::new();
        for p in snap_paths {
            let c = canonical_str(&p);
            canon_of.insert(p, c);
        }
        let fp_keys = Store::fingerprint_keys_for(&snap_rows);
        Ok::<_, IpcError>((discovered, root_canon, canon_of, fp_keys))
    })
    .await
    .map_err(|e| IpcError::new("E_IO", format!("project scan task failed: {e}")))??;

    // 3. Fan-out: `git worktree list` + `git rev-parse --git-common-dir` per
    //    discovered project, off-lock and in parallel. Both are async
    //    (tokio::process), so N repos never pin N worker threads (BE-4). A
    //    failed join or `list_worktrees` skips the project, as before.
    let mut set = tokio::task::JoinSet::new();
    for dp in discovered {
        set.spawn(async move {
            let worktrees = list_worktrees(&dp.base_path).await;
            let common_dir = git_common_dir(&dp.base_path).await;
            (dp, worktrees, common_dir)
        });
    }
    let mut scanned = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok((dp, Ok(worktrees), common_dir)) = joined {
            scanned.push(Scanned {
                dp,
                worktrees,
                common_dir,
            });
        }
    }
    let scanned = dedupe_by_common_dir(scanned);

    // 4. Apply all writes under a single brief lock.
    {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        let canon = |p: &str| canon_of.get(p).cloned().unwrap_or_else(|| p.to_string());
        let mut fresh_ids = HashSet::new();
        // Canonical checkout path -> the fresh project that owns it.
        let mut owner_of: HashMap<String, i64> = HashMap::new();
        for sc in &scanned {
            let project_id = s.upsert_project(
                &sc.dp.owner,
                &sc.dp.repo,
                &sc.dp.base_path.to_string_lossy(),
            )?;
            fresh_ids.insert(project_id);
            let mut keep_names = Vec::with_capacity(sc.worktrees.len());
            for wt in &sc.worktrees {
                let path = wt.path.to_string_lossy().into_owned();
                keep_names.push(wt.name.clone());
                s.upsert_worktree(project_id, &wt.name, &path, wt.branch.as_deref())?;
                owner_of.insert(path, project_id);
            }
            // Renamed rows (e.g. the old basename-named main row, now `main`)
            // hand their session references to the surviving row with the
            // same canonical path before they go.
            s.delete_worktrees_not_in(project_id, &keep_names, canon, &fp_keys)?;
        }

        // Duplicates, the self-heal for rows an earlier scan left behind:
        // worktree rows under a project that was NOT rediscovered, naming a
        // checkout a fresh project now owns (compared canonically, since old
        // rows may hold the logical spelling). Each goes unless a session
        // references it. A duplicate project whose own base_path is such a
        // checkout (a linked worktree scanned as a repo) then goes too, once
        // it has no rows and no sessions left.
        let mut removed = HashSet::new();
        for p in &snapshot {
            let id = p.project.id;
            if fresh_ids.contains(&id) {
                continue;
            }
            let owned_elsewhere = |path: &str| {
                owner_of
                    .get(canon(path).as_str())
                    .is_some_and(|owner| *owner != id)
            };
            let mut left = 0usize;
            for wt in &p.worktrees {
                if !(owned_elsewhere(&wt.path) && s.delete_worktree_if_unused(wt.id)?) {
                    left += 1;
                }
            }
            if left == 0
                && owned_elsewhere(&p.project.base_path)
                && s.delete_project_if_unused(id, &fp_keys)?
            {
                removed.insert(id);
                eprintln!(
                    "[projects] removed duplicate project {}/{} at {} (a worktree of another project)",
                    p.project.owner, p.project.repo, p.project.base_path
                );
            }
        }

        // Stale rows: projects from an earlier scan that were not found again,
        // whose base_path is outside the (possibly changed) root, and that no
        // session references. Left in place they would keep capturing
        // sessions through prefix linking. Rows with sessions are kept. The
        // root is checked in both spellings, so canonicalizing a symlinked
        // root does not make every old logical row look "outside".
        for p in &snapshot {
            let row = &p.project;
            if fresh_ids.contains(&row.id) || removed.contains(&row.id) {
                continue;
            }
            let inside = strip_root(&row.base_path, &root_raw).is_some()
                || strip_root(&canon(&row.base_path), &root_canon).is_some();
            if !inside && s.delete_project_if_unused(row.id, &fp_keys)? {
                eprintln!(
                    "[projects] removed stale project {}/{} at {} (outside {root_raw})",
                    row.owner, row.repo, row.base_path
                );
            }
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

    #[test]
    fn strip_root_compares_whole_components() {
        assert_eq!(strip_root("/b/x/src", "/b/x"), Some("src"));
        assert_eq!(strip_root("/b/x", "/b/x/"), Some(""));
        assert_eq!(strip_root("/b/x-build", "/b/x"), None);
        assert_eq!(strip_root("/c/x", "/b/x"), None);
    }

    #[tokio::test]
    async fn refresh_projects_drops_stale_rows_outside_the_new_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let (gone, busy, inside) = {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            let gone = s.upsert_project("o", "old", "/old/root/o/old").unwrap();
            let busy = s.upsert_project("o", "busy", "/old/root/o/busy").unwrap();
            s.upsert_session("dev", "local", Some(busy), None, 1, 1, "running", None)
                .unwrap();
            // Under the new root but not rediscovered (no .git): kept.
            let inside_path = tmp.path().join("o").join("gone-from-disk");
            let inside = s
                .upsert_project("o", "gone-from-disk", &inside_path.to_string_lossy())
                .unwrap();
            let map = serde_json::json!({ "local": tmp.path().to_string_lossy() }).to_string();
            settings::set(&s, settings::PROJECTS_BASE_PATH, &map).unwrap();
            (gone, busy, inside)
        };
        let rows = refresh_projects(&store).await.unwrap();
        let ids: Vec<i64> = rows.iter().map(|r| r.project.id).collect();
        assert!(
            !ids.contains(&gone),
            "stale, unused row outside the root is removed"
        );
        assert!(
            ids.contains(&busy),
            "a project with sessions is never dropped"
        );
        assert!(ids.contains(&inside), "rows under the root are left alone");
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
        assert_eq!(
            rows[0].project.base_path,
            canonical(&repo).to_string_lossy()
        );
    }

    #[test]
    fn dedupe_keeps_the_main_checkout_per_common_dir() {
        let sc = |repo: &str, base: &str, first: &str, common: Option<&str>| Scanned {
            dp: DiscoveredProject {
                owner: "o".into(),
                repo: repo.into(),
                base_path: PathBuf::from(base),
            },
            worktrees: vec![DiscoveredWorktree {
                name: "main".into(),
                path: PathBuf::from(first),
                branch: None,
            }],
            common_dir: common.map(PathBuf::from),
        };
        let kept = dedupe_by_common_dir(vec![
            // A linked worktree whose name sorts first still loses to main.
            sc("a-wt", "/r/a-wt", "/r/a", Some("/r/a/.git")),
            sc("a", "/r/a", "/r/a", Some("/r/a/.git")),
            // No common dir: never merged.
            sc("b", "/r/b", "/r/b", None),
            sc("b2", "/r/b2", "/r/b2", None),
        ]);
        let names: Vec<_> = kept.iter().map(|s| s.dp.repo.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "b2"]);
    }

    /// The live bug: a linked worktree scanned as its own project stored a
    /// copy of every worktree (and misnamed `main` onto itself), in whatever
    /// spelling git reported. A refresh must list each checkout once, under
    /// the main checkout, canonically, and delete the copies no session
    /// references.
    #[cfg(unix)]
    #[tokio::test]
    async fn refresh_projects_dedupes_by_common_dir_and_heals_duplicate_rows() {
        use crate::projects::test_git::{init_repo, run};
        use std::path::Path;
        let tmp = tempfile::TempDir::new().unwrap();
        let mnt = tmp.path().join("mnt");
        let app = mnt.join("o").join("app");
        if !init_repo(&app) {
            return; // no git on this box
        }
        // `projects -> mnt`, like the owner's `~/projects -> /mnt/sda4/projects`.
        let link = tmp.path().join("projects");
        std::os::unix::fs::symlink(&mnt, &link).unwrap();
        let app_l = link.join("o").join("app");
        let wt_l = link.join("o").join("app-wt");
        // A sibling linked worktree, added through the logical spelling.
        assert!(run(
            &app_l,
            &["worktree", "add", "-q", &wt_l.to_string_lossy(), "-b", "wt"]
        ));
        let app_c = canonical(&app).to_string_lossy().into_owned();
        let wt_c = canonical(&mnt.join("o").join("app-wt"))
            .to_string_lossy()
            .into_owned();
        let l = |p: &Path| p.to_string_lossy().into_owned();

        let store = Mutex::new(Store::open_in_memory().unwrap());
        let (dup, busy, pinned) = {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            // The configured root is the symlink.
            let map = serde_json::json!({ "local": l(&link) }).to_string();
            settings::set(&s, settings::PROJECTS_BASE_PATH, &map).unwrap();
            // What a pre-fix scan left: the linked worktree as a project,
            // `main` misnamed onto it, every worktree copied.
            let dup = s.upsert_project("o", "app-wt", &l(&wt_l)).unwrap();
            s.upsert_worktree(dup, "main", &l(&wt_l), Some("wt"))
                .unwrap();
            s.upsert_worktree(dup, "app", &l(&app_l), None).unwrap();
            // A second copy, one of whose rows a session still references.
            let busy = s.upsert_project("o", "app-copy", &l(&wt_l)).unwrap();
            let pinned = s.upsert_worktree(busy, "app", &l(&app_l), None).unwrap();
            s.upsert_worktree(busy, "app-wt", &wt_c, Some("wt"))
                .unwrap();
            s.upsert_session("dev", "local", None, Some(pinned), 1, 1, "running", None)
                .unwrap();
            (dup, busy, pinned)
        };

        let rows = refresh_projects(&store).await.unwrap();
        let by_id = |id: i64| rows.iter().find(|r| r.project.id == id);
        let main_row = rows
            .iter()
            .find(|r| r.project.repo == "app")
            .expect("the main checkout is scanned");
        assert_eq!(main_row.project.base_path, app_c, "physical base_path");
        let mut wts: Vec<_> = main_row
            .worktrees
            .iter()
            .map(|w| (w.name.as_str(), w.path.as_str()))
            .collect();
        wts.sort();
        assert_eq!(
            wts,
            vec![("app-wt", wt_c.as_str()), ("main", app_c.as_str())],
            "each checkout once, main first-entry named, canonical paths"
        );
        assert!(
            by_id(dup).is_none(),
            "an unreferenced duplicate project goes"
        );
        let busy_row = by_id(busy).expect("a project with a referenced row stays");
        let ids: Vec<i64> = busy_row.worktrees.iter().map(|w| w.id).collect();
        assert_eq!(
            ids,
            vec![pinned],
            "only the session-referenced copy survives"
        );

        // Idempotent: a second refresh changes nothing, row for row.
        let before = serde_json::to_value(&rows).unwrap();
        let again = refresh_projects(&store).await.unwrap();
        assert_eq!(serde_json::to_value(&again).unwrap(), before);
    }

    /// Main-first naming replaces the old basename-named main row with
    /// `main`. A session still pointing at the old row must not abort the
    /// refresh on the foreign key (which then failed every later refresh
    /// too); it moves to the `main` row of the same checkout.
    #[tokio::test]
    async fn refresh_projects_repoints_sessions_from_a_renamed_main_row() {
        use crate::projects::test_git::init_repo;
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("o").join("claude-fleet");
        if !init_repo(&repo) {
            return; // no git on this box
        }
        let repo_c = canonical(&repo).to_string_lossy().into_owned();
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let (pid, sid) = {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            let map = serde_json::json!({ "local": tmp.path().to_string_lossy() }).to_string();
            settings::set(&s, settings::PROJECTS_BASE_PATH, &map).unwrap();
            let pid = s.upsert_project("o", "claude-fleet", &repo_c).unwrap();
            // What the old scan stored: main named after the directory.
            let old = s
                .upsert_worktree(pid, "claude-fleet", &repo.to_string_lossy(), None)
                .unwrap();
            let sid = s
                .upsert_session("dev", "local", Some(pid), Some(old), 1, 1, "running", None)
                .unwrap();
            (pid, sid)
        };
        let rows = refresh_projects(&store)
            .await
            .expect("a referenced renamed row must not abort the refresh");
        let row = rows.iter().find(|r| r.project.id == pid).unwrap();
        let names: Vec<_> = row.worktrees.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, vec!["main"]);
        let main_id = row.worktrees[0].id;
        {
            let s = store.lock().unwrap();
            assert_eq!(
                s.get_session_by_id(sid).unwrap().unwrap().worktree_id,
                Some(main_id),
                "the session moved to the main row"
            );
        }
        // Later refreshes keep working.
        refresh_projects(&store).await.unwrap();
    }

    /// Remote hosts' worktree rows (stored by their EnterWorktree hooks) are
    /// not the local scan's to prune: a local refresh keeps them, even one
    /// named like a local row, and the local project tree never lists them.
    #[tokio::test]
    async fn refresh_projects_never_deletes_remote_worktree_rows() {
        use crate::projects::test_git::init_repo;
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("o").join("app");
        if !init_repo(&repo) {
            return; // no git on this box
        }
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let (pid, remote) = {
            let s = store.lock().unwrap();
            let map = serde_json::json!({ "local": tmp.path().to_string_lossy() }).to_string();
            settings::set(&s, settings::PROJECTS_BASE_PATH, &map).unwrap();
            let pid = s
                .upsert_project("o", "app", &canonical(&repo).to_string_lossy())
                .unwrap();
            // Same name as the local main row: host-scoped, so no clash.
            let remote = s
                .upsert_worktree_on(
                    "mefistos",
                    pid,
                    "main",
                    "/home/m/projects/github.com/o/app",
                    None,
                )
                .unwrap();
            (pid, remote)
        };
        let rows = refresh_projects(&store).await.unwrap();
        let row = rows.iter().find(|r| r.project.id == pid).unwrap();
        assert_eq!(row.worktrees.len(), 1, "only the local main checkout");
        assert_eq!(row.worktrees[0].host_alias, "local");
        assert_ne!(row.worktrees[0].id, remote);
        let s = store.lock().unwrap();
        let kept = s
            .get_worktree_row(remote)
            .unwrap()
            .expect("the remote row survives the local refresh");
        assert_eq!(
            (kept.host_alias.as_str(), kept.name.as_str()),
            ("mefistos", "main")
        );
    }
}
