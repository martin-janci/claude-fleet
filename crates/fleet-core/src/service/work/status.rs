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

/// The status a reader should see (design 2026-09-28 §2): the live
/// precedence over the stored `status_category`.
///
/// Precedence: a person's setting (`status_set_by = "person"`) is final;
/// a stamped `done` (`"derived"`) is not undone by new work; otherwise a
/// confirmed link whose session is presently working lifts a **local**
/// item to `in_progress` — a tracker item's column is its tracker's (§2,
/// "who may be overridden"), so a working session must never move it;
/// otherwise the stored value.
///
/// Takes fields, not a `WorkItemRow`: `WorkItemRow` does not derive
/// `Default`, so a row fixture cannot be built with `..Default::default()`
/// as the design sketch assumed, and adding `Default` to a 25-field read
/// row would manufacture a valid-looking empty row that can mask bugs
/// elsewhere. The signature is exactly this function's dependencies
/// instead, so a caller passes the three fields and a test needs no
/// fixture at all.
///
/// `has_working_session` must come from ONE join over the whole page
/// (`Store::work_items_with_working_session`) — never a query per row.
pub fn effective_status(
    status_category: &str,
    status_set_by: Option<&str>,
    source: &str,
    has_working_session: bool,
) -> &'static str {
    match status_set_by {
        Some("person") | Some("derived") => normalize(status_category),
        _ if has_working_session && source == "local" => "in_progress",
        _ => normalize(status_category),
    }
}

/// `status_category` keeps exactly three values; anything else (there
/// should be nothing else) reads as `todo` rather than propagating an
/// unrecognised string.
fn normalize(status_category: &str) -> &'static str {
    match status_category {
        "done" => "done",
        "in_progress" => "in_progress",
        _ => "todo",
    }
}

#[cfg(test)]
mod effective_status_tests {
    use super::effective_status;

    #[test]
    fn a_person_outranks_everything() {
        assert_eq!(
            effective_status("todo", Some("person"), "local", true),
            "todo"
        );
        assert_eq!(
            effective_status("done", Some("person"), "local", true),
            "done"
        );
    }

    #[test]
    fn a_stamped_done_is_not_undone_by_new_work() {
        assert_eq!(
            effective_status("done", Some("derived"), "local", true),
            "done"
        );
    }

    #[test]
    fn a_working_session_makes_a_local_item_in_progress() {
        assert_eq!(effective_status("todo", None, "local", true), "in_progress");
    }

    #[test]
    fn a_working_session_never_lifts_a_tracker_item() {
        // The sync's value, untouched by the live signal (§2, "who may be
        // overridden": a tracker item's column is its tracker's).
        assert_eq!(effective_status("todo", None, "jira", true), "todo");
    }

    #[test]
    fn otherwise_it_is_whatever_is_stored() {
        assert_eq!(effective_status("todo", None, "local", false), "todo");
        assert_eq!(
            effective_status("in_progress", None, "jira", false),
            "in_progress"
        );
    }
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
