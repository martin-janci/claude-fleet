//! Which harnesses the asset catalog serves on a host (multi-harness F3a).
//!
//! Claude is always served. Another harness (today: Codex) is served where
//! the host says so (`hosts.harnesses`, migration 089) or — when the host
//! leaves it to fleet (`NULL`, "auto") — where a scan finds it: the Codex
//! scan prints `##PRESENT` when the `codex` CLI is on PATH, or Codex's login
//! (`~/.codex/auth.json`) or session logs (`~/.codex/sessions`) exist —
//! never on `~/.codex` alone, which a Codex sync creates itself
//! (`HostSnapshot::present`). Every scanning harness is still scanned
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
use crate::ipc_error::codes::{E_INVALID, E_NOTFOUND};
use crate::ipc_error::{lock, IpcError};
use crate::store::{HostRow, Store};
use std::sync::LazyLock;
use std::sync::Mutex;

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

/// Set a host's harness choice: `None` = auto, `Some` = an explicit list
/// (checked and ordered by [`normalize_harnesses`]; `claude` required).
/// Edits fleet state only — the next `plan_sync` follows it. Every parked
/// sync plan covering the host is dropped (one computed before Codex was
/// turned off must not be applied after), and the host is owed a rescan on
/// the scan tick's next pass so its inventory follows the choice. Returns the
/// host's new row. The `set_host_harnesses` MCP tool, `catalog_admin`'s
/// `set_host_harnesses` action and the `catalog_set_host_harnesses`
/// desktop command all land here.
pub fn set_host_harnesses(
    host_alias: &str,
    harnesses: Option<&[String]>,
    store: &Mutex<Store>,
) -> Result<HostRow, IpcError> {
    crate::validate::host_alias(host_alias)?;
    let normalized = harnesses.map(normalize_harnesses).transpose()?;
    let row = {
        let s = lock(store)?;
        s.set_host_harnesses(host_alias, normalized.as_deref())?;
        s.get_host_row(host_alias)?
            .ok_or_else(|| IpcError::new(E_NOTFOUND, format!("host {host_alias} not found")))?
    };
    let dropped = super::sync::plan::registry_drop_host(host_alias);
    if dropped > 0 {
        tracing::info!(
            host = host_alias,
            dropped,
            "harness choice changed; dropped parked sync plans covering the host"
        );
    }
    super::scan_tick::owe_rescan(host_alias);
    Ok(row)
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

    /// A harness change drops every parked plan covering the host (so one
    /// computed before Codex went off cannot be applied) and owes the host
    /// a rescan; another host's plan stays.
    #[test]
    fn set_host_harnesses_drops_parked_plans_and_owes_a_rescan() {
        use crate::service::catalog::sync::plan::{
            registry_put, registry_take, HostPlan, SyncPlan,
        };
        let alias = format!("hs-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let other = format!("hs-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let hp = |a: &str| HostPlan {
            host_alias: a.to_string(),
            harness: "codex".into(),
            status: "planned".into(),
            detail: None,
            actions: Vec::new(),
            snapshot: HostSnapshot::default(),
            manifest: Manifest::default(),
        };
        let store = std::sync::Mutex::new(crate::store::Store::open_in_memory().unwrap());
        store
            .lock()
            .unwrap()
            .insert_host(&alias, Some("h"))
            .unwrap();
        let stale = registry_put(SyncPlan::new(vec![hp(&alias)]));
        let kept = registry_put(SyncPlan::new(vec![hp(&other)]));

        set_host_harnesses(&alias, Some(list(&["claude"]).as_slice()), &store).unwrap();

        assert!(
            registry_take(&stale).is_none(),
            "the pre-change plan is gone"
        );
        assert!(registry_take(&kept).is_some(), "another host's plan stays");
        assert!(crate::service::catalog::scan_tick::rescan_requested(&alias));
        assert!(!crate::service::catalog::scan_tick::rescan_requested(
            &other
        ));

        // A refused call changes nothing, so it drops nothing either.
        let again = registry_put(SyncPlan::new(vec![hp(&alias)]));
        assert!(set_host_harnesses(&alias, Some(list(&["codex"]).as_slice()), &store).is_err());
        assert!(registry_take(&again).is_some());
    }

    #[test]
    fn set_host_harnesses_normalises_validates_and_clears() {
        let store = std::sync::Mutex::new(crate::store::Store::open_in_memory().unwrap());
        store.lock().unwrap().insert_host("h", Some("h")).unwrap();
        let row = set_host_harnesses(
            "h",
            Some(list(&["codex", "claude", "codex"]).as_slice()),
            &store,
        )
        .unwrap();
        assert_eq!(row.harnesses, Some(list(&["claude", "codex"])));
        let err = set_host_harnesses("h", Some(list(&["codex"]).as_slice()), &store).unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        let err = set_host_harnesses("h", Some(list(&["claude", "gemini"]).as_slice()), &store)
            .unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        assert_eq!(
            store
                .lock()
                .unwrap()
                .get_host_row("h")
                .unwrap()
                .unwrap()
                .harnesses,
            Some(list(&["claude", "codex"])),
            "a refused call writes nothing"
        );
        assert_eq!(
            set_host_harnesses("h", None, &store).unwrap().harnesses,
            None
        );
        assert_eq!(
            set_host_harnesses("ghost", None, &store).unwrap_err().code,
            "E_NOTFOUND"
        );
    }
}
