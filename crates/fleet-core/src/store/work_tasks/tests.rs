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
