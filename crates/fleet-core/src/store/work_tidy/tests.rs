//! Migration 050's writers and readers: archive / un-archive on a touch,
//! snooze and never, what the tidy planner reads, and reopened work.

use super::*;
use crate::store::{TrackerItemWrite, WorkTarget};

fn seed(s: &Store, name: &str) -> i64 {
    s.upsert_host("h").unwrap();
    s.upsert_session(name, "h", None, None, 1, 1, "running", None)
        .unwrap()
}

fn archived(s: &Store, sid: i64) -> Option<i64> {
    s.get_session_by_id(sid)
        .unwrap()
        .unwrap()
        .work
        .and_then(|w| w.archived_at)
}

#[test]
fn archive_is_ui_only_and_the_next_prompt_or_attach_unarchives() {
    let s = Store::open_in_memory().unwrap();
    let sid = seed(&s, "dev");
    // Nothing to collapse an unlinked session into.
    assert_eq!(
        s.archive_session_work(sid).unwrap_err().code,
        codes::E_INVALID
    );
    s.link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
        .unwrap();
    assert_eq!(s.archive_session_work(sid).unwrap(), 1);
    let first = archived(&s, sid).expect("archived");
    // Idempotent: the first stamp stays.
    assert_eq!(s.archive_session_work(sid).unwrap(), 0);
    assert_eq!(archived(&s, sid), Some(first));
    let row = s.get_session_by_id(sid).unwrap().unwrap();
    assert_eq!(row.status, "running", "tmux keeps running: the row is live");

    // A prompt (UserPromptSubmit) un-archives and stamps the touch.
    s.conn
        .execute(
            "UPDATE sessions SET claude_session_id = 'c1' WHERE id = ?1",
            [sid],
        )
        .unwrap();
    s.record_prompt_submit_hook_for_row(sid).unwrap();
    assert_eq!(archived(&s, sid), None);
    let touched = s
        .tidy_sessions()
        .unwrap()
        .into_iter()
        .find(|t| t.row.id == sid)
        .unwrap();
    assert!(touched.last_touch_at.is_some());

    // An attach (touch) un-archives too.
    s.archive_session_work(sid).unwrap();
    assert!(s.touch_session(sid).unwrap());
    assert_eq!(archived(&s, sid), None);
    assert!(!s.touch_session(sid + 99).unwrap(), "no such row");
    // One click.
    s.archive_session_work(sid).unwrap();
    assert_eq!(s.unarchive_session_work(sid).unwrap(), 1);
    assert_eq!(archived(&s, sid), None);
}

#[test]
fn snooze_and_never_are_per_link_and_idempotent() {
    let s = Store::open_in_memory().unwrap();
    let sid = seed(&s, "dev");
    assert_eq!(
        s.snooze_tidy(sid, None, 99).unwrap_err().code,
        codes::E_NOTFOUND,
        "no linked work"
    );
    let l = s
        .link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
        .unwrap();
    assert_eq!(s.snooze_tidy(sid, None, 500).unwrap(), l.id);
    assert_eq!(s.snooze_tidy(sid, Some(l.id), 900).unwrap(), l.id);
    assert_eq!(
        s.snooze_tidy(sid, Some(l.id + 7), 900).unwrap_err().code,
        codes::E_NOTFOUND
    );
    assert_eq!(s.never_tidy(sid, None).unwrap(), l.id);
    assert_eq!(s.never_tidy(sid, None).unwrap(), l.id);
    let t = s
        .tidy_sessions()
        .unwrap()
        .into_iter()
        .find(|t| t.row.id == sid)
        .unwrap();
    let link = t.link.unwrap();
    assert_eq!((link.snoozed_until, link.never), (Some(900), true));
}

#[test]
fn tidy_sessions_reads_status_pr_tasks_and_protections() {
    let s = Store::open_in_memory().unwrap();
    let a = seed(&s, "a");
    let b = seed(&s, "b");
    let t = tracker(&s);
    let done = s
        .upsert_tracker_item(t, &write("1", "ABC-1", ("Done", "done")))
        .unwrap();
    let wip = s
        .upsert_tracker_item(t, &write("2", "ABC-2", ("In Progress", "in_progress")))
        .unwrap();
    s.link_session_work(a, WorkTarget::Item(done.id), "manual")
        .unwrap();
    // b's primary is done, but a second live link is in progress.
    s.link_session_work(b, WorkTarget::Item(wip.id), "manual")
        .unwrap();
    s.link_session_work(b, WorkTarget::Item(done.id), "manual")
        .unwrap();
    s.conn
        .execute(
            "UPDATE sessions SET pr_signals = '{\"head\":\"x\",\"state\":\"MERGED\"}', \
             current_branch = 'abc-1-login' WHERE id = ?1",
            [a],
        )
        .unwrap();
    s.insert_task(Some(a), Some(b), "p", "n").unwrap();
    let all = s.tidy_sessions().unwrap();
    let get = |id: i64| all.iter().find(|t| t.row.id == id).unwrap();
    let ta = get(a);
    assert_eq!(
        ta.link.as_ref().unwrap().status_category.as_deref(),
        Some("done")
    );
    assert!(ta.pr_merged);
    assert!(!ta.in_progress);
    assert_eq!(ta.branch.as_deref(), Some("abc-1-login"));
    assert!(ta.open_tasks, "the requester of an open task");
    let tb = get(b);
    assert!(tb.in_progress, "any live link in progress protects");
    assert!(!tb.pr_merged);
    assert!(tb.open_tasks, "the worker of an open task");
}

fn tracker(s: &Store) -> i64 {
    let t = s
        .add_tracker("jira", "Acme", "https://acme.atlassian.net")
        .unwrap();
    s.set_tracker_probe(
        t.id,
        None,
        &crate::store::TrackerConfig {
            key_prefixes: vec!["ABC".into()],
            ..Default::default()
        },
    )
    .unwrap();
    t.id
}

fn write(ext: &str, key: &str, status: (&str, &str)) -> TrackerItemWrite {
    TrackerItemWrite {
        external_id: ext.into(),
        key: Some(key.into()),
        title: format!("{key} title"),
        status_name: status.0.into(),
        status_category: status.1.into(),
        updated_ext: Some(100),
        ..Default::default()
    }
}

#[test]
fn a_reopen_is_the_transition_out_of_done_with_a_journal_row() {
    let s = Store::open_in_memory().unwrap();
    let sid = seed(&s, "dev");
    let t = tracker(&s);
    let item = s
        .upsert_tracker_item(t, &write("1", "ABC-1", ("In Progress", "in_progress")))
        .unwrap();
    s.link_session_work(sid, WorkTarget::Item(item.id), "manual")
        .unwrap();
    s.conn
        .execute(
            "UPDATE sessions SET claude_session_id = 'c-past' WHERE id = ?1",
            [sid],
        )
        .unwrap();
    s.conn
        .execute(
            "INSERT INTO conversations (session_id, claude_session_id, started_at, start_source) \
             VALUES (?1, 'c-past', 1, 'startup')",
            [sid],
        )
        .unwrap();
    // Done, then the session ends: past work.
    s.upsert_tracker_item(t, &write("1", "ABC-1", ("Done", "done")))
        .unwrap();
    s.delete_session(sid).unwrap();
    assert!(
        s.reopened_work().unwrap().is_empty(),
        "done is not reopened"
    );

    // Jira moves it back: a reopen, listed, journalled on the past conversation.
    s.upsert_tracker_item(t, &write("1", "ABC-1", ("In Progress", "in_progress")))
        .unwrap();
    let listed = s.reopened_work().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].key.as_deref(), Some("ABC-1"));
    assert_eq!(listed[0].past_sessions, 1);
    assert_eq!(listed[0].last_host.as_deref(), Some("h"));
    let journal = s
        .journal_for_conversations(&["c-past".to_string()])
        .unwrap();
    let row = journal
        .iter()
        .find(|j| j.kind == "reopened")
        .expect("a reopened journal row");
    assert!(row.body.as_deref().unwrap().contains("Done → In Progress"));
    assert!(row.meta.as_deref().unwrap().contains("\"reopened\":true"));

    // A second sync with the same state is no new event.
    s.upsert_tracker_item(t, &write("1", "ABC-1", ("In Progress", "in_progress")))
        .unwrap();
    let n = s
        .journal_for_conversations(&["c-past".to_string()])
        .unwrap()
        .iter()
        .filter(|j| j.kind == "reopened")
        .count();
    assert_eq!(n, 1);

    // Dismissed: gone; reopened again later: back.
    assert!(s.dismiss_reopened(item.id).unwrap());
    assert!(s.reopened_work().unwrap().is_empty());
    assert!(!s.dismiss_reopened(item.id).unwrap());
    assert_eq!(
        s.dismiss_reopened(item.id + 50).unwrap_err().code,
        codes::E_NOTFOUND
    );
    s.upsert_tracker_item(t, &write("1", "ABC-1", ("Done", "done")))
        .unwrap();
    s.upsert_tracker_item(t, &write("1", "ABC-1", ("To Do", "todo")))
        .unwrap();
    assert_eq!(s.reopened_work().unwrap().len(), 1);

    // Resumed: a session links to it after the reopen, and it drops out.
    let again = seed(&s, "again");
    s.link_session_work(again, WorkTarget::Item(item.id), "manual")
        .unwrap();
    assert!(s.reopened_work().unwrap().is_empty());
    // Done again settles it.
    s.upsert_tracker_item(t, &write("1", "ABC-1", ("Done", "done")))
        .unwrap();
    let stamp: Option<i64> = s
        .conn
        .query_row(
            "SELECT reopened_at FROM work_items WHERE id = ?1",
            [item.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stamp, None);
}
