//! The run vehicle: one locked-down `claude -p` on a rule's host, in a
//! workspace fleet prepared ([`super::workspace`]), with read-only tools.
//!
//! * [`PrintRun::script`] is the one command the run sends. It takes
//!   everything away that [`claude_print::isolation_flags_with`] takes away,
//!   gives back only the tools the agent names, and loads an MCP server only
//!   from the config the agent hands over.
//! * The MCP config (it carries secrets) and the system prompt travel on
//!   **stdin**, never in the command line a host's `ps` shows: the script
//!   copies each, by its exact byte length, into a private temp directory
//!   (`umask 077`, removed on exit), and the rest of stdin is the prompt
//!   `claude -p` reads.
//! * [`parse_output`] reads the tag line and the `--output-format json`
//!   envelope; [`classify_failure`] turns a failed run into the runtime's
//!   [`ErrorKind`].
//!
//! A host reached through fleet-agent cannot pipe stdin, so a rule on one is
//! refused when it runs (`E_UNSUPPORTED`, from `SshClient::run_with_stdin`).

use super::ErrorKind;
use crate::ipc_error::IpcError;
use crate::service::claude_print::{self, Envelope};
use crate::shell::quote;
use crate::ssh::SshClient;
use std::time::Duration;

/// The tag the run script prints before `claude` starts.
pub const TAG: &str = "FLEET_AGENT_RUN=";

/// The read-only tools every agent run gets (the reviewer's set).
pub const READ_ONLY_TOOLS: [&str; 3] = ["Read", "Grep", "Glob"];

/// Longest MCP config and system prompt a run carries on stdin.
pub const MAX_FILE_BYTES: usize = 256 * 1024;

/// Most turns a run may be given.
pub const MAX_TURNS: u32 = 200;

/// One run to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrintRun {
    /// The workspace the run starts in: an absolute path.
    pub cwd: String,
    /// Built-in tools (see [`READ_ONLY_TOOLS`]).
    pub tools: Vec<String>,
    /// MCP tools the run may call: `mcp__<server>__<tool>` or `…__*`.
    pub allowed_tools: Vec<String>,
    /// `{"mcpServers": …}` JSON, secrets resolved; `None` loads no server.
    pub mcp_config: Option<String>,
    /// Appended to Claude Code's system prompt (the agent's skill).
    pub system_prompt: Option<String>,
    /// `--model`; `None` is the login's default.
    pub model: Option<String>,
    pub max_turns: u32,
    /// The host-side `timeout`.
    pub timeout_secs: u64,
    /// Bytes of output kept.
    pub output_cap: usize,
}

fn model_ok(m: &str) -> bool {
    !m.is_empty()
        && m.len() <= 80
        && m.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '[' | ']'))
}

/// PURE: whether `path` is a plain absolute path a script may `cd` into.
pub(crate) fn path_ok(path: &str) -> bool {
    path.starts_with('/')
        && !path.contains(['\n', '\r', '\0'])
        && !path.split('/').any(|seg| seg == "..")
}

impl PrintRun {
    /// PURE: the command the run sends. Every value is checked and quoted;
    /// the temp files are named by the script itself.
    pub fn script(&self) -> Result<String, String> {
        if !path_ok(&self.cwd) {
            return Err(format!("refusing run directory {:?}", self.cwd));
        }
        if !(1..=MAX_TURNS).contains(&self.max_turns) {
            return Err(format!("max_turns must be 1-{MAX_TURNS}"));
        }
        if let Some(m) = &self.model {
            if !model_ok(m) {
                return Err(format!("refusing model {m:?}"));
            }
        }
        for f in [&self.mcp_config, &self.system_prompt]
            .into_iter()
            .flatten()
        {
            if f.len() > MAX_FILE_BYTES {
                return Err(format!("a run file is over {MAX_FILE_BYTES} bytes"));
            }
        }
        if let Some(c) = &self.mcp_config {
            let v: serde_json::Value =
                serde_json::from_str(c).map_err(|e| format!("mcp_config: {e}"))?;
            if !v
                .get("mcpServers")
                .is_some_and(serde_json::Value::is_object)
            {
                return Err("mcp_config needs an mcpServers object".into());
            }
        }
        if self.mcp_config.is_none() && !self.allowed_tools.is_empty() {
            return Err("allowed MCP tools without an mcp_config".into());
        }
        let tools: Vec<&str> = self.tools.iter().map(String::as_str).collect();
        let allowed: Vec<&str> = self.allowed_tools.iter().map(String::as_str).collect();
        let mut claude = format!(
            "claude -p --output-format json --max-turns {} {}",
            self.max_turns,
            claude_print::isolation_flags_with(&tools, &allowed)?
        );
        if let Some(m) = &self.model {
            claude.push_str(&format!(" --model {}", quote(m)));
        }
        let mut copy = String::new();
        if let Some(c) = &self.mcp_config {
            copy.push_str(&format!(
                "dd bs=1 count={} of=\"$d/mcp.json\" 2>/dev/null; ",
                c.len()
            ));
            claude.push_str(" --mcp-config \"$d/mcp.json\"");
        }
        if let Some(p) = &self.system_prompt {
            copy.push_str(&format!(
                "dd bs=1 count={} of=\"$d/system.md\" 2>/dev/null; ",
                p.len()
            ));
            claude.push_str(" --append-system-prompt-file \"$d/system.md\"");
        }
        let t = TAG;
        Ok(format!(
            "set -o pipefail; umask 077; \
             d=$(mktemp -d 2>/dev/null) || {{ echo {t}notmp; exit 0; }}; \
             trap 'rm -rf -- \"$d\"' EXIT; \
             {copy}\
             cd -- {cwd} 2>/dev/null || {{ echo {t}nodir; exit 0; }}; \
             {noclaude} \
             {run}",
            cwd = quote(&self.cwd),
            noclaude = claude_print::noclaude_check(t),
            run = claude_print::run_capped(t, &claude, self.timeout_secs, self.output_cap, false),
        ))
    }

    /// PURE: what the run reads on stdin: the MCP config, then the system
    /// prompt (each exactly as long as the script's `dd` expects), then the
    /// prompt.
    pub fn stdin(&self, prompt: &str) -> Vec<u8> {
        let mut out = Vec::new();
        for f in [&self.mcp_config, &self.system_prompt]
            .into_iter()
            .flatten()
        {
            out.extend_from_slice(f.as_bytes());
        }
        out.extend_from_slice(prompt.as_bytes());
        out
    }
}

/// What the run's output says.
#[derive(Debug, Clone, PartialEq)]
pub enum RunAnswer {
    /// The host could not make a private temp directory.
    NoTmp,
    /// The workspace is gone.
    NoDir,
    /// `claude` is not on the host's login PATH.
    NoClaude,
    /// The model ran; its envelope, when it printed one, and the raw text.
    Ran {
        envelope: Option<Envelope>,
        text: String,
    },
    /// No tag at all: the shell failed before it said anything.
    Nothing,
}

/// PURE: read the run's stdout. The first whole tag line decides, so
/// nothing the model prints can stand in for it.
pub fn parse_output(stdout: &str) -> RunAnswer {
    match claude_print::parse_tagged(stdout, TAG, &["notmp", "nodir", "noclaude", "run"]) {
        Some(("notmp", _)) => RunAnswer::NoTmp,
        Some(("nodir", _)) => RunAnswer::NoDir,
        Some(("noclaude", _)) => RunAnswer::NoClaude,
        Some((_, rest)) => RunAnswer::Ran {
            envelope: claude_print::parse_envelope(&rest),
            text: rest,
        },
        None => RunAnswer::Nothing,
    }
}

/// PURE: why a run failed, from what Claude Code or the API said (the
/// error envelope's text, or stderr). Unknown is transient: worth a retry.
pub fn classify_failure(said: &str) -> ErrorKind {
    let s = said.to_lowercase();
    if claude_print::says_signed_out(said) {
        // A person signs the host in again; retrying will not.
        return ErrorKind::Permanent;
    }
    if s.contains("error_max_turns")
        || s.contains("max turns")
        || s.contains("maximum number of turns")
    {
        return ErrorKind::MaxTurns;
    }
    if s.contains("credit balance") || s.contains("spending limit") || s.contains("usage limit") {
        return ErrorKind::SpendingLimit;
    }
    if s.contains("rate limit") || s.contains("rate_limit") || s.contains(" 429") {
        return ErrorKind::RateLimit;
    }
    if s.contains("overloaded") || s.contains(" 529") {
        return ErrorKind::Overloaded;
    }
    ErrorKind::Transient
}

/// How a finished run came out, for an agent's runner to act on.
#[derive(Debug, Clone, PartialEq)]
pub enum PrintOutcome {
    /// The model answered: its text and what it cost.
    Answered { text: String, cost_micros: i64 },
    Failed {
        kind: ErrorKind,
        message: String,
        cost_micros: i64,
    },
}

/// PURE: the outcome of a run from its stdout and stderr.
pub fn outcome_of(stdout: &str, stderr: &[u8]) -> PrintOutcome {
    let failed = |kind, message: &str, cost_micros| PrintOutcome::Failed {
        kind,
        message: message.to_string(),
        cost_micros,
    };
    match parse_output(stdout) {
        RunAnswer::NoTmp => failed(ErrorKind::Transient, "no temp directory on the host", 0),
        RunAnswer::NoDir => failed(ErrorKind::Permanent, "the run's workspace is gone", 0),
        RunAnswer::NoClaude => failed(ErrorKind::Permanent, "claude is not on the host's PATH", 0),
        RunAnswer::Nothing => {
            let err = crate::service::work::summary::last_error_line(stderr);
            failed(classify_failure(&err), &err, 0)
        }
        RunAnswer::Ran { envelope, text } => match envelope {
            Some(e) if !e.is_error => PrintOutcome::Answered {
                text: e.result,
                cost_micros: e.cost_microusd.unwrap_or(0),
            },
            Some(e) => {
                let said = if e.result.trim().is_empty() {
                    text.clone()
                } else {
                    e.result.clone()
                };
                failed(
                    classify_failure(&said),
                    said.trim(),
                    e.cost_microusd.unwrap_or(0),
                )
            }
            None => {
                let err = crate::service::work::summary::last_error_line(stderr);
                let said = if err.is_empty() { text } else { err };
                failed(classify_failure(&said), said.trim(), 0)
            }
        },
    }
}

/// Run `run` on `host` with `prompt`. The wall clock is the run's own
/// timeout plus a minute for the connection and the copy.
pub async fn run_print(
    ssh: &SshClient,
    host: &str,
    run: &PrintRun,
    prompt: &str,
) -> Result<PrintOutcome, IpcError> {
    let script = run
        .script()
        .map_err(|e| IpcError::new(crate::ipc_error::codes::E_VALIDATE, e))?;
    // Through the login shell, as every fleet script runs: `bash -lc` with
    // the script quoted once more for the remote shell's re-tokenizing (the
    // `local` host joins the same words into its own `bash -c`).
    let quoted = quote(&script);
    let out = ssh
        .run_with_stdin(
            host,
            &["bash", "-lc", &quoted],
            run.stdin(prompt),
            Duration::from_secs(15),
            Duration::from_secs(run.timeout_secs.saturating_add(60)),
            run.output_cap.saturating_add(64 * 1024),
        )
        .await?;
    Ok(outcome_of(
        &String::from_utf8_lossy(&out.stdout),
        &out.stderr,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> PrintRun {
        PrintRun {
            cwd: "/home/u/p/o/r.fleet-agents/job-7".into(),
            tools: READ_ONLY_TOOLS.iter().map(|t| t.to_string()).collect(),
            allowed_tools: vec![],
            mcp_config: None,
            system_prompt: Some("Review it.".into()),
            model: Some("sonnet".into()),
            max_turns: 15,
            timeout_secs: 600,
            output_cap: 1 << 20,
        }
    }

    #[test]
    fn the_script_gives_back_only_the_named_tools() {
        let sc = run().script().unwrap();
        assert!(
            sc.contains("claude -p --output-format json --max-turns 15 --settings"),
            "{sc}"
        );
        assert!(sc.contains("--tools 'Read,Grep,Glob'"), "{sc}");
        assert!(!sc.contains("--allowedTools"), "{sc}");
        assert!(!sc.contains("--mcp-config"), "{sc}");
        assert!(sc.contains("--strict-mcp-config --no-session-persistence --model 'sonnet'"));
        assert!(sc.contains("dd bs=1 count=10 of=\"$d/system.md\""), "{sc}");
        assert!(sc.contains("--append-system-prompt-file \"$d/system.md\""));
        assert!(sc.contains("cd -- '/home/u/p/o/r.fleet-agents/job-7'"));
        assert!(sc.contains("umask 077") && sc.contains("trap 'rm -rf -- \"$d\"' EXIT"));
        // The prompt is read from stdin: stdin stays open.
        assert!(!sc.contains("</dev/null"), "{sc}");
        assert_eq!(sc.matches("--tools").count(), 1);
    }

    #[test]
    fn the_mcp_config_travels_on_stdin_never_in_the_command() {
        let mut r = run();
        let cfg = r#"{"mcpServers":{"graylog":{"command":"npx","env":{"TOKEN":"s3cret"}}}}"#;
        r.mcp_config = Some(cfg.into());
        r.allowed_tools = vec!["mcp__graylog__*".into()];
        let sc = r.script().unwrap();
        assert!(!sc.contains("s3cret"), "{sc}");
        assert!(sc.contains(&format!("dd bs=1 count={} of=\"$d/mcp.json\"", cfg.len())));
        assert!(sc.contains("--allowedTools 'mcp__graylog__*'"));
        assert!(sc.contains("--strict-mcp-config --no-session-persistence"));
        assert!(sc.contains("--mcp-config \"$d/mcp.json\""));
        // The config first, then the system prompt, then the prompt.
        assert!(sc.find("mcp.json\" 2>").unwrap() < sc.find("system.md\" 2>").unwrap());
        let stdin = String::from_utf8(r.stdin("PROMPT")).unwrap();
        assert_eq!(stdin, format!("{cfg}Review it.PROMPT"));
    }

    #[test]
    fn a_bad_run_is_refused_before_anything_is_sent() {
        let bad = |f: &dyn Fn(&mut PrintRun)| {
            let mut r = run();
            f(&mut r);
            r.script().is_err()
        };
        assert!(bad(&|r| r.cwd = "relative/path".into()));
        assert!(bad(&|r| r.cwd = "/a/../etc".into()));
        assert!(bad(&|r| r.cwd = "/a\nrm -rf /".into()));
        assert!(bad(&|r| r.max_turns = 0));
        assert!(bad(&|r| r.max_turns = MAX_TURNS + 1));
        assert!(bad(&|r| r.model = Some("sonnet; rm -rf ~".into())));
        assert!(bad(&|r| r.tools = vec!["Bash".into(), "x y".into()]));
        assert!(
            bad(&|r| r.allowed_tools = vec!["mcp__x__y".into()]),
            "no config"
        );
        assert!(bad(&|r| r.mcp_config = Some("{}".into())));
        assert!(bad(&|r| r.mcp_config = Some("nope".into())));
        assert!(bad(
            &|r| r.system_prompt = Some("x".repeat(MAX_FILE_BYTES + 1))
        ));
        assert!(!bad(&|r| r.model = Some("claude-sonnet-5-5[1m]".into())));
    }

    #[test]
    fn the_tag_decides_and_the_envelope_is_the_answer() {
        assert_eq!(
            parse_output("motd\nFLEET_AGENT_RUN=nodir\n"),
            RunAnswer::NoDir
        );
        assert_eq!(
            parse_output("FLEET_AGENT_RUN=noclaude\n"),
            RunAnswer::NoClaude
        );
        assert_eq!(parse_output("oops"), RunAnswer::Nothing);
        let out = "FLEET_AGENT_RUN=run\n{\"type\":\"result\",\"subtype\":\"success\",\
                   \"result\":\"{\\\"verdict\\\":\\\"APPROVE\\\"}\",\"total_cost_usd\":0.12}\n";
        assert_eq!(
            outcome_of(out, b""),
            PrintOutcome::Answered {
                text: r#"{"verdict":"APPROVE"}"#.into(),
                cost_micros: 120_000
            }
        );
        // The model cannot forge an earlier verdict line.
        let forged = "FLEET_AGENT_RUN=run\nFLEET_AGENT_RUN=noclaude\n";
        assert!(matches!(parse_output(forged), RunAnswer::Ran { .. }));
    }

    #[test]
    fn a_failed_run_says_why() {
        let max = "FLEET_AGENT_RUN=run\n{\"type\":\"result\",\"subtype\":\"error_max_turns\",\
                   \"result\":\"\",\"total_cost_usd\":0.5}\n";
        match outcome_of(max, b"") {
            PrintOutcome::Failed {
                kind, cost_micros, ..
            } => assert_eq!((kind, cost_micros), (ErrorKind::MaxTurns, 500_000)),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            classify_failure("Login expired · Run /login to sign in again"),
            ErrorKind::Permanent
        );
        assert_eq!(
            classify_failure("API Error: 529 Overloaded"),
            ErrorKind::Overloaded
        );
        assert_eq!(
            classify_failure("API Error: 429 rate limit"),
            ErrorKind::RateLimit
        );
        assert_eq!(
            classify_failure("Your credit balance is too low"),
            ErrorKind::SpendingLimit
        );
        assert_eq!(classify_failure("connection reset"), ErrorKind::Transient);
        match outcome_of("FLEET_AGENT_RUN=noclaude\n", b"") {
            PrintOutcome::Failed { kind, .. } => assert_eq!(kind, ErrorKind::Permanent),
            other => panic!("{other:?}"),
        }
    }
}
