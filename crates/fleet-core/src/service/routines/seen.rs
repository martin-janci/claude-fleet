//! Routine seen items (migration 212): what a routine's runs already
//! handled, so a sweep that runs every morning does not review, or report,
//! the same pull request again while nothing about it changed.
//!
//! * **Shown.** A run's prompt lists the items earlier runs handled, with
//!   the state each was in then, fenced as untrusted text (runs write them,
//!   and runs read pull requests), and asks the run to skip an item whose
//!   state is the same now ([`prompt_block`]). `dry_run` shows the same.
//! * **Reported.** A finished run's reply is read off its transcript with
//!   the notebooks' read (`memory::read_finished`): its last
//!   `<routine-seen>` block names every item it looked at, one per line,
//!   `item | state` ([`parse_items`]). The fleet compares each state with
//!   the one it kept: new, changed or the same. The comparison is the
//!   fleet's, never the model's.
//! * **Quiet.** A run whose block names nothing new or changed came to
//!   `nothing` (a rule, `outcome`), so its finished turn stays out of the
//!   Inbox; Jev's reading of the screen is not needed for it.
//! * **Kept.** An item no run reported for [`KEEP_SECS`] is dropped, and a
//!   routine keeps at most [`MAX_ITEMS`], the last seen.
//!
//! A routine tracks them unless a person turns it off (`track_seen`); a
//! person may forget one item, or all, so the next run looks again.

use super::outcome::{self, OutcomeSource, RunOutcome};
use super::{changeable, trace, visible};
use crate::ipc_error::{lock, IpcError};
use crate::service::run_trace::Step;
use crate::service::view_scope::ViewScope;
use crate::store::{RoutineRow, RoutineRunRow, RoutineSeenRow, SeenChange, Store};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// An item no run reported for this long is forgotten.
pub const KEEP_SECS: i64 = 30 * 86_400;
/// The most items a routine keeps.
pub const MAX_ITEMS: i64 = 500;
/// The most items a run's prompt lists, the last seen first.
pub const SHOWN_MAX: i64 = 200;
/// The most items one reply reports.
pub const REPORTED_MAX: usize = 200;
/// An item's name, in characters.
pub const ITEM_MAX_CHARS: usize = 160;
/// An item's state, in characters.
pub const STATE_MAX_CHARS: usize = 240;
/// The prompt's list, in characters.
pub const LIST_MAX_CHARS: usize = 16_000;

const TAG_OPEN: &str = "<routine-seen>";
const TAG_CLOSE: &str = "</routine-seen>";

/// The items a run of `r` is shown, the last seen first; `None` when `r`
/// tracks none.
pub fn shown(s: &Store, r: &RoutineRow) -> Result<Option<Vec<RoutineSeenRow>>, IpcError> {
    if !r.track_seen {
        return Ok(None);
    }
    s.routine_seen(r.id, SHOWN_MAX).map(Some)
}

/// PURE: the block a run's prompt carries for `items`.
pub fn prompt_block(items: &[RoutineSeenRow]) -> String {
    let mut out = String::from(
        "Already handled: claude-fleet remembers the items (pull requests, issues, alerts) \
         this routine's earlier runs handled, with the state each was in then.",
    );
    if items.is_empty() {
        out.push_str(" None yet.");
    } else {
        let list = items
            .iter()
            .map(|i| format!("{} | {}", i.item, i.state))
            .collect::<Vec<_>>()
            .join("\n");
        let fenced =
            crate::mcp::guard::fence_untrusted(&list, "routine seen items", LIST_MAX_CHARS);
        out.push_str(&format!(
            " Skip an item whose state is the same now: do not review or report it \
             again. Look at it, and report it, only when its state changed.\n{fenced}"
        ));
    }
    out.push_str(&format!(
        "\n\nEnd your final reply with {TAG_OPEN} and {TAG_CLOSE}, one line between them \
         for each item you looked at this run, skipped ones included: `item | state`. \
         The item is a stable name (`pr owner/repo#12`, `issue owner/repo#40`), the state \
         a short fingerprint of what you saw that changes when the item does (for a pull \
         request `head=<short sha> comments=<n> reviews=<n> checks=<pass|fail|pending>`). \
         An empty block says there was nothing to look at."
    ));
    out
}

/// PURE: `a` and `b` as one block of a prompt, either one alone, or none.
pub fn joined(a: Option<&str>, b: Option<&str>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => Some(format!("{a}\n\n{b}")),
        (Some(x), None) | (None, Some(x)) => Some(x.to_string()),
        (None, None) => None,
    }
}

/// PURE: the text of the last `<routine-seen>` block of `text` that is
/// closed, a tag quoted as code (`` `<routine-seen>` ``) left out.
pub fn last_block(text: &str) -> Option<&str> {
    let mut found = None;
    let mut from = 0;
    while let Some(i) = text[from..].find(TAG_OPEN) {
        let open = from + i;
        from = open + TAG_OPEN.len();
        if text[..open].ends_with('`') {
            continue;
        }
        if let Some(close) = text[from..].find(TAG_CLOSE) {
            found = Some(&text[from..from + close]);
        }
    }
    found
}

/// PURE: `s` with its runs of whitespace made one space, cut to `max`
/// characters.
fn tidy(s: &str, max: usize) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

/// PURE: the items of a block, as `(item, state)`: one per line, `item |
/// state` (a line without `|` is an item with no state), list marks and
/// code quotes dropped, the item in lower case so `PR Owner/Repo#12` and
/// `pr owner/repo#12` are one. A repeated item keeps its last line.
pub fn parse_items(block: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in block.lines() {
        let line = line
            .trim()
            .trim_start_matches(['-', '*', '•'])
            .trim()
            .trim_matches('`')
            .trim();
        if line.is_empty() {
            continue;
        }
        let (item, state) = match line.split_once('|') {
            Some((i, st)) => (i, st),
            None => (line, ""),
        };
        let item = tidy(item, ITEM_MAX_CHARS).to_lowercase();
        if item.is_empty() {
            continue;
        }
        let state = tidy(state.trim().trim_matches('`'), STATE_MAX_CHARS);
        out.retain(|(i, _)| *i != item);
        out.push((item, state));
        if out.len() >= REPORTED_MAX {
            break;
        }
    }
    out
}

/// What a run's report changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    pub new: usize,
    pub changed: usize,
    pub same: usize,
}

impl Tally {
    /// Nothing new and nothing changed.
    pub fn quiet(&self) -> bool {
        self.new == 0 && self.changed == 0
    }
}

/// PURE: what a trace says of `t`.
pub fn summary(t: &Tally) -> String {
    if t.new + t.changed + t.same == 0 {
        return "Its reply listed nothing to look at".to_string();
    }
    let mut parts = Vec::new();
    for (n, what) in [
        (t.new, "new"),
        (t.changed, "changed"),
        (t.same, "unchanged"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {what}"));
        }
    }
    format!("Seen items: {}", parts.join(", "))
}

/// Apply finished run `run` of `r`'s reply (`texts`, oldest first) to its
/// seen items: each item of its last block is recorded, the old ones
/// dropped, and a done run that found nothing new or changed comes to
/// `nothing`. `None`: `r` tracks none, or the reply has no block.
pub fn apply(
    s: &Store,
    r: &RoutineRow,
    run: &RoutineRunRow,
    texts: &[String],
    now: i64,
) -> Result<Option<Tally>, IpcError> {
    if !r.track_seen || run.state != "done" {
        return Ok(None);
    }
    let Some(block) = texts.iter().rev().find_map(|t| last_block(t)) else {
        trace::write_one(
            s,
            run.id,
            Step::new(
                now,
                "outcome",
                "seen",
                "hub",
                "Its reply listed no seen items",
            )
            .data(serde_json::json!({ "block": false })),
        );
        return Ok(None);
    };
    let mut tally = Tally::default();
    for (item, state) in parse_items(block) {
        match s.report_routine_seen(r.id, &item, &state, Some(run.id), now)? {
            SeenChange::New => tally.new += 1,
            SeenChange::Changed => tally.changed += 1,
            SeenChange::Same => tally.same += 1,
        }
    }
    s.prune_routine_seen(r.id, now - KEEP_SECS, MAX_ITEMS)?;
    trace::write_one(
        s,
        run.id,
        Step::new(now, "outcome", "seen", "hub", summary(&tally)).data(serde_json::json!({
            "block": true,
            "new": tally.new,
            "changed": tally.changed,
            "same": tally.same,
        })),
    );
    if tally.quiet() {
        quiet(s, run.id, now)?;
    }
    Ok(Some(tally))
}

/// Run `run_id` found nothing new: it came to `nothing`, unless something
/// already said otherwise (a pull request it opened, a question it asked,
/// a person). Jev's guess gives way, except one that it waits on a person.
fn quiet(s: &Store, run_id: i64, now: i64) -> Result<(), IpcError> {
    let Some(run) = s.get_routine_run(run_id)? else {
        return Ok(());
    };
    let open = match (run.outcome.as_deref(), run.outcome_source.as_deref()) {
        (None, _) => true,
        (Some(o), Some("jev")) => o != RunOutcome::NeedsPerson.as_str(),
        _ => false,
    };
    if open {
        outcome::record(s, &run, RunOutcome::Nothing, OutcomeSource::Rule, now)?;
    }
    Ok(())
}

/// `routines { action: seen, routine_id, limit? }`: the items a routine's
/// runs handled, the last seen first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineSeen {
    pub routine_id: i64,
    pub track_seen: bool,
    pub items: Vec<RoutineSeenRow>,
    /// How many it keeps, all of them.
    pub total: i64,
    /// Whether this caller may forget them.
    #[serde(default)]
    pub may_change: bool,
}

fn of_routine(
    s: &Store,
    scope: &ViewScope,
    r: &RoutineRow,
    limit: Option<i64>,
) -> Result<RoutineSeen, IpcError> {
    let limit = limit.unwrap_or(SHOWN_MAX).clamp(1, MAX_ITEMS);
    Ok(RoutineSeen {
        routine_id: r.id,
        track_seen: r.track_seen,
        items: s.routine_seen(r.id, limit)?,
        total: s.routine_seen_count(r.id)?,
        may_change: super::may_change_routine(s, scope, r)?,
    })
}

/// `routines { action: seen }`, for anyone who may read the routine.
pub fn get(
    store: &Mutex<Store>,
    scope: &ViewScope,
    id: i64,
    limit: Option<i64>,
) -> Result<RoutineSeen, IpcError> {
    let s = lock(store)?;
    let r = visible(&s, scope, id)?;
    of_routine(&s, scope, &r, limit)
}

/// `routines { action: forget_seen, routine_id, text? }`: a person forgets
/// item `text`, or every item without one, so the next run looks again.
/// The same fence as changing the routine.
pub fn forget(
    store: &Mutex<Store>,
    scope: &ViewScope,
    id: i64,
    item: Option<&str>,
) -> Result<RoutineSeen, IpcError> {
    let s = lock(store)?;
    let r = changeable(&s, scope, id)?;
    let item = item.map(|i| tidy(i, ITEM_MAX_CHARS).to_lowercase());
    s.forget_routine_seen(r.id, item.as_deref().filter(|i| !i.is_empty()))?;
    of_routine(&s, scope, &r, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_closed_block_is_read_and_a_quoted_tag_is_not() {
        let text = "I list them in a `<routine-seen>` block.\n\
                    <routine-seen>\npr a/b#1 | head=1\n</routine-seen>\n\
                    then again:\n<routine-seen>\npr a/b#2 | head=2\n</routine-seen>\n\
                    <routine-seen> never closed";
        assert_eq!(last_block(text), Some("\npr a/b#2 | head=2\n"));
        assert_eq!(last_block("no block here"), None);
        assert_eq!(last_block("`<routine-seen>`x</routine-seen>"), None);
    }

    #[test]
    fn items_are_one_per_line_named_in_lower_case() {
        let items = parse_items(
            "\n- PR Owner/Repo#12 | head=abc  comments=3\n\
             * `issue o/r#40 | open`\n\
             pr o/r#13\n\
             \n\
             pr owner/repo#12 | head=def\n",
        );
        assert_eq!(
            items,
            vec![
                ("issue o/r#40".to_string(), "open".to_string()),
                ("pr o/r#13".to_string(), String::new()),
                ("pr owner/repo#12".to_string(), "head=def".to_string()),
            ]
        );
    }

    #[test]
    fn the_prompt_lists_the_items_fenced_and_asks_for_the_block() {
        let row = RoutineSeenRow {
            item: "pr o/r#12".into(),
            state: "head=abc".into(),
            first_seen_at: 1,
            seen_at: 2,
            changed_at: 1,
            run_id: Some(3),
        };
        let p = prompt_block(&[row]);
        assert!(p.contains("pr o/r#12 | head=abc"), "{p}");
        assert!(
            p.contains(&crate::mcp::guard::untrusted_marker("routine seen items")),
            "{p}"
        );
        assert!(p.contains("<routine-seen>"), "{p}");
        let empty = prompt_block(&[]);
        assert!(empty.contains("None yet."), "{empty}");
        assert!(!empty.contains("Skip an item"), "{empty}");
    }

    #[test]
    fn a_tally_says_what_changed() {
        let t = Tally {
            new: 1,
            changed: 0,
            same: 4,
        };
        assert_eq!(summary(&t), "Seen items: 1 new, 4 unchanged");
        assert!(!t.quiet());
        assert!(Tally::default().quiet());
        assert_eq!(
            summary(&Tally::default()),
            "Its reply listed nothing to look at"
        );
        assert_eq!(joined(Some("a"), None).as_deref(), Some("a"));
        assert_eq!(joined(Some("a"), Some("b")).as_deref(), Some("a\n\nb"));
        assert_eq!(joined(None, None), None);
    }
}
