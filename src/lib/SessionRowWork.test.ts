// The row's work menu (roadmap M1b.2): set a key, "Not this", clear a link.
// Asserts the command each action invokes and its arguments.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import SessionRowItem from './SessionRowItem.svelte';
import { hosts } from './hosts';
import { session } from './hosts_fixture';
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
