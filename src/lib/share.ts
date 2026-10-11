// The owner's side of a share: which session's Share sheet is open, and the
// one table that says which controls a GRANT reaches.
//
// Multi-user M1 (`docs/superpowers/plans/2026-09-30-multi-user-m1-private-sessions.md`,
// spec §4.3). `access.ts` answers *what* this client may do with a row — own
// it, drive it, watch it, or nothing. This module answers the question every
// button in the session surfaces actually asks: *may I click this one?*
//
// ── Why this is a second predicate and not a parameter of the first ───────
//
// `hub.ts::hubActionBlocked(action, status, conn)` already answers "can I
// send this at all", from the hub's side: a client never administers the
// fleet, and a routed mutation needs the live link up. It takes an action
// NAME and the connection state and **no session**, deliberately — so a
// per-session refusal cannot ride on it without changing every call site, and
// a call site that forgot to pass the session would silently read as allowed.
//
// So this is a parallel predicate with the same shape, and a call site
// composes the two with `??`:
//
//     const killBlocked = $derived(
//       hubActionBlocked('kill_session', $hubStatus, $hubConnection) ??
//         $sessionBlocked(sess, 'kill_session'),
//     );
//
// The hub half wins when both answer, which is the same precedence
// `hubActionBlocked` uses internally for refusal-over-offline: "the hub never
// accepts this from a client" is a more useful sentence than "this session
// is not yours", because the first is true of every session.
//
// ── This is the UI's copy of the rule, not the rule ───────────────────────
//
// The hub enforces independently, per request, through its own `Reach`
// (`mcp/tools/support.rs`, task T7), and is unaffected by what this module
// computed: a client that gets it wrong gets refusals, not access. What this
// buys is a disabled button with a sentence instead of a click that dies with
// a raw `E_FORBIDDEN`.
import { derived, get, writable } from 'svelte/store';
import {
  accessOf,
  backendMode,
  myGrants,
  myPersonId,
  sessionAccess,
  type GrantLevel,
  type SessionAccess,
} from './access';
import { hubStatus, unavailableReason, type HubStatus } from './hub';
import { sessions, type SessionGrant, type SessionRow } from './sessions';
import { SHARE_LEVEL_WORDS } from './session_scope';

/**
 * The session whose Share sheet is open, or null — one sheet for the whole
 * app, opened by id. Copied from `moves.ts::transferSheetFor`, which has the
 * same shape for the same reason: a sheet that lives in `App.svelte` can be
 * opened from the row, from the details panel and from a header chip without
 * any of them owning it.
 */
export const shareSheetFor = writable<number | null>(null);

/**
 * Which tier each action this UI can reach needs, from spec §4.3 invariant 5
 * and T7's two rules for everything that invariant does not name:
 *
 *  - **`own`** — exactly the operations the spec's `own` tier lists. That
 *    list is the single authority and is cited, never restated: everything
 *    that *copies, relocates, re-creates, destroys, renames, re-tags or
 *    re-shares* a session. A `drive` grantee may NOT kill, restart or rename
 *    the owner's session — `drive` is "make this machine do work", not
 *    "dispose of it".
 *  - **`drive`** — anything that writes to a pane, a row, a task or a tmux
 *    server and is not in that list (T7: "anything the spec's §4.3 `own`
 *    invariant names is `Own`; anything that writes … is `Drive`; the rest is
 *    `Read`").
 *  - **`answer`** — pressing a dialog's own keys (Orbit Fleet 11.7): one
 *    row, `answer_dialog`, the answer card's and ⌘K Approve's write.
 *  - **`watch`** — a read. Nothing in this table needs it, because a control
 *    that only reads has nothing to disable; the entries exist so a reader can
 *    see that the omission is deliberate.
 *
 * Keys are command names wherever the desktop has one, so a reader can find
 * both the `#[tauri::command]` and the matching `REASONS` key in `hub.ts`.
 */
const SESSION_TIER = {
  // ── spec §4.3 invariant 5: the `own` tier, cited not restated ──────────
  kill_session: 'own',
  // A shell on the owner's host (step 5.3): never through a grant.
  shell_terminals: 'own',
  safe_kill_session: 'own',
  // `inspect_safe_kill` is the read Safe remove opens with, but it is the
  // first step of a kill and offering it to a driver would be a dialog whose
  // confirm button then fails.
  inspect_safe_kill: 'own',
  discard_kill_session: 'own',
  restart_session: 'own',
  // `recreate_session` is also the primitive `restore_host_sessions` batches
  // over, so gating one and not the other gates nothing.
  recreate_session: 'own',
  restore_host_sessions: 'own',
  move_session: 'own',
  // Finish / Undo of a partial move (`E_MOVE_PARTIAL`). Both kill a live
  // session — Undo the half-built target, Finish the source — so neither can
  // be narrower than `move_session` itself. It is judged against the TARGET
  // session's row, because the target is the session `resolve_move` is given
  // and the one it acts on; the source is checked too, at the one call site
  // (`moves.ts::resolveMoveRun`), since Finish kills that.
  resolve_move: 'own',
  spawn_review: 'own',
  // Fork, rewind and retry alike: a permanent verbatim copy of the transcript.
  rewind_conversation: 'own',
  // `resume_work { mode: 'last' }` re-opens a past session's Claude
  // conversation in a NEW session of this person's. That is a take-over of
  // somebody's transcript — the same thing `rewind_conversation` is `own` for
  // — so it is judged against the SOURCE session's row, not the new one.
  resume_work: 'own',
  // Cancel start (task → session P-6) ends the session, so it is a kill.
  abandon_start: 'own',
  rename_session: 'own',
  set_session_tags: 'own',
  decide_related_session: 'own',
  // `work_link { summarize }`: a durable précis of the transcript that
  // outlives the grant.
  summarize_past_work: 'own',
  // Tidy up's apply (`work_link { tidy_apply }`). It can SAFE-KILL, and
  // `safe_kill_session` three lines up is `own`, so the batch form of it
  // cannot be narrower than the single-session one (F2a).
  tidy_apply: 'own',
  // This is where "a grantee cannot grant on" is enforced in front of the
  // control, as well as in the store.
  session_share: 'own',
  session_unshare: 'own',
  session_narrow: 'own',

  // ── writes that are not in that list: `drive` ──────────────────────────
  send_prompt: 'drive',
  // Typed later instead of now (step 5.10); its list and take-back are the
  // same pending input, so a watcher reads none of it.
  queue_prompt: 'drive',
  queued_prompts: 'drive',
  cancel_queued_prompt: 'drive',
  answer_form: 'drive',
  decline_form: 'drive',
  // `send_message { deliver, submit }` is a pane write by another route, so
  // refusing it to a driver while allowing `send_prompt` would be theatre.
  send_message: 'drive',
  dispatch_task: 'drive',
  cancel_task: 'drive',
  // Spec §4.3's reasoning, which this comment used to contradict: READING
  // `friendly_name` is content and a stranger never sees it, but WRITING it is
  // not an owner-only act. `rename_session` moves the row's ADDRESS and
  // `set_session_tags` writes a durable classification; the friendly label is
  // the sidebar CAPTION, which fleet rewrites itself without asking anybody
  // (`label_from_prompt`, `fill_session_name`, `tickets::start_one`) — so it is
  // not a field the owner-only tier is protecting, and a driver correcting the
  // caption of the session it drives changes nothing the owner cannot re-set.
  set_friendly_name: 'drive',
  // The row's one `last_viewed_at` (redesign 2.3): a watcher looking must not
  // clear what the owner has not seen yet.
  touch_session_viewed: 'drive',
  repair_session: 'drive',
  dismiss_ghost_session: 'drive',
  adopt_session: 'own',
  dismiss_agent_session: 'drive',
  delete_worktree: 'drive',
  // It types a prompt into the pane and waits for the reply, exactly like
  // `send_message { deliver, submit }` above.
  request_work_handover: 'drive',
  // The work graph's per-session writes: a link, a rejection, a title.
  link_session_work: 'drive',
  reject_session_work: 'drive',
  unlink_session_work: 'drive',
  confirm_session_work: 'drive',
  // Which of the session's links groups it — a per-session work-graph write,
  // like `link_session_work`.
  set_primary_work: 'drive',
  // Task → session P-2: end one of the session's links and take the
  // primary — the two per-session writes above, in one step.
  switch_session_work: 'drive',
  // Undo of a confirm / reject: the link goes back to a suggestion. A
  // per-session work-graph write like `link_session_work`.
  reconsider_work_link: 'drive',
  // Keep a conflicting link on purpose — the same family, same tier.
  ack_work_link: 'drive',
  // A batch of `confirm_session_work` / `reject_session_work` / an ack, each
  // decided on its own; the batch cannot be wider than its parts.
  decide_work_batch: 'drive',
  name_session_work: 'drive',
  rename_work_item: 'drive',
  set_work_project_trust: 'drive',
  archive_session_work: 'drive',
  unarchive_session_work: 'drive',

  // Orbit Fleet 11.7: pressing one of a dialog's own keys (a numbered option,
  // Enter, Escape, Tab) — the answer card and ⌘K Approve. The hub's
  // `send_prompt` admits it at `answer` for a key alone, after a fresh read of
  // the pane shows a dialog; a prompt stays `drive`.
  answer_dialog: 'answer',
  // Context help at the composer: its answer is text for the box, which a
  // watcher cannot send, and the run spends the owner's account.
  session_context_help: 'answer',

  // ── reads: nothing to disable, listed so the gap is visibly deliberate ──
  capture_session: 'watch',
  session_conversation: 'watch',
  session_history: 'watch',
} as const;

/** Every action this module answers for. */
export type SessionAction = keyof typeof SESSION_TIER;

/**
 * The table's keys, as data — what `share_sweep.test.ts` scans every surface
 * against. Exported from here rather than re-listed there on purpose: a sweep
 * over a hand-copied list only covers the actions someone remembered to add,
 * and the next row added is exactly the one that would be missing.
 */
export const SESSION_ACTIONS: readonly SessionAction[] = Object.keys(
  SESSION_TIER,
) as SessionAction[];

/** The tier `action` needs — read by the sweep test, which checks the four
 *  tiers F2a added against the plan rather than against the table itself. */
export function sessionTierOf(action: SessionAction): SessionTier {
  return SESSION_TIER[action];
}

/** A tier an action needs: a grant level, or `own`. */
export type SessionTier = GrantLevel | 'own';

/** The tiers, narrowest first — the order a level is allowed to satisfy. */
const RANK: Record<SessionTier, number> = { watch: 1, answer: 2, drive: 3, own: 4 };

/**
 * Why `action` is unavailable on `session` **because of who this client is**,
 * or `null` when the client's access is enough for it.
 *
 * `null` for an owned session, always — which is every session on a
 * standalone desktop (`access.ts::sessionAccess` rule 1), so nothing here
 * changes what a single-user install does.
 *
 * `access` is an argument rather than a `get()` because the answer must be
 * re-evaluated when the GRANT MAP moves and the row does not: a revoke
 * changes no column on any session, so a component that read the row alone
 * would keep its buttons enabled until the next re-list. Read it through
 * {@link sessionBlocked} in a reactive position and that happens for free.
 */
export function sessionActionBlocked(
  session: Pick<SessionRow, 'id' | 'owner_person_id'> | null | undefined,
  action: SessionAction,
  access: SessionAccess = sessionAccess(session),
  status: HubStatus = get(hubStatus),
  person: number | null = get(myPersonId),
): string | null {
  if (!session) return null;
  if (access === 'own') return null;
  if (access === null) {
    // `null` is two different states and they need two different sentences —
    // `access.ts` collapses them because its callers only need "no", while a
    // disabled button has to explain itself to the person who clicked it.
    //
    // A third state is folded in here that `noAttachReason` does not have to
    // distinguish: knowing who we are and finding no grant. The hub fences a
    // stranger's row off the stream, so holding one is not supposed to happen
    // — but "not shared with you" is the honest reading when it does, and
    // blaming the hub for it would send someone to Settings → Hub & sync over a row
    // that is simply somebody else's.
    // `hub.ts`'s own sentence, CALLED rather than copied: a disabled button
    // and the watcher's pane (`access.ts::noAttachReason`) owe the same
    // explanation of this one state, and three hand-copies of it were three
    // chances for the next edit to leave two of them stale.
    const unavailable = unavailableReason(status);
    if (unavailable) return unavailable;
    if (person === null) {
      const hub = status.url ?? 'the hub';
      return `${hub} has not said who this device is on the fleet yet, so this app cannot tell whether this session is yours. It disables what it cannot vouch for rather than letting the click fail.`;
    }
    return 'This session belongs to someone else and is not shared with you.';
  }
  const need = SESSION_TIER[action];
  if (RANK[access] >= RANK[need]) return null;
  if (need === 'own') {
    return 'Only the session’s owner can do this. A share grants watch, answer or drive; starting, stopping, moving, renaming, re-creating, summarising or re-sharing a session stays with the owner.';
  }
  if (access === 'answer') {
    return 'Shared with you to answer. Answer lets you reply to the questions the session asks; sending a prompt, typing into the pane or changing the row needs drive, which only the owner can grant.';
  }
  if (need === 'answer') {
    return 'Shared with you to watch. Watch is read-only: answering the session’s questions needs answer or drive, which only the owner can grant.';
  }
  // `need` is `drive` and `access` is `watch`.
  return 'Shared with you to watch. Watch is read-only: sending a prompt, typing into the pane or changing the row needs drive, which only the owner can grant.';
}

/**
 * `(session, action) → reason | null`, with the access derivation already
 * applied — the shape a component wants, because reading it in a reactive
 * position subscribes to the row's owner, this client's person id, its grant
 * set and the backend mode at once. Modelled on `access.ts::accessOf`, and
 * for the same reason: `sessionActionBlocked`'s `get()` default creates no
 * dependency, so a component that called it directly would never re-disable a
 * button when a grant was narrowed.
 */
export const sessionBlocked = derived(
  [accessOf, hubStatus, myPersonId],
  ([$accessOf, $status, $person]) =>
    (
      session: Pick<SessionRow, 'id' | 'owner_person_id'> | null | undefined,
      action: SessionAction,
    ): string | null =>
      sessionActionBlocked(session, action, $accessOf(session), $status, $person),
);

/**
 * `true` when `action` is allowed on every one of `rows` — the bulk paths'
 * question, asked once so Sidebar's select mode and `BulkPromptDialog` cannot
 * grow a second copy of the level table.
 */
export function bulkTargets<T extends Pick<SessionRow, 'id' | 'owner_person_id'>>(
  rows: readonly T[],
  action: SessionAction,
  blocked: (s: T, a: SessionAction) => string | null,
): T[] {
  return rows.filter((s) => blocked(s, action) === null);
}

// ── a write that names its session only by id ─────────────────────────────
//
// Several writes do not hold a row at all: the outbox carries a session id and
// a tmux name, a work link carries `session_id`, a lost transcript carries
// `existing_session_id` (or nothing). They still act ON a session, so they
// still owe the same answer — and resolving the row out of the store is the
// only way to get one.

/**
 * What a surface says when it cannot find the row a write would act on.
 *
 * This is the fail-closed half and it is the point of the helper. A missing
 * row is NOT "no session, therefore nothing to refuse": on a fleet this client
 * does not own, the hub fences rows it may not see off the stream, so "not in
 * `$sessions`" reads as *someone else's, or gone* — exactly the two cases a
 * take-over must not be offered for. A standalone desktop is excluded by the
 * `local` branch, so nothing a single-user install does changes.
 *
 * F2d made this the ONE place that rule lives. Four surfaces had each resolved
 * the row by hand and let the miss through — `TidyReview`'s `mayTidy`,
 * `WorkReview`'s `decidable`, `moves.ts`'s `moveAccessBlocked`, `TasksPanel`'s
 * `cancelAccessBlocked`, plus `SummarizeButton`'s own copy of the `local`
 * branch. A gate that answers "not blocked" when it does not know is worse than
 * no gate, because it reads as one; and five copies of the exception were five
 * chances for the next edit to widen one of them. Route a write that names its
 * session by id through here instead of resolving the row at the call site.
 */
export const UNKNOWN_SESSION_REASON =
  'This app cannot see the session this would act on, so it cannot tell whose it is. It refuses rather than acting on a conversation that may be someone else’s.';

/**
 * Why `action` is unavailable on the session with id `id`, resolved out of
 * `rows`. `null` when it is allowed.
 *
 * The pure form, for tests and for `get()` callers that are not in a reactive
 * position (the outbox sends from a `.finally`, not from a component).
 */
export function sessionIdActionBlocked(
  id: number | null | undefined,
  action: SessionAction,
  rows: readonly Pick<SessionRow, 'id' | 'owner_person_id'>[] = get(sessions),
  status: HubStatus = get(hubStatus),
  person: number | null = get(myPersonId),
  grants: ReadonlyMap<number, GrantLevel> = get(myGrants),
): string | null {
  const row = id == null ? undefined : rows.find((s) => s.id === id);
  if (row) {
    return sessionActionBlocked(row, action, sessionAccess(row, status, person, grants), status, person);
  }
  if (backendMode(status) === 'local') return null;
  const unavailable = unavailableReason(status);
  return unavailable ?? UNKNOWN_SESSION_REASON;
}

/**
 * `(id, action) → reason | null`, with the store reads already applied — the
 * reactive shape, for the same reason {@link sessionBlocked} exists: a revoke
 * moves the grant map and no row, so a component that resolved the id once
 * would keep its button live.
 */
export const sessionIdBlocked = derived(
  [sessions, hubStatus, myPersonId, myGrants],
  ([$sessions, $status, $person, $grants]) =>
    (id: number | null | undefined, action: SessionAction): string | null =>
      sessionIdActionBlocked(id, action, $sessions, $status, $person, $grants),
);

/** The header's visibility badge (SessionDetails board: "Needs you ·
 *  Private"): who can see this session, as its owner reads it. */
export interface VisibilityBadge {
  kind: 'private' | 'shared' | 'unclaimed';
  text: string;
  title: string;
}

/** A grant's recipient, in words: the person's name, an org's, or its id. */
export function grantRecipient(g: Pick<SessionGrant, 'person_id' | 'person_name' | 'person_display_name' | 'org_id' | 'org_name'>): string {
  if (g.org_id != null) return `${g.org_name || `org ${g.org_id}`} (org)`;
  return g.person_display_name || g.person_name || `person ${g.person_id ?? '?'}`;
}

/**
 * The badge for `row`, or null when there is nothing true to say:
 *
 * * `unclaimed` — fleet found the session and nobody has claimed it;
 * * the owner's own `private` row: "Private" when it is shared with nobody,
 *   "Shared · N" when it has grants. `grants` is the owner's live list
 *   (`session_access`); `null` while it is unread, and then nothing shows
 *   rather than a "Private" that may be wrong;
 * * anyone else's row: null — a recipient's header says how it was shared
 *   with them, not this.
 */
export function visibilityBadge(
  row: Pick<SessionRow, 'visibility'>,
  access: SessionAccess,
  grants: readonly SessionGrant[] | null,
  asks = 0,
): VisibilityBadge | null {
  const badge = baseBadge(row, access, grants);
  // Gap plan G4.2: an open ask for a wider level, on the owner's badge, so
  // the click that opens the Share sheet (where it is answered) is right there.
  if (!badge || badge.kind === 'unclaimed' || asks <= 0) return badge;
  const word = asks === 1 ? '1 ask' : `${asks} asks`;
  return { ...badge, text: `${badge.text} · ${word}`, title: `${badge.title}. ${word} for a wider level` };
}

function baseBadge(
  row: Pick<SessionRow, 'visibility'>,
  access: SessionAccess,
  grants: readonly SessionGrant[] | null,
): VisibilityBadge | null {
  if (row.visibility === 'unclaimed') {
    return {
      kind: 'unclaimed',
      text: 'Unclaimed',
      title:
        'Fleet did not start this session and nobody has claimed it. Until someone does, all anyone sees of it is a per-host count.',
    };
  }
  if (row.visibility !== 'private' || access !== 'own' || grants === null) return null;
  if (grants.length === 0) {
    return {
      kind: 'private',
      text: 'Private',
      title: 'Private: only you can see it until you share it.',
    };
  }
  const who = grants
    .map((g) => `${grantRecipient(g)} (${SHARE_LEVEL_WORDS[g.level as GrantLevel] ?? g.level})`)
    .join(', ');
  return { kind: 'shared', text: `Shared · ${grants.length}`, title: `Shared with ${who}` };
}
