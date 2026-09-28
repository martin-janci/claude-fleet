//! The describe cache: TTL expiry, `ttl_secs == 0`, and the `ON DELETE
//! CASCADE` that takes it with its item.

use super::*;
use crate::store::TrackerItemWrite;

fn store() -> Store {
    Store::open_in_memory().unwrap()
}

/// One tracker item to hang a description off.
fn seed_item(s: &Store) -> i64 {
    let t = s
        .add_tracker("jira", "J", "https://acme.atlassian.net")
        .unwrap()
        .id;
    s.upsert_tracker_item(
        t,
        &TrackerItemWrite {
            external_id: "1".into(),
            key: Some("ABC-1".into()),
            title: "Refund".into(),
            status_name: "In Progress".into(),
            status_category: "in_progress".into(),
            ..Default::default()
        },
    )
    .unwrap()
    .id
}

#[test]
fn a_cached_description_expires_with_its_ttl() {
    let s = store();
    let id = seed_item(&s);
    s.put_description(id, "the whole thing", 15).unwrap();
    assert_eq!(
        s.cached_description(id, 300, now_unix())
            .unwrap()
            .as_deref(),
        Some("the whole thing")
    );
    assert_eq!(
        s.cached_description(id, 300, now_unix() + 301).unwrap(),
        None
    );
    // ttl 0 turns the cache off, without deleting what is there.
    assert_eq!(s.cached_description(id, 0, now_unix()).unwrap(), None);
}

#[test]
fn deleting_an_item_takes_its_description() {
    let s = store();
    let id = seed_item(&s);
    s.put_description(id, "gone soon", 9).unwrap();
    s.conn_for_test()
        .execute("DELETE FROM work_items WHERE id = ?1", [id])
        .unwrap();
    assert_eq!(s.cached_description(id, 300, now_unix()).unwrap(), None);
}

/// The describe cache sweeps as `RetentionTable::Descriptions`, through the
/// same `retention_rows` / `retention_eligible` / `retention_delete_batch`
/// the other swept tables use (there is no second implementation to drift):
/// only rows past the window go, and a live-linked ticket earns its cache no
/// keep — this table has no liveness rule.
#[test]
fn the_cache_sweeps_only_what_is_older_than_its_window() {
    use crate::store::RetentionTable::Descriptions;
    let s = store();
    let old_id = seed_item(&s);
    let t = s
        .add_tracker("jira", "J2", "https://other.atlassian.net")
        .unwrap()
        .id;
    let fresh_id = s
        .upsert_tracker_item(
            t,
            &TrackerItemWrite {
                external_id: "2".into(),
                key: Some("DEF-1".into()),
                title: "Other".into(),
                status_name: "To Do".into(),
                status_category: "todo".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
    s.put_description(old_id, "old", 3).unwrap();
    s.put_description(fresh_id, "fresh", 5).unwrap();
    let now = now_unix();
    let days = 30;
    s.conn_for_test()
        .execute(
            "UPDATE work_item_descriptions SET fetched_at = ?1 WHERE item_id = ?2",
            rusqlite::params![now - (days + 1) * 86_400, old_id],
        )
        .unwrap();
    assert_eq!(s.retention_rows(Descriptions).unwrap(), 2);
    assert_eq!(s.retention_eligible(Descriptions, now, days).unwrap(), 1);
    let n = s
        .retention_delete_batch(Descriptions, now, days, 200)
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(s.cached_description(old_id, 10_000, now).unwrap(), None);
    assert_eq!(
        s.cached_description(fresh_id, 10_000, now)
            .unwrap()
            .as_deref(),
        Some("fresh")
    );
    assert_eq!(s.retention_rows(Descriptions).unwrap(), 1);
}

/// The batch discipline `service::work::retention::sweep_capped` relies on:
/// `limit` bounds one call, `0` deletes nothing, and repeated calls drain the
/// backlog oldest-`item_id`-first (the describe cache's primary key, which is
/// what `RetentionTable::key_col` names for it — a delete by `id` would not
/// even parse against this table).
#[test]
fn the_cache_sweeps_in_batches_oldest_item_id_first() {
    use crate::store::RetentionTable::Descriptions;
    let s = store();
    let t = s
        .add_tracker("jira", "J", "https://acme.atlassian.net")
        .unwrap()
        .id;
    let now = now_unix();
    let days = 30;
    let mut ids = Vec::new();
    for i in 0..5 {
        let id = s
            .upsert_tracker_item(
                t,
                &TrackerItemWrite {
                    external_id: i.to_string(),
                    key: Some(format!("ABC-{i}")),
                    title: "T".into(),
                    status_name: "Done".into(),
                    status_category: "done".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .id;
        s.put_description(id, "old", 3).unwrap();
        ids.push(id);
    }
    s.conn_for_test()
        .execute(
            "UPDATE work_item_descriptions SET fetched_at = ?1",
            rusqlite::params![now - (days + 1) * 86_400],
        )
        .unwrap();
    assert_eq!(
        s.retention_delete_batch(Descriptions, now, days, 0)
            .unwrap(),
        0,
        "0 deletes nothing"
    );
    assert_eq!(s.retention_eligible(Descriptions, now, days).unwrap(), 5);
    assert_eq!(
        s.retention_delete_batch(Descriptions, now, days, 2)
            .unwrap(),
        2,
        "one batch"
    );
    assert_eq!(s.retention_rows(Descriptions).unwrap(), 3);
    // The two oldest ids (lowest item_id) went first. A TTL wide enough to
    // cover the backdated `fetched_at`: what is asserted here is which rows
    // the sweep deleted, not the TTL.
    let ttl = 100 * 86_400;
    for id in &ids[..2] {
        assert_eq!(s.cached_description(*id, ttl, now).unwrap(), None);
    }
    for id in &ids[2..] {
        assert!(s.cached_description(*id, ttl, now).unwrap().is_some());
    }
    assert_eq!(
        s.retention_delete_batch(Descriptions, now, days, 200)
            .unwrap(),
        3,
        "the rest"
    );
    assert_eq!(s.retention_rows(Descriptions).unwrap(), 0);
}
