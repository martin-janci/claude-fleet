//! Control's autonomy position, on the Rust side (review 2026-10-17, F3).
//!
//! The dial in Control's composer (`src/lib/autonomy.ts`) is one chip over
//! four settings: the mission loop's ceiling (`orchestrator.max_level`),
//! auto-continue and its delay (`control.auto_continue`,
//! `control.auto_continue_minutes`) and the event feed
//! (`control.react_to_events`). [`position_of`] reads them the way
//! `autonomyOf` does, so the hub decides by the same position the person
//! sees on the chip: the operator guard pre-approves by it (F2,
//! `mcp::guard::operator_must_confirm`), and Control is told it in its
//! CLAUDE.md ([`SECTION`]) and in every event digest's header
//! ([`digest_header`]).
//!
//! `Custom` (a mix the dial does not name) behaves as `Ask` wherever a
//! position grants something ([`Position::grants`]): a person who set the
//! four settings apart by hand did not pick a position that lets Control act
//! alone.

use crate::service::settings;
use crate::store::Store;

/// Where the dial is. Mirrors `AutonomyId | 'custom'` in `src/lib/autonomy.ts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    /// Ask first: every step a person approves.
    Ask,
    /// Within your brief: prompts and tasks to Control's own open handoffs
    /// go without a card.
    Brief,
    /// Autopilot: also new sessions inside a running mission's grant.
    Autopilot,
    /// A mix of the four settings the dial does not name.
    Custom,
}

/// One named position's settings, the rows of `AUTONOMY` in
/// `src/lib/autonomy.ts` (`autonomy_positions_mirror_the_dial` keeps them
/// in step).
pub struct Preset {
    pub position: Position,
    pub id: &'static str,
    pub label: &'static str,
    pub max_level: u64,
    pub auto_continue: bool,
    /// Read only when `auto_continue`.
    pub minutes: u64,
    pub react_to_events: bool,
}

pub const PRESETS: [Preset; 3] = [
    Preset {
        position: Position::Ask,
        id: "ask",
        label: "Ask first",
        max_level: 1,
        auto_continue: false,
        minutes: 10,
        react_to_events: true,
    },
    Preset {
        position: Position::Brief,
        id: "brief",
        label: "Within your brief",
        max_level: 2,
        auto_continue: true,
        minutes: 10,
        react_to_events: true,
    },
    Preset {
        position: Position::Autopilot,
        id: "autopilot",
        label: "Autopilot",
        max_level: 3,
        auto_continue: true,
        minutes: 2,
        react_to_events: true,
    },
];

impl Position {
    /// The id the desktop uses (`AutonomyId | 'custom'`).
    pub fn id(self) -> &'static str {
        match self {
            Position::Ask => "ask",
            Position::Brief => "brief",
            Position::Autopilot => "autopilot",
            Position::Custom => "custom",
        }
    }

    /// The chip's label.
    pub fn label(self) -> &'static str {
        match self {
            Position::Ask => "Ask first",
            Position::Brief => "Within your brief",
            Position::Autopilot => "Autopilot",
            Position::Custom => "Custom",
        }
    }

    /// The position whose grants apply: `Custom` grants what `Ask` does.
    pub fn grants(self) -> Position {
        match self {
            Position::Custom => Position::Ask,
            p => p,
        }
    }

    /// True at Ask first, and at Custom, which acts as it.
    pub fn is_ask_first(self) -> bool {
        self.grants() == Position::Ask
    }

    /// What Control may do alone here, one line (the digest header's).
    pub fn rule(self) -> &'static str {
        match self.grants() {
            Position::Brief => {
                "prompts and tasks to sessions you started or briefed go without a card; \
                 anything new still asks"
            }
            Position::Autopilot => {
                "prompts and tasks to your sessions, and new sessions inside a running \
                 mission's grant, go without a card; anything else still asks"
            }
            _ => "every start, prompt and kill waits for a person's approval",
        }
    }
}

/// PURE: the position four setting values name, as `autonomyOf` reads them.
pub fn position_of(
    max_level: u64,
    auto_continue: bool,
    minutes: u64,
    react_to_events: bool,
) -> Position {
    PRESETS
        .iter()
        .find(|p| {
            p.max_level == max_level
                && p.auto_continue == auto_continue
                && (!auto_continue || p.minutes == minutes)
                && p.react_to_events == react_to_events
        })
        .map_or(Position::Custom, |p| p.position)
}

/// The position the store's settings put the dial at.
pub fn position(s: &Store) -> Position {
    position_of(
        settings::get_secs(s, settings::ORCHESTRATOR_MAX_LEVEL),
        settings::get_bool(s, settings::CONTROL_AUTO_CONTINUE),
        settings::get_secs(s, settings::CONTROL_AUTO_CONTINUE_MINUTES),
        settings::get_bool(s, settings::CONTROL_REACT_TO_EVENTS),
    )
}

/// PURE: the line every `[fleet events]` digest carries under its mark,
/// telling Control where the dial is now (F3) — the CLAUDE.md section says
/// what each position means, this says which one holds.
pub fn digest_header(p: Position) -> String {
    format!("Autonomy: {} — {}.", p.label(), p.rule())
}

/// PURE: the line under the header when session care is on and the
/// position lets it act (`control.session_care`,
/// `mcp::guard::OPERATOR_CARE_TOOLS`); `None` otherwise.
pub fn care_line(p: Position, care: bool) -> Option<&'static str> {
    if !care {
        return None;
    }
    match p.grants() {
        Position::Brief => Some(
            "Session care: on — restarting a session you started yourself goes without a card.",
        ),
        Position::Autopilot => Some(
            "Session care: on — restarting a session you started yourself, and retiring one \
             with safe_kill_session once its work is done, go without a card.",
        ),
        _ => None,
    }
}

/// PURE: [`with_header`], plus [`care_line`] under the header when it
/// applies.
pub fn with_header_care(digest: &str, p: Position, care: bool) -> String {
    let headed = with_header(digest, p);
    let Some(line) = care_line(p, care) else {
        return headed;
    };
    let header = digest_header(p);
    headed.replacen(&header, &format!("{header}\n{line}"), 1)
}

/// PURE: `digest` with [`digest_header`] as its second line, right under
/// the `[fleet events]` line, so the mark still opens it.
pub fn with_header(digest: &str, p: Position) -> String {
    match digest.split_once('\n') {
        Some((first, rest)) => format!("{first}\n{}\n{rest}", digest_header(p)),
        None => format!("{digest}\n{}", digest_header(p)),
    }
}

/// Control's CLAUDE.md section on the dial (`operator::CLAUDE_MD_SECTIONS`).
pub const SECTION: &str = "\
## Autonomy

The person sets how far you may go alone with the autonomy chip under your \
composer. Every `[fleet events]` digest says where it is now (`Autonomy: …`); \
until one arrives, act as at Ask first.

- **Ask first** (and **Custom**, a mix the chip does not name): every start, \
  prompt, task and kill you make waits for a person's approval on a card.
- **Within your brief**: `send_prompt`, `queue_prompt`, `run_prompt` and \
  `dispatch_task` to a session you started or briefed, while that handoff is \
  open, go without a card. New sessions, kills, restarts, rewinds, restores \
  and new projects still ask.
- **Autopilot**: also starting a session inside a running mission's grant \
  (its hosts, hours and budget), with `new_session`, `dispatch_task` or a \
  task's `work_link` `start`, `run` or `resume` (name the host, or the grant \
  must name none). Nothing else changes: kills, restarts, rewinds, restores \
  and new projects ask.
- **Session care** (a setting, off unless the digest says `Session care: \
  on`): at Within your brief, `restart_session` of a session you started or \
  briefed goes without a card (a wedged REPL, or back under another login \
  with `profile` after a limit); at Autopilot, so does retiring one with \
  `safe_kill_session` once its work is pushed or merged. `kill_session`, \
  rewinds and moves always ask.

A call that went without a card is still audited and shows in the chat as \
pre-approved, so say what you did in one line. When a call returns \
`E_CONFIRM_REQUIRED`, stop and tell the person what waits for them. Do not \
retry it: when they approve, fleet runs the call itself and tells you in a \
`[fleet] approved: …` note (or `[fleet] denied: …`), and you carry on from \
there.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_preset_reads_back_as_itself_and_anything_else_is_custom() {
        for p in &PRESETS {
            assert_eq!(
                position_of(p.max_level, p.auto_continue, p.minutes, p.react_to_events),
                p.position,
                "{}",
                p.id
            );
            assert_eq!(p.position.id(), p.id);
            assert_eq!(p.position.label(), p.label);
        }
        // Ask first ignores the minutes, as `autonomyOf` does.
        assert_eq!(position_of(1, false, 99, true), Position::Ask);
        assert_eq!(position_of(2, true, 3, true), Position::Custom);
        assert_eq!(position_of(3, true, 2, false), Position::Custom);
        assert_eq!(position_of(0, false, 10, true), Position::Custom);
    }

    #[test]
    fn custom_grants_what_ask_first_does() {
        assert_eq!(Position::Custom.grants(), Position::Ask);
        assert!(Position::Custom.is_ask_first());
        assert!(!Position::Brief.is_ask_first());
        assert_eq!(Position::Custom.rule(), Position::Ask.rule());
    }

    #[test]
    fn a_fresh_store_grants_what_ask_first_does() {
        let s = Store::open_in_memory().unwrap();
        // The shipped defaults (auto-continue on at level 1) are a mix the
        // dial does not name: Custom, which grants what Ask first does.
        assert!(position(&s).is_ask_first());
        s.set_setting(settings::CONTROL_AUTO_CONTINUE, "false")
            .unwrap();
        assert_eq!(position(&s), Position::Ask);
        s.set_setting(settings::ORCHESTRATOR_MAX_LEVEL, "2")
            .unwrap();
        s.set_setting(settings::CONTROL_AUTO_CONTINUE, "true")
            .unwrap();
        assert_eq!(position(&s), Position::Brief);
    }

    #[test]
    fn session_care_shows_only_where_it_acts() {
        let d = "[fleet events] What changed.\n- a";
        assert_eq!(
            with_header_care(d, Position::Brief, false),
            with_header(d, Position::Brief)
        );
        assert_eq!(
            with_header_care(d, Position::Ask, true),
            with_header(d, Position::Ask)
        );
        assert_eq!(
            with_header_care(d, Position::Custom, true),
            with_header(d, Position::Custom)
        );
        let b = with_header_care(d, Position::Brief, true);
        let lines: Vec<&str> = b.lines().collect();
        assert_eq!(lines[0], "[fleet events] What changed.");
        assert!(lines[1].starts_with("Autonomy: Within your brief"), "{b}");
        assert!(lines[2].starts_with("Session care: on"), "{b}");
        assert!(!lines[2].contains("safe_kill_session"), "{b}");
        assert_eq!(lines[3], "- a");
        let a = with_header_care(d, Position::Autopilot, true);
        assert!(
            a.lines().nth(2).unwrap().contains("safe_kill_session"),
            "{a}"
        );
        assert!(SECTION.contains("Session care: on"));
    }

    #[test]
    fn the_digest_header_names_the_position() {
        let h = digest_header(Position::Brief);
        assert!(h.starts_with("Autonomy: Within your brief"), "{h}");
        let d = with_header("[fleet events] What changed.\n- a\n- b", Position::Custom);
        let lines: Vec<&str> = d.lines().collect();
        assert_eq!(lines[0], "[fleet events] What changed.");
        assert!(
            lines[1].starts_with("Autonomy: Custom — every start"),
            "{d}"
        );
        assert_eq!(&lines[2..], ["- a", "- b"]);
        assert!(with_header("[fleet events] x", Position::Ask).ends_with("approval."));
    }

    /// `src/lib/autonomy.ts` holds the same three rows: a change to one side
    /// alone would make the hub and the chip disagree about the position.
    #[test]
    fn autonomy_positions_mirror_the_dial() {
        let ts = crate::repo_files::read("src/lib/autonomy.ts");
        for p in &PRESETS {
            let at = ts
                .find(&format!("id: '{}'", p.id))
                .unwrap_or_else(|| panic!("{} is not in autonomy.ts", p.id));
            let row = &ts[at..at + ts[at..].find("},").expect("row end")];
            for want in [
                format!("label: '{}'", p.label),
                format!("maxLevel: {}", p.max_level),
                format!("autoContinue: {}", p.auto_continue),
                format!("minutes: {}", p.minutes),
                format!("reactToEvents: {}", p.react_to_events),
            ] {
                assert!(row.contains(&want), "{}: {want} not in {row}", p.id);
            }
        }
    }
}
