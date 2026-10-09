// Redesign step 3.10: the session row's ⋯ menu and right-click offer every
// action Details offers, disabled for the same reason, and run through
// Details itself (its confirm, its dialog).
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import SessionDetails from './SessionDetails.svelte';
import SessionRowItem from './SessionRowItem.svelte';
import { hosts } from './hosts';
import { session } from './hosts_fixture';
import { sessions, type SessionRow } from './sessions';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';
import { resetAccessForTests, setMyGrants } from './access';
import { clearSelection, selectedSession } from './selection';
import { ROW_ACTIONS, requestSessionAction, sessionActionRequest, sessionMenuItems } from './session_actions';

const noop = () => {};
function rowProps(sess: SessionRow) {
  return {
    sess,
    selectMode: false,
    isChecked: false,
    isRenaming: false,
    renameValue: '',
    renameInput: undefined,
    renameError: null,
    relatedCount: 0,
    nowSec: Math.floor(Date.now() / 1000),
    onSelectSession: noop,
    onKeySession: noop,
    toggleSelected: noop,
    beginRename: noop,
    beginLabelEdit: noop,
    onRenameKey: noop,
    commitRename: noop,
    askRecreate: noop,
    askRestart: noop,
    askKill: noop,
  };
}

const remote: HubStatus = {
  ...STANDALONE,
  remote: true,
  url: 'https://fleet.example.com',
  client_name: 'laptop',
  configured_url: 'https://fleet.example.com',
  configured_client_name: 'laptop',
};

const ME = 7;
const OTHER = 9;
const work = session('mefistos', 'dev-foo', { id: 1, claude_session_id: 'c-1', project_id: 3, worktree_id: 4 });
const shell = session('mefistos', 'sh', { id: 2, kind: 'shell' });
const external = session('mefistos', 'ext', { id: 3, kind: 'external' });
const theirs = session('mefistos', 'theirs', { id: 4, owner_person_id: OTHER, visibility: 'private' as const });

beforeEach(() => {
  hosts.set([]);
  sessions.set([work, shell, external, theirs]);
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  resetAccessForTests();
  sessionActionRequest.set(null);
  clearSelection();
});

/** Details' own action buttons, as the menu's items would name them. */
function detailsActions(): Map<string, { disabled: boolean; title: string }> {
  const out = new Map<string, { disabled: boolean; title: string }>();
  for (const a of ROW_ACTIONS) {
    const b = screen.queryByTestId(a.detailsTestId) as HTMLButtonElement | null;
    if (b) out.set(a.id, { disabled: b.disabled, title: b.disabled ? b.title : '' });
  }
  return out;
}

describe('row menu parity with Details', () => {
  const cases: [string, SessionRow, () => void][] = [
    ['a work session, standalone', work, noop],
    ['a shell', shell, noop],
    ['an external row', external, noop],
    ['a hub client whose link is down', work, () => {
      hubStatus.set(remote);
      hubConnection.set({ state: 'offline', attempt: 2, retry_in_secs: 5, reason: 'refused' });
    }],
    ['a session watched through a grant', theirs, () => {
      hubStatus.set(remote);
      hubConnection.set({ state: 'connected' } as never);
      setMyGrants(ME, [{ session_id: theirs.id, level: 'watch' }]);
    }],
  ];

  it.each(cases)('%s: the same actions, disabled for the same reason', async (_name, s, setup) => {
    setup();
    render(SessionDetails, { props: { session: s } });
    await tick();
    const fromDetails = detailsActions();
    const fromMenu = new Map(
      get(sessionMenuItems)(s).map((i) => [i.id, { disabled: i.blocked !== null, title: i.blocked ?? '' }]),
    );
    expect(fromMenu).toEqual(fromDetails);
    expect(fromMenu.size).toBeGreaterThan(0);
  });
});

describe('the row menu', () => {
  it('opens from ⋯ and from a right-click', async () => {
    const { container } = render(SessionRowItem, { props: rowProps(work) });
    const row = container.querySelector('[data-testid="sess-row"]')!;
    await fireEvent.click(screen.getByTestId('row-menu-open'));
    expect(screen.getByTestId('session-row-menu')).toBeTruthy();
    await fireEvent.keyDown(screen.getByTestId('session-row-menu'), { key: 'Escape' });
    expect(screen.queryByTestId('session-row-menu')).toBeNull();
    await fireEvent.contextMenu(row);
    expect(screen.getByTestId('session-row-menu')).toBeTruthy();
    expect(screen.getByTestId('row-menu-kill')).toBeTruthy();
    expect(screen.getByTestId('row-menu-details')).toBeTruthy();
  });

  it('an item opens the session and Details runs the action through its own confirm', async () => {
    render(SessionRowItem, { props: rowProps(work) });
    await fireEvent.click(screen.getByTestId('row-menu-open'));
    await fireEvent.click(screen.getByTestId('row-menu-kill'));
    expect(screen.queryByTestId('session-row-menu')).toBeNull();
    expect(get(selectedSession)?.id).toBe(work.id);
    expect(get(sessionActionRequest)?.action).toBe('kill');

    render(SessionDetails, { props: { session: work } });
    await waitFor(() => expect(get(sessionActionRequest)).toBeNull());
    // Details' own kill confirm, not a kill.
    expect(await screen.findByRole('dialog')).toBeTruthy();
  });

  it('a request for another session waits for its Details', async () => {
    requestSessionAction(shell, 'rename');
    render(SessionDetails, { props: { session: work } });
    await tick();
    expect(get(sessionActionRequest)?.sessionId).toBe(shell.id);
  });

  it('a blocked action is disabled with Details\' reason and never runs', async () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'offline', attempt: 2, retry_in_secs: 5, reason: 'refused' });
    render(SessionRowItem, { props: rowProps(work) });
    await fireEvent.click(screen.getByTestId('row-menu-open'));
    const kill = screen.getByTestId('row-menu-kill') as HTMLButtonElement;
    expect(kill.disabled).toBe(true);
    expect(kill.title).not.toBe('');
  });
});
