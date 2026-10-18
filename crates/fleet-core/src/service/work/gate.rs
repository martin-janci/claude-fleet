//! The completion gate (design 2026-10-13 D3): what stands between a work
//! item and `done`. It reads the item's `done_when` checks (`verify.rs`) and
//! its open native children. It never moves an item; `status::set_status`
//! consults it for an agent, and the result panel shows it to a person.

use super::verify::{self, CondCheck, Verification};
use crate::ipc_error::IpcError;
use crate::service::orgs::OrgScope;
use crate::store::{Store, WorkItemRow};
use serde::{Deserialize, Serialize};

/// A subtask that still stands between its parent and `done`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenChild {
    pub id: i64,
    /// Empty when the reader may not see the subtask ([`for_scope`]).
    pub title: String,
}

/// An item's answer to "may this be set to done".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gate {
    /// `passed` | `unverified` | `blocked`.
    pub state: String,
    pub checks: Vec<CondCheck>,
    pub open_children: Vec<OpenChild>,
}

/// PURE. `blocked` when a check failed or a child is open, `passed` when
/// every check passed (an item with no lines has none to fail), `unverified`
/// otherwise. A child counts as open unless it is done or a rejected
/// proposal.
pub fn gate_from(v: Option<&Verification>, children: &[WorkItemRow]) -> Gate {
    let checks: Vec<CondCheck> = v.map(|v| v.checks.clone()).unwrap_or_default();
    let open_children: Vec<OpenChild> = children
        .iter()
        .filter(|c| c.status_category != "done")
        .filter(|c| c.proposal_state.as_deref() != Some("rejected"))
        .map(|c| OpenChild {
            id: c.id,
            title: c.title.clone(),
        })
        .collect();
    let state = if checks.iter().any(|c| c.state == "fail") || !open_children.is_empty() {
        "blocked"
    } else if checks.iter().all(|c| c.state == "pass") {
        "passed"
    } else {
        "unverified"
    };
    Gate {
        state: state.into(),
        checks,
        open_children,
    }
}

/// `item`'s gate as of `now`.
pub fn completion_gate(s: &Store, item: &WorkItemRow, now: i64) -> Result<Gate, IpcError> {
    let v = verify::verification(s, item, now)?;
    let children = s.native_children(item.id)?;
    Ok(gate_from(v.as_ref(), &children))
}

/// The gate as `scope` may read it. An open subtask of another org still
/// blocks, but its title is not the reader's to see: the fence the task
/// page puts on an item's children.
pub fn for_scope(mut g: Gate, s: &Store, scope: &OrgScope) -> Result<Gate, IpcError> {
    for c in &mut g.open_children {
        if !scope.sees_org(s.item_org(c.id)?) {
            c.title.clear();
        }
    }
    Ok(g)
}

/// PURE: the sentence an agent gets instead of `done`.
pub fn refusal(g: &Gate) -> Option<String> {
    if g.state == "passed" {
        return None;
    }
    let mut why: Vec<String> = g
        .checks
        .iter()
        .filter(|c| c.state != "pass")
        .map(|c| format!("`{}` is {}", c.line, c.state))
        .collect();
    why.extend(g.open_children.iter().map(|c| {
        if c.title.is_empty() {
            "a subtask is open".to_string()
        } else {
            format!("subtask \"{}\" is open", c.title)
        }
    }));
    Some(format!(
        "this task is not done yet: {}. Report it instead; a person marks it done.",
        why.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NativeItem;

    fn chk(line: &str, state: &str) -> CondCheck {
        CondCheck {
            line: line.into(),
            kind: "ci".into(),
            state: state.into(),
            detail: String::new(),
            by: None,
            at: None,
        }
    }

    fn ver(checks: Vec<CondCheck>) -> Verification {
        Verification {
            state: String::new(),
            checks,
        }
    }

    #[test]
    fn no_lines_and_no_children_passes() {
        let g = gate_from(None, &[]);
        assert_eq!(g.state, "passed");
        assert!(refusal(&g).is_none());
    }

    #[test]
    fn every_line_passed_passes() {
        let v = ver(vec![chk("ci", "pass"), chk("review", "pass")]);
        let g = gate_from(Some(&v), &[]);
        assert_eq!(g.state, "passed");
        assert_eq!(g.checks.len(), 2);
        assert!(refusal(&g).is_none());
    }

    #[test]
    fn a_failed_check_blocks_and_the_refusal_names_it() {
        let v = ver(vec![chk("ci", "pass"), chk("test:cargo test", "fail")]);
        let g = gate_from(Some(&v), &[]);
        assert_eq!(g.state, "blocked");
        let why = refusal(&g).unwrap();
        assert!(why.contains("`test:cargo test` is fail"), "{why}");
        assert!(
            !why.contains("`ci`"),
            "a passed line is not a reason: {why}"
        );
    }

    #[test]
    fn a_pending_check_is_unverified_and_still_refuses_an_agent() {
        let v = ver(vec![chk("review", "pending")]);
        let g = gate_from(Some(&v), &[]);
        assert_eq!(g.state, "unverified");
        assert!(refusal(&g).unwrap().contains("`review` is pending"));
    }

    #[test]
    fn an_open_child_blocks_a_done_or_rejected_one_does_not() {
        let s = Store::open_in_memory().unwrap();
        let parent = s
            .create_native_item(&NativeItem {
                title: "Parent",
                ..Default::default()
            })
            .unwrap();
        let child = |title: &str| {
            s.create_native_item(&NativeItem {
                title,
                parent_id: Some(parent.id),
                ..Default::default()
            })
            .unwrap()
        };
        let open = child("Child open");
        let done = child("Child done");
        s.set_item_status(done.id, "done").unwrap();
        let proposed = |title: &str| {
            s.propose_subtask(&crate::store::Proposal {
                parent_id: parent.id,
                title,
                notes: None,
                why: None,
                proposed_by: "s@mercury",
            })
            .unwrap()
        };
        let rejected = proposed("Child rejected");
        s.decide_proposal(rejected.id, false).unwrap();
        let waiting = proposed("Child proposed");

        let g = completion_gate(&s, &parent, 0).unwrap();
        assert_eq!(g.state, "blocked");
        assert_eq!(
            g.open_children,
            vec![
                OpenChild {
                    id: open.id,
                    title: "Child open".into()
                },
                OpenChild {
                    id: waiting.id,
                    title: "Child proposed".into()
                },
            ]
        );
        let why = refusal(&g).unwrap();
        assert!(why.contains("subtask \"Child open\" is open"), "{why}");

        // Closed, and the proposal turned down: nothing stands in the way.
        s.set_item_status(open.id, "done").unwrap();
        s.decide_proposal(waiting.id, false).unwrap();
        assert_eq!(completion_gate(&s, &parent, 0).unwrap().state, "passed");
    }

    #[test]
    fn a_subtask_of_another_org_blocks_without_its_title() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("mercury").unwrap();
        let mine = s.add_org("Mine", None, false).unwrap();
        let theirs = s.add_org("Theirs", None, false).unwrap();
        s.set_host_org("mercury", Some(mine.id)).unwrap();
        let parent = s
            .create_native_item(&NativeItem {
                title: "Parent",
                ..Default::default()
            })
            .unwrap();
        let child = |title: &str| {
            s.create_native_item(&NativeItem {
                title,
                parent_id: Some(parent.id),
                ..Default::default()
            })
            .unwrap()
        };
        let ours = child("Ours");
        let hidden = child("Their secret");
        s.set_local_item_org(hidden.id, Some(theirs.id)).unwrap();
        let host = OrgScope::for_host(&s, "mercury").unwrap();

        let g = for_scope(completion_gate(&s, &parent, 0).unwrap(), &s, &host).unwrap();
        assert_eq!(g.state, "blocked");
        assert_eq!(
            g.open_children,
            vec![
                OpenChild {
                    id: ours.id,
                    title: "Ours".into()
                },
                OpenChild {
                    id: hidden.id,
                    title: String::new()
                },
            ]
        );
        let why = refusal(&g).unwrap();
        assert!(why.contains("a subtask is open"), "{why}");
        assert!(!why.contains("Their secret"), "{why}");
    }
}
