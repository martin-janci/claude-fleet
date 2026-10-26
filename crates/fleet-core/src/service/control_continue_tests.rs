use super::*;

const CARD: &str = r#"Which first?

```fleet-ui
{"spec":"fleet.ui/1","kind":"choices","question":"Which first?","options":[
 {"label":"Fix the tests","prompt":"Fix the failing tests first."},
 {"label":"Write the docs","prompt":"Write the docs first.","recommended":true}]}
```
"#;

fn pending(turn_seq: i64, ended_at: i64) -> Pending {
    Pending {
        session_id: 9,
        turn_seq,
        label: "Write the docs".into(),
        prompt: "Write the docs first.".into(),
        ended_at,
        state: PickState::Waiting,
        settled_at: None,
    }
}

fn op(turn_seq: i64) -> OperatorNow {
    OperatorNow {
        id: 9,
        turn_seq,
        live: true,
        busy: false,
        queued: false,
    }
}

#[test]
fn the_recommended_option_of_the_last_choices_card_is_the_pick() {
    let p = recommended_pick(CARD).unwrap();
    assert_eq!(p.label, "Write the docs");
    assert_eq!(p.prompt, "Write the docs first.");
    // Two cards: the last one decides.
    let two = format!(
        "{CARD}\nAnd then:\n```fleet-ui\n{}\n```\n",
        r#"{"spec":"fleet.ui/1","kind":"choices","options":[{"label":"Stop","prompt":"Stop here."}]}"#
    );
    assert!(recommended_pick(&two).is_none());
    // A json fence counts only when it says it is a card.
    let json = CARD.replace("```fleet-ui", "```json");
    assert_eq!(recommended_pick(&json).unwrap().label, "Write the docs");
    let plain = CARD.replace("```fleet-ui", "```js");
    assert!(recommended_pick(&plain).is_none());
    // A fence tagged with the spec's name still counts.
    let alias = CARD.replace("```fleet-ui", "```fleet.ui");
    assert_eq!(recommended_pick(&alias).unwrap().label, "Write the docs");
}

/// An untagged fence is how Control quotes a pane, a log or an issue: a card
/// inside one is drawn (`ui_blocks`) but never sent for the person.
#[test]
fn a_card_quoted_in_an_untagged_fence_is_drawn_but_never_picked() {
    let quoted = CARD.replace("```fleet-ui", "```");
    assert_eq!(ui_blocks(&quoted).len(), 1, "still drawn as a card");
    assert!(recommended_pick(&quoted).is_none());
    // Control's own card before the quote is not the last one either way:
    // the quoted one is skipped, so Control's card decides.
    let both = format!("{CARD}\nThe pane printed:\n{quoted}");
    assert_eq!(recommended_pick(&both).unwrap().label, "Write the docs");
}

#[test]
fn a_risky_option_or_question_an_unclosed_fence_or_plain_text_is_never_picked() {
    assert!(recommended_pick("Just text, no card.").is_none());
    let risky_opt = CARD.replace("Write the docs first.", "Push to origin/main.");
    assert!(recommended_pick(&risky_opt).is_none());
    let risky_q = CARD.replace(
        "\"question\":\"Which first?\"",
        "\"question\":\"Delete the branch?\"",
    );
    assert!(recommended_pick(&risky_q).is_none());
    let unclosed = CARD.trim_end().trim_end_matches("```");
    assert!(recommended_pick(unclosed).is_none());
}

#[test]
fn ask_first_or_the_setting_off_keeps_it_from_running() {
    assert!(enabled_by(true, Position::Brief));
    assert!(enabled_by(true, Position::Autopilot));
    assert!(!enabled_by(true, Position::Ask));
    assert!(!enabled_by(true, Position::Custom));
    assert!(!enabled_by(false, Position::Autopilot));
    let p = pending(4, 0);
    assert_eq!(decide(Some(&p), Some(op(4)), false, 10, 10_000), Step::Drop);
}

/// Review w27-7: a card that ended while auto-continue was off (or the dial
/// at Ask first) is never sent once it is turned on: the Stop hook records
/// no pick while off, and a pass settles one already waiting.
#[tokio::test]
async fn a_card_from_while_it_was_off_is_not_sent_when_it_is_turned_on() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    {
        let s = store.lock().unwrap();
        s.upsert_host("h").unwrap();
        s.set_setting(
            crate::service::operator::SETTING_OPERATOR_SESSION,
            "h/fleet-operator",
        )
        .unwrap();
        let id = s
            .upsert_session("fleet-operator", "h", None, None, 1, 1, "running", None)
            .unwrap();
        // The dial at Within your brief, the setting off.
        s.set_setting(settings::ORCHESTRATOR_MAX_LEVEL, "2")
            .unwrap();
        s.set_setting(settings::CONTROL_AUTO_CONTINUE, "false")
            .unwrap();
        on_stop(&s, id, Some(CARD), 1000).unwrap();
        assert!(load(&s).is_none(), "no pick is recorded while off");
        // A pick recorded while it was on, then it was turned off.
        save(
            &s,
            Some(&Pending {
                session_id: id,
                ..pending(0, 1000)
            }),
        )
        .unwrap();
    }
    let exec = Count(Mutex::new(Vec::new()));
    let later = 1000 + 6 * 3600;
    assert!(!run_with(&store, &exec, later).await);
    {
        let s = store.lock().unwrap();
        assert_eq!(load(&s).unwrap().state, PickState::Superseded);
        s.set_setting(settings::CONTROL_AUTO_CONTINUE, "true")
            .unwrap();
    }
    assert!(!run_with(&store, &exec, later + 30).await);
    assert!(
        exec.0.lock().unwrap().is_empty(),
        "the old card is never sent"
    );
}

#[test]
fn it_sends_once_due_on_a_quiet_operator_and_not_before() {
    let p = pending(4, 1000);
    assert_eq!(
        decide(Some(&p), Some(op(4)), true, 10, 1000 + 599),
        Step::Wait("not_due")
    );
    assert_eq!(
        decide(Some(&p), Some(op(4)), true, 10, 1000 + 600),
        Step::Send
    );
    let busy = OperatorNow {
        busy: true,
        ..op(4)
    };
    assert_eq!(
        decide(Some(&p), Some(busy), true, 10, 5000),
        Step::Wait("operator_busy")
    );
    let queued = OperatorNow {
        queued: true,
        ..op(4)
    };
    assert_eq!(
        decide(Some(&p), Some(queued), true, 10, 5000),
        Step::Wait("operator_busy")
    );
    let down = OperatorNow {
        live: false,
        ..op(4)
    };
    assert_eq!(
        decide(Some(&p), Some(down), true, 10, 5000),
        Step::Wait("operator_not_running")
    );
    // A newer turn, or a day-old card: dropped, never sent.
    assert_eq!(decide(Some(&p), Some(op(5)), true, 10, 5000), Step::Drop);
    assert_eq!(
        decide(Some(&p), Some(op(4)), true, 10, 1000 + STALE_SECS + 1),
        Step::Drop
    );
    // Settled already: nothing.
    let sent = Pending {
        state: PickState::Sent,
        ..p
    };
    assert_eq!(
        decide(Some(&sent), Some(op(4)), true, 10, 5000),
        Step::Nothing
    );
    assert_eq!(decide(None, Some(op(4)), true, 10, 5000), Step::Nothing);
}

#[test]
fn the_prompt_is_fleets_and_names_the_option() {
    let body = prompt_for(&pending(1, 0), 10);
    assert!(body.starts_with("[fleet]"));
    assert!(crate::service::prompt_origin::FLEET_PROMPT_MARKS
        .iter()
        .any(|m| body.starts_with(m)));
    assert!(body.contains("\"Write the docs\"") && body.ends_with("Write the docs first."));
}

struct Count(Mutex<Vec<(i64, String)>>);

#[async_trait::async_trait]
impl ContinueExec for Count {
    async fn queue(&self, session_id: i64, body: String) -> Result<(), IpcError> {
        self.0.lock().unwrap().push((session_id, body));
        Ok(())
    }
}

#[test]
fn take_and_cancel_settle_the_card_so_fleet_never_sends_it_too() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    {
        let s = store.lock().unwrap();
        save(&s, Some(&pending(4, 0))).unwrap();
    }
    let args = |action: &str, turn_seq| AutoContinueArgs {
        action: action.into(),
        turn_seq,
    };
    // A take for another card leaves this one waiting.
    let v = act(&store, &args("take", Some(3)), 10).unwrap();
    assert!(v.pending.is_none());
    let v = act(&store, &args("status", None), 10).unwrap();
    assert_eq!(v.pending.as_ref().unwrap().state, PickState::Waiting);
    assert_eq!(v.due_at, Some(600));
    // Take hands back the prompt once; a second take gets nothing.
    let v = act(&store, &args("take", Some(4)), 10).unwrap();
    assert_eq!(v.pending.unwrap().prompt, "Write the docs first.");
    assert!(act(&store, &args("take", Some(4)), 11)
        .unwrap()
        .pending
        .is_none());
    let v = act(&store, &args("status", None), 12).unwrap();
    assert_eq!(v.pending.unwrap().state, PickState::Taken);
    assert!(act(&store, &args("bogus", None), 12).is_err());
}

#[tokio::test]
async fn a_pass_sends_nothing_without_an_operator_and_never_twice() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    {
        let s = store.lock().unwrap();
        s.set_setting(settings::ORCHESTRATOR_MAX_LEVEL, "2")
            .unwrap();
        save(&s, Some(&pending(4, 0))).unwrap();
    }
    let exec = Count(Mutex::new(Vec::new()));
    assert!(!run_with(&store, &exec, 10_000).await);
    assert!(exec.0.lock().unwrap().is_empty());
    // Cancelled: a pass finds nothing to send.
    act(
        &store,
        &AutoContinueArgs {
            action: "cancel".into(),
            turn_seq: None,
        },
        10,
    )
    .unwrap();
    assert!(!run_with(&store, &exec, 10_000).await);
    let s = store.lock().unwrap();
    assert_eq!(load(&s).unwrap().state, PickState::Cancelled);
}

#[test]
fn a_stop_hook_of_another_session_records_nothing() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let id = s
        .upsert_session("dev", "h", None, None, 1, 1, "running", None)
        .unwrap();
    on_stop(&s, id, Some(CARD), 5).unwrap();
    assert!(load(&s).is_none());
}

#[tokio::test]
async fn pause_all_stops_control_continue() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    {
        let s = store.lock().unwrap();
        s.set_setting(settings::AUTOMATION_PAUSED, "true").unwrap();
        s.set_setting(settings::ORCHESTRATOR_MAX_LEVEL, "2")
            .unwrap();
        save(&s, Some(&pending(4, 0))).unwrap();
    }
    let exec = Count(Mutex::new(Vec::new()));
    assert!(!run_with(&store, &exec, 10_000).await);
    let s = store.lock().unwrap();
    assert_eq!(load(&s).unwrap().state, PickState::Waiting);
}

/// Control at level 2 with its operator session `h/fleet-operator` running;
/// returns that session's id.
fn with_operator(s: &Store) -> i64 {
    s.upsert_host("h").unwrap();
    s.set_setting(settings::ORCHESTRATOR_MAX_LEVEL, "2")
        .unwrap();
    s.set_setting(
        crate::service::operator::SETTING_OPERATOR_SESSION,
        "h/fleet-operator",
    )
    .unwrap();
    s.upsert_session("fleet-operator", "h", None, None, 1, 1, "running", None)
        .unwrap()
}

#[tokio::test]
async fn controls_stop_hook_records_the_pick_and_one_pass_sends_it_once() {
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let id = {
        let s = store.lock().unwrap();
        let id = with_operator(&s);
        on_stop(&s, id, Some(CARD), 1000).unwrap();
        let p = load(&s).unwrap();
        assert_eq!((p.session_id, p.state), (id, PickState::Waiting));
        id
    };
    let exec = Count(Mutex::new(Vec::new()));
    let due = 1000 + 10 * 60;
    {
        let s = store.lock().unwrap();
        let now = operator_now(&s).unwrap();
        assert!(now.live && !now.busy && !now.queued, "{now:?}");
    }
    assert!(
        !run_with(&store, &exec, due - 1).await,
        "not before it is due"
    );
    assert!(run_with(&store, &exec, due).await);
    assert!(!run_with(&store, &exec, due + 30).await, "never twice");
    let q = exec.0.lock().unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].0, id);
    // A turn with no card forgets the sent one.
    let s = store.lock().unwrap();
    assert_eq!(load(&s).unwrap().state, PickState::Sent);
    on_stop(&s, id, Some("done."), due + 60).unwrap();
    assert!(load(&s).is_none());
}

/// Fails the send, after Control finished another turn mid-send when
/// `newer` is set (its Stop hook saved a new card).
struct FailAfter {
    store: Arc<Mutex<Store>>,
    newer: bool,
}

#[async_trait::async_trait]
impl ContinueExec for FailAfter {
    async fn queue(&self, _session_id: i64, _body: String) -> Result<(), IpcError> {
        if self.newer {
            let s = self.store.lock().unwrap();
            save(&s, Some(&pending(5, 2_000))).unwrap();
        }
        Err(IpcError::new(codes::E_INVALID, "ssh down"))
    }
}

#[tokio::test]
async fn a_failed_send_marks_only_its_own_pick_failed() {
    for newer in [false, true] {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        {
            let s = store.lock().unwrap();
            let id = with_operator(&s);
            on_stop(&s, id, Some(CARD), 1000).unwrap();
        }
        let exec = FailAfter {
            store: store.clone(),
            newer,
        };
        assert!(!run_with(&store, &exec, 1000 + 10 * 60).await);
        let s = store.lock().unwrap();
        let p = load(&s).unwrap();
        if newer {
            assert_eq!((p.turn_seq, p.state), (5, PickState::Waiting), "{p:?}");
        } else {
            assert_eq!(p.state, PickState::Failed);
        }
    }
}
