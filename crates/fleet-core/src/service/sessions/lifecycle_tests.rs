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
            what: "rename_session",
            source: LIFECYCLE,
            signature: "pub async fn rename_session(",
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
