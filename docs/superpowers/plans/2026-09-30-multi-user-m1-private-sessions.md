# M1 — two people on one hub, private sessions, explicit sharing

Companion to `docs/superpowers/specs/2026-09-30-multi-user-gap-analysis.md`.
Read §4 and §5 there first; this file is only the task list.

**Status:** not started, revision 6 (after the implementer pass, 2026-09-30).

Revision 2: ownership is always present, the migration no longer makes old
sessions org-visible, and T4a is new. Revision 3: grants move downward only,
sharing never confers a terminal, `unclaimed` surfaces as a count and is claimed
only with proof of host access, and device revocation is separated from grant
revocation. **Revision 4 replaced the whole T1–T7 breakdown** after a
thirteen-agent review (nine subsystem maps, three adversarial lenses, one
synthesis) audited revision 3 against the tree.

## Revision 6

An implementer lens read revision 5 as the person who has to write the code,
and found four places where a task could not be executed from the document.
The repo owner's delegate took four more binding decisions, **cited as R6-i …
R6-l** in the same convention. They close the last blockers. Revision 5's own
decisions, the eight rules, the definition of done, the scheduling rules and
the task list are unchanged; R6-j adds one tool and one event kind, named as an
addition below rather than left to look as though they were always there.

| Revision 5 said | Revision 6 says |
|---|---|
| **R6-i** — the pane proof is an argument on three tools (`whoami`, `register_self`, `session_claim`), backed by a durable `(host_alias, pane_id) → session_id` record that T12 builds | **The proof travels on the CONNECTION, and the durable record is deleted.** `service/provision.rs::merge_mcp_entry` writes `"X-Fleet-Pane": "${TMUX_PANE:-}"` beside the entry's `Authorization` header; `authorize` reads it, validates it exactly as `mcp::hooks::pane_header` validates the hook one, and puts it on `Caller` as `pane: Option<String>`. §4.4's clause 2 is then evaluable on **every** tool rather than on three, which is what the in-pane agent needed. No tool takes a pane argument, and the record — which, keyed by host alias, made every proven pane reachable by every agent on that host and so inverted DoD 9 — is not built |
| **R6-j** — `my_access` is a computed, per-caller field on `SessionRow`, stamped by `list_sessions` | **`my_access` is not a field on `SessionRow`.** The pipeline is hostile to one: the event bus serialises a bare `SessionRow` with no caller, `strip_nulls` removes an absent field on the way out, and the frontend row store replaces a row wholesale — so every routine `session:updated` would erase it, and on a paired desktop a fail-closed default would then shut the OWNER's own terminal. The row carries `owner_person_id` and `visibility`, which are caller-independent facts; each client holds its own person id and its own **grant set**, fetched once and kept current by its own event; the client DERIVES access from the three. `needs_attention` is not a precedent — it is not per-caller |
| **R6-k** — `people (id, name, display_name, created_at, …)`, with a literal ellipsis, and the personal owner found by name | **The DDL is written out in full, once, in T1, and the spec cites it.** `disabled_at` is required by three tasks and appeared in neither document. The personal owner is keyed by neither its name (explicitly renameable) nor "the lowest id" (fragile) but by `is_personal_owner INTEGER NOT NULL DEFAULT 0` under a partial unique index, so exactly one can exist and it survives a rename |
| **R6-l** — T4's ownership invariant calls `ViewScope::owns`, and `Access::Person` resolves the hub's personal owner by reading the store from inside the gate | **Two compile boundaries.** `store/session_grants.rs` compares `sessions.owner_person_id` to the caller's person id as a plain column read: `ViewScope` is a `service/` type built from a `Caller`, it does not exist until T6, and T4 lands before it. And `mcp/guard.rs::access_allows` stays store-free — it is shared with the store-free `mcp/tools/present.rs::visible_to`, which takes a `&Caller` and nothing else. "Is this caller the hub's personal owner?" is answered once, where the token is resolved, and travels on `Caller` as a boolean |

**What the pane header rests on, checked rather than assumed.** Revision 5 left
this as a premise. Claude Code expands `${VAR}` and `${VAR:-default}` inside an
MCP server entry's `headers` and `url`, with **no allow-list key required** —
`allowedEnvVars` is a HOOKS-only mechanism — and the blanking rule applies to
credential variables (`ANTHROPIC_API_KEY` and friends), of which `TMUX_PANE` is
not one. Note the **braced** form: the hooks entry's bare `$TMUX_PANE`
(`service/hooks_install.rs::hook_entry`) works only because that entry also
carries `"allowedEnvVars": ["TMUX_PANE"]`, which the MCP entry has no equivalent
of. Do not copy the hooks syntax. The `${TMUX_PANE:-}` spelling is already the
one the file's `command`-hook curl lines use.

**The one scope addition, from R6-j.** A client that derives its own access
needs its own person id and its own grant set, and today nothing serves either.
T12 adds the tool `my_grants` (`Client`, readonly) beside the sharing tools, T13
routes it as a desktop command beside `capture_session`, and T9 adds the event
kind `grant` with its one frame `grant:changed` — ids only, fenced to the
persons a grant actually names — so the set stays current without a re-fetch.
That is the whole cost of moving the per-caller answer off the row, and it buys
back the `my_access` field, the desktop's dependence on a field the row store
would erase, and the fail-closed default that would have shut the owner's own
terminal.

**The task chain was re-verified end to end after these moved.** Every Rust task
still ends where `cargo test -p fleet-core` is expected to pass, and no task
needs a symbol a later task creates. Three things moved and are called out at
the tasks themselves: `Caller::pane`, `Caller::is_personal_owner` and the
provisioning header are **T2's**, not T12's or the gate's, because T6 resolves
the pane into `ViewScope` and T7 reads it — leaving the header until T12 would
land two tasks whose host-token arm can never fire — and because
`mcp/guard.rs::access_allows` must stay store-free; T4's ownership check no
longer forward-references T6; and `my_grants` / `grant:changed` land in tasks
that already build their neighbours (T12 beside `session_access`, T9 beside the
fence). The task order is unchanged.

## Revision 5

A three-lens verification pass read revision 4 against the tree and against the
companion spec. It found seven review corrections that had not landed, 28
contradictions between the two documents or inside one of them, and 23 claims
the rewrite itself introduced that the code does not support. The repo owner's
delegate then took eight binding decisions, recorded here in the was →
corrected-to convention of the earlier revisions. **They are cited as R5-a …
R5-h** throughout this file, because this plan already carries its own lettered
decision list under *Decisions folded into the tasks* and the two sets would
otherwise collide — the same clash the work-graph roadmap settles by writing
"M14-D3x" and "Jev-D3x".

| Revision 4 said | Revision 5 says |
|---|---|
| **R5-a** — Rule 3: "two levels (watch / drive)"; decision (e), T7 and T12: three levels including `own` | **Two GRANTABLE levels, `watch` and `drive`.** `own` is not a level anyone can be granted: it is the set of operations only the owner may perform, and no grant reaches it. Its membership is defined in exactly one place — the spec's §4.3 invariant that names it — and decision (e), T7 and T12 CITE that place instead of restating a list. The three copies that existed disagreed with each other |
| **R5-b** — `visibility` admits `'private'`, `'org'` and `'unclaimed'`; F2 makes `'org'` a creation-time control; T6 stamps `my_access: … \| "org"` | **`sessions.visibility` has exactly two values in M1: `private` and `unclaimed`.** `'org'` is not a settable value, not a migration target, not a UI control and not a schema value; the CHECK constraint admits the two and nothing else |
| **R5-c** — DoD 5, T4's invariant 3 and T12's table promise an admin who may revoke or narrow a departed member's grants | **That authority moves to M2.** M1 has no membership, so it has no such authority to express and no task builds one. What M1 delivers is unchanged: a grant is downward-only, and only the owner creates one |
| **R5-d** — The unclaimed count is "served only to a caller who may already see that host" — a concept M1 does not have | **One person on the hub: the count is served to that person. More than one: it is served to NOBODY through the API in M1, and the operator reads it with `fleet-hub`.** The master token is not resolved to the personal owner for this, and "the operator" is never written where the code would mean "the master token" |
| **R5-e** — DoD 3/4 "B can watch it", with no desktop command a watcher can use | **SCOPE ADDITION, with its reason.** `capture_session` and `session_transcript` are MCP-only — neither is in `verdicts.rs`, confirmed against `src/lib/hub_verdicts.generated.json` — so "B can watch it" is unreachable on the desktop as things stand. M1 routes `capture_session` and builds a read-only pane view for a watcher. `session_transcript` is deliberately NOT added: the desktop's conversation panel already reads `session_conversation` / `session_conversations` / `session_tool_detail` / `session_activity` / `session_history`, all routed |
| **R5-f** — T10 (b) requires `new_session` to refuse a `resume_claude_session_id` "any row — live, lost or reaped — ever held under a different owner", and no task builds the record | **A durable `claude_session_id → owner` record is required, and migration 087 carries it.** The attack works precisely when the row is gone, so a check against live rows cannot close it |
| **R5-g** — Nothing touches `mcp/guard.rs::access_allows`'s `Access::Person` arm | **`Access::Person` must bind to the hub's personal owner, and T2a is its task.** Today it resolves through `Caller::is_person_device`, "a paired client bound to no org" — which with two people is ANY person, so the settings-write gate would let either of them write fleet settings |
| **R5-h** — Counts and bare line numbers throughout: "all 31 call sites", "all 33 non-test sites (29 + 4)", "all 17 `NewSessionArgs` literal sites", `support.rs:1430`, `events.rs:797-801` | **Precision policy.** Three independent verifiers produced three different numbers for the same query, and a dozen cited lines were off by one to twenty. This revision writes the QUERY, not its result, and anchors on `path/file.rs::function_name`, `migrations/0NN_name.sql` or a test name. A bare `:line` survives only where the anchor is a statement with no name, and then the line is described so a reader can re-find it if it moved |

Ten further corrections of fact, all verified against the tree at `77653006`:

- **`#[serde(default)]` on a `String` yields `""`, never `"unclaimed"`.** T3's
  fail-closed default is the field the whole `/events` fence keys on; it needs
  an explicit `#[serde(default = "…")]` naming a function, the convention
  already used at `store/orgs.rs` (`bound_sees_unassigned_default`),
  `store/trackers.rs` (`default_true`) and `store/work.rs` (`default_role`).
  The test that pins it is corrected with it.
- **`OrgScope::sees_session`'s Host arm has three unconditional wins, not two.**
  `service/orgs.rs::sees_session` is declared at `:137`; the predicate is the
  `if` at `:148`, `row_host == alias || row_org.is_none() || row_org == *org`.
  The third clause is the widest — a host token in org X reads every session of
  org X on every host in the fleet — and the fall-through below it,
  `!(theirs || mine)`, is permissive by default, so the arm is closer to `All`
  than revision 4 said. An implementer narrowing exactly the two named clauses
  leaves the leak.
- **`sessions.tmux_pane_id`'s provenance was mis-attributed, and the claim path
  rests on it.** See *The pane proof, and what it actually proves* below; this
  is the one correction that changes a design, not a citation.
- **The desktop's `session:killed` handling was cited at the `host:removed`
  arm.** The `"session:killed"` arm of `EventBridge::observe`
  (`src-tauri/src/backend/events.rs`) removes the id from `seen.sessions`, the
  resync bookkeeping set — not a store removal. The store removal is on the
  frontend, where `src/lib/events.ts` pushes `{ type: 'killed', id }` into
  `sessionEvents` for `applySessionEvents` to turn into `removeSession`.
- **`require_visible_session`'s call-site list omitted
  `rewind_conversation`'s** (`mcp/tools/lifecycle.rs`). A missed call site is a
  leak, so T7 states the query rather than a list.
- **T15's hub-e2e insertion point was after the hub is stopped.** `stop_hub a`
  precedes the line revision 4 named.
- `service/rewind.rs::RewindArgs` is the struct with `session_id`,
  `anchor_uuid`, `mode` and `new_worktree`; the line revision 4 cited is
  `pub async fn rewind_conversation`. The substantive claim — it carries no
  forker identity — holds.
- `store/schema.rs::client_tokens_has_org`, `mcp/guard.rs::NOT_FOR_HOST_TOKENS`,
  `mcp/events_route.rs::StreamState`, `crates/fleet-hub/src/main.rs::Cmd::Pair`
  and `mcp/tools/params.rs::PairClientParams` were each cited at a line inside
  or beside them rather than at their heads. All are now named anchors.
- **`src/lib/sessions.ts`'s optional-field precedents were cited at the wrong
  lines** (`stale_working_at?`, `row_version?`, `work_rev?`); `SessionRow`'s
  interface head was too. F1 now names the fields, not the lines.
- **`src/App.svelte`'s `<TerminalView />` mount** is in the `{:else}` of the
  `{#if $selectedSession && selNoPane}` branch, not at the line revision 4 gave.

Three things the review told revision 4 to do and it did not, now done:

- **`Backend::owns_the_fleet()`** (`src-tauri/src/backend/mod.rs`,
  `matches!(self, Backend::Local)`) is the existing, correct discriminator for
  "this process is the master" on the desktop side. F1's fail-closed default is
  answered against it instead of an invented two-armed "split by backend mode"
  that did not name `Backend::Unavailable`. (Revision 6 then moved what that
  default applies to: the derivation in `src/lib/access.ts`, not a field on the
  row — R6-j.)
- **Do not repeat a stale insert-site count in a migration header.**
  `migrations/045_participant_on_insert.sql`'s header still says `sessions` rows
  are inserted from three places; the production INSERTs are two. T1, T3 and T4
  carry the warning, and T3 corrects 045's header in passing.
- **`disable_person` revokes the grants TO that person as well as their
  tokens**, and the plan now says what becomes of the disabled person's own
  sessions.

### The pane proof, and what it actually proves

DoD 9 and T12 rest on `sessions.tmux_pane_id`. Revision 4 attributed the column
to the hook path, citing a line inside a `#[cfg(test)]` module in
`service/hooks.rs`. Established from the code instead:

- The production writer is the **reconcile pass**.
  `service/sessions/reconcile.rs` fills `tmux_pane_id` from `sess.pane_id`,
  `store/reconcile.rs` carries it into the `INSERT` column list and into
  `tmux_pane_id=COALESCE(excluded.tmux_pane_id, tmux_pane_id)` on conflict.
  `migrations/037_conversations.sql`'s own header says so: "the pane id (%17)
  **reconcile** last saw; hooks carry $TMUX_PANE in X-Fleet-Pane and **resolve
  by it**."
- `sess.pane_id` comes from `tmux.rs::list_local_sessions`'s
  `#{pane_id}` field, whose `TmuxSession::pane_id` doc says plainly: **"The
  session's active pane (`%N`)"**.

Four consequences, none of them cosmetic:

1. The column is **populated for the rows the claim path exists for.** An
   `unclaimed` row is created by the same reconcile INSERT that reads the pane,
   so it is never NULL for want of a fleet hook. Revision 4's sentence implied
   the opposite and would have read as "the claim path is unreachable".
2. It is the session's **active** pane, not any pane. An agent in a split
   window that is not the active pane cannot prove its own row, and an operator
   who switches the active pane invalidates the proof until the next reconcile
   pass. `COALESCE` never clears the column, so a stale value persists rather
   than becoming NULL.
3. `store/sessions.rs::find_session_by_pane` is already built for this read and
   is safe: it filters on `host_alias`, excludes ghosts, takes `LIMIT 2` and
   answers `None` on ambiguity.
4. **The proof is exactly as strong as §4.4's deployment rule and no stronger.**
   Any process that can run `tmux list-panes` on the host can enumerate every
   pane id there, so presenting one proves host access, not pane occupancy.
   **Corrected (fix round 3).** The rule the proof actually rests on is *one
   fleet host ALIAS per unix account*, hence one host token each — not "separate
   unix accounts". Nothing in the code binds `X-Fleet-Pane` to the requesting
   unix user: `mcp/hooks.rs::pane_header` validates only `%` plus up to ten
   digits, and `store/sessions.rs::find_session_by_pane` matches on
   `(host_alias, tmux_pane_id)` alone. So two unix accounts sharing ONE fleet
   alias share one token, pane ids are small sequential `%N`, and
   `require_person_sees` answers `E_PANE_UNPROVEN` for "exists on your host,
   wrong pane" against `E_NOTFOUND` otherwise — which makes a couple of hundred
   requests enough to enumerate every private session on that host. With one
   alias per account the other account's rows are not on this token's host at
   all and the enumeration has nothing to find. On a host with one shared
   account AND one alias the pane proof is guessable, not merely "as strong as
   host access". `Caller::pane`'s own doc (`mcp/auth.rs`) states the honest
   version; say it in `docs/hub.md` in these words too.

**How the proof reaches the hub (R6-i).** It is a request header, not a tool
argument, and there is no durable record of it anywhere:

- `service/provision.rs::merge_mcp_entry` gains
  `"X-Fleet-Pane": "${TMUX_PANE:-}"` beside its `Authorization` header, in the
  **braced** form (see the revision-6 note above: `allowedEnvVars` is a
  hooks-only mechanism and the bare `$TMUX_PANE` the hook entry uses would
  arrive unexpanded here).
- `mcp/mod.rs::authorize` — which reads only `Authorization` today, through
  `auth::check_request` — reads the header, validates it with the same rule
  `mcp::hooks::pane_header` already applies (`%` followed by digits, rejecting
  an unexpanded literal, an empty value and anything malformed), and stamps
  `Caller::pane` before inserting the caller into the request extensions.
- Because it is on the connection, §4.4's clause 2 — "the one row whose pane it
  can prove it is in" — is evaluable on **every** tool. That is the whole point:
  a per-tool argument reached three tools and left the in-pane agent of its own
  fleet-started (therefore `private`) session refused `dispatch_task`,
  `send_message`, `session_activity` and `work_link`.
- **There is no `(host_alias, pane_id) → session_id` record.** A record keyed by
  host alias makes every pane any agent ever proved reachable by every agent on
  that host, which is DoD 9 inverted. Each request resolves its own header
  through `store/sessions.rs::find_session_by_pane`, so the proof also lapses on
  its own the moment the reconcile pass rewrites `tmux_pane_id` — no
  invalidation logic, no table. (`store/mod.rs::set_controller` was never a
  candidate either: it writes the fleet-wide singleton settings
  `controller.host` / `controller.tmux`, one controller for the whole fleet.)
- **A host provisioned before M1 sends no header.** Its agent proves nothing and
  is refused — fail-closed, but **NOT visible, and `--content-only` does not fix
  it.** Established from the code during T2's repair:
  `service::provision::fingerprint` hashes four inputs — the two skills, the
  managed `CLAUDE.md` body and `hooks_install::hook_shape()` — and the
  `~/.claude.json` MCP entry is not one of them, so adding a header to that
  entry moves no fingerprint and raises no `provision_stale` on any host. (The
  hooks half *is* covered: `hook_shape()` embeds the session-start command,
  which carries `X-Fleet-Pane` already.) Folding the entry into the fingerprint
  was considered and rejected: `provision_content_only` both clears the mark and
  deliberately never rewrites `~/.claude.json`, so with one fingerprint column
  every option is wrong — clear it and the mark is raised and cleared within one
  hub start with the header still missing; don't clear it and every skills-only
  change becomes permanent stale noise; let a background sweep rewrite
  `~/.claude.json` and it races Claude Code, which writes that file itself.
  Telling the two apart needs **two** recorded fingerprints, i.e. a migration
  plus plumbing, and that is out of M1's scope by the owner's decision.
  **What M1 does instead:** the pane header arrives only with a FULL
  provisioning, so upgrading to M1 means hub, desktop **and a full
  re-provisioning of every host** — which the 6 → 7 contract bump already makes
  a coordinated upgrade anyway. D1 and `docs/hub.md`'s upgrade order say so in
  those words, and `fingerprint`'s own doc comment records the gap so nobody
  closes it by halves.
- **The real failure mode, and it needs its own error.** `tmux_pane_id` is the
  ACTIVE pane of the session's current window. An agent in a non-active pane of
  a hand-started multi-pane session presents a pane the row does not carry, so
  `find_session_by_pane` answers `None` and the claim is refused. That is safe
  but it is not "not found": the caller is on the right host and the row exists.
  `session_claim` answers `E_INVALID_STATE` naming the active-pane rule, so the
  operator reads "run the claim from the session's active pane" instead of
  hunting a row they can see in `fleet-hub session unclaimed`.

## Revision 4's corrections, kept

The thirteen-agent review returned 23 places where the two documents stated
something the code does not do, 40 leaks of which 16 are blockers, and three
independent verdicts that the milestone was not implementable as written. The
largest corrections, each of which invalidated at least one revision-3 task:

- **There is no create path.** `service::sessions::lifecycle` writes no session
  row. `new_session_inner` runs `tmux.new_session`, then `reconcile_one_host`,
  then *finds* the row; the row is made by
  `store/reconcile.rs::upsert_session_in_tx`, byte-for-byte the statement that
  creates a hand-started session. "The create path writes owner + private" and
  "reconcile's discovery path writes unclaimed" were one SQL statement. T5 is
  rebuilt around a reservation, a claim-if-unclaimed `ON CONFLICT` branch and a
  hard-failing backstop.
- **`person: None` was a privilege level above every person.**
  `mcp/auth.rs::Caller::org_scope` answers `OrgScope::All` for any client with
  no org, and revision 3 defined `person: None` as "keeps today's behaviour".
  With `--person` optional and no backfill, every device paired before the
  upgrade kept full-fleet read and anyone who can pair could mint a person-less
  device that reads every private session — the admin override DoD 2 forbids,
  with no attacker.
- **The visibility rule as written was `Option == Option`,** so `None == None`
  made every person-less caller the owner of every unclaimed row.
- **Four of the six choke points are no-ops for exactly the caller M1
  introduces** — an unbound paired client.
  `mcp/tools/support.rs::require_visible_session` returns `Ok(())` on its first
  line, `require_bound_client_sees` on its own early return,
  `service/orgs.rs::redact_work_via` is never called (`mcp/tools/mod.rs`'s
  `call_tool` wraps it in `if caller.is_scoped()`), and
  `mcp/events_route.rs::fence_frame` returns every payload verbatim. The events
  rescope machinery T4a planned to "extend" does not run at all for that caller.
  M1 is *turning the machinery on for a new class of caller*, not widening a
  predicate.
- **The choke-point list is longer than the six the spec's §5.1 names.**
  `list_worktrees`, the task tools, `send_message`'s recipient,
  `discover_lost_sessions`, `restore_host_sessions`, `usage_report`,
  `fleet_health`, `resolve_reader` and the work-graph reads all name or serve
  session content, and none of them is reached by the six. "The whole work graph
  is untouched" was the most dangerous sentence in revision 3. (The spec's §5.1
  and §5.2 lists are different lists that overlap only on `resolve_reader`;
  decision (g) below means §5.1's.)
- **Migration 087 as drafted bricks an upgrade.** Its inline `UPDATE sessions`
  is the exact thing `migrations/080_stale_demoted.sql` refused to do, for the
  reason recorded in that file's header.

**Three decisions the owner took after the review, binding on this plan:**

1. **Team sharing (`session_share { org }`) is out of M1**, deferred to M2. A
   client's org membership is written by `work_admin { assign_client }`
   (`service/orgs.rs`), which is `Access::Master` — an admin binds their own
   device to the org and reads every org-shared session, no grant touched, no
   owner consent. That defeats rule 2. The `org_id` column stays in the table
   for M2; the store refuses an org recipient today.
2. **Grants are dropped when a session moves.** `move_session` creates a new row
   with a new id; grants keyed on the old id are revoked, not carried. The owner
   re-grants if they want to. Narrowing is the safe direction.
3. **`CONTRACT_REVISION` goes 6 → 7**, with no mixed window — hub and desktop
   upgrade together, per `docs/hub.md`'s existing rule.

## The eight rules M1 must satisfy

Unchanged from the spec, restated here because every task is judged against
them.

1. A session started through fleet is private to its owner by default.
2. Privacy holds against the company admin too. No admin override, audited or
   otherwise.
3. Sharing is explicit, revocable, **two grantable levels (watch / drive)**,
   never transitive. `own` is not a grantable level — it is the set of
   operations only the owner may perform, and no grant reaches it. Its
   membership is defined once, in the spec's §4.3 invariant that names it;
   nothing in this file restates that list.
4. A grant moves downward only: revoke or narrow. Never widen, add a recipient,
   or redirect one.
5. Sharing never confers a terminal.
6. An unclaimed session leaks no metadata — only a per-host count, and only
   where R5-d says it is served at all. Claiming needs **proof that the
   claimant is in the session's pane**, never host access on its own and never
   org membership. (Revision 4's "proof of host access" was the stale
   formulation; DoD 9, decision (c) and T12 have said the pane since revision 4,
   and the header above says exactly how much the pane proves.)
7. The upgrade must widen nothing.
8. Revoking a device and revoking a grant are different events with different
   mechanisms.

**Monotonic narrowing.** Every task below either leaves access exactly as it is
today or narrows it. No task widens access. An incomplete sequence is therefore
an incomplete feature, never a regression — which is what makes it safe to land
the sixteen tasks that compile Rust one at a time over several days.

## Definition of done

1. Person A does not see person B's private sessions — in `list_sessions`, in a
   session-addressed read, in `/events`, and in the desktop UI.
2. An org admin does not get their content by being an admin — **and there is no
   path by which they can**, audited or otherwise (spec Q11).
3. A shares one session with B, watch-only.
4. B can watch it and cannot send prompts into it — and **no terminal reaches
   it**. Corrected in revision 4: *there is no Attach action to withhold.* The
   terminal is a pane that attaches automatically the moment a row is selected
   (`src/App.svelte` mounts `<TerminalView />` in the `{:else}` of its
   `{#if $selectedSession && selNoPane}` branch →
   `TerminalView.svelte::openTerm` → its `invoke('pty_open')`), so the gate is a
   mount condition plus an early return in `openTerm` before its
   `repair_session` probe, plus an active `pty_close` when the derived access
   stops being `own`. **Corrected in revision 6 (R6-j):** "the derived access"
   is computed on the client from the row's `owner_person_id` and `visibility`,
   the client's own person id and its own grant set — it is not a field on the
   row. The hub enforces independently and is unaffected by the derivation; the
   terminal gate is client-side because the PTY bypasses the hub entirely, which
   is a property of the terminal, not a weakening of the model.
   Two more surfaces are part of this criterion: SessionDetails'
   "Attach from another terminal" section, which hands a watcher
   `tmux attach -t <tmux_name>` in a copy button, and `upload_to_session`, the
   pane's drop handler, which scps files onto the owner's host with no hub in
   the path. (If B independently has SSH to that host, that is B's machine
   access; sharing neither created it nor claims to revoke it.)
   **Corrected in revision 5 (R5-e):** what B *does* see in place of the
   terminal is a read-only pane view backed by the newly routed
   `capture_session`, plus the conversation panel's existing routed reads. Until
   that command exists this criterion is unreachable on the desktop, and a
   stream-only or MCP-only acceptance run would pass with a watcher looking at
   an empty row.
5. Nobody but A can create a grant on A's session, and no call raises a level or
   redirects a recipient. **Corrected in revision 5 (R5-c):** M1 gives
   an admin no authority over A's grants at all. Revoking or narrowing a
   departed member's grants arrives with memberships in M2; M1 has no membership
   and so has no such authority to express.
6. A revokes the share; B loses access. "Loses" means, precisely: refused on the
   next request; dropped from an open `/events` stream within one 15 s
   keep-alive beat; re-checked on every wake of a long poll already in flight
   and again before it returns. **Corrected in revision 4:** a prompt
   `run_prompt` has already delivered into the owner's pane is *not* recalled —
   its order is permit → `deliver_prompt` → wait → transcript, so a pre-return
   re-check can withhold the transcript and cannot un-send the prompt. The
   attached terminal is outside this. Both exceptions are written into
   `docs/hub.md`, not only here. (The digest refers to this item as DoD 5.)

   **As built (T11).** The re-check is `service::tasks::AccessRecheck`, a
   `&dyn` predicate the MCP layer supplies (`SessionRecheck` / `TaskRecheck`
   in `mcp/tools/support.rs`, over the one copy of the gate —
   `person_sees` / `task_visible_at`), called by every wait inside the SAME
   lock window that reads the row and once more by
   `FleetTools::recheck_now` immediately before the payload is built. Of the
   five `Deadline::LongPoll` rows in `TOOL_POLICIES`, four are session-bound
   and carry it: `wait_for_session`, `wait_for_reply`, `wait_for_task` and
   `run_prompt`. The fifth, `add_project`, waits on a clone and names no
   session, so there is no grant behind it to re-check. `service::tasks::
   NoRecheck` is the named waiver, for fleet's own move engine only; a
   source-level test (`no_long_poll_tool_waives_its_access_recheck`) fails
   the build if anything under `mcp/` reaches for it.

   **Still open, recorded not fixed:** `mcp/tools/support.rs::
   long_poll_permit` buckets on `caller.label()` — a *device* name, not a
   person — so a revoked device keeps holding its
   `MAX_LONG_POLLS_PER_CALLER` slots for the remaining `LONG_POLL_CAP`
   (660 s). Those slots serve nothing: every wait behind them now refuses.
   It is therefore a small self-denial-of-service window, not a leak — and
   a standing reason never to key a future per-person quota on `label()`.
7. An existing single-user install upgrades with no registration and no lost
   sessions, and **no session becomes readable by anyone who could not read it
   before** — including after a colleague is added later.
8. An `unclaimed` session leaks no metadata **to any PERSON** — only a
   per-host count; a per-host TOKEN reaches the unclaimed rows of its own host,
   full row included, which is spec §4.4 clause 1 and what makes the claim path
   reachable at all (`ViewScope::sees_session_row`'s `Some(alias)` arm returns
   `RowAndContent` for an unclaimed row). The earlier wording — "no name,
   project, prompt, note or tag reaches anyone" — contradicted §4.4 and the
   code follows §4.4, so an acceptance run against it could not tell the
   feature from the bug. **Corrected in revision 5 (R5-d):** on a hub with
   exactly one person a per-host count is served to that person; on a hub with
   more than one it is served to nobody through the API in M1, and the operator
   reads it with `fleet-hub session unclaimed`. The count's wire carrier is
   `HostRow.unclaimed_sessions` — revision 3 assigned it none — and it is
   `None`, not `0`, whenever the API does not serve it.
9. A Claude on a shared host cannot read another person's private session
   through the per-host token (spec §4.4). The mechanism is the pane proof
   (`$TMUX_PANE` against `sessions.tmux_pane_id`), not the machine: a host token
   sees rows on its own host that are `unclaimed`, plus **the one row whose pane
   the current request proves**, and nothing else. **Corrected in revision 6
   (R6-i):** the proof rides the connection as `X-Fleet-Pane` and is resolved
   per request, so the clause holds on every tool and there is no stored proof
   that outlives the request — a record keyed by host alias would have made
   every pane any agent proved reachable by every agent on that host, which is
   this criterion inverted. See *The pane proof, and what it actually proves*
   above for the strength of that proof and the deployment rule it depends on.
10. `docs/hub.md` states plainly what privacy does and does not cover
    (spec §4.5): the hub operator reads the database, a host's unix owner reads
    its transcripts, the terminal attaches over SSH outside the hub, and there is
    no admin override inside the app.
11. An owner who has lost every paired device can still recover administrative
    access, and the procedure is written down (spec Q1).

## Scheduling rules

These shape the whole list; read them before picking up a task.

- **Only one Rust build may run at a time on the target machine.** The Rust
  tasks are executed strictly serially, one agent at a time. A second
  `cargo` invocation thrashes the machine and neither finishes.
- **Frontend tasks use pnpm**, a different toolchain, and may overlap with a
  Rust build. F1 can start on day one, beside T1.
- **Authors do not compile.** A task's author writes the change and hands it
  over; a dedicated integrator compiles, runs the tests and fixes the
  fallout. This is why every task below lists the tests that will break: the
  integrator must be able to tell an expected break from a real one.
- **No git-worktree isolation for Rust work.** There is no shared
  `CARGO_TARGET_DIR`, so a second worktree means a cold full build. Rust tasks
  land on one branch in one tree.
- **Each Rust task must end where `cargo test -p fleet-core` is expected to
  pass.** A task that leaves the workspace red is not finished, and the
  monotonic-narrowing invariant is what makes that achievable: a half-applied
  scope narrows, it never widens.
- **No task may need a symbol a later task creates**, and the chain was
  re-checked against that rule after revision 6 moved three things. The four
  places it is load-bearing, each stated at its own task: T4 compares
  `sessions.owner_person_id` as a column rather than calling `ViewScope::owns`,
  which T6 creates two tasks later (R6-l); T2a reads a boolean on `Caller` that
  T2 puts there, so the gate needs no store (R6-l); `Caller::pane` and the
  provisioning header are T2's rather than T12's, so T6 has something to resolve
  and T7's host-token arm can actually fire (R6-i); and T9's
  `announce_grant_change` needs `store/session_grants.rs`, which T4 built.
  Order is unchanged. **F1 is the one task whose runtime dependency runs
  forward** — it calls `my_grants` (T12/T13) and subscribes to `grant:changed`
  (T9) — and it is safe anyway: pnpm tests mock `invoke`, the events.ts →
  Rust cross-check runs in one direction only (`frontend_declares_every_event_name`
  asserts that every Rust name is declared, not the reverse), and F1's
  derivation answers `own` on the local backend before either exists.
- **Generated artifacts are regenerated in the task that causes them to drift**,
  never in a follow-up. Three of the regenerators write the file and then panic
  on purpose so the diff is read: run, read `git diff`, unset the variable,
  re-run clean. Under the one-build rule the regen loop, not the compile,
  dominates the wall clock — budget for it.
- **Migration numbers are decided once, here, and never renumbered:** 086
  people (T1), 087 session ownership (T3), 088 session grants (T4).
  `store/schema.rs::migrations_are_contiguous_from_one` must hold at every
  commit, so the tasks land in that order and no placeholder migration exists.

## Decisions folded into the tasks

Recorded here so no implementer re-derives them; the spec carries the long form.

- **(a)** `person: None` is never a session-visible scope built from a token.
  The master resolves to the hub's personal owner; a per-host token to a
  host-agent scope; a token with no person to a refusing scope. Hub internals —
  GC, reconcile, playbooks, attention, the `fleet-hub` CLI — do not build a
  `Caller` at all and keep unscoped store access, which is all spec §3.3
  actually requires.
- **(b)** The ownership predicate is
  `matches!((row.owner_person_id, scope.person), (Some(o), Some(p)) if o == p)`,
  written once in `service/` as `ViewScope::owns` and called from the rule
  table, `may_drive`, `may_own` and all three sharing tools. Never
  `Option == Option`. **R6-l:** the store's own check, in
  `store/session_grants.rs`, is the same shape written against the column —
  `sessions.owner_person_id = ?person` with a NULL person refused — not a call
  to `ViewScope::owns`. `ViewScope` is a `service/` type built from a `Caller`
  and T6 creates it two tasks after T4; the store compares the column it
  already holds, and `ViewScope::owns` reads the `SessionRow` the store
  produced.
- **(c)** `unclaimed` is **count-only where it is served at all** (R5-d
  below). Revision 3's "row only, content refused" arm is struck, and so is
  T6's one-click claim: on `/events` there is no middle option, because a
  `session:created` frame *is* the row and *is* its content. The claim path is
  the in-pane agent with its host token proving `$TMUX_PANE` against
  `sessions.tmux_pane_id`, plus `fleet-hub session claim`. **R6-i:** the
  proof is the `X-Fleet-Pane` request header, resolved per request and never
  stored; no tool takes a pane argument.
- **(d)** There is no create path in `lifecycle`; see T5.
- **(e)** `own` is **not a grantable level**. It names the operations only the
  owner may perform, and no grant reaches it. **Its membership is the spec's
  §4.3 invariant that names it — the single authority. Nothing in this file
  restates the list**, because the three copies revision 4 carried (spec Q6,
  this decision, T7) already disagreed with each other on whether a `drive`
  grantee may kill, restart or rename the owner's session. T7 threads
  `Reach::Own` for exactly the tools that invariant names and cites it by
  section number at the point of use.
- **(f)** Org-scoped grants are out of M1 (owner's decision 1), and
  `visibility = 'org'` is out of M1 with them (R5-b); see
  *Why `'org'` is not a value in M1* below.
- **(g)** The choke points are the spec's §5.1 list, which is longer than six;
  the extra surfaces are named in T7 and T10. Do not quote a number for it: the
  spec's §5.1 and §5.2 lists are different lists that overlap on
  `resolve_reader` alone, and revision 4 used one sentence's number for the
  other's list.
- **(h)** `CONTRACT_REVISION` → 7, deliberately with no mixed window
  (owner's decision 3).
- **(i)** **Do not quote counts.** Revision 4 carried corrected counts of
  desktop commands, `TOOL_POLICIES` rows and `SessionRow` fields; three
  verifiers then produced three different numbers for one of them, and a count
  is wrong the moment anyone edits the file. Where a number matters, name the
  generator that prints it: `src-tauri/src/backend/verdict_gen.rs` for the
  command buckets, `src-tauri/src/backend/hub_contract.golden.json` for the
  wire keys, `mcp/tools/tests.rs::every_router_tool_has_exactly_one_tool_policy_row`
  for the router. Corrected anchors that revision 4 got wrong and this revision
  keeps: `resolve_row_and_gate` is `mcp/tools/support.rs::resolve_row_and_gate`
  and `resolve_and_gate` is the wrapper above it; the policy test's real name is
  `every_router_tool_has_exactly_one_tool_policy_row` (the name cited in two
  `mcp/guard.rs` comments does not exist — one of those comments names no test
  at all).
- **(j)** **Access is DERIVED on the client; nothing per-caller rides the row**
  (R6-j). `SessionRow` carries `owner_person_id` and `visibility`, both
  caller-independent facts about the row itself. A client learns its own person
  id and its own grant set once and keeps them current by their own event, and
  computes watch / drive / own locally. The three reasons a per-caller field
  cannot work here are structural, not stylistic: `BroadcastEventBus::emit`
  serialises a bare `SessionRow` with no caller in scope; `strip_nulls` removes
  an absent key on the way out, so "absent" is not a signal; and
  `src/lib/row_store.ts::createRowStore` replaces a held row wholesale on every
  merge, so a routine `session:updated` would erase the field and the next
  render would fall to its default — which, fail-closed on a paired desktop,
  shuts the OWNER's own terminal. `needs_attention` is not a precedent for the
  shape: it is the same answer for every caller. **Say it where it could be
  misread:** the derivation is for the UI. The hub enforces independently, on
  every request, and is unaffected by what the client computed. The terminal
  gate is client-side because the PTY reaches the host over SSH with no hub in
  the path — a property of the terminal, not a weakening of the model.

### Why `'org'` is not a value in M1

Owner's decision 1 removed `session_share { org }` because an org grant names a
recipient *set* whose membership somebody else writes:
`work_admin { assign_client }` is `Access::Master`, so an admin binds their own
device to the org and reads every org-shared session with no grant touched and
no owner consent. `visibility = 'org'` is that same capability under another
name. M1 has no membership table, so "the org can see it" resolves through
`client_tokens.org_id` — written by that same `Access::Master` tool — and the
hole reopens with the admin on the inside of it. Revision 4 nevertheless kept
`'org'` in the DDL comment, in `my_access`, and as a one-click control in F2's
NewSessionDialog, while no rule anywhere said who may read such a row and no
Rust task implemented one. It is therefore removed from M1 completely: not a
settable value, not a migration target, not a UI control, not a schema value.
The CHECK constraint admits `'private'` and `'unclaimed'` and nothing else, so a
future value cannot appear by accident. `'org'` returns in M2, with memberships.

## Tasks

Twenty-one tasks: two docs (T0, done, and D1); sixteen that compile Rust (T1–T15
plus T2a, of which T13 is frontend as well); and three frontend-only (F1–F3).
Twenty remain. Each is one PR unless it says otherwise; every PR runs
`scripts/ci-local.sh`.

### T0 — Correct the two documents — **done**

**Toolchain** docs · **Depends on** — · **Parallel-safe with** everything

Revision 4 of this file and of
`docs/superpowers/specs/2026-09-30-multi-user-gap-analysis.md` was T0, and
revision 5 of both is its verification pass. It is recorded here rather than
listed as future work. Everything below assumes the corrected design.

---

### T1 — Migration 086: `people`, `client_tokens.person_id`, the hub's personal owner

**Toolchain** rust · **Depends on** T0 · **Parallel-safe with** F1, F2

**Files** `crates/fleet-core/migrations/086_people.sql`,
`store/schema.rs`, `store/mod.rs`, `store/people.rs` (new), `store/clients.rs`,
`store/rows.rs`, `mcp/settings.rs`, `crates/fleet-hub/src/serve.rs`

**Do.** Write `086_people.sql`. **The DDL in full (R6-k)** — revision 5 wrote it
with a literal ellipsis and omitted `disabled_at`, which three tasks require:

```sql
CREATE TABLE IF NOT EXISTS people (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  name              TEXT    NOT NULL,
  display_name      TEXT,
  is_personal_owner INTEGER NOT NULL DEFAULT 0,
  created_at        INTEGER NOT NULL,
  disabled_at       INTEGER
);

-- One live person per name, exactly as `idx_client_tokens_live_name`
-- (migration 032) does it for devices: `name` is not UNIQUE outright,
-- because a disabled person keeps their row (grants and sessions still
-- point at it) and must not hold the name against a new colleague.
CREATE UNIQUE INDEX IF NOT EXISTS idx_people_live_name
  ON people(name) WHERE disabled_at IS NULL;

-- Exactly one personal owner, enforced by the schema rather than by a
-- convention about ids. The same partial-index device, on a flag rather
-- than on a NULL.
CREATE UNIQUE INDEX IF NOT EXISTS idx_people_personal_owner
  ON people(is_personal_owner) WHERE is_personal_owner = 1;

ALTER TABLE client_tokens ADD COLUMN person_id INTEGER;
```

`client_tokens.person_id` has **no foreign key** — the migration-066 rationale:
deleting a person must leave the token bound to an id nothing has, never widen
it. `people` likewise carries no FK to anything.

**Why the flag and not the name or the id.** The personal owner is found on
every master request, so it must be one indexed read; the name is explicitly
renameable through `store/people.rs`, so keying on `'owner'` breaks the first
time the user types their own; and "the lowest id" is a convention a single
`INSERT` in a fixture or a future merge can violate silently. The flag survives
a rename, the partial unique index makes a second one an error at write time,
and `disabled_at` is deliberately **not** part of that index: the hub's owner
being disabled must not free the slot for a second one.

**The header must not repeat a count.**
`migrations/045_participant_on_insert.sql`'s header says `sessions` rows are
inserted from three places; the production INSERTs are two, and the header has
been stale ever since. Write 086's header, and 087's and 088's, so that nothing
in it goes stale when a call site is added: name the statements, not how many
there are.

The auth-epoch trigger is a **separate narrow companion**, not an edit to
migration 060: 060's `auth_epoch_client_tokens_update` is a fixed
`WHEN OLD.x IS NOT NEW.x` list already installed in every live database and
SQLite has no `ALTER TRIGGER`. The precedents are
`auth_epoch_client_tokens_org` (`migrations/066_work_view.sql`) and
`auth_epoch_client_tokens_assets_admin`
(`migrations/074_client_assets_admin.sql`):

```sql
CREATE TRIGGER IF NOT EXISTS auth_epoch_client_tokens_person
AFTER UPDATE OF person_id ON client_tokens
WHEN OLD.person_id IS NOT NEW.person_id
BEGIN UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1; END;
```

Insert the hub's personal owner with the placeholder name `owner` and
`is_personal_owner = 1` — **`fleet.self` does not exist**; a grep over
`crates/`, `src/` and `docs/` finds the string only in revision 3 of this file.
The only fleet-identity setting is `fleet.id` (`service/address.rs`), a lazily
minted UUID, and a migration cannot read a hostname. The name is renameable
through `store/people.rs`; the flag is not.

Then **backfill the tokens**:
`UPDATE client_tokens SET person_id = <that id> WHERE person_id IS NULL AND revoked_at IS NULL`
— no pre-existing device survives the upgrade person-less, which is half of the
fix for the `person: None` privilege level.

Register in `store/schema.rs`'s `MIGRATIONS` table with the struct-literal
`already_applied` guard form and a new `fn client_tokens_has_person(conn)`
checking `pragma_table_info('client_tokens') WHERE name='person_id'`, modelled
on `store/schema.rs::client_tokens_has_org` — the `ALTER TABLE` is not
idempotent.

New `store/people.rs`: `create_person`, `get_person`, `get_person_by_name`,
`list_people`, `disable_person`, `set_client_person`, `personal_owner_id`.
`set_client_person` follows `store/clients.rs::set_client_org`: trim the name,
refuse `mode='peer'` and `mode='updater'`, `E_NOTFOUND` on no live row.
`create_person` never sets `is_personal_owner`; the flag is written once, by
the migration and by `ensure_personal_owner` below, and no public function
moves it.

**`personal_owner_id()` exactly.** Signature
`pub fn personal_owner_id(&self) -> Result<Option<i64>, IpcError>`; body
`SELECT id FROM people WHERE is_personal_owner = 1` through
`query_row` / `optional()`, one index seek on `idx_people_personal_owner`. It
returns `Some(id)` when the row exists and **`None` when it does not** — it
never falls back to the lowest id, to the only live person, or to any other
row. Every caller treats `None` as fail-closed and says so in its own code:
`Caller::view_scope` builds the **refusing** scope for a master with no
personal owner (no person scope resolves, so no session is visible rather than
all of them), `owner_for` yields `None` so the create paths leave the row
`unclaimed` rather than attributing it to a guess, and `Access::Person` refuses
(T2a). That state is unreachable in practice — `ensure_personal_owner` runs at
every `fleet-hub` entry point the master token is minted at — and is specified
anyway because the alternative to a defined answer is an invented one.

**`disable_person` is compound, not a flag.** It revokes every `client_tokens`
row bound to that person, which bumps `auth_epoch`. Nothing anywhere reads
`people.disabled_at`, and `Store::active_client_tokens` filters only on
`client_tokens.revoked_at`, so a bare `disabled_at` would make "disable a
person" a no-op that merely frees the name. **T4 adds the second half** — every
live grant *to* that person is revoked in the same transaction — because
`store/session_grants.rs` does not exist until then; T1 leaves a `// T4:` marker
at the call site and T4's green list carries the assertion.

**What becomes of a disabled person's own sessions:** nothing. Their rows stay
`private` and owned by them, unreadable by anyone else, exactly as the spec's Q9
answer for a departure says. Disabling a person removes their reach; it never
re-attributes their work, and M1 has no operation that does. Say this in the
`disable_person` doc comment and pin it with a test — it is the half of the
spec's Q2 revocation table that revision 4 left unanswered.

Add `person_id` to `ClientTokenRow` (`store/rows.rs`), to `map_client_token_row`,
and to **every** hard-coded ten-column `SELECT` literal in `store/clients.rs` —
grep the column list rather than working from a count; they do not all live in
one function.

Add `ensure_personal_owner(&Store)` beside `mcp/settings.rs::ensure_master_token`
and call it from every entry point `ensure_master_token` is called from
(`crates/fleet-hub/src/serve.rs`: `init`, `token` and `serve`), because a store
opened outside `fleet-hub init` must also have one, and no single entry point is
guaranteed to run first.

**Green at the end.** A fresh database has exactly one person and it carries
`is_personal_owner = 1`; `ensure_personal_owner` run twice creates one row; a
second `INSERT` with `is_personal_owner = 1` fails on the partial unique index;
`personal_owner_id` still answers after the owner is renamed, answers `None` on
a database whose flagged row has been removed by hand, and never falls back to
another row; after `migrate()` no live `client_tokens` row has
`person_id IS NULL`; a new
`migration_086_on_a_populated_v85_database_is_safe_to_rerun` copying
`store/schema.rs::migration_063_on_a_populated_v62_database_is_safe_to_rerun`; a
new `binding_a_client_to_a_person_bumps_the_auth_epoch` beside
`store/schema.rs::rebinding_a_client_bumps_the_auth_epoch`, covering "the same
binding again is no change" and "a disabled person leaves the client bound to an
id nothing has"; `a_disabled_persons_own_sessions_stay_private_and_theirs`.

**Breaks, update in this task.**
`store/schema.rs::the_token_tables_columns_are_the_ones_the_auth_epoch_triggers_know`
— extend its expected column array with `"person_id"` plus a comment naming the
new trigger; do **not** extend its `AUTH_EPOCH_TRIGGERS` constant, which is
deliberately only 060's.
`store/read_pool.rs::the_auth_epoch_moves_on_every_token_write_and_not_on_a_touch`
— add a `set_client_person` arm. `store/schema.rs`'s
`migrations_are_contiguous_from_one` and `every_migration_records_its_own_version`
must stay green.

---

### T2 — What the connection carries: `ClientRef`, `Caller::person`, `Caller::pane`, pairing and the CLI

**Toolchain** rust · **Depends on** T1 · **Parallel-safe with** F1, F2

> **Ordering constraint — T2 and T2a ship as one PR, and the CLI lands last.**
> The revision-6 gate found the one leak window M1 creates for itself: T2 makes
> a second person *possible* (`fleet-hub pair --person`) while `Access::Person`
> still resolves as "any person's device", so between the two tasks a second
> person's device reaches `get_settings` / `set_setting` for the whole fleet.
> T2a is kept a separate task because it is a distinct, reviewable hole — but
> it must not be a separate *merge*. Within the combined boundary the order is:
> the caller plumbing below, then **T2a's binding**, then the `fleet-hub pair
> --person` / `client bind-person` CLI. Written that way there is no commit at
> which a second person can exist and the settings gate is unbound.

**Files** `mcp/auth.rs`, `mcp/mod.rs`, `mcp/hooks.rs`, `mcp/pairing.rs`,
`mcp/token_cache.rs`, `mcp/events_route.rs`, `mcp/tools/fleet.rs`,
`mcp/tools/params.rs`, `mcp/tools/support.rs`, `mcp/tools/tests.rs`,
`service/provision.rs`, `crates/fleet-hub/src/main.rs`,
`crates/fleet-hub/src/pair.rs`, `docs/control-api-reference.md`

**Do.** `mcp/auth.rs::ClientRef` gains `person_id: Option<i64>`;
`resolve_token` copies it from the cached row. Mechanical, but every `Caller` /
`ClientRef` literal in that module's tests needs the field. Add
`Caller::person() -> Option<i64>` and, next to `mcp/tools/fleet.rs`'s
`settings_actor`, `owner_for(caller, &Store) -> Option<i64>` mapping the master
to `Store::personal_owner_id` — and yielding `None`, never a substitute, when
that answers `None` (T1).

**Two booleans and one string the caller carries, not the gate.**

- `Caller::is_personal_owner: bool`, resolved where the token is resolved: true
  for the master, and for a client whose `person_id` equals the hub's personal
  owner. R6-l — `mcp/guard.rs::access_allows` must stay store-free, because it
  is shared with `mcp/tools/present.rs::visible_to`, which takes a `&Caller` and
  nothing else and runs over the whole router on every served list. T2a is the
  task that reads it.

  **Where the id comes from without a lock per request.**
  `mcp/auth.rs::check_request` takes token *rows*, not a `Store`, and
  `mcp/mod.rs::authorize` must not take the writer's lock on every request to
  answer one question. Carry the id beside the rows: `mcp/token_cache.rs`
  already serves `list_host_tokens` / `active_client_tokens` off a read-only
  connection per request, so it reads `Store::personal_owner_id()` on the same
  pass and hands it to `check_request` as one more argument. Cache it after the
  first successful read — the flagged row is written once, by the migration and
  by `ensure_personal_owner`, and `store/people.rs` moves the flag from
  nothing — and **never cache a `None`**: a hub that answers `None` is
  mis-provisioned, must keep asking, and fails closed meanwhile (T1). The
  no-cache path (the desktop, a loopback hub) already takes the lock in
  `authorize` and reads it there.
- **`Caller::pane: Option<String>`** — the pane proof, on the connection
  (R6-i). `mcp/mod.rs::authorize` today reads only `Authorization`, through
  `auth::check_request`. Add the header read there, after the caller is built
  and before `request.extensions_mut().insert(caller)`: take `X-Fleet-Pane` and
  validate it with the **same rule** `mcp::hooks::pane_header` applies — `%`
  followed by ASCII digits, bounded length, rejecting an unexpanded literal, an
  empty value and anything malformed. Call `mcp::hooks::pane_header` itself
  rather than writing a second validator; it already takes a `&HeaderMap` and
  its test `pane_header_accepts_only_tmux_pane_ids` is the pin. The field is
  set for every caller that sends the header and is `None` otherwise; nothing
  in T2 reads it (T6 resolves it into the scope, T7 gates on it, T12 claims
  with it), and a `Caller` field that nothing reads yet still compiles.
- **The header has to be provisioned.** `service/provision.rs::merge_mcp_entry`
  writes the `claude-fleet` MCP entry's `headers` object; add
  `"X-Fleet-Pane": "${TMUX_PANE:-}"` beside `Authorization`, in the braced form
  (the revision-6 note says why the hooks entry's bare `$TMUX_PANE` would
  arrive unexpanded here). Its two existing tests assert
  `mcpServers.claude-fleet.headers.Authorization`; give the new key a sibling
  assertion in both, including the merge-over-existing-file one. A host
  provisioned before this change sends no header, proves no pane and is
  refused — fail-closed, but **not surfaced**: `service::provision::fingerprint`
  does not cover the `~/.claude.json` MCP entry, so nothing reads
  `provision_stale` and `--content-only` would not help if it did (see the
  pane-proof bullet near the top of this plan for why that is not fixed here).
  A FULL provisioning is what adds the header, and D1 makes re-provisioning
  every host part of the M1 upgrade.

Thread `--person <name>` through every pairing site that already threads
`--org`, in lockstep with it. Find them by following `--org` / `org_id` from
`crates/fleet-hub/src/main.rs::Cmd::Pair` outward; as of this writing the chain
is `Cmd::Pair` → `crates/fleet-hub/src/pair.rs::pair`'s args →
`mcp/tools/params.rs::PairClientParams` → `mcp/tools/fleet.rs::pair_client`'s
validation block (refuse `--person` for `mode=peer|updater` exactly as `trusted`
and `org_id` are refused) → `mcp/pairing.rs::PendingPairings::mint_bound`
(convert its positionals to a `MintRequest` struct rather than adding one more)
→ `mcp/pairing.rs::Pending` → `mcp/pairing.rs::PairingRequest` and its
hand-written `Debug` impl (a person name is not a secret and may print) →
`mcp/pairing.rs::handle_pair`'s apply chain as one more `.and_then`, plus its
response JSON. Let the compiler enumerate; do not work from a count.

**The code carries a name, not an id.** The person row is created at
*redemption*, not at mint: `PendingPairings` is in-process memory and an
abandoned code must not leave an orphan `people` row. Validate the name's shape
at mint so the operator sees the error at the terminal.

**`pair_client` with no `--person` defaults to the hub's personal owner** — a
person-less client token must be unmintable. That is the other half of the fix
for the `person: None` privilege level.

`fleet-hub client bind-person <client> <person>` / `unbind-person <client>` go
in `crates/fleet-hub/src/main.rs::ClientCmd` but must **not** copy
`crates/fleet-hub/src/pair.rs`'s `client bind` implementation, which routes
through `work_admin { assign_client }` — an org-graph tool whose actions are
enumerated by the isolation matrix. Use a direct store write through
`open_store`, the `client grant assets` precedent in the same file. Grow
`pair.rs::client_table`'s fixed-width row array by one with a PERSON column, in
all four places it is spelled (cells type, header literal, width map, line
closure).

Extend `mcp/events_route.rs::client_is_live` to compare `(org_id, person_id)`
through a new `Store::client_token_binding(id)`: a device re-bound to another
person must drop its stream exactly as an org re-bind does. This is the
**device** mechanism and must not learn about grants (rule 8).

Then `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.

**Green at the end.** Two devices paired with the same `--person` resolve to one
person id; a revoked token resolves to nothing; a code minted for a new person
name creates the row only on redemption; a request carrying a well-formed
`X-Fleet-Pane` lands it on `Caller::pane` and one carrying the unexpanded
literal `${TMUX_PANE:-}`, an empty value or `%` alone lands `None`;
`merge_mcp_entry` writes the braced header and preserves every other key.

**Breaks, update in this task.**
`mcp/auth.rs::resolve_token_maps_master_and_host_tokens_to_callers` and every
whole-`Caller` literal in that module; `mcp/pairing.rs::a_code_carries_its_trust_grant`
and `each_code_carries_its_own_name_and_mode` — add a sibling for the person
slot; `mcp/pairing.rs::debug_never_prints_the_code`;
`crates/fleet-hub/src/pair.rs::the_client_table_shows_every_column_and_never_a_digest`
and `the_table_lines_up_for_wide_and_zero_width_names`;
`service/provision.rs`'s two `merge_mcp_entry` tests, which assert the headers
object;
`mcp/doc_gen.rs::reference_is_current` after the regen; and
**`mcp/tools/tests.rs::the_served_definition_budget_stays_bounded`**, whose
`BUDGET_BYTES` constant sits within a few hundred bytes of the current
measurement, so the new `person` parameter's schema text will very likely trip
it. Raise the constant to the number the failing run prints and put the reason in
the commit message, per the convention written into that test.

---

### T2a — `Access::Person` binds to the hub's personal owner

**Toolchain** rust · **Depends on** T2 · **Parallel-safe with** F1, F2 ·
**Ships in T2's PR, before T2's CLI half** (see T2's ordering constraint)

**Files** `mcp/auth.rs`, `mcp/guard.rs`, `mcp/tools/fleet.rs`,
`mcp/tools/tests.rs`

**Do.** R5-g. `mcp/guard.rs::access_allows` answers
`Some(Access::Person) => caller.is_master() || caller.is_person_device()`, and
`mcp/auth.rs::Caller::is_person_device` is "a paired client bound to no org, and
not a hub link". With one person on the hub that reads as "the owner's own
device". **With two it reads as *any* person**, and `Access::Person` is the gate
on `get_settings` / `set_setting` — the fleet's settings, with `settings_writer`
in `mcp/tools/fleet.rs` above it for writes. The moment M1 pairs a second
person, that person's device reaches the whole fleet's settings. This is small,
it is not speculative, and it is a hole M1 creates rather than inherits, so it
has its own task rather than riding inside T2's mechanical sweep.

Bind it: `Access::Person` is satisfied by the master, or by a person device
whose `person_id` is `Store::personal_owner_id()`. Everything else — a second
person's device, an org-bound client, a per-host token — is refused, with the
same `E_FORBIDDEN` shape the gate already produces. Leave `Access::PersonDevice`
alone: it is the narrower gate and already excludes the master by design.

**The gate stays store-free** (R6-l). `access_allows` takes a `&Caller` and a
tool name; it is shared with `mcp/tools/present.rs::visible_to`, which has no
store and runs over the whole router on every list. Do not give either a
`&Store`, and do not resolve the personal owner inside the arm: read T2's
`Caller::is_personal_owner`, which is answered once where the token is resolved
and where the store rows are already in hand. The arm becomes
`Some(Access::Person) => caller.is_personal_owner && (caller.is_master() || caller.is_person_device())` —
one boolean, no I/O, and no lock taken on a path that runs per request. When
`Store::personal_owner_id()` answered `None` at resolution time the boolean is
false and the gate refuses, which is T1's fail-closed rule held here.

Do **not** widen the fix into per-person settings. "Whose settings are these?"
is an M2 question that needs memberships; M1's answer is that the fleet's
settings belong to the fleet's owner, which is the state a single-person hub is
already in.

**Green at the end.** A second person's device is refused `get_settings` and
`set_setting`; the personal owner's device and the master are not; a per-host
token and an org-bound client are refused as before;
`mcp/tools/tests.rs`'s access-matrix assertions carry a two-person row; a
source scan asserting `mcp/guard.rs` names no store type, so the next edit
cannot reintroduce the read.

**Breaks, update in this task.** Every `Access::Person` case in
`mcp/tools/tests.rs` that builds an unbound client and expects it to pass —
those callers now need to be the personal owner.

---

### T3 — Migration 087: `sessions.owner_person_id` / `visibility`, the conversation-owner record, the trigger re-issue, the Rust backfill

**Toolchain** rust · **Depends on** T2a · **Parallel-safe with** F1, F2

*Swapped ahead of grants relative to the recon digest: `session_grants`' owner
invariant reads `sessions.owner_person_id`, so this column must exist first.
Numbering follows execution order, so no placeholder migration is needed.*

**Files** `crates/fleet-core/migrations/087_session_owner.sql`,
`store/schema.rs`, `store/rows.rs`, `store/sessions.rs`,
`store/scale_fixture.rs`, `store/schema/tests_upgrade.rs`,
`src-tauri/src/backend/hub_contract.golden.json`,
`src-tauri/src/backend/tests_contract.rs`

**Do.** The script adds the two columns, `idx_sessions_owner`, the
conversation-owner record below, and **re-issues `sessions_row_version_bump` in
full** — copy the trigger body from `migrations/082_result_evidence.sql`
verbatim and add `OR NEW.owner_person_id IS NOT OLD.owner_person_id` and
`OR NEW.visibility IS NOT OLD.visibility`. Both are client-visible `SessionRow`
fields, so they are watched, never listed in `store/schema.rs`'s
`ROW_VERSION_UNWATCHED`. Without the re-issue,
`store/schema.rs::the_sessions_columns_are_the_ones_the_row_version_trigger_knows`
fails — the single most likely first CI failure of this work.

**`visibility` takes exactly two values** (R5-b):

```sql
ALTER TABLE sessions ADD COLUMN visibility TEXT NOT NULL DEFAULT 'unclaimed'
  CHECK (visibility IN ('private', 'unclaimed'));
```

There is no `'org'`. The CHECK is the point: it makes a future third value a
migration, not an accident, and it makes the "no migration path produces an
`'org'` row" assertion unnecessary rather than merely true.

**The script contains no `UPDATE sessions`.** `migrations/080_stale_demoted.sql`
documents exactly why: any `UPDATE` compiles the row-version trigger, which
names `lost_reason`, a column that on a conversations-branch database exists
only after `repair_skipped_main_migrations()`, which
`store/schema.rs::migrate` runs *after* all pending migrations. Revision 3's
inline `UPDATE` aborts with `no such column: lost_reason` and bricks that
upgrade.

Instead add `Store::backfill_session_owner()`, called beside
`store/schema.rs::backfill_stale_demoted`'s call site in `migrate`. Idempotent —
after the first run it matches no row. Attribute to the single person only when
`(SELECT COUNT(*) FROM people) = 1`, **and add `AND started_at IS NOT NULL`**:
`started_at` is documented "when fleet created the session (NULL for
tmux-discovered rows)" in `store/rows.rs` and is the better heuristic. A
hand-started tmux session on a shared host is precisely the row spec §4.3's
table wants left `unclaimed`. **The companion spec's Q10 asks for a chain test
asserting that "every session" carries the person after the upgrade; that is
wrong and §4.3's own table contradicts it. The assertion this task writes is
"every session with `started_at IS NOT NULL`".**

Guard the migration with a single `fn sessions_has_visibility(conn)` on the
**last** `ADD COLUMN`, the `work_items_has_status_set_at` convention.

**The durable `claude_session_id → owner` record** (R5-f). T10 (b)
requires `new_session` to refuse a `resume_claude_session_id` that any row —
live, lost or **reaped** — ever held under a different owner. A check against
live rows cannot close that: the attack works precisely when the row is gone,
`Store::delete_session` deletes the `sessions` row outright, and `conversations`
is `ON DELETE CASCADE` on it (`migrations/037_conversations.sql`), so a reaped
row's conversation id goes with it. Add a table that outlives the session:

```sql
CREATE TABLE IF NOT EXISTS conversation_owners (
  claude_session_id TEXT PRIMARY KEY,
  owner_person_id   INTEGER NOT NULL,
  first_seen_at     INTEGER NOT NULL
);
```

No foreign key to `sessions` (the row must survive its session) and none to
`people` (the 066 rationale again). Fill it from a **trigger on `sessions`**,
not from a Rust call site: `claude_session_id` is written by
`store/reconcile.rs`'s upsert *and* by `store/sessions.rs::set_claude_session_id`,
and a future third writer must not be able to skip the record. An
`AFTER INSERT` and an `AFTER UPDATE OF claude_session_id, owner_person_id`
trigger that `INSERT OR IGNORE`s when both values are non-NULL is enough —
first writer wins, so a later re-attribution cannot overwrite the original
owner. The trigger names only columns 087 creates plus the new table, so it
compiles under the same rules the row-version trigger does.

Nothing in M1 deletes from this table. The retention question (a person is
deleted; an owner wants the record forgotten) is M2's, alongside
`store/work_retention.rs`'s windows; say so in the migration header rather than
leaving it to be discovered.

`SessionRow` gains `owner_person_id: Option<i64>` and `visibility: String`.
`owner_person_id` is `#[serde(default)]`. **`visibility` is
`#[serde(default = "visibility_unclaimed")]`, naming a function that returns
`"unclaimed".to_string()`** — a bare `#[serde(default)]` on a `String` yields
`String::default()`, the empty string, and the fence would then key on `""`.
This is the privacy-critical fail-closed path (spec §3.7's whole argument for
keying the fence on `visibility` rather than on the nullable
`owner_person_id`), and the repo's convention for exactly this is the named
default function — `store/orgs.rs::bound_sees_unassigned_default`,
`store/trackers.rs::default_true`, `store/work.rs::default_role`. Neither field
is `skip`: T12 needs them on the wire.

Add both to `store/rows.rs`'s `SESSION_COLUMNS` (after `pr_evidence`,
`pr_checked_at`) and to `map_session_row`; the doc comment above
`SESSION_COLUMNS` says a new column goes in exactly those two places. Give
`visibility` **no** `skip_serializing_if`: `BroadcastEventBus::emit` runs
`strip_nulls` before the frame enters the replay ring (`events.rs`), so a
nullable `owner_person_id` is absent from every unowned row's frame and is
indistinguishable from a pre-M1 build. `visibility` (NOT NULL) is the only key
the stream fence can safely read.

Seed both explicitly, with varied owners, in `store/scale_fixture.rs` so the M12
budget tests measure the new index against real cardinality rather than
identical NULLs. Leave `store/testgen.rs` alone — it deliberately rebuilds
v0.2.37's schema.

Extend `src-tauri/src/backend/tests_contract.rs::sample_session` with
**non-`None`** values for both — the invariant stated at the top of that file is
that no key may be missing for want of a value, and the privacy-critical field
must not join `pr_evidence` / `pr_checked_at` in escaping it. Then
`REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract`, read the diff,
unset, re-run clean. **This regen is the Tauri build and needs gtk/dbus.**

While you are in `migrations/`, correct
`migrations/045_participant_on_insert.sql`'s header, which still says `sessions`
rows are inserted from three places. Name the two production INSERTs instead of
counting them.

**Green at the end.** A new
`migration_087_on_a_populated_v86_database_is_safe_to_rerun`; a new test that is
the inverse of `store/rows.rs::stale_demoted_at_is_read_but_never_serialized` —
a `SessionRow` deserialised **with no `visibility` key at all** reads
`unclaimed`, which is what the named default function buys and what a bare
`#[serde(default)]` would fail; `an_insert_of_a_row_with_a_claude_session_id_and_an_owner_records_a_conversation_owner`
and `a_reaped_sessions_conversation_owner_survives_delete_session`;
`store/schema/tests_upgrade.rs`'s `an_older_build_refuses_a_newer_database` and
`the_downgrade_guard_admits_fresh_current_and_older_databases`;
`service/work/scale_tests.rs`' query-plan budgets against the varied fixture.

**Breaks, update in this task.**
`store/schema.rs::the_sessions_columns_are_the_ones_the_row_version_trigger_knows`.
`store/schema/tests_upgrade.rs::the_chain_from_pre_work_graph_to_latest_is_fast_complete_and_sound`
— its `SUM(row_version)` assertion must be updated **deliberately**: the
backfill runs after the trigger re-issue, so attributed rows do bump, and they
should — an open client holding a cached row must learn its visibility changed.
Quote the new sum and the reason in the test. Extend the same file with: after
the chain, `people` has one row, and every session with `started_at IS NOT NULL`
is owned by it and `private`. Do **not** add an assertion counting `'org'` rows;
the CHECK constraint makes the value unrepresentable, which is stronger.
`src-tauri/src/backend/tests_contract.rs::the_hubs_field_names_are_the_ones_the_desktop_reads`
after the regen.

---

### T4 — Migration 088 and `store/session_grants.rs`: grants, downward only, person recipients only

**Toolchain** rust · **Depends on** T3 · **Parallel-safe with** F1, F2

**Files** `crates/fleet-core/migrations/088_session_grants.sql`,
`store/schema.rs`, `store/mod.rs`, `store/session_grants.rs` (new),
`store/people.rs`, `store/rows.rs`, `store/sessions.rs`

**Do.** The table as revision 3 had it, with three corrections its SQL got
wrong:

1. **The unique index enforces nothing as written.** Exactly one of `person_id`
   / `org_id` is NULL by design and SQLite treats NULLs as distinct, so two live
   grants to the same person on the same session both insert — defeating
   invariant 3. Use the repo's own NULL-safe precedent,
   `ux_work_views_name ON work_views(COALESCE(owner_org, 0), name)`
   (`migrations/066_work_view.sql`):
   `CREATE UNIQUE INDEX IF NOT EXISTS idx_session_grants_live ON session_grants(session_id, COALESCE(person_id, 0), COALESCE(org_id, 0)) WHERE revoked_at IS NULL`.
2. Add `CHECK ((person_id IS NULL) <> (org_id IS NULL))`, so "exactly one of" is
   schema rather than a comment.
3. Add
   `CREATE INDEX idx_session_grants_person ON session_grants(person_id) WHERE revoked_at IS NULL AND person_id IS NOT NULL`
   — `grants_for_person` is the hot read, built once per request, and revision
   3's only index leads on `session_id`.

`store/session_grants.rs`: `grant`, `revoke`, `narrow`, `grants_for_session`,
`grants_for_person`, `revoke_all_for_person`, with the four invariants enforced
**in the store, not at call sites**:

1. `grant` refuses unless the caller person is the row's `owner_person_id`.
   **This is a column comparison, not a call to `ViewScope::owns`** (R6-l):
   `store/session_grants.rs` takes the granting person as an `i64` — never an
   `Option<i64>`, so the NULL-equality trap is not even expressible at the
   signature — and the statement's `WHERE` carries
   `sessions.owner_person_id = ?granter`, which SQL already answers `false` for
   a NULL column. `ViewScope` is a `service/` type built from a `Caller`, T6
   creates it two tasks later, and a store function that needed it would make
   this task unwritable in its own slot. The service-layer predicate of
   decision (b) is the same rule for the same reason and calls the store; the
   two are tested against each other in T6.
2. A grantee cannot grant: (1) already does it, and a test pins it.
3. **Downward only.** There is no `widen`. `narrow` accepts `drive` → `watch`
   and nothing else. `grant` on a session that already has a live grant for that
   recipient refuses rather than upgrading it. No function changes a live
   grant's `person_id` / `org_id` — a redirect would be a privacy bypass wearing
   a grant's clothes.
4. A grant confers no terminal; nothing about that is expressible in this table,
   so it is T7's and F1's job.

**Only the owner calls `grant`, `revoke` or `narrow` on their own session.** M1
gives no admin any authority over a grant (R5-c): there is no membership
to make "a departed member" mean anything, and inventing one here is how M2's
design gets pre-decided by a store function. `revoke_all_for_person` is the one
exception and it is not an authority over a grant — it is the person's own
disablement, below.

**Finish `disable_person`** (T1 left the marker). In the same transaction that
revokes the person's tokens, call `revoke_all_for_person` so every live grant
*to* them is revoked and `GRANT_GENERATION` bumps. Without it a disabled person
keeps every grant, and re-enabling — or a token minted for them by any path —
restores reach the operator believed they had removed. Their own sessions are
untouched: still `private`, still theirs (T1's doc comment and test).

**M1 ships person recipients only** (owner's decision 1): `grant` returns
`E_INVALID` for an `org_id` recipient. The column stays for M2; an org recipient
today delegates the recipient set to whoever writes `client_tokens.org_id`,
which is the master.

Add `GRANT_GENERATION: AtomicU64` and `grant_generation()` **here, in the store
module where the writes are**, not at a service call site —
`service/orgs.rs::ORG_GENERATION` has exactly one bump site and a write path
already misses it (`mcp/pairing.rs` calls `set_client_org` with no bump). Bump
it inside `grant`, `revoke`, `narrow` and `revoke_all_for_person`.

Pin the `ON DELETE CASCADE` with a test rather than trusting it: `sessions.id` is
a rowid alias (`migrations/001_init.sql`), SQLite reuses the highest deleted
value, and `store/sessions.rs::delete_session` hand-deletes `session_events`
rather than relying on a cascade.

**Green at the end.** `a_non_owner_cannot_grant`; `a_grantee_cannot_grant_on`;
`watch_never_becomes_drive_by_any_path` (grant, re-grant, narrow, and every
public helper);
`the_public_surface_of_session_grants_cannot_raise_a_level_or_change_a_recipient`,
which walks the module's public functions and fails on a new one that can;
`a_live_grant_to_the_same_recipient_is_refused_not_upgraded`, which proves the
`COALESCE` index actually fires;
`deleting_a_session_cascades_its_grants_and_a_reused_rowid_inherits_none`;
`an_org_recipient_is_refused_in_m1`;
`disabling_a_person_revokes_their_tokens_and_every_grant_to_them`;
`a_grant_change_bumps_GRANT_GENERATION_and_not_the_auth_epoch`, the mirror of
`store/read_pool.rs::the_auth_epoch_moves_on_every_token_write_and_not_on_a_touch`;
`migration_088_on_a_populated_database_is_safe_to_rerun`.

**Breaks, update in this task.** None expected beyond
`store/schema.rs::migrations_are_contiguous_from_one`.

---

### T5 — Ownership written on every path that creates a session row

**Toolchain** rust · **Depends on** T4 · **Parallel-safe with** F1, F2

**Files** `store/reconcile.rs`, `store/sessions.rs`,
`service/sessions/lifecycle.rs`, `service/sessions/review.rs`,
`service/bg_sessions.rs`, `service/move_session/mod.rs`, `service/rewind.rs`,
`service/trackers/tickets.rs`, `service/operator.rs`,
`service/catalog/author_session.rs`, `service/work/resume.rs`,
`mcp/tools/orchestration.rs`, `mcp/tools/session_ops.rs`,
`mcp/tools/lifecycle.rs`, `src-tauri/src/backend/remote.rs`,
`src-tauri/src/backend/tests_routing.rs`

**Do.** There is no create path to hook (correction (d)). Build the seam in
three parts.

> **Correction, after the T5 review: parts 1 and 2 were DELETED again.** The
> `OwnerIntent` map and the upsert's `owner_person_id` / `visibility` writes
> are gone, and `record_tmux_created` is back to taking no owner and sitting
> AFTER `tmux.new_session` with every other create site. The reason is
> structural, not a bug that was fixed: the only key available before the row
> exists is a tmux NAME, and a name is reused — so an intent filed against one
> claimed whatever row turned up under it (an existing `unclaimed` row, a live
> hand-started session fleet had no row for yet), and two concurrent creates of
> one name cross-stamped its single slot. Three successive guards each moved
> the hole instead of closing it.
>
> What stands is part 3's claim alone: every row the reconcile upsert inserts
> is `unclaimed`, and `Store::claim_if_unclaimed` — keyed on the ROW ID,
> refusing another person's row — is the one mechanism that stamps an owner.
> The window that opens between the insert and the claim is accepted and
> written down at `finalize_new_session`; `new_session` also refuses a name
> that already has a row (`reject_adoptable_session_name`), so the create fails
> rather than adopting a row it did not cause. The long note on
> `store/reconcile.rs` carries the whole argument, and
> `no_create_path_reserves_an_owner_for_a_tmux_name` keeps the mechanism from
> coming back.

1. **An `OwnerIntent(Mutex<HashMap<(String,String),(i64,i64)>>)` on `Store`**,
   copied verbatim from `store/reconcile.rs::KillMemory`: the same
   `KILL_MEMORY_SECS` reasoning, pruned on every access, deliberately in memory
   and not a table — a second writer on the reconcile hot path is the thing to
   avoid. Read it in `apply_host_reconcile_in_tx` exactly as
   `let kills = self.recent_kills(spec.alias)` is read there, and pass it into
   the upsert as a new parameter.
2. **Claim-if-unclaimed in the upsert SQL** in
   `store/reconcile.rs::upsert_session_in_tx`: add `owner_person_id` /
   `visibility` to the `INSERT` column list *and* to the
   `ON CONFLICT DO UPDATE` branch —
   `owner_person_id = COALESCE(owner_person_id, ?N)` and
   `visibility = CASE WHEN owner_person_id IS NULL AND ?N IS NOT NULL THEN 'private' ELSE visibility END`,
   never re-owning. This is required, not optional: `reconcile_one_host`'s own
   doc comment (`service/sessions/reconcile.rs`) says it is ungated against the
   background pass, so the background tick can insert the row between
   `tmux.new_session` and `new_session`'s own reconcile, and an INSERT-only
   write loses that race.
3. **`service/sessions/lifecycle.rs::record_tmux_created`** gains an
   `owner: Option<i64>` and **moves to before `tmux.new_session`**, so no window
   exists. `finalize_new_session` in the same file gains
   `s.claim_if_unclaimed(row_id, owner)` as a **hard failure** — unlike every
   soft-failing neighbour, because an unstamped row is `unclaimed` and therefore
   unreadable by the person who just started it.

Carry the owner on `service/sessions/lifecycle.rs::NewSessionArgs` as
`owner_person_id: Option<i64>` with `#[serde(skip_deserializing)]` — a hub
client must not be able to forge an owner over the wire.
`src-tauri/src/backend/remote.rs::HubBackend::new_session` already spells its
fields out one by one; keep it that way and assert in
`src-tauri/src/backend/tests_routing.rs` that `owner_person_id` is absent from
the wire.

Work through **every `NewSessionArgs { … }` literal site outside tests**
deliberately rather than pasting `None` — `grep -rn 'NewSessionArgs {'` over
`crates/fleet-core/src` and `src-tauri/src`, skipping the `_tests.rs` files, is
the enumeration. As of this writing the production sites are the two in
`mcp/tools/session_ops.rs` (fill from `owner_for(caller)`), the one in
`mcp/tools/orchestration.rs` (`dispatch_task`'s worker: from the requester row's
owner), and those in `service/rewind.rs`, `service/work/resume.rs`,
`service/trackers/tickets.rs`, `service/catalog/author_session.rs` and
`service/operator.rs` (from the source row's owner or the hub's personal
owner).

**`spawn_review` is a create path revision 3 never listed**
(`service/sessions/review.rs`): inherit `source.owner_person_id` /
`visibility` in the same block that calls `set_session_kind`. The review of a
private session is as private as the session, and the source row is the
authority, so no `Caller` is needed.

**`move_session`** (`service/move_session/mod.rs`, the block whose writes are
each commented "Soft-fail like new_session: the session is live either way")
carries owner and visibility to the target row as a **hard failure, outside
that block** — a soft-failed carry turns a private session `unclaimed` on the
target, making a move a silent privacy event. **Grants are dropped on a move**
(owner's decision 2): they are keyed on the old `sessions.id`, a grant is on a
row and not on a session's identity, and narrowing is the safe direction. Say so
in the code comment, and test it.

**`rewind_conversation`'s fork inherits the source's owner, not the forker's.**
Revision 3's rule is not expressible — `service/rewind.rs::RewindArgs` carries
`session_id`, `anchor_uuid`, `mode` and `new_worktree` and no forker identity —
and source-inheritance is also the safer answer, since a fork the forker owned
would be an un-revocable copy. The spec's §4.3 `own` invariant already refuses a
fork of a session you do not own.

**`restart_session`'s `None` branch** (`service/sessions/lifecycle.rs`, the arm
that creates a live tmux session under a name with no row) must let it land
`unclaimed` rather than silently attribute it.
**`service/bg_sessions.rs::stamp_bg_row`** claims the bg row post hoc — the
intent map cannot key on a claude id the caller does not yet know.

Finally, **`service/sessions/lifecycle.rs::reject_lost_session_name`** must
refuse a name matching *any* lost row on that host whose `owner_person_id`
differs from the caller's person. Today
`store/sessions.rs::lost_resumable_session_named` refuses only rows with a
`claude_session_id`, so a lost shell row is resurrected by the
`ON CONFLICT DO UPDATE` with its old owner intact and person B's new session
comes up owned by and readable to person A. That is DoD 7 violated by a live
code path, independent of the migration.

Note that `recreate_session` and `rename_session` are **not** create paths —
`service/sessions/lifecycle.rs` keeps the row through `restore_session` and says
so in its own comment, and the rename arm carries the row over before the
reconcile precisely so the pass does not insert one. Both preserve the owner for
free. (They are still owner-only operations; that is the spec's §4.3 invariant,
threaded in T7, not a create-path question.)

**Green at the end.** A background full pass that inserts the row before
`new_session`'s own reconcile still yields an owned, `private` row — drive it
through `service/sessions/reconcile.rs::reconcile_one_host_with_for_test`;
`spawn_review` inherits the source's owner and visibility; a `move_session`
whose owner-carry write fails aborts rather than leaving the target `unclaimed`
(the move module already has failure-injection hooks); a move revokes the source
row's grants; a lost row with `claude_session_id IS NULL` is refused to a
different person's `new_session`; `service/repair.rs`'s `repair_session` never
produces an ownerless row.

**Breaks, update in this task.**
`store/reconcile.rs::upsert_session_in_tx_identical_row_pushes_no_change` — the
long positional call breaks; extend it to assert a second identical pass does
not re-claim an already-owned row. The `new_session` routing case in
`src-tauri/src/backend/tests_routing.rs` — the struct literal breaks, which is
the point; add the assertion that `owner_person_id` is absent from the asserted
JSON. `service/bg_sessions.rs::stamp_bg_row_names_and_stamps_the_reconciled_row`,
plus a sibling asserting a resurrected bg row keeps its **original** owner.

---

### T6 — `ViewScope`: the type, the one constructor, the rename sweep, and `list_sessions`

**Toolchain** rust · **Depends on** T5 · **Parallel-safe with** F1, F2

**Files** `service/orgs.rs`, `service/orgs_tests.rs`, `service/view_scope.rs`
(new), `mcp/auth.rs`, `mcp/tools/session_ops.rs`, `service/hosts.rs`,
`store/sessions.rs`, `store/rows.rs`,
`src-tauri/src/backend/hub_contract.golden.json`,
`src-tauri/src/backend/tests_contract.rs`

**Do.** New `service/view_scope.rs`:

```rust
pub struct ViewScope {
    pub org: OrgScope,
    pub person: Option<i64>,
    pub grants: GrantSet,        // BTreeMap-backed: canonical ordering
    pub host: Option<String>,
    pub proven_session: Option<i64>,  // the row THIS request's pane proves
}
```

`GrantSet` is backed by a `BTreeMap` so `ViewScope` derives a meaningful
`PartialEq` — the stream compares scopes in `mcp/events_route.rs`'s keep-alive
rescope and its pre-frame generation check, and a `HashSet` would drop streams
spuriously.

**`proven_session` is one row, not a set** (R6-i). The pane proof rides the
connection as `X-Fleet-Pane` (T2's `Caller::pane`), so a request proves at most
the one pane it is running in. `Caller::view_scope` resolves it here, once per
request: `Caller::pane` and `host_alias` together through
`store/sessions.rs::find_session_by_pane`, which already filters on
`host_alias`, excludes ghosts, takes `LIMIT 2` and answers `None` on ambiguity.
Nothing is stored — a stored `(host_alias, pane_id) → session_id` record would
make every pane any agent ever proved reachable by every agent on that host,
which is DoD 9 inverted — and nothing has to be invalidated: the next reconcile
pass rewrites `tmux_pane_id` and the next request resolves to `None` on its own.

Methods: `owns(&SessionRow) -> bool` in the `matches!` form of decision (b);
`sees_session_row(&SessionRow) -> Visibility` returning the two-valued
`{ None, RowAndContent }` — **there is deliberately no "row only" arm**, because
on `/events` a `session:created` frame *is* the row and *is* its content, so
`unclaimed` can only be dropped everywhere; `may_drive(&SessionRow)`;
`may_own(&SessionRow)`; and `ViewScope::internal()` for the hub's own readers.

`Caller::view_scope(&Store)` is the **one** constructor, beside (not replacing)
`org_scope`:

| Caller | `person` | `host` | Sees |
|---|---|---|---|
| master | `Store::personal_owner_id()` | — | its own rows, grants, org rows — and **a refusing scope** if that answers `None` (T1's fail-closed rule), never an unscoped one |
| client with `person_id` | that person | — | as above |
| client with no `person_id` | — | — | **a refusing scope** (T1's backfill and T2's default make this unreachable; it must fail closed if it is not) |
| per-host token | `None` | `Some(alias)` | rows on its own host that are `unclaimed`, plus the one row this request's `X-Fleet-Pane` proves — and nothing else |

**Narrow `OrgScope::sees_session`'s host arm in the same edit, and narrow all
three of its unconditional wins.** `service/orgs.rs::sees_session` is declared
at `:137`; its Host arm is the `if` at `:148`, which reads
`row_host == alias || row_org.is_none() || row_org == *org`. Revision 4 named
two of those clauses and would have left the third, which is the widest: it lets
a host token in org X read every session of org X on **every** host in the
fleet. The fall-through below it, `!(theirs || mine)`, is also permissive by
default, so the arm as a whole is closer to `All` than either document said.
Re-derive the arm from the rule a host token is supposed to have — its own
host's rows, and nothing else — rather than deleting clauses one at a time.

Hub internals do not build a `Caller` and keep unscoped store access — that is
what spec §3.3 actually requires, and it is why the master no longer needs to be
`All`.

**The rename sweep is what makes the rest of this sequence compiler-forced.**
Rename `OrgScope::sees_row` → `sees_row_org_only` and `OrgScope::sees_session` →
`sees_session_org_only` at **every call site the compiler names**, with no
behaviour change, so every site is named and triaged. Do not work from a count:
three verifiers produced three different totals for this query, and two of the
sites are inside `OrgScope`'s own impl. T10 **deletes** both, which fails to
compile if any was missed.

Split `service/orgs_tests.rs`'s table tests into an `OrgScope` half, with its
work assertions kept, and a new `ViewScope` half.

Then apply **choke point 1**: `mcp/tools/session_ops.rs::list_sessions` reads
`caller.view_scope` **off the same handle the rows come from** — `p.force` uses
the writer, else `self.reader()`; a scope read from the other handle races a
fresh grant. Filter with `sees_session_row` and drop `unclaimed` and
other-person rows entirely, **before** `fresh_for` hashes the page, which is
correct today and must stay so.

**`list_sessions` stamps nothing on the row** (R6-j). Revision 5 had it compute
a per-caller `my_access` field here; there is no such field. The rows it returns
carry `owner_person_id` and `visibility` — facts about the row, identical for
every caller — and a client derives watch / drive / own from those plus its own
person id and its own grant set (F1). Three reasons the per-caller field cannot
live on the row, all structural: `BroadcastEventBus::emit` serialises a bare
`SessionRow` with no caller in scope, so every `session:updated` would carry
somebody else's answer or none; `strip_nulls` removes an absent key on the way
out, so "absent" is not a signal a client can read; and
`src/lib/row_store.ts::createRowStore` replaces a held row wholesale on merge,
so one routine update erases the field and the next render falls to its
default — which, fail-closed on a paired desktop, shuts the **owner's** own
terminal. Enforcement is unaffected: the hub answers every request against the
scope it built for that request, and nothing about the derivation is trusted.

**The unclaimed count's wire carrier**, which revision 3 assigned to nothing:
add `unclaimed_sessions: Option<i64>` to `HostRow` (`#[serde(default)]`),
computed in `service::hosts::list_hosts` from a new
`Store::unclaimed_counts_by_host()`. **R5-d governs who gets it:**

- `(SELECT COUNT(*) FROM people WHERE disabled_at IS NULL) = 1` **and** the
  caller is that person (or the master, which resolves to them): serve the
  count.
- Otherwise: serve `None`. Not `0` — `0` is a claim about the host that a
  caller who may not know is not entitled to.

Do **not** invent a host-administration concept to widen this, and do not write
"the operator" anywhere the code means "the master token": they are different
callers, and conflating them is how the `'org'` class of hole gets in. On a hub
with more than one person the count reaches a human only through
`fleet-hub session unclaimed` (T12), which is shell access on the hub machine.

**Single-person installs keep their rows, not only a count.** With exactly one
person on the hub, an `unclaimed` row is private to nobody and that person could
already see it before the upgrade, so `sees_session_row` admits it for them and
the Outside-fleet and orphan sections of the sidebar
(`src/lib/sidebar_index.ts::buildOutsideFleet`, `Sidebar.svelte`'s
`outside-fleet-section`) keep their content. Without this the upgrade empties
both on every standalone desktop, because `started_at IS NULL` for every
reconcile-discovered row and T3's backfill deliberately leaves them
`unclaimed` — a regression neither document acknowledged, and one D1 promises
does not happen. This follows R5-d's one-person / many-person split and
rule 7 (the upgrade widens nothing: the one person saw these rows yesterday).
**Flagged for the owner:** R5-d settled the *count*; this extends the
same split to the rows, and it is the only reading under which D1's "nothing
changed for a single user" is true.

Then `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` for
`HostRow.unclaimed_sessions`, with a non-`None` sample value. `SessionRow` gains
nothing here — its two new keys landed with T3.

**Green at the end.** A `view_scope.rs` table test whose **first row** is
`(owner: None, person: None) => Visibility::None`, so the NULL-equality trap is
pinned by name; a test that `ViewScope::owns` and
`store/session_grants.rs`'s column comparison answer alike on the same row for
an owner, a non-owner and an ownerless row (R6-l: two implementations of one
rule, in two layers that cannot call each other in this direction);
`only_caller_view_scope_constructs_a_view_scope`, a source scan
asserting no other module builds one — revision 3 promised this and
`mcp/tools/fleet.rs::usage_report` is today's counter-example for the org scope;
`list_sessions` for person B omits A's private row and returns no `unclaimed`
row, and **no row it returns carries a per-caller key** (a serialised page
compared for two different callers is byte-identical where the rows are the
same); a standalone desktop (master, one person) sees every row, including its
`unclaimed` ones; a two-person hub serves `unclaimed_sessions: None` to both; a
per-host token sees `unclaimed` rows on its own host, not another person's
private row, not an unassigned row on another host, and not an org-mate's row on
another host; a host token whose request carries the `X-Fleet-Pane` of a private
row on its own host sees that row and no other private row, and the same token
with no header sees none.

**Breaks, update in this task.**
`src-tauri/src/backend/tests_contract.rs::the_hubs_field_names_are_the_ones_the_desktop_reads`
after the regen.
`mcp/tools/tests_isolation.rs::the_isolation_matrix_holds_with_sessions_shared`
and `..._with_org_b_isolating_sessions` — the fixture inserts sessions directly
and its rows become `unclaimed` under 087's default, which changes what every
existing row expects; give the fixture owners.

---

### T7 — Choke point 2: one session gate, with read / drive / owner-only

> **Handoff from the frontend chain (recorded 2026-10-01, after F2a–F2c).**
> The desktop now carries its own tier table, `src/lib/share.ts::SESSION_TIER`,
> and a source-scanning test (`share_sweep.test.ts`) that fails when a control
> reaches a tiered command without composing the access half. **The two tables
> must agree.** Where they disagree the desktop disables a control the hub then
> allows — or, worse, offers one the hub refuses, which is the raw-`E_FORBIDDEN`
> outcome `share.ts` exists to remove.
>
> Rows the frontend added that this task must mirror, with the tier decided from
> the existing table's own reasoning:
>
> | Action | Tier | Why |
> |---|---|---|
> | `tidy_apply` | `own` | it can safe-kill, and `safe_kill_session` is `own` |
> | `request_work_handover` | `drive` | it types into the pane, like `send_message { deliver, submit }` |
> | `set_primary_work` | `drive` | a per-session work-graph write, like `link_session_work` |
> | `decide_work_batch` | `drive` | a batch of `confirm_session_work`, already `drive` |
> | `resume_work` | `own` | it takes over a conversation, like `rewind_conversation` |
> | `reconsider_work_link` | `drive` | per-session work-graph write |
> | `ack_work_link` | `drive` | same family |
> | `resolve_move` | `own` | Finish and Undo of a partial move each KILL a live session, so it cannot be narrower than `move_session`; judged against the TARGET session's row, which is the id the command is given (added F2d) |
>
> Two findings from the same reviews that belong to the BACKEND, not the
> desktop, and have no task of their own yet:
>
> - **`fleet_health` and `usage_report` branch on `caller.is_scoped()` rather
>   than on a scope value**, so a person's device may be served fleet-wide
>   session counts and per-session cost over everyone's private sessions. The
>   spec's §5.2 closing paragraph names this; T10 narrows it, and it needs to be
>   narrowed to the caller's visible set, not merely to their org.
> - **The `repo_*` writes** (`FilesPanel`'s checkout, commit, delete-branch,
>   stage, push) are real writes into the owner's worktree and have no access
>   gate of any kind on the desktop. They were judged acceptable only because of
>   their routing verdict; confirm that judgement here, because if any of them is
>   reachable for a grantee it is a write to another person's working tree.
>
> **Wire-shape asks (added after F2c).** These are the one thing that would undo
> a real narrowing the desktop had to make. `WorkLink`, `ResumeCandidate` and an
> ENDED `WorkTaskLink` carry no `session_id` and no owner, so a paired desktop
> can only resolve a resume's source session by matching `snap_host` +
> `snap_tmux` against rows it already holds — and otherwise refuses. That is the
> safe reading, but it narrows a working feature for every hub client, owner
> included. Carrying `session_id` (or an owner person id) on those rows undoes
> it. Relatedly: if `work { task }` really omits `session_id` on ended links (the
> type comment says "absent when ended"), populate it — `WorkTaskDetail`'s
> Continue is gated on exactly that field. And the hub's `recent_ended` read is
> ORG-scoped rather than person-scoped, so a past link it returns can be someone
> else's; person-scoping it would remove the need for the desktop's fail-closed
> guess entirely.
>
> **What failing closed costs, recorded after F2d.** F2d deleted four
> hand-rolled "a row I cannot resolve ⇒ allowed" hatches (`TidyReview`,
> `WorkReview`, `moves.ts`, `TasksPanel`) and routed all four through
> `share.ts::sessionIdActionBlocked`, which refuses on a fleet this client does
> not own and allows on a standalone one. A paired desktop therefore now loses,
> and each of these is undone by a wire shape rather than by a wider gate:
>
> - **Tidy up** skips a candidate whose session row the client does not hold.
>   `TidyCandidate` carries `session_id`, host and tmux name and no owner —
>   carrying the owner person id (or having the hub narrow `work { tidy }` to the
>   caller's visible set, which is the better fix) restores it.
> - **Review** skips an item whose session row it does not hold; same shape, and
>   `work { review }` is the read to narrow.
> - **Tasks** refuses a cancel when a NAMED party does not resolve. `$tasks` is
>   the fleet-wide `list_tasks`, so this is routine on a paired desktop: either
>   narrow `list_tasks` to tasks whose parties the caller can see, or carry each
>   party's owner on `TaskRow`.
> - **Transfer** refuses Finish/Undo when the TARGET row is not held, and the
>   move lifecycle (retry, cancel-wait, resolve) refuses once the source row has
>   left the store. `move:progress` / `session_move_partial` carry ids only.
> - **Resume and Summarise** need more than F2c asked for: a tmux name is not a
>   session identity, because this repo's own reconcile logic treats a lost row's
>   name as reusable, so `(snap_host, snap_tmux)` can resolve to a DIFFERENT live
>   session that merely inherited the pane name — and the access answer would then
>   be about the wrong row, in the `own` direction. `work.ts::linkSessionId` now
>   requires the row's `claude_session_id` to be one of the link's
>   `snap_claude_ids` and refuses an ambiguous name outright, which also refuses a
>   live session that has since started a new conversation. `session_id` (or an
>   owner) on `WorkLink` is what undoes all of it.
>
> **One command the hub will never see.** `upload_attachments` is
> `same_in_both`, so the gate added in `outbox.ts::pump` is the ONLY wall against
> putting a file on another person's host. Either route it so the hub can refuse
> it, or record in `docs/hub.md` that this one is permanently desktop-enforced —
> but do not leave it unstated.
>
> And one the frontend could not express: **`new_session` deliberately has no
> tier row** — a tier is about acting on an existing session, and `new_session`
> creates one. `HostDetail`'s "Find lost conversations → Resume" is gated on the
> SOURCE row instead. The backend's equivalent is the durable conversation-owner
> record refusing a `resume_claude_session_id` whose transcript belonged to
> someone else, which T3 built; make sure T7's gate actually consults it.

**Toolchain** rust · **Depends on** T6 · **Parallel-safe with** F1, F2

**Files** `mcp/tools/support.rs`, `mcp/tools/lifecycle.rs`,
`mcp/tools/session_ops.rs`, `mcp/tools/orchestration.rs`,
`mcp/tools/messaging.rs`, `mcp/tools/repo.rs`, `mcp/tools/tests.rs`,
`store/tasks.rs`

**Do.** Add `Reach { Read, Drive, Own }` and put the person gate in
`mcp/tools/support.rs::resolve_row_and_gate` — **not** in the
`resolve_and_gate` wrapper above it, because both paths meet in
`resolve_row_and_gate` and a check in the wrapper is bypassed by
`resolve_target`.

`Reach::Own` is not a grantable level (rule 3): it is the gate for the
operations only the owner may perform. **Which tools those are is the spec's
§4.3 invariant that names them — cite it, do not restate it here.** Revision 4
carried three copies of that list (the spec's Q6, this plan's decision (e), this
task) and they disagreed on whether a `drive` grantee may kill, restart or
rename the owner's session. There is now one list; T7 threads `Reach::Own` for
exactly the tools it names and `Reach::Drive` or `Reach::Read` for the rest.

**The person check cannot live inside `require_bound_client_sees`**: that
function returns `Ok(())` immediately when the client has no `org_id`, which is
every person's device and every host token. Add an unconditional sibling
`require_person_sees(s, caller, row, reach, what)` called beside it from
`resolve_row_and_gate`, answering `E_NOTFOUND` for an invisible row — no
existence oracle — and `E_FORBIDDEN` for a visible row at too low a level.

Thread `Reach` through **every non-test call site of
`resolve_target` / `resolve_target_row` / `resolve_and_gate` /
`resolve_row_and_gate`**, so the compiler forces each to be classified. The
compiler is the enumeration; do not quote a number for it (revision 4 did, and
three verifiers produced three different totals). The classification that is
*not* mechanical is `Read` vs `Drive`, so work the list in this order: anything
the spec's §4.3 `own` invariant names is `Own`; anything that writes to a pane,
a row, a task or a tmux server is `Drive`; the rest is `Read`.

`readonly` in `TOOL_POLICIES` cannot be reused as the discriminator:
`quick_replies` is `readonly: false` and a read, `whoami` is `readonly: false`,
and `inbox` is `readonly: true` and writes when `mark_read`.

**Collapse `require_visible_session`** into `resolve_row_and_gate` rather than
widening it. Its first line is `if !caller.is_scoped() { return Ok(()) }` —
false for the master *and* every unbound paired client — so today it is the only
gate on the tools that call it, and it checks nothing at all for a phone.
**Drive the collapse from `grep -rn require_visible_session crates/fleet-core/src`,
not from a list**: revision 4 enumerated `mcp/tools/messaging.rs`'s
`session_history` call and the eight in `mcp/tools/repo.rs`, and missed
`mcp/tools/lifecycle.rs`'s, inside `rewind_conversation`. That site has a second
gate so the "sole gate" claim survives, but a collapse driven from the
enumerated list leaves it behind, and a missed call site is a leak. `repo_file`
returns arbitrary worktree file contents and `session_history` is one of the
four reads the spec calls the substance of `watch`.

**Fix `mcp/tools/support.rs::resolve_reader`**: drop its `if caller.is_scoped()`
guard and consult the person scope unconditionally, answering `Ok(false)` — not
`E_FORBIDDEN` — for an invisible row, so the two cases stay indistinguishable.
Today it is both an existence oracle on any session id and a cross-person
**write** (it advances migration 044's read cursor on a session the caller does
not own).

**Fix `mcp/tools/support.rs::visible_task` and `store/tasks.rs::list_tasks`:**
both fence on `caller.host_alias` alone, so `list_tasks` serves every
`TaskRow`'s `prompt`, `result` and `error` fleet-wide — session content, and an
existing org leak independent of M1. Resolve both `requester_session_id` and
`worker_session_id` through `sees_session_row`, fail-closed when either is
invisible; `cancel_task` needs `Drive` on the worker. Re-order
`mcp/tools/orchestration.rs::wait_for_task`'s long-poll permit behind its
`visible_task` check — it is the only long poll where an unauthorized caller
burns one of the `MAX_LONG_POLLS_PER_CALLER` slots.

**Gate `mcp/tools/messaging.rs::send_message`'s recipient.** Today only the
sender's host is checked, and the recipient checks live inside
`if !scope.is_all()` blocks in `service/messages.rs` that never run for a
person's device — so `send_message { deliver: true, submit: true }` types
arbitrary text into a private session's pane and presses Enter. That is a silent
watch-to-drive escalation revision 3's deny list omitted entirely. Resolve
`to_session_id` through `resolve_row_and_gate` with `Drive`, and refuse
`deliver` / `wake` to anyone but the owner — they are pane writes. The same
owner-only rule applies to `broadcast_prompt`'s reach into a session. Neither is
in the spec's `own` list, because that list names whole tools and these are
argument-conditional arms; they get the owner-only answer by their own rule in
this task, and T14's matrix pins it.

**The in-pane agent reaches its own private row.** §4.4's rule is that a host
token sees `unclaimed` rows on its own host *and* the row whose pane it proves.
Without the second clause the agent of a fleet-started (therefore `private`)
session is refused `dispatch_task`, `send_message`, `session_activity` and
`work_link` — the exact failure §4.4 names when rejecting revision 3's version
of this rule. Nothing extra is needed here: the proof arrives on the connection
(T2's `Caller::pane`) and T6 has already resolved it into
`ViewScope::proven_session`, so `require_person_sees` reads one `Option<i64>`
and the clause holds on **every** tool this task threads. That is R6-i's whole
purpose — revision 5 put the proof in three tools' arguments and left this
paragraph describing a hole it could not close.

**Green at the end.** A watcher is refused every `Drive` tool with
`E_FORBIDDEN` and a stranger with `E_NOTFOUND`; a driver is refused every tool
the spec's `own` invariant names; `repo_file` / `session_history` on another
person's session answer as not found for an unbound paired client;
`resolve_reader` answers `Ok(false)` identically for an unknown id and an
invisible one and writes no cursor; `list_tasks` for person B omits a task whose
requester or worker is A's private session;
`send_message { deliver, submit }` into another person's session is refused; a
host token whose request carries its own row's pane reaches `session_activity`,
`dispatch_task`, `send_message` and `work_link` on that row and is refused every
other private row on the same host.

**Breaks, update in this task.**
`mcp/tools/tests.rs::resolve_row_and_gate_returns_turn_seq_for_the_completion_signal`,
which pins the current two-gate behaviour.

---

### T8 — Choke point 3: the result gate actually runs, and can drop a row

**Toolchain** rust · **Depends on** T7 · **Parallel-safe with** F1, F2

**Files** `mcp/tools/mod.rs`, `mcp/tools/support.rs`, `service/orgs.rs`,
`service/view_scope.rs`

**Do.** Revision 3's risk table called `redact_work_via` "a fail-closed backstop
over every tool result". It is neither, for the caller M1 introduces. Two
structural changes.

**(a) Make it run.** `mcp/tools/mod.rs::call_tool` invokes it only inside
`if let (Ok(result), true) = (out.as_mut(), caller.is_scoped())`, and
`is_scoped()` is false for the master and for every unbound paired client. Make
the condition unconditional for any caller carrying a person or a host, and
invert `service/orgs.rs::redact_work_via`'s early return (`if scope.is_all()
{ return; }`) to the `ViewScope` equivalent.

**(b) Give it the power to drop a row.** The gate can only delete keys listed in
`service/orgs.rs`'s `WORK_FIELDS` — nothing in `redact_json`,
`mcp/tools/support.rs::rewrite_json_content` or `strip_all_work` can remove an
object from an array. Add a row-drop mode to `rewrite_json_content`: a JSON
object that looks like a session row (`id` + `host_alias` + `tmux_name`, or
carrying `visibility`) whose **stored** row the scope cannot see is removed from
its enclosing array, and replaced by `null` at a scalar position. Key it on the
stored `visibility` / `owner_person_id` read by id from the store, never on the
payload — the same discipline `redact_work_via` already uses for orgs
("a projection that dropped `org_id` cannot make a row look unassigned").

**There is no `redact_session_row`.** Revision 4 added one "for any caller that
may see a row but not its content" — a state T6's two-valued
`sees_session_row` has just declared unrepresentable, and a redaction list of
the kind the spec's §4.3 explicitly rejects ("a **positive** list of what an
out-of-scope caller may learn … **not a redaction list** that has to be kept in
step with a growing struct"). A caller either sees the row and its content or
sees neither. Drop, never blank.

Keep the work half on `OrgScope` untouched; the session half is new code on
`ViewScope`, sized like a feature rather than a list entry.

**Green at the end.** Every tool result containing another person's private
session row has that row removed, driven through `call_tool` rather than the
tool directly; the gate runs for an unbound paired client — a regression guard
on `call_tool`'s condition; a poisoned store lock drops every session row, not
only its work fields.

**Breaks, update in this task.** None expected; the work-field assertions are
unchanged by design.

---

### T8c — Classify or gate the twelve surfaces the repaired coverage test names

**Toolchain** rust · **Depends on** T8b's gate · **Status: LANDED.**

> **What landed.** All twelve surfaces are accounted for, and
> `mcp::tools::tests::every_session_addressed_tool_declares_its_reach` passes:
> `delete_worktree` gained `Reach::Own` on every alive occupant (and its
> `E_WORKTREE_BUSY` text a count in place of `host/tmux_name`), the seven
> `work` page arms and `work_link { name }` thread the caller's `ViewScope` /
> `Reach::Drive` down into the service functions, and `new_shell_session`,
> `peer_exchange` and `whoami` are exempt with reasons a reader can check —
> `peer_exchange`'s written into `docs/hub.md` (*Security notes*, beside
> *Link two hubs*) rather than left in a table. None of the twelve was closed by
> editing the tables to excuse it.
>
> **The five behavioural pins are written** and each was confirmed to FAIL
> with its fix reverted:
> `deleting_a_worktree_under_another_persons_session_needs_own`,
> `dispatching_a_task_in_another_persons_name_needs_drive`,
> `usage_report_is_one_persons_own_spend`,
> `naming_work_on_another_persons_session_needs_drive` and
> `a_broadcast_through_the_tool_fans_out_only_to_the_senders_own_sessions`
> (the last one new: the earlier `a_broadcast_reaches_only_what_its_sender_may_drive`
> calls `select_targets` directly and never touched the handler's wiring).
>
> **One disagreement is left for the frontend**, deliberately not patched
> here: `src/lib/share.ts` still says `delete_worktree: 'drive'` where the hub
> now says `own`. The hub is the stricter side, so nothing leaks — a `drive`
> grantee is offered the control and then refused it. `share.ts` has to move
> to `'own'`.

**What the repaired gate now does**, and why it found what three hand reviews
did not: it asserts its own health first (eleven sentinel tools a reader
verifies by eye, a floor of 45 under the derived set, and a check that every key
in the address lists is a property a served tool really has); it is keyed on
**(tool, action)** for the umbrella tools, with the action sets derived from
`WORK_LINK_ACTIONS` and `WORK_ACTIONS` so a new arm cannot ship without a row;
and it recognises a session addressed by `worktree_id`, `task_id`, `tmux_name`,
`old_name`, `to_addr`, `claude_session_id`, `link_id` or `fresh_for`, not only by
`session_id`. The vacuity was proven closed rather than argued: stubbing
`session_addressed_tools()` to `Default::default()` now fails the test, where
before it would have passed.

**The twelve, as the test reports them.** Four unclassified tools:

| Surface | Addressed by | Note |
|---|---|---|
| `delete_worktree` | `worktree_id` | no session gate at all; `force: true` destroys another person's worktree; its refusal prints `host/tmux_name`. Desktop says `drive`, so the hub is the permissive side |
| `new_shell_session` | `worktree_id` | **named nowhere before** — not in the reviews, the plan or the spec. Sibling of `new_session`, so it may end up exempt for the same reason; nobody had written that down |
| `peer_exchange` | `to_addr` | `Access::Client`. The question "is a peer link a trusted operator-to-operator channel, and is that written down?" now has to be answered in a table rather than in a review |
| `whoami` | `tmux_name` | was a minor in the last review |

Eight ungated arms — `work_link { name }` (must thread `Drive`, threads
nothing), and `work { context | resume_plan | today | tidy | tree | task |
review }` (each must thread a `ViewScope` and threads nothing). Six of those
eight were named nowhere in any previous review: `context`, `resume_plan`,
`tidy`, `task`, `review`, and `new_shell_session` above. What they return is
session content — `ResumeCandidate` carries name, host, branch, worktree,
`pr_url` and `last_claude_session_id`; `TidyCandidate` carries `session_id`,
`host_alias` and `tmux_name`; `ReviewItem` carries `session_id`, `session_name`
and `host`.

**Also still open from the review that preceded the gate** (see
`m1-t6t8-findings.md`, kept in this session's scratchpad): `dispatch_task`'s
requester at `Reach::Read` when it should be `Drive`, `broadcast_prompt`'s
fail-open `Option<ViewScope>`, T8's `looks_like_session_row` recognising a row
only by the key `id` (and therefore blind to the `session_id` shape above),
`work_link { start }` creating a session owned by the hub's person rather than
the starter, `task_visible_in_scope_pure`'s proven-endpoint widening, and
`related_sessions`' exemption claiming a fence that does not exist for a
person's device.

**One judgement call to check rather than inherit.** The gate put
`work { reopened }` in the no-gate table because `ReopenedWork` carries
item-level counts plus the host of the newest past session — the per-host count
shape rule 6 allows. If `last_host` reads as session metadata, that row moves
and there is a thirteenth surface.

**Behavioural pins still unwritten**, all listed in the findings file: the
`delete_worktree { force: true }` destruction, the `dispatch_task` watcher
escalation, the `usage_report` leak, the `work_link { name }` write onto another
person's row, and a `broadcast_prompt` call through the tool.

---

### T9 — Choke point 4: the live stream — fence, rescope, grant generation, and the frames that cannot be fenced today

**Toolchain** rust · **Depends on** T8 · **Parallel-safe with** F1

**Files** `mcp/events_route.rs`, `events.rs`, `store/orgs.rs`,
`store/session_grants.rs`, `mcp/tools/tests_isolation.rs`,
`src-tauri/src/backend/events.rs`, `src/lib/events.ts`

**Do.** Five changes, all required together or the stream is the hole.

1. **`mcp/events_route.rs::fence_frame`** returns the payload verbatim on
   `scope.is_all()` — and `OrgScope::All` is documented in `service/orgs.rs` as
   covering "an unbound paired client", which is exactly both people in the M1
   scenario. Invert to
   `scope.person.is_none() && scope.host.is_none() && scope.org.is_all()`, take a
   `&ViewScope`, and drop the `msg.kind() != "session"` short-circuit.
2. **Make `rescope` unconditional.** `let rescope = caller.is_scoped().then(...)`
   in `mcp/events_route.rs` is `None` for a person's device, and *both*
   consumers — the 15 s keep-alive re-scope and the pre-frame generation check —
   are `if let Some(...)`. Revision 3's T4a table said this row was "unchanged";
   in fact **for the M1 default shape a revoked grant is noticed on the stream
   never**. Make `rescope` unconditional for any caller carrying a person or a
   host. Add `grant_generation` beside `scope_generation` in
   `mcp/events_route.rs::StreamState` and compare both. Note that the existing
   mechanism **ends** the stream — each consumer's `return None` inside the
   keep-alive branch and the pre-frame branch — rather than re-scoping:
   acceptable and fail-closed, but it means one grant change disconnects every
   affected stream, and `docs/hub.md` must say the 15 s beat is the guarantee
   while `GRANT_GENERATION` is only an optimisation, because the counter is a
   process-local atomic and `auth_epoch` is a SQLite trigger. Keep
   `client_is_live` as the **device** mechanism and the grant re-read beside it
   as the **grant** mechanism; rule 8 forbids merging them.
3. **`session:killed` cannot be fenced today.** `events.rs::SessionKilledPayload`
   is `{"id": N}` with neither `host_alias` nor `session_id`, so it falls
   through `fence_frame`'s `_ => {}` default and every kill of every private
   session reaches every stream — an existence oracle. Extend
   `SessionKilledPayload` with `host_alias`, `visibility` and `owner_person_id`
   **at emit time** (the row is gone by the time the frame is read, so a lookup
   cannot work). This is a wire addition covered by T13's contract bump.
   Fence any session frame with no `visibility` key fail-closed.
   **Where the frame is load-bearing, corrected:** on the desktop the
   `"session:killed"` arm of `src-tauri/src/backend/events.rs::EventBridge::observe`
   does `seen.sessions.remove(&id)` — it maintains the resync bookkeeping set
   that decides whether a reconnect re-lists. The **store** removal is on the
   frontend: `src/lib/events.ts` pushes `{ type: 'killed', id: ev.payload.id }`
   into `sessionEvents`, and `applySessionEvents` turns that into
   `removeSession`. Revision 4 cited the `"host:removed"` arm and called it a
   store removal; an implementer following that edits the wrong match arm and
   misses that the payload's TS type is pinned in `src/lib/events.ts` as
   `{ id: number }` — which this change widens.
4. **Fence the three unfenced kinds**: `task:updated` (a whole `TaskRow` with
   `prompt` / `result` / `error`), `move:progress` (`session_id`, `to_host`,
   free-text `detail`) and `worktree:updated` (`name`, `path`, `branch` — a
   branch name is content by §4.3's own argument), all in `events.rs`. Add
   whichever cannot be fenced per frame to a person's hidden kinds, and move
   `fence_host_bound`'s predicate off `caller.is_scoped()` onto "is not the
   hub's own reader".
5. **A grant mutates no `sessions` row**, so sharing emits nothing at all and
   revision 3's "grants arrive by row event, no re-fetch" cannot hold. Add
   `Store::announce_grant_change(session_id, affected_persons)` modelled on
   `store/orgs.rs::announce_org_moves` — the repo's one precedent for
   announcing a computed change with no column behind it, down to the shape:
   an explicit `UPDATE sessions SET row_version = row_version + 1` (since
   migration 063 a same-value UPDATE no longer moves it on its own), one
   transaction around the bumps, the rows emitted after it commits. Call it
   from `grant`, `narrow` and `revoke`.

   It emits **two** things, and R6-j is why both are needed. The
   `session:updated` carries the row itself, which is how a new recipient
   learns the row exists at all — before the grant they could not see it. The
   second is a new frame `grant:changed`, ids only —
   `{ session_id, person_id, level }` with `level: null` for a revoke — which
   is how each client keeps its **grant set** current without re-fetching.
   Since access is derived on the client from the row plus that set, a row
   event alone would leave a recipient holding a row they cannot classify.
   `grant` is a new event **kind**, so it needs the full set of steps
   `events.rs` enforces: a `RowChange` variant (the match is exhaustive at
   compile time), an `EVENT_NAMES` entry, an `EVENT_KINDS` entry, a
   declaration in `src/lib/events.ts` for `frontend_declares_every_event_name`,
   and a `fence_frame` arm — it goes to the affected persons' streams and to
   nobody else, and it joins `HOST_BOUND_HIDDEN_KINDS` beside `work`,
   `settings` and `update`, since a per-host token has no person and no grants.

**Fix the desktop resume in the same task.**
`src-tauri/src/backend/events.rs::EventBridge::pump` re-lists only when the hub
answered `resumed: false`. After a revoke the hub ends the stream, the bridge
reconnects with `Last-Event-ID`, the hub replays the correctly fenced gap and
answers `resumed: true`, no resync runs, and the revoked row stays in the Svelte
store indefinitely — **T15's e2e assertion would pass while the UI is wrong**.
Fold the caller's scope into the frame id's generation half
(`history.generation ^ hash(org_generation, grant_generation, person)`, where
`mcp/events_route.rs` builds the id) so only *that* caller's resume is
invalidated rather than forcing every phone on the hub to re-list. Leave the
deliver path in `src-tauri/src/backend/events.rs` a pure pass-through otherwise:
it applies no fence of its own and bypasses none, which is correct layering.

**Green at the end.** A session frame carrying no `visibility` key is dropped;
sharing a session emits both a `session:updated` and a `grant:changed` on the
recipient's stream and neither on a third person's; a revoke emits
`grant:changed` with a null level; a stream
drops within one beat of a grant being revoked — the natural home is
`mcp/events_route.rs::a_subscription_carries_the_emitted_event` via
`EventsState::with_keepalive`; a reconnect after a scope change re-lists rather
than resuming (desktop). **The enumerating test this area needs:** every
`EVENT_KINDS` entry is either in the hidden set, fenced per frame by
`fence_frame`, or explicitly declared to carry no session-scoped content —
without it, the next kind added is a silent leak.

**Breaks, update in this task.** `mcp/tools/tests_isolation.rs::run_matrix`'s
`/events` section — its `EVERYONE` constant gains owner, watcher, driver and
stranger members, and its frame-pair agreement assertion gains a third shape,
`session:killed`.
`mcp/tools/tests_isolation.rs::work_changed_carries_ids_only_and_reaches_only_unbound_callers`
asserts `delivered == who.is_unbound()`; `is_unbound()` becomes the **wrong**
predicate the moment an unbound client carries a person — rewrite it against the
new scope, or it keeps passing while leaking.
`mcp/events_route.rs::a_host_bound_stream_never_carries_work_frames` — add the
unbound-person case.

---

### T10 — The remaining scope sites and the unlisted leak surfaces, then delete the org-only shims

**Toolchain** rust · **Depends on** T9 · **Parallel-safe with** F1, F2

**Files** `service/orgs.rs`, `service/sessions/targeting.rs`,
`service/sessions/prompt.rs`, `service/messages.rs`, `service/health.rs`,
`service/usage.rs`, `service/worktrees.rs`, `service/sessions/discover.rs`,
`service/sessions/restore.rs`, `service/sessions/lifecycle.rs`,
`service/work/{summary,today,view,card,local,mod,agent_handover}.rs`,
`service/trackers/tickets.rs`, `service/hooks.rs`, `mcp/tools/repo.rs`,
`mcp/tools/fleet.rs`, `mcp/tools/messaging.rs`, `mcp/tools/session_ops.rs`,
`mcp/tools/orchestration.rs`

**Do.** Convert every remaining `sees_row_org_only` / `sees_session_org_only`
site from T6's rename to `ViewScope::sees_session_row`, then **delete both shims
from `OrgScope`** — the deletion is what makes this task's completeness
compiler-checked, and it is why the rename existed. `sees_org`, `sees_link` and
`redact_json` stay on `OrgScope`, untouched.

**Strike revision 3's "the whole work graph is untouched".** A work read that
names a session leaks the session, and the work graph reads session rows in
`service/work/today.rs` (`TodaySession` carries the friendly-or-tmux name, host,
`pr_url` and status for every session in the fleet), `service/work/view.rs`
(whose `link_visible` returns true immediately on `is_all()`),
`service/work/card.rs`, `service/work/local.rs`, `service/work/mod.rs`,
`service/work/agent_handover.rs`, `service/usage.rs`,
`service/trackers/tickets.rs`, `service/messages.rs` and `service/hooks.rs`.
Enumerate them from the compiler after the shims are deleted rather than from
this paragraph.

**`work_link { summarize }`** (`mcp/tools/orchestration.rs` →
`service/work/summary.rs`) is addressed by `link_id`, not `session_id`, so it
passes through **no** choke point: it forks a Claude on the session's own host,
reads the transcript and stores a summary into the org-readable work journal,
which survives the grant being revoked. It needs its own `sees_session_row`
check on the link's session at the reach the spec's §4.3 `own` invariant gives
it, and the journal `summary` row must be fenced by the summarised session's
owner.

`service/sessions/targeting.rs::related_sessions_scoped` and
`find_session_by_tmux_name_scoped` take a `&ViewScope`. The latter is `whoami`'s
backing read and therefore the claim path's addressing, so the host-agent arm
**must keep finding an `unclaimed` row on its own host**, or `session_claim` has
no way to name the session. It also finds the agent's **own** `private` row now,
through `ViewScope::proven_session` — no special case here, because the scope
already carries the answer (R6-i); it is worth a test, since `whoami` was one of
the three tools revision 5 gave a pane argument to and is the one an agent calls
first.

`service/sessions/prompt.rs`'s `BroadcastFilter.scope` becomes
`Option<ViewScope>` and must be `Some` for any caller with a person — but the
`None` comes from `mcp/tools/messaging.rs`
(`scope: (!scope.is_all()).then_some(scope)`), so **fixing `prompt.rs` alone
changes nothing**. A `watch` grant must not be a broadcast target even though
the row is visible. `service/messages.rs`'s two `if !scope.is_all()` blocks (the
addressing one and the resolved-target one) and `inbox`'s filter become
unconditional for a person.

**The surfaces the spec's §5.1 list did not name.** (Revision 4 headed this
"the five surfaces neither document mentioned"; the companion spec's §5.2 now
covers the first three in its own table and the last two in its closing
paragraph, so the heading was stale and the two documents contradicted each
other. They are still this task's work.)

- **(a) `mcp/tools/repo.rs::list_worktrees` and `list_host_worktrees`** take no
  `Extension(caller)` **at all** and return
  `WorktreeOccupant { host_alias, tmux_name }` for every alive session
  fleet-wide, plus each worktree's branch and path. Give both a caller and
  filter occupants through `sees_session_row`, dropping the occupant entry, not
  the worktree.
- **(b) `mcp/tools/session_ops.rs::discover_lost_sessions`** is gated by
  `require_host` only, which passes for every paired client, and returns `cwd`,
  `git_branch`, `claude_session_id`, `derived_tmux_name` and
  `existing_session_id` for every transcript it finds up to its cap — then
  `new_session { resume_claude_session_id }` loads another person's whole
  conversation into a session the caller owns. Make the discovery
  master/operator-only in M1; drop candidates resolving to a row the caller
  cannot see; never return `existing_session_id` for an invisible row; strip
  `row.id` / `row.tmux_name` from
  `service/sessions/lifecycle.rs::reject_held_conversation`'s refusal text.
  `new_session` must refuse a `resume_claude_session_id` that any row — live,
  lost or reaped — ever held under a different owner, **which is what T3's
  `conversation_owners` table exists for**: read it, not the `sessions` table,
  because the attack works precisely when the row is gone.
- **(c) `mcp/tools/session_ops.rs::restore_host_sessions` →
  `service/sessions/restore.rs::plan_restore`** passes no scope and returns
  `RestorePlanEntry { session_id, tmux_name, cwd, claude_session_id, friendly_name, … }`
  for every lost session on a host, with `dry_run` explicitly skipping the
  confirm gate. Filter the plan, and gate it at the reach the spec's §4.3 `own`
  invariant gives it. Note that it is implemented over `recreate_session`, so
  the two must be gated at the same reach or the batch is a way round the
  single-row gate.
- **(d) `mcp/tools/fleet.rs::usage_report`** builds its scope inline as
  `if caller.is_scoped() && … { org_scope } else { OrgScope::All }`, so a
  person's device gets names and per-session spend for every session it counts.
  Replace it with `caller.view_scope`, which is also what makes T6's "one
  constructor" test true.
- **(e) `mcp/tools/fleet.rs::fleet_health`** branches on `caller.is_scoped()`
  rather than on a scope **value** — so the rename produces **no compile error
  here and the leak survives** — giving a person's device `HealthView::Fleet`,
  whose `sessions_total`, `by_status`, `stuck`, `ghosts`, `context_red` and
  `usage_by_host` are computed over every private session in the fleet. Polled
  once a second, that is a live activity channel. Add
  `HealthView::Person(ViewScope)` filtering with `sees_session_row` as
  `service/health.rs`'s `HealthView::Org` already does, and decide explicitly:
  DoD 8 permits a per-host unclaimed count **where R5-d serves one at
  all**, and it does not permit inheriting fleet-wide aggregates over other
  people's private sessions.

**Audit every remaining `caller.is_scoped()` used as a "do I need to filter?"
test in the same pass.** There are several, and none is compiler-flagged.

**Green at the end.** `list_worktrees` for person B shows no occupant naming A's
private session; `discover_lost_sessions` is refused to a paired client, and
`new_session` refuses a `resume_claude_session_id` whose `conversation_owners`
row names someone else — including after the original session has been reaped;
`restore_host_sessions { dry_run: true }` for a non-owner returns an empty plan;
`usage_report` and `fleet_health` for person B count only B's sessions;
`work { today }` / `work { tree }` / `work_link { summarize }` for person B
never name A's private session; `broadcast_prompt` from a watcher does not reach
the watched session. **The deletion of the two shims is itself the completeness
check** — the build fails if a site was missed.

**Breaks, update in this task.** Everything that called the two shims, by
construction.

---

### T11 — Long polls re-check before returning and on every wake (revision 3's T4a)

**Toolchain** rust · **Depends on** T10 · **Parallel-safe with** F1, F2

**Files** `service/tasks.rs`, `service/messages.rs`, `mcp/tools/orchestration.rs`,
`mcp/tools/messaging.rs`, `mcp/guard.rs`, `mcp/tools/support.rs`

**Do.** The `Deadline::LongPoll` rows in `mcp/guard.rs`'s `TOOL_POLICIES` are
more than the three both documents listed: `wait_for_session`, `wait_for_reply`,
`run_prompt`, `wait_for_task` and `add_project`. `peer_exchange` takes a
long-poll permit but is `Deadline::Quick`, so "holds a permit" and "is a long
poll" are two different lists — check the policy rows, not the permit calls.

All four session-bound waits are mechanically trivial — revision 3's implicit
worry was unfounded. `service/tasks.rs::wait_for_session_probed` and
`wait_for_task_with` are `lock → read one row → unlock → sleep 500 ms` loops,
and `service/messages.rs::wait_for_reply` waits on a `Notify` but caps each wait
at `REPLY_POLL_FLOOR` = 500 ms, so it also wakes twice a second. Insert the
re-check at the **top of each loop, inside the lock window that already reads
the row** — one extra column read per wake — and again **immediately before
every `ok_json`** in `mcp/tools/orchestration.rs` and `mcp/tools/messaging.rs`
(the messaging one is the highest value, since its payload is message content).

`fleet-core`'s service layer must not import `Caller`: pass a `&dyn` predicate or
a small trait, following the existing `PaneProbe` pattern in
`service/tasks.rs`.

**`run_prompt` is the honest hard case** and revision 3 did not mention it at
all. Its order is permit → `deliver_prompt` → wait → `transcript_for`
(`mcp/tools/orchestration.rs`), so a pre-return re-check can withhold the
transcript but cannot un-send a prompt already in the owner's pane. Add a
re-check between the permit and `deliver_prompt`, a second before the
transcript, and write into DoD 6 and `docs/hub.md` that a prompt already
delivered is not recalled.

Record, without fixing, that `mcp/tools/support.rs::long_poll_permit` buckets on
`caller.label()` — a **device** name, not a person — so a revoked device holds
its `MAX_LONG_POLLS_PER_CALLER` slots for the remaining `LONG_POLL_CAP` = 660 s.
Not a leak; a small denial-of-service window, and a reason never to key a future
per-person quota on `label()`.

**Green at the end.** A `wait_for_reply` started before a revoke answers
`E_NOTFOUND` instead of the message body; a `wait_for_session` whose grant is
narrowed mid-flight returns nothing; `wait_for_task` takes its permit only after
`visible_task` passes.

**Breaks, update in this task.**
`mcp/tools/tests.rs::run_prompt_refuses_a_session_that_is_not_between_turns` and
`wait_for_task_marks_the_worker_result_as_untrusted` — both construct callers
and need the person field.

**LANDED.** `service::tasks::AccessRecheck` (+ `NoRecheck`, the named
waiver for the move engine) is the `&dyn` predicate; the MCP layer's two
implementations are `SessionRecheck` / `TaskRecheck` in
`mcp/tools/support.rs`, built on `person_sees` and `task_visible_at` — the
existing gates, split out into `IpcError` flavours so there is still ONE
copy of each. `SessionRecheck` re-reads the row **by id**, never by
`(host_alias, tmux_name)`. The in-loop calls are in
`wait_for_session_probed`, `wait_for_task_with` and
`messages::wait_for_reply`, each inside the lock window that already reads
the row (and, in `wait_for_reply`, BEFORE the inbox read — the body is
never loaded); the pre-return call is `FleetTools::recheck_now`, in
`wait_for_session`, `wait_for_task`, `wait_for_reply` and twice in
`run_prompt` (before `deliver_prompt`, and before the transcript).

Proof: eleven tests in `mcp/tools/tests.rs`, each pin confirmed by
reverting it and watching the test go red.

* the four tool-level waits — a revoke mid-flight refuses, and a positive
  control is served: `a_wait_for_{reply,session,task}_*`, plus
  `the_wait_run_prompt_parks_in_*`, which covers both a revoke
  (`E_NOTFOUND`) and a narrowing to `watch` (`E_FORBIDDEN` — the case
  `Reach::Read` could not have caught);
* `the_pre_return_recheck_closes_the_pane_probes_window` for the one await
  a wait takes outside its own lock window, which is the gap
  `recheck_now` exists for;
* **the in-loop checks of `wait_for_reply` and `wait_for_task`, pinned
  separately** (`the_wait_behind_wait_for_*`). This is worth recording:
  reverting either in-loop line left the corresponding TOOL test green,
  because `recheck_now` caught the same revoke on its own. A tool test
  therefore pins the pre-return check and says nothing about the in-loop
  one, so the two are called at the service seam with nothing behind them.
  Both of those tests drive the revoke from INSIDE the wait's own lock
  window (`RevokeOnSecondWake`), so "the wait was in flight when the share
  went" is a fact and not an interleaving of two sleeps, and the wake
  count is in the failure message. Worth knowing for the next person who
  reverts a pin: a reversion harness that restores the file afterwards
  must `touch` it — cargo compares mtimes, and a restored file older than
  the artifacts is not rebuilt, so three "load flakes" here were in fact
  the pinned-out binary still on disk;
* the source-level `no_long_poll_tool_waives_its_access_recheck`.

The `run_prompt` / `wait_for_task` caller-construction breakages predicted
above did not materialise: `device_of` and `gate_fixture` already carry
the person field, and neither test needed a change. The rename of
`require_person_sees`'s body to `person_sees` did move two
`scope_guard_tests` rows (`ORG_HALF_SITES` and the `sees_session_org_only`
guard's call-site list), both updated. `add_project`, the fifth
`Deadline::LongPoll` row, is session-less and carries no re-check (DoD 6).

---

### T12 — The sharing tools, `Access::HostToken`, the pane proof and the claim path

**Toolchain** rust · **Depends on** T11 · **Parallel-safe with** F1, F2

**Files** `mcp/guard.rs`, `mcp/tools/present.rs`, `mcp/tools/sharing.rs` (new),
`mcp/tools/mod.rs`, `mcp/tools/params.rs`, `mcp/tools/session_ops.rs`,
`mcp/tools/tests.rs`, `service/sessions/claim.rs` (new),
`crates/fleet-hub/src/main.rs`, `crates/fleet-hub/src/session.rs` (new),
`docs/control-api.md`, `docs/control-api-reference.md`

**Do.** The tools:

| Tool | Access | Notes |
|---|---|---|
| `session_share { session_id, person, level }` | `Client` | owner only; **no `org` argument in M1** (owner's decision 1); `level` is `watch` or `drive` and nothing else (rule 3) |
| `session_unshare { session_id, person }` | `Client` | **owner only.** Revision 4 annotated this "or an admin on a departed member's grant"; R5-c moves that to M2, where memberships make "a departed member" mean something |
| `session_narrow { session_id, person }` | `Client` | `drive` → `watch` only; there is deliberately no tool that raises a level |
| `session_access { session_id }` | `Client`, readonly | the grant list, for the Share sheet |
| `my_grants { }` | `Client`, readonly | **R6-j.** `{ person_id, grants: [{ session_id, level }] }` — the caller's own person and every live grant to them, over `store/session_grants.rs::grants_for_person`. This is what a client derives access from, together with each row's `owner_person_id` and `visibility`; it is fetched once at startup and after a resync, and kept current by T9's `grant:changed`. It is per-caller by construction — a tool answer, where per-caller belongs — which is exactly what a field on a broadcast row could not be |
| `session_claim { session_id, person }` | **`HostToken`** | see below. **No `pane_id` argument** (R6-i): the pane rides the connection |

The first five go in **`mcp/guard.rs::NOT_FOR_HOST_TOKENS`** (today a one-entry
list holding `catalog_admin`): a per-host token has no person and can never be
an owner or a grantee, so serving it definitions it can never use costs request
bytes for nothing.

**`session_claim` needs a level `Access` does not have.** `mcp/guard.rs::Access`
has four variants; `Access::Client`'s own doc says it covers a per-host token as
well as every paired phone, and `access_allows` returns `true` unconditionally
for it. "Host token only" as a `TOOL_POLICIES` row is therefore not
expressible. Add `Access::HostToken` and teach `access_allows`,
`mcp/tools/present.rs::visible_to`, `is_admin_tool`, `is_client_tool` and the
served-count assertion. That is the honest cost, and the compiler forces every
site.

**The pane proof is already here, and there is nothing to record** (R6-i).
Revision 5 made it an argument on three tools (`whoami`, `register_self`,
`session_claim`) backed by a durable `(host_alias, pane_id) → session_id`
record. Both halves are struck:

- **No tool takes a pane argument.** `WhoamiParams` and `RegisterSelfParams` are
  untouched, and `session_claim`'s parameters are `session_id` and `person`. The
  proof arrives on the connection as `X-Fleet-Pane` (T2's `Caller::pane`) and
  T6's `Caller::view_scope` resolves it per request through
  `store/sessions.rs::find_session_by_pane` into `ViewScope::proven_session`, so
  §4.4's clause 2 already holds on every tool by the time this task starts.
- **No durable record.** Keyed by host alias, it would have made every pane any
  agent ever proved reachable by every agent on that host — DoD 9 inverted — and
  it needed invalidation logic against the reconcile pass that rewrites
  `tmux_pane_id`. Per-request resolution has neither problem: the proof lapses
  by itself. (`store/mod.rs::set_controller` was never a candidate either: it
  writes the fleet-wide singleton settings `controller.host` /
  `controller.tmux`, one controller for the whole fleet.)

**What the pane proves, exactly.** Any process that can run `tmux list-panes` on
the host can enumerate every pane id there, so presenting one proves host
access, not pane occupancy. Under §4.4's deployment rule — separate unix
accounts per person, each its own host alias — one person's agent cannot read
another's tmux socket and the proof holds. On a host with one shared unix
account it proves nothing, which is the same sentence both documents already
write about that deployment. `docs/hub.md` (D1) says it in those words; do not
write that the pane proof is stronger than the machine without that condition
attached.

**`session_claim`'s effect.** It succeeds only when `ViewScope::proven_session`
is `Some(session_id)` — the request's own pane resolved to the row being
claimed — refusing a host token that merely happens to be on the same host. T5's
reservation closes the window in which a fresh, owned session is briefly
`unclaimed`; this closes the rest.

**Refuse it with the right error.** `sessions.tmux_pane_id` is the session's
ACTIVE pane as the last reconcile pass saw it, so an agent in a non-active pane
of a hand-started multi-pane session presents a pane no row carries and
`proven_session` is `None`. The caller is on the right host and the row exists,
so `E_NOTFOUND` would send the operator hunting a row they can see in
`fleet-hub session unclaimed`. Answer `E_INVALID_STATE` naming the rule — the
claim runs from the session's active pane — and distinguish it from the two
neighbouring refusals: a proven pane that resolves to a **different** row than
`session_id` is `E_FORBIDDEN`, and an already-owned row is `E_EXISTS`. Three
refusals, three messages; the operator can act on each.

The claim writes the owner, sets `private`, and records a `session_events` row
with
**`store/timeline.rs::insert_session_event_quietly`** — the loud form fans a
`session:event` frame to every client, which the doc comment above the quiet
variant documents as exactly the leak it exists for.

**`fleet-hub session claim <id> --person <name>`** and
**`fleet-hub session unclaimed`** are a whole new top-level `Cmd::Session` +
`SessionCmd` + `crates/fleet-hub/src/session.rs`: there is no `Session`
subcommand today, and the master cannot reach a `HostToken` tool, so both write
and read through the store directly — the `client grant assets` precedent in
`crates/fleet-hub/src/pair.rs`. `session unclaimed` prints the per-host counts,
and on a hub with more than one person it is **the only** way a human sees them
(R5-d).

Then **both** doc gates.
`REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` rewrites
`docs/control-api-reference.md`; and `mcp/doc_gen.rs::narrative_guide_names_every_tool`
`include_str!`s the **hand-written** `docs/control-api.md` and fails until each
new tool name appears there as a backticked literal — regeneration does not fix
it, and revision 3 named only the first.

**Re-measure and raise `BUDGET_BYTES`** in
`mcp/tools/tests.rs::the_served_definition_budget_stays_bounded` from the number
the failing run prints. Six definitions against a constant that sits within a
few hundred bytes of the current measurement makes this a required step, not an
incidental one. `NOT_FOR_HOST_TOKENS` pays some of it back, and no tool gains a
pane parameter (R6-i), which is schema text revision 5 would have added to three
more.

**Green at the end.** `session_claim` from a host token whose request carries no
pane header is refused; from a pane that resolves to a different row it is
`E_FORBIDDEN`; from a non-active pane of the right session it is
`E_INVALID_STATE` naming the active-pane rule, not `E_NOTFOUND`; on an
already-owned row it is `E_EXISTS`; the recorded event does not reach other
clients' streams; the same host token's next request, after the reconcile pass
has rewritten `tmux_pane_id`, proves nothing — no invalidation step, and a test
that drives a reconcile pass between two requests pins it. `my_grants` returns
the caller's own person and only grants to them, `[]` for a person with none,
and is refused to a per-host token. `session_share` by a non-owner, by a
grantee, naming an org recipient, and naming a level other than `watch` or
`drive` are each refused.

**Breaks, update in this task.**
`mcp/tools/tests.rs::every_router_tool_has_exactly_one_tool_policy_row` (note:
two comments in `mcp/guard.rs` cite a test name that does not exist, and one of
them cites no name at all — fix both while you are here);
`the_served_definition_budget_stays_bounded`;
`a_readonly_token_is_served_no_mutating_tools_and_a_client_no_admin_tools`,
whose `master.len() == all.len() - 1 - device_only` arithmetic gains a host-only
term; `readonly_tools_are_client_tools_or_the_documented_list_clients_exception`
for `session_access` and `my_grants`; `mcp/doc_gen.rs::reference_is_current` and
`narrative_guide_names_every_tool`.

---

### T13 — Tauri commands, `capture_session`, verdicts, the contract bump to 7, and three regenerations

**Toolchain** rust + frontend · **Depends on** T12 · **Parallel-safe with** F1

**Files** `src-tauri/src/lib.rs`, `src-tauri/src/commands/sessions.rs`,
`src-tauri/src/backend/verdicts.rs`, `src-tauri/src/backend/routing.rs`,
`src-tauri/src/backend/remote.rs`, `src-tauri/src/backend/contract.rs`,
`src-tauri/src/backend/tests_routing.rs`,
`src-tauri/src/backend/local_only.golden.json`,
`src-tauri/src/backend/hub_contract.golden.json`,
`crates/fleet-core/src/wire_contract.rs`, `src/lib/hub_verdicts.generated.json`,
`docs/hub.md`

**Do.** **Bump the wire contract** (owner's decision 3).
`crates/fleet-core/src/wire_contract.rs` states the rule verbatim — bump for "a
command the desktop now routes to a hub tool that did not exist before (an older
hub's router answers unknown tool, which no `#[serde(default)]` can soften)".
`session_share` / `session_unshare` / `session_narrow` / `session_access` /
`my_grants` are exactly revisions 5 and 6's precedent (`add_project` /
`list_github_repos`; `catalog_admin`). `capture_session` rides the same bump
without needing it — the tool already exists on the hub; only the desktop
command is new. `CONTRACT_REVISION` 6 → 7 with a revision-7 history entry;
`src-tauri/src/backend/contract.rs`'s `MIN_HUB_CONTRACT` and `MAX_HUB_CONTRACT`
6 → 7, each with the established paragraph of doc comment saying what a
revision-6 hub does wrong. **One bump covers the whole milestone** — the new
`SessionRow` fields are "a new field" and the rule explicitly says not to bump
for those. State in the PR that there is deliberately no mixed window: hub and
desktop upgrade together.

**SCOPE ADDITION — `capture_session` becomes a routed desktop command**
(R5-e). `capture_session` and `session_transcript` are MCP-only: neither
name appears in `src-tauri/src/backend/verdicts.rs`, confirmed against
`src/lib/hub_verdicts.generated.json`, where `session_conversation`,
`session_conversations`, `session_history`, `session_activity`,
`session_tool_detail` and `related_sessions` are all present and those two are
not. So DoD 3 and 4 — "B can watch it" — are **unreachable on the desktop** as
things stand: a watcher gets the conversation panel and no view of the live
pane, because the live pane is `pty_open`, which sharing must not confer. This
was not planned work; it is the minimum a watcher needs, and it is named as an
addition rather than allowed to look like it was always there.

`capture_session` is the one command added **for the watcher**. It is `Routed`,
`Read` reach, and it gives a watcher a read-only pane snapshot in place of the
terminal. **`session_transcript` is deliberately not added**: the desktop's
conversation
panel (`src/lib/conversation.ts`) already reads `session_conversation`,
`session_conversations`, `session_tool_detail` and `session_activity`, all
routed, and they are richer than the raw transcript. Adding a second transcript
path would be a second thing to gate.

**`my_grants` is the other routed command, and R6-j is why it is needed.**
Access is derived on the client from three inputs: the row's `owner_person_id`
and `visibility`, which arrive on every row already, the client's own person id
and its own grant set, which arrive here, and T9's `grant:changed` to keep the
set current. Without it the desktop has no way to tell a `watch` row from a
stranger's row it happens to have been sent, and F1's mount condition has
nothing to read. It is `Routed`, `Read` reach, and on a standalone desktop its
`None` arm is the same service call — the personal owner and, normally, an
empty grant list.

**A new routed command needs seven things, not the one row spec §5 claims:** the
`VERDICTS` row (`src-tauri/src/backend/verdicts.rs`, in the `── sessions ──`
group); registration in `generate_handler!` (`src-tauri/src/lib.rs`); a one-line
body reaching a `routed::` helper (the `rename_session` shape in
`src-tauri/src/commands/sessions.rs`); a `route("<command name>")` string
literal in a file listed in `SOURCES`; a driving `Case` in
`src-tauri/src/backend/tests_routing.rs`'s `routed_read_cases()` or
`routed_mutation_cases()` with a fake payload that deserialises into the
command's real return type; the tool present in `TOOL_POLICIES`; and the regens.
**Budget real time for the `Case` payloads** — six routed commands now
(`session_share`, `session_unshare`, `session_narrow`, `session_access`,
`capture_session`, `my_grants`), plus `claim_session` as `LocalOnly`.

Decide and write down the `None` arm of each `routed::` helper: on a standalone
desktop it is the same service call, not `E_UNSUPPORTED` — migration 086 gives a
standalone hub a personal owner, so grants are meaningful locally.

**`claim_session` is `LocalOnly`, not `Routed`:** parity fails on the *caller*
dimension, not the argument dimension — a desktop's client token can never
satisfy a `HostToken` tool. Its `instead` sentence names
`fleet-hub session claim <id> --person <name>` and the in-session agent.
**It is not UI-reachable** (see F3): the unclaimed surface is a count with no
rows and no expand, so nothing in the desktop has a session id to pass, and
spec §4.3 states the prohibition directly — "A UI 'claim' button for an
arbitrary org member is exactly what must not exist."

Correct `pty_write` / `pty_resize` / `pty_close` / `pty_drain`'s stale `why`
text in `src-tauri/src/backend/verdicts.rs`, which claims `pty_open` is refused
when it is `SameInBoth`, and `pty_open`'s own claim that "the terminal pane
declines that one itself", which `src/lib/TerminalView.svelte` contradicts — T7
and F1 are building the first pre-attach decline that has ever existed.

**Three regen cycles**, each of which writes the file and then panics on purpose
so the diff is read; run, read `git diff`, unset, re-run clean:

- `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen` — rewrites
  `src/lib/hub_verdicts.generated.json` and the refusal table in `docs/hub.md`,
  whose generated summary line moves with the new commands;
- `REGEN_LOCAL_ONLY=1 cargo test -p claude-fleet --lib every_local_only_message_is_the_one_the_fixture_records`
  — for `claim_session`'s new refusal message;
- `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` — revision 7
  plus `SessionKilledPayload`'s new keys from T9.

This is the Tauri build and needs the system libraries.

**Green at the end / breaks, update in this task.** In
`src-tauri/src/backend/tests_routing.rs`: `every_command_has_a_verdict`,
`every_commands_body_does_what_its_row_says`,
`every_route_names_a_command_the_table_can_route`,
`every_refusal_names_a_command_the_table_can_refuse`,
`every_routed_row_is_driven_by_a_case`,
`every_routed_tool_is_a_tool_the_hub_serves`,
`every_local_only_message_is_the_one_the_fixture_records`. In
`src-tauri/src/backend/tests_verdict_gen.rs`: `generated_json_is_current`,
`doc_table_is_current`, `every_command_lands_in_exactly_one_bucket`. In
`src-tauri/src/backend/tests_contract.rs`:
`the_hubs_field_names_are_the_ones_the_desktop_reads` and the `MIN` /
`MAX_HUB_CONTRACT` fit tests.

---

### T14 — The session isolation matrix, enumerated from the tool router

**Toolchain** rust · **Depends on** T13 · **Parallel-safe with** F1, F2, F3, T15

**Files** `mcp/tools/tests_sessions_isolation.rs` (new), `mcp/tools/mod.rs`,
`mcp/tools/tests_isolation.rs`

**Do.** The precedent revision 3 pointed at does not cover sessions:
`mcp/tools/tests_isolation.rs`'s coverage assertion builds `want` from
`WORK_ACTIONS` ∪ `WORK_LINK_ACTIONS` ∪ `AdminAction::NAMES` — the three work
tools' actions only — and the test its own doc comment names
(`every_action_has_a_matrix_row`) **does not exist**; the assertion is inline in
`run_matrix`. So this is a **new sibling file**, not an extension.

Copy the shape that makes it work: `Matrix::row(tool, args, expect)` inserting
into a `covered: BTreeSet<&str>`; `call`'s hand-written dispatch with
`other => panic!("no harness arm for {other}")` — the per-tool arm is what turns
a forgotten tool into a panic instead of a silent gap; `same_as_unknown` as the
no-existence-oracle assertion; and that file's multi-org fixture to build on.

`Who` becomes { owner, watcher, driver, stranger, host token on the row's host
with a proven pane, host token on the row's host without one, host token
elsewhere, master, unbound legacy device }. **A proven pane is a property of the
caller's request, not of a call's arguments** (R6-i), so the harness sets it
where it sets the token — one `X-Fleet-Pane` on the caller fixture, which is
what makes the "with a proven pane" row apply uniformly to every tool in the
derived set rather than to the three that once took the argument. The forbidden
markers are the
private row's `tmux_name`, `friendly_name`, `last_prompt`, `notes`, `tags`,
`cwd` and `claude_session_id`, asserted absent from **any** answer text.

**The coverage set must be derived, not hand-written:** enumerate
`FleetTools::tool_router_for_doc().list_all()` — the source
`mcp/tools/tests.rs::every_router_tool_has_exactly_one_tool_policy_row` already
uses — filtered to the session-addressed and session-reachable set, so a tool
added later with no row fails the build. That set must include the surfaces T7
and T10 added and the original lists omitted: `list_tasks`, `wait_for_task`,
`cancel_task`, `dispatch_task`, `session_history`, every `repo_*`,
`list_worktrees`, `list_host_worktrees`, `discover_lost_sessions`,
`restore_host_sessions`, `send_message`, `broadcast_prompt`, `usage_report`,
`fleet_health`, `work`, `work_link`, `spawn_review`, `move_session`,
`rewind_conversation`, `capture_session` and the six new sharing, grant-set and
claim tools. One arm per tool in the derived set — that is the real deliverable of the
enforcement work, and the derivation is what keeps it honest as the router
grows.

**Green at the end.** `the_session_matrix_holds_for_every_session_addressed_tool`;
`every_session_addressed_tool_has_a_matrix_row`, the derived coverage assertion,
which fails on an unclassified tool;
`a_watcher_and_a_stranger_are_told_apart_only_by_E_FORBIDDEN_vs_E_NOTFOUND`;
`a_driver_is_refused_every_owner_only_tool`, driven from the same spec §4.3 list
T7 cites.
`mcp/tools/tests_isolation.rs::the_isolation_matrix_holds_with_sessions_shared`
and `..._with_org_b_isolating_sessions` stay green — the org matrix is unchanged
by design.

---

### T15 — hub-e2e: two people on one hub, end to end

**Toolchain** rust · **Depends on** T13 · **Parallel-safe with** T14, F1, F2, F3

**Files** `scripts/hub-e2e.sh`

**Do.** Add the section to the
`== Client access (pairing, /events, a client token's reach)` block on **hub A**,
**before that block's `stop_hub a`**. Anchor on `stop_hub a` itself, not on a
line number: revision 4 named a line that is already past the shutdown, and an
implementer following it writes checks against a dead hub. Hub A has
`--local-host true`, a fixture project and a live `/events` subscriber pattern
in that same block. Do **not** put it in the hub-W block — that is gated on
`$WBIN` (an `--features e2e` build), `jq` and the fake tracker, and skips
outside CI.

Copy the rate-limit dodge from the W block instead:
`redeem_as() { curl … -H "X-Forwarded-For: $2" … }` works because `limiter_key`
believes the last XFF hop from a loopback peer (`mcp/pairing.rs`), so two
simulated people each get their own bucket and no `sleep` is needed between
pairings (`POST /pair` allows one attempt per address per 6 s).

Use the file's own `check "<sentence>" '<predicate>' "<detail>"` form, and a `bad`
line in an else-branch when a precondition is missing, so it fails loudly rather
than skipping.

The script executes the acceptance list: pair two clients as two people; A
starts a session; B's `list_sessions` does not contain it and names nothing
about it; `session_share` watch-only; B sees it, `capture_session` succeeds and
`send_prompt` is refused; **B's `my_grants` names that session at `watch` and
nothing else, and names it no longer after the revoke** — the grant set is what
the desktop derives access from (R6-j), so an acceptance run that never reads
it is not exercising the mechanism the UI depends on; B cannot `session_share`
A's session; B is refused every tool the spec's §4.3 `own` invariant names;
`session_narrow` lowers
`drive` to `watch` and no call raises it back; with two people on the hub the
per-host unclaimed count is absent from `list_hosts` for both of them and
`fleet-hub session unclaimed` prints it; a long poll B started before the revoke
does not return the session after it; and B's open `/events` stream stops
receiving it **and does not carry it on the resumed reconnect** — the resume
half is what T9 fixed and what a stream-only assertion would miss.

**Green at the end.** `scripts/hub-e2e.sh` (opt-in via
`scripts/ci-local.sh --hub-e2e`); the script's own `passed $PASS, failed $FAIL`
tally is the contract.

---

### F1 — Frontend: the new row fields, the terminal / attach / upload gate, and the watcher's pane view

**Toolchain** frontend (pnpm) · **Depends on** T0 · **Parallel-safe with** every
Rust task

**Files** `src/lib/sessions.ts`, `src/lib/access.ts` (new),
`src/lib/access.test.ts` (new), `src/lib/events.ts`, `src/lib/session_view.ts`,
`src/lib/TerminalView.svelte`, `src/lib/WatchView.svelte` (new),
`src/App.svelte`, `src/lib/SessionDetails.svelte`,
`src/lib/TerminalView.test.ts`, `src/lib/TerminalView.hub.test.ts`,
`src/lib/WatchView.test.ts` (new), `src/App.hosts.test.ts`,
`src/lib/SessionDetails.test.ts`, `vitest.setup.ts`

**Do.** Pure pnpm — it type-checks and `pnpm test` passes with no cargo run, so
**start it on day one alongside the Rust chain**.

Add `visibility?: 'private' | 'unclaimed'` and
`owner_person_id?: number | null` to the `SessionRow` interface in
`src/lib/sessions.ts`. **There is no `'org'`** (R5-b). **There is no
`my_access`** (R6-j). **Both must be optional:** there is no shared
`SessionRow` test factory — seven files roll their own (`selection.test.ts`,
`TerminalView.test.ts`, `TerminalView.hub.test.ts`, `QuickSwitcher.test.ts`,
`quick_switcher.test.ts`, `work_keys.test.ts`, `conversation.test.ts`) and many
more use the type as a value — and the repo's convention for newer backend
fields is `?:`, as `stale_working_at?`, `row_version?` and `work_rev?` on the
same interface show.

**Access is derived, in one place: `src/lib/access.ts`** (R6-j). The row is not
the carrier — the event bus serialises a bare `SessionRow` with no caller,
`strip_nulls` removes an absent key, and `createRowStore` replaces a held row
wholesale on merge, so a per-caller field would be erased by the next routine
`session:updated` and, defaulting fail-closed on a paired desktop, would shut
the **owner's** own terminal. The module holds:

- `myPersonId: number | null` and `myGrants: Map<number, 'watch' | 'drive'>`,
  both filled by one `my_grants` call at startup and after every resync, and
  both patched by T9's `grant:changed` frame — add it to `src/lib/events.ts`
  beside the existing subscriptions (`frontend_declares_every_event_name` is the
  Rust-side pin that the name matches);
- `sessionAccess(row, backend): 'own' | 'watch' | 'drive' | null`, evaluated in
  this order, first match winning:

  1. the backend is **`Local`** → `'own'`. `Backend::owns_the_fleet()` is
     exactly the assertion "this process is the master", the master resolves to
     the hub's personal owner (T6), and a standalone desktop's `list_sessions`
     returns that person's rows and its `unclaimed` ones and nothing else. So
     every row it holds is its own, and Attach keeps working for every
     single-user install **without `my_grants` having answered at all**;
  2. the backend is `Unavailable`, or is `Remote` and `myPersonId` is null →
     **`null`**, fail-closed — and the surface says the hub is unreachable,
     never that the session is not yours;
  3. `row.owner_person_id != null && row.owner_person_id === myPersonId` →
     `'own'`;
  4. `myGrants.get(row.id)` → `'watch'` or `'drive'`;
  5. otherwise → `null`.

  Step 3 requires `owner_person_id` to be **present**: `strip_nulls` removes a
  null on the way out, so "absent" and "unowned" are indistinguishable on the
  wire and neither may read as ownership. This is the same reason T3 keys the
  stream fence on `visibility` rather than on `owner_person_id`.

  Putting `Local` first is also what keeps F1 safe to land early. It is written
  and tested on pnpm before `my_grants` and `grant:changed` exist in the
  backend, so mid-chain a standalone desktop must not depend on either — under
  this order it does not, and a paired desktop mid-chain fails closed, which is
  the safe direction.

**The backend discriminator is `Backend::owns_the_fleet()`**
(`src-tauri/src/backend/mod.rs`, `matches!(self, Backend::Local)`) — the
existing answer to "this process is the master", in its three states:
**Local**, **Remote**, **Unavailable**. Revision 4 invented a two-armed "split
by backend mode" and left `Backend::Unavailable` unaddressed at the one place
the document decides whether a terminal opens. `Unavailable` renders "the hub
is unreachable", never "this session is not yours": they are different problems
and the user acts differently on them.

**Say it where it could be misread.** This derivation is for the UI. The hub
enforces independently, on every request, and is unaffected by what the client
computed — a client that got it wrong gets refusals, not access. The terminal
gate is client-side because `pty_open` reaches the host over SSH with no hub in
the path; that is a property of the terminal, not a weakening of the model.

**There is no Attach button to hide** — DoD 4's revision-3 wording was wrong
about the UI. The terminal is a pane that attaches automatically on selection:
`src/App.svelte` mounts `<TerminalView />` in the `{:else}` of its
`{#if $selectedSession && selNoPane}` branch, and
`TerminalView.svelte::openTerm` fires off `$selectedSession` and calls
`invoke('pty_open')` with no gesture. So:

- **(a)** add a third branch beside `selNoPane`, keyed on
  `sessionAccess(row, backend) !== 'own'`, so the component is never mounted for
  a granted session — the existing, tested precedent for "this row has no
  terminal";
- **(b)** an early return in `openTerm` **before** its `repair_session` probe,
  which respawns tmux and re-adds worktrees and must not run for a session you
  may only watch;
- **(c)** an `$effect` that calls `pty_close` the moment `sessionAccess` stops
  answering `'own'`, because a revoked grant otherwise leaves a live read/write
  channel into the owner's pane that the hub cannot reach and that only clears
  on a 30 s-throttled focus re-list. **Its input is the derivation, so it must
  re-run on all three of its sources** — the row, `myPersonId` and `myGrants` —
  and the one that actually fires on a revoke is the grant map, patched by
  `grant:changed`. Reading only the row would never fire: a revoke changes no
  column on it;
- **(d)** `src/lib/session_view.ts::resolveSessionView` is a trap — its fallback
  for a session with no `claude_session_id` is the **terminal**, so the grant
  check must be a third argument evaluated **before** the `!hasClaudeId` line,
  not after.

**What a watcher sees instead** (R5-e's frontend half). The third branch
mounts `WatchView.svelte`: a read-only pane snapshot from the newly routed
`capture_session` (T13) on a poll, with no input handling, no resize, no drop
handler and no `pty_*` call of any kind — plus the conversation panel, which
already works for a watcher because `session_conversation`,
`session_conversations`, `session_tool_detail`, `session_activity` and
`session_history` are all routed today. Without this branch DoD 3 and 4 have no
UI deliverable and T15's MCP-level acceptance would pass with the desktop
showing a watcher an empty row.

Reuse the dead testid `terminal-no-attach`, which `TerminalView.hub.test.ts`
asserts null four times against a component that renders it nowhere; those four
vacuous assertions become real.

**Two more surfaces** the spec's "sharing never confers a terminal" missed:
`src/lib/SessionDetails.svelte` renders a section headed "Attach from another
terminal" containing `tmux attach -t ${session.tmux_name}` in a
`<code data-testid="attach-command">` with a copy button — it hands a watcher
both the incantation and the tmux name, which §4.3 calls content — and
`upload_to_session`, the terminal pane's drop handler in
`src/lib/TerminalView.svelte`, which is `SameInBoth` and scps arbitrary files
onto the owner's host with no hub in the path. Gate both on
`sessionAccess(row, backend) === 'own'`.

Add `my_grants` (returning `{ person_id: 1, grants: [] }`), `session_access`
(returning `[]`, not `null`), `capture_session` and the other new commands to
the `invoke` mock chain in `vitest.setup.ts`.

**Green at the end.** An `access.test.ts` table test over `sessionAccess`, whose
rows include the three traps: a row with `owner_person_id` absent from the frame
is **not** `own` on a remote backend (the `strip_nulls` case); an `unclaimed`
row is `own` on `Local` and `null` on `Remote`; and every row is `own` on
`Local` **with `myPersonId` still null**, which is the mid-chain state and the
one that decides whether a single-user install keeps its terminal. A `watch` / `drive`
row mounts no `TerminalView` and renders `WatchView` with the reason; a
`WatchView.test.ts` pinning that it calls `capture_session` and never
`pty_open`; **a `grant:changed` revoking the grant closes an attached PTY with
no `session:updated` in between** — the assertion that would have failed under
a field on the row, since a revoke changes no column; the attach-command section
and the drop handler are absent for a non-owned session; `attach-command` /
`copy-attach` gated on ownership (`SessionDetails.test.ts`); a new component
test in the `hub_verdicts.test.ts` *spirit* pinning "the attach path consults
`sessionAccess`" — note that file itself pins only **name lists** and has no
mechanism for component behaviour, so this must be a real component test, not
that pattern.

**Breaks, update in this task.** `TerminalView.test.ts`'s
`repair_session runs before pty_open` and every `calls('pty_open')` assertion;
`TerminalView.hub.test.ts`'s paired-desktop block — a paired desktop still
attaches every **owned** session. The fixtures do not gain a field: give each
test a `myPersonId` matching its rows' `owner_person_id`, or a `Local` backend,
or the `pty_open` assertions in `TerminalView.test.ts`,
`TerminalView.hub.test.ts` and `App.hosts.test.ts` all fail. Seeding the module
once per suite is the smaller diff than touching every row literal, and it is
also the honest shape: the client's identity is one value, not a per-row one.

---

### F2 — Frontend: Share sheet, badges and the unclaimed count

**Toolchain** frontend (pnpm) · **Depends on** F1 · **Parallel-safe with** every
Rust task except T9 and T13

**Files** `src/lib/share.ts` (new), `src/lib/ShareSheet.svelte` (new),
`src/lib/sessions.ts`, `src/lib/SessionRowItem.svelte`,
`src/lib/SessionDetails.svelte`, `src/App.svelte`, `src/lib/Sidebar.svelte`,
`src/lib/hosts.ts`, `src/lib/sidebar_index.ts`, `src/lib/ShareSheet.test.ts`,
`src/lib/SessionRowItem.test.ts`, `src/lib/work_scale.test.ts`

**Do.** pnpm only; it can be authored while the Rust chain runs, but it shares
three files with F1 so it follows it.

The sheet has an exact precedent: copy
`transferSheetFor = writable<number | null>(null)` from `src/lib/moves.ts`
verbatim into a new `src/lib/share.ts` as `shareSheetFor`, and
`TransferSheet.svelte` — one app-wide instance,
`const id = $derived($shareSheetFor)`, `<Modal>` as the dialog primitive, a
self-closing effect when the target vanishes — as the structure of
`ShareSheet.svelte`. Mount it beside `<TransferSheet />` in `src/App.svelte` and
open it from a `share-from-details` button in SessionDetails' actions block,
mirroring `move-from-details`.

The sheet shows the recipient picker (**person only in M1**), watch/drive, the
live grant list from `session_access`, revoke and narrow — and says
**explicitly** that the recipient also gets the history from before the share,
and that watch/drive are enforced by Fleet and not by SSH. Making it visible
rather than silent turns it into a decision the sharer takes knowingly.

Add the mutation wrappers (`shareSession`, `unshareSession`, `narrowShare`,
`sessionAccess`) to `src/lib/sessions.ts` beside the existing ones, in the same
`invokeCmd` + optimistic `acceptCommandRow` shape. **No `claimSession`
wrapper:** `claim_session` is `LocalOnly` and not UI-reachable (T13, F3).
`my_grants` is **not** one of these: it belongs to `src/lib/access.ts` (F1),
which owns the client's own identity and grant set, and it patches no row.

**The store contract is not what CLAUDE.md says.** `mergeOne` / `removeOne` do
not exist anywhere. The real API is `src/lib/row_store.ts::createRowStore`
wrapped in `src/lib/sessions.ts` as `mergeSession`, `removeSession`,
`applySessionEvents` and `acceptCommandRow`, plus an undocumented monotonic
guard, `sessionIsStale`, which **drops any payload whose `row_version` is lower
than the held row's** — so a share mutation's return value must carry
`row_version` or the optimistic patch is silently discarded. **This is also the
mechanism R6-j names:** `createRowStore` replaces a held row wholesale, so a
per-caller field on the row would be erased by the next `session:updated` that
arrives for any other reason. The sharer's own optimistic patch is a row patch —
the *grant list* it just changed is `session_access`'s answer and
`src/lib/access.ts`'s map, neither of which lives on the row.

A private/shared badge goes in `SessionRowItem`'s line-1 chip strip next to
`WorkChip`, the component precedent. The badge reads `row.visibility` and
`row.owner_person_id` — facts about the row — so it is the same for every
viewer and needs no derivation.

Every control in the row's action block and in SessionDetails' actions must
consult a **new per-session predicate**:
`src/lib/hub.ts::hubActionBlocked(action, status, conn)` takes an action *name*
and the connection state and **no session**, so a per-session refusal cannot
ride it without changing every call site. Write
`sessionActionBlocked(session, action)` as a parallel predicate and compose the
two. Its access input is `sessionAccess(session, backend)` from
`src/lib/access.ts` (F1) — **not a field on the session** (R6-j) — which means
it must be evaluated in a reactive position, so a `grant:changed` re-disables
the buttons without a re-list.

**The bulk paths revision 3 omitted entirely:** Sidebar select mode fans
`killSession` over `selectedRows` and `BulkPromptDialog` fans `sendPrompt` —
exclude rows the caller may not drive, and rows the spec's §4.3 `own` invariant
puts out of reach, the way outside-fleet rows are already excluded as read-only.
Both filter through the same `sessionActionBlocked`, so there is one rule and
not a second copy of the level table.

**The per-host unclaimed count is a count with no rows and no expand**, rendered
in the Sidebar beside the `outside-fleet-section`. It cannot come from
`src/lib/hosts_view.ts::sessionCounts`, which derives every host badge from rows
the client holds; read `unclaimed_sessions?: number | null` from `HostRow`
(`src/lib/hosts.ts`). **R5-d means it is frequently absent**: on a hub
with more than one person the backend serves `null`, not `0`. Render nothing at
all in that case — not "0", not a dash — because a zero is a claim about the
host, and the surface must not let a second person infer one.

**There is no visibility control on the new-session sheet.** Revision 4 put
`visibility?: 'private' | 'org'` into `NewSessionArgs` behind a
NewSessionDialog control; R5-b removes `'org'` from M1 entirely, and
with it the only choice that control offered. A session started through fleet is
private to its owner by default (rule 1) and there is nothing to pick.
`NewSessionDialog.svelte` and `NewSessionArgs` are therefore untouched by this
task.

Anything added to `src/lib/sidebar_index.ts`'s `rowMatches` /
`sessionFilterRow` must stay O(1) per row.

**Green at the end.** A new `ShareSheet.test.ts`, modelled on
`src/lib/ForkSheet.test.ts` (the smallest sheet test in the repo);
`SessionRowItem.test.ts` / `SessionRowOrg.test.ts` / `SessionRowWork.test.ts` —
the badge and the disabled action set; bulk kill and bulk prompt skip a
watch-only row; a `grant:changed` narrowing `drive` to `watch` disables the
drive actions with no `session:updated` and no re-list; the unclaimed count
renders as a count when the backend sends one and renders nothing when it sends
`null`.

**Breaks, update in this task.** `src/lib/work_scale.test.ts` — the **only**
wall-clock budgets in the frontend (`buildSessionsByWork` + `sortWorkGroups`,
`rowMatches`, `sessionWorkRow` and the Today model, each with its own p95
ceiling). Extend its 2,000-row fixture with `owner_person_id` / `visibility` and
keep the per-row callback-count assertions true. Note that `sessionAccess` is
now on the render path for 2,000 rows: it must stay O(1) per row, which it is —
two field reads and one `Map` lookup — and the fixture should give the grant map
a non-trivial size so the budget measures the lookup rather than an empty map.

---

### F2a — Frontend: the surfaces that compose only the hub half

**Toolchain** frontend (pnpm) · **Depends on** F2 · **Parallel-safe with** every
Rust task

**Why this task exists.** It was not in revision 6. F1 and F2 gated the surfaces
the plan named, and an adversarial "play the watcher" review of the result then
found roughly ten more that ask `hubActionBlocked(...)` and never
`$sessionBlocked(...)` — so the control is live for a grantee and the refusal
arrives as a raw `E_FORBIDDEN` toast, which is the exact outcome `share.ts`
exists to prevent. Two of them are live write paths, not cosmetics. The pattern,
rather than the list, is the finding: **a surface that reaches a session
indirectly — by link id, by task id, or through a fan-out list — cannot ask
`sessionBlocked` without a lookup, so every such surface was skipped.**

**Four rows `share.ts::SESSION_TIER` is missing**, decided here because the table
holds the authoritative tier list and these follow its own stated logic:

| Action | Tier | Why, in the table's own terms |
|---|---|---|
| `tidy_apply` | `own` | it can safe-kill, and `safe_kill_session` is `own` |
| `request_work_handover` | `drive` | it types a prompt into the pane, like `send_message { deliver, submit }` |
| `set_primary_work` | `drive` | a per-session work-graph write, like `link_session_work` |
| `decide_work_batch` | `drive` | a batch of `confirm_session_work`, which is already `drive` |

**The two blockers.**

- `TidyReview.svelte`: `blocked` is `hubActionBlocked('tidy_apply', …)` and that
  is the whole gate on `applyRow` and `apply`, so Tidy up can **safe-kill a
  session this client does not own**.
- `PromptComposer.svelte`: gates on the hub half alone and fans `send_prompt` out
  over `$sessions.filter((s) => s.id !== source.id)` — every session in the
  fleet. Its entry button in `SessionDetails` is access-gated, so a watcher
  cannot open it for a shared row, but the target list inside it is not narrowed,
  so anyone can prompt anyone's session from it. The fix is **per-target
  narrowing** with `bulkTargets` (the shape `BulkPromptDialog` already uses), not
  one answer for the whole sheet.

**The four majors.** `WorkReview.svelte` (`decide_work_batch`, which writes
per-session through `confirmSessionWork(it.session_id, …)`), `TicketCard.svelte`
(`request_work_handover` — a watcher can make fleet type into the owner's REPL),
`SessionTasks.svelte`'s `primaryBlocked` (the one line of that file still
hub-half-only, three lines under a `linkBlocked` that composes both and carries
the comment saying why), and `SummarizeButton.svelte` (`summarize_past_work` is
already `own` in the table, but the button is handed a `WorkLink` rather than a
session row, so it needs the lookup).

**The minors.** `ConversationPanel`'s `conv-outgoing-retry` (reachable after a
drive→watch narrow with a failed message still in the outbox);
`TransferSheet`'s `transfer-force-cross-org` and its sibling; the two dialogs
`SessionDetails` opens from a gated button that do not re-ask on confirm, so a
reason arriving while the sheet is open does not reach it; and
`TransferSheet.test.ts`, which tests `confirm-move` but none of the four controls
the previous round newly disabled.

**The rule to apply, once, rather than ten times.** Where a surface holds a
session row, compose both halves — hub refusal over access answer — exactly as
`SessionRowItem` does. Where it holds a batch or a fan-out list, narrow per
target with `bulkTargets`. Where it holds only an indirect id, resolve the
session row first and say in a comment why the lookup is there. Do not invent an
eleventh predicate.

**Green at the end.** Every gate added has a test that fails if the gate is
removed, and a positive control proving the owner keeps the action. A sweep test
that fails when a new surface asks `hubActionBlocked` for a `SESSION_TIER` action
without composing the access half, so the eleventh surface cannot be added
silently.

### F2b — Frontend: invert the sweep, and the writes it was blind to

**Toolchain** frontend (pnpm) · **Depends on** F2a · **Parallel-safe with** every
Rust task

**Why this task exists.** F2a's `share_sweep.test.ts` had sound matchers and the
wrong SELECTION. It keyed on the hub half —

```js
const asked = hubAsks(src);
if (asked.size === 0) continue;   // a surface that asks NEITHER half is invisible
```

— so it audited only surfaces that had already thought about gating, and it was
per FILE and per ACTION rather than per CALL SITE, so it stayed green when a
narrowing was deleted as long as some other line in the same file still named the
action (proved by replaying its matchers with `bulkTargets(displayTargets,
'send_prompt', …)` removed from `PromptComposer`).

**The fix: key the sweep on the WRITE.** `share_sweep.test.ts` now derives, from
the store modules' own source, which exported function invokes each
`SESSION_TIER` command, and fails for any call to one of those functions with no
access answer in reach of that call — the enclosing handler, a gate variable it
reads, a gated runner it is an argument to, or a gate in the writer's own module
(the funnel). The derived map is asserted COMPLETE against `SESSION_ACTIONS`, so a
new tier row needs either a writer or a written reason the frontend has none
(`NO_FRONTEND_WRITER`). Two soundness rules the first draft needed and did not
have: comments are blanked before anything is read (a paragraph naming the gate
stood in for the gate, which hid `ShareSheet`'s and `HostDetail`'s), and only
module-scope declarations count as gate variables (seven handlers' `const r =
await …` merged into one, which hid the restore gate). The F2a sweep is kept as a
second, weaker check.

**What the inversion found that F2a's review had not.** `AnswerPrompt` (the
answer card sends keys into the pane and asked neither half — both callers hide
it, the card itself did not), `BulkPromptDialog` (re-derives its own fan-out from
`targets`, so Sidebar's narrowing did not reach it), `NameWorkDialog` (handed
session ids, wrote for all of them), `Sidebar`'s three confirm dialogs,
`TransferSheet`'s wait view and its preflight effect (a preview is
`move_session`), `WorkReview`'s `runUndo` and `takeBack`, `LinkReview`'s
auto-link Undo toast, `ShareSheet`'s `run`, `ConversationPanel`'s `sendKey`, and
the four store funnels (`moves.ts`, `preflight.ts`, `operator.ts`,
`session_rename.ts`).

**The two blockers the briefing named.** `HostDetail`'s "Restore n lost
sessions…" was gated on NOTHING: `restore_host_sessions` is the batch form of
`recreate_session` (`own`), it was also missing from `hub.ts`'s
`ROUTED_ACTIONS`, and the backend plans for the HOST rather than for the caller —
so the dialog narrows the plan again and says how many rows it leaves alone.
`Sidebar`'s Done section drew `archived · show` with neither half; it is per row
now, because one group's Done can hold rows of more than one owner.

**Where the gate went.** At the funnel wherever a write has several entrances:
`moves.ts` (`startMove` / `retryMove` / `cancelWait` / `resolveMoveRun`),
`preflight.ts` (`requestPreflight`) and `operator.ts` (`restartOperator`) refuse
on their own, so no surface can get them wrong. In the runner wherever a panel
funnels many writes: `SessionRowItem`'s `workAction`, `SessionTasks`' `act` and
`add`, `WorkReview`'s `run`, `LinkReview`'s `decideAt` (which now asks for the
action it is about to perform, not for `confirm_session_work` in both cases).

**`SummarizeButton`, said plainly.** F2a's lookup resolved nothing in the normal
case: the button is documented for ENDED work, whose session is usually gone from
the list, and `sessionActionBlocked(null, …)` answers `null`. `WorkLink` carries
`snap_host` / `snap_tmux` and no person, and the work-link reads answer none — so
it FAILS CLOSED with that sentence instead, except on a desktop that owns its
fleet (which owns every row in it).

**Green at the end.** `pnpm run check`, `pnpm run test`, `pnpm run build`. Each
gate is mutation-verified: removing it turns either its surface's own test or the
sweep red. Where a control's `disabled` makes the handler unreachable from a
click, the test says so and names the sweep as the proof of the handler half
rather than pretending a disabled button exercises it.

### F3 — Frontend: `hub.ts` routed actions and the verdict cross-check

**Toolchain** frontend (pnpm) · **Depends on** T13 **and** F2 ·
**Parallel-safe with** T14, T15, D1

**Files** `src/lib/hub.ts`, `src/lib/hub_verdicts.test.ts`

**Do.** Small, but **strictly ordered after T13's regen**.
`src/lib/hub_verdicts.test.ts` asserts every `ROUTED_ACTIONS` entry of
`src/lib/hub.ts` appears in `src/lib/hub_verdicts.generated.json`, and that
every `REASONS` key that is a command name is `local_only` there — and that JSON
is written only by
`REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`. So the
moment the sharing commands are added to `hub.ts`, a pure-pnpm run goes red
until the Rust side has landed. Neither design document flagged this ordering.

Add `share_session` / `unshare_session` / `narrow_share` to `ROUTED_ACTIONS` so
the Share sheet's buttons disable while the hub link is down; do **not** add
`session_access`, `capture_session` or `my_grants` — the file's own comment says
a routed read has no control to disable and so is not there. `my_grants` failing
while the link is down is already handled by F1's fail-closed `Unavailable` arm,
which is a different surface (the hub is unreachable) from a disabled button.

**`claim_session` is `LocalOnly` and NOT UI-reachable**, so it goes on the
`LOCAL_ONLY_WITH_NO_DIRECT_REASONS_ENTRY` allowlist with a one-line note that
the desktop has no surface for it, rather than getting a `REASONS` entry with
user-facing wording. Revision 4 called it "UI-reachable (the unclaimed-count
surface)" while F2 made that surface a count with no rows and no expand — a
count carries no session id, so there is nothing for a desktop command to pass,
and spec §4.3 forbids the button that would supply one. Without a line either
way, `every local_only command is covered` fails — and the reason that test
exists is that such a command would fail open, or open a dialog whose click dies
with a raw `E_LOCAL_ONLY`.

**Green at the end.** In `src/lib/hub_verdicts.test.ts`:
`every ROUTED_ACTIONS entry, mapped through the name exception, is routed or routed_unless`;
`every other REASONS key that is a command name is local_only in the generated file`;
`every local_only command is covered: either a REASONS key, or on the allowlist`.

---

### D1 — `docs/hub.md`, the privacy statement, and the recovery procedure

**Toolchain** docs · **Depends on** T13 · **Parallel-safe with** T14, T15, F1,
F2, F3

**Files** `docs/hub.md`, `docs/getting-started.md`

**Do.** Must follow T13, which splices the regenerated verdict table into
`docs/hub.md` between its markers — edit the prose around it, never inside.
Write:

- the person/device model and `pair --person`; `fleet-hub client bind-person` /
  `unbind-person`, and that **disabling a person revokes their devices and every
  grant to them**, while leaving their own sessions private and theirs;
- the §4.5 privacy paragraph, including in plain words that there is **no admin
  override** — the hub operator reads the database, a host's unix owner reads
  that host's transcripts, and the terminal attaches over SSH outside the hub, so
  Fleet's privacy is about what the application shows and nothing more;
- §4.4's shared-host deployment rule (separate unix accounts per person, each
  its own host alias), the statement that on a host with one shared unix account
  Fleet-level privacy is cosmetic, and — in the same paragraph, because the two
  stand or fall together — that **the pane proof is exactly as strong as that
  rule**: any process that can run `tmux list-panes` on a host can enumerate its
  pane ids, so the proof works because a separate unix account cannot read
  another's tmux socket, not because a pane is a secret;
- **how the pane proof gets to the hub and what to do when it does not**: the
  agent's own MCP entry sends `X-Fleet-Pane`, written by provisioning, so a host
  provisioned before M1 proves nothing until it is re-provisioned — and **fleet
  does not tell the operator**, because the provisioning fingerprint does not
  cover that entry. So `docs/hub.md`'s upgrade order gains a third step beside
  the 6 → 7 contract bump: upgrade the hub, upgrade the desktop in the same
  window, **then re-provision every host fully** (`provision_hosts`, or
  `fleet-hub provision --host <alias>` — *not* `--content-only`, which by
  contract never rewrites `~/.claude.json`). Until a host is re-provisioned its
  agents are refused every rule that needs the proof, which is safe but is a
  functional regression, so say it as an instruction and not as a footnote.
  Say in the same place that the proof is the session's **active**
  pane, so a claim run from a non-active pane of a split window is refused with
  a message naming that rule, and that nothing about a proof is stored: it is
  re-checked on every request and lapses on its own;
- **what revocation does to each kind of live connection**, as corrected: a new
  request is refused immediately; an open `/events` stream ends within one 15 s
  keep-alive beat — and say that the **beat** is the guarantee while
  `GRANT_GENERATION` is only an optimisation, because it is a process-local
  atomic and a grant written by another process bumps nothing; a long poll
  already in flight is re-checked on every wake and before returning; a prompt
  `run_prompt` has already delivered is **not** recalled; an attached terminal is
  outside all of this. Add what a client *shows* while this happens: the desktop
  derives watch / drive / own locally from the row and its own grant set, and a
  revoke reaches it as a `grant:changed` frame — so the buttons and the terminal
  follow within the same beat. The hub refuses independently of what any client
  computed; the derivation is a display, never a permission;
- **what a watcher sees**: the conversation panel and a read-only pane snapshot,
  and no terminal — and that `capture_session` is a snapshot on a poll, not a
  live pane, so a watcher is always slightly behind;
- **who sees an unclaimed count** (R5-d): the hub's one person when
  there is exactly one; nobody through the API when there is more than one, and
  then `fleet-hub session unclaimed` on the hub machine is the readout;
- the administrative-recovery procedure for an owner who has lost every paired
  device (DoD 11) — `fleet-hub pair` on the hub machine, which is shell access
  the operator has by definition;
- that M1 shares with a **person** only, org-scoped grants and `'org'`
  visibility both being deferred to M2 where memberships exist, and that in M1
  **no admin has any authority over a grant** — revoking or narrowing a departed
  member's grants arrives with memberships (R5-c).

One line in `docs/getting-started.md` that nothing changed for a single user: the
upgrade attributes every fleet-started session to the install's own owner, the
sessions fleet did not start stay visible to that one person exactly as before
(T6), no registration, no prompt.

**Green at the end.** `scripts/ci-local.sh` (markdown and link checks only); the
generated verdict block must be byte-identical to T13's output.

## Deliberately not in M1

- **Team sharing — `session_share { org }` — and `visibility = 'org'` with it.**
  Deferred to M2 by the owner's decision, taken knowingly after the review, for
  a reason that is not a scheduling one: a client's org membership is written by
  `work_admin { assign_client }` (`service/orgs.rs`), which is `Access::Master`.
  An admin would bind their own device to the org and read every org-shared
  session with no grant touched and no owner consent — rule 2 and T4's
  invariants 1 and 3 defeated at once. M1 also has no membership table: "an org
  the person is in" is undefined, and the only candidate,
  `client_tokens.org_id`, is a property of a **device**, which would make a
  person's visibility depend on which device they picked up and would strip that
  device of `Access::Person` (`mcp/auth.rs::is_person_device` requires
  `org_id.is_none()`). `visibility = 'org'` is the same capability under another
  name and is removed with it (R5-b); the `org_id` column
  stays in `session_grants` for M2, the store refuses an org recipient with
  `E_INVALID`, and a test pins it.
- **Any admin authority over a grant** (R5-c). Revoking or narrowing a
  departed member's grants needs a membership to say who has departed from
  what. M1 has none, so it builds none: no store function, no `fleet-hub`
  subcommand, no `Access::Master` arm on the sharing tools.
- **Per-person settings.** T2a binds `Access::Person` to the hub's personal
  owner, which closes the hole a second person opens. "Whose settings are
  these?" is M2's question.
- The space switcher, roles and company-granted permissions, org sync between
  hubs, offline work against a company hub, per-session AI-account choice, and
  any break-glass path for an admin. They are M2–M6 in the spec's §6.

## Risks

| Risk | Mitigation |
|---|---|
| A leak through a path the spec's §5.1 choke points missed | T14's derived matrix, which fails on an unclassified tool; T8's result gate as a genuine fail-closed backstop once it actually runs |
| The terminal bypasses all of this (spec §2.4) | documented; F1 gates the mount, the `openTerm` early return, the `pty_close` effect, the attach-command section and the drop handler; the Share sheet says watch-only is enforced by Fleet, not by SSH |
| **A watcher has nothing to watch.** `capture_session` and `session_transcript` are MCP-only; a desktop watcher gets the conversation panel and no view of the pane, so DoD 3 and 4 are unreachable and an MCP-level acceptance run passes anyway | T13 routes `capture_session` as a named scope addition and F1 builds `WatchView` on it; T14's matrix and T15's e2e both exercise a watcher's reads rather than only its refusals |
| An upgrade hides someone's sessions | T3's Rust backfill covers the single-person case; T6 keeps `unclaimed` rows visible to the hub's one person, so a standalone desktop's Outside-fleet and orphan sections do not empty; on a multi-person hub they surface as a per-host count where R5-d serves one, and are claimable through the in-pane agent or `fleet-hub session claim`; the upgrade chain test in `store/schema/tests_upgrade.rs` |
| An upgrade *widens* access — the failure revision 1 would have shipped | the schema default is `'unclaimed'` and a CHECK constraint admits only that and `'private'`, so `'org'` is unrepresentable rather than merely unreachable; T1 backfills `client_tokens.person_id` and T2 makes a person-less token unmintable, so `person: None` stops being a privilege level; T2a stops `Access::Person` meaning "anybody" the day a second person exists |
| "Revoked" is believed to be instant when it is not | T9 and T11, with the exact bounds in DoD 6 and `docs/hub.md`; `run_prompt`'s already-delivered prompt is called out by name |
| A grant is widened or redirected, turning sharing into a privacy bypass | T4's store-level invariants and the surface test that fails on a new function able to raise a level or change a recipient |
| Unclaimed rows leak names, prompts or projects | count-only where a count is served at all, with `HostRow.unclaimed_sessions` as the only carrier and `null` — never `0` — where R5-d serves none; a test asserts no `SessionRow` field of an unclaimed session reaches any caller who is not its hub's one person |
| **The pane proof is weaker than "not the machine" suggests.** Any process that can run `tmux list-panes` on a host can enumerate its pane ids, so presenting one proves host access; and `sessions.tmux_pane_id` is the session's **active** pane as the last reconcile pass saw it, so an agent in a non-active pane of a split window cannot prove its own row | the deployment rule is the mitigation and D1 says so in the same paragraph: separate unix accounts per person means one person's agent cannot read another's tmux socket. The active-pane constraint is a usability limit on `session_claim`, not a leak; T12 tests the refusal rather than assuming the match, and answers it `E_INVALID_STATE` naming the rule so the operator is not sent hunting a row they can see |
| **A stored pane proof outlives its pane.** Revision 5 kept a durable `(host_alias, pane_id) → session_id` record so the proof could reach more than three tools; keyed by host alias, it makes every pane any agent ever proved reachable by every agent on that host — DoD 9 inverted — and it needs invalidating against the reconcile pass that rewrites `tmux_pane_id` | struck (R6-i). The proof rides the connection as `X-Fleet-Pane`, is resolved per request into `ViewScope::proven_session`, and reaches every tool without being stored anywhere; a stale pane simply resolves to nothing on the next request, and T12's green list drives a reconcile pass between two requests to pin it |
| **A per-caller field on a broadcast row.** Revision 5 put `my_access` on `SessionRow`; the event bus serialises a bare row with no caller, `strip_nulls` erases an absent key, and the frontend row store replaces a row wholesale — so a routine `session:updated` erases it and a fail-closed default then shuts the OWNER's own terminal on a paired desktop | struck (R6-j). The row carries only caller-independent facts (`owner_person_id`, `visibility`); the client holds its own person id and grant set from `my_grants`, keeps them current from T9's `grant:changed`, and derives access in `src/lib/access.ts`. F1's tests pin the two traps — an absent `owner_person_id` is not ownership, and a revoke closes the PTY with no `session:updated` in between |
| A stolen `resume_claude_session_id` resurrects someone else's conversation after the row is reaped | T3's `conversation_owners` table, filled by a trigger on `sessions` so no writer can skip it and no `delete_session` or GC sweep removes it; T10 (b) consults it, not the `sessions` table |
| Scope built in two places and they drift | `Caller::view_scope` is the only constructor; `only_caller_view_scope_constructs_a_view_scope` is a real test, because `mcp/tools/fleet.rs::usage_report` is today's counter-example for the org scope |
| **A lost row is resurrected under its old owner.** `service/sessions/lifecycle.rs::reject_lost_session_name` → `store/sessions.rs::lost_resumable_session_named` refuses only lost rows that hold a `claude_session_id`, so a lost shell row is revived by the upsert's `ON CONFLICT DO UPDATE` with `owner_person_id` intact: person B starting a session under a tmux name person A once used gets a row owned by and readable to A. DoD 7 violated by a live code path, independent of the migration | T5 widens the refusal to any lost row on that host whose owner differs from the caller's person, with a test |
| **`move_session`'s owner carry lands in a soft-fail block.** Every write in that block of `service/move_session/mod.rs` is commented "Soft-fail like new_session: the session is live either way", so a failed carry silently turns a private session `unclaimed` on the target — a move becomes a privacy event | T5 makes the carry a hard failure that aborts the move, with a failure-injection test; grants are dropped on a move by the owner's decision 2, and that is also tested |
| **The schema version is published on the wire.** 086–088 move `known_schema_version()` forward; `crates/fleet-hub/src/main.rs::compat_json` emits `store.schema_to`, a test in the same file holds it, and `scripts/release-manifest.sh` signs the update windows read from the shipped `fleet-hub compat`. Once M1 ships, a hub that has opened an M1 database can never be reopened by a pre-M1 build except read-only | accepted; the downgrade guard already refuses it cleanly (`store/schema/tests_upgrade.rs::an_older_build_refuses_a_newer_database`). Update the compat test with the migrations, and note the one-way step in the release notes alongside the contract bump |
| The contract bump forces a coordinated upgrade | accepted by the owner (decision 3): `CONTRACT_REVISION`, `MIN_HUB_CONTRACT` and `MAX_HUB_CONTRACT` all go 6 → 7 in T13, deliberately with no mixed window, per `docs/hub.md`'s existing rule. T13 budgets the three regeneration cycles the bump forces |
| The one-build-at-a-time rule makes the chain long | F1 and F2 run on pnpm in parallel from day one; the monotonic-narrowing invariant means a partially landed chain is an incomplete feature, never a regression, so the Rust chain can pause at any task boundary |
