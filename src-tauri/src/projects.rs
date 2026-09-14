use crate::ipc_error::IpcError;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub mod path_identity;
use path_identity::canonical;

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

/// Scans `base` according to `layout` and returns every checkout: a
/// directory with a `.git` entry. That is a main checkout (`.git` is a
/// directory) or a checkout whose `.git` is a file: a linked worktree, or a
/// worktree of a bare repository (a bare-repo layout has no `.git` directory
/// anywhere, so filtering on one dropped such repos entirely). Several
/// checkouts of one repository are collapsed later by
/// `service::projects::refresh_projects`, which keeps one project per git
/// common dir and prefers the main checkout.
///
/// The base is canonicalized first, so a symlinked root stores the physical
/// `base_path` that tmux, git and `claude agents` report.
pub fn scan_projects(base: &Path, layout: Layout) -> Result<Vec<DiscoveredProject>, IpcError> {
    let canon = canonical(base);
    let mut out = match layout {
        Layout::Github => scan_owner_repo(&canon)?,
        Layout::Flat => scan_flat(&canon, base)?,
    };
    out.sort_by(|a, b| {
        (a.owner.as_str(), a.repo.as_str()).cmp(&(b.owner.as_str(), b.repo.as_str()))
    });
    Ok(out)
}

/// A checkout of some repository: `path/.git` exists, as a directory (main
/// checkout) or a file (linked worktree, bare-repo worktree).
fn is_checkout(path: &Path) -> bool {
    path.join(".git").exists()
}

/// Non-hidden subdirectories of `dir`, as `(name, path)`. `file_type()` does
/// not follow symlinks, so an entry that is itself a symlink (a repo folder
/// linked in from elsewhere) is skipped; its target is scanned only if it
/// also sits under the root.
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
            if is_checkout(&path) {
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
/// falls back to the base directory's name, taken from `owner_hint` (the
/// configured spelling of the root, so resolving a symlinked root does not
/// rename the owner).
fn scan_flat(base: &Path, owner_hint: &Path) -> Result<Vec<DiscoveredProject>, IpcError> {
    let mut out = Vec::new();
    if !base.exists() {
        return Ok(out);
    }
    let fallback_owner = owner_hint
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| crate::validate::path_component("owner", n).is_ok())
        .unwrap_or_else(|| "local".to_string());
    for (repo, path) in child_dirs(base)? {
        if !is_checkout(&path) {
            continue;
        }
        let git = path.join(".git");
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
/// The FIRST entry is named `main`: git always lists the main worktree first,
/// whichever checkout the command ran in. Extras use the dir name. Paths are
/// canonical: git reports each worktree in the form it was added from
/// (through a symlink or not), and one checkout must not become two rows.
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
    Ok(parse_worktree_porcelain(&stdout)
        .into_iter()
        .map(|mut wt| {
            wt.path = canonical(&wt.path);
            wt
        })
        .collect())
}

/// Canonical `git rev-parse --git-common-dir` of the checkout at `repo_path`:
/// the one git dir that a main checkout and all of its linked worktrees
/// share, so it identifies the repository whichever checkout is asked.
/// `None` when git cannot answer.
///
/// `--path-format=absolute` needs git 2.31+. Older git echoes the unknown
/// flag back and prints the dir relative to `repo_path`, so the last output
/// line is taken and joined onto `repo_path` when relative.
pub async fn git_common_dir(repo_path: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let line = stdout.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    let dir = Path::new(line);
    let abs = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        repo_path.join(dir)
    };
    Some(canonical(&abs))
}

/// Parse `git worktree list --porcelain`. The first entry is the main
/// worktree (git's ordering guarantee) and is named `main`; comparing paths
/// against the scanned directory instead misnamed the real main checkout
/// whenever the scan reached the repo through a linked worktree or another
/// spelling of the path. A bare main entry has no checkout and is skipped.
fn parse_worktree_porcelain(input: &str) -> Vec<DiscoveredWorktree> {
    // (path, branch, bare)
    let mut entries: Vec<(PathBuf, Option<String>, bool)> = Vec::new();
    for line in input.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            entries.push((PathBuf::from(rest), None, false));
        } else if let Some(entry) = entries.last_mut() {
            if let Some(rest) = line.strip_prefix("branch ") {
                entry.1 = Some(rest.trim_start_matches("refs/heads/").to_string());
            } else if line == "bare" {
                entry.2 = true;
            }
        }
    }
    entries
        .into_iter()
        .enumerate()
        .filter(|(_, (_, _, bare))| !bare)
        .map(|(i, (path, branch, _))| {
            let name = if i == 0 {
                "main".to_string()
            } else {
                path.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "unknown".to_string())
            };
            DiscoveredWorktree { name, path, branch }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn parse_worktree_porcelain_first_entry_is_main_wherever_it_ran() {
        // Listed from the linked worktree `app-wt`: git still puts the main
        // checkout first, and that one is `main`.
        let input = "worktree /repos/app\nHEAD a\nbranch refs/heads/dev\n\n\
                     worktree /repos/app-wt\nHEAD b\nbranch refs/heads/wt\n";
        let wts = parse_worktree_porcelain(input);
        assert_eq!(wts.len(), 2);
        assert_eq!(wts[0].name, "main");
        assert_eq!(wts[0].path, PathBuf::from("/repos/app"));
        assert_eq!(wts[0].branch.as_deref(), Some("dev"));
        assert_eq!(wts[1].name, "app-wt");
    }

    #[test]
    fn parse_worktree_porcelain_skips_a_bare_main() {
        let input = "worktree /repos/app.git\nbare\n\n\
                     worktree /repos/app/.worktrees/f\nHEAD b\nbranch refs/heads/f\n";
        let wts = parse_worktree_porcelain(input);
        assert_eq!(wts.len(), 1);
        assert_eq!(wts[0].name, "f");
    }

    /// Checkouts whose `.git` is a FILE are scanned: a linked worktree (the
    /// dedupe in `refresh_projects` folds it into its main checkout) and the
    /// bare-repo layout, which has no `.git` directory anywhere and was
    /// dropped entirely when the scan required one.
    #[test]
    fn scan_includes_checkouts_whose_git_is_a_file() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path(), "o", "app");
        // A linked worktree next to its main checkout.
        let wt = tmp.path().join("o").join("app-wt");
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join(".git"), "gitdir: ../app/.git/worktrees/app-wt\n").unwrap();
        // Bare-repo layout: `tool/.bare` is the repository, `tool/.git` a file.
        let bare = tmp.path().join("o").join("tool");
        fs::create_dir_all(bare.join(".bare")).unwrap();
        fs::write(bare.join(".git"), "gitdir: ./.bare\n").unwrap();
        // Not a checkout at all.
        fs::create_dir_all(tmp.path().join("o").join("notes")).unwrap();
        let got: Vec<_> = scan_projects(tmp.path(), Layout::Github)
            .unwrap()
            .into_iter()
            .map(|p| p.repo)
            .collect();
        assert_eq!(got, vec!["app", "app-wt", "tool"]);
        // Flat layout too.
        let flat = tmp.path().join("o");
        let got: Vec<_> = scan_projects(&flat, Layout::Flat)
            .unwrap()
            .into_iter()
            .map(|p| p.repo)
            .collect();
        assert_eq!(got, vec!["app", "app-wt", "tool"]);
    }

    #[cfg(unix)]
    #[test]
    fn scan_canonicalizes_a_symlinked_base() {
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real");
        make_project(&real, "o", "r");
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let projects = scan_projects(&link, Layout::Github).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].base_path, canonical(&real).join("o").join("r"));
        // The flat fallback owner keeps the configured root's name.
        fs::create_dir_all(real.join("solo").join(".git")).unwrap();
        let flat = scan_projects(&link, Layout::Flat).unwrap();
        assert_eq!(flat[0].owner, "link");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn list_worktrees_is_main_first_canonical_and_shares_a_common_dir() {
        use super::test_git::{init_repo, run};
        let tmp = TempDir::new().unwrap();
        let mnt = tmp.path().join("mnt");
        let app = mnt.join("app");
        if !init_repo(&app) {
            return; // no git on this box
        }
        let link = tmp.path().join("projects");
        std::os::unix::fs::symlink(&mnt, &link).unwrap();
        // Added through the logical spelling, like a fleet-spawned pane does.
        let wt_logical = link.join("app-wt");
        assert!(run(
            &app,
            &[
                "worktree",
                "add",
                "-q",
                &wt_logical.to_string_lossy(),
                "-b",
                "wt"
            ]
        ));
        let app_c = canonical(&app);
        let wt_c = canonical(&mnt.join("app-wt"));
        // Listed from the linked worktree: main is still the main checkout.
        let wts = list_worktrees(&wt_logical).await.unwrap();
        let got: Vec<_> = wts
            .iter()
            .map(|w| (w.name.as_str(), w.path.clone()))
            .collect();
        assert_eq!(got, vec![("main", app_c.clone()), ("app-wt", wt_c)]);
        let a = git_common_dir(&app).await.expect("common dir of main");
        let b = git_common_dir(&wt_logical)
            .await
            .expect("common dir of worktree");
        assert_eq!(a, b);
        assert_eq!(a, app_c.join(".git"));
    }

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
        assert_eq!(projects[0].base_path, canonical(&a));
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
        let wts = parse_worktree_porcelain(input);
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
        let wts = parse_worktree_porcelain(input);
        assert_eq!(wts.len(), 3);
        assert_eq!(wts[0].name, "main");
        assert_eq!(wts[1].name, "feature-x");
        assert_eq!(wts[2].name, "bugfix");
        assert_eq!(wts[1].branch.as_deref(), Some("feature-x"));
    }
}

/// Real-git fixtures shared by the scan and refresh tests.
#[cfg(test)]
pub(crate) mod test_git {
    use std::path::Path;

    /// Run git in `dir` with a throwaway identity. `false` when git is
    /// missing or the command fails.
    pub fn run(dir: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .map(|st| st.success())
            .unwrap_or(false)
    }

    /// A repository at `dir` with one commit. `false` when git is unavailable.
    pub fn init_repo(dir: &Path) -> bool {
        std::fs::create_dir_all(dir).is_ok()
            && run(dir, &["init", "-q"])
            && run(dir, &["commit", "--allow-empty", "-q", "-m", "init"])
    }
}
