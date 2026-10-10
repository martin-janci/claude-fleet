//! Wizards that resume on another device (Orbit Fleet M15 step G7.2): a
//! wizard saves where the person is (its step and answers) on the hub, and
//! any of that person's devices reads it back and carries on. Generalised
//! from the add-host wizard's drafts, which are the `add_host` rows.
//!
//! **Whose a row is.** Every row is a person's: a save writes the caller's
//! person, and a caller reads, replaces and clears only rows it may own
//! (`ViewScope::may_own_person_row`: its own, and fleet's own rows on a
//! single-person fleet or for the hub's own reader). Anyone else's row
//! answers as absent. A per-host token is not served the tool: a session
//! runs no wizard.
//!
//! **What a row holds.** The step and the wizard's own answers object, at
//! most [`ANSWERS_MAX_BYTES`]. Never a secret: an answer whose key names a
//! secret (a token, a password, an API key) is refused, so a wizard that
//! forgets to leave one out fails loudly instead of storing it.

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::view_scope::ViewScope;
use crate::store::{Store, WizardStateRow, WizardStateWrite};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// The wizards that save their place.
pub const WIZARD_KINDS: &[&str] = &[
    "add_host",
    "add_project",
    "add_account",
    "new_session",
    "link_peer",
    "form",
];
/// The answers object, serialised, at most.
pub const ANSWERS_MAX_BYTES: usize = 16 * 1024;
/// A key, a label, a device name: at most this many characters.
pub const TEXT_MAX_CHARS: usize = 200;
/// The highest step a wizard has.
pub const STEP_MAX: i64 = 20;
/// A wizard untouched this long is forgotten (by `purge`).
pub const KEEP_SECS: i64 = 14 * 24 * 3600;

/// Words that mark an answer's key as a secret's.
const SECRET_WORDS: &[&str] = &["secret", "token", "password", "passwd", "api_key", "apikey"];

/// One `wizard_state` call: the MCP tool's parameters and the desktop
/// command's arguments alike.
#[derive(Debug, Clone, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct WizardStateArgs {
    /// list | get | save | clear.
    pub action: String,
    /// add_host | add_project | add_account | new_session | link_peer |
    /// form. Every action but list, where it narrows the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Which one of the kind (the SSH alias of an add-host wizard); absent
    /// = the person's one wizard of that kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// save: the step it is on, from 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<i64>,
    /// save: the wizard's own answers object. Never a secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<serde_json::Value>,
    /// save: what the resume line names ("acme/api").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_INVALID, msg.into())
}

fn kind_of(args: &WizardStateArgs) -> Result<&str, IpcError> {
    let k = args
        .kind
        .as_deref()
        .ok_or_else(|| invalid(format!("{} needs kind", args.action)))?;
    if !WIZARD_KINDS.contains(&k) {
        return Err(invalid(format!(
            "kind must be one of {}, got {k:?}",
            WIZARD_KINDS.join(" | ")
        )));
    }
    Ok(k)
}

fn short(what: &str, v: Option<&str>) -> Result<Option<String>, IpcError> {
    let Some(v) = v.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if v.chars().count() > TEXT_MAX_CHARS || v.chars().any(char::is_control) {
        return Err(invalid(format!(
            "{what} is one line of at most {TEXT_MAX_CHARS} characters"
        )));
    }
    Ok(Some(v.to_string()))
}

/// PURE: the first key in `v`, at any depth, that names a secret.
pub fn secret_key(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Object(m) => m.iter().find_map(|(k, x)| {
            let lower = k.to_ascii_lowercase();
            if SECRET_WORDS.iter().any(|w| lower.contains(w)) {
                Some(k.clone())
            } else {
                secret_key(x)
            }
        }),
        serde_json::Value::Array(xs) => xs.iter().find_map(secret_key),
        _ => None,
    }
}

/// PURE: `answers` as a row keeps it, or why not.
pub fn check_answers(answers: Option<&serde_json::Value>) -> Result<serde_json::Value, IpcError> {
    let a = match answers {
        None | Some(serde_json::Value::Null) => return Ok(serde_json::json!({})),
        Some(a @ serde_json::Value::Object(_)) => a,
        Some(_) => return Err(invalid("answers is an object")),
    };
    if a.to_string().len() > ANSWERS_MAX_BYTES {
        return Err(invalid(format!(
            "answers are at most {} KiB",
            ANSWERS_MAX_BYTES / 1024
        )));
    }
    if let Some(k) = secret_key(a) {
        return Err(invalid(format!(
            "answers never hold a secret; leave {k:?} out (the person types it again)"
        )));
    }
    Ok(a.clone())
}

/// The row `scope` may read, or `None`.
fn readable(
    s: &Store,
    scope: &ViewScope,
    kind: &str,
    key: &str,
) -> Result<Option<WizardStateRow>, IpcError> {
    // The caller's own row first; on a single-person fleet, fleet's own.
    for person in [scope.person, None] {
        if let Some(r) = s.wizard_state(kind, key, person)? {
            if scope.may_own_person_row(None, r.person_id) {
                return Ok(Some(r));
            }
        }
        if scope.person.is_none() {
            break;
        }
    }
    Ok(None)
}

/// `wizard_state { list, kind? }`: the caller's wizards, the most recently
/// touched first.
pub fn list(
    store: &Mutex<Store>,
    scope: &ViewScope,
    kind: Option<&str>,
) -> Result<Vec<WizardStateRow>, IpcError> {
    let s = lock(store)?;
    Ok(s.wizard_states(kind)?
        .into_iter()
        .filter(|r| scope.may_own_person_row(None, r.person_id))
        .collect())
}

/// `wizard_state { get, kind, key? }`.
pub fn get(
    store: &Mutex<Store>,
    scope: &ViewScope,
    kind: &str,
    key: &str,
) -> Result<Option<WizardStateRow>, IpcError> {
    let s = lock(store)?;
    readable(&s, scope, kind, key)
}

/// `wizard_state { save, kind, key?, step, answers, label? }`: where the
/// person is, from `device` (the paired device's name; `None` = the hub's
/// own desktop). A row of fleet's own the caller may own is replaced in
/// place; otherwise the row is the caller's.
pub fn save(
    store: &Mutex<Store>,
    scope: &ViewScope,
    args: &WizardStateArgs,
    device: Option<&str>,
) -> Result<WizardStateRow, IpcError> {
    let kind = kind_of(args)?;
    let key = short("key", args.key.as_deref())?.unwrap_or_default();
    let label = short("label", args.label.as_deref())?;
    let device = short("device", device)?;
    let step = args.step.ok_or_else(|| invalid("save needs step"))?;
    if !(1..=STEP_MAX).contains(&step) {
        return Err(invalid(format!("step must be 1..{STEP_MAX}")));
    }
    let answers = check_answers(args.answers.as_ref())?;
    let s = lock(store)?;
    let person = match readable(&s, scope, kind, &key)? {
        Some(r) => r.person_id,
        None => scope.person,
    };
    Ok(s.save_wizard_state(&WizardStateWrite {
        kind,
        key: &key,
        person_id: person,
        label: label.as_deref(),
        step,
        answers: Some(&answers),
        checks: None,
        device: device.as_deref(),
    })?)
}

/// `wizard_state { clear, kind, key? }`: finished or discarded. Whether
/// there was one.
pub fn clear(
    store: &Mutex<Store>,
    scope: &ViewScope,
    kind: &str,
    key: &str,
) -> Result<bool, IpcError> {
    let s = lock(store)?;
    match readable(&s, scope, kind, key)? {
        Some(r) => Ok(s.delete_wizard_state(kind, key, r.person_id)?),
        None => Ok(false),
    }
}

/// Forget wizards untouched for [`KEEP_SECS`]; how many. The tick's.
pub fn purge(store: &Mutex<Store>, now: i64) -> Result<usize, IpcError> {
    Ok(lock(store)?.purge_wizard_states(now - KEEP_SECS)?)
}

/// Run one `wizard_state` call for `scope`: `list` answers the rows, `get`
/// the row or `null`, `save` the row as it now stands, `clear` `{ removed }`.
pub fn run(
    store: &Mutex<Store>,
    scope: &ViewScope,
    args: &WizardStateArgs,
    device: Option<&str>,
) -> Result<serde_json::Value, IpcError> {
    let key = || -> Result<String, IpcError> {
        Ok(short("key", args.key.as_deref())?.unwrap_or_default())
    };
    match args.action.as_str() {
        "list" => {
            let kind = match args.kind.as_deref() {
                Some(_) => Some(kind_of(args)?),
                None => None,
            };
            to_json(&list(store, scope, kind)?)
        }
        "get" => to_json(&get(store, scope, kind_of(args)?, &key()?)?),
        "save" => to_json(&save(store, scope, args, device)?),
        "clear" => {
            Ok(serde_json::json!({ "removed": clear(store, scope, kind_of(args)?, &key()?)? }))
        }
        other => Err(invalid(format!(
            "action must be list | get | save | clear, got {other:?}"
        ))),
    }
}

fn to_json<T: Serialize>(v: &T) -> Result<serde_json::Value, IpcError> {
    serde_json::to_value(v).map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))
}

#[cfg(test)]
#[path = "wizard_state_tests.rs"]
mod tests;
