//! Test-only generator of a realistic PRE-work-graph database (M12.1).
//!
//! The schema is built from the historical migration files themselves —
//! `MIGRATIONS` 001..=[`PRE_WORK_GRAPH_VERSION`], the exact files v0.2.37
//! shipped — never from today's structs, so the data cannot drift from what a
//! real install of that release holds (M12 plan, Risks). Rows are written with
//! raw SQL against that schema, never through `Store` methods, which only know
//! the current one.
//!
//! Deterministic: every choice comes from a seeded SplitMix64 and every
//! timestamp is an offset from a fixed epoch, so the same seed gives the same
//! database byte for byte. No real user data is involved or committed.

use rusqlite::{params, Connection};

/// The last migration of the last release without the work graph: v0.2.37
/// ends at 044 (`read_cursors`); v0.2.38 is the first to carry 045 onward
/// (045–053 work graph, 054/055 peer links, all in one release).
pub(crate) const PRE_WORK_GRAPH_VERSION: i64 = 44;

/// A fixed "now" for the generated history (2025-06-15).
const EPOCH: i64 = 1_750_000_000;
const DAY: i64 = 86_400;

/// How much to generate. `Default` is the M12.1 volume.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Shape {
    pub hosts: usize,
    pub projects: usize,
    pub worktrees: usize,
    pub sessions: usize,
    pub conversations: usize,
    pub events: usize,
}

impl Default for Shape {
    fn default() -> Self {
        Shape {
            hosts: 20,
            projects: 40,
            worktrees: 150,
            sessions: 500,
            conversations: 2_000,
            events: 5_000,
        }
    }
}

/// The owners projects are spread over. `local` is the placeholder owner of
/// an adopted folder; `Acme-Corp` is mixed-case on purpose (org rules match
/// owners case-insensitively). Projects of `initech` live under
/// [`INITECH_ROOT`], the path an org rule can match.
pub(crate) const OWNERS: &[&str] = &["Acme-Corp", "globex", "initech", "local", "me", "oss"];
pub(crate) const INITECH_ROOT: &str = "/srv/initech";

/// What a session looked like, for computing expectations (org derivation)
/// without reading the database back through today's code.
#[derive(Debug, Clone)]
pub(crate) struct SessionFacts {
    pub id: i64,
    pub host: String,
    pub owner: Option<String>,
    /// The worktree's path when it has one, else the project's base path.
    pub path: Option<String>,
}

/// What the generator wrote, so a test can assert against it.
#[derive(Debug, Default)]
pub(crate) struct Manifest {
    pub hosts: Vec<String>,
    pub sessions: Vec<SessionFacts>,
    /// Sessions that already had a participant at 044 (043's backfill, or a
    /// message since), with that participant's id.
    pub session_participants: Vec<(i64, i64)>,
    /// Sessions with no participant at 044: created after 043 and never
    /// messaged. 045 must mint exactly one for each.
    pub sessions_without_participant: Vec<i64>,
    pub retired_participants: usize,
    pub client_participants: usize,
    /// Row count per table, as generated.
    pub counts: Vec<(&'static str, i64)>,
    /// `SUM(sessions.row_version)`: a migration that UPDATEs a session row
    /// bumps it (042's trigger).
    pub row_version_sum: i64,
}

impl Manifest {
    pub(crate) fn count(&self, table: &str) -> i64 {
        self.counts
            .iter()
            .find(|(t, _)| *t == table)
            .map(|(_, n)| *n)
            .unwrap_or_else(|| panic!("no count recorded for {table}"))
    }
}

/// SplitMix64: tiny, seedable, good enough to spread test data.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Rng(seed)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `0..n` (`n > 0`).
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    /// True with probability `pct`/100.
    fn pct(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
    fn uuid(&mut self) -> String {
        let (a, b) = (self.next(), self.next());
        format!(
            "{:08x}-{:04x}-4{:03x}-a{:03x}-{:012x}",
            a >> 32,
            (a >> 16) & 0xffff,
            a & 0xfff,
            (b >> 48) & 0xfff,
            b & 0xffff_ffff_ffff
        )
    }
}

/// Apply the historical migrations 001..=`PRE_WORK_GRAPH_VERSION` to an
/// empty connection, foreign keys on, as a real install ran them.
fn pre_work_graph_schema(conn: &Connection) {
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    for (version, sql) in super::schema::migrations_through(PRE_WORK_GRAPH_VERSION) {
        conn.execute_batch(sql)
            .unwrap_or_else(|e| panic!("historical migration {version}: {e}"));
    }
}

/// An in-memory database at the v0.2.37 schema (044), filled per `shape`.
pub(crate) fn pre_work_graph_db(seed: u64, shape: Shape) -> (Connection, Manifest) {
    let conn = Connection::open_in_memory().unwrap();
    pre_work_graph_schema(&conn);
    let manifest = fill(&conn, seed, shape);
    (conn, manifest)
}

fn fill(conn: &Connection, seed: u64, shape: Shape) -> Manifest {
    let mut rng = Rng::new(seed);
    let mut m = Manifest::default();
    let tx = conn.unchecked_transaction().unwrap();

    // Accounts and settings.
    let accounts: Vec<String> = (0..3).map(|_| rng.uuid()).collect();
    for (i, uuid) in accounts.iter().enumerate() {
        tx.execute(
            "INSERT INTO accounts (uuid, email, display_name, seat_tier, last_seen_at, nickname) \
             VALUES (?1, ?2, ?3, 'max', ?4, ?5)",
            params![
                uuid,
                format!("dev{i}@example.test"),
                format!("Dev {i}"),
                EPOCH - i as i64 * DAY,
                (i == 0).then_some("main"),
            ],
        )
        .unwrap();
    }
    for (k, v) in [
        ("reconcile.interval_secs", "30"),
        ("ui.theme", "dark"),
        ("mcp.enabled", "true"),
    ] {
        tx.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)",
            params![k, v],
        )
        .unwrap();
    }

    // Hosts: `local` plus remotes, a few over the agent transport.
    for i in 0..shape.hosts {
        let alias = if i == 0 {
            "local".to_string()
        } else {
            format!("host-{i:02}")
        };
        tx.execute(
            "INSERT INTO hosts (alias, last_pinged_at, reachable, claude_version, tmux_version, \
                                hidden, ssh_alias, account_uuid, provisioned, transport, boot_id) \
             VALUES (?1, ?2, ?3, '2.1.0', '3.4', ?4, ?5, ?6, 1, ?7, ?8)",
            params![
                alias,
                EPOCH - rng.below(3_600) as i64,
                rng.pct(90) as i64,
                (i % 11 == 7) as i64,
                (i > 0).then(|| format!("{alias}.example.test")),
                rng.pct(70).then(|| rng.pick(&accounts).clone()),
                if i % 6 == 5 { "agent" } else { "ssh" },
                rng.pct(80).then(|| rng.uuid()),
            ],
        )
        .unwrap();
        m.hosts.push(alias);
    }

    // Projects over the owners; `local`-owned ones are adopted folders.
    let mut projects: Vec<(i64, String, String)> = Vec::new(); // (id, owner, base_path)
    for i in 0..shape.projects {
        let owner = OWNERS[i % OWNERS.len()];
        let repo = format!("repo-{i:02}");
        let base = if owner == "initech" {
            format!("{INITECH_ROOT}/{repo}")
        } else {
            format!("/home/dev/src/{}/{repo}", owner.to_lowercase())
        };
        tx.execute(
            "INSERT INTO projects (owner, repo, base_path, last_session_at, adopted, system) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                owner,
                repo,
                base,
                EPOCH - rng.below(30) as i64 * DAY,
                (owner == "local") as i64,
                (i == shape.projects - 1) as i64,
            ],
        )
        .unwrap();
        projects.push((tx.last_insert_rowid(), owner.to_string(), base));
    }

    // Worktrees: per project and host, UNIQUE(project_id, host_alias, name).
    let mut worktrees: Vec<(i64, i64, String, String)> = Vec::new(); // (id, project, host, path)
    for i in 0..shape.worktrees {
        let (pid, _, base) = projects[i % projects.len()].clone();
        let host = m.hosts[rng.below(m.hosts.len())].clone();
        let name = format!("wt-{i:03}");
        let path = format!("{base}/.worktrees/{name}");
        tx.execute(
            "INSERT INTO worktrees (project_id, host_alias, name, path, branch, updated_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                pid,
                host,
                name,
                path,
                rng.pct(90)
                    .then(|| format!("feat/PROJ-{}-thing", 100 + rng.below(900))),
                (EPOCH - rng.below(10 * DAY as usize) as i64) * 1000,
            ],
        )
        .unwrap();
        worktrees.push((tx.last_insert_rowid(), pid, host, path));
    }

    // Client tokens (their participants come below).
    let clients = 3;
    for i in 0..clients {
        tx.execute(
            "INSERT INTO client_tokens (name, token_sha256, mode, created_at, last_seen_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                format!("phone-{i}"),
                format!("{:064x}", rng.next()),
                if i == 0 { "full" } else { "readonly" },
                EPOCH - 20 * DAY,
                EPOCH - rng.below(DAY as usize) as i64,
            ],
        )
        .unwrap();
    }

    // Sessions: mixed kinds and statuses. A review session names an earlier
    // work session; a bg worker its parent.
    const KINDS: &[(&str, u64)] = &[("work", 70), ("review", 10), ("bg", 15), ("external", 5)];
    let mut work_ids: Vec<i64> = Vec::new();
    let mut claude_ids: Vec<(i64, Option<String>, i64)> = Vec::new(); // (id, current conv, created)
    for i in 0..shape.sessions {
        let kind = {
            let mut roll = rng.below(100) as u64;
            KINDS
                .iter()
                .find(|(_, w)| {
                    if roll < *w {
                        true
                    } else {
                        roll -= w;
                        false
                    }
                })
                .unwrap()
                .0
        };
        // 60% sit in a worktree (project and host follow it), 25% at a
        // project's root on some host, the rest in no project at all.
        let roll = rng.below(100);
        let (host, project, worktree) = if roll < 60 {
            let (wid, pid, host, _) = worktrees[rng.below(worktrees.len())].clone();
            (host, Some(pid), Some(wid))
        } else if roll < 85 {
            let pid = projects[rng.below(projects.len())].0;
            (m.hosts[rng.below(m.hosts.len())].clone(), Some(pid), None)
        } else {
            (m.hosts[rng.below(m.hosts.len())].clone(), None, None)
        };
        let created = EPOCH - rng.below(60 * DAY as usize) as i64;
        let last = created + rng.below(5 * DAY as usize) as i64;
        let ghost = rng.pct(25);
        let status = if ghost { "ghost" } else { "running" };
        let lost = ghost && rng.pct(40);
        let claude = (kind != "external" || rng.pct(50)) && rng.pct(88);
        let claude_id = claude.then(|| rng.uuid());
        let reviews = (kind == "review" && !work_ids.is_empty()).then(|| *rng.pick(&work_ids));
        let parent = (kind == "bg" && !work_ids.is_empty()).then(|| *rng.pick(&work_ids));
        let wt_key = worktree.map(|w| format!("wt-{:03}", (w - 1) as usize % shape.worktrees));
        tx.execute(
            "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id, created_at, \
                 last_activity_at, status, notes, account_uuid, kind, reviews_session_id, \
                 worktree_key, lost_at, lost_reason, claude_session_id, claude_status, pr_url, \
                 friendly_name, idle_since, parent_session_id, tags, transcript_path, turn_seq, \
                 usage_input_tokens, usage_output_tokens, usage_cost_micros, model, \
                 row_version, prompt_submit_seq, tmux_pane_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
                     ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30)",
            params![
                format!("{kind}-{i:04}"),
                host,
                project,
                worktree,
                created,
                last,
                status,
                rng.pct(10).then_some("remember to rebase"),
                rng.pct(60).then(|| rng.pick(&accounts).clone()),
                kind,
                reviews,
                wt_key,
                lost.then_some(last + 60),
                lost.then(|| *rng.pick(&["host_rebooted", "tmux_gone"])),
                claude_id,
                if ghost {
                    None
                } else {
                    Some(*rng.pick(&["working", "idle", "stopped"]))
                },
                rng.pct(20)
                    .then(|| format!("https://github.com/o/r/pull/{}", 1 + rng.below(4000))),
                rng.pct(30)
                    .then(|| format!("PROJ-{} fix", 100 + rng.below(900))),
                rng.pct(40).then_some(last),
                parent,
                rng.pct(10).then_some("[\"hot\"]"),
                claude_id
                    .as_ref()
                    .map(|c| format!("/home/dev/.claude/projects/x/{c}.jsonl")),
                rng.below(200) as i64,
                rng.below(5_000_000) as i64,
                rng.below(500_000) as i64,
                rng.below(20_000_000) as i64,
                rng.pct(80).then_some("claude-model"),
                rng.below(50) as i64,
                rng.below(100) as i64,
                (!ghost).then(|| format!("%{}", rng.below(300))),
            ],
        )
        .unwrap();
        let id = tx.last_insert_rowid();
        if kind == "work" {
            work_ids.push(id);
        }
        claude_ids.push((id, claude_id, created));
        let (owner, path) = match (project, worktree) {
            (_, Some(w)) => {
                let (_, pid, _, path) = worktrees.iter().find(|x| x.0 == w).unwrap();
                let owner = &projects.iter().find(|p| p.0 == *pid).unwrap().1;
                (Some(owner.clone()), Some(path.clone()))
            }
            (Some(p), None) => {
                let (_, owner, base) = projects.iter().find(|x| x.0 == p).unwrap();
                (Some(owner.clone()), Some(base.clone()))
            }
            (None, None) => (None, None),
        };
        m.sessions.push(SessionFacts {
            id,
            host,
            owner,
            path,
        });
    }

    // Participants as 044 left them: 043 backfilled one per session that
    // existed then (the older ~60%) and a message minted one lazily since;
    // the newest sessions that never messaged have none — 045's backfill.
    // Plus retired tombstones (deleted sessions) and client participants.
    let mut by_age: Vec<(i64, i64)> = claude_ids.iter().map(|(id, _, c)| (*c, *id)).collect();
    by_age.sort();
    let cutoff = by_age.len() * 6 / 10;
    for (n, &(_, sid)) in by_age.iter().enumerate() {
        if n < cutoff || rng.pct(15) {
            tx.execute(
                "INSERT INTO participants (kind, session_id, created_at) VALUES ('session', ?1, ?2)",
                params![sid, EPOCH - 30 * DAY],
            )
            .unwrap();
            m.session_participants.push((sid, tx.last_insert_rowid()));
        } else {
            m.sessions_without_participant.push(sid);
        }
    }
    m.session_participants.sort();
    m.sessions_without_participant.sort();
    for _ in 0..40 {
        let born = EPOCH - rng.below(60 * DAY as usize) as i64;
        tx.execute(
            "INSERT INTO participants (kind, session_id, created_at, retired_at) \
             VALUES ('session', NULL, ?1, ?2)",
            params![born, born + DAY],
        )
        .unwrap();
        m.retired_participants += 1;
    }
    for i in 1..=clients {
        tx.execute(
            "INSERT INTO participants (kind, client_id, created_at) VALUES ('client', ?1, ?2)",
            params![i as i64, EPOCH - 20 * DAY],
        )
        .unwrap();
        m.client_participants += 1;
    }

    // Messages between sessions that have participants.
    for _ in 0..400 {
        let a = *rng.pick(&m.session_participants);
        let b = *rng.pick(&m.session_participants);
        let at = EPOCH - rng.below(20 * DAY as usize) as i64;
        tx.execute(
            "INSERT INTO session_messages (from_session_id, to_session_id, body, kind, sent_at, \
                 read_at, from_participant_id, to_participant_id, delivered_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                a.0,
                b.0,
                "please review PROJ-123",
                if rng.pct(80) { "message" } else { "task" },
                at,
                rng.pct(70).then_some(at + 30),
                a.1,
                b.1,
                rng.pct(80).then_some(at + 10),
            ],
        )
        .unwrap();
    }

    // Conversations: every session with a Claude id has its current one
    // (open, 037's shape); the rest are closed earlier ones, spread over the
    // sessions that ever ran Claude.
    let with_claude: Vec<&(i64, Option<String>, i64)> =
        claude_ids.iter().filter(|(_, c, _)| c.is_some()).collect();
    let mut conv_ids: Vec<(i64, String)> = Vec::new();
    let mut n_conv = 0usize;
    for (sid, cid, created) in &with_claude {
        let cid = cid.clone().unwrap();
        tx.execute(
            "INSERT INTO conversations (session_id, claude_session_id, transcript_path, started_at, \
                 start_source, model, first_prompt, turns, compactions) \
             VALUES (?1, ?2, NULL, ?3, ?4, 'claude-model', ?5, ?6, ?7)",
            params![
                sid,
                cid,
                created + 60,
                *rng.pick(&["startup", "resume", "clear", "unknown"]),
                rng.pct(70)
                    .then(|| format!("PROJ-{}: make it work", 100 + rng.below(900))),
                rng.below(80) as i64,
                rng.below(4) as i64,
            ],
        )
        .unwrap();
        conv_ids.push((*sid, cid));
        n_conv += 1;
    }
    while n_conv < shape.conversations {
        let &&(sid, _, created) = rng.pick(&with_claude);
        let cid = rng.uuid();
        let start = created + rng.below(DAY as usize) as i64;
        tx.execute(
            "INSERT INTO conversations (session_id, claude_session_id, started_at, ended_at, \
                 start_source, end_reason, model, first_prompt, turns, compactions, last_compact_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'claude-model', ?7, ?8, ?9, ?10)",
            params![
                sid,
                cid,
                start,
                start + 1 + rng.below(4 * 3_600) as i64,
                *rng.pick(&["startup", "resume", "clear", "compact"]),
                *rng.pick(&["clear", "logout", "prompt_input_exit", "other"]),
                rng.pct(60).then(|| "fix the flaky test".to_string()),
                rng.below(40) as i64,
                rng.below(3) as i64,
                rng.pct(30).then_some(start + 100),
            ],
        )
        .unwrap();
        conv_ids.push((sid, cid));
        n_conv += 1;
    }

    // Timeline events: every one names a live session (no orphans; `open`
    // reaps those on its own, before and after the work graph alike).
    const EVENT_KINDS: &[&str] = &[
        "status_change",
        "prompt_sent",
        "conversation_started",
        "conversation_ended",
        "compacted",
        "stuck",
        "moved",
    ];
    for _ in 0..shape.events {
        let (sid, cid) = rng.pick(&conv_ids).clone();
        tx.execute(
            "INSERT INTO session_events (session_id, at, kind, detail, claude_session_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                sid,
                EPOCH - rng.below(60 * DAY as usize) as i64,
                *rng.pick(EVENT_KINDS),
                rng.pct(50)
                    .then_some("{\"from\":\"idle\",\"to\":\"working\"}"),
                rng.pct(70).then_some(cid),
            ],
        )
        .unwrap();
    }

    // Tasks, read cursors, usage, dismissed agents: the rest of 044's shape.
    for i in 0..120 {
        let req = rng.pick(&m.sessions).id;
        let worker = rng.pick(&m.sessions).id;
        tx.execute(
            "INSERT INTO tasks (requester_session_id, worker_session_id, prompt, state, created_at, \
                 nonce) VALUES (?1, ?2, 'run the tests', ?3, ?4, ?5)",
            params![
                req,
                worker,
                *rng.pick(&["pending", "running", "done", "failed"]),
                EPOCH - rng.below(30 * DAY as usize) as i64,
                format!("nonce-{i}"),
            ],
        )
        .unwrap();
    }
    for i in 0..300 {
        let reader = rng.pick(&m.sessions).id;
        let target = rng.pick(&m.sessions).id;
        tx.execute(
            "INSERT OR IGNORE INTO read_cursors (reader_session_id, tool, resource_key, \
                 target_session_id, watermark, updated_at) VALUES (?1, 'peek', ?2, ?3, ?4, ?5)",
            params![reader, format!("pane:{i}"), target, i as i64, EPOCH],
        )
        .unwrap();
    }
    for day in 0..30 {
        for host in m.hosts.iter().take(5) {
            tx.execute(
                "INSERT INTO usage_daily (day, host_alias, input_tokens, output_tokens, cost_micros) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    format!("2025-05-{:02}", day + 1),
                    host,
                    rng.below(1_000_000) as i64,
                    rng.below(100_000) as i64,
                    rng.below(5_000_000) as i64,
                ],
            )
            .unwrap();
        }
    }
    tx.commit().unwrap();

    for table in COUNTED_TABLES {
        let n: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        m.counts.push((table, n));
    }
    m.row_version_sum = conn
        .query_row("SELECT SUM(row_version) FROM sessions", [], |r| r.get(0))
        .unwrap();
    m
}

/// The pre-work-graph tables whose row counts a migration must preserve.
pub(crate) const COUNTED_TABLES: &[&str] = &[
    "accounts",
    "hosts",
    "projects",
    "worktrees",
    "sessions",
    "conversations",
    "session_events",
    "session_messages",
    "client_tokens",
    "tasks",
    "read_cursors",
    "usage_daily",
    "settings",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(conn: &Connection) -> Vec<String> {
        let mut out = Vec::new();
        for table in COUNTED_TABLES.iter().chain(&["participants"]) {
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .unwrap();
            let cols = stmt.column_count();
            let rows = stmt
                .query_map([], |r| {
                    let mut s = String::new();
                    for i in 0..cols {
                        s.push_str(&format!("{:?}|", r.get_ref(i)?));
                    }
                    Ok(s)
                })
                .unwrap();
            out.extend(rows.map(Result::unwrap));
        }
        out
    }

    /// Same seed, same database; the volumes are the ones asked for.
    #[test]
    fn the_generator_is_deterministic_and_sized() {
        let (a, ma) = pre_work_graph_db(7, Shape::default());
        let (b, _) = pre_work_graph_db(7, Shape::default());
        assert_eq!(dump(&a), dump(&b));
        let (c, _) = pre_work_graph_db(8, Shape::default());
        assert_ne!(dump(&a), dump(&c), "the seed must matter");

        let shape = Shape::default();
        assert_eq!(ma.count("hosts"), shape.hosts as i64);
        assert_eq!(ma.count("sessions"), shape.sessions as i64);
        assert_eq!(ma.count("conversations"), shape.conversations as i64);
        assert_eq!(ma.count("session_events"), shape.events as i64);
        assert_eq!(ma.count("projects"), shape.projects as i64);
        assert_eq!(ma.count("worktrees"), shape.worktrees as i64);
        let version: i64 = a
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, PRE_WORK_GRAPH_VERSION);
        // Both halves of 045's input are there.
        assert!(!ma.session_participants.is_empty());
        assert!(!ma.sessions_without_participant.is_empty());
        // Every kind and both statuses occur.
        for kind in ["work", "review", "bg", "external"] {
            let n: i64 = a
                .query_row(
                    "SELECT COUNT(*) FROM sessions WHERE kind = ?1",
                    [kind],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(n > 0, "no {kind} sessions");
        }
    }
}
