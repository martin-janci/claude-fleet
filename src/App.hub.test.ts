import { render, screen, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import App from './App.svelte';
import { onboardingDismissed } from './lib/onboarding';
import { clearToasts } from './lib/toasts';
import { hubStatus, STANDALONE, type HubStatus } from './lib/hub';

const remote: HubStatus = {
  remote: true,
  url: 'https://fleet.example.com',
  client_name: 'laptop',
  client_mode: null,
  configured_url: 'https://fleet.example.com',
  configured_client_name: 'laptop',
  allow_plaintext: false,
  warning: null,
  restart_required: false,
};

/** Route `invoke` by command on top of the global setup mock. */
async function routeInvoke(
  answers: (cmd: string) => unknown | undefined,
): Promise<{ inv: ReturnType<typeof vi.fn>; restore: () => void }> {
  const { invoke } = await import('@tauri-apps/api/core');
  const inv = invoke as ReturnType<typeof vi.fn>;
  const original = inv.getMockImplementation() as (
    cmd: string,
    ...rest: unknown[]
  ) => Promise<unknown>;
  // The invoke mock is global and shared, so the call log carries over from
  // whatever rendered before. Every assertion here is about what THIS render
  // did.
  inv.mockClear();
  inv.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
    const answer = answers(cmd);
    if (answer !== undefined) {
      if (answer instanceof Error) throw answer;
      return answer;
    }
    return original(cmd, ...rest);
  });
  return { inv, restore: () => inv.mockImplementation(original) };
}

beforeEach(() => {
  onboardingDismissed.set(true);
  clearToasts();
  hubStatus.set({ ...STANDALONE });
});

describe('the hub badge', () => {
  it('is absent in standalone mode, so nothing about today’s app changes', async () => {
    const { restore } = await routeInvoke((cmd) => (cmd === 'hub_status' ? STANDALONE : undefined));
    try {
      render(App);
      await waitFor(() => expect(screen.getByText(/schema/)).toBeInTheDocument());
      expect(screen.queryByTestId('hub-badge')).toBeNull();
    } finally {
      restore();
    }
  });

  it('names the hub this window is onto', async () => {
    const { restore } = await routeInvoke((cmd) => (cmd === 'hub_status' ? remote : undefined));
    try {
      render(App);
      const badge = await screen.findByTestId('hub-badge');
      expect(badge.textContent).toContain('fleet.example.com');
      expect(badge.textContent).toContain('laptop');
    } finally {
      restore();
    }
  });

  // Requirement (d)(ii): the plaintext decision is named where the hub is
  // named, persistently, not once in a log line at launch.
  it('shows the plaintext warning beside the badge, every launch', async () => {
    const warned = {
      ...remote,
      url: 'http://10.0.0.5:8787',
      allow_plaintext: true,
      warning:
        'http://10.0.0.5:8787 is plain http to a host that is not loopback, so this app’s client token would cross the network in the clear on every call — allowed by hub.allow_plaintext, so this is deliberate',
    };
    const { restore } = await routeInvoke((cmd) => (cmd === 'hub_status' ? warned : undefined));
    try {
      render(App);
      const warning = await screen.findByTestId('hub-warning');
      expect(warning.textContent).toContain('in the clear');
    } finally {
      restore();
    }
  });
});

describe('health in remote mode', () => {
  // Requirement (b). health_check used to answer from the LOCAL database in
  // remote mode, so the footer read a perfectly zeroed fleet. Now it is the
  // hub's answer — and when the hub cannot be reached the footer says so
  // instead of showing a reassuring nothing.
  it('shows the hub’s failure rather than a healthy-looking footer', async () => {
    const { restore } = await routeInvoke((cmd) => {
      if (cmd === 'hub_status') return remote;
      if (cmd === 'health_check') {
        return Object.assign(new Error('unreachable'), {
          code: 'E_HUB_UNREACHABLE',
          message: 'https://fleet.example.com did not answer: connection refused',
        });
      }
      return undefined;
    });
    try {
      render(App);
      const err = await screen.findByTestId('health-error');
      expect(err.textContent).toContain('E_HUB_UNREACHABLE');
      expect(err.textContent).toContain('connection refused');
    } finally {
      restore();
    }
  });

  it('still shows version, db and schema when the hub answers', async () => {
    const { restore } = await routeInvoke((cmd) => {
      if (cmd === 'hub_status') return remote;
      if (cmd === 'health_check') {
        return { version: '9.9.9', db_ready: true, schema_version: 41 };
      }
      return undefined;
    });
    try {
      render(App);
      await waitFor(() => expect(screen.getByText(/v9\.9\.9/)).toBeInTheDocument());
    } finally {
      restore();
    }
  });
});

describe('the commands the UI calls unprompted', () => {
  // Requirement (a). `list_account_usage` is guarded on the backend now, so
  // calling it in remote mode returns E_LOCAL_ONLY — an error toast on every
  // launch, for a panel that simply does not apply. Don't call it.
  it('does not poll account usage against a hub that owns the fleet', async () => {
    const { inv, restore } = await routeInvoke((cmd) =>
      cmd === 'hub_status' ? remote : undefined,
    );
    try {
      render(App);
      await screen.findByTestId('hub-badge');
      // Give the post-subscription tail of onMount a chance to run.
      await new Promise((r) => setTimeout(r, 0));
      expect(inv.mock.calls.some((c) => c[0] === 'list_account_usage')).toBe(false);
    } finally {
      restore();
    }
  });

  it('still polls account usage in standalone mode', async () => {
    const { inv, restore } = await routeInvoke((cmd) =>
      cmd === 'hub_status' ? STANDALONE : undefined,
    );
    try {
      render(App);
      await waitFor(() =>
        expect(inv.mock.calls.some((c) => c[0] === 'list_account_usage')).toBe(true),
      );
    } finally {
      restore();
    }
  });

  // The mode governs what the rest of the launch does, so it has to be known
  // before that launch happens — not a render later.
  it('asks for the hub status before it loads anything else', async () => {
    const { inv, restore } = await routeInvoke((cmd) =>
      cmd === 'hub_status' ? remote : undefined,
    );
    try {
      render(App);
      await waitFor(() =>
        expect(inv.mock.calls.some((c) => c[0] === 'list_sessions')).toBe(true),
      );
      const names = inv.mock.calls.map((c) => c[0] as string);
      // Child components mount (and fetch) before the parent's onMount runs,
      // so `hub_status` is not necessarily call zero. What matters is that it
      // precedes everything App's OWN bootstrap does, because that bootstrap
      // branches on the answer.
      expect(names).toContain('hub_status');
      expect(names.indexOf('hub_status')).toBeLessThan(names.indexOf('health_check'));
      expect(names.indexOf('hub_status')).toBeLessThan(names.lastIndexOf('list_sessions'));
    } finally {
      restore();
    }
  });
});
