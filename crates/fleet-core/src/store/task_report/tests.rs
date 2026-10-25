//! A task's report and evidence columns (orchestration O3).

use super::*;

#[test]
fn a_report_and_evidence_round_trip_and_garbage_reads_as_absent() {
    let s = Store::open_in_memory().unwrap();
    let item = s.create_local_work_item(None, "queue").unwrap().id;
    let t = s.insert_task(None, None, "go", "n1").unwrap();
    s.set_task_run(t.id, item, 1, "implement").unwrap();
    assert_eq!(s.get_task(t.id).unwrap().unwrap().report, None);
    let report = TaskReport {
        summary: "did it".into(),
        outcome: "done".into(),
        tests_run: vec!["cargo test".into()],
        ..Default::default()
    };
    s.set_task_report(t.id, &report).unwrap();
    let ev = TaskEvidence {
        at: 5,
        head: Some("abc".into()),
        commits_total: 1,
        commits: vec![EvidenceCommit {
            sha: "abc".into(),
            subject: "Add".into(),
        }],
        ..Default::default()
    };
    s.set_task_evidence(t.id, &ev).unwrap();
    let row = s.get_task(t.id).unwrap().unwrap();
    assert_eq!(row.report, Some(report));
    assert_eq!(row.evidence, Some(ev));
    s.conn_ref()
        .execute(
            "UPDATE tasks SET result_json = '{not json' WHERE id = ?1",
            [t.id],
        )
        .unwrap();
    assert_eq!(s.get_task(t.id).unwrap().unwrap().report, None);
}

#[test]
fn the_latest_attempt_per_role_is_found() {
    let s = Store::open_in_memory().unwrap();
    let item = s.create_local_work_item(None, "queue").unwrap().id;
    assert_eq!(s.latest_item_task(item, "review").unwrap(), None);
    let a = s.insert_task(None, None, "r1", "n1").unwrap();
    s.set_task_run(a.id, item, 1, "review").unwrap();
    let b = s.insert_task(None, None, "r2", "n2").unwrap();
    s.set_task_run(b.id, item, 2, "review").unwrap();
    let c = s.insert_task(None, None, "i1", "n3").unwrap();
    s.set_task_run(c.id, item, 1, "implement").unwrap();
    assert_eq!(
        s.latest_item_task(item, "review").unwrap().unwrap().id,
        b.id
    );
    assert_eq!(
        s.latest_item_task(item, "implement").unwrap().unwrap().id,
        c.id
    );
}

#[test]
fn an_items_pr_reading_comes_from_its_runs_worker() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let w = s
        .upsert_session("w", "h", None, None, 1, 1, "running", None)
        .unwrap();
    let item = s.create_local_work_item(None, "queue").unwrap().id;
    assert_eq!(s.item_pr_evidence(item).unwrap(), None);
    let t = s.insert_task(None, Some(w), "go", "n1").unwrap();
    s.set_task_run(t.id, item, 1, "implement").unwrap();
    s.conn_ref()
        .execute(
            "UPDATE sessions SET pr_evidence = '{\"head_oid\":\"x\",\"checks\":{\"total\":2}}', \
             pr_checked_at = 9 WHERE id = ?1",
            [w],
        )
        .unwrap();
    let (ev, at) = s.item_pr_evidence(item).unwrap().unwrap();
    assert_eq!(
        (ev.head_oid.as_deref(), ev.checks.total, at),
        (Some("x"), 2, Some(9))
    );
}

#[test]
fn the_latest_result_is_the_newest_done_task_with_a_report() {
    let s = Store::open_in_memory().unwrap();
    let item = s.create_local_work_item(None, "queue").unwrap().id;
    assert_eq!(s.latest_item_result(item).unwrap(), None);
    let mk = |nonce: &str, role: &str, state: &str, report: bool| {
        let t = s.insert_task(None, None, "p", nonce).unwrap();
        s.set_task_run(t.id, item, 1, role).unwrap();
        s.finish_task(t.id, state, Some("r"), None).unwrap();
        if report {
            let r = TaskReport {
                summary: role.into(),
                outcome: "done".into(),
                ..Default::default()
            };
            s.set_task_report(t.id, &r).unwrap();
        }
        t.id
    };
    let _plain = mk("n1", "implement", "done", false);
    assert_eq!(
        s.latest_item_result(item).unwrap(),
        None,
        "a paragraph alone is no result"
    );
    let first = mk("n2", "implement", "done", true);
    assert_eq!(s.latest_item_result(item).unwrap().unwrap().id, first);
    let second = mk("n3", "wrapup", "done", true);
    assert_eq!(s.latest_item_result(item).unwrap().unwrap().id, second);
    let _failed = mk("n4", "implement", "failed", true);
    assert_eq!(
        s.latest_item_result(item).unwrap().unwrap().id,
        second,
        "only a done task is a result"
    );
}

/// review8: a mid-turn read never replaces a settled one; the Stop owes
/// evidence to a reported run with none or a provisional one; the loop
/// reads failed reads again.
#[test]
fn open_evidence_is_replaced_and_found_settled_evidence_is_not() {
    let s = Store::open_in_memory().unwrap();
    let item = s.create_local_work_item(None, "queue").unwrap().id;
    let t = s.insert_task(None, Some(7), "go", "n1").unwrap();
    s.set_task_run(t.id, item, 1, "implement").unwrap();
    s.conn_ref()
        .execute(
            "UPDATE tasks SET state = 'done', finished_at = 100 WHERE id = ?1",
            [t.id],
        )
        .unwrap();
    let owed = |since| {
        s.runs_awaiting_stop_evidence(7, since)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(owed(50), vec![t.id], "no evidence yet");
    assert!(owed(101).is_empty(), "finished before the window");
    let mid = TaskEvidence {
        at: 101,
        provisional: true,
        ..Default::default()
    };
    assert!(s.replace_open_task_evidence(t.id, &mid, false).unwrap());
    assert_eq!(owed(50), vec![t.id], "a provisional read is read again");
    let settled = TaskEvidence {
        at: 102,
        commits_total: 1,
        ..Default::default()
    };
    s.set_task_evidence(t.id, &settled).unwrap();
    assert!(owed(50).is_empty());
    assert!(!s.replace_open_task_evidence(t.id, &mid, false).unwrap());
    assert!(!s.replace_open_task_evidence(t.id, &mid, true).unwrap());
    assert_eq!(
        s.get_task(t.id).unwrap().unwrap().evidence,
        Some(settled.clone())
    );
    // A failed read: a retry replaces it, a mid-turn read does not.
    let failed = TaskEvidence {
        at: 103,
        error: Some("ssh: reset".into()),
        host: Some("mars".into()),
        dir: Some("/w".into()),
        ..Default::default()
    };
    s.set_task_evidence(t.id, &failed).unwrap();
    assert!(!s.replace_open_task_evidence(t.id, &mid, false).unwrap());
    assert!(s.replace_open_task_evidence(t.id, &settled, true).unwrap());
}

/// review9: the loop reads again every done run whose read failed with the
/// checkout recorded, within the window and in a mission or not; a run whose
/// git answered (an error without a host) or that is not done is not read.
#[test]
fn failed_reads_with_a_checkout_are_found_for_any_run() {
    let s = Store::open_in_memory().unwrap();
    let item = s.create_local_work_item(None, "queue").unwrap().id;
    let run = |finished: i64, state: &str| {
        let t = s.insert_task(None, Some(7), "go", "n").unwrap();
        s.set_task_run(t.id, item, 1, "implement").unwrap();
        s.conn_ref()
            .execute(
                "UPDATE tasks SET state = ?2, finished_at = ?3 WHERE id = ?1",
                rusqlite::params![t.id, state, finished],
            )
            .unwrap();
        t.id
    };
    let failed = TaskEvidence {
        at: 100,
        error: Some("ssh: reset".into()),
        host: Some("mars".into()),
        dir: Some("/w".into()),
        ..Default::default()
    };
    let unread = run(100, "done");
    s.set_task_evidence(unread, &failed).unwrap();
    let mid_turn = run(100, "done");
    s.set_task_evidence(
        mid_turn,
        &TaskEvidence {
            provisional: true,
            ..failed.clone()
        },
    )
    .unwrap();
    let old = run(10, "done");
    s.set_task_evidence(old, &failed).unwrap();
    let answered = run(100, "done");
    s.set_task_evidence(
        answered,
        &TaskEvidence {
            at: 100,
            error: Some("no checkout".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let settled = run(100, "done");
    s.set_task_evidence(
        settled,
        &TaskEvidence {
            at: 100,
            commits_total: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let cancelled = run(100, "cancelled");
    s.set_task_evidence(cancelled, &failed).unwrap();
    let found: Vec<i64> = s
        .runs_with_unread_evidence(50)
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(found, vec![unread, mid_turn]);
}

/// Review r10: the evidence retry's per-pass probe walks its partial index
/// (migration 208), not every done run the store ever recorded.
#[test]
fn the_unread_evidence_probe_uses_its_index() {
    let s = Store::open_in_memory().unwrap();
    let mut st = s
        .conn
        .prepare(&format!("EXPLAIN QUERY PLAN {}", unread_evidence_sql()))
        .unwrap();
    let plan = st
        .query_map([1], |r| r.get::<_, String>(3))
        .unwrap()
        .map(|r| r.unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("idx_tasks_unread_evidence"),
        "the probe scans: {plan}"
    );
}
