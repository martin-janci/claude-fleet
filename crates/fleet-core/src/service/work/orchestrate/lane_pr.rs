//! A lane's draft pull request (mission improvements M4, design 2026-10-26
//! §7). Under `policy.lane_pr = draft`, once a task of a lane is done fleet
//! pushes the lane's branch up to the newest done task's commit (never the
//! lead's work in progress) and opens the branch's draft pull request, or
//! finds the one the lead opened; each later done task pushes again and
//! refreshes the PR's task list. Marking it ready and merging stay a
//! person's: fleet runs only `git push` of the lane's own branch and `gh pr
//! create --draft` / `gh pr edit --body`.
//!
//! What it did is a `note` in the mission's log; a host with no `gh`, or one
//! not signed in, says so once per commit and the lead's brief still asks
//! it to open the PR itself.

use super::{event, Deps};
use crate::ipc_error::lock;
use crate::shell::quote as shq;
use crate::store::{MissionLaneRow, MissionRow, Store};
use std::collections::HashMap;
use std::sync::Mutex;

/// One task of a lane, as the PR's body lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneTask {
    pub label: String,
    pub title: String,
    pub done: bool,
}

/// PURE: the PR's title.
pub fn pr_title(m: &MissionRow, lane: &str) -> String {
    let t = format!("{}: {lane}", m.name);
    t.chars().take(120).collect()
}

/// PURE: the PR's body: the mission's goal, the lane's tasks with the done
/// ones ticked, and who decides.
pub fn pr_body(m: &MissionRow, lane: &str, tasks: &[LaneTask]) -> String {
    let clip = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    let mut out = format!(
        "Lane \"{}\" of mission \"{}\".\n\n{}\n\n",
        clip(lane, 40),
        clip(&m.name, 120),
        clip(&m.goal, 1200)
    );
    for t in tasks.iter().take(30) {
        out.push_str(&format!(
            "- [{}] {} {}\n",
            if t.done { "x" } else { " " },
            clip(&t.label, 30),
            clip(&t.title, 160)
        ));
    }
    out.push_str(
        "\nOpened by fleet as a draft; it grows as the lane's tasks are done. \
         Marking it ready and merging it are a person's.",
    );
    out
}

/// What the script did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pushed {
    /// Pushed `head`; the branch's PR, when one is known now.
    Done { url: Option<String> },
    /// Pushed, but `gh` is missing or not signed in: no PR.
    NoGh,
    /// The push was refused or failed.
    PushFailed(String),
}

/// The script, run in the lane's checkout: push `sha` to `branch` on
/// `origin`, then find the branch's PR or open it as a draft, or refresh its
/// body. Says `__FLEET_PR pushed|no_gh|push_failed <why>` and
/// `__FLEET_PR_URL <url>`.
pub fn pr_script(dir: &str, branch: &str, sha: &str, title: &str, body: &str) -> String {
    format!(
        "cd -- {dir} || {{ echo '__FLEET_PR push_failed no checkout'; exit 0; }}\n\
         b={b}\n\
         if ! out=$(git push -q origin {sha}:\"refs/heads/$b\" 2>&1); then\n\
         echo \"__FLEET_PR push_failed $(printf '%s' \"$out\" | tail -1 | cut -c1-200)\"; exit 0; fi\n\
         if ! command -v gh >/dev/null 2>&1 || ! gh auth status >/dev/null 2>&1; then echo '__FLEET_PR no_gh'; exit 0; fi\n\
         url=$(gh pr view \"$b\" --json url -q .url 2>/dev/null)\n\
         if [ -n \"$url\" ]; then gh pr edit \"$b\" --body {body} >/dev/null 2>&1 || true\n\
         else url=$(gh pr create --draft --head \"$b\" --title {title} --body {body} 2>/dev/null | tail -1); fi\n\
         echo '__FLEET_PR pushed'\n\
         [ -n \"$url\" ] && echo \"__FLEET_PR_URL $url\"\n",
        dir = shq(dir),
        b = shq(branch),
        sha = shq(sha),
        title = shq(title),
        body = shq(body),
    )
}

/// PURE: the script's answer; `None` when it said nothing it knows.
pub fn parse_pr(stdout: &str) -> Option<Pushed> {
    let mut state: Option<Pushed> = None;
    let mut url: Option<String> = None;
    for line in stdout.lines() {
        if let Some(u) = line.strip_prefix("__FLEET_PR_URL ") {
            let u = u.trim();
            if u.starts_with("https://") && !u.chars().any(char::is_whitespace) {
                url = Some(u.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("__FLEET_PR ") {
            state = Some(
                match rest.split_once(' ').map_or((rest, ""), |(a, b)| (a, b)) {
                    ("pushed", _) => Pushed::Done { url: None },
                    ("no_gh", _) => Pushed::NoGh,
                    (_, why) => Pushed::PushFailed(why.trim().to_string()),
                },
            );
        }
    }
    match state? {
        Pushed::Done { .. } => Some(Pushed::Done { url }),
        other => Some(other),
    }
}

/// The commit to push for `lane`: the newest done task's, as its implement
/// run's evidence read it; `None` before a task of the lane is done.
fn verified_head(s: &Store, lane: &MissionLaneRow) -> Option<String> {
    let mut best: Option<(i64, String)> = None;
    for item in &lane.item_ids {
        let done = s
            .get_work_item(*item)
            .ok()
            .flatten()
            .is_some_and(|i| i.status_category == "done");
        if !done {
            continue;
        }
        let Some(t) = s.latest_item_task(*item, "implement").ok().flatten() else {
            continue;
        };
        let Some(head) = t.evidence.as_ref().and_then(|e| e.head.clone()) else {
            continue;
        };
        if crate::store::is_commit_id(&head) && best.as_ref().is_none_or(|(id, _)| t.id > *id) {
            best = Some((t.id, head));
        }
    }
    best.map(|(_, h)| h)
}

/// The heads a push was already tried for, per lane, so a failing push is
/// tried (and said) once per commit, not every tick.
fn tried() -> &'static Mutex<HashMap<i64, String>> {
    static T: std::sync::OnceLock<Mutex<HashMap<i64, String>>> = std::sync::OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Push each lane of `m` whose newest done task is not on its branch yet,
/// and open or refresh its draft PR. Never fails the tick.
pub async fn sync(deps: &Deps, m: &MissionRow) {
    if m.policy.execution != "lane" || m.policy.lane_pr != "draft" {
        return;
    }
    struct Job {
        lane: i64,
        name: String,
        host: String,
        script: String,
        head: String,
    }
    let jobs: Vec<Job> = {
        let Ok(s) = lock(&deps.store) else {
            return;
        };
        let Ok(lanes) = s.mission_lanes(m.id) else {
            return;
        };
        let mut jobs = Vec::new();
        for lane in lanes {
            let Some(branch) = lane.branch.clone().filter(|b| {
                crate::service::agents::workspace::branch_ok(b) && b != "main" && b != "master"
            }) else {
                continue;
            };
            let Some(head) = verified_head(&s, &lane) else {
                continue;
            };
            if lane.pr_head.as_deref() == Some(head.as_str())
                || tried().lock().is_ok_and(|t| t.get(&lane.id) == Some(&head))
            {
                continue;
            }
            let Some(wt) = lane
                .worktree_id
                .and_then(|w| s.get_worktree_row(w).ok().flatten())
            else {
                continue;
            };
            let tasks: Vec<LaneTask> = lane
                .item_ids
                .iter()
                .filter_map(|i| s.get_work_item(*i).ok().flatten())
                .map(|i| LaneTask {
                    label: i.key.clone().unwrap_or_else(|| format!("#{}", i.id)),
                    title: i.title.clone(),
                    done: i.status_category == "done",
                })
                .collect();
            let script = pr_script(
                &wt.path,
                &branch,
                &head,
                &pr_title(m, &lane.name),
                &pr_body(m, &lane.name, &tasks),
            );
            jobs.push(Job {
                lane: lane.id,
                name: lane.name.clone(),
                host: wt.host_alias.clone(),
                script,
                head,
            });
        }
        jobs
    };
    for j in jobs {
        if let Ok(mut t) = tried().lock() {
            t.insert(j.lane, j.head.clone());
        }
        let out =
            crate::service::catalog::inventory::run_host_script(&deps.ssh, &j.host, &j.script)
                .await;
        let Ok(s) = lock(&deps.store) else {
            return;
        };
        let note = |text: String| {
            event(
                &s,
                m.id,
                "note",
                "fleet",
                None,
                serde_json::json!({ "text": text }),
            )
        };
        match out.ok().as_deref().and_then(parse_pr) {
            Some(Pushed::Done { url }) => {
                let first = s
                    .get_mission_lane(j.lane)
                    .ok()
                    .flatten()
                    .is_some_and(|l| l.pr_url.is_none());
                if let Err(e) = s.set_lane_pr(j.lane, url.as_deref(), &j.head) {
                    tracing::debug!(lane = j.lane, error = %e.message, "[lane_pr] not recorded");
                }
                if let (true, Some(u)) = (first, url) {
                    note(format!("Lane {}'s draft pull request: {u}", j.name));
                }
            }
            Some(Pushed::NoGh) => {
                let _ = s.set_lane_pr(j.lane, None, &j.head);
                note(format!(
                    "Pushed lane {}'s branch; no draft pull request: gh is missing or not signed in on {}",
                    j.name, j.host
                ));
            }
            Some(Pushed::PushFailed(why)) => note(format!(
                "Lane {}'s branch was not pushed from {}: {why}",
                j.name, j.host
            )),
            None => tracing::debug!(lane = j.lane, host = %j.host, "[lane_pr] no answer"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mission() -> MissionRow {
        serde_json::from_value(serde_json::json!({
            "id": 1, "name": "Pay", "goal": "take payments", "mode": "finite",
            "state": "active", "level": 2, "plan_version": 1,
            "created_at": 0, "updated_at": 0, "version": 1
        }))
        .unwrap()
    }

    #[test]
    fn the_body_lists_the_lanes_tasks_and_who_decides() {
        let body = pr_body(
            &mission(),
            "api",
            &[
                LaneTask {
                    label: "T-1".into(),
                    title: "schema".into(),
                    done: true,
                },
                LaneTask {
                    label: "T-2".into(),
                    title: "routes".into(),
                    done: false,
                },
            ],
        );
        assert!(
            body.contains("- [x] T-1 schema\n- [ ] T-2 routes"),
            "{body}"
        );
        assert!(body.contains("are a person's"));
        assert_eq!(pr_title(&mission(), "api"), "Pay: api");
    }

    #[test]
    fn the_script_pushes_the_verified_commit_and_quotes_everything() {
        let s = pr_script("/w/a b", "m1-pay-api", "abc123", "it's", "x'y");
        assert!(
            s.contains("git push -q origin 'abc123':\"refs/heads/$b\""),
            "{s}"
        );
        assert!(s.contains("--draft"));
        assert!(!s.contains("gh pr ready") && !s.contains("gh pr merge"));
        assert!(s.contains(&shq("it's")) && s.contains(&shq("x'y")));
    }

    #[test]
    fn the_answer_says_pushed_with_its_pr_or_why_not() {
        assert_eq!(
            parse_pr("__FLEET_PR pushed\n__FLEET_PR_URL https://github.com/o/r/pull/7\n"),
            Some(Pushed::Done {
                url: Some("https://github.com/o/r/pull/7".into())
            })
        );
        assert_eq!(parse_pr("__FLEET_PR no_gh\n"), Some(Pushed::NoGh));
        assert_eq!(
            parse_pr("__FLEET_PR push_failed rejected\n"),
            Some(Pushed::PushFailed("rejected".into()))
        );
        assert_eq!(
            parse_pr("__FLEET_PR pushed\n__FLEET_PR_URL javascript:x\n"),
            Some(Pushed::Done { url: None })
        );
        assert_eq!(parse_pr("noise"), None);
    }

    /// The script against real git: it pushes the verified commit, not
    /// the lead's later work, to the lane's branch on `origin`.
    #[test]
    fn real_git_pushes_the_verified_commit_only() {
        let d = tempfile::tempdir().unwrap();
        let sh = |cmd: &str| {
            let out = crate::proc::std_command("bash")
                .arg("-c")
                .arg(format!("cd {} && {cmd}", shq(d.path().to_str().unwrap())))
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{cmd}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let g = "git -c user.name=t -c user.email=t@t";
        sh("git init -q --bare origin.git && git init -q -b main co");
        sh(&format!(
            "cd co && git remote add origin ../origin.git && echo a > a && git add a && {g} commit -qm a \
             && git checkout -qb m1-pay-api && echo b > b && git add b && {g} commit -qm b"
        ));
        let verified = sh("cd co && git rev-parse HEAD");
        sh(&format!(
            "cd co && echo c > c && git add c && {g} commit -qm wip"
        ));
        let dir = d.path().join("co");
        let out = crate::proc::std_command("bash")
            .arg("-c")
            .arg(pr_script(
                dir.to_str().unwrap(),
                "m1-pay-api",
                &verified,
                "t",
                "b",
            ))
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        let said = parse_pr(&String::from_utf8_lossy(&out.stdout));
        assert!(
            matches!(said, Some(Pushed::NoGh) | Some(Pushed::Done { .. })),
            "{said:?}"
        );
        assert_eq!(
            sh("git --git-dir=origin.git rev-parse refs/heads/m1-pay-api"),
            verified
        );
    }
}
