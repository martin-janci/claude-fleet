// The three write paths F2c closed, and the predicate they share.
//
// Each test here comes in two halves on purpose: the gate, and the OWNER's
// positive control beside it. A gate with no positive control is how a
// multi-user change quietly becomes a single-user regression — the refusal is
// easy to assert and the thing that still has to work is the whole product.
//
// Where a handler RE-ASKS, the re-ask is what is tested: open the thing, revoke
// the grant, then act. That is the case a control-only gate passes and a person
// hits, and it is the case `share.ts`'s rule ("a handler must re-ask, not trust
// the gate that opened it") exists for.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';
import { resetAccessForTests, setMyGrants } from './access';
import { hubStatus, STANDALONE, unavailableReason, type HubStatus } from './hub';
import {
  UNKNOWN_SESSION_REASON,
  sessionIdActionBlocked,
  sessionIdBlocked,
  shareSheetFor,
} from './share';
import { sessions } from './sessions';
import { session } from './hosts_fixture';
import { createOutbox, type OutboxDeps, type OutboxRow } from './outbox';
import type { Result } from './result';
import { linkSessionId } from './work';

const remote: HubStatus = {
  ...STANDALONE,
  remote: true,
  url: 'https://fleet.example.com',
  configured_url: 'https://fleet.example.com',
};
const unavailable: HubStatus = {
  ...STANDALONE,
  unavailable: 'no stored token for https://fleet.example.com',
  configured_url: 'https://fleet.example.com',
};

/** Me = person 7. Session 1 is mine, session 2 is somebody else's. */
const MINE = { id: 1, owner_person_id: 7 };
const THEIRS = { id: 2, owner_person_id: 9 };

beforeEach(() => {
  hubStatus.set({ ...STANDALONE });
  resetAccessForTests();
  shareSheetFor.set(null);
  sessions.set([]);
});

// ── the predicate a write with no row in hand asks ────────────────────────

describe('sessionIdActionBlocked: a write that names its session by id', () => {
  const rows = [MINE, THEIRS];

  it('a standalone desktop is untouched, resolvable row or not', () => {
    // Rule 1 of `access.ts`: this process IS the fleet. The `null` id is the
    // case that matters — the fail-closed branch below must not reach it.
    expect(sessionIdActionBlocked(1, 'send_prompt', rows)).toBeNull();
    expect(sessionIdActionBlocked(999, 'kill_session', rows)).toBeNull();
    expect(sessionIdActionBlocked(null, 'resume_work', rows)).toBeNull();
  });

  it('the owner keeps every action', () => {
    setMyGrants(7, []);
    for (const a of ['send_prompt', 'resume_work', 'kill_session'] as const) {
      expect(sessionIdActionBlocked(1, a, rows, remote), a).toBeNull();
    }
  });

  it('a watcher may not write, and a driver may not take over', () => {
    setMyGrants(7, [{ session_id: 2, level: 'watch' }]);
    expect(sessionIdActionBlocked(2, 'send_prompt', rows, remote)).toContain('Watch is read-only');
    setMyGrants(7, [{ session_id: 2, level: 'drive' }]);
    expect(sessionIdActionBlocked(2, 'send_prompt', rows, remote)).toBeNull();
    // `resume_work` is the `own` tier: driving is not taking over.
    expect(sessionIdActionBlocked(2, 'resume_work', rows, remote)).toContain('owner');
  });

  it('a row it cannot resolve FAILS CLOSED on a fleet it does not own', () => {
    // The whole point of the helper. The hub fences rows this client may not
    // see off the stream, so "not in $sessions" reads as *someone else's, or
    // gone* — never as "no session, nothing to refuse".
    setMyGrants(7, []);
    expect(sessionIdActionBlocked(999, 'resume_work', rows, remote)).toBe(UNKNOWN_SESSION_REASON);
    expect(sessionIdActionBlocked(null, 'send_prompt', rows, remote)).toBe(UNKNOWN_SESSION_REASON);
    expect(sessionIdActionBlocked(undefined, 'send_prompt', rows, remote)).toBe(UNKNOWN_SESSION_REASON);
  });

  it('an unreachable hub blames the hub, not the session', () => {
    expect(sessionIdActionBlocked(999, 'resume_work', rows, unavailable)).toBe(
      unavailableReason(unavailable),
    );
  });

  it('the store form re-answers when a grant is narrowed, with no row event', () => {
    // A revoke moves no column on any session: a surface that resolved the id
    // once would keep its button live. This is what makes the derived form the
    // one components read.
    sessions.set([session('mefistos', 'a', { id: 2, owner_person_id: 9 })]);
    hubStatus.set(remote);
    setMyGrants(7, [{ session_id: 2, level: 'drive' }]);
    expect(get(sessionIdBlocked)(2, 'send_prompt')).toBeNull();
    setMyGrants(7, [{ session_id: 2, level: 'watch' }]);
    expect(get(sessionIdBlocked)(2, 'send_prompt')).toContain('Watch is read-only');
    setMyGrants(7, []);
    expect(get(sessionIdBlocked)(2, 'send_prompt')).toContain('not shared with you');
  });
});

describe('linkSessionId: a pane name is not a session identity', () => {
  const link = { snap_host: 'h-b', snap_tmux: 'web', snap_claude_ids: '["conv-b"]' };

  it('resolves an ended link back to its row when the snapshot corroborates it', () => {
    // The positive control: the row holds the snapshotted pane name AND one of
    // the conversations the link names, so it is that session.
    const rows = [
      session('mefistos', 'api', { id: 4, claude_session_id: 'conv-a' }),
      session('h-b', 'web', { id: 5, claude_session_id: 'conv-b' }),
    ];
    expect(linkSessionId(link, rows)).toBe(5);
  });

  it('is null — not a guess — when the snapshot names a row this client has not got', () => {
    const rows = [session('mefistos', 'api', { id: 4, claude_session_id: 'conv-a' })];
    expect(linkSessionId(link, rows)).toBeNull();
    expect(linkSessionId({ ...link, snap_host: null }, rows)).toBeNull();
    expect(linkSessionId(null, rows)).toBeNull();
  });

  it('refuses a NAMESAKE: the pane name was reused by a different session', () => {
    // F2d, the bug this closes. The repo's own reconcile logic treats a lost
    // row's tmux name as reusable, so `(host, tmux)` can name a session that
    // merely inherited the pane — typically the one this person just started.
    // Resolving to it answers the access question about the wrong row, in the
    // one direction that matters: it would read as `own`.
    const reused = [session('h-b', 'web', { id: 6, claude_session_id: 'conv-brand-new' })];
    expect(linkSessionId(link, reused)).toBeNull();
  });

  it('refuses an AMBIGUOUS name: a lost row and the live row that reused it', () => {
    const both = [
      session('h-b', 'web', { id: 5, claude_session_id: 'conv-b', lost_at: 10 }),
      session('h-b', 'web', { id: 7, claude_session_id: 'conv-brand-new' }),
    ];
    expect(linkSessionId(link, both)).toBeNull();
    // Not even when the FIRST match is the corroborated one — `find` would have
    // taken it, which is the silent half of the same bug.
    expect(linkSessionId(link, [...both].reverse())).toBeNull();
  });

  it('refuses when there is nothing to corroborate with', () => {
    const rows = [session('h-b', 'web', { id: 5, claude_session_id: 'conv-b' })];
    // An older hub sends no `snap_claude_ids`…
    expect(linkSessionId({ snap_host: 'h-b', snap_tmux: 'web' }, rows)).toBeNull();
    // …a malformed one names nothing rather than something…
    expect(linkSessionId({ ...link, snap_claude_ids: 'not json' }, rows)).toBeNull();
    expect(linkSessionId({ ...link, snap_claude_ids: '[]' }, rows)).toBeNull();
    // …and a row with no conversation of its own cannot be the one named.
    expect(linkSessionId(link, [session('h-b', 'web', { id: 5 })])).toBeNull();
  });

  it('the refusal reaches the write as a refusal on a paired desktop, and not on a standalone one', () => {
    // What `null` costs, both ways round: the whole value of failing closed in
    // this resolver is that `sessionIdActionBlocked` reads it as "unknown".
    const reused = [session('h-b', 'web', { id: 6, owner_person_id: 7 })];
    sessions.set(reused);
    const id = linkSessionId(link, reused);
    expect(id).toBeNull();
    hubStatus.set(remote);
    setMyGrants(7, []);
    expect(get(sessionIdBlocked)(id, 'resume_work')).toBe(UNKNOWN_SESSION_REASON);
    hubStatus.set({ ...STANDALONE });
    expect(get(sessionIdBlocked)(id, 'resume_work')).toBeNull();
  });
});

// ── 1. the outbox: the queue sends on its own ─────────────────────────────

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}
const flush = () => new Promise((r) => setTimeout(r, 0));
const idle = (): OutboxRow => ({ claude_status: 'idle', turn_seq: 0, prompt_submit_seq: 0 });
const TARGET = { id: 1, host_alias: 'mefistos', tmux_name: 'api' };

/** The outbox with a `blocked` dep the test moves between calls — a grant
 *  narrowed WHILE the queue is draining, which is the only moment that
 *  distinguishes a gate at the send from a gate at the enqueue. */
function queue() {
  let refuse: string | null = null;
  const sends: { body: string; done: (r: Result<void>) => void }[] = [];
  const uploads: { done: (r: Result<string[]>) => void }[] = [];
  const deps: OutboxDeps = {
    send: vi.fn((_h, _t, body) => {
      const d = deferred<Result<void>>();
      sends.push({ body, done: d.resolve });
      return d.promise;
    }),
    upload: vi.fn(() => {
      const d = deferred<Result<string[]>>();
      uploads.push({ done: d.resolve });
      return d.promise;
    }),
    row: () => idle(),
    blocked: () => refuse,
  };
  const box = createOutbox(deps);
  return {
    box,
    sends,
    uploads,
    deps,
    revoke: (why: string) => (refuse = why),
    grant: () => (refuse = null),
    msgs: () => get(box.store).msgs[TARGET.id] ?? [],
  };
}

const WATCH = 'Shared with you to watch. Watch is read-only: …';

describe('the outbox re-asks at the SEND, not at the enqueue', () => {
  it('the owner’s messages go out, one after the other', async () => {
    // The positive control: with nothing blocking, the queue is unchanged.
    const q = queue();
    q.box.enqueue(TARGET, { kind: 'prompt', text: 'one' });
    q.box.enqueue(TARGET, { kind: 'prompt', text: 'two' });
    await flush();
    expect(q.sends.map((s) => s.body)).toEqual(['one']);
    q.sends[0].done({ ok: true, value: undefined });
    await flush();
    expect(q.sends.map((s) => s.body)).toEqual(['one', 'two']);
  });

  it('a message queued under a grant is NOT sent after the grant is revoked', async () => {
    // The gate the briefing names: `pump` is re-entered from the `.finally` of
    // the previous send, minutes later and from no component at all.
    const q = queue();
    q.box.enqueue(TARGET, { kind: 'prompt', text: 'one' });
    q.box.enqueue(TARGET, { kind: 'prompt', text: 'two' });
    await flush();
    expect(q.sends).toHaveLength(1);

    q.revoke(WATCH);
    q.sends[0].done({ ok: true, value: undefined });
    await flush();

    expect(q.sends).toHaveLength(1); // 'two' never left
    const two = q.msgs().find((m) => m.text === 'two')!;
    expect(two.state).toBe('failed');
    expect(two.error).toBe(WATCH);
    // Retryable, so the moment the grant comes back the person can send it.
    expect(two.retryable).toBe(true);
  });

  it('Retry and Discard re-ask too — they re-enter the same pump', async () => {
    const q = queue();
    q.box.enqueue(TARGET, { kind: 'prompt', text: 'one' });
    await flush();
    q.sends[0].done({ ok: false, error: { code: 'E_SSH', message: 'ssh died' } });
    await flush();
    const id = q.msgs()[0].id;
    expect(q.msgs()[0].state).toBe('failed');

    q.revoke(WATCH);
    q.box.retry(TARGET.id, id);
    await flush();
    expect(q.sends).toHaveLength(1);
    expect(q.msgs()[0]).toMatchObject({ state: 'failed', error: WATCH });

    // …and the positive control: re-granted, the same Retry goes through.
    q.grant();
    q.box.retry(TARGET.id, q.msgs()[0].id);
    await flush();
    expect(q.sends).toHaveLength(2);
  });

  const TILE = {
    id: 'a1',
    path: '/tmp/a.txt',
    name: 'a.txt',
    size: 10,
    kind: 'text' as const,
    thumb: null,
    state: 'ready' as const,
    error: null,
    pasted: false,
  };

  it('refuses BEFORE the upload, so no file reaches the owner’s host', async () => {
    // What the gate in `pump` catches that the one before `deps.send` cannot:
    // `upload_attachments` is `same_in_both`, so it is never routed and the hub
    // never sees it. A message dispatched while the grant is gone must not put
    // a file on somebody else's machine on the way to being refused.
    const q = queue();
    q.revoke(WATCH);
    q.box.enqueue(TARGET, { kind: 'prompt', text: 'look', attachments: [TILE] });
    await flush();
    expect(q.uploads).toHaveLength(0);
    expect(q.sends).toHaveLength(0);
    expect(q.msgs()[0]).toMatchObject({ state: 'failed', error: WATCH });
  });

  it('a revoke during the upload stops the prompt that would follow it', async () => {
    // `upload_attachments` is `same_in_both`: the hub never sees it, so the
    // desktop gate is the only wall — and an upload takes seconds.
    const q = queue();
    q.box.enqueue(TARGET, {
      kind: 'prompt',
      text: 'look at this',
      attachments: [TILE],
    });
    await flush();
    expect(q.uploads).toHaveLength(1);

    q.revoke(WATCH);
    q.uploads[0].done({ ok: true, value: ['/remote/a.txt'] });
    await flush();

    expect(q.sends).toHaveLength(0);
    expect(q.msgs()[0]).toMatchObject({ state: 'failed', error: WATCH });
  });

  it('the app’s outbox is wired to the real predicate', async () => {
    // The unit above proves the queue asks; this proves what it asks. Without
    // it the dep could be wired to anything and every test still passes.
    const { outbox } = await import('./outbox');
    sessions.set([session('mefistos', 'api', { id: 1, owner_person_id: 9 })]);
    hubStatus.set(remote);
    setMyGrants(7, [{ session_id: 1, level: 'watch' }]);
    outbox.enqueue(TARGET, { kind: 'prompt', text: 'hello' });
    await flush();
    const msgs = get(outbox.store).msgs[1] ?? [];
    expect(msgs[0].state).toBe('failed');
    expect(msgs[0].error).toContain('Watch is read-only');
    outbox.resetForTests();
  });
});
