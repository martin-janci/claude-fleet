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
