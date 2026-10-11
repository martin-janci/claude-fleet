//! `work { action: stats }`: the board's tasks counted (persona dashboards).

use super::*;
use crate::service::orgs::OrgScope;
use crate::service::work::view::{tree, TreeArgs};
use crate::store::{BucketRow, NewBucket, TrackerConfig, TrackerItemWrite};

struct W {
    st: Mutex<Store>,
    org_a: i64,
    items: Vec<i64>,
}

fn vs(scope: &OrgScope) -> ViewScope {
    ViewScope::internal().with_org(scope.clone())
}

/// Org A's tracker with five tickets: two to do, one in progress, one in
/// review, one done; two assigned to Ana.
fn world() -> W {
    let s = Store::open_in_memory().unwrap();
    let a = s.add_org("Acme", None, false).unwrap();
    let t = s
        .add_tracker("jira", "Jira", "https://acme.atlassian.net")
        .unwrap();
    s.set_tracker_org(t.id, Some(a.id)).unwrap();
    s.set_tracker_probe(
        t.id,
        None,
        &TrackerConfig {
            key_prefixes: vec!["TK".into()],
            ..Default::default()
        },
    )
    .unwrap();
    let rows = [
        ("TK-1", "To Do", "todo", Some("Ana")),
        ("TK-2", "To Do", "todo", None),
        ("TK-3", "In Progress", "in_progress", Some("Ana")),
        ("TK-4", "In Review", "in_progress", Some("Ben")),
        ("TK-5", "Done", "done", Some("Ben")),
    ];
    let items = rows
        .iter()
        .enumerate()
        .map(|(i, (key, name, cat, who))| {
            s.upsert_tracker_item(
                t.id,
                &TrackerItemWrite {
                    external_id: format!("{i}"),
                    key: Some((*key).into()),
                    title: format!("{key} title"),
                    status_name: (*name).into(),
                    status_category: (*cat).into(),
                    containers: vec!["TP".into()],
                    assignees: who.map(|w| vec![w.to_string()]).unwrap_or_default(),
                    ..Default::default()
                },
            )
            .unwrap()
            .id
        })
        .collect();
    W {
        st: Mutex::new(s),
        org_a: a.id,
        items,
    }
}

fn filters(by: &str) -> WorkTreeFilters {
    WorkTreeFilters {
        group_by: Some(by.into()),
        archived: Some(true),
        ..Default::default()
    }
}

#[test]
fn stats_agree_with_the_board() {
    let w = world();
    let f = filters("org");
    let st = stats(&w.st, &vs(&OrgScope::All), Some(&f)).unwrap();
    let page = tree(
        &w.st,
        &vs(&OrgScope::All),
        &TreeArgs {
            filters: f,
            limit: Some(200),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(st.total, page.total);
    let mut by_stage = StageCounts::default();
    for t in &page.tasks {
        by_stage.add(&t.stage);
    }
    assert_eq!(st.stages, by_stage, "the board's stages, counted");
    assert_eq!(st.stages.done, 1);
    assert_eq!(st.stages.in_review, 1);
    assert_eq!(st.pct_done, 20);
}

#[test]
fn stats_group_by_person_largest_first_and_none_last_among_equals() {
    let w = world();
    let st = stats(&w.st, &vs(&OrgScope::All), Some(&filters("person"))).unwrap();
    assert_eq!(st.group_by, "person");
    let labels: Vec<(&str, u32)> = st
        .groups
        .iter()
        .map(|g| (g.group.label.as_str(), g.total))
        .collect();
    assert_eq!(labels, vec![("Ana", 2), ("Ben", 2), ("No assignee", 1)]);
    let ben = &st.groups[1];
    assert_eq!(ben.stages.done, 1);
    assert_eq!(ben.pct_done, 50);
}

#[test]
fn a_sprint_group_carries_its_health_and_scope_change() {
    let w = world();
    let sprint = {
        let s = w.st.lock().unwrap();
        let id = s
            .create_bucket(&NewBucket {
                kind: "sprint",
                name: "Sprint 42",
                org_id: Some(w.org_a),
                owner_person_id: None,
                starts_at: Some(0),
                ends_at: Some(i64::MAX / 2),
                goal: Some("Checkout"),
            })
            .unwrap()
            .id;
        for item in &w.items[..3] {
            s.add_bucket_item(id, *item).unwrap();
        }
        id
    };
    let st = stats(&w.st, &vs(&OrgScope::All), Some(&filters("sprint"))).unwrap();
    let g = st
        .groups
        .iter()
        .find(|g| g.group.id == format!("sprint:{sprint}"))
        .unwrap();
    assert_eq!(g.total, 3);
    let h = g.bucket.as_ref().unwrap();
    assert_eq!(h.bucket_id, sprint);
    assert_eq!(h.goal.as_deref(), Some("Checkout"));
    assert_eq!(h.added_after_start, 3, "added after a start at 0");
    assert!(h.pct_time_elapsed.is_some());
    let none = st.groups.iter().find(|g| g.group.id == "none").unwrap();
    assert!(none.bucket.is_none());
}

#[test]
fn another_orgs_client_counts_nothing_it_cannot_open() {
    let w = world();
    let other = w.st.lock().unwrap().add_org("Beta", None, false).unwrap();
    let st = stats(
        &w.st,
        &vs(&OrgScope::Org {
            org: other.id,
            sees_unassigned: false,
        }),
        Some(&filters("org")),
    )
    .unwrap();
    assert_eq!(st.total, 0);
    assert!(st.groups.is_empty());
}

fn bucket(starts: Option<i64>, ends: Option<i64>) -> BucketRow {
    serde_json::from_value(serde_json::json!({
        "id": 1, "kind": "sprint", "name": "S", "state": "active",
        "starts_at": starts, "ends_at": ends,
        "created_at": 0, "updated_at": 0, "version": 1
    }))
    .unwrap()
}

#[test]
fn health_reads_time_gone_and_scope_change_from_dates() {
    let b = bucket(Some(100), Some(200));
    let members = [(50, None), (150, None), (60, Some(170))];
    let h = health_of(&b, &members, 150);
    assert_eq!(h.pct_time_elapsed, Some(50));
    assert_eq!(h.added_after_start, 1);
    assert_eq!(h.removed_after_start, 1);
    assert_eq!(health_of(&b, &members, 10_000).pct_time_elapsed, Some(100));
    assert_eq!(
        health_of(&bucket(None, Some(200)), &[], 150).pct_time_elapsed,
        None
    );
}
