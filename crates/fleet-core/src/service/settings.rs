//! Typed registry of the operator-facing settings stored in the `settings`
//! table (key → string). Every key the Settings dialog can edit is declared
//! here with its default and value shape, so the Tauri command that writes a
//! setting can refuse unknown keys and garbage values, and every backend
//! reader (reconcile tick, playbooks, GC, project discovery) resolves the
//! same default.
//!
//! Keys that other subsystems own (MCP `mcp.*`, `controller.*`) are NOT
//! listed and cannot be written through this path.

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
}

/// Upper bound for `Kind::Secs` (ten years): keeps every `secs as i64`
/// arithmetic in the sweeper far from overflow.
pub const MAX_SECS: u64 = 10 * 365 * 24 * 3600;

/// Upper bound on one projects-root path.
pub const MAX_PATH_LEN: usize = 1024;

#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub key: &'static str,
    pub default: &'static str,
    pub kind: Kind,
}

// ── keys ──
pub const RECONCILE_INTERVAL_SECS: &str = "reconcile.interval_secs";
/// How long a resumable mass-loss row (`lost_reason` `host_reboot` /
/// `tmux_server_gone`, with a `claude_session_id`) is kept before Phase 2
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
pub const GC_ENABLED: &str = "gc.enabled";
pub const GC_BG_IDLE_SECS: &str = "gc.bg_idle_secs";
pub const GC_SHELL_IDLE_SECS: &str = "gc.shell_idle_secs";
pub const GC_WORK_IDLE_SECS: &str = "gc.work_idle_secs";
pub const GC_SWEEP_INTERVAL_SECS: &str = "gc.sweep_interval_secs";
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

/// Every editable setting. Order is the display order.
pub const SPECS: &[Spec] = &[
    Spec {
        key: RECONCILE_INTERVAL_SECS,
        default: "20",
        kind: Kind::Secs,
    },
    Spec {
        key: SESSIONS_LOST_TTL_SECS,
        default: "1209600",
        kind: Kind::Secs,
    },
    Spec {
        key: RESTORE_BATCH_SIZE,
        default: "4",
        kind: Kind::Int { min: 1, max: 16 },
    },
    Spec {
        key: RESTORE_STAGGER_MS,
        default: "3000",
        kind: Kind::Int { min: 0, max: 60000 },
    },
    Spec {
        key: PLAYBOOK_PRESS_ENTER,
        default: "false",
        kind: Kind::Bool,
    },
    Spec {
        key: PLAYBOOK_OOM_RECREATE,
        default: "false",
        kind: Kind::Bool,
    },
    Spec {
        key: GC_ENABLED,
        default: "false",
        kind: Kind::Bool,
    },
    Spec {
        key: GC_BG_IDLE_SECS,
        default: "86400",
        kind: Kind::Secs,
    },
    Spec {
        key: GC_SHELL_IDLE_SECS,
        default: "604800",
        kind: Kind::Secs,
    },
    Spec {
        key: GC_WORK_IDLE_SECS,
        default: "0",
        kind: Kind::Secs,
    },
    Spec {
        key: GC_SWEEP_INTERVAL_SECS,
        default: "300",
        kind: Kind::Secs,
    },
    Spec {
        key: PROJECTS_BASE_PATH,
        default: "{}",
        kind: Kind::PathMap,
    },
    Spec {
        key: PROJECTS_LAYOUT,
        default: "github",
        kind: Kind::Choice(LAYOUTS),
    },
    Spec {
        key: TASKS_MAX_AGE_SECS,
        default: "86400",
        kind: Kind::Secs,
    },
    Spec {
        key: REPAIR_AUTO_ON_TICK,
        default: "false",
        kind: Kind::Bool,
    },
    Spec {
        key: REPAIR_TICK_INTERVAL_SECS,
        default: "600",
        kind: Kind::SecsMin(REPAIR_TICK_MIN_SECS),
    },
    Spec {
        key: MOVE_MAX_TRANSCRIPT_MB,
        default: "200",
        kind: Kind::Int {
            min: 1,
            max: MOVE_MAX_TRANSCRIPT_MB_MAX,
        },
    },
    Spec {
        key: MOVE_MAX_BUNDLE_MB,
        default: "500",
        kind: Kind::Int {
            min: 1,
            max: MOVE_MAX_BUNDLE_MB_MAX,
        },
    },
    Spec {
        key: MOVE_IGNORED_ENTRY_KB,
        default: "1024",
        kind: Kind::Int {
            min: 1,
            max: MOVE_IGNORED_ENTRY_KB_MAX,
        },
    },
    Spec {
        key: MOVE_IGNORED_TOTAL_MB,
        default: "20",
        kind: Kind::Int {
            min: 1,
            max: MOVE_IGNORED_TOTAL_MB_MAX,
        },
    },
    Spec {
        key: MOVE_MAX_SESSION_STATE_MB,
        default: "200",
        kind: Kind::Int {
            min: 1,
            max: MOVE_MAX_SESSION_STATE_MB_MAX,
        },
    },
    Spec {
        key: MOVE_WAIT_MAX_MINS,
        default: "240",
        kind: Kind::Int {
            min: 1,
            max: MOVE_WAIT_MAX_MINS_MAX,
        },
    },
    Spec {
        key: USAGE_ENABLED,
        default: "true",
        kind: Kind::Bool,
    },
    Spec {
        key: USAGE_INTERVAL_SECS,
        default: "300",
        kind: Kind::Secs,
    },
    Spec {
        key: USAGE_PRICES_JSON,
        default: "{}",
        kind: Kind::PriceMap,
    },
    Spec {
        key: REPORTS_MAX_ROWS,
        default: "5000",
        kind: Kind::Int {
            min: 100,
            max: 100_000,
        },
    },
    Spec {
        key: REPORTS_MAX_AGE_SECS,
        default: "604800",
        kind: Kind::Secs,
    },
    Spec {
        key: WORK_RETENTION_JOURNAL_DAYS,
        default: "365",
        kind: Kind::Int { min: 0, max: 3650 },
    },
    Spec {
        key: WORK_RETENTION_TRACKER_ITEMS_DAYS,
        default: "180",
        kind: Kind::Int { min: 0, max: 3650 },
    },
    Spec {
        key: WORK_RETENTION_TIMELINE_WORK_EVENTS_DAYS,
        default: "180",
        kind: Kind::Int { min: 0, max: 3650 },
    },
    Spec {
        key: WORK_RECENT_DAYS,
        default: "14",
        kind: Kind::Int { min: 1, max: 365 },
    },
    Spec {
        key: WORK_SYNC_INTERVAL_SECS,
        default: "300",
        kind: Kind::Secs,
    },
    Spec {
        key: WORK_TRUSTED_BRANCH_PROJECTS,
        default: "[]",
        kind: Kind::IdSet,
    },
    Spec {
        key: WORK_EVIDENCE_SNIPPETS,
        default: "true",
        kind: Kind::Bool,
    },
    Spec {
        key: WORK_SESSION_START_CONTEXT,
        default: "false",
        kind: Kind::Bool,
    },
    Spec {
        key: WORK_CLASSIFY_NUDGE,
        default: "false",
        kind: Kind::Bool,
    },
    Spec {
        key: WORK_TIDY_DONE_DAYS,
        default: "2",
        kind: Kind::Int { min: 1, max: 365 },
    },
    Spec {
        key: WORK_TIDY_IDLE_HOURS,
        default: "4",
        kind: Kind::Int { min: 1, max: 720 },
    },
    Spec {
        key: WORK_TIDY_IDLE_UNLINKED_DAYS,
        default: "7",
        kind: Kind::Int { min: 1, max: 90 },
    },
    Spec {
        key: WORK_AUTO_TIDY,
        default: "false",
        kind: Kind::Bool,
    },
    Spec {
        key: WORK_AUTO_TIDY_REASONS,
        default: "done_idle,pr_merged_idle",
        kind: Kind::ChoiceSet(AUTO_TIDY_REASONS),
    },
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

/// The `projects.base_path` map (empty when unset or malformed).
pub fn base_path_map(s: &Store) -> BTreeMap<String, String> {
    parse_path_map(PROJECTS_BASE_PATH, &get_string(s, PROJECTS_BASE_PATH)).unwrap_or_default()
}

/// Every registered setting with its effective value (stored or default),
/// plus the derived `PROJECTS_RESOLVED_BASE` preview.
pub fn read_all(s: &Store) -> BTreeMap<String, String> {
    let mut all: BTreeMap<String, String> = SPECS
        .iter()
        .map(|spec| (spec.key.to_string(), get_string(s, spec.key)))
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

/// Validate then persist one setting. A `PathMap` is stored normalised
/// (trimmed paths, sorted keys).
pub fn set(s: &Store, key: &str, value: &str) -> Result<(), IpcError> {
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

    /// Every registered setting has a Settings dialog row: a `SETTING_KEYS`
    /// entry and a matching `SETTING_DEFAULTS` value in `fleet_settings.ts`,
    /// and a control in `SettingsDialog.svelte` addressing that key. Fails when
    /// a new `SPECS` entry ships without its row.
    #[test]
    fn every_spec_has_a_settings_dialog_row() {
        const TS: &str = include_str!("../../../../src/lib/fleet_settings.ts");
        const DIALOG: &str = include_str!("../../../../src/lib/SettingsDialog.svelte");
        // Comments cannot satisfy the check: a key only mentioned in a
        // `// …`, `/* … */` or `<!-- … -->` does not count as a row.
        let ts = code_only(TS);
        let dialog = code_only(DIALOG);
        for spec in SPECS {
            // `  camelName: 'the.key',` (SETTING_DEFAULTS lines start with a quote).
            let entry = format!(": '{}',", spec.key);
            let line = ts
                .lines()
                .find(|l| l.trim_end().ends_with(&entry) && !l.trim_start().starts_with('\''))
                .unwrap_or_else(|| {
                    panic!(
                        "{} has no SETTING_KEYS entry in src/lib/fleet_settings.ts",
                        spec.key
                    )
                });
            let name = line.trim().split(':').next().unwrap_or_default().trim();
            assert!(
                ts.contains(&format!("'{}': '{}',", spec.key, spec.default)),
                "{}: SETTING_DEFAULTS must mirror the backend default {:?}",
                spec.key,
                spec.default
            );
            assert!(
                dialog.contains(&format!("SETTING_KEYS.{name}")),
                "{} (SETTING_KEYS.{name}) has no row in src/lib/SettingsDialog.svelte",
                spec.key
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
}
