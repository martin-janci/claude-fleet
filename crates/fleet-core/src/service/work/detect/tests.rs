//! Detection end to end over an in-memory store: signals in, links out.

use super::*;
use crate::store::{Decider, StartSource, TrackerConfig, WorkTarget};

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

/// A tracker item with `key` under `tracker`; its id.
fn tracker_item(s: &Store, tracker: i64, ext: &str, key: &str) -> i64 {
    s.upsert_tracker_item(
        tracker,
        &crate::store::TrackerItemWrite {
            external_id: ext.into(),
            key: Some(key.into()),
            title: "Pay".into(),
            status_name: "To Do".into(),
            status_category: "todo".into(),
            ..Default::default()
        },
    )
    .unwrap()
    .id
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
    decide(&f.s, sid, id, false, Decider::Person).unwrap();
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
fn what_claude_code_submits_itself_is_not_evidence() {
    let f = fx();
    let sid = session(&f, "dev", "c1");
    let notification = "<task-notification>\n<task-id>afb11347d54b0e640</task-id>\n\
        <status>completed</status>\n<summary>Agent \"Fix ABC-9\" completed</summary>\n\
        <result>Opened https://acme.atlassian.net/browse/ABC-9</result>\n</task-notification>";
    assert_eq!(loop_guard(notification, None, &[]), Some("harness"));
    assert_eq!(
        loop_guard(
            "<system-reminder>x</system-reminder>\nContinue ABC-1: fix the login",
            Some("Continue ABC-1: fix the login"),
            &[]
        ),
        Some("fleet_sent"),
        "fleet's own prompt behind a harness head is still fleet's"
    );
    assert!(!on_prompt(&f.s, sid, notification, true).unwrap());
    assert!(!on_prompt(
        &f.s,
        sid,
        "<command-message>review</command-message>\n<command-name>/review</command-name>\n\
         <command-args>ABC-8</command-args>",
        true
    )
    .unwrap());
    assert!(f.s.session_work_links(sid).unwrap().is_empty());

    // A person's words after a harness head are read, and only they are.
    on_prompt(
        &f.s,
        sid,
        "<system-reminder>Also see ABC-1.</system-reminder>\nfix ABC-7, the retry bug",
        true,
    )
    .unwrap();
    let ls = f.s.session_work_links(sid).unwrap();
    assert_eq!(ls.len(), 1, "{ls:?}");
    let ev = &ls[0].evidence[0];
    assert_eq!(ev["text"], "ABC-7");
    assert!(!ev["snippet"].as_str().unwrap().contains("reminder"));
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
        let became = decide(&f.s, sid, sg.link_id, true, Decider::Person).unwrap();
        assert_eq!(became, i == 2, "trusted after the third");
    }
    assert!(trusted_projects(&f.s).contains(&f.project));
    let sid = session(&f, "s4", "c4");
    f.s.set_current_branch(sid, "abc-4-d").unwrap();
    resolve_session(&f.s, sid).unwrap();
    let w = f.s.get_session_by_id(sid).unwrap().unwrap().work.unwrap();
    assert_eq!(w.rule.as_deref(), Some("R3"));
}

/// Auto-trust counts only BRANCH suggestions a PERSON confirmed: three a
/// pull request alone proposed, or three an agent confirmed, trust nothing.
#[test]
fn only_a_persons_branch_confirmations_count_toward_trust() {
    let f = fx();
    for i in 0..3 {
        let name = format!("pr{i}");
        let sid = session(&f, &name, &format!("c{i}"));
        let sig = PrSignals {
            head: Some(format!("abc-{}-x", i + 1)),
            ..Default::default()
        };
        f.s.set_pr_signals("h", &name, Some(&serde_json::to_string(&sig).unwrap()))
            .unwrap();
        resolve_session(&f.s, sid).unwrap();
        let sg =
            f.s.get_session_by_id(sid)
                .unwrap()
                .unwrap()
                .work_suggested
                .unwrap();
        assert_eq!(sg.rule.as_deref(), Some("R3b"), "a sole PR head");
        assert!(!decide(&f.s, sid, sg.link_id, true, Decider::Person).unwrap());
    }
    for i in 3..6 {
        let sid = session(&f, &format!("ag{i}"), &format!("c{i}"));
        f.s.set_current_branch(sid, &format!("abc-{}-y", i + 1))
            .unwrap();
        resolve_session(&f.s, sid).unwrap();
        let sg =
            f.s.get_session_by_id(sid)
                .unwrap()
                .unwrap()
                .work_suggested
                .unwrap();
        assert!(!decide(&f.s, sid, sg.link_id, true, Decider::Agent).unwrap());
    }
    assert_eq!(f.s.confirmed_branch_suggestions(f.project).unwrap(), 0);
    assert!(!trusted_projects(&f.s).contains(&f.project));
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

/// D34: a suggestion detection takes back (R6 decay, R7 withdraw) loses its
/// row, but leaves a timeline event with ids and vocabulary words only —
/// the negative a label set would otherwise lose.
#[test]
fn a_withdrawn_or_decayed_suggestion_leaves_an_event_of_ids_only() {
    let withdrawn = |s: &Store, sid: i64| -> Vec<serde_json::Value> {
        s.list_session_events(sid, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == crate::store::WORK_SUGGESTION_WITHDRAWN)
            .map(|e| serde_json::from_str(e.detail.as_deref().unwrap()).unwrap())
            .collect()
    };
    // Decay: ABC-8, mentioned in c1, not again in c2.
    let f = fx();
    let sid = session(&f, "dev", "c1");
    on_prompt(&f.s, sid, "first task", true).unwrap();
    on_prompt(&f.s, sid, "also ABC-8 and ABC-9", false).unwrap();
    let abc8 =
        f.s.session_work_links(sid)
            .unwrap()
            .into_iter()
            .find(|l| l.ref_key.as_deref() == Some("ABC-8"))
            .unwrap();
    assert!(withdrawn(&f.s, sid).is_empty(), "nothing taken back yet");
    f.s.rebind_conversation(sid, "c2", StartSource::Clear, None, None)
        .unwrap();
    on_prompt(&f.s, sid, "keep going on ABC-9", true).unwrap();
    let ev = withdrawn(&f.s, sid);
    assert_eq!(
        ev,
        vec![serde_json::json!({
            "link_id": abc8.id, "item_id": null, "rule": "R6", "reason": "decay"
        })]
    );
    let event = f.s.list_session_events(sid, 100).unwrap();
    let e = event
        .iter()
        .find(|e| e.kind == crate::store::WORK_SUGGESTION_WITHDRAWN)
        .unwrap();
    assert_eq!(e.claude_session_id.as_deref(), Some("c2"));
    assert!(!e.detail.as_deref().unwrap().contains("ABC"), "no key");

    // Withdraw: the PR's state and text suggestions go with the PR.
    let sid = session(&f, "pr", "c3");
    let sig = PrSignals {
        head: Some("feature/login".into()),
        closing: vec!["acme/api#42".into()],
        text: vec!["ABC-5".into()],
        trailers: vec![],
        state: None,
    };
    f.s.set_pr_signals("h", "pr", Some(&serde_json::to_string(&sig).unwrap()))
        .unwrap();
    resolve_session(&f.s, sid).unwrap();
    f.s.set_pr_signals("h", "pr", None).unwrap();
    resolve_session(&f.s, sid).unwrap();
    let mut rules: Vec<(String, String)> = withdrawn(&f.s, sid)
        .into_iter()
        .map(|v| {
            assert_eq!(
                v.as_object().unwrap().keys().collect::<Vec<_>>(),
                vec!["item_id", "link_id", "reason", "rule"]
            );
            (
                v["rule"].as_str().unwrap().to_string(),
                v["reason"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    rules.sort();
    assert_eq!(
        rules,
        vec![
            ("R3u".to_string(), "withdraw".to_string()),
            ("R6".to_string(), "withdraw".to_string())
        ]
    );
    assert!(f.s.session_work_links(sid).unwrap().is_empty());
}

/// D34: a carry (resume, fork, inherit) settles a live suggestion of the
/// same work; its row goes, but it leaves the same ids-only event, reason
/// `carried`, on the session that gained the carried link.
#[test]
fn a_suggestion_a_carry_settles_leaves_a_carried_event() {
    let carried = |s: &Store, sid: i64| -> Vec<serde_json::Value> {
        s.list_session_events(sid, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == crate::store::WORK_SUGGESTION_WITHDRAWN)
            .map(|e| {
                // Tagged with the session's current conversation.
                assert!(e.claude_session_id.is_some());
                serde_json::from_str(e.detail.as_deref().unwrap()).unwrap()
            })
            .collect()
    };
    let suggestion = |s: &Store, sid: i64, key: &str| -> i64 {
        on_prompt(s, sid, "first task", true).unwrap();
        on_prompt(s, sid, &format!("see {key} too"), false).unwrap();
        let l = s.session_work_links(sid).unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(
            (l[0].state.as_str(), l[0].rule.as_deref()),
            ("suggested", Some("R6"))
        );
        l[0].id
    };
    let f = fx();

    // A resume carries ABC-7 onto a session that only had it suggested.
    let sid = session(&f, "dev", "c1");
    let id = suggestion(&f.s, sid, "ABC-7");
    assert!(f.s.link_resumed_work(sid, "ABC-7").unwrap());
    let l = f.s.session_work_links(sid).unwrap();
    assert_eq!(
        l.iter()
            .map(|l| (l.state.as_str(), l.source.as_str()))
            .collect::<Vec<_>>(),
        vec![("confirmed", "resumed")]
    );
    assert_eq!(
        carried(&f.s, sid),
        vec![serde_json::json!({
            "link_id": id, "item_id": null, "rule": "R6", "reason": "carried"
        })]
    );

    // A fork carries its source's work over the target's suggestion.
    let target = session(&f, "fork", "c2");
    let id = suggestion(&f.s, target, "ABC-7");
    assert_eq!(f.s.copy_work_links(sid, target).unwrap(), 1);
    assert_eq!(
        carried(&f.s, target),
        vec![serde_json::json!({
            "link_id": id, "item_id": null, "rule": "R6", "reason": "carried"
        })]
    );
    // A carry that finds no suggestion records nothing.
    let other = session(&f, "other", "c3");
    assert_eq!(f.s.copy_work_links(sid, other).unwrap(), 1);
    assert!(carried(&f.s, other).is_empty());
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
        state: None,
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

/// R3u per GitHub instance (M11.4): an enterprise tracker does not make
/// github.com's `owner/repo#n` resolvable, nor the other way round — each
/// stays a pre-selected suggestion until a tracker of ITS instance exists.
#[test]
fn a_closing_ref_is_untracked_until_a_tracker_of_its_own_instance_exists() {
    for (tracker_site, closing, untracked) in [
        ("https://ghe.corp.example/acme", "acme/api#42", true),
        (
            "https://github.com/acme",
            "ghe.corp.example/acme/api#42",
            true,
        ),
        (
            "https://ghe.other.example/acme",
            "ghe.corp.example/acme/api#42",
            true,
        ),
        ("https://github.com/acme", "acme/api#42", false),
        (
            "https://ghe.corp.example/acme",
            "ghe.corp.example/acme/api#42",
            false,
        ),
    ] {
        let f = fx();
        trust(&f);
        f.s.add_tracker("github", "gh", tracker_site).unwrap();
        let sid = session(&f, "dev", "c1");
        f.s.set_pr_signals(
            "h",
            "dev",
            Some(&serde_json::json!({ "closing": [closing] }).to_string()),
        )
        .unwrap();
        resolve_session(&f.s, sid).unwrap();
        let row = f.s.get_session_by_id(sid).unwrap().unwrap();
        let rule = row
            .work
            .as_ref()
            .and_then(|w| w.rule.clone())
            .or(row.work_suggested.as_ref().and_then(|s| s.rule.clone()));
        assert_eq!(
            rule.as_deref() == Some("R3u"),
            untracked,
            "{tracker_site} / {closing}: {rule:?}"
        );
    }
}

#[test]
fn an_enterprise_closing_ref_carries_its_host() {
    let v = serde_json::json!({
        "headRefName": "fix",
        "closingIssuesReferences": [
            {"number": 42, "url": "https://GHE.corp.example/Acme/API/issues/42",
             "repository": {"name": "API", "owner": {"login": "Acme"}}},
            {"number": 7, "url": "https://github.com/acme/web/issues/7",
             "repository": {"name": "web", "owner": {"login": "acme"}}},
            {"number": 8, "url": "https://127.0.0.1/acme/web/issues/8",
             "repository": {"name": "web", "owner": {"login": "acme"}}}
        ]
    });
    let sig = PrSignals::from_gh_json(&v);
    assert_eq!(
        sig.closing,
        vec!["ghe.corp.example/acme/api#42", "acme/web#7", "acme/web#8"]
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

/// Work graph M5: a state candidate that crosses orgs is neither created
/// nor promoted — and the Primary change that follows it must not demote
/// the primary the session already has (a manual link from an earlier
/// conversation), or the session would be left with no work at all.
#[test]
fn a_cross_org_branch_candidate_leaves_the_existing_primary_alone() {
    let f = fx();
    trust(&f);
    let a = f.s.add_org("A", None, false).unwrap();
    let b = f.s.add_org("B", None, false).unwrap();
    f.s.set_host_org("h", Some(a.id)).unwrap();
    let other =
        f.s.add_tracker("jira", "Other", "https://other.atlassian.net")
            .unwrap();
    f.s.set_tracker_probe(
        other.id,
        None,
        &TrackerConfig {
            key_prefixes: vec!["XYZ".into()],
            ..Default::default()
        },
    )
    .unwrap();
    f.s.set_tracker_org(other.id, Some(b.id)).unwrap();
    tracker_item(&f.s, other.id, "9", "XYZ-9");

    let sid = session(&f, "dev", "c1");
    f.s.link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
        .unwrap();
    f.s.rebind_conversation(sid, "c2", StartSource::Clear, None, None)
        .unwrap();
    assert!(f.s.set_current_branch(sid, "xyz-9-fix").unwrap());
    resolve_session(&f.s, sid).unwrap();

    let row = f.s.get_session_by_id(sid).unwrap().unwrap();
    assert_eq!(
        row.work.as_ref().and_then(|w| w.key.as_deref()),
        Some("ABC-1"),
        "the manual primary survives a skipped cross-org candidate: {row:?}"
    );
    assert_eq!(
        links(&f.s, sid),
        vec![("ABC-1".into(), "confirmed".into(), "manual".into(), None)],
        "another org's item is never linked or suggested here"
    );
}

/// A prompt suggestion the branch promotes becomes the branch's link: when
/// the branch moves on, R7 ends it like any other auto link, instead of
/// leaving a confirmed 'prompt' link the resolver never retires.
#[test]
fn a_promoted_suggestion_takes_the_promoting_signals_source_so_r7_ends_it() {
    let f = fx();
    trust(&f);
    let sid = session(&f, "dev", "c1");
    on_prompt(&f.s, sid, "see ABC-1", true).unwrap();
    assert_eq!(
        links(&f.s, sid),
        vec![(
            "ABC-1".into(),
            "suggested".into(),
            "prompt".into(),
            Some("R5".into())
        )]
    );
    assert!(f.s.set_current_branch(sid, "abc-1-fix").unwrap());
    resolve_session(&f.s, sid).unwrap();
    assert_eq!(
        links(&f.s, sid),
        vec![(
            "ABC-1".into(),
            "confirmed".into(),
            "branch".into(),
            Some("R3".into())
        )],
        "promoted by the branch: a branch link"
    );
    let row = f.s.get_session_by_id(sid).unwrap().unwrap();
    assert_eq!(row.work.unwrap().key.as_deref(), Some("ABC-1"));

    assert!(f.s.set_current_branch(sid, "abc-2-other").unwrap());
    resolve_session(&f.s, sid).unwrap();
    let live: Vec<String> = links(&f.s, sid).into_iter().map(|l| l.0).collect();
    assert_eq!(
        live,
        vec!["ABC-2".to_string()],
        "ABC-1 ended with the branch"
    );
    let ended = f.s.ended_work_links_for_key("ABC-1").unwrap();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].end_reason.as_deref(), Some("branch_changed"));
    let row = f.s.get_session_by_id(sid).unwrap().unwrap();
    assert_eq!(row.work.unwrap().key.as_deref(), Some("ABC-2"));
}

/// A moved issue (ABC-1 became NEW-1; ABC-1 is an alias) is one target:
/// a branch still named after the alias links it once, under the item's
/// current key, and every later run finds that link instead of adding
/// another; a rejection made under the current key blocks the alias
/// candidate (R9).
#[test]
fn an_alias_candidate_meets_the_link_and_the_rejection_of_the_items_current_key() {
    let f = fx();
    trust(&f);
    let tracker = f.s.list_trackers().unwrap()[0].id;
    f.s.set_tracker_probe(
        tracker,
        None,
        &TrackerConfig {
            key_prefixes: vec!["ABC".into(), "NEW".into()],
            ..Default::default()
        },
    )
    .unwrap();
    let item = tracker_item(&f.s, tracker, "77", "ABC-1");
    assert_eq!(
        tracker_item(&f.s, tracker, "77", "NEW-1"),
        item,
        "the key moved; the item stayed"
    );
    assert_eq!(
        f.s.get_work_item(item).unwrap().unwrap().aliases,
        vec!["ABC-1".to_string()]
    );

    let sid = session(&f, "dev", "c1");
    assert!(f.s.set_current_branch(sid, "abc-1-fix").unwrap());
    assert!(resolve_session(&f.s, sid).unwrap());
    let want = vec![(
        "NEW-1".to_string(),
        "confirmed".to_string(),
        "branch".to_string(),
        Some("R3".to_string()),
    )];
    assert_eq!(links(&f.s, sid), want, "linked once, under the current key");
    let ls = f.s.session_work_links(sid).unwrap();
    assert_eq!(
        (ls[0].item_id, ls[0].ref_key.as_deref()),
        (Some(item), Some("NEW-1"))
    );
    // The next Stop: the same branch, the same link — nothing to add.
    assert!(!resolve_session(&f.s, sid).unwrap(), "idempotent");
    on_prompt(&f.s, sid, "still on ABC-1", false).unwrap();
    assert_eq!(
        links(&f.s, sid),
        want,
        "the alias in a prompt is the same link"
    );

    // Rejected under the current key: the alias candidate is never proposed.
    let other = session(&f, "other", "c2");
    f.s.reject_session_work(other, WorkTarget::Key("NEW-1"))
        .unwrap();
    f.s.set_current_branch(other, "abc-1-fix").unwrap();
    resolve_session(&f.s, other).unwrap();
    on_prompt(&f.s, other, "look at ABC-1", true).unwrap();
    assert_eq!(
        links(&f.s, other),
        vec![("NEW-1".into(), "rejected".into(), "manual".into(), None)]
    );
    assert_eq!(
        f.s.get_session_by_id(other)
            .unwrap()
            .unwrap()
            .work_suggested,
        None
    );
}

/// A PR description that lists tickets (a release PR, an audit) names none
/// of them as the session's work: past the dump guard its text proposes
/// nothing, and what an earlier, shorter text proposed is withdrawn.
#[test]
fn a_pr_text_listing_tickets_proposes_none_of_them() {
    let f = fx();
    let sid = session(&f, "dev", "c1");
    let probe = |text: &[&str]| {
        let sig = PrSignals {
            text: text.iter().map(|t| t.to_string()).collect(),
            ..Default::default()
        };
        f.s.set_pr_signals("h", "dev", Some(&serde_json::to_string(&sig).unwrap()))
            .unwrap();
        resolve_session(&f.s, sid).unwrap();
    };
    probe(&["ABC-1"]);
    assert_eq!(
        links(&f.s, sid).len(),
        1,
        "one key in a PR text is a suggestion"
    );
    probe(&["ABC-1", "ABC-2", "ABC-3", "ABC-4"]);
    assert!(
        links(&f.s, sid).is_empty(),
        "a list is a reference, not a suggestion: {:?}",
        links(&f.s, sid)
    );
    assert_eq!(
        f.s.get_session_by_id(sid).unwrap().unwrap().work_suggested,
        None
    );
}

/// R7's snapshot of a link whose only state evidence is a PR closing ref
/// records the worktree's branch, not the closing ref's text (a ticket
/// key) — the handover brief and the resume button read `snap_branch` as
/// a git branch.
#[test]
fn ending_a_closing_ref_link_snapshots_the_branch_not_the_ticket_key() {
    let f = fx();
    trust(&f);
    let wt =
        f.s.upsert_worktree(f.project, "wt", "/src/api/wt", Some("fix-login"))
            .unwrap();
    let sid =
        f.s.upsert_session("dev", "h", Some(f.project), Some(wt), 1, 1, "running", None)
            .unwrap();
    f.s.rebind_conversation(sid, "c1", StartSource::Startup, None, None)
        .unwrap();
    f.s.set_pr_signals("h", "dev", Some(r#"{"closing":["ABC-1"]}"#))
        .unwrap();
    resolve_session(&f.s, sid).unwrap();
    assert_eq!(
        links(&f.s, sid),
        vec![(
            "ABC-1".into(),
            "confirmed".into(),
            "pr".into(),
            Some("R3".into())
        )]
    );
    // The PR is gone (closed or re-targeted): the link ends.
    f.s.set_pr_signals("h", "dev", None).unwrap();
    resolve_session(&f.s, sid).unwrap();
    assert!(links(&f.s, sid).is_empty());
    let ended = f.s.ended_work_links_for_key("ABC-1").unwrap();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].end_reason.as_deref(), Some("pr_changed"));
    assert_eq!(
        ended[0].snap_branch.as_deref(),
        Some("fix-login"),
        "the worktree's branch, never the closing ref"
    );
}

/// Once a session has confirmed work, a weak mention (a prompt key, a PR
/// text key, a trailer) no longer asks for a decision: deciding one
/// suggestion must not bring up the next mention. A strong suggestion (the
/// branch or PR moved on) still does.
#[test]
fn a_session_with_confirmed_work_surfaces_only_strong_suggestions() {
    let f = fx();
    let sid = session(&f, "dev", "c1");
    on_prompt(&f.s, sid, "see ABC-1, ABC-2 and ABC-3", false).unwrap();
    let row = f.s.get_session_by_id(sid).unwrap().unwrap();
    let sg = row
        .work_suggested
        .expect("weak suggestions show while no work is confirmed");
    assert_eq!(sg.suggestions, 3);
    decide(&f.s, sid, sg.link_id, true, Decider::Person).unwrap();
    let row = f.s.get_session_by_id(sid).unwrap().unwrap();
    assert!(row.work.is_some());
    assert_eq!(row.work_suggested, None, "the other mentions stay quiet");
    assert_eq!(row.work.unwrap().suggestions, 0);

    f.s.set_current_branch(sid, "abc-7-retry").unwrap();
    resolve_session(&f.s, sid).unwrap();
    let sg = f.s.get_session_by_id(sid).unwrap().unwrap().work_suggested;
    let sg = sg.expect("a strong suggestion still asks");
    assert_eq!((sg.key.as_deref(), sg.suggestions), (Some("ABC-7"), 1));
}

// ── R9u: a person's "Clear work" holds against the unchanged state ──────

/// `work_link { action: unlink }` as `decider`, through the service entry
/// the MCP tool and the desktop share; the session's row after it.
fn unlink_as(
    st: &std::sync::Mutex<Store>,
    sid: i64,
    link_id: i64,
    decider: Decider,
) -> crate::store::SessionRow {
    crate::service::work::work_link_as(
        &crate::service::work::WorkLinkArgs {
            action: "unlink".into(),
            session_id: Some(sid),
            link_id: Some(link_id),
            ..Default::default()
        },
        st,
        &crate::service::orgs::OrgScope::All,
        decider,
    )
    .unwrap()
}

fn live(st: &std::sync::Mutex<Store>, sid: i64) -> Vec<(String, String, String, Option<String>)> {
    links(&st.lock().unwrap(), sid)
}

fn holds(st: &std::sync::Mutex<Store>) -> i64 {
    st.lock()
        .unwrap()
        .conn_for_test()
        .query_row("SELECT COUNT(*) FROM work_unlinks", [], |r| r.get(0))
        .unwrap()
}

/// D34 / R9u: in a trusted project, a person clearing the link a branch
/// made (R3) is not undone by the next resolve while the branch is the
/// same; another branch detects normally; the same branch again stays
/// cleared; a branch of the same key but another name links again. An
/// agent's unlink writes no hold, so the branch links it again. Prompt
/// events are never held, and the hold goes with the participant.
#[test]
fn clearing_a_branch_link_holds_while_the_branch_is_unchanged() {
    let f = fx();
    trust(&f);
    let sid = session(&f, "dev", "c1");
    f.s.set_current_branch(sid, "abc-1-login").unwrap();
    resolve_session(&f.s, sid).unwrap();
    let link = f.s.session_work_links(sid).unwrap()[0].clone();
    assert_eq!(
        (
            link.state.as_str(),
            link.source.as_str(),
            link.rule.as_deref()
        ),
        ("confirmed", "branch", Some("R3"))
    );
    let st = std::sync::Mutex::new(f.s);

    // The person clears it: gone, and the decision's own re-resolve did
    // not make it again.
    let row = unlink_as(&st, sid, link.id, Decider::Person);
    assert_eq!(row.work, None);
    assert!(live(&st, sid).is_empty());
    assert_eq!(holds(&st), 1);
    // Nor does a later run on the same branch — not even as a suggestion.
    resolve_session(&st.lock().unwrap(), sid).unwrap();
    assert!(live(&st, sid).is_empty());
    let row = st.lock().unwrap().get_session_by_id(sid).unwrap().unwrap();
    assert_eq!(row.work_suggested, None);

    // Another branch: normal detection.
    {
        let s = st.lock().unwrap();
        s.set_current_branch(sid, "abc-2-other").unwrap();
        resolve_session(&s, sid).unwrap();
    }
    assert_eq!(
        live(&st, sid),
        vec![(
            "ABC-2".into(),
            "confirmed".into(),
            "branch".into(),
            Some("R3".into())
        )]
    );
    // Back to the cleared branch: ABC-2's automatic link ends (R7), and
    // ABC-1 stays cleared — the same value is held.
    {
        let s = st.lock().unwrap();
        s.set_current_branch(sid, "abc-1-login").unwrap();
        resolve_session(&s, sid).unwrap();
    }
    assert!(live(&st, sid).is_empty(), "{:?}", live(&st, sid));
    // A branch of the same key under another name is another value.
    {
        let s = st.lock().unwrap();
        s.set_current_branch(sid, "abc-1-login-v2").unwrap();
        resolve_session(&s, sid).unwrap();
    }
    let again = st.lock().unwrap().session_work_links(sid).unwrap();
    assert_eq!(
        again
            .iter()
            .map(|l| (l.ref_key.as_deref(), l.state.as_str(), l.source.as_str()))
            .collect::<Vec<_>>(),
        vec![(Some("ABC-1"), "confirmed", "branch")]
    );

    // An agent's unlink stays a plain unlink: no hold, and the unchanged
    // branch links it again at once.
    let row = unlink_as(&st, sid, again[0].id, Decider::Agent);
    assert_eq!(holds(&st), 1, "an agent writes no hold");
    assert_eq!(
        row.work.and_then(|w| w.key).as_deref(),
        Some("ABC-1"),
        "relinked by the branch"
    );

    // Event candidates are never held: a prompt naming ABC-1 on the held
    // branch still proposes it.
    {
        let s = st.lock().unwrap();
        s.set_current_branch(sid, "abc-1-login").unwrap();
        resolve_session(&s, sid).unwrap();
        assert!(links(&s, sid).is_empty(), "the -v2 link ended (R7)");
        on_prompt(&s, sid, "see ABC-1 again", false).unwrap();
    }
    assert_eq!(
        live(&st, sid),
        vec![(
            "ABC-1".into(),
            "suggested".into(),
            "prompt".into(),
            Some("R6".into())
        )]
    );

    // The hold goes when the participant retires.
    {
        let s = st.lock().unwrap();
        let p = s.participant_for_session(sid).unwrap().unwrap().id;
        s.retire_participant(p).unwrap();
    }
    assert_eq!(holds(&st), 0);
}

/// R9u in an untrusted project: the branch's suggestion (R3b), confirmed
/// by the person and then cleared, is not proposed again from that branch.
#[test]
fn clearing_a_confirmed_branch_suggestion_does_not_bring_it_back() {
    let f = fx();
    let sid = session(&f, "dev", "c1");
    f.s.set_current_branch(sid, "abc-3-fix").unwrap();
    resolve_session(&f.s, sid).unwrap();
    let sg = f.s.session_work_links(sid).unwrap()[0].clone();
    assert_eq!(
        (sg.state.as_str(), sg.rule.as_deref()),
        ("suggested", Some("R3b"))
    );
    decide(&f.s, sid, sg.id, true, Decider::Person).unwrap();
    let st = std::sync::Mutex::new(f.s);
    unlink_as(&st, sid, sg.id, Decider::Person);
    assert!(live(&st, sid).is_empty(), "{:?}", live(&st, sid));
    resolve_session(&st.lock().unwrap(), sid).unwrap();
    assert!(live(&st, sid).is_empty());
}

/// R9u for a pull request: a link its closing reference made is held while
/// it is the same PR; another PR closing the same ticket links it again.
/// Clearing a link no state signal names (a person's own) holds nothing.
#[test]
fn clearing_a_pr_link_holds_for_that_pr_only() {
    let f = fx();
    trust(&f);
    let sid = session(&f, "dev", "c1");
    let pr = |s: &Store, url: &str| {
        s.conn_for_test()
            .execute(
                "UPDATE sessions SET pr_url = ?1 WHERE id = ?2",
                rusqlite::params![url, sid],
            )
            .unwrap();
        s.set_pr_signals("h", "dev", Some(r#"{"closing":["ABC-5"]}"#))
            .unwrap();
        resolve_session(s, sid).unwrap();
    };
    pr(&f.s, "https://github.com/acme/api/pull/1");
    let link = f.s.session_work_links(sid).unwrap()[0].clone();
    assert_eq!(
        (link.source.as_str(), link.rule.as_deref()),
        ("pr", Some("R3"))
    );
    let project = f.project;
    let st = std::sync::Mutex::new(f.s);
    unlink_as(&st, sid, link.id, Decider::Person);
    assert!(live(&st, sid).is_empty());
    let (signal, value): (String, String) = st
        .lock()
        .unwrap()
        .conn_for_test()
        .query_row("SELECT signal, value FROM work_unlinks", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(
        (signal.as_str(), value.as_str()),
        ("pr", "https://github.com/acme/api/pull/1")
    );
    resolve_session(&st.lock().unwrap(), sid).unwrap();
    assert!(live(&st, sid).is_empty(), "the same PR stays cleared");
    // Another PR closing the same ticket is another value.
    pr(&st.lock().unwrap(), "https://github.com/acme/api/pull/2");
    assert_eq!(
        live(&st, sid),
        vec![(
            "ABC-5".into(),
            "confirmed".into(),
            "pr".into(),
            Some("R3".into())
        )]
    );

    // A person's own link on a session no signal names: a plain unlink.
    let other = {
        let s = st.lock().unwrap();
        let id = s
            .upsert_session("other", "h", Some(project), None, 1, 1, "running", None)
            .unwrap();
        s.link_session_work(id, WorkTarget::Key("ABC-9"), "manual")
            .unwrap()
            .id
    };
    let other_sid = st
        .lock()
        .unwrap()
        .get_session("other", "h")
        .unwrap()
        .unwrap()
        .id;
    unlink_as(&st, other_sid, other, Decider::Person);
    assert!(live(&st, other_sid).is_empty());
    assert_eq!(holds(&st), 1, "only the PR's");
}
