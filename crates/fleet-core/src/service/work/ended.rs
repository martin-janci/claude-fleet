//! TASK-263: a work link whose session is gone — an ENDED link (`ended_at`
//! set, the session's snapshot kept, no session row left) — archived,
//! un-archived or removed by the control API, which has no `session_id` to
//! name because such a link no longer has one.
//!
//! `work_link { action: "archive" | "unarchive", link_id | key | item_id |
//! task_id }` stamps or clears `archived_at` on the target's ended confirmed
//! links, so a task that lives only on ended links collapses into Done
//! (`view::all_links_archived`). `work_link { action: "unlink", link_id }`
//! deletes the one ended link, snapshot and all — a detection that was wrong.
//!
//! Visibility is the one every READ of ended links takes
//! ([`orgs::scope_links_for`]: org, host, person): a link the caller may not
//! see, a live one, or none at all answers as a link that does not exist,
//! the way `resume` and `summarize` do.

use super::WorkLinkArgs;
use crate::events::WorkChanged;
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs;
use crate::service::view_scope::ViewScope;
use crate::store::{Store, WorkLinkRow};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// One ended link, as the answer spells it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndedLinkRef {
    pub link_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snap_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snap_name: Option<String>,
    pub ended_at: i64,
    /// After the write (`None` for a removed link).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<i64>,
}

/// What a sessionless `work_link` did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndedLinksChange {
    /// `archived` | `unarchived` | `removed`.
    pub action: String,
    /// The links the action addressed, as they are after it.
    pub links: Vec<EndedLinkRef>,
    /// Rows the write changed: an already-archived link counts for nothing.
    pub changed: usize,
}

/// The sessionless form: no `session_id`, and one of the three actions that
/// have one.
pub fn is_sessionless(args: &WorkLinkArgs) -> bool {
    args.session_id.is_none() && matches!(args.action.as_str(), "archive" | "unarchive" | "unlink")
}

/// The one sessionless form that deletes: `unlink`. Named here so the tool
/// layer's refusal to a per-host token reads the action through this module
/// and not through a literal of its own.
pub fn is_removal(args: &WorkLinkArgs) -> bool {
    is_sessionless(args) && args.action == "unlink"
}

/// `E_INVALID` for an action that was given no `session_id` and has no
/// sessionless form. A `reject` is a live session's sticky "not this", so
/// its message names the two forms a link whose session is gone does take,
/// and the caller is not left guessing.
pub fn needs_session(action: &str) -> IpcError {
    if action == "reject" {
        return reject_needs_session();
    }
    IpcError::new(codes::E_INVALID, format!("{action} needs session_id"))
}

/// See [`needs_session`].
pub fn reject_needs_session() -> IpcError {
    IpcError::new(
        codes::E_INVALID,
        "reject needs session_id (a live session's sticky 'not this'); a link whose \
         session is gone is hidden with archive { link_id | key | item_id } or removed \
         with unlink { link_id }, both without session_id",
    )
}

/// `E_INVALID` for a sessionless `unlink` that names no link.
fn unlink_needs_link_id() -> IpcError {
    IpcError::new(
        codes::E_INVALID,
        "unlink without session_id removes ONE ended link: pass link_id (archive takes a \
         key or item_id too)",
    )
}

/// The ended confirmed links the call names, after the caller's whole scope.
/// Returns the words for the not-found answer with them.
fn target_links(
    s: &Store,
    args: &WorkLinkArgs,
    view: &ViewScope,
) -> Result<Vec<WorkLinkRow>, IpcError> {
    let (what, mut links): (String, Vec<WorkLinkRow>) = if let Some(id) = args.link_id {
        let what = match args.key.as_deref() {
            Some(k) => format!(
                "{} has no ended work link {id}",
                crate::store::normalize_work_ref(k)?
            ),
            None => format!("no ended work link {id}"),
        };
        let found = s
            .get_work_link(id)?
            .filter(|l| l.ended_at.is_some() && l.state == "confirmed");
        (what, found.into_iter().collect())
    } else if let Some(item) = args.item_id {
        (
            format!("work item {item} has no ended work links"),
            s.ended_work_links_for_item(item)?,
        )
    } else if let Some(key) = args.key.as_deref() {
        let key = crate::store::normalize_work_ref(key)?;
        (
            format!("{key} has no ended work links"),
            s.ended_work_links_for_key(&key)?,
        )
    } else if let Some(task) = args.task_id.as_deref() {
        let task = super::view::parse_task_id(task)?;
        let links = match task.strip_prefix("item:") {
            Some(n) => s.ended_work_links_for_item(n.parse().unwrap_or_default())?,
            None => s.ended_work_links_for_key(task.trim_start_matches("ref:"))?,
        };
        (format!("task {task} has no ended work links"), links)
    } else {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "{} needs session_id (a live session), or link_id | key | item_id | task_id \
                 of work whose sessions are gone",
                args.action
            ),
        ));
    };
    orgs::scope_links_for(s, view, &mut links)?;
    if links.is_empty() {
        // One answer for a link that is not ended, not visible, or not
        // there: none of the three confirms the other two.
        return Err(IpcError::new(codes::E_NOTFOUND, what));
    }
    Ok(links)
}

/// Apply a sessionless `archive`, `unarchive` or `unlink` to ended links.
pub fn work_link_ended(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    view: &ViewScope,
) -> Result<EndedLinksChange, IpcError> {
    let action = args.action.as_str();
    if !is_sessionless(args) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("{action} needs session_id"),
        ));
    }
    if action == "unlink" && args.link_id.is_none() {
        return Err(unlink_needs_link_id());
    }
    let s = lock(store)?;
    let links = target_links(&s, args, view)?;
    let ids: Vec<i64> = links.iter().map(|l| l.id).collect();
    let (name, changed) = match action {
        "archive" => ("archived", s.archive_ended_links(&ids, true)?),
        "unarchive" => ("unarchived", s.archive_ended_links(&ids, false)?),
        _ => ("removed", s.remove_ended_links(&ids)?),
    };
    let mut out = Vec::with_capacity(links.len());
    for l in &links {
        let key = match (&l.ref_key, l.item_id) {
            (Some(k), _) => Some(k.clone()),
            (None, Some(i)) => s.get_work_item(i)?.and_then(|i| i.key),
            (None, None) => None,
        };
        out.push(EndedLinkRef {
            link_id: l.id,
            key,
            item_id: l.item_id,
            snap_host: l.snap_host.clone(),
            snap_name: l.snap_name.clone(),
            ended_at: l.ended_at.unwrap_or_default(),
            archived_at: if name == "removed" {
                None
            } else {
                s.work_link_archived_at(l.id)?
            },
        });
    }
    if changed > 0 {
        // Ids only, kind `work`: a client re-reads the tree it shows
        // (work graph M14.1d) — there is no session row to emit.
        let task_id = match (out[0].item_id, out[0].key.as_deref()) {
            (Some(i), _) => Some(format!("item:{i}")),
            (None, Some(k)) => Some(format!("ref:{k}")),
            (None, None) => None,
        };
        s.emit_work_changed(WorkChanged {
            what: "link".into(),
            task_id,
            rule_id: None,
            view_id: None,
        });
    }
    Ok(EndedLinksChange {
        action: name.into(),
        links: out,
        changed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::WorkTarget;

    fn store() -> Mutex<Store> {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        Mutex::new(s)
    }

    /// A session on `key` that has ended: one ended confirmed link.
    fn ended(st: &Mutex<Store>, name: &str, key: &str) -> i64 {
        let s = st.lock().unwrap();
        let sid = s
            .upsert_session(name, "h", None, None, 1, 1, "running", None)
            .unwrap();
        s.link_session_work(sid, WorkTarget::Key(key), "manual")
            .unwrap();
        s.delete_session(sid).unwrap();
        s.ended_work_links_for_key(key)
            .unwrap()
            .into_iter()
            .find(|l| l.snap_tmux.as_deref() == Some(name))
            .expect("the link ended with its session")
            .id
    }

    fn args(action: &str) -> WorkLinkArgs {
        WorkLinkArgs {
            action: action.into(),
            ..Default::default()
        }
    }

    fn archived_at(st: &Mutex<Store>, id: i64) -> Option<i64> {
        st.lock().unwrap().work_link_archived_at(id).unwrap()
    }

    #[test]
    fn an_ended_link_is_archived_and_unarchived_by_id_without_a_session() {
        let st = store();
        let id = ended(&st, "gone", "ABC-1");
        let view = ViewScope::internal();

        let out = work_link_ended(
            &WorkLinkArgs {
                link_id: Some(id),
                ..args("archive")
            },
            &st,
            &view,
        )
        .unwrap();
        assert_eq!((out.action.as_str(), out.changed), ("archived", 1));
        assert_eq!(out.links.len(), 1);
        let l = &out.links[0];
        assert_eq!((l.link_id, l.key.as_deref()), (id, Some("ABC-1")));
        assert_eq!(l.snap_host.as_deref(), Some("h"));
        assert!(l.archived_at.is_some(), "the answer carries the stamp");
        assert_eq!(archived_at(&st, id), l.archived_at);

        // Idempotent: the second archive changes nothing and keeps the stamp.
        let again = work_link_ended(
            &WorkLinkArgs {
                link_id: Some(id),
                ..args("archive")
            },
            &st,
            &view,
        )
        .unwrap();
        assert_eq!(again.changed, 0);
        assert_eq!(again.links[0].archived_at, l.archived_at);

        let back = work_link_ended(
            &WorkLinkArgs {
                link_id: Some(id),
                ..args("unarchive")
            },
            &st,
            &view,
        )
        .unwrap();
        assert_eq!((back.action.as_str(), back.changed), ("unarchived", 1));
        assert_eq!(back.links[0].archived_at, None);
        assert_eq!(archived_at(&st, id), None);
    }

    #[test]
    fn a_key_or_task_id_archives_every_ended_link_of_the_work() {
        let st = store();
        let a = ended(&st, "first", "ABC-1");
        let b = ended(&st, "second", "ABC-1");
        let other = ended(&st, "third", "XYZ-9");
        let view = ViewScope::internal();

        let out = work_link_ended(
            &WorkLinkArgs {
                key: Some("abc-1".into()),
                ..args("archive")
            },
            &st,
            &view,
        )
        .unwrap();
        assert_eq!(out.changed, 2);
        let mut ids: Vec<i64> = out.links.iter().map(|l| l.link_id).collect();
        ids.sort();
        assert_eq!(ids, vec![a.min(b), a.max(b)]);
        assert_eq!(
            archived_at(&st, other),
            None,
            "another key's link is untouched"
        );

        let out = work_link_ended(
            &WorkLinkArgs {
                task_id: Some("ref:xyz-9".into()),
                ..args("archive")
            },
            &st,
            &view,
        )
        .unwrap();
        assert_eq!(out.changed, 1);
        assert_eq!(out.links[0].link_id, other);
    }

    #[test]
    fn unlink_removes_one_ended_link_and_needs_its_id() {
        let st = store();
        let a = ended(&st, "first", "ABC-1");
        let b = ended(&st, "second", "ABC-1");
        let view = ViewScope::internal();

        let err = work_link_ended(
            &WorkLinkArgs {
                key: Some("ABC-1".into()),
                ..args("unlink")
            },
            &st,
            &view,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(err.message.contains("pass link_id"), "{}", err.message);

        let out = work_link_ended(
            &WorkLinkArgs {
                link_id: Some(a),
                key: Some("ABC-1".into()),
                ..args("unlink")
            },
            &st,
            &view,
        )
        .unwrap();
        assert_eq!((out.action.as_str(), out.changed), ("removed", 1));
        assert_eq!(out.links[0].link_id, a);
        assert_eq!(out.links[0].archived_at, None);
        let left: Vec<i64> = st
            .lock()
            .unwrap()
            .ended_work_links_for_key("ABC-1")
            .unwrap()
            .iter()
            .map(|l| l.id)
            .collect();
        assert_eq!(left, vec![b], "the other session's past stays");

        // Gone is gone: the same words as a link that never was.
        let err = work_link_ended(
            &WorkLinkArgs {
                link_id: Some(a),
                key: Some("ABC-1".into()),
                ..args("unlink")
            },
            &st,
            &view,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        assert_eq!(err.message, format!("ABC-1 has no ended work link {a}"));
    }

    #[test]
    fn a_live_link_an_unknown_one_and_an_unaddressed_call_are_refused() {
        let st = store();
        let view = ViewScope::internal();
        let live = {
            let s = st.lock().unwrap();
            let sid = s
                .upsert_session("live", "h", None, None, 1, 1, "running", None)
                .unwrap();
            s.link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
                .unwrap();
            s.session_work_links(sid).unwrap()[0].id
        };
        // A live link belongs to its session: it is archived through
        // `work_link { session_id, archive }`, never here.
        let err = work_link_ended(
            &WorkLinkArgs {
                link_id: Some(live),
                ..args("archive")
            },
            &st,
            &view,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        assert_eq!(err.message, format!("no ended work link {live}"));
        assert_eq!(archived_at(&st, live), None);
        let err = work_link_ended(
            &WorkLinkArgs {
                link_id: Some(live),
                ..args("unlink")
            },
            &st,
            &view,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        assert!(st.lock().unwrap().get_work_link(live).unwrap().is_some());

        let err = work_link_ended(
            &WorkLinkArgs {
                key: Some("ABC-1".into()),
                ..args("archive")
            },
            &st,
            &view,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        assert_eq!(err.message, "ABC-1 has no ended work links");

        let err = work_link_ended(&args("archive"), &st, &view).unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(
            err.message.starts_with("archive needs session_id"),
            "{}",
            err.message
        );

        let err = work_link_ended(
            &WorkLinkArgs {
                session_id: Some(1),
                link_id: Some(live),
                ..args("archive")
            },
            &st,
            &view,
        )
        .unwrap_err();
        assert_eq!(
            err.code,
            codes::E_INVALID,
            "a session's archive is not this path"
        );
        let err = needs_session("reject");
        assert_eq!(err.code, codes::E_INVALID);
        assert!(err.message.contains("unlink { link_id }"));
        assert_eq!(needs_session("link").message, "link needs session_id");
        assert!(is_removal(&WorkLinkArgs {
            link_id: Some(1),
            ..args("unlink")
        }));
        assert!(!is_removal(&WorkLinkArgs {
            session_id: Some(1),
            ..args("unlink")
        }));
        assert!(!is_removal(&args("archive")));
    }
    /// The host fence of `scope_links` holds here: a per-host scope reaches
    /// only the links that ended on its own host, and the rest answer as
    /// links that do not exist.
    #[test]
    fn another_hosts_ended_link_answers_as_none_to_a_host_scope() {
        let st = store();
        let id = ended(&st, "gone", "ABC-1");
        st.lock().unwrap().upsert_host("elsewhere").unwrap();
        let other = {
            let s = st.lock().unwrap();
            ViewScope::internal().with_org(orgs::OrgScope::for_host(&s, "elsewhere").unwrap())
        };
        let err = work_link_ended(
            &WorkLinkArgs {
                link_id: Some(id),
                key: Some("ABC-1".into()),
                ..args("archive")
            },
            &st,
            &other,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        assert_eq!(err.message, format!("ABC-1 has no ended work link {id}"));
        assert_eq!(archived_at(&st, id), None);

        let own = {
            let s = st.lock().unwrap();
            ViewScope::internal().with_org(orgs::OrgScope::for_host(&s, "h").unwrap())
        };
        let out = work_link_ended(
            &WorkLinkArgs {
                key: Some("ABC-1".into()),
                ..args("archive")
            },
            &st,
            &own,
        )
        .unwrap();
        assert_eq!(out.changed, 1);
    }
}
