//! What a mission's runs can reach, checked before they need it (access
//! review 2026-10-30, F1/F4/F6): a run that starts under a second login, on
//! a host of another organisation, or in a checkout whose remote refuses it,
//! used to find out mid-run, with nobody there to sign in or to say why.
//!
//! * [`org_conflicts`] (F4): a host of the grant whose sessions would sit in
//!   another organisation than the mission's tasks. Every such run is
//!   refused at the start (`orgs::check_cross_org`), so the grant says so
//!   when it is signed.
//! * [`check_once`] (F6): once per grant, where the runs will go (its hosts,
//!   else placement's pick for each repo) and under which login, over SSH:
//!   the login's own `.claude.json` gets the host's MCP servers
//!   (`provision::share_mcp_servers_into`, F1), a server signed in on the
//!   host's login but not on this one is named, and each repo's `origin`
//!   is asked for its `HEAD`. One `access_check` event says what it found;
//!   anything a person must do is one ask card.

use std::collections::BTreeSet;
use std::time::Duration;

use super::{ask_card, Deps};
use crate::ipc_error::{lock, IpcError};
use crate::service::provision;
use crate::shell::quote;
use crate::store::{GrantRow, MissionRow, Store};

/// Most (host, login) pairs one check visits.
pub const MAX_TARGETS: usize = 4;
/// The SSH connect budget of one probe, and its wall clock.
const CONNECT: Duration = Duration::from_secs(15);
const WALL: Duration = Duration::from_secs(60);
/// The tag the repo probe prints before its verdict.
const REPO_TAG: &str = "@@R";
/// The tag the sign-in probe prints before each server's counts.
const OAUTH_TAG: &str = "@@O";

/// The event kind, and the decision id that makes one check per grant.
pub const EVENT: &str = "access_check";

fn decision_id(grant: &GrantRow) -> String {
    format!("access:{}", grant.id)
}

/// An organisation's name, or its id when it has none any more.
fn org_name(s: &Store, id: i64) -> String {
    s.get_org(id)
        .ok()
        .flatten()
        .map_or_else(|| format!("organisation {id}"), |o| o.name)
}

/// F4: for each of `hosts`, the mission's task organisations that no run
/// there could be in, as one sentence per host. A host whose sessions are
/// unassigned, or tasks with no organisation, never conflict (the rule
/// `orgs::check_cross_org` applies at the start).
pub fn org_conflicts(s: &Store, m: &MissionRow, hosts: &[String]) -> Result<Vec<String>, IpcError> {
    let mut task_orgs: BTreeSet<i64> = BTreeSet::new();
    let mut keys: std::collections::BTreeMap<i64, Vec<String>> = Default::default();
    for item in s.mission_items(m.id)? {
        if let Some(o) = s.item_org(item.id)? {
            task_orgs.insert(o);
            let k = keys.entry(o).or_default();
            if k.len() < 3 {
                k.push(item.key.clone().unwrap_or_else(|| item.title.clone()));
            }
        }
    }
    if task_orgs.is_empty() {
        return Ok(Vec::new());
    }
    let projects: Vec<i64> = m.repos.iter().map(|r| r.project_id).collect();
    let mut out = Vec::new();
    for h in hosts {
        let mut session_orgs = Vec::new();
        if projects.is_empty() {
            session_orgs.push(s.host_org(h)?);
        } else {
            for p in &projects {
                session_orgs.push(s.org_for_new_session(h, *p)?);
            }
        }
        for o in &task_orgs {
            let fits = session_orgs
                .iter()
                .any(|so| so.is_none() || *so == Some(*o));
            if fits {
                continue;
            }
            let theirs = session_orgs
                .iter()
                .flatten()
                .map(|so| org_name(s, *so))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ");
            out.push(text::org_conflict(
                h,
                &theirs,
                &org_name(s, *o),
                &keys.get(o).cloned().unwrap_or_default(),
            ));
        }
    }
    Ok(out)
}

/// Where `grant`'s runs go: each repo's pick inside the grant (as a run's
/// start makes it), then the grant's other hosts, each under the grant's
/// login; at most [`MAX_TARGETS`].
pub fn targets(
    s: &Store,
    m: &MissionRow,
    grant: &GrantRow,
    now: i64,
) -> Vec<(String, Option<String>)> {
    let mut projects: Vec<Option<i64>> = m.repos.iter().map(|r| Some(r.project_id)).collect();
    if projects.is_empty() {
        projects.push(None);
    }
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    let mut push = |t: (String, Option<String>)| {
        if !out.contains(&t) && out.len() < MAX_TARGETS {
            out.push(t);
        }
    };
    for p in &projects {
        match super::grant_placement(s, Some(grant), *p) {
            (Some(h), profile) => push((h, profile)),
            (None, profile) => {
                let want = crate::service::placement::Want {
                    profile: profile.clone().map(Some),
                    ..Default::default()
                };
                if let Some(c) = crate::service::placement::suggest_with(
                    s,
                    *p,
                    &want,
                    crate::service::hub::local_host_enabled(),
                    now,
                )
                .ok()
                .and_then(|pl| pl.candidates.into_iter().next())
                {
                    push((c.host_alias, profile.or(c.profile)));
                }
            }
        }
    }
    for h in grant.hosts.iter().flatten() {
        push((h.clone(), grant.profile.clone()));
    }
    out
}

/// What one (host, login) pair answered.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct TargetReport {
    pub host: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// MCP servers copied into the login's `.claude.json` now.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shared: Vec<String>,
    /// MCP servers signed in on the host's own login and not on this one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sign_in: Vec<String>,
    /// Repos whose `origin` did not answer, with why.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub repos_failed: Vec<String>,
    /// What could not be checked at all (the host did not answer).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

impl TargetReport {
    /// The sentences a person must act on; empty when nothing waits.
    pub fn problems(&self) -> Vec<String> {
        let at = text::where_(&self.host, self.profile.as_deref());
        let mut out = Vec::new();
        if !self.sign_in.is_empty() {
            out.push(text::sign_in(
                &at,
                &self.host,
                self.profile.as_deref(),
                &self.sign_in,
            ));
        }
        for r in &self.repos_failed {
            out.push(text::repo_failed(&at, r));
        }
        for e in &self.errors {
            out.push(format!("{at}: {e}"));
        }
        out
    }
}

/// The sign-in probe: for each server, whether the host's own login and
/// `profile` hold an OAuth sign-in for it, as `@@O\t<name>\t<host>\t<profile>`
/// counts. It prints counts, never a token; a Keychain login (macOS) holds
/// none in a file, so it reads 0 on both sides and nothing is said.
pub fn sign_in_script(profile: &str, servers: &[String]) -> String {
    let names = servers
        .iter()
        .map(|n| quote(n))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "h=\"$HOME/.claude/.credentials.json\"; p=\"$HOME/.claude-profiles/\"{}\"/.credentials.json\"; \
         for n in {names}; do a=$(grep -c -F \"\\\"$n|\" \"$h\" 2>/dev/null); \
         b=$(grep -c -F \"\\\"$n|\" \"$p\" 2>/dev/null); \
         printf '{OAUTH_TAG}\\t%s\\t%s\\t%s\\n' \"$n\" \"${{a:-0}}\" \"${{b:-0}}\"; done",
        quote(profile)
    )
}

/// PURE: the servers [`sign_in_script`]'s output says are signed in on the
/// host's login and not on the profile's.
pub fn parse_sign_in(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            if f.next()? != OAUTH_TAG {
                return None;
            }
            let (name, host, profile) = (f.next()?, f.next()?, f.next()?);
            let n = |v: &str| v.trim().parse::<u64>().unwrap_or(0);
            (n(host) > 0 && n(profile) == 0).then(|| name.to_string())
        })
        .collect()
}

/// The repo probe: `origin`'s `HEAD`, without a prompt (no terminal, SSH
/// in batch mode unless the host sets its own command), bounded where the
/// host has `timeout`. `nodir` when the checkout is not at `path` on this
/// host (a run clones or picks its own), `ok`, or `fail` and git's last
/// line.
pub fn repo_script(path: &str) -> String {
    format!(
        "cd {} 2>/dev/null || {{ echo '{REPO_TAG} nodir'; exit 0; }}; \
         T=; command -v timeout >/dev/null 2>&1 && T='timeout 20'; \
         if e=$(GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND=\"${{GIT_SSH_COMMAND:-ssh -o BatchMode=yes}}\" \
         $T git ls-remote -q --exit-code origin HEAD 2>&1 >/dev/null); then echo '{REPO_TAG} ok'; \
         else echo \"{REPO_TAG} fail $(printf '%s' \"$e\" | tail -n 1 | cut -c1-200)\"; fi",
        provision::remote_path(path)
    )
}

/// PURE: [`repo_script`]'s verdict: `None` when the repo answered or is
/// not checked out there, `Some(why)` when its remote refused.
pub fn parse_repo(out: &str) -> Option<String> {
    let line = out.lines().find_map(|l| l.strip_prefix(REPO_TAG))?.trim();
    match line {
        "ok" | "nodir" => None,
        l => Some(
            l.strip_prefix("fail")
                .map(str::trim)
                .filter(|w| !w.is_empty())
                .unwrap_or("its remote did not answer")
                .to_string(),
        ),
    }
}

/// Probe one (host, login) pair.
async fn probe(
    deps: &Deps,
    host: &str,
    profile: Option<&str>,
    repos: &[(String, String)],
) -> TargetReport {
    let ssh = deps.ssh.as_ref();
    let mut r = TargetReport {
        host: host.to_string(),
        profile: profile.map(str::to_string),
        ..Default::default()
    };
    if let Some(p) = profile {
        match provision::share_mcp_servers_into(ssh, host, p).await {
            Ok(names) => r.shared = names,
            Err(e) => r.errors.push(text::share_failed(&e.message)),
        }
        let servers = match provision::read_host_file(ssh, host, provision::CLAUDE_JSON).await {
            Ok(j) => provision::mcp_server_names(&j),
            Err(_) => Vec::new(),
        };
        if !servers.is_empty() {
            if let Ok(out) = crate::ssh::run_shell_bounded(
                ssh,
                host,
                &sign_in_script(p, &servers),
                CONNECT,
                WALL,
            )
            .await
            {
                r.sign_in = parse_sign_in(&String::from_utf8_lossy(&out.stdout));
            }
        }
    }
    for (name, path) in repos {
        match crate::ssh::run_shell_bounded(ssh, host, &repo_script(path), CONNECT, WALL).await {
            Ok(out) => {
                if let Some(why) = parse_repo(&String::from_utf8_lossy(&out.stdout)) {
                    r.repos_failed.push(format!("{name} ({why})"));
                }
            }
            Err(e) => {
                r.errors.push(e.message);
                break;
            }
        }
    }
    r
}

/// F6: check `grant`'s places once (a newer grant checks again), record
/// what was found as one [`EVENT`], and raise one ask card when a person
/// must act. Never an error: a check that cannot run says so in its event
/// and the loop goes on.
pub async fn check_once(deps: &Deps, m: &MissionRow, grant: &GrantRow, now: i64) {
    let decision = decision_id(grant);
    let plan = {
        let Ok(s) = lock(&deps.store) else { return };
        let done = s
            .mission_events_of_kinds(m.id, &[EVENT], 1)
            .ok()
            .and_then(|e| e.into_iter().next())
            .is_some_and(|e| e.decision_id.as_deref() == Some(decision.as_str()));
        if done {
            return;
        }
        // Only hosts fleet provisioned: a probe writes the login's
        // `.claude.json`, which is fleet's to touch on its own hosts alone.
        let targets: Vec<(String, Option<String>)> = targets(&s, m, grant, now)
            .into_iter()
            .filter(|(h, _)| {
                s.get_host_row(h)
                    .ok()
                    .flatten()
                    .is_some_and(|r| r.provisioned)
            })
            .collect();
        let repos: Vec<(String, String)> = m
            .repos
            .iter()
            .filter_map(|r| {
                let p = s.get_project(r.project_id).ok().flatten()?;
                Some((r.name.clone(), p.base_path))
            })
            .collect();
        let hosts: Vec<String> = targets.iter().map(|(h, _)| h.clone()).collect();
        let orgs = org_conflicts(&s, m, &hosts).unwrap_or_default();
        (targets, repos, orgs)
    };
    let (targets, repos, orgs) = plan;
    if targets.is_empty() && orgs.is_empty() {
        return;
    }
    let mut reports = Vec::new();
    for (host, profile) in &targets {
        reports.push(probe(deps, host, profile.as_deref(), &repos).await);
    }
    let mut problems: Vec<String> = orgs.clone();
    for r in &reports {
        problems.extend(r.problems());
    }
    let Ok(s) = lock(&deps.store) else { return };
    let _ = s.record_mission_event(
        m.id,
        &crate::store::NewMissionEvent {
            kind: EVENT,
            actor: "loop",
            decision_id: Some(decision.as_str()),
            payload: Some(serde_json::json!({
                "grant_id": grant.id,
                "targets": reports,
                "org_conflicts": orgs,
                "problems": problems.len(),
            })),
            ..Default::default()
        },
    );
    if !problems.is_empty() {
        let _ = ask_card(
            &s,
            m.id,
            &super::steps::Step {
                kind: "ask".into(),
                item_id: None,
                role: None,
                reason: text::card(&problems),
                context: None,
                auto: false,
            },
        );
    }
}

/// The sentences a person reads.
mod text {
    /// `host` alone, or `host under login p`.
    pub fn where_(host: &str, profile: Option<&str>) -> String {
        match profile {
            Some(p) => format!("{host} under login {p}"),
            None => host.to_string(),
        }
    }

    pub fn org_conflict(host: &str, theirs: &str, task_org: &str, keys: &[String]) -> String {
        let tasks = if keys.is_empty() {
            "its tasks".to_string()
        } else {
            keys.join(", ")
        };
        format!(
            "{host} runs sessions in {theirs}, but {tasks} belong to {task_org}: \
             their runs there will be refused. Grant a host of {task_org}, or assign \
             {host} to it."
        )
    }

    pub fn sign_in(at: &str, host: &str, profile: Option<&str>, servers: &[String]) -> String {
        let how = match profile {
            Some(p) => format!(
                "on {host} run `CLAUDE_CONFIG_DIR=~/.claude-profiles/{p} claude`, then `/mcp`"
            ),
            None => format!("on {host} run `claude`, then `/mcp`"),
        };
        format!(
            "{at}: {} signed in on the host's own login only; sign in once ({how}).",
            servers.join(", ")
        )
    }

    pub fn repo_failed(at: &str, repo: &str) -> String {
        format!(
            "{at}: git cannot reach {repo}; give the host access to that remote \
             (an SSH key or a token for it)."
        )
    }

    pub fn share_failed(why: &str) -> String {
        format!("the host's MCP servers could not be shared with this login: {why}")
    }

    /// One ask card's question for every problem the check found.
    pub fn card(problems: &[String]) -> String {
        let mut s =
            String::from("Before its runs start, this mission cannot reach everything it needs:");
        for p in problems.iter().take(8) {
            s.push_str("\n- ");
            s.push_str(p);
        }
        if problems.len() > 8 {
            s.push_str(&format!(
                "\n- and {} more (see the access check in the log)",
                problems.len() - 8
            ));
        }
        s
    }
}

#[cfg(test)]
mod tests;
