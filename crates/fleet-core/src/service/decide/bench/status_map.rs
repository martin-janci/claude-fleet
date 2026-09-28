//! The offline `status_map` benchmark (test map card J3, phase 0):
//! `fleet-hub decide bench status-map`.
//!
//! **The cases** are labeled Asana sections, one JSON line each:
//! `{"section", "project_sections", "expect", "lang", "note"?, "org_id"?}` —
//! the section's name, its board's section names in board order (the
//! section among them), the right category (`todo`, `in_progress`, `done`
//! or `not_planned`), the language of the name (`en`, `sk`, `cs`, `de`,
//! `mixed`) and a note; a note containing `ambiguous` marks a case whose
//! right answer a reasonable person could dispute, reported apart. The
//! built-in set ([`FIXTURE`], `--fixture`) is synthetic: written by an LLM
//! (decision D43) and still to be spot-checked by the owner; `--labels FILE`
//! reads the owner's own set in the same format.
//!
//! Every name is normalised as the probe normalises it
//! ([`crate::service::trackers::asana::section_key`]: trimmed, lower case;
//! the board de-duplicated) and the Jev request is the adapter's own
//! ([`crate::service::decide::status_map::question_for`]).
//!
//! **Providers**: `none` (always abstains), `todo` (what fleet does today
//! with a section the rule cannot classify: it counts as to do), `rule` (the
//! keyword rule [`infer_section`], abstaining where it returns none) and
//! `jev` (one Choice through [`crate::service::decide::decide`], question
//! version [`QUESTION_VERSION`], subject `bench:<case>`, with the adapter's
//! confidence floor: `unsure` and an answer under the floor are
//! abstentions). A case whose org the gate refuses is skipped with that
//! fallback and nothing is sent for it; the built-in set has no org, so it
//! follows `decide.jev.unassigned`. `haiku` (D33) asks the same request —
//! the same redacted state and options — of `claude -p` on a host the
//! operator names ([`crate::service::decide::haiku`]), at the same floor;
//! an answer outside the options is an abstention counted as `invalid`.
//!
//! **No threshold is tuned on the set** — the rule is fixed and Jev runs at
//! the adapter's floor — so there is no dev/test split.
//!
//! The report holds counts and rates only: no section name.

use super::{bootstrap_acc_diff, f3, percentile, Calibration, Criterion, Paired, Verdict};
use crate::ipc_error::lock;
use crate::service::decide::haiku::{reason::OTHER_ORG, Haiku};
use crate::service::decide::status_map::{self as sm, MIN_CONFIDENCE, NO_RULE, UNSURE};
use crate::service::decide::{decide, gate_bench_at, DecideCtx, DecideRequest, Fallback, Feature};
use crate::service::trackers::asana::{infer_section, section_key};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The question's version, recorded on every Jev run of the benchmark.
pub const QUESTION_VERSION: &str = "status_map.bench.v1";
/// `decision_runs.subject_kind` of a benchmark call: gated without the
/// feature's mode, and kept out of the live breaker, budget and stats.
pub const SUBJECT_KIND: &str = crate::store::DECISION_BENCH_SUBJECT;
/// The built-in synthetic set (D43).
pub const FIXTURE: &str = include_str!("../../testdata/decide/status_map_sections.jsonl");
/// Where it lives in the repository.
pub const FIXTURE_PATH: &str =
    "crates/fleet-core/src/service/testdata/decide/status_map_sections.jsonl";
/// The categories a label takes.
pub const CATEGORIES: [&str; 4] = ["todo", "in_progress", "done", "not_planned"];
/// What a confusion matrix calls an abstention.
pub const ABSTAIN: &str = "abstain";
/// Jev calls one run makes at most, unless told otherwise.
pub const DEFAULT_MAX_CALLS: usize = 500;
/// A set (or cell) with fewer usable cases is not judged (test map §3).
pub const JUDGE_MIN_CASES: u64 = 200;
/// Bootstrap resamples and seed (the same as J1's).
pub const BOOTSTRAP_RESAMPLES: usize = super::work_link::BOOTSTRAP_RESAMPLES;
pub const BOOTSTRAP_SEED: u64 = super::work_link::BOOTSTRAP_SEED;

/// Card J3's registered acceptance (assist), test map §5.
pub const ACCEPT_DONE_PRECISION: f64 = 0.97;
pub const ACCEPT_ACCURACY: f64 = 0.90;
pub const ACCEPT_RULE_ABSTAINED_COVERAGE: f64 = 0.60;

/// A provider the benchmark asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Always abstains.
    None,
    /// Always `todo`: today's effective fallback.
    Todo,
    /// The keyword rule `infer_section`; abstains where it returns none.
    Rule,
    /// Jev, through the decision envelope (network).
    Jev,
    /// `claude -p --model haiku` on a named host (D33; network, through
    /// that host's Claude account).
    Haiku,
}

impl Provider {
    pub const ALL: [Provider; 5] = [
        Provider::None,
        Provider::Todo,
        Provider::Rule,
        Provider::Jev,
        Provider::Haiku,
    ];

    pub fn parse(s: &str) -> Option<Provider> {
        Provider::ALL.into_iter().find(|p| p.as_str() == s)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Provider::None => "none",
            Provider::Todo => "todo",
            Provider::Rule => "rule",
            Provider::Jev => "jev",
            Provider::Haiku => "haiku",
        }
    }

    /// Asks a model over the network (its latency, tokens and cost count).
    pub fn is_model(self) -> bool {
        matches!(self, Provider::Jev | Provider::Haiku)
    }
}

/// One labeled section, as the file holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionLabel {
    pub section: String,
    /// The board's section names in board order.
    #[serde(default)]
    pub project_sections: Vec<String>,
    /// `todo`, `in_progress`, `done` or `not_planned`.
    pub expect: String,
    /// `en`, `sk`, `cs`, `de`, `mixed` (anything else is its own cell).
    #[serde(default)]
    pub lang: String,
    /// Free text; `ambiguous` in it marks a disputable case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The org whose consent a Jev call needs (none: `decide.jev.unassigned`).
    /// Checked against the database before anything is sent
    /// ([`resolve_label_orgs`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    /// The Asana tracker the section is on: its org, read from the
    /// database, is the case's org (a different `org_id` is refused).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_id: Option<i64>,
}

/// Every label's org, from the DATABASE, before any case is sent (Jev's
/// consent gate and the haiku org fence trust it): a label naming its
/// `tracker_id` takes that tracker's org, and an `org_id` that differs is
/// refused; a label with only an `org_id` must name an org the database
/// knows, and when the section is on Asana trackers of the database, one
/// of them must be of that org. A label with neither has no org
/// (`decide.jev.unassigned`, a host with no org). An error names the row.
pub fn resolve_label_orgs(
    store: &crate::store::Store,
    labels: &mut [SectionLabel],
) -> Result<(), String> {
    let trackers = store.list_trackers().map_err(|e| e.message)?;
    let orgs: std::collections::HashSet<i64> = store
        .list_orgs()
        .map_err(|e| e.message)?
        .into_iter()
        .map(|o| o.id)
        .collect();
    let org_name = |o: Option<i64>| o.map_or_else(|| "none".to_string(), |o| o.to_string());
    for (i, l) in labels.iter_mut().enumerate() {
        let row = i + 1;
        if let Some(t) = l.tracker_id {
            let tr = trackers
                .iter()
                .find(|r| r.id == t)
                .ok_or_else(|| format!("row {row}: no tracker {t} in the database"))?;
            if l.org_id.is_some() && l.org_id != tr.org_id {
                return Err(format!(
                    "row {row}: org_id {} but tracker {t} is in org {}",
                    org_name(l.org_id),
                    org_name(tr.org_id)
                ));
            }
            l.org_id = tr.org_id;
            continue;
        }
        let Some(org) = l.org_id else {
            continue;
        };
        if !orgs.contains(&org) {
            return Err(format!("row {row}: no org {org} in the database"));
        }
        let Some(key) = section_key(&l.section) else {
            continue;
        };
        let on: Vec<&crate::store::TrackerRow> = trackers
            .iter()
            .filter(|r| r.provider == sm::PROVIDER && lists_section(r, &key))
            .collect();
        if !on.is_empty() && !on.iter().any(|r| r.org_id == Some(org)) {
            return Err(format!(
                "row {row}: org_id {org}, but the trackers that list this section are in                  org(s) {}; give the row its tracker_id",
                on.iter()
                    .map(|r| org_name(r.org_id))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    Ok(())
}

/// PURE: `row`'s stored config or its person's map names section `key`.
fn lists_section(row: &crate::store::TrackerRow, key: &str) -> bool {
    row.config.unmapped_sections.iter().any(|n| n == key)
        || row.config.section_map.contains_key(key)
        || row.settings.section_map.contains_key(key)
        || row
            .config
            .project_sections
            .iter()
            .any(|(_, names)| names.iter().any(|n| n == key))
}

/// PURE: the rows of a label file (JSON lines; blank lines and lines
/// starting with `//` skipped). A row with an unknown `expect` or an empty
/// section is an error naming its line.
pub fn parse_labels(jsonl: &str) -> Result<Vec<SectionLabel>, String> {
    let mut out = Vec::new();
    for (i, l) in jsonl.lines().enumerate() {
        let t = l.trim();
        if t.is_empty() || t.starts_with("//") {
            continue;
        }
        let row: SectionLabel =
            serde_json::from_str(t).map_err(|e| format!("line {}: {e}", i + 1))?;
        if !CATEGORIES.contains(&row.expect.as_str()) {
            return Err(format!(
                "line {}: expect must be one of {}, not {:?}",
                i + 1,
                CATEGORIES.join(", "),
                row.expect
            ));
        }
        if row.section.trim().is_empty() {
            return Err(format!("line {}: an empty section", i + 1));
        }
        out.push(row);
    }
    Ok(out)
}

/// One case, normalised as the probe would store it.
#[derive(Debug, Clone, PartialEq)]
pub struct SectionCase {
    /// `s<row number>` (1-based, over the rows read).
    pub id: String,
    /// The section's key (trimmed, lower case).
    pub key: String,
    /// The board's keys in order, de-duplicated.
    pub board: Vec<String>,
    pub expect: &'static str,
    pub lang: String,
    pub ambiguous: bool,
    pub org_id: Option<i64>,
    /// The keyword rule's category on the original name.
    pub rule: Option<&'static str>,
}

/// PURE: the cases of `labels`, and how many rows the probe would not have
/// kept (a name `section_key` refuses).
pub fn cases(labels: &[SectionLabel]) -> (Vec<SectionCase>, usize) {
    let mut out = Vec::new();
    let mut dropped = 0;
    for (i, l) in labels.iter().enumerate() {
        let Some(key) = section_key(&l.section) else {
            dropped += 1;
            continue;
        };
        let mut board: Vec<String> = Vec::new();
        for k in l.project_sections.iter().filter_map(|n| section_key(n)) {
            if !board.contains(&k) {
                board.push(k);
            }
        }
        let expect = CATEGORIES
            .into_iter()
            .find(|c| *c == l.expect)
            .unwrap_or("todo");
        let lang = if l.lang.trim().is_empty() {
            "unknown".to_string()
        } else {
            l.lang.trim().to_lowercase()
        };
        out.push(SectionCase {
            id: format!("s{}", i + 1),
            key,
            board,
            expect,
            lang,
            ambiguous: l
                .note
                .as_deref()
                .is_some_and(|n| n.to_lowercase().contains("ambiguous")),
            org_id: l.org_id,
            rule: infer_section(&l.section),
        });
    }
    (out, dropped)
}

// --- providers -------------------------------------------------------------------

/// What one provider did with one case.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    /// The provider was asked (false: skipped, see `reason`).
    pub ran: bool,
    /// The category it answers; `None` is an abstention.
    pub answer: Option<String>,
    /// Jev: the option it chose, `unsure` and under-the-floor answers
    /// included (they are abstentions in `answer`).
    pub model: Option<String>,
    pub confidence: Option<f64>,
    /// Why there is no usable outcome: a gate refusal (skipped), a failed
    /// call's fallback, or `max_calls`.
    pub reason: Option<String>,
    pub latency_ms: Option<i64>,
    pub input_tokens: i64,
    pub cost_microusd: i64,
    /// Haiku: it answered, but not with one of the options (an abstention).
    pub invalid: bool,
    /// Haiku: the call ran and its envelope reported no usage.
    pub usage_unknown: bool,
}

impl Outcome {
    pub fn usable(&self) -> bool {
        self.ran && self.reason.is_none()
    }

    fn answers(a: Option<&str>) -> Outcome {
        Outcome {
            ran: true,
            answer: a.map(str::to_string),
            ..Default::default()
        }
    }

    fn skipped(reason: &str) -> Outcome {
        Outcome {
            reason: Some(reason.to_string()),
            ..Default::default()
        }
    }
}

/// PURE: an offline provider's outcome (`jev` and `haiku` are not
/// offline: skipped).
pub fn offline_outcome(p: Provider, c: &SectionCase) -> Outcome {
    match p {
        Provider::None => Outcome::answers(None),
        Provider::Todo => Outcome::answers(Some("todo")),
        Provider::Rule => Outcome::answers(c.rule),
        Provider::Jev | Provider::Haiku => Outcome::skipped("no_backend"),
    }
}

/// Ask Jev about each case through the envelope, one call at a time, with
/// the adapter's own request and confidence floor. A case whose org the
/// gate refuses is skipped with the gate's fallback (nothing sent, nothing
/// recorded); every call made is recorded in `decision_runs` (feature
/// `status_map`, subject `bench:<case>`, baseline the rule's category or
/// `none`). At most `max_calls` calls; the rest are skipped as `max_calls`.
pub async fn run_jev(ctx: &DecideCtx, cases: &[SectionCase], max_calls: usize) -> Vec<Outcome> {
    let mut out = Vec::with_capacity(cases.len());
    let mut calls = 0usize;
    for c in cases {
        let gated = match lock(&ctx.store) {
            Ok(s) => gate_bench_at(&s, Feature::StatusMap, c.org_id, ctx.now()),
            Err(_) => Err(Fallback::FlagOff),
        };
        if let Err(f) = gated {
            out.push(Outcome::skipped(f.as_str()));
            continue;
        }
        if calls >= max_calls {
            out.push(Outcome::skipped("max_calls"));
            continue;
        }
        calls += 1;
        let res = decide(
            ctx,
            DecideRequest {
                feature: Feature::StatusMap,
                subject_kind: SUBJECT_KIND.into(),
                subject_id: c.id.clone(),
                org_id: c.org_id,
                request: sm::question_for(&c.key, &c.board),
                baseline: Some(c.rule.unwrap_or(NO_RULE).to_string()),
                question_version: QUESTION_VERSION.into(),
                min_confidence: Some(MIN_CONFIDENCE),
            },
        )
        .await;
        let run = res.run_id.and_then(|id| {
            lock(&ctx.store)
                .ok()
                .and_then(|s| s.get_decision_run(id).ok().flatten())
        });
        let mut o = Outcome {
            ran: res.mode.is_some(),
            // Under the floor is the adapter's abstention, not a failure.
            reason: res
                .fallback
                .filter(|f| *f != Fallback::LowConfidence)
                .map(|f| f.as_str().to_string()),
            latency_ms: run.as_ref().and_then(|r| r.latency_ms),
            input_tokens: run.as_ref().map(|r| r.input_tokens).unwrap_or_default(),
            cost_microusd: run.as_ref().map(|r| r.cost_microusd).unwrap_or_default(),
            ..Default::default()
        };
        if let Some(a) = &res.answer {
            o.model = Some(a.value.clone());
            o.confidence = a.confidence;
        }
        if let Some(a) = res.usable() {
            o.answer = (a.value != UNSURE && CATEGORIES.contains(&a.value.as_str()))
                .then(|| a.value.clone());
        }
        out.push(o);
    }
    out
}

/// Ask `claude -p` (D33) about each case, one call at a time on the
/// configured host, with the adapter's own request — the same redacted
/// state and options Jev gets — and the same confidence floor: `unsure`,
/// an answer under the floor and an answer outside the options
/// (`invalid`) are abstentions; a failed call (timeout, SSH, `claude`) is
/// skipped with its reason. An answer without a confidence stands (no
/// floor to apply) and is left out of calibration. A case whose org is not
/// the host's is skipped as `other_org` and nothing is sent for it (the
/// built-in set has no org: it needs a host with no org). At most
/// `max_calls` calls; nothing is recorded in `decision_runs`.
pub async fn run_haiku(h: &Haiku<'_>, cases: &[SectionCase], max_calls: usize) -> Vec<Outcome> {
    let mut out = Vec::with_capacity(cases.len());
    let mut calls = 0usize;
    for c in cases {
        // Never across the org boundary: a case goes only to a host of its
        // own org (no org on both sides is the same).
        if !h.may_ask(c.org_id) {
            out.push(Outcome::skipped(OTHER_ORG));
            continue;
        }
        if calls >= max_calls {
            out.push(Outcome::skipped("max_calls"));
            continue;
        }
        let r = h.ask(&sm::question_for(&c.key, &c.board)).await;
        calls += usize::from(r.ran);
        let mut o = Outcome {
            ran: r.ran,
            reason: r.error.map(str::to_string),
            latency_ms: r.latency_ms,
            input_tokens: r.input_tokens.unwrap_or_default(),
            cost_microusd: r.cost_microusd.unwrap_or_default(),
            invalid: r.invalid,
            usage_unknown: r.ran && r.error.is_none() && r.input_tokens.is_none(),
            model: r.choice.clone(),
            confidence: r.confidence,
            ..Default::default()
        };
        if let Some(choice) = r.choice.filter(|_| o.reason.is_none()) {
            let floor_ok = r.confidence.is_none_or(|x| x + 1e-9 >= MIN_CONFIDENCE);
            o.answer = (choice != UNSURE && CATEGORIES.contains(&choice.as_str()) && floor_ok)
                .then_some(choice);
        }
        out.push(o);
    }
    out
}

/// Every provider's outcome, in the order of the cases.
pub type Outcomes = BTreeMap<Provider, Vec<Outcome>>;

/// Run the offline providers, and Jev when asked for (with `jev` given;
/// without it every case is skipped as `no_backend`).
pub async fn run_providers(
    cases: &[SectionCase],
    providers: &[Provider],
    jev: Option<&DecideCtx>,
    max_calls: usize,
) -> Outcomes {
    run_providers_with(cases, providers, jev, None, max_calls).await
}

/// [`run_providers`], and the haiku baseline when asked for (with `haiku`
/// given; without it every case is skipped as `no_backend`). `max_calls`
/// bounds each network provider's calls.
pub async fn run_providers_with(
    cases: &[SectionCase],
    providers: &[Provider],
    jev: Option<&DecideCtx>,
    haiku: Option<&Haiku<'_>>,
    max_calls: usize,
) -> Outcomes {
    let mut all = Outcomes::new();
    for &p in providers {
        let v = match (p, jev, haiku) {
            (Provider::Jev, Some(ctx), _) => run_jev(ctx, cases, max_calls).await,
            (Provider::Haiku, _, Some(h)) => run_haiku(h, cases, max_calls).await,
            _ => cases.iter().map(|c| offline_outcome(p, c)).collect(),
        };
        all.insert(p, v);
    }
    all
}

// --- metrics -----------------------------------------------------------------------

/// Accuracy and coverage over a slice of the cases.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Slice {
    pub cases: u64,
    pub answered: u64,
    pub correct: u64,
    pub accuracy_on_answered: Option<f64>,
    pub coverage: Option<f64>,
}

fn ratio(k: u64, n: u64) -> Option<f64> {
    (n > 0).then(|| ((k as f64 / n as f64) * 1000.0).round() / 1000.0)
}

impl Slice {
    fn add(&mut self, expect: &str, answer: Option<&str>) {
        self.cases += 1;
        if let Some(a) = answer {
            self.answered += 1;
            self.correct += u64::from(a == expect);
        }
    }

    fn finish(mut self) -> Slice {
        self.accuracy_on_answered = ratio(self.correct, self.answered);
        self.coverage = ratio(self.answered, self.cases);
        self
    }
}

/// One provider's numbers over a set of cases.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProviderMetrics {
    pub provider: &'static str,
    /// Cases with a usable outcome.
    pub cases: u64,
    /// Cases with none, by reason.
    pub skipped: BTreeMap<String, u64>,
    /// Every usable case.
    pub all: Slice,
    /// The cases the keyword rule abstains on (today they count as to do).
    pub rule_abstained: Slice,
    /// Answered as a map would apply them (`not_planned` → `done`).
    pub applied_accuracy: Option<f64>,
    /// Answers that apply as `done` (`done` or `not_planned`).
    pub done_answers: u64,
    /// The card's headline: of the answers that apply as `done`, the share
    /// whose truth applies as `done` too. A wrong one hides live work.
    pub done_precision: Option<f64>,
    /// Of the answers that are exactly `done`, the share labeled `done`.
    pub done_precision_strict: Option<f64>,
    /// truth → answer (or `abstain`) → count.
    pub confusion: BTreeMap<String, BTreeMap<String, u64>>,
    /// Jev / haiku: every valid category answer with a confidence (under
    /// the floor too; `unsure` left out) against whether it was right.
    pub calibration: Calibration,
    pub calls: u64,
    /// Haiku: answers outside the options (counted as abstentions).
    pub invalid: u64,
    /// Haiku: calls whose envelope reported no tokens or cost (the totals
    /// leave them out).
    pub usage_unknown: u64,
    pub latency_p50_ms: Option<i64>,
    pub latency_p95_ms: Option<i64>,
    pub input_tokens: i64,
    pub cost_microusd: i64,
}

fn applied(c: &str) -> Option<&'static str> {
    sm::applied_category(c)
}

/// PURE: one provider's metrics over `rows`.
pub fn provider_metrics(p: Provider, rows: &[(&SectionCase, &Outcome)]) -> ProviderMetrics {
    let mut skipped: BTreeMap<String, u64> = BTreeMap::new();
    let (mut all, mut abst) = (Slice::default(), Slice::default());
    let (mut app_n, mut app_k) = (0u64, 0u64);
    let (mut done_n, mut done_k, mut strict_n, mut strict_k) = (0u64, 0u64, 0u64, 0u64);
    let mut confusion: BTreeMap<String, BTreeMap<String, u64>> = CATEGORIES
        .iter()
        .map(|c| (c.to_string(), BTreeMap::new()))
        .collect();
    let mut cal = Vec::new();
    let mut lat = Vec::new();
    let (mut calls, mut tokens, mut cost) = (0u64, 0i64, 0i64);
    let (mut invalid, mut usage_unknown) = (0u64, 0u64);
    for (c, o) in rows {
        if p.is_model() && o.ran {
            calls += 1;
            lat.extend(o.latency_ms);
            tokens += o.input_tokens;
            cost += o.cost_microusd;
            usage_unknown += u64::from(o.usage_unknown);
        }
        if !o.usable() {
            *skipped
                .entry(o.reason.clone().unwrap_or_else(|| "not_run".into()))
                .or_default() += 1;
            continue;
        }
        invalid += u64::from(o.invalid);
        let a = o.answer.as_deref();
        all.add(c.expect, a);
        if c.rule.is_none() {
            abst.add(c.expect, a);
        }
        *confusion
            .entry(c.expect.to_string())
            .or_default()
            .entry(a.unwrap_or(ABSTAIN).to_string())
            .or_default() += 1;
        if let Some(a) = a {
            app_n += 1;
            app_k += u64::from(applied(a) == applied(c.expect));
            if applied(a) == Some("done") {
                done_n += 1;
                done_k += u64::from(applied(c.expect) == Some("done"));
            }
            if a == "done" {
                strict_n += 1;
                strict_k += u64::from(c.expect == "done");
            }
        }
        if let (Some(m), Some(conf)) = (o.model.as_deref(), o.confidence) {
            if CATEGORIES.contains(&m) {
                cal.push((conf, m == c.expect));
            }
        }
    }
    ProviderMetrics {
        provider: p.as_str(),
        cases: all.cases,
        skipped,
        all: all.finish(),
        rule_abstained: abst.finish(),
        applied_accuracy: ratio(app_k, app_n),
        done_answers: done_n,
        done_precision: ratio(done_k, done_n),
        done_precision_strict: ratio(strict_k, strict_n),
        confusion,
        calibration: Calibration::of(&cal),
        calls,
        invalid,
        usage_unknown,
        latency_p50_ms: percentile(&lat, 50.0),
        latency_p95_ms: percentile(&lat, 95.0),
        input_tokens: tokens,
        cost_microusd: cost,
    }
}

/// One provider in one breakdown cell.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CellMetrics {
    pub provider: &'static str,
    pub accuracy_on_answered: Option<f64>,
    pub coverage: Option<f64>,
    pub rule_abstained_coverage: Option<f64>,
    pub done_precision: Option<f64>,
    pub calibration: Calibration,
}

/// One breakdown cell: a language, or the ambiguous / clear labels.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cell {
    /// `lang` or `labels`.
    pub dimension: &'static str,
    pub value: String,
    pub cases: u64,
    /// Of them, the rule abstains on.
    pub rule_abstained: u64,
    /// At least [`JUDGE_MIN_CASES`] cases.
    pub judged: bool,
    pub providers: Vec<CellMetrics>,
}

/// An accuracy-on-answered difference, with its bootstrap interval.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Diff {
    /// `all` or `rule_abstained`.
    pub scope: &'static str,
    pub a: &'static str,
    pub b: &'static str,
    /// Cases both have a usable outcome for.
    pub cases: u64,
    pub diff: Option<f64>,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
    /// `a`, `b`, `no difference` (the interval crosses 0) or `not
    /// comparable`.
    pub better: String,
}

/// One provider's verdicts on card J3's acceptance.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Acceptance {
    pub provider: &'static str,
    pub criteria: Vec<Criterion>,
    /// Every criterion passed → PASS; any failed → FAIL; else NOT JUDGED.
    pub overall: Verdict,
}

/// What was read.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Sizes {
    pub rows: u64,
    /// Rows whose name the probe would not keep (empty, too long, …).
    pub dropped: u64,
    pub cases: u64,
    pub ambiguous: u64,
    /// Cases the keyword rule abstains on.
    pub rule_abstained: u64,
    pub by_lang: BTreeMap<String, u64>,
    pub by_expect: BTreeMap<String, u64>,
}

/// The whole benchmark.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub benchmark: &'static str,
    pub question_version: &'static str,
    /// The adapter's question the requests are built from.
    pub adapter_question: &'static str,
    pub min_confidence: f64,
    /// `fixture` (synthetic, D43) or `labels`.
    pub source: &'static str,
    pub sizes: Sizes,
    pub providers: Vec<&'static str>,
    pub metrics: Vec<ProviderMetrics>,
    pub diffs: Vec<Diff>,
    pub breakdown: Vec<Cell>,
    pub acceptance: Vec<Acceptance>,
    pub notes: Vec<String>,
}

/// The haiku line of card J3: "beats `claude -p haiku`, or ties it at
/// under 1/10 of its latency".
pub const HAIKU_CRITERION: &str = "beats claude -p haiku, or ties it at < 1/10 of its latency";
/// "Ties at under 1/10 of its latency": the provider's p50 times this is
/// under haiku's p50.
pub const HAIKU_LATENCY_FACTOR: i64 = 10;

/// PURE: card J3's haiku line for provider `p`, paired with `haiku` over
/// the sections where the keyword rule abstains (the ones the adapter asks
/// in assist) that both answered usably. Accuracy on answered, `p` minus
/// haiku, with its bootstrap interval: above 0 **beats** (PASS); across 0
/// is a **tie**, which passes when `p`'s p50 latency over the same cases
/// is under a tenth of haiku's (an offline provider's is 0) and fails
/// otherwise; below 0 FAILS. NOT JUDGED when haiku did not run, when it is
/// `p` itself, or under [`JUDGE_MIN_CASES`] paired cases — the reason is
/// in `measured`.
pub fn haiku_criterion(p: Provider, cases: &[SectionCase], outs: &Outcomes) -> Criterion {
    let not = |why: String| Criterion::new(HAIKU_CRITERION, why, Verdict::NotJudged);
    if p == Provider::Haiku {
        return not("this is the haiku baseline".into());
    }
    let (Some(op), Some(oh)) = (outs.get(&p), outs.get(&Provider::Haiku)) else {
        return not(
            "claude -p haiku was not run (--provider haiku --haiku-host ALIAS, D33)".into(),
        );
    };
    let paired: Vec<usize> = (0..cases.len())
        .filter(|&i| cases[i].rule.is_none() && op[i].usable() && oh[i].usable())
        .collect();
    let n = paired.len() as u64;
    if n == 0 {
        return not("no rule-abstained section both answered usably".into());
    }
    let obs: Vec<Paired> = paired
        .iter()
        .map(|&i| {
            let (xa, xb) = (op[i].answer.as_deref(), oh[i].answer.as_deref());
            Paired {
                a_answered: xa.is_some(),
                a_correct: xa == Some(cases[i].expect),
                b_answered: xb.is_some(),
                b_correct: xb == Some(cases[i].expect),
            }
        })
        .collect();
    let cov = |o: &[Outcome]| {
        ratio(
            paired.iter().filter(|&&i| o[i].answer.is_some()).count() as u64,
            n,
        )
    };
    let acc = |o: &[Outcome]| {
        let answered: Vec<usize> = paired
            .iter()
            .copied()
            .filter(|&i| o[i].answer.is_some())
            .collect();
        ratio(
            answered
                .iter()
                .filter(|&&i| o[i].answer.as_deref() == Some(cases[i].expect))
                .count() as u64,
            answered.len() as u64,
        )
    };
    let p50 = |o: &[Outcome], model: bool| {
        if model {
            percentile(
                &paired
                    .iter()
                    .filter_map(|&i| o[i].latency_ms)
                    .collect::<Vec<_>>(),
                50.0,
            )
        } else {
            Some(0)
        }
    };
    let (lp, lh) = (p50(op, p.is_model()), p50(oh, true));
    let ci = bootstrap_acc_diff(&obs, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED);
    let judged = n >= JUDGE_MIN_CASES;
    let ms = |x: Option<i64>| x.map(|v| format!("{v} ms")).unwrap_or_else(|| "-".into());
    let r3 = |x: f64| (x * 1000.0).round() / 1000.0;
    let (verdict, what) = match ci {
        None => (Verdict::NotJudged, "not comparable (no paired answers)"),
        Some((_, lo, _)) if lo > 0.0 => (Verdict::Pass, "beats haiku"),
        Some((_, _, hi)) if hi < 0.0 => (Verdict::Fail, "worse than haiku"),
        Some(_) => match (lp, lh) {
            (Some(a), Some(b)) if a * HAIKU_LATENCY_FACTOR < b => {
                (Verdict::Pass, "ties haiku at < 1/10 of its latency")
            }
            (Some(_), Some(_)) => (Verdict::Fail, "ties haiku at ≥ 1/10 of its latency"),
            _ => (Verdict::NotJudged, "ties haiku; a latency is unknown"),
        },
    };
    let verdict = if judged { verdict } else { Verdict::NotJudged };
    let small = if judged {
        String::new()
    } else {
        format!(" (n {n} < {JUDGE_MIN_CASES}: not judged)")
    };
    Criterion::new(
        HAIKU_CRITERION,
        format!(
            "where the rule abstains, acc@ans {} vs haiku {} (coverage {} vs {}): {} [{}, {}] over {n} paired cases; p50 {} vs {} → {what}{small}",
            f3(acc(op)),
            f3(acc(oh)),
            f3(cov(op)),
            f3(cov(oh)),
            f3(ci.map(|x| r3(x.0))),
            f3(ci.map(|x| r3(x.1))),
            f3(ci.map(|x| r3(x.2))),
            ms(lp),
            ms(lh),
        ),
        verdict,
    )
}

/// PURE: card J3's acceptance for one provider's metrics, with its haiku
/// line ([`haiku_criterion`]). Not judged under [`JUDGE_MIN_CASES`] usable
/// cases.
pub fn acceptance(m: &ProviderMetrics, haiku: Criterion) -> Acceptance {
    let judged = m.cases >= JUDGE_MIN_CASES;
    let small = if judged {
        String::new()
    } else {
        format!(" (n {} < {JUDGE_MIN_CASES}: not judged)", m.cases)
    };
    let ra = &m.rule_abstained;
    let criteria = vec![
        Criterion::new(
            format!("done precision ≥ {ACCEPT_DONE_PRECISION}"),
            format!(
                "{} over {} answers that apply as done{small}",
                f3(m.done_precision),
                m.done_answers
            ),
            Verdict::at_least(m.done_precision, ACCEPT_DONE_PRECISION, judged),
        ),
        Criterion::new(
            format!("accuracy on answered ≥ {ACCEPT_ACCURACY} where the rule abstains"),
            format!(
                "{} over {} answered of {} rule-abstained sections{small}",
                f3(ra.accuracy_on_answered),
                ra.answered,
                ra.cases
            ),
            Verdict::at_least(
                ra.accuracy_on_answered,
                ACCEPT_ACCURACY,
                judged && ra.cases > 0,
            ),
        ),
        Criterion::new(
            format!("coverage ≥ {ACCEPT_RULE_ABSTAINED_COVERAGE} of rule-abstained sections"),
            format!("{}{small}", f3(ra.coverage)),
            Verdict::at_least(
                ra.coverage,
                ACCEPT_RULE_ABSTAINED_COVERAGE,
                judged && ra.cases > 0,
            ),
        ),
        haiku,
    ];
    let overall = Verdict::all(&criteria.iter().map(|c| c.verdict).collect::<Vec<_>>());
    Acceptance {
        provider: m.provider,
        criteria,
        overall,
    }
}

fn diff_of(
    scope: &'static str,
    a: Provider,
    b: Provider,
    cases: &[SectionCase],
    outs: &Outcomes,
    keep: &dyn Fn(&SectionCase) -> bool,
) -> Diff {
    let (oa, ob) = (&outs[&a], &outs[&b]);
    let obs: Vec<Paired> = cases
        .iter()
        .enumerate()
        .filter(|(_, c)| keep(c))
        .filter(|(i, _)| oa[*i].usable() && ob[*i].usable())
        .map(|(i, c)| {
            let (xa, xb) = (oa[i].answer.as_deref(), ob[i].answer.as_deref());
            Paired {
                a_answered: xa.is_some(),
                a_correct: xa == Some(c.expect),
                b_answered: xb.is_some(),
                b_correct: xb == Some(c.expect),
            }
        })
        .collect();
    let ci = bootstrap_acc_diff(&obs, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED);
    let r3 = |x: f64| (x * 1000.0).round() / 1000.0;
    Diff {
        scope,
        a: a.as_str(),
        b: b.as_str(),
        cases: obs.len() as u64,
        diff: ci.map(|x| r3(x.0)),
        lo: ci.map(|x| r3(x.1)),
        hi: ci.map(|x| r3(x.2)),
        better: match ci {
            Some((_, lo, _)) if lo > 0.0 => a.as_str().to_string(),
            Some((_, _, hi)) if hi < 0.0 => b.as_str().to_string(),
            Some(_) => "no difference".to_string(),
            None => "not comparable (no paired answers)".to_string(),
        },
    }
}

/// PURE: the report. `rows` is how many label rows were read, `dropped`
/// how many of them became no case; `synthetic` marks the built-in set.
pub fn report(
    cases: &[SectionCase],
    outs: &Outcomes,
    rows: usize,
    dropped: usize,
    synthetic: bool,
) -> Report {
    let providers: Vec<Provider> = outs.keys().copied().collect();
    let rows_of =
        |p: Provider, keep: &dyn Fn(&SectionCase) -> bool| -> Vec<(&SectionCase, &Outcome)> {
            cases
                .iter()
                .zip(outs[&p].iter())
                .filter(|(c, _)| keep(c))
                .collect()
        };
    let mut sizes = Sizes {
        rows: rows as u64,
        dropped: dropped as u64,
        cases: cases.len() as u64,
        ..Default::default()
    };
    for c in cases {
        sizes.ambiguous += u64::from(c.ambiguous);
        sizes.rule_abstained += u64::from(c.rule.is_none());
        *sizes.by_lang.entry(c.lang.clone()).or_default() += 1;
        *sizes.by_expect.entry(c.expect.to_string()).or_default() += 1;
    }
    let metrics: Vec<ProviderMetrics> = providers
        .iter()
        .map(|&p| provider_metrics(p, &rows_of(p, &|_| true)))
        .collect();

    let answering: Vec<Provider> = providers
        .iter()
        .copied()
        .filter(|p| *p != Provider::None)
        .collect();
    let mut diffs = Vec::new();
    for (i, &a) in answering.iter().enumerate() {
        for &b in &answering[i + 1..] {
            diffs.push(diff_of("all", b, a, cases, outs, &|_| true));
            if a != Provider::Rule && b != Provider::Rule {
                diffs.push(diff_of("rule_abstained", b, a, cases, outs, &|c| {
                    c.rule.is_none()
                }));
            }
        }
    }

    let mut breakdown = Vec::new();
    let mut cell = |dimension: &'static str, value: String, keep: &dyn Fn(&SectionCase) -> bool| {
        let n = cases.iter().filter(|c| keep(c)).count() as u64;
        if n == 0 {
            return;
        }
        let per = providers
            .iter()
            .map(|&p| {
                let m = provider_metrics(p, &rows_of(p, keep));
                CellMetrics {
                    provider: p.as_str(),
                    accuracy_on_answered: m.all.accuracy_on_answered,
                    coverage: m.all.coverage,
                    rule_abstained_coverage: m.rule_abstained.coverage,
                    done_precision: m.done_precision,
                    calibration: m.calibration,
                }
            })
            .collect();
        breakdown.push(Cell {
            dimension,
            value,
            cases: n,
            rule_abstained: cases.iter().filter(|c| keep(c) && c.rule.is_none()).count() as u64,
            judged: n >= JUDGE_MIN_CASES,
            providers: per,
        });
    };
    let mut langs: Vec<(&String, &u64)> = sizes.by_lang.iter().collect();
    langs.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (lang, _) in langs {
        let l = lang.clone();
        cell("lang", lang.clone(), &move |c: &SectionCase| c.lang == l);
    }
    cell("labels", "clear".into(), &|c: &SectionCase| !c.ambiguous);
    cell("labels", "ambiguous".into(), &|c: &SectionCase| c.ambiguous);

    let acceptance_rows: Vec<Acceptance> = providers
        .iter()
        .zip(&metrics)
        .filter(|(p, _)| **p != Provider::None)
        .map(|(&p, m)| acceptance(m, haiku_criterion(p, cases, outs)))
        .collect();

    let mut notes = vec![
        "no threshold is tuned on this set (the rule is fixed; jev runs at the adapter's confidence floor), so there is no dev/test split".to_string(),
        "done precision counts answers that APPLY as done (done or not_planned: a section map stores not_planned as done) against labels that apply as done — a wrong one hides live work; the strict column counts `done` alone".to_string(),
    ];
    if synthetic {
        notes.push(format!(
            "the built-in set is synthetic ({FIXTURE_PATH}): LLM-written (D43), not yet spot-checked by the owner — its verdicts are indicative; the card's gate is the owner's hand set (--labels)"
        ));
    }
    if providers.contains(&Provider::Jev) && cases.iter().all(|c| c.org_id.is_none()) {
        notes.push("no case has an org: a jev call's consent is decide.jev.unassigned".to_string());
    }
    Report {
        benchmark: "status_map (J3, phase 0, offline)",
        question_version: QUESTION_VERSION,
        adapter_question: sm::QUESTION_VERSION,
        min_confidence: MIN_CONFIDENCE,
        source: if synthetic { "fixture" } else { "labels" },
        sizes,
        providers: providers.iter().map(|p| p.as_str()).collect(),
        metrics,
        diffs,
        breakdown,
        acceptance: acceptance_rows,
        notes,
    }
}

// --- lines -------------------------------------------------------------------------

fn f2(x: Option<f64>) -> String {
    x.map(|v| format!("{v:.2}")).unwrap_or_else(|| "-".into())
}

impl Report {
    /// The report as lines for a terminal.
    pub fn lines(&self) -> Vec<String> {
        let s = &self.sizes;
        let join = |m: &BTreeMap<String, u64>| {
            m.iter()
                .map(|(k, v)| format!("{k} {v}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut v = vec![
            format!(
                "benchmark {} — question {} (the adapter's {}, floor {})",
                self.benchmark, self.question_version, self.adapter_question, self.min_confidence
            ),
            format!(
                "source: {}; {} rows, {} cases ({} dropped by the probe's name check); {} ambiguous; the rule abstains on {}",
                self.source, s.rows, s.cases, s.dropped, s.ambiguous, s.rule_abstained
            ),
            format!("by language: {}", join(&s.by_lang)),
            format!("by label: {}", join(&s.by_expect)),
            format!("providers: {}", self.providers.join(", ")),
            String::new(),
            format!(
                "  {:<5} {:>5} {:>8} {:>8} {:>8} {:>15} {:>9} {:>14} {:>7} {:>11} {:>7} {:>10}  skipped",
                "",
                "cases",
                "answered",
                "acc@ans",
                "coverage",
                "rule-abst cov",
                "applied",
                "done prec (n)",
                "strict",
                "p50/p95 ms",
                "tokens",
                "cost"
            ),
        ];
        for m in &self.metrics {
            let skipped = if m.skipped.is_empty() {
                "-".to_string()
            } else {
                join(&m.skipped)
            };
            v.push(format!(
                "  {:<5} {:>5} {:>8} {:>8} {:>8} {:>15} {:>9} {:>14} {:>7} {:>11} {:>7} {:>10}  {}",
                m.provider,
                m.cases,
                m.all.answered,
                f3(m.all.accuracy_on_answered),
                f3(m.all.coverage),
                format!(
                    "{} ({})",
                    f3(m.rule_abstained.coverage),
                    f3(m.rule_abstained.accuracy_on_answered)
                ),
                f3(m.applied_accuracy),
                format!("{} ({})", f3(m.done_precision), m.done_answers),
                f3(m.done_precision_strict),
                match (m.latency_p50_ms, m.latency_p95_ms) {
                    (Some(a), Some(b)) => format!("{a}/{b}"),
                    _ => "-".into(),
                },
                m.input_tokens,
                crate::service::decide::fmt_usd(m.cost_microusd),
                skipped
            ));
        }
        v.push(
            "  (rule-abst cov: coverage (accuracy on answered) where the keyword rule abstains; applied: accuracy with not_planned counted as done)"
                .into(),
        );
        for m in self.metrics.iter().filter(|m| m.calibration.n.0 > 0) {
            v.push(format!(
                "  calibration {}: {} (10 equal-width bins; every category answer with a confidence)",
                m.provider,
                m.calibration.line()
            ));
        }
        for m in self
            .metrics
            .iter()
            .filter(|m| m.invalid > 0 || m.usage_unknown > 0)
        {
            v.push(format!(
                "  {}: {} answers outside the options (invalid: counted as abstentions); {} calls reported no usage (the tokens and cost leave them out)",
                m.provider, m.invalid, m.usage_unknown
            ));
        }
        for m in &self.metrics {
            if m.provider == Provider::None.as_str() || m.cases == 0 {
                continue;
            }
            v.push(format!(
                "  confusion {} (truth → {} / {}):",
                m.provider,
                CATEGORIES.join(" "),
                ABSTAIN
            ));
            for (truth, row) in &m.confusion {
                let cells: Vec<String> = CATEGORIES
                    .iter()
                    .chain(std::iter::once(&ABSTAIN))
                    .map(|a| format!("{:>5}", row.get(*a).copied().unwrap_or(0)))
                    .collect();
                v.push(format!("    {truth:<12}{}", cells.join(" ")));
            }
        }
        for d in &self.diffs {
            v.push(format!(
                "  acc@ans {} − {} ({}) over {} paired cases: {} [{}, {}] → {}",
                d.a,
                d.b,
                d.scope,
                d.cases,
                f3(d.diff),
                f3(d.lo),
                f3(d.hi),
                d.better
            ));
        }
        if !self.diffs.is_empty() {
            v.push(format!(
                "  (bootstrap 95% interval, {BOOTSTRAP_RESAMPLES} resamples, seed {BOOTSTRAP_SEED:#x}; an interval across 0 is no difference)"
            ));
        }
        v.push(format!(
            "  breakdown (a cell under {JUDGE_MIN_CASES} cases is not judged; acc@ans / coverage / rule-abst cov / done prec):"
        ));
        for c in &self.breakdown {
            let mut line = format!(
                "    {:<7} {:<10} cases {:>4} rule-abst {:>4}{}",
                c.dimension,
                c.value,
                c.cases,
                c.rule_abstained,
                if c.judged { "" } else { "  not judged" }
            );
            for m in &c.providers {
                line.push_str(&format!(
                    "  {} {}/{}/{}/{}",
                    m.provider,
                    f2(m.accuracy_on_answered),
                    f2(m.coverage),
                    f2(m.rule_abstained_coverage),
                    f2(m.done_precision)
                ));
                if m.calibration.ece.is_some() {
                    line.push_str(&format!(" ece {}", f2(m.calibration.ece)));
                }
            }
            v.push(line);
        }
        v.push(String::new());
        v.push("acceptance, card J3 (assist; registered in the test map):".into());
        for a in &self.acceptance {
            v.push(format!("  {} — {}", a.provider, a.overall.as_str()));
            for c in &a.criteria {
                v.push(format!("  {}", c.line()));
            }
        }
        v.push(String::new());
        for n in &self.notes {
            v.push(format!("note: {n}"));
        }
        v
    }
}
