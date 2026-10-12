//! An item's condition lines and a person's checks (orchestration O3).

use super::*;

#[test]
fn lines_are_trimmed_deduplicated_and_capped() {
    let lines = vec![
        " review ".to_string(),
        "".into(),
        "review".into(),
        "ci".into(),
    ];
    assert_eq!(normalize_done_when(&lines).unwrap(), vec!["review", "ci"]);
    let many: Vec<String> = (0..=DONE_WHEN_MAX).map(|i| format!("l{i}")).collect();
    assert_eq!(
        normalize_done_when(&many).unwrap_err().code,
        codes::E_INVALID
    );
    let long = vec!["x".repeat(DONE_WHEN_LINE_MAX_CHARS + 1)];
    assert_eq!(
        normalize_done_when(&long).unwrap_err().code,
        codes::E_INVALID
    );
}

#[test]
fn done_when_is_stored_and_the_newest_check_per_line_wins() {
    let s = Store::open_in_memory().unwrap();
    let item = s.create_local_work_item(None, "queue").unwrap().id;
    assert!(s
        .set_item_done_when(item, &["review".into(), "person".into()], "person:1")
        .unwrap());
    assert!(!s
        .set_item_done_when(item, &["review".into(), "person".into()], "person:1")
        .unwrap());
    assert_eq!(
        s.get_work_item(item).unwrap().unwrap().done_when,
        vec!["review", "person"]
    );
    s.record_verification(item, "person", false, "person:1", Some(" not yet "))
        .unwrap();
    s.record_verification(item, "person", true, "person:1", None)
        .unwrap();
    s.record_verification(item, "review", false, "person:1", None)
        .unwrap();
    let latest = s.latest_verifications(item).unwrap();
    assert_eq!(latest.len(), 2);
    let person = latest.iter().find(|r| r.line == "person").unwrap();
    assert!(person.ok);
    assert!(s.set_item_done_when(item, &[], "person:1").unwrap());
    assert!(s.get_work_item(item).unwrap().unwrap().done_when.is_empty());
    assert_eq!(
        s.set_item_done_when(999_999, &[], "p").unwrap_err().code,
        codes::E_NOTFOUND
    );
}
