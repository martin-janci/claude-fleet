//! `work_link { action: set_status, item_id, status }`: a person's status for
//! work with no ticket (design 2026-09-28 §2), and — where its tracker's
//! admin allows it — a ticket moved at its tracker ([`move_status`], CF-6).
//! `Store::set_item_status` itself still refuses a tracker item, naming the
//! ticket: fleet never stores a ticket's status on its own say. An item
//! outside the caller's scope answers exactly as an unknown id.

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
///
/// One thing a per-host token may not do (design 2026-10-13 D3): set a
/// local item to `done` while its completion gate (`gate::completion_gate`)
/// has not passed. It is refused with `E_INVALID_STATE`, naming the
/// `done_when` lines and open subtasks in the way; a person is never
/// refused.
pub fn set_status(
    store: &Mutex<Store>,
    scope: &OrgScope,
    item_id: i64,
    status: &str,
) -> Result<WorkItemRow, IpcError> {
    let s = lock(store)?;
    let item = require_visible(&s, scope, item_id)?;
    // Design 2026-10-13 D3: an agent does not decide a task is done past its
    // own acceptance lines or open subtasks. A person is never refused.
    if status == "done" && scope.host().is_some() && item.source == "local" {
        let now = crate::service::catalog::now_secs();
        let gate = super::gate::completion_gate(&s, &item, now)?;
        let gate = super::gate::for_scope(gate, &s, scope)?;
        if let Some(why) = super::gate::refusal(&gate) {
            return Err(IpcError::new(crate::ipc_error::codes::E_INVALID_STATE, why));
        }
    }
    s.set_item_status(item_id, status)?
        .ok_or_else(|| orgs::not_found("work item", item_id))
}

/// `work_link { action: keep_open, item_id }`: a person's answer to the Work
/// view's "finished — close?" on local work (CF-5). Not a status — see
/// `Store::keep_item_open` — and fenced exactly as [`set_status`] is.
pub fn keep_open(
    store: &Mutex<Store>,
    scope: &OrgScope,
    item_id: i64,
) -> Result<WorkItemRow, IpcError> {
    let s = lock(store)?;
    require_visible(&s, scope, item_id)?;
    s.keep_item_open(item_id)?
        .ok_or_else(|| orgs::not_found("work item", item_id))
}

/// Whether `scope` may see `item`: a local one through its links
/// (`local::local_item_visible`), a tracker one through its tracker's org.
/// The one fence of [`set_status`] and [`keep_open`], and of the reads that
/// answer about an item's result.
pub(super) fn item_visible(
    s: &Store,
    scope: &OrgScope,
    item: &WorkItemRow,
) -> Result<bool, IpcError> {
    if item.source == "local" {
        super::local::local_item_visible(s, scope, item.id)
    } else {
        Ok(scope.sees_org(s.item_org(item.id)?))
    }
}

/// The fence [`set_status`] documents: an item outside `scope` answers
/// exactly as an unknown id. Answers the item it found.
fn require_visible(s: &Store, scope: &OrgScope, item_id: i64) -> Result<WorkItemRow, IpcError> {
    let Some(item) = s.get_work_item(item_id)? else {
        return Err(orgs::not_found("work item", item_id));
    };
    if !item_visible(s, scope, &item)? {
        return Err(orgs::not_found("work item", item_id));
    }
    Ok(item)
}

/// The longest column name a status move carries.
const STATUS_NAME_MAX_CHARS: usize = 200;

/// `work_link { action: set_status, item_id, status, status_name? }` for any
/// item (owner decision 2026-10-12, CF-6): fleet's own work through
/// [`set_status`], a tracker's ticket by MOVING IT AT ITS TRACKER.
///
/// A ticket's status stays its tracker's: nothing is written to
/// `work_items` here. The move is sent (`WriteOp::SetStatus`: a Jira
/// transition, a Linear state, an Asana section or completion, a GitHub
/// close or reopen), then the ticket is read back and stored as the sync
/// stores it, so the card settles on what the tracker says — which may be a
/// neighbouring status when the workflow has no exact one. `status_name` is
/// the tracker's own name for the column the card was dropped on.
///
/// Sent now, not through the outbox: a move is a person's gesture on a card
/// that has to answer on that card, and one replayed hours later by a
/// backoff would undo whatever was done at the tracker since.
///
/// Refused, with nothing sent:
/// * a per-host token (`E_FORBIDDEN`): only a person moves a ticket, never
///   an agent — as with a comment posted to the tracker;
/// * the tracker's admin has not turned `write_back.status` on
///   (`E_INVALID_STATE`, naming the tracker and where to turn it on);
/// * the tracker has no such move (`E_UNSUPPORTED`, in the provider's words:
///   a workflow with no transition there, a board with no such section).
///
/// `net` is the caller's tracker network (`trackers::default_net()` in
/// production, a fake in tests). The store guard is never held across the
/// provider calls.
pub async fn move_status(
    store: &Mutex<Store>,
    scope: &OrgScope,
    item_id: i64,
    status: &str,
    status_name: Option<&str>,
    net: &crate::service::trackers::TrackerNet,
) -> Result<WorkItemRow, IpcError> {
    use crate::ipc_error::codes;
    use crate::service::trackers::{self, ItemRef, WriteOp};
    let (item, t) = {
        let s = lock(store)?;
        let Some(item) = s.get_work_item(item_id)? else {
            return Err(orgs::not_found("work item", item_id));
        };
        if item.source == "local" {
            drop(s);
            return set_status(store, scope, item_id, status);
        }
        if !scope.sees_org(s.item_org(item_id)?) {
            return Err(orgs::not_found("work item", item_id));
        }
        let tracker = match item.tracker_id {
            Some(id) => s.get_tracker(id)?,
            None => None,
        };
        (item, tracker)
    };
    let which = item.key.clone().unwrap_or_else(|| item.title.clone());
    if !crate::store::STATUS_CATEGORIES.contains(&status) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "a status is one of {} — `blocked` is a session's state, not an item's",
                crate::store::STATUS_CATEGORIES.join(", ")
            ),
        ));
    }
    let name = status_name.map(str::trim).filter(|n| !n.is_empty());
    if name.is_some_and(|n| {
        n.chars().count() > STATUS_NAME_MAX_CHARS || n.chars().any(char::is_control)
    }) {
        return Err(IpcError::new(
            codes::E_INVALID,
            "status_name is a column's name: one line, at most 200 characters",
        ));
    }
    if matches!(scope, OrgScope::Host { .. }) {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!("{which}'s status belongs to its tracker: only a person moves a ticket there"),
        ));
    }
    let Some(t) = t else {
        return Err(IpcError::new(
            codes::E_UNSUPPORTED,
            format!("{which} has no tracker fleet can move it at"),
        ));
    };
    if !trackers::write_back::writes_status(&t) {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "{which}'s status belongs to {}. Moving tickets from fleet is off for it; a \
                 tracker admin turns it on in Settings → Trackers → Move tickets",
                t.name
            ),
        ));
    }
    // Already there: nothing to send.
    if item.status_category == status
        && name.is_none_or(|n| {
            item.status_name
                .as_deref()
                .is_some_and(|c| trackers::same_status_name(c, n))
        })
    {
        return Ok(item);
    }
    let Some(key) = item.key.clone().or_else(|| item.external_id.clone()) else {
        return Err(IpcError::new(
            codes::E_UNSUPPORTED,
            format!("{which} has no key or tracker id to move it by"),
        ));
    };
    let cred = lock(store)?.resolve_tracker_credential(t.id)?;
    if cred.is_none() && trackers::needs_credential(&t) {
        return Err(trackers::TrackerError::Unconfigured.to_ipc());
    }
    let provider = trackers::provider_for(&t, cred, net).map_err(|e| e.to_ipc())?;
    provider
        .write(&WriteOp::SetStatus {
            key: key.clone(),
            external_id: item.external_id.clone(),
            category: status.to_string(),
            name: name.map(str::to_string),
        })
        .await
        .map_err(|e| e.to_ipc())?;
    tracing::info!(
        tracker = t.id,
        item = item_id,
        status,
        "[write-back] ticket moved at its tracker"
    );
    // What the tracker says now, stored as the sync stores it. A read that
    // fails does not undo the move: the next sync brings the status.
    let reference = match item.external_id.as_deref().filter(|e| !e.is_empty()) {
        Some(e) if !matches!(t.provider.as_str(), "jira" | "jira_dc") => ItemRef::Id(e.to_string()),
        _ => ItemRef::parse(&key),
    };
    if let Err(e) = trackers::sync::fetch_one(&t, reference, store, net).await {
        tracing::debug!(
            tracker = t.id,
            item = item_id,
            error = %e.explain(),
            "[write-back] moved, not read back"
        );
    }
    lock(store)?
        .get_work_item(item_id)?
        .ok_or_else(|| orgs::not_found("work item", item_id))
}

/// The status a reader should see (design 2026-09-28 §2): the live
/// precedence over the stored `status_category`.
///
/// Precedence: a person's setting (`status_set_by = "person"`) is final;
/// a stamped `done` (`"derived"`) is not undone by new work; a job's status
/// (`"task"`, an agent subtask mirroring its dispatched job) is final like a
/// stamped `done`; otherwise a
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
///
/// **It normalises, where three callers used to pass the stored string
/// through verbatim.** `to_task`, `brief_of` and `card` reported
/// `work_items.status_category` exactly as stored; routed through here, a
/// value outside the three categories now reports as `todo` instead (see
/// [`normalize`]). Deliberate, and a behaviour change worth knowing about:
/// the three categories are the whole vocabulary
/// (`store::work_status::STATUS_CATEGORIES`), every provider maps onto them
/// (`jira_common.rs`, `linear.rs`, `github.rs`, and Asana's
/// `SECTION_CATEGORIES`), and the wire contract says a reader may switch on
/// three values — so propagating a fourth to a client that cannot render it
/// is worse than answering the one the item is closest to. An EMPTY string
/// is the exception and stays distinguishable, as `None`, below.
///
/// `None` when `status_category` is itself empty (fix round 3): the
/// schema never produces this (`NOT NULL DEFAULT 'todo'`, and every writer
/// — sync, `set_item_status`, `stamp_derived_done` — writes one of the
/// three real values), but a raw empty column must not be reported as
/// `todo`, which would assert something nobody said. This used to be a
/// guard every caller repeated outside the function (`view.rs`'s
/// `item_status`, `card.rs`, `handover.rs`) — moved inside so there is one
/// place to get it right instead of three to keep in sync, and so
/// `crate::effective_status_sql!`'s `WHEN i.status_category = '' THEN
/// NULL` branch has one Rust behaviour to match, not three call sites'
/// worth of copies.
pub fn effective_status(
    status_category: &str,
    status_set_by: Option<&str>,
    source: &str,
    has_working_session: bool,
) -> Option<&'static str> {
    if status_category.is_empty() {
        return None;
    }
    Some(match status_set_by {
        Some("person") | Some("derived") | Some("task") => normalize(status_category),
        _ if has_working_session && source == "local" => "in_progress",
        _ => normalize(status_category),
    })
}

/// `status_category` keeps exactly three values
/// (`store::work_status::STATUS_CATEGORIES`); anything else — which the
/// schema and every provider mapping say cannot happen — reads as `todo`
/// rather than propagating a string a reader cannot switch on. A COERCION,
/// not a pass-through: see [`effective_status`]'s doc for which callers it
/// changed and why that is the answer wanted here. An empty string never
/// reaches this function; [`effective_status`] answers `None` for it, so
/// "nothing said" stays distinguishable from "to do".
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
            Some("todo")
        );
        assert_eq!(
            effective_status("done", Some("person"), "local", true),
            Some("done")
        );
    }

    #[test]
    fn a_stamped_done_is_not_undone_by_new_work() {
        assert_eq!(
            effective_status("done", Some("derived"), "local", true),
            Some("done"),
            "narrowing status_set_by's IN ('person','derived') to only 'person' \
             would read this back as in_progress instead"
        );
    }

    #[test]
    fn a_jobs_status_is_final_over_the_live_lift() {
        assert_eq!(
            effective_status("todo", Some("task"), "local", true),
            Some("todo")
        );
        assert_eq!(
            effective_status("done", Some("task"), "local", true),
            Some("done")
        );
    }

    #[test]
    fn a_working_session_makes_a_local_item_in_progress() {
        assert_eq!(
            effective_status("todo", None, "local", true),
            Some("in_progress")
        );
    }

    #[test]
    fn a_working_session_never_lifts_a_tracker_item() {
        // The sync's value, untouched by the live signal (§2, "who may be
        // overridden": a tracker item's column is its tracker's).
        assert_eq!(effective_status("todo", None, "jira", true), Some("todo"));
    }

    #[test]
    fn otherwise_it_is_whatever_is_stored() {
        assert_eq!(effective_status("todo", None, "local", false), Some("todo"));
        assert_eq!(
            effective_status("in_progress", None, "jira", false),
            Some("in_progress")
        );
    }

    /// The coercion is deliberate (final review, item 7): three callers used
    /// to report the stored string verbatim, and now report `todo` for a
    /// value outside the vocabulary. Pinned so it is a decision, not a
    /// side effect — and so that restoring a pass-through has to say so
    /// here first.
    #[test]
    fn a_status_outside_the_vocabulary_reads_as_todo() {
        for stored in ["blocked", "in review", "DONE", "Done"] {
            assert_eq!(
                effective_status(stored, None, "jira", false),
                Some("todo"),
                "{stored:?} must not reach a reader that can only switch on the three"
            );
        }
        // Including under a person's or the stamp's arm, which share it.
        assert_eq!(
            effective_status("blocked", Some("person"), "local", false),
            Some("todo")
        );
    }

    #[test]
    fn an_empty_stored_status_reports_nothing() {
        // The schema never produces this, but the function must not
        // manufacture a `todo` nobody said — checked before anything else,
        // even a person's setting (which the schema also never leaves
        // paired with an empty category).
        assert_eq!(effective_status("", None, "local", false), None);
        assert_eq!(effective_status("", None, "local", true), None);
        assert_eq!(effective_status("", Some("person"), "local", true), None);
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

    fn open_subtask(w: &Fx, title: &str) -> i64 {
        w.store
            .lock()
            .unwrap()
            .create_native_item(&crate::store::NativeItem {
                title,
                parent_id: Some(w.item),
                ..Default::default()
            })
            .unwrap()
            .id
    }

    fn stored_status(w: &Fx) -> String {
        w.store
            .lock()
            .unwrap()
            .get_work_item(w.item)
            .unwrap()
            .unwrap()
            .status_category
    }

    #[test]
    fn an_agent_may_not_set_done_past_an_open_subtask_a_person_may() {
        let w = seeded_local_item_on_host("mercury");
        let child = open_subtask(&w, "Child");
        let e = super::set_status(&w.store, &w.host_scope(), w.item, "done").unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_INVALID_STATE);
        assert!(
            e.message.contains("subtask \"Child\" is open"),
            "{}",
            e.message
        );
        assert_eq!(stored_status(&w), "todo", "a refusal writes nothing");
        // Only `done` is gated: the agent still says it is working on it.
        let row = super::set_status(&w.store, &w.host_scope(), w.item, "in_progress").unwrap();
        assert_eq!(row.status_category, "in_progress");
        // Once the subtask is closed, the same call goes through.
        w.store
            .lock()
            .unwrap()
            .set_item_status(child, "done")
            .unwrap();
        let row = super::set_status(&w.store, &w.host_scope(), w.item, "done").unwrap();
        assert_eq!(row.status_category, "done");
    }

    #[test]
    fn a_person_is_never_refused_by_the_gate() {
        let w = seeded_local_item_on_host("mercury");
        open_subtask(&w, "Child");
        let row = super::set_status(&w.store, &OrgScope::All, w.item, "done").unwrap();
        assert_eq!(row.status_category, "done");
    }

    #[test]
    fn an_agent_may_not_set_done_past_an_unchecked_line_until_a_person_checks_it() {
        let w = seeded_local_item_on_host("mercury");
        let line = "the docs say how to turn it on";
        w.store
            .lock()
            .unwrap()
            .set_item_done_when(w.item, &[line.to_string()], "person:1")
            .unwrap();
        let e = super::set_status(&w.store, &w.host_scope(), w.item, "done").unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_INVALID_STATE);
        assert!(e.message.contains(line), "{}", e.message);
        assert!(e.message.contains("is pending"), "{}", e.message);
        assert_eq!(stored_status(&w), "todo");

        w.store
            .lock()
            .unwrap()
            .record_verification(w.item, line, true, "person:1", None)
            .unwrap();
        let row = super::set_status(&w.store, &w.host_scope(), w.item, "done").unwrap();
        assert_eq!(row.status_category, "done");
    }
}

/// [`move_status`] for a tracker's ticket (CF-6): the gates, the write, and
/// the card settling on what the tracker then says. No test reaches a
/// tracker: the network is a [`FakeTransport`].
#[cfg(test)]
mod move_tests {
    use super::move_status;
    use crate::ipc_error::codes;
    use crate::net::https::{FakeTransport, Method, Response};
    use crate::service::orgs::OrgScope;
    use crate::service::trackers::TrackerNet;
    use crate::store::{Store, TrackerConfig, TrackerItemWrite, TrackerSettings, WriteBack};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    struct W {
        store: Mutex<Store>,
        fake: FakeTransport,
        item: i64,
    }

    impl W {
        fn net(&self) -> TrackerNet {
            TrackerNet::fake(Arc::new(self.fake.clone()))
        }

        fn stored(&self) -> (String, Option<String>) {
            let i = self
                .store
                .lock()
                .unwrap()
                .get_work_item(self.item)
                .unwrap()
                .unwrap();
            (i.status_category, i.status_name)
        }
    }

    /// A Jira tracker of org "Mine" holding ABC-1, In Progress; status
    /// write-back on when `on`.
    fn jira(on: bool) -> W {
        let s = Store::open_in_memory().unwrap();
        let t = s
            .add_tracker("jira", "Acme Jira", "https://acme.atlassian.net")
            .unwrap()
            .id;
        s.set_tracker_credential(
            t,
            "basic",
            Some("me@x.com"),
            Some("tok-0123456789abc"),
            None,
        )
        .unwrap();
        s.set_tracker_probe(
            t,
            None,
            &TrackerConfig {
                key_prefixes: vec!["ABC".into()],
                ..Default::default()
            },
        )
        .unwrap();
        s.set_tracker_state(t, "ok", None).unwrap();
        if on {
            s.set_tracker_settings(
                t,
                &TrackerSettings {
                    write_back: WriteBack {
                        status: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let item = s
            .upsert_tracker_item(
                t,
                &TrackerItemWrite {
                    external_id: "10001".into(),
                    key: Some("ABC-1".into()),
                    title: "Refund".into(),
                    status_name: "In Progress".into(),
                    status_category: "in_progress".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .id;
        W {
            store: Mutex::new(s),
            fake: FakeTransport::new(),
            item,
        }
    }

    /// Jira's side of a move to Done: the workflow, the transition, and the
    /// issue as it reads afterwards.
    fn jira_moves_to_done(w: &W) {
        w.fake.always(
            Method::Get,
            "/rest/api/3/issue/ABC-1/transitions",
            Ok(Response::json(
                200,
                &json!({ "transitions": [{ "id": "31", "name": "Resolve",
                    "to": { "name": "Done", "statusCategory": { "key": "done" } } }] }),
            )),
        );
        w.fake.always(
            Method::Post,
            "/rest/api/3/issue/ABC-1/transitions",
            Ok(Response::new(204, "")),
        );
        w.fake.always(
            Method::Post,
            "/rest/api/3/issue/bulkfetch",
            Ok(Response::json(
                200,
                &json!({ "issues": [{ "id": "10001", "key": "ABC-1", "fields": {
                    "summary": "Refund",
                    "status": { "name": "Done", "statusCategory": { "key": "done" } },
                } }] }),
            )),
        );
    }

    #[tokio::test]
    async fn a_ticket_is_moved_at_its_tracker_and_settles_on_its_answer() {
        let w = jira(true);
        jira_moves_to_done(&w);
        let row = move_status(&w.store, &OrgScope::All, w.item, "done", None, &w.net())
            .await
            .unwrap();
        assert_eq!(row.status_category, "done");
        assert_eq!(row.status_name.as_deref(), Some("Done"));
        // Stored by the read-back, as a sync stores it: never as a person's
        // own setting, which a sync would then be unable to move.
        assert_eq!(w.stored(), ("done".to_string(), Some("Done".to_string())));
        assert_eq!(row.status_set_by, None);
        let posts: Vec<_> = w
            .fake
            .requests()
            .into_iter()
            .filter(|r| r.url.ends_with("/transitions") && r.method == Method::Post)
            .collect();
        assert_eq!(posts.len(), 1);
        assert_eq!(
            posts[0].json_body().unwrap(),
            json!({ "transition": { "id": "31" } })
        );
    }

    #[tokio::test]
    async fn a_move_that_is_not_read_back_still_succeeds() {
        let w = jira(true);
        jira_moves_to_done(&w);
        w.fake.clear_routes();
        w.fake.always(
            Method::Get,
            "/transitions",
            Ok(Response::json(
                200,
                &json!({ "transitions": [{ "id": "31",
                    "to": { "name": "Done", "statusCategory": { "key": "done" } } }] }),
            )),
        );
        w.fake
            .always(Method::Post, "/transitions", Ok(Response::new(204, "")));
        // No bulkfetch route: the read-back fails as an unreachable tracker.
        let row = move_status(&w.store, &OrgScope::All, w.item, "done", None, &w.net())
            .await
            .unwrap();
        // The next sync brings the status; nothing was invented here.
        assert_eq!(row.status_category, "in_progress");
    }

    #[tokio::test]
    async fn off_by_default_it_is_refused_naming_the_tracker_and_nothing_is_sent() {
        let w = jira(false);
        let e = move_status(&w.store, &OrgScope::All, w.item, "done", None, &w.net())
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID_STATE);
        assert!(
            e.message.contains("ABC-1's status belongs to Acme Jira"),
            "{}",
            e.message
        );
        assert!(e.message.contains("Move tickets"), "{}", e.message);
        assert!(w.fake.requests().is_empty());
        assert_eq!(w.stored().0, "in_progress");
    }

    #[tokio::test]
    async fn an_agent_never_moves_a_ticket_and_another_org_does_not_see_it() {
        let w = jira(true);
        jira_moves_to_done(&w);
        let (mine, theirs) = {
            let s = w.store.lock().unwrap();
            let mine = s.add_org("Mine", None, false).unwrap().id;
            let theirs = s.add_org("Theirs", None, false).unwrap().id;
            let t = s.list_trackers().unwrap()[0].id;
            s.set_tracker_org(t, Some(mine)).unwrap();
            for (host, org) in [("hosta", mine), ("hostb", theirs)] {
                s.upsert_host(host).unwrap();
                s.set_host_org(host, Some(org)).unwrap();
            }
            (
                OrgScope::for_host(&s, "hosta").unwrap(),
                OrgScope::for_host(&s, "hostb").unwrap(),
            )
        };
        // The host's own Claude, inside the org: it sees the ticket, and is
        // told a person moves it.
        let e = move_status(&w.store, &mine, w.item, "done", None, &w.net())
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_FORBIDDEN);
        assert!(e.message.contains("only a person"), "{}", e.message);
        // Another org: exactly an unknown id.
        let e = move_status(&w.store, &theirs, w.item, "done", None, &w.net())
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_NOTFOUND);
        assert!(w.fake.requests().is_empty());
    }

    #[tokio::test]
    async fn a_workflow_with_no_such_move_is_said_on_the_card() {
        let w = jira(true);
        w.fake.always(
            Method::Get,
            "/transitions",
            Ok(Response::json(
                200,
                &json!({ "transitions": [{ "id": "21",
                    "to": { "name": "In Review", "statusCategory": { "key": "indeterminate" } } }] }),
            )),
        );
        let e = move_status(&w.store, &OrgScope::All, w.item, "done", None, &w.net())
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_UNSUPPORTED);
        assert!(
            e.message.contains("no move to Done") && e.message.contains("In Review"),
            "{}",
            e.message
        );
        assert_eq!(w.stored().0, "in_progress");
    }

    #[tokio::test]
    async fn a_card_already_there_sends_nothing_and_a_bad_request_is_refused_first() {
        let w = jira(true);
        let row = move_status(
            &w.store,
            &OrgScope::All,
            w.item,
            "in_progress",
            Some("in  progress"),
            &w.net(),
        )
        .await
        .unwrap();
        assert_eq!(row.status_name.as_deref(), Some("In Progress"));
        let bad: [(&str, Option<String>); 3] = [
            ("blocked", None),
            ("done", Some("two\nlines".into())),
            ("done", Some("x".repeat(201))),
        ];
        for (status, name) in bad {
            let e = move_status(
                &w.store,
                &OrgScope::All,
                w.item,
                status,
                name.as_deref(),
                &w.net(),
            )
            .await
            .unwrap_err();
            assert_eq!(e.code, codes::E_INVALID, "{status} {name:?}");
        }
        assert!(w.fake.requests().is_empty());
    }

    #[tokio::test]
    async fn fleets_own_work_is_still_set_here_with_no_tracker_asked() {
        let w = jira(true);
        let local = {
            let s = w.store.lock().unwrap();
            s.upsert_host("hosta").unwrap();
            let sid = s
                .upsert_session("s", "hosta", None, None, 1, 1, "running", None)
                .unwrap();
            s.name_session_work(sid, None, "local work").unwrap().0.id
        };
        let row = move_status(
            &w.store,
            &OrgScope::All,
            local,
            "done",
            Some("Done"),
            &w.net(),
        )
        .await
        .unwrap();
        assert_eq!(row.status_category, "done");
        assert_eq!(row.status_set_by.as_deref(), Some("person"));
        assert!(w.fake.requests().is_empty());
    }
}
