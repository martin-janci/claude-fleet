use super::*;
use crate::service::account_limits::HostLogin;

fn host_row(alias: &str) -> HostRow {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host(alias).unwrap();
    s.get_host_row(alias).unwrap().unwrap()
}

fn status(blocked: Option<&str>) -> OperatorStatus {
    serde_json::from_value(serde_json::json!({
        "ready": blocked.is_none(),
        "session": null,
        "blocked": blocked,
        "host": "mefistos",
    }))
    .unwrap()
}

fn client(kind: &str, version: Option<&str>) -> ClientCheck {
    let (layout, has, lacks) = client_features(kind);
    ClientCheck {
        name: kind.into(),
        kind: kind.into(),
        version: version.map(Into::into),
        last_seen_at: Some(1),
        layout,
        has,
        lacks,
    }
}

#[test]
fn an_unknown_host_says_so_as_a_checked_fact() {
    let c = host_check("trn", None, 100);
    assert!(!c.known && c.checked);
    assert!(c.why.contains("no host named trn"));
}

#[test]
fn an_unreachable_host_names_its_probe_error_and_an_old_reading_is_inferred() {
    let now = 1_000_000;
    let mut h = host_row("trn");
    h.reachable = false;
    h.last_pinged_at = Some(now - 120);
    h.last_reachable_at = Some(now - 7200);
    h.last_probe_error_code = Some("E_SSH".into());
    h.last_probe_error = Some("connection refused".into());
    let c = host_check("trn", Some(&h), now);
    assert!(c.checked);
    assert!(
        c.why.contains("did not answer") && c.why.contains("E_SSH: connection refused"),
        "{}",
        c.why
    );
    assert!(c.why.contains("last reachable 2 h ago"), "{}", c.why);

    h.last_pinged_at = Some(now - STALE_READING_SECS - 1);
    assert!(!host_check("trn", Some(&h), now).checked);
    h.last_pinged_at = None;
    let never = host_check("trn", Some(&h), now);
    assert!(!never.checked && never.why.contains("not been probed"));
}

#[test]
fn a_phone_lacks_the_desktops_control_buttons() {
    let (layout, has, lacks) = client_features("android");
    assert_eq!(layout, "chat");
    assert!(has.contains(&"confirm_cards"));
    assert!(lacks.contains(&"autonomy_chip") && lacks.contains(&"escalation_cards"));
    let (layout, has, lacks) = client_features("desktop");
    assert_eq!(layout, "thread_room");
    assert_eq!(has.len(), FEATURES.len());
    assert!(lacks.is_empty());
    let (_, has, _) = client_features("unknown");
    assert!(has.is_empty());
}

#[test]
fn findings_mark_what_could_not_be_checked() {
    let h = host_check("mefistos", None, 0);
    // No login reading, no client version: both inferred.
    let f = findings(&status(None), &h, None, 90.0, &[client("unknown", None)]);
    assert!(f[0].what.contains("Control runs on mefistos") && f[0].checked);
    assert!(f
        .iter()
        .any(|x| x.what.contains("limit is unknown") && !x.checked));
    assert!(f
        .iter()
        .any(|x| x.what.contains("is a guess") && !x.checked));

    let over = LoginAccount {
        host_alias: "mefistos".into(),
        login: HostLogin {
            profile: Some("work".into()),
            account_uuid: "u".into(),
            used_pct: Some(97.0),
        },
        email: None,
        over: true,
        daily: None,
    };
    let f = findings(
        &status(Some("host_down")),
        &h,
        Some(&over),
        90.0,
        &[
            client("desktop", Some("0.6.2")),
            client("android", Some("0.6.2")),
        ],
    );
    assert!(f[0].what.contains("cannot work: host_down"));
    assert!(f
        .iter()
        .any(|x| x.checked && x.what.contains("(work) is at 97%") && x.what.contains("90%")));
    assert!(f.iter().any(|x| x.what.contains("a desktop and a phone")));
}

#[test]
fn diagnose_answers_from_the_store_without_a_session() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("trn").unwrap();
    let store = Mutex::new(s);
    let d = diagnose(
        &store,
        &DiagnoseArgs {
            host: Some("trn".into()),
        },
        None,
        100,
    )
    .unwrap();
    assert_eq!(d.host.as_ref().unwrap().alias, "trn");
    assert!(d.findings.iter().any(|f| f.what.contains("trn")));
    assert!(d.clients.is_empty());
}
