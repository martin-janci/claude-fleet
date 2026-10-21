//! Jira Cloud over [`FakeTransport`] and the recorded fixtures in
//! `testdata/jira/` (see its README). No test here reaches a real site.

use super::*;
use crate::net::https::{FakeTransport, Method, Response, TransportError};
use crate::service::trackers::list_all;
use crate::service::trackers::DESCRIPTION_MAX_CHARS;

const SITE: &str = "https://acme.atlassian.net";
const ME: &str = "557058:00000000-aaaa-bbbb-cccc-000000000001";

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/src/service/trackers/testdata/jira/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).expect(&path)).expect(name)
}

fn ok(name: &str) -> Result<Response, TransportError> {
    Ok(Response::json(200, &fixture(name)))
}

fn cred() -> TrackerCredential {
    TrackerCredential {
        auth_kind: "basic".into(),
        username: Some("dev@example.com".into()),
        secret: crate::store::Secret::new("ATATT3xFfGF0-test-token-not-real-0000"),
    }
}

fn config() -> TrackerConfig {
    TrackerConfig {
        account_id: Some(ME.into()),
        key_prefixes: vec!["ABC".into(), "TEAM".into()],
        sprint_projects: vec!["ABC".into()],
        sprint_field: Some("customfield_10020".into()),
        points_field: Some("customfield_10016".into()),
        ..Default::default()
    }
}

fn jira(fake: &FakeTransport) -> JiraCloud {
    JiraCloud::new(SITE, config(), Some(cred()), Arc::new(fake.clone()))
}

fn view(id: &str, q: &str) -> ViewDef {
    ViewDef {
        id: id.into(),
        label: id.into(),
        query: q.into(),
    }
}

#[tokio::test]
async fn probe_learns_identity_site_prefixes_the_sprint_field_and_sprint_projects() {
    let f = FakeTransport::new();
    f.once(Method::Get, "/rest/api/3/myself", ok("myself.json"))
        .once(Method::Get, "/_edge/tenant_info", ok("tenant_info.json"))
        .once(Method::Get, "startAt=0", ok("project_search_p1.json"))
        .once(Method::Get, "startAt=1", ok("project_search_p2.json"))
        .once(Method::Get, "/rest/api/3/field", ok("fields.json"))
        .once(Method::Post, "/search/jql", ok("sprint_projects.json"));
    let j = JiraCloud::new(
        SITE,
        TrackerConfig::default(),
        Some(cred()),
        Arc::new(f.clone()),
    );
    let info = j.probe().await.unwrap();
    assert_eq!(
        info.instance_id.as_deref(),
        Some("11111111-2222-3333-4444-555555555555")
    );
    let c = info.config;
    assert_eq!(c.account_id.as_deref(), Some(ME));
    assert_eq!(c.tz.as_deref(), Some("Europe/Bratislava"));
    assert_eq!(
        c.key_prefixes,
        vec!["ABC", "TEAM"],
        "both pages, upper-cased"
    );
    assert_eq!(
        c.sprint_field.as_deref(),
        Some("customfield_10020"),
        "found by schema.custom, not by the field named Sprint"
    );
    assert_eq!(
        c.points_field.as_deref(),
        Some("customfield_10016"),
        "the number field, not the text one named Story Points"
    );
    assert_eq!(c.sprint_projects, vec!["ABC"], "sprints are per project");
    // Every request carried Basic auth and nothing was sent anywhere else.
    for r in f.requests() {
        assert!(r.url.starts_with(SITE), "{}", r.url);
        assert!(r
            .header_value("Authorization")
            .is_some_and(|a| a.starts_with("Basic ")));
    }
    let sprint_search = f
        .requests()
        .into_iter()
        .find(|r| r.url.ends_with("/search/jql"))
        .unwrap();
    assert_eq!(
        sprint_search.json_body().unwrap()["jql"],
        "sprint in openSprints()"
    );
}

#[tokio::test]
async fn a_site_without_the_tenant_endpoint_still_probes() {
    let f = FakeTransport::new();
    f.once(Method::Get, "/rest/api/3/myself", ok("myself.json"))
        .once(
            Method::Get,
            "/_edge/tenant_info",
            Ok(Response::new(404, "")),
        )
        .once(Method::Get, "startAt=0", ok("project_search_p2.json"))
        .once(
            Method::Get,
            "/rest/api/3/field",
            Ok(Response::json(200, &json!([]))),
        );
    let j = JiraCloud::new(
        SITE,
        TrackerConfig::default(),
        Some(cred()),
        Arc::new(f.clone()),
    );
    let info = j.probe().await.unwrap();
    assert_eq!(info.instance_id, None);
    assert_eq!(info.config.sprint_field, None);
    assert!(info.config.sprint_projects.is_empty());
    assert_eq!(
        f.count("/search/jql"),
        0,
        "no sprint field: no sprint probe"
    );
}

#[tokio::test]
async fn views_are_the_built_ins_plus_favourites_wrapped_by_reference() {
    let f = FakeTransport::new();
    f.once(
        Method::Get,
        "/filter/favourite",
        ok("filter_favourite.json"),
    );
    let v = jira(&f).views(&config()).await.unwrap();
    let ids: Vec<&str> = v.iter().map(|v| v.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["mine", "sprint", "recent", "filter:10200", "filter:10201"]
    );
    assert_eq!(v[0].query, VIEW_MINE);
    assert_eq!(v[3].query, "filter = 10200", "never its ORDER BY JQL");
    assert_eq!(v[3].label, "Team bugs");

    // No project with sprints: no sprint view (team-managed only).
    f.once(
        Method::Get,
        "/filter/favourite",
        Ok(Response::json(200, &json!([]))),
    );
    let mut c = config();
    c.sprint_projects.clear();
    let ids: Vec<String> = jira(&f)
        .views(&c)
        .await
        .unwrap()
        .into_iter()
        .map(|v| v.id)
        .collect();
    assert_eq!(ids, vec!["mine", "recent"]);
}

#[tokio::test]
async fn a_listing_pages_by_token_and_normalises_every_shape() {
    let f = FakeTransport::new();
    f.once(Method::Post, "/search/jql", ok("search_mine_p1.json"))
        .once(Method::Post, "/search/jql", ok("search_mine_p2.json"));
    let j = jira(&f);
    let items = list_all(&j, &view("mine", VIEW_MINE), None).await.unwrap();
    assert_eq!(items.len(), 7);
    let reqs = f.requests();
    let first = reqs[0].json_body().unwrap();
    assert_eq!(first["jql"], format!("({VIEW_MINE}) ORDER BY updated DESC"));
    assert!(first.get("nextPageToken").is_none());
    let fields: Vec<&str> = first["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(fields.contains(&"summary") && fields.contains(&"customfield_10020"));
    assert_eq!(
        reqs[1].json_body().unwrap()["nextPageToken"],
        "Cg1BQkMtMTAyOjE3MjY"
    );

    let by = |k: &str| items.iter().find(|i| i.key.as_deref() == Some(k)).unwrap();
    // Epic: hierarchy from hierarchyLevel, not the type name.
    let epic = by("ABC-100");
    assert_eq!(
        (epic.hierarchy_level, epic.kind.as_deref()),
        (Some(1), Some("Epic"))
    );
    // Story: parent, sprint, ADF, url, assignee.
    let story = by("ABC-101");
    assert_eq!(story.external_id, "10101");
    assert_eq!(
        story.url.as_deref(),
        Some("https://acme.atlassian.net/browse/ABC-101")
    );
    assert_eq!(story.status.name, "In Review");
    assert_eq!(story.status.category, "in_progress");
    assert_eq!(story.status.resolution, None);
    assert_eq!(
        (
            story.parent_external_id.as_deref(),
            story.parent_key.as_deref()
        ),
        (Some("10100"), Some("ABC-100"))
    );
    assert_eq!(story.iteration.as_deref(), Some("ABC Sprint 7"));
    assert!(story.iteration_active);
    assert_eq!(story.assignees, vec!["Dana Dev"]);
    assert_eq!(story.assignee_id.as_deref(), Some(ME));
    assert_eq!(story.containers, vec!["ABC"]);
    assert_eq!(story.updated, Some(1_789_892_130));
    assert_eq!(
        story.description.as_deref(),
        Some(
            "Context\nInvoices still live in legacy_ledger. Ask @Pat Ops\n\
             See https://acme.atlassian.net/browse/ABC-90\n- copy rows\n- switch reads"
        )
    );
    // Resolutions: Done, Won't Do, Duplicate.
    assert_eq!(
        (
            by("ABC-102").status.category.as_str(),
            by("ABC-102").status.resolution.as_deref()
        ),
        ("done", Some("completed"))
    );
    assert_eq!(
        by("ABC-103").status.resolution.as_deref(),
        Some("not_planned")
    );
    assert!(by("ABC-103").assignees.is_empty());
    assert_eq!(
        by("ABC-104").status.resolution.as_deref(),
        Some("duplicate")
    );
    // Team-managed: a project-scoped status name, no sprint, a plain body.
    let team = by("TEAM-7");
    assert_eq!(
        (team.status.name.as_str(), team.status.category.as_str()),
        ("Doing", "in_progress")
    );
    assert_eq!(team.iteration, None);
    assert_eq!(
        team.description.as_deref(),
        Some("plain text from a v2-shaped body")
    );
    // `undefined` status category → todo; a subtask's level is -1.
    let sub = by("TEAM-8");
    assert_eq!(sub.status.category, "todo");
    assert_eq!(sub.hierarchy_level, Some(-1));
    assert_eq!(sub.parent_key.as_deref(), Some("TEAM-7"));
}

#[tokio::test]
async fn an_incremental_listing_uses_a_relative_window_with_the_callers_overlap() {
    let f = FakeTransport::new();
    f.once(Method::Post, "/search/jql", ok("search_mine_p2.json"));
    let since = crate::service::catalog::now_secs() - 10 * 60 - 5;
    jira(&f)
        .list(&view("mine", VIEW_MINE), Some(since), None)
        .await
        .unwrap();
    let jql = f.requests()[0].json_body().unwrap()["jql"].clone();
    assert_eq!(
        jql,
        format!("({VIEW_MINE}) AND updated >= -11m ORDER BY updated DESC"),
        "rounded up to whole minutes: never a gap"
    );
}

#[tokio::test]
async fn a_repeating_page_token_ends_the_listing_instead_of_looping() {
    let f = FakeTransport::new();
    f.always(Method::Post, "/search/jql", ok("search_repeat.json"));
    let items = list_all(&jira(&f), &view("mine", VIEW_MINE), None)
        .await
        .unwrap();
    assert_eq!(
        f.count("/search/jql"),
        2,
        "the second page repeated the token"
    );
    assert_eq!(
        items.len(),
        2,
        "what was read is kept; dedupe is the sync's"
    );
}

#[tokio::test]
async fn fetch_by_id_marks_missing_items_unavailable_never_gone() {
    let f = FakeTransport::new();
    f.once(Method::Post, "/issue/bulkfetch", ok("bulkfetch.json"));
    let got = jira(&f)
        .fetch(&[ItemRef::Id("10101".into()), ItemRef::Id("10999".into())])
        .await
        .unwrap();
    match &got[0] {
        Fetched::Found(s) => {
            assert_eq!(s.status.category, "done");
            assert_eq!(s.status.resolution.as_deref(), Some("completed"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        got[1],
        Fetched::Unavailable {
            reference: "10999".into(),
            reason: NOT_FOUND_OR_NO_PERMISSION.into()
        }
    );
    let body = f.requests()[0].json_body().unwrap();
    assert_eq!(body["issueIdsOrKeys"], json!(["10101", "10999"]));
}

/// Jira Cloud answers an expired or revoked API token as an anonymous
/// caller: `bulkfetch` is a 200 with no issues and no `issueErrors`, and
/// only `/myself` says 401 (seen live, 2026-10). An all-empty answer is
/// checked against `/myself` before anything is called unavailable.
#[tokio::test]
async fn an_all_empty_fetch_from_a_dead_token_is_auth_not_unavailable() {
    let empty = || {
        Ok(Response::json(
            200,
            &json!({"issues": [], "issueErrors": []}),
        ))
    };
    let f = FakeTransport::new();
    f.once(Method::Post, "/issue/bulkfetch", empty()).once(
        Method::Get,
        "/rest/api/3/myself",
        Ok(Response::new(401, "")),
    );
    let e = jira(&f)
        .fetch(&[ItemRef::Id("10101".into()), ItemRef::Id("10999".into())])
        .await
        .unwrap_err();
    assert!(matches!(e, TrackerError::Auth(_)), "{e:?}");
    assert_eq!(e.state(), Some("auth_failed"));

    // A live token that truly sees none of them: unavailable, as before.
    let f = FakeTransport::new();
    f.once(Method::Post, "/issue/bulkfetch", empty()).once(
        Method::Get,
        "/rest/api/3/myself",
        ok("myself.json"),
    );
    let got = jira(&f)
        .fetch(&[ItemRef::Id("10999".into())])
        .await
        .unwrap();
    assert!(matches!(got[0], Fetched::Unavailable { .. }), "{got:?}");

    // Something came back: no identity call.
    let f = FakeTransport::new();
    f.once(Method::Post, "/issue/bulkfetch", ok("bulkfetch.json"));
    jira(&f)
        .fetch(&[ItemRef::Id("10101".into()), ItemRef::Id("10999".into())])
        .await
        .unwrap();
    assert_eq!(f.count("/myself"), 0);
}

/// An empty view is checked the same way: a dead token would otherwise
/// read as "nothing assigned to me" and the tracker would stay ok. A live
/// token's empty view costs one `/myself` per provider, not one per view.
#[tokio::test]
async fn an_empty_view_from_a_dead_token_is_auth_and_a_live_one_is_asked_once() {
    let empty = || Ok(Response::json(200, &json!({"issues": [], "isLast": true})));
    let f = FakeTransport::new();
    f.once(Method::Post, "/search/jql", empty()).once(
        Method::Get,
        "/rest/api/3/myself",
        Ok(Response::new(401, "")),
    );
    let e = jira(&f)
        .list(&view("mine", VIEW_MINE), None, None)
        .await
        .unwrap_err();
    assert_eq!(e.state(), Some("auth_failed"), "{e:?}");

    let f = FakeTransport::new();
    f.always(Method::Post, "/search/jql", empty()).once(
        Method::Get,
        "/rest/api/3/myself",
        ok("myself.json"),
    );
    let j = jira(&f);
    for v in [view("mine", VIEW_MINE), view("recent", VIEW_RECENT)] {
        assert!(j.list(&v, None, None).await.unwrap().items.is_empty());
    }
    assert_eq!(f.count("/myself"), 1, "once per provider");
}

#[tokio::test]
async fn fetch_by_an_old_key_reveals_a_moved_issue_as_an_alias() {
    let f = FakeTransport::new();
    f.once(Method::Post, "/issue/bulkfetch", ok("bulkfetch_moved.json"));
    let got = jira(&f)
        .fetch(&[ItemRef::Key("old-5".into())])
        .await
        .unwrap();
    let Fetched::Found(s) = &got[0] else {
        panic!("{got:?}")
    };
    assert_eq!(
        (s.external_id.as_str(), s.key.as_deref()),
        ("30005", Some("NEW-5"))
    );
    assert_eq!(s.aliases, vec!["OLD-5"]);
    assert_eq!(s.status.category, "todo", "`new` is todo");
}

/// A moved key is recognised when other references share its chunk: the
/// one answer nothing asked for is its issue (`issueErrors` names the keys
/// that truly failed), and two moved keys are asked for again one at a time.
#[tokio::test]
async fn a_moved_key_is_found_next_to_other_references() {
    let moved = fixture("bulkfetch_moved.json")["issues"][0].clone();
    let abc101 = fixture("bulkfetch.json")["issues"][0].clone();
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/issue/bulkfetch",
        Ok(Response::json(
            200,
            &json!({
                "issues": [moved, abc101],
                "issueErrors": [{"issueIdsOrKeys": ["ABC-999"], "errorMessages": ["Issue does not exist or you do not have permission to see it."]}]
            }),
        )),
    );
    let got = jira(&f)
        .fetch(&[
            ItemRef::Key("old-5".into()),
            ItemRef::Key("ABC-101".into()),
            ItemRef::Key("ABC-999".into()),
        ])
        .await
        .unwrap();
    let Fetched::Found(s) = &got[0] else {
        panic!("{got:?}")
    };
    assert_eq!(
        (s.external_id.as_str(), s.key.as_deref()),
        ("30005", Some("NEW-5"))
    );
    assert_eq!(s.aliases, vec!["OLD-5"]);
    assert!(matches!(&got[1], Fetched::Found(s) if s.external_id == "10101"));
    assert!(
        matches!(&got[2], Fetched::Unavailable { .. }),
        "{:?}",
        got[2]
    );
    assert_eq!(f.count("/issue/bulkfetch"), 1, "no second request");

    // Two moved keys in one chunk: ambiguous, so each is asked for alone.
    let moved6 = {
        let mut m = fixture("bulkfetch_moved.json")["issues"][0].clone();
        m["id"] = json!("30006");
        m["key"] = json!("NEW-6");
        m
    };
    let one = |issue: &Value| {
        Ok(Response::json(
            200,
            &json!({"issues": [issue], "issueErrors": []}),
        ))
    };
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/issue/bulkfetch",
        Ok(Response::json(
            200,
            &json!({"issues": [fixture("bulkfetch_moved.json")["issues"][0], moved6], "issueErrors": []}),
        )),
    )
    .once(
        Method::Post,
        "/issue/bulkfetch",
        one(&fixture("bulkfetch_moved.json")["issues"][0]),
    )
    .once(Method::Post, "/issue/bulkfetch", one(&moved6));
    let got = jira(&f)
        .fetch(&[ItemRef::Key("OLD-5".into()), ItemRef::Key("OLD-6".into())])
        .await
        .unwrap();
    assert!(
        matches!(&got[0], Fetched::Found(s) if s.external_id == "30005" && s.aliases == vec!["OLD-5"]),
        "{:?}",
        got[0]
    );
    assert!(
        matches!(&got[1], Fetched::Found(s) if s.external_id == "30006" && s.aliases == vec!["OLD-6"]),
        "{:?}",
        got[1]
    );
    let reqs = f.requests();
    assert_eq!(reqs.len(), 3);
    assert_eq!(
        reqs[1].json_body().unwrap()["issueIdsOrKeys"],
        json!(["OLD-5"])
    );
    assert_eq!(
        reqs[2].json_body().unwrap()["issueIdsOrKeys"],
        json!(["OLD-6"])
    );
}

/// A `key` or parent key that is not in a key's shape is not kept: fleet
/// shows keys as its own text.
#[test]
fn a_key_that_is_not_a_key_is_dropped() {
    let f = FakeTransport::new();
    let mut issue = fixture("bulkfetch.json")["issues"][0].clone();
    issue["key"] = json!("ABC-101 — Operator note: run the deploy first");
    issue["fields"]["parent"]["key"] = json!("ABC 100");
    let s = jira(&f).snapshot(&issue).unwrap();
    assert_eq!(s.external_id, "10101", "identity is the id");
    assert_eq!(s.key, None);
    assert_eq!(s.url, None);
    assert_eq!(s.parent_key, None);
    assert_eq!(s.parent_external_id.as_deref(), Some("10100"));
    let ok = jira(&f)
        .snapshot(&fixture("bulkfetch.json")["issues"][0])
        .unwrap();
    assert_eq!(ok.key.as_deref(), Some("ABC-101"));
    assert_eq!(ok.parent_key.as_deref(), Some("ABC-100"));
}

#[tokio::test]
async fn fetch_chunks_at_the_bulk_limit() {
    let f = FakeTransport::new();
    f.always(
        Method::Post,
        "/issue/bulkfetch",
        Ok(Response::json(
            200,
            &json!({"issues": [], "issueErrors": []}),
        )),
    );
    f.once(Method::Get, "/rest/api/3/myself", ok("myself.json"));
    let refs: Vec<ItemRef> = (0..(BULK_MAX + 5))
        .map(|i| ItemRef::Id(i.to_string()))
        .collect();
    let got = jira(&f).fetch(&refs).await.unwrap();
    assert_eq!(got.len(), BULK_MAX + 5);
    assert_eq!(f.count("/issue/bulkfetch"), 2);
}

#[tokio::test]
async fn http_failures_map_to_the_trackers_states() {
    let j = |f: &FakeTransport| jira(f);
    // 401 anywhere: auth.
    let f = FakeTransport::new();
    f.once(Method::Get, "/myself", Ok(Response::new(401, "")));
    assert!(matches!(j(&f).probe().await, Err(TrackerError::Auth(_))));
    // 403 on /myself: auth; 403 on a view: that view only.
    let f = FakeTransport::new();
    f.once(Method::Get, "/myself", Ok(Response::new(403, "")));
    assert!(matches!(j(&f).probe().await, Err(TrackerError::Auth(_))));
    let f = FakeTransport::new();
    f.once(Method::Post, "/search/jql", Ok(Response::new(403, "")));
    let e = j(&f)
        .list(&view("filter:1", "filter = 1"), None, None)
        .await
        .unwrap_err();
    assert!(matches!(e, TrackerError::Forbidden(_)));
    assert_eq!(e.state(), None, "the tracker stays ok");
    // CAPTCHA.
    let f = FakeTransport::new();
    f.once(
        Method::Get,
        "/myself",
        Ok(Response::new(403, "").with_header("X-Seraph-LoginReason", "AUTHENTICATION_DENIED")),
    );
    let e = j(&f).probe().await.unwrap_err();
    assert_eq!(e, TrackerError::Captcha);
    assert_eq!(e.state(), Some("captcha"));
    // 429 with Retry-After.
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/search/jql",
        Ok(Response::new(429, "").with_header("Retry-After", "30")),
    );
    assert_eq!(
        j(&f)
            .list(&view("mine", VIEW_MINE), None, None)
            .await
            .unwrap_err(),
        TrackerError::RateLimited {
            retry_after_secs: Some(30)
        }
    );
    // Offline: no route answers, like a dead network.
    let f = FakeTransport::new();
    let e = j(&f).probe().await.unwrap_err();
    assert!(matches!(e, TrackerError::Unreachable(_)));
    assert_eq!(e.state(), Some("unreachable"));
    // A redirect is not followed.
    let f = FakeTransport::new();
    f.once(
        Method::Get,
        "/myself",
        Ok(Response::new(302, "").with_header("Location", "https://id.atlassian.com/login")),
    );
    assert!(
        matches!(j(&f).probe().await, Err(TrackerError::Invalid(m)) if m.contains("not followed"))
    );
    // No credential: nothing is sent.
    let f = FakeTransport::new();
    let none = JiraCloud::new(SITE, config(), None, Arc::new(f.clone()));
    assert_eq!(none.probe().await.unwrap_err(), TrackerError::Unconfigured);
    assert!(f.requests().is_empty());
}

#[test]
fn ticket_urls_name_their_site_and_key() {
    assert_eq!(
        parse_ticket_url("https://Acme.atlassian.net/browse/abc-123"),
        Some(("https://acme.atlassian.net".into(), "ABC-123".into()))
    );
    assert_eq!(
        parse_ticket_url(
            "https://acme.atlassian.net/jira/software/projects/ABC/boards/2?selectedIssue=ABC-9&x=1"
        ),
        Some(("https://acme.atlassian.net".into(), "ABC-9".into()))
    );
    for bad in [
        "https://evil.example.com/browse/ABC-1",
        "http://acme.atlassian.net/browse/ABC-1",
        "https://acme.atlassian.net/browse/not-a-key",
        "https://acme.atlassian.net/",
    ] {
        assert_eq!(parse_ticket_url(bad), None, "{bad}");
    }
}

#[test]
fn keys_are_recognised_only_with_known_prefixes_and_boundaries() {
    let p = vec!["ABC".to_string(), "TEAM".to_string()];
    assert_eq!(
        keys_in_text(
            "fix abc-12-rounding, then TEAM-7; not XABC-1 or ABC-12x or ZZZ-1; ABC-12 again",
            &p
        ),
        vec!["ABC-12", "TEAM-7"]
    );
    assert!(keys_in_text("ABC-1", &[]).is_empty());
    let f = FakeTransport::new();
    let refs = jira(&f).recognize(
        "see https://acme.atlassian.net/browse/ABC-101 and https://other.atlassian.net/browse/ZED-1 and team-8, \
         not https://partner.atlassian.net/browse/ABC-9 (another site's ABC)",
        RefCtx::default(),
    );
    assert_eq!(
        refs,
        vec![
            ItemRef::Key("ABC-101".into()),
            ItemRef::Key("TEAM-8".into())
        ]
    );
}

#[test]
fn resolutions_are_told_apart_conservatively() {
    for (name, want) in [
        ("Done", "completed"),
        ("Fixed", "completed"),
        ("Won't Do", "not_planned"),
        ("Won\u{2019}t Fix", "not_planned"),
        ("Declined", "not_planned"),
        ("Duplicate", "duplicate"),
        ("Duplicate of another", "duplicate"),
        ("Cannot Reproduce", "completed"),
        ("Custom resolution", "completed"),
    ] {
        assert_eq!(normalize_resolution(name), want, "{name}");
    }
    assert_eq!(map_status_category(Some("undefined")), "todo");
    assert_eq!(map_status_category(Some("new")), "todo");
    assert_eq!(map_status_category(None), "todo");
}

/// A description the excerpt CUT reports more than it shows, by the block
/// separators `adf_walk` counts on its way past the cap: that number only
/// has to say "there is more". The figures here are exact all the same — the
/// walk is deterministic, and the tolerance this helper used to allow (±5)
/// is what let the over-count of a COMPLETE description ship, firing the
/// notice on every Jira ticket. A complete description's length is asserted
/// against the text it returns, with `==`, by
/// [`assert_reports_the_length_it_returns`].
fn assert_cut_length(actual: Option<i64>, expected: i64) {
    assert_eq!(actual, Some(expected));
}

/// A snapshot whose description was NOT cut: `description_chars` must equal
/// the length of the description actually returned, or `fence_ticket` writes
/// a "there is more" notice for a whole description.
fn assert_reports_the_length_it_returns(snap: &WorkItemSnapshot) {
    let d = snap.description.as_deref().expect("a description");
    assert_eq!(
        snap.description_chars,
        Some(d.chars().count() as i64),
        "a complete description must report exactly what it returns: {d:?}"
    );
}

#[test]
fn adf_excerpts_are_capped() {
    let long = json!({"type":"doc","content":[{"type":"paragraph","content":[
        {"type":"text","text": "x".repeat(DESCRIPTION_MAX_CHARS * 2)}]}]});
    let (excerpt, chars) = adf_excerpt(&long);
    assert_eq!(excerpt.unwrap().chars().count(), DESCRIPTION_MAX_CHARS);
    // The text, plus the one paragraph's separator.
    assert_cut_length(chars, (DESCRIPTION_MAX_CHARS * 2 + 1) as i64);
    assert_eq!(adf_excerpt(&Value::Null), (None, None));
    assert_eq!(
        adf_excerpt(&json!({"type":"doc","content":[]})),
        (None, None)
    );
}

/// Fetch one Jira issue over [`FakeTransport`], with `description` as its
/// ADF description body.
async fn fetch_one_with_body(description: Value) -> WorkItemSnapshot {
    let f = FakeTransport::new();
    let mut body = fixture("bulkfetch.json");
    body["issues"][0]["fields"]["description"] = description;
    f.once(
        Method::Post,
        "/issue/bulkfetch",
        Ok(Response::json(200, &body)),
    );
    let got = jira(&f)
        .fetch(&[ItemRef::Id("10101".into())])
        .await
        .unwrap();
    match got.into_iter().next().unwrap() {
        Fetched::Found(s) => *s,
        other => panic!("{other:?}"),
    }
}

/// Fetch one Jira issue over [`FakeTransport`], with `text` as its
/// description body (a single ADF text node — the shape
/// [`adf_excerpts_are_capped`] uses for a description longer than the cap).
async fn fetch_one_with_description(text: &str) -> WorkItemSnapshot {
    fetch_one_with_body(json!({"type":"doc","content":[
        {"type":"paragraph","content":[{"type":"text","text": text}]}
    ]}))
    .await
}

#[tokio::test]
async fn a_long_jira_description_reports_its_true_length() {
    // The existing cap test in this file builds an ADF body of
    // DESCRIPTION_MAX_CHARS * 2 characters; reuse that fixture shape.
    let snap = fetch_one_with_description(&"x".repeat(DESCRIPTION_MAX_CHARS * 2)).await;
    assert_eq!(
        snap.description.as_ref().map(|d| d.chars().count()),
        Some(DESCRIPTION_MAX_CHARS)
    );
    assert_cut_length(
        snap.description_chars,
        (DESCRIPTION_MAX_CHARS * 2 + 1) as i64,
    );
}

/// The case the branch's own signal depends on and no test drove through an
/// adapter: a description the tracker holds WHOLE. Its reported length must
/// equal the description returned, exactly — `fence_ticket` compares
/// `full > shown`, so one character of drift puts a "there is more" notice on
/// every complete Jira ticket. `lookup`'s end of it is
/// `tests_tickets.rs::a_complete_jira_cloud_description_gets_no_notice`.
#[tokio::test]
async fn a_complete_jira_description_reports_the_length_it_returns() {
    // One paragraph, well under the cap.
    let snap = fetch_one_with_description("Ship the refund flow.").await;
    assert_eq!(
        snap.description.as_deref(),
        Some("Ship the refund flow."),
        "nothing was cut"
    );
    assert_reports_the_length_it_returns(&snap);
    // Several blocks: the separators between them are part of the text that
    // is returned, so they are part of the length reported.
    let snap = fetch_one_with_body(json!({"type":"doc","content":[
        {"type":"heading","content":[{"type":"text","text":"Context"}]},
        {"type":"paragraph","content":[{"type":"text","text":"Refunds fail."}]},
        {"type":"paragraph","content":[{"type":"text","text":"Fix them."}]}]}))
    .await;
    assert_eq!(
        snap.description.as_deref(),
        Some("Context\nRefunds fail.\nFix them.")
    );
    assert_reports_the_length_it_returns(&snap);
}

/// The Jira Data Center shape of the same rule: a v2 body arrives as a plain
/// string, and its trailing newline used to be counted into the length while
/// the excerpt's `.trim()` removed it.
#[tokio::test]
async fn a_complete_plain_string_description_reports_the_length_it_returns() {
    let snap = fetch_one_with_body(json!("Ship the refund flow.\n\n")).await;
    assert_eq!(snap.description.as_deref(), Some("Ship the refund flow."));
    assert_reports_the_length_it_returns(&snap);
}

#[tokio::test]
async fn a_jira_description_spread_across_many_small_nodes_counts_them_all() {
    // The normal shape of a real Jira description: several small blocks
    // (here, 40 paragraphs of 100 characters — 4000 total) rather than one
    // giant text node. Each block individually stays far under the cap, so
    // `adf_walk`'s append-guard would previously also stop *counting* once
    // the running excerpt crossed DESCRIPTION_MAX_CHARS partway through —
    // `a_long_jira_description_reports_its_true_length`'s single-node
    // fixture cannot reach that bug, because one `push_str` appends
    // everything before the guard is next checked.
    let paragraphs: Vec<Value> = (0..40)
        .map(|_| {
            json!({"type": "paragraph", "content": [
                {"type": "text", "text": "x".repeat(100)}
            ]})
        })
        .collect();
    let snap = fetch_one_with_body(json!({"type": "doc", "content": paragraphs})).await;
    assert_eq!(
        snap.description.as_ref().map(|d| d.chars().count()),
        Some(DESCRIPTION_MAX_CHARS),
        "the excerpt itself is still capped"
    );
    // 4000 characters of text plus one separator per paragraph: the true
    // length, not the ~2000 the excerpt stopped appending at.
    assert_cut_length(snap.description_chars, 4000 + 40);
}

/// `describe` reads the single-issue endpoint directly (not `bulkfetch`),
/// asks only for `description`, and is uncapped by `DESCRIPTION_MAX_CHARS` —
/// capped only at `DESCRIBE_MAX_CHARS`.
#[tokio::test]
async fn describe_reads_the_single_issue_endpoint_uncapped() {
    let f = FakeTransport::new();
    let long = "x".repeat(DESCRIPTION_MAX_CHARS + 500);
    let body = json!({
        "id": "10101",
        "key": "ABC-101",
        "fields": { "description": {"type":"doc","content":[
            {"type":"paragraph","content":[{"type":"text","text": long}]}
        ]} }
    });
    f.once(
        Method::Get,
        "/issue/ABC-101?fields=description",
        Ok(Response::json(200, &body)),
    );
    let p = jira(&f);
    assert!(p.caps().describe, "Jira Cloud implements describe");
    let out = p
        .describe(&ItemRef::Key("ABC-101".into()))
        .await
        .unwrap()
        .expect("a describe answer");
    assert_eq!(out.text.chars().count(), DESCRIPTION_MAX_CHARS + 500);
    assert_eq!(out.chars, (DESCRIPTION_MAX_CHARS + 500) as i64);
    let sent = f.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].method, Method::Get);
    assert!(
        sent[0]
            .url
            .ends_with("/rest/api/3/issue/ABC-101?fields=description"),
        "{}",
        sent[0].url
    );

    // A key shape a real Jira key can never be: refused, not interpolated
    // into a path (the same fence `write` gives a bad key).
    let f = FakeTransport::new();
    assert!(jira(&f)
        .describe(&ItemRef::Key("../../myself".into()))
        .await
        .unwrap()
        .is_none());
    assert!(f.requests().is_empty());
}

/// Past `DESCRIBE_MAX_CHARS` the text is cut there, and the true length is
/// reported with it — `describe` turns that into "shown N of M".
#[tokio::test]
async fn describe_reports_the_true_length_past_its_own_cap() {
    let cap = crate::service::trackers::DESCRIBE_MAX_CHARS;
    let f = FakeTransport::new();
    let body = json!({ "fields": { "description": {"type":"doc","content":[
        {"type":"paragraph","content":[{"type":"text","text": "x".repeat(cap + 99)}]}
    ]} } });
    f.once(
        Method::Get,
        "/issue/ABC-101?fields=description",
        Ok(Response::json(200, &body)),
    );
    let out = jira(&f)
        .describe(&ItemRef::Key("ABC-101".into()))
        .await
        .unwrap()
        .expect("a describe answer");
    assert_eq!(out.text.chars().count(), cap);
    assert!(out.chars >= (cap + 99) as i64, "{}", out.chars);
}

// --- the provider conformance suite (M6.0) -----------------------------------

struct JiraHarness;

#[async_trait::async_trait]
impl crate::service::trackers::conformance::Harness for JiraHarness {
    fn name(&self) -> &'static str {
        "jira"
    }

    fn expect(&self) -> crate::service::trackers::conformance::Expect {
        crate::service::trackers::conformance::Expect {
            instance_id: Some("11111111-2222-3333-4444-555555555555"),
            me: ME,
            prefixes: vec!["ABC", "TEAM"],
            view: view("mine", VIEW_MINE),
            list_ids: vec![
                "10100", "10101", "10102", "10103", "10104", "20007", "20008",
            ],
            fetch: [
                ItemRef::Id("10101".into()),
                ItemRef::Key("ABC-102".into()),
                ItemRef::Id("10999".into()),
            ],
            statuses: vec![
                ("10101", "in_progress", None),
                ("10102", "done", Some("completed")),
                ("10103", "done", Some("not_planned")),
                ("10104", "done", Some("duplicate")),
                ("20008", "todo", None),
            ],
            hierarchy: Some(("20008", "20007", Some(-1))),
            moved: Some((ItemRef::Key("OLD-5".into()), "30005", "OLD-5")),
            recognize: vec![
                (
                    "see https://acme.atlassian.net/browse/ABC-101 and team-8",
                    None,
                    vec![
                        ItemRef::Key("ABC-101".into()),
                        ItemRef::Key("TEAM-8".into()),
                    ],
                ),
                (
                    "ZZZ-1 and https://other.atlassian.net/browse/ZED-1 and https://partner.atlassian.net/browse/ABC-9",
                    None,
                    vec![],
                ),
            ],
            bare_repo: "",
            secret: Some("ATATT3xFfGF0-test-token-not-real-0000"),
            describe: true,
        }
    }

    fn provider(&self, fake: &FakeTransport) -> Box<dyn TrackerProvider> {
        Box::new(jira(fake))
    }

    fn script_probe(&self, f: &FakeTransport) {
        f.once(Method::Get, "/rest/api/3/myself", ok("myself.json"))
            .once(Method::Get, "/_edge/tenant_info", ok("tenant_info.json"))
            .once(Method::Get, "startAt=0", ok("project_search_p1.json"))
            .once(Method::Get, "startAt=1", ok("project_search_p2.json"))
            .once(Method::Get, "/rest/api/3/field", ok("fields.json"))
            .once(Method::Post, "/search/jql", ok("sprint_projects.json"))
            .once(
                Method::Get,
                "/filter/favourite",
                Ok(Response::json(200, &json!([]))),
            );
    }

    fn script_list(&self, f: &FakeTransport) {
        f.once(Method::Post, "/search/jql", ok("search_mine_p1.json"))
            .once(Method::Post, "/search/jql", ok("search_mine_p2.json"));
    }

    async fn incremental(
        &self,
        p: &dyn TrackerProvider,
        f: &FakeTransport,
    ) -> Result<Vec<WorkItemSnapshot>, TrackerError> {
        f.once(Method::Post, "/search/jql", ok("search_mine_p2.json"));
        let since = crate::service::catalog::now_secs() - 10 * 60 - 5;
        let page = p.list(&view("mine", VIEW_MINE), Some(since), None).await?;
        let jql = f.requests()[0].json_body().unwrap()["jql"].clone();
        assert!(jql.as_str().unwrap().contains("updated >= -11m"), "{jql}");
        Ok(page.items)
    }

    fn script_fetch(&self, f: &FakeTransport) {
        f.once(Method::Post, "/issue/bulkfetch", ok("bulkfetch_two.json"));
    }

    fn script_moved(&self, f: &FakeTransport) {
        f.once(Method::Post, "/issue/bulkfetch", ok("bulkfetch_moved.json"));
    }

    fn script_error(
        &self,
        f: &FakeTransport,
        case: crate::service::trackers::conformance::ErrorCase,
    ) {
        use crate::service::trackers::conformance::ErrorCase as E;
        match case {
            E::Unauthorized => {
                f.once(Method::Get, "/myself", Ok(Response::new(401, "")));
            }
            E::ForbiddenView => {
                f.once(Method::Post, "/search/jql", Ok(Response::new(403, "")));
            }
            E::RateLimited => {
                f.once(
                    Method::Post,
                    "/search/jql",
                    Ok(Response::new(429, "").with_header("Retry-After", "30")),
                );
            }
            E::Offline => {}
            E::Garbage => {
                f.once(
                    Method::Post,
                    "/search/jql",
                    Ok(Response::new(200, "<html>maintenance</html>")),
                );
            }
        }
    }

    fn script_describe(&self, f: &FakeTransport) {
        f.once(Method::Get, "/issue/ABC-101", ok("issue_description.json"));
    }

    fn describe_ref(&self) -> &'static str {
        "ABC-101"
    }
    fn version(&self) -> Option<(&'static str, &'static str)> {
        Some(("10101", "0.3.0"))
    }
}

crate::conformance_suite!(JiraHarness);

/// M6.3 acceptance 4 in miniature: a tracker only reachable from host X is
/// read through X with `curl`; the credential is on stdin, in no argv.
#[tokio::test]
async fn a_via_host_tracker_is_read_through_curl_on_its_host() {
    use crate::service::trackers::{provider_for, TrackerNet};
    use crate::ssh_fake::{FakeSsh, Match, Reply};
    let body = fixture("bulkfetch.json").to_string();
    let head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n";
    let ssh = FakeSsh::new();
    ssh.on_host(
        "vpnbox",
        Match::contains("/rest/api/3/issue/bulkfetch"),
        Reply::ok(&format!(
            "__fleet_status__=200\n__fleet_curl_exit__=0\n__fleet_head__={}\n{head}{body}",
            head.len()
        )),
    );
    let mut row = crate::store::Store::open_in_memory()
        .unwrap()
        .add_tracker("jira", "Acme", SITE)
        .unwrap();
    row.transport = "via_host:vpnbox".into();
    let p = provider_for(
        &row,
        Some(cred()),
        &TrackerNet::with_ssh(Arc::new(ssh.clone())),
    )
    .unwrap();
    let got = p.fetch(&[ItemRef::Id("10101".into())]).await.unwrap();
    assert!(matches!(&got[0], Fetched::Found(s) if s.key.as_deref() == Some("ABC-101")));
    let call = &ssh.calls_for("vpnbox")[0];
    let secret = cred().secret.expose().to_string();
    let basic = cred().literals()[1].clone();
    assert!(!call.command().contains(&secret) && !call.command().contains(&basic));
    let stdin = call.stdin_str().unwrap();
    assert!(stdin.contains(&format!("Authorization: Basic {basic}")));
    // The host fence is the provider's: a via_host Jira still only talks
    // to *.atlassian.net.
    let evil = crate::net::https::Request::get("https://evil.example.com/x");
    let t = TrackerNet::with_ssh(Arc::new(ssh.clone()))
        .transport_for(&row)
        .unwrap();
    assert!(matches!(
        t.send(evil).await,
        Err(TransportError::Refused(_))
    ));
}

#[test]
fn jira_priority_names_map_onto_fleets_four() {
    for (name, want) in [
        ("Highest", Some("urgent")),
        ("Blocker", Some("urgent")),
        ("Critical", Some("urgent")),
        ("High", Some("high")),
        ("Major", Some("high")),
        ("Medium", Some("medium")),
        (" medium ", Some("medium")),
        ("Low", Some("low")),
        ("Lowest", Some("low")),
        ("Minor", Some("low")),
        ("Trivial", Some("low")),
        ("P1", None),
        ("", None),
    ] {
        assert_eq!(
            crate::service::trackers::jira_common::priority(&json!({ "name": name })).as_deref(),
            want,
            "{name:?}"
        );
    }
    assert_eq!(
        crate::service::trackers::jira_common::priority(&Value::Null),
        None
    );
}

#[test]
fn priority_labels_and_estimate_are_asked_for_and_read() {
    let f = FakeTransport::new();
    let j = jira(&f);
    for want in [
        "priority",
        "labels",
        "timeoriginalestimate",
        "customfield_10016",
    ] {
        assert!(j.fields().iter().any(|x| x == want), "{want}");
    }
    let page = fixture("search_mine_p1.json");
    let s = |i: usize| j.snapshot(&page["issues"][i]).unwrap();
    // ABC-101: High, its labels, and story points over its time estimate.
    let story = s(1);
    assert_eq!(story.priority.as_deref(), Some("high"));
    assert_eq!(story.labels, vec!["billing", " ledger ", "billing", ""]);
    assert_eq!(
        (story.estimate.map(|e| e.0), story.estimate_unit.as_deref()),
        (Some(5.0), Some("points"))
    );
    // Stored tidied: trimmed, each once, nothing empty.
    let w = crate::service::trackers::sync::to_write(story);
    assert_eq!(w.labels, vec!["billing", "ledger"]);
    // ABC-100: Highest, no points, 28800 s of original estimate = 8 h.
    let epic = s(0);
    assert_eq!(epic.priority.as_deref(), Some("urgent"));
    assert!(epic.labels.is_empty());
    assert_eq!(
        (epic.estimate.map(|e| e.0), epic.estimate_unit.as_deref()),
        (Some(8.0), Some("hours"))
    );
    // ABC-102 has none of them.
    let done = s(2);
    assert_eq!(
        (done.priority, done.estimate, done.estimate_unit),
        (None, None, None)
    );
}

#[test]
fn the_due_date_is_asked_for_and_read_as_a_calendar_date() {
    let f = FakeTransport::new();
    let j = jira(&f);
    assert!(j.fields().iter().any(|x| x == "duedate"));
    let issue = |due: Value| {
        json!({ "id": "10101", "key": "ABC-101",
                "fields": { "summary": "S", "duedate": due,
                            "status": { "name": "To Do", "statusCategory": { "key": "new" } } } })
    };
    let due = |v: Value| j.snapshot(&issue(v)).unwrap().due_at;
    assert_eq!(due(json!("2026-10-16")).as_deref(), Some("2026-10-16"));
    assert_eq!(due(Value::Null), None);
    // Third-party text that is not a date is dropped, not stored.
    assert_eq!(due(json!("next Friday")), None);
    assert_eq!(due(json!("2026-02-30")), None);
}

#[tokio::test]
async fn issue_threads_read_each_issue_with_its_comments_as_text() {
    let f = FakeTransport::new();
    let page = serde_json::json!({
        "isLast": true,
        "issues": [{
            "key": "L2S-7",
            "fields": {
                "summary": "Payments fail",
                "description": {"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Since 9:00"}]}]},
                "assignee": {"accountId": "bot-1"},
                "status": {"name": "Open"},
                "updated": "2026-10-11T09:30:00.000+0200",
                "comment": {"comments": [
                    {"id": "100", "author": {"accountId": "ann", "displayName": "Ann"},
                     "body": {"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Tech help!"}]}]},
                     "created": "2026-10-11T09:31:00.000+0200"},
                    {"author": {"displayName": "no id"}, "body": "x"}
                ]}
            }
        }, {"fields": {"summary": "no key"}}]
    });
    f.once(Method::Post, "/search/jql", Ok(Response::json(200, &page)));
    let threads = jira(&f).issue_threads("project = L2S", 10).await.unwrap();
    assert_eq!(threads.len(), 1);
    let t = &threads[0];
    assert_eq!(
        (t.key.as_str(), t.summary.as_str()),
        ("L2S-7", "Payments fail")
    );
    assert_eq!(t.description, "Since 9:00");
    assert_eq!(
        (t.assignee_id.as_deref(), t.status.as_str()),
        (Some("bot-1"), "Open")
    );
    assert_eq!(t.updated, Some(1_791_703_800));
    assert_eq!(t.comments.len(), 1, "a comment without an id is dropped");
    let c = &t.comments[0];
    assert_eq!(
        (c.id.as_str(), c.author_id.as_deref(), c.body.as_str()),
        ("100", Some("ann"), "Tech help!")
    );
    let body = f.requests()[0].json_body().unwrap();
    assert_eq!(body["jql"], "project = L2S");
    assert!(body["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "comment"));
}

#[tokio::test]
async fn the_support_agents_writes_create_link_and_comment_in_adf() {
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/rest/api/3/issueLink",
        Ok(Response::new(201, "")),
    );
    f.once(
        Method::Post,
        "/rest/api/3/issue/L2S-7/comment",
        Ok(Response::json(201, &serde_json::json!({ "id": "300" }))),
    );
    f.once(
        Method::Post,
        "/rest/api/3/issue",
        Ok(Response::json(
            201,
            &serde_json::json!({ "id": "1", "key": "PD-12" }),
        )),
    );
    let adf = serde_json::json!({ "type": "doc", "version": 1, "content": [] });
    let j = jira(&f);
    let created = j
        .write(&WriteOp::CreateIssue {
            project: "PD".into(),
            issue_type: "Bug".into(),
            summary: "[payments] NPE".into(),
            adf: adf.clone(),
            wiki: "h2. x".into(),
            labels: vec!["payments".into()],
        })
        .await
        .unwrap();
    assert_eq!(created.remote_id.as_deref(), Some("PD-12"));
    assert!(created.url.unwrap().ends_with("/browse/PD-12"));
    j.write(&WriteOp::LinkIssues {
        link_type: "Causes".into(),
        inward: "PD-12".into(),
        outward: "l2s-7".into(),
    })
    .await
    .unwrap();
    let posted = j
        .write(&WriteOp::RichComment {
            key: "L2S-7".into(),
            adf: adf.clone(),
            wiki: "h3. x".into(),
        })
        .await
        .unwrap();
    assert_eq!(posted.remote_id.as_deref(), Some("300"));
    let sent = f.requests();
    let issue = sent[0].json_body().unwrap();
    assert_eq!(issue["fields"]["project"]["key"], "PD");
    assert_eq!(issue["fields"]["description"], adf);
    assert_eq!(issue["fields"]["labels"][0], "payments");
    let link = sent[1].json_body().unwrap();
    assert_eq!(
        (
            &link["type"]["name"],
            &link["inwardIssue"]["key"],
            &link["outwardIssue"]["key"]
        ),
        (
            &Value::from("Causes"),
            &Value::from("PD-12"),
            &Value::from("L2S-7")
        )
    );
    assert_eq!(
        sent[2].json_body().unwrap()["body"],
        adf,
        "Cloud gets the ADF"
    );
}

#[tokio::test]
async fn a_bad_project_summary_or_key_is_refused_before_sending() {
    let f = FakeTransport::new();
    let j = jira(&f);
    let create = |project: &str, summary: &str| WriteOp::CreateIssue {
        project: project.into(),
        issue_type: "Bug".into(),
        summary: summary.into(),
        adf: Value::Null,
        wiki: String::new(),
        labels: vec![],
    };
    for op in [
        create("pd", "x"),
        create("PD", "two\nlines"),
        create("PD", " "),
        WriteOp::LinkIssues {
            link_type: "Causes".into(),
            inward: "PD-1".into(),
            outward: "nope".into(),
        },
    ] {
        assert!(
            matches!(j.write(&op).await, Err(TrackerError::Invalid(_))),
            "{op:?}"
        );
    }
    assert!(f.requests().is_empty());
}
