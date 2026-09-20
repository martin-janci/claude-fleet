import { render, screen, waitFor, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import App from './App.svelte';
import { onboardingDismissed } from './lib/onboarding';
import { clearToasts } from './lib/toasts';
import { hubStatus, STANDALONE, type HubStatus } from './lib/hub';
import { hubConnection } from './lib/hub_connection';

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
  unavailable: null,
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
  // Module-global like `hubStatus`: without this the banner a previous test
  // left up is still up, and the next one asserts against its text.
  hubConnection.set({ state: 'standalone' });
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
        'http://10.0.0.5:8787 is plain http to a host that is not loopback, so this app’s client token would cross the network in the clear on every call — allowed by hub.client_plaintext_token, so this is deliberate',
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

// SF-8: the banner. Only a hub client has a live link to lose.
describe('the disconnected banner', () => {
  it('a hub client asks for its connection state and shows the banner while down', async () => {
    const { inv, restore } = await routeInvoke((cmd) =>
      cmd === 'hub_status'
        ? remote
        : cmd === 'hub_connection'
          ? { state: 'offline', attempt: 1, retry_in_secs: 1, reason: 'connection refused' }
          : undefined,
    );
    try {
      render(App);
      const banner = await screen.findByTestId('hub-connection-banner');
      expect(banner.textContent).toContain('connection refused');
      expect(inv.mock.calls.some((c) => c[0] === 'hub_connection')).toBe(true);
    } finally {
      restore();
    }
  });

  // The backend refuses every routed call while the hub's wire contract is
  // outside this build's range (`E_HUB_CONTRACT`,
  // `src-tauri/src/backend/remote.rs`), rather than deserialising its rows
  // with silently defaulted fields. A read refused that way must not read as
  // "this fleet has nothing in it": the banner says what is wrong, and the
  // startup failure says which load did not happen and why.
  it('a read refused for contract skew shows the reason, not an empty fleet', async () => {
    const skew = { state: 'hub_too_old', hub_contract: 1, min_contract: 3 };
    const { restore } = await routeInvoke((cmd) => {
      if (cmd === 'hub_status') return remote;
      if (cmd === 'hub_connection') return skew;
      if (cmd === 'list_sessions') {
        return Object.assign(new Error('contract'), {
          code: 'E_HUB_CONTRACT',
          message:
            'list_sessions was not run: https://fleet.example.com’s wire contract is ' +
            'revision 1, older than the 3 this app requires. Update the hub.',
        });
      }
      return undefined;
    });
    try {
      render(App);
      const banner = await screen.findByTestId('hub-connection-banner');
      expect(banner.textContent).toContain('Update the hub.');
      const failed = await screen.findByTestId('bootstrap-error');
      expect(failed.textContent).toContain('sessions: E_HUB_CONTRACT');
    } finally {
      restore();
    }
  });

  it('a standalone app neither asks nor shows one', async () => {
    const { inv, restore } = await routeInvoke((cmd) =>
      cmd === 'hub_status' ? STANDALONE : undefined,
    );
    try {
      render(App);
      await waitFor(() => expect(screen.getByText(/schema/)).toBeInTheDocument());
      expect(inv.mock.calls.some((c) => c[0] === 'hub_connection')).toBe(false);
      expect(screen.queryByTestId('hub-connection-banner')).toBeNull();
    } finally {
      restore();
    }
  });

  // #195/#166: a skewed hub refuses every bootstrap load (projects, sessions,
  // hosts, accounts) with the same `E_HUB_CONTRACT`. Each used to toast on
  // its own — a wall of four near-identical sticky error toasts stacked on
  // top of the banner and the footer, which already say the same thing.
  it('does not toast a storm of identical E_HUB_CONTRACT failures at bootstrap', async () => {
    const skew = { state: 'hub_too_old', hub_contract: 1, min_contract: 3 };
    const contractError = (cmd: string) =>
      Object.assign(new Error('contract'), {
        code: 'E_HUB_CONTRACT',
        message: `${cmd} was not run: https://fleet.example.com’s wire contract is revision 1, older than the 3 this app requires. Update the hub.`,
      });
    const { restore } = await routeInvoke((cmd) => {
      if (cmd === 'hub_status') return remote;
      if (cmd === 'hub_connection') return skew;
      if (['list_sessions', 'list_projects', 'list_hosts', 'list_accounts'].includes(cmd)) {
        return contractError(cmd);
      }
      return undefined;
    });
    try {
      render(App);
      const failed = await screen.findByTestId('bootstrap-error');
      expect(failed.textContent).toContain('E_HUB_CONTRACT');
      // The banner and the footer already say it; no toast repeats it.
      expect(screen.queryByTestId('toast')).toBeNull();
    } finally {
      restore();
    }
  });
});

// #195: the focus-driven catch-up fetch (see App.test.ts for the standalone
// case) discards its Result like every other failure on that path — but a
// contract-skewed hub never heals, so it is worth pinning that this stays
// silent rather than toasting on every alt-tab back into the window.
describe('the focus-driven refresh under a contract skew', () => {
  it('raises no toast when the refresh is refused with E_HUB_CONTRACT', async () => {
    let sessionsCalls = 0;
    const { restore } = await routeInvoke((cmd) => {
      if (cmd === 'hub_status') return remote;
      if (cmd === 'hub_connection') return { state: 'connected' };
      if (cmd === 'list_sessions') {
        sessionsCalls += 1;
        // Bootstrap succeeds; the focus-driven refetch is what fails.
        if (sessionsCalls === 1) return [];
        return Object.assign(new Error('contract'), {
          code: 'E_HUB_CONTRACT',
          message: 'list_sessions was not run: contract mismatch. Update the hub.',
        });
      }
      if (cmd === 'list_projects') return [];
      return undefined;
    });
    try {
      render(App);
      await screen.findByTestId('hub-badge');
      // Let the post-subscription tail of onMount settle before clearing.
      await new Promise((r) => setTimeout(r, 0));
      clearToasts();
      await fireEvent(window, new FocusEvent('focus'));
      await new Promise((r) => setTimeout(r, 0));
      expect(screen.queryByTestId('toast')).toBeNull();
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

// F1 of the final review. A hub is configured (`hub.remote_url` is set) but
// this launch could not use it: no stored token, a keychain that would not
// open, plain http without the opt-in, a URL that does not parse. The backend
// now owns NOTHING in that state — no reconcile tick, no usage poll, no
// control API — so the window must say why, where it cannot be missed, and
// lead to the place it can be fixed.
describe('a configured hub this launch cannot use', () => {
  const unavailable: HubStatus = {
    ...STANDALONE,
    configured_url: 'https://fleet.example.com',
    warning:
      'cannot read the client token for https://fleet.example.com (the keychain is locked)',
    unavailable:
      'cannot read the client token for https://fleet.example.com (the keychain is locked)',
  };

  it('says so at the top of the window, with the reason and the way to Settings', async () => {
    const { restore } = await routeInvoke((cmd) => (cmd === 'hub_status' ? unavailable : undefined));
    try {
      render(App);
      const banner = await screen.findByTestId('hub-unavailable');
      expect(banner.getAttribute('role')).toBe('alert');
      expect(banner.textContent).toContain('fleet.example.com');
      expect(banner.textContent).toContain('the keychain is locked');
      // It must not read as a working standalone app: it runs no fleet.
      expect(banner.textContent!.toLowerCase()).toContain('not managing');
      expect(screen.getByTestId('hub-unavailable-settings')).toBeInTheDocument();
    } finally {
      restore();
    }
  });

  // Every fleet command is refused in this state, so asking would put a
  // wall of identical error toasts under the banner that already explains
  // them.
  it('does not ask the refused fleet commands for anything', async () => {
    const { inv, restore } = await routeInvoke((cmd) =>
      cmd === 'hub_status' ? unavailable : undefined,
    );
    try {
      render(App);
      await screen.findByTestId('hub-unavailable');
      await new Promise((r) => setTimeout(r, 0));
      const asked = inv.mock.calls.map((c) => c[0] as string);
      for (const cmd of ['health_check', 'list_account_usage', 'hub_connection', 'list_tasks']) {
        expect(asked, cmd).not.toContain(cmd);
      }
    } finally {
      restore();
    }
  });

  it('is absent in standalone and in a working hub client', async () => {
    for (const status of [STANDALONE, remote]) {
      const { restore } = await routeInvoke((cmd) => (cmd === 'hub_status' ? status : undefined));
      try {
        const { unmount } = render(App);
        await waitFor(() => expect(screen.getByText(/schema|connecting/)).toBeInTheDocument());
        expect(screen.queryByTestId('hub-unavailable')).toBeNull();
        unmount();
      } finally {
        restore();
      }
    }
  });
});
