//! OpenAI's Codex CLI (`codex`) behind [`AgentAdapter`] (redesign step 12.2).
//!
//! What differs from Claude Code, as captured from codex-cli 0.162.0 in a
//! tmux pane (the fixtures under `testdata/codex/`):
//!
//! * **Launch.** `codex resume <id>` resumes a conversation and exits
//!   non-zero when the id is unknown; `codex resume --last` continues the
//!   cwd's newest one and starts fresh when there is none. Codex cannot be
//!   told a conversation id up front, so a new session starts bare
//!   ([`AgentAdapter::start_command`]) and has no id until one is learned.
//!   Every launch runs with `--dangerously-bypass-approvals-and-sandbox`,
//!   the counterpart of the `--dangerously-skip-permissions` every Claude
//!   session runs with ([`crate::tmux::CL_FALLBACK`]). The model is `-m`,
//!   the effort `-c model_reasoning_effort="…"`. Codex has no login
//!   profiles, so a profile is refused at start (`normalize_launch`).
//! * **Pane.** The composer and its selected choice both start with `›`;
//!   a working turn shows `• Working (4s • esc to interrupt)` and a braille
//!   spinner at the end of the status line; an approval ends in "Press
//!   enter to confirm or esc to cancel", a question in "enter to submit
//!   answer". A digit key answers either at once, like Claude's dialogs, so
//!   the prompt card's keys work unchanged.
//! * **Transcript.** `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl`:
//!   `event_msg` / `item_completed` `UserMessage` opens a turn, and the
//!   `response_item` lines carry the assistant's messages and tool calls.

use super::{AgentAdapter, LaunchSwitch, PickerOption, SlashCommand};
use crate::service::context::ContextUsage;
use crate::service::pane_intel::{
    self, ClaudeStatus, PaneIntel, PendingInput, PendingOption, StuckKind, WaitingFor,
};
use crate::service::transcript::{ConvItem, ConvTurn, ToolDetail, TOOL_DETAIL_MAX_CHARS};
use crate::tmux::ClaudeLaunch;

/// The Codex CLI adapter.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodexCli;

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

/// The models Codex lists (`codex debug models`, visibility `list`), newest
/// first as its `/model` picker orders them.
const MODELS: &[PickerOption] = &[
    opt("gpt-6.1-sol", "GPT-6.1-Sol"),
    opt("gpt-6-astra", "GPT-6-Astra"),
    opt("gpt-6-sol", "GPT-6-Sol"),
    opt("gpt-6-luna", "GPT-6-Luna"),
    opt("gpt-5.6-sol", "GPT-5.6-Sol"),
    opt("gpt-5.6-terra", "GPT-5.6-Terra"),
    opt("gpt-5.6-luna", "GPT-5.6-Luna"),
    opt("gpt-5.5", "GPT-5.5"),
];

/// The reasoning levels Codex takes as `model_reasoning_effort`, limited to
/// the ones a session stores (`validate::EFFORT_LEVELS`): Codex's `ultra`
/// is left out until the stored effort is checked per agent.
const EFFORTS: &[PickerOption] = &[
    opt("low", "Low"),
    opt("medium", "Medium"),
    opt("high", "High"),
    opt("xhigh", "Extra high"),
    opt("max", "Max"),
];

/// Codex's built-in commands worth offering from the composer (its `/`
/// menu lists about fifty).
const SLASH: &[SlashCommand] = &[
    cmd("model", "Choose the model and reasoning effort", false),
    cmd("permissions", "Choose what Codex is allowed to do", false),
    cmd("new", "Start a new chat", false),
    cmd(
        "compact",
        "Summarise the conversation to free context",
        false,
    ),
    cmd(
        "status",
        "Show the session's configuration and token usage",
        false,
    ),
    cmd("diff", "Show the git diff, untracked files included", false),
    cmd("review", "Review the current changes", false),
    cmd("plan", "Switch to Plan mode", false),
    cmd("mention", "Mention a file", true),
    cmd("resume", "Resume a saved chat", false),
    cmd("fork", "Fork the current chat", false),
    cmd("rename", "Rename the current thread", true),
    cmd("init", "Write an AGENTS.md for this project", false),
    cmd("skills", "Use skills", false),
    cmd("mcp", "List MCP tools", false),
    cmd("hooks", "View and manage lifecycle hooks", false),
    cmd("memories", "Configure memory use and generation", false),
    cmd("copy", "Copy the last response", false),
    cmd("export", "Export the conversation as markdown", false),
    cmd("exit", "Quit Codex (the tmux session stays)", false),
];

/// The flag every `codex` in a launch line carries; see the module docs.
const YOLO: &str = "--dangerously-bypass-approvals-and-sandbox";

/// What a pane falls back to when Codex exits, as for Claude.
const TAIL: &str = "exec ${SHELL:-/bin/zsh} -l";

/// ` -m 'm' -c 'model_reasoning_effort="e"'`, each only when set.
fn flags(launch: &ClaudeLaunch) -> String {
    let mut out = String::new();
    if let Some(m) = launch.model.as_deref() {
        out.push_str(&format!(" -m {}", crate::shell::quote(m)));
    }
    if let Some(e) = launch.effort.as_deref() {
        let kv = format!("model_reasoning_effort=\"{e}\"");
        out.push_str(&format!(" -c {}", crate::shell::quote(&kv)));
    }
    out
}

impl AgentAdapter for CodexCli {
    fn id(&self) -> &'static str {
        crate::store::AGENT_CODEX
    }

    fn label(&self) -> &'static str {
        "Codex"
    }

    /// With an id: `codex resume <id>`, or a fresh conversation when Codex
    /// no longer has it. Without one: `codex resume --last`, which starts
    /// fresh by itself when the cwd has no conversation.
    fn launch_command(
        &self,
        conversation_id: Option<&str>,
        _tmux_name: &str,
        launch: &ClaudeLaunch,
    ) -> String {
        let f = flags(launch);
        match conversation_id {
            Some(id) => format!(
                "codex resume {} {YOLO}{f} 2>/dev/null || codex {YOLO}{f}; {TAIL}",
                crate::shell::quote(id)
            ),
            None => format!("codex resume --last {YOLO}{f} || codex {YOLO}{f}; {TAIL}"),
        }
    }

    fn start_command(&self, _tmux_name: &str, launch: &ClaudeLaunch) -> String {
        format!("codex {YOLO}{}; {TAIL}", flags(launch))
    }

    /// Codex's ids are UUIDs (v7) in the same lowercase shape as Claude's.
    fn valid_conversation_id(&self, id: &str) -> bool {
        crate::validate::claude_session_id(id).is_ok()
    }

    fn mint_conversation_id(&self) -> Option<String> {
        None
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

    /// Codex's `/model` takes no argument (it opens a picker), so no sent
    /// line names the model or effort a relaunch should keep.
    fn launch_switch<'a>(&self, _prompt: &'a str) -> Option<LaunchSwitch<'a>> {
        None
    }

    fn analyze_pane(&self, pane_tail: &str) -> PaneIntel {
        analyze(pane_tail)
    }

    fn spinner_line(&self, pane_tail: &str) -> Option<String> {
        let text = pane_intel::strip_ansi(pane_tail);
        text.lines().rev().find_map(|l| {
            let rest = l.trim().strip_prefix("• ")?;
            rest.contains("esc to interrupt")
                .then(|| rest.chars().take(pane_intel::ACTIVITY_MAX).collect())
        })
    }

    fn parse_transcript(&self, transcript: &str) -> Vec<ConvTurn> {
        parse_rollout(transcript)
    }

    fn context_usage(&self, transcript: &str) -> Option<ContextUsage> {
        context_from_rollout(transcript)
    }

    fn tool_detail(&self, lines: &str, id: &str) -> Option<ToolDetail> {
        tool_detail_from_rollout(lines, id)
    }
}

// ---------------------------------------------------------------- pane ----

/// The last lines a reading looks at: a dialog or the live footer is always
/// at the bottom of the pane, and older text above is scrollback.
const BOTTOM_LINES: usize = 40;

/// A line of the live composer or its footer: below anything else, it means
/// that thing is scrollback.
fn is_live(line: &str) -> bool {
    let l = line.trim();
    let lower = l.to_lowercase();
    lower.contains("? for shortcuts")
        || lower.contains("ask codex to do anything")
        || lower.contains("esc to interrupt") && l.starts_with("• ")
        || (l.starts_with('›') && pane_intel::parse_choice(l).is_none())
}

/// A status line ending in a braille spinner frame (`… · /repo · ⠦`). The
/// welcome logo is braille too, but has no letters.
fn is_spinner_footer(line: &str) -> bool {
    let l = line.trim_end();
    l.chars()
        .last()
        .is_some_and(|c| ('\u{2801}'..='\u{28ff}').contains(&c))
        && l.contains('·')
        && l.chars().any(|c| c.is_ascii_alphabetic())
}

fn stuck(lines: &[&str]) -> Option<StuckKind> {
    // The cue counts only when no live composer line is drawn under it:
    // a screen Codex already left stays in the scrollback.
    let on_screen = |cue: &dyn Fn(&str) -> bool| {
        let at = lines.iter().rposition(|l| cue(&l.to_lowercase()))?;
        (!lines[at + 1..].iter().any(|l| is_live(l))).then_some(())
    };
    if on_screen(&|l| {
        l.contains("sign in with chatgpt")
            || l.contains("sign in with device code")
            || l.contains("provide your own api key")
    })
    .is_some()
    {
        return Some(StuckKind::AuthMenu);
    }
    if on_screen(&|l| l.contains("trust this folder")).is_some() {
        return Some(StuckKind::TrustPrompt);
    }
    let text = lines.join("\n");
    match pane_intel::detect_stuck(&text) {
        // Claude's own screens; Codex draws its own above.
        Some(StuckKind::AuthMenu | StuckKind::TrustPrompt) | None => None,
        Some(k) => on_screen(&|l| match k {
            StuckKind::Reconnect => l.contains("reconnecting"),
            StuckKind::PressEnter => l.contains("press enter to"),
            _ => true,
        })
        .map(|()| k),
    }
}

struct Dialog {
    kind: WaitingFor,
    question: Option<String>,
    options: Vec<PendingOption>,
    detail: Option<String>,
}

/// `Yes, proceed (y)` → `Yes, proceed`: the key hint Codex draws after each
/// approval choice.
fn without_key_hint(label: &str) -> &str {
    match label.rfind(" (") {
        Some(at)
            if label.ends_with(')')
                && label[at + 2..label.len() - 1]
                    .chars()
                    .all(|c| c.is_ascii_lowercase())
                && label.len() - at <= 8 =>
        {
            label[..at].trim_end()
        }
        _ => label,
    }
}

/// `Postgres (Recommended)  Matches production.` → `Postgres (Recommended)`:
/// a question choice's description sits after a run of spaces.
fn without_description(label: &str) -> &str {
    label.split("  ").next().unwrap_or(label).trim()
}

/// Codex's approval and question dialogs, read off its footer.
///
/// A digit ANSWERS either one; no Enter follows. Checked against Codex's
/// own TUI (`codex-rs/tui/src/bottom_pane`, 2026-10): in the approval
/// overlay a digit goes through `ListSelectionView::select_shortcut`, which
/// accepts at once unless the item sets `require_explicit_confirmation` —
/// and no approval item does; in a `request_user_input` question a digit
/// selects, commits and moves to the next question or submits
/// (`go_next_or_submit`). The footers' "press enter to confirm" / "enter to
/// submit" describe the cursor path, not the digits.
fn dialog(lines: &[&str]) -> Option<Dialog> {
    let footer = lines.iter().rposition(|l| !l.trim().is_empty())?;
    let foot = lines[footer].to_lowercase();
    let kind = if foot.contains("press enter to confirm") {
        WaitingFor::Permission
    } else if foot.contains("enter to submit") {
        WaitingFor::Input
    } else {
        return None;
    };
    // The choices: the run of numbered lines just above the footer.
    let mut options = Vec::new();
    let mut top = footer;
    for i in (0..footer).rev() {
        let l = lines[i].trim();
        if l.is_empty() {
            if options.is_empty() {
                continue;
            }
            break;
        }
        match pane_intel::parse_choice(l) {
            Some((n, label, selected)) => {
                let label = match kind {
                    WaitingFor::Permission => without_key_hint(label),
                    WaitingFor::Input => without_description(label),
                };
                options.push(PendingOption {
                    n,
                    label: label.chars().take(pane_intel::PENDING_LABEL_MAX).collect(),
                    selected,
                    checked: false,
                });
                top = i;
            }
            None => break,
        }
    }
    if options.is_empty() {
        return None;
    }
    options.reverse();
    options.truncate(pane_intel::PENDING_OPTIONS_MAX);
    // Above the choices, up to the line that opened the turn's last item.
    let above: Vec<&str> = lines[..top]
        .iter()
        .rev()
        .map(|l| l.trim())
        .take_while(|l| !l.starts_with('•') && !l.starts_with('›'))
        .filter(|l| !l.is_empty())
        .collect();
    let (question, detail) = match kind {
        WaitingFor::Permission => (
            above
                .iter()
                .find(|l| {
                    l.starts_with("Would you like to")
                        || l.starts_with("Do you want to")
                        // An MCP server's elicitation: "<server> needs your approval."
                        || l.ends_with("needs your approval.")
                })
                .map(|l| l.to_string()),
            above
                .iter()
                .find_map(|l| l.strip_prefix("$ "))
                .map(str::to_string),
        ),
        WaitingFor::Input => (
            above
                .iter()
                .find(|l| !l.starts_with("Question "))
                .map(|l| l.to_string()),
            None,
        ),
    };
    let cap = |s: String, n: usize| s.chars().take(n).collect::<String>();
    Some(Dialog {
        kind,
        question: question.map(|q| cap(q, pane_intel::PENDING_QUESTION_MAX)),
        options,
        detail: detail.map(|d| cap(d, pane_intel::PENDING_DETAIL_MAX)),
    })
}

/// The last line saying what happened: Codex's `•` / `✔` / `✗` / `■`
/// item lines, skipping the composer, the footer and the "Worked for" stamp.
fn last_activity(lines: &[&str]) -> Option<String> {
    lines.iter().rev().find_map(|l| {
        let l = l.trim();
        let item = l.starts_with(['•', '✔', '✗', '■']);
        item.then(|| l.chars().take(pane_intel::ACTIVITY_MAX).collect())
    })
}

/// `42% context left` in the status line → 58 (percent used).
fn context_pct(lines: &[&str]) -> Option<f64> {
    lines.iter().rev().find_map(|l| {
        let at = l.find("% context left")?;
        let digits: String = l[..at]
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let left: f64 = digits.parse().ok()?;
        (left <= 100.0).then_some(100.0 - left)
    })
}

/// Read a Codex pane tail; the fields mean what [`pane_intel::analyze`]'s do.
fn analyze(pane_tail: &str) -> PaneIntel {
    let text = pane_intel::strip_ansi(pane_tail);
    let all: Vec<&str> = text.lines().map(str::trim_end).collect();
    let lines = &all[all.len().saturating_sub(BOTTOM_LINES)..];
    let stuck = stuck(lines);
    let dialog = if stuck.is_none() { dialog(lines) } else { None };
    let derived_status = if stuck.is_some() || dialog.is_some() {
        Some(ClaudeStatus::Blocked)
    } else if lines.iter().any(|l| {
        let l = l.trim();
        (l.starts_with("• ") && l.contains("esc to interrupt")) || is_spinner_footer(l)
    }) {
        Some(ClaudeStatus::Working)
    } else if lines.iter().any(|l| is_live(l)) {
        Some(ClaudeStatus::Idle)
    } else {
        None
    };
    let activity = match &dialog {
        Some(d) => {
            let s = match &d.question {
                Some(q) => format!("waiting for {}: {q}", d.kind.as_str()),
                None => format!("waiting for {}", d.kind.as_str()),
            };
            Some(s.chars().take(pane_intel::ACTIVITY_MAX).collect())
        }
        None => last_activity(lines),
    };
    PaneIntel {
        activity,
        stuck,
        context_pct: context_pct(lines),
        derived_status,
        waiting_for: dialog.as_ref().map(|d| d.kind),
        pending_input: dialog.map(|d| PendingInput {
            kind: d.kind.as_str().into(),
            question: d.question,
            options: d.options,
            multi: false,
            detail: d.detail,
        }),
    }
}

// ---------------------------------------------------------- transcript ----

/// Cap on a tool call's one-line summary, as for Claude's.
const TOOL_SUMMARY_CHARS: usize = 150;

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cap(s: String, max: usize) -> String {
    if s.chars().count() > max {
        s.chars().take(max).collect::<String>() + "…"
    } else {
        s
    }
}

/// The text of a message's content blocks (`input_text`, `output_text`,
/// `text`), joined.
fn content_text(content: Option<&serde_json::Value>) -> String {
    content
        .and_then(|c| c.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// What a tool call touched: the command it ran, the file, the question.
fn tool_target(args: &serde_json::Value) -> Option<String> {
    if let Some(cmd) = args.get("cmd").and_then(|v| v.as_str()) {
        return Some(cmd.to_string());
    }
    if let Some(argv) = args.get("command").and_then(|v| v.as_array()) {
        let argv: Vec<&str> = argv.iter().filter_map(|a| a.as_str()).collect();
        // `["bash", "-lc", "git status"]` ran `git status`.
        return Some(match argv.as_slice() {
            [_, flag, script] if flag.starts_with('-') && flag.ends_with('c') => script.to_string(),
            _ => argv.join(" "),
        });
    }
    for key in ["path", "file_path", "query", "url"] {
        if let Some(s) = args.get(key).and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
    }
    args.pointer("/questions/0/question")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn tool_item(
    name: &str,
    args: &serde_json::Value,
    id: Option<String>,
    at: Option<String>,
) -> ConvItem {
    let target = tool_target(args).map(|t| one_line(t.lines().next().unwrap_or("")));
    let inner = match &target {
        Some(t) => t.clone(),
        None => one_line(&args.to_string()),
    };
    ConvItem::Tool {
        summary: cap(format!("{name}({inner})"), TOOL_SUMMARY_CHARS),
        error: false,
        id,
        name: name.to_string(),
        target: target.map(|t| cap(t, 120)),
        at,
        ended_at: None,
        done: false,
    }
}

/// A tool's output reads as a failure: a non-zero exit, or a refusal.
fn output_failed(output: &str) -> bool {
    let code = output
        .lines()
        .find_map(|l| l.trim().strip_prefix("Process exited with code "))
        .and_then(|c| c.trim().parse::<i64>().ok());
    matches!(code, Some(c) if c != 0) || output.contains("aborted by user")
}

fn new_turn(prompt: Option<String>, at: Option<String>, uuid: Option<String>) -> ConvTurn {
    ConvTurn {
        prompt,
        at,
        ended_at: None,
        items: Vec::new(),
        reminders: Vec::new(),
        prompt_uuid: uuid,
        prompt_partial: false,
    }
}

/// A tool call's arguments: `function_call` carries them as a JSON string,
/// `custom_tool_call` as its raw `input` (an `apply_patch` body).
fn call_args(p: &serde_json::Value) -> serde_json::Value {
    match p.get("arguments").and_then(|v| v.as_str()) {
        Some(raw) => {
            serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_string()))
        }
        None => p.get("input").cloned().unwrap_or(serde_json::Value::Null),
    }
}

/// A tool call's output text.
fn call_output(p: &serde_json::Value) -> String {
    match p.get("output") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// The conversation's turns from a Codex rollout file (or its tail: a line
/// that is not a whole JSON object is skipped).
fn parse_rollout(text: &str) -> Vec<ConvTurn> {
    let mut turns: Vec<ConvTurn> = Vec::new();
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let at = entry
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let Some(p) = entry.get("payload") else {
            continue;
        };
        let ptype = p.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match entry.get("type").and_then(|v| v.as_str()) {
            Some("event_msg") => {
                // The person's own words: `item_completed` `UserMessage`
                // (0.16x), `user_message` before it. The `response_item`
                // copy of a prompt is not used, because the harness's own
                // `<environment_context>` messages share its shape.
                let opened = match ptype {
                    "item_completed"
                        if p.pointer("/item/type").and_then(|v| v.as_str())
                            == Some("UserMessage") =>
                    {
                        let item = &p["item"];
                        Some((
                            content_text(item.get("content")),
                            item.get("id").and_then(|v| v.as_str()).map(str::to_string),
                        ))
                    }
                    "user_message" => Some((
                        p.get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        None,
                    )),
                    _ => None,
                };
                if let Some((prompt, uuid)) = opened {
                    turns.push(new_turn(Some(prompt), at, uuid));
                }
            }
            Some("response_item") => {
                let item = match ptype {
                    "message" if p.get("role").and_then(|v| v.as_str()) == Some("assistant") => {
                        let text = content_text(p.get("content"));
                        (!text.trim().is_empty()).then_some(ConvItem::Text { text })
                    }
                    "function_call" | "custom_tool_call" => {
                        let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                        let args = call_args(p);
                        let id = p
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        Some(tool_item(name, &args, id, at.clone()))
                    }
                    "function_call_output" | "custom_tool_call_output" => {
                        let call = p.get("call_id").and_then(|v| v.as_str());
                        let output = call_output(p);
                        finish_tool(&mut turns, call, &output, at.clone());
                        None
                    }
                    _ => None,
                };
                if let Some(item) = item {
                    if turns.is_empty() {
                        // Output whose prompt lies before the read tail.
                        turns.push(new_turn(None, at.clone(), None));
                    }
                    let turn = turns.last_mut().expect("a turn was just ensured");
                    turn.items.push(item);
                    turn.ended_at = at;
                }
            }
            _ => {}
        }
    }
    turns
}

fn finish_tool(turns: &mut [ConvTurn], call: Option<&str>, output: &str, at: Option<String>) {
    let Some(call) = call else { return };
    for turn in turns.iter_mut().rev() {
        for item in turn.items.iter_mut().rev() {
            if let ConvItem::Tool {
                id,
                done,
                error,
                ended_at,
                ..
            } = item
            {
                if id.as_deref() == Some(call) {
                    *done = true;
                    *error = output_failed(output);
                    *ended_at = at.clone();
                    turn.ended_at = at;
                    return;
                }
            }
        }
    }
}

/// Tool call `id`'s input and output from rollout lines (the whole file or
/// the lines mentioning the id). `None` when no call has that id.
fn tool_detail_from_rollout(lines: &str, id: &str) -> Option<ToolDetail> {
    let mut detail: Option<ToolDetail> = None;
    let mut output: Option<String> = None;
    for line in lines.lines() {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if entry.get("type").and_then(|v| v.as_str()) != Some("response_item") {
            continue;
        }
        let p = &entry["payload"];
        if p.get("call_id").and_then(|v| v.as_str()) != Some(id) {
            continue;
        }
        match p.get("type").and_then(|v| v.as_str()) {
            Some("function_call" | "custom_tool_call") if detail.is_none() => {
                let args = call_args(p);
                let input = match &args {
                    serde_json::Value::String(raw) => raw.clone(),
                    other => serde_json::to_string_pretty(other).unwrap_or_default(),
                };
                // A shell call's command, as Claude's `Bash` shows it.
                let command = (args.get("cmd").is_some() || args.get("command").is_some())
                    .then(|| tool_target(&args))
                    .flatten();
                detail = Some(ToolDetail {
                    id: id.to_string(),
                    name: p
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("tool")
                        .to_string(),
                    input: cap(input, TOOL_DETAIL_MAX_CHARS),
                    edit: None,
                    command: command.map(|c| cap(c, TOOL_DETAIL_MAX_CHARS)),
                    result: None,
                    is_error: false,
                });
            }
            // The first output after the call is its own.
            Some("function_call_output" | "custom_tool_call_output")
                if detail.is_some() && output.is_none() =>
            {
                output = Some(call_output(p));
            }
            _ => {}
        }
    }
    let mut d = detail?;
    if let Some(out) = output {
        d.is_error = output_failed(&out);
        d.result = Some(cap(out, TOOL_DETAIL_MAX_CHARS));
    }
    Some(d)
}

/// The context the conversation used on its last turn: Codex's own
/// `token_count` event, whose last usage is what the next request sends
/// and whose window is the model's.
fn context_from_rollout(text: &str) -> Option<ContextUsage> {
    let mut last: Option<ContextUsage> = None;
    let mut model: Option<String> = None;
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let p = &entry["payload"];
        match (
            entry.get("type").and_then(|v| v.as_str()),
            p.get("type").and_then(|v| v.as_str()),
        ) {
            (Some("turn_context"), _) => {
                model = p.get("model").and_then(|v| v.as_str()).map(str::to_string);
            }
            (Some("event_msg"), Some("token_count")) => {
                let info = &p["info"];
                let tokens = info
                    .pointer("/last_token_usage/total_tokens")
                    .and_then(|v| v.as_i64());
                let window = info.get("model_context_window").and_then(|v| v.as_i64());
                if let (Some(tokens), Some(window)) = (tokens, window) {
                    if window > 0 {
                        last = Some(ContextUsage {
                            tokens,
                            window,
                            model: model.clone(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    last
}

// ------------------------------------------------- conversation lookup ----

/// Rollout files a lookup reads the first line of, newest first.
const ROLLOUT_SCAN: usize = 200;

/// The rollout a Codex pane is writing, found on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PaneRollout {
    /// The conversation id, from the file name.
    pub id: String,
    pub path: String,
}

/// The script that finds, for each tmux session in `panes`, the newest
/// rollout under `${CODEX_HOME:-~/.codex}/sessions` whose `session_meta`
/// names the pane's cwd: what `codex resume --last` would resume there.
/// Codex allots a conversation's id itself and writes its rollout on the
/// first prompt, so this is how fleet learns the id. Prints
/// `cdx<TAB>name<TAB>path` per pane found; `None` for no panes.
pub(crate) fn rollouts_script(panes: &[&str]) -> Option<String> {
    if panes.is_empty() {
        return None;
    }
    let mut s = format!(
        r#"set +e
d="${{CODEX_HOME:-$HOME/.codex}}/sessions"
[ -d "$d" ] || exit 0
files=$(ls -1t "$d"/*/*/*/rollout-*.jsonl 2>/dev/null | head -n {ROLLOUT_SCAN})
[ -n "$files" ] || exit 0
"#
    );
    for name in panes {
        s.push_str(&format!(
            r#"c=$(tmux display-message -p -t {target} '#{{pane_current_path}}' 2>/dev/null)
if [ -n "$c" ]; then
  printf '%s\n' "$files" | while IFS= read -r f; do
    if head -n 1 "$f" | grep -qF "\"cwd\":\"$c\""; then printf 'cdx\t%s\t%s\n' {name} "$f"; break; fi
  done
fi
"#,
            target = crate::shell::quote(&crate::tmux::exact_pane(name)),
            name = crate::shell::quote(name),
        ));
    }
    Some(s)
}

/// [`rollouts_script`]'s answer by tmux session. A line whose file name
/// carries no valid id is skipped, and so is a rollout found for more than
/// one pane: two Codex panes in one worktree share a cwd, so the cwd match
/// cannot tell whose conversation it is, and each pane keeps the id it had.
pub(crate) fn parse_rollouts(out: &str) -> std::collections::HashMap<String, PaneRollout> {
    let mut found: std::collections::HashMap<String, PaneRollout> = parse_rollout_lines(out);
    let mut claims: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for r in found.values() {
        *claims.entry(r.id.clone()).or_default() += 1;
    }
    found.retain(|_, r| claims[&r.id] == 1);
    found
}

fn parse_rollout_lines(out: &str) -> std::collections::HashMap<String, PaneRollout> {
    out.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            if parts.next()? != "cdx" {
                return None;
            }
            let name = parts.next()?;
            let path = parts.next()?;
            let stem = path.rsplit('/').next()?.strip_suffix(".jsonl")?;
            let id = stem.get(stem.len().checked_sub(36)?..)?;
            crate::validate::claude_session_id(id).ok()?;
            Some((
                name.to_string(),
                PaneRollout {
                    id: id.to_string(),
                    path: path.to_string(),
                },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests;
