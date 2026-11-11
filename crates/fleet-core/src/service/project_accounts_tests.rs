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

/// Accounts review (acct-paths-7): a ticket start or a resume that names no
/// login never lands on one fleet knows is signed out while a signed-in,
/// allowed login on the host could take the work; with none signed in it
/// keeps the login, and the start reports it.
#[test]
fn a_login_nobody_chose_gives_way_when_it_is_signed_out() {
    use crate::store::SignedOutLogin;
    let (s, p) = fleet();
    let sign_out = |profiles: &[Option<&str>]| {
        let list: Vec<SignedOutLogin> = profiles
            .iter()
            .map(|p| SignedOutLogin {
                profile: p.map(str::to_string),
                since: 1,
                checked_at: 1,
            })
            .collect();
        s.set_host_signed_out_logins("mac", &list).unwrap();
    };
    sign_out(&[None]);
    assert_eq!(
        login_with_headroom_for(&s, Some(p), "mac", None, NOW).unwrap(),
        Some("work".into())
    );
    assert_eq!(
        login_with_headroom_for(&s, None, "mac", None, NOW).unwrap(),
        Some("work".into())
    );
    // Never a forbidden one in its place.
    s.set_account_forbidden_projects("acct-work", &[p]).unwrap();
    assert_eq!(
        login_with_headroom_for(&s, Some(p), "mac", None, NOW).unwrap(),
        Some("spare".into())
    );
    // Nothing signed in left: the login is kept.
    sign_out(&[None, Some("spare")]);
    assert_eq!(
        login_with_headroom_for(&s, Some(p), "mac", None, NOW).unwrap(),
        None
    );
}

/// Review of #1118: stepping off a signed-out or forbidden login never lands
/// on a login whose account spent its daily limit (the start would refuse
/// it whatever anyone says) while another login has room.
#[test]
fn stepping_off_a_login_passes_over_a_spent_daily_limit() {
    use crate::service::account_limits::seed_daily_spend;
    use crate::store::SignedOutLogin;
    let (s, p) = fleet();
    // `work` (10%) is the freest, but its account's day is spent.
    seed_daily_spend(&s, "acct-work", 1.0, 2.0, NOW);
    s.set_host_signed_out_logins(
        "mac",
        &[SignedOutLogin {
            profile: None,
            since: 1,
            checked_at: 1,
        }],
    )
    .unwrap();
    assert_eq!(
        login_with_headroom_for(&s, None, "mac", None, NOW).unwrap(),
        Some("spare".into())
    );
    s.set_host_signed_out_logins("mac", &[]).unwrap();
    s.set_account_forbidden_projects("acct-own", &[p]).unwrap();
    assert_eq!(
        login_with_headroom_for(&s, Some(p), "mac", None, NOW).unwrap(),
        Some("spare".into())
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
            forbidden_project_ids: Some(vec![p]),
            forbidden_org_ids: None,
        },
        &store,
    )
    .unwrap();
    assert_eq!(row.forbidden_project_ids, vec![p]);
    let err = set_account_projects(
        SetAccountProjectsArgs {
            uuid: "  ".into(),
            forbidden_project_ids: Some(vec![]),
            forbidden_org_ids: None,
        },
        &store,
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
    // An org list alone leaves the projects as they were.
    let org = lock(&store)
        .unwrap()
        .add_org("Acme", None, false)
        .unwrap()
        .id;
    let row = set_account_projects(
        SetAccountProjectsArgs {
            uuid: "acct-own".into(),
            forbidden_project_ids: None,
            forbidden_org_ids: Some(vec![org]),
        },
        &store,
    )
    .unwrap();
    assert_eq!(row.forbidden_project_ids, vec![p]);
    assert_eq!(row.forbidden_org_ids, vec![org]);
}

#[test]
fn a_start_in_a_forbidden_org_s_project_is_refused() {
    let (s, p) = fleet();
    let org = s.add_org("Acme", None, false).unwrap().id;
    s.add_org_rule(crate::store::OrgRuleRow {
        id: 0,
        org_id: org,
        owner: Some("acme".into()),
        repo: None,
        path_prefix: None,
        host_alias: None,
    })
    .unwrap();
    s.set_account_blocks("acct-own", None, Some(&[org]))
        .unwrap();
    let err = refuse_forbidden(&s, Some(p), "mac", None, NOW).unwrap_err();
    assert_eq!(err.code, codes::E_ACCOUNT_FORBIDDEN);
    assert!(
        err.message.contains("acme/api (all of Acme)"),
        "{}",
        err.message
    );
    assert_eq!(err.details.unwrap()["suggested_profile"], "work");
    assert!(refuse_forbidden(&s, Some(p), "mac", Some("work"), NOW).is_ok());
}

/// Review finding 6: an org that is a host (`hosts.org_id`) or a
/// host-qualified rule is blocked on that host, and the login fleet picks
/// there is an allowed one; the same project elsewhere is untouched.
#[test]
fn an_org_block_holds_for_an_org_that_is_a_host() {
    let (s, p) = fleet();
    seed_usage(&s, "vm1", None, "acct-own", 20.0, NOW);
    seed_usage(&s, "vm1", Some("work"), "acct-work", 10.0, NOW);
    let org = s.add_org("Client", None, false).unwrap().id;
    s.add_org_rule(crate::store::OrgRuleRow {
        id: 0,
        org_id: org,
        owner: None,
        repo: None,
        path_prefix: None,
        host_alias: Some("vm1".into()),
    })
    .unwrap();
    s.set_account_blocks("acct-own", None, Some(&[org]))
        .unwrap();
    assert!(forbidden_accounts(&s, Some(p)).unwrap().is_empty());
    assert_eq!(
        forbidden_accounts_on(&s, Some(p), "vm1").unwrap(),
        vec!["acct-own"]
    );
    assert!(forbidden_accounts_on(&s, Some(p), "mac")
        .unwrap()
        .is_empty());
    assert!(forbidden_accounts_on(&s, None, "vm1").unwrap().is_empty());

    let err = refuse_forbidden(&s, Some(p), "vm1", None, NOW).unwrap_err();
    assert_eq!(err.code, codes::E_ACCOUNT_FORBIDDEN);
    assert!(err.message.contains("(all of Client)"), "{}", err.message);
    assert_eq!(err.details.unwrap()["suggested_profile"], "work");
    assert!(refuse_forbidden(&s, Some(p), "mac", None, NOW).is_ok());
    // The pick moves off the forbidden login on that host only.
    assert_eq!(
        login_for(&s, Some(p), "vm1", None, NOW).unwrap(),
        Some("work".into())
    );
    assert_eq!(login_for(&s, Some(p), "mac", None, NOW).unwrap(), None);
    // No allowed login left there: the pick stays and the start refuses it.
    s.set_account_blocks("acct-work", None, Some(&[org]))
        .unwrap();
    assert_eq!(login_for(&s, Some(p), "vm1", None, NOW).unwrap(), None);
    let err = refuse_forbidden(&s, Some(p), "vm1", None, NOW).unwrap_err();
    assert!(err.details.unwrap()["suggested_profile"].is_null());
}

/// Review finding 7: a relaunch is checked when the row is not live or its
/// login is on another account than the row ran on; a live session on an
/// unchanged account, a shell and a session outside any project are exempt.
#[test]
fn a_relaunch_is_checked_unless_the_session_is_live_on_the_same_account() {
    let (s, p) = fleet();
    let id = s
        .upsert_session("dev-api", "mac", Some(p), None, 1, 1, "running", None)
        .unwrap();
    let set = |sql: &str| {
        s.conn_for_test()
            .execute(&format!("UPDATE sessions SET {sql} WHERE id = {id}"), [])
            .unwrap();
    };
    set("account_uuid = 'acct-own'");
    s.set_account_forbidden_projects("acct-own", &[p]).unwrap();
    let row = || s.get_session_by_id(id).unwrap().unwrap();
    let check = |profile: Option<&str>| refuse_forbidden_relaunch(&s, &row(), profile, NOW);

    // Live, on the account it already runs on: forbidding stops nothing.
    assert!(check(None).is_ok());
    assert!(refuse_forbidden_relaunch_of(&s, id, NOW).is_ok());
    // The same row under a login on another, allowed account is fine too.
    assert!(check(Some("work")).is_ok());
    // The row ran on another account than its login is on now.
    set("account_uuid = 'acct-work'");
    assert_eq!(check(None).unwrap_err().code, codes::E_ACCOUNT_FORBIDDEN);
    // …or on one nobody recorded.
    set("account_uuid = NULL");
    assert!(check(None).is_err());
    // A lost or ghost row coming back is work going somewhere again.
    set("account_uuid = 'acct-own', lost_at = 5");
    assert!(check(None).is_err());
    assert!(refuse_forbidden_relaunch_of(&s, id, NOW).is_err());
    set("lost_at = NULL, status = 'ghost'");
    assert!(check(None).is_err());
    // A shell bills no login; a session outside a project has no rule.
    set("kind = 'shell'");
    assert!(check(None).is_ok());
    set("kind = 'work', project_id = NULL");
    assert!(check(None).is_ok());
    // A row that is gone refuses nothing.
    assert!(refuse_forbidden_relaunch_of(&s, id + 99, NOW).is_ok());
}
