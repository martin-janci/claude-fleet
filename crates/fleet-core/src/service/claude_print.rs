//! The script around the one locked-down `claude -p` run fleet starts on a
//! host, shared by every headless run (the summaries, the drafts, the
//! planner, context help, the Jev `claude -p haiku` baseline and the
//! agents' runs, [`super::agents::vehicle`]). Which CLI runs and its command
//! words are [`super::headless`]'s.
//!
//! * [`isolation_flags`] are the words that take everything away from the
//!   run: no hooks, no built-in tool, no MCP server, no transcript left.
//! * [`run_capped`] is the tail of the script: the host-side `timeout` when
//!   there is one, the `<tag>run` line, then `claude` with its stdout capped.
//! * [`parse_tagged`] reads the script's stdout: the FIRST whole tag line
//!   decides. Each script prints exactly one tag line before `claude` starts
//!   and only `run` is followed by model output, so nothing the model says
//!   can stand in for the verdict.

use crate::shell::quote;
use serde_json::Value;

/// PURE: the flag words every locked-down run carries, each value quoted
/// with [`crate::shell::quote`].
///
/// `disableAllHooks`, not `{"hooks":{}}`: Claude Code keeps the user-level
/// hooks (fleet's own, in ~/.claude/settings.json) under an empty `hooks`
/// object, so the run would report itself to fleet as a new conversation and
/// prompt. Checked against Claude Code 2.1.283. `--tools ''` disables every
/// built-in tool; `--strict-mcp-config` with no `--mcp-config` loads no MCP
/// server; `--no-session-persistence` leaves no transcript.
pub fn isolation_flags() -> String {
    [
        "--settings",
        &quote(r#"{"disableAllHooks":true}"#),
        "--tools",
        &quote(""),
        "--strict-mcp-config",
        "--no-session-persistence",
    ]
    .join(" ")
}

/// PURE: [`isolation_flags`] with the built-in `tools` the run may use
/// (`Read`, `Grep`, `Glob` …) and the extra tools it may call without a
/// prompt (`allowed`, for MCP tools: `mcp__<server>__<tool>`, `*` allowed
/// as the tool), for the agents' runs. Everything else stays taken away: no
/// hooks, no MCP server unless the caller adds its own `--mcp-config` after
/// these words (with `--strict-mcp-config` nothing else loads), no
/// transcript; as in [`tool_flags`], [`NEVER_TOOLS`] are denied and
/// `--permission-mode dontAsk` denies whatever would ask, so a run with no
/// person never waits on a prompt. A name outside those shapes is refused,
/// so no caller can smuggle a flag or `Bash(…)` in. No tools and none
/// allowed is exactly [`isolation_flags`].
pub fn isolation_flags_with(tools: &[&str], allowed: &[&str]) -> Result<String, String> {
    fn word(s: &str) -> bool {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }
    for t in tools {
        if !t.chars().next().is_some_and(|c| c.is_ascii_uppercase()) || !word(t) {
            return Err(format!("refusing tool name {t:?}"));
        }
    }
    for a in allowed {
        let ok = a
            .strip_prefix("mcp__")
            .and_then(|rest| rest.split_once("__"))
            .is_some_and(|(server, tool)| word(server) && (tool == "*" || word(tool)));
        if !ok {
            return Err(format!(
                "refusing allowed tool {a:?}: only mcp__<server>__<tool>"
            ));
        }
    }
    if tools.is_empty() && allowed.is_empty() {
        return Ok(isolation_flags());
    }
    let settings = quote(r#"{"disableAllHooks":true}"#);
    let tools = quote(&tools.join(","));
    let never = quote(NEVER_TOOLS);
    let mut words = vec![
        "--settings",
        &settings,
        "--tools",
        &tools,
        "--disallowedTools",
        &never,
    ];
    let allowed_list = quote(&allowed.join(","));
    if !allowed.is_empty() {
        words.extend(["--allowedTools", &allowed_list]);
    }
    words.extend([
        "--permission-mode",
        "dontAsk",
        "--strict-mcp-config",
        "--no-session-persistence",
    ]);
    Ok(words.join(" "))
}

/// The built-in tools a read-only run has: reading inside its working
/// directory, nothing that writes, runs or reaches the network.
pub const READ_ONLY_TOOLS: &str = "Read,Grep,Glob";
/// The built-in tools a run may never have, whatever its policy says.
pub const NEVER_TOOLS: &str = "Edit,Write,MultiEdit,NotebookEdit,WebFetch,WebSearch,Task";

/// PURE: the flag words of a run that may use some tools, each value quoted
/// with [`crate::shell::quote`]. The same hooks, MCP and transcript lock-down
/// as [`isolation_flags`]; `--tools` names only [`READ_ONLY_TOOLS`] (plus
/// `Bash` when `bash` names commands), `--allowedTools` pre-approves exactly
/// `Bash(<command>)` for each of `bash` and nothing else, and
/// `--permission-mode dontAsk` denies whatever would have asked, so a run
/// without a person never waits on a prompt nor widens: a read outside the
/// working directory, another command, any write. [`NEVER_TOOLS`] are denied
/// on top. Checked against Claude Code 2.1.296's `--help`.
pub fn tool_flags(bash: &[&str]) -> String {
    let tools = if bash.is_empty() {
        READ_ONLY_TOOLS.to_string()
    } else {
        format!("{READ_ONLY_TOOLS},Bash")
    };
    let mut words = vec![
        "--settings".to_string(),
        quote(r#"{"disableAllHooks":true}"#),
        "--tools".into(),
        quote(&tools),
        "--disallowedTools".into(),
        quote(NEVER_TOOLS),
    ];
    if !bash.is_empty() {
        words.push("--allowedTools".into());
        words.extend(bash.iter().map(|c| quote(&format!("Bash({c})"))));
    }
    words.extend([
        "--permission-mode".into(),
        "dontAsk".into(),
        "--strict-mcp-config".into(),
        "--no-session-persistence".into(),
    ]);
    words.join(" ")
}

/// PURE: the script line that ends with `<tag>noclaude` when `claude` is not
/// on the host's login PATH.
pub fn noclaude_check(tag: &str) -> String {
    format!("if ! command -v claude >/dev/null 2>&1; then echo {tag}noclaude; exit 0; fi;")
}

/// PURE: the script's run: the host-side `timeout` (when the host has one),
/// the `<tag>run` line, then `claude_cmd` (stdin closed when `stdin_null`)
/// with its stdout capped at `output_cap` bytes. Nothing follows the pipe:
/// a caller appends its own epilogue.
pub fn run_capped(
    tag: &str,
    claude_cmd: &str,
    host_timeout_secs: u64,
    output_cap: usize,
    stdin_null: bool,
) -> String {
    let stdin = if stdin_null { " </dev/null" } else { "" };
    format!(
        "t=''; if command -v timeout >/dev/null 2>&1; then t='timeout {host_timeout_secs}'; fi; \
         echo {tag}run; \
         $t {claude_cmd}{stdin} | head -c {output_cap}"
    )
}

/// PURE: read a tagged script's stdout. The FIRST line whose trimmed text is
/// exactly `<tag><verdict>` for one of `verdicts` decides; for that line the
/// answer is the verdict and every line after it, joined with `\n` and
/// trimmed. No such line is `None`.
pub fn parse_tagged<'v>(
    stdout: &str,
    tag: &str,
    verdicts: &[&'v str],
) -> Option<(&'v str, String)> {
    let lines: Vec<&str> = stdout.lines().collect();
    lines.iter().enumerate().find_map(|(i, l)| {
        let rest = l.trim().strip_prefix(tag)?;
        let v = verdicts.iter().find(|v| **v == rest)?;
        Some((*v, lines[i + 1..].join("\n").trim().to_string()))
    })
}

// --- the `--output-format json` reply ---------------------------------------

/// What `--output-format json` wraps the model's text in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Envelope {
    /// The model's text.
    pub result: String,
    pub is_error: bool,
    /// Input tokens (plain + cache creation + cache read), when reported.
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    /// `total_cost_usd` in micro-dollars, when reported.
    pub cost_microusd: Option<i64>,
}

fn int(v: Option<&Value>) -> Option<i64> {
    v.and_then(Value::as_i64)
}

fn envelope_of(v: &Value) -> Option<Envelope> {
    let o = v.as_object()?;
    if o.get("type").and_then(Value::as_str) != Some("result") && !o.contains_key("result") {
        return None;
    }
    let usage = o.get("usage");
    let input = usage.and_then(|u| {
        let parts = [
            int(u.get("input_tokens")),
            int(u.get("cache_creation_input_tokens")),
            int(u.get("cache_read_input_tokens")),
        ];
        parts
            .iter()
            .any(Option::is_some)
            .then(|| parts.iter().flatten().sum())
    });
    Some(Envelope {
        result: o
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        is_error: o.get("is_error").and_then(Value::as_bool).unwrap_or(false)
            || o.get("subtype")
                .and_then(Value::as_str)
                .is_some_and(|s| s != "success"),
        input_tokens: input,
        output_tokens: usage.and_then(|u| int(u.get("output_tokens"))),
        cost_microusd: o
            .get("total_cost_usd")
            .and_then(Value::as_f64)
            .filter(|c| c.is_finite() && *c >= 0.0)
            .map(|c| (c * 1_000_000.0).round() as i64),
    })
}

/// PURE: the `--output-format json` envelope in `out`: one JSON object (or
/// an array of messages whose last `result` message is it), possibly after
/// other lines. `None` when there is none.
pub fn parse_envelope(out: &str) -> Option<Envelope> {
    let t = out.trim();
    let whole = serde_json::from_str::<Value>(t).ok();
    let candidates = whole.into_iter().chain(
        t.lines()
            .rev()
            .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok()),
    );
    for v in candidates {
        match &v {
            Value::Array(a) => {
                if let Some(e) = a.iter().rev().find_map(envelope_of) {
                    return Some(e);
                }
            }
            _ => {
                if let Some(e) = envelope_of(&v) {
                    return Some(e);
                }
            }
        }
    }
    None
}

// --- a signed-out `claude` ---------------------------------------------------

/// What Claude Code prints, lowercased, when the run has no usable login:
/// "Login expired · Run /login to sign in again", "Invalid API key · Please
/// run /login", "OAuth token has expired", an API `authentication_error`.
const SIGNED_OUT: &[&str] = &[
    "login expired",
    "run /login",
    "invalid api key",
    "not logged in",
    "oauth token has expired",
    "oauth token has been revoked",
    "authentication_error",
];

/// PURE: whether `said` (the text of an `is_error` envelope, a reply with no
/// envelope, or the run's stderr) is Claude Code saying its login is gone.
/// Only the opening is read: claude's own error is one short line, and a
/// model's answer that merely discusses `/login` further down is not one.
/// Callers ask only of a run that failed or answered with no envelope, never
/// of a successful envelope's text.
pub fn says_signed_out(said: &str) -> bool {
    let head: String = said
        .trim()
        .chars()
        .take(300)
        .collect::<String>()
        .to_lowercase();
    SIGNED_OUT.iter().any(|p| head.contains(p))
}

/// PURE: whether a finished run says its login is gone: no envelope, or an
/// `is_error` one, whose `text` says so, or the last line of `stderr` does.
/// A successful envelope is never read as signed out, whatever it says.
pub fn run_signed_out(env: Option<&Envelope>, text: &str, stderr: &[u8]) -> bool {
    if env.is_some_and(|e| !e.is_error) {
        return false;
    }
    says_signed_out(text)
        || says_signed_out(&crate::service::work::summary::last_error_line(stderr))
}

/// The refusal for a run whose login is gone: which login (the host's own,
/// or the login profile `profile`) on which host, and the command that
/// signs it in again there. Fleet holds no token, so only a person can.
/// Every refusal built here is also noted for `service::login_health`.
pub fn signed_out_error(host: &str, profile: Option<&str>) -> crate::ipc_error::IpcError {
    // Login health (control review F6): the next usage pass records it.
    crate::service::login_health::note_signed_out(host, profile);
    let (whose, login) = match profile {
        Some(p) => (
            format!("{host} (login profile {p})"),
            format!("CLAUDE_CONFIG_DIR=~/.claude-profiles/{p} claude /login"),
        ),
        None => (host.to_string(), "claude /login".to_string()),
    };
    crate::ipc_error::IpcError::new(
        crate::ipc_error::codes::E_CLAUDE_CLI,
        format!("Claude login expired on {whose}: run `{login}` there, then retry"),
    )
}

// --- a `claude` at its usage limit -------------------------------------------

/// Claude Code's own words, lowercased, for a run its account's usage limit
/// stopped: "Claude AI usage limit reached|1760536800", "5-hour limit
/// reached ∙ resets 3pm", "Weekly limit reached · resets Oct 20, 9am",
/// "You've hit your limit · resets 3pm (Europe/Bratislava)".
const USAGE_LIMIT: &[&str] = &[
    "usage limit reached",
    "hit your limit",
    "hit your usage limit",
];

/// A run stopped at its account's usage limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageLimit {
    /// When it resets, when the answer says so as a Unix time (the
    /// `…limit reached|<epoch>` form).
    pub reset_at: Option<i64>,
    /// How the answer says when it resets ("resets 3pm (Europe/Bratislava)"),
    /// redacted and cut to 80 characters.
    pub said: Option<String>,
}

/// PURE: whether `said` (an answer that is not what was asked for, or the
/// run's stderr) is Claude Code saying the account's usage limit stopped
/// it, and when it resets. Only the opening is read, as
/// [`says_signed_out`]: claude's own message is one short line.
pub fn usage_limit(said: &str) -> Option<UsageLimit> {
    let head: String = said.trim().chars().take(300).collect();
    let lower = head.to_lowercase();
    let hit = USAGE_LIMIT.iter().any(|p| lower.contains(p))
        || (lower.contains("limit reached") && lower.contains("reset"));
    if !hit {
        return None;
    }
    let reset_at = head.split_once('|').and_then(|(_, rest)| {
        let digits: String = rest
            .trim_start()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse::<i64>().ok().filter(|t| *t > 1_000_000_000)
    });
    // The phrase in Claude's own case ("Europe/Bratislava"), found without
    // lowercasing, whose byte offsets may differ from `head`'s.
    let said = head
        .char_indices()
        .find(|(i, _)| {
            head.get(*i..*i + 5)
                .is_some_and(|w| w.eq_ignore_ascii_case("reset"))
        })
        .map(|(i, _)| {
            let phrase = head[i..].lines().next().unwrap_or_default();
            let phrase = phrase.trim().trim_end_matches(['.', ' ']);
            crate::logging::redact(phrase)
                .chars()
                .take(80)
                .collect::<String>()
        });
    Some(UsageLimit { reset_at, said })
}

/// PURE: [`usage_limit`] of a finished run's answer `text`, else of the last
/// line of its `stderr`.
pub fn run_usage_limit(text: &str, stderr: &[u8]) -> Option<UsageLimit> {
    usage_limit(text)
        .or_else(|| usage_limit(&crate::service::work::summary::last_error_line(stderr)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::decide::haiku::haiku_script;
    use crate::service::work::summary::summary_script;

    const CID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

    #[test]
    fn the_isolation_flags_are_exactly_these() {
        assert_eq!(
            isolation_flags(),
            r#"--settings '{"disableAllHooks":true}' --tools '' --strict-mcp-config --no-session-persistence"#
        );
    }

    #[test]
    fn tools_and_mcp_tools_are_named_or_refused() {
        assert_eq!(
            isolation_flags_with(&["Read", "Grep", "Glob"], &["mcp__graylog__search"]).unwrap(),
            r#"--settings '{"disableAllHooks":true}' --tools 'Read,Grep,Glob' --disallowedTools 'Edit,Write,MultiEdit,NotebookEdit,WebFetch,WebSearch,Task' --allowedTools 'mcp__graylog__search' --permission-mode dontAsk --strict-mcp-config --no-session-persistence"#
        );
        assert!(isolation_flags_with(&["Read"], &["mcp__kb__*"]).is_ok());
        assert_eq!(isolation_flags_with(&[], &[]).unwrap(), isolation_flags());
        for bad in [
            "Bash(rm:*)",
            "read",
            "Read --dangerously-skip-permissions",
            "",
            "Re ad",
        ] {
            assert!(isolation_flags_with(&[bad], &[]).is_err(), "{bad}");
        }
        for bad in [
            "Bash",
            "mcp__x",
            "mcp____y",
            "mcp__x__y z",
            "mcp__x__y,Bash",
        ] {
            assert!(isolation_flags_with(&[], &[bad]).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_tool_policy_reads_or_runs_exactly_what_it_names() {
        let read = tool_flags(&[]);
        assert_eq!(
            read,
            r#"--settings '{"disableAllHooks":true}' --tools 'Read,Grep,Glob' --disallowedTools 'Edit,Write,MultiEdit,NotebookEdit,WebFetch,WebSearch,Task' --permission-mode dontAsk --strict-mcp-config --no-session-persistence"#
        );
        assert!(!read.contains("Bash") && !read.contains("--allowedTools"));
        let test = tool_flags(&["cargo test -p x", "pnpm test"]);
        assert!(test.contains("--tools 'Read,Grep,Glob,Bash'"), "{test}");
        assert!(
            test.contains("--allowedTools 'Bash(cargo test -p x)' 'Bash(pnpm test)' --permission-mode dontAsk"),
            "{test}"
        );
        for f in [&read, &test] {
            for gone in ["bypass", "dangerously", "acceptEdits", "auto"] {
                assert!(!f.contains(gone), "{gone} in {f}");
            }
        }
    }

    #[test]
    fn both_callers_carry_the_shared_flags_so_they_cannot_drift() {
        let flags = isolation_flags();
        let haiku = haiku_script("haiku", 110).unwrap();
        let summary = summary_script(None, CID, "haiku").unwrap();
        for sc in [&haiku, &summary] {
            assert!(sc.contains(&flags), "missing {flags:?} in {sc}");
            assert!(sc.contains("if ! command -v claude"), "{sc}");
            assert_eq!(sc.matches("--tools").count(), 1, "{sc}");
        }
    }

    #[test]
    fn the_first_whole_tag_line_decides() {
        let v = ["run", "stop"];
        assert_eq!(
            parse_tagged("motd\nT=run\nhello\nT=stop\n", "T=", &v),
            Some(("run", "hello\nT=stop".to_string()))
        );
        assert_eq!(
            parse_tagged("T=stop\n", "T=", &v),
            Some(("stop", String::new()))
        );
        // Whole-line match only: a prefix or a mid-line tag is not a verdict.
        assert_eq!(parse_tagged("T=runaway\nsaid T=run\n", "T=", &v), None);
        assert_eq!(parse_tagged("", "T=", &v), None);
    }

    #[test]
    fn a_signed_out_claude_is_told_apart_from_an_answer() {
        for said in [
            "Login expired · Run /login to sign in again, or re-authenticate your Anthropic profile",
            "Invalid API key · Please run /login",
            "OAuth token has expired. Please obtain a new token or refresh your existing token.",
            r#"API Error: 401 {"type":"error","error":{"type":"authentication_error"}}"#,
        ] {
            assert!(says_signed_out(said), "{said}");
        }
        for said in ["[]", "Goal: fix login.", "", "I think we should"] {
            assert!(!says_signed_out(said), "{said}");
        }
        let late = format!("{}run /login", "x".repeat(400));
        assert!(!says_signed_out(&late));
    }

    #[test]
    fn only_a_failed_run_is_read_as_signed_out() {
        let said = "Login expired · Run /login to sign in again";
        let failed = Envelope {
            result: said.into(),
            is_error: true,
            ..Default::default()
        };
        let ok = Envelope {
            is_error: false,
            ..failed.clone()
        };
        assert!(run_signed_out(Some(&failed), said, b""));
        assert!(run_signed_out(None, said, b""));
        assert!(!run_signed_out(Some(&ok), said, b""));
        assert!(run_signed_out(
            None,
            "",
            b"warn: x\nInvalid API key \xc2\xb7 Please run /login\n"
        ));
        assert!(!run_signed_out(None, "", b"boom\n"));
    }

    #[test]
    fn the_signed_out_refusal_names_the_login_and_its_fix() {
        let e = signed_out_error("mercury", None);
        assert_eq!(e.code, "E_CLAUDE_CLI");
        assert_eq!(
            e.message,
            "Claude login expired on mercury: run `claude /login` there, then retry"
        );
        let e = signed_out_error("mercury", Some("work"));
        assert_eq!(
            e.message,
            "Claude login expired on mercury (login profile work): run \
             `CLAUDE_CONFIG_DIR=~/.claude-profiles/work claude /login` there, then retry"
        );
    }

    #[test]
    fn the_run_closes_stdin_only_when_asked() {
        assert!(
            run_capped("T=", "claude", 5, 10, true).ends_with("$t claude </dev/null | head -c 10")
        );
        assert!(run_capped("T=", "claude", 5, 10, false).ends_with("$t claude | head -c 10"));
    }

    /// TASK-291: Claude Code's limit answers, in each form it has printed,
    /// are a usage limit with their reset; an answer or error that only
    /// mentions a limit is not.
    #[test]
    fn a_usage_limit_answer_is_recognised_with_its_reset() {
        let l = usage_limit("Claude AI usage limit reached|1760536800").unwrap();
        assert_eq!(l.reset_at, Some(1_760_536_800));
        assert_eq!(l.said, None);
        let l = usage_limit("You've hit your limit · resets 3pm (Europe/Bratislava)\n").unwrap();
        assert_eq!(l.reset_at, None);
        assert_eq!(l.said.as_deref(), Some("resets 3pm (Europe/Bratislava)"));
        let l = usage_limit("5-hour limit reached ∙ resets 3pm").unwrap();
        assert_eq!(l.said.as_deref(), Some("resets 3pm"));
        let l = usage_limit("Weekly limit reached · resets Oct 20, 9am").unwrap();
        assert_eq!(l.said.as_deref(), Some("resets Oct 20, 9am"));
        let l = usage_limit(
            "Claude usage limit reached. Your limit will reset at 3pm (Europe/Bratislava).",
        )
        .unwrap();
        assert_eq!(l.said.as_deref(), Some("reset at 3pm (Europe/Bratislava)"));
        for not in [
            "",
            "[]",
            "not a JSON array",
            "API Error: 429 rate_limit_error",
            "Invalid API key · Please run /login",
        ] {
            assert_eq!(usage_limit(not), None, "{not:?}");
        }
        // Only the opening is read.
        let late = format!("{} usage limit reached", "x".repeat(400));
        assert_eq!(usage_limit(&late), None);
        assert!(
            run_usage_limit("", b"warn: x\nClaude AI usage limit reached|1760536800\n")
                .is_some_and(|l| l.reset_at == Some(1_760_536_800))
        );
        assert_eq!(run_usage_limit("", b"boom\n"), None);
    }
}
