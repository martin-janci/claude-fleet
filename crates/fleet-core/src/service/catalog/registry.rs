//! Loaded catalogs, by `catalogs.id`. Assets S1b: replaces the old single
//! process-global `Option<Catalog>` static. Only `load` (and tests) write
//! it.

use super::repo::Catalog;
use crate::ipc_error::{codes, IpcError};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};

static CATALOGS: LazyLock<RwLock<BTreeMap<i64, Catalog>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));

fn poisoned() -> IpcError {
    IpcError::new(codes::E_LOCK, "catalog lock poisoned")
}

pub fn install(cat: Catalog) -> Result<(), IpcError> {
    CATALOGS
        .write()
        .map_err(|_| poisoned())?
        .insert(cat.id, cat);
    Ok(())
}

pub fn get(id: i64) -> Result<Option<Catalog>, IpcError> {
    Ok(CATALOGS.read().map_err(|_| poisoned())?.get(&id).cloned())
}

/// The entry with `org_id.is_none()`. In real use there is ever only one:
/// the store's `catalogs` table enforces that invariant with a `CHECK`
/// constraint. Picking the *highest* id rather than the first found is a
/// second line of defence — a stray test-built catalog (`id: 0`, by
/// convention the one a hand-built `Catalog { .. }` gets when its author
/// never sets `id`) must never shadow a real, store-issued one (`id >= 1`)
/// left installed by a test that ran earlier in the same process.
fn find_personal(catalogs: &BTreeMap<i64, Catalog>) -> Option<&Catalog> {
    catalogs
        .values()
        .filter(|c| c.org_id.is_none())
        .max_by_key(|c| c.id)
}

pub fn personal() -> Result<Option<Catalog>, IpcError> {
    let guard = CATALOGS.read().map_err(|_| poisoned())?;
    Ok(find_personal(&guard).cloned())
}

pub fn with_personal<T>(f: impl FnOnce(&Catalog) -> Result<T, IpcError>) -> Result<T, IpcError> {
    let guard = CATALOGS.read().map_err(|_| poisoned())?;
    match find_personal(&guard) {
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

#[cfg(test)]
mod tests {
    use super::*;
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

    /// A leftover `id: 0` catalog (what a hand-built `Catalog { .. }` gets
    /// by default in tests that never set `id`) must never shadow a real,
    /// higher-id personal catalog installed afterward — nor the reverse if
    /// the stray `id: 0` entry is installed second. `personal`/`with_personal`
    /// always resolve to the highest-id `org_id: None` entry, regardless of
    /// install order.
    #[test]
    fn personal_prefers_the_highest_id_over_a_stray_zero() {
        let _l = crate::service::catalog::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear().unwrap();
        install(cat(0, "", None)).unwrap();
        install(cat(1, "personal", None)).unwrap();
        assert_eq!(personal().unwrap().unwrap().id, 1);
        assert_eq!(with_personal(|c| Ok(c.id)).unwrap(), 1);

        clear().unwrap();
        // Same two entries, installed in the opposite order.
        install(cat(1, "personal", None)).unwrap();
        install(cat(0, "", None)).unwrap();
        assert_eq!(personal().unwrap().unwrap().id, 1);
        assert_eq!(with_personal(|c| Ok(c.id)).unwrap(), 1);
        clear().unwrap();
    }
}
