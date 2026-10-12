//! The error scanner (step 8), ported from support-agent's
//! `engines/scanner.ts` (its Graylog half): with `scan` in its config, a
//! support rule's poll also asks a catalog MCP server on the rule's host
//! (`graylog`'s `search_logs` by default) for the errors of the last
//! window, every `every_mins`. Each error's fingerprint is recorded
//! (`support_fingerprints`, with the error as its sample); one never seen
//! before becomes a `scan` job: an investigation with no ticket, whose only
//! write is the bug it may propose. The server is called the way `claude`
//! calls it, over stdio, with the command, arguments and environment the
//! catalog wrote into the host's `~/.claude.json`.

use super::fingerprint;
use crate::shell::quote;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// A scan job's trigger.
pub const TRIGGER: &str = "scan";

/// A rule's scanner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScanConfig {
    /// The catalog MCP server (install name) on the rule's host.
    pub server: String,
    /// Its tool: takes `from` / `to` (unix seconds) and answers `{"entries": [...]}`.
    pub tool: String,
    /// Extra arguments for the tool (a `query`, `max_level` …).
    pub arguments: Value,
    pub every_mins: i64,
    /// How far back a scan reads; a little more than `every_mins`.
    pub window_mins: i64,
    /// New errors a scan turns into jobs, at most.
    pub max_jobs: usize,
}

impl Default for ScanConfig {
    fn default() -> Self {
        ScanConfig {
            server: "graylog".into(),
            tool: "search_logs".into(),
            arguments: json!({}),
            every_mins: 30,
            window_mins: 35,
            max_jobs: 5,
        }
    }
}

impl ScanConfig {
    pub fn check(&self) -> Result<(), String> {
        let name = |s: &str| {
            !s.is_empty()
                && s.len() <= 64
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        };
        if !name(&self.server) || !name(&self.tool) {
            return Err("scan: server and tool are names".into());
        }
        if !self.arguments.is_object() {
            return Err("scan: arguments is an object".into());
        }
        if !(5..=1440).contains(&self.every_mins)
            || !(5..=1440).contains(&self.window_mins)
            || !(1..=20).contains(&self.max_jobs)
        {
            return Err("scan: every_mins, window_mins or max_jobs is out of range".into());
        }
        Ok(())
    }
}

/// PURE: a support rule's cursor: the last poll, and the last scan.
pub fn read_cursor(cursor: Option<&str>) -> (Option<i64>, Option<i64>) {
    let Some(c) = cursor else { return (None, None) };
    let (since, scan) = match c.split_once(";scan=") {
        Some((a, b)) => (a, b.parse().ok()),
        None => (c, None),
    };
    (since.parse().ok(), scan)
}

/// PURE: the cursor written back.
pub fn write_cursor(since: i64, scan: Option<i64>) -> String {
    match scan {
        Some(t) => format!("{since};scan={t}"),
        None => since.to_string(),
    }
}

/// One error a scan found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedError {
    pub exception_class: String,
    /// The message's first line.
    pub message: String,
    /// The whole message, a stack trace when there is one; capped.
    pub stack_trace: String,
    /// Where it was logged (the pod / service).
    pub component: String,
    pub timestamp: String,
    pub level: String,
}

/// The longest stack trace kept of an error.
const STACK_CAP: usize = 8000;

/// PURE: the exception class of a message's first line (`pkg.FooException:
/// …`, `Caused by: x.Y: …`), when it names one.
pub fn exception_class(first_line: &str) -> Option<String> {
    let line = first_line.trim().trim_start_matches("Caused by:").trim();
    let class = line.split(':').next()?.trim();
    let looks = !class.is_empty()
        && !class.contains(char::is_whitespace)
        && class
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._$".contains(c))
        && ["Exception", "Error", "Throwable", "Panic"]
            .iter()
            .any(|s| class.contains(s));
    looks.then(|| class.to_string())
}

/// PURE: the tool's answer (`{"entries": [{timestamp, level, source, message}]}`)
/// as errors, one per distinct fingerprint, in the order found.
pub fn errors_in(
    text: &str,
    app_frame_prefix: &str,
) -> Result<Vec<(String, ScannedError)>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("scan answer: {e}"))?;
    let entries = v
        .get("entries")
        .and_then(Value::as_array)
        .ok_or("scan answer: no entries")?;
    let mut out: Vec<(String, ScannedError)> = Vec::new();
    for e in entries {
        let s = |k: &str| {
            e.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let message = s("message");
        let first = message.lines().next().unwrap_or_default().to_string();
        let Some(class) = exception_class(&first) else {
            continue;
        };
        let stack: String = message.chars().take(STACK_CAP).collect();
        let Some(fp) = fingerprint::fingerprint(&class, &first, &stack, app_frame_prefix) else {
            continue;
        };
        if out.iter().any(|(f, _)| *f == fp) {
            continue;
        }
        out.push((
            fp,
            ScannedError {
                exception_class: class,
                message: first,
                stack_trace: stack,
                component: Some(s("source"))
                    .filter(|c| !c.is_empty())
                    .unwrap_or_else(|| "unknown".into()),
                timestamp: s("timestamp"),
                level: e
                    .get("level")
                    .map(|l| l.to_string().trim_matches('"').to_string())
                    .unwrap_or_default(),
            },
        ));
    }
    Ok(out)
}

/// The response line a script waits for: the call's (`id` 2).
const CALL_ID: i64 = 2;

/// PURE: a shell script that starts the MCP server `entry` (its
/// `~/.claude.json` entry), initializes it, calls `tool`, prints the
/// call's response line and stops the server. stdin stays open until the
/// answer is in, so a server that ends on EOF still answers.
pub fn call_script(
    entry: &Value,
    tool: &str,
    arguments: &Value,
    timeout_secs: u64,
) -> Result<String, String> {
    if entry
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| t != "stdio")
    {
        return Err("scan: only a stdio MCP server can be scanned".into());
    }
    let command = entry
        .get("command")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .ok_or("scan: the MCP server has no command")?;
    let mut argv = vec![quote(command)];
    for a in entry
        .get("args")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        argv.push(quote(
            a.as_str().ok_or("scan: an argument is not a string")?,
        ));
    }
    let mut env = Vec::new();
    for (k, v) in entry
        .get("env")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("scan: env name {k:?}"));
        }
        env.push(format!(
            "{k}={}",
            quote(v.as_str().ok_or("scan: an env value is not a string")?)
        ));
    }
    let messages = [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": { "name": "claude-fleet", "version": "1" } } }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        json!({ "jsonrpc": "2.0", "id": CALL_ID, "method": "tools/call",
            "params": { "name": tool, "arguments": arguments } }),
    ];
    let lines: Vec<String> = messages.iter().map(|m| quote(&m.to_string())).collect();
    Ok(format!(
        "out=$(mktemp) || exit 1\n\
         trap 'rm -f \"$out\"' EXIT\n\
         {{ printf '%s\\n' {msgs}; i=0; while [ $i -lt {timeout_secs} ] && ! grep -Eq '\"id\": ?{CALL_ID}[,}}]' \"$out\"; do sleep 1; i=$((i+1)); done; }} \
         | timeout {kill} env {env} {argv} > \"$out\" 2>/dev/null\n\
         grep -E '\"id\": ?{CALL_ID}[,}}]' \"$out\" | head -n1\n",
        msgs = lines.join(" "),
        kill = timeout_secs + 5,
        env = env.join(" "),
        argv = argv.join(" "),
    ))
}

/// PURE: the call's response line as the tool's text answer.
pub fn tool_text(stdout: &str) -> Result<String, String> {
    let line = stdout
        .lines()
        .find(|l| !l.trim().is_empty())
        .ok_or("scan: the MCP server did not answer")?;
    let v: Value = serde_json::from_str(line.trim()).map_err(|e| format!("scan: {e}"))?;
    if let Some(e) = v.get("error") {
        return Err(format!(
            "scan: {}",
            e.get("message").and_then(Value::as_str).unwrap_or("error")
        ));
    }
    let result = v.get("result").ok_or("scan: no result")?;
    let text: String = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(format!(
            "scan: the tool failed: {}",
            text.chars().take(300).collect::<String>()
        ));
    }
    Ok(text)
}

/// PURE: the scanned error as the ticket the investigation reads.
pub fn as_thread(fp: &str, e: &ScannedError) -> crate::service::trackers::IssueThread {
    crate::service::trackers::IssueThread {
        key: format!("SCAN-{}", &fp[..fp.len().min(12)]),
        summary: format!(
            "[{}] {}",
            e.component,
            e.message.chars().take(200).collect::<String>()
        ),
        description: format!(
            "An error the scanner found in the logs, not reported by anyone yet.\n\
             Logged at {} by {} (level {}).\n\n{}",
            e.timestamp, e.component, e.level, e.stack_trace
        ),
        assignee_id: None,
        status: "scanned".into(),
        updated: None,
        comments: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_keeps_the_last_scan() {
        assert_eq!(read_cursor(Some("100")), (Some(100), None));
        assert_eq!(read_cursor(Some("100;scan=90")), (Some(100), Some(90)));
        assert_eq!(read_cursor(None), (None, None));
        assert_eq!(write_cursor(100, Some(90)), "100;scan=90");
        assert_eq!(write_cursor(100, None), "100");
    }

    #[test]
    fn an_exception_is_read_from_the_first_line() {
        assert_eq!(
            exception_class("java.lang.NullPointerException: x").as_deref(),
            Some("java.lang.NullPointerException")
        );
        assert_eq!(
            exception_class("Caused by: a.b.IOError: y").as_deref(),
            Some("a.b.IOError")
        );
        assert_eq!(exception_class("GET /health 200: ok"), None);
        assert_eq!(exception_class("Connection refused"), None);
    }

    #[test]
    fn errors_are_grouped_by_fingerprint() {
        let text = json!({ "entries": [
            { "timestamp": "t1", "level": "3", "source": "pay", "message": "x.NPE Exception: order 1\n\tat com.p.A.a(A.java:1)" },
            { "timestamp": "t2", "level": "3", "source": "pay", "message": "x.NPException: order 2\n\tat com.p.A.a(A.java:1)" },
            { "timestamp": "t3", "level": "3", "source": "pay", "message": "x.NPException: order 3\n\tat com.p.A.a(A.java:1)" },
            { "timestamp": "t4", "level": 3, "source": "", "message": "boot ok" },
        ]})
        .to_string();
        let found = errors_in(&text, "").unwrap();
        assert_eq!(
            found.len(),
            1,
            "one class with a space is no class; orders 2 and 3 are one bug"
        );
        assert_eq!(
            (found[0].1.timestamp.as_str(), found[0].1.component.as_str()),
            ("t2", "pay")
        );
        assert!(errors_in("{}", "").is_err());
    }

    #[test]
    fn the_call_script_quotes_everything_and_waits_for_the_answer() {
        let entry = json!({ "type": "stdio", "command": "node", "args": ["/opt/sa/dist/mcp/graylog/index.js"],
            "env": { "GRAYLOG_API_TOKEN": "t'ok" } });
        let s = call_script(&entry, "search_logs", &json!({ "from": 1, "to": 2 }), 60).unwrap();
        assert!(s.contains("GRAYLOG_API_TOKEN='t'\\''ok'"), "{s}");
        assert!(
            s.contains("'node' '/opt/sa/dist/mcp/graylog/index.js'"),
            "{s}"
        );
        assert!(s.contains("timeout 65 env"));
        assert!(s.contains(r#""name":"search_logs""#));
        assert!(call_script(&json!({ "type": "http", "url": "u" }), "t", &json!({}), 60).is_err());
        assert!(call_script(
            &json!({ "command": "x", "env": { "A B": "c" } }),
            "t",
            &json!({}),
            60
        )
        .is_err());
    }

    #[test]
    fn the_answer_is_the_tools_text() {
        let ok = r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"{\"entries\":[]}"}],"isError":false}}"#;
        assert_eq!(tool_text(ok).unwrap(), r#"{"entries":[]}"#);
        let bad = r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"boom"}],"isError":true}}"#;
        assert!(tool_text(bad).unwrap_err().contains("boom"));
        assert!(tool_text("").is_err());
        assert!(tool_text(r#"{"id":2,"error":{"message":"no tool"}}"#)
            .unwrap_err()
            .contains("no tool"));
    }
}
