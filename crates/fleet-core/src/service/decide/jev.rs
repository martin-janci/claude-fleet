//! The Jev client (TypeSafe AI's decision model): the request and answer
//! types of `POST https://api.typesafe.ai/v1/systemone`, their local checks,
//! and [`JevBackend`], the one [`DecisionBackend`] that talks to it — over
//! the same [`crate::net::https::HttpTransport`] seam the trackers use,
//! fenced to exactly `api.typesafe.ai`.
//!
//! One question per call (the envelope records one answer per run). The
//! API takes a map of questions; this client always sends one, under
//! [`QUESTION_ID`].
//!
//! Verified against <https://docs.typesafe.ai/api.md> on 2026-09-27: a
//! `noul` answers `{"noul": 0.95}`; a `choice` (at most 255 options)
//! answers `{"choice", "probabilities", "confidence"}`; a `score` (2–10
//! levels) answers `{"score", "legend", "probabilities", "confidence"}`.
//! Errors: 401, 422, 429 (rate limit, may carry `retry-after`), 529
//! (overloaded). $0.042 per million input tokens, output free.

use super::Fallback;
use crate::net::https::{DirectTransport, HttpTransport, Request, TransportError};
use crate::store::{is_decision_word, Secret};
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// The endpoint.
pub const JEV_URL: &str = "https://api.typesafe.ai/v1/systemone";
/// The one host the transport may connect to.
pub const JEV_HOST: &str = "api.typesafe.ai";
/// The provider name a run records.
pub const PROVIDER_JEV: &str = "jev";
/// The id the one question travels under.
pub const QUESTION_ID: &str = "q";
/// Jev's limit on a choice's options.
pub const MAX_CHOICE_OPTIONS: usize = 255;
/// A score's levels.
pub const SCORE_LEVELS: std::ops::RangeInclusive<usize> = 2..=10;
/// The request body's cap, in bytes. Jev takes 32k tokens for the state
/// plus the longest question; at a conservative ~3.5 bytes per token this
/// stays under it without a tokenizer.
pub const MAX_REQUEST_BYTES: usize = 110_000;
/// Micro-USD per million input tokens ($0.042); output is free.
pub const MICROUSD_PER_MILLION_INPUT: i64 = 42_000;
/// How far a distribution's sum may stray from 1.
const SUM_TOLERANCE: f64 = 0.02;

/// PURE: what `input_tokens` cost, in micro-USD, rounded up.
pub fn cost_microusd(input_tokens: i64) -> i64 {
    let t = input_tokens.max(0);
    (t * MICROUSD_PER_MILLION_INPUT + 999_999) / 1_000_000
}

/// A noul's two ends, described.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoulCriteria {
    #[serde(rename = "true")]
    pub yes: Value,
    #[serde(rename = "false")]
    pub no: Value,
}

/// One question. `instructions` and descriptions may be a string or an
/// object (the API takes both).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    /// A degree of truth in 0..=1.
    Noul {
        instructions: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    /// One of the options: each key an id or vocabulary word (it is
    /// recorded), each value its description or `null`.
    Choice {
        instructions: Value,
        criteria: BTreeMap<String, Option<Value>>,
    },
    /// A level `0..n-1` (fractional), each described.
    Score {
        instructions: Value,
        criteria: Vec<Value>,
    },
}

impl Question {
    pub fn kind(&self) -> &'static str {
        match self {
            Question::Noul { .. } => "noul",
            Question::Choice { .. } => "choice",
            Question::Score { .. } => "score",
        }
    }

    /// What the run records as its candidates: a choice's option keys, a
    /// score's level numbers, nothing for a noul.
    pub fn candidates(&self) -> Vec<String> {
        match self {
            Question::Noul { .. } => Vec::new(),
            Question::Choice { criteria, .. } => criteria.keys().cloned().collect(),
            Question::Score { criteria, .. } => {
                (0..criteria.len()).map(|i| i.to_string()).collect()
            }
        }
    }

    /// The checks the API would answer 422 to, made before anything is sent,
    /// plus fleet's own: a choice's keys are ids / words.
    pub fn check(&self) -> Result<(), String> {
        match self {
            Question::Noul { .. } => Ok(()),
            Question::Choice { criteria, .. } => {
                if criteria.len() < 2 || criteria.len() > MAX_CHOICE_OPTIONS {
                    return Err(format!(
                        "a choice takes 2-{MAX_CHOICE_OPTIONS} options, not {}",
                        criteria.len()
                    ));
                }
                match criteria.keys().find(|k| !is_decision_word(k)) {
                    Some(_) => Err("a choice's options are ids or vocabulary words".into()),
                    None => Ok(()),
                }
            }
            Question::Score { criteria, .. } => {
                if SCORE_LEVELS.contains(&criteria.len()) {
                    Ok(())
                } else {
                    Err(format!("a score takes 2-10 levels, not {}", criteria.len()))
                }
            }
        }
    }

    /// Every text value run through `f` (keys untouched: they are ids).
    fn map_text(&self, f: &dyn Fn(&str) -> String) -> Question {
        match self {
            Question::Noul {
                instructions,
                criteria,
            } => Question::Noul {
                instructions: map_value(instructions, f),
                criteria: criteria.as_ref().map(|c| NoulCriteria {
                    yes: map_value(&c.yes, f),
                    no: map_value(&c.no, f),
                }),
            },
            Question::Choice {
                instructions,
                criteria,
            } => Question::Choice {
                instructions: map_value(instructions, f),
                criteria: criteria
                    .iter()
                    .map(|(k, v)| (k.clone(), v.as_ref().map(|v| map_value(v, f))))
                    .collect(),
            },
            Question::Score {
                instructions,
                criteria,
            } => Question::Score {
                instructions: map_value(instructions, f),
                criteria: criteria.iter().map(|v| map_value(v, f)).collect(),
            },
        }
    }
}

/// Every string leaf of `v` through `f`; object keys kept.
pub(super) fn map_value(v: &Value, f: &dyn Fn(&str) -> String) -> Value {
    match v {
        Value::String(s) => Value::String(f(s)),
        Value::Array(a) => Value::Array(a.iter().map(|x| map_value(x, f)).collect()),
        // Keys too: an adapter that keys its state by a URL or an address
        // would otherwise send it as written (review r15 F08).
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (f(k), map_value(x, f))).collect()),
        other => other.clone(),
    }
}

/// What an adapter asks: a state (text or JSON) and one question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevRequest {
    pub state: Value,
    pub question: Question,
}

impl JevRequest {
    /// The request with every text value through [`super::redact_state`]:
    /// what is fingerprinted and sent.
    pub fn redacted(&self) -> JevRequest {
        let f = |s: &str| super::redact_state(s);
        JevRequest {
            state: map_value(&self.state, &f),
            question: self.question.map_text(&f),
        }
    }

    /// The API body for `model`, as JSON. Its objects are sorted (fleet
    /// builds `serde_json` without `preserve_order`), so a choice's options
    /// read here in key order; [`Self::wire_body`] is what is sent.
    pub fn api_body(&self, model: &str) -> Value {
        serde_json::json!({
            "state": self.state,
            "model": model,
            "questions": { QUESTION_ID: self.question },
        })
    }

    /// The bytes sent for `model`: [`Self::api_body`] with a choice's
    /// options in [`Self::option_order`] instead of key order. Jev leans
    /// toward the first option it reads (TypeSafe's jaggedness notes for
    /// jev-1.13), and key order would put the same option first every time
    /// (`done` for `status_map`, the lowest id for `work_link`).
    pub fn wire_body(&self, model: &str) -> Vec<u8> {
        let order = self.option_order();
        let body = WireBody {
            req: self,
            model,
            order: &order,
        };
        serde_json::to_vec(&body).expect("a JSON value always serializes")
    }

    /// PURE: a choice's option keys in the order they are offered — a
    /// permutation keyed by the request itself (SHA-256 of its canonical
    /// JSON), so the same request always reads the same way (a run can be
    /// replayed, a benchmark repeated) while no option holds the first place
    /// across requests. Empty for a noul or a score: their order means
    /// something.
    pub fn option_order(&self) -> Vec<String> {
        let Question::Choice { criteria, .. } = &self.question else {
            return Vec::new();
        };
        let v = serde_json::json!({ "state": self.state, "question": self.question });
        let seed = Sha256::digest(super::canonical_json(&v).as_bytes());
        let mut keyed: Vec<([u8; 32], &String)> = criteria
            .keys()
            .map(|k| {
                let mut h = Sha256::new();
                h.update(seed);
                h.update(k.as_bytes());
                (h.finalize().into(), k)
            })
            .collect();
        keyed.sort();
        keyed.into_iter().map(|(_, k)| k.clone()).collect()
    }

    /// [`Question::check`] plus the size cap.
    pub fn check(&self, model: &str) -> Result<(), String> {
        self.question.check()?;
        let n = self.api_body(model).to_string().len();
        if n > MAX_REQUEST_BYTES {
            return Err(format!(
                "the request is {n} bytes; at most {MAX_REQUEST_BYTES} fit Jev's token limit"
            ));
        }
        Ok(())
    }
}

/// [`JevRequest::wire_body`]'s shape: the API body written field by field,
/// so a choice's options keep the order given.
struct WireBody<'a> {
    req: &'a JevRequest,
    model: &'a str,
    order: &'a [String],
}

impl Serialize for WireBody<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(3))?;
        m.serialize_entry("state", &self.req.state)?;
        m.serialize_entry("model", self.model)?;
        m.serialize_entry(
            "questions",
            &OneQuestion(WireQuestion {
                q: &self.req.question,
                order: self.order,
            }),
        )?;
        m.end()
    }
}

/// `{ QUESTION_ID: question }`.
struct OneQuestion<'a>(WireQuestion<'a>);

impl Serialize for OneQuestion<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(1))?;
        m.serialize_entry(QUESTION_ID, &self.0)?;
        m.end()
    }
}

/// A question as sent: a choice's options in `order`, anything else as
/// [`Question`] serializes it.
struct WireQuestion<'a> {
    q: &'a Question,
    order: &'a [String],
}

impl Serialize for WireQuestion<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let Question::Choice {
            instructions,
            criteria,
        } = self.q
        else {
            return self.q.serialize(s);
        };
        let mut m = s.serialize_map(Some(3))?;
        m.serialize_entry("type", "choice")?;
        m.serialize_entry("instructions", instructions)?;
        m.serialize_entry(
            "criteria",
            &Ordered {
                criteria,
                order: self.order,
            },
        )?;
        m.end()
    }
}

/// A choice's options in `order` (every key of `criteria` exactly once:
/// [`JevRequest::option_order`] is a permutation of them).
struct Ordered<'a> {
    criteria: &'a BTreeMap<String, Option<Value>>,
    order: &'a [String],
}

impl Serialize for Ordered<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(self.order.len()))?;
        for k in self.order {
            if let Some(v) = self.criteria.get(k) {
                m.serialize_entry(k, v)?;
            }
        }
        m.end()
    }
}

/// One answer, as the API returns it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        #[serde(default)]
        probabilities: BTreeMap<String, f64>,
        #[serde(default)]
        confidence: Option<f64>,
    },
    Score {
        score: f64,
        #[serde(default)]
        legend: BTreeMap<String, Value>,
        #[serde(default)]
        probabilities: BTreeMap<String, f64>,
        #[serde(default)]
        confidence: Option<f64>,
    },
}

/// Token usage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

/// The API's response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevResponse {
    pub model: String,
    #[serde(default)]
    pub answers: BTreeMap<String, Answer>,
    #[serde(default)]
    pub usage: Usage,
}

/// A checked answer, as the run records it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidAnswer {
    /// A choice's option, or a noul / score value as a number.
    pub value: String,
    pub probabilities: Option<BTreeMap<String, f64>>,
    /// The answer's top probability ([`top_probability`]): what every floor
    /// is compared with, what the run records and what a proposal shows.
    /// For a noul, `max(p, 1 - p)`.
    pub confidence: Option<f64>,
}

/// PURE: how sure an answer is, as one number: the probability of the
/// chosen option for a choice, the largest level probability for a score;
/// the model's own `confidence` field only when it sent no distribution
/// (or none for the option chosen). Independent tests of jev-1.13 found
/// that field never better calibrated than the top probability, so it is
/// not what a floor reads.
pub fn top_probability(
    q: &Question,
    value: &str,
    probabilities: Option<&BTreeMap<String, f64>>,
    confidence: Option<f64>,
) -> Option<f64> {
    let p = probabilities.filter(|p| !p.is_empty());
    let top = match q {
        Question::Choice { .. } => p.and_then(|p| p.get(value).copied()),
        Question::Score { .. } => p.and_then(|p| p.values().copied().reduce(f64::max)),
        Question::Noul { .. } => None,
    };
    top.or(confidence)
}

/// PURE: `x` as a short decimal (`0.95`, `1.05`, `1`).
pub fn fmt_num(x: f64) -> String {
    let s = format!("{x:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s.to_string()
    }
}

fn unit(x: f64) -> bool {
    x.is_finite() && (0.0..=1.0).contains(&x)
}

/// Probabilities: every key among `allowed`, every value in 0..=1, summing
/// to ~1. An empty map is allowed (and recorded as none).
fn check_distribution(
    p: &BTreeMap<String, f64>,
    allowed: &[String],
) -> Result<Option<BTreeMap<String, f64>>, String> {
    if p.is_empty() {
        return Ok(None);
    }
    if let Some(k) = p.keys().find(|k| !allowed.contains(k)) {
        return Err(format!("a probability for {k:?}, which was not offered"));
    }
    if !p.values().all(|v| unit(*v)) {
        return Err("a probability outside 0..1".into());
    }
    let sum: f64 = p.values().sum();
    if (sum - 1.0).abs() > SUM_TOLERANCE {
        return Err(format!("probabilities sum to {sum}, not 1"));
    }
    Ok(Some(p.clone()))
}

/// PURE: check `a` against the question it answers.
pub fn validate_answer(q: &Question, a: &Answer) -> Result<ValidAnswer, String> {
    let offered = q.candidates();
    match (q, a) {
        (Question::Noul { .. }, Answer::Noul { noul }) => {
            if !unit(*noul) {
                return Err(format!("a noul of {noul}"));
            }
            Ok(ValidAnswer {
                value: fmt_num(*noul),
                probabilities: None,
                confidence: Some(noul.max(1.0 - noul)),
            })
        }
        (
            Question::Choice { .. },
            Answer::Choice {
                choice,
                probabilities,
                confidence,
            },
        ) => {
            if !offered.contains(choice) {
                return Err("a choice that was not offered".into());
            }
            if confidence.is_some_and(|c| !unit(c)) {
                return Err("a confidence outside 0..1".into());
            }
            let probabilities = check_distribution(probabilities, &offered)?;
            Ok(ValidAnswer {
                confidence: top_probability(q, choice, probabilities.as_ref(), *confidence),
                value: choice.clone(),
                probabilities,
            })
        }
        (
            Question::Score { criteria, .. },
            Answer::Score {
                score,
                probabilities,
                confidence,
                ..
            },
        ) => {
            let top = (criteria.len() - 1) as f64;
            if !score.is_finite() || *score < 0.0 || *score > top {
                return Err(format!("a score of {score} on 0..{top}"));
            }
            if confidence.is_some_and(|c| !unit(c)) {
                return Err("a confidence outside 0..1".into());
            }
            let probabilities = check_distribution(probabilities, &offered)?;
            let value = fmt_num(*score);
            Ok(ValidAnswer {
                confidence: top_probability(q, &value, probabilities.as_ref(), *confidence),
                value,
                probabilities,
            })
        }
        _ => Err(format!("a {} question answered as another type", q.kind())),
    }
}

/// Why a backend returned no response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// The call outlived its timeout.
    Timeout,
    /// 429; `retry_after` in seconds when the API said.
    RateLimited { retry_after: Option<u64> },
    /// 529.
    Overloaded,
    /// Any other non-2xx status (401, 422, 5xx, a redirect).
    Http { status: u16 },
    /// No exchange: refused, DNS, TCP, TLS, a broken answer on the wire.
    Transport(String),
    /// A 2xx whose body is not the API's shape.
    Unreadable(String),
}

impl BackendError {
    /// The fallback a run records for it. 429 and 529 are `rate_limited`;
    /// a 2xx the client cannot read is `invalid_answer`.
    pub fn fallback(&self) -> Fallback {
        match self {
            BackendError::Timeout => Fallback::Timeout,
            BackendError::RateLimited { .. } | BackendError::Overloaded => Fallback::RateLimited,
            BackendError::Http { .. } | BackendError::Transport(_) => Fallback::HttpError,
            BackendError::Unreadable(_) => Fallback::InvalidAnswer,
        }
    }
}

/// The seam the envelope calls through: [`JevBackend`] for real, a fake in
/// tests. The key is handed over per call and never kept.
#[async_trait::async_trait]
pub trait DecisionBackend: Send + Sync {
    /// The provider a run records (`jev`).
    fn provider(&self) -> &'static str;
    async fn ask(
        &self,
        key: &Secret,
        model: &str,
        req: &JevRequest,
        timeout: Duration,
    ) -> Result<JevResponse, BackendError>;
}

/// Jev over HTTPS. No retries: the caller's path gets one attempt.
pub struct JevBackend {
    transport: Arc<dyn HttpTransport>,
}

impl JevBackend {
    pub fn new(transport: Arc<dyn HttpTransport>) -> Self {
        JevBackend { transport }
    }

    /// Straight from this process, to [`JEV_HOST`] only.
    pub fn direct() -> Self {
        JevBackend::new(Arc::new(DirectTransport::new(host_policy())))
    }
}

/// The transport's fence: exactly [`JEV_HOST`], nothing else (not a
/// subdomain, not a look-alike).
pub fn host_policy() -> crate::net::https::HostPolicy {
    Arc::new(|h: &str| h.eq_ignore_ascii_case(JEV_HOST))
}

#[async_trait::async_trait]
impl DecisionBackend for JevBackend {
    fn provider(&self) -> &'static str {
        PROVIDER_JEV
    }

    async fn ask(
        &self,
        key: &Secret,
        model: &str,
        req: &JevRequest,
        timeout: Duration,
    ) -> Result<JevResponse, BackendError> {
        let http = Request::post_json_bytes(JEV_URL, req.wire_body(model))
            .header("Authorization", format!("Bearer {}", key.expose()))
            .header(
                "User-Agent",
                format!("claude-fleet/{}", crate::app_version::get()),
            )
            .with_timeout(timeout);
        let resp = match self.transport.send(http).await {
            Ok(r) => r,
            Err(TransportError::Timeout) => return Err(BackendError::Timeout),
            Err(e) => return Err(BackendError::Transport(e.to_string())),
        };
        match resp.status {
            200..=299 => resp
                .parse_json::<JevResponse>()
                .map_err(BackendError::Unreadable),
            429 => Err(BackendError::RateLimited {
                retry_after: resp
                    .header("retry-after")
                    .and_then(|v| v.trim().parse().ok()),
            }),
            529 => Err(BackendError::Overloaded),
            status => Err(BackendError::Http { status }),
        }
    }
}
