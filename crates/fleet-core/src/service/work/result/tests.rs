use super::*;
use crate::ipc_error::codes;
use crate::store::{EvidenceCommit, NativeItem};

struct Fx {
    store: Mutex<Store>,
    item: i64,
    session: i64,
    host: String,
    other_host: String,
}

impl Fx {
    fn host_scope(&self) -> OrgScope {
        OrgScope::for_host(&self.store.lock().unwrap(), &self.host).unwrap()
    }

    fn other_org_scope(&self) -> OrgScope {
        OrgScope::for_host(&self.store.lock().unwrap(), &self.other_host).unwrap()
    }

    /// A finished task of `role` at the item, worked by the fixture's
    /// session, with `report` when one is given.
    fn finished(&self, role: &str, report: Option<TaskReport>) -> i64 {
        let s = self.store.lock().unwrap();
        let t = crate::service::tasks::create_task(&s, None, Some(self.session), "p").unwrap();
        s.set_task_run(t.id, self.item, 1, role).unwrap();
        s.finish_task(t.id, "done", Some("r"), None).unwrap();
        if let Some(r) = report {
            s.set_task_report(t.id, &r).unwrap();
        }
        t.id
    }
}

/// A local item named on a session of `mercury`, in its own org; a second
/// host in another org sees neither (the fixture of `status.rs`' tests).
fn fixture() -> Fx {
    let (host, other_host) = ("mercury", "the-other-host");
    let s = Store::open_in_memory().unwrap();
    s.upsert_host(host).unwrap();
    s.upsert_host(other_host).unwrap();
    let mine = s.add_org("Mine", None, false).unwrap();
    let theirs = s.add_org("Theirs", None, false).unwrap();
    s.set_host_org(host, Some(mine.id)).unwrap();
    s.set_host_org(other_host, Some(theirs.id)).unwrap();
    let sid = s
        .upsert_session("s", host, None, None, 1, 1, "running", None)
        .unwrap();
    let (item, _) = s.name_session_work(sid, None, "local work").unwrap();
    Fx {
        store: Mutex::new(s),
        item: item.id,
        session: sid,
        host: host.into(),
        other_host: other_host.into(),
    }
}

fn report(summary: &str, followups: &[&str]) -> TaskReport {
    TaskReport {
        summary: summary.into(),
        outcome: "done".into(),
        decisions: vec!["keep the cache".into()],
        followups: followups.iter().map(|f| f.to_string()).collect(),
        ..Default::default()
    }
}

#[test]
fn the_view_joins_report_evidence_gate_and_follow_ups() {
    let w = fixture();
    let task = w.finished("wrapup", Some(report("did it", &["A", "B"])));
    {
        let s = w.store.lock().unwrap();
        s.set_task_evidence(
            task,
            &TaskEvidence {
                at: 1,
                commits: vec![EvidenceCommit {
                    sha: "abc1234".into(),
                    subject: "feat: x".into(),
                }],
                commits_total: 1,
                ..Default::default()
            },
        )
        .unwrap();
        s.create_native_item(&NativeItem {
            title: "still open",
            parent_id: Some(w.item),
            ..Default::default()
        })
        .unwrap();
    }
    let v = result_for_item(&w.store, &OrgScope::All, w.item)
        .unwrap()
        .unwrap();
    assert_eq!(v.task_id, task);
    assert_eq!(v.item_id, w.item);
    assert_eq!(v.session_id, Some(w.session));
    assert_eq!(v.role, "wrapup");
    assert!(v.marker.starts_with("FLEET_TASK_DONE_"), "{}", v.marker);
    assert_eq!(v.report.summary, "did it");
    assert_eq!(v.report.decisions, vec!["keep the cache".to_string()]);
    assert_eq!(v.evidence.as_ref().unwrap().commits.len(), 1);
    assert_eq!(v.gate.state, "blocked");
    assert_eq!(v.gate.open_children.len(), 1);
    assert!(v.prs.is_empty(), "the session has no PR");
    assert_eq!(
        v.followups,
        vec![
            ResultFollowup {
                text: "A".into(),
                item_id: None
            },
            ResultFollowup {
                text: "B".into(),
                item_id: None
            },
        ]
    );
}

#[test]
fn an_item_with_no_report_has_no_result() {
    let w = fixture();
    assert!(result_for_item(&w.store, &OrgScope::All, w.item)
        .unwrap()
        .is_none());
    // A finished task with only a paragraph is not a result either.
    w.finished("implement", None);
    assert!(result_for_item(&w.store, &OrgScope::All, w.item)
        .unwrap()
        .is_none());
}

#[test]
fn the_newest_reported_task_is_the_result_whatever_its_role() {
    let w = fixture();
    let first = w.finished("implement", Some(report("first", &[])));
    let v = result_for_item(&w.store, &OrgScope::All, w.item)
        .unwrap()
        .unwrap();
    assert_eq!((v.task_id, v.role.as_str()), (first, "implement"));
    let second = w.finished("wrapup", Some(report("second", &[])));
    let v = result_for_item(&w.store, &OrgScope::All, w.item)
        .unwrap()
        .unwrap();
    assert_eq!((v.task_id, v.role.as_str()), (second, "wrapup"));
    assert_eq!(v.gate.state, "passed", "no lines and no open subtask");
}

#[test]
fn a_follow_up_already_made_names_its_item() {
    let w = fixture();
    let task = w.finished("wrapup", Some(report("r", &["Add the  index", "Docs"])));
    let (made, _) = w
        .store
        .lock()
        .unwrap()
        .create_followup(task, "add the index")
        .unwrap();
    let v = result_for_item(&w.store, &OrgScope::All, w.item)
        .unwrap()
        .unwrap();
    assert_eq!(
        v.followups[0].item_id,
        Some(made.id),
        "whitespace and case aside"
    );
    assert_eq!(v.followups[1].item_id, None);
}

/// A report's line longer than a title is stored cut (`followup::create`);
/// the view still names the item it became.
#[test]
fn a_follow_up_cut_to_a_title_still_names_its_item() {
    let w = fixture();
    let long = format!("Prune {}", "the weekly snapshots ".repeat(10));
    let task = w.finished("wrapup", Some(report("r", &[long.as_str()])));
    let made =
        super::super::followup::create(&w.store, &OrgScope::All, task, Some(0), None).unwrap();
    assert!(made.title.chars().count() < long.chars().count(), "cut");
    let v = result_for_item(&w.store, &OrgScope::All, w.item)
        .unwrap()
        .unwrap();
    assert_eq!(v.followups[0].item_id, Some(made.id));
}

#[test]
fn the_pr_is_the_probes_row_else_the_sessions_own_reading() {
    let w = fixture();
    w.finished("wrapup", Some(report("r", &[])));
    let url = "https://github.com/o/r/pull/7";
    w.store
        .lock()
        .unwrap()
        .conn_for_test()
        .execute(
            "UPDATE sessions SET pr_url = ?1, ci_status = 'pending' WHERE id = ?2",
            rusqlite::params![url, w.session],
        )
        .unwrap();
    let v = result_for_item(&w.store, &OrgScope::All, w.item)
        .unwrap()
        .unwrap();
    assert_eq!(
        v.prs,
        vec![ResultPr {
            url: url.into(),
            number: None,
            state: None,
            ci_status: Some("pending".into()),
            review_decision: None,
        }]
    );
    w.store
        .lock()
        .unwrap()
        .conn_for_test()
        .execute(
            "INSERT INTO pull_requests (url, number, state, ci_status, review_decision, \
               first_seen_at, updated_at) \
             VALUES (?1, 7, 'OPEN', 'passing', 'APPROVED', 1, 1)",
            rusqlite::params![url],
        )
        .unwrap();
    let v = result_for_item(&w.store, &OrgScope::All, w.item)
        .unwrap()
        .unwrap();
    assert_eq!(
        v.prs,
        vec![ResultPr {
            url: url.into(),
            number: Some(7),
            state: Some("OPEN".into()),
            ci_status: Some("passing".into()),
            review_decision: Some("APPROVED".into()),
        }]
    );
}

#[test]
fn an_item_or_session_outside_the_scope_answers_as_unknown() {
    let w = fixture();
    w.finished("wrapup", Some(report("r", &[])));
    let other = w.other_org_scope();
    let e = result_for_item(&w.store, &other, w.item).unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    let e = result_for_session(&w.store, &other, w.session).unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    let e = result_for_item(&w.store, &OrgScope::All, 9_999).unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
}

#[test]
fn an_agent_reads_the_workers_text_fenced_a_person_as_is() {
    let w = fixture();
    w.finished("wrapup", Some(report("did it", &["A"])));
    let v = result_for_item(&w.store, &w.host_scope(), w.item)
        .unwrap()
        .unwrap();
    assert_ne!(v.report.summary, "did it");
    assert!(v.report.summary.contains("did it"));
    assert_ne!(v.report.decisions[0], "keep the cache");
    assert_ne!(v.followups[0].text, "A");
    let v = result_for_item(&w.store, &OrgScope::All, w.item)
        .unwrap()
        .unwrap();
    assert_eq!(v.report.summary, "did it");
    assert_eq!(v.followups[0].text, "A");
}

#[test]
fn a_session_answers_its_primary_items_result() {
    let w = fixture();
    let task = w.finished("wrapup", Some(report("r", &[])));
    let v = result_for_session(&w.store, &w.host_scope(), w.session)
        .unwrap()
        .unwrap();
    assert_eq!((v.task_id, v.item_id), (task, w.item));

    let s = w.store.lock().unwrap();
    let bare = s
        .upsert_session("bare", "mercury", None, None, 1, 1, "running", None)
        .unwrap();
    drop(s);
    assert!(result_for_session(&w.store, &OrgScope::All, bare)
        .unwrap()
        .is_none());
    let e = result_for_session(&w.store, &OrgScope::All, 9_999).unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
}
