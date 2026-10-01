//! The effective catalog for a host (Assets M2): which loaded catalogs a
//! host accepts ([`acceptance`]), and what it should end up with once
//! every accepted catalog is resolved for it, the scope boundary applied
//! and collisions between catalogs refused ([`effective_for_host`]).

use crate::ipc_error::{lock, IpcError};
use crate::service::catalog::model::{Asset, Kind, Scope};
use crate::service::catalog::registry;
use crate::service::catalog::repo::{Catalog, CatalogRef};
use crate::service::catalog::resolve::Provenance;
use crate::service::catalog::sync::layers::resolve_rows;
use crate::store::{HostLayerRow, Store};
use serde::{Deserialize, Serialize};
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

/// What a host may take from a catalog, having decided which of the
/// catalog's assets it would otherwise want.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acceptance {
    /// Nothing from this catalog.
    No,
    /// Only assets marked `scope: shared`.
    SharedOnly,
    /// Every asset.
    All,
}

/// What a host with `host_org` may take from a catalog owned by
/// `catalog_org` (`None` = personal), given the host's admissions (M3;
/// always empty in M2).
///
/// - personal catalog: a host with no org gets `All`; a host with an org
///   gets `SharedOnly`.
/// - org catalog `X`: a host in org `X` gets `All`; a host with no org
///   whose admissions include `catalog_id` gets `All`; everyone else gets
///   `No`. Admission never crosses orgs — a host in a *different* org gets
///   `No` even if `catalog_id` is in its admissions list.
pub fn acceptance(
    host_org: Option<i64>,
    catalog_id: i64,
    catalog_org: Option<i64>,
    admitted: &[i64],
) -> Acceptance {
    match (catalog_org, host_org) {
        (None, None) => Acceptance::All,
        (None, Some(_)) => Acceptance::SharedOnly,
        (Some(c), Some(h)) if c == h => Acceptance::All,
        (Some(_), None) if admitted.contains(&catalog_id) => Acceptance::All,
        _ => Acceptance::No,
    }
}

/// One asset that would have gone to a host but may not. The planner turns
/// each into a `blocked` action carrying `reason`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    /// `Kind::as_str()` of the asset.
    pub kind: String,
    /// The asset's catalog name (not its install name).
    pub name: String,
    pub reason: String,
}

/// What one host should have, across every catalog it accepts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveSet {
    /// What the host should have: every accepted catalog's resolved assets,
    /// merged, minus refusals. `origin` says which catalog each came from.
    /// `head`/`loaded_at` are the personal catalog's; `layers` is empty, as
    /// on every resolved catalog — a layer never travels into the planner.
    pub catalog: Catalog,
    /// `<kind>/<name>` → provenance (layer + catalog), for assets a layer
    /// introduced. An asset from an unlayered catalog has no entry, as on
    /// the no-layering path of `resolve`.
    pub provenance: BTreeMap<String, Provenance>,
    /// Assets that would have gone to this host but may not: scope boundary
    /// or a collision between catalogs. The planner turns each into a
    /// `blocked` action with this reason.
    pub refused: Vec<Refusal>,
    /// Whether any accepted catalog has layers assigned on this host.
    pub layered: bool,
    /// `<kind>/<name>` → the layer that excluded it, merged across every
    /// accepted catalog's own `resolve()` — `resolve_preview`'s "why is this
    /// gone" (`Resolution::excluded`). On the single-catalog path this is
    /// exactly what `resolve_for_host` would answer. On the rare cross-
    /// catalog clash (two different catalogs each exclude an asset sharing
    /// the same `(kind, name)` key), the second value is prefixed with its
    /// catalog's name to disambiguate; the first is left as-is, so the
    /// common (single-catalog, or no clash) case is byte-for-byte what it
    /// was before Assets M2. `#[serde(default)]` because `EffectiveSet`
    /// travels the wire via `Resolution`: a hub older than this field never
    /// sends it.
    #[serde(default)]
    pub excluded: BTreeMap<String, String>,
    /// `(kind, name)` of every private asset the scope boundary dropped
    /// SILENTLY — an org host's unlayered personal catalog, where keeping
    /// only the shared slice is the default, not a mistake, so nothing goes
    /// into `refused` (no `Blocked` noise for the common case: that private
    /// asset was never meant for an org host to begin with). But "silent"
    /// must not mean "destructive": a host that already has this asset
    /// synced from before it had an org (or before Assets M2 at all) must
    /// keep it — the planner suppresses its removal as a manifest orphan the
    /// same way it does for `refused`, and reports a `Noop` with why
    /// whenever the asset is actually on the host. `#[serde(default)]`
    /// because `EffectiveSet` travels the wire via `Resolution`: a hub older
    /// than this field never sends it.
    #[serde(default)]
    pub withheld: BTreeSet<(String, String)>,
}

/// Compute `host_alias`'s effective catalog.
///
/// **Lock order.** All store reads (the host's org, the catalog rows, and
/// every catalog's layer rows for this host) happen under ONE store guard
/// that is dropped before the registry is taken; the registry closure never
/// touches the store. Store → registry is never allowed (see `registry`).
///
/// Admissions (`host_catalogs`) arrive in M3: here every host's admission
/// list is empty, so a host with no org accepts only personal.
pub fn effective_for_host(
    store: &Mutex<Store>,
    host_alias: &str,
) -> Result<EffectiveSet, IpcError> {
    // Store phase — one guard, dropped at the end of this block.
    let (host_org, personal_id, rows_by_catalog) = {
        let s = lock(store)?;
        let host_org = s.host_org(host_alias)?;
        let mut personal_id = None;
        let mut rows = BTreeMap::new();
        for c in s.list_catalogs()? {
            if c.org_id.is_none() {
                personal_id = Some(c.id);
            }
            rows.insert(c.id, s.get_host_layers_for(host_alias, c.id)?);
        }
        (host_org, personal_id, rows)
    };
    let admitted: &[i64] = &[];

    // Registry phase — no store access from here on.
    registry::with_catalogs(|catalogs| {
        compose(catalogs, host_alias, host_org, admitted, |cat: &Catalog| {
            // A hand-built catalog with id 0 stands for the personal one.
            let id = if cat.id == 0 && cat.org_id.is_none() {
                personal_id
            } else {
                Some(cat.id)
            };
            id.and_then(|id| rows_by_catalog.get(&id))
                .map(Vec::as_slice)
                .unwrap_or(&[])
        })
    })
}

/// One asset collected from one accepted catalog, before collisions.
struct Candidate {
    asset: Asset,
    from: CatalogRef,
    provenance: Option<Provenance>,
}

fn key_of(asset: &Asset) -> String {
    format!("{}/{}", asset.kind().as_str(), asset.header.name)
}

/// The pure half of [`effective_for_host`]: everything the store had to say
/// is already in `host_org` and `rows_for`.
fn compose<'r>(
    catalogs: &BTreeMap<i64, Catalog>,
    host_alias: &str,
    host_org: Option<i64>,
    admitted: &[i64],
    rows_for: impl Fn(&Catalog) -> &'r [HostLayerRow],
) -> Result<EffectiveSet, IpcError> {
    let Some(personal) = catalogs.values().find(|c| c.org_id.is_none()) else {
        return Err(IpcError::new(
            super::E_CATALOG_NOT_CONFIGURED,
            "catalog not loaded; call catalog_load",
        ));
    };

    let mut collected: Vec<Candidate> = Vec::new();
    let mut refused: Vec<Refusal> = Vec::new();
    let mut problems = Vec::new();
    let mut layered = false;
    let mut excluded: BTreeMap<String, String> = BTreeMap::new();
    let mut withheld: BTreeSet<(String, String)> = BTreeSet::new();

    for cat in registry::in_order(catalogs) {
        let accepts = acceptance(host_org, cat.id, cat.org_id, admitted);
        if accepts == Acceptance::No {
            continue;
        }
        let res = resolve_rows(cat, host_alias, rows_for(cat))?;
        layered |= res.layered;
        problems.extend(res.catalog.problems);
        // Merged in composition order: a key can only repeat here if TWO
        // different catalogs each exclude an asset of the same `(kind,
        // name)` — within one catalog's own `resolve()`, `excluded`'s keys
        // are already unique. Prefix the second (and only the second) value
        // with its catalog's name so the clash is distinguishable; the
        // common case (one catalog, or no shared key) stays exactly the
        // plain layer name `resolve_for_host` would give.
        for (key, layer_name) in res.excluded {
            match excluded.entry(key) {
                Entry::Occupied(mut e) => {
                    e.insert(format!("{}: {layer_name}", cat.name));
                }
                Entry::Vacant(e) => {
                    e.insert(layer_name);
                }
            }
        }
        let from = CatalogRef {
            id: cat.id,
            name: cat.name.clone(),
        };
        for asset in res.catalog.assets {
            let key = key_of(&asset);
            let provenance = res.provenance.get(&key).cloned();
            if accepts == Acceptance::SharedOnly && asset.header.scope == Scope::Private {
                // Unlayered: the host would otherwise take the whole
                // catalog, and keeping only the shared part is the default,
                // not a mistake. Layered: a layer assigned to this host
                // names a private asset — refuse it, per asset, with why.
                if res.layered {
                    let layer = provenance
                        .as_ref()
                        .map(|p| p.introduced_by.as_str())
                        .unwrap_or("none");
                    refused.push(Refusal {
                        kind: asset.kind().as_str().to_string(),
                        name: asset.header.name.clone(),
                        reason: format!(
                            "private asset \"{}\" (catalog {}, layer {layer}) may not go to \
                             org host {host_alias}; mark it shared or remove it from the layer",
                            asset.header.name, cat.name
                        ),
                    });
                } else {
                    // Dropped silently — no `Refusal`, no `Blocked` noise:
                    // this is the default, not a mistake. But "silent" must
                    // never mean "destructive": record it so `plan_sync` can
                    // keep the host's existing copy, if any, exactly as it
                    // is (same mechanism as `refused`) rather than removing
                    // it as a manifest orphan.
                    withheld.insert((asset.kind().as_str().to_string(), asset.header.name.clone()));
                }
                continue;
            }
            collected.push(Candidate {
                asset,
                from: from.clone(),
                provenance,
            });
        }
    }

    // Collisions: two catalogs' assets overlapping on `(kind, name)` (the
    // key `origin`, `provenance` and `Catalog::find` use) OR on `(kind,
    // install_name)` (the directory a harness installs into). Each
    // candidate is reachable via both keys, so overlapping groups are
    // merged into connected components (union-find over candidate
    // indices); every member of a component spanning more than one
    // catalog is refused, exactly once.
    let mut parent: Vec<usize> = (0..collected.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    // (key kind, asset kind, value) → first candidate seen with that key.
    let mut first: BTreeMap<(u8, Kind, String), usize> = BTreeMap::new();
    for (i, c) in collected.iter().enumerate() {
        let kind = c.asset.kind();
        for key in [
            (0u8, kind, c.asset.header.name.clone()),
            (1u8, kind, c.asset.install_name().to_string()),
        ] {
            match first.get(&key) {
                Some(&j) => {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    if a != b {
                        // Root at the lower index, so a component's root is
                        // its first-collected member.
                        parent[a.max(b)] = a.min(b);
                    }
                }
                None => {
                    first.insert(key, i);
                }
            }
        }
    }
    let mut components: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..collected.len() {
        let root = find(&mut parent, i);
        components.entry(root).or_default().push(i);
    }
    let mut dropped: BTreeSet<usize> = BTreeSet::new();
    // Keyed by root = first-collected member: reported personal first.
    let conflicts: Vec<&Vec<usize>> = components
        .values()
        .filter(|idx| {
            // A single-catalog component is left alone: duplicate names or
            // install names within one catalog are the loader's job.
            idx.iter()
                .map(|&i| collected[i].from.id)
                .collect::<BTreeSet<_>>()
                .len()
                > 1
        })
        .collect();
    for idx in conflicts {
        let reason = format!(
            "conflict: {} — use install_as or move one",
            idx.iter()
                .map(|&i| format!(
                    "{}/{}",
                    collected[i].from.name, collected[i].asset.header.name
                ))
                .collect::<Vec<_>>()
                .join(" vs ")
        );
        for &i in idx {
            let a = &collected[i].asset;
            refused.push(Refusal {
                kind: a.kind().as_str().to_string(),
                name: a.header.name.clone(),
                reason: reason.clone(),
            });
            dropped.insert(i);
        }
    }

    let mut catalog = Catalog {
        id: 0,
        name: String::new(),
        org_id: None,
        head: personal.head.clone(),
        loaded_at: personal.loaded_at,
        problems,
        ..Default::default()
    };
    let mut provenance = BTreeMap::new();
    for (i, c) in collected.into_iter().enumerate() {
        if dropped.contains(&i) {
            continue;
        }
        let key = key_of(&c.asset);
        if let Some(mut p) = c.provenance {
            p.catalog = c.from.name.clone();
            provenance.insert(key.clone(), p);
        }
        catalog.origin.insert(key, c.from);
        catalog.assets.push(c.asset);
    }

    Ok(EffectiveSet {
        catalog,
        provenance,
        refused,
        layered,
        excluded,
        withheld,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const PERSONAL: i64 = 1;
    const ACME: i64 = 2;
    const ACME_ORG: i64 = 10;

    #[test]
    fn a_host_without_an_org_takes_all_of_personal_and_only_admitted_org_catalogs() {
        assert_eq!(acceptance(None, PERSONAL, None, &[]), Acceptance::All);
        assert_eq!(acceptance(None, ACME, Some(ACME_ORG), &[]), Acceptance::No);
        assert_eq!(
            acceptance(None, ACME, Some(ACME_ORG), &[ACME]),
            Acceptance::All
        );
    }

    #[test]
    fn an_org_host_takes_its_org_catalog_and_only_shared_personal_assets() {
        assert_eq!(
            acceptance(Some(ACME_ORG), PERSONAL, None, &[]),
            Acceptance::SharedOnly
        );
        assert_eq!(
            acceptance(Some(ACME_ORG), ACME, Some(ACME_ORG), &[]),
            Acceptance::All
        );
        assert_eq!(
            acceptance(Some(99), ACME, Some(ACME_ORG), &[ACME]),
            Acceptance::No,
            "admission never crosses orgs"
        );
    }

    // ---- effective_for_host -------------------------------------------

    use crate::service::catalog::layer::{Layer, LayerSet};
    use crate::service::catalog::model::{Asset, Kind};
    use crate::service::catalog::registry;

    const ORG_10: i64 = 10;
    const ORG_11: i64 = 11;

    fn skill(name: &str, scope: &str) -> Asset {
        Asset::from_yaml(
            Some(Kind::Skill),
            &format!("kind: skill\nname: {name}\ndescription: d\nscope: {scope}\n"),
        )
        .unwrap()
    }

    /// A store with hosts `h` (no org yet), orgs 10 (`acme`) and 11
    /// (`other`), the personal catalog row and an `acme` catalog row owned
    /// by org 10. Returns `(store, personal_id, acme_id)`.
    fn seeded_store() -> (Mutex<Store>, i64, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        s.set_catalog_config("/p", None).unwrap();
        let personal = s.personal_catalog().unwrap().unwrap().id;
        s.conn_ref()
            .execute(
                "INSERT INTO orgs (id, name, created_at) VALUES (10, 'acme', 0), (11, 'other', 0)",
                [],
            )
            .unwrap();
        s.conn_ref()
            .execute(
                "INSERT INTO catalogs (name, repo_path, org_id, created_at) VALUES ('acme', '/a', 10, 0)",
                [],
            )
            .unwrap();
        let acme: i64 = s
            .conn_ref()
            .query_row("SELECT id FROM catalogs WHERE name='acme'", [], |r| {
                r.get(0)
            })
            .unwrap();
        (Mutex::new(s), personal, acme)
    }

    fn personal_cat(id: i64, assets: Vec<Asset>, layers: LayerSet) -> Catalog {
        Catalog {
            id,
            name: "personal".into(),
            org_id: None,
            head: "hp".into(),
            assets,
            layers,
            ..Default::default()
        }
    }

    fn acme_cat(id: i64, assets: Vec<Asset>) -> Catalog {
        Catalog {
            id,
            name: "acme".into(),
            org_id: Some(ORG_10),
            head: "ha".into(),
            assets,
            ..Default::default()
        }
    }

    fn names(e: &EffectiveSet) -> Vec<&str> {
        e.catalog
            .assets
            .iter()
            .map(|a| a.header.name.as_str())
            .collect()
    }

    #[test]
    fn only_personal_behaves_as_before() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, _acme) = seeded_store();
        registry::install_personal(personal_cat(
            personal,
            vec![skill("a", "private"), skill("b", "private")],
            LayerSet::default(),
        ))
        .unwrap();

        let e = effective_for_host(&store, "h").unwrap();
        assert_eq!(names(&e), vec!["a", "b"]);
        assert!(e.refused.is_empty(), "{:?}", e.refused);
        assert!(!e.layered);
        assert_eq!(e.catalog.origin_of(Kind::Skill, "a").name, "personal");
    }

    #[test]
    fn an_org_host_gets_shared_personal_assets_and_its_org_catalog() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, acme) = seeded_store();
        store
            .lock()
            .unwrap()
            .set_host_org("h", Some(ORG_10))
            .unwrap();
        registry::install_personal(personal_cat(
            personal,
            vec![skill("a", "private"), skill("b", "shared")],
            LayerSet::default(),
        ))
        .unwrap();
        registry::install_for_test(acme_cat(acme, vec![skill("c", "private")])).unwrap();

        let e = effective_for_host(&store, "h").unwrap();
        assert_eq!(names(&e), vec!["b", "c"]);
        // The unlayered personal catalog on an org host keeps its shared
        // assets silently: dropping `a` is the default, not a refusal.
        assert!(e.refused.is_empty(), "{:?}", e.refused);
        assert_eq!(e.catalog.origin_of(Kind::Skill, "b").name, "personal");
        assert_eq!(e.catalog.origin_of(Kind::Skill, "c").name, "acme");
    }

    #[test]
    fn a_private_asset_in_a_layer_assigned_to_an_org_host_is_refused_with_a_reason() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, acme) = seeded_store();
        {
            let s = store.lock().unwrap();
            s.set_host_org("h", Some(ORG_10)).unwrap();
            s.set_host_layers_for("h", personal, Some("core"), &[])
                .unwrap();
        }
        let (layers, errs) = LayerSet::from_layers(vec![Layer::from_yaml(
            "kind: layer\nname: core\naxis: role\nmembers:\n  - skill/a\n  - skill/b\n",
        )
        .unwrap()]);
        assert!(errs.is_empty(), "{errs:?}");
        registry::install_personal(personal_cat(
            personal,
            vec![skill("a", "private"), skill("b", "shared")],
            layers,
        ))
        .unwrap();
        registry::install_for_test(acme_cat(acme, vec![skill("c", "private")])).unwrap();

        let e = effective_for_host(&store, "h").unwrap();
        assert_eq!(names(&e), vec!["b", "c"]);
        assert!(e.layered);
        assert_eq!(e.refused.len(), 1, "{:?}", e.refused);
        let r = &e.refused[0];
        assert_eq!((r.kind.as_str(), r.name.as_str()), ("skill", "a"));
        assert!(r.reason.contains("private asset \"a\""), "{}", r.reason);
        assert!(r.reason.contains("layer core"), "{}", r.reason);
        assert!(r.reason.contains("mark it shared"), "{}", r.reason);
        // Provenance carries the catalog as well as the layer.
        assert_eq!(e.provenance["skill/b"].introduced_by, "core");
        assert_eq!(e.provenance["skill/b"].catalog, "personal");
    }

    #[test]
    fn a_collision_between_catalogs_refuses_both_copies() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, acme) = seeded_store();
        store
            .lock()
            .unwrap()
            .set_host_org("h", Some(ORG_10))
            .unwrap();
        registry::install_personal(personal_cat(
            personal,
            vec![skill("x", "shared")],
            LayerSet::default(),
        ))
        .unwrap();
        registry::install_for_test(acme_cat(acme, vec![skill("x", "private")])).unwrap();

        let e = effective_for_host(&store, "h").unwrap();
        assert!(!names(&e).contains(&"x"), "{:?}", names(&e));
        assert_eq!(e.refused.len(), 2, "{:?}", e.refused);
        for r in &e.refused {
            assert_eq!((r.kind.as_str(), r.name.as_str()), ("skill", "x"));
            assert!(
                r.reason.contains("conflict: personal/x vs acme/x"),
                "{}",
                r.reason
            );
        }
    }

    #[test]
    fn an_org_catalog_never_reaches_a_host_of_another_org_or_no_org() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, acme) = seeded_store();
        store.lock().unwrap().upsert_host("h11").unwrap();
        store
            .lock()
            .unwrap()
            .set_host_org("h11", Some(ORG_11))
            .unwrap();
        registry::install_personal(personal_cat(personal, vec![], LayerSet::default())).unwrap();
        registry::install_for_test(acme_cat(acme, vec![skill("c", "shared")])).unwrap();

        let other_org = effective_for_host(&store, "h11").unwrap();
        assert!(!names(&other_org).contains(&"c"), "{:?}", names(&other_org));
        assert!(other_org.refused.is_empty(), "{:?}", other_org.refused);

        let no_org = effective_for_host(&store, "h").unwrap();
        assert!(!names(&no_org).contains(&"c"), "{:?}", names(&no_org));
        assert!(no_org.refused.is_empty(), "{:?}", no_org.refused);
    }

    fn skill_as(name: &str, install_as: &str, scope: &str) -> Asset {
        Asset::from_yaml(
            Some(Kind::Skill),
            &format!(
                "kind: skill\nname: {name}\ndescription: d\nscope: {scope}\ninstall_as: {install_as}\n"
            ),
        )
        .unwrap()
    }

    /// Same `(kind, name)`, different install names: no directory clash,
    /// but `origin`/`provenance` (keyed `<kind>/<name>`) and
    /// `Catalog::find` would be ambiguous — still a collision.
    #[test]
    fn a_same_name_with_different_install_names_is_still_a_collision() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, acme) = seeded_store();
        store
            .lock()
            .unwrap()
            .set_host_org("h", Some(ORG_10))
            .unwrap();
        registry::install_personal(personal_cat(
            personal,
            vec![skill("x", "shared")],
            LayerSet::default(),
        ))
        .unwrap();
        registry::install_for_test(acme_cat(acme, vec![skill_as("x", "acme-x", "private")]))
            .unwrap();

        let e = effective_for_host(&store, "h").unwrap();
        assert!(!names(&e).contains(&"x"), "{:?}", names(&e));
        assert_eq!(e.refused.len(), 2, "{:?}", e.refused);
        for r in &e.refused {
            assert_eq!((r.kind.as_str(), r.name.as_str()), ("skill", "x"));
            assert!(
                r.reason.contains("conflict: personal/x vs acme/x"),
                "{}",
                r.reason
            );
        }
    }

    /// Personal `y` installs as `x`; acme's `x` installs as `x`: the two
    /// overlap on the install-name key only. Both are refused, once each.
    #[test]
    fn a_cross_key_overlap_between_catalogs_is_a_collision() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, acme) = seeded_store();
        store
            .lock()
            .unwrap()
            .set_host_org("h", Some(ORG_10))
            .unwrap();
        registry::install_personal(personal_cat(
            personal,
            vec![skill_as("y", "x", "shared"), skill("keep", "shared")],
            LayerSet::default(),
        ))
        .unwrap();
        registry::install_for_test(acme_cat(acme, vec![skill("x", "private")])).unwrap();

        let e = effective_for_host(&store, "h").unwrap();
        assert_eq!(names(&e), vec!["keep"]);
        assert_eq!(e.refused.len(), 2, "{:?}", e.refused);
        let mut refused: Vec<&str> = e.refused.iter().map(|r| r.name.as_str()).collect();
        refused.sort();
        assert_eq!(refused, vec!["x", "y"]);
        for r in &e.refused {
            assert!(
                r.reason.contains("conflict: personal/y vs acme/x"),
                "{}",
                r.reason
            );
        }
    }

    /// Parity on the layered path: with only personal and a no-org host,
    /// `effective_for_host` answers what `resolve_for_host` answers.
    #[test]
    fn a_layered_personal_only_host_matches_resolve_for_host() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, _acme) = seeded_store();
        store
            .lock()
            .unwrap()
            .set_host_layers_for("h", personal, Some("workstation"), &["extra"])
            .unwrap();
        let (layers, errs) = LayerSet::from_layers(vec![
            Layer::from_yaml(
                "kind: layer\nname: core\naxis: role\nmembers:\n  - skill/a\n  - skill/b\n",
            )
            .unwrap(),
            Layer::from_yaml(
                "kind: layer\nname: workstation\naxis: role\nextends: core\n\
                 members:\n  - skill/c\nexclude:\n  - skill/b\n",
            )
            .unwrap(),
            Layer::from_yaml("kind: layer\nname: extra\naxis: context\nmembers:\n  - skill/d\n")
                .unwrap(),
        ]);
        assert!(errs.is_empty(), "{errs:?}");
        let cat = personal_cat(
            personal,
            ["a", "b", "c", "d", "e"]
                .iter()
                .map(|n| skill(n, "private"))
                .collect(),
            layers,
        );
        registry::install_personal(cat.clone()).unwrap();

        let e = effective_for_host(&store, "h").unwrap();
        let r = crate::service::catalog::sync::layers::resolve_for_host(&store, &cat, "h").unwrap();
        let r_names: Vec<&str> = r
            .catalog
            .assets
            .iter()
            .map(|a| a.header.name.as_str())
            .collect();
        assert_eq!(names(&e), r_names);
        assert_eq!(names(&e), vec!["a", "c", "d"]);
        assert!(e.layered);
        assert_eq!(e.layered, r.layered);
        assert_eq!(
            e.provenance.keys().collect::<Vec<_>>(),
            r.provenance.keys().collect::<Vec<_>>()
        );
        assert!(e.refused.is_empty(), "{:?}", e.refused);
        // `excluded` ("why is this gone") must match too: `workstation`
        // excludes `skill/b`, and on this single-catalog path `compose`'s
        // merge introduces no clash to prefix away.
        assert_eq!(e.excluded, r.excluded);
        assert_eq!(
            e.excluded.get("skill/b").map(String::as_str),
            Some("workstation")
        );
    }
}
