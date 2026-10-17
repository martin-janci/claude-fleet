use super::*;
use crate::http_client::HubResponse;

const NOW: i64 = 1_768_472_200;

fn upload(id: &str, lines: Vec<String>) -> LogPost {
    LogPost {
        id: Some(id.into()),
        lines: Some(lines),
        app: None,
    }
}

#[test]
fn a_request_is_answered_once_per_client() {
    let mut l = ClientLogs::default();
    assert!(
        l.post("master", LogPost::default(), NOW).is_none(),
        "nothing asked"
    );
    let id = l.ask(None, 60, NOW);
    assert_eq!(
        l.ask(None, 30, NOW + 1),
        id,
        "an open request covers a narrower one"
    );
    let want = l.post("client:pixel", LogPost::default(), NOW + 5).unwrap();
    assert_eq!(want["want"]["id"], id.as_str());
    assert!(l
        .post("client:pixel", upload("stale", vec!["x".into()]), NOW + 6)
        .is_none());
    assert!(
        l.fresh(None, 60, NOW + 6).is_empty(),
        "an upload for another request is dropped"
    );
    l.post(
        "client:pixel",
        upload(&id, vec!["a".into(), "b".into()]),
        NOW + 7,
    );
    assert!(
        l.post("client:pixel", LogPost::default(), NOW + 8)
            .is_none(),
        "answered"
    );
    assert!(
        l.post("master", LogPost::default(), NOW + 8).is_some(),
        "the desktop still owes it"
    );
    let got = l.fresh(Some("PIXEL"), 60, NOW + 9);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].1.lines, ["a", "b"]);
    assert_eq!(l.seen(NOW + 9).len(), 1);
    // Both expire.
    assert!(l.post("master", LogPost::default(), NOW + 700).is_none());
    assert!(l.fresh(None, 60, NOW + 700).is_empty());
    assert!(l.seen(NOW + 4000).is_empty());
}

#[test]
fn a_request_for_one_client_is_not_asked_of_another() {
    let mut l = ClientLogs::default();
    l.ask(Some("pixel"), 60, NOW);
    assert!(l.post("master", LogPost::default(), NOW).is_none());
    assert!(l.post("client:pixel-8", LogPost::default(), NOW).is_some());
}

#[test]
fn an_upload_keeps_its_newest_lines_cut_short() {
    let mut l = ClientLogs::default();
    let id = l.ask(None, 60, NOW);
    let mut lines: Vec<String> = (0..UPLOAD_MAX_LINES + 5).map(|i| i.to_string()).collect();
    lines.push("y".repeat(5000));
    l.post("master", upload(&id, lines), NOW);
    let u = &l.fresh(None, 60, NOW)[0].1;
    assert_eq!(u.lines.len(), UPLOAD_MAX_LINES);
    assert_eq!(u.lines[0], "6");
    assert_eq!(u.lines.last().unwrap().len(), 2000);
}

struct Fake {
    answers: Mutex<Vec<Result<HubResponse, String>>>,
    seen: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl HubTransport for Fake {
    async fn post_json(
        &self,
        _url: &str,
        _bearer: &str,
        body: String,
    ) -> Result<HubResponse, String> {
        self.seen.lock().unwrap().push(body);
        self.answers.lock().unwrap().remove(0)
    }
}

fn poller(
    answers: Vec<Result<HubResponse, String>>,
    dir: Option<std::path::PathBuf>,
) -> (LogPoller, Arc<Fake>) {
    let fake = Arc::new(Fake {
        answers: Mutex::new(answers),
        seen: Mutex::new(Vec::new()),
    });
    let p = LogPoller {
        url: "http://hub/log".into(),
        token: "t".into(),
        app: "desktop 1".into(),
        transport: fake.clone(),
        dir,
    };
    (p, fake)
}

fn ok(status: u16, body: &str) -> Result<HubResponse, String> {
    Ok(HubResponse {
        status,
        body: body.into(),
    })
}

#[tokio::test]
async fn the_desktop_sends_its_log_only_when_asked() {
    let (p, _) = poller(vec![ok(204, "")], None);
    assert_eq!(p.poll_once(NOW).await, Poll::Idle);

    // The file's mtime is now, after NOW: it is read.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("claude-fleet.2026-01-15-10.log"),
        "2026-01-15T09:00:00.000000Z  INFO fleet::app: too old\n\
         2026-01-15T10:15:40.000000Z  ERROR fleet::hub: auth Bearer sk-ant-api03-abcdefghijklmnopqrstuvwxyz failed\n",
    )
    .unwrap();
    let want = r#"{"want":{"id":"abc","minutes":30}}"#;
    let (p, fake) = poller(vec![ok(200, want), ok(204, "")], Some(tmp.path().into()));
    assert_eq!(p.poll_once(NOW).await, Poll::Sent(1));
    let body = fake.seen.lock().unwrap()[1].clone();
    assert!(
        body.contains("fleet::hub") && !body.contains("abcdefghijklmnop"),
        "{body}"
    );

    let (p, fake) = poller(vec![ok(200, want), ok(204, "")], None);
    assert_eq!(p.poll_once(NOW).await, Poll::Sent(0));
    let seen = fake.seen.lock().unwrap().clone();
    assert_eq!(seen[0], "{}");
    let sent: Value = serde_json::from_str(&seen[1]).unwrap();
    assert_eq!(
        (sent["id"].as_str(), sent["app"].as_str()),
        (Some("abc"), Some("desktop 1"))
    );

    let (p, _) = poller(vec![ok(404, "")], None);
    assert_eq!(p.poll_once(NOW).await, Poll::Stop);
    let (p, _) = poller(vec![Err("reset".into())], None);
    assert_eq!(p.poll_once(NOW).await, Poll::Retry);
    let (p, _) = poller(vec![ok(200, "{}")], None);
    assert_eq!(p.poll_once(NOW).await, Poll::Retry);
}
