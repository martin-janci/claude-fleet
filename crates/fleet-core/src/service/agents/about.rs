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
    /// Actions with the same key are the same agent's calls of the same
    /// tool (`<rule>:<binding>.<tool>`): a client shows them as one card.
    /// Any other kind is `id:<n>`, a card of its own.
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

/// A scalar argument's value shown in a title up to this many characters;
/// a longer one, or an object or a list, is named only.
const SHORT_VALUE: usize = 30;

/// PURE: `v` when it is a short one-line scalar.
fn short_value(v: &Value) -> Option<String> {
    let s = match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return None,
    };
    (!s.is_empty() && !s.contains('\n') && s.chars().count() <= SHORT_VALUE).then_some(s)
}

/// PURE: what a call changes and with what: the keys of its `fields`
/// object, then its other arguments but the target, each with its value
/// when that is a short scalar (`transition_id=31`).
fn changed_fields(args: &Map<String, Value>, target: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = args
        .get("fields")
        .and_then(Value::as_object)
        .map(|f| f.keys().cloned().collect())
        .unwrap_or_default();
    for (k, v) in args {
        if k == "fields" || Some(k.as_str()) == target || out.contains(k) {
            continue;
        }
        out.push(match short_value(v) {
            Some(s) => format!("{k}={s}"),
            None => k.clone(),
        });
    }
    out
}

/// PURE: "Jira: update issue PD-2709 (status, labels)", "Jira: transition
/// issue PD-1 (resolution, comment, transition_id=31)".
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
    let agent = origin.map_or("agent", |o| o.agent.as_str());
    // Only tool calls fold: the same tool of the same server is the same
    // write in general. Any other kind names the write in its payload alone
    // (two `post_review`s are two pull requests), so each is its own card.
    let (title, what, group) = if a.kind == "mcp_call" {
        let binding = p.get("binding").and_then(Value::as_str).unwrap_or_default();
        let tool = p.get("tool").and_then(Value::as_str).unwrap_or_default();
        let by = origin
            .and_then(|o| o.rule_id)
            .map_or_else(|| agent.to_string(), |r| r.to_string());
        (
            Some(mcp_title(
                binding,
                tool,
                p.get("arguments").unwrap_or(&Value::Null),
            )),
            tool_words(binding, tool),
            format!("{by}:{binding}.{tool}"),
        )
    } else {
        (None, a.kind.replace('_', " "), format!("id:{}", a.id))
    };
    ActionAbout {
        id: a.id,
        from: agent_words(agent, rule),
        subject: origin.and_then(|o| subject_words(&o.subject)),
        title,
        what,
        group,
    }
}

/// PURE: which write a proposal is, apart from its wording: two pending
/// proposals of one rule with the same identity change the same thing, so
/// the newer one replaces the older (`queue_actions`). A kind it does not
/// know is identified by its whole payload. A payload carrying the
/// definition's `fingerprint` is identified by it, whatever its wording.
pub fn identity(kind: &str, payload: &Value) -> String {
    if let Some(fp) = fingerprint(payload) {
        return format!("fp:{kind}:{fp}");
    }
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
            format!("mcp:{action}:{}.{}:{item}", f("binding"), f("tool"))
        }
        "tracker_comment" => format!("comment:{action}:{}", f("key")),
        "tracker_link" => format!("link:{action}:{}:{}:{}", f("from"), f("to"), f("link_type")),
        "tracker_issue" => format!("issue:{action}:{}:{}", f("project"), f("summary")),
        "record" => format!("record:{action}:{}:{}", f("namespace"), f("key")),
        _ => format!("{kind}:{payload}"),
    }
}

/// PURE: the finding a payload says it is (`actions::with_fingerprint`),
/// when it carries one.
fn fingerprint(payload: &Value) -> Option<String> {
    payload.get("fingerprint").and_then(text)
}

/// PURE: whether two proposals of one kind are the very same proposal: the
/// same fingerprint when both carry one (an LLM rewords a finding every
/// run), else the same payload.
pub fn same_proposal(a: &Value, b: &Value) -> bool {
    match (fingerprint(a), fingerprint(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
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
            "Jira: update issue PD-1 (a=1, b=2, c=3, d=4 and 1 more)"
        );
        // The other arguments of a call with `fields` are named too, with
        // their values when short.
        let transition = json!({ "issue_key": "PD-1", "transition_id": "31",
            "fields": { "resolution": { "name": "Fixed" } },
            "comment": "Fixed in #42, released in 1.4, see the notes there" });
        assert_eq!(
            mcp_title("jira", "jira_transition_issue", &transition),
            "Jira: transition issue PD-1 (resolution, comment, transition_id=31)"
        );
        assert_eq!(
            mcp_title(
                "jira",
                "jira_update_issue",
                &json!({ "issue_key": "PD-1", "fields": {} })
            ),
            "Jira: update issue PD-1"
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
        assert_eq!(x.group, "4:jira.jira_update_issue");
        // A built-in agent's action keeps the client's own title, and is
        // never folded with another: two reviews are two pull requests.
        let r = about(&action("post_review", json!({})), None, None);
        assert_eq!(r.title, None);
        assert_eq!(r.group, "id:7");
        let s = about(&action("setting", json!({})), Some(&origin), Some(&rule));
        assert_eq!(s.group, "id:7");
    }

    #[test]
    fn a_fingerprint_identifies_a_proposal_whatever_its_wording() {
        let p = |body: &str, fp: &str| json!({ "body": body, "fingerprint": fp });
        assert_eq!(
            identity("fix_session", &p("Restart it", "f1")),
            identity("fix_session", &p("Please restart", "f1"))
        );
        assert_ne!(
            identity("fix_session", &p("Restart it", "f1")),
            identity("fix_session", &p("Restart it", "f2"))
        );
        assert!(same_proposal(&p("a", "f1"), &p("b", "f1")));
        assert!(!same_proposal(&p("a", "f1"), &p("a", "f2")));
        assert!(same_proposal(&json!({ "x": 1 }), &json!({ "x": 1 })));
        assert!(!same_proposal(&json!({ "x": 1 }), &json!({ "x": 2 })));
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
