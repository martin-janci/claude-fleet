//! The `claude -p haiku` baseline (decision D33): the same closed-set
//! question the envelope would send Jev, asked of a Claude model in print
//! mode on a host the operator names, so the benchmarks' acceptance lines
//! "beats / ties claude -p haiku" (card J3) and "not worse than haiku by more
//! than 3 points" (card J1) can be judged. D33 also names it as the later
//! generative fallback for asynchronous decisions only; nothing live calls
//! it yet — only `fleet-hub decide bench … --provider haiku`.
//!
//! * **The same question.** [`prompt_for`] renders the request's
//!   **redacted** form ([`JevRequest::redacted`], exactly what the envelope
//!   fingerprints and sends) as plain text: the state as JSON, the
//!   instruction, and the option ids with their descriptions, asking for
//!   exactly one line of JSON `{"choice": "<option id>", "confidence": <0..1>}`.
//!   Only a Choice is rendered.
//! * **A run that cannot act.** `claude -p --model <m> --output-format json
//!   --settings '{"disableAllHooks":true}' --tools '' --strict-mcp-config
//!   --no-session-persistence '<prompt>'` (the flags of
//!   [`crate::service::work::summary`]'s fork, without a conversation): no
//!   tool, no MCP server, none of fleet's hooks, no transcript left on the
//!   host. It runs in a fresh temporary directory (no project `CLAUDE.md`),
//!   stdin closed, output capped. [`haiku_script`] builds exactly this; the
//!   prompt is one word quoted with [`crate::shell::quote`], and a test pins
//!   both.
//! * **Where the data goes.** The prompt leaves the hub for the named host
//!   over SSH and, from there, reaches Anthropic through that host's Claude
//!   account — the processor the host's sessions already use, which is why
//!   D33 chose it. Nothing runs without an explicit host; the benchmark
//!   prints which one.
//! * **Bounded.** One call at a time per host (a second waits), each under
//!   [`HaikuConfig::timeout`] (the host-side `timeout` stops the model a
//!   little earlier), and the caller counts calls against `--max-calls`.
//! * **Read, never trusted.** The reply is parsed ([`parse_envelope`],
//!   [`parse_answer`]): a choice outside the offered options, or no JSON,
//!   is `invalid` (the benchmarks count it as an abstention); a confidence
//!   is clamped to 0..=1, and a missing one leaves that answer out of
//!   calibration. Tokens and cost come from the `--output-format json`
//!   envelope when it has them, else they are unknown.
//!
//! Nothing is recorded in `decision_runs`: that record is the envelope's,
//! for Jev only.

use super::{canonical_json, JevRequest, Question};
use crate::ipc_error::codes;
use crate::service::settings;
use crate::ssh::SshExec;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// The provider's name in the benchmarks' reports.
pub const PROVIDER_HAIKU: &str = "haiku";
/// The models `--haiku-model` takes (the same list as `work.summary_model`).
pub const MODELS: &[&str] = settings::SUMMARY_MODELS;
/// The default model (D33).
pub const DEFAULT_MODEL: &str = "haiku";
/// One call's wall clock, unless told otherwise, and its bounds.
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
pub const MIN_TIMEOUT_SECS: u64 = 10;
pub const MAX_TIMEOUT_SECS: u64 = 600;
/// The tag the script prints before the model's output.
pub const HAIKU_TAG: &str = "fleet-haiku=";
/// The connect budget for the one command.
const CONNECT: Duration = Duration::from_secs(10);
/// Bytes of stdout the script lets through.
pub const OUTPUT_CAP_BYTES: usize = 65_536;
/// The longest command (the script, quoted for `bash -lc`) sent: under the
/// kernel's 128 KiB limit on one argument, which the remote shell's `-c`
/// string is.
pub const MAX_COMMAND_BYTES: usize = 120_000;

/// Why a call gave no answer (a word, as the benchmarks' `skipped` shows).
pub mod reason {
    /// The call ran past its wall clock (or the host-side `timeout`).
    pub const TIMEOUT: &str = "timeout";
    /// SSH failed (unreachable host, spawn failure).
    pub const SSH_ERROR: &str = "ssh_error";
    /// `claude` is not on the host's login PATH.
    pub const NO_CLAUDE: &str = "noclaude";
    /// `claude` ran and failed, or said nothing readable.
    pub const CALL_FAILED: &str = "call_failed";
    /// The prompt does not fit one command.
    pub const TOO_LONG: &str = "too_long";
    /// The request is not a Choice.
    pub const UNSUPPORTED: &str = "unsupported";
    /// The caller's `--max-calls` was reached.
    pub const MAX_CALLS: &str = "max_calls";
}

/// Where and how the baseline runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HaikuConfig {
    /// The host alias the call runs on (its Claude account answers).
    pub host: String,
    /// One of [`MODELS`].
    pub model: String,
    /// One call's wall clock.
    pub timeout: Duration,
}

impl HaikuConfig {
    /// The operator's flags, checked: a valid host alias, a model of
    /// [`MODELS`] (default [`DEFAULT_MODEL`]), a timeout of
    /// [`MIN_TIMEOUT_SECS`]–[`MAX_TIMEOUT_SECS`] seconds (default
    /// [`DEFAULT_TIMEOUT_SECS`]).
    pub fn new(
        host: &str,
        model: Option<&str>,
        timeout_secs: Option<u64>,
    ) -> Result<HaikuConfig, String> {
        crate::validate::host_alias(host).map_err(|e| e.message)?;
        let model = model.unwrap_or(DEFAULT_MODEL);
        if !MODELS.contains(&model) {
            return Err(format!(
                "the model is one of {}, not {model:?}",
                MODELS.join(", ")
            ));
        }
        let secs = timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        if !(MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&secs) {
            return Err(format!(
                "the timeout is {MIN_TIMEOUT_SECS}-{MAX_TIMEOUT_SECS} seconds, not {secs}"
            ));
        }
        Ok(HaikuConfig {
            host: host.to_string(),
            model: model.to_string(),
            timeout: Duration::from_secs(secs),
        })
    }

    /// Seconds the host-side `timeout` gives `claude`: a little under the
    /// wall clock, so its answer (or its 124) still arrives.
    pub fn host_timeout_secs(&self) -> u64 {
        self.timeout.as_secs().saturating_sub(10).max(5)
    }

    /// The consent note the benchmark prints before it asks anything.
    pub fn consent_note(&self) -> String {
        format!(
            "haiku: each case's redacted state and its options leave the hub over SSH for host \
             {host} and go to Anthropic through {host}'s Claude account (claude -p --model {m}; \
             no tools, no MCP, no hooks, no transcript kept); one call at a time, {t}s each; \
             nothing is recorded in decision_runs",
            host = self.host,
            m = self.model,
            t = self.timeout.as_secs()
        )
    }
}

// --- the prompt ----------------------------------------------------------------

/// The fixed text before the state.
pub const PREAMBLE: &str = "You answer one closed-set classification question. Use only the \
    state below; do not use tools and do not ask anything back.";

/// The fixed text after the options.
pub const REPLY_FORMAT: &str = "Reply with exactly one line of JSON and nothing else:\n\
    {\"choice\": \"<one option id from the list>\", \"confidence\": <from 0 to 1, how likely \
    the choice is right>}";

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => canonical_json(other),
    }
}

/// PURE: the prompt for `req` and its option ids (in the order shown). The
/// request is redacted first ([`JevRequest::redacted`]): the text holds the
/// same state and options the envelope would send, and nothing else. Only a
/// Choice is rendered.
pub fn prompt_for(req: &JevRequest) -> Result<(String, Vec<String>), String> {
    let r = req.redacted();
    let Question::Choice {
        instructions,
        criteria,
    } = &r.question
    else {
        return Err(format!(
            "the haiku baseline asks a choice, not a {}",
            r.question.kind()
        ));
    };
    r.question.check()?;
    let mut text = String::new();
    text.push_str(PREAMBLE);
    text.push_str("\n\n<state>\n");
    text.push_str(&canonical_json(&r.state));
    text.push_str("\n</state>\n\nQuestion: ");
    text.push_str(&text_of(instructions));
    text.push_str("\n\nOptions (answer with the id before the colon):\n");
    let mut ids = Vec::with_capacity(criteria.len());
    for (id, desc) in criteria {
        text.push_str("- ");
        text.push_str(id);
        let d = desc.as_ref().map(text_of).unwrap_or_default();
        if !d.is_empty() {
            text.push_str(": ");
            text.push_str(&d);
        }
        text.push('\n');
        ids.push(id.clone());
    }
    text.push('\n');
    text.push_str(REPLY_FORMAT);
    Ok((text, ids))
}

// --- the command ---------------------------------------------------------------

/// PURE: the one command a call runs on the host. The model must be one of
/// [`MODELS`]; the prompt is one word, quoted with [`crate::shell::quote`].
pub fn haiku_script(prompt: &str, model: &str, host_timeout_secs: u64) -> Result<String, String> {
    use crate::shell::quote;
    if !MODELS.contains(&model) {
        return Err(format!("refusing model {model:?}"));
    }
    let claude = [
        "claude",
        "-p",
        "--model",
        &quote(model),
        "--output-format",
        "json",
        // `disableAllHooks`, not `{"hooks":{}}` (see summary.rs): fleet's
        // user-level hooks must not see this run.
        "--settings",
        &quote(r#"{"disableAllHooks":true}"#),
        "--tools",
        &quote(""),
        "--strict-mcp-config",
        "--no-session-persistence",
        &quote(prompt),
    ]
    .join(" ");
    let t = HAIKU_TAG;
    Ok(format!(
        "set -o pipefail; \
         if ! command -v claude >/dev/null 2>&1; then echo {t}noclaude; exit 0; fi; \
         d=$(mktemp -d 2>/dev/null) || d=''; \
         if [ -n \"$d\" ]; then cd -- \"$d\" || exit 1; else cd / || exit 1; fi; \
         t=''; if command -v timeout >/dev/null 2>&1; then t='timeout {host_timeout_secs}'; fi; \
         echo {t}run; \
         $t {claude} </dev/null | head -c {OUTPUT_CAP_BYTES}; s=$?; \
         if [ -n \"$d\" ]; then cd / && rmdir -- \"$d\" 2>/dev/null; fi; \
         exit $s"
    ))
}

/// What the script's output says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptAnswer {
    /// `claude` is not on the host's login PATH.
    NoClaude,
    /// The model ran; the text after the tag.
    Ran(String),
    /// No tag: the shell failed before it said anything.
    Nothing,
}

/// PURE: read the script's stdout. The FIRST whole tag line decides: the
/// script prints it before `claude` starts, so nothing the model says can
/// stand in for it.
pub fn parse_script_output(stdout: &str) -> ScriptAnswer {
    let lines: Vec<&str> = stdout.lines().collect();
    let Some(i) = lines.iter().position(|l| {
        let l = l.trim();
        l == format!("{HAIKU_TAG}run") || l == format!("{HAIKU_TAG}noclaude")
    }) else {
        return ScriptAnswer::Nothing;
    };
    match lines[i].trim().trim_start_matches(HAIKU_TAG) {
        "noclaude" => ScriptAnswer::NoClaude,
        _ => ScriptAnswer::Ran(lines[i + 1..].join("\n").trim().to_string()),
    }
}

// --- the reply -----------------------------------------------------------------

/// What `--output-format json` wraps the model's text in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Envelope {
    /// The model's text.
    pub result: String,
    pub is_error: bool,
    /// Input tokens (plain + cache creation + cache read), when reported.
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    /// `total_cost_usd` in micro-dollars, when reported.
    pub cost_microusd: Option<i64>,
}

fn int(v: Option<&Value>) -> Option<i64> {
    v.and_then(Value::as_i64)
}

fn envelope_of(v: &Value) -> Option<Envelope> {
    let o = v.as_object()?;
    if o.get("type").and_then(Value::as_str) != Some("result") && !o.contains_key("result") {
        return None;
    }
    let usage = o.get("usage");
    let input = usage.and_then(|u| {
        let parts = [
            int(u.get("input_tokens")),
            int(u.get("cache_creation_input_tokens")),
            int(u.get("cache_read_input_tokens")),
        ];
        parts
            .iter()
            .any(Option::is_some)
            .then(|| parts.iter().flatten().sum())
    });
    Some(Envelope {
        result: o
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        is_error: o.get("is_error").and_then(Value::as_bool).unwrap_or(false)
            || o.get("subtype")
                .and_then(Value::as_str)
                .is_some_and(|s| s != "success"),
        input_tokens: input,
        output_tokens: usage.and_then(|u| int(u.get("output_tokens"))),
        cost_microusd: o
            .get("total_cost_usd")
            .and_then(Value::as_f64)
            .filter(|c| c.is_finite() && *c >= 0.0)
            .map(|c| (c * 1_000_000.0).round() as i64),
    })
}

/// PURE: the `--output-format json` envelope in `out`: one JSON object (or
/// an array of messages whose last `result` message is it), possibly after
/// other lines. `None` when there is none.
pub fn parse_envelope(out: &str) -> Option<Envelope> {
    let t = out.trim();
    let whole = serde_json::from_str::<Value>(t).ok();
    let candidates = whole.into_iter().chain(
        t.lines()
            .rev()
            .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok()),
    );
    for v in candidates {
        match &v {
            Value::Array(a) => {
                if let Some(e) = a.iter().rev().find_map(envelope_of) {
                    return Some(e);
                }
            }
            _ => {
                if let Some(e) = envelope_of(&v) {
                    return Some(e);
                }
            }
        }
    }
    None
}

/// The model's answer, checked against the offered options.
#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    /// One of the options, with its confidence (clamped to 0..=1) when it
    /// gave a usable one.
    Valid {
        choice: String,
        confidence: Option<f64>,
    },
    /// No JSON object with a `choice`, or a choice outside the options.
    Invalid,
}

fn confidence_of(v: Option<&Value>) -> Option<f64> {
    let x = match v? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    x.is_finite().then(|| x.clamp(0.0, 1.0))
}

/// PURE: the answer in the model's `text`: the last JSON object in it that
/// has a `choice` (a code fence or words around it are ignored). The choice
/// must be one of `options` (exactly, else ignoring ASCII case when that is
/// unambiguous).
pub fn parse_answer(text: &str, options: &[String]) -> Parsed {
    let mut found: Option<serde_json::Map<String, Value>> = None;
    for (i, _) in text.match_indices('{') {
        let mut it = serde_json::Deserializer::from_str(&text[i..]).into_iter::<Value>();
        if let Some(Ok(Value::Object(m))) = it.next() {
            if m.contains_key("choice") {
                found = Some(m);
            }
        }
    }
    let Some(m) = found else {
        return Parsed::Invalid;
    };
    let Some(raw) = m.get("choice").and_then(Value::as_str).map(str::trim) else {
        return Parsed::Invalid;
    };
    let choice = options
        .iter()
        .find(|o| o.as_str() == raw)
        .cloned()
        .or_else(|| {
            let mut ci = options.iter().filter(|o| o.eq_ignore_ascii_case(raw));
            match (ci.next(), ci.next()) {
                (Some(o), None) => Some(o.clone()),
                _ => None,
            }
        });
    match choice {
        Some(choice) => Parsed::Valid {
            choice,
            confidence: confidence_of(m.get("confidence")),
        },
        None => Parsed::Invalid,
    }
}

// --- one call ------------------------------------------------------------------

/// What one call gave.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HaikuReply {
    /// The command was sent to the host (a call to count).
    pub ran: bool,
    /// Why there is no answer ([`reason`]); `None` with a valid or invalid
    /// answer.
    pub error: Option<&'static str>,
    /// The option chosen, when valid.
    pub choice: Option<String>,
    pub confidence: Option<f64>,
    /// It answered, but not with one of the options.
    pub invalid: bool,
    /// The whole call, SSH included.
    pub latency_ms: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cost_microusd: Option<i64>,
}

impl HaikuReply {
    fn failed(error: &'static str) -> HaikuReply {
        HaikuReply {
            error: Some(error),
            ..Default::default()
        }
    }

    /// Skipped before anything was sent: the caller's cap was reached.
    pub fn max_calls() -> HaikuReply {
        HaikuReply::failed(reason::MAX_CALLS)
    }
}

/// The hosts' call slots: one call at a time per host; a second waits.
static SLOTS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn slot(host: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut m = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(m.entry(host.to_string()).or_default())
}

/// The baseline, bound to a transport.
pub struct Haiku<'a> {
    pub exec: &'a dyn SshExec,
    pub cfg: HaikuConfig,
}

impl Haiku<'_> {
    /// Ask `req` once on the configured host (waiting for the host's slot).
    pub async fn ask(&self, req: &JevRequest) -> HaikuReply {
        let (prompt, options) = match prompt_for(req) {
            Ok(p) => p,
            Err(_) => return HaikuReply::failed(reason::UNSUPPORTED),
        };
        let script = match haiku_script(&prompt, &self.cfg.model, self.cfg.host_timeout_secs()) {
            Ok(s) => s,
            Err(_) => return HaikuReply::failed(reason::UNSUPPORTED),
        };
        if crate::shell::quote(&script).len() > MAX_COMMAND_BYTES {
            return HaikuReply::failed(reason::TOO_LONG);
        }
        let slot = slot(&self.cfg.host);
        let _held = slot.lock().await;
        let started = Instant::now();
        let res = crate::ssh::run_shell_bounded(
            self.exec,
            &self.cfg.host,
            &script,
            CONNECT,
            self.cfg.timeout,
        )
        .await;
        let latency = Some(started.elapsed().as_millis() as i64);
        let mut reply = HaikuReply {
            ran: true,
            latency_ms: latency,
            ..Default::default()
        };
        let out = match res {
            Ok(o) => o,
            Err(e) => {
                reply.error = Some(
                    if e.code == codes::E_SSH_TIMEOUT || e.code == codes::E_TIMEOUT {
                        reason::TIMEOUT
                    } else {
                        reason::SSH_ERROR
                    },
                );
                return reply;
            }
        };
        if out.status.code() == Some(255) && out.stdout.is_empty() {
            reply.error = Some(reason::SSH_ERROR);
            return reply;
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let text = match parse_script_output(&stdout) {
            ScriptAnswer::NoClaude => {
                reply.error = Some(reason::NO_CLAUDE);
                return reply;
            }
            ScriptAnswer::Nothing => {
                reply.error = Some(reason::CALL_FAILED);
                return reply;
            }
            ScriptAnswer::Ran(t) => t,
        };
        if out.status.code() == Some(124) {
            reply.error = Some(reason::TIMEOUT);
            return reply;
        }
        let model_text = match parse_envelope(&text) {
            Some(env) => {
                reply.input_tokens = env.input_tokens;
                reply.output_tokens = env.output_tokens;
                reply.cost_microusd = env.cost_microusd;
                if env.is_error {
                    reply.error = Some(reason::CALL_FAILED);
                    return reply;
                }
                env.result
            }
            // No envelope: read the text itself, usage unknown.
            None if !out.status.success() || text.is_empty() => {
                reply.error = Some(reason::CALL_FAILED);
                return reply;
            }
            None => text,
        };
        match parse_answer(&model_text, &options) {
            Parsed::Valid { choice, confidence } => {
                reply.choice = Some(choice);
                reply.confidence = confidence;
            }
            Parsed::Invalid => reply.invalid = true,
        }
        reply
    }
}

/// Used by the benchmarks' tests (behind `nl-detect`, like `bench`).
#[cfg(all(test, feature = "nl-detect"))]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
