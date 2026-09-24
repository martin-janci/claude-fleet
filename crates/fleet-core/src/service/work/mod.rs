//! Work links (roadmap M1b.2): read and decide which work a session is doing.
//! Storage and its rules are in `store::work`; this is the one transport-
//! agnostic entry the MCP tools `work` / `work_link` and the desktop commands
//! share, so a paired desktop and a local one answer the same way.

pub mod handover;
pub mod harvest;
pub mod resume;

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
    /// links|context|resume_plan|purge_impact
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Ended link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_id: Option<i64>,
    /// Target host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    /// Add the brief.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with_brief: Option<bool>,
    /// Purge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    /// Purge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_aliases: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "WorkLinkParams")]
pub struct WorkLinkArgs {
    /// Fleet session id.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// link|reject|unlink|resume
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
    /// last|brief|fresh
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Target host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    /// Edited brief.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<String>,
}

/// `work { action: context }`: the full handover context of a key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkContext {
    pub key: String,
    pub text: String,
}

/// `work { action: purge_impact }`: the keys a purge would leave without
/// resumable conversations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgeImpact {
    #[serde(default)]
    pub keys: Vec<String>,
}

/// The `work` read actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkAction {
    Links,
    Context,
    ResumePlan,
    PurgeImpact,
}

impl WorkArgs {
    pub fn parsed_action(&self) -> Result<WorkAction, IpcError> {
        match self.action.as_deref().unwrap_or("links") {
            "links" => Ok(WorkAction::Links),
            "context" => Ok(WorkAction::Context),
            "resume_plan" => Ok(WorkAction::ResumePlan),
            "purge_impact" => Ok(WorkAction::PurgeImpact),
            other => Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "unknown work action {other:?}; one of links, context, resume_plan, purge_impact"
                ),
            )),
        }
    }

    fn required_key(&self) -> Result<&str, IpcError> {
        self.key
            .as_deref()
            .ok_or_else(|| IpcError::new(codes::E_INVALID, "this work action needs key"))
    }
}

/// `work { action: context, key }`.
pub async fn work_context(
    args: &WorkArgs,
    store: &Mutex<Store>,
    ssh: &std::sync::Arc<crate::ssh::SshClient>,
) -> Result<WorkContext, IpcError> {
    let key = crate::store::normalize_work_ref(args.required_key()?)?;
    let input = handover::gather_handover(store, ssh.as_ref(), &key, None).await?;
    Ok(WorkContext {
        text: handover::build_context(&input),
        key,
    })
}

/// `work { action: resume_plan, key, link_id?, host_alias?, with_brief? }`.
pub async fn work_resume_plan(
    args: &WorkArgs,
    store: &Mutex<Store>,
    ssh: &std::sync::Arc<crate::ssh::SshClient>,
) -> Result<resume::ResumePlan, IpcError> {
    resume::resume_plan(
        store,
        ssh,
        args.required_key()?,
        args.link_id,
        args.host_alias.as_deref(),
        args.with_brief.unwrap_or(false),
    )
    .await
}

/// `work { action: purge_impact, project_id, host_aliases }`.
pub fn work_purge_impact(args: &WorkArgs, store: &Mutex<Store>) -> Result<PurgeImpact, IpcError> {
    let pid = args
        .project_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "purge_impact needs project_id"))?;
    let hosts = args.host_aliases.clone().unwrap_or_default();
    Ok(PurgeImpact {
        keys: lock(store)?.work_keys_for_purge(pid, &hosts)?,
    })
}

/// `work_link { action: resume, key, mode, link_id?, host_alias?, brief? }`.
pub async fn work_resume(
    args: &WorkLinkArgs,
    store: &std::sync::Arc<Mutex<Store>>,
    ssh: &std::sync::Arc<crate::ssh::SshClient>,
    reg: &std::sync::Arc<crate::cancel::CancellationRegistry>,
) -> Result<SessionRow, IpcError> {
    resume::resume_work(store, ssh, reg, &resume_args(args)?).await
}

/// The resume half of [`WorkLinkArgs`].
pub fn resume_args(args: &WorkLinkArgs) -> Result<resume::ResumeArgs, IpcError> {
    Ok(resume::ResumeArgs {
        key: args
            .key
            .clone()
            .ok_or_else(|| IpcError::new(codes::E_INVALID, "resume needs key"))?,
        mode: args.mode.clone().unwrap_or_else(|| "last".into()),
        link_id: args.link_id,
        host_alias: args.host_alias.clone(),
        brief: args.brief.clone(),
    })
}

/// Most recently ended links one `work {}` read returns.
pub const RECENT_LINKS_MAX: i64 = 200;

/// `{session_id}` → that session's live links (confirmed and rejected,
/// primary first); `{key}` → ended links to the key (past work). Exactly one.
pub fn work(args: &WorkArgs, store: &Mutex<Store>) -> Result<Vec<WorkLinkRow>, IpcError> {
    let s = lock(store)?;
    match (args.session_id, args.key.as_deref()) {
        (Some(id), None) => s.session_work_links(id),
        (None, Some(key)) => s.ended_work_links_for_key(key),
        // Neither: work that ended recently (`work.recent_days`), so past-only
        // work has a group to show in.
        (None, None) => {
            let days = crate::service::settings::resolve(
                crate::service::settings::WORK_RECENT_DAYS,
                s.get_setting(crate::service::settings::WORK_RECENT_DAYS)?
                    .as_deref(),
            )
            .parse::<i64>()
            .unwrap_or(14);
            s.recent_ended_work_links(
                crate::service::catalog::now_secs() - days * 86_400,
                RECENT_LINKS_MAX,
            )
        }
        (Some(_), Some(_)) => Err(IpcError::new(
            codes::E_INVALID,
            "pass at most one of session_id or key",
        )),
    }
}

/// Apply one link decision and return the session's updated row (its `work`
/// is the new primary link, or none).
pub fn work_link(args: &WorkLinkArgs, store: &Mutex<Store>) -> Result<SessionRow, IpcError> {
    if args.action == "resume" {
        return Err(IpcError::new(
            codes::E_INVALID,
            "resume is asynchronous; use work_resume",
        ));
    }
    let session_id = args.session_id.ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID,
            format!("{} needs session_id", args.action),
        )
    })?;
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
            s.link_session_work(session_id, target()?, source)?;
        }
        "reject" => {
            s.reject_session_work(session_id, target()?)?;
        }
        "unlink" => {
            let link_id = args
                .link_id
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "unlink needs link_id"))?;
            if !s.unlink_session_work(session_id, link_id)? {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("session {session_id} has no live work link {link_id}"),
                ));
            }
        }
        other => {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("unknown work_link action {other:?}; one of link, reject, unlink, resume"),
            ))
        }
    }
    s.get_session_by_id(session_id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("session {session_id} not found")))
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
            session_id: Some(sid),
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
                ..Default::default()
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
                ..Default::default()
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
        assert!(
            work(&WorkArgs::default(), &st).unwrap().is_empty(),
            "recent: none"
        );
        let both = WorkArgs {
            session_id: Some(sid),
            key: Some("A-1".into()),
            ..Default::default()
        };
        assert_eq!(work(&both, &st).unwrap_err().code, codes::E_INVALID);
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
