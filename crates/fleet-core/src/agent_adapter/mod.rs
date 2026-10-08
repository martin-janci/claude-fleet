//! Agent adapters (Orbit Fleet redesign step 12.1): what fleet needs to
//! know about the agent CLI running in a session's pane, behind one trait,
//! so a second agent (Codex in 12.2, Agy in 12.3) is a new adapter rather
//! than a branch in every service.
//!
//! An adapter answers six questions: the pane's launch line (with resume),
//! the model and effort lists it offers, its slash commands (and which
//! sent lines switch the stored model or effort), how to read its pane
//! (status, stuck states, dialogs), and how to read its transcript.
//!
//! A session's agent is `sessions.agent` (migration 121): [`for_session`]
//! maps a `shell` session to no adapter (a shell has no agent) and any
//! other to its agent's, Claude Code for an agent with none. [`by_id`]
//! knows `claude`, `codex` ([`CodexCli`], 12.2) and `agy` ([`Agy`], 12.3,
//! built from agy's docs and provisional until real agy screens are
//! captured).
//!
//! Claude Code moved behind [`ClaudeCode`] with no behaviour change: it
//! delegates to the functions that did the work before (`tmux`'s launch
//! chain, `pane_intel`, `transcript`), and their own tests still pin them.

mod agy;
mod claude;
mod codex;

pub use agy::Agy;
pub use claude::ClaudeCode;
pub use codex::CodexCli;

use crate::service::pane_intel::PaneIntel;
use crate::service::transcript::ConvTurn;
use crate::tmux::ClaudeLaunch;
use serde::Serialize;

/// One entry of a model or effort picker: `value` is what the agent takes
/// (`--model <value>`, `/effort <value>`), `label` what a person reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PickerOption {
    pub value: &'static str,
    pub label: &'static str,
}

/// A built-in slash command the composer's menu offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SlashCommand {
    pub name: &'static str,
    pub description: &'static str,
    /// Completes with a trailing space so the user can type the argument.
    pub args: bool,
}

/// A sent line that switches the session's model or effort. `None` inside
/// means back to the host's default (`/model default`, `/effort auto`).
#[derive(Debug, PartialEq, Eq)]
pub enum LaunchSwitch<'a> {
    Model(Option<&'a str>),
    Effort(Option<&'a str>),
}

/// What fleet needs from one agent CLI. Implementations are stateless
/// statics ([`by_id`], [`for_kind`]).
pub trait AgentAdapter: Send + Sync {
    /// The `sessions.agent` value (`claude`).
    fn id(&self) -> &'static str;

    /// The name a person reads ("Claude Code").
    fn label(&self) -> &'static str;

    /// The pane command for a session named `tmux_name`. With a
    /// conversation id: resume it, else start it under that id (or fresh,
    /// for an agent that cannot take one). Without one: continue the newest
    /// conversation in the cwd, else start fresh.
    /// `conversation_id` must already have passed
    /// [`AgentAdapter::valid_conversation_id`]; `launch` is the session's
    /// model, effort and login profile.
    fn launch_command(
        &self,
        conversation_id: Option<&str>,
        tmux_name: &str,
        launch: &ClaudeLaunch,
    ) -> String;

    /// Whether `id` is safe and well-formed as a conversation id to resume.
    /// A stored id that is not degrades to "no id" before it reaches the
    /// shell.
    fn valid_conversation_id(&self, id: &str) -> bool;

    /// A fresh conversation id for a new session, so a later recreate
    /// resumes THIS conversation rather than the cwd's newest. `None` for
    /// an agent that picks its own id (Codex): its new session starts with
    /// [`AgentAdapter::start_command`] instead.
    fn mint_conversation_id(&self) -> Option<String>;

    /// The pane command for a brand-new conversation, never resuming one,
    /// for an agent whose [`AgentAdapter::mint_conversation_id`] is `None`.
    /// An agent that mints ids starts new sessions under one through
    /// [`AgentAdapter::launch_command`] and never calls this.
    fn start_command(&self, tmux_name: &str, launch: &ClaudeLaunch) -> String;

    /// The model aliases the model picker offers (a hint list; any value
    /// the agent takes is accepted).
    fn models(&self) -> &'static [PickerOption];

    /// The effort levels the agent takes at launch.
    fn effort_levels(&self) -> &'static [PickerOption];

    /// The built-in slash commands the composer's menu offers.
    fn slash_commands(&self) -> &'static [SlashCommand];

    /// A sent prompt that switches the model or effort, which the session
    /// then keeps for a later relaunch. `None` for anything else.
    fn launch_switch<'a>(&self, prompt: &'a str) -> Option<LaunchSwitch<'a>>;

    /// Read a captured pane tail: activity, stuck state, status, dialogs.
    fn analyze_pane(&self, pane_tail: &str) -> PaneIntel;

    /// The pane's live spinner line, verbatim, if one is showing.
    fn spinner_line(&self, pane_tail: &str) -> Option<String>;

    /// The conversation's turns from its transcript file's text.
    fn parse_transcript(&self, transcript: &str) -> Vec<ConvTurn>;
}

static CLAUDE_CODE: ClaudeCode = ClaudeCode;
static AGY: Agy = Agy;
static CODEX_CLI: CodexCli = CodexCli;

/// Claude Code's adapter.
pub fn claude() -> &'static dyn AgentAdapter {
    &CLAUDE_CODE
}

/// The adapter for a `sessions.agent` value, or `None` for `shell` (no
/// agent) and for an agent fleet has no adapter for yet.
pub fn by_id(agent: &str) -> Option<&'static dyn AgentAdapter> {
    match agent {
        crate::store::AGENT_CLAUDE => Some(claude()),
        crate::store::AGENT_CODEX => Some(&CODEX_CLI),
        crate::store::AGENT_AGY => Some(&AGY),
        _ => None,
    }
}

/// The adapter for a session of `kind` running `agent` (its row's
/// `agent`, or a new session's requested one): none for a `shell`
/// session, the agent's own when it has one, else Claude Code (the default
/// agent, and what every row ran before migration 121).
pub fn for_session(kind: &str, agent: Option<&str>) -> Option<&'static dyn AgentAdapter> {
    if kind == "shell" {
        return None;
    }
    Some(agent.and_then(by_id).unwrap_or_else(claude))
}

/// The adapter for a session of `kind` whose agent is not known: none for
/// a `shell` session, Claude Code for every other kind.
pub fn for_kind(kind: &str) -> Option<&'static dyn AgentAdapter> {
    if kind == "shell" {
        None
    } else {
        Some(claude())
    }
}

#[cfg(test)]
mod tests;
