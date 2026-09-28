//! The fleet's quick replies: the chip row every composer draws above its
//! prompt box.
//!
//! # Why this is fleet state and not a device preference
//!
//! Both composers had their own copy. The desktop kept `{label, text}` pairs
//! in `localStorage` (`src/lib/composer_presets.ts`), fleet-mobile kept a
//! list of plain strings in the device's `Prefs` — documented there as
//! "never sent to the hub". So the same person editing the same row of
//! buttons got two unrelated lists, and a phone reinstall lost its one
//! silently. A chip is a prompt you send to YOUR fleet from whichever screen
//! is in front of you; it belongs to the fleet, not to the glass.
//!
//! One list, stored once, read by every client. That is also why `replace`
//! takes the whole list rather than offering add/remove verbs: two devices
//! editing at once should lose one edit visibly (last write wins on a list
//! the loser can see), not interleave into an order neither of them chose.
//!
//! # Storage
//!
//! One JSON array under [`SETTING_KEY`] in the key/value `settings` table —
//! no migration, because that table is already there. It is deliberately NOT
//! a key in [`super::settings`]: that registry is the operator's daemon
//! configuration (reconcile cadence, GC, playbooks), refused outright on a
//! desktop paired to a hub, and these are the opposite — a per-fleet UI
//! preference every paired client may both read and write.
//!
//! # Order and auto-send
//!
//! The list's order IS the chip row's order — every client draws the chips
//! as they come, so reordering is a `replace` with the entries moved.
//!
//! `auto_send` decides what a plain tap does: off (the default, and how every
//! chip behaved before the flag existed) fills the composer so the prompt can
//! be edited first; on sends the prompt at once. A `replace` entry that does
//! not carry the flag at all — a client from before it existed, such as a
//! phone whose chips are plain strings — keeps the flag the stored chip with
//! the same text already has, so an older editor saving the list does not
//! silently switch every auto-send chip back off.
//!
//! # Defaults
//!
//! [`defaults`] is the answer when nothing is stored, and storing something
//! equal to it stores nothing (see [`replace`]): a fleet that never touched
//! its chips keeps following the built-in list as it improves between
//! releases, instead of being pinned to whatever shipped the day it first
//! saved. This mirrors what `composer_presets.ts` already did on the
//! desktop.

use crate::ipc_error::{codes, IpcError};
use crate::store::Store;
use std::sync::Mutex;

/// Where the list lives in the `settings` table.
pub const SETTING_KEY: &str = "ui.quick_replies";

/// How many chips a fleet may keep. A chip row is scanned, not searched: past
/// a couple of dozen the phone's `LazyRow` is a scroll with no landmarks, and
/// the desktop's wraps into the transcript. The cap is on the stored list, so
/// it also bounds what every client parses.
pub const MAX_ENTRIES: usize = 24;

/// Longest label. It is drawn inside a chip on a phone screen; anything past
/// this is ellipsis either way.
pub const MAX_LABEL: usize = 40;

/// Longest prompt text. Generous — the built-in Review chip is ~450 chars and
/// a hand-written review prompt can be several times that — but bounded, so
/// the list can never become a way to store a document in a settings row.
pub const MAX_TEXT: usize = 4000;

/// One chip: what the button says, what it sends, and whether a tap sends it.
///
/// `label` is allowed to equal `text`, and does whenever a client that has no
/// separate label (fleet-mobile's chips were plain strings) writes one.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, rmcp::schemars::JsonSchema,
)]
#[schemars(crate = "rmcp::schemars")]
pub struct QuickReply {
    /// The chip's caption.
    pub label: String,
    /// The prompt the chip sends.
    pub text: String,
    // A tap sends (true) or only fills the composer. Omitted in a `replace`
    // entry: the stored chip's flag. A plain comment, not a doc: the doc
    // would ride in the MCP tool schema, whose byte budget is tight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_send: Option<bool>,
}

impl QuickReply {
    fn new(label: &str, text: &str) -> Self {
        Self {
            label: label.to_string(),
            text: text.to_string(),
            auto_send: Some(false),
        }
    }

    /// Whether a tap sends the chip at once.
    pub fn auto_send(&self) -> bool {
        self.auto_send.unwrap_or(false)
    }
}

/// The review prompt behind the built-in Review chip.
///
/// Kept verbatim from the desktop's `DEFAULT_REVIEW_PROMPT`, so a fleet that
/// had never customised its chips sees the same Review it always did after
/// this list moved to the hub.
const REVIEW_PROMPT: &str = "Review the work in this worktree. Run `git diff` and `git log` against the base branch to see what changed.

Pass 1 — correctness: does the code do what it should? Any bugs?
Pass 2 — code quality: clarity, structure, test coverage.
Pass 3 — risk: anything dangerous, security-sensitive, or destructive?

Cite file:line for every point. End with an overall verdict: approve / approve-with-fixes / needs-rework.";

/// The built-in list, served whenever a fleet has stored none of its own.
///
/// The first five are the desktop's former defaults unchanged. `Go on` is
/// added from fleet-mobile's, which was the one chip on that list with no
/// desktop counterpart and the most-tapped thing a pager does: tell a session
/// that stopped to keep going.
pub fn defaults() -> Vec<QuickReply> {
    vec![
        QuickReply::new("Clear", "/clear"),
        QuickReply::new("Compact", "/compact"),
        QuickReply::new("Status", "/status"),
        QuickReply::new("Continue", "Continue where you left off."),
        QuickReply::new("Go on", "go on"),
        QuickReply::new("Review", REVIEW_PROMPT),
    ]
}

/// The fleet's chips: what is stored, or [`defaults`] when nothing is.
///
/// A stored value that no longer parses (hand-edited row, a downgrade that
/// wrote another shape) answers the defaults rather than an error: this is a
/// row of buttons, and a composer that refuses to draw is worse than one
/// drawing the built-ins. The next [`replace`] overwrites the unreadable
/// value.
pub fn list(store: &Mutex<Store>) -> Result<Vec<QuickReply>, IpcError> {
    let s = crate::ipc_error::lock(store)?;
    list_in(&s)
}

/// [`list`] under a lock the caller already holds, so [`replace`] can read,
/// decide and write inside one critical section.
fn list_in(s: &Store) -> Result<Vec<QuickReply>, IpcError> {
    let Some(raw) = s.get_setting(SETTING_KEY)? else {
        return Ok(defaults());
    };
    match serde_json::from_str::<Vec<QuickReply>>(&raw) {
        Ok(entries) => Ok(normalize(entries)
            .into_iter()
            .map(|e| QuickReply {
                auto_send: Some(e.auto_send()),
                ..e
            })
            .collect()),
        Err(e) => {
            tracing::warn!(error = %e, "quick replies: stored list is unreadable, serving defaults");
            Ok(defaults())
        }
    }
}

/// Replace the whole list. Returns what a subsequent [`list`] will answer.
///
/// An empty list is not an error and not "no chips": it clears the stored
/// value, which restores the built-in [`defaults`]. A composer with no chips
/// at all would be a row of nothing that no client offers a way back from —
/// the phone's chip row is the only place it draws them, and clearing every
/// chip there would remove its own editor with them.
///
/// `expected`, when given, is the list the caller last read: the write is
/// refused with `E_CONFLICT` if the stored list is no longer that one (another
/// device saved in between), so the loser re-reads and sees the other edit
/// instead of overwriting it unseen. Omitted, the write is last-writer-wins,
/// as it always was — a client from before the argument keeps working.
pub fn replace(
    store: &Mutex<Store>,
    entries: Vec<QuickReply>,
    expected: Option<Vec<QuickReply>>,
) -> Result<Vec<QuickReply>, IpcError> {
    if entries.len() > MAX_ENTRIES {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "too many quick replies: {} (max {MAX_ENTRIES})",
                entries.len()
            ),
        ));
    }
    for e in &entries {
        if e.text.trim().is_empty() {
            return Err(IpcError::new(
                codes::E_INVALID,
                "a quick reply with no text sends nothing",
            ));
        }
        if e.label.chars().count() > MAX_LABEL {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("quick reply label is longer than {MAX_LABEL} characters"),
            ));
        }
        if e.text.chars().count() > MAX_TEXT {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("quick reply text is longer than {MAX_TEXT} characters"),
            ));
        }
        // A chip's text is typed into a tmux pane: an escape or other
        // control byte there drives the terminal, not the prompt. Newlines
        // and tabs are what a multi-line prompt is made of; a label is one
        // line drawn in a button and takes none.
        if e.text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            return Err(IpcError::new(
                codes::E_INVALID,
                "quick reply text contains a control character",
            ));
        }
        if e.label.chars().any(char::is_control) {
            return Err(IpcError::new(
                codes::E_INVALID,
                "quick reply label contains a control character",
            ));
        }
    }

    let mut entries = normalize(entries);
    // One critical section from the read to the write: the flag inheritance
    // and the `expected` check must see the list the write replaces, not one
    // another device replaced in between.
    let s = crate::ipc_error::lock(store)?;
    let stored = list_in(&s)?;
    if let Some(expected) = expected {
        if !same_list(&normalize(expected), &stored) {
            return Err(IpcError::new(
                codes::E_CONFLICT,
                "the quick replies were changed elsewhere; re-read them and apply the edit again",
            ));
        }
    }
    for e in entries.iter_mut().filter(|e| e.auto_send.is_none()) {
        e.auto_send = Some(
            stored
                .iter()
                .find(|k| k.text == e.text)
                .is_some_and(QuickReply::auto_send),
        );
    }
    if entries.is_empty() || entries == defaults() {
        // Storing a copy of the defaults would pin them forever: a later
        // release could never improve a built-in chip on this fleet. Same
        // rule `composer_presets.ts` applied to its `localStorage` copy.
        s.delete_setting(SETTING_KEY)?;
        return Ok(defaults());
    }
    let json = serde_json::to_string(&entries)
        .map_err(|e| IpcError::new(codes::E_SERIALIZE, format!("quick replies: {e}")))?;
    s.set_setting(SETTING_KEY, &json)?;
    Ok(entries)
}

/// Whether the list a caller last read (`expected`) is still the stored one.
/// An entry without the flag (a client that never had it) matches on label
/// and text alone.
fn same_list(expected: &[QuickReply], stored: &[QuickReply]) -> bool {
    expected.len() == stored.len()
        && expected.iter().zip(stored).all(|(e, k)| {
            e.label == k.label && e.text == k.text && e.auto_send.is_none_or(|a| a == k.auto_send())
        })
}

/// Trim, drop the empty, de-duplicate by text, and cap. Order is kept.
///
/// Applied on the way in AND on the way out, so a list written by an older
/// build (or by hand) is served under today's rules rather than handed to a
/// client to cope with. De-duplication is by `text`, not by label: two chips
/// that send the same prompt are one chip with two names, and the first one's
/// name is the one the fleet chose most recently.
fn normalize(entries: Vec<QuickReply>) -> Vec<QuickReply> {
    let mut out: Vec<QuickReply> = Vec::with_capacity(entries.len().min(MAX_ENTRIES));
    for e in entries {
        let text = e.text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        let label = {
            let l = e.label.trim();
            // A client with no label of its own (a plain-string chip) sends
            // the text as both. Keeping them equal is what makes such a chip
            // round-trip unchanged through a desktop that does have labels.
            if l.is_empty() {
                text.clone()
            } else {
                l.to_string()
            }
        };
        if out.iter().any(|k| k.text == text) {
            continue;
        }
        out.push(QuickReply {
            label,
            text,
            auto_send: e.auto_send,
        });
        if out.len() == MAX_ENTRIES {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn store() -> Mutex<Store> {
        Mutex::new(Store::open_in_memory().expect("store"))
    }

    /// A last-writer-wins replace, as every client before `expected` did.
    fn put(s: &Mutex<Store>, entries: Vec<QuickReply>) -> Result<Vec<QuickReply>, IpcError> {
        replace(s, entries, None)
    }

    #[test]
    fn control_characters_are_refused_but_newlines_and_tabs_are_prompt_text() {
        let s = store();
        for text in ["run\x1b[2Jthe tests", "go\ron", "a\u{7f}b", "x\0y"] {
            let e = put(&s, vec![QuickReply::new("Bad", text)]).unwrap_err();
            assert_eq!(e.code, codes::E_INVALID, "{text:?}");
        }
        for label in ["Two\nlines", "Tab\there", "Esc\x1b"] {
            let e = put(&s, vec![QuickReply::new(label, "hi")]).unwrap_err();
            assert_eq!(e.code, codes::E_INVALID, "{label:?}");
        }
        let ok = QuickReply::new("Multi", "line one\n\tline two");
        assert_eq!(put(&s, vec![ok.clone()]).unwrap(), vec![ok]);
    }

    #[test]
    fn a_write_against_a_list_that_moved_on_is_a_conflict() {
        let s = store();
        let seen = list(&s).unwrap();
        // Another device saves first.
        put(&s, vec![QuickReply::new("Tests", "run the tests")]).unwrap();
        let e = replace(
            &s,
            vec![QuickReply::new("Mine", "my prompt")],
            Some(seen.clone()),
        )
        .unwrap_err();
        assert_eq!(e.code, codes::E_CONFLICT);
        assert_eq!(
            list(&s).unwrap(),
            vec![QuickReply::new("Tests", "run the tests")],
            "a refused write stores nothing"
        );
        // Against what is stored now, it goes through.
        let now = list(&s).unwrap();
        let mine = vec![QuickReply::new("Mine", "my prompt")];
        assert_eq!(replace(&s, mine.clone(), Some(now)).unwrap(), mine);
    }

    #[test]
    fn expected_without_the_flag_matches_on_label_and_text() {
        let s = store();
        put(&s, vec![chip("Go", "go on", Some(true))]).unwrap();
        let out = replace(
            &s,
            vec![chip("Go", "go on", None), chip("New", "new one", None)],
            Some(vec![chip("Go", "go on", None)]),
        )
        .unwrap();
        assert_eq!(
            out,
            vec![
                chip("Go", "go on", Some(true)),
                chip("New", "new one", Some(false))
            ]
        );
    }

    #[test]
    fn an_untouched_fleet_gets_the_built_ins() {
        let s = store();
        assert_eq!(list(&s).unwrap(), defaults());
    }

    #[test]
    fn a_replaced_list_is_what_comes_back() {
        let s = store();
        let mine = vec![QuickReply::new("Tests", "run the tests")];
        assert_eq!(put(&s, mine.clone()).unwrap(), mine);
        assert_eq!(list(&s).unwrap(), mine);
    }

    #[test]
    fn storing_the_defaults_stores_nothing_so_they_can_still_improve() {
        let s = store();
        put(&s, defaults()).unwrap();
        let raw = {
            let g = s.lock().unwrap();
            g.get_setting(SETTING_KEY).unwrap()
        };
        assert_eq!(raw, None, "a copy of the defaults must not be persisted");
        assert_eq!(list(&s).unwrap(), defaults());
    }

    #[test]
    fn an_empty_list_restores_the_built_ins_rather_than_leaving_no_chips() {
        let s = store();
        put(&s, vec![QuickReply::new("Tests", "run the tests")]).unwrap();
        assert_eq!(put(&s, vec![]).unwrap(), defaults());
        assert_eq!(list(&s).unwrap(), defaults());
    }

    #[test]
    fn a_label_less_chip_round_trips_as_its_own_text() {
        let s = store();
        let out = put(&s, vec![QuickReply::new("", "  ship it  ")]).unwrap();
        assert_eq!(out, vec![QuickReply::new("ship it", "ship it")]);
    }

    #[test]
    fn the_same_prompt_twice_is_one_chip() {
        let s = store();
        let out = put(
            &s,
            vec![
                QuickReply::new("Tests", "run the tests"),
                QuickReply::new("Test suite", "run the tests"),
            ],
        )
        .unwrap();
        assert_eq!(out, vec![QuickReply::new("Tests", "run the tests")]);
    }

    #[test]
    fn a_chip_with_no_text_is_refused_rather_than_dropped() {
        // Dropped silently, a client that sent one would show it, reload, and
        // find it gone with nothing said.
        let s = store();
        let err = put(&s, vec![QuickReply::new("Oops", "   ")]).unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
    }

    #[test]
    fn the_caps_are_refusals_not_truncations() {
        let s = store();
        let long_label = "x".repeat(MAX_LABEL + 1);
        let e = put(&s, vec![QuickReply::new(&long_label, "hi")]).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);

        let long_text = "x".repeat(MAX_TEXT + 1);
        let e = put(&s, vec![QuickReply::new("Long", &long_text)]).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);

        let many: Vec<QuickReply> = (0..=MAX_ENTRIES)
            .map(|i| QuickReply::new(&format!("c{i}"), &format!("prompt {i}")))
            .collect();
        let e = put(&s, many).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
    }

    fn chip(label: &str, text: &str, auto_send: Option<bool>) -> QuickReply {
        QuickReply {
            auto_send,
            ..QuickReply::new(label, text)
        }
    }

    #[test]
    fn auto_send_and_order_round_trip() {
        let s = store();
        let mine = vec![
            chip("Go", "go on", Some(true)),
            chip("Tests", "run the tests", Some(false)),
        ];
        assert_eq!(put(&s, mine.clone()).unwrap(), mine);
        let reordered = vec![mine[1].clone(), mine[0].clone()];
        assert_eq!(put(&s, reordered.clone()).unwrap(), reordered);
        assert_eq!(list(&s).unwrap(), reordered);
    }

    #[test]
    fn an_entry_without_the_flag_keeps_the_stored_one() {
        // An older client (the phone's plain-string chips) saving the list
        // must not switch every auto-send chip back off.
        let s = store();
        put(
            &s,
            vec![
                chip("Go", "go on", Some(true)),
                chip("Tests", "run the tests", Some(false)),
            ],
        )
        .unwrap();
        let out = put(
            &s,
            vec![
                chip("go on", "go on", None),
                chip("new", "something new", None),
            ],
        )
        .unwrap();
        assert_eq!(
            out,
            vec![
                chip("go on", "go on", Some(true)),
                chip("new", "something new", Some(false)),
            ]
        );
    }

    #[test]
    fn a_list_stored_before_the_flag_is_served_with_it_off() {
        let s = store();
        {
            let g = s.lock().unwrap();
            g.set_setting(SETTING_KEY, r#"[{"label":"Go","text":"go on"}]"#)
                .unwrap();
        }
        assert_eq!(list(&s).unwrap(), vec![chip("Go", "go on", Some(false))]);
    }

    #[test]
    fn served_chips_always_carry_the_flag() {
        let s = store();
        let json = serde_json::to_value(list(&s).unwrap()).unwrap();
        assert!(json
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c.get("auto_send").is_some_and(|v| v.is_boolean())));
    }

    #[test]
    fn an_unreadable_stored_value_draws_the_built_ins_instead_of_failing() {
        let s = store();
        {
            let g = s.lock().unwrap();
            g.set_setting(SETTING_KEY, "{not json at all").unwrap();
        }
        assert_eq!(list(&s).unwrap(), defaults());
    }
}
