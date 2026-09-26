// "Name this work…" (work graph M11.1): the dialog, the row's menu entry,
// and the optimistic title patch of a rename.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import NameWorkDialog from './NameWorkDialog.svelte';
import SessionRowItem from './SessionRowItem.svelte';
import { hosts } from './hosts';
import { session } from './hosts_fixture';
import { sessions, type SessionRow } from './sessions';
import { LOCAL_WORK_TITLE_MAX, patchWorkItemTitle, workTitleError } from './work';

const live = (over: Partial<SessionRow> = {}): SessionRow =>
  session('mefistos', 'dev-foo', { id: 7, status: 'running', ...over });

const named = (id: number, itemId = 40): SessionRow =>
  live({
    id,
    row_version: 2,
    work: { link_id: 100 + id, item_id: itemId, key: 'OPS', title: 'Ops cleanup', source: 'manual' },
  });

beforeEach(() => {
  hosts.set([]);
  sessions.set([]);
  vi.mocked(invoke).mockReset();
});

async function type(testid: string, value: string) {
  await fireEvent.input(screen.getByTestId(testid), { target: { value } });
  await tick();
}

describe('workTitleError', () => {
  it('mirrors the backend rule', () => {
    expect(workTitleError('  Billing ')).toBeNull();
    expect(workTitleError('   ')).toMatch(/required/);
    expect(workTitleError('x'.repeat(LOCAL_WORK_TITLE_MAX))).toBeNull();
    expect(workTitleError('x'.repeat(LOCAL_WORK_TITLE_MAX + 1))).toMatch(/120/);
    expect(workTitleError('tab\there')).toMatch(/control/);
  });
});

describe('NameWorkDialog', () => {
  it('names work for one session with a title and a key', async () => {
    vi.mocked(invoke).mockResolvedValue(named(7));
    const onclose = vi.fn();
    render(NameWorkDialog, {
      props: { target: { mode: 'name', sessions: [{ id: 7, label: 'dev-foo' }] }, onclose },
    });
    const submit = screen.getByTestId('name-work-submit') as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
    expect(screen.queryByTestId('name-work-sessions')).toBeNull();
    await type('name-work-title', ' Ops cleanup ');
    await type('name-work-key', 'OPS');
    await fireEvent.click(submit);
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(invoke).toHaveBeenCalledWith('name_session_work', {
      args: { session_id: 7, title: 'Ops cleanup', key: 'OPS' },
    });
    // The optimistic patch: the command's row is in the store at once.
    expect(get(sessions).find((s) => s.id === 7)?.work?.title).toBe('Ops cleanup');
  });

  it('leaves the key out when blank, and blocks an invalid title', async () => {
    vi.mocked(invoke).mockResolvedValue(named(7));
    render(NameWorkDialog, {
      props: { target: { mode: 'name', sessions: [{ id: 7, label: 'dev-foo' }] }, onclose: vi.fn() },
    });
    await type('name-work-title', 'x'.repeat(LOCAL_WORK_TITLE_MAX + 1));
    expect(screen.getByTestId('name-work-title-error').textContent).toMatch(/120/);
    expect((screen.getByTestId('name-work-submit') as HTMLButtonElement).disabled).toBe(true);
    await type('name-work-title', 'Short');
    await fireEvent.click(screen.getByTestId('name-work-submit'));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('name_session_work', {
        args: { session_id: 7, title: 'Short' },
      }),
    );
  });

  it('shows a refusal and stays open', async () => {
    vi.mocked(invoke).mockRejectedValue({
      code: 'E_EXISTS',
      message: 'BB-1 is a tracker’s ticket, not new work',
    });
    const onclose = vi.fn();
    render(NameWorkDialog, {
      props: { target: { mode: 'name', sessions: [{ id: 7, label: 'dev-foo' }] }, onclose },
    });
    await type('name-work-title', 'Dup');
    await type('name-work-key', 'BB-1');
    await fireEvent.click(screen.getByTestId('name-work-submit'));
    await waitFor(() => expect(screen.getByTestId('name-work-error').textContent).toMatch(/ticket/));
    expect(onclose).not.toHaveBeenCalled();
  });

  it('a group names once, then links the chosen sessions to the new item', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd, a) => {
      const args = (a as { args: { session_id: number } }).args;
      return cmd === 'name_session_work' ? named(args.session_id) : named(args.session_id);
    });
    render(NameWorkDialog, {
      props: {
        target: {
          mode: 'name',
          sessions: [
            { id: 7, label: 'a' },
            { id: 8, label: 'b' },
            { id: 9, label: 'c' },
          ],
        },
        onclose: vi.fn(),
      },
    });
    const boxes = screen.getAllByTestId('name-work-session') as HTMLInputElement[];
    expect(boxes.map((b) => b.checked)).toEqual([true, true, true]);
    await fireEvent.change(boxes[1]);
    await type('name-work-title', 'Ops cleanup');
    await fireEvent.click(screen.getByTestId('name-work-submit'));
    await waitFor(() => expect(invoke).toHaveBeenCalledTimes(2));
    expect(invoke).toHaveBeenNthCalledWith(1, 'name_session_work', {
      args: { session_id: 7, title: 'Ops cleanup' },
    });
    expect(invoke).toHaveBeenNthCalledWith(2, 'link_session_work', {
      args: { session_id: 9, item_id: 40 },
    });
  });

  it('renames a local item and patches every row that shows it', async () => {
    sessions.set([named(7), named(8), named(9, 41)]);
    vi.mocked(invoke).mockResolvedValue({
      id: 40,
      source: 'local',
      key: 'OPS',
      title: 'Ops, renamed',
      status_category: 'todo',
      created_at: 1,
      updated_at: 2,
    });
    render(NameWorkDialog, {
      props: {
        target: { mode: 'rename', itemId: 40, title: 'Ops cleanup', key: 'OPS' },
        onclose: vi.fn(),
      },
    });
    expect(screen.queryByTestId('name-work-key')).toBeNull();
    expect((screen.getByTestId('name-work-title') as HTMLInputElement).value).toBe('Ops cleanup');
    await type('name-work-title', 'Ops, renamed');
    await fireEvent.click(screen.getByTestId('name-work-submit'));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('rename_work_item', {
        args: { item_id: 40, title: 'Ops, renamed' },
      }),
    );
    const titles = get(sessions).map((s) => [s.id, s.work?.title]);
    expect(titles).toEqual([
      [7, 'Ops, renamed'],
      [8, 'Ops, renamed'],
      [9, 'Ops cleanup'],
    ]);
  });
});

describe('patchWorkItemTitle', () => {
  it('leaves the store untouched when no row shows the item', () => {
    const before = [named(7)];
    sessions.set(before);
    patchWorkItemTitle(99, 'x');
    expect(get(sessions)).toBe(before);
  });
});

describe('the row work menu', () => {
  const noop = () => {};
  const props = (sess: SessionRow) => ({
    sess,
    selectMode: false,
    isChecked: false,
    isRenaming: false,
    renameValue: '',
    renameInput: undefined,
    renameError: null,
    relatedCount: 0,
    nowSec: Math.floor(Date.now() / 1000),
    workKey: null,
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
  });

  async function openMenu() {
    await fireEvent.click(screen.getByTestId('work-menu'));
    await tick();
  }

  it('"Name this work…" opens the dialog for the row', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd) => (cmd === 'session_work_links' ? [] : named(7)));
    const p = props(live());
    render(SessionRowItem, { props: p });
    await openMenu();
    expect(screen.queryByTestId('work-rename')).toBeNull();
    await fireEvent.click(screen.getByTestId('work-name'));
    await tick();
    expect(screen.getByTestId('name-work-dialog')).toBeTruthy();
    expect(screen.queryByTestId('work-menu-panel')).toBeNull();
    expect(p.onSelectSession).not.toHaveBeenCalled();
    await type('name-work-title', 'Ops cleanup');
    await fireEvent.click(screen.getByTestId('name-work-submit'));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('name_session_work', {
        args: { session_id: 7, title: 'Ops cleanup' },
      }),
    );
  });

  it('offers Rename… for a local item only, never for a ticket', async () => {
    vi.mocked(invoke).mockResolvedValue([]);
    const { unmount } = render(SessionRowItem, { props: props(named(7)) });
    await openMenu();
    await fireEvent.click(screen.getByTestId('work-rename'));
    await tick();
    expect((screen.getByTestId('name-work-title') as HTMLInputElement).value).toBe('Ops cleanup');
    unmount();

    const ticket = live({
      work: {
        link_id: 1,
        item_id: 5,
        key: 'ABC-1',
        title: 'Login',
        source: 'manual',
        status_category: 'todo',
        url: 'https://x.atlassian.net/browse/ABC-1',
      },
    });
    render(SessionRowItem, { props: props(ticket) });
    await openMenu();
    expect(screen.getByTestId('work-name')).toBeTruthy();
    expect(screen.queryByTestId('work-rename')).toBeNull();
  });
});
