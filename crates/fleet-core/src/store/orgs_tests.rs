use super::*;
use crate::store::{Store, WorkTarget};

fn rule(id: i64, org: i64) -> OrgRuleRow {
    OrgRuleRow {
        id,
        org_id: org,
        ..Default::default()
    }
}

fn owner(id: i64, org: i64, o: &str) -> OrgRuleRow {
    OrgRuleRow {
        owner: Some(o.into()),
        ..rule(id, org)
    }
}

fn repo(id: i64, org: i64, o: &str, r: &str) -> OrgRuleRow {
    OrgRuleRow {
        owner: Some(o.into()),
        repo: Some(r.into()),
        ..rule(id, org)
    }
}

fn path(id: i64, org: i64, p: &str) -> OrgRuleRow {
    OrgRuleRow {
        path_prefix: Some(p.into()),
        ..rule(id, org)
    }
}

fn host(id: i64, org: i64, h: &str) -> OrgRuleRow {
    OrgRuleRow {
        host_alias: Some(h.into()),
        ..rule(id, org)
    }
}

/// `(name, facts, rules, host org, expected)`.
type Case = (
    &'static str,
    SessionOrgFacts<'static>,
    Vec<OrgRuleRow>,
    Option<i64>,
    Option<i64>,
);

fn facts(
    h: &'static str,
    o: Option<&'static str>,
    r: Option<&'static str>,
    p: Option<&'static str>,
) -> SessionOrgFacts<'static> {
    SessionOrgFacts {
        host_alias: h,
        owner: o,
        repo: r,
        path: p,
    }
}

fn cases() -> Vec<Case> {
    let acme = facts("h1", Some("acme"), Some("api"), Some("/src/acme/api"));
    vec![
        ("no rules, no host org", acme.clone(), vec![], None, None),
        (
            "no rules: the host's org",
            acme.clone(),
            vec![],
            Some(7),
            Some(7),
        ),
        (
            "owner beats host",
            acme.clone(),
            vec![owner(1, 1, "acme")],
            Some(7),
            Some(1),
        ),
        (
            "owner matches case-insensitively",
            acme.clone(),
            vec![owner(1, 1, "ACME")],
            None,
            Some(1),
        ),
        (
            "repo beats owner",
            acme.clone(),
            vec![owner(1, 1, "acme"), repo(2, 2, "acme", "api")],
            None,
            Some(2),
        ),
        (
            "path beats repo",
            acme.clone(),
            vec![repo(1, 2, "acme", "api"), path(2, 3, "/src/acme")],
            None,
            Some(3),
        ),
        (
            "the longer path wins",
            acme.clone(),
            vec![path(1, 3, "/src"), path(2, 4, "/src/acme")],
            None,
            Some(4),
        ),
        (
            "a path matches on a directory boundary only",
            acme.clone(),
            vec![path(1, 3, "/src/ac")],
            Some(7),
            Some(7),
        ),
        (
            "an exact path matches",
            acme.clone(),
            vec![path(1, 3, "/src/acme/api")],
            None,
            Some(3),
        ),
        (
            "a host rule beats the host's own org",
            acme.clone(),
            vec![host(1, 5, "h1")],
            Some(7),
            Some(5),
        ),
        (
            "an owner rule beats a host rule",
            acme.clone(),
            vec![host(1, 5, "h1"), owner(2, 1, "acme")],
            None,
            Some(1),
        ),
        (
            "a host-qualified owner rule beats the bare one",
            acme.clone(),
            vec![
                owner(1, 1, "acme"),
                OrgRuleRow {
                    host_alias: Some("h1".into()),
                    ..owner(2, 2, "acme")
                },
            ],
            None,
            Some(2),
        ),
        (
            "a rule for another host does not match",
            acme.clone(),
            vec![OrgRuleRow {
                host_alias: Some("h2".into()),
                ..owner(1, 1, "acme")
            }],
            None,
            None,
        ),
        (
            "a tie goes to the lower id",
            acme.clone(),
            vec![owner(9, 2, "acme"), owner(3, 1, "acme")],
            None,
            Some(1),
        ),
        (
            "`local` is never an owner",
            facts("h1", Some("local"), Some("notes"), Some("/home/me/notes")),
            vec![owner(1, 1, "local")],
            None,
            None,
        ),
        (
            "an adopted folder resolves by path",
            facts("h1", Some("local"), Some("notes"), Some("/home/me/notes")),
            vec![path(1, 4, "/home/me")],
            None,
            Some(4),
        ),
        (
            "no project: the host decides",
            facts("h1", None, None, None),
            vec![owner(1, 1, "acme"), path(2, 3, "/src")],
            Some(7),
            Some(7),
        ),
        (
            "no project: a host rule",
            facts("h1", None, None, None),
            vec![host(1, 5, "h1")],
            None,
            Some(5),
        ),
    ]
}

#[test]
fn resolution_follows_specificity() {
    for (name, f, rules, host_org, want) in cases() {
        assert_eq!(org_of_session(&f, &rules, host_org), want, "{name}");
    }
}

/// Every case above through the SQL column: same answer.
#[test]
fn the_sql_column_agrees_with_the_pure_resolver() {
    for (name, f, rules, host_org, want) in cases() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host(f.host_alias).unwrap();
        s.upsert_host("h2").unwrap();
        // Orgs 1..=9 so any id a case names exists.
        for i in 1..=9 {
            s.add_org(&format!("org{i}"), None, false).unwrap();
        }
        if let Some(o) = host_org {
            s.set_host_org(f.host_alias, Some(o)).unwrap();
        }
        // Insert in id order so the stored ids are the case's ids.
        let mut sorted = rules.clone();
        sorted.sort_by_key(|r| r.id);
        for r in &sorted {
            if r.owner.as_deref() == Some("local") {
                // The table itself refuses it (CHECK), so it can never match.
                assert!(s
                    .conn
                    .execute(
                        "INSERT INTO org_rules (org_id, owner) VALUES (?1, 'local')",
                        rusqlite::params![r.org_id],
                    )
                    .is_err());
                continue;
            }
            s.conn
                .execute(
                    "INSERT INTO org_rules (id, org_id, owner, repo, path_prefix, host_alias) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![r.id, r.org_id, r.owner, r.repo, r.path_prefix, r.host_alias],
                )
                .unwrap();
        }
        let pid = match (f.owner, f.repo, f.path) {
            (Some(o), Some(r), Some(p)) => Some(s.upsert_project(o, r, p).unwrap()),
            _ => None,
        };
        let sid = s
            .upsert_session("dev", f.host_alias, pid, None, 1, 1, "running", None)
            .unwrap();
        assert_eq!(s.session_org(sid).unwrap(), want, "{name}: session_org");
        let row = s.get_session_by_id(sid).unwrap().unwrap();
        assert_eq!(row.org_id, want, "{name}: SessionRow.org_id");
    }
}

#[test]
fn a_worktree_path_beats_the_project_path() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("local").unwrap();
    let a = s.add_org("A", None, false).unwrap();
    let b = s.add_org("B", None, false).unwrap();
    let pid = s.upsert_project("acme", "api", "/src/acme/api").unwrap();
    let wt = s
        .upsert_worktree(pid, "feat", "/wt/client-b/feat", Some("feat"))
        .unwrap();
    s.add_org_rule(owner(0, a.id, "acme")).unwrap();
    s.add_org_rule(path(0, b.id, "/wt/client-b/")).unwrap();
    let on_wt = s
        .upsert_session("wt", "local", Some(pid), Some(wt), 1, 1, "running", None)
        .unwrap();
    let on_root = s
        .upsert_session("root", "local", Some(pid), None, 1, 1, "running", None)
        .unwrap();
    assert_eq!(s.session_org(on_wt).unwrap(), Some(b.id));
    assert_eq!(s.session_org(on_root).unwrap(), Some(a.id));
}

/// Review C22: a project row deleted and re-created (new id) keeps its org,
/// because the rule is text.
#[test]
fn a_recreated_project_keeps_its_org() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let a = s.add_org("A", None, false).unwrap();
    s.add_org_rule(repo(0, a.id, "acme", "api")).unwrap();
    let pid = s.upsert_project("acme", "api", "/src/api").unwrap();
    s.conn
        .execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![pid])
        .unwrap();
    let pid2 = s.upsert_project("Acme", "API", "/elsewhere/api").unwrap();
    let sid = s
        .upsert_session("dev", "h", Some(pid2), None, 1, 1, "running", None)
        .unwrap();
    assert_eq!(s.session_org(sid).unwrap(), Some(a.id));
}

/// Migration 050's trigger writes the org the session had when its link
/// ended — the same answer as the SQL column before the retirement.
#[test]
fn the_trigger_snapshots_the_session_org() {
    for (name, f, rules, host_org, want) in cases() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host(f.host_alias).unwrap();
        for i in 1..=9 {
            s.add_org(&format!("org{i}"), None, false).unwrap();
        }
        if let Some(o) = host_org {
            s.set_host_org(f.host_alias, Some(o)).unwrap();
        }
        for r in rules.iter().filter(|r| r.owner.as_deref() != Some("local")) {
            s.conn
                .execute(
                    "INSERT INTO org_rules (id, org_id, owner, repo, path_prefix, host_alias) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![r.id, r.org_id, r.owner, r.repo, r.path_prefix, r.host_alias],
                )
                .unwrap();
        }
        let pid = match (f.owner, f.repo, f.path) {
            (Some(o), Some(r), Some(p)) => Some(s.upsert_project(o, r, p).unwrap()),
            _ => None,
        };
        let sid = s
            .upsert_session("dev", f.host_alias, pid, None, 1, 1, "running", None)
            .unwrap();
        s.link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        let live = s.session_work_links(sid).unwrap();
        assert_eq!(
            s.link_org(&live[0]).unwrap(),
            want,
            "{name}: a live link has its session's org"
        );
        s.delete_session(sid).unwrap();
        let mut ended = s.ended_work_links_for_key("ABC-1").unwrap();
        assert_eq!(ended.len(), 1, "{name}");
        assert_eq!(ended[0].org_id, want, "{name}: snap_org_id");
        // Rules changing later do not move past work.
        s.conn.execute("DELETE FROM org_rules", []).unwrap();
        s.conn
            .execute("UPDATE hosts SET org_id = NULL", [])
            .unwrap();
        s.fill_link_orgs(&mut ended).unwrap();
        assert_eq!(ended[0].org_id, want, "{name}: kept after the rules moved");
    }
}

#[test]
fn a_tracker_item_takes_its_trackers_org_over_the_sessions() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let a = s.add_org("A", None, false).unwrap();
    let b = s.add_org("B", None, false).unwrap();
    s.set_host_org("h", Some(a.id)).unwrap();
    let t = s
        .add_tracker("jira", "B Jira", "https://b.atlassian.net")
        .unwrap();
    s.set_tracker_org(t.id, Some(b.id)).unwrap();
    let item = crate::store::test_support::tracker_item(&s, t.id, "10001", "BB-1", "Pay");
    let sid = s
        .upsert_session("dev", "h", None, None, 1, 1, "running", None)
        .unwrap();
    s.link_session_work(sid, WorkTarget::Item(item), "manual")
        .unwrap();
    s.link_session_work(sid, WorkTarget::Key("LOC-1"), "manual")
        .unwrap();
    let mut links = s.session_work_links(sid).unwrap();
    s.fill_link_orgs(&mut links).unwrap();
    let by_key = |k: &str| {
        links
            .iter()
            .find(|l| l.ref_key.as_deref() == Some(k) || (k == "BB-1" && l.item_id == Some(item)))
            .unwrap()
            .org_id
    };
    assert_eq!(by_key("BB-1"), Some(b.id));
    assert_eq!(by_key("LOC-1"), Some(a.id));
    let row = s.get_session_by_id(sid).unwrap().unwrap();
    assert_eq!(row.org_id, Some(a.id));
    assert_eq!(
        row.work.as_ref().unwrap().org_id,
        Some(a.id),
        "LOC-1 is primary"
    );
}

#[test]
fn crud_and_refusals() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let a = s.add_org("Company A", Some("#ABC"), false).unwrap();
    assert_eq!(a.color.as_deref(), Some("#abc"));
    assert_eq!(
        s.add_org("company a", None, false).unwrap_err().code,
        codes::E_EXISTS
    );
    assert_eq!(
        s.add_org("  ", None, false).unwrap_err().code,
        codes::E_INVALID
    );
    assert_eq!(
        s.add_org("X", Some("red"), false).unwrap_err().code,
        codes::E_INVALID
    );
    let b = s.add_org("B", None, false).unwrap();
    assert_eq!(
        s.update_org(b.id, Some("Company A"), None, None)
            .unwrap_err()
            .code,
        codes::E_EXISTS
    );
    let b = s.update_org(b.id, None, Some(""), Some(true)).unwrap();
    assert!(b.isolate_sessions);
    assert_eq!(
        s.isolated_orgs().unwrap().into_iter().collect::<Vec<_>>(),
        vec![b.id]
    );

    for bad in [
        rule(0, a.id),
        owner(0, a.id, "local"),
        owner(0, a.id, "LOCAL"),
        OrgRuleRow {
            repo: Some("api".into()),
            ..rule(0, a.id)
        },
        path(0, a.id, "/"),
        owner(0, a.id, "two\nlines"),
    ] {
        assert_eq!(
            s.add_org_rule(bad.clone()).unwrap_err().code,
            codes::E_INVALID,
            "{bad:?}"
        );
    }
    assert_eq!(
        s.add_org_rule(owner(0, 999, "acme")).unwrap_err().code,
        codes::E_NOTFOUND
    );
    let r = s.add_org_rule(path(0, a.id, " /src/acme/ ")).unwrap();
    assert_eq!(r.path_prefix.as_deref(), Some("/src/acme"));
    assert_eq!(
        s.add_org_rule(path(0, b.id, "/src/acme")).unwrap_err().code,
        codes::E_EXISTS
    );
    s.set_host_org("h", Some(a.id)).unwrap();
    assert_eq!(s.host_org("h").unwrap(), Some(a.id));
    assert_eq!(
        s.set_host_org("nope", Some(a.id)).unwrap_err().code,
        codes::E_NOTFOUND
    );
    assert_eq!(
        s.set_host_org("h", Some(999)).unwrap_err().code,
        codes::E_NOTFOUND
    );
    assert_eq!(
        s.list_hosts()
            .unwrap()
            .iter()
            .find(|h| h.alias == "h")
            .unwrap()
            .org_id,
        Some(a.id)
    );
    // Removing the org unassigns its hosts and takes its rules.
    assert!(s.remove_org(a.id).unwrap());
    assert_eq!(s.host_org("h").unwrap(), None);
    assert!(s.list_org_rules().unwrap().is_empty());
    assert!(!s.remove_org(a.id).unwrap());
    assert!(!s.remove_org_rule(r.id).unwrap());
}

/// Removing an org also unassigns its past links (`snap_org_id`, no FK):
/// the ended link of a removed org reads as unassigned, not as a fence to
/// an org that no longer exists. One transaction: nothing changes when the
/// org does not exist.
#[test]
fn remove_org_unassigns_its_past_links_too() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let a = s.add_org("A", None, false).unwrap().id;
    let b = s.add_org("B", None, false).unwrap().id;
    s.set_host_org("h", Some(a)).unwrap();
    let sid = s
        .upsert_session("dev", "h", None, None, 1, 1, "running", None)
        .unwrap();
    s.link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
        .unwrap();
    s.delete_session(sid).unwrap();
    let ended = s.ended_work_links_for_key("ABC-1").unwrap();
    assert_eq!(s.link_org(&ended[0]).unwrap(), Some(a), "snapshotted");
    // Another org's snapshot, for contrast.
    s.conn
        .execute(
            "INSERT INTO work_links (ref_key, state, source, is_primary, created_at, ended_at, snap_org_id) \
             VALUES ('XYZ-1', 'confirmed', 'manual', 1, 1, 2, ?1)",
            [b],
        )
        .unwrap();
    assert!(
        !s.remove_org(a + b + 100).unwrap(),
        "no such org: nothing changes"
    );
    assert_eq!(s.link_org(&ended[0]).unwrap(), Some(a));
    assert!(s.remove_org(a).unwrap());
    let ended = s.ended_work_links_for_key("ABC-1").unwrap();
    assert_eq!(s.link_org(&ended[0]).unwrap(), None, "unassigned now");
    let other: Option<i64> = s
        .conn
        .query_row(
            "SELECT snap_org_id FROM work_links WHERE ref_key = 'XYZ-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(other, Some(b), "B's snapshot is untouched");
}

/// `announce_org_moves` bumps exactly the rows whose org changed, in one
/// transaction (one commit, not one per row), ghosts included, and answers
/// how many it announced.
#[test]
fn announce_org_moves_bumps_only_the_moved_rows() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    s.upsert_host("g").unwrap();
    let a = s.add_org("A", None, false).unwrap().id;
    let on_h = s
        .upsert_session("dev-h", "h", None, None, 1, 1, "running", None)
        .unwrap();
    let ghost_h = s
        .upsert_session("old-h", "h", None, None, 1, 1, "running", None)
        .unwrap();
    s.mark_session_killed(ghost_h, 5).unwrap();
    let on_g = s
        .upsert_session("dev-g", "g", None, None, 1, 1, "running", None)
        .unwrap();
    let version = |id: i64| -> i64 {
        s.conn
            .query_row(
                "SELECT row_version FROM sessions WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
    };
    let before = s.session_orgs().unwrap();
    assert_eq!(s.announce_org_moves(&before).unwrap(), 0, "nothing moved");
    let (vh, vg, vghost) = (version(on_h), version(on_g), version(ghost_h));
    s.set_host_org("h", Some(a)).unwrap();
    assert_eq!(s.announce_org_moves(&before).unwrap(), 2);
    assert!(version(on_h) > vh && version(ghost_h) > vghost);
    assert_eq!(version(on_g), vg, "g's row did not move");
    assert_eq!(s.get_session_by_id(on_h).unwrap().unwrap().org_id, Some(a));
    assert!(s.conn.is_autocommit(), "the transaction was committed");
}

/// The migration over a database from before M5: every row lands
/// unassigned (the default org is "none"), and nothing reads differently.
#[test]
fn existing_rows_land_unassigned_and_read_as_before() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let pid = s.upsert_project("acme", "api", "/src/api").unwrap();
    let sid = s
        .upsert_session("dev", "h", Some(pid), None, 1, 1, "running", None)
        .unwrap();
    s.link_session_work(sid, WorkTarget::Key("ABC-1"), "manual")
        .unwrap();
    let gone = s
        .upsert_session("old", "h", Some(pid), None, 1, 1, "running", None)
        .unwrap();
    s.link_session_work(gone, WorkTarget::Key("ABC-2"), "manual")
        .unwrap();
    s.delete_session(gone).unwrap();
    let before = s.get_session_by_id(sid).unwrap().unwrap();
    // Roll back to 49 and drop what 050 made, as an M4 database would be.
    s.conn
        .execute_batch(
            "DROP TRIGGER trg_work_links_snap_org; DROP TABLE org_rules; DROP TABLE orgs; \
             ALTER TABLE hosts DROP COLUMN org_id; ALTER TABLE work_links DROP COLUMN snap_org_id; \
             DELETE FROM schema_version WHERE version >= 50;",
        )
        .unwrap();
    s.migrate().unwrap();
    assert_eq!(
        s.schema_version().unwrap(),
        crate::store::LATEST_SCHEMA_VERSION
    );
    let after = s.get_session_by_id(sid).unwrap().unwrap();
    assert_eq!(after.org_id, None);
    assert_eq!(after.work, before.work);
    assert!(after.eq_ignoring_row_version(&before));
    assert!(s.list_orgs().unwrap().is_empty());
    assert_eq!(s.host_org("h").unwrap(), None);
    let mut ended = s.ended_work_links_for_key("ABC-2").unwrap();
    s.fill_link_orgs(&mut ended).unwrap();
    assert_eq!(ended[0].org_id, None);
}

/// Work graph M5: a tracker never binds — nor fetches — a bare key for a
/// session of another org; unassigned and same-org sessions bind as before.
#[test]
fn sync_never_binds_or_fetches_across_orgs() {
    let s = Store::open_in_memory().unwrap();
    for h in ["ha", "hb", "hn"] {
        s.upsert_host(h).unwrap();
    }
    let a = s.add_org("A", None, false).unwrap();
    let b = s.add_org("B", None, false).unwrap();
    s.set_host_org("ha", Some(a.id)).unwrap();
    s.set_host_org("hb", Some(b.id)).unwrap();
    let t = s
        .add_tracker("jira", "B Jira", "https://bravo.atlassian.net")
        .unwrap();
    s.set_tracker_org(t.id, Some(b.id)).unwrap();
    s.set_tracker_probe(
        t.id,
        None,
        &crate::store::TrackerConfig {
            key_prefixes: vec!["BB".into()],
            ..Default::default()
        },
    )
    .unwrap();
    let sess = |name: &str, host: &str, key: &str| {
        let id = s
            .upsert_session(name, host, None, None, 1, 1, "running", None)
            .unwrap();
        s.link_session_work(id, WorkTarget::Key(key), "manual")
            .unwrap();
        id
    };
    let on_a = sess("a", "ha", "BB-9");
    let on_b = sess("b", "hb", "BB-8");
    let on_n = sess("n", "hn", "BB-7");
    let keys = s.unbound_ref_keys(t.id, 10).unwrap();
    assert!(!keys.contains(&"BB-9".to_string()), "{keys:?}");
    assert!(keys.contains(&"BB-8".to_string()) && keys.contains(&"BB-7".to_string()));
    for (ext, key) in [("9", "BB-9"), ("8", "BB-8"), ("7", "BB-7")] {
        crate::store::test_support::tracker_item(&s, t.id, ext, key, "t");
    }
    s.bind_tracker_refs(t.id).unwrap();
    let item_of = |sid: i64| s.session_work_links(sid).unwrap()[0].item_id;
    assert_eq!(item_of(on_a), None, "A's bare key stays bare");
    assert!(item_of(on_b).is_some());
    assert!(item_of(on_n).is_some(), "unassigned binds as before");
}
