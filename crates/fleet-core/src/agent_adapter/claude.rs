//! Claude Code behind [`AgentAdapter`]. Every method delegates to the code
//! that did the job before the adapter existed, so behaviour is unchanged:
//! the launch chain is `tmux::pane_command_with`, the pane reader
//! `service::pane_intel`, the transcript reader `service::transcript`.
//! The model, effort and slash-command lists mirror the composer's
//! (`src/lib/conversation.ts`), which a test holds them to.

use super::{AgentAdapter, LaunchSwitch, PickerOption, SlashCommand};
use crate::service::pane_intel::{self, PaneIntel};
use crate::service::transcript::{self, ConvTurn};
use crate::tmux::ClaudeLaunch;

/// The Claude Code adapter.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClaudeCode;

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

/// Claude Code's `/model` aliases (`MODEL_OPTIONS` in conversation.ts).
const MODELS: &[PickerOption] = &[
    opt("default", "Default"),
    opt("best", "Best available"),
    opt("fable", "Fable"),
    opt("opus", "Opus"),
    opt("opus[1m]", "Opus (1M context)"),
    opt("sonnet", "Sonnet"),
    opt("sonnet[1m]", "Sonnet (1M context)"),
    opt("haiku", "Haiku"),
    opt("opusplan", "Opus plan / Sonnet"),
];

/// The levels `claude --effort` takes at launch (`LAUNCH_EFFORT_OPTIONS`
/// in conversation.ts; `validate::EFFORT_LEVELS` is the same values).
const EFFORTS: &[PickerOption] = &[
    opt("low", "Low"),
    opt("medium", "Medium"),
    opt("high", "High"),
    opt("xhigh", "Extra high"),
    opt("max", "Max"),
];

/// Claude Code's built-in commands (`SLASH_COMMANDS` in conversation.ts).
const SLASH: &[SlashCommand] = &[
    cmd("clear", "Clear the conversation and start fresh", false),
    cmd(
        "compact",
        "Summarise the context to free space (optional focus text)",
        true,
    ),
    cmd("context", "Show what is using the context window", false),
    cmd("cost", "Show token usage and cost for this session", false),
    cmd("usage", "Show plan usage and rate limits", false),
    cmd(
        "status",
        "Show version, model, account and working directory",
        false,
    ),
    cmd("model", "Switch the model", true),
    cmd("effort", "Set the reasoning effort level", true),
    cmd(
        "rc",
        "Remote Control: drive this session from claude.ai",
        false,
    ),
    cmd("resume", "Resume an earlier conversation", false),
    cmd(
        "rewind",
        "Rewind the conversation and files to a checkpoint",
        false,
    ),
    cmd("review", "Review the current changes", false),
    cmd("memory", "Edit the memory files loaded into context", false),
    cmd("config", "Open settings", false),
    cmd("permissions", "Manage tool permissions", false),
    cmd("mcp", "Manage MCP servers", false),
    cmd("agents", "Manage subagent definitions", false),
    cmd("hooks", "Manage hooks", false),
    cmd("doctor", "Check the installation", false),
    cmd("init", "Write a CLAUDE.md for this project", false),
    cmd("export", "Export the conversation to a file", false),
    cmd("help", "List commands and shortcuts", false),
    cmd("exit", "Quit Claude Code (the tmux session stays)", false),
];

impl AgentAdapter for ClaudeCode {
    fn id(&self) -> &'static str {
        crate::store::AGENT_CLAUDE
    }

    fn label(&self) -> &'static str {
        "Claude Code"
    }

    fn launch_command(
        &self,
        conversation_id: Option<&str>,
        tmux_name: &str,
        launch: &ClaudeLaunch,
    ) -> String {
        crate::tmux::pane_command_with(conversation_id, tmux_name, launch)
    }

    fn valid_conversation_id(&self, id: &str) -> bool {
        crate::validate::claude_session_id(id).is_ok()
    }

    fn mint_conversation_id(&self) -> Option<String> {
        Some(uuid::Uuid::new_v4().to_string())
    }

    /// Unused: Claude Code takes a minted id, so a new session starts under
    /// one. Starting bare is the no-id launch line.
    fn start_command(&self, tmux_name: &str, launch: &ClaudeLaunch) -> String {
        crate::tmux::pane_command_with(None, tmux_name, launch)
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

    /// `/model <x>` / `/effort <x>`: exactly the command and one valid
    /// argument (`validate::claude_model` / `effort_level`), so a bare
    /// `/model` (the REPL's picker), a typo or prose is never recorded.
    fn launch_switch<'a>(&self, prompt: &'a str) -> Option<LaunchSwitch<'a>> {
        let mut words = prompt.trim().split_ascii_whitespace();
        let (cmd, arg) = (words.next()?, words.next()?);
        if words.next().is_some() {
            return None;
        }
        match cmd {
            "/model" if arg == "default" => Some(LaunchSwitch::Model(None)),
            "/model" => crate::validate::claude_model(arg)
                .is_ok()
                .then_some(LaunchSwitch::Model(Some(arg))),
            "/effort" if arg == "auto" => Some(LaunchSwitch::Effort(None)),
            "/effort" => crate::validate::effort_level(arg)
                .is_ok()
                .then_some(LaunchSwitch::Effort(Some(arg))),
            _ => None,
        }
    }

    fn analyze_pane(&self, pane_tail: &str) -> PaneIntel {
        pane_intel::analyze(pane_tail)
    }

    fn spinner_line(&self, pane_tail: &str) -> Option<String> {
        pane_intel::spinner_line(pane_tail)
    }

    fn parse_transcript(&self, transcript: &str) -> Vec<ConvTurn> {
        transcript::parse_conversation(transcript)
    }
}
