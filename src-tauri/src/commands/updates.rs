//! Settings → Updates (Orbit Fleet 11.9b): what each part of the fleet runs
//! — the hub, every host's agent, this desktop and the phone — and what the
//! hub would tell it now. A paired desktop asks its hub (`update_status`);
//! a standalone desktop reads its own store, which observes nothing until
//! it serves updates, so its list is empty.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::IpcError;
use fleet_core::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tauri::State;

/// One row of the page's "What runs where" table (`updates.targets`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateTargetRow {
    /// "Hub", a host's alias, or the paired device's number.
    pub device: String,
    /// "Hub", "Agent", "Desktop" or "Phone".
    pub part: String,
    pub version: String,
    /// What the hub would tell it now, in words.
    pub update: String,
    /// The version it is offered, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offers: Option<String>,
    pub reported_at: i64,
    /// An update in flight (step 10.10): the page draws its loader beside
    /// the `update` cell. Absent when nothing is moving.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transfer: Option<RowTransfer>,
}

/// A row's transfer, for a page table (`DataItem.svelte`): which cell it
/// sits beside and how far it is, when that is known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowTransfer {
    pub column: String,
    /// 0–100 when the size is known (Progress ring); `None` draws Data rain.
    pub percent: Option<u8>,
}

/// An install's steps once its download is in: a known count, so the ring
/// says how far it is. A download reports no bytes, so it has no size.
fn in_flight(phase: &str) -> Option<(&'static str, Option<u8>)> {
    Some(match phase {
        "downloading" => ("Downloading", None),
        "verifying" => ("Verifying", Some(25)),
        "installing" => ("Installing", Some(50)),
        "validating" => ("Checking it runs", Some(75)),
        "rolling_back" => ("Rolling back", None),
        _ => return None,
    })
}

/// The fields of `update_status`'s `targets` the page reads.
#[derive(Debug, Deserialize)]
struct Target {
    target: String,
    component: String,
    version: String,
    phase: String,
    reported_at: i64,
    status: String,
    target_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Status {
    targets: Vec<Target>,
}

fn device(target: &str) -> String {
    match target.split_once(':') {
        Some(("hub", _)) => "Hub".into(),
        Some(("agent", host)) => host.into(),
        Some(("client", id)) => format!("Device {id}"),
        _ => target.into(),
    }
}

fn part(component: &str) -> String {
    match component {
        "hub" => "Hub",
        "agent" => "Agent",
        "desktop" => "Desktop",
        "mobile" => "Phone",
        other => other,
    }
    .into()
}

fn update(t: &Target) -> String {
    if matches!(t.phase.as_str(), "failed" | "rollback_failed") {
        return "Update failed".into();
    }
    if let Some((words, _)) = in_flight(&t.phase) {
        return words.into();
    }
    match t.status.as_str() {
        "up_to_date" => "Up to date",
        "update_available" => "Update available",
        "update_required" => "Update required",
        "client_too_new" => "Newer than the hub",
        "rollback" => "Rolling back",
        "hold" => "Held",
        _ => "Unknown",
    }
    .into()
}

/// The table's rows from an `update_status` answer, hub first, then by part
/// and device.
fn rows(status: Value) -> Result<Vec<UpdateTargetRow>, IpcError> {
    let st: Status = serde_json::from_value(status).map_err(|e| {
        IpcError::new(
            fleet_core::ipc_error::codes::E_INTERNAL,
            format!("update_status: {e}"),
        )
    })?;
    let mut out: Vec<UpdateTargetRow> = st
        .targets
        .iter()
        .map(|t| UpdateTargetRow {
            device: device(&t.target),
            part: part(&t.component),
            version: t.version.clone(),
            update: update(t),
            offers: t
                .target_version
                .clone()
                .filter(|v| *v != t.version && t.status != "up_to_date"),
            reported_at: t.reported_at,
            transfer: in_flight(&t.phase).map(|(_, percent)| RowTransfer {
                column: "update".into(),
                percent,
            }),
        })
        .collect();
    let rank = |p: &str| {
        ["Hub", "Agent", "Desktop", "Phone"]
            .iter()
            .position(|x| *x == p)
    };
    out.sort_by(|a, b| {
        (rank(&a.part), &a.part, &a.device).cmp(&(rank(&b.part), &b.part, &b.device))
    });
    Ok(out)
}

#[tauri::command]
pub async fn list_update_targets(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<UpdateTargetRow>, IpcError> {
    routed::list_update_targets(&backend, &store).await
}

/// Does this desktop have an update, and how would it install? Paired: its
/// hub decides; standalone: the published channel under this app's own
/// `update.*` settings. The answer's target is verified against the release
/// key and remembered for `update_install` (design S7).
#[tauri::command]
pub async fn update_check(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    updates: State<'_, crate::self_update::SelfUpdate>,
    data_dir: State<'_, crate::commands::diagnostics::AppDataDir>,
) -> Result<crate::self_update::DesktopUpdate, IpcError> {
    let backend: Arc<FleetBackend> = Arc::clone(&backend);
    updates.check(&backend, &store, &data_dir.0).await
}

/// Install the target the last `update_check` verified, in place, and
/// restart. Refused for a target this platform only downloads.
#[tauri::command]
pub async fn update_install(
    app: tauri::AppHandle,
    backend: State<'_, Arc<FleetBackend>>,
    updates: State<'_, crate::self_update::SelfUpdate>,
    data_dir: State<'_, crate::commands::diagnostics::AppDataDir>,
) -> Result<(), IpcError> {
    let backend: Arc<FleetBackend> = Arc::clone(&backend);
    updates.install(&app, &backend, &data_dir.0).await
}

pub(crate) mod routed {
    use super::*;
    use fleet_core::service::update;
    use serde_json::json;

    pub async fn list_update_targets(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<UpdateTargetRow>, IpcError> {
        let status: Value = match backend.hub() {
            Some(hub) => hub.route("list_update_targets", &json!({})).await?,
            None => {
                let st = update::status(
                    store,
                    &fleet_core::mcp::Caller::master(),
                    &update::trusted_keys(),
                    fleet_core::store::now_unix(),
                )?;
                serde_json::to_value(st).unwrap_or(Value::Null)
            }
        };
        rows(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn target(target: &str, component: &str, status: &str, offers: Option<&str>) -> Value {
        json!({
            "target": target, "component": component, "version": "0.5.3",
            "phase": "idle", "reported_at": 100, "status": status,
            "target_version": offers, "mandatory": false, "reason": "",
        })
    }

    /// 11.9b: every host's agent and the phone each get a row, in words,
    /// the hub first.
    #[test]
    fn each_part_of_the_fleet_is_a_row_in_words() {
        let got = rows(json!({ "targets": [
            target("client:7", "mobile", "update_available", Some("0.5.4")),
            target("agent:mercury", "agent", "up_to_date", Some("0.5.3")),
            target("hub:self", "hub", "up_to_date", None),
        ]}))
        .unwrap();
        let brief: Vec<_> = got
            .iter()
            .map(|r| {
                (
                    r.device.as_str(),
                    r.part.as_str(),
                    r.update.as_str(),
                    r.offers.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            brief,
            [
                ("Hub", "Hub", "Up to date", None),
                ("mercury", "Agent", "Up to date", None),
                ("Device 7", "Phone", "Update available", Some("0.5.4")),
            ]
        );
    }

    /// 10.10: an update in flight carries its loader: a download (no
    /// bytes reported) has no size, an install's later steps do.
    #[test]
    fn an_update_in_flight_carries_its_transfer() {
        let mut down = target("agent:venus", "agent", "update_available", Some("0.5.4"));
        down["phase"] = json!("downloading");
        let mut inst = target("agent:mars", "agent", "update_available", Some("0.5.4"));
        inst["phase"] = json!("installing");
        let idle = target("hub:self", "hub", "up_to_date", None);
        let got = rows(json!({ "targets": [down, inst, idle] })).unwrap();
        let by = |d: &str| got.iter().find(|r| r.device == d).unwrap().clone();
        assert_eq!(by("venus").update, "Downloading");
        assert_eq!(
            by("venus").transfer,
            Some(RowTransfer {
                column: "update".into(),
                percent: None
            })
        );
        assert_eq!(by("mars").update, "Installing");
        assert_eq!(by("mars").transfer.and_then(|t| t.percent), Some(50));
        assert_eq!(by("Hub").transfer, None);
    }

    #[test]
    fn a_failed_install_says_so_whatever_the_hub_offers() {
        let mut t = target("agent:venus", "agent", "update_available", Some("0.5.4"));
        t["phase"] = json!("failed");
        let got = rows(json!({ "targets": [t] })).unwrap();
        assert_eq!(got[0].update, "Update failed");
    }
}
