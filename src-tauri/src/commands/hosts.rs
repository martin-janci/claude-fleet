//! Tauri IPC wrappers for SSH host management. Logic lives in `service::hosts`;
//! this file only adapts `tauri::State` to plain references.
//!
//! Remote mode: `list_hosts`, `list_accounts` and `probe_host` have hub tools
//! and route there. The rest are **fleet administration**, which the hub
//! refuses to a paired client by design (`mcp::tools::support::enforce_admin`
//! makes `add_host`, `remove_host` and `hide_host` master-only), so they
//! refuse here with the reason rather than reaching the network to be told no —
//! or worse, editing this machine's own database behind the hub's back.

use crate::backend::FleetBackend;
use fleet_core::cancel::CancellationRegistry;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::agent_install::{AgentInstallsArgs, InstallAgentArgs};
use fleet_core::service::host_setup::{self, HostSetupCheckArgs, SaveHostSetupArgs};
use fleet_core::service::hosts::{
    self, AddHostArgs, HideHostArgs, HostAliasArgs, MergeHostArgs, ProbePreview, ProbeSshAliasArgs,
    SetAccountNicknameArgs,
};
use fleet_core::service::view_scope::ViewScope;
use fleet_core::ssh::SshClient;
use fleet_core::ssh_config::SshHost;
use fleet_core::store::{AccountRow, AgentInstallRow, HostRow, HostSetupRow, SetupCheck, Store};
use std::sync::{Arc, Mutex};
use tauri::State;

/// Async: on Windows it may wait for a WSL detection (`discover_hosts_fresh`),
/// which must not block the main thread a sync command runs on.
#[tauri::command]
pub async fn discover_hosts(
    backend: State<'_, Arc<FleetBackend>>,
) -> Result<Vec<SshHost>, IpcError> {
    // Reads *this machine's* ~/.ssh/config, which says nothing about the hub's
    // hosts — and it only ever feeds the Add-host dialog, which a client
    // cannot complete anyway.
    backend.refuse_local_only("discover_hosts")?;
    hosts::discover_hosts_fresh().await
}

#[tauri::command]
pub async fn list_hosts(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<HostRow>, IpcError> {
    routed::list_hosts(&backend, &store).await
}

#[tauri::command]
pub async fn list_accounts(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<AccountRow>, IpcError> {
    routed::list_accounts(&backend, &store).await
}

#[tauri::command]
pub async fn add_host(
    args: AddHostArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<HostRow, IpcError> {
    backend.refuse_local_only("add_host")?;
    hosts::add_host(args, &store, &*ssh).await
}

#[tauri::command]
pub async fn probe_ssh_alias(
    args: ProbeSshAliasArgs,
    backend: State<'_, Arc<FleetBackend>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<ProbePreview, IpcError> {
    backend.refuse_local_only("probe_ssh_alias")?;
    hosts::probe_ssh_alias(args, &*ssh, &reg).await
}

#[tauri::command]
pub async fn probe_host(
    args: HostAliasArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<HostRow, IpcError> {
    routed::probe_host(&backend, args, &store, &ssh, &reg).await
}

/// Orbit Fleet 4.9: install fleet-agent on a host and move it onto the
/// agent — the job a person starts with the "Install <version>" button.
/// Only a hub accepts agents, so a paired desktop routes it to the hub's
/// `install_agent` (which asks for a trusted full device), and a standalone
/// one gets the service's own refusal (`E_UNSUPPORTED`) from the same code
/// the hub runs. Returns the job at once; `agent_installs` follows it.
#[tauri::command]
pub async fn install_agent(
    args: InstallAgentArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<AgentInstallRow, IpcError> {
    routed::install_agent(&backend, args, &store, &ssh).await
}

/// The fleet-agent install jobs, newest first (the step a running one is
/// on, why a failed one failed). Routed like `install_agent`.
#[tauri::command]
pub async fn agent_installs(
    args: AgentInstallsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<AgentInstallRow>, IpcError> {
    routed::agent_installs(&backend, args, &store).await
}

/// Orbit Fleet 4.7: the host detail's health checklist read (agents on
/// PATH, fleet's hooks and the worker guard, tmux). LocalOnly like
/// `provision_hosts`: it reads a host's `~/.claude/settings.json` over this
/// app's own SSH, which a paired client does not administer.
#[tauri::command]
pub async fn check_host(
    args: fleet_core::service::host_check::CheckHostArgs,
    backend: State<'_, Arc<FleetBackend>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<fleet_core::service::host_check::HostCheck, IpcError> {
    backend.refuse_local_only("check_host")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // M15 G7.12: Settings › Projects asks about a base path too.
    fleet_core::service::host_check::check_host_with(
        &ssh,
        &args.alias,
        args.base_path.as_deref(),
        now,
    )
    .await
}

// ---- Orbit Fleet 4.9: the add-host wizard ------------------------------
// LocalOnly like `add_host` and `probe_ssh_alias`, which the wizard ends in
// and replaces: a draft names an SSH alias of this machine's ~/.ssh/config,
// and its checks SSH from here. Async so the store and SSH never run on the
// main thread.

/// The wizards someone left half-way, newest first.
#[tauri::command]
pub async fn list_host_setups(
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<HostSetupRow>, IpcError> {
    backend.refuse_local_only("list_host_setups")?;
    let s = fleet_core::ipc_error::lock(&store)?;
    Ok(s.host_setups()?)
}

/// Save where the wizard is, so it resumes after a restart.
#[tauri::command]
pub async fn save_host_setup(
    args: SaveHostSetupArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<HostSetupRow, IpcError> {
    backend.refuse_local_only("save_host_setup")?;
    let s = fleet_core::ipc_error::lock(&store)?;
    host_setup::save(&s, &args)
}

/// Drop a draft: the person discarded it, or the host was added.
#[tauri::command]
pub async fn discard_host_setup(
    args: ProbeSshAliasArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<bool, IpcError> {
    backend.refuse_local_only("discard_host_setup")?;
    let s = fleet_core::ipc_error::lock(&store)?;
    Ok(s.delete_host_setup(&args.ssh_alias)?)
}

/// Run one live check and record its answer on the draft.
#[tauri::command]
pub async fn run_host_setup_check(
    args: HostSetupCheckArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SetupCheck, IpcError> {
    backend.refuse_local_only("run_host_setup_check")?;
    let accepted = ssh.agent_registry().is_some();
    let check = host_setup::run_check(&**ssh, &args.ssh_alias, &args.key, accepted).await?;
    let s = fleet_core::ipc_error::lock(&store)?;
    host_setup::record(&s, &args.ssh_alias, &check)?;
    Ok(check)
}

#[tauri::command(async)]
pub fn remove_host(
    args: HostAliasArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<HostRow, IpcError> {
    backend.refuse_local_only("remove_host")?;
    hosts::remove_host(args, &store)
}

/// Host identity & health, task 5. LocalOnly like `remove_host`: the hub
/// tool is master-only and a paired desktop holds a client token, so
/// routing it would always come back `E_FORBIDDEN`.
#[tauri::command(async)]
pub fn merge_host(
    args: MergeHostArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<fleet_core::store::MergeReport, IpcError> {
    backend.refuse_local_only("merge_host")?;
    hosts::merge_host(args, &store)
}

#[tauri::command(async)]
pub fn hide_host(
    args: HideHostArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<HostRow, IpcError> {
    backend.refuse_local_only("hide_host")?;
    hosts::hide_host(args, &store)
}

#[tauri::command(async)]
pub fn set_account_nickname(
    args: SetAccountNicknameArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<AccountRow, IpcError> {
    backend.refuse_local_only("set_account_nickname")?;
    hosts::set_account_nickname(args, &store)
}

/// The routing, away from `tauri::State` so the tests can drive it.
pub(crate) mod routed {
    use super::*;

    pub async fn list_hosts(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<HostRow>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.list_hosts().await,
            // A standalone desktop is one person's machine: it is the hub's
            // own reader here, and `ViewScope::internal` is the value that
            // says so (multi-user M1). A desktop paired to a hub never
            // reaches this arm — its `list_hosts` is the hub's, built from
            // the device's own token and its person.
            None => hosts::list_hosts(store, &ViewScope::internal()),
        }
    }

    pub async fn list_accounts(
        backend: &FleetBackend,
        store: &Mutex<Store>,
    ) -> Result<Vec<AccountRow>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.list_accounts().await,
            None => hosts::list_accounts(store),
        }
    }

    pub async fn install_agent(
        backend: &FleetBackend,
        args: InstallAgentArgs,
        store: &Arc<Mutex<Store>>,
        ssh: &Arc<SshClient>,
    ) -> Result<AgentInstallRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("install_agent", &args).await,
            None => {
                let exec: Arc<dyn fleet_core::ssh::SshExec> = ssh.clone();
                fleet_core::service::agent_install::start(
                    Arc::clone(store),
                    exec,
                    ssh.agent_registry().cloned(),
                    args,
                )
            }
        }
    }

    pub async fn agent_installs(
        backend: &FleetBackend,
        args: AgentInstallsArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<AgentInstallRow>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("agent_installs", &args).await,
            None => fleet_core::service::agent_install::list(store, args.alias.as_deref()),
        }
    }

    /// Re-probing a host is a read of the fleet's state, not fleet
    /// administration, so a paired client may do it (`add_host` /
    /// `remove_host` / `hide_host` are the master-only ones).
    pub async fn probe_host(
        backend: &FleetBackend,
        args: HostAliasArgs,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        reg: &Arc<CancellationRegistry>,
    ) -> Result<HostRow, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("probe_host", &args).await,
            None => hosts::probe_host(args, store, &**ssh, reg).await,
        }
    }
}
