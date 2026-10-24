//! The completion gate (design 2026-10-13 D3): what stands between a work
//! item and `done`. It reads the item's `done_when` checks (`verify.rs`),
//! its open native children and, for an item something implemented, the git
//! evidence of its latest implement run ([`RunEvidence`]). It never moves an
//! item; `status::set_status` consults it for an agent, the mission loop
//! for its close step, and the result panel shows it to a person.

use super::verify::{self, CondCheck, Verification};
use crate::ipc_error::IpcError;
use crate::service::orgs::OrgScope;
use crate::store::{Store, TaskRow, VerificationRow, WorkItemRow};
use serde::{Deserialize, Serialize};

/// The line a person's evidence override is recorded under
/// (`work_link { verify, line: "run evidence", ok, note }`).
pub const EVIDENCE_LINE: &str = "run evidence";

/// What git showed from an implement item's latest implement run: a run
/// that reports done but committed nothing and changed no file did not do
/// the work, whatever it says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEvidence {
    /// `present` | `missing` | `pending` (the run has not finished) |
    /// `overridden` (a person closed the gap, with a reason).
    pub state: String,
    pub task_id: i64,
    #[serde(default)]
    pub commits: u32,
    #[serde(default)]
    pub files: u32,
    /// Why, in a sentence: what is missing, or the person's reason.
    pub detail: String,
    /// The person who overrode it (`person:<id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<i64>,
}

/// How long after a run finished its evidence may still be on its way: fleet
/// reads git over a host round trip after it marks the run done
/// (`report::collect_evidence`), so a run with none yet is `pending` for this
/// long, and `missing` after.
pub const EVIDENCE_GRACE_SECS: i64 = 300;

/// How long after a run finished a failed git read (a host that dropped
/// for a moment) is read again by the mission loop
/// (`report::retry_unread_evidence`): `pending` meanwhile, and `missing`
/// only once this window passes with git still unreadable.
pub const EVIDENCE_RETRY_SECS: i64 = 1800;

/// PURE: the evidence of `run`, an item's latest implement run, at `now`,
/// given the item's newest recorded check of [`EVIDENCE_LINE`]. A person's
/// override counts for the run it was recorded over: a newer implement run
/// needs evidence (or an override) of its own.
pub fn run_evidence(run: &TaskRow, recorded: Option<&VerificationRow>, now: i64) -> RunEvidence {
    let ev = run.evidence.as_ref();
    let commits = ev.map_or(0, |e| e.commits_total);
    let files = ev.map_or(0, |e| e.files_total);
    let out = |state: &str, detail: String| RunEvidence {
        state: state.into(),
        task_id: run.id,
        commits,
        files,
        detail,
        by: None,
        at: None,
    };
    if matches!(run.state.as_str(), "queued" | "running") {
        return out("pending", "the implement run has not finished".into());
    }
    let since_done =
        |secs: i64| run.state == "done" && run.finished_at.is_some_and(|f| now - f < secs);
    // None yet, or only a read taken while the worker's turn still ran
    // (`task_report`) that shows nothing: its Stop reads git again.
    let unsettled =
        ev.is_none_or(|e| e.provisional && e.error.is_none()) && commits == 0 && files == 0;
    if unsettled && since_done(EVIDENCE_GRACE_SECS) {
        return out("pending", "git is still being read after the run".into());
    }
    if commits > 0 || files > 0 {
        return out(
            "present",
            format!("{commits} commit(s), {files} file(s) changed"),
        );
    }
    if let Some(r) = recorded.filter(|r| r.ok && r.at >= run.created_at) {
        let mut e = out(
            "overridden",
            r.note
                .clone()
                .unwrap_or_else(|| "a person accepted it without evidence".into()),
        );
        e.by = Some(r.actor.clone());
        e.at = Some(r.at);
        return e;
    }
    // A failed read the mission loop reads again (`report::retry_unread_evidence`).
    if let Some(err) = ev
        .filter(|e| e.host.is_some() && e.dir.is_some())
        .and_then(|e| e.error.as_deref())
    {
        if since_done(EVIDENCE_RETRY_SECS) {
            return out(
                "pending",
                format!("git could not be read after the run, and is read again: {err}"),
            );
        }
    }
    let detail = match ev {
        None => "the implement run left no git evidence".to_string(),
        Some(e) => match (e.error.as_deref(), e.uncommitted.filter(|n| *n > 0)) {
            (Some(err), _) => format!("git could not be read after the run: {err}"),
            (None, Some(n)) => {
                format!("the implement run committed nothing; {n} file(s) are left uncommitted")
            }
            (None, None) => "the implement run committed nothing and changed no file".into(),
        },
    };
    out("missing", detail)
}

/// `item`'s run evidence, `None` when no implement run worked on it.
pub fn item_run_evidence(
    s: &Store,
    item_id: i64,
    now: i64,
) -> Result<Option<RunEvidence>, IpcError> {
    let Some(run) = s.latest_item_task(item_id, "implement")? else {
        return Ok(None);
    };
    let recorded = s.latest_verifications(item_id)?;
    Ok(Some(run_evidence(
        &run,
        recorded.iter().find(|r| r.line == EVIDENCE_LINE),
        now,
    )))
}

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
    /// The latest implement run's git evidence; absent when no implement
    /// run worked on the item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<RunEvidence>,
}

/// PURE. `blocked` when a check failed, a child is open or the implement
/// run left no evidence; `passed` when every check passed (an item with no
/// lines has none to fail) and the evidence, where one is due, is there;
/// `unverified` otherwise. A child counts as open unless it is done or a
/// rejected proposal.
pub fn gate_from(
    v: Option<&Verification>,
    children: &[WorkItemRow],
    evidence: Option<RunEvidence>,
) -> Gate {
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
    let ev_state = evidence.as_ref().map(|e| e.state.as_str());
    let state = if checks.iter().any(|c| c.state == "fail")
        || !open_children.is_empty()
        || ev_state == Some("missing")
    {
        "blocked"
    } else if checks.iter().all(|c| c.state == "pass") && ev_state != Some("pending") {
        "passed"
    } else {
        "unverified"
    };
    Gate {
        state: state.into(),
        checks,
        open_children,
        evidence,
    }
}

/// `item`'s gate as of `now`.
pub fn completion_gate(s: &Store, item: &WorkItemRow, now: i64) -> Result<Gate, IpcError> {
    let v = verify::verification(s, item, now)?;
    let children = s.native_children(item.id)?;
    let evidence = item_run_evidence(s, item.id, now)?;
    Ok(gate_from(v.as_ref(), &children, evidence))
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
    if let Some(e) = g
        .evidence
        .as_ref()
        .filter(|e| e.state != "present" && e.state != "overridden")
    {
        why.push(format!("no run evidence ({})", e.detail));
    }
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
        let g = gate_from(None, &[], None);
        assert_eq!(g.state, "passed");
        assert!(refusal(&g).is_none());
    }

    #[test]
    fn every_line_passed_passes() {
        let v = ver(vec![chk("ci", "pass"), chk("review", "pass")]);
        let g = gate_from(Some(&v), &[], None);
        assert_eq!(g.state, "passed");
        assert_eq!(g.checks.len(), 2);
        assert!(refusal(&g).is_none());
    }

    #[test]
    fn a_failed_check_blocks_and_the_refusal_names_it() {
        let v = ver(vec![chk("ci", "pass"), chk("test:cargo test", "fail")]);
        let g = gate_from(Some(&v), &[], None);
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
        let g = gate_from(Some(&v), &[], None);
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

    fn run_row(
        id: i64,
        state: &str,
        created_at: i64,
        ev: Option<crate::store::TaskEvidence>,
    ) -> TaskRow {
        let mut t: TaskRow = serde_json::from_value(serde_json::json!({
            "id": id, "state": state, "created_at": created_at, "role": "implement"
        }))
        .unwrap();
        t.evidence = ev;
        t
    }

    fn ev(commits: u32, files: u32, uncommitted: Option<u32>) -> crate::store::TaskEvidence {
        crate::store::TaskEvidence {
            at: 1,
            commits_total: commits,
            files_total: files,
            uncommitted,
            ..Default::default()
        }
    }

    fn over(at: i64, ok: bool) -> VerificationRow {
        VerificationRow {
            id: 1,
            item_id: 1,
            line: EVIDENCE_LINE.into(),
            ok,
            actor: "person:3".into(),
            note: Some("docs-only change, nothing to commit".into()),
            at,
        }
    }

    #[test]
    fn commits_or_files_are_evidence() {
        let e = run_evidence(&run_row(9, "done", 10, Some(ev(2, 0, None))), None, 0);
        assert_eq!((e.state.as_str(), e.task_id, e.commits), ("present", 9, 2));
        let e = run_evidence(&run_row(9, "done", 10, Some(ev(0, 3, None))), None, 0);
        assert_eq!(e.state, "present");
    }

    #[test]
    fn a_run_with_nothing_to_show_is_missing_and_says_what() {
        let e = run_evidence(&run_row(9, "done", 10, None), None, 0);
        assert_eq!(e.state, "missing");
        assert!(e.detail.contains("no git evidence"), "{}", e.detail);
        let e = run_evidence(&run_row(9, "done", 10, Some(ev(0, 0, Some(4)))), None, 0);
        assert_eq!(e.state, "missing");
        assert!(
            e.detail.contains("4 file(s) are left uncommitted"),
            "{}",
            e.detail
        );
        let mut broken = ev(0, 0, None);
        broken.error = Some("not a git repository".into());
        let e = run_evidence(&run_row(9, "done", 10, Some(broken)), None, 0);
        assert!(e.detail.contains("not a git repository"), "{}", e.detail);
    }

    #[test]
    fn an_unfinished_run_is_pending() {
        let e = run_evidence(&run_row(9, "running", 10, None), None, 0);
        assert_eq!(e.state, "pending");
    }

    #[test]
    fn a_run_whose_git_read_is_on_its_way_is_pending_for_the_grace_only() {
        let mut run = run_row(9, "done", 10, None);
        run.finished_at = Some(100);
        let e = run_evidence(&run, None, 100 + EVIDENCE_GRACE_SECS - 1);
        assert_eq!(e.state, "pending");
        assert_eq!(gate_from(None, &[], Some(e)).state, "unverified");
        assert_eq!(
            run_evidence(&run, None, 100 + EVIDENCE_GRACE_SECS).state,
            "missing"
        );
        // Evidence that came back empty is missing at once.
        run.evidence = Some(ev(0, 0, None));
        assert_eq!(run_evidence(&run, None, 101).state, "missing");
    }

    /// review8: a `task_report` read taken mid-turn that shows nothing waits
    /// for the worker's Stop to read git again; one that shows work counts.
    #[test]
    fn a_provisional_read_that_shows_nothing_waits_like_none() {
        let mut run = run_row(9, "done", 10, None);
        run.finished_at = Some(100);
        let mut mid = ev(0, 0, Some(2));
        mid.provisional = true;
        run.evidence = Some(mid.clone());
        assert_eq!(run_evidence(&run, None, 101).state, "pending");
        assert_eq!(
            run_evidence(&run, None, 100 + EVIDENCE_GRACE_SECS).state,
            "missing"
        );
        mid.commits_total = 1;
        run.evidence = Some(mid);
        assert_eq!(run_evidence(&run, None, 101).state, "present");
    }

    /// review8: a git read that failed (a host that dropped) is read again
    /// by the loop, so it is pending for the retry window, then missing.
    #[test]
    fn a_failed_read_is_pending_while_the_loop_reads_it_again() {
        let mut run = run_row(9, "done", 10, None);
        run.finished_at = Some(100);
        let mut failed = ev(0, 0, None);
        failed.error = Some("ssh: connection reset".into());
        failed.host = Some("mars".into());
        failed.dir = Some("/w/x".into());
        run.evidence = Some(failed.clone());
        let e = run_evidence(&run, None, 101);
        assert_eq!(e.state, "pending");
        assert!(e.detail.contains("connection reset"), "{}", e.detail);
        assert_eq!(
            run_evidence(&run, Some(&over(20, true)), 101).state,
            "overridden",
            "a person's override still answers it at once"
        );
        let e = run_evidence(&run, None, 100 + EVIDENCE_RETRY_SECS);
        assert_eq!(e.state, "missing");
        assert!(e.detail.contains("could not be read"), "{}", e.detail);
        // Nowhere to read it again: missing at once, as before.
        failed.dir = None;
        run.evidence = Some(failed);
        assert_eq!(run_evidence(&run, None, 101).state, "missing");
    }

    #[test]
    fn a_persons_override_holds_for_the_run_it_was_recorded_over_only() {
        let run = run_row(9, "done", 10, Some(ev(0, 0, None)));
        let e = run_evidence(&run, Some(&over(20, true)), 0);
        assert_eq!(e.state, "overridden");
        assert_eq!(e.by.as_deref(), Some("person:3"));
        assert!(e.detail.contains("docs-only"), "{}", e.detail);
        // Recorded before this run started: an older attempt's.
        assert_eq!(run_evidence(&run, Some(&over(5, true)), 0).state, "missing");
        // Withdrawn.
        assert_eq!(
            run_evidence(&run, Some(&over(20, false)), 0).state,
            "missing"
        );
    }

    #[test]
    fn missing_evidence_blocks_the_gate_and_the_refusal_names_it() {
        let e = run_evidence(&run_row(9, "done", 10, None), None, 0);
        let g = gate_from(None, &[], Some(e));
        assert_eq!(g.state, "blocked");
        let why = refusal(&g).unwrap();
        assert!(why.contains("no run evidence"), "{why}");

        let pending = run_evidence(&run_row(9, "running", 10, None), None, 0);
        assert_eq!(gate_from(None, &[], Some(pending)).state, "unverified");

        for e in [
            run_evidence(&run_row(9, "done", 10, Some(ev(1, 1, None))), None, 0),
            run_evidence(&run_row(9, "done", 10, None), Some(&over(20, true)), 0),
        ] {
            let g = gate_from(None, &[], Some(e));
            assert_eq!(g.state, "passed");
            assert!(refusal(&g).is_none());
        }
    }

    #[test]
    fn an_implement_run_without_evidence_refuses_an_agent_until_a_person_overrides() {
        use crate::service::work::verify::{self, VerifyOutcome};
        use crate::service::work::WorkLinkArgs;
        let st = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        let item = {
            let s = st.lock().unwrap();
            let item = s.create_local_work_item(None, "queue").unwrap();
            let t = s.insert_task(None, None, "go", "n-impl").unwrap();
            s.set_task_run(t.id, item.id, 1, "implement").unwrap();
            let t = s.get_task(t.id).unwrap().unwrap();
            let report = crate::store::TaskReport {
                summary: "done".into(),
                outcome: "done".into(),
                ..Default::default()
            };
            crate::service::tasks::complete_task_reported(&s, &t, "", Some(&report)).unwrap();
            item
        };
        // Past the grace for git's answer: none came.
        let later = crate::store::now_unix() + EVIDENCE_GRACE_SECS;
        let g = completion_gate(&st.lock().unwrap(), &item, later).unwrap();
        assert_eq!(g.state, "blocked");
        assert_eq!(g.evidence.as_ref().unwrap().state, "missing");

        let me = crate::service::view_scope::ViewScope::internal();
        let args = |ok: bool, note: Option<&str>| WorkLinkArgs {
            action: "verify".into(),
            item_id: Some(item.id),
            line: Some(EVIDENCE_LINE.into()),
            ok: Some(ok),
            note: note.map(str::to_string),
            ..Default::default()
        };
        // An override says why.
        let e = verify::verify(&args(true, None), &st, &me).unwrap_err();
        assert!(e.message.contains("needs a note"), "{}", e.message);
        let out: VerifyOutcome = verify::verify(
            &args(true, Some("the fix was a config change on the host")),
            &st,
            &me,
        )
        .unwrap();
        assert!(out.changed);
        let g = completion_gate(&st.lock().unwrap(), &item, later).unwrap();
        assert_eq!(g.state, "passed");
        let e = g.evidence.unwrap();
        assert_eq!(e.state, "overridden");
        assert!(e.detail.contains("config change"), "{}", e.detail);
    }

    #[test]
    fn an_item_no_implement_run_worked_on_needs_no_evidence() {
        let s = Store::open_in_memory().unwrap();
        let item = s.create_local_work_item(None, "plain").unwrap();
        let g = completion_gate(&s, &item, 0).unwrap();
        assert!(g.evidence.is_none());
        assert_eq!(g.state, "passed");
        let me = crate::service::view_scope::ViewScope::internal();
        let st = std::sync::Mutex::new(s);
        let e = crate::service::work::verify::verify(
            &crate::service::work::WorkLinkArgs {
                action: "verify".into(),
                item_id: Some(item.id),
                line: Some(EVIDENCE_LINE.into()),
                ok: Some(true),
                note: Some("why".into()),
                ..Default::default()
            },
            &st,
            &me,
        )
        .unwrap_err();
        assert!(e.message.contains("no implement run"), "{}", e.message);
    }
}
