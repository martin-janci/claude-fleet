//! The catalog repo on the controller: git operations via the `git` CLI,
//! and loading / writing IR assets on disk.

use super::model::{Asset, Kind, Problem, Resource};
use super::{E_ASSET_EXISTS, E_CATALOG_GIT, E_CATALOG_PARSE};
use crate::ipc_error::IpcError;
use serde::Serialize;
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
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
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

/// Write an asset into the working tree (asset.yaml + body + resources).
pub fn write_asset(root: &Path, asset: &Asset, overwrite: bool) -> Result<(), IpcError> {
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
    }
    Ok(())
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
}
