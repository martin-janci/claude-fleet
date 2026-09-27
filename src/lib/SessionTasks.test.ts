// Session detail's Tasks section (work graph M14.2): every link, drawn by
// kind; Show in Work view; re-read only when the row's links move; nothing
// at all on an older hub.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import SessionTasks from './SessionTasks.svelte';
import { session } from './hosts_fixture';
import { sidebarMode } from './work_tree';
import type { SessionRow } from './sessions';

function brief(id: string, key: string) {
  return { task_id: id, key, title: `Title ${key}`, kind: 'tracker', unavailable: false };
}
function lk(link_id: number, state: string, primary: boolean, id: string, key: string) {
  return { link_id, link_version: 1, state, primary, session_id: 41, name: 'api', source: 'manual', why: 'branch', created_at: 1, needs_you: false, archived: false, resumable: true, cross_org: false, other_tasks: 0, task: brief(id, key) };
}

async function flush() {
  for (let i = 0; i < 8; i++) await tick();
}

describe('SessionTasks', () => {
  beforeEach(() => {
    sidebarMode.set('sessions');
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_session_tasks')
        return {
          session_id: 41,
          primary_link_id: 1,
          links: [
            lk(3, 'ended', false, 'item:3', 'ABC-3'),
            lk(2, 'suggested', false, 'item:2', 'ABC-2'),
            lk(1, 'active', true, 'item:1', 'ABC-1'),
            lk(4, 'active', false, 'ref:XY-9', 'XY-9'),
          ],
        };
      if (cmd === 'work_tree') throw { code: 'E_INVALID', message: 'unknown work action' };
      return null;
    });
  });

  it('lists every link by kind, primary first', async () => {
    render(SessionTasks, { props: { session: session('mefistos', 'api', { id: 41 }) } });
    await flush();
    const rows = screen.getAllByTestId('session-task');
    expect(rows.map((r) => r.getAttribute('data-kind'))).toEqual(['primary', 'secondary', 'suggested', 'past']);
    expect(rows[0].textContent).toContain('ABC-1');
    expect(rows[0].textContent).toContain('★');
  });

  it('Show in Work view switches the sidebar to Work', async () => {
    render(SessionTasks, { props: { session: session('mefistos', 'api', { id: 41 }) } });
    await flush();
    await fireEvent.click(screen.getAllByTestId('show-in-work-view')[0]);
    expect(get(sidebarMode)).toBe('work');
  });

  it('re-reads only when the links move, not on every row update', async () => {
    const s = session('mefistos', 'api', { id: 41 });
    const { rerender } = render(SessionTasks, { props: { session: s } });
    await flush();
    const reads = () => vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'work_session_tasks').length;
    expect(reads()).toBe(1);
    await rerender({ session: { ...s, claude_status: 'working' } as SessionRow });
    await flush();
    expect(reads()).toBe(1);
    await rerender({ session: { ...s, work: { link_id: 9, item_id: 1, key: 'ABC-1', title: '', source: 'manual' } } as SessionRow });
    await flush();
    expect(reads()).toBe(2);
  });

  it('is absent on a hub without the Work view', async () => {
    vi.mocked(invoke).mockImplementation(async () => {
      throw { code: 'E_INVALID', message: 'unknown work action "session_tasks"' };
    });
    render(SessionTasks, { props: { session: session('mefistos', 'api', { id: 41 }) } });
    await flush();
    expect(screen.queryByTestId('session-tasks')).toBeNull();
  });
});
