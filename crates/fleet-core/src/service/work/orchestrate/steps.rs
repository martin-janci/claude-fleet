//! The deterministic loop (orchestration O4, design 2026-10-07 §5.3): the
//! mechanical next steps of a mission, decided without a model.
//!
//! [`plan_steps`] is pure over a mission's graph and attempts. It answers
//! what the loop would do next: run what is ready up to the mission's
//! parallelism, retry a failure once more with its error, send finished
//! work to review and test when its `done_when` asks for them, close what
//! is verified, complete a finite mission whose members are all done. What
//! needs judgment (the same error twice, retries spent, the task budget
//! used) becomes an `ask`, which only a person answers.
//!
//! The steps are shown on the mission (its Ready cards), applied by a
//! person (*Start wave*, a card's button), and applied by the loop itself
//! only at the level a person's grant allows (O6).

use crate::service::work::graph::{GraphNode, MissionGraph};
use crate::service::work::verify::{parse_cond, Cond};
use crate::store::{MissionRow, MissionTaskCounts, TaskRow, WorkItemRow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The kinds a step takes.
pub const STEP_KINDS: [&str; 8] = [
    "run",
    "retry",
    "review",
    "test",
    "close",
    "integrate",
    "complete",
    "ask",
];

/// The level at which the loop takes a mechanical step without a person.
pub const AUTO_LEVEL: i64 = 2;

/// One next step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// One of [`STEP_KINDS`].
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<i64>,
    /// The run's role, for run / retry / review / test / integrate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Why, in a sentence.
    pub reason: String,
    /// What the run's prompt adds: the last error, the reviewer's words,
    /// the command to test. Worker text, so it is quoted, never obeyed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// `true`: the loop may take it itself at [`AUTO_LEVEL`]; `false`
    /// (an ask): only a person.
    pub auto: bool,
}

impl Step {
    fn new(kind: &str, item: Option<i64>, role: Option<&str>, reason: String) -> Step {
        Step {
            kind: kind.into(),
            item_id: item,
            role: role.map(str::to_string),
            reason,
            context: None,
            auto: kind != "ask",
        }
    }

    fn with_context(mut self, c: impl Into<String>) -> Step {
        let c: String = c.into();
        self.context = (!c.trim().is_empty()).then(|| c.chars().take(2000).collect());
        self
    }

    /// The key a step is named by in a request to take it.
    pub fn key(&self) -> String {
        format!("{}:{}", self.kind, self.item_id.unwrap_or(0))
    }
}

/// What [`plan_steps`] reads.
pub struct StepInput<'a> {
    pub mission: &'a MissionRow,
    pub graph: &'a MissionGraph,
    pub items: &'a [WorkItemRow],
    /// Every member's attempts, newest first.
    pub attempts: &'a HashMap<i64, Vec<TaskRow>>,
    pub counts: MissionTaskCounts,
    /// Runs open at once: the policy's, or less when a grant says so.
    pub max_parallel: u32,
}

fn failure_text(t: &TaskRow) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(e) = t.error.as_deref().filter(|e| !e.trim().is_empty()) {
        parts.push(e.trim().to_string());
    }
    if let Some(r) = &t.report {
        if !r.blockers.is_empty() {
            parts.push(format!("blockers: {}", r.blockers.join("; ")));
        }
        if !r.summary.is_empty() && parts.is_empty() {
            parts.push(r.summary.clone());
        }
    }
    parts.join(" — ")
}

/// Whether an attempt ended without the work: failed, cancelled, or done
/// with a reported outcome other than `done`.
pub fn attempt_failed(t: &TaskRow) -> bool {
    match t.state.as_str() {
        "failed" | "cancelled" => true,
        "done" => t.report.as_ref().is_some_and(|r| r.outcome != "done"),
        _ => false,
    }
}

fn is_impl(t: &TaskRow) -> bool {
    matches!(t.role.as_deref(), None | Some("implement"))
}

/// PURE: the mission's next steps, in graph order.
pub fn plan_steps(input: &StepInput<'_>) -> Vec<Step> {
    let m = input.mission;
    let p = &m.policy;
    let mut steps: Vec<Step> = Vec::new();
    if m.state != "active" || !crate::store::mode_runs_loop(&m.mode) {
        return steps;
    }
    let root = m.root_item_id;
    let members: HashMap<i64, &WorkItemRow> = input.items.iter().map(|i| (i.id, i)).collect();
    // The root is the mission's container: it runs only when it is all
    // there is.
    let sole_root = input.items.len() == 1 && root.is_some();
    let runnable = |n: &GraphNode| Some(n.item_id) != root || sole_root;
    let no_attempts: Vec<TaskRow> = Vec::new();
    let budget_left = (p.max_tasks as i64 - input.counts.total).max(0);
    let mut slots = (input.max_parallel as i64 - input.counts.open).max(0);
    let mut spent = 0i64;
    let take = |slots: &mut i64, spent: &mut i64| -> bool {
        if *slots > 0 && *spent < budget_left {
            *slots -= 1;
            *spent += 1;
            true
        } else {
            false
        }
    };
    let mut budget_asked = false;
    for n in &input.graph.nodes {
        if !runnable(n) {
            continue;
        }
        let Some(item) = members.get(&n.item_id) else {
            continue;
        };
        let tries = input.attempts.get(&n.item_id).unwrap_or(&no_attempts);
        let impls: Vec<&TaskRow> = tries.iter().filter(|t| is_impl(t)).collect();
        let open_role = |role: &str| {
            tries.iter().any(|t| {
                t.role.as_deref() == Some(role) && matches!(t.state.as_str(), "queued" | "running")
            })
        };
        let title = &item.title;
        match n.state.as_str() {
            "ready" => {
                if take(&mut slots, &mut spent) {
                    steps.push(Step::new(
                        "run",
                        Some(n.item_id),
                        Some("implement"),
                        format!("{title} is ready"),
                    ));
                } else if budget_left - spent <= 0 && !budget_asked {
                    budget_asked = true;
                    steps.push(Step::new(
                        "ask",
                        None,
                        None,
                        format!("the mission used its {} attempts", p.max_tasks),
                    ));
                }
            }
            "failed" => {
                let failures: Vec<&&TaskRow> = impls.iter().filter(|t| attempt_failed(t)).collect();
                let same_twice = failures.len() >= 2
                    && failure_text(failures[0]) == failure_text(failures[1])
                    && !failure_text(failures[0]).is_empty();
                let last = failures
                    .first()
                    .map(|t| failure_text(t))
                    .unwrap_or_default();
                if same_twice {
                    steps.push(
                        Step::new(
                            "ask",
                            Some(n.item_id),
                            None,
                            format!("{title} failed twice the same way"),
                        )
                        .with_context(last),
                    );
                } else if impls.len() as u32 > p.max_retries {
                    steps.push(
                        Step::new(
                            "ask",
                            Some(n.item_id),
                            None,
                            format!(
                                "{title} failed {} times; its retries are spent",
                                impls.len()
                            ),
                        )
                        .with_context(last),
                    );
                } else if take(&mut slots, &mut spent) {
                    steps.push(
                        Step::new(
                            "retry",
                            Some(n.item_id),
                            Some("implement"),
                            format!(
                                "{title} failed; attempt {} of {}",
                                impls.len() + 1,
                                p.max_retries + 1
                            ),
                        )
                        .with_context(last),
                    );
                }
            }
            "verifying" => {
                steps.extend(verify_steps(
                    n,
                    title,
                    p.require_review,
                    &impls,
                    &open_role,
                    p.max_retries,
                    &mut || take(&mut slots, &mut spent),
                ));
            }
            _ => {}
        }
    }
    // A finite mission whose members are all done completes, once its own
    // done_when (on the root) holds.
    if m.mode == "finite" && !input.items.is_empty() {
        let others_done = input
            .items
            .iter()
            .filter(|i| Some(i.id) != root || sole_root)
            .all(|i| i.status_category == "done");
        let root_ok = root
            .and_then(|r| input.graph.nodes.iter().find(|n| n.item_id == r))
            .map(|n| {
                n.verification
                    .as_ref()
                    .is_none_or(|v| v.state == "verified")
            })
            .unwrap_or(true);
        if others_done && root_ok {
            steps.push(Step::new(
                "complete",
                root,
                None,
                "every task is done and the mission's done_when holds".into(),
            ));
        }
    }
    steps
}

/// The steps of an item whose implementation finished: review and test
/// runs its lines ask for, a retry when a reviewer or tester found it not
/// done, and closing it once it is verified.
fn verify_steps(
    n: &GraphNode,
    title: &str,
    require_review: bool,
    impls: &[&TaskRow],
    open_role: &dyn Fn(&str) -> bool,
    max_retries: u32,
    take: &mut dyn FnMut() -> bool,
) -> Vec<Step> {
    let mut out = Vec::new();
    let Some(v) = &n.verification else {
        // No lines: done means done, after a review when the policy asks.
        if require_review && !open_role("review") {
            if take() {
                out.push(Step::new(
                    "review",
                    Some(n.item_id),
                    Some("review"),
                    format!("{title} is implemented; the policy asks for a review"),
                ));
            }
        } else if !require_review {
            out.push(Step::new(
                "close",
                Some(n.item_id),
                None,
                format!("{title} is implemented and has no done_when"),
            ));
        }
        return out;
    };
    if v.state == "verified" {
        out.push(Step::new(
            "close",
            Some(n.item_id),
            None,
            format!("{title} is verified"),
        ));
        return out;
    }
    if v.state == "failed" {
        let why: Vec<String> = v
            .checks
            .iter()
            .filter(|c| c.state == "fail")
            .map(|c| format!("{}: {}", c.line, c.detail))
            .collect();
        let by_run = v
            .checks
            .iter()
            .any(|c| c.state == "fail" && matches!(c.kind.as_str(), "review" | "test" | "ci"));
        if by_run && (impls.len() as u32) <= max_retries {
            if take() {
                out.push(
                    Step::new(
                        "retry",
                        Some(n.item_id),
                        Some("implement"),
                        format!("{title} is not done yet"),
                    )
                    .with_context(why.join("\n")),
                );
            }
        } else {
            out.push(
                Step::new(
                    "ask",
                    Some(n.item_id),
                    None,
                    format!("{title} did not pass its checks"),
                )
                .with_context(why.join("\n")),
            );
        }
        return out;
    }
    for c in v.checks.iter().filter(|c| c.state == "pending") {
        match parse_cond(&c.line) {
            Cond::Review if !open_role("review") && c.by.is_none() && take() => {
                out.push(Step::new(
                    "review",
                    Some(n.item_id),
                    Some("review"),
                    format!("{title} waits for its review"),
                ));
            }
            Cond::Test(cmd) if !open_role("test") && c.by.is_none() && take() => {
                let what = cmd.clone().unwrap_or_else(|| "its tests".into());
                out.push(
                    Step::new(
                        "test",
                        Some(n.item_id),
                        Some("test"),
                        format!("{title} waits for {what}"),
                    )
                    .with_context(match cmd {
                        Some(c) => format!("Run exactly: {c}"),
                        None => "Run the project's tests.".into(),
                    }),
                );
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests;
