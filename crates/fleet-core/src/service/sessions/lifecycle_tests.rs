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

/// Reproduced on a live hub: `new_shell_session` with a project id that does
/// not exist answered `E_SQLITE: Query returned no rows` — the raw rusqlite
/// error from a `query_row` that assumed a row. A caller that mistypes an id
/// gets a proper "not found" instead.
#[tokio::test]
async fn an_unknown_project_id_is_not_found_not_a_raw_sqlite_error() {
    let store = Mutex::new(crate::store::Store::open_in_memory().unwrap());
    let ssh = Arc::new(crate::ssh::SshClient::new());
    let reg = crate::cancel::CancellationRegistry::new();
    let args = NewSessionArgs {
        host_alias: "local".into(),
        project_id: 4242,
        worktree_id: None,
        name: "f3unknownproject".into(),
        call_id: None,
        new_worktree: None,
        base_branch: None,
        kind: Some("shell".into()),
        start_command: None,
        friendly_name: None,
        resume_claude_session_id: None,
    };
    let err = new_session(args, &store, &ssh, &reg).await.unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_NOTFOUND);
    assert_eq!(err.message, "project 4242 not found");
}

/// The same for the other two callers of the project lookup: creating a new
/// worktree (local) and the remote owner/repo path.
#[tokio::test]
async fn an_unknown_project_id_is_not_found_for_a_new_worktree_too() {
    let store = Mutex::new(crate::store::Store::open_in_memory().unwrap());
    let ssh = Arc::new(crate::ssh::SshClient::new());
    let reg = crate::cancel::CancellationRegistry::new();
    let args = NewSessionArgs {
        host_alias: "local".into(),
        project_id: 4242,
        worktree_id: None,
        name: "f3unknownproject2".into(),
        call_id: None,
        new_worktree: Some("some-branch".into()),
        base_branch: None,
        kind: Some("shell".into()),
        start_command: None,
        friendly_name: None,
        resume_claude_session_id: None,
    };
    let err = new_session(args, &store, &ssh, &reg).await.unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_NOTFOUND);
    assert_eq!(err.message, "project 4242 not found");
}

/// `fetch_owner_repo` is the shared lookup behind `new_bg_session`,
/// `spawn_review` and every remote-host path.
#[test]
fn fetch_owner_repo_reports_an_unknown_project_as_not_found() {
    let s = crate::store::Store::open_in_memory().unwrap();
    let err = fetch_owner_repo(&s, 4242).unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_NOTFOUND);
    assert_eq!(err.message, "project 4242 not found");
}

/// And the worktree lookup shares the shape: a stale worktree id is a
/// not-found, not a raw sqlite error.
#[test]
fn fetch_worktree_reports_an_unknown_worktree_as_not_found() {
    let s = crate::store::Store::open_in_memory().unwrap();
    let err = fetch_worktree(&s, 77).unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_NOTFOUND);
    assert_eq!(err.message, "worktree 77 not found");
}

// ---- Task 6: resume a given conversation; never reuse a lost session's name ----

const RESUME_ID: &str = "550e8400-e29b-41d4-a716-446655440000";

fn args_named(name: &str, resume: Option<&str>) -> NewSessionArgs {
    NewSessionArgs {
        host_alias: "local".into(),
        project_id: 4242,
        worktree_id: None,
        name: name.into(),
        call_id: None,
        new_worktree: None,
        base_branch: None,
        kind: None,
        start_command: None,
        friendly_name: None,
        resume_claude_session_id: resume.map(str::to_string),
    }
}

/// A `local` row named `name`, ghosted as killed, optionally carrying a
/// Claude conversation id. Returns its id.
fn lost_row(s: &crate::store::Store, name: &str, claude_id: Option<&str>) -> i64 {
    s.upsert_host("local").unwrap();
    s.upsert_session(name, "local", None, None, 1, 1, "running", None)
        .unwrap();
    let id = s.get_session(name, "local").unwrap().unwrap().id;
    if let Some(cid) = claude_id {
        s.set_claude_session_id(id, cid).unwrap();
    }
    s.mark_session_killed(id, 100).unwrap().expect("ghosted");
    id
}

#[tokio::test]
async fn a_lost_session_with_a_conversation_blocks_its_name() {
    let store = Mutex::new(crate::store::Store::open_in_memory().unwrap());
    let id = lost_row(&store.lock().unwrap(), "dev-x", Some(RESUME_ID));
    let ssh = Arc::new(crate::ssh::SshClient::new());
    let reg = crate::cancel::CancellationRegistry::new();
    let err = new_session(args_named("dev-x", None), &store, &ssh, &reg)
        .await
        .unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_EXISTS);
    assert_eq!(
        err.message,
        format!(
            "dev-x belongs to a lost session (id {id}); restore it with restore_host_sessions or dismiss it first"
        )
    );
    let row = store
        .lock()
        .unwrap()
        .get_session_by_id(id)
        .unwrap()
        .unwrap();
    assert_eq!(row.claude_session_id.as_deref(), Some(RESUME_ID));
}

/// A lost row with no conversation has nothing to resume, so the name is
/// free: the call gets past the guard and fails later on the unknown project.
#[tokio::test]
async fn a_lost_session_without_a_conversation_does_not_block() {
    let store = Mutex::new(crate::store::Store::open_in_memory().unwrap());
    lost_row(&store.lock().unwrap(), "dev-x", None);
    let ssh = Arc::new(crate::ssh::SshClient::new());
    let reg = crate::cancel::CancellationRegistry::new();
    let err = new_session(args_named("dev-x", None), &store, &ssh, &reg)
        .await
        .unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_NOTFOUND);
}

/// Shell sessions are guarded too: the check runs before the kind split.
#[tokio::test]
async fn the_lost_name_guard_applies_to_shell_sessions() {
    let store = Mutex::new(crate::store::Store::open_in_memory().unwrap());
    lost_row(&store.lock().unwrap(), "dev-x", Some(RESUME_ID));
    let ssh = Arc::new(crate::ssh::SshClient::new());
    let reg = crate::cancel::CancellationRegistry::new();
    let mut args = args_named("dev-x", None);
    args.kind = Some("shell".into());
    let err = new_session(args, &store, &ssh, &reg).await.unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_EXISTS);
}

#[tokio::test]
async fn an_invalid_resume_id_is_rejected() {
    let store = Mutex::new(crate::store::Store::open_in_memory().unwrap());
    let ssh = Arc::new(crate::ssh::SshClient::new());
    let reg = crate::cancel::CancellationRegistry::new();
    let err = new_session(args_named("dev-y", Some("x; rm -rf")), &store, &ssh, &reg)
        .await
        .unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_INVALID);
}

/// `new_session_inner` threads the pair this returns into BOTH the pane
/// command and `set_claude_session_id`; driving the whole path needs a real
/// tmux, so the choice itself is pinned here.
#[test]
fn a_resume_id_is_used_for_both_the_pane_command_and_the_stored_id() {
    let (cid, pane) = claude_id_and_pane_cmd(&args_named("dev-z", Some(RESUME_ID)));
    assert_eq!(cid.as_deref(), Some(RESUME_ID));
    assert!(
        pane.contains(&format!("--resume '{RESUME_ID}'")),
        "got: {pane}"
    );
    assert_eq!(pane, recreate_pane_command("work", Some(RESUME_ID)));
}

#[test]
fn without_a_resume_id_a_fresh_uuid_is_minted() {
    let (cid, pane) = claude_id_and_pane_cmd(&args_named("dev-z", None));
    let cid = cid.expect("work session gets an id");
    assert_ne!(cid, RESUME_ID);
    assert!(crate::validate::claude_session_id(&cid).is_ok());
    assert!(pane.contains(&format!("--resume '{cid}'")), "got: {pane}");
}

#[test]
fn a_shell_session_has_no_claude_id() {
    let mut args = args_named("dev-z", None);
    args.kind = Some("shell".into());
    let (cid, pane) = claude_id_and_pane_cmd(&args);
    assert_eq!(cid, None);
    assert_eq!(pane, crate::tmux::shell_pane_command(None));
}

#[tokio::test]
async fn a_resume_id_on_a_shell_session_is_rejected() {
    let store = Mutex::new(crate::store::Store::open_in_memory().unwrap());
    let ssh = Arc::new(crate::ssh::SshClient::new());
    let reg = crate::cancel::CancellationRegistry::new();
    let mut args = args_named("dev-y", Some(RESUME_ID));
    args.kind = Some("shell".into());
    let err = new_session(args, &store, &ssh, &reg).await.unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_INVALID);
    assert_eq!(
        err.message,
        "resume_claude_session_id applies to Claude sessions, not shell sessions"
    );
}

/// Resuming a conversation a session on the host already holds — lost or
/// live — is refused; the lost holder is restored instead.
#[tokio::test]
async fn a_resume_id_held_by_a_lost_session_is_refused() {
    let store = Mutex::new(crate::store::Store::open_in_memory().unwrap());
    let id = lost_row(&store.lock().unwrap(), "dev-x", Some(RESUME_ID));
    let ssh = Arc::new(crate::ssh::SshClient::new());
    let reg = crate::cancel::CancellationRegistry::new();
    let err = new_session(args_named("dev-new", Some(RESUME_ID)), &store, &ssh, &reg)
        .await
        .unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_EXISTS);
    assert_eq!(
        err.message,
        format!(
            "conversation {RESUME_ID} already belongs to session {id} (dev-x); restore it with restore_host_sessions instead"
        )
    );
}

#[tokio::test]
async fn a_resume_id_held_by_a_live_session_is_refused() {
    let store = Mutex::new(crate::store::Store::open_in_memory().unwrap());
    {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev-live", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let id = s.get_session("dev-live", "local").unwrap().unwrap().id;
        s.set_claude_session_id(id, RESUME_ID).unwrap();
    }
    let ssh = Arc::new(crate::ssh::SshClient::new());
    let reg = crate::cancel::CancellationRegistry::new();
    let err = new_session(args_named("dev-new", Some(RESUME_ID)), &store, &ssh, &reg)
        .await
        .unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_EXISTS);
}

/// An unheld resume id passes both checks and fails later on the unknown
/// project — the guards do not over-block.
#[tokio::test]
async fn an_unheld_resume_id_gets_past_the_guards() {
    let store = Mutex::new(crate::store::Store::open_in_memory().unwrap());
    let ssh = Arc::new(crate::ssh::SshClient::new());
    let reg = crate::cancel::CancellationRegistry::new();
    let err = new_session(args_named("dev-new", Some(RESUME_ID)), &store, &ssh, &reg)
        .await
        .unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_NOTFOUND);
}
