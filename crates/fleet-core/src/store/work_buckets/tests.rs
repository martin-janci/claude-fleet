//! Sprints and releases in the store (design 2026-09-28 §1, §5): the one
//! current sprint, history on removal and close, the org rule, and adoption
//! with its withdrawal.

use super::*;
use crate::store::TrackerItemWrite;

fn store() -> Store {
    Store::open_in_memory().unwrap()
}

fn sprint(s: &Store, name: &str) -> BucketRow {
    s.create_bucket(&NewBucket {
        kind: "sprint",
        name,
        ..Default::default()
    })
    .unwrap()
}

fn release(s: &Store, name: &str) -> BucketRow {
    s.create_bucket(&NewBucket {
        kind: "release",
        name,
        ..Default::default()
    })
    .unwrap()
}

fn local(s: &Store, title: &str) -> i64 {
    s.create_local_work_item(None, title).unwrap().id
}

fn ticket(s: &Store, tracker: i64, ext: &str, iteration: Option<&str>, versions: &[&str]) -> i64 {
    s.upsert_tracker_item(
        tracker,
        &TrackerItemWrite {
            external_id: ext.into(),
            key: Some(format!("ABC-{ext}")),
            title: format!("ticket {ext}"),
            status_name: "To Do".into(),
            status_category: "todo".into(),
            iteration: iteration.map(str::to_string),
            versions: versions.iter().map(|v| v.to_string()).collect(),
            ..Default::default()
        },
    )
    .unwrap()
    .id
}

fn jira(s: &Store) -> i64 {
    s.add_tracker("jira", "Jira", "https://acme.atlassian.net")
        .unwrap()
        .id
}

fn members(s: &Store, bucket: i64) -> Vec<(i64, String)> {
    s.bucket_members(bucket, false)
        .unwrap()
        .into_iter()
        .map(|m| (m.item.id, m.source))
        .collect()
}

#[test]
fn a_sprint_and_a_release_are_created_planned() {
    let s = store();
    let sp = sprint(&s, "Sprint 24");
    assert_eq!((sp.kind.as_str(), sp.state.as_str()), ("sprint", "planned"));
    assert_eq!((sp.total, sp.done, sp.version), (0, 0, 1));
    let r = release(&s, "0.3.0");
    assert_eq!(r.kind, "release");
    assert_eq!(s.list_buckets(None).unwrap().len(), 2);
    assert_eq!(s.list_buckets(Some("release")).unwrap()[0].id, r.id);
}

#[test]
fn a_name_is_unique_per_kind_and_org_only() {
    let s = store();
    let a = s.add_org("A", None, false).unwrap().id;
    let b = s.add_org("B", None, false).unwrap().id;
    for org in [Some(a), Some(b), None] {
        s.create_bucket(&NewBucket {
            kind: "sprint",
            name: "Sprint 24",
            org_id: org,
            ..Default::default()
        })
        .unwrap();
    }
    // A release may share a sprint's name.
    release(&s, "Sprint 24");
    let e = s
        .create_bucket(&NewBucket {
            kind: "sprint",
            name: "Sprint 24",
            org_id: Some(a),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(e.code, codes::E_EXISTS);
    let e = s
        .create_bucket(&NewBucket {
            kind: "sprint",
            name: "Sprint 24",
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(e.code, codes::E_EXISTS, "unassigned counts as one org");
}

#[test]
fn bad_input_is_refused() {
    let s = store();
    for (kind, name, starts, ends) in [
        ("epic", "x", None, None),
        ("sprint", "  ", None, None),
        ("sprint", "two\nlines", None, None),
        ("sprint", "backwards", Some(200), Some(100)),
        ("release", "has a start", Some(100), None),
    ] {
        let e = s
            .create_bucket(&NewBucket {
                kind,
                name,
                starts_at: starts,
                ends_at: ends,
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "{kind} {name:?}");
    }
}

#[test]
fn an_item_is_in_one_current_sprint_and_several_releases() {
    let s = store();
    let (s1, s2) = (sprint(&s, "S1"), sprint(&s, "S2"));
    let (r1, r2) = (release(&s, "1.0"), release(&s, "1.1"));
    let item = local(&s, "auth refactor");
    s.add_bucket_item(s1.id, item).unwrap();
    // Idempotent.
    assert_eq!(s.add_bucket_item(s1.id, item).unwrap().total, 1);
    let e = s.add_bucket_item(s2.id, item).unwrap_err();
    assert_eq!(e.code, codes::E_CONFLICT);
    assert!(e.message.contains("S1"), "names the sprint: {}", e.message);
    s.add_bucket_item(r1.id, item).unwrap();
    s.add_bucket_item(r2.id, item).unwrap();
    let mine: Vec<String> = s
        .item_buckets(item)
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert_eq!(mine, ["S1", "1.0", "1.1"]);
    // Out of S1, it may join S2.
    assert!(s.remove_bucket_item(s1.id, item).unwrap());
    assert!(!s.remove_bucket_item(s1.id, item).unwrap());
    s.add_bucket_item(s2.id, item).unwrap();
}

#[test]
fn removal_keeps_the_history() {
    let s = store();
    let sp = sprint(&s, "S1");
    let item = local(&s, "x");
    s.add_bucket_item(sp.id, item).unwrap();
    s.remove_bucket_item(sp.id, item).unwrap();
    assert!(s.bucket_members(sp.id, false).unwrap().is_empty());
    let past = s.bucket_members(sp.id, true).unwrap();
    assert_eq!(past.len(), 1);
    assert!(past[0].removed_at.is_some());
}

#[test]
fn the_roll_up_counts_done_members() {
    let s = store();
    let sp = sprint(&s, "S1");
    let (a, b) = (local(&s, "a"), local(&s, "b"));
    s.add_bucket_item(sp.id, a).unwrap();
    s.add_bucket_item(sp.id, b).unwrap();
    s.set_item_status(a, "done").unwrap();
    let row = s.get_bucket(sp.id).unwrap().unwrap();
    assert_eq!((row.total, row.done), (2, 1));
}

#[test]
fn work_is_not_planned_across_organisations() {
    let s = store();
    let a = s.add_org("A", None, false).unwrap().id;
    let b = s.add_org("B", None, false).unwrap().id;
    let t = jira(&s);
    s.set_tracker_org(t, Some(a)).unwrap();
    let item = ticket(&s, t, "1", None, &[]);
    let of_b = s
        .create_bucket(&NewBucket {
            kind: "sprint",
            name: "B's sprint",
            org_id: Some(b),
            ..Default::default()
        })
        .unwrap();
    let e = s.add_bucket_item(of_b.id, item).unwrap_err();
    assert_eq!(e.code, codes::E_FORBIDDEN);
    // Unassigned on either side is no conflict.
    s.add_bucket_item(sprint(&s, "nobody's").id, item).unwrap();
    s.add_bucket_item(of_b.id, local(&s, "unassigned work"))
        .unwrap();
}

#[test]
fn a_sprint_moves_forward_only_and_closes_through_close() {
    let s = store();
    let sp = sprint(&s, "S1");
    let active = s
        .update_bucket(
            sp.id,
            Some(1),
            &BucketPatch {
                state: Some("active".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!((active.state.as_str(), active.version), ("active", 2));
    // A stale version is a conflict, and writes nothing.
    let e = s
        .update_bucket(
            sp.id,
            Some(1),
            &BucketPatch {
                name: Some("renamed".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(e.code, codes::E_CONFLICT);
    for state in ["planned", "closed", "released"] {
        let e = s
            .update_bucket(
                sp.id,
                None,
                &BucketPatch {
                    state: Some(state.into()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "{state}");
    }
}

#[test]
fn releasing_stamps_shipped_at_and_shipped_ref_is_a_persons() {
    let s = store();
    let r = release(&s, "0.3.0");
    let out = s
        .update_bucket(
            r.id,
            None,
            &BucketPatch {
                state: Some("released".into()),
                shipped_ref: Some(Some("v0.3.0".into())),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(out.state, "released");
    assert!(out.shipped_at.is_some());
    assert_eq!(out.shipped_ref.as_deref(), Some("v0.3.0"));
    let e = s
        .update_bucket(
            sprint(&s, "S").id,
            None,
            &BucketPatch {
                shipped_ref: Some(Some("v1".into())),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
}

#[test]
fn closing_a_sprint_ends_every_membership_and_carries_the_unfinished() {
    let s = store();
    let (s1, s2) = (sprint(&s, "S1"), sprint(&s, "S2"));
    let (done, open, also_open) = (local(&s, "done"), local(&s, "open"), local(&s, "open 2"));
    for i in [done, open, also_open] {
        s.add_bucket_item(s1.id, i).unwrap();
    }
    s.set_item_status(done, "done").unwrap();
    let out = s.close_sprint(s1.id, None, Some(s2.id), None).unwrap();
    assert_eq!(out.bucket.state, "closed");
    assert_eq!(out.ended, [done, open, also_open]);
    assert_eq!(out.carried, [open, also_open]);
    assert_eq!(
        members(&s, s2.id),
        [(open, "manual".into()), (also_open, "manual".into())]
    );
    // The closed sprint keeps its history and takes nothing new.
    assert_eq!(s.bucket_members(s1.id, true).unwrap().len(), 3);
    let e = s.add_bucket_item(s1.id, local(&s, "late")).unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    let e = s.close_sprint(s1.id, None, None, None).unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
}

#[test]
fn a_person_confirms_which_unfinished_items_carry() {
    let s = store();
    let (s1, s2) = (sprint(&s, "S1"), sprint(&s, "S2"));
    let (a, b, done) = (local(&s, "a"), local(&s, "b"), local(&s, "done"));
    for i in [a, b, done] {
        s.add_bucket_item(s1.id, i).unwrap();
    }
    s.set_item_status(done, "done").unwrap();
    // A finished item is not carry-over.
    let e = s
        .close_sprint(s1.id, None, Some(s2.id), Some(&[a, done]))
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    let out = s
        .close_sprint(s1.id, None, Some(s2.id), Some(&[b]))
        .unwrap();
    assert_eq!(out.carried, [b]);
    assert_eq!(members(&s, s2.id), [(b, "manual".into())]);
}

#[test]
fn closing_with_no_target_leaves_the_work_with_no_sprint() {
    let s = store();
    let s1 = sprint(&s, "S1");
    let a = local(&s, "a");
    s.add_bucket_item(s1.id, a).unwrap();
    let out = s.close_sprint(s1.id, None, None, None).unwrap();
    assert!(out.carried.is_empty());
    assert!(s.item_buckets(a).unwrap().is_empty());
    let e = s
        .close_sprint(sprint(&s, "S2").id, None, None, Some(&[a]))
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID, "carry names items but no sprint");
}

#[test]
fn a_tracker_sprint_and_version_adopt_their_items() {
    let s = store();
    let t = jira(&s);
    let in_24 = ticket(&s, t, "1", Some("ABC Sprint 24"), &["0.3.0"]);
    let other = ticket(&s, t, "2", Some("ABC Sprint 23"), &[]);
    let sp = sprint(&s, "Sprint 24");
    let r = release(&s, "0.3.0");
    // Linking adopts what the cache already holds.
    s.add_bucket_ref(sp.id, t, "ABC Sprint 24", Some("ABC Sprint 24"))
        .unwrap();
    s.add_bucket_ref(r.id, t, "0.3.0", None).unwrap();
    assert_eq!(members(&s, sp.id), [(in_24, "adopted".into())]);
    assert_eq!(members(&s, r.id), [(in_24, "adopted".into())]);
    // A sync that sees the item move in adopts it.
    ticket(&s, t, "2", Some("ABC Sprint 24"), &["0.3.0"]);
    assert_eq!(members(&s, sp.id).len(), 2);
    // ... and one that sees it move out withdraws it, with history.
    ticket(&s, t, "2", Some("ABC Sprint 25"), &[]);
    assert_eq!(members(&s, sp.id), [(in_24, "adopted".into())]);
    assert_eq!(members(&s, r.id), [(in_24, "adopted".into())]);
    assert_eq!(s.bucket_members(sp.id, true).unwrap().len(), 2);
    let _ = other;
}

#[test]
fn adoption_never_touches_a_persons_decision() {
    let s = store();
    let t = jira(&s);
    let item = ticket(&s, t, "1", Some("ABC Sprint 24"), &[]);
    let sp = sprint(&s, "Sprint 24");
    let mine = sprint(&s, "My sprint");
    // A person planned it elsewhere first: no second sprint.
    s.add_bucket_item(mine.id, item).unwrap();
    s.add_bucket_ref(sp.id, t, "ABC Sprint 24", None).unwrap();
    assert!(members(&s, sp.id).is_empty());
    // A person's membership survives the tracker no longer reporting it.
    s.remove_bucket_item(mine.id, item).unwrap();
    s.add_bucket_item(sp.id, item).unwrap();
    ticket(&s, t, "1", Some("ABC Sprint 25"), &[]);
    assert_eq!(members(&s, sp.id), [(item, "manual".into())]);
    // A membership a person ended is not re-adopted.
    s.remove_bucket_item(sp.id, item).unwrap();
    ticket(&s, t, "1", Some("ABC Sprint 24"), &[]);
    assert!(members(&s, sp.id).is_empty());
}

#[test]
fn a_closed_sprint_adopts_nothing() {
    let s = store();
    let t = jira(&s);
    let sp = sprint(&s, "Sprint 24");
    s.add_bucket_ref(sp.id, t, "ABC Sprint 24", None).unwrap();
    s.close_sprint(sp.id, None, None, None).unwrap();
    ticket(&s, t, "1", Some("ABC Sprint 24"), &[]);
    assert!(members(&s, sp.id).is_empty());
}

#[test]
fn unlinking_withdraws_only_what_the_link_adopted() {
    let s = store();
    let t = jira(&s);
    let adopted = ticket(&s, t, "1", None, &["0.3.0"]);
    let r = release(&s, "0.3.0");
    s.add_bucket_ref(r.id, t, "0.3.0", None).unwrap();
    let manual = local(&s, "release notes");
    s.add_bucket_item(r.id, manual).unwrap();
    assert_eq!(members(&s, r.id).len(), 2);
    s.remove_bucket_ref(r.id, t, "0.3.0").unwrap();
    assert_eq!(members(&s, r.id), [(manual, "manual".into())]);
    let e = s.remove_bucket_ref(r.id, t, "0.3.0").unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    let _ = adopted;
}

#[test]
fn a_tracker_of_another_org_cannot_feed_a_bucket() {
    let s = store();
    let a = s.add_org("A", None, false).unwrap().id;
    let b = s.add_org("B", None, false).unwrap().id;
    let t = jira(&s);
    s.set_tracker_org(t, Some(a)).unwrap();
    let of_b = s
        .create_bucket(&NewBucket {
            kind: "release",
            name: "1.0",
            org_id: Some(b),
            ..Default::default()
        })
        .unwrap();
    let e = s.add_bucket_ref(of_b.id, t, "1.0", None).unwrap_err();
    assert_eq!(e.code, codes::E_FORBIDDEN);
}

#[test]
fn deleting_a_bucket_or_its_items_cascades() {
    let s = store();
    let sp = sprint(&s, "S1");
    let item = local(&s, "a");
    s.add_bucket_item(sp.id, item).unwrap();
    assert!(s.delete_bucket(sp.id).unwrap());
    assert!(!s.delete_bucket(sp.id).unwrap());
    assert!(s.item_buckets(item).unwrap().is_empty());
}

#[test]
fn an_adopted_membership_a_person_ended_stays_ended() {
    let s = store();
    let t = jira(&s);
    let item = ticket(&s, t, "1", Some("ABC Sprint 24"), &[]);
    let sp = sprint(&s, "Sprint 24");
    s.add_bucket_ref(sp.id, t, "ABC Sprint 24", None).unwrap();
    assert_eq!(members(&s, sp.id), [(item, "adopted".into())]);
    s.remove_bucket_item(sp.id, item).unwrap();
    // The next sync that changes the ticket, and a re-run of adoption over
    // the tracker, leave the person's decision alone.
    ticket(&s, t, "1", Some("ABC Sprint 24"), &["0.3.0"]);
    let r = release(&s, "0.3.0");
    s.add_bucket_ref(r.id, t, "0.3.0", None).unwrap();
    assert!(members(&s, sp.id).is_empty());
    // A tracker's withdrawal is not a person's: it re-adopts.
    let other = ticket(&s, t, "2", Some("ABC Sprint 24"), &[]);
    ticket(&s, t, "2", Some("ABC Sprint 25"), &[]);
    ticket(&s, t, "2", Some("ABC Sprint 24"), &[]);
    assert_eq!(members(&s, sp.id), [(other, "adopted".into())]);
}

#[test]
fn a_tracker_moved_to_another_org_stops_feeding_the_bucket() {
    let s = store();
    let a = s.add_org("A", None, false).unwrap().id;
    let b = s.add_org("B", None, false).unwrap().id;
    let t = jira(&s);
    let of_b = s
        .create_bucket(&NewBucket {
            kind: "sprint",
            name: "B's sprint",
            org_id: Some(b),
            ..Default::default()
        })
        .unwrap();
    s.add_bucket_ref(of_b.id, t, "ABC Sprint 24", None).unwrap();
    let before = ticket(&s, t, "1", Some("ABC Sprint 24"), &[]);
    assert_eq!(members(&s, of_b.id), [(before, "adopted".into())]);
    s.set_tracker_org(t, Some(a)).unwrap();
    ticket(&s, t, "2", Some("ABC Sprint 24"), &[]);
    assert_eq!(members(&s, of_b.id), [(before, "adopted".into())]);
}

#[test]
fn a_ref_matches_a_name_with_stray_whitespace_and_stamps_only_itself() {
    let s = store();
    let t = jira(&s);
    let r = release(&s, "1.0");
    s.add_bucket_ref(r.id, t, "v1.0", None).unwrap();
    s.add_bucket_ref(r.id, t, "never reported", None).unwrap();
    let item = ticket(&s, t, "1", None, &["v1.0 "]);
    assert_eq!(members(&s, r.id), [(item, "adopted".into())]);
    let seen: Vec<(String, Option<i64>)> = s
        .conn
        .prepare("SELECT external_id, last_seen_at FROM work_bucket_refs ORDER BY external_id")
        .unwrap()
        .query_map([], |x| Ok((x.get(0)?, x.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(seen[0].1.is_none(), "{seen:?}");
    assert!(seen[1].1.is_some(), "{seen:?}");
    let long = "x".repeat(BUCKET_NAME_MAX_CHARS + 1);
    let e = s.add_bucket_ref(r.id, t, &long, None).unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
}

#[test]
fn unlinking_one_of_two_refs_keeps_what_the_other_justifies() {
    let s = store();
    let t = jira(&s);
    let r = release(&s, "1.0");
    s.add_bucket_ref(r.id, t, "1.0", None).unwrap();
    s.add_bucket_ref(r.id, t, "1.0-rc", None).unwrap();
    let both = ticket(&s, t, "1", None, &["1.0", "1.0-rc"]);
    let rc_only = ticket(&s, t, "2", None, &["1.0-rc"]);
    s.conn
        .execute("UPDATE work_bucket_items SET added_at = 7", [])
        .unwrap();
    s.remove_bucket_ref(r.id, t, "1.0-rc").unwrap();
    let now: Vec<(i64, i64)> = s
        .bucket_members(r.id, false)
        .unwrap()
        .into_iter()
        .map(|m| (m.item.id, m.added_at))
        .collect();
    assert_eq!(now, [(both, 7)]);
    let _ = rc_only;
}

#[test]
fn deleting_an_org_unassigns_its_buckets_even_on_a_name_clash() {
    let s = store();
    let a = s.add_org("A", None, false).unwrap().id;
    sprint(&s, "Sprint 24");
    let of_a = s
        .create_bucket(&NewBucket {
            kind: "sprint",
            name: "Sprint 24",
            org_id: Some(a),
            ..Default::default()
        })
        .unwrap();
    assert!(s.remove_org(a).unwrap());
    let moved = s.get_bucket(of_a.id).unwrap().unwrap();
    assert_eq!(moved.org_id, None);
    assert_eq!(moved.name, format!("Sprint 24 (#{})", of_a.id));
}

#[test]
fn unlinking_withdraws_when_only_a_dormant_ref_remains() {
    let s = store();
    let a = s.add_org("A", None, false).unwrap().id;
    let b = s.add_org("B", None, false).unwrap().id;
    let t = jira(&s);
    let of_b = s
        .create_bucket(&NewBucket {
            kind: "release",
            name: "1.0",
            org_id: Some(b),
            ..Default::default()
        })
        .unwrap();
    s.add_bucket_ref(of_b.id, t, "1.0", None).unwrap();
    s.add_bucket_ref(of_b.id, t, "1.0-rc", None).unwrap();
    let item = ticket(&s, t, "1", None, &["1.0"]);
    assert_eq!(members(&s, of_b.id), [(item, "adopted".into())]);
    // The tracker moves to org A: both refs go dormant, and unlinking one
    // must still withdraw what it adopted.
    s.set_tracker_org(t, Some(a)).unwrap();
    s.remove_bucket_ref(of_b.id, t, "1.0").unwrap();
    assert!(members(&s, of_b.id).is_empty());
}

#[test]
fn an_unassigned_name_clash_on_org_delete_stays_within_the_name_cap() {
    let s = store();
    let a = s.add_org("A", None, false).unwrap().id;
    let long = "s".repeat(BUCKET_NAME_MAX_CHARS);
    sprint(&s, &long);
    let of_a = s
        .create_bucket(&NewBucket {
            kind: "sprint",
            name: &long,
            org_id: Some(a),
            ..Default::default()
        })
        .unwrap();
    assert!(s.remove_org(a).unwrap());
    let moved = s.get_bucket(of_a.id).unwrap().unwrap();
    assert_eq!(moved.name.chars().count(), BUCKET_NAME_MAX_CHARS);
    assert!(moved.name.ends_with(&format!(" (#{})", of_a.id)));
}

#[test]
fn an_org_delete_finds_a_free_name_when_the_suffix_is_taken_too() {
    let s = store();
    let a = s.add_org("A", None, false).unwrap().id;
    let of_a = s
        .create_bucket(&NewBucket {
            kind: "sprint",
            name: "S",
            org_id: Some(a),
            ..Default::default()
        })
        .unwrap();
    sprint(&s, "S");
    sprint(&s, &format!("S (#{})", of_a.id));
    assert!(s.remove_org(a).unwrap());
    let moved = s.get_bucket(of_a.id).unwrap().unwrap();
    assert_eq!(moved.name, format!("S (#{}-2)", of_a.id));
}

fn personal(s: &Store, name: &str, owner: i64) -> BucketRow {
    s.create_bucket(&NewBucket {
        kind: "sprint",
        name,
        owner_person_id: Some(owner),
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn a_personal_sprint_is_one_per_owner_beside_the_teams() {
    let s = store();
    let team = sprint(&s, "Sprint 1");
    // The name is unique per owner: Ana and Bo each keep a "Sprint 1".
    let ana = personal(&s, "Sprint 1", 1);
    let bo = personal(&s, "Sprint 1", 2);
    assert_eq!(ana.owner_person_id, Some(1));
    let dup = s
        .create_bucket(&NewBucket {
            kind: "sprint",
            name: "Sprint 1",
            owner_person_id: Some(1),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(dup.code, codes::E_EXISTS);
    // One current sprint per owner: the team's and Ana's hold the same item.
    let item = local(&s, "task");
    s.add_bucket_item(team.id, item).unwrap();
    s.add_bucket_item(ana.id, item).unwrap();
    let other_ana = personal(&s, "Sprint 2", 1);
    let e = s.add_bucket_item(other_ana.id, item).unwrap_err();
    assert_eq!(e.code, codes::E_CONFLICT);
    assert_eq!(e.details.unwrap()["sprint_id"], ana.id);
    // Carry-over stays within one plan.
    let e = s.close_sprint(ana.id, None, Some(bo.id), None).unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    let e = s
        .close_sprint(ana.id, None, Some(team.id), None)
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    s.close_sprint(ana.id, None, Some(other_ana.id), None)
        .unwrap();
    assert_eq!(members(&s, other_ana.id).len(), 1);
    // A personal bucket links to no tracker.
    let t = jira(&s);
    let e = s.add_bucket_ref(bo.id, t, "S1", None).unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
}
