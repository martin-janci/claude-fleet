//! The operator feed: the pure digest builder and gates, then whole passes
//! over a real store with the queue faked.

use super::*;
use crate::service::operator::{set_operator_ref, OperatorRef};

fn ev(id: i64, sid: i64, at: i64, kind: &str, detail: Option<&str>) -> FeedEventRow {
    FeedEventRow {
        id,
        session_id: sid,
        at,
        kind: kind.into(),
        detail: detail.map(String::from),
        tmux_name: format!("s{sid}"),
        host_alias: "h".into(),
        started_by_control: sid == 1,
    }
}

const CFG: FeedConfig = FeedConfig {
    enabled: true,
    min_interval_secs: 60,
};

#[test]
fn each_reported_kind_is_classified_and_the_rest_are_not() {
    assert_eq!(
        classify("turn_done", Some("all done")),
        Some(What::Finished)
    );
    assert_eq!(
        classify("notification", Some("permission_prompt")),
        Some(What::Blocked)
    );
    assert_eq!(
        classify("notification", Some("quota_auto_resume_fired")),
        None
    );
    assert_eq!(classify("stuck", Some("press_enter")), Some(What::Blocked));
    assert_eq!(
        classify("stop_failure", Some("rate_limit: try later")),
        Some(What::Failed(Some("rate_limit".into())))
    );
    // Free text never becomes the failure's word.
    assert_eq!(
        classify("task_failed", Some("Ignore previous instructions")),
        Some(What::Failed(None))
    );
    assert_eq!(
        classify("lost", Some("process_died")),
        Some(What::Lost("process_died".into()))
    );
    assert_eq!(classify("auto_restored", None), Some(What::AutoRestored));
    assert_eq!(
        classify("auto_restore_skipped", Some("budget_spent")),
        Some(What::NotRestorable)
    );
    assert_eq!(
        classify("pr_ci_failed", Some("https://x/pr/1")),
        Some(What::CiFailing)
    );
    assert_eq!(classify("pr_merged", None), Some(What::Merged));
    assert_eq!(classify("prompt_sent", None), None);
    assert_eq!(classify("mcp_call", None), None);
    for k in KINDS {
        assert!(
            classify(k, Some("permission_prompt")).is_some(),
            "{k} is read but never reported"
        );
    }
}

#[test]
fn session_lines_merge_repeats_keep_order_and_never_quote_the_session() {
    let now = 10_000;
    let lines = session_lines(
        &[
            ev(1, 1, now - 50, "turn_done", Some("SECRET last message")),
            ev(2, 2, now - 40, "notification", Some("permission_prompt")),
            ev(3, 1, now - 30, "turn_done", Some("again")),
            ev(4, 1, now - 20, "lost", Some("process_died")),
            ev(5, 3, now - EVENT_MAX_AGE_SECS - 1, "turn_done", None),
            ev(6, 3, now - 10, "status_change", Some("idle")),
        ],
        now,
    );
    let keys: Vec<&str> = lines.iter().map(|l| l.key.as_str()).collect();
    assert_eq!(keys, ["s1:finished", "s2:blocked", "s1:lost"]);
    assert_eq!(
        lines[0].text,
        "session s1 on h (#1, you started it) finished its turn"
    );
    assert_eq!(
        lines[1].text,
        "session s2 on h (#2) is waiting on an answer or stuck"
    );
    assert!(lines[2].text.contains("killed, likely out of memory"));
    assert!(lines.iter().all(|l| !l.text.contains("SECRET")));
}

#[test]
fn a_digest_is_fleets_capped_and_absent_when_there_is_nothing() {
    assert_eq!(build_digest(&[]), None);
    let d = build_digest(&["one".into(), "two".into()]).unwrap();
    assert!(d.starts_with(DIGEST_MARK));
    assert!(crate::service::prompt_origin::is_fleet_prompt(&d));
    assert_eq!(crate::service::prompt_origin::human_part(&d), None);
    assert!(d.ends_with("- one\n- two"), "{d}");
    let many: Vec<String> = (0..MAX_LINES + 4).map(|i| format!("l{i}")).collect();
    let d = build_digest(&many).unwrap();
    assert_eq!(
        d.lines().filter(|l| l.starts_with("- l")).count(),
        MAX_LINES
    );
    assert!(d.ends_with("- … and 4 more."), "{d}");
}

#[test]
fn the_gate_holds_resets_and_sends_as_documented() {
    let now = 100_000;
    let fresh = FeedState::default();
    let off = FeedConfig {
        enabled: false,
        ..CFG
    };
    assert_eq!(gate(&off, &fresh, Some(true), false, now), Gate::Reset);
    assert_eq!(gate(&CFG, &fresh, None, false, now), Gate::Reset);
    assert_eq!(
        gate(&CFG, &fresh, Some(false), false, now),
        Gate::Hold("operator_not_running")
    );
    assert_eq!(
        gate(&CFG, &fresh, Some(true), true, now),
        Gate::Hold("digest_pending")
    );
    assert_eq!(gate(&CFG, &fresh, Some(true), false, now), Gate::Send);
    let just = FeedState {
        sent: vec![now - 30],
        ..Default::default()
    };
    assert_eq!(
        gate(&CFG, &just, Some(true), false, now),
        Gate::Hold("min_interval")
    );
    let busy_hour = FeedState {
        sent: (1..=MAX_DIGESTS_PER_HOUR as i64)
            .map(|i| now - 3000 + i * 61)
            .collect(),
        ..Default::default()
    };
    let no_wait = FeedConfig {
        min_interval_secs: 0,
        ..CFG
    };
    assert_eq!(
        gate(&no_wait, &busy_hour, Some(true), false, now),
        Gate::Hold("hourly_cap")
    );
    let old_hour = FeedState {
        sent: vec![now - 3700; MAX_DIGESTS_PER_HOUR],
        ..Default::default()
    };
    assert_eq!(
        gate(&no_wait, &old_hour, Some(true), false, now),
        Gate::Send
    );
}

#[test]
fn a_down_host_reports_only_that_it_is_down() {
    assert_eq!(
        host_conditions("a", false, Some(97), Some(3.0))
            .iter()
            .map(|c| c.key.as_str())
            .collect::<Vec<_>>(),
        ["h:a:down"]
    );
    let c = host_conditions("a", true, Some(97), Some(3.4));
    assert_eq!(c.len(), 2);
    assert_eq!(c[0].text, "host a is low on disk (97% used)");
    assert_eq!(c[1].text, "host a is low on memory (3% available)");
    assert!(host_conditions("a", true, None, None).is_empty());
    assert!(mission_condition(4, "Ship", "sign_grant")
        .text
        .contains("autonomy grant"));
}

// ------------------------------------------------------------ whole passes

#[derive(Default)]
struct Fake {
    sent: Mutex<Vec<(i64, String)>>,
    /// Leave each digest queued (the operator busy) rather than typed.
    keep_queued: Option<Arc<Mutex<Store>>>,
}

#[async_trait]
impl FeedExec for Fake {
    async fn queue(&self, session_id: i64, body: String) -> Result<(), IpcError> {
        if let Some(store) = &self.keep_queued {
            store
                .lock()
                .unwrap()
                .insert_deferred_prompt(session_id, &body, 1)
                .unwrap();
        }
        self.sent.lock().unwrap().push((session_id, body));
        Ok(())
    }
}

/// A store with the operator (`op`), a session it started (`child`), one it
/// handed work to (`handed`) and one it has nothing to do with (`other`).
struct World {
    store: Arc<Mutex<Store>>,
    op: i64,
    child: i64,
    handed: i64,
    other: i64,
}

fn world() -> World {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    s.conn_ref()
        .execute("UPDATE hosts SET reachable = 1 WHERE alias = 'h'", [])
        .unwrap();
    // Created now: a session nothing happened on for a day is no longer
    // followed (F10).
    let t0 = crate::store::now_unix();
    let mk = |name: &str| {
        s.upsert_session(name, "h", None, None, t0, t0, "running", None)
            .unwrap()
    };
    let op = mk("fleet-operator");
    let child = mk("child");
    let handed = mk("handed");
    let other = mk("other");
    set_operator_ref(
        &s,
        &OperatorRef {
            host_alias: "h".into(),
            tmux_name: "fleet-operator".into(),
        },
    )
    .unwrap();
    s.conn_ref()
        .execute(
            "UPDATE sessions SET origin = 'operator' WHERE id = ?1",
            [child],
        )
        .unwrap();
    s.insert_control_handoff(
        &crate::store::NewHandoff {
            kind: "session",
            tool: "send_prompt".into(),
            session_id: Some(handed),
            ..Default::default()
        },
        1,
    )
    .unwrap();
    World {
        store: Arc::new(Mutex::new(s)),
        op,
        child,
        handed,
        other,
    }
}

fn event(w: &World, sid: i64, kind: &str, detail: Option<&str>) {
    w.store
        .lock()
        .unwrap()
        .insert_session_event(sid, kind, detail)
        .unwrap();
}

#[tokio::test]
async fn a_pass_tells_the_operator_about_its_sessions_only_and_once() {
    let w = world();
    let fake = Fake::default();
    let now = crate::store::now_unix();
    event(&w, w.child, "turn_done", None);
    // The first pass starts from now: what happened before is not replayed.
    assert_eq!(
        run_with(&w.store, &fake, &CFG, now).await,
        FeedReport::default()
    );
    assert!(fake.sent.lock().unwrap().is_empty());

    event(&w, w.child, "turn_done", Some("the last words"));
    event(&w, w.handed, "stop_failure", Some("rate_limit: later"));
    event(&w, w.other, "turn_done", None);
    event(&w, w.op, "turn_done", None);
    let r = run_with(&w.store, &fake, &CFG, now).await;
    assert!(r.sent, "{r:?}");
    assert_eq!(r.lines, 2);
    let sent = fake.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    let (to, body) = &sent[0];
    assert_eq!(*to, w.op);
    assert!(body.contains(&format!(
        "session child on h (#{}, you started it) finished",
        w.child
    )));
    assert!(body.contains(&format!(
        "session handed on h (#{}) failed (rate_limit)",
        w.handed
    )));
    assert!(!body.contains("other"), "{body}");
    assert!(!body.contains("fleet-operator"), "{body}");
    assert!(!body.contains("the last words"), "{body}");

    // Nothing new: nothing sent, even past the interval.
    let r = run_with(&w.store, &fake, &CFG, now + 120).await;
    assert!(!r.sent);
    assert_eq!(fake.sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn changes_inside_the_interval_wait_and_go_out_together() {
    let w = world();
    let fake = Fake::default();
    let now = crate::store::now_unix();
    run_with(&w.store, &fake, &CFG, now).await;
    event(&w, w.child, "stuck", Some("press_enter"));
    assert!(run_with(&w.store, &fake, &CFG, now).await.sent);
    event(&w, w.child, "lost", Some("process_died"));
    let r = run_with(&w.store, &fake, &CFG, now + 10).await;
    assert_eq!(r.held, Some("min_interval"));
    event(&w, w.child, "auto_restored", Some("process_died"));
    let r = run_with(&w.store, &fake, &CFG, now + 61).await;
    assert!(r.sent);
    let body = fake.sent.lock().unwrap()[1].1.clone();
    assert!(body.contains("was lost"), "{body}");
    assert!(body.contains("brought back automatically"), "{body}");
}

#[tokio::test]
async fn a_digest_still_queued_holds_the_next_one() {
    let w = world();
    let fake = Fake {
        keep_queued: Some(Arc::clone(&w.store)),
        ..Default::default()
    };
    let now = crate::store::now_unix();
    run_with(&w.store, &fake, &CFG, now).await;
    event(&w, w.child, "stop_failure", None);
    assert!(run_with(&w.store, &fake, &CFG, now).await.sent);
    event(&w, w.child, "stuck", Some("press_enter"));
    let r = run_with(&w.store, &fake, &CFG, now + 600).await;
    assert_eq!(r.held, Some("digest_pending"));
    assert_eq!(fake.sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_host_going_down_is_told_once_until_it_comes_back() {
    let w = world();
    let fake = Fake::default();
    let now = crate::store::now_unix();
    run_with(&w.store, &fake, &CFG, now).await;
    let reach = |up: bool| {
        w.store
            .lock()
            .unwrap()
            .conn_ref()
            .execute("UPDATE hosts SET reachable = ?1 WHERE alias = 'h'", [up])
            .unwrap();
    };
    reach(false);
    assert!(run_with(&w.store, &fake, &CFG, now).await.sent);
    assert!(fake.sent.lock().unwrap()[0]
        .1
        .contains("host h is unreachable"));
    assert!(!run_with(&w.store, &fake, &CFG, now + 120).await.sent);
    reach(true);
    assert!(!run_with(&w.store, &fake, &CFG, now + 240).await.sent);
    reach(false);
    assert!(run_with(&w.store, &fake, &CFG, now + 360).await.sent);
    assert_eq!(fake.sent.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn off_or_without_an_operator_nothing_is_kept_or_sent() {
    let w = world();
    let fake = Fake::default();
    let now = crate::store::now_unix();
    run_with(&w.store, &fake, &CFG, now).await;
    event(&w, w.child, "turn_done", None);
    let off = FeedConfig {
        enabled: false,
        ..CFG
    };
    assert_eq!(
        run_with(&w.store, &fake, &off, now).await,
        FeedReport::default()
    );
    // Turning it back on starts from then: the event while off is not told.
    run_with(&w.store, &fake, &CFG, now).await;
    assert!(!run_with(&w.store, &fake, &CFG, now).await.sent);
    assert!(fake.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn pause_all_stops_the_operator_feed() {
    let w = world();
    let fake = Fake::default();
    let now = crate::store::now_unix();
    run_with(&w.store, &fake, &CFG, now).await;
    event(&w, w.child, "turn_done", None);
    settings::set(
        &w.store.lock().unwrap(),
        settings::AUTOMATION_PAUSED,
        "true",
    )
    .unwrap();
    let r = run_with(&w.store, &fake, &CFG, now).await;
    assert_eq!(r.held, Some("paused"));
    assert!(fake.sent.lock().unwrap().is_empty());
    assert!(!crate::service::loops::gate(
        "operator_feed",
        &w.store,
        None
    ));
}

// ------------------------------------------------- F10 / F11 (Control review)

#[test]
fn only_what_needs_control_sends_a_digest() {
    for w in [What::Finished, What::AutoRestored, What::Merged] {
        assert!(!w.needs_control(), "{w:?}");
    }
    for w in [
        What::Blocked,
        What::Failed(None),
        What::Lost("process_died".into()),
        What::NotRestorable,
        What::CiFailing,
    ] {
        assert!(w.needs_control(), "{w:?}");
    }
}

#[test]
fn fold_keeps_routine_lines_and_counts_what_grew_old() {
    let now = 100_000;
    let mut st = FeedState::default();
    fold(
        &mut st,
        &[
            ev(1, 1, now - 50, "turn_done", None),
            ev(2, 1, now - 40, "turn_done", None),
            ev(3, 2, now - EVENT_MAX_AGE_SECS - 5, "stop_failure", None),
            ev(4, 3, now - EVENT_MAX_AGE_SECS - 9, "turn_done", None),
        ],
        now,
    );
    assert_eq!(st.backlog.len(), 1, "two turns of one session are one line");
    assert!(!st.backlog[0].urgent);
    assert_eq!(st.backlog[0].at, now - 40);
    assert!(st.away.urgent, "an old failure still needs Control");
    assert_eq!(st.away.counts.get("failed"), Some(&1));
    assert_eq!(st.away.counts.get("finished"), Some(&1));
    assert!(needs_control(&st, 0));
    let line = away_line(&st.away).unwrap();
    assert!(line.starts_with("while you were away"), "{line}");
    assert!(
        line.contains("1 failures") && line.contains("1 finished turns"),
        "{line}"
    );
    assert!(line.contains("s2, s3"), "{line}");
    // A held line that grows old moves to the count.
    let mut st2 = FeedState::default();
    fold(&mut st2, &[ev(5, 1, now - 10, "turn_done", None)], now);
    assert!(!needs_control(&st2, 0));
    fold(&mut st2, &[], now + EVENT_MAX_AGE_SECS);
    assert!(st2.backlog.is_empty());
    assert_eq!(st2.away.sessions, ["s1".to_string()].into_iter().collect());
    assert!(!needs_control(&st2, 0));
    assert!(needs_control(&st2, 1), "a fresh condition does");
}

#[test]
fn the_digest_puts_what_needs_control_first() {
    let now = 100_000;
    let mut st = FeedState::default();
    fold(
        &mut st,
        &[
            ev(1, 1, now - 50, "turn_done", None),
            ev(2, 2, now - 40, "lost", Some("host_reboot")),
        ],
        now,
    );
    let lines = digest_lines(&st, &["host h is unreachable".into()]);
    assert!(lines[0].contains("was lost"), "{lines:?}");
    assert_eq!(lines[1], "host h is unreachable");
    assert!(lines[2].contains("finished its turn"), "{lines:?}");
}

#[tokio::test]
async fn a_finished_turn_waits_for_something_that_needs_control() {
    let w = world();
    let fake = Fake::default();
    let now = crate::store::now_unix();
    run_with(&w.store, &fake, &CFG, now).await;
    event(&w, w.child, "turn_done", None);
    let r = run_with(&w.store, &fake, &CFG, now).await;
    assert_eq!(r.held, Some("nothing_urgent"));
    assert!(fake.sent.lock().unwrap().is_empty());
    // Past the interval, still nothing: a finished turn alone never sends.
    assert!(!run_with(&w.store, &fake, &CFG, now + 600).await.sent);
    event(&w, w.handed, "stop_failure", Some("rate_limit: later"));
    let r = run_with(&w.store, &fake, &CFG, now + 601).await;
    assert!(r.sent, "{r:?}");
    let body = fake.sent.lock().unwrap()[0].1.clone();
    let fail = body.find("failed (rate_limit)").unwrap();
    let done = body.find("finished its turn").unwrap();
    assert!(fail < done, "what needs Control comes first: {body}");
    // Told once: the backlog went with it.
    event(&w, w.handed, "stop_failure", None);
    run_with(&w.store, &fake, &CFG, now + 700).await;
    let second = fake.sent.lock().unwrap()[1].1.clone();
    assert!(!second.contains("finished its turn"), "{second}");
}

#[tokio::test]
async fn a_session_control_no_longer_follows_is_not_told() {
    let w = world();
    let fake = Fake::default();
    let now = crate::store::now_unix();
    run_with(&w.store, &fake, &CFG, now).await;
    // Muted by the person: its failure is not told.
    w.store
        .lock()
        .unwrap()
        .set_control_mute("session", w.child, true, now)
        .unwrap();
    event(&w, w.child, "stop_failure", None);
    let r = run_with(&w.store, &fake, &CFG, now).await;
    assert!(!r.sent, "{r:?}");
    // Merged: told once, then nothing more about it.
    event(&w, w.handed, "pr_merged", Some("https://x/pr/1"));
    event(&w, w.handed, "stop_failure", None);
    let r = run_with(&w.store, &fake, &CFG, now).await;
    assert!(
        !r.sent,
        "a merge alone is routine and the failure after it is not followed"
    );
    event(&w, w.child, "stuck", None);
    w.store
        .lock()
        .unwrap()
        .set_control_mute("session", w.child, false, now)
        .unwrap();
    event(&w, w.child, "stuck", None);
    let r = run_with(&w.store, &fake, &CFG, now).await;
    assert!(r.sent, "{r:?}");
    let body = fake.sent.lock().unwrap()[0].1.clone();
    assert!(body.contains("had its pull request merged"), "{body}");
    assert!(
        !body.contains(&format!("session handed on h (#{}) failed", w.handed)),
        "{body}"
    );
}
