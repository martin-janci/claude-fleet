//! Test fixtures for the changeset engine (Tasks 6–9): real git checkouts,
//! a store with reachable hosts, and a fake `ssh` that runs the remote
//! script locally under a chosen `HOME` — no process-wide env change.
//!
//! PF8: everything but [`item`] is reachable only from `#[cfg(unix)]` tests
//! (the fake `ssh` is a shell script), so it is gated the same way.

use super::{ItemAction, ItemParams};
#[cfg(unix)]
use crate::ssh::SshClient;
use crate::store::NewChangesetItem;
#[cfg(unix)]
use crate::store::{CatalogRow, Store};
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::{Arc, Mutex};

#[cfg(unix)]
pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::proc::std_command("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A catalog checkout at `root` with one commit.
#[cfg(unix)]
pub(crate) fn init_catalog(root: &Path) -> PathBuf {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "t@t"]);
    git(root, &["config", "user.name", "t"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "init"]);
    root.to_path_buf()
}

#[cfg(unix)]
pub(crate) fn head(root: &Path) -> String {
    git(root, &["rev-parse", "HEAD"])
}

#[cfg(unix)]
pub(crate) fn subjects(root: &Path) -> Vec<String> {
    git(root, &["log", "--format=%s"])
        .lines()
        .map(str::to_string)
        .collect()
}

/// `~/.claude/skills/<name>/SKILL.md` under `home`.
#[cfg(unix)]
pub(crate) fn host_skill(home: &Path, name: &str, description: &str) {
    let dir = home.join(".claude/skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\nSteps for {name}.\n"),
    )
    .unwrap();
}

/// A fake `ssh` (M3 R20's pattern): everything up to `-- <host>` is dropped
/// and the rest runs under `sh -c` with `HOME` set to `home`, as the remote
/// login shell would run it.
#[cfg(unix)]
pub(crate) fn ssh_with_home(bin_dir: &Path, home: &Path) -> Arc<SshClient> {
    ssh_with_home_running(bin_dir, home, "")
}

/// [`ssh_with_home`] that first runs `before` (a shell snippet) on every
/// remote call — how a test makes something happen mid-apply, between the
/// apply's checks and its writes (a foreign file, a foreign commit, a held
/// `index.lock`).
#[cfg(unix)]
pub(crate) fn ssh_with_home_running(bin_dir: &Path, home: &Path, before: &str) -> Arc<SshClient> {
    use crate::tmux::fake_exec::{write_exec, PROBE_GUARD};
    let bin = write_exec(
        bin_dir,
        "ssh",
        &format!(
            "#!/bin/sh\n{PROBE_GUARD}\
             case \"$*\" in *'-O check'*|*'-O exit'*) exit 0;; esac\n\
             {before}\n\
             while [ \"$#\" -gt 0 ] && [ \"$1\" != \"--\" ]; do shift; done\n\
             shift 2\n\
             HOME='{home}' exec sh -c \"$*\"\n",
            home = home.display()
        ),
    );
    Arc::new(SshClient::with_ssh_binary(bin))
}

/// A store with `personal` (a fresh checkout, loaded) and reachable hosts.
/// The store is an `Arc` so the scan tick's hook (`after_scan_pass`) can
/// take it too; everything else borrows it as `&Mutex<Store>`.
///
/// Each host serves `claude` only, pinned explicitly rather than left on
/// auto (F3a). The fake `ssh` runs the remote script on this machine, so
/// Codex's presence probe (`command -v codex`, `codex.rs`'s
/// `CODEX_PRESENT_PROBE`) reads the *test process's* `PATH` — on a
/// developer's box with the `codex` CLI installed the fixture host would
/// silently serve two harnesses and plan every asset twice, while CI
/// (no `codex` on `PATH`) serves one. `HOME` is already pinned to a
/// tempdir for the same reason; this pins the harness set. A test that
/// wants Codex asks for it with `set_host_harnesses`.
#[cfg(unix)]
pub(crate) struct Fleet {
    pub store: Arc<Mutex<Store>>,
    pub personal: CatalogRow,
    pub personal_root: PathBuf,
    dirs: Vec<tempfile::TempDir>,
}

#[cfg(unix)]
impl Fleet {
    pub(crate) fn new(hosts: &[&str]) -> Fleet {
        let dir = tempfile::tempdir().unwrap();
        let personal_root = init_catalog(&dir.path().join("personal"));
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config(&personal_root.to_string_lossy(), None)
            .unwrap();
        for h in hosts {
            s.insert_host(h, Some(h)).unwrap();
            s.update_host_probe(h, true, None, None, 1).unwrap();
            s.set_host_harnesses(h, Some(&["claude".to_string()]))
                .unwrap();
        }
        let personal = s.personal_catalog().unwrap().unwrap();
        let store = Arc::new(Mutex::new(s));
        crate::service::catalog::load_catalog(personal.id, false, &store).unwrap();
        Fleet {
            store,
            personal,
            personal_root,
            dirs: vec![dir],
        }
    }

    /// An org named `name` owning a catalog of the same name, loaded.
    pub(crate) fn add_org_catalog(&mut self, name: &str) -> (CatalogRow, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = init_catalog(&dir.path().join(name));
        let row = {
            let s = self.store.lock().unwrap();
            let org = s.add_org(name, None, false).unwrap();
            s.upsert_catalog(name, &root.to_string_lossy(), None, Some(org.id))
                .unwrap()
        };
        crate::service::catalog::load_catalog(row.id, false, &self.store).unwrap();
        self.dirs.push(dir);
        (row, root)
    }

    /// Write and commit `files` (relative path, content) in `root`, then
    /// reload catalog `id`.
    pub(crate) fn commit_files(&self, root: &Path, id: i64, files: &[(&str, &str)]) {
        for (rel, body) in files {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "seed"]);
        crate::service::catalog::load_catalog(id, false, &self.store).unwrap();
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn item(
    grp: &str,
    catalog_id: Option<i64>,
    kind: &str,
    name: &str,
    action: ItemAction,
    params: ItemParams,
) -> NewChangesetItem {
    NewChangesetItem {
        grp: grp.into(),
        catalog_id,
        kind: kind.into(),
        name: name.into(),
        action: action.as_str().into(),
        params: params.to_json(),
        decider: "rule".into(),
    }
}
