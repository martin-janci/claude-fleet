//! The offline `work_link` benchmark (test map card J1, phase 0):
//! `fleet-hub decide bench work-link`.
//!
//! **Dataset A** is every confirmed link a PERSON decided (source `manual`
//! or `started`; never `agent`, `agent_inferred` or a resolver's) whose
//! conversation kept a first prompt. Its truth is the linked item. Prompts
//! fleet typed itself (a start, a resume, a quick-reply chip…) and prompts
//! Claude Code submitted itself (a `<task-notification>`) are left out as
//! the census leaves them out ([`FleetPrompts::person_text`]; dataset H's
//! export too). For every case with other candidates a
//! **none-case** is added: the same state, the candidates without the truth,
//! and "abstain" as the right answer.
//!
//! **Dataset H** is the D39 hand labels: `--export-unlinked N --out FILE`
//! writes sessions with no link, each with its redacted prompt, its
//! candidates and `"label": null`; a person fills `label` with an item id or
//! `"none"`; `--labels FILE` reads it back. Reported on its own.
//!
//! **The candidate set** is an approximation of what fleet could have
//! offered at the decision ([`APPROXIMATIONS`]): the items of the case's org
//! (its tracker's, or local items linked in it) updated at most
//! [`SLACK_SECS`] after the decision and not unavailable then, newest first,
//! at most [`MAX_CANDIDATES`], the truth always in. Whether the truth would
//! have been there without forcing it, and whether it was in the fenced set
//! the M4.6 nudge offers (`tickets.rs` `allowed()` + `nudge.rs`), is reported
//! as **recall** before any model is judged.
//!
//! **The leakage guard** ([`redact_prompt`]): the state is the first prompt
//! with every recogniser match, every URL, the session's branch name, the
//! truth's key and exact title, and — for a `started` link — every word of
//! the `{key}-{slug}` branch slug (which is the ticket's title) removed.
//! Tests assert that neither the key, the title nor the slug survives.
//!
//! **Split** by time: cases ordered by decision time, dev the oldest
//! [`DEV_SHARE_PCT`]%, test the rest. Thresholds are chosen on dev and
//! applied to what is reported.
//!
//! **Providers**: `none` (always abstains), `bm25` ([`super::bm25`] over
//! titles and cached descriptions, abstaining under a score threshold chosen
//! on dev) and `jev` (one Choice through [`super::super::decide`], recorded
//! in `decision_runs` like any call; only when asked for, and only for a
//! case whose org the gate lets through — otherwise the case is skipped
//! with the gate's fallback). With [`Shape::ChoiceNoul`] a chosen item is
//! checked by a second call, one Noul ("does this session work on …?"),
//! and the answer is kept when the noul reaches a threshold chosen on dev.
//! `haiku` (D33) asks Jev's Choice — the same redacted state and options —
//! of `claude -p` on a host the operator names
//! ([`crate::service::decide::haiku`]); it answers at its own operating
//! point (its `none`), and an answer outside the options is an abstention
//! counted as `invalid`. It is never recorded in `decision_runs`.
//!
//! **Acceptance** (card J1, when `jev` ran): accuracy on answered at the
//! precision-0.9 point, Jev against BM25 at BM25's coverage (Jev's
//! threshold chosen on dev to match it), Jev against haiku at haiku's
//! coverage (the same way), abstention quality, calibration, and the
//! per-language-cell rule — each PASS, FAIL or NOT JUDGED.
//!
//! The report holds ids, words and numbers only: no prompt and no title.

use super::bm25::{self, Bm25};
use super::{
    bootstrap_acc_diff, f2, f3, gate_or_skip, mix, pct, percentile, round3, Calibration, Criterion,
    Paired, Verdict, MAX_CALLS,
};
use crate::ipc_error::{codes, IpcError};
use crate::service::decide::haiku::{reason::OTHER_ORG, Haiku};
use crate::service::decide::{
    decide, DecideCtx, DecideRequest, Feature, JevRequest, NoulCriteria, Question,
};
use crate::service::nl::census::{FleetPrompts, Shown};
use crate::service::nl::{self, Ranker};
use crate::service::trackers::tickets::{branch_slug, RECENT_DAYS};
use crate::service::work::recognize::{recognize, RecognizeCtx};
use crate::store::{BenchHostLink, BenchItemRow, Store, TrackerRow, NL_CENSUS_MIN_SCHEMA};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

/// The question's version, recorded on every Jev run of the benchmark.
pub const QUESTION_VERSION: &str = "work_link.bench.v1";
/// The check's version ([`Shape::ChoiceNoul`]'s second call).
pub const NOUL_QUESTION_VERSION: &str = "work_link.bench.noul.v1";
/// Card J1's registered acceptance (assist), test map §5.
pub const ACCEPT_ACCURACY: f64 = 0.90;
pub const ACCEPT_COVERAGE: f64 = 0.40;
pub const ACCEPT_ABOVE_BM25: f64 = 0.10;
pub const ACCEPT_ABSTENTION: f64 = 0.85;
/// "Not worse than `claude -p haiku` by more than 3 points" (at haiku's
/// coverage).
pub const ACCEPT_BELOW_HAIKU: f64 = 0.03;
/// A language cell this far below English at equal coverage falls back.
pub const ACCEPT_CELL_BELOW_ENGLISH: f64 = 0.10;
/// The English cell the language rule compares against.
pub const ENGLISH_CELL: &str = "en×en";
/// `decision_runs.subject_kind` of a benchmark call: gated without the
/// feature's mode, and kept out of the live breaker, budget and stats.
pub const SUBJECT_KIND: &str = crate::store::DECISION_BENCH_SUBJECT;
/// The option that means "none of these".
pub const NONE_OPTION: &str = "none";
/// The most candidates a case offers (test map J1: ≤ 50).
pub const MAX_CANDIDATES: usize = 50;
/// How long after the decision an item may have been updated and still
/// count as a candidate then.
pub const SLACK_SECS: i64 = 86_400;
/// The oldest share of dataset A that is dev.
pub const DEV_SHARE_PCT: usize = 60;
/// Coverage is reported at this precision (test map §4).
pub const TARGET_PRECISION: f64 = 0.9;
/// A threshold for coverage at precision must leave at least this many dev
/// answers: fewer is noise, not a precision.
pub const AT_PRECISION_MIN_ANSWERED: u64 = 5;
/// A cell with fewer cases is "not judged" (test map §3).
pub const JUDGE_MIN_CASES: u64 = 200;
/// Under this, a cell's metrics are not shown (its count shows as `<5`).
pub const SHOW_MIN_CASES: u64 = crate::service::nl::census::SUPPRESS_BELOW;
/// Bootstrap resamples and seed.
pub const BOOTSTRAP_RESAMPLES: usize = 1000;
pub const BOOTSTRAP_SEED: u64 = 0x4a31_5eed;
/// The nudge offers at most this many candidates (`nudge::MAX_CANDIDATES`).
pub const NUDGE_MAX: usize = crate::service::work::nudge::MAX_CANDIDATES;
/// `days` when none is given, and the longest window.
pub const DEFAULT_DAYS: u32 = 365;
pub const MAX_DAYS: u32 = 730;
/// Links read when no cap is given (newest first), and the largest cap.
pub const DEFAULT_MAX_CASES: u32 = 20_000;
pub const MAX_CASES: u32 = 100_000;
/// Jev calls one run makes at most, unless told otherwise.
pub const DEFAULT_MAX_CALLS: usize = 500;
/// The largest `--export-unlinked`.
pub const MAX_EXPORT: usize = 5_000;

/// What the benchmark approximates, printed in every header.
pub const APPROXIMATIONS: &[&str] = &[
    "candidates as of the decision: the items of the case's org updated at most 1 day after it and not unavailable then, newest first, at most 50 — the store keeps only an item's latest updated_at, not its history",
    "rejections the session had made are not subtracted from the candidates",
    "nudge recall: 'mine' is today's assignee against the tracker's account (status ignored), the host fence counts confirmed links on the host created before the decision, the host's org is today's",
    "only a conversation's first prompt (its first 200 characters) is the state; later prompts and commit subjects are not stored",
    "links of deleted sessions are not read (their conversations went with them)",
];

/// Which cases are reported (dev: the oldest [`DEV_SHARE_PCT`]% of A by
/// decision time).
pub use super::Split;

/// A provider the benchmark asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Always abstains: today's behaviour when nothing links a session.
    None,
    /// BM25 over the candidates' titles and cached descriptions.
    Bm25,
    /// Jev, through the decision envelope (network).
    Jev,
    /// `claude -p --model haiku` on a named host (D33; network, through
    /// that host's Claude account).
    Haiku,
}

impl Provider {
    pub const ALL: [Provider; 4] = [
        Provider::None,
        Provider::Bm25,
        Provider::Jev,
        Provider::Haiku,
    ];

    pub fn parse(s: &str) -> Option<Provider> {
        Provider::ALL.into_iter().find(|p| p.as_str() == s)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Provider::None => "none",
            Provider::Bm25 => "bm25",
            Provider::Jev => "jev",
            Provider::Haiku => "haiku",
        }
    }

    /// Asks a model over the network (its calls, latency, tokens and cost
    /// count, and its confidence is calibrated).
    pub fn is_model(self) -> bool {
        matches!(self, Provider::Jev | Provider::Haiku)
    }
}

/// How Jev is asked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Shape {
    /// One Choice over the candidates plus `none`.
    #[default]
    #[serde(rename = "choice")]
    Choice,
    /// The Choice, then one Noul on the chosen item only ("does this
    /// session work on …?"); the answer stands when the noul reaches a
    /// threshold chosen on dev. Two calls for a case that picks an item.
    #[serde(rename = "choice+noul")]
    ChoiceNoul,
}

impl Shape {
    pub fn parse(s: &str) -> Option<Shape> {
        match s {
            "choice" => Some(Shape::Choice),
            "choice+noul" => Some(Shape::ChoiceNoul),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Shape::Choice => "choice",
            Shape::ChoiceNoul => "choice+noul",
        }
    }
}

/// The dataset a case comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum Dataset {
    /// Person-decided links from the store.
    A,
    /// Hand labels (D39).
    H,
}

/// One candidate of a case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// The option key: `i<item id>`.
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The item's tracker provider, or `local`.
    #[serde(default)]
    pub provider: String,
}

/// PURE: an item's option key.
pub fn option_id(item_id: i64) -> String {
    format!("i{item_id}")
}

/// What the leakage test checks a case's state against. Never reported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Guard {
    pub key: Option<String>,
    pub title: Option<String>,
    /// The `{key}-{slug}` of a `started` link.
    pub slug: Option<String>,
    pub branch: Option<String>,
}

/// One benchmark case. Holds text (the redacted state, the candidates'
/// titles): it lives in this process only, and no report carries it.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchCase {
    /// `a<link>`, `a<link>n` (its none-case), `h<session>-<time>`, `h…n`.
    pub id: String,
    pub dataset: Dataset,
    /// When it was decided (A) or the conversation started (H).
    pub at: i64,
    pub org_id: Option<i64>,
    /// The truth item's tracker provider, `local`, or `-` (H, "none").
    pub tracker: String,
    /// The redacted first prompt: what a model sees.
    pub state: String,
    pub candidates: Vec<Candidate>,
    /// The right option, or `None` for "none of these" (abstain).
    pub truth: Option<String>,
    /// In the dev split (A only; H is never dev).
    pub dev: bool,
    pub nl_prompt: &'static str,
    pub nl_title: &'static str,
    pub code: &'static str,
    pub guard: Guard,
}

impl BenchCase {
    fn size_bucket(&self) -> &'static str {
        match self.candidates.len() {
            0 | 1 => "1",
            2..=5 => "2-5",
            6..=20 => "6-20",
            _ => "21-50",
        }
    }
}

// --- the leakage guard ---------------------------------------------------------

static URL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:\b(?:https?|ftp|ssh|git|wss?)://|\bwww\.)[^\s<>()\[\]{}'"`]+"#)
        .expect("URL pattern compiles")
});

/// What [`redact_prompt`] removes besides every recogniser match and URL.
#[derive(Debug, Clone, Copy, Default)]
pub struct Redact<'a> {
    pub branch: Option<&'a str>,
    /// The truth's key and exact title (dataset A).
    pub truth_key: Option<&'a str>,
    pub truth_title: Option<&'a str>,
    /// A `started` link: its branch slug. Every word of it, and of the
    /// title it was made from, is removed wherever it appears.
    pub started_slug: Option<&'a str>,
}

/// Every case-insensitive occurrence of `needle` replaced by `with`,
/// repeated until none is left (a removal can join two halves).
fn remove_ci(text: &str, needle: &str, with: &str) -> String {
    let needle = needle.trim();
    if needle.is_empty() {
        return text.to_string();
    }
    let re = match Regex::new(&format!("(?i){}", regex::escape(needle))) {
        Ok(r) => r,
        Err(_) => return text.to_string(),
    };
    let mut t = text.to_string();
    for _ in 0..16 {
        if !re.is_match(&t) {
            break;
        }
        t = re.replace_all(&t, with).into_owned();
    }
    t
}

/// Two stems name the same word: equal, or (both at least
/// [`RELATED_STEM_CHARS`] long) one a prefix of the other — `oprav` and
/// `opravi` (from `opraviť`, whose `ť` a branch slug drops).
fn related_stems(a: &str, b: &str) -> bool {
    a == b
        || (a.chars().count() >= RELATED_STEM_CHARS
            && b.chars().count() >= RELATED_STEM_CHARS
            && (a.starts_with(b) || b.starts_with(a)))
}

/// A shorter shared prefix is not taken for the same word.
const RELATED_STEM_CHARS: usize = 4;

/// Every word of `text` whose stem ([`bm25::stem`]) is related to one of
/// `stems` ([`related_stems`]) removed.
fn remove_words(text: &str, stems: &HashSet<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |w: &mut String, out: &mut String| {
        if !w.is_empty() {
            let st = bm25::stem(w);
            if !stems.iter().any(|s| related_stems(&st, s)) {
                out.push_str(w);
            }
            w.clear();
        }
    };
    for c in text.chars() {
        if c.is_alphanumeric() {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// The recognition context the guard uses: no prefixes (so every
/// key-shaped token counts), every configured tracker's host, and a
/// placeholder repository so a bare `#123` is caught too.
pub fn guard_ctx(trackers: &[TrackerRow]) -> RecognizeCtx {
    RecognizeCtx {
        prefixes: Vec::new(),
        trackers: crate::service::work::detect::tracker_hosts(trackers),
        repo: Some("bench/repo".into()),
    }
}

/// PURE: the state a model is shown for `prompt` (the leakage guard; see
/// the module docs). The envelope's own redaction (`redact_state`) runs last.
pub fn redact_prompt(prompt: &str, ctx: &RecognizeCtx, r: &Redact<'_>) -> String {
    // 1. Every recogniser match (keys, ticket URLs, #123), right to left.
    let mut t = prompt.to_string();
    let mut spans: Vec<(usize, usize)> = recognize(&t, ctx).into_iter().map(|m| m.span).collect();
    spans.sort();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (a, b) in spans {
        match merged.last_mut() {
            Some(last) if a < last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    for (a, b) in merged.into_iter().rev() {
        if t.is_char_boundary(a) && t.is_char_boundary(b) && a <= b && b <= t.len() {
            t.replace_range(a..b, "[ref]");
        }
    }
    // 2. Every URL.
    t = URL_RE.replace_all(&t, "[url]").into_owned();
    // 3. The branch, whole and by its path segments.
    if let Some(b) = r.branch.map(str::trim).filter(|b| !b.is_empty()) {
        t = remove_ci(&t, b, "[branch]");
        for seg in b.split('/').filter(|s| s.chars().count() >= 3) {
            t = remove_ci(&t, seg, "[branch]");
        }
    }
    // 4. The truth's key and exact title.
    if let Some(k) = r.truth_key {
        t = remove_ci(&t, k, "[ref]");
    }
    if let Some(title) = r.truth_title {
        t = remove_ci(&t, title, "[…]");
    }
    // 5. A started link's slug: the slug itself, then every word of it and
    //    of the title it came from, by stem (inflections too).
    if let Some(slug) = r.started_slug {
        t = remove_ci(&t, slug, "[branch]");
        let mut stems: HashSet<String> = slug
            .split('-')
            .filter(|w| !w.is_empty())
            .map(bm25::stem)
            .collect();
        if let Some(title) = r.truth_title {
            stems.extend(bm25::tokenize(title));
            stems.extend(
                title
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|w| !w.is_empty())
                    .map(bm25::stem),
            );
        }
        t = remove_words(&t, &stems);
    }
    let t = crate::service::decide::redact_state(&t);
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

// --- options -------------------------------------------------------------------

/// What to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchOptions {
    pub days: u32,
    pub org: Option<i64>,
    pub max_cases: u32,
    pub split: Split,
    pub providers: Vec<Provider>,
    /// Jev calls at most (cases beyond are skipped as `max_calls`).
    pub max_calls: usize,
    /// How Jev is asked ([`Shape::Choice`] unless set).
    pub shape: Shape,
    pub now: i64,
}

impl BenchOptions {
    pub fn new(
        days: Option<u32>,
        org: Option<i64>,
        max_cases: Option<u32>,
        split: Split,
        providers: Vec<Provider>,
        max_calls: Option<usize>,
        now: i64,
    ) -> Result<Self, IpcError> {
        let days = days.unwrap_or(DEFAULT_DAYS);
        if days == 0 || days > MAX_DAYS {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("days must be 1-{MAX_DAYS}, got {days}"),
            ));
        }
        let max_cases = max_cases.unwrap_or(DEFAULT_MAX_CASES);
        if max_cases == 0 || max_cases > MAX_CASES {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("max cases must be 1-{MAX_CASES}, got {max_cases}"),
            ));
        }
        let mut providers: Vec<Provider> = if providers.is_empty() {
            vec![Provider::None, Provider::Bm25]
        } else {
            providers
        };
        providers.sort();
        providers.dedup();
        Ok(BenchOptions {
            days,
            org,
            max_cases,
            split,
            providers,
            max_calls: max_calls.unwrap_or(DEFAULT_MAX_CALLS),
            shape: Shape::Choice,
            now,
        })
    }

    /// The same options, asking Jev in `shape`.
    pub fn with_shape(mut self, shape: Shape) -> Self {
        self.shape = shape;
        self
    }

    pub fn since(&self) -> i64 {
        self.now - i64::from(self.days) * 86_400
    }
}

// --- the dataset ---------------------------------------------------------------

/// How big the datasets are and what was left out.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Sizes {
    /// Person-decided links with a first prompt in the window.
    pub a_links_read: Shown,
    /// Of them, prompts fleet (or Claude Code) typed itself: left out.
    pub a_fleet_typed: Shown,
    /// Of them, in another org than `--org`: left out.
    pub a_other_org: Shown,
    pub a_cases: Shown,
    pub a_none_cases: Shown,
    pub a_dev: Shown,
    pub a_test: Shown,
    /// Rows in the `--labels` file.
    pub h_records: Shown,
    /// Rows a person labeled.
    pub h_labeled: Shown,
    pub h_cases: Shown,
    pub h_none_cases: Shown,
    /// Labels naming an item that is not among the row's candidates: the
    /// candidate set missed the truth (a recall miss, not scored).
    pub h_label_outside: Shown,
    /// Rows whose session is no longer in the database: their org cannot
    /// be read from it, so they are left out ([`resolve_label_orgs`]).
    pub h_session_gone: Shown,
}

/// Where the truth was, before any model is asked.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Recall {
    /// Dataset A truth cases.
    pub cases: Shown,
    /// The truth passed the benchmark's own candidate filters (it is added
    /// when it did not).
    pub bench_set: Shown,
    pub bench_set_pct: Option<f64>,
    /// The truth was in the nudge's fenced candidate set as of the decision.
    pub nudge_fence: Shown,
    pub nudge_fence_pct: Option<f64>,
    /// That set had 1..=5 candidates (the nudge would have fired).
    pub nudge_would_fire: Shown,
    pub nudge_would_fire_pct: Option<f64>,
    /// It would have fired AND offered the truth.
    pub nudge_offered_truth: Shown,
    pub nudge_offered_truth_pct: Option<f64>,
}

/// The loaded datasets.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub schema_version: i64,
    pub cases: Vec<BenchCase>,
    pub sizes: Sizes,
    pub recall: Recall,
    /// org id → name.
    pub org_names: BTreeMap<i64, String>,
}

/// [`pct`] of a count that is shown: none when the count shows as `<5`, so
/// the share cannot give it away.
fn shown_pct(k: u64, n: u64) -> Option<f64> {
    if k > 0 && k < SHOW_MIN_CASES {
        None
    } else {
        pct(k, n)
    }
}

/// The candidate pool, and the facts the filters need.
struct Pool {
    items: Vec<BenchItemRow>,
    by_id: HashMap<i64, usize>,
    trackers: HashMap<i64, TrackerRow>,
    local_orgs: HashMap<i64, BTreeSet<Option<i64>>>,
}

impl Pool {
    fn load(store: &Store) -> Result<(Pool, Vec<TrackerRow>), IpcError> {
        let items = store.bench_work_items()?;
        let by_id = items.iter().enumerate().map(|(i, it)| (it.id, i)).collect();
        let list = store.list_trackers()?;
        let trackers = list.iter().map(|t| (t.id, t.clone())).collect();
        let mut local_orgs: HashMap<i64, BTreeSet<Option<i64>>> = HashMap::new();
        for (item, org) in store.bench_local_item_orgs()? {
            local_orgs.entry(item).or_default().insert(org);
        }
        Ok((
            Pool {
                items,
                by_id,
                trackers,
                local_orgs,
            },
            list,
        ))
    }

    fn item(&self, id: i64) -> Option<&BenchItemRow> {
        self.by_id.get(&id).map(|&i| &self.items[i])
    }

    fn tracker(&self, it: &BenchItemRow) -> Option<&TrackerRow> {
        it.tracker_id.and_then(|t| self.trackers.get(&t))
    }

    fn provider(&self, it: &BenchItemRow) -> String {
        self.tracker(it)
            .map(|t| t.provider.clone())
            .unwrap_or_else(|| it.source.clone())
    }

    /// The item belongs to `org`: its tracker's org, or (a local item) an
    /// org a session that linked it belongs to.
    fn in_org(&self, it: &BenchItemRow, org: Option<i64>) -> bool {
        match it.tracker_id {
            Some(_) => self.tracker(it).map(|t| t.org_id) == Some(org),
            None => self
                .local_orgs
                .get(&it.id)
                .is_some_and(|orgs| orgs.contains(&org)),
        }
    }

    /// Passes the candidate filters at `at` for `org`.
    fn eligible(&self, it: &BenchItemRow, org: Option<i64>, at: i64) -> bool {
        it.updated_at <= at + SLACK_SECS
            && it.unavailable_at.is_none_or(|u| u > at)
            && self.in_org(it, org)
    }

    fn candidate(&self, it: &BenchItemRow) -> Candidate {
        Candidate {
            id: option_id(it.id),
            title: it.title.clone(),
            description: it.description.clone().filter(|d| !d.trim().is_empty()),
            provider: self.provider(it),
        }
    }

    /// The candidates for a decision of `org` at `at`, `truth` always in,
    /// in a stable order that does not depend on which one is the truth.
    fn candidates(
        &self,
        org: Option<i64>,
        at: i64,
        truth: Option<i64>,
        salt: u64,
    ) -> Vec<Candidate> {
        let want = if truth.is_some() {
            MAX_CANDIDATES - 1
        } else {
            MAX_CANDIDATES
        };
        let mut picked: Vec<&BenchItemRow> = self
            .items
            .iter()
            .filter(|it| Some(it.id) != truth && self.eligible(it, org, at))
            .take(want)
            .collect();
        if let Some(t) = truth.and_then(|t| self.item(t)) {
            picked.push(t);
        }
        picked.sort_by_key(|it| (mix(salt, it.id as u64), it.id));
        picked.into_iter().map(|it| self.candidate(it)).collect()
    }

    /// The nudge's candidate ids as of `at` on `host` (whose org is
    /// `host_org`), not counting the link `except`: "mine" tracker items
    /// inside the host fence and the host's org scope, plus keyed local
    /// items changed in the last [`RECENT_DAYS`].
    fn nudge_set(
        &self,
        host_links: &[BenchHostLink],
        host_org: Option<i64>,
        at: i64,
        except: i64,
    ) -> BTreeSet<i64> {
        let fence: HashSet<i64> = host_links
            .iter()
            .filter(|l| l.link_id != except && l.created_at < at)
            .map(|l| l.item_id)
            .collect();
        let mut out = BTreeSet::new();
        for it in &self.items {
            if it.created_at > at || it.unavailable_at.is_some_and(|u| u <= at) {
                continue;
            }
            match self.tracker(it) {
                Some(t) => {
                    let mine = t
                        .config
                        .account_id
                        .as_deref()
                        .is_some_and(|me| it.assignee_id.as_deref() == Some(me));
                    let sees = t.org_id.is_none() || t.org_id == host_org;
                    if mine && sees && fence.contains(&it.id) {
                        out.insert(it.id);
                    }
                }
                None if it.source == "local"
                    && it.key.is_some()
                    && it.updated_at >= at - RECENT_DAYS * 86_400 =>
                {
                    out.insert(it.id);
                }
                None => {}
            }
        }
        out
    }
}

fn read_nl(ranker: &dyn Ranker, text: &str) -> (&'static str, &'static str) {
    let r = nl::read_with(ranker, text);
    (r.bucket.as_str(), r.code.as_str())
}

/// Load dataset A from the store (read only), and dataset H from `labels`
/// when given.
pub fn load(
    store: &Store,
    ranker: &dyn Ranker,
    opts: &BenchOptions,
    labels: Option<&[UnlinkedCase]>,
) -> Result<Loaded, IpcError> {
    let schema_version = store.schema_version()?;
    if schema_version < NL_CENSUS_MIN_SCHEMA {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "this database is at schema {schema_version}; the benchmark needs \
                 {NL_CENSUS_MIN_SCHEMA} or later (open it once with a current fleet to migrate it)"
            ),
        ));
    }
    let (pool, trackers) = Pool::load(store)?;
    let ctx = guard_ctx(&trackers);
    let fleet = FleetPrompts::from_store(store)?;
    let mut sizes = Sizes::default();
    let mut recall = Recall::default();
    let mut host_links: HashMap<String, Vec<BenchHostLink>> = HashMap::new();
    let mut host_orgs: HashMap<String, Option<i64>> = HashMap::new();
    let mut truths: Vec<BenchCase> = Vec::new();
    let mut nones: Vec<BenchCase> = Vec::new();

    for row in store.bench_work_link_cases(opts.since(), opts.max_cases)? {
        sizes.a_links_read.0 += 1;
        if opts.org.is_some() && opts.org != row.org_id {
            sizes.a_other_org.0 += 1;
            continue;
        }
        let Some(prompt) = fleet.person_text(&row.first_prompt, row.last_prompt.as_deref()) else {
            sizes.a_fleet_typed.0 += 1;
            continue;
        };
        let Some(truth) = pool.item(row.item_id) else {
            continue;
        };
        let at = row.decided_at;
        let slug = (row.source == "started")
            .then(|| branch_slug(truth.key.as_deref().unwrap_or_default(), &truth.title));
        let state = redact_prompt(
            &prompt,
            &ctx,
            &Redact {
                branch: row.branch.as_deref(),
                truth_key: truth.key.as_deref(),
                truth_title: Some(&truth.title),
                started_slug: slug.as_deref(),
            },
        );
        let candidates = pool.candidates(row.org_id, at, Some(truth.id), row.link_id as u64);
        let (nl_prompt, code) = read_nl(ranker, &state);
        let (nl_title, _) = read_nl(ranker, &truth.title);

        // Recall, before any model.
        recall.cases.0 += 1;
        if pool.eligible(truth, row.org_id, at) {
            recall.bench_set.0 += 1;
        }
        if !host_links.contains_key(&row.host_alias) {
            let l = store.bench_host_links(&row.host_alias)?;
            host_links.insert(row.host_alias.clone(), l);
            let o = store.host_org(&row.host_alias)?;
            host_orgs.insert(row.host_alias.clone(), o);
        }
        let nudge = pool.nudge_set(
            &host_links[&row.host_alias],
            host_orgs[&row.host_alias],
            at,
            row.link_id,
        );
        let fires = (1..=NUDGE_MAX).contains(&nudge.len());
        let in_nudge = nudge.contains(&truth.id);
        recall.nudge_fence.0 += u64::from(in_nudge);
        recall.nudge_would_fire.0 += u64::from(fires);
        recall.nudge_offered_truth.0 += u64::from(fires && in_nudge);

        let case = BenchCase {
            id: format!("a{}", row.link_id),
            dataset: Dataset::A,
            at,
            org_id: row.org_id,
            tracker: pool.provider(truth),
            state,
            candidates,
            truth: Some(option_id(truth.id)),
            dev: false,
            nl_prompt,
            nl_title,
            code,
            guard: Guard {
                key: truth.key.clone(),
                title: Some(truth.title.clone()),
                slug,
                branch: row.branch.clone(),
            },
        };
        truths.push(case);
    }

    // The time split: oldest DEV_SHARE_PCT% of the truth cases are dev.
    truths.sort_by(|a, b| a.at.cmp(&b.at).then(a.id.cmp(&b.id)));
    let n_dev = truths.len() * DEV_SHARE_PCT / 100;
    for (i, c) in truths.iter_mut().enumerate() {
        c.dev = i < n_dev;
    }
    for c in &truths {
        let others: Vec<Candidate> = c
            .candidates
            .iter()
            .filter(|x| Some(&x.id) != c.truth.as_ref())
            .cloned()
            .collect();
        if others.is_empty() {
            continue;
        }
        nones.push(BenchCase {
            id: format!("{}n", c.id),
            candidates: others,
            truth: None,
            ..c.clone()
        });
    }
    sizes.a_cases.0 = truths.len() as u64;
    sizes.a_none_cases.0 = nones.len() as u64;
    sizes.a_dev.0 = n_dev as u64;
    sizes.a_test.0 = (truths.len() - n_dev) as u64;
    recall.bench_set_pct = shown_pct(recall.bench_set.0, recall.cases.0);
    recall.nudge_fence_pct = shown_pct(recall.nudge_fence.0, recall.cases.0);
    recall.nudge_would_fire_pct = shown_pct(recall.nudge_would_fire.0, recall.cases.0);
    recall.nudge_offered_truth_pct = shown_pct(recall.nudge_offered_truth.0, recall.cases.0);

    let mut cases = truths;
    cases.extend(nones);
    if let Some(rows) = labels {
        let rows = resolve_label_orgs(store, rows, &mut sizes)
            .map_err(|e| IpcError::new(codes::E_INVALID, e))?;
        cases.extend(h_cases(&rows, ranker, opts, &mut sizes));
    }
    let org_names = store
        .list_orgs()?
        .into_iter()
        .map(|o| (o.id, o.name))
        .collect();
    Ok(Loaded {
        schema_version,
        cases,
        sizes,
        recall,
        org_names,
    })
}

// --- D39: the hand-label export and its read-back -------------------------------

/// One row of the D39 hand-label file: a session with no link, its
/// redacted first prompt and its candidates. A person sets `label` to one
/// candidate's `id` or to `"none"`. The file holds prompt and title text:
/// it is written `0600`, never over a file, and stays on the machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnlinkedCase {
    /// `h<session>-<started_at>`.
    pub id: String,
    pub session_id: i64,
    pub started_at: i64,
    #[serde(default)]
    pub org_id: Option<i64>,
    /// The redacted first prompt.
    pub prompt: String,
    pub candidates: Vec<Candidate>,
    /// A candidate's `id`, `"none"`, or `null` (not labeled yet).
    pub label: Option<String>,
}

/// Up to `n` sessions with a first prompt and no confirmed or suggested
/// link, spread evenly over the window (one conversation per session), each
/// with its redacted prompt and its candidates as of the conversation's
/// start, unlabeled.
pub fn export_unlinked(
    store: &Store,
    opts: &BenchOptions,
    n: usize,
) -> Result<Vec<UnlinkedCase>, IpcError> {
    if n == 0 || n > MAX_EXPORT {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("export 1-{MAX_EXPORT} sessions, not {n}"),
        ));
    }
    let (pool, trackers) = Pool::load(store)?;
    let ctx = guard_ctx(&trackers);
    let fleet = FleetPrompts::from_store(store)?;
    let mut seen = HashSet::new();
    let rows: Vec<_> = store
        .bench_unlinked_conversations(opts.since(), opts.max_cases)?
        .into_iter()
        .filter(|r| opts.org.is_none() || opts.org == r.org_id)
        .filter_map(|r| {
            let p = fleet
                .person_text(&r.first_prompt, r.last_prompt.as_deref())?
                .to_string();
            Some((r, p))
        })
        .filter(|(r, _)| seen.insert(r.session_id))
        .collect();
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let step = (rows.len() as f64 / n as f64).max(1.0);
    let mut out = Vec::new();
    let mut at = 0.0;
    while (at as usize) < rows.len() && out.len() < n {
        let (r, first_prompt) = &rows[at as usize];
        let prompt = redact_prompt(
            first_prompt,
            &ctx,
            &Redact {
                branch: r.branch.as_deref(),
                ..Default::default()
            },
        );
        out.push(UnlinkedCase {
            id: format!("h{}-{}", r.session_id, r.started_at),
            session_id: r.session_id,
            started_at: r.started_at,
            org_id: r.org_id,
            prompt,
            candidates: pool.candidates(r.org_id, r.started_at, None, r.session_id as u64),
            label: None,
        });
        at += step;
    }
    Ok(out)
}

/// Every hand-label row's org, from the DATABASE, before any case is built
/// (Jev's consent gate, the haiku org fence and `--org` all trust it), as
/// `status_map::resolve_label_orgs` does for sections: the row's session's
/// org as the database computes it now. A row naming another org than
/// that is refused (the file was edited, or the session's org moved since
/// the export and its candidates are another org's: export again); a row
/// whose session is gone is left out and counted (`h_session_gone`),
/// since nothing can vouch for its org. An error names the row.
pub fn resolve_label_orgs(
    store: &Store,
    rows: &[UnlinkedCase],
    sizes: &mut Sizes,
) -> Result<Vec<UnlinkedCase>, String> {
    let org_name = |o: Option<i64>| o.map_or_else(|| "none".to_string(), |o| o.to_string());
    let mut out = Vec::with_capacity(rows.len());
    for (i, r) in rows.iter().enumerate() {
        let row = i + 1;
        let Some(session) = store
            .get_session_by_id(r.session_id)
            .map_err(|e| format!("row {row}: {e}"))?
        else {
            sizes.h_records.0 += 1;
            sizes.h_session_gone.0 += 1;
            continue;
        };
        if r.org_id != session.org_id {
            return Err(format!(
                "row {row}: org_id {} but session {} is in org {}; export the labels again",
                org_name(r.org_id),
                r.session_id,
                org_name(session.org_id)
            ));
        }
        out.push(UnlinkedCase {
            org_id: session.org_id,
            ..r.clone()
        });
    }
    Ok(out)
}

/// PURE: the rows of a hand-label file (JSON lines; blank lines skipped).
pub fn parse_labels(jsonl: &str) -> Result<Vec<UnlinkedCase>, String> {
    jsonl
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| serde_json::from_str(l).map_err(|e| format!("line {}: {e}", i + 1)))
        .collect()
}

/// Dataset H from the labeled rows.
fn h_cases(
    rows: &[UnlinkedCase],
    ranker: &dyn Ranker,
    opts: &BenchOptions,
    sizes: &mut Sizes,
) -> Vec<BenchCase> {
    let mut out = Vec::new();
    for r in rows {
        sizes.h_records.0 += 1;
        if opts.org.is_some() && opts.org != r.org_id {
            continue;
        }
        let Some(label) = r.label.as_deref().map(str::trim).filter(|l| !l.is_empty()) else {
            continue;
        };
        sizes.h_labeled.0 += 1;
        let truth = if label.eq_ignore_ascii_case(NONE_OPTION) {
            None
        } else {
            match r.candidates.iter().find(|c| c.id == label) {
                Some(c) => Some(c),
                None => {
                    sizes.h_label_outside.0 += 1;
                    continue;
                }
            }
        };
        // The file is the person's to edit: redact again on the way in.
        let state = crate::service::decide::redact_state(&r.prompt);
        let (nl_prompt, code) = read_nl(ranker, &state);
        let (nl_title, tracker) = match truth {
            Some(c) => (read_nl(ranker, &c.title).0, c.provider.clone()),
            None => ("-", "-".to_string()),
        };
        if truth.is_some() {
            sizes.h_cases.0 += 1;
        } else {
            sizes.h_none_cases.0 += 1;
        }
        out.push(BenchCase {
            id: if truth.is_some() {
                r.id.clone()
            } else {
                format!("{}n", r.id)
            },
            dataset: Dataset::H,
            at: r.started_at,
            org_id: r.org_id,
            tracker,
            state,
            candidates: r.candidates.clone(),
            truth: truth.map(|c| c.id.clone()),
            dev: false,
            nl_prompt,
            nl_title,
            code,
            guard: Guard::default(),
        });
    }
    out
}

// --- providers -----------------------------------------------------------------

/// What one provider did with one case.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    /// The provider was asked (false: skipped, see `reason`).
    pub ran: bool,
    /// Its pick (an option id), `None` for "none of these".
    pub pick: Option<String>,
    /// Its score, what thresholds apply to: BM25's top score, Jev's
    /// confidence — or, with [`Shape::ChoiceNoul`], the noul of the chosen
    /// item.
    pub score: Option<f64>,
    /// Jev: the Choice's own confidence (calibration).
    pub choice_confidence: Option<f64>,
    /// Jev with [`Shape::ChoiceNoul`]: the noul of the chosen item.
    pub noul: Option<f64>,
    /// Why there is no usable answer: a gate refusal (skipped) or a failed
    /// call's fallback.
    pub reason: Option<String>,
    /// Calls made for the case (Jev: 1, or 2 with the noul).
    pub calls: u32,
    /// The case's latency, both calls together.
    pub latency_ms: Option<i64>,
    pub input_tokens: i64,
    pub cost_microusd: i64,
    /// Haiku: it answered, but not with one of the options (an abstention).
    pub invalid: bool,
    /// Haiku: the call ran and its envelope reported no usage.
    pub usage_unknown: bool,
}

impl Outcome {
    /// Asked, and no fallback: an answer (or an abstention) to score.
    pub fn usable(&self) -> bool {
        self.ran && self.reason.is_none()
    }

    fn skipped(reason: &str) -> Outcome {
        Outcome {
            reason: Some(reason.to_string()),
            ..Default::default()
        }
    }
}

/// PURE: BM25's raw pick for `case` (before its threshold).
pub fn bm25_outcome(case: &BenchCase) -> Outcome {
    let docs: Vec<String> = case
        .candidates
        .iter()
        .map(|c| {
            format!(
                "{0} {0} {1}",
                c.title,
                c.description.as_deref().unwrap_or_default()
            )
        })
        .collect();
    let best = Bm25::new(&docs).best(&case.state);
    Outcome {
        ran: true,
        pick: best.map(|(i, _)| case.candidates[i].id.clone()),
        score: Some(best.map(|b| b.1).unwrap_or(0.0)),
        ..Default::default()
    }
}

/// The instruction of the Jev question.
pub const JEV_INSTRUCTIONS: &str = "Which one of these work items is this coding session \
    working on? The state is the first prompt a person typed into the session; ticket keys, \
    links and branch names were removed from it. Answer \"none\" if none of them fits or it \
    cannot be told.";

/// PURE: the Jev request for `case`: its state, and one Choice over the
/// candidate ids (each described by its title) plus [`NONE_OPTION`].
pub fn jev_request(case: &BenchCase) -> JevRequest {
    let mut criteria: BTreeMap<String, Option<serde_json::Value>> = case
        .candidates
        .iter()
        .map(|c| {
            (
                c.id.clone(),
                Some(serde_json::Value::String(c.title.clone())),
            )
        })
        .collect();
    criteria.insert(
        NONE_OPTION.to_string(),
        Some(serde_json::Value::String(
            "None of these: the session works on something else, or it cannot be told".into(),
        )),
    );
    JevRequest {
        state: serde_json::json!({ "first_prompt": case.state }),
        question: Question::Choice {
            instructions: serde_json::Value::String(JEV_INSTRUCTIONS.into()),
            criteria,
        },
    }
}

/// The instruction of the [`Shape::ChoiceNoul`] check; the item's title
/// follows it.
pub const NOUL_INSTRUCTIONS: &str = "The state is the first prompt a person typed into a coding \
    session; ticket keys, links and branch names were removed from it. Does this session work on \
    the following work item?";

/// PURE: the check for `case` and its chosen `candidate`: the same state,
/// one Noul on the item's title.
pub fn noul_request(case: &BenchCase, candidate: &Candidate) -> JevRequest {
    JevRequest {
        state: serde_json::json!({ "first_prompt": case.state }),
        question: Question::Noul {
            instructions: serde_json::Value::String(format!(
                "{NOUL_INSTRUCTIONS} {}",
                candidate.title
            )),
            criteria: Some(NoulCriteria {
                yes: serde_json::Value::String("The session works on this item.".into()),
                no: serde_json::Value::String(
                    "The session works on something else, or it cannot be told.".into(),
                ),
            }),
        },
    }
}

/// What one envelope call gave the benchmark.
struct Call {
    ran: bool,
    fallback: Option<String>,
    value: Option<String>,
    confidence: Option<f64>,
    latency_ms: Option<i64>,
    input_tokens: i64,
    cost_microusd: i64,
}

async fn call(
    ctx: &DecideCtx,
    case: &BenchCase,
    subject: String,
    request: JevRequest,
    version: &str,
) -> Call {
    let res = decide(
        ctx,
        DecideRequest {
            feature: Feature::WorkLink,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: subject,
            org_id: case.org_id,
            request,
            baseline: None,
            question_version: version.into(),
            min_confidence: None,
        },
    )
    .await;
    let usable = res.usable();
    Call {
        ran: res.mode.is_some(),
        fallback: res.fallback.map(|f| f.as_str().to_string()),
        value: usable.map(|a| a.value.clone()),
        confidence: usable.and_then(|a| a.confidence),
        latency_ms: res.latency_ms,
        input_tokens: res.input_tokens,
        cost_microusd: res.cost_microusd,
    }
}

/// Ask Jev about each case through the envelope, one call at a time. A case
/// whose org the gate refuses is skipped with the gate's fallback and
/// nothing is sent or recorded for it; every call that is made is recorded
/// in `decision_runs` (feature `work_link`, subject `bench:<case>`, and
/// `bench:<case>:noul` for a check). At most `max_calls` calls; the rest are
/// skipped as `max_calls` (a case whose check no longer fits too).
pub async fn run_jev(
    ctx: &DecideCtx,
    cases: &[&BenchCase],
    max_calls: usize,
    shape: Shape,
) -> Vec<Outcome> {
    let mut out = Vec::with_capacity(cases.len());
    let mut calls = 0usize;
    for case in cases {
        // The gate's refusal first, then a case with nothing to choose
        // from, then the budget.
        let skip = match gate_or_skip(ctx, Feature::WorkLink, case.org_id, calls, max_calls) {
            Err(r) if r != MAX_CALLS => Some(r),
            _ if case.candidates.is_empty() => Some("no_candidates"),
            r => r.err(),
        };
        if let Some(r) = skip {
            out.push(Outcome::skipped(r));
            continue;
        }
        calls += 1;
        let c = call(
            ctx,
            case,
            case.id.clone(),
            jev_request(case),
            QUESTION_VERSION,
        )
        .await;
        let mut o = Outcome {
            ran: c.ran,
            reason: c.fallback,
            calls: 1,
            latency_ms: c.latency_ms,
            input_tokens: c.input_tokens,
            cost_microusd: c.cost_microusd,
            ..Default::default()
        };
        if let Some(v) = &c.value {
            o.pick = (v != NONE_OPTION).then(|| v.clone());
            o.score = c.confidence;
            o.choice_confidence = c.confidence;
        }
        let chosen = o
            .pick
            .as_ref()
            .filter(|_| o.usable() && shape == Shape::ChoiceNoul)
            .and_then(|p| case.candidates.iter().find(|x| &x.id == p));
        if let Some(candidate) = chosen {
            if calls >= max_calls {
                o.reason = Some("max_calls".into());
            } else {
                calls += 1;
                let n = call(
                    ctx,
                    case,
                    format!("{}:noul", case.id),
                    noul_request(case, candidate),
                    NOUL_QUESTION_VERSION,
                )
                .await;
                o.calls = 2;
                o.latency_ms = match (o.latency_ms, n.latency_ms) {
                    (Some(a), Some(b)) => Some(a + b),
                    (a, b) => a.or(b),
                };
                o.input_tokens += n.input_tokens;
                o.cost_microusd += n.cost_microusd;
                o.reason = n.fallback;
                o.noul = n.value.as_deref().and_then(|v| v.parse::<f64>().ok());
                o.score = o.noul;
                if o.reason.is_none() && o.noul.is_none() {
                    o.reason = Some("invalid_answer".into());
                }
            }
        }
        out.push(o);
    }
    out
}

/// Ask `claude -p` (D33) about each case, one call at a time on the
/// configured host, with Jev's Choice ([`jev_request`]: the same redacted
/// state, the candidates' ids and titles, and `none`). Its pick is the
/// answer (`none` abstains) and its confidence the score; an answer outside
/// the options is an abstention counted as `invalid`, a failed call
/// (timeout, SSH, `claude`) is skipped with its reason. A case whose org
/// is not the host's is skipped as `other_org` and nothing is sent for it.
/// At most `max_calls` calls; nothing is recorded in `decision_runs`.
pub async fn run_haiku(h: &Haiku<'_>, cases: &[&BenchCase], max_calls: usize) -> Vec<Outcome> {
    let mut out = Vec::with_capacity(cases.len());
    let mut calls = 0usize;
    for case in cases {
        // Never across the org boundary: a case goes only to a host of its
        // own org (no org on both sides is the same).
        if !h.may_ask(case.org_id) {
            out.push(Outcome::skipped(OTHER_ORG));
            continue;
        }
        if case.candidates.is_empty() {
            out.push(Outcome::skipped("no_candidates"));
            continue;
        }
        if calls >= max_calls {
            out.push(Outcome::skipped("max_calls"));
            continue;
        }
        let r = h.ask(&jev_request(case)).await;
        calls += usize::from(r.ran);
        let usable = r.error.is_none();
        out.push(Outcome {
            ran: r.ran,
            pick: r.choice.clone().filter(|c| usable && c != NONE_OPTION),
            score: r.confidence.filter(|_| usable),
            choice_confidence: r.confidence.filter(|_| usable),
            reason: r.error.map(str::to_string),
            calls: u32::from(r.ran),
            latency_ms: r.latency_ms,
            input_tokens: r.input_tokens.unwrap_or_default(),
            cost_microusd: r.cost_microusd.unwrap_or_default(),
            invalid: r.invalid,
            usage_unknown: r.ran && usable && r.input_tokens.is_none(),
            ..Default::default()
        });
    }
    out
}

/// Every provider's outcome per case id.
pub type Outcomes = BTreeMap<Provider, HashMap<String, Outcome>>;

/// Run the offline providers on every case, and Jev (when asked for and
/// `jev` is given) on the reported cases of A and on H.
pub async fn run_providers(
    loaded: &Loaded,
    opts: &BenchOptions,
    jev: Option<&DecideCtx>,
) -> Outcomes {
    run_providers_with(loaded, opts, jev, None).await
}

/// [`run_providers`], and the haiku baseline (when asked for and `haiku`
/// is given) on the same cases as Jev. `max_calls` bounds each network
/// provider's calls.
pub async fn run_providers_with(
    loaded: &Loaded,
    opts: &BenchOptions,
    jev: Option<&DecideCtx>,
    haiku: Option<&Haiku<'_>>,
) -> Outcomes {
    let mut all: Outcomes = BTreeMap::new();
    for p in &opts.providers {
        let m: HashMap<String, Outcome> = match p {
            Provider::None => loaded
                .cases
                .iter()
                .map(|c| {
                    (
                        c.id.clone(),
                        Outcome {
                            ran: true,
                            ..Default::default()
                        },
                    )
                })
                .collect(),
            Provider::Bm25 => loaded
                .cases
                .iter()
                .map(|c| (c.id.clone(), bm25_outcome(c)))
                .collect(),
            Provider::Jev | Provider::Haiku => {
                let wanted: Vec<&BenchCase> = loaded
                    .cases
                    .iter()
                    .filter(|c| c.dataset == Dataset::H || opts.split.keeps(c.dev))
                    .collect();
                let outs = match (p, jev, haiku) {
                    (Provider::Jev, Some(ctx), _) => {
                        run_jev(ctx, &wanted, opts.max_calls, opts.shape).await
                    }
                    (Provider::Haiku, _, Some(h)) => run_haiku(h, &wanted, opts.max_calls).await,
                    _ => wanted
                        .iter()
                        .map(|_| Outcome::skipped("no_backend"))
                        .collect(),
                };
                wanted.iter().map(|c| c.id.clone()).zip(outs).collect()
            }
        };
        all.insert(*p, m);
    }
    all
}

// --- metrics -------------------------------------------------------------------

/// The answer a provider gives at `threshold`: its pick when its score
/// reaches it.
fn answer_at(o: &Outcome, threshold: Option<f64>) -> Option<&String> {
    let p = o.pick.as_ref()?;
    match (threshold, o.score) {
        (Some(t), Some(s)) if s < t => None,
        (Some(_), None) => None,
        _ => Some(p),
    }
}

/// Coverage at a precision: the threshold chosen on dev, applied to the
/// reported cases.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AtPrecision {
    pub target: f64,
    /// Chosen on dev (`None`: dev never reached the target, or had no run).
    pub threshold: Option<f64>,
    pub coverage: Option<f64>,
    pub accuracy_on_answered: Option<f64>,
    pub answered: Shown,
}

/// One provider on one dataset.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProviderMetrics {
    pub provider: &'static str,
    /// Truth cases with a usable outcome.
    pub cases: Shown,
    /// None-cases with a usable outcome.
    pub none_cases: Shown,
    /// Cases with no usable outcome, by reason (gate fallback, failed call,
    /// `max_calls`).
    pub skipped: BTreeMap<String, Shown>,
    /// The abstain threshold in use (BM25: chosen on dev).
    pub threshold: Option<f64>,
    pub answered: Shown,
    pub correct: Shown,
    pub accuracy_on_answered: Option<f64>,
    pub coverage: Option<f64>,
    /// Abstentions on none-cases ÷ none-cases.
    pub abstention_quality: Option<f64>,
    pub at_precision: AtPrecision,
    /// Jev / haiku: the Choice's confidence against whether its answer (an
    /// item or `none`) was right, over every usable case that had one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calibration: Option<Calibration>,
    /// Jev with the noul check: the noul against whether the chosen item
    /// was right.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub noul_calibration: Option<Calibration>,
    /// Calls made (Jev: two for a case the check was asked for).
    pub calls: Shown,
    /// Haiku: answers outside the options (counted as abstentions).
    pub invalid: Shown,
    /// Haiku: calls whose envelope reported no tokens or cost (the totals
    /// leave them out).
    pub usage_unknown: Shown,
    /// Per case, both calls together.
    pub latency_p50_ms: Option<i64>,
    pub latency_p95_ms: Option<i64>,
    pub input_tokens: i64,
    pub cost_microusd: i64,
}

/// Jev against BM25 at BM25's coverage (card J1: "≥ 10 points above BM25
/// at equal coverage"). BM25 answers at its dev-chosen abstain threshold;
/// Jev at the score threshold whose dev coverage is closest to BM25's dev
/// coverage. Both applied to the reported truth cases.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EqualCoverage {
    pub bm25_dev_coverage: Option<f64>,
    /// Chosen on dev.
    pub jev_threshold: Option<f64>,
    pub jev_dev_coverage: Option<f64>,
    /// Truth cases both have a usable outcome for.
    pub cases: Shown,
    pub bm25_accuracy: Option<f64>,
    pub bm25_coverage: Option<f64>,
    pub jev_accuracy: Option<f64>,
    pub jev_coverage: Option<f64>,
    /// Jev − BM25 accuracy on answered, with its bootstrap interval.
    pub gap: Option<f64>,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
}

/// Jev against `claude -p haiku` at haiku's coverage (card J1: "not worse
/// than haiku by more than 3 points"). Haiku answers at its own operating
/// point (its `none`); Jev at the score threshold whose dev coverage is
/// closest to haiku's dev coverage — the same construction as
/// [`EqualCoverage`]. Both applied to the reported truth cases both
/// answered usably.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VsHaiku {
    pub haiku_dev_coverage: Option<f64>,
    /// Chosen on dev.
    pub jev_threshold: Option<f64>,
    pub jev_dev_coverage: Option<f64>,
    /// Truth cases both have a usable outcome for.
    pub cases: Shown,
    pub haiku_accuracy: Option<f64>,
    pub haiku_coverage: Option<f64>,
    pub jev_accuracy: Option<f64>,
    pub jev_coverage: Option<f64>,
    /// Jev − haiku accuracy on answered, with its bootstrap interval.
    pub gap: Option<f64>,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
    /// Per case, over the paired cases.
    pub jev_latency_p50_ms: Option<i64>,
    pub haiku_latency_p50_ms: Option<i64>,
}

/// Accuracy-on-answered difference of two providers, with its bootstrap
/// interval.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Diff {
    pub a: &'static str,
    pub b: &'static str,
    /// Truth cases both have a usable outcome for.
    pub cases: Shown,
    pub diff: Option<f64>,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
    /// `a`, `b`, `no difference` (the interval crosses 0), or `not
    /// comparable` (no paired answers).
    pub better: String,
}

/// One provider in one breakdown cell.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CellMetrics {
    pub provider: &'static str,
    pub accuracy_on_answered: Option<f64>,
    pub coverage: Option<f64>,
    pub abstention_quality: Option<f64>,
    /// Jev / haiku only (see [`ProviderMetrics::calibration`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calibration: Option<Calibration>,
}

/// One breakdown cell.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cell {
    /// `org`, `tracker`, `nl` (prompt × truth title), `code`, `candidates`.
    pub dimension: &'static str,
    pub value: String,
    pub cases: Shown,
    pub none_cases: Shown,
    /// Fewer than [`JUDGE_MIN_CASES`] cases: not judged (routes to fallback).
    pub judged: bool,
    /// Empty when the cell has fewer than 5 cases.
    pub providers: Vec<CellMetrics>,
}

/// One dataset's results.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DatasetReport {
    pub dataset: &'static str,
    /// What is reported: `dev`, `test`, `all` (A), or `all labeled` (H).
    pub scope: String,
    pub cases: Shown,
    pub none_cases: Shown,
    pub providers: Vec<ProviderMetrics>,
    pub diffs: Vec<Diff>,
    pub breakdown: Vec<Cell>,
    /// When both `jev` and `bm25` ran.
    pub equal_coverage: Option<EqualCoverage>,
    /// When both `jev` and `haiku` ran.
    pub vs_haiku: Option<VsHaiku>,
    /// Card J1's acceptance, when `jev` ran: the four registered criteria,
    /// then one line per language cell.
    pub acceptance: Vec<Criterion>,
    /// Over the four registered criteria (not the cells).
    pub acceptance_overall: Option<Verdict>,
}

/// The thresholds chosen on dev.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Thresholds {
    /// BM25 abstains under this score.
    pub bm25_abstain: Option<f64>,
    /// Jev with the noul check: the answer stands at a noul of at least
    /// this (chosen like BM25's: most right answers plus right abstentions).
    pub jev_noul: Option<f64>,
    /// Per provider: the score / confidence at which dev reached
    /// [`TARGET_PRECISION`].
    pub at_precision: BTreeMap<&'static str, Option<f64>>,
    /// BM25's coverage on dev, and the Jev score threshold whose dev
    /// coverage is closest to it ([`EqualCoverage`]).
    pub bm25_dev_coverage: Option<f64>,
    pub jev_at_bm25_coverage: Option<f64>,
    /// Haiku's coverage on dev, and the Jev score threshold whose dev
    /// coverage is closest to it ([`VsHaiku`]).
    pub haiku_dev_coverage: Option<f64>,
    pub jev_at_haiku_coverage: Option<f64>,
}

/// The whole benchmark.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BenchReport {
    pub benchmark: &'static str,
    pub question_version: &'static str,
    pub detector: &'static str,
    pub bm25: &'static str,
    pub schema_version: i64,
    pub days: u32,
    pub since: i64,
    pub split: &'static str,
    pub dev_share_pct: usize,
    pub providers: Vec<&'static str>,
    /// How Jev was asked: `choice` or `choice+noul`.
    pub shape: &'static str,
    pub sizes: Sizes,
    pub recall: Recall,
    pub thresholds: Thresholds,
    pub approximated: Vec<&'static str>,
    pub datasets: Vec<DatasetReport>,
    pub notes: Vec<String>,
}

/// PURE: BM25's abstain threshold on dev: the one that maximises right
/// answers on truth cases plus abstentions on none-cases (ties: the lower).
fn choose_abstain(dev: &[(&BenchCase, &Outcome)]) -> Option<f64> {
    if dev.is_empty() {
        return None;
    }
    let mut ts: Vec<f64> = dev.iter().filter_map(|(_, o)| o.score).collect();
    ts.push(f64::INFINITY);
    ts.sort_by(|a, b| a.total_cmp(b));
    ts.dedup();
    let mut best: Option<(u64, f64)> = None;
    for t in ts {
        let good = dev
            .iter()
            .filter(|(c, o)| match (&c.truth, answer_at(o, Some(t))) {
                (Some(truth), Some(a)) => a == truth,
                (None, None) => true,
                _ => false,
            })
            .count() as u64;
        if best.is_none_or(|(g, _)| good > g) {
            best = Some((good, t));
        }
    }
    best.map(|(_, t)| t)
}

/// PURE: the lowest threshold at which dev's accuracy on answered reaches
/// `target` (the largest coverage there) over at least
/// [`AT_PRECISION_MIN_ANSWERED`] answers.
fn choose_at_precision(dev: &[(&BenchCase, &Outcome)], target: f64) -> Option<f64> {
    let truth: Vec<_> = dev.iter().filter(|(c, _)| c.truth.is_some()).collect();
    let mut ts: Vec<f64> = truth
        .iter()
        .filter(|(_, o)| o.pick.is_some())
        .filter_map(|(_, o)| o.score)
        .collect();
    ts.sort_by(|a, b| a.total_cmp(b));
    ts.dedup();
    for t in ts {
        let (mut n, mut k) = (0u64, 0u64);
        for (c, o) in &truth {
            if let Some(a) = answer_at(o, Some(t)) {
                n += 1;
                k += u64::from(Some(a) == c.truth.as_ref());
            }
        }
        if n >= AT_PRECISION_MIN_ANSWERED && k as f64 / n as f64 >= target {
            return Some(t);
        }
    }
    None
}

/// PURE: the score threshold whose coverage of the truth cases in `rows`
/// comes closest to `target` (ties: the lower threshold, the larger
/// coverage). `None` when no pick carries a score.
fn choose_for_coverage(rows: &[(&BenchCase, &Outcome)], target: f64) -> Option<f64> {
    let truth: Vec<_> = rows.iter().filter(|(c, _)| c.truth.is_some()).collect();
    if truth.is_empty() {
        return None;
    }
    let mut ts: Vec<f64> = truth
        .iter()
        .filter(|(_, o)| o.pick.is_some())
        .filter_map(|(_, o)| o.score)
        .collect();
    ts.sort_by(|a, b| a.total_cmp(b));
    ts.dedup();
    let mut best: Option<(f64, f64)> = None;
    for t in ts {
        let answered = truth
            .iter()
            .filter(|(_, o)| answer_at(o, Some(t)).is_some())
            .count();
        let d = (answered as f64 / truth.len() as f64 - target).abs();
        if best.is_none_or(|(bd, _)| d < bd - 1e-12) {
            best = Some((d, t));
        }
    }
    best.map(|(_, t)| t)
}

/// Accuracy and coverage of `rows` at `threshold`.
fn tally_at(rows: &[(&BenchCase, &Outcome)], threshold: Option<f64>) -> Tally {
    let mut t = Tally::default();
    for (c, o) in rows {
        t.add(c, answer_at(o, threshold));
    }
    t
}

#[derive(Default)]
struct Tally {
    cases: u64,
    none_cases: u64,
    answered: u64,
    correct: u64,
    abstained_none: u64,
}

impl Tally {
    fn add(&mut self, c: &BenchCase, answer: Option<&String>) {
        match &c.truth {
            Some(t) => {
                self.cases += 1;
                if let Some(a) = answer {
                    self.answered += 1;
                    self.correct += u64::from(a == t);
                }
            }
            None => {
                self.none_cases += 1;
                self.abstained_none += u64::from(answer.is_none());
            }
        }
    }

    fn accuracy(&self) -> Option<f64> {
        pct(self.correct, self.answered)
    }

    fn coverage(&self) -> Option<f64> {
        pct(self.answered, self.cases)
    }

    fn abstention(&self) -> Option<f64> {
        pct(self.abstained_none, self.none_cases)
    }
}

fn provider_metrics(
    p: Provider,
    rows: &[(&BenchCase, &Outcome)],
    threshold: Option<f64>,
    at_p: Option<f64>,
) -> ProviderMetrics {
    let mut t = Tally::default();
    let mut skipped: BTreeMap<String, Shown> = BTreeMap::new();
    let mut lat = Vec::new();
    let (mut calls, mut tokens, mut cost) = (0u64, 0i64, 0i64);
    let (mut invalid, mut usage_unknown) = (0u64, 0u64);
    let (mut pn, mut pk, mut pc) = (0u64, 0u64, 0u64);
    let (mut cal, mut noul_cal) = (Vec::new(), Vec::new());
    for (c, o) in rows {
        if o.ran && p.is_model() {
            calls += u64::from(o.calls.max(1));
            if let Some(l) = o.latency_ms {
                lat.push(l);
            }
            tokens += o.input_tokens;
            cost += o.cost_microusd;
            usage_unknown += u64::from(o.usage_unknown);
        }
        if !o.usable() {
            let r = o.reason.clone().unwrap_or_else(|| "not_run".into());
            skipped.entry(r).or_default().0 += 1;
            continue;
        }
        invalid += u64::from(o.invalid);
        if p.is_model() {
            if let Some(conf) = o.choice_confidence {
                cal.push((conf, o.pick == c.truth));
            }
            if let Some(n) = o.noul {
                noul_cal.push((n, o.pick.is_some() && o.pick == c.truth));
            }
        }
        t.add(c, answer_at(o, threshold));
        if let Some(truth) = &c.truth {
            pc += 1;
            if at_p.is_some() {
                if let Some(a) = answer_at(o, at_p) {
                    pn += 1;
                    pk += u64::from(a == truth);
                }
            }
        }
    }
    ProviderMetrics {
        provider: p.as_str(),
        cases: Shown(t.cases),
        none_cases: Shown(t.none_cases),
        skipped,
        threshold: threshold.map(round3),
        answered: Shown(t.answered),
        correct: Shown(t.correct),
        accuracy_on_answered: t.accuracy(),
        coverage: t.coverage(),
        abstention_quality: t.abstention(),
        at_precision: AtPrecision {
            target: TARGET_PRECISION,
            threshold: at_p.map(round3),
            coverage: at_p.and_then(|_| pct(pn, pc)),
            accuracy_on_answered: at_p.and_then(|_| pct(pk, pn)),
            answered: Shown(pn),
        },
        calibration: p.is_model().then(|| Calibration::of(&cal)),
        noul_calibration: (!noul_cal.is_empty()).then(|| Calibration::of(&noul_cal)),
        calls: Shown(calls),
        invalid: Shown(invalid),
        usage_unknown: Shown(usage_unknown),
        latency_p50_ms: percentile(&lat, 50.0),
        latency_p95_ms: percentile(&lat, 95.0),
        input_tokens: tokens,
        cost_microusd: cost,
    }
}

fn org_label(org: Option<i64>, names: &BTreeMap<i64, String>) -> String {
    match org {
        Some(id) => format!(
            "{} (#{id})",
            names.get(&id).cloned().unwrap_or_else(|| "org".into())
        ),
        None => "(no org)".into(),
    }
}

/// PURE: the report from the loaded datasets and every provider's outcomes.
pub fn report(loaded: &Loaded, opts: &BenchOptions, outcomes: &Outcomes) -> BenchReport {
    let mut notes = Vec::new();
    let providers: Vec<Provider> = opts
        .providers
        .iter()
        .copied()
        .filter(|p| outcomes.contains_key(p))
        .collect();
    let rows_of = |p: Provider, keep: &dyn Fn(&BenchCase) -> bool| -> Vec<(&BenchCase, &Outcome)> {
        let m = &outcomes[&p];
        loaded
            .cases
            .iter()
            .filter(|c| keep(c))
            .filter_map(|c| m.get(&c.id).map(|o| (c, o)))
            .collect()
    };
    let is_dev = |c: &BenchCase| c.dataset == Dataset::A && c.dev;

    // Thresholds, on dev.
    let mut th = Thresholds::default();
    let mut abstain: BTreeMap<Provider, Option<f64>> = BTreeMap::new();
    let mut at_p: BTreeMap<Provider, Option<f64>> = BTreeMap::new();
    for &p in &providers {
        let dev: Vec<_> = rows_of(p, &is_dev)
            .into_iter()
            .filter(|(_, o)| o.usable())
            .collect();
        let a = match p {
            Provider::Bm25 => {
                let t = choose_abstain(&dev);
                th.bm25_abstain = t.map(round3);
                if t.is_none() {
                    notes.push("bm25: no dev cases, so no abstain threshold".into());
                }
                t
            }
            Provider::Jev if opts.shape == Shape::ChoiceNoul => {
                let t = choose_abstain(&dev);
                th.jev_noul = t.map(round3);
                if t.is_none() {
                    notes.push(
                        "jev: the noul check needs its threshold from dev; run --split all (or dev) to choose one".into(),
                    );
                }
                t
            }
            _ => None,
        };
        abstain.insert(p, a);
        let ap = if p == Provider::None {
            None
        } else {
            choose_at_precision(&dev, TARGET_PRECISION)
        };
        if p == Provider::Jev && dev.is_empty() {
            notes.push(
                "jev: coverage at precision needs its threshold from dev; run --split all (or dev) to choose one".into(),
            );
        }
        if p != Provider::None {
            th.at_precision.insert(p.as_str(), ap.map(round3));
        }
        at_p.insert(p, ap);
    }
    // Jev at BM25's coverage: BM25's dev coverage, and the Jev threshold
    // whose dev coverage is closest to it.
    let mut eq_threshold: Option<f64> = None;
    let mut eq_jev_dev_coverage: Option<f64> = None;
    let jev_dev_ran = providers.contains(&Provider::Jev)
        && rows_of(Provider::Jev, &is_dev)
            .iter()
            .any(|(_, o)| o.usable());
    let both = providers.contains(&Provider::Bm25) && providers.contains(&Provider::Jev);
    let with_haiku = providers.contains(&Provider::Haiku) && providers.contains(&Provider::Jev);
    // A baseline's dev coverage at its own operating point, the Jev
    // threshold whose dev coverage comes closest to it, and that coverage.
    let at_coverage_of = |base: Provider| -> (Option<f64>, Option<f64>, Option<f64>) {
        let truth_dev = |p: Provider| -> Vec<(&BenchCase, &Outcome)> {
            rows_of(p, &is_dev)
                .into_iter()
                .filter(|(c, o)| o.usable() && c.truth.is_some())
                .collect()
        };
        let target = tally_at(&truth_dev(base), abstain[&base]).coverage();
        let jev = truth_dev(Provider::Jev);
        let t = target.and_then(|target| choose_for_coverage(&jev, target));
        (
            target,
            t,
            t.and_then(|t| tally_at(&jev, Some(t)).coverage()),
        )
    };
    if both {
        let (target, t, cov) = at_coverage_of(Provider::Bm25);
        th.bm25_dev_coverage = target;
        eq_threshold = t;
        th.jev_at_bm25_coverage = t.map(round3);
        eq_jev_dev_coverage = cov;
    }
    let (mut haiku_threshold, mut haiku_jev_dev_coverage) = (None, None);
    if with_haiku {
        let (target, t, cov) = at_coverage_of(Provider::Haiku);
        th.haiku_dev_coverage = target;
        haiku_threshold = t;
        th.jev_at_haiku_coverage = t.map(round3);
        haiku_jev_dev_coverage = cov;
    }
    if opts.split != Split::Test {
        notes.push(format!(
            "the reported cases include dev: thresholds are in-sample there (--split {})",
            opts.split.as_str()
        ));
    }

    let mut datasets = Vec::new();
    let sets: [(Dataset, &'static str, String); 2] = [
        (Dataset::A, "A", opts.split.as_str().to_string()),
        (Dataset::H, "H", "all labeled".to_string()),
    ];
    for (ds, name, scope) in sets {
        let keep = |c: &BenchCase| c.dataset == ds && (ds == Dataset::H || opts.split.keeps(c.dev));
        let cases: Vec<&BenchCase> = loaded.cases.iter().filter(|c| keep(c)).collect();
        if ds == Dataset::H && cases.is_empty() {
            continue;
        }
        let mut pm = Vec::new();
        for &p in &providers {
            let rows = rows_of(p, &keep);
            pm.push(provider_metrics(p, &rows, abstain[&p], at_p[&p]));
        }
        // Differences in accuracy on answered, paired over truth cases.
        let mut diffs = Vec::new();
        let answering: Vec<Provider> = providers
            .iter()
            .copied()
            .filter(|p| *p != Provider::None)
            .collect();
        for (i, &a) in answering.iter().enumerate() {
            for &b in &answering[i + 1..] {
                let (ma, mb) = (&outcomes[&a], &outcomes[&b]);
                let obs: Vec<Paired> = cases
                    .iter()
                    .filter(|c| c.truth.is_some())
                    .filter_map(|c| {
                        let (oa, ob) = (ma.get(&c.id)?, mb.get(&c.id)?);
                        if !oa.usable() || !ob.usable() {
                            return None;
                        }
                        let (xa, xb) = (answer_at(oa, abstain[&a]), answer_at(ob, abstain[&b]));
                        Some(Paired {
                            a_answered: xa.is_some(),
                            a_correct: xa == c.truth.as_ref(),
                            b_answered: xb.is_some(),
                            b_correct: xb == c.truth.as_ref(),
                        })
                    })
                    .collect();
                let ci = bootstrap_acc_diff(&obs, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED);
                diffs.push(Diff {
                    a: a.as_str(),
                    b: b.as_str(),
                    cases: Shown(obs.len() as u64),
                    diff: ci.map(|x| round3(x.0)),
                    lo: ci.map(|x| round3(x.1)),
                    hi: ci.map(|x| round3(x.2)),
                    better: match ci {
                        Some((_, lo, _)) if lo > 0.0 => a.as_str().to_string(),
                        Some((_, _, hi)) if hi < 0.0 => b.as_str().to_string(),
                        Some(_) => "no difference".to_string(),
                        None => "not comparable (no paired answers)".to_string(),
                    },
                });
            }
        }
        // The breakdown.
        type Key = (&'static str, String);
        let dims = |c: &BenchCase| -> Vec<Key> {
            vec![
                ("org", org_label(c.org_id, &loaded.org_names)),
                ("tracker", c.tracker.clone()),
                ("nl", format!("{}×{}", c.nl_prompt, c.nl_title)),
                ("code", c.code.to_string()),
                ("candidates", c.size_bucket().to_string()),
            ]
        };
        type CellAcc = (
            Tally,
            BTreeMap<Provider, Tally>,
            BTreeMap<Provider, Vec<(f64, bool)>>,
        );
        let mut cells: BTreeMap<Key, CellAcc> = BTreeMap::new();
        for c in &cases {
            for k in dims(c) {
                let e = cells.entry(k).or_default();
                match c.truth {
                    Some(_) => e.0.cases += 1,
                    None => e.0.none_cases += 1,
                }
                for &p in &providers {
                    if let Some(o) = outcomes[&p].get(&c.id).filter(|o| o.usable()) {
                        e.1.entry(p).or_default().add(c, answer_at(o, abstain[&p]));
                        if p.is_model() {
                            if let Some(conf) = o.choice_confidence {
                                e.2.entry(p).or_default().push((conf, o.pick == c.truth));
                            }
                        }
                    }
                }
            }
        }
        let order = ["org", "tracker", "nl", "code", "candidates"];
        let mut breakdown: Vec<Cell> = cells
            .into_iter()
            .map(|((dim, value), (n, per, cal))| {
                let shown = n.cases >= SHOW_MIN_CASES;
                Cell {
                    dimension: dim,
                    value,
                    cases: Shown(n.cases),
                    none_cases: Shown(n.none_cases),
                    judged: n.cases >= JUDGE_MIN_CASES,
                    providers: if shown {
                        per.iter()
                            .map(|(p, t)| CellMetrics {
                                provider: p.as_str(),
                                accuracy_on_answered: t.accuracy(),
                                coverage: t.coverage(),
                                abstention_quality: if t.none_cases >= SHOW_MIN_CASES {
                                    t.abstention()
                                } else {
                                    None
                                },
                                calibration: p.is_model().then(|| {
                                    Calibration::of(cal.get(p).map(Vec::as_slice).unwrap_or(&[]))
                                }),
                            })
                            .collect()
                    } else {
                        Vec::new()
                    },
                }
            })
            .collect();
        breakdown.sort_by(|x, y| {
            let pos = |d: &str| order.iter().position(|o| *o == d).unwrap_or(9);
            pos(x.dimension)
                .cmp(&pos(y.dimension))
                .then(y.cases.cmp(&x.cases))
                .then(x.value.cmp(&y.value))
        });
        let equal_coverage = both.then(|| {
            equal_coverage(
                &cases,
                outcomes,
                abstain[&Provider::Bm25],
                eq_threshold,
                th.bm25_dev_coverage,
                eq_jev_dev_coverage,
            )
        });
        let vs_haiku = with_haiku.then(|| {
            vs_haiku(
                &cases,
                outcomes,
                haiku_threshold,
                th.haiku_dev_coverage,
                haiku_jev_dev_coverage,
            )
        });
        let (acceptance, acceptance_overall) =
            match pm.iter().find(|m| m.provider == Provider::Jev.as_str()) {
                Some(jm) => {
                    let (mut crit, overall) =
                        j1_acceptance(jm, equal_coverage.as_ref(), vs_haiku.as_ref(), jev_dev_ran);
                    crit.extend(language_cells(
                        &cases,
                        &outcomes[&Provider::Jev],
                        abstain[&Provider::Jev],
                    ));
                    (crit, Some(overall))
                }
                None => (Vec::new(), None),
            };
        datasets.push(DatasetReport {
            dataset: name,
            scope,
            cases: Shown(cases.iter().filter(|c| c.truth.is_some()).count() as u64),
            none_cases: Shown(cases.iter().filter(|c| c.truth.is_none()).count() as u64),
            providers: pm,
            diffs,
            breakdown,
            equal_coverage,
            vs_haiku,
            acceptance,
            acceptance_overall,
        });
    }
    BenchReport {
        benchmark: "work_link (J1, phase 0, offline)",
        question_version: QUESTION_VERSION,
        detector: nl::DETECTOR_VERSION,
        bm25: bm25::BM25_VERSION,
        schema_version: loaded.schema_version,
        days: opts.days,
        since: opts.since(),
        split: opts.split.as_str(),
        dev_share_pct: DEV_SHARE_PCT,
        providers: providers.iter().map(|p| p.as_str()).collect(),
        shape: opts.shape.as_str(),
        sizes: loaded.sizes.clone(),
        recall: loaded.recall.clone(),
        thresholds: th,
        approximated: APPROXIMATIONS.to_vec(),
        datasets,
        notes,
    }
}

// --- acceptance (card J1) --------------------------------------------------------

/// PURE: Jev against BM25 at BM25's coverage over the truth cases of
/// `cases` both answered usably: BM25 at `bm25_t`, Jev at `jev_t` (chosen
/// on dev; without it Jev's side is empty).
fn equal_coverage(
    cases: &[&BenchCase],
    outcomes: &Outcomes,
    bm25_t: Option<f64>,
    jev_t: Option<f64>,
    bm25_dev_coverage: Option<f64>,
    jev_dev_coverage: Option<f64>,
) -> EqualCoverage {
    let p = paired_with(cases, outcomes, Provider::Bm25, bm25_t, jev_t);
    let (tb, tj, obs, ci) = (p.base, p.jev, p.obs, p.ci);
    EqualCoverage {
        bm25_dev_coverage,
        jev_threshold: jev_t.map(round3),
        jev_dev_coverage,
        cases: Shown(obs.len() as u64),
        bm25_accuracy: tb.accuracy(),
        bm25_coverage: tb.coverage(),
        jev_accuracy: jev_t.and(tj.accuracy()),
        jev_coverage: jev_t.and(tj.coverage()),
        gap: ci.map(|x| round3(x.0)),
        lo: ci.map(|x| round3(x.1)),
        hi: ci.map(|x| round3(x.2)),
    }
}

/// Jev against a baseline over the truth cases both answered usably.
struct PairedWith {
    base: Tally,
    jev: Tally,
    obs: Vec<Paired>,
    /// Jev − the baseline, with its bootstrap interval (none without a Jev
    /// threshold).
    ci: Option<(f64, f64, f64)>,
    base_latency: Vec<i64>,
    jev_latency: Vec<i64>,
}

/// PURE: the baseline `base` at `base_t` and Jev at `jev_t` (chosen on
/// dev; without it Jev's side is empty) over the truth cases of `cases`
/// both answered usably.
fn paired_with(
    cases: &[&BenchCase],
    outcomes: &Outcomes,
    base: Provider,
    base_t: Option<f64>,
    jev_t: Option<f64>,
) -> PairedWith {
    let (bm, jv) = (&outcomes[&base], &outcomes[&Provider::Jev]);
    let mut p = PairedWith {
        base: Tally::default(),
        jev: Tally::default(),
        obs: Vec::new(),
        ci: None,
        base_latency: Vec::new(),
        jev_latency: Vec::new(),
    };
    for c in cases.iter().filter(|c| c.truth.is_some()) {
        let (Some(ob), Some(oj)) = (bm.get(&c.id), jv.get(&c.id)) else {
            continue;
        };
        if !ob.usable() || !oj.usable() {
            continue;
        }
        let xb = answer_at(ob, base_t);
        let xj = jev_t.and_then(|t| answer_at(oj, Some(t)));
        p.base.add(c, xb);
        p.jev.add(c, xj);
        p.base_latency.extend(ob.latency_ms);
        p.jev_latency.extend(oj.latency_ms);
        p.obs.push(Paired {
            a_answered: xj.is_some(),
            a_correct: xj.is_some() && xj == c.truth.as_ref(),
            b_answered: xb.is_some(),
            b_correct: xb.is_some() && xb == c.truth.as_ref(),
        });
    }
    p.ci = jev_t.and_then(|_| bootstrap_acc_diff(&p.obs, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED));
    p
}

/// PURE: Jev against haiku at haiku's coverage ([`VsHaiku`]): haiku at its
/// own operating point, Jev at `jev_t`.
fn vs_haiku(
    cases: &[&BenchCase],
    outcomes: &Outcomes,
    jev_t: Option<f64>,
    haiku_dev_coverage: Option<f64>,
    jev_dev_coverage: Option<f64>,
) -> VsHaiku {
    let p = paired_with(cases, outcomes, Provider::Haiku, None, jev_t);
    VsHaiku {
        haiku_dev_coverage,
        jev_threshold: jev_t.map(round3),
        jev_dev_coverage,
        cases: Shown(p.obs.len() as u64),
        haiku_accuracy: p.base.accuracy(),
        haiku_coverage: p.base.coverage(),
        jev_accuracy: jev_t.and(p.jev.accuracy()),
        jev_coverage: jev_t.and(p.jev.coverage()),
        gap: p.ci.map(|x| round3(x.0)),
        lo: p.ci.map(|x| round3(x.1)),
        hi: p.ci.map(|x| round3(x.2)),
        jev_latency_p50_ms: percentile(&p.jev_latency, 50.0),
        haiku_latency_p50_ms: percentile(&p.base_latency, 50.0),
    }
}

/// PURE: card J1's four registered criteria for Jev's metrics on one
/// dataset, and their overall verdict. `jev_dev_ran`: Jev answered dev
/// cases, so its thresholds could be chosen. The haiku line is judged on
/// `vs` (when haiku ran): the gap at haiku's coverage no lower than
/// −[`ACCEPT_BELOW_HAIKU`] over at least [`JUDGE_MIN_CASES`] paired cases.
fn j1_acceptance(
    m: &ProviderMetrics,
    eq: Option<&EqualCoverage>,
    vs: Option<&VsHaiku>,
    jev_dev_ran: bool,
) -> (Vec<Criterion>, Verdict) {
    let judged = m.cases.0 >= JUDGE_MIN_CASES;
    let small = |n: Shown| {
        if n.0 >= JUDGE_MIN_CASES {
            String::new()
        } else {
            format!(" (n {n} < {JUDGE_MIN_CASES}: not judged)")
        }
    };
    let ap = &m.at_precision;
    let at_precision = match ap.threshold {
        _ if !judged => Verdict::NotJudged,
        None if jev_dev_ran => Verdict::Fail,
        None => Verdict::NotJudged,
        Some(_) => Verdict::all(&[
            Verdict::at_least(ap.accuracy_on_answered, ACCEPT_ACCURACY, true),
            Verdict::at_least(ap.coverage, ACCEPT_COVERAGE, true),
        ]),
    };
    let at_precision_measured = match ap.threshold {
        Some(t) => format!(
            "{} at coverage {} (confidence ≥ {} chosen on dev){}",
            f3(ap.accuracy_on_answered),
            f3(ap.coverage),
            f3(Some(t)),
            small(m.cases)
        ),
        None if jev_dev_ran => format!(
            "dev never reached {TARGET_PRECISION} over {AT_PRECISION_MIN_ANSWERED}+ answers{}",
            small(m.cases)
        ),
        None => "no jev answers on dev to choose a threshold (--split all)".to_string(),
    };
    let (above, above_measured) = match eq {
        None => (Verdict::NotJudged, "bm25 was not run".to_string()),
        Some(e) if e.jev_threshold.is_none() => (
            Verdict::NotJudged,
            "no jev threshold at bm25's dev coverage (--split all)".to_string(),
        ),
        Some(e) => (
            Verdict::at_least(e.gap, ACCEPT_ABOVE_BM25, e.cases.0 >= JUDGE_MIN_CASES),
            format!(
                "jev {} at coverage {} vs bm25 {} at {}: {} [{}, {}] over {} paired cases{}",
                f3(e.jev_accuracy),
                f3(e.jev_coverage),
                f3(e.bm25_accuracy),
                f3(e.bm25_coverage),
                f3(e.gap),
                f3(e.lo),
                f3(e.hi),
                e.cases,
                small(e.cases)
            ),
        ),
    };
    let ms = |x: Option<i64>| x.map(|v| format!("{v} ms")).unwrap_or_else(|| "-".into());
    let (haiku, haiku_measured) = match vs {
        None => (
            Verdict::NotJudged,
            "claude -p haiku was not run (--provider haiku --haiku-host ALIAS, D33)".to_string(),
        ),
        Some(e) if e.haiku_dev_coverage.is_none() || e.jev_threshold.is_none() => (
            Verdict::NotJudged,
            "no jev threshold at haiku's dev coverage (--split all)".to_string(),
        ),
        Some(e) => (
            Verdict::at_least(
                e.gap,
                -ACCEPT_BELOW_HAIKU,
                e.cases.0 >= JUDGE_MIN_CASES,
            ),
            format!(
                "jev {} at coverage {} vs haiku {} at {}: {} [{}, {}] over {} paired cases; p50 {} vs {}{}",
                f3(e.jev_accuracy),
                f3(e.jev_coverage),
                f3(e.haiku_accuracy),
                f3(e.haiku_coverage),
                f3(e.gap),
                f3(e.lo),
                f3(e.hi),
                e.cases,
                ms(e.jev_latency_p50_ms),
                ms(e.haiku_latency_p50_ms),
                small(e.cases)
            ),
        ),
    };
    let criteria = vec![
        Criterion::new(
            format!("accuracy on answered ≥ {ACCEPT_ACCURACY} at coverage ≥ {ACCEPT_COVERAGE}"),
            at_precision_measured,
            at_precision,
        ),
        Criterion::new(
            format!(
                "≥ {} points above bm25 at equal coverage",
                (ACCEPT_ABOVE_BM25 * 100.0).round()
            ),
            above_measured,
            above,
        ),
        Criterion::new(
            format!(
                "not worse than claude -p haiku by more than {} points at haiku's coverage",
                (ACCEPT_BELOW_HAIKU * 100.0).round()
            ),
            haiku_measured,
            haiku,
        ),
        Criterion::new(
            format!("abstention quality ≥ {ACCEPT_ABSTENTION}"),
            format!(
                "{} over {} none-cases, at jev's operating point{}",
                f3(m.abstention_quality),
                m.none_cases,
                small(m.none_cases)
            ),
            Verdict::at_least(
                m.abstention_quality,
                ACCEPT_ABSTENTION,
                m.none_cases.0 >= JUDGE_MIN_CASES,
            ),
        ),
    ];
    let overall = Verdict::all(&criteria.iter().map(|c| c.verdict).collect::<Vec<_>>());
    (criteria, overall)
}

/// PURE: the per-language-cell rule — a `nl` cell (prompt × truth title)
/// more than [`ACCEPT_CELL_BELOW_ENGLISH`] below [`ENGLISH_CELL`] at equal
/// coverage falls back. The cell is taken at Jev's operating point
/// (`threshold`); English is thresholded to the cell's coverage. One
/// criterion per cell: PASS keeps it, FAIL falls back, NOT JUDGED under
/// [`JUDGE_MIN_CASES`] truth cases on either side.
fn language_cells(
    cases: &[&BenchCase],
    jev: &HashMap<String, Outcome>,
    threshold: Option<f64>,
) -> Vec<Criterion> {
    let mut by_cell: BTreeMap<String, Vec<(&BenchCase, &Outcome)>> = BTreeMap::new();
    for c in cases.iter().filter(|c| c.truth.is_some()) {
        if let Some(o) = jev.get(&c.id).filter(|o| o.usable()) {
            by_cell
                .entry(format!("{}×{}", c.nl_prompt, c.nl_title))
                .or_default()
                .push((c, o));
        }
    }
    let empty = Vec::new();
    let en = by_cell.get(ENGLISH_CELL).unwrap_or(&empty);
    let mut cells: Vec<(&String, &Vec<(&BenchCase, &Outcome)>)> = by_cell
        .iter()
        .filter(|(k, _)| k.as_str() != ENGLISH_CELL)
        .collect();
    cells.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    let mut out = Vec::new();
    for (cell, rows) in cells {
        let n = Shown(rows.len() as u64);
        let en_n = Shown(en.len() as u64);
        let criterion = format!(
            "language cell {cell}: at most {} points below {ENGLISH_CELL} at equal coverage",
            (ACCEPT_CELL_BELOW_ENGLISH * 100.0).round()
        );
        if n.0 < SHOW_MIN_CASES {
            out.push(Criterion::new(
                criterion,
                format!("{n} cases"),
                Verdict::NotJudged,
            ));
            continue;
        }
        let t = tally_at(rows, threshold);
        let (acc, cov) = (t.accuracy(), t.coverage());
        let en_t = cov.and_then(|c| choose_for_coverage(en, c));
        let en_tally = en_t.map(|x| tally_at(en, Some(x)));
        let en_acc = en_tally.as_ref().and_then(|x| x.accuracy());
        let en_cov = en_tally.as_ref().and_then(|x| x.coverage());
        let gap = match (en_acc, acc) {
            (Some(e), Some(a)) => Some(round3(e - a)),
            _ => None,
        };
        let judged = n.0 >= JUDGE_MIN_CASES && en_n.0 >= JUDGE_MIN_CASES;
        let verdict = match gap {
            Some(g) if judged => {
                if g > ACCEPT_CELL_BELOW_ENGLISH + 1e-9 {
                    Verdict::Fail
                } else {
                    Verdict::Pass
                }
            }
            _ => Verdict::NotJudged,
        };
        let what = match verdict {
            Verdict::Pass => "keeps",
            Verdict::Fail => "falls back",
            Verdict::NotJudged => "not judged",
        };
        out.push(Criterion::new(
            criterion,
            format!(
                "{} at coverage {} vs {ENGLISH_CELL} {} at {} ({} points; {n} / {en_n} cases) → {what}",
                f3(acc),
                f3(cov),
                f3(en_acc),
                f3(en_cov),
                gap.map(|g| format!("{:.1}", g * 100.0))
                    .unwrap_or_else(|| "-".into()),
            ),
            verdict,
        ));
    }
    out
}

// --- lines ---------------------------------------------------------------------

impl BenchReport {
    /// The report as lines for a terminal.
    pub fn lines(&self) -> Vec<String> {
        let s = &self.sizes;
        let mut v = vec![
            format!("benchmark {} — question {}", self.benchmark, self.question_version),
            format!("detector {}; {}", self.detector, self.bm25),
            format!(
                "window: {} days; schema {}; split: {} (dev = oldest {}% of A by decision time); providers: {}; jev shape: {}",
                self.days,
                self.schema_version,
                self.split,
                self.dev_share_pct,
                self.providers.join(", "),
                self.shape
            ),
            format!(
                "A: {} person-decided links read (manual/started), fleet-typed {} and other-org {} left out; {} cases + {} none-cases; dev {} / test {}",
                s.a_links_read, s.a_fleet_typed, s.a_other_org, s.a_cases, s.a_none_cases, s.a_dev, s.a_test
            ),
        ];
        if s.h_records.0 > 0 {
            v.push(format!(
                "H: {} rows, {} labeled: {} cases + {} none-cases; {} labels outside their candidates (recall misses, not scored){}",
                s.h_records, s.h_labeled, s.h_cases, s.h_none_cases, s.h_label_outside,
                if s.h_session_gone.0 > 0 {
                    format!("; {} rows whose session is gone left out", s.h_session_gone)
                } else {
                    String::new()
                }
            ));
        }
        let th = &self.thresholds;
        v.push(format!(
            "thresholds (chosen on dev): bm25 abstain < {}; at precision {}: {}{}{}",
            f3(th.bm25_abstain),
            TARGET_PRECISION,
            th.at_precision
                .iter()
                .map(|(p, t)| format!("{p} ≥ {}", f3(*t)))
                .collect::<Vec<_>>()
                .join(", "),
            if self.shape == Shape::ChoiceNoul.as_str() {
                format!("; jev noul ≥ {}", f3(th.jev_noul))
            } else {
                String::new()
            },
            if th.bm25_dev_coverage.is_some() {
                format!(
                    "; jev at bm25's dev coverage {}: ≥ {}",
                    f3(th.bm25_dev_coverage),
                    f3(th.jev_at_bm25_coverage)
                )
            } else {
                String::new()
            }
        ));
        if th.haiku_dev_coverage.is_some() {
            v.push(format!(
                "thresholds (chosen on dev): jev at haiku's dev coverage {}: ≥ {}",
                f3(th.haiku_dev_coverage),
                f3(th.jev_at_haiku_coverage)
            ));
        }
        v.push("counts from 1 to 4 show as <5; no prompt or title is printed".into());
        v.push(String::new());
        let r = &self.recall;
        v.push(format!(
            "candidate-set recall (A, every split, {} cases): benchmark set without forcing {} ({}); nudge fence {} ({}); nudge would fire {} ({}); fired with the truth {} ({})",
            r.cases,
            r.bench_set,
            f3(r.bench_set_pct),
            r.nudge_fence,
            f3(r.nudge_fence_pct),
            r.nudge_would_fire,
            f3(r.nudge_would_fire_pct),
            r.nudge_offered_truth,
            f3(r.nudge_offered_truth_pct)
        ));
        for d in &self.datasets {
            v.push(String::new());
            v.push(format!(
                "[{} {}] {} cases, {} none-cases",
                d.dataset, d.scope, d.cases, d.none_cases
            ));
            v.push(format!(
                "  {:<6} {:>7} {:>8} {:>9} {:>8} {:>18} {:>9} {:>11} {:>9} {:>10}  skipped",
                "",
                "cases",
                "answered",
                "acc@ans",
                "coverage",
                "cov@P0.9 (acc)",
                "abstain",
                "p50/p95 ms",
                "tokens",
                "cost"
            ));
            for p in &d.providers {
                let skipped = if p.skipped.is_empty() {
                    "-".to_string()
                } else {
                    p.skipped
                        .iter()
                        .map(|(k, n)| format!("{k} {n}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                v.push(format!(
                    "  {:<6} {:>7} {:>8} {:>9} {:>8} {:>18} {:>9} {:>11} {:>9} {:>10}  {}",
                    p.provider,
                    p.cases.to_string(),
                    p.answered.to_string(),
                    f3(p.accuracy_on_answered),
                    f3(p.coverage),
                    format!(
                        "{} ({})",
                        f3(p.at_precision.coverage),
                        f3(p.at_precision.accuracy_on_answered)
                    ),
                    f3(p.abstention_quality),
                    match (p.latency_p50_ms, p.latency_p95_ms) {
                        (Some(a), Some(b)) => format!("{a}/{b}"),
                        _ => "-".into(),
                    },
                    p.input_tokens,
                    crate::service::decide::fmt_usd(p.cost_microusd),
                    skipped
                ));
            }
            for x in &d.diffs {
                v.push(format!(
                    "  acc@ans {} − {} over {} paired cases: {} [{}, {}] → {}",
                    x.a,
                    x.b,
                    x.cases,
                    f3(x.diff),
                    f3(x.lo),
                    f3(x.hi),
                    x.better
                ));
            }
            if !d.diffs.is_empty() {
                v.push(format!(
                    "  (bootstrap 95% interval, {BOOTSTRAP_RESAMPLES} resamples, seed {BOOTSTRAP_SEED:#x}; an interval across 0 is no difference)"
                ));
            }
            for p in &d.providers {
                if let Some(c) = &p.calibration {
                    v.push(format!(
                        "  calibration {} (choice confidence, 10 equal-width bins): {}",
                        p.provider,
                        c.line()
                    ));
                }
                if let Some(c) = &p.noul_calibration {
                    v.push(format!(
                        "  calibration {} (noul of the chosen item): {}",
                        p.provider,
                        c.line()
                    ));
                }
            }
            for p in d
                .providers
                .iter()
                .filter(|p| p.invalid.0 > 0 || p.usage_unknown.0 > 0)
            {
                v.push(format!(
                    "  {}: {} answers outside the options (invalid: counted as abstentions); {} calls reported no usage (the tokens and cost leave them out)",
                    p.provider, p.invalid, p.usage_unknown
                ));
            }
            if let Some(e) = &d.vs_haiku {
                v.push(format!(
                    "  at haiku's coverage (dev {}; jev ≥ {} gave {} on dev): jev {} at coverage {} vs haiku {} at {} over {} paired cases: {} [{}, {}]",
                    f3(e.haiku_dev_coverage),
                    f3(e.jev_threshold),
                    f3(e.jev_dev_coverage),
                    f3(e.jev_accuracy),
                    f3(e.jev_coverage),
                    f3(e.haiku_accuracy),
                    f3(e.haiku_coverage),
                    e.cases,
                    f3(e.gap),
                    f3(e.lo),
                    f3(e.hi)
                ));
            }
            if let Some(e) = &d.equal_coverage {
                v.push(format!(
                    "  at bm25's coverage (dev {}; jev ≥ {} gave {} on dev): jev {} at coverage {} vs bm25 {} at {} over {} paired cases: {} [{}, {}]",
                    f3(e.bm25_dev_coverage),
                    f3(e.jev_threshold),
                    f3(e.jev_dev_coverage),
                    f3(e.jev_accuracy),
                    f3(e.jev_coverage),
                    f3(e.bm25_accuracy),
                    f3(e.bm25_coverage),
                    e.cases,
                    f3(e.gap),
                    f3(e.lo),
                    f3(e.hi)
                ));
            }
            v.push(format!(
                "  breakdown (a cell under {JUDGE_MIN_CASES} cases is not judged; acc@ans / coverage / abstain):"
            ));
            for c in &d.breakdown {
                let mut line = format!(
                    "    {:<10} {:<22} cases {:>5} none {:>5}{}",
                    c.dimension,
                    c.value,
                    c.cases.to_string(),
                    c.none_cases.to_string(),
                    if c.judged { "" } else { "  not judged" }
                );
                for m in &c.providers {
                    line.push_str(&format!(
                        "  {} {}/{}/{}",
                        m.provider,
                        f2(m.accuracy_on_answered),
                        f2(m.coverage),
                        f2(m.abstention_quality)
                    ));
                    if let Some(ece) = m.calibration.as_ref().and_then(|c| c.ece) {
                        line.push_str(&format!(" ece {}", f2(Some(ece))));
                    }
                }
                v.push(line);
            }
            if let Some(overall) = d.acceptance_overall {
                v.push(format!(
                    "  acceptance, card J1 (assist; registered in the test map): {}",
                    overall.as_str()
                ));
                for c in &d.acceptance {
                    v.push(format!("  {}", c.line()));
                }
            }
        }
        if !self.notes.is_empty() {
            v.push(String::new());
            for n in &self.notes {
                v.push(format!("note: {n}"));
            }
        }
        v.push(String::new());
        v.push("approximated:".into());
        for a in &self.approximated {
            v.push(format!("  - {a}"));
        }
        v
    }
}
