// The row's work menu (roadmap M1b.2): set a key, "Not this", clear a link.
// Asserts the command each action invokes and its arguments.
import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import SessionRowItem from './SessionRowItem.svelte';
import { hosts } from './hosts';
import { session } from './hosts_fixture';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import { applyGrantChanges, resetAccessForTests, setMyGrants } from './access';
import type { SessionRow } from './sessions';
import type { WorkKey } from './work_keys';

const noop = () => {};

function props(sess: SessionRow, workKey: WorkKey | null = null, extra: Record<string, unknown> = {}) {
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
    workKey,
    onSelectSession: vi.fn(),
    onKeySession: noop,
    toggleSelected: noop,
    beginRename: noop,
    beginLabelEdit: noop,
    onRenameKey: noop,
    commitRename: noop,
    askRecreate: noop,
    askRestart: noop,
    askKill: noop,
    ...extra,
  };
}

const live = (over: Partial<SessionRow> = {}): SessionRow =>
  session('mefistos', 'dev-foo', { id: 7, status: 'running', ...over });

beforeEach(() => {
  hosts.set([]);
  vi.mocked(invoke).mockReset();
  // Every decision answers the updated row.
  vi.mocked(invoke).mockImplementation(async () => live());
});

async function openMenu() {
  await fireEvent.click(screen.getByTestId('work-menu'));
  await tick();
  return screen.getByTestId('work-menu-panel');
}

describe('SessionRowItem work menu', () => {
  it('sets work on a row without one, and does not select the row', async () => {
    const p = props(live());
    render(SessionRowItem, { props: p });
    await openMenu();
    expect(screen.queryByTestId('work-reject')).toBeNull();
    expect(screen.queryByTestId('work-unlink')).toBeNull();
    const input = screen.getByTestId('work-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: ' ABC-123 ' } });
    await fireEvent.click(screen.getByTestId('work-set'));
    await tick();
    expect(invoke).toHaveBeenCalledWith('link_session_work', {
      args: { session_id: 7, key: 'ABC-123' },
    });
    expect(p.onSelectSession).not.toHaveBeenCalled();
    await tick();
    expect(screen.queryByTestId('work-menu-panel')).toBeNull();
  });

  it('Enter in the input sets the work', async () => {
    render(SessionRowItem, { props: props(live()) });
    await openMenu();
    const input = screen.getByTestId('work-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'billing migration' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(invoke).toHaveBeenCalledWith('link_session_work', {
      args: { session_id: 7, key: 'billing migration' },
    });
  });

  it('"Not this" on a recognised key rejects that key', async () => {
    const key: WorkKey = { key: 'ABC-123', source: 'branch', from: 'abc-123-login' };
    render(SessionRowItem, { props: props(live(), key) });
    await openMenu();
    // A recognised key is not a link: nothing to clear.
    expect(screen.queryByTestId('work-unlink')).toBeNull();
    await fireEvent.click(screen.getByTestId('work-reject'));
    expect(invoke).toHaveBeenCalledWith('reject_session_work', {
      args: { session_id: 7, key: 'ABC-123' },
    });
  });

  it('a linked item offers Clear (unlink) and rejects by item id', async () => {
    const sess = live({
      work: { link_id: 11, item_id: 4, key: 'OPS-9', title: 'Rotate', source: 'manual' },
    });
    const key: WorkKey = { key: 'OPS-9', source: 'link', from: 'Rotate' };
    render(SessionRowItem, { props: props(sess, key) });
    await openMenu();
    await fireEvent.click(screen.getByTestId('work-unlink'));
    expect(invoke).toHaveBeenCalledWith('unlink_session_work', {
      args: { session_id: 7, link_id: 11 },
    });
    await openMenu();
    await fireEvent.click(screen.getByTestId('work-reject'));
    expect(invoke).toHaveBeenLastCalledWith('reject_session_work', {
      args: { session_id: 7, item_id: 4 },
    });
  });

  it('acts on workOf inside a work group, where the chip is hidden', async () => {
    const key: WorkKey = { key: 'ABC-1', source: 'tag', from: 'ABC-1' };
    render(SessionRowItem, { props: props(live(), null, { workOf: key }) });
    expect(screen.queryByTestId('work-chip')).toBeNull();
    await openMenu();
    await fireEvent.click(screen.getByTestId('work-reject'));
    expect(invoke).toHaveBeenCalledWith('reject_session_work', {
      args: { session_id: 7, key: 'ABC-1' },
    });
  });
  it('Escape in the key box closes the menu without setting anything, and is not the row’s key', async () => {
    const onKeySession = vi.fn();
    render(SessionRowItem, { props: props(live(), null, { onKeySession }) });
    await openMenu();
    const input = screen.getByTestId('work-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'ABC-1' } });
    await fireEvent.keyDown(input, { key: 'Escape' });
    await tick();
    expect(screen.queryByTestId('work-menu-panel')).toBeNull();
    expect(invoke).not.toHaveBeenCalledWith('link_session_work', expect.anything());
    expect(onKeySession).not.toHaveBeenCalled();
    // Reopened, the box starts empty: the abandoned draft is gone.
    await openMenu();
    expect((screen.getByTestId('work-input') as HTMLInputElement).value).toBe('');
    // Other keys in the box are the box's own, never the row's.
    await fireEvent.keyDown(screen.getByTestId('work-input'), { key: 'j' });
    expect(onKeySession).not.toHaveBeenCalled();
    // The # button is a toggle: a second click closes it too.
    await fireEvent.click(screen.getByTestId('work-menu'));
    await tick();
    expect(screen.queryByTestId('work-menu-panel')).toBeNull();
  });
});

// Multi-user M1, F2b: the menu's ENTRANCE was gated and nothing inside it was.
// The gate is now `workAction`, the runner every write in the popover goes
// through, so a `drive` grant narrowed to `watch` while the popover is open
// lands nothing — and `y`/`n` on the row, which reach these paths with no
// button in between, are covered by the same answer.
describe('SessionRowItem work menu: the access gate (multi-user M1)', () => {
  const REMOTE = {
    ...STANDALONE,
    remote: true,
    url: 'https://fleet.example.com',
    configured_url: 'https://fleet.example.com',
  };
  const ME = 7;
  const THEM = 9;

  beforeEach(() => {
    resetAccessForTests();
    hubStatus.set(REMOTE);
    hubConnection.set({ state: 'connected' });
  });

  afterEach(() => {
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
    resetAccessForTests();
  });

  const theirs = (over: Partial<SessionRow> = {}) => live({ owner_person_id: THEM, ...over });

  it('the owner keeps every control in the popover', async () => {
    setMyGrants(ME, []);
    render(SessionRowItem, { props: props(live({ owner_person_id: ME })) });
    const menu = await openMenu();
    await fireEvent.input(screen.getByTestId('work-input'), { target: { value: 'ABC-1' } });
    await tick();
    expect((within(menu).getByTestId('work-set') as HTMLButtonElement).disabled).toBe(false);
    await fireEvent.click(screen.getByTestId('work-set'));
    expect(invoke).toHaveBeenCalledWith('link_session_work', {
      args: { session_id: 7, key: 'ABC-1' },
    });
  });

  it('a watcher cannot open the popover at all, and the row’s y/n write nothing', async () => {
    setMyGrants(ME, [{ session_id: 7, level: 'watch' }]);
    const sess = theirs({ work_suggested: {
        link_id: 3,
        item_id: 3,
        key: 'ABC-2',
        title: 'Pay',
        source: 'branch',
        suggestions: 1,
      } });
    render(SessionRowItem, { props: props(sess) });
    await tick();
    const opener = screen.getByTestId('work-menu') as HTMLButtonElement;
    expect(opener.disabled).toBe(true);
    expect(opener.title).toMatch(/watch is read-only/i);
    // `y` decides the row's top suggestion from the keyboard, past any button.
    const row = screen.getByTestId('sess-row');
    await fireEvent.keyDown(row, { key: 'y' });
    await tick();
    expect(invoke).not.toHaveBeenCalledWith('confirm_session_work', expect.anything());
  });

  it('a grant narrowed while the popover is open stops the write (the handler re-asks)', async () => {
    // Opened as a driver, which is allowed to link…
    setMyGrants(ME, [{ session_id: 7, level: 'drive' }]);
    render(SessionRowItem, { props: props(theirs()) });
    await openMenu();
    await fireEvent.input(screen.getByTestId('work-input'), { target: { value: 'ABC-1' } });
    await tick();
    expect((screen.getByTestId('work-set') as HTMLButtonElement).disabled).toBe(false);
    // …then narrowed to watch with the popover still up.
    applyGrantChanges([{ session_id: 7, person_id: ME, level: 'watch' }]);
    await tick();
    const set = screen.getByTestId('work-set') as HTMLButtonElement;
    expect(set.disabled).toBe(true);
    // The control disabling is half of it; the handler is the other half, so
    // call it the way the keyboard would rather than through the dead button.
    await fireEvent.keyDown(screen.getByTestId('work-input'), { key: 'Enter' });
    await tick();
    expect(invoke).not.toHaveBeenCalledWith('link_session_work', expect.anything());
  });

  it('the popover’s own controls carry the reason for a watcher', async () => {
    // A row with a suggestion and a project, so the popover draws the why-block
    // (Confirm / Not this) and the trust checkbox.
    const sess = {
      ...theirs({ project_id: 4 }),
      work_suggested: {
        link_id: 3,
        item_id: 3,
        key: 'ABC-2',
        title: 'Pay',
        source: 'branch',
        suggestions: 1,
      },
    } as SessionRow;
    vi.mocked(invoke).mockImplementation(async (cmd: string) =>
      cmd === 'session_work_links'
        ? [
            {
              id: 3,
              ref_key: 'ABC-2',
              state: 'suggested',
              source: 'branch',
              created_at: 1,
              is_primary: false,
            },
          ]
        : sess,
    );

    // The owner first, so a gate that disabled these for everyone fails here.
    setMyGrants(ME, []);
    const own = render(SessionRowItem, { props: props({ ...sess, owner_person_id: ME } as SessionRow) });
    await openMenu();
    await tick();
    expect((screen.getByTestId('why-confirm') as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByTestId('why-trust') as HTMLInputElement).disabled).toBe(false);
    own.unmount();

    setMyGrants(ME, [{ session_id: 7, level: 'watch' }]);
    render(SessionRowItem, { props: props(sess) });
    await tick();
    // The popover cannot even be opened at `watch`, so the suggestion chip's own
    // Confirm is where a watcher would land — and it is gated by the same answer.
    expect((screen.getByTestId('work-menu') as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.keyDown(screen.getByTestId('sess-row'), { key: 'n' });
    await tick();
    expect(invoke).not.toHaveBeenCalledWith('reject_session_work', expect.anything());
  });
});
