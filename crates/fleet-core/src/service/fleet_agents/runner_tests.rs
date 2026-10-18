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
    /// Writes sent, in order.
    writes: Mutex<Vec<(i64, WriteOp)>>,
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
    async fn write(&self, tracker: i64, op: &WriteOp) -> Result<WriteOutcome, String> {
        self.writes.lock().unwrap().push((tracker, op.clone()));
        Ok(WriteOutcome {
            remote_id: Some("BUG-9".into()),
            url: None,
            key: None,
        })
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
    /// What tools/list answers.
    tools: Mutex<Vec<Value>>,
    /// The tracker's provider (`jira_cloud` unless set).
    provider: Mutex<Option<String>>,
    /// GitHub: the open pull requests and the comments a poll reads.
    prs: Mutex<Vec<crate::service::agents::review::plan::OpenPr>>,
    comments: Mutex<Vec<crate::service::agents::review::PrComment>>,
    /// GitHub writes, in order.
    gh_writes: Mutex<Vec<String>>,
    /// Fleet-own actions carried out, in order.
    fleet_ops: Mutex<Vec<super::actions::FleetOp>>,
    /// A tracker that is not Jira: its synced items, and the windows asked.
    items: Mutex<Vec<ItemThread>>,
    item_since: Mutex<Vec<i64>>,
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
    async fn list_tools(&self, _: &str, _: &Value) -> Result<Vec<Value>, String> {
        Ok(self.tools.lock().unwrap().clone())
    }
    fn tracker_provider(&self, _: i64) -> Option<String> {
        Some(
            self.provider
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| "jira_cloud".into()),
        )
    }
    async fn item_threads(
        &self,
        _: i64,
        since: i64,
        max: usize,
    ) -> Result<Vec<ItemThread>, String> {
        self.item_since.lock().unwrap().push(since);
        Ok(self
            .items
            .lock()
            .unwrap()
            .iter()
            .filter(|i| i.thread.updated.is_some_and(|u| u >= since))
            .take(max)
            .cloned()
            .collect())
    }
    async fn open_prs(
        &self,
        _: &str,
        repo: &str,
    ) -> Result<Vec<crate::service::agents::review::plan::OpenPr>, String> {
        self.gh_writes.lock().unwrap().push(format!("list {repo}"));
        Ok(self.prs.lock().unwrap().clone())
    }
    async fn pr_comments_since(
        &self,
        _: &str,
        repo: &str,
        since: &str,
    ) -> Result<Vec<crate::service::agents::review::PrComment>, String> {
        self.gh_writes
            .lock()
            .unwrap()
            .push(format!("comments {repo} since {since}"));
        Ok(self.comments.lock().unwrap().clone())
    }
    async fn pr_comment(&self, _: &str, repo: &str, number: i64, body: &str) -> Result<(), String> {
        self.gh_writes
            .lock()
            .unwrap()
            .push(format!("comment {repo}#{number}: {body}"));
        Ok(())
    }
    async fn set_pr_labels(
        &self,
        req: &crate::service::agents::review::labels::LabelsRequest,
    ) -> Result<(), String> {
        self.gh_writes.lock().unwrap().push(format!(
            "labels {}#{} +{:?} -{:?}",
            req.repo, req.number, req.add, req.remove
        ));
        Ok(())
    }
    fn base(&self) -> &dyn SupportHost {
        &self.base
    }
    async fn fleet_op(&self, op: super::actions::FleetOp) -> Result<String, String> {
        self.fleet_ops.lock().unwrap().push(op);
        Ok("done".into())
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
        tools: Mutex::default(),
        provider: Mutex::default(),
        prs: Mutex::default(),
        comments: Mutex::default(),
        gh_writes: Mutex::default(),
        fleet_ops: Mutex::default(),
        items: Mutex::default(),
        item_since: Mutex::default(),
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
        Some(&json!({ "minutes": 90, "host": "htz", "component": "hub", "session": "" })),
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
    assert_eq!(
        r["subject"]["scope"],
        json!({ "minutes": 90, "host": "htz", "component": "hub" }),
        "an empty session is dropped"
    );
    let (run, prompt) = f.world.base.last.lock().unwrap().clone().unwrap();
    assert_eq!(run.cwd, "/home/u");
    assert!(
        run.mcp_config.is_none() && run.allowed_tools.is_empty(),
        "no stores bound"
    );
    assert!(prompt.contains("Summarise today's incidents"));
    // A tracker agent finds its own work.
    let tracker = super::definition::TriggerSource::TrackerComments;
    assert!(queue_manual(&f.store, &f.rule, tracker, None, None, 100).is_err());
    assert!(queue_manual(&f.store, &f.rule, src, Some(&"x".repeat(4001)), None, 100).is_err());
    for bad in [
        json!({ "minutes": 0 }),
        json!({ "minutes": 60 * 24 * 8 }),
        json!({ "component": "kernel" }),
        json!({ "host": 3 }),
        json!({ "where": "htz" }),
        json!("htz"),
    ] {
        assert!(
            queue_manual(&f.store, &f.rule, src, None, Some(&bad), 100).is_err(),
            "{bad}"
        );
    }
}

#[tokio::test]
async fn a_diagnostics_run_reads_the_fleet_through_its_own_token() {
    let mut loaded = small(json!({ "source": "manual" }), json!([]));
    loaded.definition.run.diagnostics = true;
    let f = fx(loaded, json!({}), json!([]), false);
    let now = crate::store::now_unix();
    queue_manual(
        &f.store,
        &f.rule,
        super::definition::TriggerSource::Manual,
        Some("Why did Control get stuck?"),
        None,
        now,
    )
    .unwrap();
    let jobs = f
        .store
        .lock()
        .unwrap()
        .claim_agent_jobs(&[AGENT], now, 10, 60)
        .unwrap();
    *f.world.base.answer.lock().unwrap() = Some(PrintOutcome::Answered {
        text: r#"{"summary":"ok"}"#.into(),
        cost_micros: 0,
    });
    let out = f.runner().run(&jobs[0], Some(&f.rule)).await;
    assert!(matches!(out, RunOutcome::Done { .. }), "{out:?}");
    let (run, prompt) = f.world.base.last.lock().unwrap().clone().unwrap();
    let cfg = run.mcp_config.unwrap();
    assert!(
        cfg.contains("http://127.0.0.1:4180/agent-diagnostics"),
        "{cfg}"
    );
    assert!(!cfg.contains("/agent-records"), "no fleet store bound");
    assert!(run
        .allowed_tools
        .contains(&"mcp__fleet-diagnostics__hub_log".to_string()));
    assert!(prompt.contains("mcp__fleet-diagnostics__timeline"));
    // The token ended with the run.
    let token = cfg
        .split("Bearer ")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_string();
    assert_eq!(super::diagnostics::grant_for(&token, now), None);
}

/// Run now on a scheduled agent (`/propose <focus>`) carries the focus; its
/// own ticks carry none.
#[test]
fn run_now_on_a_scheduled_agent_keeps_its_focus() {
    let src = super::definition::TriggerSource::Schedule;
    let now = crate::store::now_unix();
    for request in [Some("the hub"), None] {
        let f = fx(
            small(
                json!({ "source": "schedule", "cron": "0 7 * * *" }),
                json!([]),
            ),
            json!({}),
            json!([]),
            false,
        );
        queue_manual(&f.store, &f.rule, src, request, None, now).unwrap();
        let jobs = f
            .store
            .lock()
            .unwrap()
            .claim_agent_jobs(&[AGENT], now, 10, 60)
            .unwrap();
        let job = jobs
            .first()
            .unwrap_or_else(|| panic!("{request:?}: no job"));
        let subject = super::runner::kept_subject(&f.store, job, f.rule.id, now).unwrap();
        assert_eq!(subject["at"], now);
        assert_eq!(subject.get("request").and_then(Value::as_str), request);
    }
}

// ── step 4: actions ──

use super::actions::{self, FleetExecutor};
use crate::service::agents::{ActionResult, AgentExecutor};

impl Fx {
    /// The same rule at `level`.
    fn at(mut self, level: &str) -> Fx {
        {
            let s = self.store.lock().unwrap();
            let r = &self.rule;
            s.grant_agent_rule(&NewAgentRule {
                agent: r.agent.clone(),
                scope: r.scope.clone(),
                level: level.into(),
                host_alias: r.host_alias.clone(),
                project_id: r.project_id,
                config: r.config.clone(),
                granted_by: r.granted_by.clone(),
                granted_at: r.granted_at,
                expires_at: r.expires_at,
                definition: r.definition.clone(),
                bindings: r.bindings.clone(),
            })
            .unwrap();
            self.rule = s.agent_rule(r.id).unwrap().unwrap();
        }
        self
    }
    fn executor(&self) -> FleetExecutor {
        FleetExecutor {
            store: Arc::clone(&self.store),
            host: self.world.clone(),
        }
    }
    fn actions(&self) -> Vec<crate::store::AgentActionRow> {
        self.store
            .lock()
            .unwrap()
            .list_agent_actions(None, 50)
            .unwrap()
    }
}

/// The example at `level`, its ticket asked about, the model answering `answer`.
async fn l2_run(level: &str, answer: Value) -> (Fx, Value) {
    let f = fx(
        l2(),
        json!({ "jira_tracker": "3", "project": "L2S", "bug_project": "BUG" }),
        json!([
            { "role": "logs", "server": "graylog", "name": "logs-prod",
              "env": { "GRAYLOG_API_TOKEN": "${GRAYLOG_PROD_TOKEN}" } },
            { "role": "kb", "server": "kb" },
            { "role": "memory", "namespace": "rule" }
        ]),
        true,
    )
    .at(level);
    *f.world.base.threads.lock().unwrap() = vec![thread(&[("1", "Tech help!", 1500)])];
    *f.world.base.answer.lock().unwrap() = Some(PrintOutcome::Answered {
        text: answer.to_string(),
        cost_micros: 0,
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
    let RunOutcome::Done { result, .. } = out else {
        panic!("{out:?}")
    };
    let r: Value = serde_json::from_str(&result.unwrap()).unwrap();
    (f, r)
}

fn found() -> Value {
    json!({ "summary": "Gateway key expired", "rootCause": "The payment key expired at 9:00",
        "confidence": "high", "severity": "critical", "suggestedFix": "Rotate the key",
        "exceptionClass": "KeyExpiredException" })
}

fn states(r: &Value) -> Vec<(String, String)> {
    r["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["id"].as_str().unwrap().to_string(),
                a["state"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn at_watch_the_actions_are_only_kept_in_the_result() {
    let (f, r) = l2_run("watch", found()).await;
    let want: Vec<(String, String)> = ["comment", "bug", "learn", "remember"]
        .iter()
        .map(|a| (a.to_string(), "kept".to_string()))
        .collect();
    assert_eq!(states(&r), want, "ask's `when` does not hold");
    assert!(f.actions().is_empty());
    let comment = r["actions"][0]["body"].as_str().unwrap();
    assert!(comment.contains("*Gateway key expired*") && comment.contains("Rotate the key"));
    assert!(comment.ends_with("[fleet-agent:inst]"), "{comment}");
}

#[tokio::test]
async fn at_propose_a_person_gets_each_and_at_act_the_auto_ones_run() {
    let (f, r) = l2_run("propose", found()).await;
    assert!(states(&r).iter().all(|(_, s)| s == "pending"));
    let kinds: Vec<String> = f.actions().iter().map(|a| a.kind.clone()).collect();
    assert_eq!(kinds.len(), 4);
    assert!(
        kinds.contains(&"tracker_issue".to_string()) && kinds.contains(&"mcp_call".to_string())
    );

    let (f, r) = l2_run("act", found()).await;
    let st = states(&r);
    let of = |id: &str| st.iter().find(|(a, _)| a == id).unwrap().1.clone();
    assert_eq!(of("comment"), "approved");
    assert_eq!(of("remember"), "approved");
    assert_eq!(of("bug"), "pending", "not auto");
    assert_eq!(
        of("learn"),
        "pending",
        "an mcp_call is never auto without auto_ok"
    );
    drop(f);

    // needsMoreInfo: the question, and nothing after it (stop).
    let mut ask = found();
    ask["needsMoreInfo"] = "Which terminal?".into();
    let (_f, r) = l2_run("act", ask).await;
    assert_eq!(states(&r), [("ask".to_string(), "approved".to_string())]);
}

#[tokio::test]
async fn the_executor_sends_what_was_approved_within_limits() {
    let (f, _) = l2_run("propose", found()).await;
    let ex = f.executor();
    let by_kind = |k: &str| f.actions().into_iter().find(|a| a.kind == k).unwrap();
    // The comment on the subject.
    assert_eq!(
        ex.execute(&by_kind("tracker_comment")).await,
        ActionResult::Executed
    );
    // The bug in BUG, linked to the ticket.
    assert_eq!(
        ex.execute(&by_kind("tracker_issue")).await,
        ActionResult::Executed
    );
    let writes = f.world.base.writes.lock().unwrap().clone();
    assert!(matches!(&writes[0], (3, WriteOp::RichComment { key, .. }) if key == "L2S-7"));
    assert!(
        matches!(&writes[1], (3, WriteOp::CreateIssue { project, labels, .. }) if project == "BUG" && labels.is_empty())
    );
    assert!(
        matches!(&writes[2], (3, WriteOp::LinkIssues { inward, outward, link_type })
        if inward == "BUG-9" && outward == "L2S-7" && link_type == "Causes")
    );
    // The record into the rule's own namespace.
    assert_eq!(ex.execute(&by_kind("record")).await, ActionResult::Executed);
    let rec = f
        .store
        .lock()
        .unwrap()
        .agent_record(
            &format!("rule:{}", f.rule.id),
            "record",
            "KeyExpiredException",
            crate::store::now_unix(),
        )
        .unwrap()
        .unwrap();
    assert!(rec.value.contains("L2S-7") && rec.written_by_job.is_some());
    // The mcp_call: the tool's inputSchema decides.
    let learn = by_kind("mcp_call");
    *f.world.tools.lock().unwrap() = vec![json!({ "name": "add_known_error", "inputSchema": {
        "type": "object", "required": ["exception", "rootCause"], "additionalProperties": false,
        "properties": { "exception": { "type": "string" }, "rootCause": { "type": "string" },
            "issueKey": { "type": "string" } } } })];
    *f.world.call_answer.lock().unwrap() = json!({ "jsonrpc": "2.0", "id": 2,
        "result": { "content": [{ "type": "text", "text": "ok" }] } })
    .to_string();
    assert_eq!(ex.execute(&learn).await, ActionResult::Executed);
    let (entry, tool, args) = f.world.calls.lock().unwrap()[0].clone();
    assert_eq!(
        (tool.as_str(), args["issueKey"].as_str()),
        ("add_known_error", Some("L2S-7"))
    );
    assert_eq!(entry["env"]["TOKEN"], "kb-tok");
    *f.world.tools.lock().unwrap() = vec![json!({ "name": "add_known_error", "inputSchema": {
        "type": "object", "required": ["fingerprint"], "properties": {} } })];
    match ex.execute(&learn).await {
        ActionResult::Failed(e) => assert!(e.contains("fingerprint is required"), "{e}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_executor_refuses_what_the_rule_no_longer_allows() {
    let (f, _) = l2_run("propose", found()).await;
    let ex = f.executor();
    let mut comment = f
        .actions()
        .into_iter()
        .find(|a| a.kind == "tracker_comment")
        .unwrap();
    // A key outside the rule's projects and not the subject.
    let mut p: Value = serde_json::from_str(&comment.payload).unwrap();
    p["key"] = "HR-1".into();
    comment.payload = p.to_string();
    match ex.execute(&comment).await {
        ActionResult::Failed(e) => assert!(e.contains("outside limits.write_projects"), "{e}"),
        other => panic!("{other:?}"),
    }
    p["key"] = "L2S-7".into();
    p["tracker"] = 4.into();
    comment.payload = p.to_string();
    assert!(
        matches!(ex.execute(&comment).await, ActionResult::Failed(e) if e.contains("not the instance's"))
    );
    p["tracker"] = 3.into();
    p["body"] = "x".repeat(6000).into();
    comment.payload = p.to_string();
    assert!(
        matches!(ex.execute(&comment).await, ActionResult::Failed(e) if e.contains("over 5000"))
    );
    let mut rec = f
        .actions()
        .into_iter()
        .find(|a| a.kind == "record")
        .unwrap();
    let mut rp: Value = serde_json::from_str(&rec.payload).unwrap();
    rp["namespace"] = "shared:someone-else".into();
    rec.payload = rp.to_string();
    assert!(
        matches!(ex.execute(&rec).await, ActionResult::Failed(e) if e.contains("not the rule's namespace"))
    );
    // Back at watch, nothing goes out.
    let f = f.at("watch");
    let any = f.actions().remove(0);
    assert!(matches!(
        f.executor().execute(&any).await,
        ActionResult::Expired(_)
    ));
    assert!(f.world.base.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_loop_guard_ignores_its_own_comments_and_caps_a_subject() {
    let f = l2_fx();
    *f.world.base.threads.lock().unwrap() = vec![thread(&[(
        "9",
        "Tech help! was answered\n\n[fleet-agent:other]",
        1500,
    )])];
    let polled = f
        .poller()
        .poll(&f.rule, Some(&f.cursor(1000, None)), 2000)
        .await
        .unwrap();
    assert!(polled.jobs.is_empty(), "a fleet agent's own comment");
    // Five finished runs today (one at a time: one running job per
    // subject): the sixth is skipped.
    let now = crate::store::now_unix();
    for i in 0..5 {
        let s = f.store.lock().unwrap();
        s.record_agent_poll(
            f.rule.id,
            0,
            None,
            &[crate::store::NewAgentJob {
                rule_id: f.rule.id,
                agent: AGENT.into(),
                subject: "tracker:3:L2S-7".into(),
                trigger: "comment".into(),
                dedup_key: format!("done{i}"),
                not_before: 0,
            }],
        )
        .unwrap();
        let claimed = s.claim_agent_jobs(&[AGENT], now, 10, 60).unwrap();
        assert_eq!(claimed.len(), 1);
        s.finish_agent_job(
            claimed[0].id,
            &crate::store::AgentJobEnd::Final {
                state: "done",
                result: None,
                error_kind: None,
                error: None,
            },
            0,
            now,
        )
        .unwrap();
    }
    let job = f.claim(&[crate::store::NewAgentJob {
        rule_id: f.rule.id,
        agent: AGENT.into(),
        subject: "tracker:3:L2S-7".into(),
        trigger: "comment".into(),
        dedup_key: "sixth".into(),
        not_before: 0,
    }]);
    match f.runner().run(&job[0], Some(&f.rule)).await {
        RunOutcome::Skipped { reason } => assert!(reason.contains("loop guard"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn arguments_are_checked_against_the_tools_schema() {
    let schema = json!({ "type": "object", "required": ["a"], "additionalProperties": false,
        "properties": { "a": { "type": "string" }, "n": { "type": ["integer", "null"] } } });
    assert!(actions::check_arguments(&schema, &json!({ "a": "x", "n": 3 })).is_empty());
    assert!(actions::check_arguments(&schema, &json!({ "a": "x", "n": null })).is_empty());
    let p = actions::check_arguments(&schema, &json!({ "n": 1.5, "z": 1 }));
    assert_eq!(
        p,
        [
            "a is required",
            "n is not integer or null",
            "z is not a parameter"
        ]
    );
    assert!(!actions::check_arguments(&schema, &json!([1])).is_empty());
    assert_eq!(actions::project_of("BUG-12"), "BUG");
    assert_eq!(actions::fit("abcdef", 4), "abc…");
}

// ── step 6: GitHub sources and PR actions, other trackers ──

fn pr(number: i64, sha: &str, draft: bool) -> crate::service::agents::review::plan::OpenPr {
    crate::service::agents::review::plan::OpenPr {
        number,
        title: format!("PR {number}"),
        head_sha: sha.into(),
        is_draft: draft,
        base_ref: "main".into(),
        head_ref: "feat".into(),
        author: Some("eva".into()),
    }
}

/// A definition triggered by `trigger` with a pr_comment and a pr_labels action.
fn gh_fx(trigger: Value) -> Fx {
    let d = definition::parse_value(&json!({
        "inputs": [{ "name": "repo", "type": "text", "label": "Repo", "required": true }],
        "trigger": trigger,
        "run": { "workspace": "home", "output_schema": { "type": "object",
            "required": ["summary"], "properties": { "summary": { "type": "string" },
                "risky": { "type": "boolean" } } } },
        "actions": [
            { "id": "say", "do": "pr_comment", "target": "{{subject.number}}",
              "body": "Summary: {{answer.summary}}", "auto": true },
            { "id": "flag", "when": { "field": "risky", "truthy": true }, "do": "pr_labels",
              "target": "{{subject.number}}", "labels": ["risky", "-safe"] }
        ],
        "limits": { "repos": ["acme/other"] }
    }))
    .unwrap();
    assert_eq!(
        definition::problems(&d, &BTreeMap::new()),
        Vec::<String>::new()
    );
    let loaded = Loaded {
        name: "gh".into(),
        definition: d,
        skill: "Read the PR.".into(),
        templates: BTreeMap::new(),
    };
    fx(loaded, json!({ "repo": "acme/pos" }), json!([]), false)
}

#[tokio::test]
async fn a_pr_at_a_new_head_is_one_job_and_a_draft_none() {
    let f = gh_fx(json!({ "source": "github_prs", "repo": "{{repo}}" }));
    *f.world.prs.lock().unwrap() = vec![pr(4, "abc", false), pr(5, "def", true)];
    let p = f.poller();
    let first = p
        .poll(&f.rule, Some(&f.cursor(100, None)), 200)
        .await
        .unwrap();
    let keys: Vec<&str> = first.jobs.iter().map(|j| j.dedup_key.as_str()).collect();
    assert_eq!(keys, [format!("r{}:gh:acme/pos#4@abc", f.rule.id)]);
    let again = p
        .poll(&f.rule, Some(&f.cursor(200, None)), 300)
        .await
        .unwrap();
    assert!(again.jobs.is_empty(), "the same head is not new");
    *f.world.prs.lock().unwrap() = vec![pr(4, "abd", false)];
    let pushed = p
        .poll(&f.rule, Some(&f.cursor(300, None)), 400)
        .await
        .unwrap();
    assert_eq!(pushed.jobs.len(), 1, "a push is a new job");
}

#[tokio::test]
async fn a_comment_that_asks_is_a_job_and_a_bots_or_agents_never() {
    let f = gh_fx(json!({ "source": "github_comments", "repo": "acme/pos",
        "match": { "any_phrase": "/review" } }));
    let c = |id: i64, body: &str, user_type: &str| crate::service::agents::review::PrComment {
        id,
        number: 4,
        body: body.into(),
        user_type: user_type.into(),
    };
    *f.world.comments.lock().unwrap() = vec![
        c(1, "please /review", "User"),
        c(2, "/review", "Bot"),
        c(3, "/review done\n\n[fleet-agent:inst]", "User"),
        c(4, "lgtm", "User"),
    ];
    let polled = f
        .poller()
        .poll(&f.rule, Some(&f.cursor(1000, None)), 2000)
        .await
        .unwrap();
    let keys: Vec<&str> = polled.jobs.iter().map(|j| j.dedup_key.as_str()).collect();
    assert_eq!(keys, [format!("r{}:gh:acme/pos#4#c1", f.rule.id)]);
    let asked = f.world.gh_writes.lock().unwrap()[0].clone();
    assert_eq!(
        asked, "comments acme/pos since 1970-01-01T00:14:40Z",
        "the window opens at the last poll less the slack"
    );
    assert!(
        super::github::repo_ok("acme/pos")
            && !super::github::repo_ok("acme")
            && !super::github::repo_ok("a/..")
    );
}

#[tokio::test]
async fn a_pr_answer_comments_and_labels_within_the_repos() {
    let f = gh_fx(json!({ "source": "github_prs", "repo": "{{repo}}" })).at("act");
    *f.world.prs.lock().unwrap() = vec![pr(4, "abc", false)];
    // The kept subject lives from now (the run reads it at now).
    let now = crate::store::now_unix();
    let polled = f
        .poller()
        .poll(&f.rule, Some(&f.cursor(now - 100, None)), now)
        .await
        .unwrap();
    let jobs = f.claim(&polled.jobs);
    *f.world.base.answer.lock().unwrap() = Some(PrintOutcome::Answered {
        text: r#"{"summary":"Looks fine","risky":true}"#.into(),
        cost_micros: 0,
    });
    let out = f.runner().run(&jobs[0], Some(&f.rule)).await;
    let RunOutcome::Done { result, .. } = out else {
        panic!("{out:?}")
    };
    let r: Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(r["subject"]["head_sha"], "abc");
    assert_eq!(
        states(&r),
        [
            ("say".to_string(), "approved".to_string()),
            ("flag".to_string(), "pending".to_string())
        ]
    );
    let ex = f.executor();
    for a in f.actions() {
        assert_eq!(ex.execute(&a).await, ActionResult::Executed, "{}", a.kind);
    }
    let writes = f.world.gh_writes.lock().unwrap().clone();
    assert!(
        writes
            .iter()
            .any(|w| w.starts_with("comment acme/pos#4: Summary: Looks fine")
                && w.ends_with("[fleet-agent:inst]")),
        "{writes:?}"
    );
    assert!(
        writes
            .iter()
            .any(|w| w == "labels acme/pos#4 +[\"risky\"] -[\"safe\"]"),
        "{writes:?}"
    );
    // Another repo than the trigger's or limits.repos is refused.
    let mut a = f
        .actions()
        .into_iter()
        .find(|a| a.kind == "pr_comment")
        .unwrap();
    let mut p: Value = serde_json::from_str(&a.payload).unwrap();
    p["repo"] = "evil/repo".into();
    a.payload = p.to_string();
    assert!(matches!(ex.execute(&a).await, ActionResult::Failed(e) if e.contains("limits.repos")));
    p["repo"] = "acme/other".into();
    a.payload = p.to_string();
    assert_eq!(ex.execute(&a).await, ActionResult::Executed, "limits.repos");
}

#[tokio::test]
async fn another_tracker_gets_a_plain_comment_and_no_link() {
    let (f, _) = l2_run("propose", found()).await;
    *f.world.provider.lock().unwrap() = Some("github".into());
    let ex = f.executor();
    let by_kind = |k: &str| f.actions().into_iter().find(|a| a.kind == k).unwrap();
    assert_eq!(
        ex.execute(&by_kind("tracker_comment")).await,
        ActionResult::Executed
    );
    let writes = f.world.base.writes.lock().unwrap().clone();
    assert!(
        matches!(&writes[0], (3, WriteOp::Comment { key, body, .. }) if key == "L2S-7" && body.contains("Gateway key expired"))
    );
    // The bug links to the ticket: links are Jira's.
    match ex.execute(&by_kind("tracker_issue")).await {
        ActionResult::Failed(e) => assert!(e.contains("Jira's"), "{e}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        f.world.base.writes.lock().unwrap().len(),
        1,
        "nothing created"
    );
}

/// A definition whose answer lists findings, each maybe with a proposal.
fn proposing() -> Loaded {
    let d = definition::parse_value(&json!({
        "trigger": { "source": "manual" },
        "run": { "workspace": "home", "output_schema": { "type": "object",
            "required": ["summary", "findings"], "properties": {
                "summary": { "type": "string" },
                "findings": { "type": "array", "items": { "type": "object",
                    "required": ["title", "kind"], "properties": {
                        "title": { "type": "string" },
                        "kind": { "enum": ["none", "task", "setting", "recover", "fix"] },
                        "steps": { "type": "array", "items": { "type": "string" } },
                        "key": { "type": "string" }, "value": { "type": "string" },
                        "session": { "type": "string" }, "project": { "type": "string" } } } } } } },
        "actions": [
            { "id": "task", "do": "work_task", "each": "findings", "auto": true,
              "when": { "field": "kind", "eq": "task" },
              "summary": "{{item.title}}", "body": "Found by the run: {{answer.summary}}",
              "steps": "{{item.steps}}" },
            { "id": "tune", "do": "setting", "each": "findings",
              "when": { "field": "kind", "eq": "setting" },
              "key": "{{item.key}}", "value": "{{item.value}}", "body": "{{item.title}}" },
            { "id": "recover", "do": "session_recover", "each": "findings",
              "when": { "field": "kind", "eq": "recover" },
              "target": "{{item.session}}", "body": "{{item.title}}" },
            { "id": "fix", "do": "fix_session", "each": "findings",
              "when": { "field": "kind", "eq": "fix" },
              "project": "{{item.project}}", "summary": "{{item.title}}", "body": "Fix: {{item.title}}" }
        ]
    }))
    .unwrap();
    let problems = definition::problems(&d, &BTreeMap::new());
    assert!(problems.is_empty(), "{problems:#?}");
    Loaded {
        name: "proposing".into(),
        definition: d,
        skill: "Propose.".into(),
        templates: BTreeMap::new(),
    }
}

#[tokio::test]
async fn each_finding_becomes_its_own_fleet_proposal() {
    let f = fx(proposing(), json!({}), json!([]), false).at("act");
    let now = crate::store::now_unix();
    queue_manual(
        &f.store,
        &f.rule,
        super::definition::TriggerSource::Manual,
        None,
        None,
        now,
    )
    .unwrap();
    let jobs = f
        .store
        .lock()
        .unwrap()
        .claim_agent_jobs(&[AGENT], now, 10, 60)
        .unwrap();
    let key = crate::service::settings::SPECS
        .iter()
        .find(|sp| sp.ai != crate::service::settings::AiPolicy::Never && sp.owned_by.is_none())
        .unwrap()
        .key;
    let answer = json!({ "summary": "two problems", "findings": [
        { "title": "Flaky reconnect", "kind": "task", "steps": ["Reproduce", "- Fix the backoff", ""] },
        { "title": "Disk alarm too late", "kind": "setting", "key": key, "value": "1" },
        { "title": "Not a proposal", "kind": "none" },
        { "title": "fed-v2 stuck", "kind": "recover", "session": "fed-v2" },
        { "title": "Second task", "kind": "task" },
        { "title": "Unknown key", "kind": "setting", "key": "no.such.key", "value": "1" }
    ]});
    *f.world.base.answer.lock().unwrap() = Some(PrintOutcome::Answered {
        text: answer.to_string(),
        cost_micros: 0,
    });
    let out = f.runner().run(&jobs[0], Some(&f.rule)).await;
    let RunOutcome::Done { result, .. } = out else {
        panic!("{out:?}")
    };
    let r: Value = serde_json::from_str(&result.unwrap()).unwrap();
    let st = states(&r);
    assert_eq!(
        st,
        [
            ("task", "approved"),
            ("task", "approved"),
            ("tune", "pending"),
            ("recover", "pending"),
            ("tune", "skipped"),
        ]
        .map(|(a, b)| (a.to_string(), b.to_string())),
        "a work_task may run alone at act; the others wait; a key no agent may set is skipped"
    );
    let first = r["actions"][0]["body"].as_str().unwrap();
    assert!(
        first.starts_with("Flaky reconnect") && first.contains("- Fix the backoff"),
        "{first}"
    );
    // Approved: the executor hands each to fleet.
    let ex = f.executor();
    for a in f.actions() {
        assert_eq!(ex.execute(&a).await, ActionResult::Executed, "{}", a.kind);
    }
    let ops = f.world.fleet_ops.lock().unwrap().clone();
    assert_eq!(ops.len(), 4);
    assert!(ops
        .iter()
        .any(|o| matches!(o, super::actions::FleetOp::WorkTask(p)
        if p.title == "Flaky reconnect" && p.steps == ["Reproduce", "Fix the backoff"])));
    assert!(ops
        .iter()
        .any(|o| matches!(o, super::actions::FleetOp::Recover(p) if p.session == "fed-v2")));
}

#[test]
fn a_fleet_proposal_that_acts_alone_is_refused() {
    let mut d = proposing().definition;
    d.actions[2].auto = true;
    let p = definition::problems(&d, &BTreeMap::new());
    assert!(
        p.iter().any(|x| x.contains("never runs without a person")),
        "{p:?}"
    );
    let mut d = proposing().definition;
    d.actions[0].each = Some("summary".into());
    let p = definition::problems(&d, &BTreeMap::new());
    assert!(
        p.iter().any(|x| x.contains("not an array of objects")),
        "{p:?}"
    );
    let mut d = proposing().definition;
    d.actions[0].each = None;
    let p = definition::problems(&d, &BTreeMap::new());
    assert!(
        p.iter().any(|x| x.contains("there is no item here")),
        "{p:?}"
    );
}

/// A GitHub item, updated at `updated`, with `comments`.
fn gh_item(updated: i64, comments: &[(&str, &str, i64)]) -> ItemThread {
    let mut t = thread(comments);
    t.key = "o/r#12".into();
    t.updated = Some(updated);
    t.assignee_id = Some("octocat".into());
    ItemThread {
        thread: t,
        url: Some("https://github.com/o/r/issues/12".into()),
    }
}

#[tokio::test]
async fn another_trackers_synced_items_are_polled_and_their_subject_kept() {
    let f = fx(
        small(
            json!({ "source": "tracker_comments", "tracker": "3",
                "match": { "any_phrase": "/ask" } }),
            json!([]),
        ),
        json!({}),
        json!([]),
        false,
    );
    *f.world.provider.lock().unwrap() = Some("github".into());
    let now = crate::store::now_unix();
    let since = now - 300;
    *f.world.items.lock().unwrap() = vec![
        gh_item(
            now - 10,
            &[
                ("c1", "/ask why is CI red", now - 20),
                ("c2", "unrelated", now - 15),
                ("c3", "/ask again [fleet-agent:x]", now - 12),
            ],
        ),
        // A comment from before the window is not asked again.
        gh_item(now - 60, &[("c0", "/ask old", now - 4000)]),
    ];
    let p = f.poller();
    let polled = p
        .poll(&f.rule, Some(&f.cursor(since, None)), now)
        .await
        .unwrap();
    let keys: Vec<&str> = polled.jobs.iter().map(|j| j.dedup_key.as_str()).collect();
    assert_eq!(keys, [format!("r{}:tracker:3:o/r#12#cc1", f.rule.id)]);
    assert_eq!(
        f.world.item_since.lock().unwrap()[0],
        since - SYNC_LAG_SECS - POLL_SLACK_SECS,
        "read back further by a sync's lag"
    );
    assert!(f.world.base.jql.lock().unwrap().is_empty(), "no JQL");
    // The same comment, seen again within the lag, is not a second job.
    let again = p
        .poll(&f.rule, Some(&f.cursor(now, None)), now + 60)
        .await
        .unwrap();
    assert!(again.jobs.is_empty(), "{:?}", again.jobs);

    // The run reads the kept subject: the item and the comment that asked.
    *f.world.base.answer.lock().unwrap() = Some(PrintOutcome::Answered {
        text: r#"{"summary":"flaky test"}"#.into(),
        cost_micros: 1,
    });
    let jobs = f.claim(&polled.jobs);
    let out = f.runner().run(&jobs[0], Some(&f.rule)).await;
    let RunOutcome::Done { result, .. } = out else {
        panic!("{out:?}")
    };
    let r: Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(r["subject"]["key"], "o/r#12");
    assert_eq!(r["subject"]["url"], "https://github.com/o/r/issues/12");
    assert_eq!(r["subject"]["comment"]["body"], "/ask why is CI red");
    assert!(
        f.world.base.jql.lock().unwrap().is_empty(),
        "not read from Jira"
    );
}

#[tokio::test]
async fn another_tracker_takes_an_assignment_and_refuses_a_jql_query() {
    let assigned = fx(
        small(
            json!({ "source": "tracker_assigned", "tracker": "3",
                "match": { "assignee": "octocat" } }),
            json!([]),
        ),
        json!({}),
        json!([]),
        false,
    );
    *assigned.world.provider.lock().unwrap() = Some("linear".into());
    let now = crate::store::now_unix();
    *assigned.world.items.lock().unwrap() = vec![gh_item(now - 5, &[])];
    let polled = assigned
        .poller()
        .poll(&assigned.rule, Some(&assigned.cursor(now - 60, None)), now)
        .await
        .unwrap();
    assert_eq!(
        polled.jobs[0].dedup_key,
        format!("r{}:tracker:3:o/r#12#assigned:octocat", assigned.rule.id)
    );

    let f = l2_fx();
    *f.world.provider.lock().unwrap() = Some("asana".into());
    let err = f
        .poller()
        .poll(&f.rule, Some(&f.cursor(now - 60, None)), now)
        .await
        .unwrap_err();
    assert!(err.contains("query is JQL"), "{err}");
}
