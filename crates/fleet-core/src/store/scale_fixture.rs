//! Work graph M12.2 (scale): a seeded, deterministic store at the volumes a
//! year of heavy use reaches, and a query-plan checker for the budget tests
//! in `service/work/scale_tests.rs`. Test-only.
//!
//! The rows are written with plain SQL in one transaction (the same shapes
//! the migrations and the store's writers leave behind, sessions' insert
//! trigger included) so building the fixture costs well under a second
//! instead of the minutes the event-emitting writers would take.
//!
//! Volumes: 20 hosts, 3 orgs, 40 projects, 2,000 sessions (with their
//! participants and worktrees), 5,000 tracker items over three trackers,
//! 20,000 work links in every state (about 4,000 live, the rest ended on
//! retired participants), 50,000 journal rows, 4,000 conversations and
//! 40,000 timeline events.

use super::Store;
use rusqlite::params;
use std::cell::RefCell;
use std::collections::HashMap;

pub(crate) const HOSTS: usize = 20;
pub(crate) const ORGS: i64 = 3;
pub(crate) const PROJECTS: usize = 40;
pub(crate) const SESSIONS: usize = 2_000;
pub(crate) const ITEMS: usize = 5_000;
pub(crate) const LINKS: usize = 20_000;
pub(crate) const JOURNAL: usize = 50_000;
pub(crate) const CONVERSATIONS: usize = 4_000;
pub(crate) const EVENTS: usize = 40_000;
/// Distinct Claude conversation ids the journal and the ended links cite.
const CONV_IDS: usize = 10_000;
/// The tracker account the "mine" views read (`TrackerConfig::account_id`).
pub(crate) const ME: &str = "acct-me";
pub(crate) const PREFIXES: [&str; 3] = ["ACME", "BETA", "CORE"];

/// splitmix64: small, seeded, and the same on every platform.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Rng(seed)
    }
    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `0..n`.
    pub(crate) fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    /// True with probability `pct` / 100.
    pub(crate) fn pct(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
    pub(crate) fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

pub(crate) fn host_alias(i: usize) -> String {
    format!("h{i:02}")
}

/// A host's org: every fourth host is unassigned, the rest spread over
/// orgs 1..=3 (host `h01` is org 1).
pub(crate) fn host_org(i: usize) -> Option<i64> {
    (!i.is_multiple_of(4)).then_some((i % 4) as i64)
}

/// What the benchmarks need to find their way in the fixture.
pub(crate) struct ScaleFixture {
    pub store: Store,
    pub now: i64,
    /// The session with the most live links (the resolver's worst case).
    pub busiest_session: i64,
    /// A key with live confirmed work (a tickets lookup hit).
    pub hot_key: String,
}

/// Build the fixture. The rows depend only on `seed` and `now` (timestamps
/// are offsets from `now`, so the time-window reads see the same shape
/// whenever the test runs).
pub(crate) fn build(seed: u64, now: i64) -> ScaleFixture {
    let store = Store::open_in_memory().expect("store");
    let mut rng = Rng::new(seed);
    let day = 86_400_i64;
    let conn = store.conn_for_test();
    let tx = conn.unchecked_transaction().expect("tx");

    // Orgs and their placement rules: an owner rule, an owner/repo rule, a
    // path rule and a host-only rule, so every branch of
    // `session_org_sql!` runs.
    for o in 1..=ORGS {
        tx.execute(
            "INSERT INTO orgs (id, name, color, isolate_sessions, created_at) \
             VALUES (?1, ?2, '#336699', ?3, ?4)",
            params![o, format!("org{o}"), i64::from(o == 3), now - 400 * day],
        )
        .unwrap();
    }
    for (org, owner, repo, path, host) in [
        (1, Some("owner0"), None, None, None),
        (2, Some("owner1"), Some("r1"), None, None),
        (3, None, None, Some("/srv/core"), None),
        (2, None, None, None, Some("h05")),
    ] {
        tx.execute(
            "INSERT INTO org_rules (org_id, owner, repo, path_prefix, host_alias) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![org, owner, repo, path, host],
        )
        .unwrap();
    }
    for h in 0..HOSTS {
        tx.execute(
            "INSERT INTO hosts (alias, last_pinged_at, reachable, org_id) VALUES (?1, ?2, 1, ?3)",
            params![host_alias(h), now - 60, host_org(h)],
        )
        .unwrap();
    }
    for p in 0..PROJECTS {
        let base = if p % 10 == 3 {
            format!("/srv/core/r{p}")
        } else {
            format!("/home/dev/r{p}")
        };
        tx.execute(
            "INSERT INTO projects (id, owner, repo, base_path, last_session_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                p as i64 + 1,
                format!("owner{}", p % 5),
                format!("r{p}"),
                base,
                now
            ],
        )
        .unwrap();
    }

    // Trackers: three Jira-shaped sites, one per org.
    for (t, prefix) in PREFIXES.iter().enumerate() {
        let config = serde_json::json!({ "account_id": ME, "key_prefixes": [prefix] });
        tx.execute(
            "INSERT INTO trackers (id, provider, name, site_url, org_id, config, state, \
                                   last_sync_at, created_at) \
             VALUES (?1, 'jira', ?2, ?3, ?1, ?4, 'ok', ?5, ?6)",
            params![
                t as i64 + 1,
                prefix.to_lowercase(),
                format!("https://{}.example.net", prefix.to_lowercase()),
                config.to_string(),
                now - 300,
                now - 400 * day,
            ],
        )
        .unwrap();
    }
    let categories = ["todo", "todo", "in_progress", "in_progress", "done"];
    let mut item_keys: Vec<String> = Vec::with_capacity(ITEMS);
    for n in 0..ITEMS {
        let t = n % PREFIXES.len();
        let key = format!("{}-{}", PREFIXES[t], n / PREFIXES.len() + 1);
        let cat = *rng.pick(&categories);
        let changed = now - rng.below(60) as i64 * day - rng.below(86_400) as i64;
        let mine = rng.pct(10);
        let mut meta = serde_json::json!({ "description": "Acceptance criteria: it works." });
        if mine {
            meta["assignee_id"] = ME.into();
            meta["iteration_active"] = rng.pct(50).into();
        }
        if rng.pct(20) {
            meta["views"] = serde_json::json!(["filter:10001"]);
        }
        tx.execute(
            "INSERT INTO work_items (id, source, tracker_id, external_id, key, title, url, \
                                     status_category, status_name, assignees, meta, \
                                     updated_ext, status_changed_at, fetched_at, \
                                     created_at, updated_at) \
             VALUES (?1, 'tracker', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?11)",
            params![
                n as i64 + 1,
                t as i64 + 1,
                format!("{}", 10_000 + n),
                key,
                format!("Ticket {key}: make the thing"),
                format!("https://x.example.net/browse/{key}"),
                cat,
                match cat {
                    "todo" => "To Do",
                    "in_progress" => "In Progress",
                    _ => "Done",
                },
                if mine { r#"["Me"]"# } else { r#"["Someone"]"# },
                meta.to_string(),
                changed,
                changed,
                now - 300,
                now - 400 * day,
            ],
        )
        .unwrap();
        item_keys.push(key);
    }

    // Sessions, each in its own worktree; the insert trigger gives each a
    // participant.
    let statuses = ["running"; 17]
        .into_iter()
        .chain(["lost", "lost", "ghost"])
        .collect::<Vec<_>>();
    let kinds = ["work"; 8]
        .into_iter()
        .chain(["bg", "review", "shell"])
        .collect::<Vec<_>>();
    let claude = ["working", "idle", "idle", "waiting", "stuck"];
    for i in 0..SESSIONS {
        let host = host_alias(i % HOSTS);
        let project = (i % PROJECTS) as i64 + 1;
        let wt = i as i64 + 1;
        let branch = if rng.pct(40) {
            format!("feat/{}-thing", rng.pick(&item_keys))
        } else {
            format!("wip-{i}")
        };
        tx.execute(
            "INSERT INTO worktrees (id, project_id, host_alias, name, path, branch) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                wt,
                project,
                host,
                format!("wt{i}"),
                format!("/home/dev/r{}/.wt/wt{i}", project - 1),
                branch
            ],
        )
        .unwrap();
        let status = *rng.pick(&statuses);
        let last = now - rng.below(30 * 86_400) as i64;
        tx.execute(
            "INSERT INTO sessions (id, tmux_name, host_alias, project_id, worktree_id, \
                                   created_at, last_activity_at, status, kind, worktree_key, \
                                   lost_at, claude_session_id, claude_status, pr_url, \
                                   current_branch, idle_since, last_touch_at, friendly_name) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?7, ?17)",
            params![
                i as i64 + 1,
                format!("s{i}"),
                host,
                project,
                wt,
                last - 5 * day,
                last,
                status,
                *rng.pick(&kinds),
                format!("wt{i}"),
                (status == "lost").then_some(last),
                format!("conv-{}", i * 2 + 1),
                *rng.pick(&claude),
                rng.pct(30)
                    .then(|| format!("https://github.com/owner0/r{}/pull/{i}", project - 1)),
                branch,
                rng.pct(50).then_some(last),
                format!("session {i}"),
            ],
        )
        .unwrap();
    }
    let participant_of: HashMap<i64, i64> = {
        let mut stmt = tx
            .prepare("SELECT session_id, id FROM participants WHERE session_id IS NOT NULL")
            .unwrap();
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .unwrap();
        rows.collect::<rusqlite::Result<_>>().unwrap()
    };
    assert_eq!(participant_of.len(), SESSIONS);

    // Conversations: two per session, the first ended.
    for c in 0..CONVERSATIONS {
        let sid = (c / 2) as i64 + 1;
        let started = now - rng.below(60) as i64 * day;
        tx.execute(
            "INSERT INTO conversations (session_id, claude_session_id, started_at, ended_at, \
                                        start_source, first_prompt, turns) \
             VALUES (?1, ?2, ?3, ?4, 'startup', 'fix the bug', ?5)",
            params![
                sid,
                format!("conv-{c}"),
                started,
                (c % 2 == 0).then_some(started + 3600),
                rng.below(40) as i64,
            ],
        )
        .unwrap();
    }

    // Live links: a confirmed primary on 60% of the sessions (one in ten a
    // bare key), suggestions of every strength, rejections, secondaries.
    let mut live = 0usize;
    let mut per_session: HashMap<i64, usize> = HashMap::new();
    let mut hot_key = None;
    let sources = ["manual", "branch", "started", "pr", "agent"];
    let insert_live = |tx: &rusqlite::Transaction<'_>,
                       rng: &mut Rng,
                       pid: i64,
                       state: &str,
                       primary: bool,
                       source: &str,
                       strength: &str|
     -> String {
        let n = rng.below(ITEMS);
        let bare = rng.pct(10);
        let key = item_keys[n].clone();
        let evidence = serde_json::json!([{
            "signal": "branch", "rule": "R3", "text": format!("feat/{key}-thing"),
            "at": now - 3600
        }]);
        tx.execute(
            "INSERT INTO work_links (item_id, ref_key, participant_id, state, source, \
                                     is_primary, created_at, decided_at, strength, rule, \
                                     evidence, preselected, claude_session_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'R3', ?10, ?11, ?12)",
            params![
                (!bare).then_some(n as i64 + 1),
                bare.then_some(key.clone()),
                pid,
                state,
                source,
                i64::from(primary),
                now - rng.below(20 * 86_400) as i64,
                (state != "suggested").then_some(now - 3600),
                strength,
                evidence.to_string(),
                i64::from(state == "suggested" && rng.pct(30)),
                format!("conv-{}", rng.below(CONV_IDS)),
            ],
        )
        .unwrap();
        key
    };
    for i in 0..SESSIONS {
        let sid = i as i64 + 1;
        let pid = participant_of[&sid];
        let mut n = 0;
        if i % 10 < 6 {
            let src = *rng.pick(&sources);
            let k = insert_live(&tx, &mut rng, pid, "confirmed", true, src, "strong");
            if hot_key.is_none() {
                hot_key = Some(k);
            }
            n += 1;
        }
        if rng.pct(20) {
            insert_live(&tx, &mut rng, pid, "confirmed", false, "manual", "explicit");
            n += 1;
        }
        let suggestions = if i % 10 == 6 { 3 } else { rng.below(3) };
        for _ in 0..suggestions {
            let strength = *rng.pick(&["weak", "strong", "inferred"]);
            insert_live(&tx, &mut rng, pid, "suggested", false, "prompt", strength);
            n += 1;
        }
        if rng.pct(25) {
            insert_live(&tx, &mut rng, pid, "rejected", false, "branch", "strong");
            n += 1;
        }
        // One session carries far more than any real one would.
        if i == 7 {
            for _ in 0..40 {
                insert_live(&tx, &mut rng, pid, "suggested", false, "prompt", "weak");
                n += 1;
            }
        }
        live += n;
        per_session.insert(sid, n);
    }

    // Ended links: on retired participants of sessions long gone, with the
    // snapshot the retire trigger leaves.
    let ended = LINKS - live;
    let retired = 4_000usize;
    let first_retired: i64 = {
        let mut first = 0;
        for r in 0..retired {
            tx.execute(
                "INSERT INTO participants (kind, session_id, created_at, retired_at) \
                 VALUES ('session', NULL, ?1, ?2)",
                params![now - 400 * day, now - rng.below(365) as i64 * day],
            )
            .unwrap();
            if r == 0 {
                first = tx.last_insert_rowid();
            }
        }
        first
    };
    let end_reasons = [
        "session_ended",
        "unlinked",
        "branch_changed",
        "session_deleted",
    ];
    for _ in 0..ended {
        let n = rng.below(ITEMS);
        let bare = rng.pct(10);
        let state = if rng.pct(85) {
            "confirmed"
        } else if rng.pct(66) {
            "suggested"
        } else {
            "rejected"
        };
        let ended_at = now - rng.below(365 * 86_400) as i64;
        let h = rng.below(HOSTS);
        let conv = rng.below(CONV_IDS);
        tx.execute(
            "INSERT INTO work_links (item_id, ref_key, participant_id, state, source, \
                                     is_primary, created_at, decided_at, ended_at, snap_host, \
                                     snap_tmux, snap_name, snap_project_id, snap_worktree, \
                                     snap_branch, snap_pr_url, snap_claude_ids, snap_org_id, \
                                     strength, end_reason) \
             VALUES (?1, ?2, ?3, ?4, 'branch', 0, ?5, ?5, ?6, ?7, ?8, ?8, ?9, ?10, ?11, ?12, \
                     ?13, ?14, 'strong', ?15)",
            params![
                (!bare).then_some(n as i64 + 1),
                bare.then_some(item_keys[n].clone()),
                first_retired + rng.below(retired) as i64,
                state,
                ended_at - 5 * day,
                ended_at,
                host_alias(h),
                format!("old{}", rng.below(100_000)),
                rng.below(PROJECTS) as i64 + 1,
                format!("wt-old{}", rng.below(1000)),
                format!("feat/{}", item_keys[n]),
                rng.pct(50)
                    .then(|| format!("https://github.com/owner0/r1/pull/{}", rng.below(9000))),
                format!(r#"["conv-{conv}","conv-{}"]"#, (conv + 1) % CONV_IDS),
                host_org(h),
                *rng.pick(&end_reasons),
            ],
        )
        .unwrap();
    }

    // The journal: every kind, over a year, conversation rows unique per
    // conversation (the migration's partial unique index).
    let kinds = [
        ("progress", 20),
        ("status_change", 30),
        ("handover", 10),
        ("note", 10),
        ("outcome", 5),
        ("compact_summary", 5),
        ("tidy", 5),
        ("reopened", 5),
    ];
    let weighted: Vec<&str> = kinds
        .iter()
        .flat_map(|(k, w)| std::iter::repeat_n(*k, *w))
        .collect();
    let live_pids: Vec<i64> = {
        let mut v: Vec<i64> = participant_of.values().copied().collect();
        v.sort_unstable();
        v
    };
    for j in 0..JOURNAL {
        let at = now - rng.below(365 * 86_400) as i64;
        let (kind, csid) = if j < CONV_IDS {
            ("conversation", format!("conv-{j}"))
        } else {
            (
                *rng.pick(&weighted),
                format!("conv-{}", rng.below(CONV_IDS)),
            )
        };
        let pid = if kind == "handover" || rng.pct(30) {
            Some(if rng.pct(50) {
                *rng.pick(&live_pids)
            } else {
                first_retired + rng.below(retired) as i64
            })
        } else {
            None
        };
        tx.execute(
            "INSERT INTO work_journal (claude_session_id, participant_id, at, kind, source, \
                                       body, meta, delivered_at) \
             VALUES (?1, ?2, ?3, ?4, 'fleet', ?5, NULL, ?6)",
            params![
                csid,
                pid,
                at,
                kind,
                format!("{kind} body {j}: what happened and what is next"),
                (kind == "handover" && rng.pct(90)).then_some(at + 60),
            ],
        )
        .unwrap();
    }

    // The timeline, including the work graph's own event kinds.
    let ev_kinds = [
        "status", "status", "status", "prompt", "stop", "handover", "nudge", "tidy",
    ];
    for e in 0..EVENTS {
        let sid = (e % SESSIONS) as i64 + 1;
        tx.execute(
            "INSERT INTO session_events (session_id, at, kind, detail, claude_session_id) \
             VALUES (?1, ?2, ?3, 'detail', ?4)",
            params![
                sid,
                now - rng.below(90 * 86_400) as i64,
                *rng.pick(&ev_kinds),
                format!("conv-{}", (sid - 1) * 2 + 1),
            ],
        )
        .unwrap();
    }
    for t in 0..20 {
        tx.execute(
            "INSERT INTO tasks (requester_session_id, worker_session_id, prompt, state, \
                                created_at, nonce) VALUES (?1, ?2, 'do it', 'running', ?3, ?4)",
            params![t * 7 + 1, t * 7 + 2, now - 600, format!("n{t}")],
        )
        .unwrap();
    }
    tx.commit().expect("commit");

    let busiest_session = per_session
        .iter()
        .max_by_key(|(sid, n)| (**n, std::cmp::Reverse(**sid)))
        .map(|(sid, _)| *sid)
        .unwrap();
    ScaleFixture {
        store,
        now,
        busiest_session,
        hot_key: hot_key.expect("a primary link"),
    }
}

/// Tables large enough that reading all of them is the regression the
/// plan check exists to catch.
pub(crate) const BIG_TABLES: &[&str] = &[
    "work_links",
    "work_items",
    "work_journal",
    "session_events",
    "conversations",
    "participants",
];

thread_local! {
    static TRACED: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

fn record(sql: &str) {
    TRACED.with(|t| {
        if let Some(v) = t.borrow_mut().as_mut() {
            v.push(sql.to_string());
        }
    });
}

impl Store {
    /// Test-only: start recording every statement this store runs, with
    /// its bound parameters inlined (SQLite's legacy trace), until
    /// [`Self::finish_trace`]. Recording is per thread.
    pub(crate) fn start_trace(&mut self) {
        TRACED.with(|t| *t.borrow_mut() = Some(Vec::new()));
        self.conn.trace(Some(record));
    }

    /// Test-only: stop recording; the distinct statements run since
    /// [`Self::start_trace`], in first-run order. Statements a trigger runs
    /// are left out (the trace reports them as comments).
    pub(crate) fn finish_trace(&mut self) -> Vec<String> {
        self.conn.trace(None);
        let mut seen = std::collections::HashSet::new();
        TRACED
            .with(|t| t.borrow_mut().take())
            .unwrap_or_default()
            .into_iter()
            .filter(|s| !s.trim_start().starts_with("--") && seen.insert(s.clone()))
            .collect()
    }

    /// Test-only: `EXPLAIN QUERY PLAN` of `sql` (parameters already
    /// inlined), one detail line per plan row.
    pub(crate) fn query_plan(&self, sql: &str) -> Vec<String> {
        let mut stmt = self
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap_or_else(|e| panic!("explain {sql}: {e}"));
        let rows = stmt.query_map([], |r| r.get::<_, String>(3)).expect("plan");
        rows.collect::<rusqlite::Result<_>>().expect("plan rows")
    }

    /// Test-only: whether `index` is a partial index (it has a WHERE).
    fn is_partial_index(&self, index: &str) -> bool {
        self.conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = ?1",
                [index],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
            .is_some_and(|s| s.to_uppercase().contains(" WHERE "))
    }

    /// Test-only: the plan lines of `sql` that read a whole big table: a
    /// `SCAN` of one of [`BIG_TABLES`] (by name or by its alias in `sql`)
    /// that is not a walk of a partial index (a partial index holds only
    /// the live subset — the live links, the undelivered handovers — which
    /// is the set such a read is about).
    pub(crate) fn full_scans(&self, sql: &str) -> Vec<String> {
        let aliases = table_aliases(sql);
        self.query_plan(sql)
            .into_iter()
            .filter(|line| {
                let Some(rest) = line.strip_prefix("SCAN ") else {
                    return false;
                };
                let name = rest.split_whitespace().next().unwrap_or_default();
                let table = aliases.get(name).map(String::as_str).unwrap_or(name);
                if !BIG_TABLES.contains(&table) {
                    return false;
                }
                let index = rest
                    .split("USING COVERING INDEX ")
                    .nth(1)
                    .or_else(|| rest.split("USING INDEX ").nth(1))
                    .and_then(|s| s.split_whitespace().next());
                !index.is_some_and(|i| self.is_partial_index(i))
            })
            .collect()
    }
}

/// `alias → table` for every `FROM t a` / `JOIN t a` / `, t a` in `sql`.
fn table_aliases(sql: &str) -> HashMap<String, String> {
    let words: Vec<String> = sql
        .replace(['(', ')', '\n'], " ")
        .replace(',', " , ")
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let keyword = |w: &str| {
        matches!(
            w.to_uppercase().as_str(),
            "ON" | "WHERE"
                | "LEFT"
                | "JOIN"
                | "INNER"
                | "CROSS"
                | "GROUP"
                | "ORDER"
                | "LIMIT"
                | "USING"
                | "AS"
                | ","
                | "UNION"
                | "SELECT"
                | "HAVING"
                | "SET"
        )
    };
    let mut out = HashMap::new();
    for (i, w) in words.iter().enumerate() {
        if !(w.eq_ignore_ascii_case("FROM") || w.eq_ignore_ascii_case("JOIN") || w == ",") {
            continue;
        }
        let Some(table) = words.get(i + 1) else {
            continue;
        };
        if !BIG_TABLES.contains(&table.as_str()) {
            continue;
        }
        let mut j = i + 2;
        if words.get(j).is_some_and(|w| w.eq_ignore_ascii_case("AS")) {
            j += 1;
        }
        if let Some(alias) = words.get(j).filter(|a| !keyword(a)) {
            out.insert(alias.clone(), table.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixture_is_deterministic_and_the_size_it_claims() {
        let now = 2_000_000_000;
        let a = build(7, now);
        let b = build(7, now);
        let count = |f: &ScaleFixture, t: &str| -> i64 {
            f.store
                .conn_for_test()
                .query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))
                .unwrap()
        };
        for (t, n) in [
            ("sessions", SESSIONS),
            ("work_items", ITEMS),
            ("work_links", LINKS),
            ("work_journal", JOURNAL),
            ("conversations", CONVERSATIONS),
            ("session_events", EVENTS),
            ("hosts", HOSTS),
            ("orgs", ORGS as usize),
        ] {
            assert_eq!(count(&a, t), n as i64, "{t}");
        }
        let digest = |f: &ScaleFixture| -> String {
            f.store
                .conn_for_test()
                .query_row(
                    "SELECT group_concat(state || ':' || COALESCE(item_id, ref_key) || ':' \
                                         || COALESCE(ended_at, 0), ',') \
                     FROM (SELECT * FROM work_links ORDER BY id)",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(digest(&a), digest(&b));
        assert_eq!(a.busiest_session, b.busiest_session);
        let live: i64 = a
            .store
            .conn_for_test()
            .query_row(
                "SELECT COUNT(*) FROM work_links WHERE ended_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!((3_000..6_000).contains(&live), "live links {live}");
        for state in ["confirmed", "suggested", "rejected"] {
            let n: i64 = a
                .store
                .conn_for_test()
                .query_row(
                    "SELECT COUNT(*) FROM work_links WHERE state = ?1",
                    [state],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(n > 200, "{state}: {n}");
        }
    }

    #[test]
    fn the_scan_check_sees_through_aliases_and_allows_partial_indexes() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(
            s.full_scans("SELECT l.id FROM work_links l WHERE l.state = 'x'"),
            vec!["SCAN l".to_string()]
        );
        assert!(s
            .full_scans("SELECT id FROM work_links WHERE participant_id = 1 AND ended_at IS NULL")
            .is_empty());
        assert!(s.full_scans("SELECT id FROM orgs").is_empty());
        assert!(s
            .full_scans("SELECT l.id FROM work_links l WHERE l.ended_at IS NULL")
            .is_empty());
    }
}
