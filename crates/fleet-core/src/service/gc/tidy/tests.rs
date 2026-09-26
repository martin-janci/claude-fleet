//! The tidy planner's table: one row per reason, one per protection, the
//! secondary-reason merge, snooze / never / keep, and what auto-tidy may act
//! on — including `idle_unlinked`'s own exclusions and D19.

use super::*;
use crate::service::pane_intel::PendingInput;

const NOW: i64 = 10_000_000;
const HOUR: i64 = 3600;
const DAY: i64 = 86_400;

fn cfg() -> TidyConfig {
    TidyConfig::default()
}

/// A live work session on `local`, idle for `idle` seconds, with a tracked
/// worktree of its own.
fn session(id: i64, idle: i64) -> TidySession {
    let mut row = crate::service::gc::tests::row(id, "work", Some(NOW - idle), NOW - idle);
    row.worktree_key = Some(format!("wt{id}"));
    row.project_id = Some(1);
    TidySession {
        row,
        link: None,
        in_progress: false,
        snoozed_until: None,
        never: false,
        pr_merged: false,
        last_touch_at: None,
        open_tasks: false,
        branch: None,
        any_link: false,
        kept_until: None,
    }
}

fn link(status: &str, changed_ago: i64) -> TidyLink {
    TidyLink {
        link_id: 100,
        key: Some("ABC-1".into()),
        status_category: Some(status.into()),
        status_name: Some(status.into()),
        status_changed_at: Some(NOW - changed_ago),
        ..Default::default()
    }
}

/// Done for 3 days, idle 5 hours: the canonical `done_idle` candidate.
fn done_session(id: i64) -> TidySession {
    TidySession {
        link: Some(link("done", 3 * DAY)),
        ..session(id, 5 * HOUR)
    }
}

fn local() -> HashSet<String> {
    HashSet::from(["local".to_string()])
}

fn run(sessions: &[TidySession], cfg: &TidyConfig) -> Vec<TidyCandidate> {
    let reachable = local();
    plan_tidy(
        sessions,
        cfg,
        &TidyContext {
            controller: None,
            operator: None,
            reachable: &reachable,
            now: NOW,
        },
    )
}

fn reasons(c: &[TidyCandidate]) -> Vec<(i64, TidyReason, TidyAction)> {
    c.iter()
        .map(|c| (c.session_id, c.reason, c.action))
        .collect()
}

#[test]
fn one_row_per_reason() {
    let merged = TidySession {
        pr_merged: true,
        ..session(2, 5 * HOUR)
    };
    let mut resolution = link("done", HOUR);
    resolution.resolution = Some("not_planned".into());
    let not_planned = TidySession {
        link: Some(resolution),
        ..session(3, 5 * HOUR)
    };
    // Two sessions in one worktree: the idler goes, the recent one stays.
    let mut dup_idle = session(4, DAY);
    dup_idle.row.worktree_key = Some("shared".into());
    let mut dup_recent = session(5, 5 * HOUR);
    dup_recent.row.worktree_key = Some("shared".into());
    let mut ghost = TidySession {
        link: Some(link("todo", DAY)),
        ..session(6, 5 * HOUR)
    };
    ghost.row.status = "ghost".into();
    ghost.row.claude_session_id = Some("c6".into());
    ghost.row.lost_at = Some(NOW - (14 * DAY - 2 * HOUR));

    let got = run(
        &[
            done_session(1),
            merged,
            not_planned,
            dup_idle,
            dup_recent,
            ghost,
        ],
        &cfg(),
    );
    assert_eq!(
        reasons(&got),
        vec![
            (1, TidyReason::DoneIdle, TidyAction::SafeKill),
            (2, TidyReason::PrMergedIdle, TidyAction::SafeKill),
            (3, TidyReason::NotPlanned, TidyAction::SafeKill),
            (4, TidyReason::DuplicateWorktree, TidyAction::Kill),
            (6, TidyReason::GhostExpiring, TidyAction::ResumeOrExpire),
        ]
    );
    assert_eq!(got[4].expires_at, Some(NOW + 2 * HOUR));
    assert_eq!(got[0].key.as_deref(), Some("ABC-1"));
    assert_eq!(got[0].idle_secs, 5 * HOUR);
    assert!(got.iter().all(|c| !c.auto), "auto-tidy is off by default");
}

#[test]
fn thresholds_are_respected() {
    // Done for only one day; idle for only an hour; a ghost with a week left.
    let recent_done = TidySession {
        link: Some(link("done", DAY)),
        ..session(1, 5 * HOUR)
    };
    let short_idle = TidySession {
        link: Some(link("done", 3 * DAY)),
        ..session(2, HOUR)
    };
    let mut far_ghost = TidySession {
        link: Some(link("todo", DAY)),
        ..session(3, 5 * HOUR)
    };
    far_ghost.row.status = "ghost".into();
    far_ghost.row.claude_session_id = Some("c3".into());
    far_ghost.row.lost_at = Some(NOW - 7 * DAY);
    // Done with no known transition time: never guessed old.
    let unknown_change = TidySession {
        link: Some(TidyLink {
            status_changed_at: None,
            ..link("done", 0)
        }),
        ..session(4, 5 * HOUR)
    };
    // Done and idle, but a local item still todo, and a bare key.
    let todo = TidySession {
        link: Some(link("todo", 9 * DAY)),
        ..session(5, 9 * HOUR)
    };
    assert!(run(
        &[recent_done, short_idle, far_ghost, unknown_change, todo],
        &cfg()
    )
    .is_empty());
}

#[test]
fn protection_in_progress_item() {
    let s = TidySession {
        in_progress: true,
        pr_merged: true,
        ..done_session(1)
    };
    assert!(run(&[s], &cfg()).is_empty());
}

#[test]
fn protection_working_blocked_and_failed_sessions() {
    for status in ["working", "blocked", "failed"] {
        let mut s = done_session(1);
        s.row.claude_status = Some(status.into());
        assert!(run(&[s], &cfg()).is_empty(), "{status}");
    }
}

#[test]
fn protection_stuck_including_the_trust_prompt() {
    for kind in ["trust_prompt", "auth_menu", "press_enter"] {
        let mut s = done_session(1);
        s.row.stuck_kind = Some(kind.into());
        assert!(run(&[s], &cfg()).is_empty(), "{kind}");
    }
}

#[test]
fn protection_needs_you_dialog() {
    let mut s = done_session(1);
    s.row.pending_input = Some(PendingInput {
        kind: "permission".into(),
        question: None,
        options: vec![],
    });
    assert!(run(&[s], &cfg()).is_empty());
}

#[test]
fn protection_controller_and_operator() {
    let reachable = local();
    let who = ("local".to_string(), "s1".to_string());
    for (controller, operator) in [(Some(&who), None), (None, Some(&who))] {
        let got = plan_tidy(
            &[done_session(1)],
            &cfg(),
            &TidyContext {
                controller,
                operator,
                reachable: &reachable,
                now: NOW,
            },
        );
        assert!(got.is_empty());
    }
}

#[test]
fn protection_user_touch_within_the_hour() {
    let touched = TidySession {
        last_touch_at: Some(NOW - 10 * 60),
        ..done_session(1)
    };
    assert!(run(&[touched], &cfg()).is_empty());
    // An hour later it is a candidate again.
    let later = TidySession {
        last_touch_at: Some(NOW - HOUR),
        ..done_session(1)
    };
    assert_eq!(run(&[later], &cfg()).len(), 1);
}

#[test]
fn protection_bg_agent_with_open_tasks() {
    let mut bg = TidySession {
        pr_merged: true,
        open_tasks: true,
        ..session(1, DAY)
    };
    bg.row.kind = "bg".into();
    bg.row.worktree_id = None;
    assert!(run(&[bg.clone()], &cfg()).is_empty());
    bg.open_tasks = false;
    assert_eq!(
        reasons(&run(&[bg], &cfg())),
        vec![(1, TidyReason::PrMergedIdle, TidyAction::Kill)],
        "without tasks a bg agent is plain-killed, like the idle killer does"
    );
}

#[test]
fn every_protection_is_named() {
    let reachable = local();
    let ctx = TidyContext {
        controller: None,
        operator: None,
        reachable: &reachable,
        now: NOW,
    };
    let mut s = done_session(1);
    assert_eq!(protection(&s, &ctx), None);
    s.row.safe_kill_state = Some("requested".into());
    assert_eq!(protection(&s, &ctx), Some("safe_kill_in_flight"));
    s.row.safe_kill_state = None;
    s.row.kind = "external".into();
    assert_eq!(protection(&s, &ctx), Some("external"));
}

#[test]
fn secondary_reasons_merge_under_the_highest_ranked() {
    let s = TidySession {
        pr_merged: true,
        ..done_session(1)
    };
    let got = run(&[s], &cfg());
    assert_eq!(got.len(), 1, "one candidate per session");
    assert_eq!(got[0].reason, TidyReason::DoneIdle);
    assert_eq!(got[0].secondary, vec![TidyReason::PrMergedIdle]);
}

#[test]
fn a_shared_worktree_is_never_safe_removed() {
    // Both done and idle, same tree: the idler is a duplicate AND done, and
    // is plain-killed; the recent one is done too, but still shares the tree.
    let mut a = done_session(1);
    a.row.worktree_key = Some("shared".into());
    a.row.idle_since = Some(NOW - DAY);
    let mut b = done_session(2);
    b.row.worktree_key = Some("shared".into());
    let got = run(&[a, b], &cfg());
    assert_eq!(
        reasons(&got),
        vec![
            (1, TidyReason::DoneIdle, TidyAction::Kill),
            (2, TidyReason::DoneIdle, TidyAction::Kill),
        ]
    );
    assert_eq!(got[0].secondary, vec![TidyReason::DuplicateWorktree]);
}

#[test]
fn reviews_are_not_duplicates_of_their_source() {
    let mut review = session(2, DAY);
    review.row.kind = "review".into();
    review.row.worktree_key = Some("wt1".into());
    assert!(run(&[session(1, DAY), review], &cfg()).is_empty());
}

/// A review running in its source's worktree keeps that tree in use: the
/// done source is offered a plain kill, never a safe kill (which would
/// remove the tree under the review). A shell in the same key does not.
#[test]
fn a_review_sibling_keeps_the_tree_shared_but_is_no_duplicate() {
    let source = done_session(1);
    let mut review = session(2, DAY);
    review.row.kind = "review".into();
    review.row.worktree_key = Some("wt1".into());
    let got = run(&[source.clone(), review], &cfg());
    assert_eq!(
        reasons(&got),
        vec![(1, TidyReason::DoneIdle, TidyAction::Kill)]
    );
    assert!(got[0].secondary.is_empty(), "a review is not a duplicate");
    let mut shell = session(3, DAY);
    shell.row.kind = "shell".into();
    shell.row.worktree_key = Some("wt1".into());
    let got = run(&[source, shell], &cfg());
    assert_eq!(
        reasons(&got),
        vec![(1, TidyReason::DoneIdle, TidyAction::SafeKill)]
    );
}

#[test]
fn a_work_row_without_a_tracked_worktree_is_only_archived() {
    let mut s = done_session(1);
    s.row.worktree_id = None;
    assert_eq!(
        reasons(&run(&[s], &cfg())),
        vec![(1, TidyReason::DoneIdle, TidyAction::Archive)]
    );
}

#[test]
fn unreachable_hosts_and_offline_rows_are_skipped() {
    // The control: this row on the reachable host is the candidate.
    assert_eq!(
        reasons(&run(&[done_session(1)], &cfg())),
        vec![(1, TidyReason::DoneIdle, TidyAction::SafeKill)]
    );
    let mut remote = done_session(1);
    remote.row.host_alias = "remote".into();
    // Offline rows on the reachable host: one stopped, and one lost — a
    // ghost with no Claude session to resume, which the ghost arm has no
    // suggestion for either.
    let mut stopped = done_session(2);
    stopped.row.status = "stopped".into();
    let mut lost = done_session(3);
    lost.row.status = "ghost".into();
    lost.row.lost_at = Some(NOW - (14 * DAY - 2 * HOUR));
    lost.row.claude_session_id = None;
    let got = run(&[remote, stopped, lost], &cfg());
    assert!(got.is_empty(), "{:?}", reasons(&got));
}

#[test]
fn snooze_and_never_are_idempotent() {
    let snoozed = |until: i64| TidySession {
        link: Some(TidyLink {
            snoozed_until: Some(until),
            ..link("done", 3 * DAY)
        }),
        ..session(1, 5 * HOUR)
    };
    // Snoozed: not suggested, on this sweep or any later one before it ends.
    for tick in 0..3 {
        let s = snoozed(NOW + 7 * DAY - tick);
        assert!(run(&[s], &cfg()).is_empty(), "tick {tick}");
    }
    // After the snooze it may come back.
    assert_eq!(run(&[snoozed(NOW - 1)], &cfg()).len(), 1);
    let never = TidySession {
        link: Some(TidyLink {
            never: true,
            ..link("done", 30 * DAY)
        }),
        pr_merged: true,
        ..session(1, 30 * DAY)
    };
    assert!(run(&[never], &cfg()).is_empty());
}

/// A snooze or never accepted on a secondary confirmed link (the session's
/// own flags, gathered over every live link) holds the session back exactly
/// as one on its primary.
#[test]
fn a_flag_on_any_live_link_is_honoured() {
    let snoozed = TidySession {
        snoozed_until: Some(NOW + DAY),
        ..done_session(1)
    };
    assert!(run(&[snoozed], &cfg()).is_empty());
    let expired = TidySession {
        snoozed_until: Some(NOW - 1),
        ..done_session(1)
    };
    assert_eq!(run(&[expired], &cfg()).len(), 1);
    let never = TidySession {
        never: true,
        pr_merged: true,
        ..done_session(1)
    };
    assert!(run(&[never], &cfg()).is_empty());
}

#[test]
fn auto_tidy_acts_only_on_allowed_reasons_and_safe_actions() {
    let merged = TidySession {
        pr_merged: true,
        ..session(2, 5 * HOUR)
    };
    let mut dup_a = TidySession {
        pr_merged: false,
        ..session(3, DAY)
    };
    dup_a.row.worktree_key = Some("shared".into());
    let mut dup_b = session(4, 5 * HOUR);
    dup_b.row.worktree_key = Some("shared".into());
    let sessions = [done_session(1), merged, dup_a, dup_b];

    let only_done = TidyConfig {
        auto: true,
        auto_reasons: vec![TidyReason::DoneIdle],
        ..cfg()
    };
    let got = run(&sessions, &only_done);
    let auto: Vec<i64> = auto_selection(&got).iter().map(|c| c.session_id).collect();
    assert_eq!(auto, vec![1], "pr_merged_idle is still only suggested");

    let every = TidyConfig {
        auto: true,
        auto_reasons: TidyReason::RANKED.to_vec(),
        ..cfg()
    };
    let got = run(&sessions, &every);
    let auto: Vec<i64> = auto_selection(&got).iter().map(|c| c.session_id).collect();
    assert_eq!(
        auto,
        vec![1, 2],
        "a plain kill (duplicate) is never automatic"
    );

    let off = TidyConfig {
        auto: false,
        ..every
    };
    assert!(auto_selection(&run(&sessions, &off)).is_empty());
}

#[test]
fn reasons_round_trip_and_unknown_values_read() {
    for r in TidyReason::RANKED {
        assert_eq!(TidyReason::parse(r.as_str()), Some(*r));
        let json = serde_json::to_string(r).unwrap();
        assert_eq!(json, format!("\"{}\"", r.as_str()));
    }
    let later: TidyReason = serde_json::from_str("\"stale_branch\"").unwrap();
    assert_eq!(later, TidyReason::Unknown);
    let action: TidyAction = serde_json::from_str("\"hibernate\"").unwrap();
    assert_eq!(action, TidyAction::Unknown);
}

#[test]
fn an_org_override_decides_auto_tidy_for_its_sessions() {
    let org = |id: i64, org: Option<i64>| {
        let mut s = done_session(id);
        s.row.org_id = org;
        s
    };
    let sessions = [org(1, Some(7)), org(2, Some(8)), org(3, None)];
    let auto_ids = |cfg: &TidyConfig| -> Vec<i64> {
        auto_selection(&run(&sessions, cfg))
            .iter()
            .map(|c| c.session_id)
            .collect()
    };
    let off_but_7 = TidyConfig {
        org_auto: HashMap::from([(7, true)]),
        ..cfg()
    };
    assert_eq!(auto_ids(&off_but_7), vec![1]);
    assert!(off_but_7.auto_anywhere());
    let on_but_8 = TidyConfig {
        auto: true,
        org_auto: HashMap::from([(8, false)]),
        ..cfg()
    };
    assert_eq!(auto_ids(&on_but_8), vec![1, 3]);
    assert!(!cfg().auto_anywhere());
    assert_eq!(run(&sessions, &cfg())[0].org_id, Some(7));
}

// ── idle_unlinked (work graph M11.3) ──────────────────────────────────

/// An unlinked work session, created long ago, idle for `idle` seconds, with
/// no prompt, attach or turn since it went idle.
fn unlinked(id: i64, idle: i64) -> TidySession {
    let mut s = session(id, idle);
    s.row.created_at = NOW - 60 * DAY;
    s.row.started_at = Some(NOW - 60 * DAY);
    s.row.last_turn_at = Some(NOW - idle);
    s.row.last_stop_at = Some(NOW - idle);
    s.last_touch_at = Some(NOW - idle);
    s
}

#[test]
fn idle_unlinked_is_offered_after_the_window_with_a_safe_kill() {
    let got = run(&[unlinked(1, 9 * DAY)], &cfg());
    assert_eq!(
        reasons(&got),
        vec![(1, TidyReason::IdleUnlinked, TidyAction::SafeKill)]
    );
    let c = &got[0];
    assert_eq!(c.since, NOW - 9 * DAY, "the evidence: quiet since");
    assert_eq!(c.idle_secs, 9 * DAY);
    assert_eq!(c.link_id, None);
    assert_eq!(c.key, None);
    assert!(!c.auto);
    // Exactly at the window it counts; a second short of it does not.
    assert_eq!(run(&[unlinked(1, 7 * DAY)], &cfg()).len(), 1);
    assert!(run(&[unlinked(1, 7 * DAY - 1)], &cfg()).is_empty());
    // The window is the setting's.
    let three = TidyConfig {
        unlinked_idle_secs: 3 * DAY,
        ..cfg()
    };
    assert_eq!(run(&[unlinked(1, 4 * DAY)], &three).len(), 1);
}

/// One row per thing that keeps an idle, unlinked session OUT of the sheet —
/// each a mutation of the canonical candidate above, which alone is offered.
#[test]
fn idle_unlinked_exclusions_table() {
    let base = || unlinked(1, 9 * DAY);
    let who = ("local".to_string(), "s1".to_string());
    type Case = (&'static str, Box<dyn Fn() -> TidySession>);
    let cases: Vec<Case> = vec![
        // What it must be: a work session with its own tracked tree.
        (
            "kind shell",
            Box::new(move || {
                let mut s = base();
                s.row.kind = "shell".into();
                s
            }),
        ),
        (
            "kind bg",
            Box::new(move || {
                let mut s = base();
                s.row.kind = "bg".into();
                s
            }),
        ),
        (
            "kind external",
            Box::new(move || {
                let mut s = base();
                s.row.kind = "external".into();
                s
            }),
        ),
        (
            "kind review",
            Box::new(move || {
                let mut s = base();
                s.row.kind = "review".into();
                s
            }),
        ),
        (
            "no tracked worktree",
            Box::new(move || {
                let mut s = base();
                s.row.worktree_id = None;
                s
            }),
        ),
        // No link of any kind.
        (
            "a primary confirmed link",
            Box::new(move || TidySession {
                link: Some(link("todo", 30 * DAY)),
                ..base()
            }),
        ),
        (
            "another live link or a suggestion",
            Box::new(move || TidySession {
                any_link: true,
                ..base()
            }),
        ),
        // Idle, and unused, for the whole window.
        (
            "not idle (no idle stamp)",
            Box::new(move || {
                let mut s = base();
                s.row.idle_since = None;
                s
            }),
        ),
        (
            "idle only 6 days",
            Box::new(move || {
                let mut s = base();
                s.row.idle_since = Some(NOW - 6 * DAY);
                s
            }),
        ),
        (
            "prompted or attached 2 days ago",
            Box::new(move || TidySession {
                last_touch_at: Some(NOW - 2 * DAY),
                ..base()
            }),
        ),
        (
            "a turn finished 3 days ago",
            Box::new(move || {
                let mut s = base();
                s.row.last_turn_at = Some(NOW - 3 * DAY);
                s
            }),
        ),
        (
            "a Stop hook 3 days ago",
            Box::new(move || {
                let mut s = base();
                s.row.last_stop_at = Some(NOW - 3 * DAY);
                s
            }),
        ),
        (
            "created inside the window",
            Box::new(move || {
                let mut s = base();
                s.row.created_at = NOW - 2 * DAY;
                s
            }),
        ),
        (
            "started inside the window",
            Box::new(move || {
                let mut s = base();
                s.row.started_at = Some(NOW - 2 * DAY);
                s
            }),
        ),
        (
            "kept for 3 more days",
            Box::new(move || TidySession {
                kept_until: Some(NOW + 3 * DAY),
                ..base()
            }),
        ),
        (
            "host unreachable",
            Box::new(move || {
                let mut s = base();
                s.row.host_alias = "remote".into();
                s
            }),
        ),
        (
            "a ghost",
            Box::new(move || {
                let mut s = base();
                s.row.status = "ghost".into();
                s
            }),
        ),
        // Every hard-coded protection wins.
        (
            "protection: in_progress",
            Box::new(move || TidySession {
                in_progress: true,
                ..base()
            }),
        ),
        (
            "protection: working",
            Box::new(move || {
                let mut s = base();
                s.row.claude_status = Some("working".into());
                s
            }),
        ),
        (
            "protection: blocked",
            Box::new(move || {
                let mut s = base();
                s.row.claude_status = Some("blocked".into());
                s
            }),
        ),
        (
            "protection: failed",
            Box::new(move || {
                let mut s = base();
                s.row.claude_status = Some("failed".into());
                s
            }),
        ),
        (
            "protection: stuck",
            Box::new(move || {
                let mut s = base();
                s.row.stuck_kind = Some("trust_prompt".into());
                s
            }),
        ),
        (
            "protection: needs you",
            Box::new(move || {
                let mut s = base();
                s.row.pending_input = Some(PendingInput {
                    kind: "question".into(),
                    question: None,
                    options: vec![],
                });
                s
            }),
        ),
        (
            "protection: touched within the hour",
            Box::new(move || TidySession {
                last_touch_at: Some(NOW - 60),
                ..base()
            }),
        ),
        (
            "protection: open tasks",
            Box::new(move || TidySession {
                open_tasks: true,
                ..base()
            }),
        ),
        (
            "protection: safe kill in flight",
            Box::new(move || {
                let mut s = base();
                s.row.safe_kill_state = Some("requested".into());
                s
            }),
        ),
    ];
    assert_eq!(
        run(&[base()], &cfg()).len(),
        1,
        "the base row is a candidate"
    );
    for (name, make) in &cases {
        let got = run(&[make()], &cfg());
        assert!(
            got.iter().all(|c| c.reason != TidyReason::IdleUnlinked),
            "{name}: {got:?}"
        );
    }
    // The controller and the operator, which the context names.
    let reachable = local();
    for (controller, operator) in [(Some(&who), None), (None, Some(&who))] {
        let got = plan_tidy(
            &[base()],
            &cfg(),
            &TidyContext {
                controller,
                operator,
                reachable: &reachable,
                now: NOW,
            },
        );
        assert!(got.is_empty(), "{controller:?} {operator:?}");
    }
    // A kept session is back once the keep ends.
    let lapsed = TidySession {
        kept_until: Some(NOW - 1),
        ..base()
    };
    assert_eq!(run(&[lapsed], &cfg()).len(), 1);
}

#[test]
fn idle_unlinked_never_offers_a_shared_worktree() {
    // Two unlinked sessions on one tree: the idler is a duplicate (a plain
    // kill of a tree in use), never an idle_unlinked kill; the other keeps
    // the tree and is not offered.
    let mut a = unlinked(1, 10 * DAY);
    a.row.worktree_key = Some("shared".into());
    let mut b = unlinked(2, 9 * DAY);
    b.row.worktree_key = Some("shared".into());
    let got = run(&[a, b], &cfg());
    assert_eq!(
        reasons(&got),
        vec![(1, TidyReason::DuplicateWorktree, TidyAction::Kill)]
    );
    assert!(got[0].secondary.is_empty(), "{got:?}");
}

#[test]
fn idle_unlinked_ranks_below_the_work_reasons() {
    let s = TidySession {
        pr_merged: true,
        ..unlinked(1, 9 * DAY)
    };
    let got = run(&[s], &cfg());
    assert_eq!(got[0].reason, TidyReason::PrMergedIdle);
    assert_eq!(got[0].secondary, vec![TidyReason::IdleUnlinked]);
}

#[test]
fn auto_tidy_never_acts_on_idle_unlinked() {
    // D19: on globally, on for the org, every reason in the allowed list —
    // still only suggested.
    let mut s = unlinked(1, 30 * DAY);
    s.row.org_id = Some(7);
    let every = TidyConfig {
        auto: true,
        auto_reasons: TidyReason::RANKED.to_vec(),
        org_auto: HashMap::from([(7, true)]),
        ..cfg()
    };
    let got = run(&[s], &every);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].reason, TidyReason::IdleUnlinked);
    assert!(!got[0].auto);
    assert!(auto_selection(&got).is_empty());
    // A candidate off the wire that claims `auto` is still not selected.
    let forged = TidyCandidate {
        auto: true,
        ..got[0].clone()
    };
    assert!(auto_selection(&[forged]).is_empty());
    assert!(!TidyReason::IdleUnlinked.auto_allowed());
    assert!(!TidyReason::Unknown.auto_allowed());
    assert!(TidyReason::DoneIdle.auto_allowed());
}

#[test]
fn a_keep_holds_every_reason_of_a_live_session() {
    let kept = TidySession {
        kept_until: Some(NOW + DAY),
        ..done_session(1)
    };
    assert!(run(&[kept], &cfg()).is_empty());
}
