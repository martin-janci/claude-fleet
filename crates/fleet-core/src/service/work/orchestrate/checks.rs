//! Headless checks (F3): a mission's `review` and `test` runs as one
//! locked-down `claude -p` run in the implementer's checkout, instead of an
//! interactive session in a checkout of their own. Off by default
//! (`orchestrator.headless_checks`); whatever this cannot take falls back to
//! the session path ([`super::super::run::run_item_with`]).
//!
//! * **Where.** The checkout the item's latest finished implementation left
//!   ([`implementer_checkout`], as `integrate::branches` finds it): its host,
//!   its worktree, the commit it ended on. A preflight over SSH checks the
//!   directory is there, still at that commit and clean, and that `claude`
//!   is on the host; anything else is the session path.
//! * **What it may do.** A review reads ([`claude_print::tool_flags`] with no
//!   command): Read, Grep and Glob inside the checkout, and the commits and
//!   diff fleet put in its prompt. A test also runs exactly the item's
//!   `test:<command>` lines, each pre-approved as `Bash(<command>)`; a
//!   command that is not a plain one ([`plain_command`]) is the session
//!   path. Every other tool is denied (`--permission-mode dontAsk`): nobody
//!   is there to answer a prompt.
//! * **The result.** The same task row and report as a session's run
//!   (`role`, `attempt`, the done marker and its JSON block), so
//!   `verify::check_line` and the loop's next steps read it unchanged. The
//!   task has no worker session; the liveness sweep fails it after
//!   [`DEADLINE_SECS`] ([`crate::service::tasks::liveness_verdict`]).
//! * **The cost.** Booked in `aux_usage` (origin `headless_check`) with the
//!   mission, so the grant's budget counts it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::Deps;
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::claude_print::{self, Envelope};
use crate::service::headless::{self, HeadlessJob, RunAt, ToolPolicy};
use crate::service::settings;
use crate::service::view_scope::ViewScope;
use crate::service::work::report;
use crate::service::work::verify::{self, Cond};
use crate::shell::quote;
use crate::store::{MissionRow, Store, TaskRow};

/// The tag the scripts print before their verdict.
pub const TAG: &str = "fleet-check=";
/// The preflight's verdicts, and the run's.
const VERDICTS: &[&str] = &["nodir", "moved", "dirty", "noclaude", "ok", "run"];
/// One review's and one test run's host-side wall clock.
pub const REVIEW_SECS: u64 = 900;
pub const TEST_SECS: u64 = 1_800;
/// When the liveness sweep gives up on a headless check that never
/// finished (a restart lost its task): past the longest run, with room.
pub const DEADLINE_SECS: i64 = TEST_SECS as i64 + 600;
/// Bytes of the model's output the script lets through.
const OUTPUT_CAP: usize = 65_536;
/// Bytes of the implementer's diff a review's prompt carries.
const DIFF_CAP: usize = 200_000;
/// Longest test command taken headless.
const COMMAND_MAX: usize = 200;
const CONNECT: Duration = Duration::from_secs(15);

/// Where the implementation is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    pub host: String,
    pub path: String,
    /// The commit the implementation finished on.
    pub head: String,
    /// What its branch was compared against, when evidence recorded it.
    pub merge_base: Option<String>,
}

fn is_sha(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The checkout of `item_id`'s latest finished implementation: done, not a
/// failed attempt, with a commit, on a session and worktree fleet still
/// knows. `None` when there is none.
pub fn implementer_checkout(s: &Store, item_id: i64) -> Option<Checkout> {
    let t = s.latest_item_task(item_id, "implement").ok()??;
    if t.state != "done" || super::steps::attempt_failed(&t) {
        return None;
    }
    let ev = t.evidence.as_ref()?;
    let head = ev.head.clone().filter(|h| is_sha(h))?;
    let w = s.get_session_by_id(t.worker_session_id?).ok()??;
    let wt = s.get_worktree_row(w.worktree_id?).ok()??;
    Some(Checkout {
        host: w.host_alias,
        path: wt.path,
        head,
        merge_base: ev.merge_base.clone().filter(|b| is_sha(b)),
    })
}

/// PURE: whether `cmd` may be pre-approved as `Bash(<cmd>)`: one line of
/// words, paths and flags, no shell syntax (no quote, pipe, redirect,
/// substitution, glob or separator), at most [`COMMAND_MAX`] bytes.
pub fn plain_command(cmd: &str) -> bool {
    let c = cmd.trim();
    !c.is_empty()
        && c == cmd
        && c.len() <= COMMAND_MAX
        && c.chars()
            .all(|ch| ch.is_ascii_alphanumeric() || " -_./=:,@+%".contains(ch))
        && !c.contains("  ")
}

/// The item's `test:<command>` lines. `None` when it has none, or one is
/// not a [`plain_command`]: the session path then runs them.
pub fn test_commands(s: &Store, item_id: i64) -> Option<Vec<String>> {
    let item = s.get_work_item(item_id).ok()??;
    let lines = verify::lines_for(s, &item).ok()?;
    let mut cmds = Vec::new();
    for l in lines {
        if let Cond::Test(c) = verify::parse_cond(&l) {
            let c = c?;
            if !plain_command(&c) {
                return None;
            }
            if !cmds.contains(&c) {
                cmds.push(c);
            }
        }
    }
    (!cmds.is_empty()).then_some(cmds)
}

/// PURE: what the run is asked, as the task stores it (the done marker's
/// instruction is added at delivery, [`report::with_run_instruction`]).
/// `context` is the step's (the loop's own words or an earlier run's),
/// quoted.
pub fn check_prompt(
    role: &str,
    label: &str,
    notes: Option<&str>,
    cmds: &[String],
    context: Option<&str>,
) -> String {
    let mut p = match role {
        "test" => format!(
            "You test the finished work on {label}. You are in its checkout. Run exactly \
             these commands, each once, as written, and nothing else:\n{}\n\nReport outcome \
             done only when every one passed, and list each command in tests_run exactly as \
             written. You cannot edit files.",
            cmds.iter()
                .map(|c| format!("- {c}"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        _ => format!(
            "You review the finished work on {label}. You are in its checkout; its commits \
             and diff follow these instructions. Read whatever else you need. You cannot edit \
             or run anything. Judge whether the change does what the work item asks, \
             correctly and completely. Report outcome done when it is ready to merge, else \
             partial or failed, with what must change as blockers."
        ),
    };
    if let Some(n) = notes.map(str::trim).filter(|n| !n.is_empty()) {
        p.push_str(&format!(
            "\n\nThe work item says (its words, quoted):\n> {}",
            n.replace('\n', "\n> ")
        ));
    }
    if let Some(c) = context.map(str::trim).filter(|c| !c.is_empty()) {
        p.push_str(&format!(
            "\n\nWhat the mission's earlier runs left (their words, quoted):\n> {}",
            c.replace('\n', "\n> ")
        ));
    }
    p
}

/// PURE: the checkout's preflight lines: in the directory, at `head`, clean,
/// `claude` there. Each failure prints its verdict and stops the script.
fn preflight(at: &Checkout) -> String {
    format!(
        "cd -- {path} 2>/dev/null || {{ echo {TAG}nodir; exit 0; }}; \
         [ \"$(git rev-parse HEAD 2>/dev/null)\" = {head} ] || {{ echo {TAG}moved; exit 0; }}; \
         [ -z \"$(git status --porcelain 2>/dev/null)\" ] || {{ echo {TAG}dirty; exit 0; }}; \
         {noclaude}",
        path = quote(&at.path),
        head = quote(&at.head),
        noclaude = headless::default_agent().missing_check(TAG),
    )
}

/// PURE: the preflight alone, answering `<tag>ok` when the run could start.
pub fn preflight_script(at: &Checkout) -> String {
    format!("{} echo {TAG}ok", preflight(at))
}

/// PURE: the run: the preflight again, the prompt (and, for a review, the
/// commits and diff) in a temporary file on stdin, the login profile's
/// environment, then the locked-down `claude -p`.
pub fn check_script(
    role: &str,
    at: &Checkout,
    model: &str,
    profile: Option<&str>,
    cmds: &[String],
    prompt: &str,
) -> Result<String, String> {
    if !settings::SUMMARY_MODELS.contains(&model) {
        return Err(format!("refusing check model {model:?}"));
    }
    if cmds.iter().any(|c| !plain_command(c)) {
        return Err("a test command is not a plain one".into());
    }
    let agent = headless::default_agent();
    let env = match profile {
        Some(_) => format!(
            "{}{}",
            agent.login_env(profile)?,
            crate::tmux::PROFILE_API_KEY
        ),
        None => String::new(),
    };
    let bash: Vec<&str> = cmds.iter().map(String::as_str).collect();
    let claude = agent.command(&HeadlessJob {
        model,
        stdin_file: Some("d"),
        tools: ToolPolicy::ReadOnly { bash: &bash },
        ..Default::default()
    });
    let extra = match (role, &at.merge_base) {
        ("review", Some(base)) => format!(
            " echo; echo '<commits>'; git log --oneline -n 50 {r}; echo '</commits>'; \
             echo '<diff>'; git diff --stat {r}; git diff {r} | head -c {DIFF_CAP}; echo '</diff>';",
            r = quote(&format!("{base}..HEAD")),
        ),
        ("review", None) => " echo; echo '<commits>'; git log --oneline -n 20; echo '</commits>';"
            .to_string(),
        _ => String::new(),
    };
    let secs = if role == "test" {
        TEST_SECS
    } else {
        REVIEW_SECS
    };
    Ok(format!(
        "{pre} d=$(mktemp); trap 'rm -f \"$d\"' EXIT; \
         {{ printf '%s\\n' {prompt};{extra} }} >\"$d\" 2>/dev/null || true; \
         {env}{run}",
        pre = preflight(at),
        prompt = quote(prompt),
        run = claude_print::run_capped(TAG, &claude, secs, OUTPUT_CAP, false),
    ))
}

/// A check the loop cleared to run headless.
pub struct Plan {
    pub at: Checkout,
    pub model: String,
    pub profile: Option<String>,
    pub cmds: Vec<String>,
    pub label: String,
    pub notes: Option<String>,
    pub org_id: Option<i64>,
}

/// Whether `role` on `item_id` runs headless, and how. `None`: the setting
/// is off, the role is not a check, there is no checkout to run in, the
/// grant's hosts leave its host out, or a test has no plain command.
pub fn plan(
    s: &Store,
    item_id: i64,
    role: &str,
    profile: Option<&str>,
    hosts: Option<&[String]>,
) -> Option<Plan> {
    if !matches!(role, "review" | "test")
        || !settings::get_bool(s, settings::ORCHESTRATOR_HEADLESS_CHECKS)
    {
        return None;
    }
    let at = implementer_checkout(s, item_id)?;
    if hosts.is_some_and(|h| !h.contains(&at.host)) {
        return None;
    }
    let cmds = match role {
        "test" => test_commands(s, item_id)?,
        _ => Vec::new(),
    };
    let item = s.get_work_item(item_id).ok()??;
    Some(Plan {
        model: settings::get_string(s, settings::ORCHESTRATOR_HEADLESS_MODEL),
        profile: profile.map(str::to_string),
        cmds,
        label: match &item.key {
            Some(k) => format!("{k} {}", item.title),
            None => item.title.clone(),
        },
        notes: item.notes.clone(),
        org_id: s.item_org(item_id).ok().flatten(),
        at,
    })
}

/// Start `role` on `item_id` headless, when [`plan`] and the preflight
/// allow: `Ok(None)` sends the step down the session path. The task is
/// recorded before this returns; the run, its booking and its result
/// happen in a task of their own.
#[allow(clippy::too_many_arguments)]
pub async fn try_start(
    deps: &Deps,
    m: &MissionRow,
    item_id: i64,
    role: &str,
    profile: Option<&str>,
    hosts: Option<&[String]>,
    view: &ViewScope,
    context: Option<&str>,
) -> Result<Option<(String, i64)>, IpcError> {
    let Some(p) = plan(&*lock(&deps.store)?, item_id, role, profile, hosts) else {
        return Ok(None);
    };
    let pre = crate::ssh::run_shell_bounded(
        deps.ssh.as_ref(),
        &p.at.host,
        &preflight_script(&p.at),
        CONNECT,
        Duration::from_secs(60),
    )
    .await;
    let ok = pre.is_ok_and(|o| {
        claude_print::parse_tagged(&String::from_utf8_lossy(&o.stdout), TAG, VERDICTS)
            .is_some_and(|(v, _)| v == "ok")
    });
    if !ok {
        return Ok(None);
    }
    let prompt = check_prompt(role, &p.label, p.notes.as_deref(), &p.cmds, context);
    let task = {
        let s = lock(&deps.store)?;
        match super::super::run::precheck(&s, view, item_id, role)? {
            super::super::run::Precheck::Existing(t) => {
                return Ok(Some((format!("a {role} run was already open"), t.id)))
            }
            super::super::run::Precheck::Fresh { attempt } => {
                let Some(_claim) = super::super::run::claim_start(&deps.store, item_id, role)
                else {
                    return Err(IpcError::new(
                        codes::E_INVALID_STATE,
                        format!("a {role} run on this item is already starting"),
                    ));
                };
                super::super::run::record_headless_run(&s, &prompt, item_id, attempt, role)?
            }
        }
    };
    let prompt = report::with_run_instruction(&prompt, &task.nonce);
    let store = Arc::clone(&deps.store);
    let ssh = Arc::clone(&deps.ssh);
    let mission = m.id;
    let role_owned = role.to_string();
    let id = task.id;
    crate::rt::spawn(async move {
        let res = run(&store, ssh.as_ref(), mission, &role_owned, &p, &prompt).await;
        finish(&store, &task, res);
    });
    Ok(Some((format!("started a headless {role} run"), id)))
}

/// Run the check and read its reply text.
async fn run(
    store: &Arc<Mutex<Store>>,
    exec: &dyn crate::ssh::SshExec,
    mission: i64,
    role: &str,
    p: &Plan,
    prompt: &str,
) -> Result<String, IpcError> {
    let script = check_script(role, &p.at, &p.model, p.profile.as_deref(), &p.cmds, prompt)
        .map_err(|e| IpcError::new(codes::E_INVALID, e))?;
    let secs = if role == "test" {
        TEST_SECS
    } else {
        REVIEW_SECS
    };
    let out = crate::ssh::run_shell_bounded(
        exec,
        &p.at.host,
        &script,
        CONNECT,
        Duration::from_secs(secs + 60),
    )
    .await?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let text = match claude_print::parse_tagged(&stdout, TAG, VERDICTS) {
        Some(("run", t)) if !t.is_empty() => t,
        Some(("run", _)) if out.status.code() == Some(124) => {
            return Err(IpcError::new(
                codes::E_TIMEOUT,
                format!("the {role} run took longer than {secs}s"),
            ))
        }
        Some((v @ ("nodir" | "moved" | "dirty"), _)) => {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!("the implementer's checkout changed before the {role} ran ({v})"),
            ))
        }
        Some(("noclaude", _)) => {
            return Err(IpcError::new(
                codes::E_CLAUDE_CLI,
                format!("claude is not on {}'s login PATH", p.at.host),
            ))
        }
        _ if claude_print::run_signed_out(None, "", &out.stderr) => {
            return Err(claude_print::signed_out_error(
                &p.at.host,
                p.profile.as_deref(),
            ))
        }
        _ => {
            return Err(IpcError::new(
                codes::E_CLAUDE_CLI,
                format!("the {role} run failed: claude said nothing"),
            ))
        }
    };
    let what = format!("the headless {role} run");
    let (text, _) = headless::read_reply(
        headless::default_agent(),
        text,
        &out.stderr,
        RunAt {
            host: &p.at.host,
            profile: p.profile.as_deref(),
            what: &what,
        },
        |usage| book(store, mission, p, usage),
    )?;
    Ok(text)
}

fn book(store: &Mutex<Store>, mission: i64, p: &Plan, usage: Option<&Envelope>) {
    let row = crate::store::NewAuxUsage {
        origin: crate::store::AUX_ORIGIN_HEADLESS_CHECK,
        host_alias: p.at.host.clone(),
        model: p.model.clone(),
        mission_id: Some(mission),
        org_id: p.org_id,
        claude_session_id: None,
        input_tokens: usage.and_then(|u| u.input_tokens),
        output_tokens: usage.and_then(|u| u.output_tokens),
        cost_micros: usage.and_then(|u| u.cost_microusd).unwrap_or(0),
        at: crate::store::now_unix(),
    };
    if let Err(e) = lock(store).and_then(|s| s.insert_aux_usage(&row)) {
        tracing::warn!(error = %e.message, "[checks] cost not booked");
    }
}

/// Close the task with the run's report, or fail it with why there is
/// none. The mission wakes either way ([`crate::service::tasks`]).
fn finish(store: &Mutex<Store>, task: &TaskRow, res: Result<String, IpcError>) {
    let Ok(s) = lock(store) else { return };
    let done = match res {
        Ok(text) => {
            let report =
                report::report_in(&[&text], &task.nonce).or_else(|| report::parse_report(&text));
            match report {
                Some(r) => {
                    let result = crate::service::tasks::scan_for_done(&text, &task.nonce)
                        .unwrap_or_default();
                    crate::service::tasks::complete_task_reported(&s, task, &result, Some(&r))
                        .map(|_| ())
                }
                None => {
                    crate::service::tasks::fail_task(&s, task.id, "the headless run gave no report")
                        .map(|_| ())
                }
            }
        }
        Err(e) => crate::service::tasks::fail_task(&s, task.id, &e.message).map(|_| ()),
    };
    if let Err(e) = done {
        tracing::warn!(task = task.id, error = %e.message, "[checks] the task was not closed");
    }
}

#[cfg(test)]
mod tests;
