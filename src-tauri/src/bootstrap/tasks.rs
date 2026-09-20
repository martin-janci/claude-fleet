//! The real [`FleetTasks`]: the only place in the app that spawns one of the
//! three fleet-owning background tasks.
//!
//! It lives here rather than in `lib.rs`'s setup closure for one reason —
//! nothing can test that closure, because it needs a live `tauri::App`. With
//! the spawns here and the decision in [`crate::backend::startup`], both are
//! reachable: the decision by a unit test with a recording double, and this
//! file by a test asserting `lib.rs` names none of the three.
//!
//! Every field is a handle the app already built; this struct only holds them
//! so the three `&self` methods can reach them. Cloning an `Arc` per call is
//! what makes `&self` possible, and each of these runs at most once per
//! process anyway.

use crate::app_events::AppHandleEventBus;
use crate::backend::connection::{ConnectionReporter, HubConnectionStatus};
use crate::backend::events::{spawn_event_bridge, EventBridge, HubResync, HubSse, RealDelay};
use crate::backend::remote::HubBackend;
use crate::backend::startup::FleetTasks;
use crate::backend::RemoteConfig;
use crate::bootstrap::mcp::maybe_start_mcp;
use fleet_core::events::EventBus;
use fleet_core::service::account_usage::UsageCache;
use fleet_core::service::tick::{spawn_account_usage_tick, spawn_reconcile_tick};
use fleet_core::service::tunnel::TunnelSupervisor;
use fleet_core::store::Store;
use fleet_core::{cancel, mcp, ssh};
use std::sync::{Arc, Mutex};

pub(crate) struct RealFleetTasks {
    pub app: tauri::AppHandle,
    pub store: Arc<Mutex<Store>>,
    pub ssh: Arc<ssh::SshClient>,
    pub reg: Arc<cancel::CancellationRegistry>,
    pub tunnels: Arc<TunnelSupervisor>,
    pub guards: mcp::McpGuards,
    pub usage_cache: Arc<Mutex<UsageCache>>,
    pub bus: Arc<dyn EventBus>,
    /// The same bus, concretely. The hub event bridge hands it
    /// `(&'static str, Value)` pairs directly — the trait object above only
    /// exposes `emit(&RowChange)`, and remote mode has no local row to build
    /// a `RowChange` from. Both ends feed one channel; see
    /// `app_events::AppHandleEventBus`.
    pub frontend: Arc<AppHandleEventBus>,
    /// `Some` only when this app is a window onto a hub.
    pub remote: Option<RemoteConfig>,
    /// Cancelled when the app stops, so the bridge's socket does not hold a
    /// shutdown open.
    pub shutdown: tokio_util::sync::CancellationToken,
    /// Where the bridge reports whether its stream is up; managed state, so
    /// the `hub_connection` command reads the same value.
    pub hub_link: Arc<HubConnectionStatus>,
}

impl FleetTasks for RealFleetTasks {
    /// Start the MCP control API if the user has enabled it (off by default).
    /// Reuses the same Store / SshClient / registry as the UI.
    fn start_control_api(&self) {
        maybe_start_mcp(
            &self.app,
            &self.store,
            &self.ssh,
            &self.reg,
            &self.tunnels,
            &self.guards,
        );
    }

    /// Task H: proactive background reconcile tick. A Tauri-runtime spawned
    /// interval drives `service::sessions::reconcile_now` on the same managed
    /// Store/SshClient the commands use, so fleet state stays fresh without
    /// the UI having to poll. Reconcile is Tauri-free (events flow through the
    /// store's EventBus), so the loop needs no AppHandle. Interval comes from
    /// settings (`reconcile.interval_secs`, default 20; 0 disables). A
    /// `try_lock` guard skips a tick if the previous reconcile is still
    /// running so slow passes can't stack.
    ///
    /// The desktop's shutdown path (`lib.rs`'s window-close handler) calls
    /// `ssh.shutdown_all()` directly without awaiting this task, so passing
    /// `self.shutdown` here would let that path tear down SSH masters out
    /// from under an in-flight pass — the same defect issue #144 fixed on the
    /// hub. A token that is never cancelled keeps that (pre-existing, out of
    /// scope here) desktop behaviour unchanged.
    fn start_reconcile_tick(&self) {
        let _ = spawn_reconcile_tick(
            Arc::clone(&self.store),
            Arc::clone(&self.ssh),
            tokio_util::sync::CancellationToken::new(),
        );
    }

    /// Independent 60s account-usage poll loop. Deliberately separate
    /// from the reconcile tick above (which `reconcile.interval_secs=0` can
    /// disable entirely) so usage keeps polling on its own cadence;
    /// `service::account_usage`'s 5-minute floor still caps real requests to
    /// one per account.
    ///
    /// Never cancelled, for the same reason as `start_reconcile_tick` above.
    ///
    /// `std::mem::drop`, not `let _ =`: the return is a `JoinHandle`, which
    /// is itself a `Future`, and clippy's `let_underscore_future` flags
    /// binding one to `_` as likely-accidental. The task is already running
    /// on its own regardless — this line only decides not to keep a handle
    /// to it.
    fn start_account_usage_tick(&self) {
        std::mem::drop(spawn_account_usage_tick(
            Arc::clone(&self.store),
            Arc::clone(&self.ssh),
            Arc::clone(&self.usage_cache),
            Arc::clone(&self.bus),
            tokio_util::sync::CancellationToken::new(),
        ));
    }

    /// Follow the hub's `GET /events` and re-emit every frame as the
    /// frontend event a local `RowChange` would have produced.
    ///
    /// The only background task a hub client runs, and the reason it is not
    /// optional: in remote mode nothing local mutates, so the local event bus
    /// never fires and the UI would render whatever it listed at startup and
    /// then sit frozen.
    fn start_event_bridge(&self) {
        let Some(cfg) = self.remote.clone() else {
            // `start_background_tasks` only calls this in remote mode; a
            // standalone app reaching here would be a routing bug, not a
            // reason to open a socket to nowhere.
            tracing::error!("[hub events] asked to bridge events with no hub configured");
            return;
        };
        let hub = Arc::new(HubBackend::new(cfg.clone()));
        let sink = Arc::clone(&self.frontend);
        let bridge = EventBridge::new(
            Arc::new(HubSse::new(cfg)),
            Arc::clone(&sink) as Arc<dyn crate::backend::events::RemoteEventSink>,
            Arc::new(HubResync::new(hub, sink)),
            Arc::new(RealDelay),
            self.shutdown.clone(),
        )
        // The disconnected banner's signal; the same status `hub_connection`
        // answers from.
        .reporting_to(Arc::clone(&self.hub_link) as Arc<dyn ConnectionReporter>);
        spawn_event_bridge(bridge);
    }
}
