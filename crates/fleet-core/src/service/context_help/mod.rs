//! Context help in the command line: a person asks Haiku (or whichever
//! model answers, see [`HelpModel`]) about what they are typing, with the
//! history in front of it as context.
//!
//! Two surfaces share one implementation:
//!
//! * **[`Surface::Shell`]** — a session's shell terminal (redesign step
//!   5.3). The context is the terminal's own scrollback, read from its tmux
//!   session on the host ([`shell_history`]): the commands the person ran
//!   and what they printed.
//! * **[`Surface::Composer`]** — the conversation composer, Control's
//!   included. The context is the person's earlier prompts (the composer's
//!   ↑ recall) and the commands the agent accepts (`/task`, `/plan`, a
//!   project's skills…), both sent by the caller.
//!
//! Either way the answer is a few lines of text plus, at most, one
//! proposed [`HelpAnswer::command`]. Nothing here runs it: the desktop puts
//! it on the person's prompt line, where only their own Enter sends it.
//! A shell command is one line or none ([`clean_command`]): a newline
//! pasted into a terminal would run it.
//!
//! **The model is replaceable.** [`ask`] builds the request (a fixed
//! instruction per surface, [`SHELL_INSTRUCTION`] / [`COMPOSER_INSTRUCTION`],
//! and the context as text, [`context_text`]) and reads the reply
//! ([`parse_answer`]); what answers is a [`HelpModel`]. [`ClaudeOnHost`] is
//! the one fleet ships: `claude -p --model <alias>` (Haiku unless
//! `work.help_model` says otherwise) on the session's own host, under its
//! credential profile, with the shared
//! [`claude_print::isolation_flags`] (no tools, no MCP, no hooks, no
//! transcript), the context on stdin, never in argv. Another backend (Jev,
//! an API, a local model) implements [`HelpModel`] and nothing else moves.
//!
//! **Bounded and untrusted.** Each piece of the context is capped
//! ([`QUESTION_MAX_CHARS`], [`LINE_MAX_CHARS`], [`HISTORY_MAX_ITEMS`],
//! [`COMMANDS_MAX_ITEMS`]) and the whole is the newest
//! [`CONTEXT_MAX_BYTES`]; the reply is redacted and capped
//! ([`ANSWER_MAX_CHARS`]). Nothing is stored but the run's cost.

use crate::ipc_error::{codes, IpcError};
use crate::service::claude_print;
use crate::service::decide::summary_check::tail_bytes;
use crate::service::settings;
use crate::shell::quote;
use crate::ssh::SshExec;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Where the person asked from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    /// A session's shell terminal.
    Shell,
    /// The conversation composer (Control's included).
    Composer,
}

/// What the person asked, with the context the caller holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelpRequest {
    /// Their question; empty asks about [`Self::line`] alone.
    #[serde(default)]
    pub question: String,
    /// What is on the prompt line now (the shell's input or the composer's
    /// draft).
    #[serde(default)]
    pub line: String,
    /// Earlier entries, oldest first: the composer's prompts, or, for a
    /// shell whose scrollback could not be read, what the window shows.
    #[serde(default)]
    pub history: Vec<String>,
    /// The commands the prompt line accepts, one per entry
    /// (`/plan #KEY or a goal — Plan subtasks…`). Composer only.
    #[serde(default)]
    pub commands: Vec<String>,
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelpAnswer {
    /// A few lines of plain text.
    pub answer: String,
    /// One proposal for the prompt line, never run by fleet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Who answered (`haiku`), as the Drafted label names it.
    pub model: String,
    /// Where it ran; empty for a backend that runs on no host.
    #[serde(default)]
    pub host_alias: String,
    /// History entries (or scrollback lines) the model was shown.
    pub history_items: u32,
    /// When it was drafted, unix seconds.
    pub at: i64,
}

/// The longest question kept, in characters.
pub const QUESTION_MAX_CHARS: usize = 2_000;
/// The longest prompt line kept, in characters.
pub const LINE_MAX_CHARS: usize = 4_000;
/// History entries kept, the newest.
pub const HISTORY_MAX_ITEMS: usize = 40;
/// Characters of one history entry kept.
pub const HISTORY_ITEM_MAX_CHARS: usize = 2_000;
/// Commands kept, in the caller's order.
pub const COMMANDS_MAX_ITEMS: usize = 80;
/// Scrollback lines a shell's history reads.
pub const SHELL_HISTORY_LINES: u32 = 300;
/// The newest bytes of the whole context the model reads. It rides the
/// script as base64, far below what one argument may be.
pub const CONTEXT_MAX_BYTES: usize = 40_000;
/// The longest answer kept, in characters.
pub const ANSWER_MAX_CHARS: usize = 2_000;
/// The longest proposed command kept, in characters.
pub const COMMAND_MAX_CHARS: usize = 2_000;

/// The fixed instruction for a shell. Nothing from the caller goes into it.
pub const SHELL_INSTRUCTION: &str = "The text on stdin is the recent history of a person's \
shell terminal (the commands they ran and what they printed, oldest first), then the line they \
are typing and their question. Help them at the command line: explain an error, or propose the \
next command. Reply with exactly one line of JSON and nothing else: {\"answer\": \"<at most 6 \
short lines of plain text>\", \"command\": \"<one single-line shell command for their prompt \
line, or an empty string>\"}. Use only the history and general knowledge; never claim a command \
ran. Do not use tools.";

/// The fixed instruction for the composer. Nothing from the caller goes
/// into it.
pub const COMPOSER_INSTRUCTION: &str = "The text on stdin is a person's earlier prompts to a \
coding agent (oldest first), the commands its prompt line accepts, then the text they are \
typing and their question. Help them write the next prompt or command. Reply with exactly one \
line of JSON and nothing else: {\"answer\": \"<at most 6 short lines of plain text>\", \
\"command\": \"<the text to put in their message box, or an empty string>\"}. Prefer one of the \
listed commands when one fits, with its arguments filled in. Use only the history and general \
knowledge. Do not use tools.";

/// PURE: the instruction for `surface`.
pub fn instruction(surface: Surface) -> &'static str {
    match surface {
        Surface::Shell => SHELL_INSTRUCTION,
        Surface::Composer => COMPOSER_INSTRUCTION,
    }
}

fn cut(text: &str, max: usize) -> String {
    let mut s: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        s.push('…');
    }
    s
}

/// PURE: the context the model reads on stdin, and how many history
/// entries it holds. `scrollback` (a shell's, when it was read) stands in
/// for [`HelpRequest::history`]. The newest [`CONTEXT_MAX_BYTES`] are kept
/// and the question always survives the cut: it is last.
pub fn context_text(req: &HelpRequest, scrollback: Option<&str>) -> (String, u32) {
    let mut out = String::new();
    let items = match scrollback {
        Some(text) => {
            let text = text.trim_end();
            out.push_str("## Terminal history\n");
            out.push_str(text);
            out.push('\n');
            text.lines().count()
        }
        None => {
            let from = req.history.len().saturating_sub(HISTORY_MAX_ITEMS);
            let kept: Vec<&String> = req.history[from..]
                .iter()
                .filter(|h| !h.trim().is_empty())
                .collect();
            if !kept.is_empty() {
                out.push_str("## History (oldest first)\n");
                for h in &kept {
                    out.push_str("- ");
                    out.push_str(&cut(h.trim(), HISTORY_ITEM_MAX_CHARS).replace('\n', "\n  "));
                    out.push('\n');
                }
            }
            kept.len()
        }
    };
    let commands: Vec<&String> = req
        .commands
        .iter()
        .filter(|c| !c.trim().is_empty())
        .take(COMMANDS_MAX_ITEMS)
        .collect();
    if !commands.is_empty() {
        out.push_str("\n## Commands\n");
        for c in commands {
            out.push_str(&cut(c.trim(), 300));
            out.push('\n');
        }
    }
    out.push_str("\n## Typing now\n");
    out.push_str(&cut(&req.line, LINE_MAX_CHARS));
    out.push_str("\n\n## Question\n");
    let q = req.question.trim();
    if q.is_empty() {
        out.push_str("Help me with the line I am typing.");
    } else {
        out.push_str(&cut(q, QUESTION_MAX_CHARS));
    }
    out.push('\n');
    // The question and the line are last and far below the cap, so the
    // tail always holds them; only the oldest history is cut.
    (
        tail_bytes(&out, CONTEXT_MAX_BYTES).to_string(),
        u32::try_from(items).unwrap_or(u32::MAX),
    )
}

/// PURE: a proposal as the prompt line may take it, or `None`. A shell's
/// is one line with no control character (a pasted newline would run it).
/// Code fences and a leading `$ ` prompt are stripped.
pub fn clean_command(surface: Surface, raw: &str) -> Option<String> {
    let mut s = raw.trim();
    if let Some(rest) = s.strip_prefix("```") {
        // ```bash\n…\n```
        let rest = rest.split_once('\n').map_or("", |(_, body)| body);
        s = rest.trim_end().strip_suffix("```").unwrap_or(rest).trim();
    }
    let s = s.trim_matches('`').trim();
    let s = match surface {
        Surface::Shell => s.strip_prefix("$ ").unwrap_or(s).trim(),
        Surface::Composer => s,
    };
    if s.is_empty() || s.chars().count() > COMMAND_MAX_CHARS {
        return None;
    }
    let bad = |c: char| match surface {
        Surface::Shell => c.is_control(),
        Surface::Composer => c.is_control() && c != '\n' && c != '\t',
    };
    if s.chars().any(bad) {
        return None;
    }
    Some(crate::logging::redact(s).into_owned())
}

/// PURE: the reply as a person sees it: the JSON the instruction asked
/// for, or, when there is none, the whole text as the answer and no
/// command. Redacted and capped.
pub fn parse_answer(surface: Surface, reply: &str) -> (String, Option<String>) {
    let reply = reply.trim();
    let json = reply
        .find('{')
        .zip(reply.rfind('}'))
        .filter(|(a, b)| a < b)
        .and_then(|(a, b)| serde_json::from_str::<serde_json::Value>(&reply[a..=b]).ok());
    let (answer, command) = match json.as_ref().and_then(|v| v.as_object()) {
        Some(o) => (
            o.get("answer")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            o.get("command")
                .and_then(|v| v.as_str())
                .and_then(|c| clean_command(surface, c)),
        ),
        None => (reply.to_string(), None),
    };
    let answer = crate::logging::redact(answer.trim())
        .chars()
        .take(ANSWER_MAX_CHARS)
        .collect::<String>();
    (answer, command)
}

/// One model's answer to an instruction and a context.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Completion {
    /// The model's text.
    pub text: String,
    /// What the run reported it cost, when it did.
    pub usage: Option<claude_print::Envelope>,
}

/// Whatever answers. [`ClaudeOnHost`] is fleet's; a test, or another
/// backend, implements this and nothing else.
#[async_trait::async_trait]
pub trait HelpModel: Send + Sync {
    /// Who answers, as the Drafted label names it (`haiku`).
    fn model(&self) -> String;
    /// Where it runs; empty when on no host.
    fn host(&self) -> String {
        String::new()
    }
    /// Answer `instruction` (fixed, fleet's) about `context` (the
    /// caller's, untrusted).
    async fn complete(&self, instruction: &str, context: &str) -> Result<Completion, IpcError>;
}

/// Ask `model` about `req` on `surface`. `scrollback` is a shell's history
/// when it was read ([`shell_history`]).
pub async fn ask(
    model: &dyn HelpModel,
    surface: Surface,
    req: &HelpRequest,
    scrollback: Option<&str>,
) -> Result<(HelpAnswer, Option<claude_print::Envelope>), IpcError> {
    if req.question.trim().is_empty() && req.line.trim().is_empty() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "ask a question, or type something to ask about",
        ));
    }
    let (context, history_items) = context_text(req, scrollback);
    let done = model.complete(instruction(surface), &context).await?;
    let (answer, command) = parse_answer(surface, &done.text);
    if answer.is_empty() && command.is_none() {
        return Err(IpcError::new(
            codes::E_CLAUDE_CLI,
            "the help run answered nothing",
        ));
    }
    Ok((
        HelpAnswer {
            answer,
            command,
            model: model.model(),
            host_alias: model.host(),
            history_items,
            at: crate::store::now_unix(),
        },
        done.usage,
    ))
}

// --- the shell's history -----------------------------------------------------

const CONNECT: Duration = Duration::from_secs(10);
const CAPTURE_WALL_CLOCK: Duration = Duration::from_secs(15);

/// PURE: the script that prints the newest [`SHELL_HISTORY_LINES`] of shell
/// terminal `n` of tmux session `tmux_name`, nothing when it is gone.
pub fn shell_history_script(tmux_name: &str, n: u32) -> String {
    let pane = crate::tmux::exact_pane(&crate::tmux::shell_terminal_name(tmux_name, n));
    format!(
        "tmux capture-pane -t {} -S {} -p 2>/dev/null; true",
        quote(&pane),
        quote(&format!("-{SHELL_HISTORY_LINES}")),
    )
}

/// The scrollback of shell terminal `n` of `tmux_name` on `host`, trailing
/// blank lines dropped; `None` when nothing could be read.
pub async fn shell_history(
    exec: &dyn SshExec,
    host: &str,
    tmux_name: &str,
    n: u32,
) -> Result<Option<String>, IpcError> {
    crate::validate::host_alias(host)?;
    crate::validate::tmux_name_addressable(tmux_name)?;
    if n == 0 || n > crate::tmux::MAX_SHELL_TERMINALS {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("no shell terminal {n}"),
        ));
    }
    let out = crate::ssh::run_shell_bounded(
        exec,
        host,
        &shell_history_script(tmux_name, n),
        CONNECT,
        CAPTURE_WALL_CLOCK,
    )
    .await?;
    let text = String::from_utf8_lossy(&out.stdout);
    let text = text.trim_end();
    Ok((!text.trim().is_empty()).then(|| text.to_string()))
}

// --- fleet's model: claude -p on the session's host --------------------------

/// The tag the script prints before the model's output.
pub const HELP_TAG: &str = "fleet-help=";
const WALL_CLOCK: Duration = Duration::from_secs(90);
const HOST_TIMEOUT_SECS: u64 = 80;
const OUTPUT_CAP_BYTES: usize = 65_536;

/// PURE: the script: the context (as base64) into a temp file, then one
/// isolated `claude -p` reading it on stdin. Every value is validated and
/// quoted; the context is never an argument of `claude`.
pub fn help_script(
    model: &str,
    profile: Option<&str>,
    instruction: &str,
    context: &str,
) -> Result<String, String> {
    if !settings::SUMMARY_MODELS.contains(&model) {
        return Err(format!("refusing help model {model:?}"));
    }
    let env = match profile {
        Some(p) => {
            crate::validate::claude_profile(p).map_err(|e| e.message)?;
            format!(
                "export CLAUDE_CONFIG_DIR=\"$HOME/.claude-profiles/\"{}; ",
                quote(p)
            )
        }
        None => String::new(),
    };
    let b64 =
        base64::engine::general_purpose::STANDARD.encode(tail_bytes(context, CONTEXT_MAX_BYTES));
    let claude = format!(
        "claude -p --model {} --output-format json {} {} <\"$d\"",
        quote(model),
        claude_print::isolation_flags(),
        quote(instruction),
    );
    let t = HELP_TAG;
    Ok(format!(
        "{noclaude} \
         d=$(mktemp); trap 'rm -f \"$d\"' EXIT; \
         printf %s {b64} | {{ base64 -d 2>/dev/null || base64 -D; }} >\"$d\"; \
         {env}{run}",
        noclaude = claude_print::noclaude_check(t),
        b64 = quote(&b64),
        run = claude_print::run_capped(t, &claude, HOST_TIMEOUT_SECS, OUTPUT_CAP_BYTES, false),
    ))
}

/// `claude -p` on a host, under a credential profile: fleet's [`HelpModel`].
pub struct ClaudeOnHost<'a> {
    pub exec: &'a dyn SshExec,
    pub host: String,
    pub profile: Option<String>,
    /// One of [`settings::SUMMARY_MODELS`].
    pub model: String,
}

#[async_trait::async_trait]
impl HelpModel for ClaudeOnHost<'_> {
    fn model(&self) -> String {
        self.model.clone()
    }

    fn host(&self) -> String {
        self.host.clone()
    }

    async fn complete(&self, instruction: &str, context: &str) -> Result<Completion, IpcError> {
        crate::validate::host_alias(&self.host)?;
        let profile = self.profile.as_deref();
        let script = help_script(&self.model, profile, instruction, context)
            .map_err(|e| IpcError::new(codes::E_INVALID, e))?;
        let run =
            crate::ssh::run_shell_bounded(self.exec, &self.host, &script, CONNECT, WALL_CLOCK)
                .await?;
        let stdout = String::from_utf8_lossy(&run.stdout);
        let text = match claude_print::parse_tagged(&stdout, HELP_TAG, &["noclaude", "run"]) {
            Some(("noclaude", _)) => {
                return Err(IpcError::new(
                    codes::E_CLAUDE_CLI,
                    format!("claude is not on {}'s login PATH", self.host),
                ))
            }
            Some((_, t)) if !t.is_empty() => t,
            Some(_) if run.status.code() == Some(124) => {
                return Err(IpcError::new(
                    codes::E_TIMEOUT,
                    format!("the help took longer than {HOST_TIMEOUT_SECS}s"),
                ))
            }
            _ if claude_print::run_signed_out(None, "", &run.stderr) => {
                return Err(claude_print::signed_out_error(&self.host, profile))
            }
            _ => {
                return Err(IpcError::new(
                    codes::E_CLAUDE_CLI,
                    format!(
                        "the help run failed: {}",
                        crate::service::work::summary::last_error_line(&run.stderr)
                    ),
                ))
            }
        };
        match claude_print::parse_envelope(&text) {
            Some(env) => {
                if claude_print::run_signed_out(Some(&env), &env.result, &run.stderr) {
                    return Err(claude_print::signed_out_error(&self.host, profile));
                }
                if env.is_error || env.result.trim().is_empty() {
                    return Err(IpcError::new(
                        codes::E_CLAUDE_CLI,
                        "the help run failed: claude answered with an error",
                    ));
                }
                Ok(Completion {
                    text: env.result.clone(),
                    usage: Some(env),
                })
            }
            None if claude_print::run_signed_out(None, &text, &run.stderr) => {
                Err(claude_print::signed_out_error(&self.host, profile))
            }
            None => Ok(Completion { text, usage: None }),
        }
    }
}

/// Book a run's cost (`aux_usage`, origin `context_help`).
pub fn book(
    store: &std::sync::Mutex<crate::store::Store>,
    answer: &HelpAnswer,
    usage: Option<&claude_print::Envelope>,
) {
    let row = crate::store::NewAuxUsage {
        origin: crate::store::AUX_ORIGIN_CONTEXT_HELP,
        host_alias: answer.host_alias.clone(),
        model: answer.model.clone(),
        mission_id: None,
        org_id: None,
        claude_session_id: None,
        input_tokens: usage.and_then(|u| u.input_tokens),
        output_tokens: usage.and_then(|u| u.output_tokens),
        cost_micros: usage.and_then(|u| u.cost_microusd).unwrap_or(0),
        at: answer.at,
    };
    if let Err(e) = crate::ipc_error::lock(store).and_then(|s| s.insert_aux_usage(&row)) {
        tracing::warn!(error = %e.message, "[context_help] cost not booked");
    }
}

#[cfg(test)]
mod tests;
