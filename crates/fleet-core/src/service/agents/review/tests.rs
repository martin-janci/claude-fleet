//! The reviewer's poller and runner against a fake host.

use super::*;
use crate::store::{AgentRuleRow, NewAgentJob, NewAgentRule};
use std::sync::Mutex as StdMutex;

const SHA: &str = "aaaaaaa1111";

struct FakeHost {
    open: StdMutex<Vec<plan::OpenPr>>,
    states: StdMutex<Vec<(i64, String)>>,
    diff: StdMutex<Result<String, String>>,
    ignore: Option<String>,
    prepared: StdMutex<Option<Prepared>>,
    answer: StdMutex<Option<PrintOutcome>>,
    /// What happened, in order.
    log: StdMutex<Vec<String>>,
    /// The last prompt and run.
    last: StdMutex<Option<(PrintRun, String)>>,
}

impl Default for FakeHost {
    fn default() -> Self {
        FakeHost {
            open: StdMutex::default(),
            states: StdMutex::default(),
            diff: StdMutex::new(Ok(String::new())),
            ignore: None,
            prepared: StdMutex::default(),
            answer: StdMutex::default(),
            log: StdMutex::default(),
            last: StdMutex::default(),
        }
    }
}

impl FakeHost {
    fn log(&self, s: impl Into<String>) {
        self.log.lock().unwrap().push(s.into());
    }
    fn logged(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl ReviewHost for FakeHost {
    async fn open_prs(&self, _: &str, _: &str) -> Result<Vec<plan::OpenPr>, String> {
        Ok(self.open.lock().unwrap().clone())
    }
    async fn pr_state(&self, _: &str, _: &str, number: i64) -> Result<String, String> {
        let st = self.states.lock().unwrap();
        st.iter()
            .find(|(n, _)| *n == number)
            .map(|(_, s)| s.clone())
            .ok_or_else(|| "unknown".into())
    }
    async fn diff(&self, _: &str, _: &str, _: i64) -> Result<String, String> {
        self.log("diff");
        self.diff.lock().unwrap().clone()
    }
    async fn claude_ignore(&self, _: &str, _: &str, base: &str) -> Option<String> {
        self.log(format!("claudeignore@{base}"));
        self.ignore.clone()
    }
    async fn checkout(&self, host: &str, project_id: i64) -> Result<String, IpcError> {
        self.log(format!("checkout {host} {project_id}"));
        Ok("/p/o/r".into())
    }
    async fn prepare(
        &self,
        _: &str,
        root: &str,
        job_id: i64,
        number: i64,
        sha: &str,
    ) -> Result<(Prepared, String), IpcError> {
        self.log(format!("prepare #{number}@{sha}"));
        let p = self.prepared.lock().unwrap().unwrap_or(Prepared::Ready);
        Ok((p, workspace::workspace_dir(root, job_id)))
    }
    async fn run(&self, _: &str, run: &PrintRun, prompt: &str) -> Result<PrintOutcome, IpcError> {
        self.log("run");
        *self.last.lock().unwrap() = Some((run.clone(), prompt.to_string()));
        Ok(self.answer.lock().unwrap().clone().expect("an answer"))
    }
    async fn cleanup(&self, _: &str, _: &str, job_id: i64) {
        self.log(format!("cleanup {job_id}"));
    }
    async fn prune(&self, _: &str, _: &str) {
        self.log("prune");
    }
}

const DIFF: &str = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,3 @@\n fn a() {}\n+fn b() {}\n+fn c() {}\n";

fn pr(n: i64, sha: &str) -> plan::OpenPr {
    plan::OpenPr {
        number: n,
        title: "Add b and c".into(),
        head_sha: sha.into(),
        is_draft: false,
        base_ref: "main".into(),
        head_ref: "feat".into(),
        author: Some("ann".into()),
    }
}

fn answered(json: &str) -> PrintOutcome {
    PrintOutcome::Answered {
        text: json.into(),
        cost_micros: 900,
    }
}

struct Fx {
    store: Arc<Mutex<Store>>,
    host: Arc<FakeHost>,
    rule: AgentRuleRow,
}

fn fx(config: &str) -> Fx {
    let s = Store::open_in_memory().unwrap();
    s.upsert_project("o", "r", "/p/o/r").unwrap();
    let id = s
        .grant_agent_rule(&NewAgentRule {
            agent: "review".into(),
            scope: "gh:o/r".into(),
            level: "watch".into(),
            host_alias: "box".into(),
            project_id: None,
            config: config.into(),
            granted_by: "t".into(),
            granted_at: 0,
            expires_at: None,
        })
        .unwrap();
    let rule = s.agent_rule(id).unwrap().unwrap();
    let host = Arc::new(FakeHost {
        diff: StdMutex::new(Ok(DIFF.into())),
        ..Default::default()
    });
    Fx {
        store: Arc::new(Mutex::new(s)),
        host,
        rule,
    }
}

impl Fx {
    fn poller(&self) -> ReviewPoller {
        ReviewPoller {
            store: Arc::clone(&self.store),
            host: self.host.clone(),
        }
    }
    fn runner(&self) -> ReviewRunner {
        ReviewRunner {
            store: Arc::clone(&self.store),
            host: self.host.clone(),
        }
    }
    async fn poll(&self, now: i64) -> Vec<NewAgentJob> {
        self.poller()
            .poll(&self.rule, None, now)
            .await
            .unwrap()
            .jobs
    }
    /// Poll, record the jobs as the runtime does, and claim the first.
    async fn job(&self) -> AgentJobRow {
        let jobs = self.poll(1000).await;
        let s = self.store.lock().unwrap();
        s.record_agent_poll(self.rule.id, 1000, None, &jobs)
            .unwrap();
        s.claim_agent_jobs(&["review"], 5000, 1, 60)
            .unwrap()
            .remove(0)
    }
    fn pr_row(&self, n: i64) -> crate::store::ReviewPrRow {
        self.store
            .lock()
            .unwrap()
            .review_pr("o/r", n)
            .unwrap()
            .unwrap()
    }
}

#[tokio::test]
async fn a_poll_tracks_the_open_prs_and_learns_which_closed() {
    let f = fx("{}");
    *f.host.open.lock().unwrap() = vec![pr(1, SHA), pr(2, "bbbbbbb")];
    let jobs = f.poll(1000).await;
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].not_before, 1060, "the default debounce");
    *f.host.open.lock().unwrap() = vec![pr(2, "bbbbbbb")];
    *f.host.states.lock().unwrap() = vec![(1, "MERGED".into())];
    f.poll(1100).await;
    let one = f.pr_row(1);
    assert_eq!((one.status.as_str(), one.closed_at), ("merged", Some(1100)));
}

#[tokio::test]
async fn a_review_runs_on_the_jobs_head_and_is_recorded() {
    let f = fx(r#"{"model":"sonnet","max_turns":9}"#);
    *f.host.open.lock().unwrap() = vec![pr(4, SHA)];
    *f.host.answer.lock().unwrap() = Some(answered(
        r#"{"verdict":"REQUEST_CHANGES","summary":"c is unused","findings":[
            {"severity":"issue","blocking":true,"path":"src/a.rs","line":3,"body":"c has no caller"},
            {"severity":"praise","path":"src/a.rs","line":2,"body":"tidy"}]}"#,
    ));
    let job = f.job().await;
    let out = f.runner().run(&job, Some(&f.rule)).await;
    let RunOutcome::Done {
        result,
        cost_micros,
    } = out
    else {
        panic!("{out:?}")
    };
    assert_eq!(cost_micros, 900);
    let r: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(
        (
            r["verdict"].as_str(),
            r["inline"].as_u64(),
            r["orphans"].as_u64()
        ),
        (Some("REQUEST_CHANGES"), Some(1), Some(1))
    );
    assert_eq!(
        f.host.logged(),
        vec![
            "diff".to_string(),
            "claudeignore@main".into(),
            "checkout box 1".into(),
            "prune".into(),
            format!("prepare #4@{SHA}"),
            "run".into(),
            format!("cleanup {}", job.id),
        ]
    );
    let (run, prompt) = f.host.last.lock().unwrap().clone().unwrap();
    assert_eq!(run.cwd, format!("/p/o/r.fleet-agents/job-{}", job.id));
    assert_eq!(run.tools, ["Read", "Grep", "Glob"]);
    assert_eq!((run.model.as_deref(), run.max_turns), (Some("sonnet"), 9));
    assert!(run.mcp_config.is_none() && run.allowed_tools.is_empty());
    assert!(run
        .system_prompt
        .as_deref()
        .unwrap()
        .contains("## Verdict Rules"));
    assert!(prompt.contains("## PR Title: Add b and c") && prompt.contains("+fn c() {}"));
    let row = f.pr_row(4);
    assert_eq!(
        (row.status.as_str(), row.last_reviewed_sha.as_deref()),
        ("reviewed", Some(SHA))
    );
    let rec = f
        .store
        .lock()
        .unwrap()
        .last_review_record("o/r", 4)
        .unwrap()
        .unwrap();
    assert!(!rec.posted, "step 3 posts nothing");
    let placed: output::PlacedReview = serde_json::from_str(&rec.review).unwrap();
    assert_eq!(placed.inline[0].line, 3);
}

#[tokio::test]
async fn a_re_review_shows_the_previous_findings() {
    let f = fx("{}");
    *f.host.open.lock().unwrap() = vec![pr(4, SHA)];
    *f.host.answer.lock().unwrap() = Some(answered(
        r#"{"verdict":"REQUEST_CHANGES","summary":"s","findings":[
            {"severity":"issue","blocking":true,"path":"src/a.rs","line":3,"body":"c has no caller"}]}"#,
    ));
    let job = f.job().await;
    f.runner().run(&job, Some(&f.rule)).await;
    *f.host.open.lock().unwrap() = vec![pr(4, "ccccccc2222")];
    *f.host.answer.lock().unwrap() = Some(answered(
        r#"{"verdict":"APPROVE","summary":"fixed","findings":[]}"#,
    ));
    let jobs = f.poll(2000).await;
    assert_eq!(jobs[0].trigger, "pushed");
    {
        let s = f.store.lock().unwrap();
        s.finish_agent_job(
            job.id,
            &crate::store::AgentJobEnd::Final {
                state: "done",
                result: None,
                error_kind: None,
                error: None,
            },
            0,
            2000,
        )
        .unwrap();
        s.record_agent_poll(f.rule.id, 2000, None, &jobs).unwrap();
    }
    let job2 = f
        .store
        .lock()
        .unwrap()
        .claim_agent_jobs(&["review"], 9000, 1, 60)
        .unwrap()
        .remove(0);
    assert!(matches!(
        f.runner().run(&job2, Some(&f.rule)).await,
        RunOutcome::Done { .. }
    ));
    let (_, prompt) = f.host.last.lock().unwrap().clone().unwrap();
    assert!(
        prompt.contains("Previous verdict was **REQUEST_CHANGES** at commit aaaaaaa"),
        "{prompt}"
    );
    assert!(prompt.contains("`src/a.rs:3`: c has no caller"));
}

#[tokio::test]
async fn an_oversized_or_empty_diff_is_skipped_without_a_run() {
    let f = fx(r#"{"max_diff_lines":3}"#);
    *f.host.open.lock().unwrap() = vec![pr(4, SHA)];
    let job = f.job().await;
    let out = f.runner().run(&job, Some(&f.rule)).await;
    assert!(
        matches!(&out, RunOutcome::Skipped { reason } if reason.starts_with("diff_too_large")),
        "{out:?}"
    );
    let row = f.pr_row(4);
    assert_eq!(row.skip_reason.as_deref(), Some("diff_too_large"));
    assert!(row.skip_diff_lines.unwrap() > 3);
    assert!(!f.host.logged().contains(&"run".to_string()));

    let f = fx(r#"{"exclude_paths":["src/**"]}"#);
    *f.host.open.lock().unwrap() = vec![pr(4, SHA)];
    let job = f.job().await;
    let out = f.runner().run(&job, Some(&f.rule)).await;
    assert!(
        matches!(&out, RunOutcome::Skipped { reason } if reason.starts_with("empty_diff")),
        "{out:?}"
    );
    assert_eq!(f.pr_row(4).skip_reason.as_deref(), Some("empty_diff"));
}

#[tokio::test]
async fn a_moved_head_a_revoked_rule_and_a_bad_answer() {
    let f = fx("{}");
    *f.host.open.lock().unwrap() = vec![pr(4, SHA)];
    let job = f.job().await;
    assert!(matches!(
        f.runner().run(&job, None).await,
        RunOutcome::Skipped { .. }
    ));
    *f.host.prepared.lock().unwrap() = Some(Prepared::Moved);
    assert!(matches!(
        f.runner().run(&job, Some(&f.rule)).await,
        RunOutcome::Skipped { .. }
    ));
    *f.host.prepared.lock().unwrap() = None;
    *f.host.answer.lock().unwrap() = Some(answered("Looks fine to me!"));
    match f.runner().run(&job, Some(&f.rule)).await {
        RunOutcome::Failed {
            kind, cost_micros, ..
        } => {
            assert_eq!((kind, cost_micros), (ErrorKind::Permanent, 900))
        }
        other => panic!("{other:?}"),
    }
    assert!(
        f.host.logged().iter().any(|l| l.starts_with("cleanup")),
        "cleaned up after a run"
    );
    *f.host.diff.lock().unwrap() = Err("GraphQL: Could not resolve to a PullRequest".into());
    assert!(matches!(
        f.runner().run(&job, Some(&f.rule)).await,
        RunOutcome::Failed {
            kind: ErrorKind::Permanent,
            ..
        }
    ));
    *f.host.diff.lock().unwrap() = Err("HTTP 406: diff is too large".into());
    assert!(matches!(
        f.runner().run(&job, Some(&f.rule)).await,
        RunOutcome::Skipped { .. }
    ));
    // The skip holds for this head: the next run does not fetch again.
    assert_eq!(f.pr_row(4).skip_reason.as_deref(), Some("diff_too_large"));
}

#[test]
fn gh_pr_list_is_read_as_open_prs() {
    let out = r#"[{"number":4,"title":"t","headRefOid":"abc","isDraft":true,
                   "baseRefName":"main","headRefName":"f","author":{"login":"ann"}},
                  {"number":0,"headRefOid":"x"},{"number":5}]"#;
    let prs = parse_pr_list(out).unwrap();
    assert_eq!(prs.len(), 1);
    assert_eq!(
        (prs[0].is_draft, prs[0].author.as_deref()),
        (true, Some("ann"))
    );
    assert!(parse_pr_list("not json").is_err());
    assert_eq!(diff_failure("the diff is too_large"), None);
}

#[test]
fn a_config_has_the_originals_defaults() {
    let c = ReviewConfig::parse("{}").unwrap();
    assert_eq!(
        (
            c.max_diff_lines,
            c.max_turns,
            c.timeout_secs,
            c.debounce_secs
        ),
        (5000, 15, 600, 60)
    );
    assert!(c.skip_drafts && c.skip_wip && c.require_tests && c.respect_claudeignore);
    assert!(ReviewConfig::parse(r#"{"timeout_secs":5}"#).is_err());
    assert!(ReviewConfig::parse(r#"{"confidence_threshold":101}"#).is_err());
}
