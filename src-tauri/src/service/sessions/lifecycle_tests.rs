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

#[test]
fn ensure_script_targets_the_row_own_path_not_the_claude_worktrees_guess() {
    let wt = RemoteWorktree {
        name: "feat",
        branch: Some("feature/feat"),
        path: "/home/u/projects/github.com/o/r/.worktrees/feat",
    };
    let script = ensure_script(
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
    assert!(script.contains("git worktree add"));
    assert!(script.contains("feature/feat"));
}

#[test]
fn ensure_script_skips_worktree_add_for_main() {
    let wt = RemoteWorktree {
        name: "main",
        branch: None,
        path: "/home/u/projects/github.com/o/r",
    };
    let script = ensure_script(
        "/home/u/projects/github.com/o/r",
        "git@github.com:o/r.git",
        Some(&wt),
    );
    assert!(!script.contains("git worktree add"));
    assert!(script.contains("git clone"));
}

#[test]
fn ensure_script_with_no_worktree_only_clones() {
    let script = ensure_script(
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
/// renders it — this is the test that would have caught both.
#[test]
fn ensure_script_quotes_paths_with_shell_metacharacters() {
    let project_root = "/home/u/my projects/o/r";
    let wt_path = "/home/u/my projects/o/r/.worktrees/it's $HOME";
    let wt = RemoteWorktree {
        name: "it's $HOME",
        branch: Some("feature/x"),
        path: wt_path,
    };
    let script = ensure_script(project_root, "git@github.com:o/r.git", Some(&wt));

    // The `dirname` command substitution must be double-quoted so a root
    // with a space in it does not word-split.
    assert!(
        script.contains(&format!("\"$(dirname {})\"", quote(project_root))),
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
        "branch must be shell-quoted: {script}"
    );
}

#[test]
fn ensure_script_clone_url_and_worktree_add_target_the_scanned_path() {
    // Sanity check on the non-main worktree-add step's shape: `cd <root> &&
    // git worktree add <abs path> <branch>`, guarded by an existence check on
    // the abs path (not a name-derived guess).
    let wt = RemoteWorktree {
        name: "feat",
        branch: None, // falls back to `name` as the branch
        path: "/r/.worktrees/feat",
    };
    let script = ensure_script("/r", "git@github.com:o/r.git", Some(&wt));
    assert!(script.contains(&format!(
        "if [ ! -d {} ]; then cd {} && git worktree add {} {}; fi",
        quote("/r/.worktrees/feat"),
        quote("/r"),
        quote("/r/.worktrees/feat"),
        quote("feat"),
    )));
}
