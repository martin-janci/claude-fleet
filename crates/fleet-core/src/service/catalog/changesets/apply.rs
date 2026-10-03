//! Assets M4: applying a card (spec, *Changesets* → Apply). A catalog card
//! (bootstrap, new, drift take_host) changes catalogs: its touched catalogs
//! are snapshotted, imported into, their layer files and scopes written,
//! `host_layers` updated, and one commit made per catalog — or, on any
//! failure, every checkout is reset and nothing is committed (R11, R12).
//!
//! Everything runs under [`APPLY_LOCK`], which every mutating authoring
//! action also takes (PF7), so no edit lands in a checkout mid-apply; and an
//! apply starts only from clean checkouts, so the failure reset can only
//! drop what this apply itself wrote. Nothing here writes to a host (R15).

use super::rules::{gap_hash, LayerGap, NEEDS_A_LOOK, UPDATE};
use super::{is_open, CardKind, ChangesetView, Decider, ItemAction, ItemParams, APPLY_LOCK};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::catalog::import::slugify;
use crate::service::catalog::layer::{split_key, Axis, Layer};
use crate::service::catalog::model::{Kind, Scope};
use crate::service::catalog::validate::check_layer_name;
use crate::service::catalog::{author, registry, repo, CatalogTarget, ImportArgs, E_CATALOG_PARSE};
use crate::service::settings;
use crate::ssh::SshClient;
use crate::store::{
    now_unix, now_unix_ms, CatalogRow, ChangesetItemRow, ChangesetRow, HostLayerRow,
    NewChangesetItem, Store, TriageVerdictRow,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

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
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<ChangesetView, IpcError> {
    let _busy = APPLY_LOCK.lock().await;
    let (card, items) = super::card(args.id, store)?;
    if !is_open(&card.state) {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "card {} is {}; only a proposed or failed card applies",
                card.id, card.state
            ),
        ));
    }
    let selected = select_items(&card, &items, args.positions.as_deref())?;
    if super::writes_hosts(&card, &items, args.positions.as_deref()) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "card {} writes hosts; this build applies catalog cards only",
                card.id
            ),
        ));
    }
    apply_catalog(&card, &items, &selected, store, ssh).await?;
    super::get(args.id, store)
}

/// R3, R8: the items this apply runs.
fn select_items<'a>(
    card: &ChangesetRow,
    items: &'a [ChangesetItemRow],
    positions: Option<&[i64]>,
) -> Result<Vec<&'a ChangesetItemRow>, IpcError> {
    let chosen: Vec<&ChangesetItemRow> = match positions {
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

/// A failed step and the card group it failed in (spec: "the error on the
/// failing group").
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

async fn apply_catalog(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    selected: &[&ChangesetItemRow],
    store: &Mutex<Store>,
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
    let outcome = match run_steps(card, selected, &rows, token.as_deref(), store, ssh).await {
        Ok(commits) => record(card, items, selected, &commits, &snapshot, store).map(|()| commits),
        Err(f) => Err(f),
    };
    match outcome {
        Ok(commits) => {
            after_commits(card, selected, &rows, &commits, store);
            Ok(())
        }
        Err(failure) => {
            // R12: every checkout back at its pre-apply HEAD (each was clean,
            // so this drops only what this apply wrote), host_layers back,
            // every item pending again, the failing group named.
            for row in &rows {
                if let Err(e) = repo::reset_hard(Path::new(&row.repo_path), &pre[&row.id]) {
                    tracing::error!(
                        catalog = %row.name,
                        "card {}: reset after a failed apply: {}",
                        card.id,
                        e.message
                    );
                }
            }
            let msg = format!("{}: {}", failure.group, failure.error.message);
            let pending: Vec<i64> = items
                .iter()
                .filter(|i| i.state == "pending")
                .map(|i| i.position)
                .collect();
            let s = lock(store)?;
            for cid in &touched {
                s.restore_host_layers(*cid, &snapshot)?;
            }
            s.set_changeset_item_states(card.id, &pending, "pending")?;
            s.set_changeset_state(card.id, "failed", Some(&msg))?;
            Err(IpcError::new(&failure.error.code, msg))
        }
    }
}

/// Steps 2–4. Every error is a [`Failure`]; the caller resets.
async fn run_steps(
    card: &ChangesetRow,
    selected: &[&ChangesetItemRow],
    rows: &[CatalogRow],
    token: Option<&str>,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<BTreeMap<i64, String>, Failure> {
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
    // R13 / R15: take_host replaces the catalog copy and keeps its scope.
    let mut kept_scope: Vec<(&CatalogRow, Kind, Scope, &ChangesetItemRow)> = Vec::new();
    for ((_, host), group) in &imports {
        let row = row_of(group[0])?;
        let root = Path::new(&row.repo_path);
        for &item in group
            .iter()
            .filter(|i| i.action == ItemAction::TakeHost.as_str())
        {
            let kind = kind_of(item).map_err(|e| fail(item, e))?;
            let old = repo::read_asset(root, kind, &item.name).map_err(|e| fail(item, e))?;
            kept_scope.push((row, kind, old.header.scope, item));
            repo::remove_asset(root, kind, &item.name).map_err(|e| fail(item, e))?;
        }
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
            CatalogTarget::Row(row),
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
    }
    for (row, kind, scope, item) in &kept_scope {
        set_scope(Path::new(&row.repo_path), *kind, &item.name, *scope)
            .map_err(|e| fail(item, e))?;
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
        add_layer_members(Path::new(&row.repo_path), layer, add).map_err(|e| fail_in(layer, e))?;
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
        set_scope(Path::new(&row.repo_path), kind, &name, scope).map_err(|e| fail(item, e))?;
    }
    for &item in &shares {
        let row = row_of(item)?;
        let kind = kind_of(item).map_err(|e| fail(item, e))?;
        set_scope(
            Path::new(&row.repo_path),
            kind,
            &slugify(&item.name),
            Scope::Shared,
        )
        .map_err(|e| fail(item, e))?;
    }

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
    }

    // 4. One commit per touched catalog (`fleet: <card summary>`).
    let mut commits = BTreeMap::new();
    for row in rows {
        let sha = commit_all(
            Path::new(&row.repo_path),
            &format!("fleet: {}", card.summary),
        )
        .map_err(|e| {
            fail_in(
                "commit",
                IpcError::new(&e.code, format!("catalog {}: {}", row.name, e.message)),
            )
        })?;
        if let Some(sha) = sha {
            commits.insert(row.id, sha);
        }
    }
    Ok(commits)
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

/// Stage everything and commit it; `None` when nothing changed. The tree was
/// clean when the apply began (R11), so "everything" is this apply's work.
fn commit_all(root: &Path, message: &str) -> Result<Option<String>, IpcError> {
    repo::stage_paths(root, &[])?;
    if repo::has_staged(root, &[])? {
        Ok(Some(repo::commit(root, message)?))
    } else {
        Ok(None)
    }
}

fn set_scope(root: &Path, kind: Kind, name: &str, scope: Scope) -> Result<(), IpcError> {
    let mut a = repo::read_asset(root, kind, name)?;
    if a.header.scope != scope {
        a.header.scope = scope;
        repo::write_asset(root, &a, true)?;
    }
    Ok(())
}

/// Add `add` to `layers/<name>.yaml`, creating a context layer when there is
/// none (R6). Never removes a member.
fn add_layer_members(root: &Path, name: &str, add: &BTreeSet<String>) -> Result<(), IpcError> {
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
    std::fs::create_dir_all(root.join("layers"))?;
    std::fs::write(&path, layer.to_yaml())?;
    Ok(())
}

/// Step 5's bookkeeping, after every commit landed: verdicts for hide items,
/// the items' states, the card applied (`applied_at` in ms, PF13). A
/// failure here is still a card failure — the commits are reset (R12).
fn record(
    card: &ChangesetRow,
    items: &[ChangesetItemRow],
    selected: &[&ChangesetItemRow],
    commits: &BTreeMap<i64, String>,
    snapshot: &[HostLayerRow],
    store: &Mutex<Store>,
) -> Result<(), Failure> {
    let bookkeeping = |e: IpcError| fail_in("record", e);
    let s = lock(store).map_err(bookkeeping)?;
    let now = now_unix();
    for &item in selected
        .iter()
        .filter(|i| i.action == ItemAction::Hide.as_str())
    {
        let p = ItemParams::parse(item.params.as_deref());
        s.upsert_triage_verdict(&TriageVerdictRow {
            catalog_id: None,
            kind: item.kind.clone(),
            name: item.name.clone(),
            content_hash: p.hash.unwrap_or_else(|| "-".into()),
            verdict: "ignored".into(),
            decider: item.decider.clone(),
            decided_at: now,
        })
        .map_err(|e| bookkeeping(e.into()))?;
    }
    let applied: Vec<i64> = selected.iter().map(|i| i.position).collect();
    let rest: Vec<i64> = items
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
    s.set_changeset_item_states(card.id, &applied, "applied")
        .map_err(|e| bookkeeping(e.into()))?;
    s.set_changeset_item_states(card.id, &rest, "skipped")
        .map_err(|e| bookkeeping(e.into()))?;
    s.mark_changeset_applied(card.id, now_unix_ms(), &commits_json, &snapshot_json, None)
        .map_err(|e| bookkeeping(e.into()))?;
    Ok(())
}

/// Step 5's follow-through on an applied card: reload, push when
/// `catalog.auto_push` (SB4, R18), propose the follow-up Rollout (R14).
/// None of these un-applies the card: each failure is a warning in its
/// `error` (R12).
fn after_commits(
    card: &ChangesetRow,
    selected: &[&ChangesetItemRow],
    rows: &[CatalogRow],
    commits: &BTreeMap<i64, String>,
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
        for row in rows.iter().filter(|r| commits.contains_key(&r.id)) {
            if let Err(e) = repo::push(Path::new(&row.repo_path)) {
                warnings.push(format!("push {}: {}", row.name, e.message));
            }
        }
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
fn stamp_gap_hashes(items: &mut [NewChangesetItem]) {
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
fn rollout_summary(items: &[NewChangesetItem]) -> String {
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::service::catalog::changesets::rules::NEEDS_A_LOOK;
    use crate::service::catalog::changesets::testkit::*;
    use crate::service::catalog::lock_registry_for_test;
    use crate::store::AssetInventoryRow;

    const DESC: &str = "A reasonably long description here.";

    /// The `changesets` tool (Task 9) awaits `apply` inside an MCP handler,
    /// which must be `Send`: no store guard lives across an await.
    #[test]
    fn the_apply_future_is_send() {
        fn is_send<T: Send>(_: &T) {}
        let check = |store: &Mutex<Store>, ssh: &Arc<SshClient>| {
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
}
