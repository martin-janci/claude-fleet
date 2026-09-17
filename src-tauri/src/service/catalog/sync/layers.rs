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

    // `rows` is non-empty but neither selector matched anything: every row
    // carries an axis that is neither "role" nor "context". `set_host_layers`
    // can never produce this (it hard-codes both literals), but a direct-SQL
    // path or a future third axis must not silently fall back to the whole
    // catalog — that is the exact failure the spec forbids.
    if role_name.is_none() && context_rows.is_empty() {
        let mut bogus: Vec<&str> = rows.iter().map(|r| r.axis.as_str()).collect();
        bogus.sort();
        bogus.dedup();
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "host '{host_alias}' has layer assignment row(s) with an unrecognised axis: {}",
                bogus.join(", ")
            ),
        ));
    }

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
            // A shared parent plus two contexts that both extend it, so a
            // test can tell "flattened" from "leaf only": `extra_base` is
            // never assigned directly, only reachable through its children.
            Layer::from_yaml(
                "kind: layer\nname: extra_base\naxis: context\nmembers:\n  - skill/d\n\
                 overrides:\n  skill/a:\n    version: \"base\"\n",
            )
            .unwrap(),
            Layer::from_yaml(
                "kind: layer\nname: extra_child\naxis: context\nextends: extra_base\n\
                 members:\n  - skill/e\noverrides:\n  skill/a:\n    version: \"child\"\n",
            )
            .unwrap(),
            Layer::from_yaml(
                "kind: layer\nname: extra_child2\naxis: context\nextends: extra_base\n\
                 members:\n  - skill/f\n",
            )
            .unwrap(),
        ]);
        assert!(errs.is_empty(), "{errs:?}");
        Catalog {
            assets: ["a", "b", "c", "d", "e", "f"]
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
        assert_eq!(names(&r), vec!["a", "b", "c", "d", "e", "f"]);
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

    /// Two assigned contexts (`extra_child`, `extra_child2`) share one
    /// unassigned parent (`extra_base`). This discriminates a real flatten
    /// from a version that just pushes the assigned layer itself: under that
    /// broken version `extra_base`'s member ("d") and override never appear
    /// at all, since `extra_base` was never named directly.
    #[test]
    fn context_extends_chain_is_flattened_parent_first_and_deduped() {
        let store = Mutex::new(store_with_local());
        store
            .lock()
            .unwrap()
            .set_host_layers("local", Some("core"), &["extra_child", "extra_child2"])
            .unwrap();
        let r = resolve_for_host(&store, &cat(), "local").unwrap();

        // The parent's own member ("d") is pulled in even though only its
        // children were assigned, alongside both children's members.
        assert_eq!(names(&r), vec!["a", "d", "e", "f"]);

        // `extra_base` (the shared parent) is applied BEFORE `extra_child`
        // — the final value is `extra_child`'s override, proving order —
        // and appears EXACTLY ONCE in `overridden_by` even though BOTH
        // assigned contexts extend it. A missing flatten would drop "base"
        // entirely from this list; a flatten without dedup would repeat it.
        let a = r
            .catalog
            .assets
            .iter()
            .find(|a| a.header.name == "a")
            .unwrap();
        assert_eq!(a.header.version, "child");
        assert_eq!(
            r.provenance["skill/a"].overridden_by,
            vec!["extra_base".to_string(), "extra_child".to_string()]
        );
    }

    /// `set_host_layers` can only ever write `axis` as `'role'` or
    /// `'context'` (both literals are hard-coded), so this row is reachable
    /// only by bypassing it with direct SQL — exactly what a future
    /// migration, a third axis, or a manual `UPDATE` could do. Without a
    /// guard, `role_name` and `context_rows` both come back empty and
    /// `resolve_for_host` would silently return the WHOLE catalog: the
    /// exact failure the spec forbids for an unknown layer name, now via an
    /// unknown axis instead.
    #[test]
    fn an_unrecognised_axis_is_an_error_not_a_silent_full_catalog() {
        let store = Mutex::new(store_with_local());
        {
            let s = store.lock().unwrap();
            s.conn_ref()
                .execute(
                    "INSERT INTO host_layers (host_alias, layer_name, axis, position, active) \
                     VALUES ('local', 'core', 'bogus', 0, 1)",
                    [],
                )
                .unwrap();
        }
        let err = resolve_for_host(&store, &cat(), "local").unwrap_err();
        assert!(err.message.contains("bogus"), "{}", err.message);
    }
}
