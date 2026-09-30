//! Which harnesses the asset catalog serves on a host (multi-harness F3a).
//!
//! Claude is always served. Another harness (today: Codex) is served where
//! the host says so (`hosts.harnesses`, migration 088) or — when the host
//! leaves it to fleet (`NULL`, "auto") — where a scan finds it: the Codex
//! scan prints `##PRESENT` when the `codex` CLI is on PATH or `~/.codex`
//! exists (`HostSnapshot::present`). Every scanning harness is still scanned
//! on every reachable host, because the scan is the only place detection and
//! the host's manifest come from; the gate decides what is *planned* and
//! *inventoried*.
//!
//! A harness fleet already manages on a host (its manifest names assets)
//! stays served under auto. Turned off explicitly, it is
//! [`HarnessGate::Retiring`]: planned and inventoried against an empty
//! catalog, so the only actions are removals of what fleet installed.
//!
//! [`harness_gate`] is the one decision. `sync::plan_sync`,
//! `sync::rescan_after_apply` (both through `sync::scan_and_persist`) and
//! `inventory::scan_hosts` call it.

use super::harness::{HostSnapshot, HARNESS_IDS};
use super::repo::Catalog;
use super::sync::manifest::Manifest;
use crate::ipc_error::codes::E_INVALID;
use crate::ipc_error::IpcError;
use std::sync::LazyLock;

/// The harness no host can turn off in F3a: fleet's own sessions, hooks,
/// MCP entry and provisioned skills are Claude's.
pub const ALWAYS_ON: &str = "claude";

/// What the catalog does with one harness on one host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessGate {
    /// Not served: nothing planned, no inventory rows (persisting the empty
    /// list clears rows an earlier scan left).
    Off,
    /// Served: planned and inventoried against the host's catalog.
    On,
    /// Turned off, but fleet still manages assets there: planned and
    /// inventoried against an empty catalog, so only removals come back.
    Retiring,
}

/// What one scan says about a harness on a host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HarnessFacts {
    /// The scan saw the harness itself (`HostSnapshot::present`).
    pub detected: bool,
    /// The harness's manifest on the host names at least one asset.
    pub managed: bool,
}

impl HarnessFacts {
    /// Before any scan: nothing detected, nothing managed. Only Claude and
    /// the harnesses a host lists explicitly pass the gate with these.
    pub const UNKNOWN: HarnessFacts = HarnessFacts {
        detected: false,
        managed: false,
    };

    pub fn of(snap: &HostSnapshot, manifest: &Manifest) -> HarnessFacts {
        HarnessFacts {
            detected: snap.present,
            managed: !manifest.assets.is_empty(),
        }
    }
}

/// The one decision. `configured` is the host's `harnesses` column (`None`
/// = auto). Claude is always `On`. Another harness is `On` when the host
/// lists it, or — on auto — when the scan detected it or fleet manages it
/// there; otherwise `Retiring` while fleet still manages something there,
/// else `Off`.
pub fn harness_gate(id: &str, configured: Option<&[String]>, facts: HarnessFacts) -> HarnessGate {
    if id == ALWAYS_ON {
        return HarnessGate::On;
    }
    let wanted = match configured {
        Some(list) => list.iter().any(|h| h == id),
        None => facts.detected || facts.managed,
    };
    if wanted {
        HarnessGate::On
    } else if facts.managed {
        HarnessGate::Retiring
    } else {
        HarnessGate::Off
    }
}

static EMPTY: LazyLock<Catalog> = LazyLock::new(Catalog::default);

/// The catalog a harness in `gate` is planned and inventoried against: the
/// host's own for `On`; an empty one for `Retiring` (every manifest entry
/// then reads as an orphan — `Remove` in a plan, `orphan` in the inventory);
/// none for `Off`.
pub fn gated_catalog(gate: HarnessGate, catalog: &Catalog) -> Option<&Catalog> {
    match gate {
        HarnessGate::On => Some(catalog),
        HarnessGate::Retiring => Some(&EMPTY),
        HarnessGate::Off => None,
    }
}

/// `HostPlan::detail` of a `Retiring` plan.
pub fn retiring_detail(id: &str) -> String {
    format!("{id} is turned off on this host: only what fleet installed there is removed")
}

/// An explicit harness list, checked and put in `HARNESS_IDS` order: every
/// id known, `claude` present, duplicates dropped.
pub fn normalize_harnesses(list: &[String]) -> Result<Vec<String>, IpcError> {
    if let Some(unknown) = list.iter().find(|h| !HARNESS_IDS.contains(&h.as_str())) {
        return Err(IpcError::new(
            E_INVALID,
            format!(
                "unknown harness '{unknown}' (known: {})",
                HARNESS_IDS.join(", ")
            ),
        ));
    }
    if !list.iter().any(|h| h == ALWAYS_ON) {
        return Err(IpcError::new(
            E_INVALID,
            "claude cannot be turned off on a host: fleet's own sessions, hooks and skills are Claude's",
        ));
    }
    Ok(HARNESS_IDS
        .iter()
        .filter(|id| list.iter().any(|h| h == *id))
        .map(|id| id.to_string())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::model::Asset;
    use crate::service::catalog::sync::manifest::ManifestEntry;

    fn list(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    const NOTHING: HarnessFacts = HarnessFacts {
        detected: false,
        managed: false,
    };
    const DETECTED: HarnessFacts = HarnessFacts {
        detected: true,
        managed: false,
    };
    const MANAGED: HarnessFacts = HarnessFacts {
        detected: false,
        managed: true,
    };

    #[test]
    fn claude_is_always_on() {
        for configured in [
            None,
            Some(list(&["claude"])),
            Some(list(&["claude", "codex"])),
        ] {
            for facts in [NOTHING, DETECTED, MANAGED, HarnessFacts::UNKNOWN] {
                assert_eq!(
                    harness_gate("claude", configured.as_deref(), facts),
                    HarnessGate::On
                );
            }
        }
    }

    #[test]
    fn auto_follows_detection_and_the_manifest() {
        assert_eq!(harness_gate("codex", None, NOTHING), HarnessGate::Off);
        assert_eq!(harness_gate("codex", None, DETECTED), HarnessGate::On);
        assert_eq!(
            harness_gate("codex", None, MANAGED),
            HarnessGate::On,
            "a host fleet already manages Codex on stays served"
        );
        assert_eq!(
            harness_gate("codex", None, HarnessFacts::UNKNOWN),
            HarnessGate::Off
        );
    }

    #[test]
    fn an_explicit_list_wins_and_an_explicit_off_retires_what_fleet_installed() {
        let on = list(&["claude", "codex"]);
        let off = list(&["claude"]);
        let (on, off) = (Some(on.as_slice()), Some(off.as_slice()));
        assert_eq!(harness_gate("codex", on, NOTHING), HarnessGate::On);
        assert_eq!(
            harness_gate("codex", on, HarnessFacts::UNKNOWN),
            HarnessGate::On
        );
        assert_eq!(harness_gate("codex", off, DETECTED), HarnessGate::Off);
        assert_eq!(harness_gate("codex", off, MANAGED), HarnessGate::Retiring);
        assert_eq!(
            harness_gate(
                "codex",
                off,
                HarnessFacts {
                    detected: true,
                    managed: true
                }
            ),
            HarnessGate::Retiring
        );
    }

    #[test]
    fn gated_catalog_is_the_hosts_own_an_empty_one_or_none() {
        let mut cat = Catalog::default();
        cat.assets
            .push(Asset::from_yaml(None, "kind: skill\nname: s\ndescription: d\n").unwrap());
        assert_eq!(
            gated_catalog(HarnessGate::On, &cat).unwrap().assets.len(),
            1
        );
        assert!(gated_catalog(HarnessGate::Retiring, &cat)
            .unwrap()
            .assets
            .is_empty());
        assert!(gated_catalog(HarnessGate::Off, &cat).is_none());
    }

    #[test]
    fn facts_come_from_the_presence_line_and_a_non_empty_manifest() {
        let mut snap = HostSnapshot::default();
        let mut manifest = Manifest::default();
        assert_eq!(HarnessFacts::of(&snap, &manifest), NOTHING);
        snap.present = true;
        manifest
            .assets
            .insert("skill/s".into(), ManifestEntry::default());
        assert_eq!(
            HarnessFacts::of(&snap, &manifest),
            HarnessFacts {
                detected: true,
                managed: true
            }
        );
    }

    #[test]
    fn retiring_detail_names_the_harness() {
        assert_eq!(
            retiring_detail("codex"),
            "codex is turned off on this host: only what fleet installed there is removed"
        );
    }

    #[test]
    fn normalize_harnesses_orders_dedupes_and_requires_claude() {
        assert_eq!(
            normalize_harnesses(&list(&["codex", "claude", "codex"])).unwrap(),
            list(&["claude", "codex"])
        );
        assert_eq!(
            normalize_harnesses(&list(&["claude"])).unwrap(),
            list(&["claude"])
        );
        let err = normalize_harnesses(&list(&["codex"])).unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        assert!(err.message.contains("claude"), "{}", err.message);
        assert_eq!(normalize_harnesses(&[]).unwrap_err().code, "E_INVALID");
        let err = normalize_harnesses(&list(&["claude", "gemini"])).unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        assert!(err.message.contains("gemini"), "{}", err.message);
    }
}
