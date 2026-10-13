//! Pinned dashboards (persona design, step 6): a dashboard a person pinned
//! from an agent's `dashboard` block (docs/chat-blocks.md), kept on the hub
//! so every device of theirs shows it and the agent reads what is pinned
//! before it proposes a change.
//!
//! **Whose a row is.** Every row is a person's: a pin writes the caller's
//! person, and a caller reads, replaces and unpins only rows it may own
//! (`ViewScope::may_own_person_row`), as `wizard_state` does. Anyone
//! else's row answers as absent. A per-host token is not served the tool.
//!
//! **Who pins.** A person, from the card or the Dashboards page. The agent
//! may `list` and `get`; its `pin` and `unpin` are refused (`may_pin`
//! false): it proposes with a block and the person decides.
//!
//! **Who hears of it.** A pin or an unpin emits `dashboards:changed`, an
//! empty frame: the person's open devices re-read through this tool, so no
//! dashboard rides the event stream (persona design step 7).
//!
//! **What a row holds.** A block that passes the same check the app draws
//! it with (`pages::chat_blocks::check`), never a built-in's id, at most
//! [`MAX_PINNED`] per person.

use crate::ipc_error::{codes, lock, IpcError};
use crate::pages::chat_blocks;
use crate::service::view_scope::ViewScope;
use crate::store::{PinnedDashboardRow, PinnedDashboardWrite, Store};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// The built-in dashboards' ids (`BUILTIN_DASHBOARDS` in
/// src/lib/dashboards.ts): never a pinned one's.
pub const BUILTIN_IDS: &[&str] = &[
    "dash.developer",
    "dash.tester",
    "dash.pm",
    "dash.scrum",
    "dash.po",
];
/// The most dashboards one person keeps pinned (`MAX_PINNED` in
/// dashboards.ts).
pub const MAX_PINNED: usize = 20;
/// `why` and a device name: at most this many characters.
const TEXT_MAX_CHARS: usize = 500;

/// One `dashboards` call: the MCP tool's parameters and the desktop
/// command's arguments alike.
#[derive(Debug, Clone, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct DashboardsArgs {
    /// list | get | pin | unpin.
    pub action: String,
    /// get, unpin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// pin: a `dashboard` block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard: Option<serde_json::Value>,
    /// pin: why, one line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_INVALID, msg.into())
}

fn text(what: &str, v: Option<&str>) -> Result<Option<String>, IpcError> {
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

fn id_of(args: &DashboardsArgs) -> Result<&str, IpcError> {
    args.id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| invalid(format!("{} needs id", args.action)))
}

/// PURE: the block a pin keeps, its id and title, or why not. `why` is
/// kept beside the block, not in it.
pub fn check_dashboard(
    v: Option<&serde_json::Value>,
) -> Result<(serde_json::Value, String, String), IpcError> {
    let Some(mut block) = v.filter(|v| v.is_object()).cloned() else {
        return Err(invalid(
            "pin needs dashboard, a `dashboard` fleet.ui/1 block",
        ));
    };
    if block.get("kind").and_then(|k| k.as_str()) != Some("dashboard") {
        return Err(invalid("dashboard must be a block of kind \"dashboard\""));
    }
    if let Err(problems) = chat_blocks::check(&block) {
        return Err(invalid(format!(
            "the dashboard does not check out: {}",
            problems.join("; ")
        )));
    }
    if let Some(o) = block.as_object_mut() {
        o.remove("why");
    }
    let id = block["id"].as_str().unwrap_or_default().to_string();
    if BUILTIN_IDS.contains(&id.as_str()) {
        return Err(invalid(format!(
            "{id} is a built-in dashboard's id; give the dashboard an id of its own"
        )));
    }
    let title = block["title"].as_str().unwrap_or_default().to_string();
    Ok((block, id, title))
}

/// A row as it is answered: its spec checked again, so a row an older or
/// newer build wrote that no longer checks out is left out, not drawn.
fn shown(r: PinnedDashboardRow) -> Option<PinnedDashboardRow> {
    chat_blocks::check(&r.spec).ok().map(|_| r)
}

/// The row `scope` may read, or `None`.
fn readable(
    s: &Store,
    scope: &ViewScope,
    id: &str,
) -> Result<Option<PinnedDashboardRow>, IpcError> {
    // The caller's own row first; on a single-person fleet, fleet's own.
    for person in [scope.person, None] {
        if let Some(r) = s.pinned_dashboard(id, person)? {
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

/// `dashboards { list }`: the caller's pinned dashboards, in the order
/// pinned.
pub fn list(store: &Mutex<Store>, scope: &ViewScope) -> Result<Vec<PinnedDashboardRow>, IpcError> {
    let s = lock(store)?;
    Ok(s.pinned_dashboards()?
        .into_iter()
        .filter(|r| scope.may_own_person_row(None, r.person_id))
        .filter_map(shown)
        .collect())
}

/// `dashboards { get, id }`.
pub fn get(
    store: &Mutex<Store>,
    scope: &ViewScope,
    id: &str,
) -> Result<Option<PinnedDashboardRow>, IpcError> {
    let s = lock(store)?;
    Ok(readable(&s, scope, id)?.and_then(shown))
}

/// `dashboards { pin, dashboard, why? }`: pin it, or replace the pinned one
/// of its id, from `device` (`None` = the hub's own desktop).
pub fn pin(
    store: &Mutex<Store>,
    scope: &ViewScope,
    args: &DashboardsArgs,
    device: Option<&str>,
) -> Result<PinnedDashboardRow, IpcError> {
    let (block, id, title) = check_dashboard(args.dashboard.as_ref())?;
    let why = text("why", args.why.as_deref())?;
    let device = text("device", device)?;
    let s = lock(store)?;
    let person = match readable(&s, scope, &id)? {
        Some(r) => r.person_id,
        None => {
            let held = s
                .pinned_dashboards()?
                .into_iter()
                .filter(|r| scope.may_own_person_row(None, r.person_id))
                .count();
            if held >= MAX_PINNED {
                return Err(invalid(format!(
                    "{MAX_PINNED} dashboards are pinned; unpin one first"
                )));
            }
            scope.person
        }
    };
    let row = s.pin_dashboard(&PinnedDashboardWrite {
        id: &id,
        person_id: person,
        title: &title,
        spec: &block,
        why: why.as_deref(),
        device: device.as_deref(),
    })?;
    // The person's other devices re-read their pins.
    s.bus_dashboards_changed();
    Ok(row)
}

/// `dashboards { unpin, id }`: whether it was pinned.
pub fn unpin(store: &Mutex<Store>, scope: &ViewScope, id: &str) -> Result<bool, IpcError> {
    let s = lock(store)?;
    match readable(&s, scope, id)? {
        Some(r) => {
            let removed = s.unpin_dashboard(id, r.person_id)?;
            if removed {
                s.bus_dashboards_changed();
            }
            Ok(removed)
        }
        None => Ok(false),
    }
}

/// Run one `dashboards` call for `scope`: `list` answers the rows, `get`
/// the row or `null`, `pin` the row as it now stands, `unpin`
/// `{ removed }`. `may_pin` is false for the agent: it reads, a person pins.
pub fn run(
    store: &Mutex<Store>,
    scope: &ViewScope,
    args: &DashboardsArgs,
    device: Option<&str>,
    may_pin: bool,
) -> Result<serde_json::Value, IpcError> {
    let person_only = || -> Result<(), IpcError> {
        if may_pin {
            Ok(())
        } else {
            Err(IpcError::new(
                codes::E_FORBIDDEN,
                "a person pins and unpins a dashboard; propose it with a `dashboard` block",
            ))
        }
    };
    match args.action.as_str() {
        "list" => to_json(&list(store, scope)?),
        "get" => to_json(&get(store, scope, id_of(args)?)?),
        "pin" => {
            person_only()?;
            to_json(&pin(store, scope, args, device)?)
        }
        "unpin" => {
            person_only()?;
            Ok(serde_json::json!({ "removed": unpin(store, scope, id_of(args)?)? }))
        }
        other => Err(invalid(format!(
            "action must be list | get | pin | unpin, got {other:?}"
        ))),
    }
}

fn to_json<T: Serialize>(v: &T) -> Result<serde_json::Value, IpcError> {
    serde_json::to_value(v).map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))
}

#[cfg(test)]
#[path = "dashboards_tests.rs"]
mod tests;
