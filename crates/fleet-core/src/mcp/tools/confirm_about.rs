//! A waiting call in words (`guard::ConfirmAbout`): what Approve does, for
//! the person who is asked. The bound summary (`guard::ConfirmRequest::summary`)
//! is the exact call the approval holds to, written for a replay check
//! (`key="PD-2709"#637115e13a2a552c url=- item_id=None …`); nobody can tell
//! from it what will happen. This reads the same call's arguments, names
//! the ticket, host, project and session the way the app shows them, puts
//! the brief or prompt itself on the card, and lists only the options the
//! caller set. It is display only: an approval still binds to the summary.

use serde_json::Value;

use crate::mcp::guard::{ConfirmAbout, ConfirmBody, ConfirmFact};
use crate::store::Store;

/// Characters of a brief or prompt put on the card; the rest is cut with
/// an ellipsis (the whole text is bound by its digest in the summary).
pub(super) const BODY_MAX_CHARS: usize = 4000;

/// `tool`'s call with `args` in words, read against `store`. `None` for an
/// object it cannot read (a call that failed to parse is refused anyway).
pub(super) fn describe(store: &Store, tool: &str, args: &Value) -> Option<ConfirmAbout> {
    let a = args.as_object()?;
    let mut d = Describer {
        store,
        args: a,
        facts: Vec::new(),
    };
    let (title, body) = match tool {
        "work_link" => d.work_link(),
        "new_session" | "new_shell_session" => d.new_session(tool),
        "send_prompt" | "queue_prompt" | "run_prompt" => d.prompt(tool),
        "dispatch_task" => d.dispatch_task(),
        "safe_kill_session"
        | "kill_session"
        | "restart_session"
        | "recreate_session"
        | "rewind_conversation"
        | "fork_session" => d.session_op(tool),
        _ => d.generic(tool),
    };
    Some(ConfirmAbout {
        title,
        facts: d.facts,
        body,
    })
}

struct Describer<'a> {
    store: &'a Store,
    args: &'a serde_json::Map<String, Value>,
    facts: Vec<ConfirmFact>,
}

impl<'a> Describer<'a> {
    // The argument readers hand out `'a` borrows of the arguments, not of
    // `self`, so a value read stays usable across `self.fact(..)`.
    fn str(&self, k: &str) -> Option<&'a str> {
        let args: &'a serde_json::Map<String, Value> = self.args;
        args.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    fn int(&self, k: &str) -> Option<i64> {
        self.args.get(k).and_then(Value::as_i64)
    }

    fn flag(&self, k: &str) -> Option<bool> {
        self.args.get(k).and_then(Value::as_bool)
    }

    fn fact(&mut self, label: &str, value: impl Into<String>) {
        self.facts.push(ConfirmFact {
            label: label.into(),
            value: value.into(),
            url: None,
        });
    }

    fn project_label(&self, id: i64) -> String {
        match self.store.get_project(id) {
            Ok(Some(p)) => format!("{}/{}", p.owner, p.repo),
            _ => format!("#{id}"),
        }
    }

    /// The session `session_id` (or `host_alias` + `tmux_name`/`name`)
    /// names, as the app shows it: its friendly name, else its tmux name.
    fn session(&self, id_key: &str) -> Option<(String, String)> {
        let row = match self.int(id_key) {
            Some(id) => self.store.get_session_by_id(id).ok().flatten(),
            None => {
                let host = self.str("host_alias")?;
                let name = self.str("tmux_name").or_else(|| self.str("name"))?;
                self.store.get_session(name, host).ok().flatten()
            }
        };
        match row {
            Some(r) => Some((
                r.friendly_name
                    .filter(|f| !f.trim().is_empty())
                    .unwrap_or(r.tmux_name),
                r.host_alias,
            )),
            None => self.int(id_key).map(|id| (format!("#{id}"), String::new())),
        }
    }

    fn session_facts(&mut self, id_key: &str, label: &str) -> Option<String> {
        let (name, host) = self.session(id_key)?;
        self.fact(label, name.clone());
        if !host.is_empty() {
            self.fact("Host", host);
        }
        Some(name)
    }

    fn body(&self, label: &str, key: &str) -> Option<ConfirmBody> {
        let text = self.args.get(key).and_then(Value::as_str)?;
        if text.trim().is_empty() {
            return None;
        }
        let mut cut: String = text.chars().take(BODY_MAX_CHARS).collect();
        if cut.len() < text.len() {
            cut.push('…');
        }
        Some(ConfirmBody {
            label: label.into(),
            text: cut,
        })
    }

    /// `work_link { start | resume | run }`: the only actions the operator
    /// is asked about.
    fn work_link(&mut self) -> (String, Option<ConfirmBody>) {
        let action = self.str("action").unwrap_or("start").to_string();
        let item = self
            .int("item_id")
            .and_then(|id| self.store.get_work_item(id).ok().flatten())
            .or_else(|| {
                self.str("key")
                    .and_then(|k| self.store.work_item_by_key(k).ok().flatten())
            });
        let key = item
            .as_ref()
            .and_then(|i| i.key.clone())
            .or_else(|| self.str("key").map(str::to_string));
        let what = match (&key, &item) {
            (Some(k), _) => k.clone(),
            (None, Some(i)) => i.title.clone(),
            (None, None) => self.str("url").unwrap_or("a ticket").to_string(),
        };
        let url_arg = self.str("url").map(str::to_string);
        match &item {
            Some(i) => {
                let value = match &key {
                    Some(k) if !i.title.is_empty() => format!("{k} {}", i.title),
                    Some(k) => k.clone(),
                    None => i.title.clone(),
                };
                self.facts.push(ConfirmFact {
                    label: "Ticket".into(),
                    value,
                    url: i.url.clone().or(url_arg),
                });
            }
            None => {
                self.facts.push(ConfirmFact {
                    label: "Ticket".into(),
                    value: what.clone(),
                    url: url_arg,
                });
            }
        }
        if let Some(h) = self.str("host_alias") {
            self.fact("Host", h.to_string());
        } else if action != "resume" {
            self.fact("Host", "fleet picks one");
        }
        let mut projects: Vec<i64> = self.int("project_id").into_iter().collect();
        if let Some(ids) = self.args.get("project_ids").and_then(Value::as_array) {
            projects.extend(ids.iter().filter_map(Value::as_i64));
        }
        let mut seen = std::collections::HashSet::new();
        projects.retain(|p| seen.insert(*p));
        if !projects.is_empty() {
            let labels: Vec<String> = projects.iter().map(|p| self.project_label(*p)).collect();
            let label = if labels.len() > 1 {
                "Projects"
            } else {
                "Project"
            };
            self.fact(label, labels.join(", "));
        }
        if let Some(n) = self.str("name") {
            self.fact("Session name", n.to_string());
        }
        if let Some(w) = self.str("worktree") {
            self.fact("Worktree", w.to_string());
        }
        if let Some(m) = self.str("mode") {
            let words = match m {
                "last" => "continue the last conversation",
                "brief" => "a new conversation, briefed",
                "fresh" => "a new conversation, no brief",
                other => other,
            };
            self.fact("Conversation", words);
        }
        if let Some(r) = self.str("role") {
            self.fact("Role", r.to_string());
        }
        if self.flag("parallel") == Some(true) {
            self.fact("Parallel", "beside the session already working on it");
        }
        if self.flag("force_cross_org") == Some(true) {
            self.fact(
                "Across orgs",
                "yes, the ticket and project are in different orgs",
            );
        }
        let body = self.body("Brief", "brief");
        if body.is_none() && self.flag("with_brief") == Some(true) {
            self.fact("Brief", "the ticket's own text");
        } else if body.is_none() && self.flag("with_brief") == Some(false) {
            self.fact("Brief", "none");
        }
        let title = match action.as_str() {
            "resume" => format!("Resume the session for {what}"),
            "run" => format!("Start a session to run {what}"),
            _ => format!("Start a session for {what}"),
        };
        (title, body)
    }

    fn new_session(&mut self, tool: &str) -> (String, Option<ConfirmBody>) {
        let shell = tool == "new_shell_session" || self.str("kind") == Some("shell");
        if let Some(h) = self.str("host_alias") {
            self.fact("Host", h.to_string());
        }
        if let Some(p) = self.int("project_id") {
            let label = self.project_label(p);
            self.fact("Project", label);
        }
        if let Some(n) = self.str("friendly_name").or_else(|| self.str("name")) {
            self.fact("Session name", n.to_string());
        }
        if let Some(w) = self.str("new_worktree") {
            let base = self
                .str("base_branch")
                .map(|b| format!(" from {b}"))
                .unwrap_or_default();
            self.fact("Worktree", format!("new: {w}{base}"));
        }
        for (k, label) in [
            ("model", "Model"),
            ("effort", "Effort"),
            ("profile", "Account"),
            ("agent", "Agent"),
            ("resume_claude_session_id", "Resumes conversation"),
        ] {
            if let Some(v) = self.str(k) {
                self.fact(label, v.to_string());
            }
        }
        if self.flag("over_limit_ok") == Some(true) {
            self.fact("Usage limit", "start even over the limit");
        }
        let host = self.str("host_alias").unwrap_or("a host").to_string();
        let title = if shell {
            format!("Open a shell on {host}")
        } else {
            format!("Start a Claude session on {host}")
        };
        (title, self.body("Runs", "start_command"))
    }

    fn prompt(&mut self, tool: &str) -> (String, Option<ConfirmBody>) {
        let name = self
            .session_facts("session_id", "Session")
            .unwrap_or_else(|| "a session".into());
        if tool == "queue_prompt" {
            if self.flag("until_limit_reset") == Some(true) {
                self.fact("When", "once the usage limit resets");
            } else if let Some(t) = self.int("not_before") {
                self.fact("When", format!("not before {t} (unix time)"));
            } else {
                self.fact("When", "when the session is next idle");
            }
        }
        if self.flag("submit") == Some(false) {
            self.fact("Enter", "typed but not sent");
        }
        if self.flag("force") == Some(true) {
            self.fact("Busy session", "send even while it works");
        }
        if let Some(k) = self.str("keys") {
            self.fact("Keys", k.to_string());
        }
        let title = match tool {
            "queue_prompt" => format!("Queue a prompt for {name}"),
            "run_prompt" => format!("Send a prompt to {name} and wait for its answer"),
            _ => format!("Send a prompt to {name}"),
        };
        (title, self.body("Prompt", "prompt"))
    }

    fn dispatch_task(&mut self) -> (String, Option<ConfirmBody>) {
        let args: &'a serde_json::Map<String, Value> = self.args;
        let title = if let Some(w) = args.get("new_worker").and_then(Value::as_object) {
            let host = w
                .get("host_alias")
                .and_then(Value::as_str)
                .unwrap_or("a host")
                .to_string();
            self.fact("Host", host.clone());
            if let Some(p) = w.get("project_id").and_then(Value::as_i64) {
                let label = self.project_label(p);
                self.fact("Project", label);
            }
            if let Some(n) = w.get("name").and_then(Value::as_str) {
                self.fact("Session name", n.to_string());
            }
            if let Some(p) = w.get("profile").and_then(Value::as_str) {
                self.fact("Account", p.to_string());
            }
            format!("Start a new session on {host} and hand it a task")
        } else {
            let name = self
                .session_facts("worker_session_id", "Session")
                .unwrap_or_else(|| "a session".into());
            format!("Hand a task to {name}")
        };
        (title, self.body("Task", "prompt"))
    }

    fn session_op(&mut self, tool: &str) -> (String, Option<ConfirmBody>) {
        let name = self
            .session_facts("session_id", "Session")
            .unwrap_or_else(|| "a session".into());
        if self.flag("force") == Some(true) {
            self.fact("Force", "yes, even with unsaved work");
        }
        for (k, label) in [
            ("model", "Model"),
            ("effort", "Effort"),
            ("profile", "Account"),
            ("new_worktree", "New worktree"),
        ] {
            if let Some(v) = self.str(k) {
                self.fact(label, v.to_string());
            }
        }
        let title = match tool {
            "safe_kill_session" => format!("Stop {name} once its work is saved"),
            "kill_session" => format!("Kill {name}"),
            "restart_session" => format!("Restart {name}"),
            "recreate_session" => format!("Recreate {name}"),
            "rewind_conversation" => format!("Rewind the conversation of {name}"),
            "fork_session" => format!("Fork {name}"),
            _ => format!("Run {tool} on {name}"),
        };
        (title, None)
    }

    /// Any other gated tool: its name in words, the session and host it
    /// names, and the text it carries.
    fn generic(&mut self, tool: &str) -> (String, Option<ConfirmBody>) {
        let target = self.session_facts("session_id", "Session");
        if target.is_none() {
            if let Some(h) = self.str("host_alias") {
                self.fact("Host", h.to_string());
            }
        }
        if let Some(p) = self.int("project_id") {
            let label = self.project_label(p);
            self.fact("Project", label);
        }
        let words = tool.replace('_', " ");
        let title = match target {
            Some(n) => format!("Run {words} on {n}"),
            None => format!("Run {words}"),
        };
        let body = self
            .body("Prompt", "prompt")
            .or_else(|| self.body("Text", "text"));
        (title, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> Store {
        Store::open_in_memory().expect("store")
    }

    fn fact<'a>(a: &'a ConfirmAbout, label: &str) -> Option<&'a str> {
        a.facts
            .iter()
            .find(|f| f.label == label)
            .map(|f| f.value.as_str())
    }

    #[test]
    fn a_ticket_start_reads_as_what_happens() {
        let s = store();
        let a = describe(
            &s,
            "work_link",
            &json!({
                "action": "start", "key": "PD-2709", "host_alias": "mefistos",
                "project_id": 50, "with_brief": true, "brief": "Fix the login.",
                "repos": [], "item_id": null, "force_cross_org": null
            }),
        )
        .expect("about");
        assert_eq!(a.title, "Start a session for PD-2709");
        assert_eq!(fact(&a, "Ticket"), Some("PD-2709"));
        assert_eq!(fact(&a, "Host"), Some("mefistos"));
        assert_eq!(fact(&a, "Project"), Some("#50"));
        // Defaults and unset options stay off the card.
        assert_eq!(fact(&a, "Across orgs"), None);
        assert_eq!(fact(&a, "Parallel"), None);
        let body = a.body.expect("the brief itself");
        assert_eq!(
            (body.label.as_str(), body.text.as_str()),
            ("Brief", "Fix the login.")
        );
    }

    #[test]
    fn a_start_with_no_brief_text_says_the_ticket_briefs_it() {
        let a = describe(
            &store(),
            "work_link",
            &json!({ "action": "start", "key": "PD-1", "with_brief": true }),
        )
        .expect("about");
        assert!(a.body.is_none());
        assert_eq!(fact(&a, "Brief"), Some("the ticket's own text"));
        assert_eq!(fact(&a, "Host"), Some("fleet picks one"));
    }

    #[test]
    fn a_long_body_is_cut() {
        let long = "x".repeat(BODY_MAX_CHARS + 10);
        let a = describe(
            &store(),
            "send_prompt",
            &json!({ "session_id": 999, "prompt": long }),
        )
        .expect("about");
        assert_eq!(a.title, "Send a prompt to #999");
        let text = a.body.expect("body").text;
        assert_eq!(text.chars().count(), BODY_MAX_CHARS + 1);
        assert!(text.ends_with('…'));
    }

    #[test]
    fn a_new_session_names_host_and_options_set() {
        let a = describe(
            &store(),
            "new_session",
            &json!({ "host_alias": "h1", "project_id": 3, "name": "s", "model": "opus" }),
        )
        .expect("about");
        assert_eq!(a.title, "Start a Claude session on h1");
        assert_eq!(fact(&a, "Model"), Some("opus"));
        assert_eq!(fact(&a, "Effort"), None);
    }

    #[test]
    fn not_an_object_is_none() {
        assert!(describe(&store(), "work_link", &json!(null)).is_none());
    }
}
