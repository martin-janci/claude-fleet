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
                name: if i == 0 {
                    "core".to_string()
                } else {
                    hosts.join("-")
                },
                axis: "role".to_string(),
                hosts,
                members,
            }
        })
        .collect();

    LayerProposal { layers, singletons }
}

/// Read the last scan's inventory and propose a split. Read-only: it returns
/// a proposal and writes nothing, neither to the catalog nor to the DB.
pub fn propose_layers(store: &Mutex<Store>) -> Result<LayerProposal, IpcError> {
    let rows = {
        let s = lock(store)?;
        s.list_inventory()?
    };
    let installed: Vec<(String, String)> = rows
        .iter()
        // One inventory row per harness; count each asset once.
        .filter(|r| r.harness == "claude")
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
}
