//! A task starts from the work of the tasks it depends on (mission
//! improvements M2, design 2026-10-26 §4). A dependency is done once it is
//! verified, long before a person merges its pull request, so a dependent
//! run that began from the default branch lacked the code it builds on and
//! redid it or conflicted with it. Now its checkout takes the dependencies'
//! branches before its prompt is written:
//!
//! - [`unmerged_deps`] names them: the item's direct dependencies that are
//!   done, in the same repository, have a branch from an implement run, and
//!   whose pull request is not merged;
//! - [`merge_script`] merges each into the checkout (`git merge --no-edit`)
//!   unless the checkout or the default branch already holds it, and backs
//!   out of one that does not merge cleanly (`git merge --abort`);
//! - [`note`] tells the worker what its checkout holds and which branch it
//!   must merge itself;
//! - [`prepare`] does all of it for a run's checkout and answers the head
//!   the run starts at.
//!
//! A dependency in a lane merges its verified commit ([`verified_commit`]),
//! not the lane's branch: the lane's lead has moved on past it. The run's
//! `start_head` is read after the merges, so its evidence and its review's
//! diff are its own commits only.

use crate::ipc_error::IpcError;
use crate::shell::quote as shq;
use crate::store::Store;
use std::time::Duration;

/// The most dependencies one checkout takes.
pub const DEP_BRANCHES_MAX: usize = 8;

/// The wall clock of reading a checkout's HEAD.
const HEAD_READ: Duration = Duration::from_secs(20);

/// A dependency whose work is not on the default branch yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepBranch {
    pub item_id: i64,
    /// Its key, else `#<id>`.
    pub label: String,
    pub branch: String,
    /// The commit to merge instead of the branch's tip: a lane task's
    /// verified commit, as the lane's branch moves on past it.
    pub commit: Option<String>,
    /// Where its last implement run worked.
    pub host_alias: String,
    pub host_up: bool,
}

/// What the merge script did with one branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Merged {
    /// Merged into the checkout now.
    Took,
    /// The checkout already holds it (the base, or an earlier merge).
    Had,
    /// The default branch holds it.
    Landed,
    /// Did not merge cleanly; backed out.
    Conflict,
    /// Not on this host's repository nor on `origin`.
    Missing,
}

/// The dependencies of `item_id` whose work its checkout should start from,
/// in the order of their ids. `skip_lane`: a lane's own tasks are in its
/// checkout already, so a dependency in that lane is left out.
pub fn unmerged_deps(
    s: &Store,
    item_id: i64,
    skip_lane: Option<i64>,
) -> Result<Vec<DepBranch>, IpcError> {
    let Some(item) = s.get_work_item(item_id)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for e in s.item_deps(&[item_id])? {
        let Some(dep) = s.get_work_item(e.depends_on)? else {
            continue;
        };
        if dep.status_category != "done" {
            continue;
        }
        // A task that names no repository may be in any: the work graph
        // only links tasks that work together.
        if let (Some(a), Some(b)) = (item.project_id, dep.project_id) {
            if a != b {
                continue;
            }
        }
        let dep_lane = s.item_lane(dep.id)?;
        if skip_lane.is_some() && dep_lane == skip_lane {
            continue;
        }
        let Some(prior) = super::run::prior_work(s, dep.id)? else {
            continue;
        };
        if s.branch_pull_request_merged(&prior.branch, dep.project_id)? {
            continue;
        }
        out.push(DepBranch {
            item_id: dep.id,
            label: dep.key.clone().unwrap_or_else(|| format!("#{}", dep.id)),
            branch: prior.branch,
            commit: match dep_lane {
                Some(_) => verified_commit(s, dep.id)?.map(|(_, h)| h),
                None => None,
            },
            host_alias: prior.host_alias,
            host_up: prior.host_up,
        });
        if out.len() >= DEP_BRANCHES_MAX {
            break;
        }
    }
    Ok(out)
}

/// The commit `item_id`'s latest implement run ended on, as its evidence
/// read it, with that run's id; `None` before one has.
pub fn verified_commit(s: &Store, item_id: i64) -> Result<Option<(i64, String)>, IpcError> {
    let Some(t) = s.latest_item_task(item_id, "implement")? else {
        return Ok(None);
    };
    Ok(t.evidence
        .as_ref()
        .and_then(|e| e.head.clone())
        .filter(|h| crate::store::is_commit_id(h))
        .map(|h| (t.id, h)))
}

/// The script that merges `deps`' branches into the checkout at `dir`, one
/// `__FLEET_DEP <i> <took|had|landed|conflict|missing>` line per branch,
/// then `__FLEET_DEP_HEAD <sha>`. A branch is found locally, else on
/// `origin` (fetched first); a dependency's own commit is merged when it
/// names one, and is missing when the repository lacks it. A merge commit
/// takes the repository's own identity, or fleet's when it has none.
pub fn merge_script(dir: &str, deps: &[DepBranch]) -> String {
    let mut s = format!(
        "cd -- {} || exit 0\n\
         git fetch -q origin 2>/dev/null || true\n\
         def=$(git symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null)\n\
         who=; git config user.email >/dev/null 2>&1 || who='-c user.name=fleet -c user.email=fleet@localhost'\n",
        shq(dir)
    );
    for (i, d) in deps.iter().enumerate() {
        s.push_str(&format!(
            "b={b}; r=\n\
             if git show-ref --verify --quiet \"refs/heads/$b\"; then r=\"$b\"\n\
             elif git show-ref --verify --quiet \"refs/remotes/origin/$b\"; then r=\"origin/$b\"; fi\n\
             {pin}\
             if [ -z \"$r\" ]; then echo \"__FLEET_DEP {i} missing\"\n\
             elif git merge-base --is-ancestor \"$r\" HEAD 2>/dev/null; then echo \"__FLEET_DEP {i} had\"\n\
             elif [ -n \"$def\" ] && git merge-base --is-ancestor \"$r\" \"$def\" 2>/dev/null; then echo \"__FLEET_DEP {i} landed\"\n\
             elif git $who merge -q --no-edit \"$r\" >/dev/null 2>&1; then echo \"__FLEET_DEP {i} took\"\n\
             else git merge --abort >/dev/null 2>&1; echo \"__FLEET_DEP {i} conflict\"; fi\n",
            b = shq(&d.branch),
            pin = match &d.commit {
                Some(c) => format!(
                    "c={c}; r=; git cat-file -e \"$c^{{commit}}\" 2>/dev/null && r=\"$c\"\n",
                    c = shq(c)
                ),
                None => String::new(),
            },
        ));
    }
    s.push_str("echo \"__FLEET_DEP_HEAD $(git rev-parse HEAD 2>/dev/null)\"\n");
    s
}

/// PURE: what the script did with each of `n` branches, by index (a branch
/// it said nothing of is `Missing`), and the checkout's head after.
pub fn parse_merge(stdout: &str, n: usize) -> (Vec<Merged>, Option<String>) {
    let mut out = vec![Merged::Missing; n];
    let mut head = None;
    for line in stdout.lines() {
        if let Some(h) = line.strip_prefix("__FLEET_DEP_HEAD ") {
            let h = h.trim();
            if crate::store::is_commit_id(h) {
                head = Some(h.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("__FLEET_DEP ") {
            let mut p = rest.split(' ');
            let (Some(i), Some(what)) = (p.next(), p.next()) else {
                continue;
            };
            let Ok(i) = i.parse::<usize>() else {
                continue;
            };
            if let Some(slot) = out.get_mut(i) {
                *slot = match what.trim() {
                    "took" => Merged::Took,
                    "had" => Merged::Had,
                    "landed" => Merged::Landed,
                    "conflict" => Merged::Conflict,
                    _ => Merged::Missing,
                };
            }
        }
    }
    (out, head)
}

/// PURE: what the run's prompt says about its dependencies' work; `None`
/// when the checkout took none and nothing is left to merge.
pub fn note(deps: &[DepBranch], merged: &[Merged]) -> Option<String> {
    let names = |want: &[Merged]| -> Vec<String> {
        deps.iter()
            .zip(merged)
            .filter(|(_, m)| want.contains(m))
            .map(|(d, _)| match &d.commit {
                Some(c) => format!(
                    "{} (commit `{}` of branch `{}`)",
                    d.label,
                    c.chars().take(12).collect::<String>(),
                    d.branch
                ),
                None => format!("{} (branch `{}`)", d.label, d.branch),
            })
            .collect()
    };
    let held = names(&[Merged::Took, Merged::Had]);
    let conflict = names(&[Merged::Conflict]);
    let missing = names(&[Merged::Missing]);
    let mut parts: Vec<String> = Vec::new();
    if !held.is_empty() {
        parts.push(format!(
            "Your checkout already holds the work of the tasks this one depends on: {}. \
             Their pull requests may not be merged yet: build on that work rather than \
             redoing it, and say in your pull request that it stacks on theirs.",
            held.join(", ")
        ));
    }
    if !conflict.is_empty() {
        parts.push(format!(
            "These did not merge cleanly into your checkout: {}. Merge each yourself \
             first (`git merge <branch>`), resolve the conflicts and commit, then do the task.",
            conflict.join(", ")
        ));
    }
    if !missing.is_empty() {
        parts.push(format!(
            "This task depends on work your checkout could not find: {}. Fetch it \
             (`git fetch origin <branch>`) and merge it before you start.",
            missing.join(", ")
        ));
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// Merge `deps` into the checkout at `dir` on `host`; what each came to and
/// the head after. A host that does not answer leaves every branch
/// `Missing` and no head, so the prompt asks the worker to merge them.
pub async fn merge_into(
    ssh: &std::sync::Arc<crate::ssh::SshClient>,
    host: &str,
    dir: &str,
    deps: &[DepBranch],
) -> (Vec<Merged>, Option<String>) {
    if deps.is_empty() {
        return (Vec::new(), None);
    }
    match crate::service::catalog::inventory::run_host_script(ssh, host, &merge_script(dir, deps))
        .await
    {
        Ok(out) => parse_merge(&out, deps.len()),
        Err(e) => {
            tracing::debug!(%host, error = %e.message, "[dep_work] dependencies not merged");
            (vec![Merged::Missing; deps.len()], None)
        }
    }
}

/// The commit the checkout at `dir` on `host` stands on; `None` when it
/// could not be read.
pub async fn read_head(
    ssh: &std::sync::Arc<crate::ssh::SshClient>,
    host: &str,
    dir: &str,
) -> Option<String> {
    let script = format!("git -C {} rev-parse HEAD", shq(dir));
    let run = crate::service::catalog::inventory::run_host_script(ssh, host, &script);
    let out = tokio::time::timeout(HEAD_READ, run).await.ok()?.ok()?;
    let sha = out.trim().to_string();
    crate::store::is_commit_id(&sha).then_some(sha)
}

/// A run's checkout after its dependencies: what its prompt says of them,
/// and the head its own work starts at.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Prepared {
    pub note: Option<String>,
    pub head: Option<String>,
}

/// Merge `deps` into the checkout at `dir` on `host` ([`merge_into`]) and
/// read the head the run starts at: the merge's, else the checkout's own
/// (a host that did not answer the merge), so the run's evidence never
/// reaches back into its dependencies' commits. No checkout: every
/// dependency is `Missing` and there is no head.
pub async fn prepare(
    ssh: &std::sync::Arc<crate::ssh::SshClient>,
    host: &str,
    dir: Option<&str>,
    deps: &[DepBranch],
) -> Prepared {
    let Some(dir) = dir else {
        return Prepared {
            note: note(deps, &vec![Merged::Missing; deps.len()]),
            head: None,
        };
    };
    let (merged, head) = merge_into(ssh, host, dir, deps).await;
    let head = match head {
        Some(h) => Some(h),
        None => read_head(ssh, host, dir).await,
    };
    Prepared {
        note: note(deps, &merged),
        head,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dep(id: i64, branch: &str) -> DepBranch {
        DepBranch {
            item_id: id,
            label: format!("T-{id}"),
            branch: branch.into(),
            commit: None,
            host_alias: "h".into(),
            host_up: true,
        }
    }

    #[test]
    fn the_script_says_what_each_branch_came_to() {
        let out = "__FLEET_DEP 0 had\n__FLEET_DEP 1 took\n__FLEET_DEP 2 conflict\n\
                   noise\n__FLEET_DEP_HEAD 0123456789abcdef0123456789abcdef01234567\n";
        let (m, head) = parse_merge(out, 4);
        assert_eq!(
            m,
            vec![Merged::Had, Merged::Took, Merged::Conflict, Merged::Missing]
        );
        assert_eq!(
            head.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(parse_merge("__FLEET_DEP_HEAD nope\n", 0).1, None);
    }

    #[test]
    fn the_note_names_what_the_checkout_holds_and_what_is_left_to_merge() {
        let deps = [
            dep(1, "m1-a"),
            dep(2, "m1-b"),
            dep(3, "m1-c"),
            dep(4, "m1-d"),
        ];
        let n = note(
            &deps,
            &[Merged::Had, Merged::Took, Merged::Conflict, Merged::Landed],
        )
        .unwrap();
        assert!(
            n.contains("T-1 (branch `m1-a`), T-2 (branch `m1-b`)"),
            "{n}"
        );
        assert!(
            n.contains("did not merge cleanly into your checkout: T-3"),
            "{n}"
        );
        assert!(!n.contains("T-4"), "a landed branch is the default's: {n}");
        assert_eq!(note(&deps[..1], &[Merged::Landed]), None);
    }

    #[test]
    fn the_script_quotes_every_branch_and_reads_the_head_last() {
        let s = merge_script("/w/x y", &[dep(1, "a'b")]);
        assert!(s.starts_with("cd -- '/w/x y'"), "{s}");
        assert!(s.contains(&shq("a'b")), "{s}");
        assert!(s
            .trim_end()
            .ends_with("__FLEET_DEP_HEAD $(git rev-parse HEAD 2>/dev/null)\""));
    }

    /// The script against real git: a lane dependency merges its verified
    /// commit, not the later work on its lane's branch; a commit the
    /// repository lacks is missing.
    #[test]
    fn real_git_merges_a_lane_dependencys_verified_commit_only() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().to_str().unwrap();
        let sh = |cmd: &str| {
            let out = crate::proc::std_command("bash")
                .arg("-c")
                .arg(format!("cd {} && {cmd}", shq(repo)))
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
        sh(&format!(
            "git init -q -b main && echo base > f && git add f && {g} commit -qm base \
             && git checkout -qb lane-a && echo a > a && git add a && {g} commit -qm a"
        ));
        let verified = sh("git rev-parse HEAD");
        sh(&format!(
            "echo wip > wip && git add wip && {g} commit -qm wip \
             && git checkout -q main && git checkout -qb mine"
        ));
        let pinned = DepBranch {
            commit: Some(verified.clone()),
            ..dep(1, "lane-a")
        };
        let gone = DepBranch {
            commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
            ..dep(2, "lane-a")
        };
        let out = crate::proc::std_command("bash")
            .arg("-c")
            .arg(merge_script(repo, &[pinned.clone(), gone.clone()]))
            .output()
            .unwrap();
        let (m, _) = parse_merge(&String::from_utf8_lossy(&out.stdout), 2);
        assert_eq!(m, vec![Merged::Took, Merged::Missing]);
        assert!(d.path().join("a").exists(), "the verified commit's file");
        assert!(!d.path().join("wip").exists(), "not the lead's later work");
        let n = note(&[pinned, gone], &m).unwrap();
        assert!(n.contains(&format!("commit `{}`", &verified[..12])), "{n}");
    }

    /// The script against real git: a dependency branch merges, one the
    /// checkout holds is `had`, a conflicting one is backed out of.
    #[test]
    fn real_git_merges_a_dependency_and_backs_out_of_a_conflict() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().to_str().unwrap();
        let git = |args: &str| {
            let out = crate::proc::std_command("bash")
                .arg("-c")
                .arg(format!(
                    "cd {} && git -c user.name=t -c user.email=t@t {args}",
                    shq(repo)
                ))
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git("init -q -b main");
        std::fs::write(d.path().join("f"), "base\n").unwrap();
        git("add f && git -c user.name=t -c user.email=t@t commit -qm base");
        git("checkout -qb dep-a");
        std::fs::write(d.path().join("a"), "a\n").unwrap();
        git("add a && git -c user.name=t -c user.email=t@t commit -qm a");
        git("checkout -q main && git checkout -qb dep-b");
        std::fs::write(d.path().join("f"), "b\n").unwrap();
        git("commit -qam b");
        git("checkout -q main && git checkout -qb mine");
        std::fs::write(d.path().join("f"), "mine\n").unwrap();
        git("commit -qam mine");
        let deps = [
            dep(1, "dep-a"),
            dep(2, "dep-b"),
            dep(3, "dep-a"),
            dep(4, "gone"),
        ];
        let out = crate::proc::std_command("bash")
            .arg("-c")
            .arg(merge_script(repo, &deps))
            .output()
            .unwrap();
        let (m, head) = parse_merge(&String::from_utf8_lossy(&out.stdout), deps.len());
        assert_eq!(
            m,
            vec![Merged::Took, Merged::Conflict, Merged::Had, Merged::Missing]
        );
        assert!(head.is_some());
        assert!(
            d.path().join("a").exists(),
            "dep-a's file is in the checkout"
        );
        assert_eq!(
            std::fs::read_to_string(d.path().join("f")).unwrap(),
            "mine\n"
        );
    }
}
