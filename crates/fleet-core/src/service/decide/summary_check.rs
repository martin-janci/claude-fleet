//! J9 `summary_check` (redesign step 11.11): whether a cheap model's summary
//! of a session's recent turns is supported by those turns, asked before the
//! summary is shown to someone watching the session.
//!
//! * **When.** Right after `watch_summary` drafted a "Since 13:20" summary,
//!   once per draft, on the caller's path (the summary waits for it).
//! * **What is sent.** The summary and the transcript excerpt it was written
//!   from (the newest [`TRANSCRIPT_MAX_BYTES`]), both through the envelope's
//!   redaction, and one Noul: "every statement is supported".
//! * **Off** asks nothing: the summary shows, marked unchecked. **Shadow**
//!   records the answer and the summary shows, marked unchecked. **Assist**
//!   shows the summary only when the answer is at or above [`PASS_FLOOR`];
//!   a lower answer, or no answer at all (a fallback), hides it.

use super::{decide, DecideCtx, DecideRequest, DecisionOutcome, Feature, JevRequest, Mode};
use super::{NoulCriteria, Question};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "summary_check.v1";
/// What a run is about: the session (its id as text).
pub const SUBJECT_KIND: &str = "session";
/// At or above this a summary is supported.
pub const PASS_FLOOR: f64 = 0.5;
/// The newest bytes of the excerpt sent, at most: the request stays well
/// under Jev's 32k-token state limit ([`super::jev::MAX_REQUEST_BYTES`]).
pub const TRANSCRIPT_MAX_BYTES: usize = 80_000;

/// The instruction, read literally.
pub const INSTRUCTIONS: &str = "state.summary is a short summary of state.transcript, written \
    by another model for a person who is watching the session. Decide whether every statement \
    in state.summary is supported by state.transcript: nothing invented, nothing contradicted, \
    nothing claimed finished that the transcript does not show finished.";

const SUPPORTED: &str = "Every statement in the summary is supported by the transcript.";
const UNSUPPORTED: &str = "The summary states something the transcript does not support.";

/// What the check said about one summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    /// `decide.jev.summary_check` is off: nothing was asked.
    Off,
    /// Asked and recorded only; the summary shows unchecked.
    Shadow,
    /// Jev found it supported.
    Passed,
    /// Jev found it unsupported: the summary is hidden.
    Failed,
    /// Assist, but no usable answer (the gate refused, the call failed):
    /// the summary is hidden, since it could not be checked.
    Unchecked,
}

impl Check {
    /// Whether the summary may be shown.
    pub fn shows(self) -> bool {
        matches!(self, Check::Off | Check::Shadow | Check::Passed)
    }
}

/// PURE: the question.
pub fn question() -> Question {
    Question::Noul {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria: Some(NoulCriteria {
            yes: Value::String(SUPPORTED.into()),
            no: Value::String(UNSUPPORTED.into()),
        }),
    }
}

/// PURE: the newest `max` bytes of `text`, cut at a character boundary.
pub fn tail_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// PURE: the request for `summary` of `transcript`.
pub fn request(summary: &str, transcript: &str) -> JevRequest {
    JevRequest {
        state: json!({
            "summary": summary,
            "transcript": tail_bytes(transcript, TRANSCRIPT_MAX_BYTES),
        }),
        question: question(),
    }
}

/// PURE: the check an outcome makes in `mode` (the feature's setting).
pub fn verdict(mode: super::FeatureMode, outcome: Option<&DecisionOutcome>) -> Check {
    use super::FeatureMode;
    match mode {
        FeatureMode::Off => Check::Off,
        FeatureMode::Shadow => Check::Shadow,
        FeatureMode::Assist => {
            let Some(o) = outcome.filter(|o| o.mode == Some(Mode::Assist)) else {
                return Check::Unchecked;
            };
            match o.usable().and_then(|a| a.value.trim().parse::<f64>().ok()) {
                Some(v) if v >= PASS_FLOOR => Check::Passed,
                Some(_) => Check::Failed,
                None => Check::Unchecked,
            }
        }
    }
}

/// Check `summary` of `transcript` for session `session_id`. Asks nothing
/// when the feature is off; never errors (a failed call is
/// [`Check::Unchecked`]).
pub async fn check(
    ctx: &DecideCtx,
    session_id: i64,
    org_id: Option<i64>,
    summary: &str,
    transcript: &str,
) -> Check {
    let mode = match ctx.store.lock() {
        Ok(s) => super::FeatureMode::of(&s, Feature::SummaryCheck),
        Err(_) => return Check::Unchecked,
    };
    if mode == super::FeatureMode::Off {
        return Check::Off;
    }
    let outcome = decide(
        ctx,
        DecideRequest {
            feature: Feature::SummaryCheck,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: session_id.to_string(),
            org_id,
            request: request(summary, transcript),
            baseline: None,
            question_version: QUESTION_VERSION.into(),
            min_confidence: None,
        },
    )
    .await;
    verdict(mode, Some(&outcome))
}
