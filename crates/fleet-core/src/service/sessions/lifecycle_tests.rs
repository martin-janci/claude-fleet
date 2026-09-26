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
/// `new_session` on a system project: the row's `base_path` verbatim, no
/// clone, no worktree — on a remote host as much as on `local`. A worktree
/// request against it is refused rather than guessed.
#[test]
fn a_system_project_pins_the_pane_cwd_and_refuses_worktrees() {
    let s = crate::store::Store::open_in_memory().unwrap();
    let pid = s
        .upsert_system_project("fleet", "operator", "/home/mjanci/.claude-fleet/operator")
        .unwrap();
    let regular = s.upsert_project("acme", "repo", "/base/repo").unwrap();

    assert_eq!(
        system_project_cwd(&s, pid, None, None).unwrap().as_deref(),
        Some("/home/mjanci/.claude-fleet/operator")
    );
    assert_eq!(
        system_project_cwd(&s, regular, None, None).unwrap(),
        None,
        "an ordinary project keeps the ordinary resolution"
    );
    let err = system_project_cwd(&s, pid, Some(3), None).unwrap_err();
    assert_eq!(
        err.code,
        crate::ipc_error::codes::E_INVALID,
        "{}",
        err.message
    );
    let err = system_project_cwd(&s, pid, None, Some("feat-x")).unwrap_err();
    assert_eq!(
        err.code,
        crate::ipc_error::codes::E_INVALID,
        "{}",
        err.message
    );
    let err = system_project_cwd(&s, 4242, None, None).unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_NOTFOUND);
}

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

#[test]
fn a_kill_closes_the_current_conversation_as_killed() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    let id = s
        .upsert_session("k", "local", None, None, 0, 0, "running", None)
        .unwrap();
    let a = "11111111-1111-1111-1111-111111111111";
    s.set_claude_session_id(id, a).unwrap();
    record_kill(&s, id, Some(a));
    let convs = s.list_conversations(id, 10).unwrap();
    let c = convs.iter().find(|c| c.claude_session_id == a).unwrap();
    assert_eq!(c.end_reason.as_deref(), Some("killed"));
    assert!(c.ended_at.is_some());
    // No id (a row never bound): only the timeline event, no error.
    record_kill(&s, id, None);
}

// ── Every path that brings a tmux name back to life must say so ────────────
//
// `record_tmux_created` is what lets a session created under a just-killed
// name be inserted by the reconcile that follows. Nothing else can catch its
// removal: the six functions below resolve their executor through
// `exec_for`, which is not injectable, so exercising them for real would need
// a live tmux server (and macOS CI has none). The calls are therefore pinned
// in the source itself, in the style of the desktop's command-routing audit.

/// The source of the top-level item that starts with `signature`, ending at
/// its closing brace. Only a top-level item's closing brace sits at column 0
/// once rustfmt has run, so the slice stops at this function and cannot run
/// on into the next one.
fn item_source<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source.find(signature).unwrap_or_else(|| {
        panic!("`{signature}` is no longer in its file — update this test to follow it")
    });
    let rest = &source[start..];
    let end = rest
        .find("\n}\n")
        .unwrap_or_else(|| panic!("`{signature}` has no closing brace at column 0"));
    &rest[..end + 3]
}

/// One function that creates (or renames into) a tmux session and then leans
/// on a reconcile to register the row.
struct CreateSite {
    /// Named in the failure message.
    what: &'static str,
    source: &'static str,
    signature: &'static str,
    /// Every tmux call in the body that leaves the name live. All must come
    /// BEFORE the `record_tmux_created` call, so a create that fails its `?`
    /// never forgets a real kill.
    tmux: &'static [&'static str],
    /// The call that registers/restores the row. One of these must follow
    /// the `record_tmux_created` call — a match BEFORE it does not count
    /// (`move_session_inner` reconciles the source host long before it
    /// starts the target).
    registers: &'static str,
}

#[test]
fn every_path_that_creates_a_tmux_session_forgets_the_kill_first() {
    const LIFECYCLE: &str = include_str!("lifecycle.rs");
    const REVIEW: &str = include_str!("review.rs");
    const MOVE: &str = include_str!("../move_session/mod.rs");
    let sites = [
        CreateSite {
            what: "new_session_inner",
            source: LIFECYCLE,
            signature: "pub(super) async fn new_session_inner(",
            tmux: &["tmux.new_session(&args.name, &path, &pane_cmd)"],
            registers: "reconcile_one_host(",
        },
        CreateSite {
            what: "rename_session_with",
            source: LIFECYCLE,
            signature: "pub(super) async fn rename_session_with(",
            tmux: &["tmux.rename_session(&args.old_name, &args.new_name)"],
            registers: "reconcile_one_host(",
        },
        CreateSite {
            what: "restart_session",
            source: LIFECYCLE,
            // All three arms of the respawn match, so the call has to sit
            // after the whole match and not inside one branch.
            signature: "pub async fn restart_session(",
            tmux: &[
                "tmux.new_session(&args.name, std::path::Path::new(&rep.cwd)",
                "tmux.respawn_pane_in(&args.name",
                "tmux.restart_session(&args.name, &pane_cmd)",
            ],
            registers: "reconcile_one_host(",
        },
        CreateSite {
            what: "recreate_session",
            source: LIFECYCLE,
            signature: "pub async fn recreate_session(",
            tmux: &["tmux.new_session(&sess.tmux_name"],
            // This one restores its own row instead of reconciling; the call
            // is here so the invariant holds for every tmux create.
            registers: "restore_session(",
        },
        CreateSite {
            what: "spawn_review",
            source: REVIEW,
            signature: "pub async fn spawn_review(",
            tmux: &["tmux.new_session("],
            registers: "reconcile_one_host(",
        },
        CreateSite {
            what: "move_session_inner (the target host)",
            source: MOVE,
            signature: "async fn move_session_inner(",
            tmux: &[".start_target("],
            registers: ".refresh_host(store, &target)",
        },
    ];

    const WHY: &str = "without it, a kill and a re-create of the same tmux name in the same \
                       unix second leave the reconcile refusing to insert the row, and the \
                       caller fails with \"vanished after creation\" while the tmux session \
                       is really running on the host";
    for site in &sites {
        let body = item_source(site.source, site.signature);
        let forget = body
            .find("record_tmux_created(")
            .unwrap_or_else(|| panic!("{} no longer calls record_tmux_created — {WHY}", site.what));
        // Searched only in what FOLLOWS the forget: an occurrence before it
        // proves nothing, and moving the forget past the last one must fail.
        assert!(
            body[forget..].contains(site.registers),
            "{}: record_tmux_created must come BEFORE `{}` — {WHY}",
            site.what,
            site.registers
        );
        for marker in site.tmux {
            let created = body.find(marker).unwrap_or_else(|| {
                panic!(
                    "{}: `{marker}` is gone — update this test to follow it",
                    site.what
                )
            });
            assert!(
                created < forget,
                "{}: record_tmux_created must come AFTER `{marker}`, so a create that \
                 fails never forgets a real kill",
                site.what
            );
        }
    }
}

#[test]
fn item_source_stops_at_the_function_it_was_asked_for() {
    let src = "fn a() {\n    if x {\n        mine();\n    }\n}\n\nfn b() {\n    neighbour();\n}\n";
    let a = item_source(src, "fn a(");
    assert!(a.contains("mine()"), "the whole body: {a:?}");
    assert!(
        !a.contains("neighbour()"),
        "a neighbour's call must not satisfy an assertion about this function: {a:?}"
    );
    assert!(
        a.contains("if x {\n        mine();\n    }\n"),
        "a nested closing brace must not end the item: {a:?}"
    );
    let b = item_source(src, "fn b(");
    assert!(b.contains("neighbour()") && !b.contains("mine()"), "{b:?}");
}

/// The row `new_session` hands back is the row as of its LAST write — not a
/// snapshot from before `set_started_at` / `set_friendly_name` /
/// `set_claude_session_id` that the frontend would then merge over fresher
/// state (its guard is `row_version`, and a stale snapshot has the lower one).
#[test]
fn finalize_new_session_returns_the_row_as_of_its_last_write() {
    let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
    let s = crate::store::Store::open_with_bus_in_memory(bus.clone()).unwrap();
    s.upsert_host("local").unwrap();
    let id = s
        .upsert_session("f4final", "local", None, None, 1, 1, "running", None)
        .unwrap();
    let before = s.get_session_by_id(id).unwrap().unwrap();
    let row = finalize_new_session(
        &s,
        id,
        "local",
        "f4final",
        Some("Nice Name"),
        Some("uuid-9"),
        false,
    )
    .unwrap();
    assert!(
        row.started_at.is_some(),
        "started_at is on the returned row"
    );
    assert_eq!(row.friendly_name.as_deref(), Some("Nice Name"));
    assert_eq!(row.claude_session_id.as_deref(), Some("uuid-9"));
    assert!(
        row.row_version > before.row_version,
        "the returned row is the fresh one"
    );
    let latest = s.get_session_by_id(id).unwrap().unwrap();
    assert_eq!(row, latest, "returned == stored");
}

#[test]
fn finalize_new_session_tags_a_shell_session() {
    let s = crate::store::Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    let id = s
        .upsert_session("f4shell", "local", None, None, 1, 1, "running", None)
        .unwrap();
    let row = finalize_new_session(&s, id, "local", "f4shell", None, None, true).unwrap();
    assert_eq!(row.kind, "shell");
    assert!(row.started_at.is_some());
}

/// A cancelled or killed clone must not leave a half `.git` that the
/// `[ ! -d root/.git ]` guard then treats as "already cloned" forever: the
/// clone lands in a sibling temp dir and is moved into place only when it
/// finished.
#[test]
fn ensure_script_clones_into_a_temp_dir_and_moves_it_into_place() {
    let script = ensure_remote_project_script(
        "/home/u/projects/github.com/o/r",
        "git@github.com:o/r.git",
        None,
    );
    assert!(
        script.contains("tmp=\"$(dirname -- '/home/u/projects/github.com/o/r')/.fleet-clone-$$\""),
        "{script}"
    );
    assert!(
        script.contains(
            "git clone 'git@github.com:o/r.git' \"$tmp\" && { [ ! -e '/home/u/projects/github.com/o/r' ] || rmdir '/home/u/projects/github.com/o/r'; } && mv \"$tmp\" '/home/u/projects/github.com/o/r'"
        ),
        "{script}"
    );
    assert!(
        script.contains("|| { rm -rf \"$tmp\"; exit 1; }"),
        "{script}"
    );
    assert!(
        script.contains("rm -rf \"$tmp\";"),
        "a stale temp dir from an earlier attempt is cleared first: {script}"
    );
}

/// The move must land AS the root, not inside it: `mv tmp root` onto an
/// existing directory moves `tmp` into it, so a root left behind empty (a
/// failed earlier attempt, a hand-made directory) would leave no `.git` at
/// the root and every later ensure would clone again. Runs the real script.
#[test]
fn ensure_script_clones_onto_an_existing_empty_root() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let git = |args: &[&str], dir: &std::path::Path| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    std::fs::create_dir(&src).unwrap();
    git(&["init", "-q"], &src);
    git(
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "i",
        ],
        &src,
    );
    let root = tmp.path().join("projects").join("r");
    std::fs::create_dir_all(&root).unwrap();
    let script = ensure_remote_project_script(root.to_str().unwrap(), src.to_str().unwrap(), None);
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(&script)
        .output()
        .unwrap();
    assert!(out.status.success(), "{script}\n{out:?}");
    assert!(root.join(".git").is_dir(), "cloned AS the root: {script}");
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

/// A tmux executor for the rename race: renaming into `dev-new` lands a
/// lost session of that name in the store, as a reconcile pass running
/// beside the rename (with the store lock released) would.
struct RacingTmux {
    store: std::sync::Arc<Mutex<crate::store::Store>>,
    renames: Mutex<Vec<(String, String)>>,
}

#[async_trait::async_trait]
impl crate::tmux::TmuxExec for RacingTmux {
    async fn list_sessions(&self) -> Result<Vec<crate::tmux::TmuxSession>, IpcError> {
        Ok(Vec::new())
    }
    async fn new_session(&self, _: &str, _: &std::path::Path, _: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn kill_session(&self, _: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn rename_session(&self, old: &str, new: &str) -> Result<(), IpcError> {
        self.renames.lock().unwrap().push((old.into(), new.into()));
        if new == "dev-new" {
            lost_row(&self.store.lock().unwrap(), "dev-new", Some(RESUME_ID));
        }
        Ok(())
    }
    async fn restart_session(&self, _: &str, _: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn capture_pane(&self, _: &str) -> Result<String, IpcError> {
        Ok(String::new())
    }
    async fn capture_pane_scrollback(&self, _: &str, _: u32) -> Result<String, IpcError> {
        Ok(String::new())
    }
    async fn list_claude_agents(&self) -> Option<Vec<crate::claude_agents::ClaudeAgentRow>> {
        None
    }
}

/// The lost-name guard runs twice: under the lock before tmux renames, and
/// again inside `Store::rename_session_row`. A lost row named `new` that
/// lands between the two is refused by the second — rightly — but by then
/// tmux already answers to `new`: the pane is renamed back before the
/// refusal is reported, so tmux and the row agree, and the lost row is
/// left as it was.
#[tokio::test]
async fn a_lost_name_that_lands_during_the_tmux_rename_is_refused_and_renamed_back() {
    let store = std::sync::Arc::new(Mutex::new(crate::store::Store::open_in_memory().unwrap()));
    {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev-old", "local", None, None, 1, 1, "running", None)
            .unwrap();
    }
    let tmux = RacingTmux {
        store: std::sync::Arc::clone(&store),
        renames: Mutex::new(Vec::new()),
    };
    let ssh = Arc::new(crate::ssh::SshClient::new());
    let err = rename_session_with(
        RenameSessionArgs {
            host_alias: "local".into(),
            old_name: "dev-old".into(),
            new_name: "dev-new".into(),
        },
        &store,
        &ssh,
        &tmux,
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_EXISTS);
    assert!(
        err.message.starts_with("dev-new belongs to a lost session"),
        "{}",
        err.message
    );
    assert_eq!(
        *tmux.renames.lock().unwrap(),
        vec![
            ("dev-old".to_string(), "dev-new".to_string()),
            ("dev-new".to_string(), "dev-old".to_string()),
        ],
        "renamed, refused, renamed back"
    );
    let s = store.lock().unwrap();
    assert!(
        s.get_session("dev-old", "local").unwrap().is_some(),
        "the row keeps its name"
    );
    assert!(
        s.lost_resumable_session_named("local", "dev-new")
            .unwrap()
            .is_some(),
        "the lost row is untouched"
    );
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
    assert_eq!(
        pane,
        recreate_pane_command("work", Some(RESUME_ID), "dev-z")
    );
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

/// Work graph M10.2 (found by the hub e2e): a session started in a worktree
/// is linked to that worktree's row, which reconcile never does — tidy-up,
/// safe kill and the idle killer inspect a work session's tree only through
/// it. A new worktree gets its row (for the session's host); a system
/// project, or a start in the main checkout (by path or by its `main` row),
/// links nothing.
#[test]
fn a_new_session_is_linked_to_the_worktree_it_was_started_in() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    s.upsert_host("h").unwrap();
    let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
    let row = |name: &str, host: &str| {
        s.upsert_session(name, host, Some(pid), None, 0, 0, "running", None)
            .unwrap()
    };
    let args = |host: &str, worktree_id: Option<i64>, new_worktree: Option<&str>| NewSessionArgs {
        host_alias: host.into(),
        project_id: pid,
        worktree_id,
        name: "n".into(),
        call_id: None,
        new_worktree: new_worktree.map(str::to_string),
        base_branch: None,
        kind: None,
        start_command: None,
        friendly_name: None,
        resume_claude_session_id: None,
    };

    // A new worktree on local: its row is created and linked.
    let a = row("a", "local");
    let wid = link_new_session_worktree(
        &s,
        a,
        &args("local", None, Some("abc-1-fix")),
        "/p/o/r/.worktrees/abc-1-fix",
        false,
    )
    .unwrap()
    .unwrap();
    let w = s.get_worktree_row(wid).unwrap().unwrap();
    assert_eq!(
        (w.name.as_str(), w.path.as_str(), w.host_alias.as_str()),
        ("abc-1-fix", "/p/o/r/.worktrees/abc-1-fix", "local")
    );
    assert_eq!(w.branch.as_deref(), Some("abc-1-fix"));
    assert_eq!(
        s.get_session_by_id(a).unwrap().unwrap().worktree_id,
        Some(wid)
    );

    // On a remote host: that host's row, never the local one of that name.
    let b = row("b", "h");
    let rid = link_new_session_worktree(
        &s,
        b,
        &args("h", None, Some("abc-1-fix")),
        "/home/u/projects/github.com/o/r/.worktrees/abc-1-fix",
        false,
    )
    .unwrap()
    .unwrap();
    assert_ne!(rid, wid);
    assert_eq!(s.get_worktree_row(rid).unwrap().unwrap().host_alias, "h");

    // An existing worktree: that one.
    let c = row("c", "local");
    assert_eq!(
        link_new_session_worktree(&s, c, &args("local", Some(wid), None), "/x", false).unwrap(),
        Some(wid)
    );
    assert_eq!(
        s.get_session_by_id(c).unwrap().unwrap().worktree_id,
        Some(wid)
    );

    // The main checkout, or a system project: nothing — not even when the
    // start names the `main` row (discover hands one over for a remote
    // project root): safe kill and discard `git worktree remove` a linked
    // tree, and the clone itself is not one.
    let d = row("d", "local");
    assert_eq!(
        link_new_session_worktree(&s, d, &args("local", None, None), "/p/o/r", false).unwrap(),
        None
    );
    let mid = s
        .upsert_worktree(pid, "main", "/p/o/r", Some("main"))
        .unwrap();
    assert_eq!(
        link_new_session_worktree(&s, d, &args("local", Some(mid), None), "/p/o/r", false).unwrap(),
        None
    );
    assert_eq!(
        link_new_session_worktree(&s, d, &args("local", None, Some("x")), "/sys", true).unwrap(),
        None
    );
    assert_eq!(s.get_session_by_id(d).unwrap().unwrap().worktree_id, None);
}
