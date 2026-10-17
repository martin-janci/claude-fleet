//! Eligibility, execution and the apply → re-scan step.

use super::sync::{sync, Scope};
use super::{detector_of, scan_all, Finding, FindingBody, Fix, FixPlan, Safety, ScanCx};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::settings;
use crate::ssh::SshExec;
use crate::store::{AdminEvent, AutofixRow, Store};
use std::sync::Mutex;

/// Minimum gap between two automatic attempts on one episode.
pub const AUTOFIX_RETRY_SECS: i64 = 600;

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

pub fn auto_eligible(row: &AutofixRow, fix: &Fix, cfg: &Config, now: i64) -> bool {
    fix.safety == Safety::Safe
        && fix.auto
        && cfg.auto_safe
        && cfg.detector_on(&row.kind)
        && matches!(row.state.as_str(), "open" | "apply_failed")
        && row.attempts < cfg.max_attempts
        && row
            .last_attempt_at
            .is_none_or(|t| now - t >= AUTOFIX_RETRY_SECS)
}

#[async_trait::async_trait]
pub trait FixExec: Send + Sync {
    async fn provision_content(&self, host: &str) -> Result<(), IpcError>;
    async fn update_agent(&self, host: &str) -> Result<(), IpcError>;
}

pub struct SshFixExec<'a> {
    pub store: &'a Mutex<Store>,
    pub ssh: &'a dyn SshExec,
}

#[async_trait::async_trait]
impl FixExec for SshFixExec<'_> {
    async fn provision_content(&self, host: &str) -> Result<(), IpcError> {
        let base = {
            let s = lock(self.store)?;
            crate::service::hub::HubBase::read(&s)?
        };
        crate::service::provision::provision_content_only(self.store, self.ssh, host, &base)
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
            let out = self
                .ssh
                .run(
                    alias,
                    &["bash", "-c", &script],
                    std::time::Duration::from_secs(10),
                )
                .await?;
            poke_result(&out)?;
        }
        Ok(())
    }
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
    Ok(ScanCx {
        now,
        owner: s.personal_owner_id()?,
        client_tokens: s.active_client_tokens()?,
        hosts,
        health: rows,
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
    }
}

/// Run one finding's fix, re-scan its detector, and return the row as it now
/// stands: `cleared` when the state is gone, `verifying` when it still shows
/// (the next passes decide), `apply_failed` when the fix itself errored.
pub async fn apply_one(
    store: &Mutex<Store>,
    exec: &dyn FixExec,
    id: i64,
    actor: &str,
    live_agents: &[(String, String)],
    now: i64,
) -> Result<AutofixRow, IpcError> {
    apply_with(store, exec, id, actor, now, &|s| {
        Ok(scan_all(&gather(s, live_agents, now)?))
    })
    .await
}

/// `apply_one` with the re-scan supplied, so tests can say what it finds.
async fn apply_with(
    store: &Mutex<Store>,
    exec: &dyn FixExec,
    id: i64,
    actor: &str,
    now: i64,
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
        s.mark_autofix_attempt(id, now)?;
        (row, fix)
    };
    let result = execute(store, exec, &fix.plan).await;
    let s = lock(store)?;
    s.record_admin_event_or_log(&AdminEvent {
        actor_person: None,
        actor_device: None,
        action: "autofix.apply",
        resource_type: "autofix_finding",
        resource_id: &id.to_string(),
        representation: serde_json::json!({
            "kind": row.kind,
            "subject": row.subject,
            "actor": actor,
            "plan": fix.plan,
            "ok": result.is_ok(),
            "error": result.as_ref().err().map(|e| e.message.clone()),
        }),
    });
    match result {
        Err(e) => {
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
    // The guard lives only in this block: never held across an await.
    let (synced, due) = {
        let s = lock(store)?;
        let cfg = Config::read(&s);
        let cx = gather(&s, live_agents, now)?;
        let found: Vec<_> = scan(&cx)
            .into_iter()
            .filter(|f| cfg.detector_on(&f.kind))
            .collect();
        let synced = sync(&s, &found, Scope::All, now)?;
        let due: Vec<i64> = if crate::service::loops::paused(&s) {
            Vec::new()
        } else {
            s.live_autofix_findings()?
                .into_iter()
                .filter(|r| {
                    body_of(r)
                        .ok()
                        .and_then(|b| b.fix)
                        .is_some_and(|f| auto_eligible(r, &f, &cfg, now))
                })
                .map(|r| r.id)
                .collect()
        };
        (synced, due)
    };
    let mut applied = Vec::new();
    for id in due {
        match apply_one(store, exec, id, "auto", live_agents, now).await {
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

    #[derive(Default)]
    struct FakeExec {
        provisions: AtomicUsize,
        updates: AtomicUsize,
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

    #[test]
    fn only_safe_auto_fixes_within_budget_are_eligible() {
        assert!(auto_eligible(
            &open_row(0, None),
            &safe_fix(true),
            &cfg(),
            1000
        ));
        assert!(
            !auto_eligible(&open_row(0, None), &safe_fix(false), &cfg(), 1000),
            "auto: false"
        );
        let mut approval = safe_fix(true);
        approval.safety = Safety::NeedsApproval;
        assert!(
            !auto_eligible(&open_row(0, None), &approval, &cfg(), 1000),
            "never auto"
        );
        assert!(!auto_eligible(
            &open_row(0, None),
            &safe_fix(true),
            &Config {
                auto_safe: false,
                ..cfg()
            },
            1000
        ));
        assert!(!auto_eligible(
            &open_row(0, None),
            &safe_fix(true),
            &Config {
                detectors_off: vec!["host"],
                ..cfg()
            },
            1000
        ));
        assert!(
            !auto_eligible(&open_row(3, Some(0)), &safe_fix(true), &cfg(), 100_000),
            "budget spent"
        );
        assert!(
            !auto_eligible(&open_row(1, Some(900)), &safe_fix(true), &cfg(), 1000),
            "retry backoff"
        );
        let mut dismissed = open_row(0, None);
        dismissed.state = "dismissed".into();
        assert!(!auto_eligible(&dismissed, &safe_fix(true), &cfg(), 1000));
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
        let row = apply_one(&store, &FakeExec::default(), id, "test", &[], 20)
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
                },
            };
            sync(&s, &[f], Scope::All, 10).unwrap().opened[0]
        };
        let exec = FakeExec {
            fail: true,
            ..Default::default()
        };
        let row = apply_one(&store, &exec, id, "test", &[], 20).await.unwrap();
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
                },
            };
            sync(&s, &[f], Scope::All, 10).unwrap().opened[0]
        };
        let err = apply_one(&store, &FakeExec::default(), id, "test", &[], 20)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
    }

    #[test]
    fn agent_target_names_the_component() {
        assert_eq!(agent_target("h1"), "agent:h1");
    }

    fn output(code: i32, stdout: &str, stderr: &str) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
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
        let row = apply_with(&store, &exec, id, "test", 20, &|_| {
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
        apply_one(&store, &exec, id, "test", &[], 20).await.unwrap();
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
}
