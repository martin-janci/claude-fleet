//! Tauri IPC wrappers for Settings → Diagnostics. The report itself is built
//! in `service::diagnostics`; this file only gathers managed state. These
//! are Tauri commands, not MCP tools. `docs/control-api-reference.md` lists
//! them by name only (its "Tauri IPC commands" section is read from the
//! `generate_handler!` list in `lib.rs`), so adding, removing or renaming one
//! needs a `REGEN_DOCS` run; changing a signature or body does not.

use crate::backend::connection::{HubConnection, HubConnectionStatus};
use crate::backend::contract::{MAX_HUB_CONTRACT, MIN_HUB_CONTRACT};
use crate::backend::Backend;
use fleet_core::ipc_error::lock;
use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::mcp::McpRuntime;
use fleet_core::service::diagnostics::{self, DiagnosticsBundle, DiagnosticsInputs};
use fleet_core::service::tunnel::TunnelSupervisor;
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{Manager, State};

/// The platform app data directory, resolved once at startup (`lib.rs`
/// `run`) and managed as Tauri state. The resolver panics on failure, which
/// is only acceptable at startup, so IPC handlers read this instead.
pub struct AppDataDir(pub PathBuf);

/// The managed data dir, or `E_INTERNAL` if setup never managed it.
fn data_dir_from(state: Option<&AppDataDir>) -> Result<PathBuf, IpcError> {
    state.map(|d| d.0.clone()).ok_or_else(|| {
        IpcError::new(
            codes::E_INTERNAL,
            "the app data directory was not resolved at startup",
        )
    })
}

fn managed_data_dir(app: &tauri::AppHandle) -> Result<PathBuf, IpcError> {
    data_dir_from(app.try_state::<AppDataDir>().as_deref())
}

/// The `== SSH ==` section: ControlMaster resets after a wedged command
/// since launch, in total and per host.
fn ssh_section(total: usize, per_host: &BTreeMap<String, usize>) -> String {
    let mut t = String::new();
    let _ = writeln!(t, "== SSH ==");
    let _ = writeln!(t, "master_resets_since_launch: {total}");
    for (host, n) in per_host {
        let _ = writeln!(t, "{host}: master_resets={n}");
    }
    let _ = writeln!(t);
    t
}

/// One line for a [`HubConnection`], with the numbers a reader needs to tell
/// a hub that is down from one this build cannot talk to.
fn connection_line(c: &HubConnection) -> String {
    match c {
        HubConnection::Standalone => "standalone".into(),
        HubConnection::Connecting => "connecting".into(),
        HubConnection::Connected => "connected".into(),
        HubConnection::Reconnecting {
            attempt,
            retry_in_secs,
            reason,
        } => format!("reconnecting attempt={attempt} retry_in={retry_in_secs}s reason={reason}"),
        HubConnection::Offline {
            attempt,
            retry_in_secs,
            reason,
        } => format!("offline attempt={attempt} retry_in={retry_in_secs}s reason={reason}"),
        HubConnection::HubTooOld {
            hub_contract,
            min_contract,
        } => format!(
            "hub_too_old hub_contract={hub_contract} min_contract={min_contract} (update the hub)"
        ),
        HubConnection::HubTooNew {
            hub_contract,
            max_contract,
        } => format!(
            "hub_too_new hub_contract={hub_contract} max_contract={max_contract} (update this app)"
        ),
    }
}

/// The `== Hub ==` section: which fleet this process is a window onto, and,
/// as a hub client, whether its link to the hub is up.
///
/// Without it a bundle from a desktop paired with a hub reads like a
/// standalone one with no hosts — the one mode whose bug reports most need
/// to say which mode it is. Nothing here is secret: the URL is the
/// normalised one (no userinfo, query or fragment, see
/// `backend::normalise_base_url`), the reasons are scrubbed of the token by
/// `HubConnectionStatus`, and the caller masks the client token on top.
fn hub_section(
    backend: &Backend,
    connection: &HubConnection,
    contract_verdict: Option<&HubConnection>,
) -> String {
    let mut t = String::new();
    let _ = writeln!(t, "== Hub ==");
    match backend {
        Backend::Local => {
            let _ = writeln!(t, "mode: standalone");
        }
        Backend::Remote(cfg) => {
            let _ = writeln!(t, "mode: client");
            let _ = writeln!(t, "url: {}", cfg.base_url);
            let _ = writeln!(t, "client_name: {}", cfg.client_name);
            let _ = writeln!(t, "connection: {}", connection_line(connection));
            let _ = writeln!(
                t,
                "contract_verdict: {}",
                contract_verdict
                    .map(connection_line)
                    .unwrap_or_else(|| "none".into())
            );
        }
        Backend::Unavailable(hub) => {
            let _ = writeln!(t, "mode: unavailable (managing no fleet)");
            let _ = writeln!(t, "url: {}", hub.url.as_deref().unwrap_or("(unusable)"));
            let _ = writeln!(t, "reason: {}", hub.reason);
        }
    }
    let _ = writeln!(
        t,
        "contract_accepted: {MIN_HUB_CONTRACT}..={MAX_HUB_CONTRACT}"
    );
    let _ = writeln!(t);
    t
}

/// Headings of the log sections `service::diagnostics` renders last: the
/// earlier warnings and errors (only when there are some), then the tail.
const LOG_HEADINGS: [&str; 2] = ["== Earlier warnings and errors", "== Recent log"];

/// Insert `section` just before the log sections, so the log stays at the
/// end. Appends when the text has none. The earliest heading wins, and it is
/// found before any log line: log lines only come after it.
fn insert_before_log_tail(text: &mut String, section: &str) {
    let at = LOG_HEADINGS
        .iter()
        .filter_map(|h| text.find(&format!("\n{h}")))
        .min();
    match at {
        Some(i) => text.insert_str(i + 1, section),
        None => {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(section);
        }
    }
}

/// Build the redacted plain-text diagnostics bundle (see
/// `service::diagnostics`), plus the hub-client state and the SSH
/// ControlMaster reset counters. Reads cached state only; no network.
///
/// `async`, with the work on a blocking thread: it reads up to a few MB of
/// log files, and a sync command runs on the macOS main thread (CLAUDE.md:
/// no blocking I/O on a sync Tauri command).
///
/// **Not guarded in remote mode, deliberately.** The rule elsewhere in
/// `commands/` is that a command answering from the local `Store` refuses
/// when a hub owns the fleet, because the local database is not that fleet.
/// This bundle is the exception: it describes *this process* — its log tail,
/// its tunnels, its SSH counters, its own database — and it is the first
/// thing anyone asks for when remote mode misbehaves. Refusing it would make
/// the mode that most needs a bug report the one that cannot produce one.
#[tauri::command]
pub async fn collect_diagnostics(
    app: tauri::AppHandle,
    store: State<'_, Arc<Mutex<Store>>>,
    tunnels: State<'_, Arc<TunnelSupervisor>>,
    runtime: State<'_, Mutex<McpRuntime>>,
    ssh: State<'_, Arc<SshClient>>,
    backend: State<'_, Backend>,
    hub_link: State<'_, Arc<HubConnectionStatus>>,
) -> Result<DiagnosticsBundle, IpcError> {
    let data_dir = managed_data_dir(&app)?;
    let (mcp_running, mcp_bind_error) = {
        let rt = lock(&runtime)?;
        (rt.is_running(), rt.last_error().map(str::to_string))
    };
    let store = Arc::clone(&store);
    let tunnels = tunnels.health();
    let ssh_text = ssh_section(ssh.master_reset_count(), &ssh.master_reset_counts());
    let hub_text = hub_section(
        &backend,
        &hub_link.current(),
        hub_link.contract_verdict().as_ref(),
    );
    // The hub client token lives in the OS keychain, not the store, so the
    // bundle's own pass does not know it. Mask it here, over everything.
    let client_token: Vec<String> = backend
        .remote()
        .map(|c| c.token.clone())
        .into_iter()
        .collect();
    tauri::async_runtime::spawn_blocking(move || {
        let log_dir = fleet_core::logging::log_dir_in(&data_dir);
        let mut bundle = diagnostics::collect(
            &store,
            DiagnosticsInputs {
                data_dir: &data_dir,
                log_dir: &log_dir,
                tunnels,
                mcp_running,
                mcp_bind_error,
            },
        )?;
        insert_before_log_tail(&mut bundle.text, &hub_text);
        // Host aliases and counts only: nothing here needs redaction.
        insert_before_log_tail(&mut bundle.text, &ssh_text);
        bundle.text = fleet_core::logging::redact_secrets(&bundle.text, &client_token);
        Ok(bundle)
    })
    .await
    .map_err(|e| IpcError::new(codes::E_INTERNAL, format!("diagnostics task failed: {e}")))?
}

/// Open the log folder in the OS file manager. Returns the folder path so
/// the UI can also show it (and offer a copy) when opening fails.
///
/// Same in both modes: the folder is this app's, and it has one either way.
/// `async` for the same reason as `collect_diagnostics`: it touches the disk.
#[tauri::command]
pub async fn open_log_folder(app: tauri::AppHandle) -> Result<String, IpcError> {
    use tauri_plugin_opener::OpenerExt;
    let dir = fleet_core::logging::log_dir_in(&managed_data_dir(&app)?);
    std::fs::create_dir_all(&dir)?;
    let path = dir.display().to_string();
    app.opener()
        .open_path(path.clone(), None::<&str>)
        .map_err(|e| IpcError::new(codes::E_IO, format!("could not open {path}: {e}")))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Store::open_in_memory` is a `#[cfg(test)]` helper of `fleet_core`, so
    /// it is not visible from this crate's tests; open a throwaway file-backed
    /// store through the public constructor instead.
    fn open_temp_store(dir: &std::path::Path) -> Store {
        Store::open_with_bus(
            &dir.join("state.db"),
            std::sync::Arc::new(fleet_core::events::NoopEventBus),
        )
        .expect("open store")
    }

    #[test]
    fn data_dir_comes_from_managed_state() {
        let dir = AppDataDir(PathBuf::from("/tmp/claude-fleet-data"));
        assert_eq!(
            data_dir_from(Some(&dir)).unwrap(),
            PathBuf::from("/tmp/claude-fleet-data")
        );
    }

    #[test]
    fn missing_data_dir_is_an_ipc_error_not_a_panic() {
        let err = data_dir_from(None).unwrap_err();
        assert_eq!(err.code, codes::E_INTERNAL);
        assert!(err.message.contains("data directory"), "{}", err.message);
    }

    #[test]
    fn ssh_section_reports_total_and_per_host_resets() {
        let none = ssh_section(0, &BTreeMap::new());
        assert_eq!(none, "== SSH ==\nmaster_resets_since_launch: 0\n\n");

        let per_host = BTreeMap::from([("mefistos".to_string(), 2), ("turanga".to_string(), 1)]);
        let some = ssh_section(3, &per_host);
        assert_eq!(
            some,
            "== SSH ==\nmaster_resets_since_launch: 3\n\
             mefistos: master_resets=2\nturanga: master_resets=1\n\n"
        );
    }

    #[test]
    fn a_standalone_app_says_so() {
        let s = hub_section(&Backend::Local, &HubConnection::Standalone, None);
        assert!(s.starts_with("== Hub ==\nmode: standalone\n"), "{s}");
        assert!(s.contains(&format!(
            "contract_accepted: {MIN_HUB_CONTRACT}..={MAX_HUB_CONTRACT}"
        )));
        assert!(!s.contains("url:"), "{s}");
    }

    #[test]
    fn a_hub_client_reports_its_hub_and_link() {
        let backend = Backend::Remote(crate::backend::RemoteConfig {
            base_url: "https://fleet.example.com".into(),
            token: "cl_secret_token_value".into(),
            client_name: "laptop".into(),
        });
        let s = hub_section(
            &backend,
            &HubConnection::Offline {
                attempt: 4,
                retry_in_secs: 16,
                reason: "connection refused".into(),
            },
            Some(&HubConnection::HubTooOld {
                hub_contract: 5,
                min_contract: 6,
            }),
        );
        assert!(s.contains("mode: client\n"), "{s}");
        assert!(s.contains("url: https://fleet.example.com\n"), "{s}");
        assert!(s.contains("client_name: laptop\n"), "{s}");
        assert!(
            s.contains("connection: offline attempt=4 retry_in=16s reason=connection refused\n"),
            "{s}"
        );
        assert!(
            s.contains("contract_verdict: hub_too_old hub_contract=5 min_contract=6"),
            "{s}"
        );
        assert!(!s.contains("cl_secret_token_value"), "{s}");
    }

    #[test]
    fn an_unavailable_hub_reports_why() {
        let backend = Backend::Unavailable(crate::backend::UnavailableHub {
            url: Some("https://fleet.example.com".into()),
            reason: "no client token is stored".into(),
        });
        let s = hub_section(&backend, &HubConnection::Standalone, None);
        assert!(s.contains("mode: unavailable (managing no fleet)\n"), "{s}");
        assert!(s.contains("url: https://fleet.example.com\n"), "{s}");
        assert!(s.contains("reason: no client token is stored\n"), "{s}");

        let unusable = Backend::Unavailable(crate::backend::UnavailableHub {
            url: None,
            reason: "bad url".into(),
        });
        let s = hub_section(&unusable, &HubConnection::Standalone, None);
        assert!(s.contains("url: (unusable)\n"), "{s}");
    }

    #[test]
    fn every_connection_state_renders_one_line() {
        for c in [
            HubConnection::Standalone,
            HubConnection::Connecting,
            HubConnection::Connected,
            HubConnection::Reconnecting {
                attempt: 1,
                retry_in_secs: 2,
                reason: "eof".into(),
            },
            HubConnection::HubTooNew {
                hub_contract: 9,
                max_contract: 6,
            },
        ] {
            let line = connection_line(&c);
            assert!(!line.is_empty() && !line.contains('\n'), "{c:?}: {line}");
        }
        assert_eq!(
            connection_line(&HubConnection::HubTooNew {
                hub_contract: 9,
                max_contract: 6
            }),
            "hub_too_new hub_contract=9 max_contract=6 (update this app)"
        );
    }

    #[test]
    fn ssh_section_goes_before_the_log_tail() {
        let mut text = String::from(
            "== Sessions (0 total; status/claude_status) ==\n\n\
             == Recent log (last 1 lines) ==\n\
             INFO line mentioning == Recent log\n",
        );
        insert_before_log_tail(&mut text, "== SSH ==\nx\n\n");
        assert_eq!(
            text,
            "== Sessions (0 total; status/claude_status) ==\n\n\
             == SSH ==\nx\n\n\
             == Recent log (last 1 lines) ==\n\
             INFO line mentioning == Recent log\n"
        );
    }

    #[test]
    fn sections_go_before_the_earlier_problems_too() {
        let mut text = String::from(
            "== Sessions ==\n\n\
             == Earlier warnings and errors (3 before the tail) ==\n\
             x ERROR t: line\n\n\
             == Recent log (last 1 lines) ==\n\
             y\n",
        );
        insert_before_log_tail(&mut text, "== Hub ==\n\n");
        insert_before_log_tail(&mut text, "== SSH ==\n\n");
        assert_eq!(
            text,
            "== Sessions ==\n\n== Hub ==\n\n== SSH ==\n\n\
             == Earlier warnings and errors (3 before the tail) ==\n\
             x ERROR t: line\n\n\
             == Recent log (last 1 lines) ==\n\
             y\n"
        );
    }

    #[test]
    fn ssh_section_is_appended_without_a_log_tail() {
        let mut text = String::from("== App ==\nversion: 1");
        insert_before_log_tail(&mut text, "== SSH ==\n");
        assert_eq!(text, "== App ==\nversion: 1\n== SSH ==\n");
    }

    #[test]
    fn real_bundle_gets_the_ssh_section_before_its_log_tail() {
        // The bundle stamps `app_version::get()`, which panics until the
        // binary declares its version; a test binary never runs `run()`.
        crate::declare_app_version();
        let tmp = tempfile::tempdir().unwrap();
        let logs = fleet_core::logging::log_dir_in(tmp.path());
        let store = Mutex::new(open_temp_store(tmp.path()));
        let mut b = diagnostics::collect(
            &store,
            DiagnosticsInputs {
                data_dir: tmp.path(),
                log_dir: &logs,
                tunnels: Default::default(),
                mcp_running: false,
                mcp_bind_error: None,
            },
        )
        .unwrap();
        let c = SshClient::new();
        insert_before_log_tail(
            &mut b.text,
            &ssh_section(c.master_reset_count(), &c.master_reset_counts()),
        );
        let ssh = b.text.find("== SSH ==").expect("ssh section present");
        let log = b.text.find("== Recent log").expect("log tail present");
        assert!(ssh < log, "{}", b.text);
        assert!(b.text.contains("master_resets_since_launch: 0"));
    }
}
