//! Placement: what it leaves out, how it ranks, and the store wrapper.

use super::*;

fn login(profile: Option<&str>, account: &str, used: Option<f64>) -> LoginFacts {
    LoginFacts {
        profile: profile.map(str::to_string),
        account_uuid: account.into(),
        email: None,
        used_pct: used,
        daily_limit_hit: false,
        signed_out_since: None,
        live_sessions: 0,
    }
}

fn host(alias: &str, cpu: Option<f64>, mem: Option<f64>, logins: Vec<LoginFacts>) -> HostFacts {
    HostFacts {
        alias: alias.into(),
        up: true,
        has_claude: true,
        loaded: None,
        cpu_pct: cpu,
        mem_avail_pct: mem,
        has_repo: None,
        logins,
    }
}

fn places(p: &Placement) -> Vec<(String, Option<String>)> {
    p.candidates
        .iter()
        .map(|c| (c.host_alias.clone(), c.profile.clone()))
        .collect()
}

#[test]
fn the_login_with_the_most_headroom_on_a_calm_host_wins() {
    let hosts = vec![
        host(
            "a",
            Some(10.0),
            Some(80.0),
            vec![login(None, "A", Some(70.0))],
        ),
        host(
            "b",
            Some(10.0),
            Some(80.0),
            vec![
                login(None, "B", Some(20.0)),
                login(Some("w"), "C", Some(5.0)),
            ],
        ),
    ];
    let p = rank(&hosts, &Want::default(), 90.0);
    assert_eq!(
        places(&p),
        vec![
            ("b".into(), Some("w".into())),
            ("b".into(), None),
            ("a".into(), None)
        ]
    );
    assert!(p.excluded.is_empty());
    assert!(p.candidates[0].reasons[0].contains("5% used"));
}

#[test]
fn load_breaks_a_headroom_tie() {
    let hosts = vec![
        host(
            "busy",
            Some(90.0),
            Some(20.0),
            vec![login(None, "A", Some(10.0))],
        ),
        host(
            "calm",
            Some(5.0),
            Some(90.0),
            vec![login(None, "B", Some(10.0))],
        ),
    ];
    assert_eq!(
        rank(&hosts, &Want::default(), 90.0).candidates[0].host_alias,
        "calm"
    );
}

#[test]
fn a_down_loaded_or_claude_less_host_and_a_project_less_one_are_left_out() {
    let mut down = host("down", None, None, vec![login(None, "A", Some(1.0))]);
    down.up = false;
    let mut loaded = host("loaded", None, None, vec![login(None, "B", Some(1.0))]);
    loaded.loaded = Some("memory_low".into());
    let mut bare = host("bare", None, None, vec![login(None, "C", Some(1.0))]);
    bare.has_claude = false;
    let mut norepo = host("norepo", None, None, vec![login(None, "D", Some(1.0))]);
    norepo.has_repo = Some(false);
    let nologin = host("nologin", None, None, vec![]);
    let mut ok = host("ok", None, None, vec![login(None, "E", Some(50.0))]);
    ok.has_repo = Some(true);
    let p = rank(
        &[down, loaded, bare, norepo, nologin, ok],
        &Want::default(),
        90.0,
    );
    assert_eq!(places(&p), vec![("ok".into(), None)]);
    let why: Vec<&str> = p.excluded.iter().map(|e| e.why.as_str()).collect();
    assert_eq!(why.len(), 5);
    assert!(why[1].contains("memory low"), "{why:?}");
    assert!(p.excluded.iter().all(|e| e.host));
}

#[test]
fn a_signed_out_spent_or_over_the_line_login_is_left_out_with_its_reason() {
    let mut out = login(None, "A", Some(1.0));
    out.signed_out_since = Some(1_786_276_800);
    let mut spent = login(Some("api"), "B", None);
    spent.daily_limit_hit = true;
    let over = login(Some("full"), "C", Some(95.0));
    let fine = login(Some("fine"), "D", None);
    let p = rank(
        &[host("h", None, None, vec![out, spent, over, fine])],
        &Want::default(),
        90.0,
    );
    assert_eq!(places(&p), vec![("h".into(), Some("fine".into()))]);
    let why: Vec<&str> = p.excluded.iter().map(|e| e.why.as_str()).collect();
    assert_eq!(
        why,
        vec![
            "login signed out (since Aug 9)",
            "its account spent its daily limit",
            "its account is at 95%, over accounts.pause_at (90%)",
        ]
    );
    assert!(p.candidates[0].reasons[0].contains("no usage reading"));
}

#[test]
fn a_grant_keeps_to_its_hosts_and_login_and_the_ran_out_account_is_skipped() {
    let hosts = vec![
        host(
            "a",
            None,
            None,
            vec![
                login(None, "A", Some(1.0)),
                login(Some("w"), "W", Some(2.0)),
            ],
        ),
        host("b", None, None, vec![login(None, "B", Some(1.0))]),
    ];
    let want = Want {
        hosts: Some(vec!["a".into()]),
        profile: Some(Some("w".into())),
        ..Default::default()
    };
    let p = rank(&hosts, &want, 90.0);
    assert_eq!(places(&p), vec![("a".into(), Some("w".into()))]);
    assert!(
        p.excluded.is_empty(),
        "outside the grant is not an exclusion"
    );
    let want = Want {
        not_account: Some("A".into()),
        not_host: Some("b".into()),
        ..Default::default()
    };
    let p = rank(&hosts, &want, 90.0);
    assert_eq!(places(&p), vec![("a".into(), Some("w".into()))]);
}

#[test]
fn the_preferred_host_wins_a_near_tie_and_loses_a_clear_one() {
    let hosts = vec![
        host("new", None, None, vec![login(None, "A", Some(10.0))]),
        host("old", None, None, vec![login(None, "B", Some(15.0))]),
    ];
    let want = Want {
        prefer_host: Some("old".into()),
        ..Default::default()
    };
    assert_eq!(rank(&hosts, &want, 90.0).candidates[0].host_alias, "old");
    let hosts = vec![
        host("new", None, None, vec![login(None, "A", Some(10.0))]),
        host("old", None, None, vec![login(None, "B", Some(60.0))]),
    ];
    assert_eq!(rank(&hosts, &want, 90.0).candidates[0].host_alias, "new");
}

#[test]
fn the_store_wrapper_reads_logins_usage_health_and_the_project() {
    use crate::service::account_limits::{ready_host, seed_usage};
    let s = Store::open_in_memory().unwrap();
    let now = 1_000_000;
    seed_usage(&s, "mac", None, "acct-mac", 30.0, now);
    seed_usage(&s, "htz", None, "acct-htz", 5.0, now);
    ready_host(&s, "mac");
    ready_host(&s, "htz");
    // htz has the most headroom, but its login is signed out.
    login_health::record(
        &s,
        &[login_health::Observation {
            host_alias: "htz".into(),
            profile: None,
            signed_out: true,
        }],
        now,
    )
    .unwrap();
    let p = suggest_with(&s, None, &Want::default(), false, now).unwrap();
    assert_eq!(places(&p), vec![("mac".into(), None)]);
    assert!(
        p.excluded[0].why.starts_with("login signed out"),
        "{:?}",
        p.excluded
    );

    // A project that ran only on htz leaves mac out; one nobody ran says so.
    let pid = s.upsert_project("o", "r", "/r").unwrap();
    s.upsert_session("t", "htz", Some(pid), None, 1, 1, "running", None)
        .unwrap();
    let p = suggest_with(&s, Some(pid), &Want::default(), false, now).unwrap();
    assert!(p.candidates.is_empty());
    assert!(p.excluded.iter().any(|e| e.host_alias == "mac" && e.host));
    let other = s.upsert_project("o", "other", "/o").unwrap();
    let p = suggest_with(&s, Some(other), &Want::default(), false, now).unwrap();
    assert_eq!(places(&p), vec![("mac".into(), None)]);
    assert!(p.note.is_some());
}

/// Live sessions on an account cost it: the usage reading lags the runs a
/// mission just started, so a pass's picks spread instead of piling up.
#[test]
fn live_sessions_spread_picks_across_logins() {
    let busy = LoginFacts {
        live_sessions: 3,
        ..login(Some("b"), "acct-b", Some(60.0))
    };
    let hosts = vec![host(
        "h",
        Some(10.0),
        Some(80.0),
        vec![login(Some("a"), "acct-a", Some(70.0)), busy],
    )];
    let p = rank(&hosts, &Want::default(), 90.0);
    assert_eq!(p.candidates[0].account_uuid, "acct-a");
    assert!(
        p.candidates[1]
            .reasons
            .iter()
            .any(|r| r == "3 sessions already on the account"),
        "{:?}",
        p.candidates[1].reasons
    );
    // Without them the emptier account wins.
    let idle = vec![host(
        "h",
        Some(10.0),
        Some(80.0),
        vec![
            login(Some("a"), "acct-a", Some(70.0)),
            login(Some("b"), "acct-b", Some(60.0)),
        ],
    )];
    assert_eq!(
        rank(&idle, &Want::default(), 90.0).candidates[0].account_uuid,
        "acct-b"
    );
}

/// The store side of the penalty: running sessions charge their account,
/// and a run whose link a profile start just dropped
/// (`set_session_profile`) charges the account its login holds, so the
/// next pick of the same pass does not land on that login again.
#[test]
fn the_store_wrapper_charges_live_sessions_linked_or_not_yet() {
    use crate::service::account_limits::{ready_host, seed_usage};
    let now = 1_000_000;
    let fresh = || {
        let s = Store::open_in_memory().unwrap();
        seed_usage(&s, "mac", None, "acct-a", 70.0, now);
        seed_usage(&s, "mac", Some("b"), "acct-b", 60.0, now);
        ready_host(&s, "mac");
        s
    };
    let top = |s: &Store| {
        suggest_with(s, None, &Want::default(), false, now)
            .unwrap()
            .candidates[0]
            .account_uuid
            .clone()
    };

    let s = fresh();
    assert_eq!(top(&s), "acct-b", "the emptier account wins when idle");

    // Linked runs on acct-b.
    for n in ["r1", "r2", "r3"] {
        s.upsert_session(n, "mac", None, None, 1, 1, "running", Some("acct-b"))
            .unwrap();
    }
    // A stopped one does not count.
    s.upsert_session("old", "mac", None, None, 1, 1, "stopped", Some("acct-a"))
        .unwrap();
    assert_eq!(top(&s), "acct-a", "linked runs cost their account");

    // Runs just started under profile `b`: reconcile linked them to the
    // host's account, the profile start dropped the link.
    let s = fresh();
    for n in ["p1", "p2", "p3"] {
        let id = s
            .upsert_session(n, "mac", None, None, 1, 1, "running", Some("acct-a"))
            .unwrap();
        s.set_session_profile(id, Some("b")).unwrap();
    }
    assert_eq!(
        s.live_unlinked_sessions_per_login().unwrap()[&("mac".to_string(), Some("b".to_string()))],
        3
    );
    assert_eq!(
        top(&s),
        "acct-a",
        "unlinked runs cost their login's account"
    );
}
