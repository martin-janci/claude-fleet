//! A run's result (orchestration O3, design 2026-10-07 §6): the report the
//! worker is asked for, and the evidence fleet reads from git itself.
//!
//! - **The report.** A run's prompt asks the worker to call the
//!   `task_report` tool with its report, and to follow its done marker with
//!   the same report as one fenced JSON block (`summary`, `outcome`, `done`,
//!   `decisions`, `remaining`, `context`, `tests_run`, `warnings`,
//!   `blockers`, `followups`, `confidence`). [`parse_report`] is
//!   tolerant: a missing or broken block leaves the paragraph in
//!   `tasks.result` as before and no report. What it parses is capped and
//!   stored as `tasks.result_json`; it is what the worker said, and nothing
//!   takes it for proof.
//! - **The evidence.** When a run finishes, fleet asks git in the worker's
//!   checkout for the commits and the changed files against the base
//!   ([`evidence_script`] / [`parse_evidence`]), and stores that as
//!   `tasks.evidence_json`. Best-effort: a host that cannot answer leaves an
//!   `error`, and nothing waits on it.

use std::sync::{Arc, Mutex};

use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::{
    EvidenceCommit, EvidenceFile, Store, TaskEvidence, TaskReport, TaskRow, EVIDENCE_COMMITS_MAX,
    EVIDENCE_FILES_MAX, REPORT_ENTRY_MAX_CHARS, REPORT_LIST_MAX, REPORT_OUTCOMES,
};

/// Longest summary a report keeps (the paragraph's cap).
pub const SUMMARY_MAX_CHARS: usize = 4_000;
/// Most lines after the marker searched for the JSON block.
const REPORT_SCAN_LINES: usize = 400;

/// The instruction appended to a run's prompt: the `task_report` tool call
/// first, then the marker and the block as the fallback for a worker without
/// fleet's MCP. The marker stands inline in the sentence, so the prompt's
/// echo in the pane never reads as the worker's emission.
pub fn run_instruction(task_id: i64, nonce: &str) -> String {
    format!(
        "When finished, call the claude-fleet MCP tool {} with task_id {task_id}, \
         nonce \"{nonce}\" and your report: \"summary\" (one paragraph), \"outcome\" \
         (done | partial | blocked | failed), \"done\" (what you did), \
         \"decisions\" (choices the next task must keep), \"remaining\" (work \
         left), \"context\" (facts a fresh session needs to continue), \
         \"tests_run\" (the commands you ran), \"warnings\", \"blockers\", \
         \"followups\" (each a task title) — all lists of short strings — and \
         \"confidence\" (low | medium | high). Then also print exactly {} on its \
         own line, followed by the same report as one fenced ```json block: fleet \
         reads that when the tool is not available. Fleet reads your commits and \
         changed files from git itself; do not list them.",
        crate::service::tasks::REPORT_TOOL,
        crate::service::tasks::done_marker(nonce)
    )
}

/// A run's prompt as delivered: the text, a blank line, the instruction.
pub fn with_run_instruction(prompt: &str, task_id: i64, nonce: &str) -> String {
    format!(
        "{}\n\n{}",
        prompt.trim_end(),
        run_instruction(task_id, nonce)
    )
}

fn clip(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect()
}

fn list(v: Option<&serde_json::Value>) -> Vec<String> {
    let Some(v) = v else { return Vec::new() };
    let items: Vec<&serde_json::Value> = match v {
        serde_json::Value::Array(a) => a.iter().collect(),
        other => vec![other],
    };
    items
        .into_iter()
        .filter_map(|x| match x {
            serde_json::Value::String(s) => Some(clip(s, REPORT_ENTRY_MAX_CHARS)),
            serde_json::Value::Null => None,
            other => Some(clip(&other.to_string(), REPORT_ENTRY_MAX_CHARS)),
        })
        .filter(|s| !s.is_empty())
        .take(REPORT_LIST_MAX)
        .collect()
}

/// PURE: a report from the JSON a worker printed, capped. `None` when the
/// value is not an object.
pub fn report_from_value(v: &serde_json::Value) -> Option<TaskReport> {
    let o = v.as_object()?;
    let outcome = o
        .get("outcome")
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| REPORT_OUTCOMES.contains(&s.as_str()))
        .unwrap_or_else(|| "partial".to_string());
    let confidence = match o.get("confidence") {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => Some(clip(s, 20)),
        Some(serde_json::Value::Number(n)) => Some(clip(&n.to_string(), 20)),
        _ => None,
    };
    Some(TaskReport {
        summary: o
            .get("summary")
            .and_then(|x| x.as_str())
            .map(|s| clip(s, SUMMARY_MAX_CHARS))
            .unwrap_or_default(),
        outcome,
        done: list(o.get("done")),
        decisions: list(o.get("decisions")),
        remaining: list(o.get("remaining")),
        context: list(o.get("context")),
        tests_run: list(o.get("tests_run")),
        warnings: list(o.get("warnings")),
        blockers: list(o.get("blockers")),
        followups: list(o.get("followups")),
        confidence,
    })
}

/// Strip the chrome a captured pane puts before assistant text, keeping
/// the JSON punctuation a line may start with.
fn unchrome(line: &str) -> &str {
    let t = line.trim_start();
    let t = t
        .strip_prefix('⏺')
        .or_else(|| t.strip_prefix('│'))
        .unwrap_or(t);
    t.trim_start()
}

/// PURE: the report in the text that follows a done marker: the first
/// fenced block (```json or bare ```), or a bare `{ … }` when the worker
/// left the fence out. `None` when there is none or it does not parse.
pub fn parse_report(after_marker: &str) -> Option<TaskReport> {
    let lines: Vec<&str> = after_marker.lines().take(REPORT_SCAN_LINES).collect();
    let open = lines.iter().position(|l| {
        let t = unchrome(l);
        t.starts_with("```") || t.starts_with('{')
    })?;
    let first = unchrome(lines[open]);
    let body: String = if first.starts_with("```") {
        let close = lines[open + 1..]
            .iter()
            .position(|l| unchrome(l).starts_with("```"))
            .map(|i| open + 1 + i)
            .unwrap_or(lines.len());
        lines[open + 1..close]
            .iter()
            .map(|l| unchrome(l))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        lines[open..]
            .iter()
            .map(|l| unchrome(l))
            .collect::<Vec<_>>()
            .join("\n")
    };
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body.trim()) {
        return report_from_value(&v);
    }
    // A bare object followed by more text: parse its first value only.
    let mut de = serde_json::Deserializer::from_str(body.trim()).into_iter::<serde_json::Value>();
    de.next()?.ok().as_ref().and_then(report_from_value)
}

/// PURE: the text after the LAST line that is exactly the task's marker
/// (chrome aside), or `None` before the worker emitted it.
pub fn after_marker<'a>(text: &'a str, nonce: &str) -> Option<&'a str> {
    let marker = crate::service::tasks::done_marker(nonce);
    let mut found = None;
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        let stripped = bare.trim_start_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '_'));
        if stripped.trim_end() == marker {
            found = Some(offset + line.len());
        }
        offset += line.len();
    }
    found.map(|at| &text[at.min(text.len())..])
}

/// PURE: the report after `nonce`'s marker in the first source that has
/// the marker (the transcript, then the pane).
pub fn report_in(sources: &[&str], nonce: &str) -> Option<TaskReport> {
    sources
        .iter()
        .find_map(|src| after_marker(src, nonce))
        .and_then(parse_report)
}

/// Output line prefixes of [`evidence_script`].
const EV_HEAD: &str = "__FLEET_EV_HEAD__\t";
const EV_COMMIT: &str = "__FLEET_EV_C__\t";
const EV_COMMITS: &str = "__FLEET_EV_NC__\t";
const EV_FILE: &str = "__FLEET_EV_F__\t";
const EV_FILES: &str = "__FLEET_EV_NF__\t";
const EV_DIRTY: &str = "__FLEET_EV_DIRTY__\t";
const EV_ERR: &str = "__FLEET_EV_ERR__\t";

/// The git commands that read a checkout's evidence, as one script. `cwd`
/// is shell-quoted. The base is `origin/HEAD`'s target, else `origin/main`,
/// else `origin/master`; the comparison is against the merge base, so a
/// base that moved on since the branch was cut adds nothing.
pub fn evidence_script(cwd: &str) -> String {
    evidence_script_since(cwd, None)
}

/// [`evidence_script`] from `since`, the commit a task began on in a
/// checkout it shares with the tasks before it (lanes design L3, §3.3):
/// its evidence is `since..HEAD`, never what the tasks before it did. A
/// `since` that is no longer an ancestor of `HEAD` (the history was
/// rewritten) is an error, not a guess. `None` reads against the merge
/// base, as for a run in a checkout of its own.
pub fn evidence_script_since(cwd: &str, since: Option<&str>) -> String {
    let base = match since.filter(|s| crate::store::is_commit_id(s)) {
        Some(sha) => format!(
            "base={sha}\n\
             head=$(git rev-parse HEAD 2>/dev/null) || {{ printf '{err}not a git checkout\\n'; exit 0; }}\n\
             git merge-base --is-ancestor {sha} HEAD 2>/dev/null || {{ printf '{err}the task began on %s, which the branch no longer holds\\n' {sha}; exit 0; }}\n\
             mb={sha}\n",
            err = EV_ERR,
        ),
        None => format!(
            "base=$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null)\n\
             [ -n \"$base\" ] || {{ git rev-parse -q --verify origin/main >/dev/null 2>&1 && base=origin/main; }}\n\
             [ -n \"$base\" ] || {{ git rev-parse -q --verify origin/master >/dev/null 2>&1 && base=origin/master; }}\n\
             [ -n \"$base\" ] || {{ printf '{err}no base branch\\n'; exit 0; }}\n\
             head=$(git rev-parse HEAD 2>/dev/null) || {{ printf '{err}not a git checkout\\n'; exit 0; }}\n\
             mb=$(git merge-base HEAD \"$base\" 2>/dev/null) || {{ printf '{err}no merge base with %s\\n' \"$base\"; exit 0; }}\n",
            err = EV_ERR,
        ),
    };
    format!(
        "cd {cwd} 2>/dev/null || {{ printf '{err}no checkout\\n'; exit 0; }}\n\
         {base}\
         printf '{head}%s\\t%s\\t%s\\n' \"$head\" \"$base\" \"$mb\"\n\
         printf '{nc}%s\\n' \"$(git rev-list --count \"$mb\"..HEAD 2>/dev/null)\"\n\
         git log --no-merges --format='{commit}%H%x09%s' -n {cmax} \"$mb\"..HEAD 2>/dev/null\n\
         printf '{nf}%s\\n' \"$(git diff --name-only \"$mb\" HEAD 2>/dev/null | wc -l)\"\n\
         git diff --numstat \"$mb\" HEAD 2>/dev/null | head -n {fmax} | sed 's/^/{file}/'\n\
         printf '{dirty}%s\\n' \"$(git status --porcelain --untracked-files=no 2>/dev/null | wc -l)\"\n",
        cwd = quote(cwd),
        err = EV_ERR,
        head = EV_HEAD,
        nc = EV_COMMITS,
        commit = EV_COMMIT,
        cmax = EVIDENCE_COMMITS_MAX,
        nf = EV_FILES,
        fmax = EVIDENCE_FILES_MAX,
        file = EV_FILE,
        dirty = EV_DIRTY,
    )
}

fn count(s: &str) -> Option<u32> {
    s.trim().parse().ok()
}

/// PURE: [`evidence_script`]'s output as evidence read at `now`.
pub fn parse_evidence(stdout: &str, now: i64) -> TaskEvidence {
    let mut ev = TaskEvidence {
        at: now,
        ..Default::default()
    };
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix(EV_ERR) {
            ev.error = Some(clip(rest, 200));
        } else if let Some(rest) = line.strip_prefix(EV_HEAD) {
            let mut f = rest.split('\t');
            ev.head = f.next().map(str::to_string).filter(|s| !s.is_empty());
            ev.base = f.next().map(str::to_string).filter(|s| !s.is_empty());
            ev.merge_base = f.next().map(str::to_string).filter(|s| !s.is_empty());
        } else if let Some(rest) = line.strip_prefix(EV_COMMITS) {
            ev.commits_total = count(rest).unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix(EV_COMMIT) {
            if ev.commits.len() < EVIDENCE_COMMITS_MAX {
                let (sha, subject) = rest.split_once('\t').unwrap_or((rest, ""));
                ev.commits.push(EvidenceCommit {
                    sha: sha.to_string(),
                    subject: clip(subject, 200),
                });
            }
        } else if let Some(rest) = line.strip_prefix(EV_FILES) {
            ev.files_total = count(rest).unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix(EV_FILE) {
            let mut f = rest.splitn(3, '\t');
            let (added, removed, path) = (f.next(), f.next(), f.next());
            if let Some(path) = path.filter(|p| !p.is_empty()) {
                if ev.files.len() < EVIDENCE_FILES_MAX {
                    ev.files.push(EvidenceFile {
                        path: clip(path, 300),
                        added: added.and_then(count),
                        removed: removed.and_then(count),
                    });
                }
            }
        } else if let Some(rest) = line.strip_prefix(EV_DIRTY) {
            ev.uncommitted = count(rest);
        }
    }
    ev.commits_total = ev.commits_total.max(ev.commits.len() as u32);
    ev.files_total = ev.files_total.max(ev.files.len() as u32);
    if ev.head.is_none() && ev.error.is_none() {
        ev.error = Some("git gave no answer".into());
    }
    ev
}

/// Read and store the evidence of every finished run in `tasks`, from the
/// worker's checkout at `cwd`. Best-effort and logged: the attempt is
/// already done, and its evidence only explains it.
pub async fn collect_evidence(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    host: &str,
    cwd: &str,
    tasks: &[TaskRow],
) {
    collect_evidence_as(store, ssh, host, cwd, tasks, EvidenceRead::Final).await;
}

/// When a git read of a run's checkout happens, which says what it may
/// replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceRead {
    /// After the worker's turn (its Stop): the run's evidence.
    Final,
    /// While the worker's turn still runs (`task_report`): stored
    /// provisional, never over a settled read, and read again at its Stop.
    MidTurn,
    /// The mission loop reading a failed read again: only over a read that
    /// is still unsettled or failed.
    Retry,
}

/// The longest one retried git read may take. The retry runs in the loop's
/// pass, so a host that swallows packets costs this, not ssh's own minute.
const EVIDENCE_RETRY_WALL_CLOCK: std::time::Duration = std::time::Duration::from_secs(20);

/// The most one pass of the loop spends on retried reads; the rest wait
/// for the next pass.
const EVIDENCE_RETRY_PASS: std::time::Duration = std::time::Duration::from_secs(45);

/// [`collect_evidence`], for `read`; answers whether the host answered (a
/// failed read is stored either way).
pub async fn collect_evidence_as(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    host: &str,
    cwd: &str,
    tasks: &[TaskRow],
    read: EvidenceRead,
) -> bool {
    let runs: Vec<i64> = tasks
        .iter()
        .filter(|t| t.work_item_id.is_some())
        .map(|t| t.id)
        .collect();
    if runs.is_empty() {
        return true;
    }
    // A task that began part way along a shared checkout (a lane's) is
    // read from where it began; the others against the merge base, once.
    let since: Vec<(i64, Option<String>)> = {
        let Ok(s) = store.lock() else { return false };
        runs.iter()
            .map(|&id| (id, s.task_start_head(id).ok().flatten()))
            .collect()
    };
    let mut answered = true;
    let mut done: Vec<(Option<String>, TaskEvidence)> = Vec::new();
    for (id, from) in since {
        let ev = match done.iter().find(|(f, _)| *f == from) {
            Some((_, ev)) => ev.clone(),
            None => {
                let script = evidence_script_since(cwd, from.as_deref());
                let answer = match read {
                    EvidenceRead::Retry => {
                        crate::service::catalog::inventory::run_host_script_with(
                            ssh,
                            host,
                            &script,
                            EVIDENCE_RETRY_WALL_CLOCK,
                            &tokio_util::sync::CancellationToken::new(),
                        )
                        .await
                    }
                    _ => {
                        crate::service::catalog::inventory::run_host_script(ssh, host, &script)
                            .await
                    }
                };
                answered &= answer.is_ok();
                let mut ev = match answer {
                    Ok(out) => parse_evidence(&out, now_unix()),
                    // Where it was read, so the loop can read it again (a
                    // host that dropped for a moment is not a run that did
                    // nothing).
                    Err(e) => TaskEvidence {
                        at: now_unix(),
                        error: Some(clip(&e.message, 200)),
                        host: Some(host.to_string()),
                        dir: Some(cwd.to_string()),
                        ..Default::default()
                    },
                };
                ev.provisional = read == EvidenceRead::MidTurn;
                done.push((from, ev.clone()));
                ev
            }
        };
        let Ok(s) = store.lock() else {
            return answered;
        };
        let stored = match read {
            EvidenceRead::Final => s.set_task_evidence(id, &ev),
            EvidenceRead::MidTurn => s.replace_open_task_evidence(id, &ev, false).map(|_| ()),
            // A retry keeps what it replaces open or settled: a failed
            // mid-turn read read again is still owed the worker's Stop, so
            // a commit made after it counts.
            EvidenceRead::Retry => {
                let mut ev = ev.clone();
                ev.provisional = s
                    .get_task(id)
                    .ok()
                    .flatten()
                    .and_then(|t| t.evidence)
                    .is_some_and(|e| e.provisional);
                s.replace_open_task_evidence(id, &ev, true).map(|_| ())
            }
        };
        if let Err(e) = stored {
            tracing::warn!(task = id, error = %e.message, "[report] storing evidence failed");
        }
    }
    answered
}

/// The least time between two reads of one run's failed git evidence.
const EVIDENCE_RETRY_EVERY_SECS: i64 = 60;

/// Read again the git evidence of every run whose read failed (a transient
/// SSH error right after the run), within [`super::gate::EVIDENCE_RETRY_SECS`]
/// of its finish: the gate holds it `pending` meanwhile rather than calling
/// the work missing. Any run, in a mission or not: the missions' loop calls
/// it on every pass, outside any mission's lease. One read per checkout
/// (`host`, `dir`) for all its runs, each bounded by
/// [`EVIDENCE_RETRY_WALL_CLOCK`]; a host that fails once is not tried again
/// in the same pass, and the pass stops after [`EVIDENCE_RETRY_PASS`].
pub async fn retry_unread_evidence(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>, now: i64) {
    let runs = match store.lock() {
        Ok(s) => s
            .runs_with_unread_evidence(now - super::gate::EVIDENCE_RETRY_SECS)
            .unwrap_or_default(),
        Err(_) => return,
    };
    let mut checkouts: Vec<((String, String), Vec<TaskRow>)> = Vec::new();
    for t in runs {
        let Some(ev) = t.evidence.as_ref() else {
            continue;
        };
        let (Some(host), Some(dir)) = (ev.host.clone(), ev.dir.clone()) else {
            continue;
        };
        if now - ev.at < EVIDENCE_RETRY_EVERY_SECS {
            continue;
        }
        let key = (host, dir);
        match checkouts.iter_mut().find(|(k, _)| *k == key) {
            Some((_, group)) => group.push(t),
            None => checkouts.push((key, vec![t])),
        }
    }
    let started = std::time::Instant::now();
    let mut unreachable: std::collections::HashSet<String> = std::collections::HashSet::new();
    for ((host, dir), group) in checkouts {
        if started.elapsed() >= EVIDENCE_RETRY_PASS {
            break;
        }
        if unreachable.contains(&host) {
            continue;
        }
        if !collect_evidence_as(store, ssh, &host, &dir, &group, EvidenceRead::Retry).await {
            unreachable.insert(host);
        }
    }
}

use crate::store::now_unix;

#[cfg(test)]
mod tests;
