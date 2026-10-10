//! Pure parsers for a captured pane tail.
//!
//! `reconcile_sessions` reads the last few lines of each work session's tmux
//! pane (`tmux capture-pane -p -S -8`) and feeds them to [`analyze`]. The result
//! drives four session fields: `current_activity`, a derived `claude_status`, a
//! `stuck_kind`, and `context_pct`. Everything here is side-effect-free and
//! heavily unit-tested; the reconcile wiring that calls it is intentionally thin.
//!
//! All parsers return `None` rather than guess. A misread pane that silently
//! produced a wrong `blocked` status or a bogus context % would be worse than no
//! signal at all, since Wave-2 self-heal will eventually act on these.

/// Cap on the stored activity string so a runaway pane line can't bloat a row.
pub(crate) const ACTIVITY_MAX: usize = 200;

/// Stuck states detectable from the pane tail. Detection only — auto-remedy
/// keystrokes are a deliberately-deferred follow-up (see plan self-review).
///
/// This enum is the single source of truth for the `stuck_kind` vocabulary:
/// the DB column, the MCP `list_sessions` / `peer_status` output, the server
/// instructions and the control skill all quote [`StuckKind::vocabulary_doc`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StuckKind {
    /// Claude is showing an account/login selection menu.
    AuthMenu,
    /// The transport dropped and the REPL is reconnecting.
    Reconnect,
    /// A "do you trust the files in this folder?" prompt is blocking.
    TrustPrompt,
    /// The process hit an out-of-memory / heap-allocation failure.
    Oom,
    /// A "Press Enter to continue" style prompt is waiting on a keystroke.
    PressEnter,
}

impl StuckKind {
    /// Every value, in documentation order.
    pub const ALL: &'static [StuckKind] = &[
        StuckKind::AuthMenu,
        StuckKind::Reconnect,
        StuckKind::TrustPrompt,
        StuckKind::Oom,
        StuckKind::PressEnter,
    ];

    /// Stable lowercase tag stored in `sessions.stuck_kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            StuckKind::AuthMenu => "auth_menu",
            StuckKind::Reconnect => "reconnect",
            StuckKind::TrustPrompt => "trust_prompt",
            StuckKind::Oom => "oom",
            StuckKind::PressEnter => "press_enter",
        }
    }

    /// The value list rendered as `a | b | c`, for quoting verbatim in docs.
    pub fn vocabulary_doc() -> String {
        join_vocabulary(Self::ALL.iter().map(|k| k.as_str()))
    }
}

impl std::fmt::Display for StuckKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for StuckKind {
    type Err = UnknownValue;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| UnknownValue {
                field: "stuck_kind",
                value: s.to_string(),
            })
    }
}

/// Coarse Claude REPL status stored in `sessions.claude_status`.
///
/// Authoritative values come from `claude agents --json` (`status` field);
/// the pane-tail fallback in [`analyze`] only ever derives `working`, `idle`
/// or `blocked`, and the Stop hook (`service::hooks`) stamps `idle`. This enum
/// is the single source of truth for the vocabulary: every doc string and the
/// control skill quote [`ClaudeStatus::vocabulary_doc`] verbatim, and a test
/// fails if they drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeStatus {
    /// Actively generating / running tools.
    Working,
    /// Waiting on user input (permission prompt, question, stuck state).
    Blocked,
    /// The agent finished its task (background sessions).
    Completed,
    /// The agent exited with an error (background sessions).
    Failed,
    /// The process was stopped by a hook or the user.
    Stopped,
    /// Turn over; the REPL is showing its input prompt.
    Idle,
}

impl ClaudeStatus {
    /// Every value, in documentation order.
    pub const ALL: &'static [ClaudeStatus] = &[
        ClaudeStatus::Working,
        ClaudeStatus::Blocked,
        ClaudeStatus::Completed,
        ClaudeStatus::Failed,
        ClaudeStatus::Stopped,
        ClaudeStatus::Idle,
    ];

    /// Stable lowercase tag stored in `sessions.claude_status`.
    pub fn as_str(self) -> &'static str {
        match self {
            ClaudeStatus::Working => "working",
            ClaudeStatus::Blocked => "blocked",
            ClaudeStatus::Completed => "completed",
            ClaudeStatus::Failed => "failed",
            ClaudeStatus::Stopped => "stopped",
            ClaudeStatus::Idle => "idle",
        }
    }

    /// Whether the session is between turns: nothing is generating and
    /// nothing inside a turn is waiting on the user. `Blocked` is NOT quiet —
    /// a permission prompt or a question is part of the turn it interrupts.
    /// The same set as the frontend's `isQuietStatus` (`conversation.ts`), held
    /// there by the shared fixture `testdata/quiet_statuses.json`; `store::turn_over`
    /// is defined through it.
    pub fn is_quiet(self) -> bool {
        match self {
            ClaudeStatus::Idle
            | ClaudeStatus::Completed
            | ClaudeStatus::Stopped
            | ClaudeStatus::Failed => true,
            ClaudeStatus::Working | ClaudeStatus::Blocked => false,
        }
    }

    /// The value list rendered as `a | b | c`, for quoting verbatim in docs.
    pub fn vocabulary_doc() -> String {
        join_vocabulary(Self::ALL.iter().map(|k| k.as_str()))
    }
}

impl std::fmt::Display for ClaudeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ClaudeStatus {
    type Err = UnknownValue;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| UnknownValue {
                field: "claude_status",
                value: s.to_string(),
            })
    }
}

/// Error for a string that is not in a status vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownValue {
    pub field: &'static str,
    pub value: String,
}

impl std::fmt::Display for UnknownValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown {} value: {:?}", self.field, self.value)
    }
}

impl std::error::Error for UnknownValue {}

fn join_vocabulary<'a>(values: impl Iterator<Item = &'a str>) -> String {
    values.collect::<Vec<_>>().join(" | ")
}

/// Everything we can infer from one pane tail.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneIntel {
    /// Last meaningful non-empty line, ANSI-stripped and length-capped.
    pub activity: Option<String>,
    /// Detected stuck state, if any.
    pub stuck: Option<StuckKind>,
    /// Context usage 0..100 derived from the REPL footer, if present.
    pub context_pct: Option<f64>,
    /// Inferred status: `Working` | `Idle` | `Blocked` | `None`.
    /// Only a *fallback* — the authoritative status comes from `claude agents`.
    pub derived_status: Option<ClaudeStatus>,
    /// What a `Blocked` pane waits on when the cause is a permission or
    /// question dialog (not a stuck state). No session column holds it yet,
    /// so [`analyze`] also puts it in `activity` as
    /// `waiting for <reason>: <question>`.
    pub waiting_for: Option<WaitingFor>,
    /// The dialog's question and numbered choices, stored on the session row
    /// (`sessions.pending_input`) so a client can turn them into buttons.
    /// `None` whenever the pane shows no permission/question dialog.
    pub pending_input: Option<PendingInput>,
}

/// One numbered choice of a permission or question dialog.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingOption {
    pub n: u8,
    /// The choice's text. On a multi-select question the `[ ]` / `[✔]`
    /// checkbox is not part of it: that is [`checked`](Self::checked), so
    /// ticking a box does not make the same question read as a new one.
    pub label: String,
    /// Carries the `❯` cursor (the key Enter would act on).
    pub selected: bool,
    /// Ticked, on a multi-select question ([`PendingInput::multi`]). Left off
    /// the wire when false, so a single-select dialog reads as it always has.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub checked: bool,
}

/// The permission/question dialog a blocked pane is showing, as stored on
/// `sessions.pending_input`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingInput {
    /// `"permission"` | `"input"` (mirrors [`WaitingFor::as_str`]).
    pub kind: String,
    pub question: Option<String>,
    pub options: Vec<PendingOption>,
    /// A multi-select question (AskUserQuestion with `multiSelect`): a digit
    /// TOGGLES that option's checkbox instead of answering, so no keystroke a
    /// single-select card sends ever finishes it. `Tab` keeps the ticks and
    /// moves on, to the next question or to the "Review your answers" step,
    /// whose `1. Submit answers` is an ordinary single-select dialog. Left off
    /// the wire when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multi: bool,
    /// What a permission dialog asks to run, as the tool-call line above it
    /// draws it (`Bash(git push -u origin main)`), so the approval card can
    /// show the exact command beside the answers. Left off the wire when the
    /// pane shows none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Why a pane showing a Claude Code dialog is waiting on the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitingFor {
    /// A tool-permission dialog ("Do you want to proceed?", "No, and tell
    /// Claude what to do differently"), including plan approval.
    Permission,
    /// A question with numbered answers (AskUserQuestion-style menu).
    Input,
}

impl WaitingFor {
    /// Stable lowercase tag; `input` matches the CLI's "input needed".
    pub fn as_str(self) -> &'static str {
        match self {
            WaitingFor::Permission => "permission",
            WaitingFor::Input => "input",
        }
    }
}

/// Strip ANSI/VT escape sequences (CSI `ESC[…m`, OSC, and bare control chars)
/// so pattern matching and the stored activity line see plain text.
pub(crate) fn strip_ansi(s: &str) -> String {
    let bytes: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == '\u{1b}' {
            // ESC. Look at the next char to decide what kind of sequence.
            match bytes.get(i + 1) {
                Some('[') => {
                    // CSI: ESC [ … final-byte in @..~ (0x40..=0x7e).
                    i += 2;
                    while i < bytes.len() {
                        let p = bytes[i];
                        i += 1;
                        if ('\u{40}'..='\u{7e}').contains(&p) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC: ESC ] … terminated by BEL (0x07) or ST (ESC \).
                    i += 2;
                    while i < bytes.len() {
                        let p = bytes[i];
                        if p == '\u{07}' {
                            i += 1;
                            break;
                        }
                        if p == '\u{1b}' && bytes.get(i + 1) == Some(&'\\') {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {
                    // Lone ESC or a 2-char sequence (ESC X). Drop ESC + next.
                    i += 2;
                }
            }
        } else if c == '\r' {
            // Carriage returns clutter capture output; drop them.
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Parse a context-usage percentage out of the REPL footer.
///
/// LIVE-CONFIRMED (2026-05-23, `tmux capture-pane -p` against a real Claude
/// session): the current footer shape is
///   `[████░░░] 9% used  |  Opus 4.7 (1M context)  |  /Users/...`
/// i.e. an "N% used" figure that IS the percent consumed — stored directly.
///
/// We ALSO tolerate the older/spec wording in case it returns in a future build
///   `Context left until auto-compact: 17%`
/// which is a percent *remaining*, so `context_pct = 100 - 17 = 83`.
///
/// A bare token figure (`561.8k tokens`, `93386 tokens`) carries no percentage
/// without the context-window size, so we return `None` rather than guess.
fn parse_context_pct(text: &str) -> Option<f64> {
    let lower = text.to_lowercase();
    for line in lower.lines() {
        // Shape A (live-confirmed): "... N% used ..."
        if let Some(pct) = find_pct_before_keyword(line, "used") {
            return Some(pct.clamp(0.0, 100.0));
        }
        // Shape B (spec wording): "... left ... N%" → percent remaining.
        if line.contains("left") && (line.contains("compact") || line.contains("context")) {
            if let Some(remaining) = find_any_pct(line) {
                return Some((100.0 - remaining).clamp(0.0, 100.0));
            }
        }
    }
    None
}

/// Find a `N%` that appears immediately before `keyword` on the line, e.g.
/// the `9` in `9% used`. Tolerant of decimals and surrounding whitespace.
fn find_pct_before_keyword(line: &str, keyword: &str) -> Option<f64> {
    let kw_pos = line.find(keyword)?;
    // The percent sign must sit between the start of line and the keyword.
    let before = &line[..kw_pos];
    let pct_pos = before.rfind('%')?;
    parse_trailing_number(&before[..pct_pos])
}

/// Find the first `N%` anywhere on the line.
fn find_any_pct(line: &str) -> Option<f64> {
    let pct_pos = line.find('%')?;
    parse_trailing_number(&line[..pct_pos])
}

/// Read a (possibly decimal) number off the END of `s`, ignoring trailing
/// whitespace. Returns None if the tail isn't numeric.
fn parse_trailing_number(s: &str) -> Option<f64> {
    let trimmed = s.trim_end();
    // Slice by the length of the non-numeric prefix, never by `rfind + 1`:
    // that is a byte index and lands inside a multibyte char such as `≈`
    // or `—` right before the digits, which panics.
    let prefix = trimmed.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    let num = &trimmed[prefix.len()..];
    if num.is_empty() {
        return None;
    }
    num.parse::<f64>().ok()
}

/// How far up the tail the OOM rule looks. The reconcile capture is 8 lines
/// (`PANE_TAIL_LINES`); `session_activity` reads more, and a crash block
/// older than a screen is history, not the state of the pane.
const OOM_TAIL_LINES: usize = 12;

/// Node's fatal heap block: the process that printed it is dead.
static OOM_HEAP: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"fatal error: reached heap limit|javascript heap out of memory|<--- last few gcs --->|allocation failed - javascript heap",
    )
    .expect("OOM_HEAP is a valid regex")
});

/// A kill verdict from the kernel, a container runtime or the shell. On its
/// own it is scrollback; followed by a shell prompt it is the foreground
/// process gone.
static OOM_KILLED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"oomkilled|out of memory: killed process \d+|killed process \d+.*\b(oom|out of memory)\b|^\s*(zsh: )?killed\b|\bsigkill\b",
    )
    .expect("OOM_KILLED is a valid regex")
});

/// A shell prompt line: `me@host:~/proj$ `, `$ `, `% `. The shell is back,
/// so whatever ran in the foreground is gone.
static SHELL_PROMPT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^(?:[\w.-]+@[\w.-]+[: ].*)?[$#%]\s*$")
        .expect("SHELL_PROMPT is a valid regex")
});

/// An OOM signal on one lower-cased line: the heap block or a kill verdict.
/// Prose (`out of memory`, `cannot allocate memory`, the bare acronym) is
/// deliberately NOT one: the fleet's own source, docs and MCP instructions
/// carry those words, and a session reading them was recreated twice (F1).
fn is_oom_signal(lower: &str) -> bool {
    OOM_HEAP.is_match(lower) || OOM_KILLED.is_match(lower)
}

/// One lower-cased line of a live Claude REPL's chrome: the input prompt
/// (`❯ …`, not a numbered dialog choice), the spinner / shortcut hint, the
/// mode footer or the status line. Any of these below a crash signal means
/// Claude outlived it.
fn is_live_repl_line(lower: &str) -> bool {
    let l = lower.trim_start();
    (l.starts_with('❯') && parse_choice(l).is_none())
        || LIVE_REPL_CUES.iter().any(|c| l.contains(c))
        || l.contains("bypass permissions")
        || l.contains("shift+tab to cycle")
        || l.contains("% used")
}

/// Detect a stuck state from the (already ANSI-stripped) tail. First match wins,
/// ordered most-specific first.
pub(crate) fn detect_stuck(text: &str) -> Option<StuckKind> {
    let lower = text.to_lowercase();

    // OOM: Claude died. Only the LAST signal within the tail window counts,
    // only when no live REPL chrome is drawn below it (an input box or
    // footer under the text means Claude outlived it), and only when the
    // signal is Node's own heap block or a kill verdict with the shell's
    // prompt back underneath — a process-level fact, not a word.
    let lines: Vec<&str> = lower.lines().collect();
    let tail = &lines[lines.len().saturating_sub(OOM_TAIL_LINES)..];
    if let Some(at) = tail.iter().rposition(|l| is_oom_signal(l)) {
        let below = &tail[at + 1..];
        let alive = below.iter().any(|l| is_live_repl_line(l));
        let heap = OOM_HEAP.is_match(tail[at]);
        let gone = below.iter().any(|l| SHELL_PROMPT.is_match(l.trim_end()));
        if !alive && (heap || gone) {
            return Some(StuckKind::Oom);
        }
    }

    // Trust-folder prompt.
    if (lower.contains("do you trust") && lower.contains("files"))
        || lower.contains("trust the files in this folder")
        || lower.contains("trust this folder")
    {
        return Some(StuckKind::TrustPrompt);
    }

    // Auth/login selection menu.
    if lower.contains("select login method")
        || lower.contains("choose an account")
        || lower.contains("select an account")
        || lower.contains("log in to claude")
        || (lower.contains("login") && lower.contains("subscription"))
    {
        return Some(StuckKind::AuthMenu);
    }

    // Transport reconnecting.
    if lower.contains("reconnecting") || lower.contains("connection lost, retrying") {
        return Some(StuckKind::Reconnect);
    }

    // Generic "press enter to continue" wait.
    if lower.contains("press enter to continue") || lower.contains("press enter to retry") {
        return Some(StuckKind::PressEnter);
    }

    None
}

/// Pick the last non-empty, non-pure-decoration line as the activity summary.
fn pick_activity(stripped: &str) -> Option<String> {
    for raw in stripped.lines().rev() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        // Skip lines that are only box-drawing / separators with no content.
        if line.chars().all(is_decoration) {
            continue;
        }
        let mut s = line.to_string();
        if s.chars().count() > ACTIVITY_MAX {
            s = s.chars().take(ACTIVITY_MAX).collect();
        }
        return Some(s);
    }
    None
}

fn is_decoration(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '─' | '│'
                | '┌'
                | '┐'
                | '└'
                | '┘'
                | '├'
                | '┤'
                | '┬'
                | '┴'
                | '┼'
                | '╭'
                | '╮'
                | '╰'
                | '╯'
                | '═'
                | '║'
                | '█'
                | '░'
                | '▁'
                | '▔'
                | '-'
                | '_'
                | '='
        )
}

/// Cues that belong to the live input box or the spinner, so seeing one
/// BELOW dialog-looking text means that text is scrollback (Claude's prose,
/// a diff), not a dialog on screen. The input prompt line (`❯ …`) counts too,
/// see [`detect_dialog`].
///
/// Deliberately NOT here: the status line (`% used`) and the mode line
/// (`bypass permissions`, `shift+tab to cycle`). A live capture of a
/// permission dialog (Claude Code 2.1.267) shows the dialog replacing the
/// input box, but a custom status line can still be painted underneath, and
/// a dialog with a status line under it is still a dialog.
const LIVE_REPL_CUES: &[&str] = &["esc to interrupt", "? for shortcuts"];

/// A permission or question dialog seen on screen.
struct Dialog {
    kind: WaitingFor,
    /// The dialog's question line, or the selected answer when no line of it
    /// ends with `?`.
    prompt: Option<String>,
    /// The dialog's numbered choices, in on-screen order.
    options: Vec<PendingOption>,
    /// A multi-select question: see [`PendingInput::multi`].
    multi: bool,
    /// See [`PendingInput::detail`].
    detail: Option<String>,
}

impl Dialog {
    /// The activity line recorded for a blocked pane.
    fn activity(&self) -> String {
        let s = match &self.prompt {
            Some(p) => format!("waiting for {}: {p}", self.kind.as_str()),
            None => format!("waiting for {}", self.kind.as_str()),
        };
        s.chars().take(ACTIVITY_MAX).collect()
    }

    /// What gets stored on `sessions.pending_input`. Capped by construction
    /// ([`PENDING_QUESTION_MAX`] / [`PENDING_LABEL_MAX`] / [`PENDING_OPTIONS_MAX`])
    /// so a pane a client turns straight into buttons can never blow up the
    /// row or the wire, however malformed the captured text.
    fn pending_input(&self) -> PendingInput {
        PendingInput {
            kind: self.kind.as_str().into(),
            question: self
                .prompt
                .as_deref()
                .map(|q| q.chars().take(PENDING_QUESTION_MAX).collect()),
            options: self
                .options
                .iter()
                .take(PENDING_OPTIONS_MAX)
                .map(|o| PendingOption {
                    n: o.n,
                    label: o.label.chars().take(PENDING_LABEL_MAX).collect(),
                    selected: o.selected,
                    checked: o.checked,
                })
                .collect(),
            multi: self.multi,
            detail: self
                .detail
                .as_deref()
                .map(|d| d.chars().take(PENDING_DETAIL_MAX).collect()),
        }
    }
}

/// Cap on `PendingInput.question`'s length — a client turns this straight
/// into UI, so a runaway pane read must not blow up the row or the wire.
pub(crate) const PENDING_QUESTION_MAX: usize = 300;
/// Cap on each `PendingOption.label`'s length.
pub(crate) const PENDING_LABEL_MAX: usize = 200;
/// Cap on the number of options `PendingInput` carries.
pub(crate) const PENDING_OPTIONS_MAX: usize = 16;
/// Cap on `PendingInput.detail`'s length.
pub(crate) const PENDING_DETAIL_MAX: usize = 300;

/// The tool call a permission dialog asks about: the last `⏺ Tool(args)` /
/// `● Tool(args)` line above the dialog, without its bullet. Taken only when
/// the dialog's top rule (a decoration-only line, after blank lines at most)
/// sits right under it, so a tool call further up the scrollback, with prose
/// or another dialog between, never labels this one.
fn tool_call_detail(lines: &[&str], end: usize) -> Option<String> {
    let header = (0..end).rev().find(|&i| lines[i].starts_with(['⏺', '●']))?;
    let next = (header + 1..end).find(|&i| !lines[i].is_empty())?;
    if !lines[next].chars().all(is_decoration) {
        return None;
    }
    let call = lines[header].trim_start_matches(['⏺', '●']).trim();
    let name_len = call.find('(')?;
    if name_len == 0
        || !call[..name_len]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    Some(call.to_string())
}

/// A pane line without surrounding whitespace or box-drawing borders, so a
/// boxed dialog (`│ ❯ 1. Yes   │`) reads like an unboxed one.
fn clean_line(line: &str) -> &str {
    line.trim_matches(|c: char| c.is_whitespace() || c == '│' || c == '║')
}

/// Parse a cleaned line as a numbered choice such as `❯ 1. Yes` or `2) No`:
/// the ordinal, the label after the `N. ` / `N) ` marker (trimmed), and
/// whether it carries the `❯`/`›` selection glyph. `None` for anything else
/// (`1.5 GB`, `42 + x`, prose) — the one parser [`numbered_choice`] and the
/// dialog's `options` both build on.
pub(crate) fn parse_choice(line: &str) -> Option<(u8, &str, bool)> {
    let rest = line.trim_start_matches(['❯', '›']);
    let selected = rest.len() != line.len();
    let rest = rest.trim_start();
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 || digits > 2 {
        return None;
    }
    let mut after = rest[digits..].chars();
    match (after.next(), after.next()) {
        (Some('.' | ')'), Some(' ') | None) => {
            let n: u8 = rest[..digits].parse().ok()?;
            let label = rest[digits + 1..].trim();
            Some((n, label, selected))
        }
        _ => None,
    }
}

/// Split a multi-select choice's checkbox off its label: `[ ] Auth` is
/// `Some((false, "Auth"))`, `[✔] Auth` is `Some((true, "Auth"))`. `None` for a
/// label with no checkbox, i.e. every single-select choice.
///
/// Claude Code 2.1 draws a multi-select option as `❯ 1. [ ] Label`, the tick
/// being `figures.tick` (`✔`, or `✓` / `√` where the terminal lacks it).
fn split_checkbox(label: &str) -> Option<(bool, &str)> {
    let rest = label.strip_prefix('[')?;
    let mut chars = rest.chars();
    let mark = chars.next()?;
    let rest = chars.as_str().strip_prefix(']')?;
    let checked = match mark {
        ' ' => false,
        '✔' | '✓' | '√' | 'x' | 'X' | '×' => true,
        _ => return None,
    };
    Some((checked, rest.trim_start()))
}

/// The row a multi-select dialog submits from, when it carries the `❯`
/// cursor: `❯ Submit` on the last question, `❯ Next` on an earlier one.
fn is_focused_submit_row(line: &str) -> bool {
    let rest = line.trim_start_matches(['❯', '›']);
    rest.len() != line.len() && matches!(rest.trim(), "Submit" | "Next")
}

/// `Some(selected)` when a cleaned line is a numbered choice such as
/// `❯ 1. Yes` (`selected` = true) or `2. No`. `1.5 GB` and `42 + x` are not.
/// A thin, test-only view of [`parse_choice`] (production code needs the
/// ordinal and label too, so it calls `parse_choice` directly).
#[cfg(test)]
fn numbered_choice(line: &str) -> Option<bool> {
    parse_choice(line).map(|(_, _, selected)| selected)
}

/// Detect a Claude Code permission or question dialog on screen.
///
/// Cues, on ANSI-stripped and box-trimmed lines:
/// * permission: the "No, and tell Claude what to do differently" choice, or
///   a "Do you want to …" / "Would you like to …" line followed by at least
///   two numbered choices (plan approval included);
/// * question: an "Enter to select" hint (or the "Ready to submit your
///   answers?" review line) plus a selected numbered choice, or a multi-select
///   question whose cursor is on its `❯ Submit` / `❯ Next` row.
///
/// Because a dialog replaces the REPL's input box and footer, a live-REPL
/// cue ([`LIVE_REPL_CUES`], or an input prompt line `❯ …` that is not a
/// choice) BELOW the last dialog line means the text is scrollback, and no
/// dialog is reported.
fn detect_dialog(stripped: &str) -> Option<Dialog> {
    // Kept alongside the cleaned `lines` (border/whitespace-trimmed) so the
    // `options` fallback below can still tell an indented description line
    // from an unindented one — `clean_line` erases that difference.
    let raw_lines: Vec<&str> = stripped.lines().collect();
    let lines: Vec<&str> = raw_lines.iter().map(|l| clean_line(l)).collect();
    let lower: Vec<String> = lines.iter().map(|l| l.to_lowercase()).collect();
    // (line index, ordinal, label, selected) for every numbered-choice line.
    let choices: Vec<(usize, u8, &str, bool)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| parse_choice(l).map(|(n, label, sel)| (i, n, label, sel)))
        .collect();
    let is_choice = |i: usize| choices.iter().any(|(j, ..)| *j == i);
    let last = |pred: &dyn Fn(&str) -> bool| lower.iter().rposition(|l| pred(l));
    // A label too long for the pane wraps onto the lines below it; without
    // these the card showed "Yes, and don't ask again for: git push:* in" and
    // nothing of what followed.
    let full_labels = wrapped_labels(&raw_lines, &choices, &is_choice);

    // A multi-select question whose cursor sits on its Submit/Next row has no
    // `❯` on any choice, yet it is the same dialog, still waiting.
    let submit_focused = lines.iter().any(|l| is_focused_submit_row(l));

    let tell_claude = last(&|l| l.contains("no, and tell claude"));
    let ask = last(&|l| l.contains("do you want to") || l.contains("would you like to"));
    let select_hint = last(&|l| l.contains("enter to select"));
    // The "Review your answers" step a multi-select question (or a set of
    // questions) ends on draws no footer hint, only this line above its
    // `1. Submit answers` / `2. Cancel` choices.
    let review = last(&|l| l.contains("ready to submit your answers"));

    // An AskUserQuestion may well read "Which … do you want to enable?". A
    // permission dialog never draws checkboxes, and a multi-select question
    // does on every choice.
    let ask_choices = |a: usize| choices.iter().filter(move |(j, ..)| *j > a);
    let kind = if tell_claude.is_some()
        || ask.is_some_and(|a| {
            // The review step echoes each question above its own
            // "Ready to submit your answers?" line.
            review.is_none_or(|r| r < a)
                && ask_choices(a).count() >= 2
                && !ask_choices(a).all(|(_, _, label, _)| split_checkbox(label).is_some())
        }) {
        WaitingFor::Permission
    } else if (select_hint.is_some() || review.is_some())
        && (submit_focused || choices.iter().any(|(_, _, _, sel)| *sel))
    {
        WaitingFor::Input
    } else {
        return None;
    };

    let dialog_end = [
        tell_claude,
        ask,
        select_hint,
        review,
        choices.last().map(|c| c.0),
    ]
    .into_iter()
    .flatten()
    .max()?;
    let live_below = lower.iter().enumerate().skip(dialog_end + 1).any(|(i, l)| {
        !is_choice(i)
            && !is_focused_submit_row(lines[i])
            && (LIVE_REPL_CUES.iter().any(|c| l.contains(c)) || lines[i].starts_with('❯'))
    });
    if live_below {
        return None;
    }

    // The trailing run of choice lines ending at the last one, tolerating a
    // non-choice line in between only when it is blank/decoration-only or
    // was indented in the raw capture (a description line) — an unindented,
    // non-empty line ends the run, so an unrelated list higher up the
    // scrollback is excluded.
    let block_start = choices.last().map(|(last_idx, ..)| {
        let mut start = *last_idx;
        while start > 0 {
            let prev = start - 1;
            if is_choice(prev) {
                start = prev;
                continue;
            }
            let gap_allowed = lines[prev].chars().all(is_decoration)
                || raw_lines[prev].starts_with(|c: char| c.is_whitespace());
            if !gap_allowed {
                break;
            }
            start = prev;
        }
        start
    });
    // The question sits ABOVE the choices. An option's own description line
    // may end in "?" too ("Is that enough?"); taking that as the question
    // drew the wrong question and dropped every option above it — the
    // selected one included. Only when nothing above the block reads as a
    // question is a line inside it (or the footer) considered.
    let is_question = |(i, l): &(usize, &&str)| l.ends_with('?') && !is_choice(*i);
    let question_idx = block_start
        .and_then(|b| lines[..b].iter().enumerate().rev().find(is_question))
        .or_else(|| {
            lines[..=dialog_end]
                .iter()
                .enumerate()
                .rev()
                .find(is_question)
        })
        .map(|(i, _)| i);
    let question = question_idx.map(|i| lines[i].to_string());
    let selected = choices
        .iter()
        .find(|(_, _, _, sel)| *sel)
        .map(|(i, ..)| lines[*i].trim_start_matches(['❯', '›']).trim().to_string());
    // `options` is bounded to the dialog's OWN choice block, not every
    // numbered line in the captured tail — an agent's own "2. Add the guard
    // / 3. Run the tests" text further up the scrollback must not leak in
    // and duplicate `n`.
    //
    // When the dialog has a question/ask line (the "do you want to" /
    // "would you like to" line, or the line the `question` search above
    // found), every choice AFTER it and at or before `dialog_end` belongs
    // to the dialog — real dialogs interleave indented description lines
    // between choices (see `question_ask_user.txt`), so this branch does
    // not require contiguity. `ask` is the LAST such phrase anywhere in the
    // pane, which can be stale scrollback prose sitting above the real
    // dialog's own question; taking the LATER of `ask` and `question_idx`
    // (not just preferring `ask`) keeps the bound as close to the actual
    // choices as possible, so that stale prose does not widen `options`
    // back into an unrelated numbered list between it and the real dialog.
    //
    // Without such a line (a bare `tell_claude` match with no "do you
    // want"/"?" line above its choices), fall back to the trailing run of
    // choice lines (`block_start`).
    let bound_after = ask.into_iter().chain(question_idx).max();
    let label_of = |i: &usize, label: &str| -> String {
        full_labels
            .get(i)
            .cloned()
            .unwrap_or_else(|| label.to_string())
    };
    let options: Vec<ParsedOption> = match bound_after {
        Some(after) => choices
            .iter()
            .filter(|(i, ..)| *i > after)
            .map(|(i, n, label, selected)| choice_option(*n, &label_of(i, label), *selected))
            .collect(),
        None => match block_start {
            Some(start) => choices
                .iter()
                .filter(|(i, ..)| *i >= start)
                .map(|(i, n, label, selected)| choice_option(*n, &label_of(i, label), *selected))
                .collect(),
            None => Vec::new(),
        },
    };
    // Every choice of a multi-select question carries a checkbox (the
    // "Type something" row included); one stray `[x] …` label in a
    // single-select menu does not make it one.
    let multi =
        kind == WaitingFor::Input && !options.is_empty() && options.iter().all(|o| o.multi_choice);
    let options = options
        .into_iter()
        .map(|o| PendingOption {
            checked: multi && o.option.checked,
            label: if multi { o.option.label } else { o.raw_label },
            ..o.option
        })
        .collect();
    let detail = if kind == WaitingFor::Permission {
        tool_call_detail(&lines, dialog_end)
    } else {
        None
    };
    Some(Dialog {
        kind,
        prompt: question.or(selected),
        options,
        multi,
        detail,
    })
}

/// How close to the pane's width a label line must reach for the next line
/// to be its wrapped continuation (see [`wrapped_labels`]): the dialog's own
/// right padding, plus one column of slack.
const WRAP_SLACK: usize = 3;

/// The full text of each choice whose label wrapped, by line index.
///
/// Claude Code (Ink) wraps a long label at a word boundary and continues it
/// on the next line, indented to where the label starts — exactly where an
/// AskUserQuestion option's DESCRIPTION goes too. Indentation cannot tell
/// them apart; width can: a line is a continuation only if its first word
/// would not have fitted at the end of the line above. A short label
/// followed by its description never qualifies. The pane's width is its
/// widest line (the dialog's full-width separator). Boxed dialogs (`│ … │`)
/// pad every line to the box, so they are left as they were.
fn wrapped_labels(
    raw_lines: &[&str],
    choices: &[(usize, u8, &str, bool)],
    is_choice: &dyn Fn(usize) -> bool,
) -> std::collections::HashMap<usize, String> {
    let width_of = |l: &str| l.trim_end().chars().count();
    let width = raw_lines.iter().map(|l| width_of(l)).max().unwrap_or(0);
    let mut out = std::collections::HashMap::new();
    for (i, _, label, _) in choices {
        let line = raw_lines[*i].trim_end();
        if line.contains(['│', '║']) || label.is_empty() {
            continue;
        }
        let Some(at) = line.find(*label) else {
            continue;
        };
        let label_col = line[..at].chars().count();
        let mut text = label.to_string();
        let mut reached = width_of(line);
        for (j, next) in raw_lines.iter().enumerate().skip(i + 1) {
            if is_choice(j) {
                break;
            }
            let body = next.trim();
            let indent = next.chars().take_while(|c| c.is_whitespace()).count();
            let Some(first) = body.split_whitespace().next() else {
                break;
            };
            if indent < label_col || next.contains(['│', '║']) {
                break;
            }
            // It would have fitted up there: the line above ended by choice,
            // not by running out of room.
            if reached + 1 + first.chars().count() + WRAP_SLACK <= width {
                break;
            }
            text.push(' ');
            text.push_str(body);
            reached = width_of(next);
        }
        if text.len() != label.len() {
            out.insert(*i, text);
        }
    }
    out
}

/// A dialog choice as parsed, before the dialog as a whole decides whether it
/// is a multi-select: the checkbox-split option, whether it had a checkbox,
/// and the label as drawn, for a dialog that turns out not to be one.
struct ParsedOption {
    option: PendingOption,
    multi_choice: bool,
    raw_label: String,
}

fn choice_option(n: u8, label: &str, selected: bool) -> ParsedOption {
    let (multi_choice, checked, text) = match split_checkbox(label) {
        Some((checked, text)) => (true, checked, text),
        None => (false, false, label),
    };
    ParsedOption {
        option: PendingOption {
            n,
            label: text.to_string(),
            selected,
            checked,
        },
        multi_choice,
        raw_label: label.to_string(),
    }
}

/// Derive a coarse status from the tail. Used ONLY as a fallback when the
/// authoritative `claude agents` status is absent.
fn derive_status(
    stuck: Option<StuckKind>,
    dialog: Option<&Dialog>,
    stripped: &str,
) -> Option<ClaudeStatus> {
    // A stuck state or an on-screen permission/question dialog: Claude is
    // waiting on the user. Checked before the idle cues below, because a
    // dialog's own footer ("Enter to select", "Esc to cancel") matches them.
    if stuck.is_some() || dialog.is_some() {
        return Some(ClaudeStatus::Blocked);
    }
    let lower = stripped.to_lowercase();
    if lower.trim().is_empty() {
        return None;
    }
    // WORKING only while Claude is actively generating. The interrupt hint sits
    // in the live REPL footer and vanishes the instant the turn ends, so it is
    // the one reliable "still running" signal.
    //
    // We deliberately do NOT key off `⏺` tool glyphs, "tool use", or
    // "running…"/"in progress…": those persist in the captured scrollback long
    // after the work finished (e.g. a completed "⏺ Bash(…)" line, or the summary
    // text "41 tool uses"), which made idle sessions read as "working" forever.
    if lower.contains("esc to interrupt") {
        return Some(ClaudeStatus::Working);
    }
    // IDLE: the REPL is showing its input chrome — the status bar, the
    // permissions/mode footer, the shortcut hint, or a menu hint with no
    // dialog behind it (a real permission/question dialog returned Blocked
    // above). Any of these means the turn is over and Claude wants input.
    if lower.contains("? for shortcuts")
        || lower.contains("% used")
        || lower.contains("bypass permissions")
        || lower.contains("shift+tab to cycle")
        || lower.contains("enter to select")
        || lower.contains("esc to cancel")
    {
        return Some(ClaudeStatus::Idle);
    }
    None
}

/// Analyze a captured pane tail into the reconcile signals.
/// The live spinner line of a working REPL, without its leading glyph:
/// `✶ Cooking… (3s · esc to interrupt)` → `Cooking… (3s · esc to interrupt)`.
/// The spinner is the one line that starts with a decoration glyph followed
/// by a capitalised verb ending in an ellipsis; the mode footer, `❯` prompt
/// lines and `⏺` tool lines never match. Scanned bottom-up so the newest
/// spinner wins. `None` when the pane shows no spinner.
pub fn spinner_line(pane_tail: &str) -> Option<String> {
    let stripped = strip_ansi(pane_tail);
    for raw in stripped.lines().rev() {
        let line = raw.trim();
        let mut chars = line.chars();
        let glyph = match chars.next() {
            Some(c) if !c.is_alphanumeric() && !c.is_whitespace() => c,
            _ => continue,
        };
        if matches!(glyph, '❯' | '>' | '⏺' | '⎿' | '⏸' | '⏵' | '│' | '─' | '●') {
            continue;
        }
        let rest = chars.as_str().trim_start();
        let word = rest.split_whitespace().next().unwrap_or("");
        let starts_upper = word.chars().next().is_some_and(char::is_uppercase);
        if !starts_upper || !word.ends_with('…') {
            continue;
        }
        // The REPL's spinner always carries its elapsed time in parentheses
        // (`(3s · …)`); assistant prose ending in an ellipsis and a custom
        // statusLine (`⚡ Opus… 42% ctx`) do not.
        if !rest.contains('(') || !has_elapsed(rest) {
            continue;
        }
        return Some(rest.chars().take(ACTIVITY_MAX).collect());
    }
    None
}

/// `12s` / `3m 5s`-style elapsed marker: digits directly followed by `s`
/// and then a non-letter (or the end).
fn has_elapsed(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let mut j = i;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j < b.len()
                && b[j] == b's'
                && b.get(j + 1).is_none_or(|c| !c.is_ascii_alphanumeric())
            {
                return true;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    false
}

pub fn analyze(pane_tail: &str) -> PaneIntel {
    let stripped = strip_ansi(pane_tail);
    let stuck = detect_stuck(&stripped);
    let context_pct = parse_context_pct(&stripped);
    // A stuck state (trust prompt, auth menu, …) is the more specific reading
    // of a screen that may also look like a generic dialog.
    let dialog = if stuck.is_none() {
        detect_dialog(&stripped)
    } else {
        None
    };
    // For a dialog, the question beats the last line (its key-hint footer).
    let activity = match &dialog {
        Some(d) => Some(d.activity()),
        None => pick_activity(&stripped),
    };
    let derived_status = derive_status(stuck, dialog.as_ref(), &stripped);
    let pending_input = dialog.as_ref().map(Dialog::pending_input);
    PaneIntel {
        activity,
        stuck,
        context_pct,
        derived_status,
        waiting_for: dialog.map(|d| d.kind),
        pending_input,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_pct_survives_a_multibyte_char_before_the_digits() {
        // `≈`, `—` and `·` are 2–3 bytes; `rfind + 1` used to slice mid-char
        assert_eq!(parse_context_pct("≈42% used"), Some(42.0));
        assert_eq!(parse_context_pct("context —95% used"), Some(95.0));
        assert_eq!(
            parse_context_pct("Context left until auto-compact: ·12%"),
            Some(88.0)
        );
        assert_eq!(analyze("≈42% used").context_pct, Some(42.0));
    }

    #[test]
    fn spinner_line_picks_the_verb_line_and_drops_the_glyph() {
        let tail = "⏺ Bash(cargo test)\n  ⎿ Running…\n✶ Cooking… (3s · esc to interrupt)\n";
        assert_eq!(
            spinner_line(tail).as_deref(),
            Some("Cooking… (3s · esc to interrupt)")
        );
        // newer layout: the interrupt hint moved to the footer, the spinner
        // carries hook progress and tokens; the footer itself never matches
        let tail = "● pong\n✢ Channelling… (running Stop hooks… 3/4 · 15s · ↓ 306 tokens)\n  ❯ /clear\n────\n❯ Press up to edit queued messages\n────\n  ⏸ manual mode on · esc to interrupt · ← 4 agents";
        assert_eq!(
            spinner_line(tail).as_deref(),
            Some("Channelling… (running Stop hooks… 3/4 · 15s · ↓ 306 tokens)")
        );
    }

    #[test]
    fn spinner_line_is_none_for_an_idle_or_blocked_pane() {
        assert_eq!(
            spinner_line("❯ \n  ⏸ manual mode on · ? for shortcuts"),
            None
        );
        // assistant prose ending in an ellipsis, and a custom statusLine
        assert_eq!(spinner_line("● Investigating… let me check"), None);
        assert_eq!(spinner_line("⚡ Opus… 42% ctx"), None);
        assert_eq!(spinner_line("✶ Cooking… nothing timed"), None);
        // a statusLine below the real spinner does not shadow it
        assert_eq!(
            spinner_line("✶ Cooking… (3s · esc to interrupt)\n❯ \n⚡ Opus… 42% ctx").as_deref(),
            Some("Cooking… (3s · esc to interrupt)")
        );
        assert_eq!(spinner_line("⏺ Explore(bg sessions)\n  ⎿  Done (41 tool uses)\n❯ 1. Yes\nEnter to select · Esc to cancel"), None);
        assert_eq!(spinner_line(""), None);
        // ANSI colour around the glyph is stripped first
        assert_eq!(
            spinner_line("\x1b[35m✻\x1b[0m Thinking… (1s)").as_deref(),
            Some("Thinking… (1s)")
        );
    }

    #[test]
    fn strip_ansi_removes_color_codes() {
        let raw = "\u{1b}[31mred\u{1b}[0m text\u{1b}[1;32mgreen\u{1b}[m";
        assert_eq!(strip_ansi(raw), "red textgreen");
    }

    #[test]
    fn strip_ansi_removes_osc_and_carriage_returns() {
        let raw = "\u{1b}]0;title\u{07}line\r\nmore";
        assert_eq!(strip_ansi(raw), "line\nmore");
    }

    #[test]
    fn empty_tail_yields_all_none() {
        let intel = analyze("");
        assert_eq!(intel.activity, None);
        assert_eq!(intel.stuck, None);
        assert_eq!(intel.context_pct, None);
        assert_eq!(intel.derived_status, None);

        let ws = analyze("   \n  \n\t\n");
        assert_eq!(ws.activity, None);
        assert_eq!(ws.stuck, None);
        assert_eq!(ws.context_pct, None);
        assert_eq!(ws.derived_status, None);
    }

    #[test]
    fn reconnect_is_blocked() {
        let intel = analyze("Some output\nReconnecting…\n");
        assert_eq!(intel.stuck, Some(StuckKind::Reconnect));
        assert_eq!(intel.derived_status, Some(ClaudeStatus::Blocked));
    }

    #[test]
    fn auth_menu_is_blocked() {
        let intel =
            analyze("Select login method:\n  1. Claude account with subscription\n  2. API key\n");
        assert_eq!(intel.stuck, Some(StuckKind::AuthMenu));
        assert_eq!(intel.derived_status, Some(ClaudeStatus::Blocked));
    }

    #[test]
    fn trust_prompt_detected() {
        let intel =
            analyze("Do you trust the files in this folder?\n  ❯ 1. Yes, proceed\n  2. No\n");
        assert_eq!(intel.stuck, Some(StuckKind::TrustPrompt));
        assert_eq!(intel.derived_status, Some(ClaudeStatus::Blocked));
    }

    #[test]
    fn press_enter_detected() {
        let intel = analyze("Update available.\nPress Enter to continue\n");
        assert_eq!(intel.stuck, Some(StuckKind::PressEnter));
        assert_eq!(intel.derived_status, Some(ClaudeStatus::Blocked));
    }

    #[test]
    fn oom_needs_a_kill_verdict_and_a_dead_process() {
        // A verdict followed by the shell's prompt: the process is gone.
        assert_eq!(
            analyze("Killed process 123 (OOM)\nme@host:~$ ").stuck,
            Some(StuckKind::Oom)
        );
        assert_eq!(
            analyze("container terminated reason=OOMKilled\n$ ").stuck,
            Some(StuckKind::Oom)
        );
        // The verdict alone is scrollback of unknown age.
        assert_eq!(analyze("Killed process 123 (OOM)").stuck, None);
        // Prose about memory is never a signal, whatever the words.
        assert_eq!(
            analyze("out of memory\ncannot allocate memory\noom\n$ ").stuck,
            None
        );
        assert_eq!(analyze("Let's zoom into the room — boom!").stuck, None);
        assert_eq!(analyze("⏺ Joining the Zoom meeting room").stuck, None);
    }

    /// F1: sessions 21480 / 21340 were flagged `oom` for reading the fleet's
    /// own stuck vocabulary, and the playbook recreated 21480 mid-turn twice.
    #[test]
    fn oom_never_fires_on_the_fleets_own_vocabulary() {
        for (fixture, status) in [
            (
                include_str!("testdata/pane_intel/oom_vocabulary_prose_idle.txt"),
                ClaudeStatus::Idle,
            ),
            (
                include_str!("testdata/pane_intel/oom_vocabulary_prose_working.txt"),
                ClaudeStatus::Working,
            ),
        ] {
            let intel = analyze(fixture);
            assert_eq!(intel.stuck, None, "{fixture}");
            assert_eq!(intel.derived_status, Some(status), "{fixture}");
        }
    }

    #[test]
    fn oom_fires_on_a_heap_block_or_a_kill_verdict_followed_by_the_shell() {
        for fixture in [
            include_str!("testdata/pane_intel/oom_heap_crash_to_shell.txt"),
            include_str!("testdata/pane_intel/oom_killed_to_shell.txt"),
        ] {
            let intel = analyze(fixture);
            assert_eq!(intel.stuck, Some(StuckKind::Oom), "{fixture}");
            assert_eq!(
                intel.derived_status,
                Some(ClaudeStatus::Blocked),
                "{fixture}"
            );
        }
    }

    #[test]
    fn oom_looks_only_at_the_last_twelve_lines() {
        let mut old = String::from(
            "FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory\n",
        );
        for i in 0..12 {
            old.push_str(&format!("line {i} of a long build log\n"));
        }
        old.push_str("$ ");
        assert_eq!(analyze(&old).stuck, None);
    }

    #[test]
    fn oom_text_above_a_live_repl_is_scrollback_not_a_crash() {
        // LIVE CAPTURE (2026-09-25, the fleet operator on mefistos): Claude's
        // own reply mentions another session being OOM-killed, and the idle
        // input box sits below it. Flagging this pane as OOM told the
        // operator's composer "the prompt may not be read" for as long as
        // the reply stayed on screen.
        let tail = "  Zasekli sa dve — v 240 dispatchers aj 21415 api-tenant-resolution bol Claude\n  \
                    predtým OOM-zabitý, takže prvý safe-kill prompt padol do shellu. Reštartoval\n  \
                    som ich, ale:\n\n\
                    ✻ Churned for 5m 53s · done 1:43 PM\n\
                    ─────────────────────────────────────── fleet-operator ─\n\
                    ❯ \n\
                    ────────────────────────────────────────────────────────\n  \
                    Opus 5 (1M context)  [██░░░░░░░░] 12% (122k/1.0M)  |  ~/.claude-f…\n  \
                    ⏵⏵ bypass permissions on (shift+tab to cycle) · ← for agents\n";
        let intel = analyze(tail);
        assert_eq!(intel.stuck, None);
        assert_eq!(intel.derived_status, Some(ClaudeStatus::Idle));
        // A tool's output while Claude keeps working is not Claude's OOM.
        let working = "⏺ Bash(make)\n  ⎿  cc1: out of memory allocating 65536 bytes\n\
                       ✶ Cooking… (3s · esc to interrupt)\n❯ \n";
        assert_eq!(analyze(working).stuck, None);
        // The REPL chrome ABOVE the crash is the screen Claude died on.
        let crashed = "❯ \n  ⏵⏵ bypass permissions on (shift+tab to cycle)\n\
                       <--- Last few GCs --->\n\
                       FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory\n\
                       me@host:~/proj$ ";
        assert_eq!(analyze(crashed).stuck, Some(StuckKind::Oom));
    }

    #[test]
    fn oom_detected() {
        let intel =
            analyze("<--- Last few GCs --->\nFATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory\n");
        assert_eq!(intel.stuck, Some(StuckKind::Oom));
        assert_eq!(intel.derived_status, Some(ClaudeStatus::Blocked));
    }

    #[test]
    fn context_footer_percent_used_live_shape() {
        // LIVE-CONFIRMED footer shape: "N% used" is percent CONSUMED.
        let tail = "  [█░░░░░░░░░░░░░░░░░░░] 9% used  |  Opus 4.7 (1M context)  |  /Users/me/proj";
        let intel = analyze(tail);
        let pct = intel.context_pct.expect("context_pct");
        assert!((pct - 9.0).abs() < 0.01, "expected ~9.0, got {pct}");
    }

    #[test]
    fn context_footer_left_until_autocompact_spec_shape() {
        // SPEC wording: percent REMAINING → 100 - 17 = 83.
        let tail = "Context left until auto-compact: 17%";
        let intel = analyze(tail);
        let pct = intel.context_pct.expect("context_pct");
        assert!((pct - 83.0).abs() < 0.01, "expected ~83.0, got {pct}");
    }

    #[test]
    fn token_count_variant_yields_no_pct() {
        // A bare token figure has no derivable percentage without the window size.
        let tail = "new task? /clear to save 561.8k tokens";
        let intel = analyze(tail);
        assert_eq!(intel.context_pct, None);

        let tail2 = "                                         93386 tokens";
        assert_eq!(analyze(tail2).context_pct, None);
    }

    #[test]
    fn active_generation_with_interrupt_footer_is_working() {
        // "esc to interrupt" in the live footer is the one reliable "still
        // generating" signal.
        let tail = "⏺ Bash(cargo test)\n  ⎿ Running…\n✶ Cooking… (3s · esc to interrupt)\n";
        let intel = analyze(tail);
        assert!(intel.stuck.is_none());
        assert_eq!(intel.derived_status, Some(ClaudeStatus::Working));
        let activity = intel.activity.expect("activity");
        assert!(
            activity.contains("interrupt")
                || activity.contains("Running")
                || activity.contains("⏺")
        );
    }

    #[test]
    fn lingering_tool_glyph_with_idle_footer_is_idle_not_working() {
        // LIVE-CAPTURED shape: a finished "⏺ …" line still sits in the tail, but
        // the footer shows the idle status bar. Must be idle, not working — this
        // exact case made sessions hang in "working".
        let tail = "⏺ Yes — all merged, queue empty.\n  - 7 work PRs MERGED\n  [███████░░] 59% used  |  Opus 4.7  |  /Users/me/proj\n  ⏵⏵ bypass permissions on (shift+tab to cycle) · ← for agents";
        assert_eq!(analyze(tail).derived_status, Some(ClaudeStatus::Idle));
    }

    #[test]
    fn idle_repl_footer_without_percent_is_idle() {
        // LIVE-CAPTURED shape: a fresh session whose footer shows the model +
        // permissions hint but NO "% used". Previously yielded None → the upsert
        // COALESCE froze the prior status. Must classify as idle.
        let tail = "❯ \n────────\n  Opus 4.7 (1M context)  |  /Users/me/proj\n  ⏵⏵ bypass permissions on (shift+tab to cycle)";
        assert_eq!(analyze(tail).derived_status, Some(ClaudeStatus::Idle));
    }

    #[test]
    fn selection_menu_question_is_blocked_not_idle() {
        // LIVE-CAPTURED shape: an interactive question menu (a brainstorming
        // question) is waiting on a keystroke. The "41 tool uses" summary text
        // once tripped the "tool use" working heuristic, and the menu's
        // "Enter to select · … · Esc to cancel" footer then read as idle.
        // Claude is waiting on an answer, so it is blocked (Q6/D3).
        let tail = "⏺ Explore(bg sessions)\n  ⎿  Done (41 tool uses · 133.5k tokens · 2m 1s)\n❯ 1. Show bg, toggle to hide\nEnter to select · Tab/Arrow keys to navigate · Esc to cancel";
        let intel = analyze(tail);
        assert_eq!(intel.derived_status, Some(ClaudeStatus::Blocked));
        assert_eq!(intel.waiting_for, Some(WaitingFor::Input));
        assert_eq!(intel.stuck, None);
        assert_eq!(
            intel.activity.as_deref(),
            Some("waiting for input: 1. Show bg, toggle to hide")
        );
    }

    // ---- permission / question dialogs (fixtures in testdata/pane_intel) ----

    fn fixture_intel(name: &str, text: &str) -> PaneIntel {
        let intel = analyze(text);
        assert_eq!(intel.stuck, None, "{name}: a dialog is not a stuck_kind");
        intel
    }

    fn assert_dialog(name: &str, text: &str, kind: WaitingFor, activity: &str, options_len: usize) {
        let intel = fixture_intel(name, text);
        assert_eq!(intel.derived_status, Some(ClaudeStatus::Blocked), "{name}");
        assert_eq!(intel.waiting_for, Some(kind), "{name}");
        assert_eq!(intel.activity.as_deref(), Some(activity), "{name}");
        assert_eq!(
            intel.pending_input.map(|p| p.options.len()),
            Some(options_len),
            "{name}: unexpected options count"
        );
    }

    /// LIVE CAPTURE (`tmux capture-pane -p -S -8`, Claude Code 2.1.267, a
    /// throwaway session in a scratch dir, paths anonymised): plain `claude`
    /// in manual mode asked to run `ls`. Note the real dialog has no "No, and
    /// tell Claude…" choice — it is caught by "Do you want to proceed?" plus
    /// numbered choices — and its footer is the idle-looking "Esc to cancel".
    #[test]
    fn bash_permission_dialog_is_blocked_on_permission() {
        assert_dialog(
            "permission_bash",
            include_str!("testdata/pane_intel/permission_bash.txt"),
            WaitingFor::Permission,
            "waiting for permission: Do you want to proceed?",
            4,
        );
    }

    /// The same live dialog with a status line and mode line painted under
    /// it. Those are not scrollback cues, so it stays blocked.
    #[test]
    fn permission_dialog_with_a_status_line_below_is_still_blocked() {
        let intel = fixture_intel(
            "permission_statusline_below",
            include_str!("testdata/pane_intel/permission_statusline_below.txt"),
        );
        assert_eq!(intel.derived_status, Some(ClaudeStatus::Blocked));
        assert_eq!(intel.waiting_for, Some(WaitingFor::Permission));
        // The status line is still read for the context percentage.
        assert_eq!(intel.context_pct, Some(34.0));
        assert_eq!(intel.pending_input.map(|p| p.options.len()), Some(4));
    }

    #[test]
    fn boxed_edit_permission_dialog_is_blocked_on_permission() {
        assert_dialog(
            "permission_edit_boxed",
            include_str!("testdata/pane_intel/permission_edit_boxed.txt"),
            WaitingFor::Permission,
            "waiting for permission: Do you want to make this edit to health.rs?",
            3,
        );
    }

    #[test]
    fn permission_dialog_with_esc_to_cancel_footer_is_not_idle() {
        // No "tell Claude" choice here: the "Do you want to" line plus
        // numbered choices carries it, over the "Esc to cancel" idle cue.
        assert_dialog(
            "permission_create_footer",
            include_str!("testdata/pane_intel/permission_create_footer.txt"),
            WaitingFor::Permission,
            "waiting for permission: Do you want to create notes.md?",
            3,
        );
    }

    #[test]
    fn ask_user_question_menu_is_blocked_on_input() {
        assert_dialog(
            "question_ask_user",
            include_str!("testdata/pane_intel/question_ask_user.txt"),
            WaitingFor::Input,
            "waiting for input: Keep ghosted sessions for how long before deleting them?",
            4,
        );
    }

    /// The description line under each choice (e.g. "Long enough to
    /// recreate them after a tmux server restart.") must not break the
    /// choice block: all four options survive, with their real labels.
    #[test]
    fn question_ask_user_fixture_options_survive_description_lines() {
        let p = fixture_intel(
            "question_ask_user",
            include_str!("testdata/pane_intel/question_ask_user.txt"),
        )
        .pending_input
        .expect("dialog");
        assert_eq!(
            p.options,
            vec![
                PendingOption {
                    n: 1,
                    label: "24 hours (Recommended)".into(),
                    selected: true,
                    checked: false,
                },
                PendingOption {
                    n: 2,
                    label: "1 hour".into(),
                    selected: false,
                    checked: false,
                },
                PendingOption {
                    n: 3,
                    label: "Until dismissed".into(),
                    selected: false,
                    checked: false,
                },
                PendingOption {
                    n: 4,
                    label: "Type something.".into(),
                    selected: false,
                    checked: false,
                },
            ]
        );
    }

    /// A label too long for the pane wraps onto the next line, indented to
    /// the label: the card gets the whole label, not the first line of it.
    #[test]
    fn a_wrapped_option_label_is_read_whole() {
        let sep = "─".repeat(48);
        let text = format!(
            "{sep}\n Bash command\n\n   git push origin main\n\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don’t ask again for: git push:* in\n      /home/user/claude-fleet\n   3. No\n\n Esc to cancel · Tab to amend\n"
        );
        let p = analyze(&text).pending_input.expect("dialog");
        assert_eq!(
            p.options
                .iter()
                .map(|o| o.label.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Yes",
                "Yes, and don’t ask again for: git push:* in /home/user/claude-fleet",
                "No"
            ]
        );
    }

    /// A short label followed by its description at the same indentation
    /// (AskUserQuestion) is not a wrap: the description's first word would
    /// have fitted on the label's line.
    #[test]
    fn a_description_under_a_short_label_is_not_a_wrap() {
        let p = analyze(include_str!("testdata/pane_intel/question_ask_user.txt"))
            .pending_input
            .expect("dialog");
        assert_eq!(p.options[0].label, "24 hours (Recommended)");
        assert_eq!(p.options[1].label, "1 hour");
    }

    /// An option's description may itself end in "?". It is not the
    /// question: taking it as one drew the wrong question and dropped every
    /// option above it — here the selected option 1.
    #[test]
    fn a_description_ending_in_a_question_mark_is_not_the_question() {
        let text = include_str!("testdata/pane_intel/question_ask_user.txt").replace(
            "Long enough to recreate them after a tmux server restart.",
            "Is a day long enough to recreate them?",
        );
        let p = analyze(&text).pending_input.expect("dialog");
        assert_eq!(
            p.question.as_deref(),
            Some("Keep ghosted sessions for how long before deleting them?")
        );
        assert_eq!(
            p.options.iter().map(|o| o.n).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert!(p.options[0].selected);
    }

    /// RECONSTRUCTED from Claude Code 2.1's multi-select renderer (not a
    /// live capture): an AskUserQuestion with `multiSelect: true` draws each
    /// option as `N. [ ] Label` / `N. [✔] Label` and a `Submit` row under
    /// them. A digit TOGGLES a box, so the card must know it is a multi-select
    /// and must not count the box as part of the label: ticking one would
    /// otherwise turn the same question into a "changed" dialog.
    #[test]
    fn multi_select_question_strips_the_checkbox_into_checked() {
        let text = include_str!("testdata/pane_intel/question_multi_select.txt");
        assert_dialog(
            "question_multi_select",
            text,
            WaitingFor::Input,
            "waiting for input: Which features do you want to enable?",
            4,
        );
        let p = fixture_intel("question_multi_select", text)
            .pending_input
            .expect("dialog");
        assert!(p.multi);
        let got: Vec<(u8, &str, bool, bool)> = p
            .options
            .iter()
            .map(|o| (o.n, o.label.as_str(), o.selected, o.checked))
            .collect();
        assert_eq!(
            got,
            vec![
                (1, "Auth", true, false),
                (2, "Logging", false, true),
                (3, "Metrics", false, true),
                (4, "Type something", false, false),
            ]
        );
    }

    /// With the cursor on the multi-select's own `❯ Submit` row no choice
    /// carries `❯`, and the row itself starts with it — neither may read as
    /// "the dialog is gone" or "the REPL prompt is back".
    #[test]
    fn multi_select_with_the_cursor_on_submit_is_still_the_dialog() {
        let text = include_str!("testdata/pane_intel/question_multi_select.txt")
            .replace("❯ 1. [ ] Auth", "  1. [ ] Auth")
            .replace("     Submit", "❯    Submit");
        let p = fixture_intel("multi_select_submit_focused", &text)
            .pending_input
            .expect("still a dialog");
        assert!(p.multi);
        assert_eq!(p.options.len(), 4);
        assert!(p.options.iter().all(|o| !o.selected));
    }

    /// The step a multi-select ends on: no "Enter to select" footer, only
    /// the "Ready to submit your answers?" line over two numbered choices. It
    /// is what actually sends the answers, so it must be a dialog too.
    #[test]
    fn review_your_answers_step_is_blocked_on_input() {
        let text = "\
 ←  ✔ Features  ✔ Submit  →

Review your answers

 • Which features do you want to enable?
   → Logging, Metrics

Ready to submit your answers?

❯ 1. Submit answers
  2. Cancel
";
        assert_dialog(
            "review_answers",
            text,
            WaitingFor::Input,
            "waiting for input: Ready to submit your answers?",
            2,
        );
        let p = analyze(text).pending_input.expect("dialog");
        assert!(!p.multi);
        assert_eq!(p.options[0].label, "Submit answers");
    }

    /// A box-like label in a single-select menu stays the label as drawn: a
    /// dialog is a multi-select only when EVERY choice carries a checkbox.
    #[test]
    fn one_checkbox_label_does_not_make_a_multi_select() {
        let text = "\
Which one?

❯ 1. [x] keep the marker
  2. Drop it

Enter to select · ↑/↓ to navigate · Esc to cancel
";
        let p = analyze(text).pending_input.expect("dialog");
        assert!(!p.multi);
        assert_eq!(p.options[0].label, "[x] keep the marker");
        assert!(!p.options[0].checked);
    }

    /// The two multi-select fields stay off the wire for every other dialog,
    /// and a row stored before they existed still reads.
    #[test]
    fn pending_input_multi_fields_are_omitted_when_false_and_default_when_absent() {
        let single = PendingInput {
            kind: "input".into(),
            question: Some("Q?".into()),
            options: vec![PendingOption {
                n: 1,
                label: "A".into(),
                selected: true,
                checked: false,
            }],
            multi: false,
            detail: None,
        };
        let json = serde_json::to_string(&single).expect("serialize");
        assert!(
            !json.contains("multi") && !json.contains("checked") && !json.contains("detail"),
            "{json}"
        );
        let old =
            r#"{"kind":"input","question":null,"options":[{"n":1,"label":"A","selected":true}]}"#;
        let back: PendingInput = serde_json::from_str(old).expect("old row");
        assert!(!back.multi && !back.options[0].checked && back.detail.is_none());
    }

    fn detail_of(text: &str) -> Option<String> {
        analyze(text).pending_input.and_then(|p| p.detail)
    }

    /// The approval card shows what it approves (redesign 5.9): the tool-call
    /// line right above each permission dialog, bullet dropped.
    #[test]
    fn permission_dialogs_carry_the_tool_call_they_ask_about() {
        for (name, text, want) in [
            (
                "permission_bash",
                include_str!("testdata/pane_intel/permission_bash.txt"),
                "Bash(ls)",
            ),
            (
                "permission_statusline_below",
                include_str!("testdata/pane_intel/permission_statusline_below.txt"),
                "Bash(ls)",
            ),
            (
                "permission_edit_boxed",
                include_str!("testdata/pane_intel/permission_edit_boxed.txt"),
                "Update(src/service/health.rs)",
            ),
            (
                "permission_create_footer",
                include_str!("testdata/pane_intel/permission_create_footer.txt"),
                "Write(docs/notes.md)",
            ),
        ] {
            assert_eq!(detail_of(text).as_deref(), Some(want), "{name}");
        }
    }

    /// A question has no tool call, and a tool call with prose between it
    /// and the dialog is someone else's: neither labels the card.
    #[test]
    fn detail_is_absent_for_questions_and_for_a_tool_call_further_up() {
        assert_eq!(
            detail_of(include_str!("testdata/pane_intel/question_ask_user.txt")),
            None
        );
        let stale = "⏺ Bash(rm -rf build)\n\n⏺ Done. Now the push.\n\n────────\n Do you want to proceed?\n ❯ 1. Yes\n   2. No\n";
        let p = analyze(stale).pending_input.expect("a dialog");
        assert_eq!(p.detail, None);
        let long = format!(
            "⏺ Bash(echo {})\n────────\n Do you want to proceed?\n ❯ 1. Yes\n   2. No\n",
            "x".repeat(400)
        );
        assert_eq!(
            detail_of(&long).map(|d| d.chars().count()),
            Some(PENDING_DETAIL_MAX)
        );
    }

    #[test]
    fn plan_approval_is_blocked_even_when_a_choice_says_bypass_permissions() {
        // "bypass permissions" is a live-REPL cue, but here it is a choice
        // line inside the dialog, which must not cancel the dialog.
        assert_dialog(
            "plan_approval_bypass",
            include_str!("testdata/pane_intel/plan_approval_bypass.txt"),
            WaitingFor::Permission,
            "waiting for permission: Would you like to proceed?",
            3,
        );
    }

    #[test]
    fn prose_question_above_the_idle_prompt_stays_idle() {
        // Claude asked in prose, with a numbered list, and the turn is over:
        // the input prompt and "? for shortcuts" sit below the text.
        let intel = fixture_intel(
            "prose_question_idle",
            include_str!("testdata/pane_intel/prose_question_idle.txt"),
        );
        assert_eq!(intel.derived_status, Some(ClaudeStatus::Idle));
        assert_eq!(intel.waiting_for, None);
        assert_eq!(intel.activity.as_deref(), Some("? for shortcuts"));
    }

    #[test]
    fn dialog_text_in_scrollback_while_generating_is_working() {
        // A diff that quotes dialog text, with the live spinner below it.
        let intel = fixture_intel(
            "dialog_text_in_scrollback_working",
            include_str!("testdata/pane_intel/dialog_text_in_scrollback_working.txt"),
        );
        assert_eq!(intel.derived_status, Some(ClaudeStatus::Working));
        assert_eq!(intel.waiting_for, None);
    }

    #[test]
    fn typed_input_prompt_below_dialog_text_means_scrollback() {
        // No footer hint at all, but the input box (`❯ …`) is below.
        let tail = " Do you want to proceed?\n ❯ 1. Yes\n   2. No, and tell Claude what to do differently (esc)\n⏺ Done.\n────────\n❯ now fix the tests\n────────\n";
        let intel = analyze(tail);
        assert_eq!(intel.waiting_for, None);
        assert_ne!(intel.derived_status, Some(ClaudeStatus::Blocked));
    }

    #[test]
    fn menu_hint_without_a_dialog_is_still_idle() {
        assert_eq!(
            analyze("Select a theme\nEnter to select · Esc to cancel").derived_status,
            Some(ClaudeStatus::Idle)
        );
    }

    #[test]
    fn stuck_trust_prompt_wins_over_the_generic_dialog() {
        let intel = analyze(
            "Do you trust the files in this folder?\n ❯ 1. Yes, proceed\n   2. No\nEnter to select",
        );
        assert_eq!(intel.stuck, Some(StuckKind::TrustPrompt));
        assert_eq!(intel.waiting_for, None);
        assert_eq!(intel.derived_status, Some(ClaudeStatus::Blocked));
    }

    #[test]
    fn numbered_choice_shapes() {
        assert_eq!(numbered_choice("❯ 1. Yes"), Some(true));
        assert_eq!(numbered_choice("2. No"), Some(false));
        assert_eq!(numbered_choice("3) Maybe"), Some(false));
        assert_eq!(numbered_choice("1.5 GB free"), None);
        assert_eq!(numbered_choice("42 +  ❯ 1. Yes"), None);
        assert_eq!(numbered_choice("❯ fix it"), None);
        assert_eq!(numbered_choice(""), None);
    }

    #[test]
    fn parse_choice_extracts_the_ordinal_and_label() {
        assert_eq!(
            parse_choice("10) Something"),
            Some((10, "Something", false))
        );
        assert_eq!(parse_choice("2) No"), Some((2, "No", false)));
        assert_eq!(parse_choice("❯ 1. Yes"), Some((1, "Yes", true)));
    }

    #[test]
    fn waiting_for_tags_are_stable() {
        assert_eq!(WaitingFor::Permission.as_str(), "permission");
        assert_eq!(WaitingFor::Input.as_str(), "input");
    }

    #[test]
    fn pending_input_caps_question_label_and_option_count() {
        // A client turns this straight into UI, so a malformed or
        // adversarial pane read must not blow up the row or the wire.
        let long_label = "x".repeat(500);
        let dialog = Dialog {
            kind: WaitingFor::Input,
            prompt: Some("q".repeat(500)),
            options: (1..=20u8)
                .map(|n| PendingOption {
                    n,
                    label: long_label.clone(),
                    selected: false,
                    checked: false,
                })
                .collect(),
            multi: false,
            detail: Some("d".repeat(500)),
        };
        let p = dialog.pending_input();
        assert_eq!(p.question.as_deref().map(|q| q.chars().count()), Some(300));
        assert_eq!(p.options.len(), 16);
        assert!(p.options.iter().all(|o| o.label.chars().count() == 200));
        assert_eq!(
            p.detail.map(|d| d.chars().count()),
            Some(PENDING_DETAIL_MAX)
        );
    }

    #[test]
    fn a_permission_dialog_carries_its_numbered_options() {
        let pane = "\
Do you want to make this edit to src/main.rs?
❯ 1. Yes
  2. Yes, and don't ask again this session
  3. No, and tell Claude what to do differently
";
        let d = detect_dialog(pane).expect("dialog");
        let p = d.pending_input();
        assert_eq!(p.kind, "permission");
        assert_eq!(
            p.question.as_deref(),
            Some("Do you want to make this edit to src/main.rs?")
        );
        assert_eq!(p.options.len(), 3);
        assert_eq!(
            p.options[0],
            PendingOption {
                n: 1,
                label: "Yes".into(),
                selected: true,
                checked: false,
            }
        );
        assert_eq!(p.options[2].n, 3);
        assert!(!p.options[2].selected);
    }

    #[test]
    fn a_question_dialog_is_input_and_a_boxed_dialog_loses_its_borders() {
        let pane = "│ Keep ghosted sessions for how long before deleting them? │\n│ ❯ 1. 1 hour │\n│   2. 1 day │\nEnter to select\n";
        let p = detect_dialog(pane).expect("dialog").pending_input();
        assert_eq!(p.kind, "input");
        assert_eq!(
            p.options
                .iter()
                .map(|o| o.label.as_str())
                .collect::<Vec<_>>(),
            ["1 hour", "1 day"]
        );
    }

    #[test]
    fn options_stop_at_the_dialog_and_do_not_swallow_an_earlier_numbered_list() {
        // A numbered list further up the scrollback (an agent's own plan or
        // suggestion text) must not leak into `options` and duplicate `n`.
        let pane = "\
  2. Add the guard
  3. Run the tests
Do you want to proceed?
❯ 1. Yes
  2. Yes, and don't ask again
  3. No, and tell Claude what to do differently
";
        let p = detect_dialog(pane).expect("dialog").pending_input();
        assert_eq!(p.question.as_deref(), Some("Do you want to proceed?"));
        assert_eq!(p.options.len(), 3);
        assert_eq!(p.options.iter().map(|o| o.n).collect::<Vec<_>>(), [1, 2, 3]);
    }

    #[test]
    fn options_fallback_tolerates_indented_descriptions_but_stops_at_unrelated_prose() {
        // No "do you want to"/"would you like to" line and no "?" question
        // here — `tell_claude` alone carries the kind — so `options` falls
        // back to the trailing run: the indented description line between
        // the two choices is tolerated, but the unindented, unrelated prose
        // line above the dialog ends the run.
        let pane = "\
Some unrelated prose line
❯ 1. Yes
     do it now
  2. No, and tell Claude what to do differently
";
        let p = detect_dialog(pane).expect("dialog").pending_input();
        assert_eq!(p.options.len(), 2);
        assert_eq!(
            p.options,
            vec![
                PendingOption {
                    n: 1,
                    label: "Yes".into(),
                    selected: true,
                    checked: false,
                },
                PendingOption {
                    n: 2,
                    label: "No, and tell Claude what to do differently".into(),
                    selected: false,
                    checked: false,
                },
            ]
        );
    }

    #[test]
    fn bound_after_picks_the_later_of_ask_and_question_not_stale_scrollback_prose() {
        // Stale scrollback prose containing "do you want to" sits well above
        // the real dialog and its own unrelated numbered list; the real
        // dialog's own question line is LATER (closer to its choices) and
        // must win the bound, or the stale prose's numbered list leaks back
        // into `options`.
        let pane = "\
Earlier I asked: do you want to grab coffee?
  2. Add the guard
  3. Run the tests
Keep ghosted sessions for how long before deleting them?
❯ 1. 1 hour
  2. 1 day
Enter to select
";
        let p = detect_dialog(pane).expect("dialog").pending_input();
        assert_eq!(p.options.len(), 2);
        assert_eq!(p.options.iter().map(|o| o.n).collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn activity_picks_last_meaningful_line_and_caps_length() {
        let long = "x".repeat(500);
        let tail = format!("first line\n{long}\n   \n");
        let intel = analyze(&tail);
        let activity = intel.activity.expect("activity");
        assert_eq!(activity.chars().count(), ACTIVITY_MAX);
    }

    #[test]
    fn decoration_only_lines_are_skipped_for_activity() {
        let tail = "real content\n────────────────\n";
        let intel = analyze(tail);
        assert_eq!(intel.activity.as_deref(), Some("real content"));
    }

    #[test]
    fn stuck_kind_tags_are_stable() {
        assert_eq!(StuckKind::AuthMenu.as_str(), "auth_menu");
        assert_eq!(StuckKind::Reconnect.as_str(), "reconnect");
        assert_eq!(StuckKind::TrustPrompt.as_str(), "trust_prompt");
        assert_eq!(StuckKind::Oom.as_str(), "oom");
        assert_eq!(StuckKind::PressEnter.as_str(), "press_enter");
    }

    // ---- status vocabulary ------------------------------------------------

    #[test]
    fn stuck_kind_round_trips_through_str_and_serde() {
        for k in StuckKind::ALL {
            assert_eq!(k.as_str().parse::<StuckKind>().unwrap(), *k);
            let json = serde_json::to_string(k).unwrap();
            assert_eq!(json, format!("\"{}\"", k.as_str()));
            assert_eq!(serde_json::from_str::<StuckKind>(&json).unwrap(), *k);
        }
        assert!("confirmation".parse::<StuckKind>().is_err());
        assert!("none".parse::<StuckKind>().is_err());
    }

    /// The shared fixture `src/lib/conversation.test.ts` reads too: one
    /// answer to "is this turn over?" for `is_quiet`, `store::turn_over` and
    /// the frontend's `isQuietStatus`, every vocabulary value covered and an
    /// unknown value (and none) not quiet.
    #[test]
    fn quiet_statuses_match_the_shared_fixture() {
        #[derive(serde::Deserialize)]
        struct Case {
            status: Option<String>,
            quiet: bool,
        }
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("testdata/quiet_statuses.json")).unwrap();
        for c in &cases {
            assert_eq!(
                crate::store::turn_over(c.status.as_deref()),
                c.quiet,
                "turn_over({:?})",
                c.status
            );
            if let Some(k) = c
                .status
                .as_deref()
                .and_then(|s| s.parse::<ClaudeStatus>().ok())
            {
                assert_eq!(k.is_quiet(), c.quiet, "{k}.is_quiet()");
            }
        }
        for k in ClaudeStatus::ALL {
            assert!(
                cases
                    .iter()
                    .any(|c| c.status.as_deref() == Some(k.as_str())),
                "the fixture lacks {k}"
            );
        }
        assert!(cases.iter().any(|c| c.status.is_none()));
        assert!(cases.iter().any(|c| c
            .status
            .as_deref()
            .is_some_and(|s| s.parse::<ClaudeStatus>().is_err())));
    }

    #[test]
    fn claude_status_round_trips_through_str_and_serde() {
        for k in ClaudeStatus::ALL {
            assert_eq!(k.as_str().parse::<ClaudeStatus>().unwrap(), *k);
            let json = serde_json::to_string(k).unwrap();
            assert_eq!(json, format!("\"{}\"", k.as_str()));
            assert_eq!(serde_json::from_str::<ClaudeStatus>(&json).unwrap(), *k);
        }
        assert!("stuck".parse::<ClaudeStatus>().is_err());
        assert!("awaiting_input".parse::<ClaudeStatus>().is_err());
    }

    #[test]
    fn vocabulary_doc_lists_every_value_once() {
        assert_eq!(
            ClaudeStatus::vocabulary_doc(),
            "working | blocked | completed | failed | stopped | idle"
        );
        assert_eq!(
            StuckKind::vocabulary_doc(),
            "auth_menu | reconnect | trust_prompt | oom | press_enter"
        );
    }

    /// Write sites outside this module still use string literals (files owned
    /// by other work streams). Pin them here so a renamed value becomes a test
    /// failure instead of silent drift.
    /// Mirrors `isQuietStatus` in `src/lib/conversation.ts`: a rewind's
    /// mid-turn guard and the clients' poll cadence must agree on what
    /// "between turns" means. `blocked` is inside a turn.
    #[test]
    fn quiet_is_idle_completed_stopped_failed_and_never_working_or_blocked() {
        let quiet: Vec<&str> = ClaudeStatus::ALL
            .iter()
            .filter(|s| s.is_quiet())
            .map(|s| s.as_str())
            .collect();
        assert_eq!(quiet, ["completed", "failed", "stopped", "idle"]);
    }

    #[test]
    fn external_write_site_literals_are_in_vocabulary() {
        // service/hooks.rs: the Stop hook stamps "idle".
        assert_eq!("idle".parse::<ClaudeStatus>().unwrap(), ClaudeStatus::Idle);
        // claude_agents.rs: the documented `claude agents --json` status set.
        for lit in [
            "working",
            "blocked",
            "completed",
            "failed",
            "stopped",
            "idle",
        ] {
            assert!(
                lit.parse::<ClaudeStatus>().is_ok(),
                "{lit} not in vocabulary"
            );
        }
        // `claude_status` filter values callers pass to list_sessions and
        // broadcast_prompt must be real values, so the doc examples parse.
        assert!("idle".parse::<ClaudeStatus>().is_ok());
        assert!("working".parse::<ClaudeStatus>().is_ok());
    }
}
