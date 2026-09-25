//! Rendering pending messages into a hook response's `additionalContext`.
//!
//! Claude Code caps `additionalContext` at 8000 characters and 200 lines and
//! truncates silently past either. Truncating a message body mid-sentence is
//! worse than not sending it, so this packs WHOLE messages only and names how
//! many are left in the inbox. A message whose own body cannot fit even as
//! the first block of an empty batch gets a short stub instead (naming its
//! id, pointing at `inbox`) rather than being skipped — skipping it would
//! leave it undelivered at the head of an oldest-first queue forever,
//! stalling delivery of everything behind it.
//!
//! A `Stop` block's `reason` is a second, much smaller window (2000 chars /
//! 20 lines) onto the same queue. It is PACKED to that budget
//! ([`pack_within`]), never truncated down to it — see that function.

use crate::store::SessionMessage;

/// Claude Code's `additionalContext` character cap (measured, 2.1.278).
pub const CTX_MAX_CHARS: usize = 8000;
/// Claude Code's `additionalContext` line cap (measured, 2.1.278).
pub const CTX_MAX_LINES: usize = 200;

/// Claude Code's `Stop` block `reason` character cap — far under
/// [`CTX_MAX_CHARS`].
pub const REASON_MAX_CHARS: usize = 2000;
/// Claude Code's `Stop` block `reason` line cap — far under
/// [`CTX_MAX_LINES`], and the one nothing accounted for before.
pub const REASON_MAX_LINES: usize = 20;

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
    /// Handover rows (work graph M2.3, `work_journal` kind `handover`)
    /// included in `text`, ahead of every message. Stamped `delivered_at`
    /// exactly like `included`.
    pub handovers: Vec<i64>,
}

/// A handover brief waiting for its session's next hook (work graph M2.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingHandover {
    pub id: i64,
    pub body: String,
}

/// PURE: render `messages` (oldest first) into one `additionalContext` value,
/// against the [`CTX_MAX_CHARS`] / [`CTX_MAX_LINES`] budget.
///
/// `sender_label` turns a message into something human to name its sender —
/// normally `"<tmux_name>@<host_alias>"` for a local sender, or the remote
/// address (migration 054) for one on another fleet. Taken as a closure so
/// this stays pure and testable without a store.
pub fn pack(
    messages: &[SessionMessage],
    sender_label: &dyn Fn(&SessionMessage) -> String,
) -> Packed {
    pack_within(messages, sender_label, CTX_MAX_CHARS, CTX_MAX_LINES)
}

/// As [`pack`], against an explicit budget.
///
/// A `Stop` block's `reason` has its own, much smaller caps
/// ([`REASON_MAX_CHARS`] / [`REASON_MAX_LINES`]), and a packed batch cannot
/// be truncated down to them afterwards: cutting the text splits a body
/// mid-sentence, and the `"(N more…)"` tail — the only pointer at `inbox` —
/// sits last, so it is the first thing lost. The block path packs to its own
/// budget instead, so the same whole-messages-only invariant holds there and
/// the tail's count is the truth for what actually rode.
pub fn pack_within(
    messages: &[SessionMessage],
    sender_label: &dyn Fn(&SessionMessage) -> String,
    max_chars: usize,
    max_lines: usize,
) -> Packed {
    pack_with_handovers(&[], messages, sender_label, max_chars, max_lines)
}

/// As [`pack_within`], with handover briefs packed AHEAD of the messages in
/// the same budget (work graph M2.3): a brief is fleet's context for the
/// turn about to run, so it rides first. Whole briefs only; one that does not
/// fit what is left waits for the next hook, and one that cannot fit even an
/// empty batch rides as a stub pointing at the `work` tool (the same
/// no-starvation rule as an oversized message).
pub fn pack_with_handovers(
    handovers: &[PendingHandover],
    messages: &[SessionMessage],
    sender_label: &dyn Fn(&SessionMessage) -> String,
    max_chars: usize,
    max_lines: usize,
) -> Packed {
    let budget_chars = max_chars.saturating_sub(TAIL_RESERVE_CHARS);
    let budget_lines = max_lines.saturating_sub(TAIL_RESERVE_LINES);

    let mut blocks: Vec<String> = Vec::new();
    let mut included: Vec<i64> = Vec::new();
    let mut handover_ids: Vec<i64> = Vec::new();
    let mut chars = 0usize;
    let mut lines = 0usize;

    for h in handovers {
        let mut block = format!("[fleet handover #{}]\n{}", h.id, h.body);
        if block.chars().count() > budget_chars || block.lines().count() > budget_lines {
            block = format!(
                "[fleet handover #{}]: (brief too large to inline — call the fleet `work` tool with action context)",
                h.id
            );
        }
        let (joiner_c, joiner_l) = if blocks.is_empty() { (0, 0) } else { (2, 1) };
        let c = block.chars().count() + joiner_c;
        let l = block.lines().count() + joiner_l;
        if chars + c > budget_chars || lines + l > budget_lines {
            break;
        }
        chars += c;
        lines += l;
        handover_ids.push(h.id);
        blocks.push(block);
    }

    for m in messages {
        let who = sender_label(m);
        let block = format!(
            "[fleet msg #{id} from {who}]: {body}",
            id = m.id,
            who = who,
            body = m.body
        );
        let block_chars = block.chars().count();
        let block_lines = block.lines().count();
        // Exact joiner cost: `blocks.join("\n\n")` inserts "\n\n" — 2 chars,
        // 1 extra line — between consecutive blocks, so it applies only once
        // a previous block already exists. A flat "+1" undercounts as N
        // grows: 95 short messages tracked a total of 7878 chars against the
        // 7880 budget while the real joined output was 8044 chars, 44 over
        // CTX_MAX_CHARS — exactly the silent CLI truncation this exists to
        // prevent.
        let (joiner_c, joiner_l) = if blocks.is_empty() { (0, 0) } else { (2, 1) };

        // Individually oversized: this message's own body cannot fit even as
        // the sole content of an empty batch (measured against the FULL
        // budget, not what happens to be left). Breaking here — as the
        // ordinary overflow case below does — would leave it undelivered at
        // the head of the queue forever: `take_pending_delivery_locked`
        // stamps nothing when `included` is empty, `list_undelivered_for_session`
        // is oldest-first, so every later hook would hit this exact same
        // message again. Swap in a short stub instead: it names the id and
        // says the body must be read via `inbox`, it DOES get stamped
        // delivered (its id goes into `included`), and packing continues —
        // so this message can never starve everything behind it. A message
        // that merely doesn't fit in the *remaining* space of a partly
        // filled batch is a different case (below): that one still breaks,
        // to preserve order and avoid starving it by smaller later ones.
        if block_chars > budget_chars || block_lines > budget_lines {
            let mut stub = format!(
                "[fleet msg #{id} from {who}]: (message too large to inline — {chars} chars; read it with the fleet `inbox` tool)",
                id = m.id,
                who = who,
                chars = block_chars,
            );
            // The stub carries the sender label, and a label can itself be
            // too large (a peer's address is capped at `PEER_ADDR_MAX`, but
            // nothing here may rely on that). As the FIRST block such a stub
            // would stall the queue exactly like the unstubbed body did, so
            // it drops the label: a bare stub naming only the id always fits
            // either budget, and `included` is empty only when `messages` is.
            if blocks.is_empty()
                && (stub.chars().count() > budget_chars || stub.lines().count() > budget_lines)
            {
                stub = format!(
                    "[fleet msg #{id}]: (message too large to inline; read it with the fleet `inbox` tool)",
                    id = m.id,
                );
            }
            let c = stub.chars().count() + joiner_c;
            let l = stub.lines().count() + joiner_l;
            if chars + c > budget_chars || lines + l > budget_lines {
                // Even the stub doesn't fit in what's left of this batch —
                // stop like the ordinary overflow case; everything from here
                // on, including this message, is counted in `remaining` and
                // stays undelivered to be picked up (stubbed, if still
                // individually oversized) by the next hook.
                break;
            }
            chars += c;
            lines += l;
            included.push(m.id);
            blocks.push(stub);
            continue;
        }

        let c = block_chars + joiner_c;
        let l = block_lines + joiner_l;
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
        handovers: handover_ids,
    }
}

/// Most consecutive `Stop` blocks one session may be held by. Without a cap a
/// remote sender could shut a session inside a never-ending turn — a denial of
/// service against one's own fleet, and reachable in a full mesh where every
/// endpoint can address every other.
pub const STOP_BLOCK_STREAK_MAX: u32 = 3;

/// What a `Stop` hook should do about the pending messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopAction {
    /// Add `additionalContext`; the turn ends normally.
    Context,
    /// `decision: "block"` with the messages as the `reason`, so Claude keeps
    /// working and answers.
    Block,
}

/// PURE: block only for a message that genuinely wants an answer, and only
/// while under [`STOP_BLOCK_STREAK_MAX`].
pub fn stop_action(pending: &[SessionMessage], streak: u32) -> StopAction {
    if streak >= STOP_BLOCK_STREAK_MAX {
        return StopAction::Context;
    }
    if pending.iter().any(|m| m.kind == "question") {
        StopAction::Block
    } else {
        StopAction::Context
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
            from_addr: None,
            to_addr: None,
        }
    }
    fn label(_: &SessionMessage) -> String {
        "alpha@local".into()
    }

    fn handover(id: i64, body: &str) -> PendingHandover {
        PendingHandover {
            id,
            body: body.into(),
        }
    }

    #[test]
    fn handovers_ride_first_and_an_oversized_one_is_a_stub() {
        let p = pack_with_handovers(
            &[handover(7, "the brief")],
            &[msg(1, "mail")],
            &label,
            CTX_MAX_CHARS,
            CTX_MAX_LINES,
        );
        assert_eq!(p.handovers, vec![7]);
        assert_eq!(p.included, vec![1]);
        assert!(p.text.starts_with("[fleet handover #7]\nthe brief"));

        let huge = "x".repeat(CTX_MAX_CHARS);
        let p = pack_with_handovers(
            &[handover(8, &huge)],
            &[],
            &label,
            CTX_MAX_CHARS,
            CTX_MAX_LINES,
        );
        assert_eq!(
            p.handovers,
            vec![8],
            "stamped, so it cannot starve the queue"
        );
        assert!(p.text.contains("brief too large"));
        assert!(p.text.chars().count() <= CTX_MAX_CHARS);

        // A brief that takes most of the budget leaves the mail for later,
        // counted in the tail.
        let big = "y".repeat(CTX_MAX_CHARS - 500);
        let p = pack_with_handovers(
            &[handover(9, &big)],
            &[msg(1, &"z".repeat(1000))],
            &label,
            CTX_MAX_CHARS,
            CTX_MAX_LINES,
        );
        assert_eq!(p.handovers, vec![9]);
        assert!(p.included.is_empty());
        assert_eq!(p.remaining, 1);
        assert!(p.text.chars().count() <= CTX_MAX_CHARS);
    }

    #[test]
    fn a_remote_sender_is_labelled_by_its_address() {
        let mut m = msg(1, "hello");
        m.from_session_id = 0;
        m.from_addr = Some("fleet-a/session/h/a1".into());
        let label = |m: &SessionMessage| {
            m.from_addr
                .clone()
                .unwrap_or_else(|| format!("session {}", m.from_session_id))
        };
        let p = pack(&[m], &label);
        assert!(p.text.contains("from fleet-a/session/h/a1"), "{}", p.text);
        assert!(!p.text.contains("session 0"), "{}", p.text);
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

    /// Test 1 from the fix spec. Supersedes the old
    /// `a_single_message_over_budget_is_never_packed_and_is_reported`, which
    /// asserted `included.is_empty()` and `remaining == 1` for this exact
    /// input — that WAS the bug: an individually oversized message left
    /// unstamped at the head of an oldest-first, undelivered-only queue,
    /// where `pack` breaking on it (and stamping nothing, since `included`
    /// was empty) meant every later hook hit the same message and nothing
    /// behind it was ever delivered again. The fix stubs it instead: still
    /// no body is ever cut to fit, but the id above IS stamped delivered and
    /// packing does not stop here.
    #[test]
    fn a_single_oversized_message_gets_a_stub_and_is_stamped_delivered() {
        let huge = "z".repeat(CTX_MAX_CHARS + 1);
        let p = pack(&[msg(1, &huge)], &label);
        assert_eq!(
            p.included,
            vec![1],
            "the stub still gets it stamped delivered"
        );
        assert_eq!(p.remaining, 0);
        // `#1`, not a bare `1`: any stray digit in the stub's own wording
        // (its char count, for one) satisfies `contains('1')`, so that check
        // could not tell a stub that names the id from one that does not.
        assert!(
            p.text.contains("#1"),
            "the stub must name the message id as #1: {}",
            p.text
        );
        assert!(
            !p.text.contains(&huge),
            "the body itself must never be inlined"
        );
    }

    /// Test 2 from the fix spec — the regression test for the permanent
    /// stall. Before the fix: `pack` breaks on message 1 immediately,
    /// `included` is empty, so `take_pending_delivery_locked` stamps
    /// nothing; message 1 stays at the head of the oldest-first undelivered
    /// queue and every subsequent hook repeats this forever, so messages 2
    /// and 3 are NEVER delivered. `included` would be `[]` and
    /// `remaining` would be `3`.
    #[test]
    fn an_oversized_message_at_the_head_does_not_stall_the_ones_behind_it() {
        let huge = "z".repeat(CTX_MAX_CHARS + 1);
        let p = pack(&[msg(1, &huge), msg(2, "second"), msg(3, "third")], &label);
        assert_eq!(p.included, vec![1, 2, 3], "all three get delivered");
        assert_eq!(p.remaining, 0);
        assert!(p.text.contains("second") && p.text.contains("third"));
        assert!(
            !p.text.contains(&huge),
            "the oversized body must not appear"
        );
    }

    /// Test 3 from the fix spec: the 8000-char / 200-line caps still hold
    /// once stubs are mixed into a batch.
    #[test]
    fn the_caps_still_hold_when_stubs_are_present() {
        let huge = "z".repeat(CTX_MAX_CHARS + 1);
        let messages: Vec<SessionMessage> = (1..=10)
            .map(|id| {
                if id % 2 == 0 {
                    msg(id, &huge)
                } else {
                    msg(id, "small")
                }
            })
            .collect();
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

    /// I3: a message whose SENDER LABEL alone is too long for its stub to
    /// fit still makes progress — it rides as a bare stub naming its id, is
    /// stamped delivered, and the ones behind it follow. `included` is empty
    /// only when the input is, on both budgets.
    #[test]
    fn a_message_whose_stub_cannot_fit_still_makes_progress() {
        let huge_label = |m: &SessionMessage| {
            if m.id == 1 {
                format!("fleet-a/session/h/{}", "n".repeat(8 * 1024))
            } else {
                "alpha@local".into()
            }
        };
        for (max_chars, max_lines) in [
            (CTX_MAX_CHARS, CTX_MAX_LINES),
            (REASON_MAX_CHARS, REASON_MAX_LINES),
        ] {
            let p = pack_within(
                &[msg(1, "short"), msg(2, "second")],
                &huge_label,
                max_chars,
                max_lines,
            );
            assert_eq!(p.included, vec![1, 2], "budget {max_chars}");
            assert!(p.text.contains("#1"), "{}", p.text);
            assert!(
                !p.text.contains(&"n".repeat(300)),
                "the label is not inlined"
            );
            assert!(p.text.chars().count() <= max_chars);
            assert!(p.text.lines().count() <= max_lines);
        }
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

    /// Final review, Critical 1. A `Stop` block's `reason` is capped at 2000
    /// chars AND 20 lines; packing to that budget keeps the same invariants
    /// the 8000/200 one has — whole bodies only, an accurate tail — which
    /// truncating the 8000-char text could not.
    #[test]
    fn packing_to_the_reason_budget_keeps_whole_bodies_and_an_accurate_tail() {
        let body = "x".repeat(400);
        let messages: Vec<SessionMessage> = (1..=20).map(|id| msg(id, &body)).collect();
        let p = pack_within(&messages, &label, REASON_MAX_CHARS, REASON_MAX_LINES);
        assert!(
            p.text.chars().count() <= REASON_MAX_CHARS,
            "chars = {}",
            p.text.chars().count()
        );
        assert!(!p.included.is_empty() && p.included.len() < messages.len());
        assert_eq!(p.included.len() + p.remaining, messages.len());
        assert_eq!(
            p.text.matches(&body).count(),
            p.included.len(),
            "every included body rides whole, and no excluded one appears"
        );
        assert!(
            p.text
                .contains(&format!("({} more message(s) waiting", p.remaining)),
            "the tail names the real remainder: {}",
            p.text
        );
    }

    /// The 20-line cap is independent of the 2000-char one: five-line bodies
    /// stay far under 2000 chars and still blow the line budget.
    #[test]
    fn the_reason_line_budget_is_enforced_independently() {
        let body = "a\nb\nc\nd\ne";
        let messages: Vec<SessionMessage> = (1..=20).map(|id| msg(id, body)).collect();
        let p = pack_within(&messages, &label, REASON_MAX_CHARS, REASON_MAX_LINES);
        assert!(
            p.text.lines().count() <= REASON_MAX_LINES,
            "lines = {}",
            p.text.lines().count()
        );
        assert!(
            p.text.chars().count() < REASON_MAX_CHARS,
            "chars were never the binding cap"
        );
        assert_eq!(p.included.len() + p.remaining, messages.len());
    }

    /// A single body over the reason budget is STUBBED, never cut: the stub
    /// names the id and points at `inbox`, so a block carrying it still tells
    /// the agent what to answer and where to read it.
    #[test]
    fn a_body_over_the_reason_budget_is_stubbed_not_cut() {
        let body = "q".repeat(REASON_MAX_CHARS + 500);
        let p = pack_within(&[msg(7, &body)], &label, REASON_MAX_CHARS, REASON_MAX_LINES);
        assert_eq!(p.included, vec![7]);
        assert!(p.text.chars().count() <= REASON_MAX_CHARS);
        assert!(
            !p.text.contains(&body[..200]),
            "no part of the body is cut in: {}",
            p.text
        );
        assert!(
            p.text.contains("#7") && p.text.contains("inbox"),
            "{}",
            p.text
        );
    }

    fn question(id: i64) -> SessionMessage {
        SessionMessage {
            kind: "question".into(),
            ..msg(id, "answer me")
        }
    }

    #[test]
    fn a_plain_message_never_blocks_a_stop() {
        assert_eq!(stop_action(&[msg(1, "fyi")], 0), StopAction::Context);
    }

    #[test]
    fn a_question_blocks_a_stop_while_under_the_cap() {
        assert_eq!(stop_action(&[question(1)], 0), StopAction::Block);
        assert_eq!(
            stop_action(&[question(1)], STOP_BLOCK_STREAK_MAX - 1),
            StopAction::Block
        );
    }

    #[test]
    fn at_the_cap_even_a_question_only_adds_context() {
        assert_eq!(
            stop_action(&[question(1)], STOP_BLOCK_STREAK_MAX),
            StopAction::Context
        );
        assert_eq!(
            stop_action(&[question(1)], STOP_BLOCK_STREAK_MAX + 7),
            StopAction::Context
        );
    }

    #[test]
    fn nothing_pending_never_blocks() {
        assert_eq!(stop_action(&[], 0), StopAction::Context);
    }
}
