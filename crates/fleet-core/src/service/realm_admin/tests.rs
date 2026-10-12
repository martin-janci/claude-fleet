use super::*;
use crate::service::org_admin::{Authority, Me};
use crate::store::{AdminEventQuery, Principal, RoleRef, ROLE_FLEET_ADMIN};

/// Two orgs; root holds `fleet-admin`, ann administers acme (mo is a
/// member there), oz administers other.
fn fixture() -> (Mutex<Store>, i64, i64) {
    let s = Store::open_in_memory().unwrap();
    let acme = s.add_org("acme", None, false).unwrap().id;
    let other = s.add_org("other", None, false).unwrap().id;
    let id = |n: &str| s.create_person(n, None).unwrap().id;
    let (root, ann, mo, oz) = (id("root"), id("ann"), id("mo"), id("oz"));
    s.set_org_member(acme, ann, "admin", None).unwrap();
    s.set_org_member(acme, mo, "member", None).unwrap();
    s.set_org_member(other, oz, "admin", None).unwrap();
    let fa = s
        .role(RoleRef::Realm(ROLE_FLEET_ADMIN))
        .unwrap()
        .unwrap()
        .id;
    s.map_role(Principal::Person(root), fa).unwrap();
    (Mutex::new(s), acme, other)
}

fn person(store: &Mutex<Store>, name: &str) -> i64 {
    store
        .lock()
        .unwrap()
        .get_person_by_name(name)
        .unwrap()
        .unwrap()
        .id
}

fn code(r: Result<serde_json::Value, IpcError>) -> String {
    r.expect_err("refused").code
}

fn args(action: &str) -> RealmAdminArgs {
    RealmAdminArgs {
        action: action.into(),
        ..Default::default()
    }
}

/// Every action × every kind of caller → allowed, or the exact refusal.
#[test]
fn who_may_do_what() {
    let (store, acme, other) = fixture();
    let fleet = Me {
        device: Some("root-mac"),
        person: Some(person(&store, "root")),
        authority: Authority::Fleet,
    };
    let acme_admin = Me {
        device: Some("ann-mac"),
        person: Some(person(&store, "ann")),
        authority: Authority::Org {
            org: acme,
            hub: false,
        },
    };
    let other_admin = Me {
        device: Some("oz-mac"),
        person: Some(person(&store, "oz")),
        authority: Authority::Org {
            org: other,
            hub: false,
        },
    };
    let add = |g: &str| RealmAdminArgs {
        action: "add_member".into(),
        group: Some(g.into()),
        user: Some("mo".into()),
        ..Default::default()
    };
    assert!(run(&add("/orgs/acme/viewers"), &store, fleet).is_ok());
    assert!(run(&add("/orgs/acme/admins"), &store, acme_admin).is_ok());
    assert_eq!(
        code(run(&add("/orgs/acme/admins"), &store, other_admin)),
        codes::E_FORBIDDEN
    );
    let create = RealmAdminArgs {
        action: "create_group".into(),
        name: Some("eng".into()),
        ..Default::default()
    };
    assert!(run(&create, &store, fleet).is_ok());
    assert_eq!(code(run(&create, &store, acme_admin)), codes::E_FORBIDDEN);
    let map = RealmAdminArgs {
        action: "map_role".into(),
        user: Some("mo".into()),
        role: Some("fleet-admin".into()),
        ..Default::default()
    };
    assert_eq!(code(run(&map, &store, acme_admin)), codes::E_FORBIDDEN);
    assert!(run(&map, &store, fleet).is_ok());
    let audit = args("audit");
    assert!(run(&audit, &store, fleet).is_ok());
    assert_eq!(code(run(&audit, &store, acme_admin)), codes::E_FORBIDDEN);
    // An org admin reads only their own org's groups.
    let list = args("list_groups");
    let groups = run(&list, &store, acme_admin).unwrap();
    assert!(
        groups.to_string().contains("/orgs/acme") && !groups.to_string().contains("/orgs/other")
    );
}

#[test]
fn nobody_removes_their_own_fleet_admin() {
    let (store, _, _) = fixture();
    let root = person(&store, "root");
    let fleet = Me {
        device: Some("root-mac"),
        person: Some(root),
        authority: Authority::Fleet,
    };
    let unmap = RealmAdminArgs {
        action: "unmap_role".into(),
        user: Some("root".into()),
        role: Some("fleet-admin".into()),
        ..Default::default()
    };
    assert_eq!(code(run(&unmap, &store, fleet)), codes::E_FORBIDDEN);
}

#[test]
fn every_write_is_audited() {
    let (store, _, _) = fixture();
    let fleet = Me {
        device: Some("root-mac"),
        person: Some(person(&store, "root")),
        authority: Authority::Fleet,
    };
    run(
        &RealmAdminArgs {
            action: "create_group".into(),
            name: Some("eng".into()),
            ..Default::default()
        },
        &store,
        fleet,
    )
    .unwrap();
    let s = store.lock().unwrap();
    let ev = &s
        .admin_events(&AdminEventQuery {
            limit: 5,
            ..Default::default()
        })
        .unwrap()[0];
    assert_eq!(
        (
            ev.action.as_str(),
            ev.resource_type.as_str(),
            ev.resource_id.as_str()
        ),
        ("create_group", "group", "/eng")
    );
}

/// The self rule holds even when another fleet admin would remain (so the
/// store's last-holder guard would let it through), through a group too:
/// another fleet admin takes it away.
#[test]
fn a_second_fleet_admin_is_still_no_way_to_drop_your_own() {
    let (store, _, _) = fixture();
    let root = person(&store, "root");
    let fleet = Me {
        device: None,
        person: Some(root),
        authority: Authority::Fleet,
    };
    let mut map = args("map_role");
    map.user = Some("mo".into());
    map.role = Some("fleet-admin".into());
    run(&map, &store, fleet).unwrap();
    // root also holds it through a group, then leaves that group.
    let mut create = args("create_group");
    create.name = Some("ops".into());
    run(&create, &store, fleet).unwrap();
    let mut gmap = args("map_role");
    gmap.group = Some("/ops".into());
    gmap.role = Some("fleet-admin".into());
    run(&gmap, &store, fleet).unwrap();
    let mut join = args("add_member");
    join.group = Some("/ops".into());
    join.user = Some("root".into());
    run(&join, &store, fleet).unwrap();
    let mut unmap = args("unmap_role");
    unmap.user = Some("root".into());
    unmap.role = Some("fleet-admin".into());
    // Still held through /ops: the direct mapping may go.
    run(&unmap, &store, fleet).unwrap();
    let mut leave = args("remove_member");
    leave.group = Some("/ops".into());
    leave.user = Some("root".into());
    assert_eq!(code(run(&leave, &store, fleet)), codes::E_FORBIDDEN);
    let mut del = args("delete_group");
    del.group = Some("/ops".into());
    assert_eq!(code(run(&del, &store, fleet)), codes::E_FORBIDDEN);
    // mo takes it from root.
    let by_mo = Me {
        device: None,
        person: Some(person(&store, "mo")),
        authority: Authority::Fleet,
    };
    run(&leave, &store, by_mo).unwrap();
    assert!(!store.lock().unwrap().is_fleet_admin(root).unwrap());
}

/// An org admin: their own membership is not theirs to change, a fleet
/// admin's neither; users are their org's live members only, and what is
/// shown of them stays inside the org.
#[test]
fn an_org_admin_is_fenced_to_their_org() {
    let (store, acme, _) = fixture();
    let ann = person(&store, "ann");
    let me = Me {
        device: None,
        person: Some(ann),
        authority: Authority::Org {
            org: acme,
            hub: false,
        },
    };
    let mut leave = args("remove_member");
    leave.group = Some("/orgs/acme/admins".into());
    leave.user = Some("ann".into());
    assert!(run(&leave, &store, me).is_err(), "no lock-out");
    let users = run(&args("list_users"), &store, me).unwrap().to_string();
    assert!(users.contains("\"mo\"") && !users.contains("\"oz\"") && !users.contains("\"root\""));
    let mut get = args("get_user");
    get.user = Some("oz".into());
    assert_eq!(code(run(&get, &store, me)), codes::E_FORBIDDEN);
    for a in ["list_roles", "create_role", "disable_user", "update_user"] {
        let mut x = args(a);
        x.user = Some("mo".into());
        x.name = Some("r".into());
        assert_eq!(code(run(&x, &store, me)), codes::E_FORBIDDEN, "{a}");
    }
    // Removing mo from acme's members ends the membership.
    let mut rm = args("remove_member");
    rm.group = Some("/orgs/acme/members".into());
    rm.user = Some("mo".into());
    run(&rm, &store, me).unwrap();
    let mo = person(&store, "mo");
    assert_eq!(store.lock().unwrap().org_role(acme, mo).unwrap(), None);
}

#[test]
fn unknown_actions_and_disabling_yourself_are_refused() {
    let (store, _, _) = fixture();
    let root = person(&store, "root");
    let fleet = Me {
        device: None,
        person: Some(root),
        authority: Authority::Fleet,
    };
    assert_eq!(code(run(&args("nope"), &store, fleet)), codes::E_VALIDATE);
    let mut dis = args("disable_user");
    dis.user = Some("root".into());
    assert_eq!(code(run(&dis, &store, fleet)), codes::E_FORBIDDEN);
    dis.user = Some("mo".into());
    run(&dis, &store, fleet).unwrap();
    let mut en = args("enable_user");
    en.user = Some("mo".into());
    let v = run(&en, &store, fleet).unwrap();
    assert_eq!(v["enabled"], true);
}

#[test]
fn roles_composites_and_effective_roles() {
    let (store, _, _) = fixture();
    let fleet = Me {
        device: None,
        person: Some(person(&store, "root")),
        authority: Authority::Fleet,
    };
    let mut cr = args("create_role");
    cr.name = Some("deployer".into());
    cr.client = Some("fleet".into());
    run(&cr, &store, fleet).unwrap();
    let mut comp = args("add_composite");
    comp.role = Some("fleet:deployer".into());
    comp.child = Some("fleet-trusted".into());
    run(&comp, &store, fleet).unwrap();
    let mut map = args("map_role");
    map.user = Some("mo".into());
    map.role = Some("fleet:deployer".into());
    run(&map, &store, fleet).unwrap();
    let mut eff = args("effective_roles");
    eff.user = Some("mo".into());
    let v = run(&eff, &store, fleet).unwrap();
    let names: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"deployer")
            && names.contains(&"fleet-trusted")
            && names.contains(&"org-member"),
        "{v}"
    );
    let roles = run(&args("list_roles"), &store, fleet).unwrap();
    let dep = roles
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "deployer")
        .unwrap();
    assert_eq!(dep["composites"], serde_json::json!(["fleet-trusted"]));
    let mut del = args("delete_role");
    del.role = Some("fleet:deployer".into());
    run(&del, &store, fleet).unwrap();
}

/// A managed org group takes no role by hand, so an org admin can never
/// mint a fleet admin through `add_member`.
#[test]
fn a_managed_group_takes_no_role_and_org_admins_mint_no_fleet_admin() {
    let (store, acme, _) = fixture();
    let fleet = Me {
        device: None,
        person: Some(person(&store, "root")),
        authority: Authority::Fleet,
    };
    let mut map = args("map_role");
    map.group = Some("/orgs/acme/admins".into());
    map.role = Some("fleet-admin".into());
    assert_eq!(code(run(&map, &store, fleet)), codes::E_FORBIDDEN);
    let ann = Me {
        device: None,
        person: Some(person(&store, "ann")),
        authority: Authority::Org {
            org: acme,
            hub: false,
        },
    };
    let mut add = args("add_member");
    add.group = Some("/orgs/acme/admins".into());
    add.user = Some("mo".into());
    run(&add, &store, ann).unwrap();
    let mo = person(&store, "mo");
    assert!(!store.lock().unwrap().is_fleet_admin(mo).unwrap());
}

/// Spec §5's table for an org admin: every action, with arguments a fleet
/// admin could run, is refused outside their slots and narrowed reads.
#[test]
fn an_org_admin_is_refused_every_action_outside_their_slots() {
    let (store, acme, _) = fixture();
    {
        // Something for each fleet-only action to act on.
        let s = store.lock().unwrap();
        s.create_group(None, "eng", None).unwrap();
        s.create_role(None, "deployer", None).unwrap();
    }
    let ann = Me {
        device: None,
        person: Some(person(&store, "ann")),
        authority: Authority::Org {
            org: acme,
            hub: false,
        },
    };
    let full = |action: &str| -> RealmAdminArgs {
        let mut a = args(action);
        a.user = Some("mo".into());
        match action {
            "create_group" => a.name = Some("new".into()),
            "rename_group" => {
                a.group = Some("/eng".into());
                a.name = Some("eng2".into());
            }
            "move_group" | "delete_group" | "get_group" => a.group = Some("/eng".into()),
            "add_member" | "remove_member" => a.group = Some("/orgs/acme/viewers".into()),
            "create_role" => a.name = Some("ops".into()),
            "update_role" | "delete_role" => a.role = Some("deployer".into()),
            "add_composite" | "remove_composite" => {
                a.role = Some("deployer".into());
                a.child = Some("fleet-trusted".into());
            }
            "map_role" | "unmap_role" => a.role = Some("deployer".into()),
            "update_user" => a.profile = Some(serde_json::json!({ "first_name": "Mo" })),
            // A live member of acme throughout (mo leaves in remove_member).
            "get_user" | "effective_roles" => a.user = Some("ann".into()),
            _ => {}
        }
        a
    };
    let allowed_reads = ["list_groups", "list_users", "get_user", "effective_roles"];
    for (name, _) in ACTIONS {
        let r = run(&full(name), &store, ann);
        match name {
            n if allowed_reads.contains(&n) => assert!(r.is_ok(), "{n}: {r:?}"),
            // Allowed, but "/eng" is outside acme.
            "get_group" => assert_eq!(code(r), codes::E_FORBIDDEN, "get_group /eng"),
            "add_member" | "remove_member" => assert!(r.is_ok(), "{name}: {r:?}"),
            n => assert_eq!(code(r), codes::E_FORBIDDEN, "{n}"),
        }
    }
    let mut inside = args("get_group");
    inside.group = Some("/orgs/acme/admins".into());
    assert!(run(&inside, &store, ann).is_ok());
    for slot in ["admins", "members", "viewers"] {
        let mut a = args("add_member");
        a.group = Some(format!("/orgs/acme/{slot}"));
        a.user = Some("mo".into());
        assert!(run(&a, &store, ann).is_ok(), "{slot}");
    }
    let mut other = args("add_member");
    other.group = Some("/orgs/other/members".into());
    other.user = Some("mo".into());
    assert_eq!(code(run(&other, &store, ann)), codes::E_FORBIDDEN);
    let mut root = args("add_member");
    root.group = Some("/orgs/acme".into());
    root.user = Some("mo".into());
    assert_eq!(code(run(&root, &store, ann)), codes::E_FORBIDDEN);
}
