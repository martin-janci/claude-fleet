//! Loaded catalogs, by `catalogs.id`. Assets S1b: replaces the old single
//! process-global `Option<Catalog>` static. Written by `load_catalog` (via
//! `load`/`load_all`/`ensure_fresh`), by `ensure_fresh`'s own eviction and
//! problem-entry installs, and by tests.
//!
//! **Lock order.** registry then store is allowed — that direction is
//! already load-bearing (`resolve_preview` → `sync::layers::resolve_for_host`
//! takes a `with_personal`/registry read lock and, from inside that
//! closure, briefly takes the store lock). Store then registry is never
//! allowed: read the store rows first, release the guard, then take the
//! registry. Taking the store lock and then, while still holding it,
//! calling into this module would be a lock-order inversion against the
//! `resolve_preview` direction — two callers on opposite orders can
//! deadlock each other. `load` used to do exactly that (store guard held
//! across `install`) and was fixed to release the store guard before
//! touching the registry — see its comment. When adding a new call site
//! here, check which order (if any) it nests in and make sure nothing
//! nests store-then-registry.

use super::repo::{Catalog, CatalogRef};
use crate::ipc_error::{codes, IpcError};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};

static CATALOGS: LazyLock<RwLock<BTreeMap<i64, Catalog>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));

fn poisoned() -> IpcError {
    IpcError::new(codes::E_LOCK, "catalog lock poisoned")
}

/// At most one entry may have `org_id: None` — the store's own `catalogs`
/// table enforces that with a `CHECK` constraint, and the registry is
/// supposed to mirror it. Not a hard invariant this module enforces on
/// every write (`install` is a plain "insert/replace by id" for callers,
/// tests included, that have their own reasons to bypass it), just a
/// canary: if it ever trips, something upstream of `install`/`install_personal`
/// let two no-org catalogs coexist.
fn debug_assert_at_most_one_personal(catalogs: &BTreeMap<i64, Catalog>) {
    debug_assert!(
        catalogs.values().filter(|c| c.org_id.is_none()).count() <= 1,
        "more than one org_id: None catalog in the registry"
    );
}

/// Insert/replace by `cat.id`. General-purpose: does not touch any other
/// entry, so it is the caller's job to keep "at most one `org_id: None`
/// entry" true if that matters to them — [`install_personal`] does that for
/// the one caller (`load`) that needs it.
pub fn install(cat: Catalog) -> Result<(), IpcError> {
    CATALOGS
        .write()
        .map_err(|_| poisoned())?
        .insert(cat.id, cat);
    Ok(())
}

/// Insert/replace `cat` by its id, then evict every OTHER `org_id: None`
/// entry. `cat` itself is exempt from that eviction regardless of its own
/// `org_id` — this only ever removes *other* ids. Used by `load` so a
/// stale personal catalog (a different id, left behind by whatever put it
/// there) can never coexist with the one just loaded. One write-lock
/// acquisition: insert and retain happen atomically, so no reader can
/// observe both entries at once.
pub fn install_personal(cat: Catalog) -> Result<(), IpcError> {
    let id = cat.id;
    let mut guard = CATALOGS.write().map_err(|_| poisoned())?;
    guard.insert(id, cat);
    guard.retain(|&k, c| k == id || c.org_id.is_some());
    Ok(())
}

pub fn get(id: i64) -> Result<Option<Catalog>, IpcError> {
    Ok(CATALOGS.read().map_err(|_| poisoned())?.get(&id).cloned())
}

pub fn personal() -> Result<Option<Catalog>, IpcError> {
    let guard = CATALOGS.read().map_err(|_| poisoned())?;
    debug_assert_at_most_one_personal(&guard);
    Ok(guard.values().find(|c| c.org_id.is_none()).cloned())
}

pub fn with_personal<T>(f: impl FnOnce(&Catalog) -> Result<T, IpcError>) -> Result<T, IpcError> {
    let guard = CATALOGS.read().map_err(|_| poisoned())?;
    debug_assert_at_most_one_personal(&guard);
    match guard.values().find(|c| c.org_id.is_none()) {
        Some(c) => f(c),
        None => Err(IpcError::new(
            super::E_CATALOG_NOT_CONFIGURED,
            "catalog not loaded; call catalog_load",
        )),
    }
}

pub fn clear() -> Result<(), IpcError> {
    CATALOGS.write().map_err(|_| poisoned())?.clear();
    Ok(())
}

/// Take the read lock and pass every loaded catalog, keyed by `catalogs.id`,
/// to `f`. The lock is held for the duration of `f`: per the module doc,
/// `f` must never take the store lock (that direction is the one that is
/// never allowed — registry → store is fine, store → registry never is).
pub fn with_catalogs<T>(
    f: impl FnOnce(&BTreeMap<i64, Catalog>) -> Result<T, IpcError>,
) -> Result<T, IpcError> {
    let guard = CATALOGS.read().map_err(|_| poisoned())?;
    f(&guard)
}

/// The loaded catalogs in composition order: personal (`org_id: None`)
/// first, then the rest by `name`. Shared by [`union_all`] and
/// `effective::effective_for_host`, so "which catalog wins / is named
/// first" means the same thing in both.
pub fn in_order(catalogs: &BTreeMap<i64, Catalog>) -> Vec<&Catalog> {
    let mut ordered: Vec<&Catalog> = catalogs.values().collect();
    ordered.sort_by(|a, b| match (a.org_id.is_none(), b.org_id.is_none()) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    ordered
}

/// [`union_all`] over a given map, for a caller that took a [`snapshot`].
/// `head` and `layers` are the personal catalog's, so a caller that only
/// ever read `head` before sees exactly what it saw before. `problems` is
/// the concatenation in composition order. `origin` is filled for every
/// asset in the result, so `Catalog::origin_of` always answers correctly on
/// it. `None` when no personal catalog is loaded — personal always loads
/// first, so a map without one is "not loaded", the same answer
/// `with_personal` gives.
pub fn union_of(catalogs: &BTreeMap<i64, Catalog>) -> Option<Catalog> {
    let personal = catalogs.values().find(|c| c.org_id.is_none())?;
    let ordered = in_order(catalogs);
    let mut merged = Catalog {
        id: 0,
        name: String::new(),
        head: personal.head.clone(),
        layers: personal.layers.clone(),
        ..Default::default()
    };
    let mut seen: std::collections::BTreeSet<(super::model::Kind, String)> =
        std::collections::BTreeSet::new();
    for cat in ordered {
        merged.problems.extend(cat.problems.iter().cloned());
        for asset in &cat.assets {
            let key = (asset.kind(), asset.header.name.clone());
            if !seen.insert(key) {
                continue;
            }
            merged.origin.insert(
                format!("{}/{}", asset.kind().as_str(), asset.header.name),
                CatalogRef {
                    id: cat.id,
                    name: cat.name.clone(),
                },
            );
            merged.assets.push(asset.clone());
        }
    }
    Some(merged)
}

/// Every loaded catalog merged into one borrowing view: personal (`org_id:
/// None`) first, then the rest ordered by `name`. On a `(kind, name)`
/// clash the first catalog in that order wins. `None` when no personal
/// catalog is loaded — personal always loads first, so a registry without
/// one is "not loaded", the same answer `with_personal` gives.
pub fn union_all() -> Result<Option<Catalog>, IpcError> {
    with_catalogs(|catalogs| Ok(union_of(catalogs)))
}

/// Every loaded catalog, cloned out of the lock: one consistent view for a
/// caller that reads the registry more than once (`plan_sync`, Rulings R9).
pub fn snapshot() -> Result<BTreeMap<i64, Catalog>, IpcError> {
    with_catalogs(|catalogs| Ok(catalogs.clone()))
}

/// Drop one catalog (`catalog remove`).
pub fn remove(id: i64) -> Result<(), IpcError> {
    CATALOGS.write().map_err(|_| poisoned())?.remove(&id);
    Ok(())
}

/// Drop every org catalog whose id the store no longer has (removed by
/// another process). The personal entry is `install_personal`'s to manage.
///
/// Fix round 1, item 2: checks under a READ lock first, and takes the write
/// lock only when something is actually stale. `ensure_fresh` calls this on
/// every pass, so a fleet with no stale org catalog — the common case, and
/// the whole fleet when there are no org catalogs at all (PF5) — must never
/// contend for the write lock just to find there is nothing to do.
pub fn evict_org_catalogs_not_in(
    configured: &std::collections::BTreeSet<i64>,
) -> Result<(), IpcError> {
    let stale = CATALOGS
        .read()
        .map_err(|_| poisoned())?
        .iter()
        .any(|(id, c)| c.org_id.is_some() && !configured.contains(id));
    if !stale {
        return Ok(());
    }
    CATALOGS
        .write()
        .map_err(|_| poisoned())?
        .retain(|id, c| c.org_id.is_none() || configured.contains(id));
    Ok(())
}

/// Which registry entry a store row names: personal (`org_id: None`) is
/// whichever entry in `m` also has `org_id: None` (tests may install it
/// under any id, 0 by convention), any other row by its own id. Fix round
/// 1, item 1 (PF14): shared by [`with_catalog_row`] and `ensure_fresh`'s own
/// freshness check, and Task 4's per-catalog reads will reuse it too.
pub(crate) fn entry_for<'a>(
    m: &'a BTreeMap<i64, Catalog>,
    row: &crate::store::CatalogRow,
) -> Option<&'a Catalog> {
    if row.org_id.is_none() {
        m.values().find(|c| c.org_id.is_none())
    } else {
        m.get(&row.id)
    }
}

/// Borrow the loaded catalog a store row names: personal or any other, via
/// [`entry_for`]. Not loaded, or a problem entry: `E_CATALOG_NOT_CONFIGURED`
/// saying which.
pub fn with_catalog_row<T>(
    row: &crate::store::CatalogRow,
    f: impl FnOnce(&Catalog) -> Result<T, IpcError>,
) -> Result<T, IpcError> {
    let guard = CATALOGS.read().map_err(|_| poisoned())?;
    if row.org_id.is_none() {
        debug_assert_at_most_one_personal(&guard);
    }
    match entry_for(&guard, row) {
        Some(c) if c.load_error.is_none() => f(c),
        Some(c) => Err(IpcError::new(
            super::E_CATALOG_NOT_CONFIGURED,
            format!(
                "catalog {} failed to load: {}",
                row.name,
                c.load_error.as_deref().unwrap_or_default()
            ),
        )),
        None if row.org_id.is_none() => Err(IpcError::new(
            super::E_CATALOG_NOT_CONFIGURED,
            "catalog not loaded; call catalog_load",
        )),
        None => Err(IpcError::new(
            super::E_CATALOG_NOT_CONFIGURED,
            format!(
                "catalog {} is not loaded; load it (catalog: {})",
                row.name, row.name
            ),
        )),
    }
}

/// `name=head` of every loaded catalog in composition order: what the scan
/// tick compares to decide that a catalog moved.
pub fn heads_key(catalogs: &BTreeMap<i64, Catalog>) -> String {
    in_order(catalogs)
        .iter()
        .map(|c| format!("{}={}", c.name, c.head))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Insert a non-personal (`org_id: Some(_)`) catalog under a chosen id, for
/// tests that need more than one catalog in the registry — `install_personal`
/// only ever manages the single `org_id: None` entry, so it cannot be used
/// to set up the "personal + an org catalog" fixtures `union_all`/
/// `with_catalogs` need.
#[cfg(test)]
pub fn install_for_test(cat: Catalog) -> Result<(), IpcError> {
    install(cat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::model::{Asset, Kind};
    use crate::service::catalog::repo::Catalog;

    fn cat(id: i64, name: &str, org: Option<i64>) -> Catalog {
        Catalog {
            id,
            name: name.into(),
            org_id: org,
            head: format!("h{id}"),
            ..Default::default()
        }
    }

    fn skill(name: &str) -> Asset {
        Asset::from_yaml(
            None,
            &format!("kind: skill\nname: {name}\ndescription: d\n"),
        )
        .unwrap()
    }

    #[test]
    fn personal_is_the_entry_without_an_org() {
        let _l = crate::service::catalog::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear().unwrap();
        install(cat(2, "papayapos", Some(9))).unwrap();
        assert!(personal().unwrap().is_none());
        install(cat(1, "personal", None)).unwrap();
        assert_eq!(personal().unwrap().unwrap().head, "h1");
        assert_eq!(get(2).unwrap().unwrap().name, "papayapos");
        clear().unwrap();
    }

    #[test]
    fn with_personal_reports_not_configured_when_empty() {
        let _l = crate::service::catalog::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear().unwrap();
        let err = with_personal(|_| Ok(())).unwrap_err();
        assert_eq!(err.code, crate::service::catalog::E_CATALOG_NOT_CONFIGURED);
    }

    #[test]
    fn install_replaces_by_id() {
        let _l = crate::service::catalog::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear().unwrap();
        install(cat(1, "personal", None)).unwrap();
        let mut c = cat(1, "personal", None);
        c.head = "new".into();
        install(c).unwrap();
        assert_eq!(personal().unwrap().unwrap().head, "new");
        clear().unwrap();
    }

    /// `install` alone does NOT enforce "at most one `org_id: None` entry":
    /// two coexist here, and `personal()`/`with_personal()` are left to
    /// pick whichever `.find()` hits first — that is exactly the gap
    /// `install_personal` closes for `load`'s own use.
    #[test]
    fn install_does_not_evict_other_org_none_entries() {
        let _l = crate::service::catalog::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear().unwrap();
        install(cat(0, "", None)).unwrap();
        install(cat(1, "personal", None)).unwrap();
        assert!(get(0).unwrap().is_some(), "install must not touch id 0");
        assert!(get(1).unwrap().is_some());
        clear().unwrap();
    }

    /// `install_personal` is what `load` uses: installing the current
    /// personal catalog evicts any OTHER `org_id: None` entry, regardless
    /// of which id it has or when it was installed — the registry can
    /// never hold two catalogs claiming to be "the" personal one. An
    /// `org_id: Some(..)` entry (a future org catalog) is untouched.
    #[test]
    fn install_personal_evicts_every_other_no_org_entry() {
        let _l = crate::service::catalog::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear().unwrap();
        install(cat(0, "", None)).unwrap();
        install(cat(9, "papayapos", Some(3))).unwrap();
        install_personal(cat(1, "personal", None)).unwrap();
        assert_eq!(personal().unwrap().unwrap().id, 1);
        assert!(get(0).unwrap().is_none(), "the stale id:0 entry is evicted");
        assert!(get(9).unwrap().is_some(), "an org catalog is left alone");

        // Re-loading under the SAME id must not evict itself.
        install_personal(cat(1, "personal", None)).unwrap();
        assert_eq!(personal().unwrap().unwrap().id, 1);
        clear().unwrap();
    }

    fn cat_with_skills(id: i64, name: &str, org: Option<i64>, skills: &[&str]) -> Catalog {
        Catalog {
            assets: skills.iter().map(|n| skill(n)).collect(),
            ..cat(id, name, org)
        }
    }

    /// `union_all` of `personal` (assets a, b) and `acme` (assets b, c)
    /// yields a, b (from personal) and c (from acme): on the `b` clash
    /// personal wins. `origin_of` on the merged catalog answers correctly
    /// for both the clashing and the non-clashing asset.
    #[test]
    fn union_all_merges_catalogs_personal_first_and_records_origin() {
        let _g = crate::service::catalog::lock_registry_for_test();
        install_personal(cat_with_skills(1, "personal", None, &["a", "b"])).unwrap();
        install_for_test(cat_with_skills(2, "acme", Some(9), &["b", "c"])).unwrap();

        let merged = union_all().unwrap().expect("something is loaded");
        let names: Vec<&str> = merged
            .assets
            .iter()
            .map(|a| a.header.name.as_str())
            .collect();
        assert_eq!(names, vec!["a", "b", "c"], "{names:?}");
        assert_eq!(merged.origin_of(Kind::Skill, "a").name, "personal");
        assert_eq!(merged.origin_of(Kind::Skill, "b").name, "personal");
        assert_eq!(merged.origin_of(Kind::Skill, "c").name, "acme");
        assert_eq!(merged.head, "h1", "head is the personal catalog's");
    }

    /// `with_catalogs` hands the caller the whole registry, keyed by id —
    /// both a personal and an org catalog must be visible.
    #[test]
    fn with_catalogs_sees_every_loaded_catalog() {
        let _g = crate::service::catalog::lock_registry_for_test();
        install_personal(cat(1, "personal", None)).unwrap();
        install_for_test(cat(2, "acme", Some(9))).unwrap();

        let ids: Vec<i64> = with_catalogs(|m| Ok(m.keys().copied().collect())).unwrap();
        assert_eq!(ids, vec![1, 2]);
    }

    /// An empty registry has nothing to union.
    #[test]
    fn union_all_is_none_when_nothing_is_loaded() {
        let _g = crate::service::catalog::lock_registry_for_test();
        assert!(union_all().unwrap().is_none());
    }

    /// Personal always loads first; a registry holding only org catalogs
    /// is "not loaded" as far as the union is concerned — the same answer
    /// `with_personal` gives — rather than a union headed by whichever org
    /// catalog sorts first.
    #[test]
    fn union_all_is_none_without_a_personal_catalog() {
        let _g = crate::service::catalog::lock_registry_for_test();
        install_for_test(cat_with_skills(2, "acme", Some(9), &["c"])).unwrap();
        assert!(union_all().unwrap().is_none());
    }

    /// Assets M3: the scan tick rescans when ANY loaded catalog's HEAD moves.
    #[test]
    fn heads_key_changes_when_any_catalogs_head_does() {
        let mut m = BTreeMap::new();
        m.insert(1, cat(1, "personal", None));
        m.insert(2, cat(2, "acme", Some(9)));
        let before = heads_key(&m);
        assert!(before.starts_with("personal=h1"), "{before}");
        m.get_mut(&2).unwrap().head = "moved".into();
        assert_ne!(heads_key(&m), before);
    }
}
