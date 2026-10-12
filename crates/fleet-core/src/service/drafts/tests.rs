//! Orbit Fleet 5.12: a commit message drafted on the session's own host,
//! under its own account, booked as a cost.

use super::*;
use crate::ssh_fake::{FakeSsh, Match, Reply};

const ENVELOPE: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"```\nAdd the routine account line\n\nRoutines say which account they bill.\n```","total_cost_usd":0.002,"usage":{"input_tokens":1200,"output_tokens":30}}"#;

fn store_with_session(profile: Option<&str>) -> (Mutex<Store>, i64) {
    let s = Store::open_in_memory().unwrap();
    s.insert_host("mercury", Some("mercury")).unwrap();
    let id = s
        .upsert_session("dev-1", "mercury", None, None, 1, 1, "running", None)
        .unwrap();
    s.set_session_profile(id, profile).unwrap();
    settings::set(&s, settings::WORK_DRAFT_COMMIT_MESSAGES, "true").unwrap();
    (Mutex::new(s), id)
}

#[tokio::test]
async fn nothing_runs_while_writing_help_is_off() {
    let (store, id) = store_with_session(None);
    settings::set(
        &lock(&store).unwrap(),
        settings::WORK_DRAFT_COMMIT_MESSAGES,
        "false",
    )
    .unwrap();
    let fake = FakeSsh::new();
    let e = draft_commit_message(&store, &fake, id).await.unwrap_err();
    assert_eq!(e.code, codes::E_INVALID_STATE);
    assert!(
        e.message.contains("Draft commit messages is off"),
        "{}",
        e.message
    );
    assert!(fake.calls().is_empty());
}

#[tokio::test]
async fn the_draft_runs_on_the_sessions_host_under_its_account_and_is_booked() {
    let (store, id) = store_with_session(Some("work"));
    let fake = FakeSsh::new();
    fake.on_host(
        "mercury",
        Match::script_contains("claude -p"),
        Reply::ok(&format!(
            "fleet-draft=files=2\nfleet-draft=run\n{ENVELOPE}\n"
        )),
    );
    let d = draft_commit_message(&store, &fake, id).await.unwrap();
    assert_eq!(
        d.message,
        "Add the routine account line\n\nRoutines say which account they bill."
    );
    assert_eq!((d.host_alias.as_str(), d.files), ("mercury", 2));
    let calls = fake.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].host, "mercury", "the session's own host");
    let script = calls[0].script().unwrap();
    assert!(
        script.contains("export CLAUDE_CONFIG_DIR=\"$HOME/.claude-profiles/\"'work'; "),
        "{script}"
    );
    assert!(script.contains("git diff --cached"), "{script}");
    assert!(script.contains("--tools ''"), "isolated: {script}");
    let rows = lock(&store)
        .unwrap()
        .aux_usage_of_origin(crate::store::AUX_ORIGIN_COMMIT_MESSAGE)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].host_alias.as_str(), rows[0].cost_micros),
        ("mercury", 2_000)
    );
}

#[tokio::test]
async fn nothing_staged_is_refused_and_costs_nothing() {
    let (store, id) = store_with_session(None);
    let fake = FakeSsh::new();
    fake.on_host("mercury", Match::Any, Reply::ok("fleet-draft=empty\n"));
    let e = draft_commit_message(&store, &fake, id).await.unwrap_err();
    assert_eq!(e.code, codes::E_INVALID_STATE);
    let script = fake.calls()[0].script().unwrap();
    assert!(
        !script.contains("CLAUDE_CONFIG_DIR"),
        "the host's own login"
    );
    assert!(lock(&store)
        .unwrap()
        .aux_usage_of_origin(crate::store::AUX_ORIGIN_COMMIT_MESSAGE)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn an_unknown_session_is_not_found() {
    let (store, _) = store_with_session(None);
    let fake = FakeSsh::new();
    let e = draft_commit_message(&store, &fake, 999).await.unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    assert!(fake.calls().is_empty());
}

#[test]
fn the_body_refuses_a_bad_model_or_profile() {
    assert!(commit_body("gpt", None).is_err());
    assert!(commit_body("haiku", Some("../x")).is_err());
    let b = commit_body("haiku", None).unwrap();
    assert!(b.contains("--model 'haiku'"), "{b}");
    assert!(b.contains(&format!("head -c {DIFF_CAP_BYTES}")), "{b}");
}

#[test]
fn clean_message_drops_fences_and_caps() {
    assert_eq!(clean_message("```text\nFix it\n```"), "Fix it");
    assert_eq!(clean_message("  Fix it  \n"), "Fix it");
    assert_eq!(
        clean_message(&"x".repeat(MESSAGE_MAX_CHARS + 10))
            .chars()
            .count(),
        MESSAGE_MAX_CHARS
    );
}
