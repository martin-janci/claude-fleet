//! Work links (roadmap M1b.2): read and decide which work a session is doing.
//! Storage and its rules are in `store::work`; this is the one transport-
//! agnostic entry the MCP tools `work` / `work_link` and the desktop commands
//! share, so a paired desktop and a local one answer the same way.

pub mod harvest;

use crate::ipc_error::{codes, lock, IpcError};
use crate::store::{SessionRow, Store, WorkLinkRow, WorkTarget};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Debug, Clone, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "WorkParams")]
pub struct WorkArgs {
    /// The session's live links.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// Or: ended (past) links to this key.
    #[serde(default)]
    pub key: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "WorkLinkParams")]
pub struct WorkLinkArgs {
    /// Fleet session id.
    pub session_id: i64,
    /// link | reject | unlink
    pub action: String,
    /// Work key, e.g. ABC-123, or a free-form name.
    #[serde(default)]
    pub key: Option<String>,
    /// Or: a work item id.
    #[serde(default)]
    pub item_id: Option<i64>,
    /// For unlink.
    #[serde(default)]
    pub link_id: Option<i64>,
    /// manual (default) | agent
    #[serde(default)]
    pub source: Option<String>,
}

/// `{session_id}` → that session's live links (confirmed and rejected,
/// primary first); `{key}` → ended links to the key (past work). Exactly one.
pub fn work(args: &WorkArgs, store: &Mutex<Store>) -> Result<Vec<WorkLinkRow>, IpcError> {
    let s = lock(store)?;
    match (args.session_id, args.key.as_deref()) {
        (Some(id), None) => s.session_work_links(id),
        (None, Some(key)) => s.ended_work_links_for_key(key),
        _ => Err(IpcError::new(
            codes::E_INVALID,
            "pass exactly one of session_id or key",
        )),
    }
}

/// Apply one link decision and return the session's updated row (its `work`
/// is the new primary link, or none).
pub fn work_link(args: &WorkLinkArgs, store: &Mutex<Store>) -> Result<SessionRow, IpcError> {
    let s = lock(store)?;
    let target = || -> Result<WorkTarget<'_>, IpcError> {
        match (args.item_id, args.key.as_deref()) {
            (Some(id), None) => Ok(WorkTarget::Item(id)),
            (None, Some(key)) => Ok(WorkTarget::Key(key)),
            _ => Err(IpcError::new(
                codes::E_INVALID,
                format!("{} needs exactly one of key or item_id", args.action),
            )),
        }
    };
    match args.action.as_str() {
        "link" => {
            let source = args.source.as_deref().unwrap_or("manual");
            s.link_session_work(args.session_id, target()?, source)?;
        }
        "reject" => {
            s.reject_session_work(args.session_id, target()?)?;
        }
        "unlink" => {
            let link_id = args
                .link_id
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "unlink needs link_id"))?;
            if !s.unlink_session_work(args.session_id, link_id)? {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!(
                        "session {} has no live work link {link_id}",
                        args.session_id
                    ),
                ));
            }
        }
        other => {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("unknown work_link action {other:?}; one of link, reject, unlink"),
            ))
        }
    }
    s.get_session_by_id(args.session_id)?.ok_or_else(|| {
        IpcError::new(
            codes::E_NOTFOUND,
            format!("session {} not found", args.session_id),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (Mutex<Store>, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let id = s
            .upsert_session("dev", "h", None, None, 1, 1, "running", None)
            .unwrap();
        (Mutex::new(s), id)
    }

    fn link(sid: i64, action: &str) -> WorkLinkArgs {
        WorkLinkArgs {
            session_id: sid,
            action: action.into(),
            ..Default::default()
        }
    }

    #[test]
    fn link_reject_and_unlink_answer_the_updated_row() {
        let (st, sid) = store();
        let row = work_link(
            &WorkLinkArgs {
                key: Some("abc-1".into()),
                source: Some("agent".into()),
                ..link(sid, "link")
            },
            &st,
        )
        .unwrap();
        let w = row.work.expect("primary work");
        assert_eq!(
            (w.key.as_deref(), w.source.as_str()),
            (Some("ABC-1"), "agent")
        );

        let links = work(
            &WorkArgs {
                session_id: Some(sid),
                key: None,
            },
            &st,
        )
        .unwrap();
        assert_eq!(links.len(), 1);

        let row = work_link(
            &WorkLinkArgs {
                link_id: Some(w.link_id),
                ..link(sid, "unlink")
            },
            &st,
        )
        .unwrap();
        assert_eq!(row.work, None);
        let err = work_link(
            &WorkLinkArgs {
                link_id: Some(w.link_id),
                ..link(sid, "unlink")
            },
            &st,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);

        let row = work_link(
            &WorkLinkArgs {
                key: Some("ABC-1".into()),
                ..link(sid, "reject")
            },
            &st,
        )
        .unwrap();
        assert_eq!(row.work, None);
        let links = work(
            &WorkArgs {
                session_id: Some(sid),
                key: None,
            },
            &st,
        )
        .unwrap();
        assert_eq!(links[0].state, "rejected");
    }

    #[test]
    fn malformed_requests_are_refused() {
        let (st, sid) = store();
        for args in [
            link(sid, "link"),
            WorkLinkArgs {
                key: Some("A-1".into()),
                item_id: Some(1),
                ..link(sid, "reject")
            },
            link(sid, "unlink"),
            WorkLinkArgs {
                key: Some("A-1".into()),
                ..link(sid, "primary")
            },
            WorkLinkArgs {
                key: Some("A-1".into()),
                source: Some("branch".into()),
                ..link(sid, "link")
            },
        ] {
            let err = work_link(&args, &st).unwrap_err();
            assert_eq!(err.code, codes::E_INVALID, "{args:?}");
        }
        for args in [
            WorkArgs::default(),
            WorkArgs {
                session_id: Some(sid),
                key: Some("A-1".into()),
            },
        ] {
            assert_eq!(work(&args, &st).unwrap_err().code, codes::E_INVALID);
        }
        let err = work_link(
            &WorkLinkArgs {
                key: Some("A-1".into()),
                ..link(sid + 99, "link")
            },
            &st,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
    }
}
