//! Typed registry of the operator-facing settings stored in the `settings`
//! table (key → string). Every key the Settings dialog can edit is declared
//! here with its default and value shape, so the Tauri command that writes a
//! setting can refuse unknown keys and garbage values, and every backend
//! reader (reconcile tick, playbooks, GC, project discovery) resolves the
//! same default.
//!
//! Keys that other subsystems own and write themselves — the hub daemon's
//! `hub.*` and the control API's `mcp.*` — are registered READ-ONLY
//! (decision D-P7): they carry metadata and a place on a page, `describe`
//! shows their stored value, and [`set`] refuses them, naming where they are
//! changed ([`Spec::owned_by`]). Their owners keep reading them their own
//! way; nothing here changes what a value means to them.
//!
//! Never registered, so never described or shown: secrets and key material
//! (`mcp.token`, `hub.tls_key`, `hub.tls_cert`, `operator.token_sha`,
//! `hub.client_plaintext_token`), internal state that is not a setting
//! (`operator.session`, `operator.host`, `fleet.id`), and `ui.quick_replies`,
//! a list with its own tool (`never_registered_keys_stay_out`).

use crate::ipc_error::{codes, IpcError};
use crate::store::Store;
use std::collections::BTreeMap;

/// Value shape a setting accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `"true"` / `"false"`.
    Bool,
    /// A comma-separated subset of a fixed set of strings (`""` = none),
    /// stored in the set's order without duplicates.
    ChoiceSet(&'static [&'static str]),
    /// Integer seconds in `0..=MAX_SECS` (`0` usually means "disabled" /
    /// "never").
    Secs,
    /// Integer seconds in `min..=MAX_SECS`: a cadence with no "disabled"
    /// value (an opt-in toggle elsewhere turns the feature off).
    SecsMin(u64),
    /// Integer in `min..=max` (a count or size, not seconds).
    Int { min: u64, max: u64 },
    /// One of a fixed set of strings.
    Choice(&'static [&'static str]),
    /// JSON object `{ "<host alias>": "<projects root>" }`. Every alias must
    /// pass `validate::host_alias`, every path `validate_base_path`. `{}`
    /// means "no per-host overrides".
    PathMap,
    /// JSON array of positive integer ids (`[3, 7]`), stored sorted and
    /// without duplicates. `[]` means "none".
    IdSet,
    /// JSON object `{ "<model fragment>": {input, output, cache_write,
    /// cache_read} }` in USD per million tokens (`service::usage`). `{}`
    /// means "built-in prices only".
    PriceMap,
    /// One line of text, at most `max` bytes, no control characters.
    Text { max: usize },
}

/// Upper bound for `Kind::Secs` (ten years): keeps every `secs as i64`
/// arithmetic in the sweeper far from overflow.
pub const MAX_SECS: u64 = 10 * 365 * 24 * 3600;

/// Upper bound on one projects-root path.
pub const MAX_PATH_LEN: usize = 1024;

/// One registered setting: its value shape (what the write gate checks) and
/// everything a UI, the docs and an agent need to present it (`describe`).
/// The metadata is the ONE copy: the settings pages, the generated doc
/// tables (`settings_doc_gen`) and `get_settings { describe: true }` all read
/// it here. Build one with [`Spec::new`] and the `const` modifiers below.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub key: &'static str,
    pub default: &'static str,
    pub kind: Kind,
    /// Short sentence-case name, e.g. "Reconcile interval".
    pub label: &'static str,
    /// One or two plain sentences on what the setting does, for the page,
    /// the docs and an agent. Plain text: no markup, no internal task ids.
    pub help: &'static str,
    /// The unit the value is shown in. A `Secs` / `SecsMin` value is always
    /// STORED in seconds; `Hours` / `Minutes` / `Days` here only ask the UI
    /// to convert it.
    pub unit: Unit,
    /// What `0` means ("off", "never", "forever"), when it means something
    /// other than the number. `None` for a kind that cannot be 0 or where 0
    /// is just a count.
    pub zero: Option<&'static str>,
    pub tags: &'static [Tag],
    pub danger: Danger,
    pub restart: Restart,
    pub ai: AiPolicy,
    /// `Some(how)`: another subsystem owns and writes this key, and `how`
    /// says where a person changes it. [`set`] refuses it.
    pub owned_by: Option<&'static str>,
    /// Display labels for a `Choice` / `ChoiceSet`'s options, `(value,
    /// label)`, covering every option when set; empty shows the raw values.
    pub option_labels: &'static [(&'static str, &'static str)],
}

/// The unit a setting's value is shown in (see [`Spec::unit`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    None,
    Ms,
    Seconds,
    Minutes,
    Hours,
    Days,
    Percent,
    Kib,
    Mib,
    Tokens,
    Count,
}

impl Unit {
    /// The unit as a word for docs and range text ("" for `None`).
    pub fn word(self) -> &'static str {
        match self {
            Unit::None => "",
            Unit::Ms => "ms",
            Unit::Seconds => "seconds",
            Unit::Minutes => "minutes",
            Unit::Hours => "hours",
            Unit::Days => "days",
            Unit::Percent => "%",
            Unit::Kib => "KiB",
            Unit::Mib => "MiB",
            Unit::Tokens => "tokens",
            Unit::Count => "",
        }
    }
}

/// Where a setting belongs in a UI and what it implies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tag {
    /// Rarely changed: a page folds it into its "Advanced" section.
    Advanced,
    /// A feature that is off by default and still being evaluated.
    Experimental,
    /// Turning it on can send data off this machine.
    Network,
    /// Spends model calls or tokens.
    Ai,
}

/// How much a change needs confirming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "level", content = "message")]
pub enum Danger {
    None,
    /// Confirm with this sentence, which names the consequence.
    Confirm(&'static str),
}

/// When a changed value takes effect, if not on the next read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Restart {
    None,
    /// Read once at launch: applies after the app or hub restarts.
    App,
    /// Applies when the Claude Code hooks are next installed on a host.
    Hooks,
}

/// What an agent may do with a setting (design D-P4): propose a value for a
/// person to accept, fill it inside a flow a person started, or nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiPolicy {
    Suggest,
    Fill,
    Never,
}

impl Spec {
    /// A setting with no unit, tags, danger or restart, that an agent may
    /// only suggest.
    pub const fn new(
        key: &'static str,
        default: &'static str,
        kind: Kind,
        label: &'static str,
        help: &'static str,
    ) -> Self {
        Spec {
            key,
            default,
            kind,
            label,
            help,
            unit: Unit::None,
            zero: None,
            tags: &[],
            danger: Danger::None,
            restart: Restart::None,
            ai: AiPolicy::Suggest,
            owned_by: None,
            option_labels: &[],
        }
    }
    pub const fn unit(self, unit: Unit) -> Self {
        Spec { unit, ..self }
    }
    pub const fn zero(self, meaning: &'static str) -> Self {
        Spec {
            zero: Some(meaning),
            ..self
        }
    }
    pub const fn tags(self, tags: &'static [Tag]) -> Self {
        Spec { tags, ..self }
    }
    /// A change needs confirming with `message`; an agent may never make it.
    pub const fn danger(self, message: &'static str) -> Self {
        Spec {
            danger: Danger::Confirm(message),
            ai: AiPolicy::Never,
            ..self
        }
    }
    pub const fn restart(self, restart: Restart) -> Self {
        Spec { restart, ..self }
    }
    pub const fn ai(self, ai: AiPolicy) -> Self {
        Spec { ai, ..self }
    }
    pub const fn labels(self, option_labels: &'static [(&'static str, &'static str)]) -> Self {
        Spec {
            option_labels,
            ..self
        }
    }
    /// Read-only here: `how` names where the key is changed. An agent never
    /// writes it either.
    pub const fn owned_by(self, how: &'static str) -> Self {
        Spec {
            owned_by: Some(how),
            ai: AiPolicy::Never,
            ..self
        }
    }
}

// ── keys ──
pub const RECONCILE_INTERVAL_SECS: &str = "reconcile.interval_secs";
/// A `working` row with no hook, no turn, no transcript growth and no pane
/// output for this long is demoted to `idle` by the tick (lifecycle F2).
/// `0` turns the rule off.
pub const RECONCILE_STALE_WORKING_SECS: &str = "reconcile.stale_working_secs";
/// How long a resumable mass-loss row (`lost_reason` `host_reboot` /
/// `tmux_server_gone`, with a `claude_session_id`, any kind but `external`)
/// is kept before Phase 2
/// hard-deletes it, counted from `lost_at` (when it was lost) — not extra
/// time added on top of the usual one-cycle grace. Default 14 days. Wired
/// through to `HostReconcile::lost_ttl_cutoff` / `ghost_and_clean_bg_sessions`
/// by Task 6.
pub const SESSIONS_LOST_TTL_SECS: &str = "sessions.lost_ttl_secs";
/// How many resumable lost sessions a batch restore resumes in parallel.
/// Read by Task 3's restore path via `get_setting` + `settings::resolve`.
pub const RESTORE_BATCH_SIZE: &str = "restore.batch_size";
/// Pause (ms) between starting each resumed session in a batch restore.
/// Read by Task 3's restore path via `get_setting` + `settings::resolve`.
pub const RESTORE_STAGGER_MS: &str = "restore.stagger_ms";
pub const PLAYBOOK_PRESS_ENTER: &str = "playbooks.press_enter";
pub const PLAYBOOK_OOM_RECREATE: &str = "playbooks.oom_recreate";
/// Recreates the `oom` playbook may run on one session per 24 h
/// (`service::playbooks::OOM_ATTEMPT_WINDOW_SECS`). `0` refuses every
/// recreate while keeping the refusals on the timeline.
pub const PLAYBOOK_OOM_MAX_ATTEMPTS: &str = "playbooks.oom_max_attempts";
pub const GC_ENABLED: &str = "gc.enabled";
pub const GC_BG_IDLE_SECS: &str = "gc.bg_idle_secs";
pub const GC_SHELL_IDLE_SECS: &str = "gc.shell_idle_secs";
pub const GC_WORK_IDLE_SECS: &str = "gc.work_idle_secs";
pub const GC_SWEEP_INTERVAL_SECS: &str = "gc.sweep_interval_secs";
/// How long a lost `external` row (a Claude fleet only observes, never
/// resumable) is kept before Phase 2 deletes it — long enough for the
/// desktop that owns it to restart, no longer. `0` reaps it on the next pass.
pub const GC_EXTERNAL_LOST_TTL_SECS: &str = "gc.external_lost_ttl_secs";
/// Opt-in reconcile-tick repair: re-adds deleted worktrees without anyone
/// opening them. Dropping a stale entry first (tick or click) always needs
/// the vanished-directory guard, including the parent fingerprint match.
pub const REPAIR_AUTO_ON_TICK: &str = "repair.auto_on_tick";
pub const REPAIR_TICK_INTERVAL_SECS: &str = "repair.tick_interval_secs";
/// Per-host projects root, one JSON map (host alias → path). A host with no
/// entry falls back to `$CLAUDE_FLEET_PROJECTS_BASE` (local only), then to the
/// layout default. See `service::projects::project_base_for`.
pub const PROJECTS_BASE_PATH: &str = "projects.base_path";
/// `github` (`<root>/<owner>/<repo>`) or `flat` (`<root>/<repo>`): where a
/// repository sits under the projects root. It does NOT choose the worktree
/// subdir inside a repo (`.worktrees/` vs `.claude/worktrees/`).
pub const PROJECTS_LAYOUT: &str = "projects.layout";

/// Derived, read-only entry `read_all` adds next to the registered keys: the
/// JSON map of host alias → resolved projects root, so the Settings dialog
/// can preview env/default fallbacks it cannot compute itself. Not in
/// `SPECS`, so `set` refuses it.
pub const PROJECTS_RESOLVED_BASE: &str = "projects.resolved_base";

/// Derived, read-only: `$CLAUDE_FLEET_PROJECTS_BASE` as the app sees it
/// (trimmed), or `""` when unset. Lets the Settings dialog preview the local
/// fallback for a layout that is not saved yet.
pub const PROJECTS_LOCAL_ENV_BASE: &str = "projects.local_env_base";

const LAYOUTS: &[&str] = &["github", "flat"];

/// Open tasks older than this (from start, else creation) are failed by the
/// liveness sweep. `0` disables the TTL.
pub const TASKS_MAX_AGE_SECS: &str = "tasks.max_age_secs";

/// Largest transcript (MiB) `move_session` copies (`E_MOVE_TOO_LARGE`
/// above it).
pub const MOVE_MAX_TRANSCRIPT_MB: &str = crate::service::move_session::SETTING_MAX_TRANSCRIPT_MB;
/// Upper bound for [`MOVE_MAX_TRANSCRIPT_MB`]: the copy is held in memory.
pub const MOVE_MAX_TRANSCRIPT_MB_MAX: u64 = 4096;

/// Upper bound for [`MOVE_MAX_BUNDLE_MB`]: the bundle is relayed through the
/// orchestrator in 8 MiB chunks via a private temp file, so this bounds relay
/// time and temp-disk use on the orchestrator and both hosts, not memory.
pub const MOVE_MAX_BUNDLE_MB_MAX: u64 = 4096;
/// Upper bound for [`MOVE_IGNORED_ENTRY_KB`]: no practical I/O constraint.
pub const MOVE_IGNORED_ENTRY_KB_MAX: u64 = 1_048_576;
/// Upper bound for [`MOVE_IGNORED_TOTAL_MB`]: one transfer's payload.
pub const MOVE_IGNORED_TOTAL_MB_MAX: u64 = 1024;
/// Upper bound for [`MOVE_MAX_SESSION_STATE_MB`]: one transfer's payload.
pub const MOVE_MAX_SESSION_STATE_MB_MAX: u64 = 4096;
/// Upper bound for [`MOVE_WAIT_MAX_MINS`]: a week.
pub const MOVE_WAIT_MAX_MINS_MAX: u64 = 10_080;

/// Largest git bundle (MiB) `move_session` relays (`E_MOVE_TOO_LARGE` above it).
pub const MOVE_MAX_BUNDLE_MB: &str = crate::service::move_session::carry::SETTING_MAX_BUNDLE_MB;
/// Largest single git-ignored entry (KiB) `move_session` carries; bigger ones
/// are reported as left behind.
pub const MOVE_IGNORED_ENTRY_KB: &str =
    crate::service::move_session::carry::SETTING_IGNORED_ENTRY_KB;
/// Total git-ignored payload (MiB) `move_session` carries.
pub const MOVE_IGNORED_TOTAL_MB: &str =
    crate::service::move_session::carry::SETTING_IGNORED_TOTAL_MB;
/// Largest per-session Claude directory (MiB) `move_session` carries; the
/// biggest files stay behind above it.
pub const MOVE_MAX_SESSION_STATE_MB: &str =
    crate::service::move_session::claude_state::SETTING_MAX_SESSION_STATE_MB;

/// Longest a "transfer when it finishes" wait runs before it is given up on
/// (minutes), counted from when the wait began.
pub const MOVE_WAIT_MAX_MINS: &str = "move.wait_max_mins";

/// Collect per-session token usage from Claude transcripts (Wave 5 G1).
pub const USAGE_ENABLED: &str = "usage.enabled";
/// Seconds between usage passes (one batched script per host). `0` stops
/// collection.
pub const USAGE_INTERVAL_SECS: &str = "usage.interval_secs";
/// Per-model price overrides for the estimated cost; see
/// `service::usage::BUILTIN_PRICES`.
pub const USAGE_PRICES_JSON: &str = "usage.prices_json";

/// Newest `error_reports` rows kept; pruned on every insert.
pub const REPORTS_MAX_ROWS: &str = "reports.max_rows";
/// Rows older than this are swept on the tick; `0` disables the age sweep.
pub const REPORTS_MAX_AGE_SECS: &str = "reports.max_age_secs";

/// Percent of the context window at or past which a session counts as
/// `context_red` in `fleet_health`, reads `context_full` in
/// `needs_attention`, and draws red on the desktop. One number for all
/// three (ux F-09: the hub said 85 while the desktop said 70/90).
pub const HEALTH_CONTEXT_RED_PCT: &str = "health.context_red_pct";

/// Retention (work graph M12.3, `store::work_retention`): days a work
/// journal row is kept once nothing live points at it. `0` keeps forever.
pub const WORK_RETENTION_JOURNAL_DAYS: &str = "work.retention.journal_days";
/// Retention: days a DONE tracker item no link names is kept. `0` forever.
pub const WORK_RETENTION_TRACKER_ITEMS_DAYS: &str = "work.retention.tracker_items_days";
/// Retention: days a work-graph timeline event (handover, nudge, tidy) is
/// kept; the newest of each kind per session always stays. `0` forever.
pub const WORK_RETENTION_TIMELINE_WORK_EVENTS_DAYS: &str =
    "work.retention.timeline_work_events_days";
/// M2's journal window, superseded by [`WORK_RETENTION_JOURNAL_DAYS`] and no
/// longer writable. While the new key is unset, a stored `0` still keeps
/// forever and a longer window still stands (`service::work::retention`).
pub const LEGACY_WORK_JOURNAL_DAYS: &str = "work.journal_days";
/// How far back (days) the sidebar looks for work that has only ended
/// sessions, so reopened work has a group to show in.
pub const WORK_RECENT_DAYS: &str = "work.recent_days";
/// Seconds between tracker sync passes (work graph M3); `0` turns the sync
/// off. Values under a minute are raised to one.
pub const WORK_SYNC_INTERVAL_SECS: &str = "work.sync_interval_secs";

/// Project ids whose branch keys are trusted (work graph M4, rule R3): a
/// sole branch key there links automatically, with Undo; elsewhere it is a
/// pre-selected suggestion. Set by the popover's checkbox, or automatically
/// after three branch suggestions confirmed in a project.
pub const WORK_TRUSTED_BRANCH_PROJECTS: &str = "work.trusted_branch_projects";
/// Keep a ±40-character, redacted snippet of the prompt around a detected
/// key as evidence (work graph M4). Off keeps only the matched text.
pub const WORK_EVIDENCE_SNIPPETS: &str = "work.evidence_snippets";
/// SessionStart hands Claude the linked ticket's context (work graph M4.5,
/// decision D5). Off until measured: turning it on makes the SessionStart
/// hook synchronous, which can add up to ~2 s to a start when the hub is
/// down. Takes effect when the hooks are next installed.
pub const WORK_SESSION_START_CONTEXT: &str = "work.session_start_context";
/// The classification nudge (work graph M4.6): after three turns with no
/// link, and with at most five candidates in scope, one prompt per
/// conversation carries a short note asking Claude to name its work
/// (`work_link { source: agent_inferred }` — only ever a suggestion). Off by
/// default: it spends context on a guess. Read on every prompt.
pub const WORK_CLASSIFY_NUDGE: &str = "work.classify_nudge";
/// The model a dead session's on-demand summary runs on (work graph M13.4c,
/// decisions D10 / D27), on the session's own host and account. A choice of
/// Claude Code's model aliases, never free text: it ends up in a command.
pub const WORK_SUMMARY_MODEL: &str = "work.summary_model";
/// The aliases [`WORK_SUMMARY_MODEL`] accepts.
pub const SUMMARY_MODELS: &[&str] = &["haiku", "sonnet", "opus"];

/// Tidy-up (work graph M7): a session whose linked item has been done at
/// least this many days (and that is idle, below) is suggested for tidying.
pub const WORK_TIDY_DONE_DAYS: &str = "work.tidy_done_days";
/// Tidy-up: how long a session must have been idle before any reason
/// suggests it.
pub const WORK_TIDY_IDLE_HOURS: &str = "work.tidy_idle_hours";
/// Tidy-up (work graph M11.3): a session with no work linked is suggested
/// (`idle_unlinked`) once it has been idle, and unprompted, this many days.
/// Never acted on by auto-tidy (D19).
pub const WORK_TIDY_IDLE_UNLINKED_DAYS: &str = "work.tidy_idle_unlinked_days";
/// Auto-tidy: the GC sweep acts on the tidy candidates of the allowed
/// reasons (below) by itself — safe kill or archive only, never a plain
/// kill. Off by default: tidy-up only suggests.
pub const WORK_AUTO_TIDY: &str = "work.auto_tidy";
/// The reasons auto-tidy may act on.
pub const WORK_AUTO_TIDY_REASONS: &str = "work.auto_tidy_reasons";
/// What [`WORK_AUTO_TIDY_REASONS`] may name: the reasons whose action is a
/// safe kill. A duplicate worktree is only ever plain-killed and a ghost is
/// never killed, so neither is automatic.
pub const AUTO_TIDY_REASONS: &[&str] = &["done_idle", "pr_merged_idle", "not_planned"];

// ── decisions (Jev evaluation, D35-D37; `service::decide`) ──
/// The kill switch: with it off no decision-model call is ever made. Off by
/// default; the Settings dialog's "Decisions (Jev)" toggle.
pub const DECIDE_JEV_ENABLED: &str = "decide.jev.enabled";
/// `status_map`'s mode (Asana section → status category proposals).
pub const DECIDE_JEV_STATUS_MAP: &str = "decide.jev.status_map";
/// `work_link`'s mode (choosing a work item for an unlinked session).
pub const DECIDE_JEV_WORK_LINK: &str = "decide.jev.work_link";
/// What a feature's mode may be. `auto` is not offered: no feature has
/// passed acceptance (D36).
pub const DECIDE_MODES: &[&str] = &["off", "shadow", "assist"];
/// Sessions and items with no org may be sent too (D31). Off by default.
pub const DECIDE_JEV_UNASSIGNED: &str = "decide.jev.unassigned";
/// One call's whole budget, in milliseconds.
pub const DECIDE_JEV_TIMEOUT_MS: &str = "decide.jev.timeout_ms";
/// Consecutive failed calls that open the circuit breaker.
pub const DECIDE_JEV_BREAKER_FAILURES: &str = "decide.jev.breaker_failures";
/// How long an open breaker refuses calls, in seconds.
pub const DECIDE_JEV_BREAKER_OPEN_SECS: &str = "decide.jev.breaker_open_secs";
/// Input tokens the decision model may be sent per UTC day (`0` = none).
pub const DECIDE_JEV_DAILY_TOKEN_BUDGET: &str = "decide.jev.daily_token_budget";
/// The model version a request names. Pinned by default: TypeSafe advises
/// pinning when thresholds are tuned against a version.
pub const DECIDE_JEV_MODEL: &str = "decide.jev.model";
/// What [`DECIDE_JEV_MODEL`] may be (never free text: it goes on the wire).
pub const DECIDE_JEV_MODELS: &[&str] = &["jev-1.13.0", "jev-latest"];
/// Days a `decision_runs` row is kept (`0` = forever).
pub const DECIDE_RETENTION_DAYS: &str = "decide.retention_days";

/// What `hub.tls` holds (`fleet-hub`'s `TlsMode`).
pub const HUB_TLS_MODES: &[&str] = &["off", "cert"];

/// Every editable setting. Order is the display order.
pub const SPECS: &[Spec] = &[
    Spec::new(
        RECONCILE_INTERVAL_SECS,
        "20",
        Kind::Secs,
        "Reconcile interval",
        "Seconds between background reconcile passes, which refresh session state on every host.",
    )
    .unit(Unit::Seconds)
    .zero("off")
    .restart(Restart::App)
    .tags(&[Tag::Advanced]),
    Spec::new(
        RECONCILE_STALE_WORKING_SECS,
        "1800",
        Kind::Secs,
        "Stale working timeout",
        "How long a working session may go without a hook, a turn, transcript growth or pane output before it reads idle.",
    )
    .unit(Unit::Seconds)
    .zero("never")
    .tags(&[Tag::Advanced]),
    Spec::new(
        SESSIONS_LOST_TTL_SECS,
        "1209600",
        Kind::Secs,
        "Keep lost sessions",
        "How long a resumable session lost to a host reboot or the tmux server exiting is kept before it is deleted, counted from when it was lost.",
    )
    .unit(Unit::Hours)
    .zero("removed on the next pass"),
    Spec::new(
        RESTORE_BATCH_SIZE,
        "4",
        Kind::Int { min: 1, max: 16 },
        "Concurrent restores",
        "Sessions resumed in parallel by Restore lost sessions.",
    )
    .unit(Unit::Count),
    Spec::new(
        RESTORE_STAGGER_MS,
        "3000",
        Kind::Int { min: 0, max: 60000 },
        "Delay between restores",
        "Pause between starting each resumed session in a batch restore.",
    )
    .unit(Unit::Ms)
    .tags(&[Tag::Advanced]),
    Spec::new(
        PLAYBOOK_PRESS_ENTER,
        "false",
        Kind::Bool,
        "Press Enter when stuck",
        "Press Enter for sessions stuck on a \"Press Enter\" prompt. Auth menus, trust prompts and reconnects are always notify-only.",
    ),
    Spec::new(
        PLAYBOOK_OOM_RECREATE,
        "false",
        Kind::Bool,
        "Recreate out-of-memory sessions",
        "Recreate a session that ran out of memory, within the budget below.",
    ),
    Spec::new(
        PLAYBOOK_OOM_MAX_ATTEMPTS,
        "2",
        Kind::Int { min: 0, max: 20 },
        "Out-of-memory budget",
        "Recreates one session may get per 24 hours. A session that is working, or finished a turn after the flag, is never recreated.",
    )
    .unit(Unit::Count)
    .zero("never"),
    Spec::new(
        GC_ENABLED,
        "false",
        Kind::Bool,
        "Garbage-collect idle sessions",
        "Stop or remove sessions that have been idle longer than the limits below.",
    )
    .danger("Idle sessions past the limits below will be stopped or removed without asking."),
    Spec::new(
        GC_BG_IDLE_SECS,
        "86400",
        Kind::Secs,
        "Background agent idle limit",
        "How long a background agent may sit idle before it is stopped.",
    )
    .unit(Unit::Hours)
    .zero("never"),
    Spec::new(
        GC_SHELL_IDLE_SECS,
        "604800",
        Kind::Secs,
        "Shell idle limit",
        "How long a shell session may sit inactive before it is killed.",
    )
    .unit(Unit::Hours)
    .zero("never"),
    Spec::new(
        GC_WORK_IDLE_SECS,
        "0",
        Kind::Secs,
        "Work session idle limit",
        "How long a work session may sit idle before it is removed. A dirty worktree goes through safe remove.",
    )
    .unit(Unit::Hours)
    .zero("never"),
    Spec::new(
        GC_SWEEP_INTERVAL_SECS,
        "300",
        Kind::Secs,
        "GC sweep interval",
        "Seconds between garbage-collection sweeps.",
    )
    .unit(Unit::Seconds)
    .zero("off")
    .tags(&[Tag::Advanced]),
    Spec::new(
        GC_EXTERNAL_LOST_TTL_SECS,
        "3600",
        Kind::Secs,
        "Keep lost outside sessions",
        "How long a lost session from outside fleet is kept before it is removed. It can never be resumed; this only rides out a restart.",
    )
    .unit(Unit::Hours)
    .zero("the next pass")
    .tags(&[Tag::Advanced]),
    Spec::new(
        PROJECTS_BASE_PATH,
        "{}",
        Kind::PathMap,
        "Projects roots",
        "Per-host folder that holds your repositories. A host with no entry uses $CLAUDE_FLEET_PROJECTS_BASE (local only), then the layout default.",
    ),
    Spec::new(
        PROJECTS_LAYOUT,
        "github",
        Kind::Choice(LAYOUTS),
        "Projects layout",
        "Where a repository sits under the projects root: github puts it at root/owner/repo, flat at root/repo.",
    )
    .labels(&[("github", "github: root/owner/repo"), ("flat", "flat: root/repo")]),
    Spec::new(
        TASKS_MAX_AGE_SECS,
        "86400",
        Kind::Secs,
        "Task timeout",
        "How long an open task (counted from its start, else its creation) may run before the liveness sweep fails it.",
    )
    .unit(Unit::Hours)
    .zero("never"),
    Spec::new(
        REPAIR_AUTO_ON_TICK,
        "false",
        Kind::Bool,
        "Re-create vanished worktrees",
        "Re-add deleted worktree directories without anyone opening them. A stale entry is dropped only when its parent folder is the one seen while it was healthy, so an unmounted volume is never touched.",
    ),
    Spec::new(
        REPAIR_TICK_INTERVAL_SECS,
        "600",
        Kind::SecsMin(REPAIR_TICK_MIN_SECS),
        "Workspace check interval",
        "Seconds between automatic workspace checks, each repairing at most five worktrees.",
    )
    .unit(Unit::Seconds)
    .tags(&[Tag::Advanced]),
    Spec::new(
        MOVE_MAX_TRANSCRIPT_MB,
        "200",
        Kind::Int {
            min: 1,
            max: MOVE_MAX_TRANSCRIPT_MB_MAX,
        },
        "Move: transcript cap",
        "Largest transcript Move to host copies; a bigger one is refused.",
    )
    .unit(Unit::Mib)
    .tags(&[Tag::Advanced]),
    Spec::new(
        MOVE_MAX_BUNDLE_MB,
        "500",
        Kind::Int {
            min: 1,
            max: MOVE_MAX_BUNDLE_MB_MAX,
        },
        "Move: git bundle cap",
        "Largest git bundle of unpushed work Move to host relays; a bigger one is refused.",
    )
    .unit(Unit::Mib)
    .tags(&[Tag::Advanced]),
    Spec::new(
        MOVE_IGNORED_ENTRY_KB,
        "1024",
        Kind::Int {
            min: 1,
            max: MOVE_IGNORED_ENTRY_KB_MAX,
        },
        "Move: ignored entry cap",
        "Largest single git-ignored file or directory Move to host carries; bigger ones are left behind.",
    )
    .unit(Unit::Kib)
    .tags(&[Tag::Advanced]),
    Spec::new(
        MOVE_IGNORED_TOTAL_MB,
        "20",
        Kind::Int {
            min: 1,
            max: MOVE_IGNORED_TOTAL_MB_MAX,
        },
        "Move: ignored total cap",
        "Total git-ignored payload Move to host carries.",
    )
    .unit(Unit::Mib)
    .tags(&[Tag::Advanced]),
    Spec::new(
        MOVE_MAX_SESSION_STATE_MB,
        "200",
        Kind::Int {
            min: 1,
            max: MOVE_MAX_SESSION_STATE_MB_MAX,
        },
        "Move: session state cap",
        "Largest per-session Claude directory (subagent transcripts, tool results) Move to host carries; above it the biggest files stay behind.",
    )
    .unit(Unit::Mib)
    .tags(&[Tag::Advanced]),
    Spec::new(
        MOVE_WAIT_MAX_MINS,
        "240",
        Kind::Int {
            min: 1,
            max: MOVE_WAIT_MAX_MINS_MAX,
        },
        "Move: wait timeout",
        "How long \"Transfer when it finishes\" waits for the session to go idle before giving up.",
    )
    .unit(Unit::Minutes),
    Spec::new(
        USAGE_ENABLED,
        "true",
        Kind::Bool,
        "Collect token usage",
        "Sum each session's token usage from its Claude transcript and show an estimated cost.",
    ),
    Spec::new(
        USAGE_INTERVAL_SECS,
        "300",
        Kind::Secs,
        "Usage interval",
        "Seconds between usage passes, one batched read per host.",
    )
    .unit(Unit::Seconds)
    .zero("off")
    .tags(&[Tag::Advanced]),
    Spec::new(
        USAGE_PRICES_JSON,
        "{}",
        Kind::PriceMap,
        "Price overrides",
        "Per-model prices for the estimated cost, in USD per million tokens (input, output, cache_write, cache_read). {} uses the built-in prices only.",
    )
    .tags(&[Tag::Advanced]),
    Spec::new(
        REPORTS_MAX_ROWS,
        "5000",
        Kind::Int {
            min: 100,
            max: 100_000,
        },
        "Error reports kept",
        "Newest error and warning reports kept; older ones are pruned on every insert.",
    )
    .unit(Unit::Count)
    .tags(&[Tag::Advanced]),
    Spec::new(
        REPORTS_MAX_AGE_SECS,
        "604800",
        Kind::Secs,
        "Error report age",
        "How long an error or warning report is kept before the age sweep deletes it.",
    )
    .unit(Unit::Hours)
    .zero("never")
    .tags(&[Tag::Advanced]),
    Spec::new(
        HEALTH_CONTEXT_RED_PCT,
        "85",
        Kind::Int { min: 1, max: 100 },
        "Context red threshold",
        "Percent of the context window at which a session needs you. The chip turns red here and amber 15 points below.",
    )
    .unit(Unit::Percent),
    Spec::new(
        WORK_RETENTION_JOURNAL_DAYS,
        "365",
        Kind::Int { min: 0, max: 3650 },
        "Retention: work journal",
        "Days a work journal row is kept once its conversation ended and its work is done or unlinked.",
    )
    .unit(Unit::Days)
    .zero("forever"),
    Spec::new(
        WORK_RETENTION_TRACKER_ITEMS_DAYS,
        "180",
        Kind::Int { min: 0, max: 3650 },
        "Retention: done tickets",
        "Days a done ticket that no session links to is kept in the cache.",
    )
    .unit(Unit::Days)
    .zero("forever"),
    Spec::new(
        WORK_RETENTION_TIMELINE_WORK_EVENTS_DAYS,
        "180",
        Kind::Int { min: 0, max: 3650 },
        "Retention: work timeline",
        "Days handover, nudge, tidy and withdrawn-suggestion timeline events are kept; the newest of each kind per session always stays.",
    )
    .unit(Unit::Days)
    .zero("forever"),
    Spec::new(
        WORK_RECENT_DAYS,
        "14",
        Kind::Int { min: 1, max: 365 },
        "Recent work",
        "How long ended work with no live session keeps a sidebar group.",
    )
    .unit(Unit::Days),
    Spec::new(
        WORK_SYNC_INTERVAL_SECS,
        "300",
        Kind::Secs,
        "Tracker sync interval",
        "Seconds between tracker sync passes. Under a minute is raised to one.",
    )
    .unit(Unit::Seconds)
    .zero("off")
    .restart(Restart::App),
    Spec::new(
        WORK_TRUSTED_BRANCH_PROJECTS,
        "[]",
        Kind::IdSet,
        "Trusted branch projects",
        "Projects where a sole ticket key in the branch name links automatically; elsewhere it is a suggestion. Set from the work popover.",
    ),
    Spec::new(
        WORK_EVIDENCE_SNIPPETS,
        "true",
        Kind::Bool,
        "Keep evidence snippets",
        "Keep a short, redacted prompt snippet around a detected ticket key as evidence. Off keeps only the matched text.",
    ),
    Spec::new(
        WORK_SESSION_START_CONTEXT,
        "false",
        Kind::Bool,
        "Ticket context at session start",
        "Give Claude the linked ticket at session start. Makes the start hook synchronous, which can add up to 2 s when the hub is down.",
    )
    .restart(Restart::Hooks)
    .tags(&[Tag::Experimental]),
    Spec::new(
        WORK_CLASSIFY_NUDGE,
        "false",
        Kind::Bool,
        "Classification nudge",
        "After three prompts with no ticket, ask Claude once which of your few open tickets it is on. Its answer is only ever a suggestion.",
    )
    .tags(&[Tag::Experimental, Tag::Ai]),
    Spec::new(
        WORK_SUMMARY_MODEL,
        "haiku",
        Kind::Choice(SUMMARY_MODELS),
        "Summary model",
        "The model Summarise runs on for a past session, on that session's own host and account.",
    )
    .tags(&[Tag::Ai]),
    Spec::new(
        WORK_TIDY_DONE_DAYS,
        "2",
        Kind::Int { min: 1, max: 365 },
        "Tidy: done for",
        "Days a linked ticket must be done before Tidy up suggests its session.",
    )
    .unit(Unit::Days),
    Spec::new(
        WORK_TIDY_IDLE_HOURS,
        "4",
        Kind::Int { min: 1, max: 720 },
        "Tidy: idle for",
        "Hours a session must be idle before any tidy reason suggests it.",
    )
    .unit(Unit::Hours),
    Spec::new(
        WORK_TIDY_IDLE_UNLINKED_DAYS,
        "7",
        Kind::Int { min: 1, max: 90 },
        "Tidy: unlinked for",
        "Days a session with no work linked must sit idle and unprompted before Tidy up suggests it. Only ever suggested, never auto-tidied.",
    )
    .unit(Unit::Days),
    Spec::new(
        WORK_AUTO_TIDY,
        "false",
        Kind::Bool,
        "Auto-tidy",
        "Let the GC sweep act on the allowed tidy reasons by itself, by safe kill or archive only. Off, Tidy up only suggests. An organisation can override it.",
    )
    .danger("The sweep will safe-kill finished sessions without asking."),
    Spec::new(
        WORK_AUTO_TIDY_REASONS,
        "done_idle,pr_merged_idle",
        Kind::ChoiceSet(AUTO_TIDY_REASONS),
        "Auto-tidy reasons",
        "The tidy reasons auto-tidy may act on.",
    )
    .labels(&[("done_idle", "Done and idle"), ("pr_merged_idle", "PR merged, idle"), ("not_planned", "Won't do / duplicate")]),
    Spec::new(
        DECIDE_JEV_ENABLED,
        "false",
        Kind::Bool,
        "Decisions (Jev)",
        "The kill switch for TypeSafe's decision model. Off, nothing is ever sent. On, data goes only for organisations that opted in, redacted.",
    )
    .tags(&[Tag::Experimental, Tag::Network, Tag::Ai])
    .danger("Redacted session and ticket data will be sent to TypeSafe for organisations that opted in."),
    Spec::new(
        DECIDE_JEV_STATUS_MAP,
        "off",
        Kind::Choice(DECIDE_MODES),
        "Jev: status map",
        "Proposing a status category for an Asana section. Shadow only records; assist suggests.",
    )
    .tags(&[Tag::Experimental, Tag::Ai])
    .labels(&[("off", "Off"), ("shadow", "Shadow: record only"), ("assist", "Assist: suggest")]),
    Spec::new(
        DECIDE_JEV_WORK_LINK,
        "off",
        Kind::Choice(DECIDE_MODES),
        "Jev: work link",
        "Choosing a ticket for a session no rule could link. Shadow only records; assist suggests.",
    )
    .tags(&[Tag::Experimental, Tag::Ai])
    .labels(&[("off", "Off"), ("shadow", "Shadow: record only"), ("assist", "Assist: suggest")]),
    Spec::new(
        DECIDE_JEV_UNASSIGNED,
        "false",
        Kind::Bool,
        "Jev: send unassigned",
        "Also send sessions and tickets that belong to no organisation.",
    )
    .tags(&[Tag::Experimental, Tag::Network])
    .danger("Sessions and tickets outside every organisation will be sent to TypeSafe too."),
    Spec::new(
        DECIDE_JEV_TIMEOUT_MS,
        "1500",
        Kind::Int {
            min: 100,
            max: 30_000,
        },
        "Jev: timeout",
        "How long one call may take. A call is never retried.",
    )
    .unit(Unit::Ms)
    .tags(&[Tag::Advanced]),
    Spec::new(
        DECIDE_JEV_BREAKER_FAILURES,
        "5",
        Kind::Int { min: 1, max: 100 },
        "Jev: breaker failures",
        "Failed calls in a row that open the circuit breaker.",
    )
    .unit(Unit::Count)
    .tags(&[Tag::Advanced]),
    Spec::new(
        DECIDE_JEV_BREAKER_OPEN_SECS,
        "300",
        Kind::Int {
            min: 10,
            max: 86_400,
        },
        "Jev: breaker pause",
        "How long an open breaker refuses calls.",
    )
    .unit(Unit::Seconds)
    .tags(&[Tag::Advanced]),
    Spec::new(
        DECIDE_JEV_DAILY_TOKEN_BUDGET,
        "2000000",
        Kind::Int {
            min: 0,
            max: 1_000_000_000,
        },
        "Jev: daily token budget",
        "Input tokens the decision model may be sent per UTC day. At $0.042 per million, the default is under $0.09 a day.",
    )
    .unit(Unit::Tokens)
    .zero("none"),
    Spec::new(
        DECIDE_JEV_MODEL,
        "jev-1.13.0",
        Kind::Choice(DECIDE_JEV_MODELS),
        "Jev: model",
        "The model version a request names. jev-1.13.0 is pinned; jev-latest follows TypeSafe.",
    )
    .tags(&[Tag::Advanced]),
    Spec::new(
        DECIDE_RETENTION_DAYS,
        "90",
        Kind::Int { min: 0, max: 3650 },
        "Jev: keep runs",
        "Days a decision record (ids and numbers, never text) is kept.",
    )
    .unit(Unit::Days)
    .zero("forever"),
    // ── read-only: owned and written elsewhere (D-P7) ──
    Spec::new(
        crate::service::hub::SETTING_BIND,
        "127.0.0.1",
        Kind::Text { max: 255 },
        "Hub bind address",
        "The address the hub daemon listens on. Loopback unless the daemon was started with a routable bind.",
    )
    .owned_by("fleet-hub serve --bind")
    .tags(&[Tag::Network]),
    Spec::new(
        crate::service::hub::SETTING_PUBLIC_URL,
        "",
        Kind::Text { max: 2048 },
        "Hub public URL",
        "The URL hosts and paired clients reach the hub at. Empty means loopback with a reverse tunnel per host.",
    )
    .owned_by("fleet-hub serve --public-url")
    .tags(&[Tag::Network]),
    Spec::new(
        crate::service::hub::SETTING_ALLOWED_HOSTS,
        "",
        Kind::Text { max: 4096 },
        "Hub allowed hosts",
        "Extra Host header values the hub accepts, comma-separated, besides the ones its bind and public URL imply.",
    )
    .owned_by("fleet-hub serve --allowed-host")
    .tags(&[Tag::Network, Tag::Advanced]),
    Spec::new(
        crate::service::hub::SETTING_LOCAL_HOST,
        "true",
        Kind::Bool,
        "Hub runs local sessions",
        "Whether the hub's own machine is a fleet host. On for the desktop; the daemon turns it off by default.",
    )
    .owned_by("fleet-hub serve --local-host"),
    Spec::new(
        crate::service::hub::SETTING_ALLOW_PLAINTEXT,
        "false",
        Kind::Bool,
        "Hub serves plaintext",
        "Whether the hub daemon may serve a routable bind without TLS.",
    )
    .owned_by("fleet-hub serve --allow-plaintext")
    .tags(&[Tag::Network]),
    Spec::new(
        crate::service::hub::SETTING_TLS,
        "off",
        Kind::Choice(HUB_TLS_MODES),
        "Hub TLS",
        "How the hub daemon terminates TLS: off, behind a proxy, or cert, with its own certificate.",
    )
    .owned_by("fleet-hub serve --tls")
    .tags(&[Tag::Network])
    .labels(&[("off", "Off (a proxy in front)"), ("cert", "Own certificate")]),
    Spec::new(
        crate::mcp::SETTING_ENABLED,
        "false",
        Kind::Bool,
        "Control API",
        "Whether the embedded control API (MCP) runs, so an AI assistant can drive the fleet.",
    )
    .owned_by("Settings → Control API")
    .tags(&[Tag::Ai]),
    Spec::new(
        crate::mcp::SETTING_PORT,
        "4180",
        Kind::Int {
            min: 1,
            max: 65_535,
        },
        "Control API port",
        "The localhost port the control API listens on.",
    )
    .unit(Unit::Count)
    .owned_by("Settings → Control API"),
    Spec::new(
        crate::mcp::guard::SETTING_CONFIRM_DESTRUCTIVE,
        "false",
        Kind::Bool,
        "Confirm destructive calls",
        "Every destructive control API call waits for a confirmation on the desktop.",
    )
    .owned_by("Settings → Control API"),
    Spec::new(
        crate::mcp::guard::SETTING_BROADCAST_INTERVAL,
        "30",
        Kind::Secs,
        "Broadcast interval",
        "Shortest time between two broadcast prompts from the same caller.",
    )
    .unit(Unit::Seconds)
    .owned_by("the settings table only")
    .tags(&[Tag::Advanced]),
];

/// Parse + validate a `Kind::ChoiceSet` value into the chosen options, in
/// the set's order, without duplicates.
pub fn parse_choice_set(
    key: &str,
    options: &'static [&'static str],
    raw: &str,
) -> Result<Vec<&'static str>, IpcError> {
    let mut chosen = std::collections::BTreeSet::new();
    for part in raw.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let idx = options.iter().position(|o| *o == part).ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID,
                format!(
                    "{key} takes a comma-separated subset of: {}",
                    options.join(", ")
                ),
            )
        })?;
        chosen.insert(idx);
    }
    Ok(chosen.into_iter().map(|i| options[i]).collect())
}

/// Most ids one `Kind::IdSet` holds.
pub const ID_SET_MAX: usize = 1000;

/// Parse + validate a `Kind::IdSet` value: a JSON array of positive
/// integers, at most [`ID_SET_MAX`].
pub fn parse_id_set(raw: &str) -> Result<std::collections::BTreeSet<i64>, IpcError> {
    let ids: Vec<i64> = serde_json::from_str(raw.trim()).map_err(|_| {
        IpcError::new(
            codes::E_INVALID,
            "must be a JSON array of positive integer ids",
        )
    })?;
    if ids.len() > ID_SET_MAX || ids.iter().any(|&i| i <= 0) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("must hold at most {ID_SET_MAX} positive integer ids"),
        ));
    }
    Ok(ids.into_iter().collect())
}

/// Floor for `repair.tick_interval_secs`: `0` would repair on every pass.
pub const REPAIR_TICK_MIN_SECS: u64 = 60;

pub fn spec(key: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.key == key)
}

/// Validate one projects-root path: absolute or `~` / `~/…` (expanded
/// against the host's `$HOME`), no control characters, no `..` component.
/// The value ends up inside remote shell commands (always quoted) and in a
/// local `read_dir`, so traversal and control bytes are refused outright.
pub fn validate_base_path(label: &str, path: &str) -> Result<(), IpcError> {
    let bad = |msg: &str| Err(IpcError::new(codes::E_INVALID, format!("{label}: {msg}")));
    if path.is_empty() {
        return bad("path must not be empty");
    }
    if path.len() > MAX_PATH_LEN {
        return bad("path is too long");
    }
    if path.chars().any(char::is_control) {
        return bad("path must not contain control characters");
    }
    if !(path.starts_with('/') || path == "~" || path.starts_with("~/")) {
        return bad("path must be absolute or start with ~/");
    }
    if path.split('/').any(|c| c == "..") {
        return bad("path must not contain '..'");
    }
    Ok(())
}

/// Parse + validate a `Kind::PathMap` value into a map with trimmed paths.
pub fn parse_path_map(key: &str, raw: &str) -> Result<BTreeMap<String, String>, IpcError> {
    let map: BTreeMap<String, String> = serde_json::from_str(raw).map_err(|_| {
        IpcError::new(
            codes::E_INVALID,
            format!("{key} must be a JSON object of host alias to path strings"),
        )
    })?;
    let mut out = BTreeMap::new();
    for (alias, path) in map {
        // A map key, not a target: a `local` entry must not void the whole
        // map on a hub with `hub.local_host=false`.
        crate::validate::host_alias_syntax(&alias)?;
        let path = path.trim();
        validate_base_path(&format!("{key}[{alias}]"), path)?;
        out.insert(alias, path.to_string());
    }
    Ok(out)
}

/// Validate a `(key, value)` pair against the registry. `E_INVALID` on an
/// unknown key or a value of the wrong shape.
pub fn validate(key: &str, value: &str) -> Result<(), IpcError> {
    let spec = spec(key)
        .ok_or_else(|| IpcError::new(codes::E_INVALID, format!("unknown setting {key}")))?;
    let v = value.trim();
    match spec.kind {
        Kind::Bool if v == "true" || v == "false" => Ok(()),
        Kind::Bool => Err(IpcError::new(
            codes::E_INVALID,
            format!("{key} must be \"true\" or \"false\""),
        )),
        Kind::Secs if v.parse::<u64>().is_ok_and(|n| n <= MAX_SECS) => Ok(()),
        Kind::Secs => Err(IpcError::new(
            codes::E_INVALID,
            format!("{key} must be an integer number of seconds between 0 and {MAX_SECS}"),
        )),
        Kind::SecsMin(min)
            if v.parse::<u64>()
                .is_ok_and(|n| (min..=MAX_SECS).contains(&n)) =>
        {
            Ok(())
        }
        Kind::SecsMin(min) => Err(IpcError::new(
            codes::E_INVALID,
            format!("{key} must be an integer number of seconds between {min} and {MAX_SECS}"),
        )),
        Kind::Int { min, max } if v.parse::<u64>().is_ok_and(|n| (min..=max).contains(&n)) => {
            Ok(())
        }
        Kind::Int { min, max } => Err(IpcError::new(
            codes::E_INVALID,
            format!("{key} must be an integer between {min} and {max}"),
        )),
        Kind::ChoiceSet(options) => parse_choice_set(key, options, v).map(|_| ()),
        Kind::Choice(options) if options.contains(&v) => Ok(()),
        Kind::Choice(options) => Err(IpcError::new(
            codes::E_INVALID,
            format!("{key} must be one of: {}", options.join(", ")),
        )),
        Kind::PathMap => parse_path_map(key, v).map(|_| ()),
        Kind::IdSet => parse_id_set(v)
            .map(|_| ())
            .map_err(|e| IpcError::new(codes::E_INVALID, format!("{key} {}", e.message))),
        Kind::PriceMap => crate::service::usage::parse_price_overrides(v).map(|_| ()),
        Kind::Text { max } if v.len() <= max && !v.chars().any(char::is_control) => Ok(()),
        Kind::Text { max } => Err(IpcError::new(
            codes::E_INVALID,
            format!("{key} must be one line of at most {max} characters"),
        )),
    }
}

/// Pure: resolve a raw stored value (or `None`) against its spec, falling
/// back to the default when missing or malformed.
pub fn resolve(key: &str, raw: Option<&str>) -> String {
    let Some(spec) = spec(key) else {
        return raw.unwrap_or_default().to_string();
    };
    match raw.map(str::trim) {
        Some(v) if validate(key, v).is_ok() => v.to_string(),
        _ => spec.default.to_string(),
    }
}

pub fn get_bool(s: &Store, key: &str) -> bool {
    get_string(s, key) == "true"
}

pub fn get_secs(s: &Store, key: &str) -> u64 {
    get_string(s, key).parse().unwrap_or(0)
}

/// Effective value of a registered setting (stored or default).
pub fn get_string(s: &Store, key: &str) -> String {
    let raw = s.get_setting(key).ok().flatten();
    resolve(key, raw.as_deref())
}

/// What a page shows for `spec`: the resolved value of an editable setting,
/// or, for a key another subsystem owns, its stored text as that owner wrote
/// it (else the default) — the owner may read spellings `resolve` would not.
fn effective(s: &Store, spec: &Spec) -> String {
    if spec.owned_by.is_some() {
        return s
            .get_setting(spec.key)
            .ok()
            .flatten()
            .map(|v| v.trim().to_string())
            .unwrap_or_else(|| spec.default.to_string());
    }
    get_string(s, spec.key)
}

/// The `projects.base_path` map (empty when unset or malformed).
pub fn base_path_map(s: &Store) -> BTreeMap<String, String> {
    parse_path_map(PROJECTS_BASE_PATH, &get_string(s, PROJECTS_BASE_PATH)).unwrap_or_default()
}

/// Every registered setting with its effective value (stored or default),
/// plus the derived `PROJECTS_RESOLVED_BASE` preview.
pub fn read_all(s: &Store) -> BTreeMap<String, String> {
    let mut all: BTreeMap<String, String> = SPECS
        .iter()
        .map(|spec| (spec.key.to_string(), effective(s, spec)))
        .collect();
    all.insert(
        PROJECTS_LOCAL_ENV_BASE.to_string(),
        crate::service::projects::local_env_base().unwrap_or_default(),
    );
    let resolved = crate::service::projects::resolved_bases(s);
    all.insert(
        PROJECTS_RESOLVED_BASE.to_string(),
        serde_json::to_string(&resolved).unwrap_or_else(|_| "{}".into()),
    );
    all
}

/// A setting's value shape as `describe` presents it: the bounds and
/// options a form needs, with `Secs` and `SecsMin` folded into one `secs`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KindDesc {
    Bool,
    Secs { min: u64, max: u64 },
    Int { min: u64, max: u64 },
    Choice { options: &'static [&'static str] },
    ChoiceSet { options: &'static [&'static str] },
    PathMap,
    IdSet,
    PriceMap,
    Text { max: usize },
}

impl From<Kind> for KindDesc {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Bool => KindDesc::Bool,
            Kind::Secs => KindDesc::Secs {
                min: 0,
                max: MAX_SECS,
            },
            Kind::SecsMin(min) => KindDesc::Secs { min, max: MAX_SECS },
            Kind::Int { min, max } => KindDesc::Int { min, max },
            Kind::Choice(options) => KindDesc::Choice { options },
            Kind::ChoiceSet(options) => KindDesc::ChoiceSet { options },
            Kind::PathMap => KindDesc::PathMap,
            Kind::IdSet => KindDesc::IdSet,
            Kind::PriceMap => KindDesc::PriceMap,
            Kind::Text { max } => KindDesc::Text { max },
        }
    }
}

/// One registered setting with its metadata and effective value: what a
/// generated settings page renders and what an agent reads before it
/// proposes a change (`get_settings { describe: true }`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Descriptor {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: KindDesc,
    pub default: &'static str,
    /// The effective value (stored, else the default).
    pub value: String,
    /// The effective value differs from the default.
    pub modified: bool,
    pub unit: Unit,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zero: Option<&'static str>,
    pub tags: &'static [Tag],
    pub danger: Danger,
    pub restart: Restart,
    pub ai: AiPolicy,
    /// Set for a key another subsystem owns: where it is changed. A page
    /// shows it read-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<&'static str>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub option_labels: &'static [(&'static str, &'static str)],
}

impl Spec {
    /// This spec described with `value` as its effective value.
    pub fn describe(&self, value: String) -> Descriptor {
        Descriptor {
            key: self.key,
            label: self.label,
            help: self.help,
            kind: self.kind.into(),
            default: self.default,
            modified: value != self.default,
            value,
            unit: self.unit,
            zero: self.zero,
            tags: self.tags,
            danger: self.danger,
            restart: self.restart,
            ai: self.ai,
            owned_by: self.owned_by,
            option_labels: self.option_labels,
        }
    }
}

/// Every registered setting described, in display order. The derived
/// read-only `projects.*` previews of [`read_all`] are not settings and are
/// left out.
pub fn describe(s: &Store) -> Vec<Descriptor> {
    SPECS
        .iter()
        .map(|spec| spec.describe(effective(s, spec)))
        .collect()
}

/// Validate then persist one setting. A `PathMap` is stored normalised
/// (trimmed paths, sorted keys).
pub fn set(s: &Store, key: &str, value: &str) -> Result<(), IpcError> {
    if let Some(how) = spec(key).and_then(|sp| sp.owned_by) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("{key} is read-only here: change it with {how}"),
        ));
    }
    validate(key, value)?;
    let v = value.trim();
    let stored = match spec(key).map(|sp| sp.kind) {
        Some(Kind::PathMap) => serde_json::to_string(&parse_path_map(key, v)?)
            .map_err(|e| IpcError::new(codes::E_INVALID, e.to_string()))?,
        Some(Kind::ChoiceSet(options)) => parse_choice_set(key, options, v)?.join(","),
        Some(Kind::IdSet) => serde_json::to_string(&parse_id_set(v)?)
            .map_err(|e| IpcError::new(codes::E_INVALID, e.to_string()))?,
        Some(Kind::PriceMap) => {
            serde_json::to_string(&crate::service::usage::parse_price_overrides(v)?)
                .map_err(|e| IpcError::new(codes::E_INVALID, e.to_string()))?
        }
        _ => v.to_string(),
    };
    s.set_setting(key, &stored)?;
    s.emit_settings_changed(key);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text` with its comments removed: whole `//` lines, and `/* … */` and
    /// `<!-- … -->` blocks (also across lines). Line structure is kept.
    fn code_only(text: &str) -> String {
        const BLOCKS: [(&str, &str); 2] = [("/*", "*/"), ("<!--", "-->")];
        let mut out = String::new();
        let mut in_block: Option<&str> = None;
        for line in text.lines() {
            let mut rest = line;
            let mut kept = String::new();
            loop {
                if let Some(close) = in_block {
                    match rest.find(close) {
                        Some(p) => {
                            rest = &rest[p + close.len()..];
                            in_block = None;
                        }
                        None => break,
                    }
                } else {
                    let open = BLOCKS
                        .iter()
                        .filter_map(|(o, c)| rest.find(o).map(|p| (p, *o, *c)))
                        .min_by_key(|(p, _, _)| *p);
                    match open {
                        Some((p, o, c)) => {
                            kept.push_str(&rest[..p]);
                            rest = &rest[p + o.len()..];
                            in_block = Some(c);
                        }
                        None => {
                            kept.push_str(rest);
                            break;
                        }
                    }
                }
            }
            if !kept.trim_start().starts_with("//") {
                out.push_str(&kept);
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn code_only_drops_every_comment_form() {
        let text = "  a: 'x.key',\n\
                    // b: 'y.key',\n\
                    /* c: 'z.key', */ d\n\
                    /*\n e: 'w.key',\n*/\n\
                    <!-- SETTING_KEYS.hidden -->\n\
                    <input value={SETTING_KEYS.shown} />\n";
        let code = code_only(text);
        assert!(code.contains("a: 'x.key',"));
        assert!(code.contains(" d"));
        assert!(code.contains("SETTING_KEYS.shown"));
        for gone in ["y.key", "z.key", "w.key", "SETTING_KEYS.hidden"] {
            assert!(!code.contains(gone), "{gone} survived: {code}");
        }
    }

    /// Every editable setting has a `SETTING_KEYS` entry and a matching
    /// `SETTING_DEFAULTS` value in `fleet_settings.ts`.
    #[test]
    fn every_spec_is_mirrored_in_fleet_settings_ts() {
        // Where a setting is SHOWN is the page specs' business
        // (`pages::tests::every_setting_has_one_home`); the frontend still
        // keeps a typed key and a default for every editable setting, for the
        // components that read one before `get_fleet_settings` answers.
        const TS: &str = include_str!("../../../../src/lib/fleet_settings.ts");
        let ts = code_only(TS);
        for spec in SPECS.iter().filter(|s| s.owned_by.is_none()) {
            // `  camelName: 'the.key',` (SETTING_DEFAULTS lines start with a quote).
            let entry = format!(": '{}',", spec.key);
            assert!(
                ts.lines()
                    .any(|l| l.trim_end().ends_with(&entry) && !l.trim_start().starts_with('\'')),
                "{} has no SETTING_KEYS entry in src/lib/fleet_settings.ts",
                spec.key
            );
            assert!(
                ts.contains(&format!("'{}': '{}',", spec.key, spec.default)),
                "{}: SETTING_DEFAULTS must mirror the backend default {:?}",
                spec.key,
                spec.default
            );
        }
    }

    #[test]
    fn retention_windows_default_to_d21_and_zero_keeps_forever() {
        for (key, default) in [
            (WORK_RETENTION_JOURNAL_DAYS, "365"),
            (WORK_RETENTION_TRACKER_ITEMS_DAYS, "180"),
            (WORK_RETENTION_TIMELINE_WORK_EVENTS_DAYS, "180"),
        ] {
            assert_eq!(resolve(key, None), default, "{key}");
            for ok in ["0", "1", "3650"] {
                assert!(validate(key, ok).is_ok(), "{key}={ok}");
            }
            for bad in ["3651", "-1", "1.5", "forever", ""] {
                assert_eq!(
                    validate(key, bad).unwrap_err().code,
                    codes::E_INVALID,
                    "{key}={bad}"
                );
            }
        }
        // M2's key is superseded: no longer writable through the registry.
        assert!(spec(LEGACY_WORK_JOURNAL_DAYS).is_none());
        assert_eq!(
            validate(LEGACY_WORK_JOURNAL_DAYS, "90").unwrap_err().code,
            codes::E_INVALID
        );
    }

    #[test]
    fn tidy_settings_default_to_suggest_only_with_safe_reasons() {
        assert_eq!(resolve(WORK_AUTO_TIDY, None), "false");
        assert_eq!(resolve(WORK_TIDY_DONE_DAYS, None), "2");
        assert_eq!(resolve(WORK_TIDY_IDLE_HOURS, None), "4");
        assert_eq!(resolve(WORK_TIDY_IDLE_UNLINKED_DAYS, None), "7");
        assert!(validate(WORK_TIDY_IDLE_UNLINKED_DAYS, "1").is_ok());
        assert!(validate(WORK_TIDY_IDLE_UNLINKED_DAYS, "90").is_ok());
        for bad in ["0", "91", "-3", "a week"] {
            assert!(
                validate(WORK_TIDY_IDLE_UNLINKED_DAYS, bad).is_err(),
                "{bad}"
            );
        }
        assert_eq!(
            resolve(WORK_AUTO_TIDY_REASONS, None),
            "done_idle,pr_merged_idle"
        );
        assert!(validate(WORK_AUTO_TIDY_REASONS, "").is_ok(), "none");
        assert!(validate(WORK_AUTO_TIDY_REASONS, " not_planned , done_idle").is_ok());
        for bad in [
            "duplicate_worktree",
            "ghost_expiring",
            "idle_unlinked",
            "done_idle,kill",
        ] {
            assert_eq!(
                validate(WORK_AUTO_TIDY_REASONS, bad).unwrap_err().code,
                codes::E_INVALID,
                "{bad}"
            );
        }
        assert!(validate(WORK_TIDY_DONE_DAYS, "0").is_err());
        let s = Store::open_in_memory().unwrap();
        set(
            &s,
            WORK_AUTO_TIDY_REASONS,
            "not_planned,done_idle,done_idle",
        )
        .unwrap();
        assert_eq!(
            get_string(&s, WORK_AUTO_TIDY_REASONS),
            "done_idle,not_planned"
        );
    }

    #[test]
    fn move_transcript_cap_is_a_bounded_integer_of_mib() {
        assert_eq!(spec(MOVE_MAX_TRANSCRIPT_MB).unwrap().default, "200");
        for ok in ["1", "200", "4096", " 50 "] {
            assert!(validate(MOVE_MAX_TRANSCRIPT_MB, ok).is_ok(), "{ok}");
        }
        for bad in ["0", "4097", "-1", "abc", "1.5", ""] {
            assert!(validate(MOVE_MAX_TRANSCRIPT_MB, bad).is_err(), "{bad}");
        }
        // A stored garbage value resolves to the default.
        assert_eq!(resolve(MOVE_MAX_TRANSCRIPT_MB, Some("0")), "200");
        assert_eq!(resolve(MOVE_MAX_TRANSCRIPT_MB, Some("64")), "64");
    }

    #[test]
    fn carry_settings_have_specs_defaults_and_bounds() {
        assert_eq!(spec(MOVE_MAX_BUNDLE_MB).unwrap().default, "500");
        assert_eq!(spec(MOVE_IGNORED_ENTRY_KB).unwrap().default, "1024");
        assert_eq!(spec(MOVE_IGNORED_TOTAL_MB).unwrap().default, "20");
        for key in [
            MOVE_MAX_BUNDLE_MB,
            MOVE_IGNORED_ENTRY_KB,
            MOVE_IGNORED_TOTAL_MB,
        ] {
            assert!(validate(key, "1").is_ok(), "{key}");
            assert!(validate(key, "0").is_err(), "{key}");
            assert!(validate(key, "abc").is_err(), "{key}");
        }
        assert!(validate(MOVE_MAX_BUNDLE_MB, "4096").is_ok());
        assert!(validate(MOVE_MAX_BUNDLE_MB, "4097").is_err());
        assert!(validate(MOVE_IGNORED_ENTRY_KB, "1048576").is_ok());
        assert!(validate(MOVE_IGNORED_ENTRY_KB, "1048577").is_err());
        assert!(validate(MOVE_IGNORED_TOTAL_MB, "1024").is_ok());
        assert!(validate(MOVE_IGNORED_TOTAL_MB, "1025").is_err());
        assert_eq!(resolve(MOVE_IGNORED_TOTAL_MB, None), "20");
        assert_eq!(resolve(MOVE_IGNORED_TOTAL_MB, Some("5")), "5");
    }

    #[test]
    fn session_state_cap_has_a_spec_a_default_and_bounds() {
        assert_eq!(spec(MOVE_MAX_SESSION_STATE_MB).unwrap().default, "200");
        assert!(validate(MOVE_MAX_SESSION_STATE_MB, "1").is_ok());
        assert!(validate(MOVE_MAX_SESSION_STATE_MB, "0").is_err());
        assert!(validate(MOVE_MAX_SESSION_STATE_MB, "4096").is_ok());
        assert!(validate(MOVE_MAX_SESSION_STATE_MB, "4097").is_err());
        assert_eq!(resolve(MOVE_MAX_SESSION_STATE_MB, Some("50")), "50");
    }

    #[test]
    fn wait_max_mins_has_a_spec_a_default_and_a_week_bound() {
        assert_eq!(spec(MOVE_WAIT_MAX_MINS).unwrap().default, "240");
        assert!(validate(MOVE_WAIT_MAX_MINS, "1").is_ok());
        assert!(validate(MOVE_WAIT_MAX_MINS, "0").is_err());
        assert!(validate(MOVE_WAIT_MAX_MINS, "10080").is_ok());
        assert!(validate(MOVE_WAIT_MAX_MINS, "10081").is_err());
        assert_eq!(resolve(MOVE_WAIT_MAX_MINS, None), "240");
        assert_eq!(resolve(MOVE_WAIT_MAX_MINS, Some("60")), "60");
    }

    #[test]
    fn every_default_validates_against_its_own_spec() {
        for spec in SPECS {
            assert!(validate(spec.key, spec.default).is_ok(), "{}", spec.key);
        }
    }

    #[test]
    fn validate_rejects_unknown_keys_and_bad_shapes() {
        assert_eq!(validate("mcp.token", "x").unwrap_err().code, "E_INVALID");
        assert_eq!(validate(GC_ENABLED, "yes").unwrap_err().code, "E_INVALID");
        assert_eq!(
            validate(GC_BG_IDLE_SECS, "-1").unwrap_err().code,
            "E_INVALID"
        );
        assert_eq!(
            validate(GC_BG_IDLE_SECS, "1.5").unwrap_err().code,
            "E_INVALID"
        );
        assert!(validate(GC_BG_IDLE_SECS, " 3600 ").is_ok());
        assert!(validate(GC_BG_IDLE_SECS, &MAX_SECS.to_string()).is_ok());
        assert_eq!(
            validate(GC_BG_IDLE_SECS, &(MAX_SECS + 1).to_string())
                .unwrap_err()
                .code,
            "E_INVALID"
        );
        assert!(validate(PLAYBOOK_PRESS_ENTER, "true").is_ok());
    }

    #[test]
    fn layout_accepts_only_known_choices() {
        assert!(validate(PROJECTS_LAYOUT, "github").is_ok());
        assert!(validate(PROJECTS_LAYOUT, " flat ").is_ok());
        assert_eq!(
            validate(PROJECTS_LAYOUT, "gitlab").unwrap_err().code,
            "E_INVALID"
        );
        assert_eq!(resolve(PROJECTS_LAYOUT, Some("nope")), "github");
    }

    #[test]
    fn base_path_accepts_absolute_and_home_relative() {
        for ok in [
            "/srv/repos",
            "~",
            "~/code",
            "~/projects/github.com",
            "/a/./b",
        ] {
            assert!(validate_base_path("p", ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn base_path_rejects_relative_traversal_and_control_chars() {
        for bad in [
            "",
            "code",
            "./code",
            "~user/code",
            "-oProxyCommand=x",
            "/srv/../etc",
            "~/..",
            "/srv/a\nb",
            "/srv/a\u{7}b",
        ] {
            assert_eq!(
                validate_base_path("p", bad).unwrap_err().code,
                "E_INVALID",
                "{bad:?}"
            );
        }
        assert!(validate_base_path("p", &format!("/{}", "a".repeat(MAX_PATH_LEN))).is_err());
    }

    #[test]
    fn path_map_validates_aliases_paths_and_shape() {
        assert!(validate(PROJECTS_BASE_PATH, "{}").is_ok());
        assert!(validate(PROJECTS_BASE_PATH, r#"{"local":"/srv","vps-1":"~/code"}"#).is_ok());
        for bad in [
            "",
            "[]",
            "\"/srv\"",
            r#"{"local":1}"#,
            r#"{"-oProxy":"/srv"}"#,
            r#"{"local":"relative"}"#,
            r#"{"local":"/a/../b"}"#,
        ] {
            assert_eq!(
                validate(PROJECTS_BASE_PATH, bad).unwrap_err().code,
                "E_INVALID",
                "{bad}"
            );
        }
        // garbage stored value falls back to "no overrides"
        assert_eq!(resolve(PROJECTS_BASE_PATH, Some("{oops")), "{}");
    }

    #[test]
    fn resolve_falls_back_to_default_on_missing_or_garbage() {
        assert_eq!(resolve(GC_ENABLED, None), "false");
        assert_eq!(resolve(GC_ENABLED, Some("maybe")), "false");
        assert_eq!(resolve(GC_ENABLED, Some("true")), "true");
        assert_eq!(resolve(GC_WORK_IDLE_SECS, Some("abc")), "0");
        assert_eq!(resolve(GC_SHELL_IDLE_SECS, None), "604800");
    }

    #[test]
    fn store_roundtrip_and_read_all_defaults() {
        let s = Store::open_in_memory().unwrap();
        let all = read_all(&s);
        // every spec plus the two derived projects preview entries
        assert_eq!(all.len(), SPECS.len() + 2);
        assert!(all.contains_key(PROJECTS_LOCAL_ENV_BASE));
        assert_eq!(
            set(&s, PROJECTS_LOCAL_ENV_BASE, "/x").unwrap_err().code,
            "E_INVALID"
        );
        assert_eq!(all[GC_BG_IDLE_SECS], "86400");
        assert_eq!(all[PROJECTS_BASE_PATH], "{}");
        assert_eq!(all[PROJECTS_LAYOUT], "github");
        assert!(all[PROJECTS_RESOLVED_BASE].contains("\"local\""));
        assert!(!get_bool(&s, GC_ENABLED));
        assert_eq!(get_secs(&s, GC_SWEEP_INTERVAL_SECS), 300);

        set(&s, GC_ENABLED, "true").unwrap();
        set(&s, GC_BG_IDLE_SECS, "60").unwrap();
        assert!(get_bool(&s, GC_ENABLED));
        assert_eq!(get_secs(&s, GC_BG_IDLE_SECS), 60);
        assert_eq!(set(&s, "nope", "1").unwrap_err().code, "E_INVALID");
        // the derived preview key is not writable
        assert_eq!(
            set(&s, PROJECTS_RESOLVED_BASE, "{}").unwrap_err().code,
            "E_INVALID"
        );
    }

    #[test]
    fn usage_settings_default_on_and_validate_price_overrides() {
        let s = Store::open_in_memory().unwrap();
        assert!(get_bool(&s, USAGE_ENABLED));
        assert_eq!(get_secs(&s, USAGE_INTERVAL_SECS), 300);
        assert_eq!(get_string(&s, USAGE_PRICES_JSON), "{}");
        assert_eq!(
            validate(USAGE_PRICES_JSON, r#"{"opus":{"input":1}}"#)
                .unwrap_err()
                .code,
            "E_INVALID"
        );
        set(
            &s,
            USAGE_PRICES_JSON,
            r#" {"Opus-4-1":{"input":15,"output":75,"cache_write":30,"cache_read":1.5}} "#,
        )
        .unwrap();
        // Stored normalised: lower-cased keys.
        assert!(get_string(&s, USAGE_PRICES_JSON).starts_with(r#"{"opus-4-1":"#));
        assert_eq!(resolve(USAGE_PRICES_JSON, Some("garbage")), "{}");
        set(&s, USAGE_INTERVAL_SECS, "0").unwrap();
        assert_eq!(get_secs(&s, USAGE_INTERVAL_SECS), 0);
    }

    #[test]
    fn path_map_is_stored_normalised() {
        let s = Store::open_in_memory().unwrap();
        set(
            &s,
            PROJECTS_BASE_PATH,
            r#" {"vps":" ~/code ","local":"/srv"} "#,
        )
        .unwrap();
        assert_eq!(
            s.get_setting(PROJECTS_BASE_PATH).unwrap().as_deref(),
            Some(r#"{"local":"/srv","vps":"~/code"}"#)
        );
        assert_eq!(base_path_map(&s)["vps"], "~/code");
        // a rejected write leaves the stored value alone
        assert!(set(&s, PROJECTS_BASE_PATH, r#"{"vps":"../x"}"#).is_err());
        assert_eq!(base_path_map(&s)["vps"], "~/code");
    }

    #[test]
    fn repair_on_tick_defaults_off_with_a_ten_minute_cadence() {
        let s = Store::open_in_memory().unwrap();
        assert!(!get_bool(&s, REPAIR_AUTO_ON_TICK));
        assert_eq!(get_secs(&s, REPAIR_TICK_INTERVAL_SECS), 600);
        set(&s, REPAIR_AUTO_ON_TICK, "true").unwrap();
        set(&s, REPAIR_TICK_INTERVAL_SECS, "60").unwrap();
        assert!(get_bool(&s, REPAIR_AUTO_ON_TICK));
        assert_eq!(get_secs(&s, REPAIR_TICK_INTERVAL_SECS), 60);
        assert_eq!(
            validate(REPAIR_AUTO_ON_TICK, "on").unwrap_err().code,
            "E_INVALID"
        );
        // No "repair on every pass": the cadence has a 60 s floor.
        for bad in ["0", "59", "-1", "abc"] {
            assert_eq!(
                validate(REPAIR_TICK_INTERVAL_SECS, bad).unwrap_err().code,
                "E_INVALID",
                "{bad}"
            );
        }
        assert!(validate(REPAIR_TICK_INTERVAL_SECS, &MAX_SECS.to_string()).is_ok());
        assert_eq!(resolve(REPAIR_TICK_INTERVAL_SECS, Some("0")), "600");
    }

    #[test]
    fn decide_settings_default_to_off() {
        assert_eq!(resolve(DECIDE_JEV_ENABLED, None), "false");
        assert_eq!(resolve(DECIDE_JEV_UNASSIGNED, None), "false");
        assert_eq!(resolve(DECIDE_JEV_STATUS_MAP, None), "off");
        assert_eq!(resolve(DECIDE_JEV_WORK_LINK, None), "off");
        assert_eq!(resolve(DECIDE_JEV_MODEL, None), "jev-1.13.0");
        assert_eq!(resolve(DECIDE_RETENTION_DAYS, None), "90");
        // `auto` is not offered yet (D36), nor is a free-text model.
        assert!(validate(DECIDE_JEV_WORK_LINK, "auto").is_err());
        assert!(validate(DECIDE_JEV_STATUS_MAP, "shadow").is_ok());
        assert!(validate(DECIDE_JEV_MODEL, "jev-9; rm -rf").is_err());
        assert!(validate(DECIDE_JEV_TIMEOUT_MS, "50").is_err());
        assert!(validate(DECIDE_JEV_DAILY_TOKEN_BUDGET, "0").is_ok());
    }

    /// The metadata every generated page and doc reads (design
    /// `2026-09-28-declarative-pages-design.md`): a label and help for every
    /// setting, a unit that fits its kind, `zero` only where 0 is allowed,
    /// and no agent write where a change needs confirming.
    #[test]
    fn every_spec_has_consistent_metadata() {
        let mut labels = std::collections::BTreeSet::new();
        for spec in SPECS {
            let k = spec.key;
            assert!(
                !spec.label.is_empty() && spec.label.len() <= 40,
                "{k}: label must be 1-40 chars"
            );
            assert!(
                labels.insert(spec.label),
                "{k}: label {:?} is not unique",
                spec.label
            );
            assert!(
                spec.help.ends_with('.') && !spec.help.contains('\n') && spec.help.len() <= 300,
                "{k}: help must be one short paragraph ending in a full stop"
            );
            assert!(
                !spec.help.contains('<') && !spec.help.contains('|'),
                "{k}: help is plain text (no markup, no table pipes)"
            );
            match spec.kind {
                Kind::Secs | Kind::SecsMin(_) => assert!(
                    matches!(
                        spec.unit,
                        Unit::Seconds | Unit::Minutes | Unit::Hours | Unit::Days
                    ),
                    "{k}: a seconds value needs a time unit to be shown in"
                ),
                Kind::Int { .. } => assert!(spec.unit != Unit::None, "{k}: an Int needs a unit"),
                _ => assert_eq!(spec.unit, Unit::None, "{k}: only numbers have a unit"),
            }
            let zero_allowed = matches!(spec.kind, Kind::Secs | Kind::Int { min: 0, .. });
            assert!(
                spec.zero.is_none() || zero_allowed,
                "{k}: `zero` names what 0 means, but 0 is not allowed"
            );
            if !spec.option_labels.is_empty() {
                let options: Vec<&str> = match spec.kind {
                    Kind::Choice(o) | Kind::ChoiceSet(o) => o.to_vec(),
                    _ => panic!("{k}: option labels on a kind with no options"),
                };
                let labelled: Vec<&str> = spec.option_labels.iter().map(|(v, _)| *v).collect();
                assert_eq!(labelled, options, "{k}: label every option, in order");
            }
            if spec.owned_by.is_some() {
                assert_eq!(
                    spec.ai,
                    AiPolicy::Never,
                    "{k}: an agent never writes a key it does not own"
                );
                assert_eq!(
                    spec.danger,
                    Danger::None,
                    "{k}: nothing to confirm on a read-only key"
                );
            }
            if let Danger::Confirm(message) = spec.danger {
                assert!(
                    message.ends_with('.'),
                    "{k}: a confirm message is a sentence"
                );
                assert_eq!(
                    spec.ai,
                    AiPolicy::Never,
                    "{k}: an agent never makes a confirmed change"
                );
            }
        }
    }

    /// A validated write emits `settings:changed` with the key; a refused
    /// one emits nothing.
    #[test]
    fn a_write_emits_settings_changed() {
        let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        bus.take();
        set(&s, WORK_RECENT_DAYS, "30").unwrap();
        assert!(set(&s, WORK_RECENT_DAYS, "soon").is_err());
        assert!(set(&s, crate::mcp::SETTING_ENABLED, "true").is_err());
        assert_eq!(
            bus.take(),
            vec![format!("settings:changed:{WORK_RECENT_DAYS}")]
        );
    }

    /// Secrets, key material and internal state never enter the registry,
    /// so `describe`, the docs and every page stay blind to them.
    #[test]
    fn never_registered_keys_stay_out() {
        for key in [
            crate::mcp::SETTING_TOKEN,
            crate::service::hub::SETTING_TLS_KEY,
            crate::service::hub::SETTING_TLS_CERT,
            crate::service::operator::SETTING_OPERATOR_TOKEN_SHA,
            crate::service::operator::SETTING_OPERATOR_SESSION,
            crate::service::operator::SETTING_OPERATOR_HOST,
            crate::service::address::FLEET_ID_KEY,
            crate::service::quick_replies::SETTING_KEY,
            "hub.client_plaintext_token",
        ] {
            assert!(spec(key).is_none(), "{key} must never be registered");
        }
    }

    /// D-P7: a key another subsystem owns is described as it is stored and
    /// refused on write, with where it is changed.
    #[test]
    fn owned_keys_are_shown_as_stored_and_refused_on_write() {
        let s = Store::open_in_memory().unwrap();
        s.set_setting(crate::service::hub::SETTING_LOCAL_HOST, " no ")
            .unwrap();
        let d = describe(&s)
            .into_iter()
            .find(|d| d.key == crate::service::hub::SETTING_LOCAL_HOST)
            .unwrap();
        assert_eq!(d.value, "no", "the owner's spelling, not the default");
        assert!(d.owned_by.is_some());
        let err = set(&s, crate::mcp::guard::SETTING_CONFIRM_DESTRUCTIVE, "true").unwrap_err();
        assert!(err.message.contains("read-only here"), "{}", err.message);
        assert_eq!(
            s.get_setting(crate::mcp::guard::SETTING_CONFIRM_DESTRUCTIVE)
                .unwrap(),
            None,
            "nothing written"
        );
    }

    #[test]
    fn describe_carries_the_value_and_marks_a_changed_one_modified() {
        let s = Store::open_in_memory().unwrap();
        let all = describe(&s);
        assert_eq!(all.len(), SPECS.len());
        assert!(all.iter().all(|d| !d.modified && d.value == d.default));

        set(&s, WORK_RECENT_DAYS, "30").unwrap();
        let d = describe(&s)
            .into_iter()
            .find(|d| d.key == WORK_RECENT_DAYS)
            .unwrap();
        assert!(d.modified);
        assert_eq!(d.value, "30");
        assert_eq!(d.kind, KindDesc::Int { min: 1, max: 365 });

        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["kind"]["type"], "int");
        assert_eq!(json["unit"], "days");
        assert_eq!(json["danger"]["level"], "none");
        assert_eq!(json["ai"], "suggest");

        let gc = serde_json::to_value(spec(GC_ENABLED).unwrap().describe("false".into())).unwrap();
        assert_eq!(gc["danger"]["level"], "confirm");
        assert_eq!(gc["ai"], "never");
        let secs = serde_json::to_value(
            spec(REPAIR_TICK_INTERVAL_SECS)
                .unwrap()
                .describe("600".into()),
        )
        .unwrap();
        assert_eq!(
            secs["kind"],
            serde_json::json!({"type": "secs", "min": 60, "max": MAX_SECS})
        );
    }
}
