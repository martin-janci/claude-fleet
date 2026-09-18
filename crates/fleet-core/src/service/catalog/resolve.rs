//! Resolve (catalog, role chain, contexts) into the effective catalog.

use crate::service::catalog::layer::{split_key, Layer};
use crate::service::catalog::model::Asset;
use crate::service::catalog::repo::Catalog;
use serde::Serialize;
use std::collections::BTreeMap;

/// Where one effective asset came from.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Provenance {
    /// The layer that last introduced it.
    pub introduced_by: String,
    /// Every layer that changed its fields, in application order.
    pub overridden_by: Vec<String>,
}

/// The effective catalog plus why it looks the way it does.
#[derive(Debug, Clone, Serialize)]
pub struct Resolution {
    pub catalog: Catalog,
    /// `<kind>/<name>` → provenance, for the UI's "where did this come from".
    pub provenance: BTreeMap<String, Provenance>,
    /// `<kind>/<name>` → the layer that excluded it, for "why is this gone".
    pub excluded: BTreeMap<String, String>,
    /// Whether ANY layer applied. False only on the no-layering path, which
    /// every host without a `host_layers` row takes.
    pub layered: bool,
}

/// Deep-merge `over` into `base`: mappings recurse, everything else replaces.
fn merge_yaml(base: &mut serde_yaml::Value, over: &serde_yaml::Value) {
    match (base, over) {
        (serde_yaml::Value::Mapping(b), serde_yaml::Value::Mapping(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(existing) => merge_yaml(existing, v),
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

/// Apply one override mapping to an asset by round-tripping its header+spec
/// through YAML. `body` and `resources` are not part of `to_yaml`, so they
/// are carried over explicitly.
///
/// An override may change an asset's *fields*, never its identity: a
/// `name`-changing override is rejected (it would let one override key
/// silently rename an asset out from under its own `<kind>/<name>` key, and
/// potentially collide with another asset of the same name). The re-parsed
/// asset is also re-validated with the same `Asset::validate` the loader
/// uses, so an override cannot smuggle in a value `load_one` would have
/// rejected at load time.
fn apply_override(asset: &Asset, over: &serde_yaml::Value) -> Result<Asset, String> {
    let mut value: serde_yaml::Value =
        serde_yaml::from_str(&asset.to_yaml()).map_err(|e| e.to_string())?;
    merge_yaml(&mut value, over);
    let text = serde_yaml::to_string(&value).map_err(|e| e.to_string())?;
    let mut patched = Asset::from_yaml(Some(asset.kind()), &text)?;
    if patched.header.name != asset.header.name {
        return Err(format!(
            "an override may not change an asset's name ('{}' to '{}')",
            asset.header.name, patched.header.name
        ));
    }
    let problems = patched.validate();
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    patched.body = asset.body.clone();
    patched.resources = asset.resources.clone();
    Ok(patched)
}

/// Resolve the effective catalog for one host.
///
/// `role_chain` is ROOT FIRST and already validated by `LayerSet::chain_for`
/// (cycles, missing parents and crossed axes are caught there), so this
/// function cannot loop. `contexts` apply after the entire role chain, in
/// order. An empty `role_chain` AND empty `contexts` means "no layering":
/// the whole catalog is returned, which is the backward-compatible path.
pub fn resolve(catalog: &Catalog, role_chain: &[&Layer], contexts: &[&Layer]) -> Resolution {
    let ordered: Vec<&Layer> = role_chain.iter().chain(contexts.iter()).copied().collect();

    if ordered.is_empty() {
        return Resolution {
            catalog: Catalog {
                assets: catalog.assets.clone(),
                problems: catalog.problems.clone(),
                head: catalog.head.clone(),
                loaded_at: catalog.loaded_at,
                layers: Default::default(),
            },
            provenance: BTreeMap::new(),
            excluded: BTreeMap::new(),
            layered: false,
        };
    }

    let mut members: BTreeMap<String, Provenance> = BTreeMap::new();
    let mut excluded: BTreeMap<String, String> = BTreeMap::new();
    let mut overrides: BTreeMap<String, Vec<(String, serde_yaml::Value)>> = BTreeMap::new();

    for layer in &ordered {
        for key in &layer.members {
            // A member naming an asset the catalog does not have is skipped;
            // load_dir already reported it as a Problem.
            let Some((kind, name)) = split_key(key) else {
                continue;
            };
            if catalog.find(kind, &name).is_none() {
                continue;
            }
            excluded.remove(key);
            members.insert(
                key.clone(),
                Provenance {
                    introduced_by: layer.name.clone(),
                    overridden_by: Vec::new(),
                },
            );
        }
        for key in &layer.exclude {
            members.remove(key);
            excluded.insert(key.clone(), layer.name.clone());
        }
        for (key, value) in &layer.overrides {
            overrides
                .entry(key.clone())
                .or_default()
                .push((layer.name.clone(), value.clone()));
        }
    }

    let mut problems = catalog.problems.clone();
    let mut assets = Vec::new();
    let mut provenance = BTreeMap::new();

    for (key, mut prov) in members {
        let Some((kind, name)) = split_key(&key) else {
            continue;
        };
        let Some(base) = catalog.find(kind, &name) else {
            continue;
        };
        let mut asset = base.clone();
        for (layer_name, value) in overrides.get(&key).into_iter().flatten() {
            match apply_override(&asset, value) {
                Ok(patched) => {
                    asset = patched;
                    prov.overridden_by.push(layer_name.clone());
                }
                Err(message) => problems.push(crate::service::catalog::model::Problem {
                    path: format!("layers/{layer_name}.yaml"),
                    message: format!("override for '{key}' could not be applied: {message}"),
                }),
            }
        }
        assets.push(asset);
        provenance.insert(key, prov);
    }

    Resolution {
        catalog: Catalog {
            assets,
            problems,
            head: catalog.head.clone(),
            loaded_at: catalog.loaded_at,
            // Deliberately empty: a layer must never travel into the planner.
            layers: Default::default(),
        },
        provenance,
        excluded,
        layered: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::layer::{Layer, LayerSet};
    use crate::service::catalog::model::Asset;
    use crate::service::catalog::repo::Catalog;

    fn skill(name: &str) -> Asset {
        Asset::from_yaml(
            None,
            &format!("kind: skill\nname: {name}\ndescription: d\n"),
        )
        .unwrap()
    }

    fn catalog(names: &[&str]) -> Catalog {
        Catalog {
            assets: names.iter().map(|n| skill(n)).collect(),
            ..Default::default()
        }
    }

    fn lay(yaml: &str) -> Layer {
        Layer::from_yaml(yaml).unwrap()
    }

    #[test]
    fn resolution_reports_whether_any_layer_applied() {
        let cat = catalog(&["a"]);
        assert!(!resolve(&cat, &[], &[]).layered);
        let role = lay("kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n");
        assert!(resolve(&cat, &[&role], &[]).layered);
        let ctx = lay("kind: layer\nname: c\naxis: context\nmembers:\n  - skill/a\n");
        assert!(resolve(&cat, &[], &[&ctx]).layered);
        // A role with no members restricts the host to nothing — provenance
        // is empty, yet the host IS layered. `layered` must not be inferred
        // from what survived resolution.
        let empty = lay("kind: layer\nname: e\naxis: role\n");
        let r = resolve(&cat, &[&empty], &[]);
        assert!(r.provenance.is_empty());
        assert!(r.layered);
    }

    #[test]
    fn no_layers_resolves_to_the_whole_catalog() {
        let mut cat = catalog(&["a", "b"]);
        cat.problems.push(crate::service::catalog::model::Problem {
            path: "skills/broken/asset.yaml".into(),
            message: "boom".into(),
        });
        cat.head = "deadbeef".into();
        cat.loaded_at = 12345;

        let r = resolve(&cat, &[], &[]);

        // Not just the count: the exact two assets, unchanged, in order —
        // this is the pure test of the branch's backward-compat guarantee,
        // and a `resolve` that returned two different assets of the same
        // length must fail it.
        let names: Vec<&str> = r
            .catalog
            .assets
            .iter()
            .map(|a| a.header.name.as_str())
            .collect();
        assert_eq!(names, vec!["a", "b"]);
        assert_eq!(r.provenance, BTreeMap::new());
        assert_eq!(r.excluded, BTreeMap::new());
        // Everything else about the catalog passes through unchanged too.
        assert_eq!(r.catalog.problems, cat.problems);
        assert_eq!(r.catalog.head, cat.head);
        assert_eq!(r.catalog.loaded_at, cat.loaded_at);
    }

    #[test]
    fn members_add_and_exclude_removes_along_the_chain() {
        let cat = catalog(&["a", "b", "c"]);
        let root = lay("kind: layer\nname: root\naxis: role\nmembers:\n  - skill/a\n  - skill/b\n");
        let leaf = lay("kind: layer\nname: leaf\naxis: role\nextends: root\n\
             members:\n  - skill/c\nexclude:\n  - skill/b\n");
        let r = resolve(&cat, &[&root, &leaf], &[]);
        let mut got: Vec<&str> = r
            .catalog
            .assets
            .iter()
            .map(|a| a.header.name.as_str())
            .collect();
        got.sort();
        assert_eq!(got, vec!["a", "c"]);
        assert_eq!(r.excluded.get("skill/b").map(String::as_str), Some("leaf"));
        assert_eq!(r.provenance["skill/a"].introduced_by, "root");
        assert_eq!(r.provenance["skill/c"].introduced_by, "leaf");
    }

    #[test]
    fn a_later_layer_can_re_add_what_an_earlier_one_excluded() {
        let cat = catalog(&["a"]);
        let role = lay("kind: layer\nname: r\naxis: role\nexclude:\n  - skill/a\n");
        let ctx = lay("kind: layer\nname: c\naxis: context\nmembers:\n  - skill/a\n");
        let r = resolve(&cat, &[&role], &[&ctx]);
        assert_eq!(r.catalog.assets.len(), 1);
        assert!(!r.excluded.contains_key("skill/a"));
        assert_eq!(r.provenance["skill/a"].introduced_by, "c");
    }

    #[test]
    fn contexts_apply_after_the_whole_role_chain_in_order() {
        let cat = catalog(&["a"]);
        let role = lay("kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n\
             overrides:\n  skill/a:\n    version: \"role\"\n");
        let c1 = lay(
            "kind: layer\nname: c1\naxis: context\noverrides:\n  skill/a:\n    version: \"c1\"\n",
        );
        let c2 = lay(
            "kind: layer\nname: c2\naxis: context\noverrides:\n  skill/a:\n    version: \"c2\"\n",
        );
        let r = resolve(&cat, &[&role], &[&c1, &c2]);
        assert_eq!(r.catalog.assets[0].header.version, "c2");
        assert_eq!(r.provenance["skill/a"].overridden_by, vec!["r", "c1", "c2"]);
    }

    #[test]
    fn overrides_deep_merge_rather_than_replacing_the_header() {
        let cat = Catalog {
            assets: vec![Asset::from_yaml(
                None,
                "kind: skill\nname: a\ndescription: keep me\ntags: [x]\n\
                 targets:\n  claude:\n    enabled: true\n",
            )
            .unwrap()],
            ..Default::default()
        };
        let role = lay("kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n\
             overrides:\n  skill/a:\n    targets:\n      claude:\n        enabled: false\n");
        let r = resolve(&cat, &[&role], &[]);
        let a = &r.catalog.assets[0];
        // The override touched targets.claude.enabled ONLY.
        assert_eq!(a.header.description, "keep me");
        assert_eq!(a.header.tags, vec!["x".to_string()]);
        assert!(!a.header.targets["claude"].enabled);
    }

    #[test]
    fn body_and_resources_survive_an_override() {
        let mut asset = skill("a");
        asset.body = "BODY".to_string();
        let cat = Catalog {
            assets: vec![asset],
            ..Default::default()
        };
        let role = lay("kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n\
             overrides:\n  skill/a:\n    version: \"9\"\n");
        let r = resolve(&cat, &[&role], &[]);
        assert_eq!(r.catalog.assets[0].body, "BODY");
        assert_eq!(r.catalog.assets[0].header.version, "9");
    }

    #[test]
    fn an_override_for_a_non_member_is_ignored() {
        let cat = catalog(&["a", "b"]);
        let role = lay("kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n\
             overrides:\n  skill/b:\n    version: \"9\"\n");
        let r = resolve(&cat, &[&role], &[]);
        assert_eq!(r.catalog.assets.len(), 1);
        assert_eq!(r.catalog.assets[0].header.name, "a");
    }

    #[test]
    fn a_member_naming_an_unknown_asset_is_skipped_and_the_rest_resolve() {
        let cat = catalog(&["a"]);
        let role =
            lay("kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n  - skill/ghost\n");
        let r = resolve(&cat, &[&role], &[]);
        assert_eq!(r.catalog.assets.len(), 1);
        assert!(!r.provenance.contains_key("skill/ghost"));
    }

    #[test]
    fn the_resolved_catalog_keeps_head_and_loaded_at() {
        let mut cat = catalog(&["a"]);
        cat.head = "deadbeef".to_string();
        cat.loaded_at = 42;
        let role = lay("kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n");
        let r = resolve(&cat, &[&role], &[]);
        assert_eq!(r.catalog.head, "deadbeef");
        assert_eq!(r.catalog.loaded_at, 42);
    }

    #[test]
    fn layer_set_is_not_carried_into_the_resolved_catalog() {
        // Guards against a layer ever reaching compute_host_plan. The input
        // catalog must actually carry a layer, or this proves nothing.
        let mut cat = catalog(&["a"]);
        let role = lay("kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n");
        cat.layers = LayerSet::from_layers(vec![role.clone()]).0;
        assert_ne!(cat.layers, LayerSet::default());
        let r = resolve(&cat, &[&role], &[]);
        assert_eq!(r.catalog.layers, LayerSet::default());
    }

    #[test]
    fn the_no_layering_fast_path_also_strips_the_layer_set() {
        // Same guard on the empty-chain path: every existing installation
        // takes this path, so it must never leak a populated LayerSet
        // through untouched.
        let mut cat = catalog(&["a"]);
        let role = lay("kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n");
        cat.layers = LayerSet::from_layers(vec![role]).0;
        assert_ne!(cat.layers, LayerSet::default());
        let r = resolve(&cat, &[], &[]);
        assert_eq!(r.catalog.layers, LayerSet::default());
    }

    #[test]
    fn an_override_that_changes_the_name_is_refused_and_recorded_as_a_problem() {
        let cat = catalog(&["a"]);
        let role = lay("kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n\
             overrides:\n  skill/a:\n    name: b\n");
        let r = resolve(&cat, &[&role], &[]);
        // The asset survives unpatched, still named 'a' and still the only
        // asset in the resolved catalog — the override never took effect.
        assert_eq!(r.catalog.assets.len(), 1);
        assert_eq!(r.catalog.assets[0].header.name, "a");
        assert!(r.provenance["skill/a"].overridden_by.is_empty());
        assert!(
            r.catalog
                .problems
                .iter()
                .any(|p| p.message.contains("name")),
            "{:?}",
            r.catalog.problems
        );
    }

    #[test]
    fn an_override_producing_an_invalid_asset_is_refused_and_recorded_as_a_problem() {
        let cat = catalog(&["a"]);
        let role = lay("kind: layer\nname: r\naxis: role\nmembers:\n  - skill/a\n\
             overrides:\n  skill/a:\n    description: \"\"\n");
        let r = resolve(&cat, &[&role], &[]);
        // `Asset::validate` requires a non-empty description; the override
        // is refused and the asset keeps its original, valid description.
        assert_eq!(r.catalog.assets[0].header.description, "d");
        assert!(r.provenance["skill/a"].overridden_by.is_empty());
        assert!(
            r.catalog
                .problems
                .iter()
                .any(|p| p.message.contains("description")),
            "{:?}",
            r.catalog.problems
        );
    }
}
