//! Rendering pending messages into a hook response's `additionalContext`.
//!
//! Claude Code caps `additionalContext` at 8000 characters and 200 lines and
//! truncates silently past either. Truncating a message body mid-sentence is
//! worse than not sending it, so this packs WHOLE messages only and names how
//! many are left in the inbox.

use crate::store::SessionMessage;

/// Claude Code's `additionalContext` character cap (measured, 2.1.278).
pub const CTX_MAX_CHARS: usize = 8000;
/// Claude Code's `additionalContext` line cap (measured, 2.1.278).
pub const CTX_MAX_LINES: usize = 200;

/// Headroom left for the trailing "N more in the inbox" line, so adding it can
/// never push a packed batch over either budget. Sized to the tail's actual
/// worst case, not a guess: the line is
/// `"(N more message(s) waiting — call the fleet `inbox` tool to read them)"`,
/// whose fixed wording is 89 chars including the widest `N` can ever be
/// (`remaining` is a `usize`; `usize::MAX` on any 64-bit target is 20 decimal
/// digits), plus the 2-char/1-line joiner `blocks.join("\n\n")` inserts
/// before it once earlier blocks exist. 100/2 leaves margin over that 91/2.
const TAIL_RESERVE_CHARS: usize = 100;
const TAIL_RESERVE_LINES: usize = 2;

/// What one hook response will carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packed {
    /// The `additionalContext` value; empty when nothing was packed and
    /// nothing remains.
    pub text: String,
    /// Ids included in `text`, in the order they appear. These are the rows to
    /// stamp `delivered_at` on — and only these.
    pub included: Vec<i64>,
    /// How many of `messages` did not fit.
    pub remaining: usize,
}

/// PURE: render `messages` (oldest first) into one `additionalContext` value.
///
/// `sender_label` turns a sender's session id into something human — normally
/// `"<tmux_name>@<host_alias>"`. Taken as a closure so this stays pure and
/// testable without a store.
pub fn pack(messages: &[SessionMessage], sender_label: &dyn Fn(i64) -> String) -> Packed {
    let budget_chars = CTX_MAX_CHARS.saturating_sub(TAIL_RESERVE_CHARS);
    let budget_lines = CTX_MAX_LINES.saturating_sub(TAIL_RESERVE_LINES);

    let mut blocks: Vec<String> = Vec::new();
    let mut included: Vec<i64> = Vec::new();
    let mut chars = 0usize;
    let mut lines = 0usize;

    for m in messages {
        let block = format!(
            "[fleet msg #{id} from {who}]: {body}",
            id = m.id,
            who = sender_label(m.from_session_id),
            body = m.body
        );
        // Exact joiner cost: `blocks.join("\n\n")` inserts "\n\n" — 2 chars,
        // 1 extra line — between consecutive blocks, so it applies only once
        // a previous block already exists. A flat "+1" undercounts as N
        // grows: 95 short messages tracked a total of 7878 chars against the
        // 7880 budget while the real joined output was 8044 chars, 44 over
        // CTX_MAX_CHARS — exactly the silent CLI truncation this exists to
        // prevent.
        let (joiner_c, joiner_l) = if blocks.is_empty() { (0, 0) } else { (2, 1) };
        let c = block.chars().count() + joiner_c;
        let l = block.lines().count() + joiner_l;
        if chars + c > budget_chars || lines + l > budget_lines {
            // Whole messages only: stop at the first one that does not fit
            // rather than skipping it, so delivery order is never scrambled
            // and a large message is never starved by smaller later ones.
            break;
        }
        chars += c;
        lines += l;
        included.push(m.id);
        blocks.push(block);
    }

    let remaining = messages.len() - included.len();
    if remaining > 0 {
        blocks.push(format!(
            "({remaining} more message(s) waiting — call the fleet `inbox` tool to read them)"
        ));
    }
    Packed {
        text: blocks.join("\n\n"),
        included,
        remaining,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SessionMessage;

    fn msg(id: i64, body: &str) -> SessionMessage {
        SessionMessage {
            id,
            from_session_id: 1,
            to_session_id: 2,
            body: body.into(),
            kind: "message".into(),
            sent_at: 1_700_000_000 + id,
            read_at: None,
            reply_to: None,
        }
    }
    fn label(_: i64) -> String {
        "alpha@local".into()
    }

    #[test]
    fn empty_input_packs_to_nothing() {
        let p = pack(&[], &label);
        assert_eq!(p.text, "");
        assert!(p.included.is_empty());
        assert_eq!(p.remaining, 0);
    }

    #[test]
    fn a_short_batch_is_included_whole_and_names_each_sender() {
        let p = pack(&[msg(1, "first"), msg(2, "second")], &label);
        assert_eq!(p.included, vec![1, 2]);
        assert_eq!(p.remaining, 0);
        assert!(p.text.contains("first") && p.text.contains("second"));
        assert!(p.text.contains("alpha@local"), "the sender must be visible");
        assert!(
            p.text.contains("#1") && p.text.contains("#2"),
            "ids let the agent reply"
        );
    }

    #[test]
    fn the_char_budget_includes_whole_messages_only_and_reports_the_rest() {
        // Distinct bodies (same length, distinct marker suffix) so a check
        // that a given id's body is present cannot be satisfied by some
        // OTHER included message's identical body.
        let body_for = |id: i64| {
            let marker = format!("id{id}");
            let filler = "x".repeat(CTX_MAX_CHARS / 2 - marker.len());
            format!("{filler}{marker}")
        };
        let bodies: Vec<String> = (1..=3).map(body_for).collect();
        let p = pack(
            &[msg(1, &bodies[0]), msg(2, &bodies[1]), msg(3, &bodies[2])],
            &label,
        );
        assert!(p.text.chars().count() <= CTX_MAX_CHARS, "budget respected");
        assert!(!p.included.is_empty(), "at least one message gets through");
        assert!(p.included.len() < 3, "not all three can fit");
        assert_eq!(p.remaining, 3 - p.included.len());
        // No body was cut, and only the included ids' own bodies are
        // present: an excluded id's distinct body must not appear either.
        for (i, body) in bodies.iter().enumerate() {
            let id = (i + 1) as i64;
            if p.included.contains(&id) {
                assert!(p.text.contains(body), "message {id} was truncated");
            } else {
                assert!(
                    !p.text.contains(body),
                    "excluded message {id}'s body must not appear"
                );
            }
        }
        assert!(p.text.contains(&format!("{} more", p.remaining)));
    }

    #[test]
    fn the_line_budget_is_enforced_independently_of_the_char_budget() {
        // 150 one-character lines: far under CTX_MAX_CHARS, over CTX_MAX_LINES
        // once two of them are packed together.
        let tall = "y\n".repeat(150);
        let p = pack(&[msg(1, &tall), msg(2, &tall)], &label);
        assert!(
            p.text.lines().count() <= CTX_MAX_LINES,
            "line budget respected"
        );
        assert_eq!(p.included.len(), 1, "the second does not fit on lines");
        assert_eq!(p.remaining, 1);
    }

    #[test]
    fn a_single_message_over_budget_is_never_packed_and_is_reported() {
        let huge = "z".repeat(CTX_MAX_CHARS + 1);
        let p = pack(&[msg(1, &huge)], &label);
        assert!(p.included.is_empty(), "a body is never cut to fit");
        assert_eq!(p.remaining, 1);
        assert!(
            p.text.contains("1 more"),
            "the agent must still learn it exists"
        );
    }

    #[test]
    fn multibyte_bodies_are_counted_in_chars_not_bytes() {
        // 3000 four-byte chars = 12000 bytes but only 3000 chars: two of these
        // fit the 8000-char budget, which a byte-based count would deny.
        let m = "🦀".repeat(3000);
        let p = pack(&[msg(1, &m), msg(2, &m)], &label);
        assert_eq!(p.included.len(), 2, "char budget, not byte budget");
    }

    /// The reviewer's exact failing case for the "+1" joiner bug: a flat
    /// per-block "+1" undercounts the real 2-char/1-line joiner cost of
    /// `blocks.join("\n\n")`, and the gap grows with N. At N=95 the old
    /// tracked total (7878) stayed under budget while the real joined
    /// output (8044 chars) blew CTX_MAX_CHARS by 44 — the exact silent
    /// truncation this module exists to prevent.
    #[test]
    fn ninety_five_short_messages_stay_within_both_budgets() {
        let body = "b".repeat(48);
        let messages: Vec<SessionMessage> = (1..=95).map(|id| msg(id, &body)).collect();
        let p = pack(&messages, &label);
        assert!(
            p.text.chars().count() <= CTX_MAX_CHARS,
            "chars = {}",
            p.text.chars().count()
        );
        assert!(
            p.text.lines().count() <= CTX_MAX_LINES,
            "lines = {}",
            p.text.lines().count()
        );
        assert_eq!(p.included.len() + p.remaining, messages.len());
    }

    /// Property sweep: for a range of message counts and a few body sizes,
    /// both budgets hold and `included`/`remaining` always account for every
    /// input message. The original six tests checked behaviour at a few
    /// hand-picked points; this is the invariant they were missing, which is
    /// exactly what let the joiner undercount hide at N=95.
    #[test]
    fn the_budgets_hold_across_a_sweep_of_counts_and_sizes() {
        for body_len in [1usize, 48, 200] {
            let body = "s".repeat(body_len);
            for n in 1..=120i64 {
                let messages: Vec<SessionMessage> = (1..=n).map(|id| msg(id, &body)).collect();
                let p = pack(&messages, &label);
                assert!(
                    p.text.chars().count() <= CTX_MAX_CHARS,
                    "n={n} body_len={body_len} chars={}",
                    p.text.chars().count()
                );
                assert!(
                    p.text.lines().count() <= CTX_MAX_LINES,
                    "n={n} body_len={body_len} lines={}",
                    p.text.lines().count()
                );
                assert_eq!(
                    p.included.len() + p.remaining,
                    messages.len(),
                    "n={n} body_len={body_len}"
                );
            }
        }
    }
}
