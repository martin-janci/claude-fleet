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
//! item.

use super::graph::{visible_item, GraphChange};
use super::WorkLinkArgs;
use crate::events::WorkChanged;
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::view_scope::ViewScope;
use crate::store::Store;
use std::sync::Mutex;

/// `work_link { action: archive_item, item_id, on? }`; `on` defaults to
/// archive. An item the caller cannot see answers as an unknown one.
pub fn archive_item(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<GraphChange, IpcError> {
    let item = args
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "archive_item needs item_id"))?;
    let s = lock(store)?;
    visible_item(&s, scope, item)?;
    let changed = s.set_item_archived(item, args.on.unwrap_or(true))?;
    if changed {
        // Ids only, kind `work`: a client re-reads the lists it shows.
        s.emit_work_changed(WorkChanged {
            what: "item".into(),
            task_id: Some(format!("item:{item}")),
            rule_id: None,
            view_id: None,
        });
    }
    Ok(GraphChange {
        item_id: item,
        changed,
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
