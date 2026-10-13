use super::*;
use crate::mcp::auth::{Caller, TokenMode};

fn seen(key: &str, state: &str) -> SeenDevice {
    SeenDevice {
        dev_key: key.into(),
        platform: "android".into(),
        kind: "physical".into(),
        serial: Some(key.into()),
        name: "Pixel 7".into(),
        model: None,
        os_version: None,
        state: state.into(),
    }
}

/// Hosts `a` and `b` with no org, `c` in org Acme; one phone on each.
fn fleet() -> Arc<Mutex<Store>> {
    let s = Store::open_in_memory().unwrap();
    for h in ["a", "b", "c"] {
        s.upsert_host(h).unwrap();
    }
    let acme = s.add_org("Acme", None, false).unwrap();
    s.set_host_org("c", Some(acme.id)).unwrap();
    s.debug_devices_apply_scan("a", &[seen("A1", "online")], 1)
        .unwrap();
    s.debug_devices_apply_scan("b", &[seen("B1", "online")], 1)
        .unwrap();
    s.debug_devices_apply_scan("c", &[seen("C1", "online")], 1)
        .unwrap();
    Arc::new(Mutex::new(s))
}

/// An asker built the one way production builds one: `Caller::view_scope`.
fn asker_for(store: &Mutex<Store>, caller: Caller, holder: &str) -> Asker {
    let s = store.lock().unwrap();
    Asker {
        scope: caller.view_scope(&s).unwrap(),
        holder: holder.into(),
    }
}

fn host(store: &Mutex<Store>, alias: &str) -> Asker {
    let caller = Caller {
        api: None,
        host_alias: Some(alias.into()),
        client: None,
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
        is_fleet_admin: false,
    };
    asker_for(store, caller, &format!("host:{alias}"))
}

/// The person: the desktop on its own store.
fn person() -> Asker {
    Asker::desktop()
}

fn keys(store: &Mutex<Store>, asker: &Asker) -> Vec<String> {
    let s = store.lock().unwrap();
    visible(&s, asker)
        .unwrap()
        .into_iter()
        .map(|d| d.key)
        .collect()
}

fn id_of(store: &Mutex<Store>, key: &str) -> i64 {
    let s = store.lock().unwrap();
    s.debug_devices()
        .unwrap()
        .into_iter()
        .find(|d| d.dev_key == key)
        .unwrap()
        .id
}

#[test]
fn a_host_sees_its_own_devices_and_what_its_org_shares() {
    let st = fleet();
    assert_eq!(keys(&st, &person()), ["A1", "B1", "C1"]);
    assert_eq!(keys(&st, &host(&st, "a")), ["A1"]);
    // b shares its phone, c (another org) shares its phone too.
    configure(&st, &person(), id_of(&st, "B1"), None, Some(true)).unwrap();
    configure(&st, &person(), id_of(&st, "C1"), None, Some(true)).unwrap();
    assert_eq!(keys(&st, &host(&st, "a")), ["A1", "B1"]);
    // Org Acme's host sees only its own: b's org is not its org.
    assert_eq!(keys(&st, &host(&st, "c")), ["C1"]);
}

#[test]
fn a_bound_client_never_sees_another_orgs_devices() {
    let st = fleet();
    let acme = st.lock().unwrap().host_org("c").unwrap().unwrap();
    {
        let s = st.lock().unwrap();
        let beta = s.add_org("Beta", None, false).unwrap();
        s.set_host_org("b", Some(beta.id)).unwrap();
    }
    let caller = Caller {
        api: None,
        host_alias: None,
        client: Some(crate::mcp::auth::ClientRef {
            id: 7,
            name: "work".into(),
            trusted: false,
            org_id: Some(acme),
            person_id: None,
        }),
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
        is_fleet_admin: false,
    };
    let bound = asker_for(&st, caller, "client:work");
    // Its own org's, and unassigned hosts' (D31's default); never Beta's.
    assert_eq!(keys(&st, &bound), ["A1", "C1"]);
}

#[test]
fn a_host_cannot_share_label_or_forget_a_device() {
    let st = fleet();
    let a1 = id_of(&st, "A1");
    for e in [
        configure(&st, &host(&st, "a"), a1, None, Some(true)).unwrap_err(),
        configure(&st, &host(&st, "a"), a1, Some("x"), None).unwrap_err(),
        forget(&st, &host(&st, "a"), a1).unwrap_err(),
    ] {
        assert_eq!(e.code, codes::E_FORBIDDEN);
    }
    // And a host never reaches another host's unshared device at all.
    let b1 = id_of(&st, "B1");
    assert_eq!(
        claim(&st, &host(&st, "a"), &b1.to_string(), None, None, false)
            .unwrap_err()
            .code,
        codes::E_NOTFOUND
    );
}

#[test]
fn a_device_resolves_by_id_label_serial_and_host_prefix() {
    let st = fleet();
    configure(&st, &person(), id_of(&st, "A1"), Some("bench"), None).unwrap();
    let s = st.lock().unwrap();
    let p = person();
    assert_eq!(resolve(&s, &p, "bench").unwrap().key, "A1");
    assert_eq!(resolve(&s, &p, "BENCH").unwrap().title, "bench");
    assert_eq!(resolve(&s, &p, "b1").unwrap().key, "B1");
    assert_eq!(resolve(&s, &p, "c/pixel 7").unwrap().key, "C1");
    let one = resolve(&s, &p, "B1").unwrap().id;
    assert_eq!(resolve(&s, &p, &format!("#{one}")).unwrap().key, "B1");
    // Three hosts have a "Pixel 7".
    let e = resolve(&s, &p, "Pixel 7").unwrap_err();
    assert_eq!(e.code, codes::E_AMBIGUOUS);
    assert!(
        e.message.contains("a/") && e.message.contains("c/"),
        "{}",
        e.message
    );
    assert_eq!(resolve(&s, &p, "nope").unwrap_err().code, codes::E_NOTFOUND);
}

/// A bare number that is one device's id and another's serial names
/// neither for sure; `#id` still picks the id.
#[test]
fn a_number_that_is_an_id_and_another_serial_is_ambiguous() {
    let st = fleet();
    let a1 = id_of(&st, "A1");
    let s = st.lock().unwrap();
    s.debug_devices_apply_scan(
        "b",
        &[seen("B1", "online"), seen(&a1.to_string(), "online")],
        2,
    )
    .unwrap();
    let p = person();
    let e = resolve(&s, &p, &a1.to_string()).unwrap_err();
    assert_eq!(e.code, codes::E_AMBIGUOUS, "{}", e.message);
    assert_eq!(resolve(&s, &p, &format!("#{a1}")).unwrap().key, "A1");
    assert_eq!(resolve(&s, &p, &format!("b/{a1}")).unwrap().host, "b");
}

#[test]
fn a_claim_holds_off_others_until_released() {
    let st = fleet();
    let b1 = id_of(&st, "B1").to_string();
    configure(&st, &person(), b1.parse().unwrap(), None, Some(true)).unwrap();
    let a = host(&st, "a");
    let b = host(&st, "b");
    let d = claim(&st, &a, &b1, Some(600), Some("ui tests"), false).unwrap();
    assert_eq!(d.claimed_by.as_deref(), Some("host:a"));
    assert_eq!(d.claim_note.as_deref(), Some("ui tests"));
    // Another host is refused the claim and the device.
    assert_eq!(
        claim(&st, &b, &b1, None, None, false).unwrap_err().code,
        codes::E_CONFLICT
    );
    assert_eq!(
        usable(&st, &b, &b1, false).unwrap_err().code,
        codes::E_CONFLICT
    );
    // The holder uses it, and extends the claim by using it.
    assert!(usable(&st, &a, &b1, false).is_ok());
    let until = st
        .lock()
        .unwrap()
        .debug_device(b1.parse().unwrap())
        .unwrap()
        .unwrap()
        .claimed_until
        .unwrap();
    assert!(until >= now_unix() + DEFAULT_CLAIM_SECS - 5);
    // Only the holder or a person releases it.
    assert_eq!(release(&st, &b, &b1).unwrap_err().code, codes::E_FORBIDDEN);
    assert_eq!(release(&st, &person(), &b1).unwrap().claimed_by, None);
    assert!(usable(&st, &b, &b1, false).is_ok());
}

/// Hosts `a`, `b` and `c` all see B1 (shared, and `c` moved into b's org).
fn shared_b1() -> (Arc<Mutex<Store>>, String) {
    let st = fleet();
    st.lock().unwrap().set_host_org("c", None).unwrap();
    let b1 = id_of(&st, "B1").to_string();
    configure(&st, &person(), b1.parse().unwrap(), None, Some(true)).unwrap();
    (st, b1)
}

#[test]
fn a_queued_claim_waits_its_turn_and_gets_the_device_on_release() {
    let (st, b1) = shared_b1();
    let (a, b, c) = (host(&st, "a"), host(&st, "b"), host(&st, "c"));
    claim(&st, &a, &b1, None, None, false).unwrap();
    // b and c line up; neither is refused.
    let d = claim(&st, &b, &b1, Some(900), Some("ui tests"), true).unwrap();
    assert_eq!(d.claimed_by.as_deref(), Some("host:a"));
    assert_eq!(d.queue_place, Some(1));
    let d = claim(&st, &c, &b1, None, None, true).unwrap();
    assert_eq!(d.queue_place, Some(2));
    let line: Vec<&str> = d.queue.iter().map(|w| w.holder.as_str()).collect();
    assert_eq!(line, ["host:b", "host:c"]);
    // Asking again keeps the place.
    assert_eq!(
        claim(&st, &b, &b1, None, Some("ui tests"), true)
            .unwrap()
            .queue_place,
        Some(1)
    );
    // Without queue it is still refused, and says others wait.
    let e = claim(&st, &c, &b1, None, None, false).unwrap_err();
    assert_eq!(e.code, codes::E_CONFLICT);
    assert!(e.message.contains("2 waiting"), "{}", e.message);
    // a is done: b holds it now, for the hand-off, with its note.
    let d = release(&st, &a, &b1).unwrap();
    assert_eq!(d.claimed_by.as_deref(), Some("host:b"));
    assert_eq!(d.claim_note.as_deref(), Some("ui tests"));
    assert!(d.claimed_until.unwrap() <= now_unix() + HANDOFF_SECS);
    assert_eq!(d.queue.len(), 1);
    assert!(usable(&st, &b, &b1, false).is_ok());
    assert_eq!(
        usable(&st, &a, &b1, false).unwrap_err().code,
        codes::E_CONFLICT
    );
    // c gives up its place, and so does a person: neither takes b's claim.
    let d = release(&st, &c, &b1).unwrap();
    assert_eq!(
        (d.claimed_by.as_deref(), d.queue_place),
        (Some("host:b"), None)
    );
    assert!(d.queue.is_empty());
    claim(&st, &person(), &b1, None, None, true).unwrap();
    let d = release(&st, &person(), &b1).unwrap();
    assert_eq!((d.claimed_by.as_deref(), d.waiting), (Some("host:b"), 0));
    // Out of line, a person's release takes the claim; b's would too.
    assert_eq!(release(&st, &person(), &b1).unwrap().claimed_by, None);
}

#[test]
fn a_lapsed_claim_passes_to_the_first_in_line() {
    let (st, b1) = shared_b1();
    let id: i64 = b1.parse().unwrap();
    let (a, c) = (host(&st, "a"), host(&st, "c"));
    claim(&st, &a, &b1, None, None, false).unwrap();
    claim(&st, &c, &b1, None, None, true).unwrap();
    // A host not in line still may not release another's claim.
    assert_eq!(
        release(&st, &host(&st, "b"), &b1).unwrap_err().code,
        codes::E_FORBIDDEN
    );
    st.lock()
        .unwrap()
        .debug_device_claim(id, "host:a", None, now_unix() - 1)
        .unwrap();
    let d = resolve(&st.lock().unwrap(), &person(), &b1).unwrap();
    assert_eq!(d.claimed_by.as_deref(), Some("host:c"));
    assert!(d.queue.is_empty());
    // A free device is claimed outright, queue or not.
    release(&st, &c, &b1).unwrap();
    let d = claim(&st, &a, &b1, None, None, true).unwrap();
    assert_eq!(
        (d.claimed_by.as_deref(), d.queue_place),
        (Some("host:a"), None)
    );
}

#[test]
fn an_expired_claim_reads_as_none() {
    let st = fleet();
    let a1 = id_of(&st, "A1");
    st.lock()
        .unwrap()
        .debug_device_claim(a1, "host:z", None, now_unix() - 1)
        .unwrap();
    let d = resolve(&st.lock().unwrap(), &person(), "A1").unwrap();
    assert_eq!((d.claimed_by, d.claimed_until), (None, None));
    assert!(claim(&st, &host(&st, "a"), "A1", None, None, false).is_ok());
}

#[test]
fn a_device_that_is_not_ready_says_what_to_do() {
    let st = fleet();
    st.lock()
        .unwrap()
        .debug_devices_apply_scan("a", &[seen("A1", "unauthorized")], 2)
        .unwrap();
    let e = usable(&st, &person(), "A1", false).unwrap_err();
    assert_eq!(e.code, codes::E_INVALID_STATE);
    assert!(e.message.contains("USB debugging"), "{}", e.message);
    st.lock()
        .unwrap()
        .debug_devices_apply_scan("a", &[], 3)
        .unwrap();
    let e = usable(&st, &person(), "A1", false).unwrap_err();
    assert!(e.message.contains("missing"), "{}", e.message);
}

#[test]
fn labels_are_bounded_and_an_empty_one_clears() {
    let st = fleet();
    let a1 = id_of(&st, "A1");
    let p = person();
    assert_eq!(
        configure(&st, &p, a1, Some("a/b"), None).unwrap_err().code,
        codes::E_INVALID
    );
    assert_eq!(
        configure(&st, &p, a1, Some(" bench "), None).unwrap().title,
        "bench"
    );
    let d = configure(&st, &p, a1, Some(""), None).unwrap();
    assert_eq!((d.label, d.title.as_str()), (None, "Pixel 7"));
    assert!(forget(&st, &p, a1).unwrap());
    assert_eq!(keys(&st, &p), ["B1", "C1"]);
}

#[tokio::test]
async fn a_failed_scan_is_recorded_and_keeps_the_devices() {
    let st = fleet();
    let ssh = crate::ssh_fake::FakeSsh::new();
    let scans = scan(&st, &ssh, &person(), Some("a")).await.unwrap();
    assert_eq!(scans.len(), 1);
    assert!(!scans[0].ok);
    let s = st.lock().unwrap();
    assert!(s.debug_device_scans().unwrap()["a"].error.is_some());
    assert_eq!(
        s.debug_device(id_of_locked(&s, "A1"))
            .unwrap()
            .unwrap()
            .state,
        "online"
    );
}

fn id_of_locked(s: &Store, key: &str) -> i64 {
    s.debug_devices()
        .unwrap()
        .into_iter()
        .find(|d| d.dev_key == key)
        .unwrap()
        .id
}

#[tokio::test]
async fn a_host_scans_only_itself() {
    let st = fleet();
    let ssh = crate::ssh_fake::FakeSsh::new();
    let e = scan(&st, &ssh, &host(&st, "a"), Some("b"))
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    let all = scan(&st, &ssh, &host(&st, "a"), None).await.unwrap();
    assert_eq!(
        all.iter().map(|s| s.host.as_str()).collect::<Vec<_>>(),
        ["a"]
    );
}

#[tokio::test]
async fn a_host_installs_only_from_itself() {
    let st = fleet();
    let ssh = crate::ssh_fake::FakeSsh::new();
    let e = install(
        &st,
        &ssh,
        &host(&st, "a"),
        "A1",
        InstallFrom {
            host: "b",
            path: "/tmp/app.apk",
        },
        false,
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, codes::E_FORBIDDEN);
}

#[tokio::test]
async fn a_scan_inventories_what_runs_and_keeps_a_known_stopped_simulator() {
    use crate::ssh_fake::{FakeSsh, Match, Reply};
    let st = fleet();
    let ssh = FakeSsh::new();
    let marker = crate::service::move_session::carry::OUT_MARKER;
    let booted = format!(
        "\n{marker}\n##TOOLS adb=1 emulator= xcrun=1\n##ADB\nList of devices attached\n\
         R5CT1   device usb:1 model:Pixel_7 transport_id:1\n##SIMCTL\n\
         {{\"devices\":{{\"com.apple.CoreSimulator.SimRuntime.iOS-18-0\":[\
         {{\"udid\":\"SIM-1\",\"name\":\"iPhone 16\",\"state\":\"Booted\",\"isAvailable\":true}},\
         {{\"udid\":\"SIM-2\",\"name\":\"iPhone SE\",\"state\":\"Shutdown\",\"isAvailable\":true}}]}}}}\n##END\n"
    );
    ssh.on_host(
        "a",
        Match::script_contains("cf-devices:scan"),
        Reply::ok(&booted),
    );
    let out = scan(&st, &ssh, &person(), Some("a")).await.unwrap();
    assert!(out[0].ok, "{:?}", out[0].error);
    assert_eq!(out[0].found, 2);
    assert_eq!(out[0].bootable.len(), 1);
    // The phone that was there before (A1) is now missing; R5CT1 and the
    // booted simulator are inventoried; the never-run SE is only bootable.
    let mut on_a: Vec<(String, String)> = st
        .lock()
        .unwrap()
        .debug_devices()
        .unwrap()
        .into_iter()
        .filter(|d| d.host_alias == "a")
        .map(|d| (d.dev_key, d.state))
        .collect();
    on_a.sort();
    assert_eq!(
        on_a,
        [
            ("A1".to_string(), "missing".to_string()),
            ("R5CT1".into(), "online".into()),
            ("SIM-1".into(), "booted".into()),
        ]
    );
    // The simulator shuts down: it stays inventoried as `shutdown`.
    let stopped = booted.replace("\"Booted\"", "\"Shutdown\"");
    ssh.on_host(
        "a",
        Match::script_contains("cf-devices:scan"),
        Reply::ok(&stopped),
    );
    scan(&st, &ssh, &person(), Some("a")).await.unwrap();
    let s = st.lock().unwrap();
    let sim = s
        .debug_devices()
        .unwrap()
        .into_iter()
        .find(|d| d.dev_key == "SIM-1")
        .unwrap();
    assert_eq!(
        (sim.state.as_str(), sim.serial.as_deref()),
        ("shutdown", Some("SIM-1"))
    );
    assert!(!s
        .debug_devices()
        .unwrap()
        .iter()
        .any(|d| d.dev_key == "SIM-2"));
}
