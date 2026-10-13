//! Definitions: the shipped example loads, and each kind of mistake is
//! reported where it is.

use super::definition::{self, load, problems, split_skill, Definition};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn example() -> (String, String) {
    (
        crate::repo_files::read("docs/fleet-agent-examples/l2-support/asset.yaml"),
        crate::repo_files::read("docs/fleet-agent-examples/l2-support/skill.md"),
    )
}

/// The example as JSON, to bend in each test.
fn base() -> Value {
    let (yaml, _) = example();
    let v: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
    serde_json::to_value(v).unwrap()
}

fn templates() -> BTreeMap<String, String> {
    split_skill(&example().1).unwrap().0
}

/// The problems of the example changed by `edit`.
fn problems_of(edit: impl FnOnce(&mut Value)) -> Vec<String> {
    let mut v = base();
    edit(&mut v);
    match definition::parse_value(&v) {
        Ok(d) => problems(&d, &templates()),
        Err(e) => vec![e],
    }
}

fn assert_problem(edit: impl FnOnce(&mut Value), want: &str) {
    let p = problems_of(edit);
    assert!(
        p.iter().any(|x| x.contains(want)),
        "want {want:?} in {p:#?}"
    );
}

#[test]
fn the_shipped_example_loads() {
    let (yaml, skill) = example();
    let l = load(&yaml, &skill).unwrap_or_else(|p| panic!("{p:#?}"));
    assert_eq!(l.name, "l2-support");
    assert_eq!(l.definition.stores.len(), 4);
    assert_eq!(l.definition.actions.len(), 5);
    assert!(l.templates["investigation"].contains("{{answer.rootCause}}"));
    assert!(
        l.skill.starts_with("# L2 support investigation"),
        "front matter is cut off"
    );
    assert_eq!(
        l.definition.run.tools,
        ["Read", "Grep", "Glob"],
        "the default tools"
    );
}

#[test]
fn unknown_keys_and_a_wrong_kind_are_refused() {
    assert_problem(
        |v| v["trigger"]["every"] = json!(5),
        "unknown field `every`",
    );
    assert_problem(|v| v["surprise"] = json!(1), "unknown field `surprise`");
    let (yaml, skill) = example();
    let err = load(&yaml.replace("kind: fleet_agent", "kind: skill"), &skill).unwrap_err();
    assert!(err[0].contains("not fleet_agent"), "{err:?}");
    assert!(load(&"x".repeat(70 * 1024), "").unwrap_err()[0].contains("KiB"));
}

#[test]
fn inputs_follow_the_form_rules() {
    assert_problem(|v| v["inputs"][1]["name"] = json!("Project"), "inputs:");
    assert_problem(
        |v| v["inputs"][1]["name"] = json!("jira_tracker"),
        "inputs:",
    );
    assert_problem(
        |v| v["inputs"][0]["options"] = json!([["a", "A"]]),
        "fleet lists them",
    );
    assert_problem(
        |v| v["inputs"][0]["type"] = json!("secret"),
        "unknown variant `secret`",
    );
}

#[test]
fn stores_declare_their_tools() {
    assert_problem(
        |v| v["stores"][1]["role"] = json!("logs"),
        "the role appears twice",
    );
    assert_problem(
        |v| v["stores"][3]["read_tools"] = json!(["get"]),
        "a fleet store has no tools",
    );
    assert_problem(
        |v| v["stores"][0]["read_tools"] = json!([]),
        "name its read_tools or write_tools",
    );
    assert_problem(
        |v| v["stores"][2]["read_tools"] = json!(["add_known_error"]),
        "both a read and a write tool",
    );
    assert_problem(
        |v| v["stores"][0]["read_tools"] = json!(["search logs"]),
        "not a tool name",
    );
}

#[test]
fn a_trigger_takes_its_sources_keys_only() {
    assert_problem(
        |v| v["trigger"]["cron"] = json!("0 * * * *"),
        "cron is not for this source",
    );
    assert_problem(
        |v| {
            v["trigger"]
                .as_object_mut()
                .unwrap()
                .remove("query")
                .map(|_| ())
                .unwrap()
        },
        "needs query",
    );
    assert_problem(
        |v| v["trigger"]["query"] = json!("project = {{nope}}"),
        "{{inputs.nope}} names no input",
    );
    assert_problem(
        |v| v["trigger"]["query"] = json!("{{subject.key}}"),
        "there is no subject here",
    );
    assert_problem(
        |v| v["trigger"]["match"] = json!({ "assignee": "bot" }),
        "matches on any_phrase or regex",
    );
    assert_problem(
        |v| v["trigger"]["match"] = json!({ "regex": "(" }),
        "match › regex",
    );
    assert_problem(
        |v| v["trigger"] = json!({ "source": "schedule", "cron": "every day" }),
        "trigger (schedule) › cron",
    );
    let scan = problems_of(|v| {
        v["trigger"] = json!({ "source": "mcp_scan", "store": "logs", "tool": "search_docs",
            "every_mins": 1, "key_template": "fixed" })
    });
    for want in [
        "not a read_tool of store logs",
        "every_mins is 5-1440",
        "name a field of the entry",
    ] {
        assert!(scan.iter().any(|x| x.contains(want)), "{want}: {scan:#?}");
    }
    let manual = problems_of(|v| v["trigger"] = json!({ "source": "manual" }));
    assert!(
        manual.iter().any(|x| x.contains("{{subject.key}}")),
        "a manual run has no subject.key: {manual:#?}"
    );
}

#[test]
fn a_run_is_read_only_and_describes_its_answer() {
    assert_problem(
        |v| v["run"]["tools"] = json!(["Read", "Bash"]),
        "Bash is not read-only",
    );
    assert_problem(|v| v["run"]["max_turns"] = json!(0), "max_turns is 1-");
    assert_problem(
        |v| {
            v["run"]
                .as_object_mut()
                .unwrap()
                .remove("output_schema")
                .map(|_| ())
                .unwrap()
        },
        "describe the answer",
    );
    assert_problem(
        |v| v["run"]["output_schema"]["properties"]["x"] = json!({ "oneOf": [] }),
        "`oneOf`",
    );
    assert_problem(
        |v| v["run"]["context"][0]["store"] = json!("kb"),
        "not a fleet store",
    );
    assert_problem(
        |v| v["run"]["context"][0]["recent"] = json!(5),
        "give key or recent",
    );
    assert_problem(
        |v| v["run"]["context"][0]["key"] = json!("{{answer.summary}}"),
        "no answer yet here",
    );
}

#[test]
fn actions_name_what_exists_and_write_only_where_allowed() {
    assert_problem(
        |v| v["actions"][1]["id"] = json!("ask"),
        "the id appears twice",
    );
    assert_problem(
        |v| v["actions"][0]["when"] = json!({ "field": "mood", "truthy": true }),
        "mood is not a field",
    );
    assert_problem(
        |v| v["actions"][0]["body"] = json!("{{answer.mood}}"),
        "output_schema has no such field",
    );
    assert_problem(
        |v| v["actions"][1]["body_template"] = json!("nope"),
        "skill.md has no template nope",
    );
    assert_problem(|v| v["actions"][1]["body"] = json!("x"), "not both");
    assert_problem(
        |v| v["actions"][0]["store"] = json!("kb"),
        "store is not for tracker_comment",
    );
    assert_problem(
        |v| v["actions"][3]["tool"] = json!("search_docs"),
        "not a write_tool of store kb",
    );
    assert_problem(
        |v| v["actions"][3]["auto"] = json!(true),
        "auto needs the store to mark add_known_error auto_ok",
    );
    assert_problem(
        |v| v["actions"][4]["store"] = json!("logs"),
        "not a fleet store",
    );
    assert_problem(
        |v| v["actions"][2]["do"] = json!("pr_comment"),
        "needs a github trigger",
    );
    assert_problem(
        |v| v["limits"]["write_projects"] = json!([]),
        "limits.write_projects",
    );
    assert_problem(
        |v| v["limits"]["max_body_chars"] = json!(100),
        "max_body_chars is 500-30000",
    );
}

#[test]
fn an_auto_ok_write_tool_may_run_unattended() {
    let p = problems_of(|v| {
        v["stores"][2]["write_tools"] = json!([{ "name": "add_known_error", "auto_ok": true }]);
        v["actions"][3]["auto"] = json!(true);
    });
    assert!(p.is_empty(), "{p:#?}");
}

#[test]
fn the_skills_front_matter_holds_named_templates_only() {
    let (t, rest) = split_skill("---\ntemplates:\n  a: \"x {{answer.y}}\"\n---\n# Body\n").unwrap();
    assert_eq!(
        (t["a"].as_str(), rest.as_str()),
        ("x {{answer.y}}", "# Body\n")
    );
    assert_eq!(
        split_skill("# No front matter").unwrap().1,
        "# No front matter"
    );
    assert!(split_skill("---\nother: 1\n---\n").is_err());
    assert!(split_skill("---\ntemplates: {a: 1}\n---\n").is_err());
    assert!(split_skill("---\ntemplates: {}\n").is_err(), "unclosed");
    let d: Definition = definition::parse_value(&base()).unwrap();
    let mut bad = templates();
    bad.insert("spare".into(), "{{answer.nothing}}".into());
    assert!(problems(&d, &bad)
        .iter()
        .any(|x| x.contains("skill.md › template spare")));
}

#[test]
fn the_catalog_loads_a_fleet_agent_and_installs_nothing() {
    use crate::service::catalog::harness::{claude::Claude, codex::Codex, Harness};
    use crate::service::catalog::model::Kind;
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("fleet-agents/l2-support");
    std::fs::create_dir_all(&dir).unwrap();
    let (yaml, skill) = example();
    std::fs::write(
        root.path().join("catalog.yaml"),
        "schema_version: 1\nname: t\n",
    )
    .unwrap();
    std::fs::write(dir.join("asset.yaml"), &yaml).unwrap();
    std::fs::write(dir.join("skill.md"), &skill).unwrap();
    let cat = crate::service::catalog::repo::load_dir(root.path()).unwrap();
    assert!(cat.problems.is_empty(), "{:?}", cat.problems);
    let a = cat
        .assets
        .iter()
        .find(|a| a.kind() == Kind::FleetAgent)
        .expect("loaded");
    assert_eq!(a.validate(), Vec::<String>::new());
    assert!(a.body.contains("# L2 support investigation"));
    assert!(!Kind::FleetAgent.installs_on_hosts());
    for plan in [Claude.render(a).unwrap(), Codex.render(a).unwrap()] {
        assert!(
            plan.files.is_empty() && plan.merges.is_empty(),
            "nothing on a host"
        );
    }

    // A broken definition is refused at load, as any invalid asset is.
    std::fs::write(
        dir.join("asset.yaml"),
        yaml.replace("tool: add_known_error", "tool: drop_table"),
    )
    .unwrap();
    let cat = crate::service::catalog::repo::load_dir(root.path()).unwrap();
    assert!(cat.assets.iter().all(|a| a.kind() != Kind::FleetAgent));
    assert!(
        cat.problems.iter().any(|p| p.path.contains("l2-support")
            && p.message.contains("drop_table is not a write_tool")),
        "{:?}",
        cat.problems
    );
}
