import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import ChangesetDetail from './ChangesetDetail.svelte';
import type { ChangesetView, ItemView } from './assets_workspace';

const item = (o: Partial<ItemView>): ItemView => ({ position: 0, grp: 'core', catalog: 'personal', kind: 'skill', name: 'w', action: 'import', params: {}, decider: 'rule', state: 'pending', ...o });
const v: ChangesetView = { id: 7, kind: 'bootstrap', summary: 'Adopt', state: 'proposed', created_at: 1, commits: {}, undoable: false, items: [
  item({ position: 0 }), item({ position: 1, name: 'x' }), item({ position: 2, grp: 'authoring', name: 'y', state: 'rejected' }),
] };
const props = (o = {}) => ({ view: v, readOnly: false, busy: false, onreject: vi.fn(), onapply: vi.fn(), onundo: vi.fn(), ...o });

describe('ChangesetDetail', () => {
  it('lists every item by group with its action, decider and state', () => {
    render(ChangesetDetail, props());
    expect(screen.getByTestId('card-item-7-0')).toHaveTextContent('skill/w');
    expect(screen.getByTestId('card-item-7-0')).toHaveTextContent('import');
    expect(screen.getByTestId('card-item-7-2')).toHaveTextContent('rejected');
  });
  it('rejects one pending item, or skips a whole group', async () => {
    const p = props();
    render(ChangesetDetail, p);
    await fireEvent.click(screen.getByTestId('card-reject-7-1'));
    expect(p.onreject).toHaveBeenLastCalledWith([1]);
    await fireEvent.click(screen.getByTestId('card-skip-7-core'));
    expect(p.onreject).toHaveBeenLastCalledWith([0, 1]);
    expect(screen.queryByTestId('card-reject-7-2')).toBeNull();
  });
  it('an applied card lists its commits and offers Undo when undoable', async () => {
    const p = props({ view: { ...v, state: 'applied', undoable: true, commits: { personal: 'abcdef1234' } } });
    render(ChangesetDetail, p);
    expect(screen.getByTestId('card-commits-7')).toHaveTextContent('personal abcdef1');
    await fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
    expect(p.onundo).toHaveBeenCalled();
  });
  it('read-only: no reject, skip or undo', () => {
    render(ChangesetDetail, props({ readOnly: true }));
    expect(screen.queryByTestId('card-reject-7-0')).toBeNull();
    expect(screen.queryByTestId('card-skip-7-core')).toBeNull();
  });
  it('shows what a host held back, per item as given', () => {
    const rv: ChangesetView = { ...v, kind: 'rollout', items: [
      item({ position: 0, kind: 'host', name: 'oci', action: 'sync', grp: 'core', state: 'skipped', outcome: { held: [{ kind: 'skill', name: 'w', why: 'edited' }] } }),
    ] };
    render(ChangesetDetail, props({ view: rv }));
    expect(screen.getByTestId('card-item-7-0')).toHaveTextContent('skill/w — edited on the host · sync it yourself');
  });
});
