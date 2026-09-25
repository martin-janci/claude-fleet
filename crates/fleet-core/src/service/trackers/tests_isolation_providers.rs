//! M5's isolation rules, run once per M6 provider (acceptance 6: "a Company
//! A host sees none of Company B's Asana tasks" — and GitHub issues, Linear
//! issues, Data Center tickets). The rules are provider-agnostic (an item's
//! org is its tracker's; a per-host token reads only what its own host's
//! sessions work on, inside its org; a tracker never binds another org's
//! bare reference), so this pass proves each provider's keys, URLs and
//! references take no side door: `owner/repo#n`, `asana:<gid>`, a team key,
//! a Data Center key, and their URLs.
//!
//! The full matrix per caller kind lives in `mcp/tools/tests_isolation.rs`
//! (M5); this is its per-provider slice at the service layer.

use super::conformance::fixture;
use super::tickets::{lookup, tickets, trackers};
use super::{TrackerNet, WorkItemSnapshot};
use crate::ipc_error::codes;
use crate::net::https::FakeTransport;
use crate::service::orgs::OrgScope;
use crate::store::{Store, TrackerConfig, TrackerSettings, WorkTarget};
use std::sync::{Arc, Mutex};

/// One provider's case: its tracker row and at least three normalised items
/// from its own fixtures.
struct Case {
    provider: &'static str,
    site: &'static str,
    transport: &'static str,
    items: Vec<WorkItemSnapshot>,
}

fn cases() -> Vec<Case> {
    let fake: Arc<dyn crate::net::https::HttpTransport> = Arc::new(FakeTransport::new());
    let gh = super::github::GitHub::new(
        "https://github.com/acme",
        TrackerConfig::default(),
        TrackerSettings::default(),
        fake.clone(),
    );
    let asana = super::asana::Asana::new(
        "https://app.asana.com",
        TrackerConfig::default(),
        TrackerSettings::default(),
        None,
        fake.clone(),
    );
    let linear = super::linear::Linear::new(
        "https://linear.app/acme",
        TrackerConfig::default(),
        None,
        fake.clone(),
    );
    let dc = super::jira_dc::JiraDc::new(
        "https://jira.corp.example/jira",
        TrackerConfig::default(),
        None,
        fake,
    );
    let list = |v: serde_json::Value| v.as_array().cloned().unwrap_or_default();
    vec![
        Case {
            provider: "github",
            site: "https://github.com/acme",
            transport: "via_cli:h-b",
            items: list(
                fixture("github", "search_mine_p1.json")["data"]["search"]["nodes"].clone(),
            )
            .iter()
            .filter_map(|n| gh.snapshot(n))
            .collect(),
        },
        Case {
            provider: "asana",
            site: "https://app.asana.com",
            transport: "direct",
            items: list(fixture("asana", "tasks_mine_p1.json")["data"].clone())
                .iter()
                .filter_map(|t| asana.snapshot(t))
                .collect(),
        },
        Case {
            provider: "linear",
            site: "https://linear.app/acme",
            transport: "direct",
            items: list(
                fixture("linear", "issues_mine_p1.json")["data"]["issues"]["nodes"].clone(),
            )
            .iter()
            .filter_map(|n| linear.snapshot(n))
            .collect(),
        },
        Case {
            provider: "jira_dc",
            site: "https://jira.corp.example/jira",
            transport: "direct",
            items: list(fixture("jira_dc", "search_mine_p1.json")["issues"].clone())
                .iter()
                .filter_map(|i| dc.snapshot(i))
                .collect(),
        },
    ]
}

#[tokio::test]
async fn a_company_a_host_sees_none_of_company_bs_work_in_any_provider() {
    for c in cases() {
        assert!(
            c.items.len() >= 3,
            "{}: the fixture has three items",
            c.provider
        );
        let s = Store::open_in_memory().unwrap();
        for h in ["h-a", "h-b"] {
            s.upsert_host(h).unwrap();
        }
        let a = s.add_org("Company A", None, false).unwrap();
        let b = s.add_org("Company B", None, false).unwrap();
        s.set_host_org("h-a", Some(a.id)).unwrap();
        s.set_host_org("h-b", Some(b.id)).unwrap();
        // Company A's own Jira, with one ticket its host works on.
        let ta = s
            .add_tracker("jira", "A Jira", "https://alpha.atlassian.net")
            .unwrap();
        s.set_tracker_org(ta.id, Some(a.id)).unwrap();
        let aa = s
            .upsert_tracker_item(
                ta.id,
                &crate::store::TrackerItemWrite {
                    external_id: "1".into(),
                    key: Some("AA-1".into()),
                    title: "Alpha login".into(),
                    status_name: "To Do".into(),
                    status_category: "todo".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .id;
        // Company B's tracker of this provider.
        let tb = s.add_tracker(c.provider, "B tracker", c.site).unwrap();
        s.set_tracker_transport(tb.id, c.transport).unwrap();
        s.set_tracker_org(tb.id, Some(b.id)).unwrap();
        s.set_tracker_state(tb.id, "ok", None).unwrap();
        let ids: Vec<i64> = c.items[..2]
            .iter()
            .map(|i| {
                s.upsert_tracker_item(tb.id, &super::sync::to_write(i.clone()))
                    .unwrap()
                    .id
            })
            .collect();
        let sess = |name: &str, host: &str| {
            s.upsert_session(name, host, None, None, 1, 1, "running", None)
                .unwrap()
        };
        let s_a = sess("s-a", "h-a");
        let s_b = sess("s-b", "h-b");
        let s_x = sess("s-x", "h-a");
        let s_ref = sess("s-ref", "h-a");
        s.link_session_work(s_a, WorkTarget::Item(aa), "manual")
            .unwrap();
        s.link_session_work(s_b, WorkTarget::Item(ids[0]), "manual")
            .unwrap();
        // A person forced B's second item onto an A session.
        s.link_session_work(s_x, WorkTarget::Item(ids[1]), "manual")
            .unwrap();
        // An A session mentions B's third item before B's tracker has it.
        let third = c.items[2].clone();
        let third_key = third.key.clone().unwrap();
        s.link_session_work(s_ref, WorkTarget::Key(&third_key), "manual")
            .unwrap();
        let st = Mutex::new(s);
        let host_a = OrgScope::for_host(&st.lock().unwrap(), "h-a").unwrap();
        let host_b = OrgScope::for_host(&st.lock().unwrap(), "h-b").unwrap();

        // Host A: only its own org's ticket, whatever it asks.
        let seen: Vec<i64> = tickets(&st, None, None, None, None, &host_a)
            .unwrap()
            .into_iter()
            .map(|t| t.item.id)
            .collect();
        assert_eq!(seen, vec![aa], "{}", c.provider);
        assert!(
            tickets(&st, Some(tb.id), None, None, None, &host_a)
                .unwrap()
                .is_empty(),
            "{}",
            c.provider
        );
        let ts: Vec<i64> = trackers(&st, &host_a)
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ts, vec![ta.id], "{}", c.provider);
        let fake = FakeTransport::new();
        let net = TrackerNet::fake(Arc::new(fake.clone()));
        for item in &c.items[..3] {
            let mut refs = vec![item.key.clone().unwrap()];
            refs.extend(item.url.clone());
            for r in refs {
                let e = lookup(&st, &r, &host_a, &net).await.unwrap_err();
                assert_eq!(
                    e.code,
                    codes::E_FORBIDDEN,
                    "{}: {r}: {}",
                    c.provider,
                    e.message
                );
            }
        }
        assert!(
            fake.requests().is_empty(),
            "{}: a host token never makes B's tracker fetch",
            c.provider
        );

        // Host B: the item its own session works on, and not the one
        // forced onto an A session.
        let seen_b: Vec<i64> = tickets(&st, None, None, None, None, &host_b)
            .unwrap()
            .into_iter()
            .map(|t| t.item.id)
            .collect();
        assert_eq!(seen_b, vec![ids[0]], "{}", c.provider);

        // Master sees all of them.
        assert_eq!(
            tickets(&st, None, None, None, None, &OrgScope::All)
                .unwrap()
                .len(),
            3,
            "{}",
            c.provider
        );

        // B's tracker never answers, nor binds, a reference an A session made.
        {
            let s = st.lock().unwrap();
            assert!(
                !s.unbound_ref_keys(tb.id, 50).unwrap().contains(&third_key),
                "{}: B's credentials never fetch what A's session mentioned",
                c.provider
            );
            s.upsert_tracker_item(tb.id, &super::sync::to_write(third))
                .unwrap();
            s.bind_tracker_refs(tb.id).unwrap();
            let links = s.session_work_links(s_ref).unwrap();
            assert_eq!(
                links[0].item_id, None,
                "{}: stays a bare reference",
                c.provider
            );
        }
    }
}
