//! Assets M4: undo (SB5, Rulings R20) and a person's no — dismiss and
//! reject_item (R9, R10).
//!
//! **Undo** is `git revert` of the card's commits (one per catalog) plus its
//! stored `host_layers` snapshot. Only an applied bootstrap, new or
//! drift-take_host card that committed to a catalog can be undone (a
//! hide-only card committed nothing: "nothing to undo", PF12), and only
//! while it is the latest applied catalog-changing card in every catalog it
//! touched (R20: "undo #N first"). It never touches hosts; an undone
//! take_host proposes the Rollout that carries the catalog's restored copy
//! back (R14), and the card's still-open follow-up Rollout loses the assets
//! the undo took out of the catalog — withdrawn when nothing is left (PF11).
//!
//! Data safety (PF7, the apply's lessons): under [`APPLY_LOCK`] every
//! touched checkout must be clean — every untracked file counts, whatever
//! the config — and still hold the card's commit, or undo refuses and
//! changes nothing. A revert writes exactly the card's commit's files:
//! never `add -A`, never `reset --hard` or `clean`. A revert that fails (a
//! conflict with a later hand-made commit) is aborted and then checked —
//! HEAD back at the pre-undo HEAD, the tree clean, no revert in progress —
//! and every catalog this undo already reverted is put back file by file
//! with the apply's guard (`repo::foreign_changes`, `repo::reset_paths`),
//! or left for a person, and said so ("fix by hand"), when anything foreign
//! is there. Nothing is ever forced.
//!
//! Restoring the snapshot REPLACES every `host_layers` row of each touched
//! catalog with the rows it held before the apply (hosts deleted since are
//! skipped): a layer change made in that catalog after the card — by hand,
//! or for a host the card did not name — is undone with it.
//!
//! **Dismiss** rejects every pending item of an open card; **reject_item**
//! some of them, and closes the card once nothing is pending. Each rejected
//! import, hide or sync item holds its subject by content hash (R9) — a
//! drift card's only once both its items are rejected; set_scope and
//! assign_layer carry none. Every such verdict is a `person`'s (R10), and
//! the store never replaces a person's verdict with a rule's.

use super::apply::{propose_follow_up, rollout_summary, stamp_gap_hashes};
use super::{
    applied_catalogs, changes_catalog, is_open, later_card, CardKind, ChangesetView, Decider,
    ItemAction, ItemParams, APPLY_LOCK,
};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::catalog::repo;
use crate::service::settings;
use crate::store::{
    now_unix, CatalogRow, ChangesetItemRow, ChangesetRow, HostLayerRow, NewChangesetItem, Store,
    TriageVerdictRow,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Mutex;

/// Undo card `id` and answer it as it now stands.
pub async fn undo(id: i64, store: &Mutex<Store>) -> Result<ChangesetView, IpcError> {
    let _busy = APPLY_LOCK.lock().await;
    let (card, items) = super::card(id, store)?;
    refuse_unless_undoable(&card, &items)?;
    let touched = applied_catalogs(&items);
    let rows = {
        let s = lock(store)?;
        if let Some((later, catalog)) = later_card(&card, &items, &s)? {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "card #{later} was applied after card #{id} in catalog {catalog}; undo \
                     #{later} first"
                ),
            ));
        }
        let mut rows = Vec::new();
        for cid in &touched {
            rows.push(s.get_catalog(*cid)?.ok_or_else(|| {
                IpcError::new(
                    codes::E_NOTFOUND,
                    format!("catalog {cid} no longer exists; card {id} cannot be undone"),
                )
            })?);
        }
        rows
    };
    let commits = commits_of(&card)?;
    let snapshot: Vec<HostLayerRow> =
        serde_json::from_str(card.layers_snapshot.as_deref().unwrap_or("[]")).map_err(|e| {
            IpcError::new(
                codes::E_PARSE,
                format!("card {id}'s host layers snapshot: {e}"),
            )
        })?;

    // PF7 / R20: every touched checkout clean, and still holding the card's
    // commit — or nothing happens.
    let mut pre: BTreeMap<i64, String> = BTreeMap::new();
    for row in &rows {
        let root = Path::new(&row.repo_path);
        if !repo::is_clean(root)? {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "catalog {} has uncommitted changes; commit them (catalog_admin \
                     commit_pending) or discard them before undoing card {id}",
                    row.name
                ),
            ));
        }
        let at = repo::head(root)?;
        if let Some(sha) = commits.get(&row.id) {
            if !repo::is_ancestor(root, sha, &at)? {
                return Err(IpcError::new(
                    codes::E_INVALID_STATE,
                    format!(
                        "card {id}'s commit {} is no longer in catalog {}'s history (reset or \
                         rewritten since?); nothing was changed — undo it by hand",
                        short(sha),
                        row.name
                    ),
                ));
            }
        }
        pre.insert(row.id, at);
    }

    // One revert per catalog the card committed to.
    let mut done: Vec<Reverted> = Vec::new();
    for row in &rows {
        let Some(sha) = commits.get(&row.id) else {
            continue;
        };
        let root = Path::new(&row.repo_path);
        let at = &pre[&row.id];
        let what = format!("catalog {}", row.name);
        let dirs = match dirs_a_revert_creates(root, sha) {
            Ok(d) => d,
            Err(e) => return Err(give_up(id, &what, e, None, &done, store)),
        };
        match repo::revert(root, sha) {
            Ok(commit) => done.push(Reverted {
                row,
                pre: at.clone(),
                commit,
                dirs,
            }),
            Err(e) => {
                let left = still_as_it_was(row, at);
                return Err(give_up(id, &what, e, left, &done, store));
            }
        }
    }

    // The snapshot and the card's state, in one transaction.
    let catalogs: Vec<i64> = touched.iter().copied().collect();
    let recorded = lock(store).and_then(|s| {
        s.record_changeset_undone(id, &catalogs, &snapshot)
            .map_err(IpcError::from)
    });
    match recorded {
        Ok(true) => {}
        Ok(false) => {
            let e = IpcError::new(codes::E_INVALID_STATE, "the card is no longer applied");
            return Err(give_up(id, "recording the undo", e, None, &done, store));
        }
        Err(e) => return Err(give_up(id, "recording the undo", e, None, &done, store)),
    }
    after_undo(&card, &items, &rows, &done, store);
    super::get(id, store)
}

/// R20 / PF12: why card `card` cannot be undone, if it cannot.
fn refuse_unless_undoable(card: &ChangesetRow, items: &[ChangesetItemRow]) -> Result<(), IpcError> {
    let refuse = |why: String| Err(IpcError::new(codes::E_INVALID_STATE, why));
    if card.state != "applied" {
        return refuse(format!(
            "card {} is {}; only an applied card can be undone",
            card.id, card.state
        ));
    }
    let host_card = card.kind == CardKind::Rollout.as_str()
        || (card.kind == CardKind::Drift.as_str()
            && !items
                .iter()
                .any(|i| i.state == "applied" && i.action == ItemAction::TakeHost.as_str()));
    if host_card {
        return refuse(format!(
            "card {} wrote to hosts, not to a catalog, and undo never touches hosts; nothing \
             to undo",
            card.id
        ));
    }
    if applied_catalogs(items).is_empty() {
        return refuse(format!(
            "card {} changed no catalog (it only hid assets); nothing to undo",
            card.id
        ));
    }
    if !changes_catalog(card, items) {
        return refuse(format!(
            "card {} is a {} card; only an applied bootstrap, new or take_host card can be \
             undone",
            card.id, card.kind
        ));
    }
    Ok(())
}

/// The card's commits by catalog id.
fn commits_of(card: &ChangesetRow) -> Result<BTreeMap<i64, String>, IpcError> {
    let bad =
        |why: String| IpcError::new(codes::E_PARSE, format!("card {}'s commits: {why}", card.id));
    let raw: BTreeMap<String, String> =
        serde_json::from_str(card.commits.as_deref().unwrap_or("{}"))
            .map_err(|e| bad(e.to_string()))?;
    raw.into_iter()
        .map(|(k, sha)| {
            k.parse::<i64>()
                .map(|id| (id, sha))
                .map_err(|_| bad(format!("{k} is not a catalog id")))
        })
        .collect()
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// A revert this undo made: the catalog, its HEAD before, the revert
/// commit, and the directories the revert created (shallowest first).
struct Reverted<'a> {
    row: &'a CatalogRow,
    pre: String,
    commit: String,
    dirs: Vec<String>,
}

/// The directories reverting `sha` would create: every missing parent of a
/// file the card's commit touched (a revert re-creates what it deleted).
/// Recorded before the revert, so putting it back removes exactly those —
/// and only when empty.
fn dirs_a_revert_creates(root: &Path, sha: &str) -> Result<Vec<String>, IpcError> {
    let parent = repo::parent_of(root, sha)?;
    let mut dirs: Vec<String> = Vec::new();
    for f in repo::files_between(root, &parent, sha)? {
        for (i, _) in f.match_indices('/') {
            let d = &f[..i];
            if !dirs.iter().any(|x| x == d) && std::fs::symlink_metadata(root.join(d)).is_err() {
                dirs.push(d.to_string());
            }
        }
    }
    Ok(dirs)
}

/// After a failed revert (aborted by `repo::revert`): `None` when the
/// catalog is exactly as it was — HEAD at `at`, the tree clean, no revert in
/// progress — else what a person must fix.
fn still_as_it_was(row: &CatalogRow, at: &str) -> Option<String> {
    let root = Path::new(&row.repo_path);
    let mut wrong = Vec::new();
    if repo::revert_in_progress(root) {
        wrong.push("a revert is still in progress".to_string());
    }
    match repo::head(root) {
        Ok(h) if h == at => {}
        Ok(h) => wrong.push(format!("HEAD is {}, not {}", short(&h), short(at))),
        Err(e) => wrong.push(format!("HEAD unreadable: {}", e.message)),
    }
    match repo::is_clean(root) {
        Ok(true) => {}
        Ok(false) => wrong.push("the tree has changes".to_string()),
        Err(e) => wrong.push(format!("status unreadable: {}", e.message)),
    }
    (!wrong.is_empty()).then(|| {
        format!(
            "catalog {} is not back as it was ({}) — fix by hand",
            row.name,
            wrong.join(", ")
        )
    })
}

/// Put one revert back — `None` when done, else what a person must fix.
/// The apply's guard: the revert must sit directly on `pre`, and every
/// change in the tree (and every file beside one of its own) must be one of
/// the revert's own files; then exactly those go back to `pre` and the
/// directories it created go when empty. Otherwise the catalog is left as
/// it is.
fn put_back(r: &Reverted<'_>) -> Option<String> {
    let root = Path::new(&r.row.repo_path);
    let name = &r.row.name;
    let checked = |e: IpcError| {
        Some(format!(
            "catalog {name} could not be checked before its revert was put back: {} — fix by \
             hand",
            e.message
        ))
    };
    match repo::parent_of(root, &r.commit) {
        Ok(p) if p == r.pre => {}
        Ok(p) => {
            return Some(format!(
                "manual cleanup needed in {name}: this undo's revert {} sits on {}, not on {} \
                 (nothing there was reset)",
                short(&r.commit),
                short(&p),
                short(&r.pre)
            ))
        }
        Err(e) => return checked(e),
    }
    let ours = match repo::files_between(root, &r.pre, &r.commit) {
        Ok(f) => f,
        Err(e) => return checked(e),
    };
    match repo::foreign_changes(root, &[r.pre.as_str(), r.commit.as_str()], &ours) {
        Ok(foreign) if foreign.is_empty() => repo::reset_paths(root, &r.pre, &ours, &r.dirs)
            .err()
            .map(|e| {
                format!(
                    "catalog {name}'s revert could not be put back to {}: {} — fix by hand",
                    short(&r.pre),
                    e.message
                )
            }),
        Ok(foreign) => Some(format!(
            "manual cleanup needed in {name}: {} (its revert {} was left in place)",
            foreign.join(", "),
            short(&r.commit)
        )),
        Err(e) => checked(e),
    }
}

/// R20: a failed undo puts back every revert it made and changes nothing —
/// or says what a person must fix (and notes it on the card, which stays
/// applied). Answers the error `undo` returns.
fn give_up(
    id: i64,
    what: &str,
    error: IpcError,
    left: Option<String>,
    done: &[Reverted<'_>],
    store: &Mutex<Store>,
) -> IpcError {
    let mut problems: Vec<String> = left.into_iter().collect();
    problems.extend(done.iter().rev().filter_map(put_back));
    let msg = if problems.is_empty() {
        format!(
            "undo card {id}: {what}: {}; nothing was changed",
            error.message
        )
    } else {
        for p in &problems {
            tracing::error!("undo card {id}: {p}");
        }
        let msg = format!(
            "undo card {id}: {what}: {}; {}",
            error.message,
            problems.join("; ")
        );
        let noted = lock(store).and_then(|s| {
            if s.get_changeset(id)?.is_some_and(|c| c.state == "applied") {
                s.set_changeset_state(id, "applied", Some(&msg))?;
            }
            Ok(())
        });
        if let Err(e) = noted {
            tracing::error!(
                "undo card {id}: could not note the failure on the card: {}",
                e.message
            );
        }
        msg
    };
    let mut out = IpcError::new(&error.code, msg);
    out.details = error.details;
    out
}

/// After the undo is recorded: reload, push when `catalog.auto_push` (SB4),
/// trim the card's open follow-up (PF11), propose an undone take_host's
/// Rollout (R14). None of these un-undoes the card: each failure is a
/// warning in its `error`.
fn after_undo(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    rows: &[CatalogRow],
    done: &[Reverted<'_>],
    store: &Mutex<Store>,
) {
    let mut warnings = Vec::new();
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
        for r in done {
            if let Err(e) = repo::push(Path::new(&r.row.repo_path)) {
                warnings.push(format!("push {}: {}", r.row.name, e.message));
            }
        }
    }
    let taken: Vec<&ChangesetItemRow> = items
        .iter()
        .filter(|i| i.state == "applied" && i.action == ItemAction::TakeHost.as_str())
        .collect();
    let written = lock(store).and_then(|s| {
        if let Err(e) = trim_follow_up(card.id, items, &s) {
            warnings.push(format!("withdraw its follow-up rollout: {}", e.message));
        }
        if let Err(e) = propose_follow_up(&taken, &s) {
            warnings.push(format!(
                "propose the rollout of the restored copy: {}",
                e.message
            ));
        }
        if !warnings.is_empty() {
            s.set_changeset_state(card.id, "undone", Some(&warnings.join("; ")))?;
        }
        Ok(())
    });
    if let Err(e) = written {
        tracing::warn!(
            "card {}: undone, but its follow-up could not be recorded: {}",
            card.id,
            e.message
        );
    }
}

/// PF11: take the assets this undo removed from its catalogs — the members
/// its imports added — out of every open Rollout card that has applied
/// nothing yet; a card left with no item is withdrawn. Gap hashes are
/// recomputed (PF10); an item a person rejected stays rejected while its
/// assets are unchanged (PF14). A take_host's follow-up (group `update`) is
/// not trimmed: it carries the catalog's copy of that asset, which after
/// the undo is the restored one — exactly the Rollout R14 asks undo to
/// propose, so `propose_follow_up` refreshes it instead of adding a second.
fn trim_follow_up(card_id: i64, items: &[ChangesetItemRow], s: &Store) -> Result<(), IpcError> {
    let mut gone: BTreeMap<(i64, String), BTreeSet<String>> = BTreeMap::new();
    for i in items
        .iter()
        .filter(|i| i.state == "applied" && i.action == ItemAction::Import.as_str())
    {
        let p = ItemParams::parse(i.params.as_deref());
        if let (Some(cid), Some(layer), Some(member)) = (i.catalog_id, p.layer, p.member) {
            gone.entry((cid, layer)).or_default().insert(member);
        }
    }
    if gone.is_empty() {
        return Ok(());
    }
    for rollout in s.list_changesets()? {
        if rollout.kind != CardKind::Rollout.as_str() || !is_open(&rollout.state) {
            continue;
        }
        let existing = s.changeset_items(rollout.id)?;
        if existing.iter().any(|i| i.state == "applied") {
            continue;
        }
        let mut changed = false;
        let mut kept: Vec<(NewChangesetItem, bool)> = Vec::new();
        for i in &existing {
            let mut new = NewChangesetItem::from(i);
            let mut rejected = i.state == "rejected";
            let drop = i
                .catalog_id
                .filter(|_| i.action == ItemAction::Sync.as_str())
                .and_then(|cid| gone.get(&(cid, i.grp.clone())));
            if let Some(drop) = drop {
                let mut p = ItemParams::parse(i.params.as_deref());
                let before = p.assets.len();
                p.assets.retain(|a| !drop.contains(a));
                if p.assets.len() != before {
                    changed = true;
                    rejected = false;
                    if p.assets.is_empty() {
                        continue;
                    }
                    new.params = p.to_json();
                }
            }
            kept.push((new, rejected));
        }
        if !changed {
            continue;
        }
        if kept.is_empty() {
            s.withdraw_changeset(
                rollout.id,
                &format!("withdrawn: card #{card_id} was undone"),
            )?;
            continue;
        }
        let keep: Vec<i64> = kept
            .iter()
            .enumerate()
            .filter(|(_, (_, rejected))| *rejected)
            .map(|(n, _)| n as i64)
            .collect();
        let mut news: Vec<NewChangesetItem> = kept.into_iter().map(|(n, _)| n).collect();
        stamp_gap_hashes(&mut news);
        s.replace_changeset_items_keeping(rollout.id, &rollout_summary(&news), &news, |_| keep)?;
    }
    Ok(())
}

/// A person's no to the whole card: every pending item rejected (R9).
pub async fn dismiss(id: i64, store: &Mutex<Store>) -> Result<ChangesetView, IpcError> {
    let _busy = APPLY_LOCK.lock().await;
    let (card, items) = super::card(id, store)?;
    refuse_unless_open(&card)?;
    let pending: Vec<i64> = items
        .iter()
        .filter(|i| i.state == "pending")
        .map(|i| i.position)
        .collect();
    reject(&card, &items, &pending, store)?;
    super::get(id, store)
}

/// A person's no to some items; the card is dismissed once nothing is
/// pending (R9).
pub async fn reject_items(
    id: i64,
    positions: &[i64],
    store: &Mutex<Store>,
) -> Result<ChangesetView, IpcError> {
    let _busy = APPLY_LOCK.lock().await;
    let (card, items) = super::card(id, store)?;
    refuse_unless_open(&card)?;
    if positions.is_empty() {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "no positions named for card {id}: name the items to reject, or dismiss the \
                 whole card"
            ),
        ));
    }
    let mut chosen: Vec<i64> = Vec::new();
    for p in positions {
        match items.iter().find(|i| i.position == *p) {
            None => {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("card {id} has no item {p}"),
                ))
            }
            Some(i) if i.state != "pending" => {
                return Err(IpcError::new(
                    codes::E_INVALID_STATE,
                    format!("item {p} of card {id} is {}", i.state),
                ))
            }
            Some(_) if chosen.contains(p) => {}
            Some(_) => chosen.push(*p),
        }
    }
    reject(&card, &items, &chosen, store)?;
    super::get(id, store)
}

fn refuse_unless_open(card: &ChangesetRow) -> Result<(), IpcError> {
    if is_open(&card.state) {
        return Ok(());
    }
    Err(IpcError::new(
        codes::E_INVALID_STATE,
        format!(
            "card {} is {}; only a proposed or failed card can be dismissed or have items \
             rejected",
            card.id, card.state
        ),
    ))
}

/// Reject `positions` of an open card, with their verdicts, in one
/// transaction; dismiss the card when nothing is left pending.
fn reject(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    positions: &[i64],
    store: &Mutex<Store>,
) -> Result<(), IpcError> {
    let closing = !items
        .iter()
        .any(|i| i.state == "pending" && !positions.contains(&i.position));
    let now = now_unix();
    let verdicts: Vec<TriageVerdictRow> = items
        .iter()
        .filter(|i| positions.contains(&i.position))
        .filter_map(|i| verdict_for(i, items, positions, now))
        .collect();
    if !lock(store)?.reject_changeset_items(card.id, positions, &verdicts, closing)? {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("card {} is no longer open; nothing was rejected", card.id),
        ));
    }
    Ok(())
}

/// R9: the verdict rejecting `item` leaves, if its action has a subject. A
/// drift item's only once every take_host/restore item of its card is
/// rejected (this call's `positions` included). `decided_at` is Unix
/// seconds, as every verdict's.
fn verdict_for(
    item: &ChangesetItemRow,
    items: &[ChangesetItemRow],
    positions: &[i64],
    now: i64,
) -> Option<TriageVerdictRow> {
    let drift = |a: &str| a == ItemAction::TakeHost.as_str() || a == ItemAction::Restore.as_str();
    let (kind, name) = match item.action.as_str() {
        a if a == ItemAction::Import.as_str() || a == ItemAction::Hide.as_str() => {
            (item.kind.clone(), item.name.clone())
        }
        a if drift(a) => {
            let all = items
                .iter()
                .filter(|o| drift(&o.action))
                .all(|o| o.state == "rejected" || positions.contains(&o.position));
            if !all {
                return None;
            }
            (item.kind.clone(), item.name.clone())
        }
        a if a == ItemAction::Sync.as_str() => (
            "layer".to_string(),
            format!("{}/{}", item.catalog_id?, item.grp),
        ),
        _ => return None,
    };
    Some(TriageVerdictRow {
        catalog_id: item.catalog_id,
        kind,
        name,
        content_hash: ItemParams::parse(item.params.as_deref())
            .hash
            .unwrap_or_else(|| "-".into()),
        verdict: "rejected".into(),
        decider: Decider::Person.as_str().into(),
        decided_at: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::changesets::testkit::*;
    use crate::service::catalog::lock_registry_for_test;
    #[cfg(unix)]
    use std::sync::Arc;

    /// The tool (Task 9) awaits these from a multi-threaded runtime: no
    /// store guard lives across an await.
    #[test]
    fn the_undo_dismiss_and_reject_futures_are_send() {
        fn is_send<T: Send>(_: &T) {}
        let check = |store: &Mutex<Store>| {
            is_send(&undo(1, store));
            is_send(&dismiss(1, store));
            is_send(&reject_items(1, &[], store));
        };
        let _ = check;
    }

    fn store_with_personal() -> (Mutex<Store>, i64) {
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config("/p", None).unwrap();
        let p = s.personal_catalog().unwrap().unwrap().id;
        (Mutex::new(s), p)
    }

    /// R9: dismiss rejects every pending item and holds its subjects by
    /// content hash — set_scope carries none; a person's verdict.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn dismiss_rejects_every_pending_item_and_holds_its_subjects() {
        let _g = lock_registry_for_test();
        let (store, p) = store_with_personal();
        let card = store
            .lock()
            .unwrap()
            .insert_changeset(
                "new",
                "New on oci: skill/w → core",
                &[
                    item(
                        "core",
                        Some(p),
                        "skill",
                        "w",
                        ItemAction::Import,
                        ItemParams {
                            from_host: Some("oci".into()),
                            hash: Some("h1".into()),
                            ..Default::default()
                        },
                    ),
                    item(
                        "core",
                        Some(p),
                        "skill",
                        "w",
                        ItemAction::SetScope,
                        ItemParams {
                            scope: Some("shared".into()),
                            ..Default::default()
                        },
                    ),
                ],
            )
            .unwrap();
        let v = dismiss(card.id, &store).await.unwrap();
        assert_eq!(v.state, "dismissed");
        assert!(v.items.iter().all(|i| i.state == "rejected"));
        let verdicts = store.lock().unwrap().triage_verdicts().unwrap();
        assert_eq!(verdicts.len(), 1, "set_scope carries no subject");
        assert_eq!(
            (
                verdicts[0].content_hash.as_str(),
                verdicts[0].verdict.as_str(),
                verdicts[0].decider.as_str()
            ),
            ("h1", "rejected", "person")
        );
        assert_eq!(
            dismiss(card.id, &store).await.unwrap_err().code,
            codes::E_INVALID_STATE
        );
    }

    /// R9: rejecting one drift item leaves the other to apply; the subject
    /// is held only once both are rejected.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_drift_item_rejected_alone_holds_nothing_until_both_are() {
        let _g = lock_registry_for_test();
        let (store, p) = store_with_personal();
        let drift = |action| {
            item(
                "drift",
                Some(p),
                "skill",
                "w",
                action,
                ItemParams {
                    host: Some("trn".into()),
                    hash: Some("e".into()),
                    ..Default::default()
                },
            )
        };
        let card = store
            .lock()
            .unwrap()
            .insert_changeset(
                "drift",
                "skill/w differs on trn",
                &[drift(ItemAction::TakeHost), drift(ItemAction::Restore)],
            )
            .unwrap();
        let v = reject_items(card.id, &[0], &store).await.unwrap();
        assert_eq!(
            (v.state.as_str(), v.items[0].state.as_str()),
            ("proposed", "rejected")
        );
        assert!(store.lock().unwrap().triage_verdicts().unwrap().is_empty());
        let v = reject_items(card.id, &[1], &store).await.unwrap();
        assert_eq!(v.state, "dismissed");
        let verdicts = store.lock().unwrap().triage_verdicts().unwrap();
        assert_eq!(
            (verdicts.len(), verdicts[0].content_hash.as_str()),
            (1, "e")
        );
    }

    /// reject_item names real, pending items; a rejected sync item holds its
    /// layer by the gap hash it carries (R9, PF10), as a person's verdict
    /// that a rule never replaces (R10).
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn reject_item_checks_its_positions_and_a_person_verdict_outlives_a_rule() {
        let _g = lock_registry_for_test();
        let (store, p) = store_with_personal();
        let sync = |host: &str| {
            item(
                "core",
                Some(p),
                "host",
                host,
                ItemAction::Sync,
                ItemParams {
                    layer: Some("core".into()),
                    assets: vec!["skill/w".into()],
                    hash: Some("g1".into()),
                    ..Default::default()
                },
            )
        };
        let card = store
            .lock()
            .unwrap()
            .insert_changeset(
                "rollout",
                "Roll out core to oci, trn",
                &[sync("oci"), sync("trn")],
            )
            .unwrap();
        assert_eq!(
            reject_items(card.id, &[], &store).await.unwrap_err().code,
            codes::E_INVALID
        );
        assert_eq!(
            reject_items(card.id, &[7], &store).await.unwrap_err().code,
            codes::E_NOTFOUND
        );
        let v = reject_items(card.id, &[1, 1], &store).await.unwrap();
        assert_eq!(v.state, "proposed");
        assert_eq!(
            reject_items(card.id, &[1], &store).await.unwrap_err().code,
            codes::E_INVALID_STATE
        );
        let held = |s: &Store| {
            s.triage_verdicts()
                .unwrap()
                .into_iter()
                .map(|v| (v.kind, v.name, v.content_hash, v.decider))
                .collect::<Vec<_>>()
        };
        let want = vec![(
            "layer".to_string(),
            format!("{p}/core"),
            "g1".to_string(),
            "person".to_string(),
        )];
        assert_eq!(held(&store.lock().unwrap()), want);
        store
            .lock()
            .unwrap()
            .upsert_triage_verdict(&TriageVerdictRow {
                catalog_id: Some(p),
                kind: "layer".into(),
                name: format!("{p}/core"),
                content_hash: "g1".into(),
                verdict: "ignored".into(),
                decider: Decider::Rule.as_str().into(),
                decided_at: 1,
            })
            .unwrap();
        assert_eq!(
            held(&store.lock().unwrap()),
            want,
            "a rule never replaces a person"
        );
    }

    /// PF12: an applied card that only hid assets committed nothing; undo
    /// says there is nothing to undo and changes nothing.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_hide_only_card_has_nothing_to_undo() {
        let _g = lock_registry_for_test();
        let (store, _) = store_with_personal();
        let id = {
            let s = store.lock().unwrap();
            let card = s
                .insert_changeset(
                    "new",
                    "Hide hook/stop on oci",
                    &[item(
                        "hidden",
                        None,
                        "hook",
                        "stop",
                        ItemAction::Hide,
                        ItemParams {
                            hash: Some("h-stop".into()),
                            ..Default::default()
                        },
                    )],
                )
                .unwrap();
            s.set_changeset_item_states(card.id, &[0], "applied")
                .unwrap();
            s.mark_changeset_applied(card.id, 1_000, "{}", "[]", None)
                .unwrap();
            card.id
        };
        let err = undo(id, &store).await.unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(err.message.contains("nothing to undo"), "{}", err.message);
        assert_eq!(super::super::get(id, &store).unwrap().state, "applied");
    }

    #[cfg(unix)]
    const DESC: &str = "A reasonably long description here.";

    #[cfg(unix)]
    fn import(layer: &str, cid: i64, name: &str) -> crate::store::NewChangesetItem {
        item(
            layer,
            Some(cid),
            "skill",
            name,
            ItemAction::Import,
            ItemParams {
                from_host: Some("oci".into()),
                layer: Some(layer.into()),
                member: Some(format!("skill/{name}")),
                ..Default::default()
            },
        )
    }

    #[cfg(unix)]
    fn assign(layer: &str, cid: i64) -> crate::store::NewChangesetItem {
        item(
            layer,
            Some(cid),
            "layer",
            layer,
            ItemAction::AssignLayer,
            ItemParams {
                host: Some("oci".into()),
                layer: Some(layer.into()),
                axis: Some("context".into()),
                ..Default::default()
            },
        )
    }

    /// Insert a card of `items`, apply all of it, answer its id.
    #[cfg(unix)]
    async fn apply_card(
        f: &Fleet,
        ssh: &Arc<crate::ssh::SshClient>,
        kind: &str,
        summary: &str,
        items: &[crate::store::NewChangesetItem],
        positions: Option<Vec<i64>>,
    ) -> i64 {
        let card = f
            .store
            .lock()
            .unwrap()
            .insert_changeset(kind, summary, items)
            .unwrap();
        let args = super::super::apply::ApplyArgs {
            id: card.id,
            positions,
        };
        let v = super::super::apply::apply(args, &f.store, ssh)
            .await
            .unwrap();
        assert_eq!(v.state, "applied", "{:?}", v.error);
        card.id
    }

    /// Apply a card importing `name` from oci into personal's `core` and
    /// assigning `core` to oci; answer its id.
    #[cfg(unix)]
    async fn applied_card(f: &Fleet, ssh: &Arc<crate::ssh::SshClient>, name: &str) -> i64 {
        let p = f.personal.id;
        apply_card(
            f,
            ssh,
            "new",
            &format!("Adopt {name}"),
            &[import("core", p, name), assign("core", p)],
            None,
        )
        .await
    }

    #[cfg(unix)]
    fn contexts_in(f: &Fleet, cid: i64) -> Vec<String> {
        f.store
            .lock()
            .unwrap()
            .get_host_layers_for("oci", cid)
            .unwrap()
            .into_iter()
            .map(|r| r.layer_name)
            .collect()
    }

    #[cfg(unix)]
    fn contexts(f: &Fleet) -> Vec<String> {
        contexts_in(f, f.personal.id)
    }

    /// Every rollout card: (state, each item's assets).
    #[cfg(unix)]
    fn rollouts(f: &Fleet) -> Vec<(String, Vec<Vec<String>>)> {
        let s = f.store.lock().unwrap();
        s.list_changesets()
            .unwrap()
            .into_iter()
            .filter(|c| c.kind == "rollout")
            .map(|c| {
                let assets = s
                    .changeset_items(c.id)
                    .unwrap()
                    .iter()
                    .map(|i| ItemParams::parse(i.params.as_deref()).assets)
                    .collect();
                (c.state, assets)
            })
            .collect()
    }

    #[cfg(unix)]
    fn clean(root: &std::path::Path) -> bool {
        git(root, &["status", "--porcelain", "--untracked-files=all"]).is_empty()
    }

    /// Spec, Testing: "undo reverts them and restores `host_layers`"; the
    /// card's still-open follow-up Rollout is withdrawn (PF11).
    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn undo_reverts_the_commit_and_restores_host_layers() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        host_skill(home.path(), "w", DESC);
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = applied_card(&f, &ssh, "w").await;
        assert!(super::super::get(id, &f.store).unwrap().undoable);
        assert_eq!(contexts(&f), ["core"]);
        assert_eq!(
            rollouts(&f),
            [("proposed".to_string(), vec![vec!["skill/w".to_string()]])]
        );

        let v = undo(id, &f.store).await.unwrap();
        assert_eq!(v.state, "undone", "{:?}", v.error);
        assert_eq!(v.error, None);
        assert!(!v.undoable);
        assert!(subjects(&f.personal_root)[0].starts_with("Revert \"fleet: Adopt w\""));
        assert!(!f.personal_root.join("skills/w").exists());
        assert!(!f.personal_root.join("layers/core.yaml").exists());
        assert!(clean(&f.personal_root));
        assert!(contexts(&f).is_empty(), "host_layers back to the snapshot");
        assert!(
            crate::service::catalog::registry::with_catalog_row(&f.personal, |c| {
                Ok(c.find(crate::service::catalog::model::Kind::Skill, "w")
                    .is_none())
            })
            .unwrap()
        );
        assert_eq!(rollouts(&f)[0].0, "dismissed", "its follow-up is withdrawn");
        assert_eq!(
            undo(id, &f.store).await.unwrap_err().code,
            codes::E_INVALID_STATE
        );
    }

    /// SB5: only the latest applied card per catalog; undoing it takes its
    /// assets out of the open follow-up Rollout and keeps the rest.
    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn only_the_latest_applied_card_per_catalog_can_be_undone() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        host_skill(home.path(), "w", DESC);
        host_skill(home.path(), "v", DESC);
        let ssh = ssh_with_home(bin.path(), home.path());
        let a = applied_card(&f, &ssh, "w").await;
        let b = applied_card(&f, &ssh, "v").await;
        assert!(!super::super::get(a, &f.store).unwrap().undoable);
        let err = undo(a, &f.store).await.unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(
            err.message.contains(&format!("undo #{b} first")),
            "{}",
            err.message
        );
        undo(b, &f.store).await.unwrap();
        assert_eq!(
            contexts(&f),
            ["core"],
            "b's snapshot: core was already there"
        );
        assert!(f.personal_root.join("skills/w").is_dir());
        assert!(!f.personal_root.join("skills/v").exists());
        assert_eq!(
            rollouts(&f),
            [("proposed".to_string(), vec![vec!["skill/w".to_string()]])],
            "b's asset leaves the open rollout; a's stays"
        );
        undo(a, &f.store).await.unwrap();
        assert!(contexts(&f).is_empty());
        assert_eq!(rollouts(&f)[0].0, "dismissed");
    }

    /// R20: a revert that conflicts with a later hand-made commit changes
    /// nothing — HEAD, tree, host_layers and the card stay.
    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_revert_conflict_changes_nothing() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        host_skill(home.path(), "w", DESC);
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = applied_card(&f, &ssh, "w").await;
        std::fs::write(
            f.personal_root.join("skills/w/body.md"),
            "Edited by hand.\n",
        )
        .unwrap();
        git(&f.personal_root, &["commit", "-qam", "hand edit"]);
        let before = head(&f.personal_root);

        let err = undo(id, &f.store).await.unwrap_err();
        assert!(
            err.message.contains("nothing was changed"),
            "{}",
            err.message
        );
        assert_eq!(head(&f.personal_root), before);
        assert!(clean(&f.personal_root));
        assert!(
            !f.personal_root.join(".git/REVERT_HEAD").exists(),
            "the revert was aborted"
        );
        assert_eq!(
            std::fs::read_to_string(f.personal_root.join("skills/w/body.md")).unwrap(),
            "Edited by hand.\n"
        );
        assert_eq!(super::super::get(id, &f.store).unwrap().state, "applied");
        assert_eq!(contexts(&f), ["core"]);
    }

    /// PF7 / R20: a dirty checkout refuses the undo before anything is
    /// reverted; the person's file, HEAD, host_layers and the card stay.
    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn undo_on_a_dirty_checkout_refuses_and_changes_nothing() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        host_skill(home.path(), "w", DESC);
        let ssh = ssh_with_home(bin.path(), home.path());
        let id = applied_card(&f, &ssh, "w").await;
        git(
            &f.personal_root,
            &["config", "status.showUntrackedFiles", "no"],
        );
        std::fs::create_dir_all(f.personal_root.join("notes")).unwrap();
        std::fs::write(f.personal_root.join("notes/mine.txt"), "x\n").unwrap();
        let before = head(&f.personal_root);

        let err = undo(id, &f.store).await.unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(err.message.contains("uncommitted"), "{}", err.message);
        assert!(err.message.contains("personal"), "{}", err.message);
        assert_eq!(head(&f.personal_root), before);
        assert!(f.personal_root.join("notes/mine.txt").is_file());
        assert!(f.personal_root.join("skills/w").is_dir());
        assert_eq!(contexts(&f), ["core"]);
        let v = super::super::get(id, &f.store).unwrap();
        assert_eq!((v.state.as_str(), v.undoable), ("applied", true));
    }

    /// Insert and apply a card adopting `w` into personal's `core` and `v`
    /// into acme's `ops`, both layers assigned to oci.
    #[cfg(unix)]
    async fn two_catalog_card(
        f: &mut Fleet,
        ssh: &Arc<crate::ssh::SshClient>,
    ) -> (i64, crate::store::CatalogRow, std::path::PathBuf) {
        let (acme, acme_root) = f.add_org_catalog("acme");
        let p = f.personal.id;
        let id = apply_card(
            f,
            ssh,
            "bootstrap",
            "Adopt 2 as 2 layers",
            &[
                import("core", p, "w"),
                import("ops", acme.id, "v"),
                assign("core", p),
                assign("ops", acme.id),
            ],
            None,
        )
        .await;
        (id, acme, acme_root)
    }

    /// Spec, Testing: a card touching two catalogs is undone in both — one
    /// revert each — and both catalogs' host_layers go back.
    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn undo_of_a_two_catalog_card_reverts_both_and_restores_host_layers() {
        let _g = lock_registry_for_test();
        let mut f = Fleet::new(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        host_skill(home.path(), "w", DESC);
        host_skill(home.path(), "v", DESC);
        let ssh = ssh_with_home(bin.path(), home.path());
        let (id, acme, acme_root) = two_catalog_card(&mut f, &ssh).await;
        assert_eq!(
            (contexts(&f), contexts_in(&f, acme.id)),
            (vec!["core".to_string()], vec!["ops".to_string()])
        );

        let v = undo(id, &f.store).await.unwrap();
        assert_eq!(v.state, "undone", "{:?}", v.error);
        for root in [&f.personal_root, &acme_root] {
            assert!(subjects(root)[0].starts_with("Revert \"fleet: Adopt 2 as 2 layers\""));
            assert!(clean(root));
        }
        assert!(!f.personal_root.join("skills/w").exists());
        assert!(!acme_root.join("skills/v").exists());
        assert!(contexts(&f).is_empty() && contexts_in(&f, acme.id).is_empty());
    }

    /// R20: a conflict in the second catalog puts the first one's revert
    /// back — exactly its own files — and nothing is changed anywhere.
    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_conflict_in_one_catalog_puts_the_others_revert_back() {
        let _g = lock_registry_for_test();
        let mut f = Fleet::new(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        host_skill(home.path(), "w", DESC);
        host_skill(home.path(), "v", DESC);
        let ssh = ssh_with_home(bin.path(), home.path());
        let (id, acme, acme_root) = two_catalog_card(&mut f, &ssh).await;
        assert!(f.personal.id < acme.id, "personal is reverted first");
        std::fs::write(acme_root.join("skills/v/body.md"), "Edited by hand.\n").unwrap();
        git(&acme_root, &["commit", "-qam", "hand edit"]);
        let (p0, a0) = (head(&f.personal_root), head(&acme_root));

        let err = undo(id, &f.store).await.unwrap_err();
        assert!(err.message.contains("catalog acme"), "{}", err.message);
        assert!(
            err.message.contains("nothing was changed"),
            "{}",
            err.message
        );
        assert_eq!((head(&f.personal_root), head(&acme_root)), (p0, a0));
        assert!(clean(&f.personal_root) && clean(&acme_root));
        assert!(f.personal_root.join("skills/w/asset.yaml").is_file());
        assert!(f.personal_root.join("layers/core.yaml").is_file());
        assert_eq!(super::super::get(id, &f.store).unwrap().state, "applied");
        assert_eq!(
            (contexts(&f), contexts_in(&f, acme.id)),
            (vec!["core".to_string()], vec!["ops".to_string()])
        );
    }

    /// R14: undoing a take_host puts the catalog's own copy back and
    /// proposes the Rollout that carries it to the other hosts that manage
    /// it — one open card, not a second beside the apply's follow-up.
    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn undoing_a_take_host_restores_the_copy_and_proposes_one_rollout() {
        let _g = lock_registry_for_test();
        let f = Fleet::new(&["oci", "trn"]);
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
        f.store
            .lock()
            .unwrap()
            .replace_host_inventory(
                "trn",
                "claude",
                &[crate::store::AssetInventoryRow {
                    host_alias: "trn".into(),
                    harness: "claude".into(),
                    kind: "skill".into(),
                    name: "w".into(),
                    state: "in_sync".into(),
                    catalog_hash: None,
                    host_hash: None,
                    scanned_at: 1,
                    managed: true,
                    secret_like: false,
                    fleet_owned: false,
                    catalog_id: Some(p),
                }],
            )
            .unwrap();
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        host_skill(home.path(), "w", "Edited on the host, long enough.");
        let ssh = ssh_with_home(bin.path(), home.path());
        let take = item(
            "drift",
            Some(p),
            "skill",
            "w",
            ItemAction::TakeHost,
            ItemParams {
                host: Some("oci".into()),
                hash: Some("e".into()),
                ..Default::default()
            },
        );
        let id = apply_card(
            &f,
            &ssh,
            "drift",
            "skill/w differs on oci",
            &[take],
            Some(vec![0]),
        )
        .await;
        assert_eq!(rollouts(&f).len(), 1, "the apply's follow-up");

        let v = undo(id, &f.store).await.unwrap();
        assert_eq!(v.state, "undone", "{:?}", v.error);
        let w = crate::service::catalog::repo::read_asset(
            &f.personal_root,
            crate::service::catalog::model::Kind::Skill,
            "w",
        )
        .unwrap();
        assert_eq!(w.body, "Old steps.\n");
        assert!(clean(&f.personal_root));
        assert_eq!(
            rollouts(&f),
            [("proposed".to_string(), vec![vec!["skill/w".to_string()]])]
        );
    }
}
