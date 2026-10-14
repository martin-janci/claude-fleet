//! Actions (spec step 4, *E. Actions*): what a fleet agent's checked answer
//! turns into, and how each is carried out.
//!
//! [`plan`] is PURE: each of the definition's actions whose `when` holds over
//! the answer, rendered into the payload that will be sent (the body a
//! person reads before approving is the body that goes out). A `stop` action
//! that is taken ends the list. The runner queues them by the rule's level:
//! kept in the job's result at `watch`, waiting for a person at `propose`,
//! approved at once at `act` when the action is `auto` (an `mcp_call` also
//! needs its store to mark the tool `auto_ok`).
//!
//! [`FleetExecutor`] sends an approved action after checking it again
//! against the rule as it is then: still there and above `watch`, the
//! definition still asking for that action, the tracker the instance's, the
//! issue inside `limits.write_projects` (or the job's own subject), the body
//! within `limits.max_body_chars`, an `mcp_call`'s tool among the role's
//! `write_tools`, its arguments within 16 KiB and valid against the tool's
//! `inputSchema` as the server lists it, a `record` into the rule's own
//! namespace for that role.
//!
//! **The loop guard.** Every comment a fleet agent posts ends with its
//! marker line ([`marker`]); a tracker poll never turns a comment carrying
//! one into a job. And the runner skips a subject that already ran
//! [`MAX_RUNS_PER_SUBJECT_DAY`] times in a day.

use super::answer;
use super::bindings;
use super::definition::{Action, ActionKind, Definition, InputType, StoreKind};
use super::runner::{FleetHost, Instance};
use super::template;
use crate::service::agents::{ActionResult, AgentExecutor};
use crate::service::trackers::WriteOp;
use crate::store::{AgentActionRow, AgentRuleRow, NewAgentRecord, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// A subject's runs in a day, at most, before the runner skips it.
pub const MAX_RUNS_PER_SUBJECT_DAY: i64 = 5;
/// Largest `mcp_call` arguments, as JSON.
pub const MCP_ARGS_MAX: usize = 16 * 1024;
/// What every fleet agent's comment ends with, before the instance's name.
pub const MARKER_PREFIX: &str = "[fleet-agent:";

/// The marker line of instance `instance`.
pub fn marker(instance: &str) -> String {
    format!("{MARKER_PREFIX}{instance}]")
}

/// Whether a comment is a fleet agent's own.
pub fn is_agent_comment(body: &str) -> bool {
    body.contains(MARKER_PREFIX)
}

// ── payloads ──

/// `tracker_comment`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommentPayload {
    pub action: String,
    pub tracker: i64,
    pub key: String,
    /// What is posted (Jira Data Center's wiki text; a person reads it).
    pub body: String,
    /// The same as ADF, for Jira Cloud.
    pub adf: Value,
}

/// What a new issue is linked to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkTo {
    pub to: String,
    pub link_type: String,
}

/// `tracker_issue`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssuePayload {
    pub action: String,
    pub tracker: i64,
    pub project: String,
    pub issue_type: String,
    pub summary: String,
    pub body: String,
    pub adf: Value,
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<LinkTo>,
}

/// `tracker_link`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkPayload {
    pub action: String,
    pub tracker: i64,
    pub from: String,
    pub to: String,
    pub link_type: String,
    pub body: String,
}

/// `record`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordPayload {
    pub action: String,
    pub store: String,
    pub namespace: String,
    pub key: String,
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_secs: Option<i64>,
    pub body: String,
}

/// `mcp_call`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpCallPayload {
    pub action: String,
    pub store: String,
    pub binding: String,
    pub tool: String,
    pub arguments: Value,
    /// The tool and its arguments, as a person reads them.
    pub body: String,
}

/// `pr_comment`: a comment on a pull request (or an issue) of the repo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrCommentPayload {
    pub action: String,
    pub repo: String,
    pub number: i64,
    pub body: String,
}

/// `pr_labels`: labels to add and remove (`-name` in the action's list).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrLabelsPayload {
    pub action: String,
    pub repo: String,
    pub number: i64,
    pub add: Vec<String>,
    pub remove: Vec<String>,
    pub body: String,
}

/// Whether a tracker provider is Jira (rich comments, issue links).
pub fn is_jira(provider: Option<&str>) -> bool {
    provider.is_none_or(|p| p.starts_with("jira"))
}

/// The repos `pr_*` writes may land in: the trigger's, and `limits.repos`,
/// rendered over the inputs.
pub fn write_repos(inst: &Instance, now: i64) -> Vec<String> {
    let ctx = inst.ctx(now);
    let d = &inst.loaded().definition;
    d.trigger
        .repo
        .iter()
        .chain(d.limits.repos.iter())
        .map(|r| template::render(r, &ctx).trim().to_string())
        .filter(|r| !r.is_empty())
        .collect()
}

/// One action planned from an answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Planned {
    pub id: String,
    /// `agent_actions.kind`: the action's `do`.
    pub kind: &'static str,
    /// Exactly what will be sent, JSON.
    #[serde(skip)]
    pub payload: String,
    /// The text a person reads.
    pub body: String,
    /// May run at `act` without a person.
    pub auto: bool,
}

/// An action not planned, and why (kept in the job's result).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Unplanned {
    pub id: String,
    pub why: String,
}

// ── plan, PURE ──

/// Plain text as an ADF document of paragraphs (Jira Cloud).
pub fn text_adf(text: &str) -> Value {
    let content: Vec<Value> = text
        .replace("\r\n", "\n")
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut nodes = Vec::new();
            for (i, line) in p.split('\n').enumerate() {
                if i > 0 {
                    nodes.push(json!({ "type": "hardBreak" }));
                }
                if !line.is_empty() {
                    nodes.push(json!({ "type": "text", "text": line }));
                }
            }
            json!({ "type": "paragraph", "content": nodes })
        })
        .collect();
    json!({ "type": "doc", "version": 1, "content": content })
}

/// `text` within `max` characters, its end cut with `…`.
pub fn fit(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

/// The project of an issue key (`L2S-7` → `L2S`).
pub fn project_of(key: &str) -> &str {
    key.rsplit_once('-').map(|(p, _)| p).unwrap_or(key)
}

/// The tracker an action writes to: its own, else the trigger's, else the
/// first tracker input's value, rendered over `ctx`.
pub fn tracker_of(
    d: &Definition,
    a: &Action,
    values: &serde_json::Map<String, Value>,
    ctx: &Value,
) -> Option<i64> {
    let t = a
        .tracker
        .as_deref()
        .or(d.trigger.tracker.as_deref())
        .map(|t| template::render(t, ctx))
        .or_else(|| {
            d.inputs
                .iter()
                .find(|i| i.kind == InputType::Tracker)
                .and_then(|i| values.get(&i.name))
                .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))
        })?;
    t.trim().parse().ok()
}

/// The projects writes may land in, rendered over the inputs.
pub fn write_projects(inst: &Instance, now: i64) -> Vec<String> {
    let ctx = inst.ctx(now);
    inst.loaded()
        .definition
        .limits
        .write_projects
        .iter()
        .map(|p| template::render(p, &ctx).trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

fn labels_of(v: Option<&Value>, ctx: &Value) -> Vec<String> {
    let rendered = v.map(|v| template::render_value(v, ctx));
    match rendered {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::trim).map(str::to_string))
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) => s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// PURE: the actions an answer asks for. `instance` is the rule's scope
/// (its marker); `rule_id` resolves fleet namespaces.
pub fn plan(
    inst: &Instance,
    rule_id: i64,
    instance: &str,
    subject: &Value,
    answer: &Value,
    now: i64,
) -> (Vec<Planned>, Vec<Unplanned>) {
    let loaded = inst.loaded();
    let d = &loaded.definition;
    let ctx = json!({ "inputs": inst.values, "subject": subject, "answer": answer, "now": now });
    let max = d.limits.max_body_chars;
    let namespaces = bindings::namespaces(rule_id, &inst.bindings);
    let mut planned = Vec::new();
    let mut skipped = Vec::new();
    for a in &d.actions {
        if !answer::holds(a.when.as_ref(), answer) {
            continue;
        }
        let r = |t: &Option<String>| {
            t.as_deref()
                .map(|t| template::render(t, &ctx).trim().to_string())
                .unwrap_or_default()
        };
        let body_text = || -> String {
            match (&a.body, &a.body_template) {
                (Some(b), _) => template::render(b, &ctx),
                (None, Some(name)) => loaded
                    .templates
                    .get(name)
                    .map(|t| template::render(t, &ctx))
                    .unwrap_or_default(),
                _ => String::new(),
            }
            .trim()
            .to_string()
        };
        let skip = |why: &str| Unplanned {
            id: a.id.clone(),
            why: why.to_string(),
        };
        let tracker = || tracker_of(d, a, &inst.values, &ctx);
        let made: Result<(String, String), Unplanned> = (|| match a.kind {
            ActionKind::TrackerComment => {
                let key = r(&a.target);
                let text = body_text();
                let tracker = tracker().ok_or_else(|| skip("no tracker"))?;
                if key.is_empty() || text.is_empty() {
                    return Err(skip("the target or the body is empty"));
                }
                let mark = format!("\n\n{}", marker(instance));
                let body = format!(
                    "{}{mark}",
                    fit(&text, max.saturating_sub(mark.chars().count()))
                );
                let p = CommentPayload {
                    action: a.id.clone(),
                    tracker,
                    key,
                    adf: text_adf(&body),
                    body: body.clone(),
                };
                Ok((serde_json::to_string(&p).unwrap_or_default(), body))
            }
            ActionKind::TrackerIssue => {
                let project = r(&a.project);
                let summary = fit(&r(&a.summary), 250);
                let body = fit(&body_text(), max);
                let tracker = tracker().ok_or_else(|| skip("no tracker"))?;
                if project.is_empty() || summary.is_empty() {
                    return Err(skip("the project or the summary is empty"));
                }
                let link = a.link.as_ref().and_then(|l| {
                    let to = template::render(&l.to, &ctx).trim().to_string();
                    (!to.is_empty()).then(|| LinkTo {
                        to,
                        link_type: template::render(&l.link_type, &ctx).trim().to_string(),
                    })
                });
                let shown = format!("{summary}\n\n{body}");
                let p = IssuePayload {
                    action: a.id.clone(),
                    tracker,
                    project,
                    issue_type: Some(r(&a.issue_type))
                        .filter(|t| !t.is_empty())
                        .unwrap_or_else(|| "Task".into()),
                    summary,
                    adf: text_adf(&body),
                    body,
                    labels: labels_of(a.labels.as_ref(), &ctx),
                    link,
                };
                Ok((serde_json::to_string(&p).unwrap_or_default(), shown))
            }
            ActionKind::TrackerLink => {
                let (from, to, link_type) = (r(&a.from), r(&a.to), r(&a.link_type));
                let tracker = tracker().ok_or_else(|| skip("no tracker"))?;
                if from.is_empty() || to.is_empty() || link_type.is_empty() {
                    return Err(skip("from, to or link_type is empty"));
                }
                let body = format!("{from} {link_type} {to}");
                let p = LinkPayload {
                    action: a.id.clone(),
                    tracker,
                    from,
                    to,
                    link_type,
                    body: body.clone(),
                };
                Ok((serde_json::to_string(&p).unwrap_or_default(), body))
            }
            ActionKind::Record => {
                let store = a.store.clone().unwrap_or_default();
                let namespace = namespaces
                    .get(&store)
                    .cloned()
                    .ok_or_else(|| skip("its store is not bound"))?;
                let key = r(&a.key);
                if key.is_empty() {
                    return Err(skip("the key is empty"));
                }
                let value = template::render_value(a.value.as_ref().unwrap_or(&Value::Null), &ctx);
                let body = format!("{store} · {key} = {value}");
                let p = RecordPayload {
                    action: a.id.clone(),
                    store,
                    namespace,
                    key,
                    value,
                    expires_secs: a.expires_secs,
                    body: body.clone(),
                };
                Ok((serde_json::to_string(&p).unwrap_or_default(), body))
            }
            ActionKind::McpCall => {
                let store = a.store.clone().unwrap_or_default();
                let b = inst
                    .bindings
                    .iter()
                    .find(|b| b.role == store)
                    .ok_or_else(|| skip("its store is not bound"))?;
                let tool = a.tool.clone().unwrap_or_default();
                let arguments =
                    template::render_value(a.arguments.as_ref().unwrap_or(&json!({})), &ctx);
                let body = format!(
                    "{}.{tool}({})",
                    b.server_name(),
                    serde_json::to_string_pretty(&arguments).unwrap_or_default()
                );
                let p = McpCallPayload {
                    action: a.id.clone(),
                    store,
                    binding: b.server_name().to_string(),
                    tool,
                    arguments,
                    body: body.clone(),
                };
                Ok((serde_json::to_string(&p).unwrap_or_default(), body))
            }
            ActionKind::PrComment | ActionKind::PrLabels => {
                let repo = d
                    .trigger
                    .repo
                    .as_deref()
                    .map(|t| template::render(t, &ctx).trim().to_string())
                    .unwrap_or_default();
                let number: i64 = r(&a.target)
                    .trim_start_matches('#')
                    .parse()
                    .map_err(|_| skip("the target is not a pull request number"))?;
                if !super::github::repo_ok(&repo) {
                    return Err(skip("the trigger's repo is not owner/repo"));
                }
                if a.kind == ActionKind::PrComment {
                    let text = body_text();
                    if text.is_empty() {
                        return Err(skip("the body is empty"));
                    }
                    let mark = format!("\n\n{}", marker(instance));
                    let body = format!(
                        "{}{mark}",
                        fit(&text, max.saturating_sub(mark.chars().count()))
                    );
                    let p = PrCommentPayload {
                        action: a.id.clone(),
                        repo,
                        number,
                        body: body.clone(),
                    };
                    Ok((serde_json::to_string(&p).unwrap_or_default(), body))
                } else {
                    let all = labels_of(a.labels.as_ref(), &ctx);
                    let remove: Vec<String> = all
                        .iter()
                        .filter_map(|l| l.strip_prefix('-').map(str::to_string))
                        .collect();
                    let add: Vec<String> =
                        all.into_iter().filter(|l| !l.starts_with('-')).collect();
                    if add.is_empty() && remove.is_empty() {
                        return Err(skip("no label to change"));
                    }
                    let body = add
                        .iter()
                        .map(|l| format!("+{l}"))
                        .chain(remove.iter().map(|l| format!("−{l}")))
                        .collect::<Vec<_>>()
                        .join(" ");
                    let p = PrLabelsPayload {
                        action: a.id.clone(),
                        repo,
                        number,
                        add,
                        remove,
                        body: body.clone(),
                    };
                    Ok((serde_json::to_string(&p).unwrap_or_default(), body))
                }
            }
            ActionKind::PrReview => Err(skip(
                "pr_review is the PR reviewer's: not built for a fleet agent",
            )),
        })();
        match made {
            Ok((payload, body)) => {
                let auto = a.auto
                    && match a.kind {
                        ActionKind::McpCall => d
                            .stores
                            .iter()
                            .find(|s| Some(&s.role) == a.store.as_ref())
                            .and_then(|s| {
                                s.write_tools
                                    .iter()
                                    .find(|w| Some(w.name()) == a.tool.as_deref())
                            })
                            .is_some_and(|w| w.auto_ok()),
                        _ => true,
                    };
                planned.push(Planned {
                    id: a.id.clone(),
                    kind: a.kind.word(),
                    payload,
                    body,
                    auto,
                });
                if a.stop {
                    break;
                }
            }
            Err(u) => skipped.push(u),
        }
    }
    (planned, skipped)
}

/// PURE: what is wrong with `args` for a tool whose `inputSchema` is
/// `schema`: an object, its required properties there, no property the
/// schema closes out, each given property of its declared JSON type.
pub fn check_arguments(schema: &Value, args: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(obj) = args.as_object() else {
        return vec!["the arguments are not an object".into()];
    };
    let props = schema.get("properties").and_then(Value::as_object);
    for r in schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !obj.contains_key(r) {
            out.push(format!("{r} is required"));
        }
    }
    let closed = schema.get("additionalProperties") == Some(&Value::Bool(false));
    for (k, v) in obj {
        let Some(p) = props.and_then(|p| p.get(k)) else {
            if closed {
                out.push(format!("{k} is not a parameter"));
            }
            continue;
        };
        let types: Vec<&str> = match p.get("type") {
            Some(Value::String(t)) => vec![t.as_str()],
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
            _ => continue,
        };
        let is = |t: &str| match t {
            "string" => v.is_string(),
            "number" => v.is_number(),
            "integer" => v.is_i64() || v.is_u64(),
            "boolean" => v.is_boolean(),
            "object" => v.is_object(),
            "array" => v.is_array(),
            "null" => v.is_null(),
            _ => true,
        };
        if !types.iter().any(|t| is(t)) {
            out.push(format!("{k} is not {}", types.join(" or ")));
        }
    }
    out
}

// ── the executor ──

pub struct FleetExecutor {
    pub store: Arc<Mutex<Store>>,
    pub host: Arc<dyn FleetHost>,
}

fn refuse(what: impl std::fmt::Display) -> ActionResult {
    ActionResult::Failed(format!("refused: {what}"))
}

impl FleetExecutor {
    /// The rule and the job's subject behind an action.
    fn rule_and_subject(
        &self,
        action: &AgentActionRow,
    ) -> Result<Option<(AgentRuleRow, Value)>, String> {
        let s = self.store.lock().map_err(|_| "store lock".to_string())?;
        let job = s
            .agent_job(action.job_id)
            .map_err(|e| e.to_string())?
            .ok_or("the job is gone")?;
        let Some(id) = job.rule_id else {
            return Ok(None);
        };
        let Some(rule) = s.agent_rule(id).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let subject = job
            .result
            .as_deref()
            .and_then(|r| serde_json::from_str::<Value>(r).ok())
            .map(|v| v["subject"].clone())
            .unwrap_or(Value::Null);
        Ok(Some((rule, subject)))
    }

    async fn send(
        &self,
        action: &AgentActionRow,
        rule: &AgentRuleRow,
        inst: &Instance,
        subject: &Value,
        now: i64,
    ) -> ActionResult {
        let d = &inst.loaded().definition;
        let id_of = |v: &Value| {
            v.get("action")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let raw: Value = match serde_json::from_str(&action.payload) {
            Ok(v) => v,
            Err(e) => return ActionResult::Failed(format!("payload: {e}")),
        };
        let Some(def_action) = d
            .actions
            .iter()
            .find(|a| a.id == id_of(&raw) && a.kind.word() == action.kind)
        else {
            return ActionResult::Expired("the definition no longer has this action".into());
        };
        let ctx = inst.ctx(now);
        let projects = write_projects(inst, now);
        let subject_key = subject
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let in_limits =
            |key: &str| key == subject_key || projects.iter().any(|p| p == project_of(key));
        let max = d.limits.max_body_chars;
        let fits = |body: &str| body.chars().count() <= max;
        let tracker_ok = |t: i64| tracker_of(d, def_action, &inst.values, &ctx) == Some(t);
        macro_rules! payload {
            ($t:ty) => {
                match serde_json::from_value::<$t>(raw.clone()) {
                    Ok(p) => p,
                    Err(e) => return ActionResult::Failed(format!("payload: {e}")),
                }
            };
        }
        let write = |t: i64, op: WriteOp| async move { self.host.base().write(t, &op).await };
        match def_action.kind {
            ActionKind::TrackerComment => {
                let c = payload!(CommentPayload);
                if !tracker_ok(c.tracker) {
                    return refuse(format!("tracker {} is not the instance's", c.tracker));
                }
                if !in_limits(&c.key) {
                    return refuse(format!("{} is outside limits.write_projects", c.key));
                }
                if !fits(&c.body) {
                    return refuse(format!("the comment is over {max} characters"));
                }
                let op = if is_jira(self.host.tracker_provider(c.tracker).as_deref()) {
                    WriteOp::RichComment {
                        key: c.key,
                        adf: c.adf,
                        wiki: c.body,
                    }
                } else {
                    // GitHub, Linear, Asana: the same text, plain.
                    WriteOp::Comment {
                        key: c.key,
                        external_id: None,
                        body: c.body,
                    }
                };
                match write(c.tracker, op).await {
                    Ok(_) => ActionResult::Executed,
                    Err(e) => ActionResult::Failed(e),
                }
            }
            ActionKind::TrackerIssue => {
                let i = payload!(IssuePayload);
                if !tracker_ok(i.tracker) {
                    return refuse(format!("tracker {} is not the instance's", i.tracker));
                }
                if !projects.contains(&i.project) {
                    return refuse(format!("{} is not in limits.write_projects", i.project));
                }
                if !fits(&i.body) {
                    return refuse(format!("the description is over {max} characters"));
                }
                if let Some(l) = &i.link {
                    if !in_limits(&l.to) {
                        return refuse(format!("{} is outside limits.write_projects", l.to));
                    }
                }
                let jira = is_jira(self.host.tracker_provider(i.tracker).as_deref());
                if !jira {
                    if i.link.is_some() {
                        return refuse("an issue link is Jira's: this tracker has none");
                    }
                    return match write(
                        i.tracker,
                        WriteOp::CreateItem {
                            container: Some(i.project.clone()),
                            parent: None,
                            title: i.summary.clone(),
                            description: i.body.clone(),
                        },
                    )
                    .await
                    {
                        Ok(_) => ActionResult::Executed,
                        Err(e) => ActionResult::Failed(e),
                    };
                }
                let created = match write(
                    i.tracker,
                    WriteOp::CreateIssue {
                        project: i.project.clone(),
                        issue_type: i.issue_type.clone(),
                        summary: i.summary.clone(),
                        adf: i.adf.clone(),
                        wiki: i.body.clone(),
                        labels: i.labels.clone(),
                    },
                )
                .await
                {
                    Ok(o) => o,
                    Err(e) => return ActionResult::Failed(e),
                };
                let Some(l) = i.link else {
                    return ActionResult::Executed;
                };
                let Some(new_key) = created.remote_id.or(created.key) else {
                    return ActionResult::Failed(
                        "the issue was created, its key is unknown: not linked".into(),
                    );
                };
                match write(
                    i.tracker,
                    WriteOp::LinkIssues {
                        link_type: l.link_type,
                        inward: new_key.clone(),
                        outward: l.to,
                    },
                )
                .await
                {
                    Ok(_) => ActionResult::Executed,
                    Err(e) => ActionResult::Failed(format!(
                        "{new_key} was created; linking it failed: {e}"
                    )),
                }
            }
            ActionKind::TrackerLink => {
                let l = payload!(LinkPayload);
                if !tracker_ok(l.tracker) {
                    return refuse(format!("tracker {} is not the instance's", l.tracker));
                }
                if !in_limits(&l.from) || !in_limits(&l.to) {
                    return refuse(format!(
                        "{} or {} is outside limits.write_projects",
                        l.from, l.to
                    ));
                }
                if !is_jira(self.host.tracker_provider(l.tracker).as_deref()) {
                    return refuse("an issue link is Jira's: this tracker has none");
                }
                match write(
                    l.tracker,
                    WriteOp::LinkIssues {
                        link_type: l.link_type,
                        inward: l.from,
                        outward: l.to,
                    },
                )
                .await
                {
                    Ok(_) => ActionResult::Executed,
                    Err(e) => ActionResult::Failed(e),
                }
            }
            ActionKind::Record => {
                let p = payload!(RecordPayload);
                let own = bindings::namespaces(rule.id, &inst.bindings);
                let fleet_role = d
                    .stores
                    .iter()
                    .any(|s| s.role == p.store && s.kind == StoreKind::Fleet);
                if !fleet_role || own.get(&p.store) != Some(&p.namespace) {
                    return refuse(format!(
                        "{} is not the rule's namespace for {}",
                        p.namespace, p.store
                    ));
                }
                let s = match self.store.lock() {
                    Ok(s) => s,
                    Err(_) => return ActionResult::Failed("store lock".into()),
                };
                match s.put_agent_record(
                    &NewAgentRecord {
                        namespace: p.namespace,
                        kind: "record".into(),
                        key: p.key,
                        value: p.value.to_string(),
                        expires_at: p.expires_secs.map(|e| now + e),
                        written_by_job: Some(action.job_id),
                    },
                    now,
                ) {
                    Ok(()) => ActionResult::Executed,
                    Err(e) => ActionResult::Failed(e.to_string()),
                }
            }
            ActionKind::McpCall => {
                let p = payload!(McpCallPayload);
                let Some(role) = d
                    .stores
                    .iter()
                    .find(|s| s.role == p.store && s.kind == StoreKind::Mcp)
                else {
                    return refuse(format!("{} is not an mcp store", p.store));
                };
                if !role.write_tools.iter().any(|w| w.name() == p.tool)
                    || def_action.tool.as_deref() != Some(p.tool.as_str())
                {
                    return refuse(format!("{} is not a write tool of {}", p.tool, p.store));
                }
                if p.arguments.to_string().len() > MCP_ARGS_MAX {
                    return refuse(format!(
                        "the arguments are over {} KiB",
                        MCP_ARGS_MAX / 1024
                    ));
                }
                let Some(b) = inst.bindings.iter().find(|b| b.role == p.store) else {
                    return refuse(format!("{} is not bound", p.store));
                };
                if b.server_name() != p.binding {
                    return refuse("the binding changed since it was planned");
                }
                let Some(spec) = inst
                    .sources
                    .servers
                    .get(b.server.as_deref().unwrap_or_default())
                else {
                    return ActionResult::Failed("the server is not in the catalog".into());
                };
                let secrets = match self.host.secrets(&rule.host_alias) {
                    Ok(s) => s,
                    Err(e) => return ActionResult::Failed(e),
                };
                let entry = match bindings::render_entry(spec, &b.env, &secrets) {
                    Ok(e) => e,
                    Err(e) => return ActionResult::Failed(e),
                };
                let tools = match self.host.list_tools(&rule.host_alias, &entry).await {
                    Ok(t) => t,
                    Err(e) => return ActionResult::Failed(format!("tools/list: {e}")),
                };
                let Some(def) = tools
                    .iter()
                    .find(|t| t.get("name").and_then(Value::as_str) == Some(p.tool.as_str()))
                else {
                    return refuse(format!("the server no longer lists {}", p.tool));
                };
                let problems =
                    check_arguments(def.get("inputSchema").unwrap_or(&json!({})), &p.arguments);
                if !problems.is_empty() {
                    return refuse(format!("arguments: {}", problems.join("; ")));
                }
                let line = match self
                    .host
                    .call_tool(&rule.host_alias, &entry, &p.tool, &p.arguments)
                    .await
                {
                    Ok(l) => l,
                    Err(e) => return ActionResult::Failed(e),
                };
                match crate::service::agents::support::scan::tool_text(&line) {
                    Ok(_) => ActionResult::Executed,
                    Err(e) => ActionResult::Failed(e),
                }
            }
            ActionKind::PrComment | ActionKind::PrLabels => {
                let repos = write_repos(inst, now);
                let (repo, number) = match raw.get("repo").and_then(Value::as_str) {
                    Some(r) => (
                        r.to_string(),
                        raw.get("number").and_then(Value::as_i64).unwrap_or(0),
                    ),
                    None => return ActionResult::Failed("payload: no repo".into()),
                };
                if !repos.contains(&repo) {
                    return refuse(format!(
                        "{repo} is neither the trigger's repo nor in limits.repos"
                    ));
                }
                if def_action.kind == ActionKind::PrComment {
                    let c = payload!(PrCommentPayload);
                    if !fits(&c.body) {
                        return refuse(format!("the comment is over {max} characters"));
                    }
                    match self
                        .host
                        .pr_comment(&rule.host_alias, &repo, number, &c.body)
                        .await
                    {
                        Ok(()) => ActionResult::Executed,
                        Err(e) => ActionResult::Failed(e),
                    }
                } else {
                    let l = payload!(PrLabelsPayload);
                    let req = crate::service::agents::review::labels::LabelsRequest {
                        host: rule.host_alias.clone(),
                        repo,
                        number,
                        sha: subject
                            .get("head_sha")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        add: l.add,
                        remove: l.remove,
                    };
                    match self.host.set_pr_labels(&req).await {
                        Ok(()) => ActionResult::Executed,
                        Err(e) => ActionResult::Failed(e),
                    }
                }
            }
            ActionKind::PrReview => {
                ActionResult::Failed("pr_review is not built for a fleet agent".into())
            }
        }
    }
}

#[async_trait::async_trait]
impl AgentExecutor for FleetExecutor {
    async fn execute(&self, action: &AgentActionRow) -> ActionResult {
        let now = crate::store::now_unix();
        let (rule, subject) = match self.rule_and_subject(action) {
            Ok(Some(x)) => x,
            Ok(None) => return ActionResult::Expired("the rule was revoked".into()),
            Err(e) => return ActionResult::Failed(e),
        };
        if !rule.active_at(now) {
            return ActionResult::Expired("the rule has expired".into());
        }
        if rule.level == "watch" {
            return ActionResult::Expired("the rule is at watch now".into());
        }
        let inst = match self
            .host
            .sources(&rule)
            .and_then(|src| super::runner::instance(&rule, src))
        {
            Ok(i) => i,
            Err(e) => return ActionResult::Failed(e),
        };
        self.send(action, &rule, &inst, &subject, now).await
    }
}
