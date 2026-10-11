//! Bridge between a host's stored layer assignment and the pure resolver.

use crate::ipc_error::codes;
#[cfg(test)]
use crate::ipc_error::lock;
use crate::ipc_error::IpcError;
use crate::service::catalog::layer::Axis;
use crate::service::catalog::repo::Catalog;
use crate::service::catalog::resolve::{resolve, Resolution};
use crate::store::HostLayerRow;
#[cfg(test)]
use crate::store::Store;
#[cfg(test)]
use std::sync::Mutex;

/// Read `host_alias`'s stored assignment in `catalog` and resolve the
/// catalog for it.
///
/// Only the rows of `catalog.id` are read (Assets M2: assignments are per
/// catalog). A hand-built catalog with id 0 stands for the personal one.
/// Takes the store lock briefly; callers inside a registry closure are
/// fine (registry → store is the allowed order).
///
/// This is the single-catalog M1 compatibility API: `resolve_preview` and
/// `plan_sync` moved to `effective::effective_for_host` (Assets M2), which
/// resolves every catalog a host accepts, not just one. Nothing in
/// production calls this any more — it is `#[cfg(test)]` so a future
/// caller does not take the single-catalog path by mistake — and it
/// survives only as the independent baseline several tests check
/// `effective_for_host` against on the personal-only path, where the two
/// must agree exactly.
#[cfg(test)]
pub fn resolve_for_host(
    store: &Mutex<Store>,
    catalog: &Catalog,
    host_alias: &str,
) -> Result<Resolution, IpcError> {
    let rows = {
        let s = lock(store)?;
        let id = if catalog.id == 0 {
            s.personal_catalog()?.map(|c| c.id)
        } else {
            Some(catalog.id)
        };
        match id {
            Some(id) => s.get_host_layers_for(host_alias, id)?,
            None => Vec::new(),
        }
    };
    resolve_rows(catalog, host_alias, &rows)
}

/// Resolve `catalog` for `host_alias` from that host's assignment rows in
/// this catalog.
///
/// No rows ⇒ the whole catalog (the pre-layers behaviour). A row naming a
/// layer the catalog does not define is an ERROR rather than a silent
/// fall-back to the whole catalog: syncing everything to a host the user
/// meant to restrict is the worse failure.
pub fn resolve_rows(
    catalog: &Catalog,
    host_alias: &str,
    rows: &[HostLayerRow],
) -> Result<Resolution, IpcError> {
    resolve_rows_for(catalog, host_alias, rows, None)
}

/// [`resolve_rows`] for a host in the org named `host_org`: after its own
/// contexts, the host also takes every context layer that applies by that
/// organisation (`Layer::orgs`), in name order. Only a host that has rows
/// in this catalog takes them — with none it keeps the whole catalog, since
/// layer membership is a union and an org layer alone would narrow it.
pub fn resolve_rows_for(
    catalog: &Catalog,
    host_alias: &str,
    rows: &[HostLayerRow],
    host_org: Option<&str>,
) -> Result<Resolution, IpcError> {
    if rows.is_empty() {
        return Ok(resolve(catalog, &[], &[]));
    }

    // Any row whose axis is neither "role" nor "context" is an error, even
    // beside recognised rows. `set_host_layers` can never produce one (it
    // hard-codes both literals), but a direct-SQL path or a future third axis
    // must not be silently dropped: with every row bogus the host would fall
    // back to the whole catalog, and with some bogus it would resolve to less
    // than its assignment says.
    let mut bogus: Vec<&str> = rows
        .iter()
        .map(|r| r.axis.as_str())
        .filter(|a| *a != "role" && *a != "context")
        .collect();
    if !bogus.is_empty() {
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
    if let Some(org) = host_org {
        let by_org: Vec<&str> = catalog
            .layers
            .iter()
            .filter(|l| {
                l.axis == Axis::Context && l.orgs.iter().any(|o| o.eq_ignore_ascii_case(org))
            })
            .map(|l| l.name.as_str())
            .collect();
        for name in by_org {
            for l in catalog
                .layers
                .chain_for(name)
                .map_err(|e| IpcError::new(codes::E_INVALID, e))?
            {
                if !contexts.iter().any(|c| c.name == l.name) {
                    contexts.push(l);
                }
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
    /// `set_host_layers` now also needs a personal catalog to target.
    fn store_with_local() -> Store {
        let s = Store::open_in_memory().expect("open");
        s.upsert_host("local").expect("host");
        s.set_catalog_config("/p", None).expect("personal catalog");
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
            let personal = s.personal_catalog().unwrap().unwrap().id;
            s.conn_ref()
                .execute(
                    "INSERT INTO host_layers (host_alias, catalog_id, layer_name, axis, position, active) \
                     VALUES ('local', ?1, 'core', 'bogus', 0, 1)",
                    [personal],
                )
                .unwrap();
        }
        let err = resolve_for_host(&store, &cat(), "local").unwrap_err();
        assert!(err.message.contains("bogus"), "{}", err.message);
    }

    #[test]
    fn an_unrecognised_axis_beside_a_valid_role_is_still_an_error() {
        // One recognised row must not let a bogus sibling be silently
        // dropped: the host would resolve to less than its assignment says.
        let store = Mutex::new(store_with_local());
        {
            let s = store.lock().unwrap();
            s.set_host_layers("local", Some("core"), &[]).unwrap();
            let personal = s.personal_catalog().unwrap().unwrap().id;
            s.conn_ref()
                .execute(
                    "INSERT INTO host_layers (host_alias, catalog_id, layer_name, axis, position, active) \
                     VALUES ('local', ?1, 'extra', 'bogus', 0, 1)",
                    [personal],
                )
                .unwrap();
        }
        let err = resolve_for_host(&store, &cat(), "local").unwrap_err();
        assert!(err.message.contains("bogus"), "{}", err.message);
    }

    /// `resolve_for_host` reads only the rows of the catalog it resolves:
    /// a role this host holds in ANOTHER catalog must not restrict (or, as
    /// an unknown layer name here, fail) this one.
    #[test]
    fn rows_of_another_catalog_do_not_restrict_this_one() {
        let store = Mutex::new(store_with_local());
        {
            let s = store.lock().unwrap();
            s.conn_ref()
                .execute(
                    "INSERT INTO orgs (id, name, created_at) VALUES (10, 'acme', 0)",
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
            s.set_host_layers_for("local", acme, Some("ops"), &[])
                .unwrap();
        }
        // `cat()` has id 0: the personal catalog, which has no rows here.
        let r = resolve_for_host(&store, &cat(), "local").unwrap();
        assert!(!r.layered);
        assert_eq!(names(&r), vec!["a", "b", "c", "d", "e", "f"]);
    }

    fn row(layer: &str, axis: &str) -> HostLayerRow {
        HostLayerRow {
            host_alias: "local".into(),
            catalog_id: 0,
            layer_name: layer.into(),
            axis: axis.into(),
            position: 0,
            active: true,
        }
    }

    fn with_org_layer() -> Catalog {
        let mut c = cat();
        let mut layers: Vec<Layer> = c.layers.iter().cloned().collect();
        layers.push(
            Layer::from_yaml(
                "kind: layer\nname: papaya\naxis: context\norgs:\n  - Papaya\n\
                 extends: extra_base\nmembers:\n  - skill/f\n",
            )
            .unwrap(),
        );
        let (set, errs) = LayerSet::from_layers(layers);
        assert!(errs.is_empty(), "{errs:?}");
        c.layers = set;
        c
    }

    #[test]
    fn an_org_layer_adds_to_every_layered_host_of_that_org() {
        let c = with_org_layer();
        let rows = [row("core", "role")];
        let r = resolve_rows_for(&c, "local", &rows, Some("Papaya")).unwrap();
        // core's `a`, plus the org layer's `f` and its parent's `d`.
        assert_eq!(names(&r), vec!["a", "d", "f"]);
        // Another org, or no org, does not take it.
        let other = resolve_rows_for(&c, "local", &rows, Some("Acme")).unwrap();
        assert_eq!(names(&other), vec!["a"]);
        assert_eq!(names(&resolve_rows(&c, "local", &rows).unwrap()), vec!["a"]);
    }

    #[test]
    fn an_org_layer_never_narrows_a_host_that_takes_the_whole_catalog() {
        let c = with_org_layer();
        let r = resolve_rows_for(&c, "local", &[], Some("Papaya")).unwrap();
        assert!(!r.layered);
        assert_eq!(names(&r), vec!["a", "b", "c", "d", "e", "f"]);
    }
}
