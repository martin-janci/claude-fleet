//! The person half of a session read (multi-user M1, task T6). The org
//! half's table lives beside `OrgScope`, in `service/orgs_tests.rs`.

use super::*;
use crate::mcp::auth::{Caller, ClientRef, TokenMode};
use crate::store::{GrantRecipient, SessionRow, Store, GRANT_DRIVE, GRANT_WATCH};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A row with just the two fields the person rules read, plus an id and a
/// host so the host clauses have something to compare.
fn row(id: i64, host: &str, owner: Option<i64>, visibility: &str) -> SessionRow {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host(host).unwrap();
    let real = s
        .upsert_session("dev", host, None, None, 1, 1, "running", None)
        .unwrap();
    let mut r = s.get_session_by_id(real).unwrap().unwrap();
    r.id = id;
    r.owner_person_id = owner;
    r.visibility = visibility.to_string();
    r
}

/// A person's scope: no host, no pane, the grants given.
fn person_scope(person: Option<i64>, grants: &[(i64, &str)], sole: bool) -> ViewScope {
    let map: BTreeMap<i64, String> = grants
        .iter()
        .map(|(id, lvl)| (*id, (*lvl).to_string()))
        .collect();
    ViewScope::for_caller(
        OrgScope::All,
        person,
        GrantSet::from_map(map),
        None,
        None,
        sole,
        UnclaimedReach::None,
        TeamReach::default(),
    )
}

/// A per-host token's scope, with or without a proven pane.
fn host_scope(alias: &str, proven: Option<i64>) -> ViewScope {
    ViewScope::for_caller(
        OrgScope::Host {
            alias: alias.into(),
            org: None,
            isolated: Default::default(),
        },
        None,
        GrantSet::default(),
        Some(alias.into()),
        proven,
        false,
        UnclaimedReach::None,
        TeamReach::default(),
    )
}

/// **`with_org` is not a no-op on the predicate the milestone composes.**
///
/// `sees_session_facts` opened with `if self.internal { return RowAndContent }`
/// ABOVE its org clause, so a scope that was internal AND narrowed —
/// `work::nudge`'s hook reader, `work::today`'s per-host reader,
/// `work::resume`'s landing-host reader, and every `org_only_view` test —
/// answered `RowAndContent` for every row in the fleet. The narrowing was
/// accepted, stored, and then ignored by the one predicate every session read
/// goes through.
///
/// Three claims: the narrowing BITES (a row on another host is refused), it
/// bites through the verbs built on the predicate (`may_drive`, `may_own`),
/// and the hub's own UNnarrowed reader is untouched — `OrgScope::All` passes
/// the org clause trivially, which is what makes putting it first free.
/// M15 step G2.10: an org whose members see each other's sessions lets a
/// teammate WATCH a session of the org owned by another member — never
/// answer, drive or own it, never another org's, never a non-member's,
/// never an unclaimed row.
#[test]
fn a_teammate_watches_and_nothing_more() {
    let team: BTreeMap<i64, std::collections::BTreeSet<i64>> =
        [(10, [2].into_iter().collect())].into_iter().collect();
    let me = ViewScope::for_caller(
        OrgScope::All,
        Some(1),
        GrantSet::default(),
        None,
        None,
        false,
        UnclaimedReach::None,
        TeamReach::from_map(team),
    );
    let in_org = |id, owner: Option<i64>, org: Option<i64>, vis: &str| {
        let mut r = row(id, "h", owner, vis);
        r.org_id = org;
        r
    };
    let teammates = in_org(1, Some(2), Some(10), "private");
    assert!(me.sees_session_row(&teammates).is_visible());
    assert!(me.watches_as_teammate(&teammates));
    assert!(!me.may_answer(&teammates));
    assert!(!me.may_drive(&teammates));
    assert!(!me.may_own(&teammates));
    // Another org, a non-member's session, an unclaimed one: nothing.
    assert!(!me
        .sees_session_row(&in_org(2, Some(2), Some(11), "private"))
        .is_visible());
    assert!(!me
        .sees_session_row(&in_org(3, Some(3), Some(10), "private"))
        .is_visible());
    assert!(!me
        .sees_session_row(&in_org(4, None, Some(10), "unclaimed"))
        .is_visible());
    // The switch on (no team reach): the teammate's row is private again.
    assert!(!person_scope(Some(1), &[], false)
        .sees_session_row(&teammates)
        .is_visible());
}

#[test]
fn a_narrowed_hub_reader_is_still_fenced_by_its_org() {
    let mine = row(1, "alpha", None, crate::store::VISIBILITY_UNCLAIMED);
    let theirs = row(2, "beta", None, crate::store::VISIBILITY_UNCLAIMED);
    let narrowed = ViewScope::internal().with_org(OrgScope::Host {
        alias: "alpha".into(),
        org: None,
        isolated: Default::default(),
    });

    assert_eq!(narrowed.sees_session_row(&mine), Visibility::RowAndContent);
    assert_eq!(
        narrowed.sees_session_row(&theirs),
        Visibility::None,
        "a hub reader narrowed to alpha must not read beta's rows: `with_org` \
         asked for exactly this and the internal clause used to swallow it"
    );
    assert!(narrowed.may_drive(&mine) && !narrowed.may_drive(&theirs));
    assert!(narrowed.may_own(&mine) && !narrowed.may_own(&theirs));
    assert!(
        narrowed.is_internal() && !narrowed.is_unrestricted(),
        "it is still the hub's own reader — the PERSON fence is off — and it \
         is no longer unrestricted, which is the distinction every \
         fence-skipping early return has to use"
    );

    // The hub's own reader, not narrowed: unchanged, and the one scope for
    // which skipping a fence outright is the whole truth.
    let hub = ViewScope::internal();
    for r in [&mine, &theirs] {
        assert_eq!(hub.sees_session_row(r), Visibility::RowAndContent);
        assert!(hub.may_drive(r) && hub.may_own(r));
    }
    assert!(hub.is_unrestricted());
}

/// The same fence on the PAST-conversation predicate, which had the same
/// shape: `if self.internal { return Ok(true) }` above everything, including
/// the surviving-row arm that can be judged.
///
/// Now the rows decide first (through `may_own`, hence through the org
/// boundary) and `internal` answers only where there is no row — and
/// therefore no host and no org — to judge, which is the case the hub's own
/// readers genuinely need.
#[test]
fn a_narrowed_hub_reader_cannot_read_another_orgs_past_conversation() {
    const CID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("alpha").unwrap();
    s.upsert_host("beta").unwrap();
    let on_beta = s
        .upsert_session("dev-beta", "beta", None, None, 1, 1, "running", None)
        .unwrap();
    s.set_claude_session_id(on_beta, CID).unwrap();

    let narrowed = ViewScope::internal().with_org(OrgScope::Host {
        alias: "alpha".into(),
        org: None,
        isolated: Default::default(),
    });
    assert!(
        !narrowed.sees_past_conversation(&s, CID).unwrap(),
        "the conversation ran on beta and this reader is alpha's"
    );
    assert!(
        ViewScope::internal()
            .sees_past_conversation(&s, CID)
            .unwrap(),
        "the hub's own unnarrowed reader still reads it"
    );
    // No row at all: nothing to judge, and the hub's readers must still
    // reach it (a reaped session's transcript, a summary, the GC).
    assert!(narrowed.sees_past_conversation(&s, "never-seen").unwrap());
}

/// **The NULL-equality trap, pinned by name and placed first.**
///
/// An `unclaimed` row has `owner_person_id = None`; a caller that proves no
/// person has `person = None`; and `None == None` is `true`. Written as the
/// prose suggests — `row.owner_person_id == self.person` — every person-less
/// caller would own every unclaimed row, and by invariant 1 could grant on
/// all of them. The first row of this table is that case, and the answer is
/// no access.
#[test]
fn a_person_less_caller_owns_nothing_and_sees_nothing() {
    // (owner, person, want visible, want owns)
    let table = [
        // THE trap. Both `None`, and the answer is still no.
        (None, None, Visibility::None, false),
        // A row nobody owns, and a real person asking: still not theirs.
        // (The single-person carve-out is a separate scope — below.)
        (None, Some(7), Visibility::None, false),
        // An owned row and a caller who proves nobody.
        (Some(7), None, Visibility::None, false),
        // The owner.
        (Some(7), Some(7), Visibility::RowAndContent, true),
        // Somebody else.
        (Some(7), Some(8), Visibility::None, false),
    ];
    for (owner, person, want_see, want_own) in table {
        let r = row(1, "h-a", owner, crate::store::VISIBILITY_PRIVATE);
        let scope = person_scope(person, &[], false);
        assert_eq!(
            scope.sees_session_row(&r),
            want_see,
            "owner={owner:?} person={person:?}"
        );
        assert_eq!(
            scope.owns(&r),
            want_own,
            "owner={owner:?} person={person:?}"
        );
    }
}

/// An `unclaimed` row is visible to the ONE person on a single-person hub,
/// and to nobody else. Without this the upgrade empties the sidebar's
/// Outside-fleet and orphan sections on every standalone desktop: every row
/// there has `started_at IS NULL`, which is exactly the population T3's
/// backfill leaves `unclaimed`. The person saw those rows yesterday, so
/// serving them is rule 7 (the upgrade widens nothing) rather than an
/// exception to it.
#[test]
fn an_unclaimed_row_reaches_the_one_person_on_the_hub_and_nobody_else() {
    let r = row(1, "h-a", None, crate::store::VISIBILITY_UNCLAIMED);
    assert_eq!(
        person_scope(Some(7), &[], true).sees_session_row(&r),
        Visibility::RowAndContent,
        "the only person on the hub"
    );
    assert_eq!(
        person_scope(Some(7), &[], false).sees_session_row(&r),
        Visibility::None,
        "one of several people"
    );
    assert_eq!(
        person_scope(None, &[], false).sees_session_row(&r),
        Visibility::None,
        "a caller that proves no person"
    );
}

/// A grant is the only way a second person reaches a row, and the level
/// decides what they may do with it — never whether they may see it.
#[test]
fn a_grant_lets_a_second_person_in_at_exactly_its_level() {
    let r = row(42, "h-a", Some(7), crate::store::VISIBILITY_PRIVATE);
    let watcher = person_scope(Some(8), &[(42, GRANT_WATCH)], false);
    let driver = person_scope(Some(8), &[(42, GRANT_DRIVE)], false);
    let stranger = person_scope(Some(9), &[], false);
    let owner = person_scope(Some(7), &[], false);

    assert_eq!(watcher.sees_session_row(&r), Visibility::RowAndContent);
    assert_eq!(driver.sees_session_row(&r), Visibility::RowAndContent);
    assert_eq!(stranger.sees_session_row(&r), Visibility::None);

    assert!(!watcher.may_drive(&r), "watch never confers drive");
    assert!(driver.may_drive(&r));
    assert!(owner.may_drive(&r));
    assert!(!stranger.may_drive(&r));

    // `own` is a tier, not a level: no grant reaches it.
    assert!(!watcher.may_own(&r));
    assert!(!driver.may_own(&r), "drive is not own");
    assert!(owner.may_own(&r));
    // A grant on ANOTHER row is not a grant on this one.
    let other = person_scope(Some(8), &[(43, GRANT_DRIVE)], false);
    assert_eq!(other.sees_session_row(&r), Visibility::None);
}

/// §4.4's two clauses, and nothing else. A per-host token is a machine, not
/// a person: it reaches the `unclaimed` rows on its own host (which is what
/// makes claiming reachable at all) and the one row whose pane this request
/// proves.
#[test]
fn a_host_token_sees_unclaimed_rows_on_its_host_and_the_row_its_pane_proves() {
    let mine_unclaimed = row(1, "h-a", None, crate::store::VISIBILITY_UNCLAIMED);
    let mine_private = row(2, "h-a", Some(7), crate::store::VISIBILITY_PRIVATE);
    let mine_private_2 = row(3, "h-a", Some(7), crate::store::VISIBILITY_PRIVATE);
    let theirs_unclaimed = row(4, "h-b", None, crate::store::VISIBILITY_UNCLAIMED);
    let theirs_private = row(5, "h-b", Some(7), crate::store::VISIBILITY_PRIVATE);

    // No pane header: an agent on a host provisioned before M1, or one
    // outside tmux. It proves no pane, so it reaches no private row.
    let bare = host_scope("h-a", None);
    assert_eq!(
        bare.sees_session_row(&mine_unclaimed),
        Visibility::RowAndContent
    );
    assert_eq!(bare.sees_session_row(&mine_private), Visibility::None);
    assert_eq!(
        bare.sees_session_row(&theirs_unclaimed),
        Visibility::None,
        "an unassigned row on ANOTHER host is still another host's"
    );
    assert_eq!(bare.sees_session_row(&theirs_private), Visibility::None);

    // The pane of row 2, resolved by `find_session_by_pane` at scope
    // construction: that row and no other private row.
    let proven = host_scope("h-a", Some(2));
    assert_eq!(
        proven.sees_session_row(&mine_private),
        Visibility::RowAndContent
    );
    assert_eq!(
        proven.sees_session_row(&mine_private_2),
        Visibility::None,
        "one row, never a set"
    );
    assert_eq!(proven.sees_session_row(&theirs_private), Visibility::None);
    // The agent in its own pane may drive its own session — refusing it
    // would break `send_message`, `dispatch_task` and `work_link` for every
    // fleet-started session. It is still not the owner.
    assert!(proven.may_drive(&mine_private));
    assert!(!proven.may_own(&mine_private));
    assert!(!proven.owns(&mine_private));
}

/// The org boundary is composed, never replaced: a bound client that may not
/// read another org's work may not read its sessions either, whoever owns
/// them — and a host token stays on its own host even for a row it owns
/// nothing of.
#[test]
fn the_org_fence_still_runs_under_the_person_fence() {
    let r = row(1, "h-b", Some(7), crate::store::VISIBILITY_PRIVATE);
    let mut bound = person_scope(Some(7), &[], false);
    bound.org = OrgScope::Org {
        org: 1,
        sees_unassigned: false,
    };
    assert_eq!(
        bound.sees_session_row(&r),
        Visibility::None,
        "the row's org is unassigned and D31 is off, so the org half refuses \
         before ownership is even asked"
    );
    bound.org = OrgScope::Org {
        org: 1,
        sees_unassigned: true,
    };
    assert_eq!(bound.sees_session_row(&r), Visibility::RowAndContent);
}

/// The hub's own readers — GC, reconcile, the playbooks, attention,
/// `fleet_health` — see everything, and that value is NOT the same value as
/// a token that names nobody. Before M1 they were one (`OrgScope::All`),
/// which is exactly how a person-less token would have inherited the
/// master's reach.
#[test]
fn an_internal_scope_is_not_the_same_value_as_a_person_less_one() {
    let r = row(1, "h-a", Some(7), crate::store::VISIBILITY_PRIVATE);
    let internal = ViewScope::internal();
    let nobody = person_scope(None, &[], false);
    assert_eq!(internal.sees_session_row(&r), Visibility::RowAndContent);
    assert_eq!(nobody.sees_session_row(&r), Visibility::None);
    assert_ne!(internal, nobody, "two different values, not one");
    assert!(internal.may_drive(&r) && internal.may_own(&r));
    // It is still not an OWNER: a grant is the owner's to make, and "the GC
    // owns every session" must not be a sentence anybody can write.
    assert!(!internal.owns(&r));
}

/// R6-l: the same rule, written at the two layers that each need it.
///
/// `ViewScope::owns` compares a `SessionRow` in `service/`;
/// `store/session_grants.rs` compares the `sessions.owner_person_id` column
/// inside its own SQL, because `store/` does not import `service/` and the
/// dependency runs the other way. Two implementations of one rule is a
/// drift risk, so this test asks both about the same three rows and insists
/// they agree. The store's answer is read through the behaviour that
/// depends on it — only the owner may create a grant.
#[test]
fn view_scope_owns_and_the_stores_column_comparison_agree() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h-a").unwrap();
    let ada = s.create_person("ada", None).unwrap().id;
    let bob = s.create_person("bob", None).unwrap().id;
    let cleo = s.create_person("cleo", None).unwrap().id;

    let owned = s
        .upsert_session("owned", "h-a", None, None, 1, 1, "running", None)
        .unwrap();
    s.claim_if_unclaimed(owned, Some(ada)).unwrap();
    let ownerless = s
        .upsert_session("ownerless", "h-a", None, None, 1, 1, "running", None)
        .unwrap();

    // The owner, a non-owner, and an ownerless row for anyone at all.
    for (session_id, asking) in [
        (owned, ada),
        (owned, bob),
        (ownerless, ada),
        (ownerless, bob),
    ] {
        let r = s.get_session_by_id(session_id).unwrap().unwrap();
        let service_says = person_scope(Some(asking), &[], false).owns(&r);
        // The store's answer, read through the rule it enforces: the grant
        // lands only when the granter owns the row. `cleo` is the recipient
        // throughout, and never the granter, so nothing else can decide it.
        let store_says = s
            .grant_session(
                session_id,
                GrantRecipient::Person(cleo),
                GRANT_WATCH,
                asking,
            )
            .is_ok();
        if store_says {
            s.revoke_session_grant(session_id, cleo, asking).unwrap();
        }
        assert_eq!(
            service_says, store_says,
            "session {session_id}, person {asking}: ViewScope::owns said \
             {service_says}, the store's column comparison said {store_says}"
        );
    }
}

/// Revision 3 promised this and `mcp/tools/fleet.rs::usage_report` is the
/// org scope's standing counter-example: a second site that builds a scope
/// is a second definition of who the caller is, and the one that is not
/// `Caller::view_scope` is the one nobody will update.
///
/// The compiler already carries most of the weight — `ViewScope` has
/// private fields, so a struct literal for it cannot be written outside
/// `view_scope.rs` — and this test carries the rest: the two constructors
/// are named, and only the two modules entitled to them may call them.
#[test]
fn only_caller_view_scope_constructs_a_view_scope() {
    let mut files = Vec::new();
    rs_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(files.len() > 100, "the walk found nothing: {}", files.len());

    let mut offenders = Vec::new();
    for f in &files {
        let rel = f
            .strip_prefix(env!("CARGO_MANIFEST_DIR"))
            .unwrap_or(f)
            .to_string_lossy()
            .to_string();
        // This module defines them; `mcp/auth.rs` holds the one constructor
        // from a request. Everything else is a second definition of who the
        // caller is.
        if rel.ends_with("view_scope.rs") || rel.ends_with("view_scope_tests.rs") {
            continue;
        }
        let src = std::fs::read_to_string(f).expect("read source");
        let code = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        if code.contains("ViewScope::for_caller") && !rel.ends_with("auth.rs") {
            offenders.push(format!("{rel}: calls ViewScope::for_caller"));
        }
    }
    assert!(
        offenders.is_empty(),
        "only Caller::view_scope (mcp/auth.rs) may build a ViewScope from a \
         caller; ViewScope::internal is the hub's own reader and is \
         deliberately unrestricted:\n{}",
        offenders.join("\n")
    );
}

/// `Caller::view_scope` is the one constructor, and the table in its doc
/// comment is what this asserts: a master resolves to the hub's personal
/// owner, a device to its own person, a device bound to nobody to NOBODY,
/// and a per-host token to no person at all.
#[test]
fn caller_view_scope_follows_the_table() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h-a").unwrap();
    let owner = s.personal_owner_id().unwrap().expect("096 mints one");

    let master = Caller::master().view_scope(&s).unwrap();
    assert_eq!(master.person, Some(owner));
    assert!(master.is_sole_person(), "one person on a fresh hub");
    assert!(
        !master.is_internal(),
        "a caller is never the hub's own reader"
    );

    let device = |person: Option<i64>| Caller {
        api: None,
        host_alias: None,
        client: Some(ClientRef {
            id: 1,
            name: "phone".into(),
            trusted: false,
            org_id: None,
            person_id: person,
        }),
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: person == Some(owner),
    };
    let own_device = device(Some(owner)).view_scope(&s).unwrap();
    assert_eq!(own_device.person, Some(owner));
    // A device the backfill never reached and no pairing bound: nobody.
    let unbound = device(None).view_scope(&s).unwrap();
    assert_eq!(unbound.person, None);
    assert!(!unbound.is_sole_person(), "nobody is never the sole person");
    let r = row(1, "h-a", Some(owner), crate::store::VISIBILITY_PRIVATE);
    assert_eq!(
        unbound.sees_session_row(&r),
        Visibility::None,
        "a token that names no person refuses; it does not widen"
    );

    let host = Caller {
        api: None,
        host_alias: Some("h-a".into()),
        client: None,
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
    }
    .view_scope(&s)
    .unwrap();
    assert_eq!(host.person, None, "a machine is not a person");
    assert_eq!(host.host.as_deref(), Some("h-a"));
    assert_eq!(host.proven_session, None, "no header, no proof");

    // A second person: the hub stops being a single-person install for
    // everybody, the master included.
    s.create_person("bob", None).unwrap();
    assert!(!Caller::master().view_scope(&s).unwrap().is_sole_person());
}

/// The pane proof is resolved once, at scope construction, from the header
/// the connection carries — never from an argument the caller chose. A
/// header that matches no row is simply no proof.
#[test]
fn the_pane_header_resolves_to_one_row_or_to_none() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h-a").unwrap();
    let id = s
        .upsert_session("dev", "h-a", None, None, 1, 1, "running", None)
        .unwrap();
    s.conn_for_test()
        .execute(
            "UPDATE sessions SET tmux_pane_id = '%17' WHERE id = ?1",
            [id],
        )
        .unwrap();
    let with_pane = |pane: Option<&str>| {
        Caller {
            api: None,
            host_alias: Some("h-a".into()),
            client: None,
            mode: TokenMode::Full,
            pane: pane.map(str::to_string),
            is_personal_owner: false,
        }
        .view_scope(&s)
        .unwrap()
        .proven_session
    };
    assert_eq!(with_pane(Some("%17")), Some(id));
    assert_eq!(with_pane(Some("%99")), None, "a pane no row carries");
    assert_eq!(with_pane(None), None);
    // The same pane id on another host proves nothing here: the lookup is
    // fenced on the token's own host.
    s.upsert_host("h-b").unwrap();
    let other = Caller {
        api: None,
        host_alias: Some("h-b".into()),
        client: None,
        mode: TokenMode::Full,
        pane: Some("%17".into()),
        is_personal_owner: false,
    }
    .view_scope(&s)
    .unwrap();
    assert_eq!(other.proven_session, None);
}

/// A `GrantSet` is `BTreeMap`-backed so two scopes built from the same
/// grants compare equal — `mcp/events_route.rs` drops a stream whose scope
/// moved, and a set with no canonical order would drop live streams for
/// nothing.
#[test]
fn two_scopes_over_the_same_grants_compare_equal() {
    let a = person_scope(Some(7), &[(1, GRANT_WATCH), (2, GRANT_DRIVE)], false);
    let b = person_scope(Some(7), &[(2, GRANT_DRIVE), (1, GRANT_WATCH)], false);
    assert_eq!(a, b);
    let c = person_scope(Some(7), &[(1, GRANT_DRIVE), (2, GRANT_DRIVE)], false);
    assert_ne!(a, c, "a narrowed grant moves the scope");
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
}

// ---- the result gate's session half (T8) ----------------------------------
//
// The walker, pure: what `sees` answers is injected, so these tests say what
// the SHAPE of a result does to a row and nothing about where the judgement
// comes from. The judgement over a real store is
// `mcp/tools/support.rs::visibility_resolver`, exercised through the gate in
// `mcp/tools/tests.rs`.

/// `sees`, as a list of the ids that may be seen. Anything else — including
/// a row-shaped object with no `id` at all — is refused, which is the
/// production resolver's shape too.
fn visible_ids(
    ids: &[i64],
) -> impl Fn(&serde_json::Map<String, serde_json::Value>) -> Visibility + '_ {
    move |m| match m.get("id").and_then(serde_json::Value::as_i64) {
        Some(id) if ids.contains(&id) => Visibility::RowAndContent,
        _ => Visibility::None,
    }
}

/// One full row, as `list_sessions { summary: false }` serialises it: the
/// `visibility` key is what makes it recognisable on its own.
fn row_json(id: i64) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "tmux_name": format!("dev-{id}"),
        "host_alias": "h",
        "visibility": "private",
        "last_prompt": "the private thing I am working on",
    })
}

/// An array loses the row; a named field keeps its name and answers `null`.
///
/// The two are not a style choice. A list with a `null` in it breaks every
/// consumer that deserialises `Vec<SessionRow>` — the desktop and the phone
/// both do — while a field that simply vanished changes the shape of the
/// answer the tool documents. "Drop, never blank" is about the row's
/// CONTENT: there is no blanked row either way.
#[test]
fn an_invisible_row_leaves_an_array_and_nulls_a_field() {
    let scope = person_scope(Some(7), &[], false);
    let sees = visible_ids(&[1]);
    let mut v = serde_json::json!({
        "sessions": [row_json(1), row_json(2)],
        "controller": row_json(2),
        "count": 2,
    });
    scope.drop_invisible_rows(&mut v, &sees);
    assert_eq!(
        v["sessions"].as_array().map(Vec::len),
        Some(1),
        "the array is a shorter array: {v}"
    );
    assert_eq!(v["sessions"][0]["id"].as_i64(), Some(1));
    assert!(v["controller"].is_null(), "{v}");
    assert_eq!(v["count"].as_i64(), Some(2), "nothing else is touched");
    assert!(
        !serde_json::to_string(&v).unwrap().contains("dev-2"),
        "not one field of the refused row survives: {v}"
    );
}

/// A tool whose whole answer IS one row answers `null` rather than the row —
/// the root is walked like any other position.
#[test]
fn a_result_that_is_one_invisible_row_answers_null() {
    let scope = person_scope(Some(7), &[], false);
    let mut mine = row_json(1);
    scope.drop_invisible_rows(&mut mine, &visible_ids(&[1]));
    assert_eq!(mine["id"].as_i64(), Some(1), "my own row is untouched");

    let mut theirs = row_json(2);
    scope.drop_invisible_rows(&mut theirs, &visible_ids(&[1]));
    assert!(theirs.is_null(), "{theirs}");
}

/// A row nested inside a row this caller MAY see is still judged on its own:
/// `related_sessions` hangs a list off an anchor, and a visible anchor is no
/// licence for the rows under it.
#[test]
fn a_row_inside_a_visible_row_is_judged_on_its_own() {
    let scope = person_scope(Some(7), &[], false);
    let mut v = row_json(1);
    v["related"] = serde_json::json!([row_json(2), row_json(3)]);
    scope.drop_invisible_rows(&mut v, &visible_ids(&[1, 3]));
    assert_eq!(v["id"].as_i64(), Some(1));
    assert_eq!(v["related"].as_array().map(Vec::len), Some(1));
    assert_eq!(v["related"][0]["id"].as_i64(), Some(3));
}

/// The two shapes a session row reaches the wire in, and the things that are
/// not one.
#[test]
fn the_row_shape_covers_the_full_row_and_its_projections() {
    let obj = |v: serde_json::Value| v.as_object().cloned().expect("an object");
    // The full row: `visibility` is NOT NULL in the schema, so it survives
    // `strip_nulls` and is recognisable on its own.
    assert!(looks_like_session_row(&obj(serde_json::json!({
        "visibility": "private"
    }))));
    // `SessionSummary` (`list_sessions`' default) and the `phone` view carry
    // no `visibility` at all.
    assert!(looks_like_session_row(&obj(serde_json::json!({
        "id": 3, "host_alias": "h", "tmux_name": "dev", "status": "running"
    }))));
    // A worktree row has a host and no pane; a host row names itself
    // `alias`; a kill's answer is an id and a flag.
    for not_a_row in [
        serde_json::json!({ "id": 3, "host_alias": "h", "path": "/w" }),
        serde_json::json!({ "alias": "h", "unclaimed_sessions": 2 }),
        serde_json::json!({ "id": 3, "killed": true }),
    ] {
        assert!(
            !looks_like_session_row(&obj(not_a_row.clone())),
            "{not_a_row}"
        );
    }
    // And nothing the walker leaves alone is changed at all.
    let scope = person_scope(Some(7), &[], false);
    let mut v = serde_json::json!({
        "worktrees": [{ "id": 3, "host_alias": "h", "path": "/w" }],
        "hosts": [{ "alias": "h", "unclaimed_sessions": 2 }],
    });
    let before = v.clone();
    scope.drop_invisible_rows(&mut v, &visible_ids(&[]));
    assert_eq!(v, before);
}

/// The hub's own reader is the one scope that drops nothing — and it is the
/// only early return left in the gate. `OrgScope::is_all`, the one the gate
/// used to return on, is also true for an unbound paired client, which is
/// exactly the caller this gate exists for.
#[test]
fn the_hubs_own_reader_drops_nothing() {
    let mut v = serde_json::json!([row_json(1), row_json(2)]);
    let before = v.clone();
    ViewScope::internal().drop_invisible_rows(&mut v, &visible_ids(&[]));
    assert_eq!(v, before);

    // The same payload, the same empty `sees`, a person asking: both rows go.
    let mut v = before.clone();
    person_scope(Some(7), &[], false).drop_invisible_rows(&mut v, &visible_ids(&[]));
    assert_eq!(v, serde_json::json!([]));
}

/// Nothing to ask: every row-shaped object goes, whatever it says about
/// itself. The gate's two unjudgeable cases (a poisoned store lock, a scope
/// that would not build) have no scope to consult, and a permissive answer
/// invented for them is the leak this whole task is about.
#[test]
fn with_nothing_to_judge_against_every_row_goes() {
    let mut v = serde_json::json!({
        "sessions": [row_json(1), { "id": 9, "tmux_name": "d", "host_alias": "h" }],
        "mine": row_json(1),
        "hosts": [{ "alias": "h" }],
    });
    drop_every_session_row(&mut v);
    assert_eq!(v["sessions"], serde_json::json!([]));
    assert!(v["mine"].is_null());
    assert_eq!(v["hosts"], serde_json::json!([{ "alias": "h" }]));
}
