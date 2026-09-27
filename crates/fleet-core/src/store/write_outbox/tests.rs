//! The write outbox (M13.4e): who may cause a write, where it may go, the
//! idempotency of the queue, and its retention.

use super::*;
use crate::store::{RetentionTable, TrackerItemWrite, WorkTarget};

const DAY: i64 = 86_400;
const PR: &str = "https://github.com/acme/api/pull/12";

struct Fx {
    s: Store,
    tracker: i64,
    item: i64,
}

fn fx() -> Fx {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let t = s
        .add_tracker("jira", "Acme", "https://acme.atlassian.net")
        .unwrap();
    let item = s
        .upsert_tracker_item(
            t.id,
            &TrackerItemWrite {
                external_id: "10001".into(),
                key: Some("ABC-1".into()),
                title: "t".into(),
                status_name: "To Do".into(),
                status_category: "todo".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
    Fx {
        s,
        tracker: t.id,
        item,
    }
}

/// A live session named `name` on `h`, linked to the item by `source`,
/// with `pr` as its PR. Returns (session id, link id).
fn linked(f: &Fx, name: &str, source: &str, pr: Option<&str>) -> (i64, i64) {
    let sid =
        f.s.upsert_session(name, "h", None, None, 1, 1, "running", None)
            .unwrap();
    f.s.conn
        .execute(
            "UPDATE sessions SET pr_url = ?2 WHERE id = ?1",
            rusqlite::params![sid, pr],
        )
        .unwrap();
    let link =
        f.s.link_session_work(sid, WorkTarget::Item(f.item), "manual")
            .unwrap();
    f.s.conn
        .execute(
            "UPDATE work_links SET source = ?2 WHERE id = ?1",
            rusqlite::params![link.id, source],
        )
        .unwrap();
    (sid, link.id)
}

fn candidates(f: &Fx, org: Option<i64>, now: i64) -> Vec<RemoteLinkCandidate> {
    f.s.pr_remote_link_candidates(f.tracker, org, now).unwrap()
}

#[test]
fn only_manual_and_started_links_with_a_pr_are_candidates() {
    let f = fx();
    let (_, manual) = linked(&f, "a", "manual", Some(PR));
    let (_, started) = linked(
        &f,
        "b",
        "started",
        Some("https://github.com/acme/api/pull/13"),
    );
    // A guess or a derived link never writes.
    for (i, source) in ["agent", "resumed", "forked", "inherited", "branch", "pr"]
        .iter()
        .enumerate()
    {
        linked(
            &f,
            &format!("x{i}"),
            source,
            Some("https://github.com/acme/api/pull/99"),
        );
    }
    // No PR, or not a PR fleet will hand over.
    linked(&f, "c", "manual", None);
    linked(&f, "d", "manual", Some("http://github.com/acme/api/pull/1"));
    linked(&f, "e", "manual", Some("https://evil.example/a b"));
    let got: Vec<i64> = candidates(&f, None, 1_000)
        .iter()
        .map(|c| c.link_id)
        .collect();
    assert_eq!(got, vec![manual, started]);
    let c = &candidates(&f, None, 1_000)[0];
    assert_eq!(
        (
            c.issue_id.as_str(),
            c.issue_key.as_deref(),
            c.pr_url.as_str()
        ),
        ("10001", Some("ABC-1"), PR)
    );
}

#[test]
fn a_suggestion_or_a_rejection_never_writes() {
    let f = fx();
    let (_, l) = linked(&f, "a", "manual", Some(PR));
    for state in ["suggested", "rejected"] {
        f.s.conn
            .execute(
                "UPDATE work_links SET state = ?2 WHERE id = ?1",
                rusqlite::params![l, state],
            )
            .unwrap();
        assert!(candidates(&f, None, 1_000).is_empty(), "{state}");
    }
}

#[test]
fn a_link_a_per_host_token_decided_never_writes_until_a_person_decides_again() {
    let f = fx();
    let (sid, l) = linked(&f, "a", "manual", Some(PR));
    assert!(f.s.mark_link_host_decided(l).unwrap());
    assert!(candidates(&f, None, 1_000).is_empty());
    // A person links it again: the flag clears.
    f.s.link_session_work(sid, WorkTarget::Item(f.item), "manual")
        .unwrap();
    assert_eq!(candidates(&f, None, 1_000).len(), 1);
}

#[test]
fn only_the_trackers_own_org_is_written_to() {
    let f = fx();
    let a = f.s.add_org("A", None, false).unwrap().id;
    let b = f.s.add_org("B", None, false).unwrap().id;
    linked(&f, "a", "manual", Some(PR));
    // Unassigned session, unassigned tracker: the same (no) org.
    assert_eq!(candidates(&f, None, 1_000).len(), 1);
    // The tracker in A, the session unassigned: refused.
    assert!(candidates(&f, Some(a), 1_000).is_empty());
    // The session's host in B: never to A's tracker, forced or not.
    f.s.set_host_org("h", Some(b)).unwrap();
    assert!(candidates(&f, Some(a), 1_000).is_empty());
    assert!(candidates(&f, None, 1_000).is_empty());
    assert_eq!(candidates(&f, Some(b), 1_000).len(), 1);
}

#[test]
fn an_ended_link_writes_within_the_grace_with_the_org_it_ended_in() {
    let f = fx();
    let b = f.s.add_org("B", None, false).unwrap().id;
    let (_, l) = linked(&f, "a", "manual", Some(PR));
    let now = 10 * DAY;
    f.s.conn
        .execute(
            "UPDATE work_links SET ended_at = ?2, snap_pr_url = ?3, snap_org_id = ?4, \
               snap_claude_ids = '[\"c1\",\"c2\"]' WHERE id = ?1",
            rusqlite::params![l, now - 3600, PR, b],
        )
        .unwrap();
    let got = candidates(&f, Some(b), now);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].claude_session_id.as_deref(), Some("c2"));
    assert!(candidates(&f, None, now).is_empty(), "another org");
    assert!(
        candidates(&f, Some(b), now + ENDED_GRACE_SECS).is_empty(),
        "past the grace"
    );
}

#[test]
fn the_same_global_id_twice_is_one_row() {
    let f = fx();
    linked(&f, "a", "manual", Some(PR));
    let c = candidates(&f, None, 1_000).remove(0);
    assert!(f.s.enqueue_pr_remote_link(f.tracker, &c, 1_000).unwrap());
    assert!(!f.s.enqueue_pr_remote_link(f.tracker, &c, 2_000).unwrap());
    // A second session on the same ticket with the same PR: still one.
    linked(&f, "b", "started", Some(PR));
    for c in candidates(&f, None, 3_000) {
        f.s.enqueue_pr_remote_link(f.tracker, &c, 3_000).unwrap();
    }
    let rows = f.s.outbox_rows(f.tracker).unwrap();
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.global_id, format!("fleet:pr:{PR}"));
    assert_eq!(r.title, "Pull request acme/api#12");
    assert_eq!((r.state.as_str(), r.attempts), ("pending", 0));
}

#[test]
fn the_title_is_fleets_own_words() {
    assert_eq!(
        pr_title("https://github.com/o/r/pull/7?x=1#y"),
        "Pull request o/r#7"
    );
    assert_eq!(
        pr_title("https://gitlab.example/g/p/merge_requests/3"),
        "Pull request g/p#3"
    );
    assert_eq!(pr_title("https://x.example/<b>hi</b>"), "Pull request");
}

#[test]
fn due_rows_retry_and_settle() {
    let f = fx();
    linked(&f, "a", "manual", Some(PR));
    let c = candidates(&f, None, 1_000).remove(0);
    f.s.enqueue_pr_remote_link(f.tracker, &c, 1_000).unwrap();
    let id = f.s.due_outbox_writes(f.tracker, 1_000, 10).unwrap()[0].id;
    assert!(f.s.retry_outbox_write(id, 1_100, "HTTP 500", true).unwrap());
    assert!(f
        .s
        .due_outbox_writes(f.tracker, 1_099, 10)
        .unwrap()
        .is_empty());
    assert_eq!(
        f.s.due_outbox_writes(f.tracker, 1_100, 10).unwrap().len(),
        1
    );
    let counts = f.s.outbox_counts(f.tracker).unwrap();
    assert_eq!(
        (counts.pending, counts.last_error.as_deref()),
        (1, Some("HTTP 500"))
    );
    assert!(f.s.settle_outbox_write(id, "done", None, true).unwrap());
    // Settled once: a second settle is a no-op, and nothing is due.
    assert!(!f
        .s
        .settle_outbox_write(id, "failed", Some("x"), true)
        .unwrap());
    assert!(f
        .s
        .due_outbox_writes(f.tracker, 9_999, 10)
        .unwrap()
        .is_empty());
    let r = &f.s.outbox_rows(f.tracker).unwrap()[0];
    assert_eq!((r.state.as_str(), r.attempts), ("done", 2));
    assert_eq!(f.s.outbox_counts(f.tracker).unwrap().done, 1);
}

#[test]
fn removing_the_tracker_takes_its_rows() {
    let f = fx();
    linked(&f, "a", "manual", Some(PR));
    let c = candidates(&f, None, 1_000).remove(0);
    f.s.enqueue_pr_remote_link(f.tracker, &c, 1_000).unwrap();
    assert!(f.s.remove_tracker(f.tracker).unwrap());
    assert_eq!(f.s.retention_rows(RetentionTable::WriteOutbox).unwrap(), 0);
}

/// M12.3's rule for the outbox: only settled rows, only once their link is
/// gone or ended before the window; `0` keeps them forever.
#[test]
fn retention_sweeps_only_settled_rows_whose_link_is_gone_or_long_ended() {
    let f = fx();
    let t = RetentionTable::WriteOutbox;
    let now = 400 * DAY;
    let old = DAY;
    let row = |name: &str, pr: &str, state: &str| -> (i64, i64) {
        let (_, l) = linked(&f, name, "manual", Some(pr));
        let c = candidates(&f, None, old)
            .into_iter()
            .find(|c| c.link_id == l)
            .unwrap();
        f.s.enqueue_pr_remote_link(f.tracker, &c, old).unwrap();
        let id =
            f.s.outbox_rows(f.tracker)
                .unwrap()
                .into_iter()
                .find(|r| r.link_id == Some(l))
                .unwrap()
                .id;
        f.s.conn
            .execute(
                "UPDATE tracker_write_outbox SET state = ?2, updated_at = ?3 WHERE id = ?1",
                rusqlite::params![id, state, old],
            )
            .unwrap();
        (id, l)
    };
    let pr = |n: u32| format!("https://github.com/acme/api/pull/{n}");
    // Kept: pending, whatever its age and link.
    let (pending, pending_link) = row("p", &pr(1), "pending");
    // Kept: settled, but its link is live.
    let (live, _) = row("l", &pr(2), "done");
    // Kept: settled, its link ended inside the window.
    let (recent, recent_link) = row("r", &pr(3), "done");
    // Swept: settled, link ended long ago / unlinked / never had one.
    let (ended, ended_link) = row("e", &pr(4), "failed");
    let (gone, gone_link) = row("g", &pr(5), "cancelled");
    let end = |l: i64, at: i64| {
        f.s.conn
            .execute(
                "UPDATE work_links SET ended_at = ?2 WHERE id = ?1",
                rusqlite::params![l, at],
            )
            .unwrap();
    };
    end(pending_link, old);
    end(recent_link, now - DAY);
    end(ended_link, old);
    f.s.conn
        .execute("DELETE FROM work_links WHERE id = ?1", [gone_link])
        .unwrap();
    assert_eq!(f.s.retention_rows(t).unwrap(), 5);
    assert_eq!(f.s.retention_eligible(t, now, 0).unwrap(), 0, "0 = forever");
    assert_eq!(f.s.retention_delete_batch(t, now, 0, 100).unwrap(), 0);
    assert_eq!(f.s.retention_eligible(t, now, 90).unwrap(), 2);
    assert_eq!(
        f.s.retention_delete_batch(t, now, 90, 1).unwrap(),
        1,
        "the cap"
    );
    assert_eq!(f.s.retention_delete_batch(t, now, 90, 100).unwrap(), 1);
    let left: Vec<i64> =
        f.s.outbox_rows(f.tracker)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
    assert_eq!(left, vec![pending, live, recent]);
    assert!(!left.contains(&ended) && !left.contains(&gone));
}
