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

  it('links a hub with its address and code', async () => {
    show();
    await fireEvent.click(await screen.findByTestId('resource-add'));
    await fireEvent.input(screen.getByTestId('param-peer_link.add-url'), { target: { value: 'https://hub.b.example' } });
    await fireEvent.input(screen.getByTestId('param-peer_link.add-code'), { target: { value: 'AB12CD34' } });
    await fireEvent.click(screen.getByTestId('run-peer_link.add'));
    await waitFor(() => expect(argsOf('link_peer_hub')).toEqual({ url: 'https://hub.b.example', code: 'AB12CD34' }));
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
    const t = Math.floor(Date.now() / 1000);
    invoke.mockImplementation(async (cmd: string) =>
      cmd === 'list_peer_links' ? [{ ...acme, sync: { done: 412, total: 1280, both_ways: false, since: t - 18 } }] : null,
    );
    show();
    await fireEvent.click((await screen.findAllByTestId('resource-row'))[0]);
    expect(screen.getByTestId('sync-count-sync').textContent).toBe('412 of 1 280 messages · 18 s');
    expect((await screen.findByTestId('sync-loader-sync')).getAttribute('data-loader')).toBe('constellation');
  });

  it('shows a Counter-orbit while the hubs trade with nothing queued, and nothing when idle', async () => {
    invoke.mockImplementation(async (cmd: string) =>
      cmd === 'list_peer_links' ? [{ ...acme, sync: { done: 7, total: 7, both_ways: true } }, beta] : null,
    );
    show();
    const rows = await screen.findAllByTestId('resource-row');
    await fireEvent.click(rows[0]);
    expect(screen.getByTestId('sync-count-sync').textContent).toBe('Trading both ways');
    expect((await screen.findByTestId('sync-loader-sync')).getAttribute('data-loader')).toBe('counter-orbit');
    await fireEvent.click(rows[1]);
    expect(screen.queryByTestId('sync-sync')).toBeNull();
  });

  it('runs a Counter-orbit while Link a hub talks to the other hub', async () => {
    let finish: (v: unknown) => void = () => {};
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'list_peer_links') return [];
      if (cmd === 'link_peer_hub') return new Promise((r) => (finish = r));
      return null;
    });
    show();
    await fireEvent.click(await screen.findByTestId('resource-add'));
    await fireEvent.input(screen.getByTestId('param-peer_link.add-url'), { target: { value: 'https://hub.b.example' } });
    await fireEvent.input(screen.getByTestId('param-peer_link.add-code'), { target: { value: 'AB12CD34' } });
    await fireEvent.click(screen.getByTestId('run-peer_link.add'));
    await waitFor(() => expect(screen.getByTestId('busy-peer_link.add')).toBeTruthy());
    finish(null);
    await waitFor(() => expect(screen.queryByTestId('busy-peer_link.add')).toBeNull());
  });
});
