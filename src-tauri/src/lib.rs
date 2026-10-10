// `#[async_trait]` expands each async trait method into a `#[must_use]` fn that
// returns a boxed future, which is already `#[must_use]`; clippy 1.99 flags that
// macro output as `double_must_use`. It is not code we wrote — allow it crate-wide.
#![allow(clippy::double_must_use)]
mod app_events;
pub mod backend;
mod bootstrap;
mod commands;
mod pty;
mod self_update;
mod voice;

pub use app_events::AppHandleEventBus;

use backend::{Backend, OsTokenStore};
use bootstrap::env::{appdata_dir, backfill_locale_for_gui_launch};
#[cfg(unix)]
use bootstrap::env::{backfill_path_for_gui_launch, env_looks_complete, import_login_shell_env};
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
    let (data_dir, data_dir_note) = appdata_dir();
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
    if let Some(note) = data_dir_note {
        tracing::warn!("[startup] {note}");
    }

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
    //
    // Steps 1 and 2 are Unix-only: Windows has no login shell to ask, and a
    // Git Bash `$SHELL` would hand back a POSIX-style PATH that breaks every
    // spawn.
    #[cfg(unix)]
    {
        if !env_looks_complete() {
            import_login_shell_env();
        }
        backfill_path_for_gui_launch();
    }
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
        .plugin(tauri_plugin_updater::Builder::new().build())
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
            // The tray and menu-bar icon (redesign 3.17), idle until the
            // frontend says otherwise.
            commands::tray::install(app);
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
            // Every request to a hub names this build (update design §6.3).
            if let Err(e) = fleet_core::http_client::set_client_header(self_update::client_header()) {
                tracing::warn!(error = %e, "X-Fleet-Client not set");
            }
            app.manage(self_update::SelfUpdate::default());
            let db_path = data_dir.join("state.db");
            let store = Store::open_with_bus(&db_path, bus).unwrap_or_else(|e| {
                // Still a hard fail (the app can't run without its DB), but
                // with an actionable message instead of a bare "open store".
                // No "delete it" advice for a database a newer release
                // migrated: it is intact, and its message says what to do.
                let advice = fleet_core::store::open_failure_advice(
                    &e,
                    "\nIf the file is corrupt, deleting it resets all local \
                     state — hosts, projects and sessions are re-discovered \
                     on the next launch.",
                );
                panic!(
                    "failed to open the claude-fleet database at {}: {e}{advice}",
                    db_path.display()
                )
            });
            // SEC-11: the DB holds bearer tokens and account metadata in
            // plaintext — keep it owner-only. Best-effort, logged on failure.
            fleet_core::service::provision::set_private_mode(&db_path);
            // File downloads (standalone: this machine keeps the copies).
            if let Err(e) = fleet_core::service::downloads::init(&data_dir, &store) {
                tracing::warn!(error = %e, "downloads unavailable");
            }
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
            // Windows is a client, never a fleet host: no tmux, no bash, no
            // Claude Code sessions of its own (docs/windows.md). `local` goes
            // off the way it does on a hub with `hub.local_host=false`, before
            // the ticks and the control API start: every command naming it is
            // refused with E_NOTFOUND, and the seeded row is hidden.
            #[cfg(windows)]
            {
                // WSL distributions become hosts (`fleet_core::wsl`), found on
                // a thread of their own: the first wsl.exe after a boot starts
                // the WSL service and can take seconds, which must not hold up
                // the window. Commands for `wsl-` aliases wait for it.
                fleet_core::wsl::refresh_in_background(
                    || {
                        fleet_core::ssh_config::load_user_config()
                            .into_iter()
                            .map(|h| h.alias)
                            .collect()
                    },
                    std::time::Duration::from_secs(15),
                );
                let ssh_bin = fleet_core::ssh::default_ssh_binary();
                if ssh_bin.is_absolute() && !ssh_bin.is_file() {
                    tracing::warn!(
                        ssh = %ssh_bin.display(),
                        "[startup] the ssh program (CLAUDE_FLEET_SSH) does not exist; every host will fail"
                    );
                }
                // portable-pty prefers a conpty.dll beside the exe (the one the
                // installer ships) to the built-in ConPTY. This says whether the
                // file is there; the first terminal logs which one actually
                // loaded (`pty::log_conpty_once`) — a DLL for another CPU is
                // there and still falls back.
                let bundled_conpty = std::env::current_exe()
                    .ok()
                    .and_then(|exe| exe.parent().map(|d| d.join("conpty.dll").is_file()))
                    .unwrap_or(false);
                tracing::info!(
                    ssh = %ssh_bin.display(),
                    conpty_file = if bundled_conpty { "present" } else { "absent" },
                    "[startup] Windows host sources"
                );
                fleet_core::service::hub::disable_local_host();
                if let Ok(s) = store.lock() {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    match fleet_core::service::hub::retire_local_host(&s, now) {
                        Ok(0) => {}
                        Ok(n) => tracing::info!("retired {n} session(s) on the local host"),
                        Err(e) => tracing::warn!("could not retire the local host: {e}"),
                    }
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
            // Local workspace sync: managed in both modes (its commands are
            // refused in hub-client mode); its tick starts with the other
            // fleet-owning tasks below.
            let local_sync = fleet_core::service::local_sync::LocalSync::new(
                std::sync::Arc::clone(&store),
                std::sync::Arc::clone(&ssh_client_for_setup) as std::sync::Arc<dyn fleet_core::ssh::SshExec>,
            );
            app.manage(std::sync::Arc::clone(&local_sync));
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
            tracing::info!("startup: resolving backend (hub.remote_url, then the client token)");
            let resolving = std::time::Instant::now();
            let backend = Backend::resolve(
                &store,
                &backend::token_store::BoundedTokenStore::new(
                    std::sync::Arc::clone(&tokens),
                    backend::token_store::KEYCHAIN_WAIT,
                ),
            );
            tracing::info!(
                elapsed_ms = resolving.elapsed().as_millis() as u64,
                remote = backend.is_remote(),
                unavailable = backend.unavailable().is_some(),
                "startup: backend resolved"
            );
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
                    local_sync,
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
        .manage(voice::VoiceState::default())
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
            commands::tray::set_tray_state,
            commands::projects::list_projects,
            commands::projects::refresh_projects,
            commands::projects::add_project,
            commands::projects::list_github_repos,
            commands::projects::project_picks,
            commands::projects::set_project_pick,
            commands::local_workspaces::list_local_workspaces,
            commands::local_workspaces::enable_local_workspace,
            commands::local_workspaces::pause_local_workspace,
            commands::local_workspaces::resume_local_workspace,
            commands::local_workspaces::sync_local_workspace_now,
            commands::local_workspaces::disconnect_local_workspace,
            commands::local_workspaces::set_local_workspace_excludes,
            commands::local_workspaces::resolve_local_workspace_conflict,
            commands::local_workspaces::open_local_workspace,
            commands::local_workspaces::local_workspace_changes,
            commands::local_workspaces::local_workspace_diff,
            commands::local_workspaces::commit_local_workspace,
            commands::local_workspaces::discard_local_workspace_changes,
            commands::local_workspaces::dismiss_local_workspace_activity,
            commands::local_workspaces::compare_local_conflict,
            commands::local_workspaces::keep_both_local_conflict,
            commands::local_workspaces::ask_ai_about_local_changes,
            commands::local_workspaces::set_local_workspace_driver,
            commands::sessions::list_sessions,
            commands::sessions::new_session,
            commands::sessions::kill_session,
            commands::sessions::shell_terminals,
            commands::sessions::safe_kill_session,
            commands::sessions::inspect_safe_kill,
            commands::sessions::discard_kill_session,
            commands::worktrees::list_host_worktrees,
            commands::sessions::repair_session,
            commands::sessions::rename_session,
            commands::sessions::set_session_friendly_name,
            commands::sessions::set_session_tags,
            commands::sessions::decide_related_session,
            commands::sessions::touch_session_viewed,
            commands::work::session_work_links,
            commands::work::link_session_work,
            commands::work::reject_session_work,
            commands::work::unlink_session_work,
            commands::work::confirm_session_work,
            commands::work::work_tidy,
            commands::work::work_reopened,
            commands::work::unarchive_session_work,
            commands::work::tidy_apply,
            commands::work::dismiss_reopened,
            commands::work::set_work_project_trust,
            commands::work::work_resume_plan,
            commands::work::resume_work,
            commands::work::work_purge_impact,
            commands::work::work_today,
            commands::work::work_ticket_card,
            commands::work::request_work_handover,
            commands::work::summarize_past_work,
            commands::work::name_session_work,
            commands::work::rename_work_item,
            commands::work::create_work_task,
            commands::work::set_work_status,
            commands::work::edit_work_item,
            commands::work::set_work_parent,
            commands::work::accept_work_proposal,
            commands::work::reject_work_proposal,
            commands::work_view::work_tree,
            commands::work_view::work_task,
            commands::work_view::work_session_tasks,
            commands::work_view::work_review,
            commands::work_view::work_rules,
            commands::work_view::work_rule_preview,
            commands::work_view::work_views,
            commands::work_view::work_org_impact,
            commands::work_view::set_primary_work,
            commands::work_view::switch_session_work,
            commands::work_view::reconsider_work_link,
            commands::work_view::ack_work_link,
            commands::work_view::decide_work_batch,
            commands::work_view::place_work,
            commands::work_view::assign_work_org,
            commands::work_view::save_work_rule,
            commands::work_view::delete_work_rule,
            commands::work_view::save_work_view,
            commands::work_view::delete_work_view,
            commands::work_view::comment_on_work,
            commands::work_view::delete_work_comment,
            commands::work_view::work_buckets,
            commands::work_view::work_bucket,
            commands::work_view::add_work_to_bucket,
            commands::work_view::remove_work_from_bucket,
            commands::work_view::work_bucket_admin,
            commands::missions::work_missions,
            commands::missions::work_mission,
            commands::missions::save_mission,
            commands::missions::set_mission_state,
            commands::missions::set_mission_repo,
            commands::missions::set_mission_item,
            commands::missions::import_mission_plan,
            commands::missions::delete_mission,
            commands::missions::set_work_dep,
            commands::missions::set_work_hold,
            commands::missions::accept_work_proposals,
            commands::missions::undo_work_accept,
            commands::missions::set_work_done_when,
            commands::missions::verify_work_item,
            commands::missions::start_mission_wave,
            commands::missions::retry_work_item,
            commands::missions::plan_mission,
            commands::missions::decide_mission_card,
            commands::missions::grant_mission,
            commands::missions::revoke_mission_grant,
            commands::missions::pause_all_missions,
            commands::missions::mission_release_note,
            commands::missions::today_brief,
            commands::missions::mission_triage,
            commands::trackers::add_tracker,
            commands::trackers::update_tracker,
            commands::trackers::set_tracker_credential,
            commands::trackers::test_tracker,
            commands::trackers::remove_tracker,
            commands::trackers::tracker_sync_metrics,
            commands::trackers::status_map_proposals,
            commands::trackers::decide_status_map_proposal,
            commands::trackers::work_retention_status,
            commands::trackers::work_retention_sweep,
            commands::trackers::list_trackers,
            commands::trackers::work_tickets,
            commands::trackers::work_lookup,
            commands::trackers::start_work_multi,
            commands::trackers::start_work,
            commands::trackers::abandon_start,
            commands::trackers::preview_start_work,
            commands::orgs::add_org,
            commands::orgs::update_org,
            commands::orgs::remove_org,
            commands::orgs::add_org_rule,
            commands::orgs::remove_org_rule,
            commands::orgs::assign_host_org,
            commands::orgs::assign_tracker_org,
            commands::orgs::set_org_setting,
            commands::orgs::set_org_member,
            commands::orgs::remove_org_member,
            commands::orgs::org_member_grants,
            commands::orgs::org_rule_preview,
            commands::orgs::add_org_project,
            commands::orgs::remove_org_project,
            commands::orgs::revoke_org_share,
            commands::orgs::narrow_org_share,
            commands::orgs::revoke_org_member_grants,
            commands::orgs::list_orgs,
            commands::orgs::org_suggestions,
            commands::org_devices::list_devices,
            commands::org_devices::pair_device,
            commands::org_devices::revoke_device,
            commands::org_devices::set_device_trust,
            commands::org_devices::update_device,
            commands::org_devices::bind_device_org,
            commands::org_devices::set_device_person,
            commands::org_devices::grant_device_catalog,
            commands::org_devices::list_people,
            commands::org_devices::rename_person,
            commands::org_devices::disable_person,
            commands::sessions::session_history,
            commands::sessions::session_conversations,
            commands::sessions::session_conversation,
            commands::sessions::session_tool_detail,
            commands::sessions::session_activity,
            commands::sessions::capture_session,
            commands::sessions::session_summary_since,
            commands::sessions::session_share,
            commands::sessions::session_unshare,
            commands::sessions::session_narrow,
            commands::sessions::session_access,
            commands::sessions::my_grants,
            commands::sessions::session_ask_access,
            commands::sessions::access_requests,
            commands::sessions::restart_session,
            commands::sessions::rewind_conversation,
            commands::sessions::send_prompt,
            commands::sessions::spawn_review,
            commands::sessions::queue_prompt,
            commands::sessions::queued_prompts,
            commands::sessions::cancel_queued_prompt,
            commands::sessions::recreate_session,
            commands::sessions::restore_host_sessions,
            commands::sessions::restore_all_lost_sessions,
            commands::sessions::repair_workspaces_now,
            commands::sessions::discover_lost_sessions,
            commands::move_session::move_session,
            commands::resolve_move::resolve_move,
            commands::sessions::dismiss_ghost_session,
            commands::sessions::adopt_session,
            commands::sessions::lost_target,
            commands::sessions::place_transcript,
            commands::sessions::dismiss_agent_session,
            commands::sessions::new_bg_session,
            commands::sessions::purge_project,
            commands::quick_replies::quick_replies,
            commands::quick_replies::set_quick_replies,
            commands::downloads::list_downloads,
            commands::library::list_library,
            commands::library::add_library_items,
            commands::library::remove_library_item,
            commands::downloads::send_file,
            commands::downloads::remove_download,
            commands::downloads::save_download,
            commands::pages::get_fleet_settings,
            commands::pages::describe_fleet_settings,
            commands::pages::list_pages,
            commands::pages::fetch_page_source,
            commands::pages::flow_start,
            commands::pages::flow_submit,
            commands::pages::flow_back,
            commands::pages::flow_cancel,
            commands::pages::setting_proposals,
            commands::pages::decide_setting_proposals,
            commands::pages::list_guides,
            commands::pages::decide_guide,
            commands::pages::remove_guide,
            commands::forms::list_forms,
            commands::forms::get_form,
            commands::forms::answer_form,
            commands::forms::decline_form,
            commands::federation::list_peer_links,
            commands::federation::link_peer_hub,
            commands::federation::unlink_peer_hub,
            commands::updates::list_update_targets,
            commands::updates::update_check,
            commands::updates::update_install,
            commands::debug_devices::list_debug_devices,
            commands::debug_devices::scan_debug_devices,
            commands::debug_devices::update_debug_device,
            commands::debug_devices::release_debug_device,
            commands::debug_devices::forget_debug_device,
            commands::debug_devices::boot_debug_device,
            commands::debug_devices::shutdown_debug_device,
            commands::debug_devices::claim_debug_device,
            commands::debug_devices::install_debug_device,
            commands::debug_devices::debug_device_logs,
            commands::debug_devices::debug_device_screenshot,
            commands::prs::list_pull_requests,
            commands::start_rules::start_rules,
            commands::api_tokens::api_tokens,
            commands::add_account::add_account,
            commands::presence::session_presence,
            commands::pages::setting_history,
            commands::pages::set_fleet_setting,
            commands::runs::list_runs,
            commands::routines::routines,
            commands::tasks::list_tasks,
            commands::tasks::cancel_task,
            commands::files::repo_changes,
            commands::files::repo_tree,
            commands::files::repo_file,
            commands::files::repo_diff,
            commands::files::repo_blame,
            commands::files::repo_branch_diff,
            commands::files::repo_range_diff,
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
            commands::mutate::repo_delete_merged_branches,
            commands::mutate::repo_stage,
            commands::mutate::repo_unstage,
            commands::mutate::repo_commit_create,
            commands::mutate::draft_commit_message,
            commands::mutate::repo_fetch,
            commands::mutate::repo_pull,
            commands::mutate::repo_push,
            commands::hosts::discover_hosts,
            commands::hosts::list_hosts,
            commands::hosts::list_accounts,
            commands::hosts::add_host,
            commands::hosts::probe_host,
            commands::hosts::check_host,
            commands::hosts::list_host_setups,
            commands::hosts::save_host_setup,
            commands::hosts::discard_host_setup,
            commands::hosts::run_host_setup_check,
            commands::hosts::install_agent,
            commands::hosts::agent_installs,
            commands::hosts::probe_ssh_alias,
            commands::hosts::remove_host,
            commands::hosts::merge_host,
            commands::hosts::hide_host,
            commands::hosts::set_account_nickname,
            commands::account_usage::list_account_usage,
            commands::account_usage::refresh_account_usage,
            commands::account_usage::account_usage_history,
            commands::account_usage::account_spend,
            commands::account_usage::check_account_headroom,
            commands::account_usage::propose_host_placement,
            commands::account_usage::record_host_placement,
            commands::mcp::mcp_status,
            commands::mcp::mcp_configure,
            commands::mcp::install_fleet_hook,
            commands::mcp::provision_hosts,
            commands::mcp::list_host_tokens,
            commands::mcp::set_host_token_mode,
            commands::mcp::rotate_host_token,
            commands::mcp::mcp_confirm,
            commands::mcp::mcp_pending_confirms,
            commands::mcp::control_handoffs,
            commands::operator::ensure_operator,
            commands::operator::operator_status,
            commands::operator::control_route_propose,
            commands::operator::control_route_follow,
            commands::hub::hub_status,
            commands::hub::hub_pair,
            commands::hub::hub_disconnect,
            commands::hub::hub_connection,
            commands::hub::hub_retry_now,
            commands::hub::offline_local_sessions,
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
            commands::assets::catalog_set_host_harnesses,
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
            commands::assets::catalog_repo_status_in,
            commands::assets::catalog_list_catalogs,
            commands::assets::catalog_list_changesets,
            commands::assets::catalog_get_changeset,
            commands::assets::catalog_apply_changeset,
            commands::assets::catalog_undo_changeset,
            commands::assets::catalog_dismiss_changeset,
            commands::assets::catalog_reject_changeset_items,
            commands::assets::catalog_propose_changesets,
            commands::assets::catalog_propose_layer_change,
            commands::assets::catalog_add_catalog,
            commands::assets::catalog_remove_catalog,
            commands::assets::catalog_admit_catalog,
            commands::assets::catalog_unadmit_catalog,
            commands::assets::catalog_list_layers_in,
            commands::assets::catalog_host_provenance,
            commands::assets::catalog_drift_diff,
            commands::assets::catalog_asset_history,
            commands::assets::catalog_template,
            commands::assets::catalog_spawn_author_session,
            pty::pty_open,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_close,
            pty::pty_drain,
            commands::editor::open_session_in_editor,
            commands::windows::open_terminal_window,
            commands::context_help::context_help,
            commands::voice::voice_claim,
            commands::voice::voice_release,
            cancel_command,
        ])
        .on_window_event(move |window, event| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                upload_allow_for_window.allow(paths);
            }
            // The main window going away is the app's exit: close ssh
            // masters AND any open PTY, so we don't leak background ssh
            // processes or an orphaned `tmux attach` / `ssh -tt` child after
            // quit. A pop-out terminal (step 5.4) going away closes its own
            // PTY and nothing else: the app, its ssh masters and the main
            // window's terminals carry on. Any other window does neither.
            if let tauri::WindowEvent::Destroyed = event {
                use commands::windows::OnDestroyed;
                use tauri::Manager;
                match commands::windows::on_destroyed(window.label()) {
                    OnDestroyed::ClosePty => {
                        if let Some(pty) = window.try_state::<Mutex<PtyState>>() {
                            pty::close_pty(pty.inner(), window.label());
                        }
                    }
                    OnDestroyed::ShutDown => {
                        ssh_client_for_exit.shutdown_all();
                        tunnels_for_exit.stop_all();
                        shutdown_for_exit.cancel();
                        if let Some(runtime) =
                            window.try_state::<Mutex<fleet_core::mcp::McpRuntime>>()
                        {
                            if let Ok(mut rt) = runtime.lock() {
                                rt.stop();
                            }
                        }
                        if let Some(pty) = window.try_state::<Mutex<PtyState>>() {
                            pty::close_all_ptys(pty.inner());
                        }
                    }
                    OnDestroyed::Nothing => {}
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
