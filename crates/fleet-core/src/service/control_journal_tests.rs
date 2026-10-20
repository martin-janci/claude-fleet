//! Control's journal: the pure note, then whole passes over a real store
//! with the queue faked.

use super::*;
use crate::service::operator::{set_operator_ref, OperatorRef};

fn follow(kind: &str, id: i64, state: FollowState) -> FollowLine {
    FollowLine {
        follow: Follow {
            kind: kind.into(),
            id,
            state,
            name: format!("n{id}"),
            host_alias: (kind == "session").then(|| "h".into()),
            muted: state == FollowState::Muted,
        },
        started_by_control: id == 1,
        status: Some("idle".into()),
        brief_at: Some(1_000),
        brief: Some("fix the build".into()),
        progress: None,
    }
}

fn entry(kind: &str, at: i64, text: &str) -> JournalEntry {
    JournalEntry {
        id: at,
        at,
        kind: kind.into(),
        text: text.into(),
        resolved_at: None,
    }
}

#[test]
fn an_empty_journal_says_nothing() {
    assert_eq!(render(&JournalView::default(), 0), None);
    // Only threads no longer followed: still nothing to pick up from.
    let v = JournalView {
        follows: vec![follow("session", 2, FollowState::Quiet)],
        ..Default::default()
    };
    assert_eq!(render(&v, 0), None);
}

#[test]
fn the_note_is_fleets_and_names_what_is_followed_asked_and_said() {
    let v = JournalView {
        follows: vec![
            follow("session", 1, FollowState::Following),
            follow("mission", 4, FollowState::Following),
            follow("session", 2, FollowState::Merged),
            follow("session", 3, FollowState::Muted),
        ],
        asks: vec![entry("ask", 1_500, "Start the review?")],
        said: Some(entry("said", 1_600, "and keep an eye on CI")),
        notes: vec![entry("note", 1_700, "#4 waits on 14.13")],
    };
    let n = render(&v, 1_000 + 7_200).unwrap();
    assert!(n.starts_with(JOURNAL_MARK));
    assert!(crate::service::prompt_origin::is_fleet_prompt(&n));
    assert!(
        n.contains("- session n1 on h (#1, you started it): idle; your last brief 2 h ago: \"fix the build\""),
        "{n}"
    );
    assert!(n.contains("- mission #4 \"n4\""), "{n}");
    assert!(n.contains("No longer followed: 1 merged, 1 muted."), "{n}");
    assert!(n.contains("- Start the review? (1 h ago)"), "{n}");
    assert!(
        n.contains("The person last said (1 h ago): \"and keep an eye on CI\""),
        "{n}"
    );
    assert!(n.contains("- #4 waits on 14.13"), "{n}");
}

#[test]
fn ago_reads_in_the_largest_unit() {
    assert_eq!(ago(-5), "just now");
    assert_eq!(ago(59), "just now");
    assert_eq!(ago(240), "4 min ago");
    assert_eq!(ago(3 * 3600), "3 h ago");
    assert_eq!(ago(3 * 86_400), "3 d ago");
}

// ------------------------------------------------------------ whole passes

#[derive(Default)]
struct Fake {
    sent: Mutex<Vec<(i64, String)>>,
}

#[async_trait]
impl JournalExec for Fake {
    async fn queue(&self, session_id: i64, body: String) -> Result<(), IpcError> {
        self.sent.lock().unwrap().push((session_id, body));
        Ok(())
    }
}

struct World {
    store: Arc<Mutex<Store>>,
    op: i64,
    child: i64,
}

fn world() -> World {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let now = crate::store::now_unix();
    let mk = |name: &str| {
        s.upsert_session(name, "h", None, None, now, now, "running", None)
            .unwrap()
    };
    let op = mk("fleet-operator");
    let child = mk("child");
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
    World {
        store: Arc::new(Mutex::new(s)),
        op,
        child,
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
async fn a_new_conversation_gets_the_journal_once() {
    let w = world();
    let fake = Fake::default();
    let now = crate::store::now_unix();
    // The first pass only starts the cursor.
    assert_eq!(
        run_with(&w.store, &fake, now).await,
        JournalReport::default()
    );
    // Another session's start, a turn of Control's: nothing.
    event(&w, w.child, "conversation_started", Some("startup"));
    event(&w, w.op, "turn_done", None);
    assert!(!run_with(&w.store, &fake, now).await.sent);
    // Control compacts: the journal goes, naming the session it started.
    event(&w, w.op, "compact_done", Some("auto"));
    let r = run_with(&w.store, &fake, now).await;
    assert!(r.sent, "{r:?}");
    let sent = fake.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, w.op);
    assert!(sent[0].1.contains(&format!(
        "session child on h (#{}, you started it)",
        w.child
    )));
    // Read once: the same event does not send it again.
    assert!(!run_with(&w.store, &fake, now).await.sent);
    // A /clear later: again.
    event(&w, w.op, "conversation_started", Some("clear"));
    assert!(run_with(&w.store, &fake, now).await.sent);
    assert_eq!(fake.sent.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn an_empty_journal_or_the_setting_off_sends_nothing() {
    let w = world();
    let fake = Fake::default();
    let now = crate::store::now_unix();
    run_with(&w.store, &fake, now).await;
    // Muted: nothing followed, nothing said: nothing to send.
    w.store
        .lock()
        .unwrap()
        .set_control_mute("session", w.child, true, now)
        .unwrap();
    event(&w, w.op, "conversation_started", Some("startup"));
    assert_eq!(run_with(&w.store, &fake, now).await.held, Some("empty"));
    settings::set(&w.store.lock().unwrap(), settings::CONTROL_JOURNAL, "false").unwrap();
    w.store
        .lock()
        .unwrap()
        .set_control_mute("session", w.child, false, now)
        .unwrap();
    event(&w, w.op, "conversation_started", Some("clear"));
    assert_eq!(
        run_with(&w.store, &fake, now).await,
        JournalReport::default()
    );
    assert!(fake.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn pause_all_stops_the_control_journal() {
    let w = world();
    let fake = Fake::default();
    let now = crate::store::now_unix();
    run_with(&w.store, &fake, now).await;
    event(&w, w.op, "conversation_started", Some("clear"));
    settings::set(
        &w.store.lock().unwrap(),
        settings::AUTOMATION_PAUSED,
        "true",
    )
    .unwrap();
    assert_eq!(run_with(&w.store, &fake, now).await.held, Some("paused"));
    assert!(fake.sent.lock().unwrap().is_empty());
    assert!(!crate::service::loops::gate(
        "control_journal",
        &w.store,
        None
    ));
}

#[test]
fn only_the_persons_words_to_control_are_kept() {
    let w = world();
    let s = w.store.lock().unwrap();
    let op = s.get_session_by_id(w.op).unwrap().unwrap();
    let child = s.get_session_by_id(w.child).unwrap().unwrap();
    on_prompt(&s, &child, "not Control", 10);
    assert_eq!(s.last_journal_said().unwrap(), None);
    on_prompt(&s, &op, "  watch\nthe deploy  ", 11);
    assert_eq!(
        s.last_journal_said().unwrap().unwrap().text,
        "watch the deploy"
    );
}

#[test]
fn add_and_mute_check_their_words() {
    let w = world();
    assert!(add(&w.store, "note", "keep #4 in mind", 1).is_ok());
    assert_eq!(
        add(&w.store, "said", "x", 1).unwrap_err().code,
        codes::E_INVALID
    );
    assert_eq!(
        add(&w.store, "ask", "  ", 1).unwrap_err().code,
        codes::E_INVALID
    );
    // Muted now: a mute older than the session is an earlier session's.
    let now = crate::store::now_unix();
    assert!(mute(&w.store, "session", w.child, true, now).unwrap());
    assert!(!mute(&w.store, "session", w.child, true, now).unwrap());
    assert_eq!(
        mute(&w.store, "task", 1, true, 1).unwrap_err().code,
        codes::E_INVALID
    );
    let v = view(&w.store.lock().unwrap(), 2);
    let f = v.follows.iter().find(|l| l.follow.id == w.child).unwrap();
    assert!(f.follow.muted);
    assert_eq!(f.follow.state, FollowState::Muted);
    assert_eq!(v.notes[0].text, "keep #4 in mind");
}

#[tokio::test]
async fn a_read_error_leaves_the_cursor_and_the_next_pass_still_sends() {
    let w = world();
    let fake = Fake::default();
    let now = crate::store::now_unix();
    run_with(&w.store, &fake, now).await;
    event(&w, w.op, "compact_done", Some("auto"));
    let before = JournalState::load(&w.store.lock().unwrap());
    assert!(before.cursor.is_some());
    // The events cannot be read: nothing sent, the cursor neither rewound
    // to 0 nor moved past the new conversation.
    let rename = |from: &str, to: &str| {
        w.store
            .lock()
            .unwrap()
            .conn_ref()
            .execute_batch(&format!("ALTER TABLE {from} RENAME TO {to}"))
            .unwrap();
    };
    rename("session_events", "session_events_away");
    assert!(!run_with(&w.store, &fake, now).await.sent);
    assert_eq!(JournalState::load(&w.store.lock().unwrap()), before);
    rename("session_events_away", "session_events");
    assert!(run_with(&w.store, &fake, now).await.sent);
    assert_eq!(fake.sent.lock().unwrap().len(), 1);
}

/// Review w04-11: a steer from the Mission room makes the mission one of
/// Control's threads, so the feed tells Control what happens on it next.
#[test]
fn a_steer_follows_its_mission() {
    let w = world();
    let m = w
        .store
        .lock()
        .unwrap()
        .create_mission(
            &crate::store::NewMission {
                name: "m",
                goal: "g",
                ..Default::default()
            },
            "fleet",
        )
        .unwrap()
        .id;
    assert!(!w
        .store
        .lock()
        .unwrap()
        .handed_off_mission_ids()
        .unwrap()
        .contains(&m));
    let args = |thread: &str, id: i64| JournalArgs {
        action: Some("follow".into()),
        thread: Some(thread.into()),
        id: Some(id),
        text: Some("pause it when wave 2 finishes".into()),
    };
    assert!(call(&w.store, &args("mission", m), 5).is_ok());
    let s = w.store.lock().unwrap();
    assert!(s.handed_off_mission_ids().unwrap().contains(&m));
    let h = &s.list_control_handoffs(1).unwrap()[0];
    assert_eq!((h.tool.as_str(), h.mission_id), ("steer", Some(m)));
    drop(s);
    assert_eq!(
        call(&w.store, &args("session", 1), 5).unwrap_err().code,
        codes::E_INVALID
    );
    assert_eq!(
        call(&w.store, &args("mission", 9_999), 5).unwrap_err().code,
        codes::E_NOTFOUND
    );
}
