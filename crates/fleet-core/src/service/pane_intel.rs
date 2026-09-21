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
const ACTIVITY_MAX: usize = 200;

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
    pub label: String,
    pub selected: bool,
}

/// The permission/question dialog a blocked pane is showing, as stored on
/// `sessions.pending_input`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingInput {
    /// `"permission"` | `"input"` (mirrors [`WaitingFor::as_str`]).
    pub kind: String,
    pub question: Option<String>,
    pub options: Vec<PendingOption>,
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
fn strip_ansi(s: &str) -> String {
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

/// True iff `needle` occurs in `haystack` delimited by non-alphanumeric
/// boundaries — so "oom" matches "(oom)" or "killed: oom" but NOT "zoom",
/// "room", or "boom". Used to keep the bare OOM acronym from false-matching
/// innocent words.
fn contains_word(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(i, _)| {
        let before_ok = haystack[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let after_ok = haystack[i + needle.len()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        before_ok && after_ok
    })
}

/// Detect a stuck state from the (already ANSI-stripped) tail. First match wins,
/// ordered most-specific first.
fn detect_stuck(text: &str) -> Option<StuckKind> {
    let lower = text.to_lowercase();

    // OOM / allocation failure. The acronym is matched only as a whole word
    // (plus the explicit "oomkilled") so prose like "zoom"/"room" can't trip it.
    if lower.contains("out of memory")
        || lower.contains("cannot allocate memory")
        || lower.contains("javascript heap out of memory")
        || lower.contains("fatal error: reached heap limit")
        || lower.contains("oomkilled")
        || contains_word(&lower, "oom")
    {
        return Some(StuckKind::Oom);
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

    /// What gets stored on `sessions.pending_input`.
    fn pending_input(&self) -> PendingInput {
        PendingInput {
            kind: self.kind.as_str().into(),
            question: self.prompt.clone(),
            options: self.options.clone(),
        }
    }
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
fn parse_choice(line: &str) -> Option<(u8, &str, bool)> {
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
/// * question: an "Enter to select" hint plus a selected numbered choice.
///
/// Because a dialog replaces the REPL's input box and footer, a live-REPL
/// cue ([`LIVE_REPL_CUES`], or an input prompt line `❯ …` that is not a
/// choice) BELOW the last dialog line means the text is scrollback, and no
/// dialog is reported.
fn detect_dialog(stripped: &str) -> Option<Dialog> {
    let lines: Vec<&str> = stripped.lines().map(clean_line).collect();
    let lower: Vec<String> = lines.iter().map(|l| l.to_lowercase()).collect();
    // (line index, ordinal, label, selected) for every numbered-choice line.
    let choices: Vec<(usize, u8, &str, bool)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| parse_choice(l).map(|(n, label, sel)| (i, n, label, sel)))
        .collect();
    let is_choice = |i: usize| choices.iter().any(|(j, ..)| *j == i);
    let last = |pred: &dyn Fn(&str) -> bool| lower.iter().rposition(|l| pred(l));

    let tell_claude = last(&|l| l.contains("no, and tell claude"));
    let ask = last(&|l| l.contains("do you want to") || l.contains("would you like to"));
    let select_hint = last(&|l| l.contains("enter to select"));

    let kind = if tell_claude.is_some()
        || ask.is_some_and(|a| choices.iter().filter(|(j, ..)| *j > a).count() >= 2)
    {
        WaitingFor::Permission
    } else if select_hint.is_some() && choices.iter().any(|(_, _, _, sel)| *sel) {
        WaitingFor::Input
    } else {
        return None;
    };

    let dialog_end = [tell_claude, ask, select_hint, choices.last().map(|c| c.0)]
        .into_iter()
        .flatten()
        .max()?;
    let live_below = lower.iter().enumerate().skip(dialog_end + 1).any(|(i, l)| {
        !is_choice(i) && (LIVE_REPL_CUES.iter().any(|c| l.contains(c)) || lines[i].starts_with('❯'))
    });
    if live_below {
        return None;
    }

    let question = lines[..=dialog_end]
        .iter()
        .enumerate()
        .rev()
        .find(|(i, l)| l.ends_with('?') && !is_choice(*i))
        .map(|(_, l)| l.to_string());
    let selected = choices
        .iter()
        .find(|(_, _, _, sel)| *sel)
        .map(|(i, ..)| lines[*i].trim_start_matches(['❯', '›']).trim().to_string());
    // `options` is bounded to the dialog's OWN choice block: the trailing
    // contiguous run of choice lines at or before `dialog_end`, not every
    // numbered line in the captured tail. Without this, a numbered list
    // further up the scrollback (an agent's own "2. Add the guard / 3. Run
    // the tests" text) leaks in, duplicating `n` and confusing the buttons
    // a client builds from it.
    let options = match choices.last() {
        Some((last_idx, ..)) => {
            let mut start = *last_idx;
            while start > 0 && is_choice(start - 1) {
                start -= 1;
            }
            choices
                .iter()
                .filter(|(i, ..)| *i >= start)
                .map(|(_, n, label, selected)| PendingOption {
                    n: *n,
                    label: (*label).to_string(),
                    selected: *selected,
                })
                .collect()
        }
        None => Vec::new(),
    };
    Some(Dialog {
        kind,
        prompt: question.or(selected),
        options,
    })
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
    fn oom_matches_real_signals_not_innocent_words() {
        // Real OOM signals are still detected.
        assert_eq!(
            analyze("Killed process 123 (OOM)").stuck,
            Some(StuckKind::Oom)
        );
        assert_eq!(
            analyze("container terminated reason=OOMKilled").stuck,
            Some(StuckKind::Oom)
        );
        // Innocent words that merely contain the letters "oom" must NOT trip it
        // (this was the false-positive: e.g. a session discussing Zoom).
        assert_eq!(analyze("Let's zoom into the room — boom!").stuck, None);
        assert_eq!(analyze("⏺ Joining the Zoom meeting room").stuck, None);
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

    fn assert_dialog(name: &str, text: &str, kind: WaitingFor, activity: &str) {
        let intel = fixture_intel(name, text);
        assert_eq!(intel.derived_status, Some(ClaudeStatus::Blocked), "{name}");
        assert_eq!(intel.waiting_for, Some(kind), "{name}");
        assert_eq!(intel.activity.as_deref(), Some(activity), "{name}");
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
    }

    #[test]
    fn boxed_edit_permission_dialog_is_blocked_on_permission() {
        assert_dialog(
            "permission_edit_boxed",
            include_str!("testdata/pane_intel/permission_edit_boxed.txt"),
            WaitingFor::Permission,
            "waiting for permission: Do you want to make this edit to health.rs?",
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
        );
    }

    #[test]
    fn ask_user_question_menu_is_blocked_on_input() {
        assert_dialog(
            "question_ask_user",
            include_str!("testdata/pane_intel/question_ask_user.txt"),
            WaitingFor::Input,
            "waiting for input: Keep ghosted sessions for how long before deleting them?",
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
                selected: true
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
