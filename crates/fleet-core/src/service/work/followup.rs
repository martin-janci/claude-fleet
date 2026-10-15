//! A follow-up task made from a task result: the scope fence and the title rules.
//!
//! Design 2026-10-13 D4. `work_link { action: followup, task_id, index?,
//! title? }` makes a sibling of the task's item through
//! [`Store::create_followup`]; this is the half that decides WHICH title and
//! whether the caller may: a task whose item is outside the caller's scope
//! answers exactly as an unknown task.

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::{self, OrgScope};
use crate::store::{ItemEdit, Store, WorkItemRow, LOCAL_WORK_TITLE_MAX_CHARS};
use std::sync::Mutex;

/// A follow-up of task `task_id`'s result. `title` (non-blank) wins;
/// otherwise `index` picks the report's `followups[index]`. Neither, or an
/// index the report does not have, is `E_INVALID`. The same (task, title)
/// twice answers the first item.
///
/// A report's follow-up longer than a title is cut to
/// [`LOCAL_WORK_TITLE_MAX_CHARS`] and kept whole in the new item's notes. A
/// `title` the caller wrote is not cut: the store refuses one too long.
pub fn create(
    store: &Mutex<Store>,
    scope: &OrgScope,
    task_id: i64,
    index: Option<usize>,
    title: Option<&str>,
) -> Result<WorkItemRow, IpcError> {
    let s = lock(store)?;
    let task = s
        .get_task(task_id)?
        .ok_or_else(|| orgs::not_found("task", task_id))?;
    let item = task
        .work_item_id
        .map(|id| s.get_work_item(id))
        .transpose()?
        .flatten()
        .ok_or_else(|| orgs::not_found("task", task_id))?;
    if !super::status::item_visible(&s, scope, &item)? {
        return Err(orgs::not_found("task", task_id));
    }
    let (title, full) = match (title.map(str::trim).filter(|t| !t.is_empty()), index) {
        (Some(t), _) => (t.to_string(), None),
        (None, Some(i)) => {
            let text = task
                .report
                .as_ref()
                .and_then(|r| r.followups.get(i))
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "no follow-up at that index"))?;
            cut_title(text)
        }
        (None, None) => {
            return Err(IpcError::new(
                codes::E_INVALID,
                "a follow-up needs an index or a title",
            ))
        }
    };
    let (row, new) = s.create_followup(task_id, &title)?;
    match full {
        Some(full) if new => Ok(s
            .edit_local_item(
                row.id,
                &ItemEdit {
                    notes: Some(&full),
                    ..Default::default()
                },
            )?
            .unwrap_or(row)),
        _ => Ok(row),
    }
}

/// A report's follow-up as a title: whitespace runs collapsed, cut on a char
/// boundary at [`LOCAL_WORK_TITLE_MAX_CHARS`]. `.1` is the whole text when it
/// was cut.
pub(super) fn cut_title(text: &str) -> (String, Option<String>) {
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= LOCAL_WORK_TITLE_MAX_CHARS {
        return (clean, None);
    }
    let cut: String = clean.chars().take(LOCAL_WORK_TITLE_MAX_CHARS).collect();
    (cut.trim_end().to_string(), Some(text.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::work::WorkLinkArgs;
    use crate::store::TaskReport;

    /// Hosts `h-a` (org A) and `h-b` (org B); a session on `h-a` named to
    /// the local item "Source"; a finished `wrapup` task on that item whose
    /// report lists `followups`. Answers the store and the task id.
    fn fixture_with_followups(followups: &[&str]) -> (Mutex<Store>, i64) {
        let s = Store::open_in_memory().unwrap();
        for h in ["h-a", "h-b"] {
            s.upsert_host(h).unwrap();
        }
        let a = s.add_org("A", None, false).unwrap();
        let b = s.add_org("B", None, false).unwrap();
        s.set_host_org("h-a", Some(a.id)).unwrap();
        s.set_host_org("h-b", Some(b.id)).unwrap();
        let sid = s
            .upsert_session("s-a", "h-a", None, None, 1, 1, "running", None)
            .unwrap();
        let store = Mutex::new(s);
        crate::service::work::local::name_session_work(
            &WorkLinkArgs {
                action: "name".into(),
                session_id: Some(sid),
                title: Some("Source".into()),
                ..Default::default()
            },
            &store,
            &OrgScope::All,
        )
        .unwrap();
        let task = {
            let s = store.lock().unwrap();
            let item = s
                .session_work_links(sid)
                .unwrap()
                .into_iter()
                .find_map(|l| l.item_id)
                .unwrap();
            let t = crate::service::tasks::create_task(&s, None, None, "wrap up").unwrap();
            s.set_task_run(t.id, item, 1, "wrapup").unwrap();
            s.set_task_report(
                t.id,
                &TaskReport {
                    summary: "did it".into(),
                    outcome: "done".into(),
                    followups: followups.iter().map(|f| f.to_string()).collect(),
                    ..Default::default()
                },
            )
            .unwrap();
            s.finish_task(t.id, "done", None, None).unwrap();
            t.id
        };
        (store, task)
    }

    fn host(store: &Mutex<Store>, alias: &str) -> OrgScope {
        OrgScope::for_host(&store.lock().unwrap(), alias).unwrap()
    }

    #[test]
    fn index_picks_the_reports_follow_up_and_a_title_overrides_it() {
        let (store, task) = fixture_with_followups(&["Add tests", "Fix Windows"]);
        let a = create(&store, &OrgScope::All, task, Some(1), None).unwrap();
        assert_eq!(a.title, "Fix Windows");
        assert_eq!(a.origin_task_id, Some(task));
        let b = create(&store, &OrgScope::All, task, Some(0), Some("Write docs")).unwrap();
        assert_eq!(b.title, "Write docs");
        let again = create(&store, &OrgScope::All, task, Some(1), None).unwrap();
        assert_eq!(again.id, a.id);
    }

    #[test]
    fn neither_index_nor_title_or_an_index_out_of_range_is_invalid() {
        let (store, task) = fixture_with_followups(&["Add tests"]);
        for (i, t) in [(None, None), (Some(5), None), (None, Some("   "))] {
            let e = create(&store, &OrgScope::All, task, i, t).unwrap_err();
            assert_eq!(e.code, codes::E_INVALID);
        }
        assert!(store.lock().unwrap().followups_of(task).unwrap().is_empty());
    }

    #[test]
    fn a_task_outside_the_scope_answers_as_unknown() {
        let (store, task) = fixture_with_followups(&["Add tests"]);
        let b = store.lock().unwrap().list_orgs().unwrap();
        let b = b.iter().find(|o| o.name == "B").unwrap().id;
        let other_org = OrgScope::Org {
            org: b,
            sees_unassigned: true,
        };
        for scope in [host(&store, "h-b"), other_org] {
            let e = create(&store, &scope, task, Some(0), None).unwrap_err();
            assert_eq!(e.code, codes::E_NOTFOUND);
        }
        assert!(store.lock().unwrap().followups_of(task).unwrap().is_empty());
        // The session's own host sees its item, and so may follow it up.
        let f = create(&store, &host(&store, "h-a"), task, Some(0), None).unwrap();
        assert_eq!(f.title, "Add tests");
    }

    #[test]
    fn an_unknown_task_or_one_with_no_item_answers_as_unknown() {
        let (store, task) = fixture_with_followups(&["Add tests"]);
        let bare = {
            let s = store.lock().unwrap();
            crate::service::tasks::create_task(&s, None, None, "p")
                .unwrap()
                .id
        };
        for id in [bare, task + 1000] {
            let e = create(&store, &OrgScope::All, id, None, Some("x")).unwrap_err();
            assert_eq!(e.code, codes::E_NOTFOUND);
        }
    }

    #[test]
    fn a_long_follow_up_is_cut_to_a_title_and_kept_whole_in_the_notes() {
        let long = format!("Port the {} to Windows", "é".repeat(200));
        let (store, task) = fixture_with_followups(&[&long]);
        let f = create(&store, &OrgScope::All, task, Some(0), None).unwrap();
        assert_eq!(f.title.chars().count(), LOCAL_WORK_TITLE_MAX_CHARS);
        assert!(long.starts_with(&f.title));
        assert_eq!(f.notes.as_deref(), Some(long.as_str()));
        // The same follow-up again is the same item.
        let again = create(&store, &OrgScope::All, task, Some(0), None).unwrap();
        assert_eq!(again.id, f.id);
    }

    #[test]
    fn a_short_follow_up_has_no_notes() {
        let (store, task) = fixture_with_followups(&["Add  integration\ntests"]);
        let f = create(&store, &OrgScope::All, task, Some(0), None).unwrap();
        assert_eq!(f.title, "Add integration tests");
        assert_eq!(f.notes, None);
    }
}
