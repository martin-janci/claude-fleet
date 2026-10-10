//! `ViewScope` (multi-user M1, task T6): WHO is asking, for every read and
//! write that names a session.
//!
//! The org scope ([`OrgScope`]) answers "which company's work may this
//! caller read". It is kept, whole, and this type **wraps** it rather than
//! growing a person dimension inside it. The reason is one method:
//! [`OrgScope::is_all`] is an early return at dozens of production sites —
//! a dozen of them on session paths — and every one of those reads "this
//! caller is unrestricted". Teaching `All` about people would turn each of
//! them into a silent leak the compiler never mentions. A wrapper cannot be
//! mistaken for the inner value, and the rename of `OrgScope`'s three
//! session predicates to `*_org_only` makes every remaining site say, in
//! its own name, that it answers half the question.
//!
//! **The answer a session read needs is [`Visibility`], and it has two
//! arms.** A caller either sees a row *and its content*, or sees neither.
//! There is deliberately no "row, but not its content": on `/events` a
//! `session:created` frame IS the row and IS its content, so a middle value
//! has nothing to mean at the choke point that matters most (spec §4.3,
//! *`'unclaimed'`: the safe holding state*). An out-of-scope caller learns a
//! per-host COUNT of unclaimed rows ([`crate::store::Store::unclaimed_counts_by_host`])
//! and not one byte more.
//!
//! **A scope built from a TOKEN that names no person refuses.** It does not
//! fall back to "everything", and the two states are not the same value:
//! [`ViewScope::internal`] is what the hub's own readers (GC, reconcile, the
//! playbooks, attention, `fleet_health`) use, and it is built directly,
//! never from a `Caller`. "The hub does its work" and "a caller sees
//! everything" were one value before M1, which is exactly how a person-less
//! token would have inherited the master's reach.
//!
//! The one constructor from a request is `Caller::view_scope`
//! (`mcp/auth.rs`); `view_scope_tests::only_caller_view_scope_constructs_a_view_scope`
//! holds that true by reading the source, and the private fields below hold
//! it true for the compiler as well — a struct literal for `ViewScope`
//! cannot be written outside this module.

use crate::service::orgs::OrgScope;
use crate::store::{SessionRow, VISIBILITY_UNCLAIMED};
use std::collections::BTreeMap;

/// What a caller may learn about one session row.
///
/// Two arms, and the missing third is the design: see this module's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// Nothing. The row answers exactly as an id that does not exist — no
    /// existence oracle, the same discipline [`crate::service::orgs::not_found`]
    /// already applies to the org boundary.
    None,
    /// The row, and everything on it. A session's metadata *is* content
    /// (spec §4.3, *What counts as content*), so there is no reading under
    /// which a caller gets the row with the interesting fields removed.
    RowAndContent,
}

impl Visibility {
    /// True for [`Visibility::RowAndContent`] — the shorthand for a filter
    /// that only has to keep or drop a row.
    pub fn is_visible(self) -> bool {
        matches!(self, Visibility::RowAndContent)
    }
}

/// The identity half of a session row: everything
/// [`ViewScope::sees_session_facts`] reads and nothing else.
///
/// It exists because one frame on `/events` has to be judged when the row is
/// **gone**: `session:killed` fires after the `DELETE`, so a lookup by id
/// answers nothing and the facts travel on the frame instead
/// (`events::SessionKilledPayload`, multi-user M1 T9). Rather than write the
/// visibility rule a second time over those four values, the rule's body
/// moved here and [`ViewScope::sees_session_row`] became the one-line caller
/// it always should have been — so "a killed row" and "a live row" cannot
/// drift apart by a clause.
///
/// Borrowed, never owned: every caller already has the strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionFacts<'a> {
    pub id: i64,
    pub host_alias: &'a str,
    /// The session's org, as `session_org_sql!` computed it.
    pub org_id: Option<i64>,
    /// [`crate::store::VISIBILITY_PRIVATE`] or
    /// [`crate::store::VISIBILITY_UNCLAIMED`].
    pub visibility: &'a str,
    pub owner_person_id: Option<i64>,
}

impl<'a> SessionFacts<'a> {
    /// The facts of a live row.
    pub fn of(row: &'a SessionRow) -> Self {
        SessionFacts {
            id: row.id,
            host_alias: &row.host_alias,
            org_id: row.org_id,
            visibility: &row.visibility,
            owner_person_id: row.owner_person_id,
        }
    }
}

/// The live grants TO one person, as session id → level
/// ([`crate::store::GRANT_WATCH`] / [`crate::store::GRANT_ANSWER`] /
/// [`crate::store::GRANT_DRIVE`]).
///
/// A `BTreeMap`, never a `HashMap`, and that is load-bearing rather than
/// taste: [`ViewScope`] derives `PartialEq`, and `mcp/events_route.rs`
/// compares a stream's scope against a freshly-read one on its keep-alive
/// rescope and its pre-frame generation check. A hash set's iteration order
/// is stable within a process, but a `HashMap`'s `PartialEq` is fine while
/// its *Debug* and any future ordered use are not — and the store already
/// hands this over as a `BTreeMap`
/// ([`crate::store::Store::grants_for_person`]), so a canonical ordering
/// costs nothing here and removes a whole class of spurious stream drops.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrantSet(BTreeMap<i64, String>);

impl GrantSet {
    /// Wrap what [`crate::store::Store::grants_for_person`] returned.
    pub fn from_map(grants: BTreeMap<i64, String>) -> Self {
        GrantSet(grants)
    }

    /// The level granted on `session_id`, or `None` for a session this
    /// person was never granted.
    pub fn level(&self, session_id: i64) -> Option<&str> {
        self.0.get(&session_id).map(String::as_str)
    }

    /// True when this person holds no grant at all — the common case, and
    /// worth answering without a lookup.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every grant, as the store handed them over. `my_grants` serves these
    /// pairs as they are.
    pub fn as_map(&self) -> &BTreeMap<i64, String> {
        &self.0
    }
}

/// Whose hosts' unclaimed counts a caller is served
/// ([`ViewScope::sees_unclaimed_count`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum UnclaimedReach {
    #[default]
    None,
    /// A host administrator: every host.
    Every,
    /// The hosts of these orgs.
    Orgs(std::collections::BTreeSet<i64>),
}

/// Whose sessions a person WATCHES through an org whose "members see only
/// their own sessions" switch is off (M15 step G2.10): org id → the live
/// members of that org other than the person. A session is in reach when it
/// is in one of these orgs and one of that org's listed members owns it.
///
/// Read only, by construction: [`ViewScope::may_answer`],
/// [`ViewScope::may_drive`] and [`ViewScope::may_own`] never consult it, so a
/// teammate sees a row and its content and cannot press a key on it. An
/// `unclaimed` row is never in reach (it has no owner to be a teammate).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TeamReach(BTreeMap<i64, std::collections::BTreeSet<i64>>);

impl TeamReach {
    /// Wrap what [`crate::service::org_admin::team_reach`] computed.
    pub fn from_map(m: BTreeMap<i64, std::collections::BTreeSet<i64>>) -> Self {
        TeamReach(m)
    }

    /// Is a session of `org` owned by `owner` in reach?
    pub fn covers(&self, org: Option<i64>, owner: Option<i64>) -> bool {
        match (org, owner) {
            (Some(o), Some(p)) => self.0.get(&o).is_some_and(|members| members.contains(&p)),
            _ => false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Who is asking, for every session read and write.
///
/// Built once per request, off the same store handle the rows come from —
/// a scope read through the reader while the rows come from the writer
/// races a grant that was created between the two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewScope {
    /// The org half, untouched (work graph M5 / M14). Composed with the
    /// person half rather than replaced by it: a bound client that may not
    /// read another org's work may not read its sessions either, whoever
    /// owns them.
    pub org: OrgScope,
    /// WHOSE view this is, as a `people` row id. `None` is a caller that
    /// proves no person — a per-host token (a machine is not a person), or
    /// a device the backfill never reached — and it **refuses**, it does not
    /// widen. The `None == None` trap the spec names lives exactly here, and
    /// [`ViewScope::owns`] is written so it cannot be expressed.
    pub person: Option<i64>,
    /// The live grants to [`Self::person`], read with the scope. Empty for
    /// a person-less caller, which is the honest answer rather than a
    /// special case.
    pub grants: GrantSet,
    /// The host a per-host token is bound to. `Some` makes this an agent on
    /// a machine, with §4.4's two-clause reach and nothing else.
    pub host: Option<String>,
    /// The ONE row this request's pane proves it is standing in (R6-i).
    ///
    /// One row, never a set: the proof rides the connection as
    /// `X-Fleet-Pane` (`Caller::pane`), so a request proves at most the one
    /// pane it is running in. Resolved once per request through
    /// [`crate::store::Store::find_session_by_pane`], which filters on the
    /// host, excludes ghosts, takes `LIMIT 2` and answers `None` on an
    /// ambiguous match.
    ///
    /// Nothing about it is stored. A durable `(host_alias, pane_id) →
    /// session_id` record would make every pane any agent ever proved
    /// reachable by every agent holding that host's one token, which
    /// inverts the guarantee §4.4 exists to create. Nothing has to be
    /// invalidated either: the next reconcile pass rewrites
    /// `sessions.tmux_pane_id` and the next request resolves to `None` on
    /// its own.
    pub proven_session: Option<i64>,
    /// This scope's person is the ONLY live person on the hub
    /// ([`crate::store::Store::sole_enabled_person`]).
    ///
    /// Private, because it is a fact about the installation read at
    /// construction time and never something a call site may assert. It
    /// decides two things, and they are one rule: an `unclaimed` row is
    /// visible to that person (they could see it yesterday — rule 7, the
    /// upgrade widens nothing), and `HostRow.unclaimed_sessions` is served
    /// to them (R5-d).
    sole_person: bool,
    /// Whose hosts' unclaimed COUNTS this caller is served, beyond the
    /// one-person rule (org administration phase D, the owner's answers 1 and
    /// 3). Private for the reason `sole_person` is: a fact read at
    /// construction, never asserted by a call site.
    unclaimed: UnclaimedReach,
    /// The hosts a named Control API token is limited to (M15 step G2.8),
    /// `None` for every host. Checked before every other clause: a session
    /// on another host is invisible whoever owns it. Private: set only by
    /// [`Self::with_hosts`], which can only narrow.
    hosts: Option<Vec<String>>,
    /// Teammates whose sessions this person watches ([`TeamReach`]). Private
    /// for the reason `sole_person` is.
    team: TeamReach,
    /// This is the hub's own reader, not a caller.
    ///
    /// Private, and the reason the struct has no public literal form: "the
    /// hub does its work" must not be constructible by whoever has a
    /// `person: None` to hand. Only [`ViewScope::internal`] sets it.
    internal: bool,
}

impl ViewScope {
    /// The hub's own readers: GC, the reconcile pass, the playbooks,
    /// attention, `fleet_health` — everything that runs on a tick rather
    /// than for a caller, and that spec §3.3 requires to stay unscoped.
    ///
    /// It is a separate value from "a token that names nobody" on purpose.
    /// Before M1 those were the same thing (`OrgScope::All`), which is
    /// precisely how a person-less token would inherit the master's reach
    /// the moment people exist.
    pub fn internal() -> Self {
        ViewScope {
            org: OrgScope::All,
            person: None,
            grants: GrantSet::default(),
            host: None,
            proven_session: None,
            sole_person: false,
            unclaimed: UnclaimedReach::None,
            hosts: None,
            team: TeamReach::default(),
            internal: true,
        }
    }

    /// The constructor `Caller::view_scope` calls, and the only other one.
    ///
    /// `pub(crate)` rather than `pub` so the surface stays inside this
    /// crate, and taking every field positionally so that a new dimension
    /// added later breaks the one call site instead of defaulting there.
    #[allow(clippy::too_many_arguments)] // positional on purpose, see above
    pub(crate) fn for_caller(
        org: OrgScope,
        person: Option<i64>,
        grants: GrantSet,
        host: Option<String>,
        proven_session: Option<i64>,
        sole_person: bool,
        unclaimed: UnclaimedReach,
        team: TeamReach,
    ) -> Self {
        ViewScope {
            org,
            person,
            grants,
            host,
            proven_session,
            sole_person,
            unclaimed,
            hosts: None,
            team,
            internal: false,
        }
    }

    /// The same scope limited to `hosts` (a named token's host limit,
    /// G2.8); `None` leaves it as it is. Only ever narrows: a second limit
    /// keeps the hosts both name.
    pub fn with_hosts(mut self, hosts: Option<Vec<String>>) -> Self {
        if let Some(new) = hosts {
            self.hosts = Some(match self.hosts.take() {
                Some(old) => new.into_iter().filter(|h| old.contains(h)).collect(),
                None => new,
            });
        }
        self
    }

    /// Whether this scope may reach `host` at all: false only for a host a
    /// named token's limit leaves out.
    pub fn reaches_host(&self, host: &str) -> bool {
        self.hosts
            .as_ref()
            .is_none_or(|h| h.iter().any(|x| x == host))
    }

    /// Does this person watch `row` only as a teammate — in reach through
    /// [`TeamReach`], neither owned nor granted? The org page's Team panel
    /// names such a session; `count_member_sessions` counts it as open.
    pub fn watches_as_teammate(&self, row: &SessionRow) -> bool {
        self.team.covers(row.org_id, row.owner_person_id)
    }

    /// The same scope with its ORG half replaced — the one narrowing a
    /// caller's answer may legitimately take (work graph M5's landing-host
    /// reader in `service::work::resume`, which builds a brief only what the
    /// landing host's org may read).
    ///
    /// It can only change the org half: the person, the grants, the host
    /// binding and the pane proof travel unchanged, so this is never a way to
    /// become somebody else — and `internal` is not reachable from here
    /// either, since it is not an argument.
    pub fn with_org(mut self, org: OrgScope) -> Self {
        self.org = org;
        self
    }

    /// True for [`Self::internal`] — the hub's own reader.
    ///
    /// **It does not mean "unfenced".** A hub reader can be NARROWED by
    /// [`Self::with_org`] (`work::nudge`'s hook reader, `work::today`'s
    /// per-host reader, `work::resume`'s landing-host reader), and for such a
    /// scope the org boundary still applies — see [`Self::is_unrestricted`],
    /// which is the predicate a site wants when it is about to skip a fence
    /// having examined nothing.
    pub fn is_internal(&self) -> bool {
        self.internal
    }

    /// The hub's own reader, NOT narrowed to an org: the one scope for which
    /// skipping a fence outright is the whole truth.
    ///
    /// The distinction exists because `internal` alone was being read as
    /// "unfenced" at sites that had not looked at the org half at all, which
    /// made [`Self::with_org`] a no-op there (multi-user M1, the T6 review;
    /// the instance that mattered was [`Self::sees_session_facts`], whose
    /// internal clause sat above its org clause). A site may use
    /// [`Self::is_internal`] instead when the ORG half has demonstrably
    /// already been applied — `orgs::scope_links_for` runs `scope_links`
    /// first, `work::local::person_visible_links` runs `visible_links` first,
    /// `work::tidy::reopened` filters its rows by org above the early return —
    /// and then what it is skipping is only the PERSON fence, which is what
    /// internal honestly means.
    pub fn is_unrestricted(&self) -> bool {
        // This is the org boundary, not a privacy fence: it answers whether an
        // org narrowing EXISTS on this scope, never whether somebody may read
        // a row. The person half is untouched and still runs at every
        // caller-facing answer (`sees_session_row` and the verbs built on it);
        // what this predicate is for is the opposite direction — a site that
        // is about to skip a fence outright may only do so for the hub's own
        // reader that has NOT been narrowed.
        self.internal && self.org.is_all()
    }

    /// True when this scope's person is the only live person on the hub.
    /// See [`Self::sole_person`].
    pub fn is_sole_person(&self) -> bool {
        self.sole_person
    }

    /// Is this scope served the count of unclaimed sessions on a host in
    /// `host_org`? The hub's own reader and the one person of a one-person
    /// hub always (M1, R5-d); a host administrator — the hub's owner, or an
    /// admin of the company that owns the hub (owner's answer 1) — on every
    /// host; an org's admins on that org's hosts when the hub's owner turned
    /// `orgs.admins_see_unclaimed` on (answer 3). Nobody else: a count is
    /// still a claim about other people's sessions.
    pub fn sees_unclaimed_count(&self, host_org: Option<i64>) -> bool {
        if self.internal || self.sole_person {
            return true;
        }
        match &self.unclaimed {
            UnclaimedReach::None => false,
            UnclaimedReach::Every => true,
            UnclaimedReach::Orgs(orgs) => host_org.is_some_and(|o| orgs.contains(&o)),
        }
    }

    /// Does this scope's person OWN `row`?
    ///
    /// The ownership predicate, written once (spec §4.3, *The ownership
    /// predicate, written once*). Nothing else compares the two options
    /// directly, because the obvious spelling is a hole:
    ///
    /// ```text
    /// row.owner_person_id == self.person        // WRONG: Option == Option
    /// ```
    ///
    /// An `unclaimed` row has `None` and a person-less caller has `None`,
    /// so that line makes every person-less caller the owner of every
    /// unclaimed row — and therefore, by invariant 1, able to create grants
    /// on all of them. The `matches!` form cannot be read as an identity
    /// check, and it is the same shape `mcp/auth.rs::is_the_personal_owner`
    /// already uses for the same reason.
    ///
    /// [`Self::internal`] is deliberately **not** an owner: the hub's own
    /// readers reach what they need through [`Self::may_own`], and "the GC
    /// owns every session" is not a sentence anybody should be able to write
    /// against a grant.
    pub fn owns(&self, row: &SessionRow) -> bool {
        self.owns_person(row.owner_person_id)
    }

    /// May this scope act as the OWNER of a row a person owns outright,
    /// outside the session model — a mission (orchestration O1)?
    ///
    /// The clause order is [`Self::sees_session_row`]'s: the ORG boundary
    /// first, for everyone; then the hub's own reader; then the owner, by
    /// the one ownership comparison ([`Self::owns_person`]); and an unowned
    /// row only for the one live person on a single-person hub (rule 7, as
    /// for an `unclaimed` session). A person-less caller — a per-host token
    /// — is never an owner. Org membership, which widens READING a mission
    /// to the org's members and changing it to the org's admins, is the
    /// caller's to add: it needs the store (`service::work::missions`).
    pub fn may_own_person_row(&self, org: Option<i64>, owner: Option<i64>) -> bool {
        if !self.org.sees_org(org) {
            return false;
        }
        if self.internal || self.owns_person(owner) {
            return true;
        }
        owner.is_none() && self.sole_person
    }

    /// [`Self::owns`] over the column alone, so the kill frame's carried
    /// facts ([`SessionFacts`]) go through the same comparison a live row
    /// does. Private: a caller with an `Option<i64>` in hand and no row is
    /// exactly the shape that invites `row.owner == self.person`, which is
    /// the trap above.
    fn owns_person(&self, owner_person_id: Option<i64>) -> bool {
        matches!((owner_person_id, self.person), (Some(o), Some(p)) if o == p)
    }

    /// What this scope may learn about `row`.
    ///
    /// The order of the clauses is the rule:
    ///
    /// 1. the ORG boundary, for EVERYONE — a bound client, a per-host token
    ///    and a narrowed hub reader alike never reach outside their orgs,
    ///    whoever owns the row (work graph M5 / M14, composed, never
    ///    replaced). It used to sit *below* the internal clause, which made
    ///    [`ViewScope::with_org`] a no-op on this predicate: an
    ///    internal-and-narrowed scope — `work::nudge`'s hook reader,
    ///    `work::today`'s per-host reader, `work::resume`'s landing-host
    ///    reader, and every `org_only_view` test — answered
    ///    `RowAndContent` for every row on the fleet. `OrgScope::All` passes
    ///    this clause trivially, so the hub's own unnarrowed reader is
    ///    unaffected and nothing else can skip the boundary by also being
    ///    internal;
    /// 2. the hub's own reader then sees everything inside that boundary;
    /// 3. a per-host token then gets §4.4's two clauses and nothing else:
    ///    an `unclaimed` row on its OWN host (which is what makes the claim
    ///    path reachable at all), and the one row whose pane this request
    ///    proves (which is what keeps `whoami`, `register_self`,
    ///    `send_message`, `dispatch_task` and `work_link` working for the
    ///    agent inside a fleet-started — therefore `private` — session);
    /// 4. a person sees what they own, what they were granted, and — on a
    ///    single-person hub only — the `unclaimed` rows they could already
    ///    see before the upgrade;
    /// 5. everything else, including every caller that proves no person at
    ///    all, sees nothing.
    pub fn sees_session_row(&self, row: &SessionRow) -> Visibility {
        self.sees_session_facts(&SessionFacts::of(row))
    }

    /// [`Self::sees_session_row`] over the facts alone — **the rule's one
    /// body**, so that the row path and `/events`' `session:killed` frame
    /// (whose row is already deleted) cannot answer differently.
    ///
    /// The clause order is documented on [`Self::sees_session_row`], which
    /// is the name every call site should use when it has a row.
    pub fn sees_session_facts(&self, f: &SessionFacts<'_>) -> Visibility {
        // FIRST, and above the internal clause: see the clause order on
        // `sees_session_row`. `OrgScope::All` — the hub's own unnarrowed
        // reader, the master, a person's own device — passes it trivially, so
        // this costs nobody reach they had; what it ends is a scope that was
        // narrowed by `with_org` and then ignored the narrowing because it was
        // also internal.
        if !self.org.sees_session_org_only(f.host_alias, f.org_id) {
            return Visibility::None;
        }
        // A named token's host limit (G2.8): above the internal clause too,
        // though only a caller's scope ever carries one.
        if !self.reaches_host(f.host_alias) {
            return Visibility::None;
        }
        if self.internal {
            return Visibility::RowAndContent;
        }
        let unclaimed = f.visibility == VISIBILITY_UNCLAIMED;
        match &self.host {
            // §4.4, clauses 1 and 2. In particular NOT another person's
            // private session on the same machine: one token per host means
            // every Claude on it presents the same bearer, so the token
            // alone can never be the thing that tells them apart.
            Some(alias) => {
                if f.host_alias != *alias {
                    return Visibility::None;
                }
                // `Some(f.id)` on the right, so the `(None, None)` trap
                // has nowhere to live here either: a request that proves no
                // pane matches no row.
                if unclaimed || self.proven_session == Some(f.id) {
                    Visibility::RowAndContent
                } else {
                    Visibility::None
                }
            }
            None => {
                if self.owns_person(f.owner_person_id) || self.grants.level(f.id).is_some() {
                    return Visibility::RowAndContent;
                }
                // M15 step G2.10: a teammate in an org whose members see each
                // other's sessions. Below the org clause (a bound device
                // stays fenced) and read only (see `TeamReach`).
                if f.visibility != VISIBILITY_UNCLAIMED
                    && self.team.covers(f.org_id, f.owner_person_id)
                {
                    return Visibility::RowAndContent;
                }
                // Single-person installs keep their ROWS, not only a count.
                // With one person on the hub an `unclaimed` row is private
                // to nobody, and that person could see it yesterday: D1
                // promises nothing changed for a single user, and the
                // sidebar's Outside-fleet and orphan sections are built
                // entirely from rows whose `started_at IS NULL` — every one
                // of which T3's backfill deliberately left `unclaimed`.
                // `is_sole_person` is false for a person-less caller, so
                // this clause never widens one.
                if self.sole_persons_unclaimed_facts(f) {
                    return Visibility::RowAndContent;
                }
                Visibility::None
            }
        }
    }

    /// The one-person carve-out, as a predicate rather than three copies of
    /// a clause (multi-user M1; the visibility half is T6's, the two verbs
    /// below are T7's).
    ///
    /// With exactly one live person on the hub an `unclaimed` row is private
    /// to nobody, and that person could see it, prompt it and kill it
    /// yesterday: every reconcile-discovered row is `unclaimed` (the
    /// sidebar's whole Outside-fleet and orphan sections are built from
    /// `started_at IS NULL` rows, which T3's backfill deliberately left so),
    /// and D1 promises nothing changed for a single user.
    ///
    /// It had to spread from [`Self::sees_session_row`] to the verbs because
    /// the half-state it would otherwise create — you may SEE this row but
    /// not prompt it, and not remove it — is a state no single-user install
    /// has ever been in, and the claim path that would resolve it
    /// (`session_claim`) is the operator's, not something a tool call can
    /// do on the caller's behalf.
    ///
    /// It widens nobody else: [`Self::is_sole_person`] is false for a
    /// person-less caller (a per-host token, a device no pairing bound) and
    /// false the moment a second person exists on the hub, which is rule 7 —
    /// the upgrade widens nothing — read in the one direction it also has to
    /// be read, that the upgrade must not NARROW the installation that has
    /// always had one user.
    fn sole_persons_unclaimed(&self, row: &SessionRow) -> bool {
        self.sole_persons_unclaimed_facts(&SessionFacts::of(row))
    }

    /// [`Self::sole_persons_unclaimed`] over the facts alone; see
    /// [`Self::sees_session_facts`].
    fn sole_persons_unclaimed_facts(&self, f: &SessionFacts<'_>) -> bool {
        self.sole_person && f.visibility == VISIBILITY_UNCLAIMED
    }

    /// May this scope make `row`'s machine do work — `send_prompt`, and
    /// `send_message { deliver, submit }`, which is the same pane write by
    /// another route?
    ///
    /// The owner, a `drive` grantee, and the hub's own readers. **Not** a
    /// `watch` grantee: revision 3's deny list let a watcher deliver into a
    /// pane, which is a watch grant silently conferring drive.
    ///
    /// A per-host token drives what it can see — the agent in the pane is
    /// the session, and refusing it its own row breaks the agent-facing half
    /// of the product (§4.4). What it may not do is reach a row
    /// [`Self::sees_session_row`] already refuses it.
    pub fn may_drive(&self, row: &SessionRow) -> bool {
        if !self.sees_session_row(row).is_visible() {
            return false;
        }
        if self.internal || self.host.is_some() || self.owns(row) {
            return true;
        }
        // See `sole_persons_unclaimed`: the hub's only person keeps the
        // prompt box on the rows reconcile found for them.
        if self.sole_persons_unclaimed(row) {
            return true;
        }
        self.grants.level(row.id) == Some(crate::store::GRANT_DRIVE)
    }

    /// May this scope ANSWER the dialog on `row`'s pane — press a numbered
    /// option, Enter, Escape or Tab while the pane shows a dialog (Orbit
    /// Fleet 11.7)?
    ///
    /// Everyone [`Self::may_drive`] admits, plus an `answer` grantee. What an
    /// answer grantee may press, and the fresh pane read that proves a dialog
    /// is there, are `mcp::tools::messaging::send_prompt`'s: this function
    /// only answers who. A `watch` grantee is not here.
    pub fn may_answer(&self, row: &SessionRow) -> bool {
        self.may_drive(row)
            || (self.sees_session_row(row).is_visible()
                && self.grants.level(row.id) == Some(crate::store::GRANT_ANSWER))
    }

    /// May this scope perform an **owner-only** operation on `row`?
    ///
    /// `own` is a tier, not a grantable level (spec §4.3, invariant 5): no
    /// grant ever reaches it, so this is the owner and the hub's own
    /// readers, and nobody else. The membership of the tier — which tools
    /// it covers — is the spec's §4.3 invariant and is threaded by T7; this
    /// function only answers who.
    ///
    /// A per-host token is **not** an owner, proven pane or not. The pane
    /// proof is "I am standing in this session", which is worth exactly the
    /// reach §4.4 gives it; it is not a statement about whose session it is,
    /// and a tier that destroys, relocates, copies or re-shares the owner's
    /// work must never be reachable by a proof the deployment can hand to
    /// any process that can run `tmux list-panes`.
    pub fn may_own(&self, row: &SessionRow) -> bool {
        // Visibility FIRST, for everybody — the shape `may_drive` already
        // had, and the reason it is written this way here too (multi-user M1,
        // the T6 review): `if self.internal { return true }` as the opening
        // line answered `true` for a NARROWED hub reader as well, having
        // examined nothing about the org it was narrowed to, which made
        // `with_org` a no-op on this tier and on `sees_past_conversation`,
        // whose first arm is this predicate. A plain `ViewScope::internal`
        // passes the visibility check trivially (`OrgScope::All`), so this
        // costs the hub's own readers nothing.
        if !self.sees_session_row(row).is_visible() {
            return false;
        }
        if self.internal {
            return true;
        }
        // See `sole_persons_unclaimed`. A per-host token is excluded by
        // `is_sole_person` being false for it, which is what keeps the pane
        // proof from ever reaching this tier (the paragraph above).
        if self.sole_persons_unclaimed(row) {
            return true;
        }
        self.owns(row)
    }

    /// May this scope reach a PAST conversation — its transcript, a précis of
    /// it, a resume candidate built out of it (multi-user M1, T7)?
    ///
    /// Three answers, asked in this order, because two mechanisms exist and
    /// they disagree for an `unclaimed` row:
    ///
    /// 1. **A surviving `sessions` row decides**, through [`Self::may_own`] —
    ///    past work is the `own` tier (spec §4.3 invariant 5), and this arm
    ///    is what makes `work_link { resume | summarize }` answer the same
    ///    way `session_transcript` answers for the same row. Migration 099's
    ///    triggers record a `conversation_owners` row only `WHEN
    ///    NEW.owner_person_id IS NOT NULL`, so every reconcile-discovered
    ///    session has a real transcript and NO owner record: without this arm
    ///    the second person on a hub could summarise and resume it.
    /// 2. **Else the durable record decides** (`conversation_owners`), with
    ///    the `(None, None)` trap closed the way [`Self::owns`] closes it.
    /// 3. **Else nothing is recorded at all and nobody holds the id** — the
    ///    genuine pre-M1 conversation, whose row was reaped long ago. That
    ///    passes, which is rule 7 (the upgrade widens nothing, and must not
    ///    narrow a single-person install into uselessness).
    ///
    /// A failed read is never a pass: the error propagates.
    pub fn sees_past_conversation(
        &self,
        s: &crate::store::Store,
        claude_id: &str,
    ) -> Result<bool, crate::ipc_error::IpcError> {
        let rows = s.sessions_by_claude_id(claude_id)?;
        if !rows.is_empty() {
            // Two rows sharing one id has been observed live: every one of
            // them has to be this caller's, or the conversation is not.
            //
            // The hub's own reader is NOT short-circuited above this: it
            // passes through `may_own`, which answers `true` for it — and now
            // also applies the ORG boundary, so a NARROWED hub reader
            // (`with_org`) is fenced by its org here exactly as it is on a
            // live row. Short-circuiting on `internal` alone, which is what
            // this did, made `with_org` a no-op on the one path that reaches
            // a transcript.
            return Ok(rows.iter().all(|r| self.may_own(r)));
        }
        // Below this line there is no row — and therefore no host and no org —
        // to judge, so `internal` is the whole answer rather than a skipped
        // fence. The hub's own readers (GC, the playbooks, a summary) must
        // reach a conversation whose session is long reaped, and they prove no
        // person, so the `conversation_owners` arm below would refuse them
        // every time.
        if self.internal {
            return Ok(true);
        }
        Ok(match s.conversation_owner(claude_id)? {
            None => true,
            Some(o) => matches!(self.person, Some(p) if p == o),
        })
    }
}

// ---- the result gate's session half (multi-user M1, T8) -------------------
//
// A second `impl` block, kept apart from the predicates above on purpose: the
// rules are up there, and this is the one place that applies them to BYTES
// (serialised JSON) rather than to a `SessionRow`. The gate that drives it is
// `mcp/tools/support.rs::fence_result_via`, under `call_tool`.

impl ViewScope {
    /// Drop from one serialised tool result every session row this scope may
    /// not see — the session half of spec §5.1's **choke point 3**, the last
    /// net under EVERY tool answer (multi-user M1, T8).
    ///
    /// The org half of the same gate ([`OrgScope::redact_json`]) deletes
    /// KEYS: a row whose work belongs to another company keeps its identity
    /// and loses its work fields. That is the right answer for work, and the
    /// wrong one for a private session — there is no subset of a session row
    /// an out-of-scope caller may have, because a session's metadata *is* its
    /// content (spec §4.3, *What counts as content*; and this module's
    /// header on why [`Visibility`] has no middle arm). So this half removes
    /// the row: out of its enclosing array, or replaced by `null` where it
    /// stood as one object's field.
    ///
    /// **Why a gate at all, when every tool is already gated.** T6 and T7
    /// put the rule in `list_sessions`' filter and in `resolve_row_and_gate`,
    /// and those are where a refusal should come from — with the words, the
    /// code and the audit row a refusal needs. This runs after all of them
    /// and knows nothing about which tool answered: it is the net under a
    /// tool somebody adds in a year's time, in a crate that has forgotten
    /// people exist, which happens to serialise a row it was handed. It
    /// cannot refuse such a call (it has no idea what the call was) and does
    /// not try to; it only makes sure the row is not in the bytes.
    ///
    /// `sees` answers for one row-shaped object; the gate builds it over the
    /// **store**, by id, never over the payload — see
    /// [`looks_like_session_row`] and `mcp/tools/support.rs`'s
    /// `visibility_resolver`. A projection that dropped `visibility` cannot
    /// make a private row look unclaimed, exactly as the org half's
    /// `session_org` keeps a projection that dropped `org_id` from making a
    /// row look unassigned.
    ///
    /// [`OrgScope::redact_json`]: crate::service::orgs::OrgScope::redact_json
    pub fn drop_invisible_rows(
        &self,
        v: &mut serde_json::Value,
        sees: &dyn Fn(&serde_json::Map<String, serde_json::Value>) -> Visibility,
    ) {
        // The hub's own readers see everything, and a value built by
        // `internal()` never reaches a result gate in the first place (the
        // gate builds its scope from a `Caller`). Stated anyway, so that the
        // rule lives in the same place for every method on this type: the
        // early return is "the hub does its work", and it is NOT the old
        // `OrgScope::is_all` one, which was also true for an unbound paired
        // client — the caller this gate exists for.
        //
        // `is_unrestricted`, not `is_internal`: a NARROWED hub reader has an
        // org boundary, and a gate that skipped the walk for it would be the
        // same shape as the bug `sees_session_facts` had — a fence skipped
        // having examined nothing. The walk is what applies it.
        if self.is_unrestricted() {
            return;
        }
        drop_rows(v, sees);
    }
}

/// Fail closed, with no scope at all: every row-shaped object goes.
///
/// The gate's two unjudgeable cases — a poisoned store lock, and a scope
/// that could not be built — have no [`ViewScope`] to ask and must not
/// invent a permissive one. "Every row" is the only answer available, and it
/// is the safe one: a result that keeps no session row leaks none.
pub fn drop_every_session_row(v: &mut serde_json::Value) {
    drop_rows(v, &|_| Visibility::None);
}

/// Does this JSON object look like a session row?
///
/// Three clauses, because a row reaches the wire in three shapes and every
/// one of them is row enough to leak:
///
/// * a serialised [`SessionRow`] carries `visibility`, which is `NOT NULL`
///   in the schema and therefore survives `strip_nulls` — the one key a
///   fence may rely on (see [`SessionRow::owner_person_id`]'s note on why
///   `owner_person_id` is not that key);
/// * the summaries and projections carry no `visibility` at all —
///   `SessionSummary` (`list_sessions`' default answer) and the `phone` view
///   are both `id` + `host_alias` + `tmux_name` plus columns a screen draws;
/// * and the projections that name the session `session_id` instead of `id`,
///   which is most of the ones built OUT of a row rather than from it:
///   `SessionUsage` (`usage_report`: tmux name, friendly name, model and
///   per-session spend), `TidyCandidate`, `ReviewItem`, `RestorePlanEntry`.
///   This clause was missing, and the gate is the net under exactly the tool
///   "somebody adds in a year's time which happens to serialise a row it was
///   handed" — a tool like that is at least as likely to spell the key
///   `session_id`.
///
/// Deliberately generous: a shape this recognises and the store cannot
/// resolve is DROPPED, not kept. Nothing else in a fleet result carries all
/// three keys — a worktree row has `host_alias` and no `tmux_name`, a host
/// row names itself `alias` — and a false positive costs a row in an answer,
/// where a false negative costs somebody's private session.
///
/// [`session_row_id`] is the matching half: which key the resolver reads.
pub fn looks_like_session_row(map: &serde_json::Map<String, serde_json::Value>) -> bool {
    map.contains_key("visibility")
        || (map.contains_key("host_alias")
            && map.contains_key("tmux_name")
            && (map.contains_key("id") || map.contains_key("session_id")))
}

/// The session id a row-shaped object names: `id`, else `session_id`.
///
/// The two spellings in one place, so [`looks_like_session_row`] and the
/// resolver that judges what it recognised cannot drift one key apart — which
/// is how `SessionUsage` walked under the gate: recognised by neither, since
/// the predicate wanted `id` and the resolver read `id`.
pub fn session_row_id(map: &serde_json::Map<String, serde_json::Value>) -> Option<i64> {
    map.get("id")
        .or_else(|| map.get("session_id"))
        .and_then(serde_json::Value::as_i64)
}

/// Is `v` a row-shaped object `sees` refuses?
fn invisible_row(
    v: &serde_json::Value,
    sees: &dyn Fn(&serde_json::Map<String, serde_json::Value>) -> Visibility,
) -> bool {
    v.as_object()
        .is_some_and(|m| looks_like_session_row(m) && !sees(m).is_visible())
}

/// [`ViewScope::drop_invisible_rows`] and [`drop_every_session_row`] over one
/// value, including the value itself: a tool whose whole answer is one row
/// answers `null` rather than the row.
fn drop_rows(
    v: &mut serde_json::Value,
    sees: &dyn Fn(&serde_json::Map<String, serde_json::Value>) -> Visibility,
) {
    if invisible_row(v, sees) {
        *v = serde_json::Value::Null;
        return;
    }
    prune_rows(v, sees);
}

/// The recursive half: every array loses its invisible rows, every object
/// field holding one is nulled, and everything kept is walked into — a row a
/// caller may see can itself carry a list of rows (`related_sessions`).
fn prune_rows(
    v: &mut serde_json::Value,
    sees: &dyn Fn(&serde_json::Map<String, serde_json::Value>) -> Visibility,
) {
    match v {
        serde_json::Value::Array(items) => {
            items.retain_mut(|item| {
                // Removed outright: an array is a list, and a list with a
                // hole in it is a shorter list. A `null` element here would
                // break every consumer that deserialises the array into
                // `Vec<SessionRow>`.
                if invisible_row(item, sees) {
                    return false;
                }
                prune_rows(item, sees);
                true
            });
        }
        serde_json::Value::Object(map) => {
            for (_, child) in map.iter_mut() {
                // A named field cannot simply vanish without changing the
                // shape of the answer, so it becomes `null` — which is what
                // "there is no such session for you" has always looked like
                // on this wire (`strip_nulls` is upstream of the gate, so a
                // null the gate writes stays).
                if invisible_row(child, sees) {
                    *child = serde_json::Value::Null;
                } else {
                    prune_rows(child, sees);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "view_scope_tests.rs"]
mod tests;

/// A [`ViewScope`] that fences by ORG only — the hub's own reader
/// ([`ViewScope::internal`]) narrowed to `org`.
///
/// Test-only, and that is the point: it is the value a test means when it
/// wants the pre-M1 behaviour of a function that has since been given the
/// whole scope ("this org, and no person fence"). Production code must build
/// its scope from the caller (`Caller::view_scope`) or state that it is the
/// hub's own reader; a production call site that wants "an org and nobody"
/// is the bug this type exists to make unwritable.
#[cfg(test)]
pub(crate) fn org_only_view(org: &OrgScope) -> ViewScope {
    ViewScope::internal().with_org(org.clone())
}
