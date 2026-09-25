//! Jira Data Center over [`FakeTransport`] and the recorded fixtures in
//! `testdata/jira_dc/` (see its README); the SSRF fence and the extra CA over
//! a real local TLS server. No test here reaches a real site.

use super::*;
use crate::net::https::{FakeTransport, Method, Response, TransportError};
use crate::service::trackers::conformance::{fixture, ErrorCase, Expect, Harness};
use crate::service::trackers::list_all;

const SITE: &str = "https://jira.corp.example/jira";

/// An invented PAT, assembled at run time.
static PAT_VALUE: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| format!("NjM{}", "dcTestTokenNotReal0123".repeat(2)));

fn pat() -> &'static str {
    PAT_VALUE.as_str()
}

fn ok(name: &str) -> Result<Response, TransportError> {
    Ok(Response::json(200, &fixture("jira_dc", name)))
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
        account_id: Some("ddev".into()),
        key_prefixes: vec!["OPS".into(), "PLAT".into()],
        sprint_projects: vec!["PLAT".into()],
        sprint_field: Some("customfield_10100".into()),
        epic_field: Some("customfield_10101".into()),
        ..Default::default()
    }
}

fn dc(fake: &FakeTransport) -> JiraDc {
    JiraDc::new(SITE, config(), Some(cred()), Arc::new(fake.clone()))
}

fn mine() -> ViewDef {
    ViewDef {
        id: "mine".into(),
        label: "My work".into(),
        query: VIEW_MINE.into(),
    }
}

struct DcHarness;

#[async_trait::async_trait]
impl Harness for DcHarness {
    fn name(&self) -> &'static str {
        "jira_dc"
    }

    fn expect(&self) -> Expect {
        Expect {
            instance_id: Some("jira.corp.example"),
            me: "ddev",
            prefixes: vec!["OPS", "PLAT"],
            view: mine(),
            list_ids: vec!["40001", "40002", "40003", "40004", "40005", "40006"],
            fetch: [
                ItemRef::Id("40002".into()),
                ItemRef::Key("ops-4".into()),
                ItemRef::Key("PLAT-999".into()),
            ],
            statuses: vec![
                ("40002", "in_progress", None),
                ("40003", "todo", None),
                ("40004", "done", Some("completed")),
                ("40005", "done", Some("not_planned")),
            ],
            hierarchy: Some(("40003", "40002", Some(-1))),
            moved: Some((ItemRef::Key("OLD-7".into()), "40007", "OLD-7")),
            recognize: vec![
                (
                    "see https://jira.corp.example/jira/browse/PLAT-2 and ops-4, not \
                     https://jira.other.example/browse/PLAT-9 or ZZZ-1",
                    None,
                    vec![ItemRef::Key("PLAT-2".into()), ItemRef::Key("OPS-4".into())],
                ),
                ("#12 and o/r#3", Some("o/r"), vec![]),
            ],
            bare_repo: "",
            secret: Some(pat()),
        }
    }

    fn provider(&self, fake: &FakeTransport) -> Box<dyn TrackerProvider> {
        Box::new(dc(fake))
    }

    fn script_probe(&self, f: &FakeTransport) {
        f.once(Method::Get, "/rest/api/2/myself", ok("myself.json"))
            .once(Method::Get, "/rest/api/2/project", ok("projects.json"))
            .once(Method::Get, "/rest/api/2/field", ok("fields.json"))
            .once(
                Method::Post,
                "/rest/api/2/search",
                ok("sprint_projects.json"),
            )
            .once(
                Method::Get,
                "/filter/favourite",
                Ok(Response::json(
                    200,
                    &json!([{"id": "10300", "name": "Ops board"}]),
                )),
            );
    }

    fn script_list(&self, f: &FakeTransport) {
        f.once(
            Method::Post,
            "/rest/api/2/search",
            ok("search_mine_p1.json"),
        )
        .once(
            Method::Post,
            "/rest/api/2/search",
            ok("search_mine_p2.json"),
        );
    }

    async fn incremental(
        &self,
        p: &dyn TrackerProvider,
        f: &FakeTransport,
    ) -> Result<Vec<WorkItemSnapshot>, TrackerError> {
        f.once(
            Method::Post,
            "/rest/api/2/search",
            ok("search_mine_p2.json"),
        );
        let since = crate::service::catalog::now_secs() - 10 * 60 - 5;
        let page = p.list(&mine(), Some(since), None).await?;
        let body = f.requests()[0].json_body().unwrap();
        assert_eq!(
            body["jql"],
            format!("({VIEW_MINE}) AND updated >= -11m ORDER BY updated DESC")
        );
        assert_eq!(body["startAt"], 0);
        Ok(page.items)
    }

    fn script_fetch(&self, f: &FakeTransport) {
        f.once(Method::Post, "/rest/api/2/search", ok("fetch_two.json"));
    }

    fn script_moved(&self, f: &FakeTransport) {
        f.once(Method::Post, "/rest/api/2/search", ok("fetch_moved.json"));
    }

    fn script_error(&self, f: &FakeTransport, case: ErrorCase) {
        match case {
            ErrorCase::Unauthorized => {
                f.once(Method::Get, "/myself", Ok(Response::new(401, "")));
            }
            ErrorCase::ForbiddenView => {
                f.once(
                    Method::Post,
                    "/rest/api/2/search",
                    Ok(Response::new(403, "")),
                );
            }
            ErrorCase::RateLimited => {
                f.once(
                    Method::Post,
                    "/rest/api/2/search",
                    Ok(Response::new(429, "").with_header("Retry-After", "30")),
                );
            }
            ErrorCase::Offline => {}
            ErrorCase::Garbage => {
                f.once(
                    Method::Post,
                    "/rest/api/2/search",
                    Ok(Response::new(200, "<html>Jira is starting</html>")),
                );
            }
        }
    }
}

crate::conformance_suite!(DcHarness);

#[tokio::test]
async fn the_probe_finds_the_epic_link_and_sprint_fields_by_schema() {
    let f = FakeTransport::new();
    DcHarness.script_probe(&f);
    let d = JiraDc::new(
        SITE,
        TrackerConfig::default(),
        Some(cred()),
        Arc::new(f.clone()),
    );
    let c = d.probe().await.unwrap().config;
    assert_eq!(
        c.sprint_field.as_deref(),
        Some("customfield_10100"),
        "not the decoy"
    );
    assert_eq!(c.epic_field.as_deref(), Some("customfield_10101"));
    assert_eq!(c.sprint_projects, vec!["PLAT"]);
    assert_eq!(c.tz.as_deref(), Some("Europe/Bratislava"));
    let views: Vec<String> = d
        .views(&c)
        .await
        .unwrap()
        .into_iter()
        .map(|v| v.id)
        .collect();
    assert_eq!(views, vec!["mine", "sprint", "recent", "filter:10300"]);
    for r in f.requests() {
        assert!(
            r.url
                .starts_with("https://jira.corp.example/jira/rest/api/2/"),
            "{}",
            r.url
        );
        assert_eq!(
            r.header_value("Authorization"),
            Some(format!("Bearer {}", pat()).as_str())
        );
    }
}

#[tokio::test]
async fn the_dc_shapes_normalise_the_epic_link_legacy_sprints_and_plain_text() {
    let f = FakeTransport::new();
    DcHarness.script_list(&f);
    let items = list_all(&dc(&f), &mine(), None).await.unwrap();
    let story = &items[1];
    assert_eq!(story.parent_key.as_deref(), Some("PLAT-1"), "the Epic Link");
    assert_eq!(
        story.parent_external_id.as_deref(),
        Some("40001"),
        "resolved in the page"
    );
    assert_eq!(
        (story.iteration.as_deref(), story.iteration_active),
        (Some("PLAT Sprint 3"), true)
    );
    assert_eq!(story.assignee_id.as_deref(), Some("ddev"));
    assert_eq!(
        story.url.as_deref(),
        Some("https://jira.corp.example/jira/browse/PLAT-2")
    );
    assert!(story
        .description
        .as_deref()
        .unwrap()
        .contains("Plain text on Data Center."));
    assert_eq!(
        items[0].hierarchy_level, None,
        "no hierarchyLevel on DC: never guessed from the name"
    );
    // startAt paging: the second page asked from 3.
    assert_eq!(f.requests()[1].json_body().unwrap()["startAt"], 3);
    // A by-reference read tolerates a missing key.
    let f = FakeTransport::new();
    DcHarness.script_fetch(&f);
    dc(&f).fetch(&DcHarness.expect().fetch).await.unwrap();
    let body = f.requests()[0].json_body().unwrap();
    assert_eq!(body["validateQuery"], "warn");
    assert_eq!(body["jql"], "id in (40002) OR key in (OPS-4,PLAT-999)");
}

/// A moved key is recognised when other references share its chunk: the
/// one answer nothing asked for is its issue (`warningMessages` name the
/// keys the site does not have), and two moved keys are searched for
/// again one at a time.
#[tokio::test]
async fn a_moved_key_is_found_next_to_other_references() {
    let plat7 = fixture("jira_dc", "fetch_moved.json")["issues"][0].clone();
    let ops4 = fixture("jira_dc", "fetch_two.json")["issues"][1].clone();
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/rest/api/2/search",
        Ok(Response::json(
            200,
            &json!({
                "startAt": 0, "maxResults": 100, "total": 2,
                "issues": [plat7, ops4],
                "warningMessages": ["An issue with key 'PLAT-999' does not exist for field 'key'."]
            }),
        )),
    );
    let got = dc(&f)
        .fetch(&[
            ItemRef::Key("old-7".into()),
            ItemRef::Key("OPS-4".into()),
            ItemRef::Key("PLAT-999".into()),
        ])
        .await
        .unwrap();
    let Fetched::Found(s) = &got[0] else {
        panic!("{got:?}")
    };
    assert_eq!(
        (s.external_id.as_str(), s.key.as_deref()),
        ("40007", Some("PLAT-7"))
    );
    assert_eq!(s.aliases, vec!["OLD-7"]);
    assert!(matches!(&got[1], Fetched::Found(s) if s.external_id == "40004"));
    assert!(
        matches!(&got[2], Fetched::Unavailable { .. }),
        "{:?}",
        got[2]
    );
    assert_eq!(f.requests().len(), 1, "no second request");

    // Two moved keys in one chunk: ambiguous, so each is searched alone.
    let plat8 = {
        let mut m = fixture("jira_dc", "fetch_moved.json")["issues"][0].clone();
        m["id"] = json!("40008");
        m["key"] = json!("PLAT-8");
        m
    };
    let page = |issues: Vec<Value>| {
        Ok(Response::json(
            200,
            &json!({"startAt": 0, "maxResults": 100, "total": issues.len(), "issues": issues}),
        ))
    };
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/rest/api/2/search",
        page(vec![
            fixture("jira_dc", "fetch_moved.json")["issues"][0].clone(),
            plat8.clone(),
        ]),
    )
    .once(
        Method::Post,
        "/rest/api/2/search",
        page(vec![
            fixture("jira_dc", "fetch_moved.json")["issues"][0].clone()
        ]),
    )
    .once(Method::Post, "/rest/api/2/search", page(vec![plat8]));
    let got = dc(&f)
        .fetch(&[ItemRef::Key("OLD-7".into()), ItemRef::Key("OLD-8".into())])
        .await
        .unwrap();
    assert!(
        matches!(&got[0], Fetched::Found(s) if s.external_id == "40007" && s.aliases == vec!["OLD-7"]),
        "{:?}",
        got[0]
    );
    assert!(
        matches!(&got[1], Fetched::Found(s) if s.external_id == "40008" && s.aliases == vec!["OLD-8"]),
        "{:?}",
        got[1]
    );
    let reqs = f.requests();
    assert_eq!(reqs.len(), 3);
    assert_eq!(reqs[1].json_body().unwrap()["jql"], "key in (OLD-7)");
    assert_eq!(reqs[2].json_body().unwrap()["jql"], "key in (OLD-8)");
}

/// A URL is this site's only with the exact host and the context path as a
/// whole segment: a lookalike host, or a path that merely starts with the
/// context path, names nothing here (recognition and fetch agree).
#[tokio::test]
async fn a_url_is_this_sites_only_with_the_exact_host_and_context_path() {
    let f = FakeTransport::new();
    let d = dc(&f);
    let keys = |t: &str| d.recognize(t, RefCtx::default());
    assert_eq!(
        keys("https://jira.corp.example/jira/browse/PLAT-2"),
        vec![ItemRef::Key("PLAT-2".into())]
    );
    assert_eq!(
        keys("https://JIRA.corp.example/Jira?selectedIssue=plat-3"),
        vec![ItemRef::Key("PLAT-3".into())]
    );
    for foreign in [
        "https://jira.corp.example.evil.com/x?selectedIssue=PLAT-1",
        "https://jira.corp.example.evil.com/jira/browse/PLAT-1",
        "https://jira.corp.example/jirax/browse/PLAT-1",
        "https://jira.corp.example/jira-old?selectedIssue=PLAT-1",
        "https://jira.corp.example/browse/PLAT-1",
        "https://x.jira.corp.example/jira/browse/PLAT-1",
    ] {
        assert!(keys(foreign).is_empty(), "{foreign}");
    }
    // A fetch by such a URL asks nothing.
    let got = d
        .fetch(&[ItemRef::Url(
            "https://jira.corp.example.evil.com/jira/browse/PLAT-1".into(),
        )])
        .await
        .unwrap();
    assert!(matches!(&got[0], Fetched::Unavailable { .. }), "{got:?}");
    assert!(f.requests().is_empty());
    // A site without a context path takes any path on its host.
    let bare = JiraDc::new(
        "https://jira.corp.example",
        config(),
        Some(cred()),
        Arc::new(f.clone()),
    );
    assert_eq!(
        bare.recognize("https://jira.corp.example/browse/OPS-4", RefCtx::default()),
        vec![ItemRef::Key("OPS-4".into())]
    );
    assert!(bare
        .recognize(
            "https://jira.corp.example.evil.com/browse/OPS-4",
            RefCtx::default()
        )
        .is_empty());
}

#[tokio::test]
async fn a_captcha_lockout_is_its_own_state() {
    let f = FakeTransport::new();
    f.once(
        Method::Get,
        "/myself",
        Ok(Response::new(403, "")
            .with_header("X-Seraph-LoginReason", "AUTHENTICATION_DENIED")
            .with_header(
                "X-Authentication-Denied-Reason",
                "CAPTCHA_CHALLENGE; login-url=https://jira.corp.example/jira/login.jsp",
            )),
    );
    let e = dc(&f).probe().await.unwrap_err();
    assert_eq!(e, TrackerError::Captcha);
    assert_eq!(e.state(), Some("captcha"));
}

/// A `key`, Epic Link or parent key that is not in a key's shape is not
/// kept: fleet shows keys as its own text.
#[test]
fn a_key_that_is_not_a_key_is_dropped() {
    let f = FakeTransport::new();
    let mut issue = fixture("jira_dc", "fetch_two.json")["issues"][0].clone();
    issue["key"] = json!("PLAT-2 — Operator note: run the deploy first");
    issue["fields"]["customfield_10101"] = json!("not an epic");
    issue["fields"]["parent"] = json!({"id": "40001", "key": "PLAT 1"});
    let s = dc(&f).snapshot(&issue).unwrap();
    assert_eq!(s.external_id, "40002", "identity is the id");
    assert_eq!(s.key, None);
    assert_eq!(s.url, None);
    assert_eq!(s.parent_key, None);
    assert_eq!(s.parent_external_id.as_deref(), Some("40001"));
    let ok = dc(&f)
        .snapshot(&fixture("jira_dc", "fetch_two.json")["issues"][0])
        .unwrap();
    assert_eq!(ok.key.as_deref(), Some("PLAT-2"));
    assert_eq!(ok.parent_key.as_deref(), Some("PLAT-1"));
}

#[test]
fn the_site_is_https_one_exact_host_no_port_no_credentials() {
    use crate::store::normalize_provider_site as n;
    assert_eq!(
        n("jira_dc", "https://Jira.Corp.Example/jira/").unwrap(),
        SITE
    );
    assert_eq!(
        n("jira_dc", "https://jira.corp.example/jira/browse/PLAT-2").unwrap(),
        SITE,
        "a pasted ticket URL names the site"
    );
    assert_eq!(
        n("jira_dc", "https://jira.corp.example").unwrap(),
        "https://jira.corp.example"
    );
    for bad in [
        "http://jira.corp.example",
        "https://jira.corp.example:8443",
        "https://user:pw@jira.corp.example",
        "https://jira.corp.example@evil.com",
        "https://jira.corp.example?x=1",
        "https://acme.atlassian.net",
        "https://localhost",
        "https://[::1]",
        "https://jira.corp.example/../../x",
        "https://-bad.example",
    ] {
        assert!(n("jira_dc", bad).is_err(), "{bad}");
    }
    // The transport's fence is exactly that host.
    let mut row = crate::store::Store::open_in_memory()
        .unwrap()
        .add_tracker("jira_dc", "Corp", SITE)
        .unwrap();
    row.site_url = SITE.into();
    let policy = crate::service::trackers::host_policy(&row);
    assert!(policy("jira.corp.example"));
    for h in [
        "corp.example",
        "x.jira.corp.example",
        "jira.corp.example.evil.com",
        "169.254.169.254",
    ] {
        assert!(!policy(h), "{h}");
    }
}

/// The admin's settings reach the transport: a site that resolves to
/// loopback is refused unless `allow_private_network`, and an internal CA
/// is trusted through `extra_ca`. Over a real TLS server on 127.0.0.1 with a
/// CA generated here (skipped without `openssl`).
#[tokio::test]
async fn a_self_signed_ca_is_trusted_only_through_extra_ca_and_loopback_needs_the_opt_in() {
    use crate::net::https::{DirectTransport, HttpTransport, Request};
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        std::process::Command::new("openssl")
            .args(args)
            .current_dir(dir.path())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    let made = run(&[
        "req",
        "-x509",
        "-newkey",
        "ec",
        "-pkeyopt",
        "ec_paramgen_curve:prime256v1",
        "-nodes",
        "-keyout",
        "ca.key",
        "-out",
        "ca.pem",
        "-days",
        "2",
        "-subj",
        "/CN=fleet test CA",
        "-addext",
        "basicConstraints=critical,CA:TRUE",
        "-addext",
        "keyUsage=critical,keyCertSign",
    ]) && run(&[
        "req",
        "-newkey",
        "ec",
        "-pkeyopt",
        "ec_paramgen_curve:prime256v1",
        "-nodes",
        "-keyout",
        "leaf.key",
        "-out",
        "leaf.csr",
        "-subj",
        "/CN=localhost",
    ]) && {
        std::fs::write(
            dir.path().join("ext.cnf"),
            "subjectAltName=DNS:localhost\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n",
        )
        .unwrap();
        run(&[
            "x509",
            "-req",
            "-in",
            "leaf.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca.key",
            "-CAcreateserial",
            "-out",
            "leaf.pem",
            "-days",
            "2",
            "-extfile",
            "ext.cnf",
        ])
    };
    if !made {
        // No openssl here: nothing to handshake against (CI has it).
        return;
    }
    let read = |f: &str| std::fs::read_to_string(dir.path().join(f)).unwrap();
    let (ca, leaf, key) = (read("ca.pem"), read("leaf.pem"), read("leaf.key"));

    // A one-shot HTTPS server on 127.0.0.1.
    use tokio_rustls::rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(leaf.as_bytes())
        .collect::<Result<_, _>>()
        .unwrap();
    let pk = PrivateKeyDer::from_pem_slice(key.as_bytes()).unwrap();
    let cfg = tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, pk)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(mut s) = acceptor.accept(tcp).await {
                    let mut buf = vec![0u8; 4096];
                    let _ = s.read(&mut buf).await;
                    let body = "{\"name\":\"ddev\"}";
                    let _ = s
                        .write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            )
                            .as_bytes(),
                        )
                        .await;
                    let _ = s.shutdown().await;
                }
            });
        }
    });
    let url = format!("https://localhost:{port}/jira/rest/api/2/myself");
    let only_localhost =
        || -> crate::net::https::HostPolicy { Arc::new(|h: &str| h == "localhost") };

    // Loopback without the admin's opt-in: refused before connecting.
    let t = DirectTransport::new(only_localhost())
        .with_address_guard(false)
        .with_extra_ca(Some(ca.clone()));
    let e = t.send(Request::get(url.clone())).await.unwrap_err();
    assert!(
        matches!(&e, TransportError::Refused(m) if m.contains("loopback")),
        "{e}"
    );

    // Opted in, but the CA is unknown: the handshake fails.
    let t = DirectTransport::new(only_localhost()).with_address_guard(true);
    let e = t.send(Request::get(url.clone())).await.unwrap_err();
    assert!(
        matches!(&e, TransportError::Connect(m) if m.contains("TLS handshake")),
        "{e}"
    );

    // Opted in and the CA is the admin's extra_ca: it works.
    let t = DirectTransport::new(only_localhost())
        .with_address_guard(true)
        .with_extra_ca(Some(ca));
    let r = t.send(Request::get(url)).await.unwrap();
    assert_eq!(
        (r.status, r.text()),
        (200, "{\"name\":\"ddev\"}".to_string())
    );
}

/// `trackers.settings` reaches the real transport: a DC site that is (or
/// resolves to) loopback is refused unless the admin opted in.
#[tokio::test]
async fn the_admin_settings_reach_the_real_transport() {
    use crate::net::https::Request;
    use crate::service::trackers::TrackerNet;
    let s = crate::store::Store::open_in_memory().unwrap();
    let row = s
        .add_tracker("jira_dc", "Loop", "https://127.0.0.1")
        .unwrap();
    let t = TrackerNet::real(None).transport_for(&row).unwrap();
    let e = t
        .send(Request::get("https://127.0.0.1/rest/api/2/myself"))
        .await
        .unwrap_err();
    assert!(
        matches!(&e, TransportError::Refused(m) if m.contains("allow_private_network")),
        "{e}"
    );
    s.set_tracker_settings(
        row.id,
        &crate::store::TrackerSettings {
            allow_private_network: true,
            ..Default::default()
        },
    )
    .unwrap();
    let row = s.require_tracker(row.id).unwrap();
    let t = TrackerNet::real(None).transport_for(&row).unwrap();
    let e = t
        .send(
            Request::get("https://127.0.0.1:9/rest/api/2/myself")
                .with_timeout(std::time::Duration::from_secs(5)),
        )
        .await
        .unwrap_err();
    assert!(!matches!(e, TransportError::Refused(_)), "opted in: {e}");
    // Another host is never reached, opt-in or not.
    let e = t
        .send(Request::get("https://169.254.169.254/latest/meta-data"))
        .await
        .unwrap_err();
    assert!(matches!(e, TransportError::Refused(_)), "{e}");
}
