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
//! **Who pins.** A person, from the card or the Dashboards page, and
//! Control's agent when the person asked it for a change ("Change with the
//! agent"): owner report 2026-11-06, a change that waited for a click on
//! the card in the chat read as a change that never happened. A
//! replacement keeps the version it overwrote, and the page offers
//! *Undo* (`undo`) on a dashboard Control changed. Only a person unpins
//! or undoes (`is_person` false refuses them).
//!
//! **Who hears of it.** A pin or an unpin emits `dashboards:changed`, an
//! empty frame: the person's open devices re-read through this tool, so no
//! dashboard rides the event stream (persona design step 7).
//!
//! **What a row holds.** A block that passes the same check the app draws
//! it with (`pages::chat_blocks::check`), never a built-in's id, at most
//! [`MAX_PINNED`] per person.
//!
//! **Feeds** (migration 222, owner report 2026-11-06: dashboards from
//! dynamic data). A `feed` widget shows a value by its key, never computed
//! by the app: Control's agent or the person writes one (`feed {key,
//! value}`; an empty value removes it), and a routine's run writes one in
//! its reply (`<dashboard-feed key="…">…</dashboard-feed>`, read by
//! `service::routines::memory`). A feed is its writer's person's, like a
//! pin, at most [`MAX_FEEDS`] each; a write emits `dashboards:changed`.

use crate::ipc_error::{codes, lock, IpcError};
use crate::pages::chat_blocks;
use crate::service::view_scope::ViewScope;
use crate::store::{DashboardFeedRow, PinnedDashboardRow, PinnedDashboardWrite, Store};
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
/// The most feeds one person keeps.
pub const MAX_FEEDS: usize = 50;
/// A feed's value, in characters.
pub const FEED_VALUE_MAX_CHARS: usize = 4000;
/// A feed's key, in characters.
pub const FEED_KEY_MAX_CHARS: usize = 64;

/// One `dashboards` call: the MCP tool's parameters and the desktop
/// command's arguments alike.
#[derive(Debug, Clone, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct DashboardsArgs {
    /// list | get | pin | unpin | undo | feeds | feed.
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
    /// feed key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// feed: text; empty removes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
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

/// The person whose rows `scope` writes: its own, and for the hub's own
/// caller (the standalone desktop) the hub's personal owner, as
/// `routines::save` does, so a row Control's agent (whose token is the
/// owner's) wrote is the desktop's too. `None` on a hub with no people.
fn own_person(s: &Store, scope: &ViewScope) -> Result<Option<i64>, IpcError> {
    Ok(match scope.person {
        None if scope.is_internal() => s.personal_owner_id()?,
        p => p,
    })
}

/// The row `scope` may read, or `None`.
fn readable(
    s: &Store,
    scope: &ViewScope,
    id: &str,
) -> Result<Option<PinnedDashboardRow>, IpcError> {
    // The caller's own row first; on a single-person fleet, fleet's own.
    let own = own_person(s, scope)?;
    for person in [own, None] {
        if let Some(r) = s.pinned_dashboard(id, person)? {
            if scope.may_own_person_row(None, r.person_id) {
                return Ok(Some(r));
            }
        }
        if own.is_none() {
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
            own_person(&s, scope)?
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

/// Take back the last change to a pinned dashboard, as its row's `undo`
/// says: `restore` puts back the version before its last replacement (or
/// before Control's run of changes); `remove` unpins a dashboard Control
/// pinned new, so Undo always takes back what Control did.
/// `{undone, removed, dashboard?}`.
pub fn undo(
    store: &Mutex<Store>,
    scope: &ViewScope,
    id: &str,
) -> Result<serde_json::Value, IpcError> {
    let s = lock(store)?;
    let Some(r) = readable(&s, scope, id)? else {
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("no pinned dashboard {id}"),
        ));
    };
    match r.undo.as_deref() {
        Some("restore") => {
            let row = s.undo_dashboard(id, r.person_id)?;
            s.bus_dashboards_changed();
            Ok(serde_json::json!({ "undone": true, "removed": false, "dashboard": row }))
        }
        Some("remove") => {
            let removed = s.unpin_dashboard(id, r.person_id)?;
            if removed {
                s.bus_dashboards_changed();
            }
            Ok(serde_json::json!({ "undone": removed, "removed": removed }))
        }
        _ => Err(invalid(format!(
            "{id} keeps no earlier version to go back to"
        ))),
    }
}

/// PURE: `key` if it names a feed: letters, digits and `. _ : -`, at most
/// [`FEED_KEY_MAX_CHARS`].
pub fn feed_key(key: Option<&str>) -> Result<&str, IpcError> {
    let key = key.map(str::trim).unwrap_or_default();
    let ok = !key.is_empty()
        && key.chars().count() <= FEED_KEY_MAX_CHARS
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'));
    if ok {
        Ok(key)
    } else {
        Err(invalid(format!(
            "a feed's key is 1-{FEED_KEY_MAX_CHARS} letters, digits and . _ : -"
        )))
    }
}

/// PURE: a feed's value as it is kept: trimmed, cut to
/// [`FEED_VALUE_MAX_CHARS`]; `None` when empty (a removal).
pub fn feed_value(v: &str) -> Option<String> {
    let v = v.trim();
    (!v.is_empty()).then(|| v.chars().take(FEED_VALUE_MAX_CHARS).collect())
}

/// `dashboards { feeds }`: the caller's feeds, by key. On a fleet where the
/// caller reads others' rows too (the hub's own), its own come first and
/// win a key.
pub fn feeds(store: &Mutex<Store>, scope: &ViewScope) -> Result<Vec<DashboardFeedRow>, IpcError> {
    let s = lock(store)?;
    let own = own_person(&s, scope)?;
    // Only the hub's own caller reads every person's; anyone else reads
    // their own and, on a single-person fleet, fleet's own.
    let read = if scope.is_internal() {
        s.all_dashboard_feeds()?
    } else {
        let mut v = s.dashboard_feeds(None)?;
        if own.is_some() {
            v.extend(s.dashboard_feeds(own)?);
        }
        v
    };
    let mut rows: Vec<DashboardFeedRow> = read
        .into_iter()
        .filter(|r| scope.may_own_person_row(None, r.person_id))
        .collect();
    // Others' first, so the caller's own overwrite them by key.
    rows.sort_by_key(|r| r.person_id == own);
    let by_key: std::collections::BTreeMap<String, DashboardFeedRow> =
        rows.into_iter().map(|r| (r.key.clone(), r)).collect();
    Ok(by_key.into_values().collect())
}

/// Write feed `key` of `person`, by `by` (person | agent | routine): an
/// empty value removes it. `None` when removed (or there was none).
pub fn write_feed(
    s: &Store,
    person: Option<i64>,
    key: &str,
    value: &str,
    by: &str,
    by_ref: Option<&str>,
    now: i64,
) -> Result<Option<DashboardFeedRow>, IpcError> {
    let Some(value) = feed_value(value) else {
        if s.delete_dashboard_feed(person, key)? {
            s.bus_dashboards_changed();
        }
        return Ok(None);
    };
    if s.count_other_dashboard_feeds(person, key)? >= MAX_FEEDS {
        return Err(invalid(format!(
            "{MAX_FEEDS} feeds are kept; write an empty value to one to remove it first"
        )));
    }
    let row = s.set_dashboard_feed(person, key, &value, by, by_ref, now)?;
    s.bus_dashboards_changed();
    Ok(Some(row))
}

/// `dashboards { feed, key, value }`: the caller's feed, written by the
/// person (`is_person`) or Control's agent.
pub fn feed(
    store: &Mutex<Store>,
    scope: &ViewScope,
    args: &DashboardsArgs,
    is_person: bool,
) -> Result<Option<DashboardFeedRow>, IpcError> {
    let key = feed_key(args.key.as_deref())?;
    let value = args
        .value
        .as_deref()
        .ok_or_else(|| invalid("feed needs value (empty removes the feed)"))?;
    let s = lock(store)?;
    let by = if is_person { "person" } else { "agent" };
    let own = own_person(&s, scope)?;
    write_feed(&s, own, key, value, by, None, crate::store::now_unix())
}

/// The tag a routine's reply writes a feed with (its routine's prompt names
/// it: `<dashboard-feed`).
pub const FEED_TAG: &str = "dashboard-feed";

/// PURE: each `<dashboard-feed key="…">…</dashboard-feed>` block in a
/// reply, as (key, value), in order, by the notebooks' own parser
/// (`routines::memory::tagged_blocks`): a block with no key or a bad one,
/// an unclosed one, or one quoted as code (`` `<dashboard-feed>` ``) is
/// left out, and a block holding another opening tag starts at that one.
pub fn feed_blocks(text: &str) -> Vec<(String, String)> {
    crate::service::routines::memory::tagged_blocks(text, FEED_TAG, "key")
        .into_iter()
        .filter_map(|(key, body)| {
            let key = feed_key(key).ok()?;
            Some((key.to_string(), body.trim().to_string()))
        })
        .collect()
}

/// `is_person`: the caller is a person's device, not Control's agent. The
/// agent pins (a change the person asked it for, which the page offers to
/// undo) but never unpins or undoes.
pub fn run(
    store: &Mutex<Store>,
    scope: &ViewScope,
    args: &DashboardsArgs,
    device: Option<&str>,
    is_person: bool,
) -> Result<serde_json::Value, IpcError> {
    let person_only = || -> Result<(), IpcError> {
        if is_person {
            Ok(())
        } else {
            Err(IpcError::new(
                codes::E_FORBIDDEN,
                "a person unpins a dashboard or undoes a change; pin the changed block instead",
            ))
        }
    };
    match args.action.as_str() {
        "list" => to_json(&list(store, scope)?),
        "get" => to_json(&get(store, scope, id_of(args)?)?),
        "pin" => to_json(&pin(store, scope, args, device)?),
        "unpin" => {
            person_only()?;
            Ok(serde_json::json!({ "removed": unpin(store, scope, id_of(args)?)? }))
        }
        "undo" => {
            person_only()?;
            undo(store, scope, id_of(args)?)
        }
        "feeds" => to_json(&feeds(store, scope)?),
        "feed" => to_json(&feed(store, scope, args, is_person)?),
        other => Err(invalid(format!(
            "action must be list | get | pin | unpin | undo | feeds | feed, got {other:?}"
        ))),
    }
}

fn to_json<T: Serialize>(v: &T) -> Result<serde_json::Value, IpcError> {
    serde_json::to_value(v).map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))
}

#[cfg(test)]
#[path = "dashboards_tests.rs"]
mod tests;
