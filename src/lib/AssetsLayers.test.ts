import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import AssetsLayers from './AssetsLayers.svelte';
import type { LayerListing } from './assets_workspace';

const personal: LayerListing = {
  layers: [{ name: 'core', axis: 'context', members: ['skill/w', 'skill/v'] }, { name: 'server', axis: 'role' }],
  hosts: [{ host_alias: 'oci', catalog_id: 1, layer_name: 'core', axis: 'context', position: 0, active: true }],
};
const acme: LayerListing = { layers: [{ name: 'acme-ops', axis: 'context', members: ['skill/ppt'] }], hosts: [] };
const props = (o = {}) => ({ layers: { personal, acme }, order: ['local', 'oci'], selectedKey: null, readOnly: false, busy: false, onselect: vi.fn(), onnew: vi.fn(), onpropose: vi.fn(), ...o });

describe('AssetsLayers', () => {
  it('groups layers by catalog, personal first, with members and footprint', () => {
    render(AssetsLayers, props());
    const groups = screen.getAllByTestId(/^layers-catalog-/).map((e) => e.dataset.testid);
    expect(groups).toEqual(['layers-catalog-personal', 'layers-catalog-acme']);
    const core = screen.getByTestId('layer-row-personal-core');
    expect(core).toHaveTextContent('2');
    expect(core.querySelector('[role="img"]')?.getAttribute('aria-label')).toContain('oci');
  });
  it('selects a layer by key', async () => {
    const p = props();
    render(AssetsLayers, p);
    await fireEvent.click(screen.getByTestId('layer-row-acme-acme-ops'));
    expect(p.onselect).toHaveBeenCalledWith('layer:acme:acme-ops');
  });
  it('offers New layer and Propose again, not when read-only', async () => {
    const p = props();
    const { unmount } = render(AssetsLayers, p);
    await fireEvent.click(screen.getByTestId('layers-new'));
    await fireEvent.click(screen.getByTestId('layers-propose'));
    expect(p.onnew).toHaveBeenCalled();
    expect(p.onpropose).toHaveBeenCalled();
    unmount();
    render(AssetsLayers, props({ readOnly: true }));
    expect(screen.queryByTestId('layers-new')).toBeNull();
    expect(screen.queryByTestId('layers-propose')).toBeNull();
  });
  it('says when nothing is loaded', () => {
    render(AssetsLayers, props({ layers: {} }));
    expect(screen.getByTestId('layers-view')).toHaveTextContent('No layers yet');
  });
  it('marks the selected row and keeps every control quiet (the main column’s primary is the form’s)', () => {
    render(AssetsLayers, props({ selectedKey: 'layer:personal:core' }));
    expect(screen.getByTestId('layer-row-personal-core').getAttribute('aria-current')).toBe('true');
    expect(screen.getByTestId('layer-row-personal-server').getAttribute('aria-current')).toBeNull();
    expect(document.querySelectorAll('.btn--primary')).toHaveLength(0);
  });
});
