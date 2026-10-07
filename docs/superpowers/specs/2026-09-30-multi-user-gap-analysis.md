# Multi-user fleet — gap analysis and design decisions

**Status:** analysis, revision 6. **§4's decisions are now built** — M1 is on
branch `mellow-virgo` (2026-10-01 … 2026-10-05, unmerged), per the task list
in `docs/superpowers/plans/2026-09-30-multi-user-m1-private-sessions.md`, whose
own status line says what is landed and what is left. §4.3's `own`-tier list
and §4.4's shared-host rule and pane proof are the authorities the code and
its tests cite, so a change to either is a change to the implementation.

Two things this header used to say and no longer can. "Nothing here is built"
was true when written and is not; and "no decision in §4 is final until the
owner says yes" has been overtaken — the owner took the §4 decisions across
four review rounds, including the ones that *reversed* a recommendation here
(no admin override of privacy, audited or otherwise; sharing confers no
terminal; an admin may narrow or revoke a departed member's grants but never
widen, add a recipient, or redirect one). Those are constraints on the code
now, not proposals.

**§3 remains what it is:** a reading of the tree as it was on 2026-09-30,
before any of this landed, and not a decision. It is deliberately not updated
— §3 is the *before* picture the gap is measured against, and rewriting it to
match the current tree would erase the gap this document exists to describe.

**Revision 2 (owner's review, 2026-09-30).** Five recommendations in revision 1
drifted from the agreed brief and were corrected. They are recorded here so the
change is visible, not silently folded in:

| Was | Corrected to |
|---|---|
| A personal hub can run with zero `people` rows | Ownership is always unambiguous: a hub creates its personal owner **automatically, without registration**. Simplicity of *use* does not require zero identities in the database (§4.1, §4.3) |
| Identity is per hub, full stop | Per-hub rows may be the right implementation, but the user must experience **one personal profile**; joining a company must never mean setting the profile up again (§4.1) |
| Migration sets every existing session to `visibility = 'org'` | Rejected. Old sessions must **not** become org-readable the moment a colleague is added. Unattributable rows get a safe holding state and an explicit claim (§4.3, Q10) |
| An AI account is a property of a unix user on a host | That is a **scope reduction of the requirement, not a fulfilment of it**. Independent choice of a personal vs. company account on the *same* machine stays the goal, and Codex was not covered at all (§2.5, Q7) |
| An org admin gets an audited break-glass path | Rejected. It is a new exception to the owner's explicit decision that a private session is private from the company admin too, and is **not approved** (§4.5, Q11) |

Two factual corrections to revision 1's wording are folded into Q1 and Q2:
there *is* a secret that can be lost (device tokens) and a recovery path for
administrative access is required; and revocation of a live connection is
**time-bounded, not immediate**.

**Revision 3 (owner's review, 2026-09-30).** Four more:

| Was | Corrected to |
|---|---|
| An org admin may "revoke or re-home" a departed person's grants | **Downward only.** Revoke or narrow, never widen, never add a recipient, never redirect one to themselves — those are privacy bypasses wearing a grant's clothes (Q9, §4.3) |
| The terminal bypasses Fleet, so watch-only is documented rather than enforced | Access obtained *by sharing* must be revocable, therefore **sharing never confers a terminal**. Watch and drive are hub-mediated; direct SSH stays a separate machine-level permission Fleet neither grants nor claims to revoke (§2.4, §4.3) |
| An `unclaimed` row is "listed" and claimable | A row's metadata is content. Unclaimed sessions surface as a **count**, never as rows, and claiming needs proof of host access — not org membership (§4.3) |
| A personal profile travels with the device | Three layers, named: the **shared personal profile**, the **preferences that follow a person between devices**, and **purely local settings**. A second device must inherit the first two without re-setup, and the company hub must not be the carrier (§4.1) |

Revision 3 also separates two events that revision 2 ran together: **revoking a
device** and **revoking a grant or a membership**. A valid device token is not,
by itself, a right to any session (Q2).

**Revision 4 (recon against the code, 2026-09-30).** Revisions 1–3 were written
against the architecture; a thirteen-agent read of the tree — nine subsystem
maps and three adversarial lenses — checked them against it. It returned 105
statements these documents make that the code contradicts, and 40 reachable
leaks, 16 of them blocking; all three lenses concluded M1 is not implementable
as revision 3 describes it. Nothing about the *goal* changed. What changed is
the description of the code M1 lands in, and three decisions the owner took on
the strength of it.

| Was | Corrected to |
|---|---|
| "adding a second dimension means widening one type and **six choke points**" (§1, §5) | **More surfaces than six — §5.1 tabulates them — and four of the six are no-ops for exactly the caller M1 introduces.** `require_visible_session`, `require_bound_client_sees`, `redact_work_via` and `fence_frame` each return early for an unbound paired client, which is the shape a person's device has. M1 **turns the machinery on for a new class of caller**; it does not widen a predicate (§2.3, §5.1) |
| "the enforcement architecture is already correct; it is filtering on the wrong dimension" (§2.3) | Struck. The funnel exists, but for a person's device four of its five gates are open and the fifth cannot drop a row from an answer. Whoever reads the old sentence looks for the predicate and misses the early returns (§2.3, §5.1) |
| `person: None` quietly keeps today's full-fleet behaviour | **A scope built from a *token* that names no person refuses session rows.** The old reading ships the headline guarantee false on the normal upgrade path, with no attacker: every device paired before the upgrade, and every `pair` without `--person`, resolves to `OrgScope::All` (§4.3, §3.3) |
| The ownership test is "the row's owner is the caller's person" | Written the obvious way, `row.owner_person_id == scope.person`, it is `Option == Option` — so `None == None` makes every person-less caller the **owner** of every unclaimed row. One inherent method, `ViewScope::owns`, `matches!`-shaped, called by everything (§4.3) |
| §4.4: narrow `OrgScope::Host`'s `row_host == alias` so a host token cannot see a row "private to someone else" | **Unsatisfiable as stated**, and on the wrong function. One token per host means the agent inside its own session is indistinguishable from any other agent on that machine, so the rule blinds it to itself; and the single-session gate never reaches `sees_session` for a host token at all. The proof is the **pane**: `$TMUX_PANE` against `sessions.tmux_pane_id` (§4.4) |
| `unclaimed` is "listed but content-refused" (Q10) *and* "a count, never a row" (§4.3) | One answer, in §4.3, which is now the sole authority: **a per-host count and nothing else**, plus the one carve-out that makes claiming reachable at all — a host token sees an `unclaimed` row on its own host (§4.3, Q10) |
| Sharing with a team (`session_share { org }`) is part of M1 | **Out of M1, deferred to M2** — an owner's decision, taken knowingly, with the reason recorded (§4.3, Q9) |
| Silent on what happens to a grant when a session moves | **Grants are dropped on a move** — an owner's decision (§4.3, Q9) |
| Silent on the hub↔desktop wire contract | `CONTRACT_REVISION` **6 → 7**, accepted, with no mixed window, per `docs/hub.md`'s existing rule (§5.3) |
| Two levels, `watch` and `drive` | A third tier, `own`, because `spawn_review`, `move_session`, `rewind_conversation { fork }`, `restore_host_sessions` and `work_link { summarize }` each turn a grant into access revocation cannot reach (§2.4, Q6). *Revision 5 sharpens this: `own` is a tier, not a grantable level — see below* |
| "103 tools and 216 desktop commands"; "`SessionRow` has 40 fields"; "sessions are inserted from three places" | Each was wrong, and revision 5 replaces the replacement numbers with the artifacts that hold them: `TOOL_POLICIES` for the tool count, `verdicts.rs::VERDICTS` for the desktop commands, `hub_contract.golden.json`'s `SessionRow` entry for the wire keys. The one that is a *fact* rather than a tally stands: production has exactly **two** INSERTs into `sessions`, both reached only from a reconcile pass (§1, §2.1, §3.1, §5) |

One new section, §5.2, inventories the paths to session data that pass through
none of the choke points and appeared in neither document.

**Revision 5 (owner's delegate, after a three-lens verification of revision 4,
2026-09-30).** The verification read both documents against the tree again. It
found seven revision-4 corrections that had not landed, 28 contradictions
between this document and the companion plan or inside one of them, and 23
claims the rewrite itself introduced that the code does not support. Eight
decisions settle the contradictions. They are binding, and they are applied
below rather than argued again.

| Was | Corrected to |
|---|---|
| Three sharing levels — `watch`, `drive`, `own` — tabulated as if a grant could reach the third | **Two grantable levels, `watch` and `drive`. `own` is a tier, not a level:** the set of operations only the owner may perform, which no grant ever reaches. Its membership is defined in **one place** — §4.3, invariant 5 — and Q6, §2.4 and every task in the plan **cite** that place rather than restate the list, because the three copies revision 4 shipped already disagreed with each other (§2.4, §4.3, Q6) |
| `sessions.visibility` is `private` / `org` / `unclaimed` | **Two values in M1: `private` and `unclaimed`.** The `CHECK` admits those two and nothing else, so a third cannot appear by accident. `'org'` is removed completely — not a settable value, not a migration target, not a UI control. The reason, recorded: `'org'` is team sharing under another name, and with no membership table in M1 "the org can see it" resolves through `client_tokens.org_id`, which `work_admin { assign_client }` writes under `Access::Master` — the exact hole that removed `session_share { org }` from M1. `'org'` returns in M2 with memberships (§4.3, Q10) |
| "An admin may revoke or narrow a departed member's grants" — promised as an M1 capability (Q9) | **That authority arrives in M2, with memberships.** M1 has no membership, so it has no such authority to express and builds no task for one. What M1 does deliver is unchanged: a grant is downward-only, and only the owner creates one (Q9, §4.3) |
| The unclaimed count goes to "whoever may already see that host — the operator on a shared one" | **One person, or nobody.** On a hub with exactly one person the count is served to that person. On a hub with more than one it is served to **nobody through the API in M1**; the operator reads it with `fleet-hub`. The master token is **not** resolved to the personal owner for this, and "the operator" is not written anywhere the code would mean "the master token" — they are different callers, and conflating them is how the `'org'` class of hole gets in (§4.3) |
| "B can watch it" is served by the session reads that already exist | **A scope addition, named as one.** `capture_session` and `session_transcript` are MCP-only, so on the desktop a watcher has no live view of the pane — which a grant deliberately does not confer either. M1 adds the minimum a watcher needs — a routed pane snapshot; the transcript-shaped reads a watcher uses are already routed — and says so as an addition rather than letting it read as if it were always planned (§2.4, §5.2) |
| `new_session { resume_claude_session_id }` is refused by a check against the session rows fleet holds | **A durable `claude_session_id → owner` record is required**, and it belongs in the ownership migration. The attack works precisely when the row is gone, so a check against live rows cannot close it. The shape is the implementing task's choice; the requirement is that the record is durable, survives reaping and GC, and is consulted before any resume (§5.2) |
| `Access::Person` is an observation about a hole | **An M1 obligation with a task.** It means "a paired client bound to no org", which the moment there are two people is *any* person — so the settings-write gate would let any person write the fleet's settings. M1 binds it to the hub's personal owner (§2.7, §6) |
| Counts and bare line numbers as evidence | **Queries and named anchors.** Three independent verifiers produced three different numbers for one query, and a dozen cited lines had drifted by one to twenty. This revision states the query — "every non-test call site of `require_visible_session`" — and anchors on `path/file.rs::function`, `migrations/0NN_name.sql` or a test name. A bare `:line` survives only where the anchor is an unnamed statement, and then what sits at that line is named with it. Removing a false count is a fix, not a loss of detail |

**One verification finding changed a design rather than a citation**, and it is
recorded where it bites (§4.3, §4.4): `sessions.tmux_pane_id` — the column the
whole claim path rests on — is written by the **reconcile** pass, not by the
hook path revision 4 cited, and what it records is the *active* pane of the
tmux session as of the last pass. That is good news for the rows the claim path
exists for and bad news for one case; both are stated in §4.3.

**Revision 6 (owner's delegate, after an implementer's read of revision 5,
2026-09-30).** Revision 5 closed its contradictions and left one premise
unverified, one record that inverts the guarantee it was added to provide, one
field on the wrong carrier, and one DDL with an ellipsis in it. An implementer
cannot start on any of the four. Four decisions close them. They are binding,
they are applied below rather than argued again, and the table breaks them into
the five corrections they make — the pane decision moves two things at once:
where the proof travels, and a record it makes unnecessary.

| Was | Corrected to |
|---|---|
| The pane proof travels on a *call*, and whether an MCP header can carry it is an **unverified premise** (revision 5's closing paragraph) | **Verified, and the proof travels on the CONNECTION.** Claude Code expands `${VAR}` and `${VAR:-default}` inside an MCP server entry's `headers` and `url`, with **no** allow-list key required — `allowedEnvVars` is a HOOKS-only mechanism — and the blanking rule that empties a variable covers credentials (`ANTHROPIC_API_KEY` and its siblings), which `TMUX_PANE` is not. So `service/provision.rs`'s `mcpServers` entry gains `"X-Fleet-Pane": "${TMUX_PANE:-}"` beside its `Authorization` header, `mcp/mod.rs::authorize` reads and validates it, and it lands on `Caller` as `pane: Option<String>`. Clause 2 of §4.4's host-token rule is then evaluable on **every** tool, not only on the three that could take a pane argument. The **braced** form is required: the hooks entry's bare `$TMUX_PANE` works only because hooks have `allowedEnvVars`, so copying that syntax into the MCP entry ships an unexpanded literal (§4.4) |
| A durable `(host_alias, pane_id) → session_id` record backs the pane proof | **Deleted, and it was a mistake.** Keyed by host alias, it makes every proven pane reachable by every agent on that host — it inverts the very guarantee it was added to provide. The connection header removes the need for it entirely (§4.4) |
| The desktop reports a per-caller access field **on the session row** | **Nothing per-caller ever rides a `SessionRow`.** The row carries `owner_person_id` and `visibility` — caller-independent facts, safe on a broadcast. Each client holds its own person id and its own **grant set**, and *derives* access from the three. The pipeline forbids the alternative: the bus serialises a bare row with no caller, `strip_nulls` removes absent keys on the way out, and the frontend row store replaces a row wholesale — so every routine `session:updated` would erase a per-caller field, and a fail-closed default would then shut the **owner's own** terminal on a paired desktop. The `needs_attention` precedent does not transfer; that field is not per-caller (§5.3, §2.4) |
| `people (id, name, display_name, created_at, …)`, with the personal owner found by name | **The complete DDL is written out once, in the plan's T1, and cited from here.** `disabled_at` — which three tasks require — is a column of it, not an omission. The personal owner is marked by `is_personal_owner INTEGER NOT NULL DEFAULT 0` under a partial unique index, never by name (the name is explicitly renameable) and never by "the lowest id"; if it is somehow absent, no person scope resolves at all (§4.1) |
| Silent on the two layering boundaries the first compile hits | `store/session_grants.rs`'s ownership check is a plain column comparison against `sessions.owner_person_id`, **not** a call to `ViewScope::owns`: `store/` does not import `service/`, and the dependency runs the other way. And `guard::access_allows` stays **store-free** — it is shared with the store-free `mcp/tools/present.rs::visible_to` — so "is this caller the hub's personal owner?" is resolved once, when the token is resolved, and travels on `Caller` as a boolean (§4.1, §4.3, §5.1) |

Every file:line below was read in the worktree at `77653006`; where an earlier
revision cited a line that has moved or was never right, the citation is
corrected in place — or replaced by a name — rather than footnoted.

**Input:** the owner's handover *"viac používateľov, súkromné sessions a
prepojenie organizácií"* (2026-09-30). The goal it states: one developer has a
personal identity, personal sessions and a personal environment, works alone or
for several employers, enters a company's hub through a membership with
permissions, and does **not** have to run a personal hub or hand the company
their private environment. *Keep it simple.*

**Companion:** `docs/superpowers/plans/2026-09-30-multi-user-m1-private-sessions.md`
— the task list for milestone 1.

---

## 1. The short version

Fleet today is **single-tenant by construction**. Not by accident, and not
shallowly: the whole access-control story is *one* dimension — the **org** —
and the org is derived from *where work lives* (host, repo owner, repo, path),
never from *who did it*.

There is no user. There is no session owner. There is no membership. The four
identities the hub knows are **token kinds**, not people:

| Token | `Caller` shape | Scope |
|---|---|---|
| master | `host_alias: None, client: None` | `OrgScope::All` — everything |
| per-host | `host_alias: Some(a)` | `OrgScope::Host` — its host + its org |
| paired client, unbound | `client: Some(_), org_id: None` | `OrgScope::All` — everything |
| paired client, org-bound | `client: Some(_), org_id: Some(o)` | `OrgScope::Org` |

`crates/fleet-core/src/mcp/auth.rs:97` (`Caller`), `service/orgs.rs`
(`OrgScope`), `Caller::org_scope` — the one place a caller becomes a scope.

Three consequences that shape everything below:

1. **Two people paired to one hub today see each other's everything.** Both are
   unbound `full` clients → both are `OrgScope::All` → every session, every
   transcript, every prompt, kill and restart. Org-binding narrows this, but an
   org is a *company*, not a person; two colleagues in the same org are still
   `All` relative to each other.
2. **A session has no owner column.** Read `store/rows.rs::SessionRow` and the
   `SessionRow` entry of `src-tauri/src/backend/hub_contract.golden.json` for
   what it carries — not one field says who created it. `org_id` is not stored
   either: it is computed per query by the `session_org_sql!` macro from
   `org_rules`. Ownership must be a **stored** column; it cannot be derived
   from anything Fleet knows today. The struct's size is not trivia: it is far
   wider than the six fields a redaction list written from memory names, and
   the ones such a list misses — `pr_url`, `worktree_key`, `account_uuid`,
   `current_activity`, `claude_session_id`, `parent_session_id` — are every
   one of them content (§4.3). That is the argument for a positive list, in
   §4.3, over a redaction list nobody can keep in step with the struct.
3. **A per-host token is "every agent on that machine", one shared identity.**
   `host_tokens` has `host_alias` as PRIMARY KEY — one token per host, written
   into that host's `~/.claude.json` (`service/provision.rs`). So every
   Claude on a shared host can list, read and prompt every other session on it.
   This is the single biggest obstacle to "private by default", and §4.4 says
   what to do about it — but with two corrections revision 3 did not have.

   First, `OrgScope::sees_session` (`service/orgs.rs::OrgScope::sees_session`)
   has **three** unconditional wins in its `Host` arm, not one and not the two
   revision 4 named. The predicate is a single line, and revision 4 quoted it
   truncated at both places it quoted it:

   ```rust
   if row_host == alias || row_org.is_none() || row_org == *org { return true; }
   ```

   `row_host == alias` is the one revision 3 knew about. `row_org.is_none()`
   makes an **unassigned** session visible to every host token on the fleet,
   wherever it runs. `row_org == *org` is the widest of the three: a host
   token in org X reads every session of org X on every host in the fleet. And
   the fall-through beneath them — `!(theirs || mine)`, which is false only
   when one side isolates — is permissive by default, so the `Host` arm is
   closer to `All` than any earlier revision of this document said. An
   implementer who narrows the two clauses revision 4 enumerated leaves the
   third and the fall-through in place.

   Second, and worse for revision 3's fix, `sees_session` is not on the path a
   host token takes to a single session at all: `resolve_row_and_gate`
   (`mcp/tools/support.rs::resolve_row_and_gate`) is three lines —
   `resolve_session_target`, `require_host`, `require_bound_client_sees` — and
   `require_bound_client_sees` returns `Ok(())` in its first statement for any
   caller whose `client` is `None`, which a host token's is. Narrowing
   `sees_session` therefore changes nothing for `capture_session`,
   `session_transcript`, `send_prompt` or any other tool that resolves a
   session through that gate. The list is `grep -n resolve_row_and_gate
   crates/fleet-core/src` minus tests; take it from the compiler, not from a
   number in this document.

The half-good news: the enforcement surface is **already funnelled**. Adding a
second access dimension does not mean touching every `TOOL_POLICIES` row and
every `VERDICTS` row one by one. It does not mean widening six predicates
either: four of the six do nothing at all for the caller M1 introduces, three
further surfaces are not in the list, and a further set of tools reaches
session data without passing any of them. §5 is the honest inventory — §5.1
the choke points, §5.2 the paths that go round them.

---

## 2. What the requirements ask for, against what exists

Legend: **✔** exists · **~** partly exists · **✘** does not exist.

### 2.1 Personal identity and devices

| Requirement | Today | Gap |
|---|---|---|
| Works for one person out of the box, no registration | ✔ | none — a standalone desktop *is* the master, no login anywhere |
| Phone joins the same personal profile without re-setup | ~ | pairing exists (`pair_client` → `POST /pair`, single-use code); but a pairing makes a *device row*, not a person. Two devices of one person are two unrelated `client_tokens` rows |
| Same identity on desktop + other devices | ✘ | `client_tokens` has no person. `ClientRef` = `{id, name, trusted, org_id}` |
| One personal account joins several companies | ✘ | no person, no membership table |
| Working for a company needs no personal hub | ~ | a desktop already pairs to a hub as a client and routes every command (`src-tauri/src/backend/`, one `VERDICTS` row per command). What is missing is only that the hub cannot tell *which person* that client is |

**Read:** the *device* half is built and solid. The *person* half is missing
entirely. That is one table plus a nullable FK, not an architecture.

### 2.2 Personal and work spaces (the switcher)

| Requirement | Today | Gap |
|---|---|---|
| Switch between personal space and each company | ✘ | the desktop resolves ONCE at startup, for the whole process lifetime, and to one of **three** states, not two — `Backend::{Local, Remote, Unavailable}` (`src-tauri/src/backend/mod.rs::Backend`). `Unavailable` (a hub configured but unreachable this launch) is load-bearing: `FleetBackend::from_resolved` hands it a deliberately refusing `HubBackend` so a routed command fails `E_HUB_UNAVAILABLE` instead of falling back to the local arm and becoming a second brain for the hub's fleet. M1's new sharing commands inherit that behaviour for free |
| Experimental at first | — | fits `settings` + a page field |

**The discriminator for "this process is the master" already exists, and M1
must reuse it rather than invent one.** `Backend::owns_the_fleet()`
(`src-tauri/src/backend/mod.rs::Backend::owns_the_fleet`) is
`matches!(self, Backend::Local)` — one line, with a doc comment that spells out
why `Unavailable` answers **false**: "the operator said the hub owns this
fleet, and a failure to reach it does not change whose fleet it is." Any
question of the form "on a standalone desktop the caller is the owner, on a
paired one it is not" — what the client assumes when it holds no person id of
its own, most of all (§5.3) — is answered against that predicate. A two-armed
"split by backend mode" re-derives it and silently loses the third state at the
one place the desktop decides whether a terminal opens.

**Read:** this is the item with a real architectural cost, and it is **not**
where the value is. "One space per window/process, switch = re-resolve" is
cheap; "several spaces live at once in one window" means every store, every
event stream and the single global PTY become per-space. Recommendation in
§4.6: ship the cheap one, keep it experimental, as the owner asked.

### 2.3 Session privacy

| Requirement | Today | Gap |
|---|---|---|
| A new session is visible only to its creator | ✘ | no owner, no visibility column |
| Privacy holds against the company admin **inside the app** | ✘ | the master is `All` by definition and by design |
| Enforced at data access, not by filtering a list | ~ | the *shape* is right — `list_sessions`' scope filter, `resolve_row_and_gate`, `require_visible_session`, the result gate `redact_work_via`, `fence_frame` on `/events` — but see the read below: for a person's device four of those five do not run at all |

**Read (rewritten in revision 4).** The funnel exists and is worth having. What
revision 3 said next — "the enforcement architecture is already correct, it is
filtering on the wrong dimension" — is wrong in the way most likely to mislead
whoever implements it, so it is struck. A person's device is an **unbound
paired client**, and `Caller::is_scoped()` (`mcp/auth.rs::Caller::is_scoped`) is
`host_alias.is_some() || client.org_id.is_some()` — false for it. So:

| Gate | What it does for a person's device *today* |
|---|---|
| `require_visible_session` (`mcp/tools/support.rs::require_visible_session`) | opens `if !caller.is_scoped() { return Ok(()) }`. It is the **sole** gate on `session_history` and on every `repo_*` read, `repo_file` included; `rewind_conversation` calls it too, with a second gate behind it. Take the list from `grep -n require_visible_session crates/fleet-core/src` minus tests — a missed call site is a leak, and an enumeration in a document is a list that stops being true |
| `require_bound_client_sees` (`support.rs::require_bound_client_sees`) | returns `Ok(())` in its first statement for any caller with no `org_id` — and for every host token, whose `client` is `None` |
| `redact_work_via` (`support.rs::redact_work_via`) | never called: `mcp/tools/mod.rs` wraps it in `if caller.is_scoped()`. If it were, it would return early on `scope.is_all()`; and it can only delete keys in `WORK_FIELDS`, never drop a row from an array |
| `fence_frame` (`mcp/events_route.rs::fence_frame`) | returns the payload verbatim on `scope.is_all()`, which `OrgScope::All`'s own doc comment documents as covering "an unbound paired client" |
| `list_sessions`' filter (`mcp/tools/session_ops.rs::list_sessions`) | runs, and answers `true` for every row, because the scope is `OrgScope::All` |

Only the last is a predicate to widen. The other four must first be made to
**run**, and the third must learn a capability it does not have. That is the
difference between a task and a milestone, and it is the most consequential
correction in this revision.

### 2.4 Sharing a session

| Requirement | Today | Gap |
|---|---|---|
| Share with one colleague / with a team | ✘ | nothing grants access to anything, per row |
| Watch-only vs. can-send-prompts | ~ | `TokenMode::Readonly` is token-wide, not per session. A per-session split does not exist |
| Revoke later | ✘ | — |
| Recipient also gets history before the share | ~ | history is per session (`session_history`, `session_transcript`, `conversations`); granting the session grants them |
| Sharing must not hand over AI credentials | ✔ | credentials live in the host's `~/.claude`; Fleet never carries them. `accounts` holds a uuid/nickname, not a secret |

**The terminal, and what sharing must therefore not do.** The `pty_open` and
`pty_write` rows in `src-tauri/src/backend/verdicts.rs` are **`SameInBoth`**:
the terminal attaches by the desktop's *own* `ssh … tmux attach`, straight from
the user's machine to the host. The hub is not in that path and cannot revoke
it.

Revision 1 drew the wrong conclusion — "so watch-only can only be documented".
The right one is a design rule:

> **Sharing a session never confers a terminal.** Access obtained by a grant is
> hub-mediated and therefore revocable; direct SSH is a machine-level permission
> Fleet does not grant and does not claim to revoke.

So `watch` is served through the hub — `session_transcript`,
`session_conversation`, `session_history`, `capture_session` — and `drive` adds
`send_prompt` / `dispatch_task`, also through the hub.

**Four corrections revision 4 adds, each of which defeats that rule as written.**

1. **There is no "Attach action" to withhold.** The terminal is a pane that
   attaches *automatically* the moment a row is selected. In `src/App.svelte`,
   the `{#if $selectedSession && selNoPane}` branch renders the no-pane
   placeholder and its `{:else}` mounts `<TerminalView />` — so every row that
   is not `hasNoPane` gets one; `TerminalView.svelte::openTerm` fires off
   `$selectedSession` and calls `invoke('pty_open')` with no user gesture. The
   gate is that mount condition plus an early return inside `openTerm` — and
   it needs an **active** `pty_close` the moment the client's derived access
   for that row stops being the owner's, not only a refusal to open. Derived,
   not read: the condition and the close are evaluated from the row's
   `owner_person_id` and `visibility` against the client's own person id and
   grant set (§5.3), because no per-caller field rides a `SessionRow`. That
   this gate is client-side is a property of the **terminal**, not a weakening
   of the model: the PTY bypasses the hub entirely, so there is no server to
   put it on. `pty_open` (`src-tauri/src/pty.rs`)
   takes no session id, reads no `state.db`, and its `VERDICTS` row is
   `SameInBoth`: once it is open the hub cannot reach it, and
   `selectedSession` clears only when the row leaves the store — which, after
   a revoke, may not happen at all while the window stays focused (§5.2). Note
   also that `resolveSessionView` (`src/lib/session_view.ts::resolveSessionView`)
   *falls back to the terminal* for a session with no `claude_session_id`, so
   the access check has to be evaluated before that fallback, not after.
2. **The desktop already hands a watcher the incantation, in plain text.**
   `SessionDetails` renders a section headed *"Attach from another terminal"*
   containing `tmux attach -t ${session.tmux_name}` inside a
   `<code data-testid="attach-command">` with a copy button
   (`src/lib/SessionDetails.svelte`, the `attach-command` testid). That is both
   the command and `tmux_name`, which §4.3 classes as content. It must be gated
   in the same commit as the pane, or the rule is decorative.
3. **`upload_to_session` is a second write channel with no hub in its path.**
   It is the terminal pane's drop handler (`src/lib/TerminalView.svelte`'s drop
   handler → `src-tauri/src/commands/upload.rs::upload_to_session`) and its
   `VERDICTS` row is `SameInBoth` for the recorded reason that "the bytes are
   on this machine and so is the `ssh` that carries them". A grantee with the
   pane gated and the drop handler open still stages arbitrary files on the
   owner's host.
4. **A desktop watcher has less to watch than the level implies — so M1 adds
   commands. This is a scope addition, stated as one.** `capture_session` and
   `session_transcript` are MCP-only: neither name appears in `verdicts.rs`,
   and no frontend surface calls them. What the desktop *does* have, as
   routed commands with verdict rows today, is `session_conversation`,
   `session_conversations`, `session_history`, `session_activity`,
   `session_tool_detail` and `related_sessions` — the transcript-shaped reads.
   What it does not have is any view of the **live pane**, because the only
   one that exists is the PTY, and rule 4 of §4.3 forbids a grant from
   conferring that.

   So "B can watch it" is, as things stand, unreachable on the desktop in the
   sense a user means it, and revision 4 wrote the acceptance criterion as if
   it were not. M1 adds the minimum that closes the gap, and the reason is the
   rule, not an oversight: **a watcher needs a read-only view of the screen
   precisely because they may not have the terminal.** Concretely, that is a
   routed `capture_session` — a pane snapshot the desktop can render where the
   terminal would be — with its `VERDICTS` row, its `route()` literal, its
   driving case and the four regenerations of §5.3. `session_transcript` is
   *not* part of the addition: it answers prose, and the desktop's conversation
   view is already served by `session_conversation`, which is routed.

   The plan owns the task; this document owns the reason it exists and the
   fact that it was added in revision 5 rather than planned in revision 3.

**And two grantable levels are not enough.** Several operations turn a grant
into access that revocation cannot reach, and none of them is on revision 3's
deny list: `spawn_review` (`mcp/tools/lifecycle.rs::spawn_review`) is gated by a
*read* gate and creates a session in the owner's worktree, on the owner's host,
which the caller would own — with a terminal; `move_session` relocates the
session's uncommitted work, Claude directory and project memory onto a host the
caller names; `rewind_conversation` (fork, rewind and retry alike) makes a
permanent verbatim copy of the transcript; `restore_host_sessions` and
`work_link { summarize }` produce durable copies of content;
`send_message { deliver, submit }` types into the pane and presses Enter.

§4.3's invariant 5 answers this with a positive **`own` tier** rather than a
longer deny list — a deny list is a promise to remember every future tool, and
this one already had six holes in it. **The membership of that tier is defined
in §4.3 and nowhere else.** This paragraph deliberately does not restate it:
revision 4 shipped three copies of the list, here, in Q6 and in the plan, and
all three disagreed on whether a `drive` grantee may kill or restart the
owner's session. One place, cited from everywhere.

Someone who independently has SSH to that host can of course still attach; that
is their machine access, unchanged by sharing and unaffected by revoking it.
The two are cleanly separable — but only once all four surfaces above are
closed, not `pty_open` alone.

### 2.5 Where a session runs, and which AI account

| Requirement | Today | Gap |
|---|---|---|
| Run a work session on my own machine or on a company host | ✔ | hosts are per-fleet SSH destinations; a host can be anything the hub reaches |
| Use my own Claude account or the company's | ~ | `accounts` + `sessions.account_uuid` + `hosts.account_uuid` exist, discovered from `~/.claude.json` during the probe. The choice is **per host**, not per session |
| …independently, on the same machine | ✘ | nothing selects an account at session start. The *mechanism* exists but is unused: `CLAUDE_CONFIG_DIR` is already honoured in the usage script (`service/account_usage.rs`), Codex has `CODEX_HOME`, and the pane is launched with `tmux new-session -e KEY=VAL` (`tmux.rs`) |
| Use a Codex account | ✘ | **not modelled at all.** `accounts` reads `~/.claude.json` only; the session runtime is Claude-only (`docs/superpowers/specs/2026-09-29-multi-harness-agents-design.md` §3: "accounts read `~/.claude.json`", no harness concept on `SessionRow`) |
| Company accounts handed out without leaking credentials | ✘ | no distribution mechanism, and none should be built (Q7) |

**Read:** two distinct gaps, and revision 1 collapsed them. *Never moving a
credential* is right and should stay. *One account per machine* is a limitation,
not a design — and the plumbing to lift it (a per-session config-dir env var) is
already in the codebase. Q7 separates them.

**Codex is out of scope for this document.** Multi-harness has its own umbrella
spec; anything here that says "account" means a Claude account until that lands,
and Q7 says what must not be assumed in the meantime.

### 2.6 Keeping and syncing the organisation

| Requirement | Today | Gap |
|---|---|---|
| A personal hub's org stays private and intact | ✔ | it is a local `orgs` row; nothing exfiltrates it |
| Choose which parts sync | ✘ | no sync of orgs between hubs exists at all |
| Automatic sync, pausable | ✘ | — |
| Write into the work org within my permissions | ✘ | — |
| Pick a version on a concurrent edit | ~ | the *pattern* exists: `work_links.version` + `expected_version` + `E_CONFLICT` (migration 066, M14.1a). Reusable |
| Federation transport | ✔ | hub↔hub links are built: `fleet-hub pair --mode peer`, `peer_exchange`, dialer supervisor, `fleet_health.peer_links_down` (`docs/superpowers/specs/2026-09-24-hub-federation-design.md`) |

**Read:** the *pipe* between two hubs exists and is the hard part. What is
missing is a payload: which entities, how they are identified across hubs, and
who may apply them. That is a design of its own and it is **after** M1.

### 2.7 Permissions are granted by the company

| Requirement | Today | Gap |
|---|---|---|
| The company grants permissions; only a permitted person changes work settings; same rule when the change arrives via sync from a personal hub | ~ | per-client grants exist as a pattern — `client_tokens.assets_admin_at` (migration 074), and settings writes need a trusted `full` device (`settings_writer` in `mcp/tools/fleet.rs`). But grants are per *device*, not per *person*, and there is no role/membership |

**The rule to encode once:** an incoming synced change is **not** authority. It
is a proposal evaluated under the recipient's current permissions at the moment
of application. `service/settings_review.rs` (P5 proposals, migration 083)
already has exactly this shape — reuse it, do not invent a second one.

**`Access::Person` is already wrong for two people — and M1 owes it a task
(owner's delegate, revision 5).** `Caller::is_person_device()`
(`mcp/auth.rs::Caller::is_person_device`) is
`host_alias.is_none() && mode != Peer && client.org_id.is_none()` — "a paired
client bound to no org", which is precisely what *both* people's devices are on
a two-person hub. `guard::access_allows` reads
`Some(Access::Person) => caller.is_master() || caller.is_person_device()`. So on
the day a second person is paired, their device reaches `get_settings` for the
whole fleet, and once the operator trusts it, `set_setting` as well
(`settings_writer`, `mcp/tools/fleet.rs`). Declarative pages P6 built that level
for the single-person case and it does not survive a second person.

Revision 4 recorded this as an observation. **It is an M1 obligation with a
named task:** `Access::Person` must bind to the hub's *personal owner* —
`is_person_device()` and the person on the caller, not `is_person_device()`
alone — so that "a person's device" means one person's device and not anyone's.
The change is small; the hole is real the moment a second person exists, and it
is a settings **write**, not a read. §6 carries it in M1's obligations for the
same reason.

**And the binding is a boolean on `Caller`, not a lookup inside the gate**
(revision 6, §4.1). `access_allows` is store-free and shares its answers with
the equally store-free `mcp/tools/present.rs::visible_to`; "is this caller the
hub's personal owner?" is resolved once, where the token is resolved, and the
gate reads the result.

### 2.8 Working offline, and losing membership

| Requirement | Today | Gap |
|---|---|---|
| A locally running session survives the hub going away | ✔ | tmux on the host does not care about the hub |
| Start a new work session locally while the hub is unreachable | ✘ | a paired desktop routes *everything* to the hub; no hub, no `new_session` |
| Queue changes and send them when the hub returns | ~ | the outbox pattern exists (`tracker_writes`, migration 061, drained by the sync pass). Reusable shape |
| Re-check permissions before accepting queued changes | ✘ | see §2.7 — the rule, not the code |
| Losing membership stops hub access and sync | ~ | revoking a client token is immediate (`Store::active_client_tokens` drops revoked rows; `auth_epoch` invalidates the caller cache from the next request) |
| Local history of work sessions that ran on my machine stays | ✔ | nothing deletes it; no remote-wipe exists and none was asked for |

**Read:** "keep working while the company hub is down" is the item most likely
to be underestimated. Today a paired desktop is a *window onto a hub* — it has
no local fleet of its own. Making it able to start and own a session locally and
reconcile later is a mode change, not a feature flag. It belongs late.

---

## 3. The facts that constrain any design

Revision 3 listed five. Two of them were wrong in detail, and the recon found
four more that are load-bearing.

### 3.1 There is no session create path to hang an owner on

`org_id` on a session is derived, not stored: the `session_org_sql!` macro
(`store/orgs.rs`) computes it per query from `org_rules`. An owner cannot work
that way — it must be written at insert. But revision 3's "sessions are
inserted from several places (reconcile's upsert, `upsert_session`, the bg
path)" has been false since 2026-09-15, and
`migrations/045_participant_on_insert.sql`'s own header repeats the same stale
claim. `Store::upsert_session` is `#[cfg(test)]` and every one of its callers
is a test. Production has **two** INSERTs — `store/reconcile.rs`'s upsert and
`store/sessions.rs::upsert_bg_session` — and **both are reached only from a
reconcile pass**. (The new migrations M1 adds must not repeat the stale count
in their own headers, and correcting 045's header is a free line while someone
is in there.)

That is not a counting error; it removes the place revision 3 meant to write the
owner. `service/sessions/lifecycle.rs::new_session_inner` starts tmux, calls
`reconcile_one_host`, and then *finds* the row it created. The create path and
the discovery path are the same SQL statement, so "the create path writes owner
+ `private`" and "reconcile's discovery path writes `unclaimed`" cannot be two
code paths — the distinction has to be *carried in*. And because
`reconcile_one_host` is deliberately ungated against the background pass
(`service/sessions/reconcile.rs`), the row — and its `session:created` frame —
can exist at the schema default before anything stamps an owner.

**Consequence for M1:** the intended owner must be **reserved before tmux
starts**, alongside the existing in-memory `record_tmux_created` marker
(`store/reconcile.rs`), and written by that one upsert on INSERT (`COALESCE`d
on DO UPDATE). A stamp-afterwards design leaves a window in which a live, owned
session is `unclaimed` — and §4.3 makes an `unclaimed` row claimable by anything
that can prove the pane on that host. Migration 045's `AFTER INSERT` trigger
remains the precedent for a *default*, not for an owner.

Further create paths are not in either document's earlier list, and two that
were listed are not create paths at all.
`service/sessions/review.rs::spawn_review`, `move_session`'s target row, and
`lifecycle.rs::restart_session`'s `None` branch **are** create paths;
`recreate_session` (which keeps the row via `restore_session` and says so in
its own comment) and `rename_session` (which carries the row over *before* the
reconcile precisely so the pass does not insert one) are **not**, and preserve
the owner for free. Both halves matter: the first is where an owner must be
carried in, and the second is why `recreate_session` reaching the same effect
as `restore_host_sessions` one row at a time is a gating question, not a
create-path question (§4.3, invariant 5).

### 3.2 Reconcile creates session rows Fleet never asked for

A tmux session started by hand on a host becomes a row. It has no creator. Every
ownership design needs an answer for "discovered, owner unknown" — and the
answer must not be "private to nobody", which would hide it from everyone.

A live code path makes this worse than a default.
`lifecycle.rs::reject_lost_session_name` refuses a name only when
`Store::lost_resumable_session_named` finds a row, and that query requires
`claude_session_id IS NOT NULL`. A lost *shell* session, or any row whose
`set_claude_session_id` soft-failed, is not refused — so person B starting a
session under a tmux name person A once used on that host is resurrected by the
upsert's `ON CONFLICT DO UPDATE` **with A's owner intact**. B's new session
would then be owned by, and readable by, A; B could not see it at all. M1 must
widen that refusal.

### 3.3 Something must stay unscoped — but it must not be a `Caller`

The hub operator runs `fleet-hub` on the box; GC, reconcile, playbooks and the
attention roll-up all read every row. Privacy from the company admin means the
*admin's client* is not unscoped — it does not and cannot mean no code path is.

Revision 3 stopped there, and that is exactly the gap. `Caller::org_scope`
(`mcp/auth.rs::Caller::org_scope`) returns `OrgScope::All` for **any** client
with no org, so "the hub's own GC tick" and "a bearer token that happens to name
no person" are today the same value. If M1 expresses internal readers as a
person-less `Caller`, then (a) every device paired before the upgrade keeps
full-fleet read afterwards, and (b) anyone who can run `fleet-hub pair` mints an
in-app device that reads every private session — the admin override §4.5 and
Q11 refuse, reachable with no database access at all.

**The rule M1 must encode:** internal readers get a scope **constructed
directly** (`ViewScope::internal()`), never derived from a token; a scope built
from a token that names no person is a **refusing** scope for session rows. The
master token resolves to the hub's personal owner — right on a standalone
install, and on a shared hub it means the operator sees their own sessions and
not their colleagues'.

**That resolution is a scope, and it is not a licence to write "the
operator".** A `fleet-hub` subcommand running on the box, the master token
arriving over MCP, and the hub's own GC tick are three different callers, and
this document names whichever one it means. Where an answer is owed to the
human who administers the machine rather than to any token — the unclaimed
count on a hub with more than one person is the only such case in M1 — the
answer is a `fleet-hub` subcommand and **not** an API surface (§4.3). Writing
"the operator" where the code would mean "the master token" is how the `'org'`
class of hole gets in.

### 3.4 The terminal bypasses the hub — and so do two more channels

`pty_open` / `pty_write` are the desktop's own `ssh … tmux attach`
(§2.4). So is `upload_to_session`, the pane's drop handler. And the desktop
*prints* the attach command with a copy button in `SessionDetails`. Any promise
about read-only access is an application-level promise over three surfaces, not
one.

### 3.5 A per-host token is the host, not a person, and cannot prove which session it is

`host_tokens` is PK'd by `host_alias` (§1). Two people sharing one unix account
on one host cannot be told apart by anything Fleet has — **and neither can two
agents in two panes of the same host**. That second half is what makes revision
3's §4.4 backstop unsatisfiable rather than merely weak, and §4.4 is rewritten
around the one discriminator that does exist: the pane. §4.3 says what that
discriminator is actually worth, which is less than revision 4 assumed and
enough.

### 3.6 `is_all()` is a leak class, not a value

Run `grep -rn '\.is_all()' crates/fleet-core/src src-tauri/src` and read the
non-test hits. A meaningful share of them guard **session** reads — among them
`OrgScope`'s own helpers in `service/orgs.rs`, `service/messages.rs`'s
recipient checks, `service/sessions/targeting.rs`, `mcp/tools/messaging.rs`,
`mcp/tools/support.rs::redact_work_via`, `mcp/events_route.rs::fence_frame`,
and `HealthView::Fleet`. Each is an early return that *stays compiling and
stays silently leaky* the moment `All` can also carry a person. The compiler
flags none of them.

That is the decisive argument against adding a person field to `OrgScope` in
place, and the reason §5.1 insists the old session predicates be **removed or
renamed** rather than left working. The number of such sites is not the
argument and is not asserted here: three independent reads of this repository
produced three different totals for the same query, and an implementer who
budgets against a total in a document rather than against the compiler will
believe they are finished when the count is met.

### 3.7 A nullable column is *absent*, not null, on the event stream

`BroadcastEventBus::emit` (`events.rs`) runs `strip_nulls` on the payload
**before** the frame enters the replay ring, and `SessionRow::org_id`
additionally carries `skip_serializing_if = "Option::is_none"`. A new nullable
`owner_person_id` therefore arrives at the fence with the key **missing**,
indistinguishable from a build that predates the column — and the natural
reading of "absent" is "no restriction". Any fence must key on a
`TEXT NOT NULL DEFAULT 'unclaimed'` visibility column, and must **drop** a
session frame that carries none.

**And "fails closed when absent" is a serde attribute, not a hope.** A bare
`#[serde(default)]` on a `String` field yields `String::default()` — the empty
string — so a `SessionRow` deserialised from an older hub's frame would read
`visibility == ""`, which is neither `private` nor `unclaimed` and matches no
arm anybody writes. The field needs `#[serde(default = "…")]` naming a function
that returns `"unclaimed"`, following the repo's own convention for exactly
this (`store/work.rs`'s `default_role`, `service/messages.rs`'s `default_true`,
`service/sessions/prompt.rs`'s `default_submit`). The test that pins it must
assert the value, not merely that deserialisation succeeded: a `SessionRow`
deserialised with no `visibility` key reads `unclaimed`.

### 3.8 A grant change mutates no row, so it emits nothing

Sharing a session writes a `session_grants` row and no `sessions` row, so no
frame is produced and the recipient's sidebar stays empty until the session next
changes on its own. The codebase has the precedent for exactly this shape:
`store/orgs.rs::announce_org_moves` exists because an org move is also a
computed change with no column behind it, and it bumps `row_version` and
re-emits `session_updated` per affected row. Grants need the same announce; "it
arrives by row event" is not free.

### 3.9 The migrations move a number that is published on the wire

M1's migrations move `known_schema_version()` past the last applied one (085 at
`77653006`). `crates/fleet-hub/src/main.rs::compat_json` emits
`store.schema_to`, a test in the same file holds it, and
`scripts/release-manifest.sh` signs the update windows read from the shipped
`fleet-hub compat`. Once M1 ships, a hub that has opened an M1
database can never be reopened by a pre-M1 build except read-only. That is
acceptable and normal; it must be *stated* in the release notes, not discovered.

---

## 4. Design decisions — recommendations

Each is a recommendation with a reason. Ordered by how much they cost if wrong.

### 4.1 Person = a row; device = an existing `client_tokens` row pointing at it

Add a `people` table and `client_tokens.person_id INTEGER` (nullable, no FK
cascade — same reasoning as `client_tokens.org_id` in migration 066: deleting a
person must fail closed, not widen).

**The `people` DDL is written out in full in one place — the companion plan's
T1 — and this document cites it rather than carrying a sketch.** Revisions 4
and 5 gave it here as `people (id, name, display_name, created_at, …)`, and the
ellipsis swallowed `disabled_at`, which three of the plan's own tasks depend on
(Q2's fourth revocation event, the scope builder, and the departure answer in
Q9). A column list with an ellipsis in it is not a schema, and two documents
each carrying half of one is how a required column goes missing.

**Every hub has at least one person, created automatically.** On first run (or on
upgrade) a hub mints its own personal owner — no registration, no prompt, no
name to invent; the desktop's existing identity simply acquires a row. This is
the correction from revision 1, and it is the better design on its own merits:
*ownership of a session is then never ambiguous*, which is the property M1
actually needs. Zero-identity was optimising for the wrong thing — a user never
sees the row either way.

**How the personal owner is found — revision 6.** By a column, not by a name
and not by an ordering. `people.is_personal_owner INTEGER NOT NULL DEFAULT 0`,
with `CREATE UNIQUE INDEX … ON people(is_personal_owner) WHERE
is_personal_owner = 1`, so the database itself guarantees there is at most one;
`Store::personal_owner_id()` is then a single indexed read. Both of the obvious
alternatives are wrong in ways that surface late:

- **By name** — the name is explicitly renameable (§4.1's profile layer, and Q2
  lists renaming among the things a person does), so a rename would silently
  re-home the fleet's owner or orphan it;
- **"the lowest id"** — fragile the first time a row is deleted and re-created,
  and it encodes an accident of insertion order as a security fact.

**If the marked row is somehow absent, the hub fails closed:** no person scope
resolves — not the master token's, not a device's — rather than falling back to
"anybody" or to the first row in the table. A hub with no personal owner is a
hub whose session reads all refuse, which is loud, recoverable and safe; a
fallback is quiet and is the whole privacy guarantee gone.

**Where "is this caller the personal owner?" is answered — revision 6.** Once,
when the token is resolved, and it then travels on `Caller` as a boolean.
`guard::access_allows` must stay **store-free**: it is shared with the
store-free `mcp/tools/present.rs::visible_to`, so a store read in the gate
would have to be plumbed into the presenter too, or the two would disagree
about which tools exist. §2.7's `Access::Person` binding reads that boolean and
does no lookup of its own.

Further devices are paired **onto** an existing person:
`fleet-hub pair --name phone --person martin`.

**One profile, several hubs, several devices.** Per-hub rows are an
implementation detail the user must never feel. Revision 2 said "the profile
belongs to the device", which answers *joining a company* and not *adding a
phone* — the other half of the requirement. Three layers, named, because they
have different homes and different carriers:

| Layer | Examples | Where it lives | Travels to a new device? |
|---|---|---|---|
| **Personal profile** | display name, avatar, how you are addressed | the person, wherever their own space lives | **yes, must** |
| **Portable preferences** | default host, default project, default AI account, effort, editor, notification choices | the person, per space (§4.6) | **yes, must** |
| **Local settings** | window geometry, theme, terminal font, this device's own token | the device | no, by definition |

Two requirements follow, binding on M3:

- **Adding a second device must not mean setting up anything twice.** The first
  two layers are handed to the new device *at pairing time*. The natural carrier
  is device-to-device: the existing device is the source, the pairing code the
  channel. Where a person runs their own hub, that hub is the obvious home; where
  they do not, the profile has no hub to live in and the handover is the only
  path — which is why "pair a device from another device" belongs in M3's scope,
  not a later nice-to-have.
- **The company hub is never the carrier.** It stores what it needs about a
  member and nothing about your personal space. A profile that travelled through
  a company hub would make that hub aware of your personal setup, which is the
  exact thing the brief rules out.
- The space switcher presents *one* "you" with several spaces attached, never a
  list of unrelated logins.

**Why not an account service.** Nothing above needs one, and the handover asks
for no registration. Cross-hub identity becomes a *mapping* when org linking
lands (§4.7), not a global account.

### 4.2 Space = a view, not an entity

Do not add a `spaces` table. A space is `(hub, person, org?)` — what the UI
shows. Personal space = the person's own sessions on their own hub; a work space
= the person's membership in one org on a company hub. Every backing concept
already exists or is added by 4.1/4.3.

### 4.3 Ownership + visibility + explicit grants — three small things, not an ACL engine

**This section is the single authority for two things the rest of the document
and the companion plan cite rather than restate: the values `visibility` may
take, and the membership of the `own` tier.** Revision 4 shipped three copies
of the `own` list and they disagreed; a reader who finds a fourth copy anywhere
should treat it as a bug in that copy.

```
sessions.owner_person_id   INTEGER          -- reserved before tmux starts, written by the one upsert
sessions.visibility        TEXT NOT NULL DEFAULT 'unclaimed'
                           CHECK (visibility IN ('private','unclaimed'))
session_grants (session_id, person_id, org_id, level 'watch'|'drive', granted_by, granted_at, revoked_at)
  CHECK ((person_id IS NULL) <> (org_id IS NULL))
  UNIQUE INDEX ON (session_id, COALESCE(person_id,0), COALESCE(org_id,0)) WHERE revoked_at IS NULL
  INDEX ON (person_id) WHERE revoked_at IS NULL AND person_id IS NOT NULL
```

#### `visibility` has exactly two values in M1 — owner's delegate, revision 5

`private` and `unclaimed`. `private` is the default for anything a person
starts — the handover's requirement. `unclaimed` is the safe holding state,
defined below. The `CHECK` admits those two and nothing else, so a third value
cannot appear by accident, through a hand-edited database, or through a future
`UPDATE` that nobody reviewed against this section.

**`'org'` is removed from M1 completely** — not a settable value, not a
migration target, not a UI control, not a schema value. Revision 4 kept it in
the DDL, surfaced it as an access answer on the desktop and let the new-session
dialog set it, while no rule anywhere said who could read one. The
reason it is gone, recorded so it is not re-added by someone who reads only the
DDL:

> `'org'` is team sharing under another name. M1 has **no membership table**,
> so "the org can see it" has exactly one referent in the schema —
> `client_tokens.org_id` — which is written by
> `work_admin { action: "assign_client" }` under `Access::Master`. An admin
> binds their own device to the org and reads every `'org'` session, with no
> grant touched and no owner consent. That is the precise hole the owner closed
> by removing `session_share { org }` from M1, reached by the back door of a
> column instead of a grant row.

`'org'` returns in M2, with memberships, a defined reader, and a rule that a
membership change never widens an existing grant. Until then, every
creation-time "share with my team" control is out of M1's frontend scope, and
Q10's upgrade assertion that no row carries `'org'` is satisfied by the `CHECK`
rather than by a `SELECT COUNT(*)`.

Three further schema notes, each because the obvious form enforces nothing:

- `visibility` must be `NOT NULL` with a default, and it, not
  `owner_person_id`, is what the event fence keys on — a nullable integer is
  *absent* on the wire, not null (§3.7). On the wire it needs
  `#[serde(default = "…")]` returning `"unclaimed"`; `#[serde(default)]` alone
  yields `""` and fails **open** into an arm nobody wrote (§3.7).
- The unique index must `COALESCE`. `UNIQUE (session_id, person_id, org_id)
  WHERE revoked_at IS NULL` enforces nothing at all: exactly one of the two
  recipient columns is NULL by design, and SQLite treats NULLs as distinct, so
  two live grants to the same person on the same session both insert. The
  repo's own precedent is `ux_work_views_name` on
  `work_views(COALESCE(owner_org, 0), name)`
  (`migrations/066_work_view.sql`).
- `grants_for_person` is the hot read — it runs once per request to build a
  scope — and an index led by `session_id` does not serve it.

#### The ownership predicate, written once

Revision 3 stated the rule in prose and it reads like an identity check. Written
the way prose suggests, it is a hole:

```rust
row.owner_person_id == scope.person        // WRONG: Option == Option
```

Both sides are `Option<i64>`. An `unclaimed` row has `None`; a caller with no
person has `None`; `None == None` is `true`, so **every person-less caller is
the owner of every unclaimed row** — and therefore, by invariant 1 below, may
create grants on all of them. A reviewer does not see it, because the line looks
like the thing it is supposed to be.

The rule is one inherent method and nothing else calls the comparison directly:

```rust
impl ViewScope {
    fn owns(&self, row: &SessionRow) -> bool {
        matches!((row.owner_person_id, self.person), (Some(o), Some(p)) if o == p)
    }
}
```

The visibility table, the reach check, `session_share`, `session_unshare` and
`session_narrow` all call `owns`. The first row of its table test is
`(owner: None, person: None) => no access`, pinned by name.

**Where `owns` lives, and where it does not — revision 6.** It is an inherent
method on `ViewScope`, which is a `service/` type, and the store does not call
it. `store/session_grants.rs` enforces "only the owner creates a grant" with a
plain column comparison against `sessions.owner_person_id` inside its own SQL
— `store/` does not import `service/`, the dependency runs the other way, and
`ViewScope::owns` is a `service/`-side predicate over a row the store handed
it. Writing the store's check as a call to `owns` inverts the crate's layering
and makes the grants work wait on a type a later task creates; the two checks
are the same rule expressed at the two layers that each need it, which is the
normal shape here and not a duplication to consolidate.

#### What counts as content

A session's metadata *is* content. Revision 3 said so and then listed six
fields, against a `SessionRow` that carries many times that (§1). The list must
at minimum also carry `pr_url`, `worktree_key`, `account_uuid`,
`current_activity`, `claude_session_id` and `parent_session_id` — and the point
of the table below is the *reasoning*, not the enumeration, because the
enumeration is what goes stale:

| Field | Why it is content |
|---|---|
| `tmux_name` | a branch or a ticket key |
| `friendly_name`, `notes`, `last_prompt`, `current_activity` | sentences a person wrote or a Claude produced |
| `tags`, `worktree_key`, `pr_url` | what the work is and where it lives |
| `claude_session_id`, `parent_session_id` | the conversation's identity, and the input to a resume takeover (§5.2) |
| `account_uuid` | which AI account, therefore which employer |

The safe way to express this is a **positive** list of what an out-of-scope
caller may learn — which, for `unclaimed`, is a count, and for a session private
to someone else, is nothing at all — not a redaction list that has to be kept in
step with a growing struct.

#### What a grant may and may not do

Seven invariants. They are short because each one closes a way round the privacy
rule, and they belong in the type, not in a reviewer's memory:

1. **Only the owner creates a grant.** Nobody else, at any level. "Owner" means
   `ViewScope::owns` above, never an `Option` comparison.
2. **A grantee cannot grant on.** Sharing is not transitive.
3. **A grant only ever moves downward.** It can be revoked, or narrowed
   (`drive` → `watch`). It can never be widened, have a recipient added, or be
   redirected to a different person. "Re-home the grant to me" is a privacy
   bypass wearing a grant's clothes. In M1 the only caller who may revoke or
   narrow is the **owner**; an administrative authority over a departed
   member's grants needs memberships and therefore arrives in M2 (Q9).
4. **A grant never confers a terminal** (§2.4) — nor the attach command, nor
   the drop handler. All three are the desktop's own SSH and none of them is
   revocable by the hub. This is also why M1 adds a read-only pane snapshot for
   a watcher (§2.4, correction 4): the rule takes the live view away, so
   something has to give it back in a revocable form.
5. **A grant never confers `own`. Sharing has exactly two grantable levels,
   `watch` and `drive`; `own` is a tier, not a third level anyone can be
   given.** It is the set of operations only the owner may perform, and no
   grant ever reaches it. **This list is the definition; every other mention in
   this document, in Q6 and in the companion plan cites it and does not restate
   it.**

   The membership is *everything that copies, relocates, re-creates, destroys,
   renames, re-tags or re-shares a session*:

   | Operation | Why it is `own` |
   |---|---|
   | `kill_session`, `safe_kill_session` | destroys the owner's work |
   | `restart_session`, `recreate_session` | re-creates it; `recreate_session` is also the primitive `restore_host_sessions` batches over, so gating one and not the other gates nothing |
   | `move_session` | relocates the working tree, Claude directory and project memory onto a host the caller names, where they are the unix owner |
   | `spawn_review` | creates a session in the owner's worktree on the owner's host, with a terminal |
   | `rewind_conversation` — fork, rewind and retry alike | a permanent verbatim copy of the transcript |
   | `restore_host_sessions` | restarts the owner's sessions, spending the owner's AI account |
   | `rename_session`, `set_session_tags` | re-labels the owner's row; the label is content (see below) |
   | `work_link { summarize }` | stores a durable précis of the transcript that outlives the grant |
   | `delete_worktree` | removes the checkout a session is RUNNING IN, leaving the owner's pane in a deleted directory and dropping fleet's row — which `force: true` does even while the session is alive. Strictly more than `safe_kill_session`. Added in fix round 3: it reaches its sessions by `worktree_id`, so nothing about the shape of its parameters said it was session-addressed at all, and it appeared in neither tier table while `src/lib/share.ts` already carried it at `drive` — which must move to `own` |
   | `session_share` / `session_unshare` / `session_narrow` | this is where "a grantee cannot grant on" is enforced |
   | any future tool that creates a durable copy of a session's content, relocates it, or changes who may reach it | a positive tier fails closed for the next one; a deny-list does not |

   **`set_friendly_name` is `drive`, not `own`, and the two halves of that are
   not in conflict** (settled in fix round 3, where a review found the desktop's
   justification contradicting this document). READING `friendly_name` is
   content — it stays in the table above, so a stranger never sees it — but
   WRITING it is not an `own`-tier act, for a reason the two rows above do not
   share. `rename_session` changes the session's tmux NAME, which is its
   address: every `(host, tmux_name)` reference, every bookmark and the pane the
   owner attaches to move with it. `set_session_tags` writes a durable
   classification the work graph and the sidebar filters read. The friendly
   label is the sidebar CAPTION, and fleet writes it itself without asking
   anybody — `service/sessions::label_from_prompt` on a background agent's first
   prompt, `fill_session_name` and `tickets::start_one` on a start. A field the
   hub rewrites on its own is not a field the owner-only tier is protecting, and
   a driver correcting the caption of the session it is driving changes nothing
   the owner cannot see and re-set. (`src/lib/share.ts` carries the same tier
   with a different and WRONG reason — "not the tmux name the `own` tier
   protects", which reads as if `friendly_name` were not content at all. That
   sentence is to be replaced with this paragraph's reasoning.)

   Two things this table settles that revision 4's three copies disagreed on.
   A `drive` grantee may **not** kill, restart or rename the owner's session:
   `drive` is "make this machine do work", not "dispose of it".
   `send_message { deliver, submit }` is a **pane write** and sits at `drive`,
   not at `own` — it is exactly what `send_prompt` is, by another route, and
   refusing it to a driver while allowing `send_prompt` would be theatre. What
   it must *not* do is reach a caller with only `watch`: revision 3's deny-list
   let it, which is a watch grant silently conferring drive.
6. **A session created *from* another session inherits the source's owner and
   visibility, never the caller's.** `spawn_review` and `rewind_conversation`
   both build a new session out of an existing one's `cwd`, worktree or
   transcript. Revision 3's "a fork inherits the FORKER's ownership" is both a
   hole — the forker would own a verbatim copy of a transcript revocation
   cannot reach — and unimplementable: `service/rewind.rs::RewindArgs` carries
   `session_id`, `anchor_uuid`, `mode` and `new_worktree` and **no forker
   identity**, so whoever implements it falls back to source-inheritance
   anyway. Source-inheritance is the rule, and a fork of a session you do not
   own is refused by invariant 5.
7. **A token that names no person is a refusing scope** (§3.3). Not a
   privileged one. Internal readers do not go through a token.

A revoked grant row stays for the audit trail (the `client_tokens.revoked_at`
convention). Revoking a grant must **not** ride `auth_epoch`: that is the device
mechanism, and the two are different events (Q2).

#### Team sharing is out of M1 — a scope reduction the owner took knowingly

Revision 3's `session_share { session_id, person | org, level }` shipped an org
recipient. **The owner removed it from M1 on 2026-09-30, deferred to M2**, on
this reasoning:

An org grant names a recipient *set* whose membership somebody else writes. In
M1 the only person↔org relation that exists is `client_tokens.org_id`, and it is
written by `work_admin { action: "assign_client" }` (`service/orgs.rs`), whose
`TOOL_POLICIES` row is `Access::Master`. So: A shares session S with org
`platform`, watch. The admin runs `fleet-hub client bind <their own device>
--org platform`, and that device now satisfies "a grant to an org the person is
in" and reads S in the application. No owner consent, no new grant row,
invariant 3 untouched — the recipient set simply grew. That is the admin
override Q11 refuses, reached through the application's own gate rather than
through the conceded `state.db` access of §4.5.

It is also undefined as written: M1 has **no membership table**, so "an org the
person is in" has no referent; and the only candidate, `client_tokens.org_id`,
is a property of a *device*, which would make a person's visibility depend on
which device they picked up — and would strip that device of `Access::Person`,
which requires `org_id.is_none()`
(`mcp/auth.rs::Caller::is_person_device`).

**M1 therefore ships person-to-person grants only.** The `org_id` column stays
in `session_grants` with its `CHECK`, and the store **refuses** an org
recipient, so M2 adds memberships rather than a column. When org grants return,
the rule must be that changing a membership never widens an existing grant: a
person added to an org after the grant was made gets nothing until the owner
re-grants.

**The same reasoning removed `visibility = 'org'`** (above). The two are one
decision taken twice: a grant to a set and a visibility to a set are the same
capability, and neither has a referent in M1 that the admin cannot write.

#### A grant is on a row, so it does not travel — the owner's decision on `move_session`

`move_session` creates a **new row with a new id** on the target host; the old
row is retired. `session_grants` are keyed on the old `sessions.id`.

**Owner's decision (2026-09-30): the grants are dropped, not carried.** They are
revoked as part of the move, and the owner re-grants if they still want to.
Narrowing is the safe direction, and a grant is a statement about a row, not a
subscription to a person's work.

Two things follow that the plan must build, not assume:

- Moving is an **`own`** operation (invariant 5). A grant is permission to use a
  session where it is, never to relocate it onto a machine the grantee controls
  — where they are the unix owner of its carried working tree, Claude directory
  and project memory, outside anything Fleet can revoke.
- The owner/visibility carry to the target row must be a **hard failure inside
  the transaction that creates it**. Today every write in that block is
  soft-fail by design, each commented "the session is live either way"
  (`service/move_session/mod.rs`, the target-row block). A soft-failed carry
  leaves the target at the schema default — `unclaimed`, on the target host —
  where an agent that can prove that pane claims it. A move must never be able
  to become a silent privacy event.

#### `'unclaimed'`: the safe holding state — one answer, not three

A row whose owner cannot be established must fall into neither "everyone" nor
"nobody".

| Situation | Owner | Visibility |
|---|---|---|
| Started through fleet by a person | that person | `private` |
| Existing rows at upgrade that fleet created — `started_at IS NOT NULL` — on a hub with one person | that person | `private` |
| Existing rows at upgrade that fleet did not create — `started_at IS NULL` | NULL | `unclaimed` |
| Discovered by reconcile from a hand-started tmux session | NULL | `unclaimed` |

**The upgrade's discriminator is `sessions.started_at`, and it is the same
discriminator in both rows above.** The column is documented "when fleet created
the session (NULL for tmux-discovered rows)" (`store/rows.rs::SessionRow`), so
it separates precisely the two populations: a row fleet started, which the
hub's one person demonstrably owns, from a row reconcile found, which nobody
can speak for. That is the rule; Q10's acceptance criterion is written against
it and against nothing else. Revision 4 had Q10 demand that **every** session
carry the person while this table and the plan's backfill attributed only the
first kind — the assertion was the half that was wrong, and it was the half
written as a required test.

Revisions 2 and 3 left three incompatible readings of what `unclaimed` then
means — "listed", "row only, content refused, claimable in one click", and "a
count, never a row". **This section is the single authority, and the answer is
count-only**, for a reason that is not a preference: on `/events` there is no
middle option. A `session:created` / `session:updated` frame *is* the row and
*is* its content, so an unclaimed frame can only be dropped. A "row without
content" has no expressible meaning at that choke point, and a rule that cannot
be expressed at every one of §5.1's choke points is not a rule.

- **What is served.** A per-host count. "3 unclaimed sessions on `box-2`" —
  enough that a host is not a black hole and nothing silently vanishes on
  upgrade, and not one byte about what they are. It needs a wire carrier, which
  neither document had: `HostRow.unclaimed_sessions`, beside `org_id?` /
  `health_at?` / `provision_stale?` (`src/lib/hosts.ts`). It cannot ride
  `sessionCounts` (`src/lib/hosts_view.ts`), which derives every host badge
  from rows the client holds — and the whole point is that these rows are never
  sent.
- **Who sees the count — owner's delegate, revision 5.** One person, or nobody.

  | Hub | Who is served the count |
  |---|---|
  | Exactly one person on the hub | that person |
  | More than one person on the hub | **nobody, through the API in M1.** The operator reads it with `fleet-hub` |

  Revision 4 wrote "whoever may already see that host … the operator on a
  shared one", which is not implementable in M1: there is no
  host-administration concept until M2, so an implementer had to invent one or
  ignore the sentence. The two-person answer is deliberately *no API surface at
  all* rather than a widened one. Two things follow that the plan must not
  soften. The master token is **not** resolved to the hub's personal owner in
  order to serve this — that would turn the count into a fleet-wide read for
  whoever holds the master token. And "the operator" here means a human with
  shell on the hub running a subcommand, never a caller (§3.3): writing "the
  operator" where the code would mean "the master token" is how the `'org'`
  class of hole gets in. When M2 defines who administers a host, the count
  follows that definition.
- **Who may claim, and what the claim proves.** Claiming needs proof of access
  to the **pane**, not to the machine and not to an organisation. Revision 3
  said "the host token is precisely a proof of *I am on that machine*", which is
  true and insufficient: one token per host means any agent on a shared host
  could claim every unclaimed row on it, to any person it names. The proof that
  does discriminate already exists in the schema: `sessions.tmux_pane_id`
  (`migrations/037_conversations.sql`), read by
  `store/sessions.rs::Store::find_session_by_pane` — `LIMIT 2` over
  `host_alias = ?1 AND tmux_pane_id = ?2 AND status != 'ghost'`, answering
  `None` for anything but exactly one match. So:
  - the agent in the pane calls with its **host token**, whose connection
    carries `X-Fleet-Pane` (§4.4), and the claim resolves through
    `find_session_by_pane` on that host against the pane on `Caller` — not
    against an argument the caller chose. A mismatch, or an ambiguous pane,
    refuses, with the reason named (see below);
  - the operator, who has shell on the hub anyway:
    `fleet-hub session claim <id> --person <name>`. This is a new top-level
    subcommand — there is no `Cmd::Session` in `crates/fleet-hub/src/main.rs`
    today — and it must pick one of the crate's three existing write paths
    deliberately (MCP tool against the running hub, `work_admin`, or a direct
    `open_store`), because `pair.rs`'s own module doc forbids opening a second
    write path into the store.

  A UI "claim" button for an arbitrary org member is exactly what must not
  exist, and neither does a claim that names a beneficiary the caller cannot
  demonstrate.
- **The carve-out that makes claiming reachable at all.** Count-only and
  §4.4's backstop together would leave the agent unable to address its own row:
  it reaches itself through `whoami` →
  `service/sessions/targeting.rs::find_session_by_tmux_name_scoped`, which
  needs the **row**. So the rule is stated positively, in §4.4: a host token
  sees, on its own host, an `unclaimed` row, and the row whose pane it can
  prove it is in. Nothing else.
- Claiming sets the caller's named person as owner, the row to `private`, and
  writes a `session_events` row. Never bulk, never implicit.

**What `tmux_pane_id` is actually worth — corrected in revision 5, and this one
moves a design rather than a citation.** Revision 4 said the column is "written
by the hook path (`service/hooks.rs:2994`)". That line is inside a
`#[cfg(test)]` helper. In production the column is written by the **reconcile**
pass: `service/sessions/reconcile.rs` fills `tmux_pane_id` from the tmux
probe's `sess.pane_id`, and `store/reconcile.rs`'s upsert carries it in the
INSERT list and, on conflict, as
`tmux_pane_id=COALESCE(excluded.tmux_pane_id, tmux_pane_id)`. Migration 037's
own header says exactly this: "the pane id (%17) **reconcile** last saw; hooks
carry `$TMUX_PANE` in `X-Fleet-Pane` and **resolve by it**."

Three consequences, and the design is better for two of them:

- **The claim path works for precisely the rows it exists for.** Under revision
  4's sentence a hand-started tmux session — which by definition never runs a
  fleet hook — would carry no pane id, and the claim would be unreachable for
  the whole `unclaimed` population. It is not: a row is `unclaimed` *because
  reconcile discovered it*, and the same pass that inserted it wrote its pane
  id in the same statement.
- **What the column holds is the session's *active* pane as of the last pass**,
  not "the pane this agent is in". The probe asks tmux for `#{pane_id}` per
  **session** (`tmux.rs`'s `SESSIONS_FORMAT`), which resolves against the
  session's current window's active pane. For a fleet-started session — one
  window, one pane, the agent in it — that is the agent's pane and the proof is
  exact. For a hand-started session with several panes, an agent in a
  non-active pane presents a pane the row does not carry,
  `find_session_by_pane` returns `None`, and the claim **refuses**. That is the
  right direction — an unproved claim is not granted — but it is a real failure
  mode, not a theoretical one, and revision 6 widens its blast radius: once the
  pane travels on the connection (§4.4), the same refusal is what an agent in a
  non-active pane meets on *every* session-addressed tool, not only on a claim.
  So it needs a **distinct, named refusal** — "this connection's pane is not
  the active pane of any session on this host" — and never a generic
  not-found. A not-found sends the agent looking for a missing row; the truth
  is that the row exists and the caller is standing in the wrong pane of it.
- **The pane id can be stale, and it never goes NULL.** `COALESCE` on conflict
  means a pass that saw no pane id leaves the previous value standing, and the
  probe's `pane_id` is an `Option` (a tmux that does not report it yields
  `None`). So a stale id can outlive a tmux server restart. That is exactly the
  case `find_session_by_pane`'s `LIMIT 2` → `None` behaviour was written for,
  and its doc comment says so. The claim path inherits that protection and must
  not weaken it to "take the first match".

So the pane proof is **proof of being in the session's active pane on that
host, as of the last reconcile pass** — narrower than "I am this agent", and
strictly stronger than the host token alone. It is enough for a claim, which is
a one-time, recorded, single-row act with a named beneficiary, and it is enough
for §4.4's clause 2, which reaches exactly the one row the pane resolves to.
It is **not** a general-purpose identity, and M1 may not treat it as one: it
never widens to a second row, it is never accepted as an argument the caller
chose, and where it does not resolve the answer is a refusal (above), never a
fallback to the host token's older, wider reach.

On the normal upgrade path none of this is reached: the hub had one person, so
every row that can be attributed is attributed to them (Q10) and `unclaimed`
stays rare — hand-started tmux sessions on shared hosts, and the
reconcile-discovered rows the backfill cannot speak for.

### 4.4 The per-host token on a shared host: unix accounts first, a **pane** proof second

The honest options for "two people, one host":

| Option | Cost | Verdict |
|---|---|---|
| Each person gets their own unix account on the shared host → a separate fleet host alias (`box-martin`, `box-jane`) with its own token and its own `~/.claude` | zero code | **recommended deployment rule** |
| Mint a token per (host, person) | `host_tokens` PK change, provisioning change, and the agent still cannot prove which session it is | no |
| Keep one token, narrow `OrgScope::Host`'s `sees_session` so a row private to someone else is not visible | small | **withdrawn in revision 4 — it does not work** |
| Keep one token; a host token sees only `unclaimed` rows on its own host plus the row whose pane it proves | small, and it is the only formulation that holds | **do this, in M1** |

**Why revision 3's third row is withdrawn.** Two independent reasons, each
sufficient.

*It is specified against a function the gate never reaches.* Revision 3 said the
fix is that `OrgScope::Host`'s unconditional `row_host == alias`
(`service/orgs.rs::OrgScope::sees_session`) becomes "…and the row is not private
to someone else". But `mcp/tools/support.rs::resolve_row_and_gate` — the gate
for `capture_session`, `session_transcript`, `session_conversation`,
`session_tool_detail`, `session_activity`, `send_prompt`, `kill_session`,
`restart_session`, `recreate_session`, `inbox`, `wait_for_reply`,
`dispatch_task`'s worker and every other tool that resolves a session by id —
is three lines: `resolve_session_target`, `require_host`,
`require_bound_client_sees`. The last of those opens with
`if caller.client.as_ref().is_none_or(|c| c.org_id.is_none()) { return Ok(()) }`,
and a per-host token has `client: None`. So `sees_session` is **never called on
that path for a host token**, and narrowing it changes nothing for the entire
single-session surface.

(`sees_session`'s `Host` arm has **three** unconditional wins, not the one
revision 3 knew about and not the two revision 4 enumerated:
`row_host == alias || row_org.is_none() || row_org == *org`. The third is the
widest — a host token in org X reads every session of org X anywhere in the
fleet — and the fall-through beneath them is permissive unless one side
isolates. §1, consequence 3, has the full predicate. This matters here only to
say that narrowing "the two clauses" would not even have closed the function it
was aimed at.)

*And the rule as phrased is unsatisfiable.* "Not private to someone else" needs
to know *which* session the caller is. Every Claude on a host authenticates with
the same token (`service/provision.rs`, the `mcpServers.claude-fleet` entry it
writes into `~/.claude.json`), so the agent inside A's private session is
indistinguishable, at the token, from the agent in the next pane. Either the
backstop refuses every agent its own row — breaking `whoami`, `register_self`,
`send_message`, `work_link`, `dispatch_task`, `quick_replies` and
`session_activity` for every fleet-started (therefore private) session, i.e.
the agent-facing half of the product — or it refuses nothing. There is no third
answer *at the token*.

**The formulation that does hold.** A host token sees, and may address:

1. any row on its own host whose `visibility` is `unclaimed` (this is what makes
   the claim path in §4.3 reachable); and
2. the one row whose pane it can prove it is in — the pane on `Caller`, carried
   by the connection's `X-Fleet-Pane` header (below), matched against
   `sessions.tmux_pane_id` through
   `store/sessions.rs::Store::find_session_by_pane`, which already exists and
   already returns `None` on an ambiguous match. §4.3 says exactly what that
   proof is worth and where it is narrower than it sounds. Because the proof is
   a property of the connection rather than of a call, clause 2 is evaluable on
   **every** tool the in-pane agent reaches, not only on the three that could
   take a pane argument.

Nothing else. In particular, not another person's private session on the same
host.

Mechanically this is **not** an edit to `sees_session`. It is a sibling check,
`require_person_sees(s, caller, row, reach, what)`, called **unconditionally**
from `resolve_row_and_gate` alongside — not inside — `require_bound_client_sees`,
carrying the reach (`watch` / `drive`, or the owner's own) the caller needs in
the same call. `OrgScope::sees_session` and `OrgScope::sees_row` should then be
*removed* from `OrgScope`, so that no site can keep compiling against the
org-only answer (§5.1).

**The pane proof travels on the CONNECTION — revision 6, and the premise is now
verified.** The agent-facing rule above only works if clause 2 is evaluable on
*every* session-addressed tool the in-pane agent calls — `dispatch_task`,
`send_message`, `session_activity`, `work_link` and the rest — and revision 5
could not promise that, because the pane only reached the hub as an *argument*
and the leak-fix rule allows a pane argument on three tools at most (`whoami`,
`register_self` and the claim; no other tool may take one, because a pane
argument a caller chooses is a pane argument a caller can lie about).

The transport exists in one half of the product already. Hooks carry
`$TMUX_PANE` in an `X-Fleet-Pane` header — `service/hooks_install.rs` writes it
into the host's `settings.json` hook entries and `mcp::hooks::pane_header`
validates it as `%` + digits, answering `None` for an empty value or for an
unexpanded literal — while the **MCP** entry in `~/.claude.json` carries only
`Authorization` (`service/provision.rs`, the `headers` object it writes).

**Revision 5 left "does the agent runtime expand an environment variable in an
MCP header?" as an unverified premise. It has been checked against the Claude
Code documentation and it holds.** Three facts, each load-bearing:

- Claude Code expands `${VAR}` and `${VAR:-default}` inside an MCP server
  entry's `headers` **and** its `url`;
- **no allow-list key is required.** `allowedEnvVars` is a **hooks-only**
  mechanism; it has no counterpart for MCP and none is needed;
- the rule that blanks a variable rather than expanding it applies to
  **credential** variables — `ANTHROPIC_API_KEY` and its siblings.
  `TMUX_PANE` is not one of them.

So the fix is one more entry on the object `service/provision.rs` already
writes:

```json
"headers": { "Authorization": "Bearer …", "X-Fleet-Pane": "${TMUX_PANE:-}" }
```

**Use the braced form. Do not copy the hooks entry's syntax.** The hook block
writes a bare `"$TMUX_PANE"` and that works *only* because hooks have
`allowedEnvVars` (`service/hooks_install.rs` writes both together, and its own
comment records that the pre-`allowedEnvVars` shape did not expand). Pasted
into the MCP entry, a bare `$TMUX_PANE` arrives as the literal string — which
`pane_header`'s validator correctly rejects, so the symptom is not a leak but
every in-pane agent silently losing its own row.

`mcp/mod.rs::authorize` — which reads `Authorization` and nothing else today —
reads the new header, validates it exactly as `mcp::hooks::pane_header` does
(`%` + digits; an unexpanded literal or an empty value is `None`), and puts the
result on `Caller` as `pane: Option<String>`. Three properties follow, and they
are the reason this is better than an argument: the agent cannot forge it any
more than it can forge the bearer token; it is fixed for the life of the MCP
connection, which is exactly the lifetime it describes; and **clause 2 becomes
a property of the connection, evaluable on every tool**, rather than a
parameter three tools happen to accept.

**A durable `(host_alias, pane_id) → session_id` record is rejected, and it was
a mistake to propose one.** Revision 5's companion plan reached for one so that
a scope could be built without a per-call pane. It is keyed by *host alias* —
so every pane it records becomes resolvable by every agent holding that host's
one token, which is the exact guarantee §4.4 exists to create. It inverts it.
With the header there is nothing left for it to do, and it is deleted rather
than narrowed: a table that must never be read by the caller that can read it
is not a smaller version of a good idea.

**What this does not settle.** Separate unix accounts are a way to keep two
people apart; they are *not* the answer to "one person, two AI accounts" — that
is Q7, and it has its own mechanism (a per-session config dir). Revision 1 used
one answer for both questions, which is what narrowed the requirement.

**The truth to write in `docs/`:** on a host where two people share a unix
account, Fleet-level privacy is cosmetic. The person with the account can read
the transcripts on disk. Fleet must never claim otherwise.

### 4.5 Privacy vs. the machine operator — say it once, in the docs

Two different statements, and the difference is the whole point.

**In the application, privacy is absolute — including against the company
admin.** This is the owner's explicit decision and this document does not carve
an exception out of it. There is no admin override, no break-glass, no "audited
access" path in v1 (Q11). An org admin is an admin *of the organisation's
settings and membership*, not of its members' sessions.

**But "admin" names two different people, and only one of them is fenced.**
An adversarial review of the implemented gate (2026-10-01) found that
`pair_client` is `Access::Master` and takes a `person` argument — so whoever
holds the master token can mint a device bound to any person, and then read that
person's sessions as them. That is not a defect to fix: it follows from the
operator holding `state.db`, which the paragraph below already puts out of
scope, and closing it in the application would be theatre. What it does mean is
that the rule has to be said more precisely than "privacy holds against the
admin":

| Role | Fenced by M1? |
|---|---|
| **Org admin** — authority over the organisation's settings and membership | **Yes.** No path to a member's session content. This is what rule 2 is about and it holds. |
| **Hub operator** — holds the master token and the database | **No, and by design.** They can pair as anyone, read `state.db`, and reach the hosts. §4.5's second half is the whole of the answer. |

**The consequence to state plainly, because the owner was careful here:** in a
deployment where the company's admin *is* the hub operator — which is the normal
shape for a small company — rule 2 is weaker than it reads. Privacy then holds
against colleagues and against anyone whose authority comes only through the
application, and not against the person who runs the machine. `docs/hub.md` must
say that in those words, beside the upgrade order, rather than leaving a reader
to infer it from a sentence about databases.

**Outside the application, Fleet promises nothing and must not pretend to.**
`docs/hub.md` should carry one short paragraph: the hub's operator can read the
hub's database, a host's unix owner can read that host's transcripts, and the
terminal attaches over SSH outside the hub entirely. Fleet's privacy is about
what the application shows, not about what the machine's owner can do.

The honest framing is that these are two separate protections with two separate
owners: Fleet protects the application boundary; the operator's machine policy
(who has which unix account, who has SSH where) protects the rest. Saying so
plainly is more useful than an admin bypass that blurs both.

### 4.6 The space switcher: re-resolve, don't multiplex

The desktop resolves its mode once at startup. Keep that. A switch = tear down
and re-resolve against the other hub (or local), behind an experimental setting,
with the per-space last-used choices (host, account, project) remembered in
`settings`. Several spaces live at once in one window is a much larger change —
the store, the event stream, the reconcile tick and the single global PTY are all
process-wide — and nothing in the handover asks for it.

### 4.7 Org sync: not before M1, and not as "sync the database"

When it comes: a named, versioned, per-entity-kind payload (org definition, org
rules, tracker *configuration without secrets*, status maps), carried over the
existing peer link, applied as **proposals** evaluated under current permissions
(§2.7), with `version` + `E_CONFLICT` for concurrent edits (the M14.1a pattern).
Never: sessions, work items, journals, secrets, transcripts.

---

## 5. What multi-user actually costs, mechanically

Revision 3 said: one type and six choke points. That framing is withdrawn. The
funnel is real, but for the caller M1 introduces most of it is switched off, and
three of the places that matter are not in the list. §5.1 is the corrected
inventory, §5.2 the paths that go round it, §5.3 the desktop and the
regeneration loop.

### 5.1 The choke points — and four of the six do nothing today

| # | Surface | Anchor | State today, for a person's device |
|---|---|---|---|
| 1 | The scope type itself | `service/orgs.rs::OrgScope` | no person dimension |
| 2 | Where a caller becomes a scope | `mcp/auth.rs::Caller::org_scope` | returns `OrgScope::All` for any client with no org |
| 3 | List filtering | the `scope.sees_row` filter in `mcp/tools/session_ops.rs::list_sessions` | **runs**, and passes everything |
| 4 | Single-session addressing | `mcp/tools/support.rs::resolve_row_and_gate` (the wrapper is `resolve_and_gate`; both paths meet in the former) | `require_bound_client_sees` returns `Ok(())` in its first statement |
| 5 | The second, weaker session gate | `support.rs::require_visible_session` | opens `if !caller.is_scoped() { return Ok(()) }`. Sole gate on `session_history` and on every `repo_*` read |
| 6 | The result gate | `support.rs::redact_work_via`, called from `mcp/tools/mod.rs` | never called; and it deletes `WORK_FIELDS` keys, it cannot drop a row |
| 7 | The live stream fence | `mcp/events_route.rs::fence_frame` | returns the payload verbatim on `scope.is_all()` |
| 8 | The live stream's re-scope machinery | `events_route.rs`'s `rescope`, and its two consumers (the keep-alive beat and the pre-frame check) | `rescope` is `caller.is_scoped().then(..)` → `None`; neither consumer fires |
| 9 | The `fresh_for` reader | `support.rs::resolve_reader` | consults the scope only `if caller.is_scoped()`; an existence oracle, and it writes a read cursor into a session the caller does not own |

Reading that table is the whole point of this revision. **M1 is not "widen a
predicate"; it is "turn the machinery on for a new class of caller."** Rows 4–8
must first be made to *run*, row 6 must learn a capability it does not have
(dropping a whole object from an answer, not deleting known keys), and row 8's
existing behaviour is not what revision 3 assumed: a generation move **ends the
stream** — each of the two consumers answers with a bare `return None` when the
scope it re-reads differs from the one the stream was built with — it does not
re-scope. So every grant and every revoke drops the affected person's SSE
connection. That is the right fail-closed behaviour and a different
user-visible fact.

**Do not add the person field to `OrgScope` in place.** Wrap it:
`ViewScope { org: OrgScope, person: Option<i64>, grants: GrantSet }`. The
argument is §3.6: extending in place leaves every `OrgScope::All` literal and
every `is_all()` site compiling, a meaningful share of which guard session
reads, and the compiler flags none of them. Wrapping is a large but
**compiler-forced** edit — *and only compiler-forced if `OrgScope::sees_row`
and `OrgScope::sees_session` are removed or renamed rather than left working.*
Leaving them compiling converts a mechanical migration into an audit, which is
the exact failure mode the `is_all()` sites represent. `sees_org`, `sees_link`,
`redact_json` and the work graph's own fences stay untouched.

Two further properties the type must carry, both discovered in the stream.
`ViewScope` must be `PartialEq` over a **canonically ordered** grant set
(`BTreeSet`/`BTreeMap`), or the `read_scope(..) != Some(&st.scope)` comparisons
in the stream's two consumers drop streams on every rebuild. And there must be
exactly one constructor (`Caller::view_scope`), pinned by a test, because
`usage_report` today builds its scope inline (`mcp/tools/fleet.rs::usage_report`)
and is the counter-example that would otherwise survive.

**And two layering boundaries, because the first compile finds them — revision
6.** `ViewScope` is a `service/` type and the dependency runs one way:
`ViewScope::owns` calls the store, the store never calls it. So
`store/session_grants.rs` enforces "only the owner creates a grant" with a
plain column comparison against `sessions.owner_person_id` in its own SQL
(§4.3) — which also removes the ordering problem where the grants work would
otherwise wait on a type a later task creates. Separately,
`guard::access_allows` stays **store-free**, because it is shared with the
store-free `mcp/tools/present.rs::visible_to`; "is this caller the hub's
personal owner?" is resolved when the token is resolved and travels on `Caller`
as a boolean (§4.1, §2.7). Neither boundary is a style preference: a store read
in the gate splits it from the presenter, and a `service/` call in the store is
a dependency cycle.

Plus two lists that must be kept honest. `mcp/tools/tests_isolation.rs`'s
coverage assertion enumerates `WORK_ACTIONS` ∪ `WORK_LINK_ACTIONS` ∪
`AdminAction::NAMES` only, so a **session** matrix is a new test over every
session-addressed tool, each needing a hand-written dispatch arm behind `call`'s
`other => panic!` — not an extension of a list that exists. And
`src-tauri/src/backend/verdicts.rs::VERDICTS` needs a row per new command; the
count of rows is whatever that array holds, and `verdict_gen` publishes it.

### 5.2 The leak surface the choke-point list misses

These paths reach session data without passing any of §5.1's choke points.
**This table is the one inventory of them** — the companion plan's tasks cite
it rather than keeping a second list, because revision 4 had the plan calling
five of these "the surfaces neither document mentioned" while this table
already devoted a row to each. They are gathered here because the isolation
matrix M1 relies on is derived from the same choke-point list, so it will not
catch them; and because most of them are open **today**, before M1, to any
paired phone.

| Path | Where | What it yields | What M1 owes it |
|---|---|---|---|
| `list_worktrees` / `list_host_worktrees` | `mcp/tools/repo.rs`, `service/worktrees.rs` | takes **no `Caller` at all**. Returns every worktree fleet-wide with `occupants: Vec<{host_alias, tmux_name}>` for every alive session — so every private session's `tmux_name` and branch, in one call, to a `readonly` token | an `Extension(caller)` and an occupant-level filter (drop the occupant, not the worktree). Because there is no scope argument, the `ViewScope` rename produces **zero** compile errors here — it needs a test that fails when a tool whose answer can contain a `tmux_name` takes no `Caller` |
| `list_tasks` / `wait_for_task` / `cancel_task`, and the `task` event kind | `mcp/tools/orchestration.rs::list_tasks`; `support.rs::visible_task`; `store/rows.rs::TaskRow`; the `task` arm of `events_route.rs` | the only fence is `caller.host_alias`, null for a paired client. `TaskRow` carries `prompt`, `result` and `error` — the paragraph the worker's Claude wrote. On the stream, `task` is not in `HOST_BOUND_HIDDEN_KINDS` and `fence_frame` returns early for any kind that is not `session`, so every task row is broadcast to every open stream. `cancel_task` is additionally a mutation of another person's work | a person dimension on `visible_task` (resolve both session ids, fail closed), `list_tasks` routed through it, and the `task` kind fenced or hidden. Neither document uses the word "task" |
| `send_message { deliver, submit }` | `mcp/tools/messaging.rs::send_message`; `service/messages.rs::send_message_scoped` | gates only the **sender's** host; the recipient is never resolved through a session gate, and the two recipient checks inside `send_message_scoped` are both wrapped in `if !scope.is_all()`. `deliver` pastes into the target pane and `submit` presses Enter — arbitrary prompt execution inside a private session | resolve `to_session_id` through `resolve_row_and_gate` with the mutating flag, **before** the `is_all()` guards; and refuse `deliver`/`wake` to anyone but the owner, since they are pane writes |
| `discover_lost_sessions` + `new_session { resume_claude_session_id }` | `mcp/tools/session_ops.rs::discover_lost_sessions` and `::new_session`; `service/sessions/discover.rs`; `lifecycle.rs::new_session_inner` | the only gate is `require_host`, which passes unconditionally for any caller with no `host_alias`. Reads the host's Claude transcripts off disk (`discover_transcripts_script`'s own `.clamp(1, 500)`) and returns `cwd`, `git_branch`, `claude_session_id`, `derived_tmux_name` and `existing_session_id` per candidate. Then, for any candidate fleet holds no row for, `new_session { resume_claude_session_id }` starts `claude --resume` on **another person's conversation** in a session the caller owns and may attach a terminal to. `reject_held_conversation`'s refusal text leaks `row.id` and `row.tmux_name` besides | make the discovery an administration read (master only in M1, as `/reports` already is) and filter its candidates; and **refuse a `resume_claude_session_id` the caller cannot prove access to — which requires a durable `claude_session_id → owner` record, decided in revision 5 and specified below this table.** A check against the session rows fleet holds cannot close it |
| `restore_host_sessions { dry_run: true }` | `mcp/tools/session_ops.rs::restore_host_sessions`; `service/sessions/restore.rs::plan_restore` | `require_host` only, and the confirm gate is explicitly skipped for a dry run. Returns per lost session `{ session_id, tmux_name, cwd, claude_session_id, friendly_name, action, reason }` — every field the privacy rule forbids. A reboot on a shared host turns every private session into a plan entry. Without `dry_run` it restarts them, spending the owner's AI account | thread the scope into `plan_restore`; restore only rows the caller owns (§4.3 invariant 5). Note that the same effect is reachable one row at a time through `recreate_session`, which is in the same tier for that reason |
| `spawn_review` | `mcp/tools/lifecycle.rs::spawn_review`; `service/sessions/review.rs` | gated by a **read** gate on the source. Runs `ensure_session_workspace` against the owner's session (a write: it respawns tmux and re-adds worktrees) and creates `<tmux_name>--review-<hex>` on the owner's host, in the owner's `cwd`, running a full Claude — which under revision 3's fork rule the *caller* would own, with a terminal, in someone else's checkout | the `own` tier (§4.3, invariant 5) **and** source-inheritance of owner and visibility (invariant 6) |
| `move_session` | `mcp/tools/lifecycle.rs::move_session`; `support.rs::require_move_hosts`; `service/move_session/mod.rs` | `require_move_hosts` is two `require_host` calls, both no-ops for a paired client. The move deliberately carries uncommitted work, git-ignored files, the Claude directory and project memory onto a host the caller names, where they are the unix owner — outside anything Fleet can revoke | the `own` tier (§4.3, invariant 5); a hard-failing owner/visibility carry in the transaction that creates the target row; grants dropped (§4.3) |
| The work graph's session reads | `work_link { summarize }` (`mcp/tools/orchestration.rs` → `service/work/summary.rs`); `work { today }` (`service/work/today.rs`); `service/work/view.rs::link_visible`; and the rest — the query is every session-metadata read under `service/work/`, plus `service/usage.rs`, `service/health.rs`, `service/trackers/tickets.rs` and `service/messages.rs` | "the whole work graph is untouched" is the single most dangerous sentence available here. `summarize` is addressed by `link_id`, never by `session_id`, so no session gate is on its path: it forks `claude -p --resume` on the session's own host and stores a précis of the transcript as an **org-readable journal row that survives the grant being revoked**. `today` returns `{name, host_alias, pr_url, claude_status}` for every private session in the fleet. `link_visible` returns `true` on `scope.is_all()` | every one of them is a session-metadata read and needs the person dimension; enumerate them from the code, not from this cell. `summarize` additionally needs its own read check on the link's session, sits in §4.3's `own` tier, and the journal row it writes must be fenced by the **summarised session's** owner, not by the link's org |
| `session:killed`, `move:progress`, `worktree:updated` | `events.rs::SessionKilledPayload` and the `move:progress` / `worktree:updated` emitters; `events_route.rs::fence_frame` | `SessionKilledPayload` is `{"id": N}` — no `host_alias`, no `session_id` — so it falls through `fence_frame`'s `_ => {}` arm and every kill of every private session reaches every stream: an existence oracle on a mapping the recipient learned while the grant was live. `move:progress` (`session_id`, `to_host`, free-text `detail`) and `worktree:updated` (`name`, `path`, `branch`) are never reached at all, because `fence_frame` returns early for any kind that is not `session` | extend `SessionKilledPayload` with `host_alias` + `visibility` at emit time (the row is gone by the time the frame is read) — a wire change, which the contract bump already pays for; drop the `kind != "session"` short-circuit; and add the enumerating test the isolation file's discipline implies: **every `EVENT_KINDS` entry is either hidden, fenced per frame, or explicitly declared to carry no session-scoped content.** On the desktop side, `session:killed` is re-emitted to the frontend, whose `events.ts` turns it into a `killed` session event and then a store removal; the backend's own `EventBridge::observe` arm for it maintains the resync bookkeeping set, not the store. Both halves are load-bearing and the frame cannot simply be dropped |

Two more that are smaller but in the same class: `fleet_health` picks
`HealthView::Fleet` for any caller that is not `is_scoped()`
(`mcp/tools/fleet.rs::fleet_health`), so a person's device gets
`sessions_total`, `by_status`, `ghosts`, `context_red`, `stuck` and
`usage_by_host` computed over everyone's private sessions — a live activity
channel if polled; and `mcp/tools/fleet.rs::usage_report` serves `tmux_name`,
`friendly_name` and per-session cost. Both branch on `caller.is_scoped()`
rather than on a scope *value*, so **no rename will flag them**. Every remaining
`is_scoped()` use should be read as a "do I need to filter?" test and triaged by
hand.

#### The durable `claude_session_id → owner` record — owner's delegate, revision 5

The `discover_lost_sessions` row above owes a mechanism, and revision 4 stated
the requirement without one, which would have left the implementer to narrow it
silently to the rows fleet still holds.

`new_session { resume_claude_session_id }` resurrects a transcript **precisely
when the session row is gone** — that is what makes it the interesting half of
that attack. A check against live rows therefore cannot close it, and neither
can a check against lost ones: `Store::delete_session` deletes the `sessions`
row outright, and `conversations` — the only other table that holds a
`claude_session_id` — is declared
`session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE`
(`migrations/037_conversations.sql`), so a reaped row takes its conversation
ids with it.

**M1 therefore adds a durable `claude_session_id → owner` record to the
ownership migration.** The shape is the implementing task's choice — a small
append-only table is the obvious one; the repo's own `work_unlinks`
(migration 070) is the precedent for "a fact that must outlive the row it was
about". The requirements are not the task's choice:

- it is **durable**: it survives session reaping, the GC sweep and
  `delete_session`, and nothing cascades it away;
- it is written whenever a session first binds a `claude_session_id`, on the
  same path `set_claude_session_id` takes today;
- it is **consulted before any resume** — `new_session`'s
  `resume_claude_session_id` arm, and `service/work/resume.rs`'s probe — and a
  resume of a conversation last owned by someone else is refused with a message
  that leaks neither the row id nor the `tmux_name` (today's
  `reject_held_conversation` leaks both);
- it is **not** a second source of truth for who owns a live session. The live
  answer is `sessions.owner_person_id`; this record answers only "who did this
  conversation belong to", for a row that no longer exists.

Retention is a real question and belongs with `work.retention.*`
(`store/work_retention.rs`): the record is one row per conversation, so it grows
with conversations rather than with traffic, but "durable" cannot mean "never
pruned" without someone having decided so.

### 5.3 The desktop, the contract, and the regeneration loop

Revision 3 reduced the desktop cost to "a new command needs one verdict row".
It needs seven artifacts, each enforced by a red test in
`src-tauri/src/backend/tests_routing.rs`:

1. the `VERDICTS` row (`src-tauri/src/backend/verdicts.rs`);
2. registration in `lib.rs`'s `generate_handler!`;
3. a body that reaches a `routed::` helper;
4. a `route("<command name>")` string literal in a file listed in `SOURCES` —
   asserted as a set equality both ways;
5. a driving `Case` in `routed_read_cases` / `routed_mutation_cases`, or an
   entry in `ROUTED_WITHOUT_A_CASE` with a named substitute;
6. the tool present in `guard::TOOL_POLICIES`;
7. the regenerations.

M1 pays this seven times over for the sharing commands, the claim, and the
watcher's pane snapshot added in §2.4.

There are **four** generated artifacts, not the two revision 3 named, and three
of them write the file and then panic on purpose so the diff is read — so each
needs a second clean run:

| Artifact | Command |
|---|---|
| `src/lib/hub_verdicts.generated.json` + the `docs/hub.md` refusal table | `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen` |
| `src-tauri/src/backend/hub_contract.golden.json` (every routed type's wire keys + the recorded revision) | `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` |
| `src-tauri/src/backend/local_only.golden.json` (the whole rendered `E_LOCAL_ONLY` message per command) | `REGEN_LOCAL_ONLY=1 …` |
| `docs/control-api-reference.md` | `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` |

`REGEN_DOCS` is not the only doc gate: `mcp/doc_gen.rs`'s
`narrative_guide_names_every_tool` `include_str!`s the **hand-written**
`docs/control-api.md` and fails until each new tool name appears there as a
backticked literal.

**The contract moves.** `src-tauri/src/backend/wire_contract.rs` states the rule
verbatim in its module doc: a command routed to a hub tool that did not exist
before must bump it. So `CONTRACT_REVISION` and `MIN_HUB_CONTRACT` /
`MAX_HUB_CONTRACT` all move **6 → 7**, with no mixed window — hub and desktop
upgrade together, which is `docs/hub.md`'s existing rule for this pair and the
reason it is acceptable. **The owner accepted this on 2026-09-30.** Note also
the hole in the contract golden exactly where M1's fields go: `tests_contract.rs`
states the invariant "every `Option` is `Some` so no key can go missing", but
its `sample_session()` sets `pr_evidence` and `pr_checked_at` to `None` and both
carry `skip_serializing_if`, so neither is pinned. If `owner_person_id` is added
the same way and sampled as `None`, the privacy-critical field is unpinned from
day one. `visibility` is `NOT NULL`, so it pins itself — which is a second
reason the fence keys on it (§3.7).

**Two column-pinning tests fail on the migrations as written**, and neither
document named them before revision 4:
`the_sessions_columns_are_the_ones_the_row_version_trigger_knows`
(`store/schema.rs`) forces the new session columns into a re-issued
`sessions_row_version_bump` (migrations 065, 071, 080 and 082 each had to
re-issue the whole trigger), and
`the_token_tables_columns_are_the_ones_the_auth_epoch_triggers_know`
(`schema.rs`) forces a companion `auth_epoch` trigger for
`client_tokens.person_id` — a *separate* narrow trigger in the new migration,
following `066_work_view.sql` and `074_client_assets_admin.sql`, because
migration 060's `WHEN` clause is a frozen applied file and there is no
`ALTER TRIGGER`.

**And the session migration must not be a migration.** An inline
`UPDATE sessions` is the exact thing migration 080 refused to do, with the
reason recorded in `migrations/080_stale_demoted.sql`'s header: any `UPDATE` of
`sessions` compiles `sessions_row_version_bump`, which names `lost_reason` — a
column that on a conversations-branch database exists only after
`repair_skipped_main_migrations()`, which `migrate()` runs **after** all pending
migrations. As drafted it aborts with `no such column: lost_reason` and bricks
that upgrade. It must be a Rust backfill in `store/schema.rs`, beside
`Store::backfill_stale_demoted`.

**On the frontend**, the new `SessionRow` fields must be declared **optional**,
following the `?:` convention the interface in `src/lib/sessions.ts` already
uses for its newer backend fields (`stale_working_at?`, `row_version?`,
`work_rev?`): there is no shared session test factory — several test files roll
their own, and many more use `SessionRow` as a value type — so a required field
breaks `pnpm check` across all of them. The store contract is
`src/lib/row_store.ts::createRowStore`, wrapped in `src/lib/sessions.ts` as
`mergeSession` / `removeSession` / `applySessionEvents` / `acceptCommandRow` —
`mergeOne` / `removeOne`, which earlier revisions and `CLAUDE.md` name, **do not
exist** — and it carries an undocumented monotonic guard, `sessionIsStale`,
which silently drops any payload whose `row_version` is lower than the held
row's. A share mutation's optimistic return value must therefore carry
`row_version`.

**The client's access answer is derived, never carried on the row — revision
6.** Revisions 4 and 5 reported the caller's own access as a field the backend
stamped onto each `SessionRow`. **That shape is rejected, and the pipeline is
the reason, not taste.** Three properties of the path a row takes, each
independently fatal to it:

- **The bus has no caller.** `BroadcastEventBus::emit` (`events.rs`) serialises
  a bare `SessionRow` once and hands the same frame to every stream. There is
  no per-recipient rendering step between the row and the wire on which a
  per-caller field could be computed.
- **Absent is indistinguishable from "no restriction".** `strip_nulls` runs
  before the frame enters the replay ring (§3.7), so a per-caller field the
  emitter could not fill arrives missing.
- **The store replaces the row wholesale.** `createRowStore`'s merge is a
  replacement, so every routine `session:updated` — a status change, a
  `last_prompt`, a reconcile touch — would **erase** whatever access the client
  had been told. And a fail-closed default for the erased value then shuts the
  **owner's own** terminal on a paired desktop, on an event the owner caused,
  with nothing wrong. Failing closed on a field that is wiped by normal traffic
  is not a safety property; it is an outage.

`needs_attention` is not the precedent it looks like: that field is the same
for every reader, so a broadcast can carry it.

**The shape instead.** Three inputs, and the client composes them:

| Input | Where it comes from | Why it is safe to broadcast |
|---|---|---|
| `owner_person_id` on the row | the `sessions` column | a caller-independent fact about the session |
| `visibility` on the row | the `sessions` column, `NOT NULL` (§3.7) | likewise; and the fence already keys on it |
| the client's own person id, and its **grant set** | fetched once when the client resolves, kept current by its own event | per-client, so it never rides a row |

The client **derives** `own` / `drive` / `watch` / none from the three. Nothing
per-caller rides a `SessionRow`, so a row event carries the same bytes to every
recipient and a wholesale replace loses nothing.

The grant set needs its own event because a grant mutates no `sessions` row and
therefore emits nothing today (§3.8). The repo's precedent for announcing a
computed change with no column behind it is
`store/orgs.rs::announce_org_moves`; a grant change follows it.

**Say the part that is easy to misread out loud: this derivation is for the
UI.** The hub enforces access independently, at §5.1's choke points, against
the caller it resolved from the token — a client that computed itself a
generous answer gets refused all the same, and nothing in the hub reads the
client's derivation. The one gate that genuinely *is* client-side is the
terminal, and that is a property of the terminal: `pty_open` is the desktop's
own `ssh` and the hub is not in its path (§2.4, §3.4), so there is no server
side to put it on. It is not the access model weakening; it is the one surface
the access model never reached.

**Where the derived answer is consumed.** The terminal's mount condition and
its active `pty_close` (§2.4, correction 1); the disabled state of every
per-session control; and the acceptance criteria, which must be written against
the derivation rather than against a field — "B's client derives `watch` for S
and renders no send box", not "B's row says `watch`". Mechanically there is no
home for it in the existing funnel: every mutating control reads
`src/lib/hub.ts::hubActionBlocked`, whose signature takes an action name and the
connection state and **no session at all**. A per-session refusal needs a
second, parallel predicate composed with it — and the bulk paths (`Sidebar`'s
select-mode fan-out over `killSession`, `BulkPromptDialog`'s over `sendPrompt`)
need the same exclusion the sidebar already applies to outside-fleet rows.

**One user-visible regression neither document acknowledged.** The backfill
attributes only rows fleet created (§4.3); every reconcile-discovered row stays
`unclaimed`, and `unclaimed` is count-only for every caller. Those rows are
today's sidebar content: `src/lib/sidebar_index.ts::buildOutsideFleet` filters
`s.kind === 'external'`, and `Sidebar.svelte` renders them under
`data-testid="outside-fleet-section"` with an orphan section beside it. On a
standalone desktop those two sections empty out on upgrade and are replaced by
a per-host count. That is the correct behaviour under the rule — a
hand-started session's metadata is somebody's content — but "nothing changed
for a single user" is not true of it, and the docs task must say what changed
and how to claim a row back.

**This is still the strongest argument for doing it now rather than later:**
the funnel exists, has tests, and already encodes "no existence oracle" for the
org dimension. But the work is switching it on for a new class of caller and
closing the §5.2 paths that go round it — not riding six predicates. Sizing M1
from the old sentence is the single most likely way to get it wrong.

---

## 6. Recommended sequence

| # | Milestone | Why here |
|---|---|---|
| **M1** | Two people on one hub, private sessions by default, explicit **person-to-person** sharing | the foundation everything else needs; see the companion plan |
| M2 | **Memberships**, and with them: **team sharing** (`session_share { org }`, moved out of M1 — Q9), `visibility = 'org'` (removed from M1 for the same reason — §4.3), person-scoped permissions and roles on the hub, who administers a host (which is what decides who sees an unclaimed count on a multi-person hub, §4.3), and the **downward-only administrative authority over a departed member's grants** (Q9), which M1 does not have because it has no membership to hang it on | needed before a company hub is real, and none of the four is expressible without it. **Built 2026-10-06** as org administration phase D (`2026-10-06-org-administration-design.md`, with the owner's answers to the three open questions); team sharing is an org grant rather than `visibility = 'org'` |
| M3 | The experimental space switcher | pure UX once M1/M2 exist |
| M4 | Per-session AI-account / host choice with remembered per-space defaults | small, valuable, independent |
| M5 | Org link between two hubs: payload, identity mapping, conflicts, pause | the big one; needs M1's identity |
| M6 | Offline work against a company hub + outbox + re-check on reconnect | the biggest mode change; last |

M4 is also where Q7's per-session account choice lands; it is a real project,
not a picker, because the probe, the usage poll and the transcript reader all
assume one config directory per host.

**Four obligations M1 carries that the sequence does not make obvious.** Each
is a task in the companion plan, not an aside here.

1. **Close the §5.2 paths that are open today**, before M1 —
   `list_worktrees`, the task tools, `send_message`'s recipient,
   `discover_lost_sessions`, `restore_host_sessions`, the work graph's session
   reads and the unfenced event kinds. After M1 they stop being pre-existing
   weaknesses and become holes in a guarantee the product makes.
2. **Bind `Access::Person` to the hub's personal owner** (§2.7). It is already
   wrong the moment a second person is paired, and it is a settings **write**,
   not a read. Small, and not optional.
3. **Add the durable `claude_session_id → owner` record** (§5.2) to the
   ownership migration, because `new_session { resume_claude_session_id }`
   cannot be closed without it.
4. **Give a desktop watcher something to watch** (§2.4, correction 4) — the
   routed pane snapshot. This one is a scope addition made in revision 5, not
   something revision 3 planned and revision 4 forgot; it exists because rule 4
   takes the terminal away and nothing replaced it.

---

## 7. The open questions, answered

The handover's *"Otvorené technické rozhodnutia"* list, plus the two the
analysis added. Each: the recommendation, why, how it works against the code
that exists, and what it explicitly does **not** promise.

**Where the three product decisions stand after the owner's review:**

| | Question | Status |
|---|---|---|
| Q4 | What syncs | **answered in part** — configuration by kind, yes. Two things left: what "a project" means as a cross-hub identity, and whether a project may hold several repositories. Excluding work items is a proposed scope, not a confirmed decision |
| Q9 | A person leaves | **answered** — privacy does not change on departure; existing grants survive; an administrative authority over them is downward-only, never widening or redirecting, and it **arrives with memberships in M2**, because M1 has no membership to hang it on. Revision 4 records two further owner's decisions here: **team sharing is out of M1**, and **grants are dropped when a session moves** |
| Q11 | Admin break-glass | **answered: no**, not in v1 |

One question is therefore still genuinely open, and it is a schema question
rather than a product one: **how a project is identified across hubs, and
whether one project may hold several repositories** (Q4). It does not block M1,
but the first half of it is worth fixing locally regardless — today two
non-GitHub repositories with the same basename collide.

### Q1 — How does a personal identity come into being with no registration, and how is it recovered after a lost device?

**Recommendation.** A person is a row on a hub, created **automatically** — by
first run for the hub's own owner, by pairing for anyone added later. No
registration form, no central service, no cross-hub account.

**Why.** The only thing identity has to do here is let one hub tell two humans
apart, remember which devices are whose, and make session ownership
unambiguous. That is a local question, and every answer that leaves the hub (an
account service, an email flow, key custody) buys nothing and costs a service to
run, a secret to protect and a failure mode to support.

**How it works.**

- A standalone desktop is unchanged in *use*: no login, no prompt. It simply has
  a personal-owner row from first run, so every session it starts has an owner
  (§4.1). Revision 1's "a personal hub can run with zero people" was optimising
  for an invisible property at the cost of the one that matters.
- Adding someone: `fleet-hub pair --name laptop --person jane`. On a company hub
  that command is the company's.

**Three mechanical corrections revision 4 adds.**

*The hub's own owner cannot be created by the migration alone.* The codebase's
pattern for "create the hub's own X on first run" is `ensure_master_token`
(`mcp/settings.rs::ensure_master_token`), which is idempotent and called from **three** entry
points — `serve::init`, `serve::token` and `serve::serve` — precisely because
none of them is guaranteed to run first. The person row needs the same shape.
And it cannot be *named* by the migration: a migration cannot read a hostname,
and `fleet.self`, which revision 3 cited, does not exist anywhere in the tree —
the only fleet-identity setting is `fleet.id` (`service/address.rs`), a
lazily minted UUID. The migration inserts a hardcoded placeholder, renameable
afterwards.

*A fresh desktop database and a fresh hub database each mint their own owner,
and nothing reconciles them when that desktop later pairs to that hub.* That is
tolerable — they are two hubs' worth of rows by design (§4.2) — but it must be
decided rather than discovered, because it is what "one profile, several hubs"
costs.

*"Creates the person if the name is new" hides a fork the plan must choose.*
`pair_client`'s existing `org_id: Option<i64>` is an id for a row that already
exists, and it is validated at mint time (`mcp/tools/fleet.rs::pair_client` answers
`E_NOTFOUND`). A person has no analogue. Minting a code is not a commitment —
codes live in a process-memory `Mutex<HashMap>` and die on a hub restart
(`mcp/pairing.rs`) — so creating the person at mint time leaves an orphan
`people` row for every abandoned code, while creating it at redemption means the
code carries a **name**, not an id: a different field type, a different
`Pending`, and validation that must still happen at mint time so the operator
sees the error at their terminal rather than the phone seeing it.
- The same human on two hubs is two rows. Deliberate — it becomes a *mapping*
  when org linking lands (Q4), never a global account. What must not differ is
  the **experience**: one profile, several spaces, no re-setup on joining (§4.1).

**There is a secret, and there is a recovery path.** Revision 1 said "no secret
that can be lost", which is wrong. Devices hold bearer tokens, and two distinct
losses need two distinct answers:

| Lost | Recovery |
|---|---|
| A person's device | Someone with hub administration revokes it (`fleet-hub client revoke phone`) and pairs a replacement onto the same person. The person row, their sessions and their grants are untouched. Nothing is derived from the lost device |
| **Administrative access to the hub itself** | Must be designed, and is not designed here. The master token lives in `settings` inside `state.db`; `fleet-hub token regenerate` takes effect on the next start. That is fine for an operator with shell on the hub machine and **not** an answer for a personal hub the owner reaches only through a paired desktop. An M1 exit criterion should be: an owner who has lost every paired device can still recover, and the procedure is written down in `docs/hub.md` |

**Not promised.** No password, no email verification, no protection against the
hub's operator — they hold `state.db`. Every permission question is "what may a
person do", never "what can be hidden from the operator".

### Q2 — How does a second device safely join the same identity?

**Recommendation.** Use the pairing flow that already exists, with one added
field. Do not design a new protocol.

**How it works today** (`mcp/pairing.rs`, `POST /pair`):

- A single-use code with a TTL (30–3600 s, default 600), shown as a QR.
- The DB stores only the token's **SHA-256** (`client_tokens.token_sha256`), so a
  stolen database hands out no usable bearer token.
- The code can already carry a binding — `mint_bound` carries the org today.
  "The person goes in the same slot" understates it: see Q1's third correction,
  the slot holds an **id of an existing row**, and a person may not have one
  yet.
- Migration 060 puts triggers on `client_tokens` that bump `auth_epoch` *inside
  the writing transaction*, and `TokenCache` compares the epoch on every
  request, so a revocation from any process lands on the next **request**.

**Revocation is time-bounded, not immediate — and not uniform.** Revision 1
overstated this. What actually happens to a live connection when a token is
revoked:

| Connection | What stops it | Bound |
|---|---|---|
| A new request of any kind | `TokenCache` re-reads the epoch per request | immediate |
| An open `/events` stream | `client_is_live` on the keep-alive beat | **≤ 15 s** (`KEEPALIVE_INTERVAL`) |
| A long-poll tool already in flight | nothing — authorization is checked once, at entry, and the call runs to its deadline | **up to 660 s** (`LONG_POLL_CAP`), and it returns data gathered after the revocation |
| An attached terminal (`pty_open`), and the pane's file drop (`upload_to_session`) | **nothing in Fleet.** Both are the desktop's own `ssh`, outside the hub entirely | until the user closes it or SSH drops |
| `/hook` traffic from hosts | per-request, and it uses host tokens, not client tokens | n/a |

The first two are acceptable and should be *stated* rather than rounded to
"immediate". The third and fourth are gaps M1 has to close or explicitly
document. This matters most for the sharing acceptance criterion: "A revokes the
share and B loses access" has to mean something precise.

**Three corrections revision 4 makes to that table.**

*The `/events` row is wrong for the caller M1 introduces.* The ≤ 15 s bound is
real for an **org-bound or host-bound** caller. For an unbound paired client —
which is the shape a person's device has — the keep-alive re-scope
and the pre-frame generation check are both inside
`if let Some(..) = rescope`, and `rescope` is `caller.is_scoped().then(..)`:
`None`. `client_is_live` does fire, but it re-reads only the token row and its
org. So for M1's default shape a
revoked **grant** is noticed on the stream *never*: the scope is read once at
connect and never again for the life of the connection. The ≤ 15 s bound only
exists once `rescope` is made unconditional for any caller carrying a person —
and even then it applies to `Phase::Live` only, not to a reconnect's replay
phase, which drains the whole replay ring (`events.rs::REPLAY_RING`)
without re-checking.

*There are five long polls, not three, and one of them writes.* `Deadline::LongPoll`
is carried by five rows of `mcp/guard.rs::TOOL_POLICIES` — `wait_for_session`,
`wait_for_reply`, `run_prompt`, `wait_for_task` and `add_project`.
`run_prompt` is `readonly: false` and its order is permit → `deliver_prompt`
(`mcp/tools/orchestration.rs::run_prompt`) → wait → transcript. A re-check before
returning can withhold the transcript; it cannot un-send a prompt already in the
owner's pane. So "B loses access" must be written down as **not** meaning "B's
in-flight prompt is recalled". (Holding a long-poll permit and *being* a long
poll are also different lists: `peer_exchange` takes a permit and is
`Deadline::Quick`. And `wait_for_task` takes its permit *before* its
authorization check, the reverse of the other three, so an unauthorised caller
can burn one of the eight slots.)

*A re-check is cheap in all four session-bound waits.* They already wake at
least twice a second — `service/tasks.rs`'s two 500 ms polling
loops that lock the store on each wake, and `service/messages.rs` caps each
`Notify` wait at a 500 ms floor. The only structural work is threading an access
predicate into `fleet-core`'s service layer, which must not import `Caller`:
follow the existing `PaneProbe` trait (`service/tasks.rs::PaneProbe`).

**Four different revocations, and they must not share a mechanism.** Revision 3
named two; the other two were introduced by M1's own design and had no mechanism
at all.

| Event | Question it answers | Mechanism |
|---|---|---|
| A **device** is revoked | "is this token still a person's?" | `client_tokens.revoked_at` + `auth_epoch` + `TokenCache` — exists today |
| A **grant** is revoked or narrowed | "may this person still reach this session?" | read **live** per call, with its own generation for streams — new |
| A **device is unbound from a person**, or rebound to another | "*whose* token is this now?" | **nothing today.** The per-beat liveness check is `client_is_live`, whose body is `client_token_org(id) == Some(org)` (`store/clients.rs::client_is_live`) — it reads the org column and nothing else, so a `person_id` change is invisible to it and the open stream keeps the old person's scope indefinitely. It needs `Store::client_token_binding(id) -> (Option<i64>, Option<i64>)` returning both columns from the same row, compared as a pair — kept **separate** from the grant re-read, so the two stay distinct events |
| A **person is disabled** | "has this human's access ended?" | **nothing today.** `Caller::person()` reads `ClientRef.person_id`, filled from `Store::active_client_tokens()`, whose `WHERE` is `revoked_at IS NULL` on `client_tokens` — there is no join to `people` and no `disabled_at` predicate anywhere. As drafted, disabling a person only frees the name. It must either be compound (revoke every token bound to that person, which bumps `auth_epoch` and so lands within a request and within 15 s, **and** mark every live grant to them revoked) or make the scope builder refuse a disabled person — and the plan must say which, because the two differ in what happens to that person's **own** sessions |

A valid device token is not, by itself, a right to any session. So a grant must
**never ride the token cache**: caching it in the resolved `Caller` would mean a
revoked share stays usable for as long as the cached caller lives, and bumping
`auth_epoch` on every grant change would invalidate every token's cache for a
change that concerns one row. The scope reads grants when it is built, and a
grant change bumps its own counter — the same shape `ORG_GENERATION` already
has for org changes, which streams re-read on the keep-alive beat.

**But the counter is not the guarantee, and `docs/hub.md` must say so.**
`auth_epoch` is a SQLite **trigger**, so a write from any process — including a
`fleet-hub` subcommand — bumps it. `ORG_GENERATION`, the shape being copied, is
a process-local `static AtomicU64` with exactly one bump site
(`service/orgs.rs`, one bump site), and there is already a write path that misses it
(`mcp/pairing.rs` calls `Store::set_client_org` directly). A grant revoked
by a CLI subcommand that opens the store directly — the `client grant assets`
precedent (`crates/fleet-hub/src/pair.rs`) — would bump nothing in the
serving process. Open streams still recover, because `read_scope` re-reads from
the store on the keep-alive beat; so **the 15 s beat is the guarantee, not the
counter**, and the bump belongs inside `store/session_grants.rs` where the write
is, not at a service call site.

**And a grant change emits nothing at all** (§3.8), so the recipient's UI does
not learn about it either way. Sharing needs a grant-announce mirroring
`announce_org_moves`, and revoking needs the recipient's **resume** to be
invalidated: the desktop's `EventBridge::pump` re-lists only when the hub
answered `resumed: false` (`src-tauri/src/backend/events.rs::EventBridge::pump`), so after a
revoke the hub's fence is correct, the stream ends, the bridge reconnects with
`Last-Event-ID`, the hub replays the correctly-fenced gap, answers
`resumed: true` — and the revoked row stays in the recipient's sidebar
indefinitely. A hub-side e2e assertion passes while the UI is wrong.

**The one property worth protecting:** pairing requires hub-side action. There is
no "request access" a stranger can initiate. Do not add one.

**Not promised.** Whoever can run `fleet-hub pair` can attach a device to any
person. Unavoidable, and the reason permissions are about capability, not
secrecy.

### Q3 — What exactly is the relationship between user, organisation, space and hub?

**Recommendation.** Five nouns, one of which is not stored:

| Noun | What it is | Where it lives |
|---|---|---|
| **hub** | one fleet = one `state.db` = one authority | a machine running `fleet-hub` |
| **person** | a human, *within one hub* | `people` (new, T1) |
| **device** | a paired client token pointing at a person | `client_tokens` (+ `person_id`) |
| **org** | a boundary for work data *inside* one hub | `orgs`, `org_rules` (exists) |
| **membership** | person × org, with a role | new, M2 |
| **space** | **not an entity** — what one device is currently showing | the device's selection |

Personal space = `(hub, person)`. Work space = `(hub, person, org)`.

**"One personal account, several companies"** therefore means: one human, several
hubs, one `people` row per hub, one device token per hub. The desktop switches
which `(hub, token)` it is using. Two consequences worth stating now:

- The keychain slot must become **N slots keyed by space**. Today it is exactly
  one: `claude-fleet/hub-client-token` (`src-tauri/src/backend/token_store.rs`).
  The count is right; the word "keychain" is not, on the platform this repo is
  developed on. There is no keychain on Linux: `OsTokenStore` writes an
  owner-only (0o600) plain **file** at `<data_dir>/hub-client-token`
  (`token_store.rs::OsTokenStore`). So "N slots" means N credential entries on
  macOS/Windows and N files on Linux — and the `TokenStore` trait
  (`token_store.rs::TokenStore`) has three unkeyed methods and no enumerate operation at
  all, with exactly one non-test constructor, `app.manage`d as a single
  `Arc<dyn TokenStore>`. The space key has to enter either the trait or the
  managed value; it cannot be bolted on at a call site.
- A company hub never learns your personal hub exists, and vice versa, until you
  explicitly link two orgs (Q4). Nothing in this model leaks by default, because
  nothing crosses hubs by default.

**Why a space is not a table.** Everything it would hold is already held by the
four real nouns; a table would only add a second place for them to disagree.

### Q4 — Which parts of the settings sync, and which stay strictly local? · **DECISION — owner has answered in part**

**Owner's answer (2026-09-30):** yes to syncing **configuration by kind** as the
first version — org definition, org rules, tracker configuration without
secrets, and *optionally* organisation-level views. **Personal views stay
personal.** Sessions, hosts, tokens and transcripts are not transferred.

Two items the owner flagged as not yet settled:

- **Projects must be resolved explicitly.** They were part of the original
  intent and revision 1 simply left them off the list. They are also the one
  entry that does not travel cleanly, which is *why* they need a decision rather
  than a default:

  | What a project row holds | Travels? |
  |---|---|
  | `owner` / `repo` (the GitHub coordinates) | **yes** — the same repository is the same repository on both hubs |
  | `base_path` (a filesystem path) | **no** — it names a directory on *a host*, and hosts are not shared |
  | `last_session_at`, `adopted` | no — local state |

  So the honest unit is not "the project row" but **"which repositories this
  organisation works on"**. Each hub resolves that to its own local path when a
  session starts: sync the repository list as part of the org definition, never
  `base_path`.

  **But `(owner, repo)` is not an identity, and today's schema cannot carry
  one.** The owner is right that GitHub, GitLab and self-hosted servers need a
  provider or host in the key, and the code makes it worse than a gap — it is a
  live collision:

  - `projects` is `UNIQUE (owner, repo)` with no provider column
    (`001_init.sql`).
  - `add_project`'s adopt path parses `(owner, repo)` from `origin` **only when
    it is a GitHub URL**, and otherwise falls back to
    `("local", <sanitised basename>)` (`service/add_project.rs::add_project`).
  - So `git@gitlab.com:team/api.git` and `git@git.firm.internal:ops/api.git`
    both become `local/api` and collide on the unique index — on **one** hub,
    before any sync exists.

  Recommendation: fix the identity before syncing anything that uses it. A
  project's cross-hub identity should be the **normalised remote**
  (`host/owner/repo`, e.g. `github.com/martin-janci/claude-fleet`), stored
  alongside the existing columns so nothing breaks, and used as the sync key.
  That is a small migration and it pays for itself locally regardless of M5.

  **One more thing to verify before the list is closed:** a project row is one
  repository. Multi-repo work exists at the *work item* level
  (`work_link start { project_ids }`, M9.6), not the project level. If "a project
  with several repositories" is part of what the owner means, that is a schema
  question of its own and it should be answered before the sync payload is
  designed, not after.

- **Excluding work items is a proposed scope, not a confirmed decision.** Stated
  as such: revision 1 presented it as settled and it is not. The argument for
  leaving them out of v1 is that a work item is *state* with a lifecycle, a
  tracker of origin and per-org fences, so syncing it is a different and much
  larger problem than syncing configuration. The argument against is that the
  owner's original goal was to not lose work when moving between hubs. Decide
  when v1 configuration sync is working, on evidence.

**Never synced, and this is an invariant, not a default:**

> sessions, transcripts, conversations, journals, usage, hosts, `host_tokens`,
> `client_tokens`, `people`, and **every** `*_secrets` table.

The secrets tables have exactly one reader each — `Store::resolve_tracker_credential`,
`Store::resolve_decision_credential`, the catalog's own — and that invariant must
not be widened by a sync feature. A synced tracker config arrives **without** a
credential and is inert until the receiving side supplies its own.

**Why per-kind and not per-field.** A per-field UI is a large surface for little
value, and a half-synced org is a support problem nobody can reason about. Four
or five named, understandable toggles is the right granularity.

**Direction matters.** Personal → work is a **proposal** evaluated under the
sender's current permissions on arrival (Q5). Work → personal is a **read**: the
company's rules land locally as read-only, exactly as `hub.*` / `mcp.*` specs are
already read-only on a paired desktop (`owned_by`, declarative pages P6).

### Q5 — How are deletions, conflicts and permission changes handled while disconnected?

**Recommendation.** Three separate mechanisms, each already present in the
codebase. Do not invent a fourth.

**Queue** — copy `tracker_writes` (migration 061) exactly: `state`, `attempts`,
`last_error`, `next_at`, plus a **unique index on the operation's identity** so a
repeated trigger is a no-op. Idempotency then comes from two directions: that
index, and the peer link's own acknowledgement (`WireResult::accepted` /
`rejected`, and the `after` cursor in `ExchangeRequest`). The transport is
at-least-once; de-duplication happens at apply.

**Conflicts** — `version` + `expected_version` + `E_CONFLICT`, the pattern
migration 066 already established for work links. On conflict, **do not
auto-merge**: surface both values and let a person choose. That is precisely what
the owner asked for ("výber správnej verzie pri súbežnej zmene").

**Deletions** — a tombstone, never an absence. An absent row on the sender must
never delete on the receiver, or a paused sync becomes a mass delete when it
resumes. The repo's own convention: `participants.retired_at`,
`client_tokens.revoked_at` — a retired row *resolves, and says it is gone*.

**Permission changes while disconnected** — the rule, stated once:

> A queued change carries **what** and **who proposed it**. It never carries
> authority. On arrival it is evaluated against the recipient's **current**
> membership and role.

There is already a precedent on both halves: `tracker_writes.session_org_id` is
"re-checked against the tracker's org before anything is sent", and
`setting_proposals` (migration 083) is a proposal a person reviews, applied only
through `settings::set_by` with an `Actor`. An incoming change should land in
that shape. A change made before a membership was revoked is rejected on arrival,
with a reason the sender can see — not silently dropped.

### Q6 — What does "may send tasks into someone else's session" actually grant?

This one is larger than it looks and deserves a blunt answer.

Sending a prompt into a session means that session's Claude acts **with its own
host's permissions, its own AI account, in its own working tree, with its own
tools** — including the fleet control API through that host's token. `drive` is
therefore closer to *"make this machine do things as its owner"* than to *"leave
a comment"*.

**Recommendation.** **Two grantable levels**, described honestly, plus the
owner's own tier — and sharing is **not transitive**. Revision 3 had two levels
and expressed the rest as a deny-list; revision 4 replaced the deny-list with a
positive third *level*, which was half right and half a mistake, because it
invited the reading that `own` is something a grant can confer. Revision 5
settles it: `own` is a **tier**, not a level anyone can be given.

| Level | May | Conferrable by a grant? |
|---|---|---|
| `watch` | read the row, transcript, conversations, history, repo reads, and the pane snapshot M1 adds (§2.4) | yes |
| `drive` | watch + `send_prompt`, `run_prompt`, `dispatch_task`, `send_message { deliver, submit }`, reply actions — anything that makes the session *work* | yes |
| `own` | everything that copies, relocates, re-creates, destroys, renames, re-tags or re-shares the session | **no. Not a level. The owner's alone, and no grant reaches it** |

**The membership of the `own` tier is defined in §4.3, invariant 5, and this
answer deliberately does not restate it.** Revision 4 restated it here, the
plan restated it in two more places, and all four copies disagreed — on whether
a `drive` grantee may kill, restart or rename the owner's session, which is
precisely what `drive` means. One place, cited from everywhere; go there.

Mechanically it is one **reach** carried into
`mcp/tools/support.rs::resolve_row_and_gate` (**not** the `resolve_and_gate`
wrapper: both paths meet in the former), not a `may_drive` boolean. A boolean
cannot express the third answer, and every operation a deny-list forgets lands
silently on the permissive side of it.

One thing to keep: a grantee's prompts still carry the untrusted-content marker
unless their device is trusted (`mark_untrusted` / `apply_marker`,
`client_tokens.trusted_at`). Trust stays **per device**, as it is today — not per
grant, and not implied by a grant.

### Q7 — How are company AI accounts handed out and used without leaking credentials?

This question has two halves. Revision 1 answered one and quietly dropped the
other.

**Half one — credential distribution: do not build it.** Fleet never holds an AI
token today, and that property is worth defending. `accounts` is populated from
each host's `~/.claude.json` `oauthAccount` during the probe and carries `email`,
`organization_name`, `organization_uuid`, `seat_tier` (migration 003);
`hosts.account_uuid` and `sessions.account_uuid` record which account a session
ran under. Nothing copies `~/.claude` between machines, nothing lends an account,
and no fleet-held OAuth token exists. Keep all three true.

**Half two — choosing between a personal and a company account: this is the
actual requirement, and it is not met.** Revision 1 answered "an AI account is a
property of a unix user on a host", which converts the requirement into a
deployment constraint: to use two accounts you need two machines (or two unix
users). That is a **scope reduction, not a solution**, and it should not have
been presented as one.

The mechanism to do it properly already exists in the codebase, unused for this:

- `CLAUDE_CONFIG_DIR` is already honoured — `service/account_usage.rs` reads
  `"${CLAUDE_CONFIG_DIR:-$HOME/.claude}/.credentials.json"`. Codex has
  `CODEX_HOME`.
- The pane is launched with `tmux new-session -e KEY=VAL` (`tmux.rs`), which
  is exactly how a per-session value reaches the agent.

So a per-session config directory — one per account, on the same unix user — is
mechanically straightforward. What makes it a project rather than a patch is the
rest of the surface that assumes one config dir per host: the probe that
discovers accounts, the usage poll, the transcript reader
(`~/.claude/projects/*.jsonl`), the hooks install, and provisioning. Those have
to learn "which config dir" alongside "which host".

**Recommended shape, staged:**

| Stage | What | Where |
|---|---|---|
| now (M1) | do not regress: keep Fleet credential-free | — |
| M4 | `sessions.config_dir`, a per-session account picker, `-e CLAUDE_CONFIG_DIR=…`, and the probe/usage/transcript paths taught to follow it | fleet |
| M4 | warn when a work session starts under an account whose Anthropic org differs from the work org | fleet |
| with multi-harness | the same for `CODEX_HOME` | the multi-harness spec |

**Codex is not covered by anything above.** `accounts` reads `~/.claude.json`
only; there is no harness field on `SessionRow`; the pane command is Claude's
(`cl --resume …`). Per the multi-harness umbrella spec
(`docs/superpowers/specs/2026-09-29-multi-harness-agents-design.md`, §3), Codex
sessions are a separate project. Nothing in this document should be read as
"Codex accounts work" — they are not modelled at all, and the account picker
above must be designed so adding a harness is a new row, not a rewrite.

**Honest limit that survives all of this.** Whoever holds the unix account on a
host can read the credentials in any config dir under it. A per-session config
dir separates *accounts*, not *people*. Keeping two people apart still needs two
unix accounts (§4.4). Two different problems, two different mechanisms —
conflating them is what produced revision 1's answer.

### Q8 — How does sync behave when the two hubs are different versions?

**Recommendation.** Per-kind payload versioning with **refuse and say so** —
never best-effort. Copy the *agent* model, not the desktop one.

**The two precedents in the repo, and why the choice matters.**

- hub ↔ desktop is a **hard gate**: `CONTRACT_REVISION` (today 6); a newer
  desktop refuses an older hub and vice versa with `E_HUB_CONTRACT`, and there is
  deliberately **no mixed window**. That works because one person upgrades both.
- hub ↔ agent is **tolerant**: a release holds `MIN_SUPPORTED_PROTO` at the
  previous value so an older agent keeps connecting until it is reinstalled.

Two independent companies cannot be made to upgrade in the same window, so
hub ↔ hub must be the tolerant kind.

**Concretely.** `ExchangeRequest.proto` stays the transport version (1, strict —
leave it alone). Each sync payload carries its own `kind` + `v`. A receiver that
does not know `(kind, v)` **rejects that message** — `WireResult::rejected`
already carries `code` and `message` — and the link stays up for everything else.
A rejected kind surfaces in `fleet_health` beside `peer_links_down`, so a person
reads *"the other hub is too old for tracker-config sync"* instead of watching
nothing happen.

**The rule to write down:** never widen a payload in place. A field that changes
meaning is a new `v`.

### Q9 — What happens to sharing when a session's owner leaves the company? · **ANSWERED**

**Owner's answer (2026-09-30):** access to the company ends; their local history
stays; **privacy does not change on departure** — private sessions are not made
available to the company. What happens to sessions they had *already shared* is a
separate decision, still open: blanket revocation may needlessly interrupt work
the team is in the middle of.

**What that settles.**

- The session row, its transcript and its work links stay on the company hub —
  it is the company's record on the company's machines, and deleting it was never
  asked for.
- Their private sessions stay private. They do **not** become org-visible, and
  there is no admin path into them (Q11). In the schema they are simply owned by
  a person who can no longer reach the hub.
- Their local history on their own machine stays. No remote wipe exists, none was
  asked for, and none should be built.

**What revocation actually does, precisely** (Q2's bound applies):

- Their devices are refused from the next request; open `/events` streams end
  within ~15 s; an in-flight long poll can still run to its deadline; an attached
  terminal is unaffected by Fleet and ends when SSH does.

**Two owner's decisions added in revision 4 (2026-09-30).**

*Team sharing (`session_share { org }`) is out of M1, deferred to M2.* Taken
knowingly as a scope reduction, with the reason: org membership of a client is
written by `work_admin { assign_client }`, which is `Access::Master` — so an
admin binds their own device to the org and reads every org-shared session, with
no grant touched and no owner consent. That defeats the rule that privacy holds
against the admin (§4.5, Q11). M1 ships person-to-person grants only; the
`org_id` column stays in `session_grants` and the store refuses an org
recipient. Full reasoning in §4.3.

*Grants are dropped when a session moves.* `move_session` creates a new row with
a new id, and `session_grants` are keyed on the old one. The grants are revoked
rather than carried; the owner re-grants if they want to. Narrowing is the safe
direction, and a carry would be the system widening a grant on the owner's
behalf, against invariant 3. §4.3 also makes moving an `own` operation, which is
the more important half: a grant is permission to use a session where it is,
never to relocate it onto a machine the grantee controls.

**Owner's answer on the earlier sub-decision (revision 3): existing grants
survive, and an administrative authority over them is downward only.**

| Option | Consequence |
|---|---|
| Existing grants are revoked with the membership | Nothing outlives the authorisation, but a colleague loses a session mid-task for a reason that has nothing to do with them |
| **Grants survive; an admin may revoke or narrow them** | **Chosen** — as the shape of the eventual authority |
| Grants survive and an admin may re-home them | **Rejected** — see below |

Revision 2 offered "revoke or re-home" as one option. Re-homing is not a smaller
version of revoking; it is a privacy bypass. If an admin can change a grant's
recipient, add one, or raise `watch` to `drive`, then an admin can grant
themselves access to any private session with a departed owner — which is
exactly the capability Q11 refused. So the authority is bounded by §4.3's
invariant 3:

> An admin may **revoke** a grant or **narrow** it (`drive` → `watch`). Never
> widen, never add a recipient, never redirect one. Creating a grant remains the
> owner's alone.

**When that authority arrives — owner's delegate, revision 5: in M2, not in
M1.** Revision 4 wrote the sentence above as though M1 delivered it, and the
plan's definition of done promised it could be demonstrated. It cannot be, and
not for want of a task: **M1 has no membership**, so it has no "admin of the
organisation this person belonged to" to name as the caller, and §4.3's
invariant 5 places `session_unshare` and `session_narrow` in the owner's own
tier, which no admin is in. Building an exception would have meant inventing
the role M2 exists to define, in the milestone that deliberately has no roles.

So, precisely:

- **M1**: a grant is created by the owner, revoked or narrowed by the owner,
  and by nobody else. A departed person's grants simply stand. Their own access
  ends with their devices (Q2's table), and a departed owner can issue nothing
  new, because issuing a grant is a request like any other.
- **M2**: memberships arrive, and with them the administrative authority in
  exactly the downward-only form above — a named store entry point, a
  `fleet-hub` subcommand, and an audit row. That is where it is built and
  where it must be demonstrated.

Nothing about the *privacy* answer changes between the two: a departed person's
private sessions are private in M1 and in M2, and there is no admin path into
them (Q11).

### Q10 — Which existing settings and sessions need migrating, and to whom?

**Recommendation.** Attribute what can be attributed, hold the rest safely, and
never let the upgrade widen access.

Revision 1 said "assign nothing to anyone, everything becomes `visibility =
'org'`". The owner rejected the second half, and rightly: that is precisely the
failure mode — a hub runs single-user for a year, a colleague is added, and every
session from that year is readable by them. The correction:

**§4.3's table is the authority; this is the same table, stated as an upgrade.**

| Case | Owner | Visibility |
|---|---|---|
| The hub has one person and fleet created the row — `started_at IS NOT NULL` | that person | `private` |
| Fleet did not create the row — `started_at IS NULL`, the reconcile-discovered population | NULL | `unclaimed` |
| The hub already has several paired devices and the owner is genuinely undeterminable | NULL | `unclaimed` |
| Discovered later from a hand-started tmux session | NULL | `unclaimed` |

The first case is the overwhelmingly common one and it needs no guessing: before
M1 a hub *is* single-user, so its personal-owner row (§4.1) is the owner of
everything it started. This is the "safe transition or explicit takeover by
the original user" the owner asked for, with the takeover needed only where
attribution is genuinely impossible.

**The discriminator is `sessions.started_at`, and revision 5 corrects this
answer to say so.** Revision 4 had this question demand that after the upgrade
"**every** session carries that person as owner", while §4.3's table and the
plan's backfill attributed only the rows fleet started. The demand was the half
that was wrong, and it was written as a required test, so it would have been
the half an implementer trusted. `started_at` is documented "when fleet created
the session (NULL for tmux-discovered rows)" (`store/rows.rs::SessionRow`), and
it separates exactly the two populations: a row fleet started, which the hub's
one person demonstrably owns, and a row reconcile found, which nobody can speak
for.

`unclaimed` means **a per-host count and nothing else** — §4.3 is the authority —
claimable in one explicit, recorded act that proves the pane. Revision 3's
wording here, "listed but content-refused", is struck: it was one of three
incompatible readings across the two documents, and it is the one that cannot be
expressed on `/events` at all, where a `session:*` frame *is* the row and *is*
its content. Nothing silently vanishes, because the count is shown; and nothing
is exposed, because a row is content. §5.3 says what a single-user desktop sees
change on upgrade, which is not nothing and must be documented rather than
promised away.

**How the attribution is written matters.** Not as an inline `UPDATE sessions`
in the migration: that is the exact thing migration 080 refused to do, and on a
conversations-branch database it aborts with `no such column: lost_reason` and
bricks the upgrade (§5.3). It must be a Rust backfill after
`repair_skipped_main_migrations()`, modelled on `Store::backfill_stale_demoted`.

**What is deliberately not attributed**, with a stronger reason than revision 3
gave. Revision 1 said `session_events`' caller labels are token labels rather
than people. There are no caller labels: `session_events` has five columns —
`id, session_id, at, kind, detail` (`migrations/013_session_events.sql`) — plus
`claude_session_id` from migration 037, and no actor or token column was ever
added. The only caller label anywhere is `audit()` (`mcp/tools/support.rs::audit`),
which writes to `tracing` and does not include the caller either. So there is
nothing to mine, not merely the wrong thing to mine. Attribution comes from "the
hub had one person", full stop.

**Settings.** None move. `hub.*` / `mcp.*` are already read-only `owned_by` specs
on a paired desktop. The per-person settings M3/M4 need are *new* keys with
defaults, added the normal way — a `SPECS` row plus a page `field`, because
`every_setting_has_one_home` fails otherwise.

**Verify mechanically**, with two assertions revision 3 did not name — and with
the first of them stated in the only form that is true. The upgrade test and
downgrade guard in `store::testgen` (M12) must cover M1's migrations, so an
install that upgrades and rolls back loses no column's data. And the chain test
must assert that after the upgrade:

- `people` holds exactly one row;
- every session **with `started_at IS NOT NULL`** carries that person as owner
  with `visibility = 'private'`;
- every session **with `started_at IS NULL`** carries `owner_person_id IS NULL`
  and `visibility = 'unclaimed'` — the assertion that matters as much as the
  first, because a backfill that over-reaches is a silent attribution of
  somebody else's session;
- no row carries any other `visibility`. In M1 that is enforced by the column's
  `CHECK`, not by a `SELECT COUNT(*) FROM sessions WHERE visibility = 'org'`:
  the value is not in the type, so the test asserts the constraint exists and
  that an `INSERT` of `'org'` is rejected.

**And note what the rollback costs.** M1's migrations move
`known_schema_version()` past the last applied one, which is published on the
wire through `fleet-hub compat` and signed into the update windows (§3.9). Once M1 ships, a
hub that has opened an M1 database can never be reopened by a pre-M1 build
except read-only. That is normal and acceptable; it belongs in the release
notes rather than in a support ticket.

### Q11 — Does an org admin get a break-glass path into a private session? · **ANSWERED: no, not in v1**

**Owner's answer (2026-09-30): no.** There is no administrative bypass of session
privacy in the application in v1. Revision 1 proposed one; it was a new exception
to the owner's explicit decision that a private session is private from the
company admin too, and it was not approved.

**Revision 1's argument is also withdrawn as overstated.** "Without it a company
cannot investigate an incident" is too categorical. A company that owns the
hosts and the AI accounts already has the means to investigate what happens on
them — the host's unix account, the transcripts on its own disk, its provider's
audit trail, `session_events`' record of what fleet did and who asked. What an
in-app bypass adds is *convenience for the admin*, not the difference between
possible and impossible.

And the cost is not small: an audited, explicit, one-session-at-a-time override
is still access to a private session without the owner's consent. Once it exists,
"private" means "private unless someone with a role decides otherwise" — a
materially weaker promise than the one the brief makes, and not one to slip in as
a technical detail.

**What v1 does instead.** Say plainly, in `docs/hub.md`, where the boundary is
(§4.5): Fleet enforces the application boundary; everything outside it — the host
machine, the unix account, the AI provider — belongs to whoever operates it, and
a company that needs an investigative route has one there.

**If this is ever revisited**, it must be a deliberate change to the stated
privacy rule, with the owner's explicit yes, visible to every member before it
takes effect — not a capability that appears in a release note.
