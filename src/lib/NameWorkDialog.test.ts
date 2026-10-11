// "Name this work…" (work graph M11.1): the dialog, the row's menu entry,
// and the optimistic title patch of a rename.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { get } from 'svelte/store';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import NameWorkDialog from './NameWorkDialog.svelte';
import SessionRowItem from './SessionRowItem.svelte';
import { hosts } from './hosts';
import { session } from './hosts_fixture';
import { sessions, type SessionRow } from './sessions';
import { LOCAL_WORK_TITLE_MAX, branchMates, patchWorkItemTitle, workTitleError } from './work';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import { resetAccessForTests, setMyGrants } from './access';

const live = (over: Partial<SessionRow> = {}): SessionRow =>
  session('mefistos', 'dev-foo', { id: 7, status: 'running', ...over });

const named = (id: number, itemId = 40): SessionRow =>
  live({
    id,
    row_version: 2,
    // `kind: 'local'` is the current wire (native item status, fix round
    // 2): a current hub sends this, `status_category` stays null for a
    // local item either way (wire compat with a shipped phone build).
    work: {
      link_id: 100 + id,
      item_id: itemId,
      key: 'OPS',
      title: 'Ops cleanup',
      source: 'manual',
      kind: 'local',
    },
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

  it("prefills the session agent's name as a draft, marked Drafted and undoable (G2.1, G7.6)", async () => {
    sessions.set([live({ friendly_name: 'Receipt rounding fix' })]);
    vi.mocked(invoke).mockResolvedValue(named(7));
    render(NameWorkDialog, {
      props: { target: { mode: 'name', sessions: [{ id: 7, label: 'dev-foo' }] }, onclose: vi.fn() },
    });
    await tick();
    // Prefilled as the board shows it, labelled Drafted.
    const input = screen.getByTestId('name-work-title') as HTMLInputElement;
    expect(input.value).toBe('Receipt rounding fix');
    expect(screen.getByTestId('name-work-drafted-label').textContent).toBe('Drafted');
    expect(invoke).not.toHaveBeenCalled();
    // Undo empties the field; the draft is offered again as a link.
    await fireEvent.click(screen.getByTestId('name-work-draft-undo'));
    await tick();
    expect(input.value).toBe('');
    expect(screen.queryByTestId('name-work-drafted')).toBeNull();
    // Typed, then the draft used: Undo puts back what the person had typed.
    await type('name-work-title', 'Mine');
    await fireEvent.click(screen.getByTestId('name-work-use-draft'));
    await tick();
    expect(input.value).toBe('Receipt rounding fix');
    await fireEvent.click(screen.getByTestId('name-work-draft-undo'));
    await tick();
    expect(input.value).toBe('Mine');
    // Used, then edited: it is the person's now, no pill.
    await fireEvent.click(screen.getByTestId('name-work-use-draft'));
    await tick();
    await type('name-work-title', 'Receipt rounding');
    expect(screen.queryByTestId('name-work-drafted')).toBeNull();
    await fireEvent.click(screen.getByTestId('name-work-submit'));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('name_session_work', { args: { session_id: 7, title: 'Receipt rounding' } }),
    );
  });

  it('a rename is never prefilled from a draft', async () => {
    sessions.set([live({ friendly_name: 'Receipt rounding fix' })]);
    render(NameWorkDialog, {
      props: { target: { mode: 'rename', itemId: 40, title: 'Ops cleanup' }, onclose: vi.fn() },
    });
    await tick();
    expect((screen.getByTestId('name-work-title') as HTMLInputElement).value).toBe('Ops cleanup');
    expect(screen.queryByTestId('name-work-drafted')).toBeNull();
  });

  it('also names the other sessions on the branch, ticked, and leaves them out when unticked (G7.6)', async () => {
    vi.mocked(invoke).mockImplementation(async (_cmd, a) => named((a as { args: { session_id: number } }).args.session_id));
    const target = {
      mode: 'name' as const,
      sessions: [{ id: 7, label: 'dev-foo' }],
      branchMates: [
        { id: 8, label: 'dev-foo-2' },
        { id: 9, label: 'dev-foo-3' },
      ],
    };
    const { unmount } = render(NameWorkDialog, { props: { target, onclose: vi.fn() } });
    const box = screen.getByTestId('name-work-branch-mates') as HTMLInputElement;
    expect(box.checked).toBe(true);
    expect(box.parentElement?.textContent).toMatch(/Also name the 2 other sessions on this branch/);
    await type('name-work-title', 'Ops cleanup');
    await fireEvent.click(screen.getByTestId('name-work-submit'));
    await waitFor(() => expect(invoke).toHaveBeenCalledTimes(3));
    expect(invoke).toHaveBeenNthCalledWith(1, 'name_session_work', { args: { session_id: 7, title: 'Ops cleanup' } });
    expect(invoke).toHaveBeenNthCalledWith(2, 'link_session_work', { args: { session_id: 8, item_id: 40 } });
    expect(invoke).toHaveBeenNthCalledWith(3, 'link_session_work', { args: { session_id: 9, item_id: 40 } });
    unmount();

    vi.mocked(invoke).mockClear();
    render(NameWorkDialog, { props: { target, onclose: vi.fn() } });
    await fireEvent.click(screen.getByTestId('name-work-branch-mates'));
    await type('name-work-title', 'Ops cleanup');
    await fireEvent.click(screen.getByTestId('name-work-submit'));
    await waitFor(() => expect(invoke).toHaveBeenCalledTimes(1));
    expect(invoke).toHaveBeenCalledWith('name_session_work', { args: { session_id: 7, title: 'Ops cleanup' } });
  });

  it('branchMates: same host and checkout, live, with no work yet', () => {
    const me = live({ id: 1, worktree_id: 5 });
    const all = [
      me,
      live({ id: 2, worktree_id: 5 }),
      live({ id: 3, worktree_id: 6 }),
      live({ id: 4, worktree_id: 5, lost_at: 100 }),
      named(5),
      { ...live({ id: 6, worktree_id: 5 }), host_alias: 'other' },
    ].map((r) => (r.id === 5 ? { ...r, worktree_id: 5 } : r));
    expect(branchMates(me, all).map((r) => r.id)).toEqual([2]);
    expect(branchMates(live({ id: 1, worktree_id: null }), all)).toEqual([]);
  });

  it('offers no draft when the session has no agent name', async () => {
    sessions.set([live({ friendly_name: null })]);
    render(NameWorkDialog, {
      props: { target: { mode: 'name', sessions: [{ id: 7, label: 'dev-foo' }] }, onclose: vi.fn() },
    });
    expect(screen.queryByTestId('name-work-use-draft')).toBeNull();
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

  it('"Name this work…" on a row offers the other sessions on its branch (G7.6)', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd) => (cmd === 'session_work_links' ? [] : named(7)));
    const me = live({ worktree_id: 5 });
    sessions.set([me, live({ id: 8, tmux_name: 'dev-foo-2', worktree_id: 5 }), live({ id: 9, worktree_id: 6 })]);
    render(SessionRowItem, { props: props(me) });
    await openMenu();
    await fireEvent.click(screen.getByTestId('work-name'));
    await tick();
    const box = screen.getByTestId('name-work-branch-mates') as HTMLInputElement;
    expect(box.parentElement?.textContent).toMatch(/Also name the other session on this branch/);
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
        kind: 'tracker',
        status_category: 'todo',
        url: 'https://x.atlassian.net/browse/ABC-1',
      },
    });
    render(SessionRowItem, { props: props(ticket) });
    await openMenu();
    expect(screen.getByTestId('work-name')).toBeTruthy();
    expect(screen.queryByTestId('work-rename')).toBeNull();
  });

  // The branch's only back-compat branch, and the one thing every other
  // fixture here stops covering by setting `kind`: a hub too old to send
  // `kind` at all. Without this case, deleting the `status_category == null
  // && !url` fallback in `SessionRowItem.svelte` leaves the suite green.
  it('falls back to the old heuristic for a hub that sends no kind', async () => {
    vi.mocked(invoke).mockResolvedValue([]);
    const oldHub = live({
      work: {
        link_id: 3,
        item_id: 41,
        key: 'OPS',
        title: 'Ops cleanup',
        source: 'manual',
        // No `kind`, and no `status_category` / `url` — an old hub's local
        // item, which followed the same tracker-only rule the fallback
        // assumes.
      },
    });
    render(SessionRowItem, { props: props(oldHub) });
    await openMenu();
    await fireEvent.click(screen.getByTestId('work-rename'));
    await tick();
    expect((screen.getByTestId('name-work-title') as HTMLInputElement).value).toBe('Ops cleanup');
  });

  // …and the other half of the fallback: an old hub's TICKET, which that
  // hub does send a `status_category` (or a `url`) for.
  it('a kind-less ticket is still not offered Rename…', async () => {
    vi.mocked(invoke).mockResolvedValue([]);
    const oldHubTicket = live({
      work: {
        link_id: 4,
        item_id: 42,
        key: 'ABC-3',
        title: 'Login',
        source: 'manual',
        status_category: 'todo',
      },
    });
    render(SessionRowItem, { props: props(oldHubTicket) });
    await openMenu();
    expect(screen.getByTestId('work-name')).toBeTruthy();
    expect(screen.queryByTestId('work-rename')).toBeNull();
  });

  it('kind, not the absence of status_category, decides Rename… (fix round 2)', async () => {
    vi.mocked(invoke).mockResolvedValue([]);
    // A tracker item that has not synced a status yet: `kind` says
    // tracker, but `status_category` and `url` are both absent — exactly
    // what the OLD "is status_category/url missing?" heuristic would have
    // read as a local item. `kind` must win.
    const notYetSynced = live({
      work: {
        link_id: 2,
        item_id: 6,
        key: 'ABC-2',
        title: 'Not synced yet',
        source: 'manual',
        kind: 'tracker',
      },
    });
    render(SessionRowItem, { props: props(notYetSynced) });
    await openMenu();
    expect(screen.getByTestId('work-name')).toBeTruthy();
    expect(screen.queryByTestId('work-rename')).toBeNull();
  });
});

// Multi-user M1, F2b: the dialog is handed session IDS and wrote without
// asking either half. It resolves the rows out of the store and narrows per
// target — and re-asks, since it stays open across a narrowing. `rename` mode
// has no session at all, so its answer arrives as the `accessBlocked` prop.
describe('NameWorkDialog access gate (multi-user M1)', () => {
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

  async function type(title: string) {
    await fireEvent.input(screen.getByTestId('name-work-title'), { target: { value: title } });
    await tick();
  }

  it('names work only for the ticked sessions this client may drive', async () => {
    const mine = live({ id: 1, owner_person_id: ME });
    const theirs = live({ id: 2, owner_person_id: THEM, tmux_name: 'dev-theirs' });
    sessions.set([mine, theirs]);
    setMyGrants(ME, [{ session_id: 2, level: 'watch' }]);
    // The write answers a named row; without it `nameWorkForSessions` reads
    // `work` off nothing and leaves an unhandled rejection in the run.
    vi.mocked(invoke).mockResolvedValue(named(1));
    render(NameWorkDialog, {
      props: {
        target: {
          mode: 'name',
          sessions: [
            { id: 1, label: 'dev-foo' },
            { id: 2, label: 'dev-theirs' },
          ],
        },
        onclose: vi.fn(),
      },
    });
    await tick();
    expect(screen.getByTestId('name-work-not-mine').textContent).toMatch(/1 of the ticked/);
    await type('Ops cleanup');
    await fireEvent.click(screen.getByTestId('name-work-submit'));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('name_session_work', {
        args: { session_id: 1, title: 'Ops cleanup' },
      }),
    );
    // The watched session is never linked to the new item either.
    expect(invoke).not.toHaveBeenCalledWith('link_session_work', expect.anything());
  });

  it('refuses outright when no ticked session is this client’s', async () => {
    const theirs = live({ id: 2, owner_person_id: THEM });
    sessions.set([theirs]);
    setMyGrants(ME, [{ session_id: 2, level: 'watch' }]);
    render(NameWorkDialog, {
      props: {
        target: { mode: 'name', sessions: [{ id: 2, label: 'dev-theirs' }] },
        onclose: vi.fn(),
      },
    });
    await tick();
    await type('Ops cleanup');
    const submit = screen.getByTestId('name-work-submit') as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
    expect(screen.getByTestId('name-work-blocked').textContent).toMatch(/watch is read-only/i);
    await fireEvent.click(submit);
    await tick();
    expect(invoke).not.toHaveBeenCalledWith('name_session_work', expect.anything());
  });

  it('the owner keeps it', async () => {
    const mine = live({ id: 1, owner_person_id: ME });
    sessions.set([mine]);
    setMyGrants(ME, []);
    render(NameWorkDialog, {
      props: { target: { mode: 'name', sessions: [{ id: 1, label: 'dev-foo' }] }, onclose: vi.fn() },
    });
    await tick();
    await type('Ops cleanup');
    expect((screen.getByTestId('name-work-submit') as HTMLButtonElement).disabled).toBe(false);
    expect(screen.queryByTestId('name-work-blocked')).toBeNull();
  });

  it('rename mode takes its answer from the caller, which holds the row', async () => {
    sessions.set([]);
    setMyGrants(ME, []);
    render(NameWorkDialog, {
      props: {
        target: { mode: 'rename', itemId: 40, title: 'Ops cleanup' },
        onclose: vi.fn(),
        accessBlocked: 'Shared with you to watch. Watch is read-only.',
      },
    });
    await tick();
    await type('Something else');
    const submit = screen.getByTestId('name-work-submit') as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
    expect(screen.getByTestId('name-work-blocked').textContent).toMatch(/read-only/i);
    await fireEvent.click(submit);
    await tick();
    expect(invoke).not.toHaveBeenCalledWith('rename_work_item', expect.anything());
  });
});
