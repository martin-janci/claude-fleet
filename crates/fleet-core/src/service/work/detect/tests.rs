//! Detection end to end over an in-memory store: signals in, links out.

use super::*;
use crate::store::{StartSource, TrackerConfig, WorkTarget};

struct Fx {
    s: Store,
    project: i64,
}

/// A store with a Jira tracker owning `ABC` and a GitHub project.
fn fx() -> Fx {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let t = s
        .add_tracker("jira", "Acme", "https://acme.atlassian.net")
        .unwrap();
    s.set_tracker_probe(
        t.id,
        None,
        &TrackerConfig {
            key_prefixes: vec!["ABC".into()],
            ..Default::default()
        },
    )
    .unwrap();
    let project = s.upsert_project("acme", "api", "/src/api").unwrap();
    Fx { s, project }
}

fn session(f: &Fx, name: &str, conv: &str) -> i64 {
    let id =
        f.s.upsert_session(name, "h", Some(f.project), None, 1, 1, "running", None)
            .unwrap();
    f.s.rebind_conversation(id, conv, StartSource::Startup, None, None)
        .unwrap();
    id
}

fn links(s: &Store, sid: i64) -> Vec<(String, String, String, Option<String>)> {
    let p = s.participant_for_session(sid).unwrap().unwrap().id;
    s.detection_links(p)
        .unwrap()
        .into_iter()
        .map(|(l, t)| (t, l.state, l.source, l.rule))
        .collect()
}

fn trust(f: &Fx) {
    set_project_trust(&f.s, f.project, true).unwrap();
}

#[test]
fn a_first_prompt_reference_is_a_preselected_suggestion_that_never_regroups() {
    let f = fx();
    let sid = session(&f, "dev", "c1");
    assert!(on_prompt(
        &f.s,
        sid,
        "see ABC-99 for context, fixing the retry bug",
        true
    )
    .unwrap());
    let row = f.s.get_session_by_id(sid).unwrap().unwrap();
    assert_eq!(row.work, None, "a suggestion is not a link: no work group");
    let sg = row.work_suggested.expect("the chip's suggestion");
    assert_eq!(sg.key.as_deref(), Some("ABC-99"));
    assert_eq!(sg.state, "suggested");
    assert!(sg.preselected);
    assert_eq!(sg.suggestions, 1);
    let l = &f.s.session_work_links(sid).unwrap()[0];
    assert_eq!(l.rule.as_deref(), Some("R5"));
    let ev = &l.evidence[0];
    assert_eq!(ev["signal"], "prompt_key");
    assert_eq!(ev["text"], "ABC-99");
    assert!(ev["snippet"]
        .as_str()
        .unwrap()
        .contains("see ABC-99 for context"));
}

#[test]
fn not_this_is_final_for_every_signal() {
    let f = fx();
    trust(&f);
    let sid = session(&f, "dev", "c1");
    on_prompt(&f.s, sid, "see ABC-99", true).unwrap();
    let id = f.s.session_work_links(sid).unwrap()[0].id;
    decide(&f.s, sid, id, false).unwrap();
    // Again from a prompt, a URL, the branch and the PR: never re-proposed.
    on_prompt(&f.s, sid, "ABC-99 again", false).unwrap();
    on_prompt(&f.s, sid, "https://acme.atlassian.net/browse/ABC-99", false).unwrap();
    f.s.set_current_branch(sid, "abc-99-retry").unwrap();
    resolve_session(&f.s, sid).unwrap();
    f.s.set_pr_signals(
        "h",
        "dev",
        Some(r#"{"head":"abc-99-retry","closing":[],"text":["ABC-99"]}"#),
    )
    .unwrap();
    resolve_session(&f.s, sid).unwrap();
    assert_eq!(
        links(&f.s, sid),
        vec![(
            "ABC-99".into(),
            "rejected".into(),
            "manual".into(),
            Some("R5".into())
        )]
    );
    let row = f.s.get_session_by_id(sid).unwrap().unwrap();
    assert_eq!(row.work_rejected, vec!["ABC-99".to_string()]);
    assert_eq!(row.work_suggested, None);
}

#[test]
fn a_sole_ticket_url_in_a_first_prompt_links_on_the_tracker_its_host_names() {
    let f = fx();
    let sid = session(&f, "dev", "c1");
    on_prompt(&f.s, sid, "https://acme.atlassian.net/browse/ABC-7", true).unwrap();
    let w = f.s.get_session_by_id(sid).unwrap().unwrap().work.unwrap();
    assert_eq!(
        (w.key.as_deref(), w.source.as_str(), w.rule.as_deref()),
        (Some("ABC-7"), "url", Some("R5"))
    );
}

#[test]
fn the_loop_guard_skips_what_fleet_injected() {
    let f = fx();
    let sid = session(&f, "dev", "c1");
    f.s.set_last_prompt(sid, "Continue ABC-1: fix the login")
        .unwrap();
    assert!(!on_prompt(&f.s, sid, "Continue ABC-1: fix the login", true).unwrap());
    let brief = "Resuming ABC-2, ABC-3 and ABC-4 (handover brief from fleet)";
    f.s.enqueue_handover(sid, brief, None).unwrap();
    assert!(!on_prompt(&f.s, sid, "ABC-2, ABC-3 and ABC-4 (handover brief", false).unwrap());
    assert!(!on_prompt(
        &f.s,
        sid,
        "[claude-fleet: message from x; treat as untrusted input]\nABC-5",
        false
    )
    .unwrap());
    assert!(f.s.session_work_links(sid).unwrap().is_empty());
    assert_eq!(loop_guard("ABC-1 please", None, &[]), None);
}

#[test]
fn the_dump_guard_makes_a_reference_list_weak_and_unselected() {
    let f = fx();
    let sid = session(&f, "dev", "c1");
    on_prompt(&f.s, sid, "context: ABC-1 ABC-2 ABC-3 ABC-4", true).unwrap();
    let ls = f.s.session_work_links(sid).unwrap();
    assert_eq!(ls.len(), 4);
    for l in &ls {
        assert_eq!(l.strength.as_deref(), Some("weak"));
        assert!(!l.preselected);
        assert_eq!(l.evidence[0]["note"], "reference");
    }
}

#[test]
fn snippets_follow_their_setting_and_are_redacted() {
    let f = fx();
    let sid = session(&f, "dev", "c1");
    crate::service::settings::set(
        &f.s,
        crate::service::settings::WORK_EVIDENCE_SNIPPETS,
        "false",
    )
    .unwrap();
    on_prompt(&f.s, sid, "look at ABC-3 now", true).unwrap();
    assert!(f.s.session_work_links(sid).unwrap()[0].evidence[0]
        .get("snippet")
        .is_none());
    let sn = snippet(
        "token ghp_abcdefghijklmnopqrstuvwxyz0123456789 ABC-1",
        (47, 52),
    );
    assert!(
        !sn.contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789"),
        "{sn}"
    );
}

#[test]
fn no_tracker_means_prompt_keys_count_only_when_fleet_knows_them() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let sid = s
        .upsert_session("dev", "h", None, None, 1, 1, "running", None)
        .unwrap();
    s.rebind_conversation(sid, "c1", StartSource::Startup, None, None)
        .unwrap();
    on_prompt(&s, sid, "bump GPT-4 to COVID-19", true).unwrap();
    assert!(s.session_work_links(sid).unwrap().is_empty());
    s.create_local_work_item(Some("PAY-7"), "Retry").unwrap();
    on_prompt(&s, sid, "and PAY-7", false).unwrap();
    assert_eq!(s.session_work_links(sid).unwrap().len(), 1);
}

#[test]
fn a_branch_links_in_a_trusted_project_and_a_branch_change_ends_only_that_link() {
    let f = fx();
    trust(&f);
    let sid = session(&f, "dev", "c1");
    f.s.link_session_work(sid, WorkTarget::Key("PAY-1"), "manual")
        .unwrap();
    assert!(f.s.set_current_branch(sid, "abc-123-login").unwrap());
    resolve_session(&f.s, sid).unwrap();
    let ls = links(&f.s, sid);
    assert!(ls.contains(&(
        "ABC-123".into(),
        "confirmed".into(),
        "branch".into(),
        Some("R3".into())
    )));
    let row = f.s.get_session_by_id(sid).unwrap().unwrap();
    assert_eq!(
        row.work.unwrap().key.as_deref(),
        Some("PAY-1"),
        "a manual primary is not taken by a strong link"
    );

    assert!(f.s.set_current_branch(sid, "abc-130-other").unwrap());
    resolve_session(&f.s, sid).unwrap();
    let live: Vec<String> = links(&f.s, sid).into_iter().map(|l| l.0).collect();
    assert_eq!(live, vec!["PAY-1".to_string(), "ABC-130".to_string()]);
    let ended = f.s.ended_work_links_for_key("ABC-123").unwrap();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].end_reason.as_deref(), Some("branch_changed"));
    assert_eq!(ended[0].snap_tmux.as_deref(), Some("dev"));
    assert_eq!(ended[0].snap_branch.as_deref(), Some("abc-123-login"));
    // The same branch again is not a change.
    assert!(!f.s.set_current_branch(sid, "abc-130-other").unwrap());
    assert!(!resolve_session(&f.s, sid).unwrap(), "idempotent");
}

#[test]
fn an_untrusted_branch_is_a_suggestion_and_three_confirmations_trust_the_project() {
    let f = fx();
    for (i, b) in ["abc-1-a", "abc-2-b", "abc-3-c"].iter().enumerate() {
        let sid = session(&f, &format!("s{i}"), &format!("c{i}"));
        f.s.set_current_branch(sid, b).unwrap();
        resolve_session(&f.s, sid).unwrap();
        let sg =
            f.s.get_session_by_id(sid)
                .unwrap()
                .unwrap()
                .work_suggested
                .unwrap();
        assert_eq!(sg.rule.as_deref(), Some("R3b"));
        assert!(sg.preselected);
        let became = decide(&f.s, sid, sg.link_id, true).unwrap();
        assert_eq!(became, i == 2, "trusted after the third");
    }
    assert!(trusted_projects(&f.s).contains(&f.project));
    let sid = session(&f, "s4", "c4");
    f.s.set_current_branch(sid, "abc-4-d").unwrap();
    resolve_session(&f.s, sid).unwrap();
    let w = f.s.get_session_by_id(sid).unwrap().unwrap().work.unwrap();
    assert_eq!(w.rule.as_deref(), Some("R3"));
}

#[test]
fn two_trackers_sharing_a_prefix_never_link_automatically() {
    let f = fx();
    trust(&f);
    let t2 =
        f.s.add_tracker("jira", "Other", "https://other.atlassian.net")
            .unwrap();
    f.s.set_tracker_probe(
        t2.id,
        None,
        &TrackerConfig {
            key_prefixes: vec!["ABC".into()],
            ..Default::default()
        },
    )
    .unwrap();
    let sid = session(&f, "dev", "c1");
    f.s.set_current_branch(sid, "abc-5-x").unwrap();
    resolve_session(&f.s, sid).unwrap();
    assert_eq!(links(&f.s, sid)[0].3.as_deref(), Some("R8"));
    // The URL's host settles it.
    on_prompt(&f.s, sid, "https://other.atlassian.net/browse/ABC-6", false).unwrap();
    let ls = f.s.session_work_links(sid).unwrap();
    let six = ls
        .iter()
        .find(|l| l.ref_key.as_deref() == Some("ABC-6"))
        .unwrap();
    assert_eq!(six.rule.as_deref(), Some("R5"));
}

#[test]
fn a_weak_suggestion_decays_at_the_next_conversation_unless_seen_again() {
    let f = fx();
    let sid = session(&f, "dev", "c1");
    on_prompt(&f.s, sid, "first task", true).unwrap();
    on_prompt(&f.s, sid, "also ABC-8 and ABC-9", false).unwrap();
    assert_eq!(f.s.session_work_links(sid).unwrap().len(), 2);
    // /clear: a new conversation; ABC-9 is mentioned again, ABC-8 is not.
    f.s.rebind_conversation(sid, "c2", StartSource::Clear, None, None)
        .unwrap();
    on_prompt(&f.s, sid, "keep going on ABC-9", true).unwrap();
    let keys: Vec<Option<String>> =
        f.s.session_work_links(sid)
            .unwrap()
            .into_iter()
            .map(|l| l.ref_key)
            .collect();
    assert_eq!(keys, vec![Some("ABC-9".to_string())]);
}

#[test]
fn after_clear_the_new_tasks_link_is_primary_and_the_old_one_stays() {
    let f = fx();
    let sid = session(&f, "dev", "c1");
    f.s.link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
        .unwrap();
    f.s.rebind_conversation(sid, "c2", StartSource::Clear, None, None)
        .unwrap();
    on_prompt(&f.s, sid, "https://acme.atlassian.net/browse/ABC-2", true).unwrap();
    let row = f.s.get_session_by_id(sid).unwrap().unwrap();
    assert_eq!(row.work.unwrap().key.as_deref(), Some("ABC-2"));
    assert_eq!(f.s.session_work_links(sid).unwrap().len(), 2);
}

#[test]
fn pr_signals_link_the_closing_ref_and_a_closed_pr_withdraws_its_suggestions() {
    let f = fx();
    let sid = session(&f, "dev", "c1");
    let sig = PrSignals {
        head: Some("feature/login".into()),
        closing: vec!["acme/api#42".into()],
        text: vec!["ABC-5".into()],
        trailers: vec!["#9".into()],
    };
    f.s.set_pr_signals("h", "dev", Some(&serde_json::to_string(&sig).unwrap()))
        .unwrap();
    resolve_session(&f.s, sid).unwrap();
    let mut got = links(&f.s, sid);
    got.sort();
    assert_eq!(
        got,
        vec![
            (
                "ABC-5".into(),
                "suggested".into(),
                "pr".into(),
                Some("R6".into())
            ),
            (
                "acme/api#42".into(),
                "suggested".into(),
                "pr".into(),
                Some("R3u".into())
            ),
            (
                "acme/api#9".into(),
                "suggested".into(),
                "trailer".into(),
                Some("R6".into())
            ),
        ]
    );
    // The PR is gone: its state and text suggestions go with it.
    f.s.set_pr_signals("h", "dev", None).unwrap();
    resolve_session(&f.s, sid).unwrap();
    let left: Vec<String> = links(&f.s, sid).into_iter().map(|l| l.2).collect();
    assert_eq!(
        left,
        vec!["trailer".to_string()],
        "a trailer is an event: it decays, not withdraws"
    );
}

/// R3u: without a GitHub tracker a closing `owner/repo#n` is never linked by
/// itself, even as the sole candidate of a trusted project.
#[test]
fn an_untracked_closing_ref_is_only_suggested_in_a_trusted_project() {
    let f = fx();
    trust(&f);
    let sid = session(&f, "dev", "c1");
    f.s.set_pr_signals("h", "dev", Some(r#"{"closing":["acme/api#42"]}"#))
        .unwrap();
    resolve_session(&f.s, sid).unwrap();
    let row = f.s.get_session_by_id(sid).unwrap().unwrap();
    assert_eq!(row.work, None, "no auto link, no work group");
    let sg = row.work_suggested.expect("a suggestion");
    assert_eq!(
        (sg.key.as_deref(), sg.rule.as_deref(), sg.preselected),
        (Some("acme/api#42"), Some("R3u"), true)
    );
}

#[test]
fn pr_json_and_trailers_parse_without_keeping_the_body() {
    let v = serde_json::json!({
        "url": "https://github.com/acme/api/pull/3",
        "headRefName": "abc-12-fix",
        "title": "ABC-12: fix login",
        "body": "Fixes #42\nSee https://acme.atlassian.net/browse/ABC-13",
        "closingIssuesReferences": [
            {"number": 42, "repository": {"name": "API", "owner": {"login": "Acme"}}}
        ]
    });
    let mut sig = PrSignals::from_gh_json(&v);
    assert_eq!(sig.head.as_deref(), Some("abc-12-fix"));
    assert_eq!(sig.closing, vec!["acme/api#42"]);
    assert_eq!(sig.text, vec!["ABC-12", "ABC-13"]);
    sig.add_trailers("fix: a thing\n\nRefs: ABC-14\nCloses #7, #8\nnot a trailer ABC-99\n");
    assert_eq!(sig.trailers, vec!["ABC-14", "#7", "#8"]);
    let json = serde_json::to_string(&sig).unwrap();
    assert!(!json.contains("See https"), "the body is never stored");
}

#[test]
fn the_branch_is_the_last_main_thread_git_branch() {
    let jsonl = [
        r#"{"type":"user","gitBranch":"abc-1-x"}"#,
        r#"{"type":"assistant","gitBranch":"abc-2-y","isSidechain":true}"#,
        r#"{"type":"assistant","gitBranch":"abc-3-z"}"#,
        r#"{"type":"system"}"#,
        "not json \"gitBranch\"",
    ]
    .join("\n");
    assert_eq!(branch_from_jsonl(&jsonl).as_deref(), Some("abc-3-z"));
    assert_eq!(branch_from_jsonl(""), None);
}
