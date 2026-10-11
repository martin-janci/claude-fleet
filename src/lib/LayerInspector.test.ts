import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import LayerInspector from './LayerInspector.svelte';
import type { LayerListing } from './assets_workspace';

const listing: LayerListing = {
  layers: [
    { name: 'base', axis: 'context', members: ['skill/w', 'skill/v'] },
    { name: 'server', axis: 'role', extends: 'base' },
    { name: 'extra', axis: 'context' },
  ],
  hosts: [{ host_alias: 'oci', catalog_id: 1, layer_name: 'server', axis: 'role', position: 0, active: true }],
};
const mount = (o: Record<string, unknown> = {}) => {
  const onchange = vi.fn();
  render(LayerInspector, { catalog: 'personal', layer: listing.layers[0], listing, order: ['local', 'oci'], writable: true, onchange, ...o });
  return onchange;
};

describe('LayerInspector', () => {
  it('names the layer and its catalog, with Assets · Changeset · Hosts · Source tabs (M15 G7.13)', () => {
    mount();
    expect(screen.getByText('Layer · catalog personal')).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'base' })).toBeTruthy();
    expect(screen.getAllByRole('tab').map((t) => t.textContent)).toEqual(['Assets 2', 'Changeset', 'Hosts', 'Source']);
    expect(screen.getByTestId('layer-member-skill/w')).toBeTruthy();
    expect(screen.getByTestId('layer-member-skill/v')).toBeTruthy();
  });
  it('Move… on a member opens the move form, whose submit proposes the move', async () => {
    const onchange = mount();
    await fireEvent.click(screen.getByTestId('layer-move-skill/w'));
    expect(screen.getByTestId('layer-form')).toBeTruthy();
    await fireEvent.change(screen.getByTestId('layer-form-to'), { target: { value: 'extra' } });
    await fireEvent.click(screen.getByTestId('layer-form-submit'));
    expect(onchange).toHaveBeenCalledWith({ op: 'move', catalog: 'personal', member: 'skill/w', layer: 'base', to: 'extra' });
  });
  it('the Changeset tab lists the proposed cards that change the layer, and opens one', async () => {
    const onopencard = vi.fn();
    const card = (id: number, kind: string, summary: string, over: Record<string, unknown> = {}) => ({
      id, kind, summary, state: 'proposed', created_at: 1, catalogs: ['personal'], ...over,
    });
    mount({
      onopencard,
      cards: [
        card(1, 'layer', 'Move skill/w into base'),
        card(2, 'layer', 'Rename baseline to core'),
        card(3, 'rollout', 'Roll out base to oci', { state: 'applied' }),
        card(4, 'rollout', 'Roll out base to oci', { catalogs: ['team'] }),
      ],
    });
    expect(screen.getAllByRole('tab')[1].textContent).toBe('Changeset 1');
    await fireEvent.click(screen.getByTestId('inspector-tab-changeset'));
    expect(screen.getByTestId('layer-card-1')).toBeTruthy();
    expect(screen.queryByTestId('layer-card-2')).toBeNull();
    await fireEvent.click(screen.getByTestId('layer-card-open-1'));
    expect(onopencard).toHaveBeenCalledWith(1);
  });
  it('the Source tab shows the layer as the catalog defines it', async () => {
    mount({ layer: listing.layers[1] });
    await fireEvent.click(screen.getByTestId('inspector-tab-source'));
    expect(screen.getByTestId('layer-source').textContent).toBe('name: server\naxis: role\nextends: base\nmembers: []');
  });
  it('the Hosts tab answers “why is it on oci?” with the chain', async () => {
    mount();
    await fireEvent.click(screen.getByTestId('inspector-tab-hosts'));
    expect(screen.getByTestId('layer-why-oci')).toHaveTextContent('role server → extends base');
    expect(screen.getByTestId('layer-why-oci').tagName).toBe('CODE');
    expect(screen.getByText('Why is it on oci?')).toBeTruthy();
  });
  it('says so when no host receives the layer', async () => {
    mount({ layer: listing.layers[2] });
    await fireEvent.click(screen.getByTestId('inspector-tab-hosts'));
    expect(screen.getByRole('tabpanel')).toHaveTextContent('No host receives this layer');
  });
  it('Rename opens the rename form and its submit calls onchange', async () => {
    const onchange = mount();
    await fireEvent.click(screen.getByTestId('layer-rename'));
    await fireEvent.input(screen.getByTestId('layer-form-name'), { target: { value: 'core' } });
    await fireEvent.click(screen.getByTestId('layer-form-submit'));
    expect(onchange).toHaveBeenCalledWith({ op: 'rename', catalog: 'personal', layer: 'base', to: 'core' });
  });
  it('the form’s Propose is the Inspector’s one primary; Cancel closes the form', async () => {
    mount();
    expect(document.querySelectorAll('.btn--primary')).toHaveLength(0);
    await fireEvent.click(screen.getByTestId('layer-rename'));
    expect(document.querySelectorAll('.btn--primary')).toHaveLength(1);
    await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByTestId('layer-form')).toBeNull();
  });
  it('not writable: no rename and no move', () => {
    mount({ writable: false });
    expect(screen.queryByTestId('layer-rename')).toBeNull();
    expect(screen.queryByTestId('layer-move-skill/w')).toBeNull();
    expect(screen.getByTestId('layer-member-skill/w')).toBeTruthy();
  });
});
