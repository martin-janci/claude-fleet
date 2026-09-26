//! Work graph M12.2 (scale): budget tests over the seeded fixture in
//! `store/scale_fixture.rs` (2,000 sessions on 20 hosts in 3 orgs, 20,000
//! work links, 5,000 tracker items, 50,000 journal rows).
//!
//! Each call is timed over a handful of runs; the p50 / p95 are printed
//! (`cargo test -p fleet-core scale_ -- --nocapture` shows them) and the p95
//! is held under a budget with a wide margin, for an unoptimised test build
//! on a slow CI runner. Timing is the noisy half, so every call also has its
//! statements traced and their `EXPLAIN QUERY PLAN` checked: no full scan of
//! a big table (`scale_fixture::BIG_TABLES`). That half is exact, and it is what
//! catches a lost index long before the clock would.

use crate::ipc_error::lock;
use crate::service::gc::tidy::{plan_tidy, TidyContext};
use crate::service::orgs::OrgScope;
use crate::service::work::resolve::{
    resolve, Candidate, Evidence, ExistingLink, ResolveInput, Signal, Strength,
};
use crate::store::scale_fixture::{self, ScaleFixture};
use crate::store::Store;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const SEED: u64 = 0x5CA1E;
const RUNS: usize = 11;

/// One fixture per test binary: building it is the slow part, and every
/// test only reads (the resolver's writes are idempotent on a second run).
fn fixture() -> &'static Mutex<Fixture> {
    static F: OnceLock<Mutex<Fixture>> = OnceLock::new();
    F.get_or_init(|| {
        let now = crate::service::catalog::now_secs();
        let t = Instant::now();
        let f = scale_fixture::build(SEED, now);
        println!(
            "[m12.2 scale] fixture built in {} ms",
            t.elapsed().as_millis()
        );
        let ScaleFixture {
            store,
            now,
            busiest_session,
            hot_key,
        } = f;
        Mutex::new(Fixture {
            store: Mutex::new(store),
            now,
            busiest_session,
            hot_key,
        })
    })
}

struct Fixture {
    store: Mutex<Store>,
    now: i64,
    busiest_session: i64,
    hot_key: String,
}

/// Run `f` [`RUNS`] times after one warm-up; print and return p50 / p95 in
/// milliseconds.
fn measure<T>(label: &str, mut f: impl FnMut() -> T) -> (f64, f64) {
    std::hint::black_box(f());
    let mut ms: Vec<f64> = (0..RUNS)
        .map(|_| {
            let t = Instant::now();
            std::hint::black_box(f());
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    ms.sort_by(|a, b| a.total_cmp(b));
    let p50 = ms[RUNS / 2];
    let p95 = ms[((RUNS as f64 * 0.95).ceil() as usize).min(RUNS) - 1];
    println!("[m12.2 scale] {label}: p50 {p50:.1} ms, p95 {p95:.1} ms");
    (p50, p95)
}

fn budget(label: &str, p95: f64, max_ms: f64) {
    assert!(
        p95 < max_ms,
        "{label}: p95 {p95:.1} ms is over its {max_ms} ms budget (see the M12 plan's Revisions)"
    );
}

/// The statements `f` runs, traced on the store.
fn traced<T>(store: &Mutex<Store>, f: impl FnOnce() -> T) -> (T, Vec<String>) {
    lock(store).unwrap().start_trace();
    let out = f();
    let sql = lock(store).unwrap().finish_trace();
    (out, sql)
}

/// Assert no statement of `sql` reads a whole big table.
fn assert_no_full_scans(store: &Mutex<Store>, label: &str, sql: &[String]) {
    let s = lock(store).unwrap();
    assert!(!sql.is_empty(), "{label}: nothing was traced");
    let mut bad = Vec::new();
    for stmt in sql {
        let head = stmt.trim_start().to_uppercase();
        if !(head.starts_with("SELECT") || head.starts_with("WITH")) {
            continue;
        }
        let scans = s.full_scans(stmt);
        if scans.is_empty() {
            continue;
        }
        bad.push(format!("{scans:?} in:\n    {stmt}"));
    }
    drop(s);
    assert!(
        bad.is_empty(),
        "{label}: full scans of big tables:\n  {}",
        bad.join("\n  ")
    );
}

fn host_scope(s: &Mutex<Store>) -> OrgScope {
    OrgScope::for_host(&lock(s).unwrap(), "h01").unwrap()
}

#[test]
fn scale_list_sessions_with_work_fields() {
    let f = fixture().lock().unwrap_or_else(|e| e.into_inner());
    let store = &f.store;
    let (rows, sql) = traced(store, || lock(store).unwrap().list_all_sessions().unwrap());
    assert_eq!(rows.len(), scale_fixture::SESSIONS);
    let with_work = rows.iter().filter(|r| r.work.is_some()).count();
    let suggested = rows.iter().filter(|r| r.work_suggested.is_some()).count();
    let placed = rows.iter().filter(|r| r.org_id.is_some()).count();
    assert!(with_work > 1_000 && suggested > 500 && placed > 1_000);
    assert_no_full_scans(store, "list_sessions", &sql);

    let (_, p95) = measure("list_sessions (OrgScope::All), 2,000 rows", || {
        lock(store).unwrap().list_all_sessions().unwrap()
    });
    budget("list_sessions (All)", p95, 1_500.0);

    // A per-host token: the MCP tool's shape (`session_ops::list_sessions`):
    // the same cached read, cut and redacted by the scope.
    let scope = host_scope(store);
    let scoped = |s: &Store| {
        let mut v: Vec<_> = s
            .list_all_sessions()
            .unwrap()
            .into_iter()
            .filter(|r| scope.sees_row(r))
            .collect();
        for r in &mut v {
            scope.redact_row(r);
        }
        v
    };
    let seen = scoped(&lock(store).unwrap());
    assert!(!seen.is_empty() && seen.len() < scale_fixture::SESSIONS);
    let (_, p95) = measure("list_sessions (per-host h01)", || {
        scoped(&lock(store).unwrap())
    });
    budget("list_sessions (per-host)", p95, 1_500.0);

    let (rows, sql) = traced(store, || {
        lock(store).unwrap().list_sessions_for_host("h01").unwrap()
    });
    assert_eq!(rows.len(), scale_fixture::SESSIONS / scale_fixture::HOSTS);
    assert_no_full_scans(store, "list_sessions_for_host", &sql);
    let (_, p95) = measure("list_sessions_for_host(h01), 100 rows", || {
        lock(store).unwrap().list_sessions_for_host("h01").unwrap()
    });
    budget("list_sessions_for_host", p95, 300.0);
}

#[test]
fn scale_work_today() {
    let f = fixture().lock().unwrap_or_else(|e| e.into_inner());
    let store = &f.store;
    let all = OrgScope::All;
    let (t, sql) = traced(store, || super::today::today(store, None, &all).unwrap());
    assert!(!t.groups.is_empty());
    // `tracker_items` reads every item of every tracker through the
    // tracker index (a SEARCH per tracker): the cache is the input.
    assert_no_full_scans(store, "work { today }", &sql);
    let (_, p95) = measure("work { today } (All, since midnight-ish)", || {
        super::today::today(store, None, &all).unwrap()
    });
    budget("today (All)", p95, 2_000.0);

    let month = f.now - 30 * 86_400;
    let t = super::today::today(store, Some(month), &all).unwrap();
    assert!(!t.shipped.is_empty(), "a month ships something");
    let (_, p95) = measure("work { today } (All, since 30 days)", || {
        super::today::today(store, Some(month), &all).unwrap()
    });
    budget("today (All, 30 days)", p95, 2_000.0);

    let scope = host_scope(store);
    let (t, sql) = traced(store, || super::today::today(store, None, &scope).unwrap());
    assert!(t
        .groups
        .iter()
        .flat_map(|g| &g.sessions)
        .all(|s| s.host_alias == "h01"));
    assert_no_full_scans(store, "work { today } per host", &sql);
    let (_, p95) = measure("work { today } (per-host h01)", || {
        super::today::today(store, None, &scope).unwrap()
    });
    budget("today (per-host)", p95, 2_000.0);
}

#[test]
fn scale_work_tickets() {
    use crate::service::trackers::tickets::tickets;
    let f = fixture().lock().unwrap_or_else(|e| e.into_inner());
    let store = &f.store;
    let all = OrgScope::All;
    let (page, sql) = traced(store, || {
        tickets(store, None, None, None, None, &all).unwrap()
    });
    assert_eq!(page.len(), 50);
    assert_no_full_scans(store, "work { tickets }", &sql);
    let (_, p95) = measure("work { tickets } (first page of 50)", || {
        tickets(store, None, None, None, None, &all).unwrap()
    });
    budget("tickets (page)", p95, 1_000.0);

    let (_, p95) = measure("work { tickets, limit: 200 }", || {
        tickets(store, None, None, None, Some(200), &all).unwrap()
    });
    budget("tickets (200)", p95, 1_500.0);

    let mine = tickets(store, None, Some("mine"), None, Some(200), &all).unwrap();
    assert!(!mine.is_empty());
    let (_, p95) = measure("work { tickets, view: mine }", || {
        tickets(store, None, Some("mine"), None, Some(200), &all).unwrap()
    });
    budget("tickets (mine)", p95, 1_000.0);

    // A lookup by key: the whole cache read, one hit.
    let key = f.hot_key.clone();
    let (hit, sql) = traced(store, || {
        tickets(store, None, None, Some(&key), Some(5), &all).unwrap()
    });
    assert!(hit
        .iter()
        .any(|t| t.item.key.as_deref() == Some(key.as_str())));
    assert!(
        hit.iter().any(|t| !t.live_session_ids.is_empty()),
        "{key} has live work"
    );
    assert_no_full_scans(store, "work { tickets, query }", &sql);
    let (_, p95) = measure("work { tickets, query: <key> }", || {
        tickets(store, None, None, Some(&key), Some(5), &all).unwrap()
    });
    budget("tickets (query)", p95, 1_000.0);

    let scope = host_scope(store);
    let (_, sql) = traced(store, || {
        tickets(store, None, None, None, None, &scope).unwrap()
    });
    assert_no_full_scans(store, "work { tickets } per host", &sql);
    let (_, p95) = measure("work { tickets } (per-host h01)", || {
        tickets(store, None, None, None, None, &scope).unwrap()
    });
    budget("tickets (per-host)", p95, 1_500.0);
}

#[test]
fn scale_tidy_planner() {
    let f = fixture().lock().unwrap_or_else(|e| e.into_inner());
    let store = &f.store;
    let all = OrgScope::All;
    let (report, sql) = traced(store, || {
        super::tidy::work_tidy(store, &all, f.now).unwrap()
    });
    assert!(!report.candidates.is_empty());
    assert_no_full_scans(store, "work { tidy }", &sql);
    let (_, p95) = measure("work { tidy } (read + plan, all candidates)", || {
        super::tidy::work_tidy(store, &all, f.now).unwrap()
    });
    budget("tidy (All)", p95, 2_000.0);

    // The planner alone, pure, over every session.
    let sessions = lock(store).unwrap().tidy_sessions().unwrap();
    assert_eq!(sessions.len(), scale_fixture::SESSIONS);
    let cfg = super::tidy::tidy_config(&lock(store).unwrap());
    let reachable: HashSet<String> = (0..scale_fixture::HOSTS)
        .map(scale_fixture::host_alias)
        .collect();
    let ctx = TidyContext {
        controller: None,
        operator: None,
        reachable: &reachable,
        now: f.now,
    };
    let (_, p95) = measure("plan_tidy (pure), 2,000 sessions", || {
        plan_tidy(&sessions, &cfg, &ctx)
    });
    budget("plan_tidy", p95, 500.0);

    let scope = host_scope(store);
    let r = super::tidy::work_tidy(store, &scope, f.now).unwrap();
    assert!(r.candidates.iter().all(|c| c.host_alias == "h01"));
    let (_, p95) = measure("work { tidy } (per-host h01)", || {
        super::tidy::work_tidy(store, &scope, f.now).unwrap()
    });
    budget("tidy (per-host)", p95, 2_000.0);
}

#[test]
fn scale_resolver() {
    let f = fixture().lock().unwrap_or_else(|e| e.into_inner());
    let store = &f.store;
    let sid = f.busiest_session;
    // Settle once: the first run may apply changes, later ones are the
    // steady state every trigger sees.
    super::detect::resolve_session(&lock(store).unwrap(), sid).unwrap();
    let (_, sql) = traced(store, || {
        super::detect::resolve_session(&lock(store).unwrap(), sid).unwrap()
    });
    assert_no_full_scans(store, "resolve_session", &sql);
    let (_, p95) = measure("resolve_session (busiest session)", || {
        super::detect::resolve_session(&lock(store).unwrap(), sid).unwrap()
    });
    budget("resolve_session", p95, 200.0);

    // The prompt trigger: the loop guard's handover read, recognition, the
    // resolver.
    let prompt = format!("please pick up {} and finish it", f.hot_key);
    let (_, sql) = traced(store, || {
        super::detect::on_prompt(&lock(store).unwrap(), sid, &prompt, false).unwrap()
    });
    assert_no_full_scans(store, "on_prompt", &sql);
    let (_, p95) = measure("on_prompt (busiest session)", || {
        super::detect::on_prompt(&lock(store).unwrap(), sid, &prompt, false).unwrap()
    });
    budget("on_prompt", p95, 200.0);

    // The pure resolver over an evidence set far past any real session's:
    // 60 candidates against 60 links.
    let ev = |n: usize| Evidence {
        signal: Signal::PromptKey,
        rule: "R6".into(),
        text: format!("ACME-{n}"),
        snippet: None,
        at: n as i64,
        conversation: Some("c".into()),
        note: None,
    };
    let cand = |n: usize, signal: Signal, strength: Strength| Candidate {
        target: format!("ACME-{n}"),
        signal,
        strength,
        ambiguous: false,
        first_prompt_sole: false,
        tracker_id: Some(1),
        untracked: false,
        evidence: ev(n),
    };
    let input = ResolveInput {
        conversation: Some("c".into()),
        branch: Some(vec![cand(1, Signal::Branch, Strength::Strong)]),
        pr: Some(
            (2..6)
                .map(|n| cand(n, Signal::PrHead, Strength::Strong))
                .collect(),
        ),
        events: (0..55)
            .map(|n| cand(n * 3, Signal::PromptKey, Strength::Weak))
            .collect(),
        links: (0..60)
            .map(|n| ExistingLink {
                id: n as i64 + 1,
                target: format!("ACME-{}", n * 2),
                state: if n % 7 == 0 { "rejected" } else { "suggested" }.into(),
                source: "prompt".into(),
                strength: Some(Strength::Weak),
                claude_session_id: Some("c".into()),
                is_primary: false,
                decided_at: n as i64,
                evidence_len: 1,
            })
            .collect(),
        trusted: true,
    };
    let changes = resolve(&input);
    assert!(!changes.is_empty());
    let (_, p95) = measure("resolve (pure), 60 candidates × 60 links", || {
        resolve(&input)
    });
    budget("resolve (pure)", p95, 50.0);
}

#[test]
fn scale_recent_ended_work_links() {
    let f = fixture().lock().unwrap_or_else(|e| e.into_inner());
    let store = &f.store;
    let week = f.now - 7 * 86_400;
    let (links, sql) = traced(store, || {
        lock(store)
            .unwrap()
            .recent_ended_work_links(week, 200)
            .unwrap()
    });
    assert!(!links.is_empty() && links.len() <= 200);
    assert!(links.windows(2).all(|w| w[0].ended_at >= w[1].ended_at));
    assert_no_full_scans(store, "recent_ended_work_links", &sql);
    let (_, p95) = measure("recent_ended_work_links (7 days, 200)", || {
        lock(store)
            .unwrap()
            .recent_ended_work_links(week, 200)
            .unwrap()
    });
    budget("recent_ended_work_links (7 days)", p95, 100.0);
    let (_, p95) = measure("recent_ended_work_links (a year, 200)", || {
        lock(store)
            .unwrap()
            .recent_ended_work_links(0, 200)
            .unwrap()
    });
    budget("recent_ended_work_links (all)", p95, 100.0);
}

/// The fixes M12.2 made, pinned by plan shape (exact, unlike the clock):
/// migration 058's two indexes and the key lookup's rewrite.
#[test]
fn scale_plans_pin_the_m12_fixes() {
    let f = fixture().lock().unwrap_or_else(|e| e.into_inner());
    let store = &f.store;
    let plan_of = |sql: &[String], needle: &str| -> Vec<String> {
        let stmt = sql
            .iter()
            .find(|s| s.contains(needle))
            .unwrap_or_else(|| panic!("no statement with {needle:?} in {sql:#?}"));
        lock(store).unwrap().query_plan(stmt)
    };
    let uses = |plan: &[String], index: &str| plan.iter().any(|l| l.contains(index));

    // `recent_ended_work_links`: the ended-at index, newest first, no sort.
    let (_, sql) = traced(store, || {
        lock(store)
            .unwrap()
            .recent_ended_work_links(f.now - 86_400, 200)
            .unwrap()
    });
    let plan = plan_of(&sql, "ended_at IS NOT NULL");
    assert!(uses(&plan, "idx_work_links_ended"), "{plan:?}");
    assert!(!plan.iter().any(|l| l.contains("TEMP B-TREE")), "{plan:?}");

    // The prompt's loop guard: a participant's handovers by index.
    let pid = lock(store)
        .unwrap()
        .detection_state(f.busiest_session)
        .unwrap()
        .unwrap()
        .participant;
    let (_, sql) = traced(store, || {
        lock(store).unwrap().recent_handover_bodies(pid, 5).unwrap()
    });
    let plan = plan_of(&sql, "kind = 'handover'");
    assert!(
        uses(&plan, "idx_work_journal_participant_handover"),
        "{plan:?}"
    );

    // Live work on a key: both arms of the OR by index, never a walk of
    // every live link.
    let key = f.hot_key.clone();
    let (live, sql) = traced(store, || {
        lock(store)
            .unwrap()
            .live_work_sessions_for_key(&key)
            .unwrap()
    });
    assert!(!live.is_empty());
    let plan = plan_of(&sql, "ref_key =");
    assert!(uses(&plan, "idx_work_links_ref"), "{plan:?}");
    assert!(uses(&plan, "idx_work_links_item"), "{plan:?}");
    assert!(!plan.iter().any(|l| l.starts_with("SCAN")), "{plan:?}");
}

/// Work graph M12.3: retention at scale, on its own copy of the fixture
/// (the sweep deletes, and the shared one is read-only). Two years on, so
/// most of the fixture is past its window. The number that matters is one
/// batch's time: that is how long the sweep holds the store lock.
#[test]
fn scale_retention_status_and_sweep_batches() {
    use crate::service::work::retention::{self, RetentionDays, RETENTION_BATCH};
    use crate::store::RetentionTable;
    let f = scale_fixture::build(SEED, crate::service::catalog::now_secs());
    let store = Mutex::new(f.store);
    let now = f.now + 2 * 365 * 86_400;
    let (_, p95) = measure("retention status (counts + dry run)", || {
        retention::status(&store, now).unwrap()
    });
    budget("retention status", p95, 3_000.0);
    let dry = retention::status(&store, now).unwrap();
    let days = RetentionDays::from_store(&lock(&store).unwrap());
    let mut slowest = 0.0_f64;
    for (t, row) in RetentionTable::ALL.iter().zip(&dry.tables) {
        let mut deleted = 0_i64;
        loop {
            let s = lock(&store).unwrap();
            let start = Instant::now();
            let n = s
                .retention_delete_batch(*t, now, days.of(*t), RETENTION_BATCH)
                .unwrap();
            slowest = slowest.max(start.elapsed().as_secs_f64() * 1_000.0);
            deleted += n as i64;
            if n < RETENTION_BATCH {
                break;
            }
        }
        println!(
            "[m12.3 scale] {}: {} rows, {} swept",
            row.table, row.rows, deleted
        );
        assert_eq!(deleted, row.would_delete, "{}: dry run = sweep", row.table);
    }
    println!("[m12.3 scale] slowest batch (lock held): {slowest:.1} ms");
    budget("retention batch", slowest, 1_000.0);
}
