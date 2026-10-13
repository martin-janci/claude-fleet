//! Automatic account switching: who it applies to, which login it picks,
//! and what a run records. The switch itself is a fake; nothing reaches a
//! host.

use super::*;
use crate::service::account_limits::seed_usage;
use crate::store::Store;

const NOW: i64 = 1_000_000;

/// `mac` with its own login out of credit, `work` at 40% and `spare` at
/// 10%, the fleet switch on, and one idle Claude session on the own login,
/// owned by the personal owner. Returns the session id and the owner.
fn fleet() -> (Store, i64, i64) {
    let s = Store::open_in_memory().unwrap();
    seed_usage(&s, "mac", None, "acct-out", 100.0, NOW);
    seed_usage(&s, "mac", Some("work"), "acct-work", 40.0, NOW);
    seed_usage(&s, "mac", Some("spare"), "acct-spare", 10.0, NOW);
    reachable(&s, "mac");
    settings::set(&s, settings::ACCOUNTS_AUTO_SWITCH, "true").unwrap();
    let owner = s.mint_personal_owner().unwrap();
    let id = session(&s, "s1", "acct-out", Some(owner));
    (s, id, owner)
}

fn reachable(s: &Store, alias: &str) {
    s.conn_for_test()
        .execute(
            "UPDATE hosts SET reachable = 1, last_pinged_at = 1 WHERE alias = ?1",
            [alias],
        )
        .unwrap();
}

fn session(s: &Store, name: &str, account: &str, owner: Option<i64>) -> i64 {
    let id = s
        .upsert_session(name, "mac", None, None, 1, 1, "running", Some(account))
        .unwrap();
    s.conn_for_test()
        .execute(
            "UPDATE sessions SET owner_person_id = ?2, \
             visibility = CASE WHEN ?2 IS NULL THEN 'unclaimed' ELSE 'private' END, \
             claude_status = 'idle' WHERE id = ?1",
            rusqlite::params![id, owner],
        )
        .unwrap();
    id
}

fn plan_now(s: &Store) -> Vec<Planned> {
    plan(s, true, &HashMap::new(), NOW).unwrap()
}

#[test]
fn a_paused_session_moves_to_the_login_with_the_most_headroom() {
    let (s, id, _) = fleet();
    let p = plan_now(&s);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].session_id, id);
    assert_eq!(p[0].from_account, "acct-out");
    assert_eq!(p[0].target.profile.as_deref(), Some("spare"));
    assert_eq!(p[0].candidates.len(), 2, "work and spare both qualify");
}

#[test]
fn off_for_the_fleet_nothing_moves() {
    let (s, _, _) = fleet();
    settings::set(&s, settings::ACCOUNTS_AUTO_SWITCH, "false").unwrap();
    assert!(plan_now(&s).is_empty());
}

#[test]
fn a_paired_desktop_never_switches() {
    let (s, _, _) = fleet();
    s.set_setting(
        crate::service::decide::HUB_REMOTE_URL_KEY,
        "https://hub.example:7443",
    )
    .unwrap();
    assert!(plan_now(&s).is_empty());
}

#[test]
fn a_person_who_turned_it_off_keeps_their_session_where_it_is() {
    let (s, _, owner) = fleet();
    assert!(s.person_auto_switch(owner).unwrap(), "on by default");
    let ada = s.create_person("ada", None).unwrap().id;
    let hers = session(&s, "s2", "acct-out", Some(ada));
    s.set_person_auto_switch(ada, false).unwrap();
    let ids: Vec<i64> = plan_now(&s).iter().map(|p| p.session_id).collect();
    assert!(!ids.contains(&hers), "ada said no");
    assert_eq!(ids.len(), 1, "the owner's own still moves");
    s.set_person_auto_switch(owner, false).unwrap();
    assert!(plan_now(&s).is_empty());
}

#[test]
fn an_unclaimed_session_follows_the_owner() {
    let (s, owned, owner) = fleet();
    let unclaimed = session(&s, "s2", "acct-out", None);
    let ids: Vec<i64> = plan_now(&s).iter().map(|p| p.session_id).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&owned) && ids.contains(&unclaimed));
    s.set_person_auto_switch(owner, false).unwrap();
    assert!(plan_now(&s).is_empty());
}

#[test]
fn only_a_session_paused_by_its_limit_moves() {
    let (s, id, _) = fleet();
    let set = |sql: &str| {
        s.conn_for_test()
            .execute(&format!("UPDATE sessions SET {sql} WHERE id = ?1"), [id])
            .unwrap();
    };
    set("claude_status = 'working'");
    assert!(plan_now(&s).is_empty(), "a working session is left alone");
    set("claude_status = 'idle', kind = 'shell'");
    assert!(plan_now(&s).is_empty(), "a shell has no login");
    set("kind = 'work', status = 'ghost'");
    assert!(plan_now(&s).is_empty(), "a ghost is not live");
    set("status = 'running', account_uuid = 'acct-work'");
    assert!(
        plan_now(&s).is_empty(),
        "an account with room is not paused"
    );
}

#[test]
fn with_no_login_under_the_line_the_session_waits_for_a_person() {
    let (s, _, _) = fleet();
    seed_usage(&s, "mac", Some("work"), "acct-work", 95.0, NOW);
    seed_usage(&s, "mac", Some("spare"), "acct-spare", 100.0, NOW);
    assert!(plan_now(&s).is_empty());
}

#[test]
fn a_second_login_on_the_same_account_is_no_way_out() {
    let (s, _, _) = fleet();
    seed_usage(&s, "mac", Some("work"), "acct-out", 100.0, NOW);
    seed_usage(&s, "mac", Some("spare"), "acct-out", 100.0, NOW);
    assert!(plan_now(&s).is_empty());
}

#[test]
fn a_session_tried_recently_is_left_alone() {
    let (s, id, _) = fleet();
    let recent = HashMap::from([(id, NOW - 60)]);
    assert!(plan(&s, true, &recent, NOW).unwrap().is_empty());
    let old = HashMap::from([(id, NOW - RETRY_AFTER_SECS)]);
    assert_eq!(plan(&s, true, &old, NOW).unwrap().len(), 1);
}

#[test]
fn an_unreachable_host_is_skipped() {
    let (s, _, _) = fleet();
    s.conn_for_test()
        .execute("UPDATE hosts SET reachable = 0 WHERE alias = 'mac'", [])
        .unwrap();
    assert!(plan_now(&s).is_empty());
}

#[test]
fn the_pick_is_the_least_used_and_the_hosts_own_wins_a_tie() {
    let l = |p: Option<&str>, u: f64| HostLogin {
        profile: p.map(str::to_string),
        account_uuid: p.unwrap_or("own").to_string(),
        used_pct: Some(u),
    };
    let c = [l(None, 20.0), l(Some("a"), 20.0), l(Some("b"), 30.0)];
    assert_eq!(pick(&c).unwrap().profile, None);
    let c = [l(None, 50.0), l(Some("a"), 20.0)];
    assert_eq!(pick(&c).unwrap().profile.as_deref(), Some("a"));
    assert!(pick(&[]).is_none());
}

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<(String, String, String)>>,
    fail: bool,
}

#[async_trait::async_trait]
impl Switcher for Fake {
    async fn switch(&self, host: &str, name: &str, profile: &str) -> Result<(), IpcError> {
        self.calls
            .lock()
            .unwrap()
            .push((host.into(), name.into(), profile.into()));
        if self.fail {
            Err(IpcError::new(
                crate::ipc_error::codes::E_SSH,
                "host went away",
            ))
        } else {
            Ok(())
        }
    }
}

fn events(s: &Store, id: i64) -> Vec<(String, String)> {
    let mut st = s
        .conn_for_test()
        .prepare("SELECT kind, detail FROM session_events WHERE session_id = ?1 ORDER BY id")
        .unwrap();
    st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[tokio::test]
async fn a_run_switches_records_and_does_not_try_again_at_once() {
    let (s, id, _) = fleet();
    let store = Arc::new(Mutex::new(s));
    let fake = Fake::default();
    let recent = Mutex::new(HashMap::new());
    let r = run_with(&store, &fake, None, &recent, true, NOW)
        .await
        .unwrap();
    assert_eq!(
        r,
        RunReport {
            switched: 1,
            failed: 0
        }
    );
    assert_eq!(
        fake.calls.lock().unwrap().as_slice(),
        [("mac".to_string(), "s1".to_string(), "spare".to_string())]
    );
    let ev = events(&store.lock().unwrap(), id);
    let (kind, detail) = ev.last().unwrap();
    assert_eq!(kind, EVENT_SWITCHED);
    let d: serde_json::Value = serde_json::from_str(detail).unwrap();
    assert_eq!(d["from_account"], "acct-out");
    assert_eq!(d["to_account"], "acct-spare");
    assert_eq!(d["profile"], "spare");
    // The fake did not drop the row's account, so the row still reads
    // paused: the retry window keeps the next run from trying again.
    let again = run_with(&store, &fake, None, &recent, true, NOW + 60)
        .await
        .unwrap();
    assert_eq!(again, RunReport::default());
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_failed_switch_is_recorded_with_its_code() {
    let (s, id, _) = fleet();
    let store = Arc::new(Mutex::new(s));
    let fake = Fake {
        fail: true,
        ..Default::default()
    };
    let recent = Mutex::new(HashMap::new());
    let r = run_with(&store, &fake, None, &recent, true, NOW)
        .await
        .unwrap();
    assert_eq!(
        r,
        RunReport {
            switched: 0,
            failed: 1
        }
    );
    let ev = events(&store.lock().unwrap(), id);
    let (kind, detail) = ev.last().unwrap();
    assert_eq!(kind, EVENT_SWITCH_FAILED);
    assert!(detail.contains(crate::ipc_error::codes::E_SSH), "{detail}");
}

#[tokio::test]
async fn pause_all_stops_the_account_switch() {
    let (s, _, _) = fleet();
    settings::set(&s, settings::AUTOMATION_PAUSED, "true").unwrap();
    let store = Arc::new(Mutex::new(s));
    let ssh = Arc::new(SshClient::new());
    assert!(!maybe_run(&store, &ssh), "paused: the pass does not start");
}

#[test]
fn a_person_reads_and_sets_their_own_answer() {
    let (s, _, owner) = fleet();
    let read = AutoSwitchArgs::default();
    assert_eq!(
        my_auto_switch(&s, Some(owner), &read).unwrap(),
        AutoSwitchView {
            enabled: true,
            continue_after: false,
            fleet_enabled: true
        }
    );
    let off = AutoSwitchArgs {
        enabled: Some(false),
        continue_after: Some(true),
    };
    let v = my_auto_switch(&s, Some(owner), &off).unwrap();
    assert!(!v.enabled && v.continue_after);
    assert!(!s.person_auto_switch(owner).unwrap());
    let err = my_auto_switch(&s, None, &read).unwrap_err();
    assert_eq!(err.code, crate::ipc_error::codes::E_FORBIDDEN);
    assert_eq!(
        my_auto_switch(&s, Some(9_999), &read).unwrap_err().code,
        crate::ipc_error::codes::E_NOTFOUND
    );
}

fn turn_end(s: &Store, id: i64, kind: &str, detail: &str) {
    s.insert_session_event(id, kind, Some(detail)).unwrap();
}

fn queued(s: &Store, id: i64) -> Vec<(String, Option<i64>)> {
    let mut st = s
        .conn_for_test()
        .prepare("SELECT body, not_before FROM deferred_prompts WHERE session_id = ?1 ORDER BY id")
        .unwrap();
    st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn only_a_turn_the_limit_cut_off_is_told_to_carry_on() {
    assert!(cut_off_by_limit(
        "stop_failure",
        Some("rate_limit: 429 Too Many Requests")
    ));
    assert!(cut_off_by_limit("stop_failure", Some("rate_limit")));
    assert!(!cut_off_by_limit("stop_failure", Some("auth (401)")));
    assert!(!cut_off_by_limit("turn_done", Some("rate_limit")));
    let (s, id, owner) = fleet();
    turn_end(&s, id, "stop_failure", "rate_limit: 429");
    assert!(!plan_now(&s)[0].continue_after, "off until the person asks");
    s.set_person_auto_switch_continue(owner, true).unwrap();
    assert!(plan_now(&s)[0].continue_after);
    turn_end(&s, id, "turn_done", "");
    assert!(
        !plan_now(&s)[0].continue_after,
        "a finished turn is only resumed"
    );
}

#[tokio::test]
async fn a_switch_queues_continue_for_a_turn_the_limit_cut_off() {
    let (s, id, owner) = fleet();
    s.set_person_auto_switch_continue(owner, true).unwrap();
    turn_end(&s, id, "stop_failure", "rate_limit: 429");
    let store = Arc::new(Mutex::new(s));
    let recent = Mutex::new(HashMap::new());
    run_with(&store, &Fake::default(), None, &recent, true, NOW)
        .await
        .unwrap();
    let s = store.lock().unwrap();
    assert_eq!(
        queued(&s, id),
        [(CONTINUE_PROMPT.to_string(), Some(NOW + CONTINUE_AFTER_SECS))]
    );
    let ev = events(&s, id);
    let d: serde_json::Value = serde_json::from_str(&ev.last().unwrap().1).unwrap();
    assert_eq!(d["continue"], true);
}

#[tokio::test]
async fn a_failed_switch_queues_nothing() {
    let (s, id, owner) = fleet();
    s.set_person_auto_switch_continue(owner, true).unwrap();
    turn_end(&s, id, "stop_failure", "rate_limit: 429");
    let store = Arc::new(Mutex::new(s));
    let fake = Fake {
        fail: true,
        ..Default::default()
    };
    run_with(&store, &fake, None, &Mutex::new(HashMap::new()), true, NOW)
        .await
        .unwrap();
    assert!(queued(&store.lock().unwrap(), id).is_empty());
}
