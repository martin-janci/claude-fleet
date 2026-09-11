//! Reconcile behaviour through the REAL reconcile path (W4 F1 / BE-7).
//!
//! Every test drives `reconcile_sessions_with` over `RemoteTmux<FakeSsh>`, so
//! the production probe (`list-sessions` → `claude agents --json` → one
//! `capture-pane` per live session), the pane-intel analyzer, the transition
//! detector in `reconcile_write_one_host`, `Store::apply_host_reconcile` and
//! the bg-agent pruner all run exactly as they do in the app. Only ssh is
//! scripted.
//!
//! Covered here (and deliberately NOT in `fleet_e2e_tests`, which owns the
//! unreachable/hanging-host smoke test and the exact wire scripts):
//!
//! 1. status transitions → `session_events` rows + row events, with
//!    `idle_since` / `stuck_since` stamping and the identical-row no-op;
//! 2. the ghost lifecycle: ghost → un-ghost → dismiss, and the
//!    `probe_started_at` guard against a stale probe;
//! 3. bg-agent surfacing, pruning and the `known_agent_status` filter;
//! 4. a multi-host pass with a healthy, a timing-out and a garbage host.
//!
//! Tests marked `#[ignore = "BUG: …"]` pin behaviour that is wrong today;
//! they are the spec for the fix, not flaky tests.

use crate::events::RecordingEventBus;
use crate::service::sessions::{
    dismiss_ghost_session, reconcile_sessions_with, DismissGhostSessionArgs, ReconcileDeps,
    ReconcileGate,
};
use crate::ssh_fake::{FakeSsh, Match, Reply};
use crate::store::{SessionRow, Store};
use crate::tmux::{RemoteTmux, TmuxExec};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The exact list script `RemoteTmux::list_sessions` emits (kept in step
/// with `fleet_e2e_tests::LIST_SCRIPT`; a drift makes every test here fail
/// loudly, since the fake would answer the default empty reply).
const LIST_SCRIPT: &str = "tmux list-sessions -F '#{session_name}|#{session_created}|#{session_activity}|#{session_attached}|#{pane_current_path}' 2>&1";

/// Pane tails, each chosen to hit exactly one `pane_intel::analyze` branch.
const IDLE: &str = "All done.\n❯ \n? for shortcuts\n";
const WORKING: &str = "✻ Thinking… (esc to interrupt)\n";
const TRUST: &str = "Do you trust the files in this folder?\n ❯ 1. Yes, proceed\n   2. No\n";
const PRESS_ENTER: &str = "Update available.\nPress Enter to continue\n";
/// Tool output with no status signal at all (`derived_status == None`).
const NO_SIGNAL: &str = "   Compiling claude-fleet v0.2.4\n";

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Sleep until the unix second has advanced. The ghost guard compares whole
/// seconds (`last_reconciled_at < probe_started_at`), so a pass that must
/// ghost a row the previous pass stamped has to start in a later second.
async fn next_unix_second() {
    let start = now_unix();
    while now_unix() <= start {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A store with a recording bus, a scripted fleet and real reconcile deps.
struct Fleet {
    store: Mutex<Store>,
    bus: Arc<RecordingEventBus>,
    fake: FakeSsh,
    deps: Arc<ReconcileDeps>,
}

impl Fleet {
    /// `hosts` are registered (reachable) besides `local`, which always
    /// answers "no tmux server". Seed events are drained.
    fn new(hosts: &[&str]) -> Self {
        let bus = Arc::new(RecordingEventBus::new());
        let store = Store::open_with_bus_in_memory(bus.clone()).expect("store");
        store.upsert_host("local").unwrap();
        for h in hosts {
            store.upsert_host(h).unwrap();
        }
        let fake = FakeSsh::new();
        fake.on_host(
            "local",
            Match::script(LIST_SCRIPT),
            Reply::Exit {
                code: 1,
                stdout: b"no server running on /tmp/tmux-1000/default\n".to_vec(),
                stderr: Vec::new(),
            },
        );
        let exec_fake = fake.clone();
        let deps = ReconcileDeps::fake(
            move |alias| {
                Box::new(RemoteTmux {
                    client: exec_fake.clone(),
                    host: alias.to_string(),
                })
            },
            Duration::from_secs(5),
        );
        bus.take();
        Self {
            store: Mutex::new(store),
            bus,
            fake,
            deps,
        }
    }

    async fn pass(&self) {
        reconcile_sessions_with(&self.store, &self.deps)
            .await
            .expect("the pass completes");
    }

    /// Script `host`'s `tmux list-sessions` output (later calls win).
    fn list(&self, host: &str, lines: &str) {
        self.fake
            .on_host(host, Match::script(LIST_SCRIPT), Reply::ok(lines));
    }

    /// Script `host`'s `claude agents --json` output.
    fn agents(&self, host: &str, json: &str) {
        self.fake.on_host(
            host,
            Match::script_contains("claude agents --json"),
            Reply::ok(json),
        );
    }

    /// Script the pane tail captured for `name` on `host`.
    fn pane(&self, host: &str, name: &str, tail: &str) {
        self.fake.on_host(
            host,
            Match::script_contains(&format!("tmux capture-pane -t '{name}'")),
            Reply::ok(tail),
        );
    }

    fn try_row(&self, name: &str, host: &str) -> Option<SessionRow> {
        self.store.lock().unwrap().get_session(name, host).unwrap()
    }

    fn row(&self, name: &str, host: &str) -> SessionRow {
        self.try_row(name, host)
            .unwrap_or_else(|| panic!("row {host}/{name}"))
    }

    /// The session's `session_events` timeline, OLDEST first, as
    /// `(kind, detail)`.
    fn timeline(&self, session_id: i64) -> Vec<(String, Option<String>)> {
        let mut ev = self
            .store
            .lock()
            .unwrap()
            .list_session_events(session_id, 100)
            .unwrap();
        ev.reverse();
        ev.into_iter().map(|e| (e.kind, e.detail)).collect()
    }

    /// `session_events` rows for `session_id`, orphans included (a raw
    /// count, independent of whether the session row still exists).
    fn raw_event_count(&self, session_id: i64) -> i64 {
        self.store
            .lock()
            .unwrap()
            .conn_ref()
            .query_row(
                "SELECT COUNT(*) FROM session_events WHERE session_id=?1",
                [session_id],
                |r| r.get(0),
            )
            .unwrap()
    }

    /// Session row events (`session:created|updated|killed:<id>`) emitted
    /// since the last call; host/project events are dropped.
    fn session_row_events(&self) -> Vec<String> {
        self.bus
            .take()
            .into_iter()
            .filter(|e| e.starts_with("session:"))
            .collect()
    }

    fn reachable(&self, alias: &str) -> bool {
        self.store
            .lock()
            .unwrap()
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == alias)
            .unwrap_or_else(|| panic!("host {alias}"))
            .reachable
    }
}

fn ev(kind: &str, detail: Option<&str>) -> (String, Option<String>) {
    (kind.to_string(), detail.map(str::to_string))
}

// ── 1. status transitions ───────────────────────────────────────────────────

#[tokio::test]
async fn status_transitions_emit_session_events_and_stamp_lifecycle_columns() {
    let f = Fleet::new(&["alpha"]);
    f.list("alpha", "work|1700000000|1700000100|0|/tmp/w\n");
    f.agents("alpha", "[]\n");

    // Pass 1 — first sighting, idle pane.
    f.pane("alpha", "work", IDLE);
    let t1 = now_unix();
    f.pass().await;
    let r1 = f.row("work", "alpha");
    let id = r1.id;
    assert_eq!(r1.claude_status.as_deref(), Some("idle"));
    let idle1 = r1.idle_since.expect("entering idle stamps idle_since");
    assert!(idle1 >= t1);
    assert_eq!((r1.stuck_kind.as_deref(), r1.stuck_since), (None, None));
    assert!(
        f.timeline(id).is_empty(),
        "a first sighting has no prior status to transition from"
    );
    assert_eq!(
        f.session_row_events(),
        vec![format!("session:created:{id}")]
    );

    // Pass 2 — nothing changed: identical row, zero events of either kind.
    f.pass().await;
    assert_eq!(f.row("work", "alpha"), r1, "an identical pass is a no-op");
    assert!(
        f.session_row_events().is_empty(),
        "no-op pass emits no row event"
    );
    assert!(f.timeline(id).is_empty());

    // Pass 3 — idle → working: idle_since clears.
    f.pane("alpha", "work", WORKING);
    f.pass().await;
    let r3 = f.row("work", "alpha");
    assert_eq!(r3.claude_status.as_deref(), Some("working"));
    assert_eq!(r3.idle_since, None, "leaving idle clears idle_since");
    assert_eq!(f.timeline(id), vec![ev("status_change", Some("working"))]);
    assert_eq!(
        f.session_row_events(),
        vec![format!("session:updated:{id}")]
    );

    // Pass 4 — working → blocked on a trust prompt: stuck_since starts.
    f.pane("alpha", "work", TRUST);
    let t4 = now_unix();
    f.pass().await;
    let r4 = f.row("work", "alpha");
    assert_eq!(r4.claude_status.as_deref(), Some("blocked"));
    assert_eq!(r4.stuck_kind.as_deref(), Some("trust_prompt"));
    let stuck4 = r4.stuck_since.expect("a stuck episode stamps stuck_since");
    assert!(stuck4 >= t4);
    assert_eq!(r4.idle_since, None, "blocked is not an idle status");
    assert_eq!(
        f.timeline(id),
        vec![
            ev("status_change", Some("working")),
            ev("status_change", Some("blocked")),
            ev("stuck", Some("trust_prompt")),
        ]
    );
    assert_eq!(
        f.session_row_events(),
        vec![format!("session:updated:{id}")]
    );

    // Pass 5 — same stuck pane a second later: the episode start is kept and
    // the row is untouched.
    next_unix_second().await;
    f.pass().await;
    assert_eq!(f.row("work", "alpha"), r4, "same stuck episode is a no-op");
    assert!(f.session_row_events().is_empty());
    assert_eq!(f.timeline(id).len(), 3);

    // Pass 6 — the stuck KIND changes (still blocked): stuck_since restarts,
    // a new `stuck` event, but no status_change.
    f.pane("alpha", "work", PRESS_ENTER);
    f.pass().await;
    let r6 = f.row("work", "alpha");
    assert_eq!(r6.claude_status.as_deref(), Some("blocked"));
    assert_eq!(r6.stuck_kind.as_deref(), Some("press_enter"));
    assert!(
        r6.stuck_since.unwrap() > stuck4,
        "a new stuck kind is a new episode: {:?} vs {stuck4}",
        r6.stuck_since
    );
    assert_eq!(
        f.timeline(id).last(),
        Some(&ev("stuck", Some("press_enter")))
    );
    assert_eq!(f.timeline(id).len(), 4);
    assert_eq!(
        f.session_row_events(),
        vec![format!("session:updated:{id}")]
    );

    // Pass 7 — back to idle: stuck clears (not an event), idle_since re-stamps.
    f.pane("alpha", "work", IDLE);
    let t7 = now_unix();
    f.pass().await;
    let r7 = f.row("work", "alpha");
    assert_eq!(r7.claude_status.as_deref(), Some("idle"));
    assert_eq!((r7.stuck_kind.as_deref(), r7.stuck_since), (None, None));
    assert!(r7.idle_since.expect("re-entering idle re-stamps") >= t7);
    assert_eq!(
        f.timeline(id),
        vec![
            ev("status_change", Some("working")),
            ev("status_change", Some("blocked")),
            ev("stuck", Some("trust_prompt")),
            ev("stuck", Some("press_enter")),
            ev("status_change", Some("idle")),
        ],
        "clearing a stuck flag is not an alert-worthy event"
    );
    assert_eq!(
        f.session_row_events(),
        vec![format!("session:updated:{id}")]
    );
}

#[tokio::test]
async fn failed_pane_capture_preserves_status_and_stuck_flag() {
    // A pane that cannot be captured this pass (capture exits non-zero →
    // empty tail → no intel) must not clear a stuck flag or its episode start.
    let f = Fleet::new(&["alpha"]);
    f.list("alpha", "work|1|2|0|/tmp/w\n");
    f.agents("alpha", "[]\n");
    f.pane("alpha", "work", WORKING);
    f.pass().await;
    f.pane("alpha", "work", TRUST);
    f.pass().await;
    let before = f.row("work", "alpha");
    assert_eq!(before.stuck_kind.as_deref(), Some("trust_prompt"));
    assert_eq!(f.timeline(before.id).len(), 2, "status_change + stuck");
    f.session_row_events();

    f.fake.on_host(
        "alpha",
        Match::script_contains("tmux capture-pane -t 'work'"),
        Reply::fail(1, "can't find pane: work"),
    );
    next_unix_second().await;
    f.pass().await;
    let after = f.row("work", "alpha");
    assert_eq!(
        (after.stuck_kind.as_deref(), after.stuck_since),
        (Some("trust_prompt"), before.stuck_since),
        "an unobserved pane keeps the stuck flag and its episode start"
    );
    assert_eq!(after.claude_status.as_deref(), Some("blocked"));
}

#[tokio::test]
#[ignore = "BUG: a pass with no status signal logs a phantom status_change(None) every pass (reconcile_write_one_host compares prior vs pre-COALESCE value)"]
async fn pane_without_a_status_signal_does_not_log_phantom_status_changes() {
    // The upsert COALESCEs a NULL claude_status onto the stored one, so the
    // row stays `idle` — but the transition detector compares the prior row
    // against the pre-COALESCE `None` and queues `status_change` with a NULL
    // detail on EVERY such pass. That is the flap `SESSION_EVENTS_CAP` was
    // added to contain (200k rows per session). The same happens on a
    // failed pane capture with no agent status.
    let f = Fleet::new(&["alpha"]);
    f.list("alpha", "work|1|2|0|/tmp/w\n");
    f.agents("alpha", "[]\n");
    f.pane("alpha", "work", IDLE);
    f.pass().await;
    let id = f.row("work", "alpha").id;

    f.pane("alpha", "work", NO_SIGNAL);
    f.pass().await;
    f.pass().await;
    assert_eq!(
        f.row("work", "alpha").claude_status.as_deref(),
        Some("idle"),
        "the stored status is preserved by COALESCE"
    );
    assert_eq!(
        f.timeline(id),
        Vec::<(String, Option<String>)>::new(),
        "no status actually changed, so no status_change may be recorded"
    );
}

// ── 2. ghost lifecycle ──────────────────────────────────────────────────────

#[tokio::test]
async fn ghost_lifecycle_ghosts_unghosts_and_dismissed_rows_stay_gone() {
    let f = Fleet::new(&["alpha"]);
    f.agents("alpha", "[]\n");

    // Live.
    f.list("alpha", "s1|1|2|0|/tmp/s1\n");
    f.pass().await;
    let id = f.row("s1", "alpha").id;
    assert_eq!(
        f.session_row_events(),
        vec![format!("session:created:{id}")]
    );

    // Gone from tmux → ghost with lost_at.
    next_unix_second().await;
    f.list("alpha", "");
    let t = now_unix();
    f.pass().await;
    let g = f.row("s1", "alpha");
    assert_eq!(g.status, "ghost");
    assert!(g.lost_at.expect("ghosting stamps lost_at") >= t);
    assert_eq!(
        f.session_row_events(),
        vec![format!("session:updated:{id}")]
    );

    // Back in tmux → un-ghosted in place (same id, lost_at cleared).
    f.list("alpha", "s1|1|3|0|/tmp/s1\n");
    f.pass().await;
    let u = f.row("s1", "alpha");
    assert_eq!((u.id, u.status.as_str(), u.lost_at), (id, "running", None));
    assert_eq!(
        f.session_row_events(),
        vec![format!("session:updated:{id}")]
    );

    // Gone again → ghost; the user dismisses it.
    next_unix_second().await;
    f.list("alpha", "");
    f.pass().await;
    assert_eq!(f.row("s1", "alpha").status, "ghost");
    f.session_row_events();
    dismiss_ghost_session(DismissGhostSessionArgs { session_id: id }, &f.store)
        .expect("dismiss a ghost");
    assert_eq!(f.session_row_events(), vec![format!("session:killed:{id}")]);
    assert!(f.try_row("s1", "alpha").is_none());

    // Later passes (session still absent) never bring it back.
    for _ in 0..2 {
        next_unix_second().await;
        f.pass().await;
        assert!(
            f.try_row("s1", "alpha").is_none(),
            "dismissed row resurrected"
        );
        assert!(f.session_row_events().is_empty());
    }
}

#[tokio::test]
#[ignore = "BUG: dismiss_ghost_session deletes the row but not its session_events; the orphans leak onto a reused session id"]
async fn dismissing_a_ghost_reaps_its_session_events() {
    // `Store::delete_session` (behind `dismiss_ghost_session`) has no
    // `DELETE FROM session_events`, and the table has no FK cascade. The
    // reconcile hard-delete path reaps them; dismissal does not. Because
    // `sessions.id` is a plain INTEGER PRIMARY KEY (no AUTOINCREMENT), the
    // next session can reuse the id and inherit the dead one's timeline.
    let f = Fleet::new(&["alpha"]);
    f.agents("alpha", "[]\n");
    f.list("alpha", "s1|1|2|0|/tmp/s1\n");
    f.pane("alpha", "s1", IDLE);
    f.pass().await;
    f.pane("alpha", "s1", WORKING);
    f.pass().await;
    let id = f.row("s1", "alpha").id;
    assert_eq!(f.timeline(id).len(), 1, "one status_change recorded");

    next_unix_second().await;
    f.list("alpha", "");
    f.pass().await;
    dismiss_ghost_session(DismissGhostSessionArgs { session_id: id }, &f.store).unwrap();
    assert_eq!(f.raw_event_count(id), 0, "dismissal must reap the timeline");

    f.list("alpha", "s2|5|6|0|/tmp/s2\n");
    f.pass().await;
    let s2 = f.row("s2", "alpha");
    assert!(
        f.timeline(s2.id).is_empty(),
        "a new session (id {} vs dismissed {id}) must not inherit a timeline",
        s2.id
    );
}

#[tokio::test]
async fn stale_probe_does_not_ghost_a_row_stamped_after_it_started() {
    // BE-3 through the real fan-out: alpha's `list-sessions` is slow and
    // answers "no sessions". While it is in flight, a concurrent writer
    // (standing in for `new_session`'s own single-host reconcile) creates and
    // stamps `fresh`. The stale pass must ghost the genuinely stale `old`
    // row but leave `fresh` alone.
    let f = Fleet::new(&["alpha"]);
    f.agents("alpha", "[]\n");
    {
        let s = f.store.lock().unwrap();
        s.upsert_session("old", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        s.mark_sessions_reconciled("alpha", &["old".to_string()], now_unix() - 10)
            .unwrap();
    }
    f.fake.on_host(
        "alpha",
        Match::script(LIST_SCRIPT),
        Reply::Hang {
            for_: Duration::from_millis(600),
        },
    );
    let concurrent_create = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let s = f.store.lock().unwrap();
        s.upsert_session("fresh", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        s.mark_sessions_reconciled("alpha", &["fresh".to_string()], now_unix())
            .unwrap();
    };
    let (res, ()) = tokio::join!(
        reconcile_sessions_with(&f.store, &f.deps),
        concurrent_create
    );
    res.expect("pass completes");
    assert_eq!(f.row("old", "alpha").status, "ghost", "stale row ghosted");
    let fresh = f.row("fresh", "alpha");
    assert_eq!(
        (fresh.status.as_str(), fresh.lost_at),
        ("running", None),
        "a row stamped after the probe started is not this probe's to ghost"
    );

    // A probe that starts in a later second is authoritative for it.
    next_unix_second().await;
    f.list("alpha", "");
    f.pass().await;
    assert_eq!(f.row("fresh", "alpha").status, "ghost");
}

// ── 3. bg agents ────────────────────────────────────────────────────────────

#[tokio::test]
async fn bg_agents_surface_prune_and_filter_unknown_statuses() {
    let f = Fleet::new(&["alpha"]);
    f.list("alpha", "work|1|2|0|/tmp/w\n");
    f.pane("alpha", "work", IDLE);
    // `work` has a tmux pane and a matching agent whose status is outside the
    // vocabulary; `nightly` and `report` are pane-less `claude --bg` agents.
    f.agents(
        "alpha",
        r#"[{"sessionId":"t1","name":"work","status":"awaiting_input","cwd":"/tmp/w"},
            {"sessionId":"bg1","name":"nightly","status":"working","cwd":"/tmp/bg"},
            {"sessionId":"bg2","name":"report","status":"completed","cwd":"/tmp/r"}]"#,
    );
    f.pass().await;

    let work = f.row("work", "alpha");
    assert_ne!(work.kind, "bg");
    assert_eq!(work.claude_session_id.as_deref(), Some("t1"));
    assert_eq!(
        work.claude_status.as_deref(),
        Some("idle"),
        "unknown agent status is dropped and the pane heuristic wins"
    );
    assert!(
        f.try_row("bg:t1", "alpha").is_none(),
        "matched agent is not bg"
    );
    let bg1 = f.row("bg:bg1", "alpha");
    assert_eq!((bg1.kind.as_str(), bg1.status.as_str()), ("bg", "running"));
    assert_eq!(bg1.claude_session_id.as_deref(), Some("bg1"));
    assert_eq!(bg1.claude_status.as_deref(), Some("working"));
    assert_eq!(bg1.idle_since, None);
    let bg2 = f.row("bg:bg2", "alpha");
    assert_eq!(bg2.claude_status.as_deref(), Some("completed"));
    assert!(bg2.idle_since.is_some(), "completed is an idle status");
    let bg1_id = bg1.id;

    // Pass 2: a known agent status is authoritative over the pane; both bg
    // agents are gone → ghosted (one-cycle grace), tmux row untouched.
    f.agents(
        "alpha",
        r#"[{"sessionId":"t1","name":"work","status":"working","cwd":"/tmp/w"}]"#,
    );
    f.pass().await;
    assert_eq!(
        f.row("work", "alpha").claude_status.as_deref(),
        Some("working"),
        "a vocabulary status from `claude agents` beats the pane"
    );
    for name in ["bg:bg1", "bg:bg2"] {
        let r = f.row(name, "alpha");
        assert_eq!(r.status, "ghost", "{name}");
        assert!(r.lost_at.is_some(), "{name}");
    }
    assert_eq!(f.row("work", "alpha").status, "running");

    // Pass 3: unknown status again → pane fallback; bg rows still absent →
    // hard-deleted.
    f.session_row_events();
    f.agents(
        "alpha",
        r#"[{"sessionId":"t1","name":"work","status":"thinking_hard","cwd":"/tmp/w"}]"#,
    );
    f.pass().await;
    assert_eq!(
        f.row("work", "alpha").claude_status.as_deref(),
        Some("idle")
    );
    assert!(f.try_row("bg:bg1", "alpha").is_none());
    assert!(f.try_row("bg:bg2", "alpha").is_none());
    assert!(
        f.session_row_events()
            .contains(&format!("session:killed:{bg1_id}")),
        "the prune is announced to the frontend"
    );
}

#[tokio::test]
#[ignore = "BUG: reconcile_bg_agents stores the raw `claude agents` status on bg rows, bypassing known_agent_status"]
async fn bg_agent_with_unknown_status_is_not_stored_verbatim() {
    // Tmux rows run the agent status through `known_agent_status`; bg rows
    // (`reconcile_bg_agents` → `upsert_bg_session`) pass `agent.status`
    // straight through, so an out-of-vocabulary value lands in
    // `claude_status` and reaches the MCP/UI contract.
    let f = Fleet::new(&["alpha"]);
    f.list("alpha", "");
    f.agents(
        "alpha",
        r#"[{"sessionId":"bg9","name":"x","status":"awaiting_input","cwd":"/tmp/x"}]"#,
    );
    f.pass().await;
    let bg = f.row("bg:bg9", "alpha");
    assert_eq!(bg.kind, "bg");
    assert_eq!(
        bg.claude_status, None,
        "an unknown status must be dropped for bg rows too"
    );
}

// ── 4. multi-host ───────────────────────────────────────────────────────────

#[tokio::test]
async fn multi_host_pass_isolates_timeout_and_garbage_hosts_and_frees_the_gate() {
    let f = Fleet::new(&["alpha", "gamma", "delta"]);
    {
        let s = f.store.lock().unwrap();
        for (name, host) in [
            ("alpha-stale", "alpha"),
            ("gamma-old", "gamma"),
            ("delta-old", "delta"),
        ] {
            s.upsert_session(name, host, None, None, 1, 1, "running", None)
                .unwrap();
        }
    }
    f.fake.set_wall_clock(Duration::from_millis(150));
    // alpha: healthy tmux, garbage agents JSON (degrades to "no agents").
    f.list("alpha", "alpha-live|1700000000|1700000100|0|/tmp/a\n");
    f.agents("alpha", "{\"not\": json at all\n");
    f.pane("alpha", "alpha-live", IDLE);
    // gamma: ssh black-holes; the ssh wall clock (E_SSH_TIMEOUT) gets us out.
    f.fake.hanging("gamma");
    // delta: every command exits non-zero with binary junk.
    f.fake.on_host(
        "delta",
        Match::Any,
        Reply::Exit {
            code: 1,
            stdout: b"\x1b[31m%%\xff\xfe segfault \x00 garbage\n".to_vec(),
            stderr: b"??".to_vec(),
        },
    );

    // Preconditions: the failure modes are what the test claims they are.
    let gamma_err = RemoteTmux {
        client: f.fake.clone(),
        host: "gamma".to_string(),
    }
    .list_sessions()
    .await
    .unwrap_err();
    assert_eq!(gamma_err.code, "E_SSH_TIMEOUT");
    let delta_err = RemoteTmux {
        client: f.fake.clone(),
        host: "delta".to_string(),
    }
    .list_sessions()
    .await
    .unwrap_err();
    assert_eq!(delta_err.code, "E_TMUX");
    f.fake.clear_calls();

    let gamma_before = f.row("gamma-old", "gamma");
    let delta_before = f.row("delta-old", "delta");
    f.session_row_events();

    // The pass runs under the gate exactly like `run_full_reconcile`.
    let gate = ReconcileGate::new();
    let pass = gate.try_begin().expect("free gate");
    assert!(gate.try_begin().is_none(), "single slot while a pass runs");
    let start = std::time::Instant::now();
    f.pass().await;
    let elapsed = start.elapsed();
    drop(pass);
    assert!(
        gate.try_begin().is_some(),
        "gate released after a pass with erroring probes"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "bounded by the ssh wall clock, not the 5 s probe cap: {elapsed:?}"
    );

    // Reachability.
    assert!(f.reachable("local"));
    assert!(f.reachable("alpha"));
    assert!(!f.reachable("gamma"), "timed out → unreachable");
    assert!(!f.reachable("delta"), "unparseable failure → unreachable");

    // Only alpha's rows changed.
    assert_eq!(f.row("gamma-old", "gamma"), gamma_before);
    assert_eq!(f.row("delta-old", "delta"), delta_before);
    let live = f.row("alpha-live", "alpha");
    assert_eq!(live.status, "running");
    assert_eq!(live.claude_status.as_deref(), Some("idle"));
    assert_eq!(
        live.claude_session_id, None,
        "garbage agents JSON → no match"
    );
    let stale = f.row("alpha-stale", "alpha");
    assert_eq!(stale.status, "ghost");
    let mut touched: Vec<i64> = f
        .session_row_events()
        .iter()
        .map(|e| e.rsplit(':').next().unwrap().parse().unwrap())
        .collect();
    touched.sort_unstable();
    touched.dedup();
    let mut expected = vec![live.id, stale.id];
    expected.sort_unstable();
    assert_eq!(touched, expected, "row events only for alpha's rows");
    for id in [gamma_before.id, delta_before.id] {
        assert_eq!(f.raw_event_count(id), 0);
    }
    // A failed list skips the pane captures on the broken hosts.
    for host in ["gamma", "delta"] {
        assert!(
            f.fake
                .calls_for(host)
                .iter()
                .all(|c| !c.command().contains("capture-pane")),
            "{host}"
        );
    }
}

#[tokio::test]
#[ignore = "BUG: `tmux list-sessions` exiting 0 with unparseable output parses as 'no sessions' and ghosts every row on the host"]
async fn garbage_list_output_with_exit_zero_does_not_ghost_host_rows() {
    // `RemoteTmux::list_sessions` → `parse_sessions` silently drops every
    // line it cannot parse, so a success exit whose stdout is not the
    // `-F` format at all (a tmux wrapper/alias, a banner-only reply, a
    // version whose format vars differ) becomes `Ok(vec![])` — "the host has
    // no sessions" — and the reconcile ghosts (then deletes) all of them.
    let f = Fleet::new(&["delta"]);
    {
        let s = f.store.lock().unwrap();
        s.upsert_session("delta-old", "delta", None, None, 1, 1, "running", None)
            .unwrap();
    }
    f.list(
        "delta",
        "Welcome to delta!\nsession list unavailable: ???\n",
    );
    f.agents("delta", "[]\n");
    let before = f.row("delta-old", "delta");
    f.pass().await;
    assert_eq!(
        f.row("delta-old", "delta").status,
        before.status,
        "unparseable list output is not evidence the sessions are gone"
    );
}
