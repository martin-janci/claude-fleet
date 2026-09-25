//! GitHub Issues over [`FakeTransport`] and the recorded fixtures in
//! `testdata/github/` (see its README), and over the real `gh` transport
//! against a scripted [`FakeSsh`]. No test here reaches GitHub.

use super::*;
use crate::net::https::{FakeTransport, Method, Response, TransportError};
use crate::service::trackers::conformance::{fixture, ErrorCase, Expect, Harness};
use crate::service::trackers::{list_all, TrackerNet};
use crate::ssh_fake::{FakeSsh, Match, Reply};

const SITE: &str = "https://github.com/acme";
const ME: &str = "dana-dev";

fn ok(name: &str) -> Result<Response, TransportError> {
    Ok(Response::json(200, &fixture("github", name)))
}

fn config() -> TrackerConfig {
    TrackerConfig {
        account_id: Some(ME.into()),
        ..Default::default()
    }
}

fn github(fake: &FakeTransport) -> GitHub {
    GitHub::new(
        SITE,
        config(),
        TrackerSettings::default(),
        Arc::new(fake.clone()),
    )
}

fn mine() -> ViewDef {
    ViewDef {
        id: "mine".into(),
        label: "My issues".into(),
        query: "is:issue assignee:@me is:open user:acme".into(),
    }
}

struct GitHubHarness;

#[async_trait::async_trait]
impl Harness for GitHubHarness {
    fn name(&self) -> &'static str {
        "github"
    }

    fn expect(&self) -> Expect {
        Expect {
            instance_id: Some("github.com"),
            me: ME,
            prefixes: vec![],
            view: mine(),
            list_ids: vec![
                "I_kwDOAcme0001",
                "I_kwDOAcme0002",
                "I_kwDOAcme0006",
                "I_kwDOAcme0003",
                "I_kwDOAcme0004",
                "I_kwDOAcme0005",
            ],
            fetch: [
                ItemRef::Id("I_kwDOAcme0001".into()),
                ItemRef::Id("I_kwDOAcme0003".into()),
                ItemRef::Id("I_kwDOAcme9999".into()),
            ],
            statuses: vec![
                ("I_kwDOAcme0001", "in_progress", None),
                ("I_kwDOAcme0002", "in_progress", None),
                ("I_kwDOAcme0006", "todo", None),
                ("I_kwDOAcme0003", "done", Some("completed")),
                ("I_kwDOAcme0004", "done", Some("not_planned")),
                ("I_kwDOAcme0005", "done", Some("duplicate")),
            ],
            hierarchy: Some(("I_kwDOAcme0006", "I_kwDOAcme0001", None)),
            moved: Some((
                ItemRef::RepoNumber {
                    repo: "acme/legacy".into(),
                    n: 5,
                },
                "I_kwDOAcme0007",
                "acme/legacy#5",
            )),
            recognize: vec![
                (
                    "see https://github.com/acme/api/issues/42 and https://github.com/other/x/issues/1",
                    None,
                    vec![ItemRef::RepoNumber {
                        repo: "acme/api".into(),
                        n: 42,
                    }],
                ),
                (
                    "fixes #7",
                    Some("Acme/Web"),
                    vec![ItemRef::RepoNumber {
                        repo: "acme/web".into(),
                        n: 7,
                    }],
                ),
                ("fixes #7", Some("other/x"), vec![]),
                ("ABC-12 is a Jira key", Some("acme/web"), vec![]),
            ],
            bare_repo: "acme/api",
            secret: None,
        }
    }

    fn provider(&self, fake: &FakeTransport) -> Box<dyn TrackerProvider> {
        Box::new(github(fake))
    }

    fn script_probe(&self, f: &FakeTransport) {
        f.once(Method::Post, "/graphql", ok("viewer.json")).once(
            Method::Post,
            "/graphql",
            ok("owner.json"),
        );
    }

    fn script_list(&self, f: &FakeTransport) {
        f.once(Method::Post, "/graphql", ok("search_mine_p1.json"))
            .once(Method::Post, "/graphql", ok("search_mine_p2.json"));
    }

    async fn incremental(
        &self,
        p: &dyn TrackerProvider,
        f: &FakeTransport,
    ) -> Result<Vec<WorkItemSnapshot>, TrackerError> {
        f.once(Method::Post, "/graphql", ok("search_mine_p2.json"));
        let page = p.list(&mine(), Some(1_789_900_000), None).await?;
        let q = f.requests()[0].json_body().unwrap()["variables"]["q"].clone();
        assert_eq!(
            q,
            "is:issue assignee:@me is:open user:acme updated:>=2026-09-20T10:26:40Z sort:updated-desc"
        );
        Ok(page.items)
    }

    fn script_fetch(&self, f: &FakeTransport) {
        f.once(Method::Post, "/graphql", ok("nodes_two.json"));
    }

    fn script_moved(&self, f: &FakeTransport) {
        f.once(Method::Post, "/graphql", ok("repo_moved.json"));
    }

    fn script_error(&self, f: &FakeTransport, case: ErrorCase) {
        match case {
            ErrorCase::Unauthorized => {
                f.once(
                    Method::Post,
                    "/graphql",
                    Ok(Response::new(401, "{\"message\":\"Bad credentials\"}")),
                );
            }
            ErrorCase::ForbiddenView => {
                f.once(Method::Post, "/graphql", ok("search_forbidden.json"));
            }
            ErrorCase::RateLimited => {
                f.once(
                    Method::Post,
                    "/graphql",
                    Ok(Response::new(403, "{\"message\":\"secondary rate limit\"}")
                        .with_header("Retry-After", "30")),
                );
            }
            ErrorCase::Offline => {}
            ErrorCase::Garbage => {
                f.once(
                    Method::Post,
                    "/graphql",
                    Ok(Response::new(200, "<html>unicorn</html>")),
                );
            }
        }
    }
}

crate::conformance_suite!(GitHubHarness);

#[tokio::test]
async fn a_listing_normalises_keys_assignees_and_the_parent() {
    let f = FakeTransport::new();
    GitHubHarness.script_list(&f);
    let items = list_all(&github(&f), &mine(), None).await.unwrap();
    let a1 = &items[0];
    assert_eq!(a1.key.as_deref(), Some("acme/api#42"), "lower case");
    assert_eq!(a1.containers, vec!["acme/api"]);
    assert_eq!(a1.assignees, vec!["pat-ops", "dana-dev"]);
    assert_eq!(
        a1.assignee_id.as_deref(),
        Some(ME),
        "me first, for the mine view"
    );
    assert_eq!(a1.kind.as_deref(), Some("Bug"));
    assert_eq!(a1.status.name, "Open · linked pull request");
    assert!(a1
        .description
        .as_deref()
        .unwrap()
        .starts_with("Login fails"));
    let sub = &items[2];
    assert_eq!(sub.parent_key.as_deref(), Some("acme/api#42"));
    assert_eq!(sub.kind.as_deref(), Some("Issue"));
    // The second page was asked for with the first page's cursor.
    let reqs = f.requests();
    assert_eq!(
        reqs[1].json_body().unwrap()["variables"]["after"],
        "Y3Vyc29yOjM="
    );
    // No credential in fleet: no Authorization header, ever.
    assert!(reqs
        .iter()
        .all(|r| r.header_value("Authorization").is_none()));
}

#[tokio::test]
async fn views_stay_inside_the_scope() {
    let f = FakeTransport::new();
    let v = github(&f).views(&config()).await.unwrap();
    assert_eq!(v[0].query, "is:issue assignee:@me is:open user:acme");
    assert_eq!(v[1].query, "is:issue assignee:@me user:acme");
    let narrowed = GitHub::new(
        SITE,
        config(),
        TrackerSettings {
            repos: vec!["acme/api".into(), "acme/web".into()],
            ..Default::default()
        },
        Arc::new(f.clone()),
    );
    let v = narrowed.views(&config()).await.unwrap();
    assert_eq!(
        v[0].query,
        "is:issue assignee:@me is:open repo:acme/api repo:acme/web"
    );
    assert!(narrowed.in_scope("acme/api") && !narrowed.in_scope("acme/other"));
    // The whole of github.com: no qualifier at all.
    let all = GitHub::new(
        "https://github.com",
        config(),
        TrackerSettings::default(),
        Arc::new(f),
    );
    assert!(all.in_scope("anyone/anything"));
    assert_eq!(
        all.views(&config()).await.unwrap()[0].query,
        "is:issue assignee:@me is:open"
    );
}

#[tokio::test]
async fn keys_and_urls_are_fetched_by_repository_and_number_with_variables() {
    let f = FakeTransport::new();
    f.once(Method::Post, "/graphql", ok("repo_moved.json"));
    let got = github(&f)
        .fetch(&[ItemRef::Key(
            "https://github.com/acme/legacy/issues/5".into(),
        )])
        .await
        .unwrap();
    assert!(matches!(&got[0], Fetched::Found(s) if s.aliases == vec!["acme/legacy#5"]));
    let body = f.requests()[0].json_body().unwrap();
    assert_eq!(
        body["variables"],
        json!({"o0": "acme", "r0": "legacy", "n0": 5}),
        "values travel as GraphQL variables, never inside the query text"
    );
    assert!(!body["query"].as_str().unwrap().contains("legacy"));
}

#[tokio::test]
async fn a_rate_limited_graphql_answer_backs_off() {
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/graphql",
        Ok(Response::json(
            200,
            &json!({"data": null, "errors": [{"type": "RATE_LIMITED", "message": "API rate limit exceeded"}]}),
        )),
    );
    assert_eq!(
        github(&f).list(&mine(), None, None).await.unwrap_err(),
        TrackerError::RateLimited {
            retry_after_secs: None
        }
    );
    // The primary limit is a 200 with RATE_LIMITED and the reset in the
    // headers: waited out, like the 403.
    let f = FakeTransport::new();
    let reset = crate::service::catalog::now_secs() + 120;
    f.once(
        Method::Post,
        "/graphql",
        Ok(Response::json(
            200,
            &json!({"data": null, "errors": [{"type": "RATE_LIMITED", "message": "API rate limit exceeded"}]}),
        )
        .with_header("X-RateLimit-Remaining", "0")
        .with_header("X-RateLimit-Reset", reset.to_string())),
    );
    match github(&f).list(&mine(), None, None).await.unwrap_err() {
        TrackerError::RateLimited {
            retry_after_secs: Some(s),
        } => assert!((110..=121).contains(&s), "{s}"),
        e => panic!("{e:?}"),
    }
    // A spent quota on a 403 waits until the reset.
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/graphql",
        Ok(Response::new(403, "{}")
            .with_header("X-RateLimit-Remaining", "0")
            .with_header("X-RateLimit-Reset", reset.to_string())),
    );
    match github(&f).list(&mine(), None, None).await.unwrap_err() {
        TrackerError::RateLimited {
            retry_after_secs: Some(s),
        } => assert!((110..=121).contains(&s), "{s}"),
        e => panic!("{e:?}"),
    }
}

/// End to end through `via_cli`: the provider, `TrackerNet`'s transport
/// selection and `gh` on the host, answered by a scripted SSH.
#[tokio::test]
async fn a_via_cli_tracker_runs_gh_on_its_host_with_no_token() {
    let ssh = FakeSsh::new();
    ssh.on_host(
        "devbox",
        Match::contains("gh api"),
        Reply::ok(&format!(
            "HTTP/2.0 200 OK\nContent-Type: application/json\n\n{}",
            fixture("github", "viewer.json")
        )),
    );
    let row = crate::store::TrackerRow {
        id: 1,
        provider: "github".into(),
        name: "acme".into(),
        instance_id: None,
        site_url: "https://github.com".into(),
        transport: "via_cli:devbox".into(),
        config: TrackerConfig::default(),
        state: "unconfigured".into(),
        last_sync_at: None,
        last_error: None,
        created_at: 1,
        has_credential: false,
        credential_hint: None,
        auth_kind: None,
        username: None,
        org_id: None,
        settings: TrackerSettings::default(),
    };
    assert!(!crate::service::trackers::needs_credential(&row));
    let net = TrackerNet::with_ssh(Arc::new(ssh.clone()));
    let p = crate::service::trackers::provider_for(&row, None, &net).unwrap();
    let info = p.probe().await.unwrap();
    assert_eq!(info.config.account_id.as_deref(), Some(ME));
    let calls = ssh.calls_for("devbox");
    assert_eq!(calls.len(), 1);
    let stdin = calls[0].stdin_str().unwrap();
    assert!(
        stdin.contains("viewer"),
        "the GraphQL body is on stdin: {stdin}"
    );
    assert!(!calls[0].command().contains("viewer"));
    assert!(!calls[0].command().to_lowercase().contains("authorization"));
    // A host fleet cannot reach through a process without SSH.
    let e = crate::service::trackers::provider_for(&row, None, &TrackerNet::real(None))
        .err()
        .unwrap();
    assert_eq!(e.state(), Some("unreachable"));
}

// --- the flow: admin, test, sync, bind, lookup, start (M6.1) ----------------

mod flow {
    use super::*;
    use crate::ipc_error::codes;
    use crate::service::orgs::OrgScope;
    use crate::service::trackers::admin::{admin_sync, test_tracker, WorkAdminArgs};
    use crate::service::trackers::sync::TrackerSync;
    use crate::service::trackers::tickets::{lookup, plan_start, StartArgs};
    use crate::store::{Store, WorkTarget};
    use std::sync::Mutex;

    fn args(action: &str) -> WorkAdminArgs {
        WorkAdminArgs {
            action: action.into(),
            ..Default::default()
        }
    }

    /// A store with a GitHub tracker added by pasting an issue URL.
    fn added() -> (Mutex<Store>, i64) {
        let st = Mutex::new(Store::open_in_memory().unwrap());
        let e = admin_sync(
            &WorkAdminArgs {
                site_url: Some("https://github.com/Acme/api/issues/42".into()),
                ..args("add")
            },
            &st,
        )
        .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "GitHub needs a host with gh");
        assert!(e.message.contains("via_cli"), "{}", e.message);
        let v = admin_sync(
            &WorkAdminArgs {
                site_url: Some("https://github.com/Acme/api/issues/42".into()),
                transport: Some("via_cli:devbox".into()),
                ..args("add")
            },
            &st,
        )
        .unwrap();
        assert_eq!(v["provider"], "github", "inferred from the URL");
        assert_eq!(
            v["site_url"], "https://github.com/acme",
            "narrowed to the owner"
        );
        assert_eq!(v["transport"], "via_cli:devbox");
        assert_eq!(v["name"], "acme (GitHub)");
        (st, v["id"].as_i64().unwrap())
    }

    #[test]
    fn a_github_tracker_never_stores_a_token_and_its_settings_are_validated() {
        let (st, id) = added();
        let e = admin_sync(
            &WorkAdminArgs {
                tracker_id: Some(id),
                auth_kind: Some("bearer".into()),
                secret: Some(format!(
                    "{}_{}",
                    "ghp", "abcdefghijklmnopqrstuvwxyz0123456789"
                )),
                ..args("set_credential")
            },
            &st,
        )
        .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert!(!e.message.contains("ghp_"), "{}", e.message);
        let v = admin_sync(
            &WorkAdminArgs {
                tracker_id: Some(id),
                settings: Some(json!({"repos": ["Acme/API", "acme/web", "acme/api"]})),
                ..args("update")
            },
            &st,
        )
        .unwrap();
        assert_eq!(v["settings"]["repos"], json!(["acme/api", "acme/web"]));
        for bad in [
            json!({"repos": ["--exec=x/y"]}),
            json!({"repos": ["not a repo"]}),
            json!({"section_map": {"Doing": "in_progress"}}),
            json!({"extra_ca": "-----BEGIN CERTIFICATE-----"}),
        ] {
            let e = admin_sync(
                &WorkAdminArgs {
                    tracker_id: Some(id),
                    settings: Some(bad.clone()),
                    ..args("update")
                },
                &st,
            )
            .unwrap_err();
            assert_eq!(e.code, codes::E_INVALID, "{bad}");
        }
        for bad in [
            "via_cli:-oProxyCommand=x",
            "via_cli:a b",
            "ssh://x",
            "direct",
            "via_host:devbox",
        ] {
            let e = admin_sync(
                &WorkAdminArgs {
                    tracker_id: Some(id),
                    transport: Some(bad.into()),
                    ..args("update")
                },
                &st,
            )
            .unwrap_err();
            assert_eq!(e.code, codes::E_INVALID, "{bad}");
        }
    }

    #[tokio::test]
    async fn test_sync_bind_lookup_and_start_through_gh() {
        let (st, id) = added();
        // A closing ref seen before the tracker existed: a bare reference.
        let sid = {
            let s = st.lock().unwrap();
            s.upsert_host("h").unwrap();
            let sid = s
                .upsert_session("dev", "h", None, None, 1, 1, "running", None)
                .unwrap();
            s.link_session_work(sid, WorkTarget::Key("Acme/API#42"), "manual")
                .unwrap();
            s.upsert_project("Acme", "api", "/p/api").unwrap();
            sid
        };
        let f = FakeTransport::new();
        GitHubHarness.script_probe(&f);
        let net = TrackerNet::fake(Arc::new(f.clone()));
        let r = test_tracker(id, &st, &net).await.unwrap();
        assert!(r.ok, "{:?}", r.error);
        assert_eq!(r.views, vec!["My issues", "Recent"]);
        assert_eq!(r.tracker.config.account_id.as_deref(), Some(ME));
        // One pass: both views, then the bare ref by repository and number.
        f.clear_routes();
        GitHubHarness.script_list(&f);
        f.once(
            Method::Post,
            "/graphql",
            Ok(Response::json(200, &json!({"data": {"search": {"pageInfo": {"hasNextPage": false, "endCursor": null}, "nodes": []}}}))),
        );
        f.once(
            Method::Post,
            "/graphql",
            Ok(Response::json(
                200,
                &json!({"data": {"i0": {"issue": fixture("github", "search_mine_p1.json")["data"]["search"]["nodes"][0]}}}),
            )),
        );
        let row = st.lock().unwrap().require_tracker(id).unwrap();
        let sync = TrackerSync::new(net.clone());
        let pass = sync.sync_tracker(&row, &st).await;
        assert_eq!(pass.error, None, "{pass:?}");
        assert_eq!(pass.bound_sessions, 1, "{pass:?}");
        let links = st.lock().unwrap().session_work_links(sid).unwrap();
        let item = st
            .lock()
            .unwrap()
            .get_work_item(links[0].item_id.expect("bound"))
            .unwrap()
            .unwrap();
        assert_eq!(item.key.as_deref(), Some("acme/api#42"));
        assert_eq!(item.status_category, "in_progress");
        assert_eq!(
            links[0].ref_key.as_deref(),
            Some("acme/api#42"),
            "kept for history"
        );
        // Lookup by the issue URL answers from the cache.
        let t = lookup(
            &st,
            "https://github.com/acme/api/issues/42",
            &OrgScope::All,
            &net,
        )
        .await
        .unwrap();
        assert_eq!(t.item.id, item.id);
        assert_eq!(t.live_session_ids, vec![sid]);
        // A URL no GitHub tracker covers says so.
        let e = lookup(
            &st,
            "https://github.com/other/x/issues/1",
            &OrgScope::All,
            &net,
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, codes::E_NOTFOUND);
        assert!(e.message.contains("github"), "{}", e.message);
        // Starting sub-issue #44 lands in the repository's own project.
        let plan = plan_start(
            &st,
            &StartArgs {
                reference: Some("acme/api#44".into()),
                host_alias: Some("h".into()),
                ..Default::default()
            },
            &OrgScope::All,
            &net,
        )
        .await
        .unwrap();
        assert_eq!(plan.key, "acme/api#44");
        assert_eq!(plan.branch, "44-add-a-regression-test-for-42");
        assert_eq!(plan.name, "acme/api#44 Add a regression test for #42");
        let pid = st.lock().unwrap().list_projects().unwrap()[0].id;
        assert_eq!(plan.project_id, pid);
        // No request ever carried a token.
        assert!(f
            .requests()
            .iter()
            .all(|r| r.header_value("Authorization").is_none()));
    }
}
