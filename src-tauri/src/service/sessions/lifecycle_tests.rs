//! Tests for the worktree-row-belongs-to-one-host guard and the remote
//! ensure-script builder in `lifecycle.rs`.

use super::*;

#[test]
fn a_local_row_is_refused_for_a_remote_host() {
    let err = reject_foreign_worktree("mefistos", "local", "nifty-swanson").unwrap_err();
    assert_eq!(err.code, "E_INVALID");
    assert!(err.message.contains("nifty-swanson"));
    assert!(err.message.contains("mefistos"));
}

#[test]
fn a_row_of_the_same_host_or_a_local_target_passes() {
    assert!(reject_foreign_worktree("mefistos", "mefistos", "w").is_ok());
    assert!(reject_foreign_worktree("local", "local", "w").is_ok());
}

#[test]
fn a_remote_row_for_local_is_refused_too() {
    assert!(reject_foreign_worktree("local", "mefistos", "w").is_err());
}

/// Pin: the worktree is created at the row's OWN scanned path — here under
/// `.worktrees/`, never the `.claude/worktrees/<name>` guess. This is one of
/// the two behaviours this branch added that the merge with origin/main's
/// Mirror-based builder must not regress.
#[test]
fn ensure_script_targets_the_row_own_path_not_the_claude_worktrees_guess() {
    let wt = RemoteWorktree {
        name: "feat",
        branch: Some("feature/feat"),
        path: "/home/u/projects/github.com/o/r/.worktrees/feat",
    };
    let script = ensure_remote_project_script(
        "/home/u/projects/github.com/o/r",
        "git@github.com:o/r.git",
        Some(&wt),
    );
    assert!(
        script.contains("/home/u/projects/github.com/o/r/.worktrees/feat"),
        "script should target the row's own scanned path: {script}"
    );
    assert!(
        !script.contains(".claude/worktrees/feat"),
        "script must not fall back to the .claude/worktrees/<name> guess: {script}"
    );
    // The guard checks existence of the scanned path, and the Mirror step
    // (not a naive one-liner) does the add.
    assert!(
        script.contains("if [ ! -d '/home/u/projects/github.com/o/r/.worktrees/feat' ]; then\n")
    );
    assert!(script.contains("worktree add"));
    assert!(script.contains("feature/feat"));
}

#[test]
fn ensure_script_skips_worktree_add_for_main() {
    let wt = RemoteWorktree {
        name: "main",
        branch: None,
        path: "/home/u/projects/github.com/o/r",
    };
    let script = ensure_remote_project_script(
        "/home/u/projects/github.com/o/r",
        "git@github.com:o/r.git",
        Some(&wt),
    );
    assert!(!script.contains("git worktree add"));
    assert!(script.contains("git clone"));
}

#[test]
fn ensure_script_with_no_worktree_only_clones() {
    let script = ensure_remote_project_script(
        "/home/u/projects/github.com/o/r",
        "git@github.com:o/r.git",
        None,
    );
    assert!(script.contains("git clone"));
    assert!(!script.contains("git worktree add"));
}

/// The regression case for the unquoted `mkdir -p $(dirname {root})`: a
/// project root containing a space would word-split the command
/// substitution, and a worktree path containing shell metacharacters
/// (space, `'`, `$`) must come through exactly as `crate::shell::quote`
/// renders it — this is the test that would have caught both. Pin: every
/// value interpolated anywhere in the merged script — the clone guard AND
/// the Mirror add step's path/branch — is quoted, and the `dirname`
/// substitution is double-quoted.
#[test]
fn ensure_script_quotes_paths_with_shell_metacharacters() {
    let project_root = "/home/u/my projects/o/r";
    let wt_path = "/home/u/my projects/o/r/.worktrees/it's $HOME";
    let wt = RemoteWorktree {
        name: "it's $HOME",
        branch: Some("feature/x"),
        path: wt_path,
    };
    let script = ensure_remote_project_script(project_root, "git@github.com:o/r.git", Some(&wt));

    // The `dirname --` command substitution must be double-quoted so a root
    // with a space in it does not word-split.
    assert!(
        script.contains(&format!("\"$(dirname -- {})\"", quote(project_root))),
        "dirname substitution must be double-quoted: {script}"
    );
    // Every interpolated value must appear exactly as `quote` renders it.
    assert!(
        script.contains(&quote(project_root)),
        "project root must be shell-quoted: {script}"
    );
    assert!(
        script.contains(&quote(wt_path)),
        "worktree path must be shell-quoted: {script}"
    );
    assert!(
        script.contains(&quote("feature/x")),
        "branch must be shell-quoted (as the $b assignment): {script}"
    );
}

/// Pin: the Mirror step's `worktree add` (the local-branch fast path) is
/// rendered against the row's own scanned path, quoted — not a name-derived
/// guess, and not a naive unconditional `git worktree add`.
#[test]
fn ensure_script_worktree_add_targets_the_scanned_path() {
    let wt = RemoteWorktree {
        name: "feat",
        branch: None, // falls back to `name` as the mirrored branch
        path: "/r/.worktrees/feat",
    };
    let script = ensure_remote_project_script("/r", "git@github.com:o/r.git", Some(&wt));
    assert!(
        script.contains(&format!(
            "if [ ! -d {} ]; then\n",
            quote("/r/.worktrees/feat")
        )),
        "{script}"
    );
    assert!(script.contains("b='feat'\n"), "{script}");
    assert!(
        script.contains(&format!(
            "git -C {} worktree add -- {} \"$b\"",
            quote("/r"),
            quote("/r/.worktrees/feat"),
        )),
        "local-branch fast path targets the scanned path: {script}"
    );
}

/// Composition test: the two merged behaviours work together. A
/// `RemoteWorktree` whose `path` was scanned under `.worktrees/` (this
/// branch's contribution) produces a Mirror step (origin/main's
/// contribution) whose `worktree add --track` step also targets that exact
/// scanned path, quoted — proving the scanned path survives all the way
/// through the Mirror rendering, not just the outer existence guard.
#[test]
fn scanned_worktrees_path_survives_the_mirror_rendering() {
    let wt = RemoteWorktree {
        name: "feat",
        branch: Some("feature/feat"),
        path: "/repo/.worktrees/feat",
    };
    let script = ensure_remote_project_script("/repo", "git@github.com:o/r.git", Some(&wt));
    assert!(
        script.contains(&format!(
            "worktree add --track -b \"$b\" -- {} \"origin/$b\"",
            quote("/repo/.worktrees/feat"),
        )),
        "the origin-tracking branch of the Mirror add must target the \
         row's own .worktrees/ path, not a .claude/worktrees/ guess: {script}"
    );
}

// ── the deleted-but-still-registered worktree ─────────────────────────────

#[test]
fn ensure_remote_project_script_guards_the_add_on_the_paths_own_registration() {
    let wt = RemoteWorktree {
        name: "feat",
        branch: Some("feature/feat"),
        path: "/repo/.worktrees/feat",
    };
    let script = ensure_remote_project_script("/repo", "git@github.com:o/r.git", Some(&wt));
    assert!(
        script.contains("worktree list --porcelain"),
        "the add must be guarded on git's own registration list: {script}"
    );
    assert!(
        script.contains(r#"grep -Fxq -e "worktree $wt" -e "worktree $wtc""#),
        "the guard must match this path in either spelling: {script}"
    );
    assert!(
        script.contains(&format!("wt={}\n", quote("/repo/.worktrees/feat"))),
        "the guarded path is the row's own, shell-quoted: {script}"
    );
    assert!(
        !script.contains("worktree prune"),
        "a repo-wide prune would discard other worktrees' registrations: {script}"
    );
}

fn git_ok(args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `git init` + one commit on `main`, with `origin` pointing at a bare repo
/// so the Mirror add can consult it.
fn init_repo_with_origin(base: &std::path::Path) -> std::path::PathBuf {
    let origin = base.join("origin.git");
    let root = base.join("repo");
    let (o, r) = (origin.to_str().unwrap(), root.to_str().unwrap());
    git_ok(&["init", "--bare", "-b", "main", o]);
    git_ok(&["init", "-b", "main", r]);
    git_ok(&["-C", r, "config", "user.email", "t@t"]);
    git_ok(&["-C", r, "config", "user.name", "T"]);
    std::fs::write(root.join("f"), "x").unwrap();
    git_ok(&["-C", r, "add", "."]);
    git_ok(&["-C", r, "commit", "-q", "-m", "init"]);
    git_ok(&["-C", r, "remote", "add", "origin", o]);
    git_ok(&["-C", r, "push", "-q", "-u", "origin", "main"]);
    root
}

fn run_script(script: &str) -> (bool, String) {
    let out = std::process::Command::new("bash")
        .arg("-c")
        .arg(script)
        .output()
        .expect("bash");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
    )
}

/// End to end against real git: the worktree directory was deleted but git
/// still lists its registration. No add can recover from that ("missing but
/// already registered worktree") and git never prunes on its own, so the
/// script must skip the add and exit 0 — otherwise `ensure_remote_project`
/// fails with E_GIT_SETUP before `repair::ensure_for_new_session` gets its
/// chance to unregister and re-add.
#[test]
fn ensure_remote_project_script_tolerates_a_deleted_but_registered_worktree() {
    let base = tempfile::TempDir::new().unwrap();
    let root = init_repo_with_origin(base.path());
    let (r, wt) = (root.to_str().unwrap(), root.join(".worktrees/feat"));
    git_ok(&[
        "-C",
        r,
        "worktree",
        "add",
        "-q",
        wt.to_str().unwrap(),
        "-b",
        "feat",
    ]);
    std::fs::remove_dir_all(&wt).unwrap();
    assert!(!wt.exists(), "directory deleted, registration left behind");

    let row = RemoteWorktree {
        name: "feat",
        branch: Some("feat"),
        path: wt.to_str().unwrap(),
    };
    // The clone URL is unreachable on purpose: `<root>/.git` exists, so a
    // passing run also proves the clone step stayed a no-op.
    let script = ensure_remote_project_script(r, "git@github.com:o/does-not-exist.git", Some(&row));
    let (ok, stderr) = run_script(&script);
    assert!(
        ok,
        "script must not abort on a stale registration: {stderr}"
    );
    assert!(
        !wt.exists(),
        "the add is skipped, not forced; repair owns the unregister + re-add"
    );
    let listed = std::process::Command::new("git")
        .args(["-C", r, "worktree", "list", "--porcelain"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&listed.stdout).contains(wt.to_str().unwrap()),
        "the stale registration must be left intact, not pruned"
    );
}

/// The guard must not break the case it protects: nothing registered and no
/// directory, so the Mirror add runs and checks the worktree out.
#[test]
fn ensure_remote_project_script_still_adds_an_unregistered_worktree() {
    let base = tempfile::TempDir::new().unwrap();
    let root = init_repo_with_origin(base.path());
    let (r, wt) = (root.to_str().unwrap(), root.join(".worktrees/feat"));
    git_ok(&["-C", r, "branch", "feat"]);

    let row = RemoteWorktree {
        name: "feat",
        branch: Some("feat"),
        path: wt.to_str().unwrap(),
    };
    let script = ensure_remote_project_script(r, "git@github.com:o/does-not-exist.git", Some(&row));
    let (ok, stderr) = run_script(&script);
    assert!(ok, "add should succeed: {stderr}");
    assert!(wt.join(".git").exists(), "worktree checked out at {wt:?}");
}

/// The tolerance is scoped to the stale-registration case: a branch on
/// neither the host nor origin is still refused, so the user keeps the
/// actionable "push it first" error.
#[test]
fn ensure_remote_project_script_still_refuses_an_unpushed_branch() {
    let base = tempfile::TempDir::new().unwrap();
    let root = init_repo_with_origin(base.path());
    let (r, wt) = (root.to_str().unwrap(), root.join(".worktrees/nope"));

    let row = RemoteWorktree {
        name: "nope",
        branch: Some("no-such-branch"),
        path: wt.to_str().unwrap(),
    };
    let script = ensure_remote_project_script(r, "git@github.com:o/does-not-exist.git", Some(&row));
    let (ok, stderr) = run_script(&script);
    assert!(
        !ok,
        "an unmirrorable branch must still surface as a failure"
    );
    assert!(
        stderr.contains(crate::service::repair::MIRROR_REFUSED),
        "{stderr}"
    );
}
