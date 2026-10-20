//! The person's side of the agents over the control API: the `agents` tool,
//! served only to the hub owner's own paired device (`Access::PersonDevice`),
//! never to the master token an agent holds. The console's `fleet-hub
//! agents` writes the same rules.
//!
//! `status` reads; `grant`, `revoke`, `pause_all`, `approve` and `reject`
//! write, and the tool checks the device may (a trusted full device) before
//! calling them. An approved action waits for its agent's executor, which
//! records it executed or failed.

use crate::ipc_error::{codes, lock, IpcError};
use crate::store::{
    AgentActionRow, AgentJobRow, AgentRuleRow, NewAgentRule, Store, AGENT_ACTION_STATES,
    AGENT_CONFIG_MAX_CHARS, AGENT_KINDS, AGENT_LEVELS, AGENT_SCOPE_MAX_CHARS,
};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Most jobs and actions one `status` answers.
pub const STATUS_MAX_LIMIT: u32 = 200;
const STATUS_DEFAULT_LIMIT: u32 = 20;
/// Longest a granted rule may run before it ends on its own.
pub const MAX_RULE_HOURS: i64 = 24 * 90;

/// `agents { action, agent?, scope?, level?, host?, project_id?, config?,
/// hours?, definition?, bindings?, answers?, id?, request?, run_scope?, limit? }`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "AgentsParams")]
pub struct AgentsArgs {
    /// status | explain | job | definitions | gallery | form | grant | install |
    /// revoke | pause_all | approve | reject | run.
    pub action: String,
    /// grant: review | support | fleet; status: only this agent's jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// grant: gh:<owner>/<repo> (review) | jira:<tracker id>:<PROJECT> (support) |
    /// the instance name (fleet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// grant: watch | propose | act (default watch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// grant: the host the agent's runs use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// grant: the project whose checkout runs use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    /// grant: the agent's options, a JSON object; for fleet, the instance's
    /// input values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
    /// grant: the rule ends after this many hours (default never).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hours: Option<i64>,
    /// grant (fleet): the catalog fleet_agent the rule runs; install: the
    /// gallery definition to add to the catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
    /// grant (fleet): one binding per store role, a JSON array of
    /// {role, server, name?, env?} (an MCP server; env maps its placeholders
    /// to secret names, `${NAME}`) or {role, namespace: rule | shared:<name>}.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bindings: Option<serde_json::Value>,
    /// grant (fleet): the grant form's answers (`form`), in place of scope,
    /// host, level, project_id, config, hours and bindings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<serde_json::Value>,
    /// run: a person's request to a manual fleet agent (≤ 4000 chars).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    /// run: where the run should look, an object {minutes?, host?, session?,
    /// component?}: the window back from now (≤ 7 days), a host alias, a
    /// session name, one of hub | updater | agent | host | desktop | phone |
    /// session | routine | mission | jev | update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_scope: Option<serde_json::Value>,
    /// revoke, run: a rule's id; approve, reject: an action's id; job: a job's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    /// status: jobs and actions, ≤ 200 (default 20).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl AgentsArgs {
    /// Whether the action only reads.
    pub fn is_read(&self) -> bool {
        matches!(
            self.action.as_str(),
            "status" | "explain" | "job" | "definitions" | "gallery" | "form"
        )
    }
}

/// One rule as `status` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRuleView {
    #[serde(flatten)]
    pub rule: AgentRuleRow,
    /// Not expired.
    pub active: bool,
}

/// What `status` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentsStatus {
    pub rules: Vec<AgentRuleView>,
    pub jobs: Vec<AgentJobRow>,
    /// Actions waiting for a person.
    pub pending: Vec<AgentActionRow>,
    /// Actions a person approved that ran (or failed) in the last
    /// [`RECENT_WINDOW_SECS`], newest first, at most [`RECENT_MAX`]: an
    /// executed one's `error` says what it made, a failed one's why.
    #[serde(default)]
    pub recent: Vec<AgentActionRow>,
}

/// How far back `status.recent` reaches.
pub const RECENT_WINDOW_SECS: i64 = 24 * 3600;
/// Most rows `status.recent` carries.
pub const RECENT_MAX: usize = 5;

/// What a rule write answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentsChanged {
    /// Rules written or removed.
    pub changed: u32,
    /// grant: the rule's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_VALIDATE, msg.into())
}

fn db(e: rusqlite::Error) -> IpcError {
    IpcError::new(codes::E_INTERNAL, e.to_string())
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
}

/// PURE: whether `scope` is the shape `agent` watches. `review` takes
/// `gh:<owner>/<repo>`; `support` takes `jira:<tracker id>:<PROJECT>`,
/// answering the tracker id.
pub fn parse_scope(agent: &str, scope: &str) -> Result<Option<i64>, String> {
    if scope.chars().count() > AGENT_SCOPE_MAX_CHARS {
        return Err(format!(
            "scope must be at most {AGENT_SCOPE_MAX_CHARS} characters"
        ));
    }
    match agent {
        "review" => {
            let repo = scope
                .strip_prefix("gh:")
                .ok_or("a review scope is gh:<owner>/<repo>")?;
            match repo.split_once('/') {
                Some((o, r))
                    if !o.is_empty()
                        && !r.is_empty()
                        && o.chars().all(is_name_char)
                        && r.chars().all(is_name_char)
                        && r != "."
                        && r != ".." =>
                {
                    Ok(None)
                }
                _ => Err(format!("{scope:?} is not gh:<owner>/<repo>")),
            }
        }
        "support" => {
            let bad = || format!("{scope:?} is not jira:<tracker id>:<PROJECT>");
            let rest = scope.strip_prefix("jira:").ok_or_else(bad)?;
            let (id, key) = rest.split_once(':').ok_or_else(bad)?;
            let id: i64 = id.parse().map_err(|_| bad())?;
            let key_ok = key.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                && key
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            if id <= 0 || !key_ok {
                return Err(bad());
            }
            Ok(Some(id))
        }
        other => Err(format!(
            "unknown agent {other:?}: {}",
            AGENT_KINDS.join(" | ")
        )),
    }
}

/// Check a rule before it is written: the agent, the level, the scope's
/// shape, the hours, and that the host, the project and (for support) a
/// Jira tracker exist on this hub. Returns the row to write.
pub fn validate_grant(
    s: &Store,
    args: &AgentsArgs,
    granted_by: &str,
    now: i64,
) -> Result<NewAgentRule, IpcError> {
    let agent = args
        .agent
        .as_deref()
        .ok_or_else(|| invalid("grant needs agent"))?;
    if !AGENT_KINDS.contains(&agent) {
        return Err(invalid(format!(
            "agent must be one of {}",
            AGENT_KINDS.join(", ")
        )));
    }
    if agent == "fleet" {
        // A fleet agent's grant renders its stores and asks each on the
        // host: `service::fleet_agents::grant`, which the `agents` tool and
        // `fleet-hub agents grant fleet` call.
        return Err(invalid(
            "a fleet agent is granted with its stores checked (fleet_agents::grant)",
        ));
    }
    if args.definition.is_some() || args.bindings.is_some() {
        return Err(invalid("definition and bindings are for agent fleet"));
    }
    let scope = args
        .scope
        .as_deref()
        .map(str::trim)
        .ok_or_else(|| invalid("grant needs scope"))?;
    let tracker = parse_scope(agent, scope).map_err(invalid)?;
    let level = args.level.as_deref().unwrap_or("watch");
    if !AGENT_LEVELS.contains(&level) {
        return Err(invalid(format!(
            "level must be one of {}",
            AGENT_LEVELS.join(", ")
        )));
    }
    if let Some(h) = args.hours {
        if !(1..=MAX_RULE_HOURS).contains(&h) {
            return Err(invalid(format!("hours must be 1-{MAX_RULE_HOURS}")));
        }
    }
    let config = match &args.config {
        None => "{}".to_string(),
        Some(v) if v.is_object() => v.to_string(),
        Some(_) => return Err(invalid("config must be a JSON object")),
    };
    if config.chars().count() > AGENT_CONFIG_MAX_CHARS {
        return Err(invalid(format!(
            "config must be at most {AGENT_CONFIG_MAX_CHARS} characters"
        )));
    }
    if agent == "review" {
        crate::service::agents::review::ReviewConfig::parse(&config).map_err(invalid)?;
    }
    if agent == "support" {
        crate::service::agents::support::SupportConfig::parse(&config).map_err(invalid)?;
    }
    let host = args
        .host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .ok_or_else(|| invalid("grant needs host"))?;
    if s.get_host_row(host).map_err(db)?.is_none() {
        return Err(IpcError::new(codes::E_NOTFOUND, format!("no host {host}")));
    }
    if let Some(pid) = args.project_id {
        if !s.list_projects().map_err(db)?.iter().any(|p| p.id == pid) {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("no project {pid}"),
            ));
        }
    }
    if let Some(tid) = tracker {
        match s.get_tracker(tid)? {
            Some(t) if t.provider.starts_with("jira") => {}
            Some(t) => {
                return Err(invalid(format!(
                    "tracker {tid} is {}, not Jira",
                    t.provider
                )))
            }
            None => {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("no tracker {tid}"),
                ))
            }
        }
    }
    Ok(NewAgentRule {
        agent: agent.to_string(),
        scope: scope.to_string(),
        level: level.to_string(),
        host_alias: host.to_string(),
        project_id: args.project_id,
        config,
        granted_by: granted_by.to_string(),
        granted_at: now,
        expires_at: args.hours.map(|h| now + h * 3600),
        definition: None,
        bindings: "[]".into(),
    })
}

pub fn status(store: &Mutex<Store>, args: &AgentsArgs, now: i64) -> Result<AgentsStatus, IpcError> {
    let limit = args
        .limit
        .unwrap_or(STATUS_DEFAULT_LIMIT)
        .min(STATUS_MAX_LIMIT) as usize;
    let s = lock(store)?;
    let rules = s
        .list_agent_rules()
        .map_err(db)?
        .into_iter()
        .map(|rule| AgentRuleView {
            active: rule.active_at(now),
            rule,
        })
        .collect();
    Ok(AgentsStatus {
        rules,
        jobs: s
            .list_agent_jobs(args.agent.as_deref(), limit)
            .map_err(db)?,
        pending: s.list_agent_actions(Some("pending"), limit).map_err(db)?,
        recent: s
            .recent_decided_agent_actions(now - RECENT_WINDOW_SECS, RECENT_MAX)
            .map_err(db)?,
    })
}

/// Write a rule. `granted_by` names the device, for the record.
pub fn grant(
    store: &Mutex<Store>,
    args: &AgentsArgs,
    granted_by: &str,
    now: i64,
) -> Result<AgentsChanged, IpcError> {
    let s = lock(store)?;
    let rule = validate_grant(&s, args, granted_by, now)?;
    let id = s.grant_agent_rule(&rule).map_err(db)?;
    Ok(AgentsChanged {
        changed: 1,
        id: Some(id),
    })
}

pub fn revoke(store: &Mutex<Store>, args: &AgentsArgs) -> Result<AgentsChanged, IpcError> {
    let id = args.id.ok_or_else(|| invalid("revoke needs id"))?;
    let gone = lock(store)?.revoke_agent_rule(id).map_err(db)?;
    Ok(AgentsChanged {
        changed: u32::from(gone),
        id: None,
    })
}

pub fn pause_all(store: &Mutex<Store>) -> Result<AgentsChanged, IpcError> {
    let n = lock(store)?.revoke_all_agent_rules().map_err(db)?;
    Ok(AgentsChanged {
        changed: n as u32,
        id: None,
    })
}

/// Answer a pending action. Answers the action as it is afterwards; an
/// action already answered is refused with `E_INVALID_STATE`.
pub fn decide(
    store: &Mutex<Store>,
    args: &AgentsArgs,
    approve: bool,
    decided_by: &str,
    now: i64,
) -> Result<AgentActionRow, IpcError> {
    let id = args
        .id
        .ok_or_else(|| invalid(format!("{} needs id", args.action)))?;
    let row = lock(store)?
        .decide_agent_action(id, approve, decided_by, now)
        .map_err(db)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no action {id}")))?;
    let wanted = if approve { "approved" } else { "rejected" };
    if row.state != wanted || row.decided_at != Some(now) {
        debug_assert!(AGENT_ACTION_STATES.contains(&row.state.as_str()));
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("action {id} is already {}", row.state),
        ));
    }
    Ok(row)
}

/// Queue a job for a fleet rule whose trigger is `manual` (or `schedule`:
/// run it now), with the person's request.
pub fn run_now(
    store: &Mutex<Store>,
    args: &AgentsArgs,
    now: i64,
) -> Result<serde_json::Value, IpcError> {
    let id = args.id.ok_or_else(|| invalid("run needs id"))?;
    let rule = lock(store)?
        .agent_rule(id)
        .map_err(db)?
        .filter(|r| r.active_at(now))
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no active rule {id}")))?;
    let Some(def) = rule.definition.as_deref() else {
        return Err(invalid(format!("rule {id} is a built-in agent's")));
    };
    let binds = crate::service::fleet_agents::bindings::parse(&rule.bindings).map_err(invalid)?;
    let sources = crate::service::fleet_agents::grant::Sources::from_catalog(def, &binds)?;
    let job = crate::service::fleet_agents::runner::queue_manual(
        store,
        &rule,
        sources.definition.definition.trigger.source,
        args.request.as_deref(),
        args.run_scope.as_ref(),
        now,
    )?;
    Ok(serde_json::json!({ "changed": 1, "id": id, "job": job }))
}

/// A fleet agent's grant form: `definition`'s, prefilled from rule `id`
/// when given (Edit).
pub fn fleet_form(
    store: &Mutex<Store>,
    args: &AgentsArgs,
) -> Result<crate::pages::forms::FormSpec, IpcError> {
    use crate::service::fleet_agents::{form, grant::Sources};
    let rule = match args.id {
        Some(id) => Some(
            lock(store)?
                .agent_rule(id)
                .map_err(db)?
                .filter(|r| r.agent == "fleet")
                .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no fleet rule {id}")))?,
        ),
        None => None,
    };
    let def = args
        .definition
        .as_deref()
        .or(rule.as_ref().and_then(|r| r.definition.as_deref()))
        .ok_or_else(|| invalid("form needs definition (or a fleet rule's id)"))?;
    let sources = Sources::from_catalog(def, &[])?;
    let ctx = form::FormContext::read(store)?;
    Ok(form::grant_form(&sources.definition, &ctx, rule.as_ref()))
}

/// Dispatch one call. The caller has already checked the device may write.
pub fn call(
    store: &Mutex<Store>,
    args: &AgentsArgs,
    by: &str,
    now: i64,
) -> Result<serde_json::Value, IpcError> {
    let to_json = |v: serde_json::Result<serde_json::Value>| {
        v.map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))
    };
    match args.action.as_str() {
        "status" => to_json(serde_json::to_value(status(store, args, now)?)),
        "explain" => to_json(serde_json::to_value(super::debug::explain(store, now)?)),
        "job" => {
            let id = args.id.ok_or_else(|| invalid("job needs id"))?;
            to_json(serde_json::to_value(super::debug::job(store, id, now)?))
        }
        "grant" => to_json(serde_json::to_value(grant(store, args, by, now)?)),
        "revoke" => to_json(serde_json::to_value(revoke(store, args)?)),
        "pause_all" => to_json(serde_json::to_value(pause_all(store)?)),
        "approve" => to_json(serde_json::to_value(decide(store, args, true, by, now)?)),
        "reject" => to_json(serde_json::to_value(decide(store, args, false, by, now)?)),
        "run" => run_now(store, args, now),
        "definitions" => to_json(serde_json::to_value(
            crate::service::fleet_agents::form::definitions()?,
        )),
        "form" => to_json(serde_json::to_value(fleet_form(store, args)?)),
        "gallery" => to_json(serde_json::to_value(
            crate::service::fleet_agents::gallery::gallery(),
        )),
        "install" => {
            let name = args
                .definition
                .as_deref()
                .ok_or_else(|| invalid("install needs definition (a gallery name)"))?;
            to_json(serde_json::to_value(
                crate::service::fleet_agents::gallery::install(store, name)?,
            ))
        }
        other => Err(invalid(format!(
            "unknown agents action {other:?}: status | explain | job | grant | revoke | pause_all | \
             approve | reject | run | definitions | gallery | form | install"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (Mutex<Store>, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("box").unwrap();
        let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
        (Mutex::new(s), pid)
    }

    fn args(action: &str) -> AgentsArgs {
        AgentsArgs {
            action: action.into(),
            ..Default::default()
        }
    }

    fn grant_args(pid: i64) -> AgentsArgs {
        AgentsArgs {
            agent: Some("review".into()),
            scope: Some("gh:o/r".into()),
            level: Some("propose".into()),
            host: Some("box".into()),
            project_id: Some(pid),
            config: Some(serde_json::json!({ "max_diff_lines": 3000 })),
            hours: Some(2),
            ..args("grant")
        }
    }

    #[test]
    fn scopes_are_checked_per_agent() {
        assert_eq!(parse_scope("review", "gh:o/r.js"), Ok(None));
        assert_eq!(parse_scope("support", "jira:3:L2S"), Ok(Some(3)));
        for bad in ["o/r", "gh:o", "gh:/r", "gh:o/r/x", "gh:o/..", "gh:o r/x"] {
            assert!(parse_scope("review", bad).is_err(), "{bad}");
        }
        for bad in [
            "jira:L2S",
            "jira:x:L2S",
            "jira:0:L2S",
            "jira:3:l2s",
            "jira:3:",
        ] {
            assert!(parse_scope("support", bad).is_err(), "{bad}");
        }
        assert!(parse_scope("deploy", "gh:o/r").is_err());
    }

    #[test]
    fn grant_status_revoke_round_trip() {
        let (st, pid) = store();
        let v = call(&st, &grant_args(pid), "phone", 1000).unwrap();
        assert_eq!(v["changed"], 1);
        let id = v["id"].as_i64().unwrap();
        let v = call(&st, &args("status"), "phone", 1000).unwrap();
        let r = &v["rules"][0];
        assert_eq!(r["scope"], "gh:o/r");
        assert_eq!(r["level"], "propose");
        assert_eq!(r["granted_by"], "phone");
        assert_eq!(r["expires_at"], 1000 + 7200);
        assert_eq!(r["active"], true);
        assert_eq!(r["config"], r#"{"max_diff_lines":3000}"#);
        let later = call(&st, &args("status"), "phone", 1000 + 7200).unwrap();
        assert_eq!(later["rules"][0]["active"], false);
        let mut rv = args("revoke");
        rv.id = Some(id);
        assert_eq!(call(&st, &rv, "phone", 0).unwrap()["changed"], 1);
        assert_eq!(call(&st, &rv, "phone", 0).unwrap()["changed"], 0);
    }

    #[test]
    fn grant_refuses_bad_input_and_unknown_things() {
        let (st, pid) = store();
        let mut a = grant_args(pid);
        a.agent = None;
        assert!(call(&st, &a, "x", 0).is_err());
        let mut a = grant_args(pid);
        a.level = Some("auto".into());
        assert!(call(&st, &a, "x", 0).is_err());
        let mut a = grant_args(pid);
        a.hours = Some(MAX_RULE_HOURS + 1);
        assert!(call(&st, &a, "x", 0).is_err());
        let mut a = grant_args(pid);
        a.config = Some(serde_json::json!([1]));
        assert!(call(&st, &a, "x", 0).is_err());
        for bad in [
            serde_json::json!({ "max_turns": 0 }),
            serde_json::json!({ "max_diff_lnes": 10 }),
            serde_json::json!({ "test_blocking_importance": "low" }),
        ] {
            let mut a = grant_args(pid);
            a.config = Some(bad.clone());
            assert!(call(&st, &a, "x", 0).is_err(), "{bad}");
        }
        let mut a = grant_args(pid);
        a.host = Some("nowhere".into());
        assert_eq!(call(&st, &a, "x", 0).unwrap_err().code, codes::E_NOTFOUND);
        let mut a = grant_args(pid);
        a.project_id = Some(pid + 99);
        assert_eq!(call(&st, &a, "x", 0).unwrap_err().code, codes::E_NOTFOUND);
        let mut a = grant_args(pid);
        a.agent = Some("support".into());
        a.scope = Some("jira:42:L2S".into());
        assert_eq!(
            call(&st, &a, "x", 0).unwrap_err().code,
            codes::E_VALIDATE,
            "a review config"
        );
        a.config = Some(serde_json::json!({ "phrases": [] }));
        assert_eq!(
            call(&st, &a, "x", 0).unwrap_err().code,
            codes::E_VALIDATE,
            "nothing triggers"
        );
        a.config = None;
        assert_eq!(call(&st, &a, "x", 0).unwrap_err().code, codes::E_NOTFOUND);
        assert!(call(&st, &args("nope"), "x", 0).is_err());
        assert!(lock(&st).unwrap().list_agent_rules().unwrap().is_empty());
    }

    #[test]
    fn an_action_is_approved_once_and_pause_all_clears_the_rules() {
        let (st, pid) = store();
        call(&st, &grant_args(pid), "x", 0).unwrap();
        let aid = {
            let s = lock(&st).unwrap();
            let rid = s.list_agent_rules().unwrap()[0].id;
            s.record_agent_poll(
                rid,
                10,
                None,
                &[crate::store::NewAgentJob {
                    rule_id: rid,
                    agent: "review".into(),
                    subject: "gh:o/r#1".into(),
                    trigger: "opened".into(),
                    dedup_key: "gh:o/r#1@a".into(),
                    not_before: 0,
                }],
            )
            .unwrap();
            let jid = s.list_agent_jobs(None, 1).unwrap()[0].id;
            s.insert_agent_action(jid, "post_review", "{}", 10).unwrap()
        };
        assert_eq!(
            call(&st, &args("status"), "x", 20).unwrap()["pending"][0]["id"],
            aid
        );
        let mut ap = args("approve");
        ap.id = Some(aid);
        let v = call(&st, &ap, "phone", 30).unwrap();
        assert_eq!(v["state"], "approved");
        assert_eq!(v["decided_by"], "phone");
        let mut rj = args("reject");
        rj.id = Some(aid);
        assert_eq!(
            call(&st, &rj, "phone", 40).unwrap_err().code,
            codes::E_INVALID_STATE
        );
        rj.id = Some(aid + 5);
        assert_eq!(
            call(&st, &rj, "phone", 40).unwrap_err().code,
            codes::E_NOTFOUND
        );
        assert_eq!(call(&st, &args("pause_all"), "x", 0).unwrap()["changed"], 1);
        assert!(args("status").is_read() && !args("approve").is_read());
    }
}
