//! Local workspace sync, Phase 1
//! (`docs/superpowers/specs/2026-10-07-local-workspace-sync-design.md`): one
//! remote worktree kept in step, both ways, with one directory on the
//! machine running the desktop app.
//!
//! A pass scans both sides, hashes (local) or downloads (remote) only what
//! moved since the BASE, asks [`plan::decide`] per path, and carries the
//! answer out under write guards on both sides, so an edit made on either
//! side during a pass is never overwritten: a write that finds its target
//! moved is left for the next pass, and two sides that changed differently
//! become a conflict that writes nothing until someone picks. Nothing here
//! is a git operation.
//!
//! Phases 2 and 3 ([`handoff`], [`open`]) build on the pass: the activity
//! log it writes says which side made each change, and the worktree's own
//! git answers what is uncommitted.
//!
//! Desktop-only: the directory is on this machine, so a hub never runs it
//! (every command is `LocalOnly` in hub-client mode).

pub mod excludes;
mod git;
pub mod handoff;
mod local;
pub mod open;
mod plan;
mod remote;
// Unix only: the fixture plays the host with this machine's own `bash`, and
// a fleet host is always a Unix box. Windows still runs the local-side
// (`local.rs`) and planning tests.
#[cfg(test)]
#[cfg(unix)]
mod tests;

use crate::ipc_error::{codes, lock, IpcError};
use crate::ssh::SshExec;
use crate::store::{
    BaseEntry, FileStat, LocalConflictRow, LocalPassWrite, LocalWorkspaceRow, LocalWorkspaceStatus,
    NewLocalConflict, NewLocalWorkspace, Store,
};
use dashmap::DashMap;
use excludes::Excludes;
use plan::{Action, Now};
use remote::{PushOp, PushResult};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Files larger than this are left out (and counted as skipped).
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// One download or upload carries at most about this much.
const BATCH_BYTES: u64 = 64 * 1024 * 1024;
/// How often an enabled link gets a pass.
pub const PASS_INTERVAL: Duration = Duration::from_secs(5);
/// How long an unreachable host is left alone before the next try.
pub const OFFLINE_BACKOFF: Duration = Duration::from_secs(60);
/// A stat recorded within this long of the file's own mtime is not trusted
/// (a same-size edit in the same tick would hide behind it): the side is
/// re-checked next pass, as git does with its racily clean entries.
const RACY_SECS: i64 = 2;
/// A pass that finds at least this many BASE files gone from one side, and
/// more than half of them, stops and pauses the link instead of deleting
/// them on the other side: a wiped or unmounted folder is not an edit.
const MASS_DELETE_MIN: usize = 20;
/// Passes running at once, across links.
const MAX_PARALLEL_PASSES: usize = 4;

/// Whether `p` was left out, or lies below a symlink on either side: a
/// path below a symlinked directory is in another tree, never ours to
/// write or delete. Only links block what is below them; a file that
/// became a directory still syncs its contents.
fn is_blocked(blocked: &HashSet<String>, links: &HashSet<String>, p: &str) -> bool {
    if blocked.contains(p) {
        return true;
    }
    let mut cur = p;
    while let Some(i) = cur.rfind('/') {
        cur = &cur[..i];
        if links.contains(cur) {
            return true;
        }
    }
    false
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// Whether an error means the host could not be reached (as opposed to the
/// host answering that something is wrong).
fn is_offline(e: &IpcError) -> bool {
    e.code.starts_with("E_SSH")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnableLocalWorkspaceArgs {
    /// The session whose worktree to sync.
    pub session_id: i64,
    /// An absolute directory on this machine; created when missing.
    pub local_path: String,
    /// Extra patterns to leave out, gitignore syntax.
    #[serde(default)]
    pub excludes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalWorkspaceIdArgs {
    pub id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetLocalWorkspaceExcludesArgs {
    pub id: i64,
    pub excludes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveLocalConflictArgs {
    pub id: i64,
    pub path: String,
    /// `local` or `remote`.
    pub keep: String,
}

/// The engine: the store, a way to reach hosts, and which links are mid-pass.
pub struct LocalSync {
    store: Arc<Mutex<Store>>,
    ssh: Arc<dyn SshExec>,
    /// One pass per link at a time.
    passes: DashMap<i64, Arc<tokio::sync::Mutex<()>>>,
    /// When each link is due again (offline links back off).
    due: DashMap<i64, Instant>,
    slots: Arc<tokio::sync::Semaphore>,
}

impl LocalSync {
    pub fn new(store: Arc<Mutex<Store>>, ssh: Arc<dyn SshExec>) -> Arc<Self> {
        Arc::new(LocalSync {
            store,
            ssh,
            passes: DashMap::new(),
            due: DashMap::new(),
            slots: Arc::new(tokio::sync::Semaphore::new(MAX_PARALLEL_PASSES)),
        })
    }

    fn pass_lock(&self, id: i64) -> Arc<tokio::sync::Mutex<()>> {
        self.passes.entry(id).or_default().clone()
    }

    /// Give every due, unpaused link a pass every [`PASS_INTERVAL`] until
    /// `token` fires. See [`spawn_local_sync_tick`].
    fn spawn_tick(self: &Arc<Self>, token: CancellationToken) {
        let me = Arc::clone(self);
        crate::rt::spawn(async move {
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = tokio::time::sleep(PASS_INTERVAL) => {}
                }
                me.tick();
            }
        });
    }

    /// One pass over the links; `false` when Pause all (redesign 8.1) or an
    /// unreadable store stopped it before any link was looked at.
    fn tick(self: &Arc<Self>) -> bool {
        if !crate::service::loops::gate("local_sync", &self.store, Some(PASS_INTERVAL)) {
            return false;
        }
        let rows = match lock(&self.store).and_then(|s| s.list_local_workspaces()) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %e, "local sync: could not list links");
                crate::service::loops::report("local_sync", Err(e), Some(PASS_INTERVAL));
                return false;
            }
        };
        let now = Instant::now();
        for row in rows.into_iter().filter(|r| !r.paused) {
            if self.due.get(&row.id).is_some_and(|d| *d > now) {
                continue;
            }
            let lock = self.pass_lock(row.id);
            let Ok(guard) = lock.try_lock_owned() else {
                continue; // still running
            };
            let me = Arc::clone(self);
            crate::rt::spawn(async move {
                let Ok(_slot) = me.slots.clone().acquire_owned().await else {
                    return;
                };
                me.pass_locked(row.id).await;
                drop(guard);
            });
        }
        // Forget the bookkeeping of links that are gone.
        let alive: HashSet<i64> = lock(&self.store)
            .and_then(|s| s.list_local_workspaces())
            .map(|rows| rows.into_iter().map(|r| r.id).collect())
            .unwrap_or_default();
        self.due.retain(|id, _| alive.contains(id));
        crate::service::loops::report("local_sync", Ok::<_, String>(()), Some(PASS_INTERVAL));
        true
    }

    /// One pass now, waiting for a running one to finish first.
    pub async fn sync_now(&self, id: i64) -> Result<LocalWorkspaceRow, IpcError> {
        let lock = self.pass_lock(id);
        let _guard = lock.lock().await;
        self.pass_locked(id).await;
        self.row(id)
    }

    fn row(&self, id: i64) -> Result<LocalWorkspaceRow, IpcError> {
        lock(&self.store)?
            .local_workspace(id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no local workspace {id}")))
    }

    /// Run one pass and record how it ended. The caller holds the link's
    /// pass lock.
    async fn pass_locked(&self, id: i64) {
        let row = match lock(&self.store).and_then(|s| s.local_workspace(id)) {
            Ok(Some(row)) if !row.paused => row,
            _ => return,
        };
        let result = self.pass(&row).await;
        let (status, backoff, pause) = match &result {
            Ok(o) => (
                LocalWorkspaceStatus {
                    state: o.state(),
                    last_error: None,
                    pending_local: o.pending_local,
                    pending_remote: o.pending_remote,
                    skipped: o.skipped,
                    synced_at: Some(now_secs()),
                },
                PASS_INTERVAL,
                false,
            ),
            Err(f) => (
                LocalWorkspaceStatus {
                    state: if is_offline(&f.error) {
                        "offline"
                    } else {
                        "error"
                    },
                    last_error: Some(f.error.message.as_str()),
                    pending_local: row.pending_local,
                    pending_remote: row.pending_remote,
                    skipped: row.skipped,
                    synced_at: None,
                },
                if is_offline(&f.error) {
                    OFFLINE_BACKOFF
                } else {
                    PASS_INTERVAL * 6
                },
                f.pause,
            ),
        };
        self.due.insert(id, Instant::now() + backoff);
        let Ok(s) = lock(&self.store) else { return };
        if pause {
            let _ = s.set_local_workspace_paused(id, true);
        }
        if let Err(e) = s.set_local_workspace_status(id, &status) {
            tracing::warn!(id, error = %e, "local sync: could not record a pass");
        }
    }

    /// The pass itself. Reads the store, then works without its lock, then
    /// writes everything it learned in one transaction.
    async fn pass(&self, row: &LocalWorkspaceRow) -> Result<PassOutcome, PassFail> {
        let base = lock(&self.store)?.local_workspace_base(row.id)?;
        let conflicts: HashMap<String, LocalConflictRow> = row
            .conflicts
            .iter()
            .map(|c| (c.path.clone(), c.clone()))
            .collect();
        let ex = Excludes::new(&row.excludes)?;
        let root = PathBuf::from(&row.local_path);
        let host = row.host_alias.as_str();
        let rroot = row.remote_path.as_str();

        // 1. Scan both sides.
        let also: HashSet<String> = base.keys().chain(conflicts.keys()).cloned().collect();
        let local_now = now_ns();
        let lscan = {
            let (root, ex, also) = (root.clone(), ex.clone(), also.clone());
            tokio::task::spawn_blocking(move || local::scan(&root, &ex, &also))
                .await
                .map_err(join_err)??
        };
        let mut also_sorted: Vec<String> = also.iter().cloned().collect();
        also_sorted.sort();
        let rscan = remote::scan(self.ssh.as_ref(), host, rroot, &also_sorted).await?;

        let mut skipped = lscan.skipped;
        let mut blocked: HashSet<String> = lscan.blocked.clone();
        let mut links: HashSet<String> = lscan.links.clone();
        let mut rfiles: HashMap<String, FileStat> = HashMap::new();
        for (p, (kind, st)) in &rscan.entries {
            if ex.excluded(p, false) {
                continue;
            }
            match kind {
                remote::Kind::File if st.size as u64 <= MAX_FILE_BYTES => {
                    rfiles.insert(p.clone(), *st);
                }
                // Carried as its target (`local::link_blob`); what lies
                // below it is still another tree.
                remote::Kind::Symlink if local::LINKS => {
                    links.insert(p.clone());
                    rfiles.insert(p.clone(), *st);
                }
                remote::Kind::File | remote::Kind::Symlink => {
                    skipped += 1;
                    blocked.insert(p.clone());
                    if *kind == remote::Kind::Symlink {
                        links.insert(p.clone());
                    }
                }
                remote::Kind::Other => {
                    blocked.insert(p.clone());
                }
            }
        }

        // 2. A folder that lost most of its files was wiped or unmounted,
        // not edited: stop before carrying that across.
        let live_base: Vec<&String> = base
            .keys()
            .filter(|p| !ex.excluded(p, false) && !is_blocked(&blocked, &links, p))
            .collect();
        // Gone from one side while the other still has it: a deletion made
        // on both sides is agreement, not a wipe, and goes through.
        for (side, gone, empty) in [
            (
                "local folder",
                live_base
                    .iter()
                    .filter(|p| !lscan.files.contains_key(**p) && rfiles.contains_key(**p))
                    .count(),
                lscan.files.is_empty(),
            ),
            (
                "worktree on the host",
                live_base
                    .iter()
                    .filter(|p| !rfiles.contains_key(**p) && lscan.files.contains_key(**p))
                    .count(),
                rfiles.is_empty(),
            ),
        ] {
            // Every synced file gone and nothing left: an unmounted volume's
            // empty mount point, which the count alone would let through.
            let emptied = empty && gone > 0 && gone == live_base.len();
            if emptied || (gone >= MASS_DELETE_MIN && gone * 2 > live_base.len()) {
                return Err(PassFail {
                    error: IpcError::new(
                        codes::E_INVALID_STATE,
                        format!(
                            "{gone} of {} synced files disappeared from the {side}; sync is \
                             paused and nothing was deleted. Restore them, or, if they were \
                             meant to go, delete them on the other side too and resume",
                            live_base.len()
                        ),
                    ),
                    pause: true,
                });
            }
        }

        // 3. Which side of which path needs a closer look.
        let all: BTreeSet<&String> = base
            .keys()
            .chain(lscan.files.keys())
            .chain(rfiles.keys())
            .chain(conflicts.keys())
            .collect();
        let mut work: Vec<PathWork> = Vec::new();
        for p in all {
            if ex.excluded(p, false) || is_blocked(&blocked, &links, p) {
                continue;
            }
            let b = base.get(p);
            let c = conflicts.get(p);
            let l = side_state(
                lscan.files.get(p).copied(),
                b.and_then(|b| b.local.map(|st| (b.sha256.as_str(), st))),
                c.and_then(|c| c.local.as_ref()),
            );
            let r = side_state(
                rfiles.get(p).copied(),
                b.and_then(|b| b.remote.map(|st| (b.sha256.as_str(), st))),
                c.and_then(|c| c.remote.as_ref()),
            );
            let unchanged =
                |s: &Side| matches!(s, Side::Known(sha, _) if b.is_some_and(|b| &b.sha256 == sha));
            if c.is_none() && b.is_some() && unchanged(&l) && unchanged(&r) {
                continue;
            }
            work.push(PathWork {
                path: p.clone(),
                local: l,
                remote: r,
            });
        }

        // 4. Hash what moved here; download what moved there.
        let to_hash: Vec<String> = work
            .iter()
            .filter(|w| matches!(w.local, Side::Changed(_)))
            .map(|w| w.path.clone())
            .collect();
        let hashed: HashMap<String, (String, FileStat)> = {
            let root = root.clone();
            tokio::task::spawn_blocking(move || {
                to_hash
                    .into_iter()
                    .filter_map(|p| local::hash_file(&root, &p).map(|h| (p, h)))
                    .collect()
            })
            .await
            .map_err(join_err)?
        };
        let to_fetch: Vec<(String, u64)> = work
            .iter()
            .filter_map(|w| match w.remote {
                Side::Changed(st) => Some((w.path.clone(), st.size as u64)),
                _ => None,
            })
            .collect();
        let mut fetched = self.fetch(host, rroot, &to_fetch).await?;

        // 5. Decide.
        let mut out = PassOutcome {
            skipped,
            ..Default::default()
        };
        let mut write = LocalPassWrite::default();
        let mut cleared: HashSet<String> = HashSet::new();
        let mut added: HashSet<String> = HashSet::new();
        let mut pulls: Vec<(String, String, FileStat, Option<FileStat>)> = Vec::new();
        let mut deletes_local: Vec<(String, FileStat)> = Vec::new();
        let mut pushes: Vec<(String, Now, Option<String>)> = Vec::new();
        let mut deletes_remote: Vec<(String, String)> = Vec::new();
        for w in &work {
            let Some(l) = resolve(&w.local, hashed.get(&w.path).cloned()) else {
                continue; // changed under the hash: next pass
            };
            let Some(r) = resolve(
                &w.remote,
                fetched
                    .get(&w.path)
                    .map(|(bytes, _)| (local::sha256_hex(bytes), w.remote.stat_or_default())),
            ) else {
                continue; // vanished before the download: next pass
            };
            let b = base.get(&w.path);
            let c = conflicts.get(&w.path);
            match plan::decide(b, c, &l, &r) {
                Action::Nothing => {}
                Action::Record => {
                    write.upsert.push((
                        w.path.clone(),
                        BaseEntry {
                            sha256: sha_of(&l),
                            local: trust(l.stat(), local_now, 1_000_000_000),
                            remote: trust(r.stat(), rscan.now, 1),
                        },
                    ));
                    if c.is_some() {
                        cleared.insert(w.path.clone());
                    }
                }
                Action::Forget => {
                    if b.is_some() {
                        write.remove.push(w.path.clone());
                    }
                    if c.is_some() {
                        cleared.insert(w.path.clone());
                    }
                }
                Action::Conflict(kind) => {
                    added.insert(w.path.clone());
                    write.add_conflicts.push(NewLocalConflict {
                        path: w.path.clone(),
                        kind,
                        local: l.seen(),
                        remote: r.seen(),
                    });
                }
                Action::Pull { expect_local } => {
                    let Now::Has { sha, stat } = &r else { continue };
                    pulls.push((w.path.clone(), sha.clone(), *stat, expect_local));
                }
                Action::DeleteLocal { expect } => deletes_local.push((w.path.clone(), expect)),
                Action::Push { expect_remote } => {
                    pushes.push((w.path.clone(), l.clone(), expect_remote))
                }
                Action::DeleteRemote { expect } => deletes_remote.push((w.path.clone(), expect)),
            }
        }

        // 6. Carry it out. Pulls whose bytes were not downloaded yet (a
        // conflict resolved to the remote side that has not moved) first.
        let missing: Vec<(String, u64)> = pulls
            .iter()
            .filter(|(p, ..)| !fetched.contains_key(p))
            .map(|(p, _, st, _)| (p.clone(), st.size as u64))
            .collect();
        fetched.extend(self.fetch(host, rroot, &missing).await?);
        let pull_jobs: Vec<PullJob> = pulls
            .into_iter()
            .map(|(p, sha, rst, expect)| {
                let bytes = fetched.remove(&p);
                (p, sha, rst, expect, bytes)
            })
            .collect();
        let local_results = {
            let root = root.clone();
            tokio::task::spawn_blocking(move || apply_local(&root, pull_jobs, deletes_local))
                .await
                .map_err(join_err)?
        };
        for r in local_results {
            match r {
                LocalApplied::Wrote {
                    path,
                    sha,
                    local,
                    remote,
                } => {
                    write
                        .activity
                        .push((path.clone(), "remote", change_kind(&base, &path)));
                    write.upsert.push((
                        path.clone(),
                        BaseEntry {
                            sha256: sha,
                            local: trust(local, now_ns(), 1_000_000_000),
                            remote: trust(Some(remote), rscan.now, 1),
                        },
                    ));
                    if conflicts.contains_key(&path) {
                        cleared.insert(path);
                    }
                }
                LocalApplied::Deleted(path) => {
                    write.activity.push((path.clone(), "remote", "deleted"));
                    write.remove.push(path.clone());
                    if conflicts.contains_key(&path) {
                        cleared.insert(path);
                    }
                }
                LocalApplied::Pending => out.pending_remote += 1,
                LocalApplied::Failed(path, e) => {
                    tracing::warn!(path, error = %e, "local sync: local write failed");
                    out.pending_remote += 1;
                }
            }
        }

        let mut ops: Vec<(PushOp, Option<(String, FileStat)>)> = Vec::new();
        {
            let root = root.clone();
            let reads = tokio::task::spawn_blocking(move || {
                pushes
                    .into_iter()
                    .map(|(p, l, expect)| {
                        let Now::Has { sha, stat } = l else {
                            return (p, None, expect);
                        };
                        let bytes = local::read_guarded(&root, &p, stat)
                            .map(|b| (b, local::is_executable(&root, &p), sha, stat));
                        (p, bytes, expect)
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .map_err(join_err)?;
            for (path, read, expect) in reads {
                match read {
                    Some((bytes, executable, sha, stat)) => ops.push((
                        PushOp::Write {
                            path,
                            bytes,
                            executable,
                            mtime_secs: (stat.mtime / 1_000_000_000).max(0) as u64,
                            expect,
                        },
                        Some((sha, stat)),
                    )),
                    None => out.pending_local += 1, // moved since the hash
                }
            }
        }
        for (path, expect) in deletes_remote {
            ops.push((PushOp::Delete { path, expect }, None));
        }
        for batch in batches(ops) {
            let (ops, sides): (Vec<PushOp>, Vec<_>) = batch.into_iter().unzip();
            let results = remote::push(self.ssh.as_ref(), host, rroot, &ops).await?;
            for (op, local_side) in ops.iter().zip(sides) {
                let path = op.path().to_string();
                match (results.get(&path), local_side) {
                    (Some(PushResult::Done(rst)), Some((sha, lst))) => {
                        write
                            .activity
                            .push((path.clone(), "local", change_kind(&base, &path)));
                        write.upsert.push((
                            path.clone(),
                            BaseEntry {
                                sha256: sha,
                                local: trust(Some(lst), local_now, 1_000_000_000),
                                remote: trust(*rst, rscan.now, 1),
                            },
                        ));
                        if conflicts.contains_key(&path) {
                            cleared.insert(path);
                        }
                    }
                    (Some(PushResult::Done(_)), None) => {
                        write.activity.push((path.clone(), "local", "deleted"));
                        write.remove.push(path.clone());
                        if conflicts.contains_key(&path) {
                            cleared.insert(path);
                        }
                    }
                    _ => out.pending_local += 1,
                }
            }
        }

        // 7. Record. The first pass (an empty BASE) is the initial copy, not
        // anyone's change: it leaves the activity log alone.
        if base.is_empty() {
            write.activity.clear();
        }
        write.clear_conflicts = cleared.iter().cloned().collect();
        out.conflicts = conflicts
            .keys()
            .filter(|p| !cleared.contains(*p))
            .chain(added.iter())
            .collect::<HashSet<_>>()
            .len() as i64;
        lock(&self.store)?.apply_local_pass(row.id, &write, now_secs())?;
        Ok(out)
    }

    /// Download `paths` in batches of about [`BATCH_BYTES`].
    async fn fetch(
        &self,
        host: &str,
        root: &str,
        paths: &[(String, u64)],
    ) -> Result<HashMap<String, (Vec<u8>, u32)>, IpcError> {
        let mut out = HashMap::new();
        let mut batch: Vec<String> = Vec::new();
        let mut bytes = 0u64;
        for (i, (p, size)) in paths.iter().enumerate() {
            batch.push(p.clone());
            bytes += size;
            if bytes >= BATCH_BYTES || i + 1 == paths.len() {
                out.extend(remote::pull(self.ssh.as_ref(), host, root, &batch, bytes).await?);
                batch.clear();
                bytes = 0;
            }
        }
        Ok(out)
    }
}

/// The background tick: every [`PASS_INTERVAL`], a pass for each enabled,
/// unpaused link that is due. Desktop and standalone only (the desktop's
/// `bootstrap::tasks` starts it; a hub client never does).
pub fn spawn_local_sync_tick(engine: &Arc<LocalSync>, token: CancellationToken) {
    engine.spawn_tick(token);
}

/// How a pass failed, and whether the link must pause (mass deletion).
#[derive(Debug)]
struct PassFail {
    error: IpcError,
    pause: bool,
}

impl From<IpcError> for PassFail {
    fn from(error: IpcError) -> Self {
        PassFail {
            error,
            pause: false,
        }
    }
}

fn join_err(e: tokio::task::JoinError) -> IpcError {
    IpcError::new(codes::E_INTERNAL, format!("local sync task: {e}"))
}

#[derive(Debug, Default)]
struct PassOutcome {
    pending_local: i64,
    pending_remote: i64,
    skipped: i64,
    conflicts: i64,
}

impl PassOutcome {
    fn state(&self) -> &'static str {
        if self.conflicts > 0 {
            "conflict"
        } else if self.pending_local > 0 {
            "local_changes"
        } else if self.pending_remote > 0 {
            "remote_changes"
        } else {
            "synced"
        }
    }
}

/// One side of one path before the closer look.
#[derive(Debug, Clone)]
enum Side {
    Gone,
    /// The stat says the content is the one recorded (BASE or conflict).
    Known(String, FileStat),
    /// Present and moved: hash it (local) or download it (remote).
    Changed(FileStat),
}

impl Side {
    fn stat_or_default(&self) -> FileStat {
        match self {
            Side::Known(_, st) | Side::Changed(st) => *st,
            Side::Gone => FileStat { size: 0, mtime: 0 },
        }
    }
}

struct PathWork {
    path: String,
    local: Side,
    remote: Side,
}

/// What one side holds, judged by its stat against the BASE's and an open
/// conflict's record of that side.
fn side_state(
    now: Option<FileStat>,
    base: Option<(&str, FileStat)>,
    conflict: Option<&crate::store::SideSeen>,
) -> Side {
    let Some(st) = now else { return Side::Gone };
    if let Some(c) = conflict {
        return if c.stat == st {
            Side::Known(c.sha256.clone(), st)
        } else {
            Side::Changed(st)
        };
    }
    match base {
        Some((sha, bst)) if bst == st => Side::Known(sha.to_string(), st),
        _ => Side::Changed(st),
    }
}

/// The side as a [`Now`], given the closer look's answer for a changed one.
fn resolve(side: &Side, looked: Option<(String, FileStat)>) -> Option<Now> {
    match side {
        Side::Gone => Some(Now::Gone),
        Side::Known(sha, stat) => Some(Now::Has {
            sha: sha.clone(),
            stat: *stat,
        }),
        Side::Changed(_) => looked.map(|(sha, stat)| Now::Has { sha, stat }),
    }
}

/// `added` for a path the BASE did not have, else `modified`.
fn change_kind(base: &HashMap<String, BaseEntry>, path: &str) -> &'static str {
    if base.contains_key(path) {
        "modified"
    } else {
        "added"
    }
}

fn sha_of(n: &Now) -> String {
    match n {
        Now::Has { sha, .. } => sha.clone(),
        Now::Gone => String::new(),
    }
}

/// A stat to record, or `None` when it is too close to `now` to trust
/// (`unit` = how many mtime units make a second on that side).
fn trust(st: Option<FileStat>, now: i64, unit: i64) -> Option<FileStat> {
    let st = st?;
    (now - st.mtime >= RACY_SECS * unit).then_some(st)
}

enum LocalApplied {
    Wrote {
        path: String,
        sha: String,
        local: Option<FileStat>,
        remote: FileStat,
    },
    Deleted(String),
    Pending,
    Failed(String, IpcError),
}

type PullJob = (
    String,
    String,
    FileStat,
    Option<FileStat>,
    Option<(Vec<u8>, u32)>,
);

fn apply_local(
    root: &Path,
    pulls: Vec<PullJob>,
    deletes: Vec<(String, FileStat)>,
) -> Vec<LocalApplied> {
    let mut out = Vec::new();
    for (path, sha, rst, expect, bytes) in pulls {
        // The download must be the content the plan decided on.
        let Some((bytes, mode)) = bytes.filter(|(b, _)| local::sha256_hex(b) == sha) else {
            out.push(LocalApplied::Pending);
            continue;
        };
        match local::write_guarded(root, &path, &bytes, mode & 0o111 != 0, expect) {
            Ok(local::Guarded::Done(st)) => out.push(LocalApplied::Wrote {
                path,
                sha,
                local: st,
                remote: rst,
            }),
            Ok(local::Guarded::Moved) => out.push(LocalApplied::Pending),
            Err(e) => out.push(LocalApplied::Failed(path, e)),
        }
    }
    for (path, expect) in deletes {
        match local::delete_guarded(root, &path, expect) {
            Ok(local::Guarded::Done(_)) => out.push(LocalApplied::Deleted(path)),
            Ok(local::Guarded::Moved) => out.push(LocalApplied::Pending),
            Err(e) => out.push(LocalApplied::Failed(path, e)),
        }
    }
    out
}

/// Split uploads into batches of about [`BATCH_BYTES`].
fn batches<T>(ops: Vec<(PushOp, T)>) -> Vec<Vec<(PushOp, T)>> {
    let mut out = Vec::new();
    let mut cur = Vec::new();
    let mut bytes = 0u64;
    for (op, t) in ops {
        if let PushOp::Write { bytes: b, .. } = &op {
            bytes += b.len() as u64;
        }
        cur.push((op, t));
        if bytes >= BATCH_BYTES {
            out.push(std::mem::take(&mut cur));
            bytes = 0;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

// ---------------------------------------------------------------------------
// Commands (the desktop's Tauri handlers call these).
// ---------------------------------------------------------------------------

pub fn list(store: &Mutex<Store>) -> Result<Vec<LocalWorkspaceRow>, IpcError> {
    lock(store)?.list_local_workspaces()
}

/// Bind the worktree of `args.session_id` to `args.local_path` and start
/// syncing it. The first pass runs in the background: into an empty folder
/// it is a download; into one that already has files, identical files are
/// adopted, one-sided files are copied and differing files become
/// conflicts — nothing is overwritten.
pub async fn enable(
    engine: &Arc<LocalSync>,
    args: EnableLocalWorkspaceArgs,
) -> Result<LocalWorkspaceRow, IpcError> {
    let local_path = normalize_local_path(&args.local_path)?;
    Excludes::new(&args.excludes)?;
    let (host, owner, repo, key, remote_guess) = {
        let s = lock(&engine.store)?;
        let row = s.get_session_by_id(args.session_id)?.ok_or_else(|| {
            IpcError::new(codes::E_NOTFOUND, format!("no session {}", args.session_id))
        })?;
        let pid = row.project_id.ok_or_else(|| {
            IpcError::new(
                codes::E_NOREPO,
                "this session is not in a project, so it has no worktree to sync",
            )
        })?;
        let key = row
            .worktree_key
            .clone()
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| "main".to_string());
        let (owner, repo) = crate::service::sessions::fetch_owner_repo(&s, pid)?;
        let (root, wt) = crate::service::sessions::work_dirs(&s, &row.host_alias, pid, Some(&key))?;
        (row.host_alias, owner, repo, key, wt.unwrap_or(root))
    };
    let remote_path = if remote_guess == "~" || remote_guess.starts_with("~/") {
        let home = engine.ssh.remote_home(&host).await?;
        crate::service::projects::expand_home(&remote_guess, &home)
    } else {
        remote_guess
    };
    bind(
        engine,
        NewLink {
            host,
            owner,
            repo,
            key,
            remote_path,
            local_path,
            session_id: Some(args.session_id),
            excludes: args.excludes,
        },
    )
    .await
}

/// A session's worktree as a desktop paired with a hub knows it: from the
/// hub's rows, because this machine's database holds none of them.
#[derive(Debug, Clone)]
pub struct HubSessionWorktree {
    pub host_alias: String,
    pub owner: String,
    pub repo: String,
    /// `main` for the project root.
    pub worktree_key: String,
    pub tmux_name: String,
}

const ROOT_MARK: &str = "@@FLEET-ROOT@@";

/// The paired desktop's `enable`: the session and its project come from the
/// hub, and the worktree's path from the session's own pane on the host
/// (`git rev-parse --show-toplevel` there), asked over this machine's SSH,
/// which then carries the sync. The link remembers no session: the hub's
/// session ids are not this database's.
pub async fn enable_on_hub_session(
    engine: &Arc<LocalSync>,
    w: HubSessionWorktree,
    local_path: &str,
    excludes: Vec<String>,
) -> Result<LocalWorkspaceRow, IpcError> {
    let local_path = normalize_local_path(local_path)?;
    Excludes::new(&excludes)?;
    if w.host_alias == crate::service::projects::LOCAL_HOST {
        return Err(IpcError::new(
            codes::E_INVALID,
            "this session runs on the hub's own machine, which this desktop does not reach              over SSH; local sync needs a host this machine can reach",
        ));
    }
    let script = crate::service::repo::repo_script(
        &w.tmux_name,
        &format!("printf '{ROOT_MARK}%s' \"$root\""),
    );
    let out = crate::ssh::run_shell(
        engine.ssh.as_ref(),
        &w.host_alias,
        &script,
        std::time::Duration::from_secs(30),
    )
    .await?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let remote_path = match stdout.rsplit_once(ROOT_MARK) {
        Some((_, root)) if out.status.success() && root.starts_with('/') => root.to_string(),
        _ => return Err(crate::service::repo::repo_err(&out)),
    };
    bind(
        engine,
        NewLink {
            host: w.host_alias,
            owner: w.owner,
            repo: w.repo,
            key: w.worktree_key,
            remote_path,
            local_path,
            session_id: None,
            excludes,
        },
    )
    .await
}

struct NewLink {
    host: String,
    owner: String,
    repo: String,
    key: String,
    remote_path: String,
    local_path: String,
    session_id: Option<i64>,
    excludes: Vec<String>,
}

/// Reach the worktree, make the folder, store the link and start its first
/// pass.
async fn bind(engine: &Arc<LocalSync>, l: NewLink) -> Result<LocalWorkspaceRow, IpcError> {
    let NewLink {
        host,
        owner,
        repo,
        key,
        remote_path,
        local_path,
        session_id,
        excludes,
    } = l;
    if host == crate::service::projects::LOCAL_HOST
        && crate::store::paths_overlap(&remote_path, &local_path)
    {
        return Err(IpcError::new(
            codes::E_INVALID,
            "the local folder cannot be the worktree itself, or inside or around it",
        ));
    }
    // Reach the worktree before promising anything: a missing folder or a
    // non-git one is refused here, not discovered by the first pass.
    remote::scan(engine.ssh.as_ref(), &host, &remote_path, &[]).await?;
    std::fs::create_dir_all(&local_path)
        .map_err(|e| IpcError::new(codes::E_IO, format!("create {local_path}: {e}")))?;
    let row = lock(&engine.store)?.insert_local_workspace(
        &NewLocalWorkspace {
            host_alias: &host,
            owner: &owner,
            repo: &repo,
            worktree_key: &key,
            remote_path: &remote_path,
            local_path: &local_path,
            session_id,
            excludes: &excludes,
        },
        now_secs(),
    )?;
    engine.kick(row.id);
    Ok(row)
}

/// An absolute local directory, without a trailing separator.
fn normalize_local_path(p: &str) -> Result<String, IpcError> {
    let p = p.trim();
    let expanded = match home_relative(p, cfg!(windows)) {
        Some(rest) => crate::home::home_dir()
            .map(|h| h.join(rest).to_string_lossy().into_owned())
            .unwrap_or_else(|| p.to_string()),
        None => p.to_string(),
    };
    let path = Path::new(&expanded);
    if !path.is_absolute() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "choose an absolute folder on this machine",
        ));
    }
    if path.exists() && !path.is_dir() {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("{expanded} is a file, not a folder"),
        ));
    }
    refuse_broad_folder(path, crate::home::home_dir().as_deref())?;
    let trimmed = expanded.trim_end_matches(['/', '\\']);
    Ok(if trimmed.is_empty() {
        expanded
    } else {
        trimmed.to_string()
    })
}

/// What follows the home folder's `~` in `p`: `~/rest`, and on Windows
/// `~\rest` too (review r18), the separator Explorer and a typed path use.
fn home_relative(p: &str, windows: bool) -> Option<&str> {
    p.strip_prefix("~/")
        .or_else(|| if windows { p.strip_prefix("~\\") } else { None })
}

/// Refuse a folder that is the filesystem root, the home folder, or any
/// folder above it. The first pass copies every local-only file to the
/// remote worktree, where an agent may commit and push it: `~/` (one slip in
/// the folder field) sent `~/.ssh`, `~/.aws` and every browser profile into
/// a repo, and wrote the remote's files back into the home folder. A path
/// with `..` is refused too, so the check sees the folder that is meant.
fn refuse_broad_folder(path: &Path, home: Option<&Path>) -> Result<(), IpcError> {
    use std::path::Component;
    if path.components().any(|c| c == Component::ParentDir) {
        return Err(IpcError::new(
            codes::E_INVALID,
            "choose the folder without '..' in its path",
        ));
    }
    // Resolved where it exists, so a symlink to the home folder is seen as it.
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if real.parent().is_none() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "choose a project folder, not the root of the disk",
        ));
    }
    if let Some(home) = home {
        let home = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
        if home.starts_with(&real) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "{} is your home folder or holds it; choose the project's own folder",
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

impl LocalSync {
    /// Start a pass in the background now.
    fn kick(self: &Arc<Self>, id: i64) {
        self.due.remove(&id);
        let me = Arc::clone(self);
        crate::rt::spawn(async move {
            let _ = me.sync_now(id).await;
        });
    }
}

pub fn pause(engine: &Arc<LocalSync>, id: i64) -> Result<LocalWorkspaceRow, IpcError> {
    lock(&engine.store)?.set_local_workspace_paused(id, true)
}

pub fn resume(engine: &Arc<LocalSync>, id: i64) -> Result<LocalWorkspaceRow, IpcError> {
    let row = lock(&engine.store)?.set_local_workspace_paused(id, false)?;
    engine.kick(id);
    Ok(row)
}

/// Drop the link and what it remembered. Files on either side stay.
pub async fn disconnect(engine: &Arc<LocalSync>, id: i64) -> Result<(), IpcError> {
    // Wait out a running pass, so it does not write after the row is gone.
    let pass = engine.pass_lock(id);
    let _guard = pass.lock().await;
    if !lock(&engine.store)?.delete_local_workspace(id)? {
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("no local workspace {id}"),
        ));
    }
    engine.due.remove(&id);
    engine.passes.remove(&id);
    Ok(())
}

pub fn set_excludes(
    engine: &Arc<LocalSync>,
    args: SetLocalWorkspaceExcludesArgs,
) -> Result<LocalWorkspaceRow, IpcError> {
    Excludes::new(&args.excludes)?;
    let row = lock(&engine.store)?.set_local_workspace_excludes(args.id, &args.excludes)?;
    engine.kick(args.id);
    Ok(row)
}

/// Record which side wins one conflict, then run a pass to carry it out.
pub async fn resolve_conflict(
    engine: &Arc<LocalSync>,
    args: ResolveLocalConflictArgs,
) -> Result<LocalWorkspaceRow, IpcError> {
    lock(&engine.store)?.set_local_conflict_resolution(args.id, &args.path, &args.keep)?;
    engine.sync_now(args.id).await
}
