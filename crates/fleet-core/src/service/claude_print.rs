//! The one locked-down `claude -p` run fleet starts on a host, shared by the
//! dead-session summary ([`super::work::summary`]) and the Jev `claude -p
//! haiku` baseline ([`super::decide::haiku`]).
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
pub fn signed_out_error(host: &str, profile: Option<&str>) -> crate::ipc_error::IpcError {
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
}
