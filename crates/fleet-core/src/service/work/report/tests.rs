//! The worker's report and fleet's own git evidence (orchestration O3).

use super::*;

const NONCE: &str = "abcd1234";

#[test]
fn the_instruction_names_the_marker_inline_and_asks_for_the_block() {
    let i = run_instruction(NONCE);
    assert!(
        i.contains("FLEET_TASK_DONE_abcd1234 on its own line"),
        "{i}"
    );
    assert!(i.contains("```json"), "{i}");
    // The echo of the prompt is never a marker line.
    assert_eq!(
        after_marker(&with_run_instruction("do it", NONCE), NONCE),
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
