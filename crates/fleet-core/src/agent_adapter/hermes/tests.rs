//! The Hermes adapter against screens laid out from hermes-agent 0.19.0's
//! render code (`cli.py`), not captured from a running Hermes: see the
//! module docs. Box widths and spacing follow `_append_panel_line`
//! (`│ ` + text padded to the box + ` │`) and `_build_status_bar_text`.

use super::*;
use crate::agent_adapter::{by_id, for_session};

fn hermes() -> &'static dyn AgentAdapter {
    by_id("hermes").expect("hermes has an adapter")
}

const ID: &str = "20261103_152210_a1b2c3";

/// The REPL between turns: the last reply above, then the status bar, the
/// rules and an empty input line.
const IDLE: &str = "\
╭─ ⚕ Hermes ───────────────────────────────────────────────────────────╮
│ Done: the tests pass.                                                │
╰──────────────────────────────────────────────────────────────────────╯

⚕ claude-sonnet-5 │ 12.4K/200K │ 6% │ 3m │ idle 12s │ ⚠ YOLO
────────────────────────────────────────────────────────────────────────
❯ 
────────────────────────────────────────────────────────────────────────
";

/// Mid-turn: the spinner line above the spacer, `⚕ ❯` on the input line.
const WORKING: &str = "\
  ┊ 💻 terminal  cargo test  2.1s

  💻 terminal  ( 12.3s)

⚕ claude-sonnet-5 │ 13.0K/200K │ 7% │ 3m │ 12s │ ⚠ YOLO
────────────────────────────────────────────────────────────────────────
⚕ ❯ 
────────────────────────────────────────────────────────────────────────
";

/// A dangerous-command approval (without `--yolo`, or after `/yolo off`).
const APPROVAL: &str = "\
╭────────────────────────────────────────────────────────────╮
│ ⚠️  Dangerous Command                                       │
│                                                            │
│ rm -rf build/                                              │
│                                                            │
│ ❯ 1. Allow once                                            │
│   2. Allow for this session                                │
│   3. Add to permanent allowlist                            │
│   4. Deny                                                  │
│                                                            │
│ Recursive delete of a directory                            │
╰────────────────────────────────────────────────────────────╯

⚕ claude-sonnet-5 │ 13.2K/200K │ 7% │ 4m │ ⚠ YOLO
────────────────────────────────────────────────────────────────────────
⚠ ❯ 
────────────────────────────────────────────────────────────────────────
";

/// The clarify tool's question panel.
const QUESTION: &str = "\
╭─ Hermes needs your input ──────────────────────────────────╮
│                                                            │
│ Which database should the service use?                     │
│                                                            │
│ ❯ 1. Postgres                                              │
│   2. SQLite                                                │
│   3. Other (type your answer)                              │
│                                                            │
╰────────────────────────────────────────────────────────────╯

⚕ claude-sonnet-5 │ 13.4K/200K │ 7% │ 4m
────────────────────────────────────────────────────────────────────────
? ❯ 
────────────────────────────────────────────────────────────────────────
";

/// The first-run guard when no provider is configured.
const FIRST_RUN: &str = "
It looks like Hermes isn't configured yet -- no API keys or providers found.

  Run:  hermes setup

Run setup now? [Y/n] ";

/// Hermes exited; the fallback shell's prompt (a `❯` theme) took over.
const EXITED: &str = "\
────────────────────────────────────────────────────────────────────────
❯ /quit
────────────────────────────────────────────────────────────────────────
Resume this session with: hermes --resume 20261103_152210_a1b2c3

~/repo on main
❯ 
";

#[test]
fn a_hermes_row_gets_the_hermes_adapter() {
    assert_eq!(hermes().label(), "Hermes");
    assert_eq!(hermes().id(), crate::store::AGENT_HERMES);
    assert!(crate::store::AGENTS.contains(&hermes().id()));
    assert_eq!(
        for_session("work", Some("hermes")).map(|a| a.id()),
        Some("hermes")
    );
    assert!(for_session("shell", Some("hermes")).is_none());
}

#[test]
fn hermes_resumes_by_id_falls_back_fresh_and_starts_bare() {
    let a = hermes();
    let launch = ClaudeLaunch {
        model: Some("anthropic/claude-sonnet-5".into()),
        effort: Some("high".into()),
        profile: Some("ignored".into()),
    };
    let path = crate::tmux::VOICE_PATH_PREFIX;
    let base = "hermes chat --cli --yolo";
    let m = " -m 'anthropic/claude-sonnet-5'";
    assert_eq!(
        a.launch_command(Some(ID), "dev-x", &launch),
        format!("{path}{base} --resume '{ID}'{m} 2>/dev/null || {base}{m}; exec ${{SHELL:-/bin/zsh}} -l")
    );
    assert_eq!(
        a.start_command("dev-x", &launch),
        format!("{path}{base}{m}; exec ${{SHELL:-/bin/zsh}} -l")
    );
    // No effort flag (Hermes has none) and no profile.
    let cont = a.launch_command(None, "dev-x", &launch);
    assert!(
        !cont.contains("high") && !cont.contains("ignored"),
        "{cont}"
    );
    // Without an id: the cwd's newest CLI session from state.db, never
    // Hermes' own `-c` (newest anywhere, and it changes directory).
    assert!(cont.contains("id=$(python3 -c '"), "{cont}");
    assert!(cont.contains("state.db"), "{cont}");
    assert!(
        cont.contains(&format!(
            "then {base} --resume \"$id\"{m} || {base}{m}; else {base}{m}; fi"
        )),
        "{cont}"
    );
    assert!(
        !cont.contains("--yolo -c") && !cont.contains("--continue"),
        "{cont}"
    );
    assert!(!cont.contains('\n'), "one line: {cont}");
    assert_eq!(
        a.start_command("dev-x", &ClaudeLaunch::default()),
        format!("{path}{base}; exec ${{SHELL:-/bin/zsh}} -l")
    );
}

#[test]
fn hermes_ids_are_its_own_timestamp_shape() {
    let a = hermes();
    assert!(a.valid_conversation_id(ID));
    for bad in [
        "",
        "20261103_152210_a1b2c",
        "20261103_152210_A1B2C3",
        "20261103-152210_a1b2c3",
        "x'; rm -rf ~ #_a1b2c3xx",
        "0b6f1f9e-3c2a-4b1e-9a8d-1c2b3d4e5f60",
    ] {
        assert!(!a.valid_conversation_id(bad), "{bad}");
    }
    assert_eq!(a.mint_conversation_id(), None);
}

#[test]
fn hermes_model_switch_is_kept_for_a_relaunch() {
    let a = hermes();
    assert_eq!(
        a.launch_switch("/model anthropic/claude-opus-4.8"),
        Some(LaunchSwitch::Model(Some("anthropic/claude-opus-4.8")))
    );
    assert_eq!(a.launch_switch("/model"), None, "the picker");
    assert_eq!(a.launch_switch("/model x --global"), None);
    assert_eq!(a.launch_switch("/reasoning high"), None);
    assert_eq!(a.launch_switch("fix the bug"), None);
    assert!(a.effort_levels().is_empty());
    assert!(a.slash_commands().iter().any(|c| c.name == "model"));
}

#[test]
fn an_idle_repl_reads_idle_with_its_context() {
    let intel = hermes().analyze_pane(IDLE);
    assert_eq!(intel.derived_status, Some(ClaudeStatus::Idle));
    assert_eq!(intel.context_pct, Some(6.0));
    assert_eq!(intel.stuck, None);
    assert!(intel.pending_input.is_none());
    assert_eq!(hermes().spinner_line(IDLE), None);
}

#[test]
fn a_working_repl_reads_working_with_its_spinner() {
    let intel = hermes().analyze_pane(WORKING);
    assert_eq!(intel.derived_status, Some(ClaudeStatus::Working));
    assert_eq!(intel.activity.as_deref(), Some("💻 terminal  ( 12.3s)"));
    assert_eq!(intel.context_pct, Some(7.0));
    assert_eq!(
        hermes().spinner_line(WORKING).as_deref(),
        Some("💻 terminal  ( 12.3s)")
    );
}

#[test]
fn an_approval_panel_becomes_a_permission_card() {
    let intel = hermes().analyze_pane(APPROVAL);
    assert_eq!(intel.derived_status, Some(ClaudeStatus::Blocked));
    assert_eq!(intel.waiting_for, Some(WaitingFor::Permission));
    let p = intel.pending_input.expect("a card");
    assert_eq!(p.kind, "permission");
    assert_eq!(
        p.question.as_deref(),
        Some("Recursive delete of a directory")
    );
    assert_eq!(p.detail.as_deref(), Some("rm -rf build/"));
    let labels: Vec<_> = p
        .options
        .iter()
        .map(|o| (o.n, o.label.as_str(), o.selected))
        .collect();
    assert_eq!(
        labels,
        [
            (1, "Allow once", true),
            (2, "Allow for this session", false),
            (3, "Add to permanent allowlist", false),
            (4, "Deny", false),
        ]
    );
    assert_eq!(
        intel.activity.as_deref(),
        Some("waiting for permission: Recursive delete of a directory")
    );
}

#[test]
fn a_question_panel_becomes_an_input_card() {
    let intel = hermes().analyze_pane(QUESTION);
    assert_eq!(intel.derived_status, Some(ClaudeStatus::Blocked));
    assert_eq!(intel.waiting_for, Some(WaitingFor::Input));
    let p = intel.pending_input.expect("a card");
    assert_eq!(
        p.question.as_deref(),
        Some("Which database should the service use?")
    );
    let labels: Vec<_> = p.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(labels, ["Postgres", "SQLite", "Other (type your answer)"]);
    assert!(p.options[0].selected);
}

#[test]
fn a_panel_left_in_scrollback_is_not_a_card() {
    // The approval was answered: the panel scrolled up, the turn went on.
    let pane = format!("{APPROVAL}\n{WORKING}");
    let intel = hermes().analyze_pane(&pane);
    assert_eq!(intel.derived_status, Some(ClaudeStatus::Working));
    assert!(intel.pending_input.is_none());
}

#[test]
fn the_first_run_guard_is_a_sign_in_stuck_state() {
    let intel = hermes().analyze_pane(FIRST_RUN);
    assert_eq!(intel.stuck, Some(StuckKind::AuthMenu));
    assert_eq!(intel.derived_status, Some(ClaudeStatus::Blocked));
}

#[test]
fn a_shell_prompt_after_hermes_exits_is_not_read_as_hermes() {
    let intel = hermes().analyze_pane(EXITED);
    assert_eq!(intel.derived_status, None);
    assert!(intel.pending_input.is_none());
}

#[test]
fn a_profile_prompt_reads_idle_and_a_password_blocked() {
    let profile = IDLE.replace("\n❯ \n", "\ncoder ❯ \n");
    assert_eq!(
        hermes().analyze_pane(&profile).derived_status,
        Some(ClaudeStatus::Idle)
    );
    let password = IDLE.replace("\n❯ \n", "\n🔐 ❯ \n");
    let intel = hermes().analyze_pane(&password);
    assert_eq!(intel.derived_status, Some(ClaudeStatus::Blocked));
    assert_eq!(intel.activity.as_deref(), Some("waiting for a password"));
}

#[test]
fn hermes_has_no_transcript_reader_yet() {
    assert!(hermes().parse_transcript("{}").is_empty());
}
