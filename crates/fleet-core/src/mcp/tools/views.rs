//! Named row projections for the list tools.
//!
//! # Why a named view rather than a client-supplied field list
//!
//! A pager — the phone app is the one that exists — draws a fixed set of
//! columns and pays for every other one. Measured against the live hub
//! (`fleet.rlt.sk`, 44 sessions / 56 rows, 2026-09-21), the full
//! `list_sessions` answer is 46 990 B of inner JSON *after* `strip_nulls`,
//! of which the fields that list actually draws are 16 733 B — the other
//! 64 % is `claude_session_id` (2 773 B over the capture), `account_uuid`
//! (2 160 B), the six `usage_*` columns and the rest, none of which any
//! phone screen reads.
//!
//! The obvious fix — widening [`super::SessionSummary`] — is the wrong one:
//! that type exists to keep the DEFAULT answer inside an agent's token cap,
//! and the nine extra columns would be charged to every agent that calls
//! `list_sessions` for triage. So the projection is opt-in, and it is
//! *named*: the server keeps the definition of "what a pager row is" and can
//! widen it when a screen starts drawing a new column, without waiting for
//! an app release. A client-supplied `fields=` list would freeze today's
//! screen into every installed build.
//!
//! An unknown view name is refused, never ignored: a typo that silently
//! answered full rows would look exactly like a working call and cost the
//! bytes this exists to save. That is the same rule
//! `events_route::wanted_kinds` follows for `?kinds=`, in the shape a
//! single-valued tool parameter allows.
//!
//! A columnar encoding of the same projection (`{cols:[…],rows:[[…]]}`) was
//! measured on that capture at 9 934 B plain and 2 524 B gzipped — 33 B
//! better than this row JSON once compressed, which is not worth a second
//! wire shape for either end to speak. Deliberately not built.

use super::*;
use serde_json::Value;

/// The `list_sessions` fields `view: "phone"` keeps.
///
/// Derived from what the phone actually reads, not from what its model
/// declares: `SessionRow.kt` deserialises 28 fields, but `branch`, `pr_url`,
/// `lost_at`, `created_at`, `worktree_id` and `parent_session_id` have no
/// reader on any screen. The ones here each have one:
///
/// * `id` — the row key and every action's address;
/// * `tmux_name`, `friendly_name`, `last_prompt` — `SessionRow.displayName`
///   picks among exactly these three (a `bg:` row titles itself with its
///   prompt);
/// * `host_alias`, `project_id` — the row's two group headings;
/// * `status`, `kind` — `kind` is the second line when there is no activity;
/// * `claude_status`, `stuck_kind` — the row's state dot AND the whole of
///   `needsAttention`;
/// * `current_activity` — the second line;
/// * `context_pct`, `ci_status` — the row's two trailing badges;
/// * `last_activity_at` — the age column, and the list's sort key;
/// * `needs_attention` — the hub's own answer to the question the pager is
///   opened to ask, with its reason and since. Projected away, the view
///   would hand a phone the columns to re-derive it and not the answer;
///   `service::attention` exists so that one rule decides it for the
///   desktop, the phone and anything that later sends a push;
/// * `pending_input` — the dialog a blocked session is waiting on, with its
///   options. Without it the view is a list a phone can read and not act on:
///   the one thing a pager exists for is answering that dialog, and a row
///   that says `claude_status: "blocked"` and nothing else forces the app
///   back to the full 52 KB answer to find out what the question was.
/// * `is_controller` — the session card refuses Restart and Kill on the
///   controller; projected away it reads as `false`, and the phone would
///   offer to kill the session driving it;
/// * `tags` — the card's tag editor starts from the row's current tags;
/// * `turn_seq` — whether a row change is a new turn, which is what decides
///   that a conversation needs re-reading at all;
/// * `safe_kill_state` — a retirement already armed, so the card shows it
///   rather than offering it again;
/// * `started_at`, `last_turn_at`, `last_stop_at`, `usage_cost_micros`,
///   `usage_model` — the session screen's status strip: elapsed time, cost
///   and model;
/// * `work` — the session's primary work link (key, title): the row's key
///   chip and the grouping by work. Small, and null for most rows.
///
/// Added 2026-09-23 when fleet-mobile's pager (its PR #19) began reading
/// them: the first cut of this view was taken against the list screen alone.
///
/// [`super::tests`] pins this set against the serialized row so a renamed
/// column cannot quietly fall out of the view, and pins the list itself so
/// dropping an entry is a deliberate edit rather than a silent regression on
/// a phone that then draws a blank column.
pub(super) const PHONE_SESSION_FIELDS: &[&str] = &[
    "ci_status",
    "claude_status",
    "context_pct",
    "current_activity",
    "friendly_name",
    "host_alias",
    "id",
    "is_controller",
    "kind",
    "last_activity_at",
    "last_prompt",
    "last_stop_at",
    "last_turn_at",
    "needs_attention",
    "pending_input",
    "project_id",
    "safe_kill_state",
    "started_at",
    "status",
    "stuck_kind",
    "tags",
    "tmux_name",
    "turn_seq",
    "usage_cost_micros",
    "usage_model",
    "work",
];

/// A named projection a caller may ask `list_sessions` for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionView {
    /// The columns a phone-sized session list draws. See
    /// [`PHONE_SESSION_FIELDS`].
    Phone,
}

impl SessionView {
    /// Every view name this build serves, for the schema and the refusal.
    pub(super) const NAMES: &'static [&'static str] = &["phone"];

    /// Parse a caller's `view`. Case- and whitespace-insensitive, because the
    /// value is typed by hand into an app's source once and never again.
    pub(super) fn parse(name: &str) -> Result<Self, McpError> {
        match name.trim().to_ascii_lowercase().as_str() {
            "phone" => Ok(Self::Phone),
            other => Err(mcp_err(
                codes::E_INVALID,
                format!(
                    "unknown view {other:?}; known views: {}",
                    Self::NAMES.join(", ")
                ),
                None,
            )),
        }
    }

    pub(super) fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Phone => PHONE_SESSION_FIELDS,
        }
    }
}

/// Keep only `fields` on each object of an array of rows.
///
/// Rows only: a projection is a statement about a list's columns, so a value
/// that is not an array of objects is left exactly as it is rather than
/// half-projected.
pub(super) fn project_rows(value: &mut Value, fields: &[&str]) {
    let Value::Array(rows) = value else { return };
    for row in rows {
        if let Value::Object(map) = row {
            map.retain(|k, _| fields.contains(&k.as_str()));
        }
    }
}
