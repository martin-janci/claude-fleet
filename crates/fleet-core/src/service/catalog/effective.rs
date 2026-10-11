//! The effective catalog for a host (Assets M2): which loaded catalogs a
//! host accepts ([`acceptance`]), and what it should end up with once
//! every accepted catalog is resolved for it, the scope boundary applied
//! and collisions between catalogs refused ([`effective_for_host`]).
//!
//! Assets M3: admissions are read from `host_catalogs`, so a host with no
//! org also takes every org catalog admitted on it; and the set says which
//! catalogs speak for the host (`speaks_for`) and why the others' assets
//! are kept rather than removed (`held_back`, Rulings R6/R7).

use crate::ipc_error::{lock, IpcError};
use crate::service::catalog::model::{Asset, Kind, Scope};
use crate::service::catalog::registry;
use crate::service::catalog::repo::{Catalog, CatalogRef, ProblemHolds};
use crate::service::catalog::resolve::Provenance;
use crate::service::catalog::sync::layers::resolve_rows_for;
use crate::store::{CatalogRow, HostLayerRow, Store};
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
/// `catalog_org` (`None` = personal), given the host's admissions
/// (`host_catalogs`, Assets M3).
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
    /// The catalog the refusal is about, when there is exactly one: a scope
    /// boundary names the catalog whose private asset it refused (always
    /// `personal` in M2). A cross-catalog collision refuses a member from
    /// EACH of two or more catalogs, so no single name applies — `None`
    /// there, same as before this field existed.
    #[serde(default)]
    pub catalog: Option<String>,
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
    /// was before Assets M2. `#[serde(default)]`: `EffectiveSet` itself
    /// never crosses the wire, but `resolve_preview` copies this value into
    /// `Resolution::excluded`, which does, so a hub older than this field
    /// never sends it.
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
    /// whenever the asset is actually on the host. `#[serde(default)]`:
    /// `EffectiveSet` itself never crosses the wire, but `resolve_preview`
    /// copies this value into `Resolution::withheld`, which does, so a hub
    /// older than this field never sends it.
    #[serde(default)]
    pub withheld: BTreeSet<(String, String)>,
    /// Assets M3: the catalogs composed for this host — accepted, loaded and
    /// resolved. A manifest entry may be removed as an orphan only when its
    /// catalog is one of these (Rulings R6). `personal` is always named
    /// "personal". Never crosses the wire; `#[serde(default)]` for symmetry.
    #[serde(default)]
    pub speaks_for: BTreeSet<String>,
    /// Assets M3: catalog → why its manifest entries on this host are kept
    /// rather than removed: not loaded, failed to load, or not resolvable
    /// for this host (R6, R7) — only for a catalog the host accepts. Never a
    /// catalog the host does not accept (PF10: `resolve_preview`, which any
    /// client may call, copies this into `Resolution::held_back`, so it must
    /// not list other orgs' catalogs), and never a load error's text (final
    /// review I1: it can carry the clone URL; the full error stays on the
    /// gated paths — `list_catalogs`, `fleet-hub catalog list`, the log).
    #[serde(default)]
    pub held_back: BTreeMap<String, String>,
    /// Assets M3: catalog → why, for every configured or loaded catalog this
    /// host does not accept — including one it still holds a stale admission
    /// for (it has joined an org since). NEVER serialized (it names other
    /// orgs' catalogs): `plan_sync` uses it, through [`Self::held_back_for`],
    /// only for catalogs the host's own manifest names.
    #[serde(skip)]
    pub not_accepted: BTreeMap<String, String>,
    /// Assets M4 (carry 2, R24): catalog → the keys its own load problems
    /// put in doubt, for every catalog in `speaks_for` that has any. A held
    /// key's manifest entries are kept, never removed as orphans. Never
    /// serialized (problem messages can name paths on the hub's machine).
    #[serde(skip)]
    pub problem_held: BTreeMap<String, ProblemHolds>,
}

impl EffectiveSet {
    /// `held_back` plus, for each catalog in `named` (the catalogs the
    /// host's own manifest entries name) that this host does not accept,
    /// the reason it was refused. A name the fleet does not know at all is
    /// left out: the planner's "not configured" fallback covers it.
    pub fn held_back_for<'a>(
        &self,
        named: impl IntoIterator<Item = &'a str>,
    ) -> BTreeMap<String, String> {
        let mut out = self.held_back.clone();
        for name in named {
            if let Some(why) = self.not_accepted.get(name) {
                out.entry(name.to_string()).or_insert_with(|| why.clone());
            }
        }
        out
    }
}

/// `personal` is always named "personal", whatever a hand-built catalog
/// says (schema CHECK: the no-org catalog's name IS "personal"), so a
/// manifest entry's default catalog always matches it.
pub(crate) fn label_of(org_id: Option<i64>, name: &str) -> String {
    if org_id.is_none() {
        "personal".to_string()
    } else {
        name.to_string()
    }
}

/// Everything the store says about one host, read under one guard.
struct StorePhase {
    host_org: Option<i64>,
    /// The org's name, for layers that apply by organisation (`Layer::orgs`).
    host_org_name: Option<String>,
    personal_id: Option<i64>,
    configured: Vec<CatalogRow>,
    admitted: Vec<i64>,
    rows_by_catalog: BTreeMap<i64, Vec<HostLayerRow>>,
}

fn read_store(store: &Mutex<Store>, host_alias: &str) -> Result<StorePhase, IpcError> {
    let s = lock(store)?;
    let host_org = s.host_org(host_alias)?;
    let host_org_name = match host_org {
        Some(id) => s.get_org(id)?.map(|o| o.name),
        None => None,
    };
    let configured = s.list_catalogs()?;
    let admitted = s.host_admissions(host_alias)?;
    let mut personal_id = None;
    let mut rows_by_catalog = BTreeMap::new();
    for c in &configured {
        if c.org_id.is_none() {
            personal_id = Some(c.id);
        }
        rows_by_catalog.insert(c.id, s.get_host_layers_for(host_alias, c.id)?);
    }
    Ok(StorePhase {
        host_org,
        host_org_name,
        personal_id,
        configured,
        admitted,
        rows_by_catalog,
    })
}

fn compose_from(
    catalogs: &BTreeMap<i64, Catalog>,
    host_alias: &str,
    st: &StorePhase,
) -> Result<EffectiveSet, IpcError> {
    compose(
        catalogs,
        &st.configured,
        host_alias,
        st.host_org,
        st.host_org_name.as_deref(),
        &st.admitted,
        |cat: &Catalog| {
            // A hand-built catalog with id 0 stands for the personal one.
            let id = if cat.id == 0 && cat.org_id.is_none() {
                st.personal_id
            } else {
                Some(cat.id)
            };
            id.and_then(|id| st.rows_by_catalog.get(&id))
                .map(Vec::as_slice)
                .unwrap_or(&[])
        },
    )
}

/// Compute `host_alias`'s effective catalog against the live registry.
///
/// **Lock order.** All store reads (the host's org, the catalog rows, its
/// admissions and every catalog's layer rows for this host) happen under
/// ONE store guard that is dropped before the registry is taken; the
/// registry closure never touches the store. Store → registry is never
/// allowed (see `registry`).
pub fn effective_for_host(
    store: &Mutex<Store>,
    host_alias: &str,
) -> Result<EffectiveSet, IpcError> {
    let st = read_store(store, host_alias)?;
    registry::with_catalogs(|catalogs| compose_from(catalogs, host_alias, &st))
}

/// [`effective_for_host`] against a snapshot the caller already took
/// (`registry::snapshot`), so `plan_sync` scans and plans one view (R9).
pub fn effective_for_host_in(
    store: &Mutex<Store>,
    host_alias: &str,
    catalogs: &BTreeMap<i64, Catalog>,
) -> Result<EffectiveSet, IpcError> {
    let st = read_store(store, host_alias)?;
    compose_from(catalogs, host_alias, &st)
}

/// Record a catalog this host does not accept, in `not_accepted` only —
/// it never leaves the process (PF10), and `plan_sync` names it only when
/// the host's own manifest does (`held_back_for`). The one refusal whose
/// cause is known here gets it: the host holds an admission for the catalog,
/// so (R4: admissions are made on hosts with no org) it has joined an org
/// since (final review, Task 3 M1: that too is a tie to an org's catalog
/// `resolve_preview` must not reveal).
fn refuse(
    not_accepted: &mut BTreeMap<String, String>,
    admitted: &[i64],
    host_alias: &str,
    label: String,
    id: i64,
) {
    let reason = if admitted.contains(&id) {
        format!(
            "catalog {label} is not accepted by host {host_alias}: it was admitted while the \
             host had no org, and the host has since joined an org (an admission never \
             crosses orgs); its assets are kept, not removed"
        )
    } else {
        format!("catalog {label} is not accepted by this host; its assets are kept, not removed")
    };
    not_accepted.insert(label, reason);
}

/// The fixed `held_back` reason for a catalog that failed to load: it names
/// where the error is, never the error itself (final review I1).
pub(crate) fn failed_to_load_reason(label: &str) -> String {
    format!(
        "catalog {label} failed to load; its assets are kept, not removed (see `fleet-hub \
         catalog list`)"
    )
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
/// is already in `configured`, `host_org`, `admitted` and `rows_for`.
fn compose<'r>(
    catalogs: &BTreeMap<i64, Catalog>,
    configured: &[CatalogRow],
    host_alias: &str,
    host_org: Option<i64>,
    host_org_name: Option<&str>,
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
    let mut speaks_for: BTreeSet<String> = BTreeSet::new();
    let mut held_back: BTreeMap<String, String> = BTreeMap::new();
    let mut not_accepted: BTreeMap<String, String> = BTreeMap::new();
    let mut problem_held: BTreeMap<String, ProblemHolds> = BTreeMap::new();

    for cat in registry::in_order(catalogs) {
        let label = label_of(cat.org_id, &cat.name);
        let accepts = acceptance(host_org, cat.id, cat.org_id, admitted);
        if accepts == Acceptance::No {
            refuse(&mut not_accepted, admitted, host_alias, label, cat.id);
            continue;
        }
        // Final review I1: never the load error's text — it can carry the
        // clone URL, and `resolve_preview` (open to any client, per-host
        // tokens included) serializes `held_back`. The full error is on the
        // gated paths (`list_catalogs`, `fleet-hub catalog list`, the log).
        if cat.load_error.is_some() {
            held_back.insert(label.clone(), failed_to_load_reason(&label));
            continue;
        }
        let res = match resolve_rows_for(cat, host_alias, rows_for(cat), host_org_name) {
            Ok(res) => res,
            // Rulings R7: an org catalog that cannot resolve for this host is
            // held back for it; personal keeps failing the host (M1/M2).
            Err(e) if cat.org_id.is_some() => {
                held_back.insert(
                    label.clone(),
                    format!(
                        "catalog {label} cannot resolve for {host_alias}: {}; its assets are \
                         kept, not removed",
                        e.message
                    ),
                );
                continue;
            }
            Err(e) => return Err(e),
        };
        // Carry 2 (R24): the catalog still speaks — a broken file holds only
        // the key (or the kind directory) its problem path names, never the
        // whole catalog.
        let holds = ProblemHolds::from_problems(&cat.problems);
        if !holds.is_empty() {
            problem_held.insert(label.clone(), holds);
        }
        speaks_for.insert(label);
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
                        catalog: Some(cat.name.clone()),
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

    // A configured catalog the registry does not hold at all (not loaded
    // yet, or evicted): accepted → held back as not loaded; otherwise only
    // `not_accepted` (PF10), like a loaded one.
    for row in configured {
        let label = label_of(row.org_id, &row.name);
        if speaks_for.contains(&label)
            || held_back.contains_key(&label)
            || not_accepted.contains_key(&label)
        {
            continue;
        }
        if acceptance(host_org, row.id, row.org_id, admitted) == Acceptance::No {
            refuse(&mut not_accepted, admitted, host_alias, label, row.id);
        } else {
            held_back.insert(
                label.clone(),
                format!("catalog {label} is not loaded; its assets are kept, not removed"),
            );
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
                // A collision refuses a member from each of two or more
                // catalogs; no single catalog name applies.
                catalog: None,
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
        speaks_for,
        held_back,
        not_accepted,
        problem_held,
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
        assert_eq!(r.catalog.as_deref(), Some("personal"));
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
            assert_eq!(r.catalog, None, "a collision names no single catalog");
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

    // ---- Assets M3: admissions, speaks_for, held_back --------------------

    /// Spec, Testing: a no-org host receives an org asset only when admitted.
    /// Before the admission, `acme` is not in `held_back` at all (PF10: the
    /// host has no tie to it — `plan_sync` adds "not accepted" only for a
    /// catalog the host's own manifest names).
    #[test]
    fn a_no_org_host_takes_an_org_catalog_only_when_admitted() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, acme) = seeded_store();
        registry::install_personal(personal_cat(
            personal,
            vec![skill("a", "private")],
            LayerSet::default(),
        ))
        .unwrap();
        registry::install_for_test(acme_cat(acme, vec![skill("c", "private")])).unwrap();

        let before = effective_for_host(&store, "h").unwrap();
        assert_eq!(names(&before), vec!["a"]);
        assert!(
            !before.held_back.contains_key("acme"),
            "{:?}",
            before.held_back
        );
        assert!(
            before.not_accepted.contains_key("acme"),
            "{:?}",
            before.not_accepted
        );
        assert_eq!(before.speaks_for, BTreeSet::from(["personal".to_string()]));

        store.lock().unwrap().admit_host_catalog("h", acme).unwrap();
        let after = effective_for_host(&store, "h").unwrap();
        assert_eq!(names(&after), vec!["a", "c"]);
        assert_eq!(after.catalog.origin_of(Kind::Skill, "c").name, "acme");
        assert!(after.held_back.is_empty(), "{:?}", after.held_back);
        assert!(after.not_accepted.is_empty(), "{:?}", after.not_accepted);
        assert_eq!(
            after.speaks_for,
            BTreeSet::from(["acme".to_string(), "personal".to_string()])
        );
    }

    /// An admission left behind when the host joined another org never
    /// crosses orgs (R4). The admission row ties the host to `acme`, and it
    /// says what happened, so the reason names it (PF10: "org changed" only
    /// when actually known) — but only through `held_back_for`.
    #[test]
    fn an_admission_never_reaches_a_host_bound_to_an_org() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, acme) = seeded_store();
        {
            let s = store.lock().unwrap();
            s.admit_host_catalog("h", acme).unwrap();
            s.set_host_org("h", Some(ORG_11)).unwrap();
        }
        registry::install_personal(personal_cat(personal, vec![], LayerSet::default())).unwrap();
        registry::install_for_test(acme_cat(acme, vec![skill("c", "shared")])).unwrap();
        let e = effective_for_host(&store, "h").unwrap();
        assert!(!names(&e).contains(&"c"), "{:?}", names(&e));
        // Task 3 M1: never in the serialized `held_back` (`resolve_preview`
        // would name an org's catalog to any caller) — only through
        // `held_back_for`, when the host's own manifest names it.
        assert!(!e.held_back.contains_key("acme"), "{:?}", e.held_back);
        let wire = serde_json::to_string(&e).unwrap();
        assert!(!wire.contains("acme"), "{wire}");
        let held = e.held_back_for(["acme"]);
        let why = &held["acme"];
        assert!(why.contains("not accepted"), "{why}");
        assert!(why.contains("joined an org"), "{why}");
        assert!(!e.speaks_for.contains("acme"), "{:?}", e.speaks_for);
    }

    /// PF10: `resolve_preview` (any client may call it) copies `held_back`;
    /// an org-bound host's must never name another org's catalog it has no
    /// tie to — neither a loaded one nor a configured-but-unloaded one.
    #[test]
    fn an_org_bound_hosts_held_back_never_names_an_unrelated_orgs_catalog() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, acme) = seeded_store();
        {
            let s = store.lock().unwrap();
            s.set_host_org("h", Some(ORG_11)).unwrap();
            s.upsert_catalog("beta", "/b", None, Some(ORG_10)).unwrap();
        }
        registry::install_personal(personal_cat(
            personal,
            vec![skill("b", "shared")],
            LayerSet::default(),
        ))
        .unwrap();
        registry::install_for_test(acme_cat(acme, vec![skill("c", "shared")])).unwrap();
        let e = effective_for_host(&store, "h").unwrap();
        assert_eq!(names(&e), vec!["b"]);
        assert!(e.held_back.is_empty(), "{:?}", e.held_back);
        assert_eq!(e.speaks_for, BTreeSet::from(["personal".to_string()]));
        // Never serialized: `plan_sync` uses it only for the host's own
        // manifest entries.
        let wire = serde_json::to_string(&e).unwrap();
        assert!(!wire.contains("acme") && !wire.contains("beta"), "{wire}");
    }

    /// M2 carry 1: a configured org catalog that is not loaded, or failed to
    /// load, never speaks for the host — its entries are held back.
    #[test]
    fn a_configured_catalog_that_is_not_loaded_or_failed_is_held_back() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, _acme) = seeded_store();
        let beta = {
            let s = store.lock().unwrap();
            s.set_host_org("h", Some(ORG_10)).unwrap();
            s.upsert_catalog("beta", "/b", None, Some(ORG_10))
                .unwrap()
                .id
        };
        registry::install_personal(personal_cat(
            personal,
            vec![skill("b", "shared")],
            LayerSet::default(),
        ))
        .unwrap();
        registry::install_for_test(Catalog {
            id: beta,
            name: "beta".into(),
            org_id: Some(ORG_10),
            load_error: Some("boom".into()),
            ..Default::default()
        })
        .unwrap();
        let e = effective_for_host(&store, "h").unwrap();
        assert_eq!(names(&e), vec!["b"]);
        assert!(
            e.held_back["acme"].contains("is not loaded"),
            "{:?}",
            e.held_back
        );
        assert!(
            e.held_back["beta"].contains("failed to load;"),
            "{:?}",
            e.held_back
        );
        assert!(!e.held_back["beta"].contains("boom"), "{:?}", e.held_back);
        assert_eq!(e.speaks_for, BTreeSet::from(["personal".to_string()]));
    }

    /// Final review I1: `resolve_preview` is open to per-host tokens,
    /// readonly and org-bound clients, so a problem entry's load error —
    /// which can carry the clone URL and its userinfo — never reaches its
    /// serialized output; the fixed reason points at `catalog list`.
    #[test]
    fn resolve_preview_never_carries_a_load_errors_text() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, acme) = seeded_store();
        store
            .lock()
            .unwrap()
            .set_host_org("h", Some(ORG_10))
            .unwrap();
        registry::install_personal(personal_cat(personal, vec![], LayerSet::default())).unwrap();
        registry::install_for_test(Catalog {
            id: acme,
            name: "acme".into(),
            org_id: Some(ORG_10),
            load_error: Some("git clone -q https://u:secret@host/r.git /srv/acme: failed".into()),
            ..Default::default()
        })
        .unwrap();
        let res = crate::service::catalog::resolve_preview("h", &store).unwrap();
        let wire = serde_json::to_string(&res).unwrap();
        assert!(!wire.contains("secret"), "{wire}");
        assert!(!wire.contains("u:secret"), "{wire}");
        assert!(!wire.contains("r.git"), "{wire}");
        assert_eq!(
            res.held_back.get("acme").map(String::as_str),
            Some(failed_to_load_reason("acme").as_str())
        );
    }

    /// Rulings R7: an org catalog whose layers do not resolve for this host is
    /// held back for it, not fatal to the whole host.
    #[test]
    fn an_org_catalog_that_cannot_resolve_for_a_host_is_held_back_not_fatal() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, acme) = seeded_store();
        {
            let s = store.lock().unwrap();
            s.set_host_org("h", Some(ORG_10)).unwrap();
            s.set_host_layers_for("h", acme, Some("ghost"), &[])
                .unwrap();
        }
        registry::install_personal(personal_cat(
            personal,
            vec![skill("b", "shared")],
            LayerSet::default(),
        ))
        .unwrap();
        registry::install_for_test(acme_cat(acme, vec![skill("c", "private")])).unwrap();
        let e = effective_for_host(&store, "h").unwrap();
        assert_eq!(names(&e), vec!["b"]);
        assert!(
            e.held_back["acme"].contains("cannot resolve"),
            "{:?}",
            e.held_back
        );
    }

    /// Rulings R9: `effective_for_host_in` composes the map it is given, not
    /// the live registry, so plan and scan read one snapshot.
    #[test]
    fn effective_for_host_in_composes_the_snapshot_it_is_given() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, _acme) = seeded_store();
        registry::install_personal(personal_cat(
            personal,
            vec![skill("live", "private")],
            LayerSet::default(),
        ))
        .unwrap();
        let snapshot = BTreeMap::from([(
            personal,
            personal_cat(
                personal,
                vec![skill("snap", "private")],
                LayerSet::default(),
            ),
        )]);
        let e = effective_for_host_in(&store, "h", &snapshot).unwrap();
        assert_eq!(names(&e), vec!["snap"]);
    }

    /// PF10 at the plan: a catalog the host's manifest names but the host
    /// does not accept gets the generic reason; one the fleet has never
    /// heard of is left to the planner's "not configured" fallback.
    #[test]
    fn held_back_for_adds_the_generic_reason_only_for_named_catalogs() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, acme) = seeded_store();
        registry::install_personal(personal_cat(personal, vec![], LayerSet::default())).unwrap();
        registry::install_for_test(acme_cat(acme, vec![])).unwrap();
        let e = effective_for_host(&store, "h").unwrap();
        let held = e.held_back_for(["acme", "personal", "nowhere"]);
        assert_eq!(held.keys().collect::<Vec<_>>(), vec!["acme"]);
        assert!(
            held["acme"].contains("not accepted by this host"),
            "{held:?}"
        );
        assert!(e.held_back_for([]).is_empty());
    }

    /// Carry 2: a speaking catalog's load problems are reported per key, so
    /// the planner can keep a broken asset's copies (Rulings R24).
    #[test]
    fn a_speaking_catalogs_load_problems_are_held_per_key() {
        let _g = crate::service::catalog::lock_registry_for_test();
        let (store, personal, _acme) = seeded_store();
        let mut cat = personal_cat(personal, vec![skill("a", "private")], LayerSet::default());
        cat.problems.push(crate::service::catalog::model::Problem {
            path: "skills/broken/asset.yaml".into(),
            message: "bad yaml".into(),
        });
        registry::install_personal(cat).unwrap();
        let e = effective_for_host(&store, "h").unwrap();
        let holds = e.problem_held.get("personal").expect("held");
        assert_eq!(holds.reason(Kind::Skill, "broken"), Some("bad yaml"));
        assert_eq!(holds.reason(Kind::Skill, "a"), None);
    }
}
