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
//! The rows are grouped as in `generate_handler!` (near enough its order),
//! so [`VERDICTS`] and `lib.rs` read side by side. `every_command_has_a_verdict` (in
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

/// Why the whole attachment family is the same in both modes: the composer
/// reads, measures and previews files on THIS machine's disk and copies them
/// over THIS machine's ssh, exactly as `upload_to_session` does behind the
/// terminal pane. They were `LocalOnly` on the premise that "a hub client has
/// nothing local" — but a hub client is a desktop app with a disk; what belongs
/// to the hub is the fleet's database and hosts, not this machine.
const WHY_ATTACH: &str = "the same story as `upload_to_session`: this machine has the disk, the file dialog and the `ssh` that carries the bytes, and the session is addressed by the alias passed in, reading no state.db. Being a window onto a hub does not take this machine away";

/// Why local workspace sync is the same in both modes: a link binds a
/// folder on this machine to a worktree it reaches over its own SSH, as
/// `upload_to_session` does; nothing in it is the hub's fleet.
const WHY_LOCAL_SYNC: &str = "a link binds a folder on THIS machine to a worktree this machine \
     reaches over its own SSH, the same story as `upload_to_session`; its rows live in this \
     machine's database, paired or not";

/// The verdict of `command`, or `None` when the table has no row for it —
/// which the tests make unshippable.
pub fn verdict(command: &str) -> Option<&'static Verdict> {
    VERDICTS
        .iter()
        .find(|(name, _)| *name == command)
        .map(|(_, v)| v)
}

/// Why this desktop's own update is the same in both modes (update design
/// S7, §6.5, §9): it is about THIS app's installed build. Paired, the check
/// goes to the hub's `/update/check` — an `update_proto` route, not an MCP
/// tool, and exempt from the contract gate so a desktop its hub can no
/// longer read still learns what to update to; standalone, it reads the
/// published channel. The install always happens on this machine, from a
/// target verified against the release key.
const WHY_SELF_UPDATE: &str = "this app's own build: paired, the hub's /update/check decides      (an update_proto route, not an MCP tool, exempt from the contract gate), standalone the      published channel does; either way the target is verified against the release key and      installed on THIS machine";

/// Trackers (work graph M3.1): the hub's `work_admin` is master-only.
const TRACKERS_ARE_ADMIN: &str = "trackers and their credentials are fleet administration: the \
     hub's work_admin is master-only, and a paired client is never the fleet's administrator; \
     configure them on the hub with `fleet-hub tracker add|set-credential|test`";

/// Jev's section proposals (Asana `status_map`): applying one writes the
/// tracker's settings, which is `work_admin`, master-only.
const SECTION_PROPOSALS_ARE_ADMIN: &str = "the decision model's Asana section proposals are \
     tracker administration: applying one writes the tracker's section map through the hub's \
     work_admin, master-only, and a paired client is never the fleet's administrator; decide them \
     on the hub with `fleet-hub decide proposals apply|reject`";

/// Retention (work graph M12.3): the hub sweeps its own store.
const RETENTION_IS_ADMIN: &str = "work retention is the hub's own sweep of its store: its \
     status and sweep_now are the hub's work_admin, master-only, and a paired client is never \
     the fleet's administrator; set the windows with set_setting and read the status on the \
     hub";

/// The eleven git-write commands of the Files tab, likewise.
const NO_GIT_WRITE_TOOL: &str =
    "the hub exposes no git-write tool — a remote client must not stage or commit under a \
     running agent; do it in the session, or from a standalone app";

const NO_DRAFT_TOOL: &str =
    "the hub exposes no draft tool — a commit message is drafted where the commit is made: \
     in the session, or from a standalone app";

/// Every command in `generate_handler!`, grouped as there, with its verdict.
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
    (
        "set_tray_state",
        Verdict::SameInBoth {
            why: "this window's own tray icon, which it has either way",
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
        Verdict::Routed {
            tool: "add_project",
        },
    ),
    (
        "list_github_repos",
        Verdict::Routed {
            tool: "list_github_repos",
        },
    ),
    (
        "project_picks",
        Verdict::Routed {
            tool: "project_picks",
        },
    ),
    (
        "set_project_pick",
        Verdict::Routed {
            tool: "set_project_pick",
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
        "shell_terminals",
        Verdict::Routed {
            tool: "shell_terminals",
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
        "list_host_worktrees",
        Verdict::Routed {
            tool: "list_host_worktrees",
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
    // The Label field (M15 G2.7): the hub's own `set_session_tags`, `own`.
    (
        "set_session_tags",
        Verdict::Routed {
            tool: "set_session_tags",
        },
    ),
    // Link / Not related on a related-session proposal (M15 G4.3): the
    // hub's own `decide_related_session`, `own`.
    (
        "decide_related_session",
        Verdict::Routed {
            tool: "decide_related_session",
        },
    ),
    (
        "touch_session_viewed",
        Verdict::Routed {
            tool: "touch_session_viewed",
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
        "unarchive_session_work",
        Verdict::Routed { tool: "work_link" },
    ),
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
    // Work graph M13.4c: a Claude-written summary of past work (on demand,
    // D10). Routed: the run happens on the session's own host, which the
    // hub reaches.
    ("summarize_past_work", Verdict::Routed { tool: "work_link" }),
    // Work graph M9.6: one ticket, one sibling session per repository.
    ("start_work_multi", Verdict::Routed { tool: "work_link" }),
    // Work graph M11.1: "Name this work…" — local work items, listed from
    // `work`, named and renamed through `work_link { name }`.
    ("name_session_work", Verdict::Routed { tool: "work_link" }),
    ("rename_work_item", Verdict::Routed { tool: "work_link" }),
    // Shared work context (design 2026-09-29): a task or subtask a person
    // writes, and a person's decision on an agent's proposal.
    ("create_work_task", Verdict::Routed { tool: "work_link" }),
    // The board (sprints design 2026-09-28 §6c): a person's status for a
    // native item, set by dragging its card to a column.
    ("set_work_status", Verdict::Routed { tool: "work_link" }),
    // Task editing: a person's title, notes and assignees for a native item.
    ("edit_work_item", Verdict::Routed { tool: "work_link" }),
    (
        "accept_work_proposal",
        Verdict::Routed { tool: "work_link" },
    ),
    (
        "reject_work_proposal",
        Verdict::Routed { tool: "work_link" },
    ),
    // Work graph M14: the Work view — eight reads of `work` and ten
    // decisions of `work_link`, every one the same on a paired desktop.
    ("work_tree", Verdict::Routed { tool: "work" }),
    ("work_task", Verdict::Routed { tool: "work" }),
    ("work_session_tasks", Verdict::Routed { tool: "work" }),
    ("work_review", Verdict::Routed { tool: "work" }),
    ("work_rules", Verdict::Routed { tool: "work" }),
    ("work_rule_preview", Verdict::Routed { tool: "work" }),
    ("work_views", Verdict::Routed { tool: "work" }),
    ("work_org_impact", Verdict::Routed { tool: "work" }),
    ("set_primary_work", Verdict::Routed { tool: "work_link" }),
    ("switch_session_work", Verdict::Routed { tool: "work_link" }),
    (
        "reconsider_work_link",
        Verdict::Routed { tool: "work_link" },
    ),
    ("ack_work_link", Verdict::Routed { tool: "work_link" }),
    ("decide_work_batch", Verdict::Routed { tool: "work_link" }),
    ("place_work", Verdict::Routed { tool: "work_link" }),
    ("assign_work_org", Verdict::Routed { tool: "work_link" }),
    ("save_work_rule", Verdict::Routed { tool: "work_link" }),
    ("delete_work_rule", Verdict::Routed { tool: "work_link" }),
    ("save_work_view", Verdict::Routed { tool: "work_link" }),
    ("delete_work_view", Verdict::Routed { tool: "work_link" }),
    // Orchestration O1: missions are a person's, fenced by owner and org
    // in fleet-core (`service::work::missions`), so every one routes.
    ("work_missions", Verdict::Routed { tool: "work" }),
    ("work_mission", Verdict::Routed { tool: "work" }),
    ("save_mission", Verdict::Routed { tool: "work_link" }),
    ("set_mission_state", Verdict::Routed { tool: "work_link" }),
    ("set_mission_repo", Verdict::Routed { tool: "work_link" }),
    ("set_mission_item", Verdict::Routed { tool: "work_link" }),
    ("import_mission_plan", Verdict::Routed { tool: "work_link" }),
    ("delete_mission", Verdict::Routed { tool: "work_link" }),
    // Orchestration O2: the mission graph's writes.
    ("set_work_dep", Verdict::Routed { tool: "work_link" }),
    ("set_work_hold", Verdict::Routed { tool: "work_link" }),
    (
        "accept_work_proposals",
        Verdict::Routed { tool: "work_link" },
    ),
    ("undo_work_accept", Verdict::Routed { tool: "work_link" }),
    // Orchestration O3: acceptance conditions and a person's check.
    ("set_work_done_when", Verdict::Routed { tool: "work_link" }),
    ("verify_work_item", Verdict::Routed { tool: "work_link" }),
    ("start_mission_wave", Verdict::Routed { tool: "work_link" }),
    ("retry_work_item", Verdict::Routed { tool: "work_link" }),
    ("plan_mission", Verdict::Routed { tool: "work_link" }),
    ("decide_mission_card", Verdict::Routed { tool: "work_link" }),
    ("grant_mission", Verdict::Routed { tool: "work_link" }),
    (
        "revoke_mission_grant",
        Verdict::Routed { tool: "work_link" },
    ),
    ("pause_all_missions", Verdict::Routed { tool: "work_link" }),
    (
        "mission_release_note",
        Verdict::Routed { tool: "work_link" },
    ),
    ("today_brief", Verdict::Routed { tool: "work_link" }),
    ("mission_triage", Verdict::Routed { tool: "work_link" }),
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
    // Jev `status_map` in assist: reading and deciding the section
    // proposals is tracker administration (an apply is `work_admin
    // update`); the hub's operator uses the CLI.
    (
        "status_map_proposals",
        Verdict::LocalOnly {
            instead: SECTION_PROPOSALS_ARE_ADMIN,
        },
    ),
    (
        "decide_status_map_proposal",
        Verdict::LocalOnly {
            instead: SECTION_PROPOSALS_ARE_ADMIN,
        },
    ),
    // Work graph M12.3: retention is the hub's own sweep; its status and
    // sweep_now are `work_admin`, master-only.
    (
        "work_retention_status",
        Verdict::LocalOnly {
            instead: RETENTION_IS_ADMIN,
        },
    ),
    (
        "work_retention_sweep",
        Verdict::LocalOnly {
            instead: RETENTION_IS_ADMIN,
        },
    ),
    // Work graph M3.4: reading tickets and starting work route like every
    // other work read and decision.
    ("list_trackers", Verdict::Routed { tool: "work" }),
    ("work_tickets", Verdict::Routed { tool: "work" }),
    ("work_lookup", Verdict::Routed { tool: "work" }),
    ("start_work", Verdict::Routed { tool: "work_link" }),
    // Task → session spec P-6: cancel a start nobody worked in.
    ("abandon_start", Verdict::Routed { tool: "work_link" }),
    // Task → session spec P-1: the start's dry run, the Work button's
    // popover. Nothing is made, so it routes exactly as the start does.
    ("preview_start_work", Verdict::Routed { tool: "work_link" }),
    // Work graph M5: orgs are the per-host tokens' security boundary. Since
    // org administration phase B the hub's `org_admin` lets its owner's own
    // trusted `full` device change them (and refuses anyone else), so they
    // route; reading them routes like every other work read.
    ("add_org", Verdict::Routed { tool: "org_admin" }),
    ("update_org", Verdict::Routed { tool: "org_admin" }),
    ("remove_org", Verdict::Routed { tool: "org_admin" }),
    ("add_org_rule", Verdict::Routed { tool: "org_admin" }),
    ("remove_org_rule", Verdict::Routed { tool: "org_admin" }),
    ("assign_host_org", Verdict::Routed { tool: "org_admin" }),
    ("assign_tracker_org", Verdict::Routed { tool: "org_admin" }),
    ("set_org_setting", Verdict::Routed { tool: "org_admin" }),
    ("set_org_member", Verdict::Routed { tool: "org_admin" }),
    ("remove_org_member", Verdict::Routed { tool: "org_admin" }),
    ("org_member_grants", Verdict::Routed { tool: "org_admin" }),
    // M15 step G2.10: a rule's live impact (a read) and the org's project
    // catalog.
    ("org_rule_preview", Verdict::Routed { tool: "org_admin" }),
    ("add_org_project", Verdict::Routed { tool: "org_admin" }),
    ("remove_org_project", Verdict::Routed { tool: "org_admin" }),
    ("revoke_org_share", Verdict::Routed { tool: "org_admin" }),
    ("narrow_org_share", Verdict::Routed { tool: "org_admin" }),
    (
        "revoke_org_member_grants",
        Verdict::Routed { tool: "org_admin" },
    ),
    ("list_orgs", Verdict::Routed { tool: "work" }),
    ("org_suggestions", Verdict::Routed { tool: "work" }),
    // Org administration phase B: the company's paired devices and people,
    // on the hub that holds them (`org_admin`; standalone, this desktop's
    // own store, where `pair_device` refuses — a code is the hub's).
    ("list_devices", Verdict::Routed { tool: "org_admin" }),
    ("pair_device", Verdict::Routed { tool: "org_admin" }),
    ("revoke_device", Verdict::Routed { tool: "org_admin" }),
    ("set_device_trust", Verdict::Routed { tool: "org_admin" }),
    ("update_device", Verdict::Routed { tool: "org_admin" }),
    ("bind_device_org", Verdict::Routed { tool: "org_admin" }),
    ("set_device_person", Verdict::Routed { tool: "org_admin" }),
    (
        "grant_device_catalog",
        Verdict::Routed { tool: "org_admin" },
    ),
    ("list_people", Verdict::Routed { tool: "org_admin" }),
    ("rename_person", Verdict::Routed { tool: "org_admin" }),
    ("disable_person", Verdict::Routed { tool: "org_admin" }),
    (
        "session_summary_since",
        Verdict::Routed {
            tool: "session_summary_since",
        },
    ),
    (
        "session_conversation",
        Verdict::Routed {
            tool: "session_conversation",
        },
    ),
    (
        "session_tool_detail",
        Verdict::Routed {
            tool: "session_tool_detail",
        },
    ),
    (
        "session_activity",
        Verdict::Routed {
            tool: "session_activity",
        },
    ),
    // Multi-user M1 (T13): sharing a session, and the one view of a live pane
    // a watcher gets. All six route — each command's argument struct is its
    // tool's params field for field — and all six are gated on the hub per
    // request, which is the point: a revoked grant stops the next call.
    //
    // `capture_session` is here for the WATCHER (R5-e). Sharing never confers
    // a terminal (spec §4.3 invariant 5), so `pty_open` is not a watcher's
    // path to the pane; this read-only snapshot is, and unlike an SSH
    // attach the hub can refuse the next poll. The owner still attaches.
    //
    // `session_claim` has no row because it has no command: parity fails on
    // the CALLER (the tool is `Access::HostToken`, which a desktop's client
    // token can never satisfy), and a UI claim button for an arbitrary org
    // member is what spec §4.3 forbids outright. `fleet-hub session claim`
    // is the operator's path.
    (
        "capture_session",
        Verdict::Routed {
            tool: "capture_session",
        },
    ),
    (
        "session_share",
        Verdict::Routed {
            tool: "session_share",
        },
    ),
    (
        "session_unshare",
        Verdict::Routed {
            tool: "session_unshare",
        },
    ),
    (
        "session_narrow",
        Verdict::Routed {
            tool: "session_narrow",
        },
    ),
    (
        "session_access",
        Verdict::Routed {
            tool: "session_access",
        },
    ),
    ("my_grants", Verdict::Routed { tool: "my_grants" }),
    (
        "restart_session",
        Verdict::Routed {
            tool: "restart_session",
        },
    ),
    (
        "rewind_conversation",
        Verdict::Routed {
            tool: "rewind_conversation",
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
        "queue_prompt",
        Verdict::Routed {
            tool: "queue_prompt",
        },
    ),
    (
        "queued_prompts",
        Verdict::Routed {
            tool: "queued_prompts",
        },
    ),
    (
        "cancel_queued_prompt",
        Verdict::Routed {
            tool: "queued_prompts",
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
        "adopt_session",
        Verdict::Routed {
            tool: "adopt_session",
        },
    ),
    (
        "lost_target",
        Verdict::Routed {
            tool: "lost_target",
        },
    ),
    (
        "place_transcript",
        Verdict::Routed {
            tool: "place_transcript",
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
    // The chip row is fleet state (see `service::quick_replies`), so a
    // paired desktop edits the hub's list — the same one the phone draws —
    // rather than a private copy that would disagree with it.
    (
        "quick_replies",
        Verdict::Routed {
            tool: "quick_replies",
        },
    ),
    (
        "set_quick_replies",
        Verdict::Routed {
            tool: "quick_replies",
        },
    ),
    // File downloads live on the machine that owns the fleet — the hub
    // when paired — so the list, a send and a removal are the hub's tools.
    (
        "list_downloads",
        Verdict::Routed {
            tool: "list_downloads",
        },
    ),
    ("send_file", Verdict::Routed { tool: "send_file" }),
    (
        "remove_download",
        Verdict::Routed {
            tool: "remove_download",
        },
    ),
    // Control's Library (9.7) is indexed on the machine that owns the
    // fleet, the hub when paired, beside the downloads; one tool by action.
    ("list_library", Verdict::Routed { tool: "library" }),
    ("add_library_items", Verdict::Routed { tool: "library" }),
    ("remove_library_item", Verdict::Routed { tool: "library" }),
    // Saving reads the row through `list_downloads` (refusing a file that
    // is not ready or not this client's), then streams the bytes from the
    // hub's `GET /downloads/<id>` into the file this machine's save dialog
    // picked.
    (
        "save_download",
        Verdict::Routed {
            tool: "list_downloads",
        },
    ),
    (
        "get_fleet_settings",
        Verdict::Routed {
            tool: "get_settings",
        },
    ),
    (
        "describe_fleet_settings",
        Verdict::Routed {
            tool: "get_settings",
        },
    ),
    (
        "list_pages",
        Verdict::SameInBoth {
            why: "the page specs are compiled into this binary: the same pages whichever \
                  process owns the fleet. What a page shows comes through its own commands, \
                  each with its own verdict",
        },
    ),
    (
        "fetch_page_source",
        Verdict::LocalOnly {
            instead: "a page's data sources read this fleet's store, which the hub owns; \
                      read the same numbers on the hub with usage_report",
        },
    ),
    (
        "flow_start",
        Verdict::LocalOnly {
            instead: "a flow administers the fleet this app owns, and the hub owns it; connect a \
                      tracker on the hub with fleet-hub tracker add <ticket-url>",
        },
    ),
    (
        "flow_submit",
        Verdict::LocalOnly {
            instead: "a flow administers the fleet this app owns, and the hub owns it; connect a \
                      tracker on the hub with fleet-hub tracker add <ticket-url>",
        },
    ),
    (
        "flow_back",
        Verdict::LocalOnly {
            instead: "a flow administers the fleet this app owns, and the hub owns it; connect a \
                      tracker on the hub with fleet-hub tracker add <ticket-url>",
        },
    ),
    (
        "flow_cancel",
        Verdict::LocalOnly {
            instead: "a flow administers the fleet this app owns, and the hub owns it; connect a \
                      tracker on the hub with fleet-hub tracker add <ticket-url>",
        },
    ),
    (
        "setting_proposals",
        Verdict::Routed {
            tool: "setting_proposals",
        },
    ),
    (
        "decide_setting_proposals",
        Verdict::Routed {
            tool: "decide_setting_proposals",
        },
    ),
    (
        "setting_history",
        Verdict::Routed {
            tool: "setting_history",
        },
    ),
    ("list_guides", Verdict::Routed { tool: "guide" }),
    ("decide_guide", Verdict::Routed { tool: "guide" }),
    ("remove_guide", Verdict::Routed { tool: "guide" }),
    ("list_forms", Verdict::Routed { tool: "ask" }),
    ("get_form", Verdict::Routed { tool: "ask" }),
    ("answer_form", Verdict::Routed { tool: "ask" }),
    ("decline_form", Verdict::Routed { tool: "ask" }),
    // Settings → Federation (Orbit Fleet 11.5): the hub's links to other
    // fleets' hubs. The master's and the hub owner's own device's; changing
    // a link also needs that device trusted (the tool checks).
    (
        "list_peer_links",
        Verdict::Routed {
            tool: "list_peer_links",
        },
    ),
    ("link_peer_hub", Verdict::Routed { tool: "link_peer" }),
    (
        "unlink_peer_hub",
        Verdict::Routed {
            tool: "unlink_peer",
        },
    ),
    // Settings → Updates (Orbit Fleet 11.9b): what each part of the fleet
    // runs, from the hub's update picture.
    (
        "update_check",
        Verdict::SameInBoth {
            why: WHY_SELF_UPDATE,
        },
    ),
    (
        "update_install",
        Verdict::SameInBoth {
            why: WHY_SELF_UPDATE,
        },
    ),
    (
        "list_update_targets",
        Verdict::Routed {
            tool: "update_status",
        },
    ),
    ("list_pull_requests", Verdict::Routed { tool: "prs" }),
    // Named Control API tokens (M15 step G2.8): a token for the fleet this
    // window is onto, so the hub's. Its tool serves the owner's trusted full
    // device read and act tokens; an admin one stays the hub master's.
    ("api_tokens", Verdict::Routed { tool: "api_tokens" }),
    // Start rules (Orbit Fleet 8.11): the hub decides and tallies its own
    // starts, so its rules are the ones that count.
    (
        "start_rules",
        Verdict::Routed {
            tool: "start_rules",
        },
    ),
    (
        "session_presence",
        Verdict::Routed {
            tool: "session_presence",
        },
    ),
    (
        "list_debug_devices",
        Verdict::Routed {
            tool: "debug_devices",
        },
    ),
    (
        "scan_debug_devices",
        Verdict::Routed {
            tool: "debug_devices",
        },
    ),
    (
        "update_debug_device",
        Verdict::Routed {
            tool: "debug_devices",
        },
    ),
    (
        "release_debug_device",
        Verdict::Routed {
            tool: "debug_devices",
        },
    ),
    (
        "forget_debug_device",
        Verdict::Routed {
            tool: "debug_devices",
        },
    ),
    (
        "boot_debug_device",
        Verdict::Routed {
            tool: "debug_devices",
        },
    ),
    (
        "shutdown_debug_device",
        Verdict::Routed {
            tool: "debug_devices",
        },
    ),
    (
        "claim_debug_device",
        Verdict::Routed {
            tool: "debug_devices",
        },
    ),
    (
        "install_debug_device",
        Verdict::Routed {
            tool: "debug_devices",
        },
    ),
    (
        "debug_device_logs",
        Verdict::Routed {
            tool: "debug_devices",
        },
    ),
    (
        "debug_device_screenshot",
        Verdict::Routed {
            tool: "debug_devices",
        },
    ),
    (
        "set_fleet_setting",
        Verdict::Routed {
            tool: "set_setting",
        },
    ),
    ("list_tasks", Verdict::Routed { tool: "list_tasks" }),
    // Orbit Fleet 8.3: the Automation screen's Runs list.
    ("list_runs", Verdict::Routed { tool: "runs" }),
    // Orbit Fleet 8.6: the Automation screen's Routines and the Inbox's failed runs.
    ("routines", Verdict::Routed { tool: "routines" }),
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
    ("repo_blame", Verdict::Routed { tool: "repo_blame" }),
    (
        "repo_branch_diff",
        Verdict::Routed {
            tool: "repo_branch_diff",
        },
    ),
    (
        "repo_range_diff",
        Verdict::Routed {
            tool: "repo_range_diff",
        },
    ),
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
        "repo_delete_merged_branches",
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
        "draft_commit_message",
        Verdict::LocalOnly {
            instead: NO_DRAFT_TOOL,
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
    // Orbit Fleet 4.9: only a hub accepts agents, so the job runs there; a
    // standalone desktop answers with the service's own refusal.
    (
        "install_agent",
        Verdict::Routed {
            tool: "install_agent",
        },
    ),
    (
        "agent_installs",
        Verdict::Routed {
            tool: "agent_installs",
        },
    ),
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
    // Host identity & health, task 5. LocalOnly, not Routed: the hub tool
    // is `Access::Master` and a paired desktop holds a client token, so
    // routing would be a guaranteed `E_FORBIDDEN` — the same reasoning as
    // `remove_host` (`commands/hosts.rs`).
    (
        "merge_host",
        Verdict::LocalOnly {
            instead: "merging one host's rows into another is fleet administration, which the \
                      hub reserves for its own operator — run it there with `fleet-hub host merge \
                      <from> <into>`",
        },
    ),
    (
        "check_host",
        Verdict::LocalOnly {
            instead: "the health checklist reads a host's settings over this app's own SSH; \
                      repair a host's hooks from the hub with `fleet-hub provision --host <alias>`",
        },
    ),
    (
        "list_host_setups",
        Verdict::LocalOnly {
            instead: "the add-host wizard adds a host of this machine's ~/.ssh/config and checks \
                      it over this app's own SSH; the hub adds hosts with `add_host` and installs \
                      fleet-agent with `install_agent`",
        },
    ),
    (
        "save_host_setup",
        Verdict::LocalOnly {
            instead: "the add-host wizard adds a host of this machine's ~/.ssh/config and checks \
                      it over this app's own SSH; the hub adds hosts with `add_host` and installs \
                      fleet-agent with `install_agent`",
        },
    ),
    (
        "discard_host_setup",
        Verdict::LocalOnly {
            instead: "the add-host wizard adds a host of this machine's ~/.ssh/config and checks \
                      it over this app's own SSH; the hub adds hosts with `add_host` and installs \
                      fleet-agent with `install_agent`",
        },
    ),
    (
        "run_host_setup_check",
        Verdict::LocalOnly {
            instead: "the add-host wizard adds a host of this machine's ~/.ssh/config and checks \
                      it over this app's own SSH; the hub adds hosts with `add_host` and installs \
                      fleet-agent with `install_agent`",
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
        Verdict::Routed {
            tool: "account_usage",
        },
    ),
    (
        "account_spend",
        Verdict::LocalOnly {
            instead: "this app collects no usage while a hub owns the fleet, so its store \
                      has no spend per account; read usage on the hub",
        },
    ),
    (
        "account_usage_history",
        Verdict::LocalOnly {
            instead: "this app does not poll account usage while a hub owns the fleet, so \
                      it keeps no history; read usage on the hub",
        },
    ),
    (
        "check_account_headroom",
        Verdict::Routed {
            tool: "check_account_headroom",
        },
    ),
    (
        "propose_host_placement",
        Verdict::LocalOnly {
            instead: "the decision model and the account usage are the hub's while it owns \
                      the fleet; pick the host as usual",
        },
    ),
    (
        "record_host_placement",
        Verdict::LocalOnly {
            instead: "the decision model's runs are recorded on the hub that owns the fleet; \
                      nothing to record here",
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
                      from the hub with `fleet-hub provision [--host <alias>] [--content-only]`",
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
    // Redesign step 9.2: the operator's calls that wait for a person wait in
    // the queue of the server they reached, which on a hub-backed desktop is
    // the hub's. The owner's device lists and answers it there.
    (
        "mcp_confirm",
        Verdict::Routed {
            tool: "answer_mcp_confirm",
        },
    ),
    (
        "mcp_pending_confirms",
        Verdict::Routed {
            tool: "mcp_confirms",
        },
    ),
    // Redesign step 9.3: the receipts of what the agent handed on live
    // where the agent runs, which on a hub-backed desktop is the hub.
    (
        "control_handoffs",
        Verdict::Routed {
            tool: "control_handoffs",
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
    // Redesign step 9.9 (Jev K2): the hub decides where a Control message
    // goes, since it owns the fleet and the decision envelope.
    (
        "control_route_propose",
        Verdict::Routed {
            tool: "control_route",
        },
    ),
    (
        "control_route_follow",
        Verdict::Routed {
            tool: "control_route",
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
        "hub_retry_now",
        Verdict::SameInBoth {
            why: "cuts THIS process's wait before it reconnects to the hub. It is the \
                  banner's Retry now, pressed exactly when the hub cannot be reached",
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
        "offline_local_sessions",
        Verdict::SameInBoth {
            why: "lists the tmux sessions on THIS machine for Open offline, when the hub \
                  cannot be reached. It reads this machine's own tmux server only: no \
                  state.db, no SSH, no host the hub manages, and it writes nothing",
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
    // ── the asset catalog — on a hub, the hub's checkout ─────────────────────
    //
    // Everything but the overview's list and scan routes to `catalog_admin`
    // with the command's own arguments (`catalog::admin::AdminCall`, one
    // variant per command), which answers the master and a paired client the
    // operator granted (`fleet-hub client grant <name> assets`) and refuses
    // anyone else with E_FORBIDDEN — the panel then shows its read-only
    // overview. Parity by construction: the variant carries the command's
    // argument struct, and the answer is the same type the local call returns.
    (
        "catalog_config",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_configure",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_load",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    // The overview's read, open to every paired client: the hub's
    // `list_assets` over its own catalogs and inventory — the same
    // `AssetListing`. Assets M5: this desktop sends `all_catalogs: true`;
    // the hub lists every catalog for the master or an unbound full client
    // and personal only for anyone else (or when the flag is absent).
    (
        "catalog_list_assets",
        Verdict::Routed {
            tool: "list_assets",
        },
    ),
    (
        "catalog_get_asset",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_list_layers",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_resolve_preview",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_propose_layers",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_set_host_layers",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_set_host_harnesses",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_layer_template",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_write_layer",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_delete_layer",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_import_host",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    // Read-only on the hosts, open to a paired client: it refreshes the
    // inventory the overview reads, and returns the same per-host
    // `HostScanResult`s.
    (
        "assets_scan_hosts",
        Verdict::Routed {
            tool: "scan_assets",
        },
    ),
    (
        "assets_inventory",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_plan_sync",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    // The hub's plan (`catalog_plan_sync` above parked it there); the hub
    // runs its `apply_sync` confirm gate on it, and drops `call_id`.
    (
        "catalog_apply_sync",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_last_sync",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_list_secrets",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_set_secret",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_delete_secret",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_create_asset",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_update_asset",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_delete_asset",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    // The file is on this machine: the routed function reads it here, with
    // the local path's checks and size limit, and sends its bytes
    // (`AdminCall::AddResourceBytes`).
    (
        "catalog_add_resource",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_remove_resource",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_lint_asset",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_lint_all",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_commit_pending",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_push",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_repo_status",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_template",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    // Assets M5 (R13): the workspace's reads. The footer's catalog chips:
    // `catalog_admin { list_catalogs }`, the master's or an unbound full
    // client's — no grant (M3 PF15).
    (
        "catalog_list_catalogs",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    // The Inbox's cards: `changesets { list }`, the master's or an unbound
    // full client's. The card verbs follow below.
    (
        "catalog_list_changesets",
        Verdict::Routed { tool: "changesets" },
    ),
    // Assets M6 (R8): the card verbs — `changesets { list(id) | apply |
    // undo | dismiss | reject_item | propose | propose_layer }`. The hub
    // checks the caller's grant per catalog the card touches and runs its
    // confirm gate for a rollout or restore apply; the desktop never sends
    // a nonce (as `catalog_apply_sync`). A hub before M6 refuses
    // `propose_layer` with E_INVALID (R13 precedent) — no contract bump.
    (
        "catalog_get_changeset",
        Verdict::Routed { tool: "changesets" },
    ),
    (
        "catalog_apply_changeset",
        Verdict::Routed { tool: "changesets" },
    ),
    (
        "catalog_undo_changeset",
        Verdict::Routed { tool: "changesets" },
    ),
    (
        "catalog_dismiss_changeset",
        Verdict::Routed { tool: "changesets" },
    ),
    (
        "catalog_reject_changeset_items",
        Verdict::Routed { tool: "changesets" },
    ),
    (
        "catalog_propose_changesets",
        Verdict::Routed { tool: "changesets" },
    ),
    (
        "catalog_propose_layer_change",
        Verdict::Routed { tool: "changesets" },
    ),
    // Assets M6 (R9): the catalog set (add / remove: the master only on a
    // hub; admit / unadmit: a grant on that catalog), one catalog's layers,
    // one host's provenance (the MCP resolve_preview projection) and a
    // drifted asset's two texts (catalog_admin drift_diff — a hub before M6
    // refuses the action with E_INVALID; no contract bump).
    (
        "catalog_add_catalog",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_remove_catalog",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_admit_catalog",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_unadmit_catalog",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_list_layers_in",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_host_provenance",
        Verdict::Routed {
            tool: "resolve_preview",
        },
    ),
    (
        "catalog_drift_diff",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    // One catalog's repo status by name: `catalog_admin { repo_status }`
    // with the tool's `catalog` parameter; needs a grant on that catalog.
    (
        "catalog_repo_status_in",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    // The Inspector's History: `catalog_admin { asset_history }`, per
    // catalog, with a grant on it. A hub before M5 refuses the action with
    // E_INVALID (unknown variant) — no contract bump (R13).
    (
        "catalog_asset_history",
        Verdict::Routed {
            tool: "catalog_admin",
        },
    ),
    (
        "catalog_spawn_author_session",
        Verdict::LocalOnly {
            instead: "an author session is a Claude session started in the catalog's \
                      checkout on the machine that owns it, and the hub has no tool that \
                      starts one; edit the assets from this panel, or start a session in the \
                      checkout on the hub's machine",
        },
    ),
    // ── the terminal, and this process's cancellation registry ──────────────
    (
        "pty_open",
        Verdict::SameInBoth {
            why: "the attach is this machine's own `ssh … tmux attach`, built from the \
                  alias and tmux name passed in; it reads no state.db and the hub is not \
                  in the path, so a paired client attaches exactly as a standalone app \
                  does. That is also why it is NOT where a grant is enforced: the hub \
                  cannot refuse this attach and cannot revoke it once it is up, so \
                  sharing never confers a terminal (multi-user M1) and the pane itself \
                  declines to attach a session this client does not own, offering \
                  capture_session's read-only snapshot instead. A session on an AGENT \
                  host is attempted like any other — that transport says the HUB cannot \
                  dial the host, not that this machine cannot — and a failure is \
                  explained after it happens",
        },
    ),
    (
        "pty_write",
        Verdict::SameInBoth {
            why: "acts on a pty THIS process opened, named by the id it was opened \
                  under, and pty_open is the same in both modes, so there is one answer \
                  either way; E_PTY_CLOSED when nothing is attached under that id, which \
                  includes every session the pane declined to attach",
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
        "open_session_in_editor",
        Verdict::SameInBoth {
            why: "the same story as pty_open: VS Code and the `ssh` that asks the pane \
                  for its folder are this machine's, built from the alias and tmux name \
                  passed in; it reads no state.db and the hub is not in the path",
        },
    ),
    (
        "open_terminal_window",
        Verdict::SameInBoth {
            why: "a window of this app, whose pane attaches through pty_open like the \
                  main window's; it reads no state.db and the hub is not in the path",
        },
    ),
    // ── the voice relay's microphone claim ──────────────────────────────────
    (
        "voice_claim",
        Verdict::SameInBoth {
            why: "the microphone is this machine's: standalone it is registered with the \
                  embedded server; paired, the desktop holds /voice/source open on the hub",
        },
    ),
    (
        "voice_release",
        Verdict::SameInBoth {
            why: "releases whichever claim voice_claim made on this machine",
        },
    ),
    // ── local workspace sync ────────────────────────────────────────────────
    (
        "list_local_workspaces",
        Verdict::SameInBoth {
            why: "links bind a folder on THIS machine, so they live in this machine's \
                  database, paired or not",
        },
    ),
    (
        "enable_local_workspace",
        Verdict::SameInBoth {
            why: "the folder is on THIS machine and the sync runs over its own SSH; paired, \
                  the session and its project are read from the hub (list_sessions, \
                  list_projects) and the worktree's path from the session's pane",
        },
    ),
    (
        "pause_local_workspace",
        Verdict::SameInBoth {
            why: WHY_LOCAL_SYNC,
        },
    ),
    (
        "resume_local_workspace",
        Verdict::SameInBoth {
            why: WHY_LOCAL_SYNC,
        },
    ),
    (
        "sync_local_workspace_now",
        Verdict::SameInBoth {
            why: WHY_LOCAL_SYNC,
        },
    ),
    (
        "disconnect_local_workspace",
        Verdict::SameInBoth {
            why: WHY_LOCAL_SYNC,
        },
    ),
    (
        "set_local_workspace_excludes",
        Verdict::SameInBoth {
            why: WHY_LOCAL_SYNC,
        },
    ),
    (
        "resolve_local_workspace_conflict",
        Verdict::SameInBoth {
            why: WHY_LOCAL_SYNC,
        },
    ),
    (
        "open_local_workspace",
        Verdict::SameInBoth {
            why: WHY_LOCAL_SYNC,
        },
    ),
    (
        "local_workspace_changes",
        Verdict::SameInBoth {
            why: WHY_LOCAL_SYNC,
        },
    ),
    (
        "local_workspace_diff",
        Verdict::SameInBoth {
            why: WHY_LOCAL_SYNC,
        },
    ),
    (
        "commit_local_workspace",
        Verdict::SameInBoth {
            why: WHY_LOCAL_SYNC,
        },
    ),
    (
        "discard_local_workspace_changes",
        Verdict::SameInBoth {
            why: WHY_LOCAL_SYNC,
        },
    ),
    (
        "dismiss_local_workspace_activity",
        Verdict::SameInBoth {
            why: WHY_LOCAL_SYNC,
        },
    ),
    (
        "compare_local_conflict",
        Verdict::SameInBoth {
            why: WHY_LOCAL_SYNC,
        },
    ),
    (
        "keep_both_local_conflict",
        Verdict::SameInBoth {
            why: WHY_LOCAL_SYNC,
        },
    ),
    (
        "ask_ai_about_local_changes",
        Verdict::SameInBoth {
            why: "the link and its folder are THIS machine's; paired, the session is found \
                  among the hub's (list_sessions) and the prompt goes through the hub's \
                  send_prompt",
        },
    ),
    (
        "set_local_workspace_driver",
        Verdict::SameInBoth {
            why: "the link and its folder are THIS machine's; paired, the session is found \
                  among the hub's (list_sessions) and the prompt goes through the hub's \
                  send_prompt",
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
