//! Settings → Federation (Orbit Fleet 11.5): this fleet's links to other
//! fleets' hubs, with each link's state, latency and message counts, and
//! Link a hub. Federation is hub to hub, so a paired desktop shows its hub's
//! links (`list_peer_links`, `link_peer`, `unlink_peer`); a standalone
//! desktop is no hub and links nothing — it lists its own store's links,
//! which are none, and refuses a new one.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::store::{PeerLinkSummary, Store};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::State;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LinkPeerHubArgs {
    pub url: String,
    pub code: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UnlinkPeerHubArgs {
    pub id: i64,
}

/// One row of the Federation page: the hub's summary plus the two words the
/// page shows that the summary keeps as data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationLink {
    #[serde(flatten)]
    pub link: PeerLinkSummary,
    /// The other fleet's id once an exchange has named it, else the hub's
    /// address, else the link's number.
    pub title: String,
    /// `latency_ms` in words ("42 ms"); absent until a dialer has measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency: Option<String>,
}

impl From<PeerLinkSummary> for FederationLink {
    fn from(link: PeerLinkSummary) -> Self {
        let title = link
            .fleet_id
            .clone()
            .or_else(|| {
                link.url.as_deref().map(|u| {
                    u.trim_start_matches("https://")
                        .trim_end_matches('/')
                        .to_string()
                })
            })
            .unwrap_or_else(|| format!("Link {}", link.id));
        let latency = link.latency_ms.map(|ms| format!("{ms} ms"));
        FederationLink {
            link,
            title,
            latency,
        }
    }
}

/// What `unlink_peer_hub` answers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unlinked {
    pub id: i64,
    /// Messages that were waiting for the link, failed back to their senders.
    pub failed_messages: usize,
}

const STANDALONE: &str =
    "this desktop is not a hub: pair it with one (Settings → Hub & sync), then link that hub to others";

#[tauri::command]
pub async fn list_peer_links(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<FederationLink>, IpcError> {
    routed::list_peer_links(&backend, &store).await
}

#[tauri::command]
pub async fn link_peer_hub(
    args: LinkPeerHubArgs,
    backend: State<'_, Arc<FleetBackend>>,
) -> Result<Option<PeerLinkSummary>, IpcError> {
    routed::link_peer_hub(&backend, args).await
}

#[tauri::command]
pub async fn unlink_peer_hub(
    args: UnlinkPeerHubArgs,
    backend: State<'_, Arc<FleetBackend>>,
) -> Result<Unlinked, IpcError> {
    routed::unlink_peer_hub(&backend, args).await
}

pub(crate) mod routed {
    use super::*;
    use serde_json::json;

    pub async fn list_peer_links(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<FederationLink>, IpcError> {
        let rows: Vec<PeerLinkSummary> = match backend.hub() {
            Some(hub) => hub.route("list_peer_links", &json!({})).await?,
            None => fleet_core::ipc_error::lock(store)?.peer_link_summaries()?,
        };
        // A removed link is history, not a link: the page lists live ones.
        Ok(rows
            .into_iter()
            .filter(|r| r.revoked_at.is_none())
            .map(FederationLink::from)
            .collect())
    }

    pub async fn link_peer_hub(
        backend: &FleetBackend,
        args: LinkPeerHubArgs,
    ) -> Result<Option<PeerLinkSummary>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route(
                    "link_peer_hub",
                    &json!({ "url": args.url, "code": args.code }),
                )
                .await
            }
            None => Err(IpcError::new(codes::E_UNSUPPORTED, STANDALONE)),
        }
    }

    pub async fn unlink_peer_hub(
        backend: &FleetBackend,
        args: UnlinkPeerHubArgs,
    ) -> Result<Unlinked, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("unlink_peer_hub", &json!({ "id": args.id }))
                    .await
            }
            None => Err(IpcError::new(codes::E_UNSUPPORTED, STANDALONE)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(
        fleet_id: Option<&str>,
        url: Option<&str>,
        latency_ms: Option<i64>,
    ) -> PeerLinkSummary {
        PeerLinkSummary {
            id: 3,
            fleet_id: fleet_id.map(str::to_string),
            role: "dialer".into(),
            url: url.map(str::to_string),
            state: "connected".into(),
            last_exchange_at: None,
            last_error: None,
            pending: 0,
            revoked_at: None,
            latency_ms,
            messages_today: 0,
            messages_total: 0,
        }
    }

    #[test]
    fn a_link_is_titled_by_its_fleet_then_its_address_then_its_number() {
        let named =
            FederationLink::from(summary(Some("acme"), Some("https://b.example/"), Some(42)));
        assert_eq!(named.title, "acme");
        assert_eq!(named.latency.as_deref(), Some("42 ms"));
        let dialing = FederationLink::from(summary(None, Some("https://b.example/"), None));
        assert_eq!(dialing.title, "b.example");
        assert_eq!(dialing.latency, None);
        let listener = FederationLink::from(summary(None, None, None));
        assert_eq!(listener.title, "Link 3");
        // The page reads the summary's own fields beside the two words.
        let v = serde_json::to_value(&named).unwrap();
        assert_eq!(
            (v["id"].as_i64(), v["state"].as_str()),
            (Some(3), Some("connected"))
        );
    }
}
