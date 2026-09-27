//! The `work_link` benchmark's tests: which links become cases, the leakage
//! guard, the candidate set and its recall, the time split, the providers
//! (BM25; Jev through the envelope with a fake backend, gate on and off),
//! the D39 export / labels round trip, and that no report carries text.

use super::work_link::*;
use crate::service::decide::{
    BackendError, DecideCtx, DecisionBackend, JevRequest, JevResponse, Question,
};
use crate::service::nl::{NlBucket, Ranker};
use crate::service::settings;
use crate::store::{DecisionRunFilter, Secret, Store, WorkTarget};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Reads by keyword so the tests do not depend on a model.
struct Words;
impl Ranker for Words {
    fn rank(&self, p: &str) -> Vec<(NlBucket, f64)> {
        let has = |w: &str| p.split_whitespace().any(|x| x.eq_ignore_ascii_case(w));
        let b = if has("the") {
            NlBucket::En
        } else if has("sa") {
            NlBucket::Sk
        } else {
            NlBucket::Cs
        };
        vec![(b, 1.0)]
    }
}

/// Recent enough for every window.
static NOW: std::sync::LazyLock<i64> = std::sync::LazyLock::new(crate::store::now_unix);
const DAY: i64 = 86_400;

struct W {
    s: Store,
    acme: i64,
    jira: i64,
    n: usize,
}

fn world() -> W {
    let s = Store::open_in_memory().unwrap();
    let acme = s.add_org("Acme", None, false).unwrap().id;
    s.upsert_host("h1").unwrap();
    s.set_host_org("h1", Some(acme)).unwrap();
    let jira = s
        .add_tracker("jira", "Acme Jira", "https://acme.atlassian.net")
        .unwrap()
        .id;
    s.set_tracker_org(jira, Some(acme)).unwrap();
    s.conn_for_test()
        .execute(
            "UPDATE trackers SET config = '{\"account_id\":\"me\",\"key_prefixes\":[\"PAY\"]}' WHERE id = ?1",
            [jira],
        )
        .unwrap();
    W {
        s,
        acme,
        jira,
        n: 0,
    }
}

impl W {
    fn item_in(&self, tracker: i64, key: &str, title: &str, updated: i64, mine: bool) -> i64 {
        let meta = if mine {
            Some("{\"assignee_id\":\"me\"}")
        } else {
            None
        };
        self.s
            .conn_for_test()
            .execute(
                "INSERT INTO work_items (source, tracker_id, external_id, key, title, meta, created_at, updated_at) \
                 VALUES ('jira', ?1, ?2, ?2, ?3, ?4, ?5, ?5)",
                rusqlite::params![tracker, key, title, meta, updated],
            )
            .unwrap();
        self.s.conn_for_test().last_insert_rowid()
    }

    fn item(&self, key: &str, title: &str, updated: i64) -> i64 {
        self.item_in(self.jira, key, title, updated, false)
    }

    /// A session on `host` whose conversation opened with `prompt`, linked
    /// to `item` by `source` at `at`. Returns the link id.
    fn case_on(
        &mut self,
        host: &str,
        prompt: &str,
        item: i64,
        source: &str,
        at: i64,
        branch: Option<&str>,
    ) -> i64 {
        self.n += 1;
        let n = self.n;
        self.s.upsert_host(host).unwrap();
        let sid = self
            .s
            .upsert_session(&format!("s{n}"), host, None, None, 1, 1, "running", None)
            .unwrap();
        let cid = format!("c{n}");
        self.s
            .conn_for_test()
            .execute(
                "INSERT INTO conversations (session_id, claude_session_id, started_at, start_source, first_prompt) \
                 VALUES (?1, ?2, ?3, 'startup', ?4)",
                rusqlite::params![sid, cid, at - 60, prompt],
            )
            .unwrap();
        self.s
            .conn_for_test()
            .execute(
                "UPDATE sessions SET current_branch = ?1 WHERE id = ?2",
                rusqlite::params![branch, sid],
            )
            .unwrap();
        let l = self
            .s
            .link_session_work(sid, WorkTarget::Item(item), "manual")
            .unwrap();
        self.s
            .conn_for_test()
            .execute(
                "UPDATE work_links SET source = ?1, decided_at = ?2, created_at = ?2, claude_session_id = ?3 \
                 WHERE id = ?4",
                rusqlite::params![source, at, cid, l.id],
            )
            .unwrap();
        l.id
    }

    fn case(&mut self, prompt: &str, item: i64, source: &str, at: i64) -> i64 {
        self.case_on("h1", prompt, item, source, at, None)
    }

    /// A session with a first prompt and no link.
    fn unlinked(&mut self, prompt: &str, at: i64) -> i64 {
        self.n += 1;
        let n = self.n;
        let sid = self
            .s
            .upsert_session(&format!("u{n}"), "h1", None, None, 1, 1, "running", None)
            .unwrap();
        self.s
            .conn_for_test()
            .execute(
                "INSERT INTO conversations (session_id, claude_session_id, started_at, start_source, first_prompt) \
                 VALUES (?1, ?2, ?3, 'startup', ?4)",
                rusqlite::params![sid, format!("u{n}"), at, prompt],
            )
            .unwrap();
        sid
    }
}

fn opts(split: Split, providers: Vec<Provider>) -> BenchOptions {
    BenchOptions::new(None, None, None, split, providers, None, *NOW).unwrap()
}

fn a_truths(l: &Loaded) -> Vec<&BenchCase> {
    l.cases
        .iter()
        .filter(|c| c.dataset == Dataset::A && c.truth.is_some())
        .collect()
}

/// The ten-ticket fixture: distinct titles, prompts that describe them in
/// other words, and a few that give nothing away.
fn seeded() -> W {
    let mut w = world();
    let titles = [
        ("PAY-1", "Login redirect loops on mobile"),
        ("PAY-2", "Billing export to CSV"),
        ("PAY-3", "Upgrade the database driver"),
        ("PAY-4", "Dark mode for the settings page"),
        ("PAY-5", "Flaky integration test in checkout"),
        ("PAY-6", "Invoice PDF has wrong totals"),
        ("PAY-7", "Rate limit the public API"),
        ("PAY-8", "Search results are slow"),
        ("PAY-9", "Onboarding email template"),
        ("PAY-10", "Crash when uploading avatars"),
    ];
    let prompts = [
        "the login page keeps redirecting on my phone, look at it",
        "we need the billing numbers as a CSV export",
        "bump the database driver to the new major",
        "add a dark theme to the settings screen",
        "the checkout integration test fails randomly",
        "totals on the invoice PDF are off by one",
        "put a rate limit in front of the public API",
        "why is search so slow for big accounts",
        "write the welcome email for onboarding",
        "uploading an avatar crashes the app",
    ];
    let ids: Vec<i64> = titles
        .iter()
        .map(|(k, t)| w.item(k, t, *NOW - 100 * DAY))
        .collect();
    for (i, p) in prompts.iter().enumerate() {
        w.case(p, ids[i], "manual", *NOW - (50 - i as i64) * DAY);
    }
    w
}

// --- which links are cases ----------------------------------------------------

#[test]
fn only_links_a_person_decided_become_cases() {
    let mut w = world();
    let i = w.item("PAY-1", "Login redirect loops", *NOW - 10 * DAY);
    w.case("fix the login thing please", i, "manual", *NOW - 5 * DAY);
    w.case("start the login thing now", i, "started", *NOW - 5 * DAY);
    for src in ["agent", "agent_inferred", "branch", "prompt"] {
        w.case("some agent decided this one", i, src, *NOW - 5 * DAY);
    }
    // A suggestion is not a decision.
    let l = w.case("only suggested here", i, "manual", *NOW - 5 * DAY);
    w.s.conn_for_test()
        .execute(
            "UPDATE work_links SET state = 'suggested' WHERE id = ?1",
            [l],
        )
        .unwrap();
    // Fleet's own start prompt is not a person's prompt.
    w.case(
        &crate::service::trackers::tickets::start_prompt("PAY-1"),
        i,
        "started",
        *NOW - 5 * DAY,
    );
    let got = load(&w.s, &Words, &opts(Split::All, vec![]), None).unwrap();
    assert_eq!(a_truths(&got).len(), 2);
    assert_eq!(got.sizes.a_links_read.0, 3);
    assert_eq!(got.sizes.a_fleet_typed.0, 1);
    assert!(a_truths(&got)
        .iter()
        .all(|c| c.truth.as_deref() == Some(option_id(i).as_str())));
}

#[test]
fn the_org_filter_keeps_one_org() {
    let mut w = world();
    let other = w.s.add_org("Other", None, false).unwrap().id;
    let t2 =
        w.s.add_tracker("jira", "Other Jira", "https://other.atlassian.net")
            .unwrap()
            .id;
    w.s.set_tracker_org(t2, Some(other)).unwrap();
    let a = w.item("PAY-1", "Login redirect loops", *NOW - 10 * DAY);
    let b = w.item_in(t2, "OTH-1", "Other org work", *NOW - 10 * DAY, false);
    w.case("look at the login", a, "manual", *NOW - DAY);
    w.case("look at the other thing", b, "manual", *NOW - DAY);
    let o = BenchOptions::new(None, Some(w.acme), None, Split::All, vec![], None, *NOW).unwrap();
    let got = load(&w.s, &Words, &o, None).unwrap();
    assert_eq!(a_truths(&got).len(), 1);
    assert_eq!(got.sizes.a_other_org.0, 1);
    assert_eq!(a_truths(&got)[0].org_id, Some(w.acme));
}

// --- the leakage guard ----------------------------------------------------------

fn ctx() -> crate::service::work::recognize::RecognizeCtx {
    guard_ctx(&[])
}

#[test]
fn the_guard_removes_keys_urls_and_the_branch() {
    let out = redact_prompt(
        "Fix PAY-12 and pay-7 (see https://acme.atlassian.net/browse/PAY-12 and #34), on feature/pay-12-login-fix, mail bob@acme.io",
        &ctx(),
        &Redact {
            branch: Some("feature/pay-12-login-fix"),
            truth_key: Some("PAY-7"),
            ..Default::default()
        },
    );
    for gone in [
        "PAY-12",
        "pay-7",
        "atlassian",
        "#34",
        "pay-12-login-fix",
        "bob@",
    ] {
        assert!(
            !out.to_lowercase().contains(&gone.to_lowercase()),
            "{gone}: {out}"
        );
    }
    assert!(out.starts_with("Fix [ref] and [ref]"), "{out}");
}

#[test]
fn the_guard_removes_the_exact_title_case_insensitively_and_repeatedly() {
    let out = redact_prompt(
        "please do: login REDIRECT loops. Also loLogin redirect loopsgin redirect loops",
        &ctx(),
        &Redact {
            truth_title: Some("Login redirect loops"),
            ..Default::default()
        },
    );
    assert!(
        !out.to_lowercase().contains("login redirect loops"),
        "{out}"
    );
}

#[test]
fn a_started_link_loses_every_word_of_its_slug_even_inflected() {
    let title = "Opraviť prihlásenie cez SSO";
    let slug = crate::service::trackers::tickets::branch_slug("PAY-3", title);
    let out = redact_prompt(
        "prosím oprav prihlasenia cez sso na stránke, ďakujem",
        &ctx(),
        &Redact {
            truth_key: Some("PAY-3"),
            truth_title: Some(title),
            started_slug: Some(&slug),
            ..Default::default()
        },
    );
    for w in ["prihlasenia", "cez", "sso", "oprav"] {
        assert!(
            !out.split(|c: char| !c.is_alphanumeric())
                .any(|x| x.eq_ignore_ascii_case(w)),
            "{w}: {out}"
        );
    }
    assert!(out.contains("stránke"), "unrelated words stay: {out}");
}

/// The test the test map asks for: for EVERY case, neither the truth's key,
/// nor its title, nor (started) its slug or a word of it, nor the branch,
/// survives in the state that would be sent.
#[test]
fn no_case_leaks_its_truth() {
    let mut w = world();
    let specs: Vec<(&str, &str, &str, &str, Option<&str>)> = vec![
        (
            "PAY-1",
            "Login redirect loops",
            "Fix PAY-1: Login redirect loops asap",
            "manual",
            None,
        ),
        (
            "PAY-2",
            "Billing export",
            "pay-2 billing export again, https://x.io/PAY-2",
            "manual",
            Some("feat/pay-2-billing-export"),
        ),
        (
            "PAY-3",
            "Opraviť prihlásenie",
            "oprav prihlasenie cez SSO, vetva pay-3-oprav-prihl-senie",
            "started",
            Some("pay-3-oprav-prihl-senie"),
        ),
        (
            "PAY-4",
            "Dark mode settings",
            "dark modes for SETTINGS please (PAY-4)",
            "started",
            Some("pay-4-dark-mode-settings"),
        ),
        (
            "PAY-5",
            "Crash on upload",
            "the app does crash on upload, crash on UPLOAD!",
            "manual",
            Some("bugfix/crash"),
        ),
    ];
    for (i, (key, title, prompt, source, branch)) in specs.iter().enumerate() {
        let item = w.item(key, title, *NOW - 30 * DAY);
        w.case_on(
            "h1",
            prompt,
            item,
            source,
            *NOW - (10 - i as i64) * DAY,
            *branch,
        );
    }
    let got = load(&w.s, &Words, &opts(Split::All, vec![]), None).unwrap();
    let cases: Vec<&BenchCase> = got
        .cases
        .iter()
        .filter(|c| c.dataset == Dataset::A)
        .collect();
    assert_eq!(cases.len(), 10, "5 cases and their none-cases");
    for c in cases {
        let state = c.state.to_lowercase();
        let g = &c.guard;
        if let Some(k) = &g.key {
            assert!(
                !state.contains(&k.to_lowercase()),
                "{}: key in {state}",
                c.id
            );
        }
        if let Some(t) = &g.title {
            assert!(
                !state.contains(&t.to_lowercase()),
                "{}: title in {state}",
                c.id
            );
        }
        if let Some(b) = &g.branch {
            assert!(
                !state.contains(&b.to_lowercase()),
                "{}: branch in {state}",
                c.id
            );
        }
        if let Some(slug) = &g.slug {
            assert!(!state.contains(slug.as_str()), "{}: slug in {state}", c.id);
            let stems: std::collections::HashSet<String> = slug
                .split('-')
                .filter(|w| w.len() >= 3)
                .map(super::bm25::stem)
                .collect();
            for word in state.split(|ch: char| !ch.is_alphanumeric()) {
                assert!(
                    word.is_empty() || !stems.contains(&super::bm25::stem(word)),
                    "{}: slug word {word:?} in {state}",
                    c.id
                );
            }
        }
    }
}

// --- candidates, split, recall --------------------------------------------------

#[test]
fn candidates_are_org_scoped_time_bounded_live_and_capped() {
    let mut w = world();
    let at = *NOW - 10 * DAY;
    let truth = w.item("PAY-1", "The truth, updated long after", *NOW);
    let old = w.item("PAY-2", "Updated before the decision", at - DAY);
    let late = w.item("PAY-3", "Updated two days after", at + 2 * DAY);
    let gone = w.item("PAY-4", "Unavailable before the decision", at - DAY);
    w.s.conn_for_test()
        .execute(
            "UPDATE work_items SET unavailable_at = ?1 WHERE id = ?2",
            rusqlite::params![at - 1, gone],
        )
        .unwrap();
    let other = w.s.add_org("Other", None, false).unwrap().id;
    let t2 =
        w.s.add_tracker("jira", "Other", "https://other.atlassian.net")
            .unwrap()
            .id;
    w.s.set_tracker_org(t2, Some(other)).unwrap();
    let foreign = w.item_in(t2, "OTH-1", "Another org's item", at - DAY, false);
    w.case("fix the thing", truth, "manual", at);
    let got = load(&w.s, &Words, &opts(Split::All, vec![]), None).unwrap();
    let c = a_truths(&got)[0];
    let ids: Vec<&str> = c.candidates.iter().map(|x| x.id.as_str()).collect();
    assert!(
        ids.contains(&option_id(truth).as_str()),
        "the truth is always in"
    );
    assert!(ids.contains(&option_id(old).as_str()));
    for no in [late, gone, foreign] {
        assert!(!ids.contains(&option_id(no).as_str()), "{no} in {ids:?}");
    }
    assert_eq!(got.recall.bench_set.0, 0, "the truth was added, not found");

    // The cap: 60 eligible items give 50 candidates, the truth among them.
    let mut w = world();
    for i in 0..60 {
        w.item(
            &format!("PAY-{}", 100 + i),
            &format!("Item number {i}"),
            at - DAY - i,
        );
    }
    let truth = w.item("PAY-1", "Truth", at - 500 * DAY);
    w.case("fix the thing", truth, "manual", at);
    let got = load(&w.s, &Words, &opts(Split::All, vec![]), None).unwrap();
    let c = a_truths(&got)[0];
    assert_eq!(c.candidates.len(), MAX_CANDIDATES);
    assert!(c.candidates.iter().any(|x| x.id == option_id(truth)));
    assert_eq!(got.recall.bench_set.0, 1);
}

#[test]
fn a_none_case_is_its_case_without_the_truth() {
    let w = seeded();
    let got = load(&w.s, &Words, &opts(Split::All, vec![]), None).unwrap();
    for c in a_truths(&got) {
        let n = got
            .cases
            .iter()
            .find(|x| x.id == format!("{}n", c.id))
            .expect("a none-case");
        assert!(n.truth.is_none());
        assert_eq!(n.state, c.state);
        assert_eq!(n.dev, c.dev);
        assert_eq!(n.candidates.len() + 1, c.candidates.len());
        assert!(!n.candidates.iter().any(|x| Some(&x.id) == c.truth.as_ref()));
    }
}

#[test]
fn the_split_is_by_time_oldest_sixty_percent_dev() {
    let w = seeded();
    let got = load(&w.s, &Words, &opts(Split::All, vec![]), None).unwrap();
    let t = a_truths(&got);
    assert_eq!(t.len(), 10);
    assert_eq!(t.iter().filter(|c| c.dev).count(), 6);
    let newest_dev = t.iter().filter(|c| c.dev).map(|c| c.at).max().unwrap();
    let oldest_test = t.iter().filter(|c| !c.dev).map(|c| c.at).min().unwrap();
    assert!(newest_dev <= oldest_test);
    assert_eq!((got.sizes.a_dev.0, got.sizes.a_test.0), (6, 4));
    assert_eq!(Split::parse("test"), Some(Split::Test));
    assert_eq!(Split::parse("train"), None);
}

#[test]
fn recall_follows_the_nudge_fence_as_of_the_decision() {
    let mut w = world();
    let at = *NOW - 5 * DAY;
    // Mine, and linked on h1 before: in the fence.
    let fenced = w.item_in(w.jira, "PAY-1", "Fenced and mine", at - 20 * DAY, true);
    w.case("earlier work on it", fenced, "manual", at - 10 * DAY);
    w.case("again on the fenced one", fenced, "manual", at);
    // Mine, but never linked on the host before: dropped by the fence.
    let unfenced = w.item_in(
        w.jira,
        "PAY-2",
        "Mine but never on this host",
        at - 20 * DAY,
        true,
    );
    w.case("first time on this one", unfenced, "manual", at);
    let got = load(&w.s, &Words, &opts(Split::All, vec![]), None).unwrap();
    assert_eq!(got.recall.cases.0, 3);
    // Only the second link to `fenced` had an earlier link on the host.
    assert_eq!(got.recall.nudge_fence.0, 1);
    assert_eq!(got.recall.nudge_offered_truth.0, 1);
}

// --- providers and the report ---------------------------------------------------

#[tokio::test]
async fn bm25_finds_described_tickets_and_none_always_abstains() {
    let w = seeded();
    let o = opts(Split::All, vec![Provider::None, Provider::Bm25]);
    let loaded = load(&w.s, &Words, &o, None).unwrap();
    let outs = run_providers(&loaded, &o, None).await;
    let r = report(&loaded, &o, &outs);
    let a = &r.datasets[0];
    assert_eq!(a.dataset, "A");
    let none = a.providers.iter().find(|p| p.provider == "none").unwrap();
    assert_eq!(none.answered.0, 0);
    assert_eq!(none.coverage, Some(0.0));
    assert_eq!(none.abstention_quality, Some(1.0));
    let bm = a.providers.iter().find(|p| p.provider == "bm25").unwrap();
    assert!(bm.accuracy_on_answered.unwrap() >= 0.7, "{bm:?}");
    assert!(r.thresholds.bm25_abstain.is_some());
    assert!(a.diffs.is_empty(), "no pair of answering providers");
    // Every raw BM25 pick of a truth case is one of its candidates.
    for c in &loaded.cases {
        if let Some(p) = &outs[&Provider::Bm25][&c.id].pick {
            assert!(c.candidates.iter().any(|x| &x.id == p));
        }
    }
}

#[tokio::test]
async fn small_cells_show_as_under_five_and_are_not_judged() {
    let w = seeded();
    let o = opts(Split::Test, vec![Provider::None, Provider::Bm25]);
    let loaded = load(&w.s, &Words, &o, None).unwrap();
    let outs = run_providers(&loaded, &o, None).await;
    let r = report(&loaded, &o, &outs);
    let a = &r.datasets[0];
    assert_eq!(a.cases.0, 4);
    for c in &a.breakdown {
        assert!(!c.judged);
        if c.cases.0 < 5 {
            assert!(c.providers.is_empty(), "{c:?}");
            assert_eq!(
                serde_json::to_value(c.cases).unwrap(),
                serde_json::json!("<5")
            );
        }
    }
    let lines = r.lines().join("\n");
    assert!(lines.contains("not judged"), "{lines}");
    assert!(lines.contains("<5"), "{lines}");
}

#[tokio::test]
async fn the_report_holds_no_prompt_and_no_title() {
    let w = seeded();
    let o = opts(Split::All, vec![Provider::None, Provider::Bm25]);
    let loaded = load(&w.s, &Words, &o, None).unwrap();
    let outs = run_providers(&loaded, &o, None).await;
    let r = report(&loaded, &o, &outs);
    let json = serde_json::to_string(&r).unwrap();
    let lines = r.lines().join("\n");
    for out in [json, lines] {
        for secret in [
            "redirecting",
            "Billing export",
            "avatar",
            "invoice",
            "welcome",
        ] {
            assert!(
                !out.to_lowercase().contains(&secret.to_lowercase()),
                "{secret}"
            );
        }
    }
}

#[test]
fn thresholds_come_from_dev() {
    // Dev picks: right at scores 3, 2.6, 2.4, 2.2 and 2, wrong at 1; the
    // none-case at 1.5. At 2 dev has 5 answers, all right; at 1, 5 of 6.
    let mk = |id: &str, truth: Option<&str>, dev: bool| BenchCase {
        id: id.into(),
        dataset: Dataset::A,
        at: 0,
        org_id: None,
        tracker: "jira".into(),
        state: String::new(),
        candidates: vec![],
        truth: truth.map(String::from),
        dev,
        nl_prompt: "en",
        nl_title: "en",
        code: "none",
        guard: Default::default(),
    };
    let cases = vec![
        mk("d1", Some("i1"), true),
        mk("d2", Some("i1"), true),
        mk("d3", Some("i1"), true),
        mk("d4", None, true),
        mk("d5", Some("i1"), true),
        mk("d6", Some("i1"), true),
        mk("d7", Some("i1"), true),
        mk("t1", Some("i1"), false),
        mk("t2", Some("i1"), false),
    ];
    let out = |pick: &str, score: f64| Outcome {
        ran: true,
        pick: Some(pick.into()),
        score: Some(score),
        ..Default::default()
    };
    let bm: std::collections::HashMap<String, Outcome> = [
        ("d1", out("i1", 3.0)),
        ("d2", out("i1", 2.0)),
        ("d3", out("i2", 1.0)),
        ("d4", out("i2", 1.5)),
        ("d5", out("i1", 2.2)),
        ("d6", out("i1", 2.4)),
        ("d7", out("i1", 2.6)),
        ("t1", out("i1", 2.5)),
        ("t2", out("i2", 0.9)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let loaded = Loaded {
        schema_version: 99,
        cases,
        sizes: Default::default(),
        recall: Default::default(),
        org_names: Default::default(),
    };
    let o = opts(Split::Test, vec![Provider::Bm25]);
    let outs: Outcomes = [(Provider::Bm25, bm)].into_iter().collect();
    let r = report(&loaded, &o, &outs);
    assert_eq!(r.thresholds.bm25_abstain, Some(2.0));
    assert_eq!(r.thresholds.at_precision.get("bm25"), Some(&Some(2.0)));
    let p = &r.datasets[0].providers[0];
    // On test: t1 (2.5) answered and right, t2 (0.9) abstained.
    assert_eq!((p.answered.0, p.correct.0), (1, 1));
    assert_eq!(p.coverage, Some(0.5));
    assert_eq!(p.at_precision.coverage, Some(0.5));
    assert_eq!(p.at_precision.accuracy_on_answered, Some(1.0));
}

// --- Jev through the envelope --------------------------------------------------

const KEY: &str = "tsk_test_0123456789abcdefghijklmnopqrstuv";

/// Answers every question with its first option; records what it was sent.
#[derive(Default)]
struct FirstOption {
    calls: AtomicUsize,
    seen: Mutex<Vec<JevRequest>>,
}

#[async_trait::async_trait]
impl DecisionBackend for FirstOption {
    fn provider(&self) -> &'static str {
        crate::service::decide::PROVIDER_JEV
    }
    async fn ask(
        &self,
        _key: &Secret,
        _model: &str,
        req: &JevRequest,
        _timeout: Duration,
    ) -> Result<JevResponse, BackendError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(req.clone());
        let Question::Choice { criteria, .. } = &req.question else {
            return Err(BackendError::Unreadable("not a choice".into()));
        };
        let first = criteria.keys().next().unwrap().clone();
        Ok(serde_json::from_value(serde_json::json!({
            "model": "jev-1.13.0",
            "answers": { "q": { "type": "choice", "choice": first, "confidence": 0.8 } },
            "usage": { "input_tokens": 100, "output_tokens": 2 },
        }))
        .unwrap())
    }
}

#[tokio::test]
async fn jev_is_asked_only_through_the_gate() {
    let mut w = seeded();
    // A second org that never consents, with one case.
    let other = w.s.add_org("Other", None, false).unwrap().id;
    w.s.upsert_host("h2").unwrap();
    w.s.set_host_org("h2", Some(other)).unwrap();
    let local =
        w.s.create_local_work_item(Some("LOC-1"), "Local thing")
            .unwrap()
            .id;
    w.case_on(
        "h2",
        "work on the local thing",
        local,
        "manual",
        *NOW - DAY,
        None,
    );
    let acme = w.acme;
    let store = Arc::new(Mutex::new(w.s));
    let fake = Arc::new(FirstOption::default());
    let ctx = DecideCtx::new(Arc::clone(&store), fake.clone());
    let o = opts(Split::All, vec![Provider::Bm25, Provider::Jev]);
    let loaded = load(&store.lock().unwrap(), &Words, &o, None).unwrap();

    // With the defaults: nothing is sent, nothing is recorded.
    let outs = run_providers(&loaded, &o, Some(&ctx)).await;
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert!(outs[&Provider::Jev]
        .values()
        .all(|x| !x.ran && x.reason.as_deref() == Some("flag_off")));
    let runs = |s: &Arc<Mutex<Store>>| {
        s.lock()
            .unwrap()
            .list_decision_runs(&DecisionRunFilter {
                limit: 1000,
                ..Default::default()
            })
            .unwrap()
    };
    assert!(runs(&store).is_empty());
    let r = report(&loaded, &o, &outs);
    let jev = r.datasets[0]
        .providers
        .iter()
        .find(|p| p.provider == "jev")
        .unwrap();
    assert_eq!(jev.cases.0, 0);
    assert!(jev.skipped.contains_key("flag_off"));

    // Flag on and mode shadow, but no org consent: org_off, still nothing sent.
    {
        let s = store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_WORK_LINK, "shadow").unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    let outs = run_providers(&loaded, &o, Some(&ctx)).await;
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert!(outs[&Provider::Jev]
        .values()
        .all(|x| x.reason.as_deref() == Some("org_off")));

    // Acme consents: its cases are asked and recorded; Other's are skipped.
    store
        .lock()
        .unwrap()
        .set_org_jev_allowed(acme, true)
        .unwrap();
    let outs = run_providers(&loaded, &o, Some(&ctx)).await;
    let jev_outs = &outs[&Provider::Jev];
    let acme_cases: Vec<&BenchCase> = loaded
        .cases
        .iter()
        .filter(|c| c.org_id == Some(acme))
        .collect();
    let other_cases: Vec<&BenchCase> = loaded
        .cases
        .iter()
        .filter(|c| c.org_id == Some(other))
        .collect();
    assert!(!other_cases.is_empty());
    assert_eq!(fake.calls.load(Ordering::SeqCst), acme_cases.len());
    for c in &other_cases {
        assert_eq!(jev_outs[&c.id].reason.as_deref(), Some("org_off"));
    }
    for c in &acme_cases {
        let x = &jev_outs[&c.id];
        assert!(x.ran && x.reason.is_none(), "{x:?}");
        assert_eq!(x.input_tokens, 100);
        assert!(x.latency_ms.is_some());
    }
    let recorded = runs(&store);
    assert_eq!(recorded.len(), acme_cases.len());
    assert!(recorded.iter().all(|r| r.feature == "work_link"
        && r.subject_kind == SUBJECT_KIND
        && r.question_version == QUESTION_VERSION
        && r.mode == "shadow"
        && r.org_id == Some(acme)));
    // What was sent: redacted state, candidate ids plus "none", no truth key.
    for req in fake.seen.lock().unwrap().iter() {
        let body = req.state.to_string();
        assert!(!body.contains("PAY-"), "{body}");
        let Question::Choice { criteria, .. } = &req.question else {
            panic!("a choice");
        };
        assert!(criteria.contains_key(NONE_OPTION));
        assert!(criteria
            .keys()
            .all(|k| k == NONE_OPTION || k.starts_with('i')));
    }
    let r = report(&loaded, &o, &outs);
    let jev = r.datasets[0]
        .providers
        .iter()
        .find(|p| p.provider == "jev")
        .unwrap();
    assert!(jev.calls.0 > 0 && jev.input_tokens > 0);
    assert!(jev.latency_p95_ms.is_some());
    assert!(!r.datasets[0].diffs.is_empty(), "bm25 vs jev is compared");

    // The call cap.
    let capped = BenchOptions {
        max_calls: 2,
        ..o.clone()
    };
    let before = fake.calls.load(Ordering::SeqCst);
    let outs = run_providers(&loaded, &capped, Some(&ctx)).await;
    assert_eq!(fake.calls.load(Ordering::SeqCst) - before, 2);
    assert!(outs[&Provider::Jev]
        .values()
        .any(|x| x.reason.as_deref() == Some("max_calls")));
}

// --- D39 export and labels -------------------------------------------------------

#[tokio::test]
async fn export_then_labels_round_trip_into_dataset_h() {
    let mut w = seeded();
    for i in 0..6 {
        w.unlinked(
            &format!("something unlinked number {i} for PAY-3"),
            *NOW - (20 - i) * DAY,
        );
    }
    // A session with only a suggested link is not exported either.
    let sid = w.unlinked("has a suggestion", *NOW - DAY);
    let l =
        w.s.link_session_work(sid, WorkTarget::Key("PAY-1"), "manual")
            .unwrap();
    w.s.conn_for_test()
        .execute(
            "UPDATE work_links SET state = 'suggested' WHERE id = ?1",
            [l.id],
        )
        .unwrap();
    let o = opts(Split::All, vec![Provider::None, Provider::Bm25]);
    let rows = export_unlinked(&w.s, &o, 4).unwrap();
    assert_eq!(rows.len(), 4);
    assert!(rows
        .iter()
        .all(|r| r.label.is_none() && !r.candidates.is_empty()));
    assert!(
        rows.iter().all(|r| !r.prompt.contains("PAY-3")),
        "prompts are redacted"
    );
    assert!(rows.iter().all(|r| r.session_id != sid));
    assert!(export_unlinked(&w.s, &o, 0).is_err());

    // A person labels: one item, one "none", one outside, one left blank.
    let mut labeled = rows.clone();
    labeled[0].label = Some(labeled[0].candidates[0].id.clone());
    labeled[1].label = Some("none".into());
    labeled[2].label = Some("i999999".into());
    let jsonl: String = labeled
        .iter()
        .map(|r| serde_json::to_string(r).unwrap() + "\n")
        .collect();
    let back = parse_labels(&jsonl).unwrap();
    assert_eq!(back, labeled);
    assert!(parse_labels("{not json").is_err());

    let loaded = load(&w.s, &Words, &o, Some(&back)).unwrap();
    let s = &loaded.sizes;
    assert_eq!((s.h_records.0, s.h_labeled.0), (4, 3));
    assert_eq!(
        (s.h_cases.0, s.h_none_cases.0, s.h_label_outside.0),
        (1, 1, 1)
    );
    let outs = run_providers(&loaded, &o, None).await;
    let r = report(&loaded, &o, &outs);
    let h = r
        .datasets
        .iter()
        .find(|d| d.dataset == "H")
        .expect("H is reported");
    assert_eq!((h.cases.0, h.none_cases.0), (1, 1));
    assert_eq!(h.scope, "all labeled");
}

#[test]
fn options_are_bounded() {
    assert!(BenchOptions::new(Some(0), None, None, Split::All, vec![], None, *NOW).is_err());
    assert!(BenchOptions::new(
        Some(MAX_DAYS + 1),
        None,
        None,
        Split::All,
        vec![],
        None,
        *NOW
    )
    .is_err());
    assert!(BenchOptions::new(None, None, Some(0), Split::All, vec![], None, *NOW).is_err());
    let o = BenchOptions::new(None, None, None, Split::Test, vec![], None, *NOW).unwrap();
    assert_eq!(o.providers, vec![Provider::None, Provider::Bm25]);
    assert_eq!(o.max_calls, DEFAULT_MAX_CALLS);
    let o = BenchOptions::new(
        None,
        None,
        None,
        Split::Test,
        vec![Provider::Jev, Provider::Bm25, Provider::Jev],
        None,
        *NOW,
    )
    .unwrap();
    assert_eq!(o.providers, vec![Provider::Bm25, Provider::Jev]);
    assert_eq!(Provider::parse("jev"), Some(Provider::Jev));
    assert_eq!(Provider::parse("haiku"), None);
}
