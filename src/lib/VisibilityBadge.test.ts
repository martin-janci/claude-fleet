// Gap plan G4.3: the session header's visibility badge — Private or
// Shared · N for the owner, read from the live grant list, opening the one
// Share sheet; Unclaimed for a found row; nothing on someone else's row.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import VisibilityBadge from './VisibilityBadge.svelte';
import { session } from './hosts_fixture';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';
import { resetAccessForTests, setMyGrants } from './access';
import { shareSheetFor, visibilityBadge } from './share';
import type { SessionGrant } from './sessions';

const mine = session('mac', 'fix-flake', { id: 1, visibility: 'private' as const });
const grant = (over: Partial<SessionGrant>): SessionGrant => ({ session_id: 1, person_id: 5, level: 'watch', ...over });

let grants: SessionGrant[] = [];
beforeEach(() => {
  grants = [];
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (cmd: string) => (cmd === 'session_access' ? grants : null));
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  resetAccessForTests();
  shareSheetFor.set(null);
});

describe('visibilityBadge', () => {
  it('says nothing until the grant list is read, then Private or Shared · N', () => {
    expect(visibilityBadge(mine, 'own', null)).toBeNull();
    expect(visibilityBadge(mine, 'own', [])?.text).toBe('Private');
    const b = visibilityBadge(mine, 'own', [
      grant({ person_display_name: 'Peter', level: 'drive' }),
      grant({ person_id: null, org_id: 2, org_name: '32bit', level: 'watch' }),
    ]);
    expect(b?.text).toBe('Shared · 2');
    expect(b?.title).toBe('Shared with Peter (can steer), 32bit (org) (can watch)');
  });

  it('is Unclaimed for a found row, and nothing for a recipient or an old hub', () => {
    expect(visibilityBadge({ visibility: 'unclaimed' }, null, null)?.text).toBe('Unclaimed');
    expect(visibilityBadge(mine, 'watch', [])).toBeNull();
    expect(visibilityBadge({ visibility: undefined }, 'own', [])).toBeNull();
  });
});

describe('VisibilityBadge', () => {
  it('shows Private for an unshared session and opens the Share sheet', async () => {
    render(VisibilityBadge, { props: { session: mine } });
    const badge = await screen.findByTestId('session-visibility');
    expect(badge.textContent).toBe('Private');
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('session_access', { args: { session_id: 1 } });
    await fireEvent.click(badge);
    expect(get(shareSheetFor)).toBe(1);
  });

  it('reads the grants again when the Share sheet closes', async () => {
    render(VisibilityBadge, { props: { session: mine } });
    expect((await screen.findByTestId('session-visibility')).textContent).toBe('Private');
    shareSheetFor.set(1);
    await tick();
    grants = [grant({ person_name: 'peter' })];
    shareSheetFor.set(null);
    await waitFor(() => expect(screen.getByTestId('session-visibility').textContent).toBe('Shared · 1'));
    expect(screen.getByTestId('session-visibility').dataset.kind).toBe('shared');
  });

  it('shows nothing on a session shared with this person', async () => {
    const remote: HubStatus = { ...STANDALONE, remote: true, url: 'https://hub', client_name: 'laptop' } as HubStatus;
    hubStatus.set(remote);
    hubConnection.set({ state: 'connected' } as never);
    const theirs = session('mac', 'theirs', { id: 4, owner_person_id: 9, visibility: 'private' as const });
    setMyGrants(7, [{ session_id: 4, level: 'watch' }]);
    render(VisibilityBadge, { props: { session: theirs } });
    await tick();
    expect(screen.queryByTestId('session-visibility')).toBeNull();
    expect(vi.mocked(invoke).mock.calls.some((c) => c[0] === 'session_access')).toBe(false);
  });

  it('an unclaimed row says so and is not a Share button', async () => {
    const found = session('mac', 'found', { id: 6, visibility: 'unclaimed' as const });
    render(VisibilityBadge, { props: { session: found } });
    const badge = await screen.findByTestId('session-visibility');
    expect(badge.textContent).toBe('Unclaimed');
    expect(badge.tagName).toBe('SPAN');
  });
});
