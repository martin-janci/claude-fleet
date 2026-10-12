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
