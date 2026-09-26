//! One table: what every Tauri command does when this desktop is a window
//! onto a hub.
//!
//! The verdict used to be spread over five lists kept in step by hand — the
//! `routed::` call in each command body, the sentence pasted at each
//! `local_only` call site, an exception list in the routing tests, the
//! frontend's own list of blocked actions, and the prose in `docs/hub.md`.
//! Four of them said the same thing in four vocabularies and the fifth said
//! it in English. This is the one that the rest are checked against.
//!
//! The rows are in `generate_handler!` order, so [`VERDICTS`] and `lib.rs`
//! read side by side. `every_command_has_a_verdict` (in
//! [`tests_routing`](super::tests_routing)) holds the two to exactly the same
//! set of names, and `every_commands_body_does_what_its_row_says` holds each
//! body to its row.
//!
//! What a verdict *means*, and the **parity or refusal** rule that decides
//! which one a command gets, is in [`routing`](super::routing)'s header. This
//! module only records the answers.

/// What one command does in remote mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Calls `tool` on the hub and returns its answer unchanged.
    Routed { tool: &'static str },
    /// Routes for one argument shape and refuses for another, because only
    /// part of what the command does has a counterpart on the hub. `unless`
    /// names the shape that refuses; `instead` is that refusal's sentence.
    ///
    /// One command has this verdict, and the honest thing is to say so rather
    /// than to file it under whichever half is more convenient: `repair_session`
    /// routes the explicit repair (the Repair workspace button, which maps
    /// one-to-one onto the tool's always-explicit repair) and refuses the
    /// automatic pre-attach check, which would become destructive by surprise.
    RoutedUnless {
        tool: &'static str,
        unless: &'static str,
        instead: &'static str,
    },
    /// Has no hub counterpart and must not quietly run against this machine:
    /// refuses with `E_LOCAL_ONLY`. `instead` completes the message's
    /// "…; " and must name where the operation *does* work.
    LocalOnly { instead: &'static str },
    /// About THIS process, so the local answer is the right answer either
    /// way. `why` is the reason, so that "I forgot" and "I decided" cannot
    /// look alike.
    SameInBoth { why: &'static str },
}

impl Verdict {
    /// The hub tool this command calls, when it calls one.
    pub fn tool(&self) -> Option<&'static str> {
        match self {
            Verdict::Routed { tool } | Verdict::RoutedUnless { tool, .. } => Some(tool),
            _ => None,
        }
    }

    /// The sentence this command refuses with, when it can refuse.
    ///
    /// `FleetBackend::refuse_local_only` reads exactly this, so a row that
    /// answers `None` here cannot be refused by name — which is what
    /// `every_refusal_names_a_command_the_table_can_refuse` checks.
    pub fn instead(&self) -> Option<&'static str> {
        match self {
            Verdict::LocalOnly { instead } | Verdict::RoutedUnless { instead, .. } => Some(instead),
            _ => None,
        }
    }
}

/// The verdict of `command`, or `None` when the table has no row for it —
/// which the tests make unshippable.
/// Why the whole attachment family is the same in both modes: the composer
/// reads, measures and previews files on THIS machine's disk and copies them
/// over THIS machine's ssh, exactly as `upload_to_session` does behind the
/// terminal pane. They were `LocalOnly` on the premise that "a hub client has
/// nothing local" — but a hub client is a desktop app with a disk; what belongs
/// to the hub is the fleet's database and hosts, not this machine.
const WHY_ATTACH: &str = "the same story as `upload_to_session`: this machine has the disk, the file dialog and the `ssh` that carries the bytes, and the session is addressed by the alias passed in, reading no state.db. Being a window onto a hub does not take this machine away";

pub fn verdict(command: &str) -> Option<&'static Verdict> {
    VERDICTS
        .iter()
        .find(|(name, _)| *name == command)
        .map(|(_, v)| v)
}

/// The twenty Assets commands that all refuse for the same reason.
/// Organisations (work graph M5.2): the hub's `work_admin` is master-only.
const ORGS_ARE_ADMIN: &str = "organisations, their rules and which org a host or tracker belongs \
     to are the hosts' security boundary and fleet administration: the hub's work_admin is \
     master-only, and a paired client is never the fleet's administrator; configure them on the \
     hub with `fleet-hub org add|rule add|assign-host|assign-tracker`";

/// Trackers (work graph M3.1): the hub's `work_admin` is master-only.
const TRACKERS_ARE_ADMIN: &str = "trackers and their credentials are fleet administration: the \
     hub's work_admin is master-only, and a paired client is never the fleet's administrator; \
     configure them on the hub with `fleet-hub tracker add|set-credential|test`";

const CATALOG_IS_A_CHECKOUT: &str =
    "the asset catalog is a git checkout on the machine that owns the fleet, and the hub has \
     no tool for this; work on the catalog there";

/// The ten git-write commands of the Files tab, likewise.
const NO_GIT_WRITE_TOOL: &str =
    "the hub exposes no git-write tool — a remote client must not stage or commit under a \
     running agent; do it in the session, or from a standalone app";

/// Every command in `generate_handler!`, in that order, with its verdict.
pub const VERDICTS: &[(&str, Verdict)] = &[
    // ── health and this app's own logs ──────────────────────────────────────
    (
        "health_check",
        Verdict::Routed {
            tool: "fleet_health",
        },
    ),
    (
        "collect_diagnostics",
        Verdict::SameInBoth {
            why: "describes THIS process — its log tail, its tunnels, its SSH counters — and \
                  is the first thing asked for when remote mode misbehaves",
        },
    ),
    (
        "open_log_folder",
        Verdict::SameInBoth {
            why: "this app's own log folder, which it has either way",
        },
    ),
    // ── projects ────────────────────────────────────────────────────────────
    (
        "list_projects",
        Verdict::Routed {
            tool: "list_projects",
        },
    ),
    (
        "refresh_projects",
        Verdict::Routed {
            tool: "refresh_projects",
        },
    ),
    (
        "add_project",
        Verdict::LocalOnly {
            instead: "it clones or adopts a checkout using this machine's SSH and GitHub \
                      credentials; add the project on the hub, then it appears here",
        },
    ),
    (
        "list_github_repos",
        Verdict::LocalOnly {
            instead: "it runs `gh` over this machine's SSH connection to the host; browse \
                      repositories from the hub or a standalone app",
        },
    ),
    // ── sessions ────────────────────────────────────────────────────────────
    (
        "list_sessions",
        Verdict::Routed {
            tool: "list_sessions",
        },
    ),
    (
        "related_sessions",
        Verdict::Routed {
            tool: "related_sessions",
        },
    ),
    (
        "new_session",
        Verdict::Routed {
            tool: "new_session",
        },
    ),
    (
        "kill_session",
        Verdict::Routed {
            tool: "kill_session",
        },
    ),
    (
        "safe_kill_session",
        Verdict::Routed {
            tool: "safe_kill_session",
        },
    ),
    (
        "inspect_safe_kill",
        Verdict::LocalOnly {
            instead: "it inspects the worktree over this machine's SSH connection and the \
                      hub exposes no tool for it; retire the session from the hub",
        },
    ),
    (
        "discard_kill_session",
        Verdict::LocalOnly {
            instead: "the hub exposes no tool that discards a worktree and kills in one \
                      step; use safe_kill_session, or do it from the hub",
        },
    ),
    // ── worktrees ───────────────────────────────────────────────────────────
    (
        "list_worktrees",
        Verdict::Routed {
            tool: "list_worktrees",
        },
    ),
    (
        "list_host_worktrees",
        Verdict::Routed {
            tool: "list_host_worktrees",
        },
    ),
    (
        "delete_worktree",
        Verdict::Routed {
            tool: "delete_worktree",
        },
    ),
    (
        "repair_session",
        Verdict::RoutedUnless {
            tool: "repair_session",
            unless: "explicit: false, the automatic pre-attach check",
            instead: "the hub's repair_session always runs the EXPLICIT repair, which may \
                      unregister a stale worktree entry, adopt a moved checkout and recreate \
                      a branch — this app will not turn an automatic pre-attach check into \
                      that; repair explicitly, or from the hub",
        },
    ),
    (
        "rename_session",
        Verdict::Routed {
            tool: "rename_session",
        },
    ),
    (
        "set_session_friendly_name",
        Verdict::Routed {
            tool: "set_friendly_name",
        },
    ),
    (
        "session_history",
        Verdict::Routed {
            tool: "session_history",
        },
    ),
    (
        "session_conversations",
        Verdict::Routed {
            tool: "session_conversations",
        },
    ),
    // Work links (roadmap M1b.2): four commands, two tools. Reads and link
    // decisions route; tracker admin (M3.1, below) is the LocalOnly half (C17).
    ("session_work_links", Verdict::Routed { tool: "work" }),
    ("link_session_work", Verdict::Routed { tool: "work_link" }),
    ("reject_session_work", Verdict::Routed { tool: "work_link" }),
    ("unlink_session_work", Verdict::Routed { tool: "work_link" }),
    // Work graph M4.4: decide a detected suggestion, and trust a project's
    // branch keys — both actions of the existing `work_link`.
    (
        "confirm_session_work",
        Verdict::Routed { tool: "work_link" },
    ),
    (
        "set_work_project_trust",
        Verdict::Routed { tool: "work_link" },
    ),
    // Work graph M7.2: the self-cleaning lifecycle — two reads of `work`
    // and six actions of the existing `work_link`.
    ("work_tidy", Verdict::Routed { tool: "work" }),
    ("work_reopened", Verdict::Routed { tool: "work" }),
    (
        "archive_session_work",
        Verdict::Routed { tool: "work_link" },
    ),
    (
        "unarchive_session_work",
        Verdict::Routed { tool: "work_link" },
    ),
    ("snooze_tidy", Verdict::Routed { tool: "work_link" }),
    ("never_tidy", Verdict::Routed { tool: "work_link" }),
    ("tidy_apply", Verdict::Routed { tool: "work_link" }),
    ("dismiss_reopened", Verdict::Routed { tool: "work_link" }),
    // Work graph M2.4: resume past work, and what a purge would strand.
    ("work_resume_plan", Verdict::Routed { tool: "work" }),
    ("resume_work", Verdict::Routed { tool: "work_link" }),
    ("work_purge_impact", Verdict::Routed { tool: "work" }),
    // Work graph M9.1: the Today view's digest.
    ("work_today", Verdict::Routed { tool: "work" }),
    // Work graph M9.2: the ticket context card, from the hub's cache.
    ("work_ticket_card", Verdict::Routed { tool: "work" }),
    // Work graph M9.3: ask a session for its hand-off (on demand, D9).
    (
        "request_work_handover",
        Verdict::Routed { tool: "work_link" },
    ),
    // Work graph M9.6: one ticket, one sibling session per repository.
    ("start_work_multi", Verdict::Routed { tool: "work_link" }),
    // Work graph M11.1: "Name this work…" — local work items, listed from
    // `work`, named and renamed through `work_link { name }`.
    ("list_local_work_items", Verdict::Routed { tool: "work" }),
    ("name_session_work", Verdict::Routed { tool: "work_link" }),
    ("rename_work_item", Verdict::Routed { tool: "work_link" }),
    // Work graph M3.1: trackers and their credentials are fleet
    // administration. The hub's `work_admin` is master-only, and a paired
    // desktop is a client, never the master (review C17).
    (
        "add_tracker",
        Verdict::LocalOnly {
            instead: TRACKERS_ARE_ADMIN,
        },
    ),
    (
        "update_tracker",
        Verdict::LocalOnly {
            instead: TRACKERS_ARE_ADMIN,
        },
    ),
    (
        "set_tracker_credential",
        Verdict::LocalOnly {
            instead: TRACKERS_ARE_ADMIN,
        },
    ),
    (
        "test_tracker",
        Verdict::LocalOnly {
            instead: TRACKERS_ARE_ADMIN,
        },
    ),
    (
        "remove_tracker",
        Verdict::LocalOnly {
            instead: TRACKERS_ARE_ADMIN,
        },
    ),
    // Work graph M11.4: the sync's per-tracker counters are
    // `work_admin { action: status }`, master-only like the rest.
    (
        "tracker_sync_metrics",
        Verdict::LocalOnly {
            instead: TRACKERS_ARE_ADMIN,
        },
    ),
    // Work graph M3.4: reading tickets and starting work route like every
    // other work read and decision.
    ("list_trackers", Verdict::Routed { tool: "work" }),
    ("work_tickets", Verdict::Routed { tool: "work" }),
    ("work_lookup", Verdict::Routed { tool: "work" }),
    ("start_work", Verdict::Routed { tool: "work_link" }),
    // Work graph M5: orgs are the per-host tokens' security boundary, so
    // changing them is fleet administration (the hub's `work_admin`,
    // master-only); reading them routes like every other work read.
    (
        "add_org",
        Verdict::LocalOnly {
            instead: ORGS_ARE_ADMIN,
        },
    ),
    (
        "update_org",
        Verdict::LocalOnly {
            instead: ORGS_ARE_ADMIN,
        },
    ),
    (
        "remove_org",
        Verdict::LocalOnly {
            instead: ORGS_ARE_ADMIN,
        },
    ),
    (
        "add_org_rule",
        Verdict::LocalOnly {
            instead: ORGS_ARE_ADMIN,
        },
    ),
    (
        "remove_org_rule",
        Verdict::LocalOnly {
            instead: ORGS_ARE_ADMIN,
        },
    ),
    (
        "assign_host_org",
        Verdict::LocalOnly {
            instead: ORGS_ARE_ADMIN,
        },
    ),
    (
        "assign_tracker_org",
        Verdict::LocalOnly {
            instead: ORGS_ARE_ADMIN,
        },
    ),
    ("work_scopes", Verdict::Routed { tool: "work" }),
    ("list_orgs", Verdict::Routed { tool: "work" }),
    ("org_suggestions", Verdict::Routed { tool: "work" }),
    (
        "session_conversation",
        Verdict::Routed {
            tool: "session_conversation",
        },
    ),
    (
        "session_tool_detail",
        Verdict::LocalOnly {
            instead: "the hub exposes no tool for one tool call's input and result; the \
                      Conversation tab's tool lines still come from session_conversation",
        },
    ),
    (
        "session_activity",
        Verdict::Routed {
            tool: "session_activity",
        },
    ),
    (
        "restart_session",
        Verdict::Routed {
            tool: "restart_session",
        },
    ),
    (
        "send_prompt",
        Verdict::Routed {
            tool: "send_prompt",
        },
    ),
    (
        "spawn_review",
        Verdict::Routed {
            tool: "spawn_review",
        },
    ),
    (
        "recreate_session",
        Verdict::Routed {
            tool: "recreate_session",
        },
    ),
    (
        "restore_host_sessions",
        Verdict::Routed {
            tool: "restore_host_sessions",
        },
    ),
    (
        "discover_lost_sessions",
        Verdict::Routed {
            tool: "discover_lost_sessions",
        },
    ),
    (
        "move_session",
        Verdict::Routed {
            tool: "move_session",
        },
    ),
    (
        "resolve_move",
        Verdict::Routed {
            tool: "resolve_move",
        },
    ),
    (
        "dismiss_ghost_session",
        Verdict::Routed {
            tool: "dismiss_ghost_session",
        },
    ),
    (
        "dismiss_agent_session",
        Verdict::LocalOnly {
            instead: "use Kill instead: the hub's kill_session removes an inactive agent \
                      from the list exactly as this would. It is not routed here because the \
                      two differ on a WORKING agent, which this refuses and kill_session \
                      stops",
        },
    ),
    (
        "new_bg_session",
        Verdict::Routed {
            tool: "new_bg_session",
        },
    ),
    (
        "purge_project",
        Verdict::LocalOnly {
            instead: "it deletes Claude Code state on every host over this machine's SSH \
                      connections and the hub exposes no tool for it; purge from the hub",
        },
    ),
    (
        "get_fleet_settings",
        Verdict::LocalOnly {
            instead: "these settings drive the reconcile tick, the GC sweeper and the \
                      playbooks, which the hub runs and this app does not; read them on the \
                      hub with get_settings (master token)",
        },
    ),
    (
        "set_fleet_setting",
        Verdict::LocalOnly {
            instead: "these settings drive the reconcile tick, the GC sweeper and the \
                      playbooks, which the hub runs and this app does not; change them on \
                      the hub with set_setting (master token)",
        },
    ),
    ("list_tasks", Verdict::Routed { tool: "list_tasks" }),
    // ── tasks ───────────────────────────────────────────────────────────────
    (
        "cancel_task",
        Verdict::Routed {
            tool: "cancel_task",
        },
    ),
    // ── the Files tab's reads, and the upload that feeds it ─────────────────
    (
        "repo_changes",
        Verdict::Routed {
            tool: "repo_changes",
        },
    ),
    ("repo_tree", Verdict::Routed { tool: "repo_tree" }),
    ("repo_file", Verdict::Routed { tool: "repo_file" }),
    ("repo_diff", Verdict::Routed { tool: "repo_diff" }),
    (
        "upload_to_session",
        Verdict::SameInBoth {
            why: "the same story as `pty_open`: the bytes are on this machine and so is the \
                  `ssh` that carries them, addressed by the alias passed in, reading no \
                  state.db. It is the drop handler behind the terminal pane, so it has to \
                  work wherever that pane attaches",
        },
    ),
    ("pick_attachments", Verdict::SameInBoth { why: WHY_ATTACH }),
    (
        "attachment_preview",
        Verdict::SameInBoth { why: WHY_ATTACH },
    ),
    (
        "attachment_describe",
        Verdict::SameInBoth { why: WHY_ATTACH },
    ),
    (
        "upload_attachments",
        Verdict::SameInBoth { why: WHY_ATTACH },
    ),
    ("repo_log", Verdict::Routed { tool: "repo_log" }),
    (
        "repo_branches",
        Verdict::Routed {
            tool: "repo_branches",
        },
    ),
    (
        "repo_commit",
        Verdict::Routed {
            tool: "repo_commit",
        },
    ),
    (
        "repo_commit_diff",
        Verdict::Routed {
            tool: "repo_commit_diff",
        },
    ),
    // ── the Files tab's git writes — none of them has a tool ────────────────
    (
        "repo_checkout",
        Verdict::LocalOnly {
            instead: NO_GIT_WRITE_TOOL,
        },
    ),
    (
        "repo_checkout_commit",
        Verdict::LocalOnly {
            instead: NO_GIT_WRITE_TOOL,
        },
    ),
    (
        "repo_create_branch",
        Verdict::LocalOnly {
            instead: NO_GIT_WRITE_TOOL,
        },
    ),
    (
        "repo_delete_branch",
        Verdict::LocalOnly {
            instead: NO_GIT_WRITE_TOOL,
        },
    ),
    (
        "repo_stage",
        Verdict::LocalOnly {
            instead: NO_GIT_WRITE_TOOL,
        },
    ),
    (
        "repo_unstage",
        Verdict::LocalOnly {
            instead: NO_GIT_WRITE_TOOL,
        },
    ),
    (
        "repo_commit_create",
        Verdict::LocalOnly {
            instead: NO_GIT_WRITE_TOOL,
        },
    ),
    (
        "repo_fetch",
        Verdict::LocalOnly {
            instead: NO_GIT_WRITE_TOOL,
        },
    ),
    (
        "repo_pull",
        Verdict::LocalOnly {
            instead: NO_GIT_WRITE_TOOL,
        },
    ),
    (
        "repo_push",
        Verdict::LocalOnly {
            instead: NO_GIT_WRITE_TOOL,
        },
    ),
    // ── hosts and accounts ──────────────────────────────────────────────────
    (
        "discover_hosts",
        Verdict::LocalOnly {
            instead: "it reads this machine's ~/.ssh/config, not the hub's — register hosts \
                      on the hub itself with `fleet-hub` or a standalone app",
        },
    ),
    ("list_hosts", Verdict::Routed { tool: "list_hosts" }),
    (
        "list_accounts",
        Verdict::Routed {
            tool: "list_accounts",
        },
    ),
    (
        "add_host",
        Verdict::LocalOnly {
            instead: "registering a host is fleet administration, which the hub reserves for \
                      its own operator — add it there with `fleet-hub`",
        },
    ),
    ("probe_host", Verdict::Routed { tool: "probe_host" }),
    (
        "probe_ssh_alias",
        Verdict::LocalOnly {
            instead: "it SSHes from this machine to preview a host for the Add-host dialog; \
                      the hub is the one that must be able to reach it",
        },
    ),
    (
        "remove_host",
        Verdict::LocalOnly {
            instead: "removing a host is fleet administration, which the hub reserves for \
                      its own operator — remove it there with `fleet-hub`",
        },
    ),
    (
        "hide_host",
        Verdict::LocalOnly {
            instead: "hiding a host is fleet administration, which the hub reserves for its \
                      own operator — hide it there with `fleet-hub`",
        },
    ),
    (
        "set_account_nickname",
        Verdict::LocalOnly {
            instead: "the nickname lives in the hub's database and there is no tool to set \
                      it; rename the account on the hub",
        },
    ),
    // ── account usage ───────────────────────────────────────────────────────
    (
        "list_account_usage",
        Verdict::LocalOnly {
            instead: "this app does not poll account usage while a hub owns the fleet, so \
                      the cache is empty; read usage on the hub",
        },
    ),
    (
        "refresh_account_usage",
        Verdict::LocalOnly {
            instead: "it reads the account's usage over this machine's SSH connection to the \
                      host; refresh it on the hub",
        },
    ),
    // ── this app's own control API, and the hooks that report to it ─────────
    (
        "mcp_status",
        Verdict::LocalOnly {
            instead: "this app runs no embedded control API while a hub owns the fleet; the \
                      hub is the control API",
        },
    ),
    (
        "mcp_configure",
        Verdict::LocalOnly {
            instead: "starting a second control API against a fleet the hub already owns is \
                      the failure remote mode exists to prevent; configure the hub's",
        },
    ),
    (
        "install_fleet_hook",
        Verdict::LocalOnly {
            instead: "the hook it installs points at this app's control API, which is not \
                      running; install it from the hub",
        },
    ),
    (
        "provision_hosts",
        Verdict::LocalOnly {
            instead: "it rewrites every host's hook block to report to this app; provision \
                      from the hub with `fleet-hub`",
        },
    ),
    (
        "list_host_tokens",
        Verdict::LocalOnly {
            instead: "these are this app's own per-host tokens, not the hub's; list them on \
                      the hub",
        },
    ),
    (
        "set_host_token_mode",
        Verdict::LocalOnly {
            instead: "these are this app's own per-host tokens, not the hub's; change the \
                      mode on the hub",
        },
    ),
    (
        "rotate_host_token",
        Verdict::LocalOnly {
            instead: "it re-provisions the host to report to this app; rotate the token on \
                      the hub",
        },
    ),
    (
        "mcp_confirm",
        Verdict::SameInBoth {
            why: "answers this process's own confirm queue, which is empty in remote mode — \
                  answering nothing is correct",
        },
    ),
    (
        "mcp_pending_confirms",
        Verdict::SameInBoth {
            why: "the same queue, the same reason",
        },
    ),
    // ── the UX agent's operator session ─────────────────────────────────────
    //
    // Both route unconditionally. The operator panel is the same panel on a
    // hub-backed desktop, and the phone slice inherits these tools
    // unchanged — a `LocalOnly` verdict here would have closed that door.
    // It is also the only correct answer: `service::operator`'s file writes
    // go through `provision::write_host_file*`, which calls
    // `ensure_local_allowed`, so in hub-client mode the desktop must never
    // run this against ITS OWN "local" — the hub's "local" is the one that
    // matters.
    (
        "ensure_operator",
        Verdict::Routed {
            tool: "ensure_operator",
        },
    ),
    (
        "operator_status",
        Verdict::Routed {
            tool: "operator_status",
        },
    ),
    // ── the pairing itself — about THIS process, either way ─────────────────
    (
        "hub_status",
        Verdict::SameInBoth {
            why: "reports which fleet THIS window is onto. Asking a hub would be circular, \
                  and Settings needs the answer most when the hub is unreachable",
        },
    ),
    (
        "hub_pair",
        Verdict::SameInBoth {
            why: "points this process at a hub. It talks to POST /pair — the one \
                  unauthenticated route, and not an MCP tool at all",
        },
    ),
    (
        "hub_disconnect",
        Verdict::SameInBoth {
            why: "forgets this machine's own token and setting. It revokes nothing on the \
                  hub: only an operator can, and a paired client is refused revoke_client by \
                  design",
        },
    ),
    (
        "hub_connection",
        Verdict::SameInBoth {
            why: "reports whether THIS process's event stream to the hub is up. Asking the \
                  hub would be circular, and the answer matters most exactly when the hub \
                  cannot be reached",
        },
    ),
    (
        "hub_stranded_token",
        Verdict::SameInBoth {
            why: "asks THIS machine's own token store whether a pairing that crashed before \
                  writing its URL left a credential behind. There is no hub to ask — the \
                  whole state is that no hub is configured",
        },
    ),
    (
        "report_client_error",
        Verdict::SameInBoth {
            why: "queues a frontend error in THIS process's report ring; in standalone \
                  nothing drains it, so the push is a no-op rather than a refusal",
        },
    ),
    // ── onboarding ──────────────────────────────────────────────────────────
    (
        "check_local_prereqs",
        Verdict::LocalOnly {
            instead: "the onboarding checklist is about running a fleet from this machine, \
                      which the hub is doing instead",
        },
    ),
    (
        "tunnel_status",
        Verdict::LocalOnly {
            instead: "the tunnels belong to the process that owns the fleet; check them on \
                      the hub",
        },
    ),
    // ── the asset catalog — a git checkout the hub client does not have ─────
    (
        "catalog_config",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_configure",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_load",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_list_assets",
        Verdict::LocalOnly {
            instead: "the hub does serve this list (its read-only list_assets tool, open to \
                      any paired client), but the Assets panel is built on the catalog's \
                      configuration and git checkout, which only the machine that owns the \
                      fleet has; call list_assets on the hub, or browse the catalog on that \
                      machine",
        },
    ),
    (
        "catalog_get_asset",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_list_layers",
        Verdict::LocalOnly {
            instead: "the hub does serve this (its read-only list_layers tool), but the \
                      layer definitions live in the catalog's git checkout, which only the \
                      machine that owns the fleet has; call list_layers on the hub, or work \
                      on the catalog there",
        },
    ),
    (
        "catalog_resolve_preview",
        Verdict::LocalOnly {
            instead: "the hub has a resolve_preview tool, but it answers a summary — kind, \
                      name and version per asset — while this command returns the full \
                      Resolution the UI renders, so routing it would silently drop every \
                      asset body; call resolve_preview on the hub for the summary, or \
                      resolve on the machine that owns the fleet",
        },
    ),
    (
        "catalog_propose_layers",
        Verdict::LocalOnly {
            instead: "the hub does serve this (its read-only propose_layers tool), but a \
                      proposal is only useful where the layers can then be written — the \
                      catalog's git checkout, which only the machine that owns the fleet \
                      has; call propose_layers on the hub, or propose on that machine",
        },
    ),
    (
        "catalog_set_host_layers",
        Verdict::LocalOnly {
            instead: "the hub has a set_host_layers tool, but it is master-only — a host's \
                      layer assignment decides what the next apply_sync writes to its \
                      filesystem — and a paired client is never the master; set layers on \
                      the machine that owns the fleet",
        },
    ),
    (
        "catalog_layer_template",
        Verdict::LocalOnly {
            instead: "a template is the first step of authoring a layer into the catalog's \
                      git checkout, and catalog_write_layer refuses here for want of that \
                      checkout; the hub exposes no layer-authoring tool, so author on the \
                      machine that owns the fleet",
        },
    ),
    (
        "catalog_write_layer",
        Verdict::LocalOnly {
            instead: "writing a layer edits a file in the catalog's git checkout, which only \
                      the machine that owns the fleet has, and the hub exposes no \
                      layer-authoring tool; author on that machine",
        },
    ),
    (
        "catalog_delete_layer",
        Verdict::LocalOnly {
            instead: "deleting a layer removes a file from the catalog's git checkout, which \
                      only the machine that owns the fleet has, and the hub exposes no \
                      layer-authoring tool; author on that machine",
        },
    ),
    (
        "catalog_import_host",
        Verdict::LocalOnly {
            instead: "the hub has this as its import_assets tool, but the import lands in \
                      the catalog's git checkout, which only the machine that owns the fleet \
                      has; call import_assets on the hub, or import on that machine",
        },
    ),
    (
        "assets_scan_hosts",
        Verdict::LocalOnly {
            instead: "the hub has this as its scan_assets tool, but its result feeds an \
                      inventory panel built on the catalog checkout, which only the machine \
                      that owns the fleet has; call scan_assets on the hub, or scan from \
                      that machine",
        },
    ),
    (
        "assets_inventory",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_plan_sync",
        Verdict::LocalOnly {
            instead: "the hub has this as its plan_sync tool, but the plan is shown in a \
                      sync panel built on the catalog checkout, which only the machine that \
                      owns the fleet has; call plan_sync on the hub, or plan on that machine",
        },
    ),
    (
        "catalog_apply_sync",
        Verdict::LocalOnly {
            instead: "the hub's apply_sync is master-only: a paired client is never the \
                      fleet's administrator, and a sync writes to every host over SSH; run \
                      the sync on the hub",
        },
    ),
    (
        "catalog_last_sync",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_list_secrets",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_set_secret",
        Verdict::LocalOnly {
            instead: "the hub's set_secret is master-only: a paired client is never the \
                      fleet's administrator, and the sync secrets belong to the machine that \
                      runs the sync; set it on the hub",
        },
    ),
    (
        "catalog_delete_secret",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_create_asset",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_update_asset",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_delete_asset",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_add_resource",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_remove_resource",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_lint_asset",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_lint_all",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_commit_pending",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_push",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_repo_status",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_template",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    (
        "catalog_spawn_author_session",
        Verdict::LocalOnly {
            instead: CATALOG_IS_A_CHECKOUT,
        },
    ),
    // ── the terminal, and this process's cancellation registry ──────────────
    (
        "pty_open",
        Verdict::SameInBoth {
            why: "the attach is this machine's own `ssh … tmux attach`, built from the alias \
                  and tmux name passed in; it reads no state.db and the hub is not in the \
                  path, so a paired client attaches exactly as a standalone app does. The \
                  session it cannot attach is one on an AGENT host, which has no SSH route \
                  from anywhere — the terminal pane declines that one itself",
        },
    ),
    (
        "pty_write",
        Verdict::SameInBoth {
            why: "acts on whatever is attached; with pty_open refused nothing ever is, so \
                  E_PTY_CLOSED is the true answer",
        },
    ),
    (
        "pty_resize",
        Verdict::SameInBoth {
            why: "the same as pty_write",
        },
    ),
    (
        "pty_close",
        Verdict::SameInBoth {
            why: "the same as pty_write — guarding it would make closing fail",
        },
    ),
    (
        "pty_drain",
        Verdict::SameInBoth {
            why: "the same as pty_write",
        },
    ),
    (
        "cancel_command",
        Verdict::SameInBoth {
            why: "the cancellation registry is this process's, and the call it cancels is \
                  one this process started",
        },
    ),
];
