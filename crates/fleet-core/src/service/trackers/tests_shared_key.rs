//! A key two orgs share: org A's Jira site and org B's both cache `ABC-1`,
//! A's row the older. Every single-item reader answers a caller of org B
//! with org B's item (not a refusal over org A's, nor a missing item), and
//! a caller of org A still gets org A's. Also: `tickets::item_visible` is
//! `allowed(..).contains(..)` for one item.

use super::tickets::{self, allowed, item_visible};
use crate::net::https::FakeTransport;
use crate::service::orgs::OrgScope;
use crate::service::work::{card, handover, resume};
use crate::store::{Store, TrackerConfig, TrackerItemWrite, WorkTarget};
use std::sync::{Arc, Mutex};

struct Shared {
    store: Arc<Mutex<Store>>,
    org_a: i64,
    org_b: i64,
    item_a: i64,
    item_b: i64,
}

/// One Jira tracker per org, both holding `ABC-1`; org A's first (so its
/// row is the store's "the item for this key"). `hosta` (org A) works on
/// A's item, `hostb` (org B) on B's, each through a live manual link.
fn shared_key() -> Shared {
    let s = Store::open_in_memory().unwrap();
    let org_a = s.add_org("Company A", None, false).unwrap().id;
    let org_b = s.add_org("Company B", None, false).unwrap().id;
    let mut items = Vec::new();
    for (name, base, org, title) in [
        ("Acme", "https://acme.atlassian.net", org_a, "Refund (A)"),
        ("Beta", "https://beta.atlassian.net", org_b, "Refund (B)"),
    ] {
        let t = s.add_tracker("jira", name, base).unwrap().id;
        s.set_tracker_probe(
            t,
            None,
            &TrackerConfig {
                key_prefixes: vec!["ABC".into()],
                ..Default::default()
            },
        )
        .unwrap();
        s.set_tracker_state(t, "ok", None).unwrap();
        s.set_tracker_org(t, Some(org)).unwrap();
        let id = s
            .upsert_tracker_item(
                t,
                &TrackerItemWrite {
                    external_id: "1".into(),
                    key: Some("ABC-1".into()),
                    title: title.into(),
                    status_name: "In Progress".into(),
                    status_category: "in_progress".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .id;
        items.push(id);
    }
    let (item_a, item_b) = (items[0], items[1]);
    assert_eq!(
        s.work_item_by_key("ABC-1").unwrap().unwrap().id,
        item_a,
        "org A's row is the store's first"
    );
    for (host, org, item) in [("hosta", org_a, item_a), ("hostb", org_b, item_b)] {
        s.upsert_host(host).unwrap();
        s.set_host_org(host, Some(org)).unwrap();
        let sid = s
            .upsert_session(
                &format!("dev-{host}"),
                host,
                None,
                None,
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        s.link_session_work(sid, WorkTarget::Item(item), "manual")
            .unwrap();
    }
    Shared {
        store: Arc::new(Mutex::new(s)),
        org_a,
        org_b,
        item_a,
        item_b,
    }
}

fn net() -> super::TrackerNet {
    super::TrackerNet::fake(Arc::new(FakeTransport::new()))
}

#[tokio::test]
async fn every_single_item_reader_answers_with_the_callers_own_item() {
    let f = shared_key();
    let (host_a, host_b, client_a, client_b) = {
        let s = f.store.lock().unwrap();
        (
            OrgScope::for_host(&s, "hosta").unwrap(),
            OrgScope::for_host(&s, "hostb").unwrap(),
            OrgScope::for_client(&s, f.org_a).unwrap(),
            OrgScope::for_client(&s, f.org_b).unwrap(),
        )
    };
    let cases = [
        ("org-B host", &host_b, f.item_b, "Refund (B)"),
        ("org-B bound client", &client_b, f.item_b, "Refund (B)"),
        ("org-A host", &host_a, f.item_a, "Refund (A)"),
        ("org-A bound client", &client_a, f.item_a, "Refund (A)"),
    ];
    for (who, scope, want_id, want_title) in cases {
        let c = card::card(
            &f.store,
            "ABC-1",
            &crate::service::view_scope::org_only_view(scope),
        )
        .unwrap_or_else(|e| panic!("{who}: card refused: {e:?}"));
        assert_eq!(c.title, want_title, "{who}: card");

        let t = tickets::lookup(
            &f.store,
            "ABC-1",
            &crate::service::view_scope::org_only_view(scope),
            &net(),
        )
        .await
        .unwrap_or_else(|e| panic!("{who}: lookup refused: {e:?}"));
        assert_eq!(t.item.id, want_id, "{who}: lookup");

        let s = f.store.lock().unwrap();
        let g = handover::gather_stored(
            &s,
            "ABC-1",
            None,
            &crate::service::view_scope::ViewScope::internal().with_org(scope.clone()),
        )
        .unwrap();
        assert_eq!(g.input.title.as_deref(), Some(want_title), "{who}: brief");

        let plan = resume::plan_resume(
            &s,
            "ABC-1",
            None,
            None,
            scope,
            &crate::service::view_scope::ViewScope::internal(),
        )
        .unwrap();
        assert_eq!(
            plan.title.as_deref(),
            Some(want_title),
            "{who}: resume plan"
        );
    }
}

/// Unscoped, the key stays ambiguous for a bare lookup (never guess), and
/// the card is the store's first row, as before.
#[tokio::test]
async fn an_unscoped_caller_keeps_the_stores_answer() {
    let f = shared_key();
    let c = card::card(
        &f.store,
        "ABC-1",
        &crate::service::view_scope::org_only_view(&OrgScope::All),
    )
    .unwrap();
    assert_eq!(c.title, "Refund (A)");
    assert!(
        f.store
            .lock()
            .unwrap()
            .tracker_item_for_key("ABC-1")
            .unwrap()
            .is_none(),
        "two trackers: no single answer"
    );
    assert_eq!(
        f.store
            .lock()
            .unwrap()
            .tracker_items_for_key("ABC-1")
            .unwrap()
            .iter()
            .map(|i| i.id)
            .collect::<Vec<_>>(),
        vec![f.item_a, f.item_b]
    );
}

/// A URL names one site's ticket: a scoped caller is never answered with
/// another site's item that happens to share the key. A Jira URL names its
/// tracker by site; a Linear URL for `ABC-1` leaves the tracker open (both
/// Jira trackers claim `ABC`), so it stays ambiguous and is refused, as
/// before, rather than handed the caller's own org's `ABC-1`.
#[tokio::test]
async fn a_url_is_never_answered_with_another_sites_item() {
    let f = shared_key();
    let (host_b, client_b) = {
        let s = f.store.lock().unwrap();
        (
            OrgScope::for_host(&s, "hostb").unwrap(),
            OrgScope::for_client(&s, f.org_b).unwrap(),
        )
    };
    for (who, scope) in [("org-B host", &host_b), ("org-B bound client", &client_b)] {
        for url in [
            "https://acme.atlassian.net/browse/ABC-1",
            "https://linear.app/acme/issue/ABC-1",
        ] {
            let want = crate::service::orgs::not_visible_to(scope, "ABC-1");
            match tickets::lookup(
                &f.store,
                url,
                &crate::service::view_scope::org_only_view(scope),
                &net(),
            )
            .await
            {
                Ok(t) => panic!("{who}: {url} answered with item {}", t.item.id),
                Err(e) => assert_eq!(
                    (e.code.as_str(), e.message.as_str()),
                    (want.code.as_str(), want.message.as_str()),
                    "{who}: {url}"
                ),
            }
        }
        // Org B's own site still answers with org B's item.
        let t = tickets::lookup(
            &f.store,
            "https://beta.atlassian.net/browse/ABC-1",
            &crate::service::view_scope::org_only_view(scope),
            &net(),
        )
        .await
        .unwrap_or_else(|e| panic!("{who}: beta URL refused: {e:?}"));
        assert_eq!(t.item.id, f.item_b, "{who}: beta URL");
    }
}

/// A host with no work on the key is refused, not handed either org's item.
#[tokio::test]
async fn a_host_with_no_work_on_the_key_is_still_refused() {
    let f = shared_key();
    let host_c = {
        let s = f.store.lock().unwrap();
        s.upsert_host("hostc").unwrap();
        s.set_host_org("hostc", Some(f.org_b)).unwrap();
        OrgScope::for_host(&s, "hostc").unwrap()
    };
    assert!(card::card(
        &f.store,
        "ABC-1",
        &crate::service::view_scope::org_only_view(&host_c)
    )
    .is_err());
    assert!(tickets::lookup(
        &f.store,
        "ABC-1",
        &crate::service::view_scope::org_only_view(&host_c),
        &net()
    )
    .await
    .is_err());
}

#[test]
fn item_visible_agrees_with_allowed() {
    let f = shared_key();
    let s = f.store.lock().unwrap();
    // Local items too (one unassigned, one of org B), and a host of no org
    // working on the unassigned one.
    let local = s.create_local_work_item(Some("XYZ-5"), "Local").unwrap().id;
    let local_b = s
        .create_local_work_item(Some("XYZ-6"), "Local B")
        .unwrap()
        .id;
    s.set_local_item_org(local_b, Some(f.org_b)).unwrap();
    s.upsert_host("hostc").unwrap();
    let sid = s
        .upsert_session("dev-hostc", "hostc", None, None, 1, 1, "running", None)
        .unwrap();
    s.link_session_work(sid, WorkTarget::Item(local), "manual")
        .unwrap();
    let scopes = [
        OrgScope::All,
        OrgScope::for_host(&s, "hosta").unwrap(),
        OrgScope::for_host(&s, "hostb").unwrap(),
        OrgScope::for_host(&s, "hostc").unwrap(),
        OrgScope::for_client(&s, f.org_a).unwrap(),
        OrgScope::for_client(&s, f.org_b).unwrap(),
    ];
    let ids: Vec<i64> = s
        .work_item_orgs()
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids.len(), 4, "two tickets and two local items: {ids:?}");
    for scope in &scopes {
        let set = allowed(scope, &s).unwrap();
        for id in &ids {
            let item = s.get_work_item(*id).unwrap().unwrap();
            let want = set.as_ref().is_none_or(|a| a.contains(id));
            assert_eq!(
                item_visible(scope, &s, &item).unwrap(),
                want,
                "{scope:?}, item {id}"
            );
        }
    }
}
