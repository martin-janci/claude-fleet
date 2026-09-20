//! Publishes [`VERDICTS`](super::verdicts::VERDICTS) to the two places that
//! used to keep their own hand-synced copy of it: the frontend's list of
//! routed actions and the refusal reasons it gates on, and `docs/hub.md`'s
//! prose list of what a hub client cannot do.
//!
//! Two generated artifacts, same REGEN pattern as [`super::contract`]'s
//! `hub_contract.golden.json` (see that module's header):
//!
//! - `src/lib/hub_verdicts.generated.json` — every command name, bucketed by
//!   verdict kind, sorted. [`verdict_lists`] computes it; [`render_json`]
//!   renders it. The frontend does not import this at runtime — `hub.ts`
//!   keeps its own literal `ROUTED_ACTIONS`/`REASONS` (a generated-file-driven
//!   type would have to be exactly as precise as the hand-written ones to be
//!   worth it, and it is not), and `hub_verdicts.test.ts` reads this file to
//!   hold them to it instead.
//! - `docs/hub.md`, between `BEGIN_MARKER`/`END_MARKER` — one row per
//!   command: its verdict, and the hub tool it calls or the sentence it
//!   refuses with. [`render_doc_table`] renders it; [`splice_doc`] replaces
//!   the marked block in the file without touching the hand-written prose
//!   around it.
//!
//! Regenerate both with:
//! ```text
//! REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen
//! ```
//! Read the diff before committing it, the same as `hub_contract.golden.json`:
//! a command that moved between buckets changed what a hub client can do from
//! here, and the regenerate run fails once on purpose (see the tests) so that
//! diff cannot go unread.

use super::verdicts::{Verdict, VERDICTS};
use serde::Serialize;

/// Where the generated JSON lives, relative to this file (`CARGO_MANIFEST_DIR`
/// is `src-tauri/`).
pub const JSON_REL_PATH: &str = "../src/lib/hub_verdicts.generated.json";
/// Where `docs/hub.md` lives, relative to this file.
pub const DOC_REL_PATH: &str = "../docs/hub.md";
/// The env var that rewrites both generated artifacts instead of asserting
/// against them.
pub const REGEN_ENV: &str = "REGEN_HUB_VERDICTS";

/// The marker `docs/hub.md` wraps the generated table in. Everything between
/// the two lines is replaced verbatim on regenerate; everything outside them
/// — the prose explaining *why* — is untouched.
pub const BEGIN_MARKER: &str = "<!-- BEGIN GENERATED: hub-client verdicts -->";
pub const END_MARKER: &str = "<!-- END GENERATED: hub-client verdicts -->";

/// One row of [`VerdictLists::routed_unless`] — `repair_session`'s shape
/// (routes for one argument shape, refuses for another) has no home in a
/// flat name list, so it gets its own small object instead of being folded
/// into `routed` (which would claim it always routes) or `local_only` (which
/// would claim it never does).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoutedUnlessEntry {
    pub command: String,
    pub unless: String,
}

/// Every command name in [`VERDICTS`], bucketed by verdict kind. Field order
/// is the JSON's key order (`serde_json` preserves struct declaration order),
/// and matches the shape the controller specified: `local_only`, `routed`,
/// `routed_unless`, `same_in_both`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct VerdictLists {
    pub local_only: Vec<String>,
    pub routed: Vec<String>,
    pub routed_unless: Vec<RoutedUnlessEntry>,
    pub same_in_both: Vec<String>,
}

/// [`VERDICTS`] read into [`VerdictLists`], every list sorted. `VERDICTS`
/// itself is in `generate_handler!` order (deliberately — see its header), so
/// the generated file needs its own sort to stay stable regardless of how
/// that table gets reordered. `repair_session`, the one [`Verdict::RoutedUnless`]
/// row, lands in `routed_unless` ONLY — not also in `routed`, which would
/// claim the hub always accepts it.
pub fn verdict_lists() -> VerdictLists {
    let mut lists = VerdictLists::default();
    for (name, verdict) in VERDICTS {
        match verdict {
            Verdict::Routed { .. } => lists.routed.push((*name).to_string()),
            Verdict::RoutedUnless { unless, .. } => {
                lists.routed_unless.push(RoutedUnlessEntry {
                    command: (*name).to_string(),
                    unless: (*unless).to_string(),
                });
            }
            Verdict::LocalOnly { .. } => lists.local_only.push((*name).to_string()),
            Verdict::SameInBoth { .. } => lists.same_in_both.push((*name).to_string()),
        }
    }
    lists.local_only.sort();
    lists.routed.sort();
    lists
        .routed_unless
        .sort_by(|a, b| a.command.cmp(&b.command));
    lists.same_in_both.sort();
    lists
}

/// Render [`VerdictLists`] as the committed JSON, trailing newline included.
pub fn render_json(lists: &VerdictLists) -> String {
    let mut json = serde_json::to_string_pretty(lists).expect("VerdictLists always serialises");
    json.push('\n');
    json
}

/// Escape the one character that breaks a GFM table cell. Backticks are left
/// alone: every sentence in [`super::verdicts`] uses them, deliberately, for
/// inline code (`` `gh` ``, `` `fleet-hub` ``), always in balanced pairs —
/// escaping them would turn that code back into plain text.
fn md_escape(s: &str) -> String {
    s.replace('|', "\\|")
}

/// The verdict-kind cell for one row.
fn verdict_cell(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Routed { .. } => "Routed".to_string(),
        Verdict::RoutedUnless { unless, .. } => {
            format!("Routed, unless {}", md_escape(unless))
        }
        Verdict::LocalOnly { .. } => "Local-only (`E_LOCAL_ONLY`)".to_string(),
        Verdict::SameInBoth { .. } => "Same in both modes".to_string(),
    }
}

/// The "hub tool or what to do instead" cell for one row.
fn detail_cell(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Routed { tool } => format!("`{tool}`"),
        Verdict::RoutedUnless { tool, instead, .. } => {
            format!("`{tool}`; otherwise: {}", md_escape(instead))
        }
        Verdict::LocalOnly { instead } => md_escape(instead),
        Verdict::SameInBoth { why } => md_escape(why),
    }
}

/// Render the whole table, one row per command in [`VERDICTS`], sorted
/// alphabetically by command name — `VERDICTS`' own order is
/// `generate_handler!`'s, which groups by feature area and is a worse read
/// as a lookup table than a straight alphabetical list.
pub fn render_doc_table() -> String {
    let mut rows: Vec<(&str, &Verdict)> = VERDICTS.iter().map(|(n, v)| (*n, v)).collect();
    rows.sort_by_key(|(name, _)| *name);

    let mut out = String::new();
    out.push_str(BEGIN_MARKER);
    out.push('\n');
    out.push_str(&format!(
        "<!-- Regenerate with: {REGEN_ENV}=1 cargo test -p claude-fleet --lib verdict_gen -->\n"
    ));
    out.push('\n');
    out.push_str("| Command | Verdict | Hub tool / what to do instead |\n");
    out.push_str("| --- | --- | --- |\n");
    for (name, verdict) in rows {
        out.push_str(&format!(
            "| `{}` | {} | {} |\n",
            name,
            verdict_cell(verdict),
            detail_cell(verdict)
        ));
    }
    out.push_str(END_MARKER);
    out
}

/// Replace the block between [`BEGIN_MARKER`] and [`END_MARKER`] in `doc`
/// with `table`, leaving everything else — including the markers themselves
/// — untouched. `Err` when either marker is missing, which the caller turns
/// into a panic naming what to fix (the markers are hand-added once; this
/// function never writes them).
pub fn splice_doc(doc: &str, table: &str) -> Result<String, String> {
    let start = doc
        .find(BEGIN_MARKER)
        .ok_or_else(|| format!("{DOC_REL_PATH} has no {BEGIN_MARKER} marker"))?;
    let after_begin = start + BEGIN_MARKER.len();
    let end_rel = doc[after_begin..]
        .find(END_MARKER)
        .ok_or_else(|| format!("{DOC_REL_PATH} has no {END_MARKER} marker"))?;
    let end = after_begin + end_rel + END_MARKER.len();
    // `table` already carries both markers (see `render_doc_table`), so the
    // replaced span runs from the start of BEGIN_MARKER to the end of
    // END_MARKER.
    Ok(format!("{}{}{}", &doc[..start], table, &doc[end..]))
}

#[cfg(test)]
#[path = "tests_verdict_gen.rs"]
mod tests;
