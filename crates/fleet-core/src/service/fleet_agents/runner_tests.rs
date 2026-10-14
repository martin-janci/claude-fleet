//! The generic runner against a fake world: the example `l2-support`
//! definition on a tracker, and small definitions for the other sources.

use super::definition::{self, Loaded};
use super::grant::Sources;
use super::records;
use super::runner::*;
use crate::ipc_error::IpcError;
use crate::service::agents::support::SupportHost;
use crate::service::agents::vehicle::{PrintOutcome, PrintRun};
use crate::service::agents::workspace::{self, Prepared};
use crate::service::agents::{AgentPoller, AgentRunner, ErrorKind, RunOutcome};
use crate::service::catalog::model::AssetSpec;
use crate::service::trackers::{IssueComment, IssueThread, WriteOp, WriteOutcome};
use crate::store::{AgentCursorRow, AgentRuleRow, NewAgentRecord, NewAgentRule, Store};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Base {
    threads: Mutex<Vec<IssueThread>>,
    jql: Mutex<Vec<String>>,
    answer: Mutex<Option<PrintOutcome>>,
    last: Mutex<Option<(PrintRun, String)>>,
    log: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl SupportHost for Base {
    async fn threads(&self, tracker: i64, jql: &str, _: usize) -> Result<Vec<IssueThread>, String> {
        self.jql.lock().unwrap().push(format!("{tracker} {jql}"));
        Ok(self.threads.lock().unwrap().clone())
    }
    async fn home(&self, _: &str) -> Result<String, String> {
        Ok("/home/u".into())
    }
    async fn read_file(&self, _: &str, _: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    async fn checkout(&self, _: &str, _: i64) -> Result<String, IpcError> {
        Ok("/p/o/r".into())
    }
    async fn prepare(
        &self,
        _: &str,
        root: &str,
        job_id: i64,
        branch: &str,
    ) -> Result<(Prepared, String), IpcError> {
        self.log.lock().unwrap().push(format!("prepare {branch}"));
        Ok((Prepared::Ready, workspace::workspace_dir(root, job_id)))
    }
    async fn run(&self, _: &str, run: &PrintRun, prompt: &str) -> Result<PrintOutcome, IpcError> {
        // What the run's token opens while it runs.
        let open = run
            .mcp_config
            .as_deref()
            .and_then(token_in)
            .map(|t| records::grant_for(&t, crate::store::now_unix()).is_some());
        self.log
            .lock()
            .unwrap()
            .push(format!("run token_open={open:?}"));
        *self.last.lock().unwrap() = Some((run.clone(), prompt.to_string()));
        Ok(self.answer.lock().unwrap().clone().expect("an answer"))
    }
    async fn cleanup(&self, _: &str, _: &str, job_id: i64) {
        self.log.lock().unwrap().push(format!("cleanup {job_id}"));
    }
    async fn prune(&self, _: &str, _: &str) {}
    async fn write(&self, _: i64, _: &WriteOp) -> Result<WriteOutcome, String> {
        Err("no writes in step 3".into())
    }
    async fn run_script(&self, _: &str, _: &str, _: u64) -> Result<String, String> {
        Err("no scripts".into())
    }
}

/// The fleet-records token in a run's MCP config.
fn token_in(config: &str) -> Option<String> {
    let v: Value = serde_json::from_str(config).ok()?;
    v["mcpServers"]["fleet-records"]["headers"]["Authorization"]
        .as_str()?
        .strip_prefix("Bearer ")
        .map(str::to_string)
}

struct World {
    sources: Sources,
    base: Base,
    calls: Mutex<Vec<(Value, String, Value)>>,
    call_answer: Mutex<String>,
}

#[async_trait::async_trait]
impl FleetHost for World {
    fn sources(&self, _: &AgentRuleRow) -> Result<Sources, String> {
        Ok(self.sources.clone())
    }
    fn secrets(&self, _: &str) -> Result<BTreeMap<String, String>, String> {
        Ok([("GRAYLOG_PROD_TOKEN", "g-prod"), ("KB_TOKEN", "kb-tok")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect())
    }
    fn hub_url(&self) -> Result<String, String> {
        Ok("http://127.0.0.1:4180".into())
    }
    fn tracker_site(&self, _: i64) -> Option<String> {
        Some("https://acme.atlassian.net".into())
    }
    async fn call_tool(
        &self,
        _: &str,
        entry: &Value,
        tool: &str,
        args: &Value,
    ) -> Result<String, String> {
        self.calls
            .lock()
            .unwrap()
            .push((entry.clone(), tool.to_string(), args.clone()));
        Ok(self.call_answer.lock().unwrap().clone())
    }
    fn base(&self) -> &dyn SupportHost {
        &self.base
    }
}

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

fn l2() -> Loaded {
    definition::load(
        &crate::repo_files::read("docs/fleet-agent-examples/l2-support/asset.yaml"),
        &crate::repo_files::read("docs/fleet-agent-examples/l2-support/skill.md"),
    )
    .unwrap()
}

/// A small definition with `trigger` and a `memory` fleet store.
fn small(trigger: Value, stores: Value) -> Loaded {
    let d = definition::parse_value(&json!({
        "stores": stores,
        "trigger": trigger,
        "run": { "workspace": "home", "output_schema": { "type": "object",
            "required": ["summary"], "properties": { "summary": { "type": "string" } } } }
    }))
    .unwrap();
    Loaded {
        name: "small".into(),
        definition: d,
        skill: "Be brief.".into(),
        templates: BTreeMap::new(),
    }
}

struct Fx {
    store: Arc<Mutex<Store>>,
    world: Arc<World>,
    rule: AgentRuleRow,
}

fn fx(loaded: Loaded, config: Value, bindings: Value, project: bool) -> Fx {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("box").unwrap();
    let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
    let id = s
        .grant_agent_rule(&NewAgentRule {
            agent: "fleet".into(),
            scope: "inst".into(),
            level: "watch".into(),
            host_alias: "box".into(),
            project_id: project.then_some(pid),
            config: config.to_string(),
            granted_by: "t".into(),
            granted_at: 0,
            expires_at: None,
            definition: Some(loaded.name.clone()),
            bindings: bindings.to_string(),
        })
        .unwrap();
    let rule = s.agent_rule(id).unwrap().unwrap();
    let world = Arc::new(World {
        sources: Sources {
            definition: loaded,
            servers: [
                ("graylog".to_string(), server("GRAYLOG_API_TOKEN")),
                ("kb".to_string(), server("KB_TOKEN")),
            ]
            .into(),
        },
        base: Base::default(),
        calls: Mutex::default(),
        call_answer: Mutex::default(),
    });
    Fx {
        store: Arc::new(Mutex::new(s)),
        world,
        rule,
    }
}

impl Fx {
    fn poller(&self) -> FleetPoller {
        FleetPoller {
            store: Arc::clone(&self.store),
            host: self.world.clone(),
        }
    }
    fn runner(&self) -> FleetRunner {
        FleetRunner {
            store: Arc::clone(&self.store),
            host: self.world.clone(),
        }
    }
    fn cursor(&self, since: i64, scan: Option<i64>) -> AgentCursorRow {
        AgentCursorRow {
            rule_id: self.rule.id,
            polled_at: since,
            cursor: Some(Cursor { since, scan }.text()),
        }
    }
    /// Record `jobs` and claim them back as rows.
    fn claim(&self, jobs: &[crate::store::NewAgentJob]) -> Vec<crate::store::AgentJobRow> {
        let s = self.store.lock().unwrap();
        s.record_agent_poll(self.rule.id, 0, None, jobs).unwrap();
        s.claim_agent_jobs(&[AGENT], i64::MAX / 4, 10, 60).unwrap()
    }
}

fn l2_fx() -> Fx {
    fx(
        l2(),
        json!({ "jira_tracker": "3", "project": "L2S", "phrases": "Tech help!, /analyze" }),
        json!([
            { "role": "logs", "server": "graylog", "name": "logs-prod",
              "env": { "GRAYLOG_API_TOKEN": "${GRAYLOG_PROD_TOKEN}" } },
            { "role": "kb", "server": "kb" },
            { "role": "memory", "namespace": "rule" }
        ]),
        true,
    )
}

fn thread(comments: &[(&str, &str, i64)]) -> IssueThread {
    IssueThread {
        key: "L2S-7".into(),
        summary: "Card payments declined".into(),
        description: "Since 9:00 every card payment fails.".into(),
        assignee_id: Some("acc-1".into()),
        status: "Open".into(),
        updated: Some(1000),
        comments: comments
            .iter()
            .map(|(id, body, at)| IssueComment {
                id: id.to_string(),
                author_id: Some("u".into()),
                author: "Eva".into(),
                body: body.to_string(),
                created: Some(*at),
            })
            .collect(),
    }
}

#[tokio::test]
async fn the_first_poll_sets_the_clock_and_a_comment_with_a_phrase_is_a_job() {
    let f = l2_fx();
    let p = f.poller();
    let first = p.poll(&f.rule, None, 1000).await.unwrap();
    assert!(first.jobs.is_empty());
    assert_eq!(
        first.cursor,
        Some(
            Cursor {
                since: 1000,
                scan: None
            }
            .text()
        )
    );
    *f.world.base.threads.lock().unwrap() = vec![thread(&[
        ("1", "tech HELP! please", 1500),
        ("2", "nothing to see", 1600),
        ("3", "/analyze", 100),
    ])];
    let polled = p
        .poll(&f.rule, Some(&f.cursor(1000, None)), 2000)
        .await
        .unwrap();
    let keys: Vec<&str> = polled.jobs.iter().map(|j| j.dedup_key.as_str()).collect();
    assert_eq!(keys, [format!("r{}:tracker:3:L2S-7#c1", f.rule.id)]);
    assert_eq!(polled.jobs[0].subject, "tracker:3:L2S-7");
    let jql = f.world.base.jql.lock().unwrap()[0].clone();
    assert_eq!(
        jql,
        "3 (project = \"L2S\") AND updated >= -19m ORDER BY updated DESC"
    );
    assert_eq!(
        Cursor::read(Some(&AgentCursorRow {
            rule_id: 1,
            polled_at: 0,
            cursor: polled.cursor
        }))
        .unwrap()
        .since,
        2000
    );
}

#[test]
fn an_assignment_matches_its_account_and_a_regex_its_line() {
    let f = fx(
        small(
            json!({ "source": "tracker_assigned", "tracker": "3", "query": "project = X",
                "match": { "assignee": "{{who}}" } }),
            json!([]),
        ),
        json!({ "who": "acc-1" }),
        json!([]),
        false,
    );
    let inst = instance(&f.rule, f.world.sources.clone()).unwrap();
    let ctx = inst.ctx(0);
    let jobs = plan_tracker_jobs(f.rule.id, &inst, &ctx, 3, &[thread(&[])], 0, 10).unwrap();
    assert_eq!(
        jobs[0].dedup_key,
        format!("r{}:tracker:3:L2S-7#assigned:acc-1", f.rule.id)
    );
    let re = regex::Regex::new(r"^/ask\b").unwrap();
    assert!(comment_matches("hi\n/ask now", &[], Some(&re)));
    assert!(!comment_matches("please /ask", &[], Some(&re)));
    assert_eq!(parse_tracker_subject("tracker:3:L2S-7"), Some((3, "L2S-7")));
    assert_eq!(parse_tracker_subject("tracker:3:L2S\"x"), None);
}

#[tokio::test]
async fn a_scan_turns_each_new_entry_into_one_job() {
    let f = fx(
        small(
            json!({ "source": "mcp_scan", "store": "logs", "tool": "search_logs",
                "arguments": { "query": "{{q}}" }, "every_mins": 30,
                "key_template": "{{subject.entry.id}}" }),
            json!([{ "role": "logs", "kind": "mcp", "label": "Logs", "read_tools": ["search_logs"] }]),
        ),
        json!({ "q": "level:ERROR" }),
        json!([{ "role": "logs", "server": "graylog",
            "env": { "GRAYLOG_API_TOKEN": "${GRAYLOG_PROD_TOKEN}" } }]),
        false,
    );
    let text = json!({ "entries": [ { "id": "a", "msg": "boom" }, { "id": "b" }, { "id": "a" }, { "x": 1 } ] }).to_string();
    *f.world.call_answer.lock().unwrap() = json!({ "jsonrpc": "2.0", "id": 2,
        "result": { "content": [{ "type": "text", "text": text }] } })
    .to_string();
    let p = f.poller();
    let polled = p
        .poll(&f.rule, Some(&f.cursor(0, None)), 10_000)
        .await
        .unwrap();
    let subjects: Vec<&str> = polled.jobs.iter().map(|j| j.subject.as_str()).collect();
    assert_eq!(
        subjects,
        ["scan:a", "scan:b"],
        "one per key; an entry with no key is skipped"
    );
    let (entry, tool, args) = f.world.calls.lock().unwrap()[0].clone();
    assert_eq!(
        (tool.as_str(), args),
        ("search_logs", json!({ "query": "level:ERROR" }))
    );
    assert_eq!(entry["env"]["TOKEN"], "g-prod", "the binding's own secret");
    // Within every_mins: no scan; after it, the same entries are not new.
    let c: Cursor = serde_json::from_str(polled.cursor.as_deref().unwrap()).unwrap();
    let again = p
        .poll(&f.rule, Some(&f.cursor(10_000, c.scan)), 10_000 + 60)
        .await
        .unwrap();
    assert!(again.jobs.is_empty());
    assert_eq!(f.world.calls.lock().unwrap().len(), 1);
    let later = p
        .poll(&f.rule, Some(&f.cursor(10_060, c.scan)), 10_000 + 1800)
        .await
        .unwrap();
    assert!(later.jobs.is_empty(), "seen before");
    assert_eq!(f.world.calls.lock().unwrap().len(), 2);
    assert!(scan_entries("[1,2]").is_ok() && scan_entries("3").is_err());
}

#[test]
fn a_schedule_fires_once_for_its_newest_missed_slot() {
    // Every hour on the hour.
    assert_eq!(schedule_fire("0 * * * *", 0, 3599).unwrap(), None);
    assert_eq!(schedule_fire("0 * * * *", 0, 3600).unwrap(), Some(3600));
    assert_eq!(
        schedule_fire("0 * * * *", 0, 4 * 3600 + 5).unwrap(),
        Some(4 * 3600)
    );
    assert!(schedule_fire("not cron", 0, 1).is_err());
}

const GOOD: &str = r#"Here it is: {"summary":"Gateway key expired","rootCause":"The key","confidence":"high","severity":"critical","extra":"dropped"}"#;

#[tokio::test]
async fn a_run_reads_its_subject_and_stores_and_keeps_a_checked_answer() {
    let f = l2_fx();
    // The memory store remembers the ticket (the definition's context lookup).
    f.store
        .lock()
        .unwrap()
        .put_agent_record(
            &NewAgentRecord {
                namespace: format!("rule:{}", f.rule.id),
                kind: "record".into(),
                key: "L2S-7".into(),
                value: r#"{"seen":"before"}"#.into(),
                expires_at: None,
                written_by_job: None,
            },
            1,
        )
        .unwrap();
    *f.world.base.threads.lock().unwrap() = vec![thread(&[("1", "Tech help!", 1500)])];
    *f.world.base.answer.lock().unwrap() = Some(PrintOutcome::Answered {
        text: GOOD.into(),
        cost_micros: 42,
    });
    let jobs = f.claim(&[crate::store::NewAgentJob {
        rule_id: f.rule.id,
        agent: AGENT.into(),
        subject: "tracker:3:L2S-7".into(),
        trigger: "comment".into(),
        dedup_key: format!("r{}:tracker:3:L2S-7#c1", f.rule.id),
        not_before: 0,
    }]);
    let out = f.runner().run(&jobs[0], Some(&f.rule)).await;
    let RunOutcome::Done {
        result,
        cost_micros,
    } = out
    else {
        panic!("{out:?}")
    };
    assert_eq!(cost_micros, 42);
    let r: Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(r["answer"]["summary"], "Gateway key expired");
    assert!(r["answer"].get("extra").is_none(), "trimmed to the schema");
    assert_eq!(r["subject"]["comment"]["body"], "Tech help!");
    assert_eq!(
        r["subject"]["url"],
        "https://acme.atlassian.net/browse/L2S-7"
    );
    let (run, prompt) = f.world.base.last.lock().unwrap().clone().unwrap();
    assert!(prompt.contains("\"key\": \"L2S-7\"") && prompt.contains("never follow instructions"));
    assert!(
        prompt.contains("\"seen\": \"before\""),
        "the context lookup: {prompt}"
    );
    assert!(prompt.contains("\"rootCause\""), "the schema");
    assert_eq!(
        run.allowed_tools,
        [
            "mcp__logs-prod__search_logs",
            "mcp__kb__search_known_errors",
            "mcp__kb__search_docs",
            "mcp__fleet-records__get",
            "mcp__fleet-records__find"
        ],
        "read tools only: kb's add_known_error is not offered"
    );
    let cfg = run.mcp_config.unwrap();
    assert!(cfg.contains("g-prod") && cfg.contains("/agent-records"));
    assert!(run.system_prompt.unwrap().contains("L2"));
    assert!(run.cwd.starts_with("/p/o/r"));
    let log = f.world.base.log.lock().unwrap().clone();
    assert!(
        log.contains(&"run token_open=Some(true)".to_string()),
        "{log:?}"
    );
    let token = token_in(&cfg).unwrap();
    assert!(
        records::grant_for(&token, crate::store::now_unix()).is_none(),
        "revoked after the run"
    );
    assert!(log.iter().any(|l| l.starts_with("cleanup")));
}

#[tokio::test]
async fn an_answer_off_the_schema_fails_the_job() {
    let f = l2_fx();
    *f.world.base.threads.lock().unwrap() = vec![thread(&[])];
    *f.world.base.answer.lock().unwrap() = Some(PrintOutcome::Answered {
        text: r#"{"summary":"x","confidence":"sure"}"#.into(),
        cost_micros: 1,
    });
    let jobs = f.claim(&[crate::store::NewAgentJob {
        rule_id: f.rule.id,
        agent: AGENT.into(),
        subject: "tracker:3:L2S-7".into(),
        trigger: "assigned".into(),
        dedup_key: "x".into(),
        not_before: 0,
    }]);
    match f.runner().run(&jobs[0], Some(&f.rule)).await {
        RunOutcome::Failed {
            kind,
            message,
            cost_micros,
        } => {
            assert_eq!((kind, cost_micros), (ErrorKind::Permanent, 1));
            assert!(
                message.contains("does not match output_schema"),
                "{message}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(parse_answer("no json here", &json!({"type":"object"}))
        .unwrap_err()
        .contains("no JSON object"));
    assert!(matches!(
        f.runner().run(&jobs[0], None).await,
        RunOutcome::Skipped { .. }
    ));
}

#[tokio::test]
async fn a_person_runs_a_manual_agent_with_a_request() {
    let f = fx(
        small(json!({ "source": "manual" }), json!([])),
        json!({}),
        json!([]),
        false,
    );
    let src = super::definition::TriggerSource::Manual;
    // The subject is kept for its TTL from now; the run reads it at now.
    let now = crate::store::now_unix();
    queue_manual(
        &f.store,
        &f.rule,
        src,
        Some("Summarise today's incidents"),
        now,
    )
    .unwrap();
    let jobs = f
        .store
        .lock()
        .unwrap()
        .claim_agent_jobs(&[AGENT], now, 10, 60)
        .unwrap();
    assert_eq!(jobs.len(), 1);
    *f.world.base.answer.lock().unwrap() = Some(PrintOutcome::Answered {
        text: r#"{"summary":"ok"}"#.into(),
        cost_micros: 0,
    });
    let out = f.runner().run(&jobs[0], Some(&f.rule)).await;
    let RunOutcome::Done { result, .. } = out else {
        panic!("{out:?}")
    };
    let r: Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(r["subject"]["request"], "Summarise today's incidents");
    let (run, prompt) = f.world.base.last.lock().unwrap().clone().unwrap();
    assert_eq!(run.cwd, "/home/u");
    assert!(
        run.mcp_config.is_none() && run.allowed_tools.is_empty(),
        "no stores bound"
    );
    assert!(prompt.contains("Summarise today's incidents"));
    // A tracker agent finds its own work.
    let tracker = super::definition::TriggerSource::TrackerComments;
    assert!(queue_manual(&f.store, &f.rule, tracker, None, 100).is_err());
    assert!(queue_manual(&f.store, &f.rule, src, Some(&"x".repeat(4001)), 100).is_err());
}
