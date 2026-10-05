import { describe, it, expect, vi, beforeEach } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { get } from 'svelte/store';
import { runCardVerb } from './card_actions';
import { toasts } from './toasts';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const VIEW = { id: 3, kind: 'new', summary: 'New on oci: skill/w → core', state: 'applied', created_at: 1, commits: { personal: 'abc' }, undoable: true, items: [] };

describe('runCardVerb', () => {
  beforeEach(() => { invoke.mockReset(); toasts.set([]); });
  it('applies, reloads, and offers Undo in the toast', async () => {
    invoke.mockResolvedValue(VIEW);
    const setBusy = vi.fn();
    const onchanged = vi.fn();
    const v = await runCardVerb('apply', 3, { setBusy, onchanged });
    expect(v?.id).toBe(3);
    expect(setBusy.mock.calls).toEqual([['card'], ['']]);
    expect(onchanged).toHaveBeenCalled();
    const t = get(toasts).at(-1)!;
    expect(t.message).toBe('Applied: New on oci: skill/w → core');
    expect(t.action?.label).toBe('Undo');
  });
  it('words a failed card from its error, not as success', async () => {
    invoke.mockResolvedValue({ ...VIEW, state: 'failed', undoable: false, error: 'oci: unreachable' });
    await runCardVerb('apply', 3, { setBusy: () => {}, onchanged: () => {} });
    expect(get(toasts).at(-1)!.message).toBe('Not applied: oci: unreachable');
  });
  it('words an older hub', async () => {
    invoke.mockRejectedValue({ code: 'E_INVALID', message: 'unknown changesets action propose_layer: list|…' });
    await runCardVerb('apply', 3, { setBusy: () => {}, onchanged: () => {} });
    expect(get(toasts).at(-1)!.message).toMatch(/^The hub is older than this desktop/);
  });
  it('names the verb in a refusal', async () => {
    invoke.mockRejectedValue({ code: 'E_IO', message: 'disk full' });
    await runCardVerb('dismiss', 5, { setBusy: () => {}, onchanged: () => {} });
    expect(get(toasts).at(-1)!.message).toBe('Dismiss card 5: disk full');
  });
  it('Undo in the toast runs the undo verb and reloads', async () => {
    invoke.mockResolvedValue(VIEW);
    const onchanged = vi.fn();
    await runCardVerb('apply', 3, { setBusy: () => {}, onchanged });
    invoke.mockClear();
    invoke.mockResolvedValue({ ...VIEW, state: 'undone', undoable: false });
    get(toasts).at(-1)!.action!.run();
    await vi.waitFor(() => expect(onchanged).toHaveBeenCalledTimes(2));
    expect(invoke).toHaveBeenCalledWith('catalog_undo_changeset', { args: { id: 3 } });
  });
});
