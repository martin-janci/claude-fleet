import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import AssetsHosts from './AssetsHosts.svelte';
import type { CatalogStatus, LayerListing } from './assets_workspace';

const cat = (o: Partial<CatalogStatus>): CatalogStatus => ({ id: 1, name: 'personal', org_id: null, repo_path: '', remote_url: null, head_commit: null, last_loaded_at: null, state: 'loaded', asset_count: 0, admitted: [], ...o });
const statuses = [cat({}), cat({ id: 2, name: 'papayapos', org_id: 7, admitted: ['local'] })];
const layers: Record<string, LayerListing> = {
  personal: { layers: [], hosts: [{ host_alias: 'oci', catalog_id: 1, layer_name: 'server', axis: 'role', position: 0, active: true }] },
};
const hosts = [{ alias: 'local', org_id: null }, { alias: 'oci', org_id: null }, { alias: 'trn', org_id: 7 }] as never[];
const props = (o = {}) => ({ hosts, statuses, layers, orgName: (id: number | null) => (id === 7 ? 'papayapos' : null), selectedKey: null, readOnly: false, busy: false, onselect: vi.fn(), ontoggle: vi.fn(), ...o });

describe('AssetsHosts', () => {
  it('shows each host org and its role per catalog', () => {
    render(AssetsHosts, props());
    expect(screen.getByTestId('host-org-trn')).toHaveTextContent('papayapos');
    expect(screen.getByTestId('host-org-oci')).toHaveTextContent('no org');
    expect(screen.getByTestId('host-role-oci-personal')).toHaveTextContent('role server');
    expect(screen.queryByTestId('host-role-local-personal')).toBeNull();
  });
  it('admission toggles follow acceptance: locked for personal and own org, a toggle for an org-less host', async () => {
    const p = props();
    render(AssetsHosts, p);
    const personal = screen.getByTestId('host-accept-oci-personal');
    expect(personal).toBeDisabled();
    expect(personal).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByTestId('host-accept-trn-papayapos')).toBeDisabled();
    expect(screen.getByTestId('host-accept-trn-personal')).toHaveTextContent('shared only');
    const local = screen.getByTestId('host-accept-local-papayapos');
    expect(local).toHaveAttribute('aria-pressed', 'true');
    await fireEvent.click(local);
    expect(p.ontoggle).toHaveBeenCalledWith('local', 'papayapos', false);
    await fireEvent.click(screen.getByTestId('host-accept-oci-papayapos'));
    expect(p.ontoggle).toHaveBeenLastCalledWith('oci', 'papayapos', true);
    // The toggle is not a row selection.
    expect(p.onselect).not.toHaveBeenCalled();
  });
  it('read-only: no toggles, the state in words', () => {
    render(AssetsHosts, props({ readOnly: true }));
    expect(screen.getByTestId('host-accept-local-papayapos')).toBeDisabled();
    expect(screen.getByTestId('host-accept-oci-papayapos')).toHaveTextContent('not admitted');
  });
  it('a busy view disables the toggles', () => {
    render(AssetsHosts, props({ busy: true }));
    expect(screen.getByTestId('host-accept-oci-papayapos')).toBeDisabled();
  });
  it('a row selects its host by click, Enter and Space, but a toggle’s key does not', async () => {
    const p = props();
    render(AssetsHosts, p);
    const row = screen.getByTestId('host-row-oci');
    expect(row.tagName).toBe('DIV');
    expect(row).toHaveAttribute('role', 'button');
    await fireEvent.click(row);
    expect(p.onselect).toHaveBeenLastCalledWith('host:oci');
    await fireEvent.keyDown(row, { key: 'Enter' });
    await fireEvent.keyDown(row, { key: ' ' });
    expect(p.onselect).toHaveBeenCalledTimes(3);
    await fireEvent.keyDown(screen.getByTestId('host-accept-oci-papayapos'), { key: ' ' });
    expect(p.onselect).toHaveBeenCalledTimes(3);
  });
  it('marks the selected host', () => {
    render(AssetsHosts, props({ selectedKey: 'host:trn' }));
    expect(screen.getByTestId('host-row-trn')).toHaveAttribute('aria-current', 'true');
    expect(screen.getByTestId('host-row-oci')).not.toHaveAttribute('aria-current');
  });
});
