//! `work { action: describe }`: one item's WHOLE description, on demand.
//!
//! * **Never on the sync path.** Sync keeps writing the 2000-char excerpt;
//!   this is a separate read, so no frame grows and the replay ring is
//!   untouched (the reason D18 could accept the first-sync flood).
//! * **Capability first, cache second.** The item, its tracker and that
//!   tracker's `describe` capability are resolved BEFORE the cache is read,
//!   so a tracker a person disconnected cannot keep being served its own
//!   ticket text (`Store::remove_tracker` clears the cache for its items in
//!   the same transaction, and with the tracker row gone this refuses
//!   anyway).
//! * **Cache with a TTL** (`work.describe_cache_secs`, clamped to the
//!   retention window — see `ttl_ceiling_secs`): a warm entry is
//!   served without asking the tracker. The cache lives in its own table
//!   (migration 073), so the excerpt stays authoritative for every other
//!   path — a read path has exactly one source for "the" description. The
//!   TTL window is inclusive: an entry fetched exactly `ttl_secs` ago is
//!   still served (`Store::cached_description`'s own doc says so too).
//! * **Scope.** Exactly `card`'s fence: a per-host token describes only work
//!   its own host does inside its org; anything else answers as an unknown
//!   key ([`crate::service::orgs::not_visible_to`] — `E_FORBIDDEN` for a
//!   host token, `E_NOTFOUND` for a bound client, never revealing that
//!   another org's item exists).
//! * **Fenced for an agent, here, not by the caller.** Third-party text: a
//!   per-host token (`OrgScope::Host`) gets `body` fenced exactly the way
//!   `lookup` fences its excerpt (`fence_ticket`, capped at
//!   [`trackers::DESCRIBE_MAX_CHARS`]) — this function returns the value, so
//!   this function discharges the fencing duty, the same rule `lookup`
//!   (`tickets::lookup`) and `card` (`card::card`) follow for their own
//!   answers. The notice fires only on fleet's OWN cap: `full_chars` is the
//!   tracker's true length (the provider's [`trackers::FullDescription`]
//!   `chars`, kept in the cache's `chars` column), so a description past
//!   [`trackers::DESCRIBE_MAX_CHARS`] says "shown N of M … open the ticket"
//!   (`DescribeOffer::None`: asking `describe` again would not get more).
//!   A person (master, a phone, bound or not) gets it plain, as `lookup`
//!   leaves its excerpt, with [`Described::chars`] above the body's length
//!   to say it was cut.
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
//! * **Parked, on purpose.** No single-flight for two concurrent misses on
//!   the same key (each would fetch and write once, harmlessly redundantly —
//!   not a correctness bug, just a wasted tracker call under a rare race).

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
    /// The whole description, third-party text. For a per-host token
    /// (`OrgScope::Host`) already fenced exactly as `lookup` fences its
    /// excerpt — markers defused, wrapped in the untrusted-input marker,
    /// capped at [`trackers::DESCRIBE_MAX_CHARS`] — with a trailing "shown
    /// N of M … open the ticket" notice only when that cap cut it.
    /// For a person (master, a phone, bound or not) this is the plain body,
    /// exactly as `lookup` leaves its excerpt.
    pub body: String,
    /// The description's length AT THE TRACKER, before fleet's own
    /// [`trackers::DESCRIBE_MAX_CHARS`] cap — on a fresh fetch and on a
    /// cache hit alike (the cache keeps the length the fetch reported).
    /// Above the text in `body` exactly when that cap cut it.
    pub chars: i64,
    /// Served from the cache rather than asking the tracker.
    #[serde(default)]
    pub from_cache: bool,
}

/// This item has no tracker fleet can ask (a local item, work graph M14), or
/// its provider does not implement `describe` at all: the same honest refusal
/// [`tickets::describe_offer`] gives a caller who was never offered the action
/// — never an empty success.
fn unsupported(key: &str) -> IpcError {
    IpcError::new(
        codes::E_UNSUPPORTED,
        format!("{key}'s tracker does not serve full descriptions; open the ticket"),
    )
}

/// The provider CAN serve descriptions and answered that this ticket has no
/// description text (an ADF document that renders empty, a `null` field). A
/// different answer from [`unsupported`] on purpose: "this tracker does not
/// serve full descriptions" is simply false of a Jira ticket, and an agent a
/// truncation notice has just sent here would read it as fleet contradicting
/// itself. The operation is supported; this ticket's state is what is empty.
fn no_description(key: &str) -> IpcError {
    IpcError::new(
        codes::E_INVALID_STATE,
        format!("{key} has no description text at its tracker"),
    )
}

/// The longest a cached description may be SERVED: the window the retention
/// pass actually keeps it for (`work.retention.tracker_items_days`, floored
/// by [`DESCRIBE_CACHE_RETENTION_FLOOR_DAYS`] — the same
/// `retention::describe_effective_days` `work_admin { status }` reports and
/// the sweep deletes by, so there is one definition of the window and not a
/// third).
///
/// `work.describe_cache_secs` is a `Kind::Secs` setting: the desktop input
/// suggests a day, but the registry accepts up to ten years and a hub
/// operator's `set_setting` is not bound by the input. Unclamped, the TTL
/// would promise to serve an entry the sweep deleted months ago, and would
/// hand an agent months-old requirements behind a notice that was current —
/// the same kind of defect as a notice that cries wolf.
fn ttl_ceiling_secs(s: &Store) -> i64 {
    use crate::service::work::retention::{describe_effective_days, RetentionDays};
    describe_effective_days(RetentionDays::from_store(s).tracker_items) * 86_400
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
        // The first item carrying `key` that this caller may see. Two
        // trackers can hold the same key (two Jira sites, one per org): the
        // store's single "the item for this key" would pick the oldest and
        // refuse a host of the other org over a ticket that is not theirs,
        // when their own is right there.
        let allowed = tickets::allowed(scope, &s)?;
        let mut item = None;
        for candidate in s.work_items_by_key(&key)? {
            let visible = match &allowed {
                None => true,
                Some(allowed) => {
                    allowed.contains(&candidate.id)
                        || (candidate.tracker_id.is_none()
                            && scope.sees_org(s.item_org(candidate.id)?))
                }
            };
            if visible {
                item = Some(candidate);
                break;
            }
        }
        let Some(item) = item else {
            return Err(orgs::not_visible_to(scope, &key));
        };
        let tracker = s
            .list_trackers()?
            .into_iter()
            .find(|t| Some(t.id) == item.tracker_id);
        let out_key = item.key.clone().unwrap_or_else(|| key.clone());
        // How long an entry may be served: the setting, never longer than
        // the window the retention pass keeps it for (see
        // [`ttl_ceiling_secs`]).
        let ttl_secs = (settings::get_secs(&s, settings::WORK_DESCRIBE_CACHE_SECS) as i64)
            .min(ttl_ceiling_secs(&s));
        (
            item.id,
            out_key,
            item.key.clone(),
            item.external_id.clone(),
            tracker,
            ttl_secs,
        )
    };

    // BEFORE the cache, deliberately. A tracker a person disconnected is gone
    // from `list_trackers`, so `tracker` is `None` here and this refuses —
    // and `Store::remove_tracker` clears its items' cached descriptions in
    // the same transaction as its secret. Reading the cache first made a
    // per-host token keep receiving a deleted tracker's full ticket text for
    // the whole TTL: a read that outlived the revocation gesture a person had
    // just performed, and (the TTL being a `Kind::Secs` setting) for as long
    // as an operator had set that TTL to.
    let Some(t) = tracker.filter(|t| trackers::provider_caps(t).describe) else {
        return Err(unsupported(&out_key));
    };

    let now = crate::service::catalog::now_secs();
    // WARNING for future edits: `lock(store)?` here is a temporary of this
    // `if let`'s SCRUTINEE, and for `if let` / `match` (unlike a plain `let`
    // statement) such a temporary's drop is deferred to the end of the whole
    // arm — so the store stays locked for this entire block, not just for
    // evaluating `cached_description`. Do not add a second `lock(store)?`
    // anywhere inside this block: it would try to re-acquire this same,
    // still-held, non-reentrant `std::sync::Mutex` on one thread and hang
    // forever — the exact shape of bug that deadlocked
    // `service::work::retention::status` (two `lock`s as sibling struct-field
    // initializers of one statement) before it was caught. Do not add an
    // `.await` inside this block either: it would hold the guard across an
    // await, which this module's own discipline (see the module doc) forbids.
    // Both are safe once this block has returned or fallen through.
    if let Some((body, chars)) =
        lock(store)?.cached_description_with_chars(item_id, ttl_secs, now)?
    {
        // A row written before the length was the tracker's own holds the
        // body's length; never report less than what is served.
        let chars = chars.max(body.chars().count() as i64);
        return Ok(Described {
            key: out_key,
            body: fence_for_scope(scope, &body, chars),
            chars,
            from_cache: true,
        });
    }

    let cred = lock(store)?.resolve_tracker_credential(t.id)?;
    let provider = trackers::provider_for(&t, cred, net).map_err(|e| e.to_ipc())?;
    let item_ref = match item_key {
        Some(k) => ItemRef::parse(&k),
        None => ItemRef::Id(external_id.unwrap_or_default()),
    };
    // The provider serves descriptions (checked above), so `None` here means
    // this ticket has no description text — not an unsupported operation.
    let Some(full) = provider.describe(&item_ref).await.map_err(|e| e.to_ipc())? else {
        return Err(no_description(&out_key));
    };
    let chars = full.chars.max(full.text.chars().count() as i64);
    lock(store)?.put_description(item_id, &full.text, chars)?;
    Ok(Described {
        key: out_key,
        body: fence_for_scope(scope, &full.text, chars),
        chars,
        from_cache: false,
    })
}

/// Fence `body` for a per-host token exactly as `lookup` fences its excerpt
/// (`fence_ticket`, third-party text, capped at
/// [`trackers::DESCRIBE_MAX_CHARS`]); a person (master, a phone, bound or
/// not) reads it plain, as `lookup` leaves its excerpt for the same scopes.
/// `full_chars` is the tracker's true length, so a description fleet's own
/// cap cut carries "shown N of M chars … open the ticket"
/// ([`crate::mcp::guard::DescribeOffer::None`]: `describe` is already the
/// most fleet serves); an uncut one gets no notice.
fn fence_for_scope(scope: &OrgScope, body: &str, full_chars: i64) -> String {
    match scope {
        OrgScope::Host { .. } => crate::mcp::guard::fence_ticket(
            body,
            "the tracker ticket's full description",
            trackers::DESCRIBE_MAX_CHARS,
            Some(full_chars),
            crate::mcp::guard::DescribeOffer::None,
        ),
        OrgScope::All | OrgScope::Org { .. } => body.to_string(),
    }
}

#[cfg(test)]
mod tests;
