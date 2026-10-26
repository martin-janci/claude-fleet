//! The worker's report and fleet's own git evidence (orchestration O3).

use super::*;

const NONCE: &str = "abcd1234";

#[test]
fn the_instruction_names_the_marker_inline_and_asks_for_the_block() {
    let i = run_instruction(7, NONCE);
    assert!(
        i.contains("FLEET_TASK_DONE_abcd1234 on its own line"),
        "{i}"
    );
    assert!(i.contains("```json"), "{i}");
    // The tool comes first, with the ids it needs; the marker is the fallback.
    assert!(
        i.find("task_report").unwrap() < i.find("FLEET_TASK_DONE_").unwrap(),
        "{i}"
    );
    assert!(
        i.contains("task_id 7") && i.contains("nonce \"abcd1234\""),
        "{i}"
    );
    // The echo of the prompt is never a marker line.
    assert_eq!(
        after_marker(&with_run_instruction("do it", 7, NONCE), NONCE),
        None
    );
}

#[test]
fn a_fenced_block_after_the_marker_is_the_report() {
    let pane = "⏺ Done with the schema.\n\
                ⏺ FLEET_TASK_DONE_abcd1234\n\
                ```json\n\
                {\"summary\": \"Added the queue table\", \"outcome\": \"DONE\",\n\
                 \"tests_run\": [\"cargo test queue\"], \"warnings\": [],\n\
                 \"followups\": \"index the status column\", \"confidence\": 0.8}\n\
                ```\n";
    let r = parse_report(after_marker(pane, NONCE).unwrap()).unwrap();
    assert_eq!(r.summary, "Added the queue table");
    assert_eq!(r.outcome, "done");
    assert_eq!(r.tests_run, vec!["cargo test queue"]);
    assert_eq!(
        r.followups,
        vec!["index the status column"],
        "a string is a list of one"
    );
    assert_eq!(r.confidence.as_deref(), Some("0.8"));
}

#[test]
fn a_bare_object_parses_and_prose_alone_is_no_report() {
    let bare =
        "\n{\"summary\": \"ok\", \"outcome\": \"blocked\", \"blockers\": [\"no db\"]}\nthanks";
    let r = parse_report(bare).unwrap();
    assert_eq!((r.outcome.as_str(), r.blockers.len()), ("blocked", 1));
    assert_eq!(parse_report("Implemented it, all tests pass."), None);
    assert_eq!(parse_report("```json\n{ not json\n```"), None);
}

#[test]
fn an_unknown_outcome_is_partial_and_lists_are_capped() {
    let many: Vec<String> = (0..50).map(|i| format!("w{i}")).collect();
    let v =
        serde_json::json!({ "outcome": "shipped", "warnings": many, "summary": "x".repeat(9000) });
    let r = report_from_value(&v).unwrap();
    assert_eq!(r.outcome, "partial");
    assert_eq!(r.warnings.len(), REPORT_LIST_MAX);
    assert_eq!(r.summary.chars().count(), SUMMARY_MAX_CHARS);
}

#[test]
fn the_last_marker_line_wins() {
    let t = "FLEET_TASK_DONE_abcd1234\n```json\n{\"outcome\":\"failed\"}\n```\n\
             FLEET_TASK_DONE_abcd1234\n```json\n{\"outcome\":\"done\"}\n```";
    assert_eq!(
        parse_report(after_marker(t, NONCE).unwrap())
            .unwrap()
            .outcome,
        "done"
    );
}

#[test]
fn the_evidence_output_parses_and_an_error_says_why() {
    let out = "__FLEET_EV_HEAD__\tbbb\torigin/main\taaa\n\
               __FLEET_EV_NC__\t2\n\
               __FLEET_EV_C__\tc2\tAdd the index\n\
               __FLEET_EV_C__\tc1\tAdd the table\n\
               __FLEET_EV_NF__\t2\n\
               __FLEET_EV_F__\t10\t2\tsrc/queue.rs\n\
               __FLEET_EV_F__\t-\t-\tlogo.png\n\
               __FLEET_EV_DIRTY__\t0\n";
    let ev = parse_evidence(out, 7);
    assert_eq!(ev.head.as_deref(), Some("bbb"));
    assert_eq!(ev.base.as_deref(), Some("origin/main"));
    assert_eq!(ev.commits_total, 2);
    assert_eq!(ev.commits[1].subject, "Add the table");
    assert_eq!(ev.files[0].added, Some(10));
    assert_eq!(ev.files[1].added, None, "a binary file has no line counts");
    assert_eq!(ev.uncommitted, Some(0));
    assert_eq!(ev.error, None);
    let none = parse_evidence("__FLEET_EV_ERR__\tno base branch\n", 7);
    assert_eq!(none.error.as_deref(), Some("no base branch"));
    assert_eq!(
        parse_evidence("", 7).error.as_deref(),
        Some("git gave no answer")
    );
}

#[cfg(unix)]
fn git(dir: &std::path::Path, args: &[&str]) {
    let out = crate::proc::std_command("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// The script runs on a fleet host (a Unix shell); a Windows runner's
// `bash` is not one.
#[cfg(unix)]
#[test]
fn the_script_reads_a_real_checkout_against_its_base() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    std::fs::write(d.join("a.txt"), "one\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "base"]);
    git(d, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(d, &["checkout", "-qb", "task"]);
    std::fs::write(d.join("b.txt"), "two\nthree\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "Add b"]);
    std::fs::write(d.join("a.txt"), "changed\n").unwrap();
    let out = crate::proc::std_command("bash")
        .args(["-c", &evidence_script(&d.to_string_lossy())])
        .output()
        .unwrap();
    let ev = parse_evidence(&String::from_utf8_lossy(&out.stdout), 1);
    assert_eq!(ev.error, None, "{ev:?}");
    assert_eq!(ev.base.as_deref(), Some("origin/main"));
    assert_eq!(ev.commits_total, 1);
    assert_eq!(ev.commits[0].subject, "Add b");
    assert_eq!(ev.files.len(), 1);
    assert_eq!(ev.files[0].path, "b.txt");
    assert_eq!(ev.files[0].added, Some(2));
    assert_eq!(
        ev.uncommitted,
        Some(1),
        "a.txt was changed and not committed"
    );

    let gone = crate::proc::std_command("bash")
        .args(["-c", &evidence_script("/nonexistent/fleet-o3")])
        .output()
        .unwrap();
    assert_eq!(
        parse_evidence(&String::from_utf8_lossy(&gone.stdout), 1)
            .error
            .as_deref(),
        Some("no checkout")
    );
}

#[test]
fn a_report_keeps_done_decisions_remaining_and_context() {
    let v = serde_json::json!({
        "summary": "Added the gate.",
        "outcome": "partial",
        "done": ["gate.rs with completion_gate"],
        "decisions": ["the gate never moves an item itself"],
        "remaining": ["Windows path of the probe"],
        "context": ["verify.rs computes on read"],
    });
    let r = report_from_value(&v).unwrap();
    assert_eq!(r.done, vec!["gate.rs with completion_gate"]);
    assert_eq!(r.decisions, vec!["the gate never moves an item itself"]);
    assert_eq!(r.remaining, vec!["Windows path of the probe"]);
    assert_eq!(r.context, vec!["verify.rs computes on read"]);
}

#[test]
fn a_report_stored_before_the_new_keys_still_reads() {
    let old = r#"{"summary":"x","outcome":"done","followups":["a"]}"#;
    let r: crate::store::TaskReport = serde_json::from_str(old).unwrap();
    assert!(r.done.is_empty() && r.decisions.is_empty());
    assert!(r.remaining.is_empty() && r.context.is_empty());
    assert_eq!(r.followups, vec!["a"]);
}

#[test]
fn the_instruction_names_every_key_the_parser_reads() {
    let text = run_instruction(7, "abc");
    for key in [
        "summary",
        "outcome",
        "done",
        "decisions",
        "remaining",
        "context",
        "tests_run",
        "warnings",
        "blockers",
        "followups",
        "confidence",
    ] {
        assert!(text.contains(&format!("\"{key}\"")), "{key} missing");
    }
}

/// Lanes design L3 (§3.3): a task that began part way along a shared
/// branch is read from where it began, not from the merge base; a start the
/// branch no longer holds (history rewritten) is an error.
#[cfg(unix)]
#[test]
fn a_lane_task_is_read_from_where_it_began() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let head = |d: &std::path::Path| {
        let out = crate::proc::std_command("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(d)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(d, &["init", "-q", "-b", "main"]);
    std::fs::write(d.join("a.txt"), "one\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "base"]);
    git(d, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(d, &["checkout", "-qb", "lane"]);
    std::fs::write(d.join("first.txt"), "1\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "First task"]);
    let start = head(d);
    std::fs::write(d.join("second.txt"), "2\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "Second task"]);
    let run = |since: Option<&str>| {
        let out = crate::proc::std_command("bash")
            .args(["-c", &evidence_script_since(&d.to_string_lossy(), since)])
            .output()
            .unwrap();
        parse_evidence(&String::from_utf8_lossy(&out.stdout), 1)
    };
    let ev = run(Some(&start));
    assert_eq!(ev.error, None, "{ev:?}");
    assert_eq!(ev.commits_total, 1);
    assert_eq!(ev.commits[0].subject, "Second task");
    assert_eq!(
        ev.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        ["second.txt"]
    );
    assert_eq!(run(None).commits_total, 2, "the merge base sees both");
    // An amend that rewrites the task's start.
    git(d, &["reset", "-q", "--hard", "HEAD~2"]);
    std::fs::write(d.join("other.txt"), "x\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "Rewritten"]);
    let ev = run(Some(&start));
    assert!(
        ev.error
            .as_deref()
            .is_some_and(|e| e.contains("no longer holds")),
        "{ev:?}"
    );
    // Something that is no commit id falls back to the merge base.
    assert!(!evidence_script_since("/x", Some("; rm -rf ~")).contains("rm -rf"));
}

/// review9: the loop's retry reads each failed checkout once for all its
/// runs, tries a host that failed only once a pass, leaves a read newer than
/// a minute alone, and keeps a failed mid-turn read open for the Stop.
#[cfg(unix)]
#[tokio::test]
async fn the_retry_reads_each_checkout_once_and_keeps_a_mid_turn_read_open() {
    use crate::tmux::fake_exec::{write_exec, PROBE_GUARD};
    let bin = tempfile::tempdir().unwrap();
    let log = bin.path().join("calls");
    let fake = write_exec(
        bin.path(),
        "ssh",
        &format!(
            "#!/bin/sh\n{PROBE_GUARD}\
             case \"$*\" in *'-O check'*|*'-O exit'*) exit 0;; esac\n\
             while [ \"$#\" -gt 0 ] && [ \"$1\" != \"--\" ]; do shift; done\n\
             echo \"$2\" >> '{log}'\n\
             [ \"$2\" = dead ] && exit 255\n\
             shift 2\n\
             exec sh -c \"$*\"\n",
            log = log.display()
        ),
    );
    let ssh = Arc::new(SshClient::with_ssh_binary(fake));
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let now = 10_000;
    let run = |host: &str, dir: &str, at: i64, provisional: bool| {
        let s = store.lock().unwrap();
        let item = s.create_local_work_item(None, "queue").unwrap().id;
        let t = s.insert_task(None, Some(7), "go", "n").unwrap();
        s.set_task_run(t.id, item, 1, "implement").unwrap();
        s.conn_ref()
            .execute(
                "UPDATE tasks SET state = 'done', finished_at = ?2 WHERE id = ?1",
                rusqlite::params![t.id, now - 100],
            )
            .unwrap();
        let ev = TaskEvidence {
            at,
            error: Some("ssh: reset".into()),
            host: Some(host.into()),
            dir: Some(dir.into()),
            provisional,
            ..Default::default()
        };
        s.set_task_evidence(t.id, &ev).unwrap();
        (t.id, ev)
    };
    let (mid_turn, _) = run("mars", "/nonexistent/fleet-r9", now - 100, true);
    let (after_stop, _) = run("mars", "/nonexistent/fleet-r9", now - 100, false);
    let (dead_a, _) = run("dead", "/w/a", now - 100, false);
    let (dead_b, dead_b_ev) = run("dead", "/w/b", now - 100, false);
    let (fresh, fresh_ev) = run("mars", "/nonexistent/other", now - 10, false);

    retry_unread_evidence(&store, &ssh, now).await;

    let calls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        calls.lines().collect::<Vec<_>>(),
        vec!["mars", "dead"],
        "one read per checkout, one try per failed host, none too soon"
    );
    let ev = |id: i64| {
        store
            .lock()
            .unwrap()
            .get_task(id)
            .unwrap()
            .unwrap()
            .evidence
            .unwrap()
    };
    let read = ev(mid_turn);
    assert_eq!(read.error.as_deref(), Some("no checkout"), "{read:?}");
    assert!(read.provisional, "a mid-turn read stays owed to the Stop");
    let read = ev(after_stop);
    assert_eq!(read.error.as_deref(), Some("no checkout"));
    assert!(!read.provisional);
    let failed = ev(dead_a);
    assert_eq!(failed.host.as_deref(), Some("dead"), "read again later");
    assert!(failed.at > now - 100);
    assert_eq!(ev(dead_b), dead_b_ev, "the host already failed this pass");
    assert_eq!(ev(fresh), fresh_ev, "read less than a minute ago");
}
