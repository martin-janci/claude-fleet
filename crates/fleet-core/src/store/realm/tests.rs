use super::*;
use crate::ipc_error::codes;
use crate::store::Store;

fn s() -> Store {
    Store::open_in_memory().unwrap()
}

#[test]
fn a_group_role_reaches_members_of_its_descendants_and_composites_expand() {
    let s = s();
    let ada = s.create_person("ada", None).unwrap().id;
    let eng = s.create_group(None, "eng", None).unwrap();
    let core = s.create_group(Some(eng.id), "core", None).unwrap();
    assert_eq!(core.path, "/eng/core");
    let deploy = s.create_role(None, "deployer", None).unwrap();
    let read = s.create_role(None, "reader", None).unwrap();
    s.add_composite(deploy.id, read.id).unwrap();
    s.map_role(Principal::Group(eng.id), deploy.id).unwrap();
    s.with_tx(|c| s.add_group_member_in_tx(c, core.id, ada, None))
        .unwrap();
    let names: Vec<String> = s
        .effective_roles(ada)
        .unwrap()
        .into_iter()
        .map(|e| e.role.name)
        .collect();
    assert!(
        names.contains(&"deployer".into()) && names.contains(&"reader".into()),
        "{names:?}"
    );
    assert_eq!(s.people_with_realm_role("reader").unwrap(), vec![ada]);
}

#[test]
fn a_composite_cycle_is_refused() {
    let s = s();
    let a = s.create_role(None, "a", None).unwrap();
    let b = s.create_role(None, "b", None).unwrap();
    s.add_composite(a.id, b.id).unwrap();
    assert_eq!(
        s.add_composite(b.id, a.id).unwrap_err().code,
        codes::E_VALIDATE
    );
}

#[test]
fn a_disabled_person_has_no_effective_roles() {
    let s = s();
    let ada = s.create_person("ada", None).unwrap().id;
    let r = s.role(RoleRef::Realm(ROLE_FLEET_TRUSTED)).unwrap().unwrap();
    s.map_role(Principal::Person(ada), r.id).unwrap();
    s.disable_person(ada).unwrap();
    assert!(s.effective_roles(ada).unwrap().is_empty());
    assert!(s
        .people_with_realm_role(ROLE_FLEET_TRUSTED)
        .unwrap()
        .is_empty());
}

#[test]
fn seeded_roles_and_managed_groups_cannot_be_deleted_or_restructured() {
    let s = s();
    let admin = s.role(RoleRef::Realm(ROLE_FLEET_ADMIN)).unwrap().unwrap();
    assert_eq!(
        s.delete_role(admin.id).unwrap_err().code,
        codes::E_FORBIDDEN
    );
    let orgs = s.group_by_path("/orgs").unwrap().unwrap();
    assert_eq!(
        s.delete_group(orgs.id).unwrap_err().code,
        codes::E_FORBIDDEN
    );
    assert_eq!(
        s.create_group(Some(orgs.id), "x", None).unwrap_err().code,
        codes::E_FORBIDDEN
    );
    assert_eq!(
        s.rename_group(orgs.id, "y").unwrap_err().code,
        codes::E_FORBIDDEN
    );
}

#[test]
fn org_roles_cannot_be_mapped_outside_an_orgs_managed_groups() {
    let s = s();
    let ada = s.create_person("ada", None).unwrap().id;
    let org_admin = s
        .role(RoleRef::Client(CLIENT_FLEET, "org-admin"))
        .unwrap()
        .unwrap();
    assert_eq!(
        s.map_role(Principal::Person(ada), org_admin.id)
            .unwrap_err()
            .code,
        codes::E_VALIDATE
    );
    let g = s.create_group(None, "eng", None).unwrap();
    assert_eq!(
        s.map_role(Principal::Group(g.id), org_admin.id)
            .unwrap_err()
            .code,
        codes::E_VALIDATE
    );
}

#[test]
fn a_group_cannot_move_under_itself() {
    let s = s();
    let a = s.create_group(None, "a", None).unwrap();
    let b = s.create_group(Some(a.id), "b", None).unwrap();
    assert_eq!(
        s.move_group(a.id, Some(b.id)).unwrap_err().code,
        codes::E_VALIDATE
    );
}

#[test]
fn every_realm_write_moves_the_auth_epoch() {
    let s = s();
    let e0 = s.auth_epoch().unwrap();
    s.create_group(None, "eng", None).unwrap();
    assert!(s.auth_epoch().unwrap() > e0);
}

/// What migration 178 seeds for an org, built by hand: `/orgs/acme` and its
/// `viewers` subgroup, managed, with viewers mapped to org-viewer.
fn managed_org(s: &Store) -> (GroupRow, GroupRow) {
    s.with_tx(|c| {
        let orgs = s.group_by_path("/orgs")?.unwrap();
        let root = s.create_group_in_tx(c, Some(orgs.id), "acme", true, Some(9001), None)?;
        let viewers = s.create_group_in_tx(c, Some(root.id), "viewers", true, None, None)?;
        let v = s
            .role(RoleRef::Client(CLIENT_FLEET, "org-viewer"))?
            .unwrap();
        s.map_role_in_tx(c, Principal::Group(viewers.id), v.id, true)?;
        Ok((root, viewers))
    })
    .unwrap()
}

#[test]
fn the_public_map_role_never_grants_an_org_role() {
    let s = s();
    let (root, viewers) = managed_org(&s);
    let admin = s
        .role(RoleRef::Client(CLIENT_FLEET, "org-admin"))
        .unwrap()
        .unwrap();
    for g in [root.id, viewers.id] {
        assert_eq!(
            s.map_role(Principal::Group(g), admin.id).unwrap_err().code,
            codes::E_VALIDATE
        );
    }
}

#[test]
fn a_managed_groups_mappings_cannot_be_unmapped_publicly() {
    let s = s();
    let (_, viewers) = managed_org(&s);
    let v = s
        .role(RoleRef::Client(CLIENT_FLEET, "org-viewer"))
        .unwrap()
        .unwrap();
    assert_eq!(
        s.unmap_role(Principal::Group(viewers.id), v.id)
            .unwrap_err()
            .code,
        codes::E_FORBIDDEN
    );
    assert!(s
        .with_tx(|c| s.unmap_role_in_tx(c, Principal::Group(viewers.id), v.id, true))
        .unwrap());
}

#[test]
fn the_seeded_org_chain_cannot_be_taken_apart() {
    let s = s();
    let a = s
        .role(RoleRef::Client(CLIENT_FLEET, "org-admin"))
        .unwrap()
        .unwrap();
    let m = s
        .role(RoleRef::Client(CLIENT_FLEET, "org-member"))
        .unwrap()
        .unwrap();
    assert_eq!(
        s.remove_composite(a.id, m.id).unwrap_err().code,
        codes::E_FORBIDDEN
    );
}

#[test]
fn composites_cannot_carry_org_roles_in_or_out_of_the_org_model() {
    let s = s();
    let custom = s.create_role(None, "custom", None).unwrap();
    let org_admin = s
        .role(RoleRef::Client(CLIENT_FLEET, "org-admin"))
        .unwrap()
        .unwrap();
    let org_viewer = s
        .role(RoleRef::Client(CLIENT_FLEET, "org-viewer"))
        .unwrap()
        .unwrap();
    let fleet_admin = s.role(RoleRef::Realm(ROLE_FLEET_ADMIN)).unwrap().unwrap();
    assert_eq!(
        s.add_composite(custom.id, org_admin.id).unwrap_err().code,
        codes::E_VALIDATE
    );
    assert_eq!(
        s.add_composite(org_viewer.id, fleet_admin.id)
            .unwrap_err()
            .code,
        codes::E_VALIDATE
    );
    // The seeded direction stays legal.
    s.add_composite(org_admin.id, org_viewer.id).unwrap();
}
