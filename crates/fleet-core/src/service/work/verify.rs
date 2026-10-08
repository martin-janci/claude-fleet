//! Typed `done_when` and the Verified / Unverified answer (orchestration
//! O3, design 2026-10-07 §6; Mode C's C3).
//!
//! An item's acceptance conditions are lines, typed by their prefix:
//!
//! | line              | passes when                                            |
//! |-------------------|--------------------------------------------------------|
//! | `ci` / `ci:<name>`| a fresh PR reading of a session on the item, on the    |
//! |                   | checkout's own commit, shows every check passed (or,   |
//! |                   | for a name, not among a complete list of failures)     |
//! | `review`          | the latest `review` run finished with outcome `done`   |
//! | `test:<command>`  | the latest `test` run finished `done` and reported     |
//! |                   | running the command                                    |
//! | `person`, or any  | a person checked it                                    |
//! | other text        |                                                        |
//!
//! A person's recorded check of a line decides it whatever its type (CI on
//! a forge fleet cannot read is still checkable). Nothing an agent says
//! makes a line pass: a worker's own report never counts for its own item,
//! only a separate reviewer's or tester's run does, and recording a check
//! is a person's act. An item is `verified` when every line passed,
//! `failed` when any failed, and `unverified` otherwise.

use super::graph::{person_decides, require_mission_change, visible_item};
use super::WorkLinkArgs;
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::evidence::is_stale;
use crate::service::view_scope::ViewScope;
use crate::store::{Store, TaskRow, VerificationRow, WorkItemRow};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// The kinds a condition line takes.
pub const COND_KINDS: [&str; 4] = ["ci", "review", "test", "person"];

/// One condition line, typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cond {
    /// `ci` (every check) or `ci:<name>`.
    Ci(Option<String>),
    Review,
    /// `test` (any test run) or `test:<command>`.
    Test(Option<String>),
    /// A person checks it: `person`, or free text.
    Person,
}

impl Cond {
    pub fn kind(&self) -> &'static str {
        match self {
            Cond::Ci(_) => "ci",
            Cond::Review => "review",
            Cond::Test(_) => "test",
            Cond::Person => "person",
        }
    }
}

/// PURE: a line's type. Unknown prefixes are free text, which a person
/// checks.
pub fn parse_cond(line: &str) -> Cond {
    let t = line.trim();
    let (head, arg) = match t.split_once(':') {
        Some((h, a)) => (h.trim().to_ascii_lowercase(), Some(a.trim())),
        None => (t.to_ascii_lowercase(), None),
    };
    let arg = arg.filter(|a| !a.is_empty()).map(str::to_string);
    match head.as_str() {
        "ci" => Cond::Ci(arg),
        "review" if arg.is_none() => Cond::Review,
        "test" => Cond::Test(arg),
        _ => Cond::Person,
    }
}

/// One line's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CondCheck {
    pub line: String,
    /// One of [`COND_KINDS`].
    pub kind: String,
    /// `pass` | `fail` | `pending`.
    pub state: String,
    /// Why, in a sentence.
    pub detail: String,
    /// Who decided it, for a recorded check (`person:<id>`), or the run
    /// that did (`task:<id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<i64>,
}

/// An item's answer over all its lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verification {
    /// `verified` | `failed` | `unverified`.
    pub state: String,
    pub checks: Vec<CondCheck>,
}

fn check(line: &str, kind: &str, state: &str, detail: impl Into<String>) -> CondCheck {
    CondCheck {
        line: line.to_string(),
        kind: kind.to_string(),
        state: state.to_string(),
        detail: detail.into(),
        by: None,
        at: None,
    }
}

fn from_run(mut c: CondCheck, t: &TaskRow) -> CondCheck {
    c.by = Some(format!("task:{}", t.id));
    c.at = t.finished_at;
    c
}

fn clip(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{t}…")
    } else {
        t
    }
}

/// PURE over the store: one line of `item`, as of `now`. `recorded` is the
/// item's newest recorded check per line.
pub fn check_line(
    s: &Store,
    item: &WorkItemRow,
    line: &str,
    recorded: &[VerificationRow],
    now: i64,
) -> Result<CondCheck, IpcError> {
    let cond = parse_cond(line);
    let kind = cond.kind();
    if let Some(r) = recorded.iter().find(|r| r.line == line) {
        let mut c = check(
            line,
            kind,
            if r.ok { "pass" } else { "fail" },
            r.note.clone().unwrap_or_else(|| {
                if r.ok {
                    "checked by a person".into()
                } else {
                    "a person found it not met".into()
                }
            }),
        );
        c.by = Some(r.actor.clone());
        c.at = Some(r.at);
        return Ok(c);
    }
    Ok(match cond {
        Cond::Person => check(line, kind, "pending", "waits for a person to check it"),
        Cond::Ci(name) => ci_check(s, item.id, line, name.as_deref(), now)?,
        Cond::Review => {
            let Some(t) = s.latest_item_task(item.id, "review")? else {
                return Ok(check(line, kind, "pending", "no review run yet"));
            };
            let c = match (t.state.as_str(), t.report.as_ref()) {
                ("queued" | "running", _) => check(line, kind, "pending", "the review is running"),
                ("done", Some(r)) if r.outcome == "done" => check(
                    line,
                    kind,
                    "pass",
                    format!("the reviewer approved: {}", clip(&r.summary, 200)),
                ),
                ("done", Some(r)) => check(
                    line,
                    kind,
                    "fail",
                    format!(
                        "the reviewer answered {}: {}",
                        r.outcome,
                        clip(&r.summary, 200)
                    ),
                ),
                ("done", None) => check(line, kind, "pending", "the reviewer gave no verdict"),
                _ => check(line, kind, "fail", "the review run did not finish"),
            };
            from_run(c, &t)
        }
        Cond::Test(command) => {
            let Some(t) = s.latest_item_task(item.id, "test")? else {
                return Ok(check(line, kind, "pending", "no test run yet"));
            };
            let c = match (t.state.as_str(), t.report.as_ref()) {
                ("queued" | "running", _) => {
                    check(line, kind, "pending", "the test run is running")
                }
                ("done", Some(r)) if r.outcome != "done" => check(
                    line,
                    kind,
                    "fail",
                    format!(
                        "the tester answered {}: {}",
                        r.outcome,
                        clip(&r.summary, 200)
                    ),
                ),
                ("done", Some(r)) => match command.as_deref() {
                    Some(cmd) if !r.tests_run.iter().any(|x| x.contains(cmd)) => check(
                        line,
                        kind,
                        "pending",
                        format!("the test run did not report running {cmd}"),
                    ),
                    Some(cmd) => check(line, kind, "pass", format!("ran {cmd}")),
                    None => check(line, kind, "pass", "the test run passed"),
                },
                ("done", None) => check(line, kind, "pending", "the tester gave no verdict"),
                _ => check(line, kind, "fail", "the test run did not finish"),
            };
            from_run(c, &t)
        }
    })
}

fn ci_check(
    s: &Store,
    item_id: i64,
    line: &str,
    name: Option<&str>,
    now: i64,
) -> Result<CondCheck, IpcError> {
    let pending = |d: &str| check(line, "ci", "pending", d);
    let Some((ev, checked_at)) = s.item_pr_evidence(item_id)? else {
        return Ok(pending("no PR reading for this task"));
    };
    if is_stale(checked_at, now) {
        return Ok(pending("the last PR reading is old"));
    }
    if let (Some(h), Some(l)) = (&ev.head_oid, &ev.local_head) {
        if h != l {
            return Ok(pending("the PR is on another commit than the checkout"));
        }
    }
    let c = &ev.checks;
    if c.total == 0 {
        return Ok(pending("the PR has no checks"));
    }
    let complete = c.failing_total as usize <= c.failing.len();
    let mut out = match name {
        Some(n) => {
            if c.failing.iter().any(|f| f.name.eq_ignore_ascii_case(n)) {
                check(line, "ci", "fail", format!("{n} failed"))
            } else if c.failing_total == 0 && c.pending == 0 {
                check(line, "ci", "pass", format!("all {} checks passed", c.total))
            } else if c.pending > 0 {
                pending(&format!("{} checks are still running", c.pending))
            } else if complete {
                check(
                    line,
                    "ci",
                    "pass",
                    format!("{n} is not among the {} failing checks", c.failing_total),
                )
            } else {
                pending(&format!("{} checks failed, not all named", c.failing_total))
            }
        }
        None => {
            if c.failing_total > 0 {
                check(
                    line,
                    "ci",
                    "fail",
                    format!("{} checks failed", c.failing_total),
                )
            } else if c.pending > 0 {
                pending(&format!("{} checks are still running", c.pending))
            } else {
                check(line, "ci", "pass", format!("all {} checks passed", c.total))
            }
        }
    };
    out.at = checked_at;
    Ok(out)
}

/// The lines that decide `item`: its own, then its mission's when it is
/// that mission's root.
pub fn lines_for(s: &Store, item: &WorkItemRow) -> Result<Vec<String>, IpcError> {
    let mut lines = item.done_when.clone();
    if let Some(m) = s.item_mission(item.id)? {
        if let Some(m) = s.get_mission(m)? {
            if m.root_item_id == Some(item.id) {
                for l in m.done_when {
                    if !lines.contains(&l) {
                        lines.push(l);
                    }
                }
            }
        }
    }
    Ok(lines)
}

/// `item`'s verification as of `now`, `None` when it has no lines.
pub fn verification(
    s: &Store,
    item: &WorkItemRow,
    now: i64,
) -> Result<Option<Verification>, IpcError> {
    let lines = lines_for(s, item)?;
    if lines.is_empty() {
        return Ok(None);
    }
    let recorded = s.latest_verifications(item.id)?;
    let checks = lines
        .iter()
        .map(|l| check_line(s, item, l, &recorded, now))
        .collect::<Result<Vec<_>, _>>()?;
    let state = if checks.iter().any(|c| c.state == "fail") {
        "failed"
    } else if checks.iter().all(|c| c.state == "pass") {
        "verified"
    } else {
        "unverified"
    };
    Ok(Some(Verification {
        state: state.into(),
        checks,
    }))
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// What `done_when` and `verify` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyOutcome {
    pub item_id: i64,
    #[serde(default)]
    pub changed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<Verification>,
}

/// `work_link { action: done_when, item_id, done_when: [...] }`: replace an
/// item's condition lines (`[]` clears them).
pub fn set_done_when(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<VerifyOutcome, IpcError> {
    let item_id = args
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "done_when needs item_id"))?;
    let lines = args
        .done_when
        .as_deref()
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "done_when needs done_when (a list)"))?;
    let s = lock(store)?;
    visible_item(&s, scope, item_id)?;
    require_mission_change(&s, scope, item_id)?;
    let changed = s.set_item_done_when(item_id, lines, &super::graph::actor(scope))?;
    let item = visible_item(&s, scope, item_id)?;
    Ok(VerifyOutcome {
        item_id,
        changed,
        verification: verification(&s, &item, now_unix())?,
    })
}

/// `work_link { action: verify, item_id, line, ok, note? }`: a person
/// records a check of one of the item's lines. A person's act, as accepting
/// a proposal is: an agent never verifies, its own work or anybody's.
pub fn verify(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<VerifyOutcome, IpcError> {
    person_decides(scope)?;
    let item_id = args
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "verify needs item_id"))?;
    let line = args
        .line
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "verify needs line"))?;
    let ok = args
        .ok
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "verify needs ok (true or false)"))?;
    let s = lock(store)?;
    let item = visible_item(&s, scope, item_id)?;
    require_mission_change(&s, scope, item_id)?;
    if !lines_for(&s, &item)?.iter().any(|l| l == line) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("work item {item_id} has no done_when line {line:?}"),
        ));
    }
    s.record_verification(
        item_id,
        line,
        ok,
        &super::graph::actor(scope),
        args.note.as_deref(),
    )?;
    Ok(VerifyOutcome {
        item_id,
        changed: true,
        verification: verification(&s, &item, now_unix())?,
    })
}

#[cfg(test)]
mod tests;
