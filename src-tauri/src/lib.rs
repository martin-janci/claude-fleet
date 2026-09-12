mod bootstrap;
mod cancel;
mod claude_agents;
mod claude_cli;
mod commands;
mod events;
#[cfg(test)]
mod fleet_e2e_tests;
mod humanize;
mod ipc_error;
mod local_exec;
mod logging;
mod mcp;
#[cfg(test)]
mod no_eprintln_tests;
mod projects;
mod pty;
mod repo_url;
mod service;
mod shell;
mod ssh;
mod ssh_config;
#[cfg(test)]
mod ssh_fake;
mod store;
mod tmux;
mod validate;

pub use events::{AppHandleEventBus, EventBus, NoopEventBus};

use bootstrap::env::{
    appdata_dir, backfill_locale_for_gui_launch, backfill_path_for_gui_launch, env_looks_complete,
    import_login_shell_env,
};
use bootstrap::mcp::maybe_start_mcp;
use bootstrap::singleton::kill_other_instances;
use pty::PtyState;
use service::tick::spawn_reconcile_tick;
use std::sync::Mutex;
use store::Store;

#[tauri::command]
async fn cancel_command(
    call_id: u64,
    reg: tauri::State<'_, std::sync::Arc<cancel::CancellationRegistry>>,
) -> Result<(), crate::ipc_error::IpcError> {
    reg.cancel(call_id);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // File logging first, so the instance reaper and env recovery below are
    // captured too. A failure is non-fatal: the app runs without a log file.
    let data_dir = appdata_dir();
    let log_dir = match logging::init(&data_dir) {
        Ok(dir) => Some(dir),
        Err(e) => {
            // `init` failed before installing a subscriber: install the stderr
            // fallback first, or this line would go nowhere.
            logging::init_stderr_fallback();
            tracing::error!(error = %e, "[startup] file logging unavailable; logging to stderr only");
            None
        }
    };

    // Win the singleton race before opening the DB or binding the MCP port:
    // kill any other running instance of this app (any build).
    kill_other_instances();

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
    let tunnels = std::sync::Arc::new(crate::service::tunnel::TunnelSupervisor::new());
    let tunnels_for_exit = std::sync::Arc::clone(&tunnels);
    let tunnels_for_setup = std::sync::Arc::clone(&tunnels);
    // SEC-9: the only local paths `upload_to_session` may read are the ones
    // the OS drag-drop handed the window (recorded below in the window /
    // webview event handlers).
    let upload_allow = std::sync::Arc::new(commands::upload::UploadAllowList::new());
    let upload_allow_for_window = std::sync::Arc::clone(&upload_allow);
    let upload_allow_for_webview = std::sync::Arc::clone(&upload_allow);

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(move |app| {
            use tauri::Manager;
            let handle = app.handle().clone();
            let bus: std::sync::Arc<dyn crate::events::EventBus> =
                std::sync::Arc::new(crate::events::AppHandleEventBus::new(handle));
            // The data dir was resolved once, before logging started; IPC
            // handlers read it from managed state instead of re-resolving.
            app.manage(commands::diagnostics::AppDataDir(data_dir.clone()));
            let db_path = data_dir.join("state.db");
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
            // SEC-11: the DB holds bearer tokens and account metadata in
            // plaintext — keep it owner-only. Best-effort, logged on failure.
            crate::service::provision::set_private_mode(&db_path);
            tracing::info!(
                version = env!("CARGO_PKG_VERSION"),
                schema_version = store.schema_version().unwrap_or(0),
                os = std::env::consts::OS,
                data_dir = %data_dir.display(),
                log_dir = %log_dir
                    .as_deref()
                    .map(|d| d.display().to_string())
                    .unwrap_or_else(|| "(file logging unavailable)".into()),
                "claude-fleet starting"
            );
            // Destructive-call confirmations reach the desktop as a Tauri
            // event; the frontend answers via `mcp_confirm`.
            let confirm_handle = app.handle().clone();
            let guards = mcp::McpGuards::new(std::sync::Arc::new(move |req| {
                let _ = tauri::Emitter::emit(&confirm_handle, "mcp:confirm-required", req);
            }));
            app.manage(guards.clone());
            // Managed as Arc<Mutex<Store>> (not bare Mutex<Store>) so the
            // embedded MCP server can hold a clone of the same store handle.
            let store = std::sync::Arc::new(Mutex::new(store));
            // One-shot deterministic friendly-name backfill: any session row
            // that pre-dates the agent-driven labelling (or whose agent never
            // labelled it) gets a humanised branch name so the sidebar isn't
            // dominated by raw `dev-<owner>-<repo>--…` slugs. Runs BEFORE the
            // reconcile tick / MCP server so we don't race the frontend's row
            // subscription. Best-effort — a poisoned mutex here means the app
            // is already in trouble and the sidebar fallback to tmux_name
            // remains the safety net.
            if let Ok(s) = store.lock() {
                match s.backfill_friendly_names() {
                    Ok(0) => {}
                    Ok(n) => tracing::info!("backfilled {n} friendly_name row(s)"),
                    Err(e) => tracing::warn!("friendly_name backfill failed: {e}"),
                }
            }
            app.manage(std::sync::Arc::clone(&store));
            app.manage(Mutex::new(mcp::McpRuntime::default()));
            // Start the MCP control API if the user has enabled it (off by
            // default). Reuses the same Store / SshClient / registry as the UI.
            maybe_start_mcp(
                app.handle(),
                &store,
                &ssh_client_for_setup,
                &reg_for_setup,
                &tunnels_for_setup,
                &guards,
            );
            // Task H: proactive background reconcile tick. A Tauri-runtime
            // spawned interval drives `service::sessions::reconcile_now` on the same
            // managed Store/SshClient the commands use, so fleet state stays
            // fresh without the UI having to poll. Reconcile is Tauri-free
            // (events flow through the store's EventBus), so the loop needs no
            // AppHandle. Interval comes from settings (`reconcile.interval_secs`,
            // default 20; 0 disables). A `try_lock` guard skips a tick if the
            // previous reconcile is still running so slow passes can't stack.
            spawn_reconcile_tick(
                std::sync::Arc::clone(&store),
                std::sync::Arc::clone(&ssh_client_for_setup),
            );
            Ok(())
        })
        .manage(Mutex::new(PtyState::new()))
        .manage(ssh_client)
        .manage(reg)
        .manage(tunnels)
        .manage(upload_allow)
        // Drag-drop reaches a `WebviewWindow` as a window event (and, for a
        // standalone webview, as a webview event) — record dropped paths from
        // both so `upload_to_session` can verify them.
        .on_webview_event(move |_, event| {
            if let tauri::WebviewEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                upload_allow_for_webview.allow(paths);
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::health::health_check,
            commands::diagnostics::collect_diagnostics,
            commands::diagnostics::open_log_folder,
            commands::projects::list_projects,
            commands::projects::refresh_projects,
            commands::sessions::list_sessions,
            commands::sessions::related_sessions,
            commands::sessions::new_session,
            commands::sessions::kill_session,
            commands::sessions::safe_kill_session,
            commands::sessions::inspect_safe_kill,
            commands::sessions::discard_kill_session,
            commands::worktrees::list_worktrees,
            commands::worktrees::list_host_worktrees,
            commands::worktrees::delete_worktree,
            commands::sessions::repair_session,
            commands::sessions::rename_session,
            commands::sessions::set_session_friendly_name,
            commands::sessions::session_history,
            commands::sessions::restart_session,
            commands::sessions::send_prompt,
            commands::sessions::spawn_review,
            commands::sessions::recreate_session,
            commands::move_session::move_session,
            commands::sessions::dismiss_ghost_session,
            commands::sessions::new_bg_session,
            commands::sessions::peek_session,
            commands::sessions::purge_project,
            commands::sessions::get_fleet_settings,
            commands::sessions::set_fleet_setting,
            commands::tasks::list_tasks,
            commands::tasks::cancel_task,
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
            commands::mcp::provision_hosts,
            commands::mcp::list_host_tokens,
            commands::mcp::set_host_token_mode,
            commands::mcp::rotate_host_token,
            commands::mcp::mcp_confirm,
            commands::mcp::mcp_pending_confirms,
            commands::onboarding::check_local_prereqs,
            commands::onboarding::tunnel_status,
            pty::pty_open,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_close,
            pty::pty_drain,
            cancel_command,
        ])
        .on_window_event(move |window, event| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                upload_allow_for_window.allow(paths);
            }
            // On exit: close ssh masters AND any open PTY, so we don't leak
            // background ssh processes or an orphaned `tmux attach` / `ssh
            // -tt` child after quit.
            if let tauri::WindowEvent::Destroyed = event {
                use tauri::Manager;
                ssh_client_for_exit.shutdown_all();
                tunnels_for_exit.stop_all();
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
