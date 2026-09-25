//! The tracker sync tick (work graph M3.3): poll, don't subscribe.
//!
//! Started only by a process that owns its fleet — `FleetTasks::
//! start_tracker_sync` on a standalone desktop, and `fleet-hub serve` — so a
//! paired desktop never becomes a second brain for the hub's trackers
//! (review C20; `tests_startup.rs` pins it). Its interval is
//! `work.sync_interval_secs` (default 300; `0` turns it off).
//!
//! One pass, per tracker, sequentially (single-flight across passes):
//!
//! 1. every enabled view, from its watermark minus [`OVERLAP_SECS`] — or
//!    whole, when it has none or its last whole listing is older than
//!    [`FULL_EVERY_SECS`] (favourite-filter membership is exact only then);
//! 2. every linked item by id (so a ticket that left every view still
//!    refreshes), capped at [`LINKED_MAX`];
//! 3. bare keys with this tracker's prefixes (typed before it was
//!    connected), by key, capped at [`UNBOUND_MAX`] — a key the tracker does
//!    not know is not asked again for [`NEGATIVE_TTL_SECS`];
//! 4. upsert by `(tracker_id, external_id)`, deduped on `(id, updated)`
//!    inside the pass; ids a by-id fetch misses become *unavailable*;
//! 5. bind bare refs, journal status moves, stamp `last_sync_at`.
//!
//! Which trackers run: `ok`, `rate_limited` and `unreachable` (transient) —
//! not `auth_failed` or `captcha` (a person has to act; `test` resets
//! them), nor `unconfigured`. A 429 waits out `Retry-After` plus jitter.
//! Events only on a real change: the store compares before it writes.

use super::{
    list_all, needs_credential, provider_for, Fetched, Incremental, ItemRef, TrackerError,
    TrackerNet, TrackerProvider, ViewDef, WorkItemSnapshot,
};
use crate::ipc_error::lock;
use crate::store::{Store, TrackerItemWrite, TrackerRow};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// `work.sync_interval_secs` default.
pub const DEFAULT_INTERVAL_SECS: u64 = 300;
/// A watermark window is widened by this much (C27: minute precision, and
/// the tracker's search is eventually consistent).
pub const OVERLAP_SECS: i64 = 120;
/// A view is listed whole at least this often.
pub const FULL_EVERY_SECS: i64 = 3600;
/// Most linked items refreshed by id per pass.
pub const LINKED_MAX: i64 = 500;
/// Most bare keys looked up per pass.
pub const UNBOUND_MAX: usize = 100;
/// A key the tracker did not know is not asked again for this long.
pub const NEGATIVE_TTL_SECS: i64 = 3600;
/// Wait after a 429 with no `Retry-After`.
pub const DEFAULT_RETRY_SECS: u64 = 60;
/// Longest a 429's `Retry-After` parks a tracker: a tracker (or anyone
/// answering for one) must not be able to park the sync until a restart.
pub const MAX_RETRY_SECS: u64 = 3600;

/// States a pass runs for.
pub fn runnable(state: &str) -> bool {
    matches!(state, "ok" | "rate_limited" | "unreachable")
}

/// What one tracker's pass did (for logs and tests).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackerPass {
    pub tracker_id: i64,
    pub seen: usize,
    pub changed: usize,
    pub unavailable: usize,
    pub bound_sessions: usize,
    pub disabled_views: Vec<String>,
    pub error: Option<String>,
    pub skipped: bool,
}

/// The sync's in-memory state: single-flight, back-off, the last whole
/// listing per view, and keys a tracker said it does not know.
pub struct TrackerSync {
    net: TrackerNet,
    running: AtomicBool,
    not_before: Mutex<HashMap<i64, i64>>,
    last_full: Mutex<HashMap<(i64, String), i64>>,
    unknown_keys: Mutex<HashMap<(i64, String), i64>>,
    clock: fn() -> i64,
}

fn now() -> i64 {
    crate::service::catalog::now_secs()
}

/// Up to a quarter of `secs` (at least 0..5 s) of jitter, so a fleet of
/// hubs rate-limited together does not come back together.
fn jitter(secs: u64) -> u64 {
    let span = (secs / 4).max(5);
    use rand::RngExt as _;
    rand::rng().random_range(0..=span)
}

impl TrackerSync {
    pub fn new(net: TrackerNet) -> Self {
        TrackerSync {
            net,
            running: AtomicBool::new(false),
            not_before: Mutex::new(HashMap::new()),
            last_full: Mutex::new(HashMap::new()),
            unknown_keys: Mutex::new(HashMap::new()),
            clock: now,
        }
    }

    #[cfg(test)]
    fn with_clock(mut self, clock: fn() -> i64) -> Self {
        self.clock = clock;
        self
    }

    /// One pass over every runnable tracker. `None` when a pass is already
    /// running (single-flight).
    pub async fn run_pass(&self, store: &Mutex<Store>) -> Option<Vec<TrackerPass>> {
        if self.running.swap(true, Ordering::AcqRel) {
            return None;
        }
        struct Reset<'a>(&'a AtomicBool);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _reset = Reset(&self.running);
        let trackers = match lock(store).and_then(|s| s.list_trackers()) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("tracker sync: cannot list trackers: {e}");
                return Some(Vec::new());
            }
        };
        let mut out = Vec::new();
        for t in trackers {
            out.push(self.sync_tracker(&t, store).await);
        }
        Some(out)
    }

    /// One tracker's pass; failures are recorded on the tracker, never
    /// returned: one bad tracker must not stop the others.
    pub async fn sync_tracker(&self, t: &TrackerRow, store: &Mutex<Store>) -> TrackerPass {
        let mut pass = TrackerPass {
            tracker_id: t.id,
            ..Default::default()
        };
        let now = (self.clock)();
        let waiting = self
            .not_before
            .lock()
            .ok()
            .and_then(|m| m.get(&t.id).copied())
            .is_some_and(|nb| nb > now);
        if !runnable(&t.state) || waiting {
            pass.skipped = true;
            return pass;
        }
        match self.run_tracker(t, store, now, &mut pass).await {
            Ok(()) => {
                if let Ok(mut m) = self.not_before.lock() {
                    m.remove(&t.id);
                }
                if let Ok(s) = lock(store) {
                    let changed = s.set_tracker_state(t.id, "ok", None).unwrap_or(false);
                    let first = s.set_tracker_synced(t.id, now).unwrap_or(false);
                    if changed || first {
                        let _ = s.emit_tracker(t.id);
                    }
                }
            }
            Err(e) => {
                pass.error = Some(e.explain());
                if let TrackerError::RateLimited { retry_after_secs } = &e {
                    let wait = retry_after_secs
                        .unwrap_or(DEFAULT_RETRY_SECS)
                        .min(MAX_RETRY_SECS);
                    let deadline = now.saturating_add(wait.saturating_add(jitter(wait)) as i64);
                    if let Ok(mut m) = self.not_before.lock() {
                        m.insert(t.id, deadline);
                    }
                }
                tracing::warn!(tracker_id = t.id, result = ?e.state(), "tracker sync failed");
                if let Ok(s) = lock(store) {
                    let _ = super::admin::record_failure(&s, t.id, &e);
                }
            }
        }
        pass
    }

    async fn run_tracker(
        &self,
        t: &TrackerRow,
        store: &Mutex<Store>,
        now: i64,
        pass: &mut TrackerPass,
    ) -> Result<(), TrackerError> {
        let (cred, views) = {
            let s = lock(store).map_err(|e| TrackerError::Invalid(e.message))?;
            (
                s.resolve_tracker_credential(t.id)
                    .map_err(|e| TrackerError::Invalid(e.message))?,
                s.list_tracker_views(t.id)
                    .map_err(|e| TrackerError::Invalid(e.message))?,
            )
        };
        if cred.is_none() && needs_credential(t) {
            return Err(TrackerError::Unconfigured);
        }
        let provider = provider_for(t, cred, &self.net)?;
        self.run_provider(t, provider.as_ref(), views, store, now, pass)
            .await
    }

    /// The pass proper, over the provider `run_tracker` built (tests hand
    /// in their own).
    async fn run_provider(
        &self,
        t: &TrackerRow,
        provider: &dyn TrackerProvider,
        views: Vec<crate::store::TrackerViewRow>,
        store: &Mutex<Store>,
        now: i64,
        pass: &mut TrackerPass,
    ) -> Result<(), TrackerError> {
        let incremental = provider.caps().incremental;
        // (id, updated) seen this pass: the overlap's repeats are dropped.
        let mut seen: HashSet<(String, Option<i64>)> = HashSet::new();

        // 1. views.
        for v in views.into_iter().filter(|v| v.enabled) {
            let last_full = self
                .last_full
                .lock()
                .ok()
                .and_then(|m| m.get(&(t.id, v.view_id.clone())).copied());
            let full =
                v.watermark.is_none() || last_full.is_none_or(|at| now - at >= FULL_EVERY_SECS);
            let def = ViewDef {
                id: v.view_id.clone(),
                label: v.label.clone(),
                query: v.query.clone(),
            };
            let listed = read_view(
                provider,
                &def,
                incremental,
                full,
                v.watermark,
                v.sync_mark.as_deref(),
            )
            .await;
            let (items, full, mark) = match listed {
                Ok(r) => r,
                Err(TrackerError::Forbidden(_)) => {
                    // C28 / plan: a 403 on one view disables that view only.
                    if let Ok(s) = lock(store) {
                        if s.set_tracker_view_enabled(t.id, &v.view_id, false)
                            .unwrap_or(false)
                        {
                            let _ = s.emit_tracker(t.id);
                        }
                    }
                    pass.disabled_views.push(v.view_id.clone());
                    continue;
                }
                Err(e) if e.state().is_some() => return Err(e),
                Err(e) => {
                    tracing::warn!(tracker_id = t.id, view = %v.view_id, "view listing failed: {}", e.explain());
                    continue;
                }
            };
            let newest = items.iter().filter_map(|i| i.updated).max();
            let ids: Vec<String> = items.iter().map(|i| i.external_id.clone()).collect();
            self.store_items(t.id, items, store, &mut seen, pass)?;
            // The token and the watermark move only once the items they
            // stand for are stored: a write that fails leaves them to be
            // read again, not skipped.
            if let Ok(s) = lock(store) {
                if let Some(m) = mark.filter(|m| Some(m.as_str()) != v.sync_mark.as_deref()) {
                    let _ = s.set_tracker_view_mark(t.id, &v.view_id, Some(&m));
                }
                if let Some(w) = newest {
                    let _ = s.set_tracker_view_watermark(t.id, &v.view_id, w);
                }
                // Favourite filters and containers (Asana projects) keep
                // their membership: nothing local can evaluate them.
                if v.view_id.starts_with("filter:") || v.view_id.starts_with("project:") {
                    let _ = s.set_view_members(t.id, &v.view_id, &ids, full);
                }
            }
            if full {
                if let Ok(mut m) = self.last_full.lock() {
                    m.insert((t.id, v.view_id.clone()), now);
                }
            }
        }

        // 2. linked items, by id.
        let linked = {
            let s = lock(store).map_err(|e| TrackerError::Invalid(e.message))?;
            s.linked_tracker_item_ids(t.id, LINKED_MAX)
                .map_err(|e| TrackerError::Invalid(e.message))?
        };
        if !linked.is_empty() {
            let refs: Vec<ItemRef> = linked.into_iter().map(ItemRef::Id).collect();
            let fetched = provider.fetch(&refs).await?;
            self.store_fetched(t.id, fetched, store, &mut seen, pass)?;
        }

        // 3. bare keys typed before the tracker was connected, by key.
        let unbound: Vec<String> = {
            let s = lock(store).map_err(|e| TrackerError::Invalid(e.message))?;
            let keys = s
                .unbound_ref_keys(t.id, UNBOUND_MAX * 2)
                .map_err(|e| TrackerError::Invalid(e.message))?;
            let unknown = self.unknown_keys.lock().ok();
            keys.into_iter()
                .filter(|k| {
                    unknown
                        .as_ref()
                        .and_then(|m| m.get(&(t.id, k.clone())))
                        .is_none_or(|until| *until <= now)
                })
                .take(UNBOUND_MAX)
                .collect()
        };
        if !unbound.is_empty() {
            let refs: Vec<ItemRef> = unbound.iter().map(|k| ItemRef::parse(k)).collect();
            let fetched = provider.fetch(&refs).await?;
            let mut found = Vec::new();
            for f in fetched {
                match f {
                    Fetched::Found(s) => found.push(Fetched::Found(s)),
                    Fetched::Unavailable { reference, .. } => {
                        // Not an item fleet has: nothing to mark, only
                        // remember not to ask again soon.
                        if let Ok(mut m) = self.unknown_keys.lock() {
                            m.insert((t.id, reference), now + NEGATIVE_TTL_SECS);
                        }
                    }
                }
            }
            self.store_fetched(t.id, found, store, &mut seen, pass)?;
        }

        // 5. bind.
        let s = lock(store).map_err(|e| TrackerError::Invalid(e.message))?;
        let bound = s
            .bind_tracker_refs(t.id)
            .map_err(|e| TrackerError::Invalid(e.message))?;
        pass.bound_sessions = bound.len();
        // A key that became known late re-resolves its sessions (work graph
        // M4.3); only the sessions this bind touched.
        for sid in bound {
            if let Err(e) = crate::service::work::detect::resolve_session(&s, sid) {
                tracing::debug!(error = %e.message, "[work] resolve after bind failed");
            }
        }
        Ok(())
    }

    fn store_fetched(
        &self,
        tracker_id: i64,
        fetched: Vec<Fetched>,
        store: &Mutex<Store>,
        seen: &mut HashSet<(String, Option<i64>)>,
        pass: &mut TrackerPass,
    ) -> Result<(), TrackerError> {
        let mut found = Vec::new();
        for f in fetched {
            match f {
                Fetched::Found(s) => found.push(*s),
                Fetched::Unavailable { reference, reason } => {
                    let s = lock(store).map_err(|e| TrackerError::Invalid(e.message))?;
                    if s.mark_tracker_item_unavailable(tracker_id, &reference, &reason)
                        .map_err(|e| TrackerError::Invalid(e.message))?
                    {
                        pass.unavailable += 1;
                    }
                }
            }
        }
        self.store_items(tracker_id, found, store, seen, pass)
    }

    fn store_items(
        &self,
        tracker_id: i64,
        items: Vec<WorkItemSnapshot>,
        store: &Mutex<Store>,
        seen: &mut HashSet<(String, Option<i64>)>,
        pass: &mut TrackerPass,
    ) -> Result<(), TrackerError> {
        let s = lock(store).map_err(|e| TrackerError::Invalid(e.message))?;
        for item in items {
            if !seen.insert((item.external_id.clone(), item.updated)) {
                continue;
            }
            pass.seen += 1;
            let key = item.key.clone();
            let out = s
                .upsert_tracker_item(tracker_id, &to_write(item))
                .map_err(|e| TrackerError::Invalid(e.message))?;
            if out.changed {
                pass.changed += 1;
            }
            if let Some((from, to)) = out.status_change {
                let _ = s.journal_status_change(out.id, key.as_deref(), &from, &to);
            }
        }
        Ok(())
    }
}

/// One view's items for this pass: `(items, listed whole, new sync mark)`.
///
/// * A watermark provider lists whole, or from `watermark - OVERLAP_SECS`.
/// * A sync-token provider lists whole when `full` (and asks for a first
///   token when it has none); otherwise it reads the changes since its
///   token, and an expired token (Asana: 412 after about a day) turns into
///   one whole listing plus the fresh token the tracker handed back.
async fn read_view(
    p: &dyn TrackerProvider,
    def: &ViewDef,
    incremental: Incremental,
    full: bool,
    watermark: Option<i64>,
    mark: Option<&str>,
) -> Result<(Vec<WorkItemSnapshot>, bool, Option<String>), TrackerError> {
    match incremental {
        Incremental::SyncToken if !full && mark.is_some() => {
            let ch = p.changes(def, mark).await?;
            if !ch.expired {
                return Ok((ch.items, false, ch.mark));
            }
            let items = list_all(p, def, None).await?;
            Ok((items, true, ch.mark))
        }
        Incremental::SyncToken => {
            let items = list_all(p, def, None).await?;
            let fresh = if mark.is_none() {
                // The token names "now": taken after the listing, a change
                // in between is read twice (deduped), never missed.
                p.changes(def, None).await.ok().and_then(|c| c.mark)
            } else {
                None
            };
            Ok((items, true, fresh))
        }
        Incremental::None => Ok((list_all(p, def, None).await?, true, None)),
        Incremental::Watermark => {
            let since = if full {
                None
            } else {
                watermark.map(|w| w - OVERLAP_SECS)
            };
            Ok((list_all(p, def, since).await?, full, None))
        }
    }
}

/// A provider snapshot → the store's write shape.
pub fn to_write(s: WorkItemSnapshot) -> TrackerItemWrite {
    TrackerItemWrite {
        external_id: s.external_id,
        key: s.key,
        aliases: s.aliases,
        title: s.title,
        url: s.url,
        kind: s.kind,
        hierarchy_level: s.hierarchy_level,
        status_name: s.status.name,
        status_category: s.status.category,
        resolution: s.status.resolution,
        parent_external_id: s.parent_external_id,
        containers: s.containers,
        assignees: s.assignees,
        assignee_id: s.assignee_id,
        iteration: s.iteration,
        iteration_active: s.iteration_active,
        updated_ext: s.updated,
        description: s.description,
    }
}

/// Fetch one item live (a `lookup` that missed the cache) and cache it.
/// `Ok(None)` when the tracker does not know it.
pub async fn fetch_one(
    t: &TrackerRow,
    reference: ItemRef,
    store: &Mutex<Store>,
    net: &TrackerNet,
) -> Result<Option<i64>, TrackerError> {
    let cred = {
        let s = lock(store).map_err(|e| TrackerError::Invalid(e.message))?;
        s.resolve_tracker_credential(t.id)
            .map_err(|e| TrackerError::Invalid(e.message))?
    };
    if cred.is_none() && needs_credential(t) {
        return Err(TrackerError::Unconfigured);
    }
    let provider: Box<dyn TrackerProvider> = provider_for(t, cred, net)?;
    let got = provider.fetch(std::slice::from_ref(&reference)).await?;
    let s = lock(store).map_err(|e| TrackerError::Invalid(e.message))?;
    for f in got {
        if let Fetched::Found(item) = f {
            let out = s
                .upsert_tracker_item(t.id, &to_write(*item))
                .map_err(|e| TrackerError::Invalid(e.message))?;
            let _ = s.bind_tracker_refs(t.id);
            return Ok(Some(out.id));
        }
    }
    Ok(None)
}

/// `work.sync_interval_secs`, parsed; `None` when `0` (off).
pub fn interval(store: &Mutex<Store>) -> Option<Duration> {
    let raw = lock(store).ok().and_then(|s| {
        s.get_setting(crate::service::settings::WORK_SYNC_INTERVAL_SECS)
            .ok()
            .flatten()
    });
    let secs = crate::service::settings::resolve(
        crate::service::settings::WORK_SYNC_INTERVAL_SECS,
        raw.as_deref(),
    )
    .parse::<u64>()
    .unwrap_or(DEFAULT_INTERVAL_SECS);
    (secs > 0).then(|| Duration::from_secs(secs.max(60)))
}

/// Spawn the tick. `None` (nothing spawned) when the interval is `0`. The
/// token is observed between passes only, like the reconcile tick's.
pub fn spawn_tracker_sync(
    store: Arc<Mutex<Store>>,
    net: TrackerNet,
    token: CancellationToken,
) -> Option<tokio::task::JoinHandle<()>> {
    let Some(period) = interval(&store) else {
        tracing::info!("tracker sync disabled (work.sync_interval_secs=0)");
        return None;
    };
    tracing::info!("tracker sync every {}s", period.as_secs());
    let sync = Arc::new(TrackerSync::new(net));
    Some(crate::rt::spawn(async move {
        let mut ticker = tokio::time::interval(period);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,
                _ = ticker.tick() => {}
            }
            let _ = sync.run_pass(&store).await;
        }
    }))
}

#[cfg(test)]
#[path = "tests_sync.rs"]
mod tests;
