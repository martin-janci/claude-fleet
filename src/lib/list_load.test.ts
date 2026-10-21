import { describe, it, expect } from 'vitest';
import { get } from 'svelte/store';
import { createListLoad } from './list_load';
import type { Result } from './result';

describe('createListLoad (review w17-1)', () => {
  it('goes loading → loaded, or loading → failed with the error', async () => {
    const l = createListLoad();
    expect(get(l).state).toBe('idle');
    let done!: (v: Result<number>) => void;
    const p = l.track(new Promise<Result<number>>((r) => (done = r)));
    expect(get(l).state).toBe('loading');
    done({ ok: true, value: 1 });
    await p;
    expect(get(l)).toEqual({ state: 'loaded', error: null });
    await l.track(Promise.resolve({ ok: false as const, error: { code: 'E_X', message: 'boom' } }));
    expect(get(l)).toEqual({ state: 'failed', error: { code: 'E_X', message: 'boom' } });
  });

  it('a stale answer does not overwrite a newer one', async () => {
    const l = createListLoad();
    let first!: (v: Result<number>) => void;
    const a = l.track(new Promise<Result<number>>((r) => (first = r)));
    await l.track(Promise.resolve({ ok: true as const, value: 2 }));
    first({ ok: false, error: { code: 'E_X', message: 'late' } });
    await a;
    expect(get(l).state).toBe('loaded');
  });
});
