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
fn a_subtask_hangs_three_local_levels_deep_and_no_deeper() {
    let s = Store::open_in_memory().unwrap();
    let ticket = s.create_local_work_item(Some("OM-110"), "Qomora").unwrap();
    let sub = s
        .create_native_item(&NativeItem {
            parent_id: Some(ticket.id),
            ..native("Stats")
        })
        .unwrap();
    assert_eq!(sub.parent_id, Some(ticket.id));
    // Owner decision 2026-10-10: epic → task → subtask, LOCAL_DEPTH_MAX deep.
    let deeper = s
        .create_native_item(&NativeItem {
            parent_id: Some(sub.id),
            ..native("Deeper")
        })
        .unwrap();
    assert_eq!(s.local_depth(deeper.id).unwrap(), LOCAL_DEPTH_MAX);
    let e = s
        .create_native_item(&NativeItem {
            parent_id: Some(deeper.id),
            ..native("Too deep")
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
        s.job_tasks_by_item()
            .unwrap()
            .get(&it.id)
            .map(|t| t.state.as_str()),
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

#[test]
fn a_person_edits_a_local_items_title_notes_and_assignees() {
    let s = Store::open_in_memory().unwrap();
    let t = s
        .create_native_item(&NativeItem {
            notes: Some("old"),
            ..native("Fix login")
        })
        .unwrap();
    let people = vec![" Ana ".to_string(), "ana".into(), "".into(), "Bo".into()];
    let e = s
        .edit_local_item(
            t.id,
            &ItemEdit {
                title: Some(" Fix the login "),
                notes: Some(" new notes "),
                assignees: Some(&people),
                due_at: None,
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        (e.title.as_str(), e.notes.as_deref(), e.assignees.clone()),
        (
            "Fix the login",
            Some("new notes"),
            vec!["Ana".to_string(), "Bo".into()]
        )
    );
    // A field left out stays; an empty one clears.
    let e = s
        .edit_local_item(
            t.id,
            &ItemEdit {
                notes: Some("  "),
                assignees: Some(&[]),
                ..ItemEdit::default()
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        (e.title.as_str(), e.notes, e.assignees),
        ("Fix the login", None, Vec::<String>::new())
    );
}

#[test]
fn an_edit_refuses_a_bad_title_or_assignee_and_a_job_mirrors_notes() {
    let s = Store::open_in_memory().unwrap();
    let t = s.create_native_item(&native("Fix login")).unwrap();
    let e = s
        .edit_local_item(
            t.id,
            &ItemEdit {
                title: Some(" "),
                ..ItemEdit::default()
            },
        )
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    let many: Vec<String> = (0..=ASSIGNEES_MAX).map(|i| format!("p{i}")).collect();
    let e = s
        .edit_local_item(
            t.id,
            &ItemEdit {
                assignees: Some(&many),
                ..ItemEdit::default()
            },
        )
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    assert_eq!(s.get_work_item(t.id).unwrap().unwrap().title, "Fix login");

    s.conn
        .execute(
            "UPDATE work_items SET origin = 'agent' WHERE id = ?1",
            [t.id],
        )
        .unwrap();
    let e = s
        .edit_local_item(
            t.id,
            &ItemEdit {
                notes: Some("rewrite the prompt"),
                ..ItemEdit::default()
            },
        )
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
}

#[test]
fn an_edit_leaves_a_trackers_ticket_alone() {
    let s = Store::open_in_memory().unwrap();
    let t = s.create_native_item(&native("Fix login")).unwrap();
    s.conn
        .execute(
            "UPDATE work_items SET source = 'jira' WHERE id = ?1",
            [t.id],
        )
        .unwrap();
    let r = s
        .edit_local_item(
            t.id,
            &ItemEdit {
                title: Some("Hijack"),
                ..ItemEdit::default()
            },
        )
        .unwrap();
    assert!(r.is_none());
    assert_eq!(
        s.edit_local_item(999_999, &ItemEdit::default()).unwrap(),
        None
    );
}

#[test]
fn a_person_sets_changes_and_clears_a_due_date() {
    let s = Store::open_in_memory().unwrap();
    let t = s
        .create_native_item(&native("Rotate NAS password"))
        .unwrap();
    assert_eq!(t.due_at, None);
    fn due(d: &str) -> ItemEdit<'_> {
        ItemEdit {
            due_at: Some(d),
            ..ItemEdit::default()
        }
    }
    let e = s
        .edit_local_item(t.id, &due(" 2026-10-16 "))
        .unwrap()
        .unwrap();
    assert_eq!(e.due_at.as_deref(), Some("2026-10-16"));
    assert!(e.updated_at >= t.updated_at);
    // Read back through every column reader, not only the edit's answer.
    assert_eq!(
        s.get_work_item(t.id).unwrap().unwrap().due_at.as_deref(),
        Some("2026-10-16")
    );
    // Leaving it out keeps it; editing the title alone does not clear it.
    let e = s
        .edit_local_item(
            t.id,
            &ItemEdit {
                title: Some("Rotate the NAS password"),
                ..ItemEdit::default()
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(e.due_at.as_deref(), Some("2026-10-16"));
    let e = s.edit_local_item(t.id, &due("")).unwrap().unwrap();
    assert_eq!(e.due_at, None);
}

#[test]
fn a_due_date_that_is_not_a_calendar_day_is_refused_and_writes_nothing() {
    let s = Store::open_in_memory().unwrap();
    let t = s.create_native_item(&native("Ship it")).unwrap();
    for bad in [
        "tomorrow",
        "2026-02-29",
        "2026-04-31",
        "2026-13-01",
        "2026-10-16T09:00",
        "26-10-16",
        "2026-1-16",
        "+026-10-16",
    ] {
        let e = s
            .edit_local_item(
                t.id,
                &ItemEdit {
                    title: Some("Renamed"),
                    due_at: Some(bad),
                    ..ItemEdit::default()
                },
            )
            .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "{bad}");
        assert!(e.message.contains("YYYY-MM-DD"), "{}", e.message);
    }
    let row = s.get_work_item(t.id).unwrap().unwrap();
    assert_eq!((row.title.as_str(), row.due_at), ("Ship it", None));
    assert_eq!(parse_due_date("2028-02-29"), Some((2028, 2, 29)));
    assert_eq!(parse_due_date("2100-02-29"), None);
}

/// Redesign 6.9: Merge moves what hangs on a duplicate proposal onto the
/// task it duplicates, then closes the proposal; a second decision is
/// refused, and so is a merge into the proposal itself.
#[test]
fn merging_a_duplicate_proposal_moves_its_links_and_subtasks_then_closes_it() {
    let s = Store::open_in_memory().unwrap();
    let ticket = s.create_local_work_item(Some("OM-110"), "Qomora").unwrap();
    let existing = s.create_native_item(&native("Receipt totals")).unwrap();
    let p = s
        .propose_subtask(&proposal(ticket.id, "Fix receipt totals"))
        .unwrap();
    assert_eq!(
        s.merge_proposal_into(p.id, p.id).unwrap_err().code,
        codes::E_INVALID
    );
    // Two sessions on the proposal, one of them already live on the task.
    s.upsert_host("h").unwrap();
    let a = s
        .upsert_session("a", "h", None, None, 1, 1, "running", None)
        .unwrap();
    let b = s
        .upsert_session("b", "h", None, None, 1, 1, "running", None)
        .unwrap();
    s.link_session_work(a, crate::store::WorkTarget::Item(p.id), "manual")
        .unwrap();
    s.link_session_work_as(
        b,
        crate::store::WorkTarget::Item(existing.id),
        "manual",
        true,
        None,
    )
    .unwrap();
    s.link_session_work_as(
        b,
        crate::store::WorkTarget::Item(p.id),
        "manual",
        false,
        None,
    )
    .unwrap();
    // A subtask hung on the proposal (written directly: the API proposes
    // under tickets only).
    let child = s.create_native_item(&native("Totals: tests")).unwrap();
    s.conn_for_test()
        .execute(
            "UPDATE work_items SET parent_id = ?1 WHERE id = ?2",
            rusqlite::params![p.id, child.id],
        )
        .unwrap();

    let (into, moved) = s.merge_proposal_into(p.id, existing.id).unwrap();
    assert_eq!(into.id, existing.id);
    assert_eq!(moved.subtasks, 1);
    assert_eq!(moved.ended_links, 1, "b was already live on the task");
    assert_eq!(
        s.get_work_item(child.id).unwrap().unwrap().parent_id,
        Some(existing.id)
    );
    let live_on = |sid: i64| -> Vec<Option<i64>> {
        s.session_work_links(sid)
            .unwrap()
            .into_iter()
            .filter(|l| l.ended_at.is_none())
            .map(|l| l.item_id)
            .collect()
    };
    assert_eq!(live_on(a), vec![Some(existing.id)]);
    assert_eq!(live_on(b), vec![Some(existing.id)], "no doubled link");
    let closed = s.get_work_item(p.id).unwrap().unwrap();
    assert_eq!(closed.proposal_state.as_deref(), Some("rejected"));
    assert_eq!(
        s.merge_proposal_into(p.id, existing.id).unwrap_err().code,
        codes::E_INVALID,
        "decided once"
    );
    assert_eq!(
        s.decide_proposal(p.id, true).unwrap_err().code,
        codes::E_INVALID
    );
}

/// Sprints design 2026-09-28 §3: a local item filed under an epic, one level
/// deep, never across organisations, and keeping its org when taken out.
#[test]
fn a_task_is_filed_under_an_epic_three_levels_deep_and_keeps_its_org_out() {
    let s = Store::open_in_memory().unwrap();
    let acme = s.add_org("Acme", None, false).unwrap().id;
    let other = s.add_org("Initech", None, false).unwrap().id;
    let epic = s.create_native_item(&native("Login revamp")).unwrap();
    s.set_local_item_org(epic.id, Some(acme)).unwrap();
    let e = s.set_local_epic(epic.id, true).unwrap().unwrap();
    assert!(is_epic(&e));
    let task = s.create_native_item(&native("Fix login")).unwrap();

    // Filed: it reads its org from the epic.
    let t = s.set_local_parent(task.id, Some(epic.id)).unwrap().unwrap();
    assert_eq!(t.parent_id, Some(epic.id));
    assert_eq!(s.item_org(task.id).unwrap(), Some(acme));
    // Again is no change.
    assert_eq!(
        s.set_local_parent(task.id, Some(epic.id))
            .unwrap()
            .unwrap()
            .parent_id,
        Some(epic.id)
    );

    // Three levels: a subtask goes under a filed task, and reads its org
    // from the epic two levels up; a fourth level is refused, on create and
    // on a move.
    let sub = s.create_native_item(&native("Write tests")).unwrap();
    s.set_local_parent(sub.id, Some(task.id)).unwrap();
    assert_eq!(s.item_org(sub.id).unwrap(), Some(acme));
    assert!(s
        .create_native_item(&NativeItem {
            parent_id: Some(sub.id),
            ..native("Nested")
        })
        .is_err());
    let loose = s.create_native_item(&native("Loose")).unwrap();
    let err = s.set_local_parent(loose.id, Some(sub.id)).unwrap_err();
    assert_eq!(err.code, codes::E_INVALID, "{}", err.message);
    // A cycle: the task under its own subtask.
    let err = s.set_local_parent(task.id, Some(sub.id)).unwrap_err();
    assert!(err.message.contains("own subtasks"), "{}", err.message);
    // Back out, so the rest of the test starts from the two-level shape.
    s.set_local_parent(sub.id, None).unwrap();
    let other_epic = s.create_native_item(&native("Billing")).unwrap();
    s.set_local_epic(other_epic.id, true).unwrap();
    let err = s
        .set_local_parent(epic.id, Some(other_epic.id))
        .unwrap_err();
    assert!(err.message.contains("epic"), "{}", err.message);
    let err = s.set_local_parent(sub.id, Some(sub.id)).unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
    let err = s.set_local_epic(task.id, true).unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
    // A task with a subtask goes under an epic (three levels), and its
    // subtask then reads the epic's org; one more level would not fit.
    let parent = s.create_native_item(&native("Parent")).unwrap();
    let child = s
        .create_native_item(&NativeItem {
            parent_id: Some(parent.id),
            ..native("Child")
        })
        .unwrap();
    s.set_local_item_org(other_epic.id, Some(acme)).unwrap();
    s.set_local_parent(parent.id, Some(other_epic.id)).unwrap();
    assert_eq!(s.item_org(child.id).unwrap(), Some(acme));
    s.set_local_parent(parent.id, None).unwrap();
    s.create_native_item(&NativeItem {
        parent_id: Some(child.id),
        ..native("Grandchild")
    })
    .unwrap();
    let err = s
        .set_local_parent(parent.id, Some(other_epic.id))
        .unwrap_err();
    assert!(err.message.contains("levels deep"), "{}", err.message);

    // Never across organisations.
    s.set_local_item_org(sub.id, Some(other)).unwrap();
    let err = s.set_local_parent(sub.id, Some(epic.id)).unwrap_err();
    assert_eq!(err.code, codes::E_FORBIDDEN, "{}", err.message);

    // A subtask that reads its org from its parent moves within that org
    // only.
    let foreign = s.create_native_item(&native("Elsewhere")).unwrap();
    s.set_local_item_org(foreign.id, Some(other)).unwrap();
    let err = s.set_local_parent(task.id, Some(foreign.id)).unwrap_err();
    assert_eq!(err.code, codes::E_FORBIDDEN, "{}", err.message);

    // Taken out to the top, it keeps the org it read from the epic.
    let t = s.set_local_parent(task.id, None).unwrap().unwrap();
    assert_eq!(t.parent_id, None);
    assert_eq!(s.item_org(task.id).unwrap(), Some(acme));
    // And it can be a parent again.
    s.create_native_item(&NativeItem {
        parent_id: Some(task.id),
        ..native("Subtask")
    })
    .unwrap();
}

#[test]
fn a_tracker_ticket_a_job_and_a_proposal_are_not_filed_by_hand() {
    let s = Store::open_in_memory().unwrap();
    let epic = s.create_native_item(&native("Epic")).unwrap();
    let parent = s.create_native_item(&native("Parent")).unwrap();
    let p = s
        .propose_subtask(&Proposal {
            parent_id: parent.id,
            title: "Maybe",
            notes: None,
            why: None,
            proposed_by: "agent · h1",
        })
        .unwrap();
    let err = s.set_local_parent(p.id, Some(epic.id)).unwrap_err();
    assert!(err.message.contains("proposal"), "{}", err.message);
    assert!(s.set_local_parent(999_999, None).unwrap().is_none());
}
