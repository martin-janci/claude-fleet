// Store owns the SQLite connection. It is wrapped in `Mutex<Store>` and
// registered via `tauri::Manager::manage()` because `rusqlite::Connection`
// is not Send+Sync. Commands access it via `State<'_, Mutex<Store>>`.

#[cfg(test)]
use crate::events::NoopEventBus;
use crate::events::{EventBus, RowChange};
use rusqlite::{Connection, OptionalExtension, Result, TransactionBehavior};
use std::sync::Arc;

mod access_requests;
mod account_usage_snapshots;
mod aux_usage;
pub mod backup;
mod bench_work_link;
mod catalog;
mod changesets;
mod clients;
mod control_handoffs;
mod control_tokens;
mod conversations;
mod debug_devices;
mod decisions;
mod deferred_prompts;
mod downloads;
mod forms;
mod guides;
mod host_setup;
mod hosts_accounts;
mod item_deps;
mod item_verify;
mod layers;
mod library;
mod local_workspaces;
mod mission_loop;
mod nl_census;
mod orchestration;
mod org_activity;
mod org_members;
mod org_projects;
mod orgs;
mod participants;
mod peer_links;
mod people;
mod pr_shepherd;
mod project_picks;
mod projects;
mod pull_requests;
mod read_cursors;
mod read_pool;
mod reconcile;
mod reports;
mod routines;
mod rows;
mod runs;
#[cfg(test)]
pub(crate) mod scale_fixture;
mod schema;
mod session_grants;
mod sessions;
mod setting_review;
mod start_rules;
mod task_report;
mod tasks;
#[cfg(test)]
mod test_support;
#[cfg(test)]
pub(crate) mod testgen;
mod timeline;
mod tracker_items;
mod tracker_writes;
mod trackers;
mod update;
mod usage;
mod work;
mod work_buckets;
mod work_comments;
mod work_describe;
mod work_detect;
mod work_journal;
mod work_local;
mod work_retention;
mod work_status;
mod work_tasks;
mod work_tidy;
mod work_usage;
mod work_view;

pub use access_requests::{AccessRequestRow, ACCESS_REQUEST_COOLDOWN_SECS};
pub use account_usage_snapshots::{UsageSnapshotRow, USAGE_HISTORY_KEEP_SECS};
pub use aux_usage::{
    AuxUsageRow, NewAuxUsage, AUX_ORIGINS, AUX_ORIGIN_BRIEF, AUX_ORIGIN_COMMIT_MESSAGE,
    AUX_ORIGIN_MORNING_BRIEF, AUX_ORIGIN_PLANNER, AUX_ORIGIN_RELEASE_NOTE, AUX_ORIGIN_SUMMARY,
    AUX_ORIGIN_TRIAGE, AUX_ORIGIN_WATCH_SUMMARY,
};
pub use bench_work_link::{BenchHostLink, BenchItemRow, BenchLinkRow, BenchUnlinkedRow};
pub use changesets::{
    AppliedRecord, ChangesetItemRow, ChangesetRow, NewChangesetItem, TriageVerdictRow,
};
pub use clients::{
    breaks_a_line, validate_client_mode, validate_client_name, ClientBinding, CLIENT_MODES,
    LINE_SEPARATORS,
};
pub use control_handoffs::{
    handoff_preview, ControlHandoffRow, HandoffItem, NewHandoff, HANDOFFS_KEEP, HANDOFF_PREVIEW_MAX,
};
pub use control_tokens::{ApiScope, ControlTokenRow, NewControlToken};
pub use conversations::{ConversationRow, StartSource, AWAITING_REBIND_TTL_SECS};
pub use debug_devices::{DebugDeviceRow, DebugDeviceScan, SeenDevice};
pub use decisions::{
    is_decision_word, DecisionKeyStatus, DecisionRunFilter, DecisionRunRow, DecisionStatRow,
    NewDecisionRun, RunScope, DECISION_BENCH_SUBJECT, DECISION_CALL_FAILURES, DECISION_FALLBACKS,
    DECISION_FOLLOWUPS, DECISION_MAX_CANDIDATES, DECISION_MODES, DECISION_NO_BASELINE,
    DECISION_PERSON_FOLLOWUPS, DECISION_SUBJECT_RUNS_MAX, DECISION_WORD_MAX_CHARS,
};
pub use deferred_prompts::{DeferredPromptRow, DeferredTiming, DEFERRED_MAX_ATTEMPTS};
pub use downloads::{DownloadRow, NewDownload};
pub use forms::{FormFinish, FormRow, NewForm, FORM_STATES};
pub use guides::{GuideProposalRow, NewGuideProposal, DECIDED_GUIDE_KEEP_SECS};
pub use host_setup::{AgentInstallRow, HostSetupRow, SetupCheck};
pub use item_deps::{ItemDepRow, DEP_SOURCES};
pub use item_verify::{
    normalize_done_when, VerificationRow, DONE_WHEN_LINE_MAX_CHARS, DONE_WHEN_MAX,
    VERIFY_NOTE_MAX_CHARS,
};
pub use layers::HostLayerRow;
pub use library::{LibraryItemRow, NewLibraryItem, KEEP as LIBRARY_KEEP};
pub(crate) use local_workspaces::paths_overlap;
pub use local_workspaces::{
    BaseEntry, FileStat, LocalActivityRow, LocalConflictRow, LocalPassWrite, LocalWorkspaceRow,
    LocalWorkspaceStatus, NewLocalConflict, NewLocalWorkspace, SideSeen, LOCAL_CONFLICT_KINDS,
    LOCAL_WORKSPACE_DRIVERS, LOCAL_WORKSPACE_STATES,
};
pub use mission_loop::{
    CardRow, GrantRow, MissionTaskCounts, NewCard, NewGrant, CARDS_OPEN_CAP, CARD_KINDS,
    CARD_STATES, GRANT_MAX_SECS, MISSION_LEASE_SECS,
};
pub use nl_census::{
    CensusItem, CensusJournal, CensusPair, CensusPrompt, NL_CENSUS_JOURNAL_KINDS,
    NL_CENSUS_MIN_SCHEMA,
};
pub use orchestration::{
    check_policy, mission_item_cap, mission_transition_allowed, mode_runs_loop, MissionEventRow,
    MissionPatch, MissionPolicy, MissionRepoRow, MissionRow, NewMission, NewMissionEvent,
    MISSION_FINAL_STATES, MISSION_ITEM_CAP, MISSION_MODES, MISSION_STATES, PLAN_MISSION_ITEM_CAP,
};
pub use org_members::{
    effective_device, role_receives_shares, validate_org_role, DeviceOrg, OrgMemberRow, NO_ORG,
    ORG_ROLES, ROLE_ADMIN, ROLE_MEMBER, ROLE_VIEWER,
};
pub use org_projects::{NewOrgProject, OrgProjectRow, ORG_PROJECT_NAME_MAX_CHARS};
pub use orgs::{
    normalize_rule, org_of_session, validate_org_color, validate_org_name, OrgRow, OrgRuleRow,
    SessionOrgFacts, ORG_JEV_REPLY_KEY, ORG_NAME_MAX_CHARS,
};
pub use participants::{ParticipantRow, PARTICIPANT_REMOTE, RETIRED_RETENTION_SECS};
pub use peer_links::{
    Adopted, Inbound, OutboxRow, PeerLinkRow, PeerLinkSummary, LINK_CONNECTED, LINK_INCOMPATIBLE,
    LINK_REFUSED, LINK_RETRYING, LINK_ROLE_DIALER, LINK_ROLE_LISTENER, LISTENER_STALE_SECS,
    PEER_PENDING_MAX_SECS,
};
pub use people::{
    machine_token_kind, validate_person_name, PersonRow, MAX_PERSON_NAME_LEN, PERSONAL_OWNER_NAME,
};
pub use pr_shepherd::{
    ShepherdEpisodeRow, ShepherdMergeRow, ShepherdRuleRow, SHEPHERD_LEVELS,
    SHEPHERD_RECIPES_MAX_CHARS,
};
pub use project_picks::{ProjectPickRow, PROJECT_GROUP_MAX_CHARS, PROJECT_VIS};
pub use pull_requests::{
    pr_events, repo_and_number, PrSeenBy, PullRequestRow, PR_EVENT_CI_FAILED, PR_EVENT_CI_PASSED,
    PR_EVENT_MERGED, PR_EVENT_REVIEW,
};
pub use read_cursors::CursorRow;
pub use read_pool::{read_via, ReadPool, READ_POOL_SIZE};
pub use reports::{ReportFilter, ReportRow};
pub use routines::{
    NewRoutineRun, RoutineFields, RoutineRow, RoutineRunRow, ROUTINE_LEASE_SECS, ROUTINE_OVERLAPS,
    ROUTINE_RUN_OUTCOMES, ROUTINE_RUN_OUTCOME_SOURCES, ROUTINE_RUN_STATES, ROUTINE_TRIGGERS,
};
pub use rows::*;
pub use runs::{
    RunRow, RunsFilter, RunsReach, MISSION_RUN_EVENTS, RUNS_DEFAULT_LIMIT, RUNS_MAX_LIMIT,
    RUN_KINDS, RUN_OUTCOMES, RUN_SOURCES,
};
pub use schema::known_schema_version;
#[cfg(test)]
pub(crate) use schema::LATEST_SCHEMA_VERSION;
pub use schema::{is_newer_schema_error, open_failure_advice};
pub use session_grants::{
    grant_generation, validate_grant_level, GrantDetail, GrantRecipient, SessionGrantRow,
    GRANT_ANSWER, GRANT_DRIVE, GRANT_LEVELS, GRANT_WATCH,
};
pub use sessions::PromptAckState;
pub use setting_review::{
    NewSettingProposal, SettingAuditRow, SettingProposalRow, DECIDED_PROPOSAL_KEEP_SECS,
    SETTING_AUDIT_KEEP,
};
pub use start_rules::{StartRuleRow, START_RULE_STATES};
pub use task_report::{
    EvidenceCommit, EvidenceFile, TaskEvidence, TaskReport, EVIDENCE_COMMITS_MAX,
    EVIDENCE_FILES_MAX, REPORT_ENTRY_MAX_CHARS, REPORT_LIST_MAX, REPORT_OUTCOMES,
};
pub(crate) use tracker_items::ItemUpsertOutcome;
pub use tracker_items::{github_covers, tracker_claims, ItemMeta, TrackerItemWrite, UpsertOutcome};
pub use tracker_writes::{
    NewTrackerWrite, TrackerWriteRow, WRITE_MAX_ATTEMPTS, WRITE_OP_PR_REMOTE_LINK,
};
pub use trackers::{
    ghes_host_ok, ghes_host_part, github_site, is_allowed_tracker_host, normalize_dc_site,
    normalize_provider_site, normalize_site_url, validate_credential_ref, validate_ghes_hostname,
    validate_tracker_settings, validate_tracker_transport, Secret, TrackerConfig,
    TrackerCredential, TrackerRow, TrackerSettings, TrackerViewRow, WriteBack, TRACKER_AUTH_KINDS,
    TRACKER_PROVIDERS, TRACKER_STATES,
};
pub use update::{
    UpdateDesiredRow, UpdateDocRow, UpdateEventRow, UpdateObservedRow, UpdateOrgPolicyRow,
    UpdateRolloutRow, UPDATE_EVENT_RETENTION_SECS,
};
pub use work::{
    canonical_key, github_ref, normalize_work_ref, primary_conflict, split_github_repo, Decider,
    WorkItemRow, WorkLinkRow, WorkSummary, WorkTarget, PERSON_SOURCES, WORK_LINK_SOURCES,
};
pub use work_buckets::{
    bucket_states, BucketMemberRow, BucketMembership, BucketPatch, BucketRefRow, BucketRow,
    ItemBucketRow, NewBucket, SprintClosed, BUCKET_KINDS,
};
pub use work_comments::{validate_comment, CommentRow, COMMENTS_SERVED_MAX, COMMENT_MAX_CHARS};
pub use work_detect::{
    DetectionState, WITHDRAWN_CARRIED, WITHDRAWN_DECAY, WITHDRAWN_REASONS, WITHDRAWN_WITHDRAW,
    WORK_SUGGESTION_WITHDRAWN,
};
pub use work_journal::StepView;
pub use work_journal::{
    JournalRow, COMPACT_SUMMARY_CAP, COMPACT_SUMMARY_MAX_CHARS, JOURNAL_KINDS, PROGRESS_CAP,
};
pub use work_local::{validate_local_work_title, LocalItemLink, LOCAL_WORK_TITLE_MAX_CHARS};
pub use work_retention::{retention_cutoff, RetentionTable, WORK_EVENT_KINDS};
pub use work_status::STATUS_CATEGORIES;
pub use work_tasks::{
    is_epic, job_status, parse_due_date, validate_assignees, validate_due_date, ItemEdit,
    NativeItem, Proposal, TreeEntry, TreeRef, ACCEPT_UNDO_SECS, EPIC_KIND, LOCAL_DEPTH_MAX,
    PROPOSALS_OPEN_CAP, TASK_KEY_PREFIX,
};
pub use work_tidy::ReopenedWork;
pub use work_usage::{DetectionCounts, JournalCounts};
pub use work_view::{
    version_conflict, Placement, RuleConditions, ViewItem, ViewLink, WorkRule, WorkView,
};

/// One number per `Store` ever built in this process, never reused — see
/// [`Store::instance_id`].
static NEXT_INSTANCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_instance() -> u64 {
    NEXT_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub struct Store {
    conn: Connection,
    bus: StoreBus,
    /// This store's place in a process-wide registry keyed per store (the
    /// work resume's in-flight keys): monotonic, so a store built where a
    /// dropped one was never inherits its entries the way an address would.
    instance: u64,
    /// In-memory record of the sessions fleet itself killed, so a reconcile
    /// pass that probed before a kill cannot re-insert its row once the kill
    /// has reaped it. Process-local on purpose — see [`reconcile::KillMemory`].
    kills: reconcile::KillMemory,
    /// Signalled after a `session_messages` insert commits, so a waiter wakes
    /// on arrival instead of polling.
    ///
    /// Deliberately NOT the event bus: only the hub builds a subscribable bus
    /// (`BroadcastEventBus`), while the desktop's bus forwards to Svelte and
    /// hands out no receiver. A `Notify` on the store works in every
    /// embedding and needs no new `RowChange` variant, so no contract golden
    /// or `events.ts` allowlist entry moves.
    message_notify: Arc<tokio::sync::Notify>,
    /// Signalled after a `form_requests` change, so `ask`'s wait wakes on an
    /// answer instead of polling. Not the event bus, for `message_notify`'s
    /// reasons.
    form_notify: Arc<tokio::sync::Notify>,
    /// Per hub link, the generation of its latest `peer_exchange` — so a
    /// parked listener handler a newer exchange superseded can return at
    /// once. Process-local on purpose, like `kills`: it only has to outlive
    /// a long-poll, and a restart has no parked handlers to release.
    peer_generations: std::sync::Mutex<std::collections::HashMap<i64, u64>>,
}

/// The store's handle on its [`EventBus`]. Normally a pass-through; inside
/// [`Store::atomically`] it holds every emit and releases them only after the
/// transaction commits (dropped on rollback), so no event announces a write
/// that never persisted.
struct StoreBus {
    inner: Arc<dyn EventBus>,
    held: std::sync::Mutex<Option<Vec<RowChange>>>,
    /// Frames delivered to `inner` so far (work graph M11.4's sync metrics
    /// count a pass's frames as the difference). Process-local, never reset.
    delivered: std::sync::atomic::AtomicU64,
}

impl StoreBus {
    fn new(inner: Arc<dyn EventBus>) -> Self {
        Self {
            inner,
            held: std::sync::Mutex::new(None),
            delivered: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Hand one frame to the real bus, counted.
    fn deliver(&self, e: &RowChange) {
        self.delivered
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.emit(e);
    }

    /// Start holding emits. Returns false when already holding (nested).
    fn hold(&self) -> bool {
        let mut held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        if held.is_some() {
            return false;
        }
        *held = Some(Vec::new());
        true
    }

    /// Whether emits are being held — i.e. a [`Store::atomically`]
    /// transaction is running.
    fn is_holding(&self) -> bool {
        self.held
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    /// Stop holding; the held events, in emit order.
    fn release(&self) -> Vec<RowChange> {
        self.held
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .unwrap_or_default()
    }

    /// How many events are held right now — `0` when not holding. A caller
    /// about to run a nested, individually-abortable scope (a tracker
    /// sync's per-item `Store::in_savepoint`) takes this first and hands it
    /// to [`StoreBus::discard_since`] if that scope fails, so an event the
    /// scope queued does not survive the write it announced rolling back.
    fn checkpoint(&self) -> usize {
        self.held
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map_or(0, Vec::len)
    }

    /// Drop every held event queued since `mark` ([`StoreBus::checkpoint`],
    /// taken before the scope that queued them). Not holding: a no-op,
    /// there is nothing queued to drop.
    fn discard_since(&self, mark: usize) {
        if let Some(held) = self.held.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            let mark = mark.min(held.len());
            held.truncate(mark);
        }
    }
}

impl EventBus for StoreBus {
    fn emit(&self, e: &RowChange) {
        if let Some(held) = self.held.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            held.push(e.clone());
            return;
        }
        self.deliver(e);
    }

    /// Straight through, never held: a threshold, not a row change, and a
    /// setting write does not roll back with an enclosing transaction's
    /// events.
    fn context_red_pct_changed(&self, pct: f64) {
        self.inner.context_red_pct_changed(pct);
    }
}

/// The error a lost transaction surfaces as: `E_SQLITE` once converted,
/// the code the failed `COMMIT` used to produce.
fn lost_transaction() -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ABORT),
        Some(
            "the transaction was rolled back by SQLite before it could commit; \
             none of its writes were kept"
                .to_string(),
        ),
    )
}

/// How long a statement waits on another connection's lock (a `fleet-hub`
/// CLI beside the daemon) before it fails with `SQLITE_BUSY`.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Journal settings for the long-lived file store, applied before migrating.
///
/// The default rollback journal with `synchronous = FULL` fsyncs several
/// times per commit; on the hub's HDD-backed NAS volume that measured ~200 ms
/// per autocommit write, all of it under the store mutex every reader waits
/// on. WAL with `synchronous = NORMAL` commits without an fsync (~0.1 ms
/// there): a power cut can lose the last commits, never corrupt the file.
/// WAL is persistent in the file, so the read-only CLI opens inherit it.
///
/// A filesystem without shared-memory support refuses WAL and SQLite keeps
/// the old mode; `synchronous` then stays at its safe default, since NORMAL
/// under a rollback journal can corrupt on power loss.
fn tune_file_connection(conn: &Connection) -> Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    if mode.eq_ignore_ascii_case("wal") {
        conn.execute_batch("PRAGMA synchronous = NORMAL;")?;
    } else {
        tracing::warn!(
            mode,
            "[store] WAL refused; the store stays on its journal mode"
        );
    }
    Ok(())
}

/// `chmod 600` one file, best-effort: a missing file is fine (a sidecar
/// SQLite has not created yet), any other failure is logged, never fatal.
/// No-op off unix.
fn set_owner_only(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                path = %path.display(),
                error = %e,
                "[store] chmod 600 failed; the file may be readable by other users"
            ),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// The WAL sidecars SQLite keeps beside `path` while a connection is open.
fn sidecar_paths(path: &std::path::Path) -> [std::path::PathBuf; 2] {
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    let mut shm = path.as_os_str().to_owned();
    shm.push("-shm");
    [wal.into(), shm.into()]
}

/// Make `path` owner-only BEFORE SQLite opens it.
///
/// The database holds bearer tokens (the MCP master token, client and peer
/// link tokens, tracker secrets) in plaintext, and in WAL mode every recent
/// commit lives in `<path>-wal` until a checkpoint. SQLite creates `-wal`
/// and `-shm` with the main file's mode at that moment and never re-chmods
/// them, so a chmod after the open (what the callers used to do) leaves the
/// sidecars at the umask-derived mode for the life of the process — and
/// across a crash, since a leftover `-wal` is reused. Creating the file 0600
/// here means the sidecars inherit 0600; an existing file is tightened first
/// for the same reason.
fn restrict_before_open(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if path.exists() {
            set_owner_only(path);
            return;
        }
        // An empty file is a valid (empty) SQLite database. `create_new`: a
        // file that appeared in between is an existing database, and the
        // open below reads it as such.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
        {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => set_owner_only(path),
            Err(e) => tracing::warn!(
                path = %path.display(),
                error = %e,
                "[store] could not pre-create the database owner-only"
            ),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// A fresh in-memory database holding a copy of one that every migration ran
/// on once per test process (see [`Store::open_in_memory`]). The template is
/// built by the real `migrate()`, so it is exactly what a migrated store
/// holds; the copy is SQLite's online backup, a page copy that takes well
/// under a millisecond.
#[cfg(test)]
fn migrated_template_copy() -> Result<Connection> {
    use std::sync::{Mutex, OnceLock};
    static TEMPLATE: OnceLock<Mutex<Connection>> = OnceLock::new();
    let template = TEMPLATE.get_or_init(|| {
        let store = Store {
            conn: Connection::open_in_memory().expect("open the template database"),
            bus: StoreBus::new(Arc::new(NoopEventBus)),
            kills: Default::default(),
            message_notify: Arc::new(tokio::sync::Notify::new()),
            form_notify: Arc::new(tokio::sync::Notify::new()),
            instance: next_instance(),
            peer_generations: Default::default(),
        };
        store.migrate().expect("migrate the template database");
        let Store { conn, .. } = store;
        Mutex::new(conn)
    });
    let template = template
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut copy = Connection::open_in_memory()?;
    rusqlite::backup::Backup::new(&template, &mut copy)?.run_to_completion(
        i32::MAX,
        std::time::Duration::ZERO,
        None,
    )?;
    Ok(copy)
}

impl Store {
    pub fn open_with_bus(path: &std::path::Path, bus: Arc<dyn EventBus>) -> Result<Self> {
        restrict_before_open(path);
        let conn = Connection::open(path)?;
        tune_file_connection(&conn)?;
        // A database that already existed with a wider mode may have left a
        // `-wal` / `-shm` behind (a crash, an older build) that SQLite reuses
        // as they are; tighten them too, now that WAL is on.
        for sidecar in sidecar_paths(path) {
            set_owner_only(&sidecar);
        }
        let store = Self {
            conn,
            bus: StoreBus::new(bus),
            kills: Default::default(),
            message_notify: Arc::new(tokio::sync::Notify::new()),
            form_notify: Arc::new(tokio::sync::Notify::new()),
            instance: next_instance(),
            peer_generations: Default::default(),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Open an EXISTING database read-only and **without migrating it**.
    ///
    /// For a one-shot reader that runs beside a live daemon — `fleet-hub
    /// pair`, `fleet-hub client …`, `fleet-hub token show` — where
    /// [`Store::open_with_bus`] would run *this binary's* migrations against
    /// the database the daemon has open. A CLI newer than the running daemon
    /// must not reshape the schema under it, so this open cannot: the
    /// connection is `SQLITE_OPEN_READ_ONLY` and nothing is applied.
    ///
    /// Only reads are valid on the result; a write returns SQLite's
    /// "attempt to write a readonly database".
    ///
    /// `SQLITE_OPEN_NO_MUTEX` (SQLite's multi-thread mode: no mutex around
    /// the connection itself) is safe here ONLY because of what these callers
    /// are — one-shot CLI subcommands that open the file, read a couple of
    /// `settings` rows on one thread, and exit. Nothing shares this
    /// connection between threads. A `Store` opened this way must therefore
    /// not be handed to the server, the event bus, or anything else that
    /// would use it concurrently; the long-lived paths go through
    /// [`Store::open_with_bus`], whose `Store` lives behind a
    /// `std::sync::Mutex` (see the crate's store conventions).
    pub fn open_read_only(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Ok(Self {
            conn,
            bus: StoreBus::new(Arc::new(crate::events::NoopEventBus)),
            kills: Default::default(),
            message_notify: Arc::new(tokio::sync::Notify::new()),
            form_notify: Arc::new(tokio::sync::Notify::new()),
            instance: next_instance(),
            peer_generations: Default::default(),
        })
    }

    /// What the fleet knows beyond a session's own row — down hosts,
    /// accounts at a limit or without a login — as this store's bus follows
    /// it (step 2.6, [`EventBus::attention_facts`]). Empty off the hub, which
    /// is the row-only classification. Every hub site that decides
    /// `needs_attention` reads it here, beside `health.context_red_pct`.
    pub fn attention_facts(&self) -> crate::service::attention::Facts {
        self.bus.inner.attention_facts()
    }

    /// Each account's latest usage answer as this store's bus follows it
    /// ([`EventBus::account_usage`]). Empty off the hub.
    pub fn bus_account_usage(&self) -> Vec<crate::service::account_usage::AccountUsageSnapshot> {
        self.bus.inner.account_usage()
    }

    /// An in-memory store for a test. Its database is a copy of one that went
    /// through every migration once per test process
    /// ([`migrated_template_copy`]); `migrate()` then runs on the copy as
    /// usual, where it only re-runs the idempotent bootstrap and repairs.
    /// Replaying all migrations per store cost ~90 ms, about a thousand times
    /// per run (RUST-BUILD-PERFORMANCE-AUDIT.md, Appendix D). A test about
    /// the migrations themselves builds its database another way
    /// (`store::testgen`, `migrations_through`).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::open_with_bus_in_memory(Arc::new(NoopEventBus))
    }

    /// [`Store::open_in_memory`] with the caller's event bus.
    #[cfg(test)]
    pub fn open_with_bus_in_memory(bus: Arc<dyn EventBus>) -> Result<Self> {
        let conn = migrated_template_copy()?;
        let store = Self {
            conn,
            bus: StoreBus::new(bus),
            kills: Default::default(),
            message_notify: Arc::new(tokio::sync::Notify::new()),
            form_notify: Arc::new(tokio::sync::Notify::new()),
            instance: next_instance(),
            peer_generations: Default::default(),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Test-only: a store of its own holding a copy of this one's database,
    /// as [`Store::open_with_bus_in_memory`] copies the migrated template.
    /// For a fixture that is built once and read by tests that must not
    /// queue on one connection (`service::work::scale_tests`).
    #[cfg(test)]
    pub(crate) fn copy_for_test(&self) -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        rusqlite::backup::Backup::new(&self.conn, &mut conn)?.run_to_completion(
            i32::MAX,
            std::time::Duration::ZERO,
            None,
        )?;
        let store = Self {
            conn,
            bus: StoreBus::new(Arc::new(NoopEventBus)),
            kills: Default::default(),
            message_notify: Arc::new(tokio::sync::Notify::new()),
            form_notify: Arc::new(tokio::sync::Notify::new()),
            instance: next_instance(),
            peer_generations: Default::default(),
        };
        // Nothing left to migrate; this sets the connection's own pragmas
        // (`foreign_keys`), which a backup does not carry.
        store.migrate()?;
        Ok(store)
    }

    /// A number no other `Store` of this process has or will have — the key
    /// for a process-wide, per-store registry. An address is not one: a
    /// store dropped and another built where it was would alias.
    pub fn instance_id(&self) -> u64 {
        self.instance
    }

    /// The raw connection, for a test outside `store` that has to set up a
    /// state no public method writes (a claude_session_id, say).
    #[cfg(test)]
    pub(crate) fn conn_for_test(&self) -> &Connection {
        &self.conn
    }

    /// Make every read of a `table` row matching the SQL condition `when`
    /// fail with a real `SQLITE_IOERR`, which SQLite answers by rolling the
    /// WHOLE transaction back even for a read-only statement (`IOERR` is a
    /// "special error" to `sqlite3VdbeHalt`). A TEMP view named like the
    /// table shadows it for every unqualified name, so writes to `table`
    /// fail too: arm only a table the code under test merely reads.
    #[cfg(test)]
    pub(crate) fn arm_read_ioerr_for_test(&self, table: &str, when: &str) {
        self.conn
            .create_scalar_function(
                "fleet_test_ioerr",
                0,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                |_| -> rusqlite::Result<i64> {
                    // No message: `sqlite3_result_error` would turn the code
                    // into a plain SQLITE_ERROR.
                    Err(rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
                        None,
                    ))
                },
            )
            .unwrap();
        self.conn
            .execute_batch(&format!(
                "CREATE TEMP VIEW {table} AS SELECT * FROM main.{table} \
                 WHERE CASE WHEN {when} THEN fleet_test_ioerr() ELSE 1 END"
            ))
            .unwrap();
    }

    #[cfg(test)]
    pub fn has_table(&self, name: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [name],
            |row| row.get(0),
        )?;
        Ok(count == 1)
    }

    pub fn schema_version(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
    }

    /// Read a value from the key/value `settings` table. `None` if absent.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                rusqlite::params![key],
                |row| row.get(0),
            )
            .optional()
    }

    /// Insert or replace a value in the `settings` table.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    /// Tell the bus the `health.context_red_pct` in force is now `pct`
    /// ([`EventBus::context_red_pct_changed`]). `service::settings::set`
    /// calls it on every write of that setting.
    pub fn context_red_pct_changed(&self, pct: f64) {
        self.bus.context_red_pct_changed(pct);
    }

    /// Emit `settings:changed` for `key` (declarative pages P3). Called by
    /// `service::settings::set` after a validated write, not by
    /// [`Self::set_setting`]: most rows in this table are internal state
    /// nobody renders.
    pub fn emit_settings_changed(&self, key: &str) {
        self.bus.emit(&RowChange::SettingsChanged(key.to_string()));
    }

    /// Forget a key, so the next `get_setting` answers `None` and its reader
    /// falls back to its own default. Absent and "stored as the default" are
    /// not the same thing: the second pins today's default forever (see
    /// `service::quick_replies::replace`). A key that was never there is not
    /// an error.
    pub fn delete_setting(&self, key: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM settings WHERE key=?1", rusqlite::params![key])?;
        Ok(())
    }

    /// Record which session is the fleet controller (the calling session that
    /// must not kill/recreate/restart itself without `force`). Stored as two
    /// keys in the `settings` table.
    pub fn set_controller(&self, host: &str, tmux_name: &str) -> Result<()> {
        self.set_setting("controller.host", host)?;
        self.set_setting("controller.tmux", tmux_name)?;
        Ok(())
    }

    /// Read the registered controller as `(host, tmux_name)`. `None` unless
    /// both keys are present.
    pub fn get_controller(&self) -> Result<Option<(String, String)>> {
        let host = self.get_setting("controller.host")?;
        let tmux = self.get_setting("controller.tmux")?;
        Ok(host.zip(tmux))
    }

    pub fn conn_ref(&self) -> &rusqlite::Connection {
        &self.conn
    }

    /// The store's message-arrival `Notify`, so a waiter can hold the handle
    /// across an `.await` without holding the store lock. See the field doc
    /// on `Store::message_notify` for why this is a `Notify` and not an
    /// event-bus subscription.
    pub fn message_notify(&self) -> Arc<tokio::sync::Notify> {
        self.message_notify.clone()
    }

    /// Run `f` under `SAVEPOINT <name>`: released when it returns `Ok`,
    /// rolled back to (then released) when it returns `Err`. Works both
    /// standalone — the savepoint is then the transaction and its RELEASE
    /// commits — and nested inside [`Store::atomically`] or another
    /// savepoint, where it never issues a second `BEGIN`. A write helper that
    /// may run inside a caller's transaction uses this instead of
    /// `unchecked_transaction()`, which cannot nest, and instead of a
    /// hand-written `SAVEPOINT` block. `name` must be a plain SQL identifier
    /// (it is spliced into the statement).
    ///
    /// Standalone, no failure leaves a transaction open: a RELEASE (= the
    /// COMMIT) that SQLite refuses without rolling back (`SQLITE_BUSY`, a
    /// deferred foreign key) is undone, and when the undo's own `ROLLBACK
    /// TO` fails too, whatever is still open is rolled back — otherwise every
    /// later autocommit write on the writer would join a transaction nobody
    /// commits. Nested, a failure only ever rolls back to the savepoint; the
    /// caller's transaction is the caller's.
    ///
    /// Inside [`Store::atomically`] it refuses to open (`Err`, no SAVEPOINT
    /// issued) once SQLite has rolled that transaction back — even at a READ
    /// whose error the caller swallowed (`IOERR` / `NOMEM` roll back a
    /// read-only statement's transaction too). In autocommit the SAVEPOINT
    /// would start a fresh transaction and its RELEASE commit the helper's
    /// writes on their own, outside the transaction they belonged to.
    pub(super) fn in_savepoint<R, E>(
        &self,
        name: &'static str,
        f: impl FnOnce(&Connection) -> std::result::Result<R, E>,
    ) -> std::result::Result<R, E>
    where
        E: From<rusqlite::Error>,
    {
        if self.transaction_lost() {
            return Err(lost_transaction().into());
        }
        let was_autocommit = self.conn.is_autocommit();
        self.conn.execute_batch(&format!("SAVEPOINT {name}"))?;
        let undo = |conn: &Connection| {
            // Best-effort: the caller already carries the real error.
            let _ = conn.execute_batch(&format!("ROLLBACK TO {name}; RELEASE {name}"));
            if was_autocommit && !conn.is_autocommit() {
                let _ = conn.execute_batch("ROLLBACK");
            }
        };
        match f(&self.conn) {
            Ok(r) => match self.conn.execute_batch(&format!("RELEASE {name}")) {
                Ok(()) => Ok(r),
                Err(e) => {
                    // A RELEASE that fails would otherwise leave the
                    // savepoint (standalone: the transaction) open.
                    undo(&self.conn);
                    Err(e.into())
                }
            },
            Err(e) => {
                undo(&self.conn);
                Err(e)
            }
        }
    }

    /// A `BEGIN IMMEDIATE` transaction, for code that reads and then writes.
    ///
    /// A DEFERRED transaction that has read and then writes asks SQLite to
    /// upgrade a read lock, and SQLite never runs the busy handler for that
    /// upgrade: while another connection (the running hub, beside a
    /// `fleet-hub guides …` CLI) holds the write lock, the write fails at
    /// once with "database is locked", and a commit in between makes it
    /// `SQLITE_BUSY_SNAPSHOT`. IMMEDIATE takes the write lock up front,
    /// waiting the busy timeout for it; WAL readers never block it.
    pub(crate) fn immediate_transaction(&self) -> Result<rusqlite::Transaction<'_>> {
        rusqlite::Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
    }

    /// Run `f` inside a single `conn.transaction()`. Test-only since the
    /// reconcile write burst moved onto [`Store::in_savepoint`] (it now runs
    /// inside the per-host transaction and so cannot open its own).
    #[cfg(test)]
    fn with_transaction<F, R>(&mut self, f: F) -> rusqlite::Result<R>
    where
        F: FnOnce(&rusqlite::Transaction) -> rusqlite::Result<R>,
    {
        let tx = self.conn.transaction()?;
        let r = f(&tx)?;
        tx.commit()?;
        Ok(r)
    }

    /// Run `f` against this store inside one SQLite transaction: commit when
    /// `f` returns `Ok`, roll back (drop the transaction) when it returns
    /// `Err`. The transaction is `BEGIN IMMEDIATE`: callers read then write,
    /// and a DEFERRED one would turn another connection's commit in between
    /// (a `fleet-hub` CLI) into `SQLITE_BUSY_SNAPSHOT`, which no busy wait
    /// retries. IMMEDIATE takes the write lock up front and waits the busy
    /// timeout for it instead; WAL readers (the hub's read pool) never block
    /// it.
    ///
    /// SQLite answers some errors (`SQLITE_FULL`, `IOERR`, `NOMEM`, a
    /// trigger's `RAISE(ROLLBACK)`) by rolling the WHOLE transaction back.
    /// If `f` swallowed such an error, every later statement would commit on
    /// its own; so a best-effort arm inside `f` calls
    /// [`Store::ensure_in_tx`] before going on, and an `f` that still returns
    /// `Ok` over a lost transaction is an `Err` here, its held events
    /// dropped. Unlike [`Store::with_transaction`] the closure receives the
    /// `&Store` itself, so it can compose the ordinary `&self` write helpers
    /// (`insert_message`, `insert_session_event`, …) atomically without
    /// `_in_tx` twins. Must not be nested, and `f` must not call a helper
    /// that opens its own transaction (`BEGIN` inside `BEGIN` errors). Bus
    /// events the helpers emit (`session:event`, …) are held and flushed only
    /// after COMMIT; a rollback drops them.
    pub fn atomically<F, R>(&self, f: F) -> Result<R, crate::ipc_error::IpcError>
    where
        F: FnOnce(&Store) -> Result<R, crate::ipc_error::IpcError>,
    {
        /// Stops holding even when `f` panics, so later emits are not
        /// swallowed; the held events are dropped with the rollback.
        struct Unhold<'a>(&'a StoreBus);
        impl Drop for Unhold<'_> {
            fn drop(&mut self) {
                self.0.release();
            }
        }
        let tx = rusqlite::Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let unhold = self.bus.hold().then(|| Unhold(&self.bus));
        let result = f(self).and_then(|r| {
            self.ensure_in_tx()?;
            tx.commit()?;
            Ok(r)
        });
        if unhold.is_some() {
            let held = self.bus.release();
            if result.is_ok() {
                for e in &held {
                    self.bus.deliver(e);
                }
            }
        }
        result
    }

    /// Inside [`Store::atomically`]: `Err` once SQLite has rolled the
    /// transaction back under it (the connection is back in autocommit), so
    /// a best-effort arm that just swallowed an error stops the closure
    /// instead of letting the writes after it commit one by one. Outside
    /// `atomically` there is no transaction to lose and it is always `Ok`,
    /// so a helper shared by both paths may call it unconditionally.
    ///
    /// A best-effort READ needs it as much as a write: SQLite rolls the
    /// whole transaction back on an `IOERR` / `NOMEM` answer to a SELECT
    /// too, so a swallowed read (`.ok()`, `unwrap_or_default()`) followed
    /// by plain writes calls it right after the read.
    pub fn ensure_in_tx(&self) -> Result<(), crate::ipc_error::IpcError> {
        if self.transaction_lost() {
            return Err(lost_transaction().into());
        }
        Ok(())
    }

    /// Inside an [`Store::in_savepoint`] body: `Err` once SQLite has rolled
    /// the savepoint back (with whatever transaction encloses it). A body
    /// never legitimately runs in autocommit, standalone or nested, so a
    /// swallowed error followed by more of the body's writes checks this —
    /// [`Store::ensure_in_tx`] only knows about `atomically`, and a
    /// standalone savepoint's writes would otherwise commit one by one.
    pub(super) fn ensure_in_savepoint(&self) -> Result<(), crate::ipc_error::IpcError> {
        if self.conn.is_autocommit() {
            return Err(lost_transaction().into());
        }
        Ok(())
    }

    /// An [`Store::atomically`] transaction is running (the bus is holding
    /// its events) but SQLite has already rolled it back under it.
    fn transaction_lost(&self) -> bool {
        self.bus.is_holding() && self.conn.is_autocommit()
    }

    /// Frames this store has handed to its event bus since it was opened
    /// (a held frame counts once it is released; one a rollback dropped
    /// never counts). Callers measure a span of work as the difference.
    pub fn frames_emitted(&self) -> u64 {
        self.bus
            .delivered
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_set_get_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.get_controller().unwrap(), None, "unset is None");
        s.set_controller("mac", "dev-fleet").unwrap();
        assert_eq!(
            s.get_controller().unwrap(),
            Some(("mac".to_string(), "dev-fleet".to_string()))
        );
        // overwrite
        s.set_controller("mefistos", "ctrl").unwrap();
        assert_eq!(
            s.get_controller().unwrap(),
            Some(("mefistos".to_string(), "ctrl".to_string()))
        );
    }

    #[test]
    fn with_transaction_commits_on_ok() {
        let mut store = Store::open_in_memory().expect("in-memory store");
        let r: rusqlite::Result<()> = store.with_transaction(|tx| {
            tx.execute(
                "INSERT INTO hosts (alias, ssh_alias, hidden) VALUES (?1, ?2, 0)",
                rusqlite::params!["foo", "foo-ssh"],
            )?;
            Ok(())
        });
        assert!(r.is_ok());
        let hosts = store.list_hosts().expect("list");
        assert!(hosts.iter().any(|h| h.alias == "foo"));
    }

    #[test]
    fn with_transaction_rolls_back_on_err() {
        let mut store = Store::open_in_memory().expect("in-memory store");
        let r: rusqlite::Result<()> = store.with_transaction(|tx| {
            tx.execute(
                "INSERT INTO hosts (alias, ssh_alias, hidden) VALUES (?1, ?2, 0)",
                rusqlite::params!["bar", "bar-ssh"],
            )?;
            // Trigger an error to force rollback.
            Err(rusqlite::Error::QueryReturnedNoRows)
        });
        assert!(r.is_err());
        let hosts = store.list_hosts().expect("list");
        assert!(
            !hosts.iter().any(|h| h.alias == "bar"),
            "rollback should have removed the bar row"
        );
    }

    #[test]
    fn store_holds_event_bus_field_and_default_is_noop() {
        use crate::events::NoopEventBus;
        let store = Store::open_in_memory().expect("store");
        // Just constructing the store with the default Noop bus exercises the
        // new field. The bus is a private implementation detail; we don't expose
        // it as a public getter, so this test is intentionally minimal.
        let _ = std::sync::Arc::new(NoopEventBus); // also exercises Send+Sync
        let _ = store; // touch it to keep it alive past the new
    }

    fn pragma<T: rusqlite::types::FromSql>(s: &Store, name: &str) -> T {
        s.conn
            .query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
            .unwrap()
    }

    /// On the hub's HDD-backed NAS volume a rollback-journal commit cost
    /// ~200 ms of fsync while holding the store mutex; WAL costs ~0.1 ms.
    #[test]
    fn file_store_opens_in_wal_with_normal_sync_and_a_busy_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open_with_bus(&dir.path().join("state.db"), Arc::new(NoopEventBus)).unwrap();
        assert_eq!(pragma::<String>(&s, "journal_mode"), "wal");
        // 1 = NORMAL: in WAL mode a commit does not fsync; a power cut can
        // lose the last commits but never corrupts the database.
        assert_eq!(pragma::<i64>(&s, "synchronous"), 1);
        assert!(pragma::<i64>(&s, "busy_timeout") >= 1000);
    }

    /// A `fleet-hub guides …` / `settings …` CLI opens `state.db` with
    /// `open_with_bus` while the hub runs and writes. A migrate transaction
    /// that reads before it writes got SQLITE_BUSY at once whenever the hub
    /// held the write lock at that moment: SQLite never runs the busy
    /// handler for a read→write upgrade. A writer that keeps taking the lock
    /// makes that moment likely; every open must still wait it out.
    #[test]
    fn opens_wait_out_another_connections_writes() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        drop(Store::open_with_bus(&db, Arc::new(NoopEventBus)).unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (db, stop) = (db.clone(), stop.clone());
            std::thread::spawn(move || {
                let conn = Connection::open(&db).unwrap();
                conn.busy_timeout(BUSY_TIMEOUT).unwrap();
                while !stop.load(Ordering::Relaxed) {
                    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    conn.execute_batch("COMMIT").unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            })
        };
        let failed: Vec<String> = (0..40)
            .filter_map(|_| Store::open_with_bus(&db, Arc::new(NoopEventBus)).err())
            .map(|e| e.to_string())
            .collect();
        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();
        assert!(
            failed.is_empty(),
            "{} of 40 opens failed: {failed:?}",
            failed.len()
        );
    }

    /// The bearer tokens live in `state.db-wal` until a checkpoint, and
    /// SQLite gives the sidecars the main file's mode when it creates them:
    /// the main file must be 0600 BEFORE the open, not chmodded after it.
    #[cfg(unix)]
    #[test]
    fn file_store_and_its_wal_sidecars_are_owner_only_while_open() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let wal = dir.path().join("state.db-wal");
        let shm = dir.path().join("state.db-shm");

        // A fresh database: created 0600, so the sidecars inherit 0600.
        let s = Store::open_with_bus(&db, Arc::new(NoopEventBus)).unwrap();
        s.set_controller("mac", "dev-fleet").unwrap();
        assert!(
            wal.exists() && shm.exists(),
            "WAL sidecars exist while open"
        );
        assert_eq!(mode(&db), 0o600, "state.db");
        assert_eq!(mode(&wal), 0o600, "state.db-wal");
        assert_eq!(mode(&shm), 0o600, "state.db-shm");

        // A leftover sidecar with a wider mode (a crash under an older build)
        // is tightened by the next open even though SQLite reuses it.
        std::fs::set_permissions(&wal, std::fs::Permissions::from_mode(0o644)).unwrap();
        let again = Store::open_with_bus(&db, Arc::new(NoopEventBus)).unwrap();
        assert_eq!(mode(&wal), 0o600, "leftover state.db-wal");
        drop(again);
        drop(s);

        // A database that already existed with a wider mode is tightened
        // before the open, so its new sidecars are 0600 too.
        assert!(!wal.exists(), "the WAL is gone after the last close");
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).unwrap();
        let s = Store::open_with_bus(&db, Arc::new(NoopEventBus)).unwrap();
        s.set_controller("mac", "dev-fleet").unwrap();
        assert_eq!(mode(&db), 0o600, "existing state.db");
        assert_eq!(mode(&wal), 0o600, "existing database's state.db-wal");
        assert_eq!(mode(&shm), 0o600, "existing database's state.db-shm");
    }

    /// `fleet-hub pair` / `token show` read beside a live daemon. In WAL a
    /// commit sits in `state.db-wal` until a checkpoint, and the read-only
    /// open must still see it — and must work again once the daemon is gone.
    #[test]
    fn read_only_open_sees_uncheckpointed_writes_and_opens_after_close() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let live = Store::open_with_bus(&db, Arc::new(NoopEventBus)).unwrap();
        live.set_controller("mac", "dev-fleet").unwrap();
        assert!(
            dir.path().join("state.db-wal").exists(),
            "write went to the WAL"
        );

        let ro = Store::open_read_only(&db).unwrap();
        assert_eq!(
            ro.get_controller().unwrap(),
            Some(("mac".to_string(), "dev-fleet".to_string()))
        );
        drop(ro);
        drop(live);

        let ro = Store::open_read_only(&db).unwrap();
        assert_eq!(
            ro.get_controller().unwrap(),
            Some(("mac".to_string(), "dev-fleet".to_string()))
        );
    }

    // ── I2: a SAVEPOINT that cannot finish never strands a transaction ──

    fn setting(s: &Store, key: &str) -> Option<String> {
        s.get_setting(key).unwrap()
    }

    /// A top-level savepoint's RELEASE is the COMMIT. When it is refused
    /// (here: a deferred FK violation, which SQLite does NOT roll back) the
    /// helper must still leave the connection in autocommit with nothing of
    /// the body written.
    #[test]
    fn in_savepoint_top_level_release_failure_leaves_autocommit() {
        let s = Store::open_in_memory().unwrap();
        test_support::arm_commit_failure(&s, "INSERT ON settings");
        let r: rusqlite::Result<()> = s.in_savepoint("sp_release", |c| {
            c.execute("INSERT INTO settings (key, value) VALUES ('k', 'v')", [])?;
            Ok(())
        });
        assert!(r.is_err(), "the refused RELEASE surfaces");
        assert!(s.conn.is_autocommit(), "no transaction is left open");
        s.conn
            .execute_batch("DROP TRIGGER temp.arm_commit_failure")
            .unwrap();
        assert_eq!(setting(&s, "k"), None, "the body's write is gone");
    }

    /// When the undo's own `ROLLBACK TO` fails (the savepoint is gone) at top
    /// level, whatever transaction is open is rolled back rather than left
    /// open for the next autocommit write to join.
    #[test]
    fn in_savepoint_top_level_rollback_to_failure_leaves_autocommit() {
        let s = Store::open_in_memory().unwrap();
        let r: rusqlite::Result<()> = s.in_savepoint("sp_gone", |c| {
            c.execute_batch(
                "RELEASE sp_gone; BEGIN; \
                 INSERT INTO settings (key, value) VALUES ('k', 'v');",
            )?;
            Err(rusqlite::Error::QueryReturnedNoRows)
        });
        assert!(r.is_err());
        assert!(
            s.conn.is_autocommit(),
            "a failed ROLLBACK TO must not leave a transaction open"
        );
        assert_eq!(setting(&s, "k"), None);
    }

    /// Nested inside `atomically`, a failing savepoint rolls back only to
    /// itself: the outer transaction's writes before and after it commit.
    #[test]
    fn in_savepoint_nested_failure_rolls_back_only_the_savepoint() {
        let s = Store::open_in_memory().unwrap();
        s.atomically(|s| {
            s.set_setting("before", "1")?;
            let r: rusqlite::Result<()> = s.in_savepoint("sp_inner", |c| {
                c.execute(
                    "INSERT INTO settings (key, value) VALUES ('inner', '1')",
                    [],
                )?;
                Err(rusqlite::Error::QueryReturnedNoRows)
            });
            assert!(r.is_err());
            assert!(!s.conn.is_autocommit(), "the outer transaction survives");
            s.set_setting("after", "1")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(setting(&s, "before").as_deref(), Some("1"));
        assert_eq!(setting(&s, "inner"), None);
        assert_eq!(setting(&s, "after").as_deref(), Some("1"));
    }

    /// The four store helpers that used to hand-write their SAVEPOINT
    /// (`insert_session_event`, `rebind_conversation`, `append_journal`,
    /// `apply_link_changes`): called standalone, a refused RELEASE must leave
    /// the writer in autocommit and none of their writes behind.
    #[test]
    fn savepoint_helpers_never_strand_a_transaction_on_a_refused_release() {
        use crate::service::work::resolve::LinkChange;
        let fresh = || {
            let s = Store::open_in_memory().unwrap();
            s.upsert_host("local").unwrap();
            let id = s
                .upsert_session("sess", "local", None, None, 0, 0, "running", None)
                .unwrap();
            (s, id)
        };
        let settle = |s: &Store, what: &str| {
            assert!(
                s.conn.is_autocommit(),
                "{what}: the refused RELEASE left a transaction open"
            );
            s.conn
                .execute_batch("DROP TRIGGER temp.arm_commit_failure")
                .unwrap();
        };
        let count =
            |s: &Store, sql: &str| -> i64 { s.conn.query_row(sql, [], |r| r.get(0)).unwrap() };

        let (s, id) = fresh();
        test_support::arm_commit_failure(&s, "INSERT ON session_events");
        assert!(s.insert_session_event(id, "prompt_sent", None).is_err());
        settle(&s, "insert_session_event");
        assert_eq!(count(&s, "SELECT COUNT(*) FROM session_events"), 0);

        let (s, id) = fresh();
        test_support::arm_commit_failure(&s, "INSERT ON conversations");
        assert!(s
            .rebind_conversation(id, "c-1", StartSource::Startup, None, None)
            .is_err());
        settle(&s, "rebind_conversation");
        assert_eq!(count(&s, "SELECT COUNT(*) FROM conversations"), 0);

        let (s, id) = fresh();
        test_support::arm_commit_failure(&s, "INSERT ON work_journal");
        assert!(s
            .journal_for_session(id, "c-1", "progress", "hook", "step one")
            .is_err());
        settle(&s, "append_journal");
        assert_eq!(count(&s, "SELECT COUNT(*) FROM work_journal"), 0);

        let (s, id) = fresh();
        let v0 = count(&s, "SELECT row_version FROM sessions");
        test_support::arm_commit_failure(&s, "UPDATE ON sessions");
        assert!(s
            .apply_link_changes(id, 0, None, &[LinkChange::Withdraw { link_id: 999 }])
            .is_err());
        settle(&s, "apply_link_changes");
        assert_eq!(count(&s, "SELECT row_version FROM sessions"), v0);
    }

    // ── I1: a transaction SQLite rolled back never half-commits ──

    /// A closure that swallowed the error of a statement SQLite answered by
    /// rolling the whole transaction back (a trigger's `RAISE(ROLLBACK)`
    /// stands in for SQLITE_FULL / IOERR / NOMEM) must not come back `Ok`.
    #[test]
    fn atomically_reports_a_transaction_lost_mid_closure() {
        let s = Store::open_in_memory().unwrap();
        s.conn
            .execute_batch(
                "CREATE TEMP TRIGGER lose_tx BEFORE INSERT ON settings \
                 WHEN NEW.key = 'boom' BEGIN SELECT RAISE(ROLLBACK, 'lost'); END;",
            )
            .unwrap();
        let err = s
            .atomically(|s| {
                s.set_setting("a", "1")?;
                // Best-effort write whose error is swallowed.
                let _ = s.set_setting("boom", "1");
                Ok(())
            })
            .unwrap_err();
        assert!(
            err.message.contains("rolled back"),
            "names the lost transaction: {err:?}"
        );
        assert!(s.conn.is_autocommit());
        assert_eq!(setting(&s, "a"), None);
    }

    /// `ensure_in_tx` is how a best-effort arm inside `atomically` notices
    /// the transaction is gone; outside `atomically` it has nothing to guard.
    #[test]
    fn ensure_in_tx_fails_only_inside_a_lost_atomically() {
        let s = Store::open_in_memory().unwrap();
        assert!(
            s.ensure_in_tx().is_ok(),
            "outside atomically: nothing to lose"
        );
        s.atomically(|s| {
            assert!(s.ensure_in_tx().is_ok(), "a live transaction");
            Ok(())
        })
        .unwrap();
        let r = s.atomically(|s| {
            s.conn.execute_batch("ROLLBACK")?;
            s.ensure_in_tx()?;
            s.set_setting("tail", "1")?;
            Ok(())
        });
        assert!(r.is_err());
        assert_eq!(setting(&s, "tail"), None, "the tail write never ran");
    }

    /// Residual I1: a READ that SQLite answered with a whole-transaction
    /// rollback (a real `SQLITE_IOERR`) and whose error the closure swallowed
    /// leaves the connection in autocommit. A savepoint helper after it must
    /// not open: standalone its SAVEPOINT would start a new transaction and
    /// its RELEASE commit the tail on its own.
    #[test]
    fn in_savepoint_refuses_to_open_on_a_lost_atomically() {
        let (s, bus) = test_support::store_with_recorder();
        s.conn.execute_batch("CREATE TABLE tail (x TEXT)").unwrap();
        s.set_setting("boom", "1").unwrap();
        s.arm_read_ioerr_for_test("settings", "key = 'boom'");
        bus.take();
        let mut body_ran = false;
        let mut sp: Option<Result<(), crate::ipc_error::IpcError>> = None;
        let r = s.atomically(|s| {
            s.conn.execute("INSERT INTO tail VALUES ('before')", [])?;
            // A best-effort read whose error is swallowed.
            assert!(s.get_setting("boom").is_err(), "the read fails");
            assert!(
                s.conn.is_autocommit(),
                "SQLite rolled the read's transaction back"
            );
            sp = Some(
                s.in_savepoint("sp_tail", |c| -> Result<(), crate::ipc_error::IpcError> {
                    body_ran = true;
                    c.execute("INSERT INTO tail VALUES ('after')", [])?;
                    Ok(())
                }),
            );
            // Swallowed too: `atomically`'s own check still fails the call.
            Ok(())
        });
        assert!(!body_ran, "no savepoint opens on a lost transaction");
        let sp_err = sp.unwrap().unwrap_err();
        assert!(
            sp_err.message.contains("rolled back"),
            "names the lost transaction: {sp_err:?}"
        );
        assert!(r.is_err(), "the lost transaction fails atomically");
        let tail = |s: &Store| -> i64 {
            s.conn
                .query_row("SELECT COUNT(*) FROM tail", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(tail(&s), 0, "nothing kept");
        assert!(s.conn.is_autocommit(), "no transaction left open");
        assert!(bus.take().is_empty(), "nothing announced");
        // Standalone (no `atomically`), a savepoint in autocommit is the
        // transaction itself and still opens.
        s.in_savepoint("sp_alone", |c| -> rusqlite::Result<()> {
            c.execute("INSERT INTO tail VALUES ('alone')", [])?;
            Ok(())
        })
        .unwrap();
        assert_eq!(tail(&s), 1);
    }

    // ── M3: the write lock is taken at BEGIN ──

    /// `atomically` reads, then writes. Under a DEFERRED transaction another
    /// connection (a `fleet-hub` CLI) committing in between turns the write
    /// into SQLITE_BUSY_SNAPSHOT with no busy wait; IMMEDIATE takes the
    /// write lock up front, so the other writer waits instead.
    #[test]
    fn atomically_holds_the_write_lock_from_begin() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let s = Store::open_with_bus(&db, Arc::new(NoopEventBus)).unwrap();
        let other = rusqlite::Connection::open(&db).unwrap();
        other.busy_timeout(std::time::Duration::ZERO).unwrap();
        let mut other_write = None;
        s.atomically(|s| {
            let _ = s.get_setting("k")?;
            other_write =
                Some(other.execute("INSERT INTO settings (key, value) VALUES ('cli', '1')", []));
            s.set_setting("k", "1")?;
            Ok(())
        })
        .expect("the read-then-write transaction commits");
        assert!(
            other_write.unwrap().is_err(),
            "the other connection could not write while atomically held the lock"
        );
        assert_eq!(setting(&s, "k").as_deref(), Some("1"));
    }
}
