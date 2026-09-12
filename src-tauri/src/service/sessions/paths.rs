//! Path identity for sessions: owner/repo and worktree-key extraction, the
//! per-host projects roots (`HostPaths`), project lookup for a cwd, and the
//! local / remote cwd resolution the lifecycle calls use.

use super::*;

/// Extract `(owner, repo)` from a path that follows the conventional
/// `.../projects/github.com/<owner>/<repo>/...` layout (the same layout
/// `proj-clean` enforces on disk). Remote hosts often store repos under
/// a different prefix (e.g. `/home/mjanci/...` instead of `/Users/...`),
/// but the GitHub portion is stable — so we match into the repo cell
/// regardless of where the path starts.
pub(super) fn extract_owner_repo(path: &str) -> Option<(String, String)> {
    static RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"/projects/github\.com/([^/]+)/([^/]+)").expect("static regex")
    });
    let caps = RE.captures(path)?;
    Some((
        caps.get(1)?.as_str().to_string(),
        caps.get(2)?.as_str().to_string(),
    ))
}

/// Derive a portable worktree name from a session's cwd. Host-path-independent:
///   - <repo>/.claude/worktrees/<name>[/…]  → Some("<name>")
///   - <repo>/.worktrees/<name>[/…]         → Some("<name>")
///   - <repo> root or any other subdir       → Some("main")
///   - path without a github.com repo segment → None (orphan)
///
/// Both worktree layouts are recognized: `worktree_add_script` and the
/// ecosystem's `proj-clean` use `.worktrees/` *or* `.claude/worktrees/`, and a
/// session living under either must key to its worktree name (not "main"), or
/// recreate/restart would rebuild it at the repo root.
pub(super) fn worktree_key_for_path(path: &str) -> Option<String> {
    static RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"/projects/github\.com/[^/]+/[^/]+(/.*)?$").expect("static regex")
    });
    let caps = RE.captures(path)?;
    Some(worktree_key_from_remainder(
        caps.get(1).map(|m| m.as_str()).unwrap_or(""),
    ))
}

/// Worktree name from the part of a cwd below the repo directory (`""`,
/// `/src/lib`, `/.worktrees/feat/src`, …).
pub(super) fn worktree_key_from_remainder(remainder: &str) -> String {
    // Check `.claude/worktrees/` first — it is the more specific marker, and
    // `/.worktrees/` is not a substring of `/.claude/worktrees/`.
    for marker in ["/.claude/worktrees/", "/.worktrees/"] {
        if let Some(idx) = remainder.find(marker) {
            let after = &remainder[idx + marker.len()..];
            if let Some(name) = after.split('/').next() {
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }
    "main".to_string()
}

/// A host's projects root and layout (the `projects.*` settings), captured
/// under the store lock so cwd → project linking also works off-lock (the PR
/// probe). `local` holds the absolute scan root; remote roots stay
/// unexpanded, since `$HOME` is not known here, so a `~` / `~/rest` root is
/// anchored right after the path's home directory ([`below_home`]). The
/// layout only says where a repo sits under the root; worktree subdirs are
/// recognised separately. `worktrees` holds the host's known worktree
/// checkouts as `(path, project_id)`: for `local` the scan's
/// `git worktree list` (which lists linked worktrees wherever they live), for
/// a remote host what its EnterWorktree hooks reported. A cwd in a linked
/// worktree OUTSIDE the root layout, such as a sibling folder of the repo,
/// still resolves to the repo's project through them.
#[derive(Debug, Clone)]
pub(crate) struct HostPaths {
    pub(super) root: String,
    pub(super) layout: crate::projects::Layout,
    pub(super) worktrees: Vec<(String, i64)>,
}

impl HostPaths {
    pub(crate) fn for_host(s: &Store, alias: &str) -> Self {
        use crate::service::projects::{layout, local_projects_root, project_base_for, LOCAL_HOST};
        let root = if alias == LOCAL_HOST {
            local_projects_root(s).to_string_lossy().into_owned()
        } else {
            project_base_for(s, alias)
        };
        let worktrees = s
            .list_worktrees_on_host(alias)
            .unwrap_or_default()
            .into_iter()
            .map(|w| (w.path, w.project_id))
            .collect();
        HostPaths {
            root,
            layout: layout(s),
            worktrees,
        }
    }

    /// The longest known worktree checkout on this host that contains the cwd
    /// in any of its `spellings`, by whole components, as `(project_id,
    /// matched path length)`; the length lets the caller weigh it against a
    /// project-root match.
    pub(super) fn project_by_worktree(&self, spellings: &[&str]) -> Option<(i64, usize)> {
        self.worktrees
            .iter()
            .filter(|(wt, _)| {
                !wt.is_empty()
                    && spellings
                        .iter()
                        .any(|p| crate::service::projects::strip_root(p, wt).is_some())
            })
            .max_by_key(|(wt, _)| wt.len())
            .map(|(wt, pid)| (*pid, wt.len()))
    }

    /// Path components below the root, when `path` lies under it. Compares
    /// whole components, so root `/data/git` does not match `/data/git-old`.
    pub(super) fn below_root<'a>(&self, path: &'a str) -> Option<Vec<&'a str>> {
        let root = self.root.trim_end_matches('/');
        let rest = if root == "~" {
            // The home directory itself is the root. Where home ends is only
            // known for the standard layouts; elsewhere it cannot be told
            // apart from the projects below it, so nothing matches.
            below_home(path)?
        } else if let Some(tail) = root.strip_prefix("~/") {
            match below_home(path) {
                // Anchored right after $HOME: root `~/code` never matches a
                // `/code/` run deeper in the path, nor a user named `code`.
                Some(home_rest) => crate::service::projects::strip_root(home_rest, tail)?,
                // Unrecognised home layout: fall back to the first `/tail/`
                // run anywhere in the path. This can mis-anchor when the home
                // path itself contains that run; the github.com regex
                // fallback in the callers has the same limit.
                None => {
                    let needle = format!("/{tail}/");
                    let idx = path.find(&needle)?;
                    &path[idx + needle.len()..]
                }
            }
        } else if root.starts_with('/') {
            crate::service::projects::strip_root(path, root)?
        } else {
            return None;
        };
        Some(rest.split('/').filter(|c| !c.is_empty()).collect())
    }

    /// `(owner, repo, remainder below the repo)` for a cwd under the root;
    /// `owner` is `None` under the flat layout.
    pub(super) fn locate<'a>(&self, path: &'a str) -> Option<(Option<&'a str>, &'a str, String)> {
        use crate::projects::Layout;
        let comps = self.below_root(path)?;
        let (owner, repo, rest) = match self.layout {
            Layout::Github if comps.len() >= 2 => (Some(comps[0]), comps[1], &comps[2..]),
            Layout::Flat if !comps.is_empty() => (None, comps[0], &comps[1..]),
            _ => return None,
        };
        let remainder = if rest.is_empty() {
            String::new()
        } else {
            format!("/{}", rest.join("/"))
        };
        Some((owner, repo, remainder))
    }
}

/// The part of an absolute path below its home directory, for the standard
/// home layouts `/home/<u>`, `/Users/<u>`, `/var/home/<u>` and `/root`: `""`
/// for the home itself, `None` for any other path. A remote `$HOME` is not
/// known when paths are matched, so this is how `~` roots are anchored.
pub(super) fn below_home(path: &str) -> Option<&str> {
    let p = path.strip_prefix('/')?;
    let depth = match p.split('/').next()? {
        "root" => 1,
        "home" | "Users" => 2,
        "var" if p.starts_with("var/home/") => 3,
        _ => return None,
    };
    let mut rest = p;
    for i in 0..depth {
        match rest.split_once('/') {
            Some((head, tail)) if !head.is_empty() => rest = tail,
            None if !rest.is_empty() && i + 1 == depth => rest = "",
            _ => return None,
        }
    }
    Some(rest)
}

/// `worktree_key_for_path` for a cwd on a host with a (possibly custom)
/// projects root / layout. The github.com regex stays the fallback.
pub(super) fn worktree_key_for_host(path: &str, paths: &HostPaths) -> Option<String> {
    match paths.locate(path) {
        Some((_, _, remainder)) => Some(worktree_key_from_remainder(&remainder)),
        None => worktree_key_for_path(path),
    }
}

/// Match a session's cwd to a known project id. `projects` is passed in by the
/// caller (fetched once per reconcile) rather than queried per session.
pub(crate) fn find_project_id_for_path(
    projects: &[ProjectRow],
    host_alias: &str,
    path: &std::path::Path,
    paths: &HostPaths,
) -> Option<i64> {
    let path_str = path.to_string_lossy();
    if host_alias == "local" {
        // Local paths: component-wise prefix match against the scanned
        // base_path (handles worktrees nested under repos; `/b/x` does not
        // capture `/b/x-build`), in the raw AND the canonical spelling of the
        // cwd. The scan stores physical base_paths, while a pane under a
        // symlinked root can report the logical one; without this every
        // local session there was orphaned (E_NOREPO on recreate/restart).
        // Rows stored before the scan canonicalized still match raw, and heal
        // on the next refresh. One canonicalize per session: a few local
        // stat calls, no await, so fine under the store lock.
        let canon = crate::projects::path_identity::canonical(path);
        let canon_str = canon.to_string_lossy();
        let within = |base: &str| {
            crate::service::projects::strip_root(&path_str, base).is_some()
                || crate::service::projects::strip_root(&canon_str, base).is_some()
        };
        // The most specific match wins: a project whose base contains the
        // cwd, or a checkout from `git worktree list` (a linked worktree
        // outside the layout, such as a sibling folder of the repo, belongs
        // to the repo that lists it). See `most_specific`.
        let by_root = projects
            .iter()
            .filter(|p| within(&p.base_path))
            .max_by_key(|p| p.base_path.len())
            .map(|p| (p.id, p.base_path.len()));
        return most_specific(by_root, paths.project_by_worktree(&[&path_str, &canon_str]));
    }
    // Remote paths: the project located under the host's configured root and
    // layout (owner/repo), weighed against the worktree checkouts this host's
    // hooks reported (`most_specific`), then the conventional
    // `.../projects/github.com/<owner>/<repo>/...` regex. `None` (orphan) if
    // nothing matches.
    let by_layout = remote_project_by_layout(projects, &path_str, paths);
    if let Some(pid) = most_specific(by_layout, paths.project_by_worktree(&[&path_str])) {
        return Some(pid);
    }
    let (owner, repo) = extract_owner_repo(&path_str)?;
    projects
        .iter()
        .find(|p| p.owner == owner && p.repo == repo)
        .map(|p| p.id)
}

/// The more specific of a project-root match and a worktree-row match, each
/// `(project_id, matched path length)`: the longer match wins, and a tie goes
/// to the worktree row. A tie means a project whose base IS that checkout,
/// typically a leftover duplicate of a linked worktree (a sibling folder once
/// scanned as its own repo and kept alive by a session). The worktree row
/// names the repo whose `git worktree list` holds that checkout, which must
/// win.
pub(super) fn most_specific(
    root: Option<(i64, usize)>,
    worktree: Option<(i64, usize)>,
) -> Option<i64> {
    match (root, worktree) {
        (Some((r, root_len)), Some((w, wt_len))) => Some(if wt_len >= root_len { w } else { r }),
        (root, worktree) => root.or(worktree).map(|(id, _)| id),
    }
}

/// The project of a remote cwd located under the host's configured root and
/// layout, matched by owner/repo (github layout) or a unique repo name
/// (flat), as `(project_id, length of the repo directory prefix of the cwd)`.
pub(super) fn remote_project_by_layout(
    projects: &[ProjectRow],
    path_str: &str,
    paths: &HostPaths,
) -> Option<(i64, usize)> {
    let (owner, repo, remainder) = paths.locate(path_str)?;
    let repo_len = path_str.len().saturating_sub(remainder.len());
    let pid = match owner {
        Some(o) => projects
            .iter()
            .find(|p| p.owner == o && p.repo == repo)
            .map(|p| p.id),
        None => {
            // Flat: repo name only; never guess between same-named repos.
            let mut same = projects.iter().filter(|p| p.repo == repo);
            match (same.next(), same.next()) {
                (Some(p), None) => Some(p.id),
                _ => None,
            }
        }
    }?;
    Some((pid, repo_len))
}

/// Look up `(owner, repo)` for a given project id.
pub(crate) fn fetch_owner_repo(s: &Store, project_id: i64) -> Result<(String, String), IpcError> {
    let mut stmt = s
        .conn_ref()
        .prepare("SELECT owner, repo FROM projects WHERE id=?1")?;
    stmt.query_row(rusqlite::params![project_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })
    .map_err(IpcError::from)
}

/// `(name, branch, host_alias, path)` of a worktree row. `branch` may be NULL
/// in the DB. `host_alias` is the host the row's checkout actually lives on
/// (`local` for the project scan's rows, a remote alias for a host-scanned
/// row — see `service::worktrees::list_host_worktrees`); `path` is that
/// checkout's real path AS RECORDED ON THAT HOST, which may not follow the
/// `<project_root>/.claude/worktrees/<name>` convention (e.g. `.worktrees/`,
/// or anywhere else git has it registered).
pub(super) fn fetch_worktree(
    s: &Store,
    worktree_id: i64,
) -> Result<(String, Option<String>, String, String), IpcError> {
    let mut stmt = s
        .conn_ref()
        .prepare("SELECT name, branch, host_alias, path FROM worktrees WHERE id=?1")?;
    stmt.query_row(rusqlite::params![worktree_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })
    .map_err(IpcError::from)
}

/// Build the absolute path on the remote host where a project (and optional
/// worktree) should live: `<root>/<owner>/<repo>` (`github` layout) or
/// `<root>/<repo>` (`flat`) for the project root, plus
/// `.claude/worktrees/<wt>` for non-main worktrees (a best-effort guess;
/// `service::repair` resolves the real worktree dir on the host). `root` must
/// already be absolute (see `remote_project_path_for`). Returns just the
/// project root if `wt_name` is None or "main".
pub(crate) fn remote_project_path(
    root: &str,
    layout: crate::projects::Layout,
    owner: &str,
    repo: &str,
    wt_name: Option<&str>,
) -> (String, String) {
    let project_root = layout.project_dir(root, owner, repo);
    let cwd = match wt_name {
        Some(name) if name != "main" => {
            format!("{project_root}/.claude/worktrees/{name}")
        }
        _ => project_root.clone(),
    };
    (project_root, cwd)
}

/// `remote_project_path` with the host's projects root resolved from the
/// `projects.*` settings (default `~/projects/github.com`) and expanded
/// against the remote `$HOME`.
pub(super) fn remote_project_path_for(
    s: &Store,
    host: &str,
    home: &str,
    owner: &str,
    repo: &str,
    wt_name: Option<&str>,
) -> (String, String) {
    use crate::service::projects::{expand_home, layout, project_base_for};
    let root = expand_home(&project_base_for(s, host), home);
    remote_project_path(&root, layout(s), owner, repo, wt_name)
}

/// Resolve the cwd a session should (re)open in for a LOCAL host. Order: the
/// session's worktree path (by `worktree_id`) → its project `base_path` →
/// error. Remote hosts must go through [`cwd_source_for_session`] /
/// [`resolve_cwd_source`] instead — the local paths in this table do not
/// exist on the remote machine.
pub(super) fn resolve_session_cwd(
    s: &Store,
    row: &crate::store::SessionRow,
) -> Result<String, IpcError> {
    if let Some(wt_id) = row.worktree_id {
        if let Some(path) = s.worktree_path(wt_id)? {
            return Ok(path);
        }
    }
    let base = match row.project_id {
        Some(pid) => s.project_base_path(pid)?,
        None => None,
    };
    // Sessions discovered by reconcile carry only `worktree_key` (the worktree
    // dir name, derived from their live cwd), never `worktree_id` — reconcile
    // does not resolve the FK. Honor the key so a session in a worktree is
    // recreated there, not at the repo root.
    if let (Some(pid), Some(key)) = (row.project_id, row.worktree_key.as_deref()) {
        if key != "main" {
            // 1. The worktrees table is authoritative (and handles non-standard
            //    locations) — when it is fresh.
            if let Some(path) = s
                .list_worktrees_for_project(pid)?
                .into_iter()
                .find(|w| w.name == key)
                .map(|w| w.path)
            {
                return Ok(path);
            }
            // 2. The table is only refreshed by `refresh_projects`, so a
            //    just-created worktree (the common recreate case) may be absent.
            //    Reconstruct from the on-disk standard layouts under the base.
            if let Some(ref base) = base {
                if let Some(path) =
                    worktree_path_on_disk(base, key, |p| std::path::Path::new(p).exists())
                {
                    return Ok(path);
                }
            }
        }
    }
    if let Some(base) = base {
        return Ok(base);
    }
    Err(IpcError::new(
        "E_NOREPO",
        "cannot determine a worktree path for this session",
    ))
}

/// Reconstruct a local worktree's path from the project `base` and its dir
/// name `key`, trying the two layouts the ecosystem uses (`.claude/worktrees/`
/// then `.worktrees/`). Returns the first that `exists`. Used as a fallback
/// when the worktrees table has not been refreshed since the worktree was made.
pub(super) fn worktree_path_on_disk(
    base: &str,
    key: &str,
    exists: impl Fn(&str) -> bool,
) -> Option<String> {
    for layout in [".claude/worktrees", ".worktrees"] {
        let candidate = format!("{base}/{layout}/{key}");
        if exists(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Per-host cwd resolution input, captured under the store lock before any
/// async work. For `local` we already have the absolute path; for remote we
/// keep the (owner, repo, worktree-name) tuple and only translate to a path
/// once `ssh.remote_home` resolves off-lock.
#[cfg_attr(test, derive(Debug))]
pub(super) enum CwdSource {
    Local(String),
    Remote {
        /// Host's projects root from the `projects.*` settings, unexpanded
        /// (may start with `~/`; expanded against the remote `$HOME`).
        root: String,
        layout: crate::projects::Layout,
        owner: String,
        repo: String,
        wt_name: Option<String>,
    },
}

/// Pick the cwd-resolution strategy for `row` while holding the store lock.
/// Falls back from worktree → project root (matching the local resolver) when
/// the worktree row is missing.
pub(super) fn cwd_source_for_session(
    s: &Store,
    row: &crate::store::SessionRow,
) -> Result<CwdSource, IpcError> {
    if row.host_alias == "local" {
        return Ok(CwdSource::Local(resolve_session_cwd(s, row)?));
    }
    let pid = row.project_id.ok_or_else(|| {
        IpcError::new(
            "E_NOREPO",
            "cannot determine a remote path: session has no project",
        )
    })?;
    let (owner, repo) = fetch_owner_repo(s, pid)?;
    // Like the local resolver: reconciled sessions only have `worktree_key`, so
    // fall back to it when the FK is unset. `remote_project_path` maps a
    // non-"main" name to `<repo>/.claude/worktrees/<name>`, matching how
    // `new_session` creates remote worktrees.
    let wt_name = match row.worktree_id {
        Some(wid) => s.worktree_name(wid)?,
        None => row
            .worktree_key
            .as_deref()
            .filter(|k| *k != "main")
            .map(str::to_string),
    };
    Ok(CwdSource::Remote {
        root: crate::service::projects::project_base_for(s, &row.host_alias),
        layout: crate::service::projects::layout(s),
        owner,
        repo,
        wt_name,
    })
}

/// Off-lock half of host-aware cwd resolution: for remote sources, look up
/// the remote `$HOME` and assemble the project (or worktree) path using the
/// same convention `new_session` uses. Local sources pass straight through.
pub(super) async fn resolve_cwd_source(
    src: CwdSource,
    host_alias: &str,
    ssh: &Arc<SshClient>,
) -> Result<String, IpcError> {
    match src {
        CwdSource::Local(p) => Ok(p),
        CwdSource::Remote {
            root,
            layout,
            owner,
            repo,
            wt_name,
        } => {
            let home = ssh.remote_home(host_alias).await?;
            let root = crate::service::projects::expand_home(&root, &home);
            let (_root, cwd) =
                remote_project_path(&root, layout, &owner, &repo, wt_name.as_deref());
            Ok(cwd)
        }
    }
}
