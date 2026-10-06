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
  it('names the layer and its catalog, with Members and Hosts tabs', () => {
    mount();
    expect(screen.getByText('Layer · catalog personal')).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'base' })).toBeTruthy();
    expect(screen.getAllByRole('tab').map((t) => t.textContent)).toEqual(['Members', 'Hosts']);
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
