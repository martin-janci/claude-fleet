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

/// What migration 181 seeds for an org, built by hand: `/orgs/acme` and its
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

/// Nor mapped: a managed group takes no role by hand (it could never be
/// unmapped, and the org's admins would hand it out through membership);
/// the projection's `_in_tx(.., true)` path still seeds.
#[test]
fn a_managed_group_takes_no_role_mapping_publicly() {
    let s = s();
    let (root, viewers) = managed_org(&s);
    let fa = s.role(RoleRef::Realm(ROLE_FLEET_ADMIN)).unwrap().unwrap();
    for g in [root.id, viewers.id] {
        assert_eq!(
            s.map_role(Principal::Group(g), fa.id).unwrap_err().code,
            codes::E_FORBIDDEN
        );
    }
    let v = s
        .role(RoleRef::Client(CLIENT_FLEET, "org-viewer"))
        .unwrap()
        .unwrap();
    s.with_tx(|c| s.map_role_in_tx(c, Principal::Group(root.id), v.id, true))
        .unwrap();
    assert!(s.group_roles(root.id).unwrap().iter().any(|r| r.id == v.id));
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

/// Random writes through every writer that can change a membership
/// (membership set/remove, disable/enable, org removal and creation, group
/// create/move/member): the projected `org_members` always equals the direct
/// computation from the groups, and its `shares_since` follows the role.
#[test]
fn the_projection_equals_the_direct_computation() {
    use rand::{RngExt, SeedableRng};
    let s = s();
    let mut orgs: Vec<i64> = (0..3)
        .map(|i| s.add_org(&format!("o{i}"), None, false).unwrap().id)
        .collect();
    let people: Vec<i64> = (0..4)
        .map(|i| s.create_person(&format!("p{i}"), None).unwrap().id)
        .collect();
    let mut rng = rand::rngs::StdRng::seed_from_u64(7);
    let mut free_groups: Vec<i64> = Vec::new();
    for step in 0..600 {
        let o = orgs[rng.random_range(0..orgs.len())];
        let p = people[rng.random_range(0..people.len())];
        let enabled = s.get_person(p).unwrap().unwrap().disabled_at.is_none();
        match rng.random_range(0..9) {
            0 => {
                s.remove_org_member(o, p).unwrap();
            }
            1..=3 => {
                let role = ["admin", "member", "viewer"][rng.random_range(0..3)];
                // A disabled person may be refused a membership.
                let r = s.set_org_member(o, p, role, None);
                if enabled {
                    r.unwrap();
                }
            }
            4 => {
                if enabled {
                    s.disable_person(p).unwrap();
                } else {
                    s.enable_person(p).unwrap();
                }
            }
            5 => {
                // An unmanaged group, sometimes moved under another: it must
                // not change the projection.
                let g = s.create_group(None, &format!("g{step}"), None).unwrap().id;
                if let Some(&target) = free_groups.first() {
                    let _ = s.move_group(g, Some(target));
                }
                free_groups.push(g);
            }
            6 => {
                // The org may be refused removal; then it stays.
                if s.remove_org(o).is_ok() {
                    let n = s.add_org(&format!("o{step}"), None, false).unwrap().id;
                    orgs.retain(|x| *x != o);
                    orgs.push(n);
                }
            }
            7 => {
                // Membership of an unmanaged group moves nothing.
                if !free_groups.is_empty() {
                    let g = free_groups[rng.random_range(0..free_groups.len())];
                    s.with_tx(|c| s.add_group_member_in_tx(c, g, p, None))
                        .unwrap();
                }
            }
            _ => {
                let _ = s.set_org_member(o, p, "member", None);
            }
        }
        for &p in &people {
            let enabled = s.get_person(p).unwrap().unwrap().disabled_at.is_none();
            let live: Vec<_> = s
                .memberships_of(p)
                .unwrap()
                .into_iter()
                .filter(|m| m.removed_at.is_none())
                .collect();
            for m in &live {
                // Admins and members can receive org shares; viewers cannot.
                assert_eq!(
                    m.shares_since.is_some(),
                    matches!(m.role.as_str(), "admin" | "member"),
                    "step {step}: shares_since follows the role: {m:?}"
                );
            }
            let mut direct = s.org_roles_from_groups(p).unwrap();
            let mut projected: Vec<(i64, String)> =
                live.into_iter().map(|m| (m.org_id, m.role)).collect();
            direct.sort();
            projected.sort();
            if enabled {
                assert_eq!(direct, projected, "step {step}");
            } else {
                assert!(
                    direct.is_empty() && projected.is_empty(),
                    "step {step}: a disabled person holds nothing: {direct:?} / {projected:?}"
                );
            }
        }
    }
}

/// The production text of a source file: everything before the LAST
/// `#[cfg(test)]` that opens a module body (`mod name {`, optionally `pub` /
/// `pub(crate)`); a file with no such module is whole. A `#[cfg(test)]` on a
/// `use`, an item or a `mod name;` declaration stays in the text, so the
/// guard sees every production line (`store/mod.rs` has one on line 5).
fn production_text(text: &str) -> &str {
    let re =
        regex::Regex::new(r"(?s)#\[cfg\(test\)\]\s*(pub(\(crate\))?\s+)?mod\s+\w+\s*\{").unwrap();
    match re.find_iter(text).last() {
        Some(m) => &text[..m.start()],
        None => text,
    }
}

#[test]
fn production_text_cuts_at_the_last_test_module_body() {
    let src = "#[cfg(test)]\nuse x;\nfn prod() {}\n#[cfg(test)]\nmod decl;\nfn more() {}\n#[cfg(test)]\npub(crate) mod tests {\n  UPDATE org_members\n}\n";
    let p = production_text(src);
    assert!(p.contains("use x;") && p.contains("fn prod") && p.contains("fn more"));
    assert!(!p.contains("org_members"));
    assert_eq!(production_text("fn a() {}"), "fn a() {}");
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
    // Every spelling of a write, whitespace-insensitive, case-insensitive.
    let re = regex::Regex::new(
        r"(?i)\b(insert(\s+or\s+\w+)?\s+into|replace\s+into|update|delete\s+from)\s+org_members\b",
    )
    .unwrap();
    let mut offenders = Vec::new();
    for f in files {
        let rel = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if rel == "store/realm.rs"
            || rel.ends_with("/tests.rs")
            || rel.contains("/tests/")
            || rel.ends_with("_tests.rs")
        {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        let prod = production_text(&text);
        let flat = prod.split_whitespace().collect::<Vec<_>>().join(" ");
        if re.is_match(&flat) {
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

#[test]
fn role_ref_spells_a_client_role_with_its_client() {
    assert_eq!(role_ref(Some("fleet"), "org-admin"), "fleet:org-admin");
    assert_eq!(role_ref(None, "fleet-admin"), "fleet-admin");
}

#[test]
fn a_fence_keeps_a_role_only_while_one_via_is_inside() {
    let s = s();
    let org = s.add_org("acme", None, false).unwrap().id;
    let fence = s.org_group_fence(org).unwrap();
    let mut direct = vec!["direct".to_string()];
    assert!(fence.keep_role_vias(&mut direct));
    let mut outside = vec!["group:/elsewhere".to_string()];
    assert!(
        !fence.keep_role_vias(&mut outside),
        "every via was fenced out"
    );
    let mut mixed = vec![
        "group:/elsewhere".to_string(),
        "group:/orgs/acme/admins".to_string(),
    ];
    assert!(fence.keep_role_vias(&mut mixed));
    assert_eq!(mixed, vec!["group:/orgs/acme/admins".to_string()]);
    let mut none: Vec<String> = Vec::new();
    assert!(
        fence.keep_role_vias(&mut none),
        "a role with no via is kept, as before"
    );
}

/// A subtree moved under a deep parent must not push any group past the
/// depth cap: `list_groups` walks every path and would refuse the whole
/// realm.
#[test]
fn moving_a_tall_subtree_under_a_deep_parent_is_refused() {
    let s = s();
    // A chain of MAX_GROUP_DEPTH - 2 groups under the top.
    let mut deep = None;
    for i in 0..(MAX_GROUP_DEPTH - 2) {
        deep = Some(s.create_group(deep, &format!("d{i}"), None).unwrap().id);
    }
    // A subtree three groups tall.
    let a = s.create_group(None, "a", None).unwrap().id;
    let b = s.create_group(Some(a), "b", None).unwrap().id;
    let _c = s.create_group(Some(b), "c", None).unwrap().id;
    let err = s.move_group(a, deep).unwrap_err();
    assert_eq!(err.code, codes::E_VALIDATE);
    assert!(err.message.contains("too deep"), "{}", err.message);
    assert!(
        s.list_groups().is_ok(),
        "the listing still works after the refusal"
    );
    // The exact boundary: two tall under the 62-deep chain is 64, accepted.
    let p = s.create_group(None, "p", None).unwrap().id;
    let _q = s.create_group(Some(p), "q", None).unwrap().id;
    s.move_group(p, deep).unwrap();
    assert!(s.list_groups().is_ok());
    // One group tall fits where three did not.
    let lone = s.create_group(None, "lone", None).unwrap().id;
    s.move_group(lone, deep).unwrap();
    assert!(s.list_groups().is_ok());
}

/// An org named before `/` was refused has a root group `acme/eu` whose path
/// `/orgs/acme/eu` reads like group `eu` under org `acme`. The lookup must
/// never answer one for the other.
#[test]
fn group_by_path_never_answers_a_different_group_than_the_path_names() {
    let s = s();
    s.add_org("acme", None, false).unwrap();
    let acme_root = s.group_by_path("/orgs/acme").unwrap().unwrap();
    // A legacy root beside it, written as migration-era data would be.
    s.conn
        .execute(
            "INSERT INTO groups (uuid, name, parent_id, managed, org_id, created_at) \
             VALUES ('u-legacy', 'acme/eu', ?1, 1, NULL, 0)",
            rusqlite::params![acme_root.parent_id],
        )
        .unwrap();
    let legacy: i64 = s
        .conn
        .query_row("SELECT id FROM groups WHERE uuid = 'u-legacy'", [], |r| {
            r.get(0)
        })
        .unwrap();
    // The legacy root alone: its path string is `/orgs/acme/eu`, but no
    // segment walk reaches it, so the path answers nothing.
    assert_eq!(s.group(legacy).unwrap().unwrap().path, "/orgs/acme/eu");
    assert!(s.group_by_path("/orgs/acme/eu").unwrap().is_none());
    // Group `eu` under acme: the path strings now collide.
    // (`create_group` refuses a managed parent, so write the row directly.)
    s.conn
        .execute(
            "INSERT INTO groups (uuid, name, parent_id, managed, created_at) \
             VALUES ('u-eu', 'eu', ?1, 0, 0)",
            rusqlite::params![acme_root.id],
        )
        .unwrap();
    let eu = s.group_by_path("/orgs/acme/eu").unwrap().unwrap();
    assert_eq!(eu.path, "/orgs/acme/eu");
    assert_ne!(eu.id, legacy);
    let found = s.group_by_path("/orgs/acme/eu").unwrap().unwrap();
    assert_eq!(found.id, eu.id, "the segment walk finds the nested group");
    assert_eq!(found.path, "/orgs/acme/eu");
    // A trailing slash is not normalised away.
    assert!(s.group_by_path("/orgs/acme/eu/").unwrap().is_none());
}
