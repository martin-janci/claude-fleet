//! The access check's pure parts: what the probes print and what is read
//! from it, and the organisation rule a grant is checked against.

use super::*;

#[test]
fn a_sign_in_on_the_hosts_login_only_is_named() {
    let out = "noise\n@@O\tatlassian\t1\t0\n@@O\tpapaya\t1\t1\n@@O\tlocal\t0\t0\n@@O\tbad\n";
    assert_eq!(parse_sign_in(out), vec!["atlassian".to_string()]);
    assert!(parse_sign_in("").is_empty());
}

#[test]
fn the_sign_in_probe_quotes_every_name_and_prints_no_token() {
    let script = sign_in_script("work", &["atlassian".into(), "a b'c".into()]);
    assert!(script.contains("'atlassian'"), "{script}");
    assert!(script.contains(&quote("a b'c")), "{script}");
    assert!(
        script.contains(".claude-profiles/\"'work'\"/.credentials.json"),
        "{script}"
    );
    assert!(script.contains("grep -c -F"), "{script}");
    assert!(!script.contains("cat "), "{script}");
}

#[test]
fn a_repo_that_answers_or_is_not_checked_out_there_is_no_problem() {
    assert_eq!(parse_repo("@@R ok\n"), None);
    assert_eq!(parse_repo("@@R nodir\n"), None);
    assert_eq!(parse_repo("motd\n"), None);
    assert_eq!(
        parse_repo("@@R fail git@gitlab.papayapos.dev: Permission denied (publickey).\n")
            .as_deref(),
        Some("git@gitlab.papayapos.dev: Permission denied (publickey).")
    );
    assert_eq!(
        parse_repo("@@R fail\n").as_deref(),
        Some("its remote did not answer")
    );
    let script = repo_script("~/projects/pos");
    assert!(script.contains("GIT_TERMINAL_PROMPT=0"), "{script}");
    assert!(script.contains("BatchMode=yes"), "{script}");
    assert!(
        script.contains("ls-remote -q --exit-code origin HEAD"),
        "{script}"
    );
}

#[test]
fn a_report_says_what_a_person_must_do_and_nothing_when_all_is_well() {
    let ok = TargetReport {
        host: "h1".into(),
        profile: Some("work".into()),
        shared: vec!["atlassian".into()],
        ..Default::default()
    };
    assert!(ok.problems().is_empty(), "sharing is done, not asked");
    let bad = TargetReport {
        host: "h1".into(),
        profile: Some("work".into()),
        sign_in: vec!["atlassian".into()],
        repos_failed: vec!["papaya/pos (denied)".into()],
        ..Default::default()
    };
    let p = bad.problems();
    assert_eq!(p.len(), 2, "{p:?}");
    assert!(p[0].contains("h1 under login work"), "{p:?}");
    assert!(
        p[0].contains("CLAUDE_CONFIG_DIR=~/.claude-profiles/work claude"),
        "{p:?}"
    );
    assert!(p[1].contains("papaya/pos"), "{p:?}");
    let card = text::card(&p);
    assert!(card.lines().count() == 3, "{card}");
}

fn mission(s: &Store) -> MissionRow {
    s.create_mission(
        &crate::store::NewMission {
            org_id: None,
            owner_person_id: None,
            root_item_id: None,
            name: "POS payments",
            goal: "Card payments on the POS.",
            non_goals: None,
            done_when: &[],
            mode: None,
            level: None,
        },
        "test",
    )
    .unwrap()
}

/// A host of another organisation conflicts with the mission's tasks; an
/// unassigned host, or one of the tasks' own, does not.
#[test]
fn a_host_of_another_organisation_is_named_at_the_grant() {
    let s = Store::open_in_memory().unwrap();
    let papaya = s.add_org("papaya", None, false).unwrap();
    let home = s.add_org("home", None, false).unwrap();
    for (h, org) in [
        ("work-box", Some(papaya.id)),
        ("home-box", Some(home.id)),
        ("spare", None),
    ] {
        s.upsert_host(h).unwrap();
        s.set_host_org(h, org).unwrap();
    }
    let item = s
        .create_native_item(&crate::store::NativeItem {
            title: "Terminal refunds",
            parent_id: None,
            project_id: None,
            notes: None,
        })
        .unwrap();
    s.set_local_item_org(item.id, Some(papaya.id)).unwrap();
    let m = mission(&s);
    let m = s.set_mission_item(m.id, item.id, true, "test").unwrap();
    let hosts: Vec<String> = ["work-box", "home-box", "spare"].map(String::from).to_vec();
    let found = org_conflicts(&s, &m, &hosts).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].starts_with("home-box runs sessions in home"),
        "{found:?}"
    );
    assert!(found[0].contains("belong to papaya"), "{found:?}");

    // Tasks with no organisation never conflict.
    let plain = mission(&s);
    assert!(org_conflicts(&s, &plain, &hosts).unwrap().is_empty());
}
