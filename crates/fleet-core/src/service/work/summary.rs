//! Summaries of dead sessions (work graph M13.1, decisions D10 / D24 / D27).
//!
//! A person asks for a Claude-written summary of a past session's last
//! conversation — the dead-session sibling of the agent handover (M9.3),
//! which only a live, idle REPL can write. Fleet runs one print-mode fork of
//! that conversation on the session's own host, under its own account, and
//! keeps the reply as a work journal `summary` row from the `agent`; the
//! handover brief shows the newest one inside its untrusted fence.
//!
//! * **On demand only (D27).** Only `work_link { action: summarize }` runs
//!   one; nothing runs at session end.
//! * **A fork that cannot act.** `claude -p --resume <id> --fork-session
//!   --no-session-persistence`: the original transcript is never written and
//!   the fork leaves none. `--tools ''` disables every built-in tool and
//!   `--strict-mcp-config` (with no `--mcp-config`) loads no MCP server, so
//!   the run can only read the conversation and answer. `--settings
//!   '{"hooks":{}}'` keeps fleet's hooks out, so it cannot journal or
//!   deliver into itself. [`summary_script`] builds exactly this, and a test
//!   pins it.
//! * **In the conversation's own directory.** `--resume` finds a transcript
//!   by the directory it ran in, so the script finds the transcript first
//!   (the M11.2 probe's candidates), reads its recorded `cwd`, and runs
//!   there. A transcript that is gone is `E_NO_TRANSCRIPT`; a directory that
//!   is gone is `E_NOTFOUND` — nothing is recreated for a summary.
//! * **stdout, never a pane.** The reply is read from the command's output
//!   after a tag line, so a pane's echo can never be mistaken for it.
//! * **Untrusted and bounded.** The text may quote anything the session
//!   read: it is redacted ([`crate::logging::redact`]), capped at
//!   [`SUMMARY_MAX_CHARS`], stored as the agent's, and fenced wherever it is
//!   shown.
//! * **One per host at a time**, with a wall clock ([`SUMMARY_WALL_CLOCK`]);
//!   a second request for the same host is `E_EXISTS` while one runs.
//! * **Fenced like resume.** The link must be one the caller's org scope
//!   sees, and a per-host token may summarise only its own host's past work.
//!   A conversation a live session still holds is refused: that is what
//!   `handover` is for.

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::{self, OrgScope};
use crate::service::settings;
use crate::ssh::SshExec;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

/// The tag the script prints before its answer; everything after the last
/// `<tag>run` line is the summary.
pub const SUMMARY_TAG: &str = "fleet-summary=";

/// The most of a summary fleet keeps (and fences), in characters.
pub const SUMMARY_MAX_CHARS: usize = 4_000;

/// The connect budget for the one command.
const SUMMARY_CONNECT: Duration = Duration::from_secs(10);

/// The whole run's wall clock. The host-side `timeout` (when the host has
/// one) stops the model call a little earlier, so its answer still arrives.
pub const SUMMARY_WALL_CLOCK: Duration = Duration::from_secs(180);

/// Seconds the host-side `timeout` gives `claude`.
const HOST_TIMEOUT_SECS: u64 = 170;

/// Bytes of stdout the script lets through.
const OUTPUT_CAP_BYTES: usize = 65_536;

/// The fixed prompt. Nothing from the conversation or the caller goes into
/// it.
pub const SUMMARY_PROMPT: &str = "Summarise this conversation for whoever picks up its work next. \
In at most 25 short lines of plain text, say: the goal; what was done (files, commands, decisions); \
what is left or broken; and anything the next session must not redo. \
Do not use tools. Answer from the conversation alone.";

/// What `work_link { action: summarize }` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryOutcome {
    pub key: String,
    pub link_id: i64,
    pub host_alias: String,
    pub claude_session_id: String,
    pub model: String,
    /// The stored journal row.
    pub journal_id: i64,
    pub at: i64,
    /// The summary, fenced as untrusted (it is Claude's reading of a
    /// transcript that may quote anything).
    pub summary: String,
    /// The reply was longer than [`SUMMARY_MAX_CHARS`] and was cut.
    #[serde(default)]
    pub truncated: bool,
}

/// What the store side decided before anything runs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SummaryPlan {
    key: String,
    link_id: i64,
    host: String,
    claude_session_id: String,
    stored_path: Option<String>,
    model: String,
}

/// PURE: the one command the summary runs on the host. Every value is
/// validated and quoted: the id as a UUID, the model as one of
/// [`settings::SUMMARY_MODELS`], the transcript paths by
/// [`super::resume::transcript_candidates`].
pub fn summary_script(
    stored_path: Option<&str>,
    claude_session_id: &str,
    model: &str,
) -> Result<String, String> {
    use crate::shell::quote;
    if !settings::SUMMARY_MODELS.contains(&model) {
        return Err(format!("refusing summary model {model:?}"));
    }
    let candidates = super::resume::transcript_candidates(stored_path, claude_session_id)?;
    let claude = [
        "claude",
        "-p",
        "--resume",
        &quote(claude_session_id),
        "--fork-session",
        "--no-session-persistence",
        "--model",
        &quote(model),
        "--settings",
        &quote(r#"{"hooks":{}}"#),
        "--tools",
        &quote(""),
        "--strict-mcp-config",
        &quote(SUMMARY_PROMPT),
    ]
    .join(" ");
    let t = SUMMARY_TAG;
    Ok(format!(
        "set -o pipefail; f=''; \
         for c in {cands}; do if [ -f \"$c\" ]; then f=$c; break; fi; done; \
         if [ -z \"$f\" ]; then echo {t}absent; exit 0; fi; \
         d=$(grep -o -m1 '\"cwd\":\"[^\"]*\"' \"$f\" | head -n1 | sed -e 's/^\"cwd\":\"//' -e 's/\"$//'); \
         if [ -z \"$d\" ] || ! cd -- \"$d\" 2>/dev/null; then echo {t}nodir; exit 0; fi; \
         if ! command -v claude >/dev/null 2>&1; then echo {t}noclaude; exit 0; fi; \
         t=''; if command -v timeout >/dev/null 2>&1; then t='timeout {HOST_TIMEOUT_SECS}'; fi; \
         echo {t}run; \
         $t {claude} </dev/null | head -c {OUTPUT_CAP_BYTES}",
        cands = candidates.join(" "),
    ))
}

/// What the script's output says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptAnswer {
    /// The transcript is gone from the host.
    Absent,
    /// The directory the conversation ran in is gone.
    NoDir,
    /// `claude` is not on the host's login PATH.
    NoClaude,
    /// The model ran; the text after the tag (possibly empty).
    Ran(String),
    /// No tag at all: the shell failed before it said anything.
    Nothing,
}

/// PURE: read the script's stdout. The last tag line decides; for `run`,
/// everything after it is the reply.
pub fn parse_script_output(stdout: &str) -> ScriptAnswer {
    let lines: Vec<&str> = stdout.lines().collect();
    let Some(i) = lines
        .iter()
        .rposition(|l| l.trim().starts_with(SUMMARY_TAG))
    else {
        return ScriptAnswer::Nothing;
    };
    match lines[i].trim().trim_start_matches(SUMMARY_TAG) {
        "absent" => ScriptAnswer::Absent,
        "nodir" => ScriptAnswer::NoDir,
        "noclaude" => ScriptAnswer::NoClaude,
        "run" => ScriptAnswer::Ran(lines[i + 1..].join("\n").trim().to_string()),
        _ => ScriptAnswer::Nothing,
    }
}

/// PURE: the stored form of a reply — redacted, trimmed, and cut at
/// [`SUMMARY_MAX_CHARS`] characters. The flag says whether it was cut.
pub fn clean_summary(text: &str) -> (String, bool) {
    let redacted = crate::logging::redact(text.trim());
    let n = redacted.chars().count();
    if n <= SUMMARY_MAX_CHARS {
        return (redacted.into_owned(), false);
    }
    (redacted.chars().take(SUMMARY_MAX_CHARS).collect(), true)
}

/// The hosts with a summary running now.
static RUNNING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Holds a host's slot while its summary runs.
struct HostSlot(String);

impl HostSlot {
    fn take(host: &str) -> Result<Self, IpcError> {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if !running.insert(host.to_string()) {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("a summary is already running on {host}; try again when it ends"),
            ));
        }
        Ok(Self(host.to_string()))
    }
}

impl Drop for HostSlot {
    fn drop(&mut self) {
        RUNNING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// Everything the store says, under one short lock: the ended link, its
/// host and last conversation, the fences.
fn plan(s: &Store, key: &str, link_id: i64, scope: &OrgScope) -> Result<SummaryPlan, IpcError> {
    orgs::require_key(s, scope, key)?;
    let key = crate::store::normalize_work_ref(key)?;
    let mut ended = s.ended_work_links_for_key(&key)?;
    orgs::scope_links(s, scope, &mut ended)?;
    let not_found = || {
        IpcError::new(
            codes::E_NOTFOUND,
            format!("{key} has no ended work link {link_id}"),
        )
    };
    let link = ended
        .into_iter()
        .find(|l| l.id == link_id)
        .ok_or_else(not_found)?;
    let host = link
        .snap_host
        .clone()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "that past session recorded no host"))?;
    // A per-host token sees only its own host's past work; another host's
    // link reads exactly like one that does not exist.
    if scope.host().is_some_and(|h| h != host) {
        return Err(not_found());
    }
    if !link.resumable {
        return Err(IpcError::new(
            codes::E_NO_TRANSCRIPT,
            "that session's transcripts were purged with its project",
        ));
    }
    let ids: Vec<String> = link
        .snap_claude_ids
        .as_deref()
        .and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default();
    let claude_session_id = ids.last().cloned().ok_or_else(|| {
        IpcError::new(
            codes::E_NO_TRANSCRIPT,
            "that past session recorded no conversation",
        )
    })?;
    crate::validate::claude_session_id(&claude_session_id)?;
    if s.session_with_claude_id(&host, &claude_session_id)?
        .is_some()
    {
        return Err(IpcError::new(
            codes::E_INVALID,
            "a live session still holds that conversation; ask it for a handover instead",
        ));
    }
    let stored_path = s.conversation_transcript_path_on_host(&host, &claude_session_id)?;
    Ok(SummaryPlan {
        key,
        link_id,
        host,
        claude_session_id,
        stored_path,
        model: settings::get_string(s, settings::WORK_SUMMARY_MODEL),
    })
}

/// Summarise past work `link_id` of `key` (`work_link { action: summarize }`).
/// The store is locked only to plan and to store; the run itself is off the
/// lock.
pub async fn summarize(
    store: &Mutex<Store>,
    exec: &dyn SshExec,
    key: &str,
    link_id: i64,
    scope: &OrgScope,
) -> Result<SummaryOutcome, IpcError> {
    let p = {
        let s = lock(store)?;
        plan(&s, key, link_id, scope)?
    };
    let script = summary_script(p.stored_path.as_deref(), &p.claude_session_id, &p.model)
        .map_err(|e| IpcError::new(codes::E_INVALID, e))?;
    let _slot = HostSlot::take(&p.host)?;
    let out =
        crate::ssh::run_shell_bounded(exec, &p.host, &script, SUMMARY_CONNECT, SUMMARY_WALL_CLOCK)
            .await?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let text = match parse_script_output(&stdout) {
        ScriptAnswer::Ran(t) if !t.is_empty() => t,
        ScriptAnswer::Absent => {
            return Err(IpcError::new(
                codes::E_NO_TRANSCRIPT,
                format!(
                    "the transcript of that conversation is gone from {}",
                    p.host
                ),
            ))
        }
        ScriptAnswer::NoDir => {
            let host = &p.host;
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!(
                    "the directory that conversation ran in is gone from {host}; resume it instead"
                ),
            ));
        }
        ScriptAnswer::NoClaude => {
            return Err(IpcError::new(
                codes::E_CLAUDE_CLI,
                format!("claude is not on {}'s login PATH", p.host),
            ))
        }
        ScriptAnswer::Ran(_) if out.status.code() == Some(124) => {
            return Err(IpcError::new(
                codes::E_TIMEOUT,
                format!("the summary took longer than {HOST_TIMEOUT_SECS}s"),
            ))
        }
        ScriptAnswer::Ran(_) | ScriptAnswer::Nothing => {
            return Err(IpcError::new(
                codes::E_CLAUDE_CLI,
                format!("the summary run failed: {}", last_error_line(&out.stderr)),
            ))
        }
    };
    let (body, truncated) = clean_summary(&text);
    let meta = serde_json::json!({
        "link_id": p.link_id,
        "host": p.host,
        "model": p.model,
    })
    .to_string();
    let (journal_id, at) = {
        let s = lock(store)?;
        let id = s.replace_summary(&p.claude_session_id, &body, Some(&meta))?;
        (id, crate::store::now_unix())
    };
    Ok(SummaryOutcome {
        summary: crate::mcp::guard::fence_untrusted(
            &body,
            &format!("a Claude-written summary of {}", p.key),
            SUMMARY_MAX_CHARS,
        ),
        key: p.key,
        link_id: p.link_id,
        host_alias: p.host,
        claude_session_id: p.claude_session_id,
        model: p.model,
        journal_id,
        at,
        truncated,
    })
}

/// The last non-empty stderr line, redacted and short — enough to say why
/// without echoing a transcript.
fn last_error_line(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("no output");
    let line = crate::logging::redact(line);
    line.chars().take(200).collect()
}

#[cfg(test)]
mod tests;
