//! Bridge between a host's stored layer assignment and the pure resolver.

use crate::ipc_error::codes;
use crate::ipc_error::lock;
use crate::ipc_error::IpcError;
use crate::service::catalog::repo::Catalog;
use crate::service::catalog::resolve::{resolve, Resolution};
use crate::store::Store;
use std::sync::Mutex;

/// Read `host_alias`'s stored assignment and resolve the catalog for it.
///
/// No assignment ⇒ the whole catalog (the pre-layers behaviour). An
/// assignment naming a layer the catalog does not define is an ERROR rather
/// than a silent fall-back to the whole catalog: syncing everything to a host
/// the user meant to restrict is the worse failure.
pub fn resolve_for_host(
    store: &Mutex<Store>,
    catalog: &Catalog,
    host_alias: &str,
) -> Result<Resolution, IpcError> {
    let rows = {
        let s = lock(store)?;
        s.get_host_layers(host_alias)?
    };
    if rows.is_empty() {
        return Ok(resolve(catalog, &[], &[]));
    }

    let role_name = rows
        .iter()
        .find(|r| r.axis == "role")
        .map(|r| r.layer_name.clone());
    let mut context_rows: Vec<_> = rows.iter().filter(|r| r.axis == "context").collect();
    context_rows.sort_by_key(|r| r.position);

    let role_chain = match &role_name {
        None => Vec::new(),
        Some(name) => catalog
            .layers
            .chain_for(name)
            .map_err(|e| IpcError::new(codes::E_INVALID, e))?,
    };

    let mut contexts = Vec::new();
    for r in context_rows {
        // A context may itself extend another context; flatten it too, and
        // skip a parent already contributed by an earlier context.
        for l in catalog
            .layers
            .chain_for(&r.layer_name)
            .map_err(|e| IpcError::new(codes::E_INVALID, e))?
        {
            if !contexts
                .iter()
                .any(|c: &&crate::service::catalog::layer::Layer| c.name == l.name)
            {
                contexts.push(l);
            }
        }
    }

    Ok(resolve(catalog, &role_chain, &contexts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::layer::{Layer, LayerSet};
    use crate::service::catalog::model::Asset;
    use crate::service::catalog::repo::Catalog;
    use crate::store::Store;
    use std::sync::Mutex;

    /// FKs are ON, so `local` must exist before an assignment references it.
    fn store_with_local() -> Store {
        let s = Store::open_in_memory().expect("open");
        s.upsert_host("local").expect("host");
        s
    }

    fn cat() -> Catalog {
        let (layers, errs) = LayerSet::from_layers(vec![
            Layer::from_yaml("kind: layer\nname: core\naxis: role\nmembers:\n  - skill/a\n")
                .unwrap(),
            Layer::from_yaml(
                "kind: layer\nname: workstation\naxis: role\nextends: core\nmembers:\n  - skill/b\n",
            )
            .unwrap(),
            Layer::from_yaml("kind: layer\nname: extra\naxis: context\nmembers:\n  - skill/c\n")
                .unwrap(),
        ]);
        assert!(errs.is_empty(), "{errs:?}");
        Catalog {
            assets: ["a", "b", "c"]
                .iter()
                .map(|n| {
                    Asset::from_yaml(None, &format!("kind: skill\nname: {n}\ndescription: d\n"))
                        .unwrap()
                })
                .collect(),
            layers,
            ..Default::default()
        }
    }

    fn names(r: &crate::service::catalog::resolve::Resolution) -> Vec<String> {
        let mut v: Vec<String> = r
            .catalog
            .assets
            .iter()
            .map(|a| a.header.name.clone())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn a_host_with_no_assignment_gets_the_whole_catalog() {
        let store = Mutex::new(store_with_local());
        let r = resolve_for_host(&store, &cat(), "local").unwrap();
        assert_eq!(names(&r), vec!["a", "b", "c"]);
    }

    #[test]
    fn a_role_pulls_in_its_extends_chain() {
        let store = Mutex::new(store_with_local());
        store
            .lock()
            .unwrap()
            .set_host_layers("local", Some("workstation"), &[])
            .unwrap();
        let r = resolve_for_host(&store, &cat(), "local").unwrap();
        assert_eq!(names(&r), vec!["a", "b"]);
    }

    #[test]
    fn contexts_add_on_top_of_the_role() {
        let store = Mutex::new(store_with_local());
        store
            .lock()
            .unwrap()
            .set_host_layers("local", Some("core"), &["extra"])
            .unwrap();
        let r = resolve_for_host(&store, &cat(), "local").unwrap();
        assert_eq!(names(&r), vec!["a", "c"]);
    }

    #[test]
    fn an_assignment_naming_an_unknown_layer_is_an_error_not_a_silent_full_catalog() {
        let store = Mutex::new(store_with_local());
        store
            .lock()
            .unwrap()
            .set_host_layers("local", Some("ghost"), &[])
            .unwrap();
        let err = resolve_for_host(&store, &cat(), "local").unwrap_err();
        assert!(err.message.contains("ghost"), "{}", err.message);
    }
}
