mod app_events;
pub mod backend;
mod bootstrap;
mod commands;
mod pty;

pub use app_events::AppHandleEventBus;

use backend::{Backend, OsTokenStore};
use bootstrap::env::{
    appdata_dir, backfill_locale_for_gui_launch, backfill_path_for_gui_launch, env_looks_complete,
    import_login_shell_env,
};
use bootstrap::singleton::kill_other_instances;
use commands::cancel::cancel_command;
use fleet_core::store::Store;
use pty::PtyState;
use std::sync::Mutex;

/// Declare this binary's version to fleet-core.
///
/// Mandatory, not an optimisation: `fleet_core::app_version::get()` has no
/// fallback and panics until this has run, precisely so fleet-core's internal
/// 0.1.0 can never be reported as an app version. `run()` calls it as its first
/// statement; a test that reaches a version-reporting path (health, the
/// diagnostics bundle) calls it itself, since tests never go through `run()`.
/// Repeat calls are harmless — `set` is first-call-wins.
pub fn declare_app_version() {
    fleet_core::app_version::set(env!("CARGO_PKG_VERSION"));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Before anything reports a version: fleet-core's own crate version is not
    // the app's, and `app_version::get()` panics until this has run.
    declare_app_version();
    // File logging first, so the instance reaper and env recovery below are
    // captured too. A failure is non-fatal: the app runs without a log file.
    let data_dir = appdata_dir();
    let log_dir = match fleet_core::logging::init(&data_dir) {
        Ok(dir) => Some(dir),
        Err(e) => {
            // `init` failed before installing a subscriber: install the stderr
            // fallback first, or this line would go nowhere.
            fleet_core::logging::init_stderr_fallback();
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

    let ssh_client = std::sync::Arc::new(fleet_core::ssh::SshClient::new());
    let ssh_client_for_exit = std::sync::Arc::clone(&ssh_client);
    let ssh_client_for_setup = std::sync::Arc::clone(&ssh_client);
    let reg = fleet_core::cancel::CancellationRegistry::new();
    let reg_for_setup = std::sync::Arc::clone(&reg);
    let tunnels = std::sync::Arc::new(fleet_core::service::tunnel::TunnelSupervisor::new());
    let tunnels_for_exit = std::sync::Arc::clone(&tunnels);
    let tunnels_for_setup = std::sync::Arc::clone(&tunnels);
    // Cancelled on window destroy, so the hub event bridge's socket does not
    // keep a background task alive after quit. Standalone mode never starts
    // the bridge, so nothing observes it there.
    let shutdown_token = tokio_util::sync::CancellationToken::new();
    let shutdown_for_exit = shutdown_token.clone();
    // SEC-9: the only local paths `upload_to_session` may read are the ones
    // the OS drag-drop handed the window (recorded below in the window /
    // webview event handlers).
    let upload_allow = std::sync::Arc::new(commands::upload::UploadAllowList::new());
    let upload_allow_for_window = std::sync::Arc::clone(&upload_allow);
    let upload_allow_for_webview = std::sync::Arc::clone(&upload_allow);

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            use tauri::Manager;
            // Core tasks (ticks, MCP server, hook side-jobs) spawn through
            // `fleet_core::rt`; give it this app's tokio runtime, since this
            // closure runs outside any runtime context. `block_on` executes
            // the future ON the Tauri runtime, so `Handle::current()` inside
            // it is that runtime's handle.
            tauri::async_runtime::block_on(async {
                fleet_core::rt::install(tokio::runtime::Handle::current());
            });
            let handle = app.handle().clone();
            // Built concretely and then coerced, rather than built as the
            // trait object: the hub event bridge needs the concrete type. Both
            // handles are the same bus, so a hub event and a local one go down
            // one channel to one drain thread.
            let frontend_bus =
                std::sync::Arc::new(crate::app_events::AppHandleEventBus::new(handle));
            let bus: std::sync::Arc<dyn fleet_core::events::EventBus> = frontend_bus.clone();
            // Kept alongside the clone moved into `Store` below: the account
            // usage poller and its commands emit `account_usage:updated`
            // straight through the bus, not through a `Store` row mutation
            // (usage isn't a `Store` row), so they need their own handle to
            // it as managed state.
            let bus_for_usage = std::sync::Arc::clone(&bus);
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
            fleet_core::service::provision::set_private_mode(&db_path);
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
            let guards = fleet_core::mcp::McpGuards::new(std::sync::Arc::new(move |req| {
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
            app.manage(Mutex::new(fleet_core::mcp::McpRuntime::default()));
            app.manage(std::sync::Arc::clone(&bus_for_usage));
            // One in-memory usage cache for the app's lifetime,
            // shared by the background poller (below) and the
            // `list_account_usage` / `refresh_account_usage` commands.
            let usage_cache = std::sync::Arc::new(Mutex::new(
                fleet_core::service::account_usage::UsageCache::new(),
            ));
            app.manage(std::sync::Arc::clone(&usage_cache));
            // Standalone, a window onto a `fleet-hub`, or configured for a hub
            // this launch cannot use? Decided once, here, from
            // `hub.remote_url` plus the client token kept outside the
            // database. It governs what this process starts below and what
            // every command does, because a desktop pointed at a hub — usable
            // or not — must not become a second brain reconciling and
            // mutating the same fleet.
            // Managed, not built and dropped: `commands::hub` writes to the
            // same store when the user pairs or disconnects, and there must
            // be exactly one implementation of "where the token lives".
            let tokens: std::sync::Arc<dyn backend::TokenStore> =
                std::sync::Arc::new(OsTokenStore::new(data_dir.clone()));
            let backend = Backend::resolve(&store, tokens.as_ref());
            app.manage(std::sync::Arc::clone(&tokens));
            // Whether this window's live link to the hub is up — the
            // disconnected banner. Standalone it never moves off
            // `Standalone`; a hub client's bridge reports into it. Built
            // before the backend below, which reads it.
            let hub_link = std::sync::Arc::new(match backend.remote() {
                Some(cfg) => backend::connection::HubConnectionStatus::remote(
                    std::sync::Arc::clone(&frontend_bus)
                        as std::sync::Arc<dyn backend::events::RemoteEventSink>,
                    &cfg.token,
                ),
                None => backend::connection::HubConnectionStatus::standalone(),
            });
            app.manage(std::sync::Arc::clone(&hub_link));
            // Two managed values, one decision. `Backend` is the resolved
            // answer (what this block branches on below); `FleetBackend` is
            // what the commands hold — the same answer plus the `HubBackend`
            // to call when it is remote. Built once here so that every
            // command shares one client, and so that nothing can re-resolve
            // the mode mid-run.
            //
            // `watching` hands it the status above rather than a copy: a hub
            // whose wire contract this build cannot read is then refused at
            // the call, not only ignored by the event bridge.
            app.manage(std::sync::Arc::new(
                backend::FleetBackend::from_resolved(&backend)
                    .watching(std::sync::Arc::clone(&hub_link)
                        as std::sync::Arc<dyn backend::connection::ConnectionView>),
            ));
            app.manage(backend.clone());
            // Which background tasks this process may run is decided in
            // `backend::startup`, not here, and the real spawns live in
            // `bootstrap::tasks`. Both moved out of this closure because
            // nothing can test it — it needs a live `tauri::App` — and that
            // untestable guard was a tautology in disguise: it hoisted
            // `spawn_reconcile_tick` out of the old `else` so both modes
            // started it, and all 91 tests still passed.
            //
            // `lib.rs` may no longer name any of the three; a test asserts
            // that, which is what makes that exact refactor fail now.
            backend::startup::start_background_tasks(
                &backend,
                &bootstrap::tasks::RealFleetTasks {
                    app: app.handle().clone(),
                    store: std::sync::Arc::clone(&store),
                    ssh: std::sync::Arc::clone(&ssh_client_for_setup),
                    reg: std::sync::Arc::clone(&reg_for_setup),
                    tunnels: std::sync::Arc::clone(&tunnels_for_setup),
                    guards: guards.clone(),
                    usage_cache: std::sync::Arc::clone(&usage_cache),
                    bus: bus_for_usage,
                    frontend: std::sync::Arc::clone(&frontend_bus),
                    remote: backend.remote().cloned(),
                    shutdown: shutdown_token.clone(),
                    hub_link,
                },
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
            commands::projects::add_project,
            commands::projects::list_github_repos,
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
            commands::work::session_work_links,
            commands::work::link_session_work,
            commands::work::reject_session_work,
            commands::work::unlink_session_work,
            commands::work::confirm_session_work,
            commands::work::set_work_project_trust,
            commands::work::work_resume_plan,
            commands::work::resume_work,
            commands::work::work_purge_impact,
            commands::work::work_today,
            commands::work::work_ticket_card,
            commands::work::request_work_handover,
            commands::trackers::add_tracker,
            commands::trackers::update_tracker,
            commands::trackers::set_tracker_credential,
            commands::trackers::test_tracker,
            commands::trackers::remove_tracker,
            commands::trackers::list_trackers,
            commands::trackers::work_tickets,
            commands::trackers::work_lookup,
            commands::trackers::start_work,
            commands::orgs::add_org,
            commands::orgs::update_org,
            commands::orgs::remove_org,
            commands::orgs::add_org_rule,
            commands::orgs::remove_org_rule,
            commands::orgs::assign_host_org,
            commands::orgs::assign_tracker_org,
            commands::orgs::work_scopes,
            commands::orgs::list_orgs,
            commands::orgs::org_suggestions,
            commands::sessions::session_history,
            commands::sessions::session_conversations,
            commands::sessions::session_conversation,
            commands::sessions::session_tool_detail,
            commands::sessions::session_activity,
            commands::sessions::restart_session,
            commands::sessions::send_prompt,
            commands::sessions::spawn_review,
            commands::sessions::recreate_session,
            commands::sessions::restore_host_sessions,
            commands::sessions::discover_lost_sessions,
            commands::move_session::move_session,
            commands::resolve_move::resolve_move,
            commands::sessions::dismiss_ghost_session,
            commands::sessions::dismiss_agent_session,
            commands::sessions::new_bg_session,
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
            commands::upload::pick_attachments,
            commands::upload::attachment_preview,
            commands::upload::attachment_describe,
            commands::upload::upload_attachments,
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
            commands::hosts::set_account_nickname,
            commands::account_usage::list_account_usage,
            commands::account_usage::refresh_account_usage,
            commands::mcp::mcp_status,
            commands::mcp::mcp_configure,
            commands::mcp::install_fleet_hook,
            commands::mcp::provision_hosts,
            commands::mcp::list_host_tokens,
            commands::mcp::set_host_token_mode,
            commands::mcp::rotate_host_token,
            commands::mcp::mcp_confirm,
            commands::mcp::mcp_pending_confirms,
            commands::operator::ensure_operator,
            commands::operator::operator_status,
            commands::hub::hub_status,
            commands::hub::hub_pair,
            commands::hub::hub_disconnect,
            commands::hub::hub_connection,
            commands::hub::hub_stranded_token,
            commands::hub::report_client_error,
            commands::onboarding::check_local_prereqs,
            commands::onboarding::tunnel_status,
            commands::assets::catalog_config,
            commands::assets::catalog_configure,
            commands::assets::catalog_load,
            commands::assets::catalog_list_assets,
            commands::assets::catalog_get_asset,
            commands::assets::catalog_list_layers,
            commands::assets::catalog_resolve_preview,
            commands::assets::catalog_propose_layers,
            commands::assets::catalog_set_host_layers,
            commands::assets::catalog_layer_template,
            commands::assets::catalog_write_layer,
            commands::assets::catalog_delete_layer,
            commands::assets::catalog_import_host,
            commands::assets::assets_scan_hosts,
            commands::assets::assets_inventory,
            commands::assets::catalog_plan_sync,
            commands::assets::catalog_apply_sync,
            commands::assets::catalog_last_sync,
            commands::assets::catalog_list_secrets,
            commands::assets::catalog_set_secret,
            commands::assets::catalog_delete_secret,
            commands::assets::catalog_create_asset,
            commands::assets::catalog_update_asset,
            commands::assets::catalog_delete_asset,
            commands::assets::catalog_add_resource,
            commands::assets::catalog_remove_resource,
            commands::assets::catalog_lint_asset,
            commands::assets::catalog_lint_all,
            commands::assets::catalog_commit_pending,
            commands::assets::catalog_push,
            commands::assets::catalog_repo_status,
            commands::assets::catalog_template,
            commands::assets::catalog_spawn_author_session,
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
                shutdown_for_exit.cancel();
                if let Some(runtime) = window.try_state::<Mutex<fleet_core::mcp::McpRuntime>>() {
                    if let Ok(mut rt) = runtime.lock() {
                        rt.stop();
                    }
                }
                if let Some(pty) = window.try_state::<Mutex<PtyState>>() {
                    pty::close_pty(pty.inner());
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
