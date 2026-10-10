//! **Guard (multi-user M1, T9c): every `OrgScope::is_all` / `Caller::is_scoped`
//! check in production is classified, and the classification cannot rot.**
//!
//! **What this test proves, first, so nobody reads the table as more than it
//! is: COMPLETENESS and FRESHNESS, not TRUTH.** It proves that no guard in
//! the three spellings and three crates it scans is unlisted, that a changed
//! guard condition fails here loudly, that no row names a guard that no
//! longer exists, and that an `OrgBoundary` claim is written in the code
//! beside the guard it describes. It does NOT prove a row's verdict is
//! right: [`Verdict`] has two variants and one of them,
//! [`Verdict::Fence`], is an unconditional failure, so **in any green tree
//! every row is [`Verdict::OrgBoundary`] by construction** and that column
//! carries no information. Nor does it prove a row's reason: [`MARKER`] is a
//! substring match over a comment, which a reader can satisfy by pasting the
//! sentence, and `why` is otherwise read only inside this test's own failure
//! messages. One class of untruth IS mechanised — a `why` that names SOME of
//! a guard's call sites must name them ALL
//! ([`a_row_that_names_call_sites_names_all_of_them`]), which is the check
//! that two rounds of undercounting the `local_item_visible` sites earned.
//! Everything else in a `why` is reviewed by people only. Where a guard's
//! label actually matters, give it a PROOF rather than a better adjective:
//! `VIEW_SCOPE_PROOF` in `crate::mcp::tools::tests` is the mechanism that
//! works for a privacy-relevant claim — a row names two behavioural tests,
//! and a test either passes or fails.
//!
//! The shape is `if the caller is restricted, then check` — and it has now
//! produced a privacy hole **five times** in this milestone, because the
//! caller M1 introduces (a person's own device, a paired client bound to no
//! org) is not "restricted" by either predicate: its `OrgScope` is
//! [`OrgScope::All`] and its `Caller::is_scoped()` is `false`, exactly like the
//! master's. Found and fixed that way, one per round:
//! `require_visible_session`, `require_bound_client_sees`, `redact_work_via`'s
//! invocation, `/events`' `rescope`, and `handover.rs`'s journal fence.
//!
//! Some of those checks are **right**: an org-authority question ("may this
//! caller move an org, write this key, read another company's tracker") is
//! correctly org-only, because that authority *is* an org question. Others are
//! **privacy fences**, and for those the predicate is simply wrong.
//!
//! So each one is classified here, by name, with a reason. Two rules make the
//! table worth reading:
//!
//! 1. **The list is derived by SCANNING the source, never hand-copied.** A
//!    hand list is the thing that rots: the thirty-third guard would be added
//!    and nobody would know. [`every_scope_guard_is_classified`] walks the
//!    same production files `no_eprintln_tests` walks (skipping test-only
//!    modules the same way), finds every guard, and requires each to have
//!    exactly one row — and every row to match a guard that still exists.
//! 2. **An [`Verdict::OrgBoundary`] row has to say so IN THE CODE, in words a
//!    reader can check.** The sentence is *"This is the org boundary, not a
//:    privacy fence"*, and [`MARKER`] is the part of it this test looks for,
//!    in the comment attached to the guard or in the enclosing function's doc.
//!    If that sentence cannot honestly be written, the guard is a fence —
//!    which is the whole test: the classification is not a checkbox, it is a
//!    claim somebody has to be willing to write down next to the code.
//!
//! A row in [`OPEN_QUESTIONS`] is the third thing a guard can carry: the org
//! claim is honest AND a narrower question is open that the eight rules do
//! not settle (does a person's own device get this?). The table holds such a
//! row to a classified `OrgBoundary` guard whose comment calls the question
//! an owner decision, so it is visible in the classification and not only in
//! prose beside it.
//!
//! A [`Verdict::Fence`] row **fails this test**. It is not a resting place: it
//! is how a reviewer records "this guard is a privacy fence and the person is
//! missing from it" so that the suite is red until it is fixed or honestly
//! reclassified.
//!
//! **What the scan covers, exactly** — because rule 1 is worth only what it
//! covers, and the first version of it claimed more than it delivered:
//!
//! * the three spellings in [`is_scope_guard`] — `.is_all()`,
//!   `.is_scoped()`, and the discriminant in a pattern
//!   (`OrgScope::All =>`, `matches!(…, OrgScope::All)`). The third was
//!   added in T9d, when it turned out twelve production sites were written
//!   that way, including the parent of an already-classified guard;
//! * `crates/fleet-core/src`, `crates/fleet-hub/src` and `src-tauri/src`,
//!   skipping test-only modules the way `no_eprintln_tests` does.
//!   `fleet-agent` depends on `fleet-proto` only, so it cannot hold one.
//!
//! **What it cannot see**, said plainly so nobody reads this module as a
//! proof of completeness it does not give:
//!
//! * a guard behind a NAME. Hoist the shortcut into a helper —
//!   `fn restricted(s: &OrgScope) -> bool { !s.is_all() }` — and the scan
//!   finds ONE guard (inside the helper) while every call site reads
//!   `if restricted(scope) {` and is invisible. The partial defence is that
//!   such a helper's own body IS a site, so it needs a row and the row's
//!   `why` has to account for its callers;
//! * a predicate that reaches the same decision without naming either
//!   method or the discriminant (comparing `OrgScope`'s serialised form,
//!   say);
//! * whether a row's reason is TRUE, beyond the one check described above.
//!   T9c's table was written in one pass and one row —
//!   `local_item_visible` — claimed a person gate that was on only one of
//!   the two writes it named; T9d fixed the gate and the row still named
//!   two of three call sites. The first of those two needed a reader. The
//!   second is now [`a_row_that_names_call_sites_names_all_of_them`],
//!   because "which sites call this guard's function" is a fact the same
//!   scan can establish. A claim about what a DIFFERENT function does —
//!   which is most of what a `why` says — is still a reader's job, which is
//!   why the `why` is a sentence and not a checkbox.
//!
//! What it DOES prove is that no guard in those three spellings and those
//! three crates is unaccounted for, that every org-authority claim is
//! written beside the code it describes, and that adding the next guard is a
//! test failure rather than a silent leak.
//!
//! **A second table, same discipline, different predicate** (multi-user M1,
//! T10): [`ORG_HALF_SITES`] does for `OrgScope`'s two org-only SESSION
//! predicates (`sees_row_org_only` / `sees_session_org_only`) what this one
//! does for `is_all()` — every production CALL is derived from the source,
//! must have a row, and must NAME the person predicate that finishes the
//! fence ([`PERSON_PREDICATES`]) in the code beside it. It exists because
//! T10's plan said to DELETE those two predicates, the deletion being the
//! completeness proof, and the deletion turned out to be the wrong
//! instrument: `ViewScope::sees_session_facts` **is** their composition, so
//! the org question has to stay expressible for the person fence to exist at
//! all. A table plus a scan gives the same completeness continuously instead
//! of once. See [`ORG_HALF_SITES`] for the whole argument.
//!
//! [`OrgScope::All`]: crate::service::orgs::OrgScope::All

use crate::no_eprintln_tests::{rs_files, scan_with};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The part of the canonical sentence this test looks for, lowercased.
///
/// Short on purpose: the full form ("This is the org boundary, not a privacy
/// fence") is what an author should write, and several of these guards were
/// already carrying their own longer version of it before this table existed
/// (`structure.rs`'s `org_impact`: "is not, and never was, a privacy fence").
/// Matching the shared tail keeps those intact instead of making everybody
/// paste one phrasing.
const MARKER: &str = "not a privacy fence";

/// What one guard is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// An ORG-authority question, correctly asked of the org alone: may this
    /// caller act on / read this COMPANY's data. The person dimension does
    /// not belong in it, and the row's code must carry [`MARKER`].
    OrgBoundary,
    /// A PRIVACY fence — so the predicate is wrong, because a person's own
    /// device is not "restricted" by it. **Fails the test**: fix the guard
    /// (thread a `ViewScope`, move the person half out of the block) or, if
    /// the claim turns out to be an org one after all, reclassify it and
    /// write the sentence.
    ///
    /// `#[allow(dead_code)]`: no row uses it today — the fifth instance of the
    /// class was fixed rather than recorded — and the variant must stay
    /// constructible all the same, because it is the vocabulary a reviewer
    /// needs to say "this one is a fence" and leave the suite red. Deleting it
    /// would leave only one way to classify a guard, which is no
    /// classification at all.
    #[allow(dead_code)]
    Fence,
}

/// One classified guard.
///
/// Keyed by `(file, func, nth)` rather than by line number, because line
/// numbers rot on the first edit above them. `nth` is the 0-based occurrence
/// within that function, so the key only moves when a guard is added to or
/// removed from the same function — which is exactly when the table has to be
/// revisited anyway. `code` is the guard line as it is written, so a change to
/// the condition fails here loudly instead of inheriting an old verdict.
#[derive(Debug)]
struct Guard {
    file: &'static str,
    func: &'static str,
    nth: usize,
    code: &'static str,
    verdict: Verdict,
    /// Why, for a reader. The code's own comment is what carries the claim;
    /// this is the one-line index to it.
    why: &'static str,
}

/// Every `is_all()` / `is_scoped()` guard in production, classified.
///
/// Ordered by file, as the scan finds them.
const SCOPE_GUARDS: &[Guard] = &[
    // ---- service/runs.rs ------------------------------------------------
    Guard {
        file: "crates/fleet-core/src/service/runs.rs",
        func: "reach",
        nth: 0,
        code: "let spend = scope.org.is_all() && sessions.len() == all.len();",
        verdict: Verdict::OrgBoundary,
        why: "whether the runs that belong to no session or mission (Jev, a \
              summary of a conversation no session holds) are listed: never \
              to a reader narrowed to an org. The PERSON half is the \
              conjunct beside it — every session there is passed \
              `sees_session_row` — so a person's own device that sees every \
              session is served them, and one that does not is not",
    },
    // ---- service/messages.rs -------------------------------------------
    Guard {
        file: "crates/fleet-core/src/service/messages.rs",
        func: "send_message_scoped",
        nth: 0,
        code: "if !scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "which ORG's session an address may name; the PERSON half of the \
              same recipient is `support::require_message_recipient` at the \
              tool layer, at Reach::Drive",
    },
    Guard {
        file: "crates/fleet-core/src/service/messages.rs",
        func: "send_message_scoped",
        nth: 1,
        code: "if !scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "the same org question for a recipient named by id",
    },
    // ---- service/view_scope.rs ------------------------------------------
    Guard {
        file: "crates/fleet-core/src/service/view_scope.rs",
        func: "is_unrestricted",
        nth: 0,
        code: "self.internal && self.org.is_all()",
        verdict: Verdict::OrgBoundary,
        why: "the predicate a site uses when it is about to skip a fence \
              having examined nothing: \"the hub's own reader, and not \
              narrowed by `with_org`\". It reads the org half to answer \
              whether an org narrowing EXISTS, which is an org-authority \
              question and nobody's access decision — every caller-facing \
              answer still goes through `sees_session_row`, person half and \
              all (multi-user M1, the T6 review)",
    },
    // ---- service/orgs.rs -----------------------------------------------
    Guard {
        file: "crates/fleet-core/src/service/orgs.rs",
        func: "is_all",
        nth: 0,
        code: "matches!(self, OrgScope::All)",
        verdict: Verdict::OrgBoundary,
        why: "the PREDICATE ITSELF, and where the whole class comes from: \
              `All` is \"no ORG fence\", which is not \"no fence\". Classified \
              so the definition carries the sentence every reader this table \
              classifies inherits",
    },
    Guard {
        file: "crates/fleet-core/src/service/orgs.rs",
        func: "host",
        nth: 0,
        code: "OrgScope::All | OrgScope::Org { .. } => None,",
        verdict: Verdict::OrgBoundary,
        why: "an accessor: which HOST this scope is bound to, which only a \
              per-host token has",
    },
    Guard {
        file: "crates/fleet-core/src/service/orgs.rs",
        func: "sees_org",
        nth: 0,
        code: "OrgScope::All => true,",
        verdict: Verdict::OrgBoundary,
        why: "the org boundary itself, asked of an ORG id and nothing else",
    },
    Guard {
        file: "crates/fleet-core/src/service/orgs.rs",
        func: "sees_session_org_only",
        nth: 0,
        code: "OrgScope::All => true,",
        verdict: Verdict::OrgBoundary,
        why: "a SESSION predicate, and the `_org_only` in its name is the \
              contract. SEVEN production call sites, each with its own person \
              half beside it: `sees_session_facts` (the person half is the \
              rest of that function), `sees_row_org_only` (a row-shaped \
              wrapper, org-only by its own name), `link_session_visible` \
              (whose person half is `link_person_visible`), `link_visible` \
              (where `link_hidden` has already run), \
              `person_sees` — the body of `require_person_sees`, and of the \
              T11 long-poll re-check that shares it — where \
              `sees_session_row` has already refused, and this only decides \
              whether the refusal may say E_PANE_UNPROVEN, and \
              `visible` (file downloads), whose person half is `may_own` in \
              the other arm of the same match: this call is the \
              session-is-GONE arm, where no person is left to ask and \
              `DownloadRow.org_id` is the whole of the fence, and \
              `visible_with` (the Library), the same session-is-GONE arm \
              with `may_own` beside it and `LibraryItemRow.org_id` as the \
              fence",
    },
    Guard {
        file: "crates/fleet-core/src/service/orgs.rs",
        func: "scope_links",
        nth: 0,
        code: "OrgScope::All => {}",
        verdict: Verdict::OrgBoundary,
        why: "the ORG-only link pager, and the PARENT of the classified \
              `link_session_visible` guard. THREE call sites: \
              `scope_links_for` is the person half every link-page READ \
              takes (it composes this with `link_person_visible`), while \
              `require_key` and `require_key_bound` are key AUTHORITY \
              checks that read no link out — they ask only whether the key \
              has any work in scope, and T10 DECIDED them: they answer an \
              org question for a scoped caller and `Ok(())` unconditionally \
              for every other, so the key-level open question belongs \
              wholly to `work_purge_impact`. \
              Spelled as a match arm, which is why T9d taught the scan the \
              discriminant",
    },
    Guard {
        file: "crates/fleet-core/src/service/orgs.rs",
        func: "redact_row_org_only",
        nth: 0,
        code: "if self.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "the org redaction, org-only by its own name (`_org_only`)",
    },
    Guard {
        file: "crates/fleet-core/src/service/orgs.rs",
        func: "redact_json",
        nth: 0,
        code: "if self.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "the same redaction over serialised output",
    },
    Guard {
        file: "crates/fleet-core/src/service/orgs.rs",
        func: "link_session_visible",
        nth: 0,
        code: "if scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "the org half of a link's session fence; `link_person_visible` is \
              the person half, and `scope_links_for` composes the two",
    },
    Guard {
        file: "crates/fleet-core/src/service/orgs.rs",
        func: "visible_item_for_key",
        nth: 0,
        code: "if scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "which org's ITEM a shared key resolves to; an item is not a \
              session",
    },
    Guard {
        file: "crates/fleet-core/src/service/orgs.rs",
        func: "org_item_for_key",
        nth: 0,
        code: "if scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "the same item question with no host fence",
    },
    Guard {
        file: "crates/fleet-core/src/service/orgs.rs",
        func: "org_suggestions",
        nth: 0,
        code: "if !scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "only a caller that may CREATE an org is offered one; the person \
              half runs below, on every row the suggestion counts",
    },
    // ---- service/health.rs ----------------------------------------------
    Guard {
        file: "crates/fleet-core/src/service/health.rs",
        func: "scope_sees_tracker",
        nth: 0,
        code: "OrgScope::All => true,",
        verdict: Verdict::OrgBoundary,
        why: "which org's TRACKER appears in `fleet_health`; a \
              `TrackerHealth` is a name, a site and a sync error, never a \
              session",
    },
    // ---- service/usage.rs ----------------------------------------------
    Guard {
        file: "crates/fleet-core/src/service/usage.rs",
        func: "report_on",
        nth: 0,
        code: "let visible: Option<std::collections::BTreeSet<String>> = (!scope.is_all()).then(|| {",
        verdict: Verdict::OrgBoundary,
        why: "which HOSTS' daily roll-up this caller reads; the per-session \
              rows below take `view.sees_session_row` as well",
    },
    // ---- service/trackers/tickets.rs ------------------------------------
    Guard {
        file: "crates/fleet-core/src/service/trackers/tickets.rs",
        func: "lookup",
        nth: 0,
        code: "Err(e) if e.code == codes::E_NOTFOUND && !scope.is_all() => {",
        verdict: Verdict::OrgBoundary,
        why: "which tracker SITES are connected is an org question",
    },
    Guard {
        file: "crates/fleet-core/src/service/trackers/tickets.rs",
        func: "lookup",
        nth: 1,
        code: "(_, false) if !scope.is_all() => {",
        verdict: Verdict::OrgBoundary,
        why: "which org's cache answers a bare key when two sites share it",
    },
    Guard {
        file: "crates/fleet-core/src/service/trackers/tickets.rs",
        func: "lookup",
        nth: 2,
        code: "if !scope.is_all() && !tracker.as_ref().is_some_and(|t| scope.sees_org(t.org_id)) {",
        verdict: Verdict::OrgBoundary,
        why: "which tracker this caller may make the hub fetch from",
    },
    Guard {
        file: "crates/fleet-core/src/service/trackers/tickets.rs",
        func: "item_visible",
        nth: 0,
        code: "OrgScope::All => Ok(true),",
        verdict: Verdict::OrgBoundary,
        why: "which org's work ITEM this is; the session ids hung off it are \
              person-fenced in `tickets::live_ids`, which takes the whole \
              `ViewScope`",
    },
    Guard {
        file: "crates/fleet-core/src/service/trackers/tickets.rs",
        func: "lookup",
        nth: 3,
        code: "OrgScope::All | OrgScope::Org { .. } => d,",
        verdict: Verdict::OrgBoundary,
        why: "whether a per-host token gets a TICKET's description trimmed; \
              the tracker's own text, which names no session",
    },
    // ---- service/work/handover.rs ---------------------------------------
    Guard {
        file: "crates/fleet-core/src/service/work/handover.rs",
        func: "gather_stored",
        nth: 0,
        code: "if !reader.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "which org's links go into a brief. This was instance FIVE of the \
              class: the journal fence used to sit inside this block and is \
              now unconditional, below it",
    },
    // ---- service/work/local.rs ------------------------------------------
    Guard {
        file: "crates/fleet-core/src/service/work/local.rs",
        func: "visible_links",
        nth: 0,
        code: "if scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "documented as the ORG half. Two callers: `person_visible_links` \
              is the half every READ of a count takes on top of it, and \
              `local_item_visible` keeps this org half because what it \
              answers is about the ITEM",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/local.rs",
        func: "local_items",
        nth: 0,
        code: "if !scope.is_all() && links.is_none() {",
        verdict: Verdict::OrgBoundary,
        why: "whether the ITEM itself is listed; its title and key are item \
              data, and the count beside it is person-fenced",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/local.rs",
        func: "local_item_visible",
        nth: 0,
        code: "if scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "\"does this ITEM exist for this caller\". THREE call sites, and \
              the count has been wrong twice: the two WRITES — \
              `rename_local_item` and `status::set_status` — carry the person \
              fence `require_drive_on_item_sessions` at the tool layer (T9c's \
              row claimed both while the gate was on the rename half only; \
              T9d put it on `set_status`, proved by \
              `a_local_items_status_is_not_another_persons_to_set`), and the \
              third, `name_session_work_as`, has NO item-level gate — it is \
              fenced by the caller already driving its own session, and the \
              only item-derived output under this shortcut is the `item_id` \
              in an `E_EXISTS` reply, an id `work { local_items }` lists to \
              everyone (T9e, sentence written at the guard). The gate itself \
              no longer passes an item with no CONFIRMED link: \
              `an_unlinked_local_item_is_nobodys_to_rename_or_set`. A FOURTH \
              caller arrived with shared work context: `visible_parent`, which \
              asks it about the PARENT a `create` / `propose` names — an item \
              question again, and its own rows below say so. A FIFTH arrived \
              with sprints and releases: `buckets::item_visible`, which asks \
              whether a bucket's member ITEM is the caller's to see or plan \
              (the planning write carries `require_drive_on_item_sessions` at \
              the tool layer, as `set_status` does). A SIXTH arrived with task \
              editing: `edit_local_item`, a write like the rename, behind the \
              same `require_drive_on_item_sessions` at the tool layer. A \
              SEVENTH arrived with epics: `set_parent`, which files the ITEM \
              under a parent, a write behind that same tool-layer gate",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/local.rs",
        func: "set_parent",
        nth: 0,
        code: "if !scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "this is the org boundary, not a privacy fence: it asks whether \
              the PARENT ITEM a task is filed under exists for this caller, \
              through `visible_parent` — `create_task`'s question, on an \
              item's key and title, which are work data",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/local.rs",
        func: "create_task",
        nth: 0,
        code: "None if !scope.is_all() => {",
        verdict: Verdict::OrgBoundary,
        why: "this is the org boundary, not a privacy fence: a STANDALONE task \
              has no links and no sessions, so there is no row and no person to \
              fence — the refusal is about authority to add top-level work, \
              which only an unscoped caller (the desktop, the master) has",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/local.rs",
        func: "create_task",
        nth: 1,
        code: "Some(p) if !scope.is_all() => visible_parent(&s, scope, p)?,",
        verdict: Verdict::OrgBoundary,
        why: "this is the org boundary, not a privacy fence: it asks whether \
              the PARENT ITEM exists for this caller, through \
              `local_item_visible` / `item_visible` — the same item question \
              those rows classify, on an item's key and title, which are work \
              data and survive the person fence exactly as `task_visible`'s do",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/missions.rs",
        func: "create",
        nth: 0,
        code: "None if !scope.org.is_all() => {",
        verdict: Verdict::OrgBoundary,
        why: "`create_task` #0's rule for a mission's new ROOT task: a \
              standalone native task has no links and no sessions, so there is \
              no row and no person to fence. This is the org boundary, not a \
              privacy fence; the mission itself is person-fenced by \
              `ViewScope::may_own_person_row` and org membership",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/local.rs",
        func: "propose_tree",
        nth: 0,
        code: "if !scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "`propose` #0's parent-item question, asked of a tree's parent and \
              of every existing item an entry waits for. This is the org \
              boundary, not a privacy fence; the person half is at the tool \
              layer, where the proposing session passes `resolve_target_row` at \
              `Reach::Drive`",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/graph.rs",
        func: "person_decides",
        nth: 0,
        code: "if !scope.org.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "`decide` #0's rule for many proposals at once: deciding is a \
              PERSON's act by design, so every scoped caller is refused before \
              any row is reached. This is the org boundary, not a privacy \
              fence; each item's mission is then fenced by owner and org \
              membership (`graph::require_mission_change`)",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/local.rs",
        func: "propose",
        nth: 0,
        code: "if !scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "the same parent-item question as `create_task` #1, for a \
              proposed subtask. This is the org boundary, not a privacy fence; \
              the person half of a proposal is at the tool layer, where the \
              session that gets the credit passes `resolve_target_row` at \
              `Reach::Drive` before `proposer` is built",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/local.rs",
        func: "decide",
        nth: 0,
        code: "if !scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "this is the org boundary, not a privacy fence: deciding a \
              proposal is a PERSON's act by design (an agent never accepts its \
              own), so every scoped caller — per-host token and bound client \
              alike — is refused outright and no row is reached to fence",
    },
    // ---- service/work/describe.rs ---------------------------------------
    Guard {
        file: "crates/fleet-core/src/service/work/describe.rs",
        func: "fence_for_scope",
        nth: 0,
        code: "OrgScope::All | OrgScope::Org { .. } => body.to_string(),",
        verdict: Verdict::OrgBoundary,
        why: "the same trim as `tickets::lookup`'s, on the same datum: one \
              tracker item's description",
    },
    // ---- service/work/mod.rs --------------------------------------------
    Guard {
        file: "crates/fleet-core/src/service/work/mod.rs",
        func: "work_purge_impact",
        nth: 0,
        code: "if !scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "which KEYS a purge may name; `PurgeImpact` carries keys and no \
              session row",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/mod.rs",
        func: "work_link_locked",
        nth: 0,
        code: "if !scope.is_all() && visible_link(link_id).is_err() {",
        verdict: Verdict::OrgBoundary,
        why: "a forced cross-ORG link on a session this caller already reached \
              through the tool layer's `Reach::Drive` person gate",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/mod.rs",
        func: "work_link_locked",
        nth: 1,
        code: "let only = if scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "which of the session's links an archive stamps, by org",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/mod.rs",
        func: "work_link_locked",
        nth: 2,
        code: "let seen = actual.filter(|id| scope.is_all() || visible_link(*id).is_ok());",
        verdict: Verdict::OrgBoundary,
        why: "the compare-and-set is on the primary this ORG can see, so a \
              forced cross-org primary is neither told nor refused forever",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/mod.rs",
        func: "work_link_locked",
        nth: 3,
        code: "let seen = actual.filter(|id| scope.is_all() || visible_link(*id).is_ok());",
        verdict: Verdict::OrgBoundary,
        why: "the same compare-and-set on the visible primary, for a switch",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/mod.rs",
        func: "work_link_locked",
        nth: 4,
        code: "if !scope.is_all() || args.expected_version.is_some() {",
        verdict: Verdict::OrgBoundary,
        why: "the same org check before a reject by link id",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/mod.rs",
        func: "work_link_locked",
        nth: 5,
        code: "} else if !scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "the same org check before a confirm by link id",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/mod.rs",
        func: "work_link_locked",
        nth: 6,
        code: "if !scope.is_all() || args.expected_version.is_some() {",
        verdict: Verdict::OrgBoundary,
        why: "the same org check before an unlink by link id",
    },
    // ---- service/work/resume.rs -----------------------------------------
    Guard {
        file: "crates/fleet-core/src/service/work/resume.rs",
        func: "resume_plan_with",
        nth: 0,
        code: "(OrgScope::All, Some(h)) => OrgScope::for_host(&s, h)?,",
        verdict: Verdict::OrgBoundary,
        why: "NARROWS the org half to the landing host's org (M5); it cannot \
              widen, and the person half travels unchanged because the \
              result goes through `ViewScope::with_org`",
    },
    // ---- service/work/structure.rs --------------------------------------
    Guard {
        file: "crates/fleet-core/src/service/work/structure.rs",
        func: "check_view_filters",
        nth: 0,
        code: "if scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "a filter may name only an org or tracker ID this caller sees",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/structure.rs",
        func: "impact_of",
        nth: 0,
        code: "if !scope.is_all() && !scope.sees_org(session_org) {",
        verdict: Verdict::OrgBoundary,
        why: "which orgs' links a move's preview lists; `Graph::load_for` has \
              already emptied the person-invisible rows out of the graph",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/structure.rs",
        func: "org_impact",
        nth: 0,
        code: "if !scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "the AUTHORITY to move an org (D33), and its own doc has said so \
              since T8d",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/structure.rs",
        func: "rules",
        nth: 0,
        code: "OrgScope::All => lock(store)?.work_rules(),",
        verdict: Verdict::OrgBoundary,
        why: "a placement RULE is a company's configuration — its tracker, \
              project keys and repositories — and names no session",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/structure.rs",
        func: "view_owner",
        nth: 0,
        code: "OrgScope::All => None,",
        verdict: Verdict::OrgBoundary,
        why: "a saved VIEW is a set of filters stored under an org",
    },
    // ---- service/work/tidy.rs -------------------------------------------
    Guard {
        file: "crates/fleet-core/src/service/work/tidy.rs",
        func: "reopened",
        nth: 0,
        code: "} else if !scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "a bound client reads its orgs' reopened work; the three \
              session-derived fields are recomputed below, person-fenced",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/tidy.rs",
        func: "apply_one",
        nth: 0,
        code: "let only = if scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "which of the session's links an archive stamps, by org; the \
              session itself came through `Reach::Drive` per item (the kills \
              are the `Reach::Own` arm — T9c's row named the wrong one)",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/tidy.rs",
        func: "apply_one",
        nth: 1,
        code: "\"snooze\" | \"never\" if !scope.is_all() && !link_visible(scope, s, item.link_id) => {",
        verdict: Verdict::OrgBoundary,
        why: "the same org check on the link a snooze flag goes to",
    },
    // ---- service/work/view.rs -------------------------------------------
    Guard {
        file: "crates/fleet-core/src/service/work/view.rs",
        func: "blocked_on",
        nth: 0,
        code: "g.scope.is_all() || g.item_org(i).is_some_and(|o| g.scope.sees_org(Some(o)))",
        verdict: Verdict::OrgBoundary,
        why: "whether a scoped caller is told the id of a blocking work item \
              in another org (redesign 6.3); an item is the org's work data, \
              and the item still blocks when it is not named",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/view.rs",
        func: "native_work",
        nth: 0,
        code: "if !scope.is_all() && !label.is_some_and(|(_, visible)| *visible) {",
        verdict: Verdict::OrgBoundary,
        why: "this is the org boundary, not a privacy fence: whether a \
              scoped caller is told the PROJECT a subtask sits in. The \
              session half of this very function is person-fenced by \
              `hidden_sessions` (its two `ORG_HALF_SITES` rows)",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/view.rs",
        func: "link_visible",
        nth: 0,
        code: "if scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "the org half, and deliberately AFTER `link_hidden` — the person \
              fence runs first so this shortcut can never skip it",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/view.rs",
        func: "task_visible",
        nth: 0,
        code: "if scope.is_all() {",
        verdict: Verdict::OrgBoundary,
        why: "whether the TASK appears; a task's key and title are work data \
              and survive the person fence, exactly as a local item's do, \
              while every link and session under it is person-fenced",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/view.rs",
        func: "task_visible",
        nth: 1,
        code: "OrgScope::All => true,",
        verdict: Verdict::OrgBoundary,
        why: "the same answer as this function's #0 in its other spelling; \
              every link and session under the task is person-fenced by \
              `Graph::build`",
    },
    Guard {
        file: "crates/fleet-core/src/service/work/view.rs",
        func: "visible_trackers",
        nth: 0,
        code: "OrgScope::All => true,",
        verdict: Verdict::OrgBoundary,
        why: "which companies' trackers a brief names; a `TrackerBrief` is \
              an id, a name and a kind",
    },
    // ---- service/update/mod.rs ------------------------------------------
    Guard {
        file: "crates/fleet-core/src/service/update/mod.rs",
        func: "status",
        nth: 0,
        code: "let own = if caller.is_scoped() {",
        verdict: Verdict::OrgBoundary,
        why: "a caller behind an org boundary sees its own update target only. \
              Open question beside it (T9e): an UNBOUND client is not scoped, \
              so a person's own phone reads every target's id, version, \
              platform, phase and last error — see OPEN_QUESTIONS",
    },
    Guard {
        file: "crates/fleet-core/src/service/update/mod.rs",
        func: "check_for",
        nth: 0,
        code: "if caller.is_scoped() && identity(caller)?.target != target {",
        verdict: Verdict::OrgBoundary,
        why: "the same rule asked of one named target; the open question is \
              recorded once, at `status` #0",
    },
    Guard {
        file: "crates/fleet-core/src/service/update/mod.rs",
        func: "health",
        nth: 0,
        code: "let own = if caller.is_scoped() {",
        verdict: Verdict::OrgBoundary,
        why: "the same rule for the `fleet_health` roll-up; the open question \
              is recorded once, at `status` #0",
    },
    // ---- mcp/events_route.rs --------------------------------------------
    Guard {
        file: "crates/fleet-core/src/mcp/events_route.rs",
        func: "fence_host_bound",
        nth: 0,
        code: "if !caller.is_scoped() {",
        verdict: Verdict::OrgBoundary,
        why: "`HOST_BOUND_HIDDEN_KINDS` is the org-bound and host-bound kind \
              list; its own doc argues at length why the predicate stays \
              `is_scoped`, and `KIND_FENCES` is where every kind is judged",
    },
    // ---- mcp/tools/fleet.rs ---------------------------------------------
    Guard {
        file: "crates/fleet-core/src/mcp/tools/fleet.rs",
        func: "fleet_health",
        nth: 0,
        code: "} else if caller.is_scoped() {",
        verdict: Verdict::OrgBoundary,
        why: "picks the `HealthView` by caller CLASS, and the person arm is \
              the next one (`HealthView::Person`, T8d)",
    },
    Guard {
        file: "crates/fleet-core/src/mcp/tools/fleet.rs",
        func: "fleet_health",
        nth: 1,
        code: "if caller.host_alias.is_some() || !caller.is_scoped() {",
        verdict: Verdict::OrgBoundary,
        why: "an org-bound client is told nothing of another org's tunnels",
    },
    Guard {
        file: "crates/fleet-core/src/mcp/tools/fleet.rs",
        func: "fleet_health",
        nth: 2,
        code: "if caller.is_scoped() {",
        verdict: Verdict::OrgBoundary,
        why: "the last reconcile error is the hub's text about any host, \
              another ORG's included",
    },
    Guard {
        file: "crates/fleet-core/src/mcp/tools/fleet.rs",
        func: "fleet_health",
        nth: 3,
        code: "if caller.host_alias.is_some() || caller.is_scoped() {",
        verdict: Verdict::OrgBoundary,
        why: "the decision envelope and the loops' errors are the hub's own \
              business",
    },
    Guard {
        file: "crates/fleet-core/src/mcp/tools/fleet.rs",
        func: "usage_report",
        nth: 0,
        code: "if let (Some(h), false) = (host.as_deref(), scope.is_all()) {",
        verdict: Verdict::OrgBoundary,
        why: "whether a named HOST exists for this caller — hosts belong to \
              orgs; the report's own rows take `view.sees_session_row`",
    },
    // ---- mcp/tools/orchestration.rs -------------------------------------
    Guard {
        file: "crates/fleet-core/src/mcp/tools/orchestration.rs",
        func: "work_link",
        nth: 0,
        code: "if caller.is_scoped() {",
        verdict: Verdict::OrgBoundary,
        why: "`trust_project` is fleet configuration, not one host's nor one \
              bound client's to change; it names no session",
    },
    Guard {
        file: "crates/fleet-core/src/mcp/tools/orchestration.rs",
        func: "work_link",
        nth: 1,
        code: "if caller.is_scoped() {",
        verdict: Verdict::OrgBoundary,
        why: "`dismiss` is the same fleet-wide configuration question",
    },
];

/// Guards whose verdict is `OrgBoundary` **and** which leave a narrower
/// question open that the eight rules do not settle.
///
/// Keyed like [`SCOPE_GUARDS`], and checked against it: every row here must
/// name a classified `OrgBoundary` guard, and that guard's comment must
/// record the question as an owner decision in so many words. T9c wrote two
/// of these three into the prose beside the code and left the TABLE saying
/// an unqualified `OrgBoundary` — which is what a reader and the test see,
/// so the question was invisible where it counted (T9d).
///
/// It is not a third verdict: the org claim in each of these is honest, and
/// a `Fence` would say the predicate is wrong, which it is not. What is
/// open is whether the thing being withheld from a SCOPED caller should
/// also be withheld from a person's own device.
const OPEN_QUESTIONS: &[(&str, &str, usize, &str)] = &[
    (
        "crates/fleet-core/src/service/usage.rs",
        "report_on",
        0,
        "`by_day` is the whole fleet's per-host daily spend for a person's          own device — an aggregate over other people's private sessions.          §4.3 speaks to rows and counts, not to a cost aggregate;          `HealthView::Person`'s `person_usage_by_day` is the fence if the          answer is yes",
    ),
    (
        "crates/fleet-core/src/service/work/mod.rs",
        "work_purge_impact",
        0,
        "which work KEYS exist: for a person's own device no retention          runs, so `PurgeImpact.keys` is every key on the project and hosts,          other people's included — and a LOCAL item's key is a sentence a          person typed. T10 traced the residual T9b/T9c also attributed to          `orgs::require_key` / `require_key_bound` and decided those: they          answer an org question for a scoped caller and `Ok(())`          unconditionally for everybody else, so THIS is the whole of it.          Fencing it by person would fence the hub operator too (the          master's scope carries a person), which §4.5 does not do",
    ),
    (
        "crates/fleet-core/src/service/update/mod.rs",
        "status",
        0,
        "the device-identity question `view.rs::task` answered (owner decision          2026-10-10: one person does not see another's device names), in the          strictly LARGER case, and the page the `check_for` and `health`          guards follow: `update_status` is `Access::Client` and an UNBOUND          client is not scoped, so a person's own phone reads every observed          target — `client:<id>` / `agent:<host>` / `hub:self`, each with its          version, platform, phase and last error. An inventory of every          other person's devices",
    ),
];

/// A production `is_all()` / `is_scoped()` guard, as the scan found it.
#[derive(Debug, Clone)]
struct Site {
    file: String,
    func: String,
    nth: usize,
    code: String,
    line: usize,
    /// The comment block directly above the guard — and ONLY that.
    ///
    /// It used to be that block *plus* the enclosing function's doc, which
    /// meant one `not a privacy fence` sentence in a fn doc satisfied every
    /// guard in that fn, present and future: in `work_link_locked`, which
    /// holds six, a seventh would have needed only a table row whose `why`
    /// nothing checks. No guard was taking its marker from a fn doc, so
    /// narrowing it costs nothing and closes the hatch (T9d).
    context: String,
}

/// True when `line` is one of the guard spellings.
///
/// Three spellings, not one, because the class is the PREDICATE and not a
/// sentence shape:
///
/// * `if !scope.is_all()` / `if !caller.is_scoped()` — "if the caller is
///   restricted, then check";
/// * `if scope.is_all() { return <permissive> }` — the same guard read the
///   other way round, which is where three of the five historical holes
///   actually lived;
/// * the DISCRIMINANT in a pattern — `match scope { OrgScope::All => … }`,
///   `matches!(s, OrgScope::All)` — which is the same shortcut again and was
///   invisible to the first two (T9d). Running the scan with it added found
///   twelve unclassified production sites, among them `orgs::scope_links`'s
///   `OrgScope::All => {}`: the org-only link pager that is the PARENT of
///   the classified `link_session_visible` guard, and which written as an
///   `if` would always have needed a row.
///
/// A CONSTRUCTION is deliberately not a guard — `&OrgScope::All` handed to a
/// callee, `org: OrgScope::All` in a literal, and in particular the common
/// `None => f(store, &OrgScope::All)` arm of a desktop command. It decides
/// what scope to ask WITH, which is the caller's question and is classified
/// where that caller is; it asks nothing. So the third spelling needs the
/// discriminant on the PATTERN side — left of the `=>`, or inside a
/// `matches!` — and not merely somewhere on a line that has an `=>` on it.
fn is_scope_guard(line: &str) -> bool {
    if line.contains(".is_all()") || line.contains(".is_scoped()") {
        return true;
    }
    if !line.contains("OrgScope::All") {
        return false;
    }
    if line.contains("matches!") {
        return true;
    }
    match line.find("=>") {
        Some(i) => line[..i].contains("OrgScope::All"),
        None => false,
    }
}

/// The name of the `fn` a line is inside: the nearest declaration above it.
///
/// Deliberately crude (no `syn`, no brace counting): a guard is always inside
/// some `fn`, and the nearest `fn` above a line of production code is it. A
/// closure's body reports the enclosing function, which is the useful answer
/// for a table a human reads.
fn fn_name(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let rest = [
        "pub(crate) async fn ",
        "pub(super) async fn ",
        "pub async fn ",
        "pub(crate) fn ",
        "pub(super) fn ",
        "pub fn ",
        "async fn ",
        "fn ",
    ]
    .iter()
    .find_map(|p| t.strip_prefix(p))?;
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    Some(&rest[..end]).filter(|n| !n.is_empty())
}

/// The comment block directly above `idx`, oldest line first.
fn comment_above(lines: &[&str], idx: usize) -> String {
    let mut out: Vec<&str> = Vec::new();
    for k in (0..idx).rev() {
        let t = lines[k].trim_start();
        if t.starts_with("//") {
            out.push(t);
        } else {
            break;
        }
    }
    out.reverse();
    out.join("\n")
}

/// The doc comment of the `fn` declared at (or above) `idx`: the contiguous
/// `///` block above it, skipping attributes.
fn doc_of_fn(lines: &[&str], fn_idx: usize) -> String {
    let mut out: Vec<&str> = Vec::new();
    for k in (0..fn_idx).rev() {
        let t = lines[k].trim_start();
        if t.starts_with("///") || t.starts_with("//!") {
            out.push(t);
        } else if t.starts_with("#[") || t.is_empty() && !out.is_empty() {
            // An attribute between the doc and the `fn`; a blank line ends it.
            if t.is_empty() {
                break;
            }
        } else {
            break;
        }
    }
    out.reverse();
    out.join("\n")
}

/// Every production source file this module judges, as `(rel path, text)`.
///
/// Split out of [`guard_sites`] in T9e so the call-site check
/// ([`a_row_that_names_call_sites_names_all_of_them`]) reads exactly the same
/// set of files: a `why` whose call sites were counted over a different file
/// set would be the undercount again in a new place.
fn production_sources() -> Vec<(String, String)> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    // `crates/fleet-hub/src` is scanned too (T9d): fleet-hub depends on
    // fleet-core, so `OrgScope::is_all` and `Caller::is_scoped` are in scope
    // there, and the hub's CLI and `serve` paths are where an operator-facing
    // guard would plausibly land. It is empty today, and a guard added there
    // would otherwise be unclassified and undetected — the same silent hole
    // in a different directory. `fleet-agent` depends on `fleet-proto` only,
    // so it cannot hold one at all and is not scanned.
    let roots = [
        ("crates/fleet-core/src", manifest.join("src")),
        ("crates/fleet-hub/src", manifest.join("../fleet-hub/src")),
        ("src-tauri/src", manifest.join("../../src-tauri/src")),
    ];
    let mut files: Vec<PathBuf> = Vec::new();
    for (_, root) in &roots {
        rs_files(root, &mut files);
    }
    files.sort();

    // The same test-only-module bookkeeping `no_eprintln_tests` does: a file
    // declared `#[cfg(test)] mod name;` is test code and is not scanned.
    let mut test_only: BTreeSet<PathBuf> = BTreeSet::new();
    let mut scanned = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("read source file");
        // The predicate is irrelevant here: `test_mods` / `test_paths` /
        // `unterminated` come from the `#[cfg(test)]` bookkeeping, not from
        // the hits.
        let result = scan_with(&text, |_| false);
        assert!(
            result.unterminated.is_empty(),
            "inline #[cfg(test)] module without its closing brace at the \
             `mod` line's indentation (run cargo fmt): {} lines {:?}",
            file.display(),
            result.unterminated
        );
        let base = match file.file_name().and_then(|n| n.to_str()) {
            Some("lib.rs" | "main.rs" | "mod.rs") => file.parent().expect("parent").to_path_buf(),
            _ => file.with_extension(""),
        };
        for m in &result.test_mods {
            test_only.insert(base.join(format!("{m}.rs")));
            test_only.insert(base.join(m).join("mod.rs"));
        }
        for p in &result.test_paths {
            test_only.insert(file.parent().expect("parent").join(p));
        }
        scanned.push((file.clone(), text));
    }

    let mut out = Vec::new();
    for (file, text) in scanned {
        if test_only.contains(&file) {
            continue;
        }
        let rel = roots
            .iter()
            .find_map(|(label, root)| {
                let rel = file
                    .strip_prefix(root)
                    .ok()?
                    .to_string_lossy()
                    .replace('\\', "/");
                Some(format!("{label}/{rel}"))
            })
            .expect("under a scanned root");
        out.push((rel, text));
    }
    out
}

/// Every production guard, with its key and the comments around it.
fn guard_sites() -> Vec<Site> {
    let mut out = Vec::new();
    for (rel, text) in production_sources() {
        let hits = scan_with(&text, is_scope_guard).hits;
        if hits.is_empty() {
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        let mut seen: BTreeMap<(String, String), usize> = BTreeMap::new();
        for one in &hits {
            let idx = one - 1;
            let (_, func) = (0..=idx)
                .rev()
                .find_map(|k| fn_name(lines[k]).map(|n| (k, n.to_string())))
                .unwrap_or((idx, "<no enclosing fn>".to_string()));
            let key = (rel.clone(), func.clone());
            let nth = *seen.get(&key).unwrap_or(&0);
            seen.insert(key, nth + 1);
            out.push(Site {
                file: rel.clone(),
                func,
                nth,
                code: lines[idx].trim().to_string(),
                line: *one,
                context: comment_above(&lines, idx),
            });
        }
    }
    out
}

/// **The guard.** See this module's header.
#[test]
fn every_scope_guard_is_classified() {
    let sites = guard_sites();
    // 58 today. The floor moves up with the scan's reach — it was 30 when
    // the scan read two spellings in two crates — and exists so that a
    // predicate or a root that quietly stops matching fails HERE rather than
    // making the whole test pass vacuously.
    assert!(
        sites.len() > 50,
        "the scan found only {} guards; it has stopped working, which would \
         make this whole test pass vacuously",
        sites.len()
    );

    let mut rows: BTreeMap<(&str, &str, usize), &Guard> = BTreeMap::new();
    for g in SCOPE_GUARDS {
        assert!(
            rows.insert((g.file, g.func, g.nth), g).is_none(),
            "two rows for {} {} #{}",
            g.file,
            g.func,
            g.nth
        );
    }

    let mut problems: Vec<String> = Vec::new();
    let mut matched: BTreeSet<(String, String, usize)> = BTreeSet::new();
    for s in &sites {
        let Some(row) = rows.get(&(s.file.as_str(), s.func.as_str(), s.nth)) else {
            problems.push(format!(
                "{}:{} in `{}` (#{}) has no row in SCOPE_GUARDS:\n      {}\n    \
                 Classify it: an ORG-AUTHORITY check (and then write \"This is \
                 the org boundary, not a privacy fence\" beside it, in words a \
                 reader can check), or a PRIVACY FENCE — in which case a \
                 person's own device must reach it, because `OrgScope::All` and \
                 `is_scoped() == false` are what that caller resolves to.",
                s.file, s.line, s.func, s.nth, s.code
            ));
            continue;
        };
        matched.insert((s.file.clone(), s.func.clone(), s.nth));
        if row.code != s.code {
            problems.push(format!(
                "{}:{} in `{}` (#{}) has changed since it was classified.\n    \
                 was: {}\n    now: {}\n    Re-read the guard and the row \
                 ({}).",
                s.file, s.line, s.func, s.nth, row.code, s.code, row.why
            ));
        }
        match row.verdict {
            Verdict::Fence => problems.push(format!(
                "{}:{} in `{}` (#{}) is classified as a PRIVACY FENCE and is \
                 therefore a defect, not a resting place: {}",
                s.file, s.line, s.func, s.nth, row.why
            )),
            Verdict::OrgBoundary => {
                if !s.context.to_lowercase().contains(MARKER) {
                    problems.push(format!(
                        "{}:{} in `{}` (#{}) claims to be the org boundary, and \
                         the comment directly above it does not say so. Write \
                         the sentence — \"This is the org boundary, not a \
                         privacy fence\" — AT THE GUARD (a sentence in the \
                         enclosing fn's doc no longer counts: it would cover \
                         every sibling guard in that fn, including the next \
                         one somebody adds) — or, if it cannot honestly be \
                         written, the guard is a fence and needs the person.",
                        s.file, s.line, s.func, s.nth
                    ));
                }
            }
        }
    }
    // Every open question names a classified `OrgBoundary` guard, and that
    // guard records the question beside the code — not only in this table.
    let by_site: BTreeMap<(&str, &str, usize), &Site> = sites
        .iter()
        .map(|s| ((s.file.as_str(), s.func.as_str(), s.nth), s))
        .collect();
    for (file, func, nth, question) in OPEN_QUESTIONS {
        match (
            rows.get(&(file, func, *nth)),
            by_site.get(&(*file, *func, *nth)),
        ) {
            (None, _) | (_, None) => problems.push(format!(
                "OPEN_QUESTIONS names {file} `{func}` (#{nth}), which is not a \
                 classified guard: {question}"
            )),
            (Some(row), Some(site)) => {
                if row.verdict != Verdict::OrgBoundary {
                    problems.push(format!(
                        "{file} `{func}` (#{nth}) has an open question and is \
                         not an OrgBoundary row"
                    ));
                }
                if !site.context.to_lowercase().contains("owner decision") {
                    problems.push(format!(
                        "{file}:{} in `{func}` (#{nth}) has a row in \
                         OPEN_QUESTIONS and its comment does not call it an \
                         owner decision. Say it at the guard: a question only \
                         this table knows about is a question the next reader \
                         of the code does not. ({question})",
                        site.line
                    ));
                }
            }
        }
    }

    for g in SCOPE_GUARDS {
        if !matched.contains(&(g.file.to_string(), g.func.to_string(), g.nth)) {
            problems.push(format!(
                "SCOPE_GUARDS has a row for {} `{}` (#{}) and the scan found no \
                 such guard: it was moved, renamed or removed. Drop the row, or \
                 re-key it.",
                g.file, g.func, g.nth
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "{} of {} guards are unaccounted for:\n  - {}",
        problems.len(),
        sites.len(),
        problems.join("\n  - ")
    );
}

/// The scan itself, on text whose answer is written out by hand — so a change
/// that quietly stops it finding guards fails here rather than making
/// [`every_scope_guard_is_classified`] pass vacuously.
#[test]
fn the_guard_scan_finds_guards_and_skips_test_code_and_comments() {
    let text = "\
/// doc: is_all() in a doc comment
fn a(scope: &S) {
    // a comment mentioning scope.is_all()
    if !scope.is_all() {
        let _ = 1;
    }
    if scope.is_scoped() {}
}
#[cfg(test)]
mod tests {
    fn t(s: &S) {
        if s.is_all() {}
    }
}
fn b(c: &C) {
    if c.is_scoped() {}
}
fn c(s: &OrgScope) -> bool {
    // the MATCH spelling, which the first version of this scan missed
    match s {
        OrgScope::All => true,
        OrgScope::Org { .. } => false,
    }
}
fn d(s: &OrgScope) -> bool {
    matches!(s, OrgScope::All)
}
fn e(store: &Store, o: Option<X>) -> X {
    // CONSTRUCTIONS, not guards: they decide what to ask WITH
    thing(store, &OrgScope::All);
    match o {
        Some(x) => x,
        None => thing(store, &OrgScope::All),
    }
}
";
    let found = scan_with(text, is_scope_guard);
    assert_eq!(
        found.hits,
        vec![4, 7, 16, 21, 26],
        "two guards in `a`, one in `b`, the match arm in `c` and the \
         `matches!` in `d`; the doc line, the two `//` lines, the whole test \
         module and both of `e`'s constructions are skipped — including the \
         `None => f(&OrgScope::All)` one, which has an `=>` on the same line"
    );
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(fn_name(lines[1]), Some("a"));
    assert_eq!(fn_name(lines[14]), Some("b"));
    assert_eq!(fn_name(lines[3]), None);
    assert!(comment_above(&lines, 3).contains("a comment mentioning"));
    assert!(doc_of_fn(&lines, 1).contains("in a doc comment"));
}

/// One production file, prepared for the call-site scan: every line, plus
/// which of them are production CODE.
///
/// `code` is `scan_with(text, |_| true).hits` with the line numbers made
/// 0-based — i.e. exactly the lines `no_eprintln_tests`' scanner offers a
/// predicate: non-comment lines outside an inline `#[cfg(test)]` module. It
/// is what keeps a test function's name out of a guard's caller list.
struct Prepared {
    lines: Vec<String>,
    code: BTreeSet<usize>,
}

fn prepare(sources: &[(String, String)]) -> Vec<Prepared> {
    sources
        .iter()
        .map(|(_, text)| Prepared {
            lines: text.lines().map(str::to_string).collect(),
            code: scan_with(text, |_| true)
                .hits
                .into_iter()
                .map(|n| n - 1)
                .collect(),
        })
        .collect()
}

/// Is `line` a CALL of `name` — `name(`, `.name(`, `::name(` — rather than a
/// longer identifier that merely ends in it, or the declaration itself?
///
/// `visible_links` and `person_visible_links` are the reason the word
/// boundary is checked on the left: a plain `contains("visible_links(")`
/// reads every call of the second as a call of the first.
fn is_call_of(line: &str, name: &str) -> bool {
    if fn_name(line) == Some(name) {
        // The declaration. `fn f(` is not a call of `f`.
        return false;
    }
    let needle = format!("{name}(");
    let mut from = 0usize;
    while let Some(i) = line[from..].find(&needle) {
        let at = from + i;
        let left_ok = line[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        if left_ok {
            return true;
        }
        from = at + 1;
    }
    false
}

/// How many production `fn name(` declarations there are.
///
/// More than one and this crude resolver cannot tell which function a
/// `name(` call means — `service/update/mod.rs`'s `health` and
/// `service/tunnel`'s are two different functions with one name — so
/// [`a_row_that_names_call_sites_names_all_of_them`] abstains on that row
/// rather than reporting a caller set that mixes them.
fn declarations_of(name: &str, prepared: &[Prepared]) -> usize {
    prepared
        .iter()
        .map(|f| {
            f.code
                .iter()
                .filter(|&&i| fn_name(&f.lines[i]) == Some(name))
                .count()
        })
        .sum()
}

/// The production functions that call `name`, by the enclosing `fn` of each
/// call — the same crude "nearest `fn` above" rule [`guard_sites`] keys on,
/// so the two answers are in the same vocabulary.
fn callers_of(name: &str, prepared: &[Prepared]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for f in prepared {
        for &idx in &f.code {
            if !is_call_of(&f.lines[idx], name) {
                continue;
            }
            if let Some(found) = (0..=idx).rev().find_map(|k| fn_name(&f.lines[k])) {
                if found != name {
                    out.insert(found.to_string());
                }
            }
        }
    }
    out
}

/// The identifiers a `why` names in backticks, last path segment only:
/// `` `status::set_status` `` is "set_status", `` `Store::local_item_links` ``
/// is "local_item_links".
///
/// Backticks, not bare words, are the discriminator on purpose — "a task's
/// key" must not read as naming a function called `task`, and every row that
/// does name a function already writes it in backticks.
fn backticked_idents(why: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (i, part) in why.split('`').enumerate() {
        if i % 2 == 0 {
            continue;
        }
        let seg = part.rsplit("::").next().unwrap_or(part).trim_start();
        let ident: String = seg
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !ident.is_empty() {
            out.insert(ident);
        }
    }
    out
}

/// **A row whose `why` names SOME of the guard's call sites must name them
/// ALL** (multi-user M1, T9e).
///
/// This is the one class of label untruth that is mechanisable, and it is
/// the class that actually bit: `local_item_visible`'s row said "the two
/// writes that ask it" through two rounds, while a third production call
/// site existed — `name_session_work_as`, the one with no item-level person
/// gate. "Which functions call this guard's enclosing function" is a fact
/// the same scan can establish, so an undercount is now a test failure
/// rather than a reader's catch. It found two more the moment it was
/// written (`orgs::sees_session_org_only`, which named one of five, and
/// `orgs::scope_links`, one of three).
///
/// Deliberately narrow, in three ways a reader should know:
///
/// * it cannot tell whether a named function really does what the row says
///   it does — most of what a `why` asserts is still reviewed by people;
/// * it says nothing about a row that names NO call site. A row that does
///   not want to enumerate should name none, and this test has nothing to
///   say about it;
/// * it ABSTAINS when the guard's enclosing function name is declared more
///   than once in production ([`declarations_of`]), because a bare name is
///   then ambiguous to a scan that does not resolve imports.
#[test]
fn a_row_that_names_call_sites_names_all_of_them() {
    let sources = production_sources();
    assert!(
        sources.len() > 100,
        "only {} production files; the scan has stopped working",
        sources.len()
    );
    let prepared = prepare(&sources);
    // At least this many rows must actually exercise the check, or a change
    // that quietly stops it matching would pass here vacuously.
    let mut exercised: Vec<&str> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for g in SCOPE_GUARDS {
        if declarations_of(g.func, &prepared) != 1 {
            continue;
        }
        let callers = callers_of(g.func, &prepared);
        let named = backticked_idents(g.why);
        let hit: BTreeSet<&String> = callers.intersection(&named).collect();
        if hit.is_empty() {
            continue;
        }
        exercised.push(g.func);
        let missing: Vec<&str> = callers
            .iter()
            .filter(|c| !named.contains(*c))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            problems.push(format!(
                "{} `{}` (#{}) names {:?} among the call sites of `{}` and \
                 leaves out {:?}. Either name every one of them — the \
                 undercount in this exact row is why this test exists — or \
                 rephrase the `why` so it names none.",
                g.file, g.func, g.nth, hit, g.func, missing
            ));
        }
    }
    assert!(
        exercised.len() >= 3,
        "only {} rows name a call site of their own guard's function \
         ({exercised:?}); this check has stopped matching and would pass \
         vacuously",
        exercised.len()
    );
    assert!(
        problems.is_empty(),
        "{} rows name their call sites incompletely:\n  - {}",
        problems.len(),
        problems.join("\n  - ")
    );
}

// ---------------------------------------------------------------------------
// The org-only SESSION predicates (multi-user M1, T10)
// ---------------------------------------------------------------------------

/// One production call of an org-only SESSION predicate, with the person half
/// that runs beside it.
///
/// Keyed like a [`Guard`] and for the same reason: `(file, func, nth)` moves
/// only when a call is added to or removed from that function.
#[derive(Debug)]
struct OrgHalf {
    file: &'static str,
    func: &'static str,
    nth: usize,
    code: &'static str,
    /// WHERE the person half of this question runs. Read by people; the
    /// mechanised part is that the comment at the site must NAME a person
    /// predicate ([`PERSON_PREDICATES`]).
    person_half: &'static str,
}

/// The person-side predicates. A site in [`ORG_HALF_SITES`] must name at
/// least one of them in the comment attached to it or in its enclosing
/// function's doc — the same "write the claim next to the code" discipline
/// [`MARKER`] applies to a scope guard, with a closed vocabulary instead of a
/// sentence, because here the claim is *which* predicate finishes the job.
const PERSON_PREDICATES: &[&str] = &[
    "sees_session_row",
    "sees_session_facts",
    "sees_past_conversation",
    "may_drive",
    "may_own",
    "link_person_visible",
    "link_hidden",
    "hidden_sessions",
    "resolve_row_person_gated",
    "require_person_sees",
    "require_message_recipient",
    "session gate",
];

/// True when `line` CALLS one of `OrgScope`'s two org-only session
/// predicates. The two `fn sees_…` declarations themselves are not calls.
fn calls_org_only_session_predicate(line: &str) -> bool {
    if line.contains("fn sees_") {
        return false;
    }
    line.contains("sees_row_org_only(") || line.contains("sees_session_org_only(")
}

/// **Every production call of `OrgScope::sees_row_org_only` /
/// `sees_session_org_only`, with the person half that finishes it.**
///
/// T6 renamed the two predicates to `*_org_only` so that every call site
/// would be triaged, and T10's plan then said to DELETE both — the deletion
/// being the compiler-checked completeness proof. The triage happened (T7,
/// T8d, T9b, T9c, T9d, T9e converted or composed every one of them, and T10
/// took the last unconverted site, `health::HealthView::Org`), and the
/// deletion turned out to be the wrong instrument for what is left:
///
/// * [`crate::service::view_scope::ViewScope::sees_session_facts`] **is** the
///   composition. It opens with the org predicate and the rest of its body is
///   the person half, so the org question has to be expressible for the whole
///   person fence to exist at all. Deleting it from `OrgScope` would move its
///   body, not remove it.
/// * every other surviving site is a genuine ORG-AUTHORITY question whose
///   person half runs somewhere this function cannot see — at the tool layer
///   (`resolve_row_person_gated`, `require_person_sees`), one predicate away
///   (`link_person_visible`, `link_hidden`), or already applied to the rows
///   being filtered (`Graph::load_for`'s `hidden_sessions`). The `why` column
///   of a `SCOPE_GUARDS` row says so in prose; this table says it per CALL,
///   and a test derives the call list from the source.
///
/// So the deletion is replaced by something that keeps working: a call that
/// nobody has triaged fails here, every time, rather than once at the moment
/// the shim went away.
///
/// **One reading this table killed, and it is worth recording because it is
/// the obvious one.** Eight of these calls sit directly beside a person
/// predicate that composes the org answer, and T10 first read them as pure
/// REDUNDANCY and deleted them. The suite said no in four places, and the
/// reason was a real defect rather than a subtlety:
/// [`crate::service::view_scope::ViewScope::sees_session_facts`] returned at
/// its FIRST clause for the hub's own reader —
/// [`crate::service::view_scope::ViewScope::internal`] — **before** the org
/// boundary, so for a scope that was internal AND narrowed
/// (`ViewScope::internal().with_org(..)`: `work::nudge`'s hook reader,
/// `work::today`'s per-host reader, `work::resume`'s landing-host reader, and
/// every org-level test in the work graph) the person predicate answered
/// `true` unconditionally and these calls were the only org fence there was.
///
/// **That ordering is fixed** (multi-user M1, the T6 review): the org clause
/// is now first in `sees_session_facts`, first in `may_own`, and the sites
/// that skipped a fence on `is_internal` alone ask
/// [`crate::service::view_scope::ViewScope::is_unrestricted`] instead. So the
/// composition these rows describe really does hold for every scope shape,
/// and `with_org` narrows what it says it narrows
/// (`view_scope_tests::a_narrowed_hub_reader_is_still_fenced_by_its_org`).
///
/// The rows stay, and the lesson with them. They are no longer the ONLY org
/// fence for a narrowed hub reader, but each is still the org half of a path
/// that applies it somewhere the person predicate is not asked — a filter
/// that runs before it (`work::today`), a projection over items rather than
/// rows (`orgs::scopes`) — and "a person predicate runs beside it" is still
/// not a reason to delete an org call without reading what each one guards.
const ORG_HALF_SITES: &[OrgHalf] = &[
    OrgHalf {
        file: "crates/fleet-core/src/service/work/mod.rs",
        func: "check_live_elsewhere",
        nth: 0,
        code: "if !view.org.sees_row_org_only(&row) {",
        person_half: "`sees_session_row`, on the next line: the org half \
                      decides whether another live session counts (P-3 warns \
                      even about one the caller may not see), the person half \
                      whether it is named; an unseen one is said as \
                      \"Someone is already working on this.\"",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/view_scope.rs",
        func: "sees_session_facts",
        nth: 0,
        code: "if !self.org.sees_session_org_only(f.host_alias, f.org_id) {",
        person_half: "the rest of this very function: §4.4's host clauses, \
                      `owns_person`, the grant set and `sole_persons_unclaimed`. \
                      This is THE composition — the one place the org half is \
                      meant to be called from",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/messages.rs",
        func: "send_message_scoped",
        nth: 0,
        code: "if !scope.sees_row_org_only(&row) {",
        person_half: "`support::require_message_recipient`, at the tool layer \
                      and at Reach::Drive",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/messages.rs",
        func: "send_message_scoped",
        nth: 1,
        code: "if !scope.sees_row_org_only(&to) {",
        person_half: "the same `require_message_recipient`, for a recipient \
                      named by id",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/messages.rs",
        func: "peer_status",
        nth: 0,
        code: ".filter(|r| scope.sees_row_org_only(r))",
        person_half: "`resolve_row_person_gated(.., Reach::Read, ..)` in \
                      `mcp::tools::messaging::peer_status`, which runs before \
                      this service call",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/orgs.rs",
        func: "sees_row_org_only",
        nth: 0,
        code: "self.sees_session_org_only(&row.host_alias, row.org_id)",
        person_half: "none, and none is owed: this is the row-shaped wrapper \
                      over the predicate above, org-only by its own name, and \
                      every CALLER of it is a row in this table",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/orgs.rs",
        func: "link_session_visible",
        nth: 0,
        code: "Some(row) => scope.sees_row_org_only(&row),",
        person_half: "`link_person_visible`, which `scope_links_for` composes \
                      with this for every link-page read",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/orgs.rs",
        func: "link_session_visible",
        nth: 1,
        code: "Ok(scope.sees_session_org_only(host.as_deref().unwrap_or_default(), org))",
        person_half: "the same `link_person_visible`, whose third arm judges a \
                      reaped participant's recorded conversations through \
                      `sees_past_conversation`",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/work/view.rs",
        func: "native_work",
        nth: 0,
        code: ".is_some_and(|r| scope.sees_row_org_only(r))",
        person_half: "`hidden_sessions`: `native_work` is reached only from \
                      `task`, which builds the graph with `Graph::load_for`, so \
                      every person-invisible row is already out of `g.sessions` \
                      and a lookup in it cannot answer for one",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/work/view.rs",
        func: "native_work",
        nth: 1,
        code: ".filter(|r| scope.sees_row_org_only(r))",
        person_half: "the same `hidden_sessions`, for the job's worker: the row \
                      comes out of the same `g.sessions` the person fence \
                      emptied",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/work/view.rs",
        func: "build",
        nth: 0,
        code: ".is_some_and(|row| scope.sees_row_org_only(row))",
        person_half: "`hidden_sessions` directly above, which has already taken \
                      every person-invisible row out of `sessions` when a \
                      `ViewScope` was supplied; this clause is what still \
                      fences the org-only `Graph::load` path, whose readers \
                      answer tasks and never a session row",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/work/view.rs",
        func: "link_visible",
        nth: 0,
        code: "row.host_alias == *alias && scope.sees_row_org_only(row)",
        person_half: "`link_hidden`, called first in this very function and \
                      before the `is_all()` shortcut, precisely so the person \
                      fence cannot be skipped",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/work/view.rs",
        func: "link_visible",
        nth: 1,
        code: "_ => scope.sees_row_org_only(row),",
        person_half: "the same `link_hidden`",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/work/view.rs",
        func: "link_visible",
        nth: 2,
        code: "_ => scope.sees_session_org_only(host, l.link.org_id),",
        person_half: "the same `link_hidden`, whose `hidden_links` half covers \
                      exactly this snapshot arm",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/work/mod.rs",
        func: "work_link_locked",
        nth: 0,
        code: ".is_some_and(|row| scope.sees_row_org_only(&row))",
        person_half: "the transport's session gate, which resolves every \
                      `work_link` entry point's `session_id` through \
                      `resolve_row_person_gated` before this is reached; this \
                      clause keeps the SERVICE's own answer the same for the \
                      batch path",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/work/local.rs",
        func: "name_session_work_as",
        nth: 0,
        code: "None => scope.sees_row_org_only(&r) && scope.sees_org(r.org_id),",
        person_half: "`resolve_row_person_gated(.., Reach::Drive, ..)` in \
                      `work_link { action: name }`, deliberately without the \
                      host gate in front so an unreachable session answers as \
                      an unknown id",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/work/agent_handover.rs",
        func: "request",
        nth: 0,
        code: "if row.host_alias != h || !scope.sees_row_org_only(&row) {",
        person_half: "`resolve_row_person_gated(.., Reach::Drive, ..)` in \
                      `work_link { action: handover }` — drive, because a \
                      handover request types into the pane",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/work/agent_handover.rs",
        func: "request",
        nth: 1,
        code: "} else if !scope.sees_row_org_only(&row) {",
        person_half: "the same `resolve_row_person_gated`, for a caller with no \
                      host binding",
    },
    OrgHalf {
        // `person_sees` IS the body of `require_person_sees`; T11 split the
        // two so the long-poll re-check could reuse the one gate in the
        // service layer's error type, and the call moved with the body.
        file: "crates/fleet-core/src/mcp/tools/support.rs",
        func: "person_sees",
        nth: 0,
        code: "&& scope.org.sees_session_org_only(&row.host_alias, row.org_id)",
        person_half: "this function, which has ALREADY refused through \
                      `sees_session_row`; the org call only decides whether the \
                      refusal may say E_PANE_UNPROVEN instead of E_NOTFOUND",
    },
    OrgHalf {
        file: "crates/fleet-core/src/mcp/tools/support.rs",
        func: "require_bound_client_sees",
        nth: 0,
        code: "if scope.sees_row_org_only(row) {",
        person_half: "`require_person_sees`, which runs beside this and never \
                      inside it (its own doc says so)",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/work/today.rs",
        func: "today",
        nth: 0,
        code: "Some(h) => r.host_alias == h && scope.sees_row_org_only(r),",
        person_half: "`view.sees_session_row` on the next filter. BOTH halves \
                      run here on purpose and T10 proved it by deleting this \
                      one: at the time `ViewScope::sees_session_facts` \
                      returned at its FIRST clause for the hub's own reader, \
                      before the org boundary, so an internal-and-narrowed \
                      scope (`ViewScope::internal().with_org(..)`) was fenced \
                      by this call and nothing else. That ordering is fixed \
                      (the T6 review), so this is now the org half of a \
                      filter that runs BEFORE the person predicate rather \
                      than the only fence there is",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/work/today.rs",
        func: "today",
        nth: 1,
        code: "None => scope.sees_row_org_only(r),",
        person_half: "the same `view.sees_session_row`, for a caller with no \
                      host binding",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/orgs.rs",
        func: "counted_rows",
        nth: 0,
        code: ".filter(|r| r.status != \"ghost\" && scope.sees_row_org_only(r) && scope.sees_org(r.org_id))",
        person_half: "`view.sees_session_row` on the next filter. `sees_org` \
                      beside it is a THIRD question neither asks: another \
                      org's row on a per-host token's own host. Shared by \
                      `scopes` and the org overview (`org_details`)",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/usage.rs",
        func: "report_on",
        nth: 0,
        code: ".filter(|r| scope.sees_row_org_only(r))",
        person_half: "`view.sees_session_row` on the next filter",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/sessions/targeting.rs",
        func: "related_sessions_scoped",
        nth: 0,
        code: "rows.retain(|r| scope.sees_row_org_only(r) && view.sees_session_row(r).is_visible());",
        person_half: "`view.sees_session_row`, in the same expression",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/trackers/tickets.rs",
        func: "live_ids",
        nth: 0,
        code: ".filter(|(_, r)| reader.org.sees_row_org_only(r))",
        person_half: "`reader.sees_session_row` on the next filter",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/work/card.rs",
        func: "card",
        nth: 0,
        code: "scope.sees_row_org_only(&row) && reader.sees_session_row(&row).is_visible()",
        person_half: "`reader.sees_session_row`, in the same expression",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/work/view.rs",
        func: "session_tasks",
        nth: 0,
        code: ".filter(|r| scope.sees_row_org_only(r))",
        person_half: "`Graph::load_for`'s `hidden_sessions`, which has already \
                      emptied `g.sessions` of the rows this caller may not see",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/downloads.rs",
        func: "visible",
        nth: 0,
        code: "scope.org.sees_session_org_only(&row.host_alias, row.org_id)",
        person_half: "Two halves. While the session row exists, `may_own` in \
                      the OTHER arm of the same match decides, on the session \
                      the file came out of: the `own` TIER, not \
                      `sees_session_row`, because a download is an \
                      unconstrained absolute-path read of the owner's host, \
                      which §4.3 invariant 5 says no grant confers. This arm \
                      is the session-is-GONE case (a reaped row, or a \
                      download with no `session_id`): `DownloadRow.org_id` is \
                      the org half, and `may_own_person_row` on \
                      `DownloadRow.owner_person_id` (copied from the session \
                      when the row is written, migration 148) is the person \
                      half, chained with `&&` on the next line. Before 148 \
                      this arm had no person half, and every person in the \
                      org saw a reaped session's downloads (review r04 F3). A \
                      row whose session was gone before 148 keeps a NULL \
                      owner and falls to the hub's own readers and a \
                      single-person hub's one person",
    },
    OrgHalf {
        file: "crates/fleet-core/src/service/library.rs",
        func: "visible_with",
        nth: 0,
        code: "scope.org.sees_session_org_only(&row.host_alias, row.org_id)",
        person_half: "The same two halves as `downloads::visible`. While the \
                      session row exists, `may_own` in the OTHER arm decides \
                      (the `own` tier, because a Library row names a path on \
                      the owner's host). Once it is gone, \
                      `LibraryItemRow.org_id` is the org half and \
                      `may_own_person_row` on `LibraryItemRow.owner_person_id` \
                      (migration 148) is the person half, chained with `&&` \
                      on the next line (review r04 F3). A row whose session \
                      was gone before 148 keeps a NULL owner and falls to the \
                      hub's own readers and a single-person hub's one person",
    },
];

/// Every production call of the two org-only session predicates, keyed like a
/// guard site.
fn org_half_sites() -> Vec<Site> {
    let mut out = Vec::new();
    for (rel, text) in production_sources() {
        let hits = scan_with(&text, calls_org_only_session_predicate).hits;
        if hits.is_empty() {
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        let mut seen: BTreeMap<(String, String), usize> = BTreeMap::new();
        for one in &hits {
            let idx = one - 1;
            let (fn_idx, func) = (0..=idx)
                .rev()
                .find_map(|k| fn_name(lines[k]).map(|n| (k, n.to_string())))
                .unwrap_or((idx, "<no enclosing fn>".to_string()));
            let key = (rel.clone(), func.clone());
            let nth = *seen.get(&key).unwrap_or(&0);
            seen.insert(key, nth + 1);
            out.push(Site {
                file: rel.clone(),
                func,
                nth,
                code: lines[idx].trim().to_string(),
                line: *one,
                // The comment AT the call plus the enclosing function's doc.
                // Unlike `MARKER` on a scope guard, the enclosing doc counts
                // here on purpose: several of these composals are properties
                // of the whole function (`sees_session_facts` IS the
                // composition; `link_visible` calls `link_hidden` first), and
                // the claim this check carries is about the function's
                // contract rather than about one line of it.
                context: format!(
                    "{}\n{}",
                    comment_above(&lines, idx),
                    doc_of_fn(&lines, fn_idx)
                ),
            });
        }
    }
    out
}

/// **T10's completeness check, in the form the deletion was meant to take.**
/// See [`ORG_HALF_SITES`] for why a table and a scan replaced deleting the
/// two predicates.
#[test]
fn every_org_only_session_predicate_call_names_its_person_half() {
    let sites = org_half_sites();
    // 25 today. A floor, so a predicate that quietly stops matching fails
    // HERE rather than making the test pass vacuously.
    assert!(
        sites.len() > 20,
        "the scan found only {} calls of the org-only session predicates; it \
         has stopped working, which would make this whole test pass vacuously",
        sites.len()
    );
    let mut rows: BTreeMap<(&str, &str, usize), &OrgHalf> = BTreeMap::new();
    for r in ORG_HALF_SITES {
        assert!(
            rows.insert((r.file, r.func, r.nth), r).is_none(),
            "two rows for {} {} #{}",
            r.file,
            r.func,
            r.nth
        );
    }
    let mut problems: Vec<String> = Vec::new();
    let mut matched: BTreeSet<(String, String, usize)> = BTreeSet::new();
    for s in &sites {
        let Some(row) = rows.get(&(s.file.as_str(), s.func.as_str(), s.nth)) else {
            problems.push(format!(
                "{}:{} in `{}` (#{}) calls an org-only SESSION predicate and \
                 has no row in ORG_HALF_SITES:\n      {}\n    Say where the \
                 PERSON half runs — and if the answer is \"nowhere\", this \
                 call is the fence and it is wrong, because `OrgScope::All` is \
                 what a person's own device resolves to. If the person \
                 predicate beside it already composes the org answer \
                 (`sees_session_row` does), delete this call instead of \
                 classifying it.",
                s.file, s.line, s.func, s.nth, s.code
            ));
            continue;
        };
        matched.insert((s.file.clone(), s.func.clone(), s.nth));
        if row.code != s.code {
            problems.push(format!(
                "{}:{} in `{}` (#{}) has changed since it was classified.\n    \
                 was: {}\n    now: {}\n    Re-read it and the row ({}).",
                s.file, s.line, s.func, s.nth, row.code, s.code, row.person_half
            ));
        }
        if !PERSON_PREDICATES.iter().any(|p| s.context.contains(p)) {
            problems.push(format!(
                "{}:{} in `{}` (#{}) names no person predicate in the comment \
                 attached to it or in its enclosing function's doc. One of \
                 {:?} has to appear IN THE CODE, because \"which predicate \
                 finishes this fence\" is the claim this table exists to \
                 carry, and a claim only the table knows about is one the next \
                 reader of the code does not. (The row says: {})",
                s.file, s.line, s.func, s.nth, PERSON_PREDICATES, row.person_half
            ));
        }
    }
    for r in ORG_HALF_SITES {
        if !matched.contains(&(r.file.to_string(), r.func.to_string(), r.nth)) {
            problems.push(format!(
                "ORG_HALF_SITES has a row for {} `{}` (#{}) and the scan finds \
                 no such call. Delete the row — a stale row is how a table \
                 starts lying.",
                r.file, r.func, r.nth
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "{} org-only session-predicate call(s) unaccounted for:\n  - {}",
        problems.len(),
        problems.join("\n  - ")
    );
}
