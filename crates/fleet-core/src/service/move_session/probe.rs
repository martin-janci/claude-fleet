//! Read-only target probes: nothing in this file writes — to the target's
//! worktree, its index, its refs, or its filesystem. Every git invocation
//! runs under `GIT_OPTIONAL_LOCKS=0`, which is what makes that literally
//! true of `git status`: without it, `git status` refreshes and rewrites the
//! index's stat cache, which is itself a write. See
//! `docs/superpowers/specs/2026-09-21-transfer-preflight` (slice 3).

use super::carry::{parse_err, payload_str, FAILED, OUT_MARKER, STATUS_PORCELAIN};
use crate::ipc_error::IpcError;
use crate::service::safe_kill::{parse_porcelain, DirtyFile};
use crate::shell::quote;

/// What sits at the path the move would aim at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probed {
    /// Nothing there: the move would create it.
    Absent,
    /// A git worktree, its HEAD, and its porcelain (empty when clean).
    Worktree {
        head: String,
        porcelain: Vec<DirtyFile>,
    },
}

/// Look at `cwd` and change nothing. Prints [`OUT_MARKER`] then either
/// `absent`, or `worktree`, a HEAD line, and [`STATUS_PORCELAIN`]'s output.
/// Every git call is prefixed with `GIT_OPTIONAL_LOCKS=0` — without it `git
/// status` refreshes and rewrites the index's stat cache, which is a write.
pub fn target_probe_script(cwd: &str) -> String {
    format!(
        r#"# cf-probe:target
set +e
cwd={cwd}
if [ ! -e "$cwd" ]; then printf '\n{OUT_MARKER}\nabsent\n'; exit 0; fi
cd -- "$cwd" 2>/dev/null || {{ printf '{FAILED} cd\n' >&2; exit 5; }}
head=$(GIT_OPTIONAL_LOCKS=0 git rev-parse HEAD 2>/dev/null)
if [ -z "$head" ]; then printf '{FAILED} head\n' >&2; exit 5; fi
printf '\n{OUT_MARKER}\nworktree\n%s\n' "$head"
GIT_OPTIONAL_LOCKS=0 git {status}
"#,
        cwd = quote(cwd),
        status = STATUS_PORCELAIN,
    )
}

/// Parse [`target_probe_script`]'s output.
pub fn parse_target_probe(stdout: &str) -> Result<Probed, IpcError> {
    let body = payload_str(stdout).ok_or_else(|| parse_err("target-probe", stdout))?;
    let mut lines = body.lines();
    match lines.next().map(str::trim) {
        Some("absent") => Ok(Probed::Absent),
        Some("worktree") => {
            let head = lines
                .next()
                .map(str::trim)
                .filter(|h| !h.is_empty())
                .ok_or_else(|| parse_err("target-probe", stdout))?
                .to_string();
            let porcelain = lines.collect::<Vec<_>>().join("\n");
            Ok(Probed::Worktree {
                head,
                porcelain: parse_porcelain(&porcelain),
            })
        }
        _ => Err(parse_err("target-probe", stdout)),
    }
}

/// On the TARGET: the tip of `branch` in the clone at `project_root`, or
/// nothing when there is no clone or no such branch. Never touches the
/// network — a local ref lookup only.
pub fn target_tip_script(project_root: &str, branch: &str) -> String {
    format!(
        r#"# cf-probe:tip
set +e
r={r}
br={br}
if [ ! -e "$r/.git" ]; then printf '\n{OUT_MARKER}\nnone\n'; exit 0; fi
tip=$(GIT_OPTIONAL_LOCKS=0 git -C "$r" rev-parse --verify --quiet "refs/heads/$br" 2>/dev/null)
if [ -z "$tip" ]; then printf '\n{OUT_MARKER}\nnone\n'; exit 0; fi
printf '\n{OUT_MARKER}\n%s\n' "$tip"
"#,
        r = quote(project_root),
        br = quote(branch),
    )
}

/// Parse [`target_tip_script`]'s output.
pub fn parse_target_tip(stdout: &str) -> Result<Option<String>, IpcError> {
    let body = payload_str(stdout).ok_or_else(|| parse_err("target-tip", stdout))?;
    let line = body.lines().next().map(str::trim).unwrap_or("");
    if line.is_empty() || line == "none" {
        return Ok(None);
    }
    Ok(Some(line.to_string()))
}

/// On the SOURCE: how many commits `HEAD` has that `tip` lacks, or nothing
/// when the source does not have `tip` at all — then the target holds
/// commits the source has never seen, and a count would be a guess.
pub fn commits_ahead_script(worktree: &str, tip: &str) -> String {
    format!(
        r#"# cf-probe:ahead
set +e
cwd={cwd}
tip={tip}
cd -- "$cwd" 2>/dev/null || {{ printf '{FAILED} cd\n' >&2; exit 5; }}
if ! GIT_OPTIONAL_LOCKS=0 git cat-file -e "$tip^{{commit}}" 2>/dev/null; then
  printf '\n{OUT_MARKER}\nunknown\n'; exit 0
fi
n=$(GIT_OPTIONAL_LOCKS=0 git rev-list --count "$tip..HEAD" 2>/dev/null)
if [ -z "$n" ]; then printf '\n{OUT_MARKER}\nunknown\n'; exit 0; fi
printf '\n{OUT_MARKER}\n%s\n' "$n"
"#,
        cwd = quote(worktree),
        tip = quote(tip),
    )
}

/// Parse [`commits_ahead_script`]'s output.
pub fn parse_commits_ahead(stdout: &str) -> Result<Option<u32>, IpcError> {
    let body = payload_str(stdout).ok_or_else(|| parse_err("commits-ahead", stdout))?;
    let line = body.lines().next().map(str::trim).unwrap_or("");
    if line.is_empty() || line == "unknown" {
        return Ok(None);
    }
    line.parse::<u32>()
        .map(Some)
        .map_err(|_| parse_err("commits-ahead", stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::move_session::carry::tests::{bash, git, require};

    #[test]
    fn a_missing_path_is_absent() {
        if !require(&["bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let out = bash(
            &target_probe_script(tmp.path().join("nope").to_str().unwrap()),
            tmp.path(),
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            parse_target_probe(&String::from_utf8_lossy(&out.stdout)).unwrap(),
            Probed::Absent
        );
    }

    #[test]
    fn a_clean_worktree_reports_its_head_and_no_entries() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        git(&wt, &["init", "-q", "-b", "feat"]);
        std::fs::write(wt.join("a.txt"), "a\n").unwrap();
        git(&wt, &["add", "a.txt"]);
        git(&wt, &["commit", "-q", "-m", "a"]);
        let head = git(&wt, &["rev-parse", "HEAD"]).trim().to_string();
        let out = bash(&target_probe_script(wt.to_str().unwrap()), tmp.path());
        match parse_target_probe(&String::from_utf8_lossy(&out.stdout)).unwrap() {
            Probed::Worktree { head: h, porcelain } => {
                assert_eq!(h, head);
                assert!(porcelain.is_empty(), "{porcelain:?}");
            }
            other => panic!("expected a worktree, got {other:?}"),
        }
    }

    #[test]
    fn a_dirty_worktree_lists_what_git_status_lists() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        git(&wt, &["init", "-q", "-b", "feat"]);
        std::fs::write(wt.join("a.txt"), "a\n").unwrap();
        git(&wt, &["add", "a.txt"]);
        git(&wt, &["commit", "-q", "-m", "a"]);
        std::fs::write(wt.join("a.txt"), "changed\n").unwrap();
        std::fs::write(wt.join("new file.txt"), "n\n").unwrap();
        let out = bash(&target_probe_script(wt.to_str().unwrap()), tmp.path());
        let Probed::Worktree { porcelain, .. } =
            parse_target_probe(&String::from_utf8_lossy(&out.stdout)).unwrap()
        else {
            panic!("expected a worktree");
        };
        let paths: Vec<_> = porcelain.iter().map(|d| d.path.as_str()).collect();
        assert!(paths.contains(&"a.txt"), "{paths:?}");
        assert!(
            paths.iter().any(|p| p.contains("new file.txt")),
            "{paths:?}"
        );
    }

    #[test]
    fn the_probe_writes_nothing() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        git(&wt, &["init", "-q", "-b", "feat"]);
        let a = wt.join("a.txt");
        std::fs::write(&a, "a\n").unwrap();
        git(&wt, &["add", "a.txt"]);
        git(&wt, &["commit", "-q", "-m", "a"]);
        // Settle the index's stat cache first, so the only thing that could
        // change it afterwards is the probe itself.
        git(&wt, &["update-index", "-q", "--refresh"]);
        // Bump the tracked file's mtime forward WITHOUT touching its content:
        // this is the classic "racily clean" case where `git status` must
        // rehash the file to confirm it is unchanged, and — having found no
        // difference — opportunistically rewrites the index with the fresh
        // stat info UNLESS GIT_OPTIONAL_LOCKS=0 stops it. A file that is
        // genuinely dirty never exercises this path (the original brief's
        // setup wrote new content to a.txt, which this test proved does not
        // actually trigger a write either with or without the env var — so
        // this setup was substituted to make the assertion meaningful; see
        // the task report).
        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
        std::fs::File::open(&a)
            .unwrap()
            .set_modified(future)
            .unwrap();
        let index_before = std::fs::read(wt.join(".git/index")).unwrap();
        bash(&target_probe_script(wt.to_str().unwrap()), tmp.path());
        let index_after = std::fs::read(wt.join(".git/index")).unwrap();
        // Byte-identical, not merely equivalent: the probe runs `git status`
        // under GIT_OPTIONAL_LOCKS=0, which never writes the index. Without
        // that variable `git status` refreshes the stat cache and this fails —
        // which is the point of asserting on the bytes.
        assert_eq!(index_before, index_after, "the probe wrote the index");
        assert!(
            !wt.join(".git/index.lock").exists(),
            "a lock was left behind"
        );
        // And an absent path is not created.
        let ghost = tmp.path().join("ghost");
        bash(&target_probe_script(ghost.to_str().unwrap()), tmp.path());
        assert!(!ghost.exists(), "the probe created the path it looked at");
    }

    #[test]
    fn commits_ahead_counts_what_the_tip_lacks_and_refuses_to_guess() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        git(&wt, &["init", "-q", "-b", "feat"]);
        for n in ["a", "b", "c"] {
            std::fs::write(wt.join(n), n).unwrap();
            git(&wt, &["add", n]);
            git(&wt, &["commit", "-q", "-m", n]);
        }
        let first = git(&wt, &["rev-list", "--max-parents=0", "HEAD"])
            .trim()
            .to_string();
        let out = bash(
            &commits_ahead_script(wt.to_str().unwrap(), &first),
            tmp.path(),
        );
        assert_eq!(
            parse_commits_ahead(&String::from_utf8_lossy(&out.stdout)).unwrap(),
            Some(2)
        );
        // A tip the source has never seen: unknown, not zero.
        let out = bash(
            &commits_ahead_script(wt.to_str().unwrap(), &"f".repeat(40)),
            tmp.path(),
        );
        assert_eq!(
            parse_commits_ahead(&String::from_utf8_lossy(&out.stdout)).unwrap(),
            None
        );
    }

    #[test]
    fn target_tip_is_none_without_a_clone_or_a_branch() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let out = bash(
            &target_tip_script(tmp.path().join("no-clone").to_str().unwrap(), "feat"),
            tmp.path(),
        );
        assert_eq!(
            parse_target_tip(&String::from_utf8_lossy(&out.stdout)).unwrap(),
            None
        );
        let clone = tmp.path().join("clone");
        std::fs::create_dir_all(&clone).unwrap();
        git(&clone, &["init", "-q", "-b", "main"]);
        std::fs::write(clone.join("a"), "a").unwrap();
        git(&clone, &["add", "a"]);
        git(&clone, &["commit", "-q", "-m", "a"]);
        let out = bash(
            &target_tip_script(clone.to_str().unwrap(), "feat"),
            tmp.path(),
        );
        assert_eq!(
            parse_target_tip(&String::from_utf8_lossy(&out.stdout)).unwrap(),
            None
        );
        let out = bash(
            &target_tip_script(clone.to_str().unwrap(), "main"),
            tmp.path(),
        );
        assert!(parse_target_tip(&String::from_utf8_lossy(&out.stdout))
            .unwrap()
            .is_some());
    }
}
