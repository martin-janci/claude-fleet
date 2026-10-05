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

/// The same chain through `open_with_bus` on a real file. On Linux that adds
/// ~35 ms to the in-memory chain (174 ms against 140 ms, 4 vCPU, debug), so
/// it keeps `CHAIN_BUDGET`. On the Windows runner the identical open took
/// 5.39 s and failed the 5 s budget (the v0.4.6 release run, 2026-10-04): ~30x
/// Linux, with the migrations themselves unchanged. That is the runner's file
/// I/O on a fresh temp file (NTFS, real-time scanning), not migration code.
/// The two in-memory tests still hold the migrations to `CHAIN_BUDGET` on
/// every platform, so the file test only needs to catch a stall there.
const FILE_OPEN_BUDGET: Duration = if cfg!(windows) {
    Duration::from_secs(30)
} else {
    CHAIN_BUDGET
};

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

/// `(schema objects as (type, name, sql), recorded versions, row count per table)`.
type Fingerprint = (Vec<(String, String, Option<String>)>, Vec<i64>, Vec<i64>);

/// Everything a re-run could change: the schema objects, the recorded
/// versions, every table's row count.
fn fingerprint(store: &Store) -> Fingerprint {
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
    //
    // Multi-user M1 changed what this line means, so the number is quoted
    // deliberately rather than merely re-asserted. 097's attribution of
    // pre-M1 rows is `Store::backfill_session_owner`, which runs *after* the
    // chain and after 095 re-issued the row-version trigger to watch
    // `owner_person_id` / `visibility` — so an attributed row's
    // `row_version` DOES move, by exactly one, and should: an open client
    // holding a cached row has to learn that its visibility changed.
    //
    // On THIS database the backfill attributes nothing, and the sum is
    // therefore unchanged: the backfill's discriminator is
    // `started_at IS NOT NULL` (the column documented "when fleet created
    // the session"), and `store::testgen` does not write `started_at` at all
    // — it is not in its INSERT's column list. Every generated session is a
    // row fleet did not create, which is exactly the population spec §4.3
    // leaves `unclaimed`. The backfill's own arithmetic is pinned, with a
    // `started_at` population to attribute, by
    // `the_m1_backfill_attributes_the_rows_fleet_started_and_only_those`.
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM sessions WHERE started_at IS NOT NULL"
        ),
        0,
        "the generator writes no started_at; if it starts to, the sum below moves \
         by one per such row and this test must say so"
    );
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
        (
            "sessions.owner_person_id",
            "SELECT COUNT(*) FROM sessions WHERE owner_person_id IS NOT NULL \
               OR visibility <> 'unclaimed'",
        ),
    ] {
        assert_eq!(count(&store, sql), 0, "{what} was backfilled");
    }

    // Multi-user M1 (094 + 095): the hub comes out of the upgrade with
    // exactly one person — its personal owner — and every session fleet
    // itself started belongs to her and is private. The second half is
    // vacuously true here (see the `started_at` note above) and is pinned
    // where it is not, in
    // `the_m1_backfill_attributes_the_rows_fleet_started_and_only_those`;
    // what this says about THIS database is the half that matters for rule 7
    // — the upgrade widened nothing, because `unclaimed` is a per-host count
    // and nothing else.
    //
    // Deliberately NOT an assertion that no row carries `'org'`: 087's
    // `CHECK` makes that value unrepresentable, which is stronger than any
    // `SELECT COUNT(*)` (spec §4.3, Q10).
    let people = store.list_people().unwrap();
    assert_eq!(people.len(), 1, "096 mints exactly one person");
    let owner = store.personal_owner_id().unwrap().expect("the owner");
    assert_eq!(people[0].id, owner);
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM sessions WHERE started_at IS NOT NULL \
               AND NOT (owner_person_id IS NOT NULL AND visibility = 'private')"
        ),
        0,
        "a row fleet started must come out of the upgrade owned and private"
    );
    assert_eq!(count(&store, "SELECT COUNT(*) FROM conversation_owners"), 0);
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
        elapsed < FILE_OPEN_BUDGET,
        "opening took {elapsed:?}, over {FILE_OPEN_BUDGET:?}"
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

/// Multi-user M1, 097's backfill, on the realistic upgrade path and with a
/// population to attribute.
///
/// `store::testgen` deliberately rebuilds v0.2.37's schema and writes no
/// `started_at`, so the chain test above exercises the backfill's *empty*
/// case. This one stamps `started_at` on a share of the generated rows the
/// way fleet's own create path does, then upgrades, and pins all four things
/// the backfill promises:
///
/// * a row fleet started becomes the hub's one person's, and `private`;
/// * a row reconcile found keeps NULL and `unclaimed` — the upgrade widens
///   nothing (rule 7), and nobody is guessed for it;
/// * each attributed row's `row_version` moves by exactly one, because 095
///   re-issued the trigger to watch both columns and a client with a cached
///   row must learn its visibility changed;
/// * the durable conversation-owner record is written for every attributed
///   row that has a `claude_session_id`, and for no other.
#[test]
fn the_m1_backfill_attributes_the_rows_fleet_started_and_only_those() {
    let (conn, m) = testgen::pre_work_graph_db(SEED, Shape::default());
    let store = store_over(conn);
    assert_eq!(store.schema_version().unwrap(), PRE_WORK_GRAPH_VERSION);
    // Every third row, so both populations are large and interleaved.
    let fleet_started: Vec<i64> = m
        .sessions
        .iter()
        .map(|s| s.id)
        .filter(|id| id % 3 == 0)
        .collect();
    assert!(fleet_started.len() > 50, "{}", fleet_started.len());
    for id in &fleet_started {
        store
            .conn
            .execute(
                "UPDATE sessions SET started_at = 10 WHERE id = ?1",
                rusqlite::params![*id],
            )
            .unwrap();
    }
    // The baseline is taken AFTER those updates: at v044 the row-version
    // trigger (042) bumps on any UPDATE at all, so the stamping itself moves
    // the counter and the number the backfill adds is the only one this test
    // is about.
    let before = count(&store, "SELECT SUM(row_version) FROM sessions");
    let with_conversation = count(
        &store,
        "SELECT COUNT(DISTINCT claude_session_id) FROM sessions \
         WHERE started_at IS NOT NULL AND claude_session_id IS NOT NULL",
    );
    assert!(with_conversation > 0);

    store.migrate().expect("upgrade");
    let owner = store.personal_owner_id().unwrap().expect("096 mints one");

    let attributed = count(
        &store,
        "SELECT COUNT(*) FROM sessions WHERE started_at IS NOT NULL \
           AND visibility = 'private'",
    );
    assert_eq!(attributed, fleet_started.len() as i64);
    assert_eq!(
        count(
            &store,
            &format!(
                "SELECT COUNT(*) FROM sessions WHERE started_at IS NOT NULL \
                   AND owner_person_id IS NOT {owner}"
            )
        ),
        0,
        "every attributed row is the hub's one person's"
    );
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM sessions WHERE started_at IS NULL \
               AND (owner_person_id IS NOT NULL OR visibility <> 'unclaimed')"
        ),
        0,
        "a row reconcile found stays unclaimed: nobody is guessed for it"
    );
    assert_eq!(
        count(&store, "SELECT SUM(row_version) FROM sessions"),
        before + attributed,
        "one bump per attributed row, and not one more"
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM conversation_owners"),
        with_conversation,
        "the trigger recorded every attributed conversation and no other"
    );
    assert_eq!(
        count(
            &store,
            &format!(
                "SELECT COUNT(*) FROM conversation_owners \
                 WHERE owner_person_id IS NOT {owner}"
            )
        ),
        0
    );

    // A second open writes nothing: the backfill matches no row once the
    // columns are set, so it cannot re-attribute a row somebody has since
    // decided — and it cannot record a second owner for a conversation.
    let changes = store.conn.total_changes();
    store.migrate().expect("second migrate");
    assert_eq!(store.conn.total_changes(), changes);
    integrity_ok(&store);
}

/// The downgrade guard: an older build (this one, facing a database a newer
/// build recorded a higher version in) refuses to open it, says why and what
/// to do, and writes nothing.
#[test]
fn an_older_build_refuses_a_newer_database() {
    let newer = LATEST_SCHEMA_VERSION + 1;
    let store = store_over(Connection::open_in_memory().unwrap());
    store.migrate().unwrap();
    store
        .conn
        .execute(
            "INSERT INTO schema_version (version) VALUES (?1)",
            rusqlite::params![newer],
        )
        .unwrap();
    let changes = store.conn.total_changes();
    let err = store.migrate().unwrap_err().to_string();
    assert!(
        err.contains(&format!("schema version {newer}"))
            && err.contains(&format!("only knows up to {LATEST_SCHEMA_VERSION}"))
            && err.contains("newer release")
            && err.contains("do not delete it"),
        "unclear refusal: {err}"
    );
    assert_eq!(store.conn.total_changes(), changes, "the refusal wrote");

    // Through the real open path, on a file.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    {
        let s = Store::open_with_bus(&path, Arc::new(NoopEventBus)).unwrap();
        s.conn
            .execute(
                "INSERT INTO schema_version (version) VALUES (?1)",
                rusqlite::params![newer],
            )
            .unwrap();
    }
    let err = match Store::open_with_bus(&path, Arc::new(NoopEventBus)) {
        Ok(_) => panic!("a newer database opened"),
        Err(e) => e.to_string(),
    };
    assert!(err.contains("newer release"), "{err}");
    // The read-only open (a CLI beside a newer daemon) never migrates, so it
    // is not refused.
    let ro = Store::open_read_only(&path).unwrap();
    assert_eq!(ro.schema_version().unwrap(), newer);
}

/// The guard lets through what it must: a fresh file, a database at exactly
/// the known version, and one at an older version (which it then upgrades).
#[test]
fn the_downgrade_guard_admits_fresh_current_and_older_databases() {
    let fresh = store_over(Connection::open_in_memory().unwrap());
    fresh.migrate().unwrap();
    fresh.migrate().unwrap();
    assert_eq!(fresh.schema_version().unwrap(), LATEST_SCHEMA_VERSION);

    let (conn, _) = testgen::pre_work_graph_db(
        SEED,
        Shape {
            hosts: 2,
            projects: 2,
            worktrees: 2,
            sessions: 5,
            conversations: 10,
            events: 10,
        },
    );
    let older = store_over(conn);
    older.migrate().unwrap();
    assert_eq!(older.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
}
