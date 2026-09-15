//! The catalog repo on the controller: git operations via the `git` CLI,
//! and loading / writing IR assets on disk.

use super::model::{Asset, Kind, Problem, Resource};
use super::{E_ASSET_EXISTS, E_ASSET_NOT_FOUND, E_CATALOG_GIT, E_CATALOG_PARSE};
use crate::ipc_error::codes::E_INVALID;
use crate::ipc_error::IpcError;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Catalog {
    pub assets: Vec<Asset>,
    pub problems: Vec<Problem>,
    pub head: String,
    pub loaded_at: i64,
}

impl Catalog {
    pub fn find(&self, kind: Kind, name: &str) -> Option<&Asset> {
        self.assets
            .iter()
            .find(|a| a.kind() == kind && a.header.name == name)
    }
}

#[derive(serde::Deserialize)]
struct CatalogFile {
    schema_version: u64,
}

fn git(dir: &Path, args: &[&str]) -> Result<String, IpcError> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args).current_dir(dir);
    // Tests must not depend on (or be broken by) the host's own global git
    // config: isolate every git invocation the production code makes from
    // it. This has no effect on release builds — `git config user.email`
    // there still resolves the normal local -> global -> system chain, so a
    // real global identity is honoured and the claude-fleet fallback only
    // kicks in when git itself has none.
    #[cfg(test)]
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let out = cmd
        .output()
        .map_err(|e| IpcError::new(E_CATALOG_GIT, format!("spawn git: {e}")))?;
    if !out.status.success() {
        return Err(
            IpcError::new(E_CATALOG_GIT, format!("git {}: failed", args.join(" "))).with_details(
                serde_json::json!({ "stderr": String::from_utf8_lossy(&out.stderr).trim() }),
            ),
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The directory to run `git clone` from for a clone target of `path`: the
/// parent directory, or `.` when `path` is relative with no parent segment
/// (`Path::parent` returns `Some("")` for a single relative segment like
/// `foo`, not `None`, so that empty-string case must be normalised too).
fn clone_parent(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

/// Clone `remote` into `path` when `path` has no `.git`. With no remote, the
/// directory must already be a git repo.
pub fn ensure_repo(path: &Path, remote: Option<&str>) -> Result<(), IpcError> {
    if path.join(".git").exists() {
        return Ok(());
    }
    match remote {
        Some(url) => {
            let parent = clone_parent(path);
            std::fs::create_dir_all(parent)?;
            let target = path.to_string_lossy().to_string();
            git(parent, &["clone", "-q", url, &target])?;
            Ok(())
        }
        None => Err(IpcError::new(
            E_CATALOG_GIT,
            format!(
                "{} is not a git repository and no remote URL is configured",
                path.display()
            ),
        )),
    }
}

pub fn pull(path: &Path) -> Result<(), IpcError> {
    git(path, &["pull", "-q", "--ff-only"]).map(|_| ())
}

pub fn head(path: &Path) -> Result<String, IpcError> {
    git(path, &["rev-parse", "HEAD"])
}

/// Repo working-tree status: dirty file count plus ahead/behind versus the
/// upstream (`None` for both when there is no upstream).
#[derive(Debug, Clone, Serialize)]
pub struct RepoStatus {
    pub head: String,
    pub dirty: usize,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    pub has_upstream: bool,
}

/// Used by author.rs (Task 2): repo status for the toolbar / `status()`.
#[allow(dead_code)]
pub fn git_status(root: &Path) -> Result<RepoStatus, IpcError> {
    let head_sha = head(root)?;
    let porcelain = git(root, &["status", "--porcelain"])?;
    let dirty = porcelain.lines().filter(|l| !l.is_empty()).count();
    let has_upstream = git(root, &["rev-parse", "--abbrev-ref", "@{u}"]).is_ok();
    let (behind, ahead) = if has_upstream {
        let out = git(
            root,
            &["rev-list", "--left-right", "--count", "@{u}...HEAD"],
        )?;
        let mut parts = out.split_whitespace();
        let behind = parts.next().and_then(|s| s.parse::<u64>().ok());
        let ahead = parts.next().and_then(|s| s.parse::<u64>().ok());
        (behind, ahead)
    } else {
        (None, None)
    };
    Ok(RepoStatus {
        head: head_sha,
        dirty,
        ahead,
        behind,
        has_upstream,
    })
}

/// Whether git can resolve an identity for this repo (`git config
/// user.email` succeeds) — the normal local -> global -> system resolution,
/// so a user's own global identity is honoured and the claude-fleet fallback
/// only applies when git itself has none configured anywhere. In test
/// builds, `git()` isolates every invocation from the host's global/system
/// config (see its doc comment) so this is deterministic regardless of the
/// machine running the tests.
/// Used by author.rs (Task 2) as well as `commit` below.
#[allow(dead_code)]
pub fn has_identity(root: &Path) -> bool {
    git(root, &["config", "user.email"]).is_ok()
}

/// `git add -A -- <rel_paths>`, or `git add -A` for the whole tree when
/// `rel_paths` is empty. `rel_paths` are trusted here (author.rs, Task 2,
/// validates any path that originates from the frontend before it reaches
/// this function); the `--` before them still defuses flag injection (a
/// path that happens to start with `-`) regardless.
/// Used by author.rs (Task 2).
#[allow(dead_code)]
pub fn stage_paths(root: &Path, rel_paths: &[String]) -> Result<(), IpcError> {
    if rel_paths.is_empty() {
        git(root, &["add", "-A"]).map(|_| ())
    } else {
        let mut args: Vec<&str> = vec!["add", "-A", "--"];
        args.extend(rel_paths.iter().map(String::as_str));
        git(root, &args).map(|_| ())
    }
}

/// Commit whatever is currently staged (see `stage_paths`), falling back to
/// a synthetic identity when the repo has none configured locally. Returns
/// the new HEAD. `E_CATALOG_GIT` ("nothing to commit") when the working tree
/// has no changes at all.
/// Used by author.rs (Task 2).
#[allow(dead_code)]
pub fn commit(root: &Path, message: &str) -> Result<String, IpcError> {
    let porcelain = git(root, &["status", "--porcelain"])?;
    if porcelain.trim().is_empty() {
        return Err(IpcError::new(E_CATALOG_GIT, "nothing to commit"));
    }
    let mut args: Vec<&str> = Vec::new();
    if !has_identity(root) {
        args.extend([
            "-c",
            "user.name=claude-fleet",
            "-c",
            "user.email=fleet@localhost",
        ]);
    }
    args.extend(["commit", "-q", "-m", message]);
    git(root, &args)?;
    head(root)
}

/// `git push`; requires an existing upstream (git surfaces its own stderr
/// through the shared `git()` helper on failure).
/// Used by author.rs (Task 2).
#[allow(dead_code)]
pub fn push(root: &Path) -> Result<(), IpcError> {
    git(root, &["push"]).map(|_| ())
}

pub fn asset_path(root: &Path, kind: Kind, name: &str) -> PathBuf {
    if kind.is_folder() {
        root.join(kind.dir()).join(name).join("asset.yaml")
    } else {
        root.join(kind.dir()).join(format!("{name}.yaml"))
    }
}

fn body_file(kind: Kind) -> &'static str {
    match kind {
        Kind::Skill => "body.md",
        Kind::Agent => "prompt.md",
        _ => "",
    }
}

/// A `Resource.rel_path` is safe to join onto an asset dir and write only
/// when it starts with `resources/`, stays within `[A-Za-z0-9._/-]`, and has
/// no empty or `..` segment (which would otherwise let it escape the asset
/// directory or the `resources/` subtree). Defence in depth: `author.rs`
/// (Task 2) validates resource paths coming from the frontend too.
fn valid_resource_rel_path(rel_path: &str) -> bool {
    rel_path.starts_with("resources/")
        && rel_path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
        && rel_path
            .split('/')
            .all(|seg| !seg.is_empty() && seg != "..")
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .to_string()
}

fn read_resources(dir: &Path) -> std::io::Result<Vec<Resource>> {
    let mut out = Vec::new();
    let res = dir.join("resources");
    if !res.is_dir() {
        return Ok(out);
    }
    let mut stack = vec![res];
    while let Some(d) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&d)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let p = e.path();
            // `symlink_metadata` does not follow the link, so a symlink here
            // (including one that cycles back into an ancestor directory) is
            // detected and skipped rather than walked.
            let meta = std::fs::symlink_metadata(&p)?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(p);
            } else {
                out.push(Resource {
                    rel_path: rel(dir, &p),
                    bytes: std::fs::read(&p)?,
                });
            }
        }
    }
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

fn load_one(root: &Path, kind: Kind, yaml_path: &Path, stem: &str) -> Result<Asset, String> {
    let text = std::fs::read_to_string(yaml_path).map_err(|e| e.to_string())?;
    let mut asset = Asset::from_yaml(Some(kind), &text)?;
    if asset.header.name != stem {
        return Err(format!(
            "name '{}' does not match the file/folder stem '{stem}'",
            asset.header.name
        ));
    }
    let problems = asset.validate();
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    if kind.is_folder() {
        let dir = yaml_path.parent().unwrap_or(root);
        let body = dir.join(body_file(kind));
        asset.body =
            std::fs::read_to_string(&body).map_err(|_| format!("missing {}", body_file(kind)))?;
        asset.resources = read_resources(dir).map_err(|e| e.to_string())?;
    }
    Ok(asset)
}

/// Parse a catalog working tree. Never fails on a bad asset: those become
/// `problems`. Fails only when `catalog.yaml` is missing or has the wrong
/// schema version.
pub fn load_dir(root: &Path) -> Result<Catalog, IpcError> {
    let cat_file = root.join("catalog.yaml");
    let text = std::fs::read_to_string(&cat_file)
        .map_err(|e| IpcError::new(E_CATALOG_PARSE, format!("{}: {e}", cat_file.display())))?;
    let cf: CatalogFile = serde_yaml::from_str(&text)
        .map_err(|e| IpcError::new(E_CATALOG_PARSE, format!("catalog.yaml: {e}")))?;
    if cf.schema_version != SCHEMA_VERSION {
        return Err(IpcError::new(
            E_CATALOG_PARSE,
            format!(
                "catalog.yaml schema_version {} is not supported (want {SCHEMA_VERSION})",
                cf.schema_version
            ),
        ));
    }
    let mut cat = Catalog {
        loaded_at: super::now_secs(),
        ..Default::default()
    };
    for kind in Kind::ALL {
        let dir = root.join(kind.dir());
        if !dir.is_dir() {
            continue;
        }
        let mut entries: Vec<PathBuf> = match std::fs::read_dir(&dir) {
            Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).collect(),
            Err(e) => {
                cat.problems.push(Problem {
                    path: rel(root, &dir),
                    message: e.to_string(),
                });
                continue;
            }
        };
        entries.sort();
        for p in entries {
            let (yaml_path, stem) = if kind.is_folder() {
                if !p.is_dir() {
                    continue;
                }
                (
                    p.join("asset.yaml"),
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                )
            } else {
                if p.extension().and_then(|e| e.to_str()) != Some("yaml") {
                    continue;
                }
                (
                    p.clone(),
                    p.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                )
            };
            match load_one(root, kind, &yaml_path, &stem) {
                Ok(a) => cat.assets.push(a),
                Err(message) => cat.problems.push(Problem {
                    path: rel(root, &yaml_path),
                    message,
                }),
            }
        }
    }
    Ok(cat)
}

/// Write an asset into the working tree (asset.yaml + body + resources). On
/// `overwrite`, prunes files under `<dir>/resources/` that are no longer
/// listed in `asset.resources`, removing directories left empty.
pub fn write_asset(root: &Path, asset: &Asset, overwrite: bool) -> Result<(), IpcError> {
    for r in &asset.resources {
        if !valid_resource_rel_path(&r.rel_path) {
            return Err(IpcError::new(
                E_INVALID,
                format!("invalid resource path: {}", r.rel_path),
            ));
        }
    }
    let kind = asset.kind();
    let yaml_path = asset_path(root, kind, &asset.header.name);
    if yaml_path.exists() && !overwrite {
        return Err(IpcError::new(
            E_ASSET_EXISTS,
            format!(
                "{} {} already exists in the catalog",
                kind.as_str(),
                asset.header.name
            ),
        ));
    }
    let dir = yaml_path.parent().unwrap_or(root);
    std::fs::create_dir_all(dir)?;
    std::fs::write(&yaml_path, asset.to_yaml())?;
    if kind.is_folder() {
        std::fs::write(dir.join(body_file(kind)), &asset.body)?;
        for r in &asset.resources {
            let p = dir.join(&r.rel_path);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(p, &r.bytes)?;
        }
        if overwrite {
            let keep: HashSet<&str> = asset
                .resources
                .iter()
                .map(|r| r.rel_path.as_str())
                .collect();
            prune_resources(dir, &keep)?;
        }
    }
    Ok(())
}

/// Deletes files under `<dir>/resources/` whose rel_path (relative to `dir`,
/// e.g. `resources/x.txt`) is not in `keep`, then removes any directory
/// under `resources/` (`resources/` itself included) left empty. Symlinks
/// are left untouched (and count as occupying their parent), matching
/// `read_resources`'s treatment of them.
fn prune_resources(dir: &Path, keep: &HashSet<&str>) -> std::io::Result<()> {
    let res = dir.join("resources");
    if !res.is_dir() {
        return Ok(());
    }
    if prune_dir(&res, dir, keep)? {
        std::fs::remove_dir(&res)?;
    }
    Ok(())
}

/// Recursively prunes `current`, returning whether it ended up empty (so the
/// caller can remove it too).
fn prune_dir(current: &Path, root_dir: &Path, keep: &HashSet<&str>) -> std::io::Result<bool> {
    let mut is_empty = true;
    for entry in std::fs::read_dir(current)? {
        let p = entry?.path();
        let meta = std::fs::symlink_metadata(&p)?;
        if meta.file_type().is_symlink() {
            is_empty = false;
            continue;
        }
        if meta.is_dir() {
            if prune_dir(&p, root_dir, keep)? {
                std::fs::remove_dir(&p)?;
            } else {
                is_empty = false;
            }
        } else if keep.contains(rel(root_dir, &p).as_str()) {
            is_empty = false;
        } else {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(is_empty)
}

/// Repo-relative directory (folder kinds) or file path (single-file kinds)
/// for an asset: `"skills/<name>"` or `"hooks/<name>.yaml"`.
/// Used by author.rs (Task 2).
#[allow(dead_code)]
pub fn asset_rel_dir(kind: Kind, name: &str) -> String {
    if kind.is_folder() {
        format!("{}/{name}", kind.dir())
    } else {
        format!("{}/{name}.yaml", kind.dir())
    }
}

/// Deletes an asset's folder (folder kinds) or file (single-file kinds),
/// returning the repo-relative paths removed. `E_ASSET_NOT_FOUND` when the
/// asset does not exist on disk.
/// Used by author.rs (Task 2).
#[allow(dead_code)]
pub fn remove_asset(root: &Path, kind: Kind, name: &str) -> Result<Vec<String>, IpcError> {
    let not_found = || {
        IpcError::new(
            E_ASSET_NOT_FOUND,
            format!("{} {} not found in the catalog", kind.as_str(), name),
        )
    };
    if kind.is_folder() {
        let dir = root.join(kind.dir()).join(name);
        guard_removable(root, &dir, true, &not_found)?;
        let removed = list_files_rel(root, &dir)?;
        std::fs::remove_dir_all(&dir)?;
        Ok(removed)
    } else {
        let path = asset_path(root, kind, name);
        guard_removable(root, &path, false, &not_found)?;
        std::fs::remove_file(&path)?;
        Ok(vec![rel(root, &path)])
    }
}

/// Checks `path` is safe for `remove_asset` to delete: absent -> `not_found`
/// (via `E_ASSET_NOT_FOUND`); a symlink (whether it targets a directory,
/// file, or nothing) -> `E_INVALID`, never followed or unlinked; present but
/// the wrong type (a plain file where a directory was expected, or vice
/// versa) -> `not_found` as well. `symlink_metadata` (not `metadata`) is
/// essential here: it reports on the link itself rather than following it,
/// which is what lets a symlink be detected before anything is removed.
fn guard_removable(
    root: &Path,
    path: &Path,
    want_dir: bool,
    not_found: &dyn Fn() -> IpcError,
) -> Result<(), IpcError> {
    match std::fs::symlink_metadata(path) {
        Err(_) => Err(not_found()),
        Ok(meta) if meta.file_type().is_symlink() => Err(IpcError::new(
            E_INVALID,
            format!("{} is a symlink; refusing to remove", rel(root, path)),
        )),
        Ok(meta) if meta.is_dir() != want_dir => Err(not_found()),
        Ok(_) => Ok(()),
    }
}

/// All non-symlink files under `dir`, as paths relative to `root`, sorted.
fn list_files_rel(root: &Path, dir: &Path) -> std::io::Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let p = entry?.path();
            let meta = std::fs::symlink_metadata(&p)?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(p);
            } else {
                out.push(rel(root, &p));
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::model::{Asset, Kind, Resource};
    use std::fs;

    fn tmp(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("fleet-catalog-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn write(p: &std::path::Path, rel: &str, content: &str) {
        let f = p.join(rel);
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(f, content).unwrap();
    }

    #[test]
    fn load_dir_reads_every_kind_and_collects_problems() {
        let root = tmp("load");
        write(&root, "catalog.yaml", "schema_version: 1\nname: test\n");
        write(
            &root,
            "skills/worktree/asset.yaml",
            "kind: skill\nname: worktree\ndescription: d\n",
        );
        write(&root, "skills/worktree/body.md", "# body\n");
        write(&root, "skills/worktree/resources/scripts/go.sh", "echo\n");
        write(
            &root,
            "agents/pm/asset.yaml",
            "kind: agent\nname: pm\ndescription: d\n",
        );
        write(&root, "agents/pm/prompt.md", "prompt\n");
        write(&root, "hooks/stop.yaml", "kind: hook\nname: stop\ndescription: d\nevent: stop\naction: { type: command, command: x }\n");
        write(
            &root,
            "mcp/fleet.yaml",
            "kind: mcp_server\nname: fleet\ndescription: d\ntransport: http\nurl: u\n",
        );
        write(&root, "plugins/sp.yaml", "kind: plugin_ref\nname: sp\ndescription: d\nharness: claude\nmarketplace: { name: m, source: github, repo: o/r }\nplugin: sp\nversion: latest\n");
        write(&root, "hooks/bad.yaml", "kind: hook\nname: WRONG NAME\ndescription: d\nevent: stop\naction: { type: command, command: x }\n");
        write(&root, "mcp/garbage.yaml", ": : not yaml [\n");
        write(
            &root,
            "skills/mismatch/asset.yaml",
            "kind: skill\nname: other\ndescription: d\n",
        );

        let cat = load_dir(&root).unwrap();
        let names: Vec<(Kind, String)> = cat
            .assets
            .iter()
            .map(|a| (a.kind(), a.header.name.clone()))
            .collect();
        assert_eq!(
            names,
            vec![
                (Kind::Skill, "worktree".into()),
                (Kind::Agent, "pm".into()),
                (Kind::Hook, "stop".into()),
                (Kind::McpServer, "fleet".into()),
                (Kind::PluginRef, "sp".into()),
            ]
        );
        let skill = cat.find(Kind::Skill, "worktree").unwrap();
        assert_eq!(skill.body, "# body\n");
        assert_eq!(skill.resources.len(), 1);
        assert_eq!(skill.resources[0].rel_path, "resources/scripts/go.sh");
        assert_eq!(cat.find(Kind::Agent, "pm").unwrap().body, "prompt\n");
        assert_eq!(cat.problems.len(), 3, "{:?}", cat.problems);
        assert!(cat
            .problems
            .iter()
            .any(|p| p.path.ends_with("hooks/bad.yaml") && p.message.contains("name")));
        assert!(cat
            .problems
            .iter()
            .any(|p| p.path.ends_with("mcp/garbage.yaml")));
        assert!(cat
            .problems
            .iter()
            .any(|p| p.path.ends_with("skills/mismatch/asset.yaml") && p.message.contains("stem")));
    }

    #[test]
    fn load_dir_rejects_missing_or_wrong_schema() {
        let root = tmp("schema");
        let err = load_dir(&root).unwrap_err();
        assert_eq!(err.code, "E_CATALOG_PARSE");
        write(&root, "catalog.yaml", "schema_version: 99\n");
        assert_eq!(load_dir(&root).unwrap_err().code, "E_CATALOG_PARSE");
    }

    #[test]
    fn write_asset_round_trips_and_refuses_overwrite() {
        let root = tmp("write");
        write(&root, "catalog.yaml", "schema_version: 1\n");
        let mut a = Asset::from_yaml(None, "kind: skill\nname: s\ndescription: d\n").unwrap();
        a.body = "b\n".into();
        a.resources.push(Resource {
            rel_path: "resources/x.txt".into(),
            bytes: b"x".to_vec(),
        });
        write_asset(&root, &a, false).unwrap();
        assert!(root.join("skills/s/asset.yaml").exists());
        assert_eq!(
            fs::read_to_string(root.join("skills/s/body.md")).unwrap(),
            "b\n"
        );
        assert_eq!(
            fs::read(root.join("skills/s/resources/x.txt")).unwrap(),
            b"x"
        );
        let err = write_asset(&root, &a, false).unwrap_err();
        assert_eq!(err.code, "E_ASSET_EXISTS");
        write_asset(&root, &a, true).unwrap();
        let hook = Asset::from_yaml(None, "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n").unwrap();
        write_asset(&root, &hook, false).unwrap();
        assert!(root.join("hooks/h.yaml").exists());
        let cat = load_dir(&root).unwrap();
        assert_eq!(cat.assets.len(), 2);
        assert_eq!(cat.find(Kind::Skill, "s").unwrap().resources[0].bytes, b"x");
    }

    #[test]
    fn git_ensure_head_and_pull_on_a_local_repo() {
        let origin = tmp("origin");
        let run = |dir: &std::path::Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&origin, &["init", "-q", "-b", "main"]);
        run(&origin, &["config", "user.email", "t@t"]);
        run(&origin, &["config", "user.name", "t"]);
        write(&origin, "catalog.yaml", "schema_version: 1\n");
        run(&origin, &["add", "."]);
        run(&origin, &["commit", "-q", "-m", "init"]);

        let clone =
            std::env::temp_dir().join(format!("fleet-catalog-clone-{}", std::process::id()));
        let _ = fs::remove_dir_all(&clone);
        ensure_repo(&clone, Some(origin.to_str().unwrap())).unwrap();
        assert!(clone.join(".git").exists());
        let h1 = head(&clone).unwrap();
        assert_eq!(h1.len(), 40);
        ensure_repo(&clone, Some(origin.to_str().unwrap())).unwrap(); // idempotent

        write(&origin, "hooks/x.yaml", "kind: hook\nname: x\ndescription: d\nevent: stop\naction: { type: command, command: x }\n");
        run(&origin, &["add", "."]);
        run(&origin, &["commit", "-q", "-m", "two"]);
        pull(&clone).unwrap();
        assert_ne!(head(&clone).unwrap(), h1);

        let missing = std::env::temp_dir().join("fleet-catalog-does-not-exist");
        let _ = fs::remove_dir_all(&missing);
        let err = ensure_repo(&missing, None).unwrap_err();
        assert_eq!(err.code, "E_CATALOG_GIT");
    }

    #[test]
    fn clone_parent_normalises_relative_single_segment_paths() {
        assert_eq!(
            clone_parent(std::path::Path::new("foo")),
            std::path::Path::new(".")
        );
        assert_eq!(
            clone_parent(std::path::Path::new("/tmp/x/foo")),
            std::path::Path::new("/tmp/x")
        );
        assert_eq!(
            clone_parent(std::path::Path::new("/foo")),
            std::path::Path::new("/")
        );
    }

    #[test]
    fn load_dir_skips_symlinks_in_resources_and_terminates() {
        let root = tmp("symlink");
        write(&root, "catalog.yaml", "schema_version: 1\n");
        write(
            &root,
            "skills/x/asset.yaml",
            "kind: skill\nname: x\ndescription: d\n",
        );
        write(&root, "skills/x/body.md", "b\n");
        write(&root, "skills/x/resources/real.txt", "hi\n");
        // A symlink back up the tree: following it as a directory would
        // recurse forever. It must be skipped entirely, not walked.
        std::os::unix::fs::symlink("..", root.join("skills/x/resources/loop")).unwrap();

        let cat = load_dir(&root).unwrap();
        let skill = cat.find(Kind::Skill, "x").unwrap();
        assert_eq!(skill.resources.len(), 1);
        assert_eq!(skill.resources[0].rel_path, "resources/real.txt");
    }

    #[test]
    fn load_dir_records_problem_when_kind_dir_unreadable_and_continues() {
        use std::os::unix::fs::PermissionsExt;
        let root = tmp("unreadable");
        write(&root, "catalog.yaml", "schema_version: 1\n");
        write(
            &root,
            "agents/pm/asset.yaml",
            "kind: agent\nname: pm\ndescription: d\n",
        );
        write(&root, "agents/pm/prompt.md", "prompt\n");
        let hooks_dir = root.join("hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        fs::set_permissions(&hooks_dir, fs::Permissions::from_mode(0o000)).unwrap();

        let result = load_dir(&root);

        // Restore permissions unconditionally so tmp cleanup on a later run
        // (and this test's own `tmp()` helper) can remove the directory.
        fs::set_permissions(&hooks_dir, fs::Permissions::from_mode(0o755)).unwrap();

        let cat = result.unwrap();
        assert_eq!(cat.assets.len(), 1);
        assert_eq!(cat.find(Kind::Agent, "pm").unwrap().body, "prompt\n");
        assert!(
            cat.problems.iter().any(|p| p.path.ends_with("hooks")),
            "{:?}",
            cat.problems
        );
    }

    fn git_run(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo(root: &std::path::Path) {
        git_run(root, &["init", "-q", "-b", "main"]);
    }

    fn commit_author_email(root: &std::path::Path) -> String {
        let out = std::process::Command::new("git")
            .args(["log", "-1", "--format=%ae"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn commit_uses_fallback_identity_when_unset() {
        let root = tmp("commit-fallback");
        init_repo(&root);
        write(&root, "catalog.yaml", "schema_version: 1\n");
        assert!(!has_identity(&root));

        stage_paths(&root, &[]).unwrap();
        let head_sha = commit(&root, "catalog: init").unwrap();
        assert_eq!(head_sha.len(), 40);
        assert_eq!(commit_author_email(&root), "fleet@localhost");
    }

    #[test]
    fn commit_keeps_configured_identity() {
        let root = tmp("commit-configured");
        init_repo(&root);
        git_run(&root, &["config", "user.email", "dev@example.com"]);
        git_run(&root, &["config", "user.name", "Dev"]);
        write(&root, "catalog.yaml", "schema_version: 1\n");
        assert!(has_identity(&root));

        stage_paths(&root, &[]).unwrap();
        commit(&root, "catalog: init").unwrap();
        assert_eq!(commit_author_email(&root), "dev@example.com");
    }

    #[test]
    fn stage_paths_then_commit_returns_head() {
        let root = tmp("stage-commit");
        init_repo(&root);
        git_run(&root, &["config", "user.email", "dev@example.com"]);
        git_run(&root, &["config", "user.name", "Dev"]);
        write(&root, "catalog.yaml", "schema_version: 1\n");
        write(&root, "hooks/keep.yaml", "kind: hook\n");
        write(&root, "hooks/ignored.yaml", "kind: hook\n");

        stage_paths(
            &root,
            &["catalog.yaml".to_string(), "hooks/keep.yaml".to_string()],
        )
        .unwrap();
        let head_sha = commit(&root, "catalog: partial").unwrap();
        assert_eq!(head_sha.len(), 40);
        assert_eq!(head(&root).unwrap(), head_sha);

        // Only the staged paths were committed; the unstaged file is still
        // untracked, so the tree remains dirty.
        let status = git_status(&root).unwrap();
        assert_eq!(status.dirty, 1);

        // Nothing left to stage/commit for the already-committed paths.
        let err = commit(&root, "catalog: nothing").unwrap_err();
        assert_eq!(err.code, "E_CATALOG_GIT");
    }

    #[test]
    fn git_status_counts_dirty_and_ahead() {
        let root = tmp("status");
        init_repo(&root);
        git_run(&root, &["config", "user.email", "dev@example.com"]);
        git_run(&root, &["config", "user.name", "Dev"]);
        write(&root, "catalog.yaml", "schema_version: 1\n");
        stage_paths(&root, &[]).unwrap();
        commit(&root, "catalog: init").unwrap();

        let status = git_status(&root).unwrap();
        assert!(!status.has_upstream);
        assert_eq!(status.ahead, None);
        assert_eq!(status.behind, None);
        assert_eq!(status.dirty, 0);

        write(&root, "hooks/x.yaml", "kind: hook\n");
        let status = git_status(&root).unwrap();
        assert_eq!(status.dirty, 1);

        let remote = tmp("status-remote");
        git_run(&remote, &["init", "-q", "--bare", "-b", "main"]);
        git_run(
            &root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git_run(&root, &["push", "-q", "-u", "origin", "main"]);

        stage_paths(&root, &[]).unwrap();
        commit(&root, "catalog: second").unwrap();
        let status = git_status(&root).unwrap();
        assert!(status.has_upstream);
        assert_eq!(status.ahead, Some(1));
        assert_eq!(status.behind, Some(0));
        assert_eq!(status.dirty, 0);
    }

    #[test]
    fn remove_asset_deletes_folder_and_file_kinds() {
        let root = tmp("remove");
        write(
            &root,
            "skills/s/asset.yaml",
            "kind: skill\nname: s\ndescription: d\n",
        );
        write(&root, "skills/s/body.md", "b\n");
        write(&root, "skills/s/resources/x.txt", "x");
        write(&root, "hooks/h.yaml", "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n");

        let mut removed = remove_asset(&root, Kind::Skill, "s").unwrap();
        removed.sort();
        assert!(!root.join("skills/s").exists());
        assert_eq!(
            removed,
            vec![
                "skills/s/asset.yaml".to_string(),
                "skills/s/body.md".to_string(),
                "skills/s/resources/x.txt".to_string(),
            ]
        );

        let removed = remove_asset(&root, Kind::Hook, "h").unwrap();
        assert!(!root.join("hooks/h.yaml").exists());
        assert_eq!(removed, vec!["hooks/h.yaml".to_string()]);

        let err = remove_asset(&root, Kind::Skill, "missing").unwrap_err();
        assert_eq!(err.code, "E_ASSET_NOT_FOUND");
        let err = remove_asset(&root, Kind::Hook, "missing").unwrap_err();
        assert_eq!(err.code, "E_ASSET_NOT_FOUND");
    }

    #[test]
    fn remove_asset_refuses_symlinked_targets() {
        let root = tmp("remove-symlink");
        let outside = tmp("remove-symlink-outside");
        write(&outside, "keep.txt", "keep");

        // A folder-kind asset whose directory is actually a symlink out of
        // the repo: must be refused, never unlinked or walked into.
        fs::create_dir_all(root.join("skills")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("skills/link")).unwrap();
        let err = remove_asset(&root, Kind::Skill, "link").unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        assert!(root.join("skills/link").exists());
        assert!(outside.join("keep.txt").exists());

        // Same guard for a single-file kind whose yaml is a symlink.
        let outside_file = outside.join("keep.txt");
        fs::create_dir_all(root.join("hooks")).unwrap();
        std::os::unix::fs::symlink(&outside_file, root.join("hooks/h.yaml")).unwrap();
        let err = remove_asset(&root, Kind::Hook, "h").unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        assert!(root.join("hooks/h.yaml").exists());
        assert!(outside_file.exists());
    }

    #[test]
    fn write_asset_rejects_invalid_resource_paths() {
        let root = tmp("invalid-resource");
        let mut a = Asset::from_yaml(None, "kind: skill\nname: s\ndescription: d\n").unwrap();
        a.body = "b\n".into();

        for bad in [
            "not-resources/x.txt",     // must live under resources/
            "resources/../escape.txt", // .. segment
            "resources//x.txt",        // empty segment
            "resources/x y.txt",       // space: outside [A-Za-z0-9._/-]
            "resources/héllo.txt",     // non-ASCII: outside [A-Za-z0-9._/-]
        ] {
            a.resources = vec![Resource {
                rel_path: bad.into(),
                bytes: b"x".to_vec(),
            }];
            let err = write_asset(&root, &a, false).unwrap_err();
            assert_eq!(err.code, "E_INVALID", "path: {bad}");
        }
        // None of the rejected writes touched disk.
        assert!(!root.join("skills/s").exists());

        a.resources = vec![Resource {
            rel_path: "resources/ok.txt".into(),
            bytes: b"ok".to_vec(),
        }];
        write_asset(&root, &a, false).unwrap();
        assert!(root.join("skills/s/resources/ok.txt").exists());
    }

    #[test]
    fn write_asset_overwrite_prunes_stale_resources() {
        let root = tmp("prune");
        write(&root, "catalog.yaml", "schema_version: 1\n");
        let mut a = Asset::from_yaml(None, "kind: skill\nname: s\ndescription: d\n").unwrap();
        a.body = "b\n".into();
        a.resources = vec![
            Resource {
                rel_path: "resources/keep.txt".into(),
                bytes: b"keep".to_vec(),
            },
            Resource {
                rel_path: "resources/sub/drop.txt".into(),
                bytes: b"drop".to_vec(),
            },
        ];
        write_asset(&root, &a, false).unwrap();
        assert!(root.join("skills/s/resources/sub/drop.txt").exists());

        a.resources = vec![Resource {
            rel_path: "resources/keep.txt".into(),
            bytes: b"keep2".to_vec(),
        }];
        write_asset(&root, &a, true).unwrap();

        assert!(root.join("skills/s/resources/keep.txt").exists());
        assert!(!root.join("skills/s/resources/sub/drop.txt").exists());
        assert!(!root.join("skills/s/resources/sub").exists());

        let cat = load_dir(&root).unwrap();
        let skill = cat.find(Kind::Skill, "s").unwrap();
        assert_eq!(skill.resources.len(), 1);
        assert_eq!(skill.resources[0].rel_path, "resources/keep.txt");
        assert_eq!(
            fs::read(root.join("skills/s/resources/keep.txt")).unwrap(),
            b"keep2"
        );
    }
}
