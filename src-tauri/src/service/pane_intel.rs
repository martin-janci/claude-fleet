//! Pure parsers for a captured pane tail.
//!
//! `reconcile_sessions` reads the last few lines of each work session's tmux
//! pane (`tmux capture-pane -p -S -8`) and feeds them to [`analyze`]. The result
//! drives four session fields: `current_activity`, a derived `claude_status`, a
//! `stuck_kind`, and `context_pct`. Everything here is side-effect-free and
//! heavily unit-tested; the reconcile wiring that calls it is intentionally thin.
//!
//! All parsers return `None` rather than guess. A misread pane that silently
//! produced a wrong "blocked" status or a bogus context % would be worse than no
//! signal at all, since Wave-2 self-heal will eventually act on these.

/// Cap on the stored activity string so a runaway pane line can't bloat a row.
const ACTIVITY_MAX: usize = 200;

/// Stuck states detectable from the pane tail. Detection only — auto-remedy
/// keystrokes are a deliberately-deferred follow-up (see plan self-review).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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
    /// Inferred status: `"working"` | `"idle"` | `"blocked"` | `None`.
    /// Only a *fallback* — the authoritative status comes from `claude agents`.
    pub derived_status: Option<&'static str>,
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
    let start = trimmed
        .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map(|i| i + 1)
        .unwrap_or(0);
    let num = &trimmed[start..];
    if num.is_empty() {
        return None;
    }
    num.parse::<f64>().ok()
}

/// Detect a stuck state from the (already ANSI-stripped) tail. First match wins,
/// ordered most-specific first.
fn detect_stuck(text: &str) -> Option<StuckKind> {
    let lower = text.to_lowercase();

    // OOM / allocation failure.
    if lower.contains("out of memory")
        || lower.contains("oom")
        || lower.contains("cannot allocate memory")
        || lower.contains("javascript heap out of memory")
        || lower.contains("fatal error: reached heap limit")
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

/// Derive a coarse status from the tail. Used ONLY as a fallback when the
/// authoritative `claude agents` status is absent.
fn derive_status(stuck: Option<StuckKind>, stripped: &str) -> Option<&'static str> {
    if stuck.is_some() {
        return Some("blocked");
    }
    let lower = stripped.to_lowercase();
    if lower.trim().is_empty() {
        return None;
    }
    // Active spinner / in-progress markers seen in the REPL.
    let working = lower.contains("esc to interrupt")
        || lower.contains("in progress…")
        || lower.contains("in progress...")
        || lower.contains("tool use")
        || lower.contains("running…")
        || lower.contains("running...")
        // A tool-call line: "⏺ Bash(...)" / "⏺ Read(...)".
        || stripped.contains('⏺');
    if working {
        return Some("working");
    }
    // A prompt-ready footer ("? for shortcuts", the "N% used" status bar) means
    // the REPL is sitting idle waiting for input.
    if lower.contains("? for shortcuts") || lower.contains("% used") {
        return Some("idle");
    }
    None
}

/// Analyze a captured pane tail into the four reconcile signals.
pub fn analyze(pane_tail: &str) -> PaneIntel {
    let stripped = strip_ansi(pane_tail);
    let stuck = detect_stuck(&stripped);
    let context_pct = parse_context_pct(&stripped);
    let activity = pick_activity(&stripped);
    let derived_status = derive_status(stuck, &stripped);
    PaneIntel {
        activity,
        stuck,
        context_pct,
        derived_status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(intel.derived_status, Some("blocked"));
    }

    #[test]
    fn auth_menu_is_blocked() {
        let intel =
            analyze("Select login method:\n  1. Claude account with subscription\n  2. API key\n");
        assert_eq!(intel.stuck, Some(StuckKind::AuthMenu));
        assert_eq!(intel.derived_status, Some("blocked"));
    }

    #[test]
    fn trust_prompt_detected() {
        let intel =
            analyze("Do you trust the files in this folder?\n  ❯ 1. Yes, proceed\n  2. No\n");
        assert_eq!(intel.stuck, Some(StuckKind::TrustPrompt));
        assert_eq!(intel.derived_status, Some("blocked"));
    }

    #[test]
    fn press_enter_detected() {
        let intel = analyze("Update available.\nPress Enter to continue\n");
        assert_eq!(intel.stuck, Some(StuckKind::PressEnter));
        assert_eq!(intel.derived_status, Some("blocked"));
    }

    #[test]
    fn oom_detected() {
        let intel =
            analyze("<--- Last few GCs --->\nFATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory\n");
        assert_eq!(intel.stuck, Some(StuckKind::Oom));
        assert_eq!(intel.derived_status, Some("blocked"));
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
    fn normal_tool_line_is_activity_and_working() {
        let tail = "⏺ Bash(cargo test)\n  ⎿ Running…\n";
        let intel = analyze(tail);
        assert!(intel.stuck.is_none());
        assert_eq!(intel.derived_status, Some("working"));
        let activity = intel.activity.expect("activity");
        assert!(activity.contains("Running") || activity.contains("⏺"));
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
}
