//! Fleet's own headless agent runs: the one place that knows which CLI a
//! locked-down print-mode run starts and how its reply reads, so the
//! callers (the summary, the planner, the drafts, context help, the haiku
//! baseline) never spell `claude -p` themselves and another agent CLI can
//! take a run by adding a [`HeadlessAgent`].
//!
//! * [`HeadlessJob`] is what a caller asks for: a model, an optional prompt
//!   argument, an optional file the CLI reads on stdin, an optional
//!   conversation to fork.
//! * [`HeadlessAgent`] turns it into the command words ([`command`]), the
//!   script line that reports a missing CLI ([`missing_check`]) and the
//!   login profile's environment ([`login_env`]). [`CLAUDE_CODE`] is the
//!   only one: its words are exactly what each caller built before, which
//!   the callers' own script tests pin.
//! * [`read_reply`] is the shared end of a run whose script printed its
//!   tag line: the `--output-format json` envelope booked, a signed-out
//!   login told apart, an error answer refused.
//!
//! The script around the command (the tag line, the host-side `timeout`,
//! the output cap) stays [`super::claude_print`]'s.
//!
//! [`command`]: HeadlessAgent::command
//! [`missing_check`]: HeadlessAgent::missing_check
//! [`login_env`]: HeadlessAgent::login_env

use super::claude_print::{self, Envelope};
use crate::ipc_error::{codes, IpcError};
use crate::shell::quote;

/// One headless run, as a caller asks for it. Every value is quoted by the
/// agent; a caller validates the model against its own list first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HeadlessJob<'a> {
    /// The model alias (`haiku`, `sonnet`, `opus`).
    pub model: &'a str,
    /// The prompt as the command's one argument; `None` reads it from stdin.
    pub prompt: Option<&'a str>,
    /// The name of a shell variable holding a file the CLI reads on stdin
    /// (`d` for `<"$d"`); `None` leaves stdin to the script.
    pub stdin_file: Option<&'a str>,
    /// A conversation id to fork and continue (the transcript stays as it
    /// was); `None` starts with no conversation.
    pub fork_of: Option<&'a str>,
    /// The tools the run may use; no tool at all by default.
    pub tools: ToolPolicy<'a>,
}

/// What a headless run may use. A run has no person to answer a prompt, so
/// every policy denies whatever it does not name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolPolicy<'a> {
    /// No tool ([`claude_print::isolation_flags`]).
    #[default]
    None,
    /// Read, grep and glob inside the working directory, and these exact
    /// shell commands (none: no shell). Never a write, never the network.
    ReadOnly { bash: &'a [&'a str] },
}

/// An agent CLI that can run a [`HeadlessJob`] locked down: no tools, no
/// MCP server, no hooks, no transcript left on the host.
pub trait HeadlessAgent: Send + Sync {
    /// The agent's id (`claude`), as [`agent`] takes it.
    fn id(&self) -> &'static str;
    /// PURE: the command words for `job`, every value quoted.
    fn command(&self, job: &HeadlessJob<'_>) -> String;
    /// PURE: the script line that ends the script with `<tag>noclaude` when
    /// the CLI is not on the host's login PATH.
    fn missing_check(&self, tag: &str) -> String;
    /// The `export …; ` that makes the run use the login profile `profile`
    /// (validated first). Empty for no profile.
    fn login_env(&self, profile: Option<&str>) -> Result<String, String>;
    /// PURE: the reply envelope in the run's output, when it has one.
    fn reply(&self, out: &str) -> Option<Envelope>;
}

/// Claude Code in print mode.
pub struct ClaudeCode;

/// The agent fleet's headless runs use today.
pub static CLAUDE_CODE: ClaudeCode = ClaudeCode;

impl HeadlessAgent for ClaudeCode {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn command(&self, job: &HeadlessJob<'_>) -> String {
        let mut words = vec!["claude".to_string(), "-p".to_string()];
        if let Some(cid) = job.fork_of {
            words.extend(["--resume".into(), quote(cid), "--fork-session".into()]);
        }
        words.extend([
            "--model".into(),
            quote(job.model),
            "--output-format".into(),
            "json".into(),
            match job.tools {
                ToolPolicy::None => claude_print::isolation_flags(),
                ToolPolicy::ReadOnly { bash } => claude_print::tool_flags(bash),
            },
        ]);
        if let Some(p) = job.prompt {
            words.push(quote(p));
        }
        if let Some(var) = job.stdin_file {
            words.push(format!("<\"${var}\""));
        }
        words.join(" ")
    }

    fn missing_check(&self, tag: &str) -> String {
        claude_print::noclaude_check(tag)
    }

    fn login_env(&self, profile: Option<&str>) -> Result<String, String> {
        let Some(p) = profile else {
            return Ok(String::new());
        };
        crate::validate::claude_profile(p).map_err(|e| e.message)?;
        Ok(format!(
            "export CLAUDE_CONFIG_DIR=\"$HOME/.claude-profiles/\"{}; ",
            quote(p)
        ))
    }

    fn reply(&self, out: &str) -> Option<Envelope> {
        claude_print::parse_envelope(out)
    }
}

/// The agent with id `id`, if fleet has one.
pub fn agent(id: &str) -> Option<&'static dyn HeadlessAgent> {
    match id {
        "claude" => Some(&CLAUDE_CODE),
        _ => None,
    }
}

/// The agent fleet's headless runs use.
pub fn default_agent() -> &'static dyn HeadlessAgent {
    &CLAUDE_CODE
}

/// Where a run happened, for the refusals [`read_reply`] writes.
#[derive(Debug, Clone, Copy)]
pub struct RunAt<'a> {
    pub host: &'a str,
    /// The login profile the run used, if any.
    pub profile: Option<&'a str>,
    /// The run's name in a refusal ("the draft run").
    pub what: &'a str,
}

/// The shared end of a run whose tag line said `run` and whose text after
/// it is `text`: the envelope is booked through `book` (with `None` when
/// there is none) whatever it says, then a signed-out login is refused with
/// [`claude_print::signed_out_error`], and an error or empty answer with
/// `E_CLAUDE_CLI`. With no envelope (an older CLI) the text itself is the
/// answer. Returns the answer and the envelope.
pub fn read_reply(
    agent: &dyn HeadlessAgent,
    text: String,
    stderr: &[u8],
    at: RunAt<'_>,
    book: impl FnOnce(Option<&Envelope>),
) -> Result<(String, Option<Envelope>), IpcError> {
    match agent.reply(&text) {
        Some(env) => {
            book(Some(&env));
            if claude_print::run_signed_out(Some(&env), &env.result, stderr) {
                return Err(claude_print::signed_out_error(at.host, at.profile));
            }
            if env.is_error || env.result.trim().is_empty() {
                return Err(IpcError::new(
                    codes::E_CLAUDE_CLI,
                    format!("{} failed: claude answered with an error", at.what),
                ));
            }
            Ok((env.result.clone(), Some(env)))
        }
        None => {
            book(None);
            if claude_print::run_signed_out(None, &text, stderr) {
                return Err(claude_print::signed_out_error(at.host, at.profile));
            }
            Ok((text, None))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLAGS: &str = r#"--settings '{"disableAllHooks":true}' --tools '' --strict-mcp-config --no-session-persistence"#;

    #[test]
    fn claude_code_spells_each_job_shape() {
        let a = default_agent();
        assert_eq!(a.id(), "claude");
        let base = HeadlessJob {
            model: "haiku",
            ..Default::default()
        };
        assert_eq!(
            a.command(&base),
            format!("claude -p --model 'haiku' --output-format json {FLAGS}")
        );
        let arg = HeadlessJob {
            prompt: Some("say it's done"),
            ..base
        };
        assert_eq!(
            a.command(&arg),
            format!("claude -p --model 'haiku' --output-format json {FLAGS} 'say it'\\''s done'")
        );
        let file = HeadlessJob {
            stdin_file: Some("d"),
            ..arg
        };
        assert!(a.command(&file).ends_with(r#" 'say it'\''s done' <"$d""#));
        let fork = HeadlessJob {
            fork_of: Some("0f8fad5b-d9cb-469f-a165-70867728950e"),
            ..arg
        };
        assert!(a.command(&fork).starts_with(
            "claude -p --resume '0f8fad5b-d9cb-469f-a165-70867728950e' --fork-session --model 'haiku' "
        ));
    }

    #[test]
    fn only_known_agents_are_found() {
        assert_eq!(agent("claude").map(|a| a.id()), Some("claude"));
        assert!(agent("codex").is_none());
        assert!(agent("").is_none());
    }

    #[test]
    fn the_login_env_is_validated_and_quoted() {
        let a = default_agent();
        assert_eq!(a.login_env(None).unwrap(), "");
        assert_eq!(
            a.login_env(Some("work")).unwrap(),
            "export CLAUDE_CONFIG_DIR=\"$HOME/.claude-profiles/\"'work'; "
        );
        assert!(a.login_env(Some("../x")).is_err());
    }

    fn at() -> RunAt<'static> {
        RunAt {
            host: "mercury",
            profile: None,
            what: "the draft run",
        }
    }

    #[test]
    fn a_reply_is_booked_whatever_it_says() {
        let ok = r#"{"type":"result","result":"hi","usage":{"input_tokens":3,"output_tokens":1},"total_cost_usd":0.001}"#;
        let mut booked = None;
        let (text, env) = read_reply(default_agent(), ok.into(), b"", at(), |e| {
            booked = Some(e.cloned())
        })
        .unwrap();
        assert_eq!(text, "hi");
        assert_eq!(env.unwrap().cost_microusd, Some(1000));
        assert_eq!(booked.unwrap().unwrap().input_tokens, Some(3));

        let err = r#"{"type":"result","result":"","is_error":true}"#;
        let mut n = 0;
        let e = read_reply(default_agent(), err.into(), b"", at(), |_| n += 1).unwrap_err();
        assert_eq!(n, 1);
        assert_eq!(e.code, "E_CLAUDE_CLI");
        assert_eq!(
            e.message,
            "the draft run failed: claude answered with an error"
        );
    }

    #[test]
    fn a_signed_out_run_and_a_bare_text_are_told_apart() {
        let out = r#"{"type":"result","result":"Login expired · Run /login","is_error":true}"#;
        let e = read_reply(default_agent(), out.into(), b"", at(), |_| {}).unwrap_err();
        assert!(e.message.starts_with("Claude login expired on mercury"));

        let mut booked_none = false;
        let (text, env) = read_reply(default_agent(), "plain text".into(), b"", at(), |e| {
            booked_none = e.is_none()
        })
        .unwrap();
        assert_eq!((text.as_str(), env), ("plain text", None));
        assert!(booked_none);

        let e = read_reply(
            default_agent(),
            "Invalid API key · Please run /login".into(),
            b"",
            at(),
            |_| {},
        )
        .unwrap_err();
        assert_eq!(e.code, "E_CLAUDE_CLI");
    }
}
