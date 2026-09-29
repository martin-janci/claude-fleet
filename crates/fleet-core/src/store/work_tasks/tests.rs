use super::*;
use crate::ipc_error::codes;
use crate::store::Store;

fn native(title: &str) -> NativeItem<'_> {
    NativeItem {
        title,
        parent_id: None,
        project_id: None,
        notes: None,
    }
}

#[test]
fn a_native_task_gets_a_task_key_its_project_and_notes() {
    let s = Store::open_in_memory().unwrap();
    let pid = s.upsert_project("acme", "api", "/src/api").unwrap();
    let t = s
        .create_native_item(&NativeItem {
            title: " Fix login ",
            parent_id: None,
            project_id: Some(pid),
            notes: Some("CI run 12"),
        })
        .unwrap();
    assert_eq!(
        (t.source.as_str(), t.origin.as_deref()),
        ("local", Some("manual"))
    );
    assert_eq!(t.key.as_deref(), Some(format!("TASK-{}", t.id).as_str()));
    assert_eq!(
        (t.title.as_str(), t.project_id, t.notes.as_deref()),
        ("Fix login", Some(pid), Some("CI run 12"))
    );
    assert_eq!(t.status_category, "todo");
}

#[test]
fn a_subtask_hangs_under_a_ticket_but_never_under_a_subtask() {
    let s = Store::open_in_memory().unwrap();
    let ticket = s.create_local_work_item(Some("OM-110"), "Qomora").unwrap();
    let sub = s
        .create_native_item(&NativeItem {
            parent_id: Some(ticket.id),
            ..native("Stats")
        })
        .unwrap();
    assert_eq!(sub.parent_id, Some(ticket.id));
    let e = s
        .create_native_item(&NativeItem {
            parent_id: Some(sub.id),
            ..native("Deeper")
        })
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    let e = s
        .create_native_item(&NativeItem {
            parent_id: Some(999_999),
            ..native("x")
        })
        .unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    assert_eq!(s.native_children(ticket.id).unwrap().len(), 1);
}

#[test]
fn a_native_task_refuses_an_empty_title_and_an_unknown_project() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(
        s.create_native_item(&native("  ")).unwrap_err().code,
        codes::E_INVALID
    );
    assert_eq!(
        s.create_native_item(&NativeItem {
            project_id: Some(999_999),
            ..native("x")
        })
        .unwrap_err()
        .code,
        codes::E_NOTFOUND
    );
}

#[test]
fn a_mirrored_job_is_an_agent_subtask_that_follows_the_job() {
    let s = Store::open_in_memory().unwrap();
    let parent = s.create_native_item(&native("Ship v1")).unwrap();
    let job = s
        .insert_task(None, None, "Write the changelog\nfrom git log", "n1")
        .unwrap();
    let it = s
        .create_agent_task_item(&job, Some(parent.id), None)
        .unwrap();
    assert_eq!(
        (it.origin.as_deref(), it.task_id, it.parent_id),
        (Some("agent"), Some(job.id), Some(parent.id))
    );
    assert_eq!(it.title, "Write the changelog");
    assert_eq!(
        (it.status_category.as_str(), it.status_set_by.as_deref()),
        ("todo", Some("task"))
    );
    assert_eq!(
        s.create_agent_task_item(&job, None, None).unwrap().id,
        it.id,
        "once per job"
    );
    s.mark_task_running(job.id).unwrap();
    assert_eq!(
        s.job_states_by_item()
            .unwrap()
            .get(&it.id)
            .map(String::as_str),
        Some("running")
    );
}

#[test]
fn a_jobs_status_never_overrides_a_person() {
    let s = Store::open_in_memory().unwrap();
    let t = s.create_native_item(&native("x")).unwrap();
    assert!(s.set_item_status_from_task(t.id, "in_progress").unwrap());
    assert!(
        !s.set_item_status_from_task(t.id, "in_progress").unwrap(),
        "no repeat write"
    );
    s.set_item_status(t.id, "done").unwrap();
    assert!(!s.set_item_status_from_task(t.id, "in_progress").unwrap());
    assert_eq!(
        s.get_work_item(t.id)
            .unwrap()
            .unwrap()
            .status_set_by
            .as_deref(),
        Some("person")
    );
}

#[test]
fn the_job_status_map() {
    assert_eq!(job_status("queued"), "todo");
    assert_eq!(job_status("running"), "in_progress");
    for st in ["done", "failed", "cancelled"] {
        assert_eq!(job_status(st), "done");
    }
}

fn proposal(parent: i64, title: &str) -> Proposal<'_> {
    Proposal {
        parent_id: parent,
        title,
        notes: None,
        why: Some("because"),
        proposed_by: "OM-110 · trn",
    }
}

#[test]
fn a_proposal_waits_for_a_person_then_becomes_a_subtask() {
    let s = Store::open_in_memory().unwrap();
    let ticket = s.create_local_work_item(Some("OM-110"), "Qomora").unwrap();
    let p = s
        .propose_subtask(&proposal(ticket.id, "Decide the P0 owner"))
        .unwrap();
    assert_eq!(
        (p.origin.as_deref(), p.proposal_state.as_deref()),
        (Some("proposed"), Some("proposed"))
    );
    assert_eq!(
        (p.proposed_by.as_deref(), p.proposal_why.as_deref()),
        (Some("OM-110 · trn"), Some("because"))
    );
    let a = s.decide_proposal(p.id, true).unwrap();
    assert_eq!(
        (a.proposal_state.as_deref(), a.status_category.as_str()),
        (Some("accepted"), "todo")
    );
    assert_eq!(
        s.decide_proposal(p.id, false).unwrap_err().code,
        codes::E_INVALID,
        "decided once"
    );
}

#[test]
fn a_rejected_title_is_not_proposed_again_under_the_same_parent() {
    let s = Store::open_in_memory().unwrap();
    let t = s.create_local_work_item(Some("OM-110"), "Qomora").unwrap();
    let p = s
        .propose_subtask(&proposal(t.id, "Create om-catalog module"))
        .unwrap();
    s.decide_proposal(p.id, false).unwrap();
    let e = s
        .propose_subtask(&proposal(t.id, "create OM-CATALOG module "))
        .unwrap_err();
    assert_eq!(e.code, codes::E_EXISTS);
}

#[test]
fn open_proposals_are_capped_per_parent() {
    let s = Store::open_in_memory().unwrap();
    let t = s.create_local_work_item(Some("OM-110"), "Qomora").unwrap();
    for i in 0..PROPOSALS_OPEN_CAP {
        s.propose_subtask(&proposal(t.id, &format!("idea {i}")))
            .unwrap();
    }
    assert_eq!(
        s.propose_subtask(&proposal(t.id, "one too many"))
            .unwrap_err()
            .code,
        codes::E_LIMIT
    );
}

#[test]
fn deciding_something_that_is_not_a_proposal_is_refused() {
    let s = Store::open_in_memory().unwrap();
    let t = s.create_native_item(&native("plain")).unwrap();
    assert_eq!(
        s.decide_proposal(t.id, true).unwrap_err().code,
        codes::E_INVALID
    );
}

#[test]
fn local_items_named_after_the_migration_are_manual_too() {
    let s = Store::open_in_memory().unwrap();
    let it = s.create_local_work_item(Some("OPS-9"), "ops").unwrap();
    assert_eq!(it.origin.as_deref(), Some("manual"));
    s.upsert_host("h").unwrap();
    let sid = s
        .upsert_session("w", "h", None, None, 1, 1, "running", None)
        .unwrap();
    let (named, _) = s.name_session_work(sid, None, "named work").unwrap();
    assert_eq!(named.origin.as_deref(), Some("manual"));
}
