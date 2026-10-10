//! LLM drafts in Control (redesign 9.11): what each prompt carries, where a
//! brief runs, that the brief is drafted only on Refresh, and that every
//! run is booked with its origin.

use super::*;
use crate::cancel::CancellationRegistry;
use crate::mcp::auth::{Caller, ClientRef, TokenMode};
use crate::net::https::FakeTransport;
use crate::service::trackers::TrackerNet;
use crate::service::work::missions::{self, MissionInput};
use crate::service::work::today::TodayShipped;
use std::sync::Arc;

fn person(store: &Mutex<Store>, id: i64) -> ViewScope {
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
    }
    .view_scope(&lock(store).unwrap())
    .unwrap()
}

/// A mission of a new person's, and the deps around its store.
fn fixture(name: &str) -> (Deps, ViewScope, MissionRow) {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let who = lock(&store).unwrap().create_person(name, None).unwrap().id;
    let me = person(&store, who);
    let m = missions::save(
        &WorkLinkArgs {
            action: "mission_save".into(),
            mission: Some(MissionInput {
                name: Some("Ship login".into()),
                goal: Some("Passwordless login".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        &store,
        &me,
    )
    .unwrap();
    let deps = Deps {
        store,
        ssh: Arc::new(crate::ssh::SshClient::new()),
        reg: CancellationRegistry::new(),
        net: TrackerNet::fake(Arc::new(FakeTransport::new())),
    };
    (deps, me, m)
}

fn note_args(id: i64) -> WorkLinkArgs {
    WorkLinkArgs {
        action: "mission_release_note".into(),
        mission_id: Some(id),
        ..Default::default()
    }
}

#[test]
fn a_release_note_prompt_carries_the_missions_facts_only() {
    let (prompt, from) = release_note_prompt(&NoteFacts {
        name: "Ship login".into(),
        goal: "Passwordless login".into(),
        items: vec![
            ("PD-1".into(), "Magic link".into(), "done".into()),
            ("-".into(), "Rate limit".into(), "done".into()),
        ],
        prs: vec!["https://github.com/o/r/pull/7".into()],
    });
    assert!(prompt.starts_with(RELEASE_NOTE_PROMPT), "{prompt}");
    for want in [
        "Mission: Ship login",
        "Goal: Passwordless login",
        "- PD-1 Magic link (done)",
        "- - Rate limit (done)",
        "- https://github.com/o/r/pull/7",
    ] {
        assert!(prompt.contains(want), "{want} in {prompt}");
    }
    assert_eq!(from, "2 tasks and 1 PR");
}

#[test]
fn a_draft_is_redacted_and_capped() {
    let (text, cut) = clean_draft("  token ghp_abcdefghijklmnopqrstuvwxyz0123456789 here  ");
    assert!(!text.contains("ghp_abcdef"), "{text}");
    assert!(!cut);
    let (text, cut) = clean_draft(&"x".repeat(DRAFT_MAX_CHARS + 5));
    assert_eq!((text.chars().count(), cut), (DRAFT_MAX_CHARS, true));
}

fn session(id: i64, host: &str, org: Option<i64>, at: i64) -> TodaySession {
    TodaySession {
        id,
        name: format!("s{id}"),
        host_alias: host.into(),
        org_id: org,
        last_activity_at: at,
        ..Default::default()
    }
}

fn digest() -> Today {
    Today {
        since: 0,
        now: 100,
        groups: vec![
            TodayGroup {
                bucket: "waiting".into(),
                key: Some("PD-1".into()),
                title: "Magic link".into(),
                org_id: Some(1),
                sessions: vec![TodaySession {
                    attention: Some("blocked".into()),
                    ..session(1, "mercury", Some(1), 50)
                }],
                ..Default::default()
            },
            TodayGroup {
                bucket: BUCKET_IN_PROGRESS.into(),
                key: Some("AC-9".into()),
                title: "Another org's work".into(),
                org_id: Some(2),
                sessions: vec![session(2, "venus", Some(2), 90)],
                ..Default::default()
            },
        ],
        shipped: vec![TodayShipped {
            how: "done".into(),
            key: Some("PD-0".into()),
            title: "Signup".into(),
            at: 10,
            org_id: Some(1),
            ..Default::default()
        }],
        missions: vec![],
    }
}

/// A brief covers one org, so another org's work never reaches the host it
/// runs on.
#[test]
fn a_brief_covers_one_org_and_runs_on_its_latest_sessions_host() {
    let today = digest();
    assert_eq!(
        brief_target(&today, None, |_, _| true),
        Some((Some(2), "venus".to_string())),
        "the most recently active session decides"
    );
    assert_eq!(
        brief_target(&today, Some(1), |_, _| true),
        Some((Some(1), "mercury".to_string()))
    );
    assert_eq!(brief_target(&today, Some(3), |_, _| true), None);
    let (prompt, from) = brief_prompt(&today, Some(1)).unwrap();
    assert!(prompt.starts_with(BRIEF_PROMPT));
    assert!(
        prompt.contains("waiting:\n- PD-1 Magic link [-] sessions: s1 (blocked)"),
        "{prompt}"
    );
    assert!(
        prompt.contains("shipped:\n- PD-0 Signup (done)"),
        "{prompt}"
    );
    assert!(!prompt.contains("AC-9"), "another org's work: {prompt}");
    assert_eq!(from, "1 item and 1 shipped");
    assert!(brief_prompt(&today, Some(3)).is_none());
}

/// Review r15: the brief's org and its prompt follow one rule (a session
/// counts for its own org, else its group's). The no-work group mixes orgs:
/// a brief carries only the sessions of its own org from it, so the org
/// `brief_target` picks from a session there finds that session in its
/// prompt, and another org's session never rides along.
#[test]
fn a_brief_takes_only_its_own_orgs_sessions_from_a_mixed_group() {
    let mut today = digest();
    today.groups.push(TodayGroup {
        bucket: BUCKET_IN_PROGRESS.into(),
        key: None,
        title: "No work".into(),
        org_id: None,
        sessions: vec![
            session(3, "venus", Some(2), 20),
            session(4, "mercury", Some(1), 95),
        ],
        ..Default::default()
    });
    assert_eq!(
        brief_target(&today, None, |_, _| true),
        Some((Some(1), "mercury".to_string()))
    );
    let (one, _) = brief_prompt(&today, Some(1)).unwrap();
    assert!(one.contains("sessions: s4"), "{one}");
    assert!(!one.contains("s3"), "another org's session: {one}");
    let (two, _) = brief_prompt(&today, Some(2)).unwrap();
    assert!(two.contains("sessions: s3"), "{two}");
    assert!(!two.contains("s4"), "another org's session: {two}");
}

/// The plan's 9.11 check: opening Today never drafts. Without `refresh` the
/// brief answers what was drafted last (nothing yet), and touches no host.
#[tokio::test]
async fn the_brief_regenerates_only_on_refresh() {
    let (deps, me, _) = fixture("brief-reader");
    let args = WorkLinkArgs {
        action: "today_brief".into(),
        ..Default::default()
    };
    let b = brief(&args, &deps, &me).await.unwrap();
    assert_eq!(b, Brief::default(), "nothing drafted, nothing run");
    let booked = |deps: &Deps| -> i64 {
        lock(&deps.store)
            .unwrap()
            .conn_for_test()
            .query_row("SELECT COUNT(*) FROM aux_usage", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(booked(&deps), 0);
    // Refresh with no session today: no host to run on, and still nothing
    // booked.
    let refresh = WorkLinkArgs {
        refresh: Some(true),
        ..args
    };
    let err = brief(&refresh, &deps, &me).await.unwrap_err();
    assert_eq!(err.code, codes::E_INVALID_STATE, "{}", err.message);
    assert_eq!(booked(&deps), 0);
}

#[tokio::test]
async fn a_release_note_is_for_a_completed_mission() {
    let (deps, me, m) = fixture("note-owner");
    let err = release_note(&note_args(m.id), &deps, &me)
        .await
        .unwrap_err();
    assert_eq!(err.code, codes::E_INVALID_STATE);
    assert!(err.message.contains("completed mission"), "{}", err.message);
    {
        let s = lock(&deps.store).unwrap();
        s.set_mission_state(m.id, None, "active", "t").unwrap();
        s.set_mission_state(m.id, None, "completed", "t").unwrap();
    }
    // Completed, but no worker ever ran: no host for it.
    let err = release_note(&note_args(m.id), &deps, &me)
        .await
        .unwrap_err();
    assert_eq!(err.code, codes::E_INVALID_STATE);
    assert!(err.message.contains("no host"), "{}", err.message);
}

/// The plan's 9.11 check: a run is booked with its origin, on its mission
/// for a release note and on its org for a brief.
#[test]
fn every_draft_is_booked_with_its_origin() {
    let (deps, _, m) = fixture("booker");
    let raw = r#"{"type":"result","subtype":"success","is_error":false,"result":"Note","total_cost_usd":0.002,"usage":{"input_tokens":300,"output_tokens":20}}"#;
    let (_, usage) = planner::planner_answer(raw.to_string());
    book(
        &deps,
        &Run {
            origin: crate::store::AUX_ORIGIN_RELEASE_NOTE,
            host: "mercury",
            model: "haiku",
            mission_id: Some(m.id),
            org_id: None,
        },
        usage.as_ref(),
    );
    book(
        &deps,
        &Run {
            origin: crate::store::AUX_ORIGIN_MORNING_BRIEF,
            host: "venus",
            model: "haiku",
            mission_id: None,
            org_id: None,
        },
        None,
    );
    let s = lock(&deps.store).unwrap();
    let rows = s.mission_aux_usage(m.id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (
            rows[0].origin.as_str(),
            rows[0].host_alias.as_str(),
            rows[0].cost_micros
        ),
        ("release_note", "mercury", 2_000)
    );
    let briefs: i64 = s
        .conn_for_test()
        .query_row(
            "SELECT COUNT(*) FROM aux_usage WHERE origin = 'morning_brief' AND mission_id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(briefs, 1);
}

/// Two orgs and two hosts, one in each: `mercury` is Acme's, `venus` Beta's.
fn two_orgs(store: &Mutex<Store>) -> (i64, i64) {
    let s = lock(store).unwrap();
    let acme = s.add_org("Acme", None, false).unwrap().id;
    let beta = s.add_org("Beta", None, false).unwrap().id;
    for (host, org) in [("mercury", acme), ("venus", beta)] {
        s.upsert_host(host).unwrap();
        s.set_host_org(host, Some(org)).unwrap();
    }
    (acme, beta)
}

/// Review round 15, F20: the brief runs on the latest session whose host
/// may be sent its org's text, never on another org's host.
#[test]
fn a_brief_never_runs_on_another_orgs_host() {
    let (deps, _, _) = fixture("brief-orgs");
    let (acme, beta) = two_orgs(&deps.store);
    let group = |org: i64, sid: i64, host: &str, at: i64| TodayGroup {
        bucket: BUCKET_IN_PROGRESS.into(),
        key: Some(format!("K-{sid}")),
        title: "work".into(),
        org_id: Some(org),
        sessions: vec![session(sid, host, Some(org), at)],
        ..Default::default()
    };
    // Beta's session is the latest, but it runs on Acme's host.
    let today = Today {
        since: 0,
        now: 100,
        groups: vec![group(acme, 1, "mercury", 50), group(beta, 2, "mercury", 90)],
        shipped: vec![],
        missions: vec![],
    };
    let s = lock(&deps.store).unwrap();
    let sees = |host: &str, org: Option<i64>| host_sees_org(&s, host, org).unwrap();
    assert_eq!(
        brief_target(&today, None, sees),
        Some((Some(acme), "mercury".to_string())),
        "Beta's text never goes to Acme's host"
    );
    assert_eq!(brief_target(&today, Some(beta), sees), None);
    assert_eq!(
        brief_target(&today, Some(beta), |_, _| true),
        Some((Some(beta), "mercury".to_string())),
        "without the host check it would have run there"
    );
}

/// Review round 15, F20: a mission's release note (and so its planner and
/// triage card, which take the same host) refuses a planner host of another
/// org, before anything is sent to it.
#[tokio::test]
async fn a_release_note_never_runs_on_another_orgs_host() {
    let (deps, _, m) = fixture("note-orgs");
    let (acme, _) = two_orgs(&deps.store);
    {
        let s = lock(&deps.store).unwrap();
        s.conn_for_test()
            .execute(
                "UPDATE orchestration_projects SET org_id = ?1 WHERE id = ?2",
                rusqlite::params![acme, m.id],
            )
            .unwrap();
        let policy = crate::store::MissionPolicy {
            planner_host: Some("venus".into()),
            ..Default::default()
        };
        s.update_mission(
            m.id,
            None,
            &crate::store::MissionPatch {
                policy: Some(policy),
                ..Default::default()
            },
            "t",
        )
        .unwrap();
        s.set_mission_state(m.id, None, "active", "t").unwrap();
        s.set_mission_state(m.id, None, "completed", "t").unwrap();
    }
    let err = release_note(&note_args(m.id), &deps, &ViewScope::internal())
        .await
        .unwrap_err();
    assert_eq!(err.code, codes::E_FORBIDDEN, "{}", err.message);
    assert!(
        err.message.contains("venus") && err.message.contains("same organisation"),
        "{}",
        err.message
    );
    let booked: i64 = lock(&deps.store)
        .unwrap()
        .conn_for_test()
        .query_row("SELECT COUNT(*) FROM aux_usage", [], |r| r.get(0))
        .unwrap();
    assert_eq!(booked, 0, "nothing ran");
}
