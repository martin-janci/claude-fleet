//! M12.1: the upgrade path, proven on a realistic pre-work-graph database.
//!
//! `store::testgen` builds a v0.2.37-shaped database (the historical
//! migrations 001..=044, then 20 hosts, 500 sessions, 2,000 conversations,
//! 5,000 timeline events, projects and worktrees); these tests run every
//! later migration on it — enumerated from `MIGRATIONS`, so a migration added
//! tomorrow is covered without touching this file — and check what the chain
//! must leave behind. Measured times are printed (`--nocapture`).

use super::*;
use crate::store::testgen::{self, Manifest, Shape, INITECH_ROOT, PRE_WORK_GRAPH_VERSION};
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

/// The whole chain from 044 to the latest on the generated database. Generous
/// for a debug build on a CI runner; the measured number is printed.
const CHAIN_BUDGET: Duration = Duration::from_secs(5);

const SEED: u64 = 0x5EED_0012_0001;

fn store_over(conn: Connection) -> Store {
    Store {
        conn,
        bus: StoreBus::new(Arc::new(NoopEventBus)),
        kills: Default::default(),
        message_notify: Arc::new(tokio::sync::Notify::new()),
        peer_generations: Default::default(),
        instance: super::super::next_instance(),
    }
}

/// The migrations a v0.2.37 database has not seen: every later entry.
fn pending_after_pre_work_graph() -> Vec<Migration> {
    MIGRATIONS
        .iter()
        .copied()
        .filter(|m| m.version > PRE_WORK_GRAPH_VERSION)
        .collect()
}

fn count(store: &Store, sql: &str) -> i64 {
    store.conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn integrity_ok(store: &Store) {
    let integrity: String = store
        .conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
    let dangling = fk_violations(&store.conn).unwrap();
    assert!(dangling.is_empty(), "dangling foreign keys: {dangling:?}");
}

/// Everything a re-run could change: the schema objects, the recorded
/// versions, every table's row count.
fn fingerprint(store: &Store) -> (Vec<(String, String, Option<String>)>, Vec<i64>, Vec<i64>) {
    let mut stmt = store
        .conn
        .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
        .unwrap();
    let objects: Vec<(String, String, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut stmt = store
        .conn
        .prepare("SELECT version FROM schema_version ORDER BY version")
        .unwrap();
    let versions: Vec<i64> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let counts = objects
        .iter()
        .filter(|(ty, name, _)| ty == "table" && !name.starts_with("sqlite_"))
        .map(|(_, name, _)| count(store, &format!("SELECT COUNT(*) FROM \"{name}\"")))
        .collect();
    (objects, versions, counts)
}

/// Generate, then migrate through `migrate()` — the path `open` takes.
fn upgraded() -> (Store, Manifest, Duration) {
    let (conn, manifest) = testgen::pre_work_graph_db(SEED, Shape::default());
    let store = store_over(conn);
    assert_eq!(store.schema_version().unwrap(), PRE_WORK_GRAPH_VERSION);
    let started = Instant::now();
    store
        .migrate()
        .expect("migrate the generated v0.2.37 database");
    (store, manifest, started.elapsed())
}

#[test]
fn the_chain_from_pre_work_graph_to_latest_is_fast_complete_and_sound() {
    let (store, m, elapsed) = upgraded();
    let pending = pending_after_pre_work_graph();
    println!(
        "M12.1: migrations {}..={} on the generated database took {elapsed:?} (budget {CHAIN_BUDGET:?})",
        pending.first().unwrap().version,
        pending.last().unwrap().version,
    );
    assert!(
        elapsed < CHAIN_BUDGET,
        "the migration chain took {elapsed:?}, over its {CHAIN_BUDGET:?} budget"
    );

    // Every migration is recorded, and nothing beyond the known list.
    let (_, versions, _) = fingerprint(&store);
    let expected: Vec<i64> = MIGRATIONS.iter().map(|m| m.version).collect();
    assert_eq!(versions, expected);
    assert_eq!(store.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    integrity_ok(&store);

    // No pre-existing row was lost or duplicated.
    for (table, n) in &m.counts {
        assert_eq!(
            count(&store, &format!("SELECT COUNT(*) FROM {table}")),
            *n,
            "{table} row count changed"
        );
    }
    // No migration after 044 updates a session row (row_version would bump).
    assert_eq!(
        count(&store, "SELECT SUM(row_version) FROM sessions"),
        m.row_version_sum
    );

    // M0.3 (045): every session has exactly one live participant; the ones
    // that had one keep it (same id), exactly the rest got a new one, and
    // tombstones and client participants are untouched.
    let live: HashMap<i64, i64> = {
        let mut stmt = store
            .conn
            .prepare(
                "SELECT session_id, id FROM participants \
                 WHERE session_id IS NOT NULL AND retired_at IS NULL",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        rows
    };
    assert_eq!(live.len(), m.sessions.len());
    for s in &m.sessions {
        assert!(
            live.contains_key(&s.id),
            "session {} has no participant",
            s.id
        );
    }
    for (sid, pid) in &m.session_participants {
        assert_eq!(live[sid], *pid, "session {sid} lost its participant");
    }
    let minted = count(
        &store,
        "SELECT COUNT(*) FROM participants WHERE kind = 'session' AND session_id IS NOT NULL",
    ) - m.session_participants.len() as i64;
    assert_eq!(minted, m.sessions_without_participant.len() as i64);
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM participants WHERE retired_at IS NOT NULL"
        ),
        m.retired_participants as i64
    );
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM participants WHERE kind = 'client'"
        ),
        m.client_participants as i64
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM participants"),
        (m.session_participants.len()
            + m.sessions_without_participant.len()
            + m.retired_participants
            + m.client_participants) as i64
    );

    // Columns the chain adds are backfilled with nothing: no org, no touch,
    // no branch or PR signal, no nudge — and no work invented.
    for (what, sql) in [
        ("hosts.org_id", "SELECT COUNT(*) FROM hosts WHERE org_id IS NOT NULL"),
        (
            "sessions.last_touch_at",
            "SELECT COUNT(*) FROM sessions WHERE last_touch_at IS NOT NULL",
        ),
        (
            "sessions.current_branch",
            "SELECT COUNT(*) FROM sessions WHERE current_branch IS NOT NULL \
               OR current_branch_at IS NOT NULL OR pr_signals IS NOT NULL",
        ),
        (
            "conversations.classify_nudged_at",
            "SELECT COUNT(*) FROM conversations WHERE classify_nudged_at IS NOT NULL",
        ),
        (
            "participants.address",
            "SELECT COUNT(*) FROM participants WHERE address IS NOT NULL OR peer_link_id IS NOT NULL",
        ),
        (
            "session_messages.peer_*",
            "SELECT COUNT(*) FROM session_messages WHERE peer_state IS NOT NULL \
               OR remote_fleet_id IS NOT NULL OR peer_wake <> 0",
        ),
    ] {
        assert_eq!(count(&store, sql), 0, "{what} was backfilled");
    }
    for table in [
        "orgs",
        "org_rules",
        "work_items",
        "work_links",
        "work_journal",
        "trackers",
        "tracker_secrets",
        "tracker_views",
        "peer_links",
    ] {
        assert_eq!(
            count(&store, &format!("SELECT COUNT(*) FROM {table}")),
            0,
            "{table} is not empty after the upgrade"
        );
    }
    // With no org yet, every session is unassigned (M5 derivation).
    assert!(store.session_orgs().unwrap().values().all(Option::is_none));
    let rows = store.list_all_sessions().unwrap();
    assert_eq!(rows.len(), m.sessions.len());
    assert!(rows.iter().all(|r| r.org_id.is_none() && r.work.is_none()));

    // The triggers the chain installs are all there.
    for trigger in [
        "trg_participant_on_session_insert",
        "trg_work_links_end_on_retire",
        "trg_work_journal_conversation_closed",
        "trg_work_journal_session_delete",
        "trg_work_links_snap_org",
        "trg_read_cursors_on_session_delete",
    ] {
        assert_eq!(
            count(
                &store,
                &format!(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = '{trigger}'"
                )
            ),
            1,
            "missing trigger {trigger}"
        );
    }

    // A second run is a no-op: same schema, versions and row counts, and not
    // one row written.
    let before = fingerprint(&store);
    let changes = store.conn.total_changes();
    store.migrate().expect("second migrate");
    assert_eq!(fingerprint(&store), before);
    assert_eq!(
        store.conn.total_changes(),
        changes,
        "the second migrate wrote rows"
    );
    integrity_ok(&store);
}

/// The same upgrade through the real open path on a file — WAL, a commit (and
/// fsync) per migration — as a user's `state.db` goes through it.
#[test]
fn opening_a_pre_work_graph_file_upgrades_it_within_budget() {
    let (conn, m) = testgen::pre_work_graph_db(SEED, Shape::default());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    conn.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    drop(conn);
    let started = Instant::now();
    let store = Store::open_with_bus(&path, Arc::new(NoopEventBus)).expect("open and upgrade");
    let elapsed = started.elapsed();
    println!("M12.1: open_with_bus upgraded the generated state.db in {elapsed:?}");
    assert!(
        elapsed < CHAIN_BUDGET,
        "opening took {elapsed:?}, over {CHAIN_BUDGET:?}"
    );
    assert_eq!(store.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM sessions"),
        m.count("sessions")
    );
    integrity_ok(&store);
}

/// Each migration after 044 on its own, for the per-migration numbers (and so
/// a slow one is named, not just a slow chain).
#[test]
fn per_migration_times_on_the_generated_database() {
    let (conn, _) = testgen::pre_work_graph_db(SEED, Shape::default());
    let store = store_over(conn);
    store
        .conn
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .unwrap();
    let mut total = Duration::ZERO;
    for m in pending_after_pre_work_graph() {
        let started = Instant::now();
        let preexisting = store.apply_migrations(&[m]).unwrap();
        let took = started.elapsed();
        total += took;
        assert!(preexisting.is_empty());
        println!("M12.1: migration {:03} took {took:?}", m.version);
        assert!(
            took < CHAIN_BUDGET,
            "migration {} alone took {took:?}",
            m.version
        );
    }
    store
        .conn
        .execute_batch("PRAGMA foreign_keys = ON;")
        .unwrap();
    println!("M12.1: sum of migrations {total:?}");
    assert_eq!(store.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    integrity_ok(&store);
}

/// M5 on upgraded data: add orgs the way an admin would (a path rule, an
/// owner rule, hosts), and every session's org — listed and per id — is the
/// one the rules give, computed here from what the generator wrote.
#[test]
fn org_derivation_on_upgraded_data_follows_the_rules() {
    let (store, m, _) = upgraded();
    let acme = store.add_org("Acme", None, false).unwrap().id;
    let initech = store.add_org("Initech", None, false).unwrap().id;
    let globex = store.add_org("Globex", None, false).unwrap().id;
    store
        .add_org_rule(OrgRuleRow {
            id: 0,
            org_id: acme,
            owner: Some("acme-corp".into()),
            repo: None,
            path_prefix: None,
            host_alias: None,
        })
        .unwrap();
    store
        .add_org_rule(OrgRuleRow {
            id: 0,
            org_id: initech,
            owner: None,
            repo: None,
            path_prefix: Some(INITECH_ROOT.into()),
            host_alias: None,
        })
        .unwrap();
    let globex_hosts: Vec<&String> = m.hosts.iter().rev().take(5).collect();
    for h in &globex_hosts {
        store.set_host_org(h, Some(globex)).unwrap();
    }

    // path > owner > host, per `session_org_sql!`.
    let expected: BTreeMap<i64, Option<i64>> =
        m.sessions
            .iter()
            .map(|s| {
                let org = if s.path.as_deref().is_some_and(|p| {
                    p == INITECH_ROOT || p.starts_with(&format!("{INITECH_ROOT}/"))
                }) {
                    Some(initech)
                } else if s
                    .owner
                    .as_deref()
                    .is_some_and(|o| o.eq_ignore_ascii_case("acme-corp"))
                {
                    Some(acme)
                } else if globex_hosts.contains(&&s.host) {
                    Some(globex)
                } else {
                    None
                };
                (s.id, org)
            })
            .collect();
    let derived: BTreeMap<i64, Option<i64>> = store.session_orgs().unwrap().into_iter().collect();
    assert_eq!(derived, expected);
    let listed: BTreeMap<i64, Option<i64>> = store
        .list_all_sessions()
        .unwrap()
        .into_iter()
        .map(|r| (r.id, r.org_id))
        .collect();
    assert_eq!(listed, expected);

    // Every branch of the rule was exercised, and hosts.org_id is set only
    // where assigned.
    for org in [Some(acme), Some(initech), Some(globex), None] {
        let n = expected.values().filter(|o| **o == org).count();
        println!("M12.1: org {org:?}: {n} sessions");
        assert!(n > 0, "no session derived to {org:?}");
    }
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM hosts WHERE org_id IS NOT NULL"
        ),
        globex_hosts.len() as i64
    );
    integrity_ok(&store);
}

/// The upgraded database works for the work graph's own paths: deleting a
/// session journals each of its conversations (047's trigger) and retires
/// its participant, and the database stays consistent.
#[test]
fn deleting_an_upgraded_session_journals_its_conversations() {
    let (store, m, _) = upgraded();
    let (sid, convs) = m
        .sessions
        .iter()
        .map(|s| {
            let n = count(
                &store,
                &format!(
                    "SELECT COUNT(*) FROM conversations WHERE session_id = {}",
                    s.id
                ),
            );
            (s.id, n)
        })
        .max_by_key(|&(_, n)| n)
        .unwrap();
    assert!(convs > 1);
    store.delete_session(sid).unwrap();
    assert_eq!(count(&store, "SELECT COUNT(*) FROM work_journal"), convs);
    assert_eq!(
        count(
            &store,
            &format!("SELECT COUNT(*) FROM participants WHERE session_id = {sid}")
        ),
        0
    );
    integrity_ok(&store);
}
