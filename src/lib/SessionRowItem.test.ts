import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { uiDensity } from './prefs';
import SessionRowItem from './SessionRowItem.svelte';
import { hosts } from './hosts';
import { host, session } from './hosts_fixture';
import { hubStatus, STANDALONE } from './hub';
import { applyGrantChanges, resetAccessForTests, setMyGrants } from './access';
import type { SessionRow } from './sessions';
import { expectAccessible } from './a11y_check';

// Built from the shared fixture rather than a hand-written literal: this
// file's own copy fell behind `SessionRow` twice (`model`, the context block,
// `pending_input`), and a stale literal only fails at type-check time.
const sampleSession: SessionRow = session('mefistos', 'dev-foo', {
  id: 1,
  status: 'ghost',
  claude_status: null,
  lost_at: 1,
  lost_reason: null,
  turn_seq: 0,
  last_stop_at: null,
});

const noop = () => {};

function baseProps(sess: SessionRow) {
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

beforeEach(() => {
  uiDensity.set('comfortable');
  hosts.set([]);
  hubStatus.set({ ...STANDALONE });
  resetAccessForTests();
});

describe('SessionRowItem ghost row lost_reason', () => {
  it('shows "host rebooted" for a lost_reason of host_reboot', async () => {
    render(SessionRowItem, {
      props: baseProps({ ...sampleSession, lost_reason: 'host_reboot' }),
    });
    await tick();
    const label = await screen.findByTestId('lost-reason');
    expect(label.textContent).toContain('host rebooted');
  });

  it('shows "tmux server stopped" for a lost_reason of tmux_server_gone', async () => {
    render(SessionRowItem, {
      props: baseProps({ ...sampleSession, lost_reason: 'tmux_server_gone' }),
    });
    await tick();
    const label = await screen.findByTestId('lost-reason');
    expect(label.textContent).toContain('tmux server stopped');
  });

  it('renders no lost-reason element when lost_reason is null', async () => {
    render(SessionRowItem, {
      props: baseProps({ ...sampleSession, lost_reason: null }),
    });
    await tick();
    expect(screen.queryByTestId('lost-reason')).toBeNull();
  });
});

// ── Multi-user M1 (F2): the privacy badge and the per-row action gate ───────
//
// The badge is read straight off the row (`visibility` + whether an owner came
// with it), so it says the same thing to every viewer. The action gate is the
// opposite: it is DERIVED from the row plus this client's own person id and
// grant set (`access.ts`), because no per-caller field rides a `SessionRow` —
// which is exactly why the last test here can narrow a grant and watch the
// buttons change with no `session:updated` and no re-list at all.
describe('SessionRowItem privacy badge', () => {
  const live = (over: Partial<SessionRow> = {}): SessionRow =>
    session('mefistos', 'dev-foo', { id: 500, status: 'running', ...over });

  it('names a private session private, and an unclaimed one unclaimed', async () => {
    const { unmount } = render(SessionRowItem, {
      props: baseProps(live({ visibility: 'private', owner_person_id: 1 })),
    });
    await tick();
    expect(screen.getByTestId('privacy-chip').textContent).toContain('private');
    unmount();

    render(SessionRowItem, {
      props: baseProps(live({ visibility: 'unclaimed', owner_person_id: null })),
    });
    await tick();
    const chip = screen.getByTestId('privacy-chip');
    expect(chip.textContent).toContain('unclaimed');
    expect(chip.title).toMatch(/per-host count/i);
  });

  it('renders no badge for a hub that sends neither field, nor for a private row with no owner', async () => {
    // Two different absences, one answer. The second is the `strip_nulls` case:
    // a null `owner_person_id` is REMOVED on the way out, so "private with no
    // owner" is a shape the wire can produce and cannot be described honestly.
    const { unmount } = render(SessionRowItem, { props: baseProps(live()) });
    await tick();
    expect(screen.queryByTestId('privacy-chip')).toBeNull();
    unmount();

    render(SessionRowItem, { props: baseProps(live({ visibility: 'private' })) });
    await tick();
    expect(screen.queryByTestId('privacy-chip')).toBeNull();
  });
});

describe('SessionRowItem action gate (multi-user M1)', () => {
  const live = (over: Partial<SessionRow> = {}): SessionRow =>
    session('mefistos', 'dev-foo', {
      id: 501,
      status: 'running',
      visibility: 'private',
      owner_person_id: 1,
      ...over,
    });
  const dis = (testid: string) => (screen.getByTestId(testid) as HTMLButtonElement).disabled;

  beforeEach(() => {
    hosts.set([host('mefistos')]);
    // A paired desktop: standalone short-circuits to `own` on the backend mode
    // alone, so the gate would never be exercised there — which is the point of
    // that rule, and why single-user installs are untouched by all of this.
    hubStatus.set({ ...STANDALONE, remote: true, url: 'https://fleet.example.com' });
    resetAccessForTests();
  });

  it('leaves every action alone for the owner', async () => {
    setMyGrants(1, []);
    render(SessionRowItem, { props: baseProps(live()) });
    await tick();
    for (const t of ['restart-session', 'rename-tmux', 'recreate-live', 'edit-label', 'work-menu']) {
      expect(dis(t), t).toBe(false);
    }
  });

  it('a driver works the session but cannot dispose of it', async () => {
    setMyGrants(9, [{ session_id: 501, level: 'drive' }]);
    render(SessionRowItem, { props: baseProps(live()) });
    await tick();
    // `own` tier (spec §4.3 invariant 5): restart, rename and recreate.
    expect(dis('restart-session')).toBe(true);
    expect(dis('rename-tmux')).toBe(true);
    expect(dis('recreate-live')).toBe(true);
    expect(screen.getByTestId('restart-session').title).toMatch(/only the session’s owner/i);
    // `drive` tier: the label and the work menu are row writes.
    expect(dis('edit-label')).toBe(false);
    expect(dis('work-menu')).toBe(false);
  });

  /** A row Claude is asking a question on, which is what puts the inline
   *  answer card (a pane write) on screen. */
  const asking = (over: Partial<SessionRow> = {}) =>
    live({
      claude_status: 'blocked',
      pending_input: {
        kind: 'permission' as const,
        question: 'Do you want to proceed?',
        options: [
          { n: 1, label: 'Yes', selected: true },
          { n: 2, label: 'No', selected: false },
        ],
      },
      ...over,
    });

  it('a watcher gets no write at all, and no answer card', async () => {
    setMyGrants(9, [{ session_id: 501, level: 'watch' }]);
    render(SessionRowItem, { props: baseProps(asking()) });
    await tick();
    for (const t of ['restart-session', 'rename-tmux', 'recreate-live', 'edit-label', 'work-menu']) {
      expect(dis(t), t).toBe(true);
    }
    expect(screen.getByTestId('edit-label').title).toMatch(/needs drive/i);
    // The status chip still says the session is waiting — what a watcher does
    // not get is the buttons that answer for the owner, which are a pane write
    // by another name.
    expect(screen.getByTestId('claude-chip')).toBeInTheDocument();
    expect(screen.queryByTestId('answer-card')).toBeNull();
  });

  it('a session waiting on a form shows the row chip', async () => {
    render(SessionRowItem, { props: baseProps(live({ pending_form: { form_id: 'f_a', title: 'Deploy' } })) });
    await tick();
    expect(screen.getByTestId('row-form-chip')).toBeInTheDocument();
  });

  it('the answer card is there for the owner and for a driver', async () => {
    setMyGrants(1, []);
    const { unmount } = render(SessionRowItem, { props: baseProps(asking()) });
    await tick();
    expect(screen.getByTestId('answer-card')).toBeInTheDocument();
    unmount();

    setMyGrants(9, [{ session_id: 501, level: 'drive' }]);
    render(SessionRowItem, { props: baseProps(asking()) });
    await tick();
    expect(screen.getByTestId('answer-card')).toBeInTheDocument();
  });

  it('a Compact row names the question and leaves the card to the conversation (UX audit L2)', async () => {
    setMyGrants(1, []);
    uiDensity.set('compact');
    render(SessionRowItem, { props: baseProps(asking()) });
    await tick();
    expect(screen.queryByTestId('answer-card')).toBeNull();
    expect(screen.getByTestId('meta-state').textContent).toBe('Waiting for you: Do you want to proceed?');
  });

  it('a grant:changed narrowing drive to watch re-disables the drive actions, with no row event', async () => {
    // The assertion that would have failed under a per-caller field on the row:
    // a narrow moves NO column on any session, so nothing a `session:updated`
    // could carry has changed. The only thing that moved is the grant map.
    setMyGrants(9, [{ session_id: 501, level: 'drive' }]);
    const row = live();
    render(SessionRowItem, { props: baseProps(row) });
    await tick();
    expect(dis('edit-label')).toBe(false);

    applyGrantChanges([{ session_id: 501, person_id: 9, level: 'watch' }]);
    await tick();
    expect(dis('edit-label')).toBe(true);

    // And a revoke takes the rest.
    applyGrantChanges([{ session_id: 501, person_id: 9, level: null }]);
    await tick();
    expect(dis('work-menu')).toBe(true);
    expect(screen.getByTestId('work-menu').title).toMatch(/belongs to someone else/i);
  });
});

describe('SessionRowItem accessibility (7.2)', () => {
  it('passes the axe and audit checks, live and ghost', async () => {
    for (const status of ['running', 'ghost'] as const) {
      // The row is a treeitem: it is checked inside the tree the Sidebar
      // puts it in.
      const tree = document.body.appendChild(document.createElement('div'));
      tree.setAttribute('role', 'tree');
      tree.setAttribute('aria-label', 'Sessions');
      const { unmount } = render(SessionRowItem, { target: tree, props: baseProps({ ...sampleSession, status }) });
      await tick();
      await expectAccessible(tree);
      unmount();
      tree.remove();
    }
  });

  it('says the status in words: the dot is a labelled image', async () => {
    render(SessionRowItem, { props: baseProps({ ...sampleSession, status: 'running' }) });
    await tick();
    expect(screen.getByRole('img', { name: 'Status: running' })).toBeTruthy();
  });
});

describe('SessionRowItem: a session ⌘N just started (step 5.14)', () => {
  it('shows the Pulse sequence on the new row, on its agent step, and nothing else waits on it', async () => {
    const { sessions, startedIds } = await import('./sessions');
    const { resetStarting } = await import('./session_starting');
    const row = session('mercury', 'pd-3011', { id: 41, status: 'running', claude_status: null, lost_at: null });
    sessions.set([row]);
    startedIds.set(new Set([41]));
    try {
      const { unmount } = render(SessionRowItem, { props: baseProps(row) });
      await tick();
      expect(screen.getByTestId('row-starting').textContent).toContain('Worktree ✓ · tmux ✓ · Claude Code starting');
      const sats = screen.getAllByTestId('pulse-sat').map((s) => s.getAttribute('data-state'));
      expect(sats).toEqual(['done', 'done', 'active']);
      // The pulse is the one thing animating on the row.
      expect(document.querySelectorAll('.ofl, .sat--active').length).toBe(1);
      unmount();

      // The agent reports: the row is an ordinary row again.
      const up = { ...row, claude_status: 'working' as const };
      sessions.set([up]);
      render(SessionRowItem, { props: baseProps(up) });
      await tick();
      expect(screen.queryByTestId('row-starting')).toBeNull();
    } finally {
      resetStarting();
      sessions.set([]);
    }
  });
});

describe('SessionRowItem label (G2.7)', () => {
  it('shows the session tags as its label in the row', async () => {
    render(SessionRowItem, {
      props: baseProps({ ...sampleSession, status: 'running', lost_at: null, tags: ['release'] } as SessionRow),
    });
    await tick();
    expect(screen.getAllByTestId('row-label').map((e) => e.textContent)).toEqual(['release']);
  });

  it('shows none without tags', async () => {
    render(SessionRowItem, {
      props: baseProps({ ...sampleSession, status: 'running', lost_at: null, tags: [] } as SessionRow),
    });
    await tick();
    expect(screen.queryByTestId('row-label')).toBeNull();
  });
});
