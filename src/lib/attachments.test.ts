import { describe, it, expect } from 'vitest';
import { addFiles, droppedFile, pastedName, fmtBytes, MAX_FILES, MAX_BYTES } from './attachments';

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

describe('droppedFile', () => {
  it('names and classifies a dropped path the way Rust’s classify does', () => {
    expect(droppedFile('/Users/me/shots/screen.PNG')).toMatchObject({ name: 'screen.PNG', kind: 'image' });
    expect(droppedFile('/tmp/build.log')).toMatchObject({ name: 'build.log', kind: 'text' });
    expect(droppedFile('/tmp/archive.zip')).toMatchObject({ name: 'archive.zip', kind: 'binary' });
    expect(droppedFile('/tmp/Makefile')).toMatchObject({ name: 'Makefile', kind: 'binary' });
    expect(droppedFile('C:\\Users\\me\\a.svg')).toMatchObject({ name: 'a.svg', kind: 'image' });
  });

  it('reports an unmeasured size as 0, because the drop event carries no size', () => {
    expect(droppedFile('/tmp/a.png').size).toBe(0);
    expect(droppedFile('/tmp/a.png').path).toBe('/tmp/a.png');
  });

  it('keeps the path verbatim — it is the key the Rust allow-list matches on', () => {
    expect(droppedFile('/tmp/a b.png').path).toBe('/tmp/a b.png');
  });
});
