//! Read a session's Claude Code JSONL transcript and render the last
//! assistant turn(s) as plain text (MCP-4), or as structured turns for the
//! app's Conversation tab (`session_conversation`).
//!
//! Claude Code writes one transcript per session at
//! `~/.claude/projects/<encoded cwd>/<claude_session_id>.jsonl`, where the
//! cwd is encoded by replacing every character outside `[A-Za-z0-9]` with
//! `-` (verified against a live install: `/mnt/sda4/projects/github.com/x/y/
//! .worktrees/z` → `-mnt-sda4-projects-github-com-x-y--worktrees-z`).
//!
//! The file is read over ssh (or a local `bash`) with every interpolated
//! value shell-quoted, bounded by the ssh wall clock and a byte cap; the
//! parse then keeps only `assistant` entries, splitting turns on human
//! `user` prompts. Tool calls become one summary line each so a caller gets
//! the reply text without the tool-result noise (and without the pane's
//! TUI chrome that `capture_session` returns).

use crate::ipc_error::lock;
use crate::ipc_error::{codes, IpcError};
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::{SessionEvent, SessionRow, Store};
use serde::Serialize;
use std::sync::{Arc, Mutex};

/// Default / hard cap on the characters returned by `session_transcript`.
pub const DEFAULT_MAX_CHARS: usize = 8_000;
pub const MAX_MAX_CHARS: usize = 64_000;

/// Bytes of the JSONL tail read for a given character budget. Assistant
/// entries carry thinking signatures and usage blobs, so a turn's text is a
/// small fraction of its on-disk size.
const MIN_READ_BYTES: usize = 256 * 1024;
const MAX_READ_BYTES: usize = 4 * 1024 * 1024;

/// Wall-clock bound for the remote read. Reading a few MB over an
/// established ControlMaster is sub-second; this only bounds a wedged host.
const READ_WALL_CLOCK: std::time::Duration = std::time::Duration::from_secs(20);

/// Cap on a single tool_use summary line.
const TOOL_SUMMARY_CHARS: usize = 160;

/// Chars kept of a tool target (plus "…").
const TOOL_TARGET_MAX_CHARS: usize = 120;
/// Chars kept of a subagent's final text.
const SUBAGENT_RESULT_MAX_CHARS: usize = 1_500;
/// `tool_use` names that get their own [`ConvItem::Subagent`] item instead
/// of a plain [`ConvItem::Tool`].
const SUBAGENT_TOOLS: [&str; 2] = ["Task", "Agent"];

/// Encode a working directory the way Claude Code names its per-project
/// transcript directory: every char outside `[A-Za-z0-9]` becomes `-`.
/// The read script does this on the host (after `pwd -P`) with an
/// equivalent `sed`; this is the tested reference for that rule.
#[cfg(test)]
pub fn encode_project_dir(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Bytes to read for `max_chars` of rendered text.
pub fn read_bytes_for(max_chars: usize) -> usize {
    (max_chars.saturating_mul(64)).clamp(MIN_READ_BYTES, MAX_READ_BYTES)
}

/// Sentinel the read script prints (on stderr) so the caller can map a
/// missing transcript to a code instead of parsing prose.
const NO_TRANSCRIPT: &str = "__CF_NO_TRANSCRIPT__";

/// The bash script that prints the last `max_bytes` of the transcript.
///
/// Resolution order, all on the host:
/// 1. `stored_path` — the `transcript_path` Claude Code reported in a hook
///    (exact; immune to symlinks and name truncation);
/// 2. the cwd (the pane's `#{pane_current_path}`, else `fallback_dir`),
///    resolved to its PHYSICAL path with `cd -- "$p" && pwd -P` (a checkout
///    reached through a symlink, e.g. `~/projects → /mnt/sda4/projects`, is
///    recorded by Claude under its physical path), then encoded with the
///    same rule as [`encode_project_dir`];
/// 3. `~/.claude/projects/*/<id>.jsonl` — session ids are UUIDs, unique
///    across projects, so this finds the file when the cwd is unknown (dead
///    pane) or when Claude truncated an encoded directory name longer than
///    200 chars and added a hash suffix we cannot reproduce.
///
/// Every interpolated value is shell-quoted; the session id is validated by
/// the caller (`validate::claude_session_id`).
pub fn read_script(
    tmux_name: Option<&str>,
    stored_path: Option<&str>,
    fallback_dir: Option<&str>,
    claude_session_id: &str,
    max_bytes: usize,
) -> String {
    let tmux_q = quote(tmux_name.unwrap_or(""));
    // Exact pane target: a bare name would let tmux prefix-match another session.
    let tmux_target_q = quote(&crate::tmux::exact_pane(tmux_name.unwrap_or("")));
    let stored_q = quote(stored_path.unwrap_or(""));
    let fallback_q = quote(fallback_dir.unwrap_or(""));
    let id_q = quote(claude_session_id);
    format!(
        r#"set +e
id={id_q}
f=''
sp={stored_q}
if [ -n "$sp" ] && [ -f "$sp" ]; then f="$sp"; fi
if [ -z "$f" ]; then
  cwd=''
  if [ -n {tmux_q} ]; then
    cwd=$(tmux display-message -p -t {tmux_target_q} '#{{pane_current_path}}' 2>/dev/null)
  fi
  if [ -z "$cwd" ]; then cwd={fallback_q}; fi
  if [ -n "$cwd" ]; then
    phys=$(cd -- "$cwd" 2>/dev/null && pwd -P)
    if [ -n "$phys" ]; then cwd="$phys"; fi
    enc=$(printf '%s' "$cwd" | sed 's/[^A-Za-z0-9]/-/g')
    if [ -f "$HOME/.claude/projects/$enc/$id.jsonl" ]; then f="$HOME/.claude/projects/$enc/$id.jsonl"; fi
  fi
fi
if [ -z "$f" ]; then
  for c in "$HOME"/.claude/projects/*/"$id".jsonl; do
    if [ -f "$c" ]; then f="$c"; break; fi
  done
fi
if [ -z "$f" ]; then printf '{NO_TRANSCRIPT} %s\n' "$id" >&2; exit 4; fi
tail -c {max_bytes} "$f"
"#
    )
}

/// Prefix of a tool call's line in the plain-text rendering.
const TOOL_USE_PREFIX: &str = "[tool_use] ";

/// One-line summary of a `tool_use` block: `Name(input…)`, capped so the
/// prefixed text line stays within [`TOOL_SUMMARY_CHARS`] (+ `"…)"`).
fn tool_summary(block: &serde_json::Value) -> String {
    let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
    let input = block
        .get("input")
        .map(|v| match v {
            serde_json::Value::Object(map) => {
                // Prefer the fields that identify what the tool touched.
                let mut parts: Vec<String> = Vec::new();
                for key in [
                    "command",
                    "file_path",
                    "path",
                    "pattern",
                    "query",
                    "description",
                    "skill",
                    "prompt",
                ] {
                    if let Some(s) = map.get(key).and_then(|x| x.as_str()) {
                        parts.push(format!("{key}={}", one_line(s)));
                        break;
                    }
                }
                if parts.is_empty() {
                    one_line(&v.to_string())
                } else {
                    parts.join(" ")
                }
            }
            other => one_line(&other.to_string()),
        })
        .unwrap_or_default();
    let s = format!("{name}({input})");
    let cap = TOOL_SUMMARY_CHARS - TOOL_USE_PREFIX.chars().count();
    if s.chars().count() > cap {
        s.chars().take(cap).collect::<String>() + "…)"
    } else {
        s
    }
}

/// The plain-text line for a tool call: `[tool_use] Name(input…)`.
#[cfg(test)]
fn summarize_tool_use(block: &serde_json::Value) -> String {
    format!("{TOOL_USE_PREFIX}{}", tool_summary(block))
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a tool call touched, for its compact line: a path, a pattern, a
/// URL, a query or the first line of a command. `None` when nothing
/// identifying is in the input.
pub fn tool_target(_name: &str, input: Option<&serde_json::Value>) -> Option<String> {
    let map = input?.as_object()?;
    for key in [
        "file_path",
        "notebook_path",
        "pattern",
        "url",
        "query",
        "command",
        "path",
        "skill",
        "description",
    ] {
        if let Some(s) = map.get(key).and_then(|v| v.as_str()) {
            let first = s.lines().next().unwrap_or("").trim();
            if first.is_empty() {
                continue;
            }
            return Some(cap_chars(first, TOOL_TARGET_MAX_CHARS));
        }
    }
    None
}

/// The text of a tool_result's content (a string, or its text blocks
/// joined); images are skipped.
fn tool_result_text(block: &serde_json::Value) -> Option<String> {
    match block.get("content") {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(parts)) => {
            let texts: Vec<&str> = parts
                .iter()
                .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect();
            (!texts.is_empty()).then(|| texts.join("\n"))
        }
        _ => None,
    }
}

/// Turns kept by `session_conversation`, and its character budget.
pub const CONV_TURNS: usize = 10;
/// Most turns the Conversation tab may ask for with "Load older".
pub const CONV_MAX_TURNS: usize = 100;
/// Char budget ceiling when more turns are requested (the read itself is
/// capped at `MAX_READ_BYTES`).
pub const CONV_MAX_CHARS_CEILING: usize = 512_000;

/// PURE: the (turns, max_chars) budget for a Conversation read. `None`
/// means the default window; a request is clamped to `1..=CONV_MAX_TURNS`
/// and the char budget grows with it so extra turns are not immediately
/// trimmed away again.
pub fn conv_limits(turns: Option<usize>) -> (usize, usize) {
    let turns = turns.unwrap_or(CONV_TURNS).clamp(1, CONV_MAX_TURNS);
    let chars = (CONV_MAX_CHARS.saturating_mul(turns) / CONV_TURNS)
        .clamp(CONV_MAX_CHARS, CONV_MAX_CHARS_CEILING);
    (turns, chars)
}
pub const CONV_MAX_CHARS: usize = 64_000;
/// Bytes of JSONL tail `session_conversation` reads per fetch. Fixed (not
/// [`read_bytes_for`]`(CONV_MAX_CHARS)`, a 4 MB tail): the Conversation panel
/// polls every 5 s, over ssh for remote hosts. `session_transcript` keeps its
/// char-derived budget.
pub const CONV_READ_BYTES: usize = 1_048_576;

/// One turn of a conversation: the human prompt that opened it and what the
/// assistant said / did in reply.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ConvTurn {
    /// `None` for assistant output whose prompt lies before the read tail.
    pub prompt: Option<String>,
    /// ISO timestamp of the prompt entry (else of the first assistant entry).
    pub at: Option<String>,
    /// ISO timestamp of the turn's latest assistant entry: with `at`, how
    /// long the reply took so far. `None` for a turn with no assistant entry.
    pub ended_at: Option<String>,
    pub items: Vec<ConvItem>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConvItem {
    Text {
        text: String,
    },
    /// The tool one-liner, without the `[tool_use] ` prefix. `error` is set
    /// when the matching `tool_result` came back with `is_error: true`.
    Tool {
        summary: String,
        #[serde(default)]
        error: bool,
        /// The `tool_use` id, so a lazy detail fetch can find it again.
        id: Option<String>,
        /// The tool's name: `"Bash"`, `"Edit"`, `"mcp__x__y"`, …
        name: String,
        /// What the tool touched; see [`tool_target`].
        target: Option<String>,
        /// ISO timestamp of the `tool_use` entry.
        at: Option<String>,
        /// ISO timestamp of its `tool_result` entry.
        ended_at: Option<String>,
        /// A `tool_result` was seen for this call.
        done: bool,
    },
    /// A `Task` / `Agent` call, kept apart from other tools so its final
    /// text can be shown without cramming a subagent transcript into the
    /// tool one-liner.
    Subagent {
        /// The `tool_use` id.
        id: Option<String>,
        /// `"Task"` or `"Agent"`.
        name: String,
        /// `input.subagent_type`.
        agent_type: Option<String>,
        /// `input.description`.
        description: Option<String>,
        /// The final text of its `tool_result`, capped at
        /// [`SUBAGENT_RESULT_MAX_CHARS`].
        result: Option<String>,
        error: bool,
        /// ISO timestamp of the `tool_use` entry.
        at: Option<String>,
        /// ISO timestamp of its `tool_result` entry.
        ended_at: Option<String>,
        /// A `tool_result` was seen for this call.
        done: bool,
    },
    /// A compaction (`system/compact_boundary`). `summary` is the text of
    /// the following `isCompactSummary` user entry when the read tail has
    /// it.
    Compact {
        trigger: Option<String>,
        pre_tokens: Option<i64>,
        summary: Option<String>,
    },
    /// A slash command the user ran (`<command-name>` user entry); `output`
    /// is the following `<local-command-stdout>` / `<local-command-stderr>`.
    Command {
        name: String,
        args: Option<String>,
        output: Option<String>,
    },
    /// `[Request interrupted by user]` (`during_tool`: "… for tool use").
    Interrupt {
        during_tool: bool,
    },
}

/// Cap on a compaction's carried summary text (chars).
const COMPACT_SUMMARY_MAX_CHARS: usize = 20_000;
/// Cap on a slash command's carried output text (chars).
const COMMAND_OUTPUT_MAX_CHARS: usize = 4_000;

/// Text between `<tag>` and `</tag>`, trimmed; `None` when absent or empty.
fn tag_text(s: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = s.find(&open)? + open.len();
    let end = s[start..].find(&close)? + start;
    let t = s[start..end].trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// The plain text of a user entry's content (string body, or its text
/// blocks joined), without deciding whether it is a prompt.
fn user_text(content: Option<&serde_json::Value>) -> Option<String> {
    match content {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(blocks)) => {
            if blocks
                .iter()
                .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
            {
                return None;
            }
            let texts: Vec<&str> = blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect();
            (!texts.is_empty()).then(|| texts.join("\n"))
        }
        _ => None,
    }
}

fn cap_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Conversation {
    pub turns: Vec<ConvTurn>,
    /// Older turns or items were dropped to fit the turn / char budget.
    pub truncated: bool,
    /// Current-conversation context size, from this same read's tail.
    /// `None` when the tail carried no usage (nothing yet, or a compaction
    /// with no reply since).
    pub context: Option<ContextView>,
    /// This conversation's timeline events, oldest first. Empty wherever a
    /// `Conversation` is built without a store (e.g. `trim_conversation`
    /// alone); [`fetch_conversation_for_row`] fills it in.
    pub events: Vec<SessionEvent>,
}

/// The context size shown in the Conversation payload (spec §1.5), derived
/// from a [`crate::service::context::ContextUsage`] read at the same time as
/// the transcript tail.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ContextView {
    pub tokens: i64,
    pub window: i64,
    pub pct: f64,
    /// Always `false` here: a value freshly read from the transcript is
    /// never stale.
    pub stale: bool,
}

impl From<&crate::service::context::ContextUsage> for ContextView {
    fn from(u: &crate::service::context::ContextUsage) -> Self {
        ContextView {
            tokens: u.tokens,
            window: u.window,
            pct: ((u.tokens as f64) * 100.0 / (u.window.max(1) as f64)).round(),
            stale: false,
        }
    }
}

/// The prompt text of a human `user` entry, or `None` when the entry is not
/// a prompt (a `tool_result` carrier, or no content). A string body is used
/// as is; a content array joins its text blocks with newlines.
fn prompt_text(content: Option<&serde_json::Value>) -> Option<String> {
    match content {
        Some(serde_json::Value::String(s)) => Some(s.trim().to_string()),
        Some(serde_json::Value::Array(blocks)) => {
            if blocks
                .iter()
                .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
            {
                return None;
            }
            let texts: Vec<&str> = blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect();
            Some(texts.join("\n").trim().to_string())
        }
        _ => None,
    }
}

/// Split a transcript (JSONL text; a leading partial line is tolerated)
/// into turns. A turn starts at every human `user` prompt (a string body,
/// or a content array without `tool_result` blocks); every `assistant`
/// entry until the next prompt contributes its text blocks and one tool
/// summary per `tool_use`. Assistant output before the first prompt forms a
/// prompt-less turn. Thinking blocks, tool results and sidechain (subagent)
/// entries are excluded; a turn with neither prompt nor items is dropped.
pub fn parse_conversation(jsonl: &str) -> Vec<ConvTurn> {
    fn push(turns: &mut Vec<ConvTurn>, turn: Option<ConvTurn>) {
        if let Some(t) = turn {
            if t.prompt.is_some() || !t.items.is_empty() {
                turns.push(t);
            }
        }
    }
    let mut turns: Vec<ConvTurn> = Vec::new();
    let mut current: Option<ConvTurn> = None;
    // tool_use id → index of its item in `current`, so a later tool_result
    // carrying `is_error` can flag the line. Results always land inside the
    // turn that issued the call, so the map resets with the turn.
    let mut tool_items: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
            continue;
        }
        let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let content = v.get("message").and_then(|m| m.get("content"));
        let at = || {
            v.get("timestamp")
                .and_then(|t| t.as_str())
                .map(String::from)
        };
        match kind {
            "system" if v.get("subtype").and_then(|s| s.as_str()) == Some("compact_boundary") => {
                push(&mut turns, current.take());
                tool_items.clear();
                let meta = v.get("compactMetadata");
                current = Some(ConvTurn {
                    prompt: None,
                    at: at(),
                    ended_at: None,
                    items: vec![ConvItem::Compact {
                        trigger: meta
                            .and_then(|m| m.get("trigger"))
                            .and_then(|t| t.as_str())
                            .map(String::from),
                        pre_tokens: meta
                            .and_then(|m| m.get("preTokens"))
                            .and_then(|t| t.as_i64()),
                        summary: None,
                    }],
                });
            }
            "user" => {
                if let Some(serde_json::Value::Array(blocks)) = content {
                    for b in blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                    {
                        let is_err = b.get("is_error").and_then(|e| e.as_bool()) == Some(true);
                        let idx = b
                            .get("tool_use_id")
                            .and_then(|i| i.as_str())
                            .and_then(|i| tool_items.get(i).copied());
                        if let (Some(idx), Some(turn)) = (idx, current.as_mut()) {
                            let ended = at();
                            match turn.items.get_mut(idx) {
                                Some(ConvItem::Tool {
                                    error,
                                    done,
                                    ended_at,
                                    ..
                                }) => {
                                    *error |= is_err;
                                    *done = true;
                                    *ended_at = ended;
                                }
                                Some(ConvItem::Subagent {
                                    error,
                                    done,
                                    ended_at,
                                    result,
                                    ..
                                }) => {
                                    *error |= is_err;
                                    *done = true;
                                    *ended_at = ended;
                                    if let Some(t) = tool_result_text(b) {
                                        *result =
                                            Some(cap_chars(t.trim(), SUBAGENT_RESULT_MAX_CHARS));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                if v.get("isMeta").and_then(|b| b.as_bool()) == Some(true) {
                    continue;
                }
                if v.get("isCompactSummary").and_then(|b| b.as_bool()) == Some(true) {
                    if let (Some(text), Some(turn)) = (user_text(content), current.as_mut()) {
                        if let Some(ConvItem::Compact { summary, .. }) = turn.items.last_mut() {
                            if summary.is_none() {
                                *summary = Some(cap_chars(text.trim(), COMPACT_SUMMARY_MAX_CHARS));
                            }
                        }
                    }
                    continue;
                }
                if let Some(text) = user_text(content) {
                    // Only an entry that *starts* with the tag is a command
                    // (or its output); a human prompt that merely contains
                    // one (a pasted transcript) stays a prompt.
                    let head = text.trim_start();
                    let is_command =
                        head.starts_with("<command-name>") || head.starts_with("<command-message>");
                    let is_output = head.starts_with("<local-command-stdout>")
                        || head.starts_with("<local-command-stderr>");
                    if let Some(name) = is_command
                        .then(|| tag_text(&text, "command-name"))
                        .flatten()
                    {
                        push(&mut turns, current.take());
                        tool_items.clear();
                        current = Some(ConvTurn {
                            prompt: None,
                            at: at(),
                            ended_at: None,
                            items: vec![ConvItem::Command {
                                name,
                                args: tag_text(&text, "command-args"),
                                output: None,
                            }],
                        });
                        continue;
                    }
                    // Command output is consumed only by an open command
                    // (the current turn's); without one it is a prompt.
                    let open_command = current.as_mut().and_then(|t| {
                        t.items
                            .iter_mut()
                            .rev()
                            .find(|i| matches!(i, ConvItem::Command { .. }))
                    });
                    if let (true, Some(ConvItem::Command { output, .. })) =
                        (is_output, open_command)
                    {
                        if output.is_none() {
                            *output = tag_text(&text, "local-command-stdout")
                                .or_else(|| tag_text(&text, "local-command-stderr"))
                                .map(|out| cap_chars(&out, COMMAND_OUTPUT_MAX_CHARS));
                        }
                        continue;
                    }
                    if text
                        .trim_start()
                        .starts_with("[Request interrupted by user")
                    {
                        let turn = current.get_or_insert_with(|| ConvTurn {
                            prompt: None,
                            at: at(),
                            ended_at: None,
                            items: Vec::new(),
                        });
                        turn.items.push(ConvItem::Interrupt {
                            during_tool: text.contains("for tool use"),
                        });
                        continue;
                    }
                }
                if let Some(prompt) = prompt_text(content) {
                    push(&mut turns, current.take());
                    tool_items.clear();
                    current = Some(ConvTurn {
                        // An image-only prompt still opens a turn, unquoted.
                        prompt: (!prompt.is_empty()).then_some(prompt),
                        at: at(),
                        ended_at: None,
                        items: Vec::new(),
                    });
                }
            }
            "assistant" => {
                if let Some(serde_json::Value::Array(blocks)) = content {
                    let turn = current.get_or_insert_with(|| ConvTurn {
                        prompt: None,
                        at: at(),
                        ended_at: None,
                        items: Vec::new(),
                    });
                    if let Some(ts) = at() {
                        turn.ended_at = Some(ts);
                    }
                    for b in blocks {
                        match b.get("type").and_then(|t| t.as_str()) {
                            Some("text") => {
                                if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                                    if !t.trim().is_empty() {
                                        turn.items.push(ConvItem::Text {
                                            text: t.trim_end().to_string(),
                                        });
                                    }
                                }
                            }
                            Some("tool_use") => {
                                let id = b.get("id").and_then(|i| i.as_str()).map(String::from);
                                let name = b
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("tool")
                                    .to_string();
                                if let Some(id) = &id {
                                    tool_items.insert(id.clone(), turn.items.len());
                                }
                                let input = b.get("input");
                                if SUBAGENT_TOOLS.contains(&name.as_str()) {
                                    turn.items.push(ConvItem::Subagent {
                                        id,
                                        agent_type: input
                                            .and_then(|v| v.get("subagent_type"))
                                            .and_then(|v| v.as_str())
                                            .map(String::from),
                                        description: input
                                            .and_then(|v| v.get("description"))
                                            .and_then(|v| v.as_str())
                                            .map(String::from),
                                        result: None,
                                        error: false,
                                        at: at(),
                                        ended_at: None,
                                        done: false,
                                        name,
                                    });
                                } else {
                                    let target = tool_target(&name, input);
                                    turn.items.push(ConvItem::Tool {
                                        summary: tool_summary(b),
                                        error: false,
                                        id,
                                        name,
                                        target,
                                        at: at(),
                                        ended_at: None,
                                        done: false,
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    push(&mut turns, current);
    turns
}

/// Split a transcript into assistant turns rendered as plain text: each
/// turn's text blocks and `[tool_use] ` summary lines joined with newlines.
/// Turns without assistant content are dropped. Built on
/// [`parse_conversation`].
pub fn parse_turns(jsonl: &str) -> Vec<String> {
    parse_conversation(jsonl)
        .into_iter()
        .filter(|t| !t.items.is_empty())
        .map(|t| {
            t.items
                .iter()
                .map(|i| match i {
                    ConvItem::Text { text } => text.clone(),
                    ConvItem::Tool { summary, .. } => format!("{TOOL_USE_PREFIX}{summary}"),
                    ConvItem::Subagent {
                        name, description, ..
                    } => format!(
                        "{TOOL_USE_PREFIX}{name}(description={})",
                        one_line(description.as_deref().unwrap_or(""))
                    ),
                    ConvItem::Compact { trigger, .. } => {
                        format!("[compacted] {}", trigger.as_deref().unwrap_or("unknown"))
                    }
                    ConvItem::Command { name, args, .. } => match args {
                        Some(a) => format!("[command] {name} {a}"),
                        None => format!("[command] {name}"),
                    },
                    ConvItem::Interrupt { .. } => "[interrupted]".to_string(),
                })
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string()
        })
        .collect()
}

fn item_chars(item: &ConvItem) -> usize {
    match item {
        ConvItem::Text { text } => text.chars().count(),
        ConvItem::Tool {
            summary, target, ..
        } => summary.chars().count() + target.as_deref().map_or(0, |s| s.chars().count()),
        ConvItem::Subagent {
            description,
            result,
            ..
        } => {
            description.as_deref().map_or(0, |s| s.chars().count())
                + result.as_deref().map_or(0, |s| s.chars().count())
        }
        ConvItem::Compact { summary, .. } => summary.as_deref().map_or(0, |s| s.chars().count()),
        ConvItem::Command { name, args, output } => {
            name.chars().count()
                + args.as_deref().map_or(0, |s| s.chars().count())
                + output.as_deref().map_or(0, |s| s.chars().count())
        }
        ConvItem::Interrupt { .. } => 0,
    }
}

fn prompt_chars(turn: &ConvTurn) -> usize {
    turn.prompt.as_deref().map_or(0, |p| p.chars().count())
}

/// Keep the last `max_turns` turns, then drop the oldest items (and a turn
/// once it has none left) until prompts + items fit `max_chars`. The newest
/// item is never dropped; see [`fit_last_turn`] for what happens when the
/// last turn alone is over budget.
pub fn trim_conversation(
    mut turns: Vec<ConvTurn>,
    max_turns: usize,
    max_chars: usize,
) -> Conversation {
    let mut truncated = false;
    if turns.len() > max_turns {
        turns.drain(..turns.len() - max_turns);
        truncated = true;
    }
    let mut total: usize = turns
        .iter()
        .map(|t| prompt_chars(t) + t.items.iter().map(item_chars).sum::<usize>())
        .sum();
    while total > max_chars && !turns.is_empty() {
        if turns.len() == 1 && turns[0].items.len() <= 1 {
            truncated |= fit_last_turn(&mut turns[0], max_chars);
            break;
        }
        let first = &mut turns[0];
        if !first.items.is_empty() {
            total -= item_chars(&first.items.remove(0));
        }
        if first.items.is_empty() {
            total -= prompt_chars(first);
            turns.remove(0);
        }
        truncated = true;
    }
    Conversation {
        turns,
        truncated,
        context: None,
        events: Vec::new(),
    }
}

/// Fit a lone turn holding at most one item into `max_chars`. The reply
/// keeps what the prompt leaves over, but never less than half the budget
/// (and never less than one char, so a Text item is never blanked); its text
/// is cut from the front, since the end of a reply is what a reader waits
/// for. The prompt then gets the rest and keeps its head. Tool summaries are
/// one short line and are left whole. Returns whether anything was cut.
fn fit_last_turn(turn: &mut ConvTurn, max_chars: usize) -> bool {
    let mut cut = false;
    let item_len = turn.items.first().map_or(0, item_chars);
    let item_budget = item_len
        .min(
            max_chars
                .saturating_sub(prompt_chars(turn))
                .max(max_chars / 2),
        )
        .max(item_len.min(1));
    if let Some(ConvItem::Text { text }) = turn.items.first_mut() {
        if item_len > item_budget {
            *text = text.chars().skip(item_len - item_budget).collect();
            cut = true;
        }
    }
    let prompt_budget = max_chars.saturating_sub(turn.items.first().map_or(0, item_chars));
    if let Some(prompt) = turn.prompt.as_mut() {
        if prompt.chars().count() > prompt_budget {
            *prompt = prompt.chars().take(prompt_budget).collect();
            cut = true;
        }
    }
    if turn.prompt.as_deref() == Some("") {
        turn.prompt = None;
    }
    cut
}

/// The last `count` turns joined with a separator, trimmed from the FRONT
/// to `max_chars` (the end of the reply is what a caller waited for).
pub fn render_tail(turns: &[String], count: usize, max_chars: usize) -> String {
    let count = count.max(1);
    let start = turns.len().saturating_sub(count);
    let joined = turns[start..].join("\n\n---\n\n");
    let total = joined.chars().count();
    if total <= max_chars {
        return joined;
    }
    let dropped = total - max_chars;
    let tail: String = joined.chars().skip(dropped).collect();
    format!("[session_transcript: {dropped} chars dropped from the start — raise max_chars to see more]\n{tail}")
}

/// What to read and how much to render.
#[derive(Debug)]
pub struct TranscriptArgs {
    pub host_alias: String,
    /// tmux session whose pane cwd locates the transcript (interactive
    /// sessions). `None` for background sessions.
    pub tmux_name: Option<String>,
    /// The hook-reported transcript path, tried first when present.
    pub transcript_path: Option<String>,
    /// Fallback cwd when the pane lookup fails / there is no pane.
    pub cwd: Option<String>,
    pub claude_session_id: String,
    /// Number of most-recent turns to return (≥ 1).
    pub turns: usize,
    pub max_chars: usize,
}

/// Build the [`TranscriptArgs`] for a fleet row: the fallback cwd (the
/// worktree, else the project root), the hook-reported transcript path, and
/// the pane name — `None` for rows without a pane (`bg` / `external`).
/// `E_INVALID_STATE` when the row has no `claude_session_id` yet. The store
/// lock is released before returning.
pub fn resolve_args(
    store: &Mutex<Store>,
    row: &SessionRow,
    turns: usize,
    max_chars: usize,
) -> Result<TranscriptArgs, IpcError> {
    let claude_session_id = row.claude_session_id.clone().ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "session {} has no claude_session_id yet (not reconciled, or not a Claude session)",
                row.id
            ),
        )
    })?;
    let (cwd, transcript_path) = {
        let s = lock(store)?;
        let wt = match row.worktree_id {
            Some(wid) => s.worktree_path(wid).ok().flatten(),
            None => None,
        };
        let cwd = match wt {
            Some(p) => Some(p),
            None => match row.project_id {
                Some(pid) => s.project_base_path(pid).ok().flatten(),
                None => None,
            },
        };
        (cwd, s.session_transcript_path(row.id).ok().flatten())
    };
    let no_pane = crate::store::has_no_pane(&row.kind) || row.tmux_name.starts_with("bg:");
    Ok(TranscriptArgs {
        host_alias: row.host_alias.clone(),
        tmux_name: (!no_pane).then(|| row.tmux_name.clone()),
        transcript_path,
        cwd,
        claude_session_id,
        turns,
        max_chars,
    })
}

/// [`resolve_args`] for a specific conversation of the row. `E_INVALID` when
/// `claude_session_id` is not one of the row's conversations. The transcript
/// path is that conversation's (falls back to the cwd search). The row need
/// not have a current id for an earlier one to be read.
pub fn resolve_args_for(
    store: &Mutex<Store>,
    row: &SessionRow,
    claude_session_id: &str,
    turns: usize,
    max_chars: usize,
) -> Result<TranscriptArgs, IpcError> {
    crate::validate::claude_session_id(claude_session_id)?;
    if row.claude_session_id.as_deref() == Some(claude_session_id) {
        return resolve_args(store, row, turns, max_chars);
    }
    let conv = lock(store)?
        .get_conversation(row.id, claude_session_id)?
        .ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID,
                "claude_session_id is not a conversation of this session",
            )
        })?;
    // Everything but the id and path comes from the row; `resolve_args`
    // only needs an id to proceed.
    let mut probe = row.clone();
    probe.claude_session_id = Some(conv.claude_session_id.clone());
    let mut args = resolve_args(store, &probe, turns, max_chars)?;
    args.claude_session_id = conv.claude_session_id;
    args.transcript_path = conv.transcript_path;
    Ok(args)
}

/// `session_conversation` (Tauri command and MCP tool): read the row's
/// current conversation, or the earlier one `claude_session_id` names, and
/// write the context size back so the row's meter is right immediately —
/// only when the conversation read is the row's current one. The store lock
/// is never held across the fetch.
pub async fn fetch_conversation_for_row(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    row: &SessionRow,
    claude_session_id: Option<&str>,
    turns: usize,
    max_chars: usize,
    events_limit: i64,
) -> Result<Conversation, IpcError> {
    let args = match claude_session_id {
        Some(id) => resolve_args_for(store, row, id, turns, max_chars)?,
        None => resolve_args(store, row, turns, max_chars)?,
    };
    let claude_id = args.claude_session_id.clone();
    let mut conv = fetch_conversation(args, ssh).await?;
    if let Ok(s) = lock(store) {
        conv.events = match s.list_conversation_events(row.id, &claude_id, events_limit) {
            Ok(events) => events,
            Err(e) => {
                tracing::warn!(
                    session_id = row.id,
                    code = %e.code,
                    "conversation events query failed; returning none"
                );
                Vec::new()
            }
        };
        if row.claude_session_id.as_deref() == Some(claude_id.as_str()) {
            if let Some(v) = &conv.context {
                let _ = s.set_context(row.id, &claude_id, v.tokens, v.window, "transcript", None);
            }
        }
    }
    Ok(conv)
}

/// Most timeline events [`fetch_conversation_for_row`] attaches to a
/// `Conversation` for the UI (the `session_conversation` Tauri command).
pub const CONV_EVENTS_LIMIT_UI: i64 = 200;
/// Most timeline events the `session_conversation` MCP tool returns — kept
/// small so an assistant's context is not flooded.
pub const CONV_EVENTS_LIMIT_MCP: i64 = 50;

/// Validate `args` and build the script that prints the last `max_bytes` of
/// its transcript. Errors: `E_INVALID` (bad id / host / pane name).
fn tail_script(args: &TranscriptArgs, max_bytes: usize) -> Result<String, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::claude_session_id(&args.claude_session_id)?;
    if let Some(name) = args.tmux_name.as_deref() {
        crate::validate::tmux_name_addressable(name)?;
    }
    Ok(read_script(
        args.tmux_name.as_deref(),
        args.transcript_path.as_deref(),
        args.cwd.as_deref(),
        &args.claude_session_id,
        max_bytes,
    ))
}

/// `session_transcript`'s read: `read_bytes_for(max_chars)` bytes.
fn transcript_read_script(args: &TranscriptArgs) -> Result<String, IpcError> {
    tail_script(args, read_bytes_for(args.max_chars.clamp(1, MAX_MAX_CHARS)))
}

/// `session_conversation`'s read: a fixed [`CONV_READ_BYTES`] tail.
fn conversation_read_script(args: &TranscriptArgs) -> Result<String, IpcError> {
    tail_script(args, conv_read_bytes(args.turns))
}

/// PURE: bytes of tail to read for a `turns` window. The default window
/// reads [`CONV_READ_BYTES`]; a wider one ("Load older") scales with it,
/// capped at [`MAX_READ_BYTES`].
pub fn conv_read_bytes(turns: usize) -> usize {
    (CONV_READ_BYTES.saturating_mul(turns.max(1)) / CONV_TURNS)
        .clamp(CONV_READ_BYTES, MAX_READ_BYTES)
}

/// Run a read `script` built for `args`. Errors: `E_NO_TRANSCRIPT` (file
/// absent — the session has not written a turn yet, or runs on another cwd),
/// `E_SHELL` / `E_SSH*` for transport failures.
async fn read_tail(
    args: &TranscriptArgs,
    script: &str,
    ssh: &Arc<SshClient>,
) -> Result<String, IpcError> {
    let out = run_shell(ssh, &args.host_alias, script).await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains(NO_TRANSCRIPT) {
            return Err(IpcError::new(
                codes::E_NO_TRANSCRIPT,
                format!(
                    "no transcript for claude session {} on {}: {}",
                    args.claude_session_id,
                    args.host_alias,
                    stderr.trim()
                ),
            ));
        }
        return Err(IpcError::new(
            codes::E_SHELL,
            format!("transcript read failed: {}", stderr.trim()),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Read the last `max_bytes` of `args`' transcript (shared with
/// `service::context`). Errors as [`tail_script`] / [`read_tail`].
pub(crate) async fn read_tail_bytes(
    args: &TranscriptArgs,
    max_bytes: usize,
    ssh: &Arc<SshClient>,
) -> Result<String, IpcError> {
    let script = tail_script(args, max_bytes)?;
    read_tail(args, &script, ssh).await
}

/// Fetch and render a transcript as plain text. Errors as [`tail_script`] /
/// [`read_tail`].
pub async fn fetch_transcript(
    args: TranscriptArgs,
    ssh: &Arc<SshClient>,
) -> Result<String, IpcError> {
    let script = transcript_read_script(&args)?;
    let text = read_tail(&args, &script, ssh).await?;
    let turns = parse_turns(&text);
    Ok(render_tail(
        &turns,
        args.turns,
        args.max_chars.clamp(1, MAX_MAX_CHARS),
    ))
}

/// Fetch a transcript as structured turns, trimmed to `args.turns` and
/// `args.max_chars` (callers pass [`CONV_TURNS`] / [`CONV_MAX_CHARS`]),
/// from a fixed [`CONV_READ_BYTES`] tail. Errors as [`tail_script`] /
/// [`read_tail`].
pub async fn fetch_conversation(
    args: TranscriptArgs,
    ssh: &Arc<SshClient>,
) -> Result<Conversation, IpcError> {
    let script = conversation_read_script(&args)?;
    let text = read_tail(&args, &script, ssh).await?;
    // The Conversation tab may ask for a wider window than the MCP text
    // tool's cap; its own ceiling applies here.
    let mut conv = trim_conversation(
        parse_conversation(&text),
        args.turns.max(1),
        args.max_chars.clamp(1, CONV_MAX_CHARS_CEILING),
    );
    // A tail that filled the byte budget started mid-file: older history
    // exists even when the parsed turns fit the window.
    conv.truncated |= text.len() >= conv_read_bytes(args.turns);
    conv.context = crate::service::context::context_from_jsonl(&text)
        .as_ref()
        .map(ContextView::from);
    Ok(conv)
}

/// Run a bash script on `host_alias` (local or via ssh), bounded by
/// [`READ_WALL_CLOCK`].
async fn run_shell(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    script: &str,
) -> Result<std::process::Output, IpcError> {
    crate::ssh::run_shell_bounded(
        ssh.as_ref(),
        host_alias,
        script,
        std::time::Duration::from_secs(10),
        READ_WALL_CLOCK,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StartSource;

    #[test]
    fn project_dir_encoding_matches_claude_code() {
        // Observed on a live install (see module docs).
        assert_eq!(
            encode_project_dir(
                "/mnt/sda4/projects/github.com/martin-janci/claude-fleet/.worktrees/jhkljh"
            ),
            "-mnt-sda4-projects-github-com-martin-janci-claude-fleet--worktrees-jhkljh"
        );
        assert_eq!(encode_project_dir("/home/u"), "-home-u");
        assert_eq!(encode_project_dir("/home/u/a_b c"), "-home-u-a-b-c");
    }

    #[test]
    fn read_bytes_scales_with_the_char_budget_within_bounds() {
        assert_eq!(read_bytes_for(1), MIN_READ_BYTES);
        assert_eq!(read_bytes_for(8_000), 8_000 * 64);
        assert_eq!(read_bytes_for(64_000), 64_000 * 64);
        assert_eq!(read_bytes_for(usize::MAX), MAX_READ_BYTES);
    }

    #[test]
    fn read_script_quotes_every_interpolated_value() {
        let s = read_script(
            Some("dev-x'; rm -rf /"),
            Some("/h/.claude/projects/it's/abc.jsonl"),
            Some("/home/u/it's"),
            "abc",
            1024,
        );
        assert!(s.contains("tmux display-message -p -t '=dev-x'\\''; rm -rf /:'"));
        assert!(
            s.contains("sp='/h/.claude/projects/it'\\''s/abc.jsonl'"),
            "{s}"
        );
        assert!(s.contains("cwd='/home/u/it'\\''s'"), "{s}");
        assert!(s.contains("id='abc'"));
        assert!(s.contains("tail -c 1024"));
        assert!(s.contains("pwd -P"), "fallback dir is resolved physically");
        assert!(
            s.contains("sed 's/[^A-Za-z0-9]/-/g'"),
            "remote encoding mirrors encode_project_dir"
        );
        // No tmux lookup when the pane name is absent: the `-n ''` test fails.
        let bg = read_script(None, None, Some("/x"), "abc", 1);
        assert!(bg.contains("if [ -n '' ]; then"));
    }

    /// Run [`read_script`] with a private `$HOME` (the script only touches
    /// `$HOME/.claude/projects`), exactly as the host would.
    fn run_script(home: &std::path::Path, script: &str) -> std::process::Output {
        std::process::Command::new("bash")
            .arg("-c")
            .arg(script)
            .env("HOME", home)
            .output()
            .unwrap()
    }

    const SID: &str = "550e8400-e29b-41d4-a716-446655440000";

    fn write_transcript(dir: &std::path::Path, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(format!("{SID}.jsonl")), text).unwrap();
    }

    #[test]
    fn symlinked_fallback_dir_resolves_to_the_physical_transcript_dir() {
        // Claude records a session started under `link/proj` (link → real)
        // by its PHYSICAL cwd. A decoy under the logical encoding proves the
        // script resolved the symlink instead of encoding the path verbatim.
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let real = root.join("real");
        std::fs::create_dir_all(real.join("proj")).unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let home = root.join("home");
        let projects = home.join(".claude/projects");
        let phys = real.join("proj");
        let logical = link.join("proj");
        write_transcript(
            &projects.join(encode_project_dir(&phys.to_string_lossy())),
            "PHYSICAL",
        );
        write_transcript(
            &projects.join(encode_project_dir(&logical.to_string_lossy())),
            "LOGICAL-DECOY",
        );
        let script = read_script(None, None, Some(&logical.to_string_lossy()), SID, 1000);
        let out = run_script(&home, &script);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout), "PHYSICAL");
    }

    #[test]
    fn stored_transcript_path_is_preferred_over_any_derived_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let cwd = root.join("proj");
        std::fs::create_dir_all(&cwd).unwrap();
        write_transcript(
            &home
                .join(".claude/projects")
                .join(encode_project_dir(&cwd.to_string_lossy())),
            "DERIVED",
        );
        let stored_dir = root.join("elsewhere/.claude/projects/xyz");
        write_transcript(&stored_dir, "STORED");
        let stored = stored_dir.join(format!("{SID}.jsonl"));
        let script = read_script(
            None,
            Some(&stored.to_string_lossy()),
            Some(&cwd.to_string_lossy()),
            SID,
            1000,
        );
        assert_eq!(
            String::from_utf8_lossy(&run_script(&home, &script).stdout),
            "STORED"
        );
        // A stale stored path (file gone) falls back to the derived one.
        std::fs::remove_file(&stored).unwrap();
        let script = read_script(
            None,
            Some(&stored.to_string_lossy()),
            Some(&cwd.to_string_lossy()),
            SID,
            1000,
        );
        assert_eq!(
            String::from_utf8_lossy(&run_script(&home, &script).stdout),
            "DERIVED"
        );
    }

    #[test]
    fn unknown_cwd_or_truncated_dir_name_is_found_by_session_id() {
        // Claude truncates encoded names over 200 chars with a hash suffix;
        // neither that nor a dead pane stops the id-based lookup.
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        write_transcript(
            &home
                .join(".claude/projects")
                .join(format!("{}-3f9a1c", "-x".repeat(100))),
            "BY-ID",
        );
        let out = run_script(&home, &read_script(None, None, None, SID, 1000));
        assert_eq!(String::from_utf8_lossy(&out.stdout), "BY-ID");
        let empty = tmp.path().join("empty-home");
        let out = run_script(&empty, &read_script(None, None, None, SID, 1000));
        assert_eq!(out.status.code(), Some(4));
        assert!(String::from_utf8_lossy(&out.stderr).contains(NO_TRANSCRIPT));
    }

    fn line(v: serde_json::Value) -> String {
        v.to_string()
    }

    #[test]
    fn parse_turns_splits_on_human_prompts_and_summarises_tool_use() {
        let jsonl = [
            line(serde_json::json!({"type":"user","message":{"role":"user","content":"first"}})),
            line(serde_json::json!({"type":"assistant","message":{"content":[
                {"type":"thinking","thinking":"secret"},
                {"type":"text","text":"Let me look."},
                {"type":"tool_use","name":"Bash","input":{"command":"ls -la","description":"list"}}
            ]}})),
            line(serde_json::json!({"type":"user","message":{"content":[
                {"type":"tool_result","tool_use_id":"x","content":"a\nb"}]}})),
            line(serde_json::json!({"type":"assistant","message":{"content":[
                {"type":"text","text":"Two files."}]}})),
            line(serde_json::json!({"type":"user","message":{"role":"user","content":"second"}})),
            line(
                serde_json::json!({"type":"assistant","isSidechain":true,"message":{"content":[
                {"type":"text","text":"subagent noise"}]}}),
            ),
            line(serde_json::json!({"type":"assistant","message":{"content":[
                {"type":"text","text":"Done: all good."}]}})),
        ]
        .join("\n");
        let turns = parse_turns(&jsonl);
        assert_eq!(turns.len(), 2);
        assert_eq!(
            turns[0],
            "Let me look.\n[tool_use] Bash(command=ls -la)\nTwo files."
        );
        assert_eq!(turns[1], "Done: all good.");
        assert!(!jsonl.is_empty());
        // Thinking blocks and sidechain entries never leak.
        assert!(!turns
            .iter()
            .any(|t| t.contains("secret") || t.contains("subagent")));
    }

    #[test]
    fn parse_turns_tolerates_a_truncated_leading_line_and_garbage() {
        let jsonl = format!(
            "\"content\":[{{\"type\":\"text\",\"text\":\"cut\"}}]}}}}\nnot json\n{}\n{}",
            line(serde_json::json!({"type":"user","message":{"content":"q"}})),
            line(
                serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"a"}]}})
            ),
        );
        assert_eq!(parse_turns(&jsonl), vec!["a".to_string()]);
        assert!(parse_turns("").is_empty());
    }

    #[test]
    fn render_tail_keeps_the_last_turns_and_trims_from_the_front() {
        let turns = vec!["one".to_string(), "two".to_string(), "three".to_string()];
        assert_eq!(render_tail(&turns, 1, 100), "three");
        assert_eq!(render_tail(&turns, 2, 100), "two\n\n---\n\nthree");
        assert_eq!(
            render_tail(&turns, 0, 100),
            "three",
            "count is floored at 1"
        );
        assert_eq!(
            render_tail(&turns, 10, 100),
            "one\n\n---\n\ntwo\n\n---\n\nthree"
        );
        let cut = render_tail(&turns, 1, 3);
        assert!(cut.starts_with("[session_transcript: 2 chars dropped"));
        assert!(cut.ends_with("\nree"));
        assert_eq!(render_tail(&[], 1, 10), "");
    }

    #[test]
    fn tool_use_summary_is_one_line_and_capped() {
        let b = serde_json::json!({"type":"tool_use","name":"Edit","input":{"file_path":"/a/b.rs","old_string":"x\ny"}});
        assert_eq!(summarize_tool_use(&b), "[tool_use] Edit(file_path=/a/b.rs)");
        let long = serde_json::json!({"type":"tool_use","name":"Bash","input":{"command":"x".repeat(500)}});
        let s = summarize_tool_use(&long);
        assert!(s.chars().count() <= TOOL_SUMMARY_CHARS + 2);
        assert!(!s.contains('\n'));
        let none = serde_json::json!({"type":"tool_use","name":"Skill","input":{"other":1}});
        assert_eq!(summarize_tool_use(&none), "[tool_use] Skill({\"other\":1})");
    }

    #[test]
    fn conv_read_bytes_scales_with_the_window_and_caps() {
        assert_eq!(conv_read_bytes(CONV_TURNS), CONV_READ_BYTES);
        assert_eq!(conv_read_bytes(0), CONV_READ_BYTES);
        assert_eq!(conv_read_bytes(20), CONV_READ_BYTES * 2);
        assert_eq!(conv_read_bytes(CONV_MAX_TURNS), MAX_READ_BYTES);
    }

    #[test]
    fn conv_limits_default_clamp_and_scale() {
        assert_eq!(conv_limits(None), (CONV_TURNS, CONV_MAX_CHARS));
        assert_eq!(conv_limits(Some(0)), (1, CONV_MAX_CHARS));
        assert_eq!(conv_limits(Some(20)), (20, CONV_MAX_CHARS * 2));
        assert_eq!(
            conv_limits(Some(10_000)),
            (CONV_MAX_TURNS, CONV_MAX_CHARS_CEILING)
        );
    }

    #[test]
    fn parse_conversation_records_when_the_reply_last_advanced() {
        let jsonl = [
            line(serde_json::json!({"type":"user","timestamp":"2026-09-13T10:00:00Z","message":{"content":"go"}})),
            line(serde_json::json!({"type":"assistant","timestamp":"2026-09-13T10:00:05Z","message":{"content":[{"type":"text","text":"a"}]}})),
            line(serde_json::json!({"type":"assistant","timestamp":"2026-09-13T10:02:19Z","message":{"content":[{"type":"text","text":"b"}]}})),
            line(serde_json::json!({"type":"user","timestamp":"2026-09-13T10:03:00Z","message":{"content":"again"}})),
        ]
        .join("\n");
        let turns = parse_conversation(&jsonl);
        assert_eq!(turns[0].at.as_deref(), Some("2026-09-13T10:00:00Z"));
        assert_eq!(turns[0].ended_at.as_deref(), Some("2026-09-13T10:02:19Z"));
        // a prompt with no reply yet has no end
        assert_eq!(turns[1].ended_at, None);
    }

    #[test]
    fn parse_conversation_flags_a_tool_whose_result_was_an_error() {
        let jsonl = [
            line(serde_json::json!({"type":"user","message":{"content":"go"}})),
            line(serde_json::json!({"type":"assistant","message":{"content":[
                {"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}},
                {"type":"tool_use","id":"t2","name":"Read","input":{"file_path":"a.rs"}}]}})),
            line(serde_json::json!({"type":"user","message":{"content":[
                {"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"exit 101"},
                {"type":"tool_result","tool_use_id":"t2","content":"ok"}]}})),
            line(serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"One failed."}]}})),
        ]
        .join("\n");
        let turns = parse_conversation(&jsonl);
        assert_eq!(turns.len(), 1);
        assert_eq!(
            turns[0].items,
            vec![
                ConvItem::Tool {
                    summary: "Bash(command=cargo test)".into(),
                    error: true,
                    id: Some("t1".into()),
                    name: "Bash".into(),
                    target: Some("cargo test".into()),
                    at: None,
                    ended_at: None,
                    done: true,
                },
                ConvItem::Tool {
                    summary: "Read(file_path=a.rs)".into(),
                    error: false,
                    id: Some("t2".into()),
                    name: "Read".into(),
                    target: Some("a.rs".into()),
                    at: None,
                    ended_at: None,
                    done: true,
                },
                ConvItem::Text {
                    text: "One failed.".into()
                },
            ]
        );
        // the plain-text projection (MCP) is unchanged by the flag
        assert_eq!(
            parse_turns(&jsonl),
            vec![
                "[tool_use] Bash(command=cargo test)\n[tool_use] Read(file_path=a.rs)\nOne failed."
                    .to_string()
            ]
        );
    }

    #[test]
    fn parse_conversation_keeps_prompts_text_and_tool_lines() {
        let jsonl = [
            line(serde_json::json!({"type":"user","timestamp":"2026-09-13T10:00:00Z","message":{"role":"user","content":"first"}})),
            line(serde_json::json!({"type":"assistant","message":{"content":[
                {"type":"thinking","thinking":"secret"},
                {"type":"text","text":"Let me look."},
                {"type":"tool_use","name":"Bash","input":{"command":"ls -la"}}]}})),
            line(serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"x","content":"a"}]}})),
            line(serde_json::json!({"type":"user","message":{"content":[{"type":"text","text":"second"},{"type":"text","text":"part"}]}})),
            line(serde_json::json!({"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"noise"}]}})),
            line(serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"Done."}]}})),
        ].join("\n");
        let turns = parse_conversation(&jsonl);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].prompt.as_deref(), Some("first"));
        assert_eq!(turns[0].at.as_deref(), Some("2026-09-13T10:00:00Z"));
        assert_eq!(
            turns[0].items,
            vec![
                ConvItem::Text {
                    text: "Let me look.".into()
                },
                ConvItem::Tool {
                    summary: "Bash(command=ls -la)".into(),
                    error: false,
                    id: None,
                    name: "Bash".into(),
                    target: Some("ls -la".into()),
                    at: None,
                    ended_at: None,
                    done: false,
                },
            ]
        );
        assert_eq!(turns[1].prompt.as_deref(), Some("second\npart"));
        assert_eq!(
            turns[1].items,
            vec![ConvItem::Text {
                text: "Done.".into()
            }]
        );
    }

    #[test]
    fn a_prompt_without_reply_is_kept_in_conversation_but_not_in_turns() {
        let jsonl = line(serde_json::json!({"type":"user","message":{"content":"waiting"}}));
        assert_eq!(parse_conversation(&jsonl).len(), 1);
        assert!(parse_turns(&jsonl).is_empty());
    }

    fn jl(lines: &[serde_json::Value]) -> String {
        lines
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn user(content: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"type":"user","timestamp":"2026-09-18T10:00:00Z","message":{"content":content}})
    }
    fn asst(text: &str) -> serde_json::Value {
        serde_json::json!({"type":"assistant","timestamp":"2026-09-18T10:00:05Z",
            "message":{"content":[{"type":"text","text":text}]}})
    }
    fn tool_use(ts: &str, id: &str, name: &str, input: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"type":"assistant","timestamp":ts,
            "message":{"content":[{"type":"tool_use","id":id,"name":name,"input":input}]}})
    }
    fn tool_result(ts: &str, id: &str, content: serde_json::Value, err: bool) -> serde_json::Value {
        serde_json::json!({"type":"user","timestamp":ts,
            "message":{"content":[{"type":"tool_result","tool_use_id":id,"content":content,"is_error":err}]}})
    }
    /// A `Tool` item with only `summary`/`error` set, for synthetic
    /// (non-parsed) test turns that don't exercise the new fields.
    fn tool_item(summary: &str, error: bool) -> ConvItem {
        ConvItem::Tool {
            summary: summary.into(),
            error,
            id: None,
            name: String::new(),
            target: None,
            at: None,
            ended_at: None,
            done: false,
        }
    }

    #[test]
    fn a_tool_item_carries_id_name_target_times_and_done() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            tool_use(
                "2026-09-18T09:00:20.100Z",
                "toolu_1",
                "Bash",
                serde_json::json!({"command":"cargo test -p fleet-core\nsecond line"}),
            ),
            tool_result(
                "2026-09-18T09:00:41.900Z",
                "toolu_1",
                serde_json::json!("ok"),
                false,
            ),
            tool_use(
                "2026-09-18T09:00:42Z",
                "toolu_2",
                "Edit",
                serde_json::json!({"file_path":"/w/src/a.rs","old_string":"a","new_string":"b"}),
            ),
        ]));
        match &t[0].items[0] {
            ConvItem::Tool {
                id,
                name,
                target,
                at,
                ended_at,
                done,
                error,
                ..
            } => {
                assert_eq!(id.as_deref(), Some("toolu_1"));
                assert_eq!(name, "Bash");
                assert_eq!(target.as_deref(), Some("cargo test -p fleet-core"));
                assert_eq!(at.as_deref(), Some("2026-09-18T09:00:20.100Z"));
                assert_eq!(ended_at.as_deref(), Some("2026-09-18T09:00:41.900Z"));
                assert!(*done && !*error);
            }
            other => panic!("{other:?}"),
        }
        match &t[0].items[1] {
            ConvItem::Tool {
                target,
                done,
                ended_at,
                ..
            } => {
                assert_eq!(target.as_deref(), Some("/w/src/a.rs"));
                assert!(!*done);
                assert_eq!(*ended_at, None);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn task_and_agent_calls_become_subagent_items_with_their_final_text() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            tool_use(
                "2026-09-18T09:00:00Z",
                "toolu_3",
                "Task",
                serde_json::json!({"description":"Map the store","prompt":"p","subagent_type":"Explore"}),
            ),
            tool_result(
                "2026-09-18T09:02:00Z",
                "toolu_3",
                serde_json::json!([{"type":"text","text":"Final report: all good"}]),
                false,
            ),
            tool_use(
                "2026-09-18T09:03:00Z",
                "toolu_4",
                "Agent",
                serde_json::json!({"description":"Second"}),
            ),
        ]));
        assert_eq!(
            t[0].items[0],
            ConvItem::Subagent {
                id: Some("toolu_3".into()),
                name: "Task".into(),
                agent_type: Some("Explore".into()),
                description: Some("Map the store".into()),
                result: Some("Final report: all good".into()),
                error: false,
                at: Some("2026-09-18T09:00:00Z".into()),
                ended_at: Some("2026-09-18T09:02:00Z".into()),
                done: true,
            }
        );
        assert!(matches!(
            &t[0].items[1],
            ConvItem::Subagent {
                done: false,
                agent_type: None,
                ..
            }
        ));
    }

    #[test]
    fn tool_target_prefers_what_the_tool_touched() {
        let j = |v: serde_json::Value| Some(v);
        assert_eq!(
            tool_target(
                "Read",
                j(serde_json::json!({"file_path":"/a/b.rs"})).as_ref()
            )
            .as_deref(),
            Some("/a/b.rs")
        );
        assert_eq!(
            tool_target(
                "Grep",
                j(serde_json::json!({"pattern":"fn x","path":"src"})).as_ref()
            )
            .as_deref(),
            Some("fn x")
        );
        assert_eq!(
            tool_target(
                "WebFetch",
                j(serde_json::json!({"url":"https://x.y"})).as_ref()
            )
            .as_deref(),
            Some("https://x.y")
        );
        assert_eq!(
            tool_target("Bash", j(serde_json::json!({"command":"a\nb"})).as_ref()).as_deref(),
            Some("a")
        );
        assert_eq!(
            tool_target("TodoWrite", j(serde_json::json!({"todos":[]})).as_ref()),
            None
        );
        let long = "x".repeat(300);
        assert_eq!(
            tool_target("Bash", j(serde_json::json!({"command": long})).as_ref())
                .unwrap()
                .chars()
                .count(),
            121
        );
    }

    #[test]
    fn the_mcp_text_rendering_of_tools_is_unchanged() {
        let turns = parse_turns(&jl(&[
            user(serde_json::json!("go")),
            tool_use(
                "2026-09-18T09:00:00Z",
                "toolu_1",
                "Bash",
                serde_json::json!({"command":"ls"}),
            ),
        ]));
        assert!(turns.join("\n").contains("[tool_use] Bash(command=ls)"));
    }

    #[test]
    fn subagent_calls_render_like_the_old_tool_summary_in_plain_text() {
        // parse_turns pins the plain-text (MCP) projection: a Subagent
        // renders the same "Name(description=…)" shape tool_summary used
        // to produce for Task/Agent before they were split out.
        let turns = parse_turns(&jl(&[
            user(serde_json::json!("go")),
            tool_use(
                "2026-09-18T09:00:00Z",
                "toolu_5",
                "Task",
                serde_json::json!({"description":"Map the store","prompt":"p","subagent_type":"Explore"}),
            ),
        ]));
        assert!(turns
            .join("\n")
            .contains("[tool_use] Task(description=Map the store)"));
    }

    #[test]
    fn a_compaction_is_its_own_item_with_its_summary_not_a_prompt() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("first")),
            asst("done"),
            serde_json::json!({"type":"system","subtype":"compact_boundary","timestamp":"2026-09-18T10:01:00Z",
                "compactMetadata":{"trigger":"auto","preTokens":180000}}),
            serde_json::json!({"type":"user","isCompactSummary":true,"timestamp":"2026-09-18T10:01:00Z",
                "message":{"content":"This session is being continued… Summary: A"}}),
            user(serde_json::json!("next")),
        ]));
        assert_eq!(t.len(), 3);
        assert_eq!(t[1].prompt, None);
        assert_eq!(
            t[1].items,
            vec![ConvItem::Compact {
                trigger: Some("auto".into()),
                pre_tokens: Some(180_000),
                summary: Some("This session is being continued… Summary: A".into()),
            }]
        );
        assert_eq!(t[2].prompt.as_deref(), Some("next"));
    }

    #[test]
    fn a_slash_command_opens_a_turn_and_collects_its_output() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("<command-name>/model</command-name>\n  <command-message>model</command-message>\n  <command-args>opus</command-args>")),
            user(serde_json::json!("<local-command-stdout>Set model to opus</local-command-stdout>")),
        ]));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].prompt, None);
        assert_eq!(
            t[0].items,
            vec![ConvItem::Command {
                name: "/model".into(),
                args: Some("opus".into()),
                output: Some("Set model to opus".into()),
            }]
        );
    }

    #[test]
    fn a_command_without_args_has_none() {
        let t = parse_conversation(&jl(&[user(serde_json::json!(
            "<command-name>/clear</command-name>\n<command-message>clear</command-message>\n<command-args></command-args>"
        ))]));
        assert_eq!(
            t[0].items,
            vec![ConvItem::Command {
                name: "/clear".into(),
                args: None,
                output: None
            }]
        );
    }

    #[test]
    fn a_command_message_first_shape_still_parses() {
        let t = parse_conversation(&jl(&[user(serde_json::json!(
            "<command-message>clear</command-message>\n<command-name>/clear</command-name>"
        ))]));
        assert_eq!(
            t[0].items,
            vec![ConvItem::Command {
                name: "/clear".into(),
                args: None,
                output: None
            }]
        );
    }

    #[test]
    fn a_prompt_with_an_embedded_command_tag_stays_a_prompt() {
        let text = "why did this happen?\n<command-name>/model</command-name>";
        let t = parse_conversation(&jl(&[user(serde_json::json!(text)), asst("because")]));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].prompt.as_deref(), Some(text));
        assert_eq!(
            t[0].items,
            vec![ConvItem::Text {
                text: "because".into()
            }]
        );
    }

    #[test]
    fn a_stdout_looking_prompt_without_an_open_command_stays_a_prompt() {
        let text = "<local-command-stdout>pasted</local-command-stdout> what is this?";
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("first")),
            asst("ok"),
            user(serde_json::json!(text)),
        ]));
        assert_eq!(t.len(), 2);
        assert_eq!(t[1].prompt.as_deref(), Some(text));
    }

    #[test]
    fn embedded_stdout_mid_prompt_is_not_command_output() {
        let text = "look: <local-command-stdout>x</local-command-stdout>";
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("<command-name>/model</command-name>")),
            user(serde_json::json!(text)),
        ]));
        assert_eq!(t.len(), 2);
        assert_eq!(
            t[0].items,
            vec![ConvItem::Command {
                name: "/model".into(),
                args: None,
                output: None
            }]
        );
        assert_eq!(t[1].prompt.as_deref(), Some(text));
    }

    #[test]
    fn meta_entries_are_skipped() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("real prompt")),
            serde_json::json!({"type":"user","isMeta":true,"message":{"content":[{"type":"text","text":"Base directory for this skill: x"}]}}),
            asst("ok"),
        ]));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].prompt.as_deref(), Some("real prompt"));
        assert_eq!(t[0].items, vec![ConvItem::Text { text: "ok".into() }]);
    }

    #[test]
    fn an_interrupt_is_an_item_of_the_current_turn() {
        let t = parse_conversation(&jl(&[
            user(serde_json::json!("go")),
            asst("working"),
            user(
                serde_json::json!([{"type":"text","text":"[Request interrupted by user for tool use]"}]),
            ),
            user(serde_json::json!([{"type":"text","text":"[Request interrupted by user]"}])),
        ]));
        assert_eq!(t.len(), 1);
        assert_eq!(
            t[0].items,
            vec![
                ConvItem::Text {
                    text: "working".into()
                },
                ConvItem::Interrupt { during_tool: true },
                ConvItem::Interrupt { during_tool: false },
            ]
        );
    }

    #[test]
    fn plain_text_rendering_names_the_new_items() {
        let turns = parse_turns(&jl(&[
            user(serde_json::json!(
                "<command-name>/model</command-name><command-args>opus</command-args>"
            )),
            asst("switched"),
            serde_json::json!({"type":"system","subtype":"compact_boundary","compactMetadata":{"trigger":"manual","preTokens":5}}),
        ]));
        let all = turns.join("\n");
        assert!(all.contains("[command] /model opus"));
        assert!(all.contains("[compacted] manual"));
    }

    #[test]
    fn trim_conversation_drops_oldest_first() {
        let t = |p: &str, n: usize| ConvTurn {
            prompt: Some(p.into()),
            at: None,
            ended_at: None,
            items: vec![ConvItem::Text {
                text: "x".repeat(n),
            }],
        };
        let c = trim_conversation(vec![t("a", 10), t("b", 10), t("c", 10)], 2, 1_000);
        assert_eq!(c.turns.len(), 2);
        assert!(c.truncated);
        assert_eq!(c.turns[0].prompt.as_deref(), Some("b"));
        let c = trim_conversation(vec![t("a", 50), t("b", 50)], 10, 60);
        assert!(c.truncated);
        assert_eq!(c.turns.last().unwrap().prompt.as_deref(), Some("b"));
        let c = trim_conversation(vec![t("a", 5)], 10, 1_000);
        assert!(!c.truncated);
    }

    #[test]
    fn leading_assistant_entries_form_a_prompt_less_turn_stamped_by_the_first_entry() {
        // The JSONL tail can start mid-turn: the opening prompt was cut off.
        let jsonl = [
            line(serde_json::json!({"type":"assistant","timestamp":"2026-09-13T09:00:00Z","message":{"content":[{"type":"text","text":"tail"}]}})),
            line(serde_json::json!({"type":"assistant","timestamp":"2026-09-13T09:00:05Z","message":{"content":[{"type":"text","text":"more"}]}})),
            line(serde_json::json!({"type":"user","message":{"content":"next"}})),
            line(serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"ok"}]}})),
        ]
        .join("\n");
        let turns = parse_conversation(&jsonl);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].prompt, None);
        assert_eq!(turns[0].at.as_deref(), Some("2026-09-13T09:00:00Z"));
        assert_eq!(turns[1].at, None, "no timestamp on the prompt entry");
        // A prompt-less turn with no items never appears.
        assert!(parse_conversation("").is_empty());
    }

    #[test]
    fn conversation_serializes_to_the_frontend_shape() {
        let c = Conversation {
            turns: vec![ConvTurn {
                prompt: None,
                at: Some("2026-09-13T10:00:00Z".into()),
                ended_at: None,
                items: vec![
                    ConvItem::Text { text: "hi".into() },
                    tool_item("Bash(command=ls)", false),
                ],
            }],
            truncated: false,
            context: None,
            events: Vec::new(),
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            serde_json::json!({"turns":[{"prompt":null,"at":"2026-09-13T10:00:00Z","ended_at":null,"items":[
                {"kind":"text","text":"hi"},
                {"kind":"tool","summary":"Bash(command=ls)","error":false,"id":null,"name":"","target":null,"at":null,"ended_at":null,"done":false}]}],
                "truncated":false,"context":null,"events":[]})
        );
    }

    #[test]
    fn conversation_context_view_rounds_pct() {
        let u = crate::service::context::ContextUsage {
            tokens: 50_000,
            window: 200_000,
            model: None,
        };
        let v = ContextView::from(&u);
        assert_eq!((v.pct, v.stale), (25.0, false));
    }

    #[test]
    fn trim_conversation_counts_prompts_and_keeps_the_newest_item() {
        let turn = ConvTurn {
            prompt: Some("p".repeat(10)),
            at: None,
            ended_at: None,
            items: vec![
                ConvItem::Text {
                    text: "a".repeat(10),
                },
                tool_item(&"b".repeat(10), false),
            ],
        };
        // 30 chars total; a 25 budget drops the oldest item only.
        let c = trim_conversation(vec![turn.clone()], 10, 25);
        assert!(c.truncated);
        assert_eq!(c.turns.len(), 1);
        assert_eq!(c.turns[0].prompt.as_deref(), Some("pppppppppp"));
        assert_eq!(c.turns[0].items, vec![tool_item(&"b".repeat(10), false)]);
        // Exactly at budget: nothing dropped.
        assert!(!trim_conversation(vec![turn], 10, 30).truncated);
    }

    #[test]
    fn capped_tool_line_matches_the_pre_split_rendering_exactly() {
        // Before the Name(input) / prefix split the whole prefixed line was
        // cut at TOOL_SUMMARY_CHARS chars and "…)" appended.
        let long = serde_json::json!({"type":"tool_use","name":"Bash","input":{"command":"é".repeat(500)}});
        let full = format!("[tool_use] Bash(command={})", "é".repeat(500));
        let old = full.chars().take(TOOL_SUMMARY_CHARS).collect::<String>() + "…)";
        assert_eq!(summarize_tool_use(&long), old);
        // At exactly the cap nothing is cut.
        let fits = "[tool_use] Bash(command=)".chars().count();
        let exact = serde_json::json!({"type":"tool_use","name":"Bash","input":{"command":"y".repeat(TOOL_SUMMARY_CHARS - fits)}});
        assert!(!summarize_tool_use(&exact).contains('…'));
        assert_eq!(
            summarize_tool_use(&exact).chars().count(),
            TOOL_SUMMARY_CHARS
        );
    }

    #[test]
    fn a_single_oversized_newest_item_is_cut_from_the_front() {
        let turn = ConvTurn {
            prompt: Some("q".into()),
            at: None,
            ended_at: None,
            items: vec![ConvItem::Text {
                text: format!("{}END", "x".repeat(100)),
            }],
        };
        let c = trim_conversation(vec![turn], 10, 21);
        assert!(c.truncated);
        assert_eq!(c.turns.len(), 1);
        assert_eq!(c.turns[0].prompt.as_deref(), Some("q"));
        let ConvItem::Text { text } = &c.turns[0].items[0] else {
            panic!("text item expected");
        };
        assert_eq!(text.chars().count(), 20);
        assert!(text.ends_with("END"), "{text}");
    }

    #[test]
    fn a_huge_prompt_still_keeps_its_reply() {
        let older = ConvTurn {
            prompt: Some("old".into()),
            at: None,
            ended_at: None,
            items: vec![ConvItem::Text {
                text: "earlier".into(),
            }],
        };
        let turn = ConvTurn {
            prompt: Some(format!("HEAD{}", "p".repeat(70_000))),
            at: None,
            ended_at: None,
            items: vec![
                tool_item("Bash(command=ls)", false),
                ConvItem::Text {
                    text: "the reply!".into(),
                },
            ],
        };
        let c = trim_conversation(vec![older, turn], 10, CONV_MAX_CHARS);
        assert!(c.truncated);
        assert_eq!(c.turns.len(), 1);
        let last = &c.turns[0];
        assert_eq!(
            last.items,
            vec![ConvItem::Text {
                text: "the reply!".into()
            }],
            "the newest reply survives whole"
        );
        let prompt = last.prompt.as_deref().unwrap();
        assert!(prompt.starts_with("HEAD"), "the prompt keeps its head");
        assert_eq!(prompt.chars().count() + 10, CONV_MAX_CHARS);

        // Both oversized: the reply gets half the budget (its end), the
        // prompt the rest (its head); nothing is blanked.
        let both = ConvTurn {
            prompt: Some(format!("HEAD{}", "p".repeat(100))),
            at: None,
            ended_at: None,
            items: vec![ConvItem::Text {
                text: format!("{}END", "x".repeat(100)),
            }],
        };
        let c = trim_conversation(vec![both], 10, 40);
        assert!(c.truncated);
        let ConvItem::Text { text } = &c.turns[0].items[0] else {
            panic!("text item expected");
        };
        assert_eq!(text.chars().count(), 20);
        assert!(text.ends_with("END"));
        assert_eq!(c.turns[0].prompt.as_deref(), Some("HEADpppppppppppppppp"));

        // Even a one-char budget leaves a non-empty reply.
        let tiny = ConvTurn {
            prompt: Some("long prompt".into()),
            at: None,
            ended_at: None,
            items: vec![ConvItem::Text { text: "abc".into() }],
        };
        let c = trim_conversation(vec![tiny], 10, 1);
        assert_eq!(c.turns[0].items, vec![ConvItem::Text { text: "c".into() }]);
        assert_eq!(c.turns[0].prompt, None);
    }

    const RA_UUID: &str = "550e8400-e29b-41d4-a716-446655440001";

    #[test]
    fn resolve_args_uses_the_pane_only_for_rows_that_have_one() {
        let store = std::sync::Mutex::new(crate::store::Store::open_in_memory().unwrap());
        let (tmux_id, bg_id) = {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            let tmux_id = s
                .upsert_session("dev-x", "local", None, None, 1, 1, "running", None)
                .unwrap();
            s.set_claude_session_id(tmux_id, SID).unwrap();
            s.set_transcript_path_by_claude_id(SID, "/h/.claude/projects/p/x.jsonl")
                .unwrap();
            let bg_id = s
                .upsert_bg_session(
                    "local",
                    &format!("bg:{RA_UUID}"),
                    None,
                    RA_UUID,
                    None,
                    1,
                    "external",
                )
                .unwrap();
            (tmux_id, bg_id)
        };
        let row = |id| {
            store
                .lock()
                .unwrap()
                .get_session_by_id(id)
                .unwrap()
                .unwrap()
        };

        let a = resolve_args(&store, &row(tmux_id), 3, 500).unwrap();
        assert_eq!(a.host_alias, "local");
        assert_eq!(a.tmux_name.as_deref(), Some("dev-x"));
        assert_eq!(
            a.transcript_path.as_deref(),
            Some("/h/.claude/projects/p/x.jsonl")
        );
        assert_eq!(a.claude_session_id, SID);
        assert_eq!((a.turns, a.max_chars), (3, 500));

        let a = resolve_args(&store, &row(bg_id), 1, 1).unwrap();
        assert_eq!(a.tmux_name, None);
        assert_eq!(a.claude_session_id, RA_UUID);

        // The kind decides, not only the sentinel prefix.
        let mut odd = row(tmux_id);
        odd.kind = "external".into();
        assert_eq!(resolve_args(&store, &odd, 1, 1).unwrap().tmux_name, None);

        let mut no_id = row(tmux_id);
        no_id.claude_session_id = None;
        assert_eq!(
            resolve_args(&store, &no_id, 1, 1).unwrap_err().code,
            "E_INVALID_STATE"
        );
    }

    const CONV_A: &str = "550e8400-e29b-41d4-a716-4466554400aa";
    const CONV_B: &str = "550e8400-e29b-41d4-a716-4466554400bb";

    /// A row whose current conversation is A, with an earlier B; each has
    /// a transcript in `dir` whose last reply used `a_tokens` / `b_tokens`.
    fn two_conversations(
        dir: &std::path::Path,
        a_tokens: i64,
        b_tokens: i64,
    ) -> (std::sync::Mutex<Store>, SessionRow, String, String) {
        let write = |id: &str, tokens: i64| {
            let p = dir.join(format!("{id}.jsonl"));
            let line = serde_json::json!({"type":"assistant","message":{
                "model":"claude-opus-5","usage":{"input_tokens": tokens}}});
            std::fs::write(&p, format!("{line}\n")).unwrap();
            p.to_string_lossy().into_owned()
        };
        let (pa, pb) = (write(CONV_A, a_tokens), write(CONV_B, b_tokens));
        let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        let row = {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            let id = s
                .upsert_session(
                    "no-such-pane-t7",
                    "local",
                    None,
                    None,
                    1,
                    1,
                    "running",
                    None,
                )
                .unwrap();
            s.rebind_conversation(id, CONV_B, StartSource::Startup, Some(&pb), None)
                .unwrap();
            s.rebind_conversation(id, CONV_A, StartSource::Clear, Some(&pa), None)
                .unwrap();
            s.get_session_by_id(id).unwrap().unwrap()
        };
        (store, row, pa, pb)
    }

    #[test]
    fn resolve_args_for_an_earlier_conversation_uses_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let (store, row, pa, pb) = two_conversations(dir.path(), 1, 2);
        let a = resolve_args_for(&store, &row, CONV_B, 3, 500).unwrap();
        assert_eq!(a.claude_session_id, CONV_B);
        assert_eq!(a.transcript_path.as_deref(), Some(pb.as_str()));
        assert_eq!((a.turns, a.max_chars), (3, 500));
        // The current id is the plain resolve.
        let a = resolve_args_for(&store, &row, CONV_A, 3, 500).unwrap();
        assert_eq!(a.transcript_path.as_deref(), Some(pa.as_str()));
        // Not one of the row's conversations / not an id at all.
        let unknown = "550e8400-e29b-41d4-a716-4466554400cc";
        assert_eq!(
            resolve_args_for(&store, &row, unknown, 3, 500)
                .unwrap_err()
                .code,
            "E_INVALID"
        );
        assert_eq!(
            resolve_args_for(&store, &row, "../x", 3, 500)
                .unwrap_err()
                .code,
            "E_INVALID"
        );
    }

    #[test]
    fn resolve_args_for_an_earlier_conversation_needs_no_current_id() {
        let dir = tempfile::tempdir().unwrap();
        let (store, mut row, _, pb) = two_conversations(dir.path(), 1, 2);
        row.claude_session_id = None;
        let a = resolve_args_for(&store, &row, CONV_B, 3, 500).unwrap();
        assert_eq!(a.claude_session_id, CONV_B);
        assert_eq!(a.transcript_path.as_deref(), Some(pb.as_str()));
    }

    #[test]
    fn resolve_args_for_finds_a_conversation_older_than_the_newest_500() {
        let dir = tempfile::tempdir().unwrap();
        let (store, row, _, pb) = two_conversations(dir.path(), 1, 2);
        {
            let s = store.lock().unwrap();
            // 500 conversations newer than B (A is already one of them).
            for i in 0..500i64 {
                s.conn_ref()
                    .execute(
                        "INSERT INTO conversations (session_id, claude_session_id, \
                             started_at, start_source, ended_at) \
                         VALUES (?1, ?2, ?3, 'clear', ?3)",
                        rusqlite::params![
                            row.id,
                            format!("550e8400-e29b-41d4-a716-{i:012}"),
                            4_000_000_000i64 + i
                        ],
                    )
                    .unwrap();
            }
            assert!(!s
                .list_conversations(row.id, 500)
                .unwrap()
                .iter()
                .any(|c| c.claude_session_id == CONV_B));
        }
        let a = resolve_args_for(&store, &row, CONV_B, 3, 500).unwrap();
        assert_eq!(a.transcript_path.as_deref(), Some(pb.as_str()));
    }

    #[tokio::test]
    async fn fetch_for_row_writes_the_context_back_only_for_the_current_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let (store, row, _, _) = two_conversations(dir.path(), 1_000, 2_000);
        let ssh = Arc::new(SshClient::new());
        let ctx = |store: &std::sync::Mutex<Store>| {
            let s = store.lock().unwrap();
            s.get_session_by_id(row.id)
                .unwrap()
                .unwrap()
                .context
                .context_tokens
        };
        let before = ctx(&store);
        let conv = fetch_conversation_for_row(
            &store,
            &ssh,
            &row,
            Some(CONV_B),
            10,
            8_000,
            CONV_EVENTS_LIMIT_UI,
        )
        .await
        .unwrap();
        assert_eq!(conv.context.as_ref().map(|c| c.tokens), Some(2_000));
        assert_eq!(ctx(&store), before, "an earlier conversation is read-only");
        let conv =
            fetch_conversation_for_row(&store, &ssh, &row, None, 10, 8_000, CONV_EVENTS_LIMIT_UI)
                .await
                .unwrap();
        assert_eq!(conv.context.as_ref().map(|c| c.tokens), Some(1_000));
        assert_eq!(ctx(&store), Some(1_000));
    }

    #[tokio::test]
    async fn fetch_for_row_attaches_only_the_requested_conversations_events() {
        let dir = tempfile::tempdir().unwrap();
        let (store, row, _, _) = two_conversations(dir.path(), 1_000, 2_000);
        {
            let s = store.lock().unwrap();
            s.insert_session_event_for(row.id, Some(CONV_A), "prompt_sent", Some("a1"))
                .unwrap();
            s.insert_session_event_for(row.id, Some(CONV_B), "prompt_sent", Some("b1"))
                .unwrap();
            s.insert_session_event_for(row.id, Some(CONV_A), "turn_ended", Some("a2"))
                .unwrap();
        }
        let ssh = Arc::new(SshClient::new());
        let conv = fetch_conversation_for_row(
            &store,
            &ssh,
            &row,
            Some(CONV_B),
            10,
            8_000,
            CONV_EVENTS_LIMIT_UI,
        )
        .await
        .unwrap();
        assert_eq!(
            conv.events
                .iter()
                .map(|e| e.detail.clone())
                .collect::<Vec<_>>(),
            vec![Some("b1".to_string())]
        );
        let conv = fetch_conversation_for_row(
            &store,
            &ssh,
            &row,
            Some(CONV_A),
            10,
            8_000,
            CONV_EVENTS_LIMIT_UI,
        )
        .await
        .unwrap();
        assert_eq!(
            conv.events
                .iter()
                .map(|e| e.detail.clone())
                .collect::<Vec<_>>(),
            vec![Some("a1".to_string()), Some("a2".to_string())],
            "oldest first, only this conversation's events"
        );
    }

    #[tokio::test]
    async fn fetch_for_row_caps_events_at_the_limit_keeping_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let (store, row, _, _) = two_conversations(dir.path(), 1_000, 2_000);
        {
            let s = store.lock().unwrap();
            for d in ["a1", "a2", "a3"] {
                s.insert_session_event_for(row.id, Some(CONV_A), "prompt_sent", Some(d))
                    .unwrap();
            }
        }
        let ssh = Arc::new(SshClient::new());
        let conv = fetch_conversation_for_row(&store, &ssh, &row, Some(CONV_A), 10, 8_000, 2)
            .await
            .unwrap();
        assert_eq!(
            conv.events
                .iter()
                .map(|e| e.detail.clone())
                .collect::<Vec<_>>(),
            vec![Some("a2".to_string()), Some("a3".to_string())]
        );
    }

    fn tail_args(max_chars: usize) -> TranscriptArgs {
        TranscriptArgs {
            host_alias: "mefistos".into(),
            tmux_name: None,
            transcript_path: None,
            cwd: Some("/w".into()),
            claude_session_id: "00000000-0000-0000-0000-00000000beef".into(),
            turns: CONV_TURNS,
            max_chars,
        }
    }

    #[test]
    fn conversation_reads_a_fixed_one_mib_tail() {
        // The Conversation panel polls every 5 s; a 4 MB tail per poll over
        // ssh is too heavy, so it reads a fixed 1 MiB regardless of budget.
        assert_eq!(CONV_READ_BYTES, 1_048_576);
        let script = conversation_read_script(&tail_args(CONV_MAX_CHARS)).unwrap();
        assert!(script.contains("tail -c 1048576 "), "{script}");
        let script = conversation_read_script(&tail_args(100)).unwrap();
        assert!(script.contains("tail -c 1048576 "), "{script}");
    }

    #[test]
    fn session_transcript_keeps_its_char_derived_read_budget() {
        let script = transcript_read_script(&tail_args(CONV_MAX_CHARS)).unwrap();
        let expected = format!("tail -c {} ", read_bytes_for(CONV_MAX_CHARS));
        assert!(script.contains(&expected), "{script}");
        assert_eq!(read_bytes_for(CONV_MAX_CHARS), 4_096_000);
    }

    #[test]
    fn read_scripts_validate_their_inputs() {
        let mut bad = tail_args(100);
        bad.claude_session_id = "../../etc".into();
        assert_eq!(
            conversation_read_script(&bad).unwrap_err().code,
            "E_INVALID"
        );
        assert_eq!(transcript_read_script(&bad).unwrap_err().code, "E_INVALID");
    }

    #[tokio::test]
    async fn fetch_conversation_maps_a_missing_local_transcript_to_e_no_transcript() {
        let ssh = Arc::new(SshClient::new());
        let dir = tempfile::tempdir().unwrap();
        let err = fetch_conversation(
            TranscriptArgs {
                host_alias: "local".into(),
                tmux_name: None,
                transcript_path: None,
                cwd: Some(dir.path().to_string_lossy().into_owned()),
                claude_session_id: "00000000-0000-0000-0000-00000000beef".into(),
                turns: CONV_TURNS,
                max_chars: CONV_MAX_CHARS,
            },
            &ssh,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_NO_TRANSCRIPT");
    }

    #[tokio::test]
    async fn fetch_rejects_an_invalid_session_id() {
        let ssh = Arc::new(SshClient::new());
        let err = fetch_transcript(
            TranscriptArgs {
                host_alias: "local".into(),
                tmux_name: None,
                transcript_path: None,
                cwd: Some("/tmp".into()),
                claude_session_id: "../../etc".into(),
                turns: 1,
                max_chars: 100,
            },
            &ssh,
        )
        .await
        .unwrap_err();
        assert!(err.code.starts_with("E_"), "{}", err.code);
        assert_ne!(err.code, "E_NO_TRANSCRIPT");
    }

    #[tokio::test]
    async fn fetch_maps_a_missing_local_transcript_to_e_no_transcript() {
        let ssh = Arc::new(SshClient::new());
        let dir = tempfile::tempdir().unwrap();
        let err = fetch_transcript(
            TranscriptArgs {
                host_alias: "local".into(),
                tmux_name: None,
                transcript_path: None,
                cwd: Some(dir.path().to_string_lossy().into_owned()),
                claude_session_id: "00000000-0000-0000-0000-00000000dead".into(),
                turns: 1,
                max_chars: 100,
            },
            &ssh,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_NO_TRANSCRIPT");
    }
}
