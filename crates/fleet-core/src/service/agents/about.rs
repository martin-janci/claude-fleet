//! What a waiting agent action is, in words a person reads before answering
//! it (`status.pending_about`), and which earlier proposal it repeats
//! ([`identity`]).
//!
//! A fleet agent's `mcp_call` names only a server and a tool
//! (`jira.jira_update_issue`), so a list of them read "Call
//! jira.jira_update_issue" seven times over with nothing to tell them apart
//! (Martin, 2026-11-06). [`about`] says which issue each changes and which
//! fields, which agent asked and on what, and gives the clients a `group`
//! key so the same agent's proposals of the same write fold into one card.
//!
//! PURE: the caller reads the rows.

use crate::store::{ActionOrigin, AgentActionRow, AgentRuleRow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One waiting action, explained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionAbout {
    /// The action's id.
    pub id: i64,
    /// The rule whose job proposed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<i64>,
    /// The agent that asked: "PR reviewer", "jira-triage · PD board".
    pub from: String,
    /// What its run was about: "PD-2709", "a scheduled run".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// What it does, when the hub can say it better than the client's own
    /// title (an `mcp_call`): "Jira: update issue PD-2709 (status, labels)".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The same write in general, for a group's heading: "Jira: update issue".
    pub what: String,
    /// Actions with the same key are the same agent's proposals of the same
    /// write: a client shows them as one card.
    pub group: String,
}

/// The argument names that say which item a tool call is about, first
/// match wins.
const TARGET_ARGS: &[&str] = &[
    "issue_key",
    "issueKey",
    "issue_id_or_key",
    "issueIdOrKey",
    "issue_id",
    "issueId",
    "key",
    "issue",
    "ticket",
    "page_id",
    "pageId",
    "pull_number",
    "number",
    "id",
];

/// The argument names that say which container the item is in (a repo, a
/// Jira site or project): `#12` of `o/api` and of `o/web` are different
/// items, so they are part of a call's identity.
const CONTAINER_ARGS: &[&str] = &[
    "owner",
    "repo",
    "project",
    "projectKey",
    "project_key",
    "cloudId",
    "cloud_id",
    "workspace",
    "org",
    "site",
];

fn parse(payload: &str) -> Value {
    serde_json::from_str(payload).unwrap_or(Value::Null)
}

fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// PURE: the argument naming the call's item, and its value.
fn target(args: &Map<String, Value>) -> Option<(&'static str, String)> {
    TARGET_ARGS
        .iter()
        .find_map(|k| args.get(*k).and_then(text).map(|v| (*k, v)))
}

/// PURE: "Jira: update issue" for server `jira`, tool `jira_update_issue`.
pub fn tool_words(binding: &str, tool: &str) -> String {
    let lower = tool.to_ascii_lowercase();
    let b = binding.to_ascii_lowercase();
    let rest = lower
        .strip_prefix(&format!("{b}_"))
        .or_else(|| lower.strip_prefix(&format!("{b}.")))
        .unwrap_or(lower.as_str());
    let words = rest.replace(['_', '-', '.'], " ");
    let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    let server = {
        let mut c = binding.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => String::new(),
        }
    };
    match (server.is_empty(), words.is_empty()) {
        (true, _) => words,
        (false, true) => server,
        (false, false) => format!("{server}: {words}"),
    }
}

/// PURE: the fields a call changes: the keys of its `fields` object when it
/// has one, else its other arguments, without the target.
fn changed_fields(args: &Map<String, Value>, target: Option<&str>) -> Vec<String> {
    let names: Vec<String> = match args.get("fields").and_then(Value::as_object) {
        Some(f) => f.keys().cloned().collect(),
        None => args
            .keys()
            .filter(|k| Some(k.as_str()) != target)
            .cloned()
            .collect(),
    };
    let mut out: Vec<String> = Vec::new();
    for n in names {
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

/// PURE: "Jira: update issue PD-2709 (status, labels)".
pub fn mcp_title(binding: &str, tool: &str, args: &Value) -> String {
    let mut t = tool_words(binding, tool);
    let Some(args) = args.as_object() else {
        return t;
    };
    let target = target(args);
    if let Some((_, v)) = &target {
        t.push(' ');
        t.push_str(v);
    }
    let fields = changed_fields(args, target.as_ref().map(|(k, _)| *k));
    if !fields.is_empty() {
        const SHOWN: usize = 4;
        let more = fields.len().saturating_sub(SHOWN);
        let mut list = fields
            .into_iter()
            .take(SHOWN)
            .collect::<Vec<_>>()
            .join(", ");
        if more > 0 {
            list.push_str(&format!(" and {more} more"));
        }
        t.push_str(&format!(" ({list})"));
    }
    t
}

/// PURE: a job subject in words: `tracker:3:PD-1` is "PD-1", a schedule's
/// tick "a scheduled run".
pub fn subject_words(subject: &str) -> Option<String> {
    let s = subject.trim();
    if s.is_empty() {
        return None;
    }
    if s.starts_with("schedule:") {
        return Some("a scheduled run".into());
    }
    if s.starts_with("manual:") {
        return Some("a run you started".into());
    }
    if let Some(rest) = s.strip_prefix("tracker:") {
        return Some(rest.split_once(':').map_or(rest, |(_, k)| k).to_string());
    }
    if let Some(rest) = s.strip_prefix("scan:") {
        return Some(rest.to_string());
    }
    Some(s.to_string())
}

/// PURE: the agent in words, as the clients' `ruleAgentName`, with the
/// instance's scope when it says more.
pub fn agent_words(agent: &str, rule: Option<&AgentRuleRow>) -> String {
    let name = match agent {
        "review" => "PR reviewer".to_string(),
        "support" => "Support".to_string(),
        _ => rule
            .and_then(|r| r.definition.clone())
            .unwrap_or_else(|| agent.to_string()),
    };
    match rule.map(|r| r.scope.trim()).filter(|s| !s.is_empty()) {
        Some(scope) if scope != name && agent == "fleet" => format!("{name} · {scope}"),
        _ => name,
    }
}

/// PURE: action `a` explained, with where it came from and its rule.
pub fn about(
    a: &AgentActionRow,
    origin: Option<&ActionOrigin>,
    rule: Option<&AgentRuleRow>,
) -> ActionAbout {
    let p = parse(&a.payload);
    let (title, what) = if a.kind == "mcp_call" {
        let binding = p.get("binding").and_then(Value::as_str).unwrap_or_default();
        let tool = p.get("tool").and_then(Value::as_str).unwrap_or_default();
        let args = p.get("arguments").cloned().unwrap_or(Value::Null);
        (
            Some(mcp_title(binding, tool, &args)),
            tool_words(binding, tool),
        )
    } else {
        (None, a.kind.replace('_', " "))
    };
    let rule_id = origin.and_then(|o| o.rule_id);
    let agent = origin.map_or("agent", |o| o.agent.as_str());
    let tool = p.get("tool").and_then(Value::as_str).unwrap_or_default();
    ActionAbout {
        id: a.id,
        rule_id,
        from: agent_words(agent, rule),
        subject: origin.and_then(|o| subject_words(&o.subject)),
        title,
        what,
        group: format!(
            "{}:{}:{}",
            rule_id.map_or_else(|| agent.to_string(), |r| r.to_string()),
            a.kind,
            tool
        ),
    }
}

/// PURE: which write a proposal is, apart from its wording: two pending
/// proposals of one rule with the same identity change the same thing, so
/// the newer one replaces the older (`queue_actions`). A kind it does not
/// know is identified by its whole payload.
pub fn identity(kind: &str, payload: &Value) -> String {
    let f = |k: &str| payload.get(k).and_then(text).unwrap_or_default();
    let action = f("action");
    match kind {
        "mcp_call" => {
            let args = payload.get("arguments");
            let item = args
                .and_then(Value::as_object)
                .and_then(target)
                .map(|(_, v)| v)
                // No item argument: only the very same call is the same write.
                .unwrap_or_else(|| args.map(Value::to_string).unwrap_or_default());
            let within: String = args
                .and_then(Value::as_object)
                .map(|a| {
                    CONTAINER_ARGS
                        .iter()
                        .filter_map(|k| a.get(*k).and_then(text).map(|v| format!("{k}={v};")))
                        .collect()
                })
                .unwrap_or_default();
            format!("mcp:{action}:{}.{}:{within}{item}", f("binding"), f("tool"))
        }
        "tracker_comment" => format!("comment:{action}:{}", f("key")),
        "tracker_link" => format!("link:{action}:{}:{}:{}", f("from"), f("to"), f("link_type")),
        "tracker_issue" => format!("issue:{action}:{}:{}", f("project"), f("summary")),
        "record" => format!("record:{action}:{}:{}", f("namespace"), f("key")),
        _ => format!("{kind}:{payload}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn action(kind: &str, payload: Value) -> AgentActionRow {
        AgentActionRow {
            id: 7,
            job_id: 3,
            kind: kind.into(),
            payload: payload.to_string(),
            state: "pending".into(),
            decided_by: None,
            decided_at: None,
            executed_at: None,
            error: None,
            created_at: 0,
        }
    }

    #[test]
    fn a_tool_reads_as_words_without_its_servers_prefix() {
        assert_eq!(
            tool_words("jira", "jira_update_issue"),
            "Jira: update issue"
        );
        assert_eq!(
            tool_words("confluence", "update-page"),
            "Confluence: update page"
        );
        assert_eq!(tool_words("", "do_it"), "do it");
    }

    #[test]
    fn an_mcp_title_names_the_issue_and_what_changes() {
        let args =
            json!({ "issue_key": "PD-2709", "fields": { "status": "Done", "labels": ["x"] } });
        assert_eq!(
            mcp_title("jira", "jira_update_issue", &args),
            "Jira: update issue PD-2709 (labels, status)"
        );
        let flat = json!({ "key": "PD-1", "a": 1, "b": 2, "c": 3, "d": 4, "e": 5 });
        assert_eq!(
            mcp_title("jira", "jira_update_issue", &flat),
            "Jira: update issue PD-1 (a, b, c, d and 1 more)"
        );
    }

    #[test]
    fn a_subject_reads_as_its_key_or_its_kind_of_run() {
        assert_eq!(subject_words("tracker:3:PD-1").as_deref(), Some("PD-1"));
        assert_eq!(
            subject_words("schedule:1700000000").as_deref(),
            Some("a scheduled run")
        );
        assert_eq!(
            subject_words("manual:1").as_deref(),
            Some("a run you started")
        );
        assert_eq!(subject_words("scan:abc").as_deref(), Some("abc"));
        assert_eq!(subject_words(" "), None);
    }

    #[test]
    fn about_says_who_asked_and_groups_by_rule_and_tool() {
        let a = action(
            "mcp_call",
            json!({ "action": "move", "store": "jira", "binding": "jira", "tool": "jira_update_issue",
                    "arguments": { "issue_key": "PD-9", "fields": { "status": "Done" } }, "body": "" }),
        );
        let origin = ActionOrigin {
            action_id: 7,
            rule_id: Some(4),
            agent: "fleet".into(),
            subject: "schedule:1".into(),
            trigger: "schedule".into(),
        };
        let rule = AgentRuleRow {
            id: 4,
            agent: "fleet".into(),
            scope: "PD board".into(),
            level: "propose".into(),
            host_alias: "local".into(),
            project_id: None,
            config: "{}".into(),
            granted_by: "x".into(),
            granted_at: 0,
            expires_at: None,
            definition: Some("jira-triage".into()),
            bindings: "[]".into(),
        };
        let x = about(&a, Some(&origin), Some(&rule));
        assert_eq!(x.title.as_deref(), Some("Jira: update issue PD-9 (status)"));
        assert_eq!(x.from, "jira-triage · PD board");
        assert_eq!(x.subject.as_deref(), Some("a scheduled run"));
        assert_eq!(x.what, "Jira: update issue");
        assert_eq!(x.group, "4:mcp_call:jira_update_issue");
        // A built-in agent's action keeps the client's own title.
        let r = about(&action("post_review", json!({})), None, None);
        assert_eq!(r.title, None);
    }

    #[test]
    fn the_same_write_on_the_same_item_has_one_identity() {
        let call = |key: &str, status: &str| {
            json!({ "action": "move", "binding": "jira", "tool": "jira_update_issue",
                    "arguments": { "issue_key": key, "fields": { "status": status } } })
        };
        assert_eq!(
            identity("mcp_call", &call("PD-1", "Done")),
            identity("mcp_call", &call("PD-1", "In review"))
        );
        assert_ne!(
            identity("mcp_call", &call("PD-1", "Done")),
            identity("mcp_call", &call("PD-2", "Done"))
        );
        // The same number in another repo is another item (review R6).
        let review = |owner: &str, repo: &str, body: &str| {
            json!({ "action": "review", "binding": "gh", "tool": "review",
                    "arguments": { "owner": owner, "repo": repo, "pull_number": 12, "body": body } })
        };
        assert_ne!(
            identity("mcp_call", &review("o", "api", "x")),
            identity("mcp_call", &review("o", "web", "x"))
        );
        assert_eq!(
            identity("mcp_call", &review("o", "api", "x")),
            identity("mcp_call", &review("o", "api", "reworded"))
        );
        // Without an item argument only the very same arguments match.
        let bare = |q: &str| json!({ "action": "a", "binding": "b", "tool": "t", "arguments": { "q": q } });
        assert_ne!(
            identity("mcp_call", &bare("x")),
            identity("mcp_call", &bare("y"))
        );
        assert_eq!(
            identity("mcp_call", &bare("x")),
            identity("mcp_call", &bare("x"))
        );
        assert_eq!(
            identity(
                "tracker_comment",
                &json!({ "action": "c", "key": "PD-1", "body": "a" })
            ),
            identity(
                "tracker_comment",
                &json!({ "action": "c", "key": "PD-1", "body": "b" })
            )
        );
    }
}
