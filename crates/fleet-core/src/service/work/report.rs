//! A run's result (orchestration O3, design 2026-10-07 §6): the report the
//! worker is asked for, and the evidence fleet reads from git itself.
//!
//! - **The report.** A run's prompt asks the worker to follow its done
//!   marker with one fenced JSON block (`summary`, `outcome`, `tests_run`,
//!   `warnings`, `blockers`, `followups`, `confidence`). [`parse_report`] is
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

/// The instruction appended to a run's prompt: the marker, then the block.
/// The marker stands inline in the sentence, so the prompt's echo in the
/// pane never reads as the worker's emission.
pub fn run_instruction(nonce: &str) -> String {
    format!(
        "When finished, print exactly {} on its own line, followed by one fenced \
         ```json block with these keys: \"summary\" (one paragraph), \"outcome\" \
         (done | partial | blocked | failed), \"tests_run\" (the commands you ran), \
         \"warnings\", \"blockers\", \"followups\" (lists of short strings) and \
         \"confidence\" (low | medium | high). Fleet reads your commits and changed \
         files from git itself; do not list them.",
        crate::service::tasks::done_marker(nonce)
    )
}

/// A run's prompt as delivered: the text, a blank line, the instruction.
pub fn with_run_instruction(prompt: &str, nonce: &str) -> String {
    format!("{}\n\n{}", prompt.trim_end(), run_instruction(nonce))
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
    format!(
        "cd {cwd} 2>/dev/null || {{ printf '{err}no checkout\\n'; exit 0; }}\n\
         base=$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null)\n\
         [ -n \"$base\" ] || {{ git rev-parse -q --verify origin/main >/dev/null 2>&1 && base=origin/main; }}\n\
         [ -n \"$base\" ] || {{ git rev-parse -q --verify origin/master >/dev/null 2>&1 && base=origin/master; }}\n\
         [ -n \"$base\" ] || {{ printf '{err}no base branch\\n'; exit 0; }}\n\
         head=$(git rev-parse HEAD 2>/dev/null) || {{ printf '{err}not a git checkout\\n'; exit 0; }}\n\
         mb=$(git merge-base HEAD \"$base\" 2>/dev/null) || {{ printf '{err}no merge base with %s\\n' \"$base\"; exit 0; }}\n\
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
    let runs: Vec<i64> = tasks
        .iter()
        .filter(|t| t.work_item_id.is_some())
        .map(|t| t.id)
        .collect();
    if runs.is_empty() {
        return;
    }
    let script = evidence_script(cwd);
    let ev = match crate::service::catalog::inventory::run_host_script(ssh, host, &script).await {
        Ok(out) => parse_evidence(&out, now_unix()),
        Err(e) => TaskEvidence {
            at: now_unix(),
            error: Some(clip(&e.message, 200)),
            ..Default::default()
        },
    };
    let Ok(s) = store.lock() else { return };
    for id in runs {
        if let Err(e) = s.set_task_evidence(id, &ev) {
            tracing::warn!(task = id, error = %e.message, "[report] storing evidence failed");
        }
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
