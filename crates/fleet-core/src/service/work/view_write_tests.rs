//! The Work view's writes (work graph M14.1c): secondary links, the
//! compare-and-set on links, the primary, placements, rules and views, the
//! review inbox's decisions (`decide_batch`, `reconsider`, `ack`) and a
//! local task's org. Who may write what per caller is the isolation
//! matrix's (`mcp/tools/tests_isolation.rs`); these are the behaviours.

use super::*;
use crate::service::work::structure::{LinkDecision, ViewInput};
use crate::store::Decider;

/// See `view_tests::vs`: these reads take the caller's whole scope now, and
/// every case here is about the ORG half.
fn vs(scope: &OrgScope) -> crate::service::view_scope::ViewScope {
    crate::service::view_scope::ViewScope::internal().with_org(scope.clone())
}

fn set_primary(
    w: &W,
    scope: &OrgScope,
    sid: i64,
    link: i64,
    seen: Option<i64>,
) -> Result<SessionRow, IpcError> {
    work_link(
        &WorkLinkArgs {
            link_id: Some(link),
            expected_primary: seen,
            ..wl(w, "set_primary", sid)
        },
        &w.st,
        scope,
    )
}

/// Two suggestions on `sid` (TK-3 and TK-2 from one prompt), in review
/// order.
fn two_suggestions(w: &W, sid: i64) -> (ReviewItem, ReviewItem) {
    {
        let s = w.st.lock().unwrap();
        crate::service::work::detect::on_prompt(&s, sid, "TK-3 and TK-2 both", false).unwrap();
    }
    let r = review(&w.st, &vs(&OrgScope::All), None, None).unwrap();
    let mut it = r
        .items
        .into_iter()
        .filter(|i| i.kind == "suggestion" && i.session_id == sid);
    (it.next().unwrap(), it.next().unwrap())
}

/// A local task of org B on s2 (a no-session org-B task once unlinked).
fn local_of_b(w: &W, key: &str) -> i64 {
    let s = w.st.lock().unwrap();
    let (it, _) = s.name_session_work(w.s2, Some(key), "Beta work").unwrap();
    s.set_local_item_org(it.id, Some(w.org_b)).unwrap();
    it.id
}

/// UC2: `set_primary` moves the primary and nothing else; a device that
/// still believes the old primary gets `E_CONFLICT` naming the current
/// one; setting the primary it already has is a no-op.
#[test]
fn moving_the_primary_keeps_every_link_and_refuses_a_stale_device() {
    let w = world();
    link(&w, w.s1, w.t1, true);
    link(&w, w.s1, w.t2, false);
    let before = links_of(&w, w.s1);
    let old = before.primary_link_id.unwrap();
    let new = before
        .links
        .iter()
        .find(|l| !l.link.primary)
        .unwrap()
        .link
        .link_id;
    let row = set_primary(&w, &OrgScope::All, w.s1, new, Some(old)).unwrap();
    assert_eq!(row.work.unwrap().link_id, new);
    // Device 2 still thinks `old` is primary.
    let err = set_primary(&w, &OrgScope::All, w.s1, old, Some(old)).unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    assert_eq!(err.details.as_ref().unwrap()["primary_link_id"], new);
    let after = links_of(&w, w.s1);
    assert_eq!(after.links.len(), 2, "no link was removed or ended");
    assert_eq!(after.primary_link_id, Some(new));
    assert!(after.links.iter().all(|l| l.link.state == "active"));
    // Idempotent.
    let v = w.st.lock().unwrap().work_link_version(new).unwrap();
    set_primary(&w, &OrgScope::All, w.s1, new, Some(new)).unwrap();
    assert_eq!(w.st.lock().unwrap().work_link_version(new).unwrap(), v);
    // An unknown link, and a suggestion, cannot be made primary.
    let err = set_primary(&w, &OrgScope::All, w.s1, 999_999, None).unwrap_err();
    assert_eq!(err.code, codes::E_NOTFOUND);
    let (sug, _) = two_suggestions(&w, w.s2);
    let err = set_primary(&w, &OrgScope::All, w.s2, sug.link_id, None).unwrap_err();
    assert_eq!(err.code, codes::E_INVALID_STATE);
}

/// Concurrency: two devices move the primary at once from the same view;
/// exactly one wins, the other is told the winner.
#[test]
fn two_concurrent_set_primaries_one_wins() {
    let w = world();
    link(&w, w.s1, w.t1, true);
    link(&w, w.s1, w.t2, false);
    link(&w, w.s1, w.t3, false);
    let st = links_of(&w, w.s1);
    let old = st.primary_link_id.unwrap();
    let others: Vec<i64> = st
        .links
        .iter()
        .filter(|l| !l.link.primary)
        .map(|l| l.link.link_id)
        .collect();
    assert_eq!(others.len(), 2);
    let barrier = std::sync::Barrier::new(2);
    let results: Vec<Result<SessionRow, IpcError>> = std::thread::scope(|sc| {
        let hs: Vec<_> = others
            .iter()
            .map(|&target| {
                let (w, barrier) = (&w, &barrier);
                sc.spawn(move || {
                    barrier.wait();
                    set_primary(w, &OrgScope::All, w.s1, target, Some(old))
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let won: Vec<usize> = (0..2).filter(|&i| results[i].is_ok()).collect();
    assert_eq!(won.len(), 1, "{results:?}");
    let winner = others[won[0]];
    let loser = results.iter().find_map(|r| r.as_ref().err()).unwrap();
    assert_eq!(loser.code, codes::E_CONFLICT);
    assert_eq!(loser.details.as_ref().unwrap()["primary_link_id"], winner);
    assert_eq!(links_of(&w, w.s1).primary_link_id, Some(winner));
}

/// `link` / `confirm { primary: false }` add a secondary link; a session
/// with no primary still gets one.
#[test]
fn a_secondary_link_leaves_the_primary_where_it_is() {
    let w = world();
    let row = link(&w, w.s1, w.t2, false);
    assert_eq!(
        row.work.unwrap().item_id,
        Some(w.t2),
        "no primary yet: the secondary becomes it"
    );
    link(&w, w.s2, w.t1, true);
    let (a, _) = two_suggestions(&w, w.s2);
    let row = work_link(
        &WorkLinkArgs {
            link_id: Some(a.link_id),
            primary: Some(false),
            ..wl(&w, "confirm", w.s2)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap();
    assert_eq!(row.work.unwrap().item_id, Some(w.t1));
    let st = links_of(&w, w.s2);
    let confirmed = st
        .links
        .iter()
        .find(|l| l.link.link_id == a.link_id)
        .unwrap();
    assert_eq!(
        (confirmed.link.state.as_str(), confirmed.link.primary),
        ("active", false)
    );
    // Re-linking the primary as a secondary keeps it primary.
    let row = link(&w, w.s2, w.t1, false);
    assert_eq!(row.work.unwrap().item_id, Some(w.t1));
}

/// `work_rev` moves on a secondary link's change, which `work` (the primary)
/// never shows: it is how a client knows to re-read the session's tasks.
/// A session with no live link carries 0, and a scoped caller never sees it.
#[test]
fn work_rev_moves_on_a_secondary_link_only_the_row_shows() {
    let w = world();
    let bare =
        w.st.lock()
            .unwrap()
            .get_session_by_id(w.s2)
            .unwrap()
            .unwrap();
    assert_eq!(bare.work_rev, 0, "no live link");
    let first = link(&w, w.s1, w.t1, true);
    assert_ne!(first.work_rev, 0);
    let second = link(&w, w.s1, w.t2, false);
    assert_eq!(
        second.work.as_ref().unwrap().link_id,
        first.work.as_ref().unwrap().link_id
    );
    assert_ne!(second.work_rev, first.work_rev, "a secondary link moves it");
    let mut row = second.clone();
    bound(w.org_a).redact_row_org_only(&mut row);
    assert_eq!(row.work_rev, 0, "never sent to a scoped caller");
}

/// `link { expected_version }` is a compare-and-set on the session's link
/// to that work (`0`: none): two devices adding the same link race to one.
#[test]
fn a_link_names_the_version_it_saw() {
    let w = world();
    let args = |v: i64| WorkLinkArgs {
        item_id: Some(w.t1),
        expected_version: Some(v),
        ..wl(&w, "link", w.s1)
    };
    work_link(&args(0), &w.st, &OrgScope::All).unwrap();
    let err = work_link(&args(0), &w.st, &OrgScope::All).unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    let v = err.details.as_ref().unwrap()["version"].as_i64().unwrap();
    assert!(v >= 1);
    work_link(&args(v), &w.st, &OrgScope::All).unwrap();
    // `reject { key }` and `unlink` check theirs too.
    let v = links_of(&w, w.s1).links[0].link.link_version;
    let err = work_link(
        &WorkLinkArgs {
            item_id: Some(w.t1),
            expected_version: Some(v + 7),
            ..wl(&w, "reject", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    let lid = links_of(&w, w.s1).links[0].link.link_id;
    let err = work_link(
        &WorkLinkArgs {
            link_id: Some(lid),
            expected_version: Some(v + 7),
            ..wl(&w, "unlink", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    assert_eq!(err.details.as_ref().unwrap()["version"], v);
    work_link(
        &WorkLinkArgs {
            link_id: Some(lid),
            expected_version: Some(v),
            ..wl(&w, "unlink", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap();
    assert!(links_of(&w, w.s1).links.is_empty());
}

/// A scoped caller naming a link it may not see never learns its version
/// or state: the answer is the unknown link's, expected_version or not.
#[test]
fn a_version_is_never_an_oracle_for_a_hidden_link() {
    let w = world();
    let local_b = local_of_b(&w, "HID-2");
    let hidden =
        w.st.lock()
            .unwrap()
            .link_session_work_as(w.s1, WorkTarget::Item(local_b), "manual", false, None)
            .unwrap()
            .id;
    for action in [
        "confirm",
        "reject",
        "unlink",
        "set_primary",
        "reconsider",
        "ack",
    ] {
        let ask = |link_id: i64| {
            work_link(
                &WorkLinkArgs {
                    link_id: Some(link_id),
                    expected_version: Some(99),
                    ..wl(&w, action, w.s1)
                },
                &w.st,
                &bound(w.org_a),
            )
            .unwrap_err()
        };
        let (seen, unknown) = (ask(hidden), ask(987_654));
        assert_eq!(seen.code, codes::E_NOTFOUND, "{action}");
        assert_eq!(
            seen.message.replace(&hidden.to_string(), "X"),
            unknown.message.replace("987654", "X"),
            "{action}"
        );
        assert!(seen.details.is_none(), "{action}: {:?}", seen.details);
    }
}

/// UC5: a stale decision conflicts; a batch answers each item on its own.
#[test]
fn a_stale_decision_conflicts_and_a_batch_answers_each_item() {
    let w = world();
    let (a, b) = two_suggestions(&w, w.s1);
    work_link(
        &WorkLinkArgs {
            link_id: Some(a.link_id),
            expected_version: Some(a.link_version),
            ..wl(&w, "confirm", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap();
    let err = work_link(
        &WorkLinkArgs {
            link_id: Some(a.link_id),
            expected_version: Some(a.link_version),
            ..wl(&w, "reject", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    assert_eq!(err.details.as_ref().unwrap()["state"], "confirmed");

    let d = |link_id, decision: &str, v, primary| LinkDecision {
        session_id: w.s1,
        link_id,
        decision: decision.into(),
        expected_version: v,
        primary,
    };
    let res = structure::decide_batch(
        &w.st,
        &OrgScope::All,
        Decider::Person,
        &[
            d(a.link_id, "reject", Some(a.link_version), None),
            d(b.link_id, "confirm", Some(b.link_version), Some(false)),
            d(b.link_id, "shrug", None, None),
            d(987_654, "ack", None, None),
        ],
        &|_| Ok(()),
    )
    .unwrap();
    let codes_of: Vec<Option<&str>> = res.results.iter().map(|r| r.code.as_deref()).collect();
    assert_eq!(
        codes_of,
        vec![
            Some(codes::E_CONFLICT),
            None,
            Some(codes::E_INVALID),
            Some(codes::E_NOTFOUND)
        ]
    );
    assert!(res.results[1].ok && res.results[1].version.is_some());
    assert!(res
        .results
        .iter()
        .filter(|r| !r.ok)
        .all(|r| r.version.is_none()));
    let st = links_of(&w, w.s1);
    assert_eq!(
        st.links.iter().filter(|l| l.link.state == "active").count(),
        2
    );
    assert_eq!(st.links.iter().filter(|l| l.link.primary).count(), 1);
    // The gate is asked per item; a refused session fails only its item.
    let res = structure::decide_batch(
        &w.st,
        &OrgScope::All,
        Decider::Person,
        &[
            d(a.link_id, "reconsider", None, None),
            d(b.link_id, "reconsider", None, None),
        ],
        &|sid| {
            if sid == w.s1 {
                Err(IpcError::new(codes::E_FORBIDDEN, "not yours"))
            } else {
                Ok(())
            }
        },
    )
    .unwrap();
    assert!(res
        .results
        .iter()
        .all(|r| r.code.as_deref() == Some(codes::E_FORBIDDEN)));
    // Bounds.
    let err = structure::decide_batch(&w.st, &OrgScope::All, Decider::Person, &[], &|_| Ok(()))
        .unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
    let many = vec![d(a.link_id, "ack", None, None); structure::BATCH_MAX + 1];
    let err = structure::decide_batch(&w.st, &OrgScope::All, Decider::Person, &many, &|_| Ok(()))
        .unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
}

/// D34 through the review inbox: a batch records its caller's decider, as
/// a single decision does, and an agent cannot undo a person's decision
/// (undo, then confirm, would overturn a person's rejection in two steps).
#[test]
fn a_batch_and_an_undo_keep_the_deciders_apart() {
    let w = world();
    let (a, b) = two_suggestions(&w, w.s1);
    let d = |link_id, decision: &str| LinkDecision {
        session_id: w.s1,
        link_id,
        decision: decision.into(),
        expected_version: None,
        primary: None,
    };
    let res = structure::decide_batch(
        &w.st,
        &OrgScope::All,
        Decider::Agent,
        &[d(a.link_id, "confirm")],
        &|_| Ok(()),
    )
    .unwrap();
    assert!(res.results[0].ok);
    let source = |id| {
        w.st.lock()
            .unwrap()
            .get_work_link(id)
            .unwrap()
            .unwrap()
            .source
    };
    assert_eq!(source(a.link_id), "agent", "never a person's decision");

    work_link(
        &WorkLinkArgs {
            link_id: Some(b.link_id),
            ..wl(&w, "reject", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap();
    let undo = WorkLinkArgs {
        link_id: Some(b.link_id),
        ..wl(&w, "reconsider", w.s1)
    };
    let err = crate::service::work::work_link_as(&undo, &w.st, &OrgScope::All, Decider::Agent)
        .unwrap_err();
    assert_eq!(err.code, codes::E_FORBIDDEN);
    let res = structure::decide_batch(
        &w.st,
        &OrgScope::All,
        Decider::Agent,
        &[d(b.link_id, "reconsider")],
        &|_| Ok(()),
    )
    .unwrap();
    assert_eq!(res.results[0].code.as_deref(), Some(codes::E_FORBIDDEN));
    // The agent's own decision is its to undo; the person's is theirs.
    crate::service::work::work_link_as(
        &WorkLinkArgs {
            link_id: Some(a.link_id),
            ..wl(&w, "reconsider", w.s1)
        },
        &w.st,
        &OrgScope::All,
        Decider::Agent,
    )
    .unwrap();
    // Nor remove it: unlink, then link, is the same overturn.
    let unlink = WorkLinkArgs {
        link_id: Some(b.link_id),
        ..wl(&w, "unlink", w.s1)
    };
    let err = crate::service::work::work_link_as(&unlink, &w.st, &OrgScope::All, Decider::Agent)
        .unwrap_err();
    assert_eq!(err.code, codes::E_FORBIDDEN);
    work_link(&undo, &w.st, &OrgScope::All).unwrap();
}

/// Undo: a confirmed suggestion goes back to a suggestion with its
/// evidence, and can be decided again; a link made by hand is removed,
/// not undone.
#[test]
fn reconsider_round_trips_a_decision() {
    let w = world();
    let (a, _) = two_suggestions(&w, w.s1);
    let before = links_of(&w, w.s1);
    let sug = before
        .links
        .iter()
        .find(|l| l.link.link_id == a.link_id)
        .unwrap()
        .clone();
    assert_eq!(sug.link.state, "suggested");
    let reconsider = |v: Option<i64>| {
        work_link(
            &WorkLinkArgs {
                link_id: Some(a.link_id),
                expected_version: v,
                ..wl(&w, "reconsider", w.s1)
            },
            &w.st,
            &OrgScope::All,
        )
    };
    for decision in ["confirm", "reject"] {
        work_link(
            &WorkLinkArgs {
                link_id: Some(a.link_id),
                ..wl(&w, decision, w.s1)
            },
            &w.st,
            &OrgScope::All,
        )
        .unwrap();
        let decided = links_of(&w, w.s1);
        let l = decided
            .links
            .iter()
            .find(|l| l.link.link_id == a.link_id)
            .unwrap();
        assert_ne!(l.link.state, "suggested", "{decision}");
        let err = reconsider(Some(l.link.link_version - 1)).unwrap_err();
        assert_eq!(err.code, codes::E_CONFLICT, "{decision}");
        let row = reconsider(Some(l.link.link_version)).unwrap();
        let back = links_of(&w, w.s1);
        let l = back
            .links
            .iter()
            .find(|l| l.link.link_id == a.link_id)
            .unwrap();
        assert_eq!(l.link.state, "suggested", "{decision}");
        assert!(!l.link.primary);
        assert_eq!(
            l.link.rule, sug.link.rule,
            "{decision}: it says what proposed it"
        );
        assert_eq!(l.link.evidence, sug.link.evidence, "{decision}");
        assert!(
            row.work.as_ref().is_none_or(|p| p.link_id != a.link_id),
            "{decision}: a suggestion is never the primary"
        );
        assert!(review(&w.st, &vs(&OrgScope::All), None, None)
            .unwrap()
            .items
            .iter()
            .any(|i| i.link_id == a.link_id && i.kind == "suggestion"));
    }
    // A suggestion reconsidered is left as it is.
    reconsider(None).unwrap();
    // By hand: remove it instead.
    let hand = link(&w, w.s2, w.t1, true).work.unwrap().link_id;
    let err = work_link(
        &WorkLinkArgs {
            link_id: Some(hand),
            ..wl(&w, "reconsider", w.s2)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_INVALID_STATE);
}

/// D32: a forced cross-org link is a `cross_org` review item until a
/// person keeps it (`ack`); without `force_cross_org` it is refused.
#[test]
fn a_forced_cross_org_link_is_reviewed_until_acked() {
    let w = world();
    let local_b = local_of_b(&w, "XO-1");
    let forced = |force: Option<bool>| {
        work_link(
            &WorkLinkArgs {
                item_id: Some(local_b),
                primary: Some(false),
                force_cross_org: force,
                ..wl(&w, "link", w.s1)
            },
            &w.st,
            &OrgScope::All,
        )
    };
    let err = forced(None).unwrap_err();
    assert_eq!(err.code, codes::E_FORBIDDEN, "cross-org needs force");
    forced(Some(true)).unwrap();
    let lid = links_of(&w, w.s1)
        .links
        .iter()
        .find(|l| l.task.task_id == format!("item:{local_b}"))
        .unwrap()
        .link
        .clone();
    let in_review = || {
        review(&w.st, &vs(&OrgScope::All), None, None)
            .unwrap()
            .items
            .iter()
            .any(|i| i.kind == "cross_org" && i.link_id == lid.link_id)
    };
    assert!(in_review());
    // Neither org's bound client can keep it: A does not see B's task, B
    // does not see A's session.
    for scope in [bound(w.org_a), bound(w.org_b)] {
        let err = work_link(
            &WorkLinkArgs {
                link_id: Some(lid.link_id),
                ..wl(&w, "ack", w.s1)
            },
            &w.st,
            &scope,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
    }
    let ack = |v: i64| {
        work_link(
            &WorkLinkArgs {
                link_id: Some(lid.link_id),
                expected_version: Some(v),
                ..wl(&w, "ack", w.s1)
            },
            &w.st,
            &OrgScope::All,
        )
    };
    assert_eq!(
        ack(lid.link_version + 1).unwrap_err().code,
        codes::E_CONFLICT
    );
    ack(lid.link_version).unwrap();
    assert!(!in_review(), "kept on purpose: out of the inbox");
    let v =
        w.st.lock()
            .unwrap()
            .work_link_version(lid.link_id)
            .unwrap()
            .unwrap();
    assert_eq!(
        v,
        lid.link_version + 1,
        "an ack is a change another device sees"
    );
    ack(v).unwrap();
    assert_eq!(
        w.st.lock().unwrap().work_link_version(lid.link_id).unwrap(),
        Some(v),
        "a second ack changes nothing"
    );
    // A suggestion is not a conflict to keep.
    let (sug, _) = two_suggestions(&w, w.s1);
    let err = work_link(
        &WorkLinkArgs {
            link_id: Some(sug.link_id),
            ..wl(&w, "ack", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_NOTFOUND);
}

/// Gap plan G2.2, the rule editor's live count: `matched` is every OPEN
/// task (not done, not archived) the draft's conditions match, whether it
/// would move them or not and whether it is enabled; `matched_sample` names
/// the first few by key. A draft that matches nothing says 0.
#[test]
fn rule_preview_counts_the_open_tasks_the_draft_matches() {
    let w = world();
    let draft = RuleInput {
        name: "Audit".into(),
        conditions: RuleConditions {
            key_prefix: Some("tk".into()),
            ..Default::default()
        },
        group: "Compliance".into(),
        ..Default::default()
    };
    let tree = page(&w, &OrgScope::All, WorkTreeFilters::default());
    let open: Vec<String> = tree
        .tasks
        .iter()
        .filter(|t| !t.archived && t.stage != "done")
        .filter_map(|t| t.key.clone())
        .filter(|k| k.to_ascii_uppercase().starts_with("TK-"))
        .collect();
    assert!(open.len() >= 2, "the world has open TK tasks: {open:?}");
    let pv = structure::rule_preview(&w.st, &OrgScope::All, &draft).unwrap();
    assert_eq!(pv.matched as usize, open.len(), "{pv:?}");
    assert_eq!(
        pv.matched_sample.len(),
        open.len().min(structure::MATCHED_SAMPLE)
    );
    for k in &pv.matched_sample {
        assert!(open.contains(k), "{k} is one of the open matches");
    }
    // Off, it moves nothing and still says what it matches.
    let off = RuleInput {
        enabled: Some(false),
        ..draft.clone()
    };
    let pv_off = structure::rule_preview(&w.st, &OrgScope::All, &off).unwrap();
    assert_eq!(pv_off.total, 0);
    assert_eq!(pv_off.matched, pv.matched);
    // Nothing matches: 0 and no names.
    let none = RuleInput {
        conditions: RuleConditions {
            key_prefix: Some("NOPE".into()),
            ..Default::default()
        },
        ..draft
    };
    let pv_none = structure::rule_preview(&w.st, &OrgScope::All, &none).unwrap();
    assert_eq!((pv_none.matched, pv_none.matched_sample.len()), (0, 0));
}

/// UC6: a person's placement beats a rule, a rule beats the tracker;
/// placements are compare-and-set; the rule preview is exactly what saving
/// the rule does; a disabled rule is the way back.
#[test]
fn placement_and_rules_through_the_writes() {
    let w = world();
    let tree_all = |w: &W| page(w, &OrgScope::All, WorkTreeFilters::default());
    let draft = RuleInput {
        name: "Audit".into(),
        conditions: RuleConditions {
            key_prefix: Some("tk".into()),
            title_contains: Some("o".into()),
            ..Default::default()
        },
        group: "Compliance".into(),
        ..Default::default()
    };
    let before = tree_all(&w);
    let pv = structure::rule_preview(&w.st, &OrgScope::All, &draft).unwrap();
    assert!(pv.total >= 2, "{pv:?}");
    let rule = structure::rule_save(&w.st, &OrgScope::All, &draft).unwrap();
    assert_eq!(rule.conditions.key_prefix.as_deref(), Some("TK"));
    let after = tree_all(&w);
    // Preview == apply: exactly the previewed tasks moved, each where the
    // preview said; every other task stayed.
    for t in &after.tasks {
        let was = &before
            .tasks
            .iter()
            .find(|b| b.task_id == t.task_id)
            .unwrap()
            .group;
        match pv.affected.iter().find(|a| a.task_id == t.task_id) {
            Some(a) => {
                // The draft has no id yet: the preview's `rule_id` is a
                // placeholder, the rest is the group the save gives.
                let shape = |g: &GroupRef| {
                    (
                        g.id.clone(),
                        g.label.clone(),
                        g.source.clone(),
                        g.tracker_value.clone(),
                    )
                };
                assert_eq!(&a.from, was, "{}", t.task_id);
                assert_eq!(shape(&a.to), shape(&t.group), "{}", t.task_id);
                assert_eq!(
                    (t.group.source.as_str(), t.group.rule_id),
                    ("rule", Some(rule.id))
                );
            }
            None => assert_eq!(&t.group, was, "{} moved unannounced", t.task_id),
        }
    }

    // A one-off correction of TK-2, compare-and-set.
    let t2 = structure::place(
        &w.st,
        &vs(&OrgScope::All),
        "item:2",
        Some("Security"),
        Some("why"),
        Some(0),
        "me",
    )
    .unwrap();
    assert_eq!(
        (t2.group.source.as_str(), t2.group.label.as_str()),
        ("manual", "Security")
    );
    // The answer carries the placement just written (patched into the
    // graph, not re-read), exactly as a fresh read shows it.
    assert_eq!(t2.placement_version, 1);
    let fresh = task(&w.st, &vs(&OrgScope::All), "item:2").unwrap();
    assert_eq!(fresh.task.placement_version, 1);
    let err = structure::place(
        &w.st,
        &vs(&OrgScope::All),
        "item:2",
        Some("Other"),
        None,
        Some(0),
        "phone",
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    assert_eq!(err.details.as_ref().unwrap()["group"], "Security");
    let err = structure::place(
        &w.st,
        &vs(&OrgScope::All),
        "item:2",
        Some("x"),
        None,
        None,
        "me",
    )
    .unwrap_err();
    assert_eq!(
        err.code,
        codes::E_INVALID,
        "a placement always names the version it saw"
    );
    let d = task(&w.st, &vs(&OrgScope::All), "item:2").unwrap();
    assert_eq!(
        d.placement.as_ref().unwrap().updated_by.as_deref(),
        Some("me")
    );
    // The manual placement is kept by the rule's preview.
    let pv2 = structure::rule_preview(
        &w.st,
        &OrgScope::All,
        &RuleInput {
            group: "Else".into(),
            ..draft.clone()
        },
    )
    .unwrap();
    assert!(pv2.kept_manual >= 1);
    assert!(pv2.affected.iter().all(|a| a.task_id != "item:2"));
    // Clearing falls back to the rule; disabling the rule to the tracker.
    let cleared = structure::place(
        &w.st,
        &vs(&OrgScope::All),
        "item:2",
        None,
        None,
        Some(t2.placement_version),
        "me",
    )
    .unwrap();
    // A cleared placement answers as none, its group the rule's.
    assert_eq!(cleared.placement_version, 0);
    assert_eq!(cleared.group.source, "rule");
    assert_eq!(task_of(&tree_all(&w), "TK-2").group.source, "rule");
    let err = structure::rule_save(
        &w.st,
        &OrgScope::All,
        &RuleInput {
            id: Some(rule.id),
            enabled: Some(false),
            expected_version: Some(rule.version + 1),
            ..draft.clone()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    let off = structure::rule_save(
        &w.st,
        &OrgScope::All,
        &RuleInput {
            id: Some(rule.id),
            enabled: Some(false),
            expected_version: Some(rule.version),
            ..draft.clone()
        },
    )
    .unwrap();
    assert_eq!(off.version, rule.version + 1);
    assert_eq!(task_of(&tree_all(&w), "TK-2").group.source, "tracker");
    // Delete: stale refused, current accepted, gone after.
    let err =
        structure::rule_delete(&w.st, &OrgScope::All, rule.id, Some(rule.version)).unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    structure::rule_delete(&w.st, &OrgScope::All, rule.id, Some(off.version)).unwrap();
    let err = structure::rule_delete(&w.st, &OrgScope::All, rule.id, None).unwrap_err();
    assert_eq!(err.code, codes::E_NOTFOUND);
    // An empty rule, or one naming an unknown tracker, is refused.
    let err = structure::rule_save(
        &w.st,
        &OrgScope::All,
        &RuleInput {
            name: "all".into(),
            group: "x".into(),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
    let err = structure::rule_save(
        &w.st,
        &OrgScope::All,
        &RuleInput {
            conditions: RuleConditions {
                tracker_id: Some(987_654),
                ..Default::default()
            },
            ..draft.clone()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_NOTFOUND);
    // Rules are no scoped caller's to write (D34: they reach every org).
    let host = OrgScope::Host {
        alias: "h1".into(),
        org: Some(w.org_a),
        isolated: Default::default(),
    };
    for scope in [bound(w.org_a), host.clone()] {
        let err = structure::rule_save(&w.st, &scope, &draft).unwrap_err();
        assert_eq!(err.code, codes::E_FORBIDDEN);
        let err = structure::rule_delete(&w.st, &scope, 1, None).unwrap_err();
        assert_eq!(err.code, codes::E_FORBIDDEN);
    }
    // A bound client places what it sees, and only that.
    let local_b = local_of_b(&w, "PL-1");
    let t = structure::place(
        &w.st,
        &vs(&bound(w.org_a)),
        "item:1",
        Some("Mine"),
        None,
        Some(0),
        "phone",
    )
    .unwrap();
    assert_eq!(t.group.label, "Mine");
    let seen = task(&w.st, &vs(&bound(w.org_a)), "item:1")
        .unwrap()
        .placement
        .unwrap();
    assert_eq!(
        seen.updated_by, None,
        "who placed it is not a scoped caller's to read"
    );
    let all = task(&w.st, &vs(&OrgScope::All), "item:1")
        .unwrap()
        .placement
        .unwrap();
    assert_eq!(all.updated_by.as_deref(), Some("phone"));
    let tid = format!("item:{local_b}");
    let hidden = structure::place(
        &w.st,
        &vs(&bound(w.org_a)),
        &tid,
        Some("x"),
        None,
        Some(5),
        "phone",
    )
    .unwrap_err();
    let unknown = structure::place(
        &w.st,
        &vs(&bound(w.org_a)),
        "item:987654",
        Some("x"),
        None,
        Some(5),
        "phone",
    )
    .unwrap_err();
    assert_eq!(
        hidden.code,
        codes::E_NOTFOUND,
        "never the version's conflict"
    );
    assert_eq!(
        hidden.message.replace(&local_b.to_string(), "X"),
        unknown.message.replace("987654", "X")
    );
    let err =
        structure::place(&w.st, &vs(&host), "item:1", Some("x"), None, Some(1), "h1").unwrap_err();
    assert_eq!(
        err.code,
        codes::E_FORBIDDEN,
        "a host does not reorganise the fleet"
    );
}

/// Link `sid` to the bare key `key` (no item has it yet), as typed.
fn bare(w: &W, sid: i64, key: &str) {
    w.st.lock()
        .unwrap()
        .link_session_work(sid, WorkTarget::Ref(key), "manual")
        .unwrap();
}

/// A new tracker item `key` on `w.tracker`, then the sync's retro-bind.
fn sync_item(w: &W, ext: &str, key: &str) -> i64 {
    let s = w.st.lock().unwrap();
    let id = item(&s, w.tracker, ext, key, "Arrived later", "TP");
    s.bind_tracker_refs(w.tracker).unwrap();
    id
}

/// A person's placement on a bare key's task (`ref:KEY`) follows the task
/// when a sync binds the key to an item (`item:N`): it used to stay keyed
/// on the old id, and the manual group silently disappeared.
#[test]
fn a_placement_on_a_bare_key_follows_the_bind() {
    let w = world();
    bare(&w, w.s1, "TK-9");
    let placed = structure::place(
        &w.st,
        &vs(&OrgScope::All),
        "ref:TK-9",
        Some("Payments"),
        None,
        Some(0),
        "me",
    )
    .unwrap();
    assert_eq!(
        (placed.task_id.as_str(), placed.group.source.as_str()),
        ("ref:TK-9", "manual")
    );
    let id = sync_item(&w, "9", "TK-9");
    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    let t = task_of(&p, "TK-9");
    assert_eq!(t.task_id, format!("item:{id}"));
    assert_eq!(
        (t.group.source.as_str(), t.group.label.as_str()),
        ("manual", "Payments")
    );
    let s = w.st.lock().unwrap();
    assert!(s.work_placement("ref:TK-9").unwrap().is_none());
    let moved = s.work_placement(&format!("item:{id}")).unwrap().unwrap();
    assert_eq!(moved.group.as_deref(), Some("Payments"));
    assert_eq!(moved.version, 2, "a re-key is a change a device must see");
}

/// The item already has its own placement: that one wins, and the bare
/// key's is swept rather than left behind.
#[test]
fn an_items_own_placement_wins_over_the_bare_keys() {
    let w = world();
    bare(&w, w.s1, "TK-9");
    structure::place(
        &w.st,
        &vs(&OrgScope::All),
        "ref:TK-9",
        Some("Payments"),
        None,
        Some(0),
        "me",
    )
    .unwrap();
    let id = {
        let s = w.st.lock().unwrap();
        let id = item(&s, w.tracker, "9", "TK-9", "Arrived later", "TP");
        s.seed_placement(&format!("item:{id}"), Some("Billing"), None);
        s.bind_tracker_refs(w.tracker).unwrap();
        id
    };
    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    let t = task_of(&p, "TK-9");
    assert_eq!(t.task_id, format!("item:{id}"));
    assert_eq!(
        (t.group.source.as_str(), t.group.label.as_str()),
        ("manual", "Billing")
    );
    let s = w.st.lock().unwrap();
    assert!(
        s.work_placement("ref:TK-9").unwrap().is_none(),
        "the orphan is swept"
    );
}

/// A bare link of another org stays bare (M5), so its `ref:KEY` task stays
/// too — and keeps the person's placement.
#[test]
fn a_bare_key_of_another_org_keeps_its_placement() {
    let w = world();
    let s3 = {
        let s = w.st.lock().unwrap();
        s.upsert_host("h3").unwrap();
        s.set_host_org("h3", Some(w.org_b)).unwrap();
        s.upsert_session("three", "h3", None, None, 1, 1, "running", None)
            .unwrap()
    };
    bare(&w, w.s1, "TK-9");
    bare(&w, s3, "TK-9");
    structure::place(
        &w.st,
        &vs(&OrgScope::All),
        "ref:TK-9",
        Some("Payments"),
        None,
        Some(0),
        "me",
    )
    .unwrap();
    let id = sync_item(&w, "9", "TK-9");
    let s = w.st.lock().unwrap();
    assert_eq!(
        s.work_placement("ref:TK-9")
            .unwrap()
            .unwrap()
            .group
            .as_deref(),
        Some("Payments"),
        "org B's bare task still carries it"
    );
    assert!(s.work_placement(&format!("item:{id}")).unwrap().is_none());
}

/// `sweep_orphan_placements` drops a placement whose task is gone (an item
/// row deleted, a bare key no link names) and keeps every live one.
#[test]
fn orphan_placements_are_swept_and_live_ones_kept() {
    let w = world();
    bare(&w, w.s1, "FREE-1");
    let s = w.st.lock().unwrap();
    s.seed_placement(&format!("item:{}", w.t1), Some("Keep"), None);
    s.seed_placement(&format!("item:{}", w.t3), Some("Gone"), None);
    s.seed_placement("ref:FREE-1", Some("Keep too"), None);
    s.seed_placement("ref:NOBODY-1", Some("Gone too"), None);
    s.seed_delete_item(w.t3);
    assert_eq!(s.sweep_orphan_placements().unwrap(), 2);
    let mut left: Vec<String> = s
        .work_placements()
        .unwrap()
        .into_iter()
        .map(|p| p.task_id)
        .collect();
    left.sort();
    let mut want = vec![format!("item:{}", w.t1), "ref:FREE-1".to_string()];
    want.sort();
    assert_eq!(left, want);
    assert_eq!(s.sweep_orphan_placements().unwrap(), 0, "idempotent");
}

/// D35: views are shared on the hub; a bound client's are its org's — it
/// can neither see, replace nor delete another's, nor name an org or a
/// tracker outside its own in one.
#[test]
fn views_are_fenced_per_owner() {
    let w = world();
    let view = |name: &str, filters: WorkTreeFilters| ViewInput {
        name: name.into(),
        filters,
        ..Default::default()
    };
    let (a, b) = (bound(w.org_a), bound(w.org_b));
    let mine = structure::view_save(
        &w.st,
        &a,
        &view(
            "Open",
            WorkTreeFilters {
                status: Some("open".into()),
                ..Default::default()
            },
        ),
    )
    .unwrap();
    // The same name is another org's own; a second one in the same org is refused.
    let theirs =
        structure::view_save(&w.st, &b, &view("Open", WorkTreeFilters::default())).unwrap();
    let err =
        structure::view_save(&w.st, &a, &view("Open", WorkTreeFilters::default())).unwrap_err();
    assert_eq!(err.code, codes::E_EXISTS);
    let master = structure::view_save(
        &w.st,
        &OrgScope::All,
        &view("Everything", WorkTreeFilters::default()),
    )
    .unwrap();
    assert_eq!(structure::views(&w.st, &a).unwrap(), vec![mine.clone()]);
    assert_eq!(structure::views(&w.st, &OrgScope::All).unwrap().len(), 3);
    // Another org's view, and the master's, answer as unknown views.
    for id in [theirs.id, master.id, 987_654] {
        let err = structure::view_save(
            &w.st,
            &a,
            &ViewInput {
                id: Some(id),
                ..view("Mine now", WorkTreeFilters::default())
            },
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        assert_eq!(err.message, format!("view {id} not found"));
        let err = structure::view_delete(&w.st, &a, id, None).unwrap_err();
        assert_eq!(err.message, format!("view {id} not found"));
    }
    // Filters may name only what the client sees.
    let org_b = WorkTreeFilters {
        org: Some(IdOrWord::Id(w.org_b)),
        ..Default::default()
    };
    let err = structure::view_save(&w.st, &a, &view("B", org_b.clone())).unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.message.clone()),
        (codes::E_NOTFOUND, format!("org {} not found", w.org_b))
    );
    let tb =
        w.st.lock()
            .unwrap()
            .add_tracker("jira", "B's", "https://b.atlassian.net")
            .unwrap();
    w.st.lock()
        .unwrap()
        .set_tracker_org(tb.id, Some(w.org_b))
        .unwrap();
    let err = structure::view_save(
        &w.st,
        &a,
        &view(
            "B",
            WorkTreeFilters {
                tracker: Some(IdOrWord::Id(tb.id)),
                ..Default::default()
            },
        ),
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_NOTFOUND);
    structure::view_save(&w.st, &OrgScope::All, &view("B", org_b)).unwrap();
    // Compare-and-set, then delete.
    let upd = |v| ViewInput {
        id: Some(mine.id),
        expected_version: Some(v),
        ..view("Open now", WorkTreeFilters::default())
    };
    let err = structure::view_save(&w.st, &a, &upd(mine.version + 1)).unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    let v2 = structure::view_save(&w.st, &a, &upd(mine.version)).unwrap();
    assert_eq!(
        (v2.name.as_str(), v2.version),
        ("Open now", mine.version + 1)
    );
    assert_eq!(
        structure::view_delete(&w.st, &a, mine.id, Some(mine.version))
            .unwrap_err()
            .code,
        codes::E_CONFLICT
    );
    structure::view_delete(&w.st, &a, mine.id, Some(v2.version)).unwrap();
    // The master may tidy any view; the owner stays the org's.
    let t2 = structure::view_save(
        &w.st,
        &OrgScope::All,
        &ViewInput {
            id: Some(theirs.id),
            ..view("Renamed", WorkTreeFilters::default())
        },
    )
    .unwrap();
    assert_eq!(t2.owner_org, Some(w.org_b));
    structure::view_delete(&w.st, &OrgScope::All, theirs.id, None).unwrap();
    // A host keeps no views.
    let host = OrgScope::Host {
        alias: "h1".into(),
        org: Some(w.org_a),
        isolated: Default::default(),
    };
    assert_eq!(
        structure::view_save(&w.st, &host, &view("h", WorkTreeFilters::default()))
            .unwrap_err()
            .code,
        codes::E_FORBIDDEN
    );
    assert_eq!(
        structure::view_delete(&w.st, &host, master.id, None)
            .unwrap_err()
            .code,
        codes::E_FORBIDDEN
    );
}

/// UC7 / D33: a local task changes org only with a fresh impact token,
/// only by an unrestricted caller; after the move a client bound to the
/// old org no longer receives the task, nor its link on its own session.
#[test]
fn a_local_task_moves_org_only_with_a_fresh_impact() {
    let w = world();
    let local = {
        let s = w.st.lock().unwrap();
        s.name_session_work(w.s1, Some("LOC-9"), "Refactor billing")
            .unwrap()
            .0
            .id
    };
    let tid = format!("item:{local}");
    let bound_a = bound(w.org_a);
    assert!(
        task(&w.st, &vs(&bound_a), &tid).is_ok(),
        "unassigned: visible"
    );

    let imp = structure::org_impact(&w.st, &vs(&OrgScope::All), &tid, Some(w.org_b)).unwrap();
    assert!(imp.allowed);
    assert!(imp.links[0].becomes_cross_org, "s1 is org A's");
    let err = structure::assign_org(
        &w.st,
        &vs(&OrgScope::All),
        &tid,
        Some(w.org_b),
        Some("stale"),
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    assert_eq!(
        err.details.as_ref().unwrap()["impact_token"],
        imp.impact_token,
        "the fresh impact"
    );
    for scope in [
        bound_a.clone(),
        OrgScope::Host {
            alias: "h1".into(),
            org: Some(w.org_a),
            isolated: Default::default(),
        },
    ] {
        let err = structure::assign_org(
            &w.st,
            &vs(&scope),
            &tid,
            Some(w.org_b),
            Some(&imp.impact_token),
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_FORBIDDEN, "{scope:?} moves no org");
    }
    let err =
        structure::assign_org(&w.st, &vs(&OrgScope::All), &tid, Some(w.org_b), None).unwrap_err();
    assert_eq!(err.code, codes::E_INVALID);
    // The impact changes when a session joins: the old token is refused.
    link(&w, w.s2, local, false);
    let err = structure::assign_org(
        &w.st,
        &vs(&OrgScope::All),
        &tid,
        Some(w.org_b),
        Some(&imp.impact_token),
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    let imp = structure::org_impact(&w.st, &vs(&OrgScope::All), &tid, Some(w.org_b)).unwrap();
    let moved = structure::assign_org(
        &w.st,
        &vs(&OrgScope::All),
        &tid,
        Some(w.org_b),
        Some(&imp.impact_token),
    )
    .unwrap();
    assert_eq!(
        (moved.org_id, moved.org_source.as_str()),
        (Some(w.org_b), "item")
    );
    assert!(moved.sessions.iter().all(|l| l.cross_org));
    assert_eq!(
        task(&w.st, &vs(&bound_a), &tid).unwrap_err().code,
        codes::E_NOTFOUND
    );
    assert!(session_tasks(&w.st, &vs(&bound_a), w.s1)
        .unwrap()
        .links
        .is_empty());
    assert!(
        review(&w.st, &vs(&OrgScope::All), None, None)
            .unwrap()
            .items
            .iter()
            .any(|i| i.kind == "cross_org" && i.task.task_id == tid),
        "D32: the move's cross-org links are reviewed"
    );
    // Back to no org.
    let imp = structure::org_impact(&w.st, &vs(&OrgScope::All), &tid, Some(0)).unwrap();
    let back = structure::assign_org(
        &w.st,
        &vs(&OrgScope::All),
        &tid,
        Some(0),
        Some(&imp.impact_token),
    )
    .unwrap();
    assert_ne!(
        back.org_source, "item",
        "no own org: inferred from its sessions again"
    );
    assert!(!back.org_fenced);
    // A tracker item's org is its tracker's; the same org is no move.
    let t = structure::org_impact(&w.st, &vs(&OrgScope::All), "item:1", Some(w.org_b)).unwrap();
    let err = structure::assign_org(
        &w.st,
        &vs(&OrgScope::All),
        "item:1",
        Some(w.org_b),
        Some(&t.impact_token),
    )
    .unwrap_err();
    assert_eq!(err.code, codes::E_FORBIDDEN);
}

/// Gap plan G2.2: the impact names the people whose org-bound devices lose
/// or gain the task ("Ondrej loses access"); a device nobody owns is
/// counted, never named.
#[test]
fn org_impact_names_the_people_whose_bound_devices_lose_or_gain_it() {
    let w = world();
    let local = {
        let s = w.st.lock().unwrap();
        s.set_org_bound_sees_unassigned(w.org_a, false).unwrap();
        s.set_org_bound_sees_unassigned(w.org_b, true).unwrap();
        let ondrej = s.create_person("ondrej", Some("Ondrej")).unwrap();
        let eva = s.create_person("eva", None).unwrap();
        s.insert_client_token("pa", "aa01", "full").unwrap();
        s.insert_client_token("pb", "bb02", "full").unwrap();
        s.set_client_org("pa", Some(w.org_a)).unwrap();
        s.set_client_org("pb", Some(w.org_b)).unwrap();
        s.set_client_person("pa", Some(eva.id)).unwrap();
        s.set_client_person("pb", Some(ondrej.id)).unwrap();
        s.name_session_work(w.s1, Some("LOC-8"), "Unassigned work")
            .unwrap()
            .0
            .id
    };
    let tid = format!("item:{local}");
    // No org → A: B's device (Ondrej's) saw unassigned work and loses it;
    // A's (eva's) did not and gains it.
    let imp = structure::org_impact(&w.st, &vs(&OrgScope::All), &tid, Some(w.org_a)).unwrap();
    assert_eq!(imp.people_losing, vec!["Ondrej".to_string()]);
    assert_eq!(imp.people_gaining, vec!["eva".to_string()]);
    // Unowned again: counted, not named.
    w.st.lock().unwrap().set_client_person("pb", None).unwrap();
    let imp = structure::org_impact(&w.st, &vs(&OrgScope::All), &tid, Some(w.org_a)).unwrap();
    assert_eq!(imp.bound_clients_losing, 1);
    assert!(imp.people_losing.is_empty(), "{:?}", imp.people_losing);
}

/// D31 in the impact: a bound client counts as losing or gaining a task
/// exactly as its `OrgScope::Org` sees it — unassigned work only while its
/// org's `bound_sees_unassigned` is on. An unbound client is never counted.
#[test]
fn org_impact_counts_bound_clients_as_their_scope_sees() {
    let w = world();
    let local = {
        let s = w.st.lock().unwrap();
        s.set_org_bound_sees_unassigned(w.org_a, false).unwrap();
        s.set_org_bound_sees_unassigned(w.org_b, true).unwrap();
        s.insert_client_token("pa", "aa01", "full").unwrap();
        s.insert_client_token("pb", "bb02", "full").unwrap();
        s.insert_client_token("free", "cc03", "full").unwrap();
        s.set_client_org("pa", Some(w.org_a)).unwrap();
        s.set_client_org("pb", Some(w.org_b)).unwrap();
        s.name_session_work(w.s1, Some("LOC-7"), "Unassigned work")
            .unwrap()
            .0
            .id
    };
    let tid = format!("item:{local}");
    let counts = |i: &structure::OrgImpact| (i.bound_clients_losing, i.bound_clients_gaining);
    // What each bound client's own scope says, for a move `from` → `to`.
    let expect = |from: Option<i64>, to: Option<i64>| {
        let s = w.st.lock().unwrap();
        let (mut losing, mut gaining) = (0u32, 0u32);
        for o in [w.org_a, w.org_b] {
            let cs = OrgScope::for_client(&s, o).unwrap();
            match (cs.sees_org(from), cs.sees_org(to)) {
                (true, false) => losing += 1,
                (false, true) => gaining += 1,
                _ => {}
            }
        }
        (losing, gaining)
    };

    // No org → A: A's client never saw unassigned work, so it gains the
    // task; B's did, so it loses it.
    let imp = structure::org_impact(&w.st, &vs(&OrgScope::All), &tid, Some(w.org_a)).unwrap();
    assert_eq!(imp.from_org, None);
    assert_eq!(counts(&imp), (1, 1));
    assert_eq!(counts(&imp), expect(None, Some(w.org_a)));
    structure::assign_org(
        &w.st,
        &vs(&OrgScope::All),
        &tid,
        Some(w.org_a),
        Some(&imp.impact_token),
    )
    .unwrap();

    // A → no org: the mirror image.
    let imp = structure::org_impact(&w.st, &vs(&OrgScope::All), &tid, Some(0)).unwrap();
    assert_eq!(imp.from_org, Some(w.org_a));
    assert_eq!(counts(&imp), (1, 1));
    assert_eq!(counts(&imp), expect(Some(w.org_a), None));

    // With A's switch on, A's client sees the task in A and in no org
    // alike: the move changes nothing for it.
    w.st.lock()
        .unwrap()
        .set_org_bound_sees_unassigned(w.org_a, true)
        .unwrap();
    let imp = structure::org_impact(&w.st, &vs(&OrgScope::All), &tid, Some(0)).unwrap();
    assert_eq!(
        counts(&imp),
        (0, 1),
        "B's client gains the unassigned task; A's sees it either way"
    );
    assert_eq!(counts(&imp), expect(Some(w.org_a), None));
}

/// A scoped caller whose session's primary is another org's (a forced
/// link) saw "none": its compare-and-set is on that, and it is never told
/// the hidden link's id.
#[test]
fn a_hidden_primary_neither_blocks_nor_leaks_to_a_bound_client() {
    let w = world();
    let local_b = local_of_b(&w, "HID-1");
    let hidden =
        w.st.lock()
            .unwrap()
            .link_session_work_as(w.s1, WorkTarget::Item(local_b), "manual", true, None)
            .unwrap()
            .id;
    link(&w, w.s1, w.t1, false);
    let a = bound(w.org_a);
    let st = session_tasks(&w.st, &vs(&a), w.s1).unwrap();
    assert_eq!(st.primary_link_id, None, "the hidden primary is not named");
    let mine = st.links[0].link.link_id;
    let err = set_primary(&w, &a, w.s1, mine, Some(mine)).unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    assert!(
        !err.message.contains(&hidden.to_string()),
        "{}",
        err.message
    );
    assert_eq!(
        err.details.as_ref().unwrap()["primary_link_id"],
        serde_json::Value::Null
    );
    set_primary(&w, &a, w.s1, mine, Some(0)).unwrap();
    let all = links_of(&w, w.s1);
    assert_eq!(all.primary_link_id, Some(mine));
    assert!(all
        .links
        .iter()
        .any(|l| l.link.link_id == hidden && l.link.state == "active"));
}

/// A switch's compare-and-set is on the primary the caller can see, as
/// `set_primary`'s: a client bound to org A, whose session's primary is a
/// forced link to org B's task, saw "none" and switches with that; the
/// refusal for a stale view never names B's link.
#[test]
fn a_switch_compares_the_primary_this_org_can_see() {
    let w = world();
    link(&w, w.s1, w.t1, false);
    let local_b = local_of_b(&w, "XO-2");
    work_link(
        &WorkLinkArgs {
            item_id: Some(local_b),
            primary: Some(true),
            force_cross_org: Some(true),
            ..wl(&w, "link", w.s1)
        },
        &w.st,
        &OrgScope::All,
    )
    .unwrap();
    let st = links_of(&w, w.s1);
    let foreign = st.primary_link_id.unwrap();
    let from = st
        .links
        .iter()
        .find(|l| l.link.link_id != foreign)
        .unwrap()
        .link
        .link_id;
    let switch = |seen: i64| {
        work_link(
            &WorkLinkArgs {
                link_id: Some(from),
                item_id: Some(w.t2),
                expected_primary: Some(seen),
                ..wl(&w, "switch", w.s1)
            },
            &w.st,
            &bound(w.org_a),
        )
    };
    let err = switch(foreign).unwrap_err();
    assert_eq!(err.code, codes::E_CONFLICT);
    assert!(
        !err.details.as_ref().unwrap()["primary_link_id"].is_number(),
        "{err:?}"
    );
    switch(0).unwrap();
}
