//! Nous Research's Hermes Agent CLI (`hermes`) behind [`AgentAdapter`].
//!
//! Built from the source of hermes-agent 0.19.0 (PyPI `hermes-agent`):
//! `hermes_cli/_parser.py` for the flags, `cli.py` for the classic
//! prompt_toolkit REPL a pane shows, `hermes_state.py` for where sessions
//! live. The pane fixtures in the tests are laid out from that render code
//! (`_get_approval_display_fragments`, `_get_clarify_display`,
//! `_build_status_bar_text`, `_get_tui_prompt_fragments`), not captured from
//! a running Hermes; replace them with `tmux capture-pane -p` output once a
//! fleet host runs one.
//!
//! * **Launch.** `hermes chat --cli --yolo`: `--cli` pins the classic REPL
//!   (a config's `display.interface: tui` would otherwise open the
//!   full-screen Ink TUI, which this reader does not know), `--yolo` skips
//!   dangerous-command approvals, as every Claude session runs with
//!   `--dangerously-skip-permissions`. `-m <model>` sets the model; Hermes
//!   takes no reasoning effort at launch (only `/reasoning` inside), so a
//!   stored effort is not passed. `--resume <id>` resumes a session. Its
//!   own `-c` continues the newest CLI session ANYWHERE and then changes
//!   into that session's directory, so a relaunch without an id looks the
//!   newest session for the pane's cwd up in `~/.hermes/state.db` instead
//!   ([`newest_in_cwd`]), and starts fresh when there is none.
//! * **Ids.** Hermes picks its own: `YYYYMMDD_HHMMSS_<6 hex>`, shown in the
//!   banner as `Session: <id>`. It cannot be told one up front, so a new
//!   session starts bare ([`AgentAdapter::start_command`]).
//! * **Pane.** The bottom of the screen is, top to bottom: an approval or
//!   question panel when one is open, the spinner line, the status bar
//!   (`⚕ model │ 12.4K/200K │ 6% │ …`), a `─` rule, the input line and
//!   another rule. The input line's prefix carries the state: `❯` idle,
//!   `⚕ ❯` working, a braille frame `❯` a slow slash command, `⚠ ❯` an
//!   approval, `? ❯` / `✎ ❯` a question, `🔐 ❯` / `🔑 ❯` a password.
//!   A digit answers an approval or a question at once (its key bindings
//!   call the selection handler directly), like Claude's dialogs, so the
//!   prompt card's keys work unchanged.
//! * **Transcript.** Sessions are rows of `~/.hermes/state.db` (SQLite),
//!   not a text log; nothing reads them yet, so the Conversation tab says
//!   so and the terminal tab shows the session live.

use super::{cmd, opt, AgentAdapter, LaunchSwitch, PickerOption, SlashCommand};
use crate::service::pane_intel::{self, ClaudeStatus, PaneIntel, PendingInput, PendingOption};
use crate::service::pane_intel::{StuckKind, WaitingFor};
use crate::service::transcript::ConvTurn;
use crate::tmux::ClaudeLaunch;

/// The Hermes Agent adapter.
#[derive(Debug, Default, Clone, Copy)]
pub struct Hermes;

/// A hint list: the head of Hermes' curated OpenRouter / Nous Portal list
/// (`hermes_cli/models.py`). Which slugs work depends on the provider the
/// host's Hermes is set up with; any value it takes is accepted.
const MODELS: &[PickerOption] = &[
    opt("anthropic/claude-opus-4.8", "Claude Opus 4.8"),
    opt("anthropic/claude-sonnet-5", "Claude Sonnet 5"),
    opt("openai/gpt-5.6-sol", "GPT-5.6-Sol"),
    opt("google/gemini-3.1-pro-preview", "Gemini 3.1 Pro"),
    opt("deepseek/deepseek-v4-pro", "DeepSeek V4 Pro"),
    opt("qwen/qwen3.7-max", "Qwen3.7 Max"),
];

/// None at launch: Hermes has no effort flag (`/reasoning` sets it inside).
const EFFORTS: &[PickerOption] = &[];

/// Hermes' commands worth offering from the composer
/// (`hermes_cli/commands.py` lists about ninety).
const SLASH: &[SlashCommand] = &[
    cmd("model", "Switch model for this session", true),
    cmd("reasoning", "Set reasoning effort and display", true),
    cmd("new", "Start a new session", false),
    cmd("title", "Set a title for the current session", true),
    cmd("compress", "Compress conversation context", false),
    cmd(
        "status",
        "Show session, model, token and context info",
        false,
    ),
    cmd("usage", "Show token usage and rate limits", false),
    cmd("context", "Show what fills the context window", false),
    cmd("retry", "Resend the last message", false),
    cmd("undo", "Back up one user turn", false),
    cmd("branch", "Branch the current session", false),
    cmd("queue", "Queue a prompt for the next turn", true),
    cmd("steer", "Steer after the next tool call", true),
    cmd("goal", "Set a standing goal for this session", true),
    cmd("background", "Run a prompt in the background", true),
    cmd("agents", "Show active agents and running tasks", false),
    cmd("stop", "Kill all running background processes", false),
    cmd("rollback", "List or restore filesystem checkpoints", false),
    cmd("skills", "Search, install or manage skills", true),
    cmd("tools", "List, enable or disable tools", true),
    cmd("yolo", "Toggle skipping dangerous-command approvals", false),
    cmd("help", "Show available commands", false),
    cmd("quit", "Quit Hermes (the tmux session stays)", false),
];

/// What a pane falls back to when Hermes exits, as for Claude.
const TAIL: &str = super::AGENT_EXIT_TAIL;

/// The interactive REPL every launch runs; see the module docs.
const BASE: &str = "hermes chat --cli --yolo";

/// Prints the id of the newest CLI session whose cwd is the shell's, from
/// Hermes' own database, read-only; prints nothing when there is none or the
/// database cannot be read. Python's standard library only: Hermes is a
/// Python program, so `python3` is there wherever it is.
const NEWEST_IN_CWD_PY: &str = "import os,sqlite3; \
h=os.environ.get(\"HERMES_HOME\") or os.path.expanduser(\"~/.hermes\"); \
c=sqlite3.connect(\"file:\"+os.path.join(h,\"state.db\")+\"?mode=ro\",uri=True); \
d=os.getcwd(); \
r=c.execute(\"SELECT id FROM sessions WHERE source=? AND cwd IN (?,?) ORDER BY started_at DESC LIMIT 1\",(\"cli\",d,os.path.realpath(d))).fetchone(); \
print(r[0] if r else \"\")";

/// `id=$(python3 -c '…' 2>/dev/null)`: the cwd's newest Hermes session into
/// `$id`, empty when none ([`NEWEST_IN_CWD_PY`]).
fn newest_in_cwd() -> String {
    format!(
        "id=$(python3 -c {} 2>/dev/null)",
        crate::shell::quote(NEWEST_IN_CWD_PY)
    )
}

/// ` -m 'm'` when a model is set (`default` means Hermes' configured one).
fn flags(launch: &ClaudeLaunch) -> String {
    match launch.model.as_deref().filter(|m| *m != "default") {
        Some(m) => format!(" -m {}", crate::shell::quote(m)),
        None => String::new(),
    }
}

impl AgentAdapter for Hermes {
    fn id(&self) -> &'static str {
        crate::store::AGENT_HERMES
    }

    fn label(&self) -> &'static str {
        "Hermes"
    }

    /// With an id: `--resume` it, or a fresh session when Hermes no longer
    /// has it. Without one: resume the cwd's newest CLI session when there
    /// is one, else start fresh. `launch.profile` is a Claude credential
    /// profile and does not apply (Hermes keeps its provider login in
    /// `~/.hermes`); `tmux_name` is unused, as Hermes takes no session name
    /// at launch.
    fn launch_command(
        &self,
        conversation_id: Option<&str>,
        _tmux_name: &str,
        launch: &ClaudeLaunch,
    ) -> String {
        let f = flags(launch);
        let path = crate::tmux::VOICE_PATH_PREFIX;
        match conversation_id {
            Some(id) => format!(
                "{path}{BASE} --resume {}{f} 2>/dev/null || {BASE}{f}; {TAIL}",
                crate::shell::quote(id)
            ),
            None => format!(
                "{path}{}; if [ -n \"$id\" ]; then {BASE} --resume \"$id\"{f} || {BASE}{f}; \
                 else {BASE}{f}; fi; {TAIL}",
                newest_in_cwd()
            ),
        }
    }

    /// `20260225_143052_a1b2c3`: the shape `cli.py` gives every new
    /// session, and nothing a shell would read as more than a word.
    fn valid_conversation_id(&self, id: &str) -> bool {
        let b = id.as_bytes();
        b.len() == 22
            && b[8] == b'_'
            && b[15] == b'_'
            && b[..8].iter().all(u8::is_ascii_digit)
            && b[9..15].iter().all(u8::is_ascii_digit)
            && b[16..]
                .iter()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
    }

    /// None: Hermes allocates its own ids and has no flag to start under
    /// one fleet chose, as Codex.
    fn mint_conversation_id(&self) -> Option<String> {
        None
    }

    /// A fresh session: never the cwd's newest.
    fn start_command(&self, _tmux_name: &str, launch: &ClaudeLaunch) -> String {
        format!(
            "{}{BASE}{}; {TAIL}",
            crate::tmux::VOICE_PATH_PREFIX,
            flags(launch)
        )
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

    /// `/model <name>` switches this session's model (session-scoped unless
    /// `--global`), so a relaunch keeps it. `/model` alone opens a picker and
    /// a name with `--provider` or `--global` is Hermes' own business.
    fn launch_switch<'a>(&self, prompt: &'a str) -> Option<LaunchSwitch<'a>> {
        let mut words = prompt.split_whitespace();
        if words.next() != Some("/model") {
            return None;
        }
        let model = words.next()?;
        if words.next().is_some() || model.starts_with('-') {
            return None;
        }
        crate::validate::claude_model(model).ok()?;
        Some(LaunchSwitch::Model(Some(model)))
    }

    fn analyze_pane(&self, pane_tail: &str) -> PaneIntel {
        analyze(pane_tail)
    }

    fn spinner_line(&self, pane_tail: &str) -> Option<String> {
        let text = pane_intel::strip_ansi(pane_tail);
        let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
        let screen = Screen::read(&lines)?;
        (screen.state == State::Working)
            .then(|| screen.spinner.map(str::to_string))
            .flatten()
    }

    /// Nothing yet: Hermes keeps its sessions in SQLite (`state.db`).
    fn parse_transcript(&self, _transcript: &str) -> Vec<ConvTurn> {
        Vec::new()
    }
}

// ---------------------------------------------------------------- pane ----

/// The last lines a reading looks at: the panels and the input line are
/// always at the bottom of the pane, and older text above is scrollback.
const BOTTOM_LINES: usize = 40;

/// What the input line's prefix says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Working,
    Approval,
    Question,
    Password,
}

/// The bottom of a Hermes screen, read off its input line.
struct Screen<'a> {
    state: State,
    /// Index of the input line in the lines read.
    input: usize,
    /// Index of the status bar (`⚕ model │ …`), when shown.
    status: Option<usize>,
    /// The spinner line's text, trimmed, when one is drawn above the
    /// status bar.
    spinner: Option<&'a str>,
}

/// A horizontal rule of `─`, as drawn above and below the input line.
fn is_rule(line: &str) -> bool {
    let l = line.trim();
    !l.is_empty() && l.chars().all(|c| c == '─')
}

/// A panel's own line: its border or a boxed row.
fn is_panel(line: &str) -> bool {
    let l = line.trim_start();
    l.starts_with(['│', '╭', '╰'])
}

/// The input line's state from what precedes its `❯`: nothing (idle) or a
/// profile name (`coder ❯`, also idle), else a state icon. `None` for a
/// line that is not an input line.
fn input_state(line: &str) -> Option<State> {
    let l = line.trim_start();
    if is_panel(l) {
        return None;
    }
    let at = l.find('❯')?;
    let head = l[..at].trim();
    if head.is_empty() {
        return Some(State::Idle);
    }
    let mut chars = head.chars();
    let first = chars.next()?;
    let one_glyph = chars.as_str().trim_start_matches('\u{fe0f}').is_empty();
    let state = match first {
        '⚕' if one_glyph => State::Working,
        '\u{2801}'..='\u{28ff}' if one_glyph => State::Working,
        '⚠' if one_glyph => State::Approval,
        '?' | '✎' if one_glyph => State::Question,
        '🔐' | '🔑' if one_glyph => State::Password,
        // A profile's prompt (`coder ❯`): one plain word.
        _ if head
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_')) =>
        {
            State::Idle
        }
        _ => return None,
    };
    Some(state)
}

/// The status bar: `⚕ <model>` then ` │ ` / ` · ` separated fields.
fn is_status_bar(line: &str) -> bool {
    let l = line.trim();
    l.starts_with("⚕ ") && (l.contains(" │ ") || l.contains(" · ") || l.len() < 60)
}

impl<'a> Screen<'a> {
    /// The input line is the last line read as one, and Hermes draws a `─`
    /// rule right above it; `None` when the bottom of the pane is not
    /// Hermes' REPL (it exited and a shell prompt took over, or a setup
    /// screen is up).
    fn read(lines: &[&'a str]) -> Option<Self> {
        let input = lines.iter().rposition(|l| input_state(l).is_some())?;
        let above = lines[..input].iter().rposition(|l| !l.trim().is_empty())?;
        if !is_rule(lines[above]) || lines[input + 1..].iter().any(|l| is_status_bar(l)) {
            return None;
        }
        let state = input_state(lines[input])?;
        // Above the rule: (when shown) the status bar.
        let mut i = above;
        let mut status = None;
        while i > 0 {
            i -= 1;
            let l = lines[i];
            if l.trim().is_empty() || is_rule(l) {
                continue;
            }
            if is_status_bar(l) {
                status = Some(i);
            }
            break;
        }
        // Above the status bar: a blank spacer, then the spinner line.
        let spinner = status.and_then(|s| {
            lines[..s]
                .iter()
                .rev()
                .copied()
                .find(|l| !l.trim().is_empty())
                .filter(|l| l.starts_with("  ") && !is_panel(l) && !is_rule(l))
                .map(|l| l.trim())
        });
        Some(Screen {
            state,
            input,
            status,
            spinner,
        })
    }
}

/// An open approval or question panel.
struct Dialog {
    kind: WaitingFor,
    question: Option<String>,
    options: Vec<PendingOption>,
    detail: Option<String>,
}

/// A boxed row's content: `│ text   │` → `text`.
fn boxed(line: &str) -> &str {
    let l = line.trim();
    let l = l.strip_prefix('│').unwrap_or(l);
    let l = l.strip_suffix('│').unwrap_or(l);
    l.trim()
}

/// A numbered choice inside a panel: `❯ 1. Allow once`, `  2. Deny`, or a
/// slash command's confirm `❯ [1] Yes — run it`.
fn choice(row: &str) -> Option<(u8, String, bool)> {
    if let Some((n, label, selected)) = pane_intel::parse_choice(row) {
        return Some((n, label.to_string(), selected));
    }
    let rest = row.trim_start_matches('❯');
    let selected = rest.len() != row.len();
    let rest = rest.trim_start().strip_prefix('[')?;
    let close = rest.find(']')?;
    let n: u8 = rest[..close].parse().ok()?;
    let label = rest[close + 1..].trim();
    let label = label.split(" — ").next().unwrap_or(label).trim();
    Some((n, label.to_string(), selected))
}

/// The panel right above the spinner/status rows: its rows from the `╭`
/// border down to the `╰` one.
fn panel<'a>(lines: &[&'a str], above: usize) -> Option<Vec<&'a str>> {
    let bottom = lines[..above]
        .iter()
        .rposition(|l| l.trim_start().starts_with('╰'))?;
    // Only blank, spinner or rule lines may sit between the panel and the
    // status bar / input line; anything else means the panel is scrollback.
    if lines[bottom + 1..above]
        .iter()
        .any(|l| is_panel(l) || l.contains('❯'))
    {
        return None;
    }
    let top = lines[..bottom]
        .iter()
        .rposition(|l| l.trim_start().starts_with('╭'))?;
    Some(lines[top..=bottom].to_vec())
}

fn dialog(lines: &[&str], screen: &Screen) -> Option<Dialog> {
    let kind = match screen.state {
        State::Approval => WaitingFor::Permission,
        State::Question => WaitingFor::Input,
        _ => return None,
    };
    let rows = panel(lines, screen.status.unwrap_or(screen.input))?;
    // `╭─ Hermes needs your input ─╮` carries the question panel's title in
    // its border; the approval panel's title is its first boxed row.
    let body: Vec<&str> = rows[1..rows.len().saturating_sub(1)]
        .iter()
        .map(|l| boxed(l))
        .collect();
    let mut options = Vec::new();
    let mut first_choice = None;
    for (i, row) in body.iter().enumerate() {
        if let Some((n, label, selected)) = choice(row) {
            first_choice.get_or_insert(i);
            options.push(PendingOption {
                n,
                label: label.chars().take(pane_intel::PENDING_LABEL_MAX).collect(),
                selected,
                checked: false,
                persists: pane_intel::persists_label(&label),
            });
        }
    }
    if options.is_empty() {
        return None;
    }
    options.truncate(pane_intel::PENDING_OPTIONS_MAX);
    let first_choice = first_choice.unwrap_or(0);
    let text_rows = |rows: &[&str]| -> Option<String> {
        let s = rows
            .iter()
            .map(|r| r.trim())
            .filter(|r| !r.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        (!s.is_empty()).then_some(s)
    };
    let cap = |s: String, n: usize| s.chars().take(n).collect::<String>();
    let (question, detail) = if kind == WaitingFor::Permission {
        // Title, then the command, then the choices, then the description
        // (why Hermes flagged it). A slash command's confirm has its title
        // and a detail above the choices instead.
        let title = body
            .first()
            .map(|t| t.trim_start_matches(['⚠', '\u{fe0f}']).trim());
        let above = text_rows(body.get(1..first_choice).unwrap_or_default());
        let below = text_rows(
            &body[first_choice..]
                .iter()
                .copied()
                .filter(|r| choice(r).is_none() && !r.starts_with("Type 1/"))
                .collect::<Vec<_>>(),
        );
        match title {
            Some("Dangerous Command") => (
                Some(below.unwrap_or_else(|| "Run a dangerous command?".into())),
                above,
            ),
            _ => (title.filter(|t| !t.is_empty()).map(str::to_string), above),
        }
    } else {
        (
            text_rows(body.get(..first_choice).unwrap_or_default()),
            None,
        )
    };
    Some(Dialog {
        kind,
        question: question.map(|q| cap(q, pane_intel::PENDING_QUESTION_MAX)),
        options,
        detail: detail.map(|d| cap(d, pane_intel::PENDING_DETAIL_MAX)),
    })
}

/// `6%` (or `~6%`, an estimate) in the status bar → 6.
fn context_pct(status: &str) -> Option<f64> {
    status
        .split(['│', '·'])
        .map(|f| f.trim().trim_start_matches('~'))
        .find_map(|f| f.strip_suffix('%')?.parse::<f64>().ok())
        .filter(|p| (0.0..=100.0).contains(p))
}

/// Hermes' first-run guard ("isn't configured yet … Run setup now?") or
/// its setup wizard: nothing runs until someone signs in to a provider.
fn needs_setup(lines: &[&str]) -> bool {
    lines.iter().any(|l| {
        let l = l.to_lowercase();
        l.contains("hermes isn't configured yet") || l.contains("run setup now?")
    })
}

/// Read a Hermes pane tail; the fields mean what [`pane_intel::analyze`]'s do.
fn analyze(pane_tail: &str) -> PaneIntel {
    let text = pane_intel::strip_ansi(pane_tail);
    let all: Vec<&str> = text.lines().map(str::trim_end).collect();
    let lines = &all[all.len().saturating_sub(BOTTOM_LINES)..];
    let Some(screen) = Screen::read(lines) else {
        let stuck = needs_setup(lines).then_some(StuckKind::AuthMenu);
        return PaneIntel {
            activity: None,
            stuck,
            context_pct: None,
            derived_status: stuck.map(|_| ClaudeStatus::Blocked),
            waiting_for: None,
            pending_input: None,
        };
    };
    let dialog = dialog(lines, &screen);
    let derived_status = match screen.state {
        State::Idle => ClaudeStatus::Idle,
        State::Working => ClaudeStatus::Working,
        State::Approval | State::Question | State::Password => ClaudeStatus::Blocked,
    };
    let activity = match (&dialog, screen.state) {
        (Some(d), _) => Some(match &d.question {
            Some(q) => format!("waiting for {}: {q}", d.kind.as_str()),
            None => format!("waiting for {}", d.kind.as_str()),
        }),
        (None, State::Password) => Some("waiting for a password".to_string()),
        (None, State::Working) => screen.spinner.map(str::to_string),
        _ => None,
    }
    .map(|s| s.chars().take(pane_intel::ACTIVITY_MAX).collect());
    PaneIntel {
        activity,
        stuck: None,
        context_pct: screen.status.and_then(|s| context_pct(lines[s])),
        derived_status: Some(derived_status),
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

#[cfg(test)]
mod tests;
