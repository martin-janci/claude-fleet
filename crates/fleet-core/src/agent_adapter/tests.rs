use super::*;
use crate::tmux::{pane_command_with, ClaudeLaunch};

#[test]
fn claude_is_the_adapter_for_every_kind_but_shell() {
    for kind in ["work", "review", "bg", "external"] {
        assert_eq!(for_kind(kind).map(|a| a.id()), Some("claude"), "{kind}");
    }
    assert!(for_kind("shell").is_none());
    assert_eq!(by_id("claude").map(|a| a.label()), Some("Claude Code"));
    for no_adapter_yet in ["shell", "nope"] {
        assert!(by_id(no_adapter_yet).is_none(), "{no_adapter_yet}");
    }
}

/// No behaviour change: the adapter's launch line is the one
/// `tmux::pane_command_with` built before it, for every shape of launch.
#[test]
fn claude_launches_exactly_as_before() {
    let a = claude();
    let launches = [
        ClaudeLaunch::default(),
        ClaudeLaunch {
            model: Some("opus[1m]".into()),
            effort: Some("high".into()),
            profile: Some("work".into()),
        },
    ];
    for launch in &launches {
        for id in [None, Some("0b6f1f9e-3c2a-4b1e-9a8d-1c2b3d4e5f60")] {
            assert_eq!(
                a.launch_command(id, "dev-x", launch),
                pane_command_with(id, "dev-x", launch)
            );
        }
    }
    let minted = a.mint_conversation_id().expect("Claude Code mints its ids");
    assert!(a.valid_conversation_id(&minted), "{minted}");
    assert_ne!(Some(minted), a.mint_conversation_id());
    assert!(!a.valid_conversation_id("x'; rm -rf ~"));
}

#[test]
fn claude_effort_levels_are_the_ones_launch_validates() {
    let values: Vec<&str> = claude().effort_levels().iter().map(|o| o.value).collect();
    assert_eq!(values, crate::validate::EFFORT_LEVELS);
}

#[test]
fn claude_reads_panes_and_transcripts_as_before() {
    let a = claude();
    let tail = "Do you trust the files in this folder?\n  ❯ 1. Yes, proceed\n  2. No\n";
    assert_eq!(
        a.analyze_pane(tail),
        crate::service::pane_intel::analyze(tail)
    );
    let spin = "✻ Cogitating… (12s · ↑ 1.2k tokens · esc to interrupt)\n";
    assert_eq!(
        a.spinner_line(spin),
        crate::service::pane_intel::spinner_line(spin)
    );
    let jsonl = r#"{"type":"user","message":{"role":"user","content":"hi"},"uuid":"u1"}"#;
    assert_eq!(
        a.parse_transcript(jsonl),
        crate::service::transcript::parse_conversation(jsonl)
    );
}

/// The `{ value: 'x', label: 'y' }` (or `{ name: …, description: …, args:
/// true }`) entries of one `export const NAME … = [ … ];` array in a TS file,
/// as `(first, second, args)`.
fn ts_entries(src: &str, name: &str, keys: (&str, &str)) -> Vec<(String, String, bool)> {
    let start = src
        .find(&format!("export const {name}"))
        .unwrap_or_else(|| panic!("{name} not found"));
    let body = &src[start..];
    let body = &body[body.find('[').unwrap() + 1..body.find("];").unwrap()];
    let field = |line: &str, key: &str| -> Option<String> {
        let at = line.find(&format!("{key}: '"))? + key.len() + 3;
        let rest = &line[at..];
        Some(rest[..rest.find('\'')?].to_string())
    };
    body.lines()
        .filter_map(|l| {
            Some((
                field(l, keys.0)?,
                field(l, keys.1)?,
                l.contains("args: true"),
            ))
        })
        .collect()
}

/// The composer's lists (`src/lib/conversation.ts`) are the adapter's, so
/// the adapter is where a second agent's lists go.
#[test]
fn claude_lists_mirror_the_composer() {
    let ts = crate::repo_files::read("src/lib/conversation.ts");
    let a = claude();
    let pick = |opts: &[PickerOption]| -> Vec<(String, String, bool)> {
        opts.iter()
            .map(|o| (o.value.to_string(), o.label.to_string(), false))
            .collect()
    };
    assert_eq!(
        ts_entries(&ts, "MODEL_OPTIONS", ("value", "label")),
        pick(a.models())
    );
    assert_eq!(
        ts_entries(&ts, "LAUNCH_EFFORT_OPTIONS", ("value", "label")),
        pick(a.effort_levels())
    );
    let slash: Vec<(String, String, bool)> = a
        .slash_commands()
        .iter()
        .map(|c| (c.name.to_string(), c.description.to_string(), c.args))
        .collect();
    assert_eq!(
        ts_entries(&ts, "SLASH_COMMANDS", ("name", "description")),
        slash
    );
}

/// The composer's Codex lists (`src/lib/conversation.ts` CODEX_*) are the
/// Codex adapter's, so a session's pickers offer what `codex -m` takes.
#[test]
fn codex_lists_mirror_the_composer() {
    let ts = crate::repo_files::read("src/lib/conversation.ts");
    let a = by_id(crate::store::AGENT_CODEX).expect("codex adapter");
    let pick = |opts: &[PickerOption]| -> Vec<(String, String, bool)> {
        opts.iter()
            .map(|o| (o.value.to_string(), o.label.to_string(), false))
            .collect()
    };
    assert_eq!(
        ts_entries(&ts, "CODEX_MODEL_OPTIONS", ("value", "label")),
        pick(a.models())
    );
    assert_eq!(
        ts_entries(&ts, "CODEX_EFFORT_OPTIONS", ("value", "label")),
        pick(a.effort_levels())
    );
    let slash: Vec<(String, String, bool)> = a
        .slash_commands()
        .iter()
        .map(|c| (c.name.to_string(), c.description.to_string(), c.args))
        .collect();
    assert_eq!(
        ts_entries(&ts, "CODEX_SLASH_COMMANDS", ("name", "description")),
        slash
    );
}

// Agy (12.3). PROVISIONAL: no machine fleet runs on has agy, so the panes
// and the stream below are transcribed from agy's documentation, not
// captured. Replace them with `tmux capture-pane -p` output from a real agy.

#[test]
fn agy_is_known_by_id() {
    let a = by_id("agy").expect("agy adapter");
    assert_eq!(a.id(), crate::store::AGENT_AGY);
    assert_eq!(a.label(), "Agy");
    assert!(crate::store::AGENTS.contains(&a.id()));
}

#[test]
fn agy_launches_resume_then_continue() {
    let a = by_id("agy").unwrap();
    let id = "0b6f1f9e-3c2a-4b1e-9a8d-1c2b3d4e5f60";
    let plain = a.launch_command(None, "dev-x", &ClaudeLaunch::default());
    assert!(plain.contains("agy --continue; exec"), "{plain}");
    assert!(
        !plain.contains("--name"),
        "agy takes no session name: {plain}"
    );

    let full = ClaudeLaunch {
        model: Some("gemini-3.8-flash".into()),
        effort: Some("high".into()),
        profile: Some("work".into()),
    };
    let cmd = a.launch_command(Some(id), "dev-x", &full);
    assert!(
        cmd.contains(&format!(
            "agy --conversation '{id}' --model 'gemini-3.8-flash' --effort 'high' 2>/dev/null \
             || agy --continue --model 'gemini-3.8-flash' --effort 'high';"
        )),
        "{cmd}"
    );
    assert!(
        !cmd.contains("CLAUDE_CONFIG_DIR"),
        "a Claude profile does not apply to agy: {cmd}"
    );
    assert!(cmd.ends_with("exec ${SHELL:-/bin/zsh} -l"), "{cmd}");
}

#[test]
fn agy_drops_what_it_does_not_take() {
    let a = by_id("agy").unwrap();
    for effort in ["xhigh", "max"] {
        let launch = ClaudeLaunch {
            model: Some("default".into()),
            effort: Some(effort.into()),
            profile: None,
        };
        let cmd = a.launch_command(None, "x", &launch);
        assert!(!cmd.contains("--effort"), "{effort}: {cmd}");
        assert!(!cmd.contains("--model"), "default is no flag: {cmd}");
    }
    let values: Vec<&str> = a.effort_levels().iter().map(|o| o.value).collect();
    assert_eq!(values, ["low", "medium", "high"]);
    assert!(a
        .models()
        .iter()
        .all(|m| crate::validate::claude_model(m.value).is_ok()));
    assert!(a.launch_switch("/model gemini-3.8-flash").is_none());
    assert!(a.slash_commands().iter().any(|c| c.name == "resume"));
    assert_eq!(a.mint_conversation_id(), None, "agy allocates its own ids");
    assert!(a.valid_conversation_id("0b6f1f9e-3c2a-4b1e-9a8d-1c2b3d4e5f60"));
    assert!(!a.valid_conversation_id("x'; rm -rf ~"));
}

/// agy's tool approval dialog, as the Real Python walk-through prints it.
#[test]
fn agy_approval_dialog_reads_as_a_permission_prompt() {
    use crate::service::pane_intel::{ClaudeStatus, WaitingFor};
    let pane = "\
● ListDir(/home/me/expense-report)
Command
  Requesting permission for: python cli.py transactions.csv
Do you want to proceed?
> 1. Yes
  2. Yes, and always allow in this conversation for commands that start with 'python cli.py transactions.csv'
  3. Yes, and always allow for commands that start with 'python cli.py transactions.csv' (Persist to settings.json)
  4. No
";
    let intel = by_id("agy").unwrap().analyze_pane(pane);
    assert_eq!(intel.derived_status, Some(ClaudeStatus::Blocked));
    assert_eq!(intel.waiting_for, Some(WaitingFor::Permission));
    let input = intel.pending_input.expect("pending input");
    assert_eq!(input.question.as_deref(), Some("Do you want to proceed?"));
    assert_eq!(input.options.len(), 4);
    assert!(input.options[0].selected && input.options[0].label == "Yes");
    assert_eq!(input.options[3].label, "No");
}

/// The same dialog with agy's `> ` input line drawn below it is scrollback.
#[test]
fn agy_answered_dialog_above_the_prompt_is_not_pending() {
    let pane = "\
Do you want to proceed?
  1. Yes
  4. No
● Ran python cli.py transactions.csv
────────────────────────────────────────
> 
";
    let intel = by_id("agy").unwrap().analyze_pane(pane);
    assert!(intel.pending_input.is_none(), "{intel:?}");
}

/// Headless `--output-format stream-json`, shaped as agy's headless page
/// documents it.
#[test]
fn agy_reads_a_stream_json_conversation() {
    let ndjson = r#"{"event":"init","cwd":"/w","tools":[],"permission_mode":"request-review"}
{"event":"step_update","conversation_id":"c","step_index":0,"state":"DONE","step_type":"user_input","text_delta":"Fix the bug"}
{"event":"step_update","conversation_id":"c","step_index":1,"state":"ACTIVE","step_type":"tool","tool_name":"run_command"}
{"event":"step_update","conversation_id":"c","step_index":1,"state":"DONE","step_type":"tool","tool_name":"run_command"}
{"event":"step_update","conversation_id":"c","step_index":2,"state":"ACTIVE","step_type":"agent_response","text_delta":"Fixed "}
{"event":"step_update","conversation_id":"c","step_index":2,"state":"ACTIVE","step_type":"agent_response","text_delta":"it."}
{"event":"step_update","conversation_id":"c","step_index":2,"state":"DONE","step_type":"agent_response"}
not json
{"event":"result","conversation_id":"c","status":"SUCCESS","response":"Fixed it."}"#;
    let turns = by_id("agy").unwrap().parse_transcript(ndjson);
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].prompt.as_deref(), Some("Fix the bug"));
    assert_eq!(turns[0].items.len(), 2);
    assert!(
        matches!(&turns[0].items[0], crate::service::transcript::ConvItem::Tool { name, .. } if name == "run_command")
    );
    assert!(
        matches!(&turns[0].items[1], crate::service::transcript::ConvItem::Text { text } if text == "Fixed it.")
    );
}
