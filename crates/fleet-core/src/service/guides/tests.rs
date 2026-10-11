use super::*;
use crate::store::Store;
use serde_json::json;

fn person() -> Actor<'static> {
    Actor::Person
}

fn agent() -> Actor<'static> {
    Actor::Agent("host web-1")
}

#[test]
fn the_example_checks_and_is_what_the_catalog_hands_an_author() {
    let s = Store::open_in_memory().unwrap();
    let c = check(&s, &example());
    assert!(c.ok, "{:?}", c.problems);
    let cat = authoring_catalog(&s);
    assert_eq!(cat.example, example());
    assert_eq!(cat.settings.len(), settings::SPECS.len());
    assert!(cat.items.contains(&"field"));
    assert!(!cat.items.contains(&"custom"));
    assert!(cat.pages.iter().any(|p| p["id"] == GUIDES_PAGE));
    // Values stay out: an author learns what a setting is, not what it holds.
    assert!(cat.settings.iter().all(|v| v.get("value").is_none()));
    // What an author needs to recommend a value: the registry's help, unit,
    // default, what 0 means, and the page the setting lives on.
    let behind = cat
        .settings
        .iter()
        .find(|v| v["key"] == "health.claude_max_behind")
        .unwrap();
    assert!(
        behind["help"].as_str().unwrap().contains("Patch releases"),
        "{behind}"
    );
    assert_eq!(
        (behind["default"].as_str(), behind["unit"].as_str()),
        (Some("30"), Some("count"))
    );
    assert_eq!(behind["page"], "settings.limits");
    let bg = cat
        .settings
        .iter()
        .find(|v| v["key"] == "gc.bg_idle_secs")
        .unwrap();
    assert!(bg.get("zero_means").is_some(), "{bg}");
    assert!(
        cat.settings.iter().all(|v| v.get("page").is_some()),
        "every setting has a home"
    );
}

#[test]
fn a_stored_guide_is_a_guide_under_guides_with_its_own_id() {
    let s = Store::open_in_memory().unwrap();
    let mut v = example();
    v["id"] = json!("settings.automation");
    v["parent"] = json!("settings");
    v["layout"] = json!("category");
    let c = check(&s, &v);
    let all = c.problems.join("\n");
    assert!(all.contains("\"layout\": \"guide\""), "{all}");
    assert!(all.contains("guide.<name>"), "{all}");
    assert!(all.contains("is a page of the app"), "{all}");
    assert!(all.contains("parent is \"guides\""), "{all}");

    let mut v = example();
    v["sections"][1]["items"][0]["key"] = json!("gc.nope");
    v["sections"][0]["items"][0]["text"] = json!("<script>");
    let all = check(&s, &v).problems.join("\n");
    assert!(
        all.contains("`gc.nope` is not a registered setting"),
        "{all}"
    );
    assert!(all.contains("plain"), "{all}");

    let all = check(
        &s,
        &json!({ "spec": "fleet.page/1", "id": "guide.x", "style": 1 }),
    )
    .problems
    .join("\n");
    assert!(all.contains("not a fleet.page/1 spec"), "{all}");

    let mut big = example();
    big["intro"] = json!("x".repeat(MAX_SPEC_BYTES));
    assert!(check(&s, &big).problems[0].contains("bytes"));
}

#[test]
fn an_agent_proposes_and_only_an_approved_guide_is_live() {
    let s = Store::open_in_memory().unwrap();
    let row = propose(&s, &example(), Some("people keep asking"), agent()).unwrap();
    assert_eq!(row.source, "agent");
    assert_eq!(row.source_detail.as_deref(), Some("host web-1"));
    assert!(
        live(&s).is_empty(),
        "nothing is live before a person decides"
    );
    let p = pending(&s).unwrap();
    assert_eq!((p.len(), p[0].replaces), (1, false));
    assert_eq!(keys_of(&p[0].page), ["gc.enabled", "gc.bg_idle_secs"]);

    let v = decide(&s, row.id, true, person()).unwrap();
    assert_eq!(v.guides.len(), 1);
    assert_eq!(v.guides[0].id, "guide.cleanup");
    assert!(v.proposals.is_empty());
    assert_eq!(
        s.guide_proposal(row.id)
            .unwrap()
            .unwrap()
            .decided_by
            .as_deref(),
        Some("person")
    );
    assert!(decide(&s, row.id, true, person()).is_err(), "decided once");

    // A revision waits beside the live guide, and replaces it on approval.
    let mut v2 = example();
    v2["title"] = json!("Tidy up idle sessions");
    let r2 = propose(&s, &v2, None, agent()).unwrap();
    assert!(pending(&s).unwrap()[0].replaces);
    assert_eq!(live(&s)[0].title, "Let fleet tidy up idle sessions");
    decide(&s, r2.id, true, Actor::PersonVia("client phone")).unwrap();
    let now = live(&s);
    assert_eq!(
        (now.len(), now[0].title.as_str()),
        (1, "Tidy up idle sessions")
    );
    // The view says where the live guide came from (G7.15): the approved
    // row's proposer and approver, not the superseded one's.
    let a = &view(&s, true).unwrap().approvals;
    assert_eq!(a.len(), 1);
    assert_eq!(
        (
            a[0].page_id.as_str(),
            a[0].source.as_str(),
            a[0].source_detail.as_deref(),
            a[0].approved_by.as_deref()
        ),
        (
            "guide.cleanup",
            "agent",
            Some("host web-1"),
            Some("person (client phone)")
        )
    );
    assert!(a[0].approved_at.is_some());

    // Rejecting leaves the live guide; removing takes it off.
    let r3 = propose(&s, &example(), None, agent()).unwrap();
    decide(&s, r3.id, false, person()).unwrap();
    assert_eq!(live(&s).len(), 1);
    let v = remove(&s, "guide.cleanup", person()).unwrap();
    assert!(v.guides.is_empty());
    assert!(
        v.approvals.is_empty(),
        "a removed guide has no provenance line"
    );
    assert!(remove(&s, "guide.cleanup", person()).is_err());
}

#[test]
fn a_refused_spec_is_never_stored_and_the_queue_is_bounded() {
    let s = Store::open_in_memory().unwrap();
    let mut bad = example();
    bad["sections"][1]["items"][0]["key"] = json!("gc.nope");
    let e = propose(&s, &bad, None, agent()).unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    assert!(e.message.contains("gc.nope"), "{}", e.message);
    assert!(pending(&s).unwrap().is_empty());

    for i in 0..MAX_PENDING {
        let mut v = example();
        v["id"] = json!(format!("guide.g{i}"));
        propose(&s, &v, None, agent()).unwrap();
    }
    let e = propose(&s, &example(), None, agent()).unwrap_err();
    assert_eq!(e.code, codes::E_RATE_LIMITED);
    // A revision of one already waiting replaces it, so it still fits.
    let mut v = example();
    v["id"] = json!("guide.g0");
    propose(&s, &v, None, agent()).unwrap();
}

#[test]
fn a_guide_may_link_to_another_live_guide_but_not_to_a_missing_one() {
    let s = Store::open_in_memory().unwrap();
    let a = propose(&s, &example(), None, agent()).unwrap();
    let mut b = example();
    b["id"] = json!("guide.next");
    b["sections"][2]["items"][1]["page"] = json!("guide.cleanup");
    assert!(!check(&s, &b).ok, "guide.cleanup is not live yet");
    decide(&s, a.id, true, person()).unwrap();
    assert!(check(&s, &b).ok, "{:?}", check(&s, &b).problems);
}

#[test]
fn a_live_guide_that_no_longer_checks_is_left_out() {
    let s = Store::open_in_memory().unwrap();
    let row = propose(&s, &example(), None, agent()).unwrap();
    decide(&s, row.id, true, person()).unwrap();
    // As if a later build removed the setting it names.
    let mut stale = example();
    stale["sections"][1]["items"][0]["key"] = json!("gc.removed_in_a_later_build");
    s.set_guide_spec_for_tests(row.id, &stale.to_string());
    assert!(live(&s).is_empty());
}

// ── the `fleet-guides` skill in catalog-seed/ ──

fn seed_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../catalog-seed")
}

/// The skill ships as a catalog asset: the catalog's own reader loads it,
/// its lint finds nothing, and sync renders it as a Claude skill that may
/// call the `guide` tool.
#[test]
fn the_fleet_guides_skill_is_a_clean_catalog_asset() {
    use crate::service::catalog::{author, harness::Harness, model::Kind, repo};
    let root = seed_root();
    let cat = repo::load_dir(&root).expect("catalog-seed loads");
    assert!(cat.problems.is_empty(), "{:?}", cat.problems);
    let lint = author::lint_all(&cat, &root);
    assert_eq!((lint.errors, lint.warnings), (0, 0), "{:?}", lint.assets);
    let skill = cat.find(Kind::Skill, "fleet-guides").expect("the skill");
    let plan = crate::service::catalog::harness::claude::Claude
        .render(skill)
        .expect("renders for Claude");
    let md = plan
        .files
        .iter()
        .find(|f| f.path.ends_with("skills/fleet-guides/SKILL.md"))
        .expect("SKILL.md");
    let text = String::from_utf8(md.bytes.clone()).unwrap();
    assert!(text.starts_with("---\nname: fleet-guides\n"), "{text}");
    assert!(
        text.contains("allowed-tools: mcp__claude-fleet__guide"),
        "{text}"
    );
}

/// The skill's worked example is the one `guide { catalog }` hands out, and
/// it checks; the actions it names are the tool's.
#[test]
fn the_skill_teaches_the_tool_as_it_is() {
    let body = std::fs::read_to_string(seed_root().join("skills/fleet-guides/body.md")).unwrap();
    let start = body.find("```json\n").expect("a JSON example") + "```json\n".len();
    let end = start + body[start..].find("```").unwrap();
    let shown: serde_json::Value = serde_json::from_str(&body[start..end]).expect("valid JSON");
    assert_eq!(
        shown,
        example(),
        "the skill's example drifted from service::guides::example"
    );
    for action in ["catalog", "validate", "propose", "list"] {
        assert!(
            body.contains(&format!("\"action\": \"{action}\"")),
            "the skill shows `{action}`"
        );
    }
    for rule in [
        format!("{}", validate::MAX_GUIDE_STEPS),
        format!("≤{}", validate::MAX_HINT),
        format!("≤{}", validate::MAX_TEXT),
        format!("{MAX_PENDING} guides already wait"),
        format!("{} KiB", MAX_SPEC_BYTES / 1024),
    ] {
        assert!(body.contains(&rule), "the skill states `{rule}`");
    }
}
