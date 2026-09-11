//! Prune stale REMOTE worktree rows.
//!
//! A remote host's worktree rows come from its `EnterWorktree` hooks
//! (migration 024), and only its `ExitWorktree { action: "remove" }` hook
//! deletes them. A worktree removed any other way (`git worktree remove` by
//! hand, `rm -rf`, a session killed while in the worktree) leaves its row
//! behind, and `HostPaths` would keep linking cwds to it.
//!
//! On the background tick, at most once every [`PRUNE_INTERVAL_SECS`], each
//! reachable, non-hidden remote host that has worktree rows gets ONE
//! batched, read-only ssh script: `git -C <repo root> worktree list
//! --porcelain` for each repo root the rows belong to, and `test -e` for each
//! row path. A row is deleted only when BOTH hold: its path is gone, and its
//! repo root listed successfully (non-empty) without registering it.
//!
//! **Spellings.** A hook stores the path as Claude reported it; git lists
//! each worktree as it was added. Through a symlinked root
//! (`~/projects -> /mnt/sda4/projects`) the two can differ in EITHER
//! direction: Claude reports physical cwds, while fleet's remote
//! `git worktree add` runs from the logical `$HOME` root, so git may keep
//! the logical spelling. A raw string compare would call a registered
//! worktree unregistered. Both sides are therefore resolved ON THE HOST and
//! compared symmetrically. Every listed entry is printed as listed and with
//! its parent resolved by `pwd -P`; a missing row's parent is resolved the
//! same way, plus its leaf name. Then every entry and every row spelling is
//! also taken under the other spelling of its repo root (logical to
//! physical and physical to logical, via the root's `pwd -P`). This covers a
//! row whose parent is gone too, where git is the only authority. A row is
//! stale only when no spelling of it matches any spelling of git's entries.
//!
//! **Unknown means keep.** An unreachable host (ssh exit 255), a failed or
//! empty listing, a `$HOME` that cannot be resolved, output that does not
//! parse or is cut short: nothing is deleted.
//!
//! **Race.** The probe's start time is recorded before it runs. Each row is
//! re-checked under the store lock before deletion, and skipped when it
//! changed (host, path) or was written after the probe started
//! (`updated_at_ms`, migration 026): a hook re-created the worktree while the
//! probe ran, so it is live. Deletion goes through
//! [`Store::delete_worktree`], which clears (and announces) the sessions
//! still pointing at the row and drops its parent fingerprint.
//!
//! **Not covered.** Only rows whose repository is checked out at the
//! standard layout path on the host (`<projects root>/<owner>/<repo>`, or
//! `<projects root>/<repo>` for the flat layout) can be judged. A row whose
//! repository lives anywhere else, such as a worktree of a checkout outside
//! the layout, gets a failed listing, which reads as unknown: it is never
//! pruned here.
//!
//! Local rows are never touched here: the local project refresh owns them.

use crate::ipc_error::IpcError;
use crate::projects::Layout;
use crate::service::projects::{expand_home, layout, project_base_for, LOCAL_HOST};
use crate::shell::quote;
use crate::ssh::{SshClient, SshExec};
use crate::store::{ProjectRow, Store, WorktreeRow};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the tick prunes, in seconds. One ssh call per remote host that
/// has worktree rows.
pub const PRUNE_INTERVAL_SECS: u64 = 900;

/// Wall-clock cap on one host's probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Printed last by the probe; output without it was cut short.
const DONE_MARKER: &str = "@@fleet_wtprune_done";

/// One host's rows to check, each with the index of its repo root.
#[derive(Debug, Clone, Default)]
pub struct HostPlan {
    /// Repo roots on the host, one `git worktree list` each.
    pub roots: Vec<String>,
    /// `(row, index into roots)`.
    pub rows: Vec<(WorktreeRow, usize)>,
}

/// Pure: group a host's rows by the repo root of their project on that host
/// (`root` is the host's expanded projects root). A row whose project is gone
/// is left out, so it is never judged.
pub fn plan(
    rows: Vec<WorktreeRow>,
    projects: &[ProjectRow],
    root: &str,
    layout: Layout,
) -> HostPlan {
    let by_id: HashMap<i64, &ProjectRow> = projects.iter().map(|p| (p.id, p)).collect();
    let mut out = HostPlan::default();
    for row in rows {
        let Some(p) = by_id.get(&row.project_id) else {
            continue;
        };
        let repo_root = layout.project_dir(root, &p.owner, &p.repo);
        let idx = match out.roots.iter().position(|r| *r == repo_root) {
            Some(i) => i,
            None => {
                out.roots.push(repo_root);
                out.roots.len() - 1
            }
        };
        out.rows.push((row, idx));
    }
    out
}

/// The read-only probe script for one host: every root and path `quote`d.
/// Spellings are resolved on the host (see the module docs): each root
/// prints its `pwd -P`; each listed worktree is printed as git listed it and
/// again with its parent resolved; each missing row prints its parent
/// resolved plus its leaf name, when the parent exists.
pub fn probe_script(plan: &HostPlan) -> String {
    let mut s = String::new();
    for (i, root) in plan.roots.iter().enumerate() {
        s.push_str(&format!(
            "r={root}; echo '@@root {i}'\n\
             if c=$(cd -- \"$r\" 2>/dev/null && pwd -P); then \
             printf '@@rootcanon {i} %s\\n' \"$c\"; fi\n\
             git -C \"$r\" worktree list --porcelain 2>/dev/null | grep '^worktree ' \
             | while IFS= read -r l; do p=${{l#worktree }}; printf 'worktree %s\\n' \"$p\"; \
             if c=$(cd -- \"${{p%/*}}\" 2>/dev/null && pwd -P); then \
             printf 'worktree %s/%s\\n' \"$c\" \"${{p##*/}}\"; fi; done; \
             echo \"@@rootrc {i} ${{PIPESTATUS[0]}}\"\n",
            root = quote(root),
        ));
    }
    for (j, (row, _)) in plan.rows.iter().enumerate() {
        s.push_str(&format!(
            "p={p}; if [ -e \"$p\" ]; then echo '@@present {j}'; else echo '@@missing {j}'; \
             if c=$(cd -- \"${{p%/*}}\" 2>/dev/null && pwd -P); then \
             printf '@@canon {j} %s/%s\\n' \"$c\" \"${{p##*/}}\"; fi; fi\n",
            p = quote(&row.path),
        ));
    }
    s.push_str(&format!("echo '{DONE_MARKER}'\n"));
    s
}

/// What one host's probe found.
#[derive(Debug, Default, PartialEq)]
pub struct Probe {
    /// Per root: the registered worktree paths in every spelling the host
    /// printed (as listed, and with the parent resolved), or `None` when the
    /// listing failed or came back empty (unknown; git always lists the main
    /// checkout, so an empty success is not trusted either).
    pub registered: Vec<Option<HashSet<String>>>,
    /// Per root: its physical spelling (`pwd -P`), when it exists.
    pub root_canon: Vec<Option<String>>,
    /// Per row: whether its path exists.
    pub present: Vec<bool>,
    /// Per missing row: its path with the parent resolved on the host, when
    /// the parent exists.
    pub canon: Vec<Option<String>>,
}

fn norm(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        "/".to_string()
    } else {
        t.to_string()
    }
}

/// Pure: parse the probe's output. `None` when it is incomplete or does not
/// fit the plan (the whole host is then unknown). Lines that are not the
/// probe's own (a login banner) are ignored.
pub fn parse_probe(stdout: &str, n_roots: usize, n_rows: usize) -> Option<Probe> {
    let mut registered: Vec<Option<HashSet<String>>> = vec![None; n_roots];
    let mut root_canon: Vec<Option<String>> = vec![None; n_roots];
    let mut listed = vec![false; n_roots];
    let mut present: Vec<Option<bool>> = vec![None; n_rows];
    let mut canon: Vec<Option<String>> = vec![None; n_rows];
    let mut current: Option<(usize, HashSet<String>)> = None;
    let mut done = false;
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("@@rootcanon ") {
            let (i, p) = rest.split_once(' ')?;
            let i: usize = i.parse().ok()?;
            *root_canon.get_mut(i)? = Some(norm(p));
        } else if let Some(rest) = line.strip_prefix("@@canon ") {
            let (j, p) = rest.split_once(' ')?;
            let j: usize = j.parse().ok()?;
            *canon.get_mut(j)? = Some(norm(p));
        } else if let Some(i) = line.strip_prefix("@@root ") {
            let i: usize = i.trim().parse().ok()?;
            if i >= n_roots {
                return None;
            }
            current = Some((i, HashSet::new()));
        } else if let Some(rest) = line.strip_prefix("@@rootrc ") {
            let (i, rc) = rest.split_once(' ')?;
            let i: usize = i.parse().ok()?;
            let rc: i32 = rc.trim().parse().ok()?;
            let (ci, set) = current.take()?;
            if ci != i {
                return None;
            }
            listed[i] = true;
            if rc == 0 && !set.is_empty() {
                registered[i] = Some(set);
            }
        } else if let Some(p) = line.strip_prefix("worktree ") {
            if let Some((_, set)) = current.as_mut() {
                set.insert(norm(p));
            }
        } else if let Some(j) = line.strip_prefix("@@present ") {
            let j: usize = j.trim().parse().ok()?;
            *present.get_mut(j)? = Some(true);
        } else if let Some(j) = line.strip_prefix("@@missing ") {
            let j: usize = j.trim().parse().ok()?;
            *present.get_mut(j)? = Some(false);
        } else if line.trim() == DONE_MARKER {
            done = true;
        }
    }
    if !done || listed.iter().any(|l| !l) {
        return None;
    }
    let present = present.into_iter().collect::<Option<Vec<bool>>>()?;
    Some(Probe {
        registered,
        root_canon,
        present,
        canon,
    })
}

/// Pure: the rows to delete. A row goes only when its path is gone AND its
/// repo root listed (successfully, non-empty) without registering it under
/// ANY spelling. The comparison is symmetric ([`both_spellings`]): every
/// entry git listed, and every spelling of the row (as stored, and with its
/// parent resolved on the host when the parent exists), is taken in both the
/// logical and the physical spelling of the repo root. So a row stored under
/// either spelling matches a worktree git registered under either.
pub fn stale_rows(plan: &HostPlan, probe: &Probe) -> Vec<WorktreeRow> {
    // Each root's registered entries in both root spellings, built once.
    let registered: Vec<Option<HashSet<String>>> = probe
        .registered
        .iter()
        .enumerate()
        .map(|(i, set)| {
            let root = plan.roots.get(i)?;
            let rc = probe.root_canon.get(i).cloned().flatten();
            set.as_ref().map(|set| {
                set.iter()
                    .flat_map(|e| both_spellings(e, root, rc.as_deref()))
                    .collect()
            })
        })
        .collect();
    plan.rows
        .iter()
        .enumerate()
        .filter_map(|(j, (row, root))| {
            if *probe.present.get(j)? {
                return None;
            }
            let registered = registered.get(*root)?.as_ref()?;
            let root_path = plan.roots.get(*root)?;
            let rc = probe.root_canon.get(*root).cloned().flatten();
            let mut spellings = both_spellings(&row.path, root_path, rc.as_deref());
            if let Some(c) = probe.canon.get(j).cloned().flatten() {
                spellings.extend(both_spellings(&c, root_path, rc.as_deref()));
            }
            (!spellings.iter().any(|p| registered.contains(p))).then(|| row.clone())
        })
        .collect()
}

/// A path in both spellings of its repo root: as given, plus, when it lies
/// under one spelling, the same path under the other. That means the logical
/// `root` prefix swapped for the physical `root_canon`, or the reverse. Git
/// keeps a worktree under whichever spelling it was added from (fleet adds
/// from the logical `$HOME` root, while Claude reports physical cwds), so
/// both sides of the comparison go through this. Without `root_canon` (the
/// root itself is gone) only the path as given.
fn both_spellings(path: &str, root: &str, root_canon: Option<&str>) -> Vec<String> {
    let mut out = vec![norm(path)];
    let Some(rc) = root_canon else {
        return out;
    };
    let swap = |from: &str, to: &str| {
        crate::service::projects::strip_root(path, from).map(|rest| {
            if rest.is_empty() {
                norm(to)
            } else {
                norm(&format!("{}/{rest}", to.trim_end_matches('/')))
            }
        })
    };
    for p in [swap(root, rc), swap(rc, root)].into_iter().flatten() {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Probe one remote host and delete its stale worktree rows; returns the ids
/// deleted. `Err` means the probe could not answer (unreachable, failed,
/// unreadable): nothing is deleted.
pub async fn prune_host(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    host: &str,
) -> Result<Vec<i64>, IpcError> {
    if host == LOCAL_HOST {
        return Ok(Vec::new());
    }
    // The race guard's reference point, taken before anything is read: a row
    // written after it was (re-)created while this probe ran, so it is live.
    let probe_start_ms = unix_ms_now();
    let (rows, projects, base, layout) = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        (
            s.list_worktrees_on_host(host)?,
            s.list_projects()?,
            project_base_for(&s, host),
            layout(&s),
        )
    };
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let home = ssh.remote_home(host).await?;
    let plan = plan(rows, &projects, &expand_home(&base, &home), layout);
    if plan.rows.is_empty() {
        return Ok(Vec::new());
    }
    let script = probe_script(&plan);
    let out = ssh
        .run(host, &["bash", "-lc", &quote(&script)], PROBE_TIMEOUT)
        .await?;
    if !out.status.success() {
        return Err(IpcError::new(
            "E_SSH",
            format!(
                "worktree probe on {host} exited {:?}; nothing pruned",
                out.status.code()
            ),
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let Some(probe) = parse_probe(&stdout, plan.roots.len(), plan.rows.len()) else {
        return Err(IpcError::new(
            "E_PARSE",
            format!("worktree probe on {host} was incomplete; nothing pruned"),
        ));
    };
    // Fingerprint keys are computed before the lock, as every delete caller
    // does. A remote row's key is its stored path only, never resolved on
    // this machine, so this touches no filesystem.
    let keyed: Vec<(WorktreeRow, Vec<String>)> = stale_rows(&plan, &probe)
        .into_iter()
        .map(|row| {
            let keys = Store::fingerprint_keys(&row.host_alias, &row.path);
            (row, keys)
        })
        .collect();
    let s = store.lock().map_err(|_| IpcError::lock())?;
    let mut deleted = Vec::new();
    for (row, keys) in keyed {
        // Re-check under the lock: a row moved (host, path) or written after
        // the probe started is not the one the probe judged. A hook that
        // re-created the worktree meanwhile stamped it (`updated_at_ms`).
        let unchanged = s
            .get_worktree_row(row.id)?
            .is_some_and(|r| r.host_alias == host && r.path == row.path);
        let rewritten = s
            .worktree_updated_at_ms(row.id)?
            .is_some_and(|ms| ms > probe_start_ms);
        if unchanged && !rewritten && s.delete_worktree(row.id, &keys)?.is_some() {
            deleted.push(row.id);
        }
    }
    Ok(deleted)
}

/// Now, in unix milliseconds: the same clock `Store::upsert_worktree_on`
/// stamps `updated_at_ms` with.
fn unix_ms_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// One run over every reachable, non-hidden remote host that has worktree
/// rows, one host at a time. Returns `(hosts that answered, rows deleted)`.
pub async fn run_with(store: &Mutex<Store>, ssh: &dyn SshExec) -> (usize, usize) {
    let hosts: Vec<String> = {
        let Ok(s) = store.lock() else {
            return (0, 0);
        };
        s.list_hosts()
            .unwrap_or_default()
            .into_iter()
            .filter(|h| h.alias != LOCAL_HOST && h.reachable && !h.hidden)
            .map(|h| h.alias)
            .collect()
    };
    let (mut answered, mut deleted) = (0, 0);
    for host in hosts {
        match prune_host(store, ssh, &host).await {
            Ok(ids) => {
                answered += 1;
                if !ids.is_empty() {
                    tracing::info!(
                        "[worktree-prune] {host}: removed {} stale worktree row(s)",
                        ids.len()
                    );
                }
                deleted += ids.len();
            }
            Err(e) => tracing::debug!("[worktree-prune] {host}: {e}"),
        }
    }
    (answered, deleted)
}

/// Resets the in-flight flag even if the run panics.
struct InFlight(&'static AtomicBool);
impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Tick entry point: when due and no earlier run is still going, start one
/// run in the background and return `true`. Detached, so a slow host never
/// delays the reconcile loop.
pub fn maybe_run(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) -> bool {
    static LAST: once_cell::sync::Lazy<Mutex<Option<std::time::Instant>>> =
        once_cell::sync::Lazy::new(|| Mutex::new(None));
    static RUNNING: AtomicBool = AtomicBool::new(false);
    {
        let Ok(mut last) = LAST.lock() else {
            return false;
        };
        if !crate::service::repair_tick::due(*last, PRUNE_INTERVAL_SECS) {
            return false;
        }
        if RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
        *last = Some(std::time::Instant::now());
    }
    let guard = InFlight(&RUNNING);
    let store = Arc::clone(store);
    let ssh = Arc::clone(ssh);
    tokio::spawn(async move {
        let _guard = guard;
        run_with(&store, ssh.as_ref()).await;
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_fake::{FakeSsh, Match, Reply};

    const ROOT: &str = "/home/u/projects/github.com/o/r";

    fn wt(id: i64, path: &str) -> WorktreeRow {
        WorktreeRow {
            id,
            project_id: 1,
            host_alias: "vps".into(),
            name: path.rsplit('/').next().unwrap().into(),
            path: path.into(),
            branch: None,
        }
    }

    fn one_root_plan(paths: &[&str]) -> HostPlan {
        HostPlan {
            roots: vec![ROOT.into()],
            rows: paths
                .iter()
                .enumerate()
                .map(|(i, p)| (wt(i as i64 + 1, p), 0))
                .collect(),
        }
    }

    #[test]
    fn plan_groups_rows_by_their_projects_repo_root_on_the_host() {
        let projects = vec![
            ProjectRow {
                id: 1,
                owner: "o".into(),
                repo: "r".into(),
                base_path: "/Users/me/p/o/r".into(),
                last_session_at: None,
            },
            ProjectRow {
                id: 2,
                owner: "o".into(),
                repo: "s".into(),
                base_path: "/Users/me/p/o/s".into(),
                last_session_at: None,
            },
        ];
        let mut other = wt(3, "/x/s-wt");
        other.project_id = 2;
        let mut orphan = wt(4, "/x/orphan");
        orphan.project_id = 99;
        let p = plan(
            vec![wt(1, "/x/a"), wt(2, "/x/b"), other, orphan],
            &projects,
            "/home/u/projects/github.com",
            Layout::Github,
        );
        assert_eq!(
            p.roots,
            vec![
                ROOT.to_string(),
                "/home/u/projects/github.com/o/s".to_string()
            ]
        );
        let idx: Vec<(i64, usize)> = p.rows.iter().map(|(r, i)| (r.id, *i)).collect();
        assert_eq!(
            idx,
            vec![(1, 0), (2, 0), (3, 1)],
            "a row of a gone project is not judged"
        );
    }

    #[test]
    fn stale_rows_need_a_missing_path_and_a_listing_without_it() {
        let plan = one_root_plan(&[
            "/w/gone",       // missing, not registered -> stale
            "/w/alive",      // present
            "/w/registered", // missing but still registered (prunable)
        ]);
        let out = "motd line\n@@root 0\nworktree /home/u/projects/github.com/o/r\n\
                   worktree /w/alive\nworktree /w/registered/\n@@rootrc 0 0\n\
                   @@missing 0\n@@present 1\n@@missing 2\n@@fleet_wtprune_done\n";
        let probe = parse_probe(out, 1, 3).expect("complete output");
        let ids: Vec<i64> = stale_rows(&plan, &probe).iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![1]);
    }

    #[test]
    fn unknown_answers_never_delete() {
        let plan = one_root_plan(&["/w/gone"]);
        // Listing failed (rc 128): the root is unknown.
        let failed = "@@root 0\n@@rootrc 0 128\n@@missing 0\n@@fleet_wtprune_done\n";
        let probe = parse_probe(failed, 1, 1).unwrap();
        assert!(stale_rows(&plan, &probe).is_empty());
        // A "successful" empty listing is not trusted either.
        let empty = "@@root 0\n@@rootrc 0 0\n@@missing 0\n@@fleet_wtprune_done\n";
        assert!(stale_rows(&plan, &parse_probe(empty, 1, 1).unwrap()).is_empty());
        // Cut short (no done marker), a row unreported, or a stray index:
        // the whole host is unknown.
        assert_eq!(
            parse_probe("@@root 0\nworktree /a\n@@rootrc 0 0\n@@missing 0\n", 1, 1),
            None
        );
        assert_eq!(
            parse_probe(
                "@@root 0\nworktree /a\n@@rootrc 0 0\n@@fleet_wtprune_done\n",
                1,
                1
            ),
            None
        );
        assert_eq!(
            parse_probe("@@root 3\n@@rootrc 3 0\n@@fleet_wtprune_done\n", 1, 0),
            None
        );
        assert_eq!(parse_probe("", 1, 1), None);
    }

    #[test]
    fn probe_script_quotes_every_path_and_resolves_spellings_on_the_host() {
        let plan = one_root_plan(&["/w/it's here"]);
        let script = probe_script(&plan);
        assert!(script.contains(&format!("r={}; echo '@@root 0'", quote(ROOT))));
        assert!(script.contains("git -C \"$r\" worktree list --porcelain"));
        assert!(script.contains(&format!("p={};", quote("/w/it's here"))));
        assert!(
            script.contains("pwd -P"),
            "spellings are resolved on the host"
        );
        assert!(script.contains("@@rootcanon 0") && script.contains("@@canon 0"));
        assert!(script.ends_with(&format!("echo '{DONE_MARKER}'\n")));
    }

    /// A path stored logically (through `~/projects -> /mnt/sda4/projects`)
    /// while git registers the physical one is still registered: compared
    /// canonically, via its parent resolved on the host, or via the root's
    /// physical spelling when its parent is gone too.
    #[test]
    fn a_registered_worktree_is_kept_under_either_spelling() {
        const PHYS: &str = "/mnt/sda4/projects/github.com/o/r";
        let a = format!("{ROOT}/.worktrees/a"); // parent resolves: @@canon
        let b = format!("{ROOT}/.worktrees/b"); // parent gone: root swap
        let gone = format!("{ROOT}/.worktrees/gone"); // registered under no spelling
        let plan = one_root_plan(&[a.as_str(), b.as_str(), gone.as_str()]);
        let out = format!(
            "@@root 0\n@@rootcanon 0 {PHYS}\nworktree {PHYS}\n\
             worktree {PHYS}/.worktrees/a\nworktree {PHYS}/.worktrees/b\n@@rootrc 0 0\n\
             @@missing 0\n@@canon 0 {PHYS}/.worktrees/a\n@@missing 1\n\
             @@missing 2\n@@canon 2 {PHYS}/.worktrees/gone\n{DONE_MARKER}\n"
        );
        let probe = parse_probe(&out, 1, 3).expect("complete output");
        assert_eq!(probe.root_canon, vec![Some(PHYS.to_string())]);
        assert_eq!(probe.canon[1], None, "b's parent is gone");
        let ids: Vec<i64> = stale_rows(&plan, &probe).iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![3], "only the row registered under no spelling");
        // Without the root's physical spelling, the parent-gone row matches
        // nothing git listed and goes: git is the only authority.
        let no_rootcanon = out.replace(&format!("@@rootcanon 0 {PHYS}\n"), "");
        let probe = parse_probe(&no_rootcanon, 1, 3).unwrap();
        let ids: Vec<i64> = stale_rows(&plan, &probe).iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![2, 3]);
    }

    /// End to end through `prune_host`: the rows hold the logical root while
    /// git lists the worktrees under the physical one. The still-registered
    /// row is kept; only the genuinely unregistered one goes.
    #[tokio::test]
    async fn prune_host_keeps_a_worktree_git_registers_under_the_physical_spelling() {
        const PHYS: &str = "/mnt/sda4/projects/github.com/o/r";
        let (store, _bus, [gone, alive, registered], _local, _session) = seeded();
        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        fake.on_host(
            "vps",
            Match::script_contains(DONE_MARKER),
            Reply::ok(&format!(
                "@@root 0\n@@rootcanon 0 {PHYS}\nworktree {PHYS}\n\
                 worktree {PHYS}/.claude/worktrees/alive\n\
                 worktree {PHYS}/.claude/worktrees/reg\n@@rootrc 0 0\n\
                 @@missing 0\n@@canon 0 {PHYS}/.claude/worktrees/gone\n@@present 1\n\
                 @@missing 2\n@@canon 2 {PHYS}/.claude/worktrees/reg\n{DONE_MARKER}\n"
            )),
        );
        let deleted = prune_host(&store, &fake, "vps").await.unwrap();
        assert_eq!(deleted, vec![gone], "only the genuinely unregistered row");
        let s = store.lock().unwrap();
        assert!(
            s.get_worktree_row(registered).unwrap().is_some(),
            "registered under the physical spelling: kept"
        );
        assert!(s.get_worktree_row(alive).unwrap().is_some());
    }

    /// The comparison is symmetric. A row stored PHYSICALLY (Claude reports
    /// physical cwds) is kept when git registered the worktree LOGICALLY
    /// (fleet adds from the logical `$HOME` root), and so is the mirror case,
    /// even with the whole worktrees dir gone. A row registered under no
    /// spelling still goes.
    #[test]
    fn a_registered_worktree_is_kept_whichever_side_is_logical() {
        const PHYS: &str = "/mnt/sda4/projects/github.com/o/r";
        let phys_row = format!("{PHYS}/.claude/worktrees/x"); // git: logical
        let logical_row = format!("{ROOT}/.claude/worktrees/y"); // git: physical
        let gone = format!("{PHYS}/.claude/worktrees/z"); // git: neither
        let plan = one_root_plan(&[phys_row.as_str(), logical_row.as_str(), gone.as_str()]);
        // `.claude/worktrees/` is gone: no `@@canon` lines, and git's entries
        // are printed only as listed (their parents do not resolve either).
        let out = format!(
            "@@root 0\n@@rootcanon 0 {PHYS}\nworktree {ROOT}\n\
             worktree {ROOT}/.claude/worktrees/x\nworktree {PHYS}/.claude/worktrees/y\n\
             @@rootrc 0 0\n@@missing 0\n@@missing 1\n@@missing 2\n{DONE_MARKER}\n"
        );
        let probe = parse_probe(&out, 1, 3).expect("complete output");
        let ids: Vec<i64> = stale_rows(&plan, &probe).iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![3], "only the row registered under no spelling");
    }

    /// The reviewer's case end to end through `prune_host`: the rows are
    /// stored physically, git registered the worktree under the logical
    /// root, and the worktrees dir is gone. The still-registered row is kept;
    /// only the unregistered one goes.
    #[tokio::test]
    async fn prune_host_keeps_a_physical_row_git_registered_logically() {
        const PHYS: &str = "/mnt/sda4/projects/github.com/o/r";
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("vps").unwrap();
        let pid = s.upsert_project("o", "r", "/Users/me/p/o/r").unwrap();
        let kept = s
            .upsert_worktree_on(
                "vps",
                pid,
                "x",
                &format!("{PHYS}/.claude/worktrees/x"),
                None,
            )
            .unwrap();
        let gone = s
            .upsert_worktree_on(
                "vps",
                pid,
                "z",
                &format!("{PHYS}/.claude/worktrees/z"),
                None,
            )
            .unwrap();
        let store = Mutex::new(s);
        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        fake.on_host(
            "vps",
            Match::script_contains(DONE_MARKER),
            Reply::ok(&format!(
                "@@root 0\n@@rootcanon 0 {PHYS}\nworktree {ROOT}\n\
                 worktree {ROOT}/.claude/worktrees/x\n@@rootrc 0 0\n\
                 @@missing 0\n@@missing 1\n{DONE_MARKER}\n"
            )),
        );
        let deleted = prune_host(&store, &fake, "vps").await.unwrap();
        assert_eq!(deleted, vec![gone], "only the unregistered row");
        assert!(
            store
                .lock()
                .unwrap()
                .get_worktree_row(kept)
                .unwrap()
                .is_some(),
            "registered under the logical spelling: kept"
        );
    }

    /// The race: a row written after the probe started (a hook re-created
    /// the worktree while the probe ran) survives even though the probe
    /// judged it stale; an older stale row on the same host is still pruned.
    #[tokio::test]
    async fn a_row_rewritten_after_the_probe_started_is_kept() {
        let (store, _bus, [gone, alive, registered], _local, _session) = seeded();
        store
            .lock()
            .unwrap()
            .set_worktree_updated_at_ms(gone, i64::MAX / 2)
            .unwrap();
        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        // Both `gone` and `reg` are missing and unregistered.
        fake.on_host(
            "vps",
            Match::script_contains(DONE_MARKER),
            Reply::ok(&format!(
                "@@root 0\nworktree {ROOT}\nworktree {ROOT}/.claude/worktrees/alive\n\
                 @@rootrc 0 0\n@@missing 0\n@@present 1\n@@missing 2\n{DONE_MARKER}\n"
            )),
        );
        let deleted = prune_host(&store, &fake, "vps").await.unwrap();
        assert_eq!(deleted, vec![registered], "the older stale row still goes");
        let s = store.lock().unwrap();
        assert!(
            s.get_worktree_row(gone).unwrap().is_some(),
            "written after the probe started: live, kept"
        );
        assert!(s.get_worktree_row(alive).unwrap().is_some());
    }

    /// A store with remote host `vps`, project o/r, three `vps` worktree rows
    /// and one local row, plus a session pointing at the first remote row.
    /// Returns `(store, bus, [gone, alive, registered], local, session)`.
    fn seeded() -> (
        Mutex<Store>,
        Arc<crate::events::RecordingEventBus>,
        [i64; 3],
        i64,
        i64,
    ) {
        let bus = Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        s.upsert_host("vps").unwrap();
        let pid = s.upsert_project("o", "r", "/Users/me/p/o/r").unwrap();
        let gone = s
            .upsert_worktree_on(
                "vps",
                pid,
                "gone",
                &format!("{ROOT}/.claude/worktrees/gone"),
                None,
            )
            .unwrap();
        let alive = s
            .upsert_worktree_on(
                "vps",
                pid,
                "alive",
                &format!("{ROOT}/.claude/worktrees/alive"),
                None,
            )
            .unwrap();
        let registered = s
            .upsert_worktree_on(
                "vps",
                pid,
                "reg",
                &format!("{ROOT}/.claude/worktrees/reg"),
                None,
            )
            .unwrap();
        let local = s
            .upsert_worktree(pid, "gone", "/Users/me/p/o/r/.claude/worktrees/gone", None)
            .unwrap();
        let session = s
            .upsert_session("dev", "vps", Some(pid), Some(gone), 1, 1, "running", None)
            .unwrap();
        (
            Mutex::new(s),
            bus,
            [gone, alive, registered],
            local,
            session,
        )
    }

    fn probe_reply() -> Reply {
        Reply::ok(&format!(
            "@@root 0\nworktree {ROOT}\nworktree {ROOT}/.claude/worktrees/alive\n\
             worktree {ROOT}/.claude/worktrees/reg\n@@rootrc 0 0\n\
             @@missing 0\n@@present 1\n@@missing 2\n{DONE_MARKER}\n"
        ))
    }

    #[tokio::test]
    async fn prune_host_deletes_only_gone_unregistered_rows_in_one_call() {
        let (store, bus, [gone, alive, registered], local, session) = seeded();
        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        fake.on_host("vps", Match::script_contains(DONE_MARKER), probe_reply());
        bus.take();
        let deleted = prune_host(&store, &fake, "vps").await.unwrap();
        assert_eq!(deleted, vec![gone]);
        let scripts: Vec<_> = fake
            .calls_for("vps")
            .into_iter()
            .filter_map(|c| c.script())
            .filter(|s| s.contains(DONE_MARKER))
            .collect();
        assert_eq!(
            scripts.len(),
            1,
            "one batched probe per host: {:?}",
            fake.commands()
        );
        let s = store.lock().unwrap();
        assert!(s.get_worktree_row(gone).unwrap().is_none());
        assert!(
            s.get_worktree_row(alive).unwrap().is_some(),
            "present on disk"
        );
        assert!(
            s.get_worktree_row(registered).unwrap().is_some(),
            "still registered with git"
        );
        assert!(
            s.get_worktree_row(local).unwrap().is_some(),
            "local rows untouched"
        );
        assert_eq!(
            s.get_session_by_id(session).unwrap().unwrap().worktree_id,
            None,
            "the session's reference is cleared"
        );
        assert!(
            bus.take().contains(&format!("session:updated:{session}")),
            "and announced"
        );
    }

    #[tokio::test]
    async fn an_unreachable_or_failing_host_deletes_nothing() {
        let (store, _bus, ids, _local, _session) = seeded();
        let fake = FakeSsh::new();
        fake.unreachable("vps");
        assert!(prune_host(&store, &fake, "vps").await.is_err());
        // Reachable, but the probe exits non-zero.
        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        fake.on_host(
            "vps",
            Match::script_contains(DONE_MARKER),
            Reply::fail(1, "boom"),
        );
        assert!(prune_host(&store, &fake, "vps").await.is_err());
        // Reachable, but the output is cut short.
        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        fake.on_host(
            "vps",
            Match::script_contains(DONE_MARKER),
            Reply::ok("@@root 0\nworktree /x\n"),
        );
        assert!(prune_host(&store, &fake, "vps").await.is_err());
        let s = store.lock().unwrap();
        for id in ids {
            assert!(s.get_worktree_row(id).unwrap().is_some(), "row {id} kept");
        }
    }

    /// Only hosts the last reconcile reached are probed. A host marked
    /// unreachable (what reconcile records when its probe fails) gets no ssh
    /// at all and keeps its rows; once reachable again it is probed and its
    /// stale row pruned.
    #[tokio::test]
    async fn run_with_probes_only_hosts_the_last_reconcile_reached() {
        use crate::store::HostReconcile;
        let (store, _bus, [gone, alive, registered], _local, _session) = seeded();
        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        fake.on_host("vps", Match::script_contains(DONE_MARKER), probe_reply());
        let mark = |reachable: bool| {
            store
                .lock()
                .unwrap()
                .apply_host_reconcile(HostReconcile {
                    alias: "vps",
                    reachable,
                    claude_version: None,
                    tmux_version: None,
                    last_pinged_at: 1,
                    probe_started_at: 0,
                    sessions: &[],
                    keep: &[],
                })
                .unwrap();
        };
        mark(false);
        assert_eq!(run_with(&store, &fake).await, (0, 0));
        assert!(fake.calls().is_empty(), "no ssh to an unreachable host");
        {
            let s = store.lock().unwrap();
            for id in [gone, alive, registered] {
                assert!(s.get_worktree_row(id).unwrap().is_some(), "row {id} kept");
            }
        }
        mark(true);
        assert_eq!(run_with(&store, &fake).await, (1, 1));
        assert!(store
            .lock()
            .unwrap()
            .get_worktree_row(gone)
            .unwrap()
            .is_none());
    }
}
