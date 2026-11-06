//! Headless checks: which runs go headless, the flags and script they
//! carry, and that their result reads exactly like a session's.

use super::*;
use crate::service::work::run::{record_headless_run, record_run};
use crate::store::TaskEvidence;

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const BASE: &str = "89abcdef0123456789abcdef0123456789abcdef";

/// A store with an item whose implementation finished on `HEAD` in
/// `/w/pos-2` on host `mars`; answers the item.
fn world() -> (Store, i64) {
    let s = Store::open_in_memory().unwrap();
    s.insert_host("mars", None).unwrap();
    let project = s.upsert_project("acme", "pos", "/p/acme/pos").unwrap();
    let wt = s
        .upsert_worktree_on("mars", project, "pos-2", "/w/pos-2", Some("feat"))
        .unwrap();
    let worker = s
        .upsert_session("pos-2", "mars", Some(project), None, 0, 0, "running", None)
        .unwrap();
    s.conn_ref()
        .execute(
            "UPDATE sessions SET worktree_id = ?1 WHERE id = ?2",
            rusqlite::params![wt, worker],
        )
        .unwrap();
    let item = s
        .create_native_item(&crate::store::NativeItem {
            title: "Queue schema",
            parent_id: None,
            project_id: Some(project),
            notes: Some("Use IndexedDB."),
        })
        .unwrap()
        .id;
    let t = record_run(&s, worker, "do it", item, 1, "implement").unwrap();
    // A finished implementation reports, or it reads as a failed attempt.
    let report = crate::store::TaskReport {
        outcome: "done".into(),
        ..Default::default()
    };
    crate::service::tasks::complete_task_reported(&s, &t, "done", Some(&report)).unwrap();
    s.set_task_evidence(
        t.id,
        &TaskEvidence {
            head: Some(HEAD.into()),
            merge_base: Some(BASE.into()),
            ..Default::default()
        },
    )
    .unwrap();
    s.set_item_done_when(
        item,
        &["review".into(), "test: cargo test -p queue".into()],
        "me",
    )
    .unwrap();
    (s, item)
}

fn on(s: &Store) {
    settings::set(s, settings::ORCHESTRATOR_HEADLESS_CHECKS, "true").unwrap();
}

#[test]
fn only_plain_commands_are_pre_approved() {
    for ok in [
        "cargo test -p queue",
        "pnpm test",
        "npm run test:unit",
        "go test ./...",
        "pytest tests/unit --maxfail=1",
    ] {
        assert!(plain_command(ok), "{ok}");
    }
    for bad in [
        "",
        " cargo test",
        "cargo test; rm -rf /",
        "cargo test && echo",
        "cargo test | tee x",
        "cargo test > out",
        "echo $(id)",
        "echo `id`",
        "cargo 'test'",
        "cargo test *",
        "cargo test\nid",
        "cargo  test",
        &"x".repeat(201),
    ] {
        assert!(!plain_command(bad), "{bad:?}");
    }
}

/// On by default since lanes L4; a person who turns it off gets every
/// review and test as a session again.
#[test]
fn nothing_goes_headless_with_the_setting_off() {
    let (s, item) = world();
    assert!(settings::get_bool(
        &s,
        settings::ORCHESTRATOR_HEADLESS_CHECKS
    ));
    settings::set(&s, settings::ORCHESTRATOR_HEADLESS_CHECKS, "false").unwrap();
    for role in ["review", "test"] {
        assert!(plan(&s, item, role, None, None).is_none(), "{role}");
    }
}

#[test]
fn a_check_runs_in_the_implementers_checkout() {
    let (s, item) = world();
    on(&s);
    let p = plan(&s, item, "review", Some("work"), None).expect("a review");
    assert_eq!(
        p.at,
        Checkout {
            host: "mars".into(),
            path: "/w/pos-2".into(),
            head: HEAD.into(),
            merge_base: Some(BASE.into()),
            snapshot: false,
        }
    );
    assert!(p.cmds.is_empty());
    assert_eq!(p.model, "sonnet");
    assert_eq!(p.profile.as_deref(), Some("work"));
    assert_eq!(p.label, "POS-1 Queue schema");
    let t = plan(&s, item, "test", None, None).expect("a test");
    assert_eq!(t.cmds, vec!["cargo test -p queue".to_string()]);
    // Never for the work itself, nor outside the grant's hosts.
    assert!(plan(&s, item, "implement", None, None).is_none());
    assert!(plan(&s, item, "review", None, Some(&["venus".to_string()])).is_none());
    assert!(plan(&s, item, "review", None, Some(&["mars".to_string()])).is_some());
}

#[test]
fn a_test_line_that_is_not_plain_takes_the_session_path() {
    let (s, item) = world();
    on(&s);
    s.set_item_done_when(item, &["test: cargo test && rm -rf x".into()], "me")
        .unwrap();
    assert!(plan(&s, item, "test", None, None).is_none());
    // No command at all: the session runs "the project's tests".
    s.set_item_done_when(item, &["test".into()], "me").unwrap();
    assert!(plan(&s, item, "test", None, None).is_none());
    // A review does not need one.
    assert!(plan(&s, item, "review", None, None).is_some());
}

#[test]
fn no_finished_implementation_no_headless_check() {
    let (s, item) = world();
    on(&s);
    let t = s.latest_item_task(item, "implement").unwrap().unwrap();
    s.set_task_evidence(t.id, &TaskEvidence::default()).unwrap();
    assert!(plan(&s, item, "review", None, None).is_none(), "no head");
}

#[test]
fn the_review_reads_and_the_test_runs_only_its_commands() {
    let at = Checkout {
        host: "mars".into(),
        path: "/w/it's here".into(),
        head: HEAD.into(),
        merge_base: Some(BASE.into()),
        snapshot: false,
    };
    let review = check_script("review", &at, "sonnet", None, &[], "Review it's fine").unwrap();
    assert!(review.contains("--tools 'Read,Grep,Glob' "), "{review}");
    assert!(!review.contains("Bash") && !review.contains("--allowedTools"));
    assert!(review.contains("--permission-mode dontAsk"), "{review}");
    assert!(
        review.contains(&format!("git diff '{BASE}..HEAD'")),
        "{review}"
    );
    assert!(review.contains("cd -- '/w/it'\\''s here'"), "{review}");
    assert!(review.contains(&format!("= '{HEAD}' ]")), "{review}");
    assert!(review.contains("timeout 900"), "{review}");

    let cmds = vec!["cargo test -p queue".to_string()];
    let test = check_script("test", &at, "haiku", Some("work"), &cmds, "Test it").unwrap();
    assert!(test.contains("--tools 'Read,Grep,Glob,Bash'"), "{test}");
    assert!(
        test.contains("--allowedTools 'Bash(cargo test -p queue)'"),
        "{test}"
    );
    assert!(!test.contains("git diff"), "{test}");
    assert!(test.contains("timeout 1800"), "{test}");
    assert!(test.contains("CLAUDE_CONFIG_DIR"), "{test}");

    assert!(check_script("review", &at, "opus-9", None, &[], "x").is_err());
    let bad = vec!["cargo test; id".to_string()];
    assert!(check_script("test", &at, "haiku", None, &bad, "x").is_err());
}

#[test]
fn the_prompt_quotes_the_items_and_earlier_runs_words() {
    let cmds = vec!["pnpm test".to_string()];
    let p = check_prompt(
        "test",
        "TASK-7 Queue",
        Some("a\nb"),
        &cmds,
        Some("Run exactly: pnpm test"),
    );
    assert!(p.contains("- pnpm test"), "{p}");
    assert!(p.contains("> a\n> b"), "{p}");
    assert!(p.contains("> Run exactly: pnpm test"), "{p}");
    let r = check_prompt("review", "TASK-7 Queue", None, &[], None);
    assert!(r.contains("You cannot edit or run anything"), "{r}");
}

/// The whole run script through real bash, in a real git checkout, against
/// a fake `claude` that records its argv and stdin: the preflight passes,
/// the prompt and the diff arrive on stdin, the flags are the policy's.
#[cfg(unix)]
#[test]
fn the_script_runs_claude_in_the_checkout_with_the_prompt_on_stdin() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    let sh = |cmd: &str| {
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(cmd)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(out.status.success(), "{cmd}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    sh("git init -q . && git -c user.email=a@b -c user.name=a commit -q --allow-empty -m base");
    let base = sh("git rev-parse HEAD");
    sh("echo hi > f && git add f && git -c user.email=a@b -c user.name=a commit -q -m feat");
    let head = sh("git rev-parse HEAD");
    let got = dir.path().join("got");
    let argv = dir.path().join("argv");
    let reply = format!(
        "{}\n```json\n{{\"summary\":\"fine\",\"outcome\":\"done\"}}\n```",
        crate::service::tasks::done_marker("n1")
    );
    let envelope = serde_json::json!({"type": "result", "result": reply, "total_cost_usd": 0.01});
    std::fs::write(
        bin.join("claude"),
        format!(
            "#!/bin/sh\ncat > '{}'\nprintf '%s\\n' \"$@\" > '{}'\npwd >> '{}'\nprintf '%s\\n' {}\n",
            got.display(),
            argv.display(),
            argv.display(),
            crate::shell::quote(&envelope.to_string())
        ),
    )
    .unwrap();
    std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let at = Checkout {
        host: "local".into(),
        path: repo.display().to_string(),
        head: head.clone(),
        merge_base: Some(base),
        snapshot: false,
    };
    let run = |sc: &str| {
        let full = format!(
            "export PATH={}:\"$PATH\"; {sc}",
            crate::shell::quote(&bin.display().to_string())
        );
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(["bash", "-c", &crate::shell::quote(&full)].join(" "))
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    let pre = run(&preflight_script("review", &at));
    assert_eq!(
        claude_print::parse_tagged(&pre, TAG, VERDICTS).map(|v| v.0),
        Some("ok"),
        "{pre}"
    );
    let prompt = "Review it's \"fine\" $(touch pwned)";
    let out = run(&check_script("review", &at, "sonnet", None, &[], prompt).unwrap());
    let (v, text) = claude_print::parse_tagged(&out, TAG, VERDICTS).unwrap();
    assert_eq!(v, "run", "{out}");
    assert!(text.contains("outcome"), "{text}");
    let stdin = std::fs::read_to_string(&got).unwrap();
    assert!(stdin.starts_with(prompt), "{stdin}");
    assert!(stdin.contains("<diff>") && stdin.contains("+hi"), "{stdin}");
    let args = std::fs::read_to_string(&argv).unwrap();
    assert!(args.contains("\nRead,Grep,Glob\n"), "{args}");
    assert!(args.contains("\ndontAsk\n"), "{args}");
    assert!(
        !args.contains("Review"),
        "the prompt is not in claude's argv: {args}"
    );
    assert!(
        args.trim_end().ends_with(&repo.display().to_string()),
        "{args}"
    );
    assert!(!repo.join("pwned").exists());

    // A checkout that moved, or is dirty, never reaches claude.
    std::fs::remove_file(&got).unwrap();
    sh("echo more >> f");
    let dirty = run(&check_script("review", &at, "sonnet", None, &[], "x").unwrap());
    assert_eq!(
        claude_print::parse_tagged(&dirty, TAG, VERDICTS).map(|v| v.0),
        Some("dirty")
    );
    sh("git -c user.email=a@b -c user.name=a commit -qam more");
    let moved = run(&preflight_script("review", &at));
    assert_eq!(
        claude_print::parse_tagged(&moved, TAG, VERDICTS).map(|v| v.0),
        Some("moved")
    );
    assert!(!got.exists());

    // Lanes L4 (§3.7): a lane task's review runs in a snapshot of its
    // commit while the lane's checkout has moved on and is dirty, and leaves
    // no checkout behind; its test still waits for the lane's own checkout.
    sh("echo wip >> f");
    let lane = Checkout {
        snapshot: true,
        ..at.clone()
    };
    let pre = run(&preflight_script("review", &lane));
    assert_eq!(
        claude_print::parse_tagged(&pre, TAG, VERDICTS).map(|v| v.0),
        Some("ok"),
        "{pre}"
    );
    let out = run(&check_script("review", &lane, "sonnet", None, &[], "lane").unwrap());
    assert_eq!(
        claude_print::parse_tagged(&out, TAG, VERDICTS).map(|v| v.0),
        Some("run"),
        "{out}"
    );
    let stdin = std::fs::read_to_string(&got).unwrap();
    assert!(stdin.contains("+hi"), "the task's own diff: {stdin}");
    let args = std::fs::read_to_string(&argv).unwrap();
    let ran_in = args.trim_end().lines().last().unwrap().to_string();
    assert_ne!(
        ran_in,
        repo.display().to_string(),
        "not the lane's checkout"
    );
    assert!(
        !std::path::Path::new(&ran_in).exists(),
        "the snapshot is gone"
    );
    assert_eq!(sh("git worktree list | wc -l"), "1");
    let test = run(&preflight_script("test", &lane));
    assert_eq!(
        claude_print::parse_tagged(&test, TAG, VERDICTS).map(|v| v.0),
        Some("moved")
    );
}

/// The run's reply closes its task as a session's report would, and the
/// item's lines read it unchanged.
#[test]
fn a_reply_closes_the_task_and_passes_the_line() {
    let (s, item) = world();
    let review = record_headless_run(&s, "review it", item, 1, "review").unwrap();
    assert_eq!(review.worker_session_id, None);
    assert_eq!(review.state, "running");
    let store = Mutex::new(s);
    let reply = format!(
        "Looks right.\n{}\nApproved.\n```json\n{{\"summary\":\"fine\",\"outcome\":\"done\"}}\n```",
        crate::service::tasks::done_marker(&review.nonce)
    );
    finish(&store, &review, Ok(reply));
    let s = store.into_inner().unwrap();
    let t = s.get_task(review.id).unwrap().unwrap();
    assert_eq!(t.state, "done");
    assert_eq!(t.report.as_ref().unwrap().outcome, "done");
    let it = s.get_work_item(item).unwrap().unwrap();
    let c = verify::check_line(&s, &it, "review", &[], 0).unwrap();
    assert_eq!(c.state, "pass", "{c:?}");

    // A test run that says what it ran passes its line.
    let test = record_headless_run(&s, "test it", item, 1, "test").unwrap();
    let store = Mutex::new(s);
    let reply = format!(
        "{}\n```json\n{{\"summary\":\"ok\",\"outcome\":\"done\",\"tests_run\":[\"cargo test -p queue\"]}}\n```",
        crate::service::tasks::done_marker(&test.nonce)
    );
    finish(&store, &test, Ok(reply));
    let s = store.into_inner().unwrap();
    let c = verify::check_line(&s, &it, "test: cargo test -p queue", &[], 0).unwrap();
    assert_eq!(c.state, "pass", "{c:?}");
}

#[test]
fn a_failed_or_silent_run_fails_its_task() {
    let (s, item) = world();
    let a = record_headless_run(&s, "review it", item, 1, "review").unwrap();
    let store = Mutex::new(s);
    finish(&store, &a, Err(IpcError::new(codes::E_TIMEOUT, "too slow")));
    let s = store.into_inner().unwrap();
    let t = s.get_task(a.id).unwrap().unwrap();
    assert_eq!(
        (t.state.as_str(), t.error.as_deref()),
        ("failed", Some("too slow"))
    );

    let b = record_headless_run(&s, "review it", item, 2, "review").unwrap();
    let store = Mutex::new(s);
    finish(&store, &b, Ok("I could not decide.".into()));
    let s = store.into_inner().unwrap();
    assert_eq!(s.get_task(b.id).unwrap().unwrap().state, "failed");
}

#[test]
fn a_lost_headless_check_is_failed_after_its_deadline() {
    let (s, item) = world();
    let t = record_headless_run(&s, "review it", item, 1, "review").unwrap();
    let since = t.started_at.unwrap_or(t.created_at);
    let verdict = |now| crate::service::tasks::liveness_verdict(&t, None, None, now, 0);
    assert_eq!(verdict(since + DEADLINE_SECS - 1), None);
    assert!(verdict(since + DEADLINE_SECS + 1).is_some());
}

/// A headless check names no session: whoever sees the implementation's
/// worker sees it, and nobody else (a person's Start wave and graph read it
/// through this).
#[test]
fn a_headless_check_is_shown_to_whoever_sees_the_implementer() {
    use crate::mcp::auth::{Caller, ClientRef, TokenMode};
    let (s, item) = world();
    let ana = s.create_person("ana", None).unwrap().id;
    let bob = s.create_person("bob", None).unwrap().id;
    s.conn_ref()
        .execute(
            "UPDATE sessions SET owner_person_id = ?1, visibility = 'private'",
            [ana],
        )
        .unwrap();
    let t = record_headless_run(&s, "review it", item, 1, "review").unwrap();
    let person = |id| {
        Caller {
            api: None,
            host_alias: None,
            client: Some(ClientRef {
                id: 7,
                name: "phone".into(),
                trusted: false,
                org_id: None,
                person_id: Some(id),
            }),
            mode: TokenMode::Full,
            pane: None,
            is_personal_owner: false,
            is_fleet_admin: false,
        }
        .view_scope(&s)
        .unwrap()
    };
    let sees = |v: &ViewScope| crate::service::tasks::task_visible_in_scope(&s, &t, v).unwrap();
    assert!(sees(&person(ana)), "the implementer's owner");
    assert!(!sees(&person(bob)), "someone else");
    assert!(sees(&ViewScope::internal()));
    // No implementation left to judge by: nobody but the hub.
    s.conn_ref()
        .execute("DELETE FROM tasks WHERE role = 'implement'", [])
        .unwrap();
    assert!(!sees(&person(ana)));
}
