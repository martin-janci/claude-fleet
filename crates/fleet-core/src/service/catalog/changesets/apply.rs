//! Assets M4: applying a card (spec, *Changesets* → Apply). A catalog card
//! (bootstrap, new, drift take_host) changes catalogs: its touched catalogs
//! are snapshotted, imported into, their layer files and scopes written,
//! `host_layers` updated, and one commit made per catalog — or, on any
//! failure, every checkout is reset and nothing is committed (R11, R12).
//!
//! Everything runs under [`APPLY_LOCK`], which every mutating authoring
//! action in this process also takes (PF7), and starts only from clean
//! checkouts. A writer the lock cannot see (the `fleet-hub` CLI, an author
//! session editing files) is handled by scope: the apply records every path
//! it writes, commits only those, and on failure undoes only those — and
//! only when HEAD and the tree show nothing foreign; otherwise it leaves
//! that catalog for a person. A catalog card never writes to a host.
//!
//! A host card (rollout, drift restore) plans the card's hosts and applies
//! only what R15 allows; SB6's automatic additive sync shares that path
//! (`auto_additive`). A Rollout and SB6 create, adopt, or update a copy the
//! planner verified untouched since fleet wrote it (Assets M5) — SB6's
//! update only for a managed copy that is just behind its catalog
//! (`sb6_due`). Neither ever overwrites a copy, and neither removes an
//! asset. The one deletion either makes is a moved asset's old location
//! (a re-pointed `install_as`, F3c's Codex move), backed up, and only when
//! the planner verified that old copy untouched too (`action_allowed`).
//! A restore — one asset, one host, picked by a person — may also
//! overwrite, with a backup. A card's host sync runs under its own
//! cancellation token, never registered, so `cancel_task` cannot stop it
//! (R27).

use super::rules::{gap_hash, LayerGap, NEEDS_A_LOOK, UPDATE};
use super::{
    is_open, ApplyGuard, CardKind, ChangesetView, Decider, HeldLine, HeldWhy, ItemAction,
    ItemOutcome, ItemParams, APPLY_LOCK, WITHDRAWN_PREFIX,
};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::catalog::import::slugify;
use crate::service::catalog::layer::{split_key, Axis, Layer};
use crate::service::catalog::model::{Header, Kind, Scope, TargetOverride};
use crate::service::catalog::sync::manifest::HostCopy;
use crate::service::catalog::sync::plan::{Action, ActionOp, HostPlan, SyncPlan};
use crate::service::catalog::sync::{self, ApplyArgs as SyncApplyArgs, PlanArgs};
use crate::service::catalog::validate::{check_layer_name, check_name};
use crate::service::catalog::{
    author, effective, registry, repo, CatalogTarget, ImportArgs, E_CATALOG_PARSE,
};
use crate::service::settings;
use crate::ssh::SshClient;
use crate::store::{
    now_unix, now_unix_ms, AppliedRecord, AssetInventoryRow, CatalogRow, ChangesetItemRow,
    ChangesetRow, HostLayerRow, NewChangesetItem, Store, TriageVerdictRow,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// `changesets { apply }`'s arguments.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApplyArgs {
    pub id: i64,
    /// The items to apply; `None` = every pending item but "needs a look"
    /// (R8). A drift card needs exactly one.
    #[serde(default)]
    pub positions: Option<Vec<i64>>,
}

/// Apply card `args.id` and answer it as it now stands.
pub async fn apply(
    args: ApplyArgs,
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
) -> Result<ChangesetView, IpcError> {
    let busy = APPLY_LOCK.lock().await;
    apply_held(&busy, args, store, ssh).await
}

/// [`apply`] for a caller already holding [`APPLY_LOCK`].
pub async fn apply_held(
    _busy: &ApplyGuard,
    args: ApplyArgs,
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
) -> Result<ChangesetView, IpcError> {
    let (card, items) = super::card(args.id, store)?;
    let selected = applicable(&card, &items, args.positions.as_deref())?;
    // Dispatch on what was selected (fix round 1): a restore is the one
    // item applied, whatever named it.
    if selected
        .iter()
        .any(|i| i.action == ItemAction::Restore.as_str())
    {
        if selected.len() != 1 {
            return Err(IpcError::new(
                codes::E_INVALID,
                "a restore applies alone: name only its position",
            ));
        }
        apply_restore(&card, &items, selected[0], store, ssh).await?;
    } else if card.kind == CardKind::Rollout.as_str() {
        apply_rollout(&card, &items, &selected, store, ssh).await?;
    } else {
        apply_catalog(&card, &items, &selected, store, ssh).await?;
    }
    super::get(args.id, store)
}

/// The items applying `positions` of this card runs, or why it applies
/// nothing: only an open card applies (R3), then [`select_items`]. The MCP
/// tool's grant check and confirm gate read this same selection under the
/// same `APPLY_LOCK`, so what is authorized is what runs.
pub fn applicable<'a>(
    card: &ChangesetRow,
    items: &'a [ChangesetItemRow],
    positions: Option<&[i64]>,
) -> Result<Vec<&'a ChangesetItemRow>, IpcError> {
    if !is_open(&card.state) {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "card {} is {}; only a proposed or failed card applies",
                card.id, card.state
            ),
        ));
    }
    select_items(card, items, positions)
}

/// R3, R8: the items this apply runs.
fn select_items<'a>(
    card: &ChangesetRow,
    items: &'a [ChangesetItemRow],
    positions: Option<&[i64]>,
) -> Result<Vec<&'a ChangesetItemRow>, IpcError> {
    let chosen: Vec<&ChangesetItemRow> = match positions {
        Some([]) => {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "no positions named for card {}: name the items to apply, or leave \
                     positions out to apply every pending item but \"needs a look\"",
                    card.id
                ),
            ))
        }
        Some(ps) => {
            let mut out: Vec<&ChangesetItemRow> = Vec::new();
            for p in ps {
                let item = items.iter().find(|i| i.position == *p).ok_or_else(|| {
                    IpcError::new(
                        codes::E_NOTFOUND,
                        format!("card {} has no item {p}", card.id),
                    )
                })?;
                if item.state != "pending" {
                    return Err(IpcError::new(
                        codes::E_INVALID_STATE,
                        format!("item {p} of card {} is {}", card.id, item.state),
                    ));
                }
                if !out.iter().any(|i| i.position == *p) {
                    out.push(item);
                }
            }
            out
        }
        None => items
            .iter()
            .filter(|i| i.state == "pending" && i.grp != NEEDS_A_LOOK)
            .collect(),
    };
    if card.kind == CardKind::Drift.as_str() && chosen.len() != 1 {
        return Err(IpcError::new(
            codes::E_INVALID,
            "a drift card applies one item at a time: name take_host or restore in positions",
        ));
    }
    if chosen.is_empty() {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "card {} has nothing to apply (a \"needs a look\" item applies only when \
                 named in positions)",
                card.id
            ),
        ));
    }
    for i in &chosen {
        if i.action != ItemAction::Hide.as_str() && i.catalog_id.is_none() {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "item {} names no catalog (removed since?); dismiss this card",
                    i.position
                ),
            ));
        }
    }
    Ok(chosen)
}

/// A failed step and the card group it failed in. Spec: the card is
/// `failed` "with the error on the failing group", so its error reads
/// `<group>: <error>`. The group is the failing item's `grp` (a layer name,
/// `needs a look`, `drift`, …) or, for a card-wide step that belongs to no
/// one group, a pseudo-group: `host layers` (step 3's store write), `commit`
/// (step 4, naming the catalog) or `record` (step 5's bookkeeping).
struct Failure {
    group: String,
    error: IpcError,
}

fn fail(item: &ChangesetItemRow, error: IpcError) -> Failure {
    fail_in(&item.grp, error)
}

fn fail_in(group: &str, error: IpcError) -> Failure {
    Failure {
        group: group.to_string(),
        error,
    }
}

fn kind_of(item: &ChangesetItemRow) -> Result<Kind, IpcError> {
    Kind::ALL
        .into_iter()
        .find(|k| k.as_str() == item.kind)
        .ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID,
                format!("item {} names no asset kind ({})", item.position, item.kind),
            )
        })
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// What this apply has done to the checkouts so far (PF7 guard, Task 6
/// review rounds 1–2): per catalog, every FILE it writes or deletes —
/// recorded BEFORE the write, so a write that fails half-way is still its
/// own — the directories it created, in order, and the commit it made. A
/// failed apply undoes exactly these and nothing else; a file it did not
/// record is never its own, whatever folder it sits in. Round 3: a file is
/// claimed only if it is absent, or tracked at the catalog's pre-apply HEAD
/// and still unchanged — so nobody's edit or new file is ever folded into
/// the apply's own.
#[derive(Debug, Default)]
struct Progress {
    /// Each touched catalog's HEAD before the apply.
    pre: BTreeMap<i64, String>,
    written: BTreeMap<i64, BTreeSet<String>>,
    dirs: BTreeMap<i64, Vec<String>>,
    commits: BTreeMap<i64, String>,
    /// Final review I2: imported files the checkout's ignore rules match,
    /// as `<catalog>: <path>` — never written, never named to `git add`;
    /// the applied card's note names them.
    ignored: Vec<String>,
}

impl Progress {
    /// Make `rel` this apply's own before its first write or delete: it must
    /// be absent, or tracked at `pre` and unchanged since; anything else —
    /// someone's edit, someone's new file, an ignored file at that path — is
    /// foreign, and nothing is written (`E_INVALID_STATE` naming the path).
    fn claim(&mut self, catalog_id: i64, root: &Path, rel: &str) -> Result<(), IpcError> {
        if self
            .written
            .get(&catalog_id)
            .is_some_and(|w| w.contains(rel))
        {
            return Ok(());
        }
        let present = std::fs::symlink_metadata(root.join(rel)).is_ok();
        if present {
            let pre = self.pre.get(&catalog_id).map(String::as_str).unwrap_or("");
            if !repo::unchanged_since(root, pre, rel)? {
                return Err(IpcError::new(
                    codes::E_INVALID_STATE,
                    format!("{rel} was changed during the apply, not by it; it is left as it is"),
                ));
            }
        }
        self.written
            .entry(catalog_id)
            .or_default()
            .insert(rel.to_string());
        Ok(())
    }

    /// `rel_dir` and its parents under `root`, recording each one made.
    fn make_dir(&mut self, catalog_id: i64, root: &Path, rel_dir: &str) -> Result<(), IpcError> {
        let mut cur = String::new();
        for part in rel_dir.split('/').filter(|p| !p.is_empty()) {
            if !cur.is_empty() {
                cur.push('/');
            }
            cur.push_str(part);
            if std::fs::symlink_metadata(root.join(&cur)).is_err() {
                std::fs::create_dir(root.join(&cur))?;
                self.dirs.entry(catalog_id).or_default().push(cur.clone());
            }
        }
        Ok(())
    }

    /// Claim `rel`, make its directory, write it.
    fn write_file(
        &mut self,
        catalog_id: i64,
        root: &Path,
        rel: &str,
        bytes: &[u8],
    ) -> Result<(), IpcError> {
        self.claim(catalog_id, root, rel)?;
        if let Some((dir, _)) = rel.rsplit_once('/') {
            self.make_dir(catalog_id, root, dir)?;
        }
        std::fs::write(root.join(rel), bytes)?;
        Ok(())
    }
}

async fn apply_catalog(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    selected: &[&ChangesetItemRow],
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    let touched: BTreeSet<i64> = selected
        .iter()
        .filter(|i| i.action != ItemAction::Hide.as_str())
        .filter_map(|i| i.catalog_id)
        .collect();
    let (rows, token) = {
        let s = lock(store)?;
        let mut rows = Vec::new();
        for id in &touched {
            rows.push(s.get_catalog(*id)?.ok_or_else(|| {
                IpcError::new(
                    codes::E_NOTFOUND,
                    format!("catalog {id} no longer exists; dismiss this card"),
                )
            })?);
        }
        (rows, s.get_setting(crate::mcp::SETTING_TOKEN)?)
    };
    // R11 / PF7: every touched catalog loaded and its checkout clean — under
    // APPLY_LOCK, which authoring also takes — or nothing happens and the
    // card stays as it is. Never reset over changes this card did not make.
    let mut pre: BTreeMap<i64, String> = BTreeMap::new();
    for row in &rows {
        registry::with_catalog_row(row, |_| Ok(())).map_err(|e| {
            IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "{}; card {} touches catalog {} and applies once it loads",
                    e.message, card.id, row.name
                ),
            )
        })?;
        let root = Path::new(&row.repo_path);
        if !repo::is_clean(root)? {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "catalog {} has uncommitted changes; commit them (catalog_admin \
                     commit_pending) or discard them before applying a card",
                    row.name
                ),
            ));
        }
        pre.insert(row.id, repo::head(root)?);
    }
    // 1. Snapshot the touched catalogs' host_layers (R11: theirs only).
    let snapshot: Vec<HostLayerRow> = lock(store)?
        .list_all_host_layers()?
        .into_iter()
        .filter(|r| touched.contains(&r.catalog_id))
        .collect();
    let mut progress = Progress {
        pre: pre.clone(),
        ..Default::default()
    };
    let steps = run_steps(
        card,
        selected,
        &rows,
        &pre,
        token.as_deref(),
        store,
        ssh,
        &mut progress,
    )
    .await;
    let outcome =
        steps.and_then(|()| record(card, items, selected, &progress.commits, &snapshot, store));
    match outcome {
        Ok(()) => {
            // Reloads, pushes and a reconcile pass: sync git and directory
            // walks, off the async workers (review r06).
            let (card, selected): (ChangesetRow, Vec<ChangesetItemRow>) = (
                card.clone(),
                selected.iter().map(|i| (*i).clone()).collect(),
            );
            let (commits, ignored) = (progress.commits.clone(), progress.ignored.clone());
            let store = Arc::clone(store);
            let done = tokio::task::spawn_blocking(move || {
                let selected: Vec<&ChangesetItemRow> = selected.iter().collect();
                after_commits(&card, &selected, &rows, &commits, &ignored, &store);
            })
            .await;
            if let Err(e) = done {
                tracing::warn!(error = %e, "an applied card's follow-through did not finish");
            }
            Ok(())
        }
        Err(failure) => Err(fail_card(
            card, items, &rows, &pre, &progress, &snapshot, failure, store,
        )),
    }
}

/// R12, best effort at every step: each touched checkout undone (only this
/// apply's own paths, and only when nothing foreign is there — else it is
/// left for a person and said so), host_layers restored, every pending item
/// back to pending and the card failed — the last two always, together —
/// with every cleanup problem appended to `<group>: <error>`. Answers the
/// error `apply` returns, which is the card's.
#[allow(clippy::too_many_arguments)]
fn fail_card(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    rows: &[CatalogRow],
    pre: &BTreeMap<i64, String>,
    progress: &Progress,
    snapshot: &[HostLayerRow],
    failure: Failure,
    store: &Mutex<Store>,
) -> IpcError {
    let mut problems: Vec<String> = rows
        .iter()
        .filter_map(|row| undo_catalog(row, &pre[&row.id], progress))
        .collect();
    let pending: Vec<i64> = items
        .iter()
        .filter(|i| i.state == "pending")
        .map(|i| i.position)
        .collect();
    let mut msg = format!("{}: {}", failure.group, failure.error.message);
    match lock(store) {
        Ok(s) => {
            for row in rows {
                if let Err(e) = s.restore_host_layers(row.id, snapshot) {
                    problems.push(format!(
                        "host layers of catalog {} could not be restored: {e} — fix by hand",
                        row.name
                    ));
                }
            }
            if !problems.is_empty() {
                msg = format!("{msg}; {}", problems.join("; "));
            }
            if let Err(e) = s.fail_changeset(card.id, &pending, &msg) {
                tracing::error!("card {}: could not be marked failed: {e}", card.id);
                msg = format!("{msg}; the card could not be marked failed: {e}");
            }
        }
        Err(e) => {
            problems.push(format!(
                "the store is unavailable ({}): host layers not restored, card not marked \
                 failed",
                e.message
            ));
            msg = format!("{msg}; {}", problems.join("; "));
        }
    }
    for p in &problems {
        tracing::error!("card {}: {p}", card.id);
    }
    IpcError::new(&failure.error.code, msg)
}

/// Undo this apply in one catalog — `None` when done (or nothing to undo),
/// else what a person must fix. The PF7 guard first: HEAD must be the
/// recorded pre-apply HEAD or this apply's own commit, and every change in
/// the tree — and every file beside one of its own, ignored ones included —
/// one of the files this apply wrote; anything foreign (a CLI commit, an
/// author session's file or edit) means this catalog is left exactly as it
/// is.
fn undo_catalog(row: &CatalogRow, pre: &str, progress: &Progress) -> Option<String> {
    let empty = BTreeSet::new();
    let ours = progress.written.get(&row.id).unwrap_or(&empty);
    let dirs = progress.dirs.get(&row.id).map(Vec::as_slice).unwrap_or(&[]);
    let own = progress.commits.get(&row.id);
    if ours.is_empty() && own.is_none() {
        return None;
    }
    let root = Path::new(&row.repo_path);
    // Round 3: HEAD at "our" commit is ours only when that commit sits
    // directly on the recorded pre-apply HEAD.
    if let Some(c) = own {
        match repo::parent_of(root, c) {
            Ok(parent) if parent == pre => {}
            Ok(parent) => {
                return Some(format!(
                    "manual cleanup needed in {}: this apply's commit {} sits on {}, not on {} \
                     (nothing there was reset)",
                    row.name,
                    short(c),
                    short(&parent),
                    short(pre)
                ))
            }
            Err(e) => {
                return Some(format!(
                    "catalog {} could not be checked before its reset: {} — fix by hand",
                    row.name, e.message
                ))
            }
        }
    }
    let mut heads = vec![pre];
    heads.extend(own.map(String::as_str));
    match repo::foreign_changes(root, &heads, ours) {
        Ok(foreign) if foreign.is_empty() => {
            repo::reset_paths(root, pre, ours, dirs).err().map(|e| {
                format!(
                    "catalog {} could not be reset to {}: {} — fix by hand",
                    row.name,
                    short(pre),
                    e.message
                )
            })
        }
        Ok(foreign) => Some(format!(
            "manual cleanup needed in {}: {} (nothing there was reset)",
            row.name,
            foreign.join(", ")
        )),
        Err(e) => Some(format!(
            "catalog {} could not be checked before its reset: {} — fix by hand",
            row.name, e.message
        )),
    }
}

/// Steps 2–4. Every error is a [`Failure`]; the caller undoes `progress`.
#[allow(clippy::too_many_arguments)]
async fn run_steps(
    card: &ChangesetRow,
    selected: &[&ChangesetItemRow],
    rows: &[CatalogRow],
    pre: &BTreeMap<i64, String>,
    token: Option<&str>,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    progress: &mut Progress,
) -> Result<(), Failure> {
    let row_of = |item: &ChangesetItemRow| -> Result<&CatalogRow, Failure> {
        item.catalog_id
            .and_then(|id| rows.iter().find(|r| r.id == id))
            .ok_or_else(|| {
                fail(
                    item,
                    IpcError::new(
                        codes::E_INVALID_STATE,
                        format!("item {} names no catalog", item.position),
                    ),
                )
            })
    };
    // Task 4 review M2: which named look items an org-bound host holds.
    let shares = {
        let s = lock(store).map_err(|e| fail_in(NEEDS_A_LOOK, e))?;
        look_shares(selected, rows, &s).map_err(|e| fail_in(NEEDS_A_LOOK, e))?
    };

    // 2a. Imports — one remote read per (catalog, source host) (R13).
    let mut imports: BTreeMap<(i64, String), Vec<&ChangesetItemRow>> = BTreeMap::new();
    for &item in selected.iter().filter(|i| {
        i.action == ItemAction::Import.as_str() || i.action == ItemAction::TakeHost.as_str()
    }) {
        let p = ItemParams::parse(item.params.as_deref());
        let host = if item.action == ItemAction::TakeHost.as_str() {
            p.host
        } else {
            p.from_host
        };
        let (Some(host), Some(cid)) = (host, item.catalog_id) else {
            return Err(fail(
                item,
                IpcError::new(
                    codes::E_INVALID,
                    format!("{}/{} names no source host", item.kind, item.name),
                ),
            ));
        };
        imports.entry((cid, host)).or_default().push(item);
    }
    // R13 / R15: each group is imported into a private staging directory
    // first, then copied into the checkout file by file, each file recorded
    // before it is written. take_host replaces the catalog copy's TRACKED
    // files (never an untracked or ignored one beside them) and keeps the
    // catalog header's metadata ([`keep_header`]) — and refuses when
    // someone else changed that asset meanwhile.
    let mut kept: Vec<(&CatalogRow, Kind, Header, &ChangesetItemRow)> = Vec::new();
    for ((_, host), group) in &imports {
        let row = row_of(group[0])?;
        let root = Path::new(&row.repo_path);
        let mut plan: Vec<(&ChangesetItemRow, Kind, String)> = Vec::new();
        for &item in group {
            let kind = kind_of(item).map_err(|e| fail(item, e))?;
            if item.action == ItemAction::TakeHost.as_str() {
                check_name(&item.name).map_err(|e| fail(item, e))?;
            } else if repo::asset_path(root, kind, &slugify(&item.name)).exists() {
                return Err(fail(
                    item,
                    IpcError::new(
                        codes::E_INVALID_STATE,
                        format!(
                            "{}/{} is already in catalog {}",
                            item.kind,
                            slugify(&item.name),
                            row.name
                        ),
                    ),
                ));
            }
            plan.push((item, kind, slugify(&item.name)));
        }
        let staging = tempfile::tempdir().map_err(|e| {
            fail(
                group[0],
                IpcError::new(codes::E_IO, format!("staging directory: {e}")),
            )
        })?;
        let staged = CatalogRow {
            repo_path: staging.path().to_string_lossy().to_string(),
            ..row.clone()
        };
        let only: Vec<String> = group
            .iter()
            .map(|i| format!("{}:{}", i.kind, i.name))
            .collect();
        let args = ImportArgs {
            host_alias: host.clone(),
            dry_run: false,
            only,
        };
        let report = crate::service::catalog::import_host_into(
            CatalogTarget::Row(&staged),
            args,
            store,
            ssh,
            token,
        )
        .await
        .map_err(|e| fail(group[0], e))?;
        for &item in group {
            if !report
                .created
                .contains(&(item.kind.clone(), slugify(&item.name)))
            {
                let why: Vec<&str> = report
                    .problems
                    .iter()
                    .map(|p| p.message.as_str())
                    .take(3)
                    .collect();
                let tail = if why.is_empty() {
                    String::new()
                } else {
                    format!(": {}", why.join("; "))
                };
                return Err(fail(
                    item,
                    IpcError::new(
                        codes::E_INVALID,
                        format!(
                            "{}/{} was not imported from {host}{tail}",
                            item.kind, item.name
                        ),
                    ),
                ));
            }
        }
        let started = pre[&row.id].as_str();
        for (item, kind, slug) in plan {
            let rel_asset = repo::asset_rel_path(kind, &slug);
            let mut new_files: BTreeSet<String> = if kind.is_folder() {
                repo::files_on_disk(staging.path(), &rel_asset)
                    .map_err(|e| fail(item, e))?
                    .into_iter()
                    .collect()
            } else {
                BTreeSet::from([rel_asset.clone()])
            };
            // Final review I2: a file the checkout ignores (a `.DS_Store`
            // under a global excludesFile) would make `git add` fail the
            // card. A clone would not have it anyway: a resource is left
            // out and named on the card; anything else the asset needs is
            // a failure that says why.
            let ignored = repo::ignored_paths(root, &new_files).map_err(|e| fail(item, e))?;
            let resources = format!("{rel_asset}/resources/");
            if let Some(needed) = ignored.iter().find(|p| !p.starts_with(&resources)) {
                return Err(fail(
                    item,
                    IpcError::new(
                        codes::E_INVALID_STATE,
                        format!(
                            "{needed} is ignored by catalog {}'s gitignore, so {}/{} cannot \
                             be committed; change the ignore rules, then apply again",
                            row.name, item.kind, item.name
                        ),
                    ),
                ));
            }
            for f in &ignored {
                new_files.remove(f);
                progress.ignored.push(format!("{}: {f}", row.name));
            }
            if item.action == ItemAction::TakeHost.as_str() {
                let inside = |p: &str| p == rel_asset || p.starts_with(&format!("{rel_asset}/"));
                let theirs: Vec<String> = repo::changed_paths(root)
                    .map_err(|e| fail(item, e))?
                    .into_iter()
                    .filter(|p| inside(p))
                    .collect();
                if !theirs.is_empty() {
                    return Err(fail(
                        item,
                        IpcError::new(
                            codes::E_INVALID_STATE,
                            format!(
                                "{}/{} changed in catalog {} during the apply (not by it): {}",
                                item.kind,
                                item.name,
                                row.name,
                                theirs.join(", ")
                            ),
                        ),
                    ));
                }
                let old = repo::read_asset(root, kind, &item.name).map_err(|e| fail(item, e))?;
                kept.push((row, kind, old.header, item));
                let tracked = repo::tracked_files(root, started, &[rel_asset.as_str()])
                    .map_err(|e| fail(item, e))?;
                for f in tracked.difference(&new_files) {
                    progress.claim(row.id, root, f).map_err(|e| fail(item, e))?;
                    match std::fs::remove_file(root.join(f)) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(fail(item, e.into())),
                    }
                }
            }
            for f in &new_files {
                let bytes = std::fs::read(staging.path().join(f))
                    .map_err(|e| fail(item, IpcError::from(e)))?;
                progress
                    .write_file(row.id, root, f, &bytes)
                    .map_err(|e| fail(item, e))?;
            }
        }
    }
    for (row, kind, old, item) in &kept {
        keep_header(progress, row, *kind, &item.name, old).map_err(|e| fail(item, e))?;
    }

    // 2b. Layer files: the members this card adds.
    let mut members: BTreeMap<(i64, String), BTreeSet<String>> = BTreeMap::new();
    for &item in selected
        .iter()
        .filter(|i| i.action == ItemAction::Import.as_str())
    {
        let p = ItemParams::parse(item.params.as_deref());
        if let (Some(cid), Some(layer), Some(member)) = (item.catalog_id, p.layer, p.member) {
            members.entry((cid, layer)).or_default().insert(member);
        }
    }
    for ((cid, layer), add) in &members {
        let row = rows
            .iter()
            .find(|r| r.id == *cid)
            .ok_or_else(|| fail_in(layer, IpcError::new(codes::E_INVALID_STATE, "no catalog")))?;
        let root = Path::new(&row.repo_path);
        check_layer_name(layer).map_err(|e| fail_in(layer, e))?;
        let yaml = layer_with_members(root, layer, add).map_err(|e| fail_in(layer, e))?;
        progress
            .write_file(
                row.id,
                root,
                &format!("layers/{layer}.yaml"),
                yaml.as_bytes(),
            )
            .map_err(|e| fail_in(layer, e))?;
    }

    // 2c. Scope: the card's set_scope items, then the named look items an
    //     org-bound host holds (adopted into personal, so `shared` keeps
    //     them on that host).
    for &item in selected
        .iter()
        .filter(|i| i.action == ItemAction::SetScope.as_str())
    {
        let p = ItemParams::parse(item.params.as_deref());
        let row = row_of(item)?;
        let member = p
            .member
            .clone()
            .unwrap_or_else(|| format!("{}/{}", item.kind, slugify(&item.name)));
        let (kind, name) = split_key(&member).ok_or_else(|| {
            fail(
                item,
                IpcError::new(codes::E_INVALID, format!("bad member {member}")),
            )
        })?;
        check_name(&name).map_err(|e| fail(item, e))?;
        let scope = match p.scope.as_deref() {
            Some("shared") => Scope::Shared,
            Some("private") => Scope::Private,
            other => {
                return Err(fail(
                    item,
                    IpcError::new(codes::E_INVALID, format!("bad scope {other:?}")),
                ))
            }
        };
        set_scope(progress, row, kind, &name, scope).map_err(|e| fail(item, e))?;
    }
    for &item in &shares {
        let row = row_of(item)?;
        let kind = kind_of(item).map_err(|e| fail(item, e))?;
        set_scope(progress, row, kind, &slugify(&item.name), Scope::Shared)
            .map_err(|e| fail(item, e))?;
    }

    // 2d. Layer cards (Assets M6, R5): create / rename / move, every path
    //     claimed before it is written or deleted.
    let renames = apply_layer_items(selected, rows, progress)?;

    // 3. host_layers: append this card's contexts, keeping each host's role
    //    and the order of what it had. A layer the checkout does not define
    //    (its imports were rejected or deselected) is never assigned.
    {
        let mut adds: BTreeMap<(String, i64), Vec<&ChangesetItemRow>> = BTreeMap::new();
        for &item in selected
            .iter()
            .filter(|i| i.action == ItemAction::AssignLayer.as_str())
        {
            let p = ItemParams::parse(item.params.as_deref());
            let (Some(host), Some(cid)) = (p.host, item.catalog_id) else {
                return Err(fail(
                    item,
                    IpcError::new(codes::E_INVALID, "assign_layer names no host"),
                ));
            };
            let row = row_of(item)?;
            if !Path::new(&row.repo_path)
                .join("layers")
                .join(format!("{}.yaml", item.name))
                .is_file()
            {
                return Err(fail(
                    item,
                    IpcError::new(
                        codes::E_INVALID,
                        format!(
                            "catalog {} has no layer {} to assign to {host} (were its imports \
                             left out?)",
                            row.name, item.name
                        ),
                    ),
                ));
            }
            adds.entry((host, cid)).or_default().push(item);
        }
        let s = lock(store).map_err(|e| fail_in("host layers", e))?;
        for ((host, cid), layers) in adds {
            let group = layers[0].grp.clone();
            let current = s
                .get_host_layers_for(&host, cid)
                .map_err(|e| fail_in(&group, e.into()))?;
            let role = current
                .iter()
                .find(|r| r.axis == "role")
                .map(|r| r.layer_name.clone());
            let mut contexts: Vec<String> = current
                .iter()
                .filter(|r| r.axis == "context")
                .map(|r| r.layer_name.clone())
                .collect();
            for l in layers {
                if role.as_ref() != Some(&l.name) && !contexts.contains(&l.name) {
                    contexts.push(l.name.clone());
                }
            }
            let ctx: Vec<&str> = contexts.iter().map(String::as_str).collect();
            s.set_host_layers_for(&host, cid, role.as_deref(), &ctx)
                .map_err(|e| fail_in(&group, e.into()))?;
        }
        // Assets M6 (R5): a renamed layer's rows follow it. Only active rows
        // are read and written back — `set_host_layers_for` rewrites the
        // host's rows in that catalog, as the assignments above do, and
        // nothing writes an inactive row; a host whose only row naming the
        // old layer is inactive is left as it is. Step 1's snapshot covers
        // this catalog, so a failure or an undo puts the rows back.
        for (cid, old, new) in &renames {
            let hosts: BTreeSet<String> = s
                .list_all_host_layers()
                .map_err(|e| fail_in(old, e.into()))?
                .into_iter()
                .filter(|r| r.active && r.catalog_id == *cid && &r.layer_name == old)
                .map(|r| r.host_alias)
                .collect();
            for host in hosts {
                let current = s
                    .get_host_layers_for(&host, *cid)
                    .map_err(|e| fail_in(old, e.into()))?;
                let swap = |n: &str| if n == old { new.clone() } else { n.to_string() };
                let role = current
                    .iter()
                    .find(|r| r.axis == "role")
                    .map(|r| swap(&r.layer_name));
                let mut contexts: Vec<String> = Vec::new();
                for r in current.iter().filter(|r| r.axis == "context") {
                    let name = swap(&r.layer_name);
                    if role.as_ref() != Some(&name) && !contexts.contains(&name) {
                        contexts.push(name);
                    }
                }
                let ctx: Vec<&str> = contexts.iter().map(String::as_str).collect();
                s.set_host_layers_for(&host, *cid, role.as_deref(), &ctx)
                    .map_err(|e| fail_in(old, e.into()))?;
            }
        }
    }

    // 4. One commit per touched catalog (`fleet: <card summary>`), of the
    //    paths this apply wrote and nothing else (PF7) — and only on the
    //    HEAD the apply started from.
    for row in rows {
        let root = Path::new(&row.repo_path);
        let commit_fail = |e: IpcError| {
            fail_in(
                "commit",
                IpcError::new(&e.code, format!("catalog {}: {}", row.name, e.message)),
            )
        };
        let now = repo::head(root).map_err(commit_fail)?;
        let started = pre[&row.id].as_str();
        if now != started {
            return Err(commit_fail(IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "HEAD moved from {} to {} during the apply",
                    short(started),
                    short(&now)
                ),
            )));
        }
        let ours = progress.written.get(&row.id).cloned().unwrap_or_default();
        if let Some(sha) = repo::commit_paths(root, &format!("fleet: {}", card.summary), &ours)
            .map_err(commit_fail)?
        {
            progress.commits.insert(row.id, sha);
        }
    }
    Ok(())
}

/// A rename step 3 applies to host_layers: (catalog id, old name, new name).
type Rename = (i64, String, String);

/// What one layer item does to its catalog's checkout: files to write
/// (path, content) and to delete, and the rename step 3 carries to
/// host_layers.
#[derive(Debug, Default)]
struct LayerEdits {
    writes: Vec<(String, String)>,
    deletes: Vec<String>,
    rename: Option<(String, String)>,
}

fn is_layer_action(action: &str) -> bool {
    [
        ItemAction::CreateLayer,
        ItemAction::RenameLayer,
        ItemAction::MoveMember,
    ]
    .iter()
    .any(|a| a.as_str() == action)
}

fn layer_rel(name: &str) -> String {
    format!("layers/{name}.yaml")
}

/// Whether anything — a file, a directory, a symlink, even a dangling one —
/// is at `rel`.
fn occupied(root: &Path, rel: &str) -> bool {
    std::fs::symlink_metadata(root.join(rel)).is_ok()
}

/// `layers/<name>.yaml` as the checkout has it — a file a layer card will
/// rewrite or delete, so never a symlink (a write would land wherever it
/// points).
fn read_layer_file(root: &Path, name: &str) -> Result<Layer, IpcError> {
    let rel = layer_rel(name);
    if std::fs::symlink_metadata(root.join(&rel)).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("{rel} is a symlink; a card never writes through one"),
        ));
    }
    let text = std::fs::read_to_string(root.join(&rel)).map_err(|_| {
        IpcError::new(
            codes::E_INVALID_STATE,
            format!("no layer {name} in this catalog"),
        )
    })?;
    Layer::from_yaml(&text).map_err(|e| IpcError::new(E_CATALOG_PARSE, format!("{rel}: {e}")))
}

/// A layer as its file will read, checked first: a layer that fails
/// `validate` would be dropped at the next load.
fn layer_yaml(l: &Layer) -> Result<String, IpcError> {
    l.validate()
        .map_err(|e| IpcError::new(codes::E_INVALID, e))?;
    Ok(l.to_yaml())
}

/// What `item` (a layer card's one item) writes and deletes in `row`'s
/// checkout at `root`. Reads only: every file it needs is read and every
/// result checked before step 2d changes anything. Names are checked again
/// here — they come from the store, and become paths.
fn layer_edits(
    item: &ChangesetItemRow,
    row: &CatalogRow,
    root: &Path,
) -> Result<LayerEdits, IpcError> {
    let p = ItemParams::parse(item.params.as_deref());
    let already = |name: &str| {
        IpcError::new(
            codes::E_INVALID_STATE,
            format!("catalog {} already has a layer {name}", row.name),
        )
    };
    let mut edits = LayerEdits::default();
    match item.action.as_str() {
        a if a == ItemAction::CreateLayer.as_str() => {
            check_layer_name(&item.name)?;
            if occupied(root, &layer_rel(&item.name)) {
                return Err(already(&item.name));
            }
            let axis = super::layers::parse_axis(p.axis.as_deref())?;
            let l = super::layers::new_layer(&item.name, axis, p.description, p.members, p.orgs)?;
            edits.writes.push((layer_rel(&item.name), layer_yaml(&l)?));
        }
        a if a == ItemAction::RenameLayer.as_str() => {
            let old = item.name.as_str();
            let to =
                p.to.ok_or_else(|| IpcError::new(codes::E_INVALID, "rename names no target"))?;
            check_layer_name(old)?;
            check_layer_name(&to)?;
            if old == to {
                return Err(already(&to));
            }
            if occupied(root, &layer_rel(&to)) {
                return Err(already(&to));
            }
            let mut l = read_layer_file(root, old)?;
            l.name = to.clone();
            edits.writes.push((layer_rel(&to), layer_yaml(&l)?));
            edits.deletes.push(layer_rel(old));
            // Its children in this catalog extend it by name: each one
            // follows, or the next load would drop it as extending an
            // unknown layer.
            let mut stems: Vec<String> = Vec::new();
            for entry in std::fs::read_dir(root.join("layers"))? {
                let path = entry?.path();
                if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                    continue;
                }
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if stem != old {
                        stems.push(stem.to_string());
                    }
                }
            }
            stems.sort();
            for stem in stems {
                let mut child = read_layer_file(root, &stem)?;
                if child.extends.as_deref() == Some(old) {
                    child.extends = Some(to.clone());
                    edits.writes.push((layer_rel(&stem), layer_yaml(&child)?));
                }
            }
            edits.rename = Some((old.to_string(), to));
        }
        a if a == ItemAction::MoveMember.as_str() => {
            let (Some(member), Some(from), Some(to)) = (p.member, p.layer, p.to) else {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    "move names no member, layer or target",
                ));
            };
            check_layer_name(&from)?;
            check_layer_name(&to)?;
            if from == to {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("layer {to} already has {member}"),
                ));
            }
            let mut a = read_layer_file(root, &from)?;
            let mut b = read_layer_file(root, &to)?;
            if !a.members.contains(&member) {
                return Err(IpcError::new(
                    codes::E_INVALID_STATE,
                    format!("layer {from} no longer has {member}"),
                ));
            }
            a.members.retain(|m| m != &member);
            // The member's override goes with it (final review minor 1);
            // one the target already holds for it is left as it is.
            if let Some(o) = a.overrides.remove(&member) {
                b.overrides.entry(member.clone()).or_insert(o);
            }
            if !b.members.contains(&member) {
                b.members.push(member);
            }
            edits.writes.push((layer_rel(&from), layer_yaml(&a)?));
            edits.writes.push((layer_rel(&to), layer_yaml(&b)?));
        }
        other => {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("{other} is not a layer action"),
            ))
        }
    }
    Ok(edits)
}

/// Step 2d (Assets M6, R5): each selected layer item's files. Every path is
/// claimed (`Progress::claim`) before the item writes or deletes anything,
/// so a file someone changed stops the item before its first change; then
/// the writes, then the deletes. Answers the renames for step 3.
fn apply_layer_items(
    selected: &[&ChangesetItemRow],
    rows: &[CatalogRow],
    progress: &mut Progress,
) -> Result<Vec<Rename>, Failure> {
    let mut renames = Vec::new();
    for &item in selected.iter().filter(|i| is_layer_action(&i.action)) {
        let row = item
            .catalog_id
            .and_then(|id| rows.iter().find(|r| r.id == id))
            .ok_or_else(|| {
                fail(
                    item,
                    IpcError::new(
                        codes::E_INVALID_STATE,
                        format!("item {} names no catalog", item.position),
                    ),
                )
            })?;
        let root = Path::new(&row.repo_path);
        let edits = layer_edits(item, row, root).map_err(|e| fail(item, e))?;
        for rel in edits
            .writes
            .iter()
            .map(|(rel, _)| rel)
            .chain(&edits.deletes)
        {
            progress
                .claim(row.id, root, rel)
                .map_err(|e| fail(item, e))?;
        }
        for (rel, yaml) in &edits.writes {
            progress
                .write_file(row.id, root, rel, yaml.as_bytes())
                .map_err(|e| fail(item, e))?;
        }
        for rel in &edits.deletes {
            match std::fs::remove_file(root.join(rel)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(fail(item, e.into())),
            }
        }
        if let Some((old, new)) = edits.rename {
            renames.push((row.id, old, new));
        }
    }
    Ok(renames)
}

/// Task 4 review M2: the selected "needs a look" imports bound for personal
/// that an org-bound host holds — a look item has no `set_scope` companion,
/// so the apply writes `scope: shared` for them (unless the card already
/// sets their scope).
fn look_shares<'a>(
    selected: &[&'a ChangesetItemRow],
    rows: &[CatalogRow],
    s: &Store,
) -> Result<Vec<&'a ChangesetItemRow>, IpcError> {
    let personal = |cid: Option<i64>| rows.iter().any(|r| Some(r.id) == cid && r.org_id.is_none());
    let looks: Vec<&ChangesetItemRow> = selected
        .iter()
        .copied()
        .filter(|i| {
            i.grp == NEEDS_A_LOOK
                && i.action == ItemAction::Import.as_str()
                && personal(i.catalog_id)
        })
        .collect();
    if looks.is_empty() {
        return Ok(Vec::new());
    }
    let org_hosts: BTreeSet<String> = s
        .list_hosts()?
        .into_iter()
        .filter(|h| h.org_id.is_some())
        .map(|h| h.alias)
        .collect();
    let inventory = s.list_inventory()?;
    let scoped: BTreeSet<(Option<i64>, String)> = selected
        .iter()
        .filter(|i| i.action == ItemAction::SetScope.as_str())
        .map(|i| {
            let p = ItemParams::parse(i.params.as_deref());
            let member = p
                .member
                .unwrap_or_else(|| format!("{}/{}", i.kind, slugify(&i.name)));
            (i.catalog_id, member)
        })
        .collect();
    Ok(looks
        .into_iter()
        .filter(|i| {
            let slug = slugify(&i.name);
            !scoped.contains(&(i.catalog_id, format!("{}/{slug}", i.kind)))
                && inventory.iter().any(|r| {
                    org_hosts.contains(&r.host_alias)
                        && r.kind == i.kind
                        && slugify(&r.name) == slug
                })
        })
        .collect())
}

/// Final review I5: after a take_host copied the host's files over the
/// catalog copy, put back the catalog header's metadata. The host copy
/// supplies the content — body, resources, the kind's fields, the
/// description (part of the rendered file the drift was measured on) and
/// Claude's own target fields (`targets.claude.model` / `extra`, which come
/// from its frontmatter). Everything else is the catalog's and is kept:
/// `scope`, `version`, `tags`, `install_as`, `source`, every other
/// harness's `targets` entry (`codex.enabled: false` stays) and Claude's
/// `enabled` / `render_as`. Rewrites the yaml file only (the one file it
/// records), and only when that changes it.
fn keep_header(
    progress: &mut Progress,
    row: &CatalogRow,
    kind: Kind,
    name: &str,
    old: &Header,
) -> Result<(), IpcError> {
    let root = Path::new(&row.repo_path);
    let mut a = repo::read_asset(root, kind, name)?;
    let mut header = old.clone();
    header.description = a.header.description.clone();
    let host_claude = a.header.targets.remove("claude");
    let claude = header.targets.entry("claude".to_string()).or_default();
    match host_claude {
        Some(t) => {
            claude.model = t.model;
            claude.extra = t.extra;
        }
        None => {
            claude.model = None;
            claude.extra.clear();
        }
    }
    if header.targets.get("claude") == Some(&TargetOverride::default()) {
        header.targets.remove("claude");
    }
    if a.header != header {
        a.header = header;
        let rel = if kind.is_folder() {
            format!("{}/asset.yaml", repo::asset_rel_path(kind, name))
        } else {
            repo::asset_rel_path(kind, name)
        };
        progress.write_file(row.id, root, &rel, a.to_yaml().as_bytes())?;
    }
    Ok(())
}

/// Set one asset's `scope`, rewriting its yaml file only (the one file it
/// records) — never its body or resources.
fn set_scope(
    progress: &mut Progress,
    row: &CatalogRow,
    kind: Kind,
    name: &str,
    scope: Scope,
) -> Result<(), IpcError> {
    let root = Path::new(&row.repo_path);
    let mut a = repo::read_asset(root, kind, name)?;
    if a.header.scope != scope {
        a.header.scope = scope;
        let rel = if kind.is_folder() {
            format!("{}/asset.yaml", repo::asset_rel_path(kind, name))
        } else {
            repo::asset_rel_path(kind, name)
        };
        progress.write_file(row.id, root, &rel, a.to_yaml().as_bytes())?;
    }
    Ok(())
}

/// `layers/<name>.yaml` with `add` added — a new context layer when there is
/// none (R6). Never removes a member. The caller writes it (through
/// `Progress::write_file`, so a file someone changed meanwhile is refused).
fn layer_with_members(root: &Path, name: &str, add: &BTreeSet<String>) -> Result<String, IpcError> {
    check_layer_name(name)?;
    let path = root.join("layers").join(format!("{name}.yaml"));
    let mut layer = if path.is_file() {
        Layer::from_yaml(&std::fs::read_to_string(&path)?)
            .map_err(|e| IpcError::new(E_CATALOG_PARSE, format!("layers/{name}.yaml: {e}")))?
    } else {
        author::layer_template(name, Axis::Context)
    };
    for m in add {
        if !layer.members.contains(m) {
            layer.members.push(m.clone());
        }
    }
    layer
        .validate()
        .map_err(|e| IpcError::new(codes::E_INVALID, e))?;
    Ok(layer.to_yaml())
}

/// Step 5's bookkeeping, after every commit landed, in ONE store
/// transaction: `ignored` verdicts for hide items (decider `rule`, R10 —
/// the rule that proposed the hide decided it, whoever pressed apply), the
/// items' states, the card applied (`applied_at` in ms, PF13). A failure
/// here is still a card failure — pseudo-group `record` — and the commits
/// are undone (R12).
fn record(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    selected: &[&ChangesetItemRow],
    commits: &BTreeMap<i64, String>,
    snapshot: &[HostLayerRow],
    store: &Mutex<Store>,
) -> Result<(), Failure> {
    let bookkeeping = |e: IpcError| fail_in("record", e);
    let now = now_unix();
    let verdicts: Vec<TriageVerdictRow> = selected
        .iter()
        .filter(|i| i.action == ItemAction::Hide.as_str())
        .map(|item| TriageVerdictRow {
            catalog_id: None,
            kind: item.kind.clone(),
            name: item.name.clone(),
            content_hash: ItemParams::parse(item.params.as_deref())
                .hash
                .unwrap_or_else(|| "-".into()),
            verdict: "ignored".into(),
            decider: Decider::Rule.as_str().into(),
            decided_at: now,
        })
        .collect();
    let applied: Vec<i64> = selected.iter().map(|i| i.position).collect();
    let skipped: Vec<i64> = items
        .iter()
        .filter(|i| i.state == "pending" && !applied.contains(&i.position))
        .map(|i| i.position)
        .collect();
    let commits_json: BTreeMap<String, String> = commits
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    let encode =
        |e: serde_json::Error| bookkeeping(IpcError::new(codes::E_SERIALIZE, e.to_string()));
    let commits_json = serde_json::to_string(&commits_json).map_err(encode)?;
    let snapshot_json = serde_json::to_string(snapshot).map_err(encode)?;
    lock(store)
        .map_err(bookkeeping)?
        .record_changeset_applied(
            card.id,
            &AppliedRecord {
                applied_at: now_unix_ms(),
                commits: &commits_json,
                layers_snapshot: &snapshot_json,
                applied: &applied,
                skipped: &skipped,
                verdicts: &verdicts,
                error: None,
            },
        )
        .map_err(|e| bookkeeping(e.into()))
}

/// Step 5's follow-through on an applied card: reload, push when
/// `catalog.auto_push` (SB4, R18), re-derive the cards a layer card made
/// stale (final review I2), propose the follow-up Rollout (R14).
/// None of these un-applies the card: each failure is a warning in its
/// `error` (R12), as are the imported files the checkout ignores
/// (`ignored`, final review I2), which were left out.
fn after_commits(
    card: &ChangesetRow,
    selected: &[&ChangesetItemRow],
    rows: &[CatalogRow],
    commits: &BTreeMap<i64, String>,
    ignored: &[String],
    store: &Mutex<Store>,
) {
    let mut warnings = Vec::new();
    if !ignored.is_empty() {
        warnings.push(format!(
            "not imported, as the catalog's gitignore matches them: {}",
            ignored.join(", ")
        ));
    }
    for row in rows {
        if let Err(e) = crate::service::catalog::load_catalog(row.id, false, store) {
            warnings.push(format!("reload {}: {}", row.name, e.message));
        }
    }
    let auto_push = match lock(store) {
        Ok(s) => settings::get_bool(&s, settings::CATALOG_AUTO_PUSH),
        Err(e) => {
            warnings.push(format!("read catalog.auto_push: {}", e.message));
            false
        }
    };
    if auto_push {
        for row in rows.iter().filter(|r| commits.contains_key(&r.id)) {
            if let Err(e) = repo::push(Path::new(&row.repo_path)) {
                warnings.push(format!("push {}: {}", row.name, e.message));
            }
        }
    }
    if card.kind == CardKind::Layer.as_str() {
        warnings.extend(rederive_after_layer_card(selected, store));
    }
    let written = lock(store).and_then(|s| {
        propose_follow_up(selected, &s)?;
        if !warnings.is_empty() {
            s.set_changeset_state(card.id, "applied", Some(&warnings.join("; ")))?;
        }
        Ok(())
    });
    if let Err(e) = written {
        tracing::warn!(
            "card {}: applied, but its follow-up could not be recorded: {}",
            card.id,
            e.message
        );
    }
}

/// Final review I2: after a layer card (create, rename, move) the cards
/// derived from the old layers are stale. A rename withdraws the open
/// Rollout cards of its catalog that sync the old layer — the layer they
/// name is gone — and then one reconcile pass re-derives the New and
/// Bootstrap cards (refreshed or withdrawn, as the pass decides), so none
/// adopts into a layer that no longer exists. Called after the reloads,
/// with `APPLY_LOCK` held by the apply (the pass itself takes no lock but
/// the store's, store before registry, never held together). Answers the
/// card note's warnings; best effort, like the rest of the follow-through.
fn rederive_after_layer_card(selected: &[&ChangesetItemRow], store: &Mutex<Store>) -> Vec<String> {
    let mut warnings = Vec::new();
    let renames: Vec<(i64, &str, String)> = selected
        .iter()
        .filter(|i| i.action == ItemAction::RenameLayer.as_str())
        .filter_map(|i| {
            let to = ItemParams::parse(i.params.as_deref()).to?;
            Some((i.catalog_id?, i.name.as_str(), to))
        })
        .collect();
    if !renames.is_empty() {
        let withdrawn = lock(store).and_then(|s| {
            for card in s.list_changesets()? {
                if card.kind != CardKind::Rollout.as_str() || !is_open(&card.state) {
                    continue;
                }
                let items = s.changeset_items(card.id)?;
                let hit = renames.iter().find(|(cid, old, _)| {
                    items.iter().any(|i| {
                        i.catalog_id == Some(*cid)
                            && (i.grp == *old
                                || ItemParams::parse(i.params.as_deref()).layer.as_deref()
                                    == Some(*old))
                    })
                });
                if let Some((_, old, new)) = hit {
                    s.withdraw_changeset(
                        card.id,
                        &format!("{WITHDRAWN_PREFIX} layer {old} was renamed to {new}"),
                    )?;
                }
            }
            Ok(())
        });
        if let Err(e) = withdrawn {
            warnings.push(format!(
                "the old layer's Rollout cards could not be withdrawn: {}",
                e.message
            ));
        }
    }
    let pass = lock(store)
        .map(|s| settings::get_bool(&s, settings::CATALOG_AUTO))
        .and_then(|auto| super::reconcile::reconcile(store, auto));
    if let Err(e) = pass {
        warnings.push(format!(
            "the cards could not be re-derived (the next pass will): {}",
            e.message
        ));
    }
    warnings
}

/// R14 with PF11: propose the Rollout `selected` calls for. Each of its
/// layers that an open Rollout card already names (one with no applied
/// item) is merged into that card — assets unioned per host, gap hashes
/// recomputed, an unchanged rejected item kept rejected — instead of
/// opening a second card for it; the rest becomes one new card.
pub(crate) fn propose_follow_up(selected: &[&ChangesetItemRow], s: &Store) -> Result<(), IpcError> {
    let Some((_, mut follow)) = follow_up_rollout(selected, s)? else {
        return Ok(());
    };
    for card in s.list_changesets()? {
        if follow.is_empty() {
            break;
        }
        if card.kind != CardKind::Rollout.as_str() || !is_open(&card.state) {
            continue;
        }
        let existing = s.changeset_items(card.id)?;
        if existing.iter().any(|i| i.state == "applied") {
            continue;
        }
        let groups: BTreeSet<(Option<i64>, &str)> = existing
            .iter()
            .map(|i| (i.catalog_id, i.grp.as_str()))
            .collect();
        let (mine, rest): (Vec<NewChangesetItem>, Vec<NewChangesetItem>) = follow
            .into_iter()
            .partition(|i| groups.contains(&(i.catalog_id, i.grp.as_str())));
        follow = rest;
        if mine.is_empty() {
            continue;
        }
        let merged = merge_sync_items(&existing, mine);
        let summary = rollout_summary(&merged);
        s.replace_changeset_items_keeping(card.id, &summary, &merged, |old| {
            old.iter()
                .enumerate()
                .filter(|(n, o)| {
                    o.state == "rejected"
                        && merged.get(*n).is_some_and(|m| {
                            ItemParams::parse(m.params.as_deref()).assets
                                == ItemParams::parse(o.params.as_deref()).assets
                        })
                })
                .map(|(n, _)| n as i64)
                .collect()
        })?;
    }
    if !follow.is_empty() {
        s.insert_changeset(
            CardKind::Rollout.as_str(),
            &rollout_summary(&follow),
            &follow,
        )?;
    }
    Ok(())
}

/// `existing` (in order) with `add` folded in: a sync item for a host the
/// card already has gains the new assets; any other is appended.
fn merge_sync_items(
    existing: &[ChangesetItemRow],
    add: Vec<NewChangesetItem>,
) -> Vec<NewChangesetItem> {
    let mut out: Vec<NewChangesetItem> = existing.iter().map(NewChangesetItem::from).collect();
    for new in add {
        match out.iter_mut().find(|o| {
            o.action == new.action
                && o.catalog_id == new.catalog_id
                && o.grp == new.grp
                && o.name == new.name
        }) {
            Some(o) => {
                let mut p = ItemParams::parse(o.params.as_deref());
                let assets: BTreeSet<String> = p
                    .assets
                    .drain(..)
                    .chain(ItemParams::parse(new.params.as_deref()).assets)
                    .collect();
                p.assets = assets.into_iter().collect();
                o.params = p.to_json();
            }
            None => out.push(new),
        }
    }
    stamp_gap_hashes(&mut out);
    out
}

/// PF10: every sync item carries its layer's gap hash — the hash the rules
/// hold a Rollout verdict by (`("layer", "<catalog>/<layer>", gap_hash)`),
/// so dismissing a follow-up holds like dismissing a rule-made card.
pub(super) fn stamp_gap_hashes(items: &mut [NewChangesetItem]) {
    let mut by_layer: BTreeMap<(Option<i64>, String), Vec<LayerGap>> = BTreeMap::new();
    for i in items.iter() {
        by_layer
            .entry((i.catalog_id, i.grp.clone()))
            .or_default()
            .push(LayerGap {
                catalog_id: i.catalog_id.unwrap_or_default(),
                layer: i.grp.clone(),
                host: i.name.clone(),
                assets: ItemParams::parse(i.params.as_deref()).assets,
            });
    }
    let hashes: BTreeMap<(Option<i64>, String), String> = by_layer
        .into_iter()
        .map(|(k, gaps)| (k, gap_hash(&gaps.iter().collect::<Vec<_>>())))
        .collect();
    for i in items.iter_mut() {
        let mut p = ItemParams::parse(i.params.as_deref());
        p.hash = hashes.get(&(i.catalog_id, i.grp.clone())).cloned();
        i.params = p.to_json();
    }
}

/// "Roll out <layers> to <hosts>", the rules' wording for one layer.
pub(super) fn rollout_summary(items: &[NewChangesetItem]) -> String {
    let groups: BTreeSet<&str> = items.iter().map(|i| i.grp.as_str()).collect();
    let hosts: BTreeSet<&str> = items.iter().map(|i| i.name.as_str()).collect();
    format!(
        "Roll out {} to {}",
        groups.into_iter().collect::<Vec<_>>().join(", "),
        hosts.into_iter().collect::<Vec<_>>().join(", ")
    )
}

/// R14: the Rollout an applied catalog card calls for — per (host assigned
/// the layer, layer) the members it added; per other host managing a
/// take_host asset, that asset (group `update`). Every item carries its
/// layer's gap hash (PF10); a layer whose hash a verdict already holds is
/// left out, as the rules leave it out. `None` when no host needs anything.
pub(crate) fn follow_up_rollout(
    selected: &[&ChangesetItemRow],
    s: &Store,
) -> Result<Option<(String, Vec<NewChangesetItem>)>, IpcError> {
    let mut added: BTreeMap<(i64, String), BTreeSet<String>> = BTreeMap::new();
    let mut taken: BTreeMap<(i64, String), String> = BTreeMap::new();
    for &item in selected {
        let p = ItemParams::parse(item.params.as_deref());
        let Some(cid) = item.catalog_id else { continue };
        if item.action == ItemAction::Import.as_str() {
            if let (Some(layer), Some(member)) = (p.layer, p.member) {
                added.entry((cid, layer)).or_default().insert(member);
            }
        } else if item.action == ItemAction::TakeHost.as_str() {
            taken.insert(
                (cid, format!("{}/{}", item.kind, item.name)),
                p.host.unwrap_or_default(),
            );
        }
    }
    if added.is_empty() && taken.is_empty() {
        return Ok(None);
    }
    let layers = s.list_all_host_layers()?;
    let inventory = if taken.is_empty() {
        Vec::new()
    } else {
        s.list_inventory()?
    };
    let sync = |grp: &str, cid: i64, host: String, params: ItemParams| NewChangesetItem {
        grp: grp.to_string(),
        catalog_id: Some(cid),
        kind: "host".into(),
        name: host,
        action: ItemAction::Sync.as_str().into(),
        params: params.to_json(),
        decider: Decider::Rule.as_str().into(),
    };
    let mut items = Vec::new();
    for ((cid, layer), assets) in &added {
        let assigned: BTreeSet<String> = layers
            .iter()
            .filter(|r| r.active && r.catalog_id == *cid && &r.layer_name == layer)
            .map(|r| r.host_alias.clone())
            .collect();
        for host in assigned {
            let params = ItemParams {
                layer: Some(layer.clone()),
                assets: assets.iter().cloned().collect(),
                ..Default::default()
            };
            items.push(sync(layer, *cid, host, params));
        }
    }
    for ((cid, key), source) in &taken {
        let (kind, name) = key.split_once('/').unwrap_or_default();
        let holders: BTreeSet<String> = inventory
            .iter()
            .filter(|r| {
                r.managed
                    && r.kind == kind
                    && r.name == name
                    && &r.host_alias != source
                    && r.catalog_id.is_none_or(|c| c == *cid)
            })
            .map(|r| r.host_alias.clone())
            .collect();
        for host in holders {
            let params = ItemParams {
                assets: vec![key.clone()],
                ..Default::default()
            };
            items.push(sync(UPDATE, *cid, host, params));
        }
    }
    stamp_gap_hashes(&mut items);
    let held: BTreeSet<(String, String)> = s
        .triage_verdicts()?
        .into_iter()
        .filter(|v| v.kind == "layer")
        .map(|v| (v.name, v.content_hash))
        .collect();
    items.retain(|i| {
        let hash = ItemParams::parse(i.params.as_deref())
            .hash
            .unwrap_or_default();
        let subject = format!("{}/{}", i.catalog_id.unwrap_or_default(), i.grp);
        !held.contains(&(subject, hash))
    });
    if items.is_empty() {
        return Ok(None);
    }
    Ok(Some((rollout_summary(&items), items)))
}

/// R15: which sync ops a card may carry to a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpFilter {
    /// Rollout and SB6: create, adopt, update (the applier backs up every
    /// file it replaces) — an update only over a copy the planner verified
    /// untouched ([`action_allowed`], Assets M5).
    Additive,
    /// Drift restore, one asset a person picked: update or overwrite.
    Restore,
}

/// R15: never a remove, a plugin op, a no-op or a blocked action — under
/// any filter.
pub(crate) fn op_allowed(f: OpFilter, op: ActionOp) -> bool {
    match f {
        OpFilter::Additive => matches!(op, ActionOp::Create | ActionOp::Adopt | ActionOp::Update),
        OpFilter::Restore => matches!(op, ActionOp::Update | ActionOp::Overwrite),
    }
}

/// Assets M5 (Rulings R3): whether `a` may go to a host under `f`. On top
/// of [`op_allowed`], an Additive `Update` (Rollout, SB6) needs the planner
/// to have verified the host copy untouched since fleet wrote it — never an
/// edited copy, nor one an entry from before M5 cannot vouch for. So does
/// any Additive action that deletes a moved asset's old location (final
/// review I1: a `Create` at a new location, the old copy removed). A
/// Restore (one asset, one host, a person's pick) may update any copy.
pub(crate) fn action_allowed(f: OpFilter, a: &Action) -> bool {
    let needs_verified = a.op == ActionOp::Update || sync::plan::removes_old_locations(a);
    op_allowed(f, a.op)
        && (f != OpFilter::Additive || !needs_verified || a.host_copy == Some(HostCopy::Unchanged))
}

/// Whether `a` is one of `assets` (`<kind>/<name>`) from one of `catalogs`.
fn card_owns(a: &Action, assets: &BTreeSet<String>, catalogs: &BTreeSet<String>) -> bool {
    assets.contains(&format!("{}/{}", a.kind, a.name))
        && a.catalog.as_ref().is_some_and(|c| catalogs.contains(c))
}

/// Keep only what the card may apply on this host: an allowed action
/// ([`action_allowed`]), for one of `assets` (`<kind>/<name>`), from one of
/// `catalogs`.
pub(crate) fn narrow(
    hp: &mut HostPlan,
    f: OpFilter,
    assets: &BTreeSet<String>,
    catalogs: &BTreeSet<String>,
) {
    hp.actions
        .retain(|a| action_allowed(f, a) && card_owns(a, assets, catalogs));
}

/// Assets M5 fix round 1, M6 R1: the card's own actions `narrow` drops under
/// `f` only because fleet cannot treat the host copy as untouched — an
/// `Update` over a copy whose entry predates the file hashes, a moved
/// asset's `Create` that would delete such an old copy (final review I1),
/// and EVERY `Overwrite` of the card's asset (an edited copy, or a pre-M5
/// one that differs while the catalog did not change — carry T1). Such a
/// host is left to a person, never counted done.
fn held_by_host_copy(
    hp: &HostPlan,
    f: OpFilter,
    assets: &BTreeSet<String>,
    catalogs: &BTreeSet<String>,
) -> Vec<HeldLine> {
    hp.actions
        .iter()
        .filter(|a| card_owns(a, assets, catalogs) && !action_allowed(f, a))
        .filter(|a| op_allowed(f, a.op) || a.op == ActionOp::Overwrite)
        .map(|a| HeldLine {
            kind: a.kind.clone(),
            name: a.name.clone(),
            why: match (a.host_copy, a.op) {
                (Some(HostCopy::Edited), _) => HeldWhy::Edited,
                (Some(HostCopy::Unverified), _) => HeldWhy::Unverified,
                // No verdict on an overwrite: it differs, nothing more is
                // known. An update or a moved create held without a
                // verdict keeps M5's words: the entry cannot vouch.
                (_, ActionOp::Overwrite) => HeldWhy::Differs,
                _ => HeldWhy::Unverified,
            },
        })
        .collect()
}

/// The `${NAME}`s the card's own assets wait on in `hp` — `apply_sync`'s
/// secret gate, read before [`narrow`] drops the blocked actions. Names
/// only, never a value.
fn missing_secrets(
    hp: &HostPlan,
    assets: &BTreeSet<String>,
    catalogs: &BTreeSet<String>,
) -> BTreeSet<String> {
    hp.actions
        .iter()
        .filter(|a| a.op == ActionOp::Blocked && card_owns(a, assets, catalogs))
        .flat_map(|a| a.missing_secrets.iter().cloned())
        .collect()
}

/// How one host's card sync went, over its harnesses: `failed` — a pair
/// that failed or partly applied (PF5: only these fail a card); `skipped`
/// — a pair not planned (unreachable, scan failed), which is reported, not
/// a failure; `applied` — a pair applied something; `nothing` — a pair was
/// planned and nothing the card may do was left on it (never applied);
/// `held` — the card held back an update the planner could not verify
/// (Assets M5), which keeps the host's items from counting as done.
/// Who a host sync runs for: a card a person applied, or SB6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunBy {
    Card,
    Sb6,
}

#[derive(Debug, Default)]
struct HostOutcome {
    applied: bool,
    nothing: bool,
    /// Assets M5 fix round 1: the card held back one of its own updates (or
    /// an overwrite of an edited copy) because the planner could not verify
    /// the host copy untouched ([`held_by_host_copy`], named in `skipped`).
    /// The host's items are then not done, like a skipped host's.
    held: bool,
    /// Assets M6 (R1): the same holds as data; their lines are also in
    /// `skipped`, as since M5.
    held_lines: Vec<HeldLine>,
    failed: Vec<String>,
    skipped: Vec<String>,
}

impl HostOutcome {
    /// The host's items count as applied: no pair failed, nothing was held
    /// back, and one applied or had nothing left to do.
    fn ok(&self) -> bool {
        self.failed.is_empty() && !self.held && (self.applied || self.nothing)
    }

    fn failed(why: String) -> HostOutcome {
        HostOutcome {
            failed: vec![why],
            ..Default::default()
        }
    }
}

/// Plan each host in `wants` (PF4: `allow_unlayered`, since the plan is
/// narrowed to the card's assets anyway), narrow it to what `filter` lets
/// the card write, park what is left as one plan and apply it under its own
/// token (R27). An action of the card's that `narrow` drops only for its
/// host copy ([`held_by_host_copy`]) is reported `skipped` and marks the
/// host `held` (Assets M5 fix round 1). A pair that was not planned is
/// reported `skipped`, and one
/// narrowed to nothing is `nothing` — neither is parked or applied, so a
/// sync with nothing to write never rescans a host, writes a `sync_runs`
/// row or emits progress (fix round 1, I1). For a card ([`RunBy::Card`]),
/// an asset of the card blocked on a missing secret refuses the whole sync
/// first (`E_SECRET_MISSING`, as `apply_sync` without `force_partial`); for
/// SB6 that asset just waits, and the run it records is marked `auto`
/// (final review I3). With `harness`, only that harness's pairs are kept (a
/// restore is pinned to the harness its drift was seen on, I2). Answers
/// each host's [`HostOutcome`].
async fn sync_hosts(
    wants: &BTreeMap<String, BTreeSet<String>>,
    catalogs: &BTreeSet<String>,
    filter: OpFilter,
    by: RunBy,
    harness: Option<&str>,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<BTreeMap<String, HostOutcome>, IpcError> {
    let mut out: BTreeMap<String, HostOutcome> = BTreeMap::new();
    let mut plans: Vec<HostPlan> = Vec::new();
    let mut missing: BTreeSet<String> = BTreeSet::new();
    for (host, assets) in wants {
        let args = PlanArgs {
            host_alias: Some(host.clone()),
            allow_unlayered: true,
            ..Default::default()
        };
        let planned = match sync::plan_sync(args, store, ssh).await {
            Ok(p) => p,
            Err(e) => {
                out.insert(
                    host.clone(),
                    HostOutcome::failed(format!("{host}: {}", e.message)),
                );
                continue;
            }
        };
        let Some((_, parked)) = sync::plan::registry_take_with_expiry(&planned.id) else {
            out.insert(
                host.clone(),
                HostOutcome::failed(format!("{host}: its plan expired before it was applied")),
            );
            continue;
        };
        if parked.hosts.is_empty() {
            out.insert(
                host.clone(),
                HostOutcome {
                    skipped: vec![format!("{host}: nothing planned (hidden?)")],
                    ..Default::default()
                },
            );
            continue;
        }
        let o = out.entry(host.clone()).or_default();
        for mut hp in parked.hosts {
            if harness.is_some_and(|h| h != hp.harness) {
                continue;
            }
            missing.extend(missing_secrets(&hp, assets, catalogs));
            let held = if hp.status == "planned" {
                held_by_host_copy(&hp, filter, assets, catalogs)
            } else {
                Vec::new()
            };
            narrow(&mut hp, filter, assets, catalogs);
            if !held.is_empty() {
                o.held = true;
                o.skipped.extend(held.iter().map(|h| {
                    format!(
                        "{}/{} on {host}: {} — sync it yourself",
                        h.kind,
                        h.name,
                        h.why.words()
                    )
                }));
                o.held_lines.extend(held);
            }
            if hp.status != "planned" {
                let why = hp.detail.clone().unwrap_or_else(|| hp.status.clone());
                o.skipped.push(format!("{host} ({}): {why}", hp.harness));
            } else if hp.actions.is_empty() {
                o.nothing = true;
            } else {
                plans.push(hp);
            }
        }
    }
    if by == RunBy::Card && !missing.is_empty() {
        return Err(IpcError::new(
            codes::E_SECRET_MISSING,
            format!(
                "no value for {}; set them (catalog_set_secret), then apply this card again",
                missing.into_iter().collect::<Vec<_>>().join(", ")
            ),
        ));
    }
    if plans.is_empty() {
        return Ok(out);
    }
    let plan_id = sync::plan::registry_put(SyncPlan::new(plans));
    let args = SyncApplyArgs {
        plan_id,
        // The narrowed plan holds no blocked action; the gate ran above.
        force_partial: true,
        call_id: None,
    };
    let run = match by {
        RunBy::Card => sync::apply_sync_with(args, store, ssh, CancellationToken::new()).await?,
        RunBy::Sb6 => sync::apply_sync_auto(args, store, ssh, CancellationToken::new()).await?,
    };
    for r in &run.hosts {
        let o = out.entry(r.host_alias.clone()).or_default();
        let why = || {
            let bad: Vec<String> = r
                .actions
                .iter()
                .filter(|a| matches!(a.outcome.as_str(), "failed" | "conflict"))
                .map(|a| match &a.detail {
                    Some(d) => format!("{}/{} {}: {d}", a.kind, a.name, a.outcome),
                    None => format!("{}/{} {}", a.kind, a.name, a.outcome),
                })
                .collect();
            let detail = r
                .detail
                .clone()
                .or_else(|| (!bad.is_empty()).then(|| bad.join(", ")));
            format!(
                "{} ({}): {}",
                r.host_alias,
                r.harness,
                detail.unwrap_or_else(|| r.status.clone())
            )
        };
        match r.status.as_str() {
            "applied" => o.applied = true,
            "skipped" => o.skipped.push(why()),
            // `failed`, and `partial` — some of the card's writes on this
            // pair did not land, so its items are not applied.
            _ => o.failed.push(why()),
        }
    }
    Ok(out)
}

/// The labels of the catalogs `items` name (`personal` for personal), each
/// loaded — or `E_INVALID_STATE` and nothing happens (the card stays as it
/// is): a catalog that did not load plans none of its assets, so its card
/// would sync nothing and still count its layer rolled out.
fn card_catalogs(
    card: &ChangesetRow,
    items: &[&ChangesetItemRow],
    store: &Mutex<Store>,
) -> Result<BTreeSet<String>, IpcError> {
    let rows: Vec<CatalogRow> = {
        let s = lock(store)?;
        let mut rows = Vec::new();
        for id in items
            .iter()
            .filter_map(|i| i.catalog_id)
            .collect::<BTreeSet<_>>()
        {
            rows.push(s.get_catalog(id)?.ok_or_else(|| {
                IpcError::new(
                    codes::E_NOTFOUND,
                    format!("catalog {id} no longer exists; dismiss this card"),
                )
            })?);
        }
        rows
    };
    // Store guard dropped: registry after store, never inside it.
    let mut out = BTreeSet::new();
    for row in &rows {
        registry::with_catalog_row(row, |_| Ok(())).map_err(|e| {
            IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "{}; card {} syncs catalog {} and applies once it loads",
                    e.message, card.id, row.name
                ),
            )
        })?;
        out.insert(effective::label_of(row.org_id, &row.name));
    }
    Ok(out)
}

/// A host sync that could not run at all: the card `failed` with why, its
/// items as they were. Answers the error `apply` returns.
fn fail_host_card(card: &ChangesetRow, e: IpcError, store: &Mutex<Store>) -> IpcError {
    match lock(store) {
        Ok(s) => {
            if let Err(w) = s.set_changeset_state(card.id, "failed", Some(&e.message)) {
                tracing::error!("card {}: could not be marked failed: {w}", card.id);
            }
        }
        Err(w) => tracing::error!(
            "card {}: could not be marked failed: {}",
            card.id,
            w.message
        ),
    }
    e
}

/// Spec, Rollout apply: `plan_sync` (the card's hosts) + `apply_sync`,
/// additive only (R15).
async fn apply_rollout(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    selected: &[&ChangesetItemRow],
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    let catalogs = card_catalogs(card, selected, store)?;
    let mut wants: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for &item in selected {
        wants
            .entry(item.name.clone())
            .or_default()
            .extend(ItemParams::parse(item.params.as_deref()).assets);
    }
    let outcome = sync_hosts(
        &wants,
        &catalogs,
        OpFilter::Additive,
        RunBy::Card,
        None,
        store,
        ssh,
    )
    .await
    .map_err(|e| fail_host_card(card, e, store))?;
    finish_host_card(card, items, selected, &outcome, |i| i.name.clone(), store)
}

/// Drift restore: the catalog copy back onto one host, with backup (R15) —
/// on the harness the drift was seen on only (`params.harness`, else
/// `claude`, where the drift rule reads its rows; I2): another harness's
/// copy of the same asset was never picked. A restore that wrote nothing —
/// its host skipped, or no update or overwrite planned — fails and leaves
/// the drift card open.
async fn apply_restore(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    item: &ChangesetItemRow,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    let params = ItemParams::parse(item.params.as_deref());
    let host = params
        .host
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "restore names no host"))?;
    let harness = params.harness.unwrap_or_else(|| "claude".to_string());
    let catalogs = card_catalogs(card, &[item], store)?;
    let wants = BTreeMap::from([(
        host.clone(),
        BTreeSet::from([format!("{}/{}", item.kind, item.name)]),
    )]);
    let mut outcome = sync_hosts(
        &wants,
        &catalogs,
        OpFilter::Restore,
        RunBy::Card,
        Some(&harness),
        store,
        ssh,
    )
    .await
    .map_err(|e| fail_host_card(card, e, store))?;
    // A restore that wrote nothing did not restore: the drift card stays
    // open rather than closing on a no-op or a skipped host.
    let o = outcome.entry(host.clone()).or_default();
    if o.failed.is_empty() && !o.applied {
        let why = if o.skipped.is_empty() {
            format!(
                "nothing to restore on {host}: {}/{} has no update or overwrite planned there",
                item.kind, item.name
            )
        } else {
            format!("not restored: {}", o.skipped.join("; "))
        };
        o.skipped.clear();
        o.failed.push(why);
    }
    finish_host_card(card, items, &[item], &outcome, move |_| host.clone(), store)
}

/// R15 with PF5: the selected items of hosts that applied are `applied`.
/// No host failed: the card is `applied` — in one transaction with its
/// items (every other pending item `skipped`) and its note, any skipped
/// host named in `error`. Else the card is `failed` naming the hosts that
/// failed — answered as the card's view, not as an error, since some hosts
/// may have changed — and the failed and skipped hosts' items stay
/// pending, so applying the card again finishes them.
fn finish_host_card(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    selected: &[&ChangesetItemRow],
    outcome: &BTreeMap<String, HostOutcome>,
    host_of: impl Fn(&ChangesetItemRow) -> String,
    store: &Mutex<Store>,
) -> Result<(), IpcError> {
    let done: Vec<i64> = selected
        .iter()
        .filter(|i| outcome.get(&host_of(i)).is_some_and(HostOutcome::ok))
        .map(|i| i.position)
        .collect();
    let failures: Vec<String> = outcome.values().flat_map(|o| o.failed.clone()).collect();
    let skipped: Vec<String> = outcome.values().flat_map(|o| o.skipped.clone()).collect();
    let note = (!skipped.is_empty()).then(|| format!("skipped: {}", skipped.join("; ")));
    // R1: per selected item, what its host left undone — held lines, and a
    // failed or skipped host's line. `None` for a host that applied cleanly,
    // which CLEARS the item's outcome: a failed card stays open and its
    // items are retried, and the last attempt's outcome must not outlive it.
    let outcomes: Vec<(i64, Option<String>)> = selected
        .iter()
        .map(|i| {
            let json = outcome.get(&host_of(i)).and_then(|o| {
                let note = (!o.failed.is_empty())
                    .then(|| o.failed.join("; "))
                    .or_else(|| {
                        let plain: Vec<&str> = o
                            .skipped
                            .iter()
                            .filter(|l| !l.ends_with("— sync it yourself"))
                            .map(String::as_str)
                            .collect();
                        (!plain.is_empty()).then(|| plain.join("; "))
                    });
                let out = ItemOutcome {
                    held: o.held_lines.clone(),
                    note,
                };
                (out != ItemOutcome::default())
                    .then(|| serde_json::to_string(&out).ok())
                    .flatten()
            });
            (i.position, json)
        })
        .collect();
    let s = lock(store)?;
    s.set_changeset_item_outcomes(card.id, &outcomes)?;
    if failures.is_empty() {
        let rest: Vec<i64> = items
            .iter()
            .filter(|i| i.state == "pending" && !done.contains(&i.position))
            .map(|i| i.position)
            .collect();
        s.record_changeset_applied(
            card.id,
            &AppliedRecord {
                applied_at: now_unix_ms(),
                commits: "{}",
                layers_snapshot: "[]",
                applied: &done,
                skipped: &rest,
                verdicts: &[],
                error: note.as_deref(),
            },
        )?;
    } else {
        let mut msg = failures.join("; ");
        if let Some(note) = note {
            msg = format!("{msg}; {note}");
        }
        s.set_changeset_item_states(card.id, &done, "applied")?;
        s.set_changeset_state(card.id, "failed", Some(&msg))?;
    }
    Ok(())
}

/// Assets M5 (R6): whether SB6 brings `r` up to date — missing; present,
/// identical and not yet managed (adopt); or managed and only behind its
/// catalog (`drift_side = catalog`). A copy a person edited, or one its
/// manifest entry cannot vouch for (`drift_side` unknown), waits for a
/// person. This only picks what SB6 plans: the planner re-checks the fresh
/// snapshot (R2) and the Additive filter (R3, `action_allowed`) applies an
/// Update only over a copy it reads `Unchanged` itself, so a copy edited
/// after the scan is an Overwrite and dropped — never replaced on the
/// inventory's say-so.
pub(crate) fn sb6_due(r: &AssetInventoryRow) -> bool {
    match r.state.as_str() {
        "missing" => true,
        "in_sync" => !r.managed,
        "drifted" => r.managed && r.drift_side.as_deref() == Some("catalog"),
        _ => false,
    }
}

/// SB6 (R17): with no card, sync additively what a rolled-out layer
/// introduced and a host lacks (`missing`) or has unmanaged but identical
/// (`in_sync`, adopted). A `drifted` copy is due only when it is managed
/// and only behind its catalog ([`sb6_due`], Assets M5); the planner turns
/// an edited one into an overwrite, which SB6 never applies. Answers how
/// many hosts it applied something on — 0, with nothing applied or recorded, when nothing is left
/// to do. A fleet
/// with no applied Rollout (pre-M4) answers 0 without planning anything; a
/// layer's first rollout is always a card (R16). An asset blocked on a
/// missing secret waits. A host a person rejected on a layer's Rollout card
/// is left out for that layer ([`rejected_rollouts`]). A host SB6 failed on
/// is left out until its inventory is scanned again ([`Sb6Backoff`], final
/// review I3).
pub async fn auto_additive(store: &Mutex<Store>, ssh: &Arc<SshClient>) -> Result<usize, IpcError> {
    Ok(sb6_pass(store, ssh)
        .await?
        .values()
        .filter(|o| o.applied && o.failed.is_empty())
        .count())
}

/// One SB6 pass ([`auto_additive`]): each host it synced, with its
/// [`HostOutcome`] — the lines it skipped or held included — and none when
/// nothing was due.
async fn sb6_pass(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<BTreeMap<String, HostOutcome>, IpcError> {
    let (rolled, rejected, rows, hosts, configured, backoff, scans) = {
        let s = lock(store)?;
        let rolled = s.rolled_out_layers()?;
        if rolled.is_empty() {
            return Ok(BTreeMap::new());
        }
        (
            rolled,
            rejected_rollouts(&s)?,
            s.list_inventory()?,
            s.list_hosts()?,
            s.list_catalogs()?,
            Sb6Backoff::read(&s),
            s.inventory_last_scans()?,
        )
    };
    let snapshot = registry::snapshot()?;
    let id_of = super::catalog_ids_by_label(&configured);
    let mut wants: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut catalogs: BTreeSet<String> = BTreeSet::new();
    // Per (host, catalog, layer): the assets SB6 wants there, for its card.
    let mut by_layer: BTreeMap<(String, i64, String), Vec<String>> = BTreeMap::new();
    for h in hosts.iter().filter(|h| {
        !h.hidden
            && (h.reachable || h.alias == "local")
            && !backoff.holds(&h.alias, scans.get(&h.alias).copied())
    }) {
        let Ok(eff) = effective::effective_for_host_in(store, &h.alias, &snapshot) else {
            continue;
        };
        for (key, prov) in &eff.provenance {
            let Some(cid) = id_of.get(&prov.catalog) else {
                continue;
            };
            if !rolled.contains(&(*cid, prov.introduced_by.clone()))
                || rejected.contains(&(*cid, prov.introduced_by.clone(), h.alias.clone()))
            {
                continue;
            }
            let Some((kind, name)) = key.split_once('/') else {
                continue;
            };
            let due = rows
                .iter()
                .find(|r| {
                    r.host_alias == h.alias
                        && r.harness == "claude"
                        && r.kind == kind
                        && r.name == name
                })
                .is_some_and(sb6_due);
            if due {
                wants
                    .entry(h.alias.clone())
                    .or_default()
                    .insert(key.clone());
                catalogs.insert(prov.catalog.clone());
                by_layer
                    .entry((h.alias.clone(), *cid, prov.introduced_by.clone()))
                    .or_default()
                    .push(key.clone());
            }
        }
    }
    if wants.is_empty() {
        return Ok(BTreeMap::new());
    }
    let outcome = sync_hosts(
        &wants,
        &catalogs,
        OpFilter::Additive,
        RunBy::Sb6,
        None,
        store,
        ssh,
    )
    .await?;
    for (host, o) in &outcome {
        for why in o.failed.iter().chain(&o.skipped) {
            tracing::debug!(host = %host, "catalog.auto: {why}");
        }
    }
    // Redesign 8.7: an automatic write shows as a card — an applied
    // Rollout, `catalog.auto`'s note on it, under the Inbox's *Recently
    // applied*. Best effort: a card not written never fails the sync.
    if let Err(e) = record_auto_card(store, &by_layer, &outcome) {
        tracing::warn!("catalog.auto: its card was not written: {}", e.message);
    }
    {
        let s = lock(store)?;
        let scans = s.inventory_last_scans()?;
        let mut next = backoff.clone();
        next.forget_rescanned(&scans);
        for (host, o) in &outcome {
            if o.failed.is_empty() {
                next.hosts.remove(host);
            } else {
                next.hosts.insert(host.clone(), scans.get(host).copied());
            }
        }
        if next != backoff {
            next.write(&s);
        }
    }
    Ok(outcome)
}

/// The note an SB6 card carries; [`super::ChangesetSummary::auto`] reads it.
pub const AUTO_SYNC_NOTE: &str =
    "Synced on its own: catalog.auto is on. A host sync is not undone here; sync the host to change it.";

/// Redesign 8.7: SB6's applied writes as one applied Rollout card, one
/// `sync` item per (host, layer) with the assets it brought, decided by the
/// rule and carrying [`AUTO_SYNC_NOTE`]. Only hosts whose sync applied
/// something; nothing at all (no card) when none did. A Rollout card
/// commits nothing, so it is never an undo target and never stands in the
/// way of a catalog card's undo; its layers are ones already rolled out, so
/// `rolled_out_layers` does not change.
fn record_auto_card(
    store: &Mutex<Store>,
    by_layer: &BTreeMap<(String, i64, String), Vec<String>>,
    outcome: &BTreeMap<String, HostOutcome>,
) -> Result<Option<i64>, IpcError> {
    let items: Vec<NewChangesetItem> = by_layer
        .iter()
        .filter(|((host, _, _), _)| outcome.get(host).is_some_and(|o| o.applied))
        .map(|((host, cid, layer), assets)| NewChangesetItem {
            grp: layer.clone(),
            catalog_id: Some(*cid),
            kind: "host".into(),
            name: host.clone(),
            action: ItemAction::Sync.as_str().into(),
            params: serde_json::to_string(&ItemParams {
                layer: Some(layer.clone()),
                assets: assets.clone(),
                ..Default::default()
            })
            .ok(),
            decider: Decider::Rule.as_str().into(),
        })
        .collect();
    if items.is_empty() {
        return Ok(None);
    }
    let layers: BTreeSet<&str> = items.iter().map(|i| i.grp.as_str()).collect();
    let hosts: BTreeSet<&str> = items.iter().map(|i| i.name.as_str()).collect();
    let summary = format!(
        "Synced {} to {}",
        layers.into_iter().collect::<Vec<_>>().join(", "),
        hosts.into_iter().collect::<Vec<_>>().join(", ")
    );
    let s = lock(store)?;
    let card = s.insert_changeset(CardKind::Rollout.as_str(), &summary, &items)?;
    let positions: Vec<i64> = (0..items.len() as i64).collect();
    s.record_changeset_applied(
        card.id,
        &crate::store::AppliedRecord {
            applied_at: crate::store::now_unix_ms(),
            commits: "{}",
            layers_snapshot: "[]",
            applied: &positions,
            skipped: &[],
            verdicts: &[],
            error: Some(AUTO_SYNC_NOTE),
        },
    )?;
    Ok(Some(card.id))
}

/// Final review I3: the hosts SB6 failed on, each with its newest inventory
/// scan at that moment (`None`: it had none). SB6 leaves such a host out
/// until a scan moves that time — the tick's daily rescan, a catalog
/// change, a person's sync — so one broken host costs one attempt per scan,
/// not one per pass. Kept in the store's internal `settings` rows (never a
/// user-facing setting); a row that does not parse is read as empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Sb6Backoff {
    hosts: BTreeMap<String, Option<i64>>,
}

impl Sb6Backoff {
    const KEY: &'static str = "changesets.sb6_backoff";

    fn read(s: &Store) -> Sb6Backoff {
        s.get_setting(Self::KEY)
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default()
    }

    /// Best effort: losing it only means one more attempt.
    fn write(&self, s: &Store) {
        let saved = if self.hosts.is_empty() {
            s.delete_setting(Self::KEY)
        } else {
            match serde_json::to_string(self) {
                Ok(json) => s.set_setting(Self::KEY, &json),
                Err(e) => {
                    tracing::warn!("catalog.auto: back-off not saved: {e}");
                    return;
                }
            }
        };
        if let Err(e) = saved {
            tracing::warn!("catalog.auto: back-off not saved: {e}");
        }
    }

    /// Whether `host`, whose newest scan is `scanned`, is still held.
    fn holds(&self, host: &str, scanned: Option<i64>) -> bool {
        self.hosts.get(host).is_some_and(|at| *at == scanned)
    }

    /// Drop every host scanned since it failed.
    fn forget_rescanned(&mut self, scans: &BTreeMap<String, i64>) {
        self.hosts
            .retain(|host, at| *at == scans.get(host).copied());
    }
}

/// One rollout `sync` decision, for [`rejected_from`].
struct Decision {
    at: i64,
    card: i64,
    position: i64,
    key: (i64, String, String),
    rejected: bool,
}

/// Fix round 1 (6) / Assets M5 (R8): the `(catalog, layer, host)` whose
/// latest decision on a Rollout card's `sync` item is a person's rejection
/// — only a person rejects an item (`reject_item`); a later applied item
/// for the same triple lifts it. "Latest" is by when it was decided: the
/// item's `decided_at`, else (decided before 097) its card's `applied_at`,
/// else the card's creation (seconds → ms); then card id and position.
/// Pure.
pub(super) fn rejected_from(
    cards: &[(ChangesetRow, Vec<ChangesetItemRow>)],
) -> BTreeSet<(i64, String, String)> {
    let mut decisions = Vec::new();
    for (card, items) in cards
        .iter()
        .filter(|(c, _)| c.kind == CardKind::Rollout.as_str())
    {
        for i in items
            .iter()
            .filter(|i| i.action == ItemAction::Sync.as_str())
        {
            let rejected = match i.state.as_str() {
                "rejected" => true,
                "applied" => false,
                _ => continue,
            };
            let (Some(cid), Some(layer)) =
                (i.catalog_id, ItemParams::parse(i.params.as_deref()).layer)
            else {
                continue;
            };
            decisions.push(Decision {
                at: i
                    .decided_at
                    .or(card.applied_at)
                    .unwrap_or(card.created_at.saturating_mul(1000)),
                card: card.id,
                position: i.position,
                key: (cid, layer, i.name.clone()),
                rejected,
            });
        }
    }
    decisions.sort_by_key(|d| (d.at, d.card, d.position));
    let mut last: BTreeMap<(i64, String, String), bool> = BTreeMap::new();
    for d in decisions {
        last.insert(d.key, d.rejected);
    }
    last.into_iter()
        .filter_map(|(k, rejected)| rejected.then_some(k))
        .collect()
}

/// [`rejected_from`] over the stored Rollout cards.
pub(super) fn rejected_rollouts(s: &Store) -> Result<BTreeSet<(i64, String, String)>, IpcError> {
    let mut cards = Vec::new();
    for c in s
        .list_changesets()?
        .into_iter()
        .filter(|c| c.kind == CardKind::Rollout.as_str())
    {
        let items = s.changeset_items(c.id)?;
        cards.push((c, items));
    }
    Ok(rejected_from(&cards))
}

#[cfg(test)]
mod filter_tests {
    use super::*;
    use crate::service::catalog::sync::plan::Action;

    fn action(name: &str, op: ActionOp, catalog: Option<&str>) -> Action {
        let kind = if op == ActionOp::PluginInstall {
            "plugin_ref"
        } else {
            "skill"
        };
        Action {
            kind: kind.into(),
            name: name.into(),
            op,
            catalog: catalog.map(String::from),
            reason: None,
            files: vec![],
            merges: vec![],
            backup: false,
            secrets: vec![],
            missing_secrets: vec![],
            plan: None,
            expected: Default::default(),
            secret_files: Default::default(),
            remove_entry: None,
            plugin: None,
            host_copy: None,
        }
    }

    /// R15: a Rollout keeps create, adopt and update for its own assets and
    /// catalogs only — never an overwrite, a remove or a plugin op; restore
    /// may overwrite, never remove.
    #[test]
    fn a_card_never_carries_an_overwrite_or_a_remove_to_a_host() {
        let mut hp = HostPlan {
            host_alias: "oci".into(),
            harness: "claude".into(),
            status: "planned".into(),
            detail: None,
            actions: vec![
                action("w", ActionOp::Create, Some("personal")),
                action("w2", ActionOp::Overwrite, Some("personal")),
                action("w3", ActionOp::Remove, None),
                Action {
                    host_copy: Some(HostCopy::Unchanged),
                    ..action("w4", ActionOp::Update, Some("personal"))
                },
                action("w5", ActionOp::Adopt, Some("acme")),
                // Assets M5 (R3, PF3): an update the planner could not
                // verify (no entry, a pre-M5 entry) or found edited never
                // rides an Additive card — `narrow` reads `host_copy`.
                action("w6", ActionOp::Update, Some("personal")),
                Action {
                    host_copy: Some(HostCopy::Edited),
                    ..action("w7", ActionOp::Update, Some("personal"))
                },
                Action {
                    host_copy: Some(HostCopy::Unverified),
                    ..action("w8", ActionOp::Update, Some("personal"))
                },
                action("other", ActionOp::Create, Some("personal")),
                action("p", ActionOp::PluginInstall, Some("personal")),
            ],
            snapshot: Default::default(),
            manifest: Default::default(),
        };
        let assets = BTreeSet::from(
            [
                "skill/w",
                "skill/w2",
                "skill/w3",
                "skill/w4",
                "skill/w5",
                "skill/w6",
                "skill/w7",
                "skill/w8",
                "plugin_ref/p",
            ]
            .map(String::from),
        );
        narrow(
            &mut hp,
            OpFilter::Additive,
            &assets,
            &BTreeSet::from(["personal".to_string()]),
        );
        assert_eq!(
            hp.actions
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            ["w", "w4"]
        );
        assert!(op_allowed(OpFilter::Restore, ActionOp::Overwrite));
        assert!(op_allowed(OpFilter::Restore, ActionOp::Update));
        assert!(!op_allowed(OpFilter::Restore, ActionOp::Remove));
        assert!(!op_allowed(OpFilter::Additive, ActionOp::Overwrite));
        assert!(!op_allowed(OpFilter::Additive, ActionOp::PluginInstall));
    }

    /// Assets M5 (R3): a Rollout or SB6 updates only a copy the planner
    /// verified untouched; a person's restore may update any copy.
    #[test]
    fn additive_updates_only_a_copy_the_planner_verified_untouched() {
        let mut a = action("w", ActionOp::Update, Some("personal"));
        for (copy, want) in [
            (Some(HostCopy::Unchanged), true),
            (Some(HostCopy::Edited), false),
            (Some(HostCopy::Unverified), false),
            (None, false),
        ] {
            a.host_copy = copy;
            assert_eq!(action_allowed(OpFilter::Additive, &a), want, "{copy:?}");
            assert!(action_allowed(OpFilter::Restore, &a), "restore: {copy:?}");
        }
        assert!(action_allowed(
            OpFilter::Additive,
            &action("w", ActionOp::Create, Some("personal"))
        ));
        assert!(!action_allowed(
            OpFilter::Additive,
            &action("w", ActionOp::Overwrite, Some("personal"))
        ));
    }

    /// Final review I1: a Rollout or SB6 deletes a moved asset's old
    /// location only over a copy the planner verified untouched — a
    /// `Create` (nothing at the new location yet) as much as an `Update`.
    /// A held one is named as the line the card reports. A `Create` whose
    /// entry records only locations it writes again deletes nothing.
    #[test]
    fn additive_deletes_an_old_location_only_over_a_verified_copy() {
        use crate::service::catalog::sync::manifest::ManifestEntry;
        let mut moved = action("w", ActionOp::Create, Some("personal"));
        moved.files = vec!["~/.claude/skills/w2/SKILL.md".into()];
        moved.remove_entry = Some(ManifestEntry {
            files: vec!["~/.claude/skills/w/SKILL.md".into()],
            ..Default::default()
        });
        let mut recreated = moved.clone();
        recreated.remove_entry = Some(ManifestEntry {
            files: recreated.files.clone(),
            ..Default::default()
        });
        let assets = BTreeSet::from(["skill/w".to_string()]);
        let catalogs = BTreeSet::from(["personal".to_string()]);
        let planned = |actions| HostPlan {
            host_alias: "oci".into(),
            harness: "claude".into(),
            status: "planned".into(),
            detail: None,
            actions,
            snapshot: Default::default(),
            manifest: Default::default(),
        };
        for (copy, want) in [
            (Some(HostCopy::Unchanged), true),
            (Some(HostCopy::Edited), false),
            (Some(HostCopy::Unverified), false),
            (None, false),
        ] {
            moved.host_copy = copy;
            assert_eq!(action_allowed(OpFilter::Additive, &moved), want, "{copy:?}");
            recreated.host_copy = copy;
            assert!(
                action_allowed(OpFilter::Additive, &recreated),
                "deletes nothing: {copy:?}"
            );
            let hp = planned(vec![moved.clone()]);
            let held = held_by_host_copy(&hp, OpFilter::Additive, &assets, &catalogs);
            assert_eq!(held.is_empty(), want, "{copy:?}: {held:?}");
        }
        moved.host_copy = Some(HostCopy::Unverified);
        let hp = planned(vec![moved]);
        assert_eq!(
            held_by_host_copy(&hp, OpFilter::Additive, &assets, &catalogs),
            [HeldLine {
                kind: "skill".into(),
                name: "w".into(),
                why: HeldWhy::Unverified,
            }]
        );
    }

    /// M6 R1 (Task 1 carry): an `Overwrite` of the card's asset with no
    /// verdict on the host copy is held as `Differs` — it differs, nothing
    /// more is known; an edited one says so.
    #[test]
    fn an_overwrite_without_a_verdict_is_held_as_differs() {
        let assets = BTreeSet::from(["skill/w".to_string()]);
        let catalogs = BTreeSet::from(["personal".to_string()]);
        let mut a = action("w", ActionOp::Overwrite, Some("personal"));
        let held = |a: &Action| {
            let hp = HostPlan {
                host_alias: "oci".into(),
                harness: "claude".into(),
                status: "planned".into(),
                detail: None,
                actions: vec![a.clone()],
                snapshot: Default::default(),
                manifest: Default::default(),
            };
            held_by_host_copy(&hp, OpFilter::Additive, &assets, &catalogs)
        };
        a.host_copy = None;
        assert_eq!(
            held(&a),
            [HeldLine {
                kind: "skill".into(),
                name: "w".into(),
                why: HeldWhy::Differs,
            }]
        );
        a.host_copy = Some(HostCopy::Edited);
        assert_eq!(held(&a)[0].why, HeldWhy::Edited);
    }

    /// R15: no filter lets a remove, a plugin op, a no-op or a blocked
    /// action through — every op, both filters.
    #[test]
    fn nothing_ever_removes() {
        use ActionOp::*;
        for f in [OpFilter::Additive, OpFilter::Restore] {
            for op in [
                Create,
                Update,
                Overwrite,
                Adopt,
                Remove,
                PluginInstall,
                PluginUpdate,
                Noop,
                Blocked,
            ] {
                let want = match f {
                    OpFilter::Additive => matches!(op, Create | Adopt | Update),
                    OpFilter::Restore => matches!(op, Update | Overwrite),
                };
                assert_eq!(op_allowed(f, op), want, "{f:?} {op:?}");
            }
        }
    }

    /// Assets M5 (R6): SB6 brings a missing copy, adopts an identical one,
    /// and now updates a managed copy that is only behind its catalog —
    /// never an edited or unverified one.
    #[test]
    fn sb6_brings_a_copy_that_is_only_behind_and_leaves_an_edited_one() {
        let row = |state: &str, managed: bool, side: Option<&str>| AssetInventoryRow {
            state: state.into(),
            managed,
            drift_side: side.map(String::from),
            ..Default::default()
        };
        assert!(sb6_due(&row("missing", false, None)));
        assert!(sb6_due(&row("in_sync", false, None)));
        assert!(!sb6_due(&row("in_sync", true, None)));
        assert!(sb6_due(&row("drifted", true, Some("catalog"))));
        assert!(!sb6_due(&row("drifted", true, Some("host"))));
        assert!(!sb6_due(&row("drifted", true, None)));
        assert!(!sb6_due(&row("drifted", false, Some("catalog"))));
        assert!(!sb6_due(&row("orphan", true, None)));
    }

    /// Assets M5 (R8): the later DECISION wins per (catalog, layer, host),
    /// not the larger card id; an item decided before 097 falls back to its
    /// card's apply time.
    #[test]
    fn a_later_decision_wins_by_when_it_was_made_not_by_card_id() {
        let card = |id: i64, applied_at: Option<i64>| ChangesetRow {
            id,
            kind: "rollout".into(),
            summary: "Roll out core".into(),
            state: "applied".into(),
            created_at: 1,
            applied_at,
            commits: None,
            layers_snapshot: None,
            error: None,
            withdrawn_at: None,
        };
        let sync = |card: i64, state: &str, decided_at: Option<i64>| ChangesetItemRow {
            changeset_id: card,
            position: 0,
            grp: "core".into(),
            catalog_id: Some(1),
            kind: "host".into(),
            name: "oci".into(),
            action: "sync".into(),
            params: Some(r#"{"layer":"core","assets":["skill/w"]}"#.into()),
            decider: "person".into(),
            state: state.into(),
            decided_at,
            outcome: None,
        };
        let key = (1, "core".to_string(), "oci".to_string());
        // Card 9 rejected oci at t=100; card 5 (an OLDER id) applied it at t=200.
        let applied_later = vec![
            (card(5, Some(200)), vec![sync(5, "applied", Some(200))]),
            (card(9, None), vec![sync(9, "rejected", Some(100))]),
        ];
        assert!(!rejected_from(&applied_later).contains(&key));
        let rejected_later = vec![
            (card(5, Some(200)), vec![sync(5, "applied", Some(200))]),
            (card(9, None), vec![sync(9, "rejected", Some(300))]),
        ];
        assert!(rejected_from(&rejected_later).contains(&key));
        let pre_097 = vec![
            (card(5, Some(200)), vec![sync(5, "applied", None)]),
            (card(9, None), vec![sync(9, "rejected", Some(150))]),
        ];
        assert!(
            !rejected_from(&pre_097).contains(&key),
            "falls back to applied_at"
        );
    }

    /// Assets M5 (R8): the store path — an apply on an OLDER card made after
    /// a newer card's rejection lifts it: decision time, not card id.
    #[test]
    fn rejected_rollouts_reads_decision_times_from_the_store() {
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config("/p", None).unwrap();
        let p = s.personal_catalog().unwrap().unwrap().id;
        let sync = NewChangesetItem {
            grp: "core".into(),
            catalog_id: Some(p),
            kind: "host".into(),
            name: "oci".into(),
            action: "sync".into(),
            params: Some(r#"{"layer":"core","assets":["skill/w"]}"#.into()),
            decider: "rule".into(),
        };
        let older = s
            .insert_changeset("rollout", "Roll out core", std::slice::from_ref(&sync))
            .unwrap();
        let newer = s
            .insert_changeset("rollout", "Roll out core", &[sync])
            .unwrap();
        let key = (p, "core".to_string(), "oci".to_string());
        s.set_changeset_item_states(newer.id, &[0], "rejected")
            .unwrap();
        assert!(rejected_rollouts(&s).unwrap().contains(&key));
        // Decision times are milliseconds: let the clock move on.
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.record_changeset_applied(
            older.id,
            &AppliedRecord {
                applied_at: now_unix_ms(),
                commits: "{}",
                layers_snapshot: "[]",
                applied: &[0],
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !rejected_rollouts(&s).unwrap().contains(&key),
            "the older card's later apply lifts the rejection"
        );
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::service::catalog::changesets::rules::NEEDS_A_LOOK;
    use crate::service::catalog::changesets::testkit::*;
    use crate::service::catalog::lock_registry_for_test;

    /// The `changesets` tool (Task 9) awaits `apply` inside an MCP handler,
    /// which must be `Send`: no store guard lives across an await.
    #[test]
    fn the_apply_future_is_send() {
        fn is_send<T: Send>(_: &T) {}
        let check = |store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>| {
            let fut = apply(ApplyArgs::default(), store, ssh);
            is_send(&fut);
        };
        let _ = check;
    }

    fn import(layer: &str, cid: i64, name: &str, from: &str) -> NewChangesetItem {
        item(
            layer,
            Some(cid),
            "skill",
            name,
            ItemAction::Import,
            ItemParams {
                from_host: Some(from.into()),
                layer: Some(layer.into()),
                member: Some(format!("skill/{name}")),
                hash: Some(format!("h-{name}")),
                ..Default::default()
            },
        )
    }

    fn assign(layer: &str, cid: i64, host: &str) -> NewChangesetItem {
        item(
            layer,
            Some(cid),
            "layer",
            layer,
            ItemAction::AssignLayer,
            ItemParams {
                host: Some(host.into()),
                layer: Some(layer.into()),
                axis: Some("context".into()),
                ..Default::default()
            },
        )
    }

    fn look(cid: i64, name: &str, from: &str) -> NewChangesetItem {
        item(
            NEEDS_A_LOOK,
            Some(cid),
            "skill",
            name,
            ItemAction::Import,
            ItemParams {
                from_host: Some(from.into()),
                member: Some(format!("skill/{name}")),
                reason: Some("no layer shares its hosts or name prefix".into()),
                ..Default::default()
            },
        )
    }

    fn contexts(f: &Fleet, host: &str, cid: i64) -> Vec<String> {
        f.store
            .lock()
            .unwrap()
            .get_host_layers_for(host, cid)
            .unwrap()
            .into_iter()
            .map(|r| r.layer_name)
            .collect()
    }

    fn rollouts(f: &Fleet) -> Vec<(ChangesetRow, Vec<ChangesetItemRow>)> {
        let s = f.store.lock().unwrap();
        s.list_changesets()
            .unwrap()
            .into_iter()
            .filter(|c| c.kind == "rollout")
            .map(|c| {
                let items = s.changeset_items(c.id).unwrap();
                (c, items)
            })
            .collect()
    }

    /// Spec, Apply 1–5: imports, the layer file, the scope and the host's
    /// layers land in one commit; "needs a look" is skipped unless named;
    /// the catalog reloads; a follow-up Rollout card is proposed (R14).
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn applying_a_bootstrap_commits_once_and_assigns_layers() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let p = f.personal.id;
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "bootstrap",
                "Adopt 1 as 1 layers",
                &[
                    import("core", p, "w", "oci"),
                    item(
                        "core",
                        Some(p),
                        "skill",
                        "w",
                        ItemAction::SetScope,
                        ItemParams {
                            scope: Some("shared".into()),
                            member: Some("skill/w".into()),
                            ..Default::default()
                        },
                    ),
                    assign("core", p, "oci"),
                    item(
                        NEEDS_A_LOOK,
                        Some(p),
                        "skill",
                        "odd",
                        ItemAction::Import,
                        ItemParams {
                            from_host: Some("oci".into()),
                            reason: Some("carries a secret".into()),
                            ..Default::default()
                        },
                    ),
                ],
            )
            .unwrap();
        let before = head(&f.personal_root);

        let v = apply(
            ApplyArgs {
                id: card.id,
                positions: None,
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        assert_eq!(v.error, None, "auto_push is off: no push, no warning");
        assert!(
            v.applied_at.unwrap() > 1_000_000_000_000,
            "applied_at is milliseconds (PF13)"
        );
        assert_eq!(subjects(&f.personal_root)[0], "fleet: Adopt 1 as 1 layers");
        assert_eq!(
            git(
                &f.personal_root,
                &["rev-list", "--count", &format!("{before}..HEAD")]
            ),
            "1",
            "one commit per catalog"
        );
        assert_eq!(v.commits.get("personal"), Some(&head(&f.personal_root)));
        let w = repo::read_asset(&f.personal_root, Kind::Skill, "w").unwrap();
        assert_eq!(w.header.scope, Scope::Shared);
        let layer = Layer::from_yaml(
            &std::fs::read_to_string(f.personal_root.join("layers/core.yaml")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            (layer.axis, layer.members.clone()),
            (Axis::Context, vec!["skill/w".to_string()])
        );
        assert_eq!(contexts(&f, "oci", p), ["core"]);
        let states: Vec<&str> = v.items.iter().map(|i| i.state.as_str()).collect();
        assert_eq!(states, ["applied", "applied", "applied", "skipped"]);
        assert!(
            registry::with_catalog_row(&f.personal, |c| Ok(c.find(Kind::Skill, "w").is_some()))
                .unwrap(),
            "reloaded"
        );
        let cards = f.store.lock().unwrap().list_changesets().unwrap();
        let rollout = cards
            .iter()
            .find(|c| c.kind == "rollout")
            .expect("a follow-up rollout");
        assert_eq!(rollout.state, "proposed");
        assert!(
            rollout.summary.starts_with("Roll out core to oci"),
            "{}",
            rollout.summary
        );
    }

    /// Final review I2: a file in the host's skill folder that the
    /// checkout's own `.gitignore` matches (a Finder `.DS_Store`) is never
    /// written or named to `git add` — the card applies, the file is not
    /// committed, and the card's note names it.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn an_ignored_host_file_is_skipped_with_a_warning() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        f.commit_files(&f.personal_root, p, &[(".gitignore", ".DS_Store\n")]);
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        std::fs::write(home.path().join(".claude/skills/w/.DS_Store"), "finder").unwrap();
        std::fs::write(home.path().join(".claude/skills/w/notes.txt"), "keep").unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = new_card(
            &f,
            &[import("core", p, "w", "oci"), assign("core", p, "oci")],
        );
        let v = apply_all(&f, id, &ssh).await.unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        let files = git(&f.personal_root, &["ls-files", "skills/w"]);
        assert!(files.contains("skills/w/resources/notes.txt"), "{files}");
        assert!(!files.contains(".DS_Store"), "{files}");
        assert!(
            !f.personal_root
                .join("skills/w/resources/.DS_Store")
                .exists(),
            "never written either"
        );
        assert!(repo::is_clean(&f.personal_root).unwrap());
        let note = v.error.unwrap_or_default();
        assert!(
            note.contains("skills/w/resources/.DS_Store") && note.contains("gitignore"),
            "{note}"
        );
    }

    /// Spec, Testing: "apply makes one commit per catalog".
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_card_touching_two_catalogs_commits_once_in_each() {
        let _g = lock_registry_for_test();
        let mut f = Fleet::new(&["oci"]);
        let (acme, acme_root) = f.add_org_catalog("acme");
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        host_skill(home.path(), "v", DESC);
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "bootstrap",
                "Adopt 2 as 2 layers",
                &[
                    import("core", f.personal.id, "w", "oci"),
                    import("ops", acme.id, "v", "oci"),
                ],
            )
            .unwrap();
        let (p0, a0) = (head(&f.personal_root), head(&acme_root));
        let v = apply(
            ApplyArgs {
                id: card.id,
                positions: None,
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        for (root, before) in [(&f.personal_root, p0), (&acme_root, a0)] {
            assert_eq!(
                git(root, &["rev-list", "--count", &format!("{before}..HEAD")]),
                "1"
            );
        }
        assert_eq!(
            v.commits.keys().map(String::as_str).collect::<Vec<_>>(),
            ["acme", "personal"]
        );
        assert!(acme_root.join("skills/v/asset.yaml").is_file());
        assert!(!f.personal_root.join("skills/v").exists());
    }

    /// Spec, Testing: "a failed apply commits nothing" — an import that
    /// brings nothing back fails its group; the tree is reset, and every
    /// item stays pending (R12, Task 5 carry: the pass's refresh of a
    /// failed card never meets an applied item).
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_failed_import_commits_nothing_and_leaves_the_card_failed() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let p = f.personal.id;
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "bootstrap",
                "Adopt 2 as 2 layers",
                &[
                    import("core", p, "w", "oci"),
                    import("extra", p, "absent", "oci"),
                ],
            )
            .unwrap();
        let before = head(&f.personal_root);
        let err = apply(
            ApplyArgs {
                id: card.id,
                positions: None,
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap_err();
        assert!(err.message.starts_with("extra: "), "{}", err.message);
        assert_eq!(head(&f.personal_root), before, "nothing committed");
        assert!(
            git(&f.personal_root, &["status", "--porcelain"]).is_empty(),
            "nothing left behind"
        );
        let v = super::super::get(card.id, &f.store).unwrap();
        assert_eq!(v.state, "failed");
        assert_eq!(v.error.as_deref(), Some(err.message.as_str()));
        assert_eq!(v.applied_at, None);
        assert!(
            v.items.iter().all(|i| i.state == "pending"),
            "a failed card can be applied again"
        );
    }

    /// R12: a commit that fails in the second catalog resets the first one's
    /// commit too and restores `host_layers`; every item stays pending.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_failed_commit_resets_every_catalog_and_restores_host_layers() {
        use crate::tmux::fake_exec::{write_exec, PROBE_GUARD};
        let _g = lock_registry_for_test();
        let mut f = Fleet::new(&["oci"]);
        let (acme, acme_root) = f.add_org_catalog("acme");
        write_exec(
            &acme_root.join(".git/hooks"),
            "pre-commit",
            &format!("#!/bin/sh\n{PROBE_GUARD}exit 1\n"),
        );
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        host_skill(home.path(), "v", DESC);
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let p = f.personal.id;
        f.store
            .lock()
            .unwrap()
            .set_host_layers_for("oci", p, None, &["base"])
            .unwrap();
        f.commit_files(
            &f.personal_root,
            p,
            &[(
                "layers/base.yaml",
                "kind: layer\nname: base\naxis: context\n",
            )],
        );
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "bootstrap",
                "Adopt 2 as 2 layers",
                &[
                    import("core", p, "w", "oci"),
                    assign("core", p, "oci"),
                    import("ops", acme.id, "v", "oci"),
                ],
            )
            .unwrap();
        let (p0, a0) = (head(&f.personal_root), head(&acme_root));
        let err = apply(
            ApplyArgs {
                id: card.id,
                positions: None,
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap_err();
        assert!(
            err.message.starts_with("commit: catalog acme"),
            "{}",
            err.message
        );
        assert_eq!(
            (head(&f.personal_root), head(&acme_root)),
            (p0, a0),
            "nothing committed anywhere"
        );
        assert!(!f.personal_root.join("skills/w").exists());
        assert!(git(&acme_root, &["status", "--porcelain"]).is_empty());
        assert_eq!(contexts(&f, "oci", p), ["base"], "host_layers restored");
        let v = super::super::get(card.id, &f.store).unwrap();
        assert_eq!(v.state, "failed");
        assert!(v.items.iter().all(|i| i.state == "pending"));
        assert!(rollouts(&f).is_empty(), "no follow-up for a failed card");
    }

    /// R11 / PF7: a dirty checkout refuses before anything is written; the
    /// card stays proposed and the person's file is untouched.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_dirty_catalog_refuses_before_anything_changes() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        std::fs::write(f.personal_root.join("pending.txt"), "x\n").unwrap();
        let bin = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset("new", "New", &[import("core", f.personal.id, "w", "oci")])
            .unwrap();
        let err = apply(
            ApplyArgs {
                id: card.id,
                positions: None,
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(err.message.contains("uncommitted"), "{}", err.message);
        assert!(err.message.contains("personal"), "{}", err.message);
        assert_eq!(
            super::super::get(card.id, &f.store).unwrap().state,
            "proposed"
        );
        assert!(f.personal_root.join("pending.txt").is_file());
    }

    /// Spec, Drift: one item at a time; take_host imports the host's copy
    /// and keeps the catalog copy's scope (R13, R15).
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_drift_card_takes_the_host_copy_one_item_at_a_time() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        f.commit_files(
            &f.personal_root,
            p,
            &[
                (
                    "skills/w/asset.yaml",
                    "kind: skill\nname: w\ndescription: The catalog's own long description.\nscope: shared\n",
                ),
                ("skills/w/body.md", "Old steps.\n"),
            ],
        );
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", "Edited on the host, long enough.");
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let drift = |action| {
            item(
                "drift",
                Some(p),
                "skill",
                "w",
                action,
                ItemParams {
                    host: Some("oci".into()),
                    hash: Some("e".into()),
                    ..Default::default()
                },
            )
        };
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "drift",
                "skill/w differs on oci from catalog personal",
                &[drift(ItemAction::TakeHost), drift(ItemAction::Restore)],
            )
            .unwrap();
        let err = apply(
            ApplyArgs {
                id: card.id,
                positions: None,
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(
            err.message.contains("one item at a time"),
            "{}",
            err.message
        );

        let v = apply(
            ApplyArgs {
                id: card.id,
                positions: Some(vec![0]),
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        let w = repo::read_asset(&f.personal_root, Kind::Skill, "w").unwrap();
        assert_eq!(w.header.description, "Edited on the host, long enough.");
        assert_eq!(
            w.header.scope,
            Scope::Shared,
            "the catalog copy's scope is kept"
        );
        assert_eq!(
            v.items.iter().map(|i| i.state.as_str()).collect::<Vec<_>>(),
            ["applied", "skipped"]
        );
    }

    /// Final review I5: take_host takes the content the host copy supplies
    /// (description, body, Claude's own fields) and keeps the catalog
    /// header's scope, targets for other harnesses, tags, version and
    /// install_as — `targets.codex.enabled: false` survives.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn take_host_keeps_the_catalog_headers_metadata() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        f.commit_files(
            &f.personal_root,
            p,
            &[
                (
                    "skills/w/asset.yaml",
                    "kind: skill\nname: w\nversion: '3'\ndescription: The catalog's own long \
                     description.\ntags:\n- ops\ninstall_as: w-host\nscope: shared\ntargets:\n  \
                     codex:\n    enabled: false\n    model: gpt-x\n",
                ),
                ("skills/w/body.md", "Old steps.\n"),
            ],
        );
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", "Edited on the host, long enough.");
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = drift_card(&f, p, "oci");
        let v = apply(
            ApplyArgs {
                id,
                positions: Some(vec![0]),
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        let w = repo::read_asset(&f.personal_root, Kind::Skill, "w").unwrap();
        assert_eq!(w.header.description, "Edited on the host, long enough.");
        assert!(w.body.contains("Steps for w."), "{}", w.body);
        assert_eq!(w.header.scope, Scope::Shared);
        assert_eq!(w.header.version, "3");
        assert_eq!(w.header.tags, ["ops"]);
        assert_eq!(w.header.install_as.as_deref(), Some("w-host"));
        let codex = w.header.targets.get("codex").expect("codex target kept");
        assert!(!codex.enabled);
        assert_eq!(codex.model.as_deref(), Some("gpt-x"));
    }

    /// Task 4 review carry (M2): a "needs a look" import named in
    /// `positions` lands in personal with no layer; because an org-bound
    /// host holds it, it is written `scope: shared` so that host keeps it.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_named_look_item_held_by_an_org_host_is_adopted_shared() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci", "trn"]);
        let p = f.personal.id;
        {
            let s = f.store.lock().unwrap();
            let org = s.add_org("papaya", None, false).unwrap();
            s.set_host_org("trn", Some(org.id)).unwrap();
            let held = |host: &str| AssetInventoryRow {
                host_alias: host.into(),
                harness: "claude".into(),
                kind: "skill".into(),
                name: "odd".into(),
                state: "unmanaged".into(),
                scanned_at: 1,
                ..Default::default()
            };
            s.replace_host_inventory("trn", "claude", &[held("trn")])
                .unwrap();
            s.replace_host_inventory("oci", "claude", &[held("oci")])
                .unwrap();
        }
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "odd", DESC);
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "bootstrap",
                "Adopt 0 as 0 layers; 1 need a look",
                &[look(p, "odd", "oci")],
            )
            .unwrap();
        let err = apply(
            ApplyArgs {
                id: card.id,
                positions: None,
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap_err();
        assert!(err.message.contains("nothing to apply"), "{}", err.message);
        let v = apply(
            ApplyArgs {
                id: card.id,
                positions: Some(vec![0]),
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        let odd = repo::read_asset(&f.personal_root, Kind::Skill, "odd").unwrap();
        assert_eq!(odd.header.scope, Scope::Shared);
        assert!(!f.personal_root.join("layers").exists(), "no layer");
        assert!(rollouts(&f).is_empty(), "no layer, no rollout");
    }

    /// Task 4 review carry (M3) / R11: an item whose catalog failed to load
    /// is refused until it loads — the card stays as it was.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn an_item_for_a_catalog_that_failed_to_load_is_refused() {
        let _g = lock_registry_for_test();
        let mut f = Fleet::new(&["oci"]);
        let (acme, acme_root) = f.add_org_catalog("acme");
        registry::install(crate::service::catalog::problem_entry(
            &acme,
            &IpcError::new(E_CATALOG_PARSE, "catalog.yaml: bad"),
        ))
        .unwrap();
        let bin = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset("bootstrap", "Adopt 0", &[look(acme.id, "odd", "oci")])
            .unwrap();
        let before = head(&acme_root);
        let err = apply(
            ApplyArgs {
                id: card.id,
                positions: Some(vec![0]),
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(err.message.contains("acme"), "{}", err.message);
        let v = super::super::get(card.id, &f.store).unwrap();
        assert_eq!((v.state.as_str(), v.error), ("proposed", None));
        assert_eq!(head(&acme_root), before);
    }

    /// PF10 / PF11: the follow-up Rollout carries the layer's gap hash, and
    /// a second card into the same layer refreshes that open Rollout rather
    /// than adding another.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn the_follow_up_rollout_carries_a_gap_hash_and_is_refreshed_not_duplicated() {
        use crate::service::catalog::changesets::rules::{gap_hash, LayerGap};
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        host_skill(home.path(), "v", DESC);
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let hash_of = |assets: &[&str]| {
            let g = LayerGap {
                catalog_id: p,
                layer: "core".into(),
                host: "oci".into(),
                assets: assets.iter().map(|a| a.to_string()).collect(),
            };
            gap_hash(&[&g])
        };
        let first = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "new",
                "New on oci: skill/w → core",
                &[import("core", p, "w", "oci"), assign("core", p, "oci")],
            )
            .unwrap();
        apply(
            ApplyArgs {
                id: first.id,
                positions: None,
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap();
        let r = rollouts(&f);
        assert_eq!(r.len(), 1);
        let p0 = ItemParams::parse(r[0].1[0].params.as_deref());
        assert_eq!(p0.assets, ["skill/w"]);
        assert_eq!(p0.hash, Some(hash_of(&["skill/w"])));

        let second = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "new",
                "New on oci: skill/v → core",
                &[import("core", p, "v", "oci")],
            )
            .unwrap();
        apply(
            ApplyArgs {
                id: second.id,
                positions: None,
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap();
        let r = rollouts(&f);
        assert_eq!(r.len(), 1, "refreshed, not duplicated");
        assert_eq!((r[0].0.state.as_str(), r[0].1.len()), ("proposed", 1));
        let p1 = ItemParams::parse(r[0].1[0].params.as_deref());
        assert_eq!(p1.assets, ["skill/v", "skill/w"]);
        assert_eq!(p1.hash, Some(hash_of(&["skill/v", "skill/w"])));
        assert_eq!(r[0].0.summary, "Roll out core to oci");
    }

    /// R12 / SB4: with `catalog.auto_push` on, a push that fails after the
    /// commits is a warning on the applied card, not a failure.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_failed_auto_push_is_a_warning_on_an_applied_card() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        settings::set(
            &f.store.lock().unwrap(),
            settings::CATALOG_AUTO_PUSH,
            "true",
        )
        .unwrap();
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset("new", "New", &[import("core", f.personal.id, "w", "oci")])
            .unwrap();
        let before = head(&f.personal_root);
        let v = apply(
            ApplyArgs {
                id: card.id,
                positions: None,
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap();
        assert_eq!(v.state, "applied");
        assert_ne!(head(&f.personal_root), before, "the commit stays");
        assert!(
            v.error.as_deref().unwrap_or("").contains("push personal"),
            "{:?}",
            v.error
        );
    }

    /// PF7 guard (a): a file another process drops into the checkout while
    /// the apply runs is neither committed with the card nor removed.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_foreign_file_written_mid_apply_is_neither_committed_nor_removed() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        let bin = tempfile::tempdir().unwrap();
        let foreign = f.personal_root.join("foreign.txt");
        let ssh = ssh_with_home_running(
            bin.path(),
            home.path(),
            &format!("echo theirs > '{}'", foreign.display()),
        );
        let id = new_card(
            &f,
            &[import("core", p, "w", "oci"), assign("core", p, "oci")],
        );
        let v = apply_all(&f, id, &ssh).await.unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        let files = git(
            &f.personal_root,
            &["show", "--name-only", "--format=", "HEAD"],
        );
        assert!(files.contains("skills/w/asset.yaml") && files.contains("layers/core.yaml"));
        assert!(!files.contains("foreign.txt"), "{files}");
        assert!(foreign.is_file(), "still on disk");
    }

    /// PF7 guard (b): the same, but the apply fails — the catalog is left
    /// exactly as it is and the card says a person must clean up.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_failed_apply_with_a_foreign_file_is_left_for_a_person() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        let bin = tempfile::tempdir().unwrap();
        let foreign = f.personal_root.join("foreign.txt");
        let ssh = ssh_with_home_running(
            bin.path(),
            home.path(),
            &format!("echo theirs > '{}'", foreign.display()),
        );
        // The import lands in the checkout; the assignment of a layer the
        // card never writes fails after it.
        let id = new_card(
            &f,
            &[import("core", p, "w", "oci"), assign("ghost", p, "oci")],
        );
        let before = head(&f.personal_root);
        let err = apply_all(&f, id, &ssh).await.unwrap_err();
        assert!(err.message.starts_with("ghost: "), "{}", err.message);
        assert!(
            err.message
                .contains("manual cleanup needed in personal: foreign.txt"),
            "{}",
            err.message
        );
        assert!(foreign.is_file(), "the foreign file survives");
        assert!(
            f.personal_root.join("skills/w").exists(),
            "nothing was reset"
        );
        assert_eq!(head(&f.personal_root), before, "nothing committed");
        let v = super::super::get(id, &f.store).unwrap();
        assert_eq!(v.state, "failed");
        assert_eq!(v.error.as_deref(), Some(err.message.as_str()));
        assert!(v.items.iter().all(|i| i.state == "pending"));
    }

    /// PF7 guard (c): a commit someone else made mid-apply fails the card at
    /// its commit step, and nothing in that catalog is reset.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_foreign_commit_mid_apply_fails_the_card_and_resets_nothing() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home_running(
            bin.path(),
            home.path(),
            &format!(
                "git -C '{}' -c user.name=x -c user.email=x@x commit -q --allow-empty -m foreign",
                f.personal_root.display()
            ),
        );
        let id = new_card(&f, &[import("core", p, "w", "oci")]);
        let err = apply_all(&f, id, &ssh).await.unwrap_err();
        assert!(
            err.message
                .starts_with("commit: catalog personal: HEAD moved"),
            "{}",
            err.message
        );
        assert!(
            err.message
                .contains("manual cleanup needed in personal: HEAD moved"),
            "{}",
            err.message
        );
        assert_eq!(
            subjects(&f.personal_root)[0],
            "foreign",
            "the foreign commit stays"
        );
        assert!(
            f.personal_root.join("skills/w").exists(),
            "nothing was reset"
        );
        assert_eq!(super::super::get(id, &f.store).unwrap().state, "failed");
    }

    /// Task 6 review: a reset that itself fails is on the card and in the
    /// answer, not only in the log.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_reset_that_fails_is_reported_on_the_card() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home_running(
            bin.path(),
            home.path(),
            &format!(
                ": > '{}'",
                f.personal_root.join(".git/index.lock").display()
            ),
        );
        let id = new_card(&f, &[import("core", p, "w", "oci")]);
        let err = apply_all(&f, id, &ssh).await.unwrap_err();
        assert!(
            err.message.starts_with("commit: catalog personal"),
            "{}",
            err.message
        );
        assert!(
            err.message
                .contains("catalog personal could not be reset to")
                && err.message.contains("fix by hand"),
            "{}",
            err.message
        );
        let v = super::super::get(id, &f.store).unwrap();
        assert_eq!(
            (v.state.as_str(), v.error.as_deref()),
            ("failed", Some(err.message.as_str()))
        );
    }

    /// R11: a clean personal and a dirty org catalog — refused before
    /// anything is written anywhere.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn one_dirty_catalog_refuses_the_whole_card_and_the_clean_one_is_untouched() {
        let _g = lock_registry_for_test();
        let mut f = Fleet::new(&["oci"]);
        let (acme, acme_root) = f.add_org_catalog("acme");
        std::fs::write(acme_root.join("draft.txt"), "wip\n").unwrap();
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        host_skill(home.path(), "v", DESC);
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = new_card(
            &f,
            &[
                import("core", f.personal.id, "w", "oci"),
                import("ops", acme.id, "v", "oci"),
            ],
        );
        let before = head(&f.personal_root);
        let err = apply_all(&f, id, &ssh).await.unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(
            err.message.contains("catalog acme has uncommitted"),
            "{}",
            err.message
        );
        assert_eq!(head(&f.personal_root), before);
        assert!(git(&f.personal_root, &["status", "--porcelain"]).is_empty());
        assert!(acme_root.join("draft.txt").is_file());
        assert_eq!(super::super::get(id, &f.store).unwrap().state, "proposed");
    }

    /// R12: a take_host whose import fails puts the catalog copy it removed
    /// back.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_failed_take_host_restores_the_catalog_copy() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        f.commit_files(
            &f.personal_root,
            p,
            &[
                (
                    "skills/w/asset.yaml",
                    "kind: skill\nname: w\ndescription: The catalog's own long description.\nscope: shared\n",
                ),
                ("skills/w/body.md", "Old steps.\n"),
            ],
        );
        crate::tmux::fake_exec::write_exec(
            &f.personal_root.join(".git/hooks"),
            "pre-commit",
            &format!("#!/bin/sh\n{}exit 1\n", crate::tmux::fake_exec::PROBE_GUARD),
        );
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", "Edited on the host, long enough.");
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let take = item(
            "drift",
            Some(p),
            "skill",
            "w",
            ItemAction::TakeHost,
            ItemParams {
                host: Some("oci".into()),
                ..Default::default()
            },
        );
        let id = f
            .store
            .lock()
            .unwrap()
            .insert_changeset("drift", "skill/w differs", &[take])
            .unwrap()
            .id;
        let before = head(&f.personal_root);
        let err = apply(
            ApplyArgs {
                id,
                positions: Some(vec![0]),
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap_err();
        assert!(
            err.message.starts_with("commit: catalog personal"),
            "{}",
            err.message
        );
        assert!(!err.message.contains("manual cleanup"), "{}", err.message);
        let w = repo::read_asset(&f.personal_root, Kind::Skill, "w").unwrap();
        assert_eq!(w.header.description, "The catalog's own long description.");
        assert_eq!(w.body, "Old steps.\n");
        assert_eq!(head(&f.personal_root), before);
        assert!(git(&f.personal_root, &["status", "--porcelain"]).is_empty());
    }

    /// R10: an applied hide records a `rule` verdict, whoever decided the
    /// item; `positions: []` is its own refusal.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_hide_records_a_rule_verdict_and_empty_positions_are_refused() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let bin = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let mut hide = item(
            "hidden",
            None,
            "hook",
            "stop",
            ItemAction::Hide,
            ItemParams {
                hash: Some("h-stop".into()),
                ..Default::default()
            },
        );
        hide.decider = "person".into();
        let id = new_card(&f, &[hide]);
        let err = apply(
            ApplyArgs {
                id,
                positions: Some(vec![]),
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(
            err.message.starts_with("no positions named"),
            "{}",
            err.message
        );
        let v = apply_all(&f, id, &ssh).await.unwrap();
        assert_eq!(v.state, "applied");
        let verdicts = f.store.lock().unwrap().triage_verdicts().unwrap();
        assert_eq!(verdicts.len(), 1);
        assert_eq!(
            (verdicts[0].decider.as_str(), verdicts[0].verdict.as_str()),
            ("rule", "ignored")
        );
    }

    /// Round 2 (a): a person's file inside a folder this apply creates is
    /// not the apply's — on success it is not committed and stays on disk.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_foreign_file_inside_a_created_folder_is_not_committed() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        let bin = tempfile::tempdir().unwrap();
        let notes = f.personal_root.join("skills/w/notes.md");
        let ssh = ssh_with_home_running(
            bin.path(),
            home.path(),
            &format!(
                "mkdir -p '{0}/skills/w' && echo theirs > '{0}/skills/w/notes.md'",
                f.personal_root.display()
            ),
        );
        let id = new_card(&f, &[import("core", p, "w", "oci")]);
        let v = apply_all(&f, id, &ssh).await.unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        let files = git(
            &f.personal_root,
            &["show", "--name-only", "--format=", "HEAD"],
        );
        assert!(files.contains("skills/w/asset.yaml"), "{files}");
        assert!(!files.contains("notes.md"), "{files}");
        assert!(notes.is_file());
    }

    /// Round 2 (a): the same on failure — kept, and the card says manual
    /// cleanup; nothing in that catalog is reset.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_foreign_file_inside_a_created_folder_survives_a_failure() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        let bin = tempfile::tempdir().unwrap();
        let notes = f.personal_root.join("skills/w/notes.md");
        let ssh = ssh_with_home_running(
            bin.path(),
            home.path(),
            &format!(
                "mkdir -p '{0}/skills/w' && echo theirs > '{0}/skills/w/notes.md'",
                f.personal_root.display()
            ),
        );
        let id = new_card(
            &f,
            &[import("core", p, "w", "oci"), assign("ghost", p, "oci")],
        );
        let err = apply_all(&f, id, &ssh).await.unwrap_err();
        assert!(
            err.message
                .contains("manual cleanup needed in personal: skills/w/notes.md"),
            "{}",
            err.message
        );
        assert!(notes.is_file());
        assert!(
            f.personal_root.join("skills/w/asset.yaml").is_file(),
            "not reset"
        );
    }

    /// Round 2 (b): a file git ignores, inside a folder the apply created,
    /// is never deleted by a failed apply's reset (R12).
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn an_ignored_file_inside_a_created_folder_is_kept_on_failure() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        f.commit_files(&f.personal_root, p, &[(".gitignore", "*.local\n")]);
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        let bin = tempfile::tempdir().unwrap();
        let cache = f.personal_root.join("skills/w/cache.local");
        let ssh = ssh_with_home_running(
            bin.path(),
            home.path(),
            &format!(
                "mkdir -p '{0}/skills/w' && echo mine > '{0}/skills/w/cache.local'",
                f.personal_root.display()
            ),
        );
        let id = new_card(
            &f,
            &[import("core", p, "w", "oci"), assign("ghost", p, "oci")],
        );
        let err = apply_all(&f, id, &ssh).await.unwrap_err();
        assert!(cache.is_file(), "an ignored file is never deleted");
        assert!(
            err.message
                .contains("manual cleanup needed in personal: skills/w/cache.local"),
            "{}",
            err.message
        );
    }

    /// Round 2 (a): a person's edit to a tracked file of the asset a
    /// take_host replaces, made mid-apply, is foreign: the apply refuses to
    /// overwrite it and the edit stays.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn an_edit_inside_a_take_host_asset_mid_apply_is_foreign() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        f.commit_files(
            &f.personal_root,
            p,
            &[
                (
                    "skills/w/asset.yaml",
                    "kind: skill\nname: w\ndescription: The catalog's own long description.\n",
                ),
                ("skills/w/body.md", "Old steps.\n"),
            ],
        );
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", "Edited on the host, long enough.");
        let bin = tempfile::tempdir().unwrap();
        let body = f.personal_root.join("skills/w/body.md");
        let ssh = ssh_with_home_running(
            bin.path(),
            home.path(),
            &format!("echo mine > '{}'", body.display()),
        );
        let take = item(
            "drift",
            Some(p),
            "skill",
            "w",
            ItemAction::TakeHost,
            ItemParams {
                host: Some("oci".into()),
                ..Default::default()
            },
        );
        let id = f
            .store
            .lock()
            .unwrap()
            .insert_changeset("drift", "skill/w differs", &[take])
            .unwrap()
            .id;
        let before = head(&f.personal_root);
        let err = apply(
            ApplyArgs {
                id,
                positions: Some(vec![0]),
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap_err();
        assert!(
            err.message
                .contains("changed in catalog personal during the apply")
                && err.message.contains("skills/w/body.md"),
            "{}",
            err.message
        );
        assert_eq!(std::fs::read_to_string(&body).unwrap(), "mine\n");
        let w = repo::read_asset(&f.personal_root, Kind::Skill, "w").unwrap();
        assert_eq!(w.header.description, "The catalog's own long description.");
        assert_eq!(head(&f.personal_root), before);
        assert_eq!(super::super::get(id, &f.store).unwrap().state, "failed");
    }

    const PERSON_EDIT: &str =
        "kind: skill\nname: x\ndescription: Edited by a person meanwhile, long enough.\n";

    /// Round 3 (a, b): `x` is committed; the card imports `w` and sets
    /// `x`'s scope; a person edits `x/asset.yaml` while the import runs.
    /// The scope write refuses to claim the edited file, so the item fails
    /// as foreign — with or without a later failure (`ghost`) — and the edit
    /// is neither committed nor overwritten.
    async fn scope_edit_meets_a_person_edit(with_ghost: bool) {
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        f.commit_files(
            &f.personal_root,
            p,
            &[
                (
                    "skills/x/asset.yaml",
                    "kind: skill\nname: x\ndescription: The catalog's own long description.\n",
                ),
                ("skills/x/body.md", "Steps.\n"),
            ],
        );
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        let bin = tempfile::tempdir().unwrap();
        let yaml = f.personal_root.join("skills/x/asset.yaml");
        let ssh = ssh_with_home_running(
            bin.path(),
            home.path(),
            &format!("printf '%s' '{PERSON_EDIT}' > '{}'", yaml.display()),
        );
        let mut items = vec![
            import("core", p, "w", "oci"),
            item(
                "core",
                Some(p),
                "skill",
                "x",
                ItemAction::SetScope,
                ItemParams {
                    scope: Some("shared".into()),
                    member: Some("skill/x".into()),
                    ..Default::default()
                },
            ),
        ];
        if with_ghost {
            items.push(assign("ghost", p, "oci"));
        }
        let id = new_card(&f, &items);
        let before = head(&f.personal_root);
        let err = apply_all(&f, id, &ssh).await.unwrap_err();
        assert!(
            err.message
                .starts_with("core: skills/x/asset.yaml was changed during the apply, not by it"),
            "{}",
            err.message
        );
        assert!(
            err.message
                .contains("manual cleanup needed in personal: skills/x/asset.yaml"),
            "{}",
            err.message
        );
        assert_eq!(
            std::fs::read_to_string(&yaml).unwrap(),
            PERSON_EDIT,
            "the edit survives"
        );
        assert_eq!(head(&f.personal_root), before, "nothing committed");
        let v = super::super::get(id, &f.store).unwrap();
        assert_eq!(
            (v.state.as_str(), v.error.as_deref()),
            ("failed", Some(err.message.as_str()))
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_person_edit_to_a_scoped_asset_survives_a_failed_apply() {
        let _g = lock_registry_for_test();
        scope_edit_meets_a_person_edit(true).await;
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_person_edit_to_a_scoped_asset_is_never_committed_as_ours() {
        let _g = lock_registry_for_test();
        scope_edit_meets_a_person_edit(false).await;
    }

    /// Round 3 (c): a person's file created mid-import at a path the import
    /// writes is not overwritten.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_person_file_at_an_import_target_is_not_overwritten() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", DESC);
        let bin = tempfile::tempdir().unwrap();
        let body = f.personal_root.join("skills/w/body.md");
        let ssh = ssh_with_home_running(
            bin.path(),
            home.path(),
            &format!(
                "mkdir -p '{0}/skills/w' && echo theirs > '{0}/skills/w/body.md'",
                f.personal_root.display()
            ),
        );
        let id = new_card(&f, &[import("core", p, "w", "oci")]);
        let before = head(&f.personal_root);
        let err = apply_all(&f, id, &ssh).await.unwrap_err();
        assert!(
            err.message
                .starts_with("core: skills/w/body.md was changed during the apply, not by it"),
            "{}",
            err.message
        );
        assert_eq!(std::fs::read_to_string(&body).unwrap(), "theirs\n");
        assert_eq!(head(&f.personal_root), before);
        assert!(
            err.message
                .contains("manual cleanup needed in personal: skills/w/body.md"),
            "{}",
            err.message
        );
    }

    fn sync_item(layer: &str, cid: i64, host: &str, assets: &[&str]) -> NewChangesetItem {
        item(
            layer,
            Some(cid),
            "host",
            host,
            ItemAction::Sync,
            ItemParams {
                layer: Some(layer.into()),
                assets: assets.iter().map(|a| a.to_string()).collect(),
                ..Default::default()
            },
        )
    }

    fn rollout_card(f: &Fleet, items: &[NewChangesetItem]) -> i64 {
        f.store
            .lock()
            .unwrap()
            .insert_changeset("rollout", "Roll out core", items)
            .unwrap()
            .id
    }

    fn item_states(v: &ChangesetView) -> Vec<&str> {
        v.items.iter().map(|i| i.state.as_str()).collect()
    }

    /// Spec, Rollout apply: plan_sync for the card's hosts + apply_sync —
    /// the skill lands on the host, the layer counts as rolled out, and SB6
    /// then puts a member back by itself (R16, R17).
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_rollout_installs_its_layer_and_sb6_keeps_it_there() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        let asset_yaml = format!("kind: skill\nname: w\ndescription: {DESC}\n");
        f.commit_files(
            &f.personal_root,
            p,
            &[
                ("skills/w/asset.yaml", asset_yaml.as_str()),
                ("skills/w/body.md", "Steps.\n"),
                (
                    "layers/core.yaml",
                    "kind: layer\nname: core\naxis: context\nmembers:\n- skill/w\n",
                ),
            ],
        );
        f.store
            .lock()
            .unwrap()
            .set_host_layers_for("oci", p, None, &["core"])
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        assert_eq!(
            auto_additive(&f.store, &ssh).await.unwrap(),
            0,
            "nothing rolled out yet: SB6 waits"
        );

        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "rollout",
                "Roll out core to oci",
                &[item(
                    "core",
                    Some(p),
                    "host",
                    "oci",
                    ItemAction::Sync,
                    ItemParams {
                        layer: Some("core".into()),
                        assets: vec!["skill/w".into()],
                        ..Default::default()
                    },
                )],
            )
            .unwrap();
        let v = apply(
            ApplyArgs {
                id: card.id,
                positions: None,
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        let installed = home.path().join(".claude/skills/w/SKILL.md");
        assert!(installed.is_file(), "the rollout installed the skill");
        assert_eq!(
            f.store.lock().unwrap().rolled_out_layers().unwrap(),
            BTreeSet::from([(p, "core".to_string())])
        );

        std::fs::remove_dir_all(home.path().join(".claude/skills/w")).unwrap();
        let mut missing = f.store.lock().unwrap().list_inventory().unwrap();
        missing.retain(|r| r.host_alias == "oci" && r.harness == "claude");
        for r in &mut missing {
            if r.name == "w" {
                r.state = "missing".into();
            }
        }
        f.store
            .lock()
            .unwrap()
            .replace_host_inventory("oci", "claude", &missing)
            .unwrap();
        assert_eq!(auto_additive(&f.store, &ssh).await.unwrap(), 1);
        assert!(installed.is_file(), "SB6 put the member back");
        // Redesign 8.7: the automatic write shows as an applied card, marked
        // auto, naming the layer, the host and the asset; never undoable,
        // and the rolled-out layers are as they were.
        let cards = super::super::list(&f.store).unwrap();
        let auto: Vec<_> = cards.iter().filter(|c| c.auto).collect();
        assert_eq!(auto.len(), 1, "{cards:?}");
        let c = auto[0];
        assert_eq!(
            (c.kind.as_str(), c.state.as_str(), c.undoable),
            ("rollout", "applied", false)
        );
        assert_eq!(c.summary, "Synced core to oci");
        assert_eq!(c.error.as_deref(), Some(AUTO_SYNC_NOTE));
        let items = f.store.lock().unwrap().changeset_items(c.id).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            (items[0].name.as_str(), items[0].state.as_str()),
            ("oci", "applied")
        );
        assert!(items[0].params.as_deref().unwrap_or("").contains("skill/w"));
        assert_eq!(
            f.store.lock().unwrap().rolled_out_layers().unwrap(),
            BTreeSet::from([(p, "core".to_string())])
        );
        // A pass with nothing due writes no card.
        assert_eq!(auto_additive(&f.store, &ssh).await.unwrap(), 0);
        let again = super::super::list(&f.store).unwrap();
        assert_eq!(again.iter().filter(|c| c.auto).count(), 1);
    }

    /// R15 (controller ruling): the plan under a Rollout holds a `Remove`
    /// (an asset the catalog dropped) and an `Overwrite` (a copy a person
    /// edited on the host); neither the card nor SB6 applies either — only
    /// the new member's `Create` lands.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_rollout_and_sb6_drop_the_plans_overwrite_and_remove() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        f.commit_files(
            &f.personal_root,
            p,
            &[
                ("skills/gone/asset.yaml", skill_yaml("gone").as_str()),
                ("skills/gone/body.md", "Gone.\n"),
                ("skills/edited/asset.yaml", skill_yaml("edited").as_str()),
                ("skills/edited/body.md", "Catalog steps.\n"),
                (
                    "layers/core.yaml",
                    "kind: layer\nname: core\naxis: context\nmembers:\n- skill/gone\n- skill/edited\n",
                ),
            ],
        );
        f.store
            .lock()
            .unwrap()
            .set_host_layers_for("oci", p, None, &["core"])
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let first = rollout_card(
            &f,
            &[sync_item("core", p, "oci", &["skill/gone", "skill/edited"])],
        );
        let v = apply_all(&f, first, &ssh).await.unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        let skills = home.path().join(".claude/skills");
        assert!(skills.join("gone/SKILL.md").is_file());
        assert!(skills.join("edited/SKILL.md").is_file());

        // The catalog drops `gone` and gains `w`; a person edits `edited`.
        git(&f.personal_root, &["rm", "-rq", "skills/gone"]);
        f.commit_files(
            &f.personal_root,
            p,
            &[
                ("skills/w/asset.yaml", skill_yaml("w").as_str()),
                ("skills/w/body.md", "Steps.\n"),
                (
                    "layers/core.yaml",
                    "kind: layer\nname: core\naxis: context\nmembers:\n- skill/edited\n- skill/w\n",
                ),
            ],
        );
        let hand = "edited by hand\n";
        std::fs::write(skills.join("edited/SKILL.md"), hand).unwrap();

        // The underlying plan would remove one and overwrite the other.
        let planned = sync::plan_sync(
            PlanArgs {
                host_alias: Some("oci".into()),
                allow_unlayered: true,
                ..Default::default()
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap();
        sync::plan::registry_take_with_expiry(&planned.id);
        let ops: BTreeMap<String, ActionOp> = planned
            .hosts
            .iter()
            .filter(|h| h.harness == "claude")
            .flat_map(|h| h.actions.iter())
            .map(|a| (a.name.clone(), a.op))
            .collect();
        assert_eq!(ops.get("gone"), Some(&ActionOp::Remove), "{ops:?}");
        assert_eq!(ops.get("edited"), Some(&ActionOp::Overwrite), "{ops:?}");
        assert_eq!(ops.get("w"), Some(&ActionOp::Create), "{ops:?}");

        let second = rollout_card(
            &f,
            &[sync_item(
                "core",
                p,
                "oci",
                &["skill/gone", "skill/edited", "skill/w"],
            )],
        );
        let v = apply_all(&f, second, &ssh).await.unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        // Assets M5 fix round 1: the edited copy is held back and named, so
        // the host's item is not counted done.
        assert_eq!(item_states(&v), ["skipped"], "{:?}", v.error);
        assert!(
            v.error
                .as_deref()
                .unwrap_or_default()
                .contains("skill/edited on oci: host copy edited — sync it yourself"),
            "{:?}",
            v.error
        );
        assert!(skills.join("w/SKILL.md").is_file(), "the create landed");
        assert!(skills.join("gone/SKILL.md").is_file(), "the remove did not");
        assert_eq!(
            std::fs::read_to_string(skills.join("edited/SKILL.md")).unwrap(),
            hand,
            "the overwrite did not"
        );

        // SB6 finds `edited` drifted and managed: a drifted copy is never
        // due (final review I1), so it plans nothing, applies nothing and
        // records no sync run.
        let runs = last_run(&f);
        assert_eq!(auto_additive(&f.store, &ssh).await.unwrap(), 0);
        assert_eq!(last_run(&f), runs, "a no-op pass writes no sync_runs row");
        assert_eq!(
            std::fs::read_to_string(skills.join("edited/SKILL.md")).unwrap(),
            hand
        );
        assert!(skills.join("gone/SKILL.md").is_file());
    }

    /// The newest `sync_runs` row's id.
    fn last_run(f: &Fleet) -> Option<i64> {
        f.store
            .lock()
            .unwrap()
            .last_sync_run()
            .unwrap()
            .map(|r| r.id)
    }

    /// I1: a Rollout whose narrowed plan is empty (the host already has
    /// everything) is applied — the host counts as done — without applying
    /// anything: no sync_runs row.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_rollout_with_nothing_left_to_do_applies_without_a_sync_run() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let first = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);
        assert_eq!(apply_all(&f, first, &ssh).await.unwrap().state, "applied");
        let runs = last_run(&f);
        assert!(runs.is_some(), "the first rollout synced");

        let again = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);
        let v = apply_all(&f, again, &ssh).await.unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        assert_eq!(item_states(&v), ["applied"]);
        assert_eq!(last_run(&f), runs, "nothing applied, nothing recorded");
    }

    /// Assets M5 fix round 1 (6): a copy fleet wrote and nobody touched,
    /// then a catalog change — the Rollout carries the update and the host
    /// counts as rolled out.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_rollout_updates_a_copy_fleet_verified_untouched() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        person_syncs_oci(&f, &ssh).await;
        let skill = home.path().join(".claude/skills/w/SKILL.md");
        assert!(skill.is_file());

        f.commit_files(
            &f.personal_root,
            p,
            &[("skills/w/body.md", "Steps, revised.\n")],
        );
        let card = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);
        let v = apply_all(&f, card, &ssh).await.unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        assert_eq!(item_states(&v), ["applied"], "{:?}", v.error);
        assert!(std::fs::read_to_string(&skill)
            .unwrap()
            .contains("Steps, revised."));
        assert_eq!(
            f.store.lock().unwrap().rolled_out_layers().unwrap(),
            BTreeSet::from([(p, "core".to_string())])
        );
    }

    /// Strip the file hashes from every manifest entry under `home`: the
    /// entries now read as written before M5.
    fn make_entries_pre_m5(home: &Path) {
        let path = home.join(".claude/.fleet-assets.json");
        let mut m: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for e in m["assets"].as_object_mut().unwrap().values_mut() {
            e.as_object_mut().unwrap().remove("file_hashes");
        }
        std::fs::write(&path, m.to_string()).unwrap();
    }

    /// Assets M6 (R1, carry T1): a pre-M5 copy edited on the host while the
    /// catalog did not change plans as an Overwrite. The Rollout never
    /// applies it, and now says so — the host's item is skipped with a
    /// held line, not counted done.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_rollout_holds_a_pre_m5_copy_that_differs_with_a_line() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        person_syncs_oci(&f, &ssh).await;
        make_entries_pre_m5(home.path());
        let skill = home.path().join(".claude/skills/w/SKILL.md");
        std::fs::write(&skill, "edited on the host\n").unwrap();

        let card = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);
        let v = apply_all(&f, card, &ssh).await.unwrap();
        assert_eq!(item_states(&v), ["skipped"], "{:?}", v.error);
        let o = v.items[0].outcome.clone().expect("an outcome");
        assert_eq!(
            o.held,
            vec![HeldLine {
                kind: "skill".into(),
                name: "w".into(),
                why: HeldWhy::Unverified,
            }]
        );
        assert_eq!(
            std::fs::read_to_string(&skill).unwrap(),
            "edited on the host\n"
        );
    }

    /// R1: the existing pre-M5 hold (catalog changed) is the same line as
    /// data, and the card's `error` text is unchanged for MCP readers.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_held_update_is_an_outcome_and_still_a_note() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        person_syncs_oci(&f, &ssh).await;
        make_entries_pre_m5(home.path());
        f.commit_files(
            &f.personal_root,
            p,
            &[("skills/w/body.md", "Steps, revised.\n")],
        );
        let card = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);
        let v = apply_all(&f, card, &ssh).await.unwrap();
        let o = v.items[0].outcome.clone().expect("an outcome");
        assert_eq!(o.held[0].why, HeldWhy::Unverified);
        assert!(v
            .error
            .unwrap_or_default()
            .contains("skill/w on oci: host copy predates fleet's file hashes — sync it yourself"));
    }

    /// R1: an edited copy (hashes present) is held as `edited`.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn an_edited_copy_is_held_as_edited() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        person_syncs_oci(&f, &ssh).await;
        std::fs::write(home.path().join(".claude/skills/w/SKILL.md"), "mine\n").unwrap();
        let card = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);
        let v = apply_all(&f, card, &ssh).await.unwrap();
        assert_eq!(item_states(&v), ["skipped"]);
        assert_eq!(
            v.items[0].outcome.as_ref().unwrap().held[0].why,
            HeldWhy::Edited
        );
    }

    /// Assets M5 fix round 1 (2): a manifest entry from before M5 cannot
    /// vouch for the host copy, so the Rollout leaves its update to a
    /// person — and says so: the host's item is skipped with a note, never
    /// counted applied, and the layer is not rolled out there.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_rollout_over_a_copy_fleet_cannot_vouch_for_skips_its_host() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        person_syncs_oci(&f, &ssh).await;
        let skill = home.path().join(".claude/skills/w/SKILL.md");
        let before = std::fs::read_to_string(&skill).unwrap();

        // Make the entry one written before M5: no file hashes.
        let manifest = home.path().join(".claude/.fleet-assets.json");
        assert!(
            std::fs::read_to_string(&manifest)
                .unwrap()
                .contains("file_hashes"),
            "the sync recorded file hashes"
        );
        make_entries_pre_m5(home.path());

        f.commit_files(
            &f.personal_root,
            p,
            &[("skills/w/body.md", "Steps, revised.\n")],
        );
        let card = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);
        let v = apply_all(&f, card, &ssh).await.unwrap();
        assert_eq!(item_states(&v), ["skipped"], "{:?}", v.error);
        let note = v.error.clone().unwrap_or_default();
        assert!(
            note.contains(
                "skill/w on oci: host copy predates fleet's file hashes — sync it yourself"
            ),
            "{note}"
        );
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), before);
        assert!(
            f.store
                .lock()
                .unwrap()
                .rolled_out_layers()
                .unwrap()
                .is_empty(),
            "core is not rolled out on oci"
        );
    }

    /// I1: a restore with no update or overwrite planned for its asset (the
    /// host has no copy: that would be a create) fails, naming the host —
    /// it never closes the drift card on a no-op.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_restore_with_nothing_to_restore_fails() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = drift_card(&f, p, "oci");
        let v = restore(&f, id, &ssh).await;
        assert_eq!(v.state, "failed");
        assert_eq!(item_states(&v), ["pending", "pending"]);
        let err = v.error.unwrap_or_default();
        assert!(err.starts_with("nothing to restore on oci"), "{err}");
        assert!(!home.path().join(".claude/skills/w").exists());
        assert_eq!(last_run(&f), None, "nothing applied");
    }

    /// A failed card stays open and its items are retried: the retried
    /// item's outcome is the last attempt's, so a restore that failed first
    /// (nothing there to restore) and then applies carries no stale note.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_retried_item_that_now_applies_loses_its_old_outcome() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = drift_card(&f, p, "oci");
        let v = restore(&f, id, &ssh).await;
        assert_eq!(v.state, "failed");
        let first = v.items[1].outcome.clone().expect("the failure is recorded");
        assert!(first
            .note
            .unwrap_or_default()
            .starts_with("nothing to restore on oci"));

        // The host now holds a differing copy: the restore overwrites it.
        host_skill(home.path(), "w", "Edited on the host, long enough.");
        let v = restore(&f, id, &ssh).await;
        assert_eq!(v.state, "applied", "{:?}", v.error);
        assert_eq!(item_states(&v), ["skipped", "applied"]);
        assert_eq!(v.items[1].outcome, None, "the first attempt's note is gone");
    }

    /// I2: a restore writes the harness its drift was seen on (claude) and
    /// nothing else — codex's differing copy of the same skill, which the
    /// underlying plan would overwrite, is untouched.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_restore_leaves_another_harness_copy_alone() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        f.store
            .lock()
            .unwrap()
            .set_host_harnesses(
                "oci",
                Some(&["claude".to_string(), "codex".to_string()][..]),
            )
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", "Edited on the host, long enough.");
        // Codex's skills live in ~/.agents/skills since F3c.
        let codex = home.path().join(".agents/skills/w/SKILL.md");
        std::fs::create_dir_all(codex.parent().unwrap()).unwrap();
        let theirs = "---\nname: w\ndescription: Codex's own copy, long enough.\n---\nMine.\n";
        std::fs::write(&codex, theirs).unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());

        let planned = sync::plan_sync(
            PlanArgs {
                host_alias: Some("oci".into()),
                ..Default::default()
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap();
        sync::plan::registry_take_with_expiry(&planned.id);
        let codex_op = planned
            .hosts
            .iter()
            .filter(|h| h.harness == "codex")
            .flat_map(|h| h.actions.iter())
            .find(|a| a.name == "w")
            .map(|a| a.op);
        assert_eq!(codex_op, Some(ActionOp::Overwrite), "{planned:?}");

        let id = drift_card(&f, p, "oci");
        let v = restore(&f, id, &ssh).await;
        assert_eq!(v.state, "applied", "{:?}", v.error);
        let claude =
            std::fs::read_to_string(home.path().join(".claude/skills/w/SKILL.md")).unwrap();
        assert!(claude.contains("Steps."), "{claude}");
        assert_eq!(
            std::fs::read_to_string(&codex).unwrap(),
            theirs,
            "codex untouched"
        );
    }

    /// Fix round 1 (4): a restore whose host is skipped (unreachable) fails
    /// and keeps the drift card open — it never closes as applied.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_restore_on_a_skipped_host_fails() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        f.store
            .lock()
            .unwrap()
            .update_host_probe("oci", false, None, None, 2)
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = drift_card(&f, p, "oci");
        let v = restore(&f, id, &ssh).await;
        assert_eq!(v.state, "failed");
        assert_eq!(item_states(&v), ["pending", "pending"]);
        let err = v.error.unwrap_or_default();
        assert!(
            err.starts_with("not restored: oci (claude)") && err.contains("unreachable"),
            "{err}"
        );
    }

    /// An inventory row for `w` on `host` (claude) in `state`.
    fn w_row(f: &Fleet, host: &str, state: &str) -> AssetInventoryRow {
        AssetInventoryRow {
            host_alias: host.into(),
            harness: "claude".into(),
            kind: "skill".into(),
            name: "w".into(),
            state: state.into(),
            catalog_hash: None,
            host_hash: None,
            scanned_at: 1,
            managed: false,
            secret_like: false,
            fleet_owned: false,
            catalog_id: Some(f.personal.id),
            drift_side: None,
        }
    }

    /// A fake ssh that appends a line to `counter` on every remote call
    /// whose arguments match the shell `pattern`.
    fn counting_ssh(bin: &Path, home: &Path, counter: &Path, pattern: &str) -> Arc<SshClient> {
        ssh_with_home_running(
            bin,
            home,
            &format!(
                "case \"$*\" in {pattern}) echo x >> '{}';; esac",
                counter.display()
            ),
        )
    }

    /// R17 / fix round 1 (7): on a pre-M4 fleet (no Rollout ever applied)
    /// SB6 plans nothing and never reaches a host — even with a layered host
    /// missing a member.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn sb6_on_a_pre_m4_fleet_never_plans_or_uses_ssh() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        f.store
            .lock()
            .unwrap()
            .replace_host_inventory("oci", "claude", &[w_row(&f, "oci", "missing")])
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let counter = bin.path().join("calls");
        let ssh = counting_ssh(bin.path(), home.path(), &counter, "*");
        assert_eq!(auto_additive(&f.store, &ssh).await.unwrap(), 0);
        assert!(!counter.exists(), "no ssh call at all");
        assert_eq!(last_run(&f), None);
    }

    /// Fix round 1 (6): a host a person rejected on a layer's Rollout card is
    /// left out by SB6 for that layer — it is never even planned; once the
    /// rejection is lifted, SB6 plans it again.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn sb6_leaves_out_a_host_a_person_rejected() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci", "trn"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = rollout_card(
            &f,
            &[
                sync_item("core", p, "oci", &["skill/w"]),
                sync_item("core", p, "trn", &["skill/w"]),
            ],
        );
        f.store
            .lock()
            .unwrap()
            .set_changeset_item_states(id, &[1], "rejected")
            .unwrap();
        let v = apply_all(&f, id, &ssh).await.unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        assert_eq!(item_states(&v), ["applied", "rejected"]);
        f.store
            .lock()
            .unwrap()
            .replace_host_inventory("trn", "claude", &[w_row(&f, "trn", "missing")])
            .unwrap();

        let bin2 = tempfile::tempdir().unwrap();
        let counter = bin2.path().join("trn-calls");
        let ssh = counting_ssh(bin2.path(), home.path(), &counter, "*'-- trn '*");
        assert_eq!(auto_additive(&f.store, &ssh).await.unwrap(), 0);
        assert!(!counter.exists(), "trn was never planned");

        f.store
            .lock()
            .unwrap()
            .set_changeset_item_states(id, &[1], "pending")
            .unwrap();
        auto_additive(&f.store, &ssh).await.unwrap();
        assert!(counter.exists(), "without the rejection SB6 plans trn");
    }

    /// Final review I3: an SB6 run — here one whose host write fails —
    /// never changes the scan tick's "everything changed" key, so it never
    /// makes the next tick rescan the whole fleet; and the host it failed
    /// on is left out of later passes until its inventory is scanned again.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_failing_sb6_host_neither_rescans_the_fleet_nor_retries_every_pass() {
        use crate::service::catalog::scan_tick::sync_key;
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);
        let v = apply_all(&f, id, &ssh).await.unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        let key = sync_key(&f.store.lock().unwrap());
        assert!(key.is_some(), "a card's run is a person's run");

        std::fs::remove_dir_all(home.path().join(".claude/skills/w")).unwrap();
        let missing = |f: &Fleet, at: i64| {
            let mut row = w_row(f, "oci", "missing");
            row.scanned_at = at;
            f.store
                .lock()
                .unwrap()
                .replace_host_inventory("oci", "claude", &[row])
                .unwrap();
        };
        missing(&f, 1);
        let bin2 = tempfile::tempdir().unwrap();
        let failing = ssh_with_home_running(
            bin2.path(),
            home.path(),
            "case \"$*\" in *fleet-tmp*) exit 7;; esac",
        );
        let runs = last_run(&f);
        assert_eq!(auto_additive(&f.store, &failing).await.unwrap(), 0);
        assert_ne!(last_run(&f), runs, "SB6 tried, and its run is recorded");
        assert_eq!(
            sync_key(&f.store.lock().unwrap()),
            key,
            "an SB6 run is not a reason to rescan the fleet"
        );

        // Its rescan still says `missing`: the next pass leaves oci alone.
        let bin3 = tempfile::tempdir().unwrap();
        let counter = bin3.path().join("calls");
        let counting = counting_ssh(bin3.path(), home.path(), &counter, "*");
        assert_eq!(auto_additive(&f.store, &counting).await.unwrap(), 0);
        assert!(!counter.exists(), "a host SB6 failed on is backed off");

        // A later scan of oci lifts it.
        let later = f.store.lock().unwrap().inventory_last_scans().unwrap()["oci"] + 100;
        missing(&f, later);
        assert_eq!(auto_additive(&f.store, &counting).await.unwrap(), 1);
        assert!(counter.exists());
        assert!(home.path().join(".claude/skills/w/SKILL.md").is_file());
    }

    /// The catalog moved **and** a person edited the host copy — the
    /// planner plans an Overwrite, which no Additive run carries. The row's
    /// side is unknown (`drift_side` unset, as for a manifest entry from
    /// before M5), and `sb6_due` admits a `drifted` row only when it is
    /// `catalog`: so SB6 does not even plan the host, and the copy stays as
    /// the person left it, with nothing planned, applied or recorded.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn sb6_never_touches_a_managed_drifted_copy_on_a_rolled_out_layer() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);
        let v = apply_all(&f, id, &ssh).await.unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        let installed = home.path().join(".claude/skills/w/SKILL.md");
        assert!(installed.is_file());

        f.commit_files(&f.personal_root, p, &[("skills/w/body.md", "New steps.\n")]);
        let hand = "edited by hand\n";
        std::fs::write(&installed, hand).unwrap();
        let mut row = w_row(&f, "oci", "drifted");
        row.managed = true;
        f.store
            .lock()
            .unwrap()
            .replace_host_inventory("oci", "claude", &[row])
            .unwrap();

        let bin2 = tempfile::tempdir().unwrap();
        let counter = bin2.path().join("calls");
        let ssh = counting_ssh(bin2.path(), home.path(), &counter, "*");
        let runs = last_run(&f);
        assert_eq!(auto_additive(&f.store, &ssh).await.unwrap(), 0);
        assert_eq!(std::fs::read_to_string(&installed).unwrap(), hand);
        assert!(!counter.exists(), "a drifted copy is never planned");
        assert_eq!(last_run(&f), runs, "no sync_runs row");
    }

    /// Assets M5 (R6, PF4): through the real SB6 pass (`auto_additive` asks
    /// `sb6_due`), a managed copy on a rolled-out layer that is only behind
    /// its catalog — exactly as the Rollout wrote it — is brought up to
    /// date.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn sb6_brings_a_managed_copy_only_behind_its_catalog_up_to_date() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);
        let v = apply_all(&f, id, &ssh).await.unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        let installed = home.path().join(".claude/skills/w/SKILL.md");
        assert!(!std::fs::read_to_string(&installed)
            .unwrap()
            .contains("New steps."));

        f.commit_files(&f.personal_root, p, &[("skills/w/body.md", "New steps.\n")]);
        let mut row = w_row(&f, "oci", "drifted");
        row.managed = true;
        row.drift_side = Some("catalog".into());
        f.store
            .lock()
            .unwrap()
            .replace_host_inventory("oci", "claude", &[row])
            .unwrap();

        let runs = last_run(&f);
        assert_eq!(auto_additive(&f.store, &ssh).await.unwrap(), 1);
        let now = std::fs::read_to_string(&installed).unwrap();
        assert!(now.contains("New steps."), "{now}");
        assert_ne!(last_run(&f), runs, "SB6 recorded its run");
    }

    /// Assets M5 (R6, PF4, safety): SB6 never acts on the inventory's say-so
    /// alone. A row that says the copy was edited (`host`) is never planned;
    /// a row that says `catalog` over a copy a person edited after that scan
    /// is re-checked by the planner, which plans an overwrite the Additive
    /// filter drops — the person's copy stays either way.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn sb6_leaves_a_copy_edited_on_its_host_whatever_the_scan_said() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);
        let v = apply_all(&f, id, &ssh).await.unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        let installed = home.path().join(".claude/skills/w/SKILL.md");
        f.commit_files(&f.personal_root, p, &[("skills/w/body.md", "New steps.\n")]);
        let hand = "edited by hand\n";
        std::fs::write(&installed, hand).unwrap();
        let put = |side: &str| {
            let mut row = w_row(&f, "oci", "drifted");
            row.managed = true;
            row.drift_side = Some(side.into());
            f.store
                .lock()
                .unwrap()
                .replace_host_inventory("oci", "claude", &[row])
                .unwrap();
        };

        put("host");
        let bin2 = tempfile::tempdir().unwrap();
        let counter = bin2.path().join("calls");
        let counting = counting_ssh(bin2.path(), home.path(), &counter, "*");
        assert_eq!(auto_additive(&f.store, &counting).await.unwrap(), 0);
        assert!(!counter.exists(), "an edited copy is never planned");
        assert_eq!(std::fs::read_to_string(&installed).unwrap(), hand);

        // A stale scan: it saw the copy as fleet wrote it.
        put("catalog");
        let out = sb6_pass(&f.store, &ssh).await.unwrap();
        let oci = &out["oci"];
        assert!(oci.held && !oci.applied, "{oci:?}");
        assert_eq!(
            oci.skipped,
            ["skill/w on oci: host copy edited — sync it yourself"]
        );
        assert_eq!(
            std::fs::read_to_string(&installed).unwrap(),
            hand,
            "the planner's re-check keeps the person's edit"
        );
    }

    /// Final review I1: the asset moved (`install_as`) after its Rollout,
    /// so its new location is `missing` and SB6 plans it — but the old copy
    /// a person edited is not deleted: the plan is an overwrite, the
    /// Additive filter holds it and names the held line. The old copy stays
    /// as the person left it, and nothing is written at the new location.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn sb6_never_deletes_an_edited_old_copy_of_a_moved_asset() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);
        let v = apply_all(&f, id, &ssh).await.unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        let old = home.path().join(".claude/skills/w/SKILL.md");
        assert!(old.is_file());

        let moved = format!("{}install_as: w2\n", skill_yaml("w"));
        f.commit_files(
            &f.personal_root,
            p,
            &[("skills/w/asset.yaml", moved.as_str())],
        );
        let hand = "edited by hand\n";
        std::fs::write(&old, hand).unwrap();
        f.store
            .lock()
            .unwrap()
            .replace_host_inventory("oci", "claude", &[w_row(&f, "oci", "missing")])
            .unwrap();

        let runs = last_run(&f);
        let out = sb6_pass(&f.store, &ssh).await.unwrap();
        let oci = &out["oci"];
        assert!(oci.held && !oci.applied, "{oci:?}");
        assert_eq!(
            oci.skipped,
            ["skill/w on oci: host copy edited — sync it yourself"]
        );
        assert_eq!(std::fs::read_to_string(&old).unwrap(), hand);
        assert!(!home.path().join(".claude/skills/w2").exists());
        assert_eq!(last_run(&f), runs, "nothing applied, no sync_runs row");
    }

    /// A drift card on `skill/w` at `host`: take_host (0), restore (1).
    fn drift_card(f: &Fleet, cid: i64, host: &str) -> i64 {
        let drift = |action| {
            item(
                "drift",
                Some(cid),
                "skill",
                "w",
                action,
                ItemParams {
                    host: Some(host.into()),
                    hash: Some("e".into()),
                    ..Default::default()
                },
            )
        };
        f.store
            .lock()
            .unwrap()
            .insert_changeset(
                "drift",
                &format!("skill/w differs on {host} from catalog personal"),
                &[drift(ItemAction::TakeHost), drift(ItemAction::Restore)],
            )
            .unwrap()
            .id
    }

    async fn restore(f: &Fleet, id: i64, ssh: &Arc<SshClient>) -> ChangesetView {
        apply(
            ApplyArgs {
                id,
                positions: Some(vec![1]),
            },
            &f.store,
            ssh,
        )
        .await
        .unwrap()
    }

    /// R15 + PF4: a drift restore puts the catalog copy back on one host —
    /// overwriting the host's edit, with a backup — even on a host with no
    /// layers; nothing touches the catalog.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_drift_restore_overwrites_one_host_copy_with_a_backup() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        f.commit_files(
            &f.personal_root,
            p,
            &[
                ("skills/w/asset.yaml", skill_yaml("w").as_str()),
                ("skills/w/body.md", "Catalog steps.\n"),
            ],
        );
        let home = tempfile::tempdir().unwrap();
        host_skill(home.path(), "w", "Edited on the host, long enough.");
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let drift = |action| {
            item(
                "drift",
                Some(p),
                "skill",
                "w",
                action,
                ItemParams {
                    host: Some("oci".into()),
                    hash: Some("e".into()),
                    ..Default::default()
                },
            )
        };
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(
                "drift",
                "skill/w differs on oci from catalog personal",
                &[drift(ItemAction::TakeHost), drift(ItemAction::Restore)],
            )
            .unwrap();
        let before = head(&f.personal_root);
        let v = apply(
            ApplyArgs {
                id: card.id,
                positions: Some(vec![1]),
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        assert_eq!(item_states(&v), ["skipped", "applied"]);
        let dir = home.path().join(".claude/skills/w");
        let now = std::fs::read_to_string(dir.join("SKILL.md")).unwrap();
        assert!(now.contains("Catalog steps."), "{now}");
        let backups: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".fleet-bak-"))
            .collect();
        assert_eq!(backups.len(), 1, "the host's edit is backed up");
        assert_eq!(head(&f.personal_root), before, "the catalog is untouched");
        assert!(!v.undoable, "a restore changed no catalog");
    }

    /// PF5: a host the sync skips (unreachable) is reported on the card,
    /// not a failure; its item is skipped, the reachable host's applied.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_skipped_host_is_reported_not_a_failure() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci", "trn"]);
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
            s.set_host_layers_for("oci", p, None, &["core"]).unwrap();
            s.set_host_layers_for("trn", p, None, &["core"]).unwrap();
            s.update_host_probe("trn", false, None, None, 2).unwrap();
        }
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = rollout_card(
            &f,
            &[
                sync_item("core", p, "oci", &["skill/w"]),
                sync_item("core", p, "trn", &["skill/w"]),
            ],
        );
        let v = apply_all(&f, id, &ssh).await.unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        assert_eq!(item_states(&v), ["applied", "skipped"]);
        let note = v.error.unwrap_or_default();
        assert!(
            note.contains("trn (claude)") && note.contains("unreachable"),
            "{note}"
        );
        assert!(home.path().join(".claude/skills/w/SKILL.md").is_file());
    }

    /// R15: a host whose sync fails leaves the card `failed` naming it; the
    /// host that applied keeps its item `applied`; applying the card again
    /// finishes the rest.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_failed_host_fails_the_card_and_the_applied_host_stays_applied() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci", "trn"]);
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
            s.set_host_layers_for("oci", p, None, &["core"]).unwrap();
            s.set_host_layers_for("trn", p, None, &["core"]).unwrap();
        }
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        // trn's write scripts fail; its scans (and all of oci) work.
        let ssh = ssh_with_home_running(
            bin.path(),
            home.path(),
            "case \"$*\" in *'-- trn '*fleet-tmp*) exit 7;; esac",
        );
        let id = rollout_card(
            &f,
            &[
                sync_item("core", p, "oci", &["skill/w"]),
                sync_item("core", p, "trn", &["skill/w"]),
            ],
        );
        let v = apply_all(&f, id, &ssh).await.unwrap();
        assert_eq!(v.state, "failed");
        assert_eq!(item_states(&v), ["applied", "pending"]);
        let err = v.error.unwrap_or_default();
        assert!(err.contains("trn (claude)"), "{err}");
        assert!(!err.contains("oci"), "{err}");
        assert_eq!(
            f.store.lock().unwrap().rolled_out_layers().unwrap(),
            BTreeSet::from([(p, "core".to_string())]),
            "a host applied: the layer is rolled out"
        );

        let bin2 = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin2.path(), home.path());
        let v = apply_all(&f, id, &ssh).await.unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        assert_eq!(item_states(&v), ["applied", "applied"]);
    }

    /// apply_sync's secret gate holds for a card: an asset waiting on a
    /// `${NAME}` with no value refuses the whole rollout, names it, and
    /// writes nothing.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_missing_secret_refuses_the_rollout() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let p = f.personal.id;
        f.commit_files(
            &f.personal_root,
            p,
            &[
                ("skills/w/asset.yaml", skill_yaml("w").as_str()),
                ("skills/w/body.md", "token ${MISSING_SECRET}\n"),
            ],
        );
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);
        let err = apply_all(&f, id, &ssh).await.unwrap_err();
        assert_eq!(err.code, codes::E_SECRET_MISSING);
        assert!(err.message.contains("MISSING_SECRET"), "{}", err.message);
        assert!(!home.path().join(".claude/skills/w").exists());
        let v = super::super::get(id, &f.store).unwrap();
        assert_eq!(v.state, "failed");
        assert_eq!(item_states(&v), ["pending"]);
    }

    /// R17: the scan tick's hook returns before SB6 runs — SB6 is a
    /// detached task — and SB6 then syncs the rolled-out layer by itself.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn the_tick_hook_never_waits_for_sb6() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
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
        f.store
            .lock()
            .unwrap()
            .set_host_layers_for("oci", p, None, &["core"])
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);
        assert_eq!(apply_all(&f, id, &ssh).await.unwrap().state, "applied");
        let installed = home.path().join(".claude/skills/w/SKILL.md");
        std::fs::remove_dir_all(home.path().join(".claude/skills/w")).unwrap();
        {
            let s = f.store.lock().unwrap();
            let mut rows = s.list_inventory().unwrap();
            rows.retain(|r| r.host_alias == "oci" && r.harness == "claude");
            for r in &mut rows {
                if r.name == "w" {
                    r.state = "missing".into();
                }
            }
            s.replace_host_inventory("oci", "claude", &rows).unwrap();
        }

        super::super::reconcile::after_scan_pass(&f.store, &ssh);
        assert!(!installed.exists(), "the hook returned before SB6 ran");
        for _ in 0..400 {
            if installed.is_file() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(installed.is_file(), "SB6 ran detached and put w back");
        // Let the detached task finish before the registry guard drops.
        let _done = APPLY_LOCK.lock().await;
    }

    // ── Assets M6 (R5): layer cards ──────────────────────────────────────

    use crate::service::catalog::changesets::{propose_layer, LayerChange};

    async fn propose_and_apply(
        f: &Fleet,
        change: LayerChange,
        ssh: &Arc<SshClient>,
    ) -> ChangesetView {
        let v = propose_layer(change, &f.store).await.unwrap();
        apply_all(f, v.id, ssh).await.unwrap()
    }

    fn layer_in(f: &Fleet, name: &str) -> Layer {
        let text =
            std::fs::read_to_string(f.personal_root.join(format!("layers/{name}.yaml"))).unwrap();
        Layer::from_yaml(&text).unwrap()
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_create_layer_card_commits_the_layer_file_and_undoes() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        let v = propose_and_apply(
            &f,
            LayerChange::Create {
                catalog: None,
                layer: "servers".into(),
                axis: None,
                description: Some("On the servers".into()),
                members: vec!["skill/w".into()],
                orgs: vec![],
            },
            &ssh,
        )
        .await;
        assert_eq!(v.state, "applied", "{:?}", v.error);
        assert!(v.undoable);
        let file = f.personal_root.join("layers/servers.yaml");
        let l = layer_in(&f, "servers");
        assert_eq!(
            (l.axis, l.members.clone(), l.description.as_str()),
            (Axis::Context, vec!["skill/w".to_string()], "On the servers")
        );
        assert_eq!(
            subjects(&f.personal_root)[0],
            "fleet: New layer servers in personal"
        );
        let u = crate::service::catalog::changesets::undo::undo(v.id, &f.store)
            .await
            .unwrap();
        assert_eq!(u.state, "undone");
        assert!(!file.exists());
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_create_layer_card_writes_the_orgs_it_applies_by() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        f.store
            .lock()
            .unwrap()
            .add_org("Papaya", None, false)
            .unwrap();
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        let v = propose_and_apply(
            &f,
            LayerChange::Create {
                catalog: None,
                layer: "papaya".into(),
                axis: None,
                description: None,
                members: vec!["skill/w".into()],
                orgs: vec!["Papaya".into()],
            },
            &ssh,
        )
        .await;
        assert_eq!(v.state, "applied", "{:?}", v.error);
        assert_eq!(layer_in(&f, "papaya").orgs, vec!["Papaya".to_string()]);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_rename_layer_card_renames_the_file_its_children_and_host_layers() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        f.commit_files(
            &f.personal_root,
            p,
            &[(
                "layers/server.yaml",
                "kind: layer\nname: server\naxis: role\n",
            )],
        );
        f.commit_files(
            &f.personal_root,
            p,
            &[(
                "layers/edge.yaml",
                "kind: layer\nname: edge\naxis: role\nextends: server\n",
            )],
        );
        f.store
            .lock()
            .unwrap()
            .set_host_layers_for("oci", p, Some("server"), &["core"])
            .unwrap();
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        let v = propose_and_apply(
            &f,
            LayerChange::Rename {
                catalog: None,
                layer: "server".into(),
                to: "servers".into(),
            },
            &ssh,
        )
        .await;
        assert_eq!(v.state, "applied", "{:?}", v.error);
        assert!(!f.personal_root.join("layers/server.yaml").exists());
        assert_eq!(layer_in(&f, "servers").name, "servers");
        assert_eq!(layer_in(&f, "edge").extends.as_deref(), Some("servers"));
        assert_eq!(v.commits.len(), 1, "one commit");
        assert!(repo::is_clean(&f.personal_root).unwrap(), "all committed");
        let rows = f
            .store
            .lock()
            .unwrap()
            .get_host_layers_for("oci", p)
            .unwrap();
        assert!(rows
            .iter()
            .any(|r| r.axis == "role" && r.layer_name == "servers"));
        assert!(rows
            .iter()
            .any(|r| r.axis == "context" && r.layer_name == "core"));
        // Undo puts back the files and the host's role.
        let u = crate::service::catalog::changesets::undo::undo(v.id, &f.store)
            .await
            .unwrap();
        assert_eq!(u.state, "undone");
        assert!(f.personal_root.join("layers/server.yaml").exists());
        assert!(!f.personal_root.join("layers/servers.yaml").exists());
        assert_eq!(layer_in(&f, "edge").extends.as_deref(), Some("server"));
        let rows = f
            .store
            .lock()
            .unwrap()
            .get_host_layers_for("oci", p)
            .unwrap();
        assert!(rows
            .iter()
            .any(|r| r.axis == "role" && r.layer_name == "server"));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_move_member_card_moves_it_in_one_commit() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        f.add_layer("extra", &[]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        let v = propose_and_apply(
            &f,
            LayerChange::Move {
                catalog: None,
                member: "skill/w".into(),
                layer: "core".into(),
                to: "extra".into(),
            },
            &ssh,
        )
        .await;
        assert_eq!(v.state, "applied", "{:?}", v.error);
        assert!(layer_in(&f, "core").members.is_empty());
        assert_eq!(layer_in(&f, "extra").members, vec!["skill/w".to_string()]);
        assert_eq!(v.commits.len(), 1);
        assert_eq!(
            subjects(&f.personal_root)[0],
            "fleet: Move skill/w from core to extra in personal"
        );
    }

    /// Final review minor 1: a moved member's override goes with it.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_move_member_card_carries_the_members_override() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        f.commit_files(
            &f.personal_root,
            f.personal.id,
            &[(
                "layers/core.yaml",
                "kind: layer\nname: core\naxis: context\nmembers:\n- skill/w\n\
                 overrides:\n  skill/w:\n    version: \"2\"\n",
            )],
        );
        f.add_layer("extra", &[]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        let v = propose_and_apply(
            &f,
            LayerChange::Move {
                catalog: None,
                member: "skill/w".into(),
                layer: "core".into(),
                to: "extra".into(),
            },
            &ssh,
        )
        .await;
        assert_eq!(v.state, "applied", "{:?}", v.error);
        assert!(layer_in(&f, "core").overrides.is_empty());
        let extra = layer_in(&f, "extra");
        assert_eq!(extra.members, vec!["skill/w".to_string()]);
        assert_eq!(
            extra
                .overrides
                .get("skill/w")
                .and_then(|o| o.get("version")),
            Some(&serde_yaml::Value::from("2"))
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_layer_card_never_deletes_a_file_someone_changed() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        let v = propose_layer(
            LayerChange::Rename {
                catalog: None,
                layer: "core".into(),
                to: "base".into(),
            },
            &f.store,
        )
        .await
        .unwrap();
        let before = head(&f.personal_root);
        // An uncommitted edit makes the checkout dirty: refused before
        // anything changes.
        std::fs::write(
            f.personal_root.join("layers/core.yaml"),
            "kind: layer\nname: core\naxis: context\n# mine\n",
        )
        .unwrap();
        let e = apply_all(&f, v.id, &ssh).await.unwrap_err();
        assert_eq!(e.code, codes::E_INVALID_STATE);
        assert!(
            std::fs::read_to_string(f.personal_root.join("layers/core.yaml"))
                .unwrap()
                .contains("# mine")
        );
        assert!(!f.personal_root.join("layers/base.yaml").exists());
        assert_eq!(head(&f.personal_root), before);
        let rows = f
            .store
            .lock()
            .unwrap()
            .get_host_layers_for("oci", f.personal.id)
            .unwrap();
        assert!(rows.iter().any(|r| r.layer_name == "core"));
    }

    /// R12 for a layer card: a rename that cannot read one of the
    /// catalog's layer files fails before it writes or deletes anything, and
    /// the checkout and host_layers stay exactly as they were.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_rename_that_cannot_read_a_layer_file_fails_and_changes_nothing() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        // No axis: the catalog loads (it is a problem, not an error), but
        // the rename cannot tell whether it extends `core`.
        f.commit_files(
            &f.personal_root,
            p,
            &[("layers/broken.yaml", "kind: layer\nname: broken\n")],
        );
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        let v = propose_layer(
            LayerChange::Rename {
                catalog: None,
                layer: "core".into(),
                to: "base".into(),
            },
            &f.store,
        )
        .await
        .unwrap();
        let before = head(&f.personal_root);
        let layers_before = f.store.lock().unwrap().list_all_host_layers().unwrap();
        let e = apply_all(&f, v.id, &ssh).await.unwrap_err();
        assert!(e.message.contains("layers/broken.yaml"), "{}", e.message);
        assert!(f.personal_root.join("layers/core.yaml").is_file());
        assert!(!f.personal_root.join("layers/base.yaml").exists());
        assert!(repo::is_clean(&f.personal_root).unwrap());
        assert_eq!(head(&f.personal_root), before);
        let s = f.store.lock().unwrap();
        assert_eq!(s.list_all_host_layers().unwrap(), layers_before);
        let card = s.get_changeset(v.id).unwrap().unwrap();
        assert_eq!(card.state, "failed");
    }

    /// PF7 for a layer card: every path it writes or deletes is claimed
    /// before the first change, so a child layer someone edited meanwhile
    /// (a writer APPLY_LOCK cannot see) stops the rename before it writes
    /// the new file or deletes the old one; the edit is never written over,
    /// and the failed card resets nothing over it.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_rename_never_writes_over_a_child_someone_edited() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        f.commit_files(
            &f.personal_root,
            p,
            &[(
                "layers/edge.yaml",
                "kind: layer\nname: edge\naxis: context\nextends: core\n",
            )],
        );
        let v = propose_layer(
            LayerChange::Rename {
                catalog: None,
                layer: "core".into(),
                to: "base".into(),
            },
            &f.store,
        )
        .await
        .unwrap();
        let before = head(&f.personal_root);
        let layers_before = f.store.lock().unwrap().list_all_host_layers().unwrap();
        // Step 2d as `run_steps` calls it, after the clean-checkout check —
        // with the edit landing in between.
        let mut progress = Progress {
            pre: BTreeMap::from([(p, before.clone())]),
            ..Default::default()
        };
        let edge = f.personal_root.join("layers/edge.yaml");
        let mine = "kind: layer\nname: edge\naxis: context\nextends: core\n# mine\n";
        std::fs::write(&edge, mine).unwrap();
        let (card, items) = super::super::card(v.id, &f.store).unwrap();
        let selected: Vec<&ChangesetItemRow> = items.iter().collect();
        let rows = vec![f.personal.clone()];
        let failure = match apply_layer_items(&selected, &rows, &mut progress) {
            Ok(_) => panic!("the rename went ahead over an edited child"),
            Err(failure) => failure,
        };
        assert_eq!(failure.error.code, codes::E_INVALID_STATE);
        assert!(
            failure.error.message.contains("layers/edge.yaml"),
            "{}",
            failure.error.message
        );
        assert!(f.personal_root.join("layers/core.yaml").is_file());
        assert!(!f.personal_root.join("layers/base.yaml").exists());
        let e = fail_card(
            &card,
            &items,
            &rows,
            &progress.pre.clone(),
            &progress,
            &layers_before,
            failure,
            &f.store,
        );
        assert!(
            e.message.contains("manual cleanup needed in personal"),
            "{}",
            e.message
        );
        assert_eq!(std::fs::read_to_string(&edge).unwrap(), mine);
        assert!(f.personal_root.join("layers/core.yaml").is_file());
        assert!(!f.personal_root.join("layers/base.yaml").exists());
        assert_eq!(head(&f.personal_root), before, "nothing committed");
        let s = f.store.lock().unwrap();
        assert_eq!(s.list_all_host_layers().unwrap(), layers_before);
        assert_eq!(s.get_changeset(v.id).unwrap().unwrap().state, "failed");
    }

    /// Final review I2: a rename re-derives the open cards that name the old
    /// layer in the same apply. An open New card adopting into `core` is
    /// refreshed by the pass to adopt into `base` (and so never re-creates
    /// `layers/core.yaml`); an open Rollout of `core` is withdrawn, saying
    /// why.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_rename_rederives_open_cards_and_withdraws_the_old_layers_rollouts() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let p = f.personal.id;
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        host_skill(home.path(), "x", DESC);
        let ssh = ssh_with_home(bin.path(), home.path());
        f.store
            .lock()
            .unwrap()
            .replace_host_inventory(
                "oci",
                "claude",
                &[AssetInventoryRow {
                    host_alias: "oci".into(),
                    harness: "claude".into(),
                    kind: "skill".into(),
                    name: "x".into(),
                    state: "unmanaged".into(),
                    host_hash: Some("h-x".into()),
                    scanned_at: 1,
                    ..Default::default()
                }],
            )
            .unwrap();
        super::super::reconcile::reconcile(&f.store, false).unwrap();
        let adopt_layer = |f: &Fleet| -> (String, Option<String>, String) {
            let s = f.store.lock().unwrap();
            let card = s
                .list_changesets()
                .unwrap()
                .into_iter()
                .find(|c| c.kind == "new")
                .expect("the New card for skill/x");
            let items = s.changeset_items(card.id).unwrap();
            let import = items
                .iter()
                .find(|i| i.action == "import" && i.name == "x")
                .expect("its import");
            (
                card.state.clone(),
                ItemParams::parse(import.params.as_deref()).layer,
                import.grp.clone(),
            )
        };
        assert_eq!(
            adopt_layer(&f),
            ("proposed".into(), Some("core".into()), "core".into())
        );
        let rollout = rollout_card(&f, &[sync_item("core", p, "oci", &["skill/w"])]);

        let v = propose_and_apply(
            &f,
            LayerChange::Rename {
                catalog: None,
                layer: "core".into(),
                to: "base".into(),
            },
            &ssh,
        )
        .await;
        assert_eq!(v.state, "applied", "{:?}", v.error);

        let r = f
            .store
            .lock()
            .unwrap()
            .get_changeset(rollout)
            .unwrap()
            .unwrap();
        assert_eq!(r.state, "dismissed");
        assert_eq!(
            r.error.as_deref(),
            Some("withdrawn: layer core was renamed to base")
        );
        assert_eq!(
            adopt_layer(&f),
            ("proposed".into(), Some("base".into()), "base".into()),
            "the pass refreshed the New card onto the renamed layer"
        );
        let adopt = f
            .store
            .lock()
            .unwrap()
            .list_changesets()
            .unwrap()
            .into_iter()
            .find(|c| c.kind == "new")
            .unwrap()
            .id;
        let a = apply_all(&f, adopt, &ssh).await.unwrap();
        assert_eq!(a.state, "applied", "{:?}", a.error);
        assert!(
            !f.personal_root.join("layers/core.yaml").exists(),
            "the old layer is not re-created"
        );
        assert!(layer_in(&f, "base")
            .members
            .contains(&"skill/x".to_string()));
        assert_eq!(contexts(&f, "oci", p), vec!["base".to_string()]);
    }
}
