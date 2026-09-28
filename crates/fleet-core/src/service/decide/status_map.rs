//! J3 `status_map`: the first use case of the decision envelope (decision
//! D32; the test map's card J3). An Asana task's status comes from its
//! section: the map a person confirmed (`settings.section_map`), else the
//! one the probe inferred with the keyword rule
//! ([`crate::service::trackers::asana::infer_section`] →
//! `config.section_map`), else `todo`. The names the rule cannot classify
//! (`config.unmapped_sections`) silently become `todo`. This adapter asks the
//! decision model, one Choice question per section, which category such a
//! section is — and nothing more:
//!
//! * **Shadow / assist only.** In `shadow` it also asks about the sections
//!   the rule DID map, to measure agreement with the rule (the run's
//!   baseline). In `assist` it asks about unmapped sections only, and the
//!   answers are **proposals** `fleet-hub decide proposals` lists with the
//!   exact command a person runs to apply them. Nothing here ever writes a
//!   tracker's config or settings: `auto` does not exist.
//! * **What is sent.** The provider (`asana`), the section's name and the
//!   names of its board's sections in order (at most
//!   [`MAX_PROJECT_SECTIONS`]) — names only, through the envelope's
//!   redaction. No task, no title, no project name.
//! * **What is recorded.** A run per question, about subject
//!   `tracker_section` `<tracker id>:<section id>`, where the section id is
//!   an HMAC of the name under the local fingerprint key ([`section_id`]):
//!   never the name. The proposals view maps it back from the tracker's own
//!   stored config.
//! * **Bounded.** At most [`MAX_ASKS_PER_RUN`] questions a run; a section
//!   asked in the last [`REASK_DAYS`] days with the same input fingerprint,
//!   question version, mode and model is not asked again; the sync tick
//!   runs it at most once a day per tracker (sooner when its sections
//!   change) and off the sync's path ([`StatusMapTrigger`]).
//! * **Follow-up.** When a person's `settings.section_map` later holds a
//!   section, its latest answered assist run — the one a person was shown —
//!   is marked `confirmed` (same category) or `corrected` (to theirs) —
//!   [`record_followups`], called where the settings are updated. A shadow
//!   answer nobody saw is never marked. A proposal nobody decided before a newer one
//!   for the same section arrived is marked `ignored`.
//! * **Deciding one proposal** ([`decide_proposal`], by its run id):
//!   *apply* (its category — `not_planned` applies as `done`), *apply as*
//!   another category (a correction), both through `work_admin update`
//!   like any section map change, or *reject* (`rejected`; the section
//!   stays unmapped, and the answer is not proposed again until a new one
//!   exists on another input, question version or model).
//!
//! Jira, Linear and GitHub carry exact status categories from the tracker
//! itself: a rule wins there and this adapter never looks at them.

use super::{
    decide, fingerprint, gate_at, DecideCtx, DecideRequest, Fallback, Feature, FeatureMode,
    JevRequest, Mode, Question,
};
use crate::ipc_error::{lock, IpcError};
use crate::service::settings;
use crate::service::trackers::sync::TrackerPass;
use crate::store::{DecisionRunRow, Secret, Store, TrackerRow};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "status_map.v1";
/// What a run is about: one section of one tracker.
pub const SUBJECT_KIND: &str = "tracker_section";
/// Below this confidence an answer is recorded, not proposed.
pub const MIN_CONFIDENCE: f64 = 0.5;
/// Questions one run asks at most (the rest wait for the next run).
pub const MAX_ASKS_PER_RUN: usize = 40;
/// Section names of the board sent with a question, at most.
pub const MAX_PROJECT_SECTIONS: usize = 30;
/// A section decided this recently, on the same input, is not asked again.
pub const REASK_DAYS: i64 = 14;
/// The sync tick runs the adapter at most this often per tracker (unless
/// the tracker's sections changed).
pub const RUN_EVERY_SECS: i64 = 86_400;
/// The baseline of a section the keyword rule did not classify.
pub const NO_RULE: &str = "none";
/// The option that proposes nothing.
pub const UNSURE: &str = "unsure";
/// The provider this adapter reads.
pub const PROVIDER: &str = "asana";
/// The categories a section map takes (what a person may apply).
pub const SECTION_CATEGORIES: [&str; 3] = ["todo", "in_progress", "done"];
/// The follow-up of a proposal a person said "not this" to.
pub const FOLLOWUP_REJECTED: &str = "rejected";

/// The instruction, read literally: the exact condition and what to use.
pub const INSTRUCTIONS: &str = "state.section is the name of one section (a column) of an Asana \
     project board, in lower case. state.project_sections lists every section of that board in \
     board order, first to last. Decide which status category the tasks placed in state.section \
     are in, using only the section's name and its position among state.project_sections: \
     sections placed after an in-progress section are usually later stages, and the first \
     sections are usually work not started. Choose unsure when the name and the position do not \
     settle it.";

/// The options, each with its criterion.
pub const OPTIONS: [(&str, &str); 5] = [
    (
        "todo",
        "The tasks are not started yet: a backlog, inbox, to do, ideas, next up, planned, triage \
         or waiting-to-start section.",
    ),
    (
        "in_progress",
        "The tasks are being worked on, reviewed, tested or checked before they are finished.",
    ),
    (
        "done",
        "The tasks are finished: done, shipped, released, deployed, merged or closed.",
    ),
    (
        "not_planned",
        "The tasks will not be done: won't do, cancelled, rejected, declined, duplicate or \
         abandoned.",
    ),
    (
        UNSURE,
        "The name and the position do not show which of todo, in_progress, done or not_planned \
         applies.",
    ),
];

/// PURE: the question (the same for every section).
pub fn question() -> Question {
    Question::Choice {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria: OPTIONS
            .iter()
            .map(|(k, d)| (k.to_string(), Some(Value::String(d.to_string()))))
            .collect(),
    }
}

/// PURE: the state for one section: names only.
pub fn state(section: &str, project_sections: &[String]) -> Value {
    json!({
        "provider": PROVIDER,
        "section": section,
        "project_sections": project_sections,
    })
}

/// PURE: what a person's map would hold for an answer: `not_planned` is
/// fleet's `done` (a section map takes todo / in_progress / done), `unsure`
/// is nothing.
pub fn applied_category(answer: &str) -> Option<&'static str> {
    match answer {
        "todo" => Some("todo"),
        "in_progress" => Some("in_progress"),
        "done" | "not_planned" => Some("done"),
        _ => None,
    }
}

/// PURE: a section's id: 16 hex digits of an HMAC of the tracker and the
/// (lower-case) name under the local fingerprint key. Stable across probes;
/// without the key a guessed name cannot be confirmed from the record.
pub fn section_id(fp_key: &Secret, tracker_id: i64, name: &str) -> String {
    let mac = super::hmac_sha256_hex(
        fp_key.expose().as_bytes(),
        format!("status_map.section\n{tracker_id}\n{name}").as_bytes(),
    );
    mac[..16].to_string()
}

/// PURE: the run's subject id for a section.
pub fn subject_id(tracker_id: i64, section_id: &str) -> String {
    format!("{tracker_id}:{section_id}")
}

/// PURE: the prefix of every subject id of `tracker_id`.
pub fn subject_prefix(tracker_id: i64) -> String {
    format!("{tracker_id}:")
}

/// PURE: the board around `name`: the first project whose sections hold it,
/// at most [`MAX_PROJECT_SECTIONS`] names, a window around it when longer.
pub fn board_of(row: &TrackerRow, name: &str) -> Vec<String> {
    let Some(names) = row
        .config
        .project_sections
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.iter().any(|x| x == name))
    else {
        return Vec::new();
    };
    board_window(names, name)
}

/// PURE: at most [`MAX_PROJECT_SECTIONS`] of a board's `names`: all of them,
/// or a window around `name` when longer (from the start when `name` is not
/// among them).
pub fn board_window(names: &[String], name: &str) -> Vec<String> {
    if names.len() <= MAX_PROJECT_SECTIONS {
        return names.to_vec();
    }
    let at = names.iter().position(|x| x == name).unwrap_or(0);
    let start = at
        .saturating_sub(MAX_PROJECT_SECTIONS / 2)
        .min(names.len() - MAX_PROJECT_SECTIONS);
    names[start..start + MAX_PROJECT_SECTIONS].to_vec()
}

/// PURE: the request for one section: its (lower-case) name and its
/// board's section names in order ([`board_window`]), and [`question`].
/// What the adapter sends, and what the offline benchmark
/// (`bench::status_map`) sends for a labeled section.
pub fn question_for(section: &str, project_sections: &[String]) -> JevRequest {
    JevRequest {
        state: state(section, &board_window(project_sections, section)),
        question: question(),
    }
}

/// PURE: the sections to ask about in `mode`, with their baselines: the
/// unmapped ones (`none`), and in `shadow` the rule-mapped ones too (the
/// rule's category). A section in the person's map is theirs: never asked.
pub fn sections_to_ask(row: &TrackerRow, mode: Mode) -> Vec<(String, String)> {
    let person = &row.settings.section_map;
    let mut out: Vec<(String, String)> = row
        .config
        .unmapped_sections
        .iter()
        .filter(|n| !person.contains_key(*n))
        .map(|n| (n.clone(), NO_RULE.to_string()))
        .collect();
    if mode == Mode::Shadow {
        for (n, c) in &row.config.section_map {
            if !person.contains_key(n) && !out.iter().any(|(x, _)| x == n) {
                out.push((n.clone(), c.clone()));
            }
        }
    }
    out
}

/// A run that sent a request and got an answer the envelope checked (or
/// found wanting): asking again on the same input would repeat it.
fn decided(r: &DecisionRunRow) -> bool {
    r.called
        && matches!(
            r.fallback.as_deref(),
            None | Some("low_confidence") | Some("invalid_answer")
        )
}

/// Fallbacks after which the rest of a run would only record the same
/// refusal: stop.
fn stops_the_run(f: Fallback) -> bool {
    matches!(
        f,
        Fallback::NotOwner
            | Fallback::FlagOff
            | Fallback::OrgOff
            | Fallback::ModeOff
            | Fallback::NoKey
            | Fallback::BreakerOpen
            | Fallback::Budget
            | Fallback::RateLimited
    )
}

/// What one run did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunReport {
    pub tracker_id: i64,
    /// The gate refused before anything: no question, no row.
    pub gated: Option<Fallback>,
    pub mode: Option<Mode>,
    /// Questions put to the envelope (each one a recorded run).
    pub asked: usize,
    /// Of those, answers the adapter may use (shadow: compare; assist:
    /// propose).
    pub usable: usize,
    /// Sections not asked: decided recently on the same input.
    pub skipped_recent: usize,
    /// Sections not asked (assist): a person rejected the answer on the
    /// same input, question version and pinned model.
    #[serde(default)]
    pub skipped_rejected: usize,
    /// Sections left for the next run (over [`MAX_ASKS_PER_RUN`]).
    pub deferred: usize,
    /// The fallback that stopped the run early.
    pub stopped: Option<Fallback>,
}

/// One section's question, ready.
struct Ask {
    subject: String,
    baseline: String,
    request: JevRequest,
}

/// Ask the decision model about tracker `tracker_id`'s sections (see the
/// module docs). An error only when the tracker cannot be read; every call's
/// failure is a recorded fallback, never an error. Holds the store lock
/// only for reads, never across a call.
pub async fn propose_for_tracker(ctx: &DecideCtx, tracker_id: i64) -> Result<RunReport, IpcError> {
    let now = ctx.now();
    let mut report = RunReport {
        tracker_id,
        ..Default::default()
    };
    let (row, mode, asks) = {
        let s = lock(&ctx.store)?;
        let row = s.require_tracker(tracker_id)?;
        if row.provider != PROVIDER {
            return Ok(report);
        }
        let mode = match gate_at(&s, Feature::StatusMap, row.org_id, now) {
            Ok(m) => m,
            Err(f) => {
                report.gated = Some(f);
                return Ok(report);
            }
        };
        let sections = sections_to_ask(&row, mode);
        if sections.is_empty() {
            report.mode = Some(mode);
            return Ok(report);
        }
        let fp_key = s.decision_fp_key()?;
        let model = settings::get_string(&s, settings::DECIDE_JEV_MODEL);
        // Every run of the tracker's sections: the recent ones decide a
        // re-ask; a rejection holds (in assist) for as long as the input,
        // the question and the pinned model stay the same.
        let runs = s.decision_runs_for_subjects(
            Feature::StatusMap.as_str(),
            SUBJECT_KIND,
            &subject_prefix(tracker_id),
            None,
        )?;
        let since = now - REASK_DAYS * 86_400;
        let mut asks = Vec::new();
        for (name, baseline) in sections {
            let request = question_for(&name, &board_of(&row, &name));
            let subject = subject_id(tracker_id, &section_id(&fp_key, tracker_id, &name));
            let fp = fingerprint(&fp_key, &request.redacted());
            let same_input = |r: &DecisionRunRow| {
                r.subject_id == subject
                    && r.input_fp.as_deref() == Some(fp.as_str())
                    && r.question_version == QUESTION_VERSION
            };
            let seen = runs.iter().any(|r| {
                r.at >= since
                    && same_input(r)
                    && r.mode == mode.as_str()
                    && decided(r)
                    && (model == "jev-latest" || r.model_version.as_deref() == Some(model.as_str()))
            });
            if seen {
                report.skipped_recent += 1;
                continue;
            }
            // A person said "not this" to an answer on the same input: with
            // a pinned model the same answer would come back. (Under
            // `jev-latest` the section is asked on the usual schedule, and
            // the proposals view hides an answer of the rejected model.)
            let rejected = mode == Mode::Assist
                && model != "jev-latest"
                && runs.iter().any(|r| {
                    same_input(r)
                        && r.followup.as_deref() == Some(FOLLOWUP_REJECTED)
                        && r.model_version.as_deref() == Some(model.as_str())
                });
            if rejected {
                report.skipped_rejected += 1;
                continue;
            }
            asks.push(Ask {
                subject,
                baseline,
                request,
            });
        }
        (row, mode, asks)
    };
    report.mode = Some(mode);
    report.deferred = asks.len().saturating_sub(MAX_ASKS_PER_RUN);
    for ask in asks.into_iter().take(MAX_ASKS_PER_RUN) {
        let subject = ask.subject;
        let out = decide(
            ctx,
            DecideRequest {
                feature: Feature::StatusMap,
                subject_kind: SUBJECT_KIND.into(),
                subject_id: subject.clone(),
                org_id: row.org_id,
                request: ask.request,
                baseline: Some(ask.baseline),
                question_version: QUESTION_VERSION.into(),
                min_confidence: Some(MIN_CONFIDENCE),
            },
        )
        .await;
        report.asked += 1;
        if out.usable().is_some() {
            report.usable += 1;
        }
        // A new proposal takes the place of an older one nobody decided:
        // that one is `ignored` (the view only ever offers the latest).
        if let (Some(_), Some(id)) = (out.proposal(), out.run_id) {
            if let Ok(s) = lock(&ctx.store) {
                if let Err(e) = s.supersede_decision_runs(
                    Feature::StatusMap.as_str(),
                    SUBJECT_KIND,
                    &subject,
                    Mode::Assist.as_str(),
                    id,
                    now,
                ) {
                    tracing::warn!(
                        tracker_id,
                        "[decide] ignored follow-up not recorded: {}",
                        e.message
                    );
                }
            }
        }
        if let Some(f) = out.fallback.filter(|f| stops_the_run(*f)) {
            report.stopped = Some(f);
            break;
        }
    }
    Ok(report)
}

/// PURE: the latest run per subject among `runs` (newest first) that `keep`
/// accepts.
fn latest_per_subject(
    runs: &[DecisionRunRow],
    keep: impl Fn(&DecisionRunRow) -> bool,
) -> BTreeMap<&str, &DecisionRunRow> {
    let mut out = BTreeMap::new();
    for r in runs.iter().filter(|r| keep(r)) {
        out.entry(r.subject_id.as_str()).or_insert(r);
    }
    out
}

/// An answer the adapter may use: no fallback, an answer.
fn answered(r: &DecisionRunRow) -> bool {
    r.fallback.is_none() && r.answer.is_some()
}

/// After a person's `settings.section_map` changed on `row`: mark the latest
/// answered ASSIST run of every section their map now holds `confirmed`
/// (the answer applies as their category) or `corrected` (to theirs), once.
/// Returns the runs marked. Never writes a tracker.
///
/// A shadow answer is never marked: nobody saw it, so a person's map
/// neither confirmed nor corrected it, and a follow-up is a person's
/// response to what they were shown (D34: person counts come from person
/// decisions only; D37). The shadow comparison is the run's baseline.
pub fn record_followups(s: &Store, row: &TrackerRow, now: i64) -> Result<usize, IpcError> {
    if row.provider != PROVIDER || row.settings.section_map.is_empty() {
        return Ok(0);
    }
    let runs = s.decision_runs_for_subjects(
        Feature::StatusMap.as_str(),
        SUBJECT_KIND,
        &subject_prefix(row.id),
        None,
    )?;
    if runs.is_empty() {
        return Ok(0);
    }
    let latest = latest_per_subject(&runs, |r| {
        r.mode == Mode::Assist.as_str()
            && answered(r)
            && r.answer.as_deref().and_then(applied_category).is_some()
    });
    let fp_key = s.decision_fp_key()?;
    let mut marked = 0;
    for (name, theirs) in &row.settings.section_map {
        let subject = subject_id(row.id, &section_id(&fp_key, row.id, name));
        let Some(r) = latest.get(subject.as_str()) else {
            continue;
        };
        if r.followup.is_some() {
            continue;
        }
        let ours = r.answer.as_deref().and_then(applied_category);
        let done = if ours == Some(theirs.as_str()) {
            s.set_decision_followup(r.id, "confirmed", None, now)?
        } else {
            s.set_decision_followup(r.id, "corrected", Some(theirs), now)?
        };
        marked += usize::from(done);
    }
    Ok(marked)
}

/// PURE: the section map a person would confirm on `row` with `sets`
/// (name → todo | in_progress | done) added: their confirmed map, or — not
/// confirmed yet — the probe's inferred map under their entries, as
/// Settings → Work's Confirm does.
pub fn merged_section_map(row: &TrackerRow, sets: &[(String, String)]) -> BTreeMap<String, String> {
    let mut map = if row.settings.section_map_confirmed {
        BTreeMap::new()
    } else {
        row.config.section_map.clone()
    };
    for (k, v) in &row.settings.section_map {
        map.insert(k.clone(), v.clone());
    }
    for (k, v) in sets {
        map.insert(k.trim().to_lowercase(), v.clone());
    }
    map
}

/// PURE: the `work_admin` arguments that confirm `map` on `row` (the other
/// settings kept).
pub fn apply_args(row: &TrackerRow, map: &BTreeMap<String, String>) -> Value {
    let mut settings = row.settings.clone();
    settings.section_map = map.clone();
    settings.section_map_confirmed = true;
    json!({
        "action": "update",
        "tracker_id": row.id,
        "settings": settings,
    })
}

/// One proposal: a section, the category the model chose, and what a
/// person's map would hold for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionProposal {
    /// The section's name, from the tracker's stored config.
    pub section: String,
    /// The model's option.
    pub answer: String,
    /// What applying it puts in the map (`not_planned` → `done`); `None`
    /// for `unsure`.
    pub applies_as: Option<String>,
    pub confidence: Option<f64>,
    pub run_id: i64,
    pub at: i64,
    /// The person's category, when their map already holds the section.
    pub person: Option<String>,
    pub followup: Option<String>,
    /// The why: the answer's two most probable options, most probable
    /// first (empty when the answer carried no distribution).
    #[serde(default)]
    pub top: Vec<(String, f64)>,
}

impl SectionProposal {
    /// Still up to a person: not in their map, no follow-up yet.
    pub fn pending(&self) -> bool {
        self.person.is_none() && self.followup.is_none()
    }
}

/// PURE: the two most probable options of `run`, most probable first
/// (ties by name).
fn top_two(run: &DecisionRunRow) -> Vec<(String, f64)> {
    let mut all: Vec<(String, f64)> = run
        .probabilities
        .iter()
        .flatten()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    all.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    all.truncate(2);
    all
}

/// One shadow comparison: the keyword rule and the model on a section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShadowLine {
    pub section: String,
    /// The rule's category, or `none` where it abstained.
    pub rule: String,
    pub model: String,
    pub confidence: Option<f64>,
    pub run_id: i64,
}

impl ShadowLine {
    pub fn agreed(&self) -> bool {
        self.rule == self.model
    }
}

/// A person's way to apply a tracker's pending proposals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApplyCommand {
    /// `fleet-hub tracker section-map <id> --set 'name=category' …`.
    pub cli: String,
    /// The same as `work_admin` arguments (master token).
    pub work_admin: Value,
}

/// A tracker's proposals and shadow comparisons.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackerProposals {
    pub tracker_id: i64,
    pub name: String,
    pub org_id: Option<i64>,
    /// `decide.jev.status_map` now.
    pub mode: String,
    /// The latest usable assist answer per section.
    pub proposals: Vec<SectionProposal>,
    /// The latest usable shadow answer per section.
    pub shadow: Vec<ShadowLine>,
    /// Runs about sections the tracker's config no longer names.
    pub unknown_sections: usize,
    /// Sections whose latest answer a person rejected (on the same input,
    /// question version and model): not shown until a new answer exists.
    #[serde(default)]
    pub rejected: usize,
    /// Present when a proposal is pending (not `unsure`, not in the
    /// person's map yet).
    pub apply: Option<ApplyCommand>,
}

impl TrackerProposals {
    /// `(compared, agreed)` over the sections the rule classified.
    pub fn shadow_agreement(&self) -> (usize, usize) {
        let ruled: Vec<&ShadowLine> = self.shadow.iter().filter(|l| l.rule != NO_RULE).collect();
        (ruled.len(), ruled.iter().filter(|l| l.agreed()).count())
    }

    /// The CLI's lines.
    pub fn lines(&self) -> Vec<String> {
        let conf = |c: Option<f64>| c.map(|c| format!("{c:.2}")).unwrap_or_else(|| "-".into());
        let mut out = vec![format!(
            "tracker {} {:?}  org {}  status_map mode {}",
            self.tracker_id,
            self.name,
            self.org_id
                .map(|o| o.to_string())
                .unwrap_or_else(|| "-".into()),
            self.mode
        )];
        if self.proposals.is_empty() {
            out.push("  no assist proposals".into());
        } else {
            out.push("  proposals (assist):".into());
            for p in &self.proposals {
                let applies = match (&p.applies_as, p.answer.as_str()) {
                    (None, _) => "  (proposes nothing)".to_string(),
                    (Some(a), ans) if a != ans => format!("  (applies as {a})"),
                    _ => String::new(),
                };
                let state = match (&p.person, &p.followup) {
                    (Some(c), Some(f)) => format!("  [your map: {c}, {f}]"),
                    (Some(c), None) => format!("  [your map: {c}]"),
                    _ => String::new(),
                };
                out.push(format!(
                    "    {:?} → {} ({}){applies}{state}  run {}",
                    p.section,
                    p.answer,
                    conf(p.confidence),
                    p.run_id
                ));
            }
        }
        if !self.shadow.is_empty() {
            let (compared, agreed) = self.shadow_agreement();
            let abstained = self.shadow.iter().filter(|l| l.rule == NO_RULE).count();
            out.push(format!(
                "  shadow: the keyword rule and the model agree on {agreed} of {compared} \
                 sections the rule classified; {abstained} answered where the rule abstained"
            ));
            for l in &self.shadow {
                out.push(format!(
                    "    {:?}  rule {}  model {} ({}){}  run {}",
                    l.section,
                    l.rule,
                    l.model,
                    conf(l.confidence),
                    if l.rule == NO_RULE {
                        ""
                    } else if l.agreed() {
                        "  agree"
                    } else {
                        "  DIFFER"
                    },
                    l.run_id
                ));
            }
        }
        if self.unknown_sections > 0 {
            out.push(format!(
                "  {} run(s) about sections this tracker no longer lists (test it to refresh)",
                self.unknown_sections
            ));
        }
        if self.rejected > 0 {
            out.push(format!(
                "  {} rejected proposal(s) hidden until a new answer (another input, question or \
                 model)",
                self.rejected
            ));
        }
        if let Some(a) = &self.apply {
            out.push(format!("  apply (a person decides): {}", a.cli));
        }
        if self.proposals.iter().any(SectionProposal::pending) {
            out.push(
                "  or one at a time: fleet-hub decide proposals apply <run> [--as CATEGORY] | \
                 reject <run>"
                    .into(),
            );
        }
        out
    }
}

/// PURE: the `fleet-hub tracker section-map` line for `sets`.
pub fn apply_cli(tracker_id: i64, sets: &[(String, String)]) -> String {
    let mut cli = format!("fleet-hub tracker section-map {tracker_id}");
    for (name, cat) in sets {
        cli.push_str(" --set ");
        cli.push_str(&crate::shell::quote(&format!("{name}={cat}")));
    }
    cli
}

/// PURE: subject id → section name, for every section `row`'s stored
/// config or its person's map names. The only way back from a run to a
/// name: the record holds none.
fn section_names(fp_key: &Secret, row: &TrackerRow) -> HashMap<String, String> {
    row.config
        .unmapped_sections
        .iter()
        .chain(row.config.section_map.keys())
        .chain(row.settings.section_map.keys())
        .map(|n| {
            (
                subject_id(row.id, &section_id(fp_key, row.id, n)),
                n.clone(),
            )
        })
        .collect()
}

/// PURE: a person rejected `r`'s answer — `r` itself, or another run of the
/// same section on the same input, question version and model (the same
/// answer again). A new input, question or model is a new answer.
fn was_rejected(runs: &[DecisionRunRow], r: &DecisionRunRow) -> bool {
    runs.iter().any(|x| {
        x.followup.as_deref() == Some(FOLLOWUP_REJECTED)
            && x.subject_id == r.subject_id
            && x.input_fp == r.input_fp
            && x.question_version == r.question_version
            && x.model_version == r.model_version
    })
}

/// Read-only: every Asana tracker's (or just `tracker`'s) latest proposals
/// and shadow comparisons. Section names come from the tracker's stored
/// config, matched through [`section_id`]; `decision_runs` holds none.
pub fn proposals(s: &Store, tracker: Option<i64>) -> Result<Vec<TrackerProposals>, IpcError> {
    let rows: Vec<TrackerRow> = match tracker {
        Some(id) => vec![s.require_tracker(id)?],
        None => s.list_trackers()?,
    };
    let mode = FeatureMode::of(s, Feature::StatusMap).as_str().to_string();
    let mut out = Vec::new();
    for row in rows.into_iter().filter(|r| r.provider == PROVIDER) {
        let runs = s.decision_runs_for_subjects(
            Feature::StatusMap.as_str(),
            SUBJECT_KIND,
            &subject_prefix(row.id),
            None,
        )?;
        let mut tp = TrackerProposals {
            tracker_id: row.id,
            name: row.name.clone(),
            org_id: row.org_id,
            mode: mode.clone(),
            proposals: Vec::new(),
            shadow: Vec::new(),
            unknown_sections: 0,
            rejected: 0,
            apply: None,
        };
        if runs.is_empty() {
            out.push(tp);
            continue;
        }
        // Runs exist, so the key does (the envelope made it): a read-only
        // store reads it without writing.
        let fp_key = s.decision_fp_key()?;
        let names = section_names(&fp_key, &row);
        let assist = latest_per_subject(&runs, |r| r.mode == "assist" && answered(r));
        let shadow = latest_per_subject(&runs, |r| {
            r.mode == "shadow" && answered(r) && r.baseline_answer.is_some()
        });
        let mut unknown: std::collections::BTreeSet<&str> = Default::default();
        for (subject, r) in &assist {
            let Some(name) = names.get(*subject) else {
                unknown.insert(subject);
                continue;
            };
            if was_rejected(&runs, r) {
                tp.rejected += 1;
                continue;
            }
            let answer = r.answer.clone().unwrap_or_default();
            tp.proposals.push(SectionProposal {
                section: name.clone(),
                applies_as: applied_category(&answer).map(str::to_string),
                answer,
                confidence: r.confidence,
                run_id: r.id,
                at: r.at,
                person: row.settings.section_map.get(name).cloned(),
                followup: r.followup.clone(),
                top: top_two(r),
            });
        }
        for (subject, r) in &shadow {
            let Some(name) = names.get(*subject) else {
                unknown.insert(subject);
                continue;
            };
            tp.shadow.push(ShadowLine {
                section: name.clone(),
                rule: r.baseline_answer.clone().unwrap_or_default(),
                model: r.answer.clone().unwrap_or_default(),
                confidence: r.confidence,
                run_id: r.id,
            });
        }
        tp.unknown_sections = unknown.len();
        tp.proposals.sort_by(|a, b| a.section.cmp(&b.section));
        tp.shadow.sort_by(|a, b| a.section.cmp(&b.section));
        let sets: Vec<(String, String)> = tp
            .proposals
            .iter()
            .filter(|p| p.pending())
            .filter_map(|p| Some((p.section.clone(), p.applies_as.clone()?)))
            .collect();
        if !sets.is_empty() {
            tp.apply = Some(ApplyCommand {
                cli: apply_cli(row.id, &sets),
                work_admin: apply_args(&row, &merged_section_map(&row, &sets)),
            });
        }
        out.push(tp);
    }
    Ok(out)
}

// --- a person decides one proposal (assist) -------------------------------------

/// What a person does with one proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposalAction {
    /// Its category into their section map (`not_planned` → `done`).
    Apply,
    /// Another category into their map: a correction.
    ApplyAs(String),
    /// "Not this": the section stays unmapped.
    Reject,
}

impl ProposalAction {
    /// `apply` | `apply_as` (with `category`) | `reject`.
    pub fn parse(action: &str, category: Option<&str>) -> Result<Self, IpcError> {
        let category = category.map(str::trim).filter(|c| !c.is_empty());
        match (action, category) {
            ("apply", None) => Ok(Self::Apply),
            ("reject", None) => Ok(Self::Reject),
            ("apply_as", Some(c)) => Ok(Self::ApplyAs(section_category(c)?.to_string())),
            ("apply_as", None) => Err(invalid("apply_as needs a category")),
            ("apply" | "reject", Some(_)) => Err(invalid(format!(
                "{action} takes no category (apply_as does)"
            ))),
            _ => Err(invalid(format!(
                "action is apply, apply_as or reject, not {action:?}"
            ))),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Apply => "apply",
            Self::ApplyAs(_) => "apply_as",
            Self::Reject => "reject",
        }
    }
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(crate::ipc_error::codes::E_INVALID, msg)
}

fn conflict(msg: impl Into<String>) -> IpcError {
    IpcError::new(crate::ipc_error::codes::E_INVALID_STATE, msg)
}

/// PURE: `c` if a section map takes it (todo | in_progress | done).
pub fn section_category(c: &str) -> Result<&'static str, IpcError> {
    SECTION_CATEGORIES
        .iter()
        .find(|x| **x == c)
        .copied()
        .ok_or_else(|| {
            invalid(format!(
                "a section map takes todo, in_progress or done, not {c:?}"
            ))
        })
}

/// A proposal a person may decide, read back from its run: the section's
/// name comes from the tracker's stored config, never from a caller.
#[derive(Debug, Clone)]
pub struct PendingProposal {
    pub run: DecisionRunRow,
    pub tracker: TrackerRow,
    pub section: String,
}

impl PendingProposal {
    /// The model's option.
    pub fn answer(&self) -> &str {
        self.run.answer.as_deref().unwrap_or_default()
    }

    /// The category `action` puts into the map; `None` for a rejection.
    /// Applying an `unsure` answer is refused: it proposes nothing.
    pub fn category(&self, action: &ProposalAction) -> Result<Option<String>, IpcError> {
        match action {
            ProposalAction::Reject => Ok(None),
            ProposalAction::ApplyAs(c) => Ok(Some(section_category(c)?.to_string())),
            ProposalAction::Apply => applied_category(self.answer())
                .map(|c| Some(c.to_string()))
                .ok_or_else(|| {
                    invalid(format!(
                        "run {} is {}: it proposes nothing; apply_as a category, or reject it",
                        self.run.id,
                        self.answer()
                    ))
                }),
        }
    }

    /// The follow-up applying `category` records: `confirmed` when it is
    /// what the answer applies as, else `corrected` to it.
    pub fn followup_for(&self, category: &str) -> (&'static str, Option<String>) {
        if applied_category(self.answer()) == Some(category) {
            ("confirmed", None)
        } else {
            ("corrected", Some(category.to_string()))
        }
    }
}

/// Read the proposal of run `run_id` for a person to decide: a
/// `status_map` run about a tracker section, answered in assist and usable
/// (no fallback), the latest such answer for its section, not decided yet,
/// about a section the tracker's stored config (or map) still names and
/// that is not in the person's map already. Anything else is refused.
pub fn pending_proposal(s: &Store, run_id: i64) -> Result<PendingProposal, IpcError> {
    let run = s.get_decision_run(run_id)?.ok_or_else(|| {
        IpcError::new(
            crate::ipc_error::codes::E_NOTFOUND,
            format!("no decision run {run_id}"),
        )
    })?;
    if run.feature != Feature::StatusMap.as_str() || run.subject_kind != SUBJECT_KIND {
        return Err(invalid(format!(
            "run {run_id} is not a status_map proposal"
        )));
    }
    if run.mode != Mode::Assist.as_str() || !answered(&run) {
        return Err(invalid(format!(
            "run {run_id} is not a usable assist answer (mode {}, {})",
            run.mode,
            run.fallback.as_deref().unwrap_or("no answer")
        )));
    }
    let tracker_id = run
        .subject_id
        .split_once(':')
        .and_then(|(t, _)| t.parse::<i64>().ok())
        .ok_or_else(|| invalid(format!("run {run_id} names no tracker")))?;
    let tracker = s.require_tracker(tracker_id)?;
    if tracker.provider != PROVIDER {
        return Err(invalid(format!(
            "tracker {tracker_id} is {}; a section map is an Asana setting",
            tracker.provider
        )));
    }
    let fp_key = s.decision_fp_key()?;
    let section = section_names(&fp_key, &tracker)
        .remove(&run.subject_id)
        .ok_or_else(|| {
            conflict(format!(
                "run {run_id} is about a section tracker {tracker_id} no longer lists \
                 (test the tracker to refresh its sections)"
            ))
        })?;
    if let Some(c) = tracker.settings.section_map.get(&section) {
        return Err(conflict(format!(
            "{section:?} is already in your section map as {c}"
        )));
    }
    let runs = s.decision_runs_for_subjects(
        Feature::StatusMap.as_str(),
        SUBJECT_KIND,
        &run.subject_id,
        None,
    )?;
    if let Some(newer) = runs
        .iter()
        .find(|r| r.subject_id == run.subject_id && r.mode == Mode::Assist.as_str() && answered(r))
    {
        if newer.id != run.id {
            return Err(conflict(format!(
                "run {run_id} is not the latest proposal for this section (run {} is)",
                newer.id
            )));
        }
    }
    // After the latest-check: a proposal a newer one superseded (`ignored`)
    // is refused as not the latest, which names the one to decide.
    if let Some(f) = &run.followup {
        return Err(conflict(format!("run {run_id} is already {f}")));
    }
    if was_rejected(&runs, &run) {
        return Err(conflict(format!(
            "the same answer was rejected before (run {run_id} is hidden)"
        )));
    }
    Ok(PendingProposal {
        run,
        tracker,
        section,
    })
}

/// PURE: the `work_admin update` arguments that put `section` → `category`
/// into `row`'s section map and confirm it — the inferred map kept under
/// the person's entries, as Settings → Work's Confirm and `fleet-hub
/// tracker section-map` do.
pub fn apply_one_args(row: &TrackerRow, section: &str, category: &str) -> Value {
    apply_args(
        row,
        &merged_section_map(row, &[(section.to_string(), category.to_string())]),
    )
}

/// What deciding one proposal did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposalOutcome {
    pub run_id: i64,
    pub tracker_id: i64,
    /// The section's name, from the tracker's stored config.
    pub section: String,
    /// apply | apply_as | reject.
    pub action: String,
    /// What the person's map now holds for the section (`None`: rejected,
    /// left unmapped).
    pub category: Option<String>,
    /// The run's follow-up: confirmed | corrected | rejected.
    pub followup: Option<String>,
    pub corrected_to: Option<String>,
}

/// After an apply went through `work_admin update` (whose
/// [`record_followups`] marks the latest answered run of the section):
/// make sure run `p` carries its follow-up — `record_followups` leaves an
/// `unsure` answer alone.
/// Returns the outcome as the record now reads.
pub fn record_applied(
    s: &Store,
    p: &PendingProposal,
    action: &ProposalAction,
    category: &str,
    now: i64,
) -> Result<ProposalOutcome, IpcError> {
    let current = s.get_decision_run(p.run.id)?;
    if current.as_ref().is_some_and(|r| r.followup.is_none()) {
        let (f, to) = p.followup_for(category);
        s.set_decision_followup(p.run.id, f, to.as_deref(), now)?;
    }
    let run = s.get_decision_run(p.run.id)?;
    Ok(ProposalOutcome {
        run_id: p.run.id,
        tracker_id: p.tracker.id,
        section: p.section.clone(),
        action: action.as_str().into(),
        category: Some(category.to_string()),
        followup: run.as_ref().and_then(|r| r.followup.clone()),
        corrected_to: run.and_then(|r| r.corrected_to),
    })
}

/// "Not this": mark run `run_id` `rejected`. Writes nothing but the run's
/// follow-up: the section stays unmapped, and the proposals view hides the
/// answer until a new one exists (another input, question version or
/// model).
pub fn reject_proposal(s: &Store, run_id: i64, now: i64) -> Result<ProposalOutcome, IpcError> {
    let p = pending_proposal(s, run_id)?;
    s.set_decision_followup(p.run.id, FOLLOWUP_REJECTED, None, now)?;
    Ok(ProposalOutcome {
        run_id: p.run.id,
        tracker_id: p.tracker.id,
        section: p.section,
        action: ProposalAction::Reject.as_str().into(),
        category: None,
        followup: Some(FOLLOWUP_REJECTED.into()),
        corrected_to: None,
    })
}

/// A person decides the proposal of run `run_id` (the standalone desktop;
/// the hub's operator has `fleet-hub decide proposals apply|reject`). An
/// apply is a `work_admin update` of the tracker's settings — the same
/// path, validation, event and follow-up as any section map change; a
/// rejection writes the run's follow-up only.
pub fn decide_proposal(
    store: &Mutex<Store>,
    run_id: i64,
    action: &ProposalAction,
    now: i64,
) -> Result<ProposalOutcome, IpcError> {
    let s = lock(store)?;
    if *action == ProposalAction::Reject {
        return reject_proposal(&s, run_id, now);
    }
    // One lock from the read to the write: the settings the apply merges
    // into are the tracker's as it is now, so a settings change made
    // meanwhile (another section, a sprint field) is never overwritten by
    // a stale copy, and the proposal cannot be decided twice.
    let p = pending_proposal(&s, run_id)?;
    let category = p
        .category(action)?
        .ok_or_else(|| invalid("an apply needs a category"))?;
    let args: crate::service::trackers::admin::WorkAdminArgs =
        serde_json::from_value(apply_one_args(&p.tracker, &p.section, &category))
            .map_err(|e| IpcError::new(crate::ipc_error::codes::E_SERIALIZE, e.to_string()))?;
    crate::service::trackers::admin::update_locked(&args, &s)?;
    record_applied(&s, &p, action, &category, now)
}

/// The sync tick's hook: after a pass, the Asana trackers that synced
/// cleanly and are due get a run, in a task of its own — never on the
/// sync's path, never failing it, one run at a time.
pub struct StatusMapTrigger {
    ctx: DecideCtx,
    /// Tracker → (when its last run started, its sections' digest then).
    last: Mutex<HashMap<i64, (i64, u64)>>,
    running: Arc<AtomicBool>,
}

/// PURE: a digest of what a run reads from a tracker's row.
fn sections_digest(row: &TrackerRow) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    row.org_id.hash(&mut h);
    row.config.unmapped_sections.hash(&mut h);
    row.config.project_sections.hash(&mut h);
    row.config.section_map.hash(&mut h);
    row.settings.section_map.hash(&mut h);
    h.finish()
}

/// PURE: due when never run, a day has passed, or its sections changed.
fn due(last: Option<(i64, u64)>, now: i64, digest: u64) -> bool {
    match last {
        None => true,
        Some((at, d)) => d != digest || now - at >= RUN_EVERY_SECS,
    }
}

impl StatusMapTrigger {
    pub fn new(ctx: DecideCtx) -> Arc<Self> {
        Arc::new(StatusMapTrigger {
            ctx,
            last: Mutex::new(HashMap::new()),
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    /// After a sync pass. Cheap when the feature is off (one short lock);
    /// otherwise spawns one task over the due trackers and returns its
    /// handle. `None`: nothing to do, or a run is still going.
    pub fn after_pass(
        self: &Arc<Self>,
        passes: &[TrackerPass],
    ) -> Option<tokio::task::JoinHandle<()>> {
        let now = self.ctx.now();
        let due_now: Vec<(i64, u64)> = {
            let s = lock(&self.ctx.store).ok()?;
            if !settings::get_bool(&s, settings::DECIDE_JEV_ENABLED)
                || FeatureMode::of(&s, Feature::StatusMap) == FeatureMode::Off
            {
                return None;
            }
            let last = self.last.lock().ok()?;
            let mut ids = Vec::new();
            for p in passes.iter().filter(|p| !p.skipped && p.error.is_none()) {
                let Ok(Some(row)) = s.get_tracker(p.tracker_id) else {
                    continue;
                };
                if row.provider != PROVIDER {
                    continue;
                }
                let digest = sections_digest(&row);
                if due(last.get(&row.id).copied(), now, digest) {
                    ids.push((row.id, digest));
                }
            }
            ids
        };
        if due_now.is_empty() || self.running.swap(true, Ordering::AcqRel) {
            // A run is still going: the trackers due now stay due, and the
            // next pass after it ends picks them up.
            return None;
        }
        // Only now is a run theirs: record it.
        if let Ok(mut last) = self.last.lock() {
            for (id, digest) in &due_now {
                last.insert(*id, (now, *digest));
            }
        }
        let due_ids: Vec<i64> = due_now.into_iter().map(|(id, _)| id).collect();
        let ctx = self.ctx.clone();
        let running = Arc::clone(&self.running);
        /// Clears the single-flight flag however the task ends.
        struct Reset(Arc<AtomicBool>);
        impl Drop for Reset {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        Some(crate::rt::spawn(async move {
            let _reset = Reset(running);
            for id in due_ids {
                match propose_for_tracker(&ctx, id).await {
                    Ok(r) => tracing::debug!(
                        tracker_id = id,
                        asked = r.asked,
                        usable = r.usable,
                        skipped = r.skipped_recent,
                        deferred = r.deferred,
                        gated = ?r.gated,
                        stopped = ?r.stopped,
                        "[decide] status_map run"
                    ),
                    Err(e) => tracing::debug!(
                        tracker_id = id,
                        "[decide] status_map run not made: {}",
                        e.message
                    ),
                }
            }
        }))
    }
}

#[cfg(test)]
#[path = "status_map_tests.rs"]
mod tests;
