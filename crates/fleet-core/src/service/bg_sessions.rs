//! Service functions for Claude CLI background-session operations.

use crate::claude_cli;
use crate::ipc_error::lock;
use crate::ipc_error::{codes, IpcError};
use crate::ssh::SshClient;
use crate::store::Store;
use crate::validate;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

pub use crate::claude_cli::PurgeReport;

// ─── args / result types ─────────────────────────────────────────────────────
//
// Every arg struct validates the host alias (it becomes an `ssh` operand) and
// every value that becomes a `claude` positional / option value (a leading
// `-` would be read as a flag). `claude_cli` re-checks the same rules when it
// builds the script, so DevTools / MCP callers cannot bypass them.

#[derive(Debug, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "NewBgSessionParams")]
pub struct NewBgSessionArgs {
    /// Host to launch on.
    pub host_alias: String,
    /// Display name (also its tmux/agent name).
    pub name: String,
    /// Initial prompt.
    pub prompt: String,
    /// Your session id: becomes the row's parent.
    #[serde(default)]
    pub requester_session_id: Option<i64>,
    /// Start in this project's checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    /// "claude" (default) or "codex" (a Codex session in project_id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// No edit, commit or push.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
    /// Stop after this many seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_after_secs: Option<i64>,
    /// Stop at this estimated spend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_after_usd: Option<f64>,
}

/// Bounds on [`NewBgSessionArgs::stop_after_secs`]: a minute to a week.
pub const STOP_AFTER_SECS_RANGE: std::ops::RangeInclusive<i64> = 60..=7 * 24 * 3600;
/// Upper bound on [`NewBgSessionArgs::stop_after_usd`].
pub const STOP_AFTER_USD_MAX: f64 = 1000.0;

/// Why a Codex background agent needs a project: it runs as a Codex
/// session in that checkout (gap plan G7.3), and a session starts in one.
pub const CODEX_BG_NEEDS_PROJECT: &str =
    "a Codex background agent runs in a project's checkout: pass project_id";
/// What a Codex background agent does not take: `read_only` is Claude's
/// plan mode, and the stop limits are read off `claude agents`.
pub const CODEX_BG_CLAUDE_ONLY: &str =
    "read_only and the stop limits apply to a Claude background agent, not to Codex";

impl NewBgSessionArgs {
    pub fn validate(&self) -> Result<(), IpcError> {
        validate::host_alias(&self.host_alias)?;
        validate::not_option_like("session name", &self.name)?;
        if self.name.chars().any(|c| c.is_control()) {
            return Err(IpcError::new(
                codes::E_INVALID,
                "session name must not contain control characters",
            ));
        }
        // The prompt lands after `--` (see `claude_cli::bg_script`), so a
        // leading `-` is fine — only blank prompts are rejected.
        validate::not_blank("prompt", &self.prompt)?;
        match self.agent.as_deref() {
            None | Some("claude") => {}
            Some("codex") => {
                if self.project_id.is_none() {
                    return Err(IpcError::new(codes::E_INVALID, CODEX_BG_NEEDS_PROJECT));
                }
                if self.read_only || self.stop_after_secs.is_some() || self.stop_after_usd.is_some()
                {
                    return Err(IpcError::new(codes::E_INVALID, CODEX_BG_CLAUDE_ONLY));
                }
            }
            Some(other) => {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!(
                        "unknown agent {:?}: a background agent runs \"claude\" or \"codex\"",
                        other
                    ),
                ))
            }
        }
        if let Some(secs) = self.stop_after_secs {
            if !STOP_AFTER_SECS_RANGE.contains(&secs) {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    "stop_after_secs must be between 60 and 604800",
                ));
            }
        }
        if let Some(usd) = self.stop_after_usd {
            if !(usd.is_finite() && usd > 0.0 && usd <= STOP_AFTER_USD_MAX) {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    "stop_after_usd must be more than 0 and at most 1000",
                ));
            }
        }
        Ok(())
    }

    /// Whether this launch runs Codex rather than `claude --bg`.
    pub fn is_codex(&self) -> bool {
        self.agent.as_deref() == Some("codex")
    }

    /// The limits to record for this launch, or `None` when it has none.
    fn stop_limits(&self, launched_at: i64) -> Option<StopLimits> {
        let stop_at = self.stop_after_secs.map(|s| launched_at + s);
        let stop_cost_micros = self
            .stop_after_usd
            .map(|u| (u * 1_000_000.0).round() as i64);
        (stop_at.is_some() || stop_cost_micros.is_some()).then_some(StopLimits {
            stop_at,
            stop_cost_micros,
        })
    }
}

/// A background agent's "stop after" limits (migration 149).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StopLimits {
    pub stop_at: Option<i64>,
    pub stop_cost_micros: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NewBgSessionResult {
    /// Absent-when-None on the wire (below), so a hub-read result needs the
    /// default. Same for `warning` and `session`.
    #[serde(default)]
    pub claude_session_id: Option<String>,
    /// Populated when `claude --bg` ran but no session id could be parsed from
    /// its output — the session may still be live, but the fleet can't track
    /// it by id (and thus can't `peek` it). Surfaced so the caller can warn.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub warning: Option<String>,
    /// The fleet row (MCP-7): `new_bg_session_tracked` runs a single-host
    /// reconcile right after launch so the `bg:<id>` sentinel exists before
    /// the caller's next tool call. The key is ABSENT from the JSON (not
    /// `null`) when the agent could not be matched yet (it appears on the
    /// next tick) or when the untracked path was used.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session: Option<crate::store::SessionRow>,
}

#[derive(Debug, Deserialize)]
pub struct PurgeProjectArgs {
    /// Every host whose Claude state must go. The fleet row is deleted only
    /// after all of them succeed.
    pub host_aliases: Vec<String>,
    pub project_path: String,
    pub project_id: i64,
}

impl PurgeProjectArgs {
    pub fn validate(&self) -> Result<(), IpcError> {
        if self.host_aliases.is_empty() {
            return Err(IpcError::new(
                codes::E_INVALID,
                "host_aliases must name at least one host",
            ));
        }
        for host in &self.host_aliases {
            validate::host_alias(host)?;
        }
        validate::not_option_like("project_path", &self.project_path)?;
        // A relative path would resolve against the remote $HOME.
        if !self.project_path.starts_with('/') {
            return Err(IpcError::new(
                codes::E_INVALID,
                "project_path must be absolute",
            ));
        }
        if self.project_path.chars().any(|c| c.is_control()) {
            return Err(IpcError::new(
                codes::E_INVALID,
                "project_path must not contain control characters",
            ));
        }
        Ok(())
    }
}

// ─── service functions ───────────────────────────────────────────────────────

/// Launch `claude --bg` for `args` in `cwd` (resolved by the caller from
/// `args.project_id`; see [`new_bg_session_tracked`]).
pub async fn new_bg_session(
    args: NewBgSessionArgs,
    cwd: Option<String>,
    ssh: &Arc<SshClient>,
) -> Result<NewBgSessionResult, IpcError> {
    args.validate()?;
    let opts = claude_cli::BgOptions {
        cwd,
        read_only: args.read_only,
    };
    let claude_session_id =
        claude_cli::claude_bg(ssh, &args.host_alias, &args.name, &args.prompt, &opts).await?;
    Ok(bg_session_result(claude_session_id))
}

/// Warning surfaced when `claude --bg` succeeded but its output didn't yield a
/// parseable session id.
pub const BG_NO_ID_WARNING: &str = "could not parse session id from claude --bg output";

/// Build the `new_bg_session` result, attaching a warning when no session id
/// was parsed. Pure so the warn-on-null decision is unit-testable without the
/// (SSH/local) `claude --bg` exec.
fn bg_session_result(claude_session_id: Option<String>) -> NewBgSessionResult {
    let warning = if claude_session_id.is_none() {
        Some(BG_NO_ID_WARNING.to_string())
    } else {
        None
    };
    NewBgSessionResult {
        claude_session_id,
        warning,
        session: None,
    }
}

/// `new_bg_session` + immediate registration (MCP-7): after `claude --bg`
/// returns its id, reconcile the host once (as `spawn_review` does) so the
/// synthetic `bg:<id>` row exists, then stamp the row with the launch prompt
/// (`last_prompt`, `started_at`, and a prompt-derived friendly name — PROD-4).
/// Every post-launch step is best-effort: the agent is already running, so a
/// reconcile hiccup degrades to `session: None` rather than an error.
/// `owner` is whose the row is (multi-user M1, T5): the person behind the
/// request, resolved by the caller (`mcp::tools::fleet::owner_for` at the MCP
/// edge, the hub's own person on the desktop). `None` leaves the row
/// `unclaimed`, which is what a per-host token's launch gets — a machine owns
/// nothing.
pub async fn new_bg_session_tracked(
    args: NewBgSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    reg: &Arc<crate::cancel::CancellationRegistry>,
    owner: Option<i64>,
) -> Result<NewBgSessionResult, IpcError> {
    if args.is_codex() {
        return new_codex_bg_session(args, store, ssh, reg, owner).await;
    }
    let host_alias = args.host_alias.clone();
    let name = args.name.clone();
    let prompt = args.prompt.clone();
    let requester = args.requester_session_id;
    // Recorded before `claude --bg` runs so the by-name fallback can tell
    // this launch apart from an older agent listed under the same name.
    let launch_started = now_unix();
    // Redesign 8.7: a background session bills the host's own login; say so
    // when that account is past `accounts.pause_at`. A warning, not a
    // refusal: a person or an agent asked for it now.
    let over = {
        let s = lock(store)?;
        crate::service::account_limits::over_limit(&s, &host_alias, None, launch_started)?
    };
    args.validate()?;
    let limits = args.stop_limits(launch_started);
    let cwd = match args.project_id {
        Some(pid) => {
            Some(crate::service::sessions::project_cwd_on_host(store, ssh, &host_alias, pid).await?)
        }
        None => None,
    };
    let mut res = new_bg_session(args, cwd, ssh).await?;
    if res.claude_session_id.is_none() {
        // `claude --bg` output did not carry the id; the agent is listed
        // under the `--name` we launched it with once it registers.
        res.claude_session_id = find_launched_id(ssh, &host_alias, &name, launch_started).await;
        if res.claude_session_id.is_some() {
            res.warning = None;
        }
    }
    if let Some(limits) = limits {
        // Recorded before reconcile so a limit holds even when the row only
        // appears on a later tick. Without an id there is nothing to key it
        // on: say so rather than launch an agent that silently runs on.
        let recorded = match res.claude_session_id.as_deref() {
            Some(id) => lock(store)
                .and_then(|s| record_stop_limits(&s, &host_alias, id, launch_started, limits))
                .map_err(|e| e.message),
            None => Err("the agent's session id is not known yet".to_string()),
        };
        if let Err(why) = recorded {
            let line = format!("stop limits not set ({why}); stop the agent by hand");
            res.warning = Some(match res.warning.take() {
                Some(w) => format!("{w}; {line}"),
                None => line,
            });
        }
    }
    if let Some(over) = over {
        let line = format!("{}; it may stall at the limit", over.reason());
        res.warning = Some(match res.warning.take() {
            Some(w) => format!("{w}; {line}"),
            None => line,
        });
    }
    let Some(ref claude_id) = res.claude_session_id else {
        return Ok(res);
    };
    if let Err(e) = crate::service::sessions::reconcile_one_host(store, ssh, &host_alias).await {
        tracing::warn!(
            host = %host_alias,
            error = %e,
            "[bg] post-launch reconcile failed; the row appears on the next pass"
        );
        return Ok(res);
    }
    res.session = stamp_bg_row(store, claude_id, &prompt, requester, owner);
    Ok(res)
}

/// A Codex background agent (gap plan G7.3): Codex has no `--bg` mode, so
/// it runs as a Codex session in the project's checkout, named `name`, with
/// the prompt queued as its first message (delivered once the pane reads
/// input, as a routine's is). The row is the person's like any session's;
/// `requester_session_id` becomes its parent. No `claude_session_id`:
/// Codex picks its own, which reconcile reads off its rollout.
async fn new_codex_bg_session(
    args: NewBgSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    reg: &Arc<crate::cancel::CancellationRegistry>,
    owner: Option<i64>,
) -> Result<NewBgSessionResult, IpcError> {
    args.validate()?;
    let project_id = args
        .project_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, CODEX_BG_NEEDS_PROJECT))?;
    let over = {
        let s = lock(store)?;
        crate::service::account_limits::over_limit(&s, &args.host_alias, None, now_unix())?
    };
    let session_args = crate::service::sessions::NewSessionArgs {
        host_alias: args.host_alias.clone(),
        project_id,
        worktree_id: None,
        name: String::new(),
        call_id: None,
        new_worktree: None,
        base_branch: None,
        kind: None,
        start_command: None,
        friendly_name: validate::friendly_name(&args.name)
            .is_ok()
            .then(|| args.name.clone()),
        resume_claude_session_id: None,
        model: None,
        effort: None,
        profile: None,
        agent: Some(crate::store::AGENT_CODEX.to_string()),
        origin: Some(crate::store::SessionOrigin::background(
            args.requester_session_id,
        )),
        over_limit_ok: true,
        owner_person_id: owner,
        start_token: None,
    };
    let row = crate::service::sessions::new_session(session_args, store, ssh, reg).await?;
    let mut warning = over.map(|o| format!("{}; it may stall at the limit", o.reason()));
    let row = {
        let s = lock(store)?;
        // Parentage and the requester's work, as `stamp_bg_row` gives a
        // `claude --bg` agent.
        if let Some(req) = args.requester_session_id {
            let _ = s.set_parent_session_id(row.id, Some(req));
            let _ = s.inherit_worker_work(row.id, req);
        }
        if let Err(e) = s.enqueue_handover(row.id, &args.prompt, None) {
            let line = format!(
                "the prompt was not queued ({}); type it into the session",
                e.message
            );
            warning = Some(match warning.take() {
                Some(w) => format!("{w}; {line}"),
                None => line,
            });
        }
        let _ = s.set_last_prompt(row.id, &args.prompt);
        s.get_session_by_id(row.id)?.unwrap_or(row)
    };
    Ok(NewBgSessionResult {
        claude_session_id: None,
        warning,
        session: Some(row),
    })
}

/// How many times `new_bg_session_tracked` lists `claude agents` looking for
/// a just-launched agent by name, and the pause between tries.
const LAUNCH_LOOKUP_TRIES: usize = 3;
const LAUNCH_LOOKUP_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

/// Current wall-clock time in unix seconds (0 if the clock is before 1970).
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Clock-skew slack (seconds) between fleet's clock and the host's when
/// matching a just-launched agent's `startedAt` against the launch time.
const LAUNCH_SKEW_SECS: i64 = 60;

/// The Claude session id of a just-launched agent: the id parsed from the
/// `claude --bg` output when there is one, else the `session_id` of the
/// newest agent listed under `name` that started no earlier than
/// `launch_started - LAUNCH_SKEW_SECS`, else `None`.
///
/// Dead agents stay listed by `claude agents`, so a reused name can match an
/// older run; only agents that started around this launch count, and agents
/// without a `startedAt` are ignored by the name lookup. Pure so the
/// precedence is testable.
fn pick_launched_id(
    parsed: Option<String>,
    agents: &[crate::claude_agents::ClaudeAgentRow],
    name: &str,
    launch_started: i64,
) -> Option<String> {
    parsed.or_else(|| {
        agents
            .iter()
            .filter(|a| a.name.as_deref() == Some(name))
            .filter_map(|a| a.started_at.map(|t| (t, a)))
            .filter(|(t, _)| *t >= launch_started - LAUNCH_SKEW_SECS)
            .filter(|(_, a)| a.session_id.is_some())
            .max_by_key(|(t, _)| *t)
            .and_then(|(_, a)| a.session_id.clone())
    })
}

/// Poll `claude agents` on `host_alias` (up to [`LAUNCH_LOOKUP_TRIES`] times,
/// [`LAUNCH_LOOKUP_DELAY`] apart) for the agent launched as `name` at
/// `launch_started` (unix seconds). Touches no store, so nothing is held
/// across the sleeps.
async fn find_launched_id(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    name: &str,
    launch_started: i64,
) -> Option<String> {
    let tmux = crate::service::sessions::exec_for(host_alias, ssh);
    for attempt in 0..LAUNCH_LOOKUP_TRIES {
        if attempt > 0 {
            tokio::time::sleep(LAUNCH_LOOKUP_DELAY).await;
        }
        let agents = tmux.list_claude_agents().await.unwrap_or_default();
        if let Some(id) = pick_launched_id(None, &agents, name, launch_started) {
            return Some(id);
        }
    }
    None
}

/// Find the bg row for `claude_id` and record the launch prompt on it.
/// Returns the refreshed row, or `None` when reconcile has not surfaced the
/// agent yet.
///
/// `owner` is claimed here, POST HOC, which is what every create path does
/// (multi-user M1, T5 and its review): [`Store::claim_if_unclaimed`] against
/// the row reconcile has just surfaced is the ONE mechanism that stamps an
/// owner. A background agent could not have used anything else in any case —
/// its row is the synthetic `bg:<claude session id>`, and the id is minted by
/// `claude --bg` and not known until it answers.
///
/// Soft, unlike `new_session`'s: every write in this function is, because the
/// agent is already running and the row is a *tracking* row this function may
/// legitimately not find at all (it returns `None` when reconcile has not seen
/// the agent yet). A failed claim leaves the row `unclaimed` — nobody's, never
/// somebody else's — and it is logged. A re-launch cannot mis-attribute it
/// either: `claim_if_unclaimed` refuses a row that is already another person's,
/// so a resurrected `bg:<id>` row keeps its ORIGINAL owner.
fn stamp_bg_row(
    store: &Mutex<Store>,
    claude_id: &str,
    prompt: &str,
    requester: Option<i64>,
    owner: Option<i64>,
) -> Option<crate::store::SessionRow> {
    let s = store.lock().ok()?;
    let row = s.get_session_by_claude_id(claude_id).ok().flatten()?;
    let now = now_unix();
    if let Err(e) = s.claim_if_unclaimed(row.id, owner) {
        tracing::warn!(
            session_id = row.id,
            error = %e.message,
            "[bg] claiming the background agent's row failed; it stays unclaimed"
        );
    }
    let _ = s.set_started_at(row.id, now);
    // A `claude --bg` agent, for its requester when it names one
    // (migration 124). Soft, like the stamps around it: the agent runs
    // either way, and a missing origin reads as "found on the host".
    let _ = s.set_session_origin(row.id, &crate::store::SessionOrigin::background(requester));
    let _ = s.set_last_prompt(row.id, prompt);
    if row.friendly_name.is_none() {
        // The launch prompt may carry the untrusted MCP marker as its first
        // line; the label must be derived from the body (D8 / Q2).
        let body = crate::mcp::guard::strip_marker(prompt);
        if let Some(name) = crate::service::sessions::label_from_prompt(body) {
            let _ = s.set_friendly_name(&row.host_alias, &row.tmux_name, Some(&name));
        }
    }
    let _ = s.insert_session_event(
        row.id,
        "prompt_sent",
        Some(&prompt.chars().take(120).collect::<String>()),
    );
    // Parentage is how the requester's Conversations tab finds this row
    // again; `dispatch_task` stamps its worker the same way.
    if let Some(req) = requester {
        let _ = s.set_parent_session_id(row.id, requester);
        // The worker does the requester's work (work graph M2.2).
        let _ = s.inherit_worker_work(row.id, req);
    }
    s.get_session_by_id(row.id).ok().flatten()
}

#[derive(Debug, Deserialize)]
pub struct DismissAgentArgs {
    pub session_id: i64,
}

/// Remove an inactive background agent from the list (spec §3): records the
/// dismissal and deletes its `bg:<id>` row (`session:removed`). Only a
/// `kind='bg'` row that is not `working` qualifies — an `external` row leaves
/// the list when its process ends, and a working agent must be stopped first.
pub fn dismiss_agent_session(args: DismissAgentArgs, store: &Mutex<Store>) -> Result<(), IpcError> {
    let s = lock(store)?;
    let sess = s
        .get_session_by_id(args.session_id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "session not found"))?;
    if sess.kind != "bg" {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            "only background agents can be removed from the list",
        ));
    }
    if sess.claude_status.as_deref() == Some("working") {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            "stop the agent first",
        ));
    }
    let Some(cid) = sess
        .claude_session_id
        .as_deref()
        .filter(|c| !c.trim().is_empty())
    else {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            "background agent has no Claude session id",
        ));
    };
    let now = now_unix();
    s.dismiss_agent(&sess.host_alias, cid, now)?;
    Ok(())
}

/// Purge Claude Code state for a project on every host in `host_aliases`,
/// then delete the fleet row. The row (and its session rows) is deleted only
/// when every host succeeded — purged, or held no state. On any failure it is
/// kept, so the purge can be retried and the host list re-derived from the
/// project's sessions.
pub async fn purge_project(
    args: PurgeProjectArgs,
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<PurgeReport>, IpcError> {
    purge_project_with(args, store, |host, path| {
        let ssh = Arc::clone(ssh);
        async move { claude_cli::claude_purge_project(&ssh, &host, &path).await }
    })
    .await
}

/// [`purge_project`] with the per-host purge injected, so the
/// all-or-nothing row deletion is testable without ssh or a real `claude`.
async fn purge_project_with<F, Fut>(
    args: PurgeProjectArgs,
    store: &Mutex<Store>,
    purge: F,
) -> Result<Vec<PurgeReport>, IpcError>
where
    F: Fn(String, String) -> Fut,
    Fut: std::future::Future<Output = Result<PurgeReport, IpcError>>,
{
    // Syntax (validate::host_alias) and path checks before anything runs.
    args.validate()?;
    // Syntax is not enough: only registered hosts may be reached over ssh.
    // `local` never goes through ssh and has no guaranteed hosts row.
    {
        let s = lock(store)?;
        for host in args.host_aliases.iter().filter(|h| h.as_str() != "local") {
            if s.get_host_row(host)?.is_none() {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("unknown host: {host}"),
                ));
            }
        }
    }
    let mut reports = Vec::with_capacity(args.host_aliases.len());
    for host in &args.host_aliases {
        reports.push(purge(host.clone(), args.project_path.clone()).await?);
    }
    // Fingerprint keys are resolved before the lock (filesystem access).
    let fp_keys = Store::fingerprint_keys_of_project(store, args.project_id);
    let s = lock(store)?;
    s.delete_project(args.project_id, &fp_keys)?;
    // The transcripts are gone: the work those conversations belonged to
    // can no longer be *continued*, only restarted with a brief (M2.2).
    if let Err(e) = s.mark_purged_work_unresumable(args.project_id, &args.host_aliases) {
        tracing::warn!(error = %e.message, "[work] marking purged work failed");
    }
    Ok(reports)
}

// ─── stop limits (migration 149) ─────────────────────────────────────────────

/// Record a just-launched agent's stop limits. Re-recording (a reused id)
/// replaces the old limits and re-arms them.
pub fn record_stop_limits(
    s: &Store,
    host_alias: &str,
    claude_session_id: &str,
    created_at: i64,
    limits: StopLimits,
) -> Result<(), IpcError> {
    s.conn_ref().execute(
        "INSERT INTO bg_stop_limits \
             (host_alias, claude_session_id, created_at, stop_at, stop_cost_micros) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT (host_alias, claude_session_id) DO UPDATE SET \
             created_at = excluded.created_at, stop_at = excluded.stop_at, \
             stop_cost_micros = excluded.stop_cost_micros, stopped_at = NULL, reason = NULL",
        rusqlite::params![
            host_alias,
            claude_session_id,
            created_at,
            limits.stop_at,
            limits.stop_cost_micros
        ],
    )?;
    Ok(())
}

/// How long a limit waits for its agent's row before it is dropped as gone
/// (the row was reaped, or reconcile never matched the agent).
const STOP_LIMIT_ORPHAN_SECS: i64 = 3600;

/// What the enforcement pass does with one live limit.
#[derive(Debug, Clone, PartialEq)]
pub enum StopDecision {
    /// Stop the agent's `bg:` row; `reason` is `"time"` or `"cost"`.
    Stop {
        host_alias: String,
        claude_session_id: String,
        tmux_name: String,
        reason: &'static str,
    },
    /// The agent already stopped, or its row is long gone: close the limit.
    Gone {
        host_alias: String,
        claude_session_id: String,
    },
}

/// Every live limit that needs action at `now`. A limit whose row has not
/// appeared yet waits (its time limit still fires once the row exists).
pub fn due_stops(s: &Store, now: i64) -> Result<Vec<StopDecision>, IpcError> {
    let mut stmt = s.conn_ref().prepare(
        "SELECT l.host_alias, l.claude_session_id, l.created_at, l.stop_at, \
                l.stop_cost_micros, x.tmux_name, x.usage_cost_micros, x.claude_status \
           FROM bg_stop_limits l \
           LEFT JOIN sessions x \
             ON x.host_alias = l.host_alias AND x.tmux_name = 'bg:' || l.claude_session_id \
          WHERE l.stopped_at IS NULL",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, Option<i64>>(3)?,
            r.get::<_, Option<i64>>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, Option<i64>>(6)?,
            r.get::<_, Option<String>>(7)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (host_alias, claude_session_id, created_at, stop_at, cap, tmux, cost, status) = row?;
        let Some(tmux_name) = tmux else {
            if now - created_at > STOP_LIMIT_ORPHAN_SECS {
                out.push(StopDecision::Gone {
                    host_alias,
                    claude_session_id,
                });
            }
            continue;
        };
        if status.as_deref() == Some("stopped") {
            out.push(StopDecision::Gone {
                host_alias,
                claude_session_id,
            });
            continue;
        }
        let reason = if stop_at.is_some_and(|t| t <= now) {
            "time"
        } else if cap.is_some_and(|c| cost.unwrap_or(0) >= c) {
            "cost"
        } else {
            continue;
        };
        out.push(StopDecision::Stop {
            host_alias,
            claude_session_id,
            tmux_name,
            reason,
        });
    }
    Ok(out)
}

/// Close a limit: the agent was stopped for `reason`, or is gone.
pub fn mark_stopped(
    s: &Store,
    host_alias: &str,
    claude_session_id: &str,
    now: i64,
    reason: &str,
) -> Result<(), IpcError> {
    s.conn_ref().execute(
        "UPDATE bg_stop_limits SET stopped_at = ?3, reason = ?4 \
          WHERE host_alias = ?1 AND claude_session_id = ?2 AND stopped_at IS NULL",
        rusqlite::params![host_alias, claude_session_id, now, reason],
    )?;
    Ok(())
}

/// The reconcile tick's stop-limit pass: stop every background agent past
/// its deadline or spend cap. Single-flight and off the tick body, since a
/// stop is an SSH round trip. Spend is the row's estimated cost as the
/// usage collector last summed it, so a cap can overshoot by one collection
/// interval's worth of work.
pub fn spawn_enforce_stop_limits(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static RUNNING: AtomicBool = AtomicBool::new(false);
    let now = now_unix();
    let due = match lock(store).and_then(|s| due_stops(&s, now)) {
        Ok(due) if !due.is_empty() => due,
        Ok(_) => return,
        Err(e) => {
            tracing::debug!(error = %e.message, "[bg] stop limits not read");
            return;
        }
    };
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let store = Arc::clone(store);
    let ssh = Arc::clone(ssh);
    crate::rt::spawn(async move {
        let _flight = crate::rt::ClearOnDrop::of_static(&RUNNING);
        for d in due {
            let (host, id, reason) = match d {
                StopDecision::Gone {
                    host_alias,
                    claude_session_id,
                } => (host_alias, claude_session_id, "gone"),
                StopDecision::Stop {
                    host_alias,
                    claude_session_id,
                    tmux_name,
                    reason,
                } => {
                    let args = crate::service::sessions::KillSessionArgs {
                        host_alias: host_alias.clone(),
                        name: tmux_name,
                        force: false,
                    };
                    match crate::service::sessions::kill_session(args, &store, &ssh).await {
                        Ok(_) => {
                            tracing::info!(
                                host = %host_alias,
                                claude_session_id = %claude_session_id,
                                reason,
                                "[bg] stopped a background agent at its limit"
                            );
                            (host_alias, claude_session_id, reason)
                        }
                        Err(e) if e.code == codes::E_NOTFOUND => {
                            (host_alias, claude_session_id, "gone")
                        }
                        Err(e) => {
                            // Retried on the next tick.
                            tracing::warn!(
                                host = %host_alias,
                                error = %e.message,
                                "[bg] stopping a background agent at its limit failed"
                            );
                            continue;
                        }
                    }
                }
            };
            if let Err(e) = lock(&store).and_then(|s| mark_stopped(&s, &host, &id, now, reason)) {
                tracing::warn!(error = %e.message, "[bg] stop limit not closed");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::sync::Arc;

    fn make_store() -> Arc<Mutex<Store>> {
        Arc::new(Mutex::new(Store::open_in_memory().unwrap()))
    }

    #[test]
    fn new_bg_session_args_validates_empty_prompt() {
        let _ = make_store(); // ensure it compiles; tests only need validate()
        let args = NewBgSessionArgs {
            host_alias: "local".into(),
            name: "test-session".into(),
            prompt: "".into(),
            requester_session_id: None,
            ..Default::default()
        };
        assert!(args.validate().is_err());
    }

    #[test]
    fn new_bg_session_args_validates_empty_name() {
        let args = NewBgSessionArgs {
            host_alias: "local".into(),
            name: "".into(),
            prompt: "Do the thing".into(),
            requester_session_id: None,
            ..Default::default()
        };
        assert!(args.validate().is_err());
    }

    #[test]
    fn stamp_bg_row_names_and_stamps_the_reconciled_row() {
        let store = make_store();
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_bg_session("local", "bg:u1", None, "u1", Some("working"), 5, "bg", 5)
                .unwrap();
        }
        let row =
            stamp_bg_row(&store, "u1", "Review the auth PR, carefully!", None, None).expect("row");
        assert_eq!(
            row.friendly_name.as_deref(),
            Some("review the auth pr carefully")
        );
        assert_eq!(
            row.last_prompt.as_deref(),
            Some("Review the auth PR, carefully!")
        );
        assert!(row.started_at.is_some());
        // Unknown id ⇒ None, no panic.
        assert!(stamp_bg_row(&store, "nope", "x", None, None).is_none());
    }

    /// Multi-user M1 (T5): the bg row is claimed post hoc, and a RE-LAUNCH
    /// under the same claude session id — the row reconcile resurrects — keeps
    /// its ORIGINAL owner. The reservation the tmux paths use cannot cover
    /// this: a background agent has no tmux name to reserve, and its id is not
    /// known until `claude --bg` answers.
    #[test]
    fn stamp_bg_row_claims_the_row_and_a_resurrected_one_keeps_its_first_owner() {
        let store = make_store();
        let (ann, bob) = {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_bg_session("local", "bg:u1", None, "u1", Some("working"), 5, "bg", 5)
                .unwrap();
            s.upsert_bg_session("local", "bg:u2", None, "u2", Some("working"), 5, "bg", 5)
                .unwrap();
            (
                s.create_person("ann", None).unwrap().id,
                s.create_person("bob", None).unwrap().id,
            )
        };
        let row = stamp_bg_row(&store, "u1", "go", None, Some(ann)).expect("the row is reconciled");
        assert_eq!(row.owner_person_id, Some(ann));
        assert_eq!(row.visibility, crate::store::VISIBILITY_PRIVATE);

        // The same row stamped again for somebody else: refused inside
        // `claim_if_unclaimed`, soft-failed here, and ann is still the owner.
        let again = stamp_bg_row(&store, "u1", "go again", None, Some(bob)).expect("still there");
        assert_eq!(again.owner_person_id, Some(ann), "never re-owned");

        // And no owner at all leaves the row unclaimed rather than guessing.
        let nobodys = stamp_bg_row(&store, "u2", "go", None, None).expect("the row is reconciled");
        assert_eq!(nobodys.owner_person_id, None);
        assert_eq!(nobodys.visibility, crate::store::VISIBILITY_UNCLAIMED);
    }

    #[test]
    fn stamp_bg_row_records_the_session_that_asked_for_it() {
        let store = make_store();
        let parent = {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_bg_session("local", "bg:u0", None, "u0", Some("working"), 5, "bg", 5)
                .unwrap();
            s.upsert_bg_session("local", "bg:u1", None, "u1", Some("working"), 5, "bg", 5)
                .unwrap();
            s.get_session_by_claude_id("u0").unwrap().unwrap().id
        };
        let row =
            stamp_bg_row(&store, "u1", "go", Some(parent), None).expect("the row is reconciled");
        assert_eq!(row.parent_session_id, Some(parent));
        // Migration 122: a background agent, started for that session.
        let parent_ref = parent.to_string();
        assert_eq!(
            (row.origin.as_deref(), row.origin_ref.as_deref()),
            (Some("background"), Some(parent_ref.as_str()))
        );
    }

    #[test]
    fn stamp_bg_row_leaves_no_parent_when_nobody_asked() {
        let store = make_store();
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_bg_session("local", "bg:u1", None, "u1", Some("working"), 5, "bg", 5)
                .unwrap();
        }
        let row = stamp_bg_row(&store, "u1", "go", None, None).expect("the row is reconciled");
        assert_eq!(row.parent_session_id, None);
        assert_eq!(
            (row.origin.as_deref(), row.origin_ref),
            (Some("background"), None)
        );
    }

    /// A listed background agent launched at `started_at` (unix seconds).
    fn agent(
        session_id: &str,
        name: &str,
        started_at: Option<i64>,
    ) -> crate::claude_agents::ClaudeAgentRow {
        crate::claude_agents::ClaudeAgentRow {
            session_id: Some(session_id.into()),
            name: Some(name.into()),
            status: Some("working".into()),
            cwd: None,
            kind: crate::claude_agents::AgentKind::Background,
            job_id: Some("44366faf".into()),
            started_at,
        }
    }

    const LAUNCH: i64 = 1_800_000_000;

    #[test]
    fn pick_launched_id_prefers_the_parsed_id() {
        let agents = vec![agent("listed", "review-auth", Some(LAUNCH))];
        assert_eq!(
            pick_launched_id(Some("parsed".into()), &agents, "review-auth", LAUNCH).as_deref(),
            Some("parsed")
        );
    }

    #[test]
    fn pick_launched_id_falls_back_to_the_agent_with_that_name() {
        let agents = vec![
            agent("other", "something-else", Some(LAUNCH + 1)),
            agent("listed", "review-auth", Some(LAUNCH + 1)),
        ];
        assert_eq!(
            pick_launched_id(None, &agents, "review-auth", LAUNCH).as_deref(),
            Some("listed")
        );
    }

    #[test]
    fn pick_launched_id_none_when_neither_is_known() {
        let agents = vec![agent("other", "something-else", Some(LAUNCH))];
        assert_eq!(pick_launched_id(None, &agents, "review-auth", LAUNCH), None);
        assert_eq!(pick_launched_id(None, &[], "review-auth", LAUNCH), None);
    }

    #[test]
    fn pick_launched_id_ignores_an_older_agent_with_the_same_name() {
        // A dead agent from an earlier launch stays listed under the reused
        // name; it started well before this launch, so it is not ours.
        let agents = vec![agent("stale", "review-auth", Some(LAUNCH - 3_600))];
        assert_eq!(pick_launched_id(None, &agents, "review-auth", LAUNCH), None);
        // Just inside the 60 s clock-skew window still counts.
        let agents = vec![agent("skewed", "review-auth", Some(LAUNCH - 60))];
        assert_eq!(
            pick_launched_id(None, &agents, "review-auth", LAUNCH).as_deref(),
            Some("skewed")
        );
    }

    #[test]
    fn pick_launched_id_newest_of_two_recent_same_name_agents_wins() {
        let agents = vec![
            agent("stale", "review-auth", Some(LAUNCH - 7_200)),
            agent("newer", "review-auth", Some(LAUNCH + 5)),
            agent("older", "review-auth", Some(LAUNCH - 10)),
        ];
        assert_eq!(
            pick_launched_id(None, &agents, "review-auth", LAUNCH).as_deref(),
            Some("newer")
        );
    }

    #[test]
    fn pick_launched_id_ignores_a_same_name_agent_without_started_at() {
        let agents = vec![agent("unknown", "review-auth", None)];
        assert_eq!(pick_launched_id(None, &agents, "review-auth", LAUNCH), None);
    }

    fn seed_agent_row(store: &Mutex<Store>, cid: &str, status: &str, kind: &str) -> i64 {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_bg_session(
            "local",
            &format!("bg:{cid}"),
            None,
            cid,
            Some(status),
            5,
            kind,
            5,
        )
        .unwrap()
    }

    #[test]
    fn dismiss_agent_session_refuses_an_external_row() {
        let store = make_store();
        let id = seed_agent_row(&store, "ext-1", "idle", "external");
        let err = dismiss_agent_session(DismissAgentArgs { session_id: id }, &store).unwrap_err();
        assert_eq!(err.code, "E_INVALID_STATE");
        let s = store.lock().unwrap();
        assert!(s.get_session_by_id(id).unwrap().is_some());
        assert!(s.dismissed_agents("local").unwrap().is_empty());
    }

    #[test]
    fn dismiss_agent_session_refuses_a_working_bg_agent() {
        let store = make_store();
        let id = seed_agent_row(&store, "bg-1", "working", "bg");
        let err = dismiss_agent_session(DismissAgentArgs { session_id: id }, &store).unwrap_err();
        assert_eq!(err.code, "E_INVALID_STATE");
        let s = store.lock().unwrap();
        assert!(s.get_session_by_id(id).unwrap().is_some());
        assert!(s.dismissed_agents("local").unwrap().is_empty());
    }

    #[test]
    fn dismiss_agent_session_removes_a_stopped_bg_agent() {
        let store = make_store();
        let id = seed_agent_row(&store, "bg-2", "stopped", "bg");
        dismiss_agent_session(DismissAgentArgs { session_id: id }, &store).unwrap();
        let s = store.lock().unwrap();
        assert!(s.get_session_by_id(id).unwrap().is_none());
        assert!(s.dismissed_agents("local").unwrap().contains_key("bg-2"));
    }

    #[test]
    fn dismiss_agent_session_unknown_id_is_not_found() {
        let store = make_store();
        let err = dismiss_agent_session(DismissAgentArgs { session_id: 4242 }, &store).unwrap_err();
        assert_eq!(err.code, "E_NOTFOUND");
    }

    #[test]
    fn bg_session_result_warns_when_id_missing() {
        let res = bg_session_result(None);
        assert!(res.claude_session_id.is_none());
        assert_eq!(res.warning.as_deref(), Some(BG_NO_ID_WARNING));
    }

    #[test]
    fn bg_session_result_no_warning_when_id_present() {
        let res = bg_session_result(Some("abc-123".into()));
        assert_eq!(res.claude_session_id.as_deref(), Some("abc-123"));
        assert!(res.warning.is_none());
    }

    #[test]
    fn new_bg_session_args_rejects_option_like_values_and_bad_host() {
        let ok = NewBgSessionArgs {
            host_alias: "mefistos".into(),
            name: "review-1".into(),
            prompt: "Summarise the diff".into(),
            requester_session_id: None,
            ..Default::default()
        };
        assert!(ok.validate().is_ok());

        // A prompt may start with `-` (markdown list): bg_script emits `--`
        // before it, so it can never be parsed as a flag.
        let dash_prompt = NewBgSessionArgs {
            prompt: "- fix login\n- add test".into(),
            ..ok
        };
        assert!(dash_prompt.validate().is_ok());

        let blank_prompt = NewBgSessionArgs {
            prompt: "   ".into(),
            ..dash_prompt
        };
        assert_eq!(blank_prompt.validate().unwrap_err().code, "E_INVALID");

        let bad_name = NewBgSessionArgs {
            name: "-n".into(),
            prompt: "fine".into(),
            ..blank_prompt
        };
        assert_eq!(bad_name.validate().unwrap_err().code, "E_INVALID");

        let ctrl_name = NewBgSessionArgs {
            name: "a\nb".into(),
            ..bad_name
        };
        assert_eq!(ctrl_name.validate().unwrap_err().code, "E_INVALID");

        let bad_host = NewBgSessionArgs {
            host_alias: "-oProxyCommand=id".into(),
            name: "ok".into(),
            ..ctrl_name
        };
        assert_eq!(bad_host.validate().unwrap_err().code, "E_INVALID");
    }

    fn purge_args(hosts: &[&str], path: &str, project_id: i64) -> PurgeProjectArgs {
        PurgeProjectArgs {
            host_aliases: hosts.iter().map(|h| h.to_string()).collect(),
            project_path: path.into(),
            project_id,
        }
    }

    #[test]
    fn purge_project_args_validates_hosts_and_requires_an_absolute_path() {
        assert!(purge_args(&["local"], "/home/me/projects/x", 1)
            .validate()
            .is_ok());
        assert!(purge_args(&["local", "box"], "/x", 1).validate().is_ok());
        for bad in ["", "  ", "--all", "-rf", "/a\nb", "rel/p", "~/p", "./p"] {
            let err = purge_args(&["local"], bad, 1).validate().unwrap_err();
            assert_eq!(err.code, "E_INVALID", "{bad:?}");
        }
        for hosts in [&[][..], &["has space"][..], &["local", "-tt"][..]] {
            let err = purge_args(hosts, "/x", 1).validate().unwrap_err();
            assert_eq!(err.code, "E_INVALID", "{hosts:?}");
        }
    }

    /// A project with one session on each of `hosts`; returns its id.
    fn seed_project(store: &Mutex<Store>, hosts: &[&str]) -> i64 {
        let s = store.lock().unwrap();
        let pid = s.upsert_project("o", "r", "/home/u/p/r").unwrap();
        for (i, host) in hosts.iter().enumerate() {
            s.upsert_host(host).unwrap();
            s.upsert_session(
                &format!("dev-{i}"),
                host,
                Some(pid),
                None,
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        }
        pid
    }

    /// (project row still present, session rows left on `hosts`).
    fn project_state(store: &Mutex<Store>, pid: i64, hosts: &[&str]) -> (bool, usize) {
        let s = store.lock().unwrap();
        let exists = s.list_projects().unwrap().iter().any(|p| p.id == pid);
        let sessions = hosts
            .iter()
            .map(|h| s.list_sessions_for_host(h).unwrap().len())
            .sum();
        (exists, sessions)
    }

    fn report_for(host: &str, path: &str) -> PurgeReport {
        PurgeReport {
            host_alias: host.into(),
            logical_path: path.into(),
            physical_path: Some(path.into()),
            purged: vec![path.into()],
            not_found: vec![],
        }
    }

    #[tokio::test]
    async fn purge_project_keeps_row_and_sessions_when_a_later_host_fails() {
        let store = make_store();
        let pid = seed_project(&store, &["alpha", "beta"]);
        let calls = Mutex::new(Vec::new());
        let err = purge_project_with(
            purge_args(&["alpha", "beta"], "/home/u/p/r", pid),
            &store,
            |host, path| {
                calls.lock().unwrap().push(host.clone());
                async move {
                    if host == "beta" {
                        Err(IpcError::new(
                            codes::E_CLAUDE_CLI,
                            "claude CLI failed on beta",
                        ))
                    } else {
                        Ok(report_for(&host, &path))
                    }
                }
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_CLAUDE_CLI");
        assert_eq!(*calls.lock().unwrap(), vec!["alpha", "beta"]);
        assert_eq!(project_state(&store, pid, &["alpha", "beta"]), (true, 2));
    }

    #[tokio::test]
    async fn purge_project_deletes_row_only_after_every_host_succeeds() {
        let store = make_store();
        let pid = seed_project(&store, &["alpha", "beta"]);
        let reports = purge_project_with(
            purge_args(&["alpha", "beta"], "/home/u/p/r", pid),
            &store,
            |host, path| async move { Ok(report_for(&host, &path)) },
        )
        .await
        .unwrap();
        let hosts: Vec<_> = reports.iter().map(|r| r.host_alias.as_str()).collect();
        assert_eq!(hosts, vec!["alpha", "beta"]);
        assert_eq!(project_state(&store, pid, &["alpha", "beta"]), (false, 0));
    }

    /// A session of ANOTHER project pointing at one of this project's
    /// worktree rows (the old duplicate scan left such references) must not
    /// make the row delete fail on the foreign key after the transcripts are
    /// already purged. The reference is cleared; the other session stays.
    #[tokio::test]
    async fn purge_project_clears_cross_project_worktree_references() {
        let store = make_store();
        let pid = seed_project(&store, &["alpha"]);
        let other = {
            let s = store.lock().unwrap();
            let wt = s.upsert_worktree(pid, "main", "/home/u/p/r", None).unwrap();
            let other_pid = s.upsert_project("o", "other", "/home/u/p/other").unwrap();
            s.upsert_session(
                "dev-other",
                "alpha",
                Some(other_pid),
                Some(wt),
                1,
                1,
                "running",
                None,
            )
            .unwrap()
        };
        purge_project_with(
            purge_args(&["alpha"], "/home/u/p/r", pid),
            &store,
            |host, path| async move { Ok(report_for(&host, &path)) },
        )
        .await
        .expect("the purge must not fail on the foreign key");
        assert_eq!(project_state(&store, pid, &["alpha"]), (false, 1));
        let s = store.lock().unwrap();
        let row = s
            .get_session_by_id(other)
            .unwrap()
            .expect("the other project's session stays");
        assert_eq!(row.worktree_id, None);
    }

    #[tokio::test]
    async fn purge_project_rejects_invalid_or_unknown_hosts_before_purging() {
        let store = make_store();
        let pid = seed_project(&store, &["alpha"]);
        let cases: [(&[&str], &str); 4] = [
            (&["-oProxyCommand=id"], "E_INVALID"),
            (&["has space"], "E_INVALID"),
            (&["alpha", "a;b"], "E_INVALID"),
            (&["alpha", "ghost"], "E_NOTFOUND"),
        ];
        let called = std::sync::atomic::AtomicBool::new(false);
        for (hosts, code) in cases {
            let err = purge_project_with(purge_args(hosts, "/home/u/p/r", pid), &store, |_, _| {
                called.store(true, std::sync::atomic::Ordering::SeqCst);
                async { Err::<PurgeReport, _>(IpcError::new("E_TEST", "must not run")) }
            })
            .await
            .unwrap_err();
            assert_eq!(err.code, code, "{hosts:?}");
        }
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(project_state(&store, pid, &["alpha"]), (true, 1));
    }

    #[tokio::test]
    async fn purge_project_rejects_invalid_host_and_keeps_the_project() {
        let store = make_store();
        let pid = store
            .lock()
            .unwrap()
            .upsert_project("o", "r", "/home/u/p/r")
            .unwrap();
        let ssh = Arc::new(SshClient::new());
        for bad in ["-oProxyCommand=id", "has space", "", "a;b", "x\ny"] {
            let err = purge_project(
                PurgeProjectArgs {
                    host_aliases: vec![bad.into()],
                    project_path: "/home/u/p/r".into(),
                    project_id: pid,
                },
                &store,
                &ssh,
            )
            .await
            .unwrap_err();
            assert_eq!(err.code, "E_INVALID", "{bad:?}");
        }
        let projects = store.lock().unwrap().list_projects().unwrap();
        assert!(projects.iter().any(|p| p.id == pid), "row must survive");
    }

    fn in_project_args() -> NewBgSessionArgs {
        NewBgSessionArgs {
            agent: Some("codex".into()),
            project_id: Some(7),
            ..bg_args()
        }
    }

    fn bg_args() -> NewBgSessionArgs {
        NewBgSessionArgs {
            host_alias: "mac".into(),
            name: "look".into(),
            prompt: "read the code".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_codex_background_agent_runs_in_a_project_without_claude_only_options() {
        let codex = NewBgSessionArgs {
            agent: Some("codex".into()),
            ..bg_args()
        };
        let err = codex.validate().unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        assert_eq!(err.message, CODEX_BG_NEEDS_PROJECT);
        let in_project = NewBgSessionArgs {
            project_id: Some(7),
            ..codex
        };
        in_project.validate().unwrap();
        assert!(in_project.is_codex());
        for bad in [
            NewBgSessionArgs {
                read_only: true,
                ..in_project_args()
            },
            NewBgSessionArgs {
                stop_after_secs: Some(600),
                ..in_project_args()
            },
            NewBgSessionArgs {
                stop_after_usd: Some(1.0),
                ..in_project_args()
            },
        ] {
            assert_eq!(bad.validate().unwrap_err().message, CODEX_BG_CLAUDE_ONLY);
        }
        let other = NewBgSessionArgs {
            agent: Some("gpt".into()),
            ..bg_args()
        };
        assert_eq!(other.validate().unwrap_err().code, "E_INVALID");
        let claude = NewBgSessionArgs {
            agent: Some("claude".into()),
            ..bg_args()
        };
        claude.validate().unwrap();
    }

    #[test]
    fn stop_limits_are_bounded_and_land_as_a_deadline_and_micros() {
        for secs in [0, 59, 7 * 24 * 3600 + 1, -5] {
            let a = NewBgSessionArgs {
                stop_after_secs: Some(secs),
                ..bg_args()
            };
            assert!(a.validate().is_err(), "{secs}");
        }
        for usd in [0.0, -1.0, f64::NAN, f64::INFINITY, 1000.01] {
            let a = NewBgSessionArgs {
                stop_after_usd: Some(usd),
                ..bg_args()
            };
            assert!(a.validate().is_err(), "{usd}");
        }
        let a = NewBgSessionArgs {
            stop_after_secs: Some(900),
            stop_after_usd: Some(5.0),
            ..bg_args()
        };
        a.validate().unwrap();
        assert_eq!(
            a.stop_limits(1_000),
            Some(StopLimits {
                stop_at: Some(1_900),
                stop_cost_micros: Some(5_000_000),
            })
        );
        assert_eq!(bg_args().stop_limits(1_000), None);
    }

    /// The wire shape the phone sends: every new field optional, and an
    /// old caller's args read back unchanged.
    #[test]
    fn the_new_args_are_optional_on_the_wire() {
        let old: NewBgSessionArgs =
            serde_json::from_str(r#"{"host_alias":"mac","name":"n","prompt":"p"}"#).unwrap();
        assert_eq!(old.project_id, None);
        assert!(!old.read_only);
        let json = serde_json::to_value(&old).unwrap();
        assert!(json.get("read_only").is_none(), "{json}");
        let new: NewBgSessionArgs = serde_json::from_str(
            r#"{"host_alias":"mac","name":"n","prompt":"p","project_id":3,"agent":"claude",
                "read_only":true,"stop_after_secs":3600,"stop_after_usd":5}"#,
        )
        .unwrap();
        assert_eq!(new.project_id, Some(3));
        assert!(new.read_only);
        assert_eq!(new.stop_after_usd, Some(5.0));
    }

    #[test]
    fn the_enforcement_pass_stops_at_the_deadline_or_the_cap_and_closes_gone_agents() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("mac").unwrap();
        let limits = |stop_at, cap| StopLimits {
            stop_at,
            stop_cost_micros: cap,
        };
        // a: deadline at 200. b: $1 cap. c: stopped by itself. d: no row yet.
        // e: no row for over an hour.
        for (id, l) in [
            ("a", limits(Some(200), None)),
            ("b", limits(None, Some(1_000_000))),
            ("c", limits(Some(10_000), None)),
            ("d", limits(Some(50), None)),
        ] {
            record_stop_limits(&s, "mac", id, 100, l).unwrap();
        }
        record_stop_limits(
            &s,
            "mac",
            "e",
            100 - STOP_LIMIT_ORPHAN_SECS - 1,
            limits(None, Some(1)),
        )
        .unwrap();
        for id in ["a", "b", "c"] {
            let st = if id == "c" { "stopped" } else { "working" };
            s.upsert_bg_session("mac", &format!("bg:{id}"), None, id, Some(st), 1, "bg", 1)
                .unwrap();
        }
        let b_row = s.get_session("bg:b", "mac").unwrap().unwrap().id;
        s.conn_ref()
            .execute(
                "UPDATE sessions SET usage_cost_micros = 999999 WHERE id = ?1",
                [b_row],
            )
            .unwrap();

        let mut due = due_stops(&s, 150).unwrap();
        due.sort_by(|x, y| format!("{x:?}").cmp(&format!("{y:?}")));
        assert_eq!(
            due,
            vec![
                StopDecision::Gone {
                    host_alias: "mac".into(),
                    claude_session_id: "c".into()
                },
                StopDecision::Gone {
                    host_alias: "mac".into(),
                    claude_session_id: "e".into()
                },
            ],
            "nothing has reached a limit at 150; d waits for its row"
        );

        s.conn_ref()
            .execute(
                "UPDATE sessions SET usage_cost_micros = 1000000 WHERE id = ?1",
                [b_row],
            )
            .unwrap();
        let stops: Vec<_> = due_stops(&s, 200)
            .unwrap()
            .into_iter()
            .filter_map(|d| match d {
                StopDecision::Stop {
                    claude_session_id,
                    tmux_name,
                    reason,
                    ..
                } => Some((claude_session_id, tmux_name, reason)),
                StopDecision::Gone { .. } => None,
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        assert_eq!(
            stops,
            vec![
                ("a".to_string(), "bg:a".to_string(), "time"),
                ("b".to_string(), "bg:b".to_string(), "cost"),
            ]
        );

        // Closed limits drop out; re-recording re-arms one.
        for id in ["a", "b", "c", "e"] {
            mark_stopped(&s, "mac", id, 200, "time").unwrap();
        }
        assert!(due_stops(&s, 200).unwrap().is_empty());
        record_stop_limits(&s, "mac", "a", 300, limits(Some(300), None)).unwrap();
        assert_eq!(due_stops(&s, 300).unwrap().len(), 1);
    }
}
