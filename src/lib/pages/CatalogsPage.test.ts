// Assets M6 (R10, R11): Settings → Catalogs is a master_detail page over the
// `catalog` resource — a list, an add form whose org is picked from the orgs,
// hosts admitted and unadmitted as chips, and the grants shown read-only.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ResourcePage from './ResourcePage.svelte';
import { hubStatus, STANDALONE } from '../hub';
import { hosts } from '../hosts';
import { host } from '../hosts_fixture';
import { orgs, type OrgDetail } from '../orgs';
import { toasts } from '../toasts';
import { bundle } from './testing';
import type { Page } from './pages';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const page = bundle.pages.find((p) => p.id === 'settings.catalogs') as Page;
const resource = bundle.resources.find((r) => r.id === 'catalog')!;

const ROW = {
  id: 2,
  name: 'acme',
  org_id: 1,
  org: 'Acme',
  repo_path: '/r/acme',
  remote_url: null,
  head_commit: 'abc',
  last_loaded_at: 1,
  state: 'loaded',
  asset_count: 3,
  admitted: ['mefistos'],
  granted: ['laptop'],
};

const acme = { id: 1, name: 'Acme', rules: [], hosts: [], trackers: [] } as unknown as OrgDetail;

beforeEach(() => {
  hubStatus.set({ ...STANDALONE });
  toasts.set([]);
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === 'catalog_list_catalogs') return [ROW];
    if (cmd === 'list_orgs') return [acme];
    if (cmd === 'catalog_unadmit_catalog' || cmd === 'catalog_admit_catalog') return [];
    if (cmd === 'catalog_add_catalog') return ROW;
    return null;
  });
  orgs.set([acme]);
  hosts.set([host('mefistos'), host('oci')]);
});

/** The catalog's row in the list (its name also shows in the detail). */
const openRow = () => screen.findByRole('option', { name: /acme/ });

const show = () => render(ResourcePage, { props: { page, resource } });

describe('Settings → Catalogs', () => {
  it('lists catalogs with who admits and who is granted', async () => {
    show();
    await fireEvent.click(await openRow());
    expect(await screen.findByText('mefistos')).toBeInTheDocument();
    expect(screen.getByText('laptop')).toBeInTheDocument();
  });

  it('unadmits a host with the catalog name', async () => {
    show();
    await fireEvent.click(await openRow());
    await fireEvent.click(await screen.findByRole('button', { name: /Remove: mefistos/i }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('catalog_unadmit_catalog', {
        args: { host_alias: 'mefistos', catalog: 'acme' },
      }),
    );
  });

  it('offers the orgs as the new catalog org, and sends the pick', async () => {
    show();
    await fireEvent.click(await screen.findByTestId('resource-add'));
    expect(await screen.findByRole('option', { name: 'Acme' })).toBeInTheDocument();
    await fireEvent.input(screen.getByTestId('param-catalog.add-name'), { target: { value: 'papaya' } });
    await fireEvent.input(screen.getByTestId('param-catalog.add-repo_path'), { target: { value: '/r/papaya' } });
    await fireEvent.change(screen.getByTestId('param-catalog.add-org'), { target: { value: 'Acme' } });
    await fireEvent.click(screen.getByTestId('run-catalog.add'));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('catalog_add_catalog', {
        args: { name: 'papaya', repo_path: '/r/papaya', remote_url: null, org: 'Acme' },
      }),
    );
  });

  it('shows granted clients read-only', async () => {
    show();
    await fireEvent.click(await openRow());
    expect(await screen.findByText('laptop')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Remove: laptop/i })).toBeNull();
    expect(screen.queryByTestId('item-remove-granted')).toBeNull();
  });
});
