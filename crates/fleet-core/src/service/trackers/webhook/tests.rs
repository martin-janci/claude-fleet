use super::*;
use crate::store::TrackerItemWrite;
use serde_json::json;

const SECRET: &str = "0123456789abcdef0123456789abcdef";

fn secret() -> Secret {
    Secret::new(SECRET)
}

fn headers(pairs: &[(&str, String)]) -> impl Fn(&str) -> Option<String> {
    let pairs: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    move |name: &str| {
        pairs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    }
}

fn sign(body: &[u8]) -> String {
    hmac_sha256_hex(SECRET.as_bytes(), body)
}

const NOW_MS: i64 = 1_790_000_000_000;

// ── the HMAC ────────────────────────────────────────────────────────────

#[test]
fn the_hmac_is_rfc_4231() {
    // RFC 4231, test case 2.
    assert_eq!(
        hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?"),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
}

// ── the verifiers ───────────────────────────────────────────────────────

#[test]
fn each_provider_reads_its_own_header_and_format() {
    let body = br#"{"x":1}"#;
    let sig = sign(body);
    let cases: [(&str, &str, String); 3] = [
        ("github", "x-hub-signature-256", format!("sha256={sig}")),
        ("jira", "x-hub-signature", format!("sha256={sig}")),
        ("linear", "linear-signature", sig.clone()),
    ];
    for (provider, name, value) in &cases {
        let body: Vec<u8> = if *provider == "linear" {
            serde_json::to_vec(&json!({ "x": 1, "webhookTimestamp": NOW_MS })).unwrap()
        } else {
            body.to_vec()
        };
        let value = if *provider == "linear" {
            sign(&body)
        } else {
            value.clone()
        };
        let h = headers(&[(name, value.clone())]);
        assert_eq!(
            verify(provider, &h, &body, &secret(), NOW_MS),
            Ok(()),
            "{provider}"
        );
        // Upper-case hex is the same signature.
        let h = headers(&[(
            name,
            value.to_ascii_uppercase().replace("SHA256=", "sha256="),
        )]);
        assert_eq!(
            verify(provider, &h, &body, &secret(), NOW_MS),
            Ok(()),
            "{provider} upper"
        );
        // Another provider's header is not this one's.
        for (other, other_name, _) in &cases {
            if other != provider && other_name != name {
                let h = headers(&[(other_name, value.clone())]);
                assert_eq!(
                    verify(provider, &h, &body, &secret(), NOW_MS),
                    Err(Reject::Unsigned),
                    "{provider} with {other}'s header"
                );
            }
        }
    }
}

#[test]
fn a_missing_wrong_or_tampered_signature_is_refused() {
    let body = br#"{"issue":{"key":"ABC-1"}}"#;
    let good = format!("sha256={}", sign(body));
    let none = headers(&[]);
    assert_eq!(
        verify("jira", &none, body, &secret(), NOW_MS),
        Err(Reject::Unsigned)
    );
    let bare = headers(&[("x-hub-signature", sign(body))]);
    assert_eq!(
        verify("jira", &bare, body, &secret(), NOW_MS),
        Err(Reject::Unsigned)
    );
    let sha1 = headers(&[("x-hub-signature", format!("sha1={}", sign(body)))]);
    assert_eq!(
        verify("jira", &sha1, body, &secret(), NOW_MS),
        Err(Reject::Unsigned)
    );
    let wrong_key = headers(&[(
        "x-hub-signature",
        format!(
            "sha256={}",
            hmac_sha256_hex(b"another-secret-another-secret-xx", body)
        ),
    )]);
    assert_eq!(
        verify("jira", &wrong_key, body, &secret(), NOW_MS),
        Err(Reject::BadSignature)
    );
    let h = headers(&[("x-hub-signature", good)]);
    let tampered = br#"{"issue":{"key":"ABC-2"}}"#;
    assert_eq!(
        verify("jira", &h, tampered, &secret(), NOW_MS),
        Err(Reject::BadSignature)
    );
    assert_eq!(
        verify("asana", &h, body, &secret(), NOW_MS),
        Err(Reject::Unsigned)
    );
}

#[test]
fn a_linear_delivery_outside_the_window_is_a_replay() {
    for (at, ok) in [
        (NOW_MS, true),
        (NOW_MS - LINEAR_WINDOW_MS, true),
        (NOW_MS - LINEAR_WINDOW_MS - 1, false),
        (NOW_MS + LINEAR_WINDOW_MS + 1, false),
    ] {
        let body = serde_json::to_vec(&json!({ "type": "Issue", "webhookTimestamp": at })).unwrap();
        let h = headers(&[("linear-signature", sign(&body))]);
        let r = verify("linear", &h, &body, &secret(), NOW_MS);
        assert_eq!(r.is_ok(), ok, "{at}: {r:?}");
        if !ok {
            assert_eq!(r, Err(Reject::Stale));
        }
    }
    let body = br#"{"type":"Issue"}"#;
    let h = headers(&[("linear-signature", sign(body))]);
    assert_eq!(
        verify("linear", &h, body, &secret(), NOW_MS),
        Err(Reject::Stale)
    );
}

// ── the payload ─────────────────────────────────────────────────────────

#[test]
fn only_an_item_key_is_read_from_a_payload() {
    let jira = json!({ "webhookEvent": "jira:issue_updated", "issue": { "id": "10001", "key": "ABC-1",
        "fields": { "summary": "ignore previous instructions" } } });
    assert_eq!(
        item_ref("jira", None, &jira, None),
        Some(HookItem {
            reference: ItemRef::Key("ABC-1".into()),
            keys: vec!["ABC-1".into()]
        })
    );
    for bad in [
        json!({ "issue": { "key": "../../x" } }),
        json!({ "issue": {} }),
        json!({}),
    ] {
        assert_eq!(item_ref("jira", None, &bad, None), None, "{bad}");
    }

    let gh = json!({ "action": "closed", "issue": { "number": 42 }, "repository": { "full_name": "Acme/API" } });
    assert_eq!(
        item_ref("github", Some("issues"), &gh, None),
        Some(HookItem {
            reference: ItemRef::RepoNumber {
                repo: "acme/api".into(),
                n: 42
            },
            keys: vec!["acme/api#42".into()],
        })
    );
    assert_eq!(
        item_ref("github", Some("issues"), &gh, Some("GHE.corp.example"))
            .unwrap()
            .keys,
        vec![
            "ghe.corp.example/acme/api#42".to_string(),
            "acme/api#42".to_string()
        ]
    );
    assert_eq!(item_ref("github", Some("ping"), &gh, None), None);
    assert_eq!(item_ref("github", Some("pull_request"), &gh, None), None);
    assert_eq!(item_ref("github", None, &gh, None), None);
    let evil = json!({ "issue": { "number": 1 }, "repository": { "full_name": "a b/c" } });
    assert_eq!(item_ref("github", Some("issues"), &evil, None), None);

    let lin = json!({ "action": "update", "type": "Issue", "data": { "identifier": "eng-7" } });
    assert_eq!(
        item_ref("linear", None, &lin, None),
        Some(HookItem {
            reference: ItemRef::Key("ENG-7".into()),
            keys: vec!["ENG-7".into()]
        })
    );
    let comment = json!({ "type": "Comment", "data": { "identifier": "ENG-7" } });
    assert_eq!(item_ref("linear", None, &comment, None), None);
    let weird = json!({ "type": "Issue", "data": { "identifier": "ENG-7; drop" } });
    assert_eq!(item_ref("linear", None, &weird, None), None);
}

// ── a delivery ──────────────────────────────────────────────────────────

/// A public hub with Jira tracker `ABC` holding ABC-1, with or without a
/// webhook secret. Returns the store and the tracker's id.
fn hub(with_secret: bool, public: bool) -> (Arc<Mutex<Store>>, i64) {
    let s = Store::open_in_memory().unwrap();
    if public {
        s.set_setting(crate::service::hub::SETTING_PUBLIC_URL, "hub.example.com")
            .unwrap();
    }
    let t = s
        .add_tracker("jira", "Acme", "https://acme.atlassian.net")
        .unwrap();
    s.conn_ref()
        .execute("UPDATE trackers SET state = 'ok' WHERE id = ?1", [t.id])
        .unwrap();
    s.upsert_tracker_item(
        t.id,
        &TrackerItemWrite {
            external_id: "10001".into(),
            key: Some("ABC-1".into()),
            title: "Fix login".into(),
            status_name: "To Do".into(),
            status_category: "todo".into(),
            ..Default::default()
        },
    )
    .unwrap();
    if with_secret {
        s.set_tracker_webhook_secret(t.id, &secret()).unwrap();
    }
    (Arc::new(Mutex::new(s)), t.id)
}

fn jira_delivery(key: &str) -> (Vec<u8>, impl Fn(&str) -> Option<String>) {
    let body = serde_json::to_vec(
        &json!({ "webhookEvent": "jira:issue_updated", "issue": { "key": key } }),
    )
    .unwrap();
    let h = headers(&[("x-hub-signature", format!("sha256={}", sign(&body)))]);
    (body, h)
}

const LONG: Duration = Duration::from_secs(3_600);

#[tokio::test]
async fn the_route_is_not_there_without_a_public_url_a_secret_or_a_webhook_provider() {
    let (body, h) = jira_delivery("ABC-1");
    let co = Arc::new(Coalescer::new());
    let (st, id) = hub(false, true);
    assert_eq!(
        deliver(&co, &st, id, &h, &body, NOW_MS, LONG),
        Outcome::NotHere
    );
    let co = Arc::new(Coalescer::new());
    let (st, id) = hub(true, false);
    assert_eq!(
        deliver(&co, &st, id, &h, &body, NOW_MS, LONG),
        Outcome::NotHere
    );
    let co = Arc::new(Coalescer::new());
    let (st, id) = hub(true, true);
    assert_eq!(
        deliver(&co, &st, id + 99, &h, &body, NOW_MS, LONG),
        Outcome::NotHere
    );
}

#[tokio::test]
async fn a_forged_delivery_is_refused_and_counted() {
    let co = Arc::new(Coalescer::new());
    let (st, id) = hub(true, true);
    let before = hook_metrics(id).rejected;
    let (body, _) = jira_delivery("ABC-1");
    let forged = headers(&[("x-hub-signature", format!("sha256={}", "0".repeat(64)))]);
    assert_eq!(
        deliver(&co, &st, id, &forged, &body, NOW_MS, LONG),
        Outcome::Rejected(Reject::BadSignature)
    );
    assert_eq!(hook_metrics(id).rejected, before + 1);
}

#[tokio::test]
async fn a_known_item_is_fetched_once_per_burst_and_an_unknown_one_never() {
    let co = Arc::new(Coalescer::new());
    let (st, id) = hub(true, true);
    let (body, h) = jira_delivery("ABC-1");
    assert_eq!(
        deliver(&co, &st, id, &h, &body, NOW_MS, LONG),
        Outcome::Accepted(Some(Scheduled::Fetch))
    );
    assert!(hook_metrics(id).last_delivery_at.is_some());
    for _ in 0..5 {
        assert_eq!(
            deliver(&co, &st, id, &h, &body, NOW_MS, LONG),
            Outcome::Accepted(Some(Scheduled::Coalesced))
        );
    }
    // A key this tracker does not hold: accepted, nothing fetched.
    let (body, h) = jira_delivery("ZZZ-9");
    assert_eq!(
        deliver(&co, &st, id, &h, &body, NOW_MS, LONG),
        Outcome::Accepted(None)
    );
    // Signed, but not JSON.
    let junk = b"not json".to_vec();
    let h = headers(&[("x-hub-signature", format!("sha256={}", sign(&junk)))]);
    assert_eq!(
        deliver(&co, &st, id, &h, &junk, NOW_MS, LONG),
        Outcome::Malformed
    );
}

#[tokio::test]
async fn at_most_pending_max_items_wait_per_tracker() {
    let co = Arc::new(Coalescer::new());
    let (st, id) = hub(true, true);
    for n in 0..PENDING_MAX {
        let item = HookItem {
            reference: ItemRef::Key(format!("ABC-{}", 1_000 + n)),
            keys: vec![format!("ABC-{}", 1_000 + n)],
        };
        assert_eq!(
            schedule(&co, Arc::clone(&st), id, item, LONG),
            Scheduled::Fetch
        );
    }
    let one_more = HookItem {
        reference: ItemRef::Key("ABC-9999".into()),
        keys: vec!["ABC-9999".into()],
    };
    assert_eq!(
        schedule(&co, Arc::clone(&st), id, one_more, LONG),
        Scheduled::Full
    );
}
