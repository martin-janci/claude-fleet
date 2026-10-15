//! Archiving a ticket or task (migration 190): `work_link { action:
//! archive_item, item_id, on? }`.
//!
//! A person's tidy-up of their lists, reversible: an archived item leaves
//! the ticket views (`work { tickets }`'s `mine`, `sprint`, `recent` and a
//! tracker filter) for the `archived` view, and the Work tree counts it
//! archived, so the Archived switch hides it — unless a session is active
//! on it. `on: false` unarchives. The flag lives on the hub's
//! `work_items.archived_at`, so every device sees it; a tracker sync never
//! touches it and the ticket itself is not changed in its tracker.
//!
//! Not the sessionless `archive` / `unarchive`, which stamp a task's ENDED
//! links ([`super::ended`]): those archive the sessions' history, this the
//! item. A bare task — a key detected in a prompt, with no item behind it —
//! has nothing else to archive, so `archive_item { task_id: "ref:KEY" }`
//! stamps its ended links through that same path: the Work tree counts a
//! task archived once every link is.

use super::graph::visible_item;
use super::WorkLinkArgs;
use crate::events::WorkChanged;
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::view_scope::ViewScope;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// What `archive_item` answers: the item, or `None` for a bare task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveChange {
    pub item_id: Option<i64>,
    /// Whether anything changed.
    pub changed: bool,
}

/// `work_link { action: archive_item, item_id | task_id, on? }`; `on`
/// defaults to archive. An item the caller cannot see answers as an
/// unknown one. A `task_id` of an item (`item:N`) is that item; of a bare
/// key (`ref:KEY`), its ended links.
pub fn archive_item(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<ArchiveChange, IpcError> {
    let on = args.on.unwrap_or(true);
    let item = match (args.item_id, args.task_id.as_deref()) {
        (Some(i), _) => i,
        (None, Some(task)) => {
            let task = super::view::parse_task_id(task)?;
            match task.strip_prefix("item:") {
                Some(n) => n
                    .parse()
                    .map_err(|_| IpcError::new(codes::E_INVALID, format!("bad task_id {task}")))?,
                None => return archive_bare(&task, on, store, scope),
            }
        }
        (None, None) => {
            return Err(IpcError::new(
                codes::E_INVALID,
                "archive_item needs item_id or task_id",
            ))
        }
    };
    let s = lock(store)?;
    visible_item(&s, scope, item)?;
    let changed = s.set_item_archived(item, on)?;
    if changed {
        // Ids only, kind `work`: a client re-reads the lists it shows.
        s.emit_work_changed(WorkChanged {
            what: "item".into(),
            task_id: Some(format!("item:{item}")),
            rule_id: None,
            view_id: None,
        });
    }
    Ok(ArchiveChange {
        item_id: Some(item),
        changed,
    })
}

/// A bare task's archive: its ended links, through [`super::ended`] (the
/// caller's whole scope, the event, and `E_NOTFOUND` for a task with none).
fn archive_bare(
    task: &str,
    on: bool,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<ArchiveChange, IpcError> {
    let out = super::ended::work_link_ended(
        &WorkLinkArgs {
            action: if on { "archive" } else { "unarchive" }.into(),
            task_id: Some(task.to_string()),
            ..Default::default()
        },
        store,
        scope,
    )?;
    Ok(ArchiveChange {
        item_id: None,
        changed: out.changed > 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::view_scope::ViewScope;

    fn args(item_id: i64, on: Option<bool>) -> WorkLinkArgs {
        WorkLinkArgs {
            action: "archive_item".into(),
            item_id: Some(item_id),
            on,
            ..Default::default()
        }
    }

    #[test]
    fn archive_and_unarchive_round_trip() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = lock(&store)
            .unwrap()
            .create_local_work_item(None, "Tidy the docs")
            .unwrap()
            .id;
        let all = ViewScope::internal();
        let r = archive_item(&args(id, None), &store, &all).unwrap();
        assert!(r.changed);
        let at = lock(&store)
            .unwrap()
            .get_work_item(id)
            .unwrap()
            .unwrap()
            .archived_at;
        assert!(at.is_some(), "archived");
        assert!(
            !archive_item(&args(id, Some(true)), &store, &all)
                .unwrap()
                .changed,
            "an archive keeps its first stamp"
        );
        assert!(
            archive_item(&args(id, Some(false)), &store, &all)
                .unwrap()
                .changed
        );
        let at = lock(&store)
            .unwrap()
            .get_work_item(id)
            .unwrap()
            .unwrap()
            .archived_at;
        assert_eq!(at, None, "unarchived");
    }

    /// A bare task: a key detected in a prompt (`owner/repo#333`), no work
    /// item behind it, its one session gone. The Work view's Archive sends
    /// its `task_id`, and every ended link of the key is stamped: the tree
    /// then counts the task archived.
    #[test]
    fn a_bare_task_is_archived_and_unarchived_by_task_id() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let key = "martin-janci/claude-fleet#333";
        let link = {
            let s = lock(&store).unwrap();
            s.upsert_host("h").unwrap();
            let sid = s
                .upsert_session("gone", "h", None, None, 1, 1, "running", None)
                .unwrap();
            s.link_session_work(sid, crate::store::WorkTarget::Key(key), "manual")
                .unwrap();
            s.delete_session(sid).unwrap();
            s.ended_work_links_for_key(key).unwrap()[0].id
        };
        let all = ViewScope::internal();
        let task = |on| WorkLinkArgs {
            action: "archive_item".into(),
            task_id: Some(format!("ref:{key}")),
            on,
            ..Default::default()
        };
        let archived = |store: &Mutex<Store>| {
            lock(store)
                .unwrap()
                .work_link_archived_at(link)
                .unwrap()
                .is_some()
        };

        let r = archive_item(&task(None), &store, &all).unwrap();
        assert!(r.changed);
        assert_eq!(r.item_id, None, "a bare task has no item");
        assert!(archived(&store), "its ended link is archived");

        let r = archive_item(&task(Some(false)), &store, &all).unwrap();
        assert!(r.changed);
        assert!(!archived(&store), "unarchived");
    }

    #[test]
    fn an_unknown_item_is_not_found_and_item_id_is_required() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let all = ViewScope::internal();
        let e = archive_item(&args(999, None), &store, &all).unwrap_err();
        assert_eq!(e.code, codes::E_NOTFOUND.to_string());
        let e = archive_item(
            &WorkLinkArgs {
                action: "archive_item".into(),
                ..Default::default()
            },
            &store,
            &all,
        )
        .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID.to_string());
    }
}
