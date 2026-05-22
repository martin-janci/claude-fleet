use crate::ipc_error::IpcError;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    /// True when `git worktree list` flagged this entry `prunable` — its
    /// working tree is gone or unusable.
    pub is_prunable: bool,
}

/// Walks `base/<owner>/<repo>` two levels deep and returns every directory
/// that contains a `.git` entry (regular dir or worktree gitfile).
pub fn scan_projects(base: &Path) -> Result<Vec<DiscoveredProject>, IpcError> {
    let mut out = Vec::new();
    if !base.exists() {
        return Ok(out);
    }
    for owner_entry in std::fs::read_dir(base)? {
        let owner_entry = owner_entry?;
        if !owner_entry.file_type()?.is_dir() {
            continue;
        }
        let owner = owner_entry.file_name().to_string_lossy().into_owned();
        if owner.starts_with('.') {
            continue;
        }
        for repo_entry in std::fs::read_dir(owner_entry.path())? {
            let repo_entry = repo_entry?;
            if !repo_entry.file_type()?.is_dir() {
                continue;
            }
            let repo = repo_entry.file_name().to_string_lossy().into_owned();
            if repo.starts_with('.') {
                continue;
            }
            let path = repo_entry.path();
            if path.join(".git").exists() {
                out.push(DiscoveredProject {
                    owner: owner.clone(),
                    repo,
                    base_path: path,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        (a.owner.as_str(), a.repo.as_str()).cmp(&(b.owner.as_str(), b.repo.as_str()))
    });
    Ok(out)
}

/// Runs `git worktree list --porcelain` in `repo_path` and parses the result.
/// The main checkout is normalized to `name = "main"`; extras use the dir name.
pub fn list_worktrees(repo_path: &Path) -> Result<Vec<DiscoveredWorktree>, IpcError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["worktree", "list", "--porcelain"])
        .output()
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
    let mut cur_prunable = false;
    for line in input.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(path) = cur_path.take() {
                out.push(make_worktree(
                    path,
                    cur_branch.take(),
                    cur_prunable,
                    main_path,
                ));
            }
            cur_path = Some(PathBuf::from(rest));
            cur_branch = None;
            cur_prunable = false;
        } else if let Some(rest) = line.strip_prefix("branch ") {
            cur_branch = Some(rest.trim_start_matches("refs/heads/").to_string());
        } else if line == "prunable" || line.starts_with("prunable ") {
            cur_prunable = true;
        }
    }
    if let Some(path) = cur_path {
        out.push(make_worktree(path, cur_branch, cur_prunable, main_path));
    }
    out
}

fn make_worktree(
    path: PathBuf,
    branch: Option<String>,
    is_prunable: bool,
    main_path: &Path,
) -> DiscoveredWorktree {
    let name = if path == main_path {
        "main".to_string()
    } else {
        path.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".to_string())
    };
    DiscoveredWorktree {
        name,
        path,
        branch,
        is_prunable,
    }
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
        let projects = scan_projects(tmp.path()).unwrap();
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
        let projects = scan_projects(tmp.path()).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].repo, "real-repo");
    }

    #[test]
    fn scan_returns_empty_for_missing_base() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let projects = scan_projects(&missing).unwrap();
        assert!(projects.is_empty());
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

    #[test]
    fn parse_marks_prunable_worktree() {
        let main = Path::new("/repo");
        let input = "worktree /repo\nHEAD aaaa\nbranch refs/heads/main\n\n\
                     worktree /repo/.worktrees/gone\nHEAD bbbb\nbranch refs/heads/gone\n\
                     prunable gitdir file points to non-existent location\n\n";
        let wts = parse_worktree_porcelain(input, main);
        assert_eq!(wts.len(), 2);
        assert_eq!(wts[0].name, "main");
        assert!(!wts[0].is_prunable);
        assert_eq!(wts[1].name, "gone");
        assert!(wts[1].is_prunable);
        assert_eq!(wts[1].branch.as_deref(), Some("gone"));
    }
}
