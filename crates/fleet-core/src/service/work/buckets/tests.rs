//! The bucket surface's org fence and the degradation rule.

use super::*;
use crate::store::TrackerItemWrite;

struct Fx {
    store: Mutex<Store>,
    org_a: i64,
    org_b: i64,
    /// A ticket of org A's tracker.
    ticket_a: i64,
    tracker_a: i64,
}

impl Fx {
    fn scope(&self, host: &str) -> ViewScope {
        ViewScope::internal()
            .with_org(OrgScope::for_host(&self.store.lock().unwrap(), host).unwrap())
    }

    fn bucket(&self, kind: &str, name: &str, org: Option<i64>) -> i64 {
        self.store
            .lock()
            .unwrap()
            .create_bucket(&NewBucket {
                kind,
                name,
                org_id: org,
                ..Default::default()
            })
            .unwrap()
            .id
    }
}

fn fixture() -> Fx {
    let s = Store::open_in_memory().unwrap();
    for h in ["h-a", "h-b"] {
        s.upsert_host(h).unwrap();
    }
    let a = s.add_org("A", None, false).unwrap().id;
    let b = s.add_org("B", None, false).unwrap().id;
    s.set_host_org("h-a", Some(a)).unwrap();
    s.set_host_org("h-b", Some(b)).unwrap();
    let t = s
        .add_tracker("jira", "A Jira", "https://alpha.atlassian.net")
        .unwrap()
        .id;
    s.set_tracker_org(t, Some(a)).unwrap();
    let ticket_a = s
        .upsert_tracker_item(
            t,
            &TrackerItemWrite {
                external_id: "1".into(),
                key: Some("AA-1".into()),
                title: "Alpha".into(),
                status_name: "To Do".into(),
                status_category: "todo".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
    Fx {
        store: Mutex::new(s),
        org_a: a,
        org_b: b,
        ticket_a,
        tracker_a: t,
    }
}

fn link_args(action: &str, bucket: i64, item: i64) -> WorkLinkArgs {
    WorkLinkArgs {
        action: action.into(),
        bucket_id: Some(bucket),
        item_id: Some(item),
        ..Default::default()
    }
}

#[test]
fn a_host_lists_its_orgs_buckets_and_unassigned_ones_only() {
    let fx = fixture();
    let a = fx.bucket("sprint", "A1", Some(fx.org_a));
    let b = fx.bucket("sprint", "B1", Some(fx.org_b));
    let none = fx.bucket("release", "shared", None);
    let ids = |scope: &ViewScope| -> Vec<i64> {
        buckets(&fx.store, scope, None)
            .unwrap()
            .into_iter()
            .map(|b| b.id)
            .collect()
    };
    let mut seen = ids(&fx.scope("h-a"));
    seen.sort();
    assert_eq!(seen, [a, none]);
    assert!(!ids(&fx.scope("h-b")).contains(&a));
    assert_eq!(ids(&ViewScope::internal()).len(), 3);
    let _ = b;
}

#[test]
fn another_orgs_bucket_answers_as_unknown() {
    let fx = fixture();
    let a = fx.bucket("sprint", "A1", Some(fx.org_a));
    let got = bucket(&fx.store, &fx.scope("h-b"), a).unwrap_err();
    let unknown = bucket(&fx.store, &fx.scope("h-b"), 987_654).unwrap_err();
    assert_eq!(got.code, codes::E_NOTFOUND);
    assert_eq!(
        got.message.replace(&a.to_string(), "N"),
        unknown.message.replace("987654", "N")
    );
    // Through membership too.
    let e = bucket_add(
        &link_args("bucket_add", a, fx.ticket_a),
        &fx.store,
        &fx.scope("h-b"),
    )
    .unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
}

#[test]
fn a_member_the_caller_cannot_see_is_left_out() {
    let fx = fixture();
    let shared = fx.bucket("release", "shared", None);
    bucket_add(
        &link_args("bucket_add", shared, fx.ticket_a),
        &fx.store,
        &ViewScope::internal(),
    )
    .unwrap();
    assert_eq!(
        bucket(&fx.store, &fx.scope("h-a"), shared)
            .unwrap()
            .members
            .len(),
        1
    );
    assert!(bucket(&fx.store, &fx.scope("h-b"), shared)
        .unwrap()
        .members
        .is_empty());
}

#[test]
fn membership_round_trips() {
    let fx = fixture();
    let a = fx.bucket("sprint", "A1", Some(fx.org_a));
    let row = bucket_add(
        &link_args("bucket_add", a, fx.ticket_a),
        &fx.store,
        &fx.scope("h-a"),
    )
    .unwrap();
    assert_eq!(row.total, 1);
    let row = bucket_remove(
        &link_args("bucket_remove", a, fx.ticket_a),
        &fx.store,
        &fx.scope("h-a"),
    )
    .unwrap();
    assert_eq!(row.total, 0);
}

fn admin_args(action: &str) -> WorkAdminArgs {
    WorkAdminArgs {
        action: action.into(),
        ..Default::default()
    }
}

#[test]
fn a_provider_without_sprints_or_versions_only_loses_adoption() {
    let fx = fixture();
    let s = fx.store.lock().unwrap();
    let gh = s
        .add_tracker("github", "GitHub", "https://github.com")
        .unwrap()
        .id;
    let asana = s
        .add_tracker("asana", "Asana", "https://app.asana.com")
        .unwrap()
        .id;
    let sprint = admin(
        BucketAction::Create,
        &WorkAdminArgs {
            kind: Some("sprint".into()),
            name: Some("S1".into()),
            ..admin_args("bucket_create")
        },
        &s,
    )
    .unwrap()["bucket"]["id"]
        .as_i64()
        .unwrap();
    let release = admin(
        BucketAction::Create,
        &WorkAdminArgs {
            kind: Some("release".into()),
            name: Some("1.0".into()),
            ..admin_args("bucket_create")
        },
        &s,
    )
    .unwrap()["bucket"]["id"]
        .as_i64()
        .unwrap();
    let adopt = |bucket: i64, tracker: i64| {
        admin(
            BucketAction::Adopt,
            &WorkAdminArgs {
                bucket_id: Some(bucket),
                tracker_id: Some(tracker),
                external_id: Some("x".into()),
                ..admin_args("bucket_adopt")
            },
            &s,
        )
    };
    // GitHub: no sprints, but milestones.
    assert_eq!(adopt(sprint, gh).unwrap_err().code, codes::E_INVALID);
    adopt(release, gh).unwrap();
    // Asana: neither.
    assert_eq!(adopt(release, asana).unwrap_err().code, codes::E_INVALID);
    // Jira: both.
    adopt(sprint, fx.tracker_a).unwrap();
}

#[test]
fn activating_a_second_sprint_warns_and_does_not_refuse() {
    let fx = fixture();
    let s = fx.store.lock().unwrap();
    let make = |name: &str| {
        let id = s
            .create_bucket(&NewBucket {
                kind: "sprint",
                name,
                ..Default::default()
            })
            .unwrap()
            .id;
        admin(
            BucketAction::Update,
            &WorkAdminArgs {
                bucket_id: Some(id),
                state: Some("active".into()),
                ..admin_args("bucket_update")
            },
            &s,
        )
        .unwrap()
    };
    assert!(make("S1").get("warning").is_none());
    let second = make("S2");
    assert_eq!(second["bucket"]["state"], "active");
    assert!(second["warning"].as_str().unwrap().contains("S1"));
}

#[test]
fn the_admin_actions_parse_and_delete_is_a_removal() {
    use crate::service::trackers::admin::AdminAction;
    for name in BucketAction::NAMES {
        assert!(
            AdminAction::NAMES.contains(name),
            "{name} is not in AdminAction::NAMES"
        );
        assert!(matches!(
            AdminAction::parse(name),
            Ok(AdminAction::Bucket(_))
        ));
    }
    assert!(AdminAction::parse("bucket_delete").unwrap().is_removal());
    assert!(!AdminAction::parse("bucket_close").unwrap().is_removal());
}

/// A person's device, bound to `org` or to none, scoped the way a request is.
fn person(store: &Mutex<Store>, org: Option<i64>, id: i64) -> ViewScope {
    use crate::mcp::auth::{Caller, ClientRef, TokenMode};
    Caller {
        api: None,
        host_alias: None,
        client: Some(ClientRef {
            id: 7,
            name: "phone".into(),
            trusted: false,
            org_id: org,
            person_id: Some(id),
        }),
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
    }
    .view_scope(&lock(store).unwrap())
    .unwrap()
}

fn op(action: &str) -> WorkAdminArgs {
    WorkAdminArgs {
        action: action.into(),
        ..Default::default()
    }
}

fn create_op(name: &str, org: Option<i64>) -> WorkAdminArgs {
    WorkAdminArgs {
        kind: Some("sprint".into()),
        name: Some(name.into()),
        org_id: org,
        ..op("bucket_create")
    }
}

fn created(v: serde_json::Value) -> BucketRow {
    serde_json::from_value(v["bucket"].clone()).unwrap()
}

#[test]
fn a_personal_sprint_is_its_persons_alone() {
    let fx = fixture();
    let (ana, bo) = {
        let s = lock(&fx.store).unwrap();
        let ana = s.create_person("ana", None).unwrap().id;
        let bo = s.create_person("bo", None).unwrap().id;
        s.set_org_member(fx.org_a, ana, "viewer", None).unwrap();
        s.set_org_member(fx.org_a, bo, "viewer", None).unwrap();
        (ana, bo)
    };
    let as_ana = person(&fx.store, Some(fx.org_a), ana);
    let as_bo = person(&fx.store, Some(fx.org_a), bo);
    // A viewer plans nothing of the team's, but keeps sprints of their own.
    let mine = created(person_admin(&create_op("Mine", None), &fx.store, &as_ana).unwrap());
    assert_eq!(mine.owner_person_id, Some(ana));
    assert_eq!(mine.org_id, None);
    // Bo may name one the same: the name is unique per owner.
    let bos = created(person_admin(&create_op("Mine", None), &fx.store, &as_bo).unwrap());
    assert_ne!(bos.id, mine.id);
    let listed = |scope: &ViewScope| -> Vec<i64> {
        buckets(&fx.store, scope, None)
            .unwrap()
            .into_iter()
            .map(|b| b.id)
            .collect()
    };
    assert!(listed(&as_ana).contains(&mine.id));
    assert!(
        !listed(&as_ana).contains(&bos.id),
        "Bo's is not Ana's to see"
    );
    // Unknown to everyone else, read or write alike.
    let read = bucket(&fx.store, &as_bo, mine.id).unwrap_err();
    assert_eq!(read.code, codes::E_NOTFOUND);
    let del = WorkAdminArgs {
        bucket_id: Some(mine.id),
        ..op("bucket_delete")
    };
    assert_eq!(
        person_admin(&del, &fx.store, &as_bo).unwrap_err().code,
        codes::E_NOTFOUND
    );
    let add = bucket_add(
        &link_args("bucket_add", mine.id, fx.ticket_a),
        &fx.store,
        &as_bo,
    );
    assert_eq!(add.unwrap_err().code, codes::E_NOTFOUND);
    // Its person plans into it, beside the team's sprint the item is in.
    let team = fx.bucket("sprint", "Team", Some(fx.org_a));
    bucket_add(
        &link_args("bucket_add", team, fx.ticket_a),
        &fx.store,
        &ViewScope::internal(),
    )
    .unwrap();
    bucket_add(
        &link_args("bucket_add", mine.id, fx.ticket_a),
        &fx.store,
        &as_ana,
    )
    .unwrap();
    // Adopting a tracker's sprint is never a personal bucket's.
    let adopt = WorkAdminArgs {
        bucket_id: Some(mine.id),
        tracker_id: Some(fx.tracker_a),
        external_id: Some("S1".into()),
        ..op("bucket_adopt")
    };
    assert_eq!(
        person_admin(&adopt, &fx.store, &as_ana).unwrap_err().code,
        codes::E_FORBIDDEN
    );
    // And its person deletes it.
    person_admin(
        &WorkAdminArgs {
            bucket_id: Some(mine.id),
            ..op("bucket_delete")
        },
        &fx.store,
        &as_ana,
    )
    .unwrap();
}

#[test]
fn an_orgs_sprints_are_its_admins_and_its_members_when_the_org_allows() {
    let fx = fixture();
    let (adm, mem, view) = {
        let s = lock(&fx.store).unwrap();
        let ids: Vec<i64> = ["adm", "mem", "view"]
            .iter()
            .map(|n| s.create_person(n, None).unwrap().id)
            .collect();
        s.set_org_member(fx.org_a, ids[0], "admin", None).unwrap();
        s.set_org_member(fx.org_a, ids[1], "member", None).unwrap();
        s.set_org_member(fx.org_a, ids[2], "viewer", None).unwrap();
        (ids[0], ids[1], ids[2])
    };
    let scope = |p| person(&fx.store, Some(fx.org_a), p);
    let team =
        created(person_admin(&create_op("S1", Some(fx.org_a)), &fx.store, &scope(adm)).unwrap());
    assert_eq!(team.owner_person_id, None);
    assert_eq!(team.org_id, Some(fx.org_a));
    // A member and a viewer read it but do not plan it, by default.
    for p in [mem, view] {
        assert!(bucket(&fx.store, &scope(p), team.id).is_ok());
        let e = person_admin(&create_op("S2", Some(fx.org_a)), &fx.store, &scope(p)).unwrap_err();
        assert_eq!(e.code, codes::E_FORBIDDEN);
    }
    lock(&fx.store)
        .unwrap()
        .set_org_setting(
            fx.org_a,
            crate::service::settings::WORK_MEMBERS_PLAN_SPRINTS,
            Some("true"),
        )
        .unwrap();
    // The org turned it on: a member plans; a viewer still does not.
    person_admin(&create_op("S2", Some(fx.org_a)), &fx.store, &scope(mem)).unwrap();
    let start = WorkAdminArgs {
        bucket_id: Some(team.id),
        state: Some("active".into()),
        ..op("bucket_update")
    };
    person_admin(&start, &fx.store, &scope(mem)).unwrap();
    let e = person_admin(&create_op("S3", Some(fx.org_a)), &fx.store, &scope(view)).unwrap_err();
    assert_eq!(e.code, codes::E_FORBIDDEN);
    // Another org's sprint is unknown, whatever the role here.
    let b = fx.bucket("sprint", "B1", Some(fx.org_b));
    let close = WorkAdminArgs {
        bucket_id: Some(b),
        ..op("bucket_close")
    };
    assert_eq!(
        person_admin(&close, &fx.store, &scope(adm))
            .unwrap_err()
            .code,
        codes::E_NOTFOUND
    );
}
