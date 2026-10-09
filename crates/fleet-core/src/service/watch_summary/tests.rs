//! Orbit Fleet 11.11: a watcher's "Since 13:20" summary — only with the
//! org's consent, on the session's own host and account, booked, and hidden
//! when J9 finds it unsupported.

use super::*;
use crate::service::decide::{BackendError, DecisionBackend, JevRequest, JevResponse};
use crate::ssh_fake::{FakeSsh, Match, Reply};
use crate::store::Secret;

const ENVELOPE: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"Fixed the flaky hub test; the PR waits on review.","total_cost_usd":0.001,"usage":{"input_tokens":900,"output_tokens":20}}"#;

/// 2026-10-08T13:20:00Z.
const T1320: i64 = 1_791_465_600;

fn turn(at: &str, prompt: &str, reply: &str) -> ConvTurn {
    serde_json::from_value(serde_json::json!({
        "prompt": prompt,
        "at": at,
        "ended_at": at,
        "items": [
            { "kind": "text", "text": reply },
            { "kind": "tool", "summary": "Bash: cargo test", "error": true },
        ],
    }))
    .unwrap()
}

/// A Jev that answers one noul.
struct Says(f64);

#[async_trait::async_trait]
impl DecisionBackend for Says {
    fn provider(&self) -> &'static str {
        crate::service::decide::PROVIDER_JEV
    }
    async fn ask(
        &self,
        _key: &Secret,
        _model: &str,
        _req: &JevRequest,
        _timeout: Duration,
    ) -> Result<JevResponse, BackendError> {
        Ok(serde_json::from_value(serde_json::json!({
            "model": "jev-1.13.0",
            "answers": { "q": { "type": "noul", "noul": self.0 } },
            "usage": { "input_tokens": 500, "output_tokens": 1 },
        }))
        .unwrap())
    }
}

fn world(profile: Option<&str>, consent: bool) -> (Arc<Mutex<Store>>, SessionRow) {
    let s = Store::open_in_memory().unwrap();
    s.insert_host("mercury", Some("mercury")).unwrap();
    let id = s
        .upsert_session("dev-1", "mercury", None, None, 1, 1, "running", None)
        .unwrap();
    s.set_session_profile(id, profile).unwrap();
    settings::set(
        &s,
        settings::DECIDE_JEV_UNASSIGNED,
        if consent { "true" } else { "false" },
    )
    .unwrap();
    let row = s.get_session_by_id(id).unwrap().unwrap();
    (Arc::new(Mutex::new(s)), row)
}

fn booked(store: &Mutex<Store>) -> Vec<crate::store::AuxUsageRow> {
    lock(store)
        .unwrap()
        .aux_usage_of_origin(crate::store::AUX_ORIGIN_WATCH_SUMMARY)
        .unwrap()
}

#[test]
fn the_excerpt_is_the_turns_since_the_time() {
    let turns = vec![
        turn("2026-10-08T13:00:00Z", "old ask", "old reply"),
        turn("2026-10-08T13:25:00Z", "fix the flaky test", "Fixed it."),
    ];
    let (text, n) = excerpt_since(&turns, T1320);
    assert_eq!(n, 1);
    assert!(!text.contains("old ask"), "{text}");
    assert!(
        text.contains("[2026-10-08T13:25:00Z] Person: fix the flaky test"),
        "{text}"
    );
    assert!(text.contains("Agent: Fixed it."), "{text}");
    assert!(text.contains("  tool: Bash: cargo test (failed)"), "{text}");
    assert_eq!(excerpt_since(&turns, T1320 + 3_600).1, 0);
}

/// The plan's 11.11 check: no summary without consent.
#[test]
fn no_summary_without_the_orgs_consent() {
    let (store, row) = world(None, false);
    let e = plan(&lock(&store).unwrap(), &row).unwrap_err();
    assert_eq!(e.code, codes::E_FORBIDDEN);
    let s = lock(&store).unwrap();
    let org = s.add_org("Acme", None, false).unwrap().id;
    s.set_host_org("mercury", Some(org)).unwrap();
    let row = s.get_session_by_id(row.id).unwrap().unwrap();
    assert_eq!(plan(&s, &row).unwrap_err().code, codes::E_FORBIDDEN);
    s.set_org_jev_allowed(org, true).unwrap();
    assert!(plan(&s, &row).is_ok());
}

#[tokio::test]
async fn it_runs_on_the_sessions_host_under_its_account_and_is_booked() {
    let (store, row) = world(Some("work"), true);
    let p = plan(&lock(&store).unwrap(), &row).unwrap();
    let fake = FakeSsh::new();
    fake.on_host(
        "mercury",
        Match::script_contains("claude -p"),
        Reply::ok(&format!("{SINCE_TAG}run\n{ENVELOPE}\n")),
    );
    let ctx = DecideCtx::jev(Arc::clone(&store));
    let excerpt = "[t] Person: fix it\nAgent: Fixed it.";
    let w = summarize_excerpt(&store, &fake, &ctx, &p, T1320, excerpt, 1)
        .await
        .unwrap();
    assert_eq!(
        w.text.as_deref(),
        Some("Fixed the flaky hub test; the PR waits on review.")
    );
    assert_eq!(
        (w.check, w.host_alias.as_str(), w.turns),
        (Check::Off, "mercury", 1)
    );
    let calls = fake.calls();
    assert_eq!(calls.len(), 1);
    let script = calls[0].script().unwrap();
    assert!(
        script.contains("export CLAUDE_CONFIG_DIR=\"$HOME/.claude-profiles/\"'work'; "),
        "{script}"
    );
    assert!(script.contains("--tools ''"), "isolated: {script}");
    let b64 = base64::engine::general_purpose::STANDARD.encode(excerpt);
    assert!(
        script.contains(&b64),
        "the excerpt rides the script: {script}"
    );
    assert!(
        !script.contains("Fixed it."),
        "never as plain text: {script}"
    );
    let rows = booked(&store);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].host_alias.as_str(), rows[0].cost_micros),
        ("mercury", 1_000)
    );
}

/// The plan's 11.11 check: a failed J9 check hides the summary (its run is
/// still booked).
#[tokio::test]
async fn a_failed_check_hides_the_summary() {
    let (store, row) = world(None, true);
    {
        let s = lock(&store).unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_SUMMARY_CHECK, "assist").unwrap();
        // The check reads the agent's replies: reply text (D48, review r15).
        settings::set(&s, settings::DECIDE_JEV_UNASSIGNED_REPLY, "true").unwrap();
        s.set_decision_credential(
            Some(&Secret::new("tsk_test_0123456789abcdefghijklmnopqrstuv")),
            None,
        )
        .unwrap();
    }
    let p = plan(&lock(&store).unwrap(), &row).unwrap();
    let fake = FakeSsh::new();
    fake.on_host(
        "mercury",
        Match::Any,
        Reply::ok(&format!("{SINCE_TAG}run\n{ENVELOPE}\n")),
    );
    let low = DecideCtx::new(Arc::clone(&store), Arc::new(Says(0.1)));
    let w = summarize_excerpt(&store, &fake, &low, &p, T1320, "Agent: hi", 1)
        .await
        .unwrap();
    assert_eq!((w.check, w.text), (Check::Failed, None));
    let high = DecideCtx::new(Arc::clone(&store), Arc::new(Says(0.9)));
    let w = summarize_excerpt(&store, &fake, &high, &p, T1320, "Agent: hi", 1)
        .await
        .unwrap();
    assert_eq!(w.check, Check::Passed);
    assert!(w.text.is_some());
    assert_eq!(booked(&store).len(), 2);
}

#[tokio::test]
async fn nothing_since_runs_nothing() {
    let (store, row) = world(None, true);
    let p = plan(&lock(&store).unwrap(), &row).unwrap();
    let fake = FakeSsh::new();
    let ctx = DecideCtx::jev(Arc::clone(&store));
    let w = summarize_excerpt(&store, &fake, &ctx, &p, T1320, "", 0)
        .await
        .unwrap();
    assert_eq!((w.text, w.turns), (None, 0));
    assert!(fake.calls().is_empty());
    assert!(booked(&store).is_empty());
}

#[test]
fn the_script_refuses_a_bad_model_or_profile_and_caps_the_excerpt() {
    assert!(since_script("gpt", None, "x").is_err());
    assert!(since_script("haiku", Some("../x"), "x").is_err());
    let long = "y".repeat(EXCERPT_MAX_BYTES * 2);
    let s = since_script("haiku", None, &long).unwrap();
    assert!(s.len() < EXCERPT_MAX_BYTES * 2, "capped: {}", s.len());
    assert!(s.contains("--model 'haiku'"));
}
