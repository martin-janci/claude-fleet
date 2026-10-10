//! The decision record and the decision model's credential (Jev evaluation,
//! decisions D35 / D37; migration 069). The envelope that writes these rows
//! is `service::decide`; the guide is `docs/decisions.md`.
//!
//! The rules that matter here:
//!
//! * **Never raw text in `decision_runs`.** Every text column is an id or a
//!   vocabulary word ([`is_decision_word`]), a number, or JSON of those; the
//!   input is kept only as an HMAC fingerprint keyed by a local secret.
//!   [`Store::insert_decision_run`] refuses anything else, so an adapter
//!   cannot slip a prompt or a ticket title in by mistake.
//! * **The API key has one reader.** [`Store::resolve_decision_credential`]
//!   is the one function that selects `decision_secrets`' key row, and it
//!   hands back a [`Secret`], which neither serialises nor prints. A key
//!   may be a reference (`env:NAME` / `file:/path`) the process reads at
//!   use, like a tracker credential.

use super::trackers::read_credential_ref;
use super::{now_unix, Secret, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// `decision_runs.fallback`: why a decision was not (or not only) the
/// model's. Closed; the migration's CHECK holds the same list.
pub const DECISION_FALLBACKS: &[&str] = &[
    "not_owner",
    "flag_off",
    "org_off",
    "mode_off",
    "no_key",
    "breaker_open",
    "budget",
    "timeout",
    "http_error",
    "rate_limited",
    "invalid_answer",
    "low_confidence",
];

/// The fallbacks that are a failed CALL: the circuit breaker counts these
/// (and only among rows that sent a request).
pub const DECISION_CALL_FAILURES: &[&str] = &["timeout", "http_error", "rate_limited"];

/// `decision_runs.followup`: what became of a decision, filled in later by
/// its adapter.
pub const DECISION_FOLLOWUPS: &[&str] = &["confirmed", "rejected", "corrected", "ignored"];

/// The follow-ups a PERSON gave (`ignored` is fleet's: a newer answer took
/// the proposal's place). A run carrying one is a label — and a rejection
/// must keep holding (D37) — so the retention sweep keeps it.
pub const DECISION_PERSON_FOLLOWUPS: &[&str] = &["confirmed", "rejected", "corrected"];

/// `decision_runs.subject_kind` of an offline benchmark's call (`fleet-hub
/// decide bench`). Such a run is not the live adapters': the live circuit
/// breaker and daily budget never count it ([`RunScope::Live`]), and
/// [`Store::decision_stats`] reports it apart.
pub const DECISION_BENCH_SUBJECT: &str = "bench";

/// `decision_runs.baseline_answer` when the baseline abstained (J3's keyword
/// rule on a section it cannot classify). Such a run has nothing to agree
/// with, so [`Store::decision_stats`] leaves it out of `compared` / `agreed`
/// — the same count the status_map proposals view prints.
pub const DECISION_NO_BASELINE: &str = "none";

/// The proposals about one subject (step 2.8), as one JSON array: per
/// feature, the LATEST run about `$subject` of kind `$kind`, kept only when
/// it is a live `assist` answer with no fallback and no follow-up (save a
/// `related_session` a person confirmed with Link, which stays as the
/// row's `linked` partner, M15 G4.3), not
/// `unsure`, and at or above `PROPOSAL_MIN_CONFIDENCE` (0.5, spelled out
/// in [`proposal_keep_sql!`] because `concat!` takes literals;
/// `tests::the_proposal_floor_is_the_constant` keeps them equal). A later shadow run or fallback about the same subject
/// therefore withdraws an earlier proposal, as a confirm or a change does.
/// `$subject` is an SQL expression over the outer row (`CAST(sessions.id AS
/// TEXT)`); the `decision_runs_subject` index (migration 129) serves both
/// lookups. Decoded by `rows::decode_proposals`.
#[macro_export]
macro_rules! proposals_sql {
    ($kind:literal, $subject:literal) => {
        concat!(
            "(SELECT json_group_array(json_object(\
                 'feature', d.feature, 'value', d.answer, \
                 'source', CASE d.provider WHEN 'rules' THEN 'rule' \
                                           WHEN 'llm' THEN 'llm' ELSE 'jev' END, \
                 'confidence_pct', CAST(ROUND(d.confidence * 100) AS INTEGER), \
                 'run_id', d.id, 'at', d.at, \
                 'linked', json(CASE WHEN d.followup = 'confirmed' THEN 'true' END))) \
               FROM decision_runs d \
              WHERE d.subject_kind = '",
            $kind,
            "' AND d.subject_id = ",
            $subject,
            " AND d.id = (SELECT MAX(d2.id) FROM decision_runs d2 \
                            WHERE d2.subject_kind = d.subject_kind \
                              AND d2.subject_id = d.subject_id \
                              AND d2.feature = d.feature) \
                AND ",
            $crate::proposal_keep_sql!(),
            ")"
        )
    };
}

/// Which of a subject's latest runs [`proposals_sql!`] and
/// [`Store::current_proposals`] keep, over a run aliased `d`.
#[macro_export]
macro_rules! proposal_keep_sql {
    () => {
        "d.mode = 'assist' AND d.fallback IS NULL \
         AND (d.followup IS NULL \
              OR (d.feature = 'related_session' AND d.followup = 'confirmed')) \
         AND d.answer IS NOT NULL \
         AND d.answer <> 'unsure' \
         AND (d.confidence IS NULL OR d.confidence >= 0.5)"
    };
}

/// Which runs the circuit breaker and the daily budget count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunScope {
    /// The live adapters' runs: a benchmark's never open the live breaker
    /// or spend the live budget.
    Live,
    /// Every run, a benchmark's included — what a benchmark call is gated
    /// on (so a failing API or a spent day stops it too).
    All,
}

impl RunScope {
    /// `?n = 1` in SQL: count benchmark runs too.
    fn with_bench(self) -> i64 {
        i64::from(self == RunScope::All)
    }
}

/// `decision_runs.mode`.
pub const DECISION_MODES: &[&str] = &["off", "shadow", "assist"];

/// Most candidates one run records (Jev's own limit on choice options).
pub const DECISION_MAX_CANDIDATES: usize = 255;

/// Most runs [`Store::decision_runs_for_subjects`] returns.
pub const DECISION_SUBJECT_RUNS_MAX: usize = 5_000;

/// Longest id or vocabulary word a run records.
pub const DECISION_WORD_MAX_CHARS: usize = 80;

/// `decision_secrets.name` of the API key.
const API_KEY_NAME: &str = "jev_api_key";

/// PURE: an id or a vocabulary word — the only text `decision_runs` holds:
/// 1..=80 of `A-Z a-z 0-9 _ - . : / #`. No space, so no sentence fits.
pub fn is_decision_word(s: &str) -> bool {
    !s.is_empty()
        && s.chars().count() <= DECISION_WORD_MAX_CHARS
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '/' | '#'))
}

/// One run to record. Every text field must pass [`is_decision_word`]
/// (`input_fp`: 64 lower-case hex digits).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NewDecisionRun {
    pub at: i64,
    pub feature: String,
    pub org_id: Option<i64>,
    pub subject_kind: String,
    pub subject_id: String,
    /// `off` | `shadow` | `assist`: the feature's configured mode.
    pub mode: String,
    /// `jev`, `rules`, …
    pub provider: String,
    /// The provider's `model` answer (e.g. `jev-1.13.0`).
    pub model_version: Option<String>,
    /// The adapter's question version (a constant per adapter).
    pub question_version: String,
    pub input_fp: Option<String>,
    /// The ids / vocabulary words offered.
    pub candidates: Vec<String>,
    /// A choice's option, or a noul / score value written as a number.
    pub answer: Option<String>,
    pub probabilities: Option<BTreeMap<String, f64>>,
    pub confidence: Option<f64>,
    pub fallback: Option<String>,
    /// What the current rule decided, for the shadow comparison.
    pub baseline_answer: Option<String>,
    /// A request was sent to the provider.
    pub called: bool,
    pub latency_ms: Option<i64>,
    pub input_tokens: i64,
    pub cost_microusd: i64,
}

/// One recorded run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRunRow {
    pub id: i64,
    pub at: i64,
    pub feature: String,
    pub org_id: Option<i64>,
    pub subject_kind: String,
    pub subject_id: String,
    pub mode: String,
    pub provider: String,
    pub model_version: Option<String>,
    pub question_version: String,
    pub input_fp: Option<String>,
    pub candidates: Vec<String>,
    pub answer: Option<String>,
    pub probabilities: Option<BTreeMap<String, f64>>,
    pub confidence: Option<f64>,
    pub fallback: Option<String>,
    pub baseline_answer: Option<String>,
    pub called: bool,
    pub latency_ms: Option<i64>,
    pub input_tokens: i64,
    pub cost_microusd: i64,
    pub followup: Option<String>,
    pub followup_at: Option<i64>,
    pub corrected_to: Option<String>,
}

/// Which runs [`Store::list_decision_runs`] returns, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecisionRunFilter {
    pub feature: Option<String>,
    pub provider: Option<String>,
    pub org_id: Option<i64>,
    /// Only runs at or after this unix time.
    pub since: Option<i64>,
    /// At most this many (clamped to 1..=1000; `0` = 100).
    pub limit: u32,
}

/// One group of [`Store::decision_stats`]: runs per feature, provider,
/// fallback and org.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionStatRow {
    pub feature: String,
    /// Offline benchmark runs ([`DECISION_BENCH_SUBJECT`]), grouped apart
    /// from the live ones.
    #[serde(default)]
    pub bench: bool,
    pub provider: String,
    pub fallback: Option<String>,
    pub org_id: Option<i64>,
    pub runs: i64,
    /// Runs that sent a request.
    pub called: i64,
    pub input_tokens: i64,
    pub cost_microusd: i64,
    pub confirmed: i64,
    pub rejected: i64,
    pub corrected: i64,
    pub ignored: i64,
    /// Runs with both an answer and a baseline that did not abstain
    /// ([`DECISION_NO_BASELINE`]), and of those, the ones where they agree
    /// (the shadow comparison).
    pub compared: i64,
    pub agreed: i64,
}

/// Whether an API key is configured — never the key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionKeyStatus {
    pub configured: bool,
    /// Stored as an `env:` / `file:` reference.
    pub by_reference: bool,
    /// When it was set (or last replaced).
    pub set_at: Option<i64>,
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_INVALID, msg)
}

fn check_word(field: &str, v: &str) -> Result<(), IpcError> {
    if is_decision_word(v) {
        Ok(())
    } else {
        Err(invalid(format!(
            "decision_runs.{field} holds only an id or a vocabulary word \
             (1-{DECISION_WORD_MAX_CHARS} of A-Za-z0-9_-.:/#), never text"
        )))
    }
}

fn check_opt_word(field: &str, v: Option<&str>) -> Result<(), IpcError> {
    v.map_or(Ok(()), |v| check_word(field, v))
}

fn check_unit(field: &str, v: Option<f64>) -> Result<(), IpcError> {
    match v {
        Some(x) if !x.is_finite() => Err(invalid(format!("decision_runs.{field} is not finite"))),
        _ => Ok(()),
    }
}

impl NewDecisionRun {
    /// Refuse anything but ids, vocabulary words and numbers.
    pub fn validate(&self) -> Result<(), IpcError> {
        check_word("feature", &self.feature)?;
        check_word("subject_kind", &self.subject_kind)?;
        check_word("subject_id", &self.subject_id)?;
        check_word("provider", &self.provider)?;
        check_word("question_version", &self.question_version)?;
        check_opt_word("model_version", self.model_version.as_deref())?;
        check_opt_word("answer", self.answer.as_deref())?;
        check_opt_word("baseline_answer", self.baseline_answer.as_deref())?;
        if !DECISION_MODES.contains(&self.mode.as_str()) {
            return Err(invalid(format!("unknown decision mode {:?}", self.mode)));
        }
        if let Some(f) = &self.fallback {
            if !DECISION_FALLBACKS.contains(&f.as_str()) {
                return Err(invalid(format!("unknown decision fallback {f:?}")));
            }
        }
        if let Some(fp) = &self.input_fp {
            if fp.len() != 64 || !fp.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
                return Err(invalid(
                    "decision_runs.input_fp is 64 lower-case hex digits",
                ));
            }
        }
        if self.candidates.len() > DECISION_MAX_CANDIDATES {
            return Err(invalid(format!(
                "a decision records at most {DECISION_MAX_CANDIDATES} candidates"
            )));
        }
        for c in &self.candidates {
            check_word("candidates", c)?;
        }
        if let Some(p) = &self.probabilities {
            if p.len() > DECISION_MAX_CANDIDATES {
                return Err(invalid("too many probabilities"));
            }
            for (k, v) in p {
                check_word("probabilities", k)?;
                check_unit("probabilities", Some(*v))?;
            }
        }
        check_unit("confidence", self.confidence)?;
        if self.input_tokens < 0 || self.cost_microusd < 0 {
            return Err(invalid("decision tokens and cost are never negative"));
        }
        Ok(())
    }
}

const RUN_COLUMNS: &str = "id, at, feature, org_id, subject_kind, subject_id, mode, provider, \
     model_version, question_version, input_fp, candidates, answer, probabilities, confidence, \
     fallback, baseline_answer, called, latency_ms, input_tokens, cost_microusd, followup, \
     followup_at, corrected_to";

fn map_run(r: &rusqlite::Row<'_>) -> rusqlite::Result<DecisionRunRow> {
    let candidates: String = r.get(11)?;
    let probabilities: Option<String> = r.get(13)?;
    Ok(DecisionRunRow {
        id: r.get(0)?,
        at: r.get(1)?,
        feature: r.get(2)?,
        org_id: r.get(3)?,
        subject_kind: r.get(4)?,
        subject_id: r.get(5)?,
        mode: r.get(6)?,
        provider: r.get(7)?,
        model_version: r.get(8)?,
        question_version: r.get(9)?,
        input_fp: r.get(10)?,
        candidates: serde_json::from_str(&candidates).unwrap_or_default(),
        answer: r.get(12)?,
        probabilities: probabilities.and_then(|p| serde_json::from_str(&p).ok()),
        confidence: r.get(14)?,
        fallback: r.get(15)?,
        baseline_answer: r.get(16)?,
        called: r.get::<_, i64>(17)? != 0,
        latency_ms: r.get(18)?,
        input_tokens: r.get(19)?,
        cost_microusd: r.get(20)?,
        followup: r.get(21)?,
        followup_at: r.get(22)?,
        corrected_to: r.get(23)?,
    })
}

impl Store {
    // --- decision_runs --------------------------------------------------------

    /// Record one run (validated: ids, words and numbers only). Returns its id.
    pub fn insert_decision_run(&self, run: &NewDecisionRun) -> Result<i64, IpcError> {
        run.validate()?;
        let candidates = serde_json::to_string(&run.candidates)
            .map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))?;
        let probabilities = run
            .probabilities
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))?;
        self.conn.execute(
            "INSERT INTO decision_runs (at, feature, org_id, subject_kind, subject_id, mode, \
             provider, model_version, question_version, input_fp, candidates, answer, \
             probabilities, confidence, fallback, baseline_answer, called, latency_ms, \
             input_tokens, cost_microusd) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
             ?17, ?18, ?19, ?20)",
            rusqlite::params![
                run.at,
                run.feature,
                run.org_id,
                run.subject_kind,
                run.subject_id,
                run.mode,
                run.provider,
                run.model_version,
                run.question_version,
                run.input_fp,
                candidates,
                run.answer,
                probabilities,
                run.confidence,
                run.fallback,
                run.baseline_answer,
                run.called as i64,
                run.latency_ms,
                run.input_tokens,
                run.cost_microusd,
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.emit_proposal_subject(&run.subject_kind, &run.subject_id)?;
        Ok(id)
    }

    /// Every proposal about subjects of `subject_kind`, by subject id: the
    /// same runs [`proposals_sql!`] reads for one row, for a reader that
    /// builds many rows at once (the Work view's tasks). Sorted by feature.
    pub fn current_proposals(
        &self,
        subject_kind: &str,
    ) -> Result<HashMap<String, Vec<super::DecisionProposal>>, IpcError> {
        let mut stmt = self.conn.prepare(concat!(
            "SELECT d.subject_id, d.feature, d.answer, d.provider, d.confidence, d.id, d.at, \
                    d.followup \
               FROM decision_runs d \
              WHERE d.subject_kind = ?1 \
                AND d.id IN (SELECT MAX(id) FROM decision_runs \
                              WHERE subject_kind = ?1 GROUP BY subject_id, feature) \
                AND ",
            crate::proposal_keep_sql!(),
            " ORDER BY d.subject_id, d.feature"
        ))?;
        let rows = stmt.query_map([subject_kind], |r| {
            let provider: String = r.get(3)?;
            let confidence: Option<f64> = r.get(4)?;
            Ok((
                r.get::<_, String>(0)?,
                super::DecisionProposal {
                    feature: r.get(1)?,
                    value: r.get(2)?,
                    source: super::proposal_source(&provider).to_string(),
                    reason: None,
                    confidence_pct: confidence.map(|c| (c.clamp(0.0, 1.0) * 100.0).round() as u8),
                    run_id: Some(r.get(5)?),
                    at: Some(r.get(6)?),
                    linked: (r.get::<_, Option<String>>(7)?.as_deref() == Some("confirmed"))
                        .then_some(true),
                },
            ))
        })?;
        let mut out: HashMap<String, Vec<super::DecisionProposal>> = HashMap::new();
        for row in rows {
            let (subject, p) = row?;
            out.entry(subject).or_default().push(p);
        }
        Ok(out)
    }

    /// A run about a session changes what its row proposes
    /// ([`SessionRow::proposals`](super::SessionRow::proposals) is read from
    /// `decision_runs`, not stored on the row), so the row is re-emitted for
    /// clients to merge. Nothing for any other subject.
    fn emit_proposal_subject(&self, subject_kind: &str, subject_id: &str) -> Result<(), IpcError> {
        if subject_kind == super::PROPOSAL_SUBJECT_SESSION {
            if let Ok(id) = subject_id.parse::<i64>() {
                self.emit_session(id)?;
            }
        }
        Ok(())
    }

    pub fn get_decision_run(&self, id: i64) -> Result<Option<DecisionRunRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {RUN_COLUMNS} FROM decision_runs WHERE id = ?1"),
                [id],
                map_run,
            )
            .optional()?)
    }

    /// What became of run `id` (filled in by its adapter): `followup` is one
    /// of [`DECISION_FOLLOWUPS`]; `corrected_to` (an id or word) only with
    /// `corrected`. `false` when there is no such run (swept, say).
    pub fn set_decision_followup(
        &self,
        id: i64,
        followup: &str,
        corrected_to: Option<&str>,
        at: i64,
    ) -> Result<bool, IpcError> {
        if !DECISION_FOLLOWUPS.contains(&followup) {
            return Err(invalid(format!(
                "followup is one of {}, not {followup:?}",
                DECISION_FOLLOWUPS.join(", ")
            )));
        }
        if corrected_to.is_some() && followup != "corrected" {
            return Err(invalid("corrected_to goes only with followup corrected"));
        }
        check_opt_word("corrected_to", corrected_to)?;
        let subject: Option<(String, String)> = self
            .conn
            .query_row(
                "UPDATE decision_runs SET followup = ?2, followup_at = ?3, corrected_to = ?4 \
                 WHERE id = ?1 RETURNING subject_kind, subject_id",
                rusqlite::params![id, followup, at, corrected_to],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((kind, subject_id)) = subject else {
            return Ok(false);
        };
        // A decided proposal leaves the row.
        self.emit_proposal_subject(&kind, &subject_id)?;
        Ok(true)
    }

    /// Mark `ignored` every earlier usable answer (no fallback, an answer)
    /// of `feature` in `mode` about the same subject, older than run
    /// `newer_id`, that has no follow-up yet: a newer answer took its place
    /// before anyone decided it. Never overwrites a follow-up (a person's
    /// decision racing this one wins). Returns the runs marked; when it
    /// marked any, the subject's row is re-emitted, since a withdrawn
    /// proposal must leave the screen (`quick_answer::withdraw`).
    pub fn supersede_decision_runs(
        &self,
        feature: &str,
        subject_kind: &str,
        subject_id: &str,
        mode: &str,
        newer_id: i64,
        at: i64,
    ) -> Result<usize, IpcError> {
        let n = self.conn.execute(
            "UPDATE decision_runs SET followup = 'ignored', followup_at = ?6 \
             WHERE feature = ?1 AND subject_kind = ?2 AND subject_id = ?3 AND mode = ?4 \
               AND id < ?5 AND followup IS NULL AND fallback IS NULL AND answer IS NOT NULL",
            rusqlite::params![feature, subject_kind, subject_id, mode, newer_id, at],
        )?;
        if n > 0 {
            self.emit_proposal_subject(subject_kind, subject_id)?;
        }
        Ok(n)
    }

    /// Recent runs, newest first.
    pub fn list_decision_runs(
        &self,
        f: &DecisionRunFilter,
    ) -> Result<Vec<DecisionRunRow>, IpcError> {
        let limit = match f.limit {
            0 => 100,
            n => n.min(1000),
        };
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RUN_COLUMNS} FROM decision_runs \
             WHERE (?1 IS NULL OR feature = ?1) AND (?2 IS NULL OR provider = ?2) \
               AND (?3 IS NULL OR org_id = ?3) AND (?4 IS NULL OR at >= ?4) \
             ORDER BY id DESC LIMIT ?5"
        ))?;
        let rows = stmt.query_map(
            rusqlite::params![f.feature, f.provider, f.org_id, f.since, limit],
            map_run,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// A feature's runs about subjects of `subject_kind` whose id starts with
    /// `subject_prefix` (an adapter's own namespace, e.g. `3:` for tracker
    /// 3's sections), at or after `since` when given, newest first, at most
    /// [`DECISION_SUBJECT_RUNS_MAX`]. What an adapter reads to skip a
    /// question it asked recently, and to list its proposals.
    pub fn decision_runs_for_subjects(
        &self,
        feature: &str,
        subject_kind: &str,
        subject_prefix: &str,
        since: Option<i64>,
    ) -> Result<Vec<DecisionRunRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RUN_COLUMNS} FROM decision_runs \
             WHERE feature = ?1 AND subject_kind = ?2 \
               AND substr(subject_id, 1, length(?3)) = ?3 \
               AND (?4 IS NULL OR at >= ?4) \
             ORDER BY id DESC LIMIT ?5"
        ))?;
        let rows = stmt.query_map(
            rusqlite::params![
                feature,
                subject_kind,
                subject_prefix,
                since,
                DECISION_SUBJECT_RUNS_MAX as i64
            ],
            map_run,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Runs since `since`, grouped per feature, live or benchmark
    /// ([`DECISION_BENCH_SUBJECT`]), provider, fallback and org — live
    /// first, so a benchmark never mixes into the live counts.
    pub fn decision_stats(&self, since: i64) -> Result<Vec<DecisionStatRow>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT feature, subject_kind = ?2 AS bench, provider, fallback, org_id, COUNT(*), \
               COALESCE(SUM(called), 0), COALESCE(SUM(input_tokens), 0), \
               COALESCE(SUM(cost_microusd), 0), \
               COALESCE(SUM(followup = 'confirmed'), 0), COALESCE(SUM(followup = 'rejected'), 0), \
               COALESCE(SUM(followup = 'corrected'), 0), COALESCE(SUM(followup = 'ignored'), 0), \
               COALESCE(SUM(answer IS NOT NULL AND baseline_answer IS NOT NULL \
                 AND baseline_answer <> ?3), 0), \
               COALESCE(SUM(answer IS NOT NULL AND answer = baseline_answer \
                 AND baseline_answer <> ?3), 0) \
             FROM decision_runs WHERE at >= ?1 \
             GROUP BY feature, bench, provider, fallback, org_id \
             ORDER BY feature, bench, provider, fallback IS NOT NULL, fallback, org_id",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![since, DECISION_BENCH_SUBJECT, DECISION_NO_BASELINE],
            |r| {
                Ok(DecisionStatRow {
                    feature: r.get(0)?,
                    bench: r.get::<_, i64>(1)? != 0,
                    provider: r.get(2)?,
                    fallback: r.get(3)?,
                    org_id: r.get(4)?,
                    runs: r.get(5)?,
                    called: r.get(6)?,
                    input_tokens: r.get(7)?,
                    cost_microusd: r.get(8)?,
                    confirmed: r.get(9)?,
                    rejected: r.get(10)?,
                    corrected: r.get(11)?,
                    ignored: r.get(12)?,
                    compared: r.get(13)?,
                    agreed: r.get(14)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// `(input tokens, cost in micro-USD)` of `provider`'s runs in `scope`
    /// since `since` (the daily budget's count).
    pub fn decision_usage_since(
        &self,
        provider: &str,
        since: i64,
        scope: RunScope,
    ) -> Result<(i64, i64), IpcError> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(input_tokens), 0), COALESCE(SUM(cost_microusd), 0) \
             FROM decision_runs WHERE provider = ?1 AND at >= ?2 \
               AND (?3 = 1 OR subject_kind != ?4)",
            rusqlite::params![provider, since, scope.with_bench(), DECISION_BENCH_SUBJECT],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }

    /// The circuit breaker's evidence: how many of `provider`'s latest calls
    /// in `scope` (rows that sent a request, at most `max`) failed in a row,
    /// and when the newest of those failures was. `(0, None)` when the
    /// latest call succeeded or there was none.
    pub fn decision_failure_streak(
        &self,
        provider: &str,
        max: u32,
        scope: RunScope,
    ) -> Result<(u32, Option<i64>), IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT fallback, at FROM decision_runs WHERE provider = ?1 AND called = 1 \
               AND (?3 = 1 OR subject_kind != ?4) \
             ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![provider, max, scope.with_bench(), DECISION_BENCH_SUBJECT],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)),
        )?;
        let mut streak = 0;
        let mut newest = None;
        for row in rows {
            let (fallback, at) = row?;
            let failed = fallback
                .as_deref()
                .is_some_and(|f| DECISION_CALL_FAILURES.contains(&f));
            if !failed {
                break;
            }
            newest.get_or_insert(at);
            streak += 1;
        }
        Ok((streak, newest))
    }

    /// Delete at most `limit` runs older than `cutoff`; the number deleted.
    /// A run a person followed up ([`DECISION_PERSON_FOLLOWUPS`]) is kept
    /// whatever its age: a rejection that disappeared would let the same
    /// answer be proposed again, and a confirmation or correction is the
    /// label record the evaluation is judged on.
    pub fn sweep_decision_runs(&self, cutoff: i64, limit: usize) -> Result<usize, IpcError> {
        let kept = DECISION_PERSON_FOLLOWUPS
            .iter()
            .map(|f| format!("'{f}'"))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(self.conn.execute(
            &format!(
                "DELETE FROM decision_runs WHERE id IN \
                 (SELECT id FROM decision_runs WHERE at < ?1 \
                    AND (followup IS NULL OR followup NOT IN ({kept})) \
                  ORDER BY id LIMIT ?2)"
            ),
            rusqlite::params![cutoff, limit as i64],
        )?)
    }

    /// Rows in `decision_runs`.
    pub fn decision_run_count(&self) -> Result<i64, IpcError> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM decision_runs", [], |r| r.get(0))?)
    }

    // --- decision_secrets -----------------------------------------------------

    /// Set (or replace) the decision model's API key: exactly one of a
    /// `value` or a `credential_ref` (`env:NAME` / `file:/path`).
    pub fn set_decision_credential(
        &self,
        value: Option<&Secret>,
        credential_ref: Option<&str>,
    ) -> Result<(), IpcError> {
        let value = value.map(|v| v.expose().trim()).filter(|v| !v.is_empty());
        let credential_ref = credential_ref.map(str::trim).filter(|v| !v.is_empty());
        match (value, credential_ref) {
            (Some(_), None) | (None, Some(_)) => {}
            _ => return Err(invalid("pass exactly one of a key or a credential_ref")),
        }
        if let Some(r) = credential_ref {
            super::validate_credential_ref(r)?;
        }
        if let Some(v) = value {
            if v.chars().any(char::is_control) || v.chars().any(char::is_whitespace) {
                return Err(invalid("an API key is one word with no spaces"));
            }
        }
        let now = now_unix();
        self.conn.execute(
            "INSERT INTO decision_secrets (name, value, credential_ref, created_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(name) DO UPDATE SET value = excluded.value, \
               credential_ref = excluded.credential_ref, updated_at = ?4",
            rusqlite::params![API_KEY_NAME, value, credential_ref, now],
        )?;
        Ok(())
    }

    /// Forget the API key (the fingerprint key stays). `true` when one was set.
    pub fn clear_decision_credential(&self) -> Result<bool, IpcError> {
        Ok(self.conn.execute(
            "DELETE FROM decision_secrets WHERE name = ?1",
            [API_KEY_NAME],
        )? > 0)
    }

    /// THE one place the decision model's API key is read. For the envelope's
    /// transport only; a [`Secret`] neither serialises nor prints. The
    /// reference wins over a stored value. `Ok(None)` when none is set or
    /// none can be read.
    pub fn resolve_decision_credential(&self) -> Result<Option<Secret>, IpcError> {
        let row: Option<(Option<String>, Option<String>)> = self
            .conn
            .query_row(
                "SELECT value, credential_ref FROM decision_secrets WHERE name = ?1",
                [API_KEY_NAME],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((value, credential_ref)) = row else {
            return Ok(None);
        };
        Ok(credential_ref
            .as_deref()
            .and_then(read_credential_ref)
            .or(value)
            .map(Secret::new))
    }

    /// Whether a key is configured, and how — never the key.
    pub fn decision_credential_status(&self) -> Result<DecisionKeyStatus, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT credential_ref IS NOT NULL, COALESCE(updated_at, created_at) \
                 FROM decision_secrets WHERE name = ?1",
                [API_KEY_NAME],
                |r| {
                    Ok(DecisionKeyStatus {
                        configured: true,
                        by_reference: r.get::<_, i64>(0)? != 0,
                        set_at: r.get(1)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
    }

    /// The local HMAC key of `decision_runs.input_fp`: generated (32 random
    /// bytes, hex) on first use and kept; never sent anywhere.
    pub fn decision_fp_key(&self) -> Result<Secret, IpcError> {
        if let Some(k) = self.stored_fp_key()? {
            return Ok(Secret::new(k));
        }
        let mut bytes = [0u8; 32];
        {
            use rand::Rng;
            rand::rng().fill_bytes(&mut bytes);
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO decision_secrets (name, value, created_at) \
             VALUES ('fp_key', ?1, ?2)",
            rusqlite::params![hex::encode(bytes), now_unix()],
        )?;
        // Another writer may have won the insert: read what is stored.
        self.stored_fp_key()?
            .map(Secret::new)
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "fingerprint key vanished"))
    }

    /// The API key's literal form (and the fingerprint key's), for the
    /// diagnostics bundle's masking list.
    pub fn decision_secret_literals(&self) -> Result<Vec<String>, IpcError> {
        let mut out = Vec::new();
        if let Some(k) = self.resolve_decision_credential()? {
            out.push(k.expose().to_string());
        }
        out.extend(self.stored_fp_key()?);
        Ok(out)
    }

    /// The fingerprint key as stored, if one was generated: the one read of
    /// it ([`Self::decision_fp_key`] generates, this never does).
    fn stored_fp_key(&self) -> Result<Option<String>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM decision_secrets WHERE name = 'fp_key'",
                [],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{
        DecisionProposal, PROPOSAL_MIN_CONFIDENCE, PROPOSAL_SUBJECT_SESSION,
        PROPOSAL_SUBJECT_WORK_ITEM,
    };

    fn run(feature: &str) -> NewDecisionRun {
        NewDecisionRun {
            at: 1_000,
            feature: feature.into(),
            org_id: Some(1),
            subject_kind: "session".into(),
            subject_id: "42".into(),
            mode: "shadow".into(),
            provider: "jev".into(),
            model_version: Some("jev-1.13.0".into()),
            question_version: "wl.1".into(),
            input_fp: Some("ab".repeat(32)),
            candidates: vec!["ACME-1".into(), "ACME-2".into()],
            answer: Some("ACME-1".into()),
            probabilities: Some(BTreeMap::from([
                ("ACME-1".into(), 0.9),
                ("ACME-2".into(), 0.1),
            ])),
            confidence: Some(0.8),
            fallback: None,
            baseline_answer: Some("ACME-1".into()),
            called: true,
            latency_ms: Some(120),
            input_tokens: 500,
            cost_microusd: 21,
        }
    }

    /// Step 2.8: an assist run about a session proposes on its row.
    fn assist(subject: &str, feature: &str, answer: &str, confidence: f64) -> NewDecisionRun {
        NewDecisionRun {
            mode: "assist".into(),
            subject_id: subject.into(),
            answer: Some(answer.into()),
            confidence: Some(confidence),
            ..run(feature)
        }
    }

    fn session_with_bus() -> (Store, i64, std::sync::Arc<crate::events::RecordingEventBus>) {
        let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        s.upsert_host("h").unwrap();
        let id = s
            .upsert_session("w", "h", None, None, 1, 1, "running", None)
            .unwrap();
        (s, id, bus)
    }

    #[test]
    fn a_live_assist_answer_is_proposed_on_its_session_row() {
        let (s, id, bus) = session_with_bus();
        let other = s
            .upsert_session("w2", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let row = |id| s.get_session_by_id(id).unwrap().unwrap();
        assert!(row(id).proposals.is_empty(), "no decision, no proposal");

        bus.take();
        let run_id = s
            .insert_decision_run(&assist(&id.to_string(), "turn_outcome", "finished", 0.82))
            .unwrap();
        assert!(
            bus.take()
                .iter()
                .any(|e| e.starts_with("session") && e.ends_with(&format!(":{id}"))),
            "the row is re-emitted: its proposals changed"
        );
        assert_eq!(
            row(id).proposals,
            vec![DecisionProposal {
                feature: "turn_outcome".into(),
                value: "finished".into(),
                source: "jev".into(),
                reason: None,
                confidence_pct: Some(82),
                run_id: Some(run_id),
                at: Some(1_000),
                linked: None,
            }]
        );
        assert!(row(other).proposals.is_empty(), "only the subject's row");

        // Deciding it takes it off the row, and says so.
        bus.take();
        assert!(s
            .set_decision_followup(run_id, "confirmed", None, 2_000)
            .unwrap());
        assert!(row(id).proposals.is_empty());
        assert!(bus.take().iter().any(|e| e.ends_with(&format!(":{id}"))));
    }

    #[test]
    fn only_the_latest_live_assist_answer_per_feature_is_proposed() {
        let (s, id, _bus) = session_with_bus();
        let subject = id.to_string();
        let row = || s.get_session_by_id(id).unwrap().unwrap();
        let features =
            || -> Vec<String> { row().proposals.into_iter().map(|p| p.feature).collect() };

        // A shadow answer is recorded, never shown.
        s.insert_decision_run(&NewDecisionRun {
            subject_id: subject.clone(),
            ..run("a")
        })
        .unwrap();
        assert!(features().is_empty());
        // Unsure, and an answer under the floor, pre-select nothing.
        s.insert_decision_run(&assist(&subject, "b", "unsure", 0.9))
            .unwrap();
        s.insert_decision_run(&assist(&subject, "c", "x", PROPOSAL_MIN_CONFIDENCE - 0.01))
            .unwrap();
        assert!(features().is_empty());
        // At the floor it is proposed; one per feature, sorted.
        s.insert_decision_run(&assist(&subject, "c", "x", PROPOSAL_MIN_CONFIDENCE))
            .unwrap();
        s.insert_decision_run(&assist(&subject, "a", "y", 0.9))
            .unwrap();
        assert_eq!(features(), ["a", "c"]);
        // A later rule answer replaces Jev's for its feature.
        s.insert_decision_run(&NewDecisionRun {
            provider: "rules".into(),
            ..assist(&subject, "a", "z", 0.9)
        })
        .unwrap();
        let a = row()
            .proposals
            .into_iter()
            .find(|p| p.feature == "a")
            .unwrap();
        assert_eq!((a.value.as_str(), a.source.as_str()), ("z", "rule"));
        // A later fallback withdraws the feature's proposal.
        s.insert_decision_run(&NewDecisionRun {
            fallback: Some("timeout".into()),
            answer: None,
            ..assist(&subject, "c", "x", 0.9)
        })
        .unwrap();
        assert_eq!(features(), ["a"]);
    }

    /// The Work view reads many subjects at once through
    /// [`Store::current_proposals`]; a row reads one through
    /// [`proposals_sql!`]. Both keep the same runs, with the same floor.
    #[test]
    fn the_bulk_read_and_the_row_read_keep_the_same_runs() {
        let (s, id, _bus) = session_with_bus();
        let subject = id.to_string();
        for (f, a, c) in [
            ("a", "x", PROPOSAL_MIN_CONFIDENCE),
            ("b", "y", PROPOSAL_MIN_CONFIDENCE - 0.01),
            ("c", "unsure", 0.9),
            ("d", "z", 0.7),
        ] {
            s.insert_decision_run(&assist(&subject, f, a, c)).unwrap();
        }
        let bulk = s.current_proposals(PROPOSAL_SUBJECT_SESSION).unwrap();
        assert_eq!(
            bulk.get(&subject).cloned().unwrap_or_default(),
            s.get_session_by_id(id).unwrap().unwrap().proposals
        );
        assert_eq!(bulk[&subject].len(), 2);
        assert!(s
            .current_proposals(PROPOSAL_SUBJECT_WORK_ITEM)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn runs_for_subjects_match_the_kind_and_the_prefix_exactly() {
        let s = Store::open_in_memory().unwrap();
        let at = |subject: &str, kind: &str, at: i64| NewDecisionRun {
            subject_kind: kind.into(),
            subject_id: subject.into(),
            at,
            ..run("status_map")
        };
        let a = s
            .insert_decision_run(&at("3:aa", "tracker_section", 100))
            .unwrap();
        let b = s
            .insert_decision_run(&at("3:bb", "tracker_section", 200))
            .unwrap();
        s.insert_decision_run(&at("31:aa", "tracker_section", 200))
            .unwrap();
        s.insert_decision_run(&at("3:aa", "session", 200)).unwrap();
        s.insert_decision_run(&NewDecisionRun {
            subject_kind: "tracker_section".into(),
            subject_id: "3:cc".into(),
            ..run("work_link")
        })
        .unwrap();
        let ids = |since| {
            s.decision_runs_for_subjects("status_map", "tracker_section", "3:", since)
                .unwrap()
                .into_iter()
                .map(|r| r.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(None),
            vec![b, a],
            "newest first; not 31:, not another kind or feature"
        );
        assert_eq!(ids(Some(150)), vec![b]);
    }

    #[test]
    fn a_run_round_trips_and_takes_a_followup() {
        let s = Store::open_in_memory().unwrap();
        let id = s.insert_decision_run(&run("work_link")).unwrap();
        let r = s.get_decision_run(id).unwrap().unwrap();
        assert_eq!(r.candidates, vec!["ACME-1", "ACME-2"]);
        assert_eq!(r.probabilities.as_ref().unwrap()["ACME-1"], 0.9);
        assert!(r.called);
        assert!(r.followup.is_none());
        assert!(s
            .set_decision_followup(id, "corrected", Some("ACME-2"), 2_000)
            .unwrap());
        let r = s.get_decision_run(id).unwrap().unwrap();
        assert_eq!(r.followup.as_deref(), Some("corrected"));
        assert_eq!(r.corrected_to.as_deref(), Some("ACME-2"));
        assert_eq!(r.followup_at, Some(2_000));
        assert!(s.set_decision_followup(id, "maybe", None, 1).is_err());
        assert!(s
            .set_decision_followup(id, "confirmed", Some("ACME-2"), 1)
            .is_err());
        assert!(!s.set_decision_followup(9_999, "ignored", None, 1).unwrap());
    }

    #[test]
    fn text_never_gets_into_a_run() {
        let s = Store::open_in_memory().unwrap();
        let prose = "fix the login bug for bob@example.com";
        for bad in [
            NewDecisionRun {
                answer: Some(prose.into()),
                ..run("work_link")
            },
            NewDecisionRun {
                candidates: vec![prose.into()],
                ..run("work_link")
            },
            NewDecisionRun {
                baseline_answer: Some(prose.into()),
                ..run("work_link")
            },
            NewDecisionRun {
                subject_id: prose.into(),
                ..run("work_link")
            },
            NewDecisionRun {
                input_fp: Some("not a fingerprint".into()),
                ..run("work_link")
            },
            NewDecisionRun {
                fallback: Some("because".into()),
                ..run("work_link")
            },
            NewDecisionRun {
                mode: "auto".into(),
                ..run("work_link")
            },
            NewDecisionRun {
                confidence: Some(f64::NAN),
                ..run("work_link")
            },
        ] {
            assert_eq!(
                s.insert_decision_run(&bad).unwrap_err().code,
                codes::E_INVALID,
                "{bad:?}"
            );
        }
        assert_eq!(s.decision_run_count().unwrap(), 0);
        assert!(s
            .set_decision_followup(1, "corrected", Some(prose), 1)
            .is_err());
    }

    #[test]
    fn the_breaker_streak_counts_failed_calls_in_a_row() {
        let s = Store::open_in_memory().unwrap();
        let fail = |at: i64, f: &str| NewDecisionRun {
            at,
            fallback: Some(f.into()),
            answer: None,
            ..run("work_link")
        };
        assert_eq!(
            s.decision_failure_streak("jev", 5, RunScope::Live).unwrap(),
            (0, None)
        );
        s.insert_decision_run(&fail(10, "timeout")).unwrap();
        s.insert_decision_run(&run("work_link")).unwrap();
        s.insert_decision_run(&fail(20, "http_error")).unwrap();
        s.insert_decision_run(&fail(30, "rate_limited")).unwrap();
        // A refusal that sent nothing is not a call: it neither counts nor
        // breaks the streak.
        s.insert_decision_run(&NewDecisionRun {
            called: false,
            ..fail(40, "breaker_open")
        })
        .unwrap();
        assert_eq!(
            s.decision_failure_streak("jev", 5, RunScope::Live).unwrap(),
            (2, Some(30))
        );
        assert_eq!(
            s.decision_failure_streak("jev", 1, RunScope::Live).unwrap(),
            (1, Some(30))
        );
        // An invalid answer is a call that came back: it ends a streak.
        s.insert_decision_run(&fail(50, "invalid_answer")).unwrap();
        assert_eq!(
            s.decision_failure_streak("jev", 5, RunScope::Live).unwrap(),
            (0, None)
        );
        assert_eq!(
            s.decision_failure_streak("rules", 5, RunScope::Live)
                .unwrap(),
            (0, None)
        );
    }

    #[test]
    fn usage_stats_listing_and_sweep() {
        let s = Store::open_in_memory().unwrap();
        s.insert_decision_run(&NewDecisionRun {
            at: 100,
            ..run("work_link")
        })
        .unwrap();
        s.insert_decision_run(&NewDecisionRun {
            at: 200,
            answer: Some("ACME-2".into()),
            ..run("work_link")
        })
        .unwrap();
        s.insert_decision_run(&NewDecisionRun {
            at: 300,
            feature: "status_map".into(),
            fallback: Some("flag_off".into()),
            called: false,
            answer: None,
            input_tokens: 0,
            cost_microusd: 0,
            ..run("status_map")
        })
        .unwrap();
        assert_eq!(
            s.decision_usage_since("jev", 150, RunScope::Live).unwrap(),
            (500, 21)
        );
        assert_eq!(
            s.decision_usage_since("jev", 0, RunScope::Live).unwrap(),
            (1000, 42)
        );
        let stats = s.decision_stats(0).unwrap();
        let wl = stats.iter().find(|r| r.feature == "work_link").unwrap();
        assert_eq!((wl.runs, wl.called, wl.compared, wl.agreed), (2, 2, 2, 1));
        let sm = stats.iter().find(|r| r.feature == "status_map").unwrap();
        assert_eq!(sm.fallback.as_deref(), Some("flag_off"));
        assert_eq!((sm.runs, sm.called), (1, 0));
        let listed = s
            .list_decision_runs(&DecisionRunFilter {
                feature: Some("work_link".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed[0].id > listed[1].id, "newest first");
        assert_eq!(s.sweep_decision_runs(250, 1).unwrap(), 1);
        assert_eq!(s.sweep_decision_runs(250, 10).unwrap(), 1);
        assert_eq!(s.decision_run_count().unwrap(), 1);
    }

    #[test]
    fn a_baseline_that_abstained_is_not_compared() {
        let s = Store::open_in_memory().unwrap();
        let shadow = |answer: &str, baseline: &str| NewDecisionRun {
            answer: Some(answer.into()),
            baseline_answer: Some(baseline.into()),
            ..run("status_map")
        };
        s.insert_decision_run(&shadow("done", "done")).unwrap();
        s.insert_decision_run(&shadow("todo", DECISION_NO_BASELINE))
            .unwrap();
        s.insert_decision_run(&shadow("todo", DECISION_NO_BASELINE))
            .unwrap();
        let stats = s.decision_stats(0).unwrap();
        let sm = stats.iter().find(|r| r.feature == "status_map").unwrap();
        assert_eq!((sm.runs, sm.compared, sm.agreed), (3, 1, 1));
    }

    #[test]
    fn a_benchmarks_runs_never_count_toward_the_live_breaker_budget_or_stats() {
        let s = Store::open_in_memory().unwrap();
        let bench = |at: i64, fallback: Option<&str>| NewDecisionRun {
            at,
            subject_kind: DECISION_BENCH_SUBJECT.into(),
            subject_id: format!("s{at}"),
            fallback: fallback.map(str::to_string),
            answer: fallback.is_none().then(|| "ACME-1".into()),
            ..run("work_link")
        };
        s.insert_decision_run(&run("work_link")).unwrap();
        for at in [2_000, 2_001, 2_002] {
            s.insert_decision_run(&bench(at, Some("timeout"))).unwrap();
        }
        // The live breaker and budget see the live run only.
        assert_eq!(
            s.decision_failure_streak("jev", 5, RunScope::Live).unwrap(),
            (0, None)
        );
        assert_eq!(
            s.decision_usage_since("jev", 0, RunScope::Live).unwrap(),
            (500, 21)
        );
        // A benchmark call is gated on everything.
        assert_eq!(
            s.decision_failure_streak("jev", 5, RunScope::All).unwrap(),
            (3, Some(2_002))
        );
        assert_eq!(
            s.decision_usage_since("jev", 0, RunScope::All).unwrap(),
            (2_000, 84)
        );
        // The stats keep them apart: the live row counts one run.
        let stats = s.decision_stats(0).unwrap();
        let live: Vec<_> = stats.iter().filter(|r| !r.bench).collect();
        assert_eq!(live.len(), 1);
        assert_eq!((live[0].runs, live[0].called), (1, 1));
        let b = stats.iter().find(|r| r.bench).unwrap();
        assert_eq!(
            (b.feature.as_str(), b.fallback.as_deref(), b.runs),
            ("work_link", Some("timeout"), 3)
        );
    }

    #[test]
    fn the_sweep_keeps_what_a_person_decided() {
        let s = Store::open_in_memory().unwrap();
        let mut ids = BTreeMap::new();
        for f in ["none", "confirmed", "rejected", "corrected", "ignored"] {
            let id = s
                .insert_decision_run(&NewDecisionRun {
                    at: 100,
                    ..run("status_map")
                })
                .unwrap();
            if f != "none" {
                let to = (f == "corrected").then_some("done");
                assert!(s.set_decision_followup(id, f, to, 150).unwrap());
            }
            ids.insert(f, id);
        }
        assert_eq!(s.sweep_decision_runs(1_000, 100).unwrap(), 2);
        for (f, id) in &ids {
            let kept = s.get_decision_run(*id).unwrap().is_some();
            assert_eq!(
                kept,
                DECISION_PERSON_FOLLOWUPS.contains(f),
                "{f}: kept {kept}"
            );
        }
        // Nothing more to sweep: the kept rows do not stall the batches.
        assert_eq!(s.sweep_decision_runs(1_000, 100).unwrap(), 0);
    }

    const KEY: &str = "tsk_live_0123456789abcdefghijklmnop";

    #[test]
    fn the_key_is_stored_resolved_and_cleared_but_never_listed() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.resolve_decision_credential().unwrap().is_none());
        assert!(!s.decision_credential_status().unwrap().configured);
        assert!(s.set_decision_credential(None, None).is_err());
        assert!(s
            .set_decision_credential(Some(&Secret::new(KEY)), Some("env:X"))
            .is_err());
        assert!(s
            .set_decision_credential(Some(&Secret::new("two words")), None)
            .is_err());
        assert!(s.set_decision_credential(None, Some("http://x")).is_err());
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
        let k = s.resolve_decision_credential().unwrap().unwrap();
        assert_eq!(k.expose(), KEY);
        assert!(!format!("{k:?} {k}").contains(KEY));
        let st = s.decision_credential_status().unwrap();
        assert!(st.configured && !st.by_reference && st.set_at.is_some());
        assert!(!serde_json::to_string(&st).unwrap().contains(KEY));
        assert!(s
            .decision_secret_literals()
            .unwrap()
            .contains(&KEY.to_string()));
        // The fingerprint key survives clearing the API key.
        let fp = s.decision_fp_key().unwrap();
        assert_eq!(fp.expose().len(), 64);
        assert!(s.clear_decision_credential().unwrap());
        assert!(!s.clear_decision_credential().unwrap());
        assert!(s.resolve_decision_credential().unwrap().is_none());
        assert_eq!(s.decision_fp_key().unwrap().expose(), fp.expose());
    }

    #[test]
    fn a_reference_is_read_at_use_and_wins() {
        let s = Store::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jev");
        std::fs::write(&path, format!("{KEY}\n")).unwrap();
        s.set_decision_credential(None, Some(&format!("file:{}", path.display())))
            .unwrap();
        assert_eq!(
            s.resolve_decision_credential().unwrap().unwrap().expose(),
            KEY
        );
        assert!(s.decision_credential_status().unwrap().by_reference);
        std::fs::remove_file(&path).unwrap();
        assert!(s.resolve_decision_credential().unwrap().is_none());
    }

    /// Source guard: the key row has one reader, and the secret type never
    /// grows a `Serialize` derive (it lives in `trackers.rs`, guarded there).
    #[test]
    fn the_api_key_has_one_reader() {
        let src = include_str!("decisions.rs");
        let src = &src[..src.find("#[cfg(test)]").unwrap()];
        assert_eq!(
            src.matches("SELECT value, credential_ref FROM decision_secrets")
                .count(),
            1,
            "the API key is read by resolve_decision_credential only"
        );
        // The API key's reader, its writers' delete / lookup, and the
        // fingerprint key's one reader (`stored_fp_key`).
        assert_eq!(src.matches("FROM decision_secrets").count(), 4);
    }
}
