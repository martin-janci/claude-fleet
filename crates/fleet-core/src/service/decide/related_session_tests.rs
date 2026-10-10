//! N1 `related_session` (redesign 6.9 part 2) over a scripted backend: the
//! gate, shadow and assist, who is a candidate (the same person's live
//! sessions only), and asking once per input. No test reaches TypeSafe.

use super::related_session::*;
use super::*;
use crate::store::{DecisionRunFilter, DecisionRunRow, StartSource};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

const KEY: &str = "tsk_test_0123456789abcdefghijklmnopqrstuv";

#[derive(Default)]
struct Fake {
    script: Mutex<VecDeque<Result<JevResponse, BackendError>>>,
    calls: AtomicUsize,
    seen: Mutex<Vec<JevRequest>>,
}

impl Fake {
    fn answering(answers: Vec<Result<JevResponse, BackendError>>) -> Arc<Fake> {
        Arc::new(Fake {
            script: Mutex::new(answers.into()),
            ..Default::default()
        })
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl DecisionBackend for Fake {
    fn provider(&self) -> &'static str {
        PROVIDER_JEV
    }
    async fn ask(
        &self,
        _key: &Secret,
        _model: &str,
        req: &JevRequest,
        _timeout: Duration,
    ) -> Result<JevResponse, BackendError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(req.clone());
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(BackendError::Transport("script ran out".into())))
    }
}

/// A choice answer, all the probability on `choice`'s side.
fn says(choice: &str, confidence: f64) -> Result<JevResponse, BackendError> {
    let other = if choice == NONE_OPTION {
        "s1"
    } else {
        NONE_OPTION
    };
    Ok(serde_json::from_value(serde_json::json!({
        "model": "jev-1.13.0",
        "answers": { "q": {
            "type": "choice",
            "choice": choice,
            "probabilities": { choice: confidence, other: 1.0 - confidence },
            "confidence": confidence,
        }},
        "usage": { "input_tokens": 90, "output_tokens": 2 },
    }))
    .unwrap())
}

struct World {
    store: Arc<Mutex<Store>>,
    /// Ada's session three turns in, about refund retries.
    me: i64,
    /// Ada's other live session, also on refund retries.
    twin: i64,
    /// Bob's session on the same thing: never a candidate.
    bobs: i64,
}

fn session(s: &Store, name: &str, owner: i64, prompt: &str, turns: i64) -> i64 {
    let id = s
        .upsert_session(name, "h", None, None, 1, 1, "running", None)
        .unwrap();
    s.conn_ref()
        .execute(
            "UPDATE sessions SET owner_person_id = ?1, visibility = 'private' WHERE id = ?2",
            rusqlite::params![owner, id],
        )
        .unwrap();
    let conv = format!("{id:08}-cccc-cccc-cccc-cccccccccccc");
    s.rebind_conversation(id, &conv, StartSource::Fleet, None, None)
        .unwrap();
    for _ in 0..turns {
        s.conversation_bump_turns(id, &conv).unwrap();
    }
    s.conversation_set_first_prompt(id, &conv, prompt).unwrap();
    id
}

fn world() -> World {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let ada = s.create_person("ada", None).unwrap().id;
    let bob = s.create_person("bob", None).unwrap().id;
    let turns = crate::service::work::nudge::NUDGE_AFTER_TURNS;
    let me = session(
        &s,
        "a1",
        ada,
        "the refund retries fail on the second attempt",
        turns,
    );
    let twin = session(&s, "a2", ada, "fix the second refund retry", 1);
    let bobs = session(&s, "b1", bob, "refund retries fail twice", 1);
    World {
        store: Arc::new(Mutex::new(s)),
        me,
        twin,
        bobs,
    }
}

impl World {
    fn ctx(&self, fake: &Arc<Fake>) -> DecideCtx {
        DecideCtx::new(Arc::clone(&self.store), fake.clone())
            .with_clock(Arc::new(crate::service::catalog::now_secs))
    }
    fn on(&self, mode: &str) {
        let s = self.store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_RELATED_SESSION, mode).unwrap();
        settings::set(&s, settings::DECIDE_JEV_UNASSIGNED, "true").unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    fn runs(&self) -> Vec<DecisionRunRow> {
        self.store
            .lock()
            .unwrap()
            .list_decision_runs(&DecisionRunFilter::default())
            .unwrap()
    }
    fn proposal(&self) -> Option<String> {
        let s = self.store.lock().unwrap();
        s.get_session_by_id(self.me)
            .unwrap()
            .unwrap()
            .proposals
            .into_iter()
            .find(|p| p.feature == "related_session")
            .map(|p| p.value)
    }
}

#[test]
fn options_name_sessions() {
    assert_eq!(option_of(7), "s7");
    assert_eq!(session_of("s7"), Some(7));
    assert_eq!(session_of(NONE_OPTION), None);
    assert_eq!(cut(&"x".repeat(1000)).len(), PROMPT_CHARS);
}

#[test]
fn only_the_same_persons_live_sessions_off_this_worktree_are_candidates() {
    let w = world();
    let s = w.store.lock().unwrap();
    let me = s.get_session_by_id(w.me).unwrap().unwrap();
    let ids: Vec<i64> = candidates(&s, &me)
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, vec![w.twin], "never Bob's, never itself");
    let mut gone = s.get_session_by_id(w.twin).unwrap().unwrap();
    gone.status = "ghost".into();
    assert!(!eligible(&me, &gone));
    let mut same_tree = s.get_session_by_id(w.twin).unwrap().unwrap();
    same_tree.project_id = Some(1);
    same_tree.worktree_key = Some("k".into());
    let mut me2 = me.clone();
    me2.project_id = Some(1);
    me2.worktree_key = Some("k".into());
    assert!(
        !eligible(&me2, &same_tree),
        "Related sessions lists it already"
    );
    let _ = w.bobs;
}

#[tokio::test]
async fn off_by_default_nothing_is_asked() {
    let w = world();
    let fake = Fake::answering(vec![]);
    assert_eq!(ask(&w.ctx(&fake), w.me).await, None);
    assert_eq!(fake.calls(), 0);
    assert!(w.runs().is_empty());
}

#[tokio::test]
async fn shadow_records_and_proposes_nothing() {
    let w = world();
    w.on("shadow");
    let fake = Fake::answering(vec![says(&option_of(w.twin), 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), w.me).await, None);
    let runs = w.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].subject_kind, "session");
    assert_eq!(runs[0].baseline_answer.as_deref(), Some(NONE_OPTION));
    let seen = fake.seen.lock().unwrap();
    let sent = serde_json::to_string(&seen[0].redacted()).unwrap();
    assert!(sent.contains("fix the second refund retry"));
    assert!(!sent.contains("fail twice"), "Bob's prompt is never sent");
    drop(seen);
    assert_eq!(w.proposal(), None);
}

#[tokio::test]
async fn assist_leaves_the_proposal_on_the_row_and_asks_once() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![says(&option_of(w.twin), 0.8)]);
    let ctx = w.ctx(&fake);
    assert_eq!(ask(&ctx, w.me).await, Some(w.twin));
    assert_eq!(w.proposal(), Some(option_of(w.twin)));
    assert_eq!(ask(&ctx, w.me).await, None);
    assert_eq!(fake.calls(), 1);
}

#[tokio::test]
async fn a_young_conversation_or_none_alike_is_not_asked() {
    let w = world();
    w.on("assist");
    let fake = Fake::answering(vec![]);
    // The twin's conversation has had one turn.
    assert_eq!(ask(&w.ctx(&fake), w.twin).await, None);
    assert_eq!(fake.calls(), 0);
    let fake = Fake::answering(vec![says(NONE_OPTION, 0.9)]);
    assert_eq!(ask(&w.ctx(&fake), w.me).await, None);
    assert_eq!(w.proposal(), None);
}

/// M15 G4.3: Link keeps the other session on the row as `linked`; Not
/// related withdraws it; a decided or foreign run changes nothing.
#[tokio::test]
async fn link_keeps_the_partner_and_not_related_withdraws_it() {
    let proposed = |w: &World| {
        let s = w.store.lock().unwrap();
        s.get_session_by_id(w.me)
            .unwrap()
            .unwrap()
            .proposals
            .into_iter()
            .find(|p| p.feature == "related_session")
    };
    for linked in [true, false] {
        let w = world();
        w.on("assist");
        let fake = Fake::answering(vec![says(&option_of(w.twin), 0.8)]);
        assert_eq!(ask(&w.ctx(&fake), w.me).await, Some(w.twin));
        let p = proposed(&w).expect("proposal");
        assert_eq!(p.linked, None);
        let run_id = p.run_id.expect("run");
        // Another session's id does not reach this run.
        let wrong = DecideRelatedSessionArgs {
            session_id: w.twin,
            run_id,
            linked,
        };
        assert!(!decide_proposal(&w.store.lock().unwrap(), &wrong, 1).unwrap());
        let args = DecideRelatedSessionArgs {
            session_id: w.me,
            run_id,
            linked,
        };
        let row = decide_related_session(args.clone(), &w.store).unwrap();
        let after = row
            .proposals
            .iter()
            .find(|p| p.feature == "related_session")
            .cloned();
        if linked {
            let p = after.expect("a linked partner stays");
            assert_eq!(p.value, option_of(w.twin));
            assert_eq!(p.linked, Some(true));
        } else {
            assert!(after.is_none(), "Not related withdraws it");
        }
        let run = w.runs().into_iter().find(|r| r.id == run_id).unwrap();
        assert_eq!(
            run.followup.as_deref(),
            Some(if linked { "confirmed" } else { "rejected" })
        );
        // Decided once: a second answer changes nothing.
        assert!(!decide_proposal(&w.store.lock().unwrap(), &args, 2).unwrap());
    }
    let w = world();
    let err = decide_related_session(
        DecideRelatedSessionArgs {
            session_id: 9_999,
            run_id: 1,
            linked: true,
        },
        &w.store,
    )
    .unwrap_err();
    assert_eq!(err.code, "E_NOTFOUND");
}
