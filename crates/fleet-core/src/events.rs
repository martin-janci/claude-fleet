//! Typed event bus for delta-update store sync.
//!
//! The `Store` calls into an `EventBus` whenever a row is mutated; the desktop's
//! `AppHandleEventBus` lives in `src-tauri/src/app_events.rs` and forwards each
//! event to the Svelte frontend. Tests use `NoopEventBus` (silent) or
//! `RecordingEventBus` (captures every emit for assertion).
//!
//! Every event is one [`RowChange`] variant. The typed convenience methods on
//! [`EventBus`] (`session_updated(&row)`, …) are default trait methods that
//! build the variant and hand it to the single required [`EventBus::emit`];
//! [`RowChange::name`] is the frontend event name `src/lib/events.ts`
//! subscribes to and [`RowChange::payload`] the JSON the frontend receives.

use crate::service::account_usage::AccountUsageSnapshot;
use crate::store::{
    AccountRow, AssetInventoryRow, HostRow, ProjectRow, SessionEvent, SessionRow, TaskRow,
    WorktreeRow,
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Mutex;

/// One event on the bus. Also used as the deferred form during a batched
/// write (e.g. reconcile's per-host write-burst): the SQL is applied inside a
/// transaction, the `RowChange`s are held, and only flushed to the `EventBus`
/// AFTER the transaction commits. This guarantees no event fires for a change
/// that gets rolled back. See `EventBus::emit_change`.
#[derive(Clone)]
pub enum RowChange {
    SessionCreated(SessionRow),
    SessionUpdated(SessionRow),
    /// A session row is GONE (hard-deleted). Carries the row's identity
    /// facts as well as its id, because the row is already deleted by the
    /// time a reader could look it up — see [`SessionKilledPayload`].
    SessionKilled(SessionKilledPayload),
    /// A timeline event was appended (migration 013/037). Carries the row.
    SessionEventAdded(SessionEvent),
    /// A session's conversation list changed (opened / closed / reopened).
    /// Payload is the session id only; the UI refetches the small list.
    ConversationsChanged(i64),
    HostAdded(HostRow),
    HostProbed(HostRow),
    /// A probe that found the host exactly as it was: only `last_pinged_at`
    /// moved.
    ///
    /// Reconcile probes every host every pass and `last_pinged_at` moves on
    /// each one, so the full row could never be diffed away — the comment in
    /// `upsert_session_in_tx` says as much. Measured on a five-host fleet
    /// that is three `host:probed` frames of ~265 B every 25 s to every
    /// connected client, about 108 KB/h, to say nothing changed.
    ///
    /// A client that does not know the name drops it (`known_event_name`),
    /// which is the right fallback: a missed heartbeat is cosmetic, and every
    /// real change still arrives as a full `host:probed`.
    HostPinged {
        alias: String,
        last_pinged_at: i64,
        reachable: bool,
        /// Host identity & health, task 1: the versions stamp, so a
        /// stamp-only refresh needs no full-row event. `to_value` writes it
        /// as a nullable field; a client that predates it ignores it.
        claude_version_at: Option<i64>,
        /// Task 2: the health sample of this pass, for the same reason —
        /// disk and load move every pass and must not cost a full row.
        health: Option<crate::store::HostHealth>,
    },
    HostRemoved(String),
    AccountUpserted(AccountRow),
    ProjectUpdated(ProjectRow),
    WorktreeUpdated(WorktreeRow),
    WorktreeRemoved(i64),
    /// A task row was created or changed state (migration 020). There is no
    /// `task:removed` — tasks only ever move to a terminal state.
    TaskUpdated(TaskRow),
    /// An account's usage snapshot changed (Task 4): a fetch that was due
    /// completed with a result different from what the cache already held.
    /// A call the floor turns away (not due, snapshot unchanged) never
    /// reaches this.
    AccountUsageUpdated(AccountUsageSnapshot),
    /// One asset's drift state on one host changed (migration 030).
    AssetInventoryUpdated(AssetInventoryRow),
    /// Every inventory row for (host, harness) was dropped before a rescan
    /// writes the new set.
    AssetInventoryCleared {
        host_alias: String,
        harness: String,
    },
    /// The catalog repo was (re)loaded; the summary carries its HEAD and
    /// counts. Not a store row.
    CatalogLoaded(CatalogSummary),
    /// Progress of an in-flight sync apply (sub-project 2). Not a store row.
    SyncProgress(SyncProgress),
    /// A step boundary of an in-flight move. Not a store row.
    MoveProgress(MoveProgress),
    /// A step boundary of an in-flight `new_session` (redesign step 5.13):
    /// worktree, tmux, agent. Keyed by the caller's own opaque start token,
    /// never by host or name. Not a store row.
    StartProgress(StartProgress),
    /// A tracker item's normalised row changed (work graph M3). Emitted
    /// only on a real change: a sync pass that finds nothing new is silent.
    /// Never sent to a host-bound `/events` stream (the interim fence of
    /// the M3 plan's decision 6).
    WorkItemUpdated(crate::store::WorkItemRow),
    /// A tracker's row changed: state, config, credential presence. Carries
    /// no secret (`TrackerRow` cannot).
    TrackerUpdated(crate::store::TrackerRow),
    /// A tracker was removed.
    TrackerRemoved(i64),
    /// The Work view's structure changed (work graph M14): a placement, a
    /// placement rule, a saved view, or a local item's org. Ids only — a
    /// client re-reads what it shows. Kind `work`, so never sent to a
    /// host-bound or org-bound stream.
    WorkChanged(WorkChanged),
    /// An operator setting was written (declarative pages P3): the key
    /// only — a client re-reads what it shows. Kind `settings`, so never
    /// sent to a host-bound or org-bound stream: the values are master-only.
    SettingsChanged(String),
    /// One share of one session was created, narrowed or revoked
    /// (multi-user M1, T9). Ids only — the client patches its own grant set
    /// (`src/lib/access.ts`). Kind `grant`, so never sent to a host-bound
    /// or org-bound stream: a per-host token has no person and no grants.
    GrantChanged(GrantChanged),
    /// The fleet's update picture changed (update-channel design §11): a
    /// target's observed version or phase, a pin, or the verified channel.
    /// Ids only — a client re-reads `update_status`. Kind `update`, so never
    /// sent to a host-bound or org-bound stream (it names every target).
    UpdateChanged(UpdateChanged),
    /// What the hub would tell one target changed (update-channel design
    /// §6.4): a pin, a new channel, or a policy setting moved its decision.
    /// The target, its new status and version — no documents; a client that
    /// is that target checks again (`POST /update/check`). Kind `update`.
    UpdateDecision(UpdateDecision),
    /// A file download's row was added, moved state, was fetched or was
    /// removed (migration 095). The id only — a client re-reads
    /// `list_downloads`. Kind `download`, so never sent to a host-bound or
    /// org-bound stream (it names files of every host).
    DownloadChanged(i64),
    /// A local workspace link (migration 109) was made, moved state, had a
    /// pass or was removed. The id only — the desktop re-reads
    /// `list_local_workspaces`. Kind `local_workspace`; links live on the
    /// desktop that made them, so a hub never emits one, and it is host-bound
    /// hidden besides (it names a directory on someone's machine).
    LocalWorkspaceChanged(i64),
    /// The queue of control-API calls waiting for a person changed: one was
    /// asked for, answered or expired (redesign step 9.2). Carries nothing —
    /// the owner's device re-reads `mcp_confirms` (Access::PersonDevice), so
    /// the tool and its arguments never ride a stream. Kind `confirm`,
    /// host-bound hidden.
    ConfirmChanged,
    /// Control's agent handed work on and a receipt was written (redesign
    /// step 9.3). Carries nothing: the owner's device re-reads
    /// `control_handoffs` (Access::PersonDevice), so what was sent never
    /// rides a stream. Kind `handoff`, host-bound hidden.
    HandoffChanged,
}

/// The payload of `update:changed`.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct UpdateChanged {
    /// observed | pin | channel
    pub what: String,
    /// The target (`client:3`, `agent:h`, `hub:self`) for `observed`, or a
    /// pinned target; absent for a component-wide pin and the channel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// The payload of `update:decision`.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct UpdateDecision {
    /// `client:3`, `agent:h`, `hub:self`.
    pub target: String,
    /// The decision's status (`update_available`, `update_required`, …).
    pub status: String,
    /// The version it now names, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// The payload of `grant:changed` (multi-user M1, T9): one change to one
/// share of one session, ids only.
///
/// `level: None` is a revoke — the frontend's `parseGrantChanged` reads a
/// missing or unknown level as exactly that, since the one direction a
/// grant may move is downward (spec §4.3 invariant 3).
///
/// A grant mutates no `sessions` column, so sharing could never ride a row
/// event; the two frames `Store::announce_grant_change` emits are a
/// `session:updated` (how a new recipient learns the row exists at all) and
/// this one (how each client keeps its own grant set current). It reaches
/// the person the grant names and the session's owner, and nobody else.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct GrantChanged {
    pub session_id: i64,
    pub person_id: i64,
    /// `watch` | `drive`, or `None` for a revoke. Written as `null` rather
    /// than omitted — `strip_nulls` takes it off the wire either way, and
    /// the client reads absent and null the same.
    pub level: Option<String>,
}

/// The payload of `work:changed`.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct WorkChanged {
    /// placement | rule | view | org
    pub what: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_id: Option<i64>,
}

/// The payload of `session:killed`.
///
/// **Why it carries more than an id** (multi-user M1, T9). Every other
/// `session:*` frame is fenced on `/events` by asking what the caller may
/// see about that row — from the frame itself for a row-bearing frame, or
/// by resolving `session_id` in the store. This frame can do neither: it
/// fires *after* the `DELETE`, so there is no row left to resolve, and an
/// id alone tells a stranger that a session with that id existed and has
/// now ended. That is an existence oracle on exactly the mapping a former
/// grantee learned while their grant was live.
///
/// So the facts travel with the frame, read while the row was still there
/// ([`crate::store::Store::killed_payload`]), and
/// `events_route::fence_frame` judges them through
/// [`crate::service::view_scope::ViewScope::sees_session_facts`] — the same
/// body a live row goes through.
///
/// Every added field is `Option` and `skip_serializing_if`, so a payload
/// built from an id alone still serialises to exactly `{"id": N}` — the
/// shape every existing client reads. A frame with the facts MISSING is
/// dropped by the fence for every caller but the hub's own readers: that is
/// the fail-closed direction, and [`SessionKilledPayload::from`] is the one
/// way to build such a payload (tests, and nothing else).
#[derive(Serialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct SessionKilledPayload {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    /// The session's org, for the org half of the fence (work graph M5/M14).
    /// Not in the plan's three keys and required all the same: without it a
    /// client bound to one org would learn that a session of another org
    /// had died.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_person_id: Option<i64>,
}

impl SessionKilledPayload {
    /// The facts of a row that is still there.
    pub fn of_row(row: &SessionRow) -> Self {
        Self {
            id: row.id,
            host_alias: Some(row.host_alias.clone()),
            org_id: row.org_id,
            visibility: Some(row.visibility.clone()),
            owner_person_id: row.owner_person_id,
        }
    }

    /// The facts this payload carries, for the fence — `None` when it
    /// carries none, which the fence reads as "drop it".
    pub fn facts(&self) -> Option<crate::service::view_scope::SessionFacts<'_>> {
        Some(crate::service::view_scope::SessionFacts {
            id: self.id,
            host_alias: self.host_alias.as_deref()?,
            org_id: self.org_id,
            visibility: self.visibility.as_deref()?,
            owner_person_id: self.owner_person_id,
        })
    }
}

/// An id and nothing else: a kill nobody can fence, dropped by
/// `fence_frame` for every caller but the hub's own readers. Deliberately
/// the only way to spell that, so a production emitter that forgets the
/// facts reads as what it is.
impl From<i64> for SessionKilledPayload {
    fn from(id: i64) -> Self {
        Self {
            id,
            ..Default::default()
        }
    }
}

#[derive(Serialize, Clone)]
pub struct HostRemovedPayload {
    pub alias: String,
}

#[derive(Serialize, Clone)]
pub struct WorktreeRemovedPayload {
    pub id: i64,
}

#[derive(Serialize, Clone)]
pub struct AssetInventoryClearedPayload {
    pub host_alias: String,
    pub harness: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CatalogSummary {
    pub head: String,
    pub loaded_at: i64,
    pub asset_count: usize,
    pub problem_count: usize,
}

/// Progress of one in-flight sync apply (migration 031 / sub-project 2):
/// (host, harness) pairs finished / total pairs in the plan;
/// `host_alias`/`harness` name the pair about to be applied. A run that
/// completes ends with one terminal event at `done == total` whose
/// `host_alias`/`harness` are empty — no pair is about to be applied.
#[derive(Serialize, Clone, Debug)]
pub struct SyncProgress {
    pub plan_id: String,
    pub host_alias: String,
    pub harness: String,
    pub done: usize,
    pub total: usize,
}

/// The nine user-facing steps of a move, in the order they run. Several of
/// `move_session`'s internal stages fold into one step (seeding is part of
/// `workspace`, the worktree is set up in `replay`, the confirm is part of
/// `start`): the user follows these, not the stage numbers.
///
/// `git` ends when the carried commits are in the target's clone. The target
/// worktree is created, fast-forwarded and checked after that, inside
/// `replay` — so a dirty or diverged target worktree fails `replay`, which
/// is the step that was going to replay work into it.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MoveStep {
    Check,
    Transcript,
    Workspace,
    Git,
    Replay,
    Ignored,
    ClaudeState,
    Start,
    Handoff,
}

impl MoveStep {
    /// Every step, in order. `src/lib/moveProgress.ts` mirrors it
    /// (`frontend_declares_the_move_steps_in_order`).
    pub const ALL: [MoveStep; 9] = [
        MoveStep::Check,
        MoveStep::Transcript,
        MoveStep::Workspace,
        MoveStep::Git,
        MoveStep::Replay,
        MoveStep::Ignored,
        MoveStep::ClaudeState,
        MoveStep::Start,
        MoveStep::Handoff,
    ];

    /// The wire name (what serde writes).
    pub const fn as_str(self) -> &'static str {
        match self {
            MoveStep::Check => "check",
            MoveStep::Transcript => "transcript",
            MoveStep::Workspace => "workspace",
            MoveStep::Git => "git",
            MoveStep::Replay => "replay",
            MoveStep::Ignored => "ignored",
            MoveStep::ClaudeState => "claude_state",
            MoveStep::Start => "start",
            MoveStep::Handoff => "handoff",
        }
    }

    /// 1-based position in [`Self::ALL`].
    pub const fn index(self) -> u8 {
        self as u8 + 1
    }
}

/// How a [`MoveStep`] stands. `Warned` is a step that could not do all of
/// its work but cannot fail the move (ignored files, the Claude-side state).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MoveStepState {
    Started,
    Done,
    Warned,
    Failed,
}

impl MoveStepState {
    pub const fn as_str(self) -> &'static str {
        match self {
            MoveStepState::Started => "started",
            MoveStepState::Done => "done",
            MoveStepState::Warned => "warned",
            MoveStepState::Failed => "failed",
        }
    }
}

/// One step boundary of an in-flight `move_session`. `session_id` is the
/// SOURCE row. `detail` is a short count ("2 commits") — never a path or
/// stderr — and is `None` on `Failed`: the error reaches the caller through
/// the command's result.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MoveProgress {
    pub session_id: i64,
    pub to_host: String,
    pub step: MoveStep,
    pub index: u8,
    pub total: u8,
    pub state: MoveStepState,
    pub detail: Option<String>,
}

/// The three steps a session start goes through (redesign step 5.13), in
/// order. `src/lib/start_steps.ts` mirrors them (`START_STEPS`).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StartStep {
    /// Resolving the checkout: clone, `git worktree add`, self-repair.
    Worktree,
    /// `tmux new-session` and the reconcile pass that registers the row.
    Tmux,
    /// The agent's launch recorded on the row (model, effort, agent, owner).
    Agent,
}

impl StartStep {
    pub const ALL: [StartStep; 3] = [StartStep::Worktree, StartStep::Tmux, StartStep::Agent];

    pub const fn as_str(self) -> &'static str {
        match self {
            StartStep::Worktree => "worktree",
            StartStep::Tmux => "tmux",
            StartStep::Agent => "agent",
        }
    }

    /// 1-based position in [`Self::ALL`].
    pub const fn index(self) -> u8 {
        self as u8 + 1
    }
}

/// One step boundary of an in-flight `new_session`. `token` is the opaque
/// string the caller minted and passed as `start_token`, so only the client
/// that started the session can tell whose start this is: the frame names
/// no host, no session, no person. `state` is `started`, `done` or `failed`
/// (never `warned`); the error itself reaches the caller through the
/// command's result.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StartProgress {
    pub token: String,
    pub step: StartStep,
    pub index: u8,
    pub total: u8,
    pub state: MoveStepState,
}

impl RowChange {
    /// The frontend event name (`src/lib/events.ts` `RowEvent.name`).
    pub fn name(&self) -> &'static str {
        match self {
            RowChange::SessionCreated(_) => "session:created",
            RowChange::SessionUpdated(_) => "session:updated",
            RowChange::SessionKilled(_) => "session:killed",
            RowChange::SessionEventAdded(_) => "session:event",
            RowChange::ConversationsChanged(_) => "session:conversations",
            RowChange::HostAdded(_) => "host:added",
            RowChange::HostProbed(_) => "host:probed",
            RowChange::HostPinged { .. } => "host:pinged",
            RowChange::HostRemoved(_) => "host:removed",
            RowChange::AccountUpserted(_) => "account:upserted",
            RowChange::ProjectUpdated(_) => "project:updated",
            RowChange::WorktreeUpdated(_) => "worktree:updated",
            RowChange::WorktreeRemoved(_) => "worktree:removed",
            RowChange::TaskUpdated(_) => "task:updated",
            RowChange::AccountUsageUpdated(_) => "account_usage:updated",
            RowChange::AssetInventoryUpdated(_) => "asset_inventory:updated",
            RowChange::AssetInventoryCleared { .. } => "asset_inventory:cleared",
            RowChange::CatalogLoaded(_) => "catalog:loaded",
            RowChange::SyncProgress(_) => "sync:progress",
            RowChange::MoveProgress(_) => "move:progress",
            RowChange::StartProgress(_) => "start:progress",
            RowChange::WorkItemUpdated(_) => "work:item",
            RowChange::TrackerUpdated(_) => "work:tracker",
            RowChange::TrackerRemoved(_) => "work:tracker_removed",
            RowChange::WorkChanged(_) => "work:changed",
            RowChange::SettingsChanged(_) => "settings:changed",
            RowChange::GrantChanged(_) => "grant:changed",
            RowChange::UpdateChanged(_) => "update:changed",
            RowChange::UpdateDecision(_) => "update:decision",
            RowChange::DownloadChanged(_) => "download:changed",
            RowChange::LocalWorkspaceChanged(_) => "local_workspace:changed",
            RowChange::ConfirmChanged => "confirm:changed",
            RowChange::HandoffChanged => "handoff:changed",
        }
    }

    /// The JSON payload the frontend receives for this event.
    pub fn payload(&self) -> serde_json::Value {
        fn to_value<T: Serialize>(v: &T) -> serde_json::Value {
            // `to_value` on these small structs cannot realistically fail;
            // on the off chance it does we send Null rather than panic.
            serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
        }
        match self {
            RowChange::SessionCreated(r) | RowChange::SessionUpdated(r) => to_value(r),
            RowChange::SessionKilled(k) => to_value(k),
            RowChange::SessionEventAdded(e) => to_value(e),
            RowChange::ConversationsChanged(id) => serde_json::json!({ "session_id": id }),
            RowChange::HostAdded(r) | RowChange::HostProbed(r) => to_value(r),
            RowChange::HostPinged {
                alias,
                last_pinged_at,
                reachable,
                claude_version_at,
                health,
            } => serde_json::json!({
                "alias": alias,
                "last_pinged_at": last_pinged_at,
                "reachable": reachable,
                "claude_version_at": claude_version_at,
                "health": health,
            }),
            RowChange::HostRemoved(alias) => to_value(&HostRemovedPayload {
                alias: alias.clone(),
            }),
            RowChange::AccountUpserted(r) => to_value(r),
            RowChange::ProjectUpdated(r) => to_value(r),
            RowChange::WorktreeUpdated(r) => to_value(r),
            RowChange::WorktreeRemoved(id) => to_value(&WorktreeRemovedPayload { id: *id }),
            RowChange::TaskUpdated(r) => to_value(r),
            RowChange::AccountUsageUpdated(r) => to_value(r),
            RowChange::AssetInventoryUpdated(r) => to_value(r),
            RowChange::AssetInventoryCleared {
                host_alias,
                harness,
            } => to_value(&AssetInventoryClearedPayload {
                host_alias: host_alias.clone(),
                harness: harness.clone(),
            }),
            RowChange::CatalogLoaded(s) => to_value(s),
            RowChange::SyncProgress(p) => to_value(p),
            RowChange::MoveProgress(p) => to_value(p),
            RowChange::StartProgress(p) => to_value(p),
            RowChange::WorkItemUpdated(r) => to_value(r),
            RowChange::TrackerUpdated(r) => to_value(r),
            RowChange::TrackerRemoved(id) => serde_json::json!({ "id": id }),
            RowChange::WorkChanged(w) => to_value(w),
            RowChange::SettingsChanged(key) => serde_json::json!({ "key": key }),
            RowChange::GrantChanged(g) => to_value(g),
            RowChange::UpdateChanged(u) => to_value(u),
            RowChange::UpdateDecision(u) => to_value(u),
            RowChange::DownloadChanged(id) => serde_json::json!({ "id": id }),
            RowChange::LocalWorkspaceChanged(id) => serde_json::json!({ "id": id }),
            RowChange::ConfirmChanged | RowChange::HandoffChanged => serde_json::json!({}),
        }
    }
}

pub trait EventBus: Send + Sync {
    /// The one method an impl provides: deliver a single event.
    fn emit(&self, e: &RowChange);

    fn session_created(&self, row: &SessionRow) {
        self.emit(&RowChange::SessionCreated(row.clone()));
    }
    fn session_updated(&self, row: &SessionRow) {
        self.emit(&RowChange::SessionUpdated(row.clone()));
    }
    /// See [`SessionKilledPayload`] — a production emitter passes the facts
    /// it read before the row went; `id.into()` is the id-only form.
    fn session_killed(&self, killed: SessionKilledPayload) {
        self.emit(&RowChange::SessionKilled(killed));
    }
    /// See [`RowChange::SessionEventAdded`].
    fn session_event_added(&self, e: &SessionEvent) {
        self.emit(&RowChange::SessionEventAdded(e.clone()));
    }
    /// See [`RowChange::ConversationsChanged`].
    fn conversations_changed(&self, session_id: i64) {
        self.emit(&RowChange::ConversationsChanged(session_id));
    }
    fn host_added(&self, row: &HostRow) {
        self.emit(&RowChange::HostAdded(row.clone()));
    }
    fn host_probed(&self, row: &HostRow) {
        self.emit(&RowChange::HostProbed(row.clone()));
    }
    fn host_removed(&self, alias: &str) {
        self.emit(&RowChange::HostRemoved(alias.to_string()));
    }
    fn account_upserted(&self, row: &AccountRow) {
        self.emit(&RowChange::AccountUpserted(row.clone()));
    }
    fn project_updated(&self, row: &ProjectRow) {
        self.emit(&RowChange::ProjectUpdated(row.clone()));
    }
    fn worktree_updated(&self, row: &WorktreeRow) {
        self.emit(&RowChange::WorktreeUpdated(row.clone()));
    }
    fn worktree_removed(&self, id: i64) {
        self.emit(&RowChange::WorktreeRemoved(id));
    }
    /// See [`RowChange::TaskUpdated`].
    fn task_updated(&self, row: &TaskRow) {
        self.emit(&RowChange::TaskUpdated(row.clone()));
    }
    /// See [`RowChange::AccountUsageUpdated`].
    fn account_usage_updated(&self, row: &AccountUsageSnapshot) {
        self.emit(&RowChange::AccountUsageUpdated(row.clone()));
    }
    /// See [`RowChange::AssetInventoryUpdated`].
    fn asset_inventory_updated(&self, row: &AssetInventoryRow) {
        self.emit(&RowChange::AssetInventoryUpdated(row.clone()));
    }
    /// See [`RowChange::AssetInventoryCleared`].
    fn asset_inventory_cleared(&self, host_alias: &str, harness: &str) {
        self.emit(&RowChange::AssetInventoryCleared {
            host_alias: host_alias.to_string(),
            harness: harness.to_string(),
        });
    }
    /// See [`RowChange::CatalogLoaded`].
    fn catalog_loaded(&self, summary: &CatalogSummary) {
        self.emit(&RowChange::CatalogLoaded(summary.clone()));
    }
    /// See [`RowChange::SyncProgress`].
    fn sync_progress(&self, p: &SyncProgress) {
        self.emit(&RowChange::SyncProgress(p.clone()));
    }
    /// See [`RowChange::MoveProgress`].
    fn move_progress(&self, p: &MoveProgress) {
        self.emit(&RowChange::MoveProgress(p.clone()));
    }
    /// See [`RowChange::StartProgress`].
    fn start_progress(&self, p: &StartProgress) {
        self.emit(&RowChange::StartProgress(p.clone()));
    }
    /// See [`RowChange::GrantChanged`].
    fn grant_changed(&self, g: &GrantChanged) {
        self.emit(&RowChange::GrantChanged(g.clone()));
    }
    /// See [`RowChange::ConfirmChanged`].
    fn confirm_changed(&self) {
        self.emit(&RowChange::ConfirmChanged);
    }
    /// See [`RowChange::HandoffChanged`].
    fn handoff_changed(&self) {
        self.emit(&RowChange::HandoffChanged);
    }

    /// Flush a single deferred `RowChange`. Used by batched (transactional)
    /// writes to emit AFTER commit; an alias of [`EventBus::emit`] kept for
    /// those call sites.
    fn emit_change(&self, change: &RowChange) {
        self.emit(change);
    }

    /// `health.context_red_pct` was written; `pct` is the value now in force.
    /// Not an event: a bus that stamps derived fields onto its frames (the
    /// hub's [`BroadcastEventBus`], for `needs_attention`'s `context_full`)
    /// keeps its copy of the threshold in step with the store through this.
    /// Every other bus ignores it.
    fn context_red_pct_changed(&self, _pct: f64) {}

    /// What the fleet knows beyond a session's own row (step 2.6): which
    /// hosts are down, and which accounts are at a usage limit or have no
    /// login. The hub's [`BroadcastEventBus`] follows it from the host and
    /// usage events it carries, after [`Self::attention_seeded`]; every
    /// other bus knows nothing, which is the row-only classification.
    fn attention_facts(&self) -> crate::service::attention::Facts {
        crate::service::attention::Facts::default()
    }

    /// Hand [`Self::attention_facts`] the host rows and usage answers that
    /// predate this process's events: `fleet-hub serve`'s hosts at startup,
    /// the usage history `restore_usage` seeds. Not an event. Every bus but
    /// the hub's ignores it.
    fn attention_seeded(&self, _hosts: &[HostRow], _usage: &[AccountUsageSnapshot]) {}

    /// Each account's latest usage answer this bus has carried (or was
    /// seeded with), by account: what the hub's `account_usage` tool serves,
    /// since the usage tick's cache is not reachable from the control API.
    /// Only the hub's [`BroadcastEventBus`] follows it; every other bus knows
    /// none.
    fn account_usage(&self) -> Vec<AccountUsageSnapshot> {
        Vec::new()
    }
}

/// Silently drops every event. For tests and any context that doesn't need
/// to surface row changes to a frontend.
pub struct NoopEventBus;
impl EventBus for NoopEventBus {
    fn emit(&self, _: &RowChange) {}
}

/// One event as it travels to a remote subscriber: the name and payload a
/// [`RowChange`] renders to, with the store row already serialized so nothing
/// downstream needs the row types. Cheap to clone — `Value` is the only
/// owned field, and a broadcast channel clones once per receiver.
#[derive(Clone, Debug)]
pub struct EventMessage {
    pub name: &'static str,
    pub payload: serde_json::Value,
    /// Position in this hub's event sequence, starting at 1.
    ///
    /// It is what an SSE `id:` carries and what a reconnecting client sends
    /// back as `Last-Event-ID`, so a dropped connection can cost the events
    /// it missed instead of a full re-list. Unique and increasing within one
    /// process only — see [`BroadcastEventBus::generation`].
    pub seq: u64,
}

impl EventMessage {
    /// The part of the name before `:` — `session`, `host`, `account_usage`,
    /// … — which is what the `/events` route's `?kinds=` filter matches on.
    pub fn kind(&self) -> &str {
        self.name
            .split_once(':')
            .map(|(k, _)| k)
            .unwrap_or(self.name)
    }
}

/// Fans every event out to any number of live subscribers over a
/// `tokio::sync::broadcast` channel. `fleet-hub serve` opens its store with
/// this bus and the `GET /events` SSE route subscribes per connection.
///
/// Like the desktop's `AppHandleEventBus` (which queues through an mpsc
/// channel and a drain thread), [`emit`](BroadcastEventBus::emit) never
/// blocks: `broadcast::Sender::send` writes into the ring buffer and returns
/// immediately, whether there are no subscribers at all or a slow one that
/// has fallen behind. A store write therefore never waits on delivery. The
/// price of that promise is the ring: a subscriber that falls more than
/// `capacity` events behind loses the oldest ones and is told so
/// (`RecvError::Lagged`) rather than holding anyone up.
pub struct BroadcastEventBus {
    tx: tokio::sync::broadcast::Sender<EventMessage>,
    /// Hands out [`EventMessage::seq`]. Bumped under [`Self::ring`], so the
    /// number an event carries and its position in the ring can never
    /// disagree — a client that resumes at N must not be sent N+1 before N.
    seq: AtomicU64,
    /// The last [`REPLAY_RING`] events, for a client that reconnects with a
    /// `Last-Event-ID`.
    ///
    /// A separate structure and not the broadcast channel's own buffer,
    /// because a `tokio::sync::broadcast` receiver always starts at the tail
    /// and cannot be rewound — [`Self::subscribe`] says as much.
    ring: Mutex<VecDeque<EventMessage>>,
    /// Identifies this process's sequence. A restarted hub starts counting
    /// at 1 again, so without it a client resuming at "1200" would be handed
    /// the wrong twelve hundred events, silently. A generation that does not
    /// match means no replay and a full re-list, which is the honest answer.
    generation: u64,
    /// Unix seconds when a subscriber was last present. See [`RING_GRACE_SECS`].
    last_subscriber_at: AtomicI64,
    /// `health.context_red_pct` as `f64::to_bits`, the threshold the stamped
    /// `needs_attention` judges `context_full` at. The bus holds no store, so
    /// `fleet-hub serve` sets it at startup and `service::settings::set`
    /// refreshes it on every write of the setting
    /// ([`EventBus::context_red_pct_changed`]).
    context_red_pct: AtomicU64,
    /// The host rows and usage answers [`EventBus::attention_facts`] is
    /// decided from, followed off the events this bus carries (step 2.6).
    attention: Mutex<AttentionInputs>,
}

/// What [`BroadcastEventBus`] knows of the fleet's hosts and accounts, kept
/// current from `host:*` and `account_usage:updated` frames, and the
/// [`Facts`](crate::service::attention::Facts) they make.
#[derive(Default)]
struct AttentionInputs {
    hosts: std::collections::BTreeMap<String, HostRow>,
    usage: std::collections::BTreeMap<String, AccountUsageSnapshot>,
    /// The facts as last decided, and the unix second they stop holding (the
    /// earliest limit reset among them): past it they are decided again.
    facts: Option<(crate::service::attention::Facts, Option<i64>)>,
}

impl AttentionInputs {
    /// Take in one event; anything but a host or usage change is ignored.
    fn observe(&mut self, e: &RowChange) {
        match e {
            RowChange::HostAdded(row) | RowChange::HostProbed(row) => {
                self.hosts.insert(row.alias.clone(), row.clone());
            }
            RowChange::HostPinged {
                alias,
                last_pinged_at,
                reachable,
                ..
            } => match self.hosts.get_mut(alias) {
                Some(h) if h.reachable != *reachable || h.last_pinged_at.is_none() => {
                    h.reachable = *reachable;
                    h.last_pinged_at = Some(*last_pinged_at);
                }
                // A heartbeat that changes nothing the facts read.
                _ => return,
            },
            RowChange::HostRemoved(alias) => {
                self.hosts.remove(alias);
            }
            RowChange::AccountUsageUpdated(snap) => {
                self.usage.insert(snap.account_uuid.clone(), snap.clone());
            }
            _ => return,
        }
        self.facts = None;
    }

    fn seed(&mut self, hosts: &[HostRow], usage: &[AccountUsageSnapshot]) {
        for h in hosts {
            self.hosts.insert(h.alias.clone(), h.clone());
        }
        for u in usage {
            self.usage.insert(u.account_uuid.clone(), u.clone());
        }
        self.facts = None;
    }

    fn facts(&mut self, now: i64) -> crate::service::attention::Facts {
        if let Some((facts, until)) = &self.facts {
            if until.is_none_or(|at| now < at) {
                return facts.clone();
            }
        }
        let hosts: Vec<HostRow> = self.hosts.values().cloned().collect();
        let usage: Vec<AccountUsageSnapshot> = self.usage.values().cloned().collect();
        let facts = crate::service::attention::Facts::from_fleet(&hosts, &usage, now);
        let until = facts
            .limited_accounts
            .values()
            .filter_map(|l| l.resets_at)
            .min();
        self.facts = Some((facts.clone(), until));
        facts
    }
}

/// Every event name a [`RowChange`] renders to — the exact strings
/// `src/lib/events.ts` `listen`s for.
///
/// Two callers need the whole set rather than one variant's name, and neither
/// can enumerate `RowChange` (a variant's `name()` needs an instance, and the
/// row-bearing variants cannot be built without a store row):
///
/// - the desktop's hub-client bridge, which must resolve a name that arrived
///   over the network back to one of these `&'static str`s before emitting
///   it, so an arbitrary string off the wire can never become a frontend
///   event;
/// - `frontend_declares_every_event_name`, which checks them against
///   `events.ts`.
///
/// `every_row_change_variant_is_subscribable` holds this list to `RowChange`
/// at COMPILE time: the match there is exhaustive, so a new variant does not
/// build until it has an arm, and the arm's literal is const-checked against
/// this list and [`EVENT_KINDS`].
pub const EVENT_NAMES: [&str; 33] = [
    "session:created",
    "session:updated",
    "session:killed",
    "session:event",
    "session:conversations",
    "host:added",
    "host:probed",
    "host:pinged",
    "host:removed",
    "account:upserted",
    "project:updated",
    "worktree:updated",
    "worktree:removed",
    "task:updated",
    "account_usage:updated",
    "asset_inventory:updated",
    "asset_inventory:cleared",
    "catalog:loaded",
    "sync:progress",
    "move:progress",
    "start:progress",
    "work:item",
    "work:tracker",
    "work:tracker_removed",
    "work:changed",
    "settings:changed",
    "update:changed",
    "update:decision",
    "download:changed",
    "grant:changed",
    "local_workspace:changed",
    "confirm:changed",
    "handoff:changed",
];

/// Every event kind — the part of a [`RowChange::name`] before the `:`, which
/// is what the `/events` route's `?kinds=` filter matches on.
/// `event_kinds_cover_every_name` keeps it in step with the variants.
pub const EVENT_KINDS: [&str; 20] = [
    "session",
    "host",
    "account",
    "project",
    "worktree",
    "task",
    "account_usage",
    "asset_inventory",
    "catalog",
    "sync",
    "move",
    "start",
    "work",
    "settings",
    "update",
    "download",
    "grant",
    "local_workspace",
    "confirm",
    "handoff",
];

/// Seconds since the Unix epoch (0 on a clock set before 1970).
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Ring size for [`BroadcastEventBus`]. Reconcile holds its emits until the
/// transaction commits and then flushes them in one burst (one pass every
/// ~20 s on a hub), so the buffer has to absorb a whole pass over a busy
/// fleet, not a steady trickle.
pub const BROADCAST_CAPACITY: usize = 256;

/// Events kept for replay. At the measured churn of a busy fleet — about
/// 0.64 frames a second — 512 slots is roughly thirteen minutes of history,
/// which covers a lift, a tunnel, a cell handover and an app switch. A gap
/// longer than the ring degrades to today's behaviour (a `lagged` frame and
/// a full re-list), never to something worse.
pub const REPLAY_RING: usize = 512;

/// How long after the last subscriber leaves the bus keeps recording.
///
/// [`BroadcastEventBus::emit`] returns early when nobody is listening, so a
/// hub nobody has connected a phone to pays nothing to render a feed no one
/// reads. Taken literally that would also make resume useless: the moment a
/// phone's connection drops, the receiver count is zero and the ring stops
/// filling — so there would be nothing to replay precisely when it is
/// wanted. For this long after the last subscriber, the hub keeps rendering
/// into the ring and sending nothing.
///
/// Which serves a resume no production caller can be given any more — see
/// [`REPLAY_RING`] for why that is a retention kept on purpose, and what
/// would end it.
pub const RING_GRACE_SECS: i64 = 15 * 60;

impl BroadcastEventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = tokio::sync::broadcast::channel(capacity);
        Self {
            tx,
            seq: AtomicU64::new(0),
            ring: Mutex::new(VecDeque::with_capacity(REPLAY_RING)),
            // Wall-clock nanos at construction. Not a random number, because
            // there is no rng in this crate's dependencies and none is
            // needed: the property required is "different from the last
            // process's", and two hubs starting in the same nanosecond on
            // the same machine is not a failure mode.
            generation: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(1),
            last_subscriber_at: AtomicI64::new(0),
            context_red_pct: AtomicU64::new(
                crate::service::attention::DEFAULT_CONTEXT_RED_PCT.to_bits(),
            ),
            attention: Mutex::new(AttentionInputs::default()),
        }
    }

    /// Judge `context_full` in the stamped `needs_attention` at `pct` from
    /// now on — the value of `health.context_red_pct` in force.
    pub fn set_context_red_pct(&self, pct: f64) {
        self.context_red_pct.store(pct.to_bits(), Ordering::Relaxed);
    }

    /// The threshold [`Self::set_context_red_pct`] last set.
    pub fn context_red_pct(&self) -> f64 {
        f64::from_bits(self.context_red_pct.load(Ordering::Relaxed))
    }

    /// Identifies this process's sequence; see [`EventMessage::seq`].
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The events after `seq`, oldest first, and whether the ring still
    /// reached back that far.
    ///
    /// `None` means it did not — the client was away longer than the ring is
    /// deep, or this is a different generation — and the caller must fall
    /// back to a full re-list rather than pretend continuity.
    pub fn replay_after(&self, generation: u64, seq: u64) -> Option<Vec<EventMessage>> {
        if generation != self.generation {
            return None;
        }
        let ring = self.ring.lock().ok()?;
        // Nothing recorded yet: a client that resumes at 0 has missed
        // nothing, and one that resumes at N on an empty ring has.
        let oldest = ring.front().map(|m| m.seq);
        match oldest {
            None => (seq == self.seq.load(Ordering::Relaxed)).then(Vec::new),
            // The ring must still hold the event AFTER the one the client
            // has, or there is a hole between them.
            // `seq` is the client's own `Last-Event-ID`, so `+ 1` is checked:
            // `u64::MAX` overflowed it under the ring's lock.
            Some(oldest) if seq.checked_add(1).is_some_and(|n| oldest <= n) => {
                Some(ring.iter().filter(|m| m.seq > seq).cloned().collect())
            }
            Some(_) => None,
        }
    }

    /// A receiver that sees every event emitted *after* this call. Nothing is
    /// replayed: a client that wants the current state lists it once and then
    /// follows the stream.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<EventMessage> {
        self.last_subscriber_at.store(unix_now(), Ordering::Relaxed);
        self.tx.subscribe()
    }

    /// Live subscribers. [`BroadcastEventBus::emit`] reads it to skip the
    /// work of rendering an event nobody is listening for.
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Test-only: as though the last subscriber left longer ago than
    /// [`RING_GRACE_SECS`], without waiting fifteen minutes for it.
    #[cfg(test)]
    pub(crate) fn expire_grace_for_test(&self) {
        self.last_subscriber_at
            .store(unix_now() - RING_GRACE_SECS - 1, Ordering::Relaxed);
    }
}

impl Default for BroadcastEventBus {
    fn default() -> Self {
        Self::new(BROADCAST_CAPACITY)
    }
}

impl EventBus for BroadcastEventBus {
    fn context_red_pct_changed(&self, pct: f64) {
        self.set_context_red_pct(pct);
    }

    fn attention_facts(&self) -> crate::service::attention::Facts {
        match self.attention.lock() {
            Ok(mut a) => a.facts(unix_now()),
            Err(_) => Default::default(),
        }
    }

    fn attention_seeded(&self, hosts: &[HostRow], usage: &[AccountUsageSnapshot]) {
        if let Ok(mut a) = self.attention.lock() {
            a.seed(hosts, usage);
        }
    }

    fn account_usage(&self) -> Vec<AccountUsageSnapshot> {
        match self.attention.lock() {
            Ok(a) => a.usage.values().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }

    fn emit(&self, e: &RowChange) {
        // Before anything that can return early: the facts must follow every
        // host and usage change, listened to or not.
        if let Ok(mut a) = self.attention.lock() {
            a.observe(e);
        }
        // The usual state of a hub nobody has connected a phone to. Checked
        // FIRST because `payload()` serializes a whole store row, and a
        // reconcile pass emits one per session on a busy fleet: without this,
        // every hub would pay to render a feed no one is reading. A
        // subscriber that arrives between this check and the `send` below
        // simply misses this one event, which is the same race `send` already
        // has and is exactly what "nothing is replayed" means here.
        // …with one exception, added for resume: for [`RING_GRACE_SECS`]
        // after the last subscriber left, keep recording into the ring. A
        // phone's connection dropping takes the receiver count to zero, so
        // the strict rule would stop recording exactly when a replay is
        // about to be asked for.
        let now = unix_now();
        if self.receiver_count() == 0 {
            if now - self.last_subscriber_at.load(Ordering::Relaxed) > RING_GRACE_SECS {
                // Not rendered — but not forgotten either. The event still
                // takes a number, and the ring is emptied, so a client
                // resuming from before it finds a hole (`replay_after` answers
                // `None`) and re-lists, instead of a ring that still reaches
                // back to its id and replays "nothing missed".
                if let Ok(mut ring) = self.ring.lock() {
                    ring.clear();
                    self.seq.fetch_add(1, Ordering::Relaxed);
                }
                return;
            }
        } else {
            self.last_subscriber_at.store(now, Ordering::Relaxed);
        }
        // Null keys come off before the frame is broadcast, once per event
        // rather than once per subscriber. `list_sessions` already answers
        // through `ok_json_compact`, so without this the same row arrives
        // stripped from a tool call and unstripped from the stream — and the
        // unstripped half is the one sent again on every change. Measured on
        // a 44-session fleet, nulls were 34 % of the bytes a fifteen-second
        // `/events` sample wrote. Clients deserialize an absent key and a
        // null key to the same `None` (see `crate::json`), and the desktop's
        // own Tauri bus does not come through here, so its value→null
        // clearing is untouched.
        let mut payload = e.payload();
        crate::json::strip_nulls(&mut payload);
        // The derived `needs_attention` `list_sessions` stamps, so a phone
        // that listed once and then follows this stream keeps the hub's
        // answer instead of losing it on the row's first change. Here and
        // not in `payload()`: the desktop's Tauri bus shares that, and
        // deserialises the payload straight back into `SessionRow`.
        // `context_full` is judged at `health.context_red_pct`, the same
        // threshold `list_sessions` reads — held on the bus, which has no
        // store (see `Self::context_red_pct`) — and the three `Blocked`
        // reasons from the facts the bus follows (`attention_facts`).
        if let RowChange::SessionCreated(row) | RowChange::SessionUpdated(row) = e {
            if let (Some(att), serde_json::Value::Object(map)) = (
                crate::service::attention::needs_attention_in(
                    row,
                    self.context_red_pct(),
                    &self.attention_facts(),
                ),
                &mut payload,
            ) {
                if let Ok(v) = serde_json::to_value(att) {
                    map.insert("needs_attention".to_string(), v);
                }
            }
        }
        // The number, the ring position and the live send are taken under
        // one lock, so a client resuming at N can never be sent N+1 before
        // N, and a live stream (which skips `seq <= sent_through`) never sees
        // a later number first and then drops the earlier one (review r06).
        // `send` does not block.
        let Ok(mut ring) = self.ring.lock() else {
            tracing::warn!("[events] replay ring poisoned; the stream stops recording");
            return;
        };
        let msg = EventMessage {
            name: e.name(),
            payload,
            seq: self.seq.fetch_add(1, Ordering::Relaxed) + 1,
        };
        if ring.len() == REPLAY_RING {
            ring.pop_front();
        }
        ring.push_back(msg.clone());
        // `Err` means the last receiver went away in that window. Not an
        // error, not a log line.
        let _ = self.tx.send(msg);
    }
}

/// Records every event in order. Used in unit tests to assert that a Store
/// mutation produced the expected events.
/// Crate-private: `events` is a `pub` module now, and exporting a test-only
/// helper would put it in `fleet-core`'s public API.
#[cfg(test)]
pub(crate) struct RecordingEventBus {
    pub events: std::sync::Mutex<Vec<String>>,
    /// Bare [`RowChange::name`]s in emit order, for assertions that only
    /// care which kinds of event fired.
    names: std::sync::Mutex<Vec<&'static str>>,
}

#[cfg(test)]
impl RecordingEventBus {
    pub fn new() -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
            names: std::sync::Mutex::new(Vec::new()),
        }
    }
    pub fn take(&self) -> Vec<String> {
        self.names.lock().unwrap().clear();
        std::mem::take(&mut *self.events.lock().unwrap())
    }
    /// Every event name recorded since the last [`Self::take`].
    pub fn names(&self) -> Vec<&'static str> {
        self.names.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl EventBus for RecordingEventBus {
    /// Records `<event name>:<row key>` — the same shape the store tests have
    /// always asserted on.
    fn emit(&self, e: &RowChange) {
        let key = match e {
            RowChange::SessionCreated(r) | RowChange::SessionUpdated(r) => r.id.to_string(),
            RowChange::SessionKilled(k) => k.id.to_string(),
            RowChange::WorktreeRemoved(id) | RowChange::ConversationsChanged(id) => id.to_string(),
            RowChange::SessionEventAdded(ev) => format!("{}:{}", ev.session_id, ev.kind),
            RowChange::HostAdded(r) | RowChange::HostProbed(r) => r.alias.clone(),
            RowChange::HostPinged { alias, .. } => alias.clone(),
            RowChange::HostRemoved(alias) => alias.clone(),
            RowChange::AccountUpserted(r) => r.uuid.clone(),
            RowChange::ProjectUpdated(r) => r.id.to_string(),
            RowChange::WorktreeUpdated(r) => r.id.to_string(),
            RowChange::TaskUpdated(r) => format!("{}:{}", r.id, r.state),
            RowChange::AccountUsageUpdated(r) => r.account_uuid.clone(),
            RowChange::AssetInventoryUpdated(r) => {
                format!("{}:{}:{}:{}", r.host_alias, r.harness, r.kind, r.name)
            }
            RowChange::AssetInventoryCleared {
                host_alias,
                harness,
            } => format!("{host_alias}:{harness}"),
            RowChange::CatalogLoaded(s) => s.head.clone(),
            RowChange::SyncProgress(p) => {
                format!("{}:{}:{}/{}", p.host_alias, p.harness, p.done, p.total)
            }
            RowChange::MoveProgress(p) => {
                format!("{}:{}:{}", p.session_id, p.step.as_str(), p.state.as_str())
            }
            RowChange::StartProgress(p) => {
                format!("{}:{}:{}", p.token, p.step.as_str(), p.state.as_str())
            }
            RowChange::WorkItemUpdated(r) => r.id.to_string(),
            RowChange::TrackerUpdated(r) => format!("{}:{}", r.id, r.state),
            RowChange::TrackerRemoved(id) => id.to_string(),
            RowChange::WorkChanged(w) => format!(
                "{}:{}:{:?}:{:?}",
                w.what,
                w.task_id.as_deref().unwrap_or_default(),
                w.rule_id,
                w.view_id
            ),
            RowChange::SettingsChanged(key) => key.clone(),
            RowChange::GrantChanged(g) => format!(
                "{}:{}:{}",
                g.session_id,
                g.person_id,
                g.level.as_deref().unwrap_or("revoked")
            ),
            RowChange::UpdateChanged(u) => {
                format!("{}:{}", u.what, u.target.as_deref().unwrap_or_default())
            }
            RowChange::UpdateDecision(u) => format!("{}:{}", u.target, u.status),
            RowChange::DownloadChanged(id) | RowChange::LocalWorkspaceChanged(id) => id.to_string(),
            RowChange::ConfirmChanged | RowChange::HandoffChanged => String::new(),
        };
        self.names.lock().unwrap().push(e.name());
        self.events
            .lock()
            .unwrap()
            .push(format!("{}:{key}", e.name()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontend_declares_every_event_name() {
        let events_ts = crate::repo_files::read("src/lib/events.ts");
        for name in EVENT_NAMES {
            assert!(
                events_ts.contains(&format!("'{name}'")),
                "src/lib/events.ts does not declare {name}"
            );
        }
    }

    /// `RowChange::name` for every variant a test can build without a full
    /// store row. The row-bearing variants (`SessionCreated`, `HostAdded`,
    /// `ProjectUpdated`, …) are pinned by the store tests, which assert the
    /// `RecordingEventBus` strings (`session:updated:<id>`, …) end to end.
    #[test]
    fn row_change_names_match_frontend_subscriptions() {
        let summary = CatalogSummary {
            head: String::new(),
            loaded_at: 0,
            asset_count: 0,
            problem_count: 0,
        };
        let progress = SyncProgress {
            plan_id: String::new(),
            host_alias: String::new(),
            harness: String::new(),
            done: 0,
            total: 0,
        };
        let moving = MoveProgress {
            session_id: 1,
            to_host: String::new(),
            step: MoveStep::Check,
            index: 1,
            total: 9,
            state: MoveStepState::Started,
            detail: None,
        };
        let cases: Vec<(RowChange, &str)> = vec![
            (RowChange::SessionKilled(1.into()), "session:killed"),
            (RowChange::ConversationsChanged(1), "session:conversations"),
            (RowChange::HostRemoved("h".into()), "host:removed"),
            (
                RowChange::AccountUpserted(AccountRow::default()),
                "account:upserted",
            ),
            (RowChange::WorktreeRemoved(1), "worktree:removed"),
            (
                RowChange::AssetInventoryUpdated(AssetInventoryRow::default()),
                "asset_inventory:updated",
            ),
            (
                RowChange::AssetInventoryCleared {
                    host_alias: "h".into(),
                    harness: "claude".into(),
                },
                "asset_inventory:cleared",
            ),
            (RowChange::CatalogLoaded(summary), "catalog:loaded"),
            (RowChange::SyncProgress(progress), "sync:progress"),
            (RowChange::MoveProgress(moving), "move:progress"),
            (
                RowChange::SettingsChanged("gc.enabled".into()),
                "settings:changed",
            ),
        ];
        for (change, expected) in &cases {
            assert_eq!(change.name(), *expected);
            assert!(EVENT_NAMES.contains(expected));
        }
    }

    /// True when `EVENT_KINDS` lists the part of `name` before the `:`.
    /// A `const fn` so it can be evaluated at COMPILE time; const eval has no
    /// formatting, so the caller's `const` item is what names the offender in
    /// the compiler's message.
    const fn kind_is_listed(name: &str) -> bool {
        let nb = name.as_bytes();
        let mut n = 0;
        while n < nb.len() && nb[n] != b':' {
            n += 1;
        }
        if n == nb.len() {
            return false; // no `:` at all — not an event name
        }
        let mut k = 0;
        while k < EVENT_KINDS.len() {
            let kb = EVENT_KINDS[k].as_bytes();
            if kb.len() == n {
                let mut i = 0;
                while i < n && kb[i] == nb[i] {
                    i += 1;
                }
                if i == n {
                    return true;
                }
            }
            k += 1;
        }
        false
    }

    /// True when `EVENT_NAMES` contains `name` exactly.
    const fn name_is_declared(name: &str) -> bool {
        let nb = name.as_bytes();
        let mut i = 0;
        while i < EVENT_NAMES.len() {
            let fb = EVENT_NAMES[i].as_bytes();
            if fb.len() == nb.len() {
                let mut j = 0;
                while j < nb.len() && fb[j] == nb[j] {
                    j += 1;
                }
                if j == nb.len() {
                    return true;
                }
            }
            i += 1;
        }
        false
    }

    /// An event name checked against both lists while the crate compiles.
    /// A name whose kind is not in [`EVENT_KINDS`] (it would be
    /// unsubscribable on `/events`) or which `EVENT_NAMES` does not
    /// declare fails const evaluation — a compile error, not a test failure.
    macro_rules! pinned_name {
        ($name:literal) => {{
            const N: &str = {
                assert!(
                    kind_is_listed($name),
                    "EVENT_KINDS has no kind for this event name"
                );
                assert!(
                    name_is_declared($name),
                    "EVENT_NAMES does not declare this event name"
                );
                $name
            };
            N
        }};
    }

    /// The compile-time pin for `/events` subscribability.
    ///
    /// `EVENT_KINDS` used to be cross-checked only against the hardcoded
    /// `EVENT_NAMES` fixture, never against `RowChange` itself: a new
    /// variant added without touching that fixture compiled, passed the suite,
    /// and was silently unsubscribable (`?kinds=<its kind>` would be logged as
    /// unrecognised and drop every one of its events).
    ///
    /// The `match` below is exhaustive, so **a new variant does not compile**
    /// until it has an arm here; the arm's name is checked against both lists
    /// at compile time by `pinned_name!`. Adding a variant therefore forces
    /// `EVENT_KINDS`, `EVENT_NAMES` and `RowChange::name` into step
    /// before anything builds.
    #[test]
    fn every_row_change_variant_is_subscribable() {
        fn pinned(c: &RowChange) -> &'static str {
            match c {
                RowChange::HostPinged { .. } => pinned_name!("host:pinged"),
                RowChange::SessionCreated(_) => pinned_name!("session:created"),
                RowChange::SessionUpdated(_) => pinned_name!("session:updated"),
                RowChange::SessionKilled(_) => pinned_name!("session:killed"),
                RowChange::SessionEventAdded(_) => pinned_name!("session:event"),
                RowChange::ConversationsChanged(_) => pinned_name!("session:conversations"),
                RowChange::HostAdded(_) => pinned_name!("host:added"),
                RowChange::HostProbed(_) => pinned_name!("host:probed"),
                RowChange::HostRemoved(_) => pinned_name!("host:removed"),
                RowChange::AccountUpserted(_) => pinned_name!("account:upserted"),
                RowChange::ProjectUpdated(_) => pinned_name!("project:updated"),
                RowChange::WorktreeUpdated(_) => pinned_name!("worktree:updated"),
                RowChange::WorktreeRemoved(_) => pinned_name!("worktree:removed"),
                RowChange::TaskUpdated(_) => pinned_name!("task:updated"),
                RowChange::AccountUsageUpdated(_) => pinned_name!("account_usage:updated"),
                RowChange::AssetInventoryUpdated(_) => pinned_name!("asset_inventory:updated"),
                RowChange::AssetInventoryCleared { .. } => pinned_name!("asset_inventory:cleared"),
                RowChange::CatalogLoaded(_) => pinned_name!("catalog:loaded"),
                RowChange::SyncProgress(_) => pinned_name!("sync:progress"),
                RowChange::MoveProgress(_) => pinned_name!("move:progress"),
                RowChange::StartProgress(_) => pinned_name!("start:progress"),
                RowChange::WorkItemUpdated(_) => pinned_name!("work:item"),
                RowChange::TrackerUpdated(_) => pinned_name!("work:tracker"),
                RowChange::TrackerRemoved(_) => pinned_name!("work:tracker_removed"),
                RowChange::WorkChanged(_) => pinned_name!("work:changed"),
                RowChange::SettingsChanged(_) => pinned_name!("settings:changed"),
                RowChange::GrantChanged(_) => pinned_name!("grant:changed"),
                RowChange::UpdateChanged(_) => pinned_name!("update:changed"),
                RowChange::UpdateDecision(_) => pinned_name!("update:decision"),
                RowChange::DownloadChanged(_) => pinned_name!("download:changed"),
                RowChange::LocalWorkspaceChanged(_) => pinned_name!("local_workspace:changed"),
                RowChange::ConfirmChanged => pinned_name!("confirm:changed"),
                RowChange::HandoffChanged => pinned_name!("handoff:changed"),
            }
        }
        // And for every variant a test can build without a full store row,
        // `RowChange::name` really is the name pinned above.
        for c in [
            RowChange::SessionKilled(1.into()),
            RowChange::ConversationsChanged(1),
            RowChange::HostRemoved("h".into()),
            RowChange::AccountUpserted(AccountRow::default()),
            RowChange::WorktreeRemoved(1),
            RowChange::TrackerRemoved(1),
            RowChange::AssetInventoryUpdated(AssetInventoryRow::default()),
            RowChange::AssetInventoryCleared {
                host_alias: "h".into(),
                harness: "claude".into(),
            },
        ] {
            assert_eq!(c.name(), pinned(&c));
        }
    }

    /// A new `RowChange` variant whose kind is missing here would be
    /// unfilterable: `?kinds=<it>` would be logged as unrecognised and drop
    /// every one of its events.
    #[test]
    fn event_kinds_cover_every_name() {
        for name in EVENT_NAMES {
            let kind = name.split_once(':').expect("every name has a kind").0;
            assert!(
                EVENT_KINDS.contains(&kind),
                "EVENT_KINDS is missing {kind} (from {name})"
            );
        }
        for kind in EVENT_KINDS {
            assert!(
                EVENT_NAMES
                    .iter()
                    .any(|n| n.starts_with(&format!("{kind}:"))),
                "EVENT_KINDS lists {kind}, which no event uses"
            );
        }
    }

    #[test]
    fn scalar_payloads_keep_their_wire_shape() {
        // An id-only kill still serialises to exactly what every client
        // has always read — the four added keys are `skip_serializing_if`.
        assert_eq!(
            RowChange::SessionKilled(7.into()).payload(),
            serde_json::json!({ "id": 7 })
        );
        assert_eq!(
            RowChange::SessionKilled(SessionKilledPayload {
                id: 7,
                host_alias: Some("box".into()),
                org_id: None,
                visibility: Some("private".into()),
                owner_person_id: Some(2),
            })
            .payload(),
            serde_json::json!({
                "id": 7, "host_alias": "box",
                "visibility": "private", "owner_person_id": 2
            })
        );
        assert_eq!(
            RowChange::GrantChanged(GrantChanged {
                session_id: 7,
                person_id: 2,
                level: Some("watch".into()),
            })
            .payload(),
            serde_json::json!({ "session_id": 7, "person_id": 2, "level": "watch" })
        );
        // A revoke is the same frame with a null level.
        assert_eq!(
            RowChange::GrantChanged(GrantChanged {
                session_id: 7,
                person_id: 2,
                level: None,
            })
            .payload(),
            serde_json::json!({ "session_id": 7, "person_id": 2, "level": null })
        );
        assert_eq!(
            RowChange::ConversationsChanged(4).payload(),
            serde_json::json!({ "session_id": 4 })
        );
        assert_eq!(
            RowChange::WorktreeRemoved(3).payload(),
            serde_json::json!({ "id": 3 })
        );
        assert_eq!(
            RowChange::HostRemoved("box".into()).payload(),
            serde_json::json!({ "alias": "box" })
        );
        assert_eq!(
            RowChange::AssetInventoryCleared {
                host_alias: "box".into(),
                harness: "claude".into(),
            }
            .payload(),
            serde_json::json!({ "host_alias": "box", "harness": "claude" })
        );
    }

    #[tokio::test]
    async fn a_subscriber_receives_emitted_row_changes() {
        let bus = BroadcastEventBus::new(16);
        let mut rx = bus.subscribe();
        bus.emit(&RowChange::SessionKilled(42.into()));
        let msg = rx.recv().await.expect("one message");
        assert_eq!(msg.name, "session:killed");
        assert_eq!(msg.payload["id"], 42);
    }

    /// The no-subscriber guard must not cost a live subscriber an event: the
    /// receiver is taken before the emit, which is the only ordering the
    /// stream ever uses (the route subscribes before its first frame).
    #[tokio::test]
    async fn a_live_subscriber_still_gets_everything_after_the_guard() {
        let bus = BroadcastEventBus::new(16);
        assert_eq!(bus.receiver_count(), 0, "nobody is listening yet");
        let mut rx = bus.subscribe();
        assert_eq!(bus.receiver_count(), 1);
        bus.emit(&RowChange::SessionKilled(1.into()));
        bus.emit(&RowChange::HostRemoved("box".into()));
        assert_eq!(rx.recv().await.unwrap().name, "session:killed");
        assert_eq!(rx.recv().await.unwrap().name, "host:removed");
        // A receiver that goes away takes the count with it, and emitting is
        // still fine.
        drop(rx);
        assert_eq!(bus.receiver_count(), 0);
        bus.emit(&RowChange::SessionKilled(2.into()));
    }

    /// Review r06: a live stream drops any `seq` at or below the last one it
    /// sent, so emitters on several threads must reach a subscriber in
    /// number order, or the earlier event is skipped for good.
    #[test]
    fn concurrent_emitters_reach_a_subscriber_in_number_order() {
        const THREADS: usize = 8;
        const EACH: usize = 2_000;
        let bus = std::sync::Arc::new(BroadcastEventBus::new(THREADS * EACH));
        let mut rx = bus.subscribe();
        let handles: Vec<_> = (0..THREADS)
            .map(|t| {
                let bus = std::sync::Arc::clone(&bus);
                std::thread::spawn(move || {
                    for i in 0..EACH {
                        bus.emit(&RowChange::SessionKilled(((t * EACH + i) as i64).into()));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let mut last = 0;
        for _ in 0..THREADS * EACH {
            let msg = rx.try_recv().expect("every event was sent");
            assert!(msg.seq > last, "seq {} arrived after {last}", msg.seq);
            last = msg.seq;
        }
    }

    /// The stream and the tool boundary must agree about what a null field
    /// looks like on the wire: `list_sessions` answers through
    /// `ok_json_compact`, and the frame for one of its rows goes out here.
    /// Clients give every optional field a default precisely because of it.
    #[tokio::test]
    async fn a_broadcast_frame_carries_no_null_keys() {
        let bus = BroadcastEventBus::new(4);
        let mut rx = bus.subscribe();
        bus.emit(&RowChange::SessionEventAdded(crate::store::SessionEvent {
            id: 2_125_930,
            session_id: 21_340,
            at: 1_790_027_305,
            kind: "mcp_call".into(),
            detail: Some("list_sessions".into()),
            // The field that was 396 bytes of `"claude_session_id":null`
            // across one fifteen-second sample of the live hub.
            claude_session_id: None,
        }));
        let msg = rx.recv().await.unwrap();
        assert_eq!(msg.name, "session:event");
        assert!(
            msg.payload.get("claude_session_id").is_none(),
            "a null field must not reach the wire: {}",
            msg.payload
        );
        assert_eq!(
            msg.payload.get("detail").and_then(|v| v.as_str()),
            Some("list_sessions"),
            "a field that has a value is untouched"
        );
    }

    /// A phone follows `session:updated` for hours after one `list_sessions`,
    /// so a `needs_attention` that only the list carried would vanish from
    /// the phone's row at the first change — and the phone would be back to
    /// deciding it for itself, the second rule `service::attention` exists to
    /// remove. The stream stamps it exactly as `list_sessions` does, and
    /// leaves it off a row that needs nobody.
    #[tokio::test]
    async fn a_session_frame_carries_the_hubs_needs_attention() {
        let s = crate::store::Store::open_in_memory().unwrap();
        s.upsert_host("hosta").unwrap();
        s.upsert_session("dev", "hosta", None, None, 1, 1, "running", None)
            .unwrap();
        let calm = s.get_session("dev", "hosta").unwrap().expect("row");
        let mut blocked = calm.clone();
        blocked.claude_status = Some("blocked".into());

        let bus = BroadcastEventBus::new(4);
        let mut rx = bus.subscribe();
        bus.emit(&RowChange::SessionUpdated(blocked.clone()));
        bus.emit(&RowChange::SessionCreated(blocked));
        bus.emit(&RowChange::SessionUpdated(calm));

        for name in ["session:updated", "session:created"] {
            let msg = rx.recv().await.unwrap();
            assert_eq!(msg.name, name);
            let att = msg.payload.get("needs_attention").expect("stamped");
            assert_eq!(att["reason"], "waiting", "{}", msg.payload);
            assert_eq!(att["since"], 1, "{}", msg.payload);
        }
        let msg = rx.recv().await.unwrap();
        assert!(
            msg.payload.get("needs_attention").is_none(),
            "a session that needs nobody carries no key: {}",
            msg.payload
        );
    }

    /// The three `Blocked` reasons on the stream (step 2.6): the bus follows
    /// hosts and usage off the frames it carries, so a session on a host a
    /// probe found down reads `host_down`, one on an account at its limit
    /// `account_limit`, each with `state: blocked` — and both lift on the
    /// frame that says so, a heartbeat included.
    #[tokio::test]
    async fn the_stream_follows_down_hosts_and_limited_accounts() {
        use crate::service::account_usage::{
            AccountUsage, AccountUsageSnapshot, UsageOutcomeKind, Window,
        };
        let s = crate::store::Store::open_in_memory().unwrap();
        s.upsert_host("hosta").unwrap();
        s.upsert_session("dev", "hosta", None, None, 1, 1, "running", None)
            .unwrap();
        let mut row = s.get_session("dev", "hosta").unwrap().expect("row");
        row.claude_status = Some("idle".into());
        row.account_uuid = Some("acc".into());
        let mut host = s.list_hosts().unwrap().remove(0);

        let bus = BroadcastEventBus::new(16);
        bus.attention_seeded(std::slice::from_ref(&host), &[]);
        let mut rx = bus.subscribe();
        let mut attention = |bus: &BroadcastEventBus| {
            bus.emit(&RowChange::SessionUpdated(row.clone()));
            loop {
                let msg = rx.try_recv().expect("a frame");
                if msg.name == "session:updated" {
                    return msg.payload.get("needs_attention").cloned();
                }
            }
        };
        assert_eq!(attention(&bus), None, "a reachable host, no usage known");

        host.reachable = false;
        host.last_pinged_at = Some(50);
        bus.emit(&RowChange::HostProbed(host.clone()));
        let att = attention(&bus).expect("stamped");
        assert_eq!(att["reason"], "host_down", "{att}");
        assert_eq!(att["state"], "blocked", "{att}");

        bus.emit(&RowChange::HostPinged {
            alias: "hosta".into(),
            last_pinged_at: 60,
            reachable: true,
            claude_version_at: None,
            health: None,
        });
        assert_eq!(attention(&bus), None, "the heartbeat that says it is back");

        let snap = |pct: f64| AccountUsageSnapshot {
            account_uuid: "acc".into(),
            usage: Some(AccountUsage {
                five_hour: Some(Window {
                    utilization: pct,
                    resets_at: Some(unix_now() + 3_600),
                }),
                ..Default::default()
            }),
            subscription: None,
            fetched_at: Some(unix_now()),
            source_host: None,
            status: UsageOutcomeKind::Ok,
            detail: None,
            next_try_at: 0,
        };
        bus.emit(&RowChange::AccountUsageUpdated(snap(100.0)));
        let att = attention(&bus).expect("stamped");
        assert_eq!(att["reason"], "account_limit", "{att}");
        assert_eq!(att["state"], "blocked", "{att}");
        assert!(bus.attention_facts().limited_accounts.contains_key("acc"));

        bus.emit(&RowChange::AccountUsageUpdated(snap(40.0)));
        assert_eq!(attention(&bus), None, "the window has room again");
    }

    /// A store reads its bus's facts, and a read pool following the bus reads
    /// the same: the sites that decide `needs_attention` off a pooled
    /// connection must not fall back to the row-only answer (step 2.6).
    #[test]
    fn a_store_and_its_read_pool_read_the_bus_facts() {
        let bus = std::sync::Arc::new(BroadcastEventBus::new(4));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let s = crate::store::Store::open_with_bus(&path, bus.clone()).unwrap();
        s.upsert_host("hosta").unwrap();
        s.update_host_probe("hosta", false, None, None, 50).unwrap();
        assert!(s.attention_facts().down_hosts.contains("hosta"));

        let pool = crate::store::ReadPool::open(&path, 2)
            .unwrap()
            .expect("WAL")
            .following(bus as std::sync::Arc<dyn EventBus>);
        let reader = pool.get().expect("a connection");
        assert!(reader
            .lock()
            .unwrap()
            .attention_facts()
            .down_hosts
            .contains("hosta"));
    }

    /// A row at 90 % context, live and working.
    fn ninety_percent_row() -> SessionRow {
        let s = crate::store::Store::open_in_memory().unwrap();
        s.upsert_host("hosta").unwrap();
        s.upsert_session("dev", "hosta", None, None, 1, 1, "running", None)
            .unwrap();
        let mut row = s.get_session("dev", "hosta").unwrap().expect("row");
        row.claude_status = Some("working".into());
        row.context_pct = Some(90.0);
        row
    }

    /// `list_sessions` judges `context_full` at `health.context_red_pct`, so
    /// the stream must too: at 95 a 90 % row needs nobody, at the default 85
    /// it is `context_full` — or a phone following `/events` would disagree
    /// with the list it just fetched.
    #[tokio::test]
    async fn the_stream_judges_context_full_at_the_bus_threshold() {
        let row = ninety_percent_row();

        let bus = BroadcastEventBus::new(4);
        let default = crate::service::attention::DEFAULT_CONTEXT_RED_PCT;
        assert_eq!(bus.context_red_pct(), default);
        let mut rx = bus.subscribe();
        bus.emit(&RowChange::SessionUpdated(row.clone()));
        let msg = rx.recv().await.unwrap();
        let reason = &msg.payload["needs_attention"]["reason"];
        assert_eq!(reason, "context_full", "{}", msg.payload);

        bus.set_context_red_pct(95.0);
        bus.emit(&RowChange::SessionUpdated(row));
        let msg = rx.recv().await.unwrap();
        assert!(
            msg.payload.get("needs_attention").is_none(),
            "90 % is under a 95 % threshold: {}",
            msg.payload
        );
    }

    /// A write of `health.context_red_pct` through `settings::set` reaches
    /// the store's bus, so a running hub's stream follows the setting
    /// without a restart.
    #[tokio::test]
    async fn setting_the_threshold_updates_the_bus() {
        use crate::service::settings;
        let bus = std::sync::Arc::new(BroadcastEventBus::new(4));
        let dyn_bus: std::sync::Arc<dyn EventBus> = bus.clone();
        let s = crate::store::Store::open_with_bus_in_memory(dyn_bus).unwrap();

        settings::set(&s, settings::HEALTH_CONTEXT_RED_PCT, "95").unwrap();
        assert_eq!(bus.context_red_pct(), 95.0);

        let mut rx = bus.subscribe();
        bus.emit(&RowChange::SessionUpdated(ninety_percent_row()));
        let msg = rx.recv().await.unwrap();
        assert!(
            msg.payload.get("needs_attention").is_none(),
            "{}",
            msg.payload
        );

        settings::set(&s, settings::HEALTH_CONTEXT_RED_PCT, "70").unwrap();
        assert_eq!(bus.context_red_pct(), 70.0);
    }

    /// Replay is what makes a reconnect cost the events missed rather than a
    /// full re-list. The sequence is the contract: it starts at 1, increases
    /// by one, and the ring hands back exactly what came after a given point.
    #[tokio::test]
    async fn the_ring_replays_exactly_what_came_after_a_sequence_number() {
        let bus = BroadcastEventBus::new(16);
        let _rx = bus.subscribe();
        for i in 1..=4 {
            bus.emit(&RowChange::SessionKilled(i.into()));
        }

        let after_two = bus
            .replay_after(bus.generation(), 2)
            .expect("still in the ring");
        assert_eq!(
            after_two.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![3, 4],
            "the events after 2, in order, and nothing else"
        );
        assert!(
            bus.replay_after(bus.generation(), 4).unwrap().is_empty(),
            "a client that is fully caught up has missed nothing"
        );
    }

    /// The resume point is the client's own number: `u64::MAX` overflowed
    /// `seq + 1` while the ring's lock was held, poisoning it for every
    /// later emit. It is refused, and the ring keeps recording.
    #[tokio::test]
    async fn a_resume_point_at_u64_max_is_refused_without_poisoning_the_ring() {
        let bus = BroadcastEventBus::new(16);
        let _rx = bus.subscribe();
        bus.emit(&RowChange::SessionKilled(1.into()));
        assert!(bus.replay_after(bus.generation(), u64::MAX).is_none());
        bus.emit(&RowChange::SessionKilled(2.into()));
        let after = bus
            .replay_after(bus.generation(), 1)
            .expect("still recording");
        assert_eq!(after.iter().map(|m| m.seq).collect::<Vec<_>>(), vec![2]);
    }

    /// A restarted hub counts from 1 again, so an id minted by the last
    /// process names events this one never sent. Replaying them under the
    /// right numbers would be the worst outcome — a client convinced it is
    /// current while it is not — so the generation refuses instead, and the
    /// caller re-lists.
    #[tokio::test]
    async fn a_different_generation_is_refused_rather_than_replayed() {
        let bus = BroadcastEventBus::new(16);
        let _rx = bus.subscribe();
        bus.emit(&RowChange::SessionKilled(1.into()));

        assert!(bus.replay_after(bus.generation() ^ 0xffff, 0).is_none());
    }

    /// A gap longer than the ring cannot be filled, and saying so is the
    /// point: the alternative is a client that silently skipped events.
    #[tokio::test]
    async fn a_gap_longer_than_the_ring_is_refused() {
        let bus = BroadcastEventBus::new(4096);
        let _rx = bus.subscribe();
        for i in 0..(REPLAY_RING as i64 + 10) {
            bus.emit(&RowChange::SessionKilled(i.into()));
        }

        assert!(
            bus.replay_after(bus.generation(), 1).is_none(),
            "event 2 has fallen out of the ring"
        );
        let recent = bus.replay_after(bus.generation(), REPLAY_RING as u64 + 5);
        assert_eq!(recent.expect("still held").len(), 5);
    }

    /// The reason the grace window exists: a phone's connection dropping
    /// takes the receiver count to zero, and the strict "render nothing when
    /// nobody listens" rule would then stop recording exactly when the replay
    /// is about to be asked for.
    #[tokio::test]
    async fn the_ring_keeps_recording_just_after_the_last_subscriber_leaves() {
        let bus = BroadcastEventBus::new(16);
        let rx = bus.subscribe();
        bus.emit(&RowChange::SessionKilled(1.into()));
        drop(rx);
        assert_eq!(bus.receiver_count(), 0);

        bus.emit(&RowChange::SessionKilled(2.into()));

        let missed = bus
            .replay_after(bus.generation(), 1)
            .expect("still in the ring");
        assert_eq!(
            missed.len(),
            1,
            "the event that arrived while the phone was away is replayable"
        );
    }

    /// Past the grace window the bus stops recording — and an event it does
    /// not record must still count as one a resuming client missed. It used
    /// to be dropped without a sequence number, so the ring still reached
    /// back to the client's id and the replay answered "nothing missed": a
    /// phone away for twenty minutes resumed as current and never re-listed
    /// the session that was killed while it was gone.
    #[tokio::test]
    async fn an_event_dropped_after_the_grace_window_refuses_a_resume_from_before_it() {
        let bus = BroadcastEventBus::new(16);
        let rx = bus.subscribe();
        bus.emit(&RowChange::SessionKilled(1.into()));
        drop(rx);
        bus.expire_grace_for_test();

        // Nobody listening and the window gone: not recorded.
        bus.emit(&RowChange::SessionKilled(2.into()));

        assert!(
            bus.replay_after(bus.generation(), 1).is_none(),
            "a client that saw 1 missed 2, so its resume must be refused"
        );
        // A fresh subscriber is unaffected, and what follows is replayable
        // again from the ids it hands out.
        let _rx = bus.subscribe();
        bus.emit(&RowChange::SessionKilled(3.into()));
        let after = bus.replay_after(bus.generation(), 3);
        assert_eq!(after.map(|v| v.len()), Some(0), "caught up at 3");
    }

    #[tokio::test]
    async fn emitting_without_subscribers_is_not_an_error() {
        let bus = BroadcastEventBus::new(4);
        bus.emit(&RowChange::SessionKilled(1.into())); // must not panic
    }

    #[tokio::test]
    async fn a_lagging_subscriber_reports_lag_rather_than_stalling_the_bus() {
        let bus = BroadcastEventBus::new(2);
        let mut rx = bus.subscribe();
        for i in 0..5 {
            bus.emit(&RowChange::SessionKilled(i.into()));
        }
        let err = rx.recv().await.unwrap_err();
        assert!(matches!(
            err,
            tokio::sync::broadcast::error::RecvError::Lagged(_)
        ));
    }

    #[test]
    fn typed_methods_route_through_emit() {
        let bus = RecordingEventBus::new();
        bus.session_killed(5.into());
        bus.host_removed("box");
        bus.worktree_removed(9);
        bus.asset_inventory_cleared("box", "claude");
        assert_eq!(
            bus.take(),
            vec![
                "session:killed:5",
                "host:removed:box",
                "worktree:removed:9",
                "asset_inventory:cleared:box:claude",
            ]
        );
    }

    #[test]
    fn move_steps_are_nine_in_order_and_serialize_as_their_names() {
        let names: Vec<&str> = MoveStep::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            names,
            [
                "check",
                "transcript",
                "workspace",
                "git",
                "replay",
                "ignored",
                "claude_state",
                "start",
                "handoff"
            ]
        );
        for (i, step) in MoveStep::ALL.iter().enumerate() {
            assert_eq!(step.index() as usize, i + 1);
            assert_eq!(
                serde_json::to_value(step).unwrap(),
                serde_json::json!(step.as_str())
            );
        }
        for state in [
            MoveStepState::Started,
            MoveStepState::Done,
            MoveStepState::Warned,
            MoveStepState::Failed,
        ] {
            assert_eq!(
                serde_json::to_value(state).unwrap(),
                serde_json::json!(state.as_str())
            );
        }
    }

    #[test]
    fn move_progress_keeps_its_wire_shape() {
        let p = MoveProgress {
            session_id: 7,
            to_host: "beta".into(),
            step: MoveStep::Git,
            index: 4,
            total: 9,
            state: MoveStepState::Done,
            detail: Some("2 commits".into()),
        };
        let change = RowChange::MoveProgress(p.clone());
        assert_eq!(change.name(), "move:progress");
        assert_eq!(
            change.payload(),
            serde_json::json!({
                "session_id": 7, "to_host": "beta", "step": "git", "index": 4,
                "total": 9, "state": "done", "detail": "2 commits"
            })
        );
        let back: MoveProgress = serde_json::from_value(change.payload()).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn start_progress_keeps_its_wire_shape() {
        let p = StartProgress {
            token: "st-1".into(),
            step: StartStep::Tmux,
            index: StartStep::Tmux.index(),
            total: 3,
            state: MoveStepState::Started,
        };
        let change = RowChange::StartProgress(p.clone());
        assert_eq!(change.name(), "start:progress");
        assert_eq!(
            change.payload(),
            serde_json::json!({
                "token": "st-1", "step": "tmux", "index": 2, "total": 3, "state": "started"
            })
        );
        let back: StartProgress = serde_json::from_value(change.payload()).unwrap();
        assert_eq!(back, p);
        for step in StartStep::ALL {
            assert_eq!(
                serde_json::to_value(step).unwrap(),
                serde_json::json!(step.as_str())
            );
        }
    }

    /// `start_steps.ts` names the same three start steps, in order.
    #[test]
    fn frontend_declares_the_start_steps_in_order() {
        let ts = crate::repo_files::read("src/lib/start_steps.ts");
        let line = ts
            .lines()
            .find(|l| l.contains("export const START_STEPS"))
            .expect("start_steps.ts declares START_STEPS");
        let want = StartStep::ALL
            .iter()
            .map(|s| format!("'{}'", s.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        assert!(line.contains(&format!("[{want}]")), "{line}");
    }

    /// The frontend's step list is the same nine names, in the same order.
    /// `moveProgress.ts` brackets the list with two marker comments so this
    /// test reads the list and nothing else.
    #[test]
    fn frontend_declares_the_move_steps_in_order() {
        let ts = crate::repo_files::read("src/lib/moveProgress.ts");
        let begin = ts.find("// move-steps:begin").expect("begin marker");
        let end = ts.find("// move-steps:end").expect("end marker");
        let quoted: Vec<&str> = ts[begin..end].split('\'').skip(1).step_by(2).collect();
        let want: Vec<&str> = MoveStep::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(quoted, want);
    }
}
