// Gap plan G4.2: asking for a wider share level, and the owner's answer.
// The pure wording (shared_view.ts), the recipient's header
// (SharedWithYou.svelte), the owner's Share sheet section, the badge count,
// and the toast an incoming ask raises.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import SharedWithYou from './SharedWithYou.svelte';
import ShareSheet from './ShareSheet.svelte';
import { accessRequests, onGrantFrames, type AccessRequest } from './access_requests';
import {
  askedLabel,
  askLabel,
  nextLevel,
  readOnlyAnswerLine,
  recipientStateLabel,
  sharedByMeta,
  sharedRowLine,
  sharedWithYouChip,
} from './shared_view';
import { myAccessRequests, myGrantInfo, resetAccessForTests, setMyGrants } from './access';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';
import { sessions } from './sessions';
import { shareSheetFor, visibilityBadge } from './share';
import { toasts, clearToasts } from './toasts';
import { session } from './hosts_fixture';

const mockedInvoke = invoke as unknown as ReturnType<typeof vi.fn>;
const REMOTE: HubStatus = { ...STANDALONE, remote: true, url: 'https://hub.example' };
const ROW = session('mac', 'fix-flake', { id: 42, visibility: 'private', owner_person_id: 9 });

async function settle() {
  for (let i = 0; i < 4; i++) {
    await tick();
    await Promise.resolve();
  }
}

const ask = (over: Partial<AccessRequest> = {}): AccessRequest => ({
  id: 3,
  session_id: 42,
  person_id: 2,
  person_name: 'bea',
  person_display_name: 'Bea',
  level: 'answer',
  requested_at: 1,
  ...over,
});

describe('shared_view wording (Watch board)', () => {
  const info = { sharedBy: 9, sharedByName: 'Martin Janči', grantedAt: null, viaOrg: null };
  it('names the levels Read / Answer / Steer and the next one up', () => {
    expect(sharedWithYouChip('watch')).toBe('Shared with you · Read');
    expect(nextLevel('watch')).toBe('answer');
    expect(nextLevel('answer')).toBe('drive');
    expect(nextLevel('drive')).toBeNull();
    expect(askLabel('Martin', 'answer')).toBe('Ask Martin for Answer');
    expect(askedLabel('Martin', 'answer')).toBe('Asked for Answer · waiting for Martin');
  });
  it('says the session waits on its owner', () => {
    expect(recipientStateLabel('action_required', 'Martin')).toBe('Waiting for Martin');
    expect(recipientStateLabel('working', 'Martin')).toBeNull();
  });
  it('writes the meta lead and the Inbox row', () => {
    expect(sharedByMeta('watch', info, 'Martin Janči')).toBe('Shared by Martin Janči · level Read');
    const at = new Date(2026, 9, 10, 13, 20).getTime() / 1000;
    expect(sharedByMeta('watch', { ...info, grantedAt: at }, 'Martin', new Date(2026, 9, 10, 15, 0))).toBe(
      'Shared by Martin · level Read · since 13:20',
    );
    expect(sharedRowLine('watch', info, 'Martin', 'action_required')).toBe('Martin · Read · waiting for Martin');
    expect(sharedRowLine('drive', { ...info, viaOrg: '32bit' }, 'Martin', 'working')).toBe('Martin · via 32bit · Steer');
    expect(readOnlyAnswerLine('Martin')).toBe(
      'You can read this session. The question above is Martin’s to answer; with Answer you could reply here.',
    );
  });
});

describe('the recipient header', () => {
  beforeEach(() => {
    resetAccessForTests();
    hubStatus.set(REMOTE);
    hubConnection.set({ state: 'connected' });
    mockedInvoke.mockClear();
  });
  afterEach(() => hubStatus.set({ ...STANDALONE }));

  it('says the level, asks the owner for the next one, then says it asked', async () => {
    setMyGrants(7, [{ session_id: 42, level: 'watch', shared_by: 9, shared_by_name: 'Martin' }]);
    mockedInvoke.mockImplementationOnce(async () => ask({ id: 5, person_id: 7 }));
    render(SharedWithYou, { session: ROW });
    expect(screen.getByTestId('shared-with-you').textContent).toBe('Shared with you · Read');
    const btn = screen.getByTestId('shared-ask');
    expect(btn.textContent).toBe('Ask Martin for Answer');
    await fireEvent.click(btn);
    await settle();
    expect(mockedInvoke).toHaveBeenCalledWith('session_ask_access', { args: { session_id: 42, level: 'answer' } });
    expect(screen.getByTestId('shared-asked').textContent).toBe('Asked for Answer · waiting for Martin');
    expect(get(myAccessRequests).get(42)?.level).toBe('answer');
  });

  it('shows an open ask from my_grants, and nothing to ask at Steer', async () => {
    setMyGrants(7, [{ session_id: 42, level: 'answer' }], [{ id: 1, session_id: 42, level: 'drive', requested_at: 1 }]);
    const { unmount } = render(SharedWithYou, { session: ROW });
    expect(screen.getByTestId('shared-asked').textContent).toContain('Asked for Steer');
    unmount();
    setMyGrants(7, [{ session_id: 42, level: 'drive' }]);
    render(SharedWithYou, { session: ROW });
    expect(screen.getByTestId('shared-with-you').textContent).toBe('Shared with you · Steer');
    expect(screen.queryByTestId('shared-ask')).toBeNull();
  });

  it('shows nothing on the owner’s own row', () => {
    setMyGrants(9, []);
    render(SharedWithYou, { session: ROW });
    expect(screen.queryByTestId('shared-with-you')).toBeNull();
  });

  it('keeps who shared it from my_grants', () => {
    setMyGrants(7, [{ session_id: 42, level: 'watch', shared_by: 9, shared_by_name: 'Martin', granted_at: 5, via_org: '32bit' }]);
    expect(get(myGrantInfo).get(42)).toEqual({ sharedBy: 9, sharedByName: 'Martin', grantedAt: 5, viaOrg: '32bit' });
  });
});

describe('the owner’s side', () => {
  beforeEach(() => {
    resetAccessForTests();
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
    sessions.set([{ ...ROW, owner_person_id: 1 }]);
    accessRequests.set([]);
    clearToasts();
    mockedInvoke.mockClear();
  });

  it('the Share sheet lists an ask and grants it', async () => {
    mockedInvoke.mockImplementation(async (cmd: string, a?: { args?: { action?: string } }) => {
      if (cmd === 'access_requests') return a?.args?.action === 'list' ? [ask()] : ask({ resolution: 'granted' });
      if (cmd === 'session_access') return [];
      return null;
    });
    shareSheetFor.set(42);
    render(ShareSheet);
    await settle();
    const row = screen.getByTestId('share-ask');
    expect(row.textContent).toContain('Bea');
    expect(row.textContent).toContain('asks for Answer');
    await fireEvent.click(screen.getByTestId('share-ask-grant'));
    await settle();
    expect(mockedInvoke).toHaveBeenCalledWith('access_requests', { args: { action: 'grant', id: 3 } });
    shareSheetFor.set(null);
    mockedInvoke.mockReset();
  });

  it('a decline is sent as a decline', async () => {
    mockedInvoke.mockImplementation(async (cmd: string, a?: { args?: { action?: string } }) => {
      if (cmd === 'access_requests') return a?.args?.action === 'list' ? [ask()] : ask({ resolution: 'declined' });
      if (cmd === 'session_access') return [];
      return null;
    });
    shareSheetFor.set(42);
    render(ShareSheet);
    await settle();
    await fireEvent.click(screen.getByTestId('share-ask-decline'));
    await settle();
    expect(mockedInvoke).toHaveBeenCalledWith('access_requests', { args: { action: 'decline', id: 3 } });
    shareSheetFor.set(null);
    mockedInvoke.mockReset();
  });

  it('the badge counts open asks', () => {
    const b = visibilityBadge({ visibility: 'private' }, 'own', [], 1);
    expect(b?.text).toBe('Private · 1 ask');
  });

  it('an incoming ask re-reads the list and says who asks, with Review opening the sheet', async () => {
    vi.useFakeTimers();
    try {
      mockedInvoke.mockImplementation(async (cmd: string) => (cmd === 'access_requests' ? [ask()] : null));
      setMyGrants(1, []);
      onGrantFrames([{ session_id: 42, person_id: 2, level: 'watch', request: 'answer' }]);
      await vi.advanceTimersByTimeAsync(400);
      expect(get(accessRequests)).toHaveLength(1);
      const t = get(toasts).find((x) => x.message.includes('asks for'));
      expect(t?.message).toBe('Bea asks for Answer on fix-flake');
      t?.action?.run();
      expect(get(shareSheetFor)).toBe(42);
    } finally {
      vi.useRealTimers();
      shareSheetFor.set(null);
      mockedInvoke.mockReset();
    }
  });
});
