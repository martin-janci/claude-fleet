//! Assets M4: the reconcile pass — gather the facts, ask the rules, write
//! the cards (spec: "A reconcile pass runs after each scan-tick pass and on
//! demand"). Synchronous: store, then registry, never SSH. Store rows are
//! read under one guard that is dropped before the registry is read, and the
//! writes take a fresh guard (store → registry is never allowed). The scan
//! tick's hook ([`after_scan_pass`]) runs the pass, then hands SB6's host
//! sync (which does SSH) to a detached task.

use super::rules::{
    self, CatalogFacts, DriftFacts, HostFacts, LayerFacts, LayerGap, RulesInput, SubjectItem,
};
use super::{
    is_open, CardKind, Decider, ItemKey, ItemParams, ProposedCard, ProposedItem, APPLY_LOCK,
};
use crate::ipc_error::{lock, IpcError};
use crate::service::catalog::identity::{self, AssetIdentity, IdentityClass};
use crate::service::catalog::import::slugify;
use crate::service::catalog::repo::Catalog;
use crate::service::catalog::{effective, registry};
use crate::service::settings;
use crate::ssh::SshClient;
use crate::store::{
    now_unix, AssetInventoryRow, CatalogRow, ChangesetItemRow, ChangesetRow, HostLayerRow,
    NewChangesetItem, Store, TriageVerdictRow,
};
use futures_util::FutureExt;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

/// What one pass wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub inserted: usize,
    pub refreshed: usize,
    pub withdrawn: usize,
    pub hidden: usize,
}

/// The error a withdrawn card carries (R2).
pub const WITHDRAWN: &str = "withdrawn: no longer applies";

/// Assets M5 (R9), M6 (R3): how long a card the system withdrew, untouched,
/// is kept before the pass prunes it — a week after it was withdrawn.
pub const WITHDRAWN_RETENTION_SECS: i64 = 7 * 24 * 3600;

type OpenCard = (ChangesetRow, Vec<ChangesetItemRow>);

/// Everything one pass reads, owned. [`proposals`] puts it in one stable
/// order before the rules see it (Task 4 review M5).
struct PassFacts {
    /// Inventory rows of non-hidden hosts.
    rows: Vec<AssetInventoryRow>,
    hosts: Vec<HostFacts>,
    catalogs: Vec<CatalogFacts>,
    verdict_keys: BTreeSet<(String, String, String)>,
    open: Vec<OpenCard>,
    /// R4: an applied (not undone) Bootstrap card exists.
    bootstrap_applied: bool,
    gaps: Vec<LayerGap>,
}

/// One pass. `auto` (`catalog.auto`): write `ignored` verdicts for
/// internals here instead of proposing `hide` items (R18). Then prune the
/// system's old, untouched withdrawn cards (Assets M5, R9) — best effort:
/// a failed prune is logged and tried again next pass, never failing the
/// pass whose writes already landed.
pub fn reconcile(store: &Mutex<Store>, auto: bool) -> Result<ReconcileReport, IpcError> {
    let facts = gather(store)?;
    let (identities, proposed) = proposals(&facts, auto);
    let report = write(store, &facts, &identities, &proposed, auto)?;
    let pruned = lock(store)?.prune_withdrawn_changesets(now_unix() - WITHDRAWN_RETENTION_SECS);
    match pruned {
        Ok(0) => {}
        Ok(n) => tracing::debug!(pruned = n, "changesets: pruned withdrawn cards"),
        Err(e) => tracing::warn!(error = %e, "changesets: pruning withdrawn cards failed"),
    }
    Ok(report)
}

/// Store rows under one guard, then the registry (cloned out, no lock held
/// after), then each layered host's effective set (which takes the store
/// guard itself; none is held here).
fn gather(store: &Mutex<Store>) -> Result<PassFacts, IpcError> {
    let (rows, hosts, configured, host_layers, verdicts, open, bootstrap_applied, rolled_out) = {
        let s = lock(store)?;
        let all = s.list_changesets()?;
        let mut open: Vec<OpenCard> = Vec::new();
        for c in all.iter().filter(|c| is_open(&c.state)) {
            open.push((c.clone(), s.changeset_items(c.id)?));
        }
        (
            s.list_inventory()?,
            s.list_hosts()?,
            s.list_catalogs()?,
            s.list_all_host_layers()?,
            s.triage_verdicts()?,
            open,
            all.iter()
                .any(|c| c.kind == CardKind::Bootstrap.as_str() && c.state == "applied"),
            s.rolled_out_layers()?,
        )
    };
    let snapshot = registry::snapshot()?;
    let visible: BTreeSet<&str> = hosts
        .iter()
        .filter(|h| !h.hidden)
        .map(|h| h.alias.as_str())
        .collect();
    let rows: Vec<AssetInventoryRow> = rows
        .into_iter()
        .filter(|r| visible.contains(r.host_alias.as_str()))
        .collect();
    let host_facts: Vec<HostFacts> = hosts
        .iter()
        .filter(|h| !h.hidden)
        .map(|h| HostFacts {
            alias: h.alias.clone(),
            org_id: h.org_id,
        })
        .collect();
    let catalogs: Vec<CatalogFacts> = configured
        .iter()
        .map(|row| catalog_facts(row, &snapshot, &host_layers))
        .collect();
    let verdict_keys = verdicts
        .iter()
        .map(|v| (v.kind.clone(), v.name.clone(), v.content_hash.clone()))
        .collect();
    let gaps = layer_gaps(
        store,
        &snapshot,
        &configured,
        &host_layers,
        &rows,
        &rolled_out,
    );
    Ok(PassFacts {
        rows,
        hosts: host_facts,
        catalogs,
        verdict_keys,
        open,
        bootstrap_applied,
        gaps,
    })
}

/// Assets M5 (R5): the drifted managed Claude rows a Drift card is for. A
/// copy that is only behind its catalog (`drift_side = catalog`) is not one:
/// there is no pick to make, so a catalog-only change no longer opens a
/// card per host (an open one is withdrawn by the pass, its subject no
/// longer produced). What brings it up to date: SB6 on a layer already
/// rolled out, with `catalog.auto` on; a Rollout card, only for a layer
/// never rolled out to that host; otherwise — with `catalog.auto` off —
/// only a person's own Sync. The Inbox lists it under *Behind the catalog*
/// meanwhile. An edited
/// copy (`host`) says so in its card; one whose side is unknown (`None`: a
/// manifest entry from before M5) keeps M4's card.
fn drift_facts(rows: &[AssetInventoryRow]) -> Vec<DriftFacts> {
    rows.iter()
        .filter(|r| r.state == "drifted" && r.managed && r.harness == "claude")
        .filter(|r| r.drift_side.as_deref() != Some("catalog"))
        .filter_map(|r| {
            Some(DriftFacts {
                catalog_id: r.catalog_id?,
                kind: r.kind.clone(),
                name: r.name.clone(),
                host: r.host_alias.clone(),
                host_hash: r.host_hash.clone(),
                edited: r.drift_side.as_deref() == Some("host"),
            })
        })
        .collect()
}

/// The identities and the cards the facts call for. Pure. Every input the
/// rules read in sequence is sorted first — rows by (host, harness, kind,
/// name), drift by (catalog, kind, name, host), gaps by (catalog, layer,
/// host) with their assets sorted — so the same facts read in another
/// order make the same cards, item for item (M5).
fn proposals(f: &PassFacts, auto: bool) -> (Vec<AssetIdentity>, Vec<ProposedCard>) {
    let mut rows: Vec<&AssetInventoryRow> = f.rows.iter().collect();
    rows.sort_by(|a, b| {
        (&a.host_alias, &a.harness, &a.kind, &a.name).cmp(&(
            &b.host_alias,
            &b.harness,
            &b.kind,
            &b.name,
        ))
    });
    let rows: Vec<AssetInventoryRow> = rows.into_iter().cloned().collect();
    let identities = identity::group_identities(&rows);
    let mut hosts = f.hosts.clone();
    hosts.sort_by(|a, b| a.alias.cmp(&b.alias));
    let mut catalogs = f.catalogs.clone();
    catalogs.sort_by_key(|c| c.id);
    let mut drifted = drift_facts(&rows);
    drifted.sort_by(|a, b| {
        (a.catalog_id, &a.kind, &a.name, &a.host).cmp(&(b.catalog_id, &b.kind, &b.name, &b.host))
    });
    let mut gaps = f.gaps.clone();
    for g in &mut gaps {
        g.assets.sort();
        g.assets.dedup();
    }
    gaps.sort_by(|a, b| (a.catalog_id, &a.layer, &a.host).cmp(&(b.catalog_id, &b.layer, &b.host)));
    let rollout_open: BTreeSet<(i64, String)> = f
        .open
        .iter()
        .filter(|(c, _)| c.kind == CardKind::Rollout.as_str())
        .flat_map(|(_, items)| {
            items
                .iter()
                .filter_map(|i| Some((i.catalog_id?, i.grp.clone())))
        })
        .collect();
    let personal_assets = catalogs
        .iter()
        .find(|c| c.org_id.is_none())
        .map_or(0, |c| c.asset_count);
    let input = RulesInput {
        identities: &identities,
        hosts: &hosts,
        catalogs: &catalogs,
        verdicts: &f.verdict_keys,
        drifted: &drifted,
        gaps: &gaps,
        rollout_open: &rollout_open,
        bootstrapped: f.bootstrap_applied || personal_assets > 0,
        bootstrap_open: f
            .open
            .iter()
            .any(|(c, _)| c.kind == CardKind::Bootstrap.as_str()),
        auto,
    };
    let cards = rules::propose(&input);
    (identities, cards)
}

/// The writes, under one fresh guard: automatic hides (auto on), then each
/// proposed card inserted or refreshed in place, then the open cards no
/// proposal named withdrawn (never a rollout, never a person's layer card,
/// never one with applied items).
fn write(
    store: &Mutex<Store>,
    f: &PassFacts,
    identities: &[AssetIdentity],
    proposed: &[ProposedCard],
    auto: bool,
) -> Result<ReconcileReport, IpcError> {
    let s = lock(store)?;
    let mut report = ReconcileReport::default();
    if auto {
        let now = now_unix();
        for id in identities.iter().filter(|i| {
            matches!(
                i.class,
                IdentityClass::FleetInternal | IdentityClass::HarnessInternal
            )
        }) {
            let hash = rules::identity_hash(id);
            if f.verdict_keys
                .contains(&(id.kind.clone(), id.name.clone(), hash.clone()))
            {
                continue;
            }
            s.upsert_triage_verdict(&TriageVerdictRow {
                catalog_id: None,
                kind: id.kind.clone(),
                name: id.name.clone(),
                content_hash: hash,
                verdict: "ignored".into(),
                decider: Decider::Rule.as_str().into(),
                decided_at: now,
            })?;
            report.hidden += 1;
        }
    }
    // One open card per subject (R2). `open` is newest first, so a
    // duplicate — which the pass itself never creates — leaves the newest
    // card in charge and is logged; the older one is left as it is.
    let mut by_subject: BTreeMap<String, &OpenCard> = BTreeMap::new();
    // Assets M6 (R5): a person's layer card has no rule subject; only a
    // person dismisses it, so it is never refreshed or withdrawn here.
    for o in f
        .open
        .iter()
        .filter(|o| o.0.kind != CardKind::Layer.as_str())
    {
        let subject = subject_of_row(&o.0, &o.1);
        if let Some(kept) = by_subject.get(&subject) {
            tracing::warn!(
                subject = %subject,
                kept = kept.0.id,
                ignored = o.0.id,
                "changesets: two open cards share a subject; the pass refreshes only the newest"
            );
            continue;
        }
        by_subject.insert(subject, o);
    }
    let mut produced: BTreeSet<String> = BTreeSet::new();
    for card in proposed {
        let subject = card.subject();
        let items: Vec<NewChangesetItem> = card.items.iter().map(ProposedItem::to_new).collect();
        match by_subject.get(&subject) {
            Some(o) => {
                if refresh(&s, o, card, &items)? {
                    report.refreshed += 1;
                }
            }
            None => {
                s.insert_changeset(card.kind.as_str(), &card.summary, &items)?;
                report.inserted += 1;
            }
        }
        produced.insert(subject);
    }
    for (subject, o) in &by_subject {
        let (row, items) = (&o.0, &o.1);
        if row.kind == CardKind::Rollout.as_str()
            || produced.contains(subject)
            || items.iter().any(|i| i.state == "applied")
        {
            continue;
        }
        if s.withdraw_changeset(row.id, WITHDRAWN)? {
            report.withdrawn += 1;
        }
    }
    Ok(report)
}

/// Refresh an open card in place when its proposal changed (R2). PF14: the
/// items a person rejected stay rejected — matched by [`ItemKey`] (catalog,
/// kind, name, action, host) against the card's items as they are inside
/// the store's transaction — and every other item starts pending. The
/// replace and the re-reject are one transaction
/// (`replace_changeset_items_keeping`): a failure leaves the old items.
/// `true` when the card was rewritten.
fn refresh(
    s: &Store,
    open: &OpenCard,
    card: &ProposedCard,
    items: &[NewChangesetItem],
) -> Result<bool, IpcError> {
    let (row, existing) = (&open.0, &open.1);
    let same = row.summary == card.summary
        && existing
            .iter()
            .map(NewChangesetItem::from)
            .eq(items.iter().cloned());
    if same {
        return Ok(false);
    }
    let keep_rejected = |old: &[ChangesetItemRow]| -> Vec<i64> {
        let rejected: BTreeSet<ItemKey> = old
            .iter()
            .filter(|i| i.state == "rejected")
            .map(ItemKey::of_row)
            .collect();
        card.items
            .iter()
            .enumerate()
            .filter(|(_, i)| rejected.contains(&i.key()))
            .map(|(p, _)| p as i64)
            .collect()
    };
    Ok(s.replace_changeset_items_keeping(row.id, &card.summary, items, keep_rejected)?)
}

fn catalog_facts(
    row: &CatalogRow,
    snapshot: &BTreeMap<i64, Catalog>,
    host_layers: &[HostLayerRow],
) -> CatalogFacts {
    let entry = registry::entry_for(snapshot, row);
    CatalogFacts {
        id: row.id,
        name: row.name.clone(),
        org_id: row.org_id,
        loaded: entry.is_some_and(|c| c.load_error.is_none()),
        asset_count: entry.map_or(0, |c| c.assets.len()),
        slugs: entry
            .map(|c| {
                c.assets
                    .iter()
                    .map(|a| (a.kind().as_str().to_string(), slugify(&a.header.name)))
                    .collect()
            })
            .unwrap_or_default(),
        layers: entry
            .map(|c| {
                c.layers
                    .iter()
                    .map(|l| LayerFacts {
                        name: l.name.clone(),
                        hosts: host_layers
                            .iter()
                            .filter(|r| {
                                r.active && r.catalog_id == row.id && r.layer_name == l.name
                            })
                            .map(|r| r.host_alias.clone())
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// R16: per (catalog, layer, host), the members a never-rolled-out layer
/// introduced on a host that the scan finds `missing` there. Composes each
/// layered host's effective set against `snapshot`
/// (`effective_for_host_in` takes the store guard itself; none is held here).
fn layer_gaps(
    store: &Mutex<Store>,
    snapshot: &BTreeMap<i64, Catalog>,
    configured: &[CatalogRow],
    host_layers: &[HostLayerRow],
    rows: &[AssetInventoryRow],
    rolled_out: &BTreeSet<(i64, String)>,
) -> Vec<LayerGap> {
    let id_of = super::catalog_ids_by_label(configured);
    let missing: BTreeSet<(&str, &str, &str)> = rows
        .iter()
        .filter(|r| r.harness == "claude" && r.state == "missing")
        .map(|r| (r.host_alias.as_str(), r.kind.as_str(), r.name.as_str()))
        .collect();
    let visible: BTreeSet<&str> = rows.iter().map(|r| r.host_alias.as_str()).collect();
    let layered: BTreeSet<&str> = host_layers
        .iter()
        .filter(|r| r.active)
        .map(|r| r.host_alias.as_str())
        .filter(|h| visible.contains(h))
        .collect();
    let mut out: BTreeMap<(i64, String, String), Vec<String>> = BTreeMap::new();
    for host in layered {
        let Ok(eff) = effective::effective_for_host_in(store, host, snapshot) else {
            continue;
        };
        for (key, prov) in &eff.provenance {
            let Some(cid) = id_of.get(&prov.catalog) else {
                continue;
            };
            if rolled_out.contains(&(*cid, prov.introduced_by.clone())) {
                continue;
            }
            let Some((kind, name)) = key.split_once('/') else {
                continue;
            };
            if missing.contains(&(host, kind, name)) {
                out.entry((*cid, prov.introduced_by.clone(), host.to_string()))
                    .or_default()
                    .push(key.clone());
            }
        }
    }
    out.into_iter()
        .map(|((catalog_id, layer, host), assets)| LayerGap {
            catalog_id,
            layer,
            host,
            assets,
        })
        .collect()
}

/// A stored card's subject (R2), the same function the proposals use.
fn subject_of_row(card: &ChangesetRow, items: &[ChangesetItemRow]) -> String {
    let kind = CardKind::parse(&card.kind).unwrap_or(CardKind::Rollout);
    let params: Vec<ItemParams> = items
        .iter()
        .map(|i| ItemParams::parse(i.params.as_deref()))
        .collect();
    rules::subject_of(
        kind,
        items.iter().zip(&params).map(|(i, p)| SubjectItem {
            grp: &i.grp,
            catalog_id: i.catalog_id,
            kind: &i.kind,
            name: &i.name,
            host: p.host.as_deref(),
        }),
    )
}

/// Whether [`after_scan_pass`] has already logged an unreadable store.
static STORE_UNREADABLE_LOGGED: AtomicBool = AtomicBool::new(false);

/// The scan tick's hook (R19, carry 4): with `catalog.auto` on, one pass —
/// skipped, never awaited, while an apply holds `APPLY_LOCK` — then SB6's
/// additive sync, detached ([`spawn_logged`]) so the tick never waits for
/// SSH, and skipped likewise when the lock is taken by then (R17). Errors
/// and a panic in SB6 are logged; the tick's `owed`/`seen` are never
/// touched.
pub fn after_scan_pass(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) {
    let auto = match lock(store) {
        Ok(s) => settings::get_bool(&s, settings::CATALOG_AUTO),
        Err(e) => {
            // A poisoned store would otherwise read as "auto off" on every
            // later tick, silently: say so — loudly once per process, then
            // at debug so a long-lived process does not flood the log.
            if !STORE_UNREADABLE_LOGGED.swap(true, Ordering::Relaxed) {
                tracing::error!(
                    "changesets: the store is unreadable ({}); automatic reconcile passes are skipped until fleet restarts",
                    e.message
                );
            } else {
                tracing::debug!("changesets: store unreadable; reconcile skipped");
            }
            return;
        }
    };
    if !auto {
        return;
    }
    {
        let Ok(_busy) = APPLY_LOCK.try_lock() else {
            tracing::debug!("changesets: an apply is running; this pass's reconcile is skipped");
            return;
        };
        match reconcile(store, true) {
            Ok(r) if r != ReconcileReport::default() => tracing::info!(
                inserted = r.inserted,
                refreshed = r.refreshed,
                withdrawn = r.withdrawn,
                hidden = r.hidden,
                "changesets: reconciled"
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!("changesets: reconcile failed: {}", e.message),
        }
    }
    let (store, ssh) = (Arc::clone(store), Arc::clone(ssh));
    spawn_logged("catalog.auto: the additive sync", async move {
        let Ok(_busy) = APPLY_LOCK.try_lock() else {
            tracing::debug!(
                "catalog.auto: an apply is running; this pass's additive sync is skipped"
            );
            return;
        };
        match super::apply::auto_additive(&store, &ssh).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(hosts = n, "catalog.auto: additive sync applied"),
            Err(e) => tracing::warn!("catalog.auto: additive sync failed: {}", e.message),
        }
    });
}

/// R17: run `fut` as a detached task (`rt::try_spawn`; `None`, the future
/// dropped and a warning logged, with no runtime reachable). A panic in it is caught and logged
/// as `what` — it never reaches the caller, the runtime or the next tick.
pub(crate) fn spawn_logged<F>(what: &'static str, fut: F) -> Option<JoinHandle<()>>
where
    F: Future<Output = ()> + Send + 'static,
{
    let handle = crate::rt::try_spawn(async move {
        if AssertUnwindSafe(fut).catch_unwind().await.is_err() {
            tracing::error!("{what} panicked");
        }
    });
    if handle.is_none() {
        tracing::warn!("{what}: no runtime to run it on; skipped this pass");
    }
    handle
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::layer::{Axis, LayerSet};
    use crate::service::catalog::model::{Asset, Kind};
    use crate::service::catalog::repo::Catalog;
    use crate::service::catalog::{author, lock_registry_for_test, registry};

    fn skill(name: &str) -> Asset {
        Asset::from_yaml(
            Some(Kind::Skill),
            &format!("kind: skill\nname: {name}\ndescription: d\n"),
        )
        .unwrap()
    }

    /// `personal` configured and installed holding `assets`; hosts `oci`
    /// and `trn` (org 7). Returns `(store, personal_id)`.
    fn fleet_store(assets: Vec<Asset>) -> (Arc<Mutex<Store>>, i64) {
        let s = Store::open_in_memory().unwrap();
        s.set_catalog_config("/p", None).unwrap();
        for h in ["oci", "trn"] {
            s.upsert_host(h).unwrap();
        }
        s.conn_ref()
            .execute(
                "INSERT INTO orgs (id, name, created_at) VALUES (7, 'papayapos', 0)",
                [],
            )
            .unwrap();
        s.set_host_org("trn", Some(7)).unwrap();
        let p = s.personal_catalog().unwrap().unwrap().id;
        registry::install_personal(Catalog {
            id: p,
            name: "personal".into(),
            assets,
            ..Default::default()
        })
        .unwrap();
        (Arc::new(Mutex::new(s)), p)
    }

    fn unmanaged(host: &str, name: &str) -> AssetInventoryRow {
        AssetInventoryRow {
            host_alias: host.into(),
            harness: "claude".into(),
            kind: "skill".into(),
            name: name.into(),
            state: "unmanaged".into(),
            host_hash: Some(format!("h-{name}")),
            scanned_at: 1,
            ..Default::default()
        }
    }

    fn fleet_hook(host: &str) -> AssetInventoryRow {
        let mut r = unmanaged(host, "stop");
        r.kind = "hook".into();
        r.fleet_owned = true;
        r
    }

    fn put(store: &Mutex<Store>, host: &str, rows: Vec<AssetInventoryRow>) {
        store
            .lock()
            .unwrap()
            .replace_host_inventory(host, "claude", &rows)
            .unwrap();
    }

    #[test]
    fn the_pass_proposes_a_bootstrap_hides_internals_and_refreshes_in_place() {
        let _g = lock_registry_for_test();
        let (store, _) = fleet_store(vec![]);
        let mut rows: Vec<_> = (0..25)
            .map(|n| unmanaged("oci", &format!("s{n}")))
            .collect();
        rows.push(fleet_hook("oci"));
        put(&store, "oci", rows.clone());

        let r = reconcile(&store, true).unwrap();
        assert_eq!((r.inserted, r.hidden), (1, 1));
        let cards = store.lock().unwrap().list_changesets().unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].kind, "bootstrap");
        let verdicts = store.lock().unwrap().triage_verdicts().unwrap();
        assert_eq!(
            (
                verdicts[0].name.as_str(),
                verdicts[0].verdict.as_str(),
                verdicts[0].decider.as_str()
            ),
            ("stop", "ignored", "rule")
        );

        let again = reconcile(&store, true).unwrap();
        assert_eq!(
            again,
            ReconcileReport::default(),
            "nothing changed, nothing written"
        );

        rows.push(unmanaged("oci", "s99"));
        put(&store, "oci", rows);
        let more = reconcile(&store, true).unwrap();
        assert_eq!((more.inserted, more.refreshed), (0, 1));
        let after = store.lock().unwrap().list_changesets().unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(
            after[0].id, cards[0].id,
            "the same card, refreshed in place"
        );
    }

    /// PF14: a refresh keeps what a person rejected — matched by `ItemKey`
    /// (host included) — while an item the refresh adds starts pending.
    #[test]
    fn a_rejected_item_stays_rejected_across_a_refresh_and_a_new_item_starts_pending() {
        let _g = lock_registry_for_test();
        let (store, _) = fleet_store(vec![]);
        let mut rows: Vec<_> = (0..25)
            .map(|n| unmanaged("oci", &format!("s{n}")))
            .collect();
        put(&store, "oci", rows.clone());
        reconcile(&store, true).unwrap();
        let card = store.lock().unwrap().list_changesets().unwrap()[0].clone();
        let assign = |s: &Store| {
            s.changeset_items(card.id)
                .unwrap()
                .into_iter()
                .find(|i| i.action == "assign_layer")
                .expect("an assign_layer item for oci")
        };
        let rejected = assign(&store.lock().unwrap());
        store
            .lock()
            .unwrap()
            .set_changeset_item_states(card.id, &[rejected.position], "rejected")
            .unwrap();

        rows.push(unmanaged("oci", "s99"));
        put(&store, "oci", rows);
        let r = reconcile(&store, true).unwrap();
        assert_eq!(r.refreshed, 1);
        let s = store.lock().unwrap();
        let kept = assign(&s);
        assert_eq!(ItemKey::of_row(&kept), ItemKey::of_row(&rejected));
        assert_eq!(kept.state, "rejected", "a person's rejection survives");
        let items = s.changeset_items(card.id).unwrap();
        let new = items
            .iter()
            .find(|i| i.action == "import" && i.name == "s99")
            .expect("the new identity's import");
        assert_eq!(new.state, "pending");
        assert_eq!(
            items.iter().filter(|i| i.state == "rejected").count(),
            1,
            "only the rejected key stays rejected"
        );
    }

    /// R2: an open card whose subject the rules no longer produce is
    /// withdrawn; a rollout card never is.
    #[test]
    fn a_card_whose_subject_disappears_is_withdrawn_but_a_rollout_stays() {
        let _g = lock_registry_for_test();
        let (store, p) = fleet_store(vec![skill("kept")]);
        put(&store, "oci", vec![unmanaged("oci", "w")]);
        reconcile(&store, true).unwrap();
        let card = store.lock().unwrap().list_changesets().unwrap()[0].clone();
        assert_eq!(card.kind, "new", "personal is not empty: bootstrapped");
        let rollout = store
            .lock()
            .unwrap()
            .insert_changeset(
                "rollout",
                "Roll out core to oci",
                &[NewChangesetItem {
                    grp: "core".into(),
                    catalog_id: Some(p),
                    kind: "host".into(),
                    name: "oci".into(),
                    action: "sync".into(),
                    params: Some(r#"{"layer":"core","assets":["skill/kept"]}"#.into()),
                    decider: "rule".into(),
                }],
            )
            .unwrap();
        put(&store, "oci", vec![]);
        let r = reconcile(&store, true).unwrap();
        assert_eq!(r.withdrawn, 1);
        let s = store.lock().unwrap();
        let gone = s.get_changeset(card.id).unwrap().unwrap();
        assert_eq!(
            (gone.state.as_str(), gone.error.as_deref()),
            ("dismissed", Some(WITHDRAWN))
        );
        assert_eq!(
            s.get_changeset(rollout.id).unwrap().unwrap().state,
            "proposed"
        );
    }

    /// R2: a card with an applied item is history in part — the pass never
    /// withdraws it, even once its subject is gone.
    #[test]
    fn withdrawal_skips_a_card_with_an_applied_item() {
        let _g = lock_registry_for_test();
        let (store, _) = fleet_store(vec![skill("kept")]);
        put(&store, "oci", vec![unmanaged("oci", "w")]);
        reconcile(&store, true).unwrap();
        let card = store.lock().unwrap().list_changesets().unwrap()[0].clone();
        {
            let s = store.lock().unwrap();
            s.set_changeset_item_states(card.id, &[0], "applied")
                .unwrap();
            s.set_changeset_state(card.id, "failed", Some("trn: unreachable"))
                .unwrap();
        }
        put(&store, "oci", vec![]);
        let r = reconcile(&store, true).unwrap();
        assert_eq!(r.withdrawn, 0);
        let row = store
            .lock()
            .unwrap()
            .get_changeset(card.id)
            .unwrap()
            .unwrap();
        assert_eq!(
            (row.state.as_str(), row.error.as_deref()),
            ("failed", Some("trn: unreachable"))
        );
    }

    /// R2/R3: a failed card is open, so a stale one is withdrawn like a
    /// proposed one.
    #[test]
    fn withdrawal_withdraws_a_stale_failed_card() {
        let _g = lock_registry_for_test();
        let (store, _) = fleet_store(vec![skill("kept")]);
        put(&store, "oci", vec![unmanaged("oci", "w")]);
        reconcile(&store, true).unwrap();
        let card = store.lock().unwrap().list_changesets().unwrap()[0].clone();
        store
            .lock()
            .unwrap()
            .set_changeset_state(card.id, "failed", Some("oci: import failed"))
            .unwrap();
        put(&store, "oci", vec![]);
        let r = reconcile(&store, true).unwrap();
        assert_eq!(r.withdrawn, 1);
        let row = store
            .lock()
            .unwrap()
            .get_changeset(card.id)
            .unwrap()
            .unwrap();
        assert_eq!(
            (row.state.as_str(), row.error.as_deref()),
            ("dismissed", Some(WITHDRAWN))
        );
    }

    /// R10: the pass's automatic hide never overturns a person's verdict on
    /// the same (kind, name, hash).
    #[test]
    fn a_persons_verdict_survives_an_automatic_hide_of_the_same_key() {
        let _g = lock_registry_for_test();
        let (store, _) = fleet_store(vec![skill("kept")]);
        let person = TriageVerdictRow {
            catalog_id: None,
            kind: "hook".into(),
            name: "stop".into(),
            content_hash: "h-stop".into(),
            verdict: "rejected".into(),
            decider: "person".into(),
            decided_at: 1,
        };
        store
            .lock()
            .unwrap()
            .upsert_triage_verdict(&person)
            .unwrap();
        put(&store, "oci", vec![fleet_hook("oci")]);
        let r = reconcile(&store, true).unwrap();
        assert_eq!(r.hidden, 0, "already decided");
        assert_eq!(store.lock().unwrap().triage_verdicts().unwrap(), [person]);
    }

    /// R9: a verdict holds a subject by its content hash; a changed copy is
    /// a new subject and gets a card again.
    #[test]
    fn a_verdict_on_an_old_hash_does_not_suppress_a_card_for_a_changed_hash() {
        let _g = lock_registry_for_test();
        let (store, _) = fleet_store(vec![skill("kept")]);
        store
            .lock()
            .unwrap()
            .upsert_triage_verdict(&TriageVerdictRow {
                catalog_id: None,
                kind: "skill".into(),
                name: "w".into(),
                content_hash: "h-old".into(),
                verdict: "rejected".into(),
                decider: "person".into(),
                decided_at: 1,
            })
            .unwrap();
        let mut old = unmanaged("oci", "w");
        old.host_hash = Some("h-old".into());
        put(&store, "oci", vec![old]);
        assert_eq!(reconcile(&store, true).unwrap().inserted, 0, "held");

        put(&store, "oci", vec![unmanaged("oci", "w")]);
        assert_eq!(reconcile(&store, true).unwrap().inserted, 1);
        let s = store.lock().unwrap();
        let card = &s.list_changesets().unwrap()[0];
        assert_eq!(card.kind, "new");
        let items = s.changeset_items(card.id).unwrap();
        assert_eq!(
            ItemParams::parse(items[0].params.as_deref())
                .hash
                .as_deref(),
            Some("h-w")
        );
    }

    /// R16: a layer never rolled out whose member is `missing` on a host it
    /// is assigned to gets a Rollout card for that host.
    #[test]
    fn a_layer_never_rolled_out_with_missing_members_gets_a_rollout_card() {
        let _g = lock_registry_for_test();
        let (store, p) = fleet_store(vec![skill("w")]);
        let mut core = author::layer_template("core", Axis::Context);
        core.members.push("skill/w".into());
        let (layers, errors) = LayerSet::from_layers(vec![core]);
        assert!(errors.is_empty(), "{errors:?}");
        registry::install_personal(Catalog {
            id: p,
            name: "personal".into(),
            assets: vec![skill("w")],
            layers,
            ..Default::default()
        })
        .unwrap();
        store
            .lock()
            .unwrap()
            .set_host_layers_for("oci", p, None, &["core"])
            .unwrap();
        let mut missing = unmanaged("oci", "w");
        missing.state = "missing".into();
        missing.catalog_id = Some(p);
        put(&store, "oci", vec![missing]);

        reconcile(&store, true).unwrap();
        let s = store.lock().unwrap();
        let cards = s.list_changesets().unwrap();
        let rollout = cards
            .iter()
            .find(|c| c.kind == "rollout")
            .expect("a rollout card");
        assert_eq!(rollout.summary, "Roll out core to oci");
        let items = s.changeset_items(rollout.id).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "oci");
        assert_eq!(
            ItemParams::parse(items[0].params.as_deref()).assets,
            ["skill/w"]
        );
    }

    fn drifted(host: &str, name: &str, catalog_id: i64) -> AssetInventoryRow {
        let mut r = unmanaged(host, name);
        r.state = "drifted".into();
        r.managed = true;
        r.catalog_id = Some(catalog_id);
        r
    }

    /// Assets M5 (R5): a copy that is only behind its catalog is not drift a
    /// person must decide — no card; an edited or unknown one is.
    #[test]
    fn only_edited_or_unverified_copies_become_drift_facts() {
        let row = |host: &str, side: Option<&str>| AssetInventoryRow {
            host_alias: host.into(),
            harness: "claude".into(),
            kind: "skill".into(),
            name: "w".into(),
            state: "drifted".into(),
            managed: true,
            catalog_id: Some(1),
            drift_side: side.map(String::from),
            ..Default::default()
        };
        let facts = drift_facts(&[
            row("oci", Some("catalog")),
            row("trn", Some("host")),
            row("htz", None),
        ]);
        let got: Vec<(&str, bool)> = facts.iter().map(|d| (d.host.as_str(), d.edited)).collect();
        assert_eq!(got, [("trn", true), ("htz", false)]);
    }

    /// Assets M5 (R5, PF4): through the real pass — `proposals` reads the
    /// side — an edited copy's card says where it was edited, one with an
    /// unknown side keeps M4's wording, and a copy only behind its catalog
    /// opens none; an open card whose row turns `catalog` is withdrawn.
    #[test]
    fn the_pass_opens_drift_cards_only_for_copies_someone_edited() {
        let _g = lock_registry_for_test();
        let (store, p) = fleet_store(vec![skill("w")]);
        let row = |host: &str, side: Option<&str>| {
            let mut r = drifted(host, "w", p);
            r.drift_side = side.map(String::from);
            r
        };
        let open_drift = |store: &Mutex<Store>| -> Vec<String> {
            let mut v: Vec<String> = store
                .lock()
                .unwrap()
                .list_changesets()
                .unwrap()
                .into_iter()
                .filter(|c| c.kind == "drift" && is_open(&c.state))
                .map(|c| c.summary)
                .collect();
            v.sort();
            v
        };
        put(&store, "oci", vec![row("oci", None)]);
        put(&store, "trn", vec![row("trn", Some("host"))]);
        reconcile(&store, true).unwrap();
        assert_eq!(
            open_drift(&store),
            [
                "skill/w differs on oci from catalog personal",
                "skill/w was edited on trn (catalog personal)",
            ]
        );

        put(&store, "oci", vec![row("oci", Some("catalog"))]);
        let r = reconcile(&store, true).unwrap();
        assert_eq!(r.withdrawn, 1, "{r:?}");
        assert_eq!(
            open_drift(&store),
            ["skill/w was edited on trn (catalog personal)"]
        );
    }

    /// Task 4 review (M5): the pass feeds the rules its facts in one stable
    /// order, so the same inputs read in another order make the same cards
    /// — positions and summaries do not churn between refreshes.
    #[test]
    fn the_same_facts_in_another_order_make_identical_cards() {
        let catalogs = vec![CatalogFacts {
            id: 1,
            name: "personal".into(),
            org_id: None,
            loaded: true,
            asset_count: 3,
            layers: vec![],
            slugs: BTreeSet::new(),
        }];
        let mut rows = vec![
            unmanaged("oci", "a"),
            unmanaged("trn", "a"),
            unmanaged("oci", "b"),
            unmanaged("trn", "c"),
            drifted("oci", "d", 1),
            drifted("trn", "d", 1),
            drifted("oci", "e", 1),
        ];
        rows[1].host_hash = Some("h-a2".into());
        let gap = |layer: &str, host: &str, assets: &[&str]| LayerGap {
            catalog_id: 1,
            layer: layer.into(),
            host: host.into(),
            assets: assets.iter().map(|a| a.to_string()).collect(),
        };
        let gaps = vec![
            gap("core", "oci", &["skill/x", "skill/y"]),
            gap("core", "trn", &["skill/y"]),
            gap("tools", "oci", &["skill/z"]),
        ];
        let hosts = vec![
            HostFacts {
                alias: "oci".into(),
                org_id: None,
            },
            HostFacts {
                alias: "trn".into(),
                org_id: None,
            },
        ];
        let facts =
            |rows: Vec<AssetInventoryRow>, gaps: Vec<LayerGap>, hosts: Vec<HostFacts>| PassFacts {
                rows,
                hosts,
                catalogs: catalogs.clone(),
                verdict_keys: BTreeSet::new(),
                open: vec![],
                bootstrap_applied: true,
                gaps,
            };
        let forward = facts(rows.clone(), gaps.clone(), hosts.clone());
        let mut rev_rows = rows;
        rev_rows.reverse();
        let mut rev_gaps: Vec<LayerGap> = gaps
            .into_iter()
            .map(|mut g| {
                g.assets.reverse();
                g
            })
            .collect();
        rev_gaps.reverse();
        let mut rev_hosts = hosts;
        rev_hosts.reverse();
        let backward = facts(rev_rows, rev_gaps, rev_hosts);

        let (_, a) = proposals(&forward, true);
        let (_, b) = proposals(&backward, true);
        assert!(a.iter().any(|c| c.kind == CardKind::Drift));
        assert!(a.iter().any(|c| c.kind == CardKind::Rollout));
        assert_eq!(a, b);
    }

    /// R18: with `catalog.auto` off the tick's pass writes nothing; an
    /// on-demand propose still builds cards, with `hide` items instead of
    /// automatic verdicts.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn with_auto_off_the_tick_writes_nothing_and_propose_brings_hide_items() {
        let _g = lock_registry_for_test();
        let (store, _) = fleet_store(vec![]);
        let ssh = Arc::new(SshClient::new());
        settings::set(&store.lock().unwrap(), settings::CATALOG_AUTO, "false").unwrap();
        put(
            &store,
            "oci",
            vec![unmanaged("oci", "w"), fleet_hook("oci")],
        );
        after_scan_pass(&store, &ssh);
        assert!(store.lock().unwrap().list_changesets().unwrap().is_empty());

        let cards = super::super::propose(&store).await.unwrap();
        assert_eq!(cards.len(), 1);
        let (_, items) = super::super::card(cards[0].id, &store).unwrap();
        assert!(items.iter().any(|i| i.action == "hide" && i.name == "stop"));
        assert!(store.lock().unwrap().triage_verdicts().unwrap().is_empty());
    }

    /// A poisoned store (a panic while a guard was held) makes the hook log
    /// and return — never panic, never pretend `catalog.auto` is off
    /// silently.
    #[test]
    fn a_poisoned_store_makes_the_hook_return_without_panicking() {
        let _g = lock_registry_for_test();
        let (store, _) = fleet_store(vec![]);
        let ssh = Arc::new(SshClient::new());
        put(&store, "oci", vec![unmanaged("oci", "w")]);
        let poison = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = store.lock().unwrap();
            panic!("poisoning the store mutex for a test");
        }));
        assert!(poison.is_err());
        assert!(store.is_poisoned());
        after_scan_pass(&store, &ssh);
        after_scan_pass(&store, &ssh);
        assert!(STORE_UNREADABLE_LOGGED.load(Ordering::Relaxed));
        let s = store.lock().unwrap_or_else(|e| e.into_inner());
        assert!(s.list_changesets().unwrap().is_empty(), "nothing written");
    }

    /// R19: the tick's pass never waits for an apply; it skips and the next
    /// pass catches up.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn the_tick_pass_never_waits_for_an_apply() {
        let _g = lock_registry_for_test();
        let (store, _) = fleet_store(vec![]);
        let ssh = Arc::new(SshClient::new());
        put(
            &store,
            "oci",
            (0..3).map(|n| unmanaged("oci", &format!("s{n}"))).collect(),
        );
        let busy = super::super::APPLY_LOCK.lock().await;
        after_scan_pass(&store, &ssh);
        assert!(
            store.lock().unwrap().list_changesets().unwrap().is_empty(),
            "skipped, not waited"
        );
        drop(busy);
        after_scan_pass(&store, &ssh);
        assert_eq!(store.lock().unwrap().list_changesets().unwrap().len(), 1);
    }

    /// R17: a panic in SB6's detached task is caught and logged — the task
    /// itself ends normally, so nothing reaches the tick or the runtime.
    #[tokio::test]
    async fn a_panic_in_the_detached_sync_only_logs() {
        let h = spawn_logged("test sync", async { panic!("SB6 blew up") }).expect("a runtime");
        assert!(h.await.is_ok(), "the panic did not escape the task");
    }

    /// Assets M5 (R9, PF4): the pass itself prunes its old, untouched
    /// withdrawn cards — a fresh one stays until it is past retention.
    #[test]
    fn a_pass_prunes_an_old_untouched_withdrawn_card() {
        let _g = lock_registry_for_test();
        let (store, _) = fleet_store(vec![skill("kept")]);
        put(&store, "oci", vec![unmanaged("oci", "w")]);
        reconcile(&store, true).unwrap();
        let card = store.lock().unwrap().list_changesets().unwrap()[0].clone();
        put(&store, "oci", vec![]);
        assert_eq!(reconcile(&store, true).unwrap().withdrawn, 1);
        assert!(
            store
                .lock()
                .unwrap()
                .get_changeset(card.id)
                .unwrap()
                .is_some(),
            "a card withdrawn now is kept"
        );
        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute(
                "UPDATE changesets SET withdrawn_at = ?2 WHERE id = ?1",
                rusqlite::params![card.id, now_unix() - WITHDRAWN_RETENTION_SECS - 1],
            )
            .unwrap();
        reconcile(&store, true).unwrap();
        assert!(
            store
                .lock()
                .unwrap()
                .get_changeset(card.id)
                .unwrap()
                .is_none(),
            "pruned by the pass"
        );
    }

    /// Assets M6 (R5): a person's layer card has no rule subject, so no
    /// pass ever withdraws it; only a person dismisses it.
    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn the_pass_never_withdraws_a_persons_layer_card() {
        use crate::service::catalog::changesets::testkit::fleet_with_core;
        use crate::service::catalog::changesets::{propose_layer, LayerChange};
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let v = propose_layer(
            LayerChange::Create {
                catalog: None,
                layer: "servers".into(),
                axis: None,
                description: None,
                members: vec![],
                orgs: vec![],
            },
            &f.store,
        )
        .await
        .unwrap();
        reconcile(&f.store, true).unwrap();
        reconcile(&f.store, true).unwrap();
        let after = crate::service::catalog::changesets::get(v.id, &f.store).unwrap();
        assert_eq!(after.state, "proposed", "{:?}", after.error);
    }
}
