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

// ---- Task 5: org_members is a projection of the org groups ----------------

#[test]
fn set_org_member_moves_the_person_between_the_orgs_subgroups() {
    let s = s();
    let org = s.add_org("acme", None, false).unwrap();
    let ada = s.create_person("ada", None).unwrap().id;
    s.set_org_member(org.id, ada, "viewer", None).unwrap();
    s.set_org_member(org.id, ada, "admin", None).unwrap();
    let paths: Vec<String> = s
        .groups_of(ada)
        .unwrap()
        .into_iter()
        .map(|g| g.path)
        .collect();
    assert_eq!(paths, ["/orgs/acme/admins"]);
    let m = s.org_member(org.id, ada).unwrap().unwrap();
    assert_eq!(m.role, "admin");
    assert!(
        m.shares_since.is_some(),
        "viewer → admin starts receiving shares"
    );
}

#[test]
fn the_projection_never_widens_an_existing_grant() {
    let s = s();
    let org = s.add_org("acme", None, false).unwrap();
    let ada = s.create_person("ada", None).unwrap().id;
    s.set_org_member(org.id, ada, "member", None).unwrap();
    let since = s.org_member(org.id, ada).unwrap().unwrap().shares_since;
    s.set_org_member(org.id, ada, "admin", None).unwrap();
    assert_eq!(
        s.org_member(org.id, ada).unwrap().unwrap().shares_since,
        since,
        "member → admin keeps shares_since"
    );
    s.remove_org_member(org.id, ada).unwrap();
    assert!(s
        .org_member(org.id, ada)
        .unwrap()
        .unwrap()
        .removed_at
        .is_some());
    assert!(s.groups_of(ada).unwrap().is_empty());
}

#[test]
fn org_rename_and_removal_follow_into_the_groups() {
    let s = s();
    let org = s.add_org("acme", None, false).unwrap();
    assert!(s.group_by_path("/orgs/acme/members").unwrap().is_some());
    s.update_org(org.id, Some("acme2"), None, None).unwrap();
    assert!(s.group_by_path("/orgs/acme2/members").unwrap().is_some());
    let ada = s.create_person("ada", None).unwrap().id;
    s.set_org_member(org.id, ada, "member", None).unwrap();
    s.remove_org(org.id).unwrap();
    assert!(s.group_by_path("/orgs/acme2").unwrap().is_none());
    assert!(s
        .org_member(org.id, ada)
        .unwrap()
        .unwrap()
        .removed_at
        .is_some());
}

#[test]
fn disabling_a_person_ends_every_group_membership_and_mapping() {
    let s = s();
    let org = s.add_org("acme", None, false).unwrap();
    let ada = s.create_person("ada", None).unwrap().id;
    s.set_org_member(org.id, ada, "member", None).unwrap();
    let t = s.role(RoleRef::Realm(ROLE_FLEET_TRUSTED)).unwrap().unwrap();
    s.map_role(Principal::Person(ada), t.id).unwrap();
    s.disable_person(ada).unwrap();
    assert!(s.groups_of(ada).unwrap().is_empty());
    assert!(s
        .org_member(org.id, ada)
        .unwrap()
        .unwrap()
        .removed_at
        .is_some());
    let mapped: i64 = s
        .conn
        .query_row(
            "SELECT COUNT(*) FROM user_role_mappings WHERE person_id = ?1",
            [ada],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(mapped, 0);
}

/// Random membership writes: the projected `org_members` always equals the
/// direct computation from the groups.
#[test]
fn the_projection_equals_the_direct_computation() {
    use rand::{RngExt, SeedableRng};
    let s = s();
    let orgs: Vec<i64> = (0..3)
        .map(|i| s.add_org(&format!("o{i}"), None, false).unwrap().id)
        .collect();
    let people: Vec<i64> = (0..4)
        .map(|i| s.create_person(&format!("p{i}"), None).unwrap().id)
        .collect();
    let mut rng = rand::rngs::StdRng::seed_from_u64(7);
    for _ in 0..300 {
        let (o, p) = (orgs[rng.random_range(0..3)], people[rng.random_range(0..4)]);
        match rng.random_range(0..4) {
            0 => {
                s.remove_org_member(o, p).unwrap();
            }
            n => {
                s.set_org_member(o, p, ["admin", "member", "viewer"][n - 1], None)
                    .unwrap();
            }
        }
        for &p in &people {
            let mut direct = s.org_roles_from_groups(p).unwrap();
            let mut projected: Vec<(i64, String)> = s
                .memberships_of(p)
                .unwrap()
                .into_iter()
                .filter(|m| m.removed_at.is_none())
                .map(|m| (m.org_id, m.role))
                .collect();
            direct.sort();
            projected.sort();
            assert_eq!(direct, projected);
        }
    }
}

/// Nothing but the projection writes org_members.
#[test]
fn only_the_projection_writes_org_members() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out)
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p)
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&root, &mut files);
    let writes = [
        "INSERT INTO org_members",
        "UPDATE org_members",
        "DELETE FROM org_members",
    ];
    let mut offenders = Vec::new();
    for f in files {
        let rel = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if rel == "store/realm.rs" || rel.contains("tests") {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        // Production code only: drop everything from the first `#[cfg(test)]`.
        let prod = text.split("#[cfg(test)]").next().unwrap();
        if writes.iter().any(|w| prod.contains(w)) {
            offenders.push(rel);
        }
    }
    assert!(
        offenders.is_empty(),
        "org_members written outside the projection: {offenders:?}"
    );
}

#[test]
fn the_owner_and_the_last_fleet_admin_keep_the_role() {
    let s = s();
    let owner = s.personal_owner_id().unwrap().unwrap();
    let fa = s.role(RoleRef::Realm(ROLE_FLEET_ADMIN)).unwrap().unwrap();
    assert_eq!(
        s.unmap_role(Principal::Person(owner), fa.id)
            .unwrap_err()
            .code,
        codes::E_FORBIDDEN
    );
    assert!(s.is_fleet_admin(owner).unwrap());
    assert_eq!(
        s.people_with_realm_role(ROLE_FLEET_ADMIN).unwrap(),
        vec![owner]
    );
}

/// A second fleet admin comes and goes freely while the owner holds the
/// role; `realm_sets` and `is_fleet_admin` follow the mappings.
#[test]
fn a_second_fleet_admin_is_granted_and_revoked() {
    let s = s();
    let owner = s.personal_owner_id().unwrap().unwrap();
    let ada = s.create_person("ada", None).unwrap().id;
    let fa = s.role(RoleRef::Realm(ROLE_FLEET_ADMIN)).unwrap().unwrap();
    assert!(!s.is_fleet_admin(ada).unwrap());
    let ops = s.create_group(None, "ops", None).unwrap();
    s.map_role(Principal::Group(ops.id), fa.id).unwrap();
    s.with_tx(|c| s.add_group_member_in_tx(c, ops.id, ada, None))
        .unwrap();
    assert!(s.is_fleet_admin(ada).unwrap());
    assert_eq!(s.realm_sets().unwrap().0, vec![owner, ada]);
    assert!(s
        .with_tx(|c| s.remove_group_member_in_tx(c, ops.id, ada))
        .unwrap());
    assert!(!s.is_fleet_admin(ada).unwrap());
    assert_eq!(s.realm_sets().unwrap().0, vec![owner]);
}

/// On a database whose owner holds no mapping (only the unconditional
/// union in `realm_sets`), the last LIVE holder cannot be unmapped, removed
/// from their group or disabled; nor can a hub be left with nobody.
#[test]
fn the_last_live_holder_cannot_drop_fleet_admin() {
    let s = s();
    let owner = s.personal_owner_id().unwrap().unwrap();
    let fa = s.role(RoleRef::Realm(ROLE_FLEET_ADMIN)).unwrap().unwrap();
    s.conn
        .execute(
            "DELETE FROM user_role_mappings WHERE person_id = ?1",
            rusqlite::params![owner],
        )
        .unwrap();
    // The owner is still a fleet admin, through the union.
    assert!(s.is_fleet_admin(owner).unwrap());
    assert_eq!(s.realm_sets().unwrap().0, vec![owner]);
    let ada = s.create_person("ada", None).unwrap().id;
    s.map_role(Principal::Person(ada), fa.id).unwrap();
    assert_eq!(
        s.unmap_role(Principal::Person(ada), fa.id)
            .unwrap_err()
            .code,
        codes::E_FORBIDDEN
    );
    assert_eq!(s.disable_person(ada).unwrap_err().code, codes::E_FORBIDDEN);
    assert!(s.is_fleet_admin(ada).unwrap(), "the refusal rolled back");
    assert!(s.get_person(ada).unwrap().unwrap().disabled_at.is_none());
    // A second holder makes the first one removable.
    let bo = s.create_person("bo", None).unwrap().id;
    s.map_role(Principal::Person(bo), fa.id).unwrap();
    assert!(s.unmap_role(Principal::Person(ada), fa.id).unwrap());
    assert_eq!(
        s.people_with_realm_role(ROLE_FLEET_ADMIN).unwrap(),
        vec![bo]
    );
}
