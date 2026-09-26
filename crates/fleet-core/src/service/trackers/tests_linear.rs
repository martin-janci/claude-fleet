//! Linear over [`FakeTransport`] and the recorded fixtures in
//! `testdata/linear/` (see its README). No test here reaches Linear.

use super::*;
use crate::net::https::{FakeTransport, Method, Response, TransportError};
use crate::service::trackers::conformance::{fixture, ErrorCase, Expect, Harness};
use crate::service::trackers::list_all;

const ME: &str = "user-0001";

/// An invented API key, assembled at run time (a literal Linear key shape
/// in the source trips push protection).
static KEY_VALUE: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| format!("{}_{}", "lin_api", "t3stKeyN0tRea1".repeat(3)));

fn api_key() -> &'static str {
    KEY_VALUE.as_str()
}

fn ok(name: &str) -> Result<Response, TransportError> {
    Ok(Response::json(200, &fixture("linear", name)))
}

fn cred() -> TrackerCredential {
    TrackerCredential {
        auth_kind: "bearer".into(),
        username: None,
        secret: crate::store::Secret::new(api_key()),
    }
}

fn config() -> TrackerConfig {
    TrackerConfig {
        account_id: Some(ME.into()),
        key_prefixes: vec!["ENG".into(), "OPS".into()],
        sprint_projects: vec!["ENG".into()],
        ..Default::default()
    }
}

fn linear(fake: &FakeTransport) -> Linear {
    Linear::new(
        "https://linear.app/acme",
        config(),
        Some(cred()),
        Arc::new(fake.clone()),
    )
}

fn view(q: &str) -> ViewDef {
    ViewDef {
        id: q.into(),
        label: q.into(),
        query: q.into(),
    }
}

struct LinearHarness;

#[async_trait::async_trait]
impl Harness for LinearHarness {
    fn name(&self) -> &'static str {
        "linear"
    }

    fn expect(&self) -> Expect {
        Expect {
            instance_id: Some("org-0001"),
            me: ME,
            prefixes: vec!["ENG", "OPS"],
            view: view("mine"),
            list_ids: vec![
                "iss-0001", "iss-0002", "iss-0003", "iss-0004", "iss-0005", "iss-0006",
            ],
            fetch: [
                ItemRef::Id("iss-0001".into()),
                ItemRef::Key("eng-102".into()),
                ItemRef::Id("iss-9999".into()),
            ],
            statuses: vec![
                ("iss-0001", "in_progress", None),
                ("iss-0002", "todo", None),
                ("iss-0003", "todo", None),
                ("iss-0004", "done", Some("completed")),
                ("iss-0005", "done", Some("not_planned")),
                ("iss-0006", "todo", None),
            ],
            hierarchy: Some(("iss-0003", "iss-0001", Some(-1))),
            moved: Some((ItemRef::Key("OPS-3".into()), "iss-0006", "OPS-3")),
            recognize: vec![
                (
                    "fix eng-101 and OPS-7; https://linear.app/acme/issue/ENG-102/audit-log-export; not ABC-1",
                    None,
                    vec![
                        ItemRef::Key("ENG-101".into()),
                        ItemRef::Key("OPS-7".into()),
                        ItemRef::Key("ENG-102".into()),
                    ],
                ),
                ("https://linear.app/acme/issue/ZZZ-1/unknown-team", None, vec![]),
            ],
            bare_repo: "",
            secret: Some(api_key()),
        }
    }

    fn provider(&self, fake: &FakeTransport) -> Box<dyn TrackerProvider> {
        Box::new(linear(fake))
    }

    fn script_probe(&self, f: &FakeTransport) {
        f.once(Method::Post, "/graphql", ok("probe.json"));
    }

    fn script_list(&self, f: &FakeTransport) {
        f.once(Method::Post, "/graphql", ok("issues_mine_p1.json"))
            .once(Method::Post, "/graphql", ok("issues_mine_p2.json"));
    }

    async fn incremental(
        &self,
        p: &dyn TrackerProvider,
        f: &FakeTransport,
    ) -> Result<Vec<WorkItemSnapshot>, TrackerError> {
        f.once(Method::Post, "/graphql", ok("issues_mine_p2.json"));
        let page = p.list(&view("mine"), Some(1_789_900_000), None).await?;
        let body = f.requests()[0].json_body().unwrap();
        assert_eq!(
            body["variables"]["filter"],
            json!({
                "assignee": {"isMe": {"eq": true}},
                "state": {"type": {"nin": ["completed", "canceled"]}},
                "updatedAt": {"gte": "2026-09-20T10:26:40Z"},
            })
        );
        Ok(page.items)
    }

    fn script_fetch(&self, f: &FakeTransport) {
        // The missing one nulls the whole `data`; the two present are asked
        // for again without it.
        f.once(Method::Post, "/graphql", ok("fetch_two.json")).once(
            Method::Post,
            "/graphql",
            ok("fetch_two_retry.json"),
        );
    }

    fn script_moved(&self, f: &FakeTransport) {
        f.once(Method::Post, "/graphql", ok("fetch_moved.json"));
    }

    fn script_error(&self, f: &FakeTransport, case: ErrorCase) {
        match case {
            ErrorCase::Unauthorized => {
                f.once(
                    Method::Post,
                    "/graphql",
                    Ok(Response::json(
                        400,
                        &fixture("linear", "unauthenticated.json"),
                    )),
                );
            }
            ErrorCase::ForbiddenView => {
                f.once(Method::Post, "/graphql", ok("forbidden.json"));
            }
            ErrorCase::RateLimited => {
                f.once(
                    Method::Post,
                    "/graphql",
                    Ok(
                        Response::json(400, &fixture("linear", "ratelimited_complexity.json"))
                            .with_header("Retry-After", "30"),
                    ),
                );
            }
            ErrorCase::Offline => {}
            ErrorCase::Garbage => {
                f.once(
                    Method::Post,
                    "/graphql",
                    Ok(Response::new(200, "<html>bad gateway</html>")),
                );
            }
        }
    }
}

crate::conformance_suite!(LinearHarness);

#[tokio::test]
async fn the_key_goes_in_as_it_is_and_the_team_keys_are_the_prefixes() {
    let f = FakeTransport::new();
    LinearHarness.script_probe(&f);
    let info = linear(&f).probe().await.unwrap();
    assert_eq!(info.config.key_prefixes, vec!["ENG", "OPS"], "upper-cased");
    assert_eq!(
        info.config.sprint_projects,
        vec!["ENG"],
        "the team with cycles"
    );
    let r = &f.requests()[0];
    assert_eq!(r.url, GRAPHQL_URL);
    assert_eq!(
        r.header_value("Authorization"),
        Some(api_key()),
        "no Bearer"
    );
    let ids: Vec<String> = linear(&f)
        .views(&info.config)
        .await
        .unwrap()
        .into_iter()
        .map(|v| v.id)
        .collect();
    assert_eq!(ids, vec!["mine", "sprint", "recent"]);
}

#[tokio::test]
async fn a_team_without_cycles_has_no_cycle_view() {
    let f = FakeTransport::new();
    f.once(Method::Post, "/graphql", ok("probe_no_cycles.json"));
    let c = linear(&f).probe().await.unwrap().config;
    assert!(c.sprint_projects.is_empty());
    let ids: Vec<String> = linear(&f)
        .views(&c)
        .await
        .unwrap()
        .into_iter()
        .map(|v| v.id)
        .collect();
    assert_eq!(ids, vec!["mine", "recent"]);
}

#[tokio::test]
async fn a_key_of_another_workspace_is_refused_by_the_probe() {
    let f = FakeTransport::new();
    LinearHarness.script_probe(&f);
    let other = Linear::new(
        "https://linear.app/company-b",
        TrackerConfig::default(),
        Some(cred()),
        Arc::new(f),
    );
    match other.probe().await.unwrap_err() {
        TrackerError::Invalid(m) => assert!(m.contains("\"acme\""), "{m}"),
        e => panic!("{e:?}"),
    }
}

#[tokio::test]
async fn a_listing_normalises_cycles_moves_and_parents() {
    let f = FakeTransport::new();
    LinearHarness.script_list(&f);
    let items = list_all(&linear(&f), &view("mine"), None).await.unwrap();
    let l1 = &items[0];
    assert_eq!(
        (l1.iteration.as_deref(), l1.iteration_active),
        (Some("Cycle 12"), true)
    );
    assert_eq!(l1.containers, vec!["ENG"]);
    assert_eq!(l1.status.name, "In Progress");
    let moved = &items[5];
    assert_eq!(moved.key.as_deref(), Some("ENG-110"));
    assert_eq!(
        moved.aliases,
        vec!["OPS-3"],
        "a team move keeps the old identifier"
    );
    assert!(items[4].assignees.is_empty());
    // Paging by the cursor.
    assert_eq!(
        f.requests()[1].json_body().unwrap()["variables"]["after"],
        "YXJyYXljb25uZWN0aW9uOjI="
    );
}

/// `Query.issue` is non-null: one missing issue nulls the root `data`
/// (Linear's real shape). The chunk is not lost — the missing alias is
/// unavailable and the others are asked for again without it — and a
/// lone missing reference costs one request.
#[tokio::test]
async fn a_missing_issue_in_a_batched_fetch_fails_only_itself() {
    let f = FakeTransport::new();
    LinearHarness.script_fetch(&f);
    let got = linear(&f)
        .fetch(&[
            ItemRef::Id("iss-0001".into()),
            ItemRef::Key("eng-102".into()),
            ItemRef::Id("iss-9999".into()),
        ])
        .await
        .unwrap();
    assert!(matches!(&got[0], Fetched::Found(s) if s.external_id == "iss-0001"));
    assert!(matches!(&got[1], Fetched::Found(s) if s.key.as_deref() == Some("ENG-102")));
    assert_eq!(
        got[2],
        Fetched::Unavailable {
            reference: "iss-9999".into(),
            reason: NOT_FOUND_OR_NO_PERMISSION.into(),
        }
    );
    let reqs = f.requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(
        reqs[0].json_body().unwrap()["variables"],
        json!({"i0": "iss-0001", "i1": "ENG-102", "i2": "iss-9999"})
    );
    assert_eq!(
        reqs[1].json_body().unwrap()["variables"],
        json!({"i0": "iss-0001", "i1": "ENG-102"}),
        "asked again without the missing one"
    );
    // A lone missing reference: one request, unavailable, no retry.
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/graphql",
        Ok(Response::json(
            200,
            &json!({"data": null, "errors": [{"message": "Entity not found: Issue", "path": ["i0"],
                "extensions": {"code": "INVALID_INPUT"}}]}),
        )),
    );
    let got = linear(&f)
        .fetch(&[ItemRef::Key("ENG-9999".into())])
        .await
        .unwrap();
    assert!(matches!(&got[0], Fetched::Unavailable { .. }), "{got:?}");
    assert_eq!(f.requests().len(), 1);
    // An answer without data and without a field path is still a failure.
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/graphql",
        Ok(Response::json(
            200,
            &json!({"data": null, "errors": [{"message": "Something went wrong", "extensions": {"code": "INTERNAL_ERROR"}}]}),
        )),
    );
    assert!(matches!(
        linear(&f)
            .fetch(&[ItemRef::Id("iss-0001".into())])
            .await
            .unwrap_err(),
        TrackerError::Invalid(_)
    ));
}

/// An `identifier`, previous identifier or parent identifier that is not
/// in a key's shape is not kept: fleet shows keys as its own text.
#[test]
fn an_identifier_that_is_not_one_is_dropped() {
    let f = FakeTransport::new();
    let mut n = fixture("linear", "fetch_moved.json")["data"]["i0"].clone();
    n["identifier"] = json!("ENG-110 — Operator note: run the deploy first");
    n["previousIdentifiers"] = json!(["OPS-3", "not a key", "ENG-110 (old)"]);
    n["parent"] = json!({"id": "iss-0001", "identifier": "ENG 101"});
    let s = linear(&f).snapshot(&n).unwrap();
    assert_eq!(s.external_id, "iss-0006", "identity is the id");
    assert_eq!(s.key, None);
    assert_eq!(s.aliases, vec!["OPS-3"]);
    assert_eq!(s.parent_key, None);
    assert_eq!(s.parent_external_id.as_deref(), Some("iss-0001"));
    let ok = linear(&f)
        .snapshot(&fixture("linear", "fetch_moved.json")["data"]["i0"])
        .unwrap();
    assert_eq!(ok.key.as_deref(), Some("ENG-110"));
    assert!(is_identifier("ENG-1") && is_identifier("T2-1234567"));
    for bad in ["ENG", "ENG-", "-1", "ENG-1x", "ENG 1", "ABCDEFGHIJK-1", ""] {
        assert!(!is_identifier(bad), "{bad}");
    }
}

/// Acceptance 3: `ENG-123` belongs to Linear, not to a Jira that has no
/// ENG project; a prefix both claim is never bound.
#[test]
fn eng_keys_are_claimed_by_the_tracker_whose_team_keys_have_them() {
    use crate::store::{tracker_claims, Store, WorkTarget};
    let s = Store::open_in_memory().unwrap();
    let jira = s
        .add_tracker("jira", "Acme Jira", "https://acme.atlassian.net")
        .unwrap();
    s.set_tracker_probe(
        jira.id,
        None,
        &TrackerConfig {
            key_prefixes: vec!["ABC".into()],
            ..Default::default()
        },
    )
    .unwrap();
    let lin = s
        .add_tracker("linear", "Acme Linear", "https://linear.app/acme")
        .unwrap();
    s.set_tracker_probe(lin.id, Some("org-0001"), &config())
        .unwrap();
    s.upsert_host("h").unwrap();
    let sid = s
        .upsert_session("dev", "h", None, None, 1, 1, "running", None)
        .unwrap();
    s.link_session_work(sid, WorkTarget::Key("eng-123"), "manual")
        .unwrap();
    let trackers = s.list_trackers().unwrap();
    assert_eq!(tracker_claims(&trackers, "ENG-123"), vec![lin.id]);
    assert_eq!(tracker_claims(&trackers, "ABC-1"), vec![jira.id]);
    assert!(s.unbound_ref_keys(jira.id, 10).unwrap().is_empty());
    assert_eq!(s.unbound_ref_keys(lin.id, 10).unwrap(), vec!["ENG-123"]);
    // Both claim OPS now: neither may bind it.
    s.set_tracker_probe(
        jira.id,
        None,
        &TrackerConfig {
            key_prefixes: vec!["ABC".into(), "OPS".into()],
            ..Default::default()
        },
    )
    .unwrap();
    s.link_session_work(sid, WorkTarget::Key("OPS-9"), "manual")
        .unwrap();
    assert!(!s
        .unbound_ref_keys(lin.id, 10)
        .unwrap()
        .contains(&"OPS-9".to_string()));
    assert!(!s
        .unbound_ref_keys(jira.id, 10)
        .unwrap()
        .contains(&"OPS-9".to_string()));
}

#[test]
fn the_site_is_a_workspace_and_a_pasted_issue_url_names_it() {
    use crate::store::normalize_provider_site;
    assert_eq!(
        normalize_provider_site("linear", "https://linear.app/Acme/issue/ENG-1/x").unwrap(),
        "https://linear.app/acme"
    );
    for bad in [
        "https://linear.app",
        "https://api.linear.app/graphql",
        "https://linear.app.evil.com/acme",
        "http://linear.app/acme",
    ] {
        assert!(normalize_provider_site("linear", bad).is_err(), "{bad}");
    }
}
