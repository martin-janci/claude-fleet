//! The second opinion: every question a live use case asks Jev in `shadow`
//! or `assist` is also asked of Claude in print mode ([`super::haiku`]),
//! and the answer recorded beside Jev's, so the two can be compared on real
//! traffic rather than only on the benchmark's cases. Nothing acts on it.
//!
//! * **Off by default.** `decide.haiku.shadow` turns it on, and it asks
//!   nothing until `decide.haiku.host` names a host. A use case whose mode
//!   is `off` is never asked about, whatever this says.
//! * **Never on the caller's path.** [`plan`] decides under one short lock,
//!   before Jev is asked; the call itself runs in a task of its own after
//!   Jev's answer is recorded ([`super::decide`]), so Jev's latency and
//!   answer are what they were.
//! * **The same fences as the baseline.** Only the process that owns the
//!   fleet, only a Choice, never a use case that sends Claude's reply text
//!   ([`super::Feature::sends_reply_text`]), and only a question whose org is
//!   the host's ([`haiku::resolve_host_org`]). The prompt is the redacted
//!   request, on stdin ([`Haiku::ask`]).
//! * **Bounded.** `decide.haiku.daily_calls` calls a UTC day, one at a time
//!   per host (a question that comes while one runs is not asked), and the
//!   envelope's breaker over the haiku runs.
//! * **Recorded as its own provider.** One `decision_runs` row per call:
//!   provider `haiku`, mode `shadow`, the same question version, fingerprint,
//!   candidates and baseline as Jev's run. The proposal readers and the
//!   adapters' own lookups leave these rows out
//!   ([`crate::store::Store::current_proposals`]), so a second opinion never
//!   withdraws Jev's proposal, and the envelope's health does not count them.

use super::haiku::{self, Haiku, HaikuConfig, HaikuReply, PROVIDER_HAIKU};
use super::{
    breaker_state, day_start, fingerprint, int_setting, owns_the_fleet, CallSettings, Caller,
    DecideCtx, DecideRequest, Fallback, FeatureMode, Mode, Question,
};
use crate::ipc_error::lock;
use crate::service::settings;
use crate::ssh::SshExec;
use crate::store::{is_decision_word, NewDecisionRun, RunScope, Store};
use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

/// The transport the process installed (the reconcile tick's SSH client).
static EXEC: OnceLock<Arc<dyn SshExec>> = OnceLock::new();

/// The hosts a second opinion is running on.
static IN_FLIGHT: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Install the transport second opinions run over. The first call wins.
pub fn install(exec: Arc<dyn SshExec>) {
    let _ = EXEC.set(exec);
}

/// A host's slot while its call runs; freed when dropped.
struct Slot(String);

impl Slot {
    fn claim(host: &str) -> Option<Slot> {
        let mut m = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        m.insert(host.to_string()).then(|| Slot(host.to_string()))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut m = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        m.remove(&self.0);
    }
}

/// A second opinion cleared to run: [`Job::run`] asks and records it.
pub struct Job {
    ctx: DecideCtx,
    req: DecideRequest,
    exec: Arc<dyn SshExec>,
    cfg: HaikuConfig,
    host_org: Option<i64>,
    run: NewDecisionRun,
    slot: Slot,
}

/// Whether `req` gets a second opinion, and if so the job that asks it.
/// `None` for every question the fences above keep out (most, by default).
pub fn plan(ctx: &DecideCtx, req: &DecideRequest) -> Option<Job> {
    if req.caller() != Caller::Live
        || !matches!(req.request.question, Question::Choice { .. })
        || req.feature.sends_reply_text()
    {
        return None;
    }
    let now = ctx.now();
    let s = lock(&ctx.store).ok()?;
    if !settings::get_bool(&s, settings::DECIDE_HAIKU_SHADOW)
        || FeatureMode::of(&s, req.feature) == FeatureMode::Off
        || !owns_the_fleet(&s)
    {
        return None;
    }
    let (cfg, host_org) = configured(&s)?;
    if host_org != req.org_id {
        return None;
    }
    let cap = int_setting(&s, settings::DECIDE_HAIKU_DAILY_CALLS);
    let used = s
        .decision_calls_since(PROVIDER_HAIKU, day_start(now), RunScope::Live)
        .ok()?;
    if cap <= 0 || used >= cap {
        return None;
    }
    let call = CallSettings::read(&s);
    let breaker = breaker_state(
        &s,
        PROVIDER_HAIKU,
        call.breaker_failures,
        call.breaker_open_secs,
        now,
        RunScope::Live,
    )
    .ok()?;
    if breaker.open {
        return None;
    }
    let input_fp = s
        .decision_fp_key()
        .ok()
        .map(|k| fingerprint(&k, &req.request.redacted()));
    drop(s);

    if req.request.question.check().is_err() {
        return None;
    }
    let run = NewDecisionRun {
        at: now,
        feature: req.feature.as_str().into(),
        org_id: req.org_id,
        subject_kind: req.subject_kind.clone(),
        subject_id: req.subject_id.clone(),
        mode: Mode::Shadow.as_str().into(),
        provider: PROVIDER_HAIKU.into(),
        model_version: Some(cfg.model.clone()),
        question_version: req.question_version.clone(),
        input_fp,
        candidates: req.request.question.candidates(),
        answer: None,
        probabilities: None,
        confidence: None,
        fallback: None,
        baseline_answer: req.baseline.clone().filter(|b| is_decision_word(b)),
        called: false,
        latency_ms: None,
        input_tokens: 0,
        cost_microusd: 0,
    };
    if run.validate().is_err() {
        return None;
    }
    let exec = ctx.haiku_exec.clone().or_else(|| EXEC.get().cloned())?;
    let slot = Slot::claim(&cfg.host)?;
    Some(Job {
        ctx: ctx.clone(),
        req: req.clone(),
        exec,
        cfg,
        host_org,
        run,
        slot,
    })
}

/// The configured host and model, checked, and the host's org. `None` when
/// no host is set or it is not one this database knows.
fn configured(s: &Store) -> Option<(HaikuConfig, Option<i64>)> {
    let host = settings::get_string(s, settings::DECIDE_HAIKU_HOST);
    let host = host.trim();
    if host.is_empty() {
        return None;
    }
    let model = settings::get_string(s, settings::DECIDE_HAIKU_MODEL);
    let cfg = match HaikuConfig::new(host, Some(model.trim()), None) {
        Ok(c) => c,
        Err(why) => {
            tracing::warn!("[decide] second opinion not asked: {why}");
            return None;
        }
    };
    match haiku::resolve_host_org(s, &cfg.host) {
        Ok(org) => Some((cfg, org)),
        Err(why) => {
            tracing::warn!("[decide] second opinion not asked: {why}");
            None
        }
    }
}

impl Job {
    /// Ask once and record the run, whatever the answer. Returns the run's
    /// id (`None` when the store refused it).
    pub async fn run(self) -> Option<i64> {
        let Job {
            ctx,
            req,
            exec,
            cfg,
            host_org,
            mut run,
            slot,
        } = self;
        let asker = Haiku {
            exec: exec.as_ref(),
            cfg,
            host_org,
        };
        let reply = asker.ask(&req.request).await;
        drop(slot);
        fill(&mut run, &reply, req.min_confidence);
        match lock(&ctx.store).and_then(|s| s.insert_decision_run(&run)) {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::warn!(
                    feature = %run.feature,
                    "[decide] the second opinion was not recorded: {}",
                    e.message
                );
                None
            }
        }
    }
}

/// PURE: the run's call, answer and fallback from `reply`.
pub(super) fn fill(run: &mut NewDecisionRun, reply: &HaikuReply, min_confidence: Option<f64>) {
    run.called = reply.ran;
    run.latency_ms = reply.latency_ms;
    run.input_tokens = reply.input_tokens.unwrap_or(0).max(0);
    run.cost_microusd = reply.cost_microusd.unwrap_or(0).max(0);
    let fallback = match (reply.error, &reply.choice) {
        (Some(haiku::reason::TIMEOUT), _) => Some(Fallback::Timeout),
        (Some(_), _) => Some(Fallback::HttpError),
        (None, None) => Some(Fallback::InvalidAnswer),
        (None, Some(choice)) => {
            run.answer = Some(choice.clone());
            run.confidence = reply.confidence;
            let low = match (min_confidence, reply.confidence) {
                (Some(floor), Some(c)) => c < floor,
                (Some(_), None) => true,
                (None, _) => false,
            };
            low.then_some(Fallback::LowConfidence)
        }
    };
    run.fallback = fallback.map(|f| f.as_str().to_string());
}
