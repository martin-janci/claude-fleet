//! What a fleet-own action does once a person approved it (`work_task`,
//! `fix_session`, `setting`, `session_recover`; `actions::FleetOp`). Each
//! goes through the same service a person's own button uses, so it is held
//! to the same checks: a setting through the settings review (an agent may
//! not touch a key whose `AiPolicy` is `Never`), a session through
//! `restore_host_sessions` / `restart_session`, a start through
//! `start_work`.

use super::actions::{FixSessionPayload, FleetOp, RecoverPayload, SettingPayload, WorkTaskPayload};
use crate::ipc_error::lock;
use crate::service::orgs::OrgScope;
use crate::service::settings::Actor;
use crate::service::work::WorkLinkArgs;
use crate::ssh::SshClient;
use crate::store::{Store, TreeEntry};
use std::sync::{Arc, Mutex};

/// Who the audit trail and the Work view name as proposing.
pub const PROPOSER: &str = "fleet agent";

/// Carry out `op`; answers one line on what it did.
pub async fn run(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    op: FleetOp,
) -> Result<String, String> {
    match op {
        FleetOp::WorkTask(p) => work_task(store, &p).map(|(id, _)| format!("created task {id}")),
        FleetOp::FixSession(p) => fix_session(store, ssh, &p).await,
        FleetOp::Setting(p) => setting(store, &p),
        FleetOp::Recover(p) => recover(store, ssh, &p).await,
    }
}

/// A Work task with its proposed subtasks (ticked on its card, as any
/// `propose_tree`). Answers the task's id and key.
pub fn work_task(
    store: &Mutex<Store>,
    p: &WorkTaskPayload,
) -> Result<(i64, Option<String>), String> {
    let args = WorkLinkArgs {
        action: "create".into(),
        title: Some(p.title.clone()),
        notes: Some(p.notes.clone()).filter(|n| !n.is_empty()),
        ..Default::default()
    };
    let row = crate::service::work::local::create_task(&args, store, &OrgScope::All)
        .map_err(|e| e.message)?;
    if !p.steps.is_empty() {
        let entries: Vec<TreeEntry> = p
            .steps
            .iter()
            .map(|t| TreeEntry {
                title: t.clone(),
                notes: None,
                why: None,
                depends_on: Vec::new(),
            })
            .collect();
        lock(store)
            .map_err(|e| e.message)?
            .propose_tree(row.id, &entries, PROPOSER)
            .map_err(|e| {
                format!(
                    "task {} created; its subtasks were not: {}",
                    row.id, e.message
                )
            })?;
    }
    Ok((row.id, row.key))
}

/// The project a `fix_session` names: its id, `owner/repo` or repo name.
fn project_id(store: &Mutex<Store>, name: &str) -> Result<i64, String> {
    let s = lock(store).map_err(|e| e.message)?;
    let projects = s.list_projects().map_err(|e| e.to_string())?;
    let want = name.trim().to_lowercase();
    let found: Vec<i64> = projects
        .iter()
        .filter(|p| {
            p.id.to_string() == want
                || format!("{}/{}", p.owner, p.repo).to_lowercase() == want
                || p.repo.to_lowercase() == want
        })
        .map(|p| p.id)
        .collect();
    match found.as_slice() {
        [id] => Ok(*id),
        [] => Err(format!("no project {name}")),
        _ => Err(format!(
            "{name} names more than one project: give owner/repo"
        )),
    }
}

async fn fix_session(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    p: &FixSessionPayload,
) -> Result<String, String> {
    let project = project_id(store, &p.project)?;
    if let Some(h) = &p.host {
        crate::validate::host_alias(h).map_err(|e| e.message)?;
    }
    let (item, _) = work_task(
        store,
        &WorkTaskPayload {
            action: p.action.clone(),
            title: p.title.clone(),
            notes: p.prompt.clone(),
            steps: Vec::new(),
            body: String::new(),
        },
    )?;
    let args = crate::service::trackers::tickets::StartArgs {
        item_id: Some(item),
        project_id: Some(project),
        host_alias: p.host.clone(),
        with_brief: true,
        brief: Some(p.prompt.clone()),
        origin: Some(crate::store::SessionOrigin::person(None)),
        ..Default::default()
    };
    let row = crate::service::trackers::tickets::start_work(
        store,
        ssh,
        &crate::cancel::CancellationRegistry::new(),
        &args,
        &crate::service::view_scope::ViewScope::internal(),
        &crate::service::trackers::default_net(),
    )
    .await
    .map_err(|e| {
        format!(
            "task {item} created; the session did not start: {}",
            e.message
        )
    })?;
    Ok(format!("started {} on {}", row.tmux_name, row.host_alias))
}

fn setting(store: &Mutex<Store>, p: &SettingPayload) -> Result<String, String> {
    let s = lock(store).map_err(|e| e.message)?;
    let why = Some(p.why.as_str()).filter(|w| !w.is_empty());
    let proposal =
        crate::service::settings_review::propose(&s, &p.key, &p.value, why, Actor::Agent(PROPOSER))
            .map_err(|e| e.message)?;
    let done = crate::service::settings_review::decide_as(&s, &[proposal.id], &[], Actor::Person)
        .map_err(|e| e.message)?;
    match done.failed.first() {
        Some(f) => Err(format!("{}: {}", p.key, f.error)),
        None => Ok(format!("{} = {}", p.key, p.value)),
    }
}

/// PURE: what restoring session `id` came to. `restore_host_sessions`
/// answers `Ok` for the batch even when this session's restore failed or
/// the plan skipped it (a controller row, no Claude id), so the action
/// fails on either rather than reading `executed`.
fn restore_line(
    report: &crate::service::sessions::RestoreReport,
    id: i64,
    name: &str,
) -> Result<String, String> {
    if let Some(o) = report.results.iter().find(|o| o.session_id == id) {
        return match (o.ok, &o.error) {
            (true, _) => Ok(format!("restored {name} on {}", report.host_alias)),
            (false, Some(e)) => Err(format!("{name} was not restored: {e}")),
            (false, None) => Err(format!("{name} was not restored")),
        };
    }
    match report.plan.iter().find(|e| e.session_id == id) {
        Some(e) if e.action == "skip" => Err(format!(
            "{name} was not restored: {}",
            e.reason.as_deref().unwrap_or("the restore plan skipped it")
        )),
        _ => Err(format!("{name} was not restored: it is no longer lost")),
    }
}

async fn recover(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    p: &RecoverPayload,
) -> Result<String, String> {
    let rows = lock(store)
        .map_err(|e| e.message)?
        .find_sessions_by_tmux_name(&p.session, p.host.as_deref())
        .map_err(|e| e.to_string())?;
    let row = match rows.as_slice() {
        [r] => r.clone(),
        [] => return Err(format!("no session {}", p.session)),
        _ => return Err(format!("{} is on more than one host: name it", p.session)),
    };
    if row.lost_at.is_some() {
        let report = crate::service::sessions::restore_host_sessions(
            crate::service::sessions::RestoreHostSessionsArgs {
                host_alias: row.host_alias.clone(),
                dry_run: false,
                session_ids: Some(vec![row.id]),
            },
            store,
            ssh,
        )
        .await
        .map_err(|e| e.message)?;
        return restore_line(&report, row.id, &row.tmux_name);
    }
    let r = crate::service::sessions::restart_session(
        crate::service::sessions::RestartSessionArgs {
            host_alias: row.host_alias.clone(),
            name: row.tmux_name.clone(),
            force: false,
            profile: None,
            model: None,
            effort: None,
        },
        store,
        ssh,
    )
    .await
    .map_err(|e| e.message)?;
    Ok(format!("restarted {} on {}", r.tmux_name, r.host_alias))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::settings::{self, AiPolicy, SPECS};

    #[test]
    fn a_task_is_created_with_its_subtasks_proposed() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let p = WorkTaskPayload {
            action: "task".into(),
            title: "Flaky reconnect".into(),
            notes: "Seen 7 times".into(),
            steps: vec!["Reproduce".into(), "Fix the backoff".into()],
            body: String::new(),
        };
        let (id, _) = work_task(&store, &p).unwrap();
        let s = store.lock().unwrap();
        let item = s.get_work_item(id).unwrap().unwrap();
        assert_eq!(item.title, "Flaky reconnect");
        // The subtasks wait on the task's card, as any proposed tree.
        let first = s.get_work_item(id + 1).unwrap().unwrap();
        assert_eq!(first.title, "Reproduce");
    }

    #[test]
    fn a_setting_goes_through_review_and_a_closed_key_never() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let open = SPECS
            .iter()
            .find(|sp| sp.ai != AiPolicy::Never && sp.owned_by.is_none() && sp.default == "true")
            .unwrap();
        let p = SettingPayload {
            action: "tune".into(),
            key: open.key.into(),
            value: "false".into(),
            was: String::new(),
            why: "noisy".into(),
            body: String::new(),
        };
        setting(&store, &p).unwrap();
        let s = store.lock().unwrap();
        assert_eq!(
            settings::effective_value(&s, open.key).as_deref(),
            Some("false")
        );
        drop(s);
        let closed = SPECS
            .iter()
            .find(|sp| sp.ai == AiPolicy::Never && sp.owned_by.is_none())
            .unwrap();
        let p = SettingPayload {
            key: closed.key.into(),
            value: closed.default.into(),
            ..p
        };
        assert!(setting(&store, &p).is_err());
    }

    #[test]
    fn a_failed_or_skipped_restore_fails_the_action() {
        use crate::service::sessions::{RestoreOutcome, RestorePlanEntry, RestoreReport};
        let entry = |action: &str, reason: Option<&str>| RestorePlanEntry {
            session_id: 7,
            tmux_name: Some("api-fix".into()),
            cwd: None,
            claude_session_id: None,
            friendly_name: None,
            action: action.into(),
            reason: reason.map(Into::into),
        };
        let outcome = |ok: bool, error: Option<&str>| RestoreOutcome {
            session_id: 7,
            tmux_name: "api-fix".into(),
            ok,
            error: error.map(Into::into),
            code: None,
        };
        let report = |plan, results| RestoreReport {
            host_alias: "mercury".into(),
            dry_run: false,
            plan: vec![plan],
            results,
        };
        assert_eq!(
            restore_line(
                &report(entry("restore", None), vec![outcome(true, None)]),
                7,
                "api-fix"
            ),
            Ok("restored api-fix on mercury".into())
        );
        let failed = report(
            entry("restore", None),
            vec![outcome(false, Some("tmux died"))],
        );
        assert_eq!(
            restore_line(&failed, 7, "api-fix"),
            Err("api-fix was not restored: tmux died".into())
        );
        let skipped = report(entry("skip", Some("no claude session id")), vec![]);
        assert_eq!(
            restore_line(&skipped, 7, "api-fix"),
            Err("api-fix was not restored: no claude session id".into())
        );
        assert!(restore_line(&skipped, 8, "other").is_err());
    }

    #[test]
    fn a_fix_names_one_project() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.upsert_project("martin", "claude-fleet", "/p/a").unwrap();
            s.upsert_project("other", "claude-fleet", "/p/b").unwrap();
            s.upsert_project("martin", "fleet-mobile", "/p/c").unwrap();
        }
        assert!(project_id(&store, "fleet-mobile").is_ok());
        assert!(project_id(&store, "Martin/Claude-Fleet").is_ok());
        assert!(project_id(&store, "claude-fleet")
            .unwrap_err()
            .contains("more than one"));
        assert!(project_id(&store, "nope").is_err());
    }
}
