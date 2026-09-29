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
    fn the_run_closes_stdin_only_when_asked() {
        assert!(
            run_capped("T=", "claude", 5, 10, true).ends_with("$t claude </dev/null | head -c 10")
        );
        assert!(run_capped("T=", "claude", 5, 10, false).ends_with("$t claude | head -c 10"));
    }
}
