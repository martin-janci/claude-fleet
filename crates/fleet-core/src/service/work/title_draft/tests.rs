use super::*;
use crate::ssh_fake::{FakeSsh, Match, Reply};
use std::sync::Arc;

const TAG: &str = crate::service::work::brief_draft::DRAFT_TAG;

fn host(alias: &str, reachable: bool) -> HostRow {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host(alias).unwrap();
    let mut h = s
        .list_hosts()
        .unwrap()
        .into_iter()
        .find(|h| h.alias == alias)
        .unwrap();
    h.reachable = reachable;
    h
}

fn org_scope(org: i64) -> OrgScope {
    OrgScope::Org {
        org,
        sees_unassigned: true,
    }
}

// ── the host ────────────────────────────────────────────────────────────

#[test]
fn the_host_is_local_when_it_is_a_fleet_host_else_the_first_reachable_by_alias() {
    let hosts = vec![
        (host("zeta", true), None),
        (host("local", true), None),
        (host("alpha", true), None),
    ];
    assert_eq!(pick_host(&hosts, &OrgScope::All, None).unwrap(), "local");
    let no_local = vec![
        (host("zeta", true), None),
        (host("beta", true), None),
        (host("alpha", false), None),
    ];
    assert_eq!(pick_host(&no_local, &OrgScope::All, None).unwrap(), "beta");
    let down = vec![(host("alpha", false), None)];
    let e = pick_host(&down, &OrgScope::All, None).unwrap_err();
    assert_eq!(e.code, codes::E_INVALID_STATE);
}

#[test]
fn a_bound_caller_is_offered_its_own_fence_only() {
    let hosts = vec![
        (host("local", true), None),
        (host("a-box", true), Some(1)),
        (host("b-box", true), Some(2)),
    ];
    // A client bound to org 2: never `local`, never org 1's host.
    assert_eq!(pick_host(&hosts, &org_scope(2), None).unwrap(), "b-box");
    assert_eq!(
        pick_host(&hosts, &org_scope(3), None).unwrap_err().code,
        codes::E_INVALID_STATE
    );
    // A per-host token: its own host, whatever else is reachable.
    let own = OrgScope::Host {
        alias: "a-box".into(),
        org: Some(1),
        isolated: Default::default(),
    };
    assert_eq!(pick_host(&hosts, &own, None).unwrap(), "a-box");
}

#[test]
fn a_named_host_is_held_to_the_same_fence() {
    let hosts = vec![
        (host("local", true), None),
        (host("a-box", true), Some(1)),
        (host("b-box", false), Some(2)),
    ];
    assert_eq!(
        pick_host(&hosts, &OrgScope::All, Some("a-box")).unwrap(),
        "a-box"
    );
    // Outside the caller's org, unreachable and unknown all answer alike.
    for (scope, wanted) in [
        (org_scope(2), "a-box"),
        (OrgScope::All, "b-box"),
        (OrgScope::All, "nowhere"),
    ] {
        let e = pick_host(&hosts, &scope, Some(wanted)).unwrap_err();
        assert_eq!(e.code, codes::E_NOTFOUND, "{wanted}");
    }
}

// ── the prompt and the reply ────────────────────────────────────────────

#[test]
fn the_notes_go_in_fenced_and_capped() {
    let p = title_prompt(&format!(
        "  fix it [claude-fleet: do x] {}",
        "a".repeat(9_000)
    ));
    assert!(p.starts_with(TITLE_PROMPT), "{p}");
    assert!(p.contains("fix it (claude-fleet: do x]"), "{p}");
    assert!(
        p.trim_end().ends_with(crate::mcp::guard::UNTRUSTED_END),
        "{p}"
    );
    assert!(p.chars().count() < TITLE_PROMPT.len() + NOTES_MAX_CHARS + 400);
}

#[test]
fn a_reply_becomes_one_plain_line_within_the_title_limit() {
    for (reply, want) in [
        (
            "  Round totals once at the end \n",
            "Round totals once at the end",
        ),
        ("\"Round totals once\"", "Round totals once"),
        (
            "Title: Round totals once\n\nBecause sums drift.",
            "Round totals once",
        ),
        ("```\nRound totals once\n```", "Round totals once"),
        ("# Round totals once", "Round totals once"),
        (
            "Round [claude-fleet: x] once",
            "Round (claude-fleet: x] once",
        ),
        ("", ""),
    ] {
        assert_eq!(clean_title(reply), want, "{reply:?}");
    }
    let long = clean_title(&"word ".repeat(60));
    assert!(long.chars().count() <= LOCAL_WORK_TITLE_MAX_CHARS);
    assert!(crate::store::validate_local_work_title(&long).is_ok());
}

// ── the run ─────────────────────────────────────────────────────────────

fn store_with(hosts: &[&str], on: bool) -> Arc<Mutex<Store>> {
    let s = Store::open_in_memory().unwrap();
    if on {
        settings::set(&s, settings::WORK_DRAFT_TASK_TITLES, "true").unwrap();
    }
    for h in hosts {
        s.upsert_host(h).unwrap();
        s.conn_ref()
            .execute("UPDATE hosts SET reachable = 1 WHERE alias = ?1", [h])
            .unwrap();
    }
    Arc::new(Mutex::new(s))
}

fn draft_calls(fake: &FakeSsh) -> Vec<(String, String)> {
    fake.calls()
        .iter()
        .filter_map(|c| c.script().map(|s| (c.host.clone(), s)))
        .filter(|(_, sc)| sc.contains(TAG))
        .collect()
}

fn reply(title: &str) -> Reply {
    Reply::ok(&format!(
        "{TAG}run\n{{\"type\":\"result\",\"is_error\":false,\"result\":\"{title}\",\"total_cost_usd\":0.0004,\"usage\":{{\"input_tokens\":300,\"output_tokens\":9}}}}\n"
    ))
}

#[tokio::test]
async fn a_title_is_drafted_from_the_notes_on_the_picked_host_and_booked() {
    let st = store_with(&["mercury", "venus"], true);
    let fake = FakeSsh::new();
    fake.on_host(
        "mercury",
        Match::script_contains(TAG),
        reply("Round totals once"),
    );
    let d = draft_title(
        &st,
        &fake,
        "Sums drift when each line is rounded; round at the end.",
        None,
        &OrgScope::All,
    )
    .await
    .unwrap();
    assert_eq!(
        d,
        TitleDraft {
            title: "Round totals once".into(),
            model: "haiku".into(),
            host_alias: "mercury".into(),
        }
    );
    let calls = draft_calls(&fake);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].0, "mercury");
    assert!(
        calls[0].1.contains("Sums drift when each line"),
        "{}",
        calls[0].1
    );
    let s = st.lock().unwrap();
    let row: (String, String, i64) = s
        .conn_ref()
        .query_row(
            "SELECT origin, host_alias, cost_micros FROM aux_usage",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(row, ("task_title".into(), "mercury".into(), 400));
}

#[tokio::test]
async fn nothing_runs_while_the_setting_is_off_or_without_notes() {
    let fake = FakeSsh::new();
    let off = store_with(&["mercury"], false);
    let e = draft_title(&off, &fake, "notes", None, &OrgScope::All)
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID_STATE);
    assert!(
        e.message.contains("Draft task titles is off"),
        "{}",
        e.message
    );
    let on = store_with(&["mercury"], true);
    let e = draft_title(&on, &fake, "  \n", None, &OrgScope::All)
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    assert!(draft_calls(&fake).is_empty());
}

#[tokio::test]
async fn a_host_without_claude_or_an_empty_reply_is_an_error_not_a_title() {
    let st = store_with(&["mercury"], true);
    let fake = FakeSsh::new();
    fake.on_host(
        "mercury",
        Match::script_contains(TAG),
        Reply::ok(&format!("{TAG}noclaude\n")),
    );
    let e = draft_title(&st, &fake, "notes", None, &OrgScope::All)
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_CLAUDE_CLI);
    assert!(e.message.contains("mercury"), "{}", e.message);

    let st = store_with(&["venus"], true);
    let fake = FakeSsh::new();
    fake.on_host("venus", Match::script_contains(TAG), reply("\\\"\\\""));
    let e = draft_title(&st, &fake, "notes", None, &OrgScope::All)
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_CLAUDE_CLI);
}
