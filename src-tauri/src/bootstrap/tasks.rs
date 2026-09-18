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

use crate::backend::startup::FleetTasks;
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
    fn start_reconcile_tick(&self) {
        spawn_reconcile_tick(Arc::clone(&self.store), Arc::clone(&self.ssh));
    }

    /// Task 4: independent 60s account-usage poll loop. Deliberately separate
    /// from the reconcile tick above (which `reconcile.interval_secs=0` can
    /// disable entirely) so usage keeps polling on its own cadence;
    /// `service::account_usage`'s 5-minute floor still caps real requests to
    /// one per account.
    fn start_account_usage_tick(&self) {
        spawn_account_usage_tick(
            Arc::clone(&self.store),
            Arc::clone(&self.ssh),
            Arc::clone(&self.usage_cache),
            Arc::clone(&self.bus),
        );
    }
}
