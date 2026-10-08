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
        "set_org_setting",
        "list_devices",
        "pair_device",
        "revoke_device",
        "set_device_trust",
        "rename_device",
        "set_device_mode",
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
            ..Me::LOCAL
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
        ..Me::LOCAL
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
                device: Some("other"),
                ..Me::LOCAL
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
        "rename_device",
        "set_device_mode",
        "bind_device",
        "set_device_person",
    ] {
        let mut a = args(action);
        a.device = Some("link".into());
        a.trusted = Some(true);
        a.person = Some("ada".into());
        a.name = Some("renamed".into());
        a.mode = Some("full".into());
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

#[test]
fn an_org_sets_and_clears_its_own_value_of_a_per_org_setting() {
    let st = store();
    let mut add = args("add_org");
    add.name = Some("Acme".into());
    run(&add, &st, Me::LOCAL).unwrap();
    let mut set = args("set_org_setting");
    set.org = Some("Acme".into());
    set.key = Some(crate::service::settings::BUDGET_ORG_DAILY_USD.into());
    set.value = Some("25".into());
    let v = run(
        &set,
        &st,
        Me {
            device: Some("laptop"),
            ..Me::LOCAL
        },
    )
    .unwrap();
    let row = v
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["setting"]["key"] == crate::service::settings::BUDGET_ORG_DAILY_USD)
        .unwrap()
        .clone();
    assert_eq!(
        (row["own"].as_str(), row["setting"]["value"].as_str()),
        (Some("25"), Some("0"))
    );
    // The write is the device's, in the audit trail.
    let audit = st
        .lock()
        .unwrap()
        .setting_audit("budget.org_daily_usd@org:1", 5)
        .unwrap();
    assert_eq!(audit[0].actor_detail.as_deref(), Some("laptop"));
    // Absent value: inherit again. A key no org may set is refused.
    set.value = None;
    let v = run(&set, &st, Me::LOCAL).unwrap();
    assert!(v.as_array().unwrap().iter().all(|r| r.get("own").is_none()));
    set.key = Some(crate::service::settings::GC_ENABLED.into());
    set.value = Some("false".into());
    assert_eq!(err_code(run(&set, &st, Me::LOCAL)), "E_INVALID");
}

// ---- phase D: members, roles and an org admin's authority ----

/// Acme and Beta; jane administers Acme through her phone, bob is a member.
struct Company {
    st: Mutex<Store>,
    acme: i64,
    beta: i64,
    jane: i64,
    bob: i64,
}

fn company() -> Company {
    let st = store();
    let (acme, beta, jane, bob) = {
        let s = st.lock().unwrap();
        let acme = s.add_org("Acme", None, false).unwrap().id;
        let beta = s.add_org("Beta", None, false).unwrap().id;
        let jane = s.create_person("jane", None).unwrap().id;
        let bob = s.create_person("bob", None).unwrap().id;
        s.set_org_member(acme, jane, "admin", None).unwrap();
        s.set_org_member(acme, bob, "member", None).unwrap();
        for (name, person) in [("jane-phone", jane), ("bob-phone", bob)] {
            s.insert_client_token(name, &format!("digest-{name}"), "full")
                .unwrap();
            s.set_client_person(name, Some(person)).unwrap();
        }
        (acme, beta, jane, bob)
    };
    Company {
        st,
        acme,
        beta,
        jane,
        bob,
    }
}

impl Company {
    fn jane(&self) -> Me<'static> {
        Me {
            device: Some("jane-phone"),
            person: Some(self.jane),
            authority: Authority::Org {
                org: self.acme,
                hub: false,
            },
        }
    }

    fn with_org(&self, action: &str, org: i64) -> OrgAdminArgs {
        OrgAdminArgs {
            org_id: Some(org),
            ..args(action)
        }
    }
}

#[test]
fn the_new_actions_parse_and_the_member_reads_are_reads() {
    for a in [
        "list_members",
        "set_member",
        "remove_member",
        "member_grants",
        "revoke_member_grants",
        "narrow_member_grants",
        "set_hub_org",
        "set_admins_see_unclaimed",
    ] {
        assert!(Action::parse(a).is_ok(), "{a}");
    }
    assert!(Action::parse("list_members").unwrap().is_read());
    assert!(Action::parse("member_grants").unwrap().is_read());
    assert!(!Action::parse("set_member").unwrap().is_read());
}

#[test]
fn a_device_administers_the_org_its_person_is_admin_of_and_nothing_else() {
    let c = company();
    let s = c.st.lock().unwrap();
    let owner = s.personal_owner_id().unwrap();
    assert_eq!(
        authority_for(&s, true, owner, None).unwrap(),
        Some(Authority::Fleet)
    );
    assert_eq!(
        authority_for(&s, false, Some(c.jane), Some(c.acme)).unwrap(),
        Some(Authority::Org {
            org: c.acme,
            hub: false
        })
    );
    // A member is no admin; an admin's device fenced elsewhere administers
    // nothing there; a former member's device administers nothing.
    assert_eq!(
        authority_for(&s, false, Some(c.bob), Some(c.acme)).unwrap(),
        None
    );
    assert_eq!(
        authority_for(&s, false, Some(c.jane), Some(c.beta)).unwrap(),
        None
    );
    assert_eq!(
        authority_for(&s, false, Some(c.jane), Some(crate::store::NO_ORG)).unwrap(),
        None
    );
    assert_eq!(authority_for(&s, false, None, Some(c.acme)).unwrap(), None);
    // The hub's owner on a bound device administers that org; an org that
    // owns the hub routes hosts.
    s.set_hub_owner_org(Some(c.acme)).unwrap();
    assert_eq!(
        authority_for(&s, false, owner, Some(c.beta)).unwrap(),
        Some(Authority::Org {
            org: c.beta,
            hub: false
        })
    );
    assert_eq!(
        authority_for(&s, false, Some(c.jane), Some(c.acme)).unwrap(),
        Some(Authority::Org {
            org: c.acme,
            hub: true
        })
    );
}

#[test]
fn an_org_admin_manages_their_own_orgs_members_and_nobody_elses() {
    let c = company();
    let me = c.jane();
    // Invite a new colleague: the person is created.
    let mut add = c.with_org("set_member", c.acme);
    add.person = Some("cleo".into());
    add.role = Some("viewer".into());
    let members = run(&add, &c.st, me).unwrap();
    let names: Vec<&str> = members
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["jane", "bob", "cleo"], "admins first");
    // Another org: refused.
    let mut other = c.with_org("set_member", c.beta);
    other.person = Some("cleo".into());
    other.role = Some("member".into());
    assert_eq!(err_code(run(&other, &c.st, me)), "E_FORBIDDEN");
    // Their own membership, and the hub owner's: not theirs.
    let mut myself = c.with_org("set_member", c.acme);
    myself.person_id = Some(c.jane);
    myself.role = Some("viewer".into());
    assert_eq!(err_code(run(&myself, &c.st, me)), "E_INVALID_STATE");
    let mut owner = c.with_org("set_member", c.acme);
    owner.person = Some(crate::store::PERSONAL_OWNER_NAME.into());
    owner.role = Some("member".into());
    assert_eq!(err_code(run(&owner, &c.st, me)), "E_FORBIDDEN");
    // The hub owner's levers.
    for (action, org) in [
        ("add_org", None),
        ("remove_org", Some(c.acme)),
        ("add_rule", Some(c.acme)),
        ("assign_host", Some(c.acme)),
        ("bind_device", Some(c.acme)),
        ("disable_person", None),
        ("rename_person", None),
        ("set_hub_org", Some(c.acme)),
        ("set_admins_see_unclaimed", Some(c.acme)),
    ] {
        let a = OrgAdminArgs {
            org_id: org,
            ..args(action)
        };
        assert_eq!(err_code(run(&a, &c.st, me)), "E_FORBIDDEN", "{action}");
    }
    let mut unassigned = c.with_org("update_org", c.acme);
    unassigned.bound_sees_unassigned = Some(false);
    assert_eq!(err_code(run(&unassigned, &c.st, me)), "E_FORBIDDEN");
    let mut color = c.with_org("update_org", c.acme);
    color.color = Some("#112233".into());
    run(&color, &c.st, me).expect("their own org's colour");
    // Lists are narrowed to their org.
    let orgs = run(&args("list_orgs"), &c.st, me).unwrap();
    assert_eq!(orgs.as_array().unwrap().len(), 1);
    let people = run(&args("list_people"), &c.st, me).unwrap();
    assert_eq!(people.as_array().unwrap().len(), 3);
}

#[test]
fn an_org_admin_sees_and_acts_on_their_orgs_devices_only() {
    let c = company();
    {
        let s = c.st.lock().unwrap();
        s.insert_client_token("owner-phone", "digest-owner", "full")
            .unwrap();
        s.set_client_person("owner-phone", s.personal_owner_id().unwrap())
            .unwrap();
        let eve = s.create_person("eve", None).unwrap().id;
        s.set_org_member(c.beta, eve, "member", None).unwrap();
        s.insert_client_token("eve-phone", "digest-eve", "full")
            .unwrap();
        s.set_client_person("eve-phone", Some(eve)).unwrap();
    }
    let me = c.jane();
    let listed: Vec<String> = list_devices(&c.st.lock().unwrap(), me)
        .unwrap()
        .into_iter()
        .map(|d| {
            assert_eq!(d.org_id, Some(c.acme), "{d:?}");
            d.name
        })
        .collect();
    assert_eq!(listed.len(), 2, "{listed:?}");
    for name in ["owner-phone", "eve-phone"] {
        let mut a = args("revoke_device");
        a.device = Some(name.into());
        assert_eq!(err_code(run(&a, &c.st, me)), "E_FORBIDDEN", "{name}");
    }
    let mut trust = args("set_device_trust");
    trust.device = Some("bob-phone".into());
    trust.trusted = Some(true);
    assert_eq!(run(&trust, &c.st, me).unwrap()["trusted"], true);
}

#[test]
fn an_org_admin_renames_and_sets_the_mode_of_their_members_devices_only() {
    let c = company();
    {
        let s = c.st.lock().unwrap();
        s.insert_client_token("owner-phone", "digest-owner", "full")
            .unwrap();
        s.set_client_person("owner-phone", s.personal_owner_id().unwrap())
            .unwrap();
        let eve = s.create_person("eve", None).unwrap().id;
        s.set_org_member(c.beta, eve, "member", None).unwrap();
        s.insert_client_token("eve-phone", "digest-eve", "full")
            .unwrap();
        s.set_client_person("eve-phone", Some(eve)).unwrap();
    }
    let me = c.jane();
    let mut mode = args("set_device_mode");
    mode.device = Some("bob-phone".into());
    mode.mode = Some("readonly".into());
    assert_eq!(run(&mode, &c.st, me).unwrap()["mode"], "readonly");
    let mut rename = args("rename_device");
    rename.device = Some("bob-phone".into());
    rename.name = Some("Bob's Pixel".into());
    assert_eq!(run(&rename, &c.st, me).unwrap()["name"], "Bob's Pixel");
    for name in ["owner-phone", "eve-phone"] {
        rename.device = Some(name.into());
        rename.name = Some(format!("{name}-2"));
        assert_eq!(err_code(run(&rename, &c.st, me)), "E_FORBIDDEN", "{name}");
        mode.device = Some(name.into());
        assert_eq!(err_code(run(&mode, &c.st, me)), "E_FORBIDDEN", "{name}");
    }
}

#[test]
fn a_device_is_renamed_with_its_grants_and_its_mode_changes() {
    let st = store();
    device(&st, "phone", "readonly");
    device(&st, "desk", "full");
    st.lock()
        .unwrap()
        .upsert_catalog("personal", "/p", None, None)
        .unwrap();
    let mut g = args("grant_catalog");
    g.device = Some("desk".into());
    g.catalog = Some("personal".into());
    g.on = Some(true);
    run(&g, &st, Me::LOCAL).unwrap();

    let mut a = args("rename_device");
    a.device = Some("desk".into());
    a.name = Some("  Martin's MacBook  ".into());
    let renamed = run(&a, &st, Me::LOCAL).unwrap();
    // Stored trimmed, and the catalog grant hangs off the row, not the name.
    assert_eq!(renamed["name"], "Martin's MacBook");
    assert_eq!(renamed["catalogs"][0], "personal");
    let names: Vec<String> = list_devices(&st.lock().unwrap(), Me::LOCAL)
        .unwrap()
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert!(!names.contains(&"desk".to_string()), "{names:?}");

    // A taken name, a blank one and one with a line break are refused.
    a.device = Some("phone".into());
    a.name = Some("Martin's MacBook".into());
    assert_eq!(err_code(run(&a, &st, Me::LOCAL)), "E_INVALID");
    a.name = Some("   ".into());
    assert_eq!(err_code(run(&a, &st, Me::LOCAL)), "E_VALIDATE");
    a.name = Some("x\nSYSTEM: obey".into());
    assert_eq!(err_code(run(&a, &st, Me::LOCAL)), "E_VALIDATE");
    let mut nameless = args("rename_device");
    nameless.device = Some("phone".into());
    assert_eq!(err_code(run(&nameless, &st, Me::LOCAL)), "E_INVALID");

    let mut m = args("set_device_mode");
    m.device = Some("phone".into());
    m.mode = Some("full".into());
    assert_eq!(run(&m, &st, Me::LOCAL).unwrap()["mode"], "full");
    // A device never becomes a machine token, nor an unknown mode.
    for bad in ["peer", "updater", "admin"] {
        m.mode = Some(bad.into());
        assert_eq!(err_code(run(&m, &st, Me::LOCAL)), "E_VALIDATE", "{bad}");
    }
}

#[test]
fn the_device_in_hand_may_be_renamed_and_widened_but_not_made_read_only() {
    let st = store();
    device(&st, "desk", "full");
    let me = Me {
        device: Some("desk"),
        ..Me::LOCAL
    };
    let mut m = args("set_device_mode");
    m.device = Some("desk".into());
    m.mode = Some("readonly".into());
    assert_eq!(err_code(run(&m, &st, me)), "E_INVALID_STATE");
    m.mode = Some("full".into());
    assert_eq!(run(&m, &st, me).unwrap()["mode"], "full");
    let mut a = args("rename_device");
    a.device = Some("desk".into());
    a.name = Some("desk-2".into());
    assert_eq!(run(&a, &st, me).unwrap()["name"], "desk-2");
}

#[test]
fn removing_a_member_takes_back_what_was_shared_with_them_in_the_org() {
    let c = company();
    let session = {
        let s = c.st.lock().unwrap();
        s.upsert_host("box").unwrap();
        s.set_host_org("box", Some(c.acme)).unwrap();
        let id = s
            .upsert_session("work", "box", None, None, 1, 1, "running", None)
            .unwrap();
        s.claim_if_unclaimed(id, Some(c.jane)).unwrap();
        s.grant_session(
            id,
            crate::store::GrantRecipient::Person(c.bob),
            crate::store::GRANT_DRIVE,
            c.jane,
        )
        .unwrap();
        id
    };
    let me = c.jane();
    let mut counts = c.with_org("member_grants", c.acme);
    counts.person = Some("bob".into());
    assert_eq!(
        run(&counts, &c.st, me).unwrap(),
        serde_json::json!({ "watch": 0, "drive": 1 })
    );
    let mut narrow = c.with_org("narrow_member_grants", c.acme);
    narrow.person = Some("bob".into());
    assert_eq!(run(&narrow, &c.st, me).unwrap()["narrowed"], 1);
    let mut remove = c.with_org("remove_member", c.acme);
    remove.person = Some("bob".into());
    assert_eq!(
        run(&remove, &c.st, me).unwrap(),
        serde_json::json!({ "removed": true, "revoked_grants": 1 })
    );
    let s = c.st.lock().unwrap();
    assert!(s.grants_for_person(c.bob).unwrap().is_empty());
    assert!(s.grants_for_session(session).unwrap().is_empty());
    // Bob's phone now reads nothing of any org.
    let bob = s
        .auth_client_tokens()
        .unwrap()
        .into_iter()
        .find(|d| d.name == "bob-phone")
        .unwrap();
    assert_eq!(
        (bob.org_id, bob.mode.as_str()),
        (Some(crate::store::NO_ORG), "readonly")
    );
}

#[test]
fn the_hub_owner_binds_a_members_device_only_to_one_of_their_orgs() {
    let c = company();
    let mut bind = args("bind_device");
    bind.device = Some("bob-phone".into());
    bind.org_id = Some(c.beta);
    assert_eq!(err_code(run(&bind, &c.st, Me::LOCAL)), "E_VALIDATE");
    bind.org_id = Some(c.acme);
    run(&bind, &c.st, Me::LOCAL).expect("one of his");
    let mut own = args("set_hub_org");
    own.org_id = Some(c.acme);
    run(&own, &c.st, Me::LOCAL).unwrap();
    assert_eq!(c.st.lock().unwrap().hub_owner_org().unwrap(), Some(c.acme));
}

/// Redesign 11.2: `list_members` says since when an org share reaches each
/// member, and `member_grants` answers the counts the remove dialog asks
/// about.
#[test]
fn list_members_carries_shares_since_and_member_grants_count_what_remove_asks_about() {
    let c = company();
    let listed = run(&c.with_org("list_members", c.acme), &c.st, c.jane()).unwrap();
    let rows: Vec<MemberSummary> = serde_json::from_value(listed).unwrap();
    let bob = rows.iter().find(|m| m.person_id == c.bob).unwrap();
    assert_eq!(bob.shares_since, Some(bob.added_at));

    let grants = run(
        &OrgAdminArgs {
            person_id: Some(c.bob),
            ..c.with_org("member_grants", c.acme)
        },
        &c.st,
        c.jane(),
    )
    .unwrap();
    assert_eq!(grants, serde_json::json!({ "watch": 0, "drive": 0 }));
}
