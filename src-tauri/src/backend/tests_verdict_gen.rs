//! Tests for [`super::verdict_gen`]: the two generated artifacts are current,
//! `verdict_lists` buckets and sorts correctly, and the marker splice is
//! exact.

use super::*;
use std::path::PathBuf;

fn json_abs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(JSON_REL_PATH)
}

fn doc_abs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DOC_REL_PATH)
}

/// **The JSON contract test.** `src/lib/hub_verdicts.generated.json` must
/// equal `render_json(&verdict_lists())`.
///
/// Mirrors `contract.rs`'s `the_hubs_field_names_are_the_ones_the_desktop_
/// reads`: on `REGEN_HUB_VERDICTS=1` it writes the file and then panics
/// anyway, so a regenerate can never pass silently — someone has to read
/// `git diff -- src/lib/hub_verdicts.generated.json` and decide whether a
/// command moving buckets was meant to happen.
#[test]
fn generated_json_is_current() {
    let expected = render_json(&verdict_lists());
    let path = json_abs();
    if std::env::var(REGEN_ENV).is_ok() {
        std::fs::write(&path, &expected).expect("write hub_verdicts.generated.json");
        panic!(
            "{JSON_REL_PATH} was regenerated. Read `git diff -- src/lib/hub_verdicts.generated.json`: \
             a command that moved between local_only/routed/routed_unless/same_in_both changed \
             what a hub client can do from here. Then unset {REGEN_ENV} and run again."
        );
    }
    let actual = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        actual, expected,
        "\n\nsrc/lib/hub_verdicts.generated.json is stale. Regenerate with:\n  \
         {REGEN_ENV}=1 cargo test -p claude-fleet --lib verdict_gen\n"
    );
}

/// **The docs contract test.** The block between `BEGIN_MARKER`/`END_MARKER`
/// in `docs/hub.md` must equal `render_doc_table()`. Same fail-after-write
/// regenerate behaviour as the JSON test above, for the same reason: a
/// command's verdict changing text is user-visible documentation, not a
/// mechanical rename.
#[test]
fn doc_table_is_current() {
    let path = doc_abs();
    let doc = std::fs::read_to_string(&path).expect("read docs/hub.md");
    let table = render_doc_table();
    let expected = splice_doc(&doc, &table).expect("docs/hub.md must carry both markers");
    if std::env::var(REGEN_ENV).is_ok() {
        std::fs::write(&path, &expected).expect("write docs/hub.md");
        panic!(
            "docs/hub.md was regenerated. Read `git diff -- docs/hub.md`: a command's verdict \
             or refusal sentence changed. Then unset {REGEN_ENV} and run again."
        );
    }
    assert_eq!(
        doc, expected,
        "\n\ndocs/hub.md's generated verdict table is stale. Regenerate with:\n  \
         {REGEN_ENV}=1 cargo test -p claude-fleet --lib verdict_gen\n"
    );
}

// ── verdict_lists ────────────────────────────────────────────────────────

#[test]
fn every_command_lands_in_exactly_one_bucket() {
    let lists = verdict_lists();
    let total = lists.local_only.len()
        + lists.routed.len()
        + lists.routed_unless.len()
        + lists.same_in_both.len();
    assert_eq!(
        total,
        VERDICTS.len(),
        "a command counted more than once, or not at all"
    );
}

#[test]
fn repair_session_is_in_routed_unless_only() {
    let lists = verdict_lists();
    assert!(
        lists
            .routed_unless
            .iter()
            .any(|e| e.command == "repair_session"),
        "repair_session missing from routed_unless"
    );
    assert!(
        !lists.routed.contains(&"repair_session".to_string()),
        "repair_session must not also claim to be unconditionally routed"
    );
    assert!(!lists.local_only.contains(&"repair_session".to_string()));
    assert!(!lists.same_in_both.contains(&"repair_session".to_string()));
}

#[test]
fn every_list_is_sorted() {
    let lists = verdict_lists();
    let mut local_only = lists.local_only.clone();
    local_only.sort();
    assert_eq!(lists.local_only, local_only);

    let mut routed = lists.routed.clone();
    routed.sort();
    assert_eq!(lists.routed, routed);

    let mut routed_unless = lists.routed_unless.clone();
    routed_unless.sort_by(|a, b| a.command.cmp(&b.command));
    assert_eq!(lists.routed_unless, routed_unless);

    let mut same_in_both = lists.same_in_both.clone();
    same_in_both.sort();
    assert_eq!(lists.same_in_both, same_in_both);
}

#[test]
fn a_known_command_lands_where_its_row_says() {
    let lists = verdict_lists();
    assert!(lists.routed.contains(&"list_sessions".to_string()));
    assert!(lists.local_only.contains(&"add_host".to_string()));
    assert!(lists
        .same_in_both
        .contains(&"collect_diagnostics".to_string()));
}

// ── render_json shape ───────────────────────────────────────────────────

#[test]
fn render_json_key_order_is_local_only_routed_routed_unless_same_in_both() {
    let json = render_json(&verdict_lists());
    let local_only_at = json.find("\"local_only\"").expect("local_only key");
    let routed_at = json.find("\"routed\"").expect("routed key");
    let routed_unless_at = json.find("\"routed_unless\"").expect("routed_unless key");
    let same_in_both_at = json.find("\"same_in_both\"").expect("same_in_both key");
    assert!(local_only_at < routed_at);
    assert!(routed_at < routed_unless_at);
    assert!(routed_unless_at < same_in_both_at);
    assert!(
        json.ends_with('\n'),
        "generated JSON must end with a newline"
    );
}

#[test]
fn routed_unless_entry_serialises_command_and_unless() {
    let lists = verdict_lists();
    let json = render_json(&lists);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let entry = parsed["routed_unless"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["command"] == "repair_session")
        .expect("repair_session row");
    assert_eq!(
        entry["unless"],
        serde_json::Value::String("explicit: false, the automatic pre-attach check".to_string())
    );
}

// ── doc table rendering: the REFUSAL table only (local_only + routed_unless) ─

#[test]
fn doc_table_has_exactly_the_local_only_and_routed_unless_rows() {
    let lists = verdict_lists();
    let table = render_doc_table();
    let names: Vec<&str> = table
        .lines()
        .filter(|l| l.starts_with("| `"))
        .map(|l| l.trim_start_matches("| `").split('`').next().unwrap())
        .collect();
    assert_eq!(
        names.len(),
        lists.local_only.len() + lists.routed_unless.len(),
        "row count must be local_only + routed_unless, not every command"
    );
    // Not VERDICTS.len(): Routed/SameInBoth commands must not appear at all.
    assert!(names.len() < VERDICTS.len());
}

#[test]
fn doc_table_is_sorted_alphabetically_by_command() {
    let table = render_doc_table();
    let names: Vec<&str> = table
        .lines()
        .filter(|l| l.starts_with("| `"))
        .map(|l| l.trim_start_matches("| `").split('`').next().unwrap())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}

#[test]
fn doc_table_omits_a_routed_command() {
    let table = render_doc_table();
    assert!(
        !table.lines().any(|l| l.starts_with("| `list_sessions`")),
        "a Routed row (\"| `list_sessions` | `list_sessions` |\") tells an operator nothing \
         they came to docs for — it must not appear in the refusal table"
    );
}

#[test]
fn doc_table_omits_a_same_in_both_command() {
    let table = render_doc_table();
    assert!(
        !table
            .lines()
            .any(|l| l.starts_with("| `collect_diagnostics`")),
        "a SameInBoth row is not a refusal and must not appear"
    );
}

#[test]
fn refusal_detail_is_none_for_routed_and_same_in_both() {
    assert!(refusal_detail(&Verdict::Routed {
        tool: "list_sessions"
    })
    .is_none());
    assert!(refusal_detail(&Verdict::SameInBoth { why: "why" }).is_none());
}

#[test]
fn refusal_detail_pipe_escapes_a_sentence_that_contains_one() {
    let verdict = Verdict::LocalOnly { instead: "a | b" };
    assert_eq!(refusal_detail(&verdict), Some("a \\| b".to_string()));
}

#[test]
fn doc_table_names_the_refusal_for_a_local_only_command() {
    let table = render_doc_table();
    let row = table
        .lines()
        .find(|l| l.starts_with("| `add_host`"))
        .expect("add_host row");
    assert!(row.contains("fleet administration"));
}

#[test]
fn doc_table_names_the_argument_shape_and_the_refusal_for_the_routed_unless_row() {
    let table = render_doc_table();
    let row = table
        .lines()
        .find(|l| l.starts_with("| `repair_session`"))
        .expect("repair_session row");
    assert!(row.contains("Refuses when"));
    assert!(row.contains("explicit: false, the automatic pre-attach check"));
    assert!(row.contains("otherwise routes to `repair_session`"));
}

// ── summary_sentence ─────────────────────────────────────────────────────

#[test]
fn summary_sentence_reports_todays_bucket_counts() {
    // The shape this file is pinned to: "Of the 123 commands, 35 route to a
    // hub tool, 1 routes except for one argument shape, 73 refuse, and 14
    // are the same in both modes; the full table is
    // `src-tauri/src/backend/verdicts.rs`." Pinned literally so a
    // regression in the verb-pluralisation logic is caught even if the real
    // counts drift.
    assert_eq!(
        summary_sentence(123, 35, 1, 73, 14),
        "Of the 123 commands, 35 route to a hub tool, 1 routes except for one argument shape, \
         73 refuse, and 14 are the same in both modes; the full table is \
         `src-tauri/src/backend/verdicts.rs`."
    );
}

#[test]
fn summary_sentence_pluralises_every_bucket_independently() {
    // Every bucket at exactly 1: every verb goes singular.
    assert_eq!(
        summary_sentence(4, 1, 1, 1, 1),
        "Of the 4 commands, 1 routes to a hub tool, 1 routes except for one argument shape, \
         1 refuses, and 1 is the same in both modes; the full table is \
         `src-tauri/src/backend/verdicts.rs`."
    );
    // Every bucket at 0 or 2+: every verb goes plural (0 reads as plural,
    // same as English "0 commands").
    assert_eq!(
        summary_sentence(0, 0, 0, 0, 0),
        "Of the 0 commands, 0 route to a hub tool, 0 route except for one argument shape, \
         0 refuse, and 0 are the same in both modes; the full table is \
         `src-tauri/src/backend/verdicts.rs`."
    );
}

#[test]
fn render_doc_table_includes_the_summary_sentence_with_real_counts() {
    let lists = verdict_lists();
    let table = render_doc_table();
    let expected = summary_sentence(
        VERDICTS.len(),
        lists.routed.len(),
        lists.routed_unless.len(),
        lists.local_only.len(),
        lists.same_in_both.len(),
    );
    assert!(
        table.contains(&expected),
        "generated table is missing the summary sentence:\n{expected}"
    );
}

// ── splice_doc ───────────────────────────────────────────────────────────

#[test]
fn splice_doc_replaces_only_the_marked_block() {
    let doc = "before\n<!-- BEGIN GENERATED: hub-client verdicts -->\nold\n<!-- END GENERATED: hub-client verdicts -->\nafter\n";
    let spliced = splice_doc(doc, "<!-- BEGIN GENERATED: hub-client verdicts -->\nnew\n<!-- END GENERATED: hub-client verdicts -->").unwrap();
    assert_eq!(
        spliced,
        "before\n<!-- BEGIN GENERATED: hub-client verdicts -->\nnew\n<!-- END GENERATED: hub-client verdicts -->\nafter\n"
    );
}

#[test]
fn splice_doc_refuses_when_a_marker_is_missing() {
    assert!(splice_doc("no markers here", "table").is_err());
    assert!(splice_doc(
        "<!-- BEGIN GENERATED: hub-client verdicts -->\nonly begin",
        "table"
    )
    .is_err());
}

#[test]
fn docs_hub_md_carries_both_markers_today() {
    // A precondition for `doc_table_is_current`, asserted on its own so a
    // missing marker fails with a clear message instead of inside that
    // test's `.expect`.
    let doc = std::fs::read_to_string(doc_abs()).expect("read docs/hub.md");
    assert!(doc.contains(BEGIN_MARKER), "missing {BEGIN_MARKER}");
    assert!(doc.contains(END_MARKER), "missing {END_MARKER}");
}

/// The two uploads do the same thing and must be classified alike.
///
/// `upload_to_session` is the terminal pane's drop handler; `upload_attachments`
/// is the composer's. Both take `local_paths` from THIS machine, address the
/// session by the `host_alias` passed in, carry the bytes over THIS machine's
/// `SshClient`, and read no `state.db` — the shapes are identical down to the
/// two validators. They were nonetheless split, `SameInBoth` against
/// `LocalOnly`, and the refusing one gave as its reason "Same reason as
/// upload_to_session" — citing, for the opposite conclusion, the command that
/// concluded the other way. The effect was that a paired desktop could drop a
/// file on the terminal pane but not attach one in the composer.
///
/// Being a hub client does not take this machine's disk or its ssh away; it
/// means the fleet's database and hosts are the hub's. Whatever these two are,
/// they are it together.
#[test]
fn the_two_uploads_are_classified_alike() {
    let row = |n: &str| {
        VERDICTS
            .iter()
            .find(|(name, _)| *name == n)
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("{n} has a row in VERDICTS"))
    };
    let pane = row("upload_to_session");
    let composer = row("upload_attachments");
    assert_eq!(
        std::mem::discriminant(pane),
        std::mem::discriminant(composer),
        "upload_to_session is {pane:?} but upload_attachments is {composer:?}; they copy \
         local bytes to a session's host over this machine's ssh in exactly the same way"
    );
    assert!(
        matches!(composer, Verdict::SameInBoth { .. }),
        "both uploads work from a paired desktop, so both are SameInBoth"
    );
}
