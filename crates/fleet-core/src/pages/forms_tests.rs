use super::forms::*;
use serde_json::{json, Map, Value};

fn cases(rel: &str) -> Value {
    serde_json::from_str(&crate::repo_files::read(rel)).expect(rel)
}

#[test]
fn every_shared_spec_case_reports_exactly_its_problems() {
    let doc = cases("docs/form-examples/specs.json");
    for case in doc["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let want: Vec<String> = serde_json::from_value(case["problems"].clone()).unwrap();
        let got = match parse(&case["spec"]) {
            Ok(_) => vec![],
            Err(p) => p,
        };
        assert_eq!(got, want, "case {name:?}");
    }
}

#[test]
fn every_shared_answer_case_agrees() {
    let doc = cases("docs/form-examples/answers.json");
    let form = parse(&doc["spec"]).expect("the answers spec is valid");
    for case in doc["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let values: Map<String, Value> = serde_json::from_value(case["values"].clone()).unwrap();
        match check_answers(&form, &values) {
            Ok(a) => {
                assert_eq!(Value::Object(a.values), case["answers"], "case {name:?}");
                let secrets: Vec<&str> = a.secrets.keys().map(String::as_str).collect();
                let want: Vec<&str> = case["secrets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap())
                    .collect();
                assert_eq!(secrets, want, "case {name:?}");
            }
            Err(p) => {
                assert_eq!(
                    serde_json::to_value(&p).unwrap(),
                    case["problems"],
                    "case {name:?}"
                );
            }
        }
    }
}

#[test]
fn a_secrets_value_is_kept_apart_from_the_answers() {
    let form = parse(&json!({ "spec": "fleet.form/1", "title": "T", "steps": [
        { "title": "A", "fields": [ { "name": "pw", "type": "secret", "label": "P", "required": true } ] } ] }))
    .unwrap();
    let values: Map<String, Value> = serde_json::from_value(json!({ "pw": "hunter2" })).unwrap();
    let a = check_answers(&form, &values).unwrap();
    assert!(a.values.is_empty(), "never in the answers: {:?}", a.values);
    assert_eq!(a.secrets.get("pw").map(String::as_str), Some("hunter2"));
}

#[test]
fn debug_names_a_secret_but_never_prints_its_value() {
    let mut a = Answers::default();
    a.secrets.insert("pw".into(), "hunter2".into());
    let shown = format!("{a:?}");
    assert!(shown.contains("pw"), "{shown}");
    assert!(!shown.contains("hunter2"), "{shown}");
}

#[test]
fn an_oversized_spec_is_refused_before_parsing() {
    let big = "x".repeat(MAX_SPEC_BYTES);
    let err = parse(&json!({ "spec": "fleet.form/1", "title": big, "steps": [] })).unwrap_err();
    assert!(
        err[0].contains("bytes, over the 16384 a form may have"),
        "{err:?}"
    );
}

#[test]
fn unknown_keys_are_refused() {
    let err = parse(&json!({ "spec": "fleet.form/1", "title": "T", "colour": "red", "steps": [] }))
        .unwrap_err();
    assert!(err[0].starts_with("not a fleet.form/1 spec:"), "{err:?}");
}

fn repo_path(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

#[test]
fn form_docs_are_current() {
    // Both generated from the Rust models; one command regenerates both.
    let docs = [
        (
            "docs/form-spec.schema.json",
            "crates/fleet-core/src/pages/forms.rs",
            rmcp::schemars::schema_for!(FormSpec),
        ),
        (
            "docs/chat-block.schema.json",
            "crates/fleet-core/src/pages/chat_blocks.rs",
            rmcp::schemars::schema_for!(super::chat_blocks::ChatBlock),
        ),
    ];
    let regen = std::env::var("REGEN_FORM_DOCS").is_ok();
    let mut stale = vec![];
    for (rel, source, schema) in docs {
        let text = serde_json::to_string_pretty(&schema).unwrap() + "\n";
        let path = repo_path(rel);
        if std::fs::read_to_string(&path).unwrap_or_default() == text {
            continue;
        }
        if regen {
            std::fs::write(&path, &text).expect("write");
        }
        stale.push(format!("{rel} (from {source})"));
    }
    if regen {
        panic!(
            "wrote {} — read the diff, then run again without REGEN_FORM_DOCS",
            if stale.is_empty() {
                "nothing".to_string()
            } else {
                stale.join(", ")
            }
        );
    }
    assert!(
        stale.is_empty(),
        "\n\n{} out of date. Regenerate with:\n  \
REGEN_FORM_DOCS=1 cargo fleet-test -- form_docs_are_current\n",
        stale.join(", ")
    );
}

/// Every wizard the app ships as a form (redesign step 10.12,
/// `src/lib/forms/wizards/*.json`) is a valid fleet.form/1 spec, so the same
/// file renders as a dialog, as a chat form and on the phone.
#[test]
fn every_wizard_spec_is_valid() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/lib/forms/wizards");
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).expect("the wizards directory") {
        let name = entry.unwrap().file_name().into_string().unwrap();
        if !name.ends_with(".json") {
            continue;
        }
        let spec = cases(&format!("src/lib/forms/wizards/{name}"));
        if let Err(problems) = parse(&spec) {
            panic!("{name}: {problems:?}");
        }
        seen += 1;
    }
    assert!(seen > 0, "no wizard specs in {}", dir.display());
}

/// Redesign 10.12: every wizard a `wizard` chat block may open has its spec
/// file, and the app's list (`chat_wizard_ids.ts`) is the same, in order.
#[test]
fn every_chat_wizard_has_a_spec_and_the_app_lists_the_same() {
    use super::chat_blocks::CHAT_WIZARDS;
    for id in CHAT_WIZARDS {
        let spec = cases(&format!("src/lib/forms/wizards/{id}.json"));
        if let Err(problems) = parse(&spec) {
            panic!("{id}: {problems:?}");
        }
    }
    let ts = crate::repo_files::read("src/lib/forms/chat_wizard_ids.ts");
    let quoted: Vec<String> = CHAT_WIZARDS.iter().map(|id| format!("'{id}'")).collect();
    let line = format!(
        "export const CHAT_WIZARD_IDS = [{}] as const;",
        quoted.join(", ")
    );
    assert!(ts.contains(&line), "chat_wizard_ids.ts should hold: {line}");
}

/// Every older spec still parses, and serialises back to exactly what it
/// was: an option pair stays a pair, and no new key appears.
#[test]
fn an_older_spec_round_trips_unchanged() {
    let doc = cases("docs/form-examples/specs.json");
    let old = &doc["cases"][0]["spec"];
    let form = parse(old).expect("the spec's own example is valid");
    assert_eq!(&serde_json::to_value(&form).unwrap(), old);
    let kind = &form.steps[0].fields[1];
    assert_eq!(
        kind.options.as_ref().unwrap()[0],
        FormOption::Pair("web".into(), "Web app".into())
    );
    assert_eq!(form.steps[0].kind, StepKind::Fields);
    assert!(!form.save_later);
}

#[test]
fn an_option_is_a_pair_or_an_object_and_both_read_alike() {
    let form = parse(&json!({ "spec": "fleet.form/1", "title": "T", "steps": [
        { "title": "A", "fields": [ { "name": "host", "type": "select", "label": "Host", "options": [
            ["mercury", "Mercury"],
            { "value": "venus", "label": "Venus", "detail": "2 idle",
              "proposed": { "by": "jev", "reason": "the last three ran there" } } ] } ] },
        { "title": "Check", "name": "Review", "kind": "review" } ] }))
    .unwrap();
    let opts = form.steps[0].fields[0].options.clone().unwrap();
    let read: Vec<(&str, &str, Option<&str>)> = opts
        .iter()
        .map(|o| (o.value(), o.label(), o.detail()))
        .collect();
    assert_eq!(
        read,
        [
            ("mercury", "Mercury", None),
            ("venus", "Venus", Some("2 idle"))
        ]
    );
    assert_eq!(opts[1].proposed().unwrap().by, ProposedBy::Jev);
    assert_eq!(form.steps[1].kind, StepKind::Review);
    assert!(form.steps[1].fields.is_empty());
    // Both shapes keep their own spelling on the way out.
    let back = serde_json::to_value(&form).unwrap();
    assert_eq!(
        back["steps"][0]["fields"][0]["options"][0],
        json!(["mercury", "Mercury"])
    );
    assert_eq!(
        back["steps"][0]["fields"][0]["options"][1]["detail"],
        "2 idle"
    );
    assert_eq!(back["steps"][1]["kind"], "review");
}

#[test]
fn a_disabled_field_is_never_answered_and_another_takes_free_text() {
    let form = parse(&json!({ "spec": "fleet.form/1", "title": "T", "steps": [
        { "title": "A", "fields": [
            { "name": "tier", "type": "select", "label": "Tier", "value": "s",
              "disabled_reason": "Needs an admin", "options": [["s", "Small"], ["l", "Large"]] },
            { "name": "host", "type": "select", "label": "Host", "other": true, "options": [["m", "M"]] } ] } ] }))
    .unwrap();
    let values: Map<String, Value> =
        serde_json::from_value(json!({ "tier": "l", "host": "pluto" })).unwrap();
    let a = check_answers(&form, &values).unwrap();
    assert_eq!(Value::Object(a.values), json!({ "host": "pluto" }));
}
