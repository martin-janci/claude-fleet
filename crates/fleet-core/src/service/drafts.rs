//! LLM drafts in the workspace (Orbit Fleet 5.12): a commit message written
//! from a session's staged diff, for Files › Changed. The other draft of the
//! step, "What changed" on Resume, is the past-work summary
//! (`work::summary`), shown there as a draft; a start's brief drafted from
//! its ticket (6.10) is [`crate::service::work::brief_draft`].
//!
//! * **On the session's own host and account.** The run is one `claude -p`
//!   on the session's host, under its credential profile when it has one
//!   (`CLAUDE_CONFIG_DIR`), so the diff never leaves the host and the
//!   session's account pays.
//! * **A run that cannot act.** The shared
//!   [`claude_print::isolation_flags`]: no tools, no MCP server, no hooks,
//!   no transcript. The staged diff (stat first, capped at
//!   [`DIFF_CAP_BYTES`]) goes in on stdin; the prompt is fixed.
//! * **A draft, never a commit.** The text comes back for a person to edit;
//!   nothing is committed and nothing is stored but the run's cost
//!   (`aux_usage`, origin `commit_message`).

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::claude_print;
use crate::service::settings;
use crate::shell::quote;
use crate::ssh::SshExec;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Duration;

/// The tag the script prints before its answer.
pub const DRAFT_TAG: &str = "fleet-draft=";

/// The most of the staged diff the model reads.
pub const DIFF_CAP_BYTES: usize = 60_000;

/// The longest message kept, in characters.
pub const MESSAGE_MAX_CHARS: usize = 2_000;

const CONNECT: Duration = Duration::from_secs(10);
const WALL_CLOCK: Duration = Duration::from_secs(120);
const HOST_TIMEOUT_SECS: u64 = 110;
const OUTPUT_CAP_BYTES: usize = 65_536;

/// The fixed prompt. Nothing from the caller goes into it.
pub const COMMIT_PROMPT: &str = "Write a git commit message for the staged changes given on stdin \
(a diffstat, then the diff, possibly cut). First line: an imperative summary of at most 72 \
characters. Then, only if it helps a reviewer, a blank line and at most 6 short lines on what \
changed and why. Plain text: no code fences, no quotes, no preamble. Do not use tools.";

/// What `draft_commit_message` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitDraft {
    pub message: String,
    pub model: String,
    pub host_alias: String,
    /// How many staged files the draft was written from.
    pub files: u32,
}

/// The session the draft runs for, read under one short lock.
struct Plan {
    host: String,
    tmux_name: String,
    profile: Option<String>,
    model: String,
    org_id: Option<i64>,
    claude_session_id: Option<String>,
}

fn plan(s: &Store, session_id: i64) -> Result<Plan, IpcError> {
    let row = s.get_session_by_id(session_id)?.ok_or_else(|| {
        IpcError::new(codes::E_NOTFOUND, format!("session {session_id} not found"))
    })?;
    crate::validate::host_alias(&row.host_alias)?;
    crate::validate::tmux_name_addressable(&row.tmux_name)?;
    let (_, _, profile) = s.session_launch(session_id)?;
    Ok(Plan {
        profile: profile.filter(|p| crate::validate::claude_profile(p).is_ok()),
        model: settings::get_string_for(s, settings::WORK_SUMMARY_MODEL, row.org_id),
        org_id: row.org_id,
        claude_session_id: row.claude_session_id.clone(),
        host: row.host_alias,
        tmux_name: row.tmux_name,
    })
}

/// PURE: the body run in the session's worktree root (`$root`, from
/// [`crate::service::repo::repo_script`]). Every value is validated and
/// quoted: the model as one of [`settings::SUMMARY_MODELS`], the profile by
/// `validate::claude_profile`.
pub fn commit_body(model: &str, profile: Option<&str>) -> Result<String, String> {
    if !settings::SUMMARY_MODELS.contains(&model) {
        return Err(format!("refusing draft model {model:?}"));
    }
    let env = match profile {
        Some(p) => {
            crate::validate::claude_profile(p).map_err(|e| e.message)?;
            format!(
                "export CLAUDE_CONFIG_DIR=\"$HOME/.claude-profiles/\"{}; {}",
                quote(p),
                crate::tmux::PROFILE_API_KEY
            )
        }
        None => String::new(),
    };
    let claude = format!(
        "claude -p --model {} --output-format json {} {} <\"$d\"",
        quote(model),
        claude_print::isolation_flags(),
        quote(COMMIT_PROMPT),
    );
    let t = DRAFT_TAG;
    Ok(format!(
        "cd \"$root\"; \
         n=$(git diff --cached --name-only | wc -l | tr -d ' '); \
         if [ \"$n\" = 0 ]; then echo {t}empty; exit 0; fi; \
         {noclaude} \
         d=$(mktemp); trap 'rm -f \"$d\"' EXIT; \
         {{ git diff --cached --stat; echo; git diff --cached | head -c {DIFF_CAP_BYTES}; }} >\"$d\" || true; \
         echo {t}files=$n; \
         {env}{run}",
        noclaude = claude_print::noclaude_check(t),
        run = claude_print::run_capped(t, &claude, HOST_TIMEOUT_SECS, OUTPUT_CAP_BYTES, false),
    ))
}

/// PURE: the number of files the script said it read (`<tag>files=<n>`).
fn files_of(stdout: &str) -> u32 {
    stdout
        .lines()
        .find_map(|l| l.trim().strip_prefix(DRAFT_TAG)?.strip_prefix("files="))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0)
}

/// PURE: the message as a person sees it: redacted, outer code fences and
/// blank edges dropped, cut at [`MESSAGE_MAX_CHARS`].
pub fn clean_message(text: &str) -> String {
    let t = text.trim();
    let t = t
        .strip_prefix("```")
        .map(|r| r.split_once('\n').map_or("", |(_, b)| b))
        .and_then(|r| r.trim_end().strip_suffix("```"))
        .unwrap_or(t);
    let redacted = crate::logging::redact(t.trim());
    redacted.chars().take(MESSAGE_MAX_CHARS).collect()
}

fn book(store: &Mutex<Store>, p: &Plan, usage: Option<&claude_print::Envelope>) {
    let row = crate::store::NewAuxUsage {
        origin: crate::store::AUX_ORIGIN_COMMIT_MESSAGE,
        host_alias: p.host.clone(),
        model: p.model.clone(),
        mission_id: None,
        org_id: p.org_id,
        claude_session_id: p.claude_session_id.clone(),
        input_tokens: usage.and_then(|u| u.input_tokens),
        output_tokens: usage.and_then(|u| u.output_tokens),
        cost_micros: usage.and_then(|u| u.cost_microusd).unwrap_or(0),
        at: crate::store::now_unix(),
    };
    if let Err(e) = lock(store).and_then(|s| s.insert_aux_usage(&row)) {
        tracing::warn!(error = %e.message, "[drafts] cost not booked");
    }
}

/// Draft a commit message from `session_id`'s staged changes. Nothing
/// staged is `E_INVALID_STATE`; the run itself is off the store lock.
pub async fn draft_commit_message(
    store: &Mutex<Store>,
    exec: &dyn SshExec,
    session_id: i64,
) -> Result<CommitDraft, IpcError> {
    let p = {
        let s = lock(store)?;
        plan(&s, session_id)?
    };
    let body = commit_body(&p.model, p.profile.as_deref())
        .map_err(|e| IpcError::new(codes::E_INVALID, e))?;
    let script = crate::service::repo::repo_script(&p.tmux_name, &body);
    let out = crate::ssh::run_shell_bounded(exec, &p.host, &script, CONNECT, WALL_CLOCK).await?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if String::from_utf8_lossy(&out.stderr).contains(crate::service::repo::NO_WORKTREE_SENTINEL) {
        return Err(crate::service::repo::repo_err(&out));
    }
    let files = files_of(&stdout);
    let text = match claude_print::parse_tagged(&stdout, DRAFT_TAG, &["empty", "noclaude", "run"]) {
        Some(("empty", _)) => {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                "nothing is staged to write a message for",
            ))
        }
        Some(("noclaude", _)) => {
            return Err(IpcError::new(
                codes::E_CLAUDE_CLI,
                format!("claude is not on {}'s login PATH", p.host),
            ))
        }
        Some((_, t)) if !t.is_empty() => t,
        Some(_) if out.status.code() == Some(124) => {
            return Err(IpcError::new(
                codes::E_TIMEOUT,
                format!("the draft took longer than {HOST_TIMEOUT_SECS}s"),
            ))
        }
        _ if claude_print::run_signed_out(None, "", &out.stderr) => {
            return Err(claude_print::signed_out_error(
                &p.host,
                p.profile.as_deref(),
            ))
        }
        _ if !out.status.success() && stdout.trim().is_empty() => {
            return Err(crate::service::repo::repo_err(&out))
        }
        _ => {
            return Err(IpcError::new(
                codes::E_CLAUDE_CLI,
                "the draft run failed: claude said nothing",
            ))
        }
    };
    let text = match claude_print::parse_envelope(&text) {
        Some(env) => {
            book(store, &p, Some(&env));
            if claude_print::run_signed_out(Some(&env), &env.result, &out.stderr) {
                return Err(claude_print::signed_out_error(
                    &p.host,
                    p.profile.as_deref(),
                ));
            }
            if env.is_error || env.result.trim().is_empty() {
                return Err(IpcError::new(
                    codes::E_CLAUDE_CLI,
                    "the draft run failed: claude answered with an error",
                ));
            }
            env.result
        }
        None => {
            book(store, &p, None);
            if claude_print::run_signed_out(None, &text, &out.stderr) {
                return Err(claude_print::signed_out_error(
                    &p.host,
                    p.profile.as_deref(),
                ));
            }
            text
        }
    };
    Ok(CommitDraft {
        message: clean_message(&text),
        model: p.model,
        host_alias: p.host,
        files,
    })
}

#[cfg(test)]
mod tests;
