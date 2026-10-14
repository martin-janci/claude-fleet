//! The grant-time check of an MCP binding (spec *D. Store bindings*):
//! start the rendered server on the rule's host, `initialize`, `tools/list`,
//! and confirm the role's `read_tools` and `write_tools` are there. A store
//! that is not there fails the grant with the reason, not the first job.
//!
//! The rendered entry carries secrets, so the script that starts it travels
//! on stdin (the command line only copies stdin into a private temp file
//! and runs it), as a run's MCP config does (`agents::vehicle`): nothing
//! secret is in an argument a host's `ps` shows. An `http` server is asked
//! with `curl` from the host, the same way.
//!
//! [`call_script`] is the same script for one `tools/call`: a fleet
//! agent's `mcp_scan` trigger asks its store that way.

use super::definition::{StoreRole, WriteTool};
use crate::shell::quote;
use serde_json::{json, Value};

/// How long the server may take to list its tools.
pub const PROBE_SECS: u64 = 30;

/// The response line the script waits for: the request's.
const LIST_ID: i64 = 2;

/// `initialize`, `initialized`, then `last` (whose id is [`LIST_ID`]).
fn messages(last: Value) -> [Value; 3] {
    [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": { "name": "claude-fleet", "version": "1" } } }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        last,
    ]
}

fn env_name_ok(k: &str) -> bool {
    !k.is_empty()
        && !k.starts_with(|c: char| c.is_ascii_digit())
        && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// PURE: the command line that runs the script it reads on stdin.
pub fn launcher() -> String {
    "umask 077; d=$(mktemp -d 2>/dev/null) || exit 1; trap 'rm -rf -- \"$d\"' EXIT; \
     cat > \"$d/probe.sh\"; bash \"$d/probe.sh\""
        .to_string()
}

/// PURE: the script (sent on stdin) that lists a rendered entry's tools and
/// prints the `tools/list` response line.
pub fn script(entry: &Value) -> Result<String, String> {
    request_script(
        entry,
        json!({ "jsonrpc": "2.0", "id": LIST_ID, "method": "tools/list" }),
        PROBE_SECS,
    )
}

/// PURE: the script (sent on stdin) that calls `tool` with `arguments` and
/// prints the `tools/call` response line, within `secs`.
pub fn call_script(
    entry: &Value,
    tool: &str,
    arguments: &Value,
    secs: u64,
) -> Result<String, String> {
    request_script(
        entry,
        json!({ "jsonrpc": "2.0", "id": LIST_ID, "method": "tools/call",
            "params": { "name": tool, "arguments": arguments } }),
        secs,
    )
}

fn request_script(entry: &Value, last: Value, secs: u64) -> Result<String, String> {
    let s = |k: &str| entry.get(k).and_then(Value::as_str);
    match s("type").unwrap_or("stdio") {
        "stdio" => stdio_script(entry, last, secs),
        "http" => http_script(
            s("url")
                .filter(|u| !u.is_empty())
                .ok_or("the server has no url")?,
            entry.get("headers"),
            last,
            secs,
        ),
        other => Err(format!("a {other} MCP server cannot be asked")),
    }
}

fn stdio_script(entry: &Value, last: Value, secs: u64) -> Result<String, String> {
    let command = entry
        .get("command")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .ok_or("the server has no command")?;
    let mut argv = vec![quote(command)];
    for a in entry
        .get("args")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        argv.push(quote(a.as_str().ok_or("an argument is not a string")?));
    }
    let mut exports = String::new();
    for (k, v) in entry
        .get("env")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        if !env_name_ok(k) {
            return Err(format!("env name {k:?}"));
        }
        exports.push_str(&format!(
            "export {k}={}\n",
            quote(v.as_str().ok_or("an env value is not a string")?)
        ));
    }
    let lines: Vec<String> = messages(last)
        .iter()
        .map(|m| quote(&m.to_string()))
        .collect();
    Ok(format!(
        "{exports}out=$(mktemp) || exit 1\n\
         trap 'rm -f \"$out\"' EXIT\n\
         {{ printf '%s\\n' {msgs}; i=0; while [ $i -lt {t} ] && ! grep -Eq '\"id\": ?{LIST_ID}[,}}]' \"$out\"; do sleep 1; i=$((i+1)); done; }} \
         | timeout {kill} {argv} > \"$out\" 2>/dev/null\n\
         grep -E '\"id\": ?{LIST_ID}[,}}]' \"$out\" | head -n1\n",
        msgs = lines.join(" "),
        t = secs,
        kill = secs + 5,
        argv = argv.join(" "),
    ))
}

fn http_script(
    url: &str,
    headers: Option<&Value>,
    last: Value,
    secs: u64,
) -> Result<String, String> {
    // The headers (a bearer token, typically) go into a curl config file,
    // never onto curl's command line.
    let mut cfg = String::new();
    for (k, v) in headers.and_then(Value::as_object).into_iter().flatten() {
        let v = v.as_str().ok_or("a header value is not a string")?;
        if k.is_empty() || k.contains([':', '\n', '\r', '"']) || v.contains(['\n', '\r', '"']) {
            return Err(format!("header {k:?} cannot be sent"));
        }
        cfg.push_str(&format!("header = \"{k}: {v}\"\n"));
    }
    if url.contains(['\n', '\r', '"']) {
        return Err("the url cannot be sent".into());
    }
    cfg.push_str(&format!("url = \"{url}\"\n"));
    let [init, inited, list] = messages(last);
    let post = |body: &Value, keep_session: bool| {
        format!(
            "curl -sS --max-time {t} -K \"$cfg\" -H 'Content-Type: application/json' \
             -H 'Accept: application/json, text/event-stream' {sid} -D \"$hdr\" --data-binary {b}",
            t = secs,
            sid = if keep_session {
                "${sid:+-H \"Mcp-Session-Id: $sid\"}"
            } else {
                ""
            },
            b = quote(&body.to_string()),
        )
    };
    Ok(format!(
        "cfg=$(mktemp) && hdr=$(mktemp) || exit 1\n\
         trap 'rm -f \"$cfg\" \"$hdr\"' EXIT\n\
         printf '%s' {cfg} > \"$cfg\"\n\
         {init} > /dev/null || exit 1\n\
         sid=$(grep -i '^mcp-session-id:' \"$hdr\" | head -n1 | cut -d: -f2- | tr -d ' \\r')\n\
         {inited} > /dev/null\n\
         {list} | sed -n 's/^data: //p; /^{{/p' | grep -E '\"id\": ?{LIST_ID}[,}}]' | head -n1\n",
        cfg = quote(&cfg),
        init = post(&init, false),
        inited = post(&inited, true),
        list = post(&list, true),
    ))
}

/// PURE: the tool names of the `tools/list` response line.
pub fn listed_tools(stdout: &str) -> Result<Vec<String>, String> {
    let line = stdout
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .ok_or("the server did not answer tools/list")?;
    let v: Value = serde_json::from_str(line).map_err(|e| format!("tools/list: {e}"))?;
    if let Some(e) = v.get("error") {
        return Err(format!(
            "tools/list: {}",
            e.get("message").and_then(Value::as_str).unwrap_or("error")
        ));
    }
    let tools = v
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .ok_or("tools/list: no tools")?;
    Ok(tools
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string))
        .collect())
}

/// PURE: the role's tools the server does not list.
pub fn missing_tools(role: &StoreRole, listed: &[String]) -> Vec<String> {
    role.read_tools
        .iter()
        .map(String::as_str)
        .chain(role.write_tools.iter().map(WriteTool::name))
        .filter(|t| !listed.iter().any(|l| l == t))
        .map(str::to_string)
        .collect()
}

/// Run the check on `host`: the server's tools, or why it could not be asked.
pub async fn list_tools(
    ssh: &crate::ssh::SshClient,
    host: &str,
    entry: &Value,
) -> Result<Vec<String>, String> {
    let body = script(entry)?;
    let out = ssh
        .run_with_stdin(
            host,
            &["bash", "-lc", &quote(&launcher())],
            body.into_bytes(),
            std::time::Duration::from_secs(15),
            std::time::Duration::from_secs(PROBE_SECS + 30),
            256 * 1024,
        )
        .await
        .map_err(|e| e.message)?;
    listed_tools(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stdio_script_exports_its_secrets_and_lists_the_tools() {
        let e = json!({ "type": "stdio", "command": "node", "args": ["/opt/sa/index.js"],
            "env": { "GRAYLOG_API_TOKEN": "t'ok" } });
        let s = script(&e).unwrap();
        assert!(
            s.starts_with("export GRAYLOG_API_TOKEN='t'\\''ok'\n"),
            "{s}"
        );
        assert!(s.contains("timeout 35 'node' '/opt/sa/index.js'"), "{s}");
        assert!(s.contains(r#""method":"tools/list""#));
        assert!(!launcher().contains("t'ok") && !launcher().contains("GRAYLOG"));
        assert!(script(&json!({ "command": "x", "env": { "A B": "c" } })).is_err());
        assert!(script(&json!({ "command": "" })).is_err());
        assert!(script(&json!({ "type": "sse", "url": "u" })).is_err());
    }

    #[test]
    fn an_http_script_keeps_the_headers_off_the_command_line() {
        let e = json!({ "type": "http", "url": "https://kb.example/mcp",
            "headers": { "Authorization": "Bearer s3cret" } });
        let s = script(&e).unwrap();
        let cmd_lines: Vec<&str> = s.lines().filter(|l| l.starts_with("curl")).collect();
        assert_eq!(cmd_lines.len(), 3, "{s}");
        assert!(cmd_lines.iter().all(|l| !l.contains("s3cret")), "{s}");
        assert!(s.contains("header = \"Authorization: Bearer s3cret\""));
        assert!(
            script(&json!({ "type": "http", "url": "u", "headers": { "A": "x\"y" } })).is_err()
        );
    }

    #[test]
    fn the_listed_tools_are_compared_with_the_role() {
        let line = r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"search_docs"},{"name":"search_known_errors"}]}}"#;
        let listed = listed_tools(&format!("\n{line}\n")).unwrap();
        assert_eq!(listed, ["search_docs", "search_known_errors"]);
        let role: StoreRole = serde_json::from_value(json!({ "role": "kb", "kind": "mcp",
            "label": "KB", "read_tools": ["search_docs"], "write_tools": ["add_known_error"] }))
        .unwrap();
        assert_eq!(missing_tools(&role, &listed), ["add_known_error"]);
        assert!(listed_tools("").is_err());
        assert!(listed_tools(r#"{"id":2,"error":{"message":"boom"}}"#)
            .unwrap_err()
            .contains("boom"));
    }
}
