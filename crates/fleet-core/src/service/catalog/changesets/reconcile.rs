//! Assets M4: the reconcile pass — gather the facts, ask the rules, write
//! the cards (spec: "A reconcile pass runs after each scan-tick pass and on
//! demand"). Synchronous: store, then registry, never SSH. Store rows are
//! read under one guard that is dropped before the registry is read, and the
//! writes take a fresh guard (store → registry is never allowed).

use super::rules::{
    self, CatalogFacts, DriftFacts, HostFacts, LayerFacts, LayerGap, RulesInput, SubjectItem,
};
use super::{
    is_open, CardKind, Decider, ItemKey, ItemParams, ProposedCard, ProposedItem, APPLY_LOCK,
};
use crate::ipc_error::{lock, IpcError};
use crate::service::catalog::identity::{self, AssetIdentity, IdentityClass};
use crate::service::catalog::repo::Catalog;
use crate::service::catalog::{effective, registry};
use crate::service::settings;
use crate::store::{
    now_unix, AssetInventoryRow, CatalogRow, ChangesetItemRow, ChangesetRow, HostLayerRow,
    NewChangesetItem, Store, TriageVerdictRow,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

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
/// internals here instead of proposing `hide` items (R18).
pub fn reconcile(store: &Mutex<Store>, auto: bool) -> Result<ReconcileReport, IpcError> {
    let facts = gather(store)?;
    let (identities, proposed) = proposals(&facts, auto);
    write(store, &facts, &identities, &proposed, auto)
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
    let mut drifted: Vec<DriftFacts> = rows
        .iter()
        .filter(|r| r.state == "drifted" && r.managed && r.harness == "claude")
        .filter_map(|r| {
            Some(DriftFacts {
                catalog_id: r.catalog_id?,
                kind: r.kind.clone(),
                name: r.name.clone(),
                host: r.host_alias.clone(),
                host_hash: r.host_hash.clone(),
            })
        })
        .collect();
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
/// proposal named withdrawn (never a rollout, never one with applied items).
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
    let by_subject: BTreeMap<String, &OpenCard> = f
        .open
        .iter()
        .map(|o| (subject_of_row(&o.0, &o.1), o))
        .collect();
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
        s.set_changeset_state(row.id, "dismissed", Some(WITHDRAWN))?;
        report.withdrawn += 1;
    }
    Ok(report)
}

/// Refresh an open card in place when its proposal changed (R2). PF14: the
/// items a person rejected stay rejected — matched by [`ItemKey`] (catalog,
/// kind, name, action, host) — and every other item starts pending. Both
/// writes happen under the caller's one store guard, so no reader sees the
/// card between them. `true` when the card was rewritten.
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
    let rejected: BTreeSet<ItemKey> = existing
        .iter()
        .filter(|i| i.state == "rejected")
        .map(ItemKey::of_row)
        .collect();
    if !s.replace_changeset_items(row.id, &card.summary, items)? {
        return Ok(false);
    }
    let keep: Vec<i64> = card
        .items
        .iter()
        .enumerate()
        .filter(|(_, i)| rejected.contains(&i.key()))
        .map(|(p, _)| p as i64)
        .collect();
    if !keep.is_empty() {
        s.set_changeset_item_states(row.id, &keep, "rejected")?;
    }
    Ok(true)
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
    let id_of: BTreeMap<String, i64> = configured
        .iter()
        .map(|r| {
            let label = if r.org_id.is_none() {
                "personal".to_string()
            } else {
                r.name.clone()
            };
            (label, r.id)
        })
        .collect();
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

/// The scan tick's hook (R19, carry 4): with `catalog.auto` on, one pass —
/// unless an apply holds `APPLY_LOCK`, in which case this pass is skipped
/// (the tick never waits). Errors are logged, never returned: the tick's own
/// bookkeeping (`owed`, `seen`) is not this pass's business.
pub fn after_scan_pass(store: &Arc<Mutex<Store>>) {
    let auto = store
        .lock()
        .map(|s| settings::get_bool(&s, settings::CATALOG_AUTO))
        .unwrap_or(false);
    if !auto {
        return;
    }
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
        settings::set(&store.lock().unwrap(), settings::CATALOG_AUTO, "false").unwrap();
        put(
            &store,
            "oci",
            vec![unmanaged("oci", "w"), fleet_hook("oci")],
        );
        after_scan_pass(&store);
        assert!(store.lock().unwrap().list_changesets().unwrap().is_empty());

        let cards = super::super::propose(&store).await.unwrap();
        assert_eq!(cards.len(), 1);
        let (_, items) = super::super::card(cards[0].id, &store).unwrap();
        assert!(items.iter().any(|i| i.action == "hide" && i.name == "stop"));
        assert!(store.lock().unwrap().triage_verdicts().unwrap().is_empty());
    }

    /// R19: the tick's pass never waits for an apply; it skips and the next
    /// pass catches up.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn the_tick_pass_never_waits_for_an_apply() {
        let _g = lock_registry_for_test();
        let (store, _) = fleet_store(vec![]);
        put(
            &store,
            "oci",
            (0..3).map(|n| unmanaged("oci", &format!("s{n}"))).collect(),
        );
        let busy = super::super::APPLY_LOCK.lock().await;
        after_scan_pass(&store);
        assert!(
            store.lock().unwrap().list_changesets().unwrap().is_empty(),
            "skipped, not waited"
        );
        drop(busy);
        after_scan_pass(&store);
        assert_eq!(store.lock().unwrap().list_changesets().unwrap().len(), 1);
    }
}
