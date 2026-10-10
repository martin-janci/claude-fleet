//! "Since 13:20" (Orbit Fleet redesign step 11.11): a short summary of what
//! a session did since a time, for whoever watches it — a person a share
//! reaches at the Read level, or its owner (the same summary tops Details ›
//! Facts).
//!
//! * **On demand only.** A person presses Summarise; opening a session runs
//!   nothing.
//! * **Only with the org's consent.** The summary is the owner's
//!   conversation retold to someone else, and its check sends the excerpt
//!   to Jev, so a session whose org has not consented (`orgs.jev_allowed`;
//!   `decide.jev.unassigned` for a session with no org) gets none:
//!   `E_FORBIDDEN`, and nothing runs.
//! * **On the session's own host and account.** The excerpt — the turns
//!   since the time, read from the transcript the Conversation tab reads —
//!   goes to one `claude -p` on the session's host, under its credential
//!   profile, with the shared [`claude_print::isolation_flags`] (no tools,
//!   no MCP, no hooks, no transcript). The run is booked in `aux_usage`
//!   ([`crate::store::AUX_ORIGIN_WATCH_SUMMARY`]).
//! * **Checked before it shows (J9).** [`summary_check::check`] asks Jev
//!   whether the transcript supports the summary; in `assist` a summary it
//!   finds unsupported, or cannot check, is not shown ([`WatchSummary::text`]
//!   is `None` and [`WatchSummary::check`] says why).
//! * **Untrusted and bounded.** The excerpt is the newest
//!   [`EXCERPT_MAX_BYTES`]; the summary is redacted and capped at
//!   [`SUMMARY_MAX_CHARS`]; nothing is stored but the run's cost and the
//!   check's decision run.

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::claude_print;
use crate::service::decide::summary_check::{self, Check};
use crate::service::decide::DecideCtx;
use crate::service::settings;
use crate::service::transcript::{ConvItem, ConvTurn};
use crate::shell::quote;
use crate::ssh::SshExec;
use crate::store::{SessionRow, Store};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The tag the script prints before its answer.
pub const SINCE_TAG: &str = "fleet-since=";

/// The newest bytes of the excerpt the model reads. The excerpt rides the
/// script as base64, so the remote command stays far below the 128 KiB a
/// single argument may be.
pub const EXCERPT_MAX_BYTES: usize = 40_000;

/// The longest summary kept, in characters.
pub const SUMMARY_MAX_CHARS: usize = 1_500;

/// Turns read from the transcript's tail, at most.
pub const READ_TURNS: usize = 40;

/// Characters of one turn's text kept in the excerpt.
const TURN_TEXT_MAX_CHARS: usize = 4_000;

const CONNECT: Duration = Duration::from_secs(10);
const WALL_CLOCK: Duration = Duration::from_secs(120);
const HOST_TIMEOUT_SECS: u64 = 110;
const OUTPUT_CAP_BYTES: usize = 65_536;

/// The fixed prompt. Nothing from the caller goes into it.
pub const SINCE_PROMPT: &str = "The text on stdin is an excerpt of a coding agent's session: the \
person's prompts, the agent's replies and one line per tool call. Summarise it for a teammate \
who is watching the session and missed it. At most 6 short lines: what was asked, what was \
done, and what is still open or waiting on someone. Use only the excerpt; do not invent. Plain \
text: no headings, no code fences, no preamble. Do not use tools.";

/// What `session_summary_since` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchSummary {
    /// The summary; `None` when nothing happened since, or when the check
    /// hid it (see [`Self::check`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// What J9 said. [`Check::Off`] also when nothing ran.
    pub check: Check,
    /// The start of the window, unix seconds.
    pub since: i64,
    /// Turns in the window.
    pub turns: u32,
    /// The window reaches back past the newest [`READ_TURNS`] turns, so
    /// [`Self::turns`] and the summary cover only those (review r01).
    /// Additive: absent (false) on an older hub, and an older client ignores
    /// it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub turns_capped: bool,
    /// Where it ran; empty when nothing ran.
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub host_alias: String,
    /// When it was drafted, unix seconds.
    pub at: i64,
}

/// The session the summary runs for, read under one short lock.
#[derive(Debug, Clone)]
pub struct Plan {
    pub session_id: i64,
    pub host: String,
    pub profile: Option<String>,
    pub model: String,
    pub org_id: Option<i64>,
    pub claude_session_id: Option<String>,
}

/// Whether `org_id` consented: the org's own consent, or
/// `decide.jev.unassigned` for a session with no org.
fn consented(s: &Store, org_id: Option<i64>) -> Result<bool, IpcError> {
    Ok(match org_id {
        Some(id) => s.org_jev_allowed(id)?,
        None => settings::get_bool(s, settings::DECIDE_JEV_UNASSIGNED),
    })
}

/// The plan for `row`, or why there is none (no consent).
pub fn plan(s: &Store, row: &SessionRow) -> Result<Plan, IpcError> {
    if !consented(s, row.org_id)? {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "this session's organisation has not consented to summaries",
        ));
    }
    crate::validate::host_alias(&row.host_alias)?;
    let (_, _, profile) = s.session_launch(row.id)?;
    Ok(Plan {
        session_id: row.id,
        host: row.host_alias.clone(),
        profile: profile.filter(|p| crate::validate::claude_profile(p).is_ok()),
        model: settings::get_string_for(s, settings::WORK_SUMMARY_MODEL, row.org_id),
        org_id: row.org_id,
        claude_session_id: row.claude_session_id.clone(),
    })
}

/// PURE: a turn's time: its last reply, else its prompt.
fn turn_time(t: &ConvTurn) -> Option<i64> {
    t.ended_at
        .as_deref()
        .or(t.at.as_deref())
        .and_then(crate::service::account_usage::parse_rfc3339)
}

fn cut(text: &str, max: usize) -> String {
    let mut s: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        s.push('…');
    }
    s
}

/// PURE: the turns that ended at or after `since`, as plain text, and how
/// many there were. A turn with no time is left out.
pub fn excerpt_since(turns: &[ConvTurn], since: i64) -> (String, u32) {
    let mut out = Vec::new();
    let mut n = 0;
    for t in turns
        .iter()
        .filter(|t| turn_time(t).is_some_and(|at| at >= since))
    {
        n += 1;
        let mut lines = Vec::new();
        let when = t.at.as_deref().unwrap_or("");
        if let Some(p) = t.prompt.as_deref().filter(|p| !p.trim().is_empty()) {
            lines.push(format!(
                "[{when}] Person: {}",
                cut(p.trim(), TURN_TEXT_MAX_CHARS)
            ));
        }
        for item in &t.items {
            match item {
                ConvItem::Text { text } if !text.trim().is_empty() => {
                    lines.push(format!("Agent: {}", cut(text.trim(), TURN_TEXT_MAX_CHARS)));
                }
                ConvItem::Tool { summary, error, .. } => {
                    let failed = if *error { " (failed)" } else { "" };
                    lines.push(format!("  tool: {summary}{failed}"));
                }
                ConvItem::Subagent { description, .. } => {
                    lines.push(format!(
                        "  subagent: {}",
                        description.as_deref().unwrap_or("a task")
                    ));
                }
                _ => {}
            }
        }
        out.push(lines.join("\n"));
    }
    (out.join("\n\n"), n)
}

/// PURE: the script: the excerpt (its newest [`EXCERPT_MAX_BYTES`], as
/// base64) into a temp file, then one isolated `claude -p` reading it on
/// stdin. Every value is validated and quoted.
pub fn since_script(model: &str, profile: Option<&str>, excerpt: &str) -> Result<String, String> {
    if !settings::SUMMARY_MODELS.contains(&model) {
        return Err(format!("refusing summary model {model:?}"));
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
    let b64 = base64::engine::general_purpose::STANDARD
        .encode(summary_check::tail_bytes(excerpt, EXCERPT_MAX_BYTES));
    let claude = format!(
        "claude -p --model {} --output-format json {} {} <\"$d\"",
        quote(model),
        claude_print::isolation_flags(),
        quote(SINCE_PROMPT),
    );
    let t = SINCE_TAG;
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

/// PURE: the summary as a person sees it: redacted, trimmed, capped.
pub fn clean_summary(text: &str) -> String {
    let redacted = crate::logging::redact(text.trim());
    redacted.chars().take(SUMMARY_MAX_CHARS).collect()
}

fn book(store: &Mutex<Store>, p: &Plan, usage: Option<&claude_print::Envelope>) {
    let row = crate::store::NewAuxUsage {
        origin: crate::store::AUX_ORIGIN_WATCH_SUMMARY,
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
        tracing::warn!(error = %e.message, "[watch_summary] cost not booked");
    }
}

/// Summarise `excerpt` (`turns` turns since `since`) on the plan's host,
/// then check it. Nothing in the window runs nothing.
pub async fn summarize_excerpt(
    store: &Mutex<Store>,
    exec: &dyn SshExec,
    decide: &DecideCtx,
    p: &Plan,
    since: i64,
    excerpt: &str,
    turns: u32,
) -> Result<WatchSummary, IpcError> {
    let mut out = WatchSummary {
        text: None,
        check: Check::Off,
        since,
        turns,
        turns_capped: false,
        model: String::new(),
        host_alias: String::new(),
        at: crate::store::now_unix(),
    };
    if turns == 0 || excerpt.trim().is_empty() {
        return Ok(out);
    }
    let script = since_script(&p.model, p.profile.as_deref(), excerpt)
        .map_err(|e| IpcError::new(codes::E_INVALID, e))?;
    let run = crate::ssh::run_shell_bounded(exec, &p.host, &script, CONNECT, WALL_CLOCK).await?;
    let stdout = String::from_utf8_lossy(&run.stdout);
    let text = match claude_print::parse_tagged(&stdout, SINCE_TAG, &["noclaude", "run"]) {
        Some(("noclaude", _)) => {
            return Err(IpcError::new(
                codes::E_CLAUDE_CLI,
                format!("claude is not on {}'s login PATH", p.host),
            ))
        }
        Some((_, t)) if !t.is_empty() => t,
        Some(_) if run.status.code() == Some(124) => {
            return Err(IpcError::new(
                codes::E_TIMEOUT,
                format!("the summary took longer than {HOST_TIMEOUT_SECS}s"),
            ))
        }
        _ if claude_print::run_signed_out(None, "", &run.stderr) => {
            return Err(claude_print::signed_out_error(
                &p.host,
                p.profile.as_deref(),
            ))
        }
        _ => {
            return Err(IpcError::new(
                codes::E_CLAUDE_CLI,
                format!(
                    "the summary run failed: {}",
                    crate::service::work::summary::last_error_line(&run.stderr)
                ),
            ))
        }
    };
    let text = match claude_print::parse_envelope(&text) {
        Some(env) => {
            book(store, p, Some(&env));
            if claude_print::run_signed_out(Some(&env), &env.result, &run.stderr) {
                return Err(claude_print::signed_out_error(
                    &p.host,
                    p.profile.as_deref(),
                ));
            }
            if env.is_error || env.result.trim().is_empty() {
                return Err(IpcError::new(
                    codes::E_CLAUDE_CLI,
                    "the summary run failed: claude answered with an error",
                ));
            }
            env.result
        }
        None => {
            book(store, p, None);
            if claude_print::run_signed_out(None, &text, &run.stderr) {
                return Err(claude_print::signed_out_error(
                    &p.host,
                    p.profile.as_deref(),
                ));
            }
            text
        }
    };
    let summary = clean_summary(&text);
    out.check = summary_check::check(decide, p.session_id, p.org_id, &summary, excerpt).await;
    out.text = out.check.shows().then_some(summary);
    out.model = p.model.clone();
    out.host_alias = p.host.clone();
    Ok(out)
}

/// `session_summary_since`: what `row` did since `since`, for someone who
/// may read it (the caller checked the reach). The store is locked only to
/// plan; the reads and the run are off the lock.
pub async fn summarize_since(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<crate::ssh::SshClient>,
    decide: &DecideCtx,
    row: &SessionRow,
    since: i64,
) -> Result<WatchSummary, IpcError> {
    let p = {
        let s = lock(store)?;
        plan(&s, row)?
    };
    let conv = crate::service::transcript::fetch_conversation_for_row(
        store,
        ssh,
        row,
        None,
        READ_TURNS,
        crate::service::transcript::CONV_MAX_CHARS,
        0,
    )
    .await?;
    let (excerpt, turns) = excerpt_since(&conv.turns, since);
    let mut out =
        summarize_excerpt(store, ssh.as_ref(), decide, &p, since, &excerpt, turns).await?;
    out.turns_capped = window_capped(&conv.turns, conv.truncated, since);
    Ok(out)
}

/// PURE: whether the read dropped turns that fall in the window: the read
/// was cut short and its oldest turn is already inside the window.
pub fn window_capped(turns: &[ConvTurn], truncated: bool, since: i64) -> bool {
    truncated
        && turns
            .first()
            .is_some_and(|t| turn_time(t).is_some_and(|at| at >= since))
}

#[cfg(test)]
mod tests;
