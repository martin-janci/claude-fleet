//! Eligibility, execution and the apply → re-scan step.

use super::sync::{sync, Scope};
use super::{detector_of, scan_all, Finding, FindingBody, Fix, FixPlan, Safety, ScanCx};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::playbooks::PressEnterOutcome;
use crate::service::settings;
use crate::ssh::SshClient;
use crate::store::{AdminEvent, AutofixBudget, AutofixRow, Store};
use std::sync::{Arc, Mutex};

/// Minimum gap between two automatic attempts on one episode.
pub const AUTOFIX_RETRY_SECS: i64 = 600;

/// How long after an attempt a manual apply on a `verifying` row is refused,
/// because that fix may still be running.
pub const AUTOFIX_INFLIGHT_SECS: i64 = 60;

/// The window `autofix.max_attempts` counts over, across every episode of
/// one `(kind, subject)`: a state that clears and comes back each pass
/// opens a new row each time, and must not get a fresh budget each time.
pub const AUTOFIX_BUDGET_WINDOW_SECS: i64 = 86_400;

/// How long a cleared episode is kept as history.
pub const AUTOFIX_RETENTION_SECS: i64 = 30 * 86_400;

/// Who applied a fix, for the `autofix.apply` admin event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    pub label: String,
    pub person: Option<i64>,
    pub device: Option<i64>,
}

impl Actor {
    /// The runner's own apply.
    pub fn auto() -> Self {
        Actor {
            label: "auto".into(),
            person: None,
            device: None,
        }
    }

    /// The desktop the hub runs in (standalone mode).
    pub fn desktop() -> Self {
        Actor {
            label: "desktop".into(),
            person: None,
            device: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub auto_safe: bool,
    pub max_attempts: i64,
    pub detectors_off: Vec<&'static str>,
}

impl Config {
    pub fn read(s: &Store) -> Config {
        let mut off = Vec::new();
        if !settings::get_bool(s, settings::AUTOFIX_DETECTOR_OPERATOR) {
            off.push("operator_unbound");
        }
        if !settings::get_bool(s, settings::AUTOFIX_DETECTOR_HOST) {
            off.push("host");
        }
        if !settings::get_bool(s, settings::AUTOFIX_DETECTOR_MCP_ENTRY) {
            off.push("host_mcp_entry");
        }
        if !settings::get_bool(s, settings::AUTOFIX_DETECTOR_STUCK) {
            off.push("stuck");
        }
        Config {
            auto_safe: settings::get_bool(s, settings::AUTOFIX_AUTO_SAFE),
            max_attempts: settings::get_string(s, settings::AUTOFIX_MAX_ATTEMPTS)
                .parse()
                .unwrap_or(3),
            detectors_off: off,
        }
    }

    pub fn detector_on(&self, kind: &str) -> bool {
        detector_of(kind).is_some_and(|d| !self.detectors_off.contains(&d.id))
    }
}

/// Every finding of the detectors the settings leave on.
pub fn scan_enabled(cx: &ScanCx, cfg: &Config) -> Vec<Finding> {
    only_enabled(scan_all(cx), cfg)
}

fn only_enabled(found: Vec<Finding>, cfg: &Config) -> Vec<Finding> {
    found
        .into_iter()
        .filter(|f| cfg.detector_on(&f.kind))
        .collect()
}

/// Whether the runner may apply `fix` by itself now. `budget` is the
/// attempts on every episode of the row's `(kind, subject)` within
/// [`AUTOFIX_BUDGET_WINDOW_SECS`], this row's included
/// ([`Store::autofix_budget`]), so a flapping state shares one budget.
pub fn auto_eligible(
    row: &AutofixRow,
    fix: &Fix,
    cfg: &Config,
    budget: &AutofixBudget,
    now: i64,
) -> bool {
    let attempts = budget.attempts.max(row.attempts);
    let last = budget.last_attempt_at.max(row.last_attempt_at);
    fix.safety == Safety::Safe
        && fix.auto
        && cfg.auto_safe
        && cfg.detector_on(&row.kind)
        && matches!(row.state.as_str(), "open" | "apply_failed")
        && attempts < cfg.max_attempts
        && last.is_none_or(|t| now - t >= AUTOFIX_RETRY_SECS)
}

fn budget_of(s: &Store, row: &AutofixRow, now: i64) -> Result<AutofixBudget, IpcError> {
    Ok(s.autofix_budget(
        row.id,
        &row.kind,
        &row.subject,
        now - AUTOFIX_BUDGET_WINDOW_SECS,
    )?)
}

#[async_trait::async_trait]
pub trait FixExec: Send + Sync {
    async fn provision_content(&self, host: &str) -> Result<(), IpcError>;
    async fn update_agent(&self, host: &str) -> Result<(), IpcError>;
    /// Rewrite the host's `claude-fleet` entry, then probe it again so the
    /// re-scan reads the entry as it now is.
    async fn rewrite_mcp_entry(&self, host: &str) -> Result<(), IpcError>;
    async fn press_enter(&self, host: &str, tmux_name: &str)
        -> Result<PressEnterOutcome, IpcError>;
    async fn recreate(&self, session_id: i64) -> Result<(), IpcError>;
}

/// The connected agents as `(alias, agent version)`, for the re-scan.
pub fn live_agents(ssh: &SshClient) -> Vec<(String, String)> {
    ssh.agent_registry()
        .map(|r| {
            r.snapshot()
                .into_iter()
                .map(|a| (a.alias, a.agent_version))
                .collect()
        })
        .unwrap_or_default()
}

pub struct SshFixExec<'a> {
    pub store: &'a Mutex<Store>,
    pub ssh: &'a Arc<SshClient>,
}

#[async_trait::async_trait]
impl FixExec for SshFixExec<'_> {
    async fn provision_content(&self, host: &str) -> Result<(), IpcError> {
        let base = {
            let s = lock(self.store)?;
            crate::service::hub::HubBase::read(&s)?
        };
        crate::service::provision::provision_content_only(
            self.store,
            self.ssh.as_ref(),
            host,
            &base,
        )
        .await?;
        Ok(())
    }

    async fn update_agent(&self, host: &str) -> Result<(), IpcError> {
        use crate::service::update;
        let now = crate::store::now_unix();
        let r = update::update_now(
            self.store,
            "agent",
            &agent_target(host),
            None,
            &update::trusted_keys(),
            now,
        )?;
        if r.agents.is_empty() {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("{host} has no agent updater to wake"),
            ));
        }
        let script = crate::shell::quote(update::AGENT_POKE_SCRIPT);
        for alias in &r.agents {
            let poked = match self
                .ssh
                .run(
                    alias,
                    &["bash", "-c", &script],
                    std::time::Duration::from_secs(10),
                )
                .await
            {
                Ok(out) => poke_result(&out),
                Err(e) => Err(e),
            };
            poked.map_err(|e| pinned_but_not_woken(alias, &r.pin.version, &e))?;
        }
        Ok(())
    }

    async fn rewrite_mcp_entry(&self, host: &str) -> Result<(), IpcError> {
        use super::mcp_entry;
        let (base, token) = {
            let s = lock(self.store)?;
            let base = crate::service::hub::HubBase::read(&s)?;
            let token = s.get_host_token(host)?.map(|t| t.token).ok_or_else(|| {
                IpcError::new(
                    codes::E_NOTFOUND,
                    format!("{host} has no stored token; provision it first"),
                )
            })?;
            (base, token)
        };
        crate::service::provision::rewrite_mcp_entry(self.ssh.as_ref(), host, &base, &token)
            .await?;
        let probe = mcp_entry::probe(self.ssh.as_ref(), host)
            .await
            .ok()
            .flatten();
        mcp_entry::remember(host, crate::store::now_unix(), probe);
        Ok(())
    }

    async fn press_enter(
        &self,
        host: &str,
        tmux_name: &str,
    ) -> Result<PressEnterOutcome, IpcError> {
        crate::service::playbooks::press_enter_on(self.ssh.as_ref(), host, tmux_name).await
    }

    async fn recreate(&self, session_id: i64) -> Result<(), IpcError> {
        crate::service::sessions::recreate_session(
            crate::service::sessions::RecreateSessionArgs {
                session_id,
                force: false,
            },
            self.store,
            self.ssh,
        )
        .await
        .map(|_| ())
    }
}

/// The agent was pinned (it installs at its next update check) but its
/// updater could not be woken now: a partial success, worded as one.
fn pinned_but_not_woken(host: &str, version: &str, e: &IpcError) -> IpcError {
    IpcError::new(
        &e.code,
        format!(
            "pinned the agent on {host} to {version}, but could not wake its updater ({}); \
             it installs at its next update check, or apply again to retry the wake-up",
            e.message
        ),
    )
}

/// The `update_now` target for one agent host.
fn agent_target(host: &str) -> String {
    format!("agent:{host}")
}

/// A poke worked only when the script exited 0 and printed `poked`
/// (`update::AGENT_POKE_SCRIPT` prints a reason and still exits 0 otherwise).
fn poke_result(out: &std::process::Output) -> Result<(), IpcError> {
    let stdout = String::from_utf8_lossy(&out.stdout);
    if out.status.success() && stdout.contains("poked") {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let said = format!("{} {}", stdout.trim(), stderr.trim());
    Err(IpcError::new(
        codes::E_INTERNAL,
        format!("the agent updater was not woken: {}", said.trim()),
    ))
}

/// Everything detectors read, under the caller's guard.
pub fn gather(s: &Store, live_agents: &[(String, String)], now: i64) -> Result<ScanCx, IpcError> {
    use crate::service::health;
    let hosts = s.list_hosts()?;
    let sessions = s.list_all_sessions()?;
    let version = crate::app_version::get();
    let mut rows = health::hosts_health(
        &hosts,
        &sessions,
        &health::host_thresholds(s),
        live_agents,
        version,
        now,
    );
    health::overlay_agents(&mut rows, live_agents, version);
    let (client_tokens, revoked_client_tokens) = s
        .list_client_tokens(true)?
        .into_iter()
        .partition(|t| t.revoked_at.is_none());
    Ok(ScanCx {
        now,
        owner: s.personal_owner_id()?,
        client_tokens,
        hosts,
        health: rows,
        sessions,
        controller: s.get_controller()?,
        playbooks: crate::service::playbooks::PlaybookConfig::from_store(s),
        mcp_url: crate::service::hub::HubBase::read(s)
            .ok()
            .map(|b| b.mcp_url()),
        mcp_probes: super::mcp_entry::cached(),
        host_token_shas: s
            .list_host_tokens()?
            .into_iter()
            .map(|t| (t.host_alias, crate::mcp::auth::sha256_hex(&t.token)))
            .collect(),
        revoked_client_tokens,
        operator_sha: s.get_setting(crate::service::operator::SETTING_OPERATOR_TOKEN_SHA)?,
    })
}

pub fn body_of(row: &AutofixRow) -> Result<FindingBody, IpcError> {
    serde_json::from_str(&row.body).map_err(|e| {
        IpcError::new(
            codes::E_INTERNAL,
            format!("finding {} has an unreadable body: {e}", row.id),
        )
    })
}

async fn execute(store: &Mutex<Store>, exec: &dyn FixExec, plan: &FixPlan) -> Result<(), IpcError> {
    match plan {
        FixPlan::BindOperator => {
            let s = lock(store)?;
            let owner = s.personal_owner_id()?.ok_or_else(|| {
                IpcError::new(
                    codes::E_NOTFOUND,
                    "this hub has no owner to link the token to",
                )
            })?;
            s.set_client_person(crate::service::operator::OPERATOR_CLIENT_NAME, Some(owner))?;
            Ok(())
        }
        FixPlan::ProvisionContent { host } => exec.provision_content(host).await,
        FixPlan::UpdateAgent { host } => exec.update_agent(host).await,
        FixPlan::RewriteMcpEntry { host } => exec.rewrite_mcp_entry(host).await,
        FixPlan::PressEnter {
            session_id,
            host,
            tmux_name,
            stuck_kind,
        } => {
            still_stuck(store, *session_id, stuck_kind)?;
            match exec.press_enter(host, tmux_name).await? {
                PressEnterOutcome::Sent => Ok(()),
                PressEnterOutcome::SkippedAttached => Err(IpcError::new(
                    codes::E_INVALID,
                    "someone is attached to that session, so nothing was typed into it; \
                     answer the prompt there, or apply again once they detach",
                )),
            }
        }
        FixPlan::Recreate { session_id } => {
            let row = still_stuck(store, *session_id, "oom")?;
            if row.claude_status.as_deref() == Some("working") {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    "that session is running a turn: the out-of-memory text is old, so it was not recreated",
                ));
            }
            exec.recreate(*session_id).await
        }
    }
}

/// The session a keystroke or recreate fix targets, re-read under the
/// guard: refused when it is gone, no longer shows `stuck_kind`, or is the
/// controller (fleet never types into Control's own pane).
fn still_stuck(
    store: &Mutex<Store>,
    session_id: i64,
    stuck_kind: &str,
) -> Result<crate::store::SessionRow, IpcError> {
    let s = lock(store)?;
    let row = s
        .list_all_sessions()?
        .into_iter()
        .find(|r| r.id == session_id)
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no session {session_id}")))?;
    if s.get_controller()?
        .is_some_and(|(h, t)| h == row.host_alias && t == row.tmux_name)
    {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "fleet never types into Control's own session",
        ));
    }
    if row.stuck_kind.as_deref() != Some(stuck_kind) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("that session no longer shows {stuck_kind}; nothing was sent"),
        ));
    }
    Ok(row)
}

/// Run one finding's fix on a person's say-so, re-scan its detector, and
/// return the row as it now stands: `cleared` when the state is gone,
/// `verifying` when it still shows (the next passes decide), `apply_failed`
/// when the fix itself errored.
pub async fn apply_one(
    store: &Mutex<Store>,
    exec: &dyn FixExec,
    id: i64,
    actor: &Actor,
    live_agents: &[(String, String)],
    now: i64,
) -> Result<AutofixRow, IpcError> {
    apply_scanned(store, exec, id, actor, live_agents, now, false).await
}

async fn apply_scanned(
    store: &Mutex<Store>,
    exec: &dyn FixExec,
    id: i64,
    actor: &Actor,
    live_agents: &[(String, String)],
    now: i64,
    auto: bool,
) -> Result<AutofixRow, IpcError> {
    apply_with(store, exec, id, actor, now, auto, &|s| {
        Ok(scan_enabled(
            &gather(s, live_agents, now)?,
            &Config::read(s),
        ))
    })
    .await
}

/// `apply_one` with the re-scan supplied, so tests can say what it finds.
///
/// `auto` is the runner's own apply: under the same guard as the attempt is
/// marked, it re-reads the settings and refuses unless the row is still
/// eligible (not dismissed or already tried since, automation not paused).
/// A manual apply refuses a fix that started less than
/// [`AUTOFIX_INFLIGHT_SECS`] ago and may still be running.
async fn apply_with(
    store: &Mutex<Store>,
    exec: &dyn FixExec,
    id: i64,
    actor: &Actor,
    now: i64,
    auto: bool,
    scan: &(dyn Fn(&Store) -> Result<Vec<Finding>, IpcError> + Sync),
) -> Result<AutofixRow, IpcError> {
    let (row, fix) = {
        let s = lock(store)?;
        let row = s
            .autofix_finding(id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no finding {id}")))?;
        if !matches!(
            row.state.as_str(),
            "open" | "apply_failed" | "dismissed" | "verifying"
        ) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("finding {id} is {}, there is nothing to fix", row.state),
            ));
        }
        let fix = body_of(&row)?.fix.ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID,
                format!("finding {id} has no automatic fix; its explanation says what to do"),
            )
        })?;
        if auto {
            let cfg = Config::read(&s);
            let budget = budget_of(&s, &row, now)?;
            if crate::service::loops::paused(&s) || !auto_eligible(&row, &fix, &cfg, &budget, now) {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("finding {id} is no longer eligible to fix by itself"),
                ));
            }
        } else if row.state == "verifying"
            && row
                .last_attempt_at
                .is_some_and(|t| now - t < AUTOFIX_INFLIGHT_SECS)
        {
            return Err(IpcError::new(
                codes::E_INVALID,
                "a fix for this finding is already running; try again in a minute",
            ));
        }
        // The check above and this mark share one guard, so nothing can
        // change the row between them.
        if !s.mark_autofix_attempt(id, now)? {
            return s
                .autofix_finding(id)?
                .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no finding {id}")));
        }
        (row, fix)
    };
    let result = execute(store, exec, &fix.plan).await;
    let s = lock(store)?;
    s.record_admin_event_or_log(&AdminEvent {
        actor_person: actor.person,
        actor_device: actor.device,
        action: "autofix.apply",
        resource_type: "autofix_finding",
        resource_id: &id.to_string(),
        representation: serde_json::json!({
            "kind": row.kind,
            "subject": row.subject,
            "actor": actor.label,
            "plan": fix.plan,
            "ok": result.is_ok(),
            "error": result.as_ref().err().map(|e| e.message.clone()),
        }),
    });
    match result {
        Err(e) => {
            // False when a sync resolved the row while the fix ran: the row
            // stays as that sync left it.
            s.set_autofix_state(id, "apply_failed", Some(&e.message), now)?;
        }
        Ok(()) => {
            let detector = detector_of(&row.kind).map(|d| d.id).unwrap_or("");
            let found: Vec<_> = scan(&s)?
                .into_iter()
                .filter(|f| detector_of(&f.kind).is_some_and(|d| d.id == detector))
                .collect();
            sync(&s, &found, Scope::Detector(detector), now)?;
        }
    }
    s.autofix_finding(id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no finding {id}")))
}

#[derive(Debug, Clone, Default)]
pub struct PassOutcome {
    pub sync: super::sync::SyncOutcome,
    pub applied: Vec<i64>,
}

/// One autofix pass: scan, sync findings, then apply every safe fix that is
/// eligible to run by itself. Approval fixes are never touched here.
pub async fn run_pass(
    store: &Mutex<Store>,
    exec: &dyn FixExec,
    live_agents: &[(String, String)],
    now: i64,
) -> Result<PassOutcome, IpcError> {
    run_pass_with(store, exec, live_agents, now, scan_all).await
}

/// [`run_pass`] with the scan injectable, for tests.
pub(crate) async fn run_pass_with(
    store: &Mutex<Store>,
    exec: &dyn FixExec,
    live_agents: &[(String, String)],
    now: i64,
    scan: impl Fn(&ScanCx) -> Vec<Finding>,
) -> Result<PassOutcome, IpcError> {
    // The guard lives only in this block: never held across an await. Each
    // apply below re-checks eligibility under its own guard, so a dismissal
    // or a pause made while an earlier fix runs is honoured.
    let (synced, due) = {
        let s = lock(store)?;
        let cfg = Config::read(&s);
        let cx = gather(&s, live_agents, now)?;
        let found = only_enabled(scan(&cx), &cfg);
        let synced = sync(&s, &found, Scope::All, now)?;
        s.prune_autofix_findings(now - AUTOFIX_RETENTION_SECS)?;
        let due: Vec<i64> = if crate::service::loops::paused(&s) {
            Vec::new()
        } else {
            let mut due = Vec::new();
            for r in s.live_autofix_findings()? {
                let Some(f) = body_of(&r).ok().and_then(|b| b.fix) else {
                    continue;
                };
                if auto_eligible(&r, &f, &cfg, &budget_of(&s, &r, now)?, now) {
                    due.push(r.id);
                }
            }
            due
        };
        (synced, due)
    };
    let mut applied = Vec::new();
    for id in due {
        match apply_scanned(store, exec, id, &Actor::auto(), live_agents, now, true).await {
            Ok(_) => applied.push(id),
            Err(e) => {
                tracing::warn!(id, code = %e.code, "[autofix] automatic fix refused: {}", e.message)
            }
        }
    }
    Ok(PassOutcome {
        sync: synced,
        applied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::autofix::sync::{sync, Scope};
    use crate::service::autofix::Severity;
    use crate::service::operator::OPERATOR_CLIENT_NAME;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_actor() -> Actor {
        Actor {
            label: "test".into(),
            person: Some(1),
            device: Some(2),
        }
    }

    #[derive(Default)]
    struct FakeExec {
        provisions: AtomicUsize,
        updates: AtomicUsize,
        rewrites: AtomicUsize,
        presses: AtomicUsize,
        recreates: AtomicUsize,
        attached: bool,
        fail: bool,
    }
    #[async_trait::async_trait]
    impl FixExec for FakeExec {
        async fn provision_content(&self, _host: &str) -> Result<(), IpcError> {
            self.provisions.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(IpcError::new(codes::E_SSH, "boom"))
            } else {
                Ok(())
            }
        }
        async fn update_agent(&self, _host: &str) -> Result<(), IpcError> {
            self.updates.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn rewrite_mcp_entry(&self, _host: &str) -> Result<(), IpcError> {
            self.rewrites.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn press_enter(
            &self,
            _host: &str,
            _tmux: &str,
        ) -> Result<PressEnterOutcome, IpcError> {
            self.presses.fetch_add(1, Ordering::SeqCst);
            Ok(if self.attached {
                PressEnterOutcome::SkippedAttached
            } else {
                PressEnterOutcome::Sent
            })
        }
        async fn recreate(&self, _session_id: i64) -> Result<(), IpcError> {
            self.recreates.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn safe_fix(auto: bool) -> Fix {
        Fix {
            summary: "s".into(),
            changes: vec![],
            safety: Safety::Safe,
            reversible: true,
            auto,
            plan: FixPlan::ProvisionContent { host: "h1".into() },
        }
    }

    fn open_row(attempts: i64, last: Option<i64>) -> AutofixRow {
        AutofixRow {
            id: 1,
            kind: "host.hooks_silent".into(),
            subject: "host:h1".into(),
            host_alias: Some("h1".into()),
            severity: "degraded".into(),
            body: "{}".into(),
            state: "open".into(),
            first_seen_at: 0,
            last_seen_at: 0,
            dismissed_until: None,
            attempts,
            last_attempt_at: last,
            last_error: None,
        }
    }

    fn cfg() -> Config {
        Config {
            auto_safe: true,
            max_attempts: 3,
            detectors_off: vec![],
        }
    }

    fn eligible(row: &AutofixRow, fix: &Fix, cfg: &Config, now: i64) -> bool {
        auto_eligible(row, fix, cfg, &AutofixBudget::default(), now)
    }

    #[test]
    fn only_safe_auto_fixes_within_budget_are_eligible() {
        assert!(eligible(&open_row(0, None), &safe_fix(true), &cfg(), 1000));
        assert!(
            !eligible(&open_row(0, None), &safe_fix(false), &cfg(), 1000),
            "auto: false"
        );
        let mut approval = safe_fix(true);
        approval.safety = Safety::NeedsApproval;
        assert!(
            !eligible(&open_row(0, None), &approval, &cfg(), 1000),
            "never auto"
        );
        assert!(!eligible(
            &open_row(0, None),
            &safe_fix(true),
            &Config {
                auto_safe: false,
                ..cfg()
            },
            1000
        ));
        assert!(!eligible(
            &open_row(0, None),
            &safe_fix(true),
            &Config {
                detectors_off: vec!["host"],
                ..cfg()
            },
            1000
        ));
        assert!(
            !eligible(&open_row(3, Some(0)), &safe_fix(true), &cfg(), 100_000),
            "budget spent"
        );
        assert!(
            !eligible(&open_row(1, Some(900)), &safe_fix(true), &cfg(), 1000),
            "retry backoff"
        );
        let mut dismissed = open_row(0, None);
        dismissed.state = "dismissed".into();
        assert!(!eligible(&dismissed, &safe_fix(true), &cfg(), 1000));
    }

    /// A state that clears and comes back opens a new episode each time; the
    /// attempts of the earlier ones in the window still count.
    #[tokio::test]
    async fn a_flapping_state_shares_one_attempt_budget_across_episodes() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let exec = FakeExec::default();
        let mut now = 10;
        for _ in 0..3 {
            run_pass_with(&store, &exec, &[], now, |_| vec![hooks_silent_finding()])
                .await
                .unwrap();
            // The fix "worked": the state is gone on the next pass…
            now += AUTOFIX_RETRY_SECS;
            run_pass_with(&store, &exec, &[], now, |_| vec![])
                .await
                .unwrap();
            now += AUTOFIX_RETRY_SECS;
        }
        assert_eq!(exec.provisions.load(Ordering::SeqCst), 3);
        // …and back again: a fourth episode, but the budget is spent.
        let out = run_pass_with(&store, &exec, &[], now, |_| vec![hooks_silent_finding()])
            .await
            .unwrap();
        assert!(out.applied.is_empty(), "{out:?}");
        assert_eq!(exec.provisions.load(Ordering::SeqCst), 3);
        // A day later the earlier attempts fall out of the window.
        let later = now + AUTOFIX_BUDGET_WINDOW_SECS;
        let out = run_pass_with(&store, &exec, &[], later, |_| vec![hooks_silent_finding()])
            .await
            .unwrap();
        assert_eq!(out.applied.len(), 1);
    }

    #[tokio::test]
    async fn a_pass_prunes_cleared_episodes_past_retention() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let old = {
            let s = store.lock().unwrap();
            let id = sync(&s, &[hooks_silent_finding()], Scope::All, 10)
                .unwrap()
                .opened[0];
            sync(&s, &[], Scope::All, 20).unwrap();
            id
        };
        let exec = FakeExec::default();
        run_pass_with(&store, &exec, &[], 20 + AUTOFIX_RETENTION_SECS, |_| vec![])
            .await
            .unwrap();
        assert!(store
            .lock()
            .unwrap()
            .autofix_finding(old)
            .unwrap()
            .is_some());
        run_pass_with(&store, &exec, &[], 21 + AUTOFIX_RETENTION_SECS, |_| vec![])
            .await
            .unwrap();
        assert!(store
            .lock()
            .unwrap()
            .autofix_finding(old)
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn the_admin_event_names_the_person_and_device_that_applied() {
        let (store, id) = seed(safe_fix(true), "host.hooks_silent");
        apply_with(
            &store,
            &FakeExec::default(),
            id,
            &test_actor(),
            20,
            false,
            &|_| Ok(vec![]),
        )
        .await
        .unwrap();
        let s = store.lock().unwrap();
        let ev = s
            .admin_events(&crate::store::AdminEventQuery {
                resource_type: Some("autofix_finding".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(ev.len(), 1);
        assert_eq!((ev[0].actor_person, ev[0].actor_device), (Some(1), Some(2)));
        assert_eq!(ev[0].action, "autofix.apply");
    }

    /// A store with one session `work-1` on h1, stuck on `kind`, and its
    /// finding opened by the real detector.
    fn stuck_seed(kind: &str, plan: impl Fn(i64) -> FixPlan) -> (Mutex<Store>, i64, i64) {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h1", Some("h1")).unwrap();
        let sid = s
            .upsert_session("work-1", "h1", None, None, 0, 0, "running", None)
            .unwrap();
        s.set_session_stuck_for_test(sid, Some(kind), None);
        let mut fix = safe_fix(true);
        fix.plan = plan(sid);
        let id = sync(
            &s,
            &[finding_with(fix, &format!("stuck.{kind}"))],
            Scope::All,
            10,
        )
        .unwrap()
        .opened[0];
        (Mutex::new(s), id, sid)
    }

    fn press_plan(kind: &'static str) -> impl Fn(i64) -> FixPlan {
        move |sid| FixPlan::PressEnter {
            session_id: sid,
            host: "h1".into(),
            tmux_name: "work-1".into(),
            stuck_kind: kind.into(),
        }
    }

    #[tokio::test]
    async fn pressing_enter_clears_a_reconnect_once_the_pane_moves_on() {
        let (store, id, sid) = stuck_seed("reconnect", press_plan("reconnect"));
        let exec = FakeExec::default();
        // The re-scan sees the pane as reconcile left it after the key.
        let row = apply_with(&store, &exec, id, &test_actor(), 20, false, &|s| {
            s.set_session_stuck_for_test(sid, None, None);
            Ok(scan_all(&gather(s, &[], 20)?))
        })
        .await
        .unwrap();
        assert_eq!(exec.presses.load(Ordering::SeqCst), 1);
        assert_eq!(row.state, "cleared");
    }

    #[tokio::test]
    async fn a_trust_prompt_still_showing_after_enter_stays_verifying() {
        let (store, id, _) = stuck_seed("trust_prompt", press_plan("trust_prompt"));
        let exec = FakeExec::default();
        let row = apply_with(&store, &exec, id, &test_actor(), 20, false, &|_| {
            Ok(vec![finding_with(safe_fix(true), "stuck.trust_prompt")])
        })
        .await
        .unwrap();
        assert_eq!(exec.presses.load(Ordering::SeqCst), 1);
        assert_eq!(row.state, "verifying");
    }

    #[tokio::test]
    async fn an_attached_pane_gets_no_keys_and_says_why() {
        let (store, id, _) = stuck_seed("reconnect", press_plan("reconnect"));
        let exec = FakeExec {
            attached: true,
            ..Default::default()
        };
        let row = apply_with(&store, &exec, id, &test_actor(), 20, false, &still_there)
            .await
            .unwrap();
        assert_eq!(row.state, "apply_failed");
        assert!(row.last_error.unwrap().contains("attached"));
    }

    #[tokio::test]
    async fn the_controller_is_never_sent_keys_even_by_a_stale_finding() {
        let (store, id, _) = stuck_seed("reconnect", press_plan("reconnect"));
        store
            .lock()
            .unwrap()
            .set_controller("h1", "work-1")
            .unwrap();
        let exec = FakeExec::default();
        let row = apply_with(&store, &exec, id, &test_actor(), 20, false, &still_there)
            .await
            .unwrap();
        assert_eq!(exec.presses.load(Ordering::SeqCst), 0);
        assert_eq!(row.state, "apply_failed");
        assert!(row.last_error.unwrap().contains("Control"));
    }

    #[tokio::test]
    async fn a_session_no_longer_stuck_gets_no_keys() {
        let (store, id, sid) = stuck_seed("reconnect", press_plan("reconnect"));
        store
            .lock()
            .unwrap()
            .set_session_stuck_for_test(sid, Some("press_enter"), None);
        let exec = FakeExec::default();
        let row = apply_with(&store, &exec, id, &test_actor(), 20, false, &still_there)
            .await
            .unwrap();
        assert_eq!(exec.presses.load(Ordering::SeqCst), 0);
        assert_eq!(row.state, "apply_failed");
    }

    #[tokio::test]
    async fn recreating_an_oom_session_clears_it() {
        let (store, id, sid) = stuck_seed("oom", |sid| FixPlan::Recreate { session_id: sid });
        let exec = FakeExec::default();
        let row = apply_with(&store, &exec, id, &test_actor(), 20, false, &|s| {
            s.set_session_stuck_for_test(sid, None, None);
            Ok(scan_all(&gather(s, &[], 20)?))
        })
        .await
        .unwrap();
        assert_eq!(exec.recreates.load(Ordering::SeqCst), 1);
        assert_eq!(row.state, "cleared");
    }

    #[tokio::test]
    async fn a_working_session_is_not_recreated() {
        let (store, id, sid) = stuck_seed("oom", |sid| FixPlan::Recreate { session_id: sid });
        store
            .lock()
            .unwrap()
            .set_session_stuck_for_test(sid, Some("oom"), Some("working"));
        let exec = FakeExec::default();
        let row = apply_with(&store, &exec, id, &test_actor(), 20, false, &still_there)
            .await
            .unwrap();
        assert_eq!(exec.recreates.load(Ordering::SeqCst), 0);
        assert_eq!(row.state, "apply_failed");
        assert!(row.last_error.unwrap().contains("running a turn"));
    }

    #[tokio::test]
    async fn rewriting_the_mcp_entry_clears_its_finding() {
        let mut fix = safe_fix(true);
        fix.plan = FixPlan::RewriteMcpEntry { host: "h1".into() };
        let (store, id) = seed(fix, "host_mcp_entry.header");
        let exec = FakeExec::default();
        let row = apply_with(&store, &exec, id, &test_actor(), 20, false, &|_| Ok(vec![]))
            .await
            .unwrap();
        assert_eq!(exec.rewrites.load(Ordering::SeqCst), 1);
        assert_eq!(row.state, "cleared");
    }

    #[tokio::test]
    async fn a_rewrite_whose_probe_still_shows_the_problem_stays_verifying() {
        let mut fix = safe_fix(true);
        fix.plan = FixPlan::RewriteMcpEntry { host: "h1".into() };
        let (store, id) = seed(fix.clone(), "host_mcp_entry.token");
        let exec = FakeExec::default();
        let row = apply_with(&store, &exec, id, &test_actor(), 20, false, &|_| {
            Ok(vec![finding_with(fix.clone(), "host_mcp_entry.token")])
        })
        .await
        .unwrap();
        assert_eq!(row.state, "verifying");
    }

    /// The runner auto-applies a missing entry (Safe) and never the token
    /// variant (NeedsApproval), from the real detector over a probe.
    #[tokio::test]
    async fn a_pass_rewrites_a_missing_entry_but_leaves_a_wrong_token_for_a_person() {
        use crate::service::autofix::mcp_entry::{EntryProbe, HostProbe};
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let probes = |cx: &ScanCx| {
            let mut cx = cx.clone();
            cx.mcp_url = Some("https://hub/mcp".into());
            cx.health = vec![
                crate::service::health::HostHealthRow {
                    alias: "h1".into(),
                    reachable: true,
                    ..Default::default()
                },
                crate::service::health::HostHealthRow {
                    alias: "h2".into(),
                    reachable: true,
                    ..Default::default()
                },
            ];
            cx.host_token_shas = vec![("h1".into(), "a".repeat(64)), ("h2".into(), "b".repeat(64))];
            cx.mcp_probes = vec![
                HostProbe {
                    alias: "h1".into(),
                    at: 0,
                    probe: EntryProbe::Missing,
                },
                HostProbe {
                    alias: "h2".into(),
                    at: 0,
                    probe: EntryProbe::Entry {
                        url: Some("https://hub/mcp".into()),
                        has_pane: true,
                        bearer_sha: "a".repeat(64),
                    },
                },
            ];
            crate::service::autofix::mcp_entry::scan(&cx)
        };
        let exec = FakeExec::default();
        let out = run_pass_with(&store, &exec, &[], 10, probes).await.unwrap();
        assert_eq!(out.applied.len(), 1);
        assert_eq!(exec.rewrites.load(Ordering::SeqCst), 1);
        let s = store.lock().unwrap();
        let token = out
            .sync
            .opened
            .iter()
            .map(|id| s.autofix_finding(*id).unwrap().unwrap())
            .find(|r| r.kind == "host_mcp_entry.token")
            .unwrap();
        assert_eq!(
            token.attempts, 0,
            "a wrong token is never rewritten by itself"
        );
    }

    #[test]
    fn a_pinned_agent_whose_updater_did_not_wake_says_both() {
        let e = pinned_but_not_woken(
            "h1",
            "1.2.3",
            &IpcError::new(
                codes::E_INTERNAL,
                "the agent updater was not woken: no trigger",
            ),
        );
        assert!(
            e.message.contains("pinned the agent on h1 to 1.2.3"),
            "{}",
            e.message
        );
        assert!(e.message.contains("no trigger"), "{}", e.message);
        assert!(e.message.contains("next update check"), "{}", e.message);
    }

    #[tokio::test]
    async fn binding_the_operator_clears_its_finding_at_once() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = {
            let s = store.lock().unwrap();
            s.insert_client_token(OPERATOR_CLIENT_NAME, "aa11", "full")
                .unwrap();
            let cx = gather(&s, &[], 10).unwrap();
            sync(&s, &crate::service::autofix::scan_all(&cx), Scope::All, 10)
                .unwrap()
                .opened[0]
        };
        let row = apply_one(&store, &FakeExec::default(), id, &test_actor(), &[], 20)
            .await
            .unwrap();
        assert_eq!(row.state, "cleared");
        let s = store.lock().unwrap();
        let t = s
            .active_client_tokens()
            .unwrap()
            .into_iter()
            .find(|t| t.name == OPERATOR_CLIENT_NAME)
            .unwrap();
        assert_eq!(t.person_id, s.personal_owner_id().unwrap());
    }

    #[tokio::test]
    async fn a_failed_fix_records_the_error() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = {
            let s = store.lock().unwrap();
            let f = Finding {
                kind: "host.hooks_silent".into(),
                subject: "host:h1".into(),
                host_alias: Some("h1".into()),
                severity: Severity::Degraded,
                body: FindingBody {
                    title: "t".into(),
                    explanation: "e".into(),
                    evidence: vec![],
                    fix: Some(safe_fix(true)),
                    action: None,
                },
            };
            sync(&s, &[f], Scope::All, 10).unwrap().opened[0]
        };
        let exec = FakeExec {
            fail: true,
            ..Default::default()
        };
        let row = apply_one(&store, &exec, id, &test_actor(), &[], 20)
            .await
            .unwrap();
        assert_eq!(row.state, "apply_failed");
        assert!(row.last_error.unwrap().contains("boom"));
        assert_eq!(exec.provisions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_finding_without_a_fix_is_refused() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = {
            let s = store.lock().unwrap();
            let f = Finding {
                kind: "host.claude_behind".into(),
                subject: "host:h1".into(),
                host_alias: Some("h1".into()),
                severity: Severity::Info,
                body: FindingBody {
                    title: "t".into(),
                    explanation: "e".into(),
                    evidence: vec![],
                    fix: None,
                    action: None,
                },
            };
            sync(&s, &[f], Scope::All, 10).unwrap().opened[0]
        };
        let err = apply_one(&store, &FakeExec::default(), id, &test_actor(), &[], 20)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
    }

    #[test]
    fn agent_target_names_the_component() {
        assert_eq!(agent_target("h1"), "agent:h1");
    }

    fn output(code: i32, stdout: &str, stderr: &str) -> std::process::Output {
        std::process::Output {
            status: crate::agent::transport::exit_status(code),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn a_poke_counts_only_when_it_printed_poked_and_exited_zero() {
        assert!(poke_result(&output(0, "poked\n", "")).is_ok());
        let e = poke_result(&output(
            0,
            "no update trigger: re-run fleet-agent install --auto-update\n",
            "",
        ))
        .unwrap_err();
        assert!(e.message.contains("no update trigger"), "{}", e.message);
        let e = poke_result(&output(1, "poked", "denied")).unwrap_err();
        assert!(e.message.contains("denied"), "{}", e.message);
    }

    fn finding_with(fix: Fix, kind: &str) -> Finding {
        Finding {
            kind: kind.into(),
            subject: "host:h1".into(),
            host_alias: Some("h1".into()),
            severity: Severity::Degraded,
            body: FindingBody {
                title: "t".into(),
                explanation: "e".into(),
                evidence: vec![],
                fix: Some(fix),
                action: None,
            },
        }
    }

    fn seed(fix: Fix, kind: &str) -> (Mutex<Store>, i64) {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = {
            let s = store.lock().unwrap();
            sync(&s, &[finding_with(fix, kind)], Scope::All, 10)
                .unwrap()
                .opened[0]
        };
        (store, id)
    }

    #[tokio::test]
    async fn a_provision_fix_that_ran_but_still_shows_ends_verifying() {
        let (store, id) = seed(safe_fix(true), "host.hooks_silent");
        let exec = FakeExec::default();
        let row = apply_with(&store, &exec, id, &test_actor(), 20, false, &|_| {
            Ok(vec![finding_with(safe_fix(true), "host.hooks_silent")])
        })
        .await
        .unwrap();
        assert_eq!(exec.provisions.load(Ordering::SeqCst), 1);
        assert_eq!(row.state, "verifying");
    }

    #[tokio::test]
    async fn an_agent_update_fix_runs_the_executor() {
        let mut fix = safe_fix(true);
        fix.plan = FixPlan::UpdateAgent { host: "h1".into() };
        let (store, id) = seed(fix, "host.agent_behind");
        let exec = FakeExec::default();
        apply_one(&store, &exec, id, &test_actor(), &[], 20)
            .await
            .unwrap();
        assert_eq!(exec.updates.load(Ordering::SeqCst), 1);
        assert_eq!(exec.provisions.load(Ordering::SeqCst), 0);
    }

    fn hooks_silent_finding() -> Finding {
        Finding {
            kind: "host.hooks_silent".into(),
            subject: "host:h1".into(),
            host_alias: Some("h1".into()),
            severity: Severity::Degraded,
            body: FindingBody {
                title: "t".into(),
                explanation: "e".into(),
                evidence: vec![],
                fix: Some(safe_fix(true)),
                action: None,
            },
        }
    }

    #[tokio::test]
    async fn a_pass_applies_safe_fixes_and_leaves_approval_ones() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.insert_client_token(OPERATOR_CLIENT_NAME, "aa11", "full")
                .unwrap();
        }
        let exec = FakeExec::default();
        let out = run_pass_with(&store, &exec, &[], 10, |cx| {
            let mut f = scan_all(cx);
            f.push(hooks_silent_finding());
            f
        })
        .await
        .unwrap();
        assert_eq!(out.applied.len(), 1, "only the safe one");
        assert_eq!(exec.provisions.load(Ordering::SeqCst), 1);
        let s = store.lock().unwrap();
        let op = s
            .live_autofix_findings()
            .unwrap()
            .into_iter()
            .find(|r| r.kind == "operator_unbound")
            .unwrap();
        assert_eq!(op.state, "open", "approval fixes never run by themselves");
    }

    #[tokio::test]
    async fn pause_all_stops_autofix() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            crate::service::settings::set(&s, crate::service::settings::AUTOMATION_PAUSED, "true")
                .unwrap();
        }
        let exec = FakeExec::default();
        let out = run_pass_with(&store, &exec, &[], 10, |_| vec![hooks_silent_finding()])
            .await
            .unwrap();
        assert!(out.applied.is_empty());
        assert_eq!(exec.provisions.load(Ordering::SeqCst), 0);
    }

    fn still_there(_: &Store) -> Result<Vec<Finding>, IpcError> {
        Ok(vec![hooks_silent_finding()])
    }

    #[tokio::test]
    async fn an_auto_apply_honours_a_dismissal_made_after_the_pass_decided() {
        let (store, id) = seed(safe_fix(true), "host.hooks_silent");
        {
            let s = store.lock().unwrap();
            assert!(s.dismiss_autofix_finding(id, 10_000).unwrap());
        }
        let exec = FakeExec::default();
        let err = apply_with(&store, &exec, id, &Actor::auto(), 20, true, &still_there)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert_eq!(exec.provisions.load(Ordering::SeqCst), 0);
        let row = store.lock().unwrap().autofix_finding(id).unwrap().unwrap();
        assert_eq!(row.state, "dismissed");
        assert_eq!(row.attempts, 0);
    }

    #[tokio::test]
    async fn an_auto_apply_stops_when_automation_was_paused_mid_pass() {
        let (store, id) = seed(safe_fix(true), "host.hooks_silent");
        {
            let s = store.lock().unwrap();
            crate::service::settings::set(&s, crate::service::settings::AUTOMATION_PAUSED, "true")
                .unwrap();
        }
        let exec = FakeExec::default();
        let err = apply_with(&store, &exec, id, &Actor::auto(), 20, true, &still_there)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert_eq!(exec.provisions.load(Ordering::SeqCst), 0);
        assert_eq!(
            store
                .lock()
                .unwrap()
                .autofix_finding(id)
                .unwrap()
                .unwrap()
                .state,
            "open"
        );
    }

    /// Clears the row (and opens a new episode for the same state) while the
    /// fix runs, then fails, as a tick's sync might during a long SSH apply.
    struct ClearingExec<'a> {
        store: &'a Mutex<Store>,
        id: i64,
    }
    #[async_trait::async_trait]
    impl FixExec for ClearingExec<'_> {
        async fn provision_content(&self, _host: &str) -> Result<(), IpcError> {
            {
                let s = self.store.lock().unwrap();
                s.set_autofix_state(self.id, "cleared", None, 15).unwrap();
                sync(&s, &[hooks_silent_finding()], Scope::All, 16).unwrap();
            }
            Err(IpcError::new(codes::E_SSH, "boom"))
        }
        async fn update_agent(&self, _host: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn rewrite_mcp_entry(&self, _host: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn press_enter(
            &self,
            _host: &str,
            _tmux: &str,
        ) -> Result<PressEnterOutcome, IpcError> {
            Ok(PressEnterOutcome::Sent)
        }
        async fn recreate(&self, _session_id: i64) -> Result<(), IpcError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_row_cleared_while_its_fix_ran_stays_cleared() {
        let (store, id) = seed(safe_fix(true), "host.hooks_silent");
        let exec = ClearingExec { store: &store, id };
        let row = apply_with(&store, &exec, id, &test_actor(), 20, false, &still_there)
            .await
            .unwrap();
        assert_eq!(row.id, id);
        assert_eq!(row.state, "cleared");
        let s = store.lock().unwrap();
        let live = s.live_autofix_findings().unwrap();
        assert_eq!(live.len(), 1, "the new episode is untouched");
        assert_ne!(live[0].id, id);
        assert_eq!(live[0].state, "open");
    }

    #[test]
    fn a_disabled_detector_finds_nothing_in_a_scan() {
        let s = Store::open_in_memory().unwrap();
        s.insert_client_token(OPERATOR_CLIENT_NAME, "aa11", "full")
            .unwrap();
        s.insert_host("h1", Some("h1")).unwrap();
        s.set_host_provisioned("h1", true).unwrap();
        let mut h = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == "h1")
            .unwrap();
        h.reachable = true;
        let mut cx = gather(&s, &[], 10).unwrap();
        cx.hosts = vec![h];
        cx.health = vec![crate::service::health::HostHealthRow {
            alias: "h1".into(),
            reachable: true,
            hooks_silent: true,
            ..Default::default()
        }];
        let on = scan_enabled(&cx, &cfg());
        assert!(on.iter().any(|f| f.kind.starts_with("host.")), "{on:?}");
        let off = scan_enabled(
            &cx,
            &Config {
                detectors_off: vec!["host"],
                ..cfg()
            },
        );
        assert!(!off.is_empty(), "the operator detector still runs");
        assert!(off.iter().all(|f| !f.kind.starts_with("host.")), "{off:?}");
    }

    #[tokio::test]
    async fn a_manual_apply_waits_while_a_fix_is_still_running() {
        let (store, id) = seed(safe_fix(true), "host.hooks_silent");
        {
            let s = store.lock().unwrap();
            assert!(s.mark_autofix_attempt(id, 100).unwrap());
        }
        let exec = FakeExec::default();
        let err = apply_with(
            &store,
            &exec,
            id,
            &test_actor(),
            100 + AUTOFIX_INFLIGHT_SECS - 1,
            false,
            &still_there,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(err.message.contains("already running"), "{}", err.message);
        assert_eq!(exec.provisions.load(Ordering::SeqCst), 0);
        let row = apply_with(
            &store,
            &exec,
            id,
            &test_actor(),
            100 + AUTOFIX_INFLIGHT_SECS,
            false,
            &still_there,
        )
        .await
        .unwrap();
        assert_eq!(exec.provisions.load(Ordering::SeqCst), 1);
        assert_eq!(row.attempts, 2);
    }
}
