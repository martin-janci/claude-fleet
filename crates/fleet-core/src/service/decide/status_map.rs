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
//!   section, its latest answered run is marked `confirmed` (same category)
//!   or `corrected` (to theirs) — [`record_followups`], called where the
//!   settings are updated.
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
        let recent = s.decision_runs_for_subjects(
            Feature::StatusMap.as_str(),
            SUBJECT_KIND,
            &subject_prefix(tracker_id),
            Some(now - REASK_DAYS * 86_400),
        )?;
        let mut asks = Vec::new();
        for (name, baseline) in sections {
            let request = question_for(&name, &board_of(&row, &name));
            let subject = subject_id(tracker_id, &section_id(&fp_key, tracker_id, &name));
            let fp = fingerprint(&fp_key, &request.redacted());
            let seen = recent.iter().any(|r| {
                r.subject_id == subject
                    && r.input_fp.as_deref() == Some(fp.as_str())
                    && r.question_version == QUESTION_VERSION
                    && r.mode == mode.as_str()
                    && decided(r)
                    && (model == "jev-latest" || r.model_version.as_deref() == Some(model.as_str()))
            });
            if seen {
                report.skipped_recent += 1;
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
        let out = decide(
            ctx,
            DecideRequest {
                feature: Feature::StatusMap,
                subject_kind: SUBJECT_KIND.into(),
                subject_id: ask.subject,
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
/// answered run of every section their map now holds `confirmed` (the
/// answer applies as their category) or `corrected` (to theirs), once.
/// Returns the runs marked. Never writes a tracker.
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
        answered(r) && r.answer.as_deref().and_then(applied_category).is_some()
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
        if let Some(a) = &self.apply {
            out.push(format!("  apply (a person decides): {}", a.cli));
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
            apply: None,
        };
        if runs.is_empty() {
            out.push(tp);
            continue;
        }
        // Runs exist, so the key does (the envelope made it): a read-only
        // store reads it without writing.
        let fp_key = s.decision_fp_key()?;
        let mut names: HashMap<String, String> = HashMap::new();
        for n in row
            .config
            .unmapped_sections
            .iter()
            .chain(row.config.section_map.keys())
            .chain(row.settings.section_map.keys())
        {
            names.insert(
                subject_id(row.id, &section_id(&fp_key, row.id, n)),
                n.clone(),
            );
        }
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
            .filter(|p| p.person.is_none())
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
        let due_ids: Vec<i64> = {
            let s = lock(&self.ctx.store).ok()?;
            if !settings::get_bool(&s, settings::DECIDE_JEV_ENABLED)
                || FeatureMode::of(&s, Feature::StatusMap) == FeatureMode::Off
            {
                return None;
            }
            let mut last = self.last.lock().ok()?;
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
                    ids.push(row.id);
                    last.insert(row.id, (now, digest));
                }
            }
            ids
        };
        if due_ids.is_empty() || self.running.swap(true, Ordering::AcqRel) {
            return None;
        }
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
