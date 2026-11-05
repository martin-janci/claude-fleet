use super::*;
use crate::service::account_limits::{
    freer_login_for, login_with_headroom_for, ready_host, refuse_over_limit_for, seed_usage,
};
use crate::service::placement::{suggest_with, Want};

const NOW: i64 = 1_000;

/// mac: its own login on `acct-own` (20% used), profile `work` on
/// `acct-work` (10%), profile `spare` on `acct-spare` (40%); one project.
fn fleet() -> (Store, i64) {
    let s = Store::open_in_memory().unwrap();
    seed_usage(&s, "mac", None, "acct-own", 20.0, NOW);
    seed_usage(&s, "mac", Some("work"), "acct-work", 10.0, NOW);
    seed_usage(&s, "mac", Some("spare"), "acct-spare", 40.0, NOW);
    let p = s.upsert_project("acme", "api", "/srv/acme/api").unwrap();
    (s, p)
}

#[test]
fn a_start_on_a_forbidden_account_is_refused_and_names_an_allowed_login() {
    let (s, p) = fleet();
    // Nothing forbidden: nothing refused.
    assert!(refuse_forbidden(&s, Some(p), "mac", None, NOW).is_ok());
    s.set_account_forbidden_projects("acct-own", &[p]).unwrap();

    let err = refuse_forbidden(&s, Some(p), "mac", None, NOW).unwrap_err();
    assert_eq!(err.code, codes::E_ACCOUNT_FORBIDDEN);
    assert!(err.message.contains("acme/api"), "{}", err.message);
    assert!(err.message.contains("mac's own login"), "{}", err.message);
    assert!(err.message.contains("profile \"work\""), "{}", err.message);
    let d = err.details.unwrap();
    assert_eq!(d["account_uuid"], "acct-own");
    assert_eq!(d["project_id"], p);
    assert_eq!(d["suggested_profile"], "work");
    // `""` spells the host's own login, for a start under a profile.
    assert!(refuse_forbidden(&s, Some(p), "mac", Some(""), NOW).is_err());

    // Every other login, another project, no project, a login not on a
    // known account and a host fleet does not know all go ahead.
    assert!(refuse_forbidden(&s, Some(p), "mac", Some("work"), NOW).is_ok());
    let q = s.upsert_project("acme", "web", "/srv/acme/web").unwrap();
    assert!(refuse_forbidden(&s, Some(q), "mac", None, NOW).is_ok());
    assert!(refuse_forbidden(&s, None, "mac", None, NOW).is_ok());
    assert!(refuse_forbidden(&s, Some(p), "mac", Some("fresh"), NOW).is_ok());
    assert!(refuse_forbidden(&s, Some(p), "nowhere", None, NOW).is_ok());
    assert_eq!(
        forbidden_reason(&s, Some(p), "mac", Some("work"), NOW).unwrap(),
        None
    );
    assert!(forbidden_reason(&s, Some(p), "mac", None, NOW)
        .unwrap()
        .is_some_and(|why| why.contains("forbidden for acme/api")));
}

#[test]
fn with_no_allowed_login_left_the_refusal_says_so() {
    let (s, p) = fleet();
    for a in ["acct-own", "acct-work", "acct-spare"] {
        s.set_account_forbidden_projects(a, &[p]).unwrap();
    }
    let err = refuse_forbidden(&s, Some(p), "mac", Some("work"), NOW).unwrap_err();
    assert!(
        err.message.contains("no other login on mac"),
        "{}",
        err.message
    );
    assert!(err.details.unwrap()["suggested_profile"].is_null());
}

#[test]
fn a_login_nobody_chose_gives_way_to_an_allowed_one() {
    let (s, p) = fleet();
    s.set_account_forbidden_projects("acct-own", &[p]).unwrap();
    // The host's own login is forbidden: the allowed login with the most
    // headroom takes the work, though the own one is under the line.
    assert_eq!(
        login_with_headroom_for(&s, Some(p), "mac", None, NOW).unwrap(),
        Some("work".into())
    );
    // Another project keeps the host's own login.
    assert_eq!(
        login_with_headroom_for(&s, None, "mac", None, NOW).unwrap(),
        None
    );
    // An allowed login past the line is still better than a forbidden one:
    // the start's own checks report it.
    // (A second reading a second newer, so it is the latest.)
    seed_usage(&s, "mac", Some("work"), "acct-work", 97.0, NOW + 1);
    seed_usage(&s, "mac", Some("spare"), "acct-spare", 99.0, NOW + 1);
    assert_eq!(
        login_with_headroom_for(&s, Some(p), "mac", None, NOW).unwrap(),
        Some("work".into())
    );
}

#[test]
fn a_freer_login_is_never_a_forbidden_one() {
    let (s, p) = fleet();
    seed_usage(&s, "mac", None, "acct-own", 95.0, NOW + 1);
    // Without a block the freest login is `work`.
    assert_eq!(
        freer_login_for(&s, Some(p), "mac", "acct-own", NOW)
            .unwrap()
            .and_then(|l| l.profile),
        Some("work".into())
    );
    s.set_account_forbidden_projects("acct-work", &[p]).unwrap();
    assert_eq!(
        freer_login_for(&s, Some(p), "mac", "acct-own", NOW)
            .unwrap()
            .and_then(|l| l.profile),
        Some("spare".into())
    );
    assert_eq!(
        login_with_headroom_for(&s, Some(p), "mac", None, NOW).unwrap(),
        Some("spare".into())
    );
    // The over-limit refusal names the allowed login too.
    let err = refuse_over_limit_for(&s, Some(p), "mac", None, NOW).unwrap_err();
    assert_eq!(err.code, codes::E_ACCOUNT_LIMIT);
    assert_eq!(err.details.unwrap()["suggested_profile"], "spare");
}

#[test]
fn placement_leaves_a_forbidden_login_out_and_says_why() {
    let (s, p) = fleet();
    ready_host(&s, "mac");
    s.set_account_forbidden_projects("acct-work", &[p]).unwrap();
    let placed = suggest_with(&s, Some(p), &Want::default(), false, NOW).unwrap();
    assert!(
        placed
            .candidates
            .iter()
            .all(|c| c.account_uuid != "acct-work"),
        "{placed:?}"
    );
    assert!(placed
        .excluded
        .iter()
        .any(|e| e.profile.as_deref() == Some("work") && e.why.contains("forbids")));
    // Another project may still run there.
    let other = suggest_with(&s, None, &Want::default(), false, NOW).unwrap();
    assert!(other
        .candidates
        .iter()
        .any(|c| c.account_uuid == "acct-work"));
}

#[test]
fn set_account_projects_replaces_the_list_and_refuses_a_blank_uuid() {
    let (s, p) = fleet();
    let store = Mutex::new(s);
    let row = set_account_projects(
        SetAccountProjectsArgs {
            uuid: " acct-own ".into(),
            forbidden_project_ids: vec![p],
        },
        &store,
    )
    .unwrap();
    assert_eq!(row.forbidden_project_ids, vec![p]);
    let err = set_account_projects(
        SetAccountProjectsArgs {
            uuid: "  ".into(),
            forbidden_project_ids: vec![],
        },
        &store,
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
}
