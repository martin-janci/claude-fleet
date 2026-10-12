/**
 * The composer's outbox: messages the user has sent from the conversation
 * screen, from the moment Enter is pressed until the transcript carries
 * them.
 *
 * A message is on screen at once, as `sending`, and moves through the
 * states below on what the backend actually reports — never on a timer:
 *
 *   waiting ─► sending ─► sent ─────► received ─► (settled: the transcript has it)
 *                 │        queued ──┘
 *                 └──► failed ─► (Retry → waiting | Edit / Discard → gone)
 *
 * - `waiting`  in line behind an earlier message of the same session.
 * - `sending`  upload + `send_prompt` in flight.
 * - `sent`     tmux took it while Claude was idle; no receipt yet.
 * - `queued`   tmux took it while Claude was working: Claude reads it after
 *              the current turn.
 * - `received` Claude took it: the row's `prompt_submit_seq` moved past the
 *              count the send started from (the UserPromptSubmit hook).
 * - `failed`   it did not go. Everything behind it waits (`isHeld`) until
 *              the user decides, so messages are never reordered.
 *
 * One sender per session, in order: two pastes into one REPL at once arrive
 * as one mangled line. The outbox lives at module level, not in the panel,
 * so a send keeps going (and its state is still there) across a session
 * switch — the panel is one instance for every session.
 */
import { writable, get } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { sessionIdActionBlocked } from './share';
import { sendPrompt, sessions } from './sessions';
import { markNeedsReattach, type Attachment } from './attachments';
import { withAttachments, tooLong } from './attach_prompt';
import { recordAttachments } from './library';
import { carriedCount, normalizePrompt, type Conversation } from './conversation';

export type OutboxState = 'waiting' | 'sending' | 'sent' | 'queued' | 'received' | 'failed';

export interface OutboxTarget {
  id: number;
  host_alias: string;
  tmux_name: string;
}

/** The part of a session row the receipts read. */
export interface OutboxRow {
  claude_status: string | null;
  turn_seq: number;
  prompt_submit_seq?: number;
}

export interface OutboxMessage {
  id: string;
  kind: 'prompt' | 'command';
  /** What the user typed: the bubble shows this, not the prefixed body. */
  text: string;
  prefix: string | null;
  attachments: Attachment[];
  /** Remote paths once the upload went through; kept for a retry. */
  paths: string[] | null;
  state: OutboxState;
  error: string | null;
  /** False once a retry cannot work as-is: the upload spent the picked
   *  paths, or the prompt is too long. Edit is the way out. */
  retryable: boolean;
  /** ISO time the user pressed Enter. */
  at: string;
  /** ms timestamp the current send attempt started. */
  sendingSince: number | null;
  /** Turns already carrying this text when it was sent: the transcript has
   *  caught up once `carriedCount` is greater than this, so re-sending an
   *  earlier prompt ("continue") is not mistaken for it. */
  seen: number;
  /** The row's `turn_seq` when it went out. */
  turnSeqAtSend: number | null;
  /** Claude took it (a submit receipt landed), possibly before the send returned. */
  acked: boolean;
}

export interface OutboxDeps {
  send(host: string, tmux: string, body: string): Promise<Result<void>>;
  upload(host: string, tmux: string, localPaths: string[]): Promise<Result<string[]>>;
  row(sessionId: number): OutboxRow | undefined;
  /**
   * Why this client may not write to `sessionId` RIGHT NOW, or null.
   *
   * Multi-user M1 (F2c). The composer in front of the outbox gates the
   * ENQUEUE, which is a different moment from the SEND — and the difference is
   * the whole point of a queue. A message waiting behind a slow turn, behind an
   * upload, or behind a failure the person has not decided yet goes out minutes
   * later, from a `.finally` that no component is in; by then the grant it was
   * typed under can have been narrowed to `watch` or revoked outright.
   *
   * So the queue asks for itself, at every dispatch, and refuses by FAILING the
   * message with the reason rather than dropping it: everything behind a failed
   * message is held (`isHeld`), so a revoke stops the line instead of letting
   * the messages behind the refused one overtake it, and the person sees why.
   * `retryable` stays true, so Retry works the moment the grant comes back.
   *
   * Optional, and `null` when absent: `createOutbox` is also the unit under
   * test, and a test that is not about access should not have to wire one.
   */
  blocked?(sessionId: number): string | null;
}

export interface OutboxValue {
  msgs: Record<number, OutboxMessage[]>;
  /** The last message a session's send delivered: the panel refetches on a
   *  new `stamp`, and treats a prompt as running until the turn moves. */
  lastSent: Record<number, { kind: 'prompt' | 'command'; turnSeq: number | null; stamp: number }>;
}

export interface EnqueueInput {
  kind: 'prompt' | 'command';
  text: string;
  prefix?: string | null;
  attachments?: Attachment[];
  /** `carriedCount` of the text at send time. The outbox adds identical
   *  prompts still in line, and zeroes it with attachments (their uploaded
   *  paths make the body new). */
  seen?: number;
}

/** The text a message sends: the prefix in front of a prompt (a command
 *  goes as typed), then the attachments block for any uploaded paths. */
export function outboxBody(m: {
  kind: 'prompt' | 'command';
  text: string;
  prefix: string | null;
  paths?: string[] | null;
}): string {
  const prefixed = m.prefix && m.kind === 'prompt' ? `${m.prefix}\n\n${m.text}` : m.text;
  return withAttachments(prefixed, m.paths ?? []);
}

const TOO_LONG = 'That prompt is too long to send through tmux. Shorten it.';

/** Still in line behind a failed message of the same session. */
export function isHeld(msgs: OutboxMessage[], id: string): boolean {
  const i = msgs.findIndex((m) => m.id === id);
  return i > 0 && msgs[i].state === 'waiting' && msgs.slice(0, i).some((m) => m.state === 'failed');
}

/** A send this long in flight says so, rather than looking frozen. */
export const SLOW_SEND_MS = 8_000;

export type ReceiptTone = 'muted' | 'warn' | 'ok' | 'crit';

/** The status line under a message, in words. */
export function receipt(m: OutboxMessage, held: boolean, nowMs: number): { tone: ReceiptTone; label: string } {
  switch (m.state) {
    case 'waiting':
      return { tone: 'muted', label: held ? 'Waiting for the message above' : 'Up next' };
    case 'sending':
      return {
        tone: 'muted',
        label: m.sendingSince !== null && nowMs - m.sendingSince >= SLOW_SEND_MS ? 'Still sending…' : 'Sending…',
      };
    case 'sent':
      return { tone: 'muted', label: 'Sent' };
    case 'queued':
      return { tone: 'warn', label: 'Claude reads it after this turn' };
    case 'received':
      return { tone: 'ok', label: 'Claude is on it' };
    case 'failed':
      return { tone: 'crit', label: `Not sent: ${m.error ?? 'unknown error'}` };
  }
}

export function createOutbox(deps: OutboxDeps) {
  const store = writable<OutboxValue>({ msgs: {}, lastSent: {} });
  const targets = new Map<number, OutboxTarget>();
  const running = new Set<number>();
  // Last `prompt_submit_seq` seen per session: a receipt is the count
  // moving past it, however many row events the move is spread over.
  const lastSubmit = new Map<number, number>();
  let nextId = 0;
  let stamp = 0;
  // Bumped by `resetForTests`: a send still in flight from before the reset
  // must not write into the fresh state when it resolves.
  let epoch = 0;

  const list = (sid: number) => get(store).msgs[sid] ?? [];
  const find = (sid: number, id: string) => list(sid).find((m) => m.id === id);

  function setList(sid: number, next: OutboxMessage[]) {
    store.update((v) => {
      const msgs = { ...v.msgs };
      if (next.length) msgs[sid] = next;
      else delete msgs[sid];
      return { ...v, msgs };
    });
  }
  function patch(sid: number, id: string, p: Partial<OutboxMessage>) {
    setList(
      sid,
      list(sid).map((m) => (m.id === id ? { ...m, ...p } : m)),
    );
  }
  function remove(sid: number, id: string) {
    setList(
      sid,
      list(sid).filter((m) => m.id !== id),
    );
  }

  /** Re-read the submit count to measure receipts from — only while none of
   *  ours is out: row events are observed only for a session with messages,
   *  so a count remembered from an earlier batch may have been moved since
   *  by prompts typed in the terminal. While one is out, the count carries
   *  that message's receipt and must stay where it was. */
  function baseline(sid: number) {
    if (list(sid).some((m) => m.state !== 'waiting' && m.state !== 'failed' && !m.acked)) return;
    const seq = deps.row(sid)?.prompt_submit_seq;
    if (seq === undefined) lastSubmit.delete(sid);
    else lastSubmit.set(sid, seq);
  }

  /** Identical plain prompts of `sid` still in line (not failed): the
   *  transcript will carry each of them before the new one. */
  function queuedDuplicates(sid: number, body: string): number {
    const want = normalizePrompt(body);
    return list(sid).filter(
      (m) =>
        m.kind === 'prompt' &&
        m.state !== 'failed' &&
        m.attachments.length === 0 &&
        normalizePrompt(outboxBody(m)) === want,
    ).length;
  }

  function enqueue(target: OutboxTarget, input: EnqueueInput): string {
    targets.set(target.id, target);
    baseline(target.id);
    const prefix = input.prefix ?? null;
    const attachments = input.attachments ?? [];
    const seen =
      attachments.length > 0
        ? 0
        : (input.seen ?? 0) +
          (input.kind === 'prompt'
            ? queuedDuplicates(target.id, outboxBody({ kind: input.kind, text: input.text, prefix }))
            : 0);
    const msg: OutboxMessage = {
      id: `ob${++nextId}`,
      kind: input.kind,
      text: input.text,
      prefix,
      attachments,
      paths: null,
      state: 'waiting',
      error: null,
      retryable: true,
      at: new Date().toISOString(),
      sendingSince: null,
      seen,
      turnSeqAtSend: null,
      acked: false,
    };
    setList(target.id, [...list(target.id), msg]);
    pump(target.id);
    return msg.id;
  }

  /** Send whatever is next for `sid`, until the line is empty or blocked by
   *  a failure. The first message is marked `sending` synchronously, so the
   *  bubble never shows as `waiting` for a line that is actually free. */
  function pump(sid: number) {
    if (running.has(sid)) return;
    const msgs = list(sid);
    if (msgs.some((m) => m.state === 'failed')) return;
    const next = msgs.find((m) => m.state === 'waiting');
    if (!next) return;
    // Asked HERE, at the dispatch, because `pump` is re-entered from three
    // places — `enqueue`, the `.finally` of the previous send, and
    // `retry` / `discard` / `edit` — and only one of them is a click a surface
    // could have gated. See `OutboxDeps.blocked`.
    const refused = deps.blocked?.(sid) ?? null;
    if (refused !== null) {
      patch(sid, next.id, { state: 'failed', error: refused, retryable: true, sendingSince: null });
      return;
    }
    running.add(sid);
    patch(sid, next.id, { state: 'sending', sendingSince: Date.now(), error: null });
    const mine = epoch;
    void sendOne(sid, next.id, mine).finally(() => {
      if (mine !== epoch) return;
      running.delete(sid);
      pump(sid);
    });
  }

  async function sendOne(sid: number, id: string, mine: number) {
    const target = targets.get(sid);
    let m = find(sid, id);
    if (!target || !m) return;

    let paths = m.paths;
    if (paths === null && m.attachments.length > 0) {
      const up = await deps.upload(
        target.host_alias,
        target.tmux_name,
        m.attachments.map((a) => a.path),
      );
      if (mine !== epoch) return;
      if (!up.ok) {
        // `upload_attachments` consumes each path's allow-list entry as it
        // clears the budget, so the same paths cannot be uploaded twice.
        patch(sid, id, { state: 'failed', error: up.error.message, retryable: false });
        return;
      }
      paths = up.value;
      patch(sid, id, { paths });
    }
    m = find(sid, id)!;

    const body = outboxBody({ ...m, paths });
    if (tooLong(body, target.host_alias === 'local')) {
      patch(sid, id, { state: 'failed', error: TOO_LONG, retryable: false });
      return;
    }

    // Re-asked after the upload: `upload_attachments` is `same_in_both`, so it
    // is not routed and the hub never sees it — but it can take seconds, and a
    // grant narrowed during it must stop the prompt that follows.
    const stillRefused = deps.blocked?.(sid) ?? null;
    if (stillRefused !== null) {
      patch(sid, id, { state: 'failed', error: stillRefused, retryable: true, sendingSince: null });
      return;
    }

    const before = deps.row(sid);
    patch(sid, id, { turnSeqAtSend: before?.turn_seq ?? null });
    const r = await deps.send(target.host_alias, target.tmux_name, body);
    if (mine !== epoch) return;
    if (!r.ok) {
      patch(sid, id, { state: 'failed', error: r.error.message, retryable: true, acked: false });
      return;
    }
    stamp += 1;
    store.update((v) => ({
      ...v,
      lastSent: { ...v.lastSent, [sid]: { kind: m!.kind, turnSeq: before?.turn_seq ?? null, stamp } },
    }));
    if (m.kind === 'command') {
      remove(sid, id);
      return;
    }
    const now = find(sid, id)!;
    const after = deps.row(sid);
    const busyBefore = before?.claude_status === 'working';
    // Without the counter (a hub older than it), the status is the only
    // signal: a session that went from idle to working took our prompt.
    const noCounter = after?.prompt_submit_seq === undefined;
    const state: OutboxState = now.acked
      ? 'received'
      : noCounter && !busyBefore && after?.claude_status === 'working'
        ? 'received'
        : busyBefore
          ? 'queued'
          : 'sent';
    patch(sid, id, { state, sendingSince: null });
  }

  /** A row event for `sid`: hand each submit the counter moved by to the
   *  oldest message still waiting for its receipt. */
  function observeRow(sid: number, row: OutboxRow) {
    const msgs = list(sid);
    const awaiting = (m: OutboxMessage) =>
      m.kind === 'prompt' && !m.acked && (m.state === 'sending' || m.state === 'sent' || m.state === 'queued');
    if (row.prompt_submit_seq === undefined) {
      if (row.claude_status !== 'working') return;
      const first = msgs.find((m) => m.state === 'sent');
      if (first) patch(sid, first.id, { state: 'received', acked: true });
      return;
    }
    const last = lastSubmit.get(sid);
    lastSubmit.set(sid, row.prompt_submit_seq);
    if (last === undefined) return;
    let bumps = row.prompt_submit_seq - last;
    if (bumps <= 0) return;
    for (const m of msgs) {
      if (bumps === 0) break;
      if (!awaiting(m)) continue;
      bumps -= 1;
      patch(sid, m.id, m.state === 'sending' ? { acked: true } : { state: 'received', acked: true });
    }
  }

  /** A transcript read for `sid` landed. A delivered prompt it carries is
   *  now shown by the thread itself; one it never carries (the REPL can
   *  rewrite a prompt) is dropped once its turn is over and the session is
   *  quiet — a queued one gets one turn more, since it runs in the next. */
  function settle(sid: number, conv: Conversation, at: { quiet: boolean; turnSeq: number }) {
    const msgs = list(sid);
    const keep = msgs.filter((m) => {
      if (m.kind !== 'prompt' || !['sent', 'queued', 'received'].includes(m.state)) return true;
      if (carriedCount(conv, outboxBody(m)) > m.seen) return false;
      if (!at.quiet || m.turnSeqAtSend === null) return true;
      const turns = m.state === 'queued' ? 2 : 1;
      return at.turnSeq < m.turnSeqAtSend + turns;
    });
    if (keep.length !== msgs.length) setList(sid, keep);
  }

  function retry(sid: number, id: string) {
    const m = find(sid, id);
    if (!m || m.state !== 'failed' || !m.retryable) return;
    patch(sid, id, { state: 'waiting', error: null });
    pump(sid);
  }

  function discard(sid: number, id: string) {
    const m = find(sid, id);
    if (!m || m.state !== 'failed') return;
    remove(sid, id);
    pump(sid);
  }

  /** Take a failed message back into the composer: its text, and its tiles
   *  (flagged for re-attaching when the upload already spent them). */
  function edit(sid: number, id: string): { text: string; attachments: Attachment[] } | null {
    const m = find(sid, id);
    if (!m || m.state !== 'failed') return null;
    remove(sid, id);
    pump(sid);
    const spent = m.paths !== null || !m.retryable;
    const ids = new Set(spent ? m.attachments.map((a) => a.id) : []);
    return { text: m.text, attachments: markNeedsReattach(m.attachments, ids) };
  }

  /** Forget everything, including sends still in flight. Tests only: the
   *  outbox is module state, and one test's messages must not bleed into
   *  the next. */
  function resetForTests() {
    epoch += 1;
    targets.clear();
    running.clear();
    lastSubmit.clear();
    store.set({ msgs: {}, lastSent: {} });
  }

  return { store, enqueue, retry, discard, edit, observeRow, settle, resetForTests };
}

export type Outbox = ReturnType<typeof createOutbox>;

// ─── The app's outbox ───────────────────────────────────────────────────────

/** The one outbox every composer shares: sends go through `send_prompt`,
 *  attachments through `upload_attachments`, and receipts come off the
 *  sessions store's row events. */
export const outbox = createOutbox({
  send: (host, tmux, body) => sendPrompt(host, tmux, body),
  // The access half, resolved from the id the queue carries. The pure form
  // rather than the store: a send runs from a `.finally`, not from a reactive
  // position, so the `get()` defaults are what is wanted — and a row the
  // client can no longer see fails closed (`UNKNOWN_SESSION_REASON`).
  blocked: (id) => sessionIdActionBlocked(id, 'send_prompt'),
  upload: async (host, tmux, localPaths) => {
    const r = await invokeCmd<string[]>('upload_attachments', {
      args: { host_alias: host, session_name: tmux, local_paths: localPaths },
    });
    // Control's Library (9.7) lists what was put on the host.
    if (r.ok) recordAttachments(host, tmux, r.value);
    return r;
  },
  row: (id) => get(sessions).find((s) => s.id === id),
});

// Only a session with messages out needs its row events read.
sessions.subscribe((rows) => {
  const out = get(outbox.store).msgs;
  for (const key of Object.keys(out)) {
    const row = rows.find((s) => s.id === Number(key));
    if (row) outbox.observeRow(row.id, row);
  }
});
