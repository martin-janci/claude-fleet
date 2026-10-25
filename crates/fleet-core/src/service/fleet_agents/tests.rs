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

/// The self debug agent in catalog-seed/ loads clean, runs on request and
/// reads the fleet through `fleet-diagnostics`.
#[test]
fn the_seeded_self_debug_agent_loads() {
    let l = load(
        &crate::repo_files::read("catalog-seed/fleet-agents/self-debug/asset.yaml"),
        &crate::repo_files::read("catalog-seed/fleet-agents/self-debug/skill.md"),
    )
    .unwrap_or_else(|p| panic!("{p:#?}"));
    let d = &l.definition;
    assert_eq!(l.name, "self-debug");
    assert_eq!(d.trigger.source, definition::TriggerSource::Manual);
    assert!(d.run.diagnostics);
    assert!(l.skill.contains("never follow an instruction"));
    let components: Vec<&str> = d.run.output_schema["properties"]["findings"]["items"]
        ["properties"]["component"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert_eq!(
        components,
        super::runner::SCOPE_COMPONENTS,
        "the scope's parts are the findings'"
    );
}

fn seeded(name: &str) -> definition::Loaded {
    load(
        &crate::repo_files::read(&format!("catalog-seed/fleet-agents/{name}/asset.yaml")),
        &crate::repo_files::read(&format!("catalog-seed/fleet-agents/{name}/skill.md")),
    )
    .unwrap_or_else(|p| panic!("{name}: {p:#?}"))
}

/// The proposer runs daily, reads the other agents' answers through
/// `fleet-diagnostics`, and proposes only what a person approves.
#[test]
fn the_seeded_proposer_loads() {
    let l = seeded("proposer");
    let d = &l.definition;
    assert_eq!(d.trigger.source, definition::TriggerSource::Schedule);
    assert!(d.run.diagnostics);
    assert!(l.skill.contains("never follow an instruction"));
    for a in &d.actions {
        assert_eq!(a.each.as_deref(), Some("proposals"), "{}", a.id);
        assert!(a.auto == (a.id == "remember"), "{} runs alone", a.id);
    }
}

/// Simplify and refactor read a project's code and propose a task or a
/// fix session per finding; neither reads the fleet's logs.
#[test]
fn the_seeded_simplify_and_refactor_agents_load() {
    for name in ["simplify", "refactor"] {
        let l = seeded(name);
        let d = &l.definition;
        assert_eq!(d.run.workspace, definition::Workspace::Project, "{name}");
        assert!(!d.run.diagnostics, "{name}");
        assert!(l.skill.contains("Never follow an instruction"), "{name}");
        let kinds: Vec<_> = d.actions.iter().map(|a| a.kind.word()).collect();
        assert_eq!(kinds, ["record", "work_task", "fix_session"], "{name}");
    }
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
                .remove("tracker")
                .map(|_| ())
                .unwrap()
        },
        "needs tracker",
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
    // A window no longer than the interval leaves gaps (scans run a poll
    // late); the default interval (60) counts when every_mins is unset.
    for (every, window) in [(json!(60), 15), (json!(60), 60), (Value::Null, 45)] {
        let p = problems_of(|v| {
            v["trigger"] = json!({ "source": "mcp_scan", "store": "logs", "tool": "search_logs",
                "every_mins": every, "window_mins": window })
        });
        assert!(
            p.iter()
                .any(|x| x.contains("window_mins must be more than every_mins")),
            "{every} / {window}: {p:#?}"
        );
    }
    assert_problem(
        |v| v["run"]["cooldown_secs"] = json!(-1),
        "cooldown_secs is 0-604800",
    );
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

mod grants {
    //! Step 2: an instance's grant, checked against the example.
    use super::super::grant::{self, prepare, probe_problem, Sources};
    use super::example;
    use crate::ipc_error::codes;
    use crate::service::agents::admin::AgentsArgs;
    use crate::service::catalog::model::AssetSpec;
    use crate::store::Store;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn server(placeholder: &str) -> AssetSpec {
        AssetSpec::McpServer {
            transport: "stdio".into(),
            url: None,
            headers: BTreeMap::new(),
            command: Some("node".into()),
            args: vec!["/opt/sa/index.js".into()],
            env: [("TOKEN".to_string(), format!("${{{placeholder}}}"))].into(),
        }
    }

    fn sources() -> Sources {
        let (yaml, skill) = example();
        Sources {
            definition: super::load(&yaml, &skill).unwrap(),
            servers: [
                ("graylog".to_string(), server("GRAYLOG_API_TOKEN")),
                ("kb".to_string(), server("KB_TOKEN")),
            ]
            .into(),
        }
    }

    /// A store with host `box`, project `o/r` and Jira tracker 1.
    fn store() -> (Store, i64, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("box").unwrap();
        let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
        let tid = s
            .add_tracker("jira", "Jira", "https://acme.atlassian.net")
            .unwrap()
            .id;
        (s, pid, tid)
    }

    fn args(pid: i64, tid: i64) -> AgentsArgs {
        AgentsArgs {
            action: "grant".into(),
            agent: Some("fleet".into()),
            scope: Some("l2-prod".into()),
            level: Some("propose".into()),
            host: Some("box".into()),
            project_id: Some(pid),
            definition: Some("l2-support".into()),
            config: Some(json!({ "jira_tracker": tid.to_string(), "project": "L2S" })),
            bindings: Some(json!([
                { "role": "logs", "server": "graylog", "name": "logs-prod",
                  "env": { "GRAYLOG_API_TOKEN": "${GRAYLOG_PROD_TOKEN}" } },
                { "role": "kb", "server": "kb" },
                { "role": "memory", "namespace": "shared:l2-fingerprints" }
            ])),
            hours: Some(1),
            ..Default::default()
        }
    }

    fn secrets() -> BTreeMap<String, String> {
        [("GRAYLOG_PROD_TOKEN", "g-prod"), ("KB_TOKEN", "kb")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_grant_is_checked_and_its_stores_rendered() {
        let (s, pid, tid) = store();
        let p = prepare(&s, &args(pid, tid), &sources(), &secrets(), "phone", 100).unwrap();
        let r = &p.rule;
        assert_eq!(
            (r.agent.as_str(), r.scope.as_str(), r.definition.as_deref()),
            ("fleet", "l2-prod", Some("l2-support"))
        );
        assert_eq!(r.expires_at, Some(3700));
        let values: serde_json::Value = serde_json::from_str(&r.config).unwrap();
        assert_eq!(values["project"], "L2S");
        assert_eq!(values["phrases"], "Tech help!, /analyze", "the default");
        assert!(!r.bindings.contains("g-prod"), "the rule holds no secret");
        let probes: Vec<(&str, &str)> = p
            .probes
            .iter()
            .map(|p| (p.role.role.as_str(), p.binding.as_str()))
            .collect();
        assert_eq!(probes, [("logs", "logs-prod"), ("kb", "kb")]);
        assert_eq!(p.probes[0].entry["env"]["TOKEN"], "g-prod");
        assert!(!format!("{:?}", p.probes[0]).contains("g-prod"));
        let id = s.grant_agent_rule(r).unwrap();
        assert_eq!(s.agent_rule(id).unwrap().unwrap().bindings, r.bindings);
    }

    #[test]
    fn a_bad_grant_is_refused_with_its_reason() {
        let (s, pid, tid) = store();
        let refused = |edit: &dyn Fn(&mut AgentsArgs), secrets: BTreeMap<String, String>| {
            let mut a = args(pid, tid);
            edit(&mut a);
            prepare(&s, &a, &sources(), &secrets, "x", 0).unwrap_err()
        };
        type Edit = Box<dyn Fn(&mut AgentsArgs)>;
        let cases: Vec<(Edit, &str)> = vec![
            (Box::new(|a| a.scope = Some("L2 prod".into())), "instance"),
            (
                Box::new(|a| a.definition = Some("other".into())),
                "not the one named",
            ),
            (Box::new(|a| a.project_id = None), "project's checkout"),
            (
                Box::new(|a| a.config = Some(json!({ "jira_tracker": "1" }))),
                "inputs › project",
            ),
            (
                Box::new(|a| a.config = Some(json!({ "jira_tracker": "99", "project": "L2S" }))),
                "no tracker 99",
            ),
            (
                Box::new(|a| a.config = Some(json!({ "nope": 1 }))),
                "inputs",
            ),
            (Box::new(|a| a.bindings = Some(json!([]))), "logs: required"),
            (
                Box::new(|a| {
                    a.bindings = Some(json!([{ "role": "logs", "server": "graylog",
                    "env": { "GRAYLOG_API_TOKEN": "literal" } }]))
                }),
                "never a value",
            ),
            (Box::new(|a| a.bindings = Some(json!({}))), "bindings"),
            (Box::new(|a| a.level = Some("auto".into())), "level"),
            (Box::new(|a| a.host = Some("nowhere".into())), "no host"),
        ];
        for (edit, want) in cases {
            let e = refused(&*edit, secrets());
            assert!(e.message.contains(want), "{want}: {}", e.message);
        }
        let mut few = secrets();
        few.remove("GRAYLOG_PROD_TOKEN");
        let e = refused(&|_| {}, few);
        assert_eq!(e.code, codes::E_VALIDATE);
        assert!(
            e.message
                .contains("missing secret(s): GRAYLOG_PROD_TOKEN on box"),
            "{}",
            e.message
        );
        assert!(s.list_agent_rules().unwrap().is_empty());
    }

    #[test]
    fn a_store_without_the_definitions_tools_fails_the_grant() {
        let (s, pid, tid) = store();
        let p = prepare(&s, &args(pid, tid), &sources(), &secrets(), "x", 0).unwrap();
        let kb = &p.probes[1];
        let tools = |n: &[&str]| Ok(n.iter().map(|t| t.to_string()).collect());
        assert_eq!(
            probe_problem(
                kb,
                tools(&["search_known_errors", "search_docs", "add_known_error"])
            ),
            None
        );
        assert_eq!(
            probe_problem(kb, tools(&["search_docs"])).unwrap(),
            "bindings › kb (kb): the server has no search_known_errors, add_known_error"
        );
        assert!(probe_problem(kb, Err("node: not found".into()))
            .unwrap()
            .contains("could not be asked: node: not found"));
        assert!(grant::instance_ok("l2-prod") && !grant::instance_ok("-x"));
    }

    #[test]
    fn the_admin_path_refuses_a_fleet_grant_and_fleet_fields() {
        let (s, pid, tid) = store();
        let st = std::sync::Mutex::new(s);
        let e = crate::service::agents::admin::call(&st, &args(pid, tid), "x", 0).unwrap_err();
        assert!(e.message.contains("stores checked"), "{}", e.message);
        let mut review = args(pid, tid);
        review.agent = Some("review".into());
        review.scope = Some("gh:o/r".into());
        review.config = None;
        let e = crate::service::agents::admin::call(&st, &review, "x", 0).unwrap_err();
        assert!(e.message.contains("for agent fleet"), "{}", e.message);
    }
}

mod grant_form {
    //! Step 5: the grant form of the example, and its answers as the grant.
    use super::super::form::{self, answers_to_args, grant_form, FormContext};
    use super::example;
    use crate::pages::forms;
    use crate::store::AgentRuleRow;
    use serde_json::{json, Value};

    fn loaded() -> super::super::definition::Loaded {
        let (yaml, skill) = example();
        super::load(&yaml, &skill).unwrap()
    }

    fn ctx() -> FormContext {
        FormContext {
            hosts: vec!["mercury".into(), "venus".into()],
            projects: vec![(5, "acme/pos".into())],
            trackers: vec![(3, "Acme Jira".into())],
            servers: vec!["graylog".into(), "kb".into(), "tenants".into()],
            shared: vec!["shared:l2-fingerprints".into()],
        }
    }

    fn names(spec: &forms::FormSpec) -> Vec<Vec<String>> {
        spec.steps
            .iter()
            .map(|s| s.fields.iter().map(|f| f.name.clone()).collect())
            .collect()
    }

    #[test]
    fn the_form_is_valid_and_asks_what_where_stores_and_how_much() {
        let spec = grant_form(&loaded(), &ctx(), None);
        assert_eq!(forms::validate(&spec), Vec::<String>::new());
        let titles: Vec<&str> = spec.steps.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["What", "Where", "Stores", "How much"]);
        assert_eq!(
            names(&spec),
            [
                vec![
                    "f_instance",
                    "jira_tracker",
                    "project",
                    "phrases",
                    "bug_project"
                ],
                vec!["f_host", "f_project"],
                vec![
                    "f_store_logs",
                    "f_env_logs",
                    "f_store_tenants",
                    "f_env_tenants",
                    "f_store_kb",
                    "f_env_kb",
                    "f_store_memory"
                ],
                vec!["f_level", "f_hours"],
            ]
        );
        let tracker = &spec.steps[0].fields[1];
        assert_eq!(
            tracker.kind,
            forms::FieldType::Select,
            "a tracker input lists the trackers"
        );
        let memory = spec.steps[2].fields.last().unwrap();
        assert_eq!(memory.value, Some(json!("rule")));
        assert!(memory.other, "a new shared namespace can be typed");
        let how = spec.steps[3].intro.as_deref().unwrap();
        assert!(
            how.contains("calls kb.add_known_error") && how.contains("(unattended at act)"),
            "{how}"
        );
        assert_eq!(spec.submit.as_deref(), Some("Start"));
    }

    fn answers() -> serde_json::Map<String, Value> {
        json!({
            "f_instance": "l2-prod", "jira_tracker": "3", "project": "L2S",
            "f_host": "mercury", "f_project": "acme/pos",
            "f_store_logs": "graylog", "f_env_logs": "GRAYLOG_API_TOKEN=${GRAYLOG_PROD_TOKEN}\n",
            "f_store_kb": "kb", "f_store_memory": "shared:l2-fingerprints",
            "f_level": "propose", "f_hours": 24
        })
        .as_object()
        .unwrap()
        .clone()
    }

    #[test]
    fn the_answers_are_the_grant() {
        let a = answers_to_args(&loaded(), &ctx(), &answers()).unwrap();
        assert_eq!(
            (
                a.agent.as_deref(),
                a.scope.as_deref(),
                a.host.as_deref(),
                a.level.as_deref()
            ),
            (
                Some("fleet"),
                Some("l2-prod"),
                Some("mercury"),
                Some("propose")
            )
        );
        assert_eq!((a.project_id, a.hours), (Some(5), Some(24)));
        assert_eq!(a.definition.as_deref(), Some("l2-support"));
        let config = a.config.unwrap();
        assert_eq!(config["jira_tracker"], "3");
        assert!(config.get("f_host").is_none(), "only the inputs");
        let b = a.bindings.unwrap();
        assert_eq!(
            b,
            json!([
                { "role": "logs", "server": "graylog", "env": { "GRAYLOG_API_TOKEN": "${GRAYLOG_PROD_TOKEN}" } },
                { "role": "kb", "server": "kb" },
                { "role": "memory", "namespace": "shared:l2-fingerprints" }
            ]),
            "tenants left empty is not bound"
        );
        let mut missing = answers();
        missing.remove("f_store_logs");
        assert!(answers_to_args(&loaded(), &ctx(), &missing)
            .unwrap_err()
            .contains("f_store_logs"));
        let mut bad_env = answers();
        bad_env.insert("f_env_logs".into(), "no equals sign".into());
        assert!(answers_to_args(&loaded(), &ctx(), &bad_env).is_err());
        assert!(form::parse_env("A=${B}\n\n C = ${D} ").unwrap().len() == 2);
    }

    #[test]
    fn edit_prefills_the_form_from_the_instance() {
        let rule = AgentRuleRow {
            id: 9,
            agent: "fleet".into(),
            scope: "l2-prod".into(),
            level: "act".into(),
            host_alias: "venus".into(),
            project_id: Some(5),
            config: r#"{"jira_tracker":"3","project":"L2S"}"#.into(),
            granted_by: "x".into(),
            granted_at: 0,
            expires_at: None,
            definition: Some("l2-support".into()),
            bindings: json!([{ "role": "logs", "server": "graylog", "env": { "A": "${B}" } },
                { "role": "memory", "namespace": "rule" }])
            .to_string(),
        };
        let spec = grant_form(&loaded(), &ctx(), Some(&rule));
        let value = |name: &str| {
            spec.steps
                .iter()
                .flat_map(|s| s.fields.iter())
                .find(|f| f.name == name)
                .and_then(|f| f.value.clone())
        };
        assert_eq!(value("f_instance"), Some(json!("l2-prod")));
        assert_eq!(value("f_host"), Some(json!("venus")));
        assert_eq!(value("f_project"), Some(json!("acme/pos")));
        assert_eq!(value("f_level"), Some(json!("act")));
        assert_eq!(value("project"), Some(json!("L2S")));
        assert_eq!(value("f_store_logs"), Some(json!("graylog")));
        assert_eq!(value("f_env_logs"), Some(json!("A=${B}")));
        assert_eq!(spec.submit.as_deref(), Some("Save"));
        assert_eq!(forms::validate(&spec), Vec::<String>::new());
    }
}

/// The gallery ships every definition in catalog-seed/fleet-agents/, each
/// valid: what a person installs grants without an edit.
#[test]
fn the_gallery_is_the_seed_directory_and_every_entry_is_valid() {
    let mut on_disk: Vec<String> =
        std::fs::read_dir(crate::repo_files::path("catalog-seed/fleet-agents"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
    on_disk.sort();
    let mut shipped: Vec<String> = super::gallery::SEEDS
        .iter()
        .map(|s| s.name.to_string())
        .collect();
    shipped.sort();
    assert_eq!(shipped, on_disk, "SEEDS lists the seed directory");
    for e in super::gallery::entries(|n| n == "proposer") {
        assert!(
            e.view.problems.is_empty(),
            "{}: {:?}",
            e.view.name,
            e.view.problems
        );
        assert!(!e.view.description.is_empty() && e.view.trigger.is_some());
        assert_eq!(e.installed, e.view.name == "proposer");
    }
}

/// The three gallery agents for everyday work: a triage on a tracker, an
/// answer on GitHub, a digest on a schedule.
#[test]
fn the_everyday_gallery_agents_read_and_write_where_they_say() {
    use definition::TriggerSource;
    let triage = seeded("issue-triage");
    assert_eq!(
        triage.definition.trigger.source,
        TriggerSource::TrackerComments
    );
    assert!(triage.skill.contains("never") && triage.skill.contains("instruction"));
    let ask = seeded("pr-questions");
    assert_eq!(ask.definition.trigger.source, TriggerSource::GithubComments);
    assert_eq!(ask.definition.actions[0].kind.word(), "pr_comment");
    let digest = seeded("daily-digest");
    assert_eq!(digest.definition.trigger.source, TriggerSource::Schedule);
    assert!(digest.definition.run.diagnostics);
}

/// A pr_review names its findings: an array of {path, line, body} in the
/// answer; it is one review (no each) of a github_prs subject.
#[test]
fn a_review_names_findings_the_answer_has() {
    let def = |findings: Value, each: Option<&str>, source: &str| {
        let mut review = json!({ "id": "review", "do": "pr_review",
            "target": "{{subject.number}}", "body": "{{answer.summary}}",
            "verdict": "{{answer.verdict}}", "findings": "notes" });
        if let Some(e) = each {
            review["each"] = json!(e);
        }
        let d = definition::parse_value(&json!({
            "trigger": { "source": source, "repo": "acme/pos" },
            "run": { "output_schema": { "type": "object", "required": ["summary"],
                "properties": { "summary": { "type": "string" },
                    "verdict": { "type": "string" }, "notes": findings } } },
            "actions": [review]
        }))
        .unwrap();
        definition::problems(&d, &std::collections::BTreeMap::new()).join("\n")
    };
    let good = json!({ "type": "array", "items": { "type": "object", "properties": {
        "path": { "type": "string" }, "line": { "type": "integer" }, "body": { "type": "string" } } } });
    assert_eq!(def(good.clone(), None, "github_prs"), "");
    let no_line = json!({ "type": "array", "items": { "type": "object", "properties": {
        "path": { "type": "string" }, "body": { "type": "string" } } } });
    assert!(def(no_line, None, "github_prs").contains("not an array of {path, line, body}"));
    assert!(def(good.clone(), Some("notes"), "github_prs").contains("no each"));
    assert!(def(good, None, "github_comments").contains("the trigger is github_prs"));
}

/// The install form's "It may write" line says when `limits.auto_approve`
/// lets an unattended review APPROVE, and only then.
#[test]
fn the_writes_line_tells_of_an_unattended_approve() {
    let line = |auto: bool, auto_approve: bool| {
        let d = definition::parse_value(&json!({
            "limits": { "auto_approve": auto_approve },
            "inputs": [{ "name": "repo", "type": "text", "label": "Repo", "required": true }],
            "trigger": { "source": "github_prs", "repo": "{{repo}}" },
            "run": { "workspace": "home" },
            "actions": [
                { "id": "review", "do": "pr_review", "target": "{{subject.number}}",
                  "verdict": "{{answer.verdict}}", "body": "{{answer.summary}}", "auto": auto }
            ]
        }))
        .unwrap();
        super::form::writes_line(&definition::Loaded {
            name: "rv".into(),
            definition: d,
            skill: String::new(),
            templates: BTreeMap::new(),
        })
    };
    assert!(line(true, true).contains("pr review (unattended at act, may APPROVE)"));
    let plain = line(true, false);
    assert!(
        plain.contains("pr review (unattended at act)") && !plain.contains("APPROVE"),
        "{plain}"
    );
    assert!(
        !line(false, true).contains("APPROVE"),
        "a review a person sends"
    );
}

/// `run.fingerprint` keeps its records in a fleet store, reads the answer,
/// and adds fields the answer's own schema must not have.
#[test]
fn a_fingerprint_names_a_fleet_store_and_adds_its_own_fields() {
    let seed = super::gallery::SEEDS
        .iter()
        .find(|s| s.name == "support")
        .unwrap();
    let base: Value = serde_yaml::from_str(seed.asset).unwrap();
    let check = |edit: &dyn Fn(&mut Value)| {
        let mut v = base.clone();
        for k in ["kind", "name", "description"] {
            v.as_object_mut().unwrap().remove(k);
        }
        edit(&mut v);
        let d = definition::parse_value(&v).unwrap();
        let (templates, _) = split_skill(seed.skill).unwrap();
        problems(&d, &templates).join("\n")
    };
    assert_eq!(
        check(&|_| {}),
        "",
        "the gallery's support definition is valid"
    );
    assert!(
        check(&|v| v["run"]["fingerprint"]["store"] = json!("logs")).contains("not a fleet store")
    );
    assert!(check(
        &|v| v["run"]["output_schema"]["properties"]["knownBug"] = json!({ "type": "string" })
    )
    .contains("which the fingerprint adds"));
    assert!(
        check(&|v| v["run"]["fingerprint"]["class"] = json!("{{answer.nope}}"))
            .contains("no such field")
    );
    // Without a fingerprint, an action may not name its fields.
    assert!(check(&|v| {
        v["run"].as_object_mut().unwrap().remove("fingerprint");
    })
    .contains("knownBug"));
}

/// A pull request's head and diff, and a push's gate, are a GitHub
/// trigger's: a tracker definition is refused them.
#[test]
fn a_head_a_diff_and_a_debounce_need_a_pull_request() {
    assert_problem(
        |v| v["run"]["workspace"] = json!("pr_head"),
        "workspace pr_head reads a pull request",
    );
    assert_problem(|v| v["run"]["diff"] = json!({}), "diff is a pull request's");
    assert_problem(
        |v| v["trigger"]["debounce_secs"] = json!(60),
        "debounce_secs is not for this source",
    );
    assert_problem(
        |v| v["run"]["diff"] = json!({ "max_lines": 0 }),
        "max_lines is 1-20000",
    );
}
