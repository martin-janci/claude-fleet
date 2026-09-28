//! `work_link { action: set_status, item_id, status }`: a person's status for
//! work with no ticket (design 2026-09-28 §2). A tracker item is refused —
//! `Store::set_item_status` names the ticket — and an item outside the
//! caller's scope answers exactly as an unknown id.

use crate::ipc_error::{lock, IpcError};
use crate::service::orgs::{self, OrgScope};
use crate::store::{Store, WorkItemRow};
use std::sync::Mutex;

/// `work_link { action: set_status, item_id, status }`: a person's status
/// for a local item its scope may reach. A per-host token may set its own
/// item's status (it says nothing about buckets or rules — that stays a
/// later phase); an item outside its scope answers exactly as an unknown
/// id, never revealing another org's work.
///
/// The fence is the one `service::work::local` already applies: a tracker
/// item through `Store::item_org` + `OrgScope::sees_org` (as
/// `local::rename_local_item` checks it), a local item through
/// `local::local_item_visible` (its links). Once visible, `store` decides
/// the rest: the three-category check, and the tracker refusal that names
/// the ticket.
pub fn set_status(
    store: &Mutex<Store>,
    scope: &OrgScope,
    item_id: i64,
    status: &str,
) -> Result<WorkItemRow, IpcError> {
    let s = lock(store)?;
    let Some(item) = s.get_work_item(item_id)? else {
        return Err(orgs::not_found("work item", item_id));
    };
    let visible = if item.source == "local" {
        super::local::local_item_visible(&s, scope, item_id)?
    } else {
        scope.sees_org(s.item_org(item_id)?)
    };
    if !visible {
        return Err(orgs::not_found("work item", item_id));
    }
    s.set_item_status(item_id, status)?
        .ok_or_else(|| orgs::not_found("work item", item_id))
}

#[cfg(test)]
mod tests {
    use crate::service::orgs::OrgScope;
    use crate::store::Store;
    use std::sync::Mutex;

    struct Fx {
        store: Mutex<Store>,
        item: i64,
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
    }

    /// A local item named on a session of `host`, in its own org — a second
    /// host in a different org sees neither the host nor the item (the
    /// fence `service::work::local` already applies).
    fn seeded_local_item_on_host(host: &str) -> Fx {
        let other_host = "the-other-host";
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
            host: host.to_string(),
            other_host: other_host.to_string(),
        }
    }

    #[test]
    fn a_host_token_may_set_its_own_items_status() {
        let w = seeded_local_item_on_host("mercury");
        let row = super::set_status(&w.store, &w.host_scope(), w.item, "done").unwrap();
        assert_eq!(row.status_category, "done");
    }

    #[test]
    fn an_item_outside_the_scope_answers_as_unknown() {
        let w = seeded_local_item_on_host("mercury");
        let e = super::set_status(&w.store, &w.other_org_scope(), w.item, "done").unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_NOTFOUND);
    }
}
