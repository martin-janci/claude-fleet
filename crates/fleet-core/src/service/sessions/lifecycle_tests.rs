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
        model: None,
        effort: None,
        profile: None,
        agent: None,
        origin: None,
        over_limit_ok: false,
        owner_person_id: None,
        start_token: None,
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
        model: None,
        effort: None,
        profile: None,
        agent: None,
        origin: None,
        over_limit_ok: false,
        owner_person_id: None,
        start_token: None,
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

#[test]
fn a_kill_cancels_the_sessions_pending_form() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    let id = s
        .upsert_session("k", "local", None, None, 0, 0, "running", None)
        .unwrap();
    let spec = serde_json::json!({ "spec": "fleet.form/1", "title": "T", "steps": [
        { "title": "S", "fields": [{ "name": "a", "type": "text", "label": "A" }] }] });
    let st = std::sync::Mutex::new(s);
    let form = crate::service::forms::open(&st, id, &spec, None).unwrap();
    record_kill(&st.lock().unwrap(), id, None);
    assert_eq!(
        crate::service::forms::get(&st, &form.form_id)
            .unwrap()
            .state,
        "cancelled"
    );
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
    /// Every tmux call in the body that leaves the name live. The
    /// `record_tmux_created` call must sit AFTER all of them.
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
                "restart_unrepaired(&*tmux, &args.name, &pane_cmd",
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
            // ONE rule for every site, `new_session_inner` included again
            // (T5's review): the forget comes after the create, so a create
            // that fails never forgets a real kill. The exemption that used
            // to invert this site existed only for the owner reservation,
            // which is deleted.
            assert!(
                created < forget,
                "{}: record_tmux_created must come AFTER `{marker}`, so a create that \
                 fails never forgets a real kill",
                site.what
            );
        }
    }
}

/// **An ordinary create still gets its owner** (multi-user M1, T5's review).
///
/// With the name-keyed intent deleted, the reconcile upsert inserts
/// `unclaimed` and `finalize_new_session` is the ONE thing that stamps an
/// owner — so this is the whole of the create's ownership, driven directly.
/// `new_session_inner` cannot be: it resolves its executor through
/// `exec_for`, which is not injectable.
///
/// Three cases, the three ways the claim can be reached:
///
/// 1. the ordinary one — the row the pass inserted is nobody's, and the
///    create's caller becomes its owner, `private`;
/// 2. a caller that is nobody (a per-host token, a pre-M1 device) leaves it
///    `unclaimed`, which is the answer and not a failure;
/// 3. a SECOND create finalising against the same row is refused
///    (`E_FORBIDDEN`) rather than taking it — the window between the insert
///    and the claim is lost LOUDLY, which is the half that makes it
///    acceptable (see the note at the call site).
#[test]
fn finalize_new_session_claims_the_row_for_its_caller_and_never_takes_anothers() {
    let s = crate::store::Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    let ann = s.create_person("ann", None).unwrap().id;
    let bob = s.create_person("bob", None).unwrap().id;
    let finalize = |owner: Option<i64>, name: &str, id: i64| {
        finalize_new_session(
            &s,
            id,
            "local",
            name,
            None,
            None,
            false,
            owner,
            &crate::store::SessionOrigin::person(owner),
        )
    };

    // 1. The ordinary create.
    let mine = s
        .upsert_session("dev-mine", "local", None, None, 1, 1, "running", None)
        .unwrap();
    let row = finalize(Some(ann), "dev-mine", mine).expect("the ordinary create");
    assert_eq!(row.owner_person_id, Some(ann));
    assert_eq!(row.visibility, crate::store::VISIBILITY_PRIVATE);

    // 2. A caller that is nobody.
    let nobodys = s
        .upsert_session("dev-nobodys", "local", None, None, 1, 1, "running", None)
        .unwrap();
    let row = finalize(None, "dev-nobodys", nobodys).expect("still a live session");
    assert_eq!(row.owner_person_id, None);
    assert_eq!(row.visibility, crate::store::VISIBILITY_UNCLAIMED);

    // 3. A second create against ann's row.
    let err = finalize(Some(bob), "dev-mine", mine).expect_err("never another person's row");
    assert_eq!(err.code, crate::ipc_error::codes::E_FORBIDDEN);
    let after = s.get_session_by_id(mine).unwrap().unwrap();
    assert_eq!(
        after.owner_person_id,
        Some(ann),
        "the row never changed hands"
    );
    assert_eq!(after.visibility, crate::store::VISIBILITY_PRIVATE);
}

/// `reject_adoptable_session_name` refuses EVERY pre-existing row under the
/// name, not only the lost ones `reject_lost_session_name` knows about (T5's
/// review). The row this create would otherwise adopt is the one the by-name
/// lookup after the reconcile returns, and `finalize_new_session` would stamp
/// the caller as its owner — inheriting a stranger's timeline, conversations
/// and work links along with it.
///
/// The three states are the three ways a row survives the create: running
/// (the upsert's `DO UPDATE` branch), ghost and lost (the same branch, which
/// also REVIVES the row), with the lost one carrying no `claude_session_id`
/// so `reject_lost_session_name` lets it through.
#[test]
fn reject_adoptable_session_name_refuses_every_pre_existing_row() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("alpha").unwrap();
    // A free name is not refused — the ordinary create, and the control that
    // keeps this guard from passing by refusing everything.
    assert!(reject_adoptable_session_name(&s, "alpha", "fresh").is_ok());

    let id = s
        .upsert_session("taken", "alpha", None, None, 1, 1, "running", None)
        .unwrap();
    for state in ["running", "ghost"] {
        s.conn_ref()
            .execute(
                "UPDATE sessions SET status=?1, lost_at=CASE ?1 WHEN 'ghost' THEN 7 END \
                 WHERE id=?2",
                rusqlite::params![state, id],
            )
            .unwrap();
        let err = reject_adoptable_session_name(&s, "alpha", "taken")
            .expect_err("a row under the name must be refused, whatever its status");
        assert_eq!(err.code, crate::ipc_error::codes::E_EXISTS, "{state}");
        // T10: neither the row id nor its owner is in the message.
        assert!(err.message.contains("taken"), "{}", err.message);
        assert!(
            !err.message.contains(&id.to_string()),
            "the row id must not be disclosed: {}",
            err.message
        );
    }
    // Another host's row of the same name is not this host's business.
    s.upsert_host("beta").unwrap();
    assert!(reject_adoptable_session_name(&s, "beta", "taken").is_ok());
}

/// **No create path may state an owner by NAME again** (multi-user M1, T5's
/// review). The deleted mechanism was a `(host_alias, tmux_name)` → person
/// map filed before the tmux session existed and read by whichever reconcile
/// pass inserted the row; three attempts to guard it each moved its hole
/// instead of closing it, because a tmux name is reused.
///
/// Pinned in the source because the hazard is a SHAPE, not a value: the only
/// honest test of a resurrected reservation would need a live tmux server
/// (`exec_for` is not injectable), and by the time one existed the hole would
/// be shipped. So this fails on the names themselves, in the two files where
/// such a mechanism would have to appear.
#[test]
fn no_create_path_reserves_an_owner_for_a_tmux_name() {
    const LIFECYCLE: &str = include_str!("lifecycle.rs");
    const REVIEW: &str = include_str!("review.rs");
    const RECONCILE: &str = include_str!("../../store/reconcile.rs");
    const WHY: &str = "a tmux name is not an identity: it is reused, anyone with host access \
                       can create one, and the map has one slot per name — so an intent filed \
                       against a name claims whatever row turns up under it. Own the row by \
                       id, through `claim_if_unclaimed`.";
    for (what, source) in [
        ("lifecycle.rs", LIFECYCLE),
        ("review.rs", REVIEW),
        ("store/reconcile.rs", RECONCILE),
    ] {
        for banned in [
            "reserve_session_owner",
            "release_session_owner",
            "release_tmux_owner",
            "OwnerIntent",
        ] {
            // The doc comments that record the removal name the dead symbols
            // on purpose; a real reintroduction is a call or a definition, so
            // match the shapes those take and not the bare word.
            for shape in [
                format!("{banned}("),
                format!("fn {banned}"),
                format!("struct {banned}"),
            ] {
                assert!(
                    !source.contains(&shape),
                    "{what} contains `{shape}` — {WHY}"
                );
            }
        }
    }
    // The upsert must not write either ownership column on its `DO UPDATE`
    // branch; the whole mechanism rode that one SET clause.
    for banned in ["owner_person_id=", "visibility="] {
        assert!(
            !RECONCILE.contains(banned),
            "store/reconcile.rs sets `{banned}` — a reconcile pass must never write \
             ownership at all: {WHY}"
        );
    }
}

/// `new_session` refuses a name that already has a row rather than ADOPTING
/// that row (T5's review). The refusal itself is unit-tested in
/// `reject_adoptable_session_name_refuses_every_pre_existing_row`; what is
/// pinned here is that `new_session` still calls it, and calls it before the
/// tmux create — the lookup that follows the create finds "its" row by NAME,
/// so a row that was already there is adopted, with its history, by
/// `finalize_new_session`'s claim.
#[test]
fn new_session_refuses_a_name_that_already_has_a_row() {
    const LIFECYCLE: &str = include_str!("lifecycle.rs");
    let body = item_source(LIFECYCLE, "pub async fn new_session(");
    assert!(
        body.contains("reject_adoptable_session_name("),
        "new_session must refuse a name that already has a row: the by-name lookup after \
         the create cannot tell the row it caused from one that was already there, and the \
         claim would inherit that row's timeline, conversations and work links"
    );
    let refuse = body.find("reject_adoptable_session_name(").unwrap();
    let inner = body
        .find("new_session_inner(")
        .expect("new_session no longer calls new_session_inner");
    assert!(
        refuse < inner,
        "the refusal has to come before anything touches tmux"
    );
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
        None,
        &crate::store::SessionOrigin::mission(7),
    )
    .unwrap();
    assert_eq!(
        (row.origin.as_deref(), row.origin_ref.as_deref()),
        (Some("mission"), Some("7")),
        "the origin is on the returned row"
    );
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
    let row = finalize_new_session(
        &s,
        id,
        "local",
        "f4shell",
        None,
        None,
        true,
        None,
        &crate::store::SessionOrigin::token(None),
    )
    .unwrap();
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
        model: None,
        effort: None,
        profile: None,
        agent: None,
        origin: None,
        over_limit_ok: false,
        owner_person_id: None,
        start_token: None,
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
    // Multi-user M1 (T10): no row id in the message — the lost row may be
    // another person's, and `dev-x` is the caller's own argument.
    assert_eq!(
        err.message,
        "dev-x belongs to a lost session; restore it with restore_host_sessions or dismiss it first"
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

/// A lost row with no conversation has nothing to RESUME, so
/// `reject_lost_session_name` says nothing about it — and the create is
/// refused all the same, by [`reject_adoptable_session_name`] (multi-user M1,
/// T5's review).
///
/// **This used to read "so the name is free", and that was the adoption
/// hole.** `new_session` inserts no row: it starts tmux, reconciles, and
/// finds "its" row by NAME. The reconcile upsert's `ON CONFLICT DO UPDATE`
/// revives this very row — same id, same `session_events` timeline, same
/// conversations and work links — and `finalize_new_session` would then stamp
/// the caller as its owner. So the create fails instead, before any tmux
/// call, and the two refusals are told apart by their messages.
#[tokio::test]
async fn a_lost_session_without_a_conversation_is_refused_rather_than_adopted() {
    let store = Mutex::new(crate::store::Store::open_in_memory().unwrap());
    lost_row(&store.lock().unwrap(), "dev-x", None);
    let ssh = Arc::new(crate::ssh::SshClient::new());
    let reg = crate::cancel::CancellationRegistry::new();
    let err = new_session(args_named("dev-x", None), &store, &ssh, &reg)
        .await
        .unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_EXISTS);
    assert!(
        err.message.contains("already names a session"),
        "the generic adoption guard, not the resumable one: {}",
        err.message
    );
    // A name with no row at all still gets past both guards and fails later,
    // on the unknown project `args_named` uses — the control that keeps this
    // from passing by refusing everything.
    let err = new_session(args_named("dev-free", None), &store, &ssh, &reg)
        .await
        .unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_NOTFOUND);
}

/// Multi-user M1 (T5), DoD 7: a lost row with **no** `claude_session_id` is
/// not refused by the resumable guard above, and reconcile's
/// `ON CONFLICT DO UPDATE` revives it with its `owner_person_id` intact. So
/// person B starting a session under a name person A once used would come up
/// owned by — and readable only to — A. The name is refused instead.
///
/// Four cases in one test, because the rule is the whole table and not the
/// first row of it.
#[tokio::test]
async fn a_lost_row_owned_by_another_person_blocks_its_name_even_with_no_conversation() {
    let store = Mutex::new(crate::store::Store::open_in_memory().unwrap());
    let (ann, bob) = {
        let s = store.lock().unwrap();
        let ann = s.create_person("ann", None).unwrap().id;
        let bob = s.create_person("bob", None).unwrap().id;
        // Ann's shell session on `local`, killed: lost, and NO conversation —
        // so `lost_resumable_session_named` says nothing about it.
        let id = lost_row(&s, "dev-x", None);
        s.conn_ref()
            .execute(
                "UPDATE sessions SET owner_person_id = ?1, visibility = 'private' WHERE id = ?2",
                rusqlite::params![ann, id],
            )
            .unwrap();
        (ann, bob)
    };
    let ssh = Arc::new(crate::ssh::SshClient::new());
    let reg = crate::cancel::CancellationRegistry::new();

    // Bob: refused, and the message names neither ann nor the row id.
    let mut args = args_named("dev-x", None);
    args.owner_person_id = Some(bob);
    let err = new_session(args, &store, &ssh, &reg).await.unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_EXISTS);
    assert!(
        err.message.contains("another person's lost session"),
        "{}",
        err.message
    );
    assert!(!err.message.contains("ann"), "{}", err.message);

    // A caller that is nobody (a per-host token, a pre-M1 device) is refused
    // too: `None` must not read as "the owner".
    let err = new_session(args_named("dev-x", None), &store, &ssh, &reg)
        .await
        .unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_EXISTS);
    assert!(
        err.message.contains("another person's lost session"),
        "{}",
        err.message
    );

    // Ann herself gets past THIS guard — it is HER lost row — and is then
    // refused by the generic one, which adopts nobody's row either
    // (T5's review). The two are told apart by the message: hers does not say
    // "another person's".
    let mut mine = args_named("dev-x", None);
    mine.owner_person_id = Some(ann);
    let err = new_session(mine, &store, &ssh, &reg).await.unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_EXISTS);
    assert!(
        err.message.contains("already names a session"),
        "the owner passes the owned-row guard and meets the generic one: {}",
        err.message
    );

    // And an UNOWNED lost row is refused the same way. It used to block
    // nobody, on the reasoning that the row it revives is `unclaimed` and so
    // belongs to no one — true of the OWNER column and beside the point: the
    // revived row is a stranger's abandoned history, and the create would
    // hand it back as the session it just made, claimed.
    {
        let s = store.lock().unwrap();
        lost_row(&s, "dev-nobodys", None);
    }
    let mut unowned = args_named("dev-nobodys", None);
    unowned.owner_person_id = Some(bob);
    let err = new_session(unowned, &store, &ssh, &reg).await.unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_EXISTS);
    assert!(
        err.message.contains("already names a session"),
        "{}",
        err.message
    );
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
        recreate_pane_command(
            "work",
            "claude",
            Some(RESUME_ID),
            "dev-z",
            &Default::default()
        )
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
fn model_and_effort_ride_on_every_launch_in_the_chain() {
    let mut args = args_named("dev-z", None);
    args.model = Some(" opus[1m] ".into());
    args.effort = Some("xhigh".into());
    normalize_launch(&mut args).unwrap();
    assert_eq!(args.model.as_deref(), Some("opus[1m]"));
    let (_, pane) = claude_id_and_pane_cmd(&args);
    assert_eq!(
        pane.matches("--model 'opus[1m]' --effort 'xhigh'").count(),
        3,
        "got: {pane}"
    );
    // Without either, the command is the plain one a recreate would run.
    let (cid, pane) = claude_id_and_pane_cmd(&args_named("dev-z", Some(RESUME_ID)));
    assert_eq!(
        pane,
        recreate_pane_command(
            "work",
            "claude",
            cid.as_deref(),
            "dev-z",
            &Default::default()
        )
    );
}

#[test]
fn launch_options_are_validated_and_blank_means_default() {
    let mut args = args_named("dev-z", None);
    args.model = Some("  ".into());
    args.effort = Some(String::new());
    normalize_launch(&mut args).unwrap();
    assert_eq!((args.model, args.effort), (None, None));
    for (model, effort) in [
        (Some("--dangerously"), None),
        (Some("opus; rm -rf ~"), None),
        (None, Some("huge")),
    ] {
        let mut args = args_named("dev-z", None);
        args.model = model.map(str::to_string);
        args.effort = effort.map(str::to_string);
        let err = normalize_launch(&mut args).unwrap_err();
        assert_eq!(
            err.code,
            crate::ipc_error::codes::E_INVALID,
            "{model:?} {effort:?}"
        );
    }
    let mut args = args_named("dev-z", None);
    args.kind = Some("shell".into());
    args.model = Some("opus".into());
    assert_eq!(
        normalize_launch(&mut args).unwrap_err().code,
        crate::ipc_error::codes::E_INVALID
    );
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
    // Multi-user M1 (T10): the holder's id and tmux name are not disclosed
    // — a tmux name in this fleet is a branch or a ticket key.
    assert_eq!(
        err.message,
        format!(
            "conversation {RESUME_ID} is already held by a session on local; restore it with restore_host_sessions instead"
        )
    );
    // The holder's tmux name, spelled out: an `id.to_string()` check would
    // be meaningless here, since a one-digit rowid is a substring of the
    // conversation uuid the message legitimately carries.
    assert!(
        !err.message.contains("dev-x"),
        "the holder's name is not disclosed: {}",
        err.message
    );
    let _ = id;
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
        model: None,
        effort: None,
        profile: None,
        agent: None,
        origin: None,
        over_limit_ok: false,
        owner_person_id: None,
        start_token: None,
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

// ── Multi-user M1 (T5): every create path says whose the row is ─────────────
//
// `spawn_review`, `move_session` and `repair_session` resolve their executors
// through `exec_for` / `HostExec`, which are not injectable, so driving them
// for real needs a live tmux server (and macOS CI has none) — the same reason
// `every_path_that_creates_a_tmux_session_forgets_the_kill_first` reads the
// source instead of the behaviour. These do the same for ownership: the
// behaviour of the two primitives they lean on is pinned where those live
// (`store::sessions::claim_if_unclaimed`'s tests, `store::reconcile`'s upsert
// tests, `bg_sessions::stamp_bg_row_claims_the_row_and_a_resurrected_one_keeps_its_first_owner`),
// and what is pinned here is that each path calls them, with the right owner,
// and hard where the plan says hard.

/// `spawn_review` inherits the SOURCE row's owner, never the caller's (spec
/// §4.3 invariant 6): a review runs Claude in the owner's worktree, on the
/// owner's host, so a review owned by whoever asked for it would hand a
/// watcher an owned session inside somebody else's checkout.
#[test]
fn spawn_review_claims_the_review_row_for_the_sources_owner() {
    const REVIEW: &str = include_str!("review.rs");
    let body = item_source(REVIEW, "pub async fn spawn_review(");
    assert!(
        body.contains("s.claim_if_unclaimed(row.id, source.owner_person_id)?"),
        "spawn_review must claim the review row for the SOURCE row's owner, \
         and hard (`?`): a review of a private session that lands `unclaimed` \
         is a row its owner cannot read and somebody else can claim"
    );
    // The caller is deliberately not consulted: `SpawnReviewArgs` carries no
    // person, and a grep for one here is how that stays true.
    assert!(
        !body.contains("owner_for(") && !body.contains("personal_owner_id("),
        "a review's owner is the source row's, never the caller's or the hub's"
    );
}

/// `move_session` carries the owner to the target row as a HARD failure, and
/// drops the source row's grants (spec §4.3, the owner's decision of
/// 2026-09-30). Both are in the block whose every other write is commented
/// "Soft-fail like new_session: the session is live either way" — which is
/// exactly what these two must not be.
#[test]
fn move_session_carries_the_owner_hard_and_drops_the_grants() {
    const MOVE: &str = include_str!("../move_session/mod.rs");
    let body = item_source(MOVE, "async fn move_session_inner(");
    let carry = body
        .find("claim_if_unclaimed(row.id, snap.row.owner_person_id)")
        .expect(
            "move_session must carry the source row's owner onto the target row — \
             a soft-failed carry leaves the target `unclaimed` on the host the \
             caller named, which makes a move a silent privacy event",
        );
    let revoke = body
        .find("revoke_all_grants_on_session(snap.row.id)")
        .expect(
            "a move must revoke the SOURCE row's grants: they are keyed on the \
             old `sessions.id`, the target is a new row, and narrowing is the \
             safe direction",
        );
    // Both are `?`-propagated through `partial(...)`, which is what makes them
    // hard: the move aborts (as a partial move — the target is live) instead of
    // returning a row that is nobody's or a share nobody expects. Read as "the
    // statement that starts here ends in `?;` and mentions `partial(`" rather
    // than as an exact spelling, so rustfmt is free to lay it out as it likes.
    for (what, at) in [("the owner carry", carry), ("the grant revoke", revoke)] {
        let tail = &body[at..];
        let end = tail
            .find("?;")
            .map(|e| e + 2)
            .unwrap_or_else(|| panic!("{what} does not end in `?;`, so it is not a hard failure"));
        assert!(
            tail[..end].contains("partial("),
            "{what} must fail the move through `partial(...)`, not be a \
             soft-failed write: {}",
            &tail[..end]
        );
        assert!(
            !tail[..end].contains("tracing::warn!"),
            "{what} must not be logged-and-ignored: {}",
            &tail[..end]
        );
    }
}

/// A repair cannot produce an ownerless row because it produces no row at all:
/// `repair_session` is addressed BY an existing session and only ever rebuilds
/// its workspace and its pane. The row — and so its owner and visibility —
/// survives untouched.
///
/// Asserted as the absence of a session INSERT rather than by driving the
/// repair, because the claim being made is structural: if repair ever starts
/// creating rows it becomes a create path, and a create path with no owner
/// seam is the bug this test exists to catch.
#[test]
fn repair_never_creates_a_session_row_and_so_never_an_ownerless_one() {
    const REPAIR: &str = include_str!("../repair.rs");
    // Production code only — the file's own test module seeds rows with
    // `upsert_session`, which is a `#[cfg(test)]` helper and not a create path
    // (`store/session_grants.rs`'s surface test slices the same way).
    // (Cut at the test MODULE: `#[cfg(test)]` also marks two test-only pure
    // helpers earlier in the file, so the first match is far too early.)
    let production = &REPAIR[..REPAIR
        .find("#[cfg(test)]\nmod tests {")
        .expect("repair.rs has a test module")];
    // Comments out, so prose quoting SQL does not count.
    let code = production
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in [
        "INSERT INTO sessions",
        "upsert_session(",
        "apply_host_reconcile",
        "reconcile_one_host",
    ] {
        assert!(
            !code.contains(forbidden),
            "service/repair.rs must not create session rows ({forbidden:?}): a \
             repair keeps the row it was asked about, which is what makes the \
             owner survive it. If this changes, repair needs an owner seam of \
             its own (multi-user M1, T5)"
        );
    }
}

// ── restart after a host reboot, and the operator's controller exemption ──
// (2026-10-06: the agent panel's `lost` button on `fleet-operator@mefistos`
// failed twice over — `E_SELF_TARGET`, then `E_TMUX` from a respawn on a host
// whose reboot took the tmux server with it.)

/// Records which of `restart_session`'s two tmux calls ran.
struct RebootedTmux {
    live: Vec<String>,
    calls: Mutex<Vec<String>>,
}

impl RebootedTmux {
    fn listing(live: &[&str]) -> Self {
        Self {
            live: live.iter().map(|n| n.to_string()).collect(),
            calls: Mutex::new(Vec::new()),
        }
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl crate::tmux::TmuxExec for RebootedTmux {
    async fn list_sessions(&self) -> Result<Vec<crate::tmux::TmuxSession>, IpcError> {
        Ok(self
            .live
            .iter()
            .map(|name| crate::tmux::TmuxSession {
                name: name.clone(),
                created: 1,
                last_activity: 1,
                attached: false,
                path: std::path::PathBuf::from("/"),
                pane_id: None,
            })
            .collect())
    }
    async fn new_session(
        &self,
        name: &str,
        cwd: &std::path::Path,
        cmd: &str,
    ) -> Result<(), IpcError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("new {name} in {} running {cmd}", cwd.display()));
        Ok(())
    }
    async fn kill_session(&self, _: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn rename_session(&self, _: &str, _: &str) -> Result<(), IpcError> {
        Ok(())
    }
    async fn restart_session(&self, name: &str, cmd: &str) -> Result<(), IpcError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("respawn {name} running {cmd}"));
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

/// The host rebooted: tmux lists nothing (`no server running` parses as an
/// empty list), so a respawn would fail. The session is created instead, in
/// the directory the caller resolves, with the same pane command.
#[tokio::test]
async fn a_restart_with_no_tmux_session_creates_one_instead_of_respawning() {
    let tmux = RebootedTmux::listing(&[]);
    super::lifecycle::restart_unrepaired(&tmux, "fleet-operator", "cl --resume x", || async {
        Ok("/home/u/.claude-fleet/operator".to_string())
    })
    .await
    .unwrap();
    assert_eq!(
        tmux.calls(),
        vec!["new fleet-operator in /home/u/.claude-fleet/operator running cl --resume x"]
    );
}

/// A tmux session that is still there keeps the plain respawn, and the cwd
/// (which may cost an SSH round trip) is never resolved.
#[tokio::test]
async fn a_restart_of_a_live_tmux_session_still_respawns_it() {
    let tmux = RebootedTmux::listing(&["other", "fleet-operator"]);
    super::lifecycle::restart_unrepaired(&tmux, "fleet-operator", "cl --resume x", || async {
        panic!("the cwd is only resolved for a session that has to be created")
    })
    .await
    .unwrap();
    assert_eq!(
        tmux.calls(),
        vec!["respawn fleet-operator running cl --resume x"]
    );
}

fn record_operator(s: &crate::store::Store, host: &str) {
    crate::service::operator::set_operator_ref(
        s,
        &crate::service::operator::OperatorRef {
            host_alias: host.into(),
            tmux_name: crate::service::operator::OPERATOR_TMUX_NAME.into(),
        },
    )
    .unwrap();
}

/// Which directory a gone session is re-created in: a system project's own
/// path, else the operator's directory for the recorded operator (the live
/// row had lost its project), else the host's home.
#[test]
fn a_gone_pane_is_recreated_in_its_system_dir_the_operator_dir_or_home() {
    use super::lifecycle::{gone_pane_cwd, GonePaneCwd};
    let s = crate::store::Store::open_in_memory().unwrap();
    s.upsert_host("mefistos").unwrap();
    record_operator(&s, "mefistos");
    let op = crate::service::operator::OPERATOR_TMUX_NAME;
    let pid = s
        .upsert_system_project("fleet", "operator", "/home/u/.claude-fleet/operator")
        .unwrap();

    s.upsert_session(op, "mefistos", None, None, 1, 1, "running", None)
        .unwrap();
    let row = s.get_session(op, "mefistos").unwrap();
    assert_eq!(
        gone_pane_cwd(&s, row.as_ref(), "mefistos", op).unwrap(),
        GonePaneCwd::Operator,
        "the project-less operator row (as found on mefistos) goes back to its own dir"
    );

    s.upsert_session(
        "dev-sys",
        "mefistos",
        Some(pid),
        None,
        1,
        1,
        "running",
        None,
    )
    .unwrap();
    let row = s.get_session("dev-sys", "mefistos").unwrap();
    assert_eq!(
        gone_pane_cwd(&s, row.as_ref(), "mefistos", "dev-sys").unwrap(),
        GonePaneCwd::Fixed("/home/u/.claude-fleet/operator".into())
    );

    s.upsert_session("orphan", "mefistos", None, None, 1, 1, "running", None)
        .unwrap();
    let row = s.get_session("orphan", "mefistos").unwrap();
    assert_eq!(
        gone_pane_cwd(&s, row.as_ref(), "mefistos", "orphan").unwrap(),
        GonePaneCwd::Home
    );
    assert_eq!(
        gone_pane_cwd(&s, None, "mefistos", "never-seen").unwrap(),
        GonePaneCwd::Home
    );
}

/// The operator's directory and home on a remote host are absolute: a `~`
/// handed to `tmux new-session -c` is taken as a directory named `~`.
#[tokio::test]
async fn a_gone_pane_cwd_resolves_to_an_absolute_path_on_the_host() {
    use super::lifecycle::{resolve_gone_pane_cwd, GonePaneCwd};
    let ssh = crate::ssh_fake::FakeSsh::new();
    ssh.with_home("/home/u");
    assert_eq!(
        resolve_gone_pane_cwd(GonePaneCwd::Operator, "mefistos", &ssh)
            .await
            .unwrap(),
        "/home/u/.claude-fleet/operator"
    );
    assert_eq!(
        resolve_gone_pane_cwd(GonePaneCwd::Home, "mefistos", &ssh)
            .await
            .unwrap(),
        "/home/u"
    );
    assert_eq!(
        resolve_gone_pane_cwd(GonePaneCwd::Fixed("/srv/op".into()), "mefistos", &ssh)
            .await
            .unwrap(),
        "/srv/op"
    );
}

/// The operator registered itself as the fleet controller (it reads the same
/// control skill every session does), and the panel's restart — no `force` —
/// answered `E_SELF_TARGET`. The operator is exempt; any other controller
/// still needs `force`.
#[test]
fn restart_needs_no_force_for_the_operator_even_when_it_is_the_controller() {
    use super::lifecycle::restart_guard;
    let s = crate::store::Store::open_in_memory().unwrap();
    let op = crate::service::operator::OPERATOR_TMUX_NAME;
    record_operator(&s, "mefistos");
    s.set_controller("mefistos", op).unwrap();
    restart_guard(&s, "mefistos", op, false).expect("the panel's restart must not need force");

    s.set_controller("mac", "dev-fleet").unwrap();
    let err = restart_guard(&s, "mac", "dev-fleet", false).unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_SELF_TARGET);
    restart_guard(&s, "mac", "dev-fleet", true).unwrap();
}

#[test]
fn restart_session_asks_its_guard_and_creates_a_gone_session() {
    const LIFECYCLE: &str = include_str!("lifecycle.rs");
    let body = item_source(LIFECYCLE, "pub async fn restart_session(");
    assert!(
        body.contains("restart_guard(&s, &args.host_alias, &args.name, args.force)"),
        "restart_session must refuse the controller through restart_guard"
    );
    assert!(
        body.contains("restart_unrepaired("),
        "restart_session must create a tmux session a reboot took, not only respawn"
    );
}

/// Redesign 2.1: `agent` folds into `kind`, so the rest of the create path
/// reads a shell session from `kind` alone and the row stores what it runs.
#[test]
fn the_agent_settles_against_the_kind() {
    use crate::ipc_error::codes::E_INVALID;
    let settle = |agent: Option<&str>, kind: Option<&str>| {
        let mut args = args_named("dev-z", None);
        args.agent = agent.map(str::to_string);
        args.kind = kind.map(str::to_string);
        normalize_launch(&mut args)
            .map(|()| (args.agent, args.kind))
            .ok()
    };
    let ok =
        |a: Option<&str>, k: Option<&str>| Some((a.map(str::to_string), k.map(str::to_string)));
    // No agent: the kind decides, and the row's agent follows it.
    assert_eq!(settle(None, None), ok(None, None));
    assert_eq!(settle(Some("  "), Some("work")), ok(None, Some("work")));
    assert_eq!(settle(None, Some("shell")), ok(None, Some("shell")));
    // agent shell IS a shell session.
    assert_eq!(
        settle(Some(" shell "), None),
        ok(Some("shell"), Some("shell"))
    );
    assert_eq!(
        settle(Some("shell"), Some("shell")),
        ok(Some("shell"), Some("shell"))
    );
    assert_eq!(
        settle(Some("claude"), Some("review")),
        ok(Some("claude"), Some("review"))
    );
    // Codex (12.2) starts; its row is written `codex` by `new_session`.
    assert_eq!(settle(Some("codex"), None), ok(Some("codex"), None));
    // agy (12.3) is refused until its adapter is validated.
    assert_eq!(settle(Some("agy"), None), None);
    // Contradictions and unknown names are invalid.
    for (agent, kind, code) in [
        ("shell", Some("work"), E_INVALID),
        ("claude", Some("shell"), E_INVALID),
        ("codex", Some("shell"), E_INVALID),
        ("agy", Some("shell"), E_INVALID),
        ("agy", None, crate::ipc_error::codes::E_UNSUPPORTED),
        ("gemini", None, E_INVALID),
        ("Claude", None, E_INVALID),
    ] {
        let mut args = args_named("dev-z", None);
        args.agent = Some(agent.into());
        args.kind = kind.map(str::to_string);
        assert_eq!(
            normalize_launch(&mut args).unwrap_err().code,
            code,
            "{agent} {kind:?}"
        );
    }
    // An agent shell still refuses Claude-only launch options.
    let mut args = args_named("dev-z", None);
    args.agent = Some("shell".into());
    args.effort = Some("high".into());
    assert_eq!(normalize_launch(&mut args).unwrap_err().code, E_INVALID);
    // Codex takes a model and an effort, but has no login profiles.
    let mut args = args_named("dev-z", None);
    args.agent = Some("codex".into());
    args.model = Some("gpt-6.1-sol".into());
    args.effort = Some("xhigh".into());
    assert!(normalize_launch(&mut args).is_ok());
    args.profile = Some("work".into());
    assert_eq!(normalize_launch(&mut args).unwrap_err().code, E_INVALID);
    // agy is refused before its options are looked at (12.3).
    let mut args = args_named("dev-z", None);
    args.agent = Some("agy".into());
    args.profile = Some("work".into());
    assert_eq!(
        normalize_launch(&mut args).unwrap_err().code,
        crate::ipc_error::codes::E_UNSUPPORTED
    );
}

/// A new Codex session starts bare (Codex names its own conversation, so
/// there is no id to record), and a restart, recreate or repair of a Codex
/// row resumes Codex, not Claude.
#[test]
fn a_codex_session_launches_codex() {
    use super::lifecycle::claude_id_and_pane_cmd;
    let mut args = args_named("dev-cx", None);
    args.agent = Some("codex".into());
    args.model = Some("gpt-6-luna".into());
    let (id, pane) = claude_id_and_pane_cmd(&args);
    assert_eq!(id, None);
    assert!(
        pane.starts_with("codex --dangerously-bypass-approvals-and-sandbox -m 'gpt-6-luna'"),
        "{pane}"
    );
    let id = "01a11dac-6e90-7da0-81c9-280b14a35226";
    let pane = recreate_pane_command("work", "codex", Some(id), "dev-cx", &Default::default());
    assert!(pane.starts_with(&format!("codex resume '{id}'")), "{pane}");
    let pane = recreate_pane_command("work", "codex", None, "dev-cx", &Default::default());
    assert!(pane.starts_with("codex resume --last"), "{pane}");
    // A Claude row is unchanged.
    let pane = recreate_pane_command("work", "claude", Some(id), "dev-cx", &Default::default());
    assert!(pane.contains(&format!("cl --resume '{id}'")), "{pane}");
}

/// A new agy session starts a fresh conversation with no id to record (agy
/// allocates its own), and a relaunch continues the cwd's newest one.
#[test]
fn an_agy_session_launches_agy() {
    use super::lifecycle::claude_id_and_pane_cmd;
    let mut args = args_named("dev-ag", None);
    args.agent = Some("agy".into());
    args.model = Some("gemini-3.8-flash".into());
    let (id, pane) = claude_id_and_pane_cmd(&args);
    assert_eq!(id, None);
    assert!(
        pane.contains("agy --model 'gemini-3.8-flash'; exec"),
        "{pane}"
    );
    assert!(
        !pane.contains("--continue"),
        "a new session starts fresh: {pane}"
    );
    let pane = recreate_pane_command("work", "agy", None, "dev-ag", &Default::default());
    assert!(pane.contains("agy --continue; exec"), "{pane}");
}

/// Redesign 12.3: an agy row (written before agy was refused, or by hand)
/// is not relaunched either: restart and recreate refuse it before any
/// tmux call, with the same `E_UNSUPPORTED` as `new_session`.
#[tokio::test]
async fn restart_and_recreate_refuse_an_agy_row() {
    use super::lifecycle::{recreate_session, restart_session};
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let id = s
        .upsert_session("dev-ag", "h", None, None, 1, 1, "running", None)
        .unwrap();
    s.set_session_agent(id, "agy").unwrap();
    let store = Mutex::new(s);
    let ssh = Arc::new(crate::ssh::SshClient::new());
    let restart = serde_json::from_value(serde_json::json!({
        "host_alias": "h", "name": "dev-ag"
    }))
    .unwrap();
    let err = restart_session(restart, &store, &ssh).await.unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_UNSUPPORTED, "{err:?}");
    let recreate = serde_json::from_value(serde_json::json!({ "session_id": id })).unwrap();
    let err = recreate_session(recreate, &store, &ssh).await.unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_UNSUPPORTED, "{err:?}");
    assert!(err.message.contains("agy"), "{}", err.message);
}

/// Recreate on a remote host whose checkout was deleted clones it back into
/// the root the repair resolved, without a worktree step: the repair that
/// runs next re-adds the worktree from its branch on origin.
#[tokio::test]
async fn recreate_reclones_a_missing_remote_checkout_into_the_resolved_root() {
    use super::lifecycle::reclone_project_root;
    use crate::ssh_fake::{FakeSsh, Match, Reply};
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("trn").unwrap();
    let pid = s.upsert_project("o", "pos-frontend", "/repo").unwrap();
    let wt = "/home/dev/projects/github.com/o/pos-frontend/.worktrees/feat";
    let wid = s
        .upsert_worktree_on("trn", pid, "feat", wt, Some("feat"))
        .unwrap();
    let sid = s
        .upsert_session(
            "dev-pos--feat",
            "trn",
            Some(pid),
            Some(wid),
            1,
            1,
            "lost",
            None,
        )
        .unwrap();
    let store = Mutex::new(s);
    let fake = FakeSsh::new();
    fake.with_home("/home/dev")
        .on(Match::script_contains("git clone"), Reply::ok(""));

    reclone_project_root(&store, &fake, sid).await.unwrap();

    let script = fake
        .calls_for("trn")
        .into_iter()
        .filter_map(|c| c.script())
        .find(|sc| sc.contains("git clone"))
        .expect("a clone script ran");
    assert!(
        script.contains("git clone 'git@github.com:o/pos-frontend.git'"),
        "{script}"
    );
    assert!(
        script.contains("if [ ! -d '/home/dev/projects/github.com/o/pos-frontend'/.git ]"),
        "{script}"
    );
    assert!(!script.contains("worktree add"), "{script}");
}

/// A failed clone (no access to origin) surfaces git's stderr as
/// `E_GIT_SETUP`, so the user sees why Recreate could not restore it.
#[tokio::test]
async fn a_failed_reclone_reports_git_setup() {
    use super::lifecycle::reclone_project_root;
    use crate::ssh_fake::{FakeSsh, Match, Reply};
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("trn").unwrap();
    let pid = s.upsert_project("o", "r", "/repo").unwrap();
    let sid = s
        .upsert_session("dev-r", "trn", Some(pid), None, 1, 1, "lost", None)
        .unwrap();
    let store = Mutex::new(s);
    let fake = FakeSsh::new();
    fake.with_home("/home/dev").on(
        Match::script_contains("git clone"),
        Reply::fail(128, "Permission denied (publickey).\n"),
    );
    let err = reclone_project_root(&store, &fake, sid).await.unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_GIT_SETUP);
    assert!(err.message.contains("Permission denied"), "{}", err.message);
}

/// Step 2.3: viewing stamps the row and answers it; an unknown id is
/// NOTFOUND, not a silent success.
#[test]
fn touching_a_session_marks_it_viewed() {
    use super::lifecycle::{touch_session_viewed, TouchSessionViewedArgs};
    let store = Mutex::new(crate::store::Store::open_in_memory().unwrap());
    let id = {
        let s = store.lock().unwrap();
        s.upsert_host("h").unwrap();
        s.upsert_session("w", "h", None, None, 1, 1, "running", None)
            .unwrap()
    };
    let row = touch_session_viewed(TouchSessionViewedArgs { session_id: id }, &store).unwrap();
    assert!(row.last_viewed_at.is_some_and(|t| t > 1_600_000_000));
    let missing = touch_session_viewed(
        TouchSessionViewedArgs {
            session_id: id + 99,
        },
        &store,
    );
    assert_eq!(
        missing.err().map(|e| e.code),
        Some(crate::ipc_error::codes::E_NOTFOUND.to_string())
    );
}
