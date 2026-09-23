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
/// Derived from what the phone actually draws, not from what its model
/// declares: `SessionRow.kt` deserialises 28 fields, but `branch`, `tags`,
/// `pr_url`, `turn_seq`, `lost_at`, `created_at`, `started_at`,
/// `last_stop_at`, `last_turn_at`, `usage_*` and `parent_session_id` have no
/// reader on any screen. The 14 here are the ones with one:
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
/// * `last_activity_at` — the age column, and the list's sort key.
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
    "kind",
    "last_activity_at",
    "last_prompt",
    "project_id",
    "status",
    "stuck_kind",
    "tmux_name",
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
