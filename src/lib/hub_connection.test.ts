// SF-8: the design says a dropped hub stream reconnects "showing a banner
// while disconnected". This store is that banner's source of truth, and the
// banner component renders it.
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  hubConnection,
  startHubConnection,
  connectionBanner,
  type HubConnection,
} from './hub_connection';
import HubConnectionBanner from './HubConnectionBanner.svelte';

const inv = () => invoke as ReturnType<typeof vi.fn>;
const lis = () => listen as ReturnType<typeof vi.fn>;

const reconnecting: HubConnection = {
  state: 'reconnecting',
  attempt: 3,
  retry_in_secs: 4,
  reason: 'the hub closed the event stream',
};
const offline: HubConnection = {
  state: 'offline',
  attempt: 2,
  retry_in_secs: 2,
  reason: 'connect fleet.example.com:443: connection refused',
};
const hubTooOld: HubConnection = { state: 'hub_too_old', hub_contract: 0, min_contract: 2 };
const hubTooNew: HubConnection = { state: 'hub_too_new', hub_contract: 5, max_contract: 1 };

beforeEach(() => {
  inv().mockReset();
  lis().mockReset();
  hubConnection.set({ state: 'standalone' });
});

describe('the hub connection store', () => {
  it('asks for the current state, because the first event may predate the window', async () => {
    inv().mockResolvedValue(offline);
    lis().mockResolvedValue(() => {});
    await startHubConnection();
    expect(inv().mock.calls.map((c) => c[0])).toEqual(['hub_connection']);
    expect(get(hubConnection)).toEqual(offline);
  });

  it('follows every hub:connection event after that', async () => {
    inv().mockResolvedValue({ state: 'connecting' });
    let handler: ((e: { payload: HubConnection }) => void) | undefined;
    lis().mockImplementation(async (name: string, h: typeof handler) => {
      if (name === 'hub:connection') handler = h;
      return () => {};
    });
    await startHubConnection();
    handler!({ payload: reconnecting });
    expect(get(hubConnection)).toEqual(reconnecting);
    handler!({ payload: { state: 'connected' } });
    expect(get(hubConnection)).toEqual({ state: 'connected' });
  });

  it('a failed query leaves the store alone rather than inventing a state', async () => {
    inv().mockRejectedValue({ code: 'E_IPC', message: 'no' });
    lis().mockResolvedValue(() => {});
    hubConnection.set({ state: 'connecting' });
    await startHubConnection();
    expect(get(hubConnection)).toEqual({ state: 'connecting' });
  });
});

describe('what the banner says', () => {
  it('says nothing while the link is up, or when there is no hub at all', () => {
    for (const s of ['standalone', 'connecting', 'connected'] as const) {
      expect(connectionBanner({ state: s }, 'https://fleet.example.com'), s).toBeNull();
    }
  });

  it('a dropped stream: which hub, that the view may be stale, the attempt, the wait, the reason', () => {
    const text = connectionBanner(reconnecting, 'https://fleet.example.com')!;
    expect(text).toContain('fleet.example.com');
    expect(text.toLowerCase()).toContain('out of date');
    expect(text).toContain('attempt 3');
    expect(text).toContain('4 s');
    expect(text).toContain('the hub closed the event stream');
  });

  it('an unreachable hub says it cannot be reached', () => {
    const text = connectionBanner(offline, 'https://fleet.example.com')!;
    expect(text.toLowerCase()).toContain('cannot reach');
    expect(text).toContain('connection refused');
    expect(text).toContain('attempt 2');
  });

  it('a too-old hub names both revisions and says to update the hub', () => {
    const text = connectionBanner(hubTooOld, 'https://fleet.example.com')!;
    expect(text).toContain('fleet.example.com');
    expect(text).toContain('0');
    expect(text).toContain('2');
    expect(text.toLowerCase()).toContain('out of date');
    expect(text.toLowerCase()).toContain('update the hub');
  });

  it('a too-new hub names both revisions and says to update this app', () => {
    const text = connectionBanner(hubTooNew, 'https://fleet.example.com')!;
    expect(text).toContain('fleet.example.com');
    expect(text).toContain('5');
    expect(text).toContain('1');
    expect(text.toLowerCase()).toContain('out of date');
    expect(text.toLowerCase()).toContain('update this app');
  });
});

describe('the banner', () => {
  it('is absent while connected', () => {
    hubConnection.set({ state: 'connected' });
    render(HubConnectionBanner, { props: { hubUrl: 'https://fleet.example.com' } });
    expect(screen.queryByTestId('hub-connection-banner')).toBeNull();
  });

  it('shows while disconnected, as an alert', () => {
    hubConnection.set(reconnecting);
    render(HubConnectionBanner, { props: { hubUrl: 'https://fleet.example.com' } });
    const b = screen.getByTestId('hub-connection-banner');
    expect(b.getAttribute('role')).toBe('alert');
    expect(b.textContent).toContain('attempt 3');
  });

  it('shows the too-old sentence for a hub behind this app', () => {
    hubConnection.set(hubTooOld);
    render(HubConnectionBanner, { props: { hubUrl: 'https://fleet.example.com' } });
    expect(screen.getByTestId('hub-connection-banner').textContent?.toLowerCase()).toContain(
      'update the hub'
    );
  });

  it('shows the too-new sentence for a hub ahead of this app', () => {
    hubConnection.set(hubTooNew);
    render(HubConnectionBanner, { props: { hubUrl: 'https://fleet.example.com' } });
    expect(screen.getByTestId('hub-connection-banner').textContent?.toLowerCase()).toContain(
      'update this app'
    );
  });
});
