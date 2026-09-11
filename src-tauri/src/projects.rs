use crate::ipc_error::IpcError;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveredProject {
    pub owner: String,
    pub repo: String,
    pub base_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveredWorktree {
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
}

/// On-disk arrangement of repositories under a projects root.
///
/// The root is the directory whose children are the layout's top-level
/// entries, i.e. the same thing the `CLAUDE_FLEET_PROJECTS_BASE` env var has
/// always named:
///   - `Github`: `<root>/<owner>/<repo>`. With the default root
///     `~/projects/github.com` this is the historical
///     `~/projects/github.com/<owner>/<repo>` convention.
///   - `Flat`:   `<root>/<repo>`.
///
/// The layout only says where a *repository* sits under the root. It says
/// nothing about where that repo's git worktrees live (`.worktrees/<name>` or
/// `.claude/worktrees/<name>`); that is a separate, per-repo convention
/// resolved elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Github,
    Flat,
}

impl Layout {
    pub const FLAT: &'static str = "flat";

    /// Parse a stored `projects.layout` value; anything unknown is `Github`
    /// (the historical behaviour).
    pub fn parse(value: &str) -> Layout {
        match value.trim() {
            Self::FLAT => Layout::Flat,
            _ => Layout::Github,
        }
    }

    /// Default projects root when neither the setting nor (for `local`) the
    /// env var names one. `~/` is expanded against the host's `$HOME`.
    pub fn default_root(self) -> &'static str {
        match self {
            Layout::Github => "~/projects/github.com",
            Layout::Flat => "~/projects",
        }
    }

    /// Directory of one project under `root` in this layout.
    pub fn project_dir(self, root: &str, owner: &str, repo: &str) -> String {
        let root = root.trim_end_matches('/');
        match self {
            Layout::Github => format!("{root}/{owner}/{repo}"),
            Layout::Flat => format!("{root}/{repo}"),
        }
    }
}

/// Scans `base` according to `layout` and returns every directory that
/// contains a `.git` entry (regular dir or worktree gitfile).
pub fn scan_projects(base: &Path, layout: Layout) -> Result<Vec<DiscoveredProject>, IpcError> {
    let mut out = match layout {
        Layout::Github => scan_owner_repo(base)?,
        Layout::Flat => scan_flat(base)?,
    };
    out.sort_by(|a, b| {
        (a.owner.as_str(), a.repo.as_str()).cmp(&(b.owner.as_str(), b.repo.as_str()))
    });
    Ok(out)
}

/// Non-hidden subdirectories of `dir`, as `(name, path)`.
fn child_dirs(dir: &Path) -> Result<Vec<(String, PathBuf)>, IpcError> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        out.push((name, entry.path()));
    }
    Ok(out)
}

/// `Layout::Github`: walks `base/<owner>/<repo>` two levels deep.
fn scan_owner_repo(base: &Path) -> Result<Vec<DiscoveredProject>, IpcError> {
    let mut out = Vec::new();
    if !base.exists() {
        return Ok(out);
    }
    for (owner, owner_path) in child_dirs(base)? {
        for (repo, path) in child_dirs(&owner_path)? {
            if path.join(".git").exists() {
                out.push(DiscoveredProject {
                    owner: owner.clone(),
                    repo,
                    base_path: path,
                });
            }
        }
    }
    Ok(out)
}

/// `Layout::Flat`: walks `base/<repo>` one level deep. The owner comes from
/// the repo's `origin` remote URL, so a remote host can still clone
/// `git@github.com:<owner>/<repo>.git`. A repo without a parseable origin
/// falls back to the base directory's name.
fn scan_flat(base: &Path) -> Result<Vec<DiscoveredProject>, IpcError> {
    let mut out = Vec::new();
    if !base.exists() {
        return Ok(out);
    }
    let fallback_owner = base
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| crate::validate::path_component("owner", n).is_ok())
        .unwrap_or_else(|| "local".to_string());
    for (repo, path) in child_dirs(base)? {
        let git = path.join(".git");
        if !git.exists() {
            continue;
        }
        let owner = std::fs::read_to_string(git.join("config"))
            .ok()
            .and_then(|cfg| origin_owner(&cfg))
            .unwrap_or_else(|| fallback_owner.clone());
        out.push(DiscoveredProject {
            owner,
            repo,
            base_path: path,
        });
    }
    Ok(out)
}

/// Owner segment of the `[remote "origin"]` URL in a `.git/config` body:
/// `git@github.com:<owner>/<repo>.git`, `https://github.com/<owner>/<repo>`,
/// `ssh://git@host/<owner>/<repo>.git`. `None` when absent or unparseable.
fn origin_owner(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_origin = line == "[remote \"origin\"]";
            continue;
        }
        if !in_origin {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() != "url" {
            continue;
        }
        let url = v.trim().trim_end_matches('/');
        let url = url.strip_suffix(".git").unwrap_or(url);
        let mut parts = url.rsplit(['/', ':']);
        let _repo = parts.next()?;
        let owner = parts.next()?;
        return crate::validate::path_component("owner", owner)
            .ok()
            .map(|_| owner.to_string());
    }
    None
}

/// Runs `git worktree list --porcelain` in `repo_path` and parses the result.
/// The main checkout is normalized to `name = "main"`; extras use the dir name.
///
/// Async via `tokio::process` so the per-project fan-out in
/// `service::projects::refresh_projects` awaits N git children instead of
/// blocking N tokio worker threads (BE-4).
pub async fn list_worktrees(repo_path: &Path) -> Result<Vec<DiscoveredWorktree>, IpcError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .await
        .map_err(|e| IpcError::new("E_GIT", format!("git worktree list failed: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(IpcError::new("E_GIT", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok(parse_worktree_porcelain(&stdout, repo_path))
}

fn parse_worktree_porcelain(input: &str, main_path: &Path) -> Vec<DiscoveredWorktree> {
    let mut out = Vec::new();
    let mut cur_path: Option<PathBuf> = None;
    let mut cur_branch: Option<String> = None;
    for line in input.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(path) = cur_path.take() {
                out.push(make_worktree(path, cur_branch.take(), main_path));
            }
            cur_path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("branch ") {
            cur_branch = Some(rest.trim_start_matches("refs/heads/").to_string());
        }
    }
    if let Some(path) = cur_path {
        out.push(make_worktree(path, cur_branch, main_path));
    }
    out
}

fn make_worktree(path: PathBuf, branch: Option<String>, main_path: &Path) -> DiscoveredWorktree {
    let name = if path == main_path {
        "main".to_string()
    } else {
        path.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".to_string())
    };
    DiscoveredWorktree { name, path, branch }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_project(base: &Path, owner: &str, repo: &str) -> PathBuf {
        let path = base.join(owner).join(repo);
        fs::create_dir_all(&path).unwrap();
        fs::create_dir(path.join(".git")).unwrap();
        path
    }

    #[test]
    fn scan_finds_owner_repo_with_dot_git() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path(), "martin-janci", "claude-fleet");
        make_project(tmp.path(), "papayapos", "pos-frontend");
        let projects = scan_projects(tmp.path(), Layout::Github).unwrap();
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].owner, "martin-janci");
        assert_eq!(projects[0].repo, "claude-fleet");
        assert_eq!(projects[1].owner, "papayapos");
        assert_eq!(projects[1].repo, "pos-frontend");
    }

    #[test]
    fn scan_skips_dirs_without_dot_git() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("o1").join("not-a-repo")).unwrap();
        make_project(tmp.path(), "o1", "real-repo");
        let projects = scan_projects(tmp.path(), Layout::Github).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].repo, "real-repo");
    }

    #[test]
    fn scan_returns_empty_for_missing_base() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("does-not-exist");
        assert!(scan_projects(&missing, Layout::Github).unwrap().is_empty());
        assert!(scan_projects(&missing, Layout::Flat).unwrap().is_empty());
    }

    #[test]
    fn scan_flat_finds_repos_one_level_deep() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().join("code");
        // origin-derived owner
        let a = base.join("alpha");
        fs::create_dir_all(a.join(".git")).unwrap();
        fs::write(
            a.join(".git").join("config"),
            "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = git@github.com:acme/alpha.git\n",
        )
        .unwrap();
        // no origin -> base dir name
        fs::create_dir_all(base.join("beta").join(".git")).unwrap();
        // not a repo
        fs::create_dir_all(base.join("notes")).unwrap();
        // github-layout nesting is not a flat repo
        make_project(&base, "someone", "nested");

        let projects = scan_projects(&base, Layout::Flat).unwrap();
        let got: Vec<_> = projects
            .iter()
            .map(|p| (p.owner.as_str(), p.repo.as_str()))
            .collect();
        assert_eq!(got, vec![("acme", "alpha"), ("code", "beta")]);
        assert_eq!(projects[0].base_path, a);
    }

    #[test]
    fn scan_github_ignores_flat_repos() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("flat-repo").join(".git")).unwrap();
        make_project(tmp.path(), "o", "r");
        let projects = scan_projects(tmp.path(), Layout::Github).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(
            (projects[0].owner.as_str(), projects[0].repo.as_str()),
            ("o", "r")
        );
    }

    #[test]
    fn origin_owner_parses_common_url_forms() {
        let cfg = |u: &str| format!("[remote \"origin\"]\n\turl = {u}\n");
        assert_eq!(
            origin_owner(&cfg("git@github.com:acme/r.git")).as_deref(),
            Some("acme")
        );
        assert_eq!(
            origin_owner(&cfg("https://github.com/acme/r")).as_deref(),
            Some("acme")
        );
        assert_eq!(
            origin_owner(&cfg("ssh://git@host/acme/r.git/")).as_deref(),
            Some("acme")
        );
        assert_eq!(
            origin_owner("[remote \"upstream\"]\n\turl = git@x:acme/r.git\n"),
            None
        );
        assert_eq!(origin_owner(""), None);
    }

    #[test]
    fn layout_parse_defaults_and_project_dir() {
        assert_eq!(Layout::parse("flat"), Layout::Flat);
        assert_eq!(Layout::parse("github"), Layout::Github);
        assert_eq!(Layout::parse("garbage"), Layout::Github);
        assert_eq!(Layout::Github.default_root(), "~/projects/github.com");
        assert_eq!(Layout::Flat.default_root(), "~/projects");
        assert_eq!(Layout::Github.project_dir("/h/p/", "o", "r"), "/h/p/o/r");
        assert_eq!(Layout::Flat.project_dir("/h/code", "o", "r"), "/h/code/r");
    }

    #[test]
    fn parse_worktree_porcelain_main_only() {
        let input = "worktree /repos/foo\nHEAD abc123\nbranch refs/heads/main\n\n";
        let wts = parse_worktree_porcelain(input, Path::new("/repos/foo"));
        assert_eq!(wts.len(), 1);
        assert_eq!(wts[0].name, "main");
        assert_eq!(wts[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn parse_worktree_porcelain_with_extras() {
        let input = "\
worktree /repos/foo
HEAD abc123
branch refs/heads/main

worktree /repos/foo/.worktrees/feature-x
HEAD def456
branch refs/heads/feature-x

worktree /repos/foo/.worktrees/bugfix
HEAD 789abc
branch refs/heads/bugfix
";
        let wts = parse_worktree_porcelain(input, Path::new("/repos/foo"));
        assert_eq!(wts.len(), 3);
        assert_eq!(wts[0].name, "main");
        assert_eq!(wts[1].name, "feature-x");
        assert_eq!(wts[2].name, "bugfix");
        assert_eq!(wts[1].branch.as_deref(), Some("feature-x"));
    }
}
