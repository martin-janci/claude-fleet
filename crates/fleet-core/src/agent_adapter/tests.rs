use super::*;
use crate::tmux::{pane_command_with, ClaudeLaunch};

#[test]
fn claude_is_the_adapter_for_every_kind_but_shell() {
    for kind in ["work", "review", "bg", "external"] {
        assert_eq!(for_kind(kind).map(|a| a.id()), Some("claude"), "{kind}");
    }
    assert!(for_kind("shell").is_none());
    assert_eq!(by_id("claude").map(|a| a.label()), Some("Claude Code"));
    for no_adapter_yet in ["shell", "codex", "agy", "nope"] {
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
    let minted = a.mint_conversation_id();
    assert!(a.valid_conversation_id(&minted), "{minted}");
    assert_ne!(minted, a.mint_conversation_id());
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
