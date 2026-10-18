//! Follow-up items: a sibling work item born from a task's result (`work_items.origin_task_id`, migration 199).
//!
//! Design 2026-10-13 D4: the follow-up sits beside the task's item (same
//! parent, same project) and names the task as its origin; the origin
//! session, item and result all follow from that `tasks` row. A follow-up
//! of a tracker item is still a local item: fleet never creates tickets at
//! a tracker here.

use super::{NativeItem, Store, WorkItemRow};
use crate::ipc_error::{codes, IpcError};

/// The title as stored: inner whitespace runs collapsed to one space.
fn clean(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The title as compared: [`clean`], case folded.
pub(crate) fn norm(t: &str) -> String {
    clean(t).to_lowercase()
}

impl Store {
    /// (title, item id) of every follow-up made from `task_id`, oldest first.
    pub fn followups_of(&self, task_id: i64) -> Result<Vec<(String, i64)>, IpcError> {
        let mut stmt = self
            .conn
            .prepare("SELECT title, id FROM work_items WHERE origin_task_id = ?1 ORDER BY id")?;
        let rows = stmt.query_map([task_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// A sibling of the task's item, born from that task's result. The same
    /// (task, title) twice — whitespace and case aside — answers the first
    /// item; `.1` says whether it is new. `E_INVALID` when the task has no
    /// work item (or does not exist), or the title is not a valid one.
    pub fn create_followup(
        &self,
        origin_task_id: i64,
        title: &str,
    ) -> Result<(WorkItemRow, bool), IpcError> {
        let source = self
            .get_task(origin_task_id)?
            .and_then(|t| t.work_item_id)
            .map(|id| self.get_work_item(id))
            .transpose()?
            .flatten()
            .ok_or_else(|| {
                IpcError::new(
                    codes::E_INVALID,
                    "that task has no work item to follow up on",
                )
            })?;
        let want = norm(title);
        if let Some((_, id)) = self
            .followups_of(origin_task_id)?
            .into_iter()
            .find(|(t, _)| norm(t) == want)
        {
            let row = self
                .get_work_item(id)?
                .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "follow-up vanished"))?;
            return Ok((row, false));
        }
        let title = clean(title);
        let row = self.insert_manual_item(
            &NativeItem {
                title: &title,
                parent_id: source.parent_id,
                project_id: source.project_id,
                notes: None,
            },
            Some(origin_task_id),
        )?;
        Ok((row, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A task on `item_id` in role `wrapup`, finished `done`.
    fn done_task_on(s: &Store, item_id: i64) -> i64 {
        let t = crate::service::tasks::create_task(s, None, None, "wrap up").unwrap();
        s.set_task_run(t.id, item_id, 1, "wrapup").unwrap();
        s.finish_task(t.id, "done", Some("{}"), None).unwrap();
        t.id
    }

    #[test]
    fn a_follow_up_is_a_sibling_that_names_its_origin_task() {
        let s = Store::open_in_memory().unwrap();
        let epic = s
            .create_native_item(&NativeItem {
                title: "Epic",
                parent_id: None,
                project_id: None,
                notes: None,
            })
            .unwrap();
        let src = s
            .create_native_item(&NativeItem {
                title: "Source",
                parent_id: Some(epic.id),
                project_id: None,
                notes: None,
            })
            .unwrap();
        let task = done_task_on(&s, src.id);
        let (f, new) = s
            .create_followup(task, "  Add integration   tests ")
            .unwrap();
        assert!(new);
        assert_eq!(f.title, "Add integration tests");
        assert_eq!(f.parent_id, Some(epic.id));
        assert_eq!(f.project_id, src.project_id);
        assert_eq!(f.origin_task_id, Some(task));
        assert_eq!(f.origin.as_deref(), Some("manual"));
        assert_ne!(f.id, src.id);
    }

    #[test]
    fn the_same_follow_up_twice_is_one_item() {
        let s = Store::open_in_memory().unwrap();
        let src = s
            .create_native_item(&NativeItem {
                title: "Source",
                parent_id: None,
                project_id: None,
                notes: None,
            })
            .unwrap();
        let task = done_task_on(&s, src.id);
        let (a, _) = s.create_followup(task, "Add tests").unwrap();
        let (b, new) = s.create_followup(task, "add  TESTS").unwrap();
        assert!(!new);
        assert_eq!(a.id, b.id);
        let (c, new) = s.create_followup(task, "Write docs").unwrap();
        assert!(new);
        assert_eq!(
            s.followups_of(task).unwrap(),
            vec![
                ("Add tests".to_string(), a.id),
                ("Write docs".to_string(), c.id)
            ]
        );
    }

    #[test]
    fn a_task_with_no_item_has_no_follow_ups() {
        let s = Store::open_in_memory().unwrap();
        let t = crate::service::tasks::create_task(&s, None, None, "p").unwrap();
        let e = s.create_followup(t.id, "x").unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        let e = s.create_followup(t.id + 1000, "x").unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert!(s.followups_of(t.id).unwrap().is_empty());
    }

    #[test]
    fn a_blank_title_is_refused() {
        let s = Store::open_in_memory().unwrap();
        let src = s
            .create_native_item(&NativeItem {
                title: "Source",
                parent_id: None,
                project_id: None,
                notes: None,
            })
            .unwrap();
        let task = done_task_on(&s, src.id);
        let e = s.create_followup(task, "   ").unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert!(s.followups_of(task).unwrap().is_empty());
    }

    #[test]
    fn an_ordinary_item_has_no_origin_task() {
        let s = Store::open_in_memory().unwrap();
        let src = s
            .create_native_item(&NativeItem {
                title: "Source",
                parent_id: None,
                project_id: None,
                notes: None,
            })
            .unwrap();
        assert_eq!(src.origin_task_id, None);
    }
}
