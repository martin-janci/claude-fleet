// Orbit Fleet 11.5: Settings → Federation is a master_detail page over the
// `peer_link` resource: each linked hub with its state, latency and message
// counts; Link a hub sends the address and the one-time code; Unlink asks
// first.
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ResourcePage from './ResourcePage.svelte';
import { hubStatus, STANDALONE } from '../hub';
import { toasts } from '../toasts';
import { bundle } from './testing';
import type { Page } from './pages';
import { SETTINGS_TREE } from '../settings_tree';
import { expectAccessible } from '../a11y_check';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const page = bundle.pages.find((p) => p.id === 'settings.federation') as Page;
const resource = bundle.resources.find((r) => r.id === 'peer_link')!;

const acme = {
  id: 1,
  fleet_id: 'acme',
  title: 'acme',
  role: 'dialer',
  url: 'https://hub.acme.example',
  state: 'connected',
  last_exchange_at: null,
  last_error: null,
  pending: 2,
  revoked_at: null,
  latency_ms: 42,
  latency: '42 ms',
  messages_today: 7,
  messages_total: 310,
  sync: undefined as unknown,
};
const beta = {
  ...acme,
  id: 2,
  fleet_id: null,
  title: 'Link 2',
  role: 'listener',
  url: null,
  state: 'retrying',
  last_error: 'HTTP 502',
  latency_ms: undefined,
  latency: undefined,
  messages_today: 0,
  messages_total: 0,
  pending: 0,
};

const argsOf = (cmd: string) =>
  (invoke.mock.calls.filter((c) => c[0] === cmd).at(-1)![1] as { args: Record<string, unknown> }).args;

beforeEach(() => {
  hubStatus.set({ ...STANDALONE });
  toasts.set([]);
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === 'list_peer_links') return [acme, beta];
    if (cmd === 'unlink_peer_hub') return { id: 1, failed_messages: 2 };
    return null;
  });
});

describe('Settings → Federation', () => {
  const show = () => render(ResourcePage, { props: { page, resource } });

  it('is a leaf under Organisations', () => {
    const group = SETTINGS_TREE.find((g) => g.title === 'Organisations')!;
    expect(group.items.find((i) => i.id === 'federation')?.page).toBe('settings.federation');
  });

  it('lists each linked hub with its state', async () => {
    show();
    await waitFor(() => expect(screen.getAllByTestId('resource-row')).toHaveLength(2));
    const [a, b] = screen.getAllByTestId('resource-row');
    expect(a.textContent).toContain('acme');
    expect(within(a).getAllByTestId('resource-badge').map((x) => x.textContent)).toEqual(['Connected']);
    expect(within(b).getAllByTestId('resource-badge').map((x) => x.textContent)).toEqual(['Retrying']);
  });

  it('shows a link’s latency and what it carried', async () => {
    show();
    await fireEvent.click((await screen.findAllByTestId('resource-row'))[0]);
    expect(screen.getByTestId('value-latency').textContent).toBe('42 ms');
    expect(screen.getByTestId('value-messages_today').textContent).toBe('7');
    expect(screen.getByTestId('value-messages_total').textContent).toBe('310');
    expect(screen.getByTestId('value-pending').textContent).toBe('2');
    expect(screen.getByTestId('value-role').textContent).toBe('We dial');
  });

  it('unlinks a hub after asking', async () => {
    show();
    await fireEvent.click((await screen.findAllByTestId('resource-row'))[0]);
    await fireEvent.click(screen.getByTestId('record-delete'));
    expect(invoke.mock.calls.some((c) => c[0] === 'unlink_peer_hub')).toBe(false);
    await fireEvent.click(screen.getByTestId('record-confirm'));
    await waitFor(() => expect(argsOf('unlink_peer_hub')).toEqual({ id: 1 }));
  });
});

describe('Settings → Federation: loaders (11.12)', () => {
  const show = () => render(ResourcePage, { props: { page, resource } });

  it('shows a Constellation with the count from the link’s message counters while its queue drains', async () => {
    // The clock stands still (only Date is faked, so the loader's delay and
    // the page's own timers still run): a render that crosses a second
    // boundary would otherwise read 19 s.
    vi.useFakeTimers({ toFake: ['Date'] });
    try {
      vi.setSystemTime(new Date('2026-10-09T01:00:00.900Z'));
      const t = Math.floor(Date.now() / 1000);
      invoke.mockImplementation(async (cmd: string) =>
        cmd === 'list_peer_links' ? [{ ...acme, sync: { done: 412, total: 1280, both_ways: false, since: t - 18 } }] : null,
      );
      show();
      await fireEvent.click((await screen.findAllByTestId('resource-row'))[0]);
      expect(screen.getByTestId('sync-count-sync').textContent).toBe('412 of 1 280 messages · 18 s');
      // The loader shows after its 400 ms delay; a loaded box may take longer.
      const loader = await screen.findByTestId('sync-loader-sync', {}, { timeout: 5000 });
      expect(loader.getAttribute('data-loader')).toBe('constellation');
    } finally {
      vi.useRealTimers();
    }
  });

  it('shows a Counter-orbit while the hubs trade with nothing queued, and nothing when idle', async () => {
    invoke.mockImplementation(async (cmd: string) =>
      cmd === 'list_peer_links' ? [{ ...acme, sync: { done: 7, total: 7, both_ways: true } }, beta] : null,
    );
    show();
    const rows = await screen.findAllByTestId('resource-row');
    await fireEvent.click(rows[0]);
    expect(screen.getByTestId('sync-count-sync').textContent).toBe('Trading both ways');
    expect((await screen.findByTestId('sync-loader-sync', {}, { timeout: 5000 })).getAttribute('data-loader')).toBe('counter-orbit');
    await fireEvent.click(rows[1]);
    expect(screen.queryByTestId('sync-sync')).toBeNull();
  });
});

describe('Settings → Federation: accessibility', () => {
  it('the list and a linked hub’s detail is accessible', async () => {
    const { container } = render(ResourcePage, { props: { page, resource } });
    await fireEvent.click((await screen.findAllByTestId('resource-row'))[0]);
    await screen.findByTestId('value-latency');
    await expectAccessible(container);
  });
});

describe('Settings → Federation: the graph (Federation board)', () => {
  const show = () => render(ResourcePage, { props: { page, resource } });

  it('draws each linked hub joined to this hub, solid while up and dashed while down', async () => {
    show();
    const graph = await screen.findByTestId('resource-graph');
    const links = within(graph).getAllByTestId('graph-link');
    expect(links.map((l) => l.getAttribute('data-up'))).toEqual(['true', 'false']);
    expect(links[1].classList.contains('down')).toBe(true);
    // The words say what the picture does.
    expect(graph.querySelector('svg')!.getAttribute('aria-label')).toBe(
      'This hub, linked to 2: acme up, Link 2 down (Retrying).',
    );
    expect(graph.textContent).toContain('42 ms · 7 messages today');
    expect(screen.getByTestId('resource-graph-legend').textContent).toContain('Dashed line: link down');
  });

  it('draws nothing with no link', async () => {
    invoke.mockImplementation(async (cmd: string) => (cmd === 'list_peer_links' ? [] : null));
    show();
    await screen.findByTestId('resource-empty');
    expect(screen.queryByTestId('resource-graph')).toBeNull();
  });
});

describe('Settings → Federation: Link two hubs (11.12)', () => {
  it('asks the address, then the code, and runs a Counter-orbit while the hubs trade keys', async () => {
    let finish: (v: unknown) => void = () => {};
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'list_peer_links') return [];
      if (cmd === 'link_peer_hub') return new Promise((r) => (finish = r));
      return null;
    });
    render(ResourcePage, { props: { page, resource } });
    await fireEvent.click(await screen.findByTestId('resource-add'));
    const dialog = await screen.findByTestId('wizard-link_peer');
    expect(dialog.textContent).toContain('Link two hubs');
    await fireEvent.input(within(dialog).getByLabelText(/Hub address/), { target: { value: 'https://hub.b.example' } });
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Next' }));
    expect(dialog.textContent).toContain('fleet-hub pair --mode peer');
    await fireEvent.input(within(dialog).getByLabelText(/Link code/), { target: { value: 'AB12CD34' } });
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Link' }));
    await waitFor(() => expect(argsOf('link_peer_hub')).toEqual({ url: 'https://hub.b.example', code: 'AB12CD34' }));
    const running = await screen.findByTestId('wizard-running');
    // The loader shows after its 400 ms delay; a loaded box may take longer.
    await waitFor(() => expect(running.querySelector('[data-loader]')?.getAttribute('data-loader')).toBe('counter-orbit'), {
      timeout: 5000,
    });
    // Nothing in the exchange waits on a person: no "waiting for" line.
    expect(dialog.textContent).not.toMatch(/waiting for/i);
    finish({ ...acme, id: 3 });
    await waitFor(() => expect(screen.queryByTestId('wizard-link_peer')).toBeNull());
  });

  it('keeps the wizard open with the hub’s refusal', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'list_peer_links') return [];
      if (cmd === 'link_peer_hub') throw { code: 'E_INVALID', message: 'that code expired' };
      return null;
    });
    render(ResourcePage, { props: { page, resource } });
    await fireEvent.click(await screen.findByTestId('resource-add'));
    const dialog = await screen.findByTestId('wizard-link_peer');
    await fireEvent.input(within(dialog).getByLabelText(/Hub address/), { target: { value: 'https://hub.b.example' } });
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Next' }));
    await fireEvent.input(within(dialog).getByLabelText(/Link code/), { target: { value: 'AB12CD34' } });
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Link' }));
    expect((await screen.findByTestId('wizard-error')).textContent).toContain('that code expired');
  });
});
