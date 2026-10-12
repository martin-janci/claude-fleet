// Who this client is on its fleet, and what it may do with one session row.
//
// Multi-user M1 (`docs/superpowers/plans/2026-09-30-multi-user-m1-private-sessions.md`,
// spec §4.3): a session started through fleet is PRIVATE to its owner, and
// sharing is an explicit, revocable grant at one of two levels — `watch` or
// `drive`. `own` is a third TIER nobody can be granted: the operations only
// the owner may perform.
//
// ── Why the answer is derived here and is not a field on the row ───────────
//
// An earlier revision of the design had the backend stamp a per-caller
// `my_access` onto `SessionRow`. The pipeline is hostile to one, in three
// independent ways (R6-j):
//
//   1. the event bus serialises a BARE `SessionRow` with no caller at all, so
//      a `session:updated` has nothing to compute a per-caller field from;
//   2. `strip_nulls` removes an absent key on the way out, so "absent" and
//      "null" are indistinguishable on the wire;
//   3. `row_store.ts::createRowStore` replaces a held row WHOLESALE on merge.
//
// Together: every routine `session:updated` — a reconcile tick, a turn
// counter, a context reading — would erase the field, and a fail-closed
// default would then shut the OWNER's own terminal. So the row carries only
// caller-independent facts (`owner_person_id`, `visibility`), this client
// holds its own person id and its own grant set, and access is DERIVED from
// the three. The grant set is fetched once by `my_grants` and kept current by
// the `grant:changed` frame — a grant mutates no column, so a row event alone
// could never carry it.
//
// ── This derivation is for the UI only ────────────────────────────────────
//
// The hub enforces independently, on every request, and is unaffected by what
// this module computed: a client that gets it wrong gets refusals, not
// access. The reason the terminal is gated HERE at all is that `pty_open`
// reaches the host over this machine's own SSH with no hub in the path — so
// there is no server-side moment at which it could be refused. That is a
// property of the terminal (spec §3.4), not a weakening of the model.
import { derived, get, writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { hubStatus, unavailableReason, type HubStatus } from './hub';
import type { SessionRow } from './sessions';

/** The levels a grant may carry, narrowest first. `answer` (Orbit Fleet
 *  11.7) is watch plus answering the dialog on the pane. `own` is deliberately
 *  NOT here: it is a tier, not a level anyone can be given (spec §4.3
 *  invariant 5). */
export const GRANT_LEVELS = ['watch', 'answer', 'drive'] as const;
export type GrantLevel = (typeof GRANT_LEVELS)[number];

/** What this client may do with a row: own it, drive it, watch it — or
 *  nothing, which also covers "we cannot tell yet". */
export type SessionAccess = 'own' | GrantLevel | null;

function isGrantLevel(v: unknown): v is GrantLevel {
  return typeof v === 'string' && (GRANT_LEVELS as readonly string[]).includes(v);
}

/**
 * This client's own person id on the fleet it is a window onto, or null when
 * nothing has told us yet — which is the state a standalone desktop stays in
 * forever (it needs no answer: see `sessionAccess` step 1) and the state a
 * paired desktop is in until `my_grants` replies.
 */
export const myPersonId = writable<number | null>(null);

/**
 * Session id → the level granted TO THIS PERSON on it. Only live grants are
 * in it; a revoke deletes the entry rather than storing a level of `null`, so
 * `get` answering `undefined` is the single "no grant" case.
 */
export const myGrants = writable<ReadonlyMap<number, GrantLevel>>(new Map());

/**
 * The backend's three states, which is what `Backend::owns_the_fleet()`
 * discriminates on the Rust side (`src-tauri/src/backend/mod.rs`):
 *
 *  - `local` — this process IS the fleet. `owns_the_fleet()` is true.
 *  - `remote` — a window onto someone else's fleet through a hub.
 *  - `unavailable` — a hub is configured but THIS launch could not use it, so
 *    the backend owns nothing and refuses every fleet command. It is neither
 *    of the other two, and the difference matters at exactly the place this
 *    module is read: "the hub is unreachable" and "this session is not yours"
 *    are different problems and a person acts differently on them.
 */
export type BackendMode = 'local' | 'remote' | 'unavailable';

export function backendMode(status: HubStatus = get(hubStatus)): BackendMode {
  // Checked first: `remote` is false in this state (nothing is talking to a
  // hub), so asking `remote` alone would read it as standalone — and
  // standalone is the one answer that hands out `own` unconditionally.
  if (status.unavailable) return 'unavailable';
  return status.remote ? 'remote' : 'local';
}

/**
 * What this client may do with `row`. Evaluated in this order, first match
 * winning; the order is load-bearing and is the plan's, not a preference.
 *
 *  1. **`local` → `own`.** `Backend::owns_the_fleet()` is exactly the
 *     assertion "this process is the master"; the master resolves to the
 *     hub's personal owner, and a standalone desktop's `list_sessions`
 *     returns that person's rows plus the `unclaimed` ones and nothing else.
 *     So every row it holds is its own — and the terminal keeps working for
 *     every single-user install WITHOUT `my_grants` having answered at all.
 *     That is also what makes this module safe to land before the backend
 *     half exists: standalone depends on neither `my_grants` nor
 *     `grant:changed`, and a paired desktop fails closed, which is the safe
 *     direction.
 *  2. **`unavailable`, or `remote` with no person id yet → `null`**,
 *     fail-closed. The surface must say the hub is unreachable / not yet
 *     answered, never that the session is not yours.
 *  3. **the row's owner is me → `own`.** This needs `owner_person_id` to be
 *     PRESENT: `strip_nulls` removes a null on the way out, so on the wire
 *     "absent" and "unowned" are the same thing and neither may read as
 *     ownership. (The same reason the backend keys its stream fence on
 *     `visibility`, which is `NOT NULL`, rather than on `owner_person_id`.)
 *  4. **a live grant to me → its level.**
 *  5. otherwise `null`.
 */
export function sessionAccess(
  row: Pick<SessionRow, 'id' | 'owner_person_id'> | null | undefined,
  status: HubStatus = get(hubStatus),
  person: number | null = get(myPersonId),
  grants: ReadonlyMap<number, GrantLevel> = get(myGrants),
): SessionAccess {
  if (!row) return null;
  const mode = backendMode(status);
  if (mode === 'local') return 'own';
  if (mode === 'unavailable' || person === null) return null;
  // `!= null` on purpose: absent and null are both "the hub told us nothing",
  // and `undefined === person` would be false anyway — the explicit guard is
  // here so a later reader cannot "simplify" it into `row.owner_person_id ===
  // person`, which on two absent values would read as ownership.
  if (row.owner_person_id != null && row.owner_person_id === person) return 'own';
  return grants.get(row.id) ?? null;
}

/**
 * `(row) → access`, with the three inputs already applied — the shape a
 * component wants, because reading it in a reactive position subscribes to
 * all three at once. `sessionAccess`'s `get()` defaults do NOT create a
 * dependency, so a component that called it directly would never re-run when
 * a grant was revoked. Modelled on `orgs.ts::scopeOf`.
 */
export const accessOf = derived(
  [hubStatus, myPersonId, myGrants],
  ([$status, $person, $grants]) =>
    (row: Pick<SessionRow, 'id' | 'owner_person_id'> | null | undefined): SessionAccess =>
      sessionAccess(row, $status, $person, $grants),
);

/**
 * Why this client has no terminal for a row, in words the person can act on.
 * Shared by the terminal pane and the watcher's view so the two cannot drift
 * into saying different things about one state.
 *
 * `own` has no sentence: there is nothing to explain.
 */
export function noAttachReason(
  access: SessionAccess,
  status: HubStatus = get(hubStatus),
): string | null {
  if (access === 'own') return null;
  const mode = backendMode(status);
  if (mode === 'unavailable') {
    // NOT "this session is not yours": the hub is the problem. `hub.ts`'s own
    // sentence, CALLED rather than copied — this pane and a disabled button
    // must not drift into saying different things about one state, and a
    // comment promising they match is not a mechanism. The `Not available: `
    // prefix is kept: in a pane with no control attached, the sentence has to
    // open by saying that nothing here works.
    return unavailableReason(status);
  }
  if (access === null) {
    const hub = status.url ?? 'the hub';
    return `${hub} has not said who this device is on the fleet yet, so this app cannot tell whether the session is yours. It shows nothing rather than guessing — reconnect, or reopen the window.`;
  }
  // A real grant. Say why there is no terminal, because the reason is a rule
  // and not a failure: the terminal attaches by this machine's own
  // `ssh … tmux attach`, which the hub is not in the path of and therefore
  // cannot revoke. A share that handed one over could never be taken back.
  if (access === 'answer') {
    return 'Shared with you to answer. You can answer the questions the session asks; a terminal would be a direct SSH session into the owner’s pane that no revoke could reach, so sharing never gives one. The pane below is a read-only snapshot.';
  }
  return access === 'drive'
    ? 'Shared with you to drive. Driving sends prompts through fleet, which the hub can stop at any moment; a terminal would be a direct SSH session into the owner’s pane that no revoke could reach, so sharing never gives one. The pane below is a read-only snapshot.'
    : 'Shared with you to watch. A terminal would be a direct SSH session into the owner’s pane that no revoke could reach, so sharing never gives one. The pane below is a read-only snapshot.';
}

// ── the client's own identity and grant set ────────────────────────────────

/** One entry of `my_grants`. The four optional fields (gap plan G4.2) name
 *  the share for the recipient's header; an older hub sends none. */
export interface MyGrant {
  session_id: number;
  level: string;
  shared_by?: number | null;
  shared_by_name?: string | null;
  granted_at?: number | null;
  via_org?: string | null;
}

/** One of this person's own open asks for a wider level (gap plan G4.2). */
export interface MyAccessRequest {
  id: number;
  session_id: number;
  level: string;
  requested_at: number;
}

/** `my_grants`' answer: who this caller is, and every live grant TO them. */
export interface MyGrantsAnswer {
  person_id: number | null;
  grants: MyGrant[];
  requests?: MyAccessRequest[];
}

/** Who shared a session with this person, and how (the recipient's header:
 *  "Shared by Martin · Read · since 13:20", "via 32bit"). Labels only: the
 *  level that decides anything is `myGrants`'. */
export interface GrantInfo {
  sharedBy: number | null;
  sharedByName: string | null;
  grantedAt: number | null;
  viaOrg: string | null;
}

/** Session id → how its share reached this person. Refreshed with
 *  `my_grants`; a revoke frame drops the entry. */
export const myGrantInfo = writable<ReadonlyMap<number, GrantInfo>>(new Map());

/** Session id → this person's open ask on it ("Asked for Answer"). */
export const myAccessRequests = writable<ReadonlyMap<number, { id: number; level: GrantLevel; requestedAt: number }>>(
  new Map(),
);

/** Replace both halves at once — the only way they are written together, so a
 *  half-applied identity cannot be observed. */
export function setMyGrants(
  personId: number | null,
  grants: readonly MyGrant[],
  requests: readonly MyAccessRequest[] = [],
): void {
  const m = new Map<number, GrantLevel>();
  const info = new Map<number, GrantInfo>();
  for (const g of grants) {
    // A level this app does not know is DROPPED, not coerced: a newer hub
    // adding a third level must read as "no grant" (no terminal, no drive)
    // rather than as the nearest thing we recognise.
    if (typeof g?.session_id === 'number' && isGrantLevel(g.level)) {
      m.set(g.session_id, g.level);
      info.set(g.session_id, {
        sharedBy: typeof g.shared_by === 'number' ? g.shared_by : null,
        sharedByName: typeof g.shared_by_name === 'string' ? g.shared_by_name : null,
        grantedAt: typeof g.granted_at === 'number' ? g.granted_at : null,
        viaOrg: typeof g.via_org === 'string' ? g.via_org : null,
      });
    }
  }
  const asks = new Map<number, { id: number; level: GrantLevel; requestedAt: number }>();
  for (const r of requests) {
    if (typeof r?.session_id === 'number' && typeof r.id === 'number' && isGrantLevel(r.level)) {
      asks.set(r.session_id, { id: r.id, level: r.level, requestedAt: r.requested_at });
    }
  }
  myPersonId.set(personId);
  myGrants.set(m);
  myGrantInfo.set(info);
  myAccessRequests.set(asks);
}

/**
 * Read this client's person id and grant set from the backend.
 *
 * Called once at startup and again after a resync (a hub reconnect the hub
 * could not replay): a gap can have swallowed a `grant:changed`, and a grant
 * set that silently lost an entry is a watcher whose shared session has
 * vanished from reach.
 *
 * Deliberately best-effort at every call site: a failure leaves the previous
 * answer standing rather than widening to a guess, a standalone desktop needs
 * no answer at all (`sessionAccess` step 1), and a backend older than the
 * command simply has none to give.
 */
export async function loadMyGrants(): Promise<Result<MyGrantsAnswer>> {
  const seq = ++grantsLoadSeq;
  const since = grantFrameSeq;
  const r = await invokeCmd<MyGrantsAnswer>('my_grants');
  if (seq !== grantsLoadSeq || !r.ok || !r.value) return r;
  // A `grant:changed` applied while this was in flight is newer than the
  // answer: a revoke stays revoked, a new grant stays (review r07).
  const before = get(myGrants);
  const personId = r.value.person_id ?? null;
  setMyGrants(personId, r.value.grants ?? [], r.value.requests ?? []);
  if (grantFrameSeq !== since && personId === grantFramesFor) {
    myGrants.update((cur) => {
      const next = new Map(cur);
      for (const [sid, at] of grantTouchedAt) {
        if (at <= since) continue;
        const level = before.get(sid);
        if (level === undefined) next.delete(sid);
        else next.set(sid, level);
      }
      return next;
    });
  }
  return r;
}

let grantsLoadSeq = 0;
// Every applied grant frame bumps `grantFrameSeq` and notes it per session,
// so a `my_grants` answer can tell which entries a frame overtook.
let grantFrameSeq = 0;
let grantFramesFor: number | null = null;
const grantTouchedAt = new Map<number, number>();

/** The `grant:changed` frame: ids only, `level: null` for a revoke.
 *  `request` (gap plan G4.2) is the level of an ask that just opened. */
export interface GrantChanged {
  session_id: number;
  person_id: number;
  level: GrantLevel | null;
  request?: GrantLevel;
}

/**
 * Read one `grant:changed` payload; `null` for anything malformed, which the
 * caller drops (the `parseWorkChanged` precedent in `work_view.ts`).
 *
 * A `level` this build does not understand becomes a REVOKE rather than a
 * dropped frame. The frame says this grant moved, and the one direction a
 * grant may move is downward (spec §4.3 invariant 3), so the only safe
 * reading of a level we cannot act on is "no longer a level we may act on".
 * Dropping the frame instead would leave the old, wider level in the map.
 */
export function parseGrantChanged(payload: unknown): GrantChanged | null {
  if (!payload || typeof payload !== 'object') return null;
  const p = payload as Record<string, unknown>;
  if (typeof p.session_id !== 'number' || typeof p.person_id !== 'number') return null;
  const level = isGrantLevel(p.level) ? p.level : null;
  const out: GrantChanged = { session_id: p.session_id, person_id: p.person_id, level };
  if (isGrantLevel(p.request)) out.request = p.request;
  return out;
}

/**
 * Patch the grant set from `grant:changed` frames.
 *
 * Only frames naming THIS person touch the map: the hub fences the frame to
 * the persons a grant actually names, which includes the owner doing the
 * sharing, and the owner's own access comes from `owner_person_id` — folding
 * someone else's grant into our map would be a grant to us that nobody made.
 * With no person id we cannot tell whose frame it is, so none of them apply
 * and the next `my_grants` is what fixes the set.
 */
export function applyGrantChanges(changes: readonly GrantChanged[]): void {
  const me = get(myPersonId);
  if (me === null) return;
  const mine = changes.filter((c) => c.person_id === me);
  if (mine.length === 0) return;
  if (grantFramesFor !== me) grantTouchedAt.clear();
  grantFramesFor = me;
  for (const c of mine) grantTouchedAt.set(c.session_id, ++grantFrameSeq);
  myGrants.update((cur) => {
    const next = new Map(cur);
    for (const c of mine) {
      if (c.level === null) next.delete(c.session_id);
      else next.set(c.session_id, c.level);
    }
    return next;
  });
  // Gap plan G4.2: an ask this person just made shows at once; any other
  // frame may have answered one (or changed who shared what), so the
  // labels are re-read from `my_grants` rather than guessed.
  const opened = mine.filter((c) => c.request !== undefined);
  if (opened.length > 0) {
    myAccessRequests.update((cur) => {
      const next = new Map(cur);
      for (const c of opened) next.set(c.session_id, { id: next.get(c.session_id)?.id ?? -1, level: c.request!, requestedAt: Math.floor(Date.now() / 1000) });
      return next;
    });
  }
  const revoked = mine.filter((c) => c.level === null);
  if (revoked.length > 0) {
    myGrantInfo.update((cur) => {
      const next = new Map(cur);
      for (const c of revoked) next.delete(c.session_id);
      return next;
    });
  }
  if (mine.some((c) => c.request === undefined)) scheduleGrantRefresh();
}

let refreshTimer: ReturnType<typeof setTimeout> | null = null;
/** Re-read `my_grants` once a burst of frames has settled. */
function scheduleGrantRefresh(): void {
  if (refreshTimer !== null) return;
  refreshTimer = setTimeout(() => {
    refreshTimer = null;
    void loadMyGrants();
  }, 300);
}

/** Test seam: forget who we are, the way a fresh launch has not asked yet. */
export function resetAccessForTests(): void {
  myPersonId.set(null);
  myGrants.set(new Map());
  myGrantInfo.set(new Map());
  myAccessRequests.set(new Map());
}
