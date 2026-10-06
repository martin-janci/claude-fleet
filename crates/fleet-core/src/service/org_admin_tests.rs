use super::*;

fn store() -> Mutex<Store> {
    Mutex::new(Store::open_in_memory().unwrap())
}

fn args(action: &str) -> OrgAdminArgs {
    OrgAdminArgs::new(action)
}

fn device(st: &Mutex<Store>, name: &str, mode: &str) {
    let s = st.lock().unwrap();
    s.insert_client_token(name, &format!("digest-{name}"), mode)
        .unwrap();
}

fn err_code(r: Result<serde_json::Value, IpcError>) -> String {
    r.expect_err("refused").code.to_string()
}

#[test]
fn every_action_parses_and_assign_client_is_not_one() {
    for a in [
        "list_orgs",
        "add_org",
        "update_org",
        "remove_org",
        "add_rule",
        "remove_rule",
        "assign_host",
        "unassign_host",
        "assign_tracker",
        "list_devices",
        "pair_device",
        "revoke_device",
        "set_device_trust",
        "bind_device",
        "set_device_person",
        "grant_catalog",
        "list_people",
        "rename_person",
        "disable_person",
    ] {
        assert!(Action::parse(a).is_ok(), "{a}");
    }
    // A device's org goes through `bind_device`, which knows the lock-out
    // rule; `work_admin`'s `assign_client` does not.
    assert!(Action::parse("assign_client").is_err());
    assert!(
        Action::parse("add").is_err(),
        "a tracker action is not here"
    );
    let reads: Vec<&str> = ["list_orgs", "list_devices", "list_people", "revoke_device"]
        .into_iter()
        .filter(|a| Action::parse(a).unwrap().is_read())
        .collect();
    assert_eq!(reads, vec!["list_orgs", "list_devices", "list_people"]);
}

#[test]
fn org_actions_run_orgs_admin_and_take_an_org_by_name() {
    let st = store();
    let mut add = args("add_org");
    add.name = Some("Acme".into());
    let org = run(&add, &st, Me::LOCAL).unwrap();
    let id = org["id"].as_i64().unwrap();
    let mut rule = args("add_rule");
    rule.org = Some("Acme".into());
    rule.owner = Some("acme".into());
    run(&rule, &st, Me::LOCAL).unwrap();
    let orgs = run(&args("list_orgs"), &st, Me::LOCAL).unwrap();
    assert_eq!(orgs[0]["id"], id);
    assert_eq!(orgs[0]["rules"][0]["owner"], "acme");
    let mut bad = args("update_org");
    bad.org = Some("Nobody".into());
    assert_eq!(err_code(run(&bad, &st, Me::LOCAL)), "E_NOTFOUND");
}

#[test]
fn devices_list_their_org_person_and_grants_and_never_a_machine_token() {
    let st = store();
    device(&st, "phone", "full");
    device(&st, "link", "peer");
    let org = {
        let s = st.lock().unwrap();
        let org = s.add_org("Acme", None, false).unwrap();
        s.upsert_catalog("personal", "/p", None, None).unwrap();
        s.set_client_assets_admin("phone", true).unwrap();
        org
    };
    let got = list_devices(
        &st.lock().unwrap(),
        Me {
            device: Some("phone"),
        },
    )
    .unwrap();
    assert_eq!(got.len(), 1, "a peer link is not a device: {got:?}");
    assert_eq!(got[0].name, "phone");
    assert_eq!(got[0].catalogs, vec!["personal".to_string()]);
    assert!(got[0].this_device);
    // Bound to an org, the device carries the org's name.
    let mut bind = args("bind_device");
    bind.device = Some("phone".into());
    bind.org = Some("Acme".into());
    let v = run(&bind, &st, Me::LOCAL).unwrap();
    assert_eq!(
        (v["org_id"].as_i64(), v["org"].as_str()),
        (Some(org.id), Some("Acme"))
    );
    // Unbound again (no org named).
    bind.org = None;
    let v = run(&bind, &st, Me::LOCAL).unwrap();
    assert!(v.get("org_id").is_none(), "{v}");
}

#[test]
fn the_device_in_hand_cannot_be_locked_out_through_itself() {
    let st = store();
    device(&st, "desk", "full");
    st.lock()
        .unwrap()
        .upsert_catalog("personal", "/p", None, None)
        .unwrap();
    let me = Me {
        device: Some("desk"),
    };
    let mut a = args("revoke_device");
    a.device = Some("desk".into());
    assert_eq!(err_code(run(&a, &st, me)), "E_INVALID_STATE");
    let mut a = args("set_device_trust");
    a.device = Some("desk".into());
    a.trusted = Some(false);
    assert_eq!(err_code(run(&a, &st, me)), "E_INVALID_STATE");
    // Trusting it, or granting it a catalog, only widens: allowed.
    a.trusted = Some(true);
    assert_eq!(run(&a, &st, me).unwrap()["trusted"], true);
    let mut g = args("grant_catalog");
    g.device = Some("desk".into());
    g.catalog = Some("personal".into());
    g.on = Some(true);
    assert_eq!(run(&g, &st, me).unwrap()["catalogs"][0], "personal");
    g.on = Some(false);
    assert_eq!(err_code(run(&g, &st, me)), "E_INVALID_STATE");
    let mut b = args("bind_device");
    b.device = Some("desk".into());
    assert_eq!(err_code(run(&b, &st, me)), "E_INVALID_STATE");
    let mut p = args("set_device_person");
    p.device = Some("desk".into());
    p.person = Some("ada".into());
    assert_eq!(err_code(run(&p, &st, me)), "E_INVALID_STATE");
    // Nothing was created by the refused hand-over.
    assert!(st
        .lock()
        .unwrap()
        .get_person_by_name("ada")
        .unwrap()
        .is_none());
    // The same device, from the desktop's own store or another device: fine.
    let mut a = args("revoke_device");
    a.device = Some("desk".into());
    assert_eq!(
        run(
            &a,
            &st,
            Me {
                device: Some("other")
            }
        )
        .unwrap()["revoked"],
        "desk"
    );
}

#[test]
fn a_machine_token_is_refused_by_every_device_action() {
    let st = store();
    device(&st, "link", "peer");
    for action in [
        "revoke_device",
        "set_device_trust",
        "bind_device",
        "set_device_person",
    ] {
        let mut a = args(action);
        a.device = Some("link".into());
        a.trusted = Some(true);
        a.person = Some("ada".into());
        assert_eq!(err_code(run(&a, &st, Me::LOCAL)), "E_VALIDATE", "{action}");
    }
    let mut a = args("revoke_device");
    a.device = Some("nobody".into());
    assert_eq!(err_code(run(&a, &st, Me::LOCAL)), "E_NOTFOUND");
}

#[test]
fn a_device_is_handed_to_a_person_created_on_first_use() {
    let st = store();
    device(&st, "laptop", "full");
    let mut a = args("set_device_person");
    a.device = Some("laptop".into());
    a.person = Some("ada".into());
    let v = run(&a, &st, Me::LOCAL).unwrap();
    assert_eq!(v["person"], "ada");
    let people = list_people(&st.lock().unwrap()).unwrap();
    assert!(people[0].owner, "the owner first: {people:?}");
    let ada = people.iter().find(|p| p.name == "ada").unwrap();
    assert_eq!(ada.devices, vec!["laptop".to_string()]);
    // Nobody is not an answer: a device belongs to somebody.
    a.person = None;
    assert_eq!(err_code(run(&a, &st, Me::LOCAL)), "E_INVALID");
}

#[test]
fn people_are_renamed_and_disabled_but_never_the_owner() {
    let st = store();
    device(&st, "laptop", "full");
    let (owner, ada) = {
        let s = st.lock().unwrap();
        let owner = s.personal_owner_id().unwrap().unwrap();
        let ada = s.create_person("ada", None).unwrap();
        s.set_client_person("laptop", Some(ada.id)).unwrap();
        (owner, ada.id)
    };
    let mut r = args("rename_person");
    r.person_id = Some(ada);
    r.name = Some("ada-l".into());
    assert_eq!(run(&r, &st, Me::LOCAL).unwrap()["name"], "ada-l");
    let mut d = args("disable_person");
    d.person_id = Some(owner);
    assert_eq!(err_code(run(&d, &st, Me::LOCAL)), "E_VALIDATE");
    // Disabling a person revokes their devices.
    d.person_id = Some(ada);
    assert!(run(&d, &st, Me::LOCAL).unwrap()["disabled_at"].is_i64());
    assert!(list_devices(&st.lock().unwrap(), Me::LOCAL)
        .unwrap()
        .is_empty());
}

#[test]
fn pairing_is_the_hubs_and_refused_on_a_desktops_own_store() {
    let st = store();
    assert_eq!(
        err_code(run(&args("pair_device"), &st, Me::LOCAL)),
        "E_INVALID_STATE"
    );
}

#[test]
fn the_audit_line_names_ids_and_names_and_escapes_them() {
    let mut a = args("revoke_device");
    a.device = Some("evil\nline".into());
    a.org_id = Some(3);
    let line = a.audit_summary();
    assert!(!line.contains('\n'), "{line}");
    assert!(line.contains("org_id=3"), "{line}");
}
