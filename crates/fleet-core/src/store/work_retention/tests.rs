//! Retention (M12.3), row by row: every protected row survives, every
//! eligible row goes, the dry run equals the deletion, and `0` keeps all.

use super::*;
use crate::store::StartSource;
use std::collections::BTreeSet;

const DAY: i64 = 86_400;
/// When the old rows were written.
const OLD: i64 = 1_000_000;
/// The sweep's clock: 400 days later, past every default window.
const NOW: i64 = OLD + 400 * DAY;
/// A row younger than any window.
const YOUNG: i64 = NOW - DAY;

fn session(s: &Store, name: &str) -> (i64, i64) {
    s.upsert_host("h").unwrap();
    let sid = s
        .upsert_session(name, "h", None, None, 1, 1, "running", None)
        .unwrap();
    let pid = s.ensure_participant_for_session(sid).unwrap();
    (sid, pid)
}

/// A session with conversation `cid`, closed unless `open`.
fn conv(s: &Store, name: &str, cid: &str, open: bool) -> (i64, i64) {
    let (sid, pid) = session(s, name);
    s.rebind_conversation(sid, cid, StartSource::Startup, None, None)
        .unwrap();
    if !open {
        s.close_conversation(sid, cid, "exit").unwrap();
    }
    (sid, pid)
}

fn journal(
    s: &Store,
    cid: Option<&str>,
    pid: Option<i64>,
    kind: &str,
    at: i64,
    delivered_at: Option<i64>,
) -> i64 {
    s.conn
        .execute(
            "INSERT INTO work_journal (claude_session_id, participant_id, at, kind, source, body, \
                                       delivered_at) VALUES (?1, ?2, ?3, ?4, 'fleet', 'b', ?5)",
            rusqlite::params![cid, pid, at, kind, delivered_at],
        )
        .unwrap();
    s.conn.last_insert_rowid()
}

fn item(s: &Store, tracker: Option<i64>, key: &str, status: &str, at: i64) -> i64 {
    s.conn
        .execute(
            "INSERT INTO work_items (source, tracker_id, external_id, key, title, \
                                     status_category, created_at, updated_at) \
             VALUES (CASE WHEN ?1 IS NULL THEN 'local' ELSE 'jira' END, ?1, ?2, ?2, 't', ?3, ?4, ?4)",
            rusqlite::params![tracker, key, status, at],
        )
        .unwrap();
    s.conn.last_insert_rowid()
}

fn link(
    s: &Store,
    item_id: Option<i64>,
    ref_key: Option<&str>,
    pid: Option<i64>,
    state: &str,
    ended_at: Option<i64>,
    snap: &[&str],
) {
    let snap = (!snap.is_empty()).then(|| serde_json::to_string(snap).unwrap());
    s.conn
        .execute(
            "INSERT INTO work_links (item_id, ref_key, participant_id, state, source, created_at, \
                                     ended_at, snap_claude_ids) \
             VALUES (?1, ?2, ?3, ?4, 'manual', ?5, ?6, ?7)",
            rusqlite::params![item_id, ref_key, pid, state, OLD, ended_at, snap],
        )
        .unwrap();
}

fn event(s: &Store, sid: i64, kind: &str, at: i64) -> i64 {
    s.conn
        .execute(
            "INSERT INTO session_events (session_id, at, kind) VALUES (?1, ?2, ?3)",
            rusqlite::params![sid, at, kind],
        )
        .unwrap();
    s.conn.last_insert_rowid()
}

fn ids(s: &Store, table: &str) -> BTreeSet<i64> {
    let mut stmt = s.conn.prepare(&format!("SELECT id FROM {table}")).unwrap();
    let rows = stmt.query_map([], |r| r.get(0)).unwrap();
    rows.collect::<rusqlite::Result<_>>().unwrap()
}

/// Sweep `t` to zero and check that exactly `gone` went, and that the dry
/// run said so beforehand.
fn sweep_exactly(s: &Store, t: RetentionTable, days: i64, gone: &[i64]) {
    let before = ids(s, t.table());
    let dry = s.retention_eligible(t, NOW, days).unwrap();
    let mut deleted = 0;
    loop {
        let n = s.retention_delete_batch(t, NOW, days, 2).unwrap();
        deleted += n;
        if n == 0 {
            break;
        }
    }
    let after = ids(s, t.table());
    let went: BTreeSet<i64> = before.difference(&after).copied().collect();
    assert_eq!(
        went,
        gone.iter().copied().collect(),
        "{}: wrong rows swept",
        t.table()
    );
    assert_eq!(
        dry,
        deleted as i64,
        "{}: the dry run must equal the sweep",
        t.table()
    );
    assert_eq!(s.retention_eligible(t, NOW, days).unwrap(), 0);
}

#[test]
fn journal_keeps_every_row_something_live_points_at() {
    let s = Store::open_in_memory().unwrap();
    let done = item(&s, Some(1), "ABC-1", "done", OLD);
    let open = item(&s, Some(1), "ABC-2", "in_progress", OLD);
    let done_live = item(&s, Some(1), "ABC-3", "done", OLD);
    let done_by_key = item(&s, Some(1), "ABC-4", "done", OLD);
    let done_of_live = item(&s, Some(1), "ABC-5", "done", OLD);

    // Swept: an old row of a closed, unlinked conversation.
    let (_, _) = conv(&s, "loose", "c-loose", false);
    let loose = journal(&s, Some("c-loose"), None, "progress", OLD, None);
    // Kept: younger than the window.
    let young = journal(&s, Some("c-loose"), None, "progress", YOUNG, None);
    // Kept: the conversation is still open.
    conv(&s, "open", "c-open", true);
    let in_open = journal(&s, Some("c-open"), None, "progress", OLD, None);
    // Kept: the session has a live link — even to a done item.
    let (_, p_live) = conv(&s, "live", "c-live", false);
    link(
        &s,
        Some(done_of_live),
        None,
        Some(p_live),
        "confirmed",
        None,
        &[],
    );
    let of_live = journal(&s, Some("c-live"), None, "outcome", OLD, None);
    // Kept: a live suggestion is a live link too.
    let (_, p_sug) = conv(&s, "sug", "c-sug", false);
    link(&s, Some(open), None, Some(p_sug), "suggested", None, &[]);
    let of_sug = journal(&s, Some("c-sug"), None, "progress", OLD, None);
    // Swept: a live REJECTION is not work.
    let (_, p_rej) = conv(&s, "rej", "c-rej", false);
    link(&s, Some(open), None, Some(p_rej), "rejected", None, &[]);
    let of_rej = journal(&s, Some("c-rej"), None, "progress", OLD, None);
    // Ended confirmed links, their sessions gone:
    // kept while the item is open,
    link(
        &s,
        Some(open),
        None,
        None,
        "confirmed",
        Some(OLD),
        &["c-open-item"],
    );
    let open_item = journal(&s, Some("c-open-item"), None, "progress", OLD, None);
    // swept once it is done and nothing live names it,
    link(
        &s,
        Some(done),
        None,
        None,
        "confirmed",
        Some(OLD),
        &["c-done"],
    );
    let done_item = journal(&s, Some("c-done"), None, "progress", OLD, None);
    let done_note = journal(&s, Some("c-done"), None, "note", OLD, None);
    // kept when a live link names the done item (its latest agent note too),
    link(
        &s,
        Some(done_live),
        None,
        None,
        "confirmed",
        Some(OLD),
        &["c-dl"],
    );
    link(
        &s,
        Some(done_live),
        None,
        Some(p_sug),
        "confirmed",
        None,
        &[],
    );
    let done_live_note = journal(&s, Some("c-dl"), None, "note", OLD, None);
    // or names its key through a bare ref,
    link(
        &s,
        Some(done_by_key),
        None,
        None,
        "confirmed",
        Some(OLD),
        &["c-dk"],
    );
    link(&s, None, Some("ABC-4"), Some(p_sug), "confirmed", None, &[]);
    let done_key = journal(&s, Some("c-dk"), None, "progress", OLD, None);
    // and kept for a bare ref (no item, no status to call done).
    link(
        &s,
        None,
        Some("XYZ-9"),
        None,
        "confirmed",
        Some(OLD),
        &["c-bare"],
    );
    let bare = journal(&s, Some("c-bare"), None, "progress", OLD, None);
    // Swept: an ended SUGGESTION of open work never grouped anything.
    link(
        &s,
        Some(open),
        None,
        None,
        "suggested",
        Some(OLD),
        &["c-es"],
    );
    let ended_sug = journal(&s, Some("c-es"), None, "progress", OLD, None);
    // Handovers: kept undelivered, kept for a live participant, swept
    // delivered to a retired one.
    let (_, p_gone) = session(&s, "gone");
    s.conn
        .execute(
            "UPDATE participants SET retired_at = ?1 WHERE id = ?2",
            rusqlite::params![OLD, p_gone],
        )
        .unwrap();
    let pending = journal(&s, None, Some(p_gone), "handover", OLD, None);
    let to_live = journal(&s, None, Some(p_live), "handover", OLD, Some(OLD));
    let delivered = journal(&s, None, Some(p_gone), "handover", OLD, Some(OLD));

    let kept = [
        young,
        in_open,
        of_live,
        of_sug,
        open_item,
        done_live_note,
        done_key,
        bare,
        pending,
        to_live,
    ];
    sweep_exactly(
        &s,
        RetentionTable::Journal,
        365,
        &[loose, of_rej, done_item, done_note, ended_sug, delivered],
    );
    let left = ids(&s, "work_journal");
    for k in kept {
        assert!(left.contains(&k), "journal row {k} must survive");
    }
}

#[test]
fn tracker_items_go_only_when_done_old_unlinked_and_no_kept_descendant() {
    let s = Store::open_in_memory().unwrap();
    let t = Some(1);
    let plain = item(&s, t, "A-1", "done", OLD);
    let young = item(&s, t, "A-2", "done", YOUNG);
    let todo = item(&s, t, "A-3", "todo", OLD);
    let ended = item(&s, t, "A-4", "done", OLD);
    link(&s, Some(ended), None, None, "confirmed", Some(OLD), &[]);
    let rejected = item(&s, t, "A-5", "done", OLD);
    link(&s, Some(rejected), None, None, "rejected", None, &[]);
    let by_ref = item(&s, t, "A-6", "done", OLD);
    link(&s, None, Some("A-6"), None, "confirmed", Some(OLD), &[]);
    let by_alias = item(&s, t, "A-7", "done", OLD);
    s.conn
        .execute(
            "UPDATE work_items SET aliases = '[\"OLD-7\"]' WHERE id = ?1",
            [by_alias],
        )
        .unwrap();
    link(&s, None, Some("OLD-7"), None, "confirmed", Some(OLD), &[]);
    // A parent of open work stays; a parent whose subtree is all
    // eligible goes with it.
    let parent_of_open = item(&s, t, "A-8", "done", OLD);
    let open_child = item(&s, t, "A-9", "todo", OLD);
    let parent_of_done = item(&s, t, "A-10", "done", OLD);
    let done_child = item(&s, t, "A-11", "done", OLD);
    let grand = item(&s, t, "A-12", "done", OLD);
    let mid = item(&s, t, "A-13", "done", OLD);
    let leaf = item(&s, t, "A-14", "todo", OLD);
    for (child, parent) in [
        (open_child, parent_of_open),
        (done_child, parent_of_done),
        (mid, grand),
        (leaf, mid),
    ] {
        s.conn
            .execute(
                "UPDATE work_items SET parent_id = ?2 WHERE id = ?1",
                [child, parent],
            )
            .unwrap();
    }
    let local = item(&s, None, "LOCAL-1", "done", OLD);
    // Seen by a sync lately, or reopened lately: not old.
    let fetched = item(&s, t, "A-15", "done", OLD);
    let reopened = item(&s, t, "A-16", "done", OLD);
    s.conn
        .execute(
            "UPDATE work_items SET fetched_at = ?2 WHERE id = ?1",
            [fetched, YOUNG],
        )
        .unwrap();
    s.conn
        .execute(
            "UPDATE work_items SET reopened_at = ?2 WHERE id = ?1",
            [reopened, YOUNG],
        )
        .unwrap();

    sweep_exactly(
        &s,
        RetentionTable::TrackerItems,
        180,
        &[plain, parent_of_done, done_child],
    );
    let left = ids(&s, "work_items");
    for k in [
        young,
        todo,
        ended,
        rejected,
        by_ref,
        by_alias,
        parent_of_open,
        open_child,
        grand,
        mid,
        leaf,
        local,
        fetched,
        reopened,
    ] {
        assert!(left.contains(&k), "item {k} must survive");
    }
    // Every link survived (a deleted item would cascade its links).
    let links: i64 = s
        .conn
        .query_row("SELECT COUNT(*) FROM work_links", [], |r| r.get(0))
        .unwrap();
    assert_eq!(links, 4);
}

#[test]
fn timeline_sweeps_superseded_work_events_only() {
    let s = Store::open_in_memory().unwrap();
    let (a, _) = session(&s, "a");
    let (b, _) = session(&s, "b");
    let req_old = event(&s, a, "handover_requested", OLD);
    let req_new = event(&s, a, "handover_requested", OLD + 10);
    let tidied = event(&s, a, "gc_tidied", OLD);
    let nudge_old = event(&s, a, "work_classify_nudge", OLD);
    let nudge_young = event(&s, a, "work_classify_nudge", YOUNG);
    let other = event(&s, a, "status_change", OLD);
    let other2 = event(&s, a, "status_change", OLD + 1);
    // Another session's newest is its own.
    let b_written = event(&s, b, "handover_written", OLD);
    let b_written2 = event(&s, b, "handover_written", OLD + 5);
    sweep_exactly(
        &s,
        RetentionTable::WorkEvents,
        180,
        &[req_old, nudge_old, b_written],
    );
    let left = ids(&s, "session_events");
    for k in [req_new, tidied, nudge_young, other, other2, b_written2] {
        assert!(left.contains(&k), "event {k} must survive");
    }
}

#[test]
fn the_newest_by_id_survives_a_clock_step_too() {
    // tidy_sessions reads a keep by MAX(id); a clock stepped back wrote the
    // live keep with an older `at` than the one it replaced.
    let s = Store::open_in_memory().unwrap();
    let (a, _) = session(&s, "a");
    let older = event(&s, a, "tidy_kept", OLD - 100);
    let first = event(&s, a, "tidy_kept", OLD + 100);
    let stepped = event(&s, a, "tidy_kept", OLD);
    sweep_exactly(&s, RetentionTable::WorkEvents, 180, &[older]);
    let left = ids(&s, "session_events");
    assert!(left.contains(&first), "newest by time");
    assert!(left.contains(&stepped), "newest by id: the keep in force");
}

#[test]
fn zero_days_keeps_every_table_forever() {
    let s = Store::open_in_memory().unwrap();
    let (sid, _) = session(&s, "a");
    journal(&s, Some("gone"), None, "progress", OLD, None);
    item(&s, Some(1), "A-1", "done", OLD);
    event(&s, sid, "gc_tidied", OLD);
    event(&s, sid, "gc_tidied", OLD + 1);
    for t in RetentionTable::ALL {
        assert!(s.retention_eligible(t, NOW, 1).unwrap() > 0, "{t:?}");
        assert_eq!(s.retention_eligible(t, NOW, 0).unwrap(), 0, "{t:?}");
        assert_eq!(
            s.retention_delete_batch(t, NOW, 0, 100).unwrap(),
            0,
            "{t:?}"
        );
        assert_eq!(
            s.retention_rows(t).unwrap(),
            if t == RetentionTable::WorkEvents {
                2
            } else {
                1
            }
        );
    }
}

#[test]
fn a_batch_deletes_at_most_its_limit() {
    let s = Store::open_in_memory().unwrap();
    for i in 0..7 {
        journal(&s, Some(&format!("c{i}")), None, "progress", OLD, None);
    }
    let t = RetentionTable::Journal;
    assert_eq!(s.retention_eligible(t, NOW, 1).unwrap(), 7);
    assert_eq!(s.retention_delete_batch(t, NOW, 1, 3).unwrap(), 3);
    assert_eq!(s.retention_delete_batch(t, NOW, 1, 3).unwrap(), 3);
    assert_eq!(s.retention_delete_batch(t, NOW, 1, 3).unwrap(), 1);
    assert_eq!(s.retention_delete_batch(t, NOW, 1, 3).unwrap(), 0);
}
