//! Test fixtures for the changeset engine (Tasks 6–9): real git checkouts,
//! a store with reachable hosts, and a fake `ssh` that runs the remote
//! script locally under a chosen `HOME` — no process-wide env change.
//!
//! PF8: everything but [`item`] is reachable only from `#[cfg(unix)]` tests
//! (the fake `ssh` is a shell script), so it is gated the same way.
//!
//! Assets M6 (Task 3): the card fixtures the apply, layer and reconcile
//! tests share — [`fleet_with_core`], [`new_card`], [`apply_all`],
//! [`skill_yaml`], [`person_syncs_oci`] — live here, not in one module's
//! tests.

#[cfg(unix)]
use super::apply::{apply, ApplyArgs};
#[cfg(unix)]
use super::ChangesetView;
use super::{ItemAction, ItemParams};
#[cfg(unix)]
use crate::ipc_error::IpcError;
#[cfg(unix)]
use crate::service::catalog::sync::{self, ApplyArgs as SyncApplyArgs, PlanArgs};
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
use tokio_util::sync::CancellationToken;

/// A skill description long enough for every check.
#[cfg(unix)]
pub(crate) const DESC: &str = "A reasonably long description here.";

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

    /// Commit `layers/<name>.yaml` in personal — a context layer holding
    /// `members` — and reload it.
    pub(crate) fn add_layer(&self, name: &str, members: &[&str]) {
        let list: String = members.iter().map(|m| format!("- {m}\n")).collect();
        let members = if list.is_empty() {
            "members: []\n".to_string()
        } else {
            format!("members:\n{list}")
        };
        self.commit_files(
            &self.personal_root,
            self.personal.id,
            &[(
                &format!("layers/{name}.yaml"),
                &format!("kind: layer\nname: {name}\naxis: context\n{members}"),
            )],
        );
    }

    /// Commit `mcp/<name>.yaml` (an http MCP server) and add it to layer
    /// `core`, next to skill `w`.
    pub(crate) fn add_mcp_server_to_core(&self, name: &str) {
        self.commit_files(
            &self.personal_root,
            self.personal.id,
            &[
                (
                    &format!("mcp/{name}.yaml"),
                    &format!(
                        "kind: mcp_server\nname: {name}\ndescription: {DESC}\n\
                         transport: http\nurl: https://example.com/mcp\n"
                    ),
                ),
                (
                    "layers/core.yaml",
                    &format!(
                        "kind: layer\nname: core\naxis: context\n\
                         members:\n- skill/w\n- mcp_server/{name}\n"
                    ),
                ),
            ],
        );
    }
}

/// `skills/<name>/asset.yaml` for a skill described by [`DESC`].
#[cfg(unix)]
pub(crate) fn skill_yaml(name: &str) -> String {
    format!("kind: skill\nname: {name}\ndescription: {DESC}\n")
}

/// A store with `w` in layer `core`, assigned to every host in `hosts`.
#[cfg(unix)]
pub(crate) fn fleet_with_core(hosts: &[&str]) -> Fleet {
    let f = Fleet::new(hosts);
    let p = f.personal.id;
    f.commit_files(
        &f.personal_root,
        p,
        &[
            ("skills/w/asset.yaml", skill_yaml("w").as_str()),
            ("skills/w/body.md", "Steps.\n"),
            (
                "layers/core.yaml",
                "kind: layer\nname: core\naxis: context\nmembers:\n- skill/w\n",
            ),
        ],
    );
    {
        let s = f.store.lock().unwrap();
        for h in hosts {
            s.set_host_layers_for(h, p, None, &["core"]).unwrap();
        }
    }
    f
}

/// A proposed `new` card holding `items`; answers its id.
#[cfg(unix)]
pub(crate) fn new_card(f: &Fleet, items: &[NewChangesetItem]) -> i64 {
    f.store
        .lock()
        .unwrap()
        .insert_changeset("new", "New", items)
        .unwrap()
        .id
}

/// Apply every pending item of card `id` (but "needs a look").
#[cfg(unix)]
pub(crate) async fn apply_all(
    f: &Fleet,
    id: i64,
    ssh: &Arc<SshClient>,
) -> Result<ChangesetView, IpcError> {
    apply(
        ApplyArgs {
            id,
            positions: None,
        },
        &f.store,
        ssh,
    )
    .await
}

/// A person's own sync of everything planned for `oci` — no card, so no
/// layer counts as rolled out by it.
#[cfg(unix)]
pub(crate) async fn person_syncs_oci(f: &Fleet, ssh: &Arc<SshClient>) {
    let planned = sync::plan_sync(
        PlanArgs {
            host_alias: Some("oci".into()),
            allow_unlayered: true,
            ..Default::default()
        },
        &f.store,
        ssh,
    )
    .await
    .unwrap();
    sync::apply_sync_with(
        SyncApplyArgs {
            plan_id: planned.id,
            force_partial: false,
            call_id: None,
        },
        &f.store,
        ssh,
        CancellationToken::new(),
    )
    .await
    .unwrap();
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
