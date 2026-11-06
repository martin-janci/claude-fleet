//! Keep an account off a project (docs/accounts.md, *Keep an account off a
//! project*): a person forbids a Claude account for some projects
//! (`account_project_blocks`, migration 217) or for every project of an org
//! (`account_org_blocks`, migration 218; a project's org is its owner, repo
//! or path rule, `Store::project_org`), and no work in one of them runs on a
//! login on that account.
//!
//! **Where it holds.** Every place fleet picks or takes a login for work in
//! a project:
//! * a start (`sessions::new_session`, every caller: the desktop, MCP and
//!   hub `new_session`, a ticket start, a resume, a routine run, a mission
//!   run, `dispatch_task`) is refused with [`codes::E_ACCOUNT_FORBIDDEN`]
//!   when the login it would run under is on a forbidden account
//!   ([`refuse_forbidden`]), naming an allowed login on the host when there
//!   is one;
//! * a relaunch onto another login (`restart_session { profile }`, Switch
//!   account) is refused the same way;
//! * a login fleet picks gives way to an allowed one: placement leaves a
//!   forbidden login out and says why, a ticket start or a resume that
//!   names no login takes the allowed login on the host with the most
//!   headroom (`account_limits::login_with_headroom_for`), a routine whose
//!   own login is forbidden moves to one, and the automatic account switch
//!   never moves a session onto a forbidden account.
//!
//! **What it cannot see.** A login is matched to its account by what the
//! host reported (`hosts.account_uuid`, `hosts.claude_profiles`). A profile
//! not logged in yet has no account, so a start under it is not refused; the
//! session asks for `/login` in its pane, and once the host reports the
//! account, the next start, switch or resume under it is. A session already
//! running on an account when it is forbidden keeps running: forbidding
//! changes where work goes next, never stops work.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::account_limits;
use crate::store::{AccountRow, Store};

/// The accounts work in `project_id` may not run on. Empty for no project
/// (Control's own work, a start outside any project) and for a project
/// with none.
pub fn forbidden_accounts(s: &Store, project_id: Option<i64>) -> Result<Vec<String>, IpcError> {
    match project_id {
        Some(p) => s.project_forbidden_accounts(p),
        None => Ok(Vec::new()),
    }
}

/// The project's name for a refusal: `owner/repo`, or `project <id>`.
fn project_name(s: &Store, project_id: i64) -> String {
    s.get_project(project_id).ok().flatten().map_or_else(
        || format!("project {project_id}"),
        |p| format!("{}/{}", p.owner, p.repo),
    )
}

/// An account's name for a refusal: its nickname, email or uuid.
fn account_name(s: &Store, uuid: &str) -> String {
    s.get_account_by_uuid(uuid)
        .ok()
        .flatten()
        .and_then(|a| {
            a.nickname
                .filter(|n| !n.trim().is_empty())
                .or(a.email)
                .or(a.display_name)
        })
        .unwrap_or_else(|| uuid.to_string())
}

/// " (all of SalesTwins)" when the account is forbidden for the project
/// through its org rather than by name; else empty.
fn through_org(s: &Store, uuid: &str, project_id: i64) -> String {
    let Some(org) = s.project_org(project_id).ok().flatten() else {
        return String::new();
    };
    let by_org = s.get_account_by_uuid(uuid).ok().flatten().is_some_and(|a| {
        a.forbidden_org_ids.contains(&org) && !a.forbidden_project_ids.contains(&project_id)
    });
    match (by_org, s.get_org(org).ok().flatten()) {
        (true, Some(o)) => format!(" (all of {})", o.name),
        _ => String::new(),
    }
}

/// "profile work on mac" / "mac's own login".
fn login_words(host_alias: &str, profile: Option<&str>) -> String {
    match profile {
        Some(p) => format!("profile {p} on {host_alias}"),
        None => format!("{host_alias}'s own login"),
    }
}

/// Refuse work in `project_id` under the login `profile` on `host_alias`
/// (`None` = the host's own) when the project forbids its account:
/// `E_ACCOUNT_FORBIDDEN`, naming the allowed login on the host with the most
/// headroom when there is one (`details.suggested_profile`, `""` for the
/// host's own). Nothing is refused without a project, for a host fleet does
/// not know, or for a login not on a known account. No person can start
/// anyway: the block is the person's own rule, lifted on the Accounts page.
pub fn refuse_forbidden(
    s: &Store,
    project_id: Option<i64>,
    host_alias: &str,
    profile: Option<&str>,
    now: i64,
) -> Result<(), IpcError> {
    let Some(project_id) = project_id else {
        return Ok(());
    };
    let forbidden = s.project_forbidden_accounts(project_id)?;
    if forbidden.is_empty() {
        return Ok(());
    }
    let profile = profile.map(str::trim).filter(|p| !p.is_empty());
    let Some(login) = account_limits::login_account(s, host_alias, profile, now)? else {
        return Ok(());
    };
    let account = &login.login.account_uuid;
    if !forbidden.contains(account) {
        return Ok(());
    }
    let instead =
        account_limits::login_with_headroom_for(s, Some(project_id), host_alias, profile, now)?;
    // `login_with_headroom_for` keeps `profile` when no allowed login is
    // left on the host.
    let suggestion = (instead.as_deref() != profile).then_some(instead);
    let tail = match &suggestion {
        Some(p) => format!(
            "; start under {} instead (profile {})",
            login_words(host_alias, p.as_deref()),
            match p {
                Some(p) => format!("\"{p}\""),
                None => "unset".to_string(),
            }
        ),
        None => format!(
            "; no other login on {host_alias} is on an allowed account: start on another host, or allow the account again on the Accounts page"
        ),
    };
    Err(IpcError::new(
        codes::E_ACCOUNT_FORBIDDEN,
        format!(
            "{} is on {}, which is forbidden for {}{}{tail}",
            login_words(host_alias, profile),
            account_name(s, account),
            project_name(s, project_id),
            through_org(s, account, project_id),
        ),
    )
    .with_details(serde_json::json!({
        "account_uuid": account,
        "project_id": project_id,
        "host_alias": host_alias,
        "suggested_profile": suggestion.map(|p| p.unwrap_or_default()),
    })))
}

/// [`refuse_forbidden`]'s refusal as a reason, for automation that skips
/// rather than fails (a routine's run): `None` when the login is allowed.
pub fn forbidden_reason(
    s: &Store,
    project_id: Option<i64>,
    host_alias: &str,
    profile: Option<&str>,
    now: i64,
) -> Result<Option<String>, IpcError> {
    match refuse_forbidden(s, project_id, host_alias, profile, now) {
        Ok(()) => Ok(None),
        Err(e) if e.code == codes::E_ACCOUNT_FORBIDDEN => Ok(Some(e.message)),
        Err(e) => Err(e),
    }
}

/// `set_account_projects`: replace the projects and/or orgs an account is
/// forbidden for.
#[derive(Debug, Clone, Deserialize, Serialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "SetAccountProjectsParams")]
pub struct SetAccountProjectsArgs {
    /// Account uuid (list_accounts).
    pub uuid: String,
    /// Project ids it may not run; replaces the list, empty clears, omitted
    /// keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forbidden_project_ids: Option<Vec<i64>>,
    /// Org ids (list_orgs): all their projects, later ones too; same rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forbidden_org_ids: Option<Vec<i64>>,
}

/// [`SetAccountProjectsArgs`] applied: the account's row, with its new lists.
pub fn set_account_projects(
    args: SetAccountProjectsArgs,
    store: &Mutex<Store>,
) -> Result<AccountRow, IpcError> {
    let uuid = args.uuid.trim();
    if uuid.is_empty() {
        return Err(IpcError::new(codes::E_INVALID, "uuid is required"));
    }
    lock(store)?.set_account_blocks(
        uuid,
        args.forbidden_project_ids.as_deref(),
        args.forbidden_org_ids.as_deref(),
    )
}

#[cfg(test)]
#[path = "project_accounts_tests.rs"]
mod tests;
