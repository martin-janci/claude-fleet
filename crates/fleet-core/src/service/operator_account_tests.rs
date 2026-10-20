//! Control's own account: its state, where a switch takes it, the login a
//! birth takes, and what a pass does (the switch itself is a fake).

use super::*;
use crate::service::account_limits::{ready_host, seed_usage};
use crate::service::placement::Candidate;

const NOW: i64 = 1_000_000;

fn cand(host: &str, profile: Option<&str>) -> Candidate {
    Candidate {
        host_alias: host.into(),
        profile: profile.map(str::to_string),
        account_uuid: format!("{host}-{}", profile.unwrap_or("own")),
        email: None,
        used_pct: Some(10.0),
        cpu_pct: None,
        mem_avail_pct: None,
        score: 50.0,
        reasons: vec![],
    }
}

fn placed(c: Vec<Candidate>) -> Placement {
    Placement {
        pause_at_pct: 90.0,
        candidates: c,
        ..Default::default()
    }
}

#[test]
fn a_spent_or_signed_out_login_reads_as_such() {
    assert_eq!(
        state_of(Some(100.0), false, false, 90.0),
        AccountState::Limit
    );
    assert_eq!(state_of(Some(10.0), false, true, 90.0), AccountState::Limit);
    assert_eq!(
        state_of(Some(100.0), true, false, 90.0),
        AccountState::SignedOut
    );
    assert_eq!(
        state_of(Some(92.0), false, false, 90.0),
        AccountState::NearLimit
    );
    assert_eq!(state_of(Some(10.0), false, false, 90.0), AccountState::Ok);
    assert_eq!(state_of(None, false, false, 90.0), AccountState::Unknown);
    assert_eq!(label_of(AccountState::Limit), Some("Paused · limit"));
    assert_eq!(label_of(AccountState::SignedOut), Some("signed out"));
}

#[test]
fn a_switch_tries_its_own_host_first_then_another() {
    let p = placed(vec![cand("b", None), cand("a", Some("w"))]);
    assert_eq!(
        plan_switch("a", None, AccountState::Limit, false, &p, |_| true),
        Some(Switch::SameHost {
            profile: Some("w".into())
        })
    );
    let p = placed(vec![cand("b", Some("x"))]);
    assert_eq!(
        plan_switch("a", None, AccountState::SignedOut, false, &p, |_| true),
        Some(Switch::OtherHost {
            host_alias: "b".into(),
            profile: Some("x".into())
        })
    );
    assert_eq!(
        plan_switch("a", None, AccountState::Limit, false, &p, |_| false),
        None,
        "no host the operator may live on"
    );
}

#[test]
fn nothing_moves_mid_turn_or_with_headroom_or_onto_the_same_login() {
    let p = placed(vec![cand("a", Some("w"))]);
    assert_eq!(
        plan_switch("a", None, AccountState::Limit, true, &p, |_| true),
        None
    );
    for st in [
        AccountState::Ok,
        AccountState::NearLimit,
        AccountState::Unknown,
    ] {
        assert_eq!(plan_switch("a", None, st, false, &p, |_| true), None);
    }
    assert_eq!(
        plan_switch("a", Some("w"), AccountState::Limit, false, &p, |_| true),
        None,
        "its own login is no way out"
    );
}

#[test]
fn a_birth_keeps_its_login_on_that_host_else_takes_the_best() {
    let p = placed(vec![
        cand("b", None),
        cand("a", Some("x")),
        cand("a", Some("w")),
    ]);
    assert_eq!(
        pick_birth_profile("a", Some(Some("w")), &p),
        Some("w".into())
    );
    assert_eq!(
        pick_birth_profile("a", Some(Some("gone")), &p),
        Some("x".into())
    );
    assert_eq!(pick_birth_profile("a", None, &p), Some("x".into()));
    assert_eq!(pick_birth_profile("c", None, &p), None, "the host's own");
}

/// `mac` with Control on its own login at the limit and `work` at 10%.
fn fleet() -> (Store, i64) {
    let s = Store::open_in_memory().unwrap();
    seed_usage(&s, "mac", None, "acct-out", 100.0, NOW);
    seed_usage(&s, "mac", Some("work"), "acct-work", 10.0, NOW);
    s.conn_for_test()
        .execute(
            "UPDATE hosts SET reachable = 1, last_pinged_at = 1, claude_version = '2.1' WHERE alias = 'mac'",
            [],
        )
        .unwrap();
    let id = s
        .upsert_session(
            crate::service::operator::OPERATOR_TMUX_NAME,
            "mac",
            None,
            None,
            1,
            1,
            "running",
            Some("acct-out"),
        )
        .unwrap();
    s.conn_for_test()
        .execute(
            "UPDATE sessions SET claude_status = 'idle' WHERE id = ?1",
            [id],
        )
        .unwrap();
    crate::service::operator::set_operator_ref(
        &s,
        &crate::service::operator::OperatorRef {
            host_alias: "mac".into(),
            tmux_name: crate::service::operator::OPERATOR_TMUX_NAME.into(),
        },
    )
    .unwrap();
    (s, id)
}

#[test]
fn the_status_names_controls_login_and_its_limit() {
    let (s, id) = fleet();
    let row = s.get_session_by_id(id).unwrap().unwrap();
    let a = account_of(&s, &row, NOW).unwrap();
    assert_eq!(a.profile, None);
    assert_eq!(a.account_uuid.as_deref(), Some("acct-out"));
    assert_eq!(a.state, AccountState::Limit);
    assert_eq!(a.label.as_deref(), Some("Paused · limit"));
    s.set_session_profile(id, Some("work")).unwrap();
    let a = account_of(&s, &row, NOW).unwrap();
    assert_eq!(
        (a.profile.as_deref(), a.state),
        (Some("work"), AccountState::Ok)
    );
}

#[derive(Default)]
struct FakeExec {
    calls: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl SwitchExec for FakeExec {
    async fn restart(&self, host: &str, tmux: &str, profile: &str) -> Result<i64, IpcError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("restart {host} {tmux} {profile}"));
        Ok(1)
    }
    async fn move_to(
        &self,
        host: &str,
        profile: Option<&str>,
        from: &str,
    ) -> Result<SessionRow, IpcError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("move {from} -> {host} {}", profile.unwrap_or("")));
        Err(IpcError::new(crate::ipc_error::codes::E_SSH, "fake"))
    }
    async fn queue_note(&self, id: i64, body: String) -> Result<(), IpcError> {
        self.calls.lock().unwrap().push(format!("note {id} {body}"));
        Ok(())
    }
}

#[tokio::test]
async fn a_pass_resumes_control_under_the_login_with_headroom_and_tells_it() {
    let (s, id) = fleet();
    let store = Mutex::new(s);
    let exec = FakeExec::default();
    let last = Mutex::new(None);
    assert!(run_with(&store, &exec, &last, NOW).await.unwrap());
    let calls = exec.calls.lock().unwrap().clone();
    assert_eq!(calls[0], "restart mac fleet-operator work");
    assert!(
        calls[1].starts_with("note 1 [fleet] your account"),
        "{}",
        calls[1]
    );
    let ev = store
        .lock()
        .unwrap()
        .newest_session_event_of(id, &[crate::service::account_switch::EVENT_SWITCHED])
        .unwrap()
        .expect("the switch is on the timeline");
    assert!(ev.detail.unwrap().contains("\"profile\":\"work\""));
    assert!(
        !run_with(&store, &exec, &last, NOW + 60).await.unwrap(),
        "not tried again at once"
    );
}

#[tokio::test]
async fn with_no_login_left_on_its_host_control_moves_and_a_failure_is_recorded() {
    let (s, id) = fleet();
    seed_usage(&s, "mac", Some("work"), "acct-work", 100.0, NOW);
    seed_usage(&s, "htz", None, "acct-htz", 5.0, NOW);
    ready_host(&s, "htz");
    let store = Mutex::new(s);
    let exec = FakeExec::default();
    assert!(!run_with(&store, &exec, &Mutex::new(None), NOW)
        .await
        .unwrap());
    assert_eq!(exec.calls.lock().unwrap()[0], "move mac -> htz ");
    let ev = store
        .lock()
        .unwrap()
        .newest_session_event_of(id, &[crate::service::account_switch::EVENT_SWITCH_FAILED])
        .unwrap();
    assert!(ev.is_some());
}

#[tokio::test]
async fn off_or_paused_nothing_moves() {
    let (s, _) = fleet();
    settings::set(&s, settings::CONTROL_SWITCH_AT_LIMIT, "false").unwrap();
    let store = Mutex::new(s);
    let exec = FakeExec::default();
    assert!(!run_with(&store, &exec, &Mutex::new(None), NOW)
        .await
        .unwrap());
    assert!(exec.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn pause_all_stops_the_operator_account_switch() {
    let (s, _) = fleet();
    settings::set(&s, settings::AUTOMATION_PAUSED, "true").unwrap();
    let store = Arc::new(Mutex::new(s));
    let ssh = Arc::new(SshClient::new());
    assert!(!maybe_run(&store, &ssh), "paused: the pass does not start");
}

#[test]
fn the_note_says_what_happened() {
    let from = OperatorAccount {
        host_alias: "mac".into(),
        profile: None,
        account_uuid: Some("a".into()),
        email: Some("me@x".into()),
        used_pct: Some(100.0),
        state: AccountState::Limit,
        label: None,
    };
    let same = switched_note(&from, "mac", Some("work"), false);
    assert!(same.starts_with("[fleet] your account me@x reached its limit"));
    assert!(same.contains("same conversation"));
    let moved = switched_note(&from, "htz", None, true);
    assert!(moved.contains("started again on htz under the host's own login"));
    assert!(crate::service::prompt_origin::is_fleet_prompt(&moved));
}
