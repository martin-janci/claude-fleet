//! Asana over [`FakeTransport`] and the recorded fixtures in
//! `testdata/asana/` (see its README). No test here reaches Asana.

use super::*;
use crate::net::https::{FakeTransport, Method, Response, TransportError};
use crate::service::trackers::conformance::{fixture, ErrorCase, Expect, Harness};
use crate::service::trackers::list_all;

const WS: &str = "1200000000000001";
const ME: &str = "1200000000000100";
const P1: &str = "1200000000001001";
const P2: &str = "1200000000001002";
/// An invented PAT, assembled at run time (a literal Asana token shape in
/// the source trips push protection).
static PAT_VALUE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "2/{}/{}:{}",
        "1200000000000100",
        "1209999999999999",
        "0123456789abcdef".repeat(2)
    )
});

fn pat() -> &'static str {
    PAT_VALUE.as_str()
}

fn ok(name: &str) -> Result<Response, TransportError> {
    Ok(Response::json(200, &fixture("asana", name)))
}

fn cred() -> TrackerCredential {
    TrackerCredential {
        auth_kind: "bearer".into(),
        username: None,
        secret: crate::store::Secret::new(pat()),
    }
}

fn config() -> TrackerConfig {
    TrackerConfig {
        account_id: Some(ME.into()),
        workspace: Some(WS.into()),
        projects: vec![
            (P1.into(), "Company B · Platform".into()),
            (P2.into(), "Company B · Mobile".into()),
        ],
        section_map: [
            ("in progress", "in_progress"),
            ("in review", "in_progress"),
            ("done", "done"),
            ("shipped", "done"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect(),
        ..Default::default()
    }
}

fn asana_with(fake: &FakeTransport, settings: TrackerSettings) -> Asana {
    Asana::new(
        "https://app.asana.com",
        config(),
        settings,
        Some(cred()),
        Arc::new(fake.clone()),
    )
}

fn asana(fake: &FakeTransport) -> Asana {
    asana_with(fake, TrackerSettings::default())
}

fn view(q: &str) -> ViewDef {
    ViewDef {
        id: q.into(),
        label: q.into(),
        query: q.into(),
    }
}

struct AsanaHarness;

#[async_trait::async_trait]
impl Harness for AsanaHarness {
    fn name(&self) -> &'static str {
        "asana"
    }

    fn expect(&self) -> Expect {
        Expect {
            instance_id: Some(WS),
            me: ME,
            prefixes: vec![],
            view: view("mine"),
            list_ids: vec![
                "1207000000000001",
                "1207000000000002",
                "1207000000000003",
                "1207000000000004",
                "1207000000000005",
            ],
            fetch: [
                ItemRef::Id("1207000000000001".into()),
                ItemRef::Key("asana:1207000000000004".into()),
                ItemRef::Id("1207000000009999".into()),
            ],
            statuses: vec![
                ("1207000000000001", "in_progress", None),
                ("1207000000000002", "todo", None),
                ("1207000000000004", "done", Some("completed")),
                ("1207000000000005", "done", None),
            ],
            hierarchy: Some(("1207000000000003", "1207000000000001", Some(-1))),
            moved: None,
            recognize: vec![
                (
                    "see https://app.asana.com/0/1200000000001001/1207000000000001 and \
                     https://app.asana.com/1/1200000000000001/project/1200000000001002/task/1207000000000002/f",
                    None,
                    vec![
                        ItemRef::Key("asana:1207000000000001".into()),
                        ItemRef::Key("asana:1207000000000002".into()),
                    ],
                ),
                ("ABC-12 and #4, no keys in Asana", Some("o/r"), vec![]),
            ],
            bare_repo: "",
            secret: Some(pat()),
        }
    }

    fn provider(&self, fake: &FakeTransport) -> Box<dyn TrackerProvider> {
        Box::new(asana(fake))
    }

    fn script_probe(&self, f: &FakeTransport) {
        f.once(Method::Get, "/users/me", ok("users_me.json"))
            .once(
                Method::Get,
                "/tasks?assignee=me",
                ok("probe_my_projects.json"),
            )
            .once(
                Method::Get,
                &format!("/projects/{P1}/sections"),
                ok("sections_p1.json"),
            )
            .once(
                Method::Get,
                &format!("/projects/{P2}/sections"),
                ok("sections_p2.json"),
            )
            .once(Method::Get, "/tasks/search", ok("search_premium.json"));
    }

    fn script_list(&self, f: &FakeTransport) {
        f.once(Method::Get, "/tasks?assignee=me", ok("tasks_mine_p1.json"))
            .once(Method::Get, "/tasks?assignee=me", ok("tasks_mine_p2.json"));
    }

    async fn incremental(
        &self,
        p: &dyn TrackerProvider,
        f: &FakeTransport,
    ) -> Result<Vec<WorkItemSnapshot>, TrackerError> {
        f.once(Method::Get, "/events?resource=", ok("events_p1.json"))
            .once(Method::Post, "/batch", ok("batch_changed.json"));
        let ch = p
            .changes(&view(&format!("project:{P1}")), Some("tok-1"))
            .await?;
        assert!(!ch.expired);
        assert_eq!(
            ch.mark.as_deref(),
            Some("de4774f6915eae04714ca93bb2f5ee81:1")
        );
        let reqs = f.requests();
        assert!(
            reqs[0]
                .url
                .ends_with(&format!("/events?resource={P1}&sync=tok-1")),
            "{}",
            reqs[0].url
        );
        let batch = reqs[1].json_body().unwrap();
        assert_eq!(
            batch["data"]["actions"][0]["relative_path"], "/tasks/1207000000000002",
            "the story event is not a task: only the task is fetched"
        );
        Ok(ch.items)
    }

    fn script_fetch(&self, f: &FakeTransport) {
        f.once(Method::Post, "/batch", ok("batch_two.json"));
    }

    fn script_moved(&self, _f: &FakeTransport) {}

    fn script_error(&self, f: &FakeTransport, case: ErrorCase) {
        match case {
            ErrorCase::Unauthorized => {
                f.once(
                    Method::Get,
                    "/users/me",
                    Ok(Response::new(
                        401,
                        "{\"errors\":[{\"message\":\"Not Authorized\"}]}",
                    )),
                );
            }
            ErrorCase::ForbiddenView => {
                f.once(
                    Method::Get,
                    "/tasks?assignee=me",
                    Ok(Response::new(403, "{}")),
                );
            }
            ErrorCase::RateLimited => {
                f.once(
                    Method::Get,
                    "/tasks?assignee=me",
                    Ok(Response::new(429, "{}").with_header("Retry-After", "30")),
                );
            }
            ErrorCase::Offline => {}
            ErrorCase::Garbage => {
                f.once(
                    Method::Get,
                    "/tasks?assignee=me",
                    Ok(Response::new(200, "<html>maintenance</html>")),
                );
            }
        }
    }
}

crate::conformance_suite!(AsanaHarness);

#[tokio::test]
async fn the_probe_finds_the_workspace_projects_sections_and_premium() {
    let f = FakeTransport::new();
    AsanaHarness.script_probe(&f);
    let a = Asana::new(
        "https://app.asana.com",
        TrackerConfig::default(),
        TrackerSettings::default(),
        Some(cred()),
        Arc::new(f.clone()),
    );
    let c = a.probe().await.unwrap().config;
    assert_eq!(c.workspace.as_deref(), Some(WS));
    assert_eq!(
        c.projects,
        vec![
            (P1.to_string(), "Company B · Platform".to_string()),
            (P2.to_string(), "Company B · Mobile".to_string())
        ],
        "every project a task of mine sits in, once"
    );
    assert!(c.search);
    let inferred: Vec<(&str, &str)> = c
        .section_map
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        inferred,
        vec![
            ("done", "done"),
            ("in progress", "in_progress"),
            ("in review", "in_progress"),
            ("shipped", "done")
        ],
        "backlog and untitled sections stay unmapped (todo)"
    );
    let ids: Vec<String> = a
        .views(&c)
        .await
        .unwrap()
        .into_iter()
        .map(|v| v.id)
        .collect();
    assert_eq!(
        ids,
        vec![
            "mine".to_string(),
            format!("project:{P1}"),
            format!("project:{P2}"),
            "recent".to_string()
        ]
    );
    // opt_fields everywhere, never opt_expand; the PAT as a bearer.
    for r in f.requests() {
        assert!(r.url.starts_with(API), "{}", r.url);
        assert!(!r.url.contains("opt_expand"), "{}", r.url);
        assert_eq!(
            r.header_value("Authorization"),
            Some(format!("Bearer {}", pat()).as_str())
        );
    }
}

#[tokio::test]
async fn a_workspace_that_is_not_premium_has_no_search_view() {
    let f = FakeTransport::new();
    f.once(Method::Get, "/users/me", ok("users_me.json"))
        .once(
            Method::Get,
            "/tasks?assignee=me",
            ok("probe_my_projects.json"),
        )
        .always(
            Method::Get,
            "/sections",
            Ok(Response::json(200, &json!({"data": []}))),
        )
        .once(
            Method::Get,
            "/tasks/search",
            Ok(Response::json(
                402,
                &fixture("asana", "search_not_premium.json"),
            )),
        );
    let c = asana(&f).probe().await.unwrap().config;
    assert!(!c.search);
    assert!(!asana(&f)
        .views(&c)
        .await
        .unwrap()
        .iter()
        .any(|v| v.id == "recent"));
}

#[tokio::test]
async fn two_workspaces_and_none_named_asks_which() {
    let f = FakeTransport::new();
    f.once(Method::Get, "/users/me", ok("users_me_two_workspaces.json"));
    let a = Asana::new(
        "https://app.asana.com",
        TrackerConfig::default(),
        TrackerSettings::default(),
        Some(cred()),
        Arc::new(f.clone()),
    );
    match a.probe().await.unwrap_err() {
        TrackerError::Invalid(m) => {
            assert!(m.contains("https://app.asana.com/<workspace gid>"), "{m}");
            assert!(m.contains("Personal (1200000000000002)"), "{m}");
        }
        e => panic!("{e:?}"),
    }
    // Naming it settles it.
    f.clear_routes();
    AsanaHarness.script_probe(&f);
    f.once(Method::Get, "/users/me", ok("users_me_two_workspaces.json"));
    let named = Asana::new(
        &format!("https://app.asana.com/{WS}"),
        TrackerConfig::default(),
        TrackerSettings::default(),
        Some(cred()),
        Arc::new(f.clone()),
    );
    assert_eq!(
        named.probe().await.unwrap().instance_id.as_deref(),
        Some(WS)
    );
}

#[tokio::test]
async fn a_task_in_two_projects_keeps_both_and_its_first_section_decides() {
    let f = FakeTransport::new();
    AsanaHarness.script_list(&f);
    let items = list_all(&asana(&f), &view("mine"), None).await.unwrap();
    let t1 = &items[0];
    assert_eq!(t1.key.as_deref(), Some("asana:1207000000000001"));
    assert_eq!(t1.containers, vec![P1, P2]);
    assert_eq!(t1.status.name, "In progress");
    assert_eq!(t1.status.category, "in_progress");
    assert_eq!(t1.assignee_id.as_deref(), Some(ME));
    assert_eq!(items[4].kind.as_deref(), Some("Milestone"));
    assert_eq!(items[2].kind.as_deref(), Some("Subtask"));
    assert_eq!(
        items[2].parent_key.as_deref(),
        Some("asana:1207000000000001")
    );
    // The second page used the first page's offset.
    let reqs = f.requests();
    assert!(reqs[1]
        .url
        .ends_with("&offset=eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9"));
    assert!(reqs[0].url.contains(&format!("workspace={WS}")));
    assert!(reqs[0].url.contains("completed_since=now"));
}

#[tokio::test]
async fn a_confirmed_section_map_wins_and_stops_inference() {
    let f = FakeTransport::new();
    AsanaHarness.script_list(&f);
    let mut map = std::collections::BTreeMap::new();
    map.insert("backlog".to_string(), "in_progress".to_string());
    let a = asana_with(
        &f,
        TrackerSettings {
            section_map: map,
            section_map_confirmed: true,
            ..Default::default()
        },
    );
    let items = list_all(&a, &view("mine"), None).await.unwrap();
    assert_eq!(
        items[1].status.category, "in_progress",
        "backlog, as confirmed"
    );
    assert_eq!(
        items[0].status.category, "todo",
        "the inferred `in progress` no longer applies once a person confirmed the map"
    );
    assert_eq!(
        items[3].status.category, "done",
        "completed stays authoritative"
    );
}

#[tokio::test]
async fn an_expired_sync_token_hands_back_a_fresh_one_and_asks_for_a_whole_listing() {
    let f = FakeTransport::new();
    f.once(
        Method::Get,
        "/events?resource=",
        Ok(Response::json(
            412,
            &fixture("asana", "events_expired.json"),
        )),
    );
    let ch = asana(&f)
        .changes(&view(&format!("project:{P1}")), Some("old-token"))
        .await
        .unwrap();
    assert!(ch.expired);
    assert!(ch.items.is_empty());
    assert_eq!(
        ch.mark.as_deref(),
        Some("a1b2c3d4e5f60718293a4b5c6d7e8f90:0")
    );
    // `mine` has no event stream: always a whole listing, no request.
    let f = FakeTransport::new();
    let ch = asana(&f).changes(&view("mine"), Some("x")).await.unwrap();
    assert!(ch.expired && ch.mark.is_none());
    assert!(f.requests().is_empty());
}

/// Through the sync: a project view's first pass lists whole and takes a
/// token; the next reads events; an expired token lists whole again.
#[tokio::test]
async fn the_sync_follows_a_project_by_token_and_recovers_from_expiry() {
    use crate::service::trackers::sync::TrackerSync;
    use crate::service::trackers::TrackerNet;
    use crate::store::Store;
    use std::sync::Mutex;
    let st = Mutex::new(Store::open_in_memory().unwrap());
    let id = {
        let s = st.lock().unwrap();
        let t = s
            .add_tracker("asana", "Company B", "https://app.asana.com")
            .unwrap();
        s.set_tracker_credential(t.id, "bearer", None, Some(pat()), None)
            .unwrap();
        s.set_tracker_probe(t.id, Some(WS), &config()).unwrap();
        s.sync_tracker_views(
            t.id,
            &[(
                format!("project:{P1}"),
                "Platform".into(),
                format!("project:{P1}"),
            )],
        )
        .unwrap();
        s.set_tracker_state(t.id, "ok", None).unwrap();
        t.id
    };
    let f = FakeTransport::new();
    let listing = || {
        Ok(Response::json(
            200,
            &json!({"data": fixture("asana", "tasks_mine_p1.json")["data"], "next_page": null}),
        ))
    };
    // Pass 1: whole, then a first token (412 with a token).
    f.once(Method::Get, &format!("/projects/{P1}/tasks"), listing())
        .once(
            Method::Get,
            "/events?resource=",
            Ok(Response::json(412, &json!({"sync": "tok-A"}))),
        );
    let row = || st.lock().unwrap().require_tracker(id).unwrap();
    let sync = TrackerSync::new(TrackerNet::fake(Arc::new(f.clone())));
    let p = sync.sync_tracker(&row(), &st).await;
    assert_eq!((p.seen, p.error.as_deref()), (3, None), "{p:?}");
    let mark = |st: &Mutex<Store>| {
        st.lock().unwrap().list_tracker_views(id).unwrap()[0]
            .sync_mark
            .clone()
    };
    assert_eq!(mark(&st).as_deref(), Some("tok-A"));
    // The stored views remember which items a project view returned.
    let members = st
        .lock()
        .unwrap()
        .tracker_item_for_key("asana:1207000000000001")
        .unwrap()
        .unwrap();
    let meta = st.lock().unwrap().work_item_meta(members.id).unwrap();
    assert_eq!(meta.views, vec![format!("project:{P1}")]);

    // Pass 2: the token's changes only — one task, fetched by gid.
    f.clear_routes();
    f.once(Method::Get, "/events?resource=", ok("events_p1.json"))
        .once(Method::Post, "/batch", ok("batch_changed.json"));
    let before = f.requests().len();
    let p = sync.sync_tracker(&row(), &st).await;
    assert_eq!(
        (p.seen, p.changed, p.error.as_deref()),
        (1, 1, None),
        "{p:?}"
    );
    assert!(f.requests()[before].url.ends_with("&sync=tok-A"));
    assert_eq!(
        mark(&st).as_deref(),
        Some("de4774f6915eae04714ca93bb2f5ee81:1")
    );
    let t2 = st
        .lock()
        .unwrap()
        .tracker_item_for_key("asana:1207000000000002")
        .unwrap()
        .unwrap();
    assert_eq!(
        t2.status_category, "in_progress",
        "moved to the In progress section"
    );

    // Pass 3: the token expired — one whole listing and the fresh token.
    f.clear_routes();
    f.once(
        Method::Get,
        "/events?resource=",
        Ok(Response::json(
            412,
            &fixture("asana", "events_expired.json"),
        )),
    )
    .once(Method::Get, &format!("/projects/{P1}/tasks"), listing());
    let p = sync.sync_tracker(&row(), &st).await;
    assert_eq!((p.seen, p.error.as_deref()), (3, None), "{p:?}");
    assert_eq!(
        mark(&st).as_deref(),
        Some("a1b2c3d4e5f60718293a4b5c6d7e8f90:0")
    );
}

#[test]
fn sections_are_inferred_from_their_words() {
    for (name, want) in [
        ("In progress", Some("in_progress")),
        ("Doing", Some("in_progress")),
        ("Code review", Some("in_progress")),
        ("Done ✅", Some("done")),
        ("Shipped", Some("done")),
        ("Backlog", None),
        ("To do", None),
    ] {
        assert_eq!(infer_section(name), want, "{name}");
    }
}

/// Connect by pasting a task URL; the PAT is a bearer and never comes back.
#[test]
fn connect_by_pasting_a_task_url() {
    use crate::service::trackers::admin::{admin_sync, WorkAdminArgs};
    use crate::store::Store;
    use std::sync::Mutex;
    let st = Mutex::new(Store::open_in_memory().unwrap());
    let v = admin_sync(
        &WorkAdminArgs {
            action: "add".into(),
            site_url: Some(format!(
                "https://app.asana.com/1/{WS}/project/{P1}/task/1207000000000001"
            )),
            ..Default::default()
        },
        &st,
    )
    .unwrap();
    assert_eq!(v["provider"], "asana");
    assert_eq!(v["site_url"], format!("https://app.asana.com/{WS}"));
    let id = v["id"].as_i64().unwrap();
    let v = admin_sync(
        &WorkAdminArgs {
            action: "set_credential".into(),
            tracker_id: Some(id),
            auth_kind: Some("bearer".into()),
            secret: Some(pat().into()),
            ..Default::default()
        },
        &st,
    )
    .unwrap();
    assert!(!v.to_string().contains(pat()));
    assert_eq!(v["auth_kind"], "bearer");
    let c = st
        .lock()
        .unwrap()
        .resolve_tracker_credential(id)
        .unwrap()
        .unwrap();
    assert_eq!(c.authorization().expose(), format!("Bearer {}", pat()));
    // Only Asana's own host; not a lookalike.
    for bad in [
        "https://app.asana.com.evil.com/0/1/2",
        "https://evil.example.com/0/1/2",
        "http://app.asana.com",
        "https://app.asana.com:8443",
    ] {
        let e = crate::store::normalize_provider_site("asana", bad).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_INVALID, "{bad}");
    }
    let row = st.lock().unwrap().require_tracker(id).unwrap();
    let policy = crate::service::trackers::host_policy(&row);
    assert!(policy("app.asana.com"));
    assert!(!policy("app.asana.com.evil.com") && !policy("169.254.169.254"));
}

/// Acceptance 6 under the interim fence (M5 replaces it with orgs): a host
/// token sees none of an Asana tracker's tasks unless a session on its own
/// host works on one.
#[tokio::test]
async fn a_host_token_sees_no_asana_task_its_host_does_not_work_on() {
    use crate::service::trackers::tickets::{lookup, tickets, trackers, Scope};
    use crate::service::trackers::TrackerNet;
    use crate::store::{Store, WorkTarget};
    use std::sync::Mutex;
    let st = Mutex::new(Store::open_in_memory().unwrap());
    let f = FakeTransport::new();
    {
        let s = st.lock().unwrap();
        let t = s
            .add_tracker("asana", "Company B", "https://app.asana.com")
            .unwrap();
        s.set_tracker_probe(t.id, Some(WS), &config()).unwrap();
        let a = asana(&f);
        for task in fixture("asana", "tasks_mine_p1.json")["data"]
            .as_array()
            .unwrap()
        {
            let snap = a.snapshot(task).unwrap();
            s.upsert_tracker_item(t.id, &crate::service::trackers::sync::to_write(snap))
                .unwrap();
        }
        s.upsert_host("company-a").unwrap();
        s.upsert_host("company-b").unwrap();
        let sid = s
            .upsert_session("b-dev", "company-b", None, None, 1, 1, "running", None)
            .unwrap();
        s.link_session_work(sid, WorkTarget::Key("asana:1207000000000001"), "manual")
            .unwrap();
    }
    let a_host = Scope::Host("company-a");
    assert!(tickets(&st, None, None, None, None, a_host)
        .unwrap()
        .is_empty());
    assert!(trackers(&st, a_host).unwrap().is_empty());
    let net = TrackerNet::fake(Arc::new(f.clone()));
    for r in [
        "https://app.asana.com/0/1200000000001001/1207000000000001",
        "asana:1207000000000002",
    ] {
        let e = lookup(&st, r, a_host, &net).await.unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_FORBIDDEN, "{r}");
    }
    assert!(
        f.requests().is_empty(),
        "a host token never makes the hub fetch"
    );
    // Company B's host sees the one its session works on, and only that.
    let b = tickets(&st, None, None, None, None, Scope::Host("company-b")).unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].item.key.as_deref(), Some("asana:1207000000000001"));
}
