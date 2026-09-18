//! Propose an initial layer split from what is already installed.

use crate::ipc_error::lock;
use crate::ipc_error::IpcError;
use crate::store::Store;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProposedLayer {
    pub name: String,
    /// Always `"role"`: the proposal cannot know what is a context.
    pub axis: String,
    /// The hosts whose installed set this layer was derived from.
    pub hosts: Vec<String>,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProposedSingleton {
    pub key: String,
    pub host: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LayerProposal {
    pub layers: Vec<ProposedLayer>,
    /// Assets on exactly one host: a context, or a mistake. The user decides.
    pub singletons: Vec<ProposedSingleton>,
}

/// Group `(host, key)` pairs by each key's EXACT host-set signature.
///
/// Grouping by signature rather than intersecting across all hosts is what
/// makes this usable: on the measured fleet a strict all-host intersection
/// yields a `core` of only 7 skills, because one near-empty outlier host drags
/// it down. By signature, the big shared group wins and the outlier gets its
/// own layer instead of impoverishing everyone else.
pub fn propose_from_installed(installed: &[(String, String)]) -> LayerProposal {
    let mut hosts_by_key: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (host, key) in installed {
        let entry = hosts_by_key.entry(key.as_str()).or_default();
        if !entry.contains(&host.as_str()) {
            entry.push(host.as_str());
        }
    }
    for hosts in hosts_by_key.values_mut() {
        hosts.sort();
    }

    let mut singletons = Vec::new();
    let mut groups: BTreeMap<Vec<String>, Vec<String>> = BTreeMap::new();
    for (key, hosts) in hosts_by_key {
        if hosts.len() < 2 {
            if let Some(h) = hosts.first() {
                singletons.push(ProposedSingleton {
                    key: key.to_string(),
                    host: h.to_string(),
                });
            }
            continue;
        }
        groups
            .entry(hosts.iter().map(|h| h.to_string()).collect())
            .or_default()
            .push(key.to_string());
    }

    // Largest group first; ties broken by host-set so the output is stable.
    let mut ordered: Vec<(Vec<String>, Vec<String>)> = groups.into_iter().collect();
    ordered.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));

    let layers = ordered
        .into_iter()
        .enumerate()
        .map(|(i, (hosts, mut members))| {
            members.sort();
            ProposedLayer {
                // Positional, not derived from `hosts`: host aliases may
                // themselves contain `-` (e.g. `claude-fleet-oci`), so
                // joining them with `-` can make two distinct host-sets
                // collide on the same name. These are starting points the
                // user renames anyway; `hosts` on the layer already says
                // which hosts a group covers.
                name: if i == 0 {
                    "core".to_string()
                } else {
                    format!("group-{}", i + 1)
                },
                axis: "role".to_string(),
                hosts,
                members,
            }
        })
        .collect();

    LayerProposal { layers, singletons }
}

/// Whether an inventory row's `state` means the asset is actually present on
/// the host, as opposed to merely known about it.
///
/// `Store::list_inventory()` has no `WHERE` clause: it returns the full
/// catalog x host x harness cross product, including `missing` (the catalog
/// defines it, the host does not have it) and `unsupported` (the harness
/// cannot render it there). Counting those as "installed" would make every
/// host appear to share nearly the whole catalog's key-set, which is the
/// opposite of what this tool is for.
///
/// `unmanaged` counts: it is not optional. The bootstrap scenario this tool
/// exists for is a fleet with an *empty* catalog and hundreds of assets
/// already sitting on hosts — in that state every real asset is
/// `unmanaged`, so excluding it would make `propose_layers` return nothing
/// precisely when it is needed. `orphan` counts too: the host still has it,
/// even though the catalog has since dropped it.
fn is_installed(state: &str) -> bool {
    matches!(state, "in_sync" | "drifted" | "unmanaged" | "orphan")
}

/// Read the last scan's inventory and propose a split from what is actually
/// present on each host (`in_sync`, `drifted`, `unmanaged`, or `orphan` —
/// see `is_installed`; `missing` and `unsupported` rows are excluded).
/// Read-only: it returns a proposal and writes nothing, neither to the
/// catalog nor to the DB.
pub fn propose_layers(store: &Mutex<Store>) -> Result<LayerProposal, IpcError> {
    let rows = {
        let s = lock(store)?;
        s.list_inventory()?
    };
    let installed: Vec<(String, String)> = rows
        .iter()
        // One inventory row per harness; count each asset once.
        .filter(|r| r.harness == "claude" && is_installed(&r.state))
        .map(|r| (r.host_alias.clone(), format!("{}/{}", r.kind, r.name)))
        .collect();
    Ok(propose_from_installed(&installed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(host: &str, key: &str) -> (String, String) {
        (host.to_string(), key.to_string())
    }

    #[test]
    fn the_largest_host_group_becomes_core_and_an_outlier_gets_its_own_layer() {
        // Mirrors the measured fleet: four hosts share most assets, one
        // near-empty outlier shares only a little. A strict all-host
        // INTERSECTION would collapse core to just "shared"; grouping by
        // host-set signature must not.
        let mut installed = Vec::new();
        for host in ["local", "oci", "trn", "mefistos"] {
            for k in ["skill/a", "skill/b", "skill/c"] {
                installed.push(item(host, k));
            }
        }
        for host in ["local", "oci", "trn", "mefistos", "htz"] {
            installed.push(item(host, "skill/shared"));
        }
        installed.push(item("htz", "skill/only-htz"));

        let p = propose_from_installed(&installed);

        let core = p.layers.iter().find(|l| l.name == "core").unwrap();
        let mut members = core.members.clone();
        members.sort();
        assert_eq!(members, vec!["skill/a", "skill/b", "skill/c"]);
        assert_eq!(core.hosts.len(), 4);

        // The all-host group still becomes its own layer, not part of core.
        assert!(p
            .layers
            .iter()
            .any(|l| l.members == vec!["skill/shared".to_string()] && l.hosts.len() == 5));

        // Single-host assets are triage, not layers.
        assert!(p
            .layers
            .iter()
            .all(|l| !l.members.contains(&"skill/only-htz".to_string())));
        assert_eq!(p.singletons.len(), 1);
        assert_eq!(p.singletons[0].key, "skill/only-htz");
        assert_eq!(p.singletons[0].host, "htz");
    }

    #[test]
    fn every_proposed_layer_is_a_role_and_names_are_unique() {
        let installed = vec![
            item("a", "skill/x"),
            item("b", "skill/x"),
            item("a", "skill/y"),
            item("c", "skill/y"),
        ];
        let p = propose_from_installed(&installed);
        assert!(p.layers.iter().all(|l| l.axis == "role"));
        let mut names: Vec<&str> = p.layers.iter().map(|l| l.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), p.layers.len());
    }

    #[test]
    fn an_empty_fleet_proposes_nothing() {
        let p = propose_from_installed(&[]);
        assert!(p.layers.is_empty() && p.singletons.is_empty());
    }

    /// Two distinct host-set signatures, `{"a-b", "c"}` and `{"a", "b-c"}`,
    /// that a naive `hosts.join("-")` would both render as `"a-b-c"`. Real
    /// fleet aliases contain hyphens (e.g. `claude-fleet-oci`), so this is
    /// not hypothetical. A third, clearly-largest group makes `core`
    /// deterministic and leaves the ambiguous pair to be named positionally.
    #[test]
    fn layer_names_do_not_collide_when_host_aliases_contain_hyphens() {
        let mut installed = vec![
            item("m", "skill/core1"),
            item("n", "skill/core1"),
            item("m", "skill/core2"),
            item("n", "skill/core2"),
            item("m", "skill/core3"),
            item("n", "skill/core3"),
        ];
        installed.push(item("a-b", "skill/x"));
        installed.push(item("c", "skill/x"));
        installed.push(item("a", "skill/y"));
        installed.push(item("b-c", "skill/y"));

        let p = propose_from_installed(&installed);
        assert_eq!(p.layers.len(), 3);

        let mut names: Vec<&str> = p.layers.iter().map(|l| l.name.as_str()).collect();
        names.sort();
        let mut deduped = names.clone();
        deduped.dedup();
        assert_eq!(
            deduped.len(),
            names.len(),
            "layer names collided: {names:?}"
        );

        let core = p.layers.iter().find(|l| l.name == "core").unwrap();
        assert_eq!(core.hosts, vec!["m".to_string(), "n".to_string()]);
    }

    /// `propose_layers` must read from a real `Store`, so this uses an
    /// in-memory one (rather than a pure predicate unit test) to exercise
    /// the actual `list_inventory` -> filter -> `propose_from_installed`
    /// pipeline end to end. `asset_inventory` has no foreign key to
    /// `hosts` (checked: `crates/fleet-core/migrations/030_asset_catalog.sql`
    /// has no `REFERENCES hosts`, and the existing `replace_host_inventory`
    /// store tests write rows without an `upsert_host` call), so this test
    /// follows that existing pattern rather than adding one.
    #[test]
    fn propose_layers_only_counts_present_states() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let row = |name: &str, state: &str| crate::store::AssetInventoryRow {
            host_alias: "h".into(),
            harness: "claude".into(),
            kind: "skill".into(),
            name: name.into(),
            state: state.into(),
            catalog_hash: None,
            host_hash: None,
            scanned_at: 1,
            managed: false,
        };
        lock(&store)
            .unwrap()
            .replace_host_inventory(
                "h",
                "claude",
                &[
                    row("in-sync", "in_sync"),
                    row("drifted", "drifted"),
                    row("missing", "missing"),
                    row("unmanaged", "unmanaged"),
                    row("unsupported", "unsupported"),
                    row("orphan", "orphan"),
                ],
            )
            .unwrap();

        let p = propose_layers(&store).unwrap();

        // Single host: every surviving key is a singleton, never a layer.
        assert!(p.layers.is_empty());
        let mut keys: Vec<&str> = p.singletons.iter().map(|s| s.key.as_str()).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "skill/drifted",
                "skill/in-sync",
                "skill/orphan",
                "skill/unmanaged",
            ]
        );
    }
}
