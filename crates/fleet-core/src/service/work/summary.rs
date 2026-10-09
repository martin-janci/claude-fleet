//! Summaries of dead sessions (work graph M13.4c, decisions D10 / D27 / D30).
//!
//! A person asks for a Claude-written summary of a past session's last
//! conversation — the dead-session sibling of the agent handover (M9.3),
//! which only a live, idle REPL can write. Fleet runs one print-mode fork of
//! that conversation on the session's own host, under its own account, and
//! keeps the reply as a work journal `summary` row from the `agent`; the
//! handover brief shows the newest one inside its untrusted fence.
//!
//! * **On demand only (D30).** Only `work_link { action: summarize }` runs
//!   one; nothing runs at session end.
//! * **A fork that cannot act.** `claude -p --resume <id> --fork-session
//!   --no-session-persistence`: the original transcript is never written and
//!   the fork leaves none. `--tools ''` disables every built-in tool and
//!   `--strict-mcp-config` (with no `--mcp-config`) loads no MCP server, so
//!   the run can only read the conversation and answer. `--settings
//!   '{"disableAllHooks":true}'` keeps fleet's hooks out, so it cannot journal or
//!   deliver into itself. These are the shared
//!   [`crate::service::claude_print::isolation_flags`]; [`summary_script`]
//!   builds exactly this, and a test pins it.
//! * **In the conversation's own directory.** `--resume` finds a transcript
//!   by the directory it ran in, so the script finds the transcript first
//!   (the M11.2 probe's candidates), reads its recorded `cwd`, and runs
//!   there. A transcript that is gone is `E_NO_TRANSCRIPT`; a directory that
//!   is gone is `E_NOTFOUND` — nothing is recreated for a summary.
//! * **stdout, never a pane.** The reply is read from the command's output
//!   after the first tag line, so neither a pane's echo nor a line the model
//!   prints can be mistaken for the verdict.
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
use crate::service::orgs::{self};
use crate::service::settings;
use crate::ssh::SshExec;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

/// The tag the script prints before its answer; everything after the first
/// whole `<tag>run` line is the summary.
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
    org_id: Option<i64>,
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
    use crate::service::claude_print;
    use crate::shell::quote;
    if !settings::SUMMARY_MODELS.contains(&model) {
        return Err(format!("refusing summary model {model:?}"));
    }
    let candidates = super::resume::transcript_candidates(stored_path, claude_session_id)?;
    let claude = format!(
        "claude -p --resume {} --fork-session --model {} --output-format json {} {}",
        quote(claude_session_id),
        quote(model),
        claude_print::isolation_flags(),
        quote(SUMMARY_PROMPT),
    );
    let t = SUMMARY_TAG;
    Ok(format!(
        "set -o pipefail; f=''; \
         for c in {cands}; do if [ -f \"$c\" ]; then f=$c; break; fi; done; \
         if [ -z \"$f\" ]; then echo {t}absent; exit 0; fi; \
         d=$(grep -o -m1 '\"cwd\":\"[^\"]*\"' \"$f\" | head -n1 | sed -e 's/^\"cwd\":\"//' -e 's/\"$//'); \
         if [ -z \"$d\" ] || ! cd -- \"$d\" 2>/dev/null; then echo {t}nodir; exit 0; fi; \
         {noclaude} \
         {run}",
        cands = candidates.join(" "),
        noclaude = claude_print::noclaude_check(t),
        run = claude_print::run_capped(t, &claude, HOST_TIMEOUT_SECS, OUTPUT_CAP_BYTES, true),
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

/// PURE: read the script's stdout. The FIRST whole tag line decides; for
/// `run`, everything after it is the reply. The script prints exactly one tag
/// line, before `claude` starts, so a line the model prints (the summarised
/// conversation is untrusted) can never stand in for the verdict.
pub fn parse_script_output(stdout: &str) -> ScriptAnswer {
    match crate::service::claude_print::parse_tagged(
        stdout,
        SUMMARY_TAG,
        &["absent", "nodir", "noclaude", "run"],
    ) {
        Some(("absent", _)) => ScriptAnswer::Absent,
        Some(("nodir", _)) => ScriptAnswer::NoDir,
        Some(("noclaude", _)) => ScriptAnswer::NoClaude,
        Some((_, rest)) => ScriptAnswer::Ran(rest),
        None => ScriptAnswer::Nothing,
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

/// What an ended link says about the conversation a summary would read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PastConversation {
    /// The key in its one spelling ([`crate::store::normalize_work_ref`]).
    pub key: String,
    pub host: String,
    pub claude_session_id: String,
    /// The org the past session was in when it ended (`snap_org_id`).
    pub org_id: Option<i64>,
}

/// What a link this caller may not summarise answers, and the same thing a
/// `link_id` naming no ended link answers: `E_NOTFOUND` on the LINK.
///
/// A function rather than a closure because the owner gate at the MCP layer
/// (multi-user M1, T7) refuses with it too, and the two sentences must be
/// one sentence — a link whose conversation belongs to another person has to
/// read exactly like a link that does not exist.
pub fn no_such_link(key: &str, link_id: i64) -> IpcError {
    IpcError::new(
        codes::E_NOTFOUND,
        format!("{key} has no ended work link {link_id}"),
    )
}

/// PURE of the network: the ended link's host and last conversation, under
/// this scope's org and host fences.
///
/// The half of [`plan`] that runs before anything is spent, exposed because
/// the owner gate needs the conversation id BEFORE the model call
/// (multi-user M1, T7): `summarize` is addressed by a link, so choke point
/// 2's session gate never sees it, and the person it must be checked against
/// is the one T3's `conversation_owners` recorded. [`plan`] calls this, so
/// there is exactly one place a link is resolved.
pub fn planned_conversation(
    s: &Store,
    key: &str,
    link_id: i64,
    reader: &crate::service::view_scope::ViewScope,
) -> Result<PastConversation, IpcError> {
    let scope = &reader.org;
    orgs::require_key(s, scope, key)?;
    let key = crate::store::normalize_work_ref(key)?;
    let mut ended = s.ended_work_links_for_key(&key)?;
    // `scope_links_for`, the ONE ended-link predicate, not the org-only
    // `scope_links` (multi-user M1, T9c). The DATA was always person-gated
    // at the choke point above — `require_conversation_person` runs on the
    // id this function returns — but the REFUSALS below are not data and
    // they are a wire answer all the same: for another person's ended link
    // this function used to reach "that past session recorded no host"
    // (`E_INVALID`), "that session's transcripts were purged with its
    // project" or "that past session recorded no conversation"
    // (`E_NO_TRANSCRIPT`), where a link id naming nothing answers
    // `no_such_link`'s `E_NOTFOUND`. That is a cross-person
    // existence-and-resumability oracle, one call per id, and it breaks the
    // doctrine stated on `no_such_link`: the two must be one sentence.
    orgs::scope_links_for(s, reader, &mut ended)?;
    let not_found = || no_such_link(&key, link_id);
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
    Ok(PastConversation {
        key,
        host,
        claude_session_id,
        org_id: link.org_id,
    })
}

/// Everything the store says, under one short lock: the ended link, its
/// host and last conversation, the fences.
fn plan(
    s: &Store,
    key: &str,
    link_id: i64,
    reader: &crate::service::view_scope::ViewScope,
) -> Result<SummaryPlan, IpcError> {
    let PastConversation {
        key,
        host,
        claude_session_id,
        org_id,
    } = planned_conversation(s, key, link_id, reader)?;
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
        // Org administration phase C: the org the work was done for may
        // pick its own model (its account pays).
        model: settings::get_string_for(s, settings::WORK_SUMMARY_MODEL, org_id),
        org_id,
    })
}

/// Book a summary run in `aux_usage`. A failed write is logged, never the
/// summary's error.
fn book(
    store: &Mutex<Store>,
    p: &SummaryPlan,
    usage: Option<&crate::service::claude_print::Envelope>,
) {
    let row = crate::store::NewAuxUsage {
        origin: crate::store::AUX_ORIGIN_SUMMARY,
        host_alias: p.host.clone(),
        model: p.model.clone(),
        mission_id: None,
        org_id: p.org_id,
        claude_session_id: Some(p.claude_session_id.clone()),
        input_tokens: usage.and_then(|u| u.input_tokens),
        output_tokens: usage.and_then(|u| u.output_tokens),
        cost_micros: usage.and_then(|u| u.cost_microusd).unwrap_or(0),
        at: crate::store::now_unix(),
    };
    if let Err(e) = lock(store).and_then(|s| s.insert_aux_usage(&row)) {
        tracing::warn!(error = %e.message, "[summary] cost not booked");
    }
}

/// Summarise past work `link_id` of `key` (`work_link { action: summarize }`).
/// The store is locked only to plan and to store; the run itself is off the
/// lock.
pub async fn summarize(
    store: &Mutex<Store>,
    exec: &dyn SshExec,
    key: &str,
    link_id: i64,
    reader: &crate::service::view_scope::ViewScope,
) -> Result<SummaryOutcome, IpcError> {
    let p = {
        let s = lock(store)?;
        plan(&s, key, link_id, reader)?
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
    // Redesign 8.2: the run is booked with its cost, whatever it said.
    let text = match crate::service::claude_print::parse_envelope(&text) {
        Some(env) => {
            book(store, &p, Some(&env));
            if env.is_error || env.result.trim().is_empty() {
                return Err(IpcError::new(
                    codes::E_CLAUDE_CLI,
                    "the summary run failed: claude answered with an error",
                ));
            }
            env.result
        }
        // An envelope the output cap cut: its text is JSON, not a summary.
        None if text.trim_start().starts_with("{\"type\"") => {
            book(store, &p, None);
            return Err(IpcError::new(
                codes::E_CLAUDE_CLI,
                "the summary run failed: its answer was longer than the output cap",
            ));
        }
        // No envelope (an older `claude`): the text itself.
        None => {
            book(store, &p, None);
            text
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
pub(crate) fn last_error_line(stderr: &[u8]) -> String {
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
