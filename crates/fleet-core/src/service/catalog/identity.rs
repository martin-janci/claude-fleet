//! Assets S1a: collapse per-host inventory rows into one identity per
//! (kind, name), and classify each by rules. Pure: no store, no I/O.

use crate::store::AssetInventoryRow;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityClass {
    Normal,
    FleetInternal,
    HarnessInternal,
    NeedsPerson,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityHost {
    pub host_alias: String,
    pub harness: String,
    pub host_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetIdentity {
    pub kind: String,
    pub name: String,
    pub hosts: Vec<IdentityHost>,
    /// Sorted distinct host aliases joined by ',' — the host-set signature.
    pub signature: String,
    /// Distinct known content hashes across copies (0 when none is known).
    pub variants: usize,
    pub class: IdentityClass,
    /// Why a `needs_person` identity needs one.
    pub reason: Option<String>,
}

/// Group `unmanaged` rows into one identity per (kind, name) and classify
/// each. Orphans and every other state stay out — they are not asset
/// identities to reconcile, they are `AssetListing::unmanaged` entries as
/// today. Output is sorted by kind, then name (via the `BTreeMap` key).
pub fn group_identities(rows: &[AssetInventoryRow]) -> Vec<AssetIdentity> {
    let mut by_key: BTreeMap<(String, String), Vec<&AssetInventoryRow>> = BTreeMap::new();
    for r in rows.iter().filter(|r| r.state == "unmanaged") {
        by_key
            .entry((r.kind.clone(), r.name.clone()))
            .or_default()
            .push(r);
    }
    by_key
        .into_iter()
        .map(|((kind, name), copies)| {
            let mut aliases: Vec<&str> = copies.iter().map(|c| c.host_alias.as_str()).collect();
            aliases.sort();
            aliases.dedup();
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            for c in &copies {
                if let Some(h) = c.host_hash.as_deref() {
                    *counts.entry(h).or_default() += 1;
                }
            }
            let variants = counts.len();
            let (class, reason) = if name.starts_with('.') {
                (IdentityClass::HarnessInternal, None)
            } else if copies.iter().all(|c| c.fleet_owned) {
                (IdentityClass::FleetInternal, None)
            } else if copies.iter().any(|c| c.secret_like) {
                (
                    IdentityClass::NeedsPerson,
                    Some("carries a secret".to_string()),
                )
            } else if variants > 1 {
                let common = counts.iter().max_by_key(|(_, n)| **n).map(|(h, _)| *h);
                let mut odd: Vec<&str> = copies
                    .iter()
                    .filter(|c| c.host_hash.as_deref().is_some_and(|h| Some(h) != common))
                    .map(|c| c.host_alias.as_str())
                    .collect();
                odd.sort();
                odd.dedup();
                (
                    IdentityClass::NeedsPerson,
                    Some(format!("copies differ on {}", odd.join(", "))),
                )
            } else {
                (IdentityClass::Normal, None)
            };
            AssetIdentity {
                hosts: copies
                    .iter()
                    .map(|c| IdentityHost {
                        host_alias: c.host_alias.clone(),
                        harness: c.harness.clone(),
                        host_hash: c.host_hash.clone(),
                    })
                    .collect(),
                signature: aliases.join(","),
                variants,
                class,
                reason,
                kind,
                name,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::AssetInventoryRow;

    fn r(host: &str, kind: &str, name: &str, hash: Option<&str>) -> AssetInventoryRow {
        AssetInventoryRow {
            host_alias: host.into(),
            harness: "claude".into(),
            kind: kind.into(),
            name: name.into(),
            state: "unmanaged".into(),
            host_hash: hash.map(String::from),
            scanned_at: 1,
            ..Default::default()
        }
    }

    #[test]
    fn groups_copies_into_one_identity_with_a_signature() {
        let rows = vec![
            r("oci", "skill", "w", Some("h")),
            r("local", "skill", "w", Some("h")),
            r("trn", "skill", "w", Some("h")),
        ];
        let ids = group_identities(&rows);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].signature, "local,oci,trn");
        assert_eq!(ids[0].variants, 1);
        assert_eq!(ids[0].class, IdentityClass::Normal);
    }

    #[test]
    fn differing_copies_need_a_person_and_name_the_odd_host() {
        let rows = vec![
            r("local", "skill", "w", Some("a")),
            r("oci", "skill", "w", Some("b")),
            r("trn", "skill", "w", Some("a")),
        ];
        let id = &group_identities(&rows)[0];
        assert_eq!(id.class, IdentityClass::NeedsPerson);
        assert_eq!(id.reason.as_deref(), Some("copies differ on oci"));
    }

    #[test]
    fn rules_hide_internals_and_flag_secrets() {
        let mut fleet = r("local", "hook", "stop", None);
        fleet.fleet_owned = true;
        let mut secret = r("local", "mcp_server", "jira", Some("x"));
        secret.secret_like = true;
        let rows = vec![fleet, secret, r("oci", "skill", ".system", Some("s"))];
        let ids = group_identities(&rows);
        let class = |n: &str| ids.iter().find(|i| i.name == n).unwrap().class.clone();
        assert_eq!(class("stop"), IdentityClass::FleetInternal);
        assert_eq!(class("jira"), IdentityClass::NeedsPerson);
        assert_eq!(class(".system"), IdentityClass::HarnessInternal);
    }

    #[test]
    fn orphans_are_not_grouped() {
        let mut o = r("local", "skill", "gone", None);
        o.state = "orphan".into();
        assert!(group_identities(&[o]).is_empty());
    }

    /// The live fleet's shape (2026-09-29): 520 rows are 164 identities in 8
    /// host-set signatures. Synthetic names, real distribution.
    #[test]
    fn live_shape_collapses_520_rows_to_164_identities() {
        let sets: &[(&[&str], usize)] = &[
            (&["local", "mefistos", "oci", "trn"], 82),
            (&["local"], 30),
            (&["local", "mefistos", "oci", "trn", "htz"], 17),
            (&["local", "oci", "trn"], 11),
            (&["trn"], 9),
            (&["local", "mefistos"], 9),
            (&["oci", "htz"], 5),
            (&["htz"], 1),
        ];
        let mut rows = Vec::new();
        let mut n = 0;
        for (hosts, count) in sets {
            for _ in 0..*count {
                n += 1;
                for h in *hosts {
                    rows.push(r(h, "skill", &format!("s{n}"), Some("same")));
                }
            }
        }
        let ids = group_identities(&rows);
        assert_eq!(ids.len(), 164);
        let mut sigs: Vec<&str> = ids.iter().map(|i| i.signature.as_str()).collect();
        sigs.sort();
        sigs.dedup();
        assert_eq!(sigs.len(), 8);
    }
}
