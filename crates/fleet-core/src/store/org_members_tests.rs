use super::*;

fn member(org: i64, person: i64, role: &str, added_at: i64, removed: bool) -> OrgMemberRow {
    OrgMemberRow {
        org_id: org,
        person_id: person,
        role: role.into(),
        added_at,
        added_by: None,
        shares_since: role_receives_shares(role).then_some(added_at),
        removed_at: removed.then_some(added_at + 1),
    }
}

const OWNER: Option<i64> = Some(1);

#[test]
fn the_owner_and_a_person_in_no_org_keep_their_binding() {
    let any = [member(5, 1, ROLE_VIEWER, 10, false)];
    for stored in [None, Some(5), Some(9)] {
        // The owner, whatever rows name them.
        assert_eq!(
            effective_device(Some(1), stored, OWNER, &any),
            DeviceOrg {
                org_id: stored,
                readonly: false
            }
        );
        // A colleague with no membership row: as before migration 107.
        assert_eq!(
            effective_device(Some(2), stored, OWNER, &any),
            DeviceOrg {
                org_id: stored,
                readonly: false
            }
        );
    }
    // A device bound to nobody is never resolved as the owner's.
    assert_eq!(
        effective_device(None, None, None, &any),
        DeviceOrg {
            org_id: None,
            readonly: false
        }
    );
}

#[test]
fn a_members_device_is_fenced_to_one_of_their_orgs_and_never_another() {
    let rows = [
        member(7, 2, ROLE_MEMBER, 20, false),
        member(5, 2, ROLE_ADMIN, 10, false),
        member(9, 2, ROLE_MEMBER, 5, true),
    ];
    // Unbound: their first live membership.
    assert_eq!(
        effective_device(Some(2), None, OWNER, &rows).org_id,
        Some(5)
    );
    // Bound to one of theirs: kept.
    assert_eq!(
        effective_device(Some(2), Some(7), OWNER, &rows).org_id,
        Some(7)
    );
    // Bound to an org they are not (or no longer) in: their first one.
    for stranger in [3, 9] {
        assert_eq!(
            effective_device(Some(2), Some(stranger), OWNER, &rows).org_id,
            Some(5)
        );
    }
}

#[test]
fn a_viewer_reads_only_and_a_former_member_reads_nothing() {
    let viewer = [member(5, 2, ROLE_VIEWER, 10, false)];
    assert_eq!(
        effective_device(Some(2), None, OWNER, &viewer),
        DeviceOrg {
            org_id: Some(5),
            readonly: true
        }
    );
    let gone = [member(5, 2, ROLE_ADMIN, 10, true)];
    for stored in [None, Some(5)] {
        assert_eq!(
            effective_device(Some(2), stored, OWNER, &gone),
            DeviceOrg {
                org_id: Some(NO_ORG),
                readonly: true
            }
        );
    }
}

#[test]
fn joining_changing_role_and_leaving_keep_shares_since_honest() {
    let s = Store::open_in_memory().unwrap();
    let org = s.add_org("Acme", None, false).unwrap().id;
    let jane = s.create_person("jane", None).unwrap().id;
    let epoch = s.auth_epoch().unwrap();
    let m = s.set_org_member(org, jane, "Viewer", None).unwrap();
    assert!(
        s.auth_epoch().unwrap() > epoch,
        "a membership moves the auth epoch"
    );
    assert_eq!((m.role.as_str(), m.shares_since), (ROLE_VIEWER, None));
    let m = s.set_org_member(org, jane, ROLE_MEMBER, None).unwrap();
    let since = m.shares_since.expect("a member receives shares from now");
    let m = s.set_org_member(org, jane, ROLE_ADMIN, None).unwrap();
    assert_eq!(
        m.shares_since,
        Some(since),
        "member → admin keeps when it started"
    );
    assert_eq!(s.org_role(org, jane).unwrap().as_deref(), Some(ROLE_ADMIN));
    assert!(s.remove_org_member(org, jane).unwrap());
    assert!(!s.remove_org_member(org, jane).unwrap(), "already gone");
    assert_eq!(s.org_role(org, jane).unwrap(), None);
    assert!(s.org_members(org).unwrap().is_empty());
    assert_eq!(s.memberships_of(jane).unwrap().len(), 1, "the row stays");
    // Re-joining is a new membership.
    let back = s.set_org_member(org, jane, ROLE_MEMBER, None).unwrap();
    assert!(back.is_live());
    assert!(s.set_org_member(org, jane, "owner", None).is_err());
    assert!(s.set_org_member(org + 99, jane, ROLE_MEMBER, None).is_err());
}

#[test]
fn the_auth_rows_carry_the_membership_and_the_listing_does_not() {
    let s = Store::open_in_memory().unwrap();
    let org = s.add_org("Acme", None, false).unwrap().id;
    let jane = s.create_person("jane", None).unwrap().id;
    s.insert_client_token("jane-phone", "h1", "full").unwrap();
    s.set_client_person("jane-phone", Some(jane)).unwrap();
    s.set_org_member(org, jane, ROLE_VIEWER, None).unwrap();
    let auth = s.auth_client_tokens().unwrap();
    let row = auth.iter().find(|r| r.name == "jane-phone").unwrap();
    assert_eq!((row.org_id, row.mode.as_str()), (Some(org), "readonly"));
    let stored = s.active_client_tokens().unwrap();
    let row = stored.iter().find(|r| r.name == "jane-phone").unwrap();
    assert_eq!((row.org_id, row.mode.as_str()), (None, "full"));
    let binding = s.client_token_binding(row.id).unwrap().unwrap();
    assert_eq!(binding.org_id, Some(org));
}

#[test]
fn removing_an_org_leaves_former_members_and_one_owner_of_the_hub() {
    let s = Store::open_in_memory().unwrap();
    let a = s.add_org("Acme", None, false).unwrap().id;
    let b = s.add_org("Beta", None, false).unwrap().id;
    let jane = s.create_person("jane", None).unwrap().id;
    s.set_org_member(a, jane, ROLE_MEMBER, None).unwrap();
    s.set_hub_owner_org(Some(a)).unwrap();
    s.set_hub_owner_org(Some(b)).unwrap();
    assert_eq!(s.hub_owner_org().unwrap(), Some(b));
    assert!(!s.get_org(a).unwrap().unwrap().owns_hub);
    assert!(s.remove_org(a).unwrap());
    let rows = s.memberships_of(jane).unwrap();
    assert!(rows.iter().all(|m| !m.is_live()), "{rows:?}");
    s.set_hub_owner_org(None).unwrap();
    assert_eq!(s.hub_owner_org().unwrap(), None);
    assert!(
        s.set_org_admins_see_unclaimed(b, true)
            .unwrap()
            .admins_see_unclaimed
    );
}
