//! Chat forms (`docs/superpowers/specs/2026-10-07-chat-forms-design.md`):
//! an agent opens a `fleet.form/1` form in its session's chat and waits; a
//! person answers or declines. Access is the caller's business (the `ask`
//! tool, the desktop commands); this module takes ids already gated.

use crate::ipc_error::{codes, lock, IpcError};
use crate::pages::forms::{self, FieldProblem};
use crate::service::tasks::AccessRecheck;
use crate::ssh::SshExec;
use crate::store::{FormFinish, FormRow, NewForm, Store};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const DEFAULT_WAIT_SECS: u64 = 600;
pub const MAX_WAIT_SECS: u64 = 600;
/// A pending form nobody answered within this long expires.
pub const EXPIRE_SECS: i64 = 24 * 3600;
/// A decided form's row is kept this long.
pub const KEEP_SECS: i64 = 7 * 24 * 3600;
/// Where a form's secrets go on its session's host: `<dir>/<form_id>/<field>`.
pub const SECRET_DIR: &str = "~/.cache/claude-fleet/forms";
pub const SECRET_NOTE: &str = "Delete each secret file once you have used it.";
/// A waiter re-reads at least this often, so a missed wake costs latency only.
const POLL_FLOOR: Duration = Duration::from_millis(500);
const SWEEP_TIMEOUT: Duration = Duration::from_secs(30);
/// One tick removes at most this many secret directories, so a slow fleet
/// cannot stretch it.
const MAX_SWEEPS_PER_PASS: usize = 20;

pub fn wait_timeout(timeout_s: Option<u64>) -> Duration {
    Duration::from_secs(timeout_s.unwrap_or(DEFAULT_WAIT_SECS).min(MAX_WAIT_SECS))
}

/// What `ask` answers the agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormResult {
    pub status: String,
    pub form_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One form as a person's screen reads it (`list` / `get` / `answer` /
/// `decline`, and the desktop's commands).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormView {
    pub form_id: String,
    pub session_id: i64,
    pub host_alias: String,
    pub title: String,
    pub spec: Value,
    #[serde(default)]
    pub why: Option<String>,
    pub state: String,
    #[serde(default)]
    pub answers: Option<Value>,
    #[serde(default)]
    pub secrets: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub answered_by: Option<String>,
    pub created_at: i64,
    #[serde(default)]
    pub decided_at: Option<i64>,
    /// What Jev proposes for the form's first choice while it waits (J5,
    /// `decide::quick_answer`, assist mode): the card moves that option
    /// first and says so; a person still answers. Left off the wire when
    /// there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<crate::service::decide::quick_answer::FormProposal>,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    answers: Map<String, Value>,
    secrets: BTreeMap<String, String>,
}

fn stored(row: &FormRow) -> Option<Stored> {
    row.answers
        .as_deref()
        .and_then(|j| serde_json::from_str(j).ok())
}

pub fn view(row: &FormRow) -> FormView {
    let spec: Value = serde_json::from_str(&row.spec).unwrap_or(Value::Null);
    let st = stored(row);
    FormView {
        form_id: row.form_id.clone(),
        session_id: row.session_id,
        host_alias: row.host_alias.clone(),
        title: spec["title"].as_str().unwrap_or_default().to_string(),
        spec,
        why: row.why.clone(),
        state: row.state.clone(),
        answers: st.as_ref().map(|s| Value::Object(s.answers.clone())),
        secrets: st.map(|s| s.secrets),
        note: row.note.clone(),
        answered_by: row.answered_by.clone(),
        created_at: row.created_at,
        decided_at: row.decided_at,
        proposal: None,
    }
}

/// [`view`], with the live proposal for a pending form's first choice
/// (`get`: what a person reads before answering).
pub fn view_with_proposal(s: &Store, row: &FormRow) -> FormView {
    let mut v = view(row);
    if v.state == "pending" {
        let org = s.session_org(row.session_id).ok().flatten();
        v.proposal = crate::service::decide::quick_answer::form_choice(
            &v.form_id,
            org,
            &v.title,
            v.why.as_deref(),
            &v.spec,
        )
        .and_then(|fc| crate::service::decide::quick_answer::form_proposal(s, &fc));
    }
    v
}

pub fn result_of(row: &FormRow) -> FormResult {
    let mut r = FormResult {
        status: row.state.clone(),
        form_id: row.form_id.clone(),
        answers: None,
        secrets: None,
        answered_by: None,
        note: None,
    };
    match row.state.as_str() {
        "answered" => {
            let st = stored(row).unwrap_or_default();
            if !st.secrets.is_empty() {
                r.note = Some(SECRET_NOTE.into());
                r.secrets = Some(st.secrets);
            }
            r.answers = Some(Value::Object(st.answers));
            r.answered_by = row.answered_by.clone();
        }
        "declined" => {
            r.note = row.note.clone();
            r.answered_by = row.answered_by.clone();
        }
        _ => {}
    }
    r
}

fn new_form_id() -> String {
    use rand::RngExt;
    const ABC: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::rng();
    let tail: String = (0..16)
        .map(|_| ABC[rng.random_range(0..ABC.len())] as char)
        .collect();
    format!("f_{tail}")
}

fn not_found(form_id: &str) -> IpcError {
    IpcError::new(codes::E_NOTFOUND, format!("form {form_id} not found"))
}

fn not_pending(row: &FormRow) -> IpcError {
    IpcError::new(
        codes::E_CONFLICT,
        format!("form {} is {}, not pending", row.form_id, row.state),
    )
    .with_details(serde_json::json!({ "form_id": row.form_id, "state": row.state }))
}

fn bounded_text(what: &str, t: Option<&str>) -> Result<(), IpcError> {
    match t {
        Some(t) if t.chars().count() > forms::MAX_TEXT => Err(IpcError::new(
            codes::E_INVALID,
            format!("{what} is longer than {} characters", forms::MAX_TEXT),
        )),
        _ => Ok(()),
    }
}

pub fn row(store: &Mutex<Store>, form_id: &str) -> Result<FormRow, IpcError> {
    lock(store)?
        .form(form_id)?
        .ok_or_else(|| not_found(form_id))
}

pub fn get(store: &Mutex<Store>, form_id: &str) -> Result<FormView, IpcError> {
    let s = lock(store)?;
    let r = s.form(form_id)?.ok_or_else(|| not_found(form_id))?;
    Ok(view_with_proposal(&s, &r))
}

/// Validate `spec` and open it for `session_id` (a session the caller
/// proved it is).
pub fn open(
    store: &Mutex<Store>,
    session_id: i64,
    spec: &Value,
    why: Option<&str>,
) -> Result<FormView, IpcError> {
    bounded_text("why", why)?;
    let form = forms::parse(spec).map_err(|problems| {
        IpcError::new(codes::E_INVALID, problems.join("; "))
            .with_details(serde_json::json!({ "problems": problems }))
    })?;
    let s = lock(store)?;
    let session = s.get_session_by_id(session_id)?.ok_or_else(|| {
        IpcError::new(codes::E_NOTFOUND, format!("session {session_id} not found"))
    })?;
    if let Some(open) = s.pending_form_of_session(session_id)? {
        return Err(IpcError::new(
            codes::E_CONFLICT,
            format!(
                "this session already waits on form {}; wait on it or cancel it",
                open.form_id
            ),
        )
        .with_details(serde_json::json!({ "form_id": open.form_id })));
    }
    let text = serde_json::to_string(&form).expect("a form serialises");
    let id = new_form_id();
    let row = s.insert_form(&NewForm {
        form_id: &id,
        session_id,
        host_alias: &session.host_alias,
        spec: &text,
        why,
    })?;
    Ok(view(&row))
}

/// How long a draft lasts after its last write before the tick drops it.
pub const DRAFT_TTL_SECS: i64 = 10 * 60;

/// What `ask { draft }` answers.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DraftAck {
    /// `drafting`, or `cleared` for an empty draft.
    pub status: &'static str,
    pub bytes: usize,
}

/// Show `text`, the fleet.form/1 JSON `session_id`'s agent has written so
/// far, in its chat (redesign 10.12): the card draws the title and each
/// whole field in while the agent goes on. Not validated (it is not whole
/// yet), only bounded; `ask { form }` validates and replaces it. An empty
/// `text` drops the draft.
pub fn draft(
    store: &Mutex<Store>,
    session_id: i64,
    text: &str,
    why: Option<&str>,
) -> Result<DraftAck, IpcError> {
    bounded_text("why", why)?;
    if text.len() > forms::MAX_SPEC_BYTES {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "the draft is {} bytes, over the {} a form may have",
                text.len(),
                forms::MAX_SPEC_BYTES
            ),
        ));
    }
    let s = lock(store)?;
    if s.get_session_by_id(session_id)?.is_none() {
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("session {session_id} not found"),
        ));
    }
    if text.trim().is_empty() {
        s.clear_form_draft(session_id)?;
        return Ok(DraftAck {
            status: "cleared",
            bytes: 0,
        });
    }
    if let Some(open) = s.pending_form_of_session(session_id)? {
        return Err(IpcError::new(
            codes::E_CONFLICT,
            format!(
                "this session already waits on form {}; a draft is for the next one",
                open.form_id
            ),
        )
        .with_details(serde_json::json!({ "form_id": open.form_id })));
    }
    s.set_form_draft(session_id, text, why)?;
    Ok(DraftAck {
        status: "drafting",
        bytes: text.len(),
    })
}

/// Wait up to `timeout` for `form_id` to finish. A pending form at the
/// deadline answers `pending`, not an error.
pub async fn wait(
    store: &Mutex<Store>,
    form_id: &str,
    timeout: Duration,
    recheck: &dyn AccessRecheck,
) -> Result<FormResult, IpcError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let notify = {
        let s = lock(store)?;
        s.form(form_id)?.ok_or_else(|| not_found(form_id))?;
        s.form_notify()
    };
    loop {
        // Registered before the read, so a change between the read and the
        // sleep is not lost.
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        {
            let s = lock(store)?;
            recheck.check(&s)?;
            let row = s.form(form_id)?.ok_or_else(|| not_found(form_id))?;
            if row.state != "pending" {
                return Ok(result_of(&row));
            }
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(FormResult {
                status: "pending".into(),
                form_id: form_id.into(),
                answers: None,
                secrets: None,
                answered_by: None,
                note: None,
            });
        }
        let _ = tokio::time::timeout(POLL_FLOOR.min(deadline - now), notified).await;
    }
}

pub fn cancel(store: &Mutex<Store>, form_id: &str) -> Result<FormResult, IpcError> {
    let s = lock(store)?;
    let row = s.form(form_id)?.ok_or_else(|| not_found(form_id))?;
    let done = FormFinish {
        state: "cancelled",
        answers: None,
        note: None,
        answered_by: None,
        secrets_on_host: false,
    };
    if !s.finish_form(form_id, &done)? {
        return Err(not_pending(&row));
    }
    Ok(result_of(
        &s.form(form_id)?.ok_or_else(|| not_found(form_id))?,
    ))
}

pub fn decline(
    store: &Mutex<Store>,
    form_id: &str,
    note: Option<&str>,
    by: &str,
) -> Result<FormView, IpcError> {
    bounded_text("note", note)?;
    let s = lock(store)?;
    let row = s.form(form_id)?.ok_or_else(|| not_found(form_id))?;
    let done = FormFinish {
        state: "declined",
        answers: None,
        note,
        answered_by: Some(by),
        secrets_on_host: false,
    };
    if !s.finish_form(form_id, &done)? {
        return Err(not_pending(&row));
    }
    Ok(view(&s.form(form_id)?.ok_or_else(|| not_found(form_id))?))
}

/// The form's secret directory on its host, `~/` form (for
/// `write_host_file_secret`) and absolute (for the agent).
async fn secret_dir(
    ssh: &dyn SshExec,
    host: &str,
    form_id: &str,
) -> Result<(String, String), IpcError> {
    let tilde = format!("{SECRET_DIR}/{form_id}");
    let home = if host == crate::service::projects::LOCAL_HOST {
        local_home(std::env::var("HOME").ok())?
    } else {
        ssh.remote_home(host).await?
    };
    let abs = format!("{home}/{}", tilde.trim_start_matches("~/"));
    Ok((tilde, abs))
}

fn local_home(home: Option<String>) -> Result<String, IpcError> {
    home.filter(|h| !h.is_empty())
        .ok_or_else(|| IpcError::new(codes::E_HOST_WRITE, "HOME is not set"))
}

async fn remove_secret_dir(ssh: &dyn SshExec, host: &str, form_id: &str) -> Result<(), IpcError> {
    let script = format!(
        "rm -rf -- \"$HOME\"/.cache/claude-fleet/forms/{}",
        crate::shell::quote(form_id)
    );
    let out = crate::ssh::run_shell(ssh, host, &script, SWEEP_TIMEOUT).await?;
    if out.status.success() {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_HOST_WRITE,
            format!("{host}: {}", String::from_utf8_lossy(&out.stderr).trim()),
        ))
    }
}

/// Forms an `answer` is working on right now, in this process.
static ANSWERING: std::sync::LazyLock<Mutex<BTreeSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(BTreeSet::new()));

/// Holds one form id in [`ANSWERING`] until dropped. The std mutex is only
/// taken for the insert and the remove, never across an `.await`.
struct Answering(String);

impl Answering {
    fn claim(form_id: &str) -> Result<Self, IpcError> {
        let mut set = ANSWERING.lock().unwrap_or_else(|e| e.into_inner());
        if !set.insert(form_id.to_string()) {
            return Err(IpcError::new(
                codes::E_CONFLICT,
                format!("form {form_id} is being answered right now"),
            )
            .with_details(serde_json::json!({ "form_id": form_id, "state": "pending" })));
        }
        Ok(Self(form_id.to_string()))
    }
}

impl Drop for Answering {
    fn drop(&mut self) {
        ANSWERING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

fn host_write_error(field: &str, why: &str) -> IpcError {
    IpcError::new(codes::E_HOST_WRITE, format!("{field}: {why}"))
        .with_details(serde_json::json!({ "field": field }))
}

/// Remove a form's secret directory; clear its durable marker only when the
/// removal worked, so a failed removal is retried by the sweep.
async fn forget_secrets(store: &Mutex<Store>, ssh: &dyn SshExec, host: &str, form_id: &str) {
    if remove_secret_dir(ssh, host, form_id).await.is_ok() {
        if let Ok(s) = lock(store) {
            let _ = s.mark_form_swept(form_id);
        }
    }
}

/// Answer `form_id` as `by`. Secrets are written to the host first; any
/// failure there (the home lookup included) is `E_HOST_WRITE` and leaves the
/// form pending.
pub async fn answer(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    form_id: &str,
    values: &Map<String, Value>,
    by: &str,
) -> Result<FormView, IpcError> {
    // One answer at a time per form: two answerers would write the same
    // files, and the loser's cleanup would delete the winner's secrets.
    let _answering = Answering::claim(form_id)?;
    let row = row(store, form_id)?;
    if row.state != "pending" {
        return Err(not_pending(&row));
    }
    let form: forms::FormSpec = serde_json::from_str(&row.spec)
        .map_err(|e| IpcError::new(codes::E_INTERNAL, format!("stored form {form_id}: {e}")))?;
    let answers = forms::check_answers(&form, values).map_err(|problems: Vec<FieldProblem>| {
        IpcError::new(
            codes::E_INVALID,
            problems
                .iter()
                .map(|p| format!("{}: {}", p.field, p.problem))
                .collect::<Vec<_>>()
                .join("; "),
        )
        .with_details(serde_json::json!({ "problems": problems }))
    })?;
    let mut paths = BTreeMap::new();
    if let Some(first) = answers.secrets.keys().next() {
        let (dir, abs) = secret_dir(ssh, &row.host_alias, form_id)
            .await
            .map_err(|e| host_write_error(first, &e.message))?;
        // Durable before the first write: whatever happens next, the sweep
        // knows this form's directory may exist. The form may have been
        // withdrawn during the home lookup above: then nothing is written.
        let marked = lock(store)?.mark_form_secrets_pending(form_id)?;
        if !marked {
            return Err(not_pending(&self::row(store, form_id)?));
        }
        for (field, secret) in &answers.secrets {
            let path = format!("{dir}/{field}");
            if let Err(e) = crate::service::provision::write_host_file_secret(
                ssh,
                &row.host_alias,
                &dir,
                &path,
                secret,
            )
            .await
            {
                forget_secrets(store, ssh, &row.host_alias, form_id).await;
                return Err(host_write_error(field, &e.message));
            }
            paths.insert(field.clone(), format!("{abs}/{field}"));
        }
    }
    let text = serde_json::to_string(&Stored {
        answers: answers.values,
        secrets: paths.clone(),
    })
    .expect("answers serialise");
    let done = FormFinish {
        state: "answered",
        answers: Some(&text),
        note: None,
        answered_by: Some(by),
        secrets_on_host: !paths.is_empty(),
    };
    let finished = {
        let s = lock(store)?;
        s.finish_form(form_id, &done)?
    };
    if !finished {
        if !paths.is_empty() {
            forget_secrets(store, ssh, &row.host_alias, form_id).await;
        }
        return Err(not_pending(&self::row(store, form_id)?));
    }
    get(store, form_id)
}

/// The tick: expire pending forms older than [`EXPIRE_SECS`], delete
/// decided ones older than [`KEEP_SECS`]. Returns the rows touched.
pub fn expire_and_purge(store: &Mutex<Store>, now: i64) -> usize {
    let Ok(s) = lock(store) else { return 0 };
    let expired = s.expire_forms(now - EXPIRE_SECS).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "[forms] expiry failed");
        0
    });
    let purged = s.purge_forms(now - KEEP_SECS).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "[forms] purge failed");
        0
    });
    let drafts = s
        .purge_form_drafts(now - DRAFT_TTL_SECS)
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "[forms] draft purge failed");
            0
        });
    expired + purged + drafts
}

/// First retry delay for a host whose sweep failed; doubles per consecutive
/// failure up to [`SWEEP_BACKOFF_MAX`].
const SWEEP_BACKOFF_BASE: Duration = Duration::from_secs(10 * 60);
const SWEEP_BACKOFF_MAX: Duration = Duration::from_secs(6 * 3600);

/// Per host: when the sweep may try it again, and how many times in a row it
/// failed. An unreachable host costs ~30 s a try, so without this every tick
/// paid it again for as long as the host stayed down.
type SweepBackoff = Mutex<HashMap<String, (Instant, u32)>>;

/// In-process only: a restart tries every host once more, which is fine.
static SWEEP_BACKOFF: std::sync::LazyLock<SweepBackoff> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// The wait after the `failures`-th consecutive failure (1-based).
fn sweep_backoff_delay(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    SWEEP_BACKOFF_BASE
        .saturating_mul(1u32 << doublings)
        .min(SWEEP_BACKOFF_MAX)
}

/// The tick: remove secret directories no form needs any more. A host that
/// does not answer is tried again after a growing delay (10 min, doubling,
/// at most 6 h); a host that is no longer in the hosts table has nothing to
/// remove and its forms are marked swept.
pub async fn sweep_secret_dirs(store: &Mutex<Store>, ssh: &dyn SshExec, now: i64) -> usize {
    sweep_secret_dirs_at(store, ssh, now, &SWEEP_BACKOFF, Instant::now()).await
}

async fn sweep_secret_dirs_at(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    now: i64,
    backoff: &SweepBackoff,
    at: Instant,
) -> usize {
    let due = match lock(store).and_then(|s| Ok(s.forms_to_sweep(now - KEEP_SECS)?)) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e.message, "[forms] sweep query failed");
            return 0;
        }
    };
    let mut swept = 0;
    for (form_id, host) in due {
        if swept >= MAX_SWEEPS_PER_PASS {
            break;
        }
        // The host was removed: there is nothing left to reach, so the
        // marker would otherwise retry forever.
        let gone = host != crate::service::projects::LOCAL_HOST
            && lock(store)
                .and_then(|s| Ok(s.get_host_row(&host)?))
                .is_ok_and(|h| h.is_none());
        if gone {
            tracing::warn!(%form_id, %host, "[forms] host is gone; marking its form swept");
            if let Ok(s) = lock(store) {
                if s.mark_form_swept(&form_id).is_ok() {
                    swept += 1;
                }
            }
            continue;
        }
        // A host that failed recently (this pass included) waits: the rest
        // of its forms do not cost a timeout each.
        let waiting = backoff
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&host)
            .is_some_and(|(next, _)| *next > at);
        if waiting {
            continue;
        }
        // An answer writing this form's secrets right now owns the
        // directory: removing it under the answer would leave the files it
        // writes next with no marker. Held across the removal, so an answer
        // that starts meanwhile is refused (E_CONFLICT) rather than raced.
        let Ok(_answering) = Answering::claim(&form_id) else {
            continue;
        };
        match remove_secret_dir(ssh, &host, &form_id).await {
            Ok(()) => {
                backoff
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&host);
                if let Ok(s) = lock(store) {
                    if s.mark_form_swept(&form_id).is_ok() {
                        swept += 1;
                    }
                }
            }
            Err(e) => {
                let mut map = backoff.lock().unwrap_or_else(|e| e.into_inner());
                let failures = map.get(&host).map_or(0, |(_, n)| *n).saturating_add(1);
                let delay = sweep_backoff_delay(failures);
                tracing::debug!(%form_id, %host, error = %e.message, ?delay, "[forms] sweep deferred");
                map.insert(host, (at + delay, failures));
            }
        }
    }
    swept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::tasks::NoRecheck;
    use crate::ssh_fake::{FakeSsh, Match, Reply};
    use crate::store::now_unix;
    use serde_json::json;
    use std::time::Duration;

    const PASSWORD: &str = "hunter2-the-secret";

    fn spec() -> Value {
        json!({ "spec": "fleet.form/1", "title": "Deploy", "steps": [
            { "title": "Target", "fields": [
                { "name": "env", "type": "select", "label": "Env", "required": true,
                  "options": [["stg", "Staging"], ["prod", "Production"]] },
                { "name": "pw", "type": "secret", "label": "Password" } ] } ] })
    }

    fn fixture() -> (Mutex<Store>, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let sid = s
            .upsert_session("dev", "h", None, None, 1, 1, "running", None)
            .unwrap();
        (Mutex::new(s), sid)
    }

    /// One sweep pass with its own backoff map (the static one is shared by
    /// every test in the process).
    async fn sweep(st: &Mutex<Store>, fake: &FakeSsh) -> usize {
        sweep_secret_dirs_at(
            st,
            fake,
            now_unix(),
            &SweepBackoff::default(),
            Instant::now(),
        )
        .await
    }

    fn values(v: Value) -> Map<String, Value> {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn an_invalid_spec_is_refused_with_every_problem() {
        let (st, sid) = fixture();
        let err = open(
            &st,
            sid,
            &json!({ "spec": "fleet.form/1", "title": "", "steps": [] }),
            None,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(
            err.message.contains("title must not be empty"),
            "{}",
            err.message
        );
        assert!(
            st.lock().unwrap().forms(None, None).unwrap().is_empty(),
            "nothing stored"
        );
    }

    #[test]
    fn a_second_form_while_one_is_pending_is_a_conflict_naming_it() {
        let (st, sid) = fixture();
        let first = open(&st, sid, &spec(), None).unwrap();
        let err = open(&st, sid, &spec(), None).unwrap_err();
        assert_eq!(err.code, codes::E_CONFLICT);
        assert_eq!(err.details.unwrap()["form_id"], json!(first.form_id));
    }

    /// Redesign 10.12: `ask { draft }` puts the form being written on the
    /// session row, its `ask { form }` replaces it, and the tick drops one
    /// left half-way.
    #[test]
    fn a_draft_rides_the_row_until_the_form_opens() {
        let (st, sid) = fixture();
        let text = r#"{"spec":"fleet.form/1","title":"Deploy","steps":[{"title":"Tar"#;
        let ack = draft(&st, sid, text, Some("your hosts")).unwrap();
        assert_eq!(
            ack,
            DraftAck {
                status: "drafting",
                bytes: text.len()
            }
        );
        let row = st.lock().unwrap().get_session_by_id(sid).unwrap().unwrap();
        let d = row.form_draft.expect("the row carries the draft");
        assert_eq!(d.draft, text);
        assert_eq!(d.why.as_deref(), Some("your hosts"));
        // A newer draft replaces it.
        draft(&st, sid, &format!("{text}get"), None).unwrap();
        let row = st.lock().unwrap().get_session_by_id(sid).unwrap().unwrap();
        assert!(row.form_draft.unwrap().draft.ends_with("Target"));
        // The whole form takes its place, and a draft for it is now refused.
        open(&st, sid, &spec(), None).unwrap();
        let row = st.lock().unwrap().get_session_by_id(sid).unwrap().unwrap();
        assert_eq!(row.form_draft, None);
        assert!(row.pending_form.is_some());
        let err = draft(&st, sid, text, None).unwrap_err();
        assert_eq!(err.code, codes::E_CONFLICT);
    }

    #[test]
    fn a_draft_is_bounded_cleared_by_empty_text_and_dropped_when_stale() {
        let (st, sid) = fixture();
        let big = "x".repeat(forms::MAX_SPEC_BYTES + 1);
        assert_eq!(
            draft(&st, sid, &big, None).unwrap_err().code,
            codes::E_INVALID
        );
        assert_eq!(
            draft(&st, 9999, "{", None).unwrap_err().code,
            codes::E_NOTFOUND
        );
        draft(&st, sid, "{", None).unwrap();
        assert_eq!(draft(&st, sid, "  ", None).unwrap().status, "cleared");
        let row = st.lock().unwrap().get_session_by_id(sid).unwrap().unwrap();
        assert_eq!(row.form_draft, None);
        draft(&st, sid, "{", None).unwrap();
        // Not yet stale: kept.
        expire_and_purge(&st, now_unix());
        assert!(st
            .lock()
            .unwrap()
            .get_session_by_id(sid)
            .unwrap()
            .unwrap()
            .form_draft
            .is_some());
        expire_and_purge(&st, now_unix() + DRAFT_TTL_SECS + 1);
        assert_eq!(
            st.lock()
                .unwrap()
                .get_session_by_id(sid)
                .unwrap()
                .unwrap()
                .form_draft,
            None
        );
    }

    #[test]
    fn form_ids_are_unguessable_and_well_formed() {
        let (st, sid) = fixture();
        let a = open(&st, sid, &spec(), None).unwrap().form_id;
        assert!(a.starts_with("f_") && a.len() == 18, "{a}");
        assert!(a[2..].chars().all(|c| c.is_ascii_alphanumeric()), "{a}");
    }

    #[tokio::test]
    async fn a_wait_that_runs_out_answers_pending_and_a_later_wait_gets_the_answer() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        let r = wait(&st, &id, Duration::from_millis(30), &NoRecheck)
            .await
            .unwrap();
        assert_eq!((r.status.as_str(), r.answers.is_none()), ("pending", true));

        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        let waiting = wait(&st, &id, Duration::from_secs(5), &NoRecheck);
        let answering = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            answer(
                &st,
                &fake,
                &id,
                &values(json!({ "env": "stg" })),
                "ada (desktop)",
            )
            .await
        };
        let (r, v) = tokio::join!(waiting, answering);
        let r = r.unwrap();
        v.unwrap();
        assert_eq!(r.status, "answered");
        assert_eq!(r.answers, Some(json!({ "env": "stg" })));
        assert_eq!(r.answered_by.as_deref(), Some("ada (desktop)"));
        assert!(fake.calls().is_empty(), "no secret, no host write");
        let again = wait(&st, &id, Duration::from_millis(1), &NoRecheck)
            .await
            .unwrap();
        assert_eq!(
            again, r,
            "a finished form answers the same, at once, every time"
        );
    }

    #[tokio::test]
    async fn a_secret_goes_to_the_host_over_stdin_and_only_its_path_is_kept() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        answer(
            &st,
            &fake,
            &id,
            &values(json!({ "env": "prod", "pw": PASSWORD })),
            "ada",
        )
        .await
        .unwrap();
        let calls = fake.calls();
        assert!(
            calls.iter().all(|c| !c.command().contains(PASSWORD)),
            "never in argv or a script: {:?}",
            calls.iter().map(|c| c.command()).collect::<Vec<_>>()
        );
        let uploads: Vec<_> = calls.iter().filter(|c| c.stdin.is_some()).collect();
        assert_eq!(uploads.len(), 1);
        assert_eq!(uploads[0].stdin_str().as_deref(), Some(PASSWORD));
        let path = format!("/home/u/.cache/claude-fleet/forms/{id}/pw");
        let row = st.lock().unwrap().form(&id).unwrap().unwrap();
        assert!(row.secrets_on_host);
        let stored = row.answers.unwrap();
        assert!(!stored.contains(PASSWORD), "{stored}");
        assert!(stored.contains(&path), "{stored}");
        let r = result_of(&st.lock().unwrap().form(&id).unwrap().unwrap());
        assert_eq!(r.secrets.unwrap().get("pw"), Some(&path));
        assert_eq!(r.note.as_deref(), Some(SECRET_NOTE));
    }

    #[tokio::test]
    async fn a_failed_secret_write_leaves_the_form_pending_and_the_marker_when_cleanup_fails() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        let fake = FakeSsh::new();
        // Later rules win: everything fails except the home lookup.
        fake.on(Match::Any, Reply::fail(1, "disk full"));
        fake.with_home("/home/u");
        let err = answer(
            &st,
            &fake,
            &id,
            &values(json!({ "env": "prod", "pw": PASSWORD })),
            "ada",
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_HOST_WRITE);
        assert_eq!(err.details.unwrap()["field"], json!("pw"));
        assert!(
            fake.calls().iter().any(|c| c
                .script()
                .is_some_and(|s| s.contains("rm -rf") && s.contains(&id))),
            "a cleanup was tried"
        );
        let row = st.lock().unwrap().form(&id).unwrap().unwrap();
        assert_eq!(row.state, "pending");
        assert!(
            row.secrets_on_host,
            "the cleanup failed, so the sweep must still find it"
        );
    }

    /// A transport that withdraws a form just as the home lookup answers —
    /// the window between `answer`'s pending check and its first write.
    struct WithdrawsOnHome<'a> {
        inner: &'a FakeSsh,
        store: &'a Mutex<Store>,
        form_id: String,
    }

    #[async_trait::async_trait]
    impl SshExec for WithdrawsOnHome<'_> {
        async fn run(
            &self,
            host: &str,
            args: &[&str],
            timeout: Duration,
        ) -> Result<std::process::Output, IpcError> {
            self.inner.run(host, args, timeout).await
        }
        async fn run_bounded(
            &self,
            host: &str,
            args: &[&str],
            connect_timeout: Duration,
            wall_clock: Duration,
        ) -> Result<std::process::Output, IpcError> {
            self.inner
                .run_bounded(host, args, connect_timeout, wall_clock)
                .await
        }
        async fn run_cancellable(
            &self,
            host: &str,
            args: &[&str],
            timeout: Duration,
            token: tokio_util::sync::CancellationToken,
        ) -> Result<std::process::Output, IpcError> {
            self.inner.run_cancellable(host, args, timeout, token).await
        }
        async fn run_bounded_cancellable(
            &self,
            host: &str,
            args: &[&str],
            connect_timeout: Duration,
            wall_clock: Duration,
            token: tokio_util::sync::CancellationToken,
        ) -> Result<std::process::Output, IpcError> {
            self.inner
                .run_bounded_cancellable(host, args, connect_timeout, wall_clock, token)
                .await
        }
        async fn upload_file(
            &self,
            host: &str,
            local_path: &std::path::Path,
            remote_path: &str,
            timeout: Duration,
        ) -> Result<(), IpcError> {
            self.inner
                .upload_file(host, local_path, remote_path, timeout)
                .await
        }
        async fn remote_home(&self, host: &str) -> Result<String, IpcError> {
            let _ = cancel(self.store, &self.form_id);
            self.inner.remote_home(host).await
        }
        async fn run_with_stdin(
            &self,
            host: &str,
            args: &[&str],
            stdin: Vec<u8>,
            connect_timeout: Duration,
            wall_clock: Duration,
            max_output: usize,
        ) -> Result<std::process::Output, IpcError> {
            self.inner
                .run_with_stdin(host, args, stdin, connect_timeout, wall_clock, max_output)
                .await
        }
    }

    #[tokio::test]
    async fn a_form_withdrawn_during_the_home_lookup_writes_no_secret() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        let ssh = WithdrawsOnHome {
            inner: &fake,
            store: &st,
            form_id: id.clone(),
        };
        let err = answer(
            &st,
            &ssh,
            &id,
            &values(json!({ "env": "prod", "pw": PASSWORD })),
            "ada",
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_CONFLICT);
        assert!(
            fake.calls().iter().all(|c| c.stdin.is_none()),
            "no secret may be uploaded for a form that is no longer pending"
        );
        let row = st.lock().unwrap().form(&id).unwrap().unwrap();
        assert_eq!(row.state, "cancelled");
        assert!(!row.secrets_on_host);
    }

    #[tokio::test]
    async fn a_failed_upload_with_a_working_cleanup_clears_the_marker() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        fake.on(Match::prefix("cat > "), Reply::fail(1, "disk full"));
        let err = answer(
            &st,
            &fake,
            &id,
            &values(json!({ "env": "prod", "pw": PASSWORD })),
            "ada",
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_HOST_WRITE);
        assert!(
            fake.calls().iter().any(|c| c
                .script()
                .is_some_and(|s| s.contains("rm -rf") && s.contains(&id))),
            "a cleanup was tried"
        );
        let row = st.lock().unwrap().form(&id).unwrap().unwrap();
        assert_eq!(
            (row.state.as_str(), row.secrets_on_host),
            ("pending", false)
        );
    }

    #[tokio::test]
    async fn a_failed_home_lookup_is_a_host_write_error_naming_the_first_secret() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        let fake = FakeSsh::new();
        fake.on(Match::prefix("printenv HOME"), Reply::fail(1, "no shell"));
        let err = answer(
            &st,
            &fake,
            &id,
            &values(json!({ "env": "prod", "pw": PASSWORD })),
            "ada",
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_HOST_WRITE);
        assert_eq!(err.details.unwrap()["field"], json!("pw"));
        let row = st.lock().unwrap().form(&id).unwrap().unwrap();
        assert_eq!(
            (row.state.as_str(), row.secrets_on_host),
            ("pending", false)
        );
    }

    #[test]
    fn a_local_host_without_home_is_a_host_write_error() {
        assert_eq!(local_home(None).unwrap_err().code, codes::E_HOST_WRITE);
        assert_eq!(
            local_home(Some(String::new())).unwrap_err().code,
            codes::E_HOST_WRITE
        );
        assert_eq!(local_home(Some("/h".into())).unwrap(), "/h");
    }

    #[tokio::test]
    async fn two_answers_at_once_one_wins_and_keeps_its_secret() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        let v = values(json!({ "env": "prod", "pw": PASSWORD }));
        let (a, b) = tokio::join!(
            answer(&st, &fake, &id, &v, "ada"),
            answer(&st, &fake, &id, &v, "bob")
        );
        let (ok, err) = match (a, b) {
            (Ok(v), Err(e)) | (Err(e), Ok(v)) => (v, e),
            other => panic!("exactly one wins: {:?}", other.0.is_ok()),
        };
        assert_eq!(err.code, codes::E_CONFLICT);
        assert_eq!(ok.state, "answered");
        let row = st.lock().unwrap().form(&id).unwrap().unwrap();
        assert!(row.secrets_on_host);
        assert!(row
            .answers
            .unwrap()
            .contains(&format!("/home/u/.cache/claude-fleet/forms/{id}/pw")));
    }

    #[tokio::test]
    async fn a_form_being_answered_refuses_a_second_answer() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        let claim = Answering::claim(&id).unwrap();
        let err = answer(
            &st,
            &FakeSsh::new(),
            &id,
            &values(json!({ "env": "stg" })),
            "bob",
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_CONFLICT);
        assert!(
            err.message.contains("being answered right now"),
            "{}",
            err.message
        );
        assert_eq!(
            err.details.unwrap(),
            json!({ "form_id": id, "state": "pending" })
        );
        drop(claim);
        answer(
            &st,
            &FakeSsh::new(),
            &id,
            &values(json!({ "env": "stg" })),
            "bob",
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn one_sweep_pass_tries_an_unreachable_host_once() {
        let (st, sid) = fixture();
        open(&st, sid, &spec(), None).unwrap();
        let first = st
            .lock()
            .unwrap()
            .pending_form_of_session(sid)
            .unwrap()
            .unwrap()
            .form_id;
        cancel(&st, &first).unwrap();
        open(&st, sid, &spec(), None).unwrap();
        {
            let s = st.lock().unwrap();
            s.conn_ref()
                .execute("UPDATE form_requests SET secrets_on_host = 1", [])
                .unwrap();
            s.mark_session_killed(sid, 5).unwrap();
        }
        let fake = FakeSsh::new();
        fake.on(Match::Any, Reply::Unreachable);
        assert_eq!(sweep(&st, &fake).await, 0);
        assert_eq!(
            fake.calls().len(),
            1,
            "the second form on the same host waits"
        );
    }

    #[tokio::test]
    async fn bad_values_come_back_per_field_and_the_form_stays_pending() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        let err = answer(
            &st,
            &FakeSsh::new(),
            &id,
            &values(json!({ "env": "dev" })),
            "ada",
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert_eq!(
            err.details.unwrap()["problems"],
            json!([{ "field": "env", "problem": "must be one of the options" }])
        );
        assert_eq!(get(&st, &id).unwrap().state, "pending");
    }

    #[tokio::test]
    async fn decline_cancel_and_a_late_answer() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        let v = decline(&st, &id, Some("not now"), "ada").unwrap();
        assert_eq!(
            (v.state.as_str(), v.note.as_deref()),
            ("declined", Some("not now"))
        );
        let late = answer(
            &st,
            &FakeSsh::new(),
            &id,
            &values(json!({ "env": "stg" })),
            "bob",
        )
        .await
        .unwrap_err();
        assert_eq!(late.code, codes::E_CONFLICT);
        assert_eq!(late.details.unwrap()["state"], json!("declined"));

        let id2 = open(&st, sid, &spec(), None).unwrap().form_id;
        assert_eq!(cancel(&st, &id2).unwrap().status, "cancelled");
        assert_eq!(cancel(&st, &id2).unwrap_err().code, codes::E_CONFLICT);
        assert_eq!(get(&st, "f_nope").unwrap_err().code, codes::E_NOTFOUND);
    }

    #[tokio::test]
    async fn the_tick_expires_and_the_sweep_removes_a_ghosts_secrets() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        assert_eq!(expire_and_purge(&st, now_unix() + EXPIRE_SECS + 1), 1);
        assert_eq!(get(&st, &id).unwrap().state, "expired");
        {
            let s = st.lock().unwrap();
            s.conn_ref()
                .execute("UPDATE form_requests SET secrets_on_host = 1", [])
                .unwrap();
            s.mark_session_killed(sid, 5).unwrap();
        }
        let fake = FakeSsh::new();
        assert_eq!(sweep(&st, &fake).await, 1);
        let script = fake.calls()[0].script().unwrap();
        assert!(
            script.contains("rm -rf") && script.contains(&id),
            "{script}"
        );
        assert!(
            !st.lock()
                .unwrap()
                .form(&id)
                .unwrap()
                .unwrap()
                .secrets_on_host
        );
        assert_eq!(sweep(&st, &fake).await, 0, "done once");
    }

    /// The sweep leaves alone a form that is being answered right now.
    #[tokio::test]
    async fn the_sweep_skips_a_form_while_it_is_answered() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        {
            let s = st.lock().unwrap();
            s.mark_form_secrets_pending(&id).unwrap();
            s.mark_session_killed(sid, 5).unwrap();
        }
        let fake = FakeSsh::new();
        let answering = Answering::claim(&id).unwrap();
        assert_eq!(sweep(&st, &fake).await, 0);
        assert!(fake.calls().is_empty());
        assert!(
            st.lock()
                .unwrap()
                .form(&id)
                .unwrap()
                .unwrap()
                .secrets_on_host
        );
        drop(answering);
        assert_eq!(sweep(&st, &fake).await, 1);
    }

    #[tokio::test]
    async fn an_unreachable_host_is_swept_on_a_later_tick() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        {
            let s = st.lock().unwrap();
            s.conn_ref()
                .execute("UPDATE form_requests SET secrets_on_host = 1", [])
                .unwrap();
            s.mark_session_killed(sid, 5).unwrap();
        }
        let fake = FakeSsh::new();
        fake.on(Match::Any, Reply::Unreachable);
        assert_eq!(sweep(&st, &fake).await, 0);
        assert!(
            st.lock()
                .unwrap()
                .form(&id)
                .unwrap()
                .unwrap()
                .secrets_on_host
        );
    }

    /// A session killed with a pending secret marker, one form per call.
    fn orphaned_form(st: &Mutex<Store>, sid: i64) -> String {
        let id = open(st, sid, &spec(), None).unwrap().form_id;
        let s = st.lock().unwrap();
        s.conn_ref()
            .execute(
                "UPDATE form_requests SET secrets_on_host = 1 WHERE form_id = ?1",
                [&id],
            )
            .unwrap();
        s.mark_session_killed(sid, 5).unwrap();
        id
    }

    #[tokio::test]
    async fn an_unreachable_host_is_skipped_until_its_backoff_has_passed() {
        let (st, sid) = fixture();
        orphaned_form(&st, sid);
        let fake = FakeSsh::new();
        fake.on(Match::Any, Reply::Unreachable);
        let backoff = SweepBackoff::default();
        let t0 = Instant::now();
        let now = now_unix();
        assert_eq!(sweep_secret_dirs_at(&st, &fake, now, &backoff, t0).await, 0);
        assert_eq!(fake.calls().len(), 1, "tried once");
        // The very next pass: zero ssh calls.
        assert_eq!(sweep_secret_dirs_at(&st, &fake, now, &backoff, t0).await, 0);
        assert_eq!(fake.calls().len(), 1, "skipped while backing off");
        let almost = t0 + SWEEP_BACKOFF_BASE - Duration::from_secs(1);
        sweep_secret_dirs_at(&st, &fake, now, &backoff, almost).await;
        assert_eq!(fake.calls().len(), 1, "still skipped just before the delay");
        // After 10 minutes it is tried again, and the delay doubles.
        let t1 = t0 + SWEEP_BACKOFF_BASE;
        sweep_secret_dirs_at(&st, &fake, now, &backoff, t1).await;
        assert_eq!(fake.calls().len(), 2, "tried again once the delay passed");
        sweep_secret_dirs_at(&st, &fake, now, &backoff, t1 + SWEEP_BACKOFF_BASE).await;
        assert_eq!(
            fake.calls().len(),
            2,
            "the second failure waits twice as long"
        );
        // A host that answers again is swept and forgotten.
        let up = FakeSsh::new();
        let t2 = t1 + SWEEP_BACKOFF_BASE * 2;
        assert_eq!(sweep_secret_dirs_at(&st, &up, now, &backoff, t2).await, 1);
        assert!(backoff.lock().unwrap().is_empty());
    }

    #[test]
    fn the_backoff_doubles_from_ten_minutes_and_stops_at_six_hours() {
        let mins = |f| sweep_backoff_delay(f).as_secs() / 60;
        assert_eq!(
            [
                mins(1),
                mins(2),
                mins(3),
                mins(4),
                mins(5),
                mins(6),
                mins(40)
            ],
            [10, 20, 40, 80, 160, 320, 360]
        );
    }

    #[tokio::test]
    async fn a_form_on_a_host_that_is_gone_is_marked_swept_without_ssh() {
        let (st, sid) = fixture();
        let id = orphaned_form(&st, sid);
        st.lock()
            .unwrap()
            .conn_ref()
            .execute("PRAGMA foreign_keys = OFF", [])
            .unwrap();
        st.lock()
            .unwrap()
            .conn_ref()
            .execute("DELETE FROM hosts WHERE alias = 'h'", [])
            .unwrap();
        let fake = FakeSsh::new();
        assert_eq!(sweep(&st, &fake).await, 1);
        assert!(fake.calls().is_empty(), "no host to reach, no ssh call");
        let row = st.lock().unwrap().form(&id).unwrap().unwrap();
        assert!(!row.secrets_on_host);
    }
}
