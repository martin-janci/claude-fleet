import { describe, it, expect } from 'vitest';
import { inboxRows, nextInInbox, notWaiting, notWaitingText } from './inbox';
import { attentionState, countNeedsYou } from './attention';
import { session } from './hosts_fixture';

// Redesign step 3.3: the Inbox lists exactly what raises the badge.

const opts = { idleSecs: 600, now: 2_000_000_000 };

const rows = [
  session('mac', 'asking', { claude_status: 'blocked' }),
  session('mac', 'crashed', { claude_status: 'failed' }),
  session('mac', 'busy', { claude_status: 'working' }),
  session('mac', 'quiet', { claude_status: 'idle', last_activity_at: opts.now - 60 }),
  session('mac', 'gone', { status: 'ghost', claude_status: 'idle' }),
  session('mac', 'shell', { kind: 'shell', claude_status: 'blocked' }),
];

describe('inbox', () => {
  it('lists the rows the badge counts, worst first, and nothing else', () => {
    const inbox = inboxRows(rows, opts);
    expect(inbox.map((s) => s.tmux_name)).toEqual(['asking', 'crashed']);
    expect(inbox.length).toBe(countNeedsYou(rows, opts));
  });

  it('a ghost leaves the Inbox and is counted below it as paused', () => {
    expect(attentionState(rows[4], opts)).toBe('paused');
    const n = notWaiting(rows, opts);
    expect(n).toEqual({ working: 1, idle: 2, done: 0, paused: 1 });
    expect(notWaitingText(n)).toBe('1 running · 2 idle · 1 paused');
    expect(notWaitingText({ working: 0, idle: 0, done: 0, paused: 0 })).toBe('');
  });
});

describe('nextInInbox (redesign 5.9)', () => {
  const row = (id: number) => ({ id }) as unknown as import('./sessions').SessionRow;
  it('takes the next row in order, wraps round, and never the one just answered', () => {
    const q = [row(1), row(2), row(3)];
    expect(nextInInbox(q, 2)?.id).toBe(3);
    expect(nextInInbox(q, 3)?.id).toBe(1);
    expect(nextInInbox(q, 9)?.id).toBe(1);
    expect(nextInInbox([row(4)], 4)).toBeNull();
    expect(nextInInbox([], 4)).toBeNull();
  });
});
