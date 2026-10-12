//! Context help: the context is capped with the question kept, the reply is
//! read as JSON or as text, a shell's proposal is one line or none, and
//! fleet's model is one isolated `claude -p` on the session's host.

use super::*;
use crate::ssh_fake::{FakeSsh, Match, Reply};
use crate::store::Store;
use std::sync::Mutex;

const ENVELOPE: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"{\"answer\": \"The branch has no upstream.\", \"command\": \"git push -u origin HEAD\"}","total_cost_usd":0.0004,"usage":{"input_tokens":700,"output_tokens":30}}"#;

fn req(question: &str, line: &str) -> HelpRequest {
    HelpRequest {
        question: question.into(),
        line: line.into(),
        ..HelpRequest::default()
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
fn the_context_holds_the_history_the_commands_the_line_and_the_question() {
    let mut r = req("why does this fail?", "git push");
    r.history = vec!["first".into(), "  ".into(), "second\nline".into()];
    r.commands = vec!["/plan #KEY or a goal — Plan subtasks".into()];
    let (text, n) = context_text(&r, None);
    assert_eq!(n, 2, "a blank entry is not history");
    let at = |s: &str| text.find(s).unwrap_or_else(|| panic!("{s:?} in {text}"));
    assert!(at("- first") < at("- second\n  line"));
    assert!(at("## Commands\n/plan") < at("## Typing now\ngit push"));
    assert!(
        text.ends_with("## Question\nwhy does this fail?\n"),
        "{text}"
    );
}

#[test]
fn a_shells_scrollback_stands_in_for_the_history() {
    let mut r = req("", "cargo tset");
    r.history = vec!["ignored".into()];
    let (text, n) = context_text(
        &r,
        Some("$ ls\nsrc\n$ cargo tset\nerror: no such command\n\n"),
    );
    assert_eq!(n, 4);
    assert!(!text.contains("ignored"), "{text}");
    assert!(text.starts_with("## Terminal history\n$ ls\n"), "{text}");
    assert!(
        text.ends_with("## Question\nHelp me with the line I am typing.\n"),
        "an empty question asks about the line: {text}"
    );
}

#[test]
fn the_newest_context_is_kept_and_the_question_always_survives() {
    let mut r = req(&"q".repeat(QUESTION_MAX_CHARS * 2), "line");
    r.history = (0..HISTORY_MAX_ITEMS * 3)
        .map(|i| format!("{i}:{}", "h".repeat(HISTORY_ITEM_MAX_CHARS * 2)))
        .collect();
    let (text, n) = context_text(&r, None);
    assert_eq!(n as usize, HISTORY_MAX_ITEMS, "the newest entries only");
    assert!(text.len() <= CONTEXT_MAX_BYTES);
    assert!(text.contains("## Question\nqqq"));
    assert!(text.trim_end().ends_with('…'), "the question is capped too");
    assert!(text.contains(&format!("{}:", HISTORY_MAX_ITEMS * 3 - 1)));
}

#[test]
fn the_reply_is_read_as_json_or_as_plain_text() {
    let (a, c) = parse_answer(
        Surface::Shell,
        "Sure:\n```json\n{\"answer\": \"Typo.\", \"command\": \"cargo test\"}\n```",
    );
    assert_eq!((a.as_str(), c.as_deref()), ("Typo.", Some("cargo test")));
    let (a, c) = parse_answer(Surface::Shell, "Use cargo test instead.");
    assert_eq!((a.as_str(), c), ("Use cargo test instead.", None));
    let (_, c) = parse_answer(Surface::Shell, r#"{"answer": "x", "command": ""}"#);
    assert_eq!(c, None, "an empty command is none");
    let long = format!(r#"{{"answer": "{}"}}"#, "a".repeat(ANSWER_MAX_CHARS * 2));
    assert_eq!(
        parse_answer(Surface::Composer, &long).0.chars().count(),
        ANSWER_MAX_CHARS
    );
}

/// A newline pasted into a terminal runs what came before it: a shell's
/// proposal is one line or nothing. The composer keeps its lines (its
/// Enter is the person's).
#[test]
fn a_shell_proposal_is_one_line_or_none() {
    assert_eq!(clean_command(Surface::Shell, "ls\nrm -rf ~"), None);
    assert_eq!(clean_command(Surface::Shell, "ls\rrm -rf ~"), None);
    assert_eq!(clean_command(Surface::Shell, "echo \u{1b}[31m"), None);
    assert_eq!(
        clean_command(Surface::Shell, "```bash\n$ git status\n```").as_deref(),
        Some("git status")
    );
    assert_eq!(
        clean_command(Surface::Shell, "`make check`").as_deref(),
        Some("make check")
    );
    assert_eq!(
        clean_command(Surface::Composer, "/plan #FLEET-12\nthen ship it").as_deref(),
        Some("/plan #FLEET-12\nthen ship it")
    );
    assert_eq!(
        clean_command(Surface::Shell, &"x".repeat(COMMAND_MAX_CHARS + 1)),
        None
    );
}

#[tokio::test]
async fn ask_sends_the_surfaces_instruction_and_reads_the_answer() {
    let model = Says::new(r#"{"answer": "Use /plan.", "command": "/plan #FLEET-3"}"#);
    let mut r = req("how do I split this?", "");
    r.history = vec!["/task Split the parser".into()];
    let (a, usage) = ask(&model, Surface::Composer, &r, None).await.unwrap();
    assert_eq!(a.answer, "Use /plan.");
    assert_eq!(a.command.as_deref(), Some("/plan #FLEET-3"));
    assert_eq!((a.model.as_str(), a.host_alias.as_str()), ("fake", ""));
    assert_eq!(a.history_items, 1);
    assert!(usage.is_none());
    let seen = model.seen.lock().unwrap();
    assert_eq!(seen[0].0, COMPOSER_INSTRUCTION);
    assert!(seen[0].1.contains("/task Split the parser"));
}

#[tokio::test]
async fn nothing_to_ask_about_runs_nothing() {
    let model = Says::new("x");
    let e = ask(&model, Surface::Shell, &req(" ", "\n"), None)
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    assert!(model.seen.lock().unwrap().is_empty());
    let e = ask(&Says::new("   "), Surface::Shell, &req("hm", ""), None)
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_CLAUDE_CLI, "an empty reply is a failure");
}

#[test]
fn the_shell_history_reads_the_terminals_own_tmux_session() {
    assert_eq!(
        shell_history_script("dev-1", 2),
        "tmux capture-pane -t '=dev-1--sh2:' -S '-300' -p 2>/dev/null; true"
    );
}

#[tokio::test]
async fn the_shell_history_is_read_over_ssh_and_refuses_a_bad_target() {
    let fake = FakeSsh::new();
    fake.on_host(
        "mercury",
        Match::script_contains("capture-pane"),
        Reply::ok("$ make\nmake: *** No rule\n\n\n"),
    );
    let got = shell_history(&fake, "mercury", "dev-1", 1).await.unwrap();
    assert_eq!(got.as_deref(), Some("$ make\nmake: *** No rule"));
    assert!(shell_history(&fake, "mercury", "dev-1", 0).await.is_err());
    assert!(shell_history(&fake, "mercury", "bg:abc", 1).await.is_err());
    assert!(shell_history(&fake, "-oProxy", "dev-1", 1).await.is_err());
    assert_eq!(fake.calls().len(), 1);
    let blank = FakeSsh::new();
    blank.on_host("mercury", Match::Any, Reply::ok("\n\n"));
    assert_eq!(
        shell_history(&blank, "mercury", "dev-1", 1).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn fleets_model_runs_one_isolated_claude_on_the_host_and_is_booked() {
    let fake = FakeSsh::new();
    fake.on_host(
        "mercury",
        Match::script_contains("claude -p"),
        Reply::ok(&format!("{HELP_TAG}run\n{ENVELOPE}\n")),
    );
    let model = ClaudeOnHost {
        exec: &fake,
        host: "mercury".into(),
        profile: Some("work".into()),
        model: "haiku".into(),
    };
    let (a, usage) = ask(
        &model,
        Surface::Shell,
        &req("why?", "git push"),
        Some("$ git push\nfatal: no upstream"),
    )
    .await
    .unwrap();
    assert_eq!(a.answer, "The branch has no upstream.");
    assert_eq!(a.command.as_deref(), Some("git push -u origin HEAD"));
    assert_eq!(
        (a.model.as_str(), a.host_alias.as_str()),
        ("haiku", "mercury")
    );
    let script = fake.calls()[0].script().unwrap();
    assert!(script.contains("--model 'haiku'"), "{script}");
    assert!(script.contains("claude-profiles/\"'work'"), "{script}");
    assert!(script.contains("--tools ''"), "{script}");
    assert!(
        !script.contains("fatal: no upstream"),
        "never as plain text"
    );

    let store = Mutex::new(Store::open_in_memory().unwrap());
    book(&store, &a, usage.as_ref(), None);
    let rows = crate::ipc_error::lock(&store)
        .unwrap()
        .aux_usage_of_origin(crate::store::AUX_ORIGIN_CONTEXT_HELP)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].host_alias.as_str(), rows[0].cost_micros),
        ("mercury", 400)
    );
}

#[tokio::test]
async fn fleets_model_says_why_it_gave_no_answer() {
    let run = |reply: Reply| async move {
        let fake = FakeSsh::new();
        fake.on_host("mercury", Match::Any, reply);
        let model = ClaudeOnHost {
            exec: &fake,
            host: "mercury".into(),
            profile: None,
            model: "haiku".into(),
        };
        ask(&model, Surface::Shell, &req("why?", ""), None)
            .await
            .unwrap_err()
    };
    let e = run(Reply::ok(&format!("{HELP_TAG}noclaude\n"))).await;
    assert!(e.message.contains("not on mercury's login PATH"), "{e:?}");
    let e = run(Reply::Exit {
        code: 124,
        stdout: format!("{HELP_TAG}run\n").into_bytes(),
        stderr: Vec::new(),
    })
    .await;
    assert_eq!(e.code, codes::E_TIMEOUT);
    let e = run(Reply::fail(1, "Invalid API key · Please run /login")).await;
    assert!(e.message.contains("claude /login"), "{e:?}");
}

#[test]
fn the_script_refuses_a_bad_model_or_profile_and_caps_the_context() {
    assert!(help_script("gpt", None, SHELL_INSTRUCTION, "x").is_err());
    assert!(help_script("haiku", Some("../x"), SHELL_INSTRUCTION, "x").is_err());
    let long = "y".repeat(CONTEXT_MAX_BYTES * 2);
    let s = help_script("sonnet", None, SHELL_INSTRUCTION, &long).unwrap();
    assert!(s.len() < CONTEXT_MAX_BYTES * 2, "capped: {}", s.len());
    assert!(s.contains("--model 'sonnet'"));
}

/// The whole script, run by a real bash through the same two layers ssh
/// applies, against a fake `claude` that records its stdin and its argv:
/// the context arrives verbatim on stdin and in no argument.
#[cfg(unix)]
#[test]
fn the_script_hands_claude_the_context_on_stdin_and_never_in_argv() {
    use std::os::unix::fs::PermissionsExt;
    const NASTY: &str = "$(touch /tmp/pwned) `id` 'q' \"d\" \\ ; | & \n ünï ✓\n";
    let dir = tempfile::tempdir().unwrap();
    let got = dir.path().join("got");
    let argv = dir.path().join("argv");
    let fake = dir.path().join("claude");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\ncat > '{}'\nprintf '%s\\n' \"$@\" > '{}'\necho '{{}}'\n",
            got.display(),
            argv.display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let sc = help_script("haiku", None, SHELL_INSTRUCTION, NASTY).unwrap();
    let full = format!(
        "export PATH={}:\"$PATH\"; {sc}",
        crate::shell::quote(&dir.path().display().to_string())
    );
    let outer = ["bash", "-c", &crate::shell::quote(&full)].join(" ");
    let out = std::process::Command::new("bash")
        .arg("-c")
        .arg(outer)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains(&format!("{HELP_TAG}run")));
    assert_eq!(std::fs::read_to_string(&got).unwrap(), NASTY);
    let args = std::fs::read_to_string(&argv).unwrap();
    assert!(!args.contains("pwned"), "{args}");
    assert!(args.contains(SHELL_INSTRUCTION), "{args}");
}

/// The hub's path (`session_context_help`): refused while Writing help's
/// toggle is off; then the composer's help runs on the session's host
/// under the profile it was launched with, on the org's model, and is
/// booked to the session's org.
#[tokio::test]
async fn a_sessions_composer_help_runs_under_its_launch_and_is_booked_to_its_org() {
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
    let r = req("why?", "/plan");
    let ask = || ask_for_session(&store, &fake, &row, &r);

    let off = ask().await.unwrap_err();
    assert_eq!(off.code, codes::E_INVALID_STATE, "{off:?}");
    assert!(fake.calls().is_empty(), "nothing runs while it is off");

    settings::set(
        &crate::ipc_error::lock(&store).unwrap(),
        settings::WORK_CONTEXT_HELP,
        "true",
    )
    .unwrap();
    let a = ask().await.unwrap();
    assert_eq!(
        (a.model.as_str(), a.host_alias.as_str()),
        ("haiku", "mercury")
    );
    let script = fake.calls()[0].script().unwrap();
    assert!(script.contains("claude-profiles/\"'work'"), "{script}");
    let rows = crate::ipc_error::lock(&store)
        .unwrap()
        .aux_usage_of_origin(crate::store::AUX_ORIGIN_CONTEXT_HELP)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].org_id, Some(org));
}
