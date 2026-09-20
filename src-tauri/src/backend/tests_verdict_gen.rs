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
fn render_json_key_order_matches_the_controllers_shape() {
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

// ── doc table rendering ─────────────────────────────────────────────────

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
    assert_eq!(names.len(), VERDICTS.len());
}

#[test]
fn doc_table_pipe_escapes_a_sentence_that_contains_one() {
    let verdict = Verdict::LocalOnly { instead: "a | b" };
    assert_eq!(detail_cell(&verdict), "a \\| b");
}

#[test]
fn doc_table_names_the_tool_for_a_routed_command() {
    let table = render_doc_table();
    let row = table
        .lines()
        .find(|l| l.starts_with("| `list_sessions`"))
        .expect("list_sessions row");
    assert!(row.contains("Routed"));
    assert!(row.contains("`list_sessions`"));
}

#[test]
fn doc_table_names_the_refusal_for_a_local_only_command() {
    let table = render_doc_table();
    let row = table
        .lines()
        .find(|l| l.starts_with("| `add_host`"))
        .expect("add_host row");
    assert!(row.contains("Local-only"));
    assert!(row.contains("fleet administration"));
}

#[test]
fn doc_table_marks_the_routed_unless_row_distinctly() {
    let table = render_doc_table();
    let row = table
        .lines()
        .find(|l| l.starts_with("| `repair_session`"))
        .expect("repair_session row");
    assert!(row.contains("Routed, unless"));
    assert!(row.contains("`repair_session`; otherwise:"));
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
