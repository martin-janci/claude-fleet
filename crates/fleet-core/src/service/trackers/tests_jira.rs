//! Jira Cloud over [`FakeTransport`] and the recorded fixtures in
//! `testdata/jira/` (see its README). No test here reaches a real site.

use super::*;
use crate::net::https::{FakeTransport, Method, Response};
use crate::service::trackers::list_all;

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
        "see https://acme.atlassian.net/browse/ABC-101 and https://other.atlassian.net/browse/ZED-1 and team-8",
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

#[test]
fn adf_excerpts_are_capped() {
    let long = json!({"type":"doc","content":[{"type":"paragraph","content":[
        {"type":"text","text": "x".repeat(DESCRIPTION_MAX_CHARS * 2)}]}]});
    assert_eq!(
        adf_excerpt(&long).unwrap().chars().count(),
        DESCRIPTION_MAX_CHARS
    );
    assert_eq!(adf_excerpt(&Value::Null), None);
    assert_eq!(adf_excerpt(&json!({"type":"doc","content":[]})), None);
}
