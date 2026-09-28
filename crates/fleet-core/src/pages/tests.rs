//! The contract every page spec is held to, and the generated docs an
//! author (a person or an agent) writes a spec against.

use super::catalog::{self, LAYOUTS, WIDGETS};
use super::model::Page;
use super::sources::SOURCES;
use super::validate::{unplaced_keys, validate};
use super::{parse, PAGE_FILES, UNLISTED};
use crate::service::settings::{KindDesc, SPECS};
use serde_json::{json, Value};
use std::path::PathBuf;

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn compiled_pages() -> Vec<Page> {
    PAGE_FILES
        .iter()
        .map(|(file, text)| parse(file, text).unwrap_or_else(|e| panic!("{e}")))
        .collect()
}

#[test]
fn every_page_parses_and_validates() {
    let pages = compiled_pages();
    let problems = validate(&pages);
    assert!(
        problems.is_empty(),
        "\n{}\n",
        problems
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert_eq!(super::all().len(), PAGE_FILES.len(), "all() drops nothing");
}

#[test]
fn every_page_file_is_compiled_in() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("pages");
    let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
        .expect("read pages/")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    on_disk.sort();
    let mut listed: Vec<String> = PAGE_FILES.iter().map(|(f, _)| f.to_string()).collect();
    listed.sort();
    assert_eq!(on_disk, listed, "list every file in pages/ in PAGE_FILES");
    for (file, text) in PAGE_FILES {
        let page = parse(file, text).unwrap();
        assert_eq!(
            *file,
            format!("{}.json", page.id),
            "a page's file is named after its id"
        );
    }
}

/// Coverage: a new setting cannot ship without a place in the UI (the
/// generated pages replace `every_spec_has_a_settings_dialog_row` once the
/// renderer lands).
#[test]
fn every_setting_has_one_home() {
    let unplaced = unplaced_keys(&compiled_pages(), UNLISTED);
    assert!(
        unplaced.is_empty(),
        "these settings are on no page: {unplaced:?}. Place each as a `field` \
         (one home per setting), or add it to pages::UNLISTED with a reason."
    );
    for (key, why) in UNLISTED {
        assert!(
            crate::service::settings::spec(key).is_some(),
            "UNLISTED names `{key}`, not a setting"
        );
        assert!(
            why.ends_with('.'),
            "UNLISTED `{key}`: say why, in a sentence"
        );
    }
}

// ── the validator refuses what the registries do not know ──

fn page(v: Value) -> Page {
    serde_json::from_value(v).expect("parses")
}

fn category(items: Value) -> Page {
    page(json!({
        "spec": "fleet.page/1", "id": "t", "title": "T", "layout": "category",
        "sections": [{ "title": "S", "items": items }]
    }))
}

fn messages(pages: &[Page]) -> String {
    validate(pages)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_valid_page_has_no_problems() {
    let p = category(json!([
        { "type": "field", "key": "gc.enabled" },
        { "type": "field", "key": "gc.bg_idle_secs", "when": { "key": "gc.enabled", "truthy": true } },
        { "type": "field", "key": "projects.layout", "widget": "radio" },
        { "type": "stat", "source": { "id": "usage.total" }, "field": "cost_micros" }
    ]));
    assert_eq!(messages(&[p]), "");
}

#[test]
fn unknown_keys_sources_and_widgets_are_refused() {
    let cases = [
        (
            json!([{ "type": "field", "key": "no.such" }]),
            "not a registered setting",
        ),
        (
            json!([{ "type": "field", "key": "gc.enabled", "widget": "number" }]),
            "cannot use the Number widget",
        ),
        (
            json!([{ "type": "stat", "source": { "id": "nope" } }]),
            "not a data source",
        ),
        (
            json!([{ "type": "stat", "source": { "id": "usage.total" }, "field": "nope" }]),
            "has no field `nope`",
        ),
        (
            json!([{ "type": "stat", "source": { "id": "usage.by_day", "params": { "days": 9999 } } }]),
            "whole number of days",
        ),
        (
            json!([{ "type": "field", "key": "gc.enabled", "when": { "key": "gc.enabled", "eq": "maybe" } }]),
            "can never be",
        ),
        (
            json!([{ "type": "field", "key": "gc.enabled", "when": { "key": "work.summary_model", "truthy": true } }]),
            "truthy is for on/off settings",
        ),
        (
            json!([{ "type": "field", "key": "gc.enabled", "when": { "key": "gc.enabled", "truthy": true, "all": [] } }]),
            "exactly one of",
        ),
        (
            json!([{ "type": "field", "key": "gc.enabled" }, { "type": "field", "key": "gc.enabled" }]),
            "already placed",
        ),
        (json!([{ "type": "link", "page": "nowhere" }]), "not a page"),
        (
            json!([{ "type": "notice", "tone": "info", "text": "<script>x</script>" }]),
            "no markup",
        ),
    ];
    for (items, want) in cases {
        let got = messages(&[category(items.clone())]);
        assert!(
            got.contains(want),
            "{items}\n  wanted: {want}\n  got: {got}"
        );
    }
}

#[test]
fn a_layout_holds_only_its_items() {
    let p = page(json!({
        "spec": "fleet.page/1", "id": "t", "title": "T", "layout": "data_page",
        "sections": [{ "title": "S", "items": [
            { "type": "field", "key": "gc.enabled" },
            { "type": "chart", "source": { "id": "usage.by_model" }, "chart": "bar" },
            { "type": "table", "source": { "id": "usage.by_day" } }
        ] }]
    }));
    let got = messages(&[p]);
    assert!(got.contains("cannot hold a `field`"), "{got}");
    assert!(got.contains("a chart shows a series, not rows"), "{got}");
    assert!(got.contains("a table shows rows, not series"), "{got}");

    let p = page(json!({
        "spec": "fleet.page/1", "id": "t", "title": "T", "layout": "master_detail",
        "sections": [{ "title": "S", "items": [{ "type": "notice", "tone": "info", "text": "x" }] }]
    }));
    assert!(messages(&[p]).contains("needs the resource registry"));
}

#[test]
fn page_level_rules_hold() {
    let base = |id: &str, parent: Option<&str>| {
        let mut v = json!({
            "spec": "fleet.page/1", "id": id, "title": "T", "layout": "cards",
            "sections": [{ "title": "S", "items": [{ "type": "notice", "tone": "info", "text": "x" }] }]
        });
        if let Some(p) = parent {
            v["parent"] = json!(p);
        }
        page(v)
    };
    assert!(messages(&[base("Bad Id", None)]).contains("dotted lowercase"));
    assert!(messages(&[base("a", None), base("a", None)]).contains("another page has this id"));
    assert!(messages(&[base("a", Some("missing"))]).contains("not a page"));
    assert!(messages(&[base("a", Some("b")), base("b", Some("a"))]).contains("lead back to itself"));

    let mut v = serde_json::to_value(base("a", None)).unwrap();
    v["spec"] = json!("fleet.page/2");
    assert!(messages(&[page(v)]).contains("spec must be"));
}

#[test]
fn unknown_fields_are_refused_when_parsing() {
    let err = parse(
        "x.json",
        r#"{"spec":"fleet.page/1","id":"x","title":"X","layout":"cards","style":"red","sections":[]}"#,
    )
    .unwrap_err();
    assert!(err.contains("unknown field `style`"), "{err}");
    let err = parse(
        "x.json",
        r#"{"spec":"fleet.page/1","id":"x","title":"X","layout":"cards","sections":[{"title":"S","items":[{"type":"html","html":"<b>"}]}]}"#,
    )
    .unwrap_err();
    assert!(err.contains("unknown variant `html`"), "{err}");
}

// ── generated docs for authors ──

/// What an author can name: every layout and the items it holds, every
/// widget and the setting kinds it takes, every setting with its default
/// and allowed widgets, and every data source with its shape and params.
fn render_catalog() -> String {
    let kind_type = |k| {
        serde_json::to_value(KindDesc::from(k)).unwrap()["type"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let layouts: Vec<Value> = LAYOUTS
        .iter()
        .map(|l| {
            json!({
                "id": l,
                "supported": catalog::layout_supported(*l),
                "items": catalog::layout_item_types(*l),
            })
        })
        .collect();
    let widgets: Vec<Value> = WIDGETS
        .iter()
        .map(|w| {
            let mut kinds: Vec<String> = SPECS
                .iter()
                .filter(|s| catalog::accepts(*w, s.kind))
                .map(|s| kind_type(s.kind))
                .collect();
            kinds.sort();
            kinds.dedup();
            json!({ "id": w, "kinds": kinds })
        })
        .collect();
    let settings: Vec<Value> = SPECS
        .iter()
        .map(|s| {
            let widgets: Vec<_> = WIDGETS
                .iter()
                .filter(|w| catalog::accepts(**w, s.kind))
                .collect();
            json!({
                "key": s.key,
                "label": s.label,
                "kind": KindDesc::from(s.kind),
                "default_widget": catalog::default_widget(s.kind),
                "widgets": widgets,
            })
        })
        .collect();
    let v = json!({
        "_generated": "from crates/fleet-core/src/pages; regenerate with REGEN_PAGE_DOCS=1 cargo test -p fleet-core page_docs_are_current",
        "spec": super::model::SPEC_VERSION,
        "layouts": layouts,
        "widgets": widgets,
        "settings": settings,
        "sources": SOURCES,
    });
    serde_json::to_string_pretty(&v).unwrap() + "\n"
}

fn render_schema() -> String {
    let schema = rmcp::schemars::schema_for!(Page);
    serde_json::to_string_pretty(&schema).unwrap() + "\n"
}

#[test]
fn page_docs_are_current() {
    let regen = std::env::var("REGEN_PAGE_DOCS").is_ok();
    let mut stale = Vec::new();
    for (rel, text) in [
        ("docs/page-spec.schema.json", render_schema()),
        ("docs/page-catalog.json", render_catalog()),
    ] {
        let path = repo_path(rel);
        if regen {
            std::fs::write(&path, &text).expect("write");
        } else if std::fs::read_to_string(&path).unwrap_or_default() != text {
            stale.push(rel);
        }
    }
    assert!(
        stale.is_empty(),
        "\n\n{} out of date with crates/fleet-core/src/pages. Regenerate with:\n  \
REGEN_PAGE_DOCS=1 cargo test -p fleet-core page_docs_are_current\n",
        stale.join(", ")
    );
}

#[test]
fn the_catalog_names_what_the_validator_accepts() {
    let v: Value = serde_json::from_str(&render_catalog()).unwrap();
    let supported: Vec<&str> = v["layouts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["supported"] == true)
        .map(|l| l["id"].as_str().unwrap())
        .collect();
    assert_eq!(supported, ["category", "cards", "data_page"]);
    assert_eq!(
        v["settings"].as_array().unwrap().len(),
        SPECS.len(),
        "every setting is in the catalog"
    );
}
