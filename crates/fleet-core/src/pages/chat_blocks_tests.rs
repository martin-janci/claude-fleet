use super::chat_blocks::*;
use serde_json::{json, Value};

fn cases() -> Vec<Value> {
    let rel = "docs/chat-block-examples/blocks.json";
    let doc: Value = serde_json::from_str(&crate::repo_files::read(rel)).expect(rel);
    doc["cases"].as_array().unwrap().clone()
}

#[test]
fn every_shared_block_case_reports_exactly_its_problems() {
    for case in cases() {
        let name = case["name"].as_str().unwrap();
        let want: Vec<String> = serde_json::from_value(case["problems"].clone()).unwrap();
        let got = check(&case["block"]).err().unwrap_or_default();
        assert_eq!(got, want, "case {name:?}");
    }
}

#[test]
fn every_valid_case_but_a_loose_report_fits_the_schema_model() {
    for case in cases() {
        let name = case["name"].as_str().unwrap();
        if case["problems"].as_array().is_some_and(|p| !p.is_empty()) {
            continue;
        }
        let block: ChatBlock = serde_json::from_value(case["block"].clone())
            .unwrap_or_else(|e| panic!("case {name:?}: {e}"));
        assert_eq!(block.spec, UI_SPEC, "case {name:?}");
    }
}

#[test]
fn text_is_checked_for_size_and_json_before_shape() {
    let big = format!(
        r#"{{"spec":"fleet.ui/1","kind":"callout","body":"{}"}}"#,
        "x".repeat(MAX_BLOCK_BYTES)
    );
    assert_eq!(check_text(&big).unwrap_err(), ["is larger than 32 KiB"]);
    assert_eq!(check_text("{nope").unwrap_err(), ["is not valid JSON"]);
    assert_eq!(check(&json!([1])).unwrap_err(), ["must be a JSON object"]);
    assert!(check_text(r#"{"spec":"fleet.ui/1","kind":"callout","body":"x"}"#).is_ok());
}

#[test]
fn at_most_twenty_problems_are_reported() {
    let rows: Vec<Value> = (0..40).map(|_| json!("row")).collect();
    let block = json!({ "spec": "fleet.ui/1", "kind": "results", "items": [
        { "type": "table", "columns": [ { "label": "A" } ], "rows": rows } ] });
    assert_eq!(check(&block).unwrap_err().len(), 20);
}

#[test]
fn unknown_keys_are_ignored_for_a_newer_writer() {
    let block =
        json!({ "spec": "fleet.ui/1", "kind": "progress", "id": "a", "title": "T", "eta_s": 30 });
    assert!(check(&block).is_ok());
}
