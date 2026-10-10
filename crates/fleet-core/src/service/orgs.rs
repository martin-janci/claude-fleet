//! Organisations (work graph M5): the scope every work read and write runs
//! under, the zero-config scope list, suggestions, and the admin actions.
//!
//! **One scope, two strengths** (plan, decision 1). [`OrgScope::All`] is the
//! master, a paired client and the desktop: every org, and the org is only a
//! view filter (the UI's selector). [`OrgScope::Host`] is a per-host token —
//! every in-session Claude — and there the org is a BOUNDARY:
//!
//! * work data (items, links, journal and handover text, trackers,
//!   `SessionRow.work` / `work_suggested` / `work_rejected`) is visible only
//!   when its org is the host's or unassigned; a host with no org sees
//!   unassigned work only ([`OrgScope::sees_org`]);
//! * sessions are fenced too, but only between orgs that turned
//!   `isolate_sessions` on (decision D7, default off) —
//!   [`OrgScope::sees_session_org_only`]. Since multi-user M1 that is only
//!   the ORG half of a session read; the person half lives in
//!   [`crate::service::view_scope::ViewScope`], which wraps this type.
//!
//! The scope is computed in exactly one place, `Caller::org_scope` (MCP), and
//! is `All` for every Tauri command (the desktop is the master; a paired
//! desktop reaches the hub as a client). Service functions take it and filter
//! with the predicates here; nothing at a call site decides visibility.
//!
//! **No existence oracle.** Something out of scope named by id answers exactly
//! as an id that does not exist ([`not_found`]); by key or URL, exactly as a
//! key nothing is linked to on the host ([`not_visible_key`]).

use crate::ipc_error::{codes, lock, IpcError};
use crate::store::{OrgRow, OrgRuleRow, SessionRow, Store, WorkItemRow, WorkLinkRow};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

/// Bumped by every org administration change. A long-lived reader of a
/// scope (an `/events` stream) compares it on each frame and re-reads its
/// scope when it moved, so a host moved to another org is fenced from the
/// very next frame, not the next keep-alive beat.
static ORG_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// See [`ORG_GENERATION`].
pub fn org_generation() -> u64 {
    ORG_GENERATION.load(std::sync::atomic::Ordering::SeqCst)
}

/// Who is asking, for every work read and write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrgScope {
    /// Master, an unbound paired client, the desktop: everything.
    All,
    /// A paired client bound to one org (work graph M14): that org's work,
    /// and — strictly, whatever `isolate_sessions` says — only that org's
    /// sessions; unassigned work and sessions too while the org's
    /// `bound_sees_unassigned` is on (D31, the default). The client asked
    /// to be restricted, so nothing of another org reaches it. There is no
    /// host fence: a phone is not a host.
    Org {
        /// The bound org. An org that was deleted since still binds: the
        /// client then reads nothing of any org, and no unassigned data
        /// either (fail closed, never widened to `All`).
        org: i64,
        /// D31: `orgs.bound_sees_unassigned`, read with the scope. `false`
        /// for an org that no longer exists.
        sees_unassigned: bool,
    },
    /// A per-host token.
    Host {
        alias: String,
        /// The host's org; `None` sees unassigned data only.
        org: Option<i64>,
        /// Orgs with `isolate_sessions` on (D7).
        ///
        /// **No longer consulted for session visibility** (multi-user M1,
        /// plan T6): [`OrgScope::sees_session_org_only`]'s host arm is now
        /// "its own host's rows, and nothing else", which hides strictly
        /// more than D7 ever did for a per-host token — every other host's
        /// rows, isolating org or not. The set is still read with the scope
        /// (and still administered through `work_admin`) because it is the
        /// recorded intent of an org, and M2's memberships are where a
        /// reader for it comes back; nothing in M1 may widen a caller on
        /// the strength of it.
        isolated: BTreeSet<i64>,
    },
}

impl OrgScope {
    /// The scope of a per-host token for `alias`, read now.
    pub fn for_host(s: &Store, alias: &str) -> Result<Self, IpcError> {
        Ok(OrgScope::Host {
            alias: alias.to_string(),
            org: s.host_org(alias)?,
            isolated: s.isolated_orgs()?,
        })
    }

    /// The scope of a paired client bound to `org` (work graph M14), with
    /// D31's switch read now, so a change applies from the client's next
    /// call on.
    pub(crate) fn for_client(s: &Store, org: i64) -> Result<Self, IpcError> {
        Ok(OrgScope::Org {
            org,
            sees_unassigned: s.get_org(org)?.is_some_and(|o| o.bound_sees_unassigned),
        })
    }

    pub fn is_all(&self) -> bool {
        // This is the org boundary, not a privacy fence — and this is where
        // the whole class comes from: `All` is "no ORG fence", which is NOT
        // "no fence". The master, a person's own device and a paired client
        // bound to no org all resolve to it, so a privacy question asked of
        // this predicate is asked of nobody. The person half is
        // `ViewScope`, and `scope_guard_tests` classifies every caller that
        // reads this one.
        matches!(self, OrgScope::All)
    }

    /// The host a per-host token is bound to.
    pub fn host(&self) -> Option<&str> {
        match self {
            // This is the org boundary, not a privacy fence: it reports the
            // scope's own HOST BINDING, which only a per-host token has. An
            // accessor, not a decision about anybody's data.
            OrgScope::All | OrgScope::Org { .. } => None,
            OrgScope::Host { alias, .. } => Some(alias),
        }
    }

    /// The org a bound client is fenced to (work graph M14).
    pub fn bound_org(&self) -> Option<i64> {
        match self {
            OrgScope::Org { org, .. } => Some(*org),
            _ => None,
        }
    }

    /// Work data of `org` is visible: always for `All`; for a host, its own
    /// org's and unassigned data; for a bound client, its own org's, and
    /// unassigned data while its org's `bound_sees_unassigned` is on (D31).
    pub fn sees_org(&self, org: Option<i64>) -> bool {
        match self {
            // This is the org boundary, not a privacy fence: it is the org
            // boundary itself, asked of an ORG id and nothing else. Every
            // session answer that composes it also takes the person half
            // (`ViewScope::sees_session_row`).
            OrgScope::All => true,
            OrgScope::Host { org: mine, .. } => org.is_none() || org == *mine,
            OrgScope::Org {
                org: mine,
                sees_unassigned,
            } => match org {
                None => *sees_unassigned,
                Some(o) => o == *mine,
            },
        }
    }

    /// A link (its `org_id` resolved by `Store::fill_link_orgs`).
    pub fn sees_link(&self, l: &WorkLinkRow) -> bool {
        self.sees_org(l.org_id)
    }

    /// The ORG half of "may this caller see that session" — and, since
    /// multi-user M1, only the org half. The person half is
    /// [`crate::service::view_scope::ViewScope::sees_session_row`], which
    /// composes this with ownership, grants and the pane proof; the name
    /// says `_org_only` so that no call site can go on believing this one
    /// answers the whole question (plan T6; T10 removes it outright).
    ///
    /// * `All`: everything, as before.
    /// * `Org` (a bound client, M14): strict — another org's session never
    ///   reaches it, isolated or not; an unassigned one only under D31.
    /// * `Host` (a per-host token): **its own host's rows, and nothing
    ///   else.**
    ///
    /// The host arm was re-derived rather than trimmed clause by clause.
    /// It used to read `row_host == alias || row_org.is_none() || row_org ==
    /// *org`, with a permissive `!(theirs || mine)` fall-through under it —
    /// three unconditional wins, the widest of which let a host token in
    /// org X read every session of org X on **every** host in the fleet.
    /// Deleting one or two of them would have left the arm closer to `All`
    /// than to the rule a host token is supposed to have, so the rule is
    /// written out instead: a machine's token speaks for that machine.
    ///
    /// D7 (`isolate_sessions`) is subsumed rather than dropped: its whole
    /// effect for a host token was to hide OTHER hosts' rows, and those are
    /// now hidden unconditionally. The flag still fences a stream
    /// (`mcp/events_route.rs`) and still means what it meant for everyone
    /// else.
    pub fn sees_session_org_only(&self, row_host: &str, row_org: Option<i64>) -> bool {
        match self {
            // This is the org boundary, not a privacy fence, and the `_org_only`
            // in the name is the whole contract: this answers "may this
            // caller read this company's rows", and the PERSON half is
            // `ViewScope::sees_session_row` / `sees_session_facts`, which
            // every caller-facing path applies next to it. Called straight
            // only where there is no person to ask about.
            OrgScope::All => true,
            OrgScope::Org { .. } => self.sees_org(row_org),
            OrgScope::Host { alias, .. } => row_host == alias,
        }
    }

    /// [`Self::sees_session_org_only`] over a row — the same half-answer in
    /// the shape most call sites have. The person half is
    /// [`crate::service::view_scope::ViewScope::sees_session_row`], and every
    /// production CALLER of this is a row in `scope_guard_tests`'
    /// `ORG_HALF_SITES`, which names where that half runs for each one.
    pub fn sees_row_org_only(&self, row: &SessionRow) -> bool {
        self.sees_session_org_only(&row.host_alias, row.org_id)
    }

    /// Take out of a session row the work data this scope may not read: all
    /// of it for a session outside the scope's orgs, else a link (primary or
    /// suggestion) whose own org is outside. `work_rejected` — bare keys
    /// with no org of their own, which only the sidebar's fallback
    /// recognition reads — and `work_rev` (a digest over every link) never
    /// reach a scoped caller.
    pub fn redact_row_org_only(&self, row: &mut SessionRow) {
        // This is the org boundary, not a privacy fence: this function answers half the
        // question, and its name says which half.
        if self.is_all() {
            return;
        }
        row.work_rejected.clear();
        // Another org's hidden link would move it (M14).
        row.work_rev = 0;
        if !self.sees_org(row.org_id) {
            row.work = None;
            row.work_suggested = None;
            row.work_rejected.clear();
            return;
        }
        if row.work.as_ref().is_some_and(|w| !self.sees_org(w.org_id)) {
            row.work = None;
        }
        if row
            .work_suggested
            .as_ref()
            .is_some_and(|w| !self.sees_org(w.org_id))
        {
            row.work_suggested = None;
        }
    }

    /// [`Self::redact_row_org_only`] over serialised output: every JSON object that
    /// is a session row (it has `tmux_name` and a work field) anywhere in
    /// `v`. `session_org` answers a row object's org — the MCP gate looks it
    /// up by `id` (a projection may have dropped `org_id`); an event frame
    /// carries the whole row, so its own `org_id` is read.
    pub fn redact_json(
        &self,
        v: &mut serde_json::Value,
        session_org: &dyn Fn(&serde_json::Map<String, serde_json::Value>) -> Option<i64>,
    ) {
        // This is the org boundary, not a privacy fence: as for `redact_row_org_only` above: the
        // org half, by name.
        if self.is_all() {
            return;
        }
        match v {
            serde_json::Value::Array(items) => {
                for i in items {
                    self.redact_json(i, session_org);
                }
            }
            serde_json::Value::Object(map) => {
                let is_row = map.contains_key("tmux_name")
                    && WORK_FIELDS.iter().any(|k| map.contains_key(*k));
                if is_row {
                    map.remove("work_rejected");
                    map.remove("work_rev");
                    let org = session_org(map);
                    if !self.sees_org(org) {
                        for k in WORK_FIELDS {
                            map.remove(*k);
                        }
                    } else {
                        for k in ["work", "work_suggested"] {
                            let link_org = map
                                .get(k)
                                .and_then(|w| w.get("org_id"))
                                .and_then(serde_json::Value::as_i64)
                                // A link without its own org is the session's.
                                .or(org);
                            let present = map.get(k).is_some_and(|w| !w.is_null());
                            if present && !self.sees_org(link_org) {
                                map.remove(k);
                            }
                        }
                    }
                }
                for (_, child) in map.iter_mut() {
                    self.redact_json(child, session_org);
                }
            }
            _ => {}
        }
    }
}

/// The `SessionRow` fields that are work data.
pub const WORK_FIELDS: &[&str] = &["work", "work_suggested", "work_rejected", "work_rev"];

/// What an id outside the scope answers: the words an unknown id gets.
pub fn not_found(what: &str, id: i64) -> IpcError {
    IpcError::new(codes::E_NOTFOUND, format!("{what} {id} not found"))
}

/// What a key or URL outside a host's scope answers, whether or not
/// anything by that name exists.
pub fn not_visible_key(host: &str, key: &str) -> IpcError {
    IpcError::new(
        codes::E_FORBIDDEN,
        format!(
            "{key} is not visible to host {host}: a per-host token reads only the work its \
             own host's sessions do, within the host's organisation"
        ),
    )
}

/// What a key or URL outside `scope` answers: a host's sentence for a
/// per-host token ([`not_visible_key`]); for a bound client (work graph M14)
/// exactly what a key no connected tracker has answers.
pub fn not_visible_to(scope: &OrgScope, key: &str) -> IpcError {
    match scope.host() {
        Some(h) => not_visible_key(h, key),
        None => IpcError::new(
            codes::E_NOTFOUND,
            format!("{key} is not a ticket of any connected tracker"),
        ),
    }
}

/// The data-integrity rule (plan §M5.3, for every caller, master too): a
/// link between a session of one org and work of another is refused unless
/// `force_cross_org` — it stops Company B's ticket from being attached to a
/// Company A session by mistake. Unassigned on either side is never a
/// conflict.
pub fn check_cross_org(
    work_org: Option<i64>,
    session_org: Option<i64>,
    what: &str,
    force: bool,
) -> Result<(), IpcError> {
    match (work_org, session_org) {
        (Some(w), Some(sess)) if w != sess && !force => Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!(
                "{what} belongs to organisation {w} and the session to organisation {sess}; \
                 fleet does not link work across organisations by mistake — pass \
                 force_cross_org: true if this is meant"
            ),
        )
        .with_details(serde_json::json!({
            "work_org_id": w, "session_org_id": sess, "cross_org": true
        }))),
        _ => Ok(()),
    }
}

/// Resolve the links' orgs and keep what the scope may read: for a per-host
/// token, links inside its orgs, and ended (past) links only of its own
/// host's sessions (M2's fence, kept: the org alone is wider). For a bound
/// client (M14), links inside its orgs whose session it may see too — a
/// forced cross-org link of its org's task on another org's session names
/// that session, which a bound client never sees.
pub fn scope_links(
    s: &Store,
    scope: &OrgScope,
    links: &mut Vec<WorkLinkRow>,
) -> Result<(), IpcError> {
    s.fill_link_orgs(links)?;
    match scope {
        // This is the org boundary, not a privacy fence: `scope_links` is
        // the ORG-only link pager, as its callers' own names say. The person
        // half is `scope_links_for`, which composes this with
        // `link_person_visible` — and the classified `link_session_visible`
        // guard below is this function's child. Written as
        // `if scope.is_all() { return Ok(()); }` it would have needed a row
        // from the first round; it was spelled as a match arm, which is why
        // the scan now reads the discriminant too (T9d).
        OrgScope::All => {}
        OrgScope::Host { alias, .. } => links.retain(|l| {
            scope.sees_link(l) && (l.ended_at.is_none() || l.snap_host.as_deref() == Some(alias))
        }),
        OrgScope::Org { .. } => {
            let mut keep = Vec::with_capacity(links.len());
            for l in links.drain(..) {
                if scope.sees_link(&l) && link_session_visible(s, scope, &l)? {
                    keep.push(l);
                }
            }
            *links = keep;
        }
    }
    Ok(())
}

/// May `view`'s PERSON see the session behind link `l` (multi-user M1, T8d)?
///
/// The person half of [`link_session_visible`], and a separate function for
/// the reason the whole of `ViewScope` is a separate type: the org half's
/// first line is `scope.is_all()`, which is TRUE for the master AND for every
/// paired client bound to no org — i.e. for every ordinary person's own phone
/// or laptop, the caller M1 exists to fence. A `work { links }` page was
/// filtered by the org half alone and therefore by nothing at all for such a
/// caller, which handed a second person's readonly device `snap_tmux`,
/// `snap_name`, `snap_branch`, `snap_worktree`, `snap_pr_url` and
/// `snap_claude_ids` for every recently ended link on the hub.
///
/// Three arms, in this order, and they are the three
/// [`ViewScope::sees_past_conversation`] asks, for the same reason — two
/// mechanisms answering one question must not disagree:
///
/// 1. **A live participant decides**, through
///    [`ViewScope::sees_session_row`]. This covers a live link and a link
///    that ended on a session still running, the two cases where there is a
///    row to judge. A participant naming a row that is GONE is refused
///    rather than falling through to the snapshot arm: the snapshot is what
///    the fence exists to withhold, so it must never stand in for the row
///    (the same discipline `Graph::link_visible` applies to
///    `hidden_sessions`).
/// 2. **Else the conversations the link ran in decide.** An ended link whose
///    session was reaped is all snapshot, and the only durable handle on
///    whose work it was is `conversation_owners`, keyed by the conversation
///    ids the link itself recorded (`claude_session_id` and the
///    `snap_claude_ids` array). EVERY recorded id must be this caller's — a
///    link that ran two conversations, one of them somebody else's, is
///    somebody else's work.
/// 3. **Else nothing is recorded at all**: no live participant and not one
///    conversation id. There is no record to judge, and the answer is rule
///    7's and nothing wider (multi-user M1, T9c) —
///    [`ViewScope::is_sole_person`] or a per-host token passes, every other
///    caller is refused.
///
///    It used to pass for everybody, on the reading that such a link can
///    only be a pre-M1 one whose row was reaped long ago. That reading was
///    wrong, and the loop below is why: with `session_id == None` and no
///    conversation id the `for` body never runs, so the function fell
///    through to `Ok(true)` — fail-OPEN, for the one shape it has nothing to
///    judge. The shape is reachable on a fleet running M1 today: migration
///    046's retire trigger fills `snap_claude_ids` only
///    `HAVING COUNT(*) > 0`, and `work_links.claude_session_id` is NULL both
///    for a session that never had one (a `new_shell_session` row, a Claude
///    session reaped before its first SessionStart hook) and for
///    `Store::link_session_work_at`'s insert, which never sets the column.
///    So an ordinary private session, linked to work and then reaped, handed
///    every person in the org its `snap_host` / `snap_tmux` / `snap_name` /
///    `snap_branch` / `snap_pr_url` through `work { links }`, `today`,
///    `tree` and `resume_plan`.
///
///    Rule 7 is kept where it is actually about rule 7:
///    [`ViewScope::is_sole_person`] is the predicate M1 already uses for
///    "this install cannot tell two people apart, so nothing may narrow for
///    it", and it is false for a person-less caller and false the moment a
///    second person exists. A per-host token passes for the other reason —
///    §4.4 gives it a HOST's reach and no person dimension at all, and the
///    org/host fence it does answer to ([`scope_links`], M2's own-host
///    clause for an ended link) has already run.
///
/// A failed read is never a pass: the error propagates.
///
/// [`ViewScope::sees_past_conversation`]: crate::service::view_scope::ViewScope::sees_past_conversation
/// [`ViewScope::sees_session_row`]: crate::service::view_scope::ViewScope::sees_session_row
pub fn link_person_visible(
    s: &Store,
    view: &crate::service::view_scope::ViewScope,
    l: &WorkLinkRow,
) -> Result<bool, IpcError> {
    link_person_visible_at(s, view, l, s.link_session_id(l)?)
}

/// **[`link_person_visible`] with the link's live session already resolved —
/// the ONE body of the rule, and the entry point every page-shaped reader
/// takes** (multi-user M1, T9b).
///
/// `session_id` is the link's LIVE participant's session, i.e. exactly what
/// [`crate::store::Store::link_session_id`] answers and exactly what
/// `ViewLink.session_id` already holds (both are
/// `participants … AND retired_at IS NULL`). Taking it as an argument is the
/// point: the Work view's `Graph` has it in hand already, and before this
/// existed it wrote its own half of the rule — one that tested a LIVE id and
/// fell through to the snapshot for everything else, so every link of a
/// reaped session (`session_id IS NULL`) was judged by `OrgScope` alone and
/// served by its snapshot name. Two fences for one question is how the ENDED
/// half of a session's life came to be fenced differently from the LIVE half,
/// five separate ways; there is now one.
///
/// `seen` memoises [`crate::service::view_scope::ViewScope::sees_past_conversation`]
/// across the links of one page, so a page costs one lookup per DISTINCT
/// conversation rather than one per link. Pass a fresh map for a single link.
pub fn link_person_visible_at(
    s: &Store,
    view: &crate::service::view_scope::ViewScope,
    l: &WorkLinkRow,
    session_id: Option<i64>,
) -> Result<bool, IpcError> {
    link_person_visible_memo(s, view, l, session_id, &mut BTreeMap::new())
}

/// [`link_person_visible_at`] with the conversation memo supplied; see there.
pub fn link_person_visible_memo(
    s: &Store,
    view: &crate::service::view_scope::ViewScope,
    l: &WorkLinkRow,
    session_id: Option<i64>,
    seen: &mut BTreeMap<String, bool>,
) -> Result<bool, IpcError> {
    if view.is_internal() {
        return Ok(true);
    }
    if let Some(sid) = session_id {
        return Ok(match s.get_session_by_id(sid)? {
            Some(row) => view.sees_session_row(&row).is_visible(),
            None => false,
        });
    }
    let cids = link_conversations(l);
    // Arm 3, written as a branch rather than as the loop's fall-through: a
    // link with nothing recorded has no record to judge, and the `for` below
    // silently answering `true` for it is what made this fail open. See the
    // doc comment.
    if cids.is_empty() {
        return Ok(view.host.is_some() || view.is_sole_person());
    }
    for cid in cids {
        let ok = match seen.get(&cid) {
            Some(v) => *v,
            None => {
                let v = view.sees_past_conversation(s, &cid)?;
                seen.insert(cid, v);
                v
            }
        };
        if !ok {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Every Claude conversation id one link recorded: the one it was decided in
/// and the snapshot's whole array. Deduplicated, so a link that names the
/// same id twice costs one lookup.
pub(crate) fn link_conversations(l: &WorkLinkRow) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(c) = l.claude_session_id.clone() {
        out.push(c);
    }
    if let Some(ids) = l
        .snap_claude_ids
        .as_deref()
        .and_then(|j| serde_json::from_str::<Vec<String>>(j).ok())
    {
        out.extend(ids);
    }
    out.sort();
    out.dedup();
    out
}

/// [`scope_links`] with the caller's WHOLE scope: the org fence, then the
/// person fence ([`link_person_visible`]) on every link that survived it.
///
/// The one every READ of a link page takes. `scope_links` stays for the
/// writes, which name one link the caller already reached by id.
pub fn scope_links_for(
    s: &Store,
    view: &crate::service::view_scope::ViewScope,
    links: &mut Vec<WorkLinkRow>,
) -> Result<(), IpcError> {
    scope_links(s, &view.org, links)?;
    if view.is_internal() {
        return Ok(());
    }
    let mut seen: BTreeMap<String, bool> = BTreeMap::new();
    let mut keep = Vec::with_capacity(links.len());
    for l in links.drain(..) {
        let sid = s.link_session_id(&l)?;
        if link_person_visible_memo(s, view, &l, sid, &mut seen)? {
            keep.push(l);
        }
    }
    *links = keep;
    Ok(())
}

/// May `scope` see the session behind link `l`: the live session's row, or
/// for an ended link the snapshot's host and org. A link with no session
/// left to name (a swept participant, no snapshot) names nothing.
pub fn link_session_visible(
    s: &Store,
    scope: &OrgScope,
    l: &WorkLinkRow,
) -> Result<bool, IpcError> {
    // This is the org boundary, not a privacy fence: the org half of a link's session fence.
    // `link_person_visible` is the person half and `scope_links_for` composes the two; a caller
    // that reads a PAGE of links takes both.
    if scope.is_all() {
        return Ok(true);
    }
    // A live participant decides, also for a link that ended on a live
    // session (no snapshot yet).
    if let Some(sid) = s.link_session_id(l)? {
        return Ok(match s.get_session_by_id(sid)? {
            // The org half of a live participant; `link_person_visible` is
            // the person half and `scope_links_for` composes the two.
            Some(row) => scope.sees_row_org_only(&row),
            None => false,
        });
    }
    let (host, org) = s.link_snapshot_place(l.id)?;
    // The snapshot arm, org half again: `link_person_visible`'s third arm
    // judges a reaped participant's recorded conversations through
    // `sees_past_conversation`.
    Ok(scope.sees_session_org_only(host.as_deref().unwrap_or_default(), org))
}

/// The first item carrying `key` (in [`Store::work_items_by_key`]'s order)
/// that `scope` may read as a ticket: what `tickets::allowed` lets it read
/// ([`crate::service::trackers::tickets::item_visible`]), or a local item
/// (no tracker) whose org it sees. Two trackers can hold the same key (two
/// Jira sites, one per org, both with `PAY`): the store's single "the item
/// for this key" is the oldest, and would refuse a caller of the other org
/// over a ticket that is not theirs when their own is right there. For
/// [`OrgScope::All`] this is [`Store::work_item_by_key`].
pub fn visible_item_for_key(
    s: &Store,
    scope: &OrgScope,
    key: &str,
) -> Result<Option<WorkItemRow>, IpcError> {
    // This is the org boundary, not a privacy fence: it asks which ORG's item a shared key
    // resolves to. An item is a ticket, not a session.
    if scope.is_all() {
        return s.work_item_by_key(key);
    }
    for item in s.work_items_by_key(key)? {
        let visible = crate::service::trackers::tickets::item_visible(scope, s, &item)?
            || (item.tracker_id.is_none() && scope.sees_org(s.item_org(item.id)?));
        if visible {
            return Ok(Some(item));
        }
    }
    Ok(None)
}

/// The first item carrying `key` whose ORG `scope` sees, with no host
/// fence: the item a brief or a resume plan names for a reader (the landing
/// host of a resume need not have worked on the key before). Same shared-key
/// walk as [`visible_item_for_key`]; for [`OrgScope::All`] this is
/// [`Store::work_item_by_key`].
pub fn org_item_for_key(
    s: &Store,
    scope: &OrgScope,
    key: &str,
) -> Result<Option<WorkItemRow>, IpcError> {
    // This is the org boundary, not a privacy fence: the same item question, with no host fence.
    if scope.is_all() {
        return s.work_item_by_key(key);
    }
    for item in s.work_items_by_key(key)? {
        if scope.sees_org(s.item_org(item.id)?) {
            return Ok(Some(item));
        }
    }
    Ok(None)
}

/// May a per-host token read work `key` (its context, resume plan, or
/// resume it)? When the key's item is inside its orgs, and some of the
/// work — a live link on a session of its host, or a past one whose session
/// ran there — is visible to it. One refusal whether the key exists or not.
///
/// **DECIDED, multi-user M1 (T10): this is an org-authority question and it
/// is correctly asked of the org alone.** T9b/T9c disclosed a "key-level
/// residual" here — that a second person could learn *this key has some
/// work on it* from the absence of `E_NOTFOUND` — and T10 traced it and
/// found it is not in this function:
///
/// * for a per-host token and for an org-bound client (which goes to
///   [`require_key_bound`]) the question answered is "does this key have
///   work inside YOUR host / YOUR org", and that is the org boundary doing
///   its job;
/// * for every other caller — including a person's own device, whose
///   `OrgScope` is `All` — `scope.host()` is `None` and this returns
///   `Ok(())` **unconditionally**. It discloses nothing, because it decides
///   nothing: the read it precedes is person-fenced downstream
///   ([`scope_links_for`], which composes [`link_session_visible`] with
///   `link_person_visible`).
///
/// So the residual the two earlier rounds attributed partly here belongs
/// wholly to `work::work_purge_impact`, whose `OPEN_QUESTIONS` row says so
/// since T10. There is no owner decision owed at this function.
pub fn require_key(s: &Store, scope: &OrgScope, key: &str) -> Result<(), IpcError> {
    if let OrgScope::Org { .. } = scope {
        return require_key_bound(s, scope, key);
    }
    let Some(h) = scope.host() else {
        return Ok(());
    };
    let key = crate::store::normalize_work_ref(key)?;
    let refuse = || Err(not_visible_key(h, &key));
    if let KeyItems::OnlyOthers = key_items(s, scope, &key)? {
        return refuse();
    }
    let mut live: Vec<WorkLinkRow> = s
        .live_work_sessions_for_key(&key)?
        .into_iter()
        .filter(|(_, r)| r.host_alias == h)
        .map(|(l, _)| l)
        .collect();
    s.fill_link_orgs(&mut live)?;
    if live.iter().any(|l| scope.sees_link(l)) {
        return Ok(());
    }
    let mut ended = s.ended_work_links_for_key(&key)?;
    scope_links(s, scope, &mut ended)?;
    if ended.is_empty() {
        refuse()
    } else {
        Ok(())
    }
}

/// [`require_key`] for a bound client (work graph M14): the key's item must
/// be inside its orgs, and a key with no org of its own (a bare key, an
/// unassigned local item) must have some work the client may see — a live
/// or past link whose session is visible to it — or no work at all yet.
/// Refused as a key nothing is linked to, whether it exists or not.
/// What the items carrying a key are to `scope`.
enum KeyItems {
    /// No item carries the key.
    None,
    /// At least one does and `scope` sees its org: the first such item's
    /// org (`None` for an unassigned one).
    Visible(Option<i64>),
    /// Items carry it, every one in an org `scope` does not see.
    OnlyOthers,
}

/// Every item carrying `key`, not just the store's first: two trackers can
/// hold the same key (two Jira sites, one per org, both with `PAY`), and a
/// caller of the second org must not be refused over the first org's
/// ticket when its own is right there. An org-assigned item is preferred
/// over an unassigned one, in the store's order otherwise.
fn key_items(s: &Store, scope: &OrgScope, key: &str) -> Result<KeyItems, IpcError> {
    let items = s.work_items_by_key(key)?;
    if items.is_empty() {
        return Ok(KeyItems::None);
    }
    let mut unassigned = false;
    for item in &items {
        let org = s.item_org(item.id)?;
        if scope.sees_org(org) {
            if org.is_some() {
                return Ok(KeyItems::Visible(org));
            }
            unassigned = true;
        }
    }
    Ok(if unassigned {
        KeyItems::Visible(None)
    } else {
        KeyItems::OnlyOthers
    })
}

fn require_key_bound(s: &Store, scope: &OrgScope, key: &str) -> Result<(), IpcError> {
    let key = crate::store::normalize_work_ref(key)?;
    let refuse = || {
        Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("nothing is linked to {key}"),
        ))
    };
    // An org-assigned item this client sees answers at once; an unassigned
    // one it sees still needs a link in scope, unless nothing links the key.
    let has_item = match key_items(s, scope, &key)? {
        KeyItems::OnlyOthers => return refuse(),
        KeyItems::Visible(Some(_)) => return Ok(()),
        KeyItems::Visible(None) => true,
        KeyItems::None => false,
    };
    let mut links: Vec<WorkLinkRow> = s
        .live_work_sessions_for_key(&key)?
        .into_iter()
        .map(|(l, _)| l)
        .collect();
    links.extend(s.ended_work_links_for_key(&key)?);
    if links.is_empty() {
        return if has_item { Ok(()) } else { refuse() };
    }
    scope_links(s, scope, &mut links)?;
    if links.is_empty() {
        refuse()
    } else {
        Ok(())
    }
}

/// The live session rows `view` may count: what `work { action: scopes }`
/// and the org overview tally. Two org clauses, and both earn their place:
/// `sees_row_org_only` is the host/org answer (see the ORG_HALF_SITES note on
/// this file's other sites — an internal-and-narrowed scope is fenced by it
/// alone), and `sees_org` is what keeps another org's row on a per-host
/// token's OWN host out of the tally, which the first does not ask.
/// `view.sees_session_row` is the person half (multi-user M1, T9b).
fn counted_rows(
    s: &Store,
    view: &crate::service::view_scope::ViewScope,
) -> Result<Vec<SessionRow>, IpcError> {
    let scope = &view.org;
    Ok(s.list_all_sessions()?
        .into_iter()
        .filter(|r| r.status != "ghost" && scope.sees_row_org_only(r) && scope.sees_org(r.org_id))
        .filter(|r| view.sees_session_row(r).is_visible())
        .collect())
}

/// Whether a row waits on a person (`service::attention::needs_attention`).
fn needs_person(s: &Store) -> impl Fn(&SessionRow) -> bool {
    let red = crate::service::health::context_red_pct(s);
    let facts = s.attention_facts();
    move |r: &SessionRow| crate::service::attention::needs_attention_in(r, red, &facts).is_some()
}

/// `org id → (sessions, sessions that need a person)` over `rows`.
fn org_session_counts(
    rows: &[SessionRow],
    needs: &dyn Fn(&SessionRow) -> bool,
) -> BTreeMap<i64, (usize, usize)> {
    let mut out: BTreeMap<i64, (usize, usize)> = BTreeMap::new();
    for r in rows {
        if let Some(org) = r.org_id {
            let slot = out.entry(org).or_default();
            slot.0 += 1;
            slot.1 += usize::from(needs(r));
        }
    }
    out
}

/// One entry of `work { action: scopes }`: a named org, or — zero-config —
/// a project owner no org covers, or the unassigned rest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeEntry {
    /// A named org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    /// An owner-derived pseudo-scope (no org covers the owner's sessions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The sessions nothing places: no org, no owner.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unassigned: bool,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub session_count: usize,
    /// Sessions waiting on a person (`service::attention::needs_attention`).
    pub needs_you: usize,
}

/// `work { action: scopes }`: the named orgs (each with its live sessions),
/// then an owner pseudo-scope per project owner whose sessions no org
/// covers, then the unassigned rest when there is any. A host-bound caller
/// sees only its org and unassigned work.
///
/// **Takes the caller's WHOLE [`crate::service::view_scope::ViewScope`]**
/// (multi-user M1, T9b), not only its org half. `ScopeEntry.session_count`
/// and `.needs_you` are counts OVER SESSION ROWS, and rule 6's allowance is
/// a per-host count of `unclaimed` rows — not a per-org, per-owner tally of
/// every person's live work with "how much of it is blocked or stuck" beside
/// it. Fenced by the org scope alone these were fleet-wide, because
/// `OrgScope::All` is what the master AND every paired client bound to no
/// org resolve to.
pub fn scopes(
    store: &Mutex<Store>,
    view: &crate::service::view_scope::ViewScope,
) -> Result<Vec<ScopeEntry>, IpcError> {
    let scope = &view.org;
    let s = lock(store)?;
    let orgs = s.list_orgs()?;
    let projects: BTreeMap<i64, String> = s
        .list_projects()?
        .into_iter()
        .filter(|p| p.owner != "local" && !p.system)
        .map(|p| (p.id, p.owner))
        .collect();
    let rows = counted_rows(&s, view)?;
    let needs = needs_person(&s);
    let per_org = org_session_counts(&rows, &needs);
    let mut out = Vec::new();
    for o in orgs.iter().filter(|o| scope.sees_org(Some(o.id))) {
        let (session_count, needs_you) = per_org.get(&o.id).copied().unwrap_or_default();
        out.push(ScopeEntry {
            id: Some(o.id),
            owner: None,
            unassigned: false,
            label: o.name.clone(),
            color: o.color.clone(),
            session_count,
            needs_you,
        });
    }
    let mut by_owner: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut rest = (0usize, 0usize);
    for r in rows.iter().filter(|r| r.org_id.is_none()) {
        let slot = match r.project_id.and_then(|p| projects.get(&p)) {
            Some(owner) => by_owner.entry(owner.clone()).or_default(),
            None => &mut rest,
        };
        slot.0 += 1;
        slot.1 += usize::from(needs(r));
    }
    for (owner, (n, nu)) in by_owner {
        out.push(ScopeEntry {
            id: None,
            label: owner.clone(),
            owner: Some(owner),
            unassigned: false,
            color: None,
            session_count: n,
            needs_you: nu,
        });
    }
    if rest.0 > 0 {
        out.push(ScopeEntry {
            id: None,
            owner: None,
            unassigned: true,
            label: "Unassigned".into(),
            color: None,
            session_count: rest.0,
            needs_you: rest.1,
        });
    }
    Ok(out)
}

/// `work { action: orgs }`: the orgs with their rules, hosts and trackers,
/// and what the org overview shows (org administration, phase A), for
/// Settings → Organisations (read-only; changes are `work_admin`). Every
/// field past `trackers` is absent from an older hub and reads empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgDetail {
    #[serde(flatten)]
    pub org: OrgRow,
    #[serde(default)]
    pub rules: Vec<OrgRuleRow>,
    #[serde(default)]
    pub hosts: Vec<String>,
    /// `(id, name)` of the trackers assigned to it.
    #[serde(default)]
    pub trackers: Vec<OrgTrackerRef>,
    /// The asset catalogs it owns (`catalogs.org_id`), by name.
    #[serde(default)]
    pub catalogs: Vec<String>,
    /// M15 step G2.10: its project catalog. Absent from an older hub.
    #[serde(default)]
    pub projects: Vec<crate::store::OrgProjectRow>,
    /// Its live sessions the caller may count, as `work { action: scopes }`
    /// counts them.
    #[serde(default)]
    pub session_count: usize,
    /// Of those, the ones waiting on a person.
    #[serde(default)]
    pub needs_you: usize,
    /// The live paired devices bound to it — only for the fleet's
    /// administrator ([`AdminView::Admin`]). `None` (no key at all) for
    /// everyone else, so a host or an org-bound device never learns another
    /// device's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devices: Option<Vec<OrgDevice>>,
    /// Phase C, administrator only: the per-org settings
    /// ([`crate::service::settings::OrgSetting`]: the setting described with
    /// the fleet's value, and the org's own). Kept as JSON, so a desktop
    /// reads a hub's whatever settings that hub has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Vec<serde_json::Value>>,
    /// Phase C, administrator only and only for a caller that sees every
    /// session: live spend in micro-USD today, over the last 7 days and this
    /// month (UTC), and the budgets in whole USD (`0` none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent_today_micros: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent_week_micros: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent_month_micros: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_daily_usd: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_monthly_usd: Option<u64>,
    /// `daily` / `monthly`: the budgets it has reached.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub over_budget: Vec<crate::service::org_spend::Period>,
    /// Redesign 11.1, with the spend above: its live spend on each of the
    /// last 14 UTC days, today last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spend_series: Option<Vec<crate::service::org_spend::SpendDay>>,
    /// Redesign 11.8: the same spend by person (today, 7 days, month), all
    /// or nothing — only to an administrator who sees every session there
    /// is, since a person's figure counts their private sessions. Absent
    /// for anyone else, never cut down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spend_by_person: Option<Vec<crate::service::org_spend::PersonSpend>>,
    /// Redesign 11.1, for whoever administers it: what its admins should
    /// look at (`service::org_needs`). Absent for anyone else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_admin: Option<Vec<crate::service::org_needs::AdminNeed>>,
    /// Phase D: who is in the company, with their roles — for the fleet's
    /// administrator and for the org's own people. Absent for anyone else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<OrgMember>>,
    /// Phase D: the caller's own role in it (`admin` / `member` / `viewer`),
    /// so a page offers what the caller may do. Absent when they have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_role: Option<String>,
}

/// One member as the org overview lists them (phase D).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgMember {
    pub person_id: i64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub role: String,
    /// Redesign 11.2: when they joined (absent from an older hub).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at: Option<i64>,
    /// Redesign 11.2: since when an org share reaches them (a grant made
    /// before it does not); absent for a viewer, who receives none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shares_since: Option<i64>,
    /// Redesign 11.2, for whoever administers the org: their live devices,
    /// by name. Absent for anyone else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devices: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgTrackerRef {
    pub id: i64,
    pub name: String,
}

/// A paired client bound to an org, as the org overview lists it. Never
/// carries the token digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgDevice {
    pub name: String,
    /// `full` or `readonly`.
    pub mode: String,
    pub trusted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<i64>,
}

/// Whether [`org_details`] carries what only the fleet's administrator
/// sees of an org: its bound devices, its own settings and — when the
/// caller also sees every session (`org_spend::sees_all_spend`) — its
/// spend and budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminView {
    /// The fleet's administrator: the desktop's own store, the master token,
    /// or the hub's personal owner on an unbound device.
    Admin,
    /// A person's device (phase D): the orgs they administer — whose
    /// devices, own settings and spend they see, as the fleet's
    /// administrator does — and every org they are in, live (whose members
    /// they see), with their role.
    Person { roles: BTreeMap<i64, String> },
    /// Everyone else.
    Other,
}

impl AdminView {
    /// The one rule for an MCP caller (`work { action: orgs }`).
    pub fn for_caller(
        caller: &crate::mcp::auth::Caller,
        store: &Mutex<Store>,
    ) -> Result<Self, IpcError> {
        if caller.is_master() || (caller.is_person_device() && caller.is_personal_owner) {
            return Ok(AdminView::Admin);
        }
        let Some(p) = caller.person() else {
            return Ok(AdminView::Other);
        };
        let s = lock(store)?;
        let mut roles: BTreeMap<i64, String> = s
            .memberships_of(p)?
            .into_iter()
            .filter(|m| m.is_live())
            .map(|m| (m.org_id, m.role))
            .collect();
        // The hub's owner on a device bound to an org administers it.
        if caller.is_personal_owner {
            if let Some(o) = caller.client.as_ref().and_then(|c| c.org_id) {
                roles.insert(o, crate::store::ROLE_ADMIN.to_string());
            }
        }
        Ok(if roles.is_empty() {
            AdminView::Other
        } else {
            AdminView::Person { roles }
        })
    }

    fn role(&self, org: i64) -> Option<&str> {
        match self {
            AdminView::Person { roles } => roles.get(&org).map(String::as_str),
            _ => None,
        }
    }

    /// Sees `org`'s administrator data: devices, own settings, spend.
    fn administers(&self, org: i64) -> bool {
        matches!(self, AdminView::Admin) || self.role(org) == Some(crate::store::ROLE_ADMIN)
    }
}

pub fn org_details(
    store: &Mutex<Store>,
    view: &crate::service::view_scope::ViewScope,
    devices: AdminView,
) -> Result<Vec<OrgDetail>, IpcError> {
    org_details_locked(&*lock(store)?, view, devices)
}

// --- rule preview (M15 step G2.10) ----------------------------------------------

/// One "Match by" value as the rule it makes (the org form's single rule
/// form, `pages::resources::ORG_RULE_MATCH`): `repository` takes `owner/name`
/// (or the repository's GitHub URL), `owner` a GitHub owner, `path` a path
/// prefix, `host` a host alias. The rule's `org_id` is the caller's.
pub fn rule_from_match(org_id: i64, match_by: &str, value: &str) -> Result<OrgRuleRow, IpcError> {
    let v = value.trim();
    if v.is_empty() {
        return Err(IpcError::new(codes::E_INVALID, "say what the rule matches"));
    }
    let mut rule = OrgRuleRow {
        id: 0,
        org_id,
        owner: None,
        repo: None,
        path_prefix: None,
        host_alias: None,
    };
    match match_by.trim() {
        "repository" => {
            let bare = v
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_start_matches("git@github.com:")
                .trim_start_matches("github.com/")
                .trim_end_matches('/')
                .trim_end_matches(".git");
            let Some((owner, repo)) = bare
                .split_once('/')
                .filter(|(o, r)| !o.is_empty() && !r.is_empty() && !r.contains('/'))
            else {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("a repository is owner/name, not {v:?}"),
                ));
            };
            rule.owner = Some(owner.to_string());
            rule.repo = Some(repo.to_string());
        }
        "owner" => rule.owner = Some(v.to_string()),
        "path" => rule.path_prefix = Some(v.to_string()),
        "host" => rule.host_alias = Some(v.to_string()),
        other => {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("match_by is repository, path, host or owner, not {other:?}"),
            ))
        }
    }
    Ok(rule)
}

/// What an unsaved org rule would do now: the live sessions its condition
/// matches, and how many of them would move into the org, from where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulePreview {
    /// Live sessions the rule's condition matches.
    pub matches: usize,
    /// Of those, the ones that would move into the org (another rule that is
    /// more specific keeps the rest where they are).
    pub moving: usize,
    /// Where the moving ones are now, largest first.
    #[serde(default)]
    pub from: Vec<RuleMoveFrom>,
    /// Matched sessions a more specific rule keeps elsewhere.
    #[serde(default)]
    pub kept: usize,
    /// "Matches 14 sessions now; 3 of them are in Personal and would move."
    pub sentence: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleMoveFrom {
    /// The org they are in now; `None` = no org (Personal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    /// Its name, or `Personal`.
    pub name: String,
    pub count: usize,
}

/// Preview `rule` for `org` (M15 step G2.10): normalised and checked as
/// `add_rule` would, then answered over the live sessions without writing.
/// Counts only, never a session: an admin previewing a rule learns how many
/// of the fleet's sessions it reaches, which the org overview's counts
/// already tell the hub's owner.
pub fn rule_preview(s: &Store, org: i64, rule: OrgRuleRow) -> Result<RulePreview, IpcError> {
    if s.get_org(org)?.is_none() {
        return Err(not_found("org", org));
    }
    let rule = crate::store::normalize_rule(OrgRuleRow {
        org_id: org,
        ..rule
    })?;
    let rows = s.preview_org_rule(&rule)?;
    let names: std::collections::HashMap<i64, String> =
        s.list_orgs()?.into_iter().map(|o| (o.id, o.name)).collect();
    let matches = rows.iter().filter(|r| r.matched).count();
    let mut from: std::collections::BTreeMap<Option<i64>, usize> = Default::default();
    let mut kept = 0;
    for r in rows.iter().filter(|r| r.matched) {
        if r.after == Some(org) && r.before != Some(org) {
            *from.entry(r.before).or_default() += 1;
        } else if r.after != Some(org) {
            kept += 1;
        }
    }
    let mut from: Vec<RuleMoveFrom> = from
        .into_iter()
        .map(|(o, count)| RuleMoveFrom {
            org_id: o,
            name: o
                .and_then(|o| names.get(&o).cloned())
                .unwrap_or_else(|| "Personal".into()),
            count,
        })
        .collect();
    from.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    let moving: usize = from.iter().map(|f| f.count).sum();
    let sentence = rule_preview_sentence(matches, moving, &from, kept);
    Ok(RulePreview {
        matches,
        moving,
        from,
        kept,
        sentence,
    })
}

fn sessions_word(n: usize) -> String {
    format!("{n} {}", if n == 1 { "session" } else { "sessions" })
}

/// The preview in one sentence, the form's live impact line.
fn rule_preview_sentence(
    matches: usize,
    moving: usize,
    from: &[RuleMoveFrom],
    kept: usize,
) -> String {
    if matches == 0 {
        return "Matches no session now; it applies to sessions started later.".into();
    }
    let mut out = format!("Matches {} now", sessions_word(matches));
    if moving == 0 {
        out.push_str("; none would move");
    } else if let [only] = from {
        out.push_str(&format!(
            "; {moving} of them {} in {} and would move",
            if moving == 1 { "is" } else { "are" },
            only.name
        ));
    } else {
        let wheres: Vec<String> = from
            .iter()
            .map(|f| format!("{} from {}", f.count, f.name))
            .collect();
        out.push_str(&format!(
            "; {moving} of them would move ({})",
            wheres.join(", ")
        ));
    }
    if kept > 0 {
        out.push_str(&format!(
            "; a more specific rule keeps {} where {}",
            sessions_word(kept),
            if kept == 1 { "it is" } else { "they are" }
        ));
    }
    out.push('.');
    out
}

// --- administration (work graph M5.2) ------------------------------------------

/// The org actions of `work_admin` — Master-only on the MCP surface
/// (`work_admin` is `Access::Master`), `LocalOnly` on a paired desktop, and
/// `fleet-hub org …` on the hub. A host can never move itself: no host-bound
/// path reaches here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgAction {
    ListOrgs,
    AddOrg,
    UpdateOrg,
    RemoveOrg,
    AddRule,
    RemoveRule,
    AssignHost,
    UnassignHost,
    AssignTracker,
    /// Bind a paired client to an org, or unbind it (work graph M14).
    AssignClient,
}

impl OrgAction {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "list_orgs" => OrgAction::ListOrgs,
            "add_org" => OrgAction::AddOrg,
            "update_org" => OrgAction::UpdateOrg,
            "remove_org" => OrgAction::RemoveOrg,
            "add_rule" | "add_org_rule" => OrgAction::AddRule,
            "remove_rule" | "remove_org_rule" => OrgAction::RemoveRule,
            "assign_host" => OrgAction::AssignHost,
            "unassign_host" => OrgAction::UnassignHost,
            "assign_tracker" => OrgAction::AssignTracker,
            "assign_client" => OrgAction::AssignClient,
            _ => return None,
        })
    }
}

/// `auto_tidy` of `add_org` / `update_org` (work graph M7): `on` / `off`
/// override `work.auto_tidy` for the org, `inherit` clears the override.
/// `None` (absent) leaves it as it is.
fn parse_auto_tidy(v: Option<&str>) -> Result<Option<Option<bool>>, IpcError> {
    match v.map(str::trim) {
        None => Ok(None),
        Some("on") => Ok(Some(Some(true))),
        Some("off") => Ok(Some(Some(false))),
        Some("inherit") => Ok(Some(None)),
        Some(other) => Err(IpcError::new(
            codes::E_INVALID,
            format!("auto_tidy is on, off or inherit, not {other:?}"),
        )),
    }
}

/// `jev` of `add_org` / `update_org` (Jev evaluation, D31 / D36): `on`
/// lets this org's redacted texts go to the decision model when the global
/// flag and a feature's mode allow it; `off` (the default) never. `None`
/// (absent) leaves it as it is.
fn parse_jev(v: Option<&str>) -> Result<Option<bool>, IpcError> {
    match v.map(str::trim) {
        None => Ok(None),
        Some("on") => Ok(Some(true)),
        Some("off") => Ok(Some(false)),
        Some(other) => Err(IpcError::new(
            codes::E_INVALID,
            format!("jev is on or off, not {other:?}"),
        )),
    }
}

fn need<T: Clone>(v: &Option<T>, action: &str, field: &str) -> Result<T, IpcError> {
    v.clone()
        .ok_or_else(|| IpcError::new(codes::E_INVALID, format!("{action} needs {field}")))
}

fn to_json<T: Serialize>(v: &T) -> Result<serde_json::Value, IpcError> {
    serde_json::to_value(v).map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))
}

/// Run one org action (the caller already passed the Master gate and, for a
/// removal, the confirmation gate). Every change that can move a session
/// between orgs re-announces the moved sessions (`Store::announce_org_moves`).
pub fn admin(
    action: OrgAction,
    args: &crate::service::trackers::admin::WorkAdminArgs,
    s: &Store,
) -> Result<serde_json::Value, IpcError> {
    let name = args.action.as_str();
    let before = s.session_orgs()?;
    let out = match action {
        OrgAction::ListOrgs => {
            return to_json(&org_details_locked(
                s,
                &crate::service::view_scope::ViewScope::internal(),
                AdminView::Admin,
            )?);
        }
        OrgAction::AddOrg => {
            let auto = parse_auto_tidy(args.auto_tidy.as_deref())?;
            let jev = parse_jev(args.jev.as_deref())?;
            let jev_reply = parse_jev(args.jev_reply.as_deref())?;
            let mut org = s.add_org(
                &need(&args.name, name, "name")?,
                args.color.as_deref(),
                args.isolate_sessions.unwrap_or(false),
            )?;
            if let Some(a) = auto {
                org = s.set_org_auto_tidy(org.id, a)?;
            }
            if let Some(j) = jev {
                org = s.set_org_jev_allowed(org.id, j)?;
            }
            if let Some(j) = jev_reply {
                org = s.set_org_jev_reply_allowed(org.id, j)?;
            }
            if let Some(on) = args.bound_sees_unassigned {
                org = s.set_org_bound_sees_unassigned(org.id, on)?;
            }
            to_json(&org)?
        }
        OrgAction::UpdateOrg => {
            let auto = parse_auto_tidy(args.auto_tidy.as_deref())?;
            let jev = parse_jev(args.jev.as_deref())?;
            let jev_reply = parse_jev(args.jev_reply.as_deref())?;
            let id = need(&args.org_id, name, "org_id")?;
            let mut org = s.update_org(
                id,
                args.name.as_deref(),
                args.color.as_deref(),
                args.isolate_sessions,
            )?;
            if let Some(a) = auto {
                org = s.set_org_auto_tidy(id, a)?;
            }
            if let Some(j) = jev {
                org = s.set_org_jev_allowed(id, j)?;
                tracing::info!(org_id = id, jev = j, "[decide] org consent changed");
            }
            // D48: the second consent, to reply text (J2).
            if let Some(j) = jev_reply {
                org = s.set_org_jev_reply_allowed(id, j)?;
                tracing::info!(
                    org_id = id,
                    jev_reply = j,
                    "[decide] org reply consent changed"
                );
            }
            // D31 (work graph M14.1b): what the org's bound clients see of
            // unassigned work and sessions.
            if let Some(on) = args.bound_sees_unassigned {
                org = s.set_org_bound_sees_unassigned(id, on)?;
            }
            to_json(&org)?
        }
        OrgAction::RemoveOrg => {
            let id = need(&args.org_id, name, "org_id")?;
            let org = s.get_org(id)?.ok_or_else(|| not_found("org", id))?;
            let trackers = s.trackers_of_org(id)?;
            if !trackers.is_empty() {
                let names: Vec<String> = trackers
                    .iter()
                    .map(|(tid, n)| format!("{n} (tracker {tid})"))
                    .collect();
                return Err(IpcError::new(
                    codes::E_INVALID_STATE,
                    format!(
                        "org {:?} still owns {}; move them first (assign_tracker)",
                        org.name,
                        names.join(", ")
                    ),
                )
                .with_details(serde_json::json!({
                    "trackers": trackers.iter().map(|(i, _)| i).collect::<Vec<_>>()
                })));
            }
            s.remove_org(id)?;
            serde_json::json!({ "removed": id })
        }
        OrgAction::AddRule => to_json(&s.add_org_rule(OrgRuleRow {
            id: 0,
            org_id: need(&args.org_id, name, "org_id")?,
            owner: args.owner.clone(),
            repo: args.repo.clone(),
            path_prefix: args.path_prefix.clone(),
            host_alias: args.host_alias.clone(),
        })?)?,
        OrgAction::RemoveRule => {
            let id = need(&args.rule_id, name, "rule_id")?;
            if !s.remove_org_rule(id)? {
                return Err(not_found("rule", id));
            }
            serde_json::json!({ "removed": id })
        }
        OrgAction::AssignHost => {
            let host = need(&args.host_alias, name, "host_alias")?;
            let org = need(&args.org_id, name, "org_id")?;
            s.set_host_org(&host, Some(org))?;
            serde_json::json!({ "host_alias": host, "org_id": org })
        }
        OrgAction::UnassignHost => {
            let host = need(&args.host_alias, name, "host_alias")?;
            s.set_host_org(&host, None)?;
            serde_json::json!({ "host_alias": host, "org_id": null })
        }
        OrgAction::AssignTracker => {
            let id = need(&args.tracker_id, name, "tracker_id")?;
            s.set_tracker_org(id, args.org_id)?;
            s.emit_tracker(id)?;
            to_json(&s.require_tracker(id)?)?
        }
        // `org_id` absent unbinds. The row never carries the token digest.
        OrgAction::AssignClient => {
            let client = need(&args.name, name, "name")?;
            let row = s.set_client_org(&client, args.org_id)?;
            serde_json::json!({ "name": row.name, "mode": row.mode, "org_id": row.org_id })
        }
    };
    // Before the announcement: a stream that reads the moved rows must
    // already see a new generation and re-read its scope.
    ORG_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    s.announce_org_moves(&before)?;
    Ok(out)
}

fn org_details_locked(
    s: &Store,
    view: &crate::service::view_scope::ViewScope,
    devices: AdminView,
) -> Result<Vec<OrgDetail>, IpcError> {
    let scope = &view.org;
    let rules = s.list_org_rules()?;
    let hosts = s.list_hosts()?;
    let catalogs = s.list_catalogs()?;
    let needs = needs_person(s);
    let counts = org_session_counts(&counted_rows(s, view)?, &needs);
    let admin = devices == AdminView::Admin;
    let any_admin = admin || matches!(devices, AdminView::Person { .. });
    // The devices as the auth layer resolves them: an org admin sees the
    // devices fenced to their org by membership (phase D); the fleet's
    // administrator, the ones bound to it.
    let clients = any_admin
        .then(|| {
            if admin {
                s.list_client_tokens(false)
            } else {
                s.auth_client_tokens()
            }
        })
        .transpose()?;
    let owner = s.personal_owner_id()?;
    // An org admin sees their own org's spend (phases A–C, for that org);
    // the fleet's administrator every org's, when they see every session.
    let now = crate::service::catalog::now_secs();
    let sees_all = any_admin && crate::service::org_spend::sees_all_spend(s, view);
    let spend = (any_admin && (!admin || sees_all))
        .then(|| crate::service::org_spend::spend_by_org(s, now));
    // Read once, and only when an org's admin is told the count.
    let mut unclaimed: Option<BTreeMap<String, i64>> = None;
    let mut out = Vec::new();
    for o in s.list_orgs()? {
        if !scope.sees_org(Some(o.id)) {
            continue;
        }
        let (session_count, needs_you) = counts.get(&o.id).copied().unwrap_or_default();
        let administers = devices.administers(o.id);
        let my_role = devices.role(o.id).map(str::to_string);
        let member_devices = clients.as_deref().filter(|_| administers);
        let members = (admin || my_role.is_some())
            .then(|| org_member_list(s, o.id, member_devices))
            .transpose()?;
        // The devices fenced to it, as its administrators are shown them.
        let org_clients: Option<Vec<&crate::store::ClientTokenRow>> =
            clients.as_ref().filter(|_| administers).map(|cs| {
                cs.iter()
                    .filter(|c| c.org_id == Some(o.id))
                    .filter(|c| admin || !(c.person_id.is_some() && c.person_id == owner))
                    .filter(|c| crate::store::machine_token_kind(&c.mode).is_none())
                    .collect()
            });
        let mut needs_admin = administers.then(Vec::new);
        if let (Some(needs), Some(cs)) = (needs_admin.as_mut(), org_clients.as_ref()) {
            use crate::service::org_needs::AdminNeed;
            needs.extend(
                cs.iter()
                    .filter(|c| c.trusted_at.is_none() && c.mode == "full")
                    .map(|c| AdminNeed::UntrustedDevice {
                        device: c.name.clone(),
                        paired_at: c.created_at,
                    }),
            );
        }
        if let Some(needs) = needs_admin.as_mut() {
            if view.sees_unclaimed_count(Some(o.id)) {
                let counts = unclaimed
                    .get_or_insert_with(|| s.unclaimed_counts_by_host().unwrap_or_default());
                for h in hosts.iter().filter(|h| h.org_id == Some(o.id)) {
                    let count = counts.get(&h.alias).copied().unwrap_or(0);
                    if count > 0 {
                        needs.push(crate::service::org_needs::AdminNeed::UnclaimedSessions {
                            host: h.alias.clone(),
                            count: count as usize,
                        });
                    }
                }
            }
        }
        let mut d = OrgDetail {
            needs_admin,
            spend_series: None,
            spend_by_person: None,
            members,
            my_role,
            catalogs: catalogs
                .iter()
                .filter(|c| c.org_id == Some(o.id))
                .map(|c| c.name.clone())
                .collect(),
            session_count,
            needs_you,
            devices: org_clients.as_ref().map(|cs| {
                cs.iter()
                    .map(|c| OrgDevice {
                        name: c.name.clone(),
                        mode: c.mode.clone(),
                        trusted: c.trusted_at.is_some(),
                        last_seen_at: c.last_seen_at,
                    })
                    .collect()
            }),
            rules: rules.iter().filter(|r| r.org_id == o.id).cloned().collect(),
            projects: s.org_projects(o.id)?,
            hosts: hosts
                .iter()
                .filter(|h| h.org_id == Some(o.id))
                .map(|h| h.alias.clone())
                .collect(),
            trackers: s
                .trackers_of_org(o.id)?
                .into_iter()
                .map(|(id, name)| OrgTrackerRef { id, name })
                .collect(),
            settings: administers.then(|| {
                crate::service::settings::org_settings(s, o.id)
                    .iter()
                    .filter_map(|v| serde_json::to_value(v).ok())
                    .collect()
            }),
            spent_today_micros: None,
            spent_week_micros: None,
            spent_month_micros: None,
            budget_daily_usd: None,
            budget_monthly_usd: None,
            over_budget: Vec::new(),
            org: o,
        };
        if let Some(spend) = spend.as_ref().filter(|_| administers) {
            use crate::service::org_spend as os;
            let got = spend.get(&d.org.id).copied().unwrap_or_default();
            let (daily, monthly) = os::budgets(s, d.org.id);
            d.spent_today_micros = Some(got.today_micros);
            d.spent_week_micros = Some(got.week_micros);
            d.spent_month_micros = Some(got.month_micros);
            d.budget_daily_usd = Some(daily);
            d.budget_monthly_usd = Some(monthly);
            d.over_budget = os::reached(got, (daily, monthly))
                .into_iter()
                .map(|(p, ..)| p)
                .collect();
            d.spend_series = Some(os::series(s, d.org.id, now));
            // A person's figure is a sum over their sessions, private ones
            // among them: the whole table or none of it (11.8).
            if sees_all {
                d.spend_by_person = Some(os::by_person(s, d.org.id, now));
            }
            if let Some(needs) = d.needs_admin.as_mut() {
                let found = crate::service::org_needs::budget_needs(
                    got,
                    (daily, monthly),
                    now.div_euclid(86_400),
                );
                // Budgets first: they are the org's own, the rest are things in it.
                needs.splice(0..0, found);
            }
        }
        out.push(d);
    }
    Ok(out)
}

/// The live members of `org`, admins first, as the overview lists them;
/// with each one's devices out of `devices` when the caller administers it.
fn org_member_list(
    s: &Store,
    org: i64,
    devices: Option<&[crate::store::ClientTokenRow]>,
) -> Result<Vec<OrgMember>, IpcError> {
    let mut out = Vec::new();
    for m in s.org_members(org)? {
        if let Some(p) = s
            .get_person(m.person_id)?
            .filter(|p| p.disabled_at.is_none())
        {
            out.push(OrgMember {
                devices: devices.map(|cs| {
                    cs.iter()
                        .filter(|c| c.person_id == Some(p.id) && c.revoked_at.is_none())
                        .filter(|c| crate::store::machine_token_kind(&c.mode).is_none())
                        .map(|c| c.name.clone())
                        .collect()
                }),
                person_id: p.id,
                name: p.name,
                display_name: p.display_name,
                role: m.role,
                added_at: Some(m.added_at),
                shares_since: m.shares_since,
            });
        }
    }
    Ok(out)
}

/// One proposal of `work { action: org_suggestions }`: create org `name`
/// from `owner/*` and/or for a tracker. Never applied automatically; the UI
/// offers it as one click.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgSuggestion {
    pub name: String,
    /// Add the rule `owner/*`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Assign this tracker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_id: Option<i64>,
    /// Live sessions the rule would place.
    pub sessions: usize,
    /// Why, in one line.
    pub reason: String,
}

/// The first label of an Atlassian site (`https://acme.atlassian.net` →
/// `acme`), which is usually the company.
fn site_label(site_url: &str) -> Option<String> {
    let host = site_url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()?;
    let first = host.split('.').next()?;
    (!first.is_empty()).then(|| first.to_ascii_lowercase())
}

/// Proposals, from what fleet already sees:
///
/// * an owner of live sessions that no rule names and that no org covers
///   → "Create org <owner> from `<owner>/*`";
/// * a tracker without an org → an org named after its site, with the
///   owner of the same name when there is one (`acme.atlassian.net` and
///   GitHub `acme` are one company more often than not).
///
/// Empty for a per-host token: it cannot act on any of it.
///
/// `OrgSuggestion.sessions` ("Live sessions the rule would place") is a count
/// over session ROWS, so this takes the caller's whole
/// [`crate::service::view_scope::ViewScope`] too — see [`scopes`] for why the
/// org half alone fences nobody's own device.
pub fn org_suggestions(
    store: &Mutex<Store>,
    view: &crate::service::view_scope::ViewScope,
) -> Result<Vec<OrgSuggestion>, IpcError> {
    let scope = &view.org;
    // This is the org boundary, not a privacy fence: only a caller that may CREATE an org is
    // offered one to create. The person half is below, on every row a suggestion counts.
    if !scope.is_all() {
        return Ok(Vec::new());
    }
    let s = lock(store)?;
    let owners: BTreeMap<i64, String> = s
        .list_projects()?
        .into_iter()
        .filter(|p| p.owner != "local" && !p.system)
        .map(|p| (p.id, p.owner))
        .collect();
    let named: BTreeSet<String> = s
        .list_org_rules()?
        .into_iter()
        .filter_map(|r| r.owner.map(|o| o.to_ascii_lowercase()))
        .collect();
    let org_names: BTreeSet<String> = s
        .list_orgs()?
        .into_iter()
        .map(|o| o.name.to_ascii_lowercase())
        .collect();
    // Uncovered owners of live, unassigned sessions, with their counts.
    let mut uncovered: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for r in s.list_all_sessions()? {
        if r.status == "ghost" || r.org_id.is_some() {
            continue;
        }
        // The PERSON half: a suggestion counts the rows this caller may see.
        if !view.sees_session_row(&r).is_visible() {
            continue;
        }
        let Some(owner) = r.project_id.and_then(|p| owners.get(&p)) else {
            continue;
        };
        if named.contains(&owner.to_ascii_lowercase()) {
            continue;
        }
        uncovered
            .entry(owner.to_ascii_lowercase())
            .or_insert_with(|| (owner.clone(), 0))
            .1 += 1;
    }
    let mut out = Vec::new();
    let mut used_owner = BTreeSet::new();
    for t in s
        .list_trackers()?
        .into_iter()
        .filter(|t| t.org_id.is_none())
    {
        let Some(label) = site_label(&t.site_url) else {
            continue;
        };
        if org_names.contains(&label) {
            continue;
        }
        let owner = uncovered.get(&label).cloned();
        if owner.is_some() {
            used_owner.insert(label.clone());
        }
        out.push(OrgSuggestion {
            name: owner.as_ref().map(|o| o.0.clone()).unwrap_or(label.clone()),
            reason: match &owner {
                Some((o, _)) => format!("tracker {} and GitHub owner {o} share a name", t.name),
                None => format!("tracker {} has no org yet", t.name),
            },
            sessions: owner.as_ref().map(|o| o.1).unwrap_or(0),
            owner: owner.map(|o| o.0),
            tracker_id: Some(t.id),
        });
    }
    for (key, (owner, n)) in uncovered {
        if used_owner.contains(&key) || org_names.contains(&key) {
            continue;
        }
        out.push(OrgSuggestion {
            name: owner.clone(),
            reason: format!(
                "{n} live session{} under {owner}/*",
                if n == 1 { "" } else { "s" }
            ),
            owner: Some(owner),
            tracker_id: None,
            sessions: n,
        });
    }
    // Only worth offering when it would separate something: two or more
    // scopes after it, or a tracker to attach.
    if out.len() < 2 && out.iter().all(|o| o.tracker_id.is_none()) && s.list_orgs()?.is_empty() {
        return Ok(Vec::new());
    }
    Ok(out)
}

#[cfg(test)]
#[path = "orgs_tests.rs"]
mod tests;
