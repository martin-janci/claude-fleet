//! Work links (roadmap M1b.2): read and decide which work a session is doing.
//! Storage and its rules are in `store::work`; this is the one transport-
//! agnostic entry the MCP tools `work` / `work_link` and the desktop commands
//! share, so a paired desktop and a local one answer the same way.

pub mod detect;
pub mod handover;
pub mod harvest;
pub mod recognize;
pub mod resolve;
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
    /// Default links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "work_action_schema")]
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
    /// Tickets: one tracker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_id: Option<i64>,
    /// mine|sprint|recent|filter:<id>
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    /// Tickets: text filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Tickets: max rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Lookup: a ticket URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "WorkLinkParams")]
pub struct WorkLinkArgs {
    /// Fleet session id.
    #[serde(default)]
    pub session_id: Option<i64>,
    /// The decision.
    #[schemars(schema_with = "work_link_action_schema")]
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
    /// Start: a ticket URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Start: the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    /// Start: brief Claude with the ticket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with_brief: Option<bool>,
    /// Start: session name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Start: worktree name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    /// trust_project: on/off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

impl WorkLinkArgs {
    pub fn parsed_action(&self) -> Result<WorkLinkAction, IpcError> {
        parse_action("work_link", WORK_LINK_ACTIONS, &self.action)
    }
}

/// `work_link { action: trust_project }`: the projects whose branch keys
/// now link automatically.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectTrust {
    #[serde(default)]
    pub trusted: Vec<i64>,
}

/// `work_link { action: trust_project, project_id, on }`.
pub fn trust_project(args: &WorkLinkArgs, store: &Mutex<Store>) -> Result<ProjectTrust, IpcError> {
    let pid = args
        .project_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "trust_project needs project_id"))?;
    let on = args
        .on
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "trust_project needs on"))?;
    let s = lock(store)?;
    Ok(ProjectTrust {
        trusted: detect::set_project_trust(&s, pid, on)?
            .into_iter()
            .collect(),
    })
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
    /// Tracker tickets from the cache (work graph M3.4).
    Tickets,
    /// One ticket by key or URL: the cache, else one live fetch.
    Lookup,
    /// The trackers (no secrets).
    Trackers,
}

/// Every `work` action by its wire name. [`WorkArgs::parsed_action`] reads
/// this table and the tool schema's `action` enum is generated from it, so a
/// client that reads the enum (the phone, work graph M8) is offered exactly
/// what the parser accepts.
pub const WORK_ACTIONS: &[(&str, WorkAction)] = &[
    ("links", WorkAction::Links),
    ("context", WorkAction::Context),
    ("resume_plan", WorkAction::ResumePlan),
    ("purge_impact", WorkAction::PurgeImpact),
    ("tickets", WorkAction::Tickets),
    ("lookup", WorkAction::Lookup),
    ("trackers", WorkAction::Trackers),
];

/// The `work_link` actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkLinkAction {
    Link,
    Reject,
    Unlink,
    Confirm,
    TrustProject,
    Resume,
    Start,
}

/// Every `work_link` action by its wire name; see [`WORK_ACTIONS`].
pub const WORK_LINK_ACTIONS: &[(&str, WorkLinkAction)] = &[
    ("link", WorkLinkAction::Link),
    ("reject", WorkLinkAction::Reject),
    ("unlink", WorkLinkAction::Unlink),
    ("confirm", WorkLinkAction::Confirm),
    ("trust_project", WorkLinkAction::TrustProject),
    ("resume", WorkLinkAction::Resume),
    ("start", WorkLinkAction::Start),
];

/// Look `name` up in an action table; the refusal names every action.
fn parse_action<A: Copy>(tool: &str, table: &[(&str, A)], name: &str) -> Result<A, IpcError> {
    table
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, a)| *a)
        .ok_or_else(|| {
            let names: Vec<&str> = table.iter().map(|(n, _)| *n).collect();
            IpcError::new(
                codes::E_INVALID,
                format!(
                    "unknown {tool} action {name:?}; one of {}",
                    names.join(", ")
                ),
            )
        })
}

/// A string schema whose `enum` is an action table's names.
fn action_schema<A>(table: &[(&str, A)]) -> rmcp::schemars::Schema {
    let names: Vec<&str> = table.iter().map(|(n, _)| *n).collect();
    rmcp::schemars::json_schema!({ "type": "string", "enum": names })
}

fn work_action_schema(_: &mut rmcp::schemars::SchemaGenerator) -> rmcp::schemars::Schema {
    action_schema(WORK_ACTIONS)
}

fn work_link_action_schema(_: &mut rmcp::schemars::SchemaGenerator) -> rmcp::schemars::Schema {
    action_schema(WORK_LINK_ACTIONS)
}

impl WorkArgs {
    pub fn parsed_action(&self) -> Result<WorkAction, IpcError> {
        parse_action(
            "work",
            WORK_ACTIONS,
            self.action.as_deref().unwrap_or("links"),
        )
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

/// `work { action: lookup }`'s reference: `url`, else `key`.
pub fn lookup_reference(args: &WorkArgs) -> Result<&str, IpcError> {
    args.url
        .as_deref()
        .or(args.key.as_deref())
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "lookup needs key or url"))
}

/// The start half of [`WorkLinkArgs`] (work graph M3.4).
pub fn start_args(args: &WorkLinkArgs) -> crate::service::trackers::tickets::StartArgs {
    crate::service::trackers::tickets::StartArgs {
        reference: args.url.clone().or(args.key.clone()),
        item_id: args.item_id,
        project_id: args.project_id,
        host_alias: args.host_alias.clone(),
        with_brief: args.with_brief.unwrap_or(false) || args.brief.is_some(),
        brief: args.brief.clone(),
        name: args.name.clone(),
        worktree: args.worktree.clone(),
    }
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
    let action = args.parsed_action()?;
    let own_entry_point = || {
        IpcError::new(
            codes::E_INVALID,
            format!("{} has its own entry point", args.action),
        )
    };
    if matches!(
        action,
        WorkLinkAction::Resume | WorkLinkAction::Start | WorkLinkAction::TrustProject
    ) {
        return Err(own_entry_point());
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
    match action {
        WorkLinkAction::Link => {
            let source = args.source.as_deref().unwrap_or("manual");
            s.link_session_work(session_id, target()?, source)?;
        }
        // `reject { link_id }` decides one suggestion (work graph M4.4);
        // `reject { key | item_id }` any target.
        WorkLinkAction::Reject
            if args.link_id.is_some() && args.key.is_none() && args.item_id.is_none() =>
        {
            detect::decide(&s, session_id, args.link_id.unwrap_or_default(), false)?;
        }
        WorkLinkAction::Reject => {
            s.reject_session_work(session_id, target()?)?;
        }
        WorkLinkAction::Confirm => {
            let link_id = args
                .link_id
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "confirm needs link_id"))?;
            detect::decide(&s, session_id, link_id, true)?;
        }
        WorkLinkAction::Unlink => {
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
        WorkLinkAction::Resume | WorkLinkAction::Start | WorkLinkAction::TrustProject => {
            return Err(own_entry_point());
        }
    }
    // A decision can leave a sole candidate or free a primary (M4.3).
    if let Err(e) = detect::resolve_session(&s, session_id) {
        tracing::debug!(error = %e.message, "[work] resolve after a decision failed");
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

    /// A variant's position, by an exhaustive match: a new action fails to
    /// compile here, and then fails the table test below until the table
    /// names it — so the parser, the schema enum and the dispatch cannot
    /// disagree on which actions exist.
    fn work_ordinal(a: WorkAction) -> usize {
        match a {
            WorkAction::Links => 0,
            WorkAction::Context => 1,
            WorkAction::ResumePlan => 2,
            WorkAction::PurgeImpact => 3,
            WorkAction::Tickets => 4,
            WorkAction::Lookup => 5,
            WorkAction::Trackers => 6,
        }
    }

    fn work_link_ordinal(a: WorkLinkAction) -> usize {
        match a {
            WorkLinkAction::Link => 0,
            WorkLinkAction::Reject => 1,
            WorkLinkAction::Unlink => 2,
            WorkLinkAction::Confirm => 3,
            WorkLinkAction::TrustProject => 4,
            WorkLinkAction::Resume => 5,
            WorkLinkAction::Start => 6,
        }
    }

    fn assert_table_is_exact<A: Copy + std::fmt::Debug>(
        table: &[(&str, A)],
        ordinal: fn(A) -> usize,
        parse: impl Fn(&str) -> Result<A, IpcError>,
    ) {
        let mut seen = vec![false; table.len()];
        for (name, action) in table {
            let parsed = parse(name).unwrap_or_else(|e| panic!("{name}: {}", e.message));
            assert_eq!(ordinal(parsed), ordinal(*action), "{name}");
            let i = ordinal(*action);
            assert!(i < seen.len(), "{action:?} is missing from the table");
            assert!(!seen[i], "{action:?} is named twice");
            seen[i] = true;
        }
        assert!(seen.iter().all(|s| *s), "every variant has a wire name");
    }

    #[test]
    fn every_work_action_is_in_the_table_and_parses() {
        assert_table_is_exact(WORK_ACTIONS, work_ordinal, |n| {
            WorkArgs {
                action: Some(n.into()),
                ..Default::default()
            }
            .parsed_action()
        });
        assert_eq!(
            WorkArgs::default().parsed_action().unwrap(),
            WorkAction::Links
        );
        let err = WorkArgs {
            action: Some("nope".into()),
            ..Default::default()
        }
        .parsed_action()
        .unwrap_err();
        assert!(
            err.message.contains("\"nope\"; one of links, context"),
            "{}",
            err.message
        );
    }

    #[test]
    fn every_work_link_action_is_in_the_table_and_parses() {
        assert_table_is_exact(WORK_LINK_ACTIONS, work_link_ordinal, |n| {
            WorkLinkArgs {
                action: n.into(),
                ..Default::default()
            }
            .parsed_action()
        });
        let err = link(1, "nope").parsed_action().unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(
            err.message.contains("trust_project, resume, start"),
            "{}",
            err.message
        );
    }

    /// The served schema's `enum` is the table, in order — what the phone
    /// reads to decide which buttons to draw (work graph M8.0).
    #[test]
    fn the_action_schemas_enumerate_the_tables() {
        fn names<A>(t: &[(&str, A)]) -> Vec<serde_json::Value> {
            t.iter().map(|(n, _)| serde_json::json!(n)).collect()
        }
        let w = serde_json::to_value(rmcp::schemars::schema_for!(WorkArgs)).unwrap();
        assert_eq!(
            w["properties"]["action"]["enum"],
            serde_json::Value::Array(names(WORK_ACTIONS))
        );
        let l = serde_json::to_value(rmcp::schemars::schema_for!(WorkLinkArgs)).unwrap();
        assert_eq!(
            l["properties"]["action"]["enum"],
            serde_json::Value::Array(names(WORK_LINK_ACTIONS))
        );
        assert_eq!(l["required"], serde_json::json!(["action"]));
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

    /// Work graph M4.4: confirm / reject a suggestion by link id, and trust
    /// a project, through the one entry both transports share.
    #[test]
    fn suggestions_are_decided_by_link_id_and_projects_trusted() {
        let (st, sid) = store();
        let pid = {
            let s = st.lock().unwrap();
            s.create_local_work_item(Some("PAY-7"), "Retry").unwrap();
            detect::on_prompt(&s, sid, "see PAY-7 and PAY-8", false).unwrap();
            s.upsert_project("acme", "api", "/src/api").unwrap()
        };
        let sg = |st: &Mutex<Store>| {
            st.lock()
                .unwrap()
                .get_session_by_id(sid)
                .unwrap()
                .unwrap()
                .work_suggested
        };
        let first = sg(&st).expect("a suggestion");
        let row = work_link(
            &WorkLinkArgs {
                link_id: Some(first.link_id),
                ..link(sid, "confirm")
            },
            &st,
        )
        .unwrap();
        assert_eq!(row.work.unwrap().link_id, first.link_id);
        assert_eq!(
            work_link(&link(sid, "confirm"), &st).unwrap_err().code,
            codes::E_INVALID
        );
        let err = work_link(
            &WorkLinkArgs {
                link_id: Some(9999),
                ..link(sid, "reject")
            },
            &st,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);

        let t = trust_project(
            &WorkLinkArgs {
                action: "trust_project".into(),
                project_id: Some(pid),
                on: Some(true),
                ..Default::default()
            },
            &st,
        )
        .unwrap();
        assert_eq!(t.trusted, vec![pid]);
        let missing = trust_project(
            &WorkLinkArgs {
                action: "trust_project".into(),
                project_id: Some(pid + 50),
                on: Some(true),
                ..Default::default()
            },
            &st,
        )
        .unwrap_err();
        assert_eq!(missing.code, codes::E_NOTFOUND);
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
