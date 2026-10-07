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
    fn scope(&self, host: &str) -> OrgScope {
        OrgScope::for_host(&self.store.lock().unwrap(), host).unwrap()
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
    let ids = |scope: &OrgScope| -> Vec<i64> {
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
    assert_eq!(ids(&OrgScope::All).len(), 3);
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
        &OrgScope::All,
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
