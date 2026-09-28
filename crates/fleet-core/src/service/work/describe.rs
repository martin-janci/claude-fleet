//! `work { action: describe }`: one item's WHOLE description, on demand.
//!
//! * **Never on the sync path.** Sync keeps writing the 2000-char excerpt;
//!   this is a separate read, so no frame grows and the replay ring is
//!   untouched (the reason D18 could accept the first-sync flood).
//! * **Cache with a TTL** (`work.describe_cache_secs`): a warm entry is
//!   served without asking the tracker. The cache lives in its own table
//!   (migration 068), so the excerpt stays authoritative for every other
//!   path — a read path has exactly one source for "the" description.
//! * **Scope.** Exactly `card`'s fence: a per-host token describes only work
//!   its own host does inside its org; anything else answers as an unknown
//!   key ([`crate::service::orgs::not_visible_to`] — `E_FORBIDDEN` for a
//!   host token, `E_NOTFOUND` for a bound client, never revealing that
//!   another org's item exists).
//! * **The notice still applies.** A warm cache does not stop `lookup` or a
//!   start brief saying they cut: that notice is about THEIR budget (the
//!   2000-char excerpt vs. the tracker's true length), not about what fleet
//!   happens to hold in this cache. Neither path reads this cache at all —
//!   see `no_projection_carries_a_full_description` in
//!   `crate::mcp::tools::tests`.
//! * **Never across an `.await`.** The store guard is dropped before the
//!   provider call and re-taken to write the cache — the same discipline
//!   `sync::fetch_one` uses.
//! * **Retention.** Swept by `service::work::retention` with the tracker
//!   items, at a fixed floor of [`DESCRIBE_CACHE_RETENTION_FLOOR_DAYS`] when
//!   `work.retention.tracker_items_days` is `0` — that setting's usual
//!   "keep forever" must not apply to a full-text cache, or
//!   `DESCRIPTION_MAX_CHARS` would be reopened by the back door.

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::{self, OrgScope};
use crate::service::settings;
use crate::service::trackers::{self, tickets, ItemRef, TrackerNet};
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// The describe cache's retention floor (see the module doc): when
/// `work.retention.tracker_items_days` is `0`, `service::work::retention`
/// sweeps this cache at this fixed window instead of keeping it forever.
pub const DESCRIBE_CACHE_RETENTION_FLOOR_DAYS: i64 = 30;

/// `work { action: describe, key }`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Described {
    /// The tracker's own key, when it has one (else the reference asked).
    pub key: String,
    /// The whole description, third-party text, NOT fenced: the caller (the
    /// `work` tool) is responsible for fencing it the way `lookup` fences
    /// its excerpt, capped at [`trackers::DESCRIBE_MAX_CHARS`].
    pub body: String,
    /// `body`'s length. Equal to the tracker's true length on a fresh fetch
    /// (before fleet's own cap); on a cache hit, the length of what was
    /// stored (already capped when it was fetched).
    pub chars: i64,
    /// Served from the cache rather than asking the tracker.
    #[serde(default)]
    pub from_cache: bool,
}

fn unsupported(key: &str) -> IpcError {
    IpcError::new(
        codes::E_UNSUPPORTED,
        format!("{key}'s tracker does not serve full descriptions; open the ticket"),
    )
}

/// `work { action: describe, key }`: the cache, else one live fetch, cached
/// for `work.describe_cache_secs`. `net` is the caller's tracker network
/// (`trackers::default_net()` in production, a fake in tests) — describe
/// needs one exactly like `lookup` does, so it takes the same parameter.
pub async fn describe(
    store: &Mutex<Store>,
    scope: &OrgScope,
    key: &str,
    net: &TrackerNet,
) -> Result<Described, IpcError> {
    let key = crate::store::normalize_work_ref(key)?;
    // Resolve the key to an item inside `scope`: exactly the fence
    // `service::work::card::card` uses (its own doc explains each check).
    let (item_id, out_key, item_key, external_id, tracker, ttl_secs) = {
        let s = lock(store)?;
        orgs::require_key(&s, scope, &key)?;
        let Some(item) = s.work_item_by_key(&key)? else {
            return Err(orgs::not_visible_to(scope, &key));
        };
        let org_id = s.item_org(item.id)?;
        if let Some(allowed) = tickets::allowed(scope, &s)? {
            if !allowed.contains(&item.id)
                && (item.tracker_id.is_some() || !scope.sees_org(org_id))
            {
                return Err(orgs::not_visible_to(scope, &key));
            }
        }
        let tracker = s
            .list_trackers()?
            .into_iter()
            .find(|t| Some(t.id) == item.tracker_id);
        let out_key = item.key.clone().unwrap_or_else(|| key.clone());
        let ttl_secs = settings::get_secs(&s, settings::WORK_DESCRIBE_CACHE_SECS) as i64;
        (
            item.id,
            out_key,
            item.key.clone(),
            item.external_id.clone(),
            tracker,
            ttl_secs,
        )
    };

    let now = crate::service::catalog::now_secs();
    if let Some(body) = lock(store)?.cached_description(item_id, ttl_secs, now)? {
        let chars = body.chars().count() as i64;
        return Ok(Described {
            key: out_key,
            body,
            chars,
            from_cache: true,
        });
    }

    // No tracker (a local item, work graph M14) or one whose provider does
    // not implement `describe`: the same honest refusal `describe_offer`
    // gives a caller who was never offered the action in the first place —
    // never an empty success.
    let Some(t) = tracker.filter(|t| trackers::provider_caps(t).describe) else {
        return Err(unsupported(&out_key));
    };
    let cred = lock(store)?.resolve_tracker_credential(t.id)?;
    let provider = trackers::provider_for(&t, cred, net).map_err(|e| e.to_ipc())?;
    let item_ref = match item_key {
        Some(k) => ItemRef::parse(&k),
        None => ItemRef::Id(external_id.unwrap_or_default()),
    };
    let Some(body) = provider.describe(&item_ref).await.map_err(|e| e.to_ipc())? else {
        return Err(unsupported(&out_key));
    };
    let chars = body.chars().count() as i64;
    lock(store)?.put_description(item_id, &body, chars)?;
    Ok(Described {
        key: out_key,
        body,
        chars,
        from_cache: false,
    })
}

#[cfg(test)]
mod tests;
