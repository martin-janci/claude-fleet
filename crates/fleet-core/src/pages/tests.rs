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
/// generated pages replaced `every_spec_has_a_settings_dialog_row` in P3).
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

fn org_page(sections: Value) -> Page {
    page(json!({
        "spec": "fleet.page/1", "id": "o", "title": "O", "layout": "master_detail",
        "resource": "org", "sections": sections
    }))
}

/// Every org field, once, in one section: the minimal valid org page.
fn all_org_fields() -> Value {
    let items: Vec<Value> = super::resources::resource("org")
        .unwrap()
        .fields
        .iter()
        .map(|f| json!({ "type": "field", "key": f.id }))
        .collect();
    json!(items)
}

#[test]
fn a_master_detail_page_lays_out_every_field_of_its_resource_once() {
    let ok = org_page(json!([{ "title": "All", "items": all_org_fields() }]));
    assert_eq!(messages(&[ok]), "");

    let mut short = all_org_fields();
    short.as_array_mut().unwrap().pop();
    let got = messages(&[org_page(json!([{ "title": "All", "items": short }]))]);
    assert!(got.contains("is on no section"), "{got}");

    let mut twice = all_org_fields();
    twice
        .as_array_mut()
        .unwrap()
        .push(json!({ "type": "field", "key": "name" }));
    let got = messages(&[org_page(json!([{ "title": "All", "items": twice }]))]);
    assert!(got.contains("`name` is already placed"), "{got}");

    let mut bad = all_org_fields();
    bad.as_array_mut().unwrap().extend([
        json!({ "type": "field", "key": "gc.enabled" }),
        json!({ "type": "field", "key": "color", "widget": "text" }),
        json!({ "type": "notice", "tone": "info", "text": "x" }),
    ]);
    let got = messages(&[org_page(json!([{ "title": "All", "items": bad,
        "when": { "key": "name", "truthy": true } }]))]);
    assert!(got.contains("`gc.enabled` is not a field of org"), "{got}");
    assert!(got.contains("no widget"), "{got}");
    assert!(got.contains("truthy is for on/off fields"), "{got}");
}

#[test]
fn a_resource_belongs_to_master_detail_only() {
    let p = page(json!({
        "spec": "fleet.page/1", "id": "t", "title": "T", "layout": "category", "resource": "org",
        "sections": [{ "title": "S", "items": [{ "type": "notice", "tone": "info", "text": "x" }] }]
    }));
    assert!(messages(&[p]).contains("only a master_detail page names a resource"));
    let p = page(json!({
        "spec": "fleet.page/1", "id": "t", "title": "T", "layout": "master_detail",
        "sections": [{ "title": "S", "items": [{ "type": "notice", "tone": "info", "text": "x" }] }]
    }));
    assert!(messages(&[p]).contains("names its resource"));
    let p = page(json!({
        "spec": "fleet.page/1", "id": "t", "title": "T", "layout": "master_detail", "resource": "nope",
        "sections": [{ "title": "S", "items": [{ "type": "notice", "tone": "info", "text": "x" }] }]
    }));
    assert!(messages(&[p]).contains("`nope` is not a resource"));
}

#[test]
fn custom_items_are_capped() {
    let item = json!({ "type": "custom", "component": "auto_tidy_preview" });
    let p = category(json!([item, item, item, item]));
    assert!(messages(&[p]).contains("over the cap of 3"));
}

// ── L8: embed pages and account usage ──

fn embed(id: &str, slot: &str, view: &str) -> Page {
    page(json!({
        "spec": "fleet.page/1", "id": id, "title": "E", "layout": "embed", "slot": slot,
        "sections": [{ "title": "S", "items": [
            { "type": "account_usage", "source": { "id": "accounts.usage" }, "view": view }
        ] }]
    }))
}

#[test]
fn an_embed_page_fills_one_slot_with_the_views_it_takes() {
    assert_eq!(messages(&[embed("e.a", "host_detail", "block")]), "");
    let got = messages(&[embed("e.a", "new_session_chip", "block")]);
    assert!(
        got.contains("slot takes [Chip], not the Block view"),
        "{got}"
    );
    let got = messages(&[
        embed("e.a", "status_footer", "footer"),
        embed("e.b", "status_footer", "footer"),
    ]);
    assert!(
        got.contains("e.a already fills the StatusFooter slot"),
        "{got}"
    );

    let no_slot = page(json!({
        "spec": "fleet.page/1", "id": "e", "title": "E", "layout": "embed",
        "sections": [{ "title": "S", "items": [
            { "type": "account_usage", "source": { "id": "accounts.usage" }, "view": "block" }
        ] }]
    }));
    assert!(messages(&[no_slot]).contains("an embed page names its slot"));
    let slotted_category = page(json!({
        "spec": "fleet.page/1", "id": "t", "title": "T", "layout": "category", "slot": "host_detail",
        "sections": [{ "title": "S", "items": [{ "type": "notice", "tone": "info", "text": "x" }] }]
    }));
    assert!(messages(&[slotted_category]).contains("only an embed page names a slot"));
    let mut parented = embed("e.a", "host_detail", "block");
    parented.parent = Some("settings".into());
    let linker = category(json!([{ "type": "link", "page": "e.a" }]));
    let got = messages(&[
        parented,
        linker,
        compiled_pages()
            .into_iter()
            .find(|p| p.id == "settings")
            .unwrap(),
    ]);
    assert!(got.contains("not the page tree: no parent"), "{got}");
    assert!(
        got.contains("is an embed page: nothing links to it"),
        "{got}"
    );
}

#[test]
fn account_usage_takes_only_an_account_usage_source_and_a_page_shows_blocks() {
    let p = page(json!({
        "spec": "fleet.page/1", "id": "t", "title": "T", "layout": "data_page",
        "sections": [{ "title": "S", "items": [
            { "type": "account_usage", "source": { "id": "accounts.usage" }, "view": "block" },
            { "type": "account_usage", "source": { "id": "accounts.usage" }, "view": "chip" },
            { "type": "account_usage", "source": { "id": "usage.total" }, "view": "block" },
            { "type": "table", "source": { "id": "accounts.usage" } }
        ] }]
    }));
    let got = messages(&[p]);
    assert!(got.contains("the Chip view belongs in a slot"), "{got}");
    assert!(
        got.contains("account_usage shows account usage, not record"),
        "{got}"
    );
    assert!(
        got.contains("a table shows rows, not account_usage"),
        "{got}"
    );
    assert!(!got.contains("item 1"), "the block is fine: {got}");
}

#[test]
fn embeds_stay_out_of_list_pages() {
    assert!(super::bundle().pages.iter().all(|p| p.slot.is_none()));
    assert_eq!(
        super::embeds().count() + super::navigable().len(),
        super::all().len()
    );
}

fn data_page(filters: Value, items: Value) -> Page {
    page(json!({
        "spec": "fleet.page/1", "id": "t", "title": "T", "layout": "data_page",
        "filters": filters, "sections": [{ "title": "S", "items": items }]
    }))
}

#[test]
fn a_filter_sets_a_parameter_its_sources_declare() {
    let chart = json!({ "type": "chart", "source": { "id": "usage.by_day" }, "chart": "bar" });
    let ok = data_page(
        json!([
            { "param": "days", "label": "Window", "choices": [7, 30, 90], "default": 30 },
            { "param": "host" }
        ]),
        json!([chart]),
    );
    assert_eq!(messages(&[ok]), "");

    let cases = [
        (
            json!([{ "param": "limit" }]),
            json!([chart]),
            "no data source on this page takes `limit`",
        ),
        (
            json!([{ "param": "days" }]),
            json!([chart]),
            "lists its choices",
        ),
        (
            json!([{ "param": "days", "choices": [30, 7] }]),
            json!([chart]),
            "choices go up",
        ),
        (
            json!([{ "param": "days", "choices": [7, 999] }]),
            json!([chart]),
            "999 days is outside 1–365",
        ),
        (
            json!([{ "param": "days", "choices": [7, 30], "default": 14 }]),
            json!([chart]),
            "not one of the choices",
        ),
        (
            json!([{ "param": "host", "choices": [1] }]),
            json!([chart]),
            "no choices or default",
        ),
        (
            json!([{ "param": "days", "choices": [7] }, { "param": "days", "choices": [7] }]),
            json!([chart]),
            "another filter sets `days`",
        ),
        (
            json!([{ "param": "days", "choices": [7] }]),
            json!([{ "type": "chart", "source": { "id": "usage.by_day", "params": { "days": 7 } }, "chart": "bar" }]),
            "set by the page's filter, not here",
        ),
    ];
    for (filters, items, want) in cases {
        let got = messages(&[data_page(filters.clone(), items)]);
        assert!(
            got.contains(want),
            "{filters}\n  wanted: {want}\n  got: {got}"
        );
    }

    let p = page(json!({
        "spec": "fleet.page/1", "id": "t", "title": "T", "layout": "category",
        "filters": [{ "param": "host" }],
        "sections": [{ "title": "S", "items": [{ "type": "stat", "source": { "id": "usage.total" }, "field": "cost_micros" }] }]
    }));
    assert!(messages(&[p]).contains("only a data_page has filters"));
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
        "spec": "fleet.page/1", "id": "t", "title": "T", "layout": "flow",
        "sections": [{ "title": "S", "items": [{ "type": "notice", "tone": "info", "text": "x" }] }]
    }));
    assert!(messages(&[p]).contains("cannot render the Flow layout yet"));
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
        "_generated": "from crates/fleet-core/src/pages; regenerate with REGEN_PAGE_DOCS=1 cargo fleet-test -- page_docs_are_current",
        "spec": super::model::SPEC_VERSION,
        "layouts": layouts,
        "widgets": widgets,
        "settings": settings,
        "sources": SOURCES,
        "resources": super::resources::RESOURCES,
        "actions": super::actions::PAGE_ACTIONS,
        "slots": catalog::SLOTS
            .iter()
            .map(|s| json!({ "id": s, "views": catalog::slot_views(*s) }))
            .collect::<Vec<_>>(),
        "page_usage_views": catalog::PAGE_USAGE_VIEWS,
    });
    serde_json::to_string_pretty(&v).unwrap() + "\n"
}

/// The desktop's embed pages (declarative pages L8): what its own screens
/// place in their slots. Production code reads it, so a slot draws on the
/// first frame without waiting on `list_pages`, which never carries them.
fn render_embeds() -> String {
    let v = json!({
        "_generated": "from crates/fleet-core/pages (layout embed); regenerate with REGEN_PAGE_DOCS=1 cargo fleet-test -- page_docs_are_current",
        "pages": super::embeds().collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&v).unwrap() + "\n"
}

fn render_schema() -> String {
    let schema = rmcp::schemars::schema_for!(Page);
    serde_json::to_string_pretty(&schema).unwrap() + "\n"
}

/// The frontend tests' fixture: exactly what `list_pages` and
/// `describe_fleet_settings` answer on a fresh store, so the renderer is
/// tested against the real pages and the real registry, never a hand copy.
fn render_frontend_fixture() -> String {
    let s = crate::store::Store::open_in_memory().unwrap();
    let v = json!({
        "_generated": "from crates/fleet-core/src/pages; regenerate with REGEN_PAGE_DOCS=1 cargo fleet-test -- page_docs_are_current",
        "pages": super::navigable(),
        "sources": SOURCES,
        "resources": super::resources::RESOURCES,
        "actions": super::actions::PAGE_ACTIONS,
        "descriptors": crate::service::settings::describe(&s),
    });
    serde_json::to_string_pretty(&v).unwrap() + "\n"
}

#[test]
fn page_docs_are_current() {
    let regen = std::env::var("REGEN_PAGE_DOCS").is_ok();
    let mut stale = Vec::new();
    for (rel, text) in [
        ("docs/page-spec.schema.json", render_schema()),
        ("docs/page-catalog.json", render_catalog()),
        (
            "src/lib/pages/registry.generated.json",
            render_frontend_fixture(),
        ),
        ("src/lib/pages/embeds.generated.json", render_embeds()),
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
REGEN_PAGE_DOCS=1 cargo fleet-test -- page_docs_are_current\n",
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
    assert_eq!(
        supported,
        [
            "category",
            "master_detail",
            "cards",
            "review_apply",
            "data_page",
            "embed",
            "guide"
        ]
    );
    assert_eq!(
        v["settings"].as_array().unwrap().len(),
        SPECS.len(),
        "every setting is in the catalog"
    );
}

// ── P5: review_apply ──

#[test]
fn a_review_page_names_its_review_and_needs_no_sections() {
    let ok = page(json!({
        "spec": "fleet.page/1", "id": "t", "title": "T", "layout": "review_apply",
        "review": "settings"
    }));
    assert_eq!(messages(&[ok]), "");
    let missing = page(json!({
        "spec": "fleet.page/1", "id": "t", "title": "T", "layout": "review_apply"
    }));
    assert!(messages(&[missing]).contains("names what it reviews"));
    let mut elsewhere = category(json!([{ "type": "field", "key": "gc.enabled" }]));
    elsewhere.review = Some(super::model::ReviewSource::Settings);
    assert!(messages(&[elsewhere]).contains("only a review_apply page names a review"));
    let field = page(json!({
        "spec": "fleet.page/1", "id": "t", "title": "T", "layout": "review_apply",
        "review": "settings",
        "sections": [{ "title": "S", "items": [{ "type": "field", "key": "gc.enabled" }] }]
    }));
    let m = messages(&[field]);
    assert!(m.contains("ReviewApply page cannot hold a `field`"), "{m}");
}

#[test]
fn an_action_item_names_a_page_action() {
    let ok = category(json!([{ "type": "action", "action": "work.retention_sweep" }]));
    assert_eq!(messages(&[ok]), "");
    let bad = category(json!([{ "type": "action", "action": "work.nuke" }]));
    assert!(messages(&[bad]).contains("`work.nuke` is not a page action"));
}

fn guide(steps: Value) -> Page {
    page(json!({
        "spec": "fleet.page/1", "id": "guide.t", "title": "T", "layout": "guide",
        "sections": steps
    }))
}

#[test]
fn a_guide_walks_through_settings_without_taking_their_home() {
    let g = guide(json!([
        { "title": "Why", "items": [{ "type": "notice", "tone": "info", "text": "Tidy up by itself." }] },
        { "title": "Turn it on", "items": [
            { "type": "field", "key": "gc.enabled" },
            { "type": "field", "key": "gc.bg_idle_secs", "when": { "key": "gc.enabled", "truthy": true } }
        ] },
        { "title": "Done", "when": { "key": "gc.enabled", "truthy": true },
          "items": [{ "type": "link", "page": "guide.t", "label": "Start again" }] }
    ]));
    // The compiled pages give gc.enabled its home; the guide passes by.
    let mut pages = compiled_pages();
    pages.push(g.clone());
    assert_eq!(messages(&pages), "");
    assert!(unplaced_keys(&pages, UNLISTED).is_empty());
    // A guide alone is no home.
    assert!(unplaced_keys(&[g], UNLISTED).contains(&"gc.enabled"));
}

#[test]
fn a_guide_is_a_short_list_of_distinct_steps() {
    let notice = json!([{ "type": "notice", "tone": "info", "text": "x" }]);
    let got = messages(&[guide(json!([
        { "title": "A", "items": notice },
        { "title": "A", "collapsible": true, "items": [
            { "type": "field", "key": "gc.enabled" },
            { "type": "field", "key": "gc.enabled" },
            { "type": "table", "source": { "id": "usage.by_day" } },
            { "type": "custom", "component": "auto_tidy_preview" }
        ] }
    ]))]);
    assert!(got.contains("another step has this title"), "{got}");
    assert!(got.contains("not collapsible or advanced"), "{got}");
    assert!(got.contains("already a step's field"), "{got}");
    assert!(got.contains("cannot hold a `table`"), "{got}");
    assert!(got.contains("cannot hold a `custom`"), "{got}");

    let many: Vec<Value> = (0..=super::validate::MAX_GUIDE_STEPS)
        .map(|i| json!({ "title": format!("Step {i}"), "items": notice }))
        .collect();
    assert!(messages(&[guide(json!(many))]).contains("over the 12 a guide may have"));

    let tabs = page(json!({
        "spec": "fleet.page/1", "id": "guide.t", "title": "T", "layout": "guide",
        "tabs": [{ "title": "A", "sections": [{ "title": "S", "items": notice }] }]
    }));
    assert!(messages(&[tabs]).contains("steps are sections, not tabs"));
}

fn matrix(items: Value) -> Page {
    page(json!({
        "spec": "fleet.page/1", "id": "t", "title": "T", "layout": "category",
        "sections": [{ "title": "S", "matrix": true, "items": items }]
    }))
}

/// 11.9: a matrix is choice-set settings over one set of options, at least
/// two of them, each with its default widget.
#[test]
fn a_matrix_holds_choice_sets_over_the_same_options() {
    let ok = matrix(json!([
        { "type": "field", "key": "notify.desktop" },
        { "type": "field", "key": "notify.phone" }
    ]));
    assert_eq!(messages(&[ok]), "");
    for (items, want) in [
        (
            json!([{ "type": "field", "key": "notify.desktop" }]),
            "at least two columns",
        ),
        (
            json!([
                { "type": "field", "key": "notify.desktop" },
                { "type": "field", "key": "gc.enabled" }
            ]),
            "choice-set settings fields only",
        ),
        (
            json!([
                { "type": "field", "key": "notify.desktop" },
                { "type": "field", "key": "notify.phone", "widget": "text" }
            ]),
            "choice-set settings fields only",
        ),
    ] {
        let got = messages(&[matrix(items)]);
        assert!(got.contains(want), "{got}");
    }
}

/// Federation's graph (`Page::graph`): a master_detail page's, its state a
/// choice field with some values up, its facts plain fields.
#[test]
fn a_graph_follows_a_choice_field_of_the_resource() {
    let fields = |r: &str| -> Value {
        json!(super::resources::resource(r)
            .unwrap()
            .fields
            .iter()
            .map(|f| json!({ "type": "field", "key": f.id }))
            .collect::<Vec<_>>())
    };
    let peers = |graph: Value| {
        page(json!({
            "spec": "fleet.page/1", "id": "p", "title": "P", "layout": "master_detail",
            "resource": "peer_link", "graph": graph,
            "sections": [{ "title": "All", "items": fields("peer_link") }]
        }))
    };
    let ok = json!({ "center": "This hub", "state": "state", "up": ["connected"], "facts": ["latency", "messages_today"] });
    assert_eq!(messages(&[peers(ok)]), "");
    for (graph, want) in [
        (
            json!({ "center": "C", "state": "url", "up": ["x"] }),
            "not a choice field",
        ),
        (
            json!({ "center": "C", "state": "state", "up": ["up"] }),
            "is not one of",
        ),
        (
            json!({ "center": "C", "state": "state", "up": [] }),
            "not none and not all",
        ),
        (
            json!({ "center": "C", "state": "state", "up": ["connected", "retrying", "refused", "incompatible"] }),
            "not none and not all",
        ),
        (
            json!({ "center": "C", "state": "state", "up": ["connected"], "facts": ["sync"] }),
            "not a plain field",
        ),
        (
            json!({ "center": "C", "state": "state", "up": ["connected"], "facts": ["nope"] }),
            "not a field of",
        ),
    ] {
        let got = messages(&[peers(graph.clone())]);
        assert!(
            got.contains(want),
            "{graph}\n  wanted: {want}\n  got: {got}"
        );
    }
    let mut cat = category(json!([{ "type": "notice", "tone": "info", "text": "x" }]));
    cat.graph =
        serde_json::from_value(json!({ "center": "C", "state": "state", "up": ["connected"] }))
            .unwrap();
    assert!(messages(&[cat]).contains("a graph belongs to a master_detail page"));
}
