//! The prompt tuner: a person's draft prompt, rewritten by a model into a
//! clearer one for the agent it goes to, at the session composer (desktop
//! and phone) and in a routine's prompt field.
//!
//! The answer is a proposal and nothing more: [`TuneAnswer::prompt`] is the
//! rewrite and [`TuneAnswer::notes`] a few lines on what changed. The
//! client shows the two side by side, and the person accepts it into the
//! box, edits it there, or discards it; only their own Send sends it.
//!
//! It runs the way context help does ([`super::context_help`]): one
//! isolated `claude -p` ([`ClaudeOnHost`]) on the target's host under its
//! credential profile, on `work.help_model`, the draft on stdin, booked as
//! `context_help`. What the prompt goes to (agent, host, project) is read
//! by fleet from the session row or the routine draft, never taken from
//! the caller's free text. Off until Writing help's `work.prompt_tune` is
//! turned on.
//!
//! **Bounded and untrusted.** The draft, the person's ask and each history
//! entry are capped ([`DRAFT_MAX_CHARS`], [`ASK_MAX_CHARS`],
//! [`super::context_help::HISTORY_MAX_ITEMS`]); a draft too long to send
//! whole is refused, never cut, and only history is cut to keep the context
//! within [`super::context_help::CONTEXT_MAX_BYTES`]. The reply is redacted
//! (sparing values the draft itself holds) and capped at the target's
//! limit ([`PROMPT_MAX_CHARS`], [`NOTES_MAX_CHARS`]). Nothing is stored but
//! the run's cost.

use crate::ipc_error::{codes, IpcError};
use crate::service::context_help::{
    self, ClaudeOnHost, HelpModel, HISTORY_ITEM_MAX_CHARS, HISTORY_MAX_ITEMS,
};
use crate::service::decide::summary_check::tail_bytes;
use crate::service::settings;
use crate::ssh::SshExec;
use serde::{Deserialize, Serialize};

/// What the person asks to have tuned, with the context the caller holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuneRequest {
    /// The draft prompt.
    #[serde(default)]
    pub draft: String,
    /// What they want from the rewrite ("shorter", "add acceptance
    /// criteria"); empty asks for a general tune.
    #[serde(default)]
    pub ask: String,
    /// Their earlier prompts to the same session, oldest first.
    #[serde(default)]
    pub history: Vec<String>,
    /// The commands the composer accepts, one per entry.
    #[serde(default)]
    pub commands: Vec<String>,
}

/// Where the prompt goes, as fleet knows it (never the caller's words).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TuneTarget {
    /// `session` (the composer) or `routine` (a saved prompt that starts a
    /// session on its own).
    pub kind: &'static str,
    /// The agent CLI (`claude`, `codex`).
    pub agent: String,
    pub host_alias: String,
    /// `owner/repo`, when the prompt runs in a project.
    pub project: Option<String>,
    /// The longest prompt the target takes, in characters.
    pub max_chars: usize,
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuneAnswer {
    /// The rewritten prompt, for the box; never sent by fleet.
    pub prompt: String,
    /// A few lines on what changed and why.
    #[serde(default)]
    pub notes: String,
    /// The rewrite is the draft as it was: nothing worth changing.
    #[serde(default)]
    pub unchanged: bool,
    /// The model ran past the target's limit and the rewrite was cut there
    /// (ending in `…`): the client warns before it is used as-is.
    #[serde(default)]
    pub cut: bool,
    /// Who rewrote it (`haiku`), as the Drafted label names it.
    pub model: String,
    /// Where it ran.
    #[serde(default)]
    pub host_alias: String,
    /// When it was drafted, unix seconds.
    pub at: i64,
}

/// The longest draft read, in characters.
pub const DRAFT_MAX_CHARS: usize = 12_000;
/// The longest ask kept, in characters.
pub const ASK_MAX_CHARS: usize = 1_000;
/// The longest rewrite kept, in characters.
pub const PROMPT_MAX_CHARS: usize = 12_000;
/// The longest notes kept, in characters.
pub const NOTES_MAX_CHARS: usize = 1_200;

/// The fixed instruction. Nothing from the caller goes into it.
pub const TUNE_INSTRUCTION: &str = "You improve prompts that a person writes for an autonomous \
coding agent. The text on stdin says where the prompt goes, the person's earlier prompts to the \
same agent (oldest first), the commands the prompt box accepts, the draft, and what they want \
from the rewrite. Rewrite the draft into the prompt the agent will act on best: state the goal \
first, then the context, constraints and what done looks like, when the draft implies them. Keep \
the person's intent, facts, names, paths, ticket keys and language; never invent requirements, \
files or facts the draft and history do not give; keep a leading slash command and its \
arguments as they are; keep it as short as the task allows, and plain text unless the draft \
uses markdown. Reply with exactly one line of JSON and nothing else: {\"prompt\": \"<the \
rewritten prompt>\", \"notes\": \"<at most 4 short lines on what you changed and why>\"}. If \
the draft is already clear, return it unchanged and say so in notes. Do not use tools.";

fn cut(text: &str, max: usize) -> String {
    let mut s: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        s.push('…');
    }
    s
}

/// The context in three parts: where it goes, the history and commands,
/// and the draft with the ask. Only the middle one may be cut.
fn sections(target: &TuneTarget, req: &TuneRequest) -> (String, String, String) {
    let mut head = String::from("## Where it goes\n");
    let what = match target.kind {
        "routine" => {
            "a routine: a saved prompt that starts a new session on a schedule or an \
                      event, with no person there to answer questions"
        }
        _ => "the message box of a running session; the agent has the conversation so far",
    };
    head.push_str(&format!("- {what}\n- agent: {}\n", target.agent));
    head.push_str(&format!("- host: {}\n", target.host_alias));
    if let Some(p) = target.project.as_deref().filter(|p| !p.is_empty()) {
        head.push_str(&format!("- project: {p}\n"));
    }
    head.push_str(&format!(
        "- the prompt may be at most {} characters\n",
        target.max_chars
    ));
    let mut middle = String::new();
    let from = req.history.len().saturating_sub(HISTORY_MAX_ITEMS);
    let kept: Vec<&String> = req.history[from..]
        .iter()
        .filter(|h| !h.trim().is_empty())
        .collect();
    if !kept.is_empty() {
        middle.push_str("\n## Earlier prompts (oldest first)\n");
        for h in kept {
            middle.push_str("- ");
            middle.push_str(&cut(h.trim(), HISTORY_ITEM_MAX_CHARS).replace('\n', "\n  "));
            middle.push('\n');
        }
    }
    let commands: Vec<&String> = req
        .commands
        .iter()
        .filter(|c| !c.trim().is_empty())
        .take(context_help::COMMANDS_MAX_ITEMS)
        .collect();
    if !commands.is_empty() {
        middle.push_str("\n## Commands\n");
        for c in commands {
            middle.push_str(&cut(c.trim(), 300));
            middle.push('\n');
        }
    }
    let mut tail = String::from("\n## Draft\n");
    tail.push_str(&cut(req.draft.trim(), DRAFT_MAX_CHARS));
    tail.push_str("\n\n## What I want from the rewrite\n");
    let ask = req.ask.trim();
    if ask.is_empty() {
        tail.push_str("Make it clearer and more complete for the agent.");
    } else {
        tail.push_str(&cut(ask, ASK_MAX_CHARS));
    }
    tail.push('\n');
    (head, middle, tail)
}

/// PURE: whether the draft is too long to tune whole: over
/// [`DRAFT_MAX_CHARS`], or so many bytes that where it goes, the draft and
/// the ask do not fit in [`context_help::CONTEXT_MAX_BYTES`] together.
/// [`tune`] refuses such a draft rather than rewrite a cut one, since
/// accepting the rewrite replaces the whole box.
pub fn draft_too_long(target: &TuneTarget, req: &TuneRequest) -> bool {
    if req.draft.trim().chars().count() > DRAFT_MAX_CHARS {
        return true;
    }
    let (head, _, tail) = sections(target, req);
    head.len() + tail.len() > context_help::CONTEXT_MAX_BYTES
}

/// PURE: the context the model reads on stdin, at most
/// [`context_help::CONTEXT_MAX_BYTES`]. Where it goes comes first and the
/// draft and the ask last; only the oldest history (then the commands) is
/// cut to fit. A draft too long to fit with them ([`draft_too_long`]) is
/// refused by [`tune`] before this runs.
pub fn context_text(target: &TuneTarget, req: &TuneRequest) -> String {
    let (head, middle, tail) = sections(target, req);
    let budget = context_help::CONTEXT_MAX_BYTES.saturating_sub(head.len() + tail.len());
    let out = format!("{head}{}{tail}", tail_bytes(&middle, budget));
    tail_bytes(&out, context_help::CONTEXT_MAX_BYTES).to_string()
}

/// Opens and closes a stand-in for a value of the draft's own while the
/// rewrite is redacted (Unicode private use; never in a reply we keep).
const KEEP_OPEN: char = '\u{E000}';
const KEEP_CLOSE: char = '\u{E001}';

/// The values in `draft` that [`crate::logging::redact`] would mask: each
/// word, its pieces around `=`, `:`, `?`, `&`, `/` and quotes, and each
/// pair of words (`Bearer <token>`). Longest first.
fn draft_secrets(draft: &str) -> Vec<String> {
    let words: Vec<&str> = draft.split_whitespace().collect();
    let mut cands: Vec<String> = Vec::new();
    for (i, w) in words.iter().enumerate() {
        cands.push((*w).to_string());
        if let Some(next) = words.get(i + 1) {
            cands.push(format!("{w} {next}"));
        }
        let bare = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
        cands.push(bare.to_string());
        cands.extend(
            bare.split([
                '=', ':', '?', '&', '/', '"', '\'', ',', ';', '(', ')', '<', '>',
            ])
            .map(str::to_string),
        );
    }
    let mut keep: Vec<String> = cands
        .into_iter()
        .filter(|c| !c.is_empty() && crate::logging::redact(c) != c.as_str())
        .collect();
    keep.sort_by_key(|c| std::cmp::Reverse(c.len()));
    keep.dedup();
    keep
}

/// [`crate::logging::redact`], except for values the draft itself holds:
/// the person's own checksum or key in their own prompt is theirs, and
/// masking it would hand them back a proposal with `[REDACTED]` in place of
/// what they wrote. Anything else the model added is still masked.
fn redact_keeping_draft(text: &str, draft: &str) -> String {
    let keep = draft_secrets(draft);
    if keep.is_empty() || text.contains(KEEP_OPEN) || text.contains(KEEP_CLOSE) {
        return crate::logging::redact(text).into_owned();
    }
    let mut masked = text.to_string();
    let mut used = Vec::new();
    for value in &keep {
        if masked.contains(value.as_str()) {
            let stand_in = format!("{KEEP_OPEN}{}{KEEP_CLOSE}", used.len());
            masked = masked.replace(value.as_str(), &stand_in);
            used.push((stand_in, value));
        }
    }
    let mut out = crate::logging::redact(&masked).into_owned();
    for (stand_in, value) in used.iter().rev() {
        out = out.replace(stand_in.as_str(), value);
    }
    out
}

/// PURE: the reply as the person sees it: the JSON the instruction asked
/// for, or, when there is none, the whole text as the rewrite. A code fence
/// around the rewrite is stripped; control characters other than newline
/// and tab are refused. Redacted (except for values `draft` itself holds)
/// and capped at `max_chars` (at most [`PROMPT_MAX_CHARS`]), saying so in
/// the notes and the third value when it was cut; `None` when no rewrite is
/// left.
pub fn parse_reply(reply: &str, draft: &str, max_chars: usize) -> Option<(String, String, bool)> {
    let reply = reply.trim();
    let json = reply
        .find('{')
        .zip(reply.rfind('}'))
        .filter(|(a, b)| a < b)
        .and_then(|(a, b)| serde_json::from_str::<serde_json::Value>(&reply[a..=b]).ok());
    let (prompt, notes) = match json.as_ref().and_then(|v| v.as_object()) {
        Some(o) => (
            o.get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            o.get("notes")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        ),
        None => (reply.to_string(), String::new()),
    };
    let mut p = prompt.trim();
    if let Some(rest) = p.strip_prefix("```") {
        let rest = rest.split_once('\n').map_or("", |(_, body)| body);
        p = rest.trim_end().strip_suffix("```").unwrap_or(rest).trim();
    }
    if p.is_empty() || p.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return None;
    }
    let max = max_chars.clamp(1, PROMPT_MAX_CHARS);
    let draft = draft.trim();
    let redacted = if p == draft {
        p.to_string()
    } else {
        redact_keeping_draft(p, draft)
    };
    let over = redacted.chars().count() > max;
    let prompt = if over {
        let mut s: String = redacted.chars().take(max - 1).collect();
        s.push('…');
        s
    } else {
        redacted
    };
    let mut notes = redact_keeping_draft(notes.trim(), draft)
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(NOTES_MAX_CHARS)
        .collect::<String>();
    if over {
        if !notes.is_empty() {
            notes.push('\n');
        }
        notes.push_str(&format!(
            "The rewrite ran past the {max}-character limit and was cut there."
        ));
    }
    Some((prompt, notes, over))
}

/// Ask `model` to tune `req` for `target`.
pub async fn tune(
    model: &dyn HelpModel,
    target: &TuneTarget,
    req: &TuneRequest,
) -> Result<(TuneAnswer, Option<crate::service::claude_print::Envelope>), IpcError> {
    if req.draft.trim().is_empty() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "write a draft first: the tuner rewrites what is in the box",
        ));
    }
    if draft_too_long(target, req) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "the draft is too long to tune (at most {DRAFT_MAX_CHARS} characters, \
                 fewer for non-Latin text): tune a shorter part of it"
            ),
        ));
    }
    let context = context_text(target, req);
    let done = model.complete(TUNE_INSTRUCTION, &context).await?;
    let (prompt, notes, cut) = parse_reply(&done.text, &req.draft, target.max_chars)
        .ok_or_else(|| IpcError::new(codes::E_CLAUDE_CLI, "the tuner run proposed no prompt"))?;
    let unchanged = prompt.trim() == req.draft.trim();
    Ok((
        TuneAnswer {
            prompt,
            notes,
            unchanged,
            cut,
            model: model.model(),
            host_alias: model.host(),
            at: crate::store::now_unix(),
        },
        done.usage,
    ))
}

/// Book a run's cost as context help (`aux_usage`, origin `context_help`):
/// the same writing help at the same prompt line, on the same model.
fn book(
    store: &std::sync::Mutex<crate::store::Store>,
    answer: &TuneAnswer,
    usage: Option<&crate::service::claude_print::Envelope>,
    org_id: Option<i64>,
) {
    let as_help = context_help::HelpAnswer {
        answer: String::new(),
        command: None,
        model: answer.model.clone(),
        host_alias: answer.host_alias.clone(),
        history_items: 0,
        at: answer.at,
    };
    context_help::book(store, &as_help, usage, org_id);
}

fn project_name(s: &crate::store::Store, id: Option<i64>) -> Option<String> {
    let p = s.get_project(id?).ok().flatten()?;
    Some(format!("{}/{}", p.owner, p.repo))
}

/// Tune a draft at a session's composer, for whoever may answer it (the
/// row's owner, or an `answer` / `drive` grant: the `session_prompt_tune`
/// hub tool checks that). Runs on the session's host under the profile it
/// was launched with, on `work.help_model`, booked to the session's org.
/// Refused while Writing help's `work.prompt_tune` is off.
pub async fn tune_for_session(
    store: &std::sync::Mutex<crate::store::Store>,
    exec: &dyn SshExec,
    row: &crate::store::SessionRow,
    req: &TuneRequest,
) -> Result<TuneAnswer, IpcError> {
    let (profile, model, project) = {
        let s = crate::ipc_error::lock(store)?;
        settings::require_writing_help(&s, settings::WORK_PROMPT_TUNE)?;
        let (_, _, profile) = s.session_launch(row.id)?;
        (
            profile,
            settings::get_string_for(&s, settings::WORK_HELP_MODEL, row.org_id),
            project_name(&s, row.project_id),
        )
    };
    let target = TuneTarget {
        kind: "session",
        agent: row.agent.clone(),
        host_alias: row.host_alias.clone(),
        project,
        max_chars: DRAFT_MAX_CHARS,
    };
    let model = ClaudeOnHost {
        exec,
        host: row.host_alias.clone(),
        profile: profile.filter(|p| !p.is_empty()),
        model,
        what: "the tune",
    };
    let (answer, usage) = tune(&model, &target, req).await?;
    book(store, &answer, usage.as_ref(), row.org_id);
    Ok(answer)
}

/// Tune a routine's prompt while it is being written (`routines { action:
/// tune_prompt, routine }`): on the routine's host under its profile, for
/// a caller who sees that host's org. The routine need not be saved yet.
/// Refused while `work.prompt_tune` is off.
pub async fn tune_for_routine(
    store: &std::sync::Mutex<crate::store::Store>,
    exec: &dyn SshExec,
    scope: &crate::service::view_scope::ViewScope,
    input: &crate::service::routines::RoutineInput,
    ask: &str,
) -> Result<TuneAnswer, IpcError> {
    let alias = input.host_alias.as_str();
    crate::validate::host_alias(alias)?;
    let (model, org, project) = {
        let s = crate::ipc_error::lock(store)?;
        settings::require_writing_help(&s, settings::WORK_PROMPT_TUNE)?;
        let org = s.host_org(alias)?;
        if s.get_host_row(alias)?.is_none() || !scope.org.sees_org(org) {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("no host {alias} to tune the prompt on"),
            ));
        }
        (
            settings::get_string_for(&s, settings::WORK_HELP_MODEL, org),
            org,
            project_name(&s, Some(input.project_id).filter(|id| *id > 0)),
        )
    };
    let target = TuneTarget {
        kind: "routine",
        agent: "claude".into(),
        host_alias: alias.to_string(),
        project,
        max_chars: crate::service::work::handover::BRIEF_MAX_CHARS,
    };
    let model = ClaudeOnHost {
        exec,
        host: alias.to_string(),
        profile: input.profile.clone().filter(|p| !p.is_empty()),
        model,
        what: "the tune",
    };
    let req = TuneRequest {
        draft: input.prompt.clone(),
        ask: ask.to_string(),
        ..Default::default()
    };
    let (answer, usage) = tune(&model, &target, &req).await?;
    book(store, &answer, usage.as_ref(), org);
    Ok(answer)
}

#[cfg(test)]
mod tests;
