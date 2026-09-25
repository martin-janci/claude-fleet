// Work graph M5.4: the org colour bar on a row, and the cross-org link
// refusal explained in the work menu, with "Link anyway".
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import SessionRowItem from './SessionRowItem.svelte';
import { hosts } from './hosts';
import { session } from './hosts_fixture';
import { orgs } from './orgs';
import type { SessionRow } from './sessions';

const noop = () => {};
const live = (over: Partial<SessionRow> = {}): SessionRow =>
  session('mefistos', 'dev-foo', { id: 7, status: 'running', ...over });

function props(sess: SessionRow, extra: Record<string, unknown> = {}) {
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
    ...extra,
  };
}

beforeEach(() => {
  hosts.set([]);
  orgs.set([
    { id: 1, name: 'Company A', created_at: 1, rules: [], hosts: [], trackers: [] },
    { id: 2, name: 'Company B', created_at: 1, rules: [], hosts: [], trackers: [] },
  ]);
  vi.mocked(invoke).mockReset();
});

describe('a session row and its org', () => {
  it('draws the colour bar it is given, and none otherwise', () => {
    const { unmount } = render(SessionRowItem, { props: props(live(), { orgColor: '#ff0000' }) });
    const r = screen.getByTestId('sess-row');
    expect(r.dataset.orgColor).toBe('#ff0000');
    expect(r.style.boxShadow).toContain('3px');
    unmount();
    render(SessionRowItem, { props: props(live()) });
    expect(screen.getByTestId('sess-row').dataset.orgColor).toBeUndefined();
  });

  it('explains a cross-org refusal with org names, and "Link anyway" retries with the override', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string, a?: unknown) => {
      const args = (a as { args: Record<string, unknown> }).args;
      if (cmd === 'link_session_work' && !args.force_cross_org) {
        throw {
          code: 'E_FORBIDDEN',
          message: 'BB-1 belongs to organisation 2 and the session to organisation 1',
          details: { cross_org: true, work_org_id: 2, session_org_id: 1 },
        };
      }
      return live();
    });
    render(SessionRowItem, { props: props(live()) });
    await fireEvent.click(screen.getByTestId('work-menu'));
    await tick();
    await fireEvent.input(screen.getByTestId('work-input'), { target: { value: 'bb-1' } });
    await fireEvent.click(screen.getByTestId('work-set'));
    const note = await screen.findByTestId('cross-org');
    expect(note.textContent).toContain('BB-1 belongs to Company B, and this session to Company A');
    await fireEvent.click(screen.getByTestId('cross-org-force'));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('link_session_work', {
        args: { session_id: 7, key: 'bb-1', force_cross_org: true },
      }),
    );
    await waitFor(() => expect(screen.queryByTestId('cross-org')).toBeNull());
  });
});
