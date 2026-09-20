import { describe, it, expect } from 'vitest';
import { addFiles, pastedName, fmtBytes, markNeedsReattach, clearSent, NEEDS_REATTACH, MAX_FILES, MAX_BYTES } from './attachments';

const file = (o: Partial<{ path: string; name: string; size: number; kind: 'image' | 'text' | 'binary' }> = {}) => ({
  path: '/tmp/a.png', name: 'a.png', size: 1024, kind: 'image' as const, ...o,
});

describe('addFiles', () => {
  it('adds a picked file as a reading tile, so the strip reserves space at once', () => {
    const { next, rejected } = addFiles([], [file()]);
    expect(rejected).toEqual([]);
    expect(next).toHaveLength(1);
    expect(next[0]).toMatchObject({ name: 'a.png', kind: 'image', state: 'reading', thumb: null });
    expect(next[0].id).toBeTruthy();
  });

  it('rejects a file over the per-file limit, naming the size and the limit', () => {
    const { next, rejected } = addFiles([], [file({ name: 'big.png', size: MAX_BYTES + 1 })]);
    expect(next).toHaveLength(0);
    expect(rejected[0]).toContain('big.png');
    expect(rejected[0]).toContain('10 MB');
  });

  it('rejects past the count cap instead of silently truncating', () => {
    const full = Array.from({ length: MAX_FILES }, (_, i) => file({ path: `/tmp/${i}.png`, name: `${i}.png` }));
    const { next } = addFiles([], full);
    expect(next).toHaveLength(MAX_FILES);
    const { next: after, rejected } = addFiles(next, [file({ path: '/tmp/x.png', name: 'x.png' })]);
    expect(after).toHaveLength(MAX_FILES);
    expect(rejected[0]).toContain('10 files');
  });

  it('rejects when the total would exceed the budget', () => {
    const { next } = addFiles([], [file({ size: 9 * 1024 * 1024 })]);
    const { rejected } = addFiles(next, [
      file({ path: '/tmp/b.png', name: 'b.png', size: 9 * 1024 * 1024 }),
      file({ path: '/tmp/c.png', name: 'c.png', size: 9 * 1024 * 1024 }),
    ]);
    expect(rejected.some((r) => r.includes('in total'))).toBe(true);
  });

  it('does not add the same path twice', () => {
    const { next } = addFiles([], [file()]);
    const { next: after } = addFiles(next, [file()]);
    expect(after).toHaveLength(1);
  });

  // FINDING 1 (fix round 2): a tile marked `NEEDS_REATTACH` tells the user
  // to attach the file again, but the plain dedupe above would silently
  // drop that second attach as a duplicate — telling the user to act and
  // then ignoring the action is worse than the failure that put the tile
  // there. Attaching the same path while it is in that state must replace
  // it with a fresh, authorised entry; an ordinary live duplicate must
  // still collapse exactly as before.
  describe('re-attaching a path already in the tray', () => {
    const spent = {
      id: 'att-x', path: '/tmp/a.png', name: 'a.png', size: 1024, kind: 'image' as const,
      thumb: null, state: 'error' as const, error: NEEDS_REATTACH, pasted: false,
    };
    const live = { ...spent, id: 'att-live', state: 'ready' as const, error: null };

    it('replaces a needs-reattach tile with a fresh, authorised entry', () => {
      const { next, rejected } = addFiles([spent], [file()]);
      expect(rejected).toEqual([]);
      expect(next).toHaveLength(1);
      expect(next[0].id).not.toBe('att-x');
      expect(next[0]).toMatchObject({ path: '/tmp/a.png', state: 'reading', error: null });
    });

    it('still collapses two attaches of the same live file — an ordinary no-op', () => {
      const { next } = addFiles([live], [file()]);
      expect(next).toHaveLength(1);
      expect(next[0]).toBe(live);
    });

    it('checks a replacement against the total budget without double-counting the stale entry', () => {
      const other = {
        id: 'att-other', path: '/tmp/other.png', name: 'other.png', size: 20 * 1024 * 1024,
        kind: 'image' as const, thumb: null, state: 'ready' as const, error: null, pasted: false,
      };
      const smallSpent = { ...spent, size: 1024 };
      // Replacing the spent 1 KB entry with a fresh 6 MB one would bring the
      // total to just over 25 MB alongside the other 20 MB file — refused,
      // and the stale tile must still be there since the replacement did
      // not happen.
      const { next, rejected } = addFiles(
        [other, smallSpent],
        [file({ size: 6 * 1024 * 1024 })],
      );
      expect(rejected[0]).toContain('in total');
      expect(next).toHaveLength(2);
      expect(next.find((a) => a.id === 'att-x')).toBeTruthy();
    });
  });

  // Pasted files (a clipboard screenshot) arrive with no filesystem path —
  // nothing authorised one, so the Rust allow-list can never accept them.
  // addFiles represents that honestly instead of letting it fail later as a
  // permission error.
  it('marks a pasted file (no path) as an error tile instead of queuing it for upload', () => {
    const { next, rejected } = addFiles([], [file({ path: '', name: 'pasted-14.05.09.png' })]);
    expect(rejected).toEqual([]);
    expect(next).toHaveLength(1);
    expect(next[0]).toMatchObject({ name: 'pasted-14.05.09.png', state: 'error', pasted: true, thumb: null });
    expect(next[0].error).toBeTruthy();
    expect(next[0].error).toMatch(/paste/i);
    expect(next[0].error).toMatch(/drop|attach/i);
  });

  it('does not dedupe pasted files against each other, since they share no real path', () => {
    const { next } = addFiles([], [file({ path: '', name: 'pasted-a.png' }), file({ path: '', name: 'pasted-b.png' })]);
    expect(next).toHaveLength(2);
    expect(next.every((a) => a.state === 'error' && a.pasted)).toBe(true);
  });

  it('leaves ordinary picked files unmarked as pasted', () => {
    const { next } = addFiles([], [file()]);
    expect(next[0].pasted).toBe(false);
  });
});

describe('pastedName', () => {
  it('makes ten pastes distinguishable', () => {
    expect(pastedName(new Date('2026-09-20T14:05:09'))).toBe('pasted-14.05.09.png');
  });
});

describe('fmtBytes', () => {
  it('reads like a person wrote it', () => {
    expect(fmtBytes(512)).toBe('512 B');
    expect(fmtBytes(1024 * 1024)).toBe('1.0 MB');
    expect(fmtBytes(14.2 * 1024 * 1024)).toBe('14.2 MB');
  });
});

// `droppedFile` (a client-side `size: 0` placeholder guessed from the
// extension) is gone — a dropped path is now measured by the Rust
// `attachment_describe` command, the same way a picked one is measured by
// `pick_attachments`. Once that real `PickedFile` reaches `addFiles`, it is
// indistinguishable from a picked one, so there is nothing drop-specific
// left to unit-test here; the integration is covered in
// ConversationPanel.test.ts, where an oversized *dropped* file is shown to
// be rejected by the same `addFiles` limit a picked one hits.

// `upload_attachments` consumes each path's allow-list entry the moment it
// passes the byte budget, before a single byte transfers (`UploadAllowList
// ::consume` in `src-tauri/src/commands/upload.rs`). A failure anywhere
// downstream of that — the upload call itself, or a later refusal on this
// side of the wire — leaves the tiles' local paths unusable for a second
// attempt even though nothing in the UI says so. `markNeedsReattach` is how
// the composer stops claiming a plain retry will work.
describe('markNeedsReattach', () => {
  const two = () => [
    { id: 'a', path: '/tmp/a.png', name: 'a.png', size: 1, kind: 'image' as const, thumb: null, state: 'ready' as const, error: null, pasted: false },
    { id: 'b', path: '/tmp/b.png', name: 'b.png', size: 1, kind: 'image' as const, thumb: null, state: 'ready' as const, error: null, pasted: false },
  ];

  it('marks only the given ids as needing reattachment', () => {
    const out = markNeedsReattach(two(), new Set(['a']));
    expect(out[0]).toMatchObject({ id: 'a', state: 'error', error: NEEDS_REATTACH });
    expect(out[1]).toMatchObject({ id: 'b', state: 'ready', error: null });
  });

  it('is a no-op — same array reference — for an empty id set', () => {
    const list = two();
    expect(markNeedsReattach(list, new Set())).toBe(list);
  });
});

// The other half of the same fix: only the attachments this send actually
// uploaded are spent. Anything it could not upload (a pasted entry) was
// never part of the attempt, so it is not this function's to remove — the
// evidence that it did not go must not disappear with the ones that did.
describe('clearSent', () => {
  const mixed = () => [
    { id: 'a', path: '/tmp/a.png', name: 'a.png', size: 1, kind: 'image' as const, thumb: null, state: 'ready' as const, error: null, pasted: false },
    { id: 'p', path: '', name: 'pasted-14.05.09.png', size: 0, kind: 'image' as const, thumb: null, state: 'error' as const, error: "Pasted files aren't supported yet.", pasted: true },
  ];

  it('removes only the ids that were uploaded, keeping everything else', () => {
    const out = clearSent(mixed(), new Set(['a']));
    expect(out).toHaveLength(1);
    expect(out[0].id).toBe('p');
  });

  it('is a no-op — same array reference — for an empty id set', () => {
    const list = mixed();
    expect(clearSent(list, new Set())).toBe(list);
  });
});
