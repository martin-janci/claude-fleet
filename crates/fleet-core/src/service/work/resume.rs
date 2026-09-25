//! Resume past work (work graph M2.4): `work { action: resume_plan }` says
//! what a resume of a work key would do and which modes are possible (and
//! why not); `work_link { action: resume }` does it.
//!
//! Modes:
//! * `last`  — continue the last Claude conversation (`claude --resume`) on
//!   its host, in its worktree (recreated from the branch when a safe kill
//!   removed it).
//! * `brief` — a fresh session there, with the handover brief queued for its
//!   first hook and a short start prompt typed only into a ready REPL.
//! * `fresh` — a fresh session there, nothing else.
//!
//! Resume never destroys or duplicates: a key with a live session offers
//! *Jump* instead, a conversation some session on the host still holds is
//! never resumed twice, and nothing is deleted.

use super::handover;
use crate::cancel::CancellationRegistry;
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::OrgScope;
use crate::service::pane_intel::StuckKind;
use crate::service::sessions::{self, NewSessionArgs};
use crate::ssh::{SshClient, SshExec};
use crate::store::{SessionRow, Store, WorkLinkRow};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The resume modes, in the order the UI offers them.
pub const RESUME_MODES: &[&str] = &["last", "brief", "fresh"];

/// Whether one mode is possible for the chosen candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeMode {
    pub mode: String,
    pub ok: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

/// A live session already doing the work: the UI offers *Jump*.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveWork {
    pub session_id: i64,
    pub host_alias: String,
    pub tmux_name: String,
    #[serde(default)]
    pub friendly_name: Option<String>,
}

/// One ended link a resume could start from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeCandidate {
    pub link_id: i64,
    #[serde(default)]
    pub ended_at: Option<i64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub host_alias: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub worktree: Option<String>,
    #[serde(default)]
    pub pr_url: Option<String>,
    #[serde(default)]
    pub conversations: usize,
    #[serde(default)]
    pub last_claude_session_id: Option<String>,
    #[serde(default = "yes")]
    pub resumable: bool,
}

fn yes() -> bool {
    true
}

/// What a resume of `key` would do. `modes` is required on the wire on
/// purpose: an older hub answers `work { action: resume_plan }` with its
/// plain link list, which then fails to parse here instead of passing for an
/// empty plan — the UI hides Resume and says the hub is too old.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumePlan {
    pub key: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub live: Vec<LiveWork>,
    /// Ended links, newest first.
    #[serde(default)]
    pub candidates: Vec<ResumeCandidate>,
    /// The candidate the modes are about.
    #[serde(default)]
    pub link_id: Option<i64>,
    /// Where the session would land.
    #[serde(default)]
    pub host_alias: Option<String>,
    #[serde(default)]
    pub project_id: Option<i64>,
    #[serde(default)]
    pub branch: Option<String>,
    /// Worktree name (the session's cwd), `None` for the project root.
    #[serde(default)]
    pub worktree: Option<String>,
    /// The worktree is known on the host (else a resume recreates it from
    /// `branch`).
    #[serde(default)]
    pub worktree_present: bool,
    pub modes: Vec<ResumeMode>,
    /// Reachable hosts, for a `host_alias` override.
    #[serde(default)]
    pub hosts: Vec<String>,
    /// The handover brief a `brief` resume would queue (only when asked).
    #[serde(default)]
    pub brief: Option<String>,
    /// Things the plan could not check, none of which blocks a mode (work
    /// graph M11.2: a transcript probe that timed out or failed).
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// `work_link { action: resume }`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResumeArgs {
    pub key: String,
    /// `last` | `brief` | `fresh`.
    pub mode: String,
    #[serde(default)]
    pub link_id: Option<i64>,
    #[serde(default)]
    pub host_alias: Option<String>,
    /// The (edited) brief for `brief`; the built one when absent.
    #[serde(default)]
    pub brief: Option<String>,
    /// Resume another org's work onto a session of this org anyway (work
    /// graph M5's integrity rule, as for a link).
    #[serde(default)]
    pub force_cross_org: bool,
}

fn mode(mode: &str, why_not: Option<String>) -> ResumeMode {
    ResumeMode {
        mode: mode.into(),
        ok: why_not.is_none(),
        reason: why_not,
    }
}

fn candidate(l: &WorkLinkRow) -> ResumeCandidate {
    let ids: Vec<String> = l
        .snap_claude_ids
        .as_deref()
        .and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default();
    ResumeCandidate {
        link_id: l.id,
        ended_at: l.ended_at,
        name: l.snap_name.clone(),
        host_alias: l.snap_host.clone(),
        branch: l.snap_branch.clone(),
        worktree: l
            .snap_worktree
            .clone()
            .filter(|w| !w.is_empty() && w != "main"),
        pr_url: l.snap_pr_url.clone(),
        conversations: ids.len(),
        last_claude_session_id: ids.last().cloned(),
        resumable: l.resumable,
    }
}

fn host_reachable(s: &Store, host: &str) -> Result<Option<bool>, IpcError> {
    if host == crate::service::projects::LOCAL_HOST {
        return Ok(Some(
            crate::service::hub::ensure_local_allowed(host).is_ok(),
        ));
    }
    Ok(s.get_host_row(host)?.map(|h| h.reachable))
}

fn reachable_hosts(s: &Store) -> Result<Vec<String>, IpcError> {
    let mut hosts: Vec<String> = s
        .list_hosts()?
        .into_iter()
        .filter(|h| h.reachable)
        .map(|h| h.alias)
        .collect();
    if crate::service::hub::ensure_local_allowed(crate::service::projects::LOCAL_HOST).is_ok()
        && !hosts.iter().any(|h| h == "local")
    {
        hosts.insert(0, "local".into());
    }
    Ok(hosts)
}

/// PURE of the network: what a resume of `key` would do, from the store.
/// `link_id` picks a candidate (default: the newest ended link);
/// `host_alias` overrides the snapshot's host.
pub fn plan_resume(
    s: &Store,
    key: &str,
    link_id: Option<i64>,
    host_alias: Option<&str>,
    scope: &OrgScope,
) -> Result<ResumePlan, IpcError> {
    crate::service::orgs::require_key(s, scope, key)?;
    let key = crate::store::normalize_work_ref(key)?;
    let item = match s.work_item_by_key(&key)? {
        Some(i) if scope.sees_org(s.item_org(i.id)?) => Some(i),
        _ => None,
    };
    let mut live_links = s.live_work_sessions_for_key(&key)?;
    for (l, _) in live_links.iter_mut() {
        l.org_id = s.link_org(l)?;
    }
    let live: Vec<LiveWork> = live_links
        .into_iter()
        .filter(|(l, _)| scope.sees_link(l))
        .map(|(_, r)| LiveWork {
            session_id: r.id,
            host_alias: r.host_alias,
            tmux_name: r.tmux_name,
            friendly_name: r.friendly_name,
        })
        .collect();
    let mut ended = s.ended_work_links_for_key(&key)?;
    crate::service::orgs::scope_links(s, scope, &mut ended)?;
    let candidates: Vec<ResumeCandidate> = ended.iter().map(candidate).collect();
    let chosen = match link_id {
        Some(id) => Some(
            ended
                .iter()
                .find(|l| l.id == id)
                .ok_or_else(|| {
                    IpcError::new(
                        codes::E_NOTFOUND,
                        format!("{key} has no ended work link {id}"),
                    )
                })?
                .clone(),
        ),
        None => ended.first().cloned(),
    };
    let mut plan = ResumePlan {
        key: key.clone(),
        title: item.map(|i| i.title).filter(|t| !t.is_empty()),
        live,
        candidates,
        link_id: chosen.as_ref().map(|l| l.id),
        host_alias: None,
        project_id: None,
        branch: None,
        worktree: None,
        worktree_present: false,
        modes: Vec::new(),
        hosts: reachable_hosts(s)?,
        brief: None,
        warnings: Vec::new(),
    };

    // Reasons that block every mode.
    let blocked: Option<String> = if let Some(l) = plan.live.first() {
        Some(format!(
            "{key} is live in {} on {} — jump to it",
            l.friendly_name.as_deref().unwrap_or(&l.tmux_name),
            l.host_alias
        ))
    } else if chosen.is_none() {
        Some(format!("{key} has no past sessions to resume"))
    } else {
        None
    };
    let Some(link) = chosen.filter(|_| blocked.is_none()) else {
        let why = blocked.unwrap_or_default();
        plan.modes = RESUME_MODES
            .iter()
            .map(|m| mode(m, Some(why.clone())))
            .collect();
        return Ok(plan);
    };
    let c = candidate(&link);

    // Host: an override, else the snapshot's when it is reachable.
    let snap_host = link.snap_host.clone();
    let host: Result<String, String> = match (host_alias, snap_host.as_deref()) {
        (Some(h), _) => match host_reachable(s, h)? {
            Some(true) => Ok(h.to_string()),
            Some(false) => Err(format!("{h} is unreachable")),
            None => Err(format!("unknown host {h}")),
        },
        (None, Some(h)) => match host_reachable(s, h)? {
            Some(true) => Ok(h.to_string()),
            _ => Err(format!(
                "{h} is unreachable or gone; pick another host ({})",
                plan.hosts.join(", ")
            )),
        },
        (None, None) => Err("the past session's host is unknown; pick a host".into()),
    };
    // Project.
    let project: Result<i64, String> = match link.snap_project_id {
        Some(pid) => match crate::service::sessions::fetch_owner_repo(s, pid) {
            Ok(_) => Ok(pid),
            Err(_) => Err("its project is no longer known to fleet".into()),
        },
        None => Err("the past session had no project".into()),
    };
    if let Ok(h) = &host {
        plan.host_alias = Some(h.clone());
    }
    if let Ok(pid) = &project {
        plan.project_id = Some(*pid);
    }
    plan.branch = c.branch.clone();
    plan.worktree = c.worktree.clone();
    if let (Ok(h), Ok(pid), Some(wt)) = (&host, &project, c.worktree.as_deref()) {
        plan.worktree_present = s
            .list_worktrees_on_host(h)?
            .iter()
            .any(|w| w.project_id == *pid && w.name == wt);
    }
    let base_err = host.as_ref().err().cloned().or(project.clone().err());

    // `last`: the transcript must be where the session will run, still
    // there, and held by no other session on the host.
    let last_err = base_err.clone().or_else(|| {
        let h = host.as_ref().ok()?;
        if snap_host.as_deref() != Some(h.as_str()) {
            return Some(format!(
                "the conversation's transcript is on {}; continue it there, or start fresh here",
                snap_host.as_deref().unwrap_or("another host")
            ));
        }
        if !c.resumable {
            return Some("its transcripts were purged".into());
        }
        let Some(id) = c.last_claude_session_id.as_deref() else {
            return Some("no Claude conversation was recorded".into());
        };
        match s.session_with_claude_id(h, id) {
            Ok(Some(r)) => Some(format!(
                "session {} still holds that conversation; restore or jump to it",
                r.tmux_name
            )),
            _ => None,
        }
    });
    plan.modes = vec![
        mode("last", last_err),
        mode("brief", base_err.clone()),
        mode("fresh", base_err),
    ];
    Ok(plan)
}

/// Cap on the transcript probe (work graph M11.2): one command over the
/// host's ControlMaster, connect and wall clock alike.
const TRANSCRIPT_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// The probe's answer lines are `<tag>present` / `<tag>absent`; anything
/// else (login-shell noise, a failed shell) is not an answer.
pub(crate) const TRANSCRIPT_PROBE_TAG: &str = "fleet-transcript=";

/// Where Claude keeps a host's transcripts, from the host's own `$HOME`
/// (expanded host-side, never interpolated from fleet).
const HOME_PROJECTS: &str = "\"$HOME\"/.claude/projects";

/// The directory a transcript path starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptRoot<'a> {
    /// `$HOME/.claude/projects` on the host.
    Home,
    /// A stored `<…>/.claude/projects` directory (from a hook-reported
    /// transcript path).
    Dir(&'a str),
}

/// A project slug is one path component Claude derived from the cwd.
fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 255
        && slug != "."
        && slug != ".."
        && !slug.contains('/')
        && !slug.chars().any(|c| c.is_control())
}

/// An absolute, `.`/`..`-free, control-free `…/.claude/projects` directory.
fn valid_projects_dir(dir: &str) -> bool {
    dir.starts_with('/')
        && dir.len() <= 4096
        && dir.ends_with("/.claude/projects")
        && !dir.chars().any(|c| c.is_control())
        && !dir.split('/').any(|c| c == ".." || c == ".")
}

/// PURE: the shell word naming a conversation's transcript, built only from
/// validated parts and quoted — `root/slug/<claude id>.jsonl`, where a
/// missing slug is the glob `*` (any project directory). A non-UUID id, a
/// slug that is not a single path component, or a root that is not a
/// `…/.claude/projects` directory is refused.
pub fn transcript_path_expr(
    root: TranscriptRoot<'_>,
    slug: Option<&str>,
    claude_session_id: &str,
) -> Result<String, String> {
    crate::validate::claude_session_id(claude_session_id).map_err(|e| e.message)?;
    let root = match root {
        TranscriptRoot::Home => HOME_PROJECTS.to_string(),
        TranscriptRoot::Dir(d) if valid_projects_dir(d) => crate::shell::quote(d),
        TranscriptRoot::Dir(d) => return Err(format!("refusing transcript directory {d:?}")),
    };
    let slug = match slug {
        None => "*".to_string(),
        Some(s) if valid_slug(s) => crate::shell::quote(s),
        Some(s) => return Err(format!("refusing transcript project slug {s:?}")),
    };
    Ok(format!(
        "{root}/{slug}/{}",
        crate::shell::quote(&format!("{claude_session_id}.jsonl"))
    ))
}

/// PURE: the one-command existence check for `claude_session_id`'s
/// transcript. It looks at the path a conversation row recorded (when it
/// still validates), then under every project of the host's
/// `$HOME/.claude/projects` — "absent" means absent from both, so a moved
/// cwd never reads as a lost transcript.
pub fn transcript_probe_script(
    stored_path: Option<&str>,
    claude_session_id: &str,
) -> Result<String, String> {
    let mut paths = Vec::new();
    if let Some(p) =
        stored_path.filter(|p| crate::service::hooks::valid_transcript_path(p, claude_session_id))
    {
        let stored = p
            .rsplit_once('/')
            .and_then(|(dir, _)| dir.rsplit_once('/'))
            .and_then(|(root, slug)| {
                transcript_path_expr(TranscriptRoot::Dir(root), Some(slug), claude_session_id).ok()
            });
        paths.extend(stored);
    }
    paths.push(transcript_path_expr(
        TranscriptRoot::Home,
        None,
        claude_session_id,
    )?);
    Ok(format!(
        "for f in {}; do if [ -f \"$f\" ]; then echo {TRANSCRIPT_PROBE_TAG}present; exit 0; fi; done; echo {TRANSCRIPT_PROBE_TAG}absent",
        paths.join(" ")
    ))
}

/// What the transcript probe found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptProbe {
    Present,
    Absent,
    /// Timed out, failed, or could not be asked: never blocks a resume.
    Unknown,
}

/// PURE: read the probe's output (its last answer line wins).
pub fn parse_transcript_probe(stdout: &str) -> TranscriptProbe {
    stdout
        .lines()
        .rev()
        .find_map(|l| match l.trim().strip_prefix(TRANSCRIPT_PROBE_TAG) {
            Some("present") => Some(TranscriptProbe::Present),
            Some("absent") => Some(TranscriptProbe::Absent),
            _ => None,
        })
        .unwrap_or(TranscriptProbe::Unknown)
}

/// The probe a planned *continue* needs, read under the lock.
struct TranscriptCheck {
    host: String,
    /// The script, or why none could be built (answered as `Unknown`).
    script: Result<String, String>,
}

/// The probe for `plan`, when it would continue the held conversation on a
/// reachable host (`last` is possible) — else `None`: an unreachable host,
/// another host, a purge or a held conversation already said no.
fn transcript_check(s: &Store, plan: &ResumePlan) -> Result<Option<TranscriptCheck>, IpcError> {
    if !plan.modes.iter().any(|m| m.mode == "last" && m.ok) {
        return Ok(None);
    }
    let Some(host) = plan.host_alias.clone() else {
        return Ok(None);
    };
    let Some(id) = plan
        .candidates
        .iter()
        .find(|c| Some(c.link_id) == plan.link_id)
        .and_then(|c| c.last_claude_session_id.clone())
    else {
        return Ok(None);
    };
    let stored = if crate::validate::claude_session_id(&id).is_ok() {
        s.conversation_transcript_path_on_host(&host, &id)?
    } else {
        None
    };
    Ok(Some(TranscriptCheck {
        script: transcript_probe_script(stored.as_deref(), &id),
        host,
    }))
}

/// Run the check: one command over the host's ControlMaster (`local` runs
/// it locally, without SSH), capped at [`TRANSCRIPT_PROBE_TIMEOUT`].
async fn run_transcript_check(exec: &dyn SshExec, check: &TranscriptCheck) -> TranscriptProbe {
    let Ok(script) = &check.script else {
        return TranscriptProbe::Unknown;
    };
    match crate::ssh::run_shell_bounded(
        exec,
        &check.host,
        script,
        TRANSCRIPT_PROBE_TIMEOUT,
        TRANSCRIPT_PROBE_TIMEOUT,
    )
    .await
    {
        Ok(out) if out.status.success() => {
            parse_transcript_probe(&String::from_utf8_lossy(&out.stdout))
        }
        _ => TranscriptProbe::Unknown,
    }
}

/// Fold the probe into the plan: an absent transcript turns `last` off (the
/// UI then lands on *Fresh with brief*); an unknown one only warns.
fn apply_transcript_probe(plan: &mut ResumePlan, host: &str, found: TranscriptProbe) {
    match found {
        TranscriptProbe::Present => {}
        TranscriptProbe::Absent => {
            if let Some(m) = plan.modes.iter_mut().find(|m| m.mode == "last") {
                *m = mode(
                    "last",
                    Some(format!(
                        "the conversation's transcript is no longer on {host}"
                    )),
                );
            }
        }
        TranscriptProbe::Unknown => plan
            .warnings
            .push(format!("could not check the transcript on {host}")),
    }
}

/// What [`resume_plan_with`] reads beyond the store.
#[derive(Debug, Clone, Copy, Default)]
struct PlanReads {
    /// Build the handover brief (one git probe).
    with_brief: bool,
    /// Probe the transcript a *continue* would resume (work graph M11.2).
    transcript: bool,
}

/// [`plan_resume`], plus the handover brief when `with_brief` (one git probe
/// on the landing host, off the lock), and — when the plan would continue
/// the held conversation — one probe that its transcript is still on the
/// host (work graph M11.2).
///
/// The brief is written for the Claude that will read it — the one on the
/// landing host — so it holds only what that host's org scope may read
/// (work graph M5), whoever asked for the plan.
pub async fn resume_plan(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    key: &str,
    link_id: Option<i64>,
    host_alias: Option<&str>,
    with_brief: bool,
    scope: &OrgScope,
) -> Result<ResumePlan, IpcError> {
    let reads = PlanReads {
        with_brief,
        transcript: true,
    };
    resume_plan_with(store, ssh.as_ref(), key, link_id, host_alias, reads, scope).await
}

async fn resume_plan_with(
    store: &Mutex<Store>,
    exec: &dyn SshExec,
    key: &str,
    link_id: Option<i64>,
    host_alias: Option<&str>,
    reads: PlanReads,
    scope: &OrgScope,
) -> Result<ResumePlan, IpcError> {
    let (mut plan, gathered, transcript) = {
        let s = lock(store)?;
        let plan = plan_resume(&s, key, link_id, host_alias, scope)?;
        let transcript = if reads.transcript {
            transcript_check(&s, &plan)?
        } else {
            None
        };
        let gathered = if reads.with_brief {
            let target = probe_target_for(&s, &plan);
            // Two readers, and the brief is only what BOTH may read: the
            // caller (who sees the plan now) and the landing host (whose
            // Claude reads it later) — the plan's host, else the one asked
            // for (even when it cannot be used now), else the past
            // session's. A per-host token is its own reader whatever host
            // it names, so it cannot borrow another org's scope.
            let landing = plan.host_alias.as_deref().or(host_alias).or(plan
                .candidates
                .iter()
                .find(|c| Some(c.link_id) == plan.link_id)
                .and_then(|c| c.host_alias.as_deref()));
            let reader = match (scope, landing) {
                (OrgScope::All, Some(h)) => OrgScope::for_host(&s, h)?,
                _ => scope.clone(),
            };
            Some(handover::gather_stored(&s, key, target, &reader)?)
        } else {
            None
        };
        (plan, gathered, transcript)
    };
    if let Some(check) = transcript {
        let found = run_transcript_check(exec, &check).await;
        apply_transcript_probe(&mut plan, &check.host, found);
    }
    if let Some(g) = gathered {
        let mut input = g.input;
        if let Some(t) = g.target {
            match handover::probe(exec, &t).await {
                Ok(facts) => input.git = Some(facts),
                Err(note) => input.git_note = Some(note),
            }
        }
        plan.brief = Some(handover::build_handover(&input));
    }
    Ok(plan)
}

fn probe_target_for(s: &Store, plan: &ResumePlan) -> Option<handover::ProbeTarget> {
    handover::probe_target(
        s,
        plan.host_alias.as_deref()?,
        plan.project_id?,
        plan.worktree.as_deref(),
        plan.branch.clone(),
    )
}

/// The worktree to open: the known one, else recreate it from the branch.
/// The recreated worktree takes the BRANCH's name when that is a plain name,
/// so the work stays on its branch (`worktree_add_script` checks an existing
/// branch out when base == name); a `feat/x`-style branch keeps the old
/// worktree name and forks from it instead.
fn worktree_choice(
    s: &Store,
    plan: &ResumePlan,
) -> Result<(Option<i64>, Option<String>), IpcError> {
    let (Some(host), Some(pid)) = (plan.host_alias.as_deref(), plan.project_id) else {
        return Ok((None, None));
    };
    let Some(wt) = plan.worktree.as_deref() else {
        return Ok((None, None));
    };
    if let Some(row) = s
        .list_worktrees_on_host(host)?
        .into_iter()
        .find(|w| w.project_id == pid && w.name == wt)
    {
        return Ok((Some(row.id), None));
    }
    let plain = |b: &str| {
        crate::validate::git_ref(b).is_ok() && !b.contains('/') && b != "main" && b != "master"
    };
    let name = match plan.branch.as_deref() {
        Some(b) if plain(b) => b.to_string(),
        _ => wt.to_string(),
    };
    Ok((None, Some(name)))
}

/// The `new_session` a resume makes, from its plan.
pub fn resume_session_args(
    s: &Store,
    plan: &ResumePlan,
    mode: &str,
) -> Result<NewSessionArgs, IpcError> {
    let m = plan.modes.iter().find(|m| m.mode == mode).ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID,
            format!(
                "unknown resume mode {mode:?}; one of {}",
                RESUME_MODES.join(", ")
            ),
        )
    })?;
    if !m.ok {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "cannot resume {} ({mode}): {}",
                plan.key,
                m.reason.as_deref().unwrap_or("not possible")
            ),
        ));
    }
    let (Some(host), Some(pid)) = (plan.host_alias.clone(), plan.project_id) else {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            "no host or project to resume on",
        ));
    };
    let (worktree_id, new_worktree) = worktree_choice(s, plan)?;
    let chosen = plan
        .candidates
        .iter()
        .find(|c| Some(c.link_id) == plan.link_id);
    let friendly_name = chosen
        .and_then(|c| c.name.clone())
        .filter(|n| crate::validate::friendly_name(n).is_ok());
    Ok(NewSessionArgs {
        host_alias: host,
        project_id: pid,
        worktree_id,
        name: String::new(),
        call_id: None,
        base_branch: new_worktree.as_ref().and(plan.branch.clone()),
        new_worktree,
        kind: None,
        start_command: None,
        friendly_name,
        resume_claude_session_id: if mode == "last" {
            chosen.and_then(|c| c.last_claude_session_id.clone())
        } else {
            None
        },
    })
}

/// Keys with a resume between its re-checked guards and its link, per
/// store (`Store::instance_id` tells one fleet's registry from another's,
/// which only matters to tests — and, unlike the store's address, is never
/// reused by a store built where a dropped one was): a second resume of one
/// of them meanwhile is refused with `E_EXISTS` (the store lock is not held
/// across the spawn, so the guards alone would let two callers spawn two
/// sessions on one conversation).
static IN_FLIGHT: Mutex<std::collections::BTreeSet<(u64, String)>> =
    Mutex::new(std::collections::BTreeSet::new());

/// One key's claim; released on drop, whatever the resume's outcome.
#[derive(Debug)]
struct InFlight(u64, String);

fn store_key(store: &Store) -> u64 {
    store.instance_id()
}

impl InFlight {
    fn claim(store: &Store, key: &str) -> Result<Self, IpcError> {
        let mut set = IN_FLIGHT
            .lock()
            .map_err(|_| IpcError::new(codes::E_LOCK, "the resume registry is poisoned"))?;
        let id = store_key(store);
        if !set.insert((id, key.to_string())) {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("{key} is being resumed already; wait for that session, then jump to it"),
            ));
        }
        Ok(InFlight(id, key.to_string()))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if let Ok(mut set) = IN_FLIGHT.lock() {
            set.remove(&(self.0, std::mem::take(&mut self.1)));
        }
    }
}

/// The brief a caller edited, blank counting as none: the Resume dialog
/// sends `""` for a cleared textarea, and a blank body could only fail
/// after the session exists. None here means the plan builds the brief.
fn edited_brief(brief: Option<&str>) -> Option<String> {
    brief
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(str::to_string)
}

/// The short start prompt typed into a `brief` resume (the brief itself
/// rides the hook's `additionalContext`, never the pane).
pub fn start_prompt(key: &str) -> String {
    format!(
        "Pick up {key} where the previous sessions left it: the fleet handover brief is in your context. Verify the git state first."
    )
}

/// Do the resume. `spawn` makes the session (production: `new_session`);
/// injected so the flow is testable without tmux. Returns the new row,
/// linked to the work with source `resumed`.
pub async fn resume_with<F, Fut>(
    store: &Arc<Mutex<Store>>,
    ssh: &dyn SshExec,
    args: &ResumeArgs,
    scope: &OrgScope,
    spawn: F,
) -> Result<(SessionRow, Option<i64>), IpcError>
where
    F: FnOnce(NewSessionArgs) -> Fut,
    Fut: std::future::Future<Output = Result<SessionRow, IpcError>>,
{
    if !RESUME_MODES.contains(&args.mode.as_str()) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "unknown resume mode {:?}; one of {}",
                args.mode,
                RESUME_MODES.join(", ")
            ),
        ));
    }
    let edited = edited_brief(args.brief.as_deref());
    let reads = PlanReads {
        with_brief: args.mode == "brief" && edited.is_none(),
        // Only a *continue* resumes the transcript.
        transcript: args.mode == "last",
    };
    let plan = resume_plan_with(
        store,
        ssh,
        &args.key,
        args.link_id,
        args.host_alias.as_deref(),
        reads,
        scope,
    )
    .await?;
    // A per-host token resumes only onto its own host.
    if let Some(h) = scope.host() {
        let target = plan.host_alias.as_deref().unwrap_or("an unknown host");
        if target != h {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                format!("the resumed session is on host {target}; this token is bound to {h}"),
            ));
        }
    }
    let (new_args, _claim) = {
        let s = lock(store)?;
        // The plan's guards (no live session for the key; for `last`, the
        // conversation held by no session on the host) ran under an earlier
        // lock, and the spawn runs off it: re-check now, and hold the key
        // until the new session is linked, so a concurrent resume of the
        // same key (desktop and phone, a double click) is refused instead
        // of resuming one conversation twice.
        let fresh = plan_resume(
            &s,
            &plan.key,
            plan.link_id,
            args.host_alias.as_deref(),
            scope,
        )?;
        if let Some(l) = fresh.live.first() {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!(
                    "{} is live in {} on {} — jump to it",
                    plan.key,
                    l.friendly_name.as_deref().unwrap_or(&l.tmux_name),
                    l.host_alias
                ),
            ));
        }
        if args.mode == "last" {
            let held = fresh
                .candidates
                .iter()
                .find(|c| Some(c.link_id) == plan.link_id)
                .and_then(|c| c.last_claude_session_id.clone())
                .zip(plan.host_alias.as_deref())
                .map(|(id, h)| s.session_with_claude_id(h, &id))
                .transpose()?
                .flatten();
            if let Some(r) = held {
                return Err(IpcError::new(
                    codes::E_EXISTS,
                    format!(
                        "session {} still holds that conversation; restore or jump to it",
                        r.tmux_name
                    ),
                ));
            }
        }
        let claim = InFlight::claim(&s, &plan.key)?;
        let new_args = resume_session_args(&s, &plan, &args.mode)?;
        // The resumed session links the work: the same integrity rule as a
        // link, for every caller (M5).
        if let Some(pid) = plan.project_id {
            let work_org = match s.work_item_by_key(&plan.key)? {
                Some(i) => s.item_org(i.id)?,
                None => None,
            };
            let session_org = s.org_for_new_session(&new_args.host_alias, pid)?;
            crate::service::orgs::check_cross_org(
                work_org,
                session_org,
                &plan.key,
                args.force_cross_org,
            )?;
        }
        (new_args, claim)
    };
    let row = spawn(new_args).await?;
    let handover = {
        let s = lock(store)?;
        s.link_resumed_work(row.id, &plan.key)?;
        if args.mode == "brief" {
            let body = edited.or(plan.brief.clone()).unwrap_or_default();
            let body: String = body.chars().take(handover::BRIEF_MAX_CHARS).collect();
            let meta = serde_json::json!({ "key": plan.key, "link_id": plan.link_id }).to_string();
            // The session exists and is linked by now: a brief that cannot
            // be queued (a blank one, say) is recorded on its timeline and
            // the row is still answered; an error here would leave the
            // caller with a session it was told failed.
            match s.enqueue_handover(row.id, &body, Some(&meta)) {
                Ok(id) => Some(id),
                Err(e) => {
                    tracing::warn!(session_id = row.id, key = %plan.key, error = %e.message, "[resume] the brief was not queued");
                    let _ = s.insert_session_event(row.id, "handover_missing", Some(&e.message));
                    None
                }
            }
        } else {
            None
        }
    };
    let row = lock(store)?.get_session_by_id(row.id)?.unwrap_or(row);
    Ok((row, handover))
}

/// [`resume_with`] over the real `new_session`, then (for `brief`) the
/// start prompt in the background.
pub async fn resume_work(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    reg: &Arc<CancellationRegistry>,
    args: &ResumeArgs,
    scope: &OrgScope,
) -> Result<SessionRow, IpcError> {
    let (row, handover) = resume_with(store, ssh.as_ref(), args, scope, |a| {
        sessions::new_session(a, store.as_ref(), ssh, reg)
    })
    .await?;
    if handover.is_some() {
        spawn_start_prompt(
            Arc::clone(store),
            Arc::clone(ssh),
            &row,
            start_prompt(&args.key),
        );
    }
    Ok(row)
}

/// Type `prompt` into `row`'s pane once its REPL is ready, in the
/// background (see [`send_start_prompt`] for what it never types into).
/// Shared by a brief resume and a brief start (work graph M3.4).
pub fn spawn_start_prompt(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    row: &SessionRow,
    prompt: String,
) {
    let (id, host, tmux) = (row.id, row.host_alias.clone(), row.tmux_name.clone());
    crate::rt::spawn(async move {
        send_start_prompt(store, ssh, id, host, tmux, prompt).await;
    });
}

/// What the pane says about typing into it now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptGate {
    /// The REPL is up and waiting: type.
    Ready,
    /// "Do you trust the files in this folder?": NEVER type (Enter would
    /// answer it). The brief stays queued for the user's first prompt.
    TrustDialog,
    /// Another dialog or stuck state: do not type.
    Blocked,
    /// Still starting.
    NotYet,
}

/// PURE: decide from a captured pane.
pub fn prompt_gate(pane: &str) -> PromptGate {
    let intel = crate::service::pane_intel::analyze(pane);
    match intel.stuck {
        Some(StuckKind::TrustPrompt) => return PromptGate::TrustDialog,
        Some(_) => return PromptGate::Blocked,
        None => {}
    }
    if intel.waiting_for.is_some() {
        return PromptGate::Blocked;
    }
    if crate::service::tasks::pane_shows_repl(pane) {
        PromptGate::Ready
    } else {
        PromptGate::NotYet
    }
}

const START_PROMPT_WAIT: Duration = Duration::from_secs(60);
const START_PROMPT_POLL: Duration = Duration::from_millis(1000);
const START_PROMPT_ACK: Duration = Duration::from_secs(15);

/// Type the short start prompt once the REPL is ready — and never while the
/// pane shows the trust dialog, another dialog, or a stuck state. Confirmed
/// through `prompt_submit_seq`. Every outcome is a timeline event; the brief
/// itself is already queued and rides the next prompt whatever happens here.
async fn send_start_prompt(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    session_id: i64,
    host: String,
    tmux_name: String,
    prompt: String,
) {
    let tmux: Box<dyn crate::tmux::TmuxExec> = if host == "local" {
        Box::new(crate::tmux::LocalTmux)
    } else {
        Box::new(crate::tmux::RemoteTmux {
            client: Arc::clone(&ssh),
            host: host.clone(),
        })
    };
    let event = |kind: &str, detail: Option<&str>| {
        if let Ok(s) = store.lock() {
            let _ = s.insert_session_event(session_id, kind, detail);
        }
    };
    let deadline = tokio::time::Instant::now() + START_PROMPT_WAIT;
    loop {
        let gate = match tmux.capture_pane(&tmux_name).await {
            Ok(text) => prompt_gate(&text),
            Err(_) => PromptGate::NotYet,
        };
        match gate {
            PromptGate::Ready => break,
            PromptGate::TrustDialog => {
                event("handover_waiting", Some("trust_prompt"));
                return;
            }
            PromptGate::Blocked | PromptGate::NotYet => {}
        }
        if tokio::time::Instant::now() >= deadline {
            event("handover_waiting", Some("repl_not_ready"));
            return;
        }
        tokio::time::sleep(START_PROMPT_POLL).await;
    }
    let before = match store.lock() {
        Ok(s) => s
            .prompt_ack_state(session_id)
            .ok()
            .flatten()
            .map(|st| st.prompt_submit_seq),
        Err(_) => return,
    };
    let sent = sessions::send_prompt(
        sessions::SendPromptArgs {
            host_alias: host,
            tmux_name,
            prompt,
            submit: true,
            keys: None,
        },
        &store,
        &ssh,
    )
    .await;
    if let Err(e) = sent {
        event(
            "handover_waiting",
            Some(&format!("send failed: {}", e.code)),
        );
        return;
    }
    let deadline = tokio::time::Instant::now() + START_PROMPT_ACK;
    loop {
        let seq = store
            .lock()
            .ok()
            .and_then(|s| s.prompt_ack_state(session_id).ok().flatten())
            .map(|st| st.prompt_submit_seq);
        if let (Some(b), Some(now)) = (before, seq) {
            if now > b {
                event("handover_started", None);
                return;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            event("handover_waiting", Some("start prompt not acknowledged"));
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_fake::{FakeSsh, Match, Reply};
    use crate::store::WorkTarget;

    /// A past session of ABC-1 on `h`, ended, in worktree `abc-1` of a
    /// project, with conversation `c-1`.
    fn fixture() -> (Arc<Mutex<Store>>, i64) {
        fixture_on("h")
    }

    /// [`fixture`] with the past session on `host`.
    fn fixture_on(host: &str) -> (Arc<Mutex<Store>>, i64) {
        fixture_full(host, "c-1")
    }

    /// [`fixture`] with conversation `conv`.
    fn fixture_with(conv: &str) -> (Arc<Mutex<Store>>, i64) {
        fixture_full("h", conv)
    }

    fn fixture_full(host: &str, conv: &str) -> (Arc<Mutex<Store>>, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host(host).unwrap();
        let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
        let id = s
            .upsert_session("dev-o-r--abc-1", host, None, None, 1, 1, "running", None)
            .unwrap();
        s.conn_ref()
            .execute(
                "UPDATE sessions SET project_id = ?1, worktree_key = 'abc-1', \
                 friendly_name = 'Fix login' WHERE id = ?2",
                rusqlite::params![pid, id],
            )
            .unwrap();
        s.conn_ref()
            .execute(
                "INSERT INTO worktrees (project_id, host_alias, name, path, branch) \
                 VALUES (?1, ?2, 'abc-1', '/p/o/r/.worktrees/abc-1', 'abc-1')",
                rusqlite::params![pid, host],
            )
            .unwrap();
        let wt: i64 = s.conn_ref().last_insert_rowid();
        s.conn_ref()
            .execute(
                "UPDATE sessions SET worktree_id = ?1 WHERE id = ?2",
                rusqlite::params![wt, id],
            )
            .unwrap();
        s.rebind_conversation(id, conv, crate::store::StartSource::Startup, None, None)
            .unwrap();
        s.link_session_work(id, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.delete_session(id).unwrap();
        (Arc::new(Mutex::new(s)), pid)
    }

    fn modes(p: &ResumePlan) -> Vec<(&str, bool)> {
        p.modes.iter().map(|m| (m.mode.as_str(), m.ok)).collect()
    }

    #[test]
    fn a_reachable_host_with_its_worktree_allows_every_mode() {
        let (st, pid) = fixture();
        let s = st.lock().unwrap();
        let p = plan_resume(&s, "abc-1", None, None, &OrgScope::All).unwrap();
        assert_eq!(
            modes(&p),
            vec![("last", true), ("brief", true), ("fresh", true)]
        );
        assert_eq!(p.host_alias.as_deref(), Some("h"));
        assert_eq!(p.project_id, Some(pid));
        assert!(p.worktree_present);
        assert_eq!(
            p.candidates[0].last_claude_session_id.as_deref(),
            Some("c-1")
        );
        let a = resume_session_args(&s, &p, "last").unwrap();
        assert_eq!(a.resume_claude_session_id.as_deref(), Some("c-1"));
        assert!(a.worktree_id.is_some());
        assert_eq!(a.new_worktree, None);
        assert_eq!(a.friendly_name.as_deref(), Some("Fix login"));
        let a = resume_session_args(&s, &p, "fresh").unwrap();
        assert_eq!(a.resume_claude_session_id, None);
    }

    #[test]
    fn a_removed_worktree_is_recreated_from_its_branch() {
        let (st, _) = fixture();
        let s = st.lock().unwrap();
        s.conn_ref().execute("DELETE FROM worktrees", []).unwrap();
        let p = plan_resume(&s, "ABC-1", None, None, &OrgScope::All).unwrap();
        assert!(!p.worktree_present);
        assert!(p.modes.iter().all(|m| m.ok));
        let a = resume_session_args(&s, &p, "last").unwrap();
        assert_eq!(a.worktree_id, None);
        assert_eq!(a.new_worktree.as_deref(), Some("abc-1"));
        assert_eq!(a.base_branch.as_deref(), Some("abc-1"));
    }

    #[test]
    fn an_unreachable_host_disables_every_mode_until_another_is_picked() {
        let (st, _) = fixture();
        let s = st.lock().unwrap();
        s.update_host_probe("h", false, None, None, 0).unwrap();
        s.upsert_host("g").unwrap();
        let p = plan_resume(&s, "ABC-1", None, None, &OrgScope::All).unwrap();
        assert!(p.modes.iter().all(|m| !m.ok));
        assert!(p.modes[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("unreachable"));
        assert!(p.hosts.contains(&"g".to_string()));
        let err = resume_session_args(&s, &p, "fresh").err().expect("refused");
        assert_eq!(err.code, codes::E_INVALID_STATE);

        let p = plan_resume(&s, "ABC-1", None, Some("g"), &OrgScope::All).unwrap();
        assert_eq!(
            modes(&p),
            vec![("last", false), ("brief", true), ("fresh", true)]
        );
        assert!(p.modes[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("transcript is on h"));
        let a = resume_session_args(&s, &p, "brief").unwrap();
        assert_eq!(a.host_alias, "g");
        assert_eq!(
            a.new_worktree.as_deref(),
            Some("abc-1"),
            "no worktree row on g"
        );
    }

    #[test]
    fn purged_transcripts_disable_continue_but_not_a_fresh_start() {
        let (st, pid) = fixture();
        let s = st.lock().unwrap();
        s.mark_purged_work_unresumable(pid, &["h".to_string()])
            .unwrap();
        let p = plan_resume(&s, "ABC-1", None, None, &OrgScope::All).unwrap();
        assert_eq!(
            modes(&p),
            vec![("last", false), ("brief", true), ("fresh", true)]
        );
        assert!(p.modes[0].reason.as_deref().unwrap().contains("purged"));
    }

    #[test]
    fn live_work_offers_jump_and_a_held_conversation_is_never_resumed_twice() {
        let (st, _) = fixture();
        let s = st.lock().unwrap();
        let live = s
            .upsert_session("other", "h", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(live, "c-1").unwrap();
        // The rebind carried the work onto `live`: it is live now.
        let p = plan_resume(&s, "ABC-1", None, None, &OrgScope::All).unwrap();
        assert_eq!(p.live.len(), 1);
        assert!(p.modes.iter().all(|m| !m.ok));
        assert!(p.modes[0].reason.as_deref().unwrap().contains("jump"));

        // Unlinked, it still holds the conversation: `last` stays off.
        let link = s.session_work_links(live).unwrap()[0].id;
        s.unlink_session_work(live, link).unwrap();
        let p = plan_resume(&s, "ABC-1", None, None, &OrgScope::All).unwrap();
        assert_eq!(
            modes(&p),
            vec![("last", false), ("brief", true), ("fresh", true)]
        );
        assert!(p.modes[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("still holds"));
    }

    #[test]
    fn unknown_keys_links_and_modes_are_refused() {
        let (st, _) = fixture();
        let s = st.lock().unwrap();
        let p = plan_resume(&s, "NOPE-1", None, None, &OrgScope::All).unwrap();
        assert!(p.modes.iter().all(|m| !m.ok));
        assert_eq!(
            plan_resume(&s, "ABC-1", Some(999), None, &OrgScope::All)
                .unwrap_err()
                .code,
            codes::E_NOTFOUND
        );
        let p = plan_resume(&s, "ABC-1", None, None, &OrgScope::All).unwrap();
        assert_eq!(
            resume_session_args(&s, &p, "sideways")
                .err()
                .expect("refused")
                .code,
            codes::E_INVALID
        );
    }

    #[tokio::test]
    async fn a_brief_resume_links_the_session_and_queues_the_edited_brief() {
        let (st, _) = fixture();
        let ssh = Arc::new(SshClient::new());
        let args = ResumeArgs {
            key: "abc-1".into(),
            mode: "brief".into(),
            brief: Some("my edited brief".into()),
            ..Default::default()
        };
        let spawned = Arc::new(Mutex::new(None));
        let seen = Arc::clone(&spawned);
        let st2 = Arc::clone(&st);
        let (row, handover) = resume_with(&st, &ssh, &args, &OrgScope::All, |a| async move {
            *seen.lock().unwrap() =
                Some((a.host_alias.clone(), a.resume_claude_session_id.clone()));
            let s = st2.lock().unwrap();
            let id = s
                .upsert_session("dev-new", &a.host_alias, None, None, 1, 1, "running", None)
                .unwrap();
            Ok(s.get_session_by_id(id).unwrap().unwrap())
        })
        .await
        .unwrap();
        assert_eq!(
            spawned.lock().unwrap().clone(),
            Some(("h".to_string(), None))
        );
        assert!(handover.is_some());
        let w = row.work.expect("linked");
        assert_eq!(
            (w.key.as_deref(), w.source.as_str()),
            (Some("ABC-1"), "resumed")
        );
        let s = st.lock().unwrap();
        let queued = s.undelivered_handovers(row.id).unwrap();
        assert_eq!(queued[0].body.as_deref(), Some("my edited brief"));
    }

    // ── work graph M11.2: the transcript probe ─────────────────────────────

    const CID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

    const PLAN: PlanReads = PlanReads {
        with_brief: false,
        transcript: true,
    };

    async fn probed(st: &Arc<Mutex<Store>>, fake: &FakeSsh, host: Option<&str>) -> ResumePlan {
        resume_plan_with(st, fake, "ABC-1", None, host, PLAN, &OrgScope::All)
            .await
            .unwrap()
    }

    fn probe_calls(fake: &FakeSsh) -> Vec<String> {
        fake.calls()
            .iter()
            .filter_map(|c| c.script())
            .filter(|sc| sc.contains(TRANSCRIPT_PROBE_TAG))
            .collect()
    }

    #[tokio::test]
    async fn a_present_transcript_keeps_continue() {
        let (st, _) = fixture_with(CID);
        let fake = FakeSsh::new();
        fake.on_host(
            "h",
            Match::script_contains(TRANSCRIPT_PROBE_TAG),
            Reply::ok("motd noise\nfleet-transcript=present\n"),
        );
        let p = probed(&st, &fake, None).await;
        assert_eq!(
            modes(&p),
            vec![("last", true), ("brief", true), ("fresh", true)]
        );
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        let calls = probe_calls(&fake);
        assert_eq!(calls.len(), 1, "one command: {calls:?}");
        assert!(
            calls[0].contains(&format!("\"$HOME\"/.claude/projects/*/'{CID}.jsonl'")),
            "{}",
            calls[0]
        );
    }

    #[tokio::test]
    async fn an_absent_transcript_turns_continue_into_a_fresh_start() {
        let (st, _) = fixture_with(CID);
        let fake = FakeSsh::new();
        fake.on_host(
            "h",
            Match::script_contains(TRANSCRIPT_PROBE_TAG),
            Reply::ok("fleet-transcript=absent\n"),
        );
        let p = probed(&st, &fake, None).await;
        assert_eq!(
            modes(&p),
            vec![("last", false), ("brief", true), ("fresh", true)]
        );
        assert_eq!(
            p.modes[0].reason.as_deref(),
            Some("the conversation's transcript is no longer on h")
        );
        assert!(p.warnings.is_empty());

        // Doing the *continue* anyway is refused with the same reason.
        let args = ResumeArgs {
            key: "ABC-1".into(),
            mode: "last".into(),
            ..Default::default()
        };
        let err = resume_with(&st, &fake, &args, &OrgScope::All, |_| async {
            panic!("must not spawn")
        })
        .await
        .expect_err("refused");
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(err.message.contains("no longer on h"), "{}", err.message);
    }

    #[tokio::test]
    async fn a_timed_out_or_failed_probe_keeps_continue_and_warns() {
        let (st, _) = fixture_with(CID);
        let fake = FakeSsh::new();
        fake.hanging("h").set_wall_clock(Duration::from_millis(20));
        let p = probed(&st, &fake, None).await;
        assert!(p.modes.iter().all(|m| m.ok), "{:?}", p.modes);
        assert_eq!(p.warnings, vec!["could not check the transcript on h"]);

        let fake = FakeSsh::new();
        fake.on_host("h", Match::Any, Reply::fail(1, "boom"));
        let p = probed(&st, &fake, None).await;
        assert!(p.modes.iter().all(|m| m.ok));
        assert_eq!(p.warnings, vec!["could not check the transcript on h"]);

        // No answer line at all is not an answer either.
        let fake = FakeSsh::new();
        let p = probed(&st, &fake, None).await;
        assert!(p.modes.iter().all(|m| m.ok));
        assert_eq!(p.warnings.len(), 1);
    }

    #[tokio::test]
    async fn an_unreachable_host_wins_and_nothing_is_probed() {
        let (st, _) = fixture_with(CID);
        st.lock()
            .unwrap()
            .update_host_probe("h", false, None, None, 0)
            .unwrap();
        let fake = FakeSsh::new();
        let p = probed(&st, &fake, None).await;
        assert!(p.modes.iter().all(|m| !m.ok));
        assert!(p.modes[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("unreachable"));
        assert!(p.warnings.is_empty());
        assert!(fake.calls().is_empty(), "{:?}", fake.commands());
    }

    #[tokio::test]
    async fn only_a_planned_continue_is_probed() {
        let (st, pid) = fixture_with(CID);
        let fake = FakeSsh::new();
        // Another host: `last` is already off.
        st.lock().unwrap().upsert_host("g").unwrap();
        let p = probed(&st, &fake, Some("g")).await;
        assert!(!p.modes[0].ok);
        // Purged transcripts: `last` is already off.
        st.lock()
            .unwrap()
            .mark_purged_work_unresumable(pid, &["h".to_string()])
            .unwrap();
        probed(&st, &fake, None).await;
        assert!(fake.calls().is_empty(), "{:?}", fake.commands());

        // A brief or fresh resume never asks.
        let (st, _) = fixture_with(CID);
        let args = ResumeArgs {
            key: "ABC-1".into(),
            mode: "fresh".into(),
            ..Default::default()
        };
        let st2 = Arc::clone(&st);
        resume_with(&st, &fake, &args, &OrgScope::All, |a| async move {
            let s = st2.lock().unwrap();
            let id = s
                .upsert_session("dev-new", &a.host_alias, None, None, 1, 1, "running", None)
                .unwrap();
            Ok(s.get_session_by_id(id).unwrap().unwrap())
        })
        .await
        .unwrap();
        assert!(probe_calls(&fake).is_empty(), "{:?}", fake.commands());
    }

    #[tokio::test]
    async fn a_stored_id_that_is_no_uuid_is_never_sent_to_the_host() {
        let (st, _) = fixture();
        let fake = FakeSsh::new();
        let p = probed(&st, &fake, None).await;
        assert!(p.modes.iter().all(|m| m.ok));
        assert_eq!(p.warnings, vec!["could not check the transcript on h"]);
        assert!(fake.calls().is_empty(), "{:?}", fake.commands());
    }

    #[tokio::test]
    async fn the_recorded_transcript_path_is_checked_first() {
        let (st, _) = fixture_with(CID);
        let path = format!("/home/it's/.claude/projects/-p-o-r/{CID}.jsonl");
        {
            let s = st.lock().unwrap();
            // A dead row that still carries the conversation's record.
            let id = s
                .upsert_session("old", "h", None, None, 1, 1, "dead", None)
                .unwrap();
            s.conn_ref()
                .execute(
                    "INSERT INTO conversations (session_id, claude_session_id, transcript_path, started_at, start_source) \
                     VALUES (?1, ?2, ?3, 1, 'startup')",
                    rusqlite::params![id, CID, path],
                )
                .unwrap();
        }
        let fake = FakeSsh::new();
        fake.on_host(
            "h",
            Match::script_contains(TRANSCRIPT_PROBE_TAG),
            Reply::ok("fleet-transcript=present\n"),
        );
        let p = probed(&st, &fake, None).await;
        assert!(p.modes[0].ok, "{:?}", p.modes);
        let calls = probe_calls(&fake);
        let want = format!(
            r#"for f in '/home/it'\''s/.claude/projects'/'-p-o-r'/'{CID}.jsonl' "$HOME"/.claude/projects/*/'{CID}.jsonl'; do"#
        );
        assert!(calls[0].starts_with(&want), "{}", calls[0]);
    }

    #[test]
    fn the_transcript_path_is_built_only_from_validated_parts() {
        use TranscriptRoot::{Dir, Home};
        assert_eq!(
            transcript_path_expr(Home, Some("-home-u-p"), CID).unwrap(),
            format!("\"$HOME\"/.claude/projects/'-home-u-p'/'{CID}.jsonl'")
        );
        assert_eq!(
            transcript_path_expr(Home, None, CID).unwrap(),
            format!("\"$HOME\"/.claude/projects/*/'{CID}.jsonl'")
        );
        assert_eq!(
            transcript_path_expr(Dir("/home/u/.claude/projects"), Some("x"), CID).unwrap(),
            format!("'/home/u/.claude/projects'/'x'/'{CID}.jsonl'")
        );
        // Not a UUID.
        for id in [
            "c-1",
            "",
            "../../etc/passwd",
            "0F8FAD5B-D9CB-469F-A165-70867728950E",
            &format!("{CID}; rm -rf ~"),
        ] {
            assert!(transcript_path_expr(Home, None, id).is_err(), "{id:?}");
            assert!(transcript_probe_script(None, id).is_err(), "{id:?}");
        }
        // A slug that is not one path component.
        for slug in ["x/..", "/..", "..", ".", "", "a/b", "a\nb"] {
            assert!(
                transcript_path_expr(Home, Some(slug), CID).is_err(),
                "{slug:?}"
            );
        }
        // A root that is not a `.claude/projects` directory.
        for dir in [
            "/etc",
            "relative/.claude/projects",
            "/h/../.claude/projects",
            "/h/./.claude/projects",
        ] {
            assert!(
                transcript_path_expr(Dir(dir), None, CID).is_err(),
                "{dir:?}"
            );
        }
        // A recorded path that does not validate is ignored, never used.
        let script =
            transcript_probe_script(Some("/etc/x/.claude/projects/../../passwd"), CID).unwrap();
        assert!(!script.contains("passwd"), "{script}");
        assert!(!script.contains("/etc"), "{script}");
    }

    #[test]
    fn probe_output_is_read_by_its_answer_line_only() {
        assert_eq!(
            parse_transcript_probe("fleet-transcript=present\n"),
            TranscriptProbe::Present
        );
        assert_eq!(
            parse_transcript_probe("hi\nfleet-transcript=absent\n"),
            TranscriptProbe::Absent
        );
        assert_eq!(parse_transcript_probe(""), TranscriptProbe::Unknown);
        assert_eq!(
            parse_transcript_probe("fleet-transcript=maybe\n"),
            TranscriptProbe::Unknown
        );
    }

    /// The generated script run against a real directory tree, as `local`
    /// runs it: present when the file is there, absent when it is not.
    #[tokio::test]
    async fn the_probe_script_answers_on_a_real_tree() {
        let home = tempfile::tempdir().unwrap();
        let proj = home.path().join(".claude/projects/-p");
        std::fs::create_dir_all(&proj).unwrap();
        let run = |script: String| {
            let home = home.path().to_path_buf();
            async move {
                let out = tokio::process::Command::new("bash")
                    .args(["-c", &script])
                    .env("HOME", home)
                    .output()
                    .await
                    .unwrap();
                parse_transcript_probe(&String::from_utf8_lossy(&out.stdout))
            }
        };
        let script = transcript_probe_script(None, CID).unwrap();
        assert_eq!(run(script.clone()).await, TranscriptProbe::Absent);
        std::fs::write(proj.join(format!("{CID}.jsonl")), "{}").unwrap();
        assert_eq!(run(script).await, TranscriptProbe::Present);
    }

    /// The dialog sends `""` for a cleared textarea: that is no brief, so
    /// the plan builds one, and the resume answers the row it made rather
    /// than failing after the session exists.
    #[tokio::test]
    async fn a_blank_edited_brief_gets_the_built_one_and_never_fails_after_spawn() {
        assert_eq!(edited_brief(None), None);
        assert_eq!(edited_brief(Some("")), None);
        assert_eq!(edited_brief(Some(" \n\t")), None);
        assert_eq!(edited_brief(Some(" x ")).as_deref(), Some("x"));
        // On `local`, so the brief's git probe runs (and fails) here rather
        // than over SSH; the brief is built from the store either way.
        let (st, _) = fixture_on("local");
        let ssh = Arc::new(SshClient::new());
        let args = ResumeArgs {
            key: "abc-1".into(),
            mode: "brief".into(),
            brief: Some("   ".into()),
            ..Default::default()
        };
        let st2 = Arc::clone(&st);
        let (row, handover) = resume_with(&st, &ssh, &args, &OrgScope::All, |a| async move {
            let s = st2.lock().unwrap();
            let id = s
                .upsert_session("dev-new", &a.host_alias, None, None, 1, 1, "running", None)
                .unwrap();
            Ok(s.get_session_by_id(id).unwrap().unwrap())
        })
        .await
        .expect("the row, not an error after the spawn");
        assert!(handover.is_some(), "the plan's brief was queued");
        assert_eq!(
            row.work.as_ref().and_then(|w| w.key.as_deref()),
            Some("ABC-1")
        );
        let s = st.lock().unwrap();
        let queued = s.undelivered_handovers(row.id).unwrap();
        let body = queued[0].body.as_deref().unwrap_or_default();
        assert!(body.contains("ABC-1"), "built, not the blank: {body:?}");
    }

    /// Two resumes of one key in flight together (desktop and phone, a
    /// double click): the second is refused while the first is between
    /// its guards and its link, and once the first's session is live the
    /// plan offers Jump instead. One conversation, one session.
    #[tokio::test]
    async fn a_concurrent_resume_of_the_same_key_is_refused() {
        let (st, _) = fixture();
        let ssh = Arc::new(SshClient::new());
        let args = ResumeArgs {
            key: "abc-1".into(),
            mode: "last".into(),
            ..Default::default()
        };
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let st1 = Arc::clone(&st);
        let first = resume_with(&st, &ssh, &args, &OrgScope::All, |a| async move {
            // Parked mid-spawn (tmux is slow) until the second call answered.
            rx.await.unwrap();
            let s = st1.lock().unwrap();
            let id = s
                .upsert_session("dev-new", &a.host_alias, None, None, 1, 1, "running", None)
                .unwrap();
            s.set_claude_session_id(id, a.resume_claude_session_id.as_deref().unwrap())
                .unwrap();
            Ok(s.get_session_by_id(id).unwrap().unwrap())
        });
        let second = async {
            let err = resume_with(&st, &ssh, &args, &OrgScope::All, |_| async {
                panic!("the second resume never spawns")
            })
            .await
            .unwrap_err();
            tx.send(()).unwrap();
            err
        };
        let (first, err) = tokio::join!(first, second);
        assert_eq!(err.code, codes::E_EXISTS, "{}", err.message);
        assert!(err.message.contains("being resumed"), "{}", err.message);
        let (row, _) = first.expect("the first resume completes");
        assert_eq!(row.tmux_name, "dev-new");
        // Afterwards the key is live: a resume is blocked by the plan (Jump),
        // and the in-flight claim is released.
        let err = resume_with(&st, &ssh, &args, &OrgScope::All, |_| async {
            panic!("a live key never spawns")
        })
        .await
        .unwrap_err();
        assert!(err.message.contains("jump"), "{}", err.message);
        let key = store_key(&st.lock().unwrap());
        assert!(!IN_FLIGHT.lock().unwrap().iter().any(|(id, _)| *id == key));
    }

    /// The registry is keyed by the store's instance id, never its address:
    /// two stores never alias — not even one built where a dropped one was —
    /// and a claim is released on drop.
    #[test]
    fn the_in_flight_registry_never_aliases_two_stores() {
        let a = crate::store::Store::open_in_memory().unwrap();
        let first = InFlight::claim(&a, "ABC-1").unwrap();
        assert_eq!(
            InFlight::claim(&a, "ABC-1").unwrap_err().code,
            codes::E_EXISTS
        );
        let b = crate::store::Store::open_in_memory().unwrap();
        assert_ne!(a.instance_id(), b.instance_id());
        let _other = InFlight::claim(&b, "ABC-1").expect("another store's key is not this one's");
        drop(first);
        let _again = InFlight::claim(&a, "ABC-1").expect("released on drop");
        let a_id = a.instance_id();
        drop(_again);
        drop(a);
        let c = crate::store::Store::open_in_memory().unwrap();
        assert_ne!(c.instance_id(), a_id, "an id is never reused");
        let _fresh = InFlight::claim(&c, "ABC-1").expect("a new store starts clean");
    }

    #[test]
    fn the_start_prompt_is_never_typed_into_a_dialog() {
        assert_eq!(
            prompt_gate("Do you trust the files in this folder?\n❯ 1. Yes, proceed\n  2. No, exit"),
            PromptGate::TrustDialog
        );
        assert_eq!(prompt_gate("Loading…"), PromptGate::NotYet);
        assert_eq!(
            prompt_gate("╭────╮\n│ >  │\n╰────╯\n  ? for shortcuts"),
            PromptGate::Ready
        );
    }
}
