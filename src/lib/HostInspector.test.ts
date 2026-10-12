import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import HostInspector from './HostInspector.svelte';
import type { ResolutionView } from './assets_workspace';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const view = (o: Partial<ResolutionView> = {}): ResolutionView => ({ provenance: {}, excluded: {}, refused: [], withheld: [], held_back: {}, assets: [], ...o });
const answer = (v: ResolutionView) => invoke.mockImplementation(async (cmd: string) => {
  if (cmd === 'catalog_host_provenance') return v;
  throw { code: 'E_TEST', message: cmd };
});

beforeEach(() => {
  invoke.mockReset();
});

describe('HostInspector', () => {
  it('groups the effective set by catalog with the layer that brought each asset', async () => {
    answer(view({ provenance: {
      'skill/ppt': { introduced_by: 'acme-ops', catalog: 'papayapos' },
      'skill/w': { introduced_by: 'core', catalog: 'personal' },
      'skill/v': { introduced_by: 'core', overridden_by: ['acme-ops'], catalog: 'personal' },
    } }));
    render(HostInspector, { alias: 'oci' });
    expect(invoke).toHaveBeenCalledWith('catalog_host_provenance', { args: { host_alias: 'oci' } });
    const personal = await screen.findByTestId('host-prov-personal');
    expect(personal).toHaveTextContent('skill/w — via layer core from personal');
    expect(personal.querySelector('[data-testid="host-prov-line-skill/w"]')).toBeTruthy();
    expect(screen.getByTestId('host-prov-line-skill/v')).toHaveTextContent('(overridden by acme-ops)');
    expect(screen.getByTestId('host-prov-papayapos')).toHaveTextContent('skill/ppt — via layer acme-ops from papayapos');
    const order = screen.getAllByTestId(/^host-prov-(personal|papayapos)$/).map((e) => e.dataset.testid);
    expect(order).toEqual(['host-prov-personal', 'host-prov-papayapos']);
    expect(screen.getByRole('heading', { name: 'oci' })).toBeTruthy();
    expect(screen.getByText('Host')).toBeTruthy();
  });
  it('lists refused and held-back catalogs', async () => {
    answer(view({
      refused: [{ kind: 'skill', name: 'x', reason: 'private asset; oci belongs to an org' }],
      held_back: { papayapos: 'failed to load' },
    }));
    render(HostInspector, { alias: 'oci' });
    expect(await screen.findByTestId('host-refused')).toHaveTextContent('private asset; oci belongs to an org');
    expect(screen.getByTestId('host-held-back')).toHaveTextContent('papayapos: failed to load — its assets are left as they are');
  });
  it('an older hub or a refusal is said in words', async () => {
    invoke.mockRejectedValue({ code: 'E_INVALID', message: 'unknown changesets action host_provenance: list|…' });
    const { unmount } = render(HostInspector, { alias: 'oci' });
    expect(await screen.findByTestId('host-prov-error')).toHaveTextContent('The hub is older than this desktop');
    unmount();
    invoke.mockRejectedValue({ code: 'E_FORBIDDEN', message: 'catalog_admin needs a grant' });
    render(HostInspector, { alias: 'oci' });
    await waitFor(() => expect(screen.getByTestId('host-prov-error')).toHaveTextContent('needs a grant'));
  });
  it('says so when nothing resolves', async () => {
    answer(view());
    render(HostInspector, { alias: 'oci' });
    expect(await screen.findByText('Nothing resolves for this host yet.')).toBeTruthy();
  });
});
