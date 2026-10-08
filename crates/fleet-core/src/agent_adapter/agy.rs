//! Agy (Google's Antigravity CLI) behind [`AgentAdapter`], redesign step 12.3.
//!
//! **Provisional.** No machine fleet runs on has `agy` installed, and it is
//! not on npm, so everything here is built from agy's documentation
//! (antigravity.google/docs/cli, its headless page and the published
//! walk-throughs), not from captured screens. The fixtures in the tests are
//! transcribed from those pages and marked as such; replace them with
//! `tmux capture-pane -p` output from a real agy before relying on the pane
//! reader to answer prompts.
//!
//! What the docs pin down:
//! - Launch: `agy` is the interactive TUI; `--conversation <id>` resumes a
//!   conversation, `-c` / `--continue` resumes the cwd's newest one and
//!   starts fresh when the workspace has none; `--model <slug>` and
//!   `--effort low|medium|high` set the session's model and reasoning.
//! - Conversation ids are UUID-shaped, and agy allocates them itself: there
//!   is no `--session-id` to start a fresh conversation under an id fleet
//!   chose.
//! - Conversations live in `~/.gemini/antigravity-cli/conversations/` as one
//!   SQLite database each, not as a text log; headless `--output-format
//!   stream-json` prints NDJSON `step_update` events.
//! - The tool approval dialog reads "Do you want to proceed?" over numbered
//!   choices (`1. Yes` … `4. No`), the cursor drawn as `>`.

use super::{AgentAdapter, LaunchSwitch, PickerOption, SlashCommand};
use crate::service::pane_intel::{self, PaneIntel};
use crate::service::transcript::{ConvItem, ConvTurn};
use crate::tmux::ClaudeLaunch;

/// The Agy adapter.
#[derive(Debug, Default, Clone, Copy)]
pub struct Agy;

const fn opt(value: &'static str, label: &'static str) -> PickerOption {
    PickerOption { value, label }
}

const fn cmd(name: &'static str, description: &'static str, args: bool) -> SlashCommand {
    SlashCommand {
        name,
        description,
        args,
    }
}

/// A hint list only: the slugs `agy models` printed in its documentation.
/// Which models an account may use depends on its tier, and `agy models`
/// on the host is the real list.
const MODELS: &[PickerOption] = &[
    opt("default", "Default"),
    opt("gemini-3.8-flash", "Gemini 3.8 Flash"),
    opt("gemini-3.8-flash-high", "Gemini 3.8 Flash (high)"),
    opt("gemini-3.8-flash-medium", "Gemini 3.8 Flash (medium)"),
    opt("gemini-3.8-flash-low", "Gemini 3.8 Flash (low)"),
];

/// The levels `agy --effort` takes.
const EFFORTS: &[PickerOption] = &[
    opt("low", "Low"),
    opt("medium", "Medium"),
    opt("high", "High"),
];

/// Agy's built-in commands, as its command reference lists them.
const SLASH: &[SlashCommand] = &[
    cmd(
        "clear",
        "Clear the context and start a new conversation",
        false,
    ),
    cmd("resume", "Switch to an earlier conversation", false),
    cmd("rewind", "Roll back to a previous checkpoint", false),
    cmd("fork", "Branch the conversation into a new one", false),
    cmd("rename", "Rename the current conversation", true),
    cmd("model", "Choose the default model", false),
    cmd("fast", "Toggle low-latency reasoning", false),
    cmd("planning", "Toggle multi-turn plan generation", false),
    cmd(
        "permissions",
        "Set how much agy may do without asking",
        false,
    ),
    cmd("config", "Open settings", false),
    cmd("diff", "Open the git diff viewer", false),
    cmd("btw", "Send a background note to steer the agent", true),
    cmd("context", "Show token usage and context files", false),
    cmd("tasks", "Monitor or stop background tasks", false),
    cmd("skills", "Browse local and global skills", false),
    cmd("mcp", "Manage MCP servers", false),
    cmd("agents", "View and approve subagent actions", false),
    cmd("hooks", "Browse active hooks", false),
    cmd("credits", "Show credit balance and quota", false),
    cmd("help", "List commands and shortcuts", false),
    cmd("exit", "Quit agy (the tmux session stays)", false),
];

impl Agy {
    /// ` --model 'm' --effort 'e'`, each only when set and, for the effort,
    /// only a level agy takes: a session stored with Claude's `xhigh` or
    /// `max` launches at agy's default rather than failing to start.
    fn flags(launch: &ClaudeLaunch) -> String {
        let mut out = String::new();
        if let Some(m) = launch.model.as_deref().filter(|m| *m != "default") {
            out.push_str(&format!(" --model {}", crate::shell::quote(m)));
        }
        if let Some(e) = launch
            .effort
            .as_deref()
            .filter(|e| EFFORTS.iter().any(|o| o.value == *e))
        {
            out.push_str(&format!(" --effort {}", crate::shell::quote(e)));
        }
        out
    }
}

impl AgentAdapter for Agy {
    fn id(&self) -> &'static str {
        crate::store::AGENT_AGY
    }

    fn label(&self) -> &'static str {
        "Agy"
    }

    /// With an id: resume it, else continue the cwd's newest conversation
    /// (which starts a fresh one when the workspace has none). Without one:
    /// continue. agy has no way to start a conversation under a given id,
    /// so a minted id that agy never saw falls through to `--continue`.
    /// `launch.profile` is a Claude credential profile and does not apply:
    /// agy keeps its login in `~/.gemini`. `tmux_name` is unused, as agy
    /// takes no session name.
    fn launch_command(
        &self,
        conversation_id: Option<&str>,
        _tmux_name: &str,
        launch: &ClaudeLaunch,
    ) -> String {
        let tail = "exec ${SHELL:-/bin/zsh} -l";
        let flags = Self::flags(launch);
        let path = crate::tmux::VOICE_PATH_PREFIX;
        match conversation_id {
            Some(id) => format!(
                "{path}agy --conversation {}{flags} 2>/dev/null || agy --continue{flags}; {tail}",
                crate::shell::quote(id)
            ),
            None => format!("{path}agy --continue{flags}; {tail}"),
        }
    }

    /// The docs show UUID-shaped ids only, so the same lowercase-UUID check
    /// as Claude's.
    fn valid_conversation_id(&self, id: &str) -> bool {
        crate::validate::claude_session_id(id).is_ok()
    }

    /// A placeholder agy has never seen: agy allocates its own ids, so the
    /// first launch under this falls through to `--continue`. Reading the
    /// real id back (agy's `cache/last_conversations.json` maps a workspace
    /// to its newest conversation) is left to the routing that stores it.
    fn mint_conversation_id(&self) -> String {
        uuid::Uuid::new_v4().to_string()
    }

    fn models(&self) -> &'static [PickerOption] {
        MODELS
    }

    fn effort_levels(&self) -> &'static [PickerOption] {
        EFFORTS
    }

    fn slash_commands(&self) -> &'static [SlashCommand] {
        SLASH
    }

    /// Never: agy's `/model` opens a picker and takes no documented
    /// argument, and its choice persists in agy's own settings, so there is
    /// nothing for fleet to record for a relaunch.
    fn launch_switch<'a>(&self, _prompt: &'a str) -> Option<LaunchSwitch<'a>> {
        None
    }

    /// Claude Code's reader over the pane with agy's `>` cursor redrawn as
    /// Claude's `❯`: the approval dialog ("Do you want to proceed?" over
    /// numbered choices) then reads as a permission dialog with its
    /// selected choice, and agy's `> ` input line as the live prompt that
    /// marks a dialog above it as scrollback. Working and idle cues are
    /// Claude's; agy's own are undocumented, and the reader returns no
    /// status rather than guess.
    fn analyze_pane(&self, pane_tail: &str) -> PaneIntel {
        pane_intel::analyze(&redraw_cursor(pane_tail))
    }

    fn spinner_line(&self, pane_tail: &str) -> Option<String> {
        pane_intel::spinner_line(pane_tail)
    }

    /// Turns from headless `--output-format stream-json` output: each
    /// `step_update` that reaches `DONE` closes a step, whose text is the
    /// `text_delta`s of its `ACTIVE` updates (or the `DONE` event's own
    /// `text_delta` when it carries one). A `user_input` step opens a turn,
    /// an `agent_response` adds text and a `tool` step a tool line. agy's
    /// interactive conversations are SQLite databases, not this; reading
    /// those needs a captured database and is not done here.
    fn parse_transcript(&self, transcript: &str) -> Vec<ConvTurn> {
        parse_stream_json(transcript)
    }
}

/// Every line whose first visible character is agy's `>` cursor or prompt
/// marker, with that `>` replaced by `❯` (box borders and indentation kept).
fn redraw_cursor(pane_tail: &str) -> String {
    let mut out = String::with_capacity(pane_tail.len() + 16);
    for line in pane_tail.split_inclusive('\n') {
        let lead = line.len()
            - line
                .trim_start_matches(|c: char| c.is_whitespace() || c == '│' || c == '║')
                .len();
        match line[lead..].strip_prefix('>') {
            Some(rest) if rest.is_empty() || rest.starts_with([' ', '\n']) => {
                out.push_str(&line[..lead]);
                out.push('❯');
                out.push_str(rest);
            }
            _ => out.push_str(line),
        }
    }
    out
}

fn parse_stream_json(ndjson: &str) -> Vec<ConvTurn> {
    use std::collections::HashMap;

    let mut turns: Vec<ConvTurn> = Vec::new();
    // Text so far of each step still ACTIVE, by step_index.
    let mut open: HashMap<i64, String> = HashMap::new();
    for line in ndjson.lines() {
        let Ok(ev) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if ev.get("event").and_then(|v| v.as_str()) != Some("step_update") {
            continue;
        }
        let Some(index) = ev.get("step_index").and_then(|v| v.as_i64()) else {
            continue;
        };
        let delta = ev.get("text_delta").and_then(|v| v.as_str()).unwrap_or("");
        let text = open.entry(index).or_default();
        text.push_str(delta);
        if ev.get("state").and_then(|v| v.as_str()) != Some("DONE") {
            continue;
        }
        let text = open.remove(&index).unwrap_or_default();
        let at = ev
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        match ev.get("step_type").and_then(|v| v.as_str()) {
            Some("user_input") => turns.push(ConvTurn {
                prompt: Some(text),
                at,
                ended_at: None,
                items: Vec::new(),
                reminders: Vec::new(),
                prompt_uuid: None,
                prompt_partial: false,
            }),
            Some("agent_response") if !text.is_empty() => {
                turn_for_reply(&mut turns, &at)
                    .items
                    .push(ConvItem::Text { text });
            }
            Some("tool") => {
                let name = ev
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool")
                    .to_string();
                turn_for_reply(&mut turns, &at).items.push(ConvItem::Tool {
                    summary: name.clone(),
                    error: false,
                    id: None,
                    name,
                    target: None,
                    at: at.clone(),
                    ended_at: at,
                    done: true,
                });
            }
            _ => {}
        }
    }
    turns
}

/// The turn a reply step belongs to: the last one, or a prompt-less turn
/// when the stream starts mid-reply. Stamps the turn's `ended_at`.
fn turn_for_reply<'a>(turns: &'a mut Vec<ConvTurn>, at: &Option<String>) -> &'a mut ConvTurn {
    if turns.is_empty() {
        turns.push(ConvTurn {
            prompt: None,
            at: at.clone(),
            ended_at: None,
            items: Vec::new(),
            reminders: Vec::new(),
            prompt_uuid: None,
            prompt_partial: false,
        });
    }
    let turn = turns.last_mut().expect("pushed above");
    if at.is_some() {
        turn.ended_at = at.clone();
    }
    turn
}
