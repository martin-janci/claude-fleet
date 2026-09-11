//! Tauri IPC wrappers for Settings → Diagnostics. The report itself is built
//! in `service::diagnostics`; this file only gathers managed state. These
//! are Tauri commands, not MCP tools: `docs/control-api-reference.md` is
//! generated from the MCP tool router only, so changing them needs no
//! `REGEN_DOCS` run.

use crate::ipc_error::{codes, IpcError};
use crate::mcp::McpRuntime;
use crate::service::diagnostics::{self, DiagnosticsBundle, DiagnosticsInputs};
use crate::service::tunnel::TunnelSupervisor;
use crate::ssh::SshClient;
use crate::store::Store;
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

/// Heading of the log-tail section `service::diagnostics` renders last.
const LOG_TAIL_HEADING: &str = "== Recent log";

/// Insert `section` just before the log tail, so the log stays the final
/// section. Appends when the text has no log-tail heading. The first match
/// is the heading itself: log lines only come after it.
fn insert_before_log_tail(text: &mut String, section: &str) {
    match text.find(&format!("\n{LOG_TAIL_HEADING}")) {
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
/// `service::diagnostics`), plus the SSH ControlMaster reset counters.
/// Reads cached state only; no network.
#[tauri::command]
pub fn collect_diagnostics(
    app: tauri::AppHandle,
    store: State<'_, Arc<Mutex<Store>>>,
    tunnels: State<'_, Arc<TunnelSupervisor>>,
    runtime: State<'_, Mutex<McpRuntime>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<DiagnosticsBundle, IpcError> {
    let data_dir = managed_data_dir(&app)?;
    let log_dir = crate::logging::log_dir_in(&data_dir);
    let (mcp_running, mcp_bind_error) = {
        let rt = runtime.lock().map_err(|_| IpcError::lock())?;
        (rt.is_running(), rt.last_error().map(str::to_string))
    };
    let mut bundle = diagnostics::collect(
        &store,
        DiagnosticsInputs {
            data_dir: &data_dir,
            log_dir: &log_dir,
            tunnels: tunnels.snapshot(),
            mcp_running,
            mcp_bind_error,
        },
    )?;
    // Host aliases and counts only: nothing here needs redaction.
    insert_before_log_tail(
        &mut bundle.text,
        &ssh_section(ssh.master_reset_count(), &ssh.master_reset_counts()),
    );
    Ok(bundle)
}

/// Open the log folder in the OS file manager. Returns the folder path so
/// the UI can also show it (and offer a copy) when opening fails.
#[tauri::command]
pub fn open_log_folder(app: tauri::AppHandle) -> Result<String, IpcError> {
    use tauri_plugin_opener::OpenerExt;
    let dir = crate::logging::log_dir_in(&managed_data_dir(&app)?);
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
    fn ssh_section_is_appended_without_a_log_tail() {
        let mut text = String::from("== App ==\nversion: 1");
        insert_before_log_tail(&mut text, "== SSH ==\n");
        assert_eq!(text, "== App ==\nversion: 1\n== SSH ==\n");
    }

    #[test]
    fn real_bundle_gets_the_ssh_section_before_its_log_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = crate::logging::log_dir_in(tmp.path());
        let store = Mutex::new(Store::open_in_memory().unwrap());
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
