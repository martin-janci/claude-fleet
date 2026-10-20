//! The prompt tuner: the context says where the prompt goes and keeps the
//! draft, the reply is read as JSON or as text, an empty draft runs
//! nothing, and a session's or a routine's tune runs on its host only while
//! the toggle is on.

use super::*;
use crate::service::context_help::{Completion, HELP_TAG};
use crate::ssh_fake::{FakeSsh, Match, Reply};
use crate::store::Store;
use std::sync::Mutex;

const ENVELOPE: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"{\"prompt\": \"Fix the failing login test in src/auth.rs.\\nDone when cargo test passes.\", \"notes\": \"Named the file and when it is done.\"}","total_cost_usd":0.0004,"usage":{"input_tokens":700,"output_tokens":30}}"#;

fn target() -> TuneTarget {
    TuneTarget {
        kind: "session",
        agent: "claude".into(),
        host_alias: "mercury".into(),
        project: Some("acme/api".into()),
        max_chars: DRAFT_MAX_CHARS,
    }
}

fn req(draft: &str) -> TuneRequest {
    TuneRequest {
        draft: draft.into(),
        ..TuneRequest::default()
    }
}

/// A model that answers one fixed text and keeps what it was asked.
struct Says {
    text: String,
    seen: Mutex<Vec<(String, String)>>,
}

impl Says {
    fn new(text: &str) -> Self {
        Self {
            text: text.into(),
            seen: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl HelpModel for Says {
    fn model(&self) -> String {
        "fake".into()
    }
    async fn complete(&self, instruction: &str, context: &str) -> Result<Completion, IpcError> {
        self.seen
            .lock()
            .unwrap()
            .push((instruction.into(), context.into()));
        Ok(Completion {
            text: self.text.clone(),
            usage: None,
        })
    }
}

#[test]
fn the_context_says_where_it_goes_then_the_history_the_draft_and_the_ask() {
    let mut r = req("fix login test");
    r.ask = "add when it is done".into();
    r.history = vec!["first".into(), " ".into(), "second".into()];
    r.commands = vec!["/plan #KEY or a goal — Plan subtasks".into()];
    let text = context_text(&target(), &r);
    let at = |s: &str| text.find(s).unwrap_or_else(|| panic!("{s:?} in {text}"));
    assert!(
        text.starts_with("## Where it goes\n- the message box"),
        "{text}"
    );
    assert!(at("- agent: claude") < at("- project: acme/api"));
    assert!(at("- first") < at("- second"));
    assert!(
        !text.contains("-  \n"),
        "a blank entry is not history: {text}"
    );
    assert!(at("## Commands\n/plan") < at("## Draft\nfix login test"));
    assert!(
        text.ends_with("## What I want from the rewrite\nadd when it is done\n"),
        "{text}"
    );
    let routine = TuneTarget {
        kind: "routine",
        project: None,
        max_chars: 4000,
        ..target()
    };
    let text = context_text(&routine, &req("x"));
    assert!(text.contains("no person there to answer"), "{text}");
    assert!(text.contains("at most 4000 characters"), "{text}");
    assert!(!text.contains("project:"), "{text}");
    assert!(context_text(&target(), &req("x"))
        .ends_with("Make it clearer and more complete for the agent.\n"));
}

#[test]
fn the_draft_survives_a_long_history() {
    let mut r = req("THE DRAFT");
    r.history = (0..200)
        .map(|i| format!("{i} {}", "x".repeat(1_900)))
        .collect();
    let text = context_text(&target(), &r);
    assert!(text.len() <= context_help::CONTEXT_MAX_BYTES);
    assert!(text.contains("## Draft\nTHE DRAFT"), "the draft is kept");
}

#[test]
fn the_reply_is_read_as_json_or_as_plain_text() {
    let parse = |reply: &str| parse_reply(reply, "", DRAFT_MAX_CHARS);
    let (p, n) = parse(r#"Sure: {"prompt": "Do X.\nThen Y.", "notes": "Split it."}"#).unwrap();
    assert_eq!((p.as_str(), n.as_str()), ("Do X.\nThen Y.", "Split it."));
    let (p, n) = parse("Just do X.").unwrap();
    assert_eq!((p.as_str(), n.as_str()), ("Just do X.", ""));
    let (p, _) = parse(r#"{"prompt": "```\nDo X.\n```"}"#).unwrap();
    assert_eq!(p, "Do X.", "a fence is stripped");
    assert_eq!(parse(r#"{"prompt": "", "notes": "n"}"#), None);
    assert_eq!(
        parse("{\"prompt\": \"a\\u0007b\"}"),
        None,
        "a bell is refused"
    );
    let long = format!(r#"{{"prompt": "{}"}}"#, "y".repeat(PROMPT_MAX_CHARS + 50));
    assert_eq!(
        parse_reply(&long, "", PROMPT_MAX_CHARS + 100)
            .unwrap()
            .0
            .chars()
            .count(),
        PROMPT_MAX_CHARS
    );
}

/// A multibyte draft keeps where it goes and its own start: only history
/// is cut, and a draft that cannot fit whole is refused, never cut.
#[tokio::test]
async fn a_multibyte_draft_keeps_the_header_and_a_too_long_one_is_refused() {
    let mut r = req(&format!("START {}", "語".repeat(11_000)));
    r.ask = "短く".repeat(400);
    r.history = (0..50)
        .map(|i| format!("{i} {}", "x".repeat(1_900)))
        .collect();
    assert!(!draft_too_long(&target(), &r));
    let text = context_text(&target(), &r);
    assert!(text.len() <= context_help::CONTEXT_MAX_BYTES);
    assert!(text.starts_with("## Where it goes\n"), "the header is kept");
    assert!(
        text.contains("## Draft\nSTART "),
        "the draft's start is kept"
    );

    let m = Says::new(r#"{"prompt": "x"}"#);
    for draft in [
        "🙂".repeat(DRAFT_MAX_CHARS),
        "a".repeat(DRAFT_MAX_CHARS + 1),
    ] {
        let e = tune(&m, &target(), &req(&draft)).await.unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "{e:?}");
        assert!(e.message.contains("too long"), "{e:?}");
    }
    assert!(m.seen.lock().unwrap().is_empty(), "nothing ran");
    assert!(
        tune(&m, &target(), &req(&"a".repeat(DRAFT_MAX_CHARS)))
            .await
            .is_ok(),
        "a draft at the limit runs"
    );
}

/// The draft's own checksum is not masked in the rewrite, so an unchanged
/// draft reads as unchanged; a token the model added still is.
#[tokio::test]
async fn the_drafts_own_hashes_survive_redaction_and_new_ones_do_not() {
    let sha = "ab".repeat(32);
    let draft = format!("verify the release checksum is {sha}");
    let m = Says::new(&format!(r#"{{"prompt": "{draft}", "notes": "Clear."}}"#));
    let (a, _) = tune(&m, &target(), &req(&draft)).await.unwrap();
    assert!(a.unchanged, "{a:?}");
    assert_eq!(a.prompt, draft);

    let other = "cd".repeat(32);
    let reply =
        format!(r#"{{"prompt": "Verify that the release checksum equals {sha} (not {other})."}}"#);
    let (p, _) = parse_reply(&reply, &draft, DRAFT_MAX_CHARS).unwrap();
    assert!(p.contains(&sha), "the draft's own value is kept: {p}");
    assert!(
        !p.contains(&other),
        "a value the model added is masked: {p}"
    );
    assert!(p.contains("[REDACTED]"), "{p}");
}

/// A rewrite longer than the target takes (a routine's 4000) is cut to it
/// and the notes say so, so accepting it never yields an unsaveable prompt.
#[test]
fn the_rewrite_is_capped_at_the_targets_limit() {
    let reply = format!(
        r#"{{"prompt": "{}", "notes": "Longer."}}"#,
        "y".repeat(5_000)
    );
    let (p, n) = parse_reply(&reply, "short", 4_000).unwrap();
    assert_eq!(p.chars().count(), 4_000);
    assert!(n.starts_with("Longer.\n"), "{n}");
    assert!(n.contains("4000-character limit"), "{n}");
    let (p, n) = parse_reply(r#"{"prompt": "ok"}"#, "short", 4_000).unwrap();
    assert_eq!((p.as_str(), n.as_str()), ("ok", ""));
}

#[tokio::test]
async fn tune_sends_the_fixed_instruction_and_says_when_nothing_changed() {
    let m = Says::new(r#"{"prompt": "fix it", "notes": "Already clear."}"#);
    let (a, _) = tune(&m, &target(), &req("  fix it ")).await.unwrap();
    assert!(a.unchanged);
    assert_eq!(a.model, "fake");
    let seen = m.seen.lock().unwrap();
    assert_eq!(seen[0].0, TUNE_INSTRUCTION);
    assert!(seen[0].1.contains("## Draft\nfix it"));
}

#[tokio::test]
async fn an_empty_draft_runs_nothing_and_an_empty_reply_is_an_error() {
    let m = Says::new("{}");
    let e = tune(&m, &target(), &req("   ")).await.unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    assert!(m.seen.lock().unwrap().is_empty());
    let e = tune(&m, &target(), &req("x")).await.unwrap_err();
    assert_eq!(e.code, codes::E_CLAUDE_CLI);
}

/// The composer's tune runs under the session's launch profile, only while
/// Prompt tuner is on, and is booked to the session's org as context help.
#[tokio::test]
async fn a_sessions_tune_runs_under_its_launch_and_is_booked_to_its_org() {
    let s = Store::open_in_memory().unwrap();
    s.insert_host("mercury", Some("mercury")).unwrap();
    let org = s.add_org("Acme", None, false).unwrap().id;
    s.set_host_org("mercury", Some(org)).unwrap();
    let id = s
        .upsert_session("dev-1", "mercury", None, None, 1, 1, "running", None)
        .unwrap();
    s.set_session_profile(id, Some("work")).unwrap();
    let row = s.get_session_by_id(id).unwrap().unwrap();
    let store = Mutex::new(s);
    let fake = FakeSsh::new();
    fake.on_host(
        "mercury",
        Match::script_contains("claude -p"),
        Reply::ok(&format!("{HELP_TAG}run\n{ENVELOPE}\n")),
    );
    let r = req("fix login test");
    let go = || tune_for_session(&store, &fake, &row, &r);

    let off = go().await.unwrap_err();
    assert_eq!(off.code, codes::E_INVALID_STATE, "{off:?}");
    assert!(off.message.contains("Prompt tuner is off"), "{off:?}");
    assert!(fake.calls().is_empty(), "nothing runs while it is off");

    settings::set(
        &crate::ipc_error::lock(&store).unwrap(),
        settings::WORK_PROMPT_TUNE,
        "true",
    )
    .unwrap();
    let a = go().await.unwrap();
    assert_eq!(
        a.prompt,
        "Fix the failing login test in src/auth.rs.\nDone when cargo test passes."
    );
    assert_eq!(a.notes, "Named the file and when it is done.");
    assert!(!a.unchanged);
    assert_eq!(
        (a.model.as_str(), a.host_alias.as_str()),
        ("haiku", "mercury")
    );
    let script = fake.calls()[0].script().unwrap();
    assert!(script.contains("claude-profiles/\"'work'"), "{script}");
    assert!(
        !script.contains("fix login test"),
        "the draft rides as base64: {script}"
    );
    let rows = crate::ipc_error::lock(&store)
        .unwrap()
        .aux_usage_of_origin(crate::store::AUX_ORIGIN_CONTEXT_HELP)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].org_id, Some(org));
}

/// A routine's draft is tuned on its own host and profile, unsaved, and a
/// host the caller cannot see is not one it may spend.
#[tokio::test]
async fn a_routines_draft_is_tuned_on_its_host_and_an_unknown_host_refuses() {
    let s = Store::open_in_memory().unwrap();
    s.insert_host("mercury", Some("mercury")).unwrap();
    settings::set(&s, settings::WORK_PROMPT_TUNE, "true").unwrap();
    let store = Mutex::new(s);
    let fake = FakeSsh::new();
    fake.on_host(
        "mercury",
        Match::script_contains("claude -p"),
        Reply::ok(&format!("{HELP_TAG}run\n{ENVELOPE}\n")),
    );
    let scope = crate::service::view_scope::ViewScope::internal();
    let mut input = crate::service::routines::RoutineInput {
        host_alias: "mercury".into(),
        profile: Some("work".into()),
        prompt: "check ci".into(),
        ..Default::default()
    };
    let a = tune_for_routine(&store, &fake, &scope, &input, "shorter")
        .await
        .unwrap();
    assert_eq!(a.host_alias, "mercury");
    assert!(fake.calls()[0]
        .script()
        .unwrap()
        .contains("claude-profiles/\"'work'"));

    input.host_alias = "venus".into();
    let e = tune_for_routine(&store, &fake, &scope, &input, "")
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND, "{e:?}");
    assert_eq!(fake.calls().len(), 1, "nothing ran for the unknown host");
}
