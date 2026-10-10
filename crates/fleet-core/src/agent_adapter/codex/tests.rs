//! The Codex adapter against screens and a rollout captured from
//! codex-cli 0.162.0 in a 120×40 tmux pane (`testdata/codex/`), driven by a
//! local stand-in for the Responses API so every state could be produced
//! on demand. Paths were rewritten to `/home/dev/repo`.

use super::*;
use crate::agent_adapter::{by_id, for_session};

const SIGN_IN: &str = include_str!("../testdata/codex/sign_in.txt");
const TRUST: &str = include_str!("../testdata/codex/trust_folder.txt");
const IDLE: &str = include_str!("../testdata/codex/idle_after_turn.txt");
const STREAMING: &str = include_str!("../testdata/codex/working_streaming.txt");
const THINKING: &str = include_str!("../testdata/codex/working_thinking.txt");
const APPROVAL: &str = include_str!("../testdata/codex/approval_command.txt");
const QUESTION: &str = include_str!("../testdata/codex/question_plan_mode.txt");
const INTERRUPTED: &str = include_str!("../testdata/codex/interrupted.txt");
const ROLLOUT: &str = include_str!("../testdata/codex/rollout.jsonl");

const ID: &str = "01a11dac-6e90-7da0-81c9-280b14a35226";

fn codex() -> &'static dyn AgentAdapter {
    by_id("codex").expect("codex has an adapter")
}

#[test]
fn a_codex_row_gets_the_codex_adapter_and_a_shell_none() {
    assert_eq!(codex().label(), "Codex");
    assert_eq!(
        for_session("work", Some("codex")).map(|a| a.id()),
        Some("codex")
    );
    assert_eq!(
        for_session("work", Some("claude")).map(|a| a.id()),
        Some("claude")
    );
    assert_eq!(for_session("work", None).map(|a| a.id()), Some("claude"));
    assert_eq!(
        for_session("work", Some("agy")).map(|a| a.id()),
        Some("agy")
    );
    // An agent with no adapter reads as Claude Code, as every row did.
    assert_eq!(
        for_session("work", Some("nope")).map(|a| a.id()),
        Some("claude")
    );
    assert!(for_session("shell", Some("codex")).is_none());
}

#[test]
fn codex_resumes_by_id_falls_back_fresh_and_starts_bare() {
    let a = codex();
    let launch = ClaudeLaunch {
        model: Some("gpt-6.1-sol".into()),
        effort: Some("high".into()),
        profile: Some("ignored".into()),
    };
    let flags = "--dangerously-bypass-approvals-and-sandbox -m 'gpt-6.1-sol' -c 'model_reasoning_effort=\"high\"'";
    assert_eq!(
        a.launch_command(Some(ID), "dev-x", &launch),
        format!("codex resume '{ID}' {flags} 2>/dev/null || codex {flags}; exec ${{SHELL:-/bin/zsh}} -l")
    );
    assert_eq!(
        a.launch_command(None, "dev-x", &ClaudeLaunch::default()),
        "codex resume --last --dangerously-bypass-approvals-and-sandbox \
         || codex --dangerously-bypass-approvals-and-sandbox; exec ${SHELL:-/bin/zsh} -l"
    );
    assert_eq!(
        a.start_command("dev-x", &ClaudeLaunch::default()),
        "codex --dangerously-bypass-approvals-and-sandbox; exec ${SHELL:-/bin/zsh} -l"
    );
    // Codex names its own conversations; a profile is not Codex's.
    assert!(a.mint_conversation_id().is_none());
    assert!(!a
        .launch_command(Some(ID), "x", &launch)
        .contains("CLAUDE_CONFIG_DIR"));
    assert!(a.valid_conversation_id(ID));
    assert!(!a.valid_conversation_id("x'; rm -rf ~"));
}

/// The pane line runs under `sh -c`: what it is given is quoted.
#[test]
fn codex_launch_line_quotes_what_it_is_given() {
    let launch = ClaudeLaunch {
        model: Some("m'; touch /tmp/pwned; '".into()),
        effort: None,
        profile: None,
    };
    let line = codex().start_command("x", &launch);
    assert!(
        line.contains(&crate::shell::quote("m'; touch /tmp/pwned; '")),
        "{line}"
    );
}

#[test]
fn codex_lists_hold_values_a_session_stores() {
    for e in codex().effort_levels() {
        assert!(
            crate::validate::effort_level(e.value).is_ok(),
            "{}",
            e.value
        );
    }
    for m in codex().models() {
        assert!(
            crate::validate::claude_model(m.value).is_ok(),
            "{}",
            m.value
        );
    }
    let names: Vec<&str> = codex().slash_commands().iter().map(|c| c.name).collect();
    assert!(
        names.contains(&"model") && names.contains(&"compact"),
        "{names:?}"
    );
    // `/model` opens Codex's picker; no sent line switches the stored model.
    assert_eq!(codex().launch_switch("/model gpt-6-luna"), None);
}

#[test]
fn the_sign_in_screen_is_an_auth_menu_not_a_press_enter() {
    let i = codex().analyze_pane(SIGN_IN);
    assert_eq!(i.stuck, Some(StuckKind::AuthMenu));
    assert_eq!(i.derived_status, Some(ClaudeStatus::Blocked));
    assert_eq!(i.pending_input, None);
}

#[test]
fn the_folder_trust_screen_is_a_trust_prompt() {
    let i = codex().analyze_pane(TRUST);
    assert_eq!(i.stuck, Some(StuckKind::TrustPrompt));
    assert_eq!(i.derived_status, Some(ClaudeStatus::Blocked));
    // Once trusted, the same words higher up the scrollback are history.
    let after = format!("{TRUST}\n{IDLE}");
    assert_eq!(codex().analyze_pane(&after).stuck, None);
}

#[test]
fn an_idle_composer_reads_idle_and_names_the_last_item() {
    for (name, pane) in [("idle", IDLE), ("interrupted", INTERRUPTED)] {
        let i = codex().analyze_pane(pane);
        assert_eq!(i.derived_status, Some(ClaudeStatus::Idle), "{name}");
        assert_eq!(i.stuck, None, "{name}");
        assert_eq!(i.waiting_for, None, "{name}");
        assert_eq!(codex().spinner_line(pane), None, "{name}");
    }
    assert_eq!(
        codex().analyze_pane(IDLE).activity.as_deref(),
        Some("• Done.")
    );
    assert_eq!(
        codex().analyze_pane(INTERRUPTED).activity.as_deref(),
        Some("■ Conversation interrupted - use /feedback if something went wrong")
    );
}

#[test]
fn a_turn_in_flight_reads_working() {
    // Streaming: only the status line's spinner frame says so.
    assert_eq!(
        codex().analyze_pane(STREAMING).derived_status,
        Some(ClaudeStatus::Working)
    );
    // Thinking: the `• Working (…)` line as well.
    let i = codex().analyze_pane(THINKING);
    assert_eq!(i.derived_status, Some(ClaudeStatus::Working));
    assert_eq!(
        codex().spinner_line(THINKING).as_deref(),
        Some("Working (4s • esc to interrupt)")
    );
}

/// The prompt card: the approval's question, the command, and its three
/// choices without their key hints. A digit key answers it (checked live:
/// `1` approved the command at once), so the card's keys work unchanged.
#[test]
fn a_command_approval_is_a_permission_card() {
    let i = codex().analyze_pane(APPROVAL);
    assert_eq!(i.derived_status, Some(ClaudeStatus::Blocked));
    assert_eq!(i.waiting_for, Some(WaitingFor::Permission));
    let p = i.pending_input.expect("a card");
    assert_eq!(p.kind, "permission");
    assert_eq!(
        p.question.as_deref(),
        Some("Would you like to run the following command?")
    );
    assert_eq!(p.detail.as_deref(), Some("curl -sI https://example.com"));
    let labels: Vec<(u8, &str, bool)> = p
        .options
        .iter()
        .map(|o| (o.n, o.label.as_str(), o.selected))
        .collect();
    assert_eq!(
        labels,
        [
            (1, "Yes, proceed", true),
            (
                2,
                "Yes, and don't ask again for commands that start with `curl -sI https://example.com`",
                false
            ),
            (3, "No, and tell Codex what to do differently", false),
        ]
    );
    assert_eq!(
        i.activity.as_deref(),
        Some("waiting for permission: Would you like to run the following command?")
    );
}

/// An MCP server's approval has no "Would you like to" line; its title is
/// the question.
#[test]
fn an_mcp_elicitation_names_its_server_as_the_question() {
    let pane = APPROVAL.replace(
        "Would you like to run the following command?",
        "github needs your approval.",
    );
    let p = codex().analyze_pane(&pane).pending_input.expect("a card");
    assert_eq!(p.question.as_deref(), Some("github needs your approval."));
}

#[test]
fn a_plan_mode_question_is_an_input_card() {
    let i = codex().analyze_pane(QUESTION);
    assert_eq!(i.derived_status, Some(ClaudeStatus::Blocked));
    let p = i.pending_input.expect("a card");
    assert_eq!(p.kind, "input");
    assert_eq!(
        p.question.as_deref(),
        Some("Which database should the service use?")
    );
    let labels: Vec<&str> = p.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(
        labels,
        ["Postgres (Recommended)", "SQLite", "None of the above"]
    );
    assert!(p.options[0].selected && !p.multi);
}

/// A dialog Codex already answered is scrollback under a live composer.
#[test]
fn an_answered_dialog_is_not_a_card() {
    let after = format!("{APPROVAL}\n{IDLE}");
    let i = codex().analyze_pane(&after);
    assert_eq!(i.pending_input, None);
    assert_eq!(i.derived_status, Some(ClaudeStatus::Idle));
}

#[test]
fn the_rollout_reads_as_turns_with_tools_and_answers() {
    let turns = codex().parse_transcript(ROLLOUT);
    let prompts: Vec<Option<&str>> = turns.iter().map(|t| t.prompt.as_deref()).collect();
    // The harness's own `<environment_context>` messages are not prompts.
    assert_eq!(
        prompts,
        [
            Some("please run the check"),
            Some("think deeply"),
            Some("please ask me something")
        ]
    );
    let first = &turns[0];
    assert!(first.prompt_uuid.is_some());
    match &first.items[0] {
        ConvItem::Tool {
            name,
            summary,
            target,
            done,
            error,
            id,
            ..
        } => {
            assert_eq!(name, "exec_command");
            assert_eq!(summary, "exec_command(curl -sI https://example.com)");
            assert_eq!(target.as_deref(), Some("curl -sI https://example.com"));
            assert!(*done, "its output was read");
            assert!(*error, "curl exited 56");
            assert_eq!(id.as_deref(), Some("call_resp_0"));
        }
        other => panic!("expected the command, got {other:?}"),
    }
    assert_eq!(
        first.items[1],
        ConvItem::Text {
            text: "Done.".into()
        }
    );
    assert!(first.ended_at.is_some());
    // The plan-mode question is a tool call naming its question.
    match &turns[2].items[0] {
        ConvItem::Tool { target, error, .. } => {
            assert_eq!(
                target.as_deref(),
                Some("Which database should the service use?")
            );
            assert!(!error);
        }
        other => panic!("expected the question, got {other:?}"),
    }
}

/// A read that starts mid-file: a torn first line is skipped, and output
/// before any prompt lands in a prompt-less turn.
#[test]
fn a_rollout_tail_starting_mid_turn_still_reads() {
    let tail: String = ROLLOUT
        .lines()
        .skip_while(|l| !l.contains("\"role\": \"assistant\""))
        .collect::<Vec<_>>()
        .join("\n");
    let torn = format!("{{\"timestamp\": \"2026-10\n{tail}");
    let turns = codex().parse_transcript(&torn);
    assert_eq!(turns[0].prompt, None);
    assert_eq!(
        turns[0].items[0],
        ConvItem::Text {
            text: "Done.".into()
        }
    );
}

#[test]
fn a_tool_call_detail_reads_its_command_and_failed_output() {
    let d = codex()
        .tool_detail(ROLLOUT, "call_resp_0")
        .expect("the call is in the rollout");
    assert_eq!(d.name, "exec_command");
    assert_eq!(d.command.as_deref(), Some("curl -sI https://example.com"));
    assert!(d.input.contains("\"cmd\""), "{}", d.input);
    assert!(d.is_error, "curl exited 56");
    assert!(d.result.is_some_and(|r| r.contains("exited with code 56")));
    assert!(codex().tool_detail(ROLLOUT, "call_nope").is_none());
}

#[test]
fn the_context_is_the_last_token_count() {
    let c = codex()
        .context_usage(ROLLOUT)
        .expect("the rollout counts tokens");
    assert!(c.tokens > 0 && c.window > c.tokens, "{c:?}");
    assert_eq!(codex().context_usage("{\"type\":\"event_msg\"}"), None);
}

#[test]
fn rollout_lookup_lines_carry_the_id_from_the_file_name() {
    let path = format!("/h/.codex/sessions/2026/10/08/rollout-2026-10-08T22-40-02-{ID}.jsonl");
    let out = format!("cdx\tdev-x\t{path}\ncdx\tdev-y\t/h/x/rollout-bogus.jsonl\nnoise\n");
    let found = super::parse_rollouts(&out);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found["dev-x"].id, ID);
    assert_eq!(found["dev-x"].path, path);
    assert!(super::rollouts_script(&[]).is_none());
}

#[test]
fn two_panes_on_one_rollout_learn_nothing() {
    let path = format!("/h/.codex/sessions/2026/10/08/rollout-2026-10-08T22-40-02-{ID}.jsonl");
    let out = format!("cdx\tdev-a\t{path}\ncdx\tdev-b\t{path}\n");
    assert!(super::parse_rollouts(&out).is_empty());
}

/// The lookup run as the host runs it: a stand-in `tmux` answers each
/// pane's cwd, and the newest rollout naming that cwd wins.
#[cfg(unix)]
#[test]
fn the_rollout_lookup_finds_each_panes_newest_rollout_for_its_cwd() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    // `-t '=name:'` → `/work/<name>`.
    std::fs::write(
        bin.join("tmux"),
        "#!/bin/sh\nfor a; do t=\"$a\"; case \"$prev\" in -t) n=\"${a#=}\"; n=\"${n%:}\";; esac; prev=\"$a\"; done\n\
         [ \"$n\" = gone ] && exit 1\nprintf '/work/%s\\n' \"$n\"\n",
    )
    .unwrap();
    std::fs::set_permissions(bin.join("tmux"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let day = dir.path().join("codex/sessions/2026/10/08");
    std::fs::create_dir_all(&day).unwrap();
    let ids = [
        "01a11dac-0000-7000-8000-000000000001",
        "01a11dac-0000-7000-8000-000000000002",
        "01a11dac-0000-7000-8000-000000000003",
    ];
    let write = |n: usize, cwd: &str, age: u64| {
        let f = day.join(format!("rollout-2026-10-08T22-40-0{n}-{}.jsonl", ids[n]));
        std::fs::write(
            &f,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{}\",\"cwd\":\"{cwd}\"}}}}\n",
                ids[n]
            ),
        )
        .unwrap();
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(age);
        std::fs::File::options()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(t)
            .unwrap();
    };
    write(0, "/work/dev-a", 300);
    write(1, "/work/dev-a", 10);
    write(2, "/work/dev-b-other", 5);
    let script = super::rollouts_script(&["dev-a", "dev-b", "gone"]).unwrap();
    let out = std::process::Command::new("bash")
        .arg("-c")
        .arg(&script)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("CODEX_HOME", dir.path().join("codex"))
        .output()
        .unwrap();
    let found = super::parse_rollouts(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(
        found.len(),
        1,
        "{found:?} {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(found["dev-a"].id, ids[1], "the newer of dev-a's two");
}
