mod cancel;
mod claude_agents;
mod claude_cli;
mod commands;
mod events;
mod ipc_error;
mod mcp;
mod projects;
mod pty;
mod service;
mod shell;
mod ssh;
mod ssh_config;
mod store;
mod tmux;
mod validate;

pub use events::{AppHandleEventBus, EventBus, NoopEventBus};

use directories::ProjectDirs;
use pty::PtyState;
use std::sync::Mutex;
use store::Store;

fn appdata_db_path() -> std::path::PathBuf {
    let dirs = ProjectDirs::from("sk", "rlt", "claude-fleet")
        .expect("could not resolve platform appdata dir");
    let dir = dirs.data_dir();
    std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create appdata dir {dir:?}: {e}"));
    dir.join("state.db")
}

/// Pure: compute a new PATH that appends any of `common_bin_dirs` that are not
/// already in `current` and that `dir_exists` reports as present. Returns
/// `None` if nothing would change.
fn compute_backfilled_path(
    current: &str,
    common_bin_dirs: &[&str],
    dir_exists: impl Fn(&str) -> bool,
) -> Option<String> {
    let parts: Vec<&str> = current.split(':').filter(|p| !p.is_empty()).collect();
    let additions: Vec<&str> = common_bin_dirs
        .iter()
        .copied()
        .filter(|d| !parts.contains(d) && dir_exists(d))
        .collect();
    if additions.is_empty() {
        return None;
    }
    let mut new_parts: Vec<String> = parts.iter().map(|s| s.to_string()).collect();
    new_parts.extend(additions.iter().map(|s| s.to_string()));
    Some(new_parts.join(":"))
}

/// Run the user's login shell once and adopt several env vars that a
/// Finder-launched GUI app does not inherit by default:
///
///   - PATH    — catches Homebrew, dotfiles bin/, fnm/nvm/asdf/mise, and
///     custom per-user wrappers like `cl`.
///   - LANG / LC_ALL / LC_CTYPE — locale. Without these, claude and other
///     TUIs detect a non-UTF-8 terminal and render ASCII fallbacks
///     (`_` instead of `└` / `↑` / `█` etc.).
///
/// One shell invocation prints the values delimited by `\x1f` (US sep) and
/// terminated by `\x1e` (RS) per variable. We parse and call set_var on each
/// non-empty value. Best-effort: any failure leaves the var unchanged.
fn import_login_shell_env() -> bool {
    let Ok(shell) = std::env::var("SHELL") else {
        return false;
    };
    // Order MUST match VARS below.
    const VARS: &[&str] = &["PATH", "LANG", "LC_ALL", "LC_CTYPE"];
    let script = VARS
        .iter()
        .map(|v| format!("printf '%s\\x1e' \"${v}\""))
        .collect::<Vec<_>>()
        .join("; ");
    let Ok(output) = std::process::Command::new(&shell)
        .args(["-l", "-c", &script])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut parts = stdout.split('\x1e');
    let mut any_set = false;
    for var in VARS {
        let Some(val) = parts.next() else { break };
        let trimmed = val.trim();
        if trimmed.is_empty() {
            continue;
        }
        std::env::set_var(var, trimmed);
        any_set = true;
    }
    any_set
}

/// When launched from Finder (Spotlight, Dock, double-click), macOS hands the
/// app a minimal PATH that does NOT include Homebrew (`/opt/homebrew/bin` on
/// Apple Silicon, `/usr/local/bin` on Intel). Without this fix, every shelled
/// command — `tmux`, `git` — fails with "binary not found on PATH".
///
/// Backfill the common locations once at startup. We append (not prepend) so
/// anything the user has explicitly set wins for ambiguous cases.
/// Ensure LANG points at a UTF-8 locale so spawned PTYs (and the shell-detection
/// inside claude/tmux) treat the terminal as Unicode-capable. macOS ships
/// en_US.UTF-8; we use C.UTF-8 as a portable fallback. Only writes if no UTF-8
/// locale is already present in any of LC_ALL / LANG / LC_CTYPE — we never
/// override an explicit user choice.
fn backfill_locale_for_gui_launch() {
    let has_utf8 = ["LC_ALL", "LANG", "LC_CTYPE"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .any(|v| {
            v.to_ascii_uppercase().contains("UTF-8") || v.to_ascii_uppercase().contains("UTF8")
        });
    if has_utf8 {
        return;
    }
    std::env::set_var("LANG", "en_US.UTF-8");
}

/// True when the process already has a Homebrew bin dir on PATH and a UTF-8
/// locale — the signature of a terminal launch, where the (100-500 ms)
/// login-shell import is redundant. Errs toward `false` (run the import) when
/// unsure: claude-fleet only shells out to `ssh`/`git`/`tmux`, which live in
/// the standard bin dirs `backfill_path_for_gui_launch` guarantees anyway.
fn env_looks_complete() -> bool {
    let path = std::env::var("PATH").unwrap_or_default();
    let has_brew = path
        .split(':')
        .any(|p| p == "/opt/homebrew/bin" || p == "/usr/local/bin");
    let has_utf8 = ["LC_ALL", "LANG", "LC_CTYPE"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .any(|v| {
            let u = v.to_ascii_uppercase();
            u.contains("UTF-8") || u.contains("UTF8")
        });
    has_brew && has_utf8
}

fn backfill_path_for_gui_launch() {
    const COMMON_BIN_DIRS: &[&str] = &[
        "/opt/homebrew/bin", // Apple Silicon Homebrew
        "/usr/local/bin",    // Intel Homebrew
        "/usr/bin",
        "/bin",
    ];
    let current = std::env::var("PATH").unwrap_or_default();
    if let Some(new_path) = compute_backfilled_path(&current, COMMON_BIN_DIRS, |d| {
        std::path::Path::new(d).exists()
    }) {
        std::env::set_var("PATH", new_path);
    }
}

#[tauri::command]
async fn cancel_command(
    call_id: u64,
    reg: tauri::State<'_, std::sync::Arc<cancel::CancellationRegistry>>,
) -> Result<(), crate::ipc_error::IpcError> {
    reg.cancel(call_id);
    Ok(())
}

/// Read the MCP control-API settings and, if the user enabled it, start the
/// server, recording the outcome in the managed `McpRuntime`. Generates and
/// persists a bearer token on first enable. Off by default — a fresh install
/// never opens a listener.
fn maybe_start_mcp(
    app: &tauri::AppHandle,
    store: &std::sync::Arc<Mutex<Store>>,
    ssh: &std::sync::Arc<ssh::SshClient>,
    reg: &std::sync::Arc<cancel::CancellationRegistry>,
) {
    use tauri::Manager;
    let (enabled, port, token) = {
        let Ok(s) = store.lock() else {
            return;
        };
        let enabled = s
            .get_setting(mcp::SETTING_ENABLED)
            .ok()
            .flatten()
            .as_deref()
            == Some("true");
        let port = s
            .get_setting(mcp::SETTING_PORT)
            .ok()
            .flatten()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(mcp::DEFAULT_PORT);
        let token = s.get_setting(mcp::SETTING_TOKEN).ok().flatten();
        (enabled, port, token)
    };
    if !enabled {
        return;
    }
    // Ensure a token exists before the listener binds — never a tokenless API.
    let token = match token {
        Some(t) if !t.is_empty() => t,
        _ => {
            let fresh = mcp::generate_token();
            if let Ok(s) = store.lock() {
                let _ = s.set_setting(mcp::SETTING_TOKEN, &fresh);
            }
            fresh
        }
    };
    let result = tauri::async_runtime::block_on(mcp::start(
        std::sync::Arc::clone(store),
        std::sync::Arc::clone(ssh),
        std::sync::Arc::clone(reg),
        port,
        token,
    ));
    if let Some(runtime) = app.try_state::<Mutex<mcp::McpRuntime>>() {
        if let Ok(mut rt) = runtime.lock() {
            match result {
                Ok(shutdown) => rt.set_running(shutdown),
                Err(e) => {
                    eprintln!("[mcp] {e}");
                    rt.set_error(e);
                }
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Layered env recovery for Finder-launched apps:
    //   1. Import the user's full login-shell env (PATH + locale). PATH
    //      catches Homebrew/dotfiles/etc.; LANG/LC_ALL/LC_CTYPE prevent
    //      claude and other TUIs from rendering ASCII fallbacks because
    //      they think the terminal is non-UTF-8.
    //   2. Belt-and-suspenders PATH backfill ensures the standard macOS
    //      bin dirs are present even if step 1 failed or returned a
    //      stunted PATH.
    //   3. Force LANG to a sensible UTF-8 default if both step 1 and the
    //      OS env left it empty.
    //
    // Step 1 spawns a login shell (~100-500ms). Skip it when the env already
    // looks like a terminal launch — the common dev case — so startup stays
    // snappy; the backfills below still run as a safety net.
    if !env_looks_complete() {
        import_login_shell_env();
    }
    backfill_path_for_gui_launch();
    backfill_locale_for_gui_launch();

    let ssh_client = std::sync::Arc::new(ssh::SshClient::new());
    let ssh_client_for_exit = std::sync::Arc::clone(&ssh_client);
    let ssh_client_for_setup = std::sync::Arc::clone(&ssh_client);
    let reg = cancel::CancellationRegistry::new();
    let reg_for_setup = std::sync::Arc::clone(&reg);

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            use tauri::Manager;
            let handle = app.handle().clone();
            let bus: std::sync::Arc<dyn crate::events::EventBus> =
                std::sync::Arc::new(crate::events::AppHandleEventBus::new(handle));
            let db_path = appdata_db_path();
            let store = Store::open_with_bus(&db_path, bus).unwrap_or_else(|e| {
                // Still a hard fail (the app can't run without its DB), but
                // with an actionable message instead of a bare "open store".
                panic!(
                    "failed to open the claude-fleet database at {}: {e}\n\
                     If the file is corrupt, deleting it resets all local \
                     state — hosts, projects and sessions are re-discovered \
                     on the next launch.",
                    db_path.display()
                )
            });
            // Managed as Arc<Mutex<Store>> (not bare Mutex<Store>) so the
            // embedded MCP server can hold a clone of the same store handle.
            let store = std::sync::Arc::new(Mutex::new(store));
            app.manage(std::sync::Arc::clone(&store));
            app.manage(Mutex::new(mcp::McpRuntime::default()));
            // Start the MCP control API if the user has enabled it (off by
            // default). Reuses the same Store / SshClient / registry as the UI.
            maybe_start_mcp(app.handle(), &store, &ssh_client_for_setup, &reg_for_setup);
            Ok(())
        })
        .manage(Mutex::new(PtyState::new()))
        .manage(ssh_client)
        .manage(reg)
        .invoke_handler(tauri::generate_handler![
            commands::health::health_check,
            commands::projects::list_projects,
            commands::projects::refresh_projects,
            commands::sessions::list_sessions,
            commands::sessions::related_sessions,
            commands::sessions::new_session,
            commands::sessions::kill_session,
            commands::sessions::rename_session,
            commands::sessions::restart_session,
            commands::sessions::send_prompt,
            commands::sessions::spawn_review,
            commands::sessions::recreate_session,
            commands::sessions::dismiss_ghost_session,
            commands::sessions::new_bg_session,
            commands::sessions::peek_session,
            commands::sessions::purge_project,
            commands::files::repo_changes,
            commands::files::repo_tree,
            commands::files::repo_file,
            commands::files::repo_diff,
            commands::upload::upload_to_session,
            commands::history::repo_log,
            commands::history::repo_branches,
            commands::history::repo_commit,
            commands::history::repo_commit_diff,
            commands::mutate::repo_checkout,
            commands::mutate::repo_checkout_commit,
            commands::mutate::repo_create_branch,
            commands::mutate::repo_delete_branch,
            commands::mutate::repo_stage,
            commands::mutate::repo_unstage,
            commands::mutate::repo_commit_create,
            commands::mutate::repo_fetch,
            commands::mutate::repo_pull,
            commands::mutate::repo_push,
            commands::hosts::discover_hosts,
            commands::hosts::list_hosts,
            commands::hosts::list_accounts,
            commands::hosts::add_host,
            commands::hosts::probe_host,
            commands::hosts::probe_ssh_alias,
            commands::hosts::remove_host,
            commands::hosts::hide_host,
            commands::mcp::mcp_status,
            commands::mcp::mcp_configure,
            commands::mcp::install_fleet_hook,
            pty::pty_open,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_close,
            pty::pty_drain,
            cancel_command,
        ])
        .on_window_event(move |window, event| {
            // On exit: close ssh masters AND any open PTY, so we don't leak
            // background ssh processes or an orphaned `tmux attach` / `ssh
            // -tt` child after quit.
            if let tauri::WindowEvent::Destroyed = event {
                use tauri::Manager;
                ssh_client_for_exit.shutdown_all();
                if let Some(runtime) = window.try_state::<Mutex<mcp::McpRuntime>>() {
                    if let Ok(mut rt) = runtime.lock() {
                        rt.stop();
                    }
                }
                if let Some(pty) = window.try_state::<Mutex<PtyState>>() {
                    if let Ok(mut s) = pty.lock() {
                        s.close();
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod path_backfill_tests {
    use super::*;

    #[test]
    fn appends_missing_dirs_at_end() {
        let result =
            compute_backfilled_path("/usr/bin", &["/opt/homebrew/bin", "/usr/bin"], |_| true)
                .unwrap();
        assert_eq!(result, "/usr/bin:/opt/homebrew/bin");
    }

    #[test]
    fn does_not_add_nonexistent_dirs() {
        let result = compute_backfilled_path("/usr/bin", &["/this/does/not/exist"], |_| false);
        assert!(result.is_none());
    }

    #[test]
    fn no_change_when_all_present() {
        let result = compute_backfilled_path(
            "/opt/homebrew/bin:/usr/bin",
            &["/opt/homebrew/bin", "/usr/bin"],
            |_| true,
        );
        assert!(result.is_none());
    }

    #[test]
    fn skips_empty_path_components() {
        let result =
            compute_backfilled_path("/usr/bin::", &["/opt/homebrew/bin"], |_| true).unwrap();
        assert_eq!(result, "/usr/bin:/opt/homebrew/bin");
    }

    #[test]
    fn handles_empty_path() {
        let result =
            compute_backfilled_path("", &["/opt/homebrew/bin", "/usr/bin"], |_| true).unwrap();
        assert_eq!(result, "/opt/homebrew/bin:/usr/bin");
    }

    /// Pure helper for the locale-backfill decision used by
    /// `backfill_locale_for_gui_launch`. Lifted out for testability —
    /// the real fn touches std::env which makes parallel tests racy.
    fn needs_locale_backfill(lc_all: &str, lang: &str, lc_ctype: &str) -> bool {
        ![lc_all, lang, lc_ctype].iter().any(|v| {
            v.to_ascii_uppercase().contains("UTF-8") || v.to_ascii_uppercase().contains("UTF8")
        })
    }

    #[test]
    fn locale_backfill_triggers_when_all_empty() {
        assert!(needs_locale_backfill("", "", ""));
    }

    #[test]
    fn locale_backfill_skipped_when_lang_is_utf8() {
        assert!(!needs_locale_backfill("", "en_US.UTF-8", ""));
        assert!(!needs_locale_backfill("", "C.UTF-8", ""));
        assert!(!needs_locale_backfill("", "sk_SK.utf8", "")); // case-insensitive
    }

    #[test]
    fn locale_backfill_skipped_when_lc_all_is_utf8() {
        assert!(!needs_locale_backfill("en_US.UTF-8", "C", ""));
    }

    #[test]
    fn locale_backfill_triggers_when_only_c() {
        // Plain POSIX C locale isn't UTF-8 — we should still backfill.
        assert!(needs_locale_backfill("C", "C", "C"));
        assert!(needs_locale_backfill("POSIX", "POSIX", ""));
    }
}
