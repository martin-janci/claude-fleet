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
            // `fleet_owned` first, deliberately: fleet's own MCP server is
            // both fleet-owned and secret-bearing on every scanned host, and it
            // must stay an internal rather than becoming a person's problem
            // fleet-wide.
            //
            // A CREDENTIAL then outranks the dot-name shortcut, which used to be
            // tested first — so a `.`-named asset carrying a token was filed
            // `harness_internal` with no reason, the one class a person never
            // looks at, while the same content under any other name was
            // `needs_person`. Nothing about a leading dot makes a secret less
            // of one.
            let (class, reason) = if copies.iter().all(|c| c.fleet_owned) {
                (IdentityClass::FleetInternal, None)
            } else if copies.iter().any(|c| c.secret_like) {
                (
                    IdentityClass::NeedsPerson,
                    Some("carries a secret".to_string()),
                )
            } else if name.starts_with('.') {
                (IdentityClass::HarnessInternal, None)
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

    /// The classification PRECEDENCE, not just each rule once.
    ///
    /// Every row here used to carry at most one flag, so the branch order —
    /// dot-prefix, then `fleet_owned`, then `secret_like` — was load-bearing on
    /// every real fleet and yet free to reorder: fleet's own
    /// `mcp_server:claude-fleet` is `fleet_owned && secret_like` on *every*
    /// scanned host, so only the order keeps it out of `needs_person`
    /// fleet-wide. The two-flag rows below are what makes a swap fail.
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

    /// `fleet_owned` outranks `secret_like`, and `secret_like` outranks the
    /// dot-prefix.
    #[test]
    fn classification_precedence_holds_when_two_flags_meet() {
        // fleet's own MCP server, exactly as every scanned host reports it:
        // fleet-owned AND secret-bearing. It must stay an internal, not become
        // a person's problem on every host in the fleet.
        let mut ours = r("local", "mcp_server", "claude-fleet", Some("h"));
        ours.fleet_owned = true;
        ours.secret_like = true;
        let id = &group_identities(&[ours])[0];
        assert_eq!(
            id.class,
            IdentityClass::FleetInternal,
            "fleet_owned must outrank secret_like"
        );
        assert_eq!(id.reason, None);

        // A dot-named secret-bearing server is NeedsPerson. This assertion was
        // the other way round, recording the dot check coming first and saying
        // that changing it should be a conscious decision — this is that
        // decision: hiding a credential-carrying asset behind the internals
        // toggle, with no reason given, is the one outcome a person cannot act
        // on. Nothing about a leading dot makes a secret less of one.
        let mut dotted = r("oci", "mcp_server", ".vendor", Some("h"));
        dotted.secret_like = true;
        let id = &group_identities(&[dotted])[0];
        assert_eq!(id.class, IdentityClass::NeedsPerson);
        assert_eq!(id.reason.as_deref(), Some("carries a secret"));

        // A dot-named asset with no secret is still an internal.
        let plain = r("oci", "mcp_server", ".vendor", Some("h"));
        assert_eq!(
            group_identities(&[plain])[0].class,
            IdentityClass::HarnessInternal
        );

        // And secret_like still outranks the copies-differ rule.
        let mut a = r("local", "mcp_server", "jira", Some("x"));
        a.secret_like = true;
        let b = r("oci", "mcp_server", "jira", Some("y"));
        let id = &group_identities(&[a, b])[0];
        assert_eq!(id.class, IdentityClass::NeedsPerson);
        assert_eq!(
            id.reason.as_deref(),
            Some("carries a secret"),
            "the secret is the reason, not the drift"
        );
    }

    #[test]
    fn orphans_are_not_grouped() {
        let mut o = r("local", "skill", "gone", None);
        o.state = "orphan".into();
        assert!(group_identities(&[o]).is_empty());
    }

    /// The live fleet's shape (2026-09-29): 164 identities in 8 host-set
    /// signatures, over the 514 rows this distribution accounts for. Synthetic
    /// names, real distribution.
    ///
    /// Named and documented as 520 rows while building 514, with nothing
    /// asserting the count — so the one number the fixture exists to represent
    /// was both wrong and unheld. 520 was the live fleet's total; the eight
    /// signatures below are 514 of it. The assertion is what keeps the name
    /// honest when a count below is edited.
    #[test]
    fn live_shape_collapses_514_rows_to_164_identities() {
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
        assert_eq!(rows.len(), 514, "the distribution below, counted");
        let ids = group_identities(&rows);
        assert_eq!(ids.len(), 164);
        let mut sigs: Vec<&str> = ids.iter().map(|i| i.signature.as_str()).collect();
        sigs.sort();
        sigs.dedup();
        assert_eq!(sigs.len(), 8);
    }
}
