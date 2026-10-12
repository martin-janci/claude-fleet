import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import ChangesetCard from './ChangesetCard.svelte';
import type { ChangesetSummary, ChangesetView, ItemView } from './assets_workspace';

const item = (o: Partial<ItemView>): ItemView => ({ position: 0, grp: 'core', catalog: 'personal', kind: 'skill', name: 'w', action: 'import', params: {}, decider: 'rule', state: 'pending', ...o });
const boot: ChangesetSummary = { id: 7, kind: 'bootstrap', summary: 'Adopt 3 as 2 layers; 1 need a look', state: 'proposed', created_at: 1, groups: { core: 2, authoring: 1, 'needs a look': 1 }, pending: 4, catalogs: ['personal', 'papayapos'] };
const bootView: ChangesetView = { id: 7, kind: 'bootstrap', summary: boot.summary, state: 'proposed', created_at: 1, commits: {}, undoable: false, catalogs: ['personal', 'papayapos'], items: [
  item({ position: 0 }), item({ position: 1, name: 'x', catalog: 'papayapos', decider: 'person' }), item({ position: 2, grp: 'authoring', name: 'y', decider: 'rule' }),
  item({ position: 3, grp: 'needs a look', name: 'z', params: { reason: 'copy on oci differs' } }),
] };
const props = (o = {}) => ({ card: boot, view: bootView, selected: false, readOnly: false, busy: false, primary: false, onselect: vi.fn(), onapply: vi.fn(), ondismiss: vi.fn(), onundo: vi.fn(), ...o });

describe('ChangesetCard', () => {
  it('shows the sentence, one row per group with its count and decider, and the looks as chips', () => {
    render(ChangesetCard, props());
    expect(screen.getByTestId('card-7')).toHaveTextContent('Adopt 3 as 2 layers');
    expect(screen.getByText('core')).toBeInTheDocument();
    expect(screen.getByText('rule')).toBeInTheDocument();
    expect(screen.getByText('rule + person')).toBeInTheDocument();
    expect(screen.getByTestId('card-look-7')).toHaveTextContent('z · copy on oci differs');
  });
  it('applies with its primary and says what applying does', async () => {
    const p = props({ primary: true });
    render(ChangesetCard, p);
    const primary = screen.getByTestId('card-primary-7');
    expect(primary).toHaveTextContent('Adopt 3 as 2 layers');
    expect(primary).toHaveClass('btn--primary');
    await fireEvent.click(primary);
    expect(p.onapply).toHaveBeenCalledWith(null);
    expect(screen.getByText('2 commits: personal, papayapos. One Undo. No host is touched.')).toBeInTheDocument();
  });
  it('without primary the verb is a plain .btn', async () => {
    const p = props();
    render(ChangesetCard, p);
    const verb = screen.getByTestId('card-primary-7');
    expect(verb).toHaveClass('btn');
    expect(verb).not.toHaveClass('btn--primary');
    await fireEvent.click(verb);
    expect(p.onapply).toHaveBeenCalledWith(null);
  });
  it('a verb that only reviews selects the card instead of applying', async () => {
    const drift: ChangesetSummary = { id: 4, kind: 'drift', summary: 'oci edited skill/w', state: 'proposed', created_at: 1 };
    const p = props({ card: drift, view: null });
    render(ChangesetCard, p);
    await fireEvent.click(screen.getByTestId('card-primary-4'));
    expect(p.onselect).toHaveBeenCalled();
    expect(p.onapply).not.toHaveBeenCalled();
  });
  it('marks the selected card in words for assistive tech, not colour alone', () => {
    const { unmount } = render(ChangesetCard, props({ selected: true }));
    expect(screen.getByTestId('card-7').getAttribute('aria-current')).toBe('true');
    unmount();
    render(ChangesetCard, props());
    expect(screen.getByTestId('card-7').getAttribute('aria-current')).toBeNull();
  });
  it('clicking the card selects it', async () => {
    const p = props();
    render(ChangesetCard, p);
    await fireEvent.click(screen.getByTestId('card-7'));
    expect(p.onselect).toHaveBeenCalled();
  });
  it('dismisses', async () => {
    const p = props();
    render(ChangesetCard, p);
    await fireEvent.click(screen.getByTestId('card-dismiss-7'));
    expect(p.ondismiss).toHaveBeenCalled();
    expect(p.onselect).not.toHaveBeenCalled();
  });
  it('read-only: no verbs', () => {
    render(ChangesetCard, props({ readOnly: true }));
    expect(screen.queryByTestId('card-primary-7')).toBeNull();
    expect(screen.queryByTestId('card-dismiss-7')).toBeNull();
  });
  it('busy: verbs disabled', () => {
    render(ChangesetCard, props({ busy: true }));
    expect(screen.getByTestId('card-primary-7')).toBeDisabled();
  });
  it('a failed card says so in words and keeps its retry', () => {
    render(ChangesetCard, props({ card: { ...boot, state: 'failed', error: 'core: E_IO' }, view: { ...bootView, state: 'failed', error: 'core: E_IO' } }));
    expect(screen.getByRole('alert')).toHaveTextContent('Failed: core: E_IO');
    expect(screen.getByTestId('card-primary-7')).toBeInTheDocument();
  });
  it('a rollout lists its hosts with the copies it held back', () => {
    const r: ChangesetSummary = { id: 9, kind: 'rollout', summary: 'Roll out core to oci, htz', state: 'proposed', created_at: 1, groups: { core: 2 } };
    const rv: ChangesetView = { id: 9, kind: 'rollout', summary: r.summary, state: 'proposed', created_at: 1, commits: {}, undoable: false, items: [
      item({ position: 0, kind: 'host', name: 'oci', action: 'sync', state: 'skipped', outcome: { held: [{ kind: 'skill', name: 'w', why: 'edited' }] } }),
      item({ position: 1, kind: 'host', name: 'htz', action: 'sync' }),
    ] };
    const onsynchost = vi.fn();
    render(ChangesetCard, props({ card: r, view: rv, onsynchost }));
    expect(screen.getByTestId('card-primary-9')).toHaveTextContent('Roll out to 1 host');
    const oci = screen.getByTestId('card-host-9-oci');
    expect(oci).toHaveTextContent('skill/w — edited on the host · sync it yourself');
    fireEvent.click(oci.querySelector('button')!);
    expect(onsynchost).toHaveBeenCalledWith('oci');
  });
  it('an applied rollout that held a host back shows the held lines and Sync, and no Dismiss or primary', () => {
    const r: ChangesetSummary = { id: 9, kind: 'rollout', summary: 'Roll out core to oci', state: 'applied', created_at: 1, applied_at: 2, held_hosts: ['oci'] };
    const rv: ChangesetView = { id: 9, kind: 'rollout', summary: r.summary, state: 'applied', created_at: 1, commits: {}, undoable: false, items: [
      item({ position: 0, kind: 'host', name: 'oci', action: 'sync', state: 'applied', outcome: { held: [{ kind: 'skill', name: 'w', why: 'edited' }] } }),
    ] };
    const onsynchost = vi.fn();
    render(ChangesetCard, props({ card: r, view: rv, onsynchost }));
    expect(screen.getByTestId('card-host-9-oci')).toHaveTextContent('skill/w — edited on the host · sync it yourself');
    fireEvent.click(screen.getByRole('button', { name: 'Sync oci' }));
    expect(onsynchost).toHaveBeenCalledWith('oci');
    expect(screen.queryByTestId('card-dismiss-9')).toBeNull();
    expect(screen.queryByTestId('card-primary-9')).toBeNull();
  });
  it('an applied, undoable card is a banner with Undo', async () => {
    const p = props({ card: { ...boot, state: 'applied', undoable: true, applied_at: 1 }, view: { ...bootView, state: 'applied', undoable: true, commits: { personal: 'a1b2c3d4', papayapos: '9f0e1d2' } } });
    render(ChangesetCard, p);
    expect(screen.getByTestId('card-7')).toHaveTextContent('personal a1b2c3d');
    await fireEvent.click(screen.getByTestId('card-undo-7'));
    expect(p.onundo).toHaveBeenCalled();
  });
});
